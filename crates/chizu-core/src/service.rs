//! 発電所と浄水場、電気と水道の供給の判定。
//!
//! 供給の判定は [`evaluate_supply`] の 1 つに閉じ込める。いまの判定は次のとおりである。
//! - 道路網でつながった範囲（連結成分）ごとに、施設の供給量と建物の需要を足す。
//! - 範囲の中の建物を、同じ種類の施設の入口から道路網の上で近い順に並べ、供給量の残りに
//!   収まるあいだ届ける。収まらない建物が出たら、そこから先の建物には届かない。
//!   道のりが同じなら建物の ID の小さい順にする。電気と水道は別々に配る。
//!
//! 道のりは、施設の入口を始点にした多始点の最短距離で求める。道路の向きは見ない。
//! 電気と水道で 1 回ずつ、区間の数を S、建物の数を B として O(S log S + B log B) で済む。
//!
//! 区画に新しい建物が建つのは、その範囲の電気と水道に余りがあるときだけである（[`Headroom`]）。
//! 施設のない範囲にも [`UNSUPPLIED_ALLOWANCE`] までは建つので、施設を置く前に家が数軒建つ。

use serde::{Deserialize, Serialize};
use std::cmp::Reverse;
use std::collections::{BTreeMap, BTreeSet, BinaryHeap};

use crate::building::{Building, Buildings};
use crate::command::ServiceKind;
use crate::id::BuildingId;
use crate::network::{Dir, LinkId, Network, SegmentId};
use crate::state_hash::{StateHash, StateHasher};
use crate::tile::{TilePos, TileRect};
use crate::units::Money;

impl ServiceKind {
    pub const ALL: [ServiceKind; 2] = [ServiceKind::PowerPlant, ServiceKind::WaterPump];

    /// `ALL` の中での位置。施設の種類ごとの表の添え字に使う。
    pub fn index(self) -> usize {
        self as usize - 1
    }

    /// 建設費。
    pub fn cost(self) -> Money {
        match self {
            ServiceKind::PowerPlant => Money(60_000),
            ServiceKind::WaterPump => Money(25_000),
        }
    }

    /// 1 日あたりの維持費。
    pub fn upkeep_per_day(self) -> Money {
        match self {
            ServiceKind::PowerPlant => Money(1_200),
            ServiceKind::WaterPump => Money(600),
        }
    }

    /// 間口と奥行き（マス）。
    pub fn size(self) -> (u64, u64) {
        match self {
            ServiceKind::PowerPlant => (4, 4),
            ServiceKind::WaterPump => (2, 2),
        }
    }

    /// 供給量。建物の需要と同じ単位である。
    pub fn capacity(self) -> u64 {
        match self {
            ServiceKind::PowerPlant => 3_600,
            ServiceKind::WaterPump => 3_000,
        }
    }
}

/// 建物 1 つの電気と水道の需要。占めるマスの数と段階に比例する。
pub fn building_demand(b: &Building) -> u64 {
    b.tile_count() * u64::from(b.level)
}

/// プレイヤーが置いた公共の施設。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Facility {
    pub kind: ServiceKind,
    pub area: TileRect,
    pub front: TilePos,
    pub toward_road: (i64, i64),
    /// 面した道路の歩道のリンク。道路が分かれると付け直し、道路がなくなると `None` になる。
    pub entrance: Option<LinkId>,
}

impl StateHash for Supply {
    fn write_state(&self, h: &mut StateHasher) {
        h.write(&self.power);
        h.write(&self.water);
    }
}

impl StateHash for Facility {
    fn write_state(&self, h: &mut StateHasher) {
        h.write(&(self.kind as u8));
        for t in [self.area.min, self.area.max, self.front] {
            h.write(&t.x);
            h.write(&t.y);
        }
        h.write(&self.toward_road);
        h.write(
            &self
                .entrance
                .map(|e| (e.segment, e.dir as u8, e.mode as u8)),
        );
    }
}

/// 建物 1 つに電気と水道が届いているか。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Supply {
    pub power: bool,
    pub water: bool,
}

/// 道路網の連結成分。ノードの添え字ごとに、成分の代表の添え字を持つ。
pub(crate) struct Components {
    parent: Vec<usize>,
}

impl Components {
    pub(crate) fn of(network: &Network) -> Components {
        let len = network
            .nodes()
            .map(|(id, _)| id.index as usize + 1)
            .max()
            .unwrap_or(0);
        let mut c = Components {
            parent: (0..len).collect(),
        };
        for (_, s) in network.segments() {
            let (a, b) = (c.find(s.a.index as usize), c.find(s.b.index as usize));
            // 小さい添え字を代表にすると、つなぐ順番によらず代表が決まる。
            let (lo, hi) = (a.min(b), a.max(b));
            c.parent[hi] = lo;
        }
        c
    }

    pub(crate) fn find(&mut self, mut i: usize) -> usize {
        while self.parent[i] != i {
            self.parent[i] = self.parent[self.parent[i]];
            i = self.parent[i];
        }
        i
    }

    /// リンクのある区間の成分。区間が消えていれば `None`。
    pub(crate) fn of_link(&mut self, network: &Network, link: LinkId) -> Option<usize> {
        let s = network.segment(link.segment)?;
        Some(self.find(s.a.index as usize))
    }
}

/// 連結成分 1 つの、電気か水道の供給量と需要。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Balance {
    pub capacity: u64,
    pub demand: u64,
}

impl Balance {
    /// この成分の建物すべてに届くか。偽なら、施設から遠い建物の一部か全部に届かない。
    pub fn enough(self) -> bool {
        self.capacity > 0 && self.capacity >= self.demand
    }
}

/// 道路網の連結成分ごとの、電気と水道の釣り合い。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SupplyBalance {
    /// 成分ごとの `[電気, 水道]`。成分の代表のノードの添え字の順に並ぶ。
    pub components: Vec<[Balance; 2]>,
    /// 区間ごとの、`components` の添え字。区間の ID の順に並ぶ。
    pub segments: Vec<(SegmentId, usize)>,
    /// 成分ごとの代表のマス。成分の中で需要が最も大きい建物の、道路に面したマスである
    /// （需要が同じなら ID の小さい建物）。建物のない成分は `None`。
    pub spots: Vec<Option<TilePos>>,
}

/// 成分ごとに施設の供給量と建物の需要を足したもの。
struct Tally {
    comps: Components,
    /// ノードの添え字ごとの `[電気, 水道]`。成分の代表の添え字のところにだけ値が入る。
    balance: Vec<[Balance; 2]>,
    placed: Vec<(BuildingId, Option<usize>)>,
}

fn tally(network: &Network, facilities: &[Facility], buildings: &Buildings) -> Tally {
    let mut comps = Components::of(network);
    let mut balance = vec![[Balance::default(); 2]; comps.parent.len()];
    for f in facilities {
        if let Some(c) = f.entrance.and_then(|e| comps.of_link(network, e)) {
            let slot = match f.kind {
                ServiceKind::PowerPlant => 0,
                ServiceKind::WaterPump => 1,
            };
            balance[c][slot].capacity += f.kind.capacity();
        }
    }
    let placed = buildings
        .iter()
        .map(|(id, b)| {
            let c = comps.of_link(network, b.entrance);
            if let Some(c) = c {
                let d = building_demand(b);
                balance[c][0].demand += d;
                balance[c][1].demand += d;
            }
            (id, c)
        })
        .collect();
    Tally {
        comps,
        balance,
        placed,
    }
}

/// 区間の上の点。区間の `a` の端からの道のり（mm）で表す。
#[derive(Clone, Copy)]
struct OnSegment {
    segment: SegmentId,
    from_a: u64,
}

/// 区間ごとの、両端のノードの添え字と長さ（mm）。
struct Roads {
    ends: BTreeMap<SegmentId, (usize, usize, u64)>,
    /// ノードの添え字ごとの、隣のノードの添え字と区間の長さ。
    adjacent: Vec<Vec<(usize, u64)>>,
}

impl Roads {
    fn of(network: &Network, nodes: usize) -> Roads {
        let mut ends = BTreeMap::new();
        let mut adjacent = vec![Vec::new(); nodes];
        for (id, s) in network.segments() {
            let line = network.segment_line(id).expect("区間の線がない");
            let len = u64::try_from(line.length().0).expect("区間の長さが負");
            let (a, b) = (s.a.index as usize, s.b.index as usize);
            ends.insert(id, (a, b, len));
            adjacent[a].push((b, len));
            adjacent[b].push((a, len));
        }
        Roads { ends, adjacent }
    }

    /// 始点の集まりから各ノードまでの最短の道のり。届かないノードは `None`。
    fn distances(&self, sources: &[OnSegment]) -> Vec<Option<u64>> {
        let mut dist: Vec<Option<u64>> = vec![None; self.adjacent.len()];
        let mut heap = BinaryHeap::new();
        fn reach(
            dist: &mut [Option<u64>],
            heap: &mut BinaryHeap<Reverse<(u64, usize)>>,
            n: usize,
            d: u64,
        ) {
            if dist[n].is_none_or(|old| d < old) {
                dist[n] = Some(d);
                heap.push(Reverse((d, n)));
            }
        }
        for p in sources {
            let Some(&(a, b, len)) = self.ends.get(&p.segment) else {
                continue;
            };
            reach(&mut dist, &mut heap, a, p.from_a);
            reach(&mut dist, &mut heap, b, len - p.from_a);
        }
        while let Some(Reverse((d, n))) = heap.pop() {
            if dist[n] != Some(d) {
                continue;
            }
            for &(m, len) in &self.adjacent[n] {
                let next = d.checked_add(len).expect("道のりがあふれた");
                reach(&mut dist, &mut heap, m, next);
            }
        }
        dist
    }

    /// 点 `p` までの最短の道のり。`dist` は [`Roads::distances`] の結果で、`same` は `p` と同じ区間にある始点である。
    fn distance_to(&self, dist: &[Option<u64>], same: &[u64], p: OnSegment) -> Option<u64> {
        let &(a, b, len) = self.ends.get(&p.segment)?;
        let via_a = dist[a].map(|d| d.checked_add(p.from_a).expect("道のりがあふれた"));
        let via_b = dist[b].map(|d| d.checked_add(len - p.from_a).expect("道のりがあふれた"));
        let direct = same.iter().map(|s| s.abs_diff(p.from_a)).min();
        [via_a, via_b, direct].into_iter().flatten().min()
    }
}

/// 建物の出入口の位置。
fn building_point(network: &Network, b: &Building) -> Option<OnSegment> {
    let len = u64::try_from(network.segment_line(b.entrance.segment)?.length().0).ok()?;
    let at = u64::from(b.entrance_at.0).min(len);
    let from_a = match b.entrance.dir {
        Dir::Forward => at,
        Dir::Backward => len - at,
    };
    Some(OnSegment {
        segment: b.entrance.segment,
        from_a,
    })
}

/// 施設の入口の位置。間口の端のマスの前の道路のマスにする。
fn facility_point(network: &Network, f: &Facility) -> Option<OnSegment> {
    let segment = f.entrance?.segment;
    let line = network.segment_line(segment)?;
    let road = TilePos::new(f.front.x + f.toward_road.0, f.front.y + f.toward_road.1);
    let len = u64::try_from(line.length().0).ok()?;
    // 施設の間口は縦か横の道路に面するので、ふつうは前のマスが区間の上にある。なければ区間の `a` の端とみなす。
    let from_a = line.position_of(road).map_or(0, |k| {
        (len / line.len).checked_mul(k).expect("道のりがあふれた")
    });
    Some(OnSegment { segment, from_a })
}

/// 建物ごとに、電気と水道が届いているかを決める。結果は建物の ID の順に並ぶ。
pub fn evaluate_supply(
    network: &Network,
    facilities: &[Facility],
    buildings: &Buildings,
) -> Vec<(BuildingId, Supply)> {
    let t = tally(network, facilities, buildings);
    let roads = Roads::of(network, t.balance.len());
    let mut out: Vec<(BuildingId, Supply)> = t
        .placed
        .iter()
        .map(|(id, _)| (*id, Supply::default()))
        .collect();
    for kind in ServiceKind::ALL {
        let slot = kind.index();
        let sources: Vec<OnSegment> = facilities
            .iter()
            .filter(|f| f.kind == kind)
            .filter_map(|f| facility_point(network, f))
            .collect();
        let mut on_segment: BTreeMap<SegmentId, Vec<u64>> = BTreeMap::new();
        for p in &sources {
            on_segment.entry(p.segment).or_default().push(p.from_a);
        }
        let dist = roads.distances(&sources);
        // 成分、道のり、建物の ID の順に並べる。`placed` は建物の ID の順なので、添え字の順は ID の順である。
        let mut order: Vec<(usize, u64, usize, u64)> = t
            .placed
            .iter()
            .enumerate()
            .filter_map(|(i, (id, c))| {
                let b = buildings.get(*id).expect("建物がない");
                let d = building_point(network, b)
                    .and_then(|p| {
                        let same = on_segment.get(&p.segment).map_or(&[][..], Vec::as_slice);
                        roads.distance_to(&dist, same, p)
                    })
                    .unwrap_or(u64::MAX);
                Some(((*c)?, d, i, building_demand(b)))
            })
            .collect();
        order.sort_unstable();
        let mut left: Vec<Option<u64>> = t.balance.iter().map(|b| Some(b[slot].capacity)).collect();
        for (c, _, i, demand) in order {
            match left[c] {
                Some(rest) if demand <= rest => {
                    left[c] = Some(rest - demand);
                    let s = &mut out[i].1;
                    match kind {
                        ServiceKind::PowerPlant => s.power = true,
                        ServiceKind::WaterPump => s.water = true,
                    }
                }
                // 収まらない建物が出たら、この成分のそれより遠い建物には配らない。
                _ => left[c] = None,
            }
        }
    }
    out
}

/// 供給の足りない網にも、施設を置く前に建ってよい建物の需要の合計。小さな家が数軒建つ程度にする。
pub const UNSUPPLIED_ALLOWANCE: u64 = 48;

/// 連結成分ごとの、新しい建物に回せる電気と水道の残り。区画に建物を建てるときに使う。
pub(crate) struct Headroom {
    comps: Components,
    /// ノードの添え字ごとの `[電気, 水道]` の残り。負なら足りない。成分の代表の添え字のところにだけ値が入る。
    left: Vec<[i64; 2]>,
}

impl Headroom {
    pub(crate) fn of(
        network: &Network,
        facilities: &[Facility],
        buildings: &Buildings,
    ) -> Headroom {
        let t = tally(network, facilities, buildings);
        let allowance = i64::try_from(UNSUPPLIED_ALLOWANCE).expect("余地があふれた");
        let left = t
            .balance
            .iter()
            .map(|b| {
                b.map(|s| {
                    let cap = i64::try_from(s.capacity).expect("供給量があふれた");
                    let need = i64::try_from(s.demand).expect("需要があふれた");
                    cap.checked_sub(need)
                        .and_then(|r| r.checked_add(allowance))
                        .expect("余地があふれた")
                })
            })
            .collect();
        Headroom {
            comps: t.comps,
            left,
        }
    }

    /// 出入口が `link` の網に建てられる建物の需要の上限。電気と水道の少ないほうで、余裕がなければ `None`。
    pub(crate) fn room(&mut self, network: &Network, link: LinkId) -> Option<u64> {
        let c = self.comps.of_link(network, link)?;
        u64::try_from(self.left[c].into_iter().min()?)
            .ok()
            .filter(|r| *r > 0)
    }

    /// 出入口が `link` の網に、需要 1 の建物を建てる余地が `[電気, 水道]` のそれぞれにないか。区間が消えていれば `None`。
    pub(crate) fn lacks(&mut self, network: &Network, link: LinkId) -> Option<[bool; 2]> {
        let c = self.comps.of_link(network, link)?;
        Some(self.left[c].map(|r| r < 1))
    }

    /// 出入口が `link` の、需要 `demand` の建物を建てる余地が電気にも水道にもあれば、その分を差し引いて真を返す。
    pub(crate) fn take(&mut self, network: &Network, link: LinkId, demand: u64) -> bool {
        let Some(c) = self.comps.of_link(network, link) else {
            return false;
        };
        let d = i64::try_from(demand).expect("需要があふれた");
        if self.left[c].iter().any(|r| *r < d) {
            return false;
        }
        for r in &mut self.left[c] {
            *r -= d;
        }
        true
    }
}

/// 道路網の連結成分ごとの供給量と需要。[`evaluate_supply`] と同じ足し方で、状態は変えない。
pub fn supply_balance(
    network: &Network,
    facilities: &[Facility],
    buildings: &Buildings,
) -> SupplyBalance {
    let mut t = tally(network, facilities, buildings);
    let roots: Vec<(SegmentId, usize)> = network
        .segments()
        .map(|(id, s)| (id, t.comps.find(s.a.index as usize)))
        .collect();
    let order: BTreeMap<usize, usize> = roots
        .iter()
        .map(|(_, r)| *r)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .enumerate()
        .map(|(i, r)| (r, i))
        .collect();
    let mut spots: Vec<Option<(u64, TilePos)>> = vec![None; order.len()];
    for (id, root) in &t.placed {
        let (Some(root), Some(b)) = (root, buildings.get(*id)) else {
            continue;
        };
        let Some(&slot) = order.get(root) else {
            continue;
        };
        let d = building_demand(b);
        // 建物は ID の順に来るので、需要が等しいときは先に来た小さい ID を残す。
        if spots[slot].is_none_or(|(best, _)| d > best) {
            spots[slot] = Some((d, b.front));
        }
    }
    SupplyBalance {
        spots: spots.into_iter().map(|s| s.map(|(_, p)| p)).collect(),
        components: order.keys().map(|r| t.balance[*r]).collect(),
        segments: roots.into_iter().map(|(id, r)| (id, order[&r])).collect(),
    }
}
