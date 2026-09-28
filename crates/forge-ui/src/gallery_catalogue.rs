//! The WP-U2 half of `ui_gallery`: every §21.16 catalogue widget, one tab page per
//! catalogue group, in a scroll area. Built by [`crate::gallery::build`] and walked by
//! `tests/test_ui_widgets_catalogue.rs` exactly as a user sees it.
//!
//! The node canvas is WP-U8's (its "Graph" page); the dock tab strip is WP-U3's (the dock).

use std::collections::BTreeMap;
use std::rc::Rc;

use accesskit::Role;
use forge_frames::{DQuat, FrameId, FramePos};

use crate::UiError;
use crate::color::HdrColor;
use crate::geom::{Point, Rect};
use crate::id::{Key, WidgetId};
use crate::layout::{FocusScope, NodeStyle};
use crate::state::Signal;
use crate::style::{ColorRole, Radius, Variant};
use crate::ui::{ActionEnvelope, Ui};
use crate::widgets::*;

/// Catalogue groups, in tab order (§21.16's table).
pub const GROUPS: [&str; 12] = [
    "Text & display",
    "Buttons & toggles",
    "Numeric",
    "Text entry",
    "Choice",
    "Collections",
    "Containers",
    "Menus & overlays",
    "Feedback",
    "Editors",
    "Navigation",
    "Graph",
];

/// Handles to the catalogue.
pub struct Catalogue {
    pub panel: WidgetId,
    pub tabs: WidgetId,
    pub tab: Signal<usize>,
    /// The scroll area the pages sit in (a tab stop).
    pub scroll: WidgetId,
    /// One page per group.
    pub pages: Vec<WidgetId>,
    /// Every catalogue widget by name.
    pub ids: BTreeMap<&'static str, WidgetId>,
    pub color: Signal<HdrColor>,
    pub rotation: Signal<DQuat>,
    pub position: Signal<FramePos>,
    pub curve: Signal<Curve>,
    pub gradient: Signal<Gradient>,
    pub texture: Signal<Option<String>>,
}

struct B<'a> {
    ui: &'a mut Ui,
    ids: BTreeMap<&'static str, WidgetId>,
    /// `GalleryFaults::omit_widget`: this catalogue widget is not built at all.
    omit: Option<&'static str>,
}

impl B<'_> {
    fn add(
        &mut self,
        parent: WidgetId,
        name: &'static str,
        style: NodeStyle,
        w: impl crate::Widget,
    ) -> Result<WidgetId, UiError> {
        if self.omit == Some(name) {
            // The completeness control: the widget is never added to the tree and never
            // recorded. Callers that ignore the id get the page instead.
            return Ok(parent);
        }
        let id = self.ui.add(parent, Key::Static(name), style, w)?;
        self.ids.insert(name, id);
        Ok(id)
    }
    fn sig<T: PartialEq + 'static>(&mut self, v: T) -> Signal<T> {
        self.ui.rt_mut().signal(v)
    }
}

fn page(ui: &mut Ui, parent: WidgetId, i: usize) -> Result<WidgetId, UiError> {
    let space = ui.theme().space;
    ui.add(
        parent,
        Key::Index(i as u32),
        NodeStyle::row(space[3])
            .wrap()
            .align_start()
            .padding(space[2]),
        Container::new(Role::TabPanel).labelled(GROUPS[i]),
    )
}

/// Build the catalogue panel under `parent`. `faults` is all-off for the real gallery;
/// only the guards' positive controls switch its catalogue faults on.
pub fn build(
    ui: &mut Ui,
    parent: WidgetId,
    faults: crate::gallery::GalleryFaults,
) -> Result<Catalogue, UiError> {
    let space = ui.theme().space;
    let panel = ui.add(
        parent,
        "catalogue",
        NodeStyle::column(space[2])
            .padding(space[3])
            .background(ColorRole::BgRaised)
            .rounded(Radius::Lg)
            .scope(FocusScope::Panel),
        Container::pane("Widget catalogue"),
    )?;
    let tab = ui.rt_mut().signal(0usize);
    let tabs = ui.add(panel, "groups", NodeStyle::leaf(), Tabs::new(&GROUPS, tab))?;
    let sa = scroll_area(
        ui,
        panel,
        "scroll",
        NodeStyle::default().size(1180.0, 330.0),
        "Catalogue page",
    )?;
    let mut pages = Vec::new();
    for i in 0..GROUPS.len() {
        pages.push(page(ui, sa.viewport, i)?);
    }
    if let Some(t) = ui.widget_mut::<Tabs>(tabs) {
        t.set_pages(pages.clone());
    }
    for p in pages.iter().skip(1) {
        ui.set_hidden(*p, true)?;
    }
    let mut b = B {
        ui,
        ids: BTreeMap::new(),
        omit: faults.omit_widget(),
    };

    // 0. Text & display.
    let p = pages[0];
    b.add(p, "label", NodeStyle::leaf(), Label::new("A label"))?;
    b.add(
        p,
        "rich_text",
        NodeStyle::leaf().width(300.0),
        RichText::new(vec![
            Span::plain("Rich text with "),
            Span::strong("bold"),
            Span::plain(", "),
            Span::code("code"),
            Span::plain(", an icon "),
            Span::icon("★", "star"),
            Span::plain(" and a "),
            Span::link("link", "forge://docs/ui"),
            Span::plain("."),
        ]),
    )?;
    b.add(p, "icon", NodeStyle::leaf(), Icon::new("⚙", "Settings"))?;
    b.add(
        p,
        "separator",
        NodeStyle::leaf().height(28.0),
        Separator::vertical(),
    )?;
    b.add(p, "badge", NodeStyle::leaf(), Badge::new("12"))?;
    b.add(
        p,
        "shortcut",
        NodeStyle::leaf(),
        ShortcutHint::new("Ctrl+Shift+P"),
    )?;
    let px: Vec<u8> = (0..16 * 16)
        .flat_map(|i| {
            let on = ((i % 16) / 4 + (i / 16) / 4) % 2 == 0;
            if on {
                [200, 120, 40, 255]
            } else {
                [40, 90, 160, 255]
            }
        })
        .collect();
    let img = b.ui.add_image(16, 16, px)?;
    b.add(
        p,
        "image",
        NodeStyle::leaf(),
        ImageView::new(img, crate::geom::Size::new(32.0, 32.0), "Checker thumbnail"),
    )?;

    // 1. Buttons & toggles.
    let p = pages[1];
    b.add(
        p,
        "btn_primary",
        NodeStyle::leaf(),
        Button::new("Primary").primary(),
    )?;
    b.add(
        p,
        "btn_secondary",
        NodeStyle::leaf(),
        Button::new("Secondary"),
    )?;
    b.add(
        p,
        "btn_ghost",
        NodeStyle::leaf(),
        Button::new("Ghost").variant(Variant::Ghost),
    )?;
    b.add(
        p,
        "btn_danger",
        NodeStyle::leaf(),
        Button::new("Danger").variant(Variant::Danger),
    )?;
    b.add(
        p,
        "btn_small",
        NodeStyle::leaf(),
        Button::new("Small").size(ButtonSize::Small),
    )?;
    b.add(
        p,
        "btn_large",
        NodeStyle::leaf(),
        Button::new("Large").size(ButtonSize::Large),
    )?;
    b.add(
        p,
        "icon_button",
        NodeStyle::leaf(),
        IconButton::new("✎", "Edit"),
    )?;
    let s = b.sig(false);
    b.add(
        p,
        "toggle_button",
        NodeStyle::leaf(),
        ToggleButton::new(s, "Snap"),
    )?;
    let s = b.sig(CheckState::Mixed);
    b.add(
        p,
        "tri_checkbox",
        NodeStyle::leaf(),
        TriCheckbox::new(s, "All layers"),
    )?;
    let s = b.sig(true);
    b.add(
        p,
        "checkbox",
        NodeStyle::leaf(),
        Checkbox::new(s, "Cast shadows"),
    )?;
    let s = b.sig(0usize);
    b.add(
        p,
        "radio",
        NodeStyle::leaf(),
        RadioGroup::new("Units", &["Metric", "Imperial"], s),
    )?;
    let s = b.sig(true);
    b.add(p, "switch", NodeStyle::leaf(), Switch::new(s, "Autosave"))?;
    let s = b.sig(1usize);
    b.add(
        p,
        "segmented",
        NodeStyle::leaf(),
        SegmentedControl::new("View", &["2D", "3D", "Split"], s),
    )?;

    // 2. Numeric.
    let p = pages[2];
    let s = b.sig(0.5f32);
    b.add(
        p,
        "slider",
        NodeStyle::leaf().width(180.0),
        Slider::new(s, "Roughness", 0.0, 1.0, 0.01),
    )?;
    let s = b.sig(10.0f32);
    b.add(
        p,
        "slider_log",
        NodeStyle::leaf().width(180.0),
        Slider::new(s, "Distance", 0.01, 1000.0, 0.01).log(),
    )?;
    let s = b.sig(12.5f64);
    b.add(
        p,
        "numeric",
        NodeStyle::leaf().width(130.0),
        NumericField::new(s, "Impulse").unit("N·s").decimals(2),
    )?;
    let s = b.sig(3.0f64);
    b.add(
        p,
        "spin_box",
        NodeStyle::leaf().width(110.0),
        NumericField::new(s, "Samples")
            .range(1.0, 64.0)
            .step(1.0)
            .decimals(0)
            .spin(),
    )?;
    let s = b.sig((20.0f32, 80.0f32));
    b.add(
        p,
        "range_slider",
        NodeStyle::leaf().width(200.0),
        RangeSlider::new(s, "LOD range", 0.0, 100.0, 1.0),
    )?;

    // 3. Text entry.
    let p = pages[3];
    let s = b.sig(String::from("Player"));
    b.add(
        p,
        "text_field",
        NodeStyle::leaf().width(180.0),
        TextField::new(s, "Name"),
    )?;
    let s = b.sig(String::new());
    b.add(
        p,
        "password",
        NodeStyle::leaf().width(160.0),
        TextField::new(s, "Password").password(),
    )?;
    let s = b.sig(String::from("fn main() {\n    println!(\"forge\");\n}\n"));
    b.add(
        p,
        "multiline",
        NodeStyle::leaf().size(300.0, 110.0),
        MultilineEditor::new(s, "Script").line_numbers().mono(),
    )?;
    let s = b.sig(String::new());
    b.add(
        p,
        "search",
        NodeStyle::leaf().width(180.0),
        SearchField::new(s, "Search assets"),
    )?;
    b.add(
        p,
        "path",
        NodeStyle::leaf().width(220.0),
        PathField::new("Output folder", "build/out"),
    )?;

    // 4. Choice.
    let p = pages[4];
    let s = b.sig(0usize);
    b.add(
        p,
        "combo",
        NodeStyle::leaf().width(170.0),
        ComboBox::new(
            "Blend mode",
            &["Opaque", "Masked", "Translucent", "Additive"],
            s,
        ),
    )?;
    let s = b.sig(vec![0usize, 2]);
    b.add(
        p,
        "chips",
        NodeStyle::leaf().width(300.0),
        Chips::new("Tags", &["terrain", "water", "foliage", "rock"], s),
    )?;
    let s = b.sig(String::new());
    b.add(
        p,
        "autocomplete",
        NodeStyle::leaf().width(200.0),
        Autocomplete::new(
            "Component",
            &["Transform", "Rigid body", "Collider", "Light", "Camera"],
            s,
        ),
    )?;

    // 5. Collections.
    let p = pages[5];
    let mut list = VirtualTree::list("Entities");
    list.extend((0..1000u64).map(|i| (i, RowItem::new(format!("Entity {i}")))));
    b.add(p, "list", NodeStyle::leaf().size(220.0, 280.0), list)?;
    let mut tree = VirtualTree::tree("Scene");
    for f in 0..20u64 {
        let fk = 10_000 + f * 100;
        tree.push(None, fk, RowItem::new(format!("Folder {f}")).icon("▣"));
        for c in 1..=20u64 {
            tree.push(Some(fk), fk + c, RowItem::new(format!("Node {f}.{c}")));
        }
    }
    tree.set_expanded(10_000, true);
    b.add(p, "tree", NodeStyle::leaf().size(240.0, 280.0), tree)?;
    b.add(
        p,
        "table",
        NodeStyle::leaf().size(560.0, 280.0),
        VirtualTable::new(
            "Assets",
            vec![
                Column::new("Name", 180.0),
                Column::new("Kind", 110.0),
                Column::new("Size (KiB)", 110.0).numeric(),
                Column::new("Refs", 80.0).numeric(),
            ],
            Box::new(DemoTable::new(1000)),
        ),
    )?;

    // 6. Containers.
    let p = pages[6];
    let sa2 = scroll_area(
        b.ui,
        p,
        "scroll_area",
        NodeStyle::default().size(180.0, 140.0),
        "Scrolled notes",
    )?;
    b.ids.insert("scroll_area", sa2.area);
    for i in 0..12u32 {
        b.ui.add(
            sa2.viewport,
            Key::Index(i),
            NodeStyle::leaf(),
            Label::new(format!("Line {i}")),
        )?;
    }
    let ratio = b.sig(0.4f32);
    let sp = splitter(
        b.ui,
        p,
        "splitter",
        NodeStyle::default().size(260.0, 140.0),
        Axis::Horizontal,
        ratio,
        "Split view",
    )?;
    b.ids.insert("splitter", sp.handle);
    b.ui.add(sp.first, "a", NodeStyle::leaf(), Label::new("Left pane"))?;
    b.ui.add(sp.second, "b", NodeStyle::leaf(), Label::new("Right pane"))?;
    let doc_tab = b.sig(0usize);
    let mut tab_box_style = NodeStyle::column(space[1]);
    if faults.doc_tabs_box_traps_tab() {
        tab_box_style = tab_box_style.scope(FocusScope::Panel);
    }
    let tab_box = b.add(p, "doc_tabs_box", tab_box_style, Container::group())?;
    let doc_tabs = b.add(
        tab_box,
        "doc_tabs",
        NodeStyle::leaf(),
        Tabs::new(&["main.rs", "lib.rs", "ui.rs"], doc_tab)
            .closable()
            .reorderable(),
    )?;
    let mut doc_pages = Vec::new();
    for (i, t) in ["main.rs", "lib.rs", "ui.rs"].iter().enumerate() {
        let dp = b.ui.add(
            tab_box,
            Key::Index(i as u32),
            NodeStyle::leaf(),
            Label::new(format!("Contents of {t}")),
        )?;
        if i > 0 {
            b.ui.set_hidden(dp, true)?;
        }
        doc_pages.push(dp);
    }
    if let Some(t) = b.ui.widget_mut::<Tabs>(doc_tabs) {
        t.set_pages(doc_pages);
    }
    let open = b.sig(true);
    let sec = b.add(
        p,
        "section_box",
        NodeStyle::column(space[1]),
        Container::group(),
    )?;
    let body_id = sec.child(&Key::Static("section_body"));
    let mut header = CollapsibleHeader::new("Physics", open);
    header.set_body(body_id);
    b.add(sec, "collapsible", NodeStyle::leaf().width(180.0), header)?;
    b.add(
        sec,
        "section_body",
        NodeStyle::column(space[1]),
        Container::group(),
    )?;
    b.ui.add(body_id, "mass", NodeStyle::leaf(), Label::new("Mass 12 kg"))?;
    let card = b.add(
        p,
        "card",
        NodeStyle::card(space[1], space[3]).background(ColorRole::BgSunken),
        Container::new(Role::Group).labelled("Card"),
    )?;
    b.ui.add(card, "t", NodeStyle::leaf(), Label::new("Card").heading())?;
    b.ui.add(
        card,
        "b",
        NodeStyle::leaf(),
        Label::new("A raised, rounded group.").muted(),
    )?;
    let gb = b.add(
        p,
        "group_box",
        GroupBox::style(b.ui.theme()),
        GroupBox::new("Group box"),
    )?;
    b.ui.add(gb, "l", NodeStyle::leaf(), Label::new("Grouped content"))?;
    let grid = b.add(
        p,
        "grid",
        NodeStyle::grid(3, space[1]).width(180.0),
        Container::new(Role::Group).labelled("Grid"),
    )?;
    for i in 0..6u32 {
        b.ui.add(
            grid,
            Key::Index(i),
            NodeStyle::leaf(),
            Badge::new(format!("{i}")),
        )?;
    }

    // 7. Menus & overlays.
    let p = pages[7];
    b.add(
        p,
        "menu_bar",
        NodeStyle::leaf(),
        MenuBar::new(vec![
            (
                "File",
                vec![
                    MenuItem::action("new", "New").shortcut("Ctrl+N"),
                    MenuItem::action("open", "Open…").shortcut("Ctrl+O"),
                    MenuItem::submenu("Recent", vec![MenuItem::action("r1", "island.forge")]),
                    MenuItem::action("quit", "Quit"),
                ],
            ),
            (
                "Edit",
                vec![
                    MenuItem::action("undo", "Undo").shortcut("Ctrl+Z"),
                    MenuItem::action("redo", "Redo")
                        .shortcut("Ctrl+Y")
                        .disabled(),
                ],
            ),
        ]),
    )?;
    b.add(
        p,
        "context_area",
        NodeStyle::leaf().size(160.0, 60.0),
        ContextMenuArea::new(
            "Right-click area",
            vec![
                MenuItem::action("cut", "Cut"),
                MenuItem::action("copy", "Copy"),
                MenuItem::action("paste", "Paste"),
            ],
        ),
    )?;
    b.add(
        p,
        "tooltip_button",
        NodeStyle::leaf(),
        IconButton::new("ⓘ", "Tooltips appear after half a second"),
    )?;
    b.add(
        p,
        "popover",
        NodeStyle::leaf(),
        PopoverButton::new("Popover", "A non-modal panel anchored to its button."),
    )?;
    b.add(
        p,
        "dialog_button",
        NodeStyle::leaf(),
        Button::new("Delete layer…")
            .variant(Variant::Danger)
            .on_press(|cx| {
                if let Ok(spec) = DialogSpec::destructive(
                    "delete-layer",
                    "Delete layer?",
                    "The layer's 12 painted tiles will be lost.",
                    "Delete",
                ) {
                    let owner = cx.id();
                    let _ = open_dialog(cx.ui, owner, spec);
                }
            }),
    )?;
    b.add(
        p,
        "toast_button",
        NodeStyle::leaf(),
        Button::new("Show toast")
            .on_press(|cx| cx.toast("Saved island.forge", crate::overlay::Severity::Success)),
    )?;

    // 8. Feedback.
    let p = pages[8];
    let s = b.sig(Progress::Fraction(0.6));
    b.add(
        p,
        "progress",
        NodeStyle::leaf().width(180.0),
        ProgressBar::new(s, "Import"),
    )?;
    let s = b.sig(Progress::Indeterminate);
    b.add(
        p,
        "progress_busy",
        NodeStyle::leaf().width(180.0),
        ProgressBar::new(s, "Compiling shaders"),
    )?;
    let s = b.sig(true);
    b.add(p, "spinner", NodeStyle::leaf(), Spinner::new(s, "Loading"))?;
    b.add(
        p,
        "skeleton",
        NodeStyle::leaf().width(180.0),
        Skeleton::new(3, "Loading details"),
    )?;
    b.add(
        p,
        "empty_state",
        NodeStyle::leaf().width(240.0),
        EmptyState::new("No assets yet").action("Import…"),
    )?;

    // 9. Editors.
    let p = pages[9];
    let color = b.sig(HdrColor::linear(4.0, 2.0, 0.5, 1.0));
    b.add(
        p,
        "color_button",
        NodeStyle::leaf().width(80.0),
        ColorButton::new(color, "Emission"),
    )?;
    let pk = color_picker(b.ui, p, "color_picker", color, "Emission")?;
    b.ids.insert("color_picker", pk.picker);
    let v = b.sig([1.0f64, 2.0, 3.0]);
    let ve = vector_editor::<3>(b.ui, p, "vector", v, "Scale", None)?;
    b.ids.insert("vector", ve.editor);
    let rotation = b.sig(euler_to_quat(30.0, 10.0, 0.0));
    let qe = quat_editor(b.ui, p, "quat", rotation, "Rotation")?;
    b.ids.insert("quat", qe.editor);
    let position = b.sig(FramePos::new(
        FrameId(1),
        forge_frames::DVec3 {
            x: 1.5,
            y: 0.0,
            z: -2.25,
        },
    ));
    let fe = frame_pos_editor(
        b.ui,
        p,
        "frame_pos",
        position,
        &[(FrameId(0), "World"), (FrameId(1), "Island")],
        None,
        "Position",
    )?;
    b.ids.insert("frame_pos", fe.editor);
    let curve = b.sig(Curve::new(vec![
        CurveKey {
            t: 0.0,
            v: 0.1,
            interp: Interp::Smooth,
        },
        CurveKey {
            t: 0.4,
            v: 0.8,
            interp: Interp::Smooth,
        },
        CurveKey {
            t: 1.0,
            v: 0.3,
            interp: Interp::Linear,
        },
    ]));
    b.add(
        p,
        "curve",
        NodeStyle::leaf().size(280.0, 140.0),
        curve_editor(curve, faults),
    )?;
    let gradient = b.sig(Gradient::new(vec![
        GradientStop {
            pos: 0.0,
            color: HdrColor::from_srgb(0.1, 0.2, 0.6, 1.0),
        },
        GradientStop {
            pos: 0.5,
            color: HdrColor::from_srgb(0.9, 0.8, 0.3, 1.0),
        },
        GradientStop {
            pos: 1.0,
            color: HdrColor::from_srgb(0.8, 0.2, 0.1, 1.0),
        },
    ]));
    let stop = b.sig(HdrColor::default());
    b.add(
        p,
        "gradient",
        NodeStyle::leaf().width(280.0),
        GradientEditor::new(gradient, stop, "Sky"),
    )?;
    let index: Rc<dyn AssetIndex> = Rc::new(MemoryAssetIndex::new(&[
        ("textures/rock_albedo.tex", "Texture"),
        ("textures/grass.tex", "Texture"),
        ("meshes/boulder.mesh", "Mesh"),
    ]));
    let texture = b.sig(Some(String::from("textures/grass.tex")));
    let choice = b.sig(None);
    b.add(
        p,
        "asset_ref",
        NodeStyle::leaf().width(220.0),
        AssetRefPicker::new(texture, choice, "Texture", index.clone(), "Albedo"),
    )?;
    let mass = b.sig(12.0f64);
    let visible = b.sig(true);
    let pname = b.sig(String::from("Boulder"));
    let tint = b.sig(HdrColor::WHITE);
    let body = b.sig(0usize);
    let id_text = b.sig(String::from("entity #4711"));
    let grid_parts = property_grid(
        b.ui,
        p,
        "property_grid",
        "Inspector",
        vec![
            Property::new("General", "Name", PropKind::Text(pname)),
            Property::new("General", "Visible", PropKind::Bool(visible)),
            Property::new("General", "Id", PropKind::ReadOnly(id_text)),
            Property::new(
                "Physics",
                "Mass",
                PropKind::Number {
                    value: mass,
                    unit: Some("kg".into()),
                    range: Some((0.0, 1e6)),
                },
            ),
            Property::new(
                "Physics",
                "Body",
                PropKind::Choice {
                    options: vec!["Static".into(), "Dynamic".into(), "Kinematic".into()],
                    selected: body,
                },
            ),
            Property::new("Render", "Tint", PropKind::Color(tint)),
            Property::new(
                "Render",
                "Albedo",
                PropKind::Asset {
                    value: texture,
                    kind: "Texture".into(),
                    index,
                },
            ),
        ],
    )?;
    b.ids.insert("property_grid", grid_parts.grid);

    // 10. Navigation.
    let p = pages[10];
    let segs = b.sig(vec![
        "Project".to_string(),
        "Levels".to_string(),
        "Forest".to_string(),
        "Clearing".to_string(),
    ]);
    b.add(
        p,
        "breadcrumb",
        NodeStyle::leaf(),
        Breadcrumb::new(segs, "Location"),
    )?;
    let tb = b.add(
        p,
        "toolbar",
        NodeStyle::row(space[1]),
        Toolbar::new("Tools"),
    )?;
    for (i, (g, l)) in [("✥", "Move"), ("⟳", "Rotate"), ("⤢", "Scale")]
        .iter()
        .enumerate()
    {
        b.ui.add(
            tb,
            Key::Index(i as u32),
            NodeStyle::leaf(),
            IconButton::new(g, l),
        )?;
    }
    b.add(
        p,
        "palette",
        NodeStyle::leaf(),
        CommandPaletteButton::new(vec![
            PickItem::new("file.open", "Open project")
                .detail("Ctrl+O")
                .hint("File"),
            PickItem::new("view.profiler", "Open profiler")
                .detail("Ctrl+Alt+P")
                .hint("View"),
            PickItem::new("edit.undo", "Undo")
                .detail("Ctrl+Z")
                .hint("Edit"),
            PickItem::new("build.run", "Build and run")
                .detail("F5")
                .hint("Build"),
        ]),
    )?;
    let st1 = b.sig(String::from("Ready"));
    let st2 = b.sig(String::from("12 ms"));
    let st3 = b.sig(String::from("Ln 4, Col 2"));
    b.add(
        p,
        "status_bar",
        NodeStyle::leaf().width(520.0),
        StatusBar::new(vec![
            StatusItem {
                text: st1,
                right: false,
            },
            StatusItem {
                text: st3,
                right: false,
            },
            StatusItem {
                text: st2,
                right: true,
            },
        ]),
    )?;

    // 11. Graph: the node canvas (WP-U8).
    let p = pages[11];
    if b.omit != Some("node_canvas") {
        let c = NodeCanvas::build(
            b.ui,
            p,
            Key::Static("node_canvas"),
            NodeStyle::default().size(1100.0, 300.0),
            "Example graph",
        )?;
        b.ids.insert("node_canvas", c);
        let f =
            |l: &str, unit: &str| CanvasPin::new(l, &format!("f64 · {unit}"), ColorRole::Accent);
        NodeCanvas::edit(b.ui, c, |e| {
            e.set_comment(
                1,
                CanvasComment {
                    rect: Rect::new(20.0, 10.0, 560.0, 250.0),
                    text: "Motion".into(),
                },
            );
            e.set_node(
                1,
                CanvasNode::new("Distance", Point::new(40.0, 60.0))
                    .with_inputs(vec![f("speed", "m/s"), f("time", "s")])
                    .with_outputs(vec![f("return", "m")])
                    .with_doc("Distance covered at a constant speed."),
            );
            e.set_node(
                2,
                CanvasNode::new("Kinetic energy", Point::new(330.0, 130.0))
                    .with_inputs(vec![f("mass", "kg"), f("speed", "m/s")])
                    .with_outputs(vec![f("return", "J")]),
            );
            e.set_node(
                3,
                CanvasNode::reroute(Point::new(250.0, 190.0), ColorRole::Accent, "f64 · m/s"),
            );
            let mut bad = CanvasNode::new("Clamp", Point::new(640.0, 60.0))
                .with_inputs(vec![f("value", "J"), f("max", "J")])
                .with_outputs(vec![f("return", "J")]);
            bad.error = Some("input `max` is not connected".into());
            e.set_node(4, bad);
            e.set_wire(CanvasWire {
                from: PinRef::output(3, 0),
                to: PinRef::input(2, 1),
                role: ColorRole::Accent,
            });
            e.set_wire(CanvasWire {
                from: PinRef::output(2, 0),
                to: PinRef::input(4, 0),
                role: ColorRole::Success,
            });
        });
    }

    Ok(Catalogue {
        panel,
        tabs,
        tab,
        scroll: sa.area,
        pages,
        ids: b.ids,
        color,
        rotation,
        position,
        curve,
        gradient,
        texture,
    })
}

/// Apply the catalogue's model-changing actions, as an editor panel's command handler
/// would (the gallery has no command bus; I7 lives in the editor, WP-U4). Returns
/// whether the action was one of them.
pub fn apply_action(ui: &mut Ui, cat: &Catalogue, a: &ActionEnvelope) -> bool {
    if let Some(r) = a.get::<RowRenamed>() {
        VirtualTree::edit(ui, r.view, |t| t.set_label(r.key, &r.name));
        return true;
    }
    if let Some(d) = a.get::<RowsDropped>() {
        VirtualTree::edit(ui, d.view, |t| {
            let mut index = d.target.index;
            for k in &d.keys {
                // Moving a row down within its parent shifts the target left by one.
                if t.index().parent(*k) == d.target.parent
                    && t.index().index_in_parent(*k).is_some_and(|i| i < index)
                {
                    index -= 1;
                }
                t.move_node(
                    *k,
                    crate::widgets::DropTarget {
                        parent: d.target.parent,
                        index,
                    },
                );
                index += 1;
            }
        });
        return true;
    }
    if let Some(d) = a.get::<RowsDeleteRequested>() {
        VirtualTree::edit(ui, d.view, |t| {
            for k in &d.keys {
                t.remove(*k);
            }
        });
        return true;
    }
    if let Some(c) = a.get::<TabClosed>() {
        let page = ui
            .widget_mut::<Tabs>(c.tabs)
            .and_then(|t| t.remove_tab(c.index));
        if let Some(p) = page {
            let _ = ui.remove(p);
        }
        ui.invalidate(c.tabs, crate::damage::Dirty::LAYOUT);
        if let Some(t) = ui.widget::<Tabs>(c.tabs)
            && let Some(first) = t.pages().first().copied()
        {
            let _ = ui.set_hidden(first, false);
        }
        let _ = cat;
        return true;
    }
    false
}

/// A 4-column demo table model; sorting keeps a permutation (§21.12: model side).
pub struct DemoTable {
    order: Vec<u32>,
}

impl DemoTable {
    pub fn new(n: u32) -> Self {
        Self {
            order: (0..n).collect(),
        }
    }
    fn kind(i: u32) -> &'static str {
        ["Texture", "Mesh", "Material", "Sound"][(i % 4) as usize]
    }
    fn size(i: u32) -> u64 {
        u64::from(i).wrapping_mul(2_654_435_761) % 90_000 + 12
    }
}

impl TableModel for DemoTable {
    fn len(&self) -> usize {
        self.order.len()
    }
    fn key(&self, row: usize) -> u64 {
        u64::from(self.order[row])
    }
    fn cell(&self, row: usize, col: usize) -> String {
        let i = self.order[row];
        match col {
            0 => format!("asset_{i:04}"),
            1 => Self::kind(i).to_string(),
            2 => Self::size(i).to_string(),
            _ => (i % 17).to_string(),
        }
    }
    fn sort(&mut self, col: usize, dir: SortDir) {
        match col {
            1 => self.order.sort_by_key(|i| (Self::kind(*i), *i)),
            2 => self.order.sort_by_key(|i| (Self::size(*i), *i)),
            3 => self.order.sort_by_key(|i| (i % 17, *i)),
            _ => self.order.sort_unstable(),
        }
        if dir == SortDir::Descending {
            self.order.reverse();
        }
    }
}

/// The catalogue's curve editor (with the idle control's fault in test builds).
fn curve_editor(curve: Signal<Curve>, faults: crate::gallery::GalleryFaults) -> CurveEditor {
    let c = CurveEditor::new(curve, "Falloff");
    #[cfg(any(test, feature = "controls"))]
    if faults.curve_never_idles {
        return c.with_fault_never_idle();
    }
    let _ = faults;
    c
}
