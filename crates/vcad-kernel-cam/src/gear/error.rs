//! Errors from gear geometry.
//!
//! Every variant names the number that made the case impossible, because the
//! reader is a machinist deciding what to change: a smaller cutter, a shorter
//! addendum, a different pin.

use thiserror::Error;

/// Errors from [`super::SpurGear`] and the pair, cutter, contour, measurement
/// and planetary helpers built on it.
#[derive(Debug, Clone, PartialEq, Error)]
pub enum GearError {
    /// Module is zero, negative or not finite.
    #[error("invalid gear module: {0}")]
    InvalidModule(f64),

    /// Fewer than four teeth is not a gear.
    #[error("gear needs at least 4 teeth, got {0}")]
    TooFewTeeth(u32),

    /// Pressure angle outside (0, 90°).
    #[error("invalid pressure angle: {0} rad")]
    InvalidPressureAngle(f64),

    /// Face width is zero, negative or not finite.
    #[error("invalid face width: {0}")]
    InvalidFaceWidth(f64),

    /// Backlash thinning is negative or not finite.
    #[error("invalid backlash thinning: {0} (must be >= 0)")]
    InvalidBacklash(f64),

    /// Tip radius override is zero, negative or not finite.
    #[error("invalid tip radius override: {0}")]
    InvalidTipRadius(f64),

    /// A radius argument is zero, negative or not finite.
    #[error("invalid radius: {0}")]
    InvalidRadius(f64),

    /// Tip and root are the wrong way round for the gear's handedness.
    #[error(
        "degenerate gear proportions: tip radius {tip:.6}, root radius {root:.6} \
         ({}) — the tooth has no height", if *internal { "internal" } else { "external" }
    )]
    DegenerateProportions {
        /// Tip radius, mm.
        tip: f64,
        /// Root radius, mm.
        root: f64,
        /// True for an internal gear.
        internal: bool,
    },

    /// An internal gear whose tip circle is inside its own base circle: the
    /// flank would have to be an involute where no involute exists.
    #[error("internal gear tip radius {tip:.6} is inside its base circle {base:.6}")]
    TipInsideBaseCircle {
        /// Tip radius, mm.
        tip: f64,
        /// Base radius, mm.
        base: f64,
    },

    /// A radius below the base circle was handed to an involute-only routine.
    #[error("radius {radius:.6} is inside the base circle {base:.6}: there is no involute there")]
    RadiusBelowBaseCircle {
        /// The offending radius, mm.
        radius: f64,
        /// Base radius, mm.
        base: f64,
    },

    /// The flanks meet before the tip circle.
    #[error("z{teeth} tooth is pointed at tip radius {tip_radius:.6}: shorten the addendum")]
    PointedTooth {
        /// Tooth count.
        teeth: u32,
        /// Tip radius, mm.
        tip_radius: f64,
    },

    /// The inverse involute has no answer for this value.
    #[error("inverse involute has no solution for {0}")]
    InverseInvolute(f64),

    /// A flank-deviation tolerance that is negative or not finite.
    #[error("invalid flank deviation tolerance: {0} (must be >= 0)")]
    InvalidFlankTolerance(f64),

    /// Cutter diameter is zero, negative or not finite.
    #[error("invalid cutter diameter: {0}")]
    InvalidCutterDiameter(f64),

    /// The cutter cannot enter the tooth space at all.
    #[error(
        "Ø{cutter_diameter:.4} cutter does not fit the tooth space: the space is {space_width:.4} \
         wide at r{radius:.4}"
    )]
    CutterTooLarge {
        /// Cutter diameter, mm.
        cutter_diameter: f64,
        /// Space width at the tightest point the cutter must pass, mm.
        space_width: f64,
        /// Radius at which the space is that wide, mm.
        radius: f64,
    },

    /// No cutter, however small, leaves the involute intact past the contact
    /// limit — the tooth proportions themselves are the problem.
    #[error("no cutter diameter keeps the form radius clear of the contact limit {limit:.4}")]
    NoUsableCutter {
        /// Contact radius the form radius has to clear, mm.
        limit: f64,
    },

    /// Two gears that cannot mesh: different modules.
    #[error("modules differ: {a} and {b}")]
    ModuleMismatch {
        /// First gear's module.
        a: f64,
        /// Second gear's module.
        b: f64,
    },

    /// Two gears that cannot mesh: different pressure angles.
    #[error("pressure angles differ: {a} and {b} rad")]
    PressureAngleMismatch {
        /// First gear's pressure angle, rad.
        a: f64,
        /// Second gear's pressure angle, rad.
        b: f64,
    },

    /// A pair with the internal gear first, or two internal gears.
    #[error("invalid pair: an internal mesh is (external pinion, internal ring), in that order")]
    InvalidPairing,

    /// An internal pair whose ring does not have more teeth than its pinion.
    #[error("internal pair needs z_ring > z_pinion, got {pinion} and {ring}")]
    InternalToothCounts {
        /// Pinion tooth count.
        pinion: u32,
        /// Ring tooth count.
        ring: u32,
    },

    /// The pair's profile shifts admit no working pressure angle.
    #[error("no working pressure angle for this pair: profile shift sum {shift_sum} is too large")]
    NoWorkingPressureAngle {
        /// Sum (external) or difference (internal) of the profile shifts.
        shift_sum: f64,
    },

    /// A centre distance the pair cannot be spread to.
    #[error("centre distance {0:.6} is unreachable for this pair")]
    InvalidCentreDistance(f64),

    /// Contact ratio below one: the mesh loses contact between teeth.
    #[error("contact ratio {0:.4} is below 1.0: the mesh would drop out between teeth")]
    ContactRatioBelowOne(f64),

    /// Pin or ball diameter is zero, negative or not finite.
    #[error("invalid pin diameter: {0}")]
    InvalidPinDiameter(f64),

    /// The pin does not touch the involute flank between root and tip.
    #[error(
        "Ø{pin_diameter:.4} pin contacts at r{contact_radius:.4}, outside the usable flank \
         r{form_radius:.4}..r{tip_radius:.4}"
    )]
    PinContactOutsideFlank {
        /// Pin diameter, mm.
        pin_diameter: f64,
        /// Radius at which the pin touches the flank, mm.
        contact_radius: f64,
        /// Lowest usable flank radius, mm.
        form_radius: f64,
        /// Tip radius, mm.
        tip_radius: f64,
    },

    /// A measurement that no tooth thickness can produce.
    #[error("measurement {0:.6} mm is outside the range this gear can produce")]
    MeasurementOutOfRange(f64),

    /// A span of `k` teeth that does not straddle the flanks properly.
    #[error("span over {0} teeth does not land on the involute flanks of this gear")]
    InvalidSpanTeeth(u32),

    /// Span measurement asked of an internal gear.
    #[error("base tangent (span) measurement is defined for external gears only")]
    SpanOnInternalGear,

    /// A planetary train with fewer than two planets.
    #[error("a planetary train needs at least 2 planets, got {0}")]
    TooFewPlanets(u32),

    /// `(z_sun + z_ring)` is not divisible by the planet count.
    #[error(
        "planets do not assemble: (z_sun {sun} + z_ring {ring}) / {planets} = {quotient:.4} \
         is not an integer"
    )]
    AssemblyCondition {
        /// Sun tooth count.
        sun: u32,
        /// Ring tooth count.
        ring: u32,
        /// Planet count.
        planets: u32,
        /// The non-integer quotient.
        quotient: f64,
    },

    /// Adjacent planets overlap at their tip circles.
    #[error("adjacent planets overlap by {overlap:.4} mm at the tip circle")]
    PlanetsOverlap {
        /// Overlap, mm (positive means interference).
        overlap: f64,
    },

    /// The two meshes of a planetary train disagree on the centre distance.
    #[error(
        "sun-planet centre distance {sun_planet:.6} and planet-ring {planet_ring:.6} differ by \
         {delta:.6} mm: the ring profile shift does not close the train"
    )]
    CentreDistanceMismatch {
        /// Sun–planet centre distance, mm.
        sun_planet: f64,
        /// Planet–ring centre distance, mm.
        planet_ring: f64,
        /// Difference, mm.
        delta: f64,
    },
}
