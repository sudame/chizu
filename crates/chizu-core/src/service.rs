//! 発電所と浄水場、電気と水道の供給の判定。
//!
//! 供給の判定は [`evaluate_supply`] の 1 つに閉じ込める。いまの判定は次のとおりである。
//! - 道路網でつながった範囲（連結成分）ごとに、施設の供給量と建物の需要を足す。
//! - 範囲の中の施設の供給量の合計が、建物の需要の合計以上なら、その範囲の建物すべてに届く。
//!   足りなければ、その範囲のどの建物にも届かない。
//!
//! 届く範囲の決め方は要件で未定なので、ここを差し替えれば判定が変わるようにしている。

use std::collections::{BTreeMap, BTreeSet};

use crate::building::{Building, Buildings};
use crate::command::ServiceKind;
use crate::id::BuildingId;
use crate::network::{LinkId, Network, SegmentId};
use crate::state_hash::{StateHash, StateHasher};
use crate::tile::{TilePos, TileRect};
use crate::units::Money;

impl ServiceKind {
    /// 建設費。
    pub fn cost(self) -> Money {
        match self {
            ServiceKind::PowerPlant => Money(60_000),
            ServiceKind::WaterPump => Money(25_000),
        }
    }

    /// 1 日あたりの維持費。財政を入れる PR で所持金から引く。
    pub fn upkeep_per_day(self) -> Money {
        match self {
            ServiceKind::PowerPlant => Money(1_800),
            ServiceKind::WaterPump => Money(700),
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
            ServiceKind::PowerPlant => 400,
            ServiceKind::WaterPump => 300,
        }
    }
}

/// 建物 1 つの電気と水道の需要。占めるマスの数と段階に比例する。
pub fn building_demand(b: &Building) -> u64 {
    b.tile_count() * u64::from(b.level)
}

/// プレイヤーが置いた公共の施設。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
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
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
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
    /// この成分の建物に届くか。
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

/// 建物ごとに、電気と水道が届いているかを決める。結果は建物の ID の順に並ぶ。
pub fn evaluate_supply(
    network: &Network,
    facilities: &[Facility],
    buildings: &Buildings,
) -> Vec<(BuildingId, Supply)> {
    let t = tally(network, facilities, buildings);
    t.placed
        .into_iter()
        .map(|(id, c)| {
            let ok = |slot: usize| c.is_some_and(|c| t.balance[c][slot].enough());
            (
                id,
                Supply {
                    power: ok(0),
                    water: ok(1),
                },
            )
        })
        .collect()
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
    SupplyBalance {
        components: order.keys().map(|r| t.balance[*r]).collect(),
        segments: roots.into_iter().map(|(id, r)| (id, order[&r])).collect(),
    }
}
