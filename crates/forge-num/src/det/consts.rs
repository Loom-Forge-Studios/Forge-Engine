//! Every mathematical constant `det` embeds, as exact bit patterns.
//!
//! None of these is typed from memory: each is derived from first principles (Machin's
//! formula for pi, the series for ln 2 and atan, Newton's reciprocal for 2/pi) by the
//! test-only bignum in `tests/support/bignum.rs`, and `constants_match_first_principles`
//! (in `det/mod.rs`) re-derives and compares every one of them on every test run.

use crate::dd::Dd;

/// Bits of 2/pi after the binary point, most significant first: word 0 holds the bits of
/// weight 2^-1 .. 2^-64. 1280 bits — enough for Payne-Hanek reduction of the largest
/// finite f64 (exponent 971 needs bits up to ~1100) with a 192-bit window.
pub(crate) const TWO_OVER_PI: [u64; 20] = [
    0xA2F9_836E_4E44_1529,
    0xFC27_57D1_F534_DDC0,
    0xDB62_9599_3C43_9041,
    0xFE51_63AB_DEBB_C561,
    0xB724_6E3A_424D_D2E0,
    0x0649_2EEA_09D1_921C,
    0xFE1D_EB1C_B129_A73E,
    0xE882_35F5_2EBB_4484,
    0xE99C_7026_B45F_7E41,
    0x3991_D639_8353_39F4,
    0x9C84_5F8B_BDF9_283B,
    0x1FF8_97FF_DE05_980F,
    0xEF2F_118B_5A0A_6D1F,
    0x6D36_7ECF_27CB_09B7,
    0x4F46_3F66_9E5F_EA2D,
    0x7527_BAC7_EBE5_F17B,
    0x3D07_39F7_8A52_92EA,
    0x6BFB_5FB1_1F8D_5D08,
    0x5603_3046_FC7B_6BAB,
    0xF0CF_BC20_9AF4_361D,
];

/// pi/2 in four Cody-Waite pieces: P1..P3 carry 33 significant bits each, so `n * Pi` is
/// exact for `|n| < 2^20`; P4 is the rounded remainder. Sum = pi/2 to ~2^-150.
pub(crate) const PIO2_1: f64 = f64::from_bits(0x3FF9_21FB_5440_0000);
pub(crate) const PIO2_2: f64 = f64::from_bits(0x3DD0_B461_1A60_0000);
pub(crate) const PIO2_3: f64 = f64::from_bits(0x3BA3_198A_2E00_0000);
pub(crate) const PIO2_4: f64 = f64::from_bits(0x397B_839A_2520_49C1);

/// Round-to-nearest 2/pi — only used to *choose* the quadrant, so any close value works;
/// the pieces above carry the precision.
pub(crate) const INV_PIO2: f64 = f64::from_bits(0x3FE4_5F30_6DC9_C883);

/// pi/4 (the reduction threshold), rounded.
pub(crate) const PIO4: f64 = f64::from_bits(0x3FE9_21FB_5444_2D18);

/// Double-double pi/4, pi/2, 3pi/4, pi (hi = round(v), lo = round(v - hi)).
pub(crate) const PIO4_DD: Dd = Dd::new(
    f64::from_bits(0x3FE9_21FB_5444_2D18),
    f64::from_bits(0x3C81_A626_3314_5C07),
);
pub(crate) const PIO2_DD: Dd = Dd::new(
    f64::from_bits(0x3FF9_21FB_5444_2D18),
    f64::from_bits(0x3C91_A626_3314_5C07),
);
pub(crate) const THREE_PIO4_DD: Dd = Dd::new(
    f64::from_bits(0x4002_D97C_7F33_21D2),
    f64::from_bits(0x3C9A_7939_4C9E_8A0A),
);
pub(crate) const PI_DD: Dd = Dd::new(
    f64::from_bits(0x4009_21FB_5444_2D18),
    f64::from_bits(0x3CA1_A626_3314_5C07),
);

/// ln 2 in two pieces: LN2_HI carries 42 significant bits, so `k * LN2_HI` is exact for
/// `|k| < 2^11` (every exponent a double can produce); LN2_LO is the rounded remainder.
pub(crate) const LN2_HI: f64 = f64::from_bits(0x3FE6_2E42_FEFA_3800);
pub(crate) const LN2_LO: f64 = f64::from_bits(0x3D2E_F357_93C7_6730);

/// Round-to-nearest 1/ln 2 — only used to choose the exponent `k`.
pub(crate) const INV_LN2: f64 = f64::from_bits(0x3FF7_1547_652B_82FE);

/// Double-double atan(j/8) for j = 0..=8 (entry 0 is zero, entry 8 is pi/4).
pub(crate) const ATAN_J8: [Dd; 9] = [
    Dd::new(0.0, 0.0),
    Dd::new(
        f64::from_bits(0x3FBF_D5BA_9AAC_2F6E),
        f64::from_bits(0xBC4C_D376_8676_0C17),
    ),
    Dd::new(
        f64::from_bits(0x3FCF_5B75_F92C_80DD),
        f64::from_bits(0x3C68_AB6E_3CF7_AFBD),
    ),
    Dd::new(
        f64::from_bits(0x3FD6_F619_41E4_DEF1),
        f64::from_bits(0xBC7C_63AA_E6F6_E918),
    ),
    Dd::new(
        f64::from_bits(0x3FDD_AC67_0561_BB4F),
        f64::from_bits(0x3C7A_2B7F_222F_65E2),
    ),
    Dd::new(
        f64::from_bits(0x3FE1_E00B_ABDE_FEB4),
        f64::from_bits(0xBC59_28DF_287A_668F),
    ),
    Dd::new(
        f64::from_bits(0x3FE4_978F_A326_9EE1),
        f64::from_bits(0x3C72_419A_87F2_A458),
    ),
    Dd::new(
        f64::from_bits(0x3FE7_00A7_C578_4634),
        f64::from_bits(0xBC78_C34D_25AA_DEF6),
    ),
    PIO4_DD,
];

/// 1.5 * 2^52: adding and subtracting it rounds any |x| < 2^51 to the nearest integer
/// (ties to even) using only IEEE addition — no `round()` call into the platform.
pub(crate) const ROUND_MAGIC: f64 = 6_755_399_441_055_744.0;

/// The canonical quiet NaN. Every NaN `det` returns is exactly this bit pattern: x86-64
/// hardware produces a *negative* default NaN (0xFFF8...) and aarch64 a positive one, so a
/// NaN computed by an operation would hash differently on the two (Ch.3.1, failure mode 3).
pub(crate) const NAN: f64 = f64::from_bits(0x7FF8_0000_0000_0000);

/// `2^e` for `e` in the normal exponent range, built from bits.
#[inline(always)]
pub(crate) const fn pow2(e: i32) -> f64 {
    f64::from_bits(((e + 1023) as u64) << 52)
}

#[inline(always)]
pub(crate) fn round_int(x: f64) -> f64 {
    (x + ROUND_MAGIC) - ROUND_MAGIC
}

/// Double-double 1/3, for the `r^3/3` term of `ln_dd`'s `log1p` (a multiply, not a division).
pub(crate) const THIRD_DD: Dd = Dd::new(
    f64::from_bits(0x3FD5_5555_5555_5555),
    f64::from_bits(0x3C75_5555_5555_5555),
);
