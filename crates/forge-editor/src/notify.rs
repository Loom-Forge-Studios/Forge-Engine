//! The notification centre (Ch.21 §21.18, §21.21 "Notifications", DoD M2-42).
//!
//! Every refusal from the bus, every failed user-config write and every message a panel
//! raises becomes a [`Notification`]: shown as a toast, and kept in the history drawer (the
//! Notifications panel) with its stable `ErrorCode` and its actions ("Open console",
//! "Retry"). Nothing is silently dropped; the history is bounded and says how many old
//! entries it let go.
//!
//! **A problem says what to do next (M2-70, WP-U12).** A warning or an error is posted only
//! as a [`Problem`]: its stable `ErrorCode`, what happened, and the next step — the type
//! has no way to leave either out, and [`NotificationCentre::post_problem`] refuses an
//! empty one (it is still shown, under the `EDITOR-0015` code of an unexplained problem, so
//! nothing is dropped). Plain [`NotificationCentre::post`] carries information and
//! successes only.

use std::collections::VecDeque;

/// How serious a notification is. Colour is never the only carrier (each has a word).
#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Level {
    Info,
    Success,
    Warning,
    Error,
}

impl Level {
    pub fn word(self) -> &'static str {
        match self {
            Level::Info => forge_ui::tr!("Info"),
            Level::Success => forge_ui::tr!("Done"),
            Level::Warning => forge_ui::tr!("Warning"),
            Level::Error => forge_ui::tr!("Error"),
        }
    }
}

/// What a notification's action button asks the shell to do (session state only).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NoticeAction {
    /// Open (focus) a panel, e.g. the console at the entry.
    OpenPanel { panel: String, label: String },
    /// Run an action by id again (e.g. "Retry" a save).
    Run { action: String, label: String },
    /// Open a panel at a thing ("Details": the console at the rejection's entry).
    Reveal {
        reveal: crate::session::Reveal,
        panel: String,
        label: String,
    },
}

impl NoticeAction {
    pub fn label(&self) -> &str {
        match self {
            NoticeAction::OpenPanel { label, .. }
            | NoticeAction::Run { label, .. }
            | NoticeAction::Reveal { label, .. } => label,
        }
    }
}

/// One notification.
#[derive(Clone, Debug, PartialEq)]
pub struct Notification {
    pub id: u64,
    pub level: Level,
    /// The stable error code (`CMD-0003`, `EDITOR-0006`), when it is an error.
    pub code: Option<String>,
    pub title: String,
    pub detail: String,
    /// What to do next (every warning and error has one, M2-70).
    pub next: Option<String>,
    pub actions: Vec<NoticeAction>,
    pub read: bool,
}

impl Notification {
    /// The one-line text a toast shows: code first, so it can be searched and reported,
    /// then the next step.
    pub fn toast_text(&self) -> String {
        let title = &self.title;
        match (&self.code, &self.next) {
            (Some(code), Some(next)) => {
                forge_ui::trf!("{code}: {title} \u{2014} {next}", code, title, next)
            }
            (Some(code), None) => forge_ui::trf!("{code}: {title}", code, title),
            (None, Some(next)) => forge_ui::trf!("{title} \u{2014} {next}", title, next),
            (None, None) => title.clone(),
        }
    }
}

/// How serious a [`Problem`] is.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Severity {
    Warning,
    Error,
}

/// A warning or an error the user sees (M2-70): the stable code, what happened, the detail
/// (the reason, usually the error's own message) and what to do next.
#[derive(Clone, Debug, PartialEq)]
pub struct Problem {
    pub severity: Severity,
    /// The stable `ErrorCode` (`CMD-0003`, `EDITOR-0006`; `docs/error-codes.md`).
    pub code: String,
    /// What happened, in a few words ("Rename refused").
    pub what: String,
    /// Why (the error's message).
    pub detail: String,
    /// What to do next (a sentence the user can act on).
    pub next: String,
    /// `next` is the generic step looked up from the code or the refusal
    /// ([`next_step_for_code`], [`next_step_for_rejection`], an editor error's own), not one
    /// the poster wrote for this problem: a plugin's [`NoticeHint`] may replace it.
    pub generic_next: bool,
    pub actions: Vec<NoticeAction>,
}

impl Problem {
    pub fn error(code: &str, what: &str, detail: &str, next: &str) -> Self {
        Self::new(Severity::Error, code, what, detail, next)
    }
    pub fn warning(code: &str, what: &str, detail: &str, next: &str) -> Self {
        Self::new(Severity::Warning, code, what, detail, next)
    }
    fn new(severity: Severity, code: &str, what: &str, detail: &str, next: &str) -> Self {
        Self {
            severity,
            code: code.to_string(),
            what: what.to_string(),
            detail: detail.to_string(),
            next: next.to_string(),
            generic_next: false,
            actions: Vec::new(),
        }
    }
    fn generic(mut self) -> Self {
        self.generic_next = true;
        self
    }
    /// An editor error, with its code and the next step its variant names.
    pub fn from_editor(severity: Severity, what: &str, e: &crate::EditorError) -> Self {
        Self::new(severity, e.code(), what, &e.to_string(), e.next_step()).generic()
    }
    /// A problem from any engine error by its code: the next step is the editor error
    /// variant's own, or the one [`next_step_for_code`] gives for the code's crate.
    pub fn coded(severity: Severity, code: &str, what: &str, detail: &str) -> Self {
        Self::new(severity, code, what, detail, next_step_for_code(code)).generic()
    }
    /// A command the bus refused: its `CMD-*` code, its reason and the next step for it.
    pub fn from_rejection(what: &str, r: &forge_cmd::Rejection) -> Self {
        Self::new(
            Severity::Error,
            r.code().as_str(),
            what,
            &r.error.to_string(),
            next_step_for_rejection(&r.error),
        )
        .generic()
    }
    /// With these action buttons.
    #[must_use]
    pub fn with_actions(mut self, a: Vec<NoticeAction>) -> Self {
        self.actions.extend(a);
        self
    }
    /// With an action button.
    #[must_use]
    pub fn action(mut self, a: NoticeAction) -> Self {
        self.actions.push(a);
        self
    }
    /// Whether it says everything M2-70 asks: a well-formed code, what happened and a next
    /// step.
    pub fn is_complete(&self) -> bool {
        well_formed_code(&self.code) && !self.what.trim().is_empty() && !self.next.trim().is_empty()
    }
}

/// `PREFIX-NNNN` (`docs/error-codes.md`).
pub fn well_formed_code(code: &str) -> bool {
    code.split_once('-').is_some_and(|(p, n)| {
        !p.is_empty()
            && p.chars().all(|c| c.is_ascii_uppercase())
            && n.len() == 4
            && n.chars().all(|c| c.is_ascii_digit())
    })
}

/// The code an incomplete [`Problem`] is shown under (`EDITOR-0015`, `EditorError::Unexplained`):
/// the refusal is still shown with whatever it said, so nothing is dropped, and the gap is visible
/// and counted.
pub const INCOMPLETE_PROBLEM_CODE: &str = "EDITOR-0015";

/// What to do next about an error with this code: an editor error variant's own next step,
/// otherwise the one for the crate that owns the code's prefix (`docs/error-codes.md`).
pub fn next_step_for_code(code: &str) -> &'static str {
    if let Some(e) = crate::EditorError::all_variants_for_tests()
        .into_iter()
        .find(|e| e.code() == code)
    {
        return e.next_step();
    }
    match code.split_once('-').map_or(code, |(p, _)| p) {
        "CMD" => forge_ui::tr!("Read the reason, correct the edit and try again."),
        "PROJECT" | "STORE" => {
            forge_ui::tr!(
                "Check the project folder and its store (disk space, permissions, the linked remote), then try again."
            )
        }
        "PLUGIN" | "WASM" => {
            forge_ui::tr!(
                "Disable or update the plugin named in the message in the plugin manager."
            )
        }
        "ASSET" => forge_ui::tr!("Check the source file named in the message and import it again."),
        "SIM" => forge_ui::tr!("Stop the play session and start it again."),
        "REMOTE" => forge_ui::tr!("Check the connection and pair the device again."),
        "UI" => forge_ui::tr!("Reset the layout from the Window menu."),
        "RENDER" | "GPU" => {
            forge_ui::tr!("Update the graphics driver, or pick another adapter in Settings.")
        }
        _ => forge_ui::tr!("Report this code with what you were doing."),
    }
}

/// What to do next about a command the bus refused (Ch.21 §21.21: a refusal shows its
/// reason and a next step, never a generic "failed").
pub fn next_step_for_rejection(e: &forge_cmd::CmdError) -> &'static str {
    use forge_cmd::CmdError as C;
    match e {
        C::UnknownEntity(_) => {
            forge_ui::tr!("The entity no longer exists: pick another one in the hierarchy.")
        }
        C::Cycle { .. } => {
            forge_ui::tr!("Choose a new parent that is not inside the entity you are moving.")
        }
        C::BadPath(_) => {
            forge_ui::tr!("Use a key made of dot-separated names (letters, digits, underscores).")
        }
        C::NonFinite { .. } => forge_ui::tr!("Enter a finite number."),
        C::UnknownTxn(_) | C::TxnState { .. } => {
            forge_ui::tr!("Try the edit again; the step it belonged to had already ended.")
        }
        C::BadName(_) => {
            forge_ui::tr!(
                "Enter a name that is not empty, not too long and has no control characters."
            )
        }
        C::Conflict { .. } => {
            forge_ui::tr!(
                "Someone changed this first: look at the new value, then make your edit again."
            )
        }
        C::Panicked { .. } => {
            forge_ui::tr!("Nothing was changed. Report the code and the console entry.")
        }
        C::UnknownHandler(_) => {
            forge_ui::tr!("Enable the plugin that provides this command in the plugin manager.")
        }
        C::BadArgs { .. } => {
            forge_ui::tr!("Correct the values named in the message and try again.")
        }
        C::IssuerMismatch { .. } => {
            forge_ui::tr!("Start your own edit; another issuer's step cannot be continued.")
        }
        C::DuplicateCommand(_) => forge_ui::tr!("Nothing to do: the edit was already applied."),
        C::PolicyRefused { .. } => {
            forge_ui::tr!("Ask a person with the right to make this change.")
        }
        C::Poisoned => {
            forge_ui::tr!("Save a copy, then reopen the project from its last revision.")
        }
        C::DuplicateHandler(_) => {
            forge_ui::tr!("Disable one of the two plugins that register this command.")
        }
        _ => forge_ui::tr!("Read the reason above, correct it and try again."),
    }
}

// ---- hints plugins add to problems (the NoticeHint point) -----------------------------------

/// Which problems a [`NoticeHint`] helps with, by their stable code (`docs/error-codes.md`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HintMatch {
    /// One code (`CMD-0014`).
    Code(String),
    /// Every code of one crate: the prefix before the dash (`ASSET` matches `ASSET-0002`).
    Prefix(String),
}

impl HintMatch {
    /// Does `code` fall under this match?
    #[must_use]
    pub fn matches(&self, code: &str) -> bool {
        match self {
            HintMatch::Code(c) => c == code,
            HintMatch::Prefix(p) => code.split_once('-').is_some_and(|(q, _)| q == p),
        }
    }
}

/// A hint a plugin adds to the problems it knows how to help with (WP-47): the base editor
/// knows only generic next steps for a code its plugins own, and nothing about the panels a
/// plugin brings. A hint gives such a problem a better next step and a button that opens the
/// plugin's panel where the problem is fixed. Both texts are localisation keys (M2-31).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NoticeHint {
    /// The problems it applies to.
    pub applies_to: HintMatch,
    /// The next step, replacing the **generic** one looked up for the code or the refusal
    /// ([`Problem::generic_next`]) — never a next step the poster wrote for this problem.
    pub next: Option<String>,
    /// A panel the notification offers to open: `(panel id, button label key)`.
    pub open: Option<(String, String)>,
}

/// The `NoticeHint` extension point (`forge.editor.notice_hint`, WP-47). The registry key is
/// the hint's own id; [`NoticeHints`] is what the notification centre consults.
pub struct NoticeHintPoint;

impl forge_plugin::ExtensionPoint for NoticeHintPoint {
    type Item = NoticeHint;
    const ID: &'static str = "forge.editor.notice_hint";
    const NAME: &'static str = "NoticeHint";
}

/// The hints the loaded plugins added (taken from the plugin registry when the shell is
/// assembled), in registry order. For a code, a hint naming that exact code is chosen over one
/// matching its prefix; among equals, the first in registry order.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct NoticeHints {
    hints: Vec<NoticeHint>,
}

impl NoticeHints {
    /// The hints of a registry, in its order.
    #[must_use]
    pub fn from_registry(reg: &forge_plugin::Registry<NoticeHintPoint>) -> Self {
        Self {
            hints: reg.iter().map(|(_, h)| h.clone()).collect(),
        }
    }

    /// With one more hint (after the others).
    #[must_use]
    pub fn with(mut self, h: NoticeHint) -> Self {
        self.hints.push(h);
        self
    }

    /// The hint for problems with `code`, if any (see the type docs).
    #[must_use]
    pub fn find(&self, code: &str) -> Option<&NoticeHint> {
        self.hints
            .iter()
            .find(|h| matches!(&h.applies_to, HintMatch::Code(c) if c == code))
            .or_else(|| self.hints.iter().find(|h| h.applies_to.matches(code)))
    }

    /// How many hints there are.
    #[must_use]
    pub fn len(&self) -> usize {
        self.hints.len()
    }

    /// True when no plugin added a hint.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.hints.is_empty()
    }

    /// `p` with the hint for its code applied: the generic next step replaced by the hint's,
    /// and the hint's panel offered as an action (once).
    #[must_use]
    pub fn apply(&self, mut p: Problem) -> Problem {
        let Some(h) = self.find(&p.code) else {
            return p;
        };
        if p.generic_next
            && let Some(next) = &h.next
        {
            p.next = forge_ui::l10n::tr_str(next).into_owned();
            p.generic_next = false;
        }
        if let Some((panel, label)) = &h.open {
            let offered = p
                .actions
                .iter()
                .any(|a| matches!(a, NoticeAction::OpenPanel { panel: q, .. } if q == panel));
            if !offered {
                p.actions.push(NoticeAction::OpenPanel {
                    panel: panel.clone(),
                    label: forge_ui::l10n::tr_str(label).into_owned(),
                });
            }
        }
        p
    }
}

/// Bound on the history drawer.
pub const NOTICE_HISTORY_CAP: usize = 500;

/// The centre (see the module docs).
#[derive(Debug, Default)]
pub struct NotificationCentre {
    history: VecDeque<Notification>,
    next_id: u64,
    /// Not yet shown as a toast (the shell takes them each turn).
    to_toast: Vec<u64>,
    dropped: u64,
    revision: u64,
    incomplete: u64,
    /// The hints plugins added ([`NoticeHints`]), applied to every problem posted.
    hints: std::rc::Rc<NoticeHints>,
}

impl NotificationCentre {
    pub fn new() -> Self {
        Self::default()
    }

    /// Apply these plugin hints to every problem posted from now on (the shell sets the
    /// loaded plugins' hints when it is assembled).
    pub fn set_hints(&mut self, hints: std::rc::Rc<NoticeHints>) {
        self.hints = hints;
    }

    /// The hints applied to problems.
    #[must_use]
    pub fn hints(&self) -> &NoticeHints {
        &self.hints
    }

    /// Post information or a success (a warning or an error is a [`Problem`]:
    /// [`NotificationCentre::post_problem`]); returns its id.
    pub fn post(
        &mut self,
        level: Level,
        title: &str,
        detail: &str,
        actions: Vec<NoticeAction>,
    ) -> u64 {
        let level = match level {
            Level::Warning | Level::Error => Level::Info,
            l => l,
        };
        self.push(level, None, title, detail, None, actions)
    }

    /// Post a problem (M2-70); returns its id. An incomplete one (no well-formed code, no
    /// "what", no next step) is shown anyway under [`INCOMPLETE_PROBLEM_CODE`] and counted
    /// in [`NotificationCentre::incomplete`] — the guard's measure.
    pub fn post_problem(&mut self, p: Problem) -> u64 {
        let p = self.hints.apply(p);
        let level = match p.severity {
            Severity::Warning => Level::Warning,
            Severity::Error => Level::Error,
        };
        if p.is_complete() {
            return self.push(
                level,
                Some(&p.code),
                &p.what,
                &p.detail,
                Some(&p.next),
                p.actions,
            );
        }
        self.incomplete += 1;
        let detail = if well_formed_code(&p.code) {
            p.detail.clone()
        } else {
            format!("{} {}", p.code, p.detail).trim().to_string()
        };
        let what = if p.what.trim().is_empty() {
            forge_ui::tr!("Something was refused")
        } else {
            &p.what
        };
        let next = if p.next.trim().is_empty() {
            forge_ui::tr!("Report this code: the message did not say what to do next.")
        } else {
            &p.next
        };
        self.push(
            level,
            Some(INCOMPLETE_PROBLEM_CODE),
            what,
            &detail,
            Some(next),
            p.actions,
        )
    }

    /// Problems posted without everything M2-70 asks (see [`NotificationCentre::post_problem`]).
    pub fn incomplete(&self) -> u64 {
        self.incomplete
    }

    fn push(
        &mut self,
        level: Level,
        code: Option<&str>,
        title: &str,
        detail: &str,
        next: Option<&str>,
        actions: Vec<NoticeAction>,
    ) -> u64 {
        self.next_id += 1;
        let id = self.next_id;
        if self.history.len() >= NOTICE_HISTORY_CAP {
            self.history.pop_front();
            self.dropped += 1;
        }
        self.history.push_back(Notification {
            id,
            level,
            code: code.map(str::to_string),
            title: title.to_string(),
            detail: detail.to_string(),
            next: next.map(str::to_string),
            actions,
            read: false,
        });
        // Bounded like the history: a consumer that never takes toasts cannot grow this.
        if self.to_toast.len() >= NOTICE_HISTORY_CAP {
            self.to_toast.remove(0);
        }
        self.to_toast.push(id);
        self.revision += 1;
        id
    }

    /// Notifications posted after the one with id `id`, oldest first (the toasts overlay
    /// follows the centre by id, reading, never draining).
    pub fn after(&self, id: u64) -> impl Iterator<Item = &Notification> {
        self.history.iter().filter(move |n| n.id > id)
    }

    /// Notifications not yet shown as toasts, oldest first.
    pub fn take_toasts(&mut self) -> Vec<Notification> {
        let ids = std::mem::take(&mut self.to_toast);
        ids.into_iter()
            .filter_map(|id| self.get(id).cloned())
            .collect()
    }

    pub fn get(&self, id: u64) -> Option<&Notification> {
        self.history.iter().find(|n| n.id == id)
    }

    /// The history, oldest first.
    pub fn history(&self) -> impl DoubleEndedIterator<Item = &Notification> + ExactSizeIterator {
        self.history.iter()
    }

    pub fn len(&self) -> usize {
        self.history.len()
    }

    pub fn is_empty(&self) -> bool {
        self.history.is_empty()
    }

    pub fn unread(&self) -> usize {
        self.history.iter().filter(|n| !n.read).count()
    }

    pub fn mark_all_read(&mut self) {
        if self.history.iter().any(|n| !n.read) {
            for n in &mut self.history {
                n.read = true;
            }
            self.revision += 1;
        }
    }

    pub fn dismiss(&mut self, id: u64) -> bool {
        let n = self.history.len();
        self.history.retain(|x| x.id != id);
        let gone = self.history.len() != n;
        if gone {
            self.revision += 1;
        }
        gone
    }

    pub fn clear(&mut self) {
        if !self.history.is_empty() {
            self.history.clear();
            self.revision += 1;
        }
    }

    /// Old entries let go because the history is bounded.
    pub fn dropped(&self) -> u64 {
        self.dropped
    }

    pub fn revision(&self) -> u64 {
        self.revision
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// M2-31 for problems: the next steps (by editor error, by code prefix, by rejection),
    /// the unexplained-problem fallbacks and the toast's composition are looked up.
    #[test]
    fn next_steps_and_toasts_are_looked_up() {
        use forge_ui::l10n::{PSEUDO_LOCALE, clear_ui_locale, is_pseudo, set_ui_locale};
        set_ui_locale(PSEUDO_LOCALE, Default::default());
        for e in crate::EditorError::all_variants_for_tests() {
            assert!(is_pseudo(e.next_step()), "{}: {}", e.code(), e.next_step());
        }
        for code in ["CMD-0003", "PROJECT-0001", "ASSET-0002", "XYZ-0001"] {
            assert!(is_pseudo(next_step_for_code(code)), "{code}");
        }
        assert!(is_pseudo(next_step_for_rejection(
            &forge_cmd::CmdError::BadName(String::new())
        )));
        let mut c = NotificationCentre::new();
        c.post_problem(Problem::coded(
            Severity::Error,
            "CMD-0003",
            forge_ui::tr!("Rename refused"),
            "no entity",
        ));
        c.post_problem(Problem::warning("oops", "", "why", ""));
        for n in c.take_toasts() {
            let t = n.toast_text();
            assert!(is_pseudo(&t), "{t}");
            assert!(is_pseudo(n.next.as_deref().unwrap_or_default()), "{t}");
        }
        assert!(is_pseudo(Level::Warning.word()));
        clear_ui_locale();
    }

    /// WP-47: an exact code's hint beats a prefix's whatever the registration order; a
    /// prefix matches only the part before the dash; a poster's own next step is kept.
    #[test]
    fn hints_prefer_an_exact_code_and_keep_a_posters_next_step() {
        let hint = |m: HintMatch, next: &str| NoticeHint {
            applies_to: m,
            next: Some(next.into()),
            open: Some(("p".into(), "Open".into())),
        };
        let hints = NoticeHints::default()
            .with(hint(HintMatch::Prefix("CMD".into()), "prefix"))
            .with(hint(HintMatch::Code("CMD-0014".into()), "exact"));
        assert_eq!(
            hints.find("CMD-0014").and_then(|h| h.next.as_deref()),
            Some("exact")
        );
        assert_eq!(
            hints.find("CMD-0001").and_then(|h| h.next.as_deref()),
            Some("prefix")
        );
        assert!(hints.find("CMDX-0001").is_none() && hints.find("CM-0001").is_none());
        let p = hints.apply(Problem::coded(
            Severity::Error,
            "CMD-0001",
            "Refused",
            "why",
        ));
        assert_eq!((p.next.as_str(), p.actions.len()), ("prefix", 1));
        // Applied twice (a re-post), the button is offered once.
        let p = hints.apply(p);
        assert_eq!(p.actions.len(), 1);
        let own = hints.apply(Problem::error("CMD-0001", "Refused", "why", "Mine."));
        assert_eq!((own.next.as_str(), own.actions.len()), ("Mine.", 1));
    }

    #[test]
    fn notifications_carry_codes_toast_once_and_stay_bounded() {
        let mut c = NotificationCentre::new();
        let id = c.post_problem(Problem::error(
            "CMD-0003",
            "Rename refused",
            "no entity",
            "Pick another entity.",
        ));
        assert_eq!(
            c.take_toasts()[0].toast_text(),
            "CMD-0003: Rename refused \u{2014} Pick another entity."
        );
        assert_eq!(c.incomplete(), 0);
        assert!(c.take_toasts().is_empty(), "a toast shows once");
        assert_eq!(c.unread(), 1);
        c.mark_all_read();
        assert_eq!(c.unread(), 0);
        assert!(c.dismiss(id));
        for i in 0..(NOTICE_HISTORY_CAP + 3) {
            c.post(Level::Info, &format!("n{i}"), "", vec![]);
        }
        assert_eq!(c.len(), NOTICE_HISTORY_CAP);
        assert_eq!(c.dropped(), 3);
    }

    #[test]
    fn positive_control_a_problem_without_a_next_step_is_counted_and_still_shown() {
        let mut c = NotificationCentre::new();
        c.post_problem(Problem::error(
            "CMD-0003",
            "Rename refused",
            "no entity",
            " ",
        ));
        c.post_problem(Problem::warning("oops", "Refused", "why", "Do this."));
        assert_eq!(c.incomplete(), 2, "both gaps are counted");
        let shown: Vec<String> = c
            .take_toasts()
            .iter()
            .map(Notification::toast_text)
            .collect();
        assert!(
            shown.iter().all(|t| t.starts_with(INCOMPLETE_PROBLEM_CODE)),
            "shown under the incomplete-problem code: {shown:?}"
        );
        // A warning posted through `post` cannot pose as one.
        c.post(Level::Error, "x", "", vec![]);
        assert_eq!(c.history().last().map(|n| n.level), Some(Level::Info));
    }
}
