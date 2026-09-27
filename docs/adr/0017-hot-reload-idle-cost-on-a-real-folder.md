# ADR 0017 — Poll a real folder with one directory walk and bound its idle cost

- **Status:** accepted
- **Date:** 2026-09-21
- **Plan references:** Ch.8 §8.4 / §8.9, Ch.33.6, I17, M2-6; amends ADR 0015 point 5

## Context

ADR 0015 made hot reload a poll of `ProjectStore::stamps()`. The quiet-poll figure quoted
there (1–5 µs) was measured on `MemoryStore`, whose stamps are hashes it already holds. On
a real folder `LocalFs::stamps` listed the tree, then opened and `stat`ed every file by path,
re-read and hashed in full every file modified in the last 2 s on every poll in that window,
and a poll that found a change walked the whole tree a second time at the end. Measured on
the dev box (NTFS, release): 10 000 files cost 0.5–0.8 s per quiet poll, almost all of it
the per-path `stat`. The editor pays that for as long as a project is open.

## Decision

1. **One walk, metadata from the directory entries.** `LocalFs::stamps` takes size and
   mtime from `DirEntry::metadata` during the walk. On Windows that data comes back with
   each name from `FindNextFileW` (no per-file open, the source git for Windows' fscache
   uses); elsewhere it is one `lstat` per entry. No file is read on a quiet poll.
2. **A racy window from the timestamp resolution, and each hash kept.** A file is hashed
   only while two same-size writes could still share its mtime: 100 ms when the mtime has a
   sub-second part (NTFS/ext4/APFS/btrfs/exFAT, clocks ticking ≤ ~16 ms), 2 s when it lies
   on a whole second (FAT, HFS+, ext3). The hash is streamed, cached with the metadata it
   was taken under and with when its read began. It is reused once a read has begun after
   the window closed, so a just-written file is read once or twice and never again. The
   cached content stamp stays in use while the metadata is unchanged, so the stamp does not
   flip when the window closes.
3. **Re-stamp only our own writes.** Additive `ProjectStore::stamps_of(paths)` (default:
   filter `stamps()`). `Vfs` records the paths written or deleted through it, and the end of
   `AssetServer::poll` re-stamps just those. That replaces the second whole-tree walk, which
   also swallowed any edit made to another file while the poll ran.
4. **Budget, and a poll interval from the measured cost.** Budget: a quiet `LocalFs` poll
   takes ≤ 60 ms per 10 000 files (release; about 2× the measured median). It is asserted
   in release builds by `forge-store/tests/test_local_stamps.rs`, and the work (zero bytes
   read) is asserted in every build. `AssetServer::poll_interval()` is 20× the last
   `stamps()` cost, clamped to 250 ms–2 s, so polling stays under ~5 % of one core until
   the 2 s ceiling.
5. **Plan past ~50k files: a change feed behind the trait.** An additive
   `ProjectStore::changes()`, with `LocalFs` backed by `ReadDirectoryChangesW` / inotify and
   the walk kept as the resync after an event-queue overflow or a restart. With it, idle
   cost no longer grows with project size. The feed stays behind the trait (I17), never
   inside forge-asset. It is not built here (a native-watcher dependency needs its licence
   checked and a Linux leg to test on).

## Measured (dev box, NTFS, release)

| Tree | Quiet poll (median) |
|---|---|
| 10 000 files, before this ADR | 0.5–0.8 s |
| 10 000 files | 24–30 ms (≈ 3 µs/file; the raw walk alone is ≈ 15 ms) |
| 10 000 files + the 500-object scene, through `AssetServer::poll` | 16–34 ms → poll interval 275–555 ms |
| 100 000 files | 375 ms (343–442) → poll interval 2 s |
| 28 MiB file just written | hashed ≤ 2×, then 10 polls in ~1.4 ms |

## Why — the owner's two rules

1. **Better for the user:** an open editor no longer burns a core scanning a large project.
   An edit made while a reimport is running is no longer lost. Hot reload latency stays at
   250 ms for ordinary projects.
2. **Faster / more efficient engine:** about 20–30× cheaper quiet polls on disk. A large
   freshly written file is no longer re-hashed on every poll for 2 s. There is no second
   walk per active poll.

## Alternatives rejected

- *Per-directory mtime short-circuit:* a directory's mtime changes when entries are added,
  removed or renamed, not when a file's content changes, so it cannot skip the files that
  matter. On Windows the listing already carries the metadata, so there is nothing to save.
- *Keep 2 s for every filesystem:* safe, but it re-hashes a just-written file on every poll
  for 2 s. Sub-second mtimes show the filesystem is fine-grained, so 100 ms is safe there.
- *Trust metadata immediately:* misses a same-size rewrite inside one clock tick. That is
  guarded by `a_same_size_rewrite_inside_one_tick_is_seen` and by the hot-reload gate's
  positive control.
- *Build the watcher now:* the right end state, but the plain walk meets the budget up to
  tens of thousands of files, and the watcher needs a vetted dependency and a Linux test leg.

## Consequences

- `stamps()` on a network share whose server clock is skewed by more than the window can
  miss a same-size rewrite inside one server tick. The old 2 s rule had the same limit at
  2 s of skew.
- The watcher (point 5) is the recorded follow-up once projects of 50k+ files appear.
