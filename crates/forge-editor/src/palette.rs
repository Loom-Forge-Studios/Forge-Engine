//! The command palette (Ch.21 §21.17; DoD M2-45): one fuzzy-searchable list over every
//! action, every panel ("Open: Profiler") and every command — each `EditorCommand` variant
//! that can be built with no arguments or from the selection, a prompt for the ones that
//! need arguments, and every registered `Invoke` handler — so every registered command is
//! reachable by keyboard. Results show their chords, and recently used entries rank first.
//!
//! Picking returns a [`PaletteOutcome`] for the shell: commands go on the bus (I7), session
//! operations change session state. The palette itself changes nothing.

use forge_cmd::EditorCommand;
use forge_plugin::Registry;
use forge_plugin::points::Command;
use forge_ui::dock::PanelId;
use forge_ui::fuzzy::rank;
use forge_ui::widgets::PickItem;

use crate::EditorError;
use crate::actions::{Action, ActionCx, Invocation, invoke};
use crate::keymap::KeyMap;

/// A registered `Invoke` handler (a `#[forge_api]` mutating fn or a plugin command).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommandInfo {
    pub target: String,
    /// Declared argument names (`None`: the handler declares none; it gets `{}`).
    pub fields: Option<Vec<String>>,
}

/// Every plugin command in a `Command` registry.
pub fn commands_from_registry(reg: &Registry<Command>) -> Vec<CommandInfo> {
    reg.keys()
        .map(|k| CommandInfo {
            target: k.to_string(),
            fields: None,
        })
        .collect()
}

/// How the palette reaches one `EditorCommand` variant.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VariantReach {
    /// Built with no arguments (the selection, if any, supplies context).
    Constructible(&'static str),
    /// Built from the selection; offered only while something is selected.
    WithSelection(&'static str),
    /// Needs arguments: the entry opens a prompt for these fields.
    NeedsArguments(&'static str, &'static [&'static str]),
    /// One entry per registered handler.
    PerHandler,
}

/// Every `EditorCommand` variant and how the palette reaches it. The coverage guard
/// (`palette_reaches_every_editor_command_variant`) compares this against the variants
/// `EditorCommand` really has, so a variant added to the bus cannot be missing here.
// l10n-block: each row is an EditorCommand variant's name (an identifier) and its entry's key
pub const VARIANTS: &[(&str, VariantReach)] = &[
    (
        "Spawn",
        VariantReach::Constructible(forge_ui::tr_key!("Create entity")),
    ),
    (
        "Despawn",
        VariantReach::WithSelection(forge_ui::tr_key!("Delete selected entities")),
    ),
    (
        "Rename",
        VariantReach::NeedsArguments(
            forge_ui::tr_key!("Rename entity\u{2026}"),
            &["entity", "name"],
        ),
    ),
    (
        "Reparent",
        VariantReach::WithSelection(forge_ui::tr_key!("Unparent selected entities")),
    ),
    (
        "SetProperty",
        VariantReach::NeedsArguments(
            forge_ui::tr_key!("Set property\u{2026}"),
            &["entity", "path", "value"],
        ),
    ),
    (
        "RemoveProperty",
        VariantReach::NeedsArguments(
            forge_ui::tr_key!("Remove property\u{2026}"),
            &["entity", "path"],
        ),
    ),
    (
        "SetSetting",
        VariantReach::NeedsArguments(
            forge_ui::tr_key!("Set project setting\u{2026}"),
            &["key", "value"],
        ),
    ),
    ("Invoke", VariantReach::PerHandler),
];

/// What an entry does.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PaletteTarget {
    Action(String),
    OpenPanel(PanelId),
    /// An `EditorCommand` variant by name.
    Variant(&'static str),
    /// A registered `Invoke` handler.
    Invoke(CommandInfo),
}

/// One palette row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PaletteEntry {
    /// Unique and stable (recency is keyed by it): `action:<id>`, `panel:<id>`,
    /// `command:<Variant>`, `invoke:<target>`.
    pub key: String,
    pub label: String,
    pub category: String,
    /// The chord shown on the right (the first one, outermost context).
    pub chord: Option<String>,
    pub enabled: bool,
    pub target: PaletteTarget,
}

/// What picking an entry asks the shell to do.
#[derive(Clone, Debug, PartialEq)]
pub enum PaletteOutcome {
    /// Run an action's invocation (commands go on the bus as one transaction).
    Run(Invocation),
    /// Open (or focus) a panel.
    OpenPanel(PanelId),
    /// Prompt for these arguments, then send `command`.
    NeedsArguments {
        command: String,
        fields: Vec<String>,
    },
}

/// How many recently used entries are remembered (user config).
pub const RECENT_CAP: usize = 32;
const RECENT_BOOST: i32 = 40;

/// The palette model (see the module docs).
#[derive(Clone, Debug, Default)]
pub struct Palette {
    entries: Vec<PaletteEntry>,
    recent: Vec<String>,
}

impl Palette {
    /// Build the entries from the registries as they are now.
    pub fn build(
        actions: &Registry<Action>,
        keymap: &KeyMap,
        panels: &[(PanelId, String)],
        commands: &[CommandInfo],
        cx: &ActionCx,
    ) -> Self {
        let mut entries = Vec::new();
        for (id, a) in actions.iter() {
            entries.push(PaletteEntry {
                key: format!("action:{id}"),
                label: forge_ui::l10n::tr_str(&a.title).into_owned(),
                category: forge_ui::l10n::tr_str(&a.category).into_owned(),
                chord: keymap.chords_for(id).first().map(|c| c.label()),
                enabled: a.is_enabled(cx),
                target: PaletteTarget::Action(id.to_string()),
            });
        }
        for (p, title) in panels {
            entries.push(PaletteEntry {
                key: format!("panel:{p}"),
                label: forge_ui::trf!("Open: {title}", title = forge_ui::l10n::tr_str(title)),
                category: forge_ui::tr!("Panels").into(),
                chord: keymap
                    .chords_for(&format!("forge.panel.open.{p}"))
                    .first()
                    .map(|c| c.label()),
                enabled: true,
                target: PaletteTarget::OpenPanel(p.clone()),
            });
        }
        for (name, reach) in VARIANTS {
            let (label, enabled) = match reach {
                VariantReach::Constructible(l) => (*l, true),
                VariantReach::WithSelection(l) => (*l, !cx.selection.is_empty()),
                VariantReach::NeedsArguments(l, _) => (*l, true),
                VariantReach::PerHandler => continue,
            };
            entries.push(PaletteEntry {
                key: format!("command:{name}"),
                label: forge_ui::l10n::tr_str(label).into_owned(),
                category: forge_ui::tr!("Commands").into(),
                chord: None,
                enabled,
                target: PaletteTarget::Variant(name),
            });
        }
        for c in commands {
            entries.push(PaletteEntry {
                key: format!("invoke:{}", c.target),
                label: if c.fields.as_ref().is_some_and(|f| !f.is_empty()) {
                    forge_ui::trf!("Run: {command}\u{2026}", command = c.target)
                } else {
                    forge_ui::trf!("Run: {command}", command = c.target)
                },
                category: forge_ui::tr!("Commands").into(),
                chord: None,
                enabled: true,
                target: PaletteTarget::Invoke(c.clone()),
            });
        }
        Self {
            entries,
            recent: Vec::new(),
        }
    }

    pub fn entries(&self) -> &[PaletteEntry] {
        &self.entries
    }

    /// The recently used keys, most recent first (persist as user config).
    pub fn recent(&self) -> &[String] {
        &self.recent
    }

    pub fn set_recent(&mut self, recent: Vec<String>) {
        self.recent = recent;
        self.recent.truncate(RECENT_CAP);
    }

    fn boost(&self, e: &PaletteEntry) -> i32 {
        let recency = self
            .recent
            .iter()
            .position(|k| *k == e.key)
            .map_or(0, |i| RECENT_BOOST - i as i32);
        recency - if e.enabled { 0 } else { 1000 }
    }

    fn text(e: &PaletteEntry) -> String {
        format!("{}: {}", e.category, e.label)
    }

    /// Entry indices matching `query`, best first: fuzzy score plus recency; unavailable
    /// entries last.
    pub fn search(&self, query: &str) -> Vec<usize> {
        let texts: Vec<String> = self.entries.iter().map(Self::text).collect();
        let mut r = rank(query, &texts, String::as_str);
        r.sort_by_key(|(i, m)| {
            (
                std::cmp::Reverse(m.score + self.boost(&self.entries[*i])),
                *i,
            )
        });
        r.into_iter().map(|(i, _)| i).collect()
    }

    /// The popup's items (the `forge-ui` pick list ranks them by the same boost).
    pub fn pick_items(&self) -> Vec<PickItem> {
        self.entries
            .iter()
            .map(|e| {
                let detail = match (&e.chord, e.enabled) {
                    (_, false) => forge_ui::tr!("not available here").to_string(),
                    (Some(c), true) => c.clone(),
                    (None, true) => String::new(),
                };
                PickItem::new(&e.key, &Self::text(e))
                    .detail(&detail)
                    .hint(&e.category)
                    .boost(self.boost(e))
            })
            .collect()
    }

    /// Run the entry `key`. It moves to the front of the recent list.
    pub fn pick(
        &mut self,
        key: &str,
        actions: &Registry<Action>,
        cx: &ActionCx,
    ) -> Result<PaletteOutcome, EditorError> {
        let e = self
            .entries
            .iter()
            .find(|e| e.key == key)
            .cloned()
            .ok_or_else(|| EditorError::UnknownAction(key.to_string()))?;
        if !e.enabled {
            return Err(EditorError::ActionDisabled(key.to_string()));
        }
        let out = match &e.target {
            PaletteTarget::Action(id) => PaletteOutcome::Run(invoke(actions, id, cx)?),
            PaletteTarget::OpenPanel(p) => PaletteOutcome::OpenPanel(p.clone()),
            PaletteTarget::Variant(name) => variant_outcome(name, &e.label, cx)?,
            PaletteTarget::Invoke(c) => match &c.fields {
                Some(f) if !f.is_empty() => PaletteOutcome::NeedsArguments {
                    command: c.target.clone(),
                    fields: f.clone(),
                },
                _ => PaletteOutcome::Run(Invocation::Commands {
                    label: c.target.clone(),
                    commands: vec![EditorCommand::Invoke {
                        target: c.target.clone(),
                        args: "{}".into(),
                    }],
                }),
            },
        };
        self.recent.retain(|k| k != key);
        self.recent.insert(0, key.to_string());
        self.recent.truncate(RECENT_CAP);
        Ok(out)
    }
}

fn variant_outcome(name: &str, label: &str, cx: &ActionCx) -> Result<PaletteOutcome, EditorError> {
    let reach = VARIANTS
        .iter()
        .find(|(n, _)| *n == name)
        .map(|(_, r)| *r)
        .ok_or_else(|| EditorError::UnknownAction(name.to_string()))?;
    let run = |commands: Vec<EditorCommand>| {
        PaletteOutcome::Run(Invocation::Commands {
            label: label.to_string(),
            commands,
        })
    };
    // l10n-block: matched by EditorCommand variant name (identifiers); a spawned entity's name is
    // project data
    Ok(match (name, reach) {
        ("Spawn", _) => run(vec![EditorCommand::Spawn {
            name: "Entity".into(),
            parent: cx.selection.first().copied(),
        }]),
        ("Despawn", _) => run(cx
            .selection
            .iter()
            .map(|e| EditorCommand::Despawn { entity: *e })
            .collect()),
        ("Reparent", _) => run(cx
            .selection
            .iter()
            .map(|e| EditorCommand::Reparent {
                entity: *e,
                parent: None,
            })
            .collect()),
        (_, VariantReach::NeedsArguments(_, fields)) => PaletteOutcome::NeedsArguments {
            command: name.to_string(),
            fields: fields.iter().map(|f| f.to_string()).collect(),
        },
        _ => return Err(EditorError::UnknownAction(name.to_string())),
    })
}
