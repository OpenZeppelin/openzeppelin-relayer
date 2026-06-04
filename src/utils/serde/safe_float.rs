//! Shared guard for recovering integers from floats produced by a Redis/Lua
//! `cjson` round-trip.
//!
//! When a large integer is re-encoded by Lua's `cjson` it can come back as a
//! float (e.g. `643918676885760.0`). For values within the exactly
//! representable range this is a lossless reformatting we can recover from.
//! Beyond 2^53, however, consecutive integers can no longer all be represented
//! distinctly as `f64`, so the original value is no longer knowable — in that
//! case we reject rather than return a silently wrong number.

use serde::de;

/// 2^53 - 1, i.e. JavaScript's `Number.MAX_SAFE_INTEGER`: the largest `N` such
/// that *every* integer in `[-N, N]` is exactly representable as `f64` and
/// round-trips uniquely. Individual larger integers (e.g. 2^53) are still
/// representable, but 2^53 + 1 is not — it collapses onto 2^53 — so once a value
/// exceeds this bound it can no longer be recovered from a float without an
/// authoritative source.
pub(crate) const MAX_SAFE_INTEGER_F64: f64 = 9_007_199_254_740_991.0;

/// Convert a JSON float that should have been an integer back into an `i128`,
/// rejecting non-integral values and any magnitude beyond the exactly
/// representable range (where precision may already have been lost).
pub(crate) fn f64_to_safe_integer<E>(value: f64) -> Result<i128, E>
where
    E: de::Error,
{
    if !value.is_finite() {
        return Err(E::custom(format!(
            "cannot convert non-finite float {value} to an integer"
        )));
    }
    if value.fract() != 0.0 {
        return Err(E::custom(format!(
            "cannot convert non-integral float {value} to an integer"
        )));
    }
    if value.abs() > MAX_SAFE_INTEGER_F64 {
        return Err(E::custom(format!(
            "float {value} exceeds the exactly-representable integer range \
             (2^53-1); it was floatified by a Lua cjson round-trip and cannot \
             be recovered safely"
        )));
    }
    Ok(value as i128)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::de::value::Error;

    #[test]
    fn accepts_value_at_safe_boundary() {
        assert_eq!(
            f64_to_safe_integer::<Error>(9_007_199_254_740_991.0).unwrap(),
            9_007_199_254_740_991
        );
    }

    #[test]
    fn accepts_real_world_corrupted_value() {
        assert_eq!(
            f64_to_safe_integer::<Error>(643918676885760.0).unwrap(),
            643918676885760
        );
    }

    #[test]
    fn rejects_value_beyond_safe_range() {
        // 2^53 + 1 is not representable and arrives as 2^53; we cannot trust it.
        assert!(f64_to_safe_integer::<Error>(9_007_199_254_740_993.0).is_err());
    }

    #[test]
    fn rejects_non_integral() {
        assert!(f64_to_safe_integer::<Error>(1.5).is_err());
    }
}
