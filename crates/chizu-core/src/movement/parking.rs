//! 建物の駐車場と、建物から建物への「歩く → 車 → 歩く」の移動。
//!
//! 移動は 3 つの区切りに分ける。出発の建物の出入口から、その建物の駐車場まで歩く。車で行き先の
//! 駐車場まで走る。車を停めて、行き先の出入口まで歩く。駐車場と出入口は、どちらも建物の前の道路の
//! 位置に置く。そのため、自分の建物の駐車場までの歩きは長さ 0 で、1 tick で終わる。
//!
//! 行き先の駐車場は出発のときに決めて、空きを 1 台ぶん押さえる。満車なら、行き先から
//! [`PARK_RADIUS`] マスの中で空きのある、いちばん近い建物の駐車場にする。どこも満車なら車では行けない。
//! 停めてある台数と押さえた台数は別に数える。建物から車が出るときは停めてある台数だけを減らすので、
//! まだ着いていない車のために押さえた空きを、出る車が使ってしまうことはない。
//! 建物が消えると、その駐車場に停めてあった車も消える。そこへ向かっていた車は、着いたら停めずに消える。

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

use super::{Ends, SidewalkPos, Trip, Trips, car, lane_shift, make_trip, plan};
use crate::building::{Building, Buildings};
use crate::id::{BuildingId, TripId};
use crate::network::{LaneMode, Network};
use crate::routing::AstarRouter;
use crate::state_hash::{StateHash, StateHasher};
use crate::tile::{TilePos, TileRect};
use crate::units::Tick;

/// 満車のときに、ほかの駐車場を探す範囲（マス）。
pub const PARK_RADIUS: u64 = 16;

/// いまの区切りが終わったあとにすること。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(super) enum Then {
    /// 移動を終える。
    Arrive,
    /// 車に乗って `park_at` まで走り、駐車場 `lot` に停めて `to` まで歩く。
    Drive {
        park_at: SidewalkPos,
        lot: BuildingId,
        to: SidewalkPos,
    },
    /// 車を駐車場 `lot` に停めて、`to` まで歩く。
    Walk { lot: BuildingId, to: SidewalkPos },
}

impl Then {
    /// 押さえている駐車場。
    pub(super) fn lot(&self) -> Option<BuildingId> {
        match self {
            Then::Arrive => None,
            Then::Drive { lot, .. } | Then::Walk { lot, .. } => Some(*lot),
        }
    }
}

impl StateHash for Then {
    fn write_state(&self, h: &mut StateHasher) {
        match self {
            Then::Arrive => h.write(&0_u8),
            Then::Drive { park_at, lot, to } => {
                h.write(&1_u8);
                h.write(park_at);
                h.write(lot);
                h.write(to);
            }
            Then::Walk { lot, to } => {
                h.write(&2_u8);
                h.write(lot);
                h.write(to);
            }
        }
    }
}

/// 1 つの駐車場の、停めてある台数と、向かっている車のために押さえた台数。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Lot {
    pub parked: u32,
    pub reserved: u32,
}

impl StateHash for Lot {
    fn write_state(&self, h: &mut StateHasher) {
        h.write(&self.parked);
        h.write(&self.reserved);
    }
}

/// 建物ごとの駐車場。どちらの台数も 0 の建物は持たない。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct Parking {
    lots: BTreeMap<BuildingId, Lot>,
}

impl Parking {
    fn get(&self, b: BuildingId) -> Lot {
        self.lots.get(&b).copied().unwrap_or_default()
    }

    /// 建物 `b` の駐車場を `f` で書き換える。どちらの台数も 0 になったら表から外す。
    fn update(&mut self, b: BuildingId, f: impl FnOnce(&mut Lot)) {
        let lot = self.lots.entry(b).or_default();
        f(lot);
        if *lot == Lot::default() {
            self.lots.remove(&b);
        }
    }

    fn reserve(&mut self, b: BuildingId) {
        self.update(b, |l| {
            l.reserved = l.reserved.checked_add(1).expect("押さえた台数があふれた");
        });
    }

    /// 押さえた空きを返す。建物が消えて駐車場がなくなっていれば何もしない。
    fn release(&mut self, b: BuildingId) {
        if self.lots.contains_key(&b) {
            self.update(b, |l| {
                l.reserved = l
                    .reserved
                    .checked_sub(1)
                    .expect("押さえていない空きを返した");
            });
        }
    }

    /// 押さえた空きに車を停め、停めたら `true`。駐車場がなくなっていれば、車は停めずに消える。
    fn park(&mut self, b: BuildingId) -> bool {
        let there = self.lots.contains_key(&b);
        if there {
            self.update(b, |l| {
                l.reserved = l
                    .reserved
                    .checked_sub(1)
                    .expect("押さえていない空きに停めた");
                l.parked = l.parked.checked_add(1).expect("停めた台数があふれた");
            });
        }
        there
    }

    /// 停めてある車を 1 台出す。停めてある車がなければ `false`。
    pub(super) fn unpark(&mut self, b: BuildingId) -> bool {
        let had = self.get(b).parked > 0;
        if had {
            self.update(b, |l| l.parked -= 1);
        }
        had
    }

    fn is_free(&self, id: BuildingId, b: &Building) -> bool {
        let l = self.get(id);
        u64::from(l.parked) + u64::from(l.reserved) < b.parking_spaces()
    }

    /// 行き先 `to` の近くで空きのある駐車場を、試す順に返す。行き先そのものを先に、そのあとは
    /// [`PARK_RADIUS`] マスの中の建物を近い順に並べる。同じ距離なら番号の小さい建物を先にする。
    fn candidates(
        &self,
        buildings: &Buildings,
        to: BuildingId,
        dest: &Building,
    ) -> Vec<BuildingId> {
        let r = i64::try_from(PARK_RADIUS).expect("範囲が i64 に収まらない");
        let at = |dx: i64, dy: i64| {
            let c = |v: i64, d: i64| v.checked_add(d).expect("マスの座標があふれた");
            TilePos::new(c(dest.front.x, dx), c(dest.front.y, dy))
        };
        let area = TileRect::from_corners(at(-r, -r), at(r, r));
        let dist = |a: TilePos| a.x.abs_diff(dest.front.x).max(a.y.abs_diff(dest.front.y));
        let mut near: Vec<(u64, BuildingId)> = buildings
            .fronts_in(area)
            .filter(|id| *id != to)
            .filter_map(|id| Some((id, buildings.get(id)?)))
            .filter(|(id, b)| self.is_free(*id, b))
            .map(|(id, b)| (dist(b.front), id))
            .collect();
        near.sort_unstable();
        let first = self.is_free(to, dest).then_some(to);
        first
            .into_iter()
            .chain(near.into_iter().map(|(_, id)| id))
            .collect()
    }
}

impl StateHash for Parking {
    fn write_state(&self, h: &mut StateHasher) {
        h.write(&self.lots);
    }
}

/// 建物の出入口の位置。出入口の歩道の上でもあり、同じ側の車のリンクの上でもある。
/// 出入口のリンクがなければ `None`。
pub fn building_spot(net: &Network, b: &Building) -> Option<SidewalkPos> {
    SidewalkPos::on_any_link(net, b.entrance, b.entrance_at)
}

/// 手段 `mode` で `from` から `to` へ行けるか。
fn reachable(
    net: &Network,
    router: &mut AstarRouter,
    mode: LaneMode,
    from: SidewalkPos,
    to: SidewalkPos,
) -> bool {
    let ends = from.resolve_on(net, mode).zip(to.resolve_on(net, mode));
    ends.is_some_and(|e| plan(router, net, mode, &[e], false)[0].is_some())
}

impl Trip {
    /// 空きを押さえている駐車場。車を停めるまでのあいだだけ持つ。
    pub fn parking_lot(&self) -> Option<BuildingId> {
        self.then.lot()
    }
}

impl Trips {
    /// 建物 `b` の駐車場の台数。
    pub fn lot(&self, b: BuildingId) -> Lot {
        self.parking.get(b)
    }

    /// 駐車場を建物の番号の順に返す。
    pub fn lots(&self) -> impl Iterator<Item = (BuildingId, Lot)> + '_ {
        self.parking.lots.iter().map(|(id, l)| (*id, *l))
    }

    /// 建物 `from` から建物 `to` へ、歩く → 車 → 歩く で向かう移動を作る。行き先の駐車場の空きは
    /// このときに押さえ、`from` に停めてある車があれば 1 台出す。建物がないか、行き先の近くがどこも満車か、
    /// 3 つの区切りのどれかの経路がなければ `None` で、駐車場も変えない。
    pub fn spawn_drive(
        &mut self,
        net: &Network,
        router: &mut AstarRouter,
        buildings: &Buildings,
        (from, to): (BuildingId, BuildingId),
        now: Tick,
    ) -> Option<TripId> {
        self.spawn_drive_taking(net, router, buildings, (from, to), Some(from), now)
    }

    /// [`Trips::spawn_drive`] と同じだが、停めてある車を出す駐車場を `take` で決める。`None` なら
    /// どの駐車場からも出さない。
    pub(super) fn spawn_drive_taking(
        &mut self,
        net: &Network,
        router: &mut AstarRouter,
        buildings: &Buildings,
        (from, to): (BuildingId, BuildingId),
        take: Option<BuildingId>,
        now: Tick,
    ) -> Option<TripId> {
        let origin = building_spot(net, buildings.get(from)?)?;
        let dest_b = buildings.get(to)?;
        let dest = building_spot(net, dest_b)?;
        let took = take.filter(|b| self.parking.unpark(*b));
        let id = self.start_drive(net, router, buildings, (origin, to, dest), now);
        if let (None, Some(b)) = (id, took) {
            self.parking.update(b, |l| {
                l.parked = l.parked.checked_add(1).expect("停めた台数があふれた");
            });
        }
        id
    }

    fn start_drive(
        &mut self,
        net: &Network,
        router: &mut AstarRouter,
        buildings: &Buildings,
        (origin, to, dest): (SidewalkPos, BuildingId, SidewalkPos),
        now: Tick,
    ) -> Option<TripId> {
        router.sync(net);
        // 近い順に試し、車でも歩いても行ける最初の駐車場にする。
        let (lot, park_at) = self
            .parking
            .candidates(buildings, to, buildings.get(to)?)
            .into_iter()
            .filter_map(|lot| Some((lot, building_spot(net, buildings.get(lot)?)?)))
            .find(|(_, at)| {
                reachable(net, router, LaneMode::Car, origin, *at)
                    && reachable(net, router, LaneMode::Sidewalk, *at, dest)
            })?;
        let id = self.spawn_many(net, router, &[(origin, origin)], now)[0]?;
        self.rows[id.slot()]
            .as_mut()
            .expect("作った主体がない")
            .then = Then::Drive {
            park_at,
            lot,
            to: dest,
        };
        self.parking.reserve(lot);
        Some(id)
    }

    /// 建物が消えたあとに呼ぶ。表にない建物の駐車場を、停めてあった車ごと捨てる。
    pub fn forget_lots(&mut self, buildings: &Buildings) {
        self.parking
            .lots
            .retain(|id, _| buildings.get(*id).is_some());
    }

    /// 主体を消す。押さえていた駐車場の空きは返す。
    pub(super) fn remove(&mut self, id: TripId) {
        if self.slots.release(id.index, id.generation) {
            let t = self.rows[id.slot()].take().expect("消す主体がない");
            if let Some(lot) = t.then.lot() {
                self.parking.release(lot);
            }
        }
    }

    /// 車線に出られないまま待ち続けた車 `rows` の車での移動をやめ、出発の建物から行き先まで歩く移動に
    /// 切り替える。押さえていた駐車場の空きは返す。歩く経路がなければ `ended` に足す。
    pub(super) fn walk_instead(
        &mut self,
        net: &Network,
        router: &mut AstarRouter,
        rows: Vec<usize>,
        ended: &mut Vec<usize>,
    ) {
        for i in rows {
            let t = self.rows[i].as_mut().expect("待つ主体がない");
            let Then::Walk { lot, to } = t.then else {
                ended.push(i);
                continue;
            };
            t.then = Then::Arrive;
            let (from, departed) = (t.position(), t.departed);
            self.parking.release(lot);
            let ends = from
                .resolve_on(net, LaneMode::Sidewalk)
                .zip(to.resolve_on(net, LaneMode::Sidewalk));
            let found =
                ends.and_then(|e| plan(router, net, LaneMode::Sidewalk, &[e], false).remove(0));
            match found {
                Some((route, e, jay)) => {
                    self.rows[i] = Some(make_trip(net, route, e, to, departed, jay));
                    self.rescues.walked_instead = self.rescues.walked_instead.saturating_add(1);
                }
                None => ended.push(i),
            }
        }
    }

    /// 区切りの終わりに着いた行 `done` を、次の区切りへ進める。移動を終える行と、次の区切りの経路が
    /// なくなっていた行を返す。
    pub(super) fn next_legs(
        &mut self,
        net: &Network,
        router: &mut AstarRouter,
        done: Vec<usize>,
    ) -> Vec<usize> {
        let mut ended = Vec::new();
        let mut legs: [Vec<(usize, SidewalkPos, SidewalkPos, Then)>; 2] = Default::default();
        for i in done {
            let t = self.rows[i].as_mut().expect("着いた主体がない");
            match t.then {
                Then::Arrive => ended.push(i),
                Then::Drive { park_at, lot, to } => {
                    legs[0].push((i, t.dest, park_at, Then::Walk { lot, to }));
                }
                Then::Walk { lot, to } => {
                    t.then = Then::Arrive;
                    let dest = t.dest;
                    if self.parking.park(lot) {
                        let (index, generation) = self.slots.current(i).expect("着いた主体がない");
                        self.commuters.parked(TripId { index, generation }, lot);
                    }
                    legs[1].push((i, dest, to, Then::Arrive));
                }
            }
        }
        // 歩く区切りを先に作り、そのあと車を車線に置く。降りた車は、もう車線の上にいない。
        let [cars, walks] = legs;
        for (mode, legs) in [(LaneMode::Sidewalk, walks), (LaneMode::Car, cars)] {
            let resolved: Vec<Option<Ends>> = legs
                .iter()
                .map(|(_, a, b, _)| a.resolve_on(net, mode).zip(b.resolve_on(net, mode)))
                .collect();
            let ends: Vec<Ends> = resolved.iter().flatten().copied().collect();
            let mut routes = plan(router, net, mode, &ends, false).into_iter();
            // 置き直す行はまだ歩いているので、車線の上の車には入らない。
            let mut ix = (mode == LaneMode::Car).then(|| car::LaneIndex::of(&self.rows, None));
            for ((i, _, to, then), e) in legs.into_iter().zip(resolved) {
                let found = e.and_then(|_| routes.next().expect("探した経路の数が合わない"));
                let Some((route, e, jay)) = found else {
                    ended.push(i);
                    continue;
                };
                let old = self.rows[i].as_ref().expect("着いた主体がない");
                let departed = old.departed;
                // 歩道から車線へ、車線から歩道へ移るときも、それまで描いていたずれから少しずつ移す。
                // 車は道路のどちらの側の車線からも出るので、移る幅は道路の幅ほどになりうる。
                let shown = old.shift_on_leg(net, old.leg as usize);
                let mut t: Trip = make_trip(net, route, e, to, departed, jay);
                t.then = then;
                self.rows[i] = Some(t);
                // 出発と同じく、空いた車線に置き、前の車と重なるなら下げ、空きがなければ待たせる。
                if let Some(ix) = ix.as_mut() {
                    car::place(&mut self.rows, net, ix, i);
                }
                let t = self.rows[i].as_mut().expect("置いた主体がない");
                t.blend = Some(lane_shift::Blend {
                    from: shown,
                    start: t.offset.0,
                });
            }
        }
        ended
    }
}
