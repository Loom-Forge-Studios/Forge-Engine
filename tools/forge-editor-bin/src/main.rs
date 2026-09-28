//! `forge-editor` — the Forge editor (Ch.21 §21.18) and its headless mode (Ch.34 §34.4):
//! [`forge_editor_bin::app`] with the base edition's first-party plugins and nothing more.

fn main() {
    forge_editor_bin::app::main_with(&mut forge_editor_bin::app::BaseEdition);
}
