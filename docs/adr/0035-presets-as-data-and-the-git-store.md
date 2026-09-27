# ADR 0035 — Presets are data a project can carry; the Git store keeps Forge's revisions as Git commits

- **Status:** accepted
- **Date:** 2026-09-22
- **Plan references:** Ch.31 §31.3, §31.4, §31.5; Ch.33 §33.1, §33.2, §33.4, §33.5; Ch.32 §32.7;
  I15, I16, I17; D-4; DoD M2-12, M2-15; ADR 0032 (the lifecycle), ADR 0033 (plugins)

(Numbered 0034: `dev` holds up to 0033; D-9.)

## Context

WP-16 makes presets real data (M2-12: the plugin set, layout, settings, new-scene template and
extension-point defaults as `presets/*/workspace.ron`, with `test_no_preset_gating` and
`test_preset_promotion`) and builds the `Git` store backend (M2-15: GitHub device flow and any
remote, the blob store, generated ignore rules), which must pass store parity. WP-U7 built the
lifecycle UI against the store trait and the compiled-in presets; WP-15 moved presets and store
backends into plugins. The plan leaves open: how a Forge revision maps onto Git, where binaries
go, how a Git remote meets `forge_project::sync` (which moves revisions through the
`ProjectStore` trait alone), which Git library and transport, what "a project's own presets"
means for promotion, and what the I15 surface is.

## Decision

1. **gitoxide (`gix`, MIT/Apache-2.0, pure Rust) for objects and refs; our own smart-HTTP
   client for the wire.** gix has no push, and libgit2 (`git2`) is GPL-2.0-with-exception and C.
   The client is `plugins/forge-store-backends/src/git/smart.rs`: discovery, fetch
   (`want`/`have`/`done`, side-band demux streamed into `gix-pack`, which indexes the pack on
   every core) and push (`receive-pack` with `report-status` and `atomic` when offered).
   HTTP is `ureq` on rustls/ring with the **operating system's trust store**
   (`rustls-platform-verifier`) — no bundled root list (webpki-roots is CDLA and is not on the
   allow-list), and a company's own CA works as in the browser. SSH remotes are refused with
   their HTTPS form (a follow-up); a bare repository on a path is `git-file:<path>` (objects
   copied between object databases, no `git` binary).
2. **One Forge revision is one Git commit on `refs/heads/main`, and the commit is a pure
   function of the revision.** Its tree holds every **text** file (UTF-8, no NUL, ≤ 1 MiB) at
   its path — diffable in a pull request — plus `.forge/revision.ron` (the record, whose id
   is the id every backend gives it), `.forge/tree` and `.forge/commands.jsonl` (the command
   log, Ch.33.4). Author and time come from the record (first issuer, `at_ms`), so every
   replica of a history is the same Git history, and pushes send only what is new.
   The revision model is forge-store's public **kit** (`forge_store::kit`: `RevRecord`,
   `build_commit`, …) so a backend plugin builds identical ids with no private path (I16).
3. **Binaries are engine-native blobs on a second ref, `refs/forge/blobs`** (one commit per
   revision that brought new ones; a tree of `<blake3 hex>` entries): LFS semantics without
   LFS, identical on every host. A plain `git clone` (branches only) and every PR diff never
   download them; a Forge clone fetches both refs. Blobs handed to `blob_put` and never
   committed (import artefacts: derivable) stay local and are never pushed. A BLAKE3 → Git id
   index (`forge-index`, append-only, rebuilt from history when lost) makes a blob read one
   probe; every read is verified against its BLAKE3 address.
4. **Generated ignore rules**: each commit's tree carries a `.gitignore` the store writes from
   what it knows — its own data (`/.forge/`) and every binary path — unless the project has
   its own `.gitignore`. Nobody hand-writes one (Ch.33.5).
5. **A project folder with Git history is `git:<folder>`**: working files are the folder
   (through `LocalFs`: atomic writes, the folder's locks), the repository is the bare
   `<folder>/.forge/git` (the folder never looks like a half-checked-out work tree).
6. **A Git remote is a `ProjectStore` view** (`https:`, `http:`, `git-file:` on the
   `StoreBackend` point): opening fetches into a mirror under the per-user cache; its working
   files are the remote head's, read lazily; commits go to the mirror; a new additive trait
   method, **`publish`**, pushes them with **compare-and-swap** on both refs (a remote that
   moved is `STORE-0011 RemoteMoved`, never overwritten). `forge_project::sync` stays
   Git-agnostic: it replays revisions and calls `publish`. A second additive method,
   **`commit_at`**, lets a replay keep each revision's time (`LocalFs` and `MemoryStore`
   honour it), so a clone's history shows when things happened, not when it was cloned.
7. **GitHub sign-in is the OAuth device flow**, one non-blocking poll per request
   (`forge.project.sign_in` start / continue, session commands the core performs); "repo
   created for you" is `forge.project.create_remote` (a private repository, linked by an
   ordinary command). **The token never crosses the bus**, a setting, a URL or a message: it
   lives in the plugin's in-memory `Credentials` for the session. Forge's OAuth app is not
   registered (the owner's decision): `FORGE_GITHUB_CLIENT_ID` names one; gate row
   `C-github-oauth-app` is UNBUILT.

<!-- -->

10. **Transfers run off the core's lock.** Push, pull, clone, sign-in start/continue and
    create-remote are checked under the lock (a refusal is immediate), then their network
    part runs on its own thread: opening a Git remote (the fetch), `publish` (the push),
    GitHub's device flow and API. The thread takes the lock only for the local steps between
    (a push's replay into the remote's mirror, a pull's replay and load, adopting a clone)
    and to record the outcome, which wakes every client. `ProjectStatus::transfer` names
    what is running (the history panel and launcher show it); a second transfer, or a
    create/open, while one runs is refused with `PROJECT-0012`; a panicking transfer records
    `PROJECT-0013`. The headless runner and test rigs call `EditorCore::wait_transfers`
    (sequential clients); the GUI never waits.
11. **The `forge spine` golden moves** (`908317d6…` → `122de480…`): its plugin line lists the
    `StoreBackend` registry, which now also holds `git`, `https`, `http`, `git-file`. Checked:
    the report with the old key list hashes to the old golden — nothing numeric moved.

## Why (the owner's two rules)

- **Better for the user:** GitHub, GitLab, Gitea/Forgejo or a share work as remotes with no
  Git install and no hand-written `.gitignore`; text is reviewable in a PR while binaries
  never bloat a clone; a racing push is refused with "pull, then push", never merged or
  overwritten; sign-in shows a code, needs no redirect, and the token never leaks; a studio
  ships its own presets inside a template project, and a switch applies them while keeping
  every value a user set.
- **Faster:** an unchanged file costs a hash (objects are written only when absent; the
  folder commit re-reads only files whose stamps changed); a blob read is one index probe;
  fetches stream packs into a multi-threaded indexer; pushes send only objects the remote
  lacks; derivable artefacts are never uploaded; a replayed history reproduces the same Git
  ids, so nothing is sent twice.

## Consequences

- `test_store_backend_parity` runs LocalFs, memory, a `git:` folder and a Git remote view;
  gate rows I15 and `C-store-git-remote`, `C-project-presets-from-folder`,
  `C-preset-promotion` are bound; `C-store-git-s3-sql` narrows to S3 and SQL.
- Follow-ups: SSH remotes (the same pkt-line protocol over `ssh`); deltified push packs
  (gix-pack's output side); tokens in the OS credential store; importing commits made
  outside Forge (refused today, named); lazy blob fetch (per-object wants); applying a
  project preset's layout and keymap when the project opens (its defaults and template apply
  today); Forge's GitHub OAuth app.
