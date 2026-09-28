//! The `EditorOverlay` and `Theme` extension points (Ch.21 §21.19).
//!
//! **`EditorOverlay`** (`forge.editor.overlay`): things drawn over the whole editor rather
//! than inside a panel — toasts, the play-in-editor banner, presence badges and drag
//! previews (Ch.37). The shell builds every registered overlay once into the main window's
//! overlay layer (above the dock, pointer-transparent) with the same [`PanelCx`] a panel
//! gets: the mirror (read), the command emitter (write), session state, sync steps. So a
//! plugin can add an overlay, replace the toasts with its own, chain the play banner or
//! remove it, exactly as it can a panel (I16).
//!
//! **`Theme`** (`forge.ui.theme`): a token set (§21.8). The three built-in themes are
//! registered through the point by the editor's own plugin; a plugin can add a theme (it
//! appears in View → Theme and the palette), replace a built-in one's tokens, chain one
//! (adjust a token, keep the rest) or remove one. The point's item is `forge_ui::Theme`;
//! the point itself is defined here, in `forge-editor`, because `forge-ui` must not depend
//! on the plugin kernel (games link `forge-ui` without `forge-cmd`, §21.20) — ADR 0029.

use std::sync::Arc;

use forge_plugin::ExtensionPoint;
use forge_plugin::points::BuildFn;

use crate::panels::PanelCx;

/// One overlay (the `EditorOverlay` point's item). Its key is its id
/// (`forge.overlay.toasts`). Generic over the build context like `EditorPanel` (the editor
/// uses its `PanelCx`; a conformance kit uses its own).
pub struct OverlayDescriptor<Cx = PanelCx> {
    pub title: String,
    /// Paint order among overlays: higher is drawn above.
    pub order: i32,
    /// Builds the overlay's widgets under the overlay layer (absolute positions; the layer
    /// passes the pointer through, so only an overlay's own interactive widgets take it).
    pub build: BuildFn<Cx>,
}

impl<Cx> Clone for OverlayDescriptor<Cx> {
    fn clone(&self) -> Self {
        Self {
            title: self.title.clone(),
            order: self.order,
            build: Arc::clone(&self.build),
        }
    }
}

impl<Cx> std::fmt::Debug for OverlayDescriptor<Cx> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OverlayDescriptor")
            .field("title", &self.title)
            .field("order", &self.order)
            .finish_non_exhaustive()
    }
}

/// The `EditorOverlay` point (`forge.editor.overlay`).
pub struct EditorOverlay<Cx = PanelCx>(std::marker::PhantomData<fn(&mut Cx)>);

impl<Cx: 'static> ExtensionPoint for EditorOverlay<Cx> {
    type Item = OverlayDescriptor<Cx>;
    const ID: &'static str = "forge.editor.overlay";
    const NAME: &'static str = "EditorOverlay";
}

/// One theme (the `Theme` point's item). Its key is the theme id (`forge.dark`).
#[derive(Clone, Debug)]
pub struct ThemeItem {
    /// Shown in View → Theme.
    pub label: String,
    /// The token set.
    pub tokens: Arc<forge_ui::Theme>,
}

impl ThemeItem {
    pub fn new(label: &str, tokens: forge_ui::Theme) -> Self {
        Self {
            label: label.to_string(),
            tokens: Arc::new(tokens),
        }
    }
}

/// The `Theme` point (`forge.ui.theme`).
pub struct ThemePoint;

impl ExtensionPoint for ThemePoint {
    type Item = ThemeItem;
    const ID: &'static str = "forge.ui.theme";
    // l10n: the extension point's identifier
    const NAME: &'static str = "Theme";
}

/// The built-in themes, `(id, item)` — registered by the editor's own plugin.
pub fn builtin_themes() -> Vec<(&'static str, ThemeItem)> {
    vec![
        (
            "forge.dark",
            ThemeItem::new(forge_ui::tr_key!("Dark"), forge_ui::Theme::dark()),
        ),
        (
            "forge.light",
            ThemeItem::new(forge_ui::tr_key!("Light"), forge_ui::Theme::light()),
        ),
        (
            "forge.high_contrast",
            ThemeItem::new(
                forge_ui::tr_key!("High contrast"),
                forge_ui::Theme::high_contrast(),
            ),
        ),
    ]
}

// ---- the first-party overlays -------------------------------------------------------------

/// The toasts overlay (`forge.overlay.toasts`): every notification posted to the session
/// becomes a toast once, in order, code first. A plugin that replaces it routes
/// notifications elsewhere; removing it leaves the notification centre's history alone.
pub fn toasts_overlay(cx: &mut PanelCx) {
    cx.add_live(|pb| {
        let seen = std::rc::Rc::new(std::cell::Cell::new(0u64));
        let owner = pb.parent;
        pb.sync(owner, move |sy| {
            for n in sy.session.notifications.after(seen.get()) {
                let sev = match n.level {
                    crate::notify::Level::Info => forge_ui::overlay::Severity::Info,
                    crate::notify::Level::Success => forge_ui::overlay::Severity::Success,
                    crate::notify::Level::Warning => forge_ui::overlay::Severity::Warning,
                    crate::notify::Level::Error => forge_ui::overlay::Severity::Error,
                };
                let _ = sy.ui.toast(&n.toast_text(), sev);
                seen.set(n.id);
            }
            Ok(())
        });
        Ok(())
    });
}

/// What the play banner says for a backend's state (`None`: nothing is playing).
pub fn play_banner_text(state: crate::play::PlayState, tick: forge_frames::Tick) -> Option<String> {
    let secs = tick.0 as f64 / forge_frames::TICKS_PER_SECOND as f64;
    match state {
        crate::play::PlayState::Stopped => None,
        crate::play::PlayState::Playing => Some(forge_ui::trf!(
            "\u{25b6} Playing in your sandbox \u{00b7} {secs} s \u{2014} the project is untouched; Stop discards the run",
            secs = format!("{secs:.2}")
        )),
        crate::play::PlayState::Paused => Some(forge_ui::trf!(
            "\u{23f8} Paused in your sandbox \u{00b7} {secs} s \u{2014} Step advances one tick; Stop discards the run",
            secs = format!("{secs:.2}")
        )),
    }
}

/// The play banner overlay (`forge.overlay.play_banner`): while a play session exists, a
/// strip at the top of the window says so — the sandbox is not the project (Ch.37 §37.5).
pub fn play_banner_overlay(cx: &mut PanelCx) {
    cx.add_live(|pb| {
        let text = pb.b.signal(String::new());
        let theme_space = pb.b.theme_ref().space[2];
        // A strip across the top of the window, below the menu and toolbar.
        let inset = theme_space * 24.0;
        let style =
            forge_ui::NodeStyle::absolute(Some(inset), Some(theme_space * 10.0), Some(inset), None)
                .padding(theme_space)
                .background(forge_ui::ColorRole::BgRaised)
                .rounded(forge_ui::style::Radius::Md)
                .no_hit_test();
        let banner = pb.b.add(
            pb.parent,
            "play_banner",
            style,
            forge_ui::widgets::Label::new(text).kind(forge_ui::widgets::LabelKind::Heading),
        )?;
        pb.b.hide(banner, true);
        let seen = std::rc::Rc::new(std::cell::Cell::new(u64::MAX));
        pb.sync(banner, move |sy| {
            let (rev, state, tick) = {
                let p = sy.services.play.borrow();
                (p.revision(), p.state(), p.tick())
            };
            if seen.get() == rev {
                return Ok(());
            }
            seen.set(rev);
            match play_banner_text(state, tick) {
                Some(t) => {
                    text.set(sy.ui.rt_mut(), t);
                    let _ = sy.ui.set_hidden(banner, false);
                }
                None => {
                    let _ = sy.ui.set_hidden(banner, true);
                }
            }
            Ok(())
        });
        Ok(())
    });
}

/// The first-party overlays, `(id, item)`, registered by the editor's own plugin.
pub fn builtin_overlays() -> Vec<(&'static str, OverlayDescriptor)> {
    vec![
        (
            "forge.overlay.toasts",
            OverlayDescriptor {
                title: forge_ui::tr_key!("Toasts").into(),
                order: 100,
                build: Arc::new(toasts_overlay),
            },
        ),
        (
            "forge.overlay.play_banner",
            OverlayDescriptor {
                title: forge_ui::tr_key!("Play-in-editor banner").into(),
                order: 50,
                build: Arc::new(play_banner_overlay),
            },
        ),
    ]
}
