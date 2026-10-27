//! Finite binary64 interval bounds for the whole-arc contact screen.
//!
//! Basic operations assume IEEE 754 round-to-nearest binary64 arithmetic with
//! gradual underflow (no flush-to-zero), and enlarge each rounded result by one
//! adjacent representable value. Overflow,
//! an invalid domain, or a bound that cannot be represented returns `None`.
//! The square-root seed is accepted only after algebraic bound checks; no
//! accuracy allowance for the platform trigonometric library is used.

#[derive(Clone, Copy, Debug)]
pub(crate) struct Interval {
    pub lo: f64,
    pub hi: f64,
}

fn next_down(value: f64) -> f64 {
    if value == 0.0 {
        -f64::from_bits(1)
    } else if value > 0.0 {
        f64::from_bits(value.to_bits() - 1)
    } else {
        f64::from_bits(value.to_bits() + 1)
    }
}

fn next_up(value: f64) -> f64 {
    if value == 0.0 {
        f64::from_bits(1)
    } else if value > 0.0 {
        f64::from_bits(value.to_bits() + 1)
    } else {
        f64::from_bits(value.to_bits() - 1)
    }
}

fn down(value: f64) -> Option<f64> {
    if !value.is_finite() {
        return None;
    }
    let result = next_down(value);
    result.is_finite().then_some(result)
}

fn up(value: f64) -> Option<f64> {
    if !value.is_finite() {
        return None;
    }
    let result = next_up(value);
    result.is_finite().then_some(result)
}

impl Interval {
    pub(crate) fn new(lo: f64, hi: f64) -> Option<Self> {
        if !lo.is_finite() || !hi.is_finite() || lo > hi {
            return None;
        }
        Some(Self {
            lo: if lo == 0.0 { 0.0 } else { lo },
            hi: if hi == 0.0 { 0.0 } else { hi },
        })
    }

    pub(crate) fn point(value: f64) -> Option<Self> {
        Self::new(value, value)
    }

    fn valid(self) -> bool {
        self.lo.is_finite() && self.hi.is_finite() && self.lo <= self.hi
    }

    fn zero(self) -> bool {
        self.lo == 0.0 && self.hi == 0.0
    }

    // These nonfallible set operations require the constructor invariant.
    // Fallible arithmetic also checks it, including before exact identities.
    pub(crate) fn hull(self, other: Self) -> Self {
        assert!(self.valid() && other.valid(), "invalid interval in hull");
        Self {
            lo: self.lo.min(other.lo),
            hi: self.hi.max(other.hi),
        }
    }

    pub(crate) fn intersect(self, other: Self) -> Option<Self> {
        if !self.valid() || !other.valid() {
            return None;
        }
        Self::new(self.lo.max(other.lo), self.hi.min(other.hi))
    }

    pub(crate) fn add(self, other: Self) -> Option<Self> {
        if !self.valid() || !other.valid() {
            return None;
        }
        if self.zero() {
            return Some(other);
        }
        if other.zero() {
            return Some(self);
        }
        Self::new(down(self.lo + other.lo)?, up(self.hi + other.hi)?)
    }

    pub(crate) fn sub(self, other: Self) -> Option<Self> {
        if !self.valid() || !other.valid() {
            return None;
        }
        if other.zero() {
            return Some(self);
        }
        if self.lo == self.hi && other.lo == other.hi && self.lo == other.lo {
            return Self::point(0.0);
        }
        if self.zero() {
            return Self::new(-other.hi, -other.lo);
        }
        Self::new(down(self.lo - other.hi)?, up(self.hi - other.lo)?)
    }

    pub(crate) fn mul(self, other: Self) -> Option<Self> {
        if !self.valid() || !other.valid() {
            return None;
        }
        if self.zero() || other.zero() {
            return Self::point(0.0);
        }
        if other.lo == 1.0 && other.hi == 1.0 {
            return Some(self);
        }
        if self.lo == 1.0 && self.hi == 1.0 {
            return Some(other);
        }
        if other.lo == -1.0 && other.hi == -1.0 {
            return Self::new(-self.hi, -self.lo);
        }
        if self.lo == -1.0 && self.hi == -1.0 {
            return Self::new(-other.hi, -other.lo);
        }
        let products = [
            self.lo * other.lo,
            self.lo * other.hi,
            self.hi * other.lo,
            self.hi * other.hi,
        ];
        let mut lo = f64::MAX;
        let mut hi = -f64::MAX;
        for product in products {
            lo = lo.min(down(product)?);
            hi = hi.max(up(product)?);
        }
        Self::new(lo, hi)
    }

    pub(crate) fn div(self, other: Self) -> Option<Self> {
        if !self.valid() || !other.valid() || (other.lo <= 0.0 && other.hi >= 0.0) {
            return None;
        }
        if self.zero() {
            return Self::point(0.0);
        }
        if other.lo == 1.0 && other.hi == 1.0 {
            return Some(self);
        }
        if other.lo == -1.0 && other.hi == -1.0 {
            return Self::new(-self.hi, -self.lo);
        }
        let quotients = [
            self.lo / other.lo,
            self.lo / other.hi,
            self.hi / other.lo,
            self.hi / other.hi,
        ];
        let mut lo = f64::MAX;
        let mut hi = -f64::MAX;
        for quotient in quotients {
            lo = lo.min(down(quotient)?);
            hi = hi.max(up(quotient)?);
        }
        Self::new(lo, hi)
    }

    pub(crate) fn scale(self, factor: f64) -> Option<Self> {
        self.mul(Self::point(factor)?)
    }

    pub(crate) fn expand(self, amount: f64) -> Option<Self> {
        if !self.valid() || !amount.is_finite() || amount < 0.0 {
            return None;
        }
        if amount == 0.0 {
            return Some(self);
        }
        Self::new(down(self.lo - amount)?, up(self.hi + amount)?)
    }

    pub(crate) fn sqrt(self) -> Option<Self> {
        if !self.valid() || self.lo < 0.0 {
            return None;
        }
        if self.zero() {
            return Self::point(0.0);
        }
        let mut lo = self.lo.sqrt();
        let mut hi = self.hi.sqrt();
        if !lo.is_finite() || !hi.is_finite() || lo < 0.0 || hi < 0.0 {
            return None;
        }
        let mut lower_verified = false;
        for step in 0..=32 {
            // Zero is an exact lower bound, including when the radicand
            // starts at zero. Otherwise an upward square proves lo^2 <= x.
            if lo == 0.0 || up(lo * lo).is_some_and(|square| square <= self.lo) {
                lower_verified = true;
                break;
            }
            if step == 32 {
                break;
            }
            lo = next_down(lo).max(0.0);
        }
        let mut upper_verified = false;
        for step in 0..=32 {
            // A downward square proves hi^2 >= x. Unresolvable products,
            // including square underflow/overflow, cannot grant a bound.
            if down(hi * hi).is_some_and(|square| square >= self.hi) {
                upper_verified = true;
                break;
            }
            if step == 32 {
                break;
            }
            hi = next_up(hi);
            if !hi.is_finite() {
                return None;
            }
        }
        if !lower_verified || !upper_verified {
            return None;
        }
        Self::new(lo, hi)
    }

    pub(crate) fn positive_part(self) -> Self {
        assert!(self.valid(), "invalid interval in positive_part");
        Self {
            lo: self.lo.max(0.0),
            hi: self.hi.max(0.0),
        }
    }

    pub(crate) fn max(self, other: Self) -> Self {
        assert!(self.valid() && other.valid(), "invalid interval in max");
        Self {
            lo: self.lo.max(other.lo),
            hi: self.hi.max(other.hi),
        }
    }
}

/// Enclose sin and cos using 24 Taylor terms and an explicit remainder.
///
/// Each recurrence encloses a nonnegative absolute term. After the last sine
/// term (degree 47), the zero degree-48 coefficient allows the degree-49
/// Lagrange bound x^49/49!. For cosine (degree 46), the zero degree-47
/// coefficient similarly gives x^48/48!. Every derivative has magnitude <= 1.
fn sine_cosine(angle: Interval) -> Option<(Interval, Interval)> {
    let square = angle.mul(angle)?.positive_part();
    let mut sine_term = angle;
    let mut cosine_term = Interval::point(1.0)?;
    let mut sine = sine_term;
    let mut cosine = cosine_term;
    for k in 1..24_u32 {
        let sine_divisor = f64::from((2 * k) * (2 * k + 1));
        let cosine_divisor = f64::from((2 * k - 1) * (2 * k));
        sine_term = sine_term.mul(square)?.div(Interval::point(sine_divisor)?)?.positive_part();
        cosine_term = cosine_term.mul(square)?.div(Interval::point(cosine_divisor)?)?.positive_part();
        if k % 2 == 0 {
            sine = sine.add(sine_term)?;
            cosine = cosine.add(cosine_term)?;
        } else {
            sine = sine.sub(sine_term)?;
            cosine = cosine.sub(cosine_term)?;
        }
    }
    let sine_remainder = sine_term.mul(square)?.div(Interval::point(48.0 * 49.0)?)?.positive_part().hi;
    let cosine_remainder = cosine_term.mul(square)?.div(Interval::point(47.0 * 48.0)?)?.positive_part().hi;
    let unit = Interval::new(0.0, 1.0)?;
    // The caller restricts the complete angle interval to [0, pi/2).
    // Intersecting with these exact analytic ranges is therefore sound.
    Some((
        sine.expand(sine_remainder)?.intersect(unit)?,
        cosine.expand(cosine_remainder)?.intersect(unit)?,
    ))
}

/// Enclose tan(phi) for exact binary64 degrees and a declared absolute angle
/// uncertainty in radians. Physical angles are clipped at zero; an interval
/// reaching the conservative lower bound for pi/2 is unresolved (`None`).
pub(crate) fn friction_tangent(degrees: f64, uncertainty_rad: f64) -> Option<Interval> {
    if !degrees.is_finite() || degrees < 0.0 || !uncertainty_rad.is_finite() || uncertainty_rad < 0.0 {
        return None;
    }
    if degrees == 0.0 && uncertainty_rad == 0.0 {
        return Interval::point(0.0);
    }
    // These adjacent binary64 numbers bracket mathematical pi:
    // 3.141592653589793115997... < pi < 3.141592653589793560087...
    let pi_lo = f64::from_bits(0x4009_21fb_5444_2d18);
    let pi_hi = f64::from_bits(0x4009_21fb_5444_2d19);
    let pi = Interval::new(pi_lo, pi_hi)?;
    // Halving these normal binary64 constants is exact.
    let half_pi_lower = pi_lo * 0.5;
    let angle = Interval::point(degrees)?
        .mul(pi)?
        .div(Interval::point(180.0)?)?
        .expand(uncertainty_rad)?
        .intersect(Interval::new(0.0, f64::MAX)?)?;
    if angle.hi >= half_pi_lower {
        return None;
    }
    let (sine, cosine) = sine_cosine(angle)?;
    if cosine.lo <= 0.0 {
        return None;
    }
    // Both exact functions are nonnegative on the supported physical domain.
    Some(sine.div(cosine)?.positive_part())
}

#[cfg(test)]
mod tests {
    use super::{Interval, friction_tangent};

    #[test]
    fn contact_interval_arithmetic_encloses_signed_ranges() {
        let a = Interval::new(2.0, 3.0).unwrap();
        let b = Interval::new(-4.0, -1.0).unwrap();
        let product = a.mul(b).unwrap();
        assert!(product.lo <= -12.0 && product.hi >= -2.0 && product.hi < 0.0);
        let quotient = a.div(b).unwrap();
        assert!(quotient.lo <= -3.0 && quotient.hi >= -0.5 && quotient.hi < 0.0);
        let rounded_sum = Interval::point(0.1).unwrap().add(Interval::point(0.2).unwrap()).unwrap();
        assert!(rounded_sum.lo <= 0.3 && rounded_sum.hi >= 0.30000000000000004);
        let maximum = a.max(Interval::new(-1.0, 4.0).unwrap());
        assert_eq!((maximum.lo, maximum.hi), (2.0, 4.0));
    }

    #[test]
    fn contact_interval_invalid_domains_and_overflow_fail_closed() {
        assert!(Interval::new(f64::NAN, 1.0).is_none());
        assert!(Interval::new(2.0, 1.0).is_none());
        assert!(Interval::point(1.0).unwrap().div(Interval::new(-1.0, 1.0).unwrap()).is_none());
        assert!(Interval::point(f64::MAX).unwrap().scale(2.0).is_none());
        assert!(Interval::point(1.0).unwrap().expand(-1.0).is_none());
        assert!(Interval::point(0.0).unwrap().scale(f64::INFINITY).is_none());
    }

    #[test]
    fn contact_interval_square_roots_have_verified_endpoints() {
        let zero = Interval::point(0.0).unwrap().sqrt().unwrap();
        assert_eq!((zero.lo, zero.hi), (0.0, 0.0));
        let root = Interval::new(2.0, 9.0).unwrap().sqrt().unwrap();
        assert!(root.lo >= 0.0 && root.lo * root.lo <= 2.0);
        assert!(root.hi >= 3.0 && root.hi * root.hi >= 9.0);
        assert!(Interval::new(-1.0, 1.0).unwrap().sqrt().is_none());
        // At this scale a squared binary64 enclosure cannot resolve the lower
        // bound within the bounded ULP budget; it must not invent precision.
        assert!(Interval::point(f64::from_bits(1)).unwrap().sqrt().is_none());
    }

    #[test]
    fn contact_interval_friction_tangent_encloses_exact_reference_angles() {
        let zero = friction_tangent(0.0, 0.0).unwrap();
        assert_eq!((zero.lo, zero.hi), (0.0, 0.0));
        let forty_five = friction_tangent(45.0, 0.0).unwrap();
        assert!(forty_five.lo <= 1.0 && forty_five.hi >= 1.0);
        assert!(forty_five.hi - forty_five.lo < 1e-10);
        let uncertain_zero = friction_tangent(0.0, 1e-12).unwrap();
        assert_eq!(uncertain_zero.lo, 0.0);
        assert!(uncertain_zero.hi >= 1e-12);
        assert!(friction_tangent(90.0, 0.0).is_none());
        assert!(friction_tangent(45.0, -1e-12).is_none());
    }
}
