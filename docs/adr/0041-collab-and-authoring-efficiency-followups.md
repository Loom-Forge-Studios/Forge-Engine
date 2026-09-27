# ADR 0041 — Sandbox upkeep off the bus stream, the story as metadata, and per-change editor work

- **Status:** accepted
- **Date:** 2026-09-23
- **Plan references:** Ch.21 §21.20, §21.21; Ch.33 §33.4; Ch.37 (I19, I20); I7; D-5;
  DoD M2-29, M2-35, M2-58, M2-63; ADR 0039, ADR 0040

(Numbered 0041: `dev` holds up to 0040; D-9.)

## Context

The WP-U10 and WP-U11 verifiers left follow-ups (WP-U14): a sandbox row held back by any
open transaction; a membership check that cloned the team record on every pump; sandbox
transaction counts kept by a call at every cancel / undo / redo site; a folded command list
that looked like a replayable log; a hierarchy that repainted for edits it does not show; a
sequencer that re-showed every track per key-drag frame and painted every key; a runtime
sample that never advanced its message buffers and quit with `process::exit`.

## Decision

1. **A sandbox row waits only for the transaction that moved it.** The deltas keep the (few)
   transactions whose commands moved the summary since the last row; the row goes as soon as
   one of them is no longer open. Frames of an open gesture still send none.
2. **Membership is memoised** on the identity database's and the server's generations and
   the team setting (borrowed): an idle pump allocates nothing (rule 2). The status cache is
   keyed to the same generations, so a removed member stops reading as joined.
3. **Transaction-count upkeep is a bus-side hook.** The core's own view of the `Applied`
   stream (drained before its lock is released and before the sandbox is read) moves a
   transaction's count on every Undo / Redo / Cancel event, whichever path asked the bus;
   a stream gap recomputes from `Bus::txn_state`. The per-call-site calls are gone, so a
   new path cannot drift silently.
4. **The folded sandbox list is metadata, never replayed.** It is `Publish::envelopes`: the
   revision's summary, command count and audit trail. The server commits `Publish::doc`.
   Folding, undo filtering, absent pulls and re-keying make the list unreplayable by design
   — replayed after a re-keying pull it edits a teammate's entity — so nothing replays it.
5. **Editor views do work per change they show.** The hierarchy edits its tree only for a
   change a row shows (name, place, hidden/locked). The sequencer re-shows only the tracks
   the change log re-read (`TimelineDoc::apply_changes` now returns them) when that is all
   that changed; the timeline paints only keys in the visible time range, one mark per
   pixel column (selected keys always). The unused `TimelinePreview::next_frame` is
   removed: a playing preview is driven by the timeline's own frame loop.
6. **The runtime sample runs a real game loop.** `end_turn` advances `Messages<GameEvent>`
   (`Messages::update`) then delivers; Quit asks the runner to end (`UiApp::exit_requested`,
   additive) so `exiting` runs and the run report prints. Credits headings are localised
   (`credits.section.<heading>`, rebuilt on a language switch); credit lines (names,
   licences, the E-64 entry) are shown as written and exempt from the pseudo-locale audit,
   which now covers every screen. `GameMenu::update` compares without allocating.

## Consequences

Guards (W1/W2, each with a positive control): `C-sandbox-row-follows-its-txn`,
`C-collab-pump-no-alloc`, `C-sandbox-txn-hook`, `C-sandbox-story-metadata`,
`C-hierarchy-hidden-edits-no-damage`, `C-sequencer-drag-cost`, `C-game-messages-bounded`;
`C-authoring-panels-idle` now plays then pauses (control: a pause that keeps playing).
Measured: an idle follow in a team of 61 went from 186 allocations to 0; 40 unshown edits
from 40 hierarchy accessibility rebuilds to 0; a 20-frame key drag over 21 tracks from 315
timeline rows rebuilt to 15; a paint of 20 dense rows from 7,602 key marks to 5,759 (keys
past the clip's end culled).
