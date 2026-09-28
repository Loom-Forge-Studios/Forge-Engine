//! The `ui_gallery` content (WP-U1 starts it; WP-U2 completes it with the full
//! catalogue). Built by the `ui_gallery` example and by the guard tests, so the tests
//! walk exactly what a user sees.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use accesskit::Role;

use crate::UiError;
use crate::damage::{LiveCell, LiveFeed};
use crate::geom::Size;
use crate::id::{Key, WidgetId};
use crate::layout::{FocusScope, NodeStyle};
use crate::state::Signal;
use crate::style::{ColorRole, Radius, Variant};
use crate::ui::{OpenWindow, Ui};
use crate::widgets::{
    Button, Checkbox, Container, IconButton, Label, LabelKind, LiveReadout, RadioGroup, Slider,
    Spinner, Tabs, TextField,
};

forge_trace::control_switches! {
    /// Fault switches the guards' positive controls use. All off for the real gallery.
    #[derive(Clone, Copy, Debug, Default)]
    pub struct GalleryFaults {
        /// Build the catalogue without the widget of this gallery name (the
        /// `test_ui_widgets_catalogue` completeness control).
        pub omit_widget: Option<&'static str>,
        /// Add an icon button with no accessible name (`test_ui_a11y_tree` control).
        pub unlabelled_icon_button: bool,
        /// The jobs spinner keeps animating while its tab is hidden (`ui_idle_zero_redraw`).
        pub spinner_ignores_visibility: bool,
        /// The text field applies typed text one frame late (`ui_typing_latency_one_frame`).
        pub field_defers_update: bool,
        /// The text field writes pre-edit text to the model (`test_ui_ime_composition`).
        pub field_commits_preedit_early: bool,
        /// A focusable widget that Tab never reaches because it is not focusable when it
        /// should be (`test_ui_focus_traversal` control): the checkbox is built as a label.
        pub checkbox_unfocusable: bool,
        /// The catalogue's curve editor asks for another animation frame on every event it
        /// sees, so it never goes idle (`test_ui_widgets_catalogue` idle control).
        pub curve_never_idles: bool,
        /// The catalogue's document-tabs box is wrongly declared a focus scope, so Tab enters
        /// it and never leaves (`test_ui_widgets_catalogue` keyboard control).
        pub doc_tabs_box_traps_tab: bool,
    }
}

/// Handles to the gallery's widgets and state.
pub struct Gallery {
    pub root: WidgetId,
    pub panels: Vec<WidgetId>,
    pub save: WidgetId,
    pub cancel: WidgetId,
    pub close_icon: WidgetId,
    pub text_field: WidgetId,
    pub name: Signal<String>,
    pub checkbox: WidgetId,
    pub snap: Signal<bool>,
    pub slider: WidgetId,
    pub opacity: Signal<f32>,
    pub radio: WidgetId,
    pub space: Signal<usize>,
    pub tabs: WidgetId,
    pub tab: Signal<usize>,
    /// Tab pages: General, Jobs, Live.
    pub pages: Vec<WidgetId>,
    pub spinner: WidgetId,
    pub jobs_live: Signal<bool>,
    pub readout: WidgetId,
    pub live_cell: Arc<LiveCell>,
    /// The raw value behind the readout (latency in µs); the readout shows whole ms.
    pub latency_us: Arc<AtomicU64>,
    /// Every widget a keyboard user must be able to reach, in visual order.
    pub interactive: Vec<WidgetId>,
    /// The §21.16 catalogue (WP-U2): one tab page per group.
    pub catalogue: crate::gallery_catalogue::Catalogue,
}

/// Build the gallery under the root of `ui`.
pub fn build(ui: &mut Ui, faults: GalleryFaults) -> Result<Gallery, UiError> {
    let space = ui.theme().space;
    let root = ui.add(
        ui.root(),
        "gallery",
        NodeStyle::column(space[3])
            .fill()
            .padding(space[4])
            .align_start(),
        Container::group().labelled("forge-ui gallery"),
    )?;
    ui.add(
        root,
        "title",
        NodeStyle::leaf(),
        Label::new("forge-ui gallery").heading(),
    )?;
    ui.add(
        root,
        "subtitle",
        NodeStyle::leaf(),
        Label::new("Retained, damage-tracked, zero redraw when idle. Tab / Shift+Tab to move, F6 between panels.")
            .kind(LabelKind::Muted),
    )?;

    let panel = |ui: &mut Ui, key: &'static str, label: &str| -> Result<WidgetId, UiError> {
        ui.add(
            root,
            key,
            NodeStyle::row(space[2])
                .padding(space[3])
                .wrap()
                .background(ColorRole::BgRaised)
                .rounded(Radius::Lg)
                .scope(FocusScope::Panel),
            Container::pane(label),
        )
    };

    // Buttons.
    let buttons = panel(ui, "buttons", "Buttons")?;
    let save = ui.add(
        buttons,
        "save",
        NodeStyle::leaf(),
        Button::new("Save").primary(),
    )?;
    let cancel = ui.add(buttons, "cancel", NodeStyle::leaf(), Button::new("Cancel"))?;
    let more = ui.add(
        buttons,
        "more",
        NodeStyle::leaf(),
        Button::new("More…").variant(Variant::Ghost),
    )?;
    let delete = ui.add(
        buttons,
        "delete",
        NodeStyle::leaf(),
        Button::new("Delete").variant(Variant::Danger),
    )?;
    let close_icon = ui.add(
        buttons,
        "close",
        NodeStyle::leaf(),
        IconButton::new("✕", "Close"),
    )?;
    let settings = ui.add(
        buttons,
        "settings",
        NodeStyle::leaf(),
        IconButton::new("⚙", "Settings"),
    )?;
    let new_window = ui.add(
        buttons,
        "new_window",
        NodeStyle::leaf(),
        Button::new("New window").on_press(|cx| {
            cx.action(OpenWindow {
                key: 1,
                title: "forge-ui gallery (second window)".into(),
                size: Size::new(640.0, 420.0),
                position: None,
                monitor: None,
            })
        }),
    )?;
    let mut interactive = vec![save, cancel, more, delete, close_icon, settings, new_window];
    if faults.unlabelled_icon_button() {
        let bad = ui.add(
            buttons,
            "unlabelled",
            NodeStyle::leaf(),
            IconButton::unlabelled("★"),
        )?;
        interactive.push(bad);
    }

    // The Forge icon set (WP-U12): every icon, named.
    let icons = panel(ui, "icons", "Icons")?;
    for icon in crate::icons::Icon::ALL {
        ui.add(
            icons,
            Key::Str(icon.name().into()),
            NodeStyle::leaf(),
            crate::widgets::IconView::new(icon, icon.name()),
        )?;
    }

    // Inputs.
    let inputs = panel(ui, "inputs", "Inputs")?;
    let name = ui.rt_mut().signal(String::new());
    let field = with_field_faults(
        TextField::new(name, "Name").placeholder("Entity name"),
        faults,
    );
    let text_field = ui.add(inputs, "name", NodeStyle::leaf().width(220.0), field)?;
    let snap = ui.rt_mut().signal(true);
    let checkbox = if faults.checkbox_unfocusable() {
        ui.add(
            inputs,
            "snap",
            NodeStyle::leaf(),
            Label::new("Snap to grid"),
        )?
    } else {
        ui.add(
            inputs,
            "snap",
            NodeStyle::leaf(),
            Checkbox::new(snap, "Snap to grid"),
        )?
    };
    let opacity = ui.rt_mut().signal(0.8f32);
    let slider = ui.add(
        inputs,
        "opacity",
        NodeStyle::leaf().width(200.0),
        Slider::new(opacity, "Opacity", 0.0, 1.0, 0.05),
    )?;
    interactive.extend([text_field, checkbox, slider]);

    // Choices and tabs.
    let choices = panel(ui, "choices", "Choices")?;
    let space_sel = ui.rt_mut().signal(1usize);
    let radio = ui.add(
        choices,
        "space",
        NodeStyle::leaf(),
        RadioGroup::new("Gizmo space", &["Local", "World", "Frame"], space_sel),
    )?;
    let tabs_box = ui.add(
        choices,
        "tabs_box",
        NodeStyle::column(space[2]),
        Container::group(),
    )?;
    let tab = ui.rt_mut().signal(0usize);
    let tabs = ui.add(
        tabs_box,
        "tabs",
        NodeStyle::leaf(),
        Tabs::new(&["General", "Jobs", "Live"], tab),
    )?;
    let pages_box = ui.add(
        tabs_box,
        "pages",
        NodeStyle::column(space[2]).min_size(260.0, 60.0),
        Container::group(),
    )?;
    let p0 = ui.add(
        pages_box,
        Key::Index(0),
        NodeStyle::column(space[1]),
        Container::new(Role::TabPanel).labelled("General"),
    )?;
    ui.add(
        p0,
        "about",
        NodeStyle::leaf().width(260.0),
        Label::new("Mixed scripts shape and wrap: 漢字かな交じり文, مرحبا بالعالم, emoji 🎨✨.")
            .wrapping(),
    )?;
    let p1 = ui.add(
        pages_box,
        Key::Index(1),
        NodeStyle::row(space[2]),
        Container::new(Role::TabPanel).labelled("Jobs"),
    )?;
    let jobs_live = ui.rt_mut().signal(true);
    let spinner = with_spinner_faults(Spinner::new(jobs_live, "Baking lightmaps"), faults);
    let spinner = ui.add(p1, "spinner", NodeStyle::leaf(), spinner)?;
    ui.add(
        p1,
        "jobs_label",
        NodeStyle::leaf(),
        Label::new("Baking lightmaps…").muted(),
    )?;
    let p2 = ui.add(
        pages_box,
        Key::Index(2),
        NodeStyle::column(space[1]),
        Container::new(Role::TabPanel).labelled("Live"),
    )?;
    let latency_us = Arc::new(AtomicU64::new(12_300));
    let shown = ui.rt_mut().signal(String::from("12 ms"));
    let lat = latency_us.clone();
    let readout = ui.add(
        p2,
        "latency",
        NodeStyle::leaf(),
        LiveReadout::new("Remote latency", shown, move || {
            format!("{} ms", lat.load(Ordering::Relaxed) / 1000)
        }),
    )?;
    let live_cell = LiveCell::new();
    let feed = ui.add_feed(
        readout,
        LiveFeed {
            source: live_cell.clone(),
            max_hz: 2,
            self_ui: false,
        },
    );
    if let Some(r) = ui.widget_mut::<LiveReadout>(readout) {
        r.set_feed(feed);
    }
    let pages = vec![p0, p1, p2];
    if let Some(t) = ui.widget_mut::<Tabs>(tabs) {
        t.set_pages(pages.clone());
    }
    ui.set_hidden(p1, true)?;
    ui.set_hidden(p2, true)?;
    interactive.extend([radio, tabs]);

    // Text samples.
    let samples = panel(ui, "samples", "Text samples")?;
    ui.add(
        samples,
        "mono",
        NodeStyle::leaf(),
        Label::new("let pos: FramePos = frame.local(1.5, 0.0, -2.25);").kind(LabelKind::Mono),
    )?;
    ui.add(
        samples,
        "small",
        NodeStyle::leaf(),
        Label::new("Small print, muted.").kind(LabelKind::Small),
    )?;

    // The §21.16 catalogue (WP-U2).
    let catalogue = crate::gallery_catalogue::build(ui, root, faults)?;
    // The catalogue's tab stops on its first page, in visual order.
    interactive.push(catalogue.tabs);
    interactive.push(catalogue.scroll);
    if let Some(rt) = catalogue.ids.get("rich_text") {
        interactive.push(*rt);
    }

    Ok(Gallery {
        root,
        panels: vec![buttons, inputs, choices, samples, catalogue.panel],
        catalogue,
        save,
        cancel,
        close_icon,
        text_field,
        name,
        checkbox,
        snap,
        slider,
        opacity,
        radio,
        space: space_sel,
        tabs,
        tab,
        pages,
        spinner,
        jobs_live,
        readout,
        live_cell,
        latency_us,
        interactive,
    })
}

/// The gallery's text field, with the typing and IME controls' faults in test builds.
fn with_field_faults(field: TextField, faults: GalleryFaults) -> TextField {
    #[cfg(any(test, feature = "controls"))]
    {
        let mut field = field;
        if faults.field_defers_update {
            field = field.with_fault_defer_update();
        }
        if faults.field_commits_preedit_early {
            field = field.with_fault_commit_preedit_early();
        }
        field
    }
    #[cfg(not(any(test, feature = "controls")))]
    {
        let _ = faults;
        field
    }
}

/// The gallery's jobs spinner, with the idle control's fault in test builds.
fn with_spinner_faults(spinner: Spinner, faults: GalleryFaults) -> Spinner {
    #[cfg(any(test, feature = "controls"))]
    if faults.spinner_ignores_visibility {
        return spinner.with_fault_ignore_visibility();
    }
    let _ = faults;
    spinner
}
