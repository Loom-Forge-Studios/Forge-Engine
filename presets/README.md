# Workspace presets (Ch.31)

| File | What it holds |
|---|---|
| `workspace.ron` | the default plugin set (loaded by default — never a filter: every installed plugin loads under every preset), the default layout file, which panels it opens, default project settings, the new-scene template, extension-point defaults, an optional keymap layer |
| `layout.ron` | the default dock layout (Ch.21 §21.17), the same RON a user layout uses |
| `keymap.ron` | an optional keymap layer over the editor defaults (`crates/forge-editor/data/keymap.ron`) |
| `templates/*.scene.ron` | the new-scene template: what a new project from this preset starts with, in the same format as a project's own `project/scene.ron` |

A preset chooses defaults and never gates a capability (I15): there is no field that could
hide a panel or a command, and `tests/preset/test_no_preset_gating.rs` checks every panel,
command, extension-point item and `#[forge_api]` item under all three. The `forge.presets`
plugin (`plugins/forge-presets`) compiles these directories in and provides them on the
`Preset` point, so they always load (`forge_editor::presets::builtin_presets`).

**Author your own by copying one.** Copy a preset directory into a project's own `presets/`
folder (`<project>/presets/3d/`) and edit it: that copy is what 3D means for that project — its
defaults are what switching to 3D applies (a value you set yourself always stays) — and it
travels with the project through every store and remote. A copy that does not load, or sits in
the wrong directory, is named in the preset switcher and the built-in preset stays in its place.
