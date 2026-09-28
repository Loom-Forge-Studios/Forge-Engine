//! The `Ui`: the retained widget arena and the frame pipeline of Ch.21 §21.3.
//!
//! ```text
//!  1 drain queue → 2 route events → 3 run actions → 4 flush signals (batched)
//!  5 layout dirty subtrees (taffy) → 6 re-record dirty display-list slices
//!  7 a11y TreeUpdate for changed nodes → 8 if damage ≠ ∅: batch + render
//! ```
//!
//! Steps 2–3 happen as input arrives ([`Ui::handle`]); 1 and 4–7 in [`Ui::frame`]; 8 in
//! [`Ui::render`], which does nothing at all when there is no damage.

use std::any::{Any, TypeId};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::Duration;

use slotmap::SlotMap;
use taffy::TaffyTree;

use crate::UiError;
use crate::a11y::A11yState;
use crate::clipboard::{Clipboard, InProcessClipboard};
use crate::damage::{
    Dirty, FEED_TIMER_BASE, FeedFaults, FeedId, LiveFeed, LiveFeeds, Scheduler, UiTime, UiWaker,
    Wake,
};
use crate::dnd::{DragPayload, DragSession, DropVerdict};
use crate::focus::{self, Direction, Focusable};
use crate::geom::{Color, Corners, Point, PxRect, Rect, Size};
use crate::id::{Fnv, Key, WidgetId};
use crate::ime::{ImeRequest, ImeState};
use crate::input::{Handled, InputEvent, KeyCode, UiEvent};
use crate::layout::{FocusScope, NodeStyle};
use crate::render::batch::{self, BatchFaults};
use crate::render::cache::{BatchCache, SliceBatch, Step, batch_slice};
use crate::render::{
    Border, FrameStats, ImageAtlas, MeshId, MeshStore, Primitive, TargetId, UiRenderer,
    merge_damage,
};
use crate::state::Runtime;
use crate::style::{ColorPair, ColorRole, PairKind, Theme, WidgetState};
use crate::text::{AtlasConfig, FontConfig, GlyphRunId, TextSystem};
use crate::widget::{Binder, EventCx, MeasureCx, PaintCx, Widget};
use std::cell::{Ref, RefCell, RefMut};
use std::rc::Rc;

slotmap::new_key_type! {
    /// A generational handle into the widget arena: a stale handle is detected instead of
    /// aliasing a new widget.
    pub struct NodeKey;
}

pub(crate) struct Node {
    pub(crate) id: WidgetId,
    pub(crate) key: Key,
    pub(crate) parent: Option<NodeKey>,
    pub(crate) children: Vec<NodeKey>,
    pub(crate) widget: Option<Box<dyn Widget>>,
    pub(crate) style: NodeStyle,
    pub(crate) hidden: bool,
    pub(crate) taffy: taffy::NodeId,
    pub(crate) rect: Rect,
    pub(crate) slice: Vec<Primitive>,
    pub(crate) slice_bounds: Rect,
    pub(crate) runs: Vec<GlyphRunId>,
    /// Meshes this slice draws (reference-counted in the shared `MeshStore`).
    pub(crate) meshes: Vec<MeshId>,
    /// The slice, batched (`None`: re-recorded since the last render; batch it).
    pub(crate) batch: Option<SliceBatch>,
    /// Scroll state for a scroll viewport (`NodeStyle::scrollable`).
    pub(crate) scroll: Option<ScrollState>,
    pub(crate) dirty: Dirty,
    pub(crate) state: WidgetState,
}

/// A scroll viewport's offset and extents (logical px).
#[derive(Copy, Clone, Debug, Default, PartialEq)]
pub struct ScrollState {
    /// How far the content is scrolled (content px hidden above / left of the viewport).
    pub offset: Point,
    /// The content's size.
    pub content: Size,
    /// The viewport's size.
    pub viewport: Size,
}

impl ScrollState {
    /// The largest offset that still shows content.
    pub fn max_offset(&self) -> Point {
        Point::new(
            (self.content.w - self.viewport.w).max(0.0),
            (self.content.h - self.viewport.h).max(0.0),
        )
    }
}

/// Two presses of one button within this time and 4 px are a double click.
pub const DOUBLE_CLICK: Duration = Duration::from_millis(500);

/// Space between a widget and its focus ring (logical px).
pub const FOCUS_RING_GAP: f32 = 2.0;

/// The window's root widget: fills the window with `bg.base`; its a11y name is the
/// window title.
struct Root(String);
impl Widget for Root {
    fn role(&self) -> accesskit::Role {
        accesskit::Role::Window
    }
    fn paint(&self, _cx: &mut PaintCx) {}
    fn a11y(&self, _cx: &crate::widget::A11yCx, node: &mut accesskit::Node) {
        node.set_label(self.0.as_str());
    }
}

/// Process-wide UI resources shared by every window (§21.14): the shaping cache and the
/// one glyph atlas (in `text`), and the image atlas. The renderer holds one copy of their
/// textures, so windows on one device never duplicate an atlas.
pub struct Resources {
    pub text: TextSystem,
    pub images: ImageAtlas,
    /// Tessellated meshes (curve, gradient and graph editors), shared like glyph runs.
    pub meshes: MeshStore,
}

pub type SharedResources = Rc<RefCell<Resources>>;

/// Construction parameters.
#[derive(Clone)]
pub struct UiConfig {
    pub fonts: FontConfig,
    pub atlas: AtlasConfig,
    pub theme: Theme,
    /// Window size in logical pixels.
    pub size: Size,
    /// Window scale factor × user UI scale.
    pub scale: f32,
}

impl Default for UiConfig {
    fn default() -> Self {
        Self {
            fonts: FontConfig::default(),
            atlas: AtlasConfig::default(),
            theme: Theme::dark(),
            size: Size::new(1280.0, 800.0),
            scale: 1.0,
        }
    }
}

/// Action: open another OS window (a floating dock window, a detached viewport). The
/// runner handles it and calls `UiApp::build_window(ui, key)` with a `Ui` that shares this
/// window's resources (§21.14).
#[derive(Clone, Debug, PartialEq)]
pub struct OpenWindow {
    pub key: u64,
    pub title: String,
    pub size: Size,
    /// Where the window opens on the desktop (logical px of the virtual screen); `None`
    /// lets the OS choose. A floating dock window reopens where it was.
    pub position: Option<crate::geom::Point>,
    /// The monitor it belongs on (a hint; see `dock::geometry::place_window`).
    pub monitor: Option<String>,
}

/// Action: close the window this widget is in (ignored for the main window).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct CloseWindow;

/// A raised action on its way to a handler.
pub struct ActionEnvelope {
    pub source: WidgetId,
    pub action: Box<dyn Any>,
}

impl std::fmt::Debug for ActionEnvelope {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "ActionEnvelope({:?})", self.source)
    }
}

impl ActionEnvelope {
    pub fn get<A: Any>(&self) -> Option<&A> {
        self.action.downcast_ref::<A>()
    }
}

/// What an action handler may touch: the signal runtime (panel state), nothing else.
pub struct ActionCx<'a> {
    pub rt: &'a mut Runtime,
    pub source: WidgetId,
}

type Handler = Box<dyn FnMut(&mut ActionCx, &dyn Any) -> bool>;

/// A closure posted from another thread, run on the UI thread at step 1.
pub type Posted = Box<dyn FnOnce(&mut Runtime) + Send>;

/// A `Send` handle for other threads (bus client, thumbnails, viewport renderer) to post
/// work into the UI thread's queue and wake its loop.
#[derive(Clone)]
pub struct Poster {
    tx: Sender<Posted>,
    waker: Option<Arc<dyn UiWaker>>,
}

impl Poster {
    pub fn post(&self, f: impl FnOnce(&mut Runtime) + Send + 'static) {
        if self.tx.send(Box::new(f)).is_ok()
            && let Some(w) = &self.waker
        {
            w.wake();
        }
    }
}

/// Counters the budget tests read.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct UiStats {
    pub frames_rendered: u64,
    pub slices_recorded: u64,
    /// Slices re-recorded by the last `frame()`.
    pub last_slices: u64,
    /// Widgets whose slice the last `frame()` re-recorded.
    pub last_slice_ids: Vec<WidgetId>,
    pub layout_passes: u64,
    pub last_frame: FrameStats,
    /// Damage of the last `frame()` (logical px).
    pub last_damage: Vec<Rect>,
    /// Slices the last `render()` turned into GPU instances (the rest reused their cache).
    pub last_slices_batched: u64,
    /// Whether the last `render()` had to re-walk the paint order (structure change,
    /// growth beyond a slot, atlas epoch) instead of patching in place.
    pub last_reassembled: bool,
    pub reassemblies: u64,
}

forge_trace::control_switches! {
    /// Fault switches for positive controls (W2). All off in production.
    #[derive(Clone, Copy, Debug, Default)]
    pub struct UiFaults {
        /// Hover marks the root dirty (the `ui_damage_bounded` control).
        pub hover_marks_root: bool,
        pub batch: BatchFaults,
        pub feeds: FeedFaults,
        /// Re-batch and re-upload every slice every frame (the pre-cache behaviour; the
        /// `ui_interaction_rebatches_o1` control).
        pub rebatch_everything: bool,
    }
}

/// What `frame()` produced.
#[derive(Clone, Debug, PartialEq)]
pub struct FrameReport {
    /// Logical damage; empty ⇒ `render` draws nothing.
    pub damage: Vec<Rect>,
    /// What the loop should do next (never `Poll`).
    pub wake: Wake,
}

/// One window's retained UI.
pub struct Ui {
    pub(crate) nodes: SlotMap<NodeKey, Node>,
    pub(crate) index: HashMap<WidgetId, NodeKey>,
    pub(crate) root: NodeKey,
    taffy: TaffyTree<WidgetId>,
    pub(crate) rt: Runtime,
    pub(crate) res: SharedResources,
    pub(crate) theme: Theme,
    pub(crate) scheduler: Scheduler,
    feeds: LiveFeeds,
    waker: Option<Arc<dyn UiWaker>>,
    pub(crate) focused: Option<WidgetId>,
    scope_memory: HashMap<WidgetId, WidgetId>,
    pub(crate) hover: Option<WidgetId>,
    pub(crate) capture: Option<WidgetId>,
    press_target: Option<WidgetId>,
    pressed: Option<WidgetId>,
    pub(crate) ime: ImeState,
    pub(crate) clipboard: Box<dyn Clipboard>,
    pub(crate) a11y: A11yState,
    actions: Vec<ActionEnvelope>,
    handlers: HashMap<(WidgetId, TypeId), Handler>,
    pub(crate) now: UiTime,
    size: Size,
    pub(crate) scale: f32,
    dirty_keys: Vec<NodeKey>,
    pub(crate) layout_dirty: bool,
    damage: Vec<Rect>,
    pub(crate) reduced_motion: bool,
    /// Settings: whether focused text carets blink at all (§21.3, M2-43).
    pub(crate) caret_blink: bool,
    /// Which OS window this is: `None` for the main window, the `OpenWindow` key for the
    /// others (an application routes a window's actions by it).
    window_key: Option<u64>,
    pub(crate) drag: Option<DragSession>,
    window_visible: bool,
    used_pairs: BTreeSet<ColorPair>,
    stats: UiStats,
    pub faults: UiFaults,
    tx: Sender<Posted>,
    rx: Receiver<Posted>,
    /// Unhandled arrow keys move focus spatially (games / gamepads, §21.20).
    pub spatial_arrows: bool,
    /// The game-menu convention (§21.20): Up / Down always move focus spatially between
    /// rows (a focused slider or segmented control does not swallow them); Left / Right go
    /// to the focused control first (adjusting a slider) and move focus when unhandled
    /// (with `spatial_arrows`). Off in the editor; inert while a popup is open.
    pub game_nav: bool,
    /// The retained batch cache (render step 8).
    cache: BatchCache<NodeKey>,
    /// Slices re-recorded since the last render (they need batching).
    repainted: Vec<NodeKey>,
    /// The other half of the repaint double buffer (empty, capacity retained).
    repainted_spare: Vec<NodeKey>,
    /// Reused per render: the re-batched slices and their node clips.
    batch_changed: Vec<(NodeKey, Option<Rect>)>,
    /// The last pointer position in this window (logical px), if the pointer is inside.
    pointer: Option<Point>,
    /// Where this window's client area is on the desktop (logical px of the virtual
    /// screen), and the monitor it is on: set by the platform runner (§21.14), read by
    /// docking to tear panels off to a screen position.
    window_origin: Point,
    monitor: Option<String>,
    /// Receives the keys the focus chain did not handle (see [`Ui::set_key_sink`]).
    key_sink: Option<WidgetId>,
    /// Reused per frame: the new slice's glyph runs.
    scratch_runs: Vec<GlyphRunId>,
    /// Keyboard modifiers held now (pointer events read them).
    pub(crate) mods: crate::input::Modifiers,
    /// The last press: time, position, button, click count (double-click detection).
    last_click: Option<(UiTime, Point, crate::input::PointerButton, u8)>,
    /// The overlay layer (popups, tooltips, toasts).
    pub(crate) overlay: Option<NodeKey>,
    pub(crate) popups: Vec<crate::overlay::Popup>,
    pub(crate) pending_close: Vec<WidgetId>,
    pub(crate) pending_popup_focus: Option<WidgetId>,
    pub(crate) tooltip: Option<(WidgetId, WidgetId)>,
    pub(crate) tooltip_armed: Option<WidgetId>,
    pub(crate) toast_seq: u64,
    /// A scroll offset changed: re-place absolute rects without a taffy pass.
    rects_dirty: bool,
}

impl Ui {
    pub fn new(cfg: UiConfig) -> Result<Ui, UiError> {
        let res = Rc::new(RefCell::new(Resources {
            text: TextSystem::with_atlas(&cfg.fonts, cfg.atlas),
            images: ImageAtlas::default(),
            meshes: MeshStore::default(),
        }));
        Self::with_resources(cfg, res)
    }

    /// A window that shares `res` (one glyph atlas, one image atlas, one shaping cache)
    /// with other windows of the process (§21.14).
    pub fn with_resources(cfg: UiConfig, res: SharedResources) -> Result<Ui, UiError> {
        let mut taffy: TaffyTree<WidgetId> = TaffyTree::new();
        res.borrow_mut().text.set_scale(cfg.scale);
        let root_style = NodeStyle::default().background(ColorRole::BgBase);
        let mut ls = root_style.layout.clone();
        ls.size = taffy::Size {
            width: taffy::prelude::length(cfg.size.w),
            height: taffy::prelude::length(cfg.size.h),
        };
        let root_t = taffy
            .new_leaf(ls)
            .map_err(|e| UiError::Layout(e.to_string()))?;
        let mut nodes = SlotMap::with_key();
        let root = nodes.insert(Node {
            id: WidgetId::ROOT,
            key: Key::Static("root"),
            parent: None,
            children: Vec::new(),
            widget: Some(Box::new(Root("Forge".into()))),
            style: root_style,
            hidden: false,
            taffy: root_t,
            rect: Rect::ZERO,
            slice: Vec::new(),
            slice_bounds: Rect::ZERO,
            runs: Vec::new(),
            meshes: Vec::new(),
            batch: None,
            scroll: None,
            dirty: Dirty::ALL,
            state: WidgetState::default(),
        });
        let mut index = HashMap::new();
        index.insert(WidgetId::ROOT, root);
        let (tx, rx) = channel();
        Ok(Ui {
            nodes,
            index,
            root,
            taffy,
            rt: Runtime::new(),
            res,
            theme: cfg.theme,
            scheduler: Scheduler::new(),
            feeds: LiveFeeds::default(),
            waker: None,
            focused: None,
            scope_memory: HashMap::new(),
            hover: None,
            capture: None,
            press_target: None,
            pressed: None,
            ime: ImeState::default(),
            clipboard: Box::new(InProcessClipboard::default()),
            a11y: A11yState::default(),
            actions: Vec::new(),
            handlers: HashMap::new(),
            now: Duration::ZERO,
            size: cfg.size,
            scale: cfg.scale,
            dirty_keys: vec![root],
            layout_dirty: true,
            damage: Vec::new(),
            reduced_motion: false,
            caret_blink: true,
            window_key: None,
            drag: None,
            window_visible: true,
            used_pairs: BTreeSet::new(),
            stats: UiStats::default(),
            faults: UiFaults::default(),
            tx,
            rx,
            spatial_arrows: false,
            game_nav: false,
            cache: BatchCache::default(),
            repainted: Vec::new(),
            repainted_spare: Vec::new(),
            batch_changed: Vec::new(),
            pointer: None,
            window_origin: Point::ZERO,
            monitor: None,
            key_sink: None,
            scratch_runs: Vec::new(),
            mods: crate::input::Modifiers::NONE,
            last_click: None,
            overlay: None,
            popups: Vec::new(),
            pending_close: Vec::new(),
            pending_popup_focus: None,
            tooltip: None,
            tooltip_armed: None,
            toast_seq: 0,
            rects_dirty: false,
        })
    }

    // ---- accessors ------------------------------------------------------------------

    pub fn root(&self) -> WidgetId {
        WidgetId::ROOT
    }
    pub fn rt(&self) -> &Runtime {
        &self.rt
    }
    pub fn rt_mut(&mut self) -> &mut Runtime {
        &mut self.rt
    }
    pub fn theme(&self) -> &Theme {
        &self.theme
    }
    pub fn text(&self) -> Ref<'_, TextSystem> {
        Ref::map(self.res.borrow(), |r| &r.text)
    }
    pub fn text_mut(&mut self) -> RefMut<'_, TextSystem> {
        RefMut::map(self.res.borrow_mut(), |r| &mut r.text)
    }
    /// The resources this window shares with the process's other windows.
    pub fn resources(&self) -> SharedResources {
        self.res.clone()
    }
    /// Add an RGBA8 image (thumbnails, bitmaps) to the shared image atlas.
    pub fn add_image(
        &mut self,
        w: u32,
        h: u32,
        rgba: Vec<u8>,
    ) -> Result<crate::render::ImageId, UiError> {
        self.res.borrow_mut().images.add(w, h, rgba)
    }
    /// Free an image's atlas slot (an evicted thumbnail).
    pub fn remove_image(&mut self, id: crate::render::ImageId) -> bool {
        self.res.borrow_mut().images.remove(id)
    }
    /// Images resident in the shared atlas.
    pub fn image_count(&self) -> usize {
        self.res.borrow().images.len()
    }
    pub fn stats(&self) -> &UiStats {
        &self.stats
    }
    pub fn size(&self) -> Size {
        self.size
    }
    pub fn scale(&self) -> f32 {
        self.scale
    }
    pub fn focused(&self) -> Option<WidgetId> {
        self.focused
    }
    pub fn now(&self) -> UiTime {
        self.now
    }
    pub fn scheduler(&self) -> &Scheduler {
        &self.scheduler
    }
    pub fn contains(&self, id: WidgetId) -> bool {
        self.index.contains_key(&id)
    }
    /// Every `(fg, bg, kind)` colour pair painted so far (the contrast guard reads it).
    pub fn used_pairs(&self) -> &BTreeSet<ColorPair> {
        &self.used_pairs
    }
    pub fn set_clipboard(&mut self, c: Box<dyn Clipboard>) {
        self.clipboard = c;
    }
    pub fn clipboard(&mut self) -> &mut dyn Clipboard {
        self.clipboard.as_mut()
    }

    /// The layout rect (logical, window coordinates) of a widget.
    pub fn rect(&self, id: WidgetId) -> Option<Rect> {
        self.index.get(&id).map(|k| self.nodes[*k].rect)
    }
    pub(crate) fn node_rect(&self, k: NodeKey) -> Rect {
        self.nodes.get(k).map(|n| n.rect).unwrap_or_default()
    }
    pub(crate) fn node_state(&self, k: NodeKey) -> WidgetState {
        self.nodes.get(k).map(|n| n.state).unwrap_or_default()
    }

    /// Typed read access to a widget.
    pub fn widget<W: Widget>(&self, id: WidgetId) -> Option<&W> {
        let k = self.index.get(&id)?;
        self.nodes[*k].widget.as_ref()?.as_any().downcast_ref::<W>()
    }

    /// Typed mutable access; the widget is marked `LAYOUT` dirty (conservatively).
    pub fn widget_mut<W: Widget>(&mut self, id: WidgetId) -> Option<&mut W> {
        let k = *self.index.get(&id)?;
        self.mark(k, Dirty::LAYOUT);
        self.nodes[k]
            .widget
            .as_mut()?
            .as_any_mut()
            .downcast_mut::<W>()
    }

    /// The key a widget was added under.
    pub fn key_of(&self, id: WidgetId) -> Option<&Key> {
        self.index.get(&id).map(|k| &self.nodes[*k].key)
    }

    /// Children ids of a widget, in order.
    pub fn children(&self, id: WidgetId) -> Vec<WidgetId> {
        self.index
            .get(&id)
            .map(|k| {
                self.nodes[*k]
                    .children
                    .iter()
                    .map(|c| self.nodes[*c].id)
                    .collect()
            })
            .unwrap_or_default()
    }

    /// A `Send` handle for posting work from other threads.
    pub fn poster(&self) -> Poster {
        Poster {
            tx: self.tx.clone(),
            waker: self.waker.clone(),
        }
    }

    /// Install the loop waker (the runner's `EventLoopProxy`, or a test counter).
    pub fn set_waker(&mut self, w: Arc<dyn UiWaker>) {
        self.waker = Some(w);
    }

    /// The window title, announced as the root node's name.
    pub fn set_title(&mut self, title: &str) {
        if let Some(r) = self.nodes[self.root]
            .widget
            .as_mut()
            .and_then(|w| w.as_any_mut().downcast_mut::<Root>())
        {
            r.0 = title.to_string();
        }
        self.mark(self.root, Dirty::A11Y);
    }

    pub fn set_reduced_motion(&mut self, on: bool) {
        if self.reduced_motion != on {
            self.reduced_motion = on;
            // A settings change: animations re-decide on their next frame and static
            // variants (a striped busy bar) repaint now.
            self.mark_all(Dirty::PAINT);
        }
    }

    /// Settings: turn the caret blink on or off for every text field in this window. Off
    /// means a solid caret and no blink timer at all (§21.3, M2-43).
    pub fn set_caret_blink(&mut self, on: bool) {
        if self.caret_blink != on {
            self.caret_blink = on;
            self.mark_all(Dirty::PAINT);
        }
    }

    pub fn caret_blink(&self) -> bool {
        self.caret_blink
    }

    /// Mark this `Ui` as the window opened with `OpenWindow { key }` (`None`: the main
    /// window). The runner sets it.
    pub fn set_window_key(&mut self, key: Option<u64>) {
        self.window_key = key;
    }

    pub fn window_key(&self) -> Option<u64> {
        self.window_key
    }

    pub fn reduced_motion(&self) -> bool {
        self.reduced_motion
    }

    /// Switch theme: every slice re-records (a theme switch is a full repaint).
    pub fn set_theme(&mut self, theme: Theme) {
        self.theme = theme;
        self.mark_all(Dirty::LAYOUT);
    }

    /// The window is minimised or reported `Occluded`: nothing is visible (§21.11 rule 2).
    pub fn set_window_visible(&mut self, visible: bool) {
        if self.window_visible != visible {
            self.window_visible = visible;
            let keys: Vec<NodeKey> = self.nodes.keys().collect();
            for k in keys {
                if self.is_visible_key(k) || !visible {
                    self.dispatch(k, &UiEvent::VisibilityChanged(visible));
                }
            }
        }
    }

    /// Resize and/or rescale. Layout is logical, so a scale change only re-shapes text and
    /// repaints (§21.14).
    pub fn set_viewport(&mut self, size: Size, scale: f32) {
        let scale_changed = (scale - self.scale).abs() > f32::EPSILON;
        if size != self.size {
            self.size = size;
            let root_t = self.nodes[self.root].taffy;
            if let Ok(mut s) = self.taffy.style(root_t).cloned() {
                s.size = taffy::Size {
                    width: taffy::prelude::length(size.w),
                    height: taffy::prelude::length(size.h),
                };
                let _ = self.taffy.set_style(root_t, s);
            }
            self.mark(self.root, Dirty::LAYOUT);
            self.damage.push(Rect::new(0.0, 0.0, size.w, size.h));
        }
        if scale_changed {
            self.scale = scale;
            self.res.borrow_mut().text.set_scale(scale);
            self.mark_all(Dirty::LAYOUT);
        }
    }

    fn mark_all(&mut self, d: Dirty) {
        let keys: Vec<NodeKey> = self.nodes.keys().collect();
        for k in keys {
            self.mark(k, d);
        }
        self.damage
            .push(Rect::new(0.0, 0.0, self.size.w, self.size.h));
    }

    // ---- building the tree ----------------------------------------------------------

    /// Add a widget under `parent`. Its id is `hash64(parent, key)`.
    pub fn add(
        &mut self,
        parent: WidgetId,
        key: impl Into<Key>,
        style: NodeStyle,
        widget: impl Widget,
    ) -> Result<WidgetId, UiError> {
        self.add_boxed(parent, key.into(), style, Box::new(widget))
    }

    pub fn add_boxed(
        &mut self,
        parent: WidgetId,
        key: Key,
        style: NodeStyle,
        widget: Box<dyn Widget>,
    ) -> Result<WidgetId, UiError> {
        let id = self.add_unordered(parent, key, style, widget)?;
        // The overlay layer stays the root's last child: it paints on top and hit-tests first.
        if parent == WidgetId::ROOT
            && let Some(ok) = self.overlay
            && self.nodes.contains_key(ok)
            && self.nodes[self.root].children.last() != Some(&ok)
        {
            self.nodes[self.root].children.retain(|c| *c != ok);
            self.nodes[self.root].children.push(ok);
            let t: Vec<taffy::NodeId> = self.nodes[self.root]
                .children
                .iter()
                .map(|c| self.nodes[*c].taffy)
                .collect();
            let rt = self.nodes[self.root].taffy;
            let _ = self.taffy.set_children(rt, &t);
        }
        Ok(id)
    }

    pub(crate) fn add_unordered(
        &mut self,
        parent: WidgetId,
        key: Key,
        style: NodeStyle,
        widget: Box<dyn Widget>,
    ) -> Result<WidgetId, UiError> {
        let pk = *self
            .index
            .get(&parent)
            .ok_or(UiError::UnknownWidget(parent))?;
        let id = parent.child(&key);
        if self.index.contains_key(&id) {
            return Err(UiError::DuplicateKey(id));
        }
        let t = if widget.measured() {
            self.taffy.new_leaf_with_context(style.layout.clone(), id)
        } else {
            self.taffy.new_leaf(style.layout.clone())
        }
        .map_err(|e| UiError::Layout(e.to_string()))?;
        let pt = self.nodes[pk].taffy;
        self.taffy
            .add_child(pt, t)
            .map_err(|e| UiError::Layout(e.to_string()))?;
        let mut b = Binder { subs: Vec::new() };
        widget.bind(&mut b);
        for (s, d) in b.subs {
            self.rt.subscribe(s, id, d);
        }
        let scroll = style.scroll.then(ScrollState::default);
        let k = self.nodes.insert(Node {
            id,
            key,
            parent: Some(pk),
            children: Vec::new(),
            widget: Some(widget),
            style,
            hidden: false,
            taffy: t,
            rect: Rect::ZERO,
            slice: Vec::new(),
            slice_bounds: Rect::ZERO,
            runs: Vec::new(),
            meshes: Vec::new(),
            batch: None,
            scroll,
            dirty: Dirty::NONE,
            state: WidgetState::default(),
        });
        self.nodes[pk].children.push(k);
        self.index.insert(id, k);
        self.cache.structure_dirty = true;
        self.mark(k, Dirty::LAYOUT);
        self.mark(pk, Dirty::A11Y);
        if self.is_visible_key(k) {
            self.dispatch(k, &UiEvent::VisibilityChanged(true));
        }
        Ok(id)
    }

    /// Remove a widget and its subtree.
    pub fn remove(&mut self, id: WidgetId) -> Result<(), UiError> {
        let k = *self.index.get(&id).ok_or(UiError::UnknownWidget(id))?;
        if k == self.root {
            return Err(UiError::UnknownWidget(id));
        }
        let parent = self.nodes[k].parent;
        let mut stack = vec![k];
        let mut doomed = Vec::new();
        while let Some(n) = stack.pop() {
            doomed.push(n);
            stack.extend(self.nodes[n].children.iter().copied());
        }
        for n in doomed {
            let Some(node) = self.nodes.remove(n) else {
                continue;
            };
            self.damage.push(node.slice_bounds);
            for r in &node.runs {
                self.res.borrow_mut().text.release(*r);
            }
            for m in &node.meshes {
                self.res.borrow_mut().meshes.release(*m);
            }
            self.rt.unsubscribe_widget(node.id);
            self.scheduler.forget_widget(node.id);
            self.feeds.remove_owner(node.id);
            let _ = self.taffy.remove(node.taffy);
            self.index.remove(&node.id);
            self.a11y.forget(node.id);
            self.popups.retain(|p| p.root != node.id && p.id != node.id);
            if self
                .tooltip
                .is_some_and(|(t, b)| t == node.id || b == node.id)
            {
                self.tooltip = None;
            }
            if self.overlay == Some(n) {
                self.overlay = None;
            }
            if self.focused == Some(node.id) {
                self.focused = None;
                self.ime.allowed = false;
                self.a11y.focus_changed = true;
            }
            for w in [
                &mut self.hover,
                &mut self.capture,
                &mut self.press_target,
                &mut self.pressed,
            ] {
                if *w == Some(node.id) {
                    *w = None;
                }
            }
        }
        self.cache.structure_dirty = true;
        if let Some(p) = parent {
            self.nodes[p].children.retain(|c| *c != k);
            self.mark(p, Dirty::LAYOUT);
        }
        Ok(())
    }

    /// Reorder `parent`'s children to the given keys: keyed widgets **move**, so focus,
    /// scroll offsets and animations survive (§21.4).
    pub fn reorder(&mut self, parent: WidgetId, keys: &[Key]) -> Result<(), UiError> {
        let pk = *self
            .index
            .get(&parent)
            .ok_or(UiError::UnknownWidget(parent))?;
        let mut order = Vec::with_capacity(keys.len());
        for key in keys {
            let id = parent.child(key);
            let k = *self.index.get(&id).ok_or(UiError::UnknownWidget(id))?;
            order.push(k);
        }
        if order.len() != self.nodes[pk].children.len() {
            return Err(UiError::Layout(format!(
                "reorder of {parent:?}: {} keys for {} children",
                order.len(),
                self.nodes[pk].children.len()
            )));
        }
        let t: Vec<taffy::NodeId> = order.iter().map(|k| self.nodes[*k].taffy).collect();
        let pt = self.nodes[pk].taffy;
        self.taffy
            .set_children(pt, &t)
            .map_err(|e| UiError::Layout(e.to_string()))?;
        self.nodes[pk].children = order;
        self.cache.structure_dirty = true;
        self.mark(pk, Dirty::LAYOUT);
        Ok(())
    }

    /// Move a widget to the end of its parent's children, so it paints above (and
    /// hit-tests before) its siblings — a drop preview over docked panels. The overlay
    /// layer stays the root's last child.
    pub fn bring_to_front(&mut self, id: WidgetId) -> Result<(), UiError> {
        let k = *self.index.get(&id).ok_or(UiError::UnknownWidget(id))?;
        let Some(pk) = self.nodes[k].parent else {
            return Ok(());
        };
        let mut children = self.nodes[pk].children.clone();
        let keep_last = (pk == self.root)
            .then_some(self.overlay)
            .flatten()
            .filter(|o| *o != k && children.last() == Some(o));
        let want_last = keep_last.map_or(Some(&k), |_| children.iter().rev().nth(1));
        if want_last == Some(&k) {
            return Ok(());
        }
        children.retain(|c| *c != k);
        match keep_last {
            Some(o) => {
                children.retain(|c| *c != o);
                children.push(k);
                children.push(o);
            }
            None => children.push(k),
        }
        let t: Vec<taffy::NodeId> = children.iter().map(|c| self.nodes[*c].taffy).collect();
        let pt = self.nodes[pk].taffy;
        self.taffy
            .set_children(pt, &t)
            .map_err(|e| UiError::Layout(e.to_string()))?;
        self.nodes[pk].children = children;
        self.cache.structure_dirty = true;
        self.mark(pk, Dirty::LAYOUT);
        Ok(())
    }

    /// Replace a widget's node style.
    pub fn set_style(&mut self, id: WidgetId, style: NodeStyle) -> Result<(), UiError> {
        let k = *self.index.get(&id).ok_or(UiError::UnknownWidget(id))?;
        let mut ls = style.layout.clone();
        if self.nodes[k].hidden {
            ls.display = taffy::Display::None;
        }
        let t = self.nodes[k].taffy;
        self.taffy
            .set_style(t, ls)
            .map_err(|e| UiError::Layout(e.to_string()))?;
        self.nodes[k].style = style;
        self.mark(k, Dirty::LAYOUT);
        self.cache.structure_dirty = true;
        Ok(())
    }

    /// Show or hide a subtree (tabs, collapsed sections). Hidden widgets take no space,
    /// draw nothing, are skipped by focus and a11y, and receive `VisibilityChanged`.
    pub fn set_hidden(&mut self, id: WidgetId, hidden: bool) -> Result<(), UiError> {
        let k = *self.index.get(&id).ok_or(UiError::UnknownWidget(id))?;
        if self.nodes[k].hidden == hidden {
            return Ok(());
        }
        let sub = self.subtree(k);
        let before: Vec<bool> = sub.iter().map(|n| self.is_visible_key(*n)).collect();
        if hidden {
            for n in &sub {
                self.damage.push(self.nodes[*n].slice_bounds);
            }
        }
        self.nodes[k].hidden = hidden;
        self.cache.structure_dirty = true;
        let mut ls = self.nodes[k].style.layout.clone();
        if hidden {
            ls.display = taffy::Display::None;
        }
        let t = self.nodes[k].taffy;
        self.taffy
            .set_style(t, ls)
            .map_err(|e| UiError::Layout(e.to_string()))?;
        if let Some(p) = self.nodes[k].parent {
            self.mark(p, Dirty::LAYOUT);
        }
        let mut changed = false;
        for (i, n) in sub.iter().enumerate() {
            let after = self.is_visible_key(*n);
            if after != before[i] {
                changed = true;
                if after {
                    self.mark(*n, Dirty::LAYOUT);
                }
                self.dispatch(*n, &UiEvent::VisibilityChanged(after));
            }
        }
        // Live feeds follow visibility at once (§21.11): a feed hidden here drops its waker
        // now, and one shown here compares generations and refreshes now — not at the next
        // frame's start, which an idle UI may never reach.
        if changed
            && !self.feeds.feeds.is_empty()
            && !self.faults.feeds().visibility_on_next_frame()
        {
            self.sync_feeds();
        }
        if hidden
            && let Some(f) = self.focused
            && let Some(fk) = self.index.get(&f)
            && sub.contains(fk)
        {
            self.set_focus(None, false);
        }
        Ok(())
    }

    pub fn is_hidden(&self, id: WidgetId) -> bool {
        self.index.get(&id).is_some_and(|k| self.nodes[*k].hidden)
    }

    /// Whether a widget and all its ancestors are shown, in a visible window.
    pub fn is_visible(&self, id: WidgetId) -> bool {
        self.index.get(&id).is_some_and(|k| self.is_visible_key(*k))
    }

    pub(crate) fn is_visible_key(&self, mut k: NodeKey) -> bool {
        if !self.window_visible {
            return false;
        }
        loop {
            let Some(n) = self.nodes.get(k) else {
                return false;
            };
            if n.hidden {
                return false;
            }
            match n.parent {
                Some(p) => k = p,
                None => return true,
            }
        }
    }

    fn subtree(&self, k: NodeKey) -> Vec<NodeKey> {
        let mut out = Vec::new();
        let mut stack = vec![k];
        while let Some(n) = stack.pop() {
            out.push(n);
            stack.extend(self.nodes[n].children.iter().rev().copied());
        }
        out
    }

    /// Register an action handler on `id`: actions raised by it or its descendants of
    /// type `A` reach `f`, which returns `true` to stop bubbling.
    pub fn on_action<A: Any>(
        &mut self,
        id: WidgetId,
        mut f: impl FnMut(&mut ActionCx, &A) -> bool + 'static,
    ) {
        self.handlers.insert(
            (id, TypeId::of::<A>()),
            Box::new(move |cx, a| a.downcast_ref::<A>().is_some_and(|a| f(cx, a))),
        );
    }

    /// Raise an action on behalf of `source` from application code (e.g. an
    /// [`OpenWindow`] at startup). It bubbles like a widget's action.
    pub fn raise(&mut self, source: WidgetId, action: impl Any) {
        self.raise_action(source, Box::new(action));
    }

    /// Actions no handler consumed, for the application.
    pub fn take_actions(&mut self) -> Vec<ActionEnvelope> {
        std::mem::take(&mut self.actions)
    }

    /// Register a live-data feed owned by `owner` (§21.11).
    pub fn add_feed(&mut self, owner: WidgetId, feed: LiveFeed) -> FeedId {
        let id = self.feeds.add(owner, feed);
        self.sync_feeds();
        id
    }

    // ---- dirty marking --------------------------------------------------------------

    pub(crate) fn mark(&mut self, k: NodeKey, d: Dirty) {
        let Some(n) = self.nodes.get_mut(k) else {
            return;
        };
        let d = d.implied();
        if n.dirty.is_empty() {
            self.dirty_keys.push(k);
        }
        n.dirty |= d;
        if d.contains(Dirty::LAYOUT) {
            let _ = self.taffy.mark_dirty(n.taffy);
            self.layout_dirty = true;
        }
    }

    /// Mark a widget by id (panels invalidating custom-painted content).
    pub fn invalidate(&mut self, id: WidgetId, d: Dirty) {
        if let Some(k) = self.index.get(&id).copied() {
            self.mark(k, d);
        }
    }

    // ---- actions --------------------------------------------------------------------

    pub(crate) fn raise_action(&mut self, source: WidgetId, action: Box<dyn Any>) {
        let tid = (*action).type_id();
        let mut cur = self.index.get(&source).copied();
        while let Some(k) = cur {
            let id = self.nodes[k].id;
            if let Some(mut h) = self.handlers.remove(&(id, tid)) {
                let mut cx = ActionCx {
                    rt: &mut self.rt,
                    source,
                };
                let done = h(&mut cx, action.as_ref());
                self.handlers.insert((id, tid), h);
                if done {
                    return;
                }
            }
            cur = self.nodes[k].parent;
        }
        self.actions.push(ActionEnvelope { source, action });
    }

    // ---- dispatch -------------------------------------------------------------------

    /// Deliver an event to one widget.
    pub(crate) fn dispatch(&mut self, k: NodeKey, ev: &UiEvent) -> Handled {
        let Some(node) = self.nodes.get_mut(k) else {
            return Handled::No;
        };
        let Some(mut w) = node.widget.take() else {
            return Handled::No;
        };
        let id = node.id;
        let h = {
            let mut cx = EventCx {
                ui: self,
                key: k,
                id,
            };
            w.event(&mut cx, ev)
        };
        if let Some(node) = self.nodes.get_mut(k) {
            node.widget = Some(w);
        }
        h
    }

    /// Deliver an event to a widget and bubble it up until handled.
    fn bubble(&mut self, k: NodeKey, ev: &UiEvent) -> Handled {
        let mut cur = Some(k);
        while let Some(n) = cur {
            if self.dispatch(n, ev).is_yes() {
                return Handled::Yes;
            }
            cur = self.nodes.get(n).and_then(|n| n.parent);
        }
        Handled::No
    }

    /// Send an event to a widget by id (tests, a11y actions, programmatic activation).
    pub fn send(&mut self, id: WidgetId, ev: &UiEvent) -> Handled {
        match self.index.get(&id).copied() {
            Some(k) => self.dispatch(k, ev),
            None => Handled::No,
        }
    }

    // ---- focus ----------------------------------------------------------------------

    fn widget_of(&self, k: NodeKey) -> Option<&dyn Widget> {
        self.nodes.get(k)?.widget.as_deref()
    }

    /// Move focus. `by_keyboard` decides whether the focus ring is drawn.
    pub fn set_focus(&mut self, id: Option<WidgetId>, by_keyboard: bool) {
        let new_k = id.and_then(|i| self.index.get(&i).copied());
        let new_id = new_k.map(|k| self.nodes[k].id);
        if new_id == self.focused {
            if let Some(k) = new_k
                && self.nodes[k].state.focus_visible != by_keyboard
            {
                self.nodes[k].state.focus_visible = by_keyboard;
                self.mark(k, Dirty::PAINT);
            }
            return;
        }
        if let Some(old) = self.focused.and_then(|f| self.index.get(&f).copied()) {
            self.nodes[old].state.focused = false;
            self.nodes[old].state.focus_visible = false;
            self.mark(old, Dirty::PAINT | Dirty::A11Y);
            self.dispatch(old, &UiEvent::FocusLost);
        }
        self.focused = new_id;
        self.a11y.focus_changed = true;
        self.ime.caret = None;
        self.ime.allowed = false;
        if let Some(k) = new_k {
            self.nodes[k].state.focused = true;
            self.nodes[k].state.focus_visible = by_keyboard;
            self.ime.allowed = self.widget_of(k).is_some_and(|w| w.accepts_text());
            self.mark(k, Dirty::PAINT | Dirty::A11Y);
            if let Some(scope) = self.scope_of(k) {
                let sid = self.nodes[scope].id;
                self.scope_memory.insert(sid, self.nodes[k].id);
            }
            self.dispatch(k, &UiEvent::FocusGained { by_keyboard });
        }
        if by_keyboard {
            self.tooltip_target(new_id);
            if let Some(id) = new_id {
                self.scroll_into_view(id);
            }
        }
    }

    fn scope_of(&self, k: NodeKey) -> Option<NodeKey> {
        let mut cur = self.nodes.get(k)?.parent;
        while let Some(n) = cur {
            if self.nodes[n].style.scope.is_some() {
                return Some(n);
            }
            cur = self.nodes[n].parent;
        }
        None
    }

    /// The last shown modal, if any: it traps focus.
    fn active_modal(&self) -> Option<NodeKey> {
        self.paint_order()
            .into_iter()
            .filter(|k| self.nodes[*k].style.scope == Some(FocusScope::Modal))
            .last()
    }

    /// Visible focusable widgets under `scope` in visual order.
    fn focusables_in(&self, scope: NodeKey) -> Vec<Focusable> {
        // Order by *unscrolled* positions: a widget inside a scroll viewport keeps its place
        // in the reading order however far the viewport has scrolled (Tab scrolls it into
        // view, which must not reshuffle the order and trap the walk).
        let mut out = Vec::new();
        let mut stack = vec![(scope, 0.0f32, 0.0f32)];
        while let Some((k, ox, oy)) = stack.pop() {
            let n = &self.nodes[k];
            if n.hidden {
                continue;
            }
            if n.widget.as_ref().is_some_and(|w| w.focusable()) && k != scope {
                out.push(Focusable {
                    id: n.id,
                    rect: n.rect.translate(ox, oy),
                });
            }
            let (sx, sy) = n.scroll.map_or((0.0, 0.0), |s| (s.offset.x, s.offset.y));
            stack.extend(n.children.iter().map(|c| (*c, ox + sx, oy + sy)));
        }
        focus::visual_order(out)
    }

    fn traversal_scope(&self) -> NodeKey {
        if let Some(m) = self.active_modal() {
            return m;
        }
        self.focused
            .and_then(|f| self.index.get(&f).copied())
            .and_then(|k| self.scope_of(k))
            .unwrap_or(self.root)
    }

    /// Tab / Shift+Tab.
    pub fn focus_next(&mut self, backward: bool) {
        let scope = self.traversal_scope();
        let order = self.focusables_in(scope);
        let next = focus::next(&order, self.focused, backward);
        if next.is_some() {
            self.set_focus(next, true);
        }
    }

    /// F6: cycle focus between panels (scopes), restoring each panel's last focus.
    pub fn focus_next_panel(&mut self, backward: bool) {
        if self.active_modal().is_some() {
            return;
        }
        let scopes: Vec<Focusable> = focus::visual_order(
            self.paint_order()
                .into_iter()
                .filter(|k| self.nodes[*k].style.scope == Some(FocusScope::Panel))
                .map(|k| Focusable {
                    id: self.nodes[k].id,
                    rect: self.nodes[k].rect,
                })
                .collect(),
        );
        let cur = self
            .focused
            .and_then(|f| self.index.get(&f).copied())
            .and_then(|k| self.scope_of(k))
            .map(|k| self.nodes[k].id);
        // Panels with nothing focusable are skipped.
        let mut probe = cur;
        for _ in 0..scopes.len() {
            let Some(next_scope) = focus::next(&scopes, probe, backward) else {
                return;
            };
            let target = self
                .scope_memory
                .get(&next_scope)
                .copied()
                .filter(|w| self.is_visible(*w))
                .or_else(|| {
                    let sk = *self.index.get(&next_scope)?;
                    self.focusables_in(sk).first().map(|f| f.id)
                });
            if target.is_some() {
                self.set_focus(target, true);
                return;
            }
            probe = Some(next_scope);
        }
    }

    /// The parent of a widget.
    pub fn parent(&self, id: WidgetId) -> Option<WidgetId> {
        let k = self.index.get(&id)?;
        self.nodes[*k].parent.map(|p| self.nodes[p].id)
    }

    /// Whether a popup (menu, combo list, dialog, popover) is open in this window.
    pub fn has_popups(&self) -> bool {
        !self.popups.is_empty()
    }

    /// Spatial navigation (gamepads; §21.20).
    pub fn navigate(&mut self, dir: Direction) {
        let scope = self.traversal_scope();
        let items = self.focusables_in(scope);
        let from = self
            .focused
            .and_then(|f| self.rect(f))
            .unwrap_or(Rect::new(0.0, 0.0, 0.0, 0.0));
        if self.focused.is_none() {
            let first = items.first().map(|f| f.id);
            self.set_focus(first, true);
            return;
        }
        let others: Vec<Focusable> = items
            .into_iter()
            .filter(|f| Some(f.id) != self.focused)
            .collect();
        if let Some(t) = focus::spatial(&others, from, dir) {
            self.set_focus(Some(t), true);
        }
    }

    // ---- input routing --------------------------------------------------------------

    /// Nodes in paint order (pre-order, visible only).
    pub(crate) fn paint_order(&self) -> Vec<NodeKey> {
        let mut out = Vec::new();
        let mut stack = vec![self.root];
        while let Some(k) = stack.pop() {
            let n = &self.nodes[k];
            if n.hidden {
                continue;
            }
            out.push(k);
            stack.extend(n.children.iter().rev().copied());
        }
        out
    }

    /// Topmost hit-testable widget at `p`, honouring clips.
    pub fn hit_test(&self, p: Point) -> Option<WidgetId> {
        self.hit_key(p).map(|k| self.nodes[k].id)
    }

    fn hit_key(&self, p: Point) -> Option<NodeKey> {
        // Walk front to back: reverse paint order, with each node's effective clip.
        let mut best = None;
        let mut stack: Vec<(NodeKey, Option<Rect>)> = vec![(self.root, None)];
        while let Some((k, clip)) = stack.pop() {
            let n = &self.nodes[k];
            if n.hidden {
                continue;
            }
            let inside_clip = clip.is_none_or(|c| c.contains(p));
            if n.style.hit_test && inside_clip && n.rect.contains(p) {
                best = Some(k); // later in paint order = on top
            }
            let child_clip = if n.style.clip {
                Some(clip.map_or(n.rect, |c| c.intersect(&n.rect)))
            } else {
                clip
            };
            for c in n.children.iter().rev() {
                stack.push((*c, child_clip));
            }
        }
        best
    }

    fn interactive_ancestor(&self, k: NodeKey) -> Option<NodeKey> {
        let mut cur = Some(k);
        while let Some(n) = cur {
            if self.widget_of(n).is_some_and(|w| w.focusable()) {
                return Some(n);
            }
            cur = self.nodes[n].parent;
        }
        None
    }

    fn set_hover(&mut self, new: Option<NodeKey>) {
        let new_id = new.map(|k| self.nodes[k].id);
        if new_id == self.hover {
            return;
        }
        if let Some(old) = self.hover.and_then(|h| self.index.get(&h).copied()) {
            self.nodes[old].state.hovered = false;
            if self.widget_of(old).is_some_and(|w| w.hover_sensitive()) {
                self.mark(old, Dirty::PAINT);
            }
            self.dispatch(old, &UiEvent::PointerLeave);
        }
        self.hover = new_id;
        self.tooltip_target(new_id);
        if let Some(k) = new {
            self.nodes[k].state.hovered = true;
            if self.widget_of(k).is_some_and(|w| w.hover_sensitive()) {
                self.mark(k, Dirty::PAINT);
            }
            if self.faults.hover_marks_root() {
                self.mark(self.root, Dirty::PAINT);
            }
            self.dispatch(k, &UiEvent::PointerEnter);
        }
    }

    /// Route one window-level input event (steps 2–3 of §21.3).
    pub fn handle(&mut self, ev: InputEvent) {
        self.route(ev);
        self.flush_pending_close();
    }

    fn route(&mut self, ev: InputEvent) {
        match ev {
            InputEvent::PointerMoved(pos) => {
                self.pointer = Some(pos);
                let hit = self.hit_key(pos);
                let hover = hit.and_then(|k| self.interactive_ancestor(k)).or(hit);
                self.set_hover(hover);
                let target = self
                    .capture
                    .and_then(|c| self.index.get(&c).copied())
                    .or(hit);
                if let Some(t) = target {
                    self.dispatch(t, &UiEvent::PointerMove { pos });
                }
                if let Some(d) = self.drag.as_mut() {
                    let prev = d.over.take().map(|(w, _)| w);
                    let payload = d.payload.clone();
                    if let Some(h) = hit {
                        self.bubble_drag_over(h, payload);
                    }
                    let now = self
                        .drag
                        .as_ref()
                        .and_then(|d| d.over.as_ref())
                        .map(|o| o.0);
                    if let Some(p) = prev
                        && now != Some(p)
                    {
                        // The drag left the target that answered last: it drops its preview.
                        self.send(p, &UiEvent::DragLeave);
                    }
                }
            }
            InputEvent::PointerButton {
                pos,
                button,
                pressed: true,
            } => {
                self.hide_tooltip();
                self.pointer = Some(pos);
                let hit = self.hit_key(pos);
                if self.dismiss_outside(hit) {
                    return; // a modal's scrim: nothing beneath is reachable
                }
                let hit = self.hit_key(pos);
                let clicks = match self.last_click {
                    Some((t, p, b, n))
                        if b == button
                            && self.now.saturating_sub(t) <= DOUBLE_CLICK
                            && (p.x - pos.x).abs() <= 4.0
                            && (p.y - pos.y).abs() <= 4.0 =>
                    {
                        n.saturating_add(1).min(3)
                    }
                    _ => 1,
                };
                self.last_click = Some((self.now, pos, button, clicks));
                let interactive = hit.and_then(|k| self.interactive_ancestor(k));
                let focus_to = interactive.map(|k| self.nodes[k].id);
                self.set_focus(focus_to, false);
                if let Some(k) = interactive {
                    self.nodes[k].state.pressed = true;
                    self.pressed = Some(self.nodes[k].id);
                    self.mark(k, Dirty::PAINT);
                }
                self.press_target = hit.map(|k| self.nodes[k].id);
                if let Some(h) = hit {
                    self.bubble(
                        h,
                        &UiEvent::PointerDown {
                            pos,
                            button,
                            clicks,
                        },
                    );
                }
            }
            InputEvent::PointerButton {
                pos,
                button,
                pressed: false,
            } => {
                self.pointer = Some(pos);
                let hit = self.hit_key(pos);
                let target = self
                    .capture
                    .and_then(|c| self.index.get(&c).copied())
                    .or_else(|| self.press_target.and_then(|p| self.index.get(&p).copied()))
                    .or(hit);
                let inside = match (
                    hit,
                    self.press_target.and_then(|p| self.index.get(&p).copied()),
                ) {
                    (Some(h), Some(p)) => self.is_ancestor_or_self_key(p, h),
                    _ => false,
                };
                if let Some(pk) = self
                    .pressed
                    .take()
                    .and_then(|p| self.index.get(&p).copied())
                {
                    self.nodes[pk].state.pressed = false;
                    self.mark(pk, Dirty::PAINT);
                }
                if let Some(t) = target {
                    self.bubble(
                        t,
                        &UiEvent::PointerUp {
                            pos,
                            button,
                            inside,
                        },
                    );
                }
                self.capture = None;
                self.press_target = None;
                if let Some(d) = self.drag.take()
                    && let Some((over, verdict)) = d.over
                {
                    if matches!(verdict, DropVerdict::Refused(_)) {
                        self.send(over, &UiEvent::DragLeave);
                    } else {
                        self.send(over, &UiEvent::Drop(d.payload));
                    }
                }
            }
            InputEvent::PointerLeft => {
                self.set_hover(None);
                // A captured drag keeps its last position (the OS keeps reporting moves
                // outside the window to the capturing window); otherwise it is gone.
                if self.capture.is_none() {
                    self.pointer = None;
                }
            }
            InputEvent::Wheel { pos, dx, dy } => {
                if let Some(h) = self.hit_key(pos) {
                    self.bubble(h, &UiEvent::Wheel { dx, dy });
                }
            }
            InputEvent::Key(k) => {
                if !k.pressed {
                    return;
                }
                self.hide_tooltip();
                if k.code == KeyCode::Escape
                    && let Some(d) = self.drag.take()
                {
                    // Escape cancels a drag: nothing is dropped, the target drops its preview.
                    if let Some((over, _)) = d.over {
                        self.send(over, &UiEvent::DragLeave);
                    }
                    if let Some(src) = self.index.get(&d.source).copied() {
                        self.dispatch(src, &UiEvent::DragCancelled);
                    }
                    return;
                }
                if self.game_nav
                    && self.popups.is_empty()
                    && k.mods == crate::input::Modifiers::NONE
                    && matches!(k.code, KeyCode::Up | KeyCode::Down)
                {
                    self.navigate(if k.code == KeyCode::Up {
                        Direction::Up
                    } else {
                        Direction::Down
                    });
                    return;
                }
                let start = self
                    .focused
                    .and_then(|f| self.index.get(&f).copied())
                    .unwrap_or(self.root);
                let handled = self.bubble(start, &UiEvent::Key(k.clone()));
                if handled.is_yes() {
                    return;
                }
                // The window's key sink (the editor's keymap host) gets what the focus chain
                // left — also with nothing focused, so a shortcut never needs focus. When
                // the focused widget is inside the sink, the bubble already reached it.
                if let Some(sk) = self.key_sink.and_then(|s| self.index.get(&s).copied())
                    && !self.is_ancestor_or_self_key(sk, start)
                    && self.dispatch(sk, &UiEvent::Key(k.clone())).is_yes()
                {
                    return;
                }
                if k.code == KeyCode::Escape && self.escape_popup() {
                    return;
                }
                match k.code {
                    KeyCode::Tab => self.focus_next(k.mods.shift),
                    KeyCode::F6 => self.focus_next_panel(k.mods.shift),
                    KeyCode::Left if self.spatial_arrows => self.navigate(Direction::Left),
                    KeyCode::Right if self.spatial_arrows => self.navigate(Direction::Right),
                    KeyCode::Up if self.spatial_arrows => self.navigate(Direction::Up),
                    KeyCode::Down if self.spatial_arrows => self.navigate(Direction::Down),
                    _ => {}
                }
            }
            InputEvent::Ime(ime) => {
                if let Some(fk) = self.focused.and_then(|f| self.index.get(&f).copied()) {
                    self.dispatch(fk, &UiEvent::Ime(ime));
                }
            }
            InputEvent::FileDropped { pos, path } => {
                if let Some(h) = self.hit_key(pos) {
                    self.bubble(h, &UiEvent::Drop(DragPayload::Files(vec![path])));
                }
            }
            InputEvent::WindowFocused(false) => {
                self.set_hover(None);
                self.capture = None;
            }
            InputEvent::WindowFocused(true) => {}
            InputEvent::Modifiers(m) => self.mods = m,
        }
    }

    fn bubble_drag_over(&mut self, k: NodeKey, payload: DragPayload) {
        let mut cur = Some(k);
        while let Some(n) = cur {
            self.dispatch(n, &UiEvent::DragOver(payload.clone()));
            if self.drag.as_ref().is_some_and(|d| d.over.is_some()) {
                self.mark(n, Dirty::PAINT);
                return;
            }
            cur = self.nodes[n].parent;
        }
    }

    /// The in-flight drag, for previews.
    pub fn drag(&self) -> Option<&DragSession> {
        self.drag.as_ref()
    }

    /// The last pointer position in this window (logical px). During a captured drag it
    /// may lie outside the window (the OS keeps reporting to the capturing window).
    pub fn pointer(&self) -> Option<Point> {
        self.pointer
    }

    /// The platform tells the window where its client area is on the desktop (logical px
    /// of the virtual screen) and which monitor it is on (§21.14). Docking uses it to tear
    /// a panel off to the screen position it was dropped at.
    pub fn set_window_origin(&mut self, origin: Point, monitor: Option<String>) {
        self.window_origin = origin;
        self.monitor = monitor;
    }

    /// Where this window's client area is on the desktop (logical px).
    pub fn window_origin(&self) -> Point {
        self.window_origin
    }

    /// The monitor this window is on, if the platform reported one.
    pub fn monitor(&self) -> Option<&str> {
        self.monitor.as_deref()
    }

    /// Make `id` the window's key sink: it receives every key press the focused widget and
    /// its ancestors did not handle, and every key press while nothing is focused (the
    /// editor's keymap host, §21.17). `None` removes it.
    pub fn set_key_sink(&mut self, id: Option<WidgetId>) {
        self.key_sink = id;
    }

    pub(crate) fn is_ancestor_or_self_key(&self, a: NodeKey, mut b: NodeKey) -> bool {
        loop {
            if a == b {
                return true;
            }
            match self.nodes.get(b).and_then(|n| n.parent) {
                Some(p) => b = p,
                None => return false,
            }
        }
    }

    // ---- the frame ------------------------------------------------------------------

    fn sync_feeds(&mut self) {
        let vis: HashMap<WidgetId, bool> = self
            .feeds
            .feeds
            .values()
            .map(|f| (f.owner, self.is_visible(f.owner)))
            .collect();
        let became = self.feeds.sync_wakers(
            &self.waker,
            |w| vis.get(&w).copied().unwrap_or(false),
            self.faults.feeds(),
        );
        // On becoming visible, a panel compares generations and refreshes once.
        for id in became {
            if let Some(f) = self.feeds.feeds.get(&id)
                && f.feed.source.generation() != f.seen_gen
            {
                self.refresh_feed(id);
            }
        }
    }

    fn refresh_feed(&mut self, id: FeedId) {
        let Some(f) = self.feeds.feeds.get_mut(&id) else {
            return;
        };
        f.seen_gen = f.feed.source.generation();
        f.last_refresh = Some(self.now);
        f.feed.source.consumed();
        let owner = f.owner;
        if let Some(k) = self.index.get(&owner).copied() {
            self.dispatch(k, &UiEvent::FeedChanged(id));
        }
    }

    /// Rules 1–4 of §21.11: refresh visible, changed, non-self feeds at ≤ `max_hz`.
    fn poll_feeds_after_wake(&mut self) {
        let now = self.now;
        let mut refresh = Vec::new();
        let mut defer = Vec::new();
        for (id, f) in &self.feeds.feeds {
            if f.feed.self_ui && !self.faults.feeds().self_ui_can_wake() {
                continue;
            }
            if !f.visible && !self.faults.feeds().keep_waker_when_hidden() {
                continue;
            }
            if f.feed.source.generation() == f.seen_gen {
                continue;
            }
            let period = Duration::from_secs_f64(1.0 / f64::from(f.feed.max_hz.max(1)));
            match f.last_refresh {
                Some(t) if now < t + period => defer.push((*id, f.owner, t + period)),
                _ => refresh.push(*id),
            }
        }
        for id in refresh {
            self.refresh_feed(id);
        }
        for (id, owner, at) in defer {
            let tag = FEED_TIMER_BASE + u64::from(id.0);
            self.scheduler.cancel_timer(owner, tag);
            self.scheduler.set_timer(owner, at, tag);
        }
    }

    /// Run steps 1 and 4–7 of the loop at presentation time `now`.
    pub fn frame(&mut self, now: UiTime) -> FrameReport {
        self.now = self.now.max(now);
        // 1. drain the cross-thread queue.
        while let Ok(f) = self.rx.try_recv() {
            f(&mut self.rt);
        }
        // Timers and animation frames due now.
        for (w, tag) in self.scheduler.due_timers(self.now) {
            if tag == crate::overlay::TOOLTIP_TAG {
                self.show_tooltip(w);
                continue;
            }
            if tag >= FEED_TIMER_BASE {
                continue; // a deferred feed refresh: handled by poll_feeds below
            }
            if let Some(k) = self.index.get(&w).copied() {
                self.dispatch(k, &UiEvent::Timer(tag));
            }
        }
        for w in self.scheduler.due_anims(self.now) {
            if let Some(k) = self.index.get(&w).copied() {
                self.dispatch(k, &UiEvent::AnimFrame);
            }
        }
        self.sync_feeds();
        self.poll_feeds_after_wake();
        // 4. flush signals. A composite reconciling its parts (a colour picker writing the
        // colour its exposure field changed) may set signals while handling
        // `BindingChanged`; those settle in this same frame. Equal writes are no-ops, so
        // the rounds converge; the cap only stops a mis-written widget ping-ponging.
        for _ in 0..4 {
            let marks = self.rt.flush();
            for (w, d) in marks {
                if let Some(k) = self.index.get(&w).copied() {
                    self.mark(k, d);
                    self.dispatch(k, &UiEvent::BindingChanged);
                }
            }
            if !self.rt.has_pending() {
                break;
            }
        }
        self.flush_pending_close();
        // 5. layout.
        if self.layout_dirty {
            self.layout();
        } else if self.rects_dirty {
            self.place_rects();
        }
        self.flush_pending_close();
        // 6. re-record dirty slices.
        self.paint_dirty();
        // 7. accessibility.
        self.update_a11y();
        for k in std::mem::take(&mut self.dirty_keys) {
            if let Some(n) = self.nodes.get_mut(k) {
                n.dirty = Dirty::NONE;
            }
        }
        let damage: Vec<Rect> = self
            .damage
            .iter()
            .filter(|r| !r.is_empty())
            .copied()
            .collect();
        self.stats.last_damage = damage.clone();
        FrameReport {
            wake: self.scheduler.next_wake(!damage.is_empty()),
            damage,
        }
    }

    /// Whether `render` would draw anything.
    pub fn has_damage(&self) -> bool {
        self.damage.iter().any(|r| !r.is_empty())
    }

    /// The zero-idle decision for the loop right now.
    pub fn next_wake(&self) -> Wake {
        self.scheduler.next_wake(self.has_damage())
    }

    fn layout(&mut self) {
        self.stats.layout_passes += 1;
        self.layout_dirty = false;
        let root_t = self.nodes[self.root].taffy;
        let avail = taffy::Size {
            width: taffy::AvailableSpace::Definite(self.size.w),
            height: taffy::AvailableSpace::Definite(self.size.h),
        };
        {
            let nodes = &mut self.nodes;
            let index = &self.index;
            let mut res = self.res.borrow_mut();
            res.text.set_scale(self.scale);
            let text = &mut res.text;
            let rt = &self.rt;
            let theme = &self.theme;
            let _ =
                self.taffy
                    .compute_layout_with_measure(root_t, avail, |input, _nid, ctx, style| {
                        let wid = ctx.map(|c| *c);
                        taffy::compute_leaf_layout(
                            input,
                            style,
                            |_, _| 0.0,
                            |known, avail| {
                                let Some(k) = wid.and_then(|w| index.get(&w).copied()) else {
                                    return taffy::Size::ZERO;
                                };
                                let Some(w) = nodes.get_mut(k).and_then(|n| n.widget.as_mut())
                                else {
                                    return taffy::Size::ZERO;
                                };
                                let mut cx = MeasureCx { text, rt, theme };
                                let s = w.measure(&mut cx, known, avail);
                                taffy::Size {
                                    width: s.w,
                                    height: s.h,
                                }
                            },
                        )
                    });
        }
        self.place_rects();
    }

    /// Absolute rects from the taffy layout (and scroll offsets); anything that moved is
    /// repainted, and its old area damaged.
    fn place_rects(&mut self) {
        self.rects_dirty = false;
        let mut stack = vec![(self.root, 0.0f32, 0.0f32)];
        while let Some((k, ox, oy)) = stack.pop() {
            let t = self.nodes[k].taffy;
            let Ok(l) = self.taffy.layout(t) else {
                continue;
            };
            let r = Rect::new(
                ox + l.location.x,
                oy + l.location.y,
                l.size.width,
                l.size.height,
            );
            if self.nodes[k].rect != r {
                self.damage.push(self.nodes[k].slice_bounds);
                self.nodes[k].rect = r;
                self.mark(k, Dirty::PAINT | Dirty::A11Y);
            }
            if self.nodes[k].hidden {
                continue;
            }
            let (sx, sy) = self.nodes[k]
                .scroll
                .map_or((0.0, 0.0), |s| (s.offset.x, s.offset.y));
            for c in self.nodes[k].children.clone() {
                stack.push((c, r.x - sx, r.y - sy));
            }
        }
        self.update_scroll_extents();
    }

    // ---- scrolling ------------------------------------------------------------------

    /// Scroll a viewport (`NodeStyle::scrollable`); the offset is clamped to its content.
    /// Only rects move: no taffy pass, and only the slices that moved re-record.
    pub fn set_scroll(&mut self, id: WidgetId, offset: Point) {
        let Some(k) = self.index.get(&id).copied() else {
            return;
        };
        let Some(s) = self.nodes[k].scroll else {
            return;
        };
        let max = s.max_offset();
        let o = Point::new(offset.x.clamp(0.0, max.x), offset.y.clamp(0.0, max.y));
        if o == s.offset {
            return;
        }
        if let Some(st) = self.nodes[k].scroll.as_mut() {
            st.offset = o;
        }
        self.rects_dirty = true;
        if let Some(p) = self.nodes[k].parent {
            self.dispatch(p, &UiEvent::ScrollChanged);
        }
    }

    /// Scroll every scroll viewport above `id` so that `id` is visible (keyboard focus
    /// never lands on something scrolled out of view).
    pub fn scroll_into_view(&mut self, id: WidgetId) {
        let Some(mut k) = self.index.get(&id).copied() else {
            return;
        };
        let target = self.nodes[k].rect;
        while let Some(p) = self.nodes[k].parent {
            if let Some(s) = self.nodes[p].scroll {
                let v = self.nodes[p].rect;
                let mut o = s.offset;
                if target.y < v.y {
                    o.y -= v.y - target.y;
                } else if target.bottom() > v.bottom() {
                    o.y += (target.bottom() - v.bottom()).min(target.y - v.y);
                }
                if target.x < v.x {
                    o.x -= v.x - target.x;
                } else if target.right() > v.right() {
                    o.x += (target.right() - v.right()).min(target.x - v.x);
                }
                let pid = self.nodes[p].id;
                self.set_scroll(pid, o);
            }
            k = p;
        }
    }

    /// A viewport's scroll state.
    pub fn scroll_state(&self, id: WidgetId) -> Option<ScrollState> {
        self.index.get(&id).and_then(|k| self.nodes[*k].scroll)
    }

    /// After placement: recompute each viewport's content extent, clamp its offset, and
    /// tell its parent (the scroll area) when anything changed.
    fn update_scroll_extents(&mut self) {
        let viewports: Vec<NodeKey> = self
            .nodes
            .iter()
            .filter(|(_, n)| n.scroll.is_some() && !n.hidden)
            .map(|(k, _)| k)
            .collect();
        for k in viewports {
            let r = self.nodes[k].rect;
            let Some(old) = self.nodes[k].scroll else {
                continue;
            };
            let pad = self
                .taffy
                .layout(self.nodes[k].taffy)
                .map(|l| (l.padding.right, l.padding.bottom))
                .unwrap_or((0.0, 0.0));
            let mut w: f32 = 0.0;
            let mut h: f32 = 0.0;
            for c in &self.nodes[k].children {
                let cr = self.nodes[*c].rect;
                // Child rects are already shifted by the offset; undo it.
                w = w.max(cr.right() - r.x + old.offset.x);
                h = h.max(cr.bottom() - r.y + old.offset.y);
            }
            let mut s = ScrollState {
                offset: old.offset,
                content: Size::new(w + pad.0, h + pad.1),
                viewport: Size::new(r.w, r.h),
            };
            let max = s.max_offset();
            s.offset = Point::new(s.offset.x.min(max.x), s.offset.y.min(max.y));
            if s != old {
                self.nodes[k].scroll = Some(s);
                if s.offset != old.offset {
                    self.rects_dirty = true;
                }
                if let Some(p) = self.nodes[k].parent {
                    self.dispatch(p, &UiEvent::ScrollChanged);
                }
            }
        }
        if self.rects_dirty {
            // A clamp moved the content: place once more (bounded: clamping is idempotent).
            self.rects_dirty = false;
            let mut stack = vec![(self.root, 0.0f32, 0.0f32)];
            while let Some((k, ox, oy)) = stack.pop() {
                let t = self.nodes[k].taffy;
                let Ok(l) = self.taffy.layout(t) else {
                    continue;
                };
                let r = Rect::new(
                    ox + l.location.x,
                    oy + l.location.y,
                    l.size.width,
                    l.size.height,
                );
                if self.nodes[k].rect != r {
                    self.damage.push(self.nodes[k].slice_bounds);
                    self.nodes[k].rect = r;
                    self.mark(k, Dirty::PAINT | Dirty::A11Y);
                }
                if self.nodes[k].hidden {
                    continue;
                }
                let (sx, sy) = self.nodes[k]
                    .scroll
                    .map_or((0.0, 0.0), |s| (s.offset.x, s.offset.y));
                for c in self.nodes[k].children.clone() {
                    stack.push((c, r.x - sx, r.y - sy));
                }
            }
        }
    }

    fn inherited_bg(&self, k: NodeKey) -> ColorRole {
        let mut cur = self.nodes[k].parent;
        while let Some(n) = cur {
            if let Some(b) = self.nodes[n].style.background {
                return b;
            }
            cur = self.nodes[n].parent;
        }
        ColorRole::BgBase
    }

    fn paint_dirty(&mut self) {
        let mut count = 0u64;
        let mut ids = Vec::new();
        let keys = self.dirty_keys.clone();
        for k in keys {
            let Some(n) = self.nodes.get(k) else {
                continue;
            };
            if !n.dirty.contains(Dirty::PAINT) || !self.is_visible_key(k) {
                continue;
            }
            self.paint_node(k);
            count += 1;
            ids.push(self.nodes[k].id);
        }
        self.stats.last_slices = count;
        self.stats.last_slice_ids = ids;
        self.stats.slices_recorded += count;
    }

    fn paint_node(&mut self, k: NodeKey) {
        let bg = self.inherited_bg(k);
        let (rect, state, own_style_bg, radius, id) = {
            let n = &self.nodes[k];
            (
                n.rect,
                n.state,
                n.style.background,
                n.style.radius.map_or(0.0, |r| r.resolve(&self.theme)),
                n.id,
            )
        };
        let focused = self.focused == Some(id);
        let Some(w) = self.nodes[k].widget.take() else {
            return;
        };
        // Reuse the slice's allocation (owner rule 2: no per-frame Vec churn).
        let mut prims = std::mem::take(&mut self.nodes[k].slice);
        prims.clear();
        let mut runs = std::mem::take(&mut self.scratch_runs);
        runs.clear();
        let mut res = self.res.borrow_mut();
        let res = &mut *res;
        res.text.set_scale(self.scale);
        {
            let mut cx = PaintCx {
                rect,
                state,
                theme: &self.theme,
                rt: &self.rt,
                text: &mut res.text,
                meshes: &mut res.meshes,
                prims: &mut prims,
                runs: &mut runs,
                bg,
                own_bg: None,
                pairs: &mut self.used_pairs,
                now: self.now,
                reduced_motion: self.reduced_motion,
                scale: self.scale,
            };
            // The node's declared background fills its rect first (rounded when the node
            // style says so); children inherit it.
            if let Some(b) = own_style_bg {
                cx.fill(rect, b, radius);
            }
            w.paint(&mut cx);
        }
        // The focus ring is always drawn when focus arrived by keyboard (§21.9). It sits a
        // gap away from the widget, so its only adjacent colour is the background it is
        // drawn on, whatever state the widget's own fill is in; it must meet the boundary
        // floor against that background.
        if focused && state.focus_visible {
            let ring = self.theme.color(ColorRole::FocusRing);
            self.used_pairs.insert(ColorPair {
                fg: ColorRole::FocusRing,
                bg,
                kind: PairKind::Boundary,
            });
            let wdt = self.theme.focus_ring_width;
            let gap = FOCUS_RING_GAP;
            prims.push(Primitive::Quad {
                rect: rect.outset(wdt + gap),
                radii: Corners::all(self.theme.radius.md + wdt + gap),
                fill: Color::TRANSPARENT,
                border: Some(Border {
                    width: wdt,
                    color: ring,
                }),
                shadow: None,
            });
        }
        self.nodes[k].widget = Some(w);
        let bounds = prims.iter().fold(Rect::ZERO, |acc, p| {
            acc.union(&p.bounds(|r| res.text.run_size(r), |m| res.meshes.bounds(m)))
        });
        for r in &runs {
            res.text.retain(*r);
        }
        let mut new_meshes: Vec<MeshId> = Vec::new();
        for p in &prims {
            if let Primitive::Mesh { mesh } = p {
                res.meshes.retain(*mesh);
                new_meshes.push(*mesh);
            }
        }
        let n = &mut self.nodes[k];
        let old_runs = std::mem::replace(&mut n.runs, runs);
        let old_meshes = std::mem::replace(&mut n.meshes, new_meshes);
        let old_bounds = n.slice_bounds;
        n.slice = prims;
        n.slice_bounds = bounds;
        n.batch = None;
        for r in &old_runs {
            res.text.release(*r);
        }
        for m in old_meshes {
            res.meshes.release(m);
        }
        self.scratch_runs = old_runs;
        self.repainted.push(k);
        self.damage.push(old_bounds);
        self.damage.push(bounds);
    }

    /// The whole display list in paint order (slices plus clip push/pop).
    pub fn display_list(&self) -> Vec<Primitive> {
        let mut out = Vec::new();
        self.emit(self.root, &mut out);
        out
    }

    fn emit(&self, k: NodeKey, out: &mut Vec<Primitive>) {
        let n = &self.nodes[k];
        if n.hidden {
            return;
        }
        out.extend(n.slice.iter().cloned());
        if n.children.is_empty() {
            return;
        }
        if n.style.clip {
            out.push(Primitive::PushClip {
                rect: n.rect,
                radii: Corners::ZERO,
            });
        }
        for c in &n.children {
            self.emit(*c, out);
        }
        if n.style.clip {
            out.push(Primitive::PopClip);
        }
    }

    /// The retained batch list the last `render` drew (tests and diagnostics).
    pub fn batch_list(&self) -> &crate::render::BatchList {
        &self.cache.list
    }

    /// A from-scratch batch of the current display list (the reference the retained cache
    /// must match). Does not change the cache.
    #[doc(hidden)]
    pub fn reference_batches(&mut self) -> crate::render::BatchList {
        let list = self.display_list();
        let mut res = self.res.borrow_mut();
        let res = &mut *res;
        res.text.set_scale(self.scale);
        batch::build(
            &list,
            &mut res.text,
            &mut res.images,
            &res.meshes,
            self.scale,
            BatchFaults::default(),
        )
    }

    /// Exact hash of the display list (display-list goldens, §21.13).
    pub fn display_list_hash(&self) -> u64 {
        let mut h = Fnv::new();
        for p in self.display_list() {
            p.hash_into(&mut h);
        }
        h.finish()
    }

    /// Step 8: if there is damage, batch the display list and render only the damaged
    /// rects. With no damage nothing is rendered, submitted or presented (returns `None`).
    pub fn render(
        &mut self,
        renderer: &mut dyn UiRenderer,
        target: TargetId,
    ) -> Result<Option<FrameStats>, UiError> {
        if !self.has_damage() {
            return Ok(None);
        }
        self.update_batches();
        let win = Rect::new(0.0, 0.0, self.size.w, self.size.h).to_px(self.scale);
        let px: Vec<PxRect> = self
            .damage
            .iter()
            .map(|r| r.to_px(self.scale).intersect(&win))
            .collect();
        let merged = merge_damage(px, 4);
        let stats = renderer.render(target, &self.cache.list, &merged)?;
        self.cache.list.pre_uploads.clear();
        self.cache.list.instance_uploads.clear();
        self.cache.list.mesh_vertex_uploads.clear();
        self.cache.list.mesh_index_uploads.clear();
        self.cache.list.meshes_changed = false;
        self.damage.clear();
        {
            let mut res = self.res.borrow_mut();
            res.text.end_frame();
            res.meshes.collect();
        }
        self.stats.frames_rendered += 1;
        self.stats.last_frame = stats;
        Ok(Some(stats))
    }

    fn invalidate_batches(&mut self) {
        for n in self.nodes.values_mut() {
            n.batch = None;
        }
        self.cache.structure_dirty = true;
    }

    /// Bring the retained batch list up to date: batch only the re-recorded slices and
    /// patch them into their slots; reassemble only on a structure change; fall back to
    /// the full legacy build for an atlas epoch or the batching-disabled control.
    ///
    /// Allocation-free in the steady state: the repainted list and the changed list are
    /// retained scratch buffers that keep their capacity across frames.
    fn update_batches(&mut self) {
        // Swap the filled repaint list for the empty spare: `self.repainted` keeps
        // collecting into retained capacity, and the spare comes back empty below.
        let mut repainted = std::mem::take(&mut self.repainted_spare);
        std::mem::swap(&mut repainted, &mut self.repainted);
        let mut changed = std::mem::take(&mut self.batch_changed);
        changed.clear();
        self.update_batches_with(&mut repainted, &mut changed);
        repainted.clear();
        changed.clear();
        self.repainted_spare = repainted;
        self.batch_changed = changed;
    }

    /// Capacities of the retained batching scratch lists `(repaint, changed)` (tests).
    #[doc(hidden)]
    pub fn batch_scratch_capacity(&self) -> (usize, usize) {
        (
            self.repainted
                .capacity()
                .max(self.repainted_spare.capacity()),
            self.batch_changed.capacity(),
        )
    }

    fn update_batches_with(
        &mut self,
        repainted: &mut Vec<NodeKey>,
        changed: &mut Vec<(NodeKey, Option<Rect>)>,
    ) {
        let scale = self.scale;
        if self.faults.batch().disable_batching() {
            self.legacy_build();
            return;
        }
        if self.faults.rebatch_everything() {
            self.invalidate_batches();
        }
        let res_rc = self.res.clone();
        let mut guard = res_rc.borrow_mut();
        let res = &mut *guard;
        res.text.set_scale(scale);
        if res.text.atlas.generation() != self.cache.atlas_generation {
            // Resident glyphs moved (eviction/epoch): every cached glyph instance is stale.
            self.invalidate_batches();
            self.cache.atlas_generation = res.text.atlas.generation();
        }
        let mut batched = 0u64;
        let mut epoch = false;
        // Batch the re-recorded slices that are still visible.
        for k in repainted.drain(..) {
            if !self.is_visible_key(k) {
                continue;
            }
            let Some(n) = self.nodes.get_mut(k) else {
                continue;
            };
            if n.batch.is_some() {
                continue; // repainted twice since the last render
            }
            match batch_slice(&n.slice, &mut res.text, &res.images, &res.meshes, scale) {
                Ok(b) => {
                    n.batch = Some(b);
                    batched += 1;
                    let clip = (n.style.clip && !n.children.is_empty()).then_some(n.rect);
                    changed.push((k, clip));
                }
                Err(_) => {
                    epoch = true;
                    break;
                }
            }
        }
        if epoch {
            drop(guard);
            self.legacy_build();
            return;
        }
        let mut pre = res.text.atlas.take_updates();
        pre.extend(res.images.take_updates());
        if res.text.atlas.generation() != self.cache.atlas_generation {
            // Batching the changed slices evicted glyphs other slices use: re-batch all.
            self.invalidate_batches();
            self.cache.atlas_generation = res.text.atlas.generation();
        }
        let patched = {
            let nodes = &self.nodes;
            self.cache.patch(
                changed,
                |k| nodes.get(k).and_then(|n| n.batch.as_ref()),
                scale,
            )
        };
        self.stats.last_reassembled = !patched;
        if !patched {
            // Reassemble: batch whatever visible slice has no cached batch, then walk.
            for k in self.paint_order() {
                let Some(n) = self.nodes.get_mut(k) else {
                    continue;
                };
                if n.batch.is_none() {
                    match batch_slice(&n.slice, &mut res.text, &res.images, &res.meshes, scale) {
                        Ok(b) => {
                            n.batch = Some(b);
                            batched += 1;
                        }
                        Err(_) => {
                            epoch = true;
                            break;
                        }
                    }
                }
            }
            if epoch {
                drop(guard);
                self.legacy_build();
                return;
            }
            pre.extend(res.text.atlas.take_updates());
            let mut steps = Vec::with_capacity(self.nodes.len());
            walk_steps(&self.nodes, self.root, &mut steps);
            self.cache.assemble(steps.into_iter(), scale);
            self.stats.reassemblies += 1;
        }
        self.cache.atlas_generation = res.text.atlas.generation();
        self.cache.list.pre_uploads.extend(pre);
        self.cache.list.extra_epochs = 0;
        self.stats.last_slices_batched = batched;
    }

    /// The paint-order walk for reassembly: slices and node-level clips.
    /// The full build over the display list (atlas epochs; the batching-disabled control).
    fn legacy_build(&mut self) {
        let list = self.display_list();
        let batches = {
            let mut res = self.res.borrow_mut();
            let res = &mut *res;
            res.text.set_scale(self.scale);
            batch::build(
                &list,
                &mut res.text,
                &mut res.images,
                &res.meshes,
                self.scale,
                self.faults.batch(),
            )
        };
        self.invalidate_batches();
        self.cache.adopt_legacy(batches);
        self.cache.atlas_generation = self.res.borrow().text.atlas.generation();
        self.stats.last_reassembled = true;
        self.stats.last_slices_batched = self.nodes.len() as u64;
    }

    /// Mark the whole window damaged (first frame on a new surface, surface lost).
    pub fn damage_all(&mut self) {
        self.damage
            .push(Rect::new(0.0, 0.0, self.size.w, self.size.h));
    }

    /// What the IME should do, when it changed since the last call.
    pub fn take_ime_request(&mut self) -> Option<ImeRequest> {
        self.ime.take_request()
    }

    /// All widget ids with their depth, in paint order (diagnostics, gallery walkers).
    pub fn walk(&self) -> Vec<(WidgetId, usize)> {
        let mut out = Vec::new();
        let mut stack = vec![(self.root, 0usize)];
        while let Some((k, d)) = stack.pop() {
            let n = &self.nodes[k];
            out.push((n.id, d));
            for c in n.children.iter().rev() {
                stack.push((*c, d + 1));
            }
        }
        out
    }

    /// The AccessKit role of a widget.
    pub fn role(&self, id: WidgetId) -> Option<accesskit::Role> {
        let k = self.index.get(&id)?;
        self.nodes[*k].widget.as_ref().map(|w| w.role())
    }

    /// Whether a widget is focusable.
    pub fn is_focusable(&self, id: WidgetId) -> bool {
        self.index
            .get(&id)
            .and_then(|k| self.nodes[*k].widget.as_ref())
            .is_some_and(|w| w.focusable())
    }

    pub(crate) fn scale_factor(&self) -> f32 {
        self.scale
    }

    pub(crate) fn dirty_keys_ref(&self) -> &[NodeKey] {
        &self.dirty_keys
    }

    /// Paint pairs by widget, for diagnostics.
    pub fn debug_counts(&self) -> BTreeMap<&'static str, usize> {
        let mut m = BTreeMap::new();
        m.insert("nodes", self.nodes.len());
        m.insert("cached_runs", self.res.borrow().text.cached_runs());
        m.insert("timers", self.scheduler.pending_timers());
        m.insert("anims", self.scheduler.running_anims());
        m
    }
}

/// The paint-order walk for reassembly: slices and node-level clips.
fn walk_steps<'a>(nodes: &'a SlotMap<NodeKey, Node>, k: NodeKey, out: &mut Vec<Step<'a, NodeKey>>) {
    let n = &nodes[k];
    if n.hidden {
        return;
    }
    if let Some(b) = &n.batch {
        out.push(Step::Slice(k, b));
    }
    if n.children.is_empty() {
        return;
    }
    if n.style.clip {
        out.push(Step::PushNodeClip(k, n.rect));
    }
    for c in &n.children {
        walk_steps(nodes, *c, out);
    }
    if n.style.clip {
        out.push(Step::PopNodeClip);
    }
}
