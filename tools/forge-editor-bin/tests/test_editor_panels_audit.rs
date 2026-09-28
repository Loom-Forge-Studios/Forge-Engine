//! Editor-wide audits of **every panel** as a user gets them (WP-U12; Ch.21 §21.21
//! "Requirements on every panel", §21.10, §21.8, §21.9; DoD M2-70, M2-31, M2-22's audit):
//! the real first-party plugin set (`forge_editor_bin::first_party`), each panel opened
//! alone in the headless editor loop. The screen-reader, contrast, keyboard and pseudo-locale
//! audits run **twice**: on an empty project in an editor with no services, and on a
//! populated project in the editor as the binary wires it (`forge_editor_bin::wiring`: asset
//! database, connect services, in-memory team server and licence) with a user config
//! directory, so every first-run tip shows. The populated pass is checked for reach
//! (`the_populated_pass_reaches_the_panels_content`).
//!
//! The audits themselves live in `tests/panels_audit/mod.rs`, shared with every edition that
//! adds panels (WP-47): an edition runs the same audits over the first-party set plus its own
//! plugins, so its panels meet exactly these rules. This file runs them over the base editor.
//!
//! * `test_panel_empty_states` (gate `C-panel-empty-states`): every panel shows its empty
//!   state from the catalogue widget (`EmptyState`: text and at most one action) — or it
//!   declared that it always has content (`PanelCx::never_empty`) and shows some. A panel
//!   that renders a blank area fails. Controls: a blank panel; a panel that declares it is
//!   never empty and shows nothing.
//! * First-run tips (gate `C-first-run-tips`): every first-party panel has a tip; it shows
//!   on first open (with a user config directory), takes no focus, dismisses to nothing —
//!   **0 frames and 0 wakeups** over 10 s after — and stays dismissed. Control: a dismissed
//!   tip that keeps animating (`PanelFaults::tip_keeps_timer`).
//! * Screen-reader tree (gate `C-editor-a11y`): every visible focusable widget of every
//!   panel has a role, a non-empty accessible name and the Focus action in the AccessKit
//!   tree. Control: an unlabelled icon button in a panel.
//! * Contrast (gate `C-editor-contrast`): every `(fg, bg)` pair the panels paint, in all
//!   three themes, meets its theme's floor (§21.8). Control: a theme with one low-contrast
//!   token.
//! * Keyboard-only walkthrough (gate `C-keyboard-walkthrough`, M2-31): Tab from the panel's
//!   first control visits every visible focusable widget of the panel and comes back, each
//!   stop named. The record is written to `docs/evidence/keyboard-walkthrough.md` when
//!   `FORGE_WRITE_EVIDENCE=1`. Control: a panel whose inner focus scope traps Tab.
//! * Pseudo-locale (M2-31): every visible string is a localisation key. Controls: a literal
//!   label; a literal row in a virtualised tree.
//! * Idle with every panel open (§21.22, gate `C-ui-idle-every-panel`): 0 frames and 0
//!   wakeups. Control: an always-animating panel.

mod panels_audit;

use panels_audit::Edition;

#[test]
fn the_populated_pass_reaches_the_panels_content() {
    panels_audit::populated_pass_reach(&Edition::base());
}

#[test]
fn test_panel_empty_states() {
    panels_audit::empty_states(&Edition::base());
}

#[test]
fn positive_control_a_blank_panel_fails_the_empty_state_audit() {
    panels_audit::control_blank_panel(&Edition::base());
}

#[test]
fn every_panel_has_a_first_run_tip_that_never_blocks_and_never_schedules_once_dismissed() {
    panels_audit::tips(&Edition::base());
}

#[test]
fn positive_control_a_dismissed_tip_that_keeps_a_timer_fails() {
    panels_audit::control_tip_timer(&Edition::base());
}

#[test]
fn no_tips_without_a_user_config_directory_or_when_turned_off() {
    panels_audit::no_tips_without_config(&Edition::base());
}

#[test]
fn every_focusable_widget_of_every_panel_has_a_role_and_a_name() {
    panels_audit::a11y(&Edition::base());
}

#[test]
fn positive_control_an_unlabelled_icon_button_in_a_panel_fails() {
    panels_audit::control_unlabelled(&Edition::base());
}

#[test]
fn every_pair_the_panels_paint_meets_its_theme_floor_in_all_three_themes() {
    panels_audit::contrast(&Edition::base());
}

#[test]
fn positive_control_a_low_contrast_token_fails_the_panel_contrast_audit() {
    panels_audit::control_low_contrast(&Edition::base());
}

#[test]
fn keyboard_only_walkthrough_of_every_panel() {
    panels_audit::walkthrough(&Edition::base());
}

#[test]
fn positive_control_a_focus_trap_fails_the_walkthrough() {
    panels_audit::control_focus_trap(&Edition::base());
}

#[test]
fn test_pseudo_locale_no_hardcoded_strings() {
    panels_audit::pseudo_locale(&Edition::base());
}

#[test]
fn positive_control_one_literal_string_in_a_panel_fails() {
    panels_audit::control_literal_string(&Edition::base());
}

#[test]
fn positive_control_one_literal_row_in_a_virtual_tree_fails() {
    panels_audit::control_literal_row(&Edition::base());
}

#[test]
fn ui_idle_zero_redraw_with_every_panel_open() {
    panels_audit::idle(&Edition::base());
}

#[test]
fn positive_control_an_animating_panel_breaks_idle_with_every_panel_open() {
    panels_audit::control_spinning(&Edition::base());
}
