//! Sample-format conversion for devices that only offer integer formats (never allocates).

/// Integer samples convertible to and from `f32` in `[-1, 1)`.
pub(crate) trait IntSample: Copy {
    /// To float.
    fn to_f32(self) -> f32;
    /// From float, clamped to the representable range.
    fn from_f32(x: f32) -> Self;
}

impl IntSample for i16 {
    fn to_f32(self) -> f32 {
        f32::from(self) / 32_768.0
    }
    fn from_f32(x: f32) -> Self {
        if x.is_nan() {
            return 0;
        }
        (x * 32_768.0).round().clamp(-32_768.0, 32_767.0) as i16
    }
}

impl IntSample for i32 {
    fn to_f32(self) -> f32 {
        (f64::from(self) / 2_147_483_648.0) as f32
    }
    fn from_f32(x: f32) -> Self {
        if x.is_nan() {
            return 0;
        }
        (f64::from(x) * 2_147_483_648.0)
            .round()
            .clamp(f64::from(i32::MIN), f64::from(i32::MAX)) as i32
    }
}

/// Converts `src` into `dst` (up to the shorter length).
pub(crate) fn to_f32<T: IntSample>(src: &[T], dst: &mut [f32]) {
    for (d, s) in dst.iter_mut().zip(src) {
        *d = s.to_f32();
    }
}

/// Converts `src` into `dst` (up to the shorter length), clamping.
pub(crate) fn from_f32<T: IntSample>(src: &[f32], dst: &mut [T]) {
    for (d, s) in dst.iter_mut().zip(src) {
        *d = T::from_f32(*s);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn i16_conversion_maps_full_scale_and_clamps() {
        assert!((0i16.to_f32()).abs() < f32::EPSILON);
        assert!((i16::MIN.to_f32() + 1.0).abs() < f32::EPSILON);
        assert!((16384i16.to_f32() - 0.5).abs() < f32::EPSILON);
        assert_eq!(i16::from_f32(0.5), 16384);
        assert_eq!(i16::from_f32(-1.0), i16::MIN);
        assert_eq!(i16::from_f32(1.0), i16::MAX);
        assert_eq!(i16::from_f32(7.0), i16::MAX);
        assert_eq!(i16::from_f32(-7.0), i16::MIN);
        assert_eq!(i16::from_f32(f32::NAN), 0);
    }

    #[test]
    fn i32_conversion_maps_full_scale_and_clamps() {
        assert!((i32::MIN.to_f32() + 1.0).abs() < f32::EPSILON);
        assert!(((1i32 << 30).to_f32() - 0.5).abs() < f32::EPSILON);
        assert_eq!(i32::from_f32(0.5), 1 << 30);
        assert_eq!(i32::from_f32(-1.0), i32::MIN);
        assert_eq!(i32::from_f32(2.0), i32::MAX);
        assert_eq!(i32::from_f32(f32::NAN), 0);
    }

    #[test]
    fn integer_round_trip_is_exact_for_i16() {
        let src: Vec<i16> = (i16::MIN..=i16::MAX).step_by(7).collect();
        let mut f = vec![0.0; src.len()];
        let mut back = vec![0i16; src.len()];
        to_f32(&src, &mut f);
        from_f32(&f, &mut back);
        assert_eq!(src, back);
    }

    #[test]
    fn conversion_is_allocation_free() {
        let src = [0.25f32; 64];
        let mut a = [0i16; 64];
        let mut b = [0i32; 64];
        let mut c = [0f32; 64];
        atomik_testkit::no_alloc::assert_no_alloc(|| {
            from_f32(&src, &mut a);
            from_f32(&src, &mut b);
            to_f32(&a, &mut c);
        });
        assert!((c[0] - 0.25).abs() < 1e-4);
    }
}
