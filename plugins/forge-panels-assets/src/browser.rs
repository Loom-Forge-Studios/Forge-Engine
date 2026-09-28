//! The **Asset browser** (`forge.assets`, Ch.21 §21.21, DoD M2-38).
//!
//! * Grid and list views over the asset catalogue ([`VirtualGrid`], virtualised: a
//!   100,000-asset library costs a screenful), folders (derived from source paths, plus
//!   folders made here), a breadcrumb and Up/Backspace, and search across every folder.
//! * **Async thumbnails.** A tile's thumbnail is asked of the catalogue the first time the
//!   tile is painted; it renders on the asset workers, and a ready thumbnail is posted to
//!   the UI thread, waking the loop once. An idle browser schedules nothing.
//! * **Every change is a command** (I7): rename in place (F2 or `assets.rename`) and moving
//!   tiles onto a folder are `forge.asset.rename`; renaming a folder moves everything in it
//!   (one transaction); Delete removes the registration (`forge.asset.remove`: undoable, the
//!   file stays); dropping OS files copies them into the folder shown and imports them
//!   (`forge.asset.import`, one transaction). An automation session's import appears here the same
//!   way.
//! * **Lock indicators** (Ch.33 §33.3): a tile whose source a store lock holds shows a
//!   lock badge and who holds it; renaming or moving it is refused here with that name.

use std::cell::RefCell;
use std::collections::{BTreeSet, HashMap};
use std::rc::Rc;
use std::sync::{Arc, Mutex};

use forge_cmd::EditorCommand;
use forge_editor::assets::{
    AssetCatalog, AssetRow, import_command, remove_command, rename_command,
};
use forge_editor::composition::{
    SCENE_FOLDER, SceneEntry, instance_command, new_scene_command, scene_entries, scene_payload,
};
use forge_editor::mirror::ProjectMirror;
use forge_editor::panels::PanelCx;
use forge_ui::ui::Poster;
use forge_ui::widgets::{
    Breadcrumb, BreadcrumbChosen, Button, Container, EmptyState, FilesDropped, GridMode, GridTile,
    GridUp, Label, LabelKind, Pressed, ReadyThumb, RowActivated, RowRenamed, RowsDeleteRequested,
    RowsDropped, SearchChanged, SearchField, ThumbProvider, VirtualGrid,
};
use forge_ui::{InputEvent, KeyCode, KeyEvent, Modifiers, NodeStyle, Role, Signal, Ui, WidgetId};

/// Thumbnail size in pixels.
pub const THUMB_PX: u32 = 96;
/// Folder tiles have this bit set in their key; asset tiles never do.
pub const FOLDER_BIT: u64 = 1 << 63;

fn folder_key(path: &str) -> u64 {
    // FNV-1a: stable across runs (a folder keeps its tile, its selection, its a11y id).
    let mut h: u64 = 0xCBF2_9CE4_8422_2325;
    for b in path.as_bytes() {
        h ^= u64::from(*b);
        h = h.wrapping_mul(0x0000_0100_0000_01B3);
    }
    h | FOLDER_BIT
}

fn asset_key(row: &AssetRow) -> u64 {
    row.key & !FOLDER_BIT
}

fn parent_folder(f: &str) -> String {
    f.rsplit_once('/')
        .map_or(String::new(), |(p, _)| p.to_string())
}

fn join(folder: &str, name: &str) -> String {
    if folder.is_empty() {
        name.to_string()
    } else {
        format!("{folder}/{name}")
    }
}

fn file_name(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

fn kind_glyph(kind: &str) -> &'static str {
    match kind {
        "texture" => "\u{25a7}",
        "mesh" => "\u{25b2}",
        "scene" => "\u{2756}",
        "material" => "\u{25cf}",
        _ => "\u{25a1}",
    }
}

/// The thumbnail provider over the catalogue (see the module docs).
struct Thumbs {
    catalog: Rc<RefCell<dyn AssetCatalog>>,
    poster: Option<Poster>,
    signal: Signal<u64>,
    inbox: Arc<Mutex<Vec<ReadyThumb>>>,
    /// Grid key -> catalogue key.
    keys: RefCell<HashMap<u64, u64>>,
    requests: std::cell::Cell<u64>,
}

impl ThumbProvider for Thumbs {
    fn request(&self, key: u64) {
        let Some(ck) = self.keys.borrow().get(&key).copied() else {
            return;
        };
        self.requests.set(self.requests.get() + 1);
        let inbox = Arc::clone(&self.inbox);
        let poster = self.poster.clone();
        let signal = self.signal;
        self.catalog.borrow().thumbnail(
            ck,
            THUMB_PX,
            Box::new(move |r| {
                if let Ok((size, rgba)) = r {
                    inbox
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .push(ReadyThumb {
                            key,
                            w: size,
                            h: size,
                            rgba,
                        });
                    if let Some(p) = poster {
                        p.post(move |rt| signal.update(rt, |g| *g += 1));
                    }
                }
            }),
        );
    }
    fn take_ready(&self) -> Vec<ReadyThumb> {
        std::mem::take(
            &mut *self
                .inbox
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
        )
    }
    fn ready_signal(&self) -> Signal<u64> {
        self.signal
    }
}

/// What the browser remembers (session state).
struct Browser {
    grid: WidgetId,
    crumbs: Signal<Vec<String>>,
    count: Signal<String>,
    cwd: String,
    query: String,
    /// Folders made here that hold no asset yet.
    made: BTreeSet<String>,
    rows: Vec<AssetRow>,
    by_key: HashMap<u64, AssetRow>,
    folders: HashMap<u64, String>,
    versions: HashMap<u64, u64>,
    seen_rev: u64,
    thumbs: Rc<Thumbs>,
    /// The project's scenes (WP-U20), listed in the scene library's folder: tiles to drag
    /// into the hierarchy or activate to place.
    scenes: Vec<SceneEntry>,
    scene_keys: HashMap<u64, SceneEntry>,
    /// The mirror's structure revision the scenes were read at.
    seen_structure: u64,
}

/// A scene tile's grid key (stable per scene id; bit 62 keeps it apart from asset keys).
fn scene_key(id: &str) -> u64 {
    (folder_key(&scene_payload(id)) & !FOLDER_BIT) | (1 << 62)
}

impl Browser {
    /// Re-read the project's scenes when the hierarchy changed. `true` if they did.
    fn sync_scenes(&mut self, m: &ProjectMirror) -> bool {
        // Scenes appear, go and are renamed only with the hierarchy's structure (a scene is
        // made with its root entity), so a property edit costs nothing here.
        let rev = m.structure_revision();
        if rev == self.seen_structure {
            return false;
        }
        self.seen_structure = rev;
        let fresh = scene_entries(m);
        if fresh == self.scenes {
            return false;
        }
        self.scene_keys = fresh
            .iter()
            .map(|s| (scene_key(&s.id), s.clone()))
            .collect();
        self.scenes = fresh;
        true
    }

    fn all_folders(&self) -> BTreeSet<String> {
        let mut out: BTreeSet<String> = self.made.clone();
        if !self.scenes.is_empty() {
            out.insert(SCENE_FOLDER.to_string());
        }
        for r in &self.rows {
            let mut f = r.folder.clone();
            while !f.is_empty() {
                out.insert(f.clone());
                f = parent_folder(&f);
            }
        }
        out
    }

    /// The tiles for the folder shown (or the search results).
    fn tiles(&mut self) -> Vec<(u64, GridTile)> {
        let q = self.query.trim().to_lowercase();
        let mut out = Vec::new();
        self.folders.clear();
        let mut thumb_keys = HashMap::new();
        if q.is_empty() {
            for f in self.all_folders() {
                if parent_folder(&f) == self.cwd {
                    let k = folder_key(&f);
                    out.push((k, GridTile::folder(file_name(&f))));
                    self.folders.insert(k, f);
                }
            }
        }
        for r in &self.rows {
            let shown = if q.is_empty() {
                r.folder == self.cwd
            } else {
                r.name.to_lowercase().contains(&q) || r.path.to_lowercase().contains(&q)
            };
            if !shown {
                continue;
            }
            let k = asset_key(r);
            thumb_keys.insert(k, r.key);
            let caption = match (&r.locked_by, q.is_empty()) {
                (Some(who), _) => {
                    forge_ui::trf!("{kind} \u{b7} locked by {who}", kind = r.kind, who)
                }
                (None, true) => r.kind.clone(),
                (None, false) => format!("{} \u{b7} {}", r.kind, r.folder),
            };
            // A source's tile is named by its file (renames show at once); a generated
            // asset has no file and shows its generator's name.
            let label = if r.generated {
                r.name.clone()
            } else {
                file_name(&r.path).to_string()
            };
            out.push((
                k,
                GridTile::new(label, caption)
                    .glyph(kind_glyph(&r.kind))
                    .badge(r.locked_by.as_ref().map(|_| "\u{1f512}"))
                    .drag_id(r.id.clone()),
            ));
        }
        *self.thumbs.keys.borrow_mut() = thumb_keys;
        // The project's scenes, in the scene library's folder (and in search results).
        for s in &self.scenes {
            let shown = if q.is_empty() {
                self.cwd == SCENE_FOLDER
            } else {
                s.name.to_lowercase().contains(&q) || s.id.contains(&q)
            };
            if !shown {
                continue;
            }
            let caption = match &s.inherits {
                Some(base) => forge_ui::trf!("scene \u{b7} derived from {base}", base),
                None => forge_ui::tr!("scene").to_string(),
            };
            out.push((
                scene_key(&s.id),
                GridTile::new(s.name.clone(), caption)
                    .glyph(kind_glyph("scene"))
                    .drag_id(scene_payload(&s.id)),
            ));
        }
        out
    }

    fn crumb_names(&self) -> Vec<String> {
        let mut v = vec![forge_ui::tr!("Project").to_string()];
        if !self.cwd.is_empty() {
            v.extend(self.cwd.split('/').map(str::to_string));
        }
        v
    }

    /// Re-read the catalogue if it changed; refresh the grid.
    fn refresh(&mut self, ui: &mut Ui, catalog: &dyn AssetCatalog, force: bool) {
        let rev = catalog.revision();
        if rev != self.seen_rev {
            self.seen_rev = rev;
            self.rows = catalog.rows();
            self.by_key = self
                .rows
                .iter()
                .map(|r| (asset_key(r), r.clone()))
                .collect();
        } else if !force {
            return;
        }
        let tiles = self.tiles();
        let stale: Vec<u64> = self
            .rows
            .iter()
            .filter(|r| {
                self.versions
                    .get(&asset_key(r))
                    .is_some_and(|v| *v != r.version)
            })
            .map(asset_key)
            .collect();
        self.versions = self
            .rows
            .iter()
            .map(|r| (asset_key(r), r.version))
            .collect();
        let n = tiles.len();
        let freed = VirtualGrid::edit(ui, self.grid, |g| {
            g.set_tiles(tiles);
            stale
                .iter()
                .filter_map(|k| g.invalidate_thumb(*k))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
        for img in freed {
            ui.remove_image(img);
        }
        let where_ = if self.query.trim().is_empty() {
            forge_ui::trf!(
                "{n} item(s) here \u{b7} {total} asset(s) in the project",
                n,
                total = self.rows.len()
            )
        } else {
            forge_ui::trf!(
                "{n} match(es) for \u{201c}{query}\u{201d}",
                n,
                query = self.query.trim()
            )
        };
        self.count.set(ui.rt_mut(), where_);
        self.crumbs.set(ui.rt_mut(), self.crumb_names());
    }

    /// The commands moving every asset in `keys` (folders included) into `dest`.
    fn move_commands(&self, keys: &[u64], dest: &str) -> (Vec<EditorCommand>, Vec<String>) {
        let mut cmds = Vec::new();
        let mut locked = Vec::new();
        let mut push = |r: &AssetRow, to: String, cmds: &mut Vec<EditorCommand>| {
            if r.generated || r.path == to {
                return;
            }
            match &r.locked_by {
                Some(who) => locked.push(format!("{} ({who})", r.name)),
                None => cmds.push(rename_command(&r.path, &to)),
            }
        };
        for k in keys {
            if let Some(r) = self.by_key.get(k) {
                push(r, join(dest, file_name(&r.path)), &mut cmds);
            } else if let Some(f) = self.folders.get(k) {
                // A folder moves with everything under it.
                let base = file_name(f).to_string();
                for r in self
                    .rows
                    .iter()
                    .filter(|r| r.folder == *f || r.folder.starts_with(&format!("{f}/")))
                {
                    let rest = &r.path[f.len() + 1..];
                    push(r, join(&join(dest, &base), rest), &mut cmds);
                }
            }
        }
        (cmds, locked)
    }
}

fn refuse_locked(session: &mut forge_editor::session::SessionState, locked: &[String]) {
    if !locked.is_empty() {
        session.problem(forge_editor::notify::Problem::coded(
            forge_editor::notify::Severity::Warning,
            "EDITOR-0014",
            &forge_ui::trf!("{n} asset(s) are locked", n = locked.len()),
            &forge_ui::trf!("Locked by a teammate: {names}", names = locked.join(", ")),
        ));
    }
}

pub fn build(cx: &mut PanelCx) {
    cx.add_live(|pb| {
        let space = pb.b.theme_ref().space;
        let services = pb.services();
        let Some(catalog) = services.assets.clone() else {
            pb.b.add(
                pb.parent,
                "empty",
                NodeStyle::leaf().grow(1.0),
                EmptyState::new(forge_ui::tr!(
                    "No asset database: open a project to browse and import its assets."
                )),
            )?;
            return Ok(());
        };
        let bar = pb.b.add(
            pb.parent,
            "bar",
            NodeStyle::row(space[1]).padding(space[1]),
            Container::new(Role::Toolbar).labelled(forge_ui::tr!("Asset browser actions")),
        )?;
        let up = pb.b.add(
            bar,
            "up",
            NodeStyle::leaf(),
            Button::new(forge_ui::tr!("Up")),
        )?;
        let crumbs = pb.b.signal(vec![forge_ui::tr!("Project").to_string()]);
        let crumb = pb.b.add(
            bar,
            "crumbs",
            NodeStyle::leaf().grow(1.0),
            Breadcrumb::new(crumbs, forge_ui::tr!("Folder")),
        )?;
        let query = pb.b.signal(String::new());
        let search = pb.b.add(
            bar,
            "search",
            NodeStyle::leaf().width(200.0),
            SearchField::new(query, forge_ui::tr!("Search assets")),
        )?;
        let grid_b = pb.b.add(
            bar,
            "mode.grid",
            NodeStyle::leaf(),
            Button::new(forge_ui::tr!("Grid")),
        )?;
        let list_b = pb.b.add(
            bar,
            "mode.list",
            NodeStyle::leaf(),
            Button::new(forge_ui::tr!("List")),
        )?;
        let newf = pb.b.add(
            bar,
            "new_folder",
            NodeStyle::leaf(),
            Button::new(forge_ui::tr!("New folder")),
        )?;
        let new_scene = pb.b.add(
            bar,
            "new_scene",
            NodeStyle::leaf(),
            Button::new(forge_ui::tr!("New scene")),
        )?;
        pb.on(new_scene, |act, _: &Pressed| {
            act.cmd.emit(new_scene_command(forge_ui::tr!("Scene")));
        });
        let thumbs = Rc::new(Thumbs {
            catalog: Rc::clone(&catalog),
            poster: pb.b.poster(),
            signal: pb.b.signal(0u64),
            inbox: Arc::new(Mutex::new(Vec::new())),
            keys: RefCell::new(HashMap::new()),
            requests: std::cell::Cell::new(0),
        });
        let count = pb.b.signal(String::new());
        let mut st = Browser {
            grid: WidgetId(0),
            crumbs,
            count,
            cwd: String::new(),
            query: String::new(),
            made: BTreeSet::new(),
            rows: catalog.borrow().rows(),
            by_key: HashMap::new(),
            folders: HashMap::new(),
            versions: HashMap::new(),
            seen_rev: catalog.borrow().revision(),
            thumbs: Rc::clone(&thumbs),
            scenes: Vec::new(),
            scene_keys: HashMap::new(),
            seen_structure: u64::MAX,
        };
        st.sync_scenes(&pb.mirror());
        st.by_key = st.rows.iter().map(|r| (asset_key(r), r.clone())).collect();
        st.versions = st.rows.iter().map(|r| (asset_key(r), r.version)).collect();
        let mut g = VirtualGrid::new(forge_ui::tr!("Assets"))
            .provider(thumbs.clone() as Rc<dyn ThumbProvider>);
        g.set_tiles(st.tiles());
        let grid =
            pb.b.add(pb.parent, "grid", NodeStyle::leaf().grow(1.0), g)?;
        st.grid = grid;
        let n0 = st.rows.len();
        count.update(pb.b.runtime(), |c| {
            *c = forge_ui::trf!("{n} asset(s) in the project", n = n0)
        });
        pb.b.add(
            pb.parent,
            "count",
            NodeStyle::leaf().padding(space[1]),
            Label::new(count).kind(LabelKind::Small),
        )?;
        pb.b.add(
            pb.parent,
            "backend",
            NodeStyle::leaf().padding(space[1]),
            Label::new(forge_ui::l10n::tr_str(catalog.borrow().backend()).into_owned())
                .kind(LabelKind::Small)
                .wrapping(),
        )?;
        let state = Rc::new(RefCell::new(st));

        let (s, c) = (state.clone(), catalog.clone());
        pb.on(grid, move |act, e: &RowActivated| {
            let mut b = s.borrow_mut();
            // A scene tile places the scene under the selected entity (a root without one).
            if let Some(scene) = b.scene_keys.get(&e.key) {
                let parent = act.session.selection.first().copied();
                act.cmd.emit_all(
                    forge_ui::tr!("Instance scene"),
                    vec![instance_command(&scene.id, parent)],
                );
                return;
            }
            if let Some(f) = b.folders.get(&e.key).cloned() {
                b.cwd = f;
                b.refresh(act.ui, &*c.borrow(), true);
            }
        });
        let (s, c) = (state.clone(), catalog.clone());
        pb.on(grid, move |act, _: &GridUp| {
            let mut b = s.borrow_mut();
            if !b.cwd.is_empty() {
                b.cwd = parent_folder(&b.cwd);
                b.refresh(act.ui, &*c.borrow(), true);
            }
        });
        let (s, c) = (state.clone(), catalog.clone());
        pb.on(up, move |act, _: &Pressed| {
            let mut b = s.borrow_mut();
            if !b.cwd.is_empty() {
                b.cwd = parent_folder(&b.cwd);
                b.refresh(act.ui, &*c.borrow(), true);
            }
        });
        let (s, c) = (state.clone(), catalog.clone());
        pb.on(crumb, move |act, e: &BreadcrumbChosen| {
            let mut b = s.borrow_mut();
            let parts: Vec<String> = b
                .cwd
                .split('/')
                .filter(|p| !p.is_empty())
                .map(str::to_string)
                .collect();
            b.cwd = parts[..e.index.min(parts.len())].join("/");
            b.refresh(act.ui, &*c.borrow(), true);
        });
        let (s, c) = (state.clone(), catalog.clone());
        pb.on(search, move |act, e: &SearchChanged| {
            let mut b = s.borrow_mut();
            b.query = e.query.clone();
            b.refresh(act.ui, &*c.borrow(), true);
        });
        pb.on(grid_b, move |act, _: &Pressed| {
            VirtualGrid::edit(act.ui, grid, |g| g.set_mode(GridMode::Grid));
        });
        pb.on(list_b, move |act, _: &Pressed| {
            VirtualGrid::edit(act.ui, grid, |g| g.set_mode(GridMode::List));
        });
        let (s, c) = (state.clone(), catalog.clone());
        pb.on(newf, move |act, _: &Pressed| {
            let mut b = s.borrow_mut();
            let existing = b.all_folders();
            let name = (1..)
                .map(|i| {
                    if i == 1 {
                        forge_ui::tr!("New folder").to_string()
                    } else {
                        forge_ui::trf!("New folder {i}", i)
                    }
                })
                .map(|n| join(&b.cwd, &n))
                .find(|p| !existing.contains(p))
                .unwrap_or_default();
            b.made.insert(name);
            b.refresh(act.ui, &*c.borrow(), true);
        });
        let s = state.clone();
        pb.on(grid, move |act, e: &RowRenamed| {
            let b = s.borrow();
            let name = e.name.trim();
            if name.contains('/') || name.is_empty() {
                act.session.problem(forge_editor::notify::Problem::coded(
                    forge_editor::notify::Severity::Warning,
                    "EDITOR-0014",
                    forge_ui::tr!("Names cannot contain \u{201c}/\u{201d}"),
                    forge_ui::tr!("Move a tile onto a folder to move it."),
                ));
                return;
            }
            // A scene tile renames the scene (its root entity).
            if let Some(scene) = b.scene_keys.get(&e.key) {
                act.cmd.emit(EditorCommand::Rename {
                    entity: scene.root,
                    name: name.to_string(),
                });
                return;
            }
            if let Some(r) = b.by_key.get(&e.key) {
                if let Some(who) = &r.locked_by {
                    refuse_locked(act.session, &[format!("{} ({who})", r.name)]);
                    return;
                }
                // Keep the extension unless the new name gives one: it picks the importer.
                let ext = file_name(&r.path)
                    .rsplit_once('.')
                    .map(|(_, e)| e.to_string());
                let new_name = match ext {
                    Some(x) if !name.contains('.') => format!("{name}.{x}"),
                    _ => name.to_string(),
                };
                act.cmd
                    .emit(rename_command(&r.path, &join(&r.folder, &new_name)));
            } else if let Some(f) = b.folders.get(&e.key).cloned() {
                let parent = parent_folder(&f);
                let dest = join(&parent, name);
                let mut cmds = Vec::new();
                let mut locked = Vec::new();
                for r in b
                    .rows
                    .iter()
                    .filter(|r| r.folder == f || r.folder.starts_with(&format!("{f}/")))
                {
                    match &r.locked_by {
                        Some(who) => locked.push(format!("{} ({who})", r.name)),
                        None => cmds.push(rename_command(
                            &r.path,
                            &format!("{dest}{}", &r.path[f.len()..]),
                        )),
                    }
                }
                refuse_locked(act.session, &locked);
                drop(b);
                let mut b = s.borrow_mut();
                if b.made.remove(&f) {
                    b.made.insert(dest);
                }
                act.cmd.emit_all(
                    &forge_ui::trf!("Rename folder {f} to {name}", f, name),
                    cmds,
                );
            }
        });
        let s = state.clone();
        pb.on(grid, move |act, e: &RowsDropped| {
            let b = s.borrow();
            let Some(dest) = e.target.parent.and_then(|k| b.folders.get(&k)).cloned() else {
                return;
            };
            let (cmds, locked) = b.move_commands(&e.keys, &dest);
            refuse_locked(act.session, &locked);
            act.cmd.emit_all(
                &forge_ui::trf!(
                    "Move {cmds_count} asset(s) to {dest}",
                    cmds_count = cmds.len(),
                    dest
                ),
                cmds,
            );
        });
        let s = state.clone();
        pb.on(grid, move |act, e: &RowsDeleteRequested| {
            let b = s.borrow();
            let cmds: Vec<EditorCommand> = e
                .keys
                .iter()
                .filter_map(|k| b.by_key.get(k))
                .filter(|r| !r.generated)
                .map(|r| remove_command(&r.path))
                .collect();
            let n = cmds.len();
            act.cmd
                .emit_all(&forge_ui::trf!("Remove {n} asset(s)", n), cmds);
            // Scene tiles delete their scene (refused, with who uses it, while in use).
            let scenes: Vec<EditorCommand> = e
                .keys
                .iter()
                .filter_map(|k| b.scene_keys.get(k))
                .map(|s| EditorCommand::Despawn { entity: s.root })
                .collect();
            let n = scenes.len();
            act.cmd
                .emit_all(&forge_ui::trf!("Delete {n} scene(s)", n), scenes);
        });
        let (s, c) = (state.clone(), catalog.clone());
        pb.on(grid, move |act, e: &FilesDropped| {
            let dest = {
                let b = s.borrow();
                e.folder
                    .and_then(|k| b.folders.get(&k).cloned())
                    .unwrap_or_else(|| b.cwd.clone())
            };
            let staged = c.borrow_mut().stage(&e.files, &dest);
            match staged {
                Ok(paths) => {
                    let (ok, skipped): (Vec<String>, Vec<String>) =
                        paths.into_iter().partition(|p| c.borrow().importable(p));
                    if !skipped.is_empty() {
                        act.session.problem(forge_editor::notify::Problem::coded(
                            forge_editor::notify::Severity::Warning,
                            "EDITOR-0014",
                            &forge_ui::trf!("{n} file(s) have no importer", n = skipped.len()),
                            &forge_ui::trf!(
                                "Copied but not imported: {files}",
                                files = skipped.join(", ")
                            ),
                        ));
                    }
                    let cmds: Vec<EditorCommand> = ok.iter().map(|p| import_command(p)).collect();
                    let n = cmds.len();
                    act.cmd
                        .emit_all(&forge_ui::trf!("Import {n} asset(s)", n), cmds);
                }
                Err(err) => {
                    act.session.problem(forge_editor::notify::Problem::coded(
                        forge_editor::notify::Severity::Error,
                        err.code(),
                        forge_ui::tr!("The files could not be copied into the project"),
                        &err.to_string(),
                    ));
                }
            }
        });
        pb.on_op(grid, "assets.rename", move |act| {
            act.ui.set_focus(Some(grid), true);
            act.ui.handle(InputEvent::Key(KeyEvent::press(
                KeyCode::F2,
                Modifiers::NONE,
            )));
        });
        let (s, c) = (state, catalog);
        pb.sync(grid, move |sx| {
            let mut b = s.borrow_mut();
            let scenes = b.sync_scenes(sx.mirror);
            b.refresh(sx.ui, &*c.borrow(), scenes);
            Ok(())
        });
        Ok(())
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn folder_keys_never_collide_with_asset_keys() {
        let f = folder_key("models");
        assert_ne!(f & FOLDER_BIT, 0);
        assert_eq!(folder_key("models"), f, "stable");
        assert_ne!(folder_key("models/a"), f);
        assert_eq!(join("", "a.png"), "a.png");
        assert_eq!(join("m", "a.png"), "m/a.png");
        assert_eq!(parent_folder("a/b/c"), "a/b");
        assert_eq!(parent_folder("a"), "");
    }
}
