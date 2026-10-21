//! Adaptive scoring (D21, docs/autocomplete.md 3.3).

use std::time::{SystemTime, UNIX_EPOCH};

/// Days since 1970-01-01 UTC.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct Day(pub u32);

impl Day {
    /// The UTC day containing `t`; times before 1970 map to day 0.
    pub fn from_system_time(t: SystemTime) -> Day {
        let secs = t.duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs());
        Day(u32::try_from(secs / 86_400).unwrap_or(u32::MAX))
    }

    /// Today, from the system clock.
    pub fn today() -> Day {
        Day::from_system_time(SystemTime::now())
    }
}

/// Ranking constants. Not user config; tuned by the evaluation (section 9).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ScoreParams {
    /// Weight of `log2(1 + accepts)`.
    pub alpha: f32,
    /// Weight of `log2(1 + rejects)`.
    pub beta: f32,
    /// Half-life of accepts and rejects, in days.
    pub half_life_days: f32,
    /// Rejection weight of a skipped row.
    pub skip: f32,
    /// Rejection weight of an ignored row.
    pub ignore: f32,
    /// Rejection weight of a dismissed row.
    pub dismiss: f32,
    /// Lower bound of the learned term.
    pub learned_min: f32,
    /// Upper bound of the learned term.
    pub learned_max: f32,
    /// Upper bound of the stored accepts and rejects.
    pub cap: f32,
}

impl Default for ScoreParams {
    fn default() -> Self {
        ScoreParams {
            alpha: 1.5,
            beta: 1.0,
            half_life_days: 30.0,
            skip: 0.25,
            ignore: 0.5,
            dismiss: 1.0,
            learned_min: -4.0,
            learned_max: 6.0,
            cap: 64.0,
        }
    }
}

fn clamp_or(v: f32, lo: f32, hi: f32, default: f32) -> f32 {
    if v.is_nan() {
        default
    } else {
        v.max(lo).min(hi)
    }
}

impl ScoreParams {
    /// Every field forced into its documented bounds (NaN becomes the default).
    pub fn clamped(&self) -> ScoreParams {
        let d = ScoreParams::default();
        ScoreParams {
            alpha: clamp_or(self.alpha, 0.0, 3.0, d.alpha),
            beta: clamp_or(self.beta, 0.0, 3.0, d.beta),
            half_life_days: clamp_or(self.half_life_days, 7.0, 365.0, d.half_life_days),
            skip: clamp_or(self.skip, 0.0, 1.0, d.skip),
            ignore: clamp_or(self.ignore, 0.0, 2.0, d.ignore),
            dismiss: clamp_or(self.dismiss, 0.0, 4.0, d.dismiss),
            learned_min: clamp_or(self.learned_min, -8.0, 0.0, d.learned_min),
            learned_max: clamp_or(self.learned_max, 0.0, 8.0, d.learned_max),
            cap: clamp_or(self.cap, 1.0, 1000.0, d.cap),
        }
    }
}

/// Multiplier that decays a value recorded on `from` to `to` (clock going backwards: 1).
pub(crate) fn decay_factor(from: Day, to: Day, p: &ScoreParams) -> f32 {
    let days = to.0.saturating_sub(from.0) as f32;
    (-days / p.half_life_days).exp2()
}

/// The clamped learned term for already decayed `accepts` and `rejects`.
pub(crate) fn learned_term(accepts: f32, rejects: f32, p: &ScoreParams) -> f32 {
    let a = accepts.max(0.0);
    let r = rejects.max(0.0);
    let raw = p.alpha * (1.0 + a).log2() - p.beta * (1.0 + r).log2();
    clamp_or(raw, p.learned_min, p.learned_max, 0.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn day_from_time() {
        assert_eq!(Day::from_system_time(UNIX_EPOCH), Day(0));
        let t = UNIX_EPOCH + Duration::from_secs(86_400 * 20_000 + 5);
        assert_eq!(Day::from_system_time(t), Day(20_000));
        assert_eq!(
            Day::from_system_time(UNIX_EPOCH - Duration::from_secs(10)),
            Day(0)
        );
    }

    #[test]
    fn clamped_bounds() {
        let p = ScoreParams {
            alpha: 9.0,
            beta: -1.0,
            half_life_days: 1.0,
            skip: f32::NAN,
            ignore: 3.0,
            dismiss: f32::INFINITY,
            learned_min: -100.0,
            learned_max: 100.0,
            cap: 0.0,
        }
        .clamped();
        assert_eq!(p.alpha, 3.0);
        assert_eq!(p.beta, 0.0);
        assert_eq!(p.half_life_days, 7.0);
        assert_eq!(p.skip, 0.25);
        assert_eq!(p.ignore, 2.0);
        assert_eq!(p.dismiss, 4.0);
        assert_eq!(p.learned_min, -8.0);
        assert_eq!(p.learned_max, 8.0);
        assert_eq!(p.cap, 1.0);
        assert_eq!(ScoreParams::default().clamped(), ScoreParams::default());
    }

    #[test]
    fn decay_and_learned() {
        let p = ScoreParams::default();
        assert_eq!(decay_factor(Day(10), Day(40), &p), 0.5);
        assert_eq!(decay_factor(Day(40), Day(10), &p), 1.0);
        assert!((learned_term(1.0, 0.0, &p) - 1.5).abs() < 1e-6);
        assert!((learned_term(0.0, 1.0, &p) + 1.0).abs() < 1e-6);
        assert_eq!(learned_term(1000.0, 0.0, &p), 6.0);
        assert_eq!(learned_term(0.0, 1e9, &p), -4.0);
        assert_eq!(learned_term(f32::NAN, 0.0, &p), 0.0);
    }
}
