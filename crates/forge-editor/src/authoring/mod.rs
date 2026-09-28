//! The authoring editors' models (Ch.21 §21.21 rows `forge.sequencer`, `forge.anim_graph`,
//! `forge.localisation`; WP-U11, ADR 0040): the sequencer's clips and tracks
//! ([`timeline`], Ch.19 animate-any-property) and its preview on the play core
//! ([`preview`]), the animation state machine, blend spaces and bone masks ([`anim`],
//! Ch.19), and the game's string tables ([`strings`], Ch.28).
//!
//! The conventions are the domain editors' ([`crate::domain`], ADR 0026): project state is
//! settings in a namespace per editor (`seq.`, `anim.`, `loc.`), every edit is a
//! `SetSetting` command (several fields at once are one transaction, one undo entry — I7),
//! parsing is total (a malformed value is skipped and reported), and what only the real
//! subsystem can answer goes through the trait the plan names — [`anim::AnimSource`]
//! (in-memory until `forge-anim`) and [`strings::StringTables`] (in-memory until
//! `forge-play`, M5-8) — with a labelled in-memory implementation (D-4).

pub mod anim;
pub mod preview;
pub mod strings;
pub mod timeline;
