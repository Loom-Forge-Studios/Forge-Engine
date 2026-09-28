//! The Tracy sink (feature `tracy`): zones become Tracy spans, counters plots, frame marks
//! frame marks, instants messages. The client listens on loopback only and collects nothing
//! until a Tracy viewer connects (`only-localhost`, `ondemand`; see Cargo.toml).

use std::collections::BTreeMap;
use std::sync::{Mutex, PoisonError};

use tracy_client::{Client, PlotName, Span};

/// Start the client (idempotent).
pub(crate) fn start() {
    let _ = Client::start();
}

fn client() -> Option<Client> {
    Client::running()
}

pub(crate) fn span(name: &str, file: &str, line: u32) -> Span {
    // A zone may open after `disable` raced `enable`; start is idempotent and cheap.
    let c = client().unwrap_or_else(Client::start);
    c.span_alloc(Some(name), "", file, line, 0)
}

pub(crate) fn plot(name: &'static str, value: f64) {
    static NAMES: Mutex<BTreeMap<&'static str, PlotName>> = Mutex::new(BTreeMap::new());
    let Some(c) = client() else { return };
    let pn = *NAMES
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .entry(name)
        .or_insert_with(|| PlotName::new_leak(name.to_owned()));
    c.plot(pn, value);
}

pub(crate) fn frame_mark() {
    if let Some(c) = client() {
        c.frame_mark();
    }
}

pub(crate) fn message(text: &str) {
    if let Some(c) = client() {
        c.message(text, 0);
    }
}
