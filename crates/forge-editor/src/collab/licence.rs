//! **Licence and entitlement status** (Ch.38 §38.2, E-57, I21; DoD M2-62): what the licence
//! panel shows and what the collaboration commands are gated on — **evaluated locally,
//! with no network call anywhere in the path**.
//!
//! The four rules of Ch.38 §38.2, all local:
//!
//! 1. `Tier::Individual` → the Ch.37 team and collaboration features are unavailable.
//! 2. `Tier::Team` → they are available.
//! 3. A seat that is `Included` or `Additional` works only for projects of its `bound` team.
//! 4. `expires` in the past, past the grace period, and no `fallback` covering the running
//!    version → **degrade to Individual** (E-57). **Never lock out**: every project opens,
//!    edits, saves, builds and ships; only the collaboration commands are refused, the panels
//!    grey, and a banner offers renewal.
//!
//! The grace period is [`GRACE_MS`] (14 days) until O-27 settles it. Renewal is opportunistic
//! ([`EntitlementSource::renew`]): never a precondition for anything.
//!
//! The editor binary reads the signed entitlement file, verified offline by `forge-licence`
//! (`forge_editor_bin::licence`, WP-47). [`MemoryEntitlement`] is the labelled in-memory source
//! (D-4) the panels' tests and the editor-wide audits pin a licence with.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, PoisonError};

/// The grace period after expiry before collaboration degrades (O-27 open: 14 days).
pub const GRACE_MS: u64 = 14 * 24 * 60 * 60 * 1000;

/// The licence tier (Appendix A).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tier {
    Individual,
    /// A team licence (self-declared revenue threshold, Ch.38 §38.4).
    Team {
        over_100k: bool,
    },
}

impl Tier {
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Individual => forge_ui::tr!("Individual"),
            Self::Team { over_100k: false } => forge_ui::tr!("Team (under $100k)"),
            Self::Team { over_100k: true } => forge_ui::tr!("Team (over $100k)"),
        }
    }
}

/// Which seat this machine's entitlement is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SeatKind {
    Purchaser,
    Included,
    Additional,
}

impl SeatKind {
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Purchaser => forge_ui::tr!("purchaser"),
            Self::Included => forge_ui::tr!("included seat"),
            Self::Additional => forge_ui::tr!("additional seat"),
        }
    }
}

/// A signed entitlement, as the editor reads it from disk (Ch.38 §38.2).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entitlement {
    pub tier: Tier,
    pub seat: SeatKind,
    /// The team an included or additional seat is bound to.
    pub bound: Option<String>,
    /// Unix ms; `None`: perpetual (Individual).
    pub expires_ms: Option<u64>,
    /// E-59: perpetual Team rights up to this version (`0.1.0`).
    pub fallback: Option<String>,
}

/// What the evaluation says (see the module docs).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LicenceState {
    /// The entitlement on disk, if any.
    pub entitlement: Option<Entitlement>,
    /// Collaboration is available (rules 2, 3 and 4 hold).
    pub collaboration: bool,
    /// Why not, when it is not: shown in the banner.
    pub why_not: Option<String>,
    /// The Team entitlement expired and the grace period runs until this time (ms).
    pub grace_until_ms: Option<u64>,
    /// The subscription lapsed (rule 4): a banner and a renew button.
    pub lapsed: bool,
}

/// Is version `v` at or below `max` (dotted numbers)?
fn version_le(v: &str, max: &str) -> bool {
    let parse = |s: &str| -> Vec<u64> {
        s.trim()
            .split(['.', '-', '+'])
            .take(3)
            .map(|p| p.parse::<u64>().unwrap_or(0))
            .collect()
    };
    parse(v) <= parse(max)
}

/// A date for people (UTC, `2026-09-23`), from Unix ms.
#[must_use]
pub fn date(ms: u64) -> String {
    // Civil-from-days (Howard Hinnant's algorithm), integer only.
    let days = (ms / 86_400_000) as i64;
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!("{y:04}-{m:02}-{d:02}")
}

/// Evaluate an entitlement locally (see the module docs): at `now_ms`, for this editor's
/// `version`, for a project of `team` (`None`: not a team project).
#[must_use]
pub fn evaluate(
    e: Option<&Entitlement>,
    now_ms: u64,
    version: &str,
    team: Option<&str>,
) -> LicenceState {
    let mut s = LicenceState {
        entitlement: e.cloned(),
        collaboration: false,
        why_not: None,
        grace_until_ms: None,
        lapsed: false,
    };
    let Some(e) = e else {
        s.why_not = Some(
            forge_ui::tr!(
                "no licence is activated on this machine: collaboration needs a Team licence (everything else works)"
            )
            .into(),
        );
        return s;
    };
    if e.tier == Tier::Individual {
        s.why_not = Some(
            forge_ui::tr!(
                "an Individual licence: teams and collaboration need a Team licence (everything else works)"
            )
            .into(),
        );
        return s;
    }
    if let Some(exp) = e.expires_ms
        && now_ms >= exp
    {
        let covered = e
            .fallback
            .as_deref()
            .is_some_and(|f| version_le(version, f));
        let grace_end = exp.saturating_add(GRACE_MS);
        if !covered {
            if now_ms < grace_end {
                s.grace_until_ms = Some(grace_end);
            } else {
                s.lapsed = true;
                s.why_not = Some(forge_ui::trf!(
                    "the Team subscription lapsed on {date}: collaboration is paused until it is renewed. Your projects, building and shipping are unaffected",
                    date = date(exp)
                ));
                return s;
            }
        }
    }
    if matches!(e.seat, SeatKind::Included | SeatKind::Additional)
        && let Some(t) = team
        && e.bound.as_deref() != Some(t)
    {
        s.why_not = Some(forge_ui::trf!(
            "this seat is bound to team {team}, not to this project's team: collaboration is available in that team's projects",
            team = e.bound.as_deref().unwrap_or(forge_ui::tr!("(none)"))
        ));
        return s;
    }
    s.collaboration = true;
    s
}

/// Where the entitlement comes from (the signed file on disk, verified offline).
pub trait EntitlementSource: Send + Sync {
    /// What serves it (`in-memory stand-in (D-4)`).
    fn backend(&self) -> &'static str;
    /// The entitlement on disk (no network).
    fn entitlement(&self) -> Option<Entitlement>;
    /// Now, on the machine's clock (Ch.38 §38.2: honour-system, accepted).
    fn now_ms(&self) -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
    }
    /// Fetch a fresh entitlement opportunistically (only when a person asks, or the machine
    /// happens to be online): never a precondition for starting, opening, building or
    /// shipping. What happened, for the panel.
    fn renew(&self) -> String;
    /// Bumped when the entitlement changes.
    fn generation(&self) -> u64;
    /// Network attempts made so far (I21: the status path makes none).
    fn network_calls(&self) -> u64;
    /// Call `f` when the entitlement changes (a renewal, a new file on disk). The default
    /// never calls it.
    fn observe(&self, f: forge_project::collab::Observer) {
        drop(f);
    }
}

/// The labelled in-memory entitlement source (D-4): tests and the audits pin a licence with it;
/// the editor binary reads the signed file (`forge_editor_bin::licence`).
pub struct MemoryEntitlement {
    e: Mutex<Option<Entitlement>>,
    now: Mutex<Option<u64>>,
    generation: AtomicU64,
    calls: AtomicU64,
    observers: Mutex<Vec<forge_project::collab::Observer>>,
}

impl std::fmt::Debug for MemoryEntitlement {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MemoryEntitlement")
            .field("entitlement", &self.entitlement())
            .finish_non_exhaustive()
    }
}

impl MemoryEntitlement {
    /// A source holding `e`.
    #[must_use]
    pub fn new(e: Option<Entitlement>) -> Self {
        Self {
            e: Mutex::new(e),
            now: Mutex::new(None),
            generation: AtomicU64::new(0),
            calls: AtomicU64::new(0),
            observers: Mutex::new(Vec::new()),
        }
    }
    /// A Team purchaser's entitlement for a year from `now_ms` (the editor's default
    /// stand-in, so the collaboration panels can be tried: it says it is in-memory).
    #[must_use]
    pub fn team_for_a_year(now_ms: u64) -> Self {
        Self::new(Some(Entitlement {
            tier: Tier::Team { over_100k: false },
            seat: SeatKind::Purchaser,
            bound: None,
            expires_ms: Some(now_ms.saturating_add(365 * 86_400_000)),
            fallback: None,
        }))
    }
    /// Pin the clock (tests: the day after expiry).
    pub fn set_now(&self, ms: Option<u64>) {
        *self.now.lock().unwrap_or_else(PoisonError::into_inner) = ms;
        self.generation.fetch_add(1, Ordering::AcqRel);
    }
    /// Replace the entitlement (a new file on disk).
    pub fn set(&self, e: Option<Entitlement>) {
        *self.e.lock().unwrap_or_else(PoisonError::into_inner) = e;
        self.generation.fetch_add(1, Ordering::AcqRel);
        self.changed();
    }

    fn changed(&self) {
        let list: Vec<forge_project::collab::Observer> = self
            .observers
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        for f in list {
            f();
        }
    }
}

impl EntitlementSource for MemoryEntitlement {
    fn backend(&self) -> &'static str {
        forge_project::collab::IN_MEMORY
    }
    fn entitlement(&self) -> Option<Entitlement> {
        self.e
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }
    fn now_ms(&self) -> u64 {
        let pinned = *self.now.lock().unwrap_or_else(PoisonError::into_inner);
        pinned.unwrap_or_else(|| {
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
        })
    }
    fn renew(&self) -> String {
        // The stand-in "reaches the store" and extends a Team term by a year.
        self.calls.fetch_add(1, Ordering::AcqRel);
        let now = self.now_ms();
        let mut e = self.e.lock().unwrap_or_else(PoisonError::into_inner);
        match e.as_mut() {
            Some(x) if x.tier != Tier::Individual => {
                x.expires_ms = Some(now.saturating_add(365 * 86_400_000));
                drop(e);
                self.generation.fetch_add(1, Ordering::AcqRel);
                self.changed();
                format!(
                    "Renewed (in-memory stand-in): valid until {}",
                    date(now.saturating_add(365 * 86_400_000))
                )
            }
            _ => "There is no Team subscription to renew: buy a Team licence on the website".into(),
        }
    }
    fn generation(&self) -> u64 {
        self.generation.load(Ordering::Acquire)
    }
    fn network_calls(&self) -> u64 {
        self.calls.load(Ordering::Acquire)
    }
    fn observe(&self, f: forge_project::collab::Observer) {
        self.observers
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(f);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DAY: u64 = 86_400_000;

    fn team(expires: Option<u64>) -> Entitlement {
        Entitlement {
            tier: Tier::Team { over_100k: false },
            seat: SeatKind::Purchaser,
            bound: None,
            expires_ms: expires,
            fallback: None,
        }
    }

    #[test]
    fn lapse_degrades_after_grace_and_a_fallback_covers_its_versions() {
        let exp = 1000 * DAY;
        let e = team(Some(exp));
        assert!(evaluate(Some(&e), exp - 1, "0.1.0", None).collaboration);
        let grace = evaluate(Some(&e), exp + DAY, "0.1.0", None);
        assert!(grace.collaboration && grace.grace_until_ms == Some(exp + GRACE_MS));
        let lapsed = evaluate(Some(&e), exp + GRACE_MS, "0.1.0", None);
        assert!(!lapsed.collaboration && lapsed.lapsed);
        assert!(lapsed.why_not.unwrap_or_default().contains("unaffected"));
        let mut f = e.clone();
        f.fallback = Some("0.2.0".into());
        assert!(evaluate(Some(&f), exp + GRACE_MS, "0.1.5", None).collaboration);
        assert!(!evaluate(Some(&f), exp + GRACE_MS, "0.3.0", None).collaboration);
    }

    #[test]
    fn individual_and_a_seat_bound_elsewhere_have_no_collaboration() {
        let mut i = team(None);
        i.tier = Tier::Individual;
        assert!(!evaluate(Some(&i), 0, "0.1.0", None).collaboration);
        assert!(!evaluate(None, 0, "0.1.0", None).collaboration);
        let mut seat = team(None);
        seat.seat = SeatKind::Included;
        seat.bound = Some("team-1".into());
        assert!(evaluate(Some(&seat), 0, "0.1.0", Some("team-1")).collaboration);
        assert!(!evaluate(Some(&seat), 0, "0.1.0", Some("team-9")).collaboration);
    }

    #[test]
    fn dates_read_as_utc_days() {
        assert_eq!(date(0), "1970-01-01");
        assert_eq!(date(1_790_121_600_000), "2026-09-23");
    }
}
