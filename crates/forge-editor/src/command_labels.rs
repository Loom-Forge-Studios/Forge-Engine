//! Command labels as a person reads them, in the UI locale (Ch.21 §21.23, M2-31).
//!
//! The bus names a one-command transaction with `EditorCommand::label` — the English,
//! canonical form every peer, the journal and the audit share (`Set e1.light.kind`). The
//! editor shows it translated: [`command_label`] builds the text from a command, and
//! [`history_label`] from a label the bus recorded, by recognising the canonical forms (a
//! label the editor gave a gesture is a string key, looked up as it is).
//! `test::every_command_label_is_recognised` fails if forge-cmd adds a command or rewords a
//! label without this module following.

use forge_cmd::EditorCommand;

/// `e17` — how `EntityKey` displays.
fn is_entity(s: &str) -> bool {
    s.strip_prefix('e')
        .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
}

/// `entity.path` with an entity key before the first dot.
fn entity_path(s: &str) -> Option<(&str, &str)> {
    let (e, p) = s.split_once('.')?;
    (is_entity(e) && !p.is_empty()).then_some((e, p))
}

/// What `cmd` does, in the UI locale (a refusal's message, a console link).
#[must_use]
pub fn command_label(cmd: &EditorCommand) -> String {
    match cmd {
        EditorCommand::Spawn { name, .. } => forge_ui::trf!("Spawn {name}", name),
        EditorCommand::Despawn { entity } => forge_ui::trf!("Delete {entity}", entity),
        EditorCommand::Rename { entity, name } => {
            forge_ui::trf!("Rename {entity} to {name}", entity, name)
        }
        EditorCommand::Reparent { entity, .. } => forge_ui::trf!("Move {entity}", entity),
        EditorCommand::SetProperty { entity, path, .. } => {
            forge_ui::trf!("Set {entity}.{path}", entity, path)
        }
        EditorCommand::RemoveProperty { entity, path } => {
            forge_ui::trf!("Remove {entity}.{path}", entity, path)
        }
        EditorCommand::SetSetting { key, .. } => forge_ui::trf!("Set setting {key}", key),
        // A handler's name is an identifier (`forge.project.link_remote`).
        EditorCommand::Invoke { target, .. } => target.clone(),
    }
}

/// The kind of a property value, in the UI locale (`Value::kind` is the canonical
/// identifier logs and files use).
#[must_use]
pub fn value_kind_label(v: &forge_cmd::Value) -> &'static str {
    use forge_cmd::Value;
    match v {
        Value::Bool(_) => forge_ui::tr!("true or false"),
        Value::Int(_) => forge_ui::tr!("whole number"),
        Value::Float(_) => forge_ui::tr!("number"),
        Value::Text(_) => forge_ui::tr!("text"),
        Value::Vec3(_) => forge_ui::tr!("3D vector"),
        Value::Entity(_) => forge_ui::tr!("entity"),
    }
}

/// A transaction's label as the undo history, the menus and the console show it: the
/// bus's canonical form of a built-in command translated, an identifier (an invoked
/// handler's name) as it is, anything else — a label the editor gave a gesture — looked
/// up as a string key.
#[must_use]
pub fn history_label(label: &str) -> String {
    // l10n: the bus's canonical form (EditorCommand::label), matched, never shown
    if let Some(key) = label.strip_prefix("Set setting ") {
        return forge_ui::trf!("Set setting {key}", key);
    }
    // l10n: the bus's canonical form (EditorCommand::label), matched, never shown
    if let Some((entity, path)) = label.strip_prefix("Set ").and_then(entity_path) {
        return forge_ui::trf!("Set {entity}.{path}", entity, path);
    }
    // l10n: the bus's canonical form (EditorCommand::label), matched, never shown
    if let Some((entity, path)) = label.strip_prefix("Remove ").and_then(entity_path) {
        return forge_ui::trf!("Remove {entity}.{path}", entity, path);
    }
    if let Some((entity, name)) = label
        // l10n: the bus's canonical form (EditorCommand::label), matched, never shown
        .strip_prefix("Rename ")
        .and_then(|r| r.split_once(" to "))
        .filter(|(e, _)| is_entity(e))
    {
        return forge_ui::trf!("Rename {entity} to {name}", entity, name);
    }
    // l10n: the bus's canonical form (EditorCommand::label), matched, never shown
    if let Some(entity) = label.strip_prefix("Delete ").filter(|e| is_entity(e)) {
        return forge_ui::trf!("Delete {entity}", entity);
    }
    // l10n: the bus's canonical form (EditorCommand::label), matched, never shown
    if let Some(entity) = label.strip_prefix("Move ").filter(|e| is_entity(e)) {
        return forge_ui::trf!("Move {entity}", entity);
    }
    // l10n: the bus's canonical form (EditorCommand::label), matched, never shown
    if let Some(name) = label.strip_prefix("Spawn ") {
        return forge_ui::trf!("Spawn {name}", name);
    }
    let identifier = !label.is_empty()
        && !label.contains(' ')
        && label.contains('.')
        && label
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "._-:".contains(c));
    if identifier {
        return label.to_string();
    }
    forge_ui::l10n::tr_str(label).into_owned()
}

#[cfg(test)]
mod test {
    use super::*;

    /// Every command's canonical label is recognised: under the pseudo-locale the history
    /// shows exactly what [`command_label`] builds from the command itself (an invoked
    /// handler's identifier as it is, everything else translated).
    #[test]
    fn every_command_label_is_recognised() {
        forge_ui::l10n::set_ui_locale(
            forge_ui::l10n::PSEUDO_LOCALE,
            std::collections::BTreeMap::new(),
        );
        let mut translated = 0;
        for cmd in forge_cmd::contract::samples() {
            let canonical = cmd.label();
            let shown = history_label(&canonical);
            assert_eq!(shown, command_label(&cmd), "{canonical:?}");
            if !matches!(cmd, EditorCommand::Invoke { .. }) {
                assert!(
                    forge_ui::l10n::is_pseudo(&shown),
                    "{canonical:?} -> {shown:?}"
                );
                translated += 1;
            }
        }
        assert!(translated >= 7, "only {translated} built-in labels checked");
        // A gesture's label is a key; an identifier is shown as it is.
        assert!(forge_ui::l10n::is_pseudo(&history_label("Drag scale")));
        assert_eq!(
            history_label("forge.project.link_remote"),
            "forge.project.link_remote"
        );
        // `Move 3 entities` is a gesture's key, not `Move {entity}`.
        assert_eq!(
            history_label("Move 3 entities"),
            forge_ui::l10n::tr_str("Move 3 entities")
        );
        assert_ne!(
            history_label("Move 3 entities"),
            forge_ui::trf!("Move {entity}", entity = "3 entities")
        );
        forge_ui::l10n::clear_ui_locale();
    }

    /// Without a UI locale the labels are the canonical ones, unchanged.
    #[test]
    fn source_locale_shows_the_canonical_label() {
        forge_ui::l10n::clear_ui_locale();
        for cmd in forge_cmd::contract::samples() {
            assert_eq!(history_label(&cmd.label()), cmd.label());
            assert_eq!(command_label(&cmd), cmd.label());
        }
    }
}
