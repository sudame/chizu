//! テストとベンチマークに使う、種から決定論的に作る街。

use chizu_core::routing::RouteRequest;
use chizu_core::{
    LaneMode, Line, LinkId, Money, Network, Population, RngPurpose, RoadKind, Tick, TilePos, rng,
};

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

/// 格子状の道路網。`blocks` × `blocks` の街区で、1 街区の辺は `block_tiles` マスである。
///
/// `arterial_every` 本ごとに、東西を太い道路、南北を中くらいの道路にし、ほかは細い道路にする。
pub fn grid_network(blocks: i64, block_tiles: i64, arterial_every: i64) -> Network {
    let mut n = Network::new();
    let side = blocks * block_tiles;
    for i in 0..=blocks {
        let at = i * block_tiles;
        let arterial = i % arterial_every == 0;
        let roads = [
            (TilePos::new(0, at), TilePos::new(side, at), RoadKind::Large),
            (
                TilePos::new(at, 0),
                TilePos::new(at, side),
                RoadKind::Medium,
            ),
        ];
        for (from, to, kind) in roads {
            let kind = if arterial { kind } else { RoadKind::Small };
            let line = Line::between(from, to).expect("格子の道路が引けない");
            n.build_road(&line, kind).expect("格子の道路が重なった");
        }
    }
    n
}

/// 人口 10 万の街の規模の格子。48 × 48 の街区（1 辺 96m、街全体で約 4.6km 四方）である。
pub fn city_grid_100k() -> Network {
    grid_network(48, 12, 4)
}

/// `count` 人ぶんの、家から職場への車の通勤の依頼。
///
/// 家はすべての車のリンクから一様に選ぶ。職場は車のリンクの 1/10 に集まっているとし、そこから一様に選ぶ。
pub fn commute_requests(seed: u64, net: &Network, count: u32) -> Vec<RouteRequest> {
    let links: Vec<LinkId> = net.links().filter(|l| l.mode == LaneMode::Car).collect();
    let len = u32::try_from(links.len()).expect("リンクが多すぎる");
    let offices: Vec<LinkId> = links.iter().copied().step_by(10).collect();
    let office_len = u32::try_from(offices.len()).expect("リンクが多すぎる");
    (0..count)
        .map(|i| {
            let mut r = rng(seed, RngPurpose::JobAssignment, u64::from(i), Tick::ZERO);
            RouteRequest {
                from: links[r.below(len) as usize],
                to: offices[r.below(office_len) as usize],
                mode: LaneMode::Car,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use chizu_core::routing::{AstarRouter, Router};

    #[test]
    fn grid_is_connected_for_commutes() {
        let n = grid_network(6, 4, 3);
        assert_eq!(n.nodes().count(), 49);
        let reqs = commute_requests(1, &n, 500);
        assert_eq!(reqs, commute_requests(1, &n, 500));
        let routes = AstarRouter::new(&n).route_batch(&reqs);
        assert!(routes.iter().all(Option::is_some));
    }
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
    fn every_household_size_and_minors_appear() {
        let p = synthetic_population(9, 10_000);
        let mut sizes = [0_usize; MAX_HOUSEHOLD_SIZE as usize + 1];
        let mut minors = 0;
        for id in p.citizen_ids() {
            let row = p.citizen(id).unwrap();
            let members = p.members(row.household).unwrap();
            if members[0] == id {
                sizes[members.len()] += 1;
            } else if row.age < 18 {
                minors += 1;
            }
        }
        // 人数は 1〜4 人から一様に引くので、どの人数の世帯も全体の 1/8 より多い。
        let households = p.household_count();
        for size in 1..=MAX_HOUSEHOLD_SIZE as usize {
            assert!(sizes[size] * 8 > households, "{size} 人の世帯 {sizes:?}");
        }
        assert!(minors > 0);
    }

    #[test]
    fn golden_hash() {
        assert_eq!(
            state_hash_of(&synthetic_population(42, 1_000)),
            3_943_354_908_999_942_000
        );
    }
}
