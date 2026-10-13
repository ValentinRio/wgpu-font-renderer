// Centre rays approximate a box filter: vertices and tight features inside a
// pixel can change area without changing either centre ray. These frozen bounds
// describe that approximation, not allowance for implementation mistakes.
pub const C3_MAX_ERROR: f32 = 0.39; // Measured weighted maximum: 0.343367 (13.6% headroom).
pub const C3_MEAN_ERRORS: [(f32, f64); 5] = [
    (12., 0.018),   // Measured worst font mean: 0.014783 (21.8% headroom).
    (16., 0.014),   // Measured worst font mean: 0.011615 (20.5% headroom).
    (24., 0.010),   // Measured worst font mean: 0.008327 (20.1% headroom).
    (48., 0.0055),  // Measured worst font mean: 0.004452 (23.5% headroom).
    (200., 0.0014), // Measured worst font mean: 0.001129 (24.0% headroom).
];
// Dense CJK glyphs have more edge pixels per area, increasing box-filter mean
// error. These bounds are frozen like the Latin mean bounds above.
pub const C3_DENSE_MEAN_ERRORS: [(f32, f64); 5] = [
    (12., 0.032),   // Measured Noto Serif SC mean: 0.026539 (20.6% headroom).
    (16., 0.0285),  // Measured Noto Serif SC mean: 0.023661 (20.5% headroom).
    (24., 0.019),   // Measured Noto Serif SC mean: 0.015749 (20.6% headroom).
    (48., 0.009),   // Measured Noto Serif SC mean: 0.007422 (21.3% headroom).
    (200., 0.0019), // Measured Noto Serif SC mean: 0.001587 (19.7% headroom).
];
pub const READBACK_ERROR: f32 = 1.5 / 255.;
// Measured worst vertex-free curved-edge error: 0.07421875. Curvature
// and corner-only intersections of a pixel still differ from two centre rays.
pub const C5_NO_STREAK_ERROR: f32 = 0.075;
// Measured straight-edge worst: 0.00196081 (half a readback step).
pub const C5_STRAIGHT_ERROR: f32 = 0.0021;
