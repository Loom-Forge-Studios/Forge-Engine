//! The error contract (Ch.1.2).
//!
//! One error enum per crate, every variant carrying a stable [`ErrorCode`], and every enum
//! convertible into the one engine-wide [`Error`]. Codes are allocated in
//! `docs/error-codes.md` and nowhere else (W8).
//!
//! * A crate **above** `forge-core` implements [`CodedError`] for its enum and gets
//!   `From<ItsError> for Error` from the blanket impl — `?` just works.
//! * The crates **below** `forge-core` (`forge-num`, `forge-frames`, `forge-seed`) cannot see
//!   this trait (the spine only points down), so `forge-core` implements [`CodedError`] for
//!   their enums here.
//!
//! [`Error`] is built to be cheap on the happy path and cheap to annotate. It is **one
//! pointer** (D-10, ADR 0006): the pinned Ch.1.2 fields live on [`ErrorInner`] behind a
//! `Box`, so `Result<(), Error>` is 8 bytes and `Result<T, Error>` costs a return register,
//! not a ~200-byte out-pointer copy, on every hot path that merely *can* fail. The one heap
//! allocation happens only when an error is actually built; inside it the code is a
//! `&'static str` and breadcrumbs ([`Ctx`]) live inline up to four deep.

use std::borrow::Cow;
use std::error::Error as StdError;
use std::fmt;
use std::ops::{Deref, DerefMut};

use smallvec::SmallVec;

/// A stable, greppable error code such as `CORE-0007`: an upper-case crate prefix, a dash,
/// four digits. Allocated in `docs/error-codes.md`, append-only, never reused.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ErrorCode(&'static str);

impl ErrorCode {
    /// Build a code from its literal text. Meant for `const` items, where a malformed code is
    /// a **compile error** (`const` evaluation of the format check fails):
    ///
    /// ```
    /// use forge_core::ErrorCode;
    /// const MY_CODE: ErrorCode = ErrorCode::new("CORE-0001");
    /// assert_eq!(MY_CODE.as_str(), "CORE-0001");
    /// ```
    ///
    /// ```compile_fail
    /// use forge_core::ErrorCode;
    /// const BAD: ErrorCode = ErrorCode::new("core-1"); // not PREFIX-NNNN
    /// ```
    ///
    /// In a function body use [`error_code!`](crate::error_code), which forces the same check
    /// to compile time; for text only known at run time use [`ErrorCode::parse`], which cannot
    /// panic.
    #[must_use]
    pub const fn new(code: &'static str) -> Self {
        if !is_well_formed(code) {
            panic!("an ErrorCode is PREFIX-NNNN: upper-case ASCII prefix, '-', four digits");
        }
        Self(code)
    }

    /// Parse a code at run time; `None` if it is not `PREFIX-NNNN`.
    #[must_use]
    pub const fn parse(code: &'static str) -> Option<Self> {
        if is_well_formed(code) {
            Some(Self(code))
        } else {
            None
        }
    }

    /// The code's text, exactly as it appears in `docs/error-codes.md`.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        self.0
    }

    /// The crate prefix (`CORE` for `CORE-0007`).
    #[must_use]
    pub fn prefix(self) -> &'static str {
        // Well-formed by construction: exactly one '-' followed by four digits.
        let cut = self.0.len() - 5;
        &self.0[..cut]
    }

    /// The number within the prefix (`7` for `CORE-0007`).
    #[must_use]
    pub fn number(self) -> u16 {
        self.0
            .bytes()
            .skip(self.0.len() - 4)
            .fold(0u16, |n, b| n * 10 + u16::from(b - b'0'))
    }
}

/// An [`ErrorCode`] checked **at compile time** wherever it is written — including inside a
/// `match` arm of a function that runs at run time — so a typo is a build error and never a
/// reachable panic (Ch.1.2: no panic reachable from a command handler).
///
/// ```
/// fn code(ok: bool) -> forge_core::ErrorCode {
///     if ok { forge_core::error_code!("CORE-0001") } else { forge_core::error_code!("CORE-0002") }
/// }
/// assert_eq!(code(false).number(), 2);
/// ```
///
/// ```compile_fail
/// let _ = forge_core::error_code!("CORE-12"); // not PREFIX-NNNN: rejected by the compiler
/// ```
#[macro_export]
macro_rules! error_code {
    ($code:literal) => {
        const { $crate::ErrorCode::new($code) }
    };
}

/// `PREFIX-NNNN`: one or more `A-Z`, a `-`, exactly four `0-9`.
const fn is_well_formed(code: &str) -> bool {
    let b = code.as_bytes();
    let n = b.len();
    if n < 6 || b[n - 5] != b'-' {
        return false;
    }
    let mut i = 0;
    while i < n - 5 {
        if !b[i].is_ascii_uppercase() {
            return false;
        }
        i += 1;
    }
    let mut j = n - 4;
    while j < n {
        if !b[j].is_ascii_digit() {
            return false;
        }
        j += 1;
    }
    true
}

impl fmt::Debug for ErrorCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.0)
    }
}

impl fmt::Display for ErrorCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.0)
    }
}

/// A breadcrumb value. Numbers and static strings never allocate.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CtxValue {
    /// A breadcrumb with no value (a bare marker such as `"during barrier"`).
    None,
    /// An unsigned number (an id, an index, a count).
    U64(u64),
    /// A signed number (a tick, an offset).
    I64(i64),
    /// Text: borrowed when static, owned otherwise.
    Text(Cow<'static, str>),
}

impl From<u64> for CtxValue {
    fn from(v: u64) -> Self {
        Self::U64(v)
    }
}
impl From<u32> for CtxValue {
    fn from(v: u32) -> Self {
        Self::U64(u64::from(v))
    }
}
impl From<usize> for CtxValue {
    fn from(v: usize) -> Self {
        Self::U64(v as u64)
    }
}
impl From<i64> for CtxValue {
    fn from(v: i64) -> Self {
        Self::I64(v)
    }
}
impl From<&'static str> for CtxValue {
    fn from(v: &'static str) -> Self {
        Self::Text(Cow::Borrowed(v))
    }
}
impl From<String> for CtxValue {
    fn from(v: String) -> Self {
        Self::Text(Cow::Owned(v))
    }
}
impl From<()> for CtxValue {
    fn from((): ()) -> Self {
        Self::None
    }
}

/// One breadcrumb: where the error passed through and what it was doing there.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Ctx {
    /// What was happening (`"region"`, `"system"`, `"barrier"`).
    pub key: &'static str,
    /// The value that identifies it.
    pub value: CtxValue,
}

impl fmt::Display for Ctx {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.value {
            CtxValue::None => f.write_str(self.key),
            CtxValue::U64(v) => write!(f, "{}={v}", self.key),
            CtxValue::I64(v) => write!(f, "{}={v}", self.key),
            CtxValue::Text(v) => write!(f, "{}={v}", self.key),
        }
    }
}

/// The engine-wide error (Ch.1.2). Every crate's error enum converts into it.
///
/// One pointer wide (D-10): the fields Ch.1.2 pins are on [`ErrorInner`], behind a `Box`,
/// and read through `Deref` exactly as if they were on `Error` — `e.code`, `e.ctx`,
/// `e.source` — so the contract's shape is unchanged; only its size is.
///
/// `Display` prints the code first so every log line is greppable, then the source's own
/// message, then the breadcrumbs outermost-last:
/// `CORE-0005: entity 3v0 is not owned by region 2 [system=mover, tick=41]`.
pub struct Error(Box<ErrorInner>);

/// The fields of an [`Error`], exactly as Ch.1.2 pins them. Reached through `Error`'s
/// `Deref`; never built directly (use [`Error::new`] / [`Error::with_source`] / `?`).
#[non_exhaustive]
pub struct ErrorInner {
    /// Stable, greppable, never reused (`docs/error-codes.md`).
    pub code: ErrorCode,
    /// Breadcrumbs, innermost first. Four fit inline without allocating.
    pub ctx: SmallVec<[Ctx; 4]>,
    /// The crate-level error this came from, if any.
    pub source: Option<Box<dyn StdError + Send + Sync>>,
}

// D-10: pointer-sized, so `Result<(), Error>` fits a register. Breaking this is a build error.
const _: () = assert!(size_of::<Error>() == size_of::<usize>());
const _: () = assert!(size_of::<Result<(), Error>>() == size_of::<usize>());

impl Deref for Error {
    type Target = ErrorInner;
    fn deref(&self) -> &ErrorInner {
        &self.0
    }
}

impl DerefMut for Error {
    fn deref_mut(&mut self) -> &mut ErrorInner {
        &mut self.0
    }
}

impl Error {
    /// An error with a code and nothing else.
    #[must_use]
    pub fn new(code: ErrorCode) -> Self {
        Self(Box::new(ErrorInner {
            code,
            ctx: SmallVec::new(),
            source: None,
        }))
    }

    /// An error wrapping a crate-level cause.
    #[must_use]
    pub fn with_source(code: ErrorCode, source: impl StdError + Send + Sync + 'static) -> Self {
        Self(Box::new(ErrorInner {
            code,
            ctx: SmallVec::new(),
            source: Some(Box::new(source)),
        }))
    }

    /// Add a breadcrumb and return the error (builder style).
    #[must_use]
    pub fn ctx(mut self, key: &'static str, value: impl Into<CtxValue>) -> Self {
        self.push_ctx(key, value);
        self
    }

    /// Add a breadcrumb in place.
    pub fn push_ctx(&mut self, key: &'static str, value: impl Into<CtxValue>) {
        self.ctx.push(Ctx {
            key,
            value: value.into(),
        });
    }

    /// The stable code.
    #[must_use]
    pub fn code(&self) -> ErrorCode {
        self.code
    }

    /// The source, downcast to a concrete crate error, if it is one.
    #[must_use]
    pub fn source_as<E: StdError + 'static>(&self) -> Option<&E> {
        self.source.as_deref().and_then(|s| s.downcast_ref::<E>())
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.source {
            Some(src) => {
                // Crate errors already lead with their code (the greppable rule); do not
                // print it twice.
                let msg = src.to_string();
                if msg.starts_with(self.code.as_str()) {
                    f.write_str(&msg)?;
                } else {
                    write!(f, "{}: {msg}", self.code)?;
                }
            }
            None => write!(f, "{}", self.code)?,
        }
        if !self.ctx.is_empty() {
            f.write_str(" [")?;
            for (i, c) in self.ctx.iter().enumerate() {
                if i > 0 {
                    f.write_str(", ")?;
                }
                write!(f, "{c}")?;
            }
            f.write_str("]")?;
        }
        Ok(())
    }
}

impl fmt::Debug for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Error")
            .field("code", &self.code)
            .field("ctx", &self.ctx)
            .field("source", &self.source.as_ref().map(ToString::to_string))
            .finish()
    }
}

impl StdError for Error {
    fn source(&self) -> Option<&(dyn StdError + 'static)> {
        self.source
            .as_deref()
            .map(|s| s as &(dyn StdError + 'static))
    }
}

/// A crate-level error enum: every variant has a stable code. Implementing this gives
/// `From<E> for Error`, so `?` converts.
pub trait CodedError: StdError + Send + Sync + 'static {
    /// The variant's code, as allocated in `docs/error-codes.md`.
    fn error_code(&self) -> ErrorCode;
}

impl<E: CodedError> From<E> for Error {
    fn from(e: E) -> Self {
        let code = e.error_code();
        Error::with_source(code, e)
    }
}

/// `Result` with the engine error as the default error type.
pub type Result<T, E = Error> = std::result::Result<T, E>;

/// Breadcrumbs on a `Result`: `thing().ctx("region", id.0)?`.
pub trait ResultExt<T> {
    /// Convert the error into [`Error`] and add a breadcrumb.
    fn ctx(self, key: &'static str, value: impl Into<CtxValue>) -> Result<T>;
}

impl<T, E: Into<Error>> ResultExt<T> for std::result::Result<T, E> {
    fn ctx(self, key: &'static str, value: impl Into<CtxValue>) -> Result<T> {
        self.map_err(|e| e.into().ctx(key, value))
    }
}

// ---- the crates below forge-core ----------------------------------------------------------
//
// Their enums already expose `code() -> &'static str` (they cannot name `ErrorCode`, which
// lives above them). Parsing that text — rather than mirroring each variant here — means a
// variant added below needs no edit in forge-core, and their enums being `#[non_exhaustive]`
// costs nothing. A malformed code (which `cargo xtask allocators` and
// `tests::lower_crate_codes_parse` both rule out) degrades to `CORE-0013`, never a panic.

/// Code used when a lower crate reports a code that is not `PREFIX-NNNN`.
const MALFORMED_LOWER_CODE: ErrorCode = ErrorCode::new("CORE-0013");

fn lower(code: &'static str) -> ErrorCode {
    ErrorCode::parse(code).unwrap_or(MALFORMED_LOWER_CODE)
}

impl CodedError for forge_frames::FrameError {
    fn error_code(&self) -> ErrorCode {
        lower(self.code())
    }
}

impl CodedError for forge_seed::SeedPathError {
    fn error_code(&self) -> ErrorCode {
        lower(self.code())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use forge_frames::{FrameError, FrameId};
    use forge_seed::SeedPathError;

    #[test]
    fn code_parts() {
        let c = ErrorCode::new("CORE-0007");
        assert_eq!(c.prefix(), "CORE");
        assert_eq!(c.number(), 7);
        assert_eq!(c.to_string(), "CORE-0007");
        assert_eq!(ErrorCode::new("FRAMES-1234").number(), 1234);
    }

    #[test]
    fn malformed_codes_do_not_parse() {
        for bad in [
            "",
            "CORE",
            "CORE-",
            "CORE-01",
            "CORE-00001",
            "core-0001",
            "CORE_0001",
            "-0001",
            "CO RE-0001",
            "CORE-00a1",
        ] {
            assert!(ErrorCode::parse(bad).is_none(), "{bad:?} parsed");
        }
        assert!(ErrorCode::parse("CORE-0001").is_some());
    }

    #[test]
    fn lower_crate_codes_parse() {
        let frames = [
            FrameError::UnknownFrame(FrameId(1)),
            FrameError::Disconnected {
                from: FrameId(1),
                to: FrameId(2),
            },
        ];
        for e in frames {
            assert_eq!(e.error_code().as_str(), e.code());
            assert_ne!(e.error_code(), MALFORMED_LOWER_CODE);
        }
        // A resolver's own error keeps its own code through the seam; a malformed one is
        // reported as such rather than trusted.
        let ext = |code| {
            FrameError::Extension(forge_frames::ExtensionError {
                code,
                message: "x".into(),
            })
        };
        assert_eq!(ext("TEST-0042").error_code().as_str(), "TEST-0042");
        assert_eq!(ext("not a code").error_code(), MALFORMED_LOWER_CODE);
        for e in [
            SeedPathError::MissingRoot,
            SeedPathError::BadSegment("x".into()),
        ] {
            assert_eq!(e.error_code().as_str(), e.code());
            assert_ne!(e.error_code(), MALFORMED_LOWER_CODE);
        }
    }

    #[test]
    fn question_mark_converts_and_keeps_the_source() {
        fn inner() -> Result<()> {
            Err(FrameError::UnknownFrame(FrameId(4)))?;
            Ok(())
        }
        let e = inner().expect_err("must fail");
        assert_eq!(e.code().as_str(), "FRAMES-0001");
        assert!(matches!(
            e.source_as::<FrameError>(),
            Some(FrameError::UnknownFrame(FrameId(4)))
        ));
        assert!(StdError::source(&e).is_some());
    }

    #[test]
    fn display_leads_with_the_code_once_and_lists_breadcrumbs() {
        let e: Error = SeedPathError::MissingRoot.into();
        let e = e
            .ctx("tick", 41i64)
            .ctx("system", "mover")
            .ctx("marker", ());
        let s = e.to_string();
        assert!(s.starts_with("SEED-0001: "), "{s}");
        assert_eq!(s.matches("SEED-0001").count(), 1, "{s}");
        assert!(s.ends_with(" [tick=41, system=mover, marker]"), "{s}");

        let bare = Error::new(ErrorCode::new("CORE-0002"));
        assert_eq!(bare.to_string(), "CORE-0002");
    }

    #[test]
    fn result_ext_adds_breadcrumbs() {
        let r: std::result::Result<(), FrameError> = Err(FrameError::UnknownFrame(FrameId(2)));
        let e = r.ctx("region", 3u32).expect_err("must fail");
        assert_eq!(e.ctx.len(), 1);
        assert_eq!(e.ctx[0].value, CtxValue::U64(3));
    }

    /// D-10 guard (the `const` asserts above make it a build error; this names it as a test).
    /// Positive control: the unboxed field layout the assert protects against is far over
    /// clippy's 128-byte `result_large_err` line, so the check discriminates.
    #[test]
    fn error_is_pointer_sized() {
        assert_eq!(size_of::<Result<(), Error>>(), size_of::<usize>());
        assert_eq!(size_of::<Result<u64, Error>>(), 2 * size_of::<usize>());
        assert!(
            size_of::<Result<(), ErrorInner>>() > 128,
            "positive control: the unboxed layout must be large ({} bytes)",
            size_of::<Result<(), ErrorInner>>()
        );
    }

    #[test]
    fn fields_read_through_the_box() {
        let mut e = Error::new(ErrorCode::new("CORE-0001")).ctx("tick", 3i64);
        assert_eq!(e.code, ErrorCode::new("CORE-0001"));
        assert_eq!(e.ctx.len(), 1);
        assert!(e.source.is_none());
        e.code = ErrorCode::new("CORE-0002");
        assert_eq!(e.code().as_str(), "CORE-0002");
    }

    #[test]
    fn four_breadcrumbs_stay_inline() {
        let mut e = Error::new(ErrorCode::new("CORE-0001"));
        for i in 0..4u64 {
            e.push_ctx("i", i);
        }
        assert!(!e.ctx.spilled());
    }
}
