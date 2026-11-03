//! 種を固定した乱数。
//!
//! 共有の生成器は持たない。引くたびに [`rng`] で「種・用途・対象の ID・tick」から
//! 生成器を作る。並列に回しても、引く順番で結果が変わらない。
//!
//! 種を混ぜるのは SplitMix64、引くのは PCG32（XSH RR）である。どちらも自前で書き、
//! 外部クレートの版が上がって乱数の列が変わることを避ける。ここを変えると
//! ゴールデンのハッシュがすべて変わる。

use crate::units::Tick;

/// 乱数を引く用途。用途ごとに別の列になる。
///
/// 値は種に混ぜる。既存の値を変えると乱数の列が変わるので、足すときは新しい値を使う。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
#[repr(u32)]
pub enum RngPurpose {
    /// 種からの地形（水辺など）の生成。
    Terrain = 1,
    /// 外からの移住。
    Migration = 2,
    /// 建物が建つ・育つ。
    BuildingGrowth = 3,
    /// 職場の割り当て。
    JobAssignment = 4,
    /// 1 日の予定。
    DailySchedule = 5,
    /// 外部接続の幹線の生成。
    TrunkLine = 6,
    /// 幸福度の低い世帯が街を出ていく。
    Emigration = 7,
    /// 買い物の外出。
    Errand = 8,
    /// 世帯が車を持つか。
    CarOwnership = 9,
    /// 費用の同じ経路から 1 本を選ぶ順。
    RouteVariant = 10,
}

pub(crate) const GOLDEN_GAMMA: u64 = 0x9E37_79B9_7F4A_7C15;

/// SplitMix64 の出力関数。入力が 1 ビット違えば出力は大きく変わる。
pub(crate) fn mix64(z: u64) -> u64 {
    let z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    let z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// SplitMix64。種を混ぜるためだけに使う。
#[derive(Clone, Debug)]
struct SplitMix64 {
    state: u64,
}

impl SplitMix64 {
    fn next_u64(&mut self) -> u64 {
        self.state = self.state.wrapping_add(GOLDEN_GAMMA);
        mix64(self.state)
    }
}

/// PCG32（XSH RR、64 ビットの状態で 32 ビットを出す）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pcg32 {
    state: u64,
    inc: u64,
}

const PCG_MULTIPLIER: u64 = 6_364_136_223_846_793_005;

impl Pcg32 {
    /// PCG の参照実装の `pcg32_srandom_r` と同じ初期化。
    pub fn new(init_state: u64, init_seq: u64) -> Pcg32 {
        let mut rng = Pcg32 {
            state: 0,
            inc: (init_seq << 1) | 1,
        };
        rng.step();
        rng.state = rng.state.wrapping_add(init_state);
        rng.step();
        rng
    }

    fn step(&mut self) {
        self.state = self
            .state
            .wrapping_mul(PCG_MULTIPLIER)
            .wrapping_add(self.inc);
    }

    pub fn next_u32(&mut self) -> u32 {
        let old = self.state;
        self.step();
        let xorshifted = (((old >> 18) ^ old) >> 27) as u32;
        let rot = (old >> 59) as u32;
        xorshifted.rotate_right(rot)
    }

    /// 2 回引いて、先に引いた値を上位に置く。
    pub fn next_u64(&mut self) -> u64 {
        let hi = u64::from(self.next_u32());
        let lo = u64::from(self.next_u32());
        (hi << 32) | lo
    }

    /// `0..n` から偏りなく 1 つ選ぶ。`n` が 0 なら panic する。
    #[track_caller]
    pub fn below(&mut self, n: u32) -> u32 {
        assert!(n > 0, "Pcg32::below に 0 を渡した");
        // 2^32 を n で割った余りの分だけ下の値を捨てると、残りは n の倍数個になる。
        let threshold = n.wrapping_neg() % n;
        loop {
            let r = self.next_u32();
            if r >= threshold {
                return r % n;
            }
        }
    }

    /// `numerator / denominator` の確率で `true` を返す。
    #[track_caller]
    pub fn chance(&mut self, numerator: u32, denominator: u32) -> bool {
        assert!(
            numerator <= denominator,
            "Pcg32::chance の確率が 1 を超えている: {numerator} / {denominator}"
        );
        self.below(denominator) < numerator
    }
}

/// 種・用途と語の列を 1 つの値に混ぜる。生成器を作らずに、座標などから値を 1 つだけ引くときに使う。
pub fn mix_words(seed: u64, purpose: RngPurpose, words: &[u64]) -> u64 {
    let mut h = mix64(seed.wrapping_add(GOLDEN_GAMMA));
    for word in core::iter::once(purpose as u64).chain(words.iter().copied()) {
        h = mix64(h ^ mix64(word.wrapping_add(GOLDEN_GAMMA)));
    }
    h
}

/// その場で生成器を作る。同じ引数からは必ず同じ列が出る。
pub fn rng(seed: u64, purpose: RngPurpose, id: u64, tick: Tick) -> Pcg32 {
    let h = mix_words(seed, purpose, &[id, tick.0]);
    let mut sm = SplitMix64 { state: h };
    let init_state = sm.next_u64();
    let init_seq = sm.next_u64();
    Pcg32::new(init_state, init_seq)
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn splitmix64_matches_reference() {
        // Vigna の splitmix64.c に種 1234567 を与えたときの最初の 5 個。
        let mut sm = SplitMix64 { state: 1_234_567 };
        let got: Vec<u64> = (0..5).map(|_| sm.next_u64()).collect();
        assert_eq!(
            got,
            [
                6_457_827_717_110_365_317,
                3_203_168_211_198_807_973,
                9_817_491_932_198_370_423,
                4_593_380_528_125_082_431,
                16_408_922_859_458_223_821,
            ]
        );
    }

    #[test]
    fn pcg32_matches_reference() {
        // PCG の参照実装 pcg32-demo の `pcg32_srandom_r(&rng, 42, 54)` の最初の 6 個。
        let mut rng = Pcg32::new(42, 54);
        let got: Vec<u32> = (0..6).map(|_| rng.next_u32()).collect();
        assert_eq!(
            got,
            [
                0xa15c02b7, 0x7b47f409, 0xba1d3330, 0x83d2f293, 0xbfa4784b, 0xcbed606e
            ]
        );
    }

    #[test]
    fn rng_golden_sequence() {
        let mut r = rng(42, RngPurpose::Migration, 7, Tick(100));
        let got: Vec<u32> = (0..8).map(|_| r.next_u32()).collect();
        assert_eq!(
            got,
            [
                2_667_852_214,
                1_351_138_978,
                3_720_368_655,
                2_976_134_454,
                3_695_876_162,
                742_252_499,
                873_560_861,
                2_345_466_334,
            ]
        );
    }

    #[test]
    fn rng_golden_below() {
        let mut r = rng(0, RngPurpose::Terrain, 0, Tick::ZERO);
        let got: Vec<u32> = (0..8).map(|_| r.below(10)).collect();
        assert_eq!(got, [9, 4, 4, 9, 8, 4, 9, 6]);
    }

    #[test]
    fn rng_inputs_each_change_the_sequence() {
        let base = rng(1, RngPurpose::Migration, 2, Tick(3));
        let variants = [
            rng(0, RngPurpose::Migration, 2, Tick(3)),
            rng(1, RngPurpose::Terrain, 2, Tick(3)),
            rng(1, RngPurpose::Migration, 3, Tick(3)),
            rng(1, RngPurpose::Migration, 2, Tick(4)),
            // 引数の位置を入れ替えても同じ列にならない。
            rng(1, RngPurpose::Migration, 3, Tick(2)),
        ];
        for v in variants {
            assert_ne!(v, base);
        }
    }

    #[test]
    fn below_one_is_always_zero() {
        let mut r = rng(9, RngPurpose::DailySchedule, 9, Tick(9));
        for _ in 0..100 {
            assert_eq!(r.below(1), 0);
        }
    }

    #[test]
    #[should_panic(expected = "0 を渡した")]
    fn below_zero_panics() {
        rng(0, RngPurpose::Terrain, 0, Tick::ZERO).below(0);
    }

    proptest! {
        #[test]
        fn rng_is_reproducible(seed in any::<u64>(), id in any::<u64>(), tick in any::<u64>()) {
            let mut a = rng(seed, RngPurpose::JobAssignment, id, Tick(tick));
            let mut b = rng(seed, RngPurpose::JobAssignment, id, Tick(tick));
            for _ in 0..16 {
                prop_assert_eq!(a.next_u32(), b.next_u32());
            }
        }

        #[test]
        fn below_stays_in_range(seed in any::<u64>(), n in 1_u32..) {
            let mut r = rng(seed, RngPurpose::BuildingGrowth, 0, Tick::ZERO);
            for _ in 0..16 {
                prop_assert!(r.below(n) < n);
            }
        }

        #[test]
        fn chance_extremes(seed in any::<u64>(), d in 1_u32..) {
            let mut r = rng(seed, RngPurpose::BuildingGrowth, 1, Tick::ZERO);
            prop_assert!(!r.chance(0, d));
            prop_assert!(r.chance(d, d));
        }
    }
}
