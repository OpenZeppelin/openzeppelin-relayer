//! Shared guard for recovering integers from floats produced by a Redis/Lua
//! `cjson` round-trip.
//!
//! When a large integer is re-encoded by Lua's `cjson` it can come back as a
//! float (e.g. `643918676885760.0`). For values within the exactly
//! representable range this is a lossless reformatting we can recover from, but
//! beyond 2^53 distinct integers collapse onto the same `f64`, so the original
//! value is no longer knowable. In that case we reject rather than return a
//! silently wrong number.

use serde::de;

/// 2^53 - 1: the largest-magnitude integer that `f64` represents exactly.
/// Beyond this, `n` and `n + 1` can map to the same double, so a floatified
/// value can no longer be recovered without an authoritative source.
pub(crate) const MAX_SAFE_INTEGER_F64: f64 = 9_007_199_254_740_991.0;

/// Convert a JSON float that should have been an integer back into an `i128`,
/// rejecting non-integral values and any magnitude beyond the exactly
/// representable range (where precision may already have been lost).
pub(crate) fn f64_to_safe_integer<E>(value: f64) -> Result<i128, E>
where
    E: de::Error,
{
    if !value.is_finite() || value.fract() != 0.0 {
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
