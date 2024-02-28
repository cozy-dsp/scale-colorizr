use nih_plug::debug::nih_debug_assert;
use std::f32::consts;
use std::ops::{Add, Mul, Sub};
use std::simd::f32x2;

/// A simple biquad filter with functions for generating coefficients for an all-pass filter.
/// Stolen from NIH-Plug examples
///
/// Based on <https://en.wikipedia.org/wiki/Digital_biquad_filter#Transposed_direct_forms>.
///
/// The type parameter T  should be either an `f32` or a SIMD type.
#[derive(Clone, Copy, Debug)]
pub struct Biquad<T> {
    pub coefficients: BiquadCoefficients<T>,
    s1: T,
    s2: T,
}

/// The coefficients `[b0, b1, b2, a1, a2]` for [`Biquad`]. These coefficients are all
/// prenormalized, i.e. they have been divided by `a0`.
///
/// The type parameter T  should be either an `f32` or a SIMD type.
#[derive(Clone, Copy, Debug)]
pub struct BiquadCoefficients<T> {
    b0: T,
    b1: T,
    b2: T,
    a1: T,
    a2: T,
}

/// Either an `f32` or some SIMD vector type of `f32`s that can be used with our biquads.
pub trait SimdType:
    Mul<Output = Self> + Sub<Output = Self> + Add<Output = Self> + Copy + Sized
{
    fn from_f32(value: f32) -> Self;
}

impl<T: SimdType> Default for Biquad<T> {
    /// Before setting constants the filter should just act as an identity function.
    fn default() -> Self {
        Self {
            coefficients: BiquadCoefficients::identity(),
            s1: T::from_f32(0.0),
            s2: T::from_f32(0.0),
        }
    }
}

impl<T: SimdType> Biquad<T> {
    pub fn new(biquad_coefficients: BiquadCoefficients<T>) -> Self {
        Self {
            coefficients: biquad_coefficients,
            s1: T::from_f32(0.0),
            s2: T::from_f32(0.0),
        }
    }

    /// Process a single sample.
    pub fn process(&mut self, sample: T) -> T {
        let result = self.coefficients.b0 * sample + self.s1;

        self.s1 = self.coefficients.b1 * sample - self.coefficients.a1 * result + self.s2;
        self.s2 = self.coefficients.b2 * sample - self.coefficients.a2 * result;

        result
    }

    /// Reset the state to zero, useful after making making large, non-interpolatable changes to the
    /// filter coefficients.
    pub fn reset(&mut self) {
        self.s1 = T::from_f32(0.0);
        self.s2 = T::from_f32(0.0);
    }
}

impl<T: SimdType> BiquadCoefficients<T> {
    /// Convert scalar coefficients into the correct vector type.
    pub fn from_f32s(scalar: BiquadCoefficients<f32>) -> Self {
        Self {
            b0: T::from_f32(scalar.b0),
            b1: T::from_f32(scalar.b1),
            b2: T::from_f32(scalar.b2),
            a1: T::from_f32(scalar.a1),
            a2: T::from_f32(scalar.a2),
        }
    }

    /// Filter coefficients that would cause the sound to be passed through as is.
    pub fn identity() -> Self {
        Self::from_f32s(BiquadCoefficients {
            b0: 1.0,
            b1: 0.0,
            b2: 0.0,
            a1: 0.0,
            a2: 0.0,
        })
    }

    /// Compute the coefficients for a bandpass filter.
    ///
    /// Based on <http://shepazu.github.io/Audio-EQ-Cookbook/audio-eq-cookbook.html>.
    pub fn bandpass(sample_rate: f32, frequency: f32, q: f32) -> Self {
        nih_debug_assert!(sample_rate > 0.0);
        nih_debug_assert!(frequency > 0.0);
        nih_debug_assert!(frequency < sample_rate / 2.0);
        nih_debug_assert!(q > 0.0);

        let omega0 = consts::TAU * (frequency / sample_rate);
        let (sin_omega0, cos_omega0) = omega0.sin_cos();
        let alpha = sin_omega0 / (2.0 * q);

        // We'll prenormalize everything with a0
        let a0 = 1.0 + alpha;
        let b0 = (q * alpha) / a0;
        let b1 = 0.;
        let b2 = (-q * alpha) / a0;
        let a1 = (-2.0 * cos_omega0) / a0;
        let a2 = (1.0 - alpha) / a0;

        Self::from_f32s(BiquadCoefficients { b0, b1, b2, a1, a2 })
    }

    /// Compute the coefficients for a bandpass filter.
    ///
    /// Based on <http://shepazu.github.io/Audio-EQ-Cookbook/audio-eq-cookbook.html>.
    pub fn peaking_eq(sample_rate: f32, frequency: f32, db_gain: f32, q: f32) -> Self {
        nih_debug_assert!(sample_rate > 0.0);
        nih_debug_assert!(frequency > 0.0);
        nih_debug_assert!(frequency < sample_rate / 2.0);
        nih_debug_assert!(q > 0.0);

        let a = 10_f32.powf(db_gain / 40.0);
        let omega0 = consts::TAU * (frequency / sample_rate);
        let (sin_omega0, cos_omega0) = omega0.sin_cos();
        let alpha = sin_omega0 / (2.0 * q);

        // We'll prenormalize everything with a0
        let a0 = 1.0 + alpha / a;
        let b0 = (1.0 + alpha * a) / a0;
        let b1 = (-2.0 * cos_omega0) / a0;
        let b2 = (1.0 - alpha * a) / a0;
        let a1 = (-2.0 * cos_omega0) / a0;
        let a2 = (1.0 - alpha / a) / a0;

        Self::from_f32s(BiquadCoefficients { b0, b1, b2, a1, a2 })
    }
}

impl SimdType for f32 {
    #[inline(always)]
    fn from_f32(value: f32) -> Self {
        value
    }
}

impl SimdType for f32x2 {
    #[inline(always)]
    fn from_f32(value: f32) -> Self {
        Self::splat(value)
    }
}
