//! Atmospheres parameterised by a body's composition (Ch.11, DoD M1-8).
//!
//! A body's air is described physically — its gases as mole fractions, surface pressure and
//! temperature, gravity, aerosols, an ozone column ([`AtmosphereBody`]) — and
//! [`AtmosphereBody::derive`] turns that into the coefficients Bruneton's precomputed
//! scattering model takes ([`AtmosphereParams`]), at the three wavelengths the renderer
//! samples ([`WAVELENGTHS`]: 680, 550, 440 nm):
//!
//! * **Rayleigh scattering** from first principles: each gas's per-molecule cross-section
//!   `sigma(l) = 24 pi^3 / (l^4 N_L^2) ((n^2 - 1) / (n^2 + 2))^2 F_K` from its refractivity at
//!   standard conditions (dispersed with the Ciddor/Edlen form for air) and its King factor;
//!   the mixture's coefficient is `N sum x_i sigma_i` with `N = P / (k T)` at the surface.
//! * **Scale height** `H = k T / (mu m_u g)` from the mixture's mean molar mass.
//! * **Aerosols** (Mie): extinction `tau / H_M (l / 550 nm)^-alpha`, per-channel single-scattering
//!   albedo, Cornette-Shanks asymmetry `g`.
//! * **Ozone** absorption: a tent profile 10-40 km peaking at 25 km holding the column
//!   (Dobson units) with Chappuis-band cross-sections.
//!
//! So the sky follows the air: a thin cold CO2 atmosphere is a thin sky, a dusty one is a red
//! one, with no artist-set colour (Ch.11's contract).
//!
//! [`ReferenceModel`] integrates the same model directly in `f64` (no lookup tables): the
//! transmittance along any segment and the single-scattered sky radiance of any ray. The GPU
//! tables are checked against it (`test_atmosphere`).

use forge_num::det;

use crate::error::SkyError;

/// The wavelengths (metres) of the renderer's three channels: red, green, blue.
pub const WAVELENGTHS: [f64; 3] = [680e-9, 550e-9, 440e-9];

/// Boltzmann's constant, J/K.
pub const BOLTZMANN: f64 = 1.380_649e-23;
/// The atomic mass unit, kg.
pub const ATOMIC_MASS: f64 = 1.660_539_066_60e-27;
/// Loschmidt's number: molecules per m^3 at 0 C and 101,325 Pa (where refractivities are
/// tabulated).
pub const LOSCHMIDT: f64 = 2.686_780_111e25;
/// One Dobson unit, molecules per m^2.
pub const DOBSON: f64 = 2.686_7e20;

/// A gas the model knows the optical constants of.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Gas {
    /// Molecular nitrogen.
    N2,
    /// Molecular oxygen.
    O2,
    /// Argon.
    Ar,
    /// Carbon dioxide.
    CO2,
    /// Water vapour.
    H2O,
    /// Methane.
    CH4,
    /// Molecular hydrogen.
    H2,
    /// Helium.
    He,
    /// Neon.
    Ne,
}

impl Gas {
    /// Every gas, in declaration order.
    pub const ALL: [Gas; 9] = [
        Gas::N2,
        Gas::O2,
        Gas::Ar,
        Gas::CO2,
        Gas::H2O,
        Gas::CH4,
        Gas::H2,
        Gas::He,
        Gas::Ne,
    ];

    /// Molar mass, g/mol.
    #[must_use]
    pub const fn molar_mass(self) -> f64 {
        match self {
            Gas::N2 => 28.0134,
            Gas::O2 => 31.9988,
            Gas::Ar => 39.948,
            Gas::CO2 => 44.0095,
            Gas::H2O => 18.0153,
            Gas::CH4 => 16.0425,
            Gas::H2 => 2.01588,
            Gas::He => 4.002_602,
            Gas::Ne => 20.1797,
        }
    }

    /// Refractivity `n - 1` at 550 nm, 0 C and 101,325 Pa.
    #[must_use]
    pub const fn refractivity_550(self) -> f64 {
        match self {
            Gas::N2 => 2.9806e-4,
            Gas::O2 => 2.7137e-4,
            Gas::Ar => 2.8175e-4,
            Gas::CO2 => 4.4967e-4,
            Gas::H2O => 2.5600e-4,
            Gas::CH4 => 4.4400e-4,
            Gas::H2 => 1.3950e-4,
            Gas::He => 3.4960e-5,
            Gas::Ne => 6.7100e-5,
        }
    }

    /// King correction factor (depolarisation) of the gas's Rayleigh cross-section.
    #[must_use]
    pub const fn king_factor(self) -> f64 {
        match self {
            Gas::N2 => 1.0350,
            Gas::O2 => 1.0960,
            Gas::Ar | Gas::He | Gas::Ne | Gas::H2 => 1.0,
            Gas::CO2 => 1.1364,
            Gas::H2O => 1.0010,
            Gas::CH4 => 1.0000,
        }
    }

    /// The per-molecule Rayleigh cross-section (m^2) at wavelength `lambda` (metres).
    #[must_use]
    pub fn rayleigh_cross_section(self, lambda: f64) -> f64 {
        let n1 = self.refractivity_550() * air_dispersion(lambda);
        let n2 = (1.0 + n1) * (1.0 + n1);
        let lorentz = (n2 - 1.0) / (n2 + 2.0);
        let pi3 = core::f64::consts::PI * core::f64::consts::PI * core::f64::consts::PI;
        let l2 = lambda * lambda;
        24.0 * pi3 / (l2 * l2 * LOSCHMIDT * LOSCHMIDT) * lorentz * lorentz * self.king_factor()
    }
}

/// `(n - 1)(lambda) / (n - 1)(550 nm)` for standard air (Ciddor 1996), used as the dispersion
/// of every gas (their dispersions differ from air's by a few per cent across the visible).
#[must_use]
pub fn air_dispersion(lambda: f64) -> f64 {
    let f = |l: f64| {
        let s2 = 1.0 / (l * 1e6 * l * 1e6);
        5_792_105.0 / (238.0185 - s2) + 167_917.0 / (57.362 - s2)
    };
    f(lambda) / f(550e-9)
}

/// Ozone's absorption cross-section (m^2) in the Chappuis band at the three channel
/// wavelengths (Serdyuchenko et al. 2014, 293 K, rounded).
pub const OZONE_CROSS_SECTION: [f64; 3] = [1.21e-25, 3.50e-25, 1.58e-26];

/// A mixture of gases by mole fraction.
#[derive(Clone, Debug, PartialEq)]
pub struct Composition {
    /// `(gas, mole fraction)`; fractions are normalised to sum to 1.
    pub gases: Vec<(Gas, f64)>,
}

impl Composition {
    /// Dry Earth air (plus 0.4 % water vapour).
    #[must_use]
    pub fn earth() -> Self {
        Self {
            gases: vec![
                (Gas::N2, 0.7808),
                (Gas::O2, 0.2095),
                (Gas::Ar, 0.0093),
                (Gas::CO2, 0.000_42),
                (Gas::H2O, 0.004),
            ],
        }
    }

    /// Mars: carbon dioxide with nitrogen and argon.
    #[must_use]
    pub fn mars() -> Self {
        Self {
            gases: vec![(Gas::CO2, 0.9532), (Gas::N2, 0.027), (Gas::Ar, 0.016)],
        }
    }

    /// Titan: nitrogen with methane.
    #[must_use]
    pub fn titan() -> Self {
        Self {
            gases: vec![(Gas::N2, 0.95), (Gas::CH4, 0.049), (Gas::H2, 0.001)],
        }
    }

    fn normalised(&self) -> Result<Vec<(Gas, f64)>, SkyError> {
        if self.gases.is_empty() {
            return Err(SkyError::atmosphere("no gases"));
        }
        let mut sum = 0.0;
        for &(g, x) in &self.gases {
            if !(x.is_finite() && x >= 0.0) {
                return Err(SkyError::atmosphere(format!("{g:?} fraction {x}")));
            }
            sum += x;
        }
        if sum <= 0.0 {
            return Err(SkyError::atmosphere("mole fractions sum to zero"));
        }
        Ok(self.gases.iter().map(|&(g, x)| (g, x / sum)).collect())
    }

    /// Mean molar mass, g/mol.
    pub fn mean_molar_mass(&self) -> Result<f64, SkyError> {
        Ok(self
            .normalised()?
            .iter()
            .map(|&(g, x)| x * g.molar_mass())
            .sum())
    }

    /// The mixture's Rayleigh cross-section per molecule (m^2) at each channel.
    pub fn rayleigh_cross_section(&self) -> Result<[f64; 3], SkyError> {
        let n = self.normalised()?;
        Ok(WAVELENGTHS.map(|l| {
            n.iter()
                .map(|&(g, x)| x * g.rayleigh_cross_section(l))
                .sum()
        }))
    }
}

/// Suspended particles (dust, haze, droplets): the Mie part of the model.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Aerosols {
    /// Vertical optical depth of the whole column at 550 nm.
    pub optical_depth_550: f64,
    /// Scale height of the aerosol density, metres.
    pub scale_height_m: f64,
    /// Angstrom exponent: extinction goes as `(lambda / 550 nm)^-alpha`.
    pub angstrom_alpha: f64,
    /// Single-scattering albedo per channel (scattering / extinction; dust absorbs blue).
    pub single_scattering_albedo: [f64; 3],
    /// Cornette-Shanks asymmetry parameter (forward scattering > 0).
    pub asymmetry_g: f64,
}

impl Aerosols {
    /// Clean continental air (Bruneton's reference aerosols: tau ~ 0.005).
    #[must_use]
    pub const fn earth_clean() -> Self {
        Self {
            optical_depth_550: 0.005_328,
            scale_height_m: 1200.0,
            angstrom_alpha: 0.0,
            single_scattering_albedo: [0.9, 0.9, 0.9],
            asymmetry_g: 0.8,
        }
    }

    /// Continental aerosol on a clear day (optical depth 0.08 at 550 nm, Angstrom exponent 1.3,
    /// 1.2 km scale height): the haze that whitens the horizon.
    #[must_use]
    pub const fn earth_continental() -> Self {
        Self {
            optical_depth_550: 0.08,
            scale_height_m: 1200.0,
            angstrom_alpha: 1.3,
            single_scattering_albedo: [0.9, 0.9, 0.9],
            asymmetry_g: 0.76,
        }
    }

    /// Martian dust at a typical optical depth, absorbing in the blue.
    #[must_use]
    pub const fn mars_dust() -> Self {
        Self {
            optical_depth_550: 0.5,
            scale_height_m: 11_000.0,
            angstrom_alpha: 0.0,
            single_scattering_albedo: [0.97, 0.93, 0.70],
            asymmetry_g: 0.65,
        }
    }

    /// No particles.
    #[must_use]
    pub const fn none() -> Self {
        Self {
            optical_depth_550: 0.0,
            scale_height_m: 1200.0,
            angstrom_alpha: 0.0,
            single_scattering_albedo: [1.0, 1.0, 1.0],
            asymmetry_g: 0.8,
        }
    }
}

/// A body's atmosphere, physically: what a world's models
/// produce and what the sky is derived from.
#[derive(Clone, Debug, PartialEq)]
pub struct AtmosphereBody {
    /// Radius of the atmosphere's bottom (sea level), metres.
    pub ground_radius_m: f64,
    /// Surface gravity, m/s^2.
    pub surface_gravity: f64,
    /// Surface pressure, pascals.
    pub surface_pressure_pa: f64,
    /// Mean temperature of the scattering layer, kelvin (isothermal model).
    pub temperature_k: f64,
    /// The gases.
    pub composition: Composition,
    /// Particles.
    pub aerosols: Aerosols,
    /// Ozone column, Dobson units (0 for none).
    pub ozone_du: f64,
    /// Mean ground albedo seen by multiply-scattered light (per channel).
    pub ground_albedo: [f64; 3],
    /// Angular radius of the star as seen from the body, radians (smooths the horizon).
    pub sun_angular_radius: f64,
}

impl AtmosphereBody {
    /// Earth: 101,325 Pa, 288 K, 9.807 m/s^2, 300 DU of ozone, continental haze.
    #[must_use]
    pub fn earth() -> Self {
        Self {
            ground_radius_m: 6.371e6,
            surface_gravity: 9.807,
            surface_pressure_pa: 101_325.0,
            temperature_k: 288.15,
            composition: Composition::earth(),
            aerosols: Aerosols::earth_continental(),
            ozone_du: 300.0,
            ground_albedo: [0.1, 0.1, 0.1],
            sun_angular_radius: 0.004_675,
        }
    }

    /// Mars: 610 Pa of carbon dioxide at 210 K under 3.72 m/s^2, with dust.
    #[must_use]
    pub fn mars() -> Self {
        Self {
            ground_radius_m: 3.3895e6,
            surface_gravity: 3.721,
            surface_pressure_pa: 610.0,
            temperature_k: 210.0,
            composition: Composition::mars(),
            aerosols: Aerosols::mars_dust(),
            ozone_du: 0.0,
            ground_albedo: [0.3, 0.2, 0.12],
            sun_angular_radius: 0.004_675 / 1.524,
        }
    }

    fn validate(&self) -> Result<(), SkyError> {
        let pos = |x: f64| x.is_finite() && x > 0.0;
        if !pos(self.ground_radius_m)
            || !pos(self.surface_gravity)
            || !pos(self.surface_pressure_pa)
            || !pos(self.temperature_k)
        {
            return Err(SkyError::atmosphere(format!(
                "radius {} m, gravity {}, pressure {} Pa, temperature {} K must be finite and > 0",
                self.ground_radius_m,
                self.surface_gravity,
                self.surface_pressure_pa,
                self.temperature_k
            )));
        }
        let a = &self.aerosols;
        if !(a.optical_depth_550.is_finite() && a.optical_depth_550 >= 0.0)
            || !pos(a.scale_height_m)
            || !a.angstrom_alpha.is_finite()
            || !(a.asymmetry_g > -1.0 && a.asymmetry_g < 1.0)
            || a.single_scattering_albedo
                .iter()
                .any(|s| !(0.0..=1.0).contains(s))
        {
            return Err(SkyError::atmosphere(format!("aerosols {a:?}")));
        }
        if !(self.ozone_du.is_finite() && self.ozone_du >= 0.0) {
            return Err(SkyError::atmosphere(format!("ozone {} DU", self.ozone_du)));
        }
        if self.ground_albedo.iter().any(|g| !(0.0..=1.0).contains(g)) {
            return Err(SkyError::atmosphere(format!(
                "ground albedo {:?}",
                self.ground_albedo
            )));
        }
        if !(self.sun_angular_radius > 0.0 && self.sun_angular_radius < 0.5) {
            return Err(SkyError::atmosphere(format!(
                "sun angular radius {} rad",
                self.sun_angular_radius
            )));
        }
        Ok(())
    }

    /// Molecules per m^3 at the surface (ideal gas).
    #[must_use]
    pub fn surface_number_density(&self) -> f64 {
        self.surface_pressure_pa / (BOLTZMANN * self.temperature_k)
    }

    /// Derive the scattering model's coefficients.
    pub fn derive(&self) -> Result<AtmosphereParams, SkyError> {
        self.validate()?;
        let mu = self.composition.mean_molar_mass()?;
        // A molecule of mean molar mass mu g/mol weighs mu atomic mass units.
        let rayleigh_h = BOLTZMANN * self.temperature_k / (mu * ATOMIC_MASS * self.surface_gravity);
        let n0 = self.surface_number_density();
        let sigma = self.composition.rayleigh_cross_section()?;
        let rayleigh = sigma.map(|s| n0 * s);
        let a = &self.aerosols;
        let ext550 = a.optical_depth_550 / a.scale_height_m;
        let mut mie_ext = [0.0; 3];
        let mut mie_sca = [0.0; 3];
        for k in 0..3 {
            let e = ext550 * det::pow(WAVELENGTHS[k] / 550e-9, -a.angstrom_alpha);
            mie_ext[k] = e;
            mie_sca[k] = e * a.single_scattering_albedo[k];
        }
        // Ozone: a tent from 10 to 40 km peaking at 25 km; the tent's integral is 15 km times
        // its peak, which holds the column.
        let ozone_peak = self.ozone_du * DOBSON / 15_000.0;
        let absorption = OZONE_CROSS_SECTION.map(|s| s * ozone_peak);
        let mut height = 10.0 * rayleigh_h;
        height = height.max(6.0 * a.scale_height_m);
        if self.ozone_du > 0.0 {
            height = height.max(48_000.0);
        }
        Ok(AtmosphereParams {
            bottom_radius: self.ground_radius_m,
            top_radius: self.ground_radius_m + height,
            rayleigh_scattering: rayleigh,
            rayleigh_density: DensityProfile::exponential(rayleigh_h),
            mie_scattering: mie_sca,
            mie_extinction: mie_ext,
            mie_density: DensityProfile::exponential(a.scale_height_m),
            mie_g: a.asymmetry_g,
            absorption_extinction: absorption,
            absorption_density: DensityProfile::ozone_tent(),
            ground_albedo: self.ground_albedo,
            sun_angular_radius: self.sun_angular_radius,
            mu_s_min: det::cos(102.0_f64.to_radians()),
        })
    }
}

/// One layer of a density profile: `clamp(exp_term e^(exp_scale h) + linear_term h +
/// constant, 0, 1)` for altitudes `h` (metres) below `width` (the last layer extends up).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DensityLayer {
    /// Altitude where this layer ends, metres (ignored for the upper layer).
    pub width: f64,
    /// Coefficient of the exponential.
    pub exp_term: f64,
    /// Rate of the exponential, 1/m.
    pub exp_scale: f64,
    /// Linear coefficient, 1/m.
    pub linear_term: f64,
    /// Constant.
    pub constant_term: f64,
}

impl DensityLayer {
    const ZERO: Self = Self {
        width: 0.0,
        exp_term: 0.0,
        exp_scale: 0.0,
        linear_term: 0.0,
        constant_term: 0.0,
    };

    fn density(&self, h: f64) -> f64 {
        (self.exp_term * det::exp(self.exp_scale * h) + self.linear_term * h + self.constant_term)
            .clamp(0.0, 1.0)
    }
}

/// A density profile of two layers (Bruneton's form): relative density at altitude `h`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DensityProfile {
    /// Lower layer (below its `width`).
    pub lower: DensityLayer,
    /// Upper layer.
    pub upper: DensityLayer,
}

impl DensityProfile {
    /// `e^(-h / H)`.
    #[must_use]
    pub fn exponential(scale_height: f64) -> Self {
        Self {
            lower: DensityLayer::ZERO,
            upper: DensityLayer {
                exp_term: 1.0,
                exp_scale: -1.0 / scale_height,
                ..DensityLayer::ZERO
            },
        }
    }

    /// Ozone's tent: 0 below 10 km, 1 at 25 km, 0 above 40 km.
    #[must_use]
    pub fn ozone_tent() -> Self {
        Self {
            lower: DensityLayer {
                width: 25_000.0,
                linear_term: 1.0 / 15_000.0,
                constant_term: -2.0 / 3.0,
                ..DensityLayer::ZERO
            },
            upper: DensityLayer {
                linear_term: -1.0 / 15_000.0,
                constant_term: 8.0 / 3.0,
                ..DensityLayer::ZERO
            },
        }
    }

    /// Relative density at altitude `h` (metres).
    #[must_use]
    pub fn density(&self, h: f64) -> f64 {
        if h < self.lower.width {
            self.lower.density(h)
        } else {
            self.upper.density(h)
        }
    }
}

/// The coefficients of Bruneton's precomputed atmospheric scattering model (SI units:
/// metres, 1/m), per channel ([`WAVELENGTHS`]). Derived from a body by
/// [`AtmosphereBody::derive`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AtmosphereParams {
    /// Radius of the bottom (sea level), metres.
    pub bottom_radius: f64,
    /// Radius of the top, metres.
    pub top_radius: f64,
    /// Rayleigh scattering coefficient at the bottom, 1/m.
    pub rayleigh_scattering: [f64; 3],
    /// Rayleigh density profile.
    pub rayleigh_density: DensityProfile,
    /// Mie scattering coefficient at the bottom, 1/m.
    pub mie_scattering: [f64; 3],
    /// Mie extinction coefficient at the bottom, 1/m.
    pub mie_extinction: [f64; 3],
    /// Mie density profile.
    pub mie_density: DensityProfile,
    /// Cornette-Shanks asymmetry.
    pub mie_g: f64,
    /// Absorption coefficient at the absorber's peak, 1/m.
    pub absorption_extinction: [f64; 3],
    /// Absorber density profile.
    pub absorption_density: DensityProfile,
    /// Ground albedo.
    pub ground_albedo: [f64; 3],
    /// The star's angular radius, radians.
    pub sun_angular_radius: f64,
    /// Cosine of the largest sun zenith angle the tables cover.
    pub mu_s_min: f64,
}

/// Rayleigh phase function.
#[must_use]
pub fn rayleigh_phase(nu: f64) -> f64 {
    3.0 / (16.0 * core::f64::consts::PI) * (1.0 + nu * nu)
}

/// Cornette-Shanks phase function.
#[must_use]
pub fn mie_phase(g: f64, nu: f64) -> f64 {
    let k = 3.0 / (8.0 * core::f64::consts::PI) * (1.0 - g * g) / (2.0 + g * g);
    let d = 1.0 + g * g - 2.0 * g * nu;
    k * (1.0 + nu * nu) / (d * det::sqrt(d))
}

/// The model integrated directly in `f64` (no tables): the reference the GPU tables are
/// checked against. Lengths in metres; radiance per unit solar irradiance.
#[derive(Clone, Copy, Debug)]
pub struct ReferenceModel {
    /// The coefficients.
    pub p: AtmosphereParams,
    /// Integration steps per segment.
    pub steps: u32,
}

impl ReferenceModel {
    /// A reference with 2,000 steps per segment.
    #[must_use]
    pub fn new(p: AtmosphereParams) -> Self {
        Self { p, steps: 2000 }
    }

    fn extinction_at(&self, r: f64) -> [f64; 3] {
        let p = &self.p;
        let h = r - p.bottom_radius;
        let (dr, dm, da) = (
            p.rayleigh_density.density(h),
            p.mie_density.density(h),
            p.absorption_density.density(h),
        );
        [0, 1, 2].map(|k| {
            p.rayleigh_scattering[k] * dr
                + p.mie_extinction[k] * dm
                + p.absorption_extinction[k] * da
        })
    }

    /// Distance from radius `r` along direction cosine `mu` to the top boundary.
    #[must_use]
    pub fn distance_to_top(&self, r: f64, mu: f64) -> f64 {
        let t = self.p.top_radius;
        let disc = r * r * (mu * mu - 1.0) + t * t;
        (-r * mu + det::sqrt(disc.max(0.0))).max(0.0)
    }

    /// Distance to the bottom boundary (valid when the ray hits it).
    #[must_use]
    pub fn distance_to_bottom(&self, r: f64, mu: f64) -> f64 {
        let b = self.p.bottom_radius;
        let disc = r * r * (mu * mu - 1.0) + b * b;
        (-r * mu - det::sqrt(disc.max(0.0))).max(0.0)
    }

    /// Does the ray from `r` along `mu` hit the ground?
    #[must_use]
    pub fn hits_ground(&self, r: f64, mu: f64) -> bool {
        let b = self.p.bottom_radius;
        mu < 0.0 && r * r * (mu * mu - 1.0) + b * b >= 0.0
    }

    /// Transmittance over the segment of length `d` from radius `r` along `mu` (trapezoid).
    #[must_use]
    pub fn transmittance(&self, r: f64, mu: f64, d: f64) -> [f64; 3] {
        let n = self.steps.max(2);
        let dx = d / f64::from(n);
        let mut sum = [0.0; 3];
        for i in 0..=n {
            let di = f64::from(i) * dx;
            let ri = det::sqrt(di * di + 2.0 * r * mu * di + r * r);
            let w = if i == 0 || i == n { 0.5 } else { 1.0 };
            let e = self.extinction_at(ri);
            for k in 0..3 {
                sum[k] += e[k] * w * dx;
            }
        }
        sum.map(|s| det::exp(-s))
    }

    /// Transmittance from radius `r` along `mu` to the top of the atmosphere (zero if the ray
    /// hits the ground).
    #[must_use]
    pub fn transmittance_to_top(&self, r: f64, mu: f64) -> [f64; 3] {
        if self.hits_ground(r, mu) {
            return [0.0; 3];
        }
        self.transmittance(r, mu, self.distance_to_top(r, mu))
    }

    /// Transmittance toward the sun (the fraction of the star's disc above the horizon
    /// smoothed as in the tables).
    #[must_use]
    pub fn transmittance_to_sun(&self, r: f64, mu_s: f64) -> [f64; 3] {
        let sin_h = self.p.bottom_radius / r;
        let cos_h = -det::sqrt((1.0 - sin_h * sin_h).max(0.0));
        let a = sin_h * self.p.sun_angular_radius;
        let x = ((mu_s - cos_h + a) / (2.0 * a)).clamp(0.0, 1.0);
        let visible = x * x * (3.0 - 2.0 * x);
        if visible == 0.0 {
            return [0.0; 3];
        }
        let t = self.transmittance(r, mu_s, self.distance_to_top(r, mu_s));
        t.map(|v| v * visible)
    }

    /// Single-scattered radiance per unit solar irradiance reaching a viewer at radius `r`
    /// looking along `mu` (cosine of the view zenith angle), the sun at `mu_s`, with `nu` the
    /// cosine of the view-sun angle. The ray stops at the ground or the top. `steps` samples
    /// along the view ray, each with its own sun and view transmittance integrals.
    #[must_use]
    pub fn single_scattering(&self, r: f64, mu: f64, mu_s: f64, nu: f64, steps: u32) -> [f64; 3] {
        let p = &self.p;
        // A viewer above the atmosphere starts at its top.
        let (mut r, mut mu) = (r, mu);
        if r > p.top_radius {
            let rmu = r * mu;
            let disc = rmu * rmu - r * r + p.top_radius * p.top_radius;
            if disc < 0.0 || rmu > 0.0 {
                return [0.0; 3];
            }
            let d = -rmu - det::sqrt(disc);
            let (nr, nmu) = advance(r, mu, d);
            r = nr;
            mu = nmu;
        }
        let ground = self.hits_ground(r, mu);
        let len = if ground {
            self.distance_to_bottom(r, mu)
        } else {
            self.distance_to_top(r, mu)
        };
        let n = steps.max(2);
        let inner = Self {
            p: self.p,
            steps: (self.steps / 4).max(50),
        };
        let mut ray = [0.0; 3];
        let mut mie = [0.0; 3];
        for i in 0..=n {
            // Samples crowd toward the densest air: t = len s^2 from the viewer (or its mirror
            // toward the ground), trapezoid in s.
            let s = f64::from(i) / f64::from(n);
            let (d, dt) = if ground {
                (len * (1.0 - (1.0 - s) * (1.0 - s)), 2.0 * len * (1.0 - s))
            } else {
                (len * s * s, 2.0 * len * s)
            };
            let dx = dt / f64::from(n);
            let (ri, _) = advance(r, mu, d);
            let mu_s_i = ((r * mu_s + d * nu) / ri).clamp(-1.0, 1.0);
            let t_view = inner.transmittance(r, mu, d);
            let t_sun = inner.transmittance_to_sun(ri, mu_s_i);
            let h = ri - p.bottom_radius;
            let (dr, dm) = (p.rayleigh_density.density(h), p.mie_density.density(h));
            let w = if i == 0 || i == n { 0.5 } else { 1.0 };
            for k in 0..3 {
                let t = t_view[k] * t_sun[k] * w * dx;
                ray[k] += t * dr;
                mie[k] += t * dm;
            }
        }
        let (pr, pm) = (rayleigh_phase(nu), mie_phase(p.mie_g, nu));
        [0, 1, 2]
            .map(|k| ray[k] * p.rayleigh_scattering[k] * pr + mie[k] * p.mie_scattering[k] * pm)
    }
}

/// Radius and direction cosine after moving `d` along a ray from `(r, mu)`.
#[must_use]
pub fn advance(r: f64, mu: f64, d: f64) -> (f64, f64) {
    let rd = det::sqrt((d * d + 2.0 * r * mu * d + r * r).max(0.0));
    let mud = if rd > 0.0 {
        ((r * mu + d) / rd).clamp(-1.0, 1.0)
    } else {
        1.0
    };
    (rd, mud)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Bucholtz (1995): Rayleigh optical depth of the standard atmosphere at 550 nm is 0.0973
    /// (column of 101,325 Pa of air); our cross-sections reproduce it to 3 %.
    #[test]
    fn earth_rayleigh_matches_bucholtz() {
        let c = Composition::earth();
        let sigma = c.rayleigh_cross_section().expect("sigma");
        let mu = c.mean_molar_mass().expect("mu");
        let column = 101_325.0 / (mu * ATOMIC_MASS * 9.807);
        let tau550 = sigma[1] * column;
        assert!(
            (tau550 - 0.0973).abs() / 0.0973 < 0.03,
            "tau(550) = {tau550}"
        );
        // Close to lambda^-4 with air's dispersion on top.
        let ratio = sigma[2] / sigma[0];
        assert!(
            ratio > 5.64 && ratio < 6.3,
            "sigma(440)/sigma(680) = {ratio}"
        );
    }

    #[test]
    fn earth_params_are_physical() {
        let p = AtmosphereBody::earth().derive().expect("derive");
        // Scale height ~8.4 km, sea-level Rayleigh ~1.15e-5 / m at 550 nm.
        let h = -1.0 / p.rayleigh_density.upper.exp_scale;
        assert!((h - 8430.0).abs() < 100.0, "H = {h}");
        assert!(
            (p.rayleigh_scattering[1] - 1.15e-5).abs() < 0.05e-5,
            "{:?}",
            p.rayleigh_scattering
        );
        // Ozone at its peak (Bruneton's reference: 0.65, 1.88, 0.085 e-6 / m).
        assert!((p.absorption_extinction[1] - 1.881e-6).abs() < 0.05e-6);
        assert!(p.top_radius - p.bottom_radius > 60_000.0);
    }

    #[test]
    fn composition_changes_the_sky() {
        // At the same pressure and temperature CO2 scatters ~2.5x more than N2.
        let body = |gases| AtmosphereBody {
            composition: Composition { gases },
            ..AtmosphereBody::earth()
        };
        let n2 = body(vec![(Gas::N2, 1.0)]).derive().expect("n2");
        let co2 = body(vec![(Gas::CO2, 1.0)]).derive().expect("co2");
        let r = co2.rayleigh_scattering[1] / n2.rayleigh_scattering[1];
        assert!(r > 2.2 && r < 2.8, "CO2 / N2 = {r}");
        // A heavier gas has a smaller scale height.
        assert!(co2.rayleigh_density.upper.exp_scale < n2.rayleigh_density.upper.exp_scale);
        // Mars: two orders of magnitude thinner Rayleigh, dust absorbing blue.
        let mars = AtmosphereBody::mars().derive().expect("mars");
        let earth = AtmosphereBody::earth().derive().expect("earth");
        assert!(mars.rayleigh_scattering[1] < earth.rayleigh_scattering[1] / 30.0);
        assert!(mars.mie_scattering[2] < mars.mie_scattering[0]);
    }

    #[test]
    fn invalid_bodies_are_refused() {
        let mut b = AtmosphereBody::earth();
        b.surface_pressure_pa = 0.0;
        assert_eq!(
            b.derive().map(|_| ()).unwrap_err().code().as_str(),
            "SKY-0001"
        );
        let mut b = AtmosphereBody::earth();
        b.composition.gases.clear();
        assert!(b.derive().is_err());
        let mut b = AtmosphereBody::earth();
        b.aerosols.asymmetry_g = 1.0;
        assert!(b.derive().is_err());
    }

    #[test]
    fn reference_transmittance_is_beer_lambert() {
        // A vertical column: exp(-(beta_R H_R + beta_M H_M + ozone 15 km)) to within the
        // truncation at the top.
        let p = AtmosphereBody::earth().derive().expect("derive");
        let m = ReferenceModel::new(p);
        let t = m.transmittance_to_top(p.bottom_radius, 1.0);
        let h_r = -1.0 / p.rayleigh_density.upper.exp_scale;
        let h_m = -1.0 / p.mie_density.upper.exp_scale;
        for (k, tk) in t.iter().enumerate() {
            let tau = p.rayleigh_scattering[k] * h_r
                + p.mie_extinction[k] * h_m
                + p.absorption_extinction[k] * 15_000.0;
            let expect = det::exp(-tau);
            assert!((tk - expect).abs() < 2e-4, "channel {k}: {tk} vs {expect}");
        }
        // Grazing rays are dimmer; rays into the ground are black.
        let g = m.transmittance_to_top(p.bottom_radius + 10.0, 0.0);
        assert!(g[2] < t[2] * 0.1);
        assert_eq!(
            m.transmittance_to_top(p.bottom_radius + 1000.0, -0.5),
            [0.0; 3]
        );
    }

    #[test]
    fn reference_sky_is_blue_at_noon_and_red_at_sunset() {
        let p = AtmosphereBody::earth().derive().expect("derive");
        let m = ReferenceModel { p, steps: 400 };
        let r = p.bottom_radius + 2.0;
        // Zenith with the sun 45 degrees up (not looking into the aureole).
        let c45 = det::sqrt(0.5);
        let zenith_noon = m.single_scattering(r, 1.0, c45, c45, 64);
        assert!(zenith_noon[2] > 2.0 * zenith_noon[0], "{zenith_noon:?}");
        // Looking at the horizon toward a setting sun.
        let mu_s = 0.02;
        let s = det::sqrt(1.0 - mu_s * mu_s);
        let horizon = m.single_scattering(r, 0.0, mu_s, s, 64);
        assert!(horizon[0] > horizon[2], "{horizon:?}");
    }

    #[test]
    fn density_profiles() {
        let o = DensityProfile::ozone_tent();
        assert_eq!(o.density(5_000.0), 0.0);
        assert!((o.density(25_000.0) - 1.0).abs() < 1e-12);
        assert!((o.density(32_500.0) - 0.5).abs() < 1e-12);
        assert_eq!(o.density(45_000.0), 0.0);
        let e = DensityProfile::exponential(8000.0);
        assert!((e.density(8000.0) - det::exp(-1.0)).abs() < 1e-15);
    }
}
