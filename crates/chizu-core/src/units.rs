//! 単位ごとの整数の型。
//!
//! 演算子（`+`、`-` など）は、結果が型に収まらないと panic する。リリースビルドでも同じで、
//! 黙って折り返したり端の値に張りついたりはしない。あふれうる場面では `checked_` の
//! メソッドを使い、`None` を呼び出し側で扱う。

use core::ops::{Add, AddAssign, Mul, Neg, Sub, SubAssign};

use crate::round;

#[cold]
#[track_caller]
fn overflow(what: &str) -> ! {
    panic!("{what} があふれた")
}

/// 長さ（mm）。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub struct Mm(pub i64);

impl Mm {
    pub const ZERO: Mm = Mm(0);

    /// m で表した長さから作る。
    #[track_caller]
    pub fn from_m(m: i64) -> Mm {
        m.checked_mul(1000)
            .map(Mm)
            .unwrap_or_else(|| overflow("Mm"))
    }

    pub fn checked_add(self, rhs: Mm) -> Option<Mm> {
        self.0.checked_add(rhs.0).map(Mm)
    }

    pub fn checked_sub(self, rhs: Mm) -> Option<Mm> {
        self.0.checked_sub(rhs.0).map(Mm)
    }

    pub fn checked_neg(self) -> Option<Mm> {
        self.0.checked_neg().map(Mm)
    }

    pub fn checked_mul(self, rhs: i64) -> Option<Mm> {
        self.0.checked_mul(rhs).map(Mm)
    }

    pub fn checked_abs(self) -> Option<Mm> {
        self.0.checked_abs().map(Mm)
    }

    #[track_caller]
    pub fn abs(self) -> Mm {
        self.checked_abs().unwrap_or_else(|| overflow("Mm"))
    }

    #[track_caller]
    pub fn div_trunc(self, d: i64) -> Mm {
        Mm(round::div_trunc(self.0, d))
    }

    #[track_caller]
    pub fn div_floor(self, d: i64) -> Mm {
        Mm(round::div_floor(self.0, d))
    }

    #[track_caller]
    pub fn div_ceil(self, d: i64) -> Mm {
        Mm(round::div_ceil(self.0, d))
    }

    #[track_caller]
    pub fn div_round_half_even(self, d: i64) -> Mm {
        Mm(round::div_round_half_even(self.0, d))
    }
}

impl Add for Mm {
    type Output = Mm;
    #[track_caller]
    fn add(self, rhs: Mm) -> Mm {
        self.checked_add(rhs).unwrap_or_else(|| overflow("Mm"))
    }
}

impl Sub for Mm {
    type Output = Mm;
    #[track_caller]
    fn sub(self, rhs: Mm) -> Mm {
        self.checked_sub(rhs).unwrap_or_else(|| overflow("Mm"))
    }
}

impl Neg for Mm {
    type Output = Mm;
    #[track_caller]
    fn neg(self) -> Mm {
        self.checked_neg().unwrap_or_else(|| overflow("Mm"))
    }
}

impl Mul<i64> for Mm {
    type Output = Mm;
    #[track_caller]
    fn mul(self, rhs: i64) -> Mm {
        self.checked_mul(rhs).unwrap_or_else(|| overflow("Mm"))
    }
}

impl AddAssign for Mm {
    #[track_caller]
    fn add_assign(&mut self, rhs: Mm) {
        *self = *self + rhs;
    }
}

impl SubAssign for Mm {
    #[track_caller]
    fn sub_assign(&mut self, rhs: Mm) {
        *self = *self - rhs;
    }
}

/// ワールド座標。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub struct WorldPos {
    pub x: Mm,
    pub y: Mm,
}

impl WorldPos {
    pub const ORIGIN: WorldPos = WorldPos {
        x: Mm::ZERO,
        y: Mm::ZERO,
    };

    pub fn new(x: Mm, y: Mm) -> WorldPos {
        WorldPos { x, y }
    }

    pub fn checked_offset(self, dx: Mm, dy: Mm) -> Option<WorldPos> {
        Some(WorldPos {
            x: self.x.checked_add(dx)?,
            y: self.y.checked_add(dy)?,
        })
    }

    /// x と y をそれぞれずらした位置。
    #[track_caller]
    pub fn offset(self, dx: Mm, dy: Mm) -> WorldPos {
        self.checked_offset(dx, dy)
            .unwrap_or_else(|| overflow("WorldPos"))
    }

    /// `self` から `to` までの x と y の差。
    #[track_caller]
    pub fn delta_to(self, to: WorldPos) -> (Mm, Mm) {
        (to.x - self.x, to.y - self.y)
    }
}

/// シミュレーションの時刻（tick の通し番号）。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub struct Tick(pub u64);

impl Tick {
    pub const ZERO: Tick = Tick(0);

    pub fn checked_add(self, ticks: u64) -> Option<Tick> {
        self.0.checked_add(ticks).map(Tick)
    }

    /// 次の tick。
    #[track_caller]
    pub fn next(self) -> Tick {
        self + 1
    }

    /// `earlier` から `self` までの tick 数。`earlier` が後ならば `None`。
    pub fn checked_since(self, earlier: Tick) -> Option<u64> {
        self.0.checked_sub(earlier.0)
    }

    #[track_caller]
    pub fn since(self, earlier: Tick) -> u64 {
        self.checked_since(earlier)
            .unwrap_or_else(|| overflow("Tick"))
    }
}

impl Add<u64> for Tick {
    type Output = Tick;
    #[track_caller]
    fn add(self, ticks: u64) -> Tick {
        self.checked_add(ticks).unwrap_or_else(|| overflow("Tick"))
    }
}

impl AddAssign<u64> for Tick {
    #[track_caller]
    fn add_assign(&mut self, ticks: u64) {
        *self = *self + ticks;
    }
}

/// 速さ（1 tick に進む mm）。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub struct MmPerTick(pub i32);

impl MmPerTick {
    pub const ZERO: MmPerTick = MmPerTick(0);

    pub fn checked_add(self, rhs: MmPerTick) -> Option<MmPerTick> {
        self.0.checked_add(rhs.0).map(MmPerTick)
    }

    pub fn checked_sub(self, rhs: MmPerTick) -> Option<MmPerTick> {
        self.0.checked_sub(rhs.0).map(MmPerTick)
    }

    pub fn checked_neg(self) -> Option<MmPerTick> {
        self.0.checked_neg().map(MmPerTick)
    }

    /// `ticks` のあいだにこの速さで進む距離。
    pub fn checked_distance(self, ticks: u64) -> Option<Mm> {
        let ticks = i64::try_from(ticks).ok()?;
        i64::from(self.0).checked_mul(ticks).map(Mm)
    }

    #[track_caller]
    pub fn distance(self, ticks: u64) -> Mm {
        self.checked_distance(ticks)
            .unwrap_or_else(|| overflow("Mm"))
    }

    #[track_caller]
    pub fn div_round_half_even(self, d: i32) -> MmPerTick {
        let q = round::div_round_half_even(i64::from(self.0), i64::from(d));
        // |d| >= 1 なので |q| <= |self| + 1 で、収まらないのは i32::MIN / -1 だけである。
        i32::try_from(q)
            .map(MmPerTick)
            .unwrap_or_else(|_| overflow("MmPerTick"))
    }
}

impl Add for MmPerTick {
    type Output = MmPerTick;
    #[track_caller]
    fn add(self, rhs: MmPerTick) -> MmPerTick {
        self.checked_add(rhs)
            .unwrap_or_else(|| overflow("MmPerTick"))
    }
}

impl Sub for MmPerTick {
    type Output = MmPerTick;
    #[track_caller]
    fn sub(self, rhs: MmPerTick) -> MmPerTick {
        self.checked_sub(rhs)
            .unwrap_or_else(|| overflow("MmPerTick"))
    }
}

impl Neg for MmPerTick {
    type Output = MmPerTick;
    #[track_caller]
    fn neg(self) -> MmPerTick {
        self.checked_neg().unwrap_or_else(|| overflow("MmPerTick"))
    }
}

/// 車線の始点からの距離（mm）。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub struct LaneOffset(pub u32);

impl LaneOffset {
    pub const START: LaneOffset = LaneOffset(0);

    pub fn to_mm(self) -> Mm {
        Mm(i64::from(self.0))
    }

    /// 車線に沿って `by` だけ進めた位置。負の `by` は戻る向きである。
    /// 始点より手前や、`u32` で表せない先になるときは `None`。
    pub fn checked_advance(self, by: Mm) -> Option<LaneOffset> {
        let to = i64::from(self.0).checked_add(by.0)?;
        u32::try_from(to).ok().map(LaneOffset)
    }

    #[track_caller]
    pub fn advance(self, by: Mm) -> LaneOffset {
        self.checked_advance(by)
            .unwrap_or_else(|| overflow("LaneOffset"))
    }

    /// `self` から `to` までの距離。`to` が手前なら負になる。
    pub fn distance_to(self, to: LaneOffset) -> Mm {
        // どちらも u32 なので差は i64 に必ず収まる。
        Mm(i64::from(to.0) - i64::from(self.0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn mm_from_m() {
        assert_eq!(Mm::from_m(5), Mm(5_000));
        assert_eq!(Mm::from_m(-2), Mm(-2_000));
    }

    #[test]
    #[should_panic(expected = "Mm があふれた")]
    fn mm_add_overflow_panics() {
        let _ = Mm(i64::MAX) + Mm(1);
    }

    #[test]
    #[should_panic(expected = "Mm があふれた")]
    fn mm_neg_overflow_panics() {
        let _ = -Mm(i64::MIN);
    }

    #[test]
    #[should_panic(expected = "Tick があふれた")]
    fn tick_since_later_panics() {
        let _ = Tick(3).since(Tick(4));
    }

    #[test]
    fn tick_arithmetic() {
        assert_eq!(Tick(7).next(), Tick(8));
        assert_eq!(Tick(10).since(Tick(3)), 7);
        assert_eq!(Tick(u64::MAX).checked_add(1), None);
        let mut t = Tick::ZERO;
        t += 7_200;
        assert_eq!(t, Tick(7_200));
    }

    #[test]
    fn speed_distance() {
        // 時速 50km はおよそ 13,889 mm/tick（1 tick = 移動の 1 秒）。
        let speed = MmPerTick(13_889);
        assert_eq!(speed.distance(357), Mm(4_958_373));
        assert_eq!(MmPerTick(-3).distance(4), Mm(-12));
        assert_eq!(MmPerTick(1).checked_distance(u64::MAX), None);
        assert_eq!(
            MmPerTick(i32::MIN).checked_distance(1),
            Some(Mm(i64::from(i32::MIN)))
        );
    }

    #[test]
    fn speed_division() {
        assert_eq!(MmPerTick(5).div_round_half_even(2), MmPerTick(2));
        assert_eq!(MmPerTick(-7).div_round_half_even(2), MmPerTick(-4));
        assert_eq!(
            MmPerTick(i32::MIN).div_round_half_even(1),
            MmPerTick(i32::MIN)
        );
    }

    #[test]
    #[should_panic(expected = "MmPerTick があふれた")]
    fn speed_division_overflow_panics() {
        let _ = MmPerTick(i32::MIN).div_round_half_even(-1);
    }

    #[test]
    fn lane_offset_bounds() {
        assert_eq!(LaneOffset(10).checked_advance(Mm(-10)), Some(LaneOffset(0)));
        assert_eq!(LaneOffset(10).checked_advance(Mm(-11)), None);
        assert_eq!(LaneOffset(u32::MAX).checked_advance(Mm(1)), None);
        assert_eq!(
            LaneOffset(0).distance_to(LaneOffset(u32::MAX)),
            Mm(i64::from(u32::MAX))
        );
        assert_eq!(
            LaneOffset(u32::MAX).distance_to(LaneOffset(0)),
            Mm(-i64::from(u32::MAX))
        );
    }

    #[test]
    fn world_pos_offset() {
        let p = WorldPos::new(Mm(1), Mm(-2)).offset(Mm(10), Mm(20));
        assert_eq!(p, WorldPos::new(Mm(11), Mm(18)));
        assert_eq!(WorldPos::ORIGIN.delta_to(p), (Mm(11), Mm(18)));
        assert_eq!(
            WorldPos::new(Mm(i64::MAX), Mm(0)).checked_offset(Mm(1), Mm(0)),
            None
        );
    }

    proptest! {
        #[test]
        fn mm_checked_ops_match_i128(a in any::<i64>(), b in any::<i64>()) {
            let fits = |v: i128| i64::try_from(v).ok().map(Mm);
            prop_assert_eq!(Mm(a).checked_add(Mm(b)), fits(i128::from(a) + i128::from(b)));
            prop_assert_eq!(Mm(a).checked_sub(Mm(b)), fits(i128::from(a) - i128::from(b)));
            prop_assert_eq!(Mm(a).checked_mul(b), fits(i128::from(a) * i128::from(b)));
        }

        #[test]
        fn mm_add_then_sub_roundtrips(a in -(1_i64 << 62)..(1_i64 << 62), b in -(1_i64 << 62)..(1_i64 << 62)) {
            prop_assert_eq!(Mm(a) + Mm(b) - Mm(b), Mm(a));
        }

        #[test]
        fn lane_advance_matches_distance(from in any::<u32>(), to in any::<u32>()) {
            let (from, to) = (LaneOffset(from), LaneOffset(to));
            prop_assert_eq!(from.advance(from.distance_to(to)), to);
        }

        #[test]
        fn lane_advance_stays_in_range(from in any::<u32>(), by in any::<i64>()) {
            let expected = i128::from(from) + i128::from(by);
            let in_range = (0..=i128::from(u32::MAX)).contains(&expected);
            prop_assert_eq!(LaneOffset(from).checked_advance(Mm(by)).is_some(), in_range);
        }

        #[test]
        fn world_pos_delta_inverts_offset(
            x in -(1_i64 << 61)..(1_i64 << 61), y in -(1_i64 << 61)..(1_i64 << 61),
            dx in -(1_i64 << 61)..(1_i64 << 61), dy in -(1_i64 << 61)..(1_i64 << 61),
        ) {
            let p = WorldPos::new(Mm(x), Mm(y));
            prop_assert_eq!(p.delta_to(p.offset(Mm(dx), Mm(dy))), (Mm(dx), Mm(dy)));
        }
    }
}
