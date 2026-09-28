//! `dock_demo` — docking in real OS windows (Ch.21 §21.17): drag tabs to dock with drop
//! previews, drag a tab out of the window to tear it off into its own window (on any
//! monitor), close that window to dock its panels back, Shift+Space to maximise.
//!
//! ```text
//! cargo run -p forge-ui --example dock_demo -- [--float] [--exit-after SECS]
//! ```
//!
//! `--float` tears the Console off at start-up (the runner path a drag-out takes);
//! `--exit-after` exits that long after the UI settles and prints the windows opened and
//! the idle window's frames and wakeups (both 0: an idle dock draws nothing).

use std::time::Duration;

use forge_ui::dock::{
    AreaId, Axis, DockController, DockNode, DockOp, DockRequest, Layout, MonitorInfo, PanelHost,
    PanelId,
};
use forge_ui::platform_winit::{RunOptions, UiApp, run};
use forge_ui::widgets::{Build, Checkbox, Label};
use forge_ui::{ActionEnvelope, NodeStyle, Rect, Ui, UiError, WidgetId};

struct DemoHost;

impl PanelHost for DemoHost {
    fn title(&self, p: &PanelId) -> Option<String> {
        let t = match p.as_str() {
            "demo.hierarchy" => "Hierarchy",
            "demo.viewport" => "Viewport",
            "demo.console" => "Console",
            "demo.assets" => "Assets",
            "demo.inspector" => "Inspector",
            _ => return None,
        };
        Some(t.to_string())
    }
    fn build(&mut self, b: &mut dyn Build, parent: WidgetId, p: &PanelId) -> Result<bool, UiError> {
        let Some(t) = self.title(p) else {
            return Ok(false);
        };
        b.add(parent, "title", NodeStyle::leaf(), Label::new(t.as_str()))?;
        let on = b.signal(false);
        b.add(
            parent,
            "toggle",
            NodeStyle::leaf(),
            Checkbox::new(on, "Panel state survives re-docking"),
        )?;
        Ok(true)
    }
}

struct Demo {
    dock: DockController,
    float_at_start: bool,
}

impl UiApp for Demo {
    fn title(&self) -> String {
        "forge-ui dock demo".into()
    }
    fn build(&mut self, ui: &mut Ui) -> Result<(), UiError> {
        let root = ui.root();
        self.dock
            .build_main(ui, root, "dock", NodeStyle::default().fill())?;
        if self.float_at_start {
            let o = ui.window_origin();
            ui.raise(
                root,
                DockRequest {
                    area: AreaId::Main,
                    op: DockOp::Float {
                        panel: PanelId::new("demo.console"),
                        rect: Rect::new(o.x + 760.0, o.y + 120.0, 480.0, 320.0),
                        monitor: ui.monitor().map(str::to_string),
                    },
                },
            );
        }
        Ok(())
    }
    fn build_window(&mut self, ui: &mut Ui, key: u64) -> Result<(), UiError> {
        self.dock.build_floating(ui, key).map(|_| ())
    }
    fn on_actions(&mut self, ui: &mut Ui, actions: Vec<ActionEnvelope>) {
        let _rest = self.dock.handle_actions(actions);
        let root = ui.root();
        for o in self.dock.windows_to_open() {
            ui.raise(root, o);
        }
    }
    fn update_window(&mut self, ui: &mut Ui, key: Option<u64>) {
        self.dock
            .sync_window(ui, key.map_or(AreaId::Main, AreaId::Floating));
    }
    fn window_moved(&mut self, key: Option<u64>, rect: Rect, monitor: Option<String>) {
        self.dock
            .window_moved(key.map_or(AreaId::Main, AreaId::Floating), rect, monitor);
    }
    fn window_closed(&mut self, key: u64) {
        self.dock.window_closed(key);
    }
    fn monitors(&mut self, monitors: Vec<MonitorInfo>) {
        self.dock.set_monitors(monitors);
    }
}

fn layout() -> Layout {
    Layout::new(DockNode::split(
        Axis::Horizontal,
        vec![
            (0.2, DockNode::tabs(&["demo.hierarchy"])),
            (
                0.58,
                DockNode::split(
                    Axis::Vertical,
                    vec![
                        (0.68, DockNode::tabs(&["demo.viewport"])),
                        (0.32, DockNode::tabs(&["demo.console", "demo.assets"])),
                    ],
                ),
            ),
            (0.22, DockNode::tabs(&["demo.inspector"])),
        ],
    ))
}

fn main() {
    let mut opts = RunOptions::default();
    let mut float_at_start = false;
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--float" => float_at_start = true,
            "--exit-after" => {
                let secs: f64 = args.get(i + 1).and_then(|s| s.parse().ok()).unwrap_or(3.0);
                opts.exit_after = Some(Duration::from_secs_f64(secs));
                i += 1;
            }
            other => eprintln!("dock_demo: ignoring unknown argument {other}"),
        }
        i += 1;
    }
    let dock = DockController::new(layout(), Box::new(|_| Box::new(DemoHost)));
    match run(
        Demo {
            dock,
            float_at_start,
        },
        opts,
    ) {
        Ok(r) => println!(
            "dock_demo: adapter {} | windows opened {} | frames {} | idle window {:.1} s: {} frames, {} wakeups",
            r.adapter,
            r.windows_opened,
            r.frames_rendered,
            r.idle_window.as_secs_f64(),
            r.idle_frames,
            r.idle_wakeups
        ),
        Err(e) => {
            eprintln!("dock_demo: {e}");
            std::process::exit(1);
        }
    }
}
