//! テストとベンチマークに使う、種から決定論的に作る街。

use chizu_core::{Money, Population, RngPurpose, Tick, rng};

/// 1 世帯の住民の数の上限。
pub const MAX_HOUSEHOLD_SIZE: u32 = 4;

/// 種 `seed` から `count` 人の合成の住民と世帯を作る。
///
/// 世帯は 1〜[`MAX_HOUSEHOLD_SIZE`] 人で、最初の 1 人は 18〜79 歳、残りは 0〜79 歳である。
/// 最後の世帯は、合計がちょうど `count` 人になるように人数を削る。
/// 住む建物と職場はまだない。
pub fn synthetic_population(seed: u64, count: u32) -> Population {
    let mut p = Population::default();
    let mut made = 0_u32;
    let mut household_no = 0_u64;
    while made < count {
        let mut r = rng(seed, RngPurpose::Migration, household_no, Tick::ZERO);
        household_no += 1;
        let size = (1 + r.below(MAX_HOUSEHOLD_SIZE)).min(count - made);
        let money = Money(i64::from(r.below(100_000)));
        let h = p.add_household(money);
        for i in 0..size {
            let age = if i == 0 {
                18 + r.below(62)
            } else {
                r.below(80)
            };
            p.add_citizen(h, age as u8)
                .expect("足したばかりの世帯がない");
        }
        made += size;
    }
    p
}

#[cfg(test)]
mod tests {
    use super::*;
    use chizu_core::state_hash_of;

    #[test]
    fn makes_exactly_the_requested_number_of_citizens() {
        for n in [0, 1, 5, 1_000] {
            let p = synthetic_population(3, n);
            assert_eq!(p.citizen_count(), n as usize);
            assert!(p.household_count() <= n as usize);
        }
    }

    #[test]
    fn is_deterministic_and_depends_on_the_seed() {
        let a = state_hash_of(&synthetic_population(1, 10_000));
        assert_eq!(a, state_hash_of(&synthetic_population(1, 10_000)));
        assert_ne!(a, state_hash_of(&synthetic_population(2, 10_000)));
    }

    #[test]
    fn households_and_ages_are_in_range() {
        let p = synthetic_population(9, 10_000);
        let ids: Vec<_> = p.citizen_ids().collect();
        let mut heads = 0;
        for id in &ids {
            let row = p.citizen(*id).unwrap();
            let members = p.members(row.household).unwrap();
            assert!((1..=MAX_HOUSEHOLD_SIZE as usize).contains(&members.len()));
            assert!(row.age < 80);
            if members[0] == *id {
                heads += 1;
                assert!(row.age >= 18);
            }
        }
        assert_eq!(heads, p.household_count());
    }

    #[test]
    fn golden_hash() {
        assert_eq!(
            state_hash_of(&synthetic_population(42, 1_000)),
            3_943_354_908_999_942_000
        );
    }
}
