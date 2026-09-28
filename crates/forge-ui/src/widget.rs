//! The `Widget` trait and the contexts widgets work through (Ch.21 §21.4, §21.5).
//!
//! Widgets do not mutate anything outside themselves. They change their own state,
//! set signals they were given, and raise **typed actions** ([`EventCx::action`]) that
//! bubble to the nearest handler. `forge-ui` does not depend on `forge-cmd`: in the
//! editor, action handlers are panel code that emits commands (§21.18); in a game they
//! are game systems (§21.20).

use std::any::Any;
use std::collections::BTreeSet;
use std::time::Duration;

use accesskit::Role;

use crate::damage::{Dirty, UiTime};
use crate::dnd::DropVerdict;
use crate::geom::{Color, Corners, Point, Rect, Size};
use crate::id::WidgetId;
use crate::input::{Handled, UiEvent};
use crate::render::{Border, ImageId, MeshData, MeshStore, Primitive};
use crate::state::{AnySignal, Runtime};
use crate::style::{ColorPair, ColorRole, PairKind, Shadow, Theme, WidgetState};
use crate::text::{GlyphRunId, TextStyle, TextSystem};
use crate::ui::{NodeKey, Ui};

/// Blanket `Any` access for downcasting widgets (tests, typed lookups).
pub trait AsAny {
    fn as_any(&self) -> &dyn Any;
    fn as_any_mut(&mut self) -> &mut dyn Any;
}
impl<T: Any> AsAny for T {
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}

/// A retained widget. Built once, then changed in place by bindings (§21.5).
pub trait Widget: AsAny + 'static {
    /// AccessKit role. Required: a widget with no role does not compile.
    fn role(&self) -> Role;

    /// Declare subscriptions (called once, at insertion): which signals mark this widget
    /// `LAYOUT`/`PAINT`/`A11Y` dirty when they change.
    fn bind(&self, _b: &mut Binder) {}

    /// Leaves only (text, images, virtualised containers); containers are pure taffy.
    fn measured(&self) -> bool {
        false
    }
    fn measure(
        &mut self,
        _cx: &mut MeasureCx,
        _known: taffy::Size<Option<f32>>,
        _avail: taffy::Size<taffy::AvailableSpace>,
    ) -> Size {
        Size::ZERO
    }

    fn event(&mut self, _cx: &mut EventCx, _ev: &UiEvent) -> Handled {
        Handled::No
    }

    /// Records this widget's primitives into its own display-list slice (§21.11).
    fn paint(&self, cx: &mut PaintCx);

    /// Name, value, states and actions for the AccessKit node (role and bounds are set
    /// by the framework).
    fn a11y(&self, _cx: &A11yCx, _node: &mut accesskit::Node) {}

    /// Interactive widgets are focusable (§21.9).
    fn focusable(&self) -> bool {
        false
    }
    /// A text widget: IME is enabled while it has focus.
    fn accepts_text(&self) -> bool {
        false
    }
    /// Repaint on hover enter/leave (interactive widgets). Others never re-record on hover.
    fn hover_sensitive(&self) -> bool {
        self.focusable()
    }

    /// Tooltip text, shown after the pointer rests on (or keyboard focus reaches) this
    /// widget for [`crate::overlay::TOOLTIP_DELAY`] (§21.11 timers). `None`: no tooltip.
    fn tooltip(&self, _rt: &Runtime) -> Option<String> {
        None
    }

    /// Virtual accessibility children — list rows, menu items, curve keys — that exist as
    /// AccessKit nodes without being widgets (§21.10: a virtualised container realises
    /// nodes only for live rows). Each is `(local key, node)`; its node id is derived
    /// from this widget's id and the key, so it is stable across runs.
    fn a11y_children(&self, _cx: &A11yCx, _out: &mut Vec<(u64, accesskit::Node)>) {}

    /// The virtual child that holds focus while this widget is focused (the active row).
    fn a11y_focus(&self, _rt: &Runtime) -> Option<u64> {
        None
    }
}

/// Collects a widget's subscriptions at insertion.
pub struct Binder {
    pub(crate) subs: Vec<(AnySignal, Dirty)>,
}

impl Binder {
    pub fn watch(&mut self, s: Option<AnySignal>, dirty: Dirty) {
        if let Some(s) = s {
            self.subs.push((s, dirty));
        }
    }
}

/// Context for `Widget::measure`.
pub struct MeasureCx<'a> {
    pub text: &'a mut TextSystem,
    pub rt: &'a Runtime,
    pub theme: &'a Theme,
}

/// Context for `Widget::a11y`.
pub struct A11yCx<'a> {
    pub rt: &'a Runtime,
    pub state: WidgetState,
    pub rect: Rect,
    /// Physical pixels per logical pixel (virtual children's bounds are physical).
    pub scale: f32,
}

/// Context for `Widget::paint`: records primitives into the widget's slice.
pub struct PaintCx<'a> {
    pub(crate) rect: Rect,
    pub(crate) state: WidgetState,
    pub(crate) theme: &'a Theme,
    pub(crate) rt: &'a Runtime,
    pub(crate) text: &'a mut TextSystem,
    pub(crate) meshes: &'a mut MeshStore,
    pub(crate) prims: &'a mut Vec<Primitive>,
    pub(crate) runs: &'a mut Vec<GlyphRunId>,
    /// The nearest ancestor's declared background (what this widget is drawn on).
    pub(crate) bg: ColorRole,
    /// The background this widget painted itself, if any.
    pub(crate) own_bg: Option<ColorRole>,
    pub(crate) pairs: &'a mut BTreeSet<ColorPair>,
    pub(crate) now: UiTime,
    pub(crate) reduced_motion: bool,
    /// Display scale (physical px per logical px).
    pub(crate) scale: f32,
}

impl PaintCx<'_> {
    /// Physical pixels per logical pixel.
    pub fn scale(&self) -> f32 {
        self.scale
    }

    /// Draw tessellated geometry (the mesh pipeline). An unchanged mesh re-registers the
    /// same id, so a repaint that did not move the curve tessellates nothing new.
    pub fn mesh(&mut self, data: MeshData) {
        if data.is_empty() {
            return;
        }
        let (mesh, _) = self.meshes.register(data);
        self.prims.push(Primitive::Mesh { mesh });
    }

    /// Draw `icon` from the Forge icon set into `rect` in a token colour. An icon carries
    /// meaning, so it is contrast-checked as text against the current background (§21.8).
    pub fn icon(&mut self, icon: crate::icons::Icon, rect: Rect, role: ColorRole) {
        let bg = self.current_bg();
        self.record(role, bg, PairKind::Text);
        let color = self.theme.color(role);
        self.prims.push(Primitive::Icon { icon, rect, color });
    }

    /// Stroke `path` with a token colour, anti-aliased; a UI boundary against `on`.
    pub fn stroke_path(
        &mut self,
        path: &crate::render::mesh::Path,
        width: f32,
        role: ColorRole,
        on: ColorRole,
    ) {
        self.record(role, on, PairKind::Boundary);
        let mut m = MeshData::default();
        let px = 1.0 / self.scale.max(0.25);
        m.stroke(path, width, self.theme.color(role), px);
        self.mesh(m);
    }

    /// Stroke several polylines in one colour as one mesh (see
    /// [`MeshData::stroke_polyline`]): the node canvas's wires.
    pub fn stroke_polylines(
        &mut self,
        lines: &[Vec<Point>],
        width: f32,
        role: ColorRole,
        on: ColorRole,
    ) {
        self.record(role, on, PairKind::Boundary);
        let mut m = MeshData::default();
        let px = 1.0 / self.scale.max(0.25);
        let c = self.theme.color(role);
        for l in lines {
            m.stroke_polyline(l, width, c, px);
        }
        self.mesh(m);
    }

    /// Stroke straight segments `[ax, ay, bx, by]` (logical px, relative to `origin`) in one
    /// colour as one mesh: a caller's reused segment buffer drawn with no path built.
    pub fn stroke_segments(
        &mut self,
        segs: &[[f32; 4]],
        origin: Point,
        width: f32,
        role: ColorRole,
        on: ColorRole,
    ) {
        if segs.is_empty() {
            return;
        }
        self.record(role, on, PairKind::Boundary);
        let mut m = MeshData::default();
        m.verts.reserve(segs.len() * 4);
        m.indices.reserve(segs.len() * 6);
        let px = 1.0 / self.scale.max(0.25);
        let c = self.theme.color(role);
        for s in segs {
            let a = Point::new(origin.x + s[0], origin.y + s[1]);
            let b = Point::new(origin.x + s[2], origin.y + s[3]);
            m.stroke_polyline(&[a, b], width, c, px);
        }
        self.mesh(m);
    }

    /// Fill `path` with a token colour (optionally translucent), a boundary against `on`.
    pub fn fill_path(
        &mut self,
        path: &crate::render::mesh::Path,
        role: ColorRole,
        on: ColorRole,
        alpha: f32,
    ) {
        self.record(role, on, PairKind::Boundary);
        let mut m = MeshData::default();
        let c = self.theme.color(role);
        m.fill(path, c.with_alpha(c.a * alpha.clamp(0.0, 1.0)));
        self.mesh(m);
    }

    /// Draw rich text (see `TextSystem::layout_rich`): `colors[tag]` colours each span.
    /// Every colour is contrast-checked as text on the current background.
    pub fn rich_text(
        &mut self,
        spans: &[crate::text::RichSpan<'_>],
        style: &TextStyle,
        origin: Point,
        colors: &[ColorRole],
    ) -> (GlyphRunId, Size) {
        let wrap = style.wrap.then_some(self.rect.w);
        let (run, size) = self.text.layout_rich(spans, style, wrap);
        let bg = self.current_bg();
        for c in colors {
            self.record(*c, bg, PairKind::Text);
        }
        self.runs.push(run);
        let cols: std::sync::Arc<[Color]> = colors.iter().map(|c| self.theme.color(*c)).collect();
        self.prims.push(Primitive::RichGlyphs {
            run,
            origin,
            colors: cols,
        });
        (run, size)
    }

    pub fn rect(&self) -> Rect {
        self.rect
    }
    pub fn state(&self) -> WidgetState {
        self.state
    }
    pub fn theme(&self) -> &Theme {
        self.theme
    }
    pub fn rt(&self) -> &Runtime {
        self.rt
    }
    pub fn now(&self) -> UiTime {
        self.now
    }
    pub fn reduced_motion(&self) -> bool {
        self.reduced_motion
    }
    /// The background under this widget (inherited from the nearest ancestor).
    pub fn parent_bg(&self) -> ColorRole {
        self.bg
    }
    /// The background text is drawn on right now.
    pub fn current_bg(&self) -> ColorRole {
        self.own_bg.unwrap_or(self.bg)
    }

    fn record(&mut self, fg: ColorRole, bg: ColorRole, kind: PairKind) {
        self.pairs.insert(ColorPair { fg, bg, kind });
    }

    /// Declare a colour pair drawn by other means (text on a selection highlight).
    pub fn pair(&mut self, fg: ColorRole, bg: ColorRole, kind: PairKind) {
        self.record(fg, bg, kind);
    }

    /// Draw text on an explicit background token (selection highlights).
    pub fn text_on(
        &mut self,
        s: &str,
        style: &TextStyle,
        origin: Point,
        fg: ColorRole,
        bg: ColorRole,
    ) -> Size {
        let saved = self.own_bg;
        self.own_bg = Some(bg);
        let size = self.text(s, style, origin, fg);
        self.own_bg = saved;
        size
    }

    /// A token colour with reduced alpha (spinner trails, disabled fades). The pair is
    /// still contrast-checked at full strength against `on`.
    pub fn mark_alpha(
        &mut self,
        rect: Rect,
        role: ColorRole,
        on: ColorRole,
        radius: f32,
        alpha: f32,
    ) {
        self.record(role, on, PairKind::Boundary);
        let fill = self.theme.color(role);
        self.prims.push(Primitive::Quad {
            rect,
            radii: Corners::all(radius),
            fill: fill.with_alpha(fill.a * alpha.clamp(0.0, 1.0)),
            border: None,
            shadow: None,
        });
    }

    /// Draw an RGBA image as-is (thumbnails): no tint.
    pub fn image_untinted(&mut self, image: ImageId, rect: Rect) {
        self.prims.push(Primitive::Image {
            image,
            rect,
            tint: Color::from_rgba8(255, 255, 255, 255),
        });
    }

    /// Fill a rounded rect with a background token (it becomes the widget's own bg).
    /// A translucent decorative surface (a comment frame's wash, a drop highlight): not a
    /// UI boundary, so not contrast-checked — whatever marks its edge (a `panel` border)
    /// is. It does not become the background later text is checked against.
    pub fn tint(&mut self, rect: Rect, role: ColorRole, radius: f32, alpha: f32) {
        let fill = self.theme.color(role);
        self.prims.push(Primitive::Quad {
            rect,
            radii: Corners::all(radius),
            fill: fill.with_alpha(fill.a * alpha.clamp(0.0, 1.0)),
            border: None,
            shadow: None,
        });
    }

    pub fn fill(&mut self, rect: Rect, role: ColorRole, radius: f32) {
        self.own_bg = Some(role);
        let fill = self.theme.color(role);
        self.prims.push(Primitive::Quad {
            rect,
            radii: Corners::all(radius),
            fill,
            border: None,
            shadow: None,
        });
    }

    /// A filled, bordered, optionally shadowed rounded rect. The border is a UI boundary
    /// and is contrast-checked against the parent background.
    pub fn panel(
        &mut self,
        rect: Rect,
        fill: Option<ColorRole>,
        border: Option<ColorRole>,
        radius: f32,
        shadow: Option<Shadow>,
    ) {
        if let Some(f) = fill {
            self.own_bg = Some(f);
        }
        if let Some(b) = border {
            let bg = self.bg;
            self.record(b, bg, PairKind::Boundary);
        }
        let width = self.theme.border_width;
        self.prims.push(Primitive::Quad {
            rect,
            radii: Corners::all(radius),
            fill: fill
                .map(|f| self.theme.color(f))
                .unwrap_or(Color::TRANSPARENT),
            border: border.map(|b| Border {
                width,
                color: self.theme.color(b),
            }),
            shadow,
        });
    }

    /// A UI mark (check, thumb, indicator) that must meet the boundary floor against `on`.
    pub fn mark(&mut self, rect: Rect, role: ColorRole, on: ColorRole, radius: f32) {
        self.record(role, on, PairKind::Boundary);
        let fill = self.theme.color(role);
        self.prims.push(Primitive::Quad {
            rect,
            radii: Corners::all(radius),
            fill,
            border: None,
            shadow: None,
        });
    }

    /// Shape (cached) and draw text with its top-left at `origin`, in colour `fg` on the
    /// current background. Returns the run's logical size.
    pub fn text(&mut self, s: &str, style: &TextStyle, origin: Point, fg: ColorRole) -> Size {
        let wrap = style.wrap.then_some(self.rect.w);
        let (run, size) = self.text.layout(s, style, wrap);
        let bg = self.current_bg();
        self.record(fg, bg, PairKind::Text);
        self.runs.push(run);
        self.prims.push(Primitive::Glyphs {
            run,
            origin,
            color: self.theme.color(fg),
        });
        size
    }

    /// Draw an already-shaped run (text fields that also need caret positions).
    pub fn run(&mut self, run: GlyphRunId, origin: Point, fg: ColorRole) {
        let bg = self.current_bg();
        self.record(fg, bg, PairKind::Text);
        self.runs.push(run);
        self.prims.push(Primitive::Glyphs {
            run,
            origin,
            color: self.theme.color(fg),
        });
    }

    /// Lay out text without drawing it (caret math).
    pub fn shape(&mut self, s: &str, style: &TextStyle) -> (GlyphRunId, Size) {
        let wrap = style.wrap.then_some(self.rect.w);
        self.text.layout(s, style, wrap)
    }
    pub fn text_system(&self) -> &TextSystem {
        self.text
    }

    pub fn image(&mut self, image: ImageId, rect: Rect, tint: ColorRole) {
        let bg = self.current_bg();
        self.record(tint, bg, PairKind::Text);
        let tint = self.theme.color(tint);
        self.prims.push(Primitive::Image { image, rect, tint });
    }

    /// Escape hatch for renderer-level primitives (viewports). Colours must still come
    /// from the theme.
    pub fn primitive(&mut self, p: Primitive) {
        self.prims.push(p);
    }
}

/// Context for `Widget::event`: the only way a widget affects anything outside itself.
pub struct EventCx<'a> {
    pub(crate) ui: &'a mut Ui,
    pub(crate) key: NodeKey,
    pub(crate) id: WidgetId,
}

impl EventCx<'_> {
    pub fn id(&self) -> WidgetId {
        self.id
    }
    pub fn rect(&self) -> Rect {
        self.ui.node_rect(self.key)
    }
    pub fn state(&self) -> WidgetState {
        self.ui.node_state(self.key)
    }
    pub fn now(&self) -> UiTime {
        self.ui.now
    }
    pub fn rt(&self) -> &Runtime {
        &self.ui.rt
    }
    pub fn rt_mut(&mut self) -> &mut Runtime {
        &mut self.ui.rt
    }
    pub fn theme(&self) -> &Theme {
        &self.ui.theme
    }
    pub fn text(&mut self) -> std::cell::RefMut<'_, TextSystem> {
        self.ui.text_mut()
    }
    pub fn reduced_motion(&self) -> bool {
        self.ui.reduced_motion
    }
    /// Whether text carets blink (the window's setting).
    pub fn caret_blink(&self) -> bool {
        self.ui.caret_blink
    }

    /// Re-record this widget's slice this frame.
    pub fn request_paint(&mut self) {
        self.ui.mark(self.key, Dirty::PAINT);
    }
    /// Re-measure and lay out this widget (implies paint and a11y).
    pub fn request_layout(&mut self) {
        self.ui.mark(self.key, Dirty::LAYOUT);
    }
    pub fn request_a11y(&mut self) {
        self.ui.mark(self.key, Dirty::A11Y);
    }

    /// Fire `UiEvent::Timer(tag)` after `after`. Timers are deadlines in one heap; the
    /// loop sleeps until the earliest and fires nothing early.
    pub fn set_timer(&mut self, after: Duration, tag: u64) {
        let at = self.ui.now + after;
        self.ui.scheduler.set_timer(self.id, at, tag);
    }
    pub fn cancel_timer(&mut self, tag: u64) {
        self.ui.scheduler.cancel_timer(self.id, tag);
    }

    /// Request one `UiEvent::AnimFrame` at the next display frame. An animation calls this
    /// every frame until it settles, then stops (§21.15).
    pub fn request_anim_frame(&mut self) {
        let at = self.ui.now + self.ui.scheduler.frame_interval;
        self.ui.scheduler.request_anim_frame(self.id, at);
    }
    pub fn stop_anim(&mut self) {
        self.ui.scheduler.stop_anim(self.id);
    }

    /// Whether this widget and every ancestor are shown (spinners run only while visible).
    pub fn is_visible(&self) -> bool {
        self.ui.is_visible_key(self.key)
    }

    /// Raise a typed action. It bubbles to the nearest `Ui::on_action` handler, else to
    /// the application's queue.
    pub fn action<A: Any>(&mut self, a: A) {
        self.ui.raise_action(self.id, Box::new(a));
    }

    pub fn request_focus(&mut self) {
        self.ui.set_focus(Some(self.id), false);
    }
    pub fn capture_pointer(&mut self) {
        self.ui.capture = Some(self.id);
    }
    pub fn release_pointer(&mut self) {
        if self.ui.capture == Some(self.id) {
            self.ui.capture = None;
        }
    }

    pub fn clipboard(&mut self) -> &mut dyn crate::clipboard::Clipboard {
        self.ui.clipboard.as_mut()
    }

    /// Report where the text caret is (logical px) so the IME candidate window follows it.
    pub fn set_ime_caret(&mut self, caret: Rect) {
        self.ui.ime.caret = Some(caret);
    }

    /// Start a typed drag from this widget.
    pub fn start_drag(&mut self, payload: crate::dnd::DragPayload) {
        self.ui.drag = Some(crate::dnd::DragSession {
            payload,
            source: self.id,
            over: None,
        });
    }
    /// Show or hide a widget this composite owns (a tab strip's pages).
    pub fn set_hidden(&mut self, id: WidgetId, hidden: bool) {
        let _ = self.ui.set_hidden(id, hidden);
    }

    /// The keyboard modifiers held right now (Ctrl/Shift-click).
    pub fn modifiers(&self) -> crate::input::Modifiers {
        self.ui.mods
    }

    /// Open a popup (menu, dropdown, popover, dialog) owned by this widget. It is placed
    /// by `spec.anchor`, kept inside the window, focused, and closed by an outside click
    /// or Escape; this widget then receives `UiEvent::PopupClosed`.
    pub fn open_popup(
        &mut self,
        key: impl Into<crate::id::Key>,
        spec: crate::overlay::PopupSpec,
        widget: impl Widget,
    ) -> Option<WidgetId> {
        let owner = self.id;
        self.ui
            .open_popup_boxed(owner, key.into(), spec, Box::new(widget))
            .ok()
    }

    /// Close a popup (deferred to the end of this event, so a widget may close itself).
    pub fn close_popup(&mut self, id: WidgetId) {
        self.ui.pending_close.push(id);
    }

    /// Close the popup this widget is (or is inside of).
    pub fn close_own_popup(&mut self) {
        if let Some(p) = self.ui.popup_containing(self.id) {
            self.ui.pending_close.push(p);
        }
    }

    /// Show a transient notification (§21.16 toast).
    pub fn toast(&mut self, text: &str, severity: crate::overlay::Severity) {
        let _ = self.ui.toast(text, severity);
    }

    /// Enable the IME while this widget edits text (an inline rename in a list).
    pub fn set_ime_allowed(&mut self, on: bool) {
        self.ui.ime.allowed = on;
    }

    /// Move keyboard focus to another widget.
    pub fn focus(&mut self, id: WidgetId, by_keyboard: bool) {
        self.ui.set_focus(Some(id), by_keyboard);
    }

    /// The layout rect of another widget.
    pub fn rect_of(&self, id: WidgetId) -> Option<Rect> {
        self.ui.rect(id)
    }

    /// Scroll a scroll viewport; the offset is clamped to its content.
    pub fn set_scroll(&mut self, viewport: WidgetId, offset: Point) {
        self.ui.set_scroll(viewport, offset);
    }

    /// A scroll viewport's state.
    pub fn scroll_state(&self, viewport: WidgetId) -> Option<crate::ui::ScrollState> {
        self.ui.scroll_state(viewport)
    }

    /// Build UI from an event (a popover's content, a dialog's buttons). The new widget
    /// joins the retained tree like any other; nothing is rebuilt.
    pub fn add_widget(
        &mut self,
        parent: WidgetId,
        key: impl Into<crate::id::Key>,
        style: crate::layout::NodeStyle,
        widget: impl Widget,
    ) -> Option<WidgetId> {
        self.ui.add(parent, key, style, widget).ok()
    }

    /// Restyle a widget this composite owns (a splitter resizing its panes).
    pub fn set_style(&mut self, id: WidgetId, style: crate::layout::NodeStyle) {
        let _ = self.ui.set_style(id, style);
    }

    /// Mark another widget dirty (a composite whose popup shows its state).
    pub fn invalidate(&mut self, id: WidgetId, d: Dirty) {
        self.ui.invalidate(id, d);
    }

    /// The children of a widget (a toolbar roving focus among its buttons).
    pub fn children_of(&self, id: WidgetId) -> Vec<WidgetId> {
        self.ui.children(id)
    }

    /// The widget holding keyboard focus.
    pub fn focused(&self) -> Option<WidgetId> {
        self.ui.focused()
    }

    /// Whether a widget takes focus (and is shown).
    pub fn is_focusable(&self, id: WidgetId) -> bool {
        self.ui.is_focusable(id)
    }

    /// Remove a widget this composite owns (deferred like a popup close).
    pub fn remove_widget(&mut self, id: WidgetId) {
        self.ui.pending_close.push(id);
    }

    /// Typed access to another widget (a composite reading its parts).
    pub fn widget_mut<W: Widget>(&mut self, id: WidgetId) -> Option<&mut W> {
        self.ui.widget_mut::<W>(id)
    }

    /// The window size in logical px.
    pub fn window_size(&self) -> Size {
        self.ui.size()
    }

    /// Paint a widget this composite owns above its siblings (a drop preview).
    pub fn bring_to_front(&mut self, id: WidgetId) {
        let _ = self.ui.bring_to_front(id);
    }

    /// The parent of a widget (finding which panel holds focus).
    pub fn parent_of(&self, id: WidgetId) -> Option<WidgetId> {
        self.ui.parent(id)
    }

    /// The AccessKit role of another widget (the keymap's focused-widget context).
    pub fn role_of(&self, id: WidgetId) -> Option<Role> {
        self.ui.role(id)
    }

    /// The key another widget was added under (a dock panel frame's `panel:<id>`).
    pub fn key_of(&self, id: WidgetId) -> Option<crate::id::Key> {
        self.ui.key_of(id).cloned()
    }

    /// The last pointer position in this window (logical px): where a `DragOver` is.
    pub fn pointer_pos(&self) -> Option<Point> {
        self.ui.pointer()
    }

    /// The drag in flight, if any (a drag source checks at `PointerUp` whether a target
    /// accepted it, before the drop is delivered).
    pub fn drag(&self) -> Option<&crate::dnd::DragSession> {
        self.ui.drag()
    }

    /// Where this window's client area is on the desktop (logical px), and its monitor.
    pub fn window_origin(&self) -> (Point, Option<String>) {
        (
            self.ui.window_origin(),
            self.ui.monitor().map(str::to_string),
        )
    }

    /// Answer a `DragOver`.
    pub fn set_drop_verdict(&mut self, v: DropVerdict) {
        let id = self.id;
        if let Some(d) = self.ui.drag.as_mut() {
            d.over = Some((id, v));
        }
    }
}

impl A11yCx<'_> {
    /// AccessKit bounds (physical pixels) for a logical rect (virtual children).
    pub fn bounds(&self, r: Rect) -> accesskit::Rect {
        let s = f64::from(self.scale);
        accesskit::Rect {
            x0: f64::from(r.x) * s,
            y0: f64::from(r.y) * s,
            x1: f64::from(r.right()) * s,
            y1: f64::from(r.bottom()) * s,
        }
    }
}

impl PaintCx<'_> {
    /// Draw an already-shaped run on an explicit background (list rows, table cells:
    /// shape once for the size, draw the same run).
    pub fn run_on(&mut self, run: GlyphRunId, origin: Point, fg: ColorRole, bg: ColorRole) {
        let saved = self.own_bg;
        self.own_bg = Some(bg);
        self.run(run, origin, fg);
        self.own_bg = saved;
    }
}
