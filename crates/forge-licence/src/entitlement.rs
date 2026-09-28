//! The signed entitlement (Ch.38 §38.2): the wire type, offline signature verification against
//! an embedded public key, and the local tier / lapse / degrade evaluation. No field, function
//! or path here touches the network or the clock except [`Entitlement`]-independent `now_ms`
//! that the caller supplies — evaluation is a pure function of `(entitlement, now, version,
//! team)`, so the same code runs in the editor and in the droplet server.

#[cfg(feature = "mint")]
use serde::Serialize;

use crate::crypto::{self, PublicKey, Signature};

// The entitlement's field names and its tier/seat identifiers are **licensing tokens**: they
// are produced through `obf!` (crate::obf) so a `strings` dump of a shipped premium binary
// does not hand over the exact JSON the offline check parses and re-signs over (ADR 0063
// item 2). serde `derive` would embed them as plain text, so the wire (de)serialisation here
// is by hand, against `obf!`-revealed keys, and produces byte-identical canonical JSON to what
// the committed fixtures were signed over (proven by `tests/test_offline_verify.rs`). The
// human-readable pretty form the `mint` signer writes still uses the serde `Serialize` derive
// (gated on `mint`, never shipped) — its plain-text field names live only in the dev/test
// signer, not in the verifier a customer runs.

/// The grace period after a Team term expires before it degrades to base (E-57; O-27 open at
/// 14 days). During grace, collaboration and premium keep working and a banner warns.
pub const GRACE_MS: u64 = 14 * 24 * 60 * 60 * 1000;

/// The licence tier (Appendix A.1).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "mint", derive(Serialize))]
#[cfg_attr(feature = "mint", serde(rename_all = "snake_case", tag = "kind"))]
pub enum Tier {
    /// A $5 perpetual single-person licence: the base engine, no collaboration, no premium.
    Individual,
    /// The paid recurring tier: collaboration (Ch.37) and the premium features (E-66).
    Team {
        /// The self-declared revenue band (Ch.38 §38.4); it changes the price, not the rights.
        over_100k: bool,
    },
}

/// Which seat this machine's entitlement is (Ch.38 §38.2).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "mint", derive(Serialize))]
#[cfg_attr(feature = "mint", serde(rename_all = "snake_case"))]
pub enum SeatKind {
    Purchaser,
    Included,
    Additional,
}

/// A signed entitlement, as the editor and the server read it from disk (Ch.38 §38.2). The
/// signature (in [`SignedEntitlement`]) is over the canonical JSON of this struct.
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "mint", derive(Serialize))]
pub struct Entitlement {
    pub tier: Tier,
    pub seat: SeatKind,
    /// The team an `Included` or `Additional` seat is bound to.
    pub bound: Option<String>,
    /// Unix ms the Team term expires; `None` is perpetual (Individual).
    pub expires_ms: Option<u64>,
    /// E-59: perpetual Team rights up to this engine version (e.g. `"0.1.0"`).
    pub fallback: Option<String>,
}

/// The on-disk file: an entitlement and a detached Ed25519 signature (hex) over its canonical
/// JSON. This is what activation writes and what the offline check reads.
#[derive(Clone, Debug)]
#[cfg_attr(feature = "mint", derive(Serialize))]
pub struct SignedEntitlement {
    pub entitlement: Entitlement,
    /// The 64-byte Ed25519 signature, hex-encoded (128 characters).
    pub sig: String,
}

/// Why an entitlement file was rejected. Never a localised string: the editor maps these to
/// its own `tr!` keys; the server logs them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VerifyError {
    /// The bytes were not a valid signed-entitlement document.
    Malformed,
    /// The signature was not 64 bytes of hex.
    BadSignatureEncoding,
    /// The signature did not verify against the embedded public key (a forgery, a tampered
    /// field, or the wrong signer). This is the guard that makes a forged tier useless.
    BadSignature,
}

impl std::fmt::Display for VerifyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // These are server-log / diagnostic strings, and licensing tokens: obfuscated in a
        // shipped premium binary (ADR 0063 item 2). The editor never shows this English text
        // to a user — it maps the variant to a localisation key (ADR 0063 item 3, WP-45).
        f.write_str(&match self {
            Self::Malformed => {
                crate::obf!("the entitlement file is not a valid signed entitlement")
            }
            Self::BadSignatureEncoding => {
                crate::obf!("the entitlement signature is not 64 bytes of hex")
            }
            Self::BadSignature => crate::obf!("the entitlement signature did not verify"),
        })
    }
}

impl std::error::Error for VerifyError {}

/// The canonical bytes an entitlement's signature covers: its compact JSON, deterministic for a
/// struct with no maps. Signer and verifier both call this, so re-serialising a parsed
/// entitlement reproduces exactly what was signed regardless of the file's whitespace.
///
/// Built **by hand** (not by a serde `derive`) so the field names and tier/seat identifiers are
/// `obf!`-revealed and are not plain text in a shipped binary (ADR 0063 item 2). The output is
/// byte-identical to the derive form the committed fixtures were signed over — the field order
/// (`tier, seat, bound, expires_ms, fallback`; `kind` before `over_100k`) and the compact
/// separators match serde_json exactly (`tests/test_offline_verify.rs` proves it).
///
/// # Errors
/// If a `bound`/`fallback` string cannot be JSON-encoded (it always can; the signature is over
/// the result).
pub fn canonical_bytes(e: &Entitlement) -> Result<Vec<u8>, VerifyError> {
    let mut s = String::with_capacity(128);
    s.push('{');
    push_key(&mut s, &crate::obf!("tier"));
    push_tier(&mut s, &e.tier)?;
    s.push(',');
    push_key(&mut s, &crate::obf!("seat"));
    push_seat(&mut s, e.seat);
    s.push(',');
    push_key(&mut s, &crate::obf!("bound"));
    push_opt_str(&mut s, e.bound.as_deref())?;
    s.push(',');
    push_key(&mut s, &crate::obf!("expires_ms"));
    match e.expires_ms {
        Some(v) => s.push_str(&v.to_string()),
        None => s.push_str("null"),
    }
    s.push(',');
    push_key(&mut s, &crate::obf!("fallback"));
    push_opt_str(&mut s, e.fallback.as_deref())?;
    s.push('}');
    Ok(s.into_bytes())
}

/// Write `"key":` (the key is a plain ASCII token, so it needs no JSON escaping).
fn push_key(s: &mut String, key: &str) {
    s.push('"');
    s.push_str(key);
    s.push_str("\":");
}

/// Write a tier as serde's internally-tagged form: `{"kind":"team","over_100k":<bool>}` or
/// `{"kind":"individual"}`.
fn push_tier(s: &mut String, tier: &Tier) -> Result<(), VerifyError> {
    s.push('{');
    push_key(s, &crate::obf!("kind"));
    match tier {
        Tier::Individual => {
            push_json_str(s, &crate::obf!("individual"))?;
        }
        Tier::Team { over_100k } => {
            push_json_str(s, &crate::obf!("team"))?;
            s.push(',');
            push_key(s, &crate::obf!("over_100k"));
            s.push_str(if *over_100k { "true" } else { "false" });
        }
    }
    s.push('}');
    Ok(())
}

/// Write a seat as its snake_case JSON string.
fn push_seat(s: &mut String, seat: SeatKind) {
    let tok = match seat {
        SeatKind::Purchaser => crate::obf!("purchaser"),
        SeatKind::Included => crate::obf!("included"),
        SeatKind::Additional => crate::obf!("additional"),
    };
    // These identifiers contain no character JSON must escape.
    s.push('"');
    s.push_str(&tok);
    s.push('"');
}

/// Write an optional string field's value: `null`, or the JSON-escaped string.
fn push_opt_str(s: &mut String, v: Option<&str>) -> Result<(), VerifyError> {
    match v {
        None => {
            s.push_str("null");
            Ok(())
        }
        Some(v) => push_json_str(s, v),
    }
}

/// Write a JSON string literal (properly escaped) for an arbitrary value.
fn push_json_str(s: &mut String, v: &str) -> Result<(), VerifyError> {
    let encoded = serde_json::to_string(v).map_err(|_| VerifyError::Malformed)?;
    s.push_str(&encoded);
    Ok(())
}

/// Verify a signed-entitlement document `bytes` against `public`, offline, and return the
/// entitlement it carries. This is the whole trust boundary: an entitlement that does not come
/// back from here is never acted on.
///
/// The document is parsed into a generic `serde_json::Value` (which embeds no field-name
/// literals) and the fields are read out against `obf!`-revealed keys, then the signature is
/// checked over [`canonical_bytes`] of the parsed entitlement.
///
/// # Errors
/// [`VerifyError`] for a malformed document, a badly encoded signature, or one that does not
/// verify against `public`.
pub fn verify_signed(bytes: &[u8], public: &PublicKey) -> Result<Entitlement, VerifyError> {
    let doc: serde_json::Value =
        serde_json::from_slice(bytes).map_err(|_| VerifyError::Malformed)?;
    let obj = doc.as_object().ok_or(VerifyError::Malformed)?;
    let sig_hex = obj
        .get(crate::obf!("sig").as_str())
        .and_then(serde_json::Value::as_str)
        .ok_or(VerifyError::Malformed)?;
    let ent_val = obj
        .get(crate::obf!("entitlement").as_str())
        .ok_or(VerifyError::Malformed)?;
    let entitlement = parse_entitlement(ent_val)?;
    let sig = decode_signature(sig_hex)?;
    let msg = canonical_bytes(&entitlement)?;
    if crypto::verify(public, &msg, &sig) {
        Ok(entitlement)
    } else {
        Err(VerifyError::BadSignature)
    }
}

/// Parse an [`Entitlement`] from a JSON object read against `obf!`-revealed keys. Missing
/// `bound`/`expires_ms`/`fallback` default to `None` (as a serde `Option` field would);
/// `tier` and `seat` are required. Any wrong-typed field is [`VerifyError::Malformed`].
fn parse_entitlement(v: &serde_json::Value) -> Result<Entitlement, VerifyError> {
    use serde_json::Value;
    let obj = v.as_object().ok_or(VerifyError::Malformed)?;

    // tier: {"kind": "individual" | "team", "over_100k": bool}. Keys and identifiers are bound
    // to locals (revealed once) so the comparisons are &str == &str, not against a fresh
    // owned String (clippy::cmp_owned).
    let tier_obj = obj
        .get(crate::obf!("tier").as_str())
        .and_then(Value::as_object)
        .ok_or(VerifyError::Malformed)?;
    let kind = tier_obj
        .get(crate::obf!("kind").as_str())
        .and_then(Value::as_str)
        .ok_or(VerifyError::Malformed)?;
    let (k_individual, k_team) = (crate::obf!("individual"), crate::obf!("team"));
    let tier = if kind == k_individual.as_str() {
        Tier::Individual
    } else if kind == k_team.as_str() {
        let over_100k = tier_obj
            .get(crate::obf!("over_100k").as_str())
            .and_then(Value::as_bool)
            .ok_or(VerifyError::Malformed)?;
        Tier::Team { over_100k }
    } else {
        return Err(VerifyError::Malformed);
    };

    // seat: "purchaser" | "included" | "additional"
    let seat_str = obj
        .get(crate::obf!("seat").as_str())
        .and_then(Value::as_str)
        .ok_or(VerifyError::Malformed)?;
    let (k_purchaser, k_included, k_additional) = (
        crate::obf!("purchaser"),
        crate::obf!("included"),
        crate::obf!("additional"),
    );
    let seat = if seat_str == k_purchaser.as_str() {
        SeatKind::Purchaser
    } else if seat_str == k_included.as_str() {
        SeatKind::Included
    } else if seat_str == k_additional.as_str() {
        SeatKind::Additional
    } else {
        return Err(VerifyError::Malformed);
    };

    let bound = parse_opt_str(obj.get(crate::obf!("bound").as_str()))?;
    let expires_ms = parse_opt_u64(obj.get(crate::obf!("expires_ms").as_str()))?;
    let fallback = parse_opt_str(obj.get(crate::obf!("fallback").as_str()))?;

    Ok(Entitlement {
        tier,
        seat,
        bound,
        expires_ms,
        fallback,
    })
}

/// An absent or `null` value is `None`; a string is `Some`; anything else is malformed.
fn parse_opt_str(v: Option<&serde_json::Value>) -> Result<Option<String>, VerifyError> {
    match v {
        None => Ok(None),
        Some(v) if v.is_null() => Ok(None),
        Some(serde_json::Value::String(s)) => Ok(Some(s.clone())),
        Some(_) => Err(VerifyError::Malformed),
    }
}

/// An absent or `null` value is `None`; a u64 is `Some`; anything else is malformed.
fn parse_opt_u64(v: Option<&serde_json::Value>) -> Result<Option<u64>, VerifyError> {
    match v {
        None => Ok(None),
        Some(v) if v.is_null() => Ok(None),
        Some(v) => v.as_u64().map(Some).ok_or(VerifyError::Malformed),
    }
}

/// Sign an entitlement with the store's seed, producing the on-disk document. `mint` only:
/// the offline signer and the tests, never the shipped verifier.
///
/// # Errors
/// If the entitlement cannot be serialised.
#[cfg(feature = "mint")]
pub fn sign_entitlement(
    e: &Entitlement,
    seed: &[u8; 32],
) -> Result<SignedEntitlement, VerifyError> {
    let msg = canonical_bytes(e)?;
    let sig = crypto::sign(seed, &msg);
    Ok(SignedEntitlement {
        entitlement: e.clone(),
        sig: encode_hex(&sig),
    })
}

/// Decode a 128-character hex signature into 64 bytes.
fn decode_signature(hex: &str) -> Result<Signature, VerifyError> {
    let bytes = hex.as_bytes();
    if bytes.len() != 128 {
        return Err(VerifyError::BadSignatureEncoding);
    }
    let mut out = [0u8; 64];
    let mut i = 0;
    while i < 64 {
        let hi = hex_nibble(bytes[i * 2]).ok_or(VerifyError::BadSignatureEncoding)?;
        let lo = hex_nibble(bytes[i * 2 + 1]).ok_or(VerifyError::BadSignatureEncoding)?;
        out[i] = (hi << 4) | lo;
        i += 1;
    }
    Ok(out)
}

fn hex_nibble(c: u8) -> Option<u8> {
    match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'a'..=b'f' => Some(c - b'a' + 10),
        b'A'..=b'F' => Some(c - b'A' + 10),
        _ => None,
    }
}

/// Hex-encode bytes (lowercase). `mint` only (the signer emits it).
#[cfg(feature = "mint")]
fn encode_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut s = String::with_capacity(bytes.len() * 2);
    for &b in bytes {
        s.push(HEX[(b >> 4) as usize] as char);
        s.push(HEX[(b & 0xf) as usize] as char);
    }
    s
}

/// What a verified entitlement means right now, evaluated locally (see the module docs). The
/// collaboration gate and the premium gate are both read off this.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Standing {
    /// No entitlement, or an Individual licence: the base engine only.
    Base,
    /// A valid, active Team entitlement: collaboration and premium are available.
    Team,
    /// A Team term expired but the grace period still runs: still available, warn and renew.
    Grace {
        /// When grace ends (Unix ms); after this, [`Standing::Lapsed`].
        until_ms: u64,
    },
    /// A Team term lapsed past grace with no covering fallback: degrade to base (E-57). The
    /// project still opens, builds and ships — only the paid features go dark.
    Lapsed {
        /// When the term expired (Unix ms), for the banner.
        since_ms: u64,
    },
    /// A bound seat opened against a project whose team is not the one it is bound to: base for
    /// this project (a personal project wants a $5 Individual licence).
    SeatBound {
        /// The team the seat is bound to.
        team: String,
    },
}

/// Evaluate an entitlement locally at `now_ms`, for this build's `version`, for a project of
/// `team` (`None`: not a team project, or the caller does not gate on the project's team — as
/// the premium gate does not). Pure; no network, no ambient clock.
#[must_use]
pub fn evaluate(
    e: Option<&Entitlement>,
    now_ms: u64,
    version: &str,
    team: Option<&str>,
) -> Standing {
    let Some(e) = e else {
        return Standing::Base;
    };
    if e.tier == Tier::Individual {
        return Standing::Base;
    }
    // A Team term. Is it expired, and if so is the running version covered by a fallback?
    if let Some(exp) = e.expires_ms
        && now_ms >= exp
    {
        let covered = e
            .fallback
            .as_deref()
            .is_some_and(|f| version_le(version, f));
        if !covered {
            let grace_end = exp.saturating_add(GRACE_MS);
            if now_ms < grace_end {
                // Grace: still Team, but a seat bound elsewhere is still base for the project.
                return seat_or(
                    e,
                    team,
                    Standing::Grace {
                        until_ms: grace_end,
                    },
                );
            }
            return Standing::Lapsed { since_ms: exp };
        }
    }
    seat_or(e, team, Standing::Team)
}

/// If the entitlement is a bound seat used outside its team, return [`Standing::SeatBound`];
/// otherwise the `active` standing (Team or Grace).
fn seat_or(e: &Entitlement, team: Option<&str>, active: Standing) -> Standing {
    if matches!(e.seat, SeatKind::Included | SeatKind::Additional)
        && let Some(t) = team
        && e.bound.as_deref() != Some(t)
    {
        return Standing::SeatBound {
            team: e.bound.clone().unwrap_or_default(),
        };
    }
    active
}

impl Standing {
    /// Are the paid features (collaboration; and, in the premium edition, the premium
    /// features) available? True in [`Standing::Team`] and during [`Standing::Grace`].
    #[must_use]
    pub fn active(&self) -> bool {
        matches!(self, Self::Team | Self::Grace { .. })
    }

    /// The Team term lapsed past grace: the caller degrades to base and shows a renew banner.
    #[must_use]
    pub fn lapsed(&self) -> bool {
        matches!(self, Self::Lapsed { .. })
    }
}

/// Is dotted version `v` at or below `max`? (Compares the first three numeric components.)
#[must_use]
pub fn version_le(v: &str, max: &str) -> bool {
    let parse = |s: &str| -> Vec<u64> {
        s.trim()
            .split(['.', '-', '+'])
            .take(3)
            .map(|p| p.parse::<u64>().unwrap_or(0))
            .collect()
    };
    parse(v) <= parse(max)
}

/// A calendar date for people (`2026-09-23`, UTC) from Unix ms — for banners and server logs.
#[must_use]
pub fn date(ms: u64) -> String {
    // Civil-from-days (Howard Hinnant), integer only.
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
    fn no_entitlement_and_individual_are_base() {
        assert_eq!(evaluate(None, 0, "0.1.0", None), Standing::Base);
        let mut i = team(None);
        i.tier = Tier::Individual;
        assert_eq!(evaluate(Some(&i), 0, "0.1.0", None), Standing::Base);
    }

    #[test]
    fn team_is_active_then_grace_then_lapsed() {
        let exp = 1000 * DAY;
        let e = team(Some(exp));
        assert_eq!(evaluate(Some(&e), exp - 1, "0.1.0", None), Standing::Team);
        assert!(evaluate(Some(&e), exp - 1, "0.1.0", None).active());
        let grace = evaluate(Some(&e), exp + DAY, "0.1.0", None);
        assert_eq!(
            grace,
            Standing::Grace {
                until_ms: exp + GRACE_MS
            }
        );
        assert!(grace.active() && !grace.lapsed());
        let lapsed = evaluate(Some(&e), exp + GRACE_MS, "0.1.0", None);
        assert_eq!(lapsed, Standing::Lapsed { since_ms: exp });
        assert!(lapsed.lapsed() && !lapsed.active());
    }

    #[test]
    fn a_fallback_covers_its_versions_forever() {
        let exp = 1000 * DAY;
        let mut e = team(Some(exp));
        e.fallback = Some("0.2.0".into());
        // Long past grace, but the running version is within the fallback: still Team.
        assert_eq!(
            evaluate(Some(&e), exp + 100 * GRACE_MS, "0.1.5", None),
            Standing::Team
        );
        // A newer version than the fallback covers: lapsed.
        assert!(evaluate(Some(&e), exp + 100 * GRACE_MS, "0.3.0", None).lapsed());
    }

    #[test]
    fn a_bound_seat_is_base_outside_its_team() {
        let mut seat = team(None);
        seat.seat = SeatKind::Included;
        seat.bound = Some("team-1".into());
        assert_eq!(
            evaluate(Some(&seat), 0, "0.1.0", Some("team-1")),
            Standing::Team
        );
        assert_eq!(
            evaluate(Some(&seat), 0, "0.1.0", Some("team-9")),
            Standing::SeatBound {
                team: "team-1".into()
            }
        );
        // The premium gate passes team=None, so a bound seat is Team for premium purposes.
        assert_eq!(evaluate(Some(&seat), 0, "0.1.0", None), Standing::Team);
    }

    #[test]
    fn dates_read_as_utc_days() {
        assert_eq!(date(0), "1970-01-01");
        assert_eq!(date(1_790_121_600_000), "2026-09-23");
    }
}
