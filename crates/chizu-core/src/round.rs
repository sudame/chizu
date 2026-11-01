//! 丸め方を名前に入れた整数の割り算。
//!
//! 中核では、符号つきの整数の割り算と剰余はこのモジュールの関数だけで行う。
//! 符号つきの `/` と `%` は 0 に向かって切り捨てるが、それを丸め方として選んだことが
//! 読み手に伝わらないためである。符号なしの整数の `/` と `%` は、切り捨てと床が一致して
//! 曖昧さがないので、そのまま使ってよい。
//!
//! `checked_` で始まる関数は、0 で割ったときと結果が `i64` に収まらないとき
//! （`i64::MIN / -1`）に `None` を返す。始まらない関数は、同じ場合に panic する。

/// 0 に向かって切り捨てる。
pub fn checked_div_trunc(a: i64, d: i64) -> Option<i64> {
    a.checked_div(d)
}

/// 負の無限大に向かって切り捨てる。
pub fn checked_div_floor(a: i64, d: i64) -> Option<i64> {
    let q = a.checked_div(d)?;
    let r = a % d;
    if r != 0 && ((r < 0) != (d < 0)) {
        Some(q - 1)
    } else {
        Some(q)
    }
}

/// 正の無限大に向かって切り上げる。
pub fn checked_div_ceil(a: i64, d: i64) -> Option<i64> {
    let q = a.checked_div(d)?;
    let r = a % d;
    if r != 0 && ((r < 0) == (d < 0)) {
        Some(q + 1)
    } else {
        Some(q)
    }
}

/// いちばん近い整数に丸める。ちょうど半分のときは偶数のほうに寄せる（銀行家の丸め）。
pub fn checked_div_round_half_even(a: i64, d: i64) -> Option<i64> {
    let q = a.checked_div(d)?;
    let r = a % d;
    if r == 0 {
        return Some(q);
    }
    // |r| < |d| <= 2^63 なので、2|r| は u64 に収まる。
    let twice_r = r.unsigned_abs() * 2;
    let abs_d = d.unsigned_abs();
    let away_from_zero = match twice_r.cmp(&abs_d) {
        core::cmp::Ordering::Less => false,
        core::cmp::Ordering::Greater => true,
        core::cmp::Ordering::Equal => q % 2 != 0,
    };
    // 余りがあるとき |d| >= 2 なので |q| <= 2^62 で、q ± 1 はあふれない。
    if !away_from_zero {
        Some(q)
    } else if (a < 0) != (d < 0) {
        Some(q - 1)
    } else {
        Some(q + 1)
    }
}

/// [`checked_div_trunc`] の panic する版。
pub fn div_trunc(a: i64, d: i64) -> i64 {
    checked_div_trunc(a, d).unwrap_or_else(|| div_failed(a, d))
}

/// [`checked_div_floor`] の panic する版。
pub fn div_floor(a: i64, d: i64) -> i64 {
    checked_div_floor(a, d).unwrap_or_else(|| div_failed(a, d))
}

/// [`checked_div_ceil`] の panic する版。
pub fn div_ceil(a: i64, d: i64) -> i64 {
    checked_div_ceil(a, d).unwrap_or_else(|| div_failed(a, d))
}

/// [`checked_div_round_half_even`] の panic する版。
pub fn div_round_half_even(a: i64, d: i64) -> i64 {
    checked_div_round_half_even(a, d).unwrap_or_else(|| div_failed(a, d))
}

#[cold]
fn div_failed(a: i64, d: i64) -> ! {
    panic!("割り算ができない: {a} / {d}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn rounding_table() {
        // (a, d, trunc, floor, ceil, half_even)
        let table: &[(i64, i64, i64, i64, i64, i64)] = &[
            (0, 3, 0, 0, 0, 0),
            (6, 3, 2, 2, 2, 2),
            (7, 2, 3, 3, 4, 4),
            (5, 2, 2, 2, 3, 2),
            (3, 2, 1, 1, 2, 2),
            (1, 2, 0, 0, 1, 0),
            (-1, 2, 0, -1, 0, 0),
            (-3, 2, -1, -2, -1, -2),
            (-5, 2, -2, -3, -2, -2),
            (5, -2, -2, -3, -2, -2),
            (-5, -2, 2, 2, 3, 2),
            (7, 3, 2, 2, 3, 2),
            (8, 3, 2, 2, 3, 3),
            (-7, 3, -2, -3, -2, -2),
            (-8, 3, -2, -3, -2, -3),
            (10, 4, 2, 2, 3, 2),
            (14, 4, 3, 3, 4, 4),
            (i64::MAX, 1, i64::MAX, i64::MAX, i64::MAX, i64::MAX),
            (i64::MIN, 1, i64::MIN, i64::MIN, i64::MIN, i64::MIN),
            (
                i64::MIN,
                2,
                i64::MIN / 2,
                i64::MIN / 2,
                i64::MIN / 2,
                i64::MIN / 2,
            ),
            (
                i64::MAX,
                2,
                i64::MAX / 2,
                i64::MAX / 2,
                i64::MAX / 2 + 1,
                i64::MAX / 2 + 1,
            ),
            (i64::MIN, i64::MAX, -1, -2, -1, -1),
            (i64::MAX, i64::MIN, 0, -1, 0, -1),
        ];
        for &(a, d, trunc, floor, ceil, half_even) in table {
            assert_eq!(div_trunc(a, d), trunc, "trunc {a} / {d}");
            assert_eq!(div_floor(a, d), floor, "floor {a} / {d}");
            assert_eq!(div_ceil(a, d), ceil, "ceil {a} / {d}");
            assert_eq!(div_round_half_even(a, d), half_even, "half_even {a} / {d}");
        }
    }

    #[test]
    fn division_failures_return_none() {
        for a in [0, 1, -1, i64::MAX, i64::MIN] {
            assert_eq!(checked_div_trunc(a, 0), None);
            assert_eq!(checked_div_floor(a, 0), None);
            assert_eq!(checked_div_ceil(a, 0), None);
            assert_eq!(checked_div_round_half_even(a, 0), None);
        }
        assert_eq!(checked_div_trunc(i64::MIN, -1), None);
        assert_eq!(checked_div_floor(i64::MIN, -1), None);
        assert_eq!(checked_div_ceil(i64::MIN, -1), None);
        assert_eq!(checked_div_round_half_even(i64::MIN, -1), None);
    }

    #[test]
    #[should_panic(expected = "割り算ができない")]
    fn division_by_zero_panics() {
        div_round_half_even(1, 0);
    }

    fn divisor() -> impl Strategy<Value = i64> {
        any::<i64>().prop_filter("0 では割らない", |d| *d != 0)
    }

    fn operands() -> impl Strategy<Value = (i64, i64)> {
        (any::<i64>(), divisor()).prop_filter("i64::MIN / -1 は収まらない", |(a, d)| {
            !(*a == i64::MIN && *d == -1)
        })
    }

    proptest! {
        #[test]
        fn floor_and_ceil_bracket_the_quotient((a, d) in operands()) {
            let (a128, d128) = (i128::from(a), i128::from(d));
            let floor = i128::from(div_floor(a, d));
            let ceil = i128::from(div_ceil(a, d));
            prop_assert!(ceil - floor <= 1);
            if d > 0 {
                prop_assert!(floor * d128 <= a128 && a128 < (floor + 1) * d128);
            } else {
                prop_assert!(floor * d128 >= a128 && a128 > (floor + 1) * d128);
            }
            prop_assert_eq!(ceil == floor, a128 % d128 == 0);
        }

        #[test]
        fn half_even_is_nearest((a, d) in operands()) {
            let q = div_round_half_even(a, d);
            let err = (i128::from(a) - i128::from(q) * i128::from(d)).abs() * 2;
            let abs_d = i128::from(d).abs();
            prop_assert!(err <= abs_d);
            if err == abs_d {
                prop_assert_eq!(q % 2, 0);
            }
        }

        #[test]
        fn half_even_is_symmetric(a in -(i64::MAX)..=i64::MAX, d in divisor()) {
            prop_assert_eq!(div_round_half_even(-a, d), -div_round_half_even(a, d));
        }

        #[test]
        fn exact_division_agrees(q in -(1_i64 << 31)..(1_i64 << 31), d in -(1_i64 << 31)..(1_i64 << 31)) {
            prop_assume!(d != 0);
            let a = q * d;
            prop_assert_eq!(div_trunc(a, d), q);
            prop_assert_eq!(div_floor(a, d), q);
            prop_assert_eq!(div_ceil(a, d), q);
            prop_assert_eq!(div_round_half_even(a, d), q);
        }
    }
}
