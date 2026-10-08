//! Platform-independent elementary functions.
//!
//! Rust's `f64::exp`, `ln`, `sin` and the like call the operating system's
//! math library, whose last bit differs between glibc versions, Apple's libm
//! and the Windows runtime. FARIS evidence files are verified by recomputing
//! them and comparing SHA-256 digests, so every transcendental function in the
//! model and engine goes through this module, which uses the pure-Rust `libm`
//! crate (identical results on every target). Clippy rejects the std methods
//! in those crates (see `clippy.toml`).
//!
//! `sqrt`, `abs`, `floor`, `round` and the basic arithmetic operators are exact
//! under IEEE 754 and stay on std. Do not use `mul_add` (fused multiply-add is
//! hardware dependent).
#![allow(clippy::disallowed_methods)]

pub fn exp(x: f64) -> f64 {
    libm::exp(x)
}
pub fn exp_m1(x: f64) -> f64 {
    libm::expm1(x)
}
pub fn ln(x: f64) -> f64 {
    libm::log(x)
}
pub fn ln_1p(x: f64) -> f64 {
    libm::log1p(x)
}
pub fn log10(x: f64) -> f64 {
    libm::log10(x)
}
pub fn log2(x: f64) -> f64 {
    libm::log2(x)
}
pub fn powf(x: f64, y: f64) -> f64 {
    libm::pow(x, y)
}
pub fn sin(x: f64) -> f64 {
    libm::sin(x)
}
pub fn cos(x: f64) -> f64 {
    libm::cos(x)
}
pub fn tan(x: f64) -> f64 {
    libm::tan(x)
}
pub fn atan2(y: f64, x: f64) -> f64 {
    libm::atan2(y, x)
}
pub fn tanh(x: f64) -> f64 {
    libm::tanh(x)
}
pub fn cbrt(x: f64) -> f64 {
    libm::cbrt(x)
}
pub fn hypot(x: f64, y: f64) -> f64 {
    libm::hypot(x, y)
}

/// Integer power by binary exponentiation in a fixed order, using only IEEE
/// multiplication and division (LLVM's `powi` intrinsic may expand differently
/// per target). `powi(x, 2)` is exactly `x * x` and `powi(x, 3)` is
/// `(x * x) * x`.
pub fn powi(x: f64, n: i32) -> f64 {
    let mut exponent = n.unsigned_abs();
    let mut base = x;
    let mut result = 1.0;
    while exponent > 0 {
        if exponent & 1 == 1 {
            result *= base;
        }
        exponent >>= 1;
        if exponent > 0 {
            base *= base;
        }
    }
    if n < 0 { 1.0 / result } else { result }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn powi_matches_repeated_multiplication() {
        let x = 1.37_f64;
        assert_eq!(powi(x, 0), 1.0);
        assert_eq!(powi(x, 1), x);
        assert_eq!(powi(x, 2), x * x);
        assert_eq!(powi(x, 3), (x * x) * x);
        assert_eq!(powi(x, -2), 1.0 / (x * x));
        assert_eq!(powi(10.0, 6), 1.0e6);
    }

    #[test]
    fn libm_values_are_pinned() {
        // Exactly representable or correctly-rounded anchors.
        assert_eq!(exp(0.0), 1.0);
        assert_eq!(ln(1.0), 0.0);
        assert_eq!(exp_m1(0.0), 0.0);
        assert_eq!(powf(2.0, 10.0), 1024.0);
        assert_eq!(hypot(3.0, 4.0), 5.0);
        assert!((exp(1.0) - std::f64::consts::E).abs() < 1e-15);
    }
}
