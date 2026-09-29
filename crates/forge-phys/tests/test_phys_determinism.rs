//! `test_phys_determinism` — **same inputs, same state hash, on Windows and Linux** (M4-3,
//! Ch.3; gate `C-phys-determinism`).
//!
//! The determinism corpus (`forge_phys::scenes::corpus`: ~120 mixed bodies on a floor and a
//! height field, every joint kind, a trigger, a circling kinematic platform, a gravity and
//! a damping zone; scripted by `scenes::drive`: an impulse, a teleport, a removal) runs 240
//! fixed steps on each backend, and its state hash at steps 60, 120 and 240 is compared with
//! `tests/goldens/phys_state_hashes.txt`, committed. CI runs this on windows-x86_64 and
//! ubuntu-x86_64 (Ch.3.4); the same file must pass on both: that is the cross-platform
//! check. Both backends are built with `enhanced-determinism` (portable math, no SIMD, no
//! parallel solver) and every generated quantity here is a pure function of the scene.
//!
//! **A golden change needs a recorded reason (Ch.3.4).** There is no bless switch: on a
//! mismatch the test prints the complete new file and a human decides.
//!
//! Also asserted: two runs in one process are identical, and **queries and reads never
//! change the simulation** (a run that raycasts, sweeps, overlaps, batches and reads masses
//! and states between every step hashes the same as one that does not — the editor may pick
//! with rays during Play without changing the game).
//!
//! Positive control (W2): `positive_control_a_one_ulp_nudge_changes_every_hash` — the same
//! corpus with its first body's start moved by one ulp hashes differently on every backend,
//! so the golden comparison has teeth.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::BTreeMap;
use std::path::PathBuf;

use forge_frames::{DQuat, DVec3, FramePos};
use forge_phys::scenes::{self, FRAME};
use forge_phys::{Overlap, Query, QueryFilter, Ray, Shape, ShapeCast};

const GOLDEN: &str = "tests/goldens/phys_state_hashes.txt";
const CHECKPOINTS: [u64; 3] = [60, 120, 240];

fn golden_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(GOLDEN)
}

/// Run the corpus on `backend`: its hash at each checkpoint. `probe` queries and reads
/// between every step.
fn run(backend: &str, nudge: u64, probe: bool) -> Vec<(String, String)> {
    let (mut w, platform) = scenes::corpus(backend, nudge).unwrap();
    let mut out = Vec::new();
    let mut batch = Vec::new();
    for i in 0..*CHECKPOINTS.last().unwrap() {
        if probe {
            let at = |x, y, z| FramePos::new(FRAME, DVec3::new(x, y, z));
            let f = QueryFilter::default();
            let down = DVec3::new(0.0, -1.0, 0.0);
            let _ = w.raycast(
                &Ray {
                    start: at(0.5, 10.0, 0.5),
                    direction: down,
                    max_distance: 20.0,
                },
                &f,
            );
            let qs = vec![
                Query::Shape(ShapeCast {
                    shape: Shape::Sphere { radius: 0.3 },
                    start: at(-3.0, 8.0, 0.0),
                    rotation: DQuat::IDENTITY,
                    direction: down,
                    max_distance: 20.0,
                }),
                Query::Overlap(Overlap {
                    shape: Shape::Sphere { radius: 2.0 },
                    position: at(0.0, 1.0, 0.0),
                    rotation: DQuat::IDENTITY,
                }),
            ];
            w.query_batch(&qs, &f, &mut batch);
            let _ = w.mass(forge_phys::BodyId(40));
            let _ = w.state(forge_phys::BodyId(41));
            let _ = w.contact_count();
        }
        scenes::drive(&mut w, platform, i).unwrap();
        if CHECKPOINTS.contains(&w.steps()) {
            out.push((
                format!("{backend}/corpus/{}", w.steps()),
                w.state_hash().unwrap(),
            ));
        }
    }
    out
}

fn golden() -> BTreeMap<String, String> {
    let text = std::fs::read_to_string(golden_path()).unwrap_or_default();
    text.lines()
        .filter(|l| !l.starts_with('#') && !l.trim().is_empty())
        .filter_map(|l| {
            let (k, v) = l.split_once(' ')?;
            Some((k.to_owned(), v.trim().to_owned()))
        })
        .collect()
}

#[test]
fn the_state_hash_matches_the_golden_on_every_platform() {
    let want = golden();
    let mut got = BTreeMap::new();
    for b in forge_phys::FIRST_PARTY {
        got.extend(run(b, 0, false));
    }
    if got != want {
        let mut file = String::from(
            "# forge-phys determinism goldens (Ch.3.4): BLAKE3 of PhysicsWorld::state_hash for the\n\
             # scenes::corpus script at each checkpoint step, per backend. The same file must pass on\n\
             # windows-x86_64 and ubuntu-x86_64. A change needs a recorded reason (docs/adr/).\n",
        );
        for (k, v) in &got {
            file += &format!("{k} {v}\n");
        }
        panic!(
            "GOLDEN MISMATCH: {}\n--- new file ---\n{file}",
            golden_path().display()
        );
    }
}

#[test]
fn two_runs_in_one_process_are_identical() {
    for b in forge_phys::FIRST_PARTY {
        assert_eq!(run(b, 0, false), run(b, 0, false), "{b}");
    }
}

#[test]
fn queries_and_reads_never_change_the_simulation() {
    for b in forge_phys::FIRST_PARTY {
        assert_eq!(
            run(b, 0, true),
            run(b, 0, false),
            "{b}: querying changed the game"
        );
    }
}

#[test]
fn positive_control_a_one_ulp_nudge_changes_every_hash() {
    for b in forge_phys::FIRST_PARTY {
        let clean = run(b, 0, false);
        let nudged = run(b, 1, false);
        for ((k, h0), (_, h1)) in clean.iter().zip(&nudged) {
            assert_ne!(h0, h1, "{k}: a one-ulp nudge did not change the hash");
        }
    }
}
