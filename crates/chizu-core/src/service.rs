//! 発電所と浄水場、電気と水道の供給の判定。
//!
//! 供給の判定は [`evaluate_supply`] の 1 つに閉じ込める。いまの判定は次のとおりである。
//! - 道路網でつながった範囲（連結成分）ごとに、施設の供給量と建物の需要を足す。
//! - 範囲の中の施設の供給量の合計が、建物の需要の合計以上なら、その範囲の建物すべてに届く。
//!   足りなければ、その範囲のどの建物にも届かない。
//!
//! 届く範囲の決め方は要件で未定なので、ここを差し替えれば判定が変わるようにしている。

use crate::building::{Building, Buildings};
use crate::command::ServiceKind;
use crate::id::BuildingId;
use crate::network::{LinkId, Network};
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

/// 建物ごとに、電気と水道が届いているかを決める。結果は建物の ID の順に並ぶ。
pub fn evaluate_supply(
    network: &Network,
    facilities: &[Facility],
    buildings: &Buildings,
) -> Vec<(BuildingId, Supply)> {
    let mut comps = Components::of(network);
    let n = comps.parent.len();
    // 成分ごとの、電気と水道の供給量と需要。
    let mut capacity = vec![[0_u64; 2]; n];
    let mut demand = vec![[0_u64; 2]; n];
    for f in facilities {
        if let Some(c) = f.entrance.and_then(|e| comps.of_link(network, e)) {
            let slot = match f.kind {
                ServiceKind::PowerPlant => 0,
                ServiceKind::WaterPump => 1,
            };
            capacity[c][slot] += f.kind.capacity();
        }
    }
    let placed: Vec<(BuildingId, Option<usize>)> = buildings
        .iter()
        .map(|(id, b)| {
            let c = comps.of_link(network, b.entrance);
            if let Some(c) = c {
                let d = building_demand(b);
                demand[c][0] += d;
                demand[c][1] += d;
            }
            (id, c)
        })
        .collect();
    placed
        .into_iter()
        .map(|(id, c)| {
            let ok = |slot: usize| {
                c.is_some_and(|c| capacity[c][slot] > 0 && capacity[c][slot] >= demand[c][slot])
            };
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
