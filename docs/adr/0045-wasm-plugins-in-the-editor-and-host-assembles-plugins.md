# ADR 0045 — WASM plugins in the running editor, presets from the real registry, and "the host assembles plugins"

- **Status:** accepted
- **Date:** 2026-09-23
- **Work package:** WP-21 (plugin system v1.1)

(Numbered 0045: `dev` holds up to 0044; D-9.)

## Context

WP-15 built the WASM host (`forge-wasm`) and moved the importers, presets and store backends
into `plugins/`, but the editor still loaded only source plugins: `shell::assemble` called
`loader::load` with no hosted plugins, `PluginWatcher::poll` had no caller, the `Preset`
registry the editor listed was a private one of the three built-ins (`presets_from_registry`
had no production caller), the `Importer` point had no WASM adapter, and §21.19's ViewSpec
panels did not exist. Nothing said which kernel crates may depend on `plugins/*`.

## Decisions

1. **Where the editor looks** (`forge_editor::hosting::PluginDirs`): `<user config>/plugins/
   <name>/` (the person's drop-ins, every project), the plugin cache's `<user config>/plugins/
   <id>/<version>/` for each plugin the open project's set names, enabled, at that version
   (what the Plugin manager's *Add* downloads — an added WASM plugin now installs while the
   editor runs), and `<project folder>/plugins/<name>/` (plugins the project carries in
   version control). A plugin the project's set disables is loaded from nowhere; the same id
   twice loads once and the other directory is reported. Project plugins run code from a
   cloned repository, but only in the sandbox and with no capability until a person grants
   one — the same rule as every WASM plugin (E-27).
2. **One load at start.** `shell::assemble_hosted` compiles the WASM plugins found on a host
   over the core's one grant table and passes them to `loader::load_hosted` **with** the source
   plugins: one manifest check, one conflict check, one install order, so a WASM plugin may
   add, replace, remove or chain anything a source plugin may at the points it has adapters
   for. A WASM plugin the load refuses is left out with the reason (the Plugin manager lists
   it *not loaded*, the console logs why) and the load runs again without it — a bad plugin
   never stops the editor starting (better for the user). `assemble` is `assemble_hosted`
   with no hosting. The editor's default plugin set (`forge.editor`, `forge.asset`,
   `forge.importers`, `forge.presets`) is loaded by `assemble` itself, so the Plugin manager
   lists it (I16).
3. **Dropped in while it runs.** The shell polls (`PluginHosting::poll`) on loop turns it
   runs anyway, at most once a second: a new directory loads through `load_hosted` into a
   scratch registry set, is conflict-checked against every loaded manifest, and its items are
   added to the live registries — commands on the core's bus and in the palette, presets in
   the catalog the launcher lists, importers in the asset database (`AssetCatalog::
   plugins_changed`, `AssetServer::refresh_plugins`), panels in the dock's shared panel host
   and the Window actions. A plugin that **replaces, removes or chains** waits for the next
   start: those change what other plugins installed, and the loader's single install order is
   what makes the result independent of discovery order. The notice says so.
4. **No idle cost (D-5), measured.** No thread, no timer, no file watcher: a poll is one
   `read_dir` per plugin root plus one `metadata` per watched file, and it runs only on a turn
   the loop takes for another reason. So an idle editor stays at zero frames and zero wakeups
   (`test_wasm_plugins_in_editor`: 10 s idle, 0/0; the control polls on a timer and wakes).
   The cost is latency: a plugin dropped in while the editor sleeps installs when it next
   wakes (focus, a pointer move). Rejected: an OS file watcher (`notify` is CC0/Artistic —
   not on the A.5 allow-list — and a watcher thread per root to save a person one mouse move).
5. **Hot reload** is the WP-15 `PluginWatcher`, now called: changed code swaps under every
   installed item (the Plugin manager shows the generation), a broken save keeps the old code
   and says so, a changed manifest asks for a restart.
6. **Presets from the real registry.** `PresetCatalog` is built from the editor's `Preset`
   registry and shared (`SharedPresets`) by the shell's launcher and the core, which creates
   projects from it (`Template::Preset(key)`, id `preset:<key>`; `template_doc_in`). A
   defaults-only descriptor (no `workspace.ron`, what a small WASM preset is) is its family's
   built-in preset with its label and defaults on top. The built-in presets are built once per
   process (`builtin_preset`/`builtin_presets` over a `OnceLock`). A built-in template uses
   what its family's key holds in the registry, so a plugin that replaces `forge.preset.3d`
   changes what "3D" creates.
7. **WASM importers** (`forge_wasm::importer`): describe once at install (`{version,
   extensions}`); import with a binary framing (length-prefixed JSON header, source, supplied
   dependencies; the answer is a header and length-prefixed artefacts — bytes are never
   escaped into JSON); a dependency is asked for by name (`needs`) and supplied on the next
   call through `ImportCx::dependency` (recorded, so it reimports), and reading any file beyond
   the source needs `Fs(ProjectRead)`, checked at that moment. The version is the declared
   version mixed with an FNV-1a hash of the plugin's code, so a hot reload (or new code at the
   next start) reimports and unchanged code never does, identically in every run.
8. **ViewSpec panels** (`forge_editor::viewspec`, §21.19): headings, text, live setting
   readouts, rows and buttons, JSON, bounded (256 nodes, 4 rows deep). A button runs only a
   command **the same plugin provides** — which plans under that plugin's grants — so a
   panel's actions can do nothing its plugin was not granted, and a plugin cannot borrow the
   person's authority by labelling a button (a foreign command is shown as text, not a
   button).
9. **The host assembles plugins** (plan §32.8, `cargo xtask layering`, in `just verify` and
   CI): a `crates/*` crate names a `plugins/*` crate in `[dependencies]`/
   `[build-dependencies]` only when it is a listed **host** (forge-editor → forge-importers,
   forge-presets; forge-project → forge-store-backends); dev-dependencies are free; a plugin
   never depends on `tools/*`; a stale allowance is an error. The alternative — every host
   dependency in `tools/*` — would move `forge_editor::shell::assemble`'s default set and
   `ProjectHost::first_party` out of the libraries the tests and the split editor use, for no
   gain in replaceability (they still load through the loader).

## Consequences

- The editor binary hosts WASM plugins (not as a split-editor device: its core is the
  host's). The headless core does not host plugins yet.
- A runtime-dropped plugin that replaces or chains needs a restart; a changed manifest too.
- Follow-ups outside this WP: the `project-read` interface is not yet wired to the open
  project's store in the editor (UNBUILT: the store is the core's, whose lock a plugin
  command's plan already runs under; `read` says the editor does not serve project files yet,
  and an importer names the files it needs instead, which is served); the
  I16 prefix scan, the stronger hot-reload/capability controls and the forge-wasm hot paths are
  lane L's (L-06/L-07).

## Amendment 1 (WP-34, 2026-09-23): project-carried plugins need trust; a live install is all or nothing

**Problem (WP-21 verifier).** Separately, `PluginHosting::install_live` put a plugin's
commands on the core's bus and in the live registry before its presets and importers were
added: a later failure left the commands installed with the directory marked refused, and
the retry after a fix failed on its own leftovers (`DuplicateHandler`).

**Decided (by the owner's rules: better for the user, safe by default).**

10. **A project's own plugins run only once the person trusts it** (`forge_editor::trust`).
    Until a project is trusted:
    * the hosting **does not load its plugins** — its `plugins/` folders and the cached
      plugins its set names (code the project chose); the person's own drop-ins load as always.
      The Plugin manager lists them *not loaded: the project is not trusted*;
    * the person is **asked once**: a notice that opens the Plugin manager, whose *Project
      trust* group lists what the project carries and offers *Trust this project* / *Don't
      trust*.

    Trusting records the answer, accepts the grants a **person's** open held (through the
    human-only `forge.security.accept_held`, so it is audited and in the history under their
    name), and the plugins install at the next poll. Not trusting keeps the plugins out and
    the grants held; the answer is remembered and not asked again. Both are audited (`trust`
    / `distrust`).
11. **What needs no decision.** A project that carries no plugins and no plugin grants is not
    asked about. A project a person creates in this editor is theirs: trusted from the start
    (unless its folder already held plugins). With no project open the editor holds the
    person's own unsaved work: nothing needs trust. A team pull follows the open project's
    trust (untrusted: what it would add is held, against the project in memory); a sandbox with
    no project folder is the baseline the person chose to join (a human-only command).
12. **Headless has no one to ask**, so it trusts nothing unless its launcher passes
    `--trust-project` (`forge --headless`, `forge-editor --headless`, with or without
    `--remote-host`; `HeadlessOptions::trust_project`, `trust::TrustEvery`) — a flag on the
    command line no script or project file can set. It does not accept what a script's own
    open holds; `--accept-project-security` still does that.
13. **A live install is all or nothing.** Every item the plugin adds is staged first, per
    point, and checked against the live registries and the core's bus (a key or target already
    held refuses the plugin naming the holder); then it is committed — presets and importers
    into the live registries, each add undone if a later one is refused, and last the
    commands, which the core takes all or none under one lock
    (`EditorCore::install_commands_all`); a refusal there undoes the registries. Only a
    committed install is announced (palette, presets catalog, asset database, panels). A
    failed install leaves nothing, so the retry after a fix installs cleanly.

**Rejected.** Loading an untrusted project's plugins with no grants: the sandbox makes that
safe in principle, but a person who has not decided would still run a stranger's code (CPU,
panels that look like the editor's), and there is nothing to gain before they decide.
Remembering trust in the project (a `.forge/trusted` file): a hostile project would ship it.

**Consequences.** The person's own existing projects that carry plugins or plugin grants are
asked about once (then remembered). A trusted project's plugins that were installed stay until
restart if the person later distrusts it (the answer's line says so; a live
uninstall is out of scope). The `--remote-host` console answers held proposals (`accept-held`)
but has no `trust` command yet: a headless host is trusted with `--trust-project` or not at
all. *(Superseded by Amendment 3, decision 26: the console has `trust ID` / `distrust ID`.)*

### Amendment 1, continued (the WP-34 verifier's findings, 2026-09-23)

16. **Panels are staged with the rest of a live install.** The shell's panel host can hold
    panels no loaded manifest declares, so the manifest conflict check could pass while the
    shell's `add_panel` then refused the plugin's panel — commands live, panel missing.
    `PluginHosting::poll` now takes the shell's `panel_taken` check and the `Panels` step
    refuses the whole plugin when a panel id is held (or was installed earlier in the same
    poll); every `add_panel` after a committed install takes its panel. Test:
    `a_plugin_whose_panel_id_is_taken_installs_nothing`, control
    `positive_control_unchecked_panels_install_partly` (`HostingFaults::skip_panel_check`), in
    C-install-live-atomic.
17. **Only a question the person saw gets an answer.** The Plugin manager shows *Trust this
    project* / *Don't trust* only when the open project carries something to trust, and
    `EditorCore::decide_trust` refuses an answer about a project that carries nothing (it
    would otherwise be remembered for whatever the folder carries later).

**Known limitation (superseded by Amendment 2): trust is keyed by the folder, not its contents.** The key is the canonical
project folder, so a different repository cloned later into a folder the person trusted is
trusted too (VS Code's workspace trust has the same property). Keying by content (a hash of
the plugins and grants) would re-ask on every teammate's plugin update or grant change, which
the person would learn to click through; keying by remote URL fails for local-only projects.
The Plugin manager shows what a trusted project carries, and distrusting it takes effect at
the next open. Revisit if trust ever gates more than plugins and grants.

*(Superseded by Amendment 2 below: trust is now keyed by the folder and its content.)*

## Amendment 2 (WP-35, 2026-09-24): project trust is keyed by content, not only the folder

**Problem (WP-34 verifier).** Amendment 1 keyed trust by the canonical project folder. A
different repository cloned into a trusted folder was trusted too, and — the common case — a
`git pull` into a trusted project that added a `plugins/` folder, changed a trusted plugin's
code (which then hot-reloaded in place) or added a plugin grant ran it without asking. The
known-limitation note above rejected content keying because it "would re-ask on every
teammate's plugin update or grant change". That cost is real, but the alternative lets any
commit to a shared repository run new code with new grants on every teammate's machine
without anyone looking, and it makes the Amendment 1 question a one-time formality. By the
owner's rules (better for the user: a person decides about code before it runs on their
machine; safe by default), the question is asked again — but only about what changed, and
never about the person's own changes.

**Decided.**

18. **An answer covers what the project carried when it was given.** BLAKE3 rather than the
    watcher's `DefaultHasher`: the digest is persisted and compared across runs and
    toolchains, and a hostile commit must not be able to collide with content the person
    trusted.
19. **Unchanged — or less — is trusted; anything new or changed asks again**
    (`TrustState::Changed`). What the person trusted keeps running and in effect; what
    changed waits: a new or changed plugin folder, or a cached plugin at a new version, is not
    loaded (the Plugin manager lists it *not loaded*); a **loaded** plugin whose files changed
    keeps serving the code the person trusted — its hot reload waits
    (`PluginWatcher::poll_where` skips it without taking its stamps, so the reload happens at
    the first poll after the person trusts the change); a new grant or wider capability is
    held against what the person trusted (`trust::approved_settings`, `held_changes_by`) as a
    proposal marked `untrusted`. The shell posts *This project changed since you trusted it*
    once per question; the Plugin manager's *Project trust* group lists each change first
    (*… is new since you trusted the project* / *… changed since you trusted the project*).
    Trusting records the new content, accepts what a person's open held, and the next poll
    installs and reloads. A removal never asks (it runs nothing new). The same gate covers a
    team pull (the pull's settings against what the person trusted, held against the project
    in memory) and the start-up assembly.
20. **The person's own change never asks.** A load and a team pull are applied by the core
    itself and excluded by their `seq` (`core_applied`): what they bring the person has not
    seen.
21. **The start-up trust gate.** `assemble_hosted` derives the open project's plugin folder
    from the core when the setup names none, and vets the project's plugin folders and cached
    versions before the first turn (`hosting::discover(.., Some(core))`, which adds what it
    left out to the question). The editor binary opens no project at start today; the gate is
    what makes a future start-up open (or a core shared with another shell) safe, and it is
    tested now.
22. **WP-34 trust files** (bare answers) still read: each answer covers no content, so each
    trusted project that carries anything is asked about once more, showing all it carries.

**Rejected.** Keying by the repository's remote URL or commit: fails for local projects and
re-asks on every unrelated commit. Re-asking about the whole project on any change (not the
changed items): the person would click through a list they have already seen. Hashing plugin
code in the hosting on every poll: the digest is recomputed only when a folder's manifest or
code stamp (length and modification time, whole) changes.

**Consequences.** A plugin developer editing their own plugin inside a project's `plugins/`
folder is asked on each change they make outside the editor (the editor cannot tell their
save from a pull); developing in `<user config>/plugins/<name>/` hot-reloads without asking,
as before. The team-pull branch has no dedicated content-keyed test yet (team sandboxes are
joined without a project folder and follow the join, as in Amendment 1). *(Resolved by
Amendment 3, decision 23.)*

**Gate rows:** C-project-trust-content (`test_project_trust.rs`:
`a_pull_into_a_trusted_project_asks_again_before_it_runs`,
`a_persons_own_change_never_asks_again`; control
`positive_control_trust_by_folder_runs_what_a_pull_brought`, `CoreFaults::trust_by_folder`),
C-project-trust-cached (`an_untrusted_projects_cached_plugin_version_is_not_loaded`, which
fails when the hosting scan's cache-branch `if trusted` is broken to `if true || trusted`;
control `positive_control_a_hosting_that_ignores_trust_runs_the_cached_plugin`,
`HostingFaults::ignore_trust`), C-project-trust-startup
(`the_startup_gate_loads_an_open_projects_plugins_only_as_trusted`; control
`positive_control_trust_by_folder_at_startup_loads_changed_code`).

## Amendment 3 (WP-36, 2026-09-26): what was vetted is what runs; idle vets; console trust

**Problems (WP-35 verifier).** (a) The team pull's trust gate had no test: team sandboxes
join without a project folder, so breaking `let trusted = pulled.iter().all(..)` to
`true || ..` left every test green. (b) A time-of-check/time-of-use window: the hosting's scan
hashed a project plugin folder for the gate, then `load_dir` and the hot-reload watcher read
the files **again** and compiled what they found — a `git pull` writing between the two reads
ran code nobody vetted, once. (c) Every poll asked the core's gate about every plugin the open
project brings — one core lock per plugin per poll, for a trusted project with nothing new —
and the digest cache kept removed folders. (d) The `--remote-host` console could not answer
trust (Amendment 1's consequence).

**Decided.**

23. **The team pull is tested in a project folder** (`test_project_trust`:
    `a_team_pull_into_a_trusted_project_asks_before_it_takes_effect`): the person creates a
    project in a folder (trusted), makes a team of it; a teammate grants a plugin, adds a
    cached plugin to the set and publishes; the person's pull holds the grant (marked held for
    trust), asks, listing both as new, and trusting accepts the grant; a later pull with
    nothing new asks nothing. The literal `true ||` break fails it (observed), as does the
    control `CoreFaults::trust_by_folder`.
24. **Read once, vet, compile from that buffer.** `forge_wasm::PluginFiles` is one read of a
    plugin folder (stamps taken before the bytes); `WasmHost::load_files` parses the manifest
    from and compiles exactly those bytes, and the plugin keeps a small baseline (manifest
    bytes, stamps, code hash — no code bytes) that the watcher starts from, so nothing is read
    a third time. For an open-project folder the scan's approved digest travels with the
    folder; the load reads the files once, digests that buffer (`trust::plugin_files_digest`,
    the same BLAKE3 layout as before, so recorded answers still match) and compiles it only if
    it is the approved digest — otherwise nothing is compiled and the next poll vets the
    folder as it is now (at start-up the folder is listed not loaded). The watcher's
    `poll_vetted` shows the host's vet the manifest in effect and the new code bytes it read,
    **before** compiling them; a refusal keeps the old code (`ReloadEvent::Held`) and leaves the
    change unstamped for a later poll. Rejected: re-checking the stamps after the load (a
    same-length write within the clock tick passes), locking the folder (no portable way, and
    a pull must not fail because the editor is open).
25. **The gate's answers are cached against the core's trust version.**
    `EditorCore::trust_version` is an atomic the core moves whenever an answer could change —
    every trust-book write (`record_trust`), a new trust book, a load (the open project), the
    faults. The hosting reads it (lock-free) before any question and keeps the answers per
    item and digest while it and the open project stand still; a changed folder has a new
    digest, so it is asked about afresh; answers a poll did not use are dropped. Measured: a
    trusted project with two plugins, 5 idle polls — 0 questions (10 without the cache,
    `HostingStats::vets`). The digest cache drops a folder that is gone and every folder of a
    project no longer open. Caching refusals too is safe: a refusal adds the item to the
    question, which only a load resets (and a load moves the version).
26. **`trust` / `distrust` at the console.** `trust` shows the open project's question (id,
    what it carries, what changed); `trust ID` / `distrust ID` call
    `EditorCore::decide_trust_seen` — the Plugin manager's call, audited as `trust` /
    `distrust` under the person at the terminal, a trusted project's held grants accepted
    with the human-only `forge.security.accept_held` — which refuses an answer to a question
    whose id changed since it was shown (a pull in between), under the same lock that
    records it. A new question is announced as it appears. The answer lives in the core's
    trust book, i.e. for this run on a host started without `--trust-project` (a headless
    host's answers are not written to user config; revisit if hosts are restarted often
    enough for that to cost). The GUI Plugin manager answers the same way: its buttons call
    `decide_trust_seen` with the id of the question the group last showed, so a press on a
    question a pull changed since is refused and the status says to look again
    (`test_plugin_manager`:
    `a_press_on_a_question_that_changed_since_it_was_shown_is_refused`; the id-less
    `decide_trust` fails it, observed).
27. **Two guards, two seams (the WP-36 verifier).** A write can land at two places, and each
    has its own seam and its own control, each a real version of the defect:
    * *Before the read* (`HostingFaults::swap_after_vet`): after the scan's look, before the
      load and the watcher read. Caught by the check of what is read — the load's digest
      comparison and the watcher's vet. Control `HostingFaults::unchecked_reads` (no
      comparison at the load, no vet at the reload, as before WP-36).
    * *After the check, before the compile* (`HostingFaults::swap_after_check`): at the load,
      after the digest comparison passed; at the hot reload, inside the watcher's vet after it
      allowed a change the person trusted. Caught only by compiling the checked buffer. Control
      `HostingFaults::reread_after_vet`, which checks the read and then compiles a **second**
      read — `WasmHost::load_dir` at the load, and `forge_wasm::WatchFaults::reread_after_vet`
      (a new `controls` feature on `forge-wasm`, turned on only by dev-dependencies) in the
      watcher. The verifier's two mutations — the watcher compiling a fresh `read_file` after
      the vet, the load calling `host.load_dir(dir)` after the comparison — each fail
      `code_written_after_the_check_never_runs_the_checked_bytes_do` (observed). In both
      cases the write is a change of its own: the next poll asks about it and the checked code
      keeps serving.
    The library-level guard is `forge-wasm`'s
    `a_write_between_the_vet_and_the_compile_never_runs` (a vet that writes new code into the
    folder and then allows the change), control
    `positive_control_a_watcher_that_reads_again_runs_the_later_write`.
    An idle poll also no longer allocates in the vet cache: answers are kept by item name
    (looked up borrowed) with the digest and a used flag, and the digest cache hands out a
    reference, not a clone. (The scan's directory listing still allocates its paths.)

**Gate rows:** C-project-trust-team-pull (`test_project_trust.rs`; control
`positive_control_trust_by_folder_takes_what_a_team_pull_brought`), C-project-trust-read-once
(`code_written_after_the_check_never_runs_the_checked_bytes_do`, seam
`HostingFaults::swap_after_check`; control
`positive_control_reading_again_after_the_check_runs_the_later_write`,
`HostingFaults::reread_after_vet`, which runs the later write at both the load and the reload),
C-project-trust-check-at-read (`code_swapped_in_after_the_trust_gate_looked_never_runs`, seam
`HostingFaults::swap_after_vet`; control `positive_control_unchecked_reads_run_the_swapped_code`,
`HostingFaults::unchecked_reads`), C-wasm-vetted-reload (`crates/forge-wasm/tests/test_wasm_hot_reload.rs`;
control `positive_control_a_watcher_that_reads_again_runs_the_later_write`,
`WatchFaults::reread_after_vet`), C-project-trust-idle-vets
(`idle_polls_of_a_trusted_project_ask_the_trust_gate_nothing`; control
`positive_control_without_the_cache_every_poll_asks`, `HostingFaults::no_vet_cache`),
C-console-trust (`crates/forge-remote/src/console.rs`:
`the_console_answers_project_trust_through_the_audited_call`; control
`positive_control_a_console_that_trusts_by_the_book_fails`, `ConsoleFaults::trust_by_book`).
Unit tests: `hosting::tests::a_removed_plugin_folder_keeps_no_digest` (fails without the
scan's `retain`, observed), `vet_answers_are_kept_against_the_version_and_the_project`;
`forge-wasm` `test_wasm_hot_reload`: `a_plugin_loaded_from_files_runs_the_bytes_read_not_a_later_write`,
`a_vetted_poll_shows_the_bytes_it_compiles_and_a_refusal_keeps_the_old_code`.
