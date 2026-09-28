# Bundled UI fonts (Ch.21 §21.2, D-7, ADR 0013)

| File | Face | Licence | Used for |
|---|---|---|---|
| `Roboto-Regular.ttf` | Roboto 400 | Apache-2.0 | UI text |
| `Roboto-Italic.ttf` | Roboto 400 italic | Apache-2.0 | emphasis |
| `Roboto-Medium.ttf` | Roboto 500 | Apache-2.0 | medium weight |
| `Roboto-Bold.ttf` | Roboto 700 | Apache-2.0 | headings, strong (600 resolves here) |
| `DroidSansMono.ttf` | Droid Sans Mono | Apache-2.0 | monospace (interim, see below) |

All five are Google fonts published under the Apache License 2.0 (`LICENSE-Apache-2.0.txt`,
"Font data copyright Google 2012"). They are the unmodified files; NOTICES credits them.

**Provenance.** The task asked for the Apache-2.0 releases on the googlefonts GitHub. Roboto
here is the 2012 release (name table "Version 1.00000").

**Interim mono face.** No Apache-2.0 Roboto Mono was available locally. Droid Sans Mono
(Roboto Mono's Android-lineage predecessor, same licence) stands in. Swapping in
`RobotoMono-Regular.ttf` from the Apache-2.0 v2 release is one file plus one line in
`text.rs` (`BUNDLED_MONO_FAMILY`), then `FORGE_BLESS=1` for the goldens.

Other scripts (CJK, Arabic, Thai, …) and emoji are not bundled: on the first glyph the
bundle lacks, `TextSystem` loads the user's installed fonts once (lazily) and falls back
to them. They are never redistributed.

**Gate status.** The orchestrator amended D-7: the Apache-2.0 files above (the Roboto 2012
set and Droid Sans Mono) **are** the bundled set; nothing is downloaded. Row
`C-ui-bundled-font` is BOUND in `tests/test_ui_bundled_font.rs`: UI text resolves to these
faces, byte for byte the ones the display-list goldens were blessed with, so the goldens
compare for real on every machine (positive control: a text system without the bundle
fails). Row `C-ui-lazy-font-fallback` guards the lazy system fallback by observing the font
database itself (its face count and the merge counter inside the one function that merges).
