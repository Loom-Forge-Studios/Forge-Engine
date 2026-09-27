# ADR 0012 — Build the forge-ui core on system fonts and an in-process clipboard until the owner clears the bundled font and BSL-1.0

- **Status:** accepted
- **Date:** 2026-09-20
- **Plan references:** Ch.21 §21.2, §21.7, §21.9, §21.11, §21.13, §21.22; I13; Appendix A.5;
  D-3, D-4, D-5; DoD M2-19..M2-23; WP-U1

## Context

WP-U1 builds the `forge-ui` core. Five places left a choice open, or met an obstacle the
plan did not foresee:

1. **The bundled UI font.** §21.2 names Roboto and Roboto Mono from their Apache-2.0
   releases. Fetching font files from the internet is an owner action in this
   environment, so they are not in the repository yet.
2. **The OS clipboard.** §21.2 names `arboard`. On Windows it depends on `clipboard-win`
   and `error-code`, both **BSL-1.0**. BSL-1.0 is permissive (OSI-approved, no copyleft),
   but it is not on the Appendix A.5 allow-list, `cargo xtask licence-audit` rejects it,
   and widening the list is an owner decision (the same rule ADR 0001 §7 applied to
   OFL-1.1).
3. **How a live source wakes the loop.** §21.11's `LiveSource` has `generation()` and
   `is_live()`, but rule 1 ("the producer wakes the loop through `EventLoopProxy`, at most
   once until the UI consumed the bump") needs the producer to hold a waker.
4. **Where the focus ring goes.** §21.8 requires the high-contrast focus ring to reach 3:1
   against *every* adjacent background; `test_ui_contrast` showed a ring drawn flush
   against a hovered primary button fails that in `forge.dark` (2.2:1).
5. **Tab inside panels.** §21.9 says Tab traverses "within a focus scope" and F6 cycles
   panels; it does not say what F6 does with a panel that has nothing focusable.

## Decision

1. **Fonts.** `TextSystem` loads the system font database once per process
   (`fontdb`, never redistributed) and prefers, in order, Roboto, Segoe UI, Noto Sans,
   DejaVu Sans, Liberation Sans, Cantarell (sans) and Roboto Mono, Cascadia Mono,
   Consolas, … (mono). `FontConfig::bundled` takes font bytes that override the system
   faces; the day Roboto is vendored it is passed there and nothing else changes.
   Display-list goldens record the **font fingerprint** (a hash of the resolved faces'
   bytes) and report `AWAITING(fonts differ)` on a machine whose UI fonts differ; gate
   row `C-ui-bundled-font` is `Awaiting` until the fonts are committed.
2. **Clipboard.** `forge-ui` defines the `Clipboard` trait (text + structured
   `(mime, data)` with a mandatory text fallback) and ships `InProcessClipboard`, the D-4
   in-memory implementation, labelled as such by `backend_name()`. Copy/cut/paste work
   across every widget and window of one process. The OS backend is `UNBUILT` (gate row
   `C-ui-os-clipboard`) until the owner either adds BSL-1.0 to A.5 (then `arboard` drops
   in behind the trait) or chooses another route.
3. **Live sources.** `LiveSource` gains `set_waker(Option<Arc<dyn UiWaker>>)` and
   `consumed()`. The shell registers a waker only while the feed's panel is visible and
   never for a `self_ui` feed; `LiveCell` is the standard atomic implementation that wakes
   at most once until consumed. §21.11's four rules are otherwise exactly as written.
4. **Focus ring.** The ring is drawn `FOCUS_RING_GAP` (2 logical px) outside the widget,
   so its only neighbour is the background it sits on, whatever state the widget's fill
   is in. The contrast guard checks the ring against that background in every theme.
5. **Panels.** Tab and Shift+Tab stay inside the focused widget's panel scope (wrapping);
   with no focus they cover the window. F6 skips panels that contain nothing focusable,
   and restores each panel's last-focused widget. A shown modal traps both.

Also recorded here, because they are implementation choices the plan leaves open:

- **Damage merging** collapses to one bounding rect beyond 32 rects
  (`MERGE_PAIRWISE_LIMIT`); below that, overlapping rects merge and at most 4 remain. The
  pairwise merge is quadratic, and a frame with that many damaged slices is a large change
  anyway (theme switch, relayout). Found by `ui_text_shaping_cached`: the unbounded merge
  took 87 s for a 1,000-label repaint; the bounded one takes 42 ms (debug).
- **Windows share one `Resources`** (`TextSystem` with the one glyph atlas and the shaping
  cache, plus the image atlas) through `Rc<RefCell<_>>`, UI-thread only (W7). Each `Ui`
  sets the shared text scale to its own window's scale before it lays out, paints or
  batches, so windows on monitors with different DPI shape through one cache.
- **The `Mesh` primitive and the R8 icon atlas** are not built in WP-U1: `lyon` meshes
  serve the curve, gradient and graph editors (WP-U2, WP-U8) and the Lucide icon atlas
  serves the icon set (WP-U12). The bind group already has room for both.

## Why — the owner's two rules

1. **Better for the user:** the UI is complete and testable today — shaping, IME,
   accessibility, the zero-idle loop — instead of waiting on two licensing questions the
   owner has not answered. Nothing a user sees is faked: the in-process clipboard says what
   it is, the gate shows both gaps, and the ring gap makes keyboard focus visible on every
   state of every widget (it failed WCAG 1.4.11 without it).
2. **Faster engine:** system fonts are loaded once per process and shared by every
   window; bounded damage merging keeps large repaints linear; the live-source waker keeps
   a hidden or `ui.self` feed at zero wakeups (proven by `ui_live_panel_refresh_bounded`).

## Alternatives rejected

- **Vendoring a font from another crate's test fixtures** (e.g. `fontdb`'s Tuffy, public
  domain): public domain is not on the A.5 list either, and a test fixture is not a UI font.
- **A clipboard through child processes** (`powershell Get-Clipboard`, `wl-copy`): works
  without new licences but costs hundreds of milliseconds per paste — worse for the user
  than an honest in-process clipboard.
- **Writing the Win32 clipboard code in `forge-ui`:** needs `unsafe`, and `forge-ui` is
  `#![forbid(unsafe_code)]` (Ch.1.5).
- **Polling `generation()` each frame:** a busy loop, which D-5 forbids.

## Consequences

- `C-ui-bundled-font` (Awaiting) and `C-ui-os-clipboard` (Unbuilt) are gate rows; both
  close by dropping an implementation behind an existing seam.
- Display-list goldens are exact on this machine and on any machine with the same UI
  fonts; they become portable when Roboto is bundled.
- M2-21 cannot be settled until the OS clipboard exists and RON-over-reflect has
  `forge-reflect` (WP-04) to serialise through.
