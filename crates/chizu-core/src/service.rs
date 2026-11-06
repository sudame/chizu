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
    /// 施設の一覧（`World::facilities`）に入る種類。駅（[`ServiceKind::Station`]）は線路の網に持つので入れない。
    /// 施設の種類ごとの表は、この長さで作り、[`ServiceKind::index`] で引く。
    pub const ALL: [ServiceKind; 4] = [
        ServiceKind::PowerPlant,
        ServiceKind::WaterPump,
        ServiceKind::ParkingLot,
        ServiceKind::ParkingGarage,
    ];

    /// 電気と水道を供給する施設。並びは [`ServiceKind::utility_slot`] の順である。
    pub const UTILITIES: [ServiceKind; 2] = [ServiceKind::PowerPlant, ServiceKind::WaterPump];

    /// 共同の駐車場。
    pub const PARKING: [ServiceKind; 2] = [ServiceKind::ParkingLot, ServiceKind::ParkingGarage];

    /// `ALL` の中での位置。施設の種類ごとの表の添え字に使う。駅は `ALL` に入らないので、`ALL.len()` になる。
    pub fn index(self) -> usize {
        self as usize - 1
    }

    /// 電気なら 0、水道なら 1。共同の駐車場は電気も水道も供給しないので `None`。
    pub fn utility_slot(self) -> Option<usize> {
        match self {
            ServiceKind::PowerPlant => Some(0),
            ServiceKind::WaterPump => Some(1),
            ServiceKind::ParkingLot | ServiceKind::ParkingGarage | ServiceKind::Station => None,
        }
    }

    /// 共同の駐車場か。
    pub fn is_parking(self) -> bool {
        self.parking_spaces() > 0
    }

    /// 建設費。
    pub fn cost(self) -> Money {
        match self {
            ServiceKind::PowerPlant => Money(60_000),
            ServiceKind::WaterPump => Money(25_000),
            ServiceKind::ParkingLot => Money(3_000),
            ServiceKind::ParkingGarage => Money(72_000),
            ServiceKind::Station => Money(25_000),
        }
    }

    /// 1 日あたりの維持費。
    pub fn upkeep_per_day(self) -> Money {
        match self {
            ServiceKind::PowerPlant => Money(1_200),
            ServiceKind::WaterPump => Money(600),
            ServiceKind::ParkingLot => Money(96),
            ServiceKind::ParkingGarage => Money(2_400),
            ServiceKind::Station => Money(400),
        }
    }

    /// 間口と奥行き（マス）。
    pub fn size(self) -> (u64, u64) {
        match self {
            ServiceKind::PowerPlant | ServiceKind::ParkingGarage => (4, 4),
            ServiceKind::WaterPump | ServiceKind::ParkingLot => (2, 2),
            ServiceKind::Station => (crate::rail::STATION_FRONTAGE, crate::rail::STATION_DEPTH),
        }
    }

    /// 電気か水道の供給量。建物の需要と同じ単位である。共同の駐車場は 0。
    pub fn capacity(self) -> u64 {
        match self {
            ServiceKind::PowerPlant => 3_600,
            ServiceKind::WaterPump => 3_000,
            ServiceKind::ParkingLot | ServiceKind::ParkingGarage | ServiceKind::Station => 0,
        }
    }

    /// 共同の駐車場の台数。平面は 1 マスに 3 台、駐車場ビルはそれを 5 層に積む。ほかの施設は 0。
    pub fn parking_spaces(self) -> u32 {
        match self {
            ServiceKind::PowerPlant | ServiceKind::WaterPump | ServiceKind::Station => 0,
            ServiceKind::ParkingLot => 12,
            ServiceKind::ParkingGarage => 240,
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

    /// 余りが負か、供給量の 1 割未満か。需要があって供給量が 0 のときも真。画面の前もっての知らせと同じ基準である。
    pub fn is_tight(self) -> bool {
        self.capacity <= self.demand && self.demand > 0
            || self.capacity > self.demand && (self.capacity - self.demand) * 10 < self.capacity
    }
}

/// 余りが住民の何人分にあたるか。読み取り用で、状態には入れない。
///
/// 住民 1 人あたりの需要は、街の需要の合計（`total_demand`。住宅だけでなく、商業・工業・オフィスの職場の分も含む）を
/// 家に住んでいる人の数（`citizens`。家のない人は需要を生まないので数えない）で割ったものとする。人数は `headroom × citizens ÷ total_demand` を整数で求め、小数は切り捨てる
/// （余りが負のときは、足りない人数を多めに数える向きに丸める）。
/// 余りが負なら、足りない人数が負の値で返る。
/// 人口が 0、または需要の合計が 0 で 1 人あたりの需要が決まらないときは `None`。
pub fn headroom_people(headroom: i128, total_demand: u64, citizens: u64) -> Option<i128> {
    if citizens == 0 || total_demand == 0 {
        return None;
    }
    Some((headroom * i128::from(citizens)).div_euclid(i128::from(total_demand)))
}

/// 施設 1 基で増える住民の数。読み取り用で、状態には入れない。
///
/// 施設 1 基の供給量（[`ServiceKind::capacity`]）を、[`headroom_people`] と同じ 1 人あたりの需要で割る。
/// 式は `capacity × citizens ÷ total_demand` で、小数は切り捨てる。人口か需要の合計が 0 のときは `None`。
pub fn facility_people(kind: ServiceKind, total_demand: u64, citizens: u64) -> Option<u64> {
    let people = headroom_people(i128::from(kind.capacity()), total_demand, citizens)?;
    Some(u64::try_from(people).expect("供給量は正なので人数は負にならない"))
}

/// 余りが、このペースでどう動くか。量の単位は呼び出し側が決める（供給量と同じ単位でも、人数でもよい）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HeadroomTrend {
    /// すでに足りない（余りが負）。
    Short,
    /// 需要が増えていない（減ったか、同じ）。`per_day` は 1 日あたりの余りの増え（0 以上）。
    NotShrinking { per_day: i128 },
    /// 需要が増えて、余りが減っている。`drop_per_day` は 1 日あたりの余りの減り（正）、
    /// `ticks_until_out` は同じ伸びが続くとして、余りが 0 を割るまでの tick の数。
    Shrinking {
        drop_per_day: i128,
        ticks_until_out: u64,
    },
}

/// 減り方を求めるのに要る最短の間隔（tick）。ゲーム内の 1 時間である。画面が見積もりを出し始める最短の控えにも使う。
pub const MIN_TREND_TICKS: u64 = crate::clock::TICKS_PER_DAY / 24;

/// 需要の伸びから、余りの先を見積もる。読み取り用で、状態には入れない。
///
/// `headroom` はいまの余り（供給量 − 需要）、`demand_growth_per_day` は 1 日前の同じ時刻からの需要の伸びである。
/// 同じ時刻どうしで比べるので、朝の建設の波を 1 日の伸びと取り違えない。供給量の変化は伸びに入らないので、施設を足した直後でも見積もれる。
/// 余りが負なら [`HeadroomTrend::Short`]、伸びが 0 以下なら [`HeadroomTrend::NotShrinking`]、
/// 正なら尽きるまでを `headroom × TICKS_PER_DAY ÷ 伸び`（tick、小数は切り捨て）で求める。どれも整数で求める。
pub fn demand_trend(headroom: i128, demand_growth_per_day: i128) -> HeadroomTrend {
    if headroom < 0 {
        return HeadroomTrend::Short;
    }
    if demand_growth_per_day <= 0 {
        return HeadroomTrend::NotShrinking {
            per_day: -demand_growth_per_day,
        };
    }
    let day = i128::from(crate::clock::TICKS_PER_DAY);
    let ticks = headroom * day / demand_growth_per_day;
    HeadroomTrend::Shrinking {
        drop_per_day: demand_growth_per_day,
        ticks_until_out: u64::try_from(ticks).unwrap_or(u64::MAX),
    }
}

/// 2 つの控え `(tick, 需要)` のあいだの `at` の需要を、直線でつないで求める。`lo` からの増減の小数は 0 向きに切り捨てる。
/// `lo.0 <= at <= hi.0` でないとき、`lo.0 >= hi.0` で間が決まらないときは、`lo.0 == hi.0 == at` を除いて `None`。
pub fn interpolate_demand(lo: (u64, u64), hi: (u64, u64), at: u64) -> Option<u64> {
    if at < lo.0 || at > hi.0 {
        return None;
    }
    if lo.0 == hi.0 {
        return (lo.1 == hi.1).then_some(lo.1);
    }
    let span = i128::from(hi.0 - lo.0);
    let off = i128::from(at - lo.0);
    let v = i128::from(lo.1) + (i128::from(hi.1) - i128::from(lo.1)) * off / span;
    u64::try_from(v).ok()
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

impl SupplyBalance {
    /// `slot`（電気が 0、水道が 1）の主の網の `components` の添え字。
    /// 需要がいちばん大きい網で、同じなら添え字の小さいほう。どの網にも需要がなければ `None`。
    pub fn main_network(&self, slot: usize) -> Option<usize> {
        let mut best: Option<(u64, usize)> = None;
        for (i, c) in self.components.iter().enumerate() {
            let d = c[slot].demand;
            if d > 0 && best.is_none_or(|(b, _)| d > b) {
                best = Some((d, i));
            }
        }
        best.map(|(_, i)| i)
    }

    /// 成分 `component` を見分ける印。成分の中でいちばん小さい区間の ID を、0 にならない 1 つの数にしたもの。
    /// 読み取り用で、状態には入れない。道路を足しても、いちばん小さい区間が消えなければ変わらない。
    /// 主の網が別の網に替わったかを見分けるのに使う。区間のない成分は `None`。
    pub fn network_mark(&self, component: usize) -> Option<u64> {
        self.segments
            .iter()
            .filter(|(_, c)| *c == component)
            .map(|(id, _)| id)
            .min()
            .map(|id| ((u64::from(id.index) + 1) << 16) | u64::from(id.generation))
    }
}

/// 街の道路網につながっていない施設（定義は [`isolated_facilities`]）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct IsolatedFacility {
    pub kind: ServiceKind,
    /// 施設の道路に面したマス。
    pub front: TilePos,
}

/// 孤立した施設を、施設の並びの順に返す。状態は変えない。
///
/// 次のどちらかに当たる施設である。
/// - 入口がない（面した道路が消えた）。
/// - 主の網ではない網にあり、その網の余りが正で、主の網の余りが少ないか足りない
///   （余りが供給量の 1 割未満か負、供給量 0 で需要があるときも足りない）。
///
/// 主の網に十分な余りがあるあいだは、ほかの網に施設があっても数えない。別の町を作る遊び方で、
/// 誤った知らせを出さないためである。切れた道路に区画を塗って家が建っても、主の網が足りないあいだは数える。
pub fn isolated_facilities(
    network: &Network,
    facilities: &[Facility],
    buildings: &Buildings,
) -> Vec<IsolatedFacility> {
    let balance = supply_balance(network, facilities, buildings);
    let of_segment: BTreeMap<SegmentId, usize> = balance.segments.iter().copied().collect();
    facilities
        .iter()
        .filter(|f| {
            // 共同の駐車場は電気も水道も供給しないので、網につながっていなくても知らせない。
            let Some(slot) = f.kind.utility_slot() else {
                return false;
            };
            let Some(&c) = f.entrance.and_then(|e| of_segment.get(&e.segment)) else {
                return true;
            };
            let Some(main) = balance.main_network(slot) else {
                return false;
            };
            let (here, top) = (balance.components[c][slot], balance.components[main][slot]);
            c != main && here.capacity > here.demand && top.is_tight()
        })
        .map(|f| IsolatedFacility {
            kind: f.kind,
            front: f.front,
        })
        .collect()
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
        if let Some(c) = f.entrance.and_then(|e| comps.of_link(network, e))
            && let Some(slot) = f.kind.utility_slot()
        {
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
    for (slot, kind) in ServiceKind::UTILITIES.into_iter().enumerate() {
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
                    if slot == 0 {
                        s.power = true;
                    } else {
                        s.water = true;
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

    /// 出入口が `link` の網の識別子。同じ網なら同じ値で、網の中の最小のノードの添え字である。区間が消えていれば `None`。
    pub(crate) fn network_of(&mut self, network: &Network, link: LinkId) -> Option<usize> {
        self.comps.of_link(network, link)
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

#[cfg(test)]
mod tests {
    use super::{
        HeadroomTrend, demand_trend, facility_people, headroom_people, interpolate_demand,
    };
    use crate::clock::TICKS_PER_DAY;
    use crate::command::ServiceKind;

    #[test]
    fn one_facility_is_its_capacity_divided_by_demand_per_citizen() {
        // 需要の合計 2,300 を 1,000 人で使う街（1 人あたり 2.3）。発電所 3,600 は 1,565 人分、浄水場はその分だけ。
        assert_eq!(
            facility_people(ServiceKind::PowerPlant, 2300, 1000),
            Some(3600 * 1000 / 2300)
        );
        assert_eq!(
            facility_people(ServiceKind::WaterPump, 2300, 1000),
            Some(ServiceKind::WaterPump.capacity() * 1000 / 2300)
        );
        // 余りの人数と同じ式で、供給量を余りとして渡した値に等しい
        let cap = i128::from(ServiceKind::PowerPlant.capacity());
        assert_eq!(
            facility_people(ServiceKind::PowerPlant, 2300, 1000).map(i128::from),
            headroom_people(cap, 2300, 1000)
        );
        assert_eq!(facility_people(ServiceKind::PowerPlant, 2300, 0), None);
        assert_eq!(facility_people(ServiceKind::PowerPlant, 0, 1000), None);
    }

    #[test]
    fn growing_demand_gives_time_until_out() {
        // 余り 1,000 で、需要が 1 日に 500 伸びる。あと 2 日で尽きる。
        assert_eq!(
            demand_trend(1000, 500),
            HeadroomTrend::Shrinking {
                drop_per_day: 500,
                ticks_until_out: 2 * TICKS_PER_DAY,
            }
        );
        // 余りが同じでも、伸びが倍なら尽きるのは半分の時間
        assert_eq!(
            demand_trend(1000, 1000),
            HeadroomTrend::Shrinking {
                drop_per_day: 1000,
                ticks_until_out: TICKS_PER_DAY,
            }
        );
        // 余りが 0 ちょうどで伸びていれば、すぐ尽きる
        assert_eq!(
            demand_trend(0, 10),
            HeadroomTrend::Shrinking {
                drop_per_day: 10,
                ticks_until_out: 0,
            }
        );
    }

    #[test]
    fn flat_or_falling_demand_does_not_run_out() {
        assert_eq!(
            demand_trend(1000, 0),
            HeadroomTrend::NotShrinking { per_day: 0 }
        );
        assert_eq!(
            demand_trend(1000, -300),
            HeadroomTrend::NotShrinking { per_day: 300 }
        );
    }

    #[test]
    fn a_shortage_is_short_whatever_the_growth() {
        assert_eq!(demand_trend(-1, 500), HeadroomTrend::Short);
        assert_eq!(demand_trend(-1, -500), HeadroomTrend::Short);
    }

    #[test]
    fn the_estimate_does_not_depend_on_how_much_supply_was_added() {
        // 施設を足すと余りは増えるが、需要の伸びは変わらない。尽きるまでは余りに比例して延びるだけ。
        let before = demand_trend(300, 600);
        let after = demand_trend(300 + 3600, 600);
        let (
            HeadroomTrend::Shrinking {
                drop_per_day: a, ..
            },
            HeadroomTrend::Shrinking {
                drop_per_day: b, ..
            },
        ) = (before, after)
        else {
            panic!("どちらも減っている");
        };
        assert_eq!(a, b);
    }

    #[test]
    fn demand_is_interpolated_between_two_samples() {
        assert_eq!(
            interpolate_demand((100, 1000), (300, 1200), 200),
            Some(1100)
        );
        // 端はその値、切り捨ては 0 向き
        assert_eq!(
            interpolate_demand((100, 1000), (300, 1200), 100),
            Some(1000)
        );
        assert_eq!(
            interpolate_demand((100, 1000), (400, 1001), 200),
            Some(1000)
        );
        assert_eq!(interpolate_demand((100, 1000), (400, 998), 200), Some(1000));
        assert_eq!(interpolate_demand((100, 1000), (400, 995), 200), Some(999));
        // 減る向きでもよい
        assert_eq!(interpolate_demand((0, 100), (10, 0), 5), Some(50));
        // 外は求めない。同じ tick は、値が同じときだけ
        assert_eq!(interpolate_demand((100, 1000), (300, 1200), 99), None);
        assert_eq!(interpolate_demand((100, 1000), (300, 1200), 301), None);
        assert_eq!(
            interpolate_demand((100, 1000), (100, 1000), 100),
            Some(1000)
        );
        assert_eq!(interpolate_demand((100, 1000), (100, 1001), 100), None);
    }

    #[test]
    fn headroom_is_divided_by_demand_per_citizen() {
        // 需要の合計 2,300 を 1,000 人で使う街（1 人あたり 2.3）。余り 920 は 400 人分。
        assert_eq!(headroom_people(920, 2300, 1000), Some(400));
    }

    #[test]
    fn fractions_round_down() {
        // 920 / 2.3 = 400。余り 919 は 399.56 人なので 399 人。
        assert_eq!(headroom_people(919, 2300, 1000), Some(399));
        assert_eq!(headroom_people(0, 2300, 1000), Some(0));
    }

    #[test]
    fn a_shortage_is_a_negative_number_of_people() {
        assert_eq!(headroom_people(-230, 2300, 1000), Some(-100));
        // 足りない人数は多めに数える
        assert_eq!(headroom_people(-1, 2300, 1000), Some(-1));
    }

    #[test]
    fn unknown_without_people_or_demand() {
        assert_eq!(headroom_people(500, 2300, 0), None);
        assert_eq!(headroom_people(500, 0, 1000), None);
        assert_eq!(headroom_people(500, 0, 0), None);
    }
}
