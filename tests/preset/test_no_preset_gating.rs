//! I15 — `test_no_preset_gating` (Ch.31 §31.1, §31.4; DoD M2-12): **a preset sets
//! defaults; it never gates capability.** Every `#[forge_api]` item, every command, every
//! extension-point item and every panel resolves under every built-in preset (2D and 3D).
//!
//! For each preset the check builds the editor exactly as a project using it runs: the
//! first-party plugin set through the ordinary loader with the preset's workspace (its
//! layout, keymap and panel build context), and a core whose project was created from the
//! preset's own template (its default settings, its new-scene template — the preset's data).
//! It then collects the capability surface:
//!
//! * the editor's `#[forge_api]` registry (`forge_editor::inspect::editor_api_registry`) — every
//!   type and function;
//! * every `Invoke` target on the core's bus, and how each **plans** under this project:
//!   - with **valid sample arguments** ([`samples`]: built with each command's own
//!     constructor, with the prerequisite commands before it — an asset's rename after the
//!     import that makes it — dry-run as one batch). Every sample must plan under
//!     every preset: a command that refuses valid arguments only under one preset ("not
//!     available in 2D projects") is the Unity-style gate. A target with no sample fails the
//!     check, so a new command cannot slip past it;
//!   - with no arguments (`{}`), recording the **full** refusal (code and message), so a
//!     gate placed before the argument check is named too;
//! * every item on the editor's extension points (panels, actions, palette commands,
//!   overlays, themes, inspector widgets, viewport tools), and whether each panel builds
//!   content under the preset.
//!
//! The surfaces must be identical; an item missing under one preset is named with it, and
//! a command that plans differently is named once with every preset's outcome side by side.
//!
//! Positive controls (W2), each a real way a preset could gate:
//! * `positive_control_a_loader_that_loads_only_the_preset_plugins_fails` — loading only the
//!   preset's default plugin set (instead of loading it by default) hides panels under 3D;
//!   the check names them.
//! * `positive_control_the_real_command_gated_in_2d_before_its_argument_check_fails` — the
//!   real `forge.project.link_remote` planner behind a "not in 2D" refusal that runs before
//!   it parses its arguments (so `{}` is refused differently under 2D).
//! * `positive_control_the_real_command_gated_in_2d_after_its_argument_check_fails` — the
//!   same gate after the real planner accepted the arguments: `{}` is refused identically
//!   under every preset, so only the valid sample catches it.

use std::collections::BTreeSet;
use std::sync::atomic::{AtomicU64, Ordering};

use forge_cmd::{Bus, CmdError, DiffBuilder, EditorCommand, Issuer, TxnId, Value};
use forge_editor::client::BusClient;
use forge_editor::collab;
use forge_editor::core::EditorCore;
use forge_editor::panels::check_panels_under_presets;
use forge_editor::play::{PLAY_INPUT_CMD, PlayCommand, play_command};
use forge_editor::presets::{Family, Preset, builtin_presets};
use forge_editor::project::promote::promote_command;
use forge_editor::project::{
    LINK_REMOTE_CMD, ProjectOp, Template, link_remote_command, save_command, template_doc,
};
use forge_editor::security;
use forge_editor::shell::assemble;
use forge_editor::stand_in::StandInPanels;
use forge_plugin::{Capability, CommandClass, FsScope, PluginId, Principal, SourcePlugin};
use forge_project::collab::Role;

/// A preset's capability surface: `kind:name` strings.
type Surface = BTreeSet<String>;

/// The real command the positive controls gate in 2D projects.
const GATED: &str = LINK_REMOTE_CMD;

/// Where a gate sits in the positive controls' gated planner.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Gate {
    /// Refuse 2D projects before the arguments are read.
    BeforeArgs,
    /// Refuse 2D projects once the real planner accepted the arguments.
    AfterArgs,
}

/// How a surface is built (the positive controls build it the ways a gate would).
#[derive(Clone, Copy, PartialEq, Eq)]
enum Build {
    /// As the editor does: every installed plugin loads under every preset.
    Real,
    /// A loader that loads only the preset's default plugin set.
    OnlyPresetPlugins,
    /// The real build with a real command gated in 2D projects.
    Gated(Gate),
}

fn template_of(p: &Preset) -> Template {
    match p.workspace.id.as_str() {
        "forge.preset.2d" => Template::TwoD,
        _ => Template::ThreeD,
    }
}

const REMOTE: &str = "https://github.com/forge-engine/orbits.git";

fn invoke(target: &str, args: serde_json::Value) -> EditorCommand {
    EditorCommand::Invoke {
        target: target.into(),
        args: args.to_string(),
    }
}

/// Valid arguments for `target` against the core's current project: batches to dry-run,
/// each ending with a `target` command after the commands it needs. `None`: no sample (the
/// check fails, naming the target).
const I15_PLUGIN: &str = "com.example.i15";
fn i15_automation() -> Principal {
    Principal::Automation("auto-i15".into())
}
fn i15_plugin() -> Principal {
    Principal::Plugin(PluginId::new(I15_PLUGIN).expect("a valid plugin id"))
}
fn i15_cap() -> Capability {
    Capability::Command(CommandClass::Destructive)
}
fn samples(bus: &mut Bus, target: &str) -> Option<Vec<Vec<EditorCommand>>> {
    let family = match bus.project().setting(forge_project::PRESET_SETTING) {
        Some(Value::Text(t)) => Family::from_dir(t),
        _ => None,
    }
    .unwrap_or(Family::ThreeD);
    let ship = serde_json::json!({ "source": "models/ship.gltf", "settings": { "mips": "false" } });
    let import = invoke("forge.asset.import", ship.clone());
    let one = |c: EditorCommand| vec![vec![c]];
    Some(match target {
        "forge.asset.import" => one(import),
        "forge.asset.adopt" => one(invoke(
            "forge.asset.adopt",
            serde_json::json!({ "assets": [ship], "clear": ["models/old.gltf"] }),
        )),
        "forge.asset.remove" => vec![vec![
            import,
            invoke(
                "forge.asset.remove",
                serde_json::json!({ "source": "models/ship.gltf" }),
            ),
        ]],
        "forge.asset.rename" => vec![vec![
            import,
            invoke(
                "forge.asset.rename",
                serde_json::json!({ "from": "models/ship.gltf", "to": "models/hull.gltf" }),
            ),
        ]],
        // Scene composition (WP-U20): a scene, an instance of it, and what acts on them. Keys
        // are the ones the batch allocates, in order (the library first, if the project has
        // none yet).
        t if t.starts_with("forge.scene.") => {
            let k0 = bus.project().next_key().0;
            let has_library = bus.project().roots().any(|r| {
                bus.project()
                    .entity(r)
                    .and_then(|e| e.property("scene.library"))
                    == Some(&Value::Bool(true))
            });
            let root = k0 + u64::from(!has_library);
            let inst = root + 1;
            let new = invoke(
                "forge.scene.new",
                serde_json::json!({ "name": "I15 tree", "id": "i15_tree" }),
            );
            let instance = invoke(
                "forge.scene.instance",
                serde_json::json!({ "scene": "i15_tree" }),
            );
            match t {
                "forge.scene.new" => one(new),
                "forge.scene.new_inherited" => vec![vec![
                    new,
                    invoke(
                        "forge.scene.new_inherited",
                        serde_json::json!({ "base": "i15_tree", "id": "i15_oak" }),
                    ),
                ]],
                "forge.scene.instance" => vec![vec![new, instance]],
                "forge.scene.revert" => vec![vec![
                    new,
                    instance,
                    invoke("forge.scene.revert", serde_json::json!({ "entity": inst })),
                ]],
                "forge.scene.make_local" => vec![vec![
                    new,
                    instance,
                    invoke(
                        "forge.scene.make_local",
                        serde_json::json!({ "entity": inst }),
                    ),
                ]],
                "forge.scene.pack" => vec![vec![
                    EditorCommand::Spawn {
                        name: "I15 lamp".into(),
                        parent: None,
                    },
                    invoke(
                        "forge.scene.pack",
                        serde_json::json!({ "entity": k0, "id": "i15_lamp" }),
                    ),
                ]],
                _ => return None,
            }
        }
        "forge.play.control" => [
            PlayCommand::Play,
            PlayCommand::Pause,
            PlayCommand::Step(3),
            PlayCommand::Stop,
        ]
        .into_iter()
        .map(|c| vec![play_command(c)])
        .collect(),
        // A simulation input (WP-19): well-formed arguments are all the bus checks; the play
        // core checks the body when the input arrives.
        "forge.play.input" => [
            r#"{"Impulse":[0.0,5.0,0.0]}"#,
            r#"{"SetVelocity":[1.0,0.0,0.0]}"#,
            r#"{"SetAcceleration":[0.0,-9.81,0.0]}"#,
            r#"{"SetSpin":90.0}"#,
        ]
        .into_iter()
        .map(|action| {
            vec![EditorCommand::Invoke {
                target: PLAY_INPUT_CMD.into(),
                args: format!(r#"{{"entity":0,"action":{action}}}"#),
            }]
        })
        .collect(),
        "forge.project.create" => Template::ALL
            .into_iter()
            .map(|template| {
                vec![
                    ProjectOp::Create {
                        location: "memory:i15-new".into(),
                        name: "New".into(),
                        template,
                        discard_unsaved: true,
                    }
                    .command(),
                ]
            })
            .collect(),
        "forge.project.open" => one(ProjectOp::Open {
            location: "memory:i15-other".into(),
            discard_unsaved: true,
        }
        .command()),
        "forge.project.clone" => one(ProjectOp::Clone {
            remote: REMOTE.into(),
            location: "memory:i15-clone".into(),
            discard_unsaved: true,
        }
        .command()),
        "forge.project.save" => vec![vec![save_command("I15")], vec![save_command("")]],
        "forge.project.push" => one(ProjectOp::Push.command()),
        "forge.project.pull" => one(ProjectOp::Pull.command()),
        "forge.export.build" => ["windows", "linux", "web", "android"]
            .into_iter()
            .map(|t| {
                vec![invoke(
                    "forge.export.build",
                    serde_json::json!({ "target": t }),
                )]
            })
            .collect(),
        "forge.project.sign_in" => vec![
            vec![ProjectOp::SignIn { resume: false }.command()],
            vec![ProjectOp::SignIn { resume: true }.command()],
        ],
        "forge.project.create_remote" => one(ProjectOp::CreateRemote {
            name: "orbits".into(),
        }
        .command()),
        "forge.project.link_remote" => {
            vec![
                vec![link_remote_command(REMOTE)],
                vec![link_remote_command("")],
            ]
        }
        // To every other family (to its own is "already uses": not a gate).
        "forge.project.promote" => Family::ALL
            .into_iter()
            .filter(|f| *f != family)
            .map(|f| vec![promote_command(f, true)])
            .collect(),
        "forge.project.load" => Template::ALL
            .into_iter()
            .map(|t| {
                template_doc(t, "Loaded")
                    .and_then(|d| d.load_command())
                    .map(|c| vec![c])
            })
            .collect::<Result<_, _>>()
            .ok()?,
        // The security class (WP-U9): human-only, but a dry run plans them like any command.
        "forge.automation.grant" => one(security::grant_command(&i15_automation(), i15_cap())),
        "forge.automation.revoke" => vec![vec![
            security::grant_command(&i15_automation(), i15_cap()),
            security::revoke_command(&i15_automation(), i15_cap()),
        ]],
        "forge.plugin.grant" => one(security::grant_command(&i15_plugin(), i15_cap())),
        "forge.plugin.revoke" => vec![vec![
            security::grant_command(&i15_plugin(), i15_cap()),
            security::revoke_command(&i15_plugin(), i15_cap()),
        ]],
        "forge.automation.default_policy" => one(security::policy_command(&[Capability::Fs(
            FsScope::ProjectRead,
        )])),
        "forge.automation.approve" => one(security::approve_command(TxnId(1))),
        "forge.automation.reject" => one(security::reject_command(TxnId(1))),
        // WP-33: planned here (the core checks them against the proposal it holds).
        "forge.security.accept_held" => {
            let key = security::grant_key(&i15_plugin(), i15_cap()).unwrap_or_default();
            one(security::accept_held_command(&security::HeldSecurity {
                id: 1,
                by: "automation:i15".into(),
                source: "memory:i15".into(),
                untrusted: false,
                items: vec![security::HeldSetting {
                    key,
                    file: Some(Value::Bool(true)),
                    in_effect: None,
                }],
            }))
        }
        "forge.security.discard_held" => {
            one(security::discard_held_command(&security::HeldSecurity {
                id: 1,
                by: "automation:i15".into(),
                source: "memory:i15".into(),
                untrusted: false,
                items: Vec::new(),
            }))
        }
        "forge.plugin.add" => one(security::plugin_add_command(I15_PLUGIN, "1.0.0", "i15")),
        "forge.plugin.remove" => vec![vec![
            security::plugin_add_command(I15_PLUGIN, "1.0.0", "i15"),
            security::plugin_remove_command(I15_PLUGIN),
        ]],
        "forge.plugin.set_enabled" => vec![vec![
            security::plugin_add_command(I15_PLUGIN, "1.0.0", "i15"),
            security::plugin_enabled_command(I15_PLUGIN, false),
        ]],
        // Teams and collaboration (WP-U10): the planners check arguments; the core performs
        // them (a dry run on a bare bus plans them like any command).
        "forge.team.create" => one(collab::create_command("I15 team")),
        "forge.team.bind" => one(invoke(
            "forge.team.bind",
            serde_json::json!({"team": "team-i15", "name": "I15"}),
        )),
        "forge.team.invite" => one(collab::invite_command(None, Role::Developer, &[])),
        "forge.team.revoke_invite" => one(collab::revoke_invite_command("inv-1")),
        "forge.team.accept" => one(collab::accept_command("ABCD-EFGH")),
        "forge.team.set_role" => one(collab::set_role_command("i15", Role::Viewer)),
        "forge.team.set_paths" => one(collab::set_paths_command("i15", &["scene/**".into()])),
        "forge.team.remove" => one(collab::remove_command("i15")),
        "forge.collab.claim" => one(collab::claim_command(forge_cmd::EntityKey(0))),
        "forge.collab.release" => one(collab::release_command(forge_cmd::EntityKey(0))),
        "forge.collab.publish" => one(collab::publish_command("i15")),
        "forge.collab.approve" => one(collab::approve_command(1)),
        "forge.collab.reject" => one(collab::reject_command(1, "i15")),
        "forge.collab.pull" => one(collab::pull_command(None)),
        "forge.collab.resolve" => one(collab::resolve_command(&Default::default())),
        "forge.collab.review_policy" => one(collab::review_policy_command(
            true,
            &[("scene/Sandbox/**".into(), false)],
        )),
        "forge.collab.sync" => one(invoke(
            "forge.collab.sync",
            forge_project::merge::sync_args(&[], None, "i15"),
        )),
        _ => return None,
    })
}

/// How one batch plans: `plans`, or the step that refused and its full refusal.
fn plan(bus: &mut Bus, batch: Vec<EditorCommand>) -> String {
    // The security class is human-only (WP-U9): its samples plan as a human would issue them.
    let human_only = batch.iter().any(|c| {
        matches!(c, EditorCommand::Invoke { target, .. } if ["forge.automation.", "forge.plugin.", "forge.security.", "forge.team.", "forge.collab."].iter().any(|p| target.starts_with(p)))
    });
    let envs: Vec<_> = batch
        .into_iter()
        .map(|c| {
            let issuer = if human_only {
                Issuer::Human { user: "i15".into() }
            } else {
                Issuer::Script {
                    path: "i15.fscript".into(),
                }
            };
            bus.envelope(issuer, c)
        })
        .collect();
    match bus.dry_run_batch(&envs) {
        Ok(_) => "plans".into(),
        Err((i, r)) => format!("refused at step {i}: {}", r.error),
    }
}

/// A bus with the editor's handlers, the real planner of [`GATED`] behind a 2D gate first.
fn gated_bus(gate: Gate) -> Bus {
    let mut bus = Bus::new();
    let (target, policy, real) = forge_editor::project::handlers()
        .into_iter()
        .find(|(t, ..)| *t == GATED)
        .expect("the gated command is a lifecycle command");
    let refuse = || CmdError::BadArgs {
        target: GATED.into(),
        why: "remotes are not available in 2D projects".into(),
    };
    bus.register_handler(
        target,
        policy,
        move |b: &mut DiffBuilder<'_>, args: &serde_json::Value| {
            let two_d = b.project().setting(forge_project::PRESET_SETTING)
                == Some(&Value::Text("2d".into()));
            if gate == Gate::BeforeArgs && two_d {
                return Err(refuse());
            }
            real(b, args)?;
            if gate == Gate::AfterArgs && two_d {
                return Err(refuse());
            }
            Ok(())
        },
    )
    .expect("the gated command registers");
    // The rest as the editor registers them; the gated command keeps the gated planner.
    EditorCore::register_editor_handlers(&mut bus);
    bus
}

fn surface(preset: &Preset, build: Build) -> Surface {
    static RUN: AtomicU64 = AtomicU64::new(0);
    let mut s = Surface::new();
    // ---- the editor's extension points, as the editor binary loads them -----------------
    let core_p = forge_panels_core::PanelsCore::new().expect("forge-panels-core manifest");
    let scene = forge_panels_scene::PanelsScene::new().expect("forge-panels-scene manifest");
    let assets = forge_panels_assets::PanelsAssets::new().expect("forge-panels-assets manifest");
    let domain = forge_panels_domain::PanelsDomain::new().expect("forge-panels-domain manifest");
    let authoring =
        forge_panels_authoring::PanelsAuthoring::new().expect("forge-panels-authoring manifest");
    let project =
        forge_panels_project::PanelsProject::new().expect("forge-panels-project manifest");
    let mut real = forge_panels_core::panel_ids();
    real.extend(forge_panels_scene::panel_ids());
    real.extend(forge_panels_assets::panel_ids());
    real.extend(forge_panels_domain::panel_ids());
    real.extend(forge_panels_authoring::panel_ids());
    real.extend(forge_panels_project::panel_ids());
    let stand_in = StandInPanels::without(&real).expect("stand-in manifest");
    let all: Vec<&dyn SourcePlugin> = vec![
        &core_p, &scene, &assets, &domain, &authoring, &project, &stand_in,
    ];
    let plugins: Vec<&dyn SourcePlugin> = match build {
        Build::OnlyPresetPlugins => all
            .into_iter()
            .filter(|p| {
                preset
                    .workspace
                    .plugins
                    .iter()
                    .any(|id| *id == p.manifest().id.to_string())
            })
            .collect(),
        _ => all,
    };
    let cfg = assemble(preset.clone(), &plugins, &[], None).expect("the editor assembles");
    let unbuilt: BTreeSet<String> =
        check_panels_under_presets(&cfg.panels, std::slice::from_ref(preset))
            .into_iter()
            .collect();
    for id in cfg.panels.keys() {
        // A panel is available when it is registered and builds content under the preset.
        if !unbuilt.iter().any(|p| p.contains(id)) {
            s.insert(format!("panel:{id}"));
        }
    }
    s.extend(cfg.actions.keys().map(|k| format!("action:{k}")));
    s.extend(cfg.commands.iter().map(|c| format!("command:{}", c.target)));
    s.extend(cfg.overlays.keys().map(|k| format!("overlay:{k}")));
    s.extend(cfg.themes.keys().map(|k| format!("theme:{k}")));
    s.extend(
        cfg.services
            .inspector_widgets
            .keys()
            .map(|k| format!("inspector-widget:{k}")),
    );
    s.extend(
        cfg.services
            .viewport_tools
            .keys()
            .map(|k| format!("viewport-tool:{k}")),
    );

    // ---- a core whose project uses the preset -------------------------------------------
    let core = match build {
        Build::Gated(gate) => EditorCore::with_bus(gated_bus(gate)),
        _ => EditorCore::new(),
    };
    let mut client = EditorCore::connect(
        &core,
        Issuer::Script {
            path: "i15.fscript".into(),
        },
    );
    let n = RUN.fetch_add(1, Ordering::Relaxed);
    client.apply(
        ProjectOp::Create {
            location: format!("memory:i15-{}-{n}", preset.workspace.id),
            name: "I15".into(),
            template: template_of(preset),
            discard_unsaved: false,
        }
        .command(),
        None,
    );
    let _ = client.pump();
    let st = EditorCore::project_status(&core);
    assert!(
        st.open.is_some(),
        "{}: the project was created from the preset: {:?}",
        preset.workspace.id,
        st.outcomes
    );
    EditorCore::read(&core, |p| {
        assert_eq!(
            p.setting(forge_project::PRESET_SETTING),
            template_of(preset)
                .preset()
                .map(|d| Value::Text(d.into()))
                .as_ref(),
            "the project uses the preset"
        );
    });
    let targets: Vec<String> =
        EditorCore::configure(&core, |bus| bus.targets().map(str::to_string).collect());
    for t in &targets {
        s.insert(format!("invoke:{t}"));
        let (bare, sampled) = EditorCore::configure(&core, |bus| {
            let bare = plan(
                bus,
                vec![EditorCommand::Invoke {
                    target: t.clone(),
                    args: "{}".into(),
                }],
            );
            let sampled = match samples(bus, t) {
                None => "NO SAMPLE: add valid arguments for it to samples()".to_string(),
                Some(batches) => {
                    let outcomes: BTreeSet<String> =
                        batches.into_iter().map(|b| plan(bus, b)).collect();
                    outcomes.into_iter().collect::<Vec<_>>().join(" | ")
                }
            };
            (bare, sampled)
        });
        s.insert(format!("bare:{t} -> {bare}"));
        s.insert(format!("plans:{t} -> {sampled}"));
    }
    // ---- the #[forge_api] registry every API client reads ------------------------------
    let registry = forge_editor::inspect::editor_api_registry().expect("the API registry");
    for item in registry.items() {
        s.insert(format!("api:{}", item.path));
        if let Some(c) = &item.command {
            // A mutating fn resolves when its command is on the bus.
            if targets.contains(&c.target) {
                s.insert(format!("api-command:{}", c.target));
            }
        }
    }
    s
}

/// The kinds of surface item that record how a command plans (`<kind>:<target> -> <outcome>`).
const OUTCOME_KINDS: [&str; 2] = ["bare", "plans"];

/// Between the presets' outcomes on one problem line (an outcome itself may hold ` | `).
const SIDE_BY_SIDE: &str = " || ";

/// `(kind, target)` when `item` is a command's planning outcome.
fn outcome_of(item: &str) -> Option<(&'static str, &str)> {
    OUTCOME_KINDS.into_iter().find_map(|kind| {
        let rest = item.strip_prefix(kind)?.strip_prefix(':')?;
        Some((kind, rest.split_once(" -> ")?.0))
    })
}

/// Every item missing under a preset, named with the preset (empty: I15 holds).
///
/// A command whose outcome differs between presets is one line per target and kind, with
/// **every** preset's outcome side by side (`[preset] outcome`, joined by [`SIDE_BY_SIDE`]),
/// so the line shows at once which preset refuses and what the others do instead.
fn gating(surfaces: &[(String, Surface)]) -> Vec<String> {
    let union: Surface = surfaces
        .iter()
        .flat_map(|(_, s)| s.iter().cloned())
        .collect();
    let mut out = Vec::new();
    let mut differing: BTreeSet<(&'static str, String)> = BTreeSet::new();
    for (id, s) in surfaces {
        for item in union.difference(s) {
            match outcome_of(item) {
                Some((kind, target)) => {
                    differing.insert((kind, target.to_string()));
                }
                None => out.push(format!("{id}: {item} is not available under this preset")),
            }
        }
    }
    for (kind, target) in differing {
        let prefix = format!("{kind}:{target} -> ");
        let side: Vec<String> = surfaces
            .iter()
            .map(|(id, s)| {
                let outcome = s
                    .iter()
                    .find_map(|i| i.strip_prefix(prefix.as_str()))
                    .unwrap_or("(not on the bus)");
                format!("[{id}] {outcome}")
            })
            .collect();
        out.push(format!(
            "{prefix}differs by preset: {}",
            side.join(SIDE_BY_SIDE)
        ));
    }
    out
}

fn surfaces(build: Build) -> Vec<(String, Surface)> {
    let presets = builtin_presets().expect("the built-in presets load");
    assert_eq!(presets.len(), 2);
    presets
        .iter()
        .map(|p| (p.workspace.id.clone(), surface(p, build)))
        .collect()
}

#[test]
fn test_no_preset_gating() {
    let all = surfaces(Build::Real);
    let problems = gating(&all);
    assert!(problems.is_empty(), "I15 broken:\n{}", problems.join("\n"));
    // Non-vacuity: the surface really holds the editor.
    let s = &all[0].1;
    let count = |kind: &str| s.iter().filter(|i| i.starts_with(kind)).count();
    assert!(count("panel:") >= 30, "panels: {}", count("panel:"));
    assert!(count("action:") >= 20, "actions: {}", count("action:"));
    assert!(count("invoke:") >= 15, "commands: {}", count("invoke:"));
    assert!(count("api:") >= 5, "#[forge_api] items: {}", count("api:"));
    for must in [
        "panel:forge.editors_2d",
        "invoke:forge.project.promote",
        "invoke:forge.project.link_remote",
    ] {
        assert!(s.contains(must), "{must} is in the surface");
    }
    // Every command was dry-run with valid arguments under each preset's project, and
    // every sample planned there (not merely "is registered", and not a refusal that
    // happens to be the same everywhere).
    assert_eq!(count("plans:"), count("invoke:"));
    assert_eq!(count("bare:"), count("invoke:"));
    for (preset, s) in &all {
        let not_planned: Vec<&String> = s
            .iter()
            .filter(|i| i.starts_with("plans:") && !i.ends_with("-> plans"))
            .collect();
        assert!(
            not_planned.is_empty(),
            "{preset}: valid arguments must plan: {not_planned:#?}"
        );
    }
}

#[test]
fn positive_control_a_loader_that_loads_only_the_preset_plugins_fails() {
    let problems = gating(&surfaces(Build::OnlyPresetPlugins));
    assert!(!problems.is_empty(), "a preset-filtered plugin set passed");
    // The 2D editors live in forge.panels.domain, which only the 2D preset loads by
    // default: under 3D they are gone, and that is named.
    let p = "forge.preset.3d";
    assert!(
        problems
            .iter()
            .any(|l| l.starts_with(&format!("{p}: panel:forge.editors_2d"))),
        "{p} loses the 2D editors: {problems:#?}"
    );
}

/// The gated command is named, and nothing else is: one `which` line with every preset's
/// outcome side by side — the 2D refusal next to what 3D does.
fn assert_gate_named(problems: &[String], which: &str) {
    assert!(!problems.is_empty(), "a 2D-gated command passed ({which})");
    assert!(
        problems
            .iter()
            .all(|l| l.starts_with(&format!("bare:{GATED} -> "))
                || l.starts_with(&format!("plans:{GATED} -> "))),
        "only the gated command is named: {problems:#?}"
    );
    let head = format!("{which}:{GATED} -> differs by preset: ");
    let lines: Vec<&String> = problems.iter().filter(|l| l.starts_with(&head)).collect();
    assert_eq!(
        lines.len(),
        1,
        "one line for the target ({which}): {problems:#?}"
    );
    let side: Vec<&str> = lines[0][head.len()..].split(SIDE_BY_SIDE).collect();
    let presets = ["forge.preset.2d", "forge.preset.3d"];
    assert_eq!(
        side.len(),
        presets.len(),
        "every preset's outcome on the line ({which}): {}",
        lines[0]
    );
    for p in presets {
        let tag = format!("[{p}] ");
        let outcome = side
            .iter()
            .find_map(|s| s.strip_prefix(tag.as_str()))
            .unwrap_or_else(|| panic!("{p}'s outcome is on the line ({which}): {}", lines[0]));
        let gated = outcome.contains("not available in 2D projects");
        assert_eq!(
            gated,
            p == "forge.preset.2d",
            "{p}: the 2D gate shows under 2D only ({which}): {}",
            lines[0]
        );
        if which == "plans" && p != "forge.preset.2d" {
            assert_eq!(outcome, "plans", "{p} plans the valid sample: {}", lines[0]);
        }
    }
}

#[test]
fn positive_control_the_real_command_gated_in_2d_before_its_argument_check_fails() {
    let problems = gating(&surfaces(Build::Gated(Gate::BeforeArgs)));
    // Refused before its arguments are read: `{}` is refused differently under 2D, and the
    // valid sample plans everywhere but 2D.
    assert_gate_named(&problems, "bare");
    assert_gate_named(&problems, "plans");
}

#[test]
fn positive_control_the_real_command_gated_in_2d_after_its_argument_check_fails() {
    let problems = gating(&surfaces(Build::Gated(Gate::AfterArgs)));
    // `{}` is refused by the real argument check under every preset alike: only the valid
    // sample shows the gate.
    assert_gate_named(&problems, "plans");
    assert!(
        !problems
            .iter()
            .any(|l| l.contains(&format!("bare:{GATED}"))),
        "the bare refusal is the same everywhere: {problems:#?}"
    );
}
