#![allow(unsafe_op_in_unsafe_fn)]

use super::*;

pub struct SumWindow<'a, T, S> {
    slice: &'a [T],
    validity: &'a Bitmap,
    sum: Option<S>,
    err: S,
    last_start: usize,
    last_end: usize,
    pub(super) null_count: usize,
}

impl<T, S> SumWindow<'_, T, S>
where
    T: NativeType + IsFloat + Sub<Output = T> + NumCast,
    S: NativeType + AddAssign + SubAssign + Sub<Output = S> + Add<Output = S> + NumCast,
{
    // Kahan summation
    fn add(&mut self, val: T) {
        if T::is_float() && val.is_finite() {
            self.sum = self.sum.map(|sum| {
                let val: S = NumCast::from(val).unwrap();
                let y = val - self.err;
                let new_sum = sum + y;
                self.err = (new_sum - sum) - y;
                new_sum
            });
        } else {
            let val: S = NumCast::from(val).unwrap();
            self.sum = self.sum.map(|v| v + val)
        }
    }

    fn sub(&mut self, val: T) {
        if T::is_float() && val.is_finite() {
            self.add(T::zeroed() - val)
        } else {
            let val: S = NumCast::from(val).unwrap();
            self.sum = self.sum.map(|v| v - val)
        }
    }
}

impl<T, S> SumWindow<'_, T, S>
where
    T: NativeType + IsFloat + Sub<Output = T> + NumCast,
    S: NativeType + AddAssign + SubAssign + Sub<Output = S> + Add<Output = S> + NumCast,
{
    // compute sum from the entire window
    unsafe fn compute_sum_and_null_count(&mut self, start: usize, end: usize) -> Option<S> {
        let mut sum = None;
        let mut idx = start;
        self.null_count = 0;
        for value in &self.slice[start..end] {
            let value: S = NumCast::from(*value).unwrap();
            let valid = self.validity.get_bit_unchecked(idx);
            if valid {
                match sum {
                    None => sum = Some(value),
                    Some(current) => sum = Some(value + current),
                }
            } else {
                self.null_count += 1;
            }
            idx += 1;
        }
        self.sum = sum;
        sum
    }
}

impl<'a, T, S> RollingAggWindowNulls<'a, T> for SumWindow<'a, T, S>
where
    T: NativeType + IsFloat + Sub<Output = T> + NumCast,
    S: NativeType + AddAssign + SubAssign + Sub<Output = S> + Add<Output = S> + NumCast,
{
    unsafe fn new(
        slice: &'a [T],
        validity: &'a Bitmap,
        start: usize,
        end: usize,
        _params: Option<RollingFnParams>,
        _window_size: Option<usize>,
    ) -> Self {
        let mut out = Self {
            slice,
            validity,
            sum: None,
            err: S::zeroed(),
            last_start: start,
            last_end: end,
            null_count: 0,
        };
        out.compute_sum_and_null_count(start, end);
        out
    }

    unsafe fn update(&mut self, start: usize, end: usize) -> Option<T> {
        // if we exceed the end, we have a completely new window
        // so we recompute
        let recompute_sum = if start >= self.last_end {
            true
        } else {
            // remove elements that should leave the window
            let mut recompute_sum = false;
            for idx in self.last_start..start {
                // SAFETY:
                // we are in bounds
                let valid = self.validity.get_bit_unchecked(idx);
                if valid {
                    let leaving_value = self.slice.get_unchecked(idx);

                    // if the leaving value is nan we need to recompute the window
                    if T::is_float() && !leaving_value.is_finite() {
                        recompute_sum = true;
                        break;
                    }
                    self.sub(*leaving_value);
                } else {
                    // null value leaving the window
                    self.null_count -= 1;

                    // self.sum is None and the leaving value is None
                    // if the entering value is valid, we might get a new sum.
                    if self.sum.is_none() {
                        recompute_sum = true;
                        break;
                    }
                }
            }
            recompute_sum
        };

        self.last_start = start;

        // we traverse all values and compute
        if recompute_sum {
            self.compute_sum_and_null_count(start, end);
        } else {
            for idx in self.last_end..end {
                let valid = self.validity.get_bit_unchecked(idx);

                if valid {
                    let value = *self.slice.get_unchecked(idx);
                    match self.sum {
                        None => self.sum = NumCast::from(value),
                        _ => self.add(value),
                    }
                } else {
                    // null value entering the window
                    self.null_count += 1;
                }
            }
        }
        self.last_end = end;
        self.sum.and_then(NumCast::from).or(Some(T::zeroed()))
    }

    fn is_valid(&self, min_periods: usize) -> bool {
        ((self.last_end - self.last_start) - self.null_count) >= min_periods
    }
}

pub fn rolling_sum<T>(
    arr: &PrimitiveArray<T>,
    window_size: usize,
    min_periods: usize,
    center: bool,
    weights: Option<&[f64]>,
    _params: Option<RollingFnParams>,
) -> ArrayRef
where
    T: NativeType
        + IsFloat
        + PartialOrd
        + Add<Output = T>
        + Sub<Output = T>
        + SubAssign
        + AddAssign
        + NumCast,
{
    if weights.is_some() {
        panic!("weights not yet supported on array with null values")
    }
    if center {
        rolling_apply_agg_window::<SumWindow<T, T>, _, _>(
            arr.values().as_slice(),
            arr.validity().as_ref().unwrap(),
            window_size,
            min_periods,
            det_offsets_center,
            None,
        )
    } else {
        rolling_apply_agg_window::<SumWindow<T, T>, _, _>(
            arr.values().as_slice(),
            arr.validity().as_ref().unwrap(),
            window_size,
            min_periods,
            det_offsets,
            None,
        )
    }
}
