# `just verify-linux` on Windows (ADR 0044): run `just verify` (or the given command) on the
# ubuntu-x86_64 leg, locally, in the forge-linux-verify container, on the CURRENT worktree.
#
#   powershell -NoProfile -ExecutionPolicy Bypass -File tools/docker/linux-verify/verify-linux.ps1 [command...]
#   just verify-linux                     # just verify
#   just verify-linux cargo nextest run -p forge-sim --locked
#
# Windows PowerShell 5.1 safe: no native-to-native pipes (5.1 re-encodes them as text, which
# corrupts a tar stream), no `&&`, no `??`. The snapshot goes through a temp file instead.
#
# Argument passing (fixed 2026-09-24, backlog L-16) - three independent hops, each of which
# used to lose a spaced or quoted argument:
# 1. This script reads $args, not a declared param(), because a declared
#    `[Parameter(ValueFromRemainingArguments = $true)]` name lets `powershell -File`
#    prefix-match any leading-dash argument against that name (e.g. `-c` binds to `-Command`)
#    before it ever reaches "remaining arguments" - so `just verify-linux bash -c ...` used to
#    fail with "Missing an argument for parameter 'Command'" no matter how the argument was
#    quoted. $args has no declared name to collide with, so every argument - leading dashes
#    included - arrives unchanged.
# 2. `just`'s `{{ARGS}}` joins this recipe's variadic arguments with plain spaces and does not
#    re-quote an argument that itself contains a space, so an argument with an embedded space
#    must be double-quoted by the CALLER for it to survive the just -> cmd.exe ->
#    powershell -File hop as one argument, e.g.:
#      just verify-linux bash -c '"cargo test -p forge-sim --locked -- --nocapture"'
#    (the outer single quotes are your own shell's; the inner double quotes are literal
#    characters that must reach `just` - without them, "cargo test ... --nocapture" arrives
#    split into separate, wrongly-divided arguments.)
# 3. Windows PowerShell 5.1 splatting an array to a native command (`docker @Run`) does not
#    reliably preserve an embedded space, and can mangle embedded `"` characters, within one
#    array element - observed on both a fixed `-c` script string and an appended $Command
#    element. `Invoke-DockerLine` below builds the docker invocation as one Win32-quoted
#    string by hand and runs it through `cmd.exe`, which reconstructs it correctly.
# Recorded: docs/evidence/linux/README.md.
#
# Bandwidth: the image is built once from the pinned local base (see the Dockerfile) and never
# pulled; the host's crate cache is mounted, not re-downloaded; the target dir and the source
# snapshot live in ONE persistent volume PER WORKTREE (forge-linux-target-<name>-<hash of the
# worktree path>; FORGE_LINUX_VOLUME overrides), so two lanes never build or test on each
# other's snapshot. A new worktree's volume is seeded once from the old shared volume
# (forge-linux-target) when it exists, so its first build stays incremental. Nothing of the
# Windows target dirs is copied.
# Load: the container is capped at FORGE_LINUX_CPUS (default 6) CPUs and cargo at as many jobs,
# so the Windows lanes are not starved; and one Linux verify runs at a time on this machine
# (in-container.sh takes a lock on the shared forge-linux-lock volume; a second run waits and
# says so), so two containers never load the VM's CPUs under each other's timed tests.
$Command = $args

# 'Continue', not 'Stop': under 5.1 'Stop' turns a native command's redirected stderr into a
# terminating error. Every native call is checked through its exit code instead.
$ErrorActionPreference = 'Continue'
$Code = 1
$Image = 'forge-linux-verify:1.98.1'
$Legacy = 'forge-linux-target'
$LockVolume = 'forge-linux-lock'
$Cpus = if ($env:FORGE_LINUX_CPUS) { $env:FORGE_LINUX_CPUS } else { '6' }
$Memory = if ($env:FORGE_LINUX_MEMORY) { $env:FORGE_LINUX_MEMORY } else { '12g' }
$CargoHome = if ($env:CARGO_HOME) { $env:CARGO_HOME } else { Join-Path $env:USERPROFILE '.cargo' }
$Registry = Join-Path $CargoHome 'registry'
$Here = Split-Path -Parent $MyInvocation.MyCommand.Path
$Repo = (git -C $Here rev-parse --show-toplevel).Trim()

function Invoke-Native([string]$What) {
    if ($LASTEXITCODE -ne 0) { throw "verify-linux: $What failed (exit $LASTEXITCODE)" }
}

# See "Argument passing" above (point 3): builds a Win32-quoted command line by hand (wrap in
# double quotes, backslash-escape embedded quotes, only when an element needs it) and runs it
# through cmd.exe instead of `docker @Args`, so an argument with an embedded space or quote
# reaches the container exactly as given.
function Invoke-DockerLine([string[]]$DockerArgs) {
    $quoted = $DockerArgs | ForEach-Object {
        if ($_ -match '[\s"]') { '"' + ($_ -replace '"', '\"') + '"' } else { $_ }
    }
    cmd.exe /d /c ('docker ' + ($quoted -join ' '))
}

# The worktree's own volume: its folder name (lowercased, [a-z0-9-] only) and the first 8 hex
# digits of the SHA-256 of its lowercased path as git prints it - the same name
# verify-linux.sh computes for the same worktree.
if ($env:FORGE_LINUX_VOLUME) {
    $Volume = $env:FORGE_LINUX_VOLUME
} else {
    $Sha = [System.Security.Cryptography.SHA256]::Create()
    $Bytes = $Sha.ComputeHash([System.Text.Encoding]::UTF8.GetBytes($Repo.ToLowerInvariant()))
    $Hash = (($Bytes[0..3] | ForEach-Object { $_.ToString('x2') }) -join '')
    $Name = ((Split-Path -Leaf $Repo).ToLowerInvariant() -replace '[^a-z0-9]+', '-').Trim('-')
    $Volume = "forge-linux-target-$Name-$Hash"
}

# 1. The image: built once, only if missing (FORGE_LINUX_REBUILD=1 forces a rebuild).
docker image inspect $Image *> $null
if ($LASTEXITCODE -ne 0 -or $env:FORGE_LINUX_REBUILD -eq '1') {
    Write-Host "verify-linux: building $Image (once) from the pinned local rust:1.98.1-bookworm"
    docker build -t $Image `
        --build-context "hostcache=$(Join-Path $Registry 'cache')" `
        --build-context "hostindex=$(Join-Path $Registry 'index')" `
        $Here
    Invoke-Native 'docker build'
}
docker volume create $LockVolume | Out-Null
Invoke-Native 'docker volume create (lock)'
docker volume inspect $Volume *> $null
if ($LASTEXITCODE -ne 0) {
    docker volume create $Volume | Out-Null
    Invoke-Native 'docker volume create'
    docker volume inspect $Legacy *> $null
    if ($LASTEXITCODE -eq 0 -and $Volume -ne $Legacy) {
        # Seed, under the run lock (no verify writes the old volume while it is copied).
        Write-Host "verify-linux: seeding $Volume from $Legacy (once, local copy) so the first build is incremental"
        docker run --rm `
            --mount "type=volume,src=$Legacy,dst=/from,readonly" `
            --mount "type=volume,src=$Volume,dst=/to" `
            --mount "type=volume,src=$LockVolume,dst=/lock" `
            $Image bash -c 'exec 9>/lock/verify.lock && flock 9 && cp -a /from/. /to/'
        if ($LASTEXITCODE -ne 0) {
            docker volume rm $Volume | Out-Null
            throw "verify-linux: seeding $Volume failed (exit $LASTEXITCODE); it was removed, run again"
        }
    }
}
Write-Host "verify-linux: volume $Volume"

# 2. The snapshot: the tree `git add -A` would commit (tracked + untracked, .gitignore
#    respected), built in a throwaway index so the real index is untouched, archived with LF
#    line endings - what CI's ubuntu checkout sees.
$Tmp = [System.IO.Path]::GetTempPath()
$Index = Join-Path $Tmp "forge-linux-verify-$PID.index"
$Tar = Join-Path $Tmp "forge-linux-verify-$PID.tar"
try {
    $env:GIT_INDEX_FILE = $Index
    git -C $Repo read-tree HEAD
    Invoke-Native 'git read-tree'
    git -C $Repo -c core.safecrlf=false add -A
    Invoke-Native 'git add -A (temporary index)'
    $Tree = (git -C $Repo write-tree).Trim()
    Invoke-Native 'git write-tree'
    Remove-Item Env:GIT_INDEX_FILE
    git -C $Repo -c core.autocrlf=false -c core.eol=lf archive --format=tar -o $Tar $Tree
    Invoke-Native 'git archive'
    Write-Host "verify-linux: snapshot tree $Tree of $Repo"

    # 3. Run. The entry script comes out of the snapshot itself, so it always matches the tree.
    $Run = @(
        'run', '--rm', '--init',
        '--cpus', $Cpus, '--memory', $Memory,
        '-e', "CARGO_BUILD_JOBS=$Cpus",
        # CI=true as on GitHub's runners: a GPU guard with no adapter FAILS instead of
        # reporting AWAITING (W9), so a green run means the guards really rendered.
        '-e', 'CI=true',
        '-e', 'CARGO_HOME=/vol/cargo-home',
        '-e', 'CARGO_TARGET_DIR=/vol/target',
        '--mount', "type=volume,src=$Volume,dst=/vol",
        '--mount', "type=volume,src=$LockVolume,dst=/lock",
        '--mount', "type=bind,src=$(Join-Path $Registry 'cache'),dst=/vol/cargo-home/registry/cache",
        '--mount', "type=bind,src=$(Join-Path $Registry 'index'),dst=/vol/cargo-home/registry/index",
        '--mount', "type=bind,src=$Tar,dst=/snapshot.tar,readonly",
        $Image,
        'bash', '-c', 'tar -xOf /snapshot.tar tools/docker/linux-verify/in-container.sh > /tmp/in-container.sh && exec bash /tmp/in-container.sh "$@"', 'in-container'
    )
    if ($Command) { $Run += $Command }
    Invoke-DockerLine $Run
    $Code = $LASTEXITCODE
} finally {
    if ($env:GIT_INDEX_FILE) { Remove-Item Env:GIT_INDEX_FILE }
    Remove-Item -Force -ErrorAction SilentlyContinue $Index, $Tar
}
exit $Code
