//! The **Settings** window (`forge.settings`, Ch.21 §21.18, DoD M2-43), generated from
//! reflection (`forge_editor::settings`).
//!
//! * **Editor** rows (theme, UI scale, reduced motion, caret blink) are user config: an
//!   edit changes the session's editor settings, which the shell applies to every window
//!   at once and saves to the per-user config directory.
//! * **Project** rows are project state: an edit is an `EditorCommand::SetSetting` sent
//!   through the emitter — one command per toggle, choice or committed number, and **one
//!   gesture** (one transaction, one undo entry) per slider drag, cancelled by Esc. The
//!   rows follow the mirror, so an automation session's or a script's change shows here next frame.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use forge_cmd::{EditorCommand, Value};
use forge_editor::emitter::Gesture;
use forge_editor::panel_rt::{PanelBuilder, PanelSync, ShellHandles};
use forge_editor::panels::PanelCx;
use forge_editor::session::SessionState;
use forge_editor::session::ShellRequest;
use forge_editor::settings::{
    RowEditor, SettingRow, SettingsPage, applies_at_next_start, editor_page, lifecycle_value,
    project_editor_page, project_graphics_page, project_lifecycle_page, row_link, row_note,
    variant_label,
};
use forge_ui::dock::PanelId;

use forge_ui::widgets::{
    Button, Checkbox, ComboBox, Label, LabelKind, NumericCommitted, NumericField, Pressed, Slider,
    SliderEdit, SliderPhase, Submitted, TextField,
};
use forge_ui::{NodeStyle, UiError, WidgetId};

/// Where a row's value lives.
#[derive(Clone, Debug, PartialEq)]
enum Target {
    /// An editor-settings field (user config).
    User(String),
    /// A project setting key (project state, on the bus).
    Project(String),
}

/// Integer-valued rows send `Value::Int`.
fn is_int(r: &SettingRow) -> bool {
    matches!(r.default, Value::Int(_))
}

/// A reflected string (a field's label, tooltip or unit, a variant, a page or group title)
/// in the UI language: reflection metadata is a localisation key like any UI text (M2-31).
fn l(s: &str) -> String {
    forge_ui::l10n::tr_str(s).into_owned()
}

fn ui_err(e: forge_editor::EditorError) -> UiError {
    UiError::Layout(e.to_string())
}

fn current(
    target: &Target,
    row: &SettingRow,
    session: &SessionState,
    mirror: &forge_editor::mirror::ProjectMirror,
) -> Value {
    match target {
        Target::User(f) => session
            .settings()
            .get_field(f)
            .unwrap_or_else(|| row.default.clone()),
        Target::Project(k) => row.shown(mirror.setting(k)).clone(),
    }
}

fn as_f64(v: &Value) -> f64 {
    match v {
        Value::Float(x) => *x,
        Value::Int(i) => *i as f64,
        _ => 0.0,
    }
}

fn as_bool(v: &Value) -> bool {
    matches!(v, Value::Bool(true))
}

fn as_text(v: &Value) -> String {
    match v {
        Value::Text(s) => s.clone(),
        other => format!("{other:?}"),
    }
}

fn variant_index(row: &SettingRow, v: &Value) -> usize {
    let t = as_text(v);
    row.variants.iter().position(|n| *n == t).unwrap_or(0)
}

/// The number a row stores: snapped to its step (a slider's f32 must not leak
/// `0.10000000149` into the project) and an integer for integer rows.
fn number(row: &SettingRow, x: f64) -> Value {
    let x = match row.field.step.filter(|s| *s > 0.0) {
        Some(step) => {
            let base = row.field.min.unwrap_or(0.0);
            let snapped = ((x - base) / step).round() * step + base;
            // Print-and-parse at the step's precision removes the binary residue.
            let decimals = (-step.log10()).ceil().max(0.0) as usize;
            format!("{snapped:.decimals$}").parse().unwrap_or(snapped)
        }
        None => x,
    };
    if is_int(row) {
        Value::Int(x.round() as i64)
    } else {
        Value::Float(x)
    }
}

/// Write from a handler (the session is already borrowed by the dispatcher).
fn write_in_handler(
    target: &Target,
    v: Value,
    session: &mut SessionState,
    cmd: &forge_editor::emitter::CommandEmitter,
) {
    match target {
        Target::User(f) => session.update_settings(|s| {
            s.set_field(f, &v);
        }),
        Target::Project(k) => {
            cmd.emit(EditorCommand::SetSetting {
                key: k.clone(),
                value: Some(v),
            });
        }
    }
}

/// Write from a signal effect (nothing else holds the session then).
fn write_in_effect(target: &Target, v: Value, h: &ShellHandles) {
    let key = match target {
        Target::User(k) | Target::Project(k) => k,
    };
    if applies_at_next_start(key) {
        // The GPU pool is built at startup: say so, never restart silently.
        h.session_mut().notify(
            forge_editor::notify::Level::Info,
            forge_ui::tr!("Applies at next start"),
            forge_ui::tr!("The GPU mode changes the next time the program starts."),
        );
    }
    match target {
        Target::User(f) => h.session_mut().update_settings(|s| {
            s.set_field(f, &v);
        }),
        Target::Project(k) => {
            h.cmd().emit(EditorCommand::SetSetting {
                key: k.clone(),
                value: Some(v),
            });
        }
    }
}

fn page(pb: &mut PanelBuilder, key: &str, page: &SettingsPage, note: &str) -> Result<(), UiError> {
    let space = pb.b.theme_ref().space;
    let parent = pb.parent;
    pb.b.add(
        parent,
        forge_ui::Key::Str(format!("{key}.title").into()),
        NodeStyle::leaf().padding(space[1]),
        Label::new(l(&page.title)).kind(LabelKind::Heading),
    )?;
    pb.b.add(
        parent,
        forge_ui::Key::Str(format!("{key}.note").into()),
        NodeStyle::leaf().padding(space[1]),
        Label::new(note).kind(LabelKind::Muted).wrapping(),
    )?;
    for (gi, (group, rows)) in page.groups.iter().enumerate() {
        if let Some(g) = group
            && page.groups.len() > 1
        {
            pb.b.add(
                parent,
                forge_ui::Key::Str(format!("{key}.g{gi}").into()),
                NodeStyle::leaf().padding(space[1]),
                Label::new(l(g)).kind(LabelKind::Small),
            )?;
        }
        for r in rows {
            let target = if page.project {
                Target::Project(r.key.clone())
            } else {
                Target::User(r.key.clone())
            };
            row(pb, key, r, target)?;
        }
    }
    Ok(())
}

fn label_of(r: &SettingRow) -> String {
    match r.field.units {
        Some(u) => forge_ui::trf!("{label} ({unit})", label = l(r.field.label), unit = l(u)),
        None => l(r.field.label),
    }
}

fn row(
    pb: &mut PanelBuilder,
    page_key: &str,
    r: &SettingRow,
    target: Target,
) -> Result<(), UiError> {
    let space = pb.b.theme_ref().space;
    let line = pb.b.add(
        pb.parent,
        forge_ui::Key::Str(format!("{page_key}.{}", r.key).into()),
        NodeStyle::row(space[2]).padding(space[1]),
        forge_ui::widgets::Container::new(forge_ui::Role::Group).labelled(&l(r.field.label)),
    )?;
    let label = label_of(r);
    if r.editor() != RowEditor::Toggle {
        pb.b.add(
            line,
            "label",
            NodeStyle::leaf().width(170.0),
            Label::new(label.as_str()),
        )?;
    }
    let init = {
        let m = pb.mirror();
        let s = pb.session();
        current(&target, r, &s, &m)
    };
    let h = pb.handles();
    let edit: WidgetId = match r.editor() {
        RowEditor::Toggle => {
            let sig = pb.b.signal(as_bool(&init));
            let known = Rc::new(Cell::new(as_bool(&init)));
            let id =
                pb.b.add(line, "edit", NodeStyle::leaf(), Checkbox::new(sig, &label))?;
            let (k, t, hh) = (known.clone(), target.clone(), h.clone());
            pb.b.runtime().effect(move |rt| {
                let v = sig.get(rt);
                if v != k.get() {
                    k.set(v);
                    write_in_effect(&t, Value::Bool(v), &hh);
                }
            });
            let (row_c, t) = (r.clone(), target.clone());
            pb.sync(id, move |s: &mut PanelSync| {
                let v = as_bool(&current(&t, &row_c, s.session, s.mirror));
                if v != known.get() {
                    known.set(v);
                    sig.set(s.ui.rt_mut(), v);
                }
                Ok(())
            });
            id
        }
        RowEditor::Choice => {
            let shown: Vec<String> = r
                .variants
                .iter()
                .map(|v| l(variant_label(&r.key, v).unwrap_or(v)))
                .collect();
            let opts: Vec<&str> = shown.iter().map(String::as_str).collect();
            let i0 = variant_index(r, &init);
            let sig = pb.b.signal(i0);
            let known = Rc::new(Cell::new(i0));
            let id = pb.b.add(
                line,
                "edit",
                NodeStyle::leaf().width(200.0),
                ComboBox::new(&label, &opts, sig),
            )?;
            let (k, t, hh, variants) =
                (known.clone(), target.clone(), h.clone(), r.variants.clone());
            pb.b.runtime().effect(move |rt| {
                let i = sig.get(rt);
                if i != k.get() {
                    k.set(i);
                    if let Some(name) = variants.get(i) {
                        write_in_effect(&t, Value::Text(name.clone()), &hh);
                    }
                }
            });
            let (row_c, t) = (r.clone(), target.clone());
            pb.sync(id, move |s: &mut PanelSync| {
                let i = variant_index(&row_c, &current(&t, &row_c, s.session, s.mirror));
                if i != known.get() {
                    known.set(i);
                    sig.set(s.ui.rt_mut(), i);
                }
                Ok(())
            });
            id
        }
        RowEditor::Slider => {
            let (min, max) = (r.field.min.unwrap_or(0.0), r.field.max.unwrap_or(1.0));
            let step = r.field.step.unwrap_or((max - min) / 100.0);
            let x0 = as_f64(&init);
            let sig = pb.b.signal(x0 as f32);
            let known = Rc::new(Cell::new(x0));
            let id = pb.b.add(
                line,
                "edit",
                NodeStyle::leaf().width(220.0),
                Slider::new(sig, &label, min as f32, max as f32, step as f32),
            )?;
            let readout = pb.b.signal(format_number(r, x0));
            pb.b.add(
                line,
                "value",
                NodeStyle::leaf().width(80.0),
                Label::new(readout),
            )?;
            let gesture: Rc<RefCell<Option<Gesture>>> = Rc::new(RefCell::new(None));
            let (k, t, row_c, g) = (known.clone(), target.clone(), r.clone(), gesture.clone());
            pb.on(id, move |act, e: &SliderEdit| {
                let v = number(&row_c, f64::from(e.value));
                k.set(as_f64(&v));
                readout.set(act.ui.rt_mut(), format_number(&row_c, as_f64(&v)));
                let Target::Project(key) = &t else {
                    // User config: follow the thumb live; nothing to undo.
                    write_in_handler(&t, v, act.session, act.cmd);
                    return;
                };
                let cmd = EditorCommand::SetSetting {
                    key: key.clone(),
                    value: Some(v),
                };
                match e.phase {
                    SliderPhase::Begin => {
                        let mut gg = act
                            .cmd
                            .gesture(&forge_ui::trf!("Drag {label}", label = row_c.field.label));
                        gg.update(cmd);
                        *g.borrow_mut() = Some(gg);
                    }
                    SliderPhase::Update => match g.borrow_mut().as_mut() {
                        Some(gg) => gg.update(cmd),
                        None => {
                            act.cmd.emit(cmd);
                        }
                    },
                    SliderPhase::End => {
                        if let Some(gg) = g.borrow_mut().take() {
                            gg.commit();
                        }
                    }
                    SliderPhase::Cancel => {
                        if let Some(gg) = g.borrow_mut().take() {
                            gg.cancel();
                        }
                    }
                    SliderPhase::Step => {
                        act.cmd.emit(cmd);
                    }
                }
            });
            let (row_c, t) = (r.clone(), target.clone());
            pb.sync(id, move |s: &mut PanelSync| {
                if gesture.borrow().is_some() {
                    return Ok(()); // mid-drag: the thumb is the truth until release
                }
                let v = as_f64(&current(&t, &row_c, s.session, s.mirror));
                if v != known.get() {
                    known.set(v);
                    sig.set(s.ui.rt_mut(), v as f32);
                    readout.set(s.ui.rt_mut(), format_number(&row_c, v));
                }
                Ok(())
            });
            id
        }
        RowEditor::Number => {
            let x0 = as_f64(&init);
            let sig = pb.b.signal(x0);
            let known = Rc::new(Cell::new(x0));
            let mut field = NumericField::new(sig, &label);
            if let Some(u) = r.field.units {
                field = field.unit(&l(u));
            }
            if let (Some(a), Some(b)) = (r.field.min, r.field.max) {
                field = field.range(a, b);
            }
            if let Some(s) = r.field.step {
                field = field.step(s);
            }
            let id =
                pb.b.add(line, "edit", NodeStyle::leaf().width(160.0), field)?;
            let (k, t, row_c) = (known.clone(), target.clone(), r.clone());
            pb.on(id, move |act, e: &NumericCommitted| {
                let v = number(&row_c, e.value);
                k.set(as_f64(&v));
                write_in_handler(&t, v, act.session, act.cmd);
            });
            let (row_c, t) = (r.clone(), target.clone());
            pb.sync(id, move |s: &mut PanelSync| {
                let v = as_f64(&current(&t, &row_c, s.session, s.mirror));
                if v != known.get() {
                    known.set(v);
                    sig.set(s.ui.rt_mut(), v);
                }
                Ok(())
            });
            id
        }
        RowEditor::Text => {
            let s0 = as_text(&init);
            let sig = pb.b.signal(s0.clone());
            let known = Rc::new(RefCell::new(s0));
            let id = pb.b.add(
                line,
                "edit",
                NodeStyle::leaf().width(220.0),
                TextField::new(sig, &label),
            )?;
            let (k, t) = (known.clone(), target.clone());
            pb.on(id, move |act, e: &Submitted| {
                if *k.borrow() != e.text {
                    *k.borrow_mut() = e.text.clone();
                    write_in_handler(&t, Value::Text(e.text.clone()), act.session, act.cmd);
                }
            });
            let (row_c, t) = (r.clone(), target.clone());
            pb.sync(id, move |s: &mut PanelSync| {
                let v = as_text(&current(&t, &row_c, s.session, s.mirror));
                if v != *known.borrow() {
                    *known.borrow_mut() = v.clone();
                    sig.set(s.ui.rt_mut(), v);
                }
                Ok(())
            });
            id
        }
    };
    let _ = edit;
    if let Target::Project(key) = target {
        let reset = pb.b.add(
            line,
            "reset",
            NodeStyle::leaf(),
            Button::new(forge_ui::tr!("Reset")),
        )?;
        pb.on(reset, move |act, _: &Pressed| {
            if act.mirror.setting(&key).is_some() {
                act.cmd.emit(EditorCommand::SetSetting {
                    key: key.clone(),
                    value: None,
                });
            }
        });
    }
    if let Some(note) = row_note(&r.key) {
        pb.b.add(
            pb.parent,
            forge_ui::Key::Str(format!("{page_key}.{}.note", r.key).into()),
            NodeStyle::leaf().padding(space[1]),
            Label::new(l(note)).kind(LabelKind::Muted).wrapping(),
        )?;
    }
    Ok(())
}

/// The **Project** page (WP-U7, M2-47): generated from `ProjectLifecycleSettings` like any
/// project settings — its editable rows (name, version) are ordinary project-setting rows
/// (every edit a command); its read-only rows show where the value comes from and open the
/// dialog that changes it (remote → Revision history, preset → the preset switcher,
/// → the preset switcher), so each lossy change keeps its one confirmation path.
fn lifecycle_page(pb: &mut PanelBuilder, page: &SettingsPage) -> Result<(), UiError> {
    let space = pb.b.theme_ref().space;
    let parent = pb.parent;
    pb.b.add(
        parent,
        "lifecycle.title",
        NodeStyle::leaf().padding(space[1]),
        Label::new(l(&page.title)).kind(LabelKind::Heading),
    )?;
    pb.b.add(
        parent,
        "lifecycle.note",
        NodeStyle::leaf().padding(space[1]),
        Label::new(
            forge_ui::tr!("The project's name and version are project settings: every change is a command.              The remote and the preset change in their own dialogs,              which say what a change costs before it happens."),
        )
        .kind(LabelKind::Muted)
        .wrapping(),
    )?;
    for r in page.rows() {
        if r.field.read_only {
            read_only_row(pb, r)?;
        } else {
            row(pb, "lifecycle", r, Target::Project(r.key.clone()))?;
        }
    }
    Ok(())
}

fn read_only_row(pb: &mut PanelBuilder, r: &SettingRow) -> Result<(), UiError> {
    let space = pb.b.theme_ref().space;
    let line = pb.b.add(
        pb.parent,
        forge_ui::Key::Str(format!("lifecycle.{}", r.key).into()),
        NodeStyle::row(space[2]).padding(space[1]),
        forge_ui::widgets::Container::new(forge_ui::Role::Group).labelled(&l(r.field.label)),
    )?;
    pb.b.add(
        line,
        "label",
        NodeStyle::leaf().width(170.0),
        Label::new(l(r.field.label)).tooltip(&l(r.field.tooltip)),
    )?;
    let v0 = lifecycle_value(&r.key, &pb.mirror());
    let value = pb.b.signal(v0);
    pb.b.add(
        line,
        "value",
        NodeStyle::leaf().grow(1.0),
        Label::new(value).wrapping(),
    )?;
    // The editor's own dialog for the row, else the link a plugin added beside it (WP-47).
    let link = row_link(&r.key, &pb.services().setting_links);
    if let Some((panel, text)) = link {
        let open =
            pb.b.add(line, "open", NodeStyle::leaf(), Button::new(l(&text)))?;
        pb.on(open, move |act, _: &Pressed| {
            act.session
                .request(ShellRequest::OpenPanel(PanelId::new(&panel)));
        });
    }
    let key = r.key.clone();
    let mut seen = pb.mirror().revision();
    pb.sync(line, move |s: &mut PanelSync| {
        let rev = s.mirror.revision();
        if rev != seen {
            seen = rev;
            let v = lifecycle_value(&key, s.mirror);
            if value.get(s.ui.rt()) != v {
                value.set(s.ui.rt_mut(), v);
            }
        }
        Ok(())
    });
    Ok(())
}

fn format_number(r: &SettingRow, x: f64) -> String {
    let decimals = r
        .field
        .step
        .filter(|s| *s > 0.0 && *s < 1.0)
        .map_or(0, |s| (-s.log10()).ceil() as usize);
    match r.field.units {
        Some(u) => forge_ui::trf!(
            "{value} {unit}",
            value = format!("{x:.decimals$}"),
            unit = l(u)
        ),
        None => format!("{x:.decimals$}"),
    }
}

pub fn build(cx: &mut PanelCx) {
    cx.never_empty(forge_ui::tr!("the editor and project settings forms"));
    cx.add_live(|pb| {
        let editor = editor_page().map_err(ui_err)?;
        let project = project_editor_page().map_err(ui_err)?;
        let lifecycle = project_lifecycle_page().map_err(ui_err)?;
        let graphics = project_graphics_page().map_err(ui_err)?;
        let space = pb.b.theme_ref().space;
        pb.parent = pb.b.add(
            pb.parent,
            "content",
            NodeStyle::column(space[1])
                .padding(space[2])
                .grow(1.0)
                .scrollable(),
            forge_ui::widgets::Container::new(forge_ui::Role::Group)
                .labelled(forge_ui::tr!("Settings")),
        )?;
        page(
            pb,
            "editor",
            &editor,
            forge_ui::tr!(
                "Your settings, on this computer only \u{2014} never part of the project."
            ),
        )?;
        // M2-70: dismissed first-run tips are user config; show them all again.
        let reset_tips = pb.b.add(
            pb.parent,
            "reset_tips",
            NodeStyle::leaf(),
            Button::new(forge_ui::tr!("Reset tips")),
        )?;
        pb.on(reset_tips, |act, _: &Pressed| {
            act.session.update_settings(|s| s.reset_tips());
            act.session.notify(
                forge_editor::notify::Level::Success,
                forge_ui::tr!("Tips reset"),
                forge_ui::tr!("Each panel shows its tip again the next time it opens."),
            );
        });
        page(
            pb,
            "project",
            &project,
            forge_ui::tr!(
                "Project settings are shared with everyone on the project. Every change is a \
             command: undoable, and listed in the undo history with who made it."
            ),
        )?;
        lifecycle_page(pb, &lifecycle)?;
        page(
            pb,
            "graphics",
            &graphics,
            forge_ui::tr!(
                "What the exported game does. Project settings: every change is a command."
            ),
        )?;
        Ok(())
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slider_values_are_snapped_to_the_step_without_binary_residue() {
        let p = project_editor_page().unwrap_or_else(|e| panic!("{e}"));
        let grid = p.rows().next().unwrap_or_else(|| panic!("row"));
        assert_eq!(number(grid, f64::from(0.3_f32)), Value::Float(0.3));
        assert_eq!(number(grid, 2.04), Value::Float(2.0));
        assert_eq!(format_number(grid, 2.0), "2.0 m");
    }
}
