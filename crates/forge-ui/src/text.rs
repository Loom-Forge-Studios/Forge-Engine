//! Text: `cosmic-text` shaping behind a cache, and the one glyph atlas (Ch.21 §21.7).
//!
//! * Every text-bearing widget owns a [`GlyphRunId`] into the **shaping cache**, keyed by
//!   `(text hash, attrs, font size × scale factor, wrap-width bucket)`. Unchanged text
//!   never re-shapes; [`TextStats::shape_calls`] counts the misses so
//!   `ui_text_shaping_cached` can prove it.
//! * There is **one** glyph atlas ([`GlyphAtlas`]) with two planes: an `R8Unorm` coverage
//!   mask for outline glyphs and an `Rgba8UnormSrgb` colour plane for emoji/COLR glyphs.
//!   It starts at 5 MB and is capped at 32 MB. When a frame's working set does not fit
//!   after eviction, the frame is drawn in **atlas epochs** (see [`crate::render::batch`]).

use std::collections::HashMap;
use std::sync::OnceLock;

use cosmic_text::{
    Attrs, Buffer, CacheKey, Family, FontSystem, Metrics, Shaping, Style as FontStyle, SwashCache,
    SwashContent, Weight, Wrap,
};
use etagere::{AllocId, BucketedAtlasAllocator, size2};

use crate::geom::Size;
use crate::id::{Fnv, hash_str, splitmix};
use crate::render::{TextureId, TextureUpdate, TextureUpdateKind};

/// Text attributes a widget chooses from theme tokens.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct TextStyle {
    /// Font size in logical pixels.
    pub size: f32,
    /// Line height as a multiple of `size`.
    pub line_height: f32,
    pub weight: u16,
    pub italic: bool,
    pub mono: bool,
    /// Wrap at the available width (only wrapping text has a width in its cache key).
    pub wrap: bool,
}

impl TextStyle {
    pub fn body(size: f32) -> Self {
        Self {
            size,
            line_height: 1.35,
            weight: 400,
            italic: false,
            mono: false,
            wrap: false,
        }
    }
    pub fn strong(mut self) -> Self {
        self.weight = 600;
        self
    }
    pub fn wrapping(mut self) -> Self {
        self.wrap = true;
        self
    }
    fn hash_into(&self, h: &mut Fnv) {
        h.f32(self.size);
        h.f32(self.line_height);
        h.u32(u32::from(self.weight));
        h.byte(u8::from(self.italic) | (u8::from(self.mono) << 1) | (u8::from(self.wrap) << 2));
    }
}

/// One span of rich text (`TextSystem::layout_rich`).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct RichSpan<'a> {
    pub text: &'a str,
    /// Overrides the base weight.
    pub weight: Option<u16>,
    pub italic: bool,
    pub mono: bool,
    /// Returned per glyph (`glyph_tag`): a colour-palette index or a link id.
    pub tag: u16,
}

impl<'a> RichSpan<'a> {
    pub const fn plain(text: &'a str) -> Self {
        Self {
            text,
            weight: None,
            italic: false,
            mono: false,
            tag: 0,
        }
    }
}

/// A handle into the shaping cache: one shaped, laid-out text buffer.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct GlyphRunId(pub u64);

#[derive(Clone, Debug, PartialEq)]
struct ShapeKey {
    text_hash: u64,
    text_len: usize,
    style_hash: u64,
    scale_bits: u32,
    wrap_bucket: Option<u32>,
}

struct Entry {
    key: ShapeKey,
    buffer: Buffer,
    /// Logical size of the laid-out text.
    size: Size,
    /// Logical line height and baseline of the first line (for carets / baselines).
    line_height: f32,
    /// How many retained display-list slices reference this run.
    refs: u32,
    last_used: u64,
}

/// Counters the budget tests read.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TextStats {
    /// Cache misses: a buffer was shaped.
    pub shape_calls: u64,
    pub cache_hits: u64,
    pub glyphs_rasterised: u64,
    /// Times the system font database was merged in (at most once per `TextSystem`).
    pub system_font_loads: u64,
}

forge_trace::control_switches! {
    /// Fault switches for positive controls (W2). All off in production.
    #[derive(Clone, Copy, Debug, Default)]
    pub struct TextFaults {
        /// Every layout request re-shapes (the `ui_text_shaping_cached` control).
        pub disable_shaping_cache: bool,
        /// One atlas per font instead of one atlas (a `ui_single_glyph_atlas` control).
        pub atlas_per_font: bool,
        /// An RGBA8 mask plane instead of R8 (a `ui_single_glyph_atlas` control).
        pub rgba_mask_plane: bool,
    }
}

/// Font bytes shared without copying (`&'static` bundled data or owned user faces).
pub type FontBytes = std::sync::Arc<dyn AsRef<[u8]> + Send + Sync>;

/// The bundled UI faces (D-7, ADR 0013): Roboto for UI text and a monospace face, all
/// Apache-2.0, compiled into the binary so cold start never waits for a system font scan
/// and every machine lays text out identically.
pub const BUNDLED_FACES: &[(&str, &[u8])] = &[
    (
        "Roboto-Regular",
        include_bytes!("../fonts/Roboto-Regular.ttf"),
    ),
    (
        "Roboto-Italic",
        include_bytes!("../fonts/Roboto-Italic.ttf"),
    ),
    (
        "Roboto-Medium",
        include_bytes!("../fonts/Roboto-Medium.ttf"),
    ),
    ("Roboto-Bold", include_bytes!("../fonts/Roboto-Bold.ttf")),
    (
        "DroidSansMono",
        include_bytes!("../fonts/DroidSansMono.ttf"),
    ),
];
/// The family UI text resolves to.
pub const BUNDLED_SANS_FAMILY: &str = "Roboto";
/// The family monospace text resolves to (interim Droid Sans Mono; `fonts/README.md`).
pub const BUNDLED_MONO_FAMILY: &str = "Droid Sans Mono";

/// Font discovery (§21.2 "Fonts", D-7).
///
/// `bundled` faces are always loaded and are what UI text resolves to. The user's system
/// fonts are a **lazy fallback**: they are scanned (once per process, ~700 ms on a
/// typical Windows install) only when a shaped run contains a glyph the bundle lacks —
/// CJK, Arabic, emoji — and never redistributed.
#[derive(Clone)]
pub struct FontConfig {
    pub bundled: Vec<FontBytes>,
    /// Fall back to installed system fonts on a glyph miss. Goldens turn it off so a
    /// missing glyph draws the same `.notdef` box on every machine.
    pub system_fallback: bool,
    /// Fault (W2 control for `test_ui_bundled_font`): scan system fonts at construction,
    /// as the pre-D-7 core did.
    #[doc(hidden)]
    pub eager_system_scan: bool,
}

impl Default for FontConfig {
    fn default() -> Self {
        Self {
            bundled: BUNDLED_FACES
                .iter()
                .map(|(_, b)| std::sync::Arc::new(*b) as FontBytes)
                .collect(),
            system_fallback: true,
            eager_system_scan: false,
        }
    }
}

impl FontConfig {
    /// Bundled faces only, no system fallback (portable goldens).
    pub fn bundled_only() -> Self {
        Self {
            system_fallback: false,
            ..Self::default()
        }
    }
}

/// The system font database, scanned at most once per process and only on demand.
fn system_db() -> &'static cosmic_text::fontdb::Database {
    static DB: OnceLock<cosmic_text::fontdb::Database> = OnceLock::new();
    DB.get_or_init(|| {
        let mut db = cosmic_text::fontdb::Database::new();
        db.load_system_fonts();
        db
    })
}

fn build_db(cfg: &FontConfig, stats: &mut TextStats) -> cosmic_text::fontdb::Database {
    let mut db = cosmic_text::fontdb::Database::new();
    for data in &cfg.bundled {
        db.load_font_source(cosmic_text::fontdb::Source::Binary(data.clone()));
    }
    if cfg.eager_system_scan {
        add_system_faces(&mut db, stats);
    }
    set_families(&mut db);
    db
}

/// What one system font scan costs right now (a fresh scan, not the cached one): the
/// cold-start time D-7 removed. Diagnostics and `test_ui_bundled_font` only.
#[doc(hidden)]
pub fn measure_system_scan() -> std::time::Duration {
    let t = std::time::Instant::now();
    let mut db = cosmic_text::fontdb::Database::new();
    db.load_system_fonts();
    std::hint::black_box(db.len());
    t.elapsed()
}

/// Merge the (process-wide) system faces into `db`; face data stays memory-mapped/shared.
/// Every merge is counted here, where it happens, so no path can merge without it showing
/// in [`TextStats::system_font_loads`]: the cold-start guard reads that counter and the
/// database's face count ([`TextSystem::face_count`]), never a self-reported flag.
fn add_system_faces(db: &mut cosmic_text::fontdb::Database, stats: &mut TextStats) {
    stats.system_font_loads += 1;
    for f in system_db().faces() {
        db.push_face_info(f.clone());
    }
}

fn set_families(db: &mut cosmic_text::fontdb::Database) {
    let has = |db: &cosmic_text::fontdb::Database, fam: &str| {
        db.faces()
            .any(|f| f.families.iter().any(|(n, _)| n.eq_ignore_ascii_case(fam)))
    };
    if let Some(f) = SANS_PREFERENCE.iter().find(|f| has(db, f)) {
        db.set_sans_serif_family(*f);
    }
    if let Some(f) = MONO_PREFERENCE.iter().find(|f| has(db, f)) {
        db.set_monospace_family(*f);
    }
}

/// Preferred faces, first match wins: the bundle first, then platform UI faces (only
/// reachable when a caller supplies no bundle).
const SANS_PREFERENCE: &[&str] = &[
    BUNDLED_SANS_FAMILY,
    "Segoe UI",
    "Noto Sans",
    "DejaVu Sans",
    "Liberation Sans",
    "Cantarell",
];
const MONO_PREFERENCE: &[&str] = &[
    "Roboto Mono",
    BUNDLED_MONO_FAMILY,
    "Cascadia Mono",
    "Consolas",
    "Noto Sans Mono",
    "DejaVu Sans Mono",
    "Liberation Mono",
];

/// Shaping, the shaping cache and the glyph atlas. Shared by every window (§21.14).
pub struct TextSystem {
    fonts: FontSystem,
    swash: SwashCache,
    cache: HashMap<u64, Entry>,
    scale: f32,
    frame: u64,
    pub stats: TextStats,
    pub faults: TextFaults,
    pub atlas: GlyphAtlas,
    /// Only populated under `TextFaults::atlas_per_font`.
    extra_atlases: HashMap<cosmic_text::fontdb::ID, GlyphAtlas>,
    /// Entries kept (unreferenced ones beyond this are evicted, least recently used first).
    pub cache_capacity: usize,
    system_fallback: bool,
    system_loaded: bool,
}

impl TextSystem {
    pub fn new(cfg: &FontConfig) -> Self {
        Self::with_atlas(cfg, AtlasConfig::default())
    }

    pub fn with_atlas(cfg: &FontConfig, atlas: AtlasConfig) -> Self {
        let mut stats = TextStats::default();
        let db = build_db(cfg, &mut stats);
        Self {
            fonts: FontSystem::new_with_locale_and_db("en-US".into(), db),
            swash: SwashCache::new(),
            cache: HashMap::new(),
            scale: 1.0,
            frame: 0,
            stats,
            faults: TextFaults::default(),
            atlas: GlyphAtlas::new(atlas),
            extra_atlases: HashMap::new(),
            cache_capacity: 16_384,
            system_fallback: cfg.system_fallback,
            system_loaded: cfg.eager_system_scan,
        }
    }

    /// The display scale (window scale × user UI scale). A change re-shapes text at the
    /// new size, because the scale is part of every cache key (§21.14).
    pub fn set_scale(&mut self, scale: f32) {
        self.scale = scale.max(0.25);
    }
    pub fn scale(&self) -> f32 {
        self.scale
    }

    /// A stable fingerprint of the faces that resolve for the UI families (display-list
    /// goldens are only comparable between machines whose fonts match).
    pub fn font_fingerprint(&self) -> (String, u64) {
        let db = self.fonts.db();
        let mut h = Fnv::new();
        let mut names = Vec::new();
        for fam in [Family::SansSerif, Family::Monospace] {
            let q = cosmic_text::fontdb::Query {
                families: &[fam],
                ..cosmic_text::fontdb::Query::default()
            };
            if let Some(id) = db.query(&q) {
                if let Some(face) = db.face(id) {
                    names.push(
                        face.families
                            .first()
                            .map(|(n, _)| n.clone())
                            .unwrap_or_default(),
                    );
                }
                db.with_face_data(id, |data, idx| {
                    h.bytes(data);
                    h.u32(idx);
                });
            }
        }
        (names.join(" + "), h.finish())
    }

    fn key_for(&self, text: &str, style: &TextStyle, wrap_width: Option<f32>) -> ShapeKey {
        let mut sh = Fnv::new();
        style.hash_into(&mut sh);
        ShapeKey {
            text_hash: hash_str(text),
            text_len: text.len(),
            style_hash: sh.finish(),
            scale_bits: self.scale.to_bits(),
            // Buckets are 1 logical px; only wrapping text has a width in its key.
            wrap_bucket: if style.wrap {
                wrap_width.map(|w| w.max(0.0).floor() as u32)
            } else {
                None
            },
        }
    }

    fn id_of(key: &ShapeKey) -> u64 {
        let mut h = Fnv::new();
        h.u64(key.text_hash);
        h.u64(key.text_len as u64);
        h.u64(key.style_hash);
        h.u32(key.scale_bits);
        match key.wrap_bucket {
            Some(b) => {
                h.byte(1);
                h.u32(b);
            }
            None => h.byte(0),
        }
        splitmix(h.finish())
    }

    /// Shape (or fetch from cache) and lay out `text`. Returns the run and its logical size.
    pub fn layout(
        &mut self,
        text: &str,
        style: &TextStyle,
        wrap_width: Option<f32>,
    ) -> (GlyphRunId, Size) {
        let key = self.key_for(text, style, wrap_width);
        self.layout_keyed(key, style, &[RichSpan::plain(text)])
    }

    /// Shape (or fetch) rich text: spans with their own weight, slant, family and a `tag`
    /// that comes back per glyph (span colours, link hit-testing). One buffer, so wrapping
    /// and bidi run across span boundaries.
    pub fn layout_rich(
        &mut self,
        spans: &[RichSpan<'_>],
        style: &TextStyle,
        wrap_width: Option<f32>,
    ) -> (GlyphRunId, Size) {
        let mut h = Fnv::new();
        let mut len = 0usize;
        for s in spans {
            h.bytes(s.text.as_bytes());
            h.byte(0xff);
            h.u32(u32::from(s.weight.unwrap_or(0)));
            h.byte(u8::from(s.italic) | (u8::from(s.mono) << 1));
            h.u32(u32::from(s.tag));
            len += s.text.len();
        }
        let mut key = self.key_for("", style, wrap_width);
        key.text_hash = h.finish() ^ 0x5249_4348; // "RICH": never collides with plain text
        key.text_len = len;
        self.layout_keyed(key, style, spans)
    }

    fn attrs_for<'a>(style: &TextStyle, span: &RichSpan<'_>) -> Attrs<'a> {
        let mono = style.mono || span.mono;
        Attrs::new()
            .family(if mono {
                Family::Monospace
            } else {
                Family::SansSerif
            })
            .weight(Weight(span.weight.unwrap_or(style.weight)))
            .style(if style.italic || span.italic {
                FontStyle::Italic
            } else {
                FontStyle::Normal
            })
            .metadata(usize::from(span.tag))
    }

    fn shape_buffer(
        &mut self,
        style: &TextStyle,
        spans: &[RichSpan<'_>],
        width_px: Option<f32>,
    ) -> Buffer {
        let s = self.scale;
        let metrics = Metrics::new(style.size * s, style.size * style.line_height * s);
        let mut buffer = Buffer::new(&mut self.fonts, metrics);
        buffer.set_wrap(if style.wrap {
            Wrap::WordOrGlyph
        } else {
            Wrap::None
        });
        buffer.set_size(width_px, None);
        let base = Self::attrs_for(style, &RichSpan::plain(""));
        if let [one] = spans {
            buffer.set_text(
                one.text,
                &Self::attrs_for(style, one),
                Shaping::Advanced,
                None,
            );
        } else {
            buffer.set_rich_text(
                spans.iter().map(|sp| (sp.text, Self::attrs_for(style, sp))),
                &base,
                Shaping::Advanced,
                None,
            );
        }
        buffer.shape_until_scroll(&mut self.fonts, false);
        buffer
    }

    /// A glyph the loaded faces lack (`.notdef`, glyph 0) for a visible character.
    fn has_missing_glyph(buffer: &Buffer) -> bool {
        buffer.layout_runs().any(|r| {
            r.glyphs.iter().any(|g| {
                g.glyph_id == 0
                    && r.text
                        .get(g.start..g.end)
                        .is_some_and(|t| t.chars().any(|c| !c.is_whitespace() && !c.is_control()))
            })
        })
    }

    /// Whether the system font database has been merged in (lazy fallback, D-7).
    pub fn system_fonts_loaded(&self) -> bool {
        self.system_loaded
    }

    /// How many faces the font database holds right now, read from the database itself:
    /// the bundle alone until a glyph miss merges the system faces.
    pub fn face_count(&self) -> usize {
        self.fonts.db().len()
    }

    fn layout_keyed(
        &mut self,
        key: ShapeKey,
        style: &TextStyle,
        spans: &[RichSpan<'_>],
    ) -> (GlyphRunId, Size) {
        let id = Self::id_of(&key);
        let frame = self.frame;
        if !self.faults.disable_shaping_cache()
            && let Some(e) = self.cache.get_mut(&id)
            && e.key == key
        {
            e.last_used = frame;
            self.stats.cache_hits += 1;
            return (GlyphRunId(id), e.size);
        }
        self.stats.shape_calls += 1;
        let s = self.scale;
        let width_px = key.wrap_bucket.map(|b| b as f32 * s);
        let mut buffer = self.shape_buffer(style, spans, width_px);
        if self.system_fallback && !self.system_loaded && Self::has_missing_glyph(&buffer) {
            // The first glyph the bundle lacks: merge the system faces once and reshape.
            self.system_loaded = true;
            add_system_faces(self.fonts.db_mut(), &mut self.stats);
            self.stats.shape_calls += 1;
            buffer = self.shape_buffer(style, spans, width_px);
        }
        let mut w: f32 = 0.0;
        let mut lines = 0usize;
        for run in buffer.layout_runs() {
            w = w.max(run.line_w);
            lines += 1;
        }
        let lh = style.size * style.line_height * s;
        let size = Size::new(w / s, (lines.max(1) as f32 * lh) / s);
        let refs = self.cache.get(&id).map(|e| e.refs).unwrap_or(0);
        self.cache.insert(
            id,
            Entry {
                key,
                buffer,
                size,
                line_height: lh / s,
                refs,
                last_used: frame,
            },
        );
        (GlyphRunId(id), size)
    }

    /// The logical size of a run already in the cache.
    pub fn run_size(&self, run: GlyphRunId) -> Option<Size> {
        self.cache.get(&run.0).map(|e| e.size)
    }
    pub fn line_height(&self, run: GlyphRunId) -> Option<f32> {
        self.cache.get(&run.0).map(|e| e.line_height)
    }

    /// A retained slice now references `run` (so eviction keeps it).
    pub fn retain(&mut self, run: GlyphRunId) {
        if let Some(e) = self.cache.get_mut(&run.0) {
            e.refs += 1;
        }
    }
    pub fn release(&mut self, run: GlyphRunId) {
        if let Some(e) = self.cache.get_mut(&run.0) {
            e.refs = e.refs.saturating_sub(1);
        }
    }

    /// Logical x of the caret before byte `index` on the first line (single-line fields).
    pub fn caret_x(&self, run: GlyphRunId, index: usize) -> f32 {
        let Some(e) = self.cache.get(&run.0) else {
            return 0.0;
        };
        let mut x = 0.0;
        if let Some(r) = e.buffer.layout_runs().next() {
            for g in r.glyphs {
                if index <= g.start {
                    return g.x / self.scale;
                }
                x = (g.x + g.w) / self.scale;
            }
        }
        x
    }

    /// The byte index nearest logical `x` on the first line.
    pub fn hit_index(&self, run: GlyphRunId, x: f32) -> usize {
        let Some(e) = self.cache.get(&run.0) else {
            return 0;
        };
        let xp = x * self.scale;
        let mut last_end = 0;
        if let Some(r) = e.buffer.layout_runs().next() {
            for g in r.glyphs {
                if xp < g.x + g.w * 0.5 {
                    return g.start;
                }
                last_end = g.end;
            }
        }
        last_end
    }

    /// Byte offset of each paragraph (`\n`-separated line) of a run's text.
    fn paragraph_starts(e: &Entry) -> Vec<usize> {
        let mut out = Vec::with_capacity(e.buffer.lines.len());
        let mut at = 0usize;
        for l in &e.buffer.lines {
            out.push(at);
            at += l.text().len() + l.ending().as_str().len();
        }
        out
    }

    /// Caret geometry for global byte `index` of a (possibly multi-line, wrapped) run:
    /// `(x, line top, line height)` in logical px relative to the run's origin.
    pub fn caret_pos(&self, run: GlyphRunId, index: usize) -> (f32, f32, f32) {
        let Some(e) = self.cache.get(&run.0) else {
            return (0.0, 0.0, 0.0);
        };
        let s = self.scale;
        let starts = Self::paragraph_starts(e);
        let para = starts.iter().rposition(|st| *st <= index).unwrap_or(0);
        let local = index - starts.get(para).copied().unwrap_or(0);
        let mut best: Option<(f32, f32, f32)> = None;
        for r in e.buffer.layout_runs().filter(|r| r.line_i == para) {
            let first = r.glyphs.first().map_or(0, |g| g.start);
            let last = r.glyphs.last().map_or(0, |g| g.end);
            let x = if r.glyphs.is_empty() {
                Some(0.0)
            } else if local < first {
                None
            } else if local >= last {
                Some(r.line_w)
            } else {
                r.glyphs
                    .iter()
                    .find(|g| local <= g.start)
                    .map(|g| g.x)
                    .or(Some(r.line_w))
            };
            if let Some(x) = x {
                best = Some((x / s, r.line_top / s, r.line_height / s));
                if local < last {
                    break;
                }
            }
        }
        best.unwrap_or((0.0, 0.0, e.line_height))
    }

    /// Global byte index nearest the logical point `(x, y)` relative to the run's origin.
    pub fn hit_point(&self, run: GlyphRunId, x: f32, y: f32) -> usize {
        let Some(e) = self.cache.get(&run.0) else {
            return 0;
        };
        let starts = Self::paragraph_starts(e);
        let s = self.scale;
        match e.buffer.hit(x.max(0.0) * s, y.max(0.0) * s) {
            Some(c) => starts.get(c.line).copied().unwrap_or(0) + c.index,
            None => {
                starts.last().copied().unwrap_or(0)
                    + e.buffer.lines.last().map_or(0, |l| l.text().len())
            }
        }
    }

    /// Rects (logical, relative to the origin) covered by glyphs whose span tag is `tag`
    /// (links in rich text).
    pub fn tag_rects(&self, run: GlyphRunId, tag: usize) -> Vec<crate::geom::Rect> {
        let Some(e) = self.cache.get(&run.0) else {
            return Vec::new();
        };
        let s = self.scale;
        let mut out: Vec<crate::geom::Rect> = Vec::new();
        for r in e.buffer.layout_runs() {
            let mut cur: Option<(f32, f32)> = None;
            for g in r.glyphs {
                if g.metadata == tag {
                    cur = Some(match cur {
                        Some((x0, _)) => (x0, g.x + g.w),
                        None => (g.x, g.x + g.w),
                    });
                } else if let Some((x0, x1)) = cur.take() {
                    out.push(crate::geom::Rect::new(
                        x0 / s,
                        r.line_top / s,
                        (x1 - x0) / s,
                        r.line_height / s,
                    ));
                }
            }
            if let Some((x0, x1)) = cur {
                out.push(crate::geom::Rect::new(
                    x0 / s,
                    r.line_top / s,
                    (x1 - x0) / s,
                    r.line_height / s,
                ));
            }
        }
        out
    }

    /// Visual line count of a run (wrapping included).
    pub fn visual_lines(&self, run: GlyphRunId) -> usize {
        self.cache
            .get(&run.0)
            .map_or(0, |e| e.buffer.layout_runs().count())
    }

    /// Positioned glyphs of `run` for drawing at physical `origin`: the atlas key, the
    /// integer physical pen position and the span tag of each glyph.
    pub(crate) fn glyphs_tagged(
        &mut self,
        run: GlyphRunId,
        origin: (f32, f32),
    ) -> Vec<(CacheKey, i32, i32, usize)> {
        let frame = self.frame;
        let Some(e) = self.cache.get_mut(&run.0) else {
            return Vec::new();
        };
        e.last_used = frame;
        let mut out = Vec::new();
        for r in e.buffer.layout_runs() {
            for g in r.glyphs {
                let p = g.physical((origin.0, origin.1 + r.line_y), 1.0);
                out.push((p.cache_key, p.x, p.y, g.metadata));
            }
        }
        out
    }

    /// Rasterise (if needed) and place a glyph in the atlas.
    pub(crate) fn glyph(&mut self, key: CacheKey) -> Result<Option<GlyphEntry>, AtlasFull> {
        let rgba_mask = self.faults.rgba_mask_plane();
        let per_font = self.faults.atlas_per_font() && key.font_id != self.primary_font();
        let atlas = if per_font {
            self.extra_atlases
                .entry(key.font_id)
                .or_insert_with(|| GlyphAtlas::new(AtlasConfig::default()))
        } else {
            &mut self.atlas
        };
        atlas.mask_is_rgba = rgba_mask;
        if let Some(hit) = atlas.lookup(AtlasKey::Glyph(key)) {
            return Ok(hit);
        }
        let img = self.swash.get_image_uncached(&mut self.fonts, key);
        self.stats.glyphs_rasterised += 1;
        let atlas = if per_font {
            self.extra_atlases.get_mut(&key.font_id).ok_or(AtlasFull)?
        } else {
            &mut self.atlas
        };
        let img = img.map(|i| {
            let plane = match i.content {
                SwashContent::Color => Plane::Color,
                SwashContent::Mask | SwashContent::SubpixelMask => Plane::Mask,
            };
            let data = match i.content {
                SwashContent::SubpixelMask => i
                    .data
                    .chunks(4)
                    .map(|c| c.first().copied().unwrap_or(0))
                    .collect(),
                _ => i.data,
            };
            (plane, i.placement, data)
        });
        atlas.insert(AtlasKey::Glyph(key), img)
    }

    /// Rasterise (if needed) and place an icon of the Forge set, `px` physical pixels
    /// square and stroked `stroke` pixels wide, in the glyph atlas's mask plane: an icon
    /// draws as a glyph, in the text batch.
    pub(crate) fn icon(
        &mut self,
        icon: crate::icons::Icon,
        px: u32,
        stroke: f32,
    ) -> Result<Option<GlyphEntry>, AtlasFull> {
        let key = AtlasKey::Icon {
            icon,
            px,
            stroke: stroke.to_bits(),
        };
        self.atlas.mask_is_rgba = self.faults.rgba_mask_plane();
        if let Some(hit) = self.atlas.lookup(key) {
            return Ok(hit);
        }
        let placement = cosmic_text::Placement {
            left: 0,
            top: 0,
            width: px,
            height: px,
        };
        let data = crate::icons::mask(icon, px, stroke);
        self.atlas.insert(key, Some((Plane::Mask, placement, data)))
    }

    fn primary_font(&self) -> cosmic_text::fontdb::ID {
        let q = cosmic_text::fontdb::Query {
            families: &[Family::SansSerif],
            ..cosmic_text::fontdb::Query::default()
        };
        self.fonts
            .db()
            .query(&q)
            .unwrap_or_else(cosmic_text::fontdb::ID::dummy)
    }

    /// Number of glyph atlases in existence (the budget is exactly one).
    pub fn atlas_count(&self) -> usize {
        1 + self.extra_atlases.len()
    }

    /// End of a drawn frame: advance the frame counter and trim the cache.
    pub fn end_frame(&mut self) {
        self.frame += 1;
        self.atlas.frame = self.frame;
        if self.cache.len() > self.cache_capacity {
            let mut idle: Vec<(u64, u64)> = self
                .cache
                .iter()
                .filter(|(_, e)| e.refs == 0)
                .map(|(k, e)| (e.last_used, *k))
                .collect();
            idle.sort_unstable();
            let excess = self.cache.len() - self.cache_capacity;
            for (_, k) in idle.into_iter().take(excess) {
                self.cache.remove(&k);
            }
        }
    }

    pub fn cached_runs(&self) -> usize {
        self.cache.len()
    }
}

// ---- the glyph atlas ------------------------------------------------------------------

/// Which plane a glyph lives in.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum Plane {
    /// `R8Unorm` coverage: every outline glyph.
    Mask,
    /// `Rgba8UnormSrgb`: colour glyphs (emoji, COLR/bitmap fonts).
    Color,
}

/// Texel format of a plane.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum PlaneFormat {
    R8Unorm,
    Rgba8UnormSrgb,
}

impl PlaneFormat {
    pub fn bytes_per_texel(self) -> u64 {
        match self {
            PlaneFormat::R8Unorm => 1,
            PlaneFormat::Rgba8UnormSrgb => 4,
        }
    }
}

/// Sizes of the two planes (§21.7 table). Tests shrink them to exercise growth, eviction
/// and epochs without megabytes of glyphs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AtlasConfig {
    pub mask_start: u32,
    pub mask_cap: u32,
    pub color_start: u32,
    pub color_cap: u32,
    /// Glyphs unused for this many drawn frames may be evicted when a plane runs short.
    pub evict_after_frames: u64,
}

impl Default for AtlasConfig {
    fn default() -> Self {
        Self {
            mask_start: 2048,
            mask_cap: 4096,
            color_start: 512,
            color_cap: 2048,
            evict_after_frames: 600,
        }
    }
}

/// Where a glyph sits in the atlas.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct GlyphEntry {
    pub plane: Plane,
    /// Texel rect in its plane.
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
    /// Placement relative to the pen position (physical px).
    pub left: i32,
    pub top: i32,
    alloc: AllocId,
    last_used: u64,
}

/// The allocation failed even after growing to the cap and evicting: draw an epoch.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct AtlasFull;

struct AtlasPlane {
    alloc: BucketedAtlasAllocator,
    size: u32,
    cap: u32,
}

impl AtlasPlane {
    fn new(start: u32, cap: u32) -> Self {
        let s = i32::try_from(start).unwrap_or(2048);
        Self {
            alloc: BucketedAtlasAllocator::new(size2(s, s)),
            size: start,
            cap: cap.max(start),
        }
    }
}

/// What an atlas entry holds: a font glyph, or an icon of the Forge set at one pixel size
/// and stroke (its bit pattern).
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) enum AtlasKey {
    Glyph(CacheKey),
    Icon {
        icon: crate::icons::Icon,
        px: u32,
        stroke: u32,
    },
}

/// The one glyph atlas: one object, one key space, one bind group, two planes.
pub struct GlyphAtlas {
    mask: AtlasPlane,
    color: AtlasPlane,
    map: HashMap<AtlasKey, Option<GlyphEntry>>,
    pending: Vec<TextureUpdate>,
    cfg: AtlasConfig,
    frame: u64,
    /// Under the RGBA-mask fault the mask plane stores 4 bytes per texel.
    pub(crate) mask_is_rgba: bool,
    /// Epochs started since creation (each costs one extra draw call).
    pub epochs: u64,
    pub evictions: u64,
}

impl GlyphAtlas {
    pub fn new(cfg: AtlasConfig) -> Self {
        let mut a = Self {
            mask: AtlasPlane::new(cfg.mask_start, cfg.mask_cap),
            color: AtlasPlane::new(cfg.color_start, cfg.color_cap),
            map: HashMap::new(),
            pending: Vec::new(),
            cfg,
            frame: 0,
            mask_is_rgba: false,
            epochs: 0,
            evictions: 0,
        };
        // The renderer creates both planes at their start size up front.
        a.pending.push(TextureUpdate {
            tex: TextureId::GlyphMask,
            kind: TextureUpdateKind::Create {
                w: cfg.mask_start,
                h: cfg.mask_start,
            },
        });
        a.pending.push(TextureUpdate {
            tex: TextureId::GlyphColor,
            kind: TextureUpdateKind::Create {
                w: cfg.color_start,
                h: cfg.color_start,
            },
        });
        a
    }

    /// Plane formats: exactly one `R8Unorm` mask plane and one `Rgba8UnormSrgb` colour plane.
    pub fn plane_formats(&self) -> (PlaneFormat, PlaneFormat) {
        (
            if self.mask_is_rgba {
                PlaneFormat::Rgba8UnormSrgb
            } else {
                PlaneFormat::R8Unorm
            },
            PlaneFormat::Rgba8UnormSrgb,
        )
    }

    pub fn plane_sizes(&self) -> (u32, u32) {
        (self.mask.size, self.color.size)
    }

    /// Bytes of VRAM the two planes occupy now.
    pub fn vram_bytes(&self) -> u64 {
        let (m, c) = self.plane_formats();
        u64::from(self.mask.size).pow(2) * m.bytes_per_texel()
            + u64::from(self.color.size).pow(2) * c.bytes_per_texel()
    }

    /// Bytes at the cap (the §21.7 bound: 32 MB).
    pub fn vram_cap_bytes(&self) -> u64 {
        let (m, c) = self.plane_formats();
        u64::from(self.mask.cap).pow(2) * m.bytes_per_texel()
            + u64::from(self.color.cap).pow(2) * c.bytes_per_texel()
    }

    /// Bumped whenever resident glyphs move or vanish (eviction, epoch): cached glyph
    /// instances built against an older generation are stale.
    pub fn generation(&self) -> u64 {
        self.evictions + self.epochs
    }

    pub fn resident_glyphs(&self) -> usize {
        self.map.values().filter(|e| e.is_some()).count()
    }

    /// Texture updates the renderer must apply before drawing what was batched so far.
    pub fn take_updates(&mut self) -> Vec<TextureUpdate> {
        std::mem::take(&mut self.pending)
    }

    fn lookup(&mut self, key: AtlasKey) -> Option<Option<GlyphEntry>> {
        let frame = self.frame;
        match self.map.get_mut(&key) {
            Some(Some(e)) => {
                e.last_used = frame;
                Some(Some(*e))
            }
            Some(None) => Some(None),
            None => None,
        }
    }

    fn plane_mut(&mut self, p: Plane) -> &mut AtlasPlane {
        match p {
            Plane::Mask => &mut self.mask,
            Plane::Color => &mut self.color,
        }
    }

    /// Place a rasterised image: its plane, placement and texels (one byte a texel on the
    /// mask plane, RGBA on the colour plane).
    fn insert(
        &mut self,
        key: AtlasKey,
        img: Option<(Plane, cosmic_text::Placement, Vec<u8>)>,
    ) -> Result<Option<GlyphEntry>, AtlasFull> {
        let Some((plane, placement, data)) = img.filter(|(_, p, _)| p.width > 0 && p.height > 0)
        else {
            // Whitespace and empty glyphs: remembered, never allocated.
            self.map.insert(key, None);
            return Ok(None);
        };
        let (w, h) = (placement.width, placement.height);
        let alloc = self.allocate(plane, w + 1, h + 1)?;
        let r = alloc.rectangle;
        let (x, y) = (r.min.x.max(0) as u32, r.min.y.max(0) as u32);
        let data = match plane {
            Plane::Mask if self.mask_is_rgba => data.iter().flat_map(|&c| [c, c, c, c]).collect(),
            _ => data,
        };
        self.pending.push(TextureUpdate {
            tex: match plane {
                Plane::Mask => TextureId::GlyphMask,
                Plane::Color => TextureId::GlyphColor,
            },
            kind: TextureUpdateKind::Write { x, y, w, h, data },
        });
        let e = GlyphEntry {
            plane,
            x,
            y,
            w,
            h,
            left: placement.left,
            top: placement.top,
            alloc: alloc.id,
            last_used: self.frame,
        };
        self.map.insert(key, Some(e));
        Ok(Some(e))
    }

    fn allocate(&mut self, plane: Plane, w: u32, h: u32) -> Result<etagere::Allocation, AtlasFull> {
        let sz = size2(
            i32::try_from(w).map_err(|_| AtlasFull)?,
            i32::try_from(h).map_err(|_| AtlasFull)?,
        );
        loop {
            if let Some(a) = self.plane_mut(plane).alloc.allocate(sz) {
                return Ok(a);
            }
            // 1. Grow by reallocating and copying, up to the cap (never a second texture).
            let p = self.plane_mut(plane);
            if p.size < p.cap {
                let new = (p.size * 2).min(p.cap);
                let n = i32::try_from(new).map_err(|_| AtlasFull)?;
                p.alloc.grow(size2(n, n));
                p.size = new;
                self.pending.push(TextureUpdate {
                    tex: match plane {
                        Plane::Mask => TextureId::GlyphMask,
                        Plane::Color => TextureId::GlyphColor,
                    },
                    kind: TextureUpdateKind::Grow { w: new, h: new },
                });
                continue;
            }
            // 2. Evict glyphs unused for `evict_after_frames` drawn frames.
            if self.evict(plane) == 0 {
                return Err(AtlasFull);
            }
        }
    }

    fn evict(&mut self, plane: Plane) -> usize {
        let cutoff = self.frame.saturating_sub(self.cfg.evict_after_frames);
        let victims: Vec<(AtlasKey, AllocId)> = self
            .map
            .iter()
            .filter_map(|(k, e)| {
                e.filter(|e| {
                    e.plane == plane
                        && e.last_used < cutoff
                        && self.frame >= self.cfg.evict_after_frames
                })
                .map(|e| (*k, e.alloc))
            })
            .collect();
        for (k, id) in &victims {
            self.plane_mut(plane).alloc.deallocate(*id);
            self.map.remove(k);
        }
        self.evictions += victims.len() as u64;
        victims.len()
    }

    /// Start a new atlas epoch for `plane`: everything resident there is dropped so the
    /// rest of the frame's glyphs can be placed. The batcher has already closed the batch
    /// that referenced the old contents.
    pub(crate) fn begin_epoch(&mut self, plane: Plane) {
        self.epochs += 1;
        self.plane_mut(plane).alloc.clear();
        self.map
            .retain(|_, e| e.as_ref().is_none_or(|e| e.plane != plane));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn two_planes_at_start_and_cap_budget() {
        let a = GlyphAtlas::new(AtlasConfig::default());
        assert_eq!(
            a.plane_formats(),
            (PlaneFormat::R8Unorm, PlaneFormat::Rgba8UnormSrgb)
        );
        assert_eq!(a.vram_bytes(), 2048 * 2048 + 512 * 512 * 4); // 5 MB
        assert_eq!(a.vram_cap_bytes(), 32 * 1024 * 1024);
    }
}
