#!/bin/sh
# `just verify-linux` on a Unix host or Git Bash (ADR 0044); the twin of verify-linux.ps1 —
# read that file for the rules (one volume per worktree, seeded once from the old shared
# volume; one Linux verify at a time on the machine). Runs `just verify` (or the given
# command) in the forge-linux-verify container on the CURRENT worktree.
set -eu

IMAGE=forge-linux-verify:1.98.1
LEGACY=forge-linux-target
LOCK_VOLUME=forge-linux-lock
CPUS=${FORGE_LINUX_CPUS:-6}
MEMORY=${FORGE_LINUX_MEMORY:-12g}
CARGO_HOME_HOST=${CARGO_HOME:-$HOME/.cargo}
REGISTRY=$CARGO_HOME_HOST/registry
HERE=$(cd "$(dirname "$0")" && pwd)
REPO=$(git -C "$HERE" rev-parse --show-toplevel)

# The worktree's own volume, named exactly as verify-linux.ps1 names it: the folder name
# (lowercased, [a-z0-9-] only) and the first 8 hex digits of the SHA-256 of the lowercased
# path as git prints it.
if [ -n "${FORGE_LINUX_VOLUME:-}" ]; then
    VOLUME=$FORGE_LINUX_VOLUME
else
    LOWER=$(printf '%s' "$REPO" | tr '[:upper:]' '[:lower:]')
    HASH=$(printf '%s' "$LOWER" | sha256sum | cut -c1-8)
    NAME=$(basename "$LOWER" | sed -e 's/[^a-z0-9][^a-z0-9]*/-/g' -e 's/^-//' -e 's/-$//')
    VOLUME=forge-linux-target-$NAME-$HASH
fi

# Git Bash: hand Docker Desktop Windows paths (C:/...), not MSYS ones (/c/...).
HOST_REGISTRY=$REGISTRY
HOST_HERE=$HERE
if command -v cygpath >/dev/null 2>&1; then
    HOST_REGISTRY=$(cygpath -m "$REGISTRY")
    HOST_HERE=$(cygpath -m "$HERE")
fi

if ! docker image inspect "$IMAGE" >/dev/null 2>&1 || [ "${FORGE_LINUX_REBUILD:-0}" = 1 ]; then
    echo "verify-linux: building $IMAGE (once) from the pinned local rust:1.98.1-bookworm"
    MSYS_NO_PATHCONV=1 docker build -t "$IMAGE" \
        --build-context "hostcache=$HOST_REGISTRY/cache" \
        --build-context "hostindex=$HOST_REGISTRY/index" \
        "$HOST_HERE"
fi
docker volume create "$LOCK_VOLUME" >/dev/null
if ! docker volume inspect "$VOLUME" >/dev/null 2>&1; then
    docker volume create "$VOLUME" >/dev/null
    if [ "$VOLUME" != "$LEGACY" ] && docker volume inspect "$LEGACY" >/dev/null 2>&1; then
        # Seed, under the run lock (no verify writes the old volume while it is copied).
        echo "verify-linux: seeding $VOLUME from $LEGACY (once, local copy) so the first build is incremental"
        if ! MSYS_NO_PATHCONV=1 docker run --rm \
            --mount "type=volume,src=$LEGACY,dst=/from,readonly" \
            --mount "type=volume,src=$VOLUME,dst=/to" \
            --mount "type=volume,src=$LOCK_VOLUME,dst=/lock" \
            "$IMAGE" bash -c 'exec 9>/lock/verify.lock && flock 9 && cp -a /from/. /to/'; then
            docker volume rm "$VOLUME" >/dev/null
            echo "verify-linux: seeding $VOLUME failed; it was removed, run again" >&2
            exit 1
        fi
    fi
fi
echo "verify-linux: volume $VOLUME"

TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT
GIT_INDEX_FILE=$TMP/index git -C "$REPO" read-tree HEAD
GIT_INDEX_FILE=$TMP/index git -C "$REPO" -c core.safecrlf=false add -A
TREE=$(GIT_INDEX_FILE=$TMP/index git -C "$REPO" write-tree)
git -C "$REPO" -c core.autocrlf=false -c core.eol=lf archive --format=tar -o "$TMP/snapshot.tar" "$TREE"
echo "verify-linux: snapshot tree $TREE of $REPO"

HOST_TMP=$TMP
command -v cygpath >/dev/null 2>&1 && HOST_TMP=$(cygpath -m "$TMP")

# MSYS_NO_PATHCONV: Git Bash must not rewrite the container-side paths.
MSYS_NO_PATHCONV=1 docker run --rm --init \
    --cpus "$CPUS" --memory "$MEMORY" \
    -e "CARGO_BUILD_JOBS=$CPUS" \
    -e CI=true \
    -e CARGO_HOME=/vol/cargo-home \
    -e CARGO_TARGET_DIR=/vol/target \
    --mount "type=volume,src=$VOLUME,dst=/vol" \
    --mount "type=volume,src=$LOCK_VOLUME,dst=/lock" \
    --mount "type=bind,src=$HOST_REGISTRY/cache,dst=/vol/cargo-home/registry/cache" \
    --mount "type=bind,src=$HOST_REGISTRY/index,dst=/vol/cargo-home/registry/index" \
    --mount "type=bind,src=$HOST_TMP/snapshot.tar,dst=/snapshot.tar,readonly" \
    "$IMAGE" \
    bash -c 'tar -xOf /snapshot.tar tools/docker/linux-verify/in-container.sh > /tmp/in-container.sh && exec bash /tmp/in-container.sh "$@"' in-container "$@"
