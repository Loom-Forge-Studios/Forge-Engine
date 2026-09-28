#!/usr/bin/env bash
# Runs INSIDE the forge-linux-verify container (ADR 0044). Started by verify-linux.ps1 /
# verify-linux.sh, never by hand. Layout of the worktree's persistent volume
# (`forge-linux-target-<name>-<hash>`, one per worktree):
#
#   /vol/src         the worktree snapshot the build runs on (LF line endings, as CI checks out)
#   /vol/target      CARGO_TARGET_DIR: incremental across runs
#   /vol/cargo-home  CARGO_HOME: extracted crate sources and the package-cache lock; its
#                    registry/cache and registry/index are the HOST's, bind-mounted
#
# and /lock, the machine-wide `forge-linux-lock` volume: one Linux verify at a time.
#
# Arguments: the command to run in /vol/src (default: `just verify`).
set -euo pipefail

# 0. The toolchain must be the bytes the image was built with (see the Dockerfile): a page
#    cache corrupted in the WSL2 VM made every rustc codegen SIGSEGV on 2026-09-23.
if ! sha256sum --quiet -c /etc/forge-toolchain.sha256; then
    echo "linux-verify: the toolchain's bytes differ from the image build: the VM's page cache" >&2
    echo "linux-verify: (or disk) is corrupted, and compiler crashes would follow. Restart the" >&2
    echo "linux-verify: Docker/WSL2 VM (wsl --shutdown), then run again." >&2
    exit 3
fi

# 0b. One Linux verify at a time on this machine: two containers building and testing at once
#     load the VM's CPUs under each other's timed tests. The lock is held (fd 9, inherited by
#     the command) until the container exits; a second run waits and says so.
if [ -d /lock ]; then
    exec 9>/lock/verify.lock
    if ! flock -n 9; then
        echo "linux-verify: another Linux verify is running on this machine; waiting for it to finish"
        flock 9
    fi
else
    echo "linux-verify: no /lock mount (an old host script?): not serialised with other runs" >&2
fi

SNAPSHOT=/snapshot.tar
STAGE=/vol/stage
SRC=/vol/src
CACHE_DIR="$CARGO_HOME/registry/cache/index.crates.io-1949cf8c6b5b557f"

# 1. The snapshot: exactly the files `git add -A` would commit, with LF endings. rsync
#    --checksum rewrites only files whose bytes changed, so unchanged files keep their mtime
#    and cargo's fingerprints stay fresh: rebuilds are incremental.
rm -rf "$STAGE"
mkdir -p "$STAGE" "$SRC"
tar -xf "$SNAPSHOT" -C "$STAGE"
rsync -rlpc --delete "$STAGE/" "$SRC/"
rm -rf "$STAGE"
cd "$SRC"

# 2. Crates: every crate the Linux build needs comes from the host's cache. Only a crate the
#    host never downloaded (a Linux-only dependency) is fetched, once, into the host cache; the
#    run itself is offline. The listing before and after proves nothing was re-downloaded:
#    cargo only adds a .crate file it does not already have.
ls "$CACHE_DIR" 2>/dev/null | sort > /tmp/crates-before.txt || true
if ! cargo fetch --locked --offline --target x86_64-unknown-linux-gnu 2> /tmp/fetch-offline.log; then
    echo "linux-verify: crates missing from the host cache (Linux-only dependencies):"
    grep -E 'failed to download|attempting to make an HTTP request' /tmp/fetch-offline.log | head -20 || true
    cargo fetch --locked --target x86_64-unknown-linux-gnu
fi
ls "$CACHE_DIR" | sort > /tmp/crates-after.txt
echo "linux-verify: host crate cache $(wc -l < /tmp/crates-before.txt) -> $(wc -l < /tmp/crates-after.txt) files; added:"
comm -13 /tmp/crates-before.txt /tmp/crates-after.txt | sed 's/^/  + /'
export CARGO_NET_OFFLINE=true

# 3. What was observed: the Vulkan device (software lavapipe, not a hardware GPU) and tools.
echo "linux-verify: $(uname -srm); $(rustc --version); $(cargo nextest --version | head -1)"
vulkaninfo --summary 2>/dev/null | grep -E 'deviceName|driverName|apiVersion' | sed 's/^/linux-verify: vulkan /' || true

# 4. The command, under an X server so the X11 clipboard and winit have a display.
if [ "$#" -eq 0 ]; then
    set -- just verify
fi
echo "linux-verify: running: $*"
exec xvfb-run -a -s "-screen 0 1920x1080x24" "$@"
