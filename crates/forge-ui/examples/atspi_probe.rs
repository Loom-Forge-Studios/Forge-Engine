//! `atspi_probe` — a screen-reader-side client for the Linux accessibility bus (AT-SPI), the
//! evidence for DoD M2-22 on Linux (WP-U12; `tools/docker/linux-verify/atspi-probe.sh`).
//!
//! What an assistive technology does, over D-Bus, with nothing of forge-ui linked in:
//!
//! 1. `atspi_probe enable` — turn accessibility on for the session (`org.a11y.Status.IsEnabled`
//!    on the session bus's `org.a11y.Bus`), as a screen reader starting up does. AccessKit's
//!    Linux adapter builds no tree until then (lazy activation, §21.10).
//! 2. `atspi_probe walk NAME [SECONDS]` — wait (up to SECONDS) for an application whose name
//!    contains NAME to register with the AT-SPI registry, then walk its tree: every node's
//!    role, name and child count, printed as an indented outline, with the totals. Exits 0 if
//!    the walk found the widgets `expect` names (role and name), 1 otherwise.
//!
//! Linux only: elsewhere it prints that and exits 0.

#[cfg(target_os = "linux")]
mod probe {
    use std::time::{Duration, Instant};

    use zbus::blocking::Connection;
    use zbus::zvariant::{OwnedObjectPath, OwnedValue, Value};

    type Ref = (String, OwnedObjectPath);

    fn err(e: impl std::fmt::Display) -> String {
        e.to_string()
    }

    fn session() -> Result<Connection, String> {
        Connection::session().map_err(|e| format!("session bus: {e}"))
    }

    /// The accessibility bus (its address is published by `org.a11y.Bus` on the session bus).
    fn a11y_bus() -> Result<Connection, String> {
        let s = session()?;
        let m = s
            .call_method(
                Some("org.a11y.Bus"),
                "/org/a11y/bus",
                Some("org.a11y.Bus"),
                "GetAddress",
                &(),
            )
            .map_err(|e| format!("org.a11y.Bus.GetAddress: {e}"))?;
        let addr: String = m.body().deserialize().map_err(err)?;
        zbus::blocking::connection::Builder::address(addr.as_str())
            .map_err(err)?
            .build()
            .map_err(|e| format!("a11y bus {addr}: {e}"))
    }

    pub fn enable() -> Result<(), String> {
        let s = session()?;
        s.call_method(
            Some("org.a11y.Bus"),
            "/org/a11y/bus",
            Some("org.freedesktop.DBus.Properties"),
            "Set",
            &("org.a11y.Status", "IsEnabled", Value::from(true)),
        )
        .map_err(|e| format!("org.a11y.Status.IsEnabled = true: {e}"))?;
        println!(
            "atspi_probe: accessibility enabled on the session bus (org.a11y.Status.IsEnabled)"
        );
        Ok(())
    }

    fn children(c: &Connection, r: &Ref) -> Result<Vec<Ref>, String> {
        let m = c
            .call_method(
                Some(r.0.as_str()),
                r.1.as_str(),
                Some("org.a11y.atspi.Accessible"),
                "GetChildren",
                &(),
            )
            .map_err(|e| format!("GetChildren {}{}: {e}", r.0, r.1.as_str()))?;
        m.body().deserialize().map_err(err)
    }

    /// AT-SPI role names by role number (AtspiRole, as at-spi2-core numbers them).
    const ROLES: [&str; 124] = [
        "invalid",
        "accelerator label",
        "alert",
        "animation",
        "arrow",
        "calendar",
        "canvas",
        "check box",
        "check menu item",
        "color chooser",
        "column header",
        "combo box",
        "date editor",
        "desktop icon",
        "desktop frame",
        "dial",
        "dialog",
        "directory pane",
        "drawing area",
        "file chooser",
        "filler",
        "focus traversable",
        "font chooser",
        "frame",
        "glass pane",
        "htmlcontainer",
        "icon",
        "image",
        "internal frame",
        "label",
        "layered pane",
        "list",
        "list item",
        "menu",
        "menu bar",
        "menu item",
        "option pane",
        "page tab",
        "page tab list",
        "panel",
        "password text",
        "popup menu",
        "progress bar",
        "push button",
        "radio button",
        "radio menu item",
        "root pane",
        "row header",
        "scroll bar",
        "scroll pane",
        "separator",
        "slider",
        "spin button",
        "split pane",
        "status bar",
        "table",
        "table cell",
        "table column header",
        "table row header",
        "tearoff menu item",
        "terminal",
        "text",
        "toggle button",
        "tool bar",
        "tool tip",
        "tree",
        "tree table",
        "unknown",
        "viewport",
        "window",
        "extended",
        "header",
        "footer",
        "paragraph",
        "ruler",
        "application",
        "autocomplete",
        "editbar",
        "embedded",
        "entry",
        "chart",
        "caption",
        "document frame",
        "heading",
        "page",
        "section",
        "redundant object",
        "form",
        "link",
        "input method window",
        "table row",
        "tree item",
        "document spreadsheet",
        "document presentation",
        "document text",
        "document web",
        "document email",
        "comment",
        "list box",
        "grouping",
        "image map",
        "notification",
        "info bar",
        "level bar",
        "title bar",
        "block quote",
        "audio",
        "video",
        "definition",
        "article",
        "landmark",
        "log",
        "marquee",
        "math",
        "rating",
        "timer",
        "static",
        "math fraction",
        "math root",
        "subscript",
        "superscript",
        "description list",
        "description term",
        "description value",
    ];

    /// The node's role (`GetRole`, an AtspiRole number), by its AT-SPI name.
    fn role(c: &Connection, r: &Ref) -> Result<String, String> {
        let m = c
            .call_method(
                Some(r.0.as_str()),
                r.1.as_str(),
                Some("org.a11y.atspi.Accessible"),
                "GetRole",
                &(),
            )
            .map_err(|e| format!("GetRole {}{}: {e}", r.0, r.1.as_str()))?;
        let n: u32 = m.body().deserialize().map_err(err)?;
        Ok(ROLES
            .get(n as usize)
            .map_or_else(|| format!("role {n}"), |s| (*s).to_string()))
    }

    fn name(c: &Connection, r: &Ref) -> Result<String, String> {
        let m = c
            .call_method(
                Some(r.0.as_str()),
                r.1.as_str(),
                Some("org.freedesktop.DBus.Properties"),
                "Get",
                &("org.a11y.atspi.Accessible", "Name"),
            )
            .map_err(|e| format!("Name {}{}: {e}", r.0, r.1.as_str()))?;
        let v: OwnedValue = m.body().deserialize().map_err(err)?;
        String::try_from(v).map_err(err)
    }

    fn root() -> Ref {
        (
            "org.a11y.atspi.Registry".to_string(),
            OwnedObjectPath::try_from("/org/a11y/atspi/accessible/root").unwrap_or_default(),
        )
    }

    /// The widgets the gallery must expose: (role name, accessible name).
    const EXPECT: &[(&str, &str)] = &[
        ("push button", "Save"),
        ("push button", "Cancel"),
        ("push button", "Close"),
        ("check box", "Snap to grid"),
        ("slider", "Opacity"),
        ("radio button", "World"),
        ("page tab", "Jobs"),
    ];

    pub fn walk(want: &str, wait: Duration) -> Result<bool, String> {
        let c = a11y_bus()?;
        let start = Instant::now();
        let app = loop {
            let apps = children(&c, &root())?;
            let mut names = Vec::new();
            let mut found = None;
            for a in apps {
                let n = name(&c, &a).unwrap_or_default();
                if n.contains(want) {
                    found = Some((a, n.clone()));
                }
                names.push(n);
            }
            if let Some(f) = found {
                break f;
            }
            if start.elapsed() > wait {
                return Err(format!(
                    "no application named like {want:?} registered in {:.0} s; registered: {names:?}",
                    wait.as_secs_f64()
                ));
            }
            std::thread::sleep(Duration::from_millis(250));
        };
        println!(
            "atspi_probe: application {:?} at {}{} (found after {:.1} s)",
            app.1,
            (app.0).0,
            (app.0).1.as_str(),
            start.elapsed().as_secs_f64()
        );
        // Depth-first, bounded (a virtualised list exposes only its live rows anyway).
        let mut stack = vec![(app.0, 0usize)];
        let mut nodes = 0usize;
        let mut named = 0usize;
        let mut seen: Vec<(String, String)> = Vec::new();
        while let Some((r, depth)) = stack.pop() {
            nodes += 1;
            if nodes > 5000 {
                println!("atspi_probe: stopped at 5000 nodes");
                break;
            }
            let ro = role(&c, &r).unwrap_or_else(|e| format!("? ({e})"));
            let n = name(&c, &r).unwrap_or_default();
            let kids = children(&c, &r).unwrap_or_default();
            if !n.is_empty() {
                named += 1;
            }
            println!(
                "{}{ro} \u{201c}{n}\u{201d}{}",
                "  ".repeat(depth),
                if kids.is_empty() {
                    String::new()
                } else {
                    format!(" ({} children)", kids.len())
                }
            );
            seen.push((ro, n));
            for k in kids.into_iter().rev() {
                stack.push((k, depth + 1));
            }
        }
        println!("atspi_probe: {nodes} nodes, {named} named");
        let mut ok = true;
        for (ro, n) in EXPECT {
            let hit = seen.iter().any(|(r, nn)| r == ro && nn == n);
            println!(
                "atspi_probe: expect {ro} \u{201c}{n}\u{201d}: {}",
                if hit { "found" } else { "MISSING" }
            );
            ok &= hit;
        }
        Ok(ok)
    }
}

#[cfg(target_os = "linux")]
fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let r = match args.first().map(String::as_str) {
        Some("enable") => probe::enable().map(|()| true),
        Some("walk") => {
            let want = args.get(1).cloned().unwrap_or_else(|| "forge-ui".into());
            let secs = args
                .get(2)
                .and_then(|s| s.parse::<f64>().ok())
                .unwrap_or(30.0);
            probe::walk(&want, std::time::Duration::from_secs_f64(secs))
        }
        _ => Err("usage: atspi_probe enable | walk NAME [SECONDS]".into()),
    };
    match r {
        Ok(true) => {}
        Ok(false) => std::process::exit(1),
        Err(e) => {
            eprintln!("atspi_probe: {e}");
            std::process::exit(2);
        }
    }
}

#[cfg(not(target_os = "linux"))]
fn main() {
    println!("atspi_probe: AT-SPI is the Linux accessibility bus; nothing to probe here");
}
