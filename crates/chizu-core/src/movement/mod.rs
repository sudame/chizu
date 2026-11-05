//! 移動の主体（`Trips`）と、歩道を歩く人と車線を走る車の細かい計算。
//!
//! 移動の主体は、経路と、いまのリンクの上の位置（リンクの始まりからの距離）を持つ。
//! 1 tick に移動の 1 秒ぶん進み、行き先に着いたら消える。歩行者どうしはぶつからない。
//! 歩行者は交差点の横断歩道と、区間の途中の横切りで車道を渡り、渡るあいだは車を止める（`crossing`）。
//! 車は同じ車線の前の車との車間で速さを決め、ノードで次のリンクの車線へ移る。
//! 交差点では、衝突する向きの車に譲ってノードの手前で待つ（`junction`）。
//! 建物から建物へ車で向かう移動は、歩く → 車 → 歩く の 3 つの区切りを順に進む（`parking`）。

mod car;
#[cfg(test)]
mod car_tests;
mod commute;
mod congestion;
pub mod crossing;
#[cfg(test)]
mod crossing_tests;
mod junction;
#[cfg(test)]
mod junction_tests;
pub(crate) mod lane_shift;
mod parking;
#[cfg(test)]
mod parking_tests;
mod pos;
pub mod signal;
#[cfg(test)]
mod signal_tests;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod upgrade_tests;

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::sync::Arc;

use smallvec::SmallVec;

#[cfg(feature = "parallel")]
use rayon::prelude::*;

pub use car::{CAR_ACCEL, CAR_DECEL, CAR_LENGTH, MIN_GAP};
pub use commute::{CommuteRequest, Commuter, DRIVE_MIN_MM, HouseholdCar, OWNER_DRIVE_MIN_MM};
pub use congestion::{Congestion, classify as classify_congestion};
pub use junction::Rescues;
pub use parking::{
    CurbWait, DailyLotMisses, Lot, LotEnd, LotId, LotMisses, PARK_RADIUS, ParkingTally,
    SHORTAGE_LOTS_FULL, SharedLot, building_spot, facility_spot, lane_tiles, lot_end, lot_spot,
    lot_tiles, shared_facility,
};
pub use pos::SidewalkPos;

use crate::command::NodeId;
use crate::id::{Slots, TripId};
use crate::network::{Dir, LaneId, LaneMode, LinkId, Network, SegmentId};
use crate::rng::{RngPurpose, mix_words};
use crate::routing::{
    AstarRouter, Connector, Graph, Route, RouteRequest, Router, Scratch, Toward, WALK_SPEED,
    one_to_many,
};
use crate::state_hash::{StateHash, StateHasher};
use crate::tile::TilePos;
use crate::units::{LaneOffset, Mm, MmPerTick, Tick, WorldPos};

/// 歩く人を並列に進めるときに、1 本のスレッドが受け持つ行の数の下限。行がこの 2 倍に満たなければ
/// 1 本のスレッドで回す。1 行は数ナノ秒で済むので、これより小さく分けると、スレッドを起こして
/// 待ち合わせる時間のほうが長くなる。
///
/// 分け方で結果は変わらない。単体テストでは、小さな街でも分けた道を通してスレッド数に依らないことを
/// 確かめられるように 1 にする。
#[cfg(feature = "parallel")]
const PAR_MIN_WALKS: usize = if cfg!(test) { 1 } else { 8_192 };

/// 車の前を見て速さを決めるのを並列に回すときに、1 本のスレッドが受け持つ車の数の下限。
/// 単体テストで 1 にするのは [`PAR_MIN_WALKS`] と同じ理由である。
#[cfg(feature = "parallel")]
const PAR_MIN_CARS: usize = if cfg!(test) { 1 } else { 256 };

/// 歩くか車で走って移動している主体。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Trip {
    mode: LaneMode,
    route: Arc<Route>,
    /// いまいる車線。歩くときは歩道の車線である。
    lane: LaneId,
    /// 車の速さ。歩くときは使わない。
    speed: MmPerTick,
    /// 出発の位置の車線に空きがなく、車線の上に出るのを待っている車か。
    waiting: bool,
    /// `waiting` の車のうち、道路の種類が変わって走っていた車線がなくなり、区間の途中で車線に戻るのを
    /// 待っている車か。建物の前から出発を待つ車とは分けて数える。
    rejoining: bool,
    /// 車なら、交差点の手前で、ほかの車や横断歩道の人に譲って待った tick 数。ノードを抜けたら 0 に戻す。
    /// 歩く人なら、車道を渡り始めるのを続けて待った tick 数。歩き出したら 0 に戻す。
    waited: u32,
    /// 車が交差点の手前で、横断歩道の人だけのために待った tick 数。ノードを抜けたら 0 に戻す。
    /// 歩く人が渡り始めてよいかを決めるのに使う（`crossing`）。
    yielded: u32,
    /// 車線の上で、位置が変わらないまま過ぎた tick 数。動いたら 0 に戻す。
    stalled: u32,
    /// いま歩いているリンクの、経路の中での番号。
    leg: u32,
    offset: LaneOffset,
    /// いまのリンクの長さ。
    link_len: LaneOffset,
    /// いまのリンクの始まりのマスと、1 歩の向き。網が変わっても位置を求め直せるように持つ。
    start: TilePos,
    step: (i8, i8),
    /// 行き先の出入口と、経路の最後のリンクの上でのその位置。
    dest: SidewalkPos,
    dest_offset: LaneOffset,
    /// いまの区切りが終わったあとにすること。
    then: parking::Then,
    /// 歩道の側と、区間の途中の横切り。歩くときだけ使う。
    walk: crossing::Walk,
    /// 交差点で車道を渡り始めた人の、次のリンクでの側と渡る腕。渡り終えるまで変えない。渡っている途中で
    /// 横断歩道を外したり戻したりしても、いまの位置が別の腕の上へ移らないようにするためである。
    across: Option<(bool, crossing::Arms)>,
    departed: Tick,
    /// 車線や歩道が変わった直後の、描くずれの移り変わり。描画のためだけの値で、状態ハッシュには入れない。
    blend: Option<lane_shift::Blend>,
    /// 経路の最初のリンクのどこから歩き出したか。描画のためだけの値で、状態ハッシュにも保存データにも
    /// 入れず、比べるときも無視する。
    #[serde(skip)]
    origin: Origin,
}

/// [`Trip::origin`]。比べると常に等しい。
#[derive(Clone, Copy, Debug)]
enum Origin {
    /// 出入口から歩き出した。値は、その出入口の、リンクの始まりからの距離である。
    Door(LaneOffset),
    /// リンクの始まりのノードを抜けてきた。探し直す前の経路で、ノードを抜けていまのリンクに入っていた人である。
    Entered,
}

impl Default for Origin {
    fn default() -> Origin {
        Origin::Door(LaneOffset(0))
    }
}

impl PartialEq for Origin {
    fn eq(&self, _: &Origin) -> bool {
        true
    }
}

impl Eq for Origin {}

impl Trip {
    pub fn route(&self) -> &Route {
        &self.route
    }

    pub fn mode(&self) -> LaneMode {
        self.mode
    }

    pub fn lane(&self) -> LaneId {
        self.lane
    }

    /// 車線に出るのを待っている車か。車線に戻るのを待つ車も含む。
    pub fn waiting(&self) -> bool {
        self.waiting
    }

    /// 駐車場の中で、車線に出る順番を待っている車か。
    pub fn waiting_to_depart(&self) -> bool {
        self.mode == LaneMode::Car && self.waiting && !self.rejoining
    }

    /// 駐車場の中で車線に出るのを待ち始めてから数えた tick 数。車線に出るのを待っていない車は 0。
    /// 読み取り用で、`RESCUE` を越えると歩きに切り替わる。
    pub fn curb_wait_ticks(&self) -> u32 {
        if self.waiting_to_depart() {
            self.stalled
        } else {
            0
        }
    }

    /// 行き先の建物の駐車場が満車で、近くのほかの建物の駐車場に停めに向かっているか。
    /// 押さえた駐車場の建物の出入口が、行き先の出入口と違えば、ほかの建物の駐車場である。
    pub fn parks_elsewhere(&self, net: &Network, buildings: &crate::building::Buildings) -> bool {
        let Some((lot, to)) = (match self.then {
            parking::Then::Arrive => None,
            parking::Then::Drive { lot, to, .. } | parking::Then::Walk { lot, to } => {
                Some((lot, to))
            }
        }) else {
            return false;
        };
        match lot {
            // 駐車場の建物が取り壊されていれば、もう向かう先の駐車場はないので数えない。
            parking::LotId::Building(lot) => buildings
                .get(lot)
                .is_some_and(|b| building_spot(net, b) != Some(to)),
            // 共同の駐車場の外来の枠は、建物の駐車場ではない所である。
            parking::LotId::Shared(_) => true,
            // 借りた枠に停めるのは、ふつう家に帰る車である。
            parking::LotId::Rented(_) => false,
        }
    }

    pub fn speed(&self) -> MmPerTick {
        self.speed
    }

    pub fn link(&self) -> LinkId {
        self.route.links[self.leg as usize]
    }

    /// いまのリンクの始まりからの距離。
    pub fn offset(&self) -> LaneOffset {
        self.offset
    }

    pub fn departed(&self) -> Tick {
        self.departed
    }

    /// いまの位置。歩く人が交差点で車道を渡っているあいだは、リンクの終わりにいるものとする。
    pub fn position(&self) -> SidewalkPos {
        let at = if self.mode == LaneMode::Sidewalk {
            self.offset.min(self.link_len)
        } else {
            self.offset
        };
        SidewalkPos::along(self.start, self.step, at)
    }

    /// 歩く人が、交差点の横断歩道か区間の途中の横切りで、車道を渡っているか渡るのを待っているか。
    /// 区間の途中なら `Some(true)`、横断歩道なら `Some(false)`。
    pub fn crossing(&self) -> Option<bool> {
        if self.mode != LaneMode::Sidewalk {
            return None;
        }
        if self.walk.jay.is_some() {
            return Some(true);
        }
        (!self.last_leg() && self.offset >= self.link_len).then_some(false)
    }

    /// リンク `link` のいまの車線のずれ。車線を移る前のずれからの移り変わりは含まない。
    fn lane_target(&self, net: &Network, link: LinkId) -> (i64, i64) {
        let index = match self.mode {
            LaneMode::Car => net
                .link_lanes(link)
                .position(|l| l == self.lane)
                .unwrap_or(0),
            LaneMode::Sidewalk => 0,
        };
        let (x, y) = lane_shift::target(net, link, self.step, index);
        if self.mode == LaneMode::Sidewalk && self.walk.far {
            (-x, -y)
        } else {
            (x, y)
        }
    }

    /// 経路の `leg` 番めのリンクの上にいるときの、描くずれ。
    fn shift_on_leg(&self, net: &Network, leg: usize) -> (i64, i64) {
        let Some(&link) = self.route.links.get(leg) else {
            return (0, 0);
        };
        lane_shift::blended(self.lane_target(net, link), self.blend, self.offset.0)
    }

    /// 描くときに位置へ足す、車線や歩道に寄せるずれ。
    fn shift(&self, net: &Network) -> (Mm, Mm) {
        let (x, y) = self.shift_on_leg(net, self.leg as usize);
        (Mm(x), Mm(y))
    }

    /// 車の描くための道筋（[`CarPath`]）。歩く人なら `None`。
    fn car_path(&self, net: &Network) -> Option<CarPath> {
        if self.mode != LaneMode::Car {
            return None;
        }
        let mm = |(x, y): (i64, i64)| (Mm(x), Mm(y));
        let prev = (self.leg as usize)
            .checked_sub(1)
            .map(|k| self.route.links[k])
            .and_then(|l| {
                let (_, step, _) = pos::link_geometry(net, l)?;
                // ノードを越えたときに、越える前のずれを移り変わりの始まりとして控えてある。
                let shown = self
                    .blend
                    .map_or_else(|| lane_shift::target(net, l, step, 0), |b| b.from);
                Some((l, mm(shown)))
            });
        Some(CarPath {
            link: self.link(),
            offset: self.offset,
            len: self.link_len,
            prev,
            lane_shift: mm(self.lane_target(net, self.link())),
        })
    }

    /// 歩く人の描くための道筋（[`WalkPath`]）。車なら `None`。区間の途中を横切っているあいだと、
    /// リンクの両端から離れているあいだも `None` で、中核の位置のまま描く。ただし、網が変わって探し直すのを
    /// 待っている人は、どこにいても値を返す（[`WalkPath::stale`]）。
    fn walk_path(&self, net: &Network) -> Option<WalkPath> {
        if self.mode != LaneMode::Sidewalk || self.walk.jay.is_some() {
            return None;
        }
        let door = match self.origin {
            Origin::Door(at) if self.leg == 0 => Some(at),
            _ => None,
        };
        let entered = door.is_none();
        let len = self.link_len.0;
        let stale = self.stale(net);
        let near_start = entered && self.offset.0 < WALK_WINDOW_MM;
        let near_end = !self.last_leg() && self.offset.0.saturating_add(WALK_WINDOW_MM) >= len;
        if !stale && !near_start && !near_end {
            return None;
        }
        let ahead = (near_end && !stale).then(|| self.walk_ahead(net)).flatten();
        // 渡っているあいだは、道筋が求められなくても、画面が交差点の中心に描かないよう値を返す。
        if !stale && !near_start && ahead.is_none() && self.offset.0 <= len {
            return None;
        }
        let (x, y) = self.lane_target(net, self.link());
        Some(WalkPath {
            link: self.link(),
            offset: self.offset,
            len: self.link_len,
            lane_shift: (Mm(x), Mm(y)),
            entered,
            from: door.unwrap_or(LaneOffset(0)),
            to: self.last_leg().then_some(self.dest_offset),
            ahead,
            stale,
        })
    }

    /// 探し直す前の、歩き出した所。最初のリンクの上にいて出入口から歩き出していれば、その出入口の位置
    /// （`Ok`）、ノードを抜けてきたなら、いまのリンクの始まりのマスと 1 歩の向き（`Err`）。
    fn walk_start(&self) -> Result<SidewalkPos, (TilePos, (i8, i8))> {
        match self.origin {
            Origin::Door(at) if self.leg == 0 => Ok(SidewalkPos::along(self.start, self.step, at)),
            _ => Err((self.start, self.step)),
        }
    }

    /// 探し直して作った移動の、[`Trip::origin`]。前の出入口がいまのリンクの上の、いまの位置より手前に
    /// 残っていればそれを、前にノードを抜けて入ったリンクと同じ所から始まるリンクにいれば `Entered` を引き継ぐ。
    /// どちらでもなければ、リンクの始まりから歩き出したものとする。描く位置がいまのリンクの始まりから
    /// 交差点まで続くので、道筋なしにならない。
    fn rerouted_origin(
        &self,
        net: &Network,
        before: Result<SidewalkPos, (TilePos, (i8, i8))>,
    ) -> Origin {
        match before {
            Ok(door) => match door.resolve_on(net, self.mode) {
                Some((link, at)) if link == self.link() && at <= self.offset => Origin::Door(at),
                _ => Origin::default(),
            },
            Err(start) if start == (self.start, self.step) => Origin::Entered,
            Err(_) => Origin::default(),
        }
    }

    /// 網が変わって、いまのリンク、次のリンク、渡っている腕のどれかの区間がなくなったか。
    /// 次の `step` で探し直すまでのあいだである。
    fn stale(&self, net: &Network) -> bool {
        let gone = |s: SegmentId| net.segment(s).is_none();
        self.route.links[self.leg as usize..]
            .iter()
            .take(2)
            .any(|l| gone(l.segment))
            || self
                .across
                .as_ref()
                .is_some_and(|(_, arms)| arms.iter().any(|&s| gone(s)))
    }

    /// リンクの終わりのノードを抜ける道筋。網が変わって区間やノードがなくなっていれば `None`。
    fn walk_ahead(&self, net: &Network) -> Option<WalkAhead> {
        let link = self.link();
        let next = *self.route.links.get(self.leg as usize + 1)?;
        // 網を変えた直後は、次の `step` で経路を直すまで、経路のリンクが網にないことがある。
        net.segment(next.segment)?;
        let node = crossing::end_node(net, link)?;
        let (_, _, next_len) = pos::link_geometry(net, next)?;
        let (arms, corners) = match &self.across {
            // 渡り始めた人は、固定した腕から角の並びを作る。
            Some((far, arms)) => {
                let corners = crossing::corner_route(
                    net,
                    node,
                    (link, self.walk.far),
                    (next, *far),
                    Some(arms.as_slice()),
                );
                (arms.clone(), corners)
            }
            None => {
                let len = self.route.links.len();
                let need = (self.leg as usize + 2 == len).then_some(self.walk.far_end);
                let then = self.route.links.get(self.leg as usize + 2).map(|l| {
                    let need = (self.leg as usize + 3 == len).then_some(self.walk.far_end);
                    (*l, need)
                });
                let (_, arms, corners) = crossing::depart_route(
                    net,
                    node,
                    (link, self.walk.far),
                    next,
                    need,
                    then,
                    true,
                );
                (arms, corners)
            }
        };
        let next_end = if self.leg as usize + 2 == self.route.links.len() {
            self.dest_offset
        } else {
            next_len
        };
        (corners.len() >= 2).then_some(WalkAhead {
            node,
            corners,
            arms,
            next_len,
            next_end,
        })
    }

    fn last_leg(&self) -> bool {
        self.leg as usize + 1 == self.route.links.len()
    }

    /// 歩く人を 1 tick 進める。着いたかと、この tick の終わりに渡っている所を返す。
    ///
    /// 交差点を通るときは、リンクの終わりを越えた距離を、渡る腕の横断歩道を順に並べた道のりの上の位置とみなす。
    /// 渡り始めるところで `gate` が許さなければ、その手前（歩道の端）で待つ。
    fn advance(&mut self, net: &Network, gate: &crossing::Gate) -> (bool, Option<crossing::Occ>) {
        let speed = u32::try_from(WALK_SPEED.0).expect("歩く速さが負");
        let waited = core::mem::take(&mut self.waited);
        if let Some(done) = self.walk.jay {
            let spot = (self.link(), self.offset.0);
            let w = crossing::width(net, spot.0.segment);
            let start = if done == 0 {
                gate.jaywalk(net, spot.0, spot.1, waited)
            } else {
                crossing::Jaywalk::Cross
            };
            let next = if start == crossing::Jaywalk::Cross {
                done.saturating_add(speed)
            } else {
                self.waited = waited.saturating_add(1);
                0
            };
            self.walk.hold = start == crossing::Jaywalk::Hold;
            if next < w {
                self.walk.jay = Some(next);
                let occ = match start {
                    crossing::Jaywalk::Cross => Some(crossing::Occ::Jay(spot.0, spot.1)),
                    crossing::Jaywalk::Hold => Some(crossing::Occ::Hold(spot.0, spot.1)),
                    crossing::Jaywalk::Wait => None,
                };
                return (false, occ);
            }
            // 渡り終えた。反対の側の歩道から、少しずつ寄せて歩き出す。
            let shown = self.shift_on_leg(net, self.leg as usize);
            self.walk.far = !self.walk.far;
            self.walk.jay = None;
            self.blend = Some(lane_shift::Blend {
                from: shown,
                start: self.offset.0,
            });
            return (false, None);
        }
        let mut p0 = self.offset.0;
        let mut p1 = p0.saturating_add(speed);
        loop {
            if self.last_leg() {
                self.offset = LaneOffset(p1);
                return (p1 >= self.dest_offset.0, None);
            }
            let end = self.link_len.0;
            if p1 < end {
                self.offset = LaneOffset(p1);
                return (false, None);
            }
            let (far, arms) = self.departure(net);
            let fixed = (far, arms.clone());
            // 渡る所を、リンクの上の位置の区間 [始まり, 終わり) で並べる。
            let node = car::end_node(net, self.link());
            let mut spans: SmallVec<[(u32, u32, crossing::Occ); 4]> = SmallVec::new();
            let mut at = end;
            for s in arms {
                let w = crossing::width(net, s);
                spans.push((at, at + w, crossing::Occ::Crosswalk(node, s)));
                at += w;
            }
            for &(s, _, occ) in &spans {
                let crossing::Occ::Crosswalk(n, seg) = occ else {
                    continue;
                };
                if p0 <= s && s < p1 && !gate.may_cross(net, n, seg) {
                    p1 = s;
                    self.waited = waited.saturating_add(1);
                    break;
                }
            }
            if p1 < at {
                self.offset = LaneOffset(p1);
                if p1 > end {
                    self.across = Some(fixed);
                }
                let occ = spans
                    .iter()
                    .find(|(s, e, _)| *s < p1 && p1 < *e)
                    .map(|x| x.2);
                return (false, occ);
            }
            let rest = p1 - at;
            self.across = None;
            self.leg += 1;
            self.enter(net, LaneOffset(rest), None);
            self.walk.far = far;
            (p0, p1) = (0, rest);
        }
    }

    /// 歩く人が、いまのリンクの終わりのノードから次のリンクへ移るときの、次のリンクでの歩道の側と、
    /// 渡る腕。次のリンクが経路の最後なら、行き先の建物の側へ出る。そうでなければ、その次の交差点で
    /// 横断歩道を外した腕を越えずに済む側を選ぶ。
    fn departure(&self, net: &Network) -> (bool, crossing::Arms) {
        if let Some((far, arms)) = &self.across {
            return (*far, arms.clone());
        }
        let link = self.link();
        let next = self.route.links[self.leg as usize + 1];
        let len = self.route.links.len();
        let need = (self.leg as usize + 2 == len).then_some(self.walk.far_end);
        let then = self.route.links.get(self.leg as usize + 2).map(|l| {
            let need = (self.leg as usize + 3 == len).then_some(self.walk.far_end);
            (*l, need)
        });
        crossing::depart(
            net,
            car::end_node(net, link),
            (link, self.walk.far),
            next,
            need,
            then,
        )
    }

    /// いまの経路の番号のリンクに入る。車線を指定しなければ、リンクの最初の車線に入る。
    fn enter(&mut self, net: &Network, offset: LaneOffset, lane: Option<LaneId>) {
        // 呼ぶ側は `leg` を進めてから呼ぶ。直前のリンクでのずれを、新しいリンクへ移すために覚える。
        let from = (self.leg > 0).then(|| self.shift_on_leg(net, self.leg as usize - 1));
        let link = self.link();
        let (start, step, len) = pos::link_geometry(net, link).expect("経路のリンクがない");
        let lane = lane.unwrap_or_else(|| net.link_lanes(link).next().expect("リンクに車線がない"));
        (self.start, self.step, self.link_len, self.offset, self.lane) =
            (start, step, len, offset, lane);
        self.blend = from.map(|from| lane_shift::Blend {
            from,
            start: offset.0,
        });
    }
}

/// 移動の主体の表。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Trips {
    slots: Slots,
    rows: Vec<Option<Trip>>,
    parking: parking::Parking,
    commuters: commute::Commuters,
    /// 交差点の詰まりをほどく救済が働いた回数。状態ハッシュには入れない。
    rescues: junction::Rescues,
    /// 車で向かおうとした移動の、駐車場の決まり方の数。状態ハッシュには入れない。
    parking_tally: parking::ParkingTally,
    /// 建物ごとの、行き先にした車が駐車場で困った数。状態ハッシュには入れない。
    lot_misses: parking::MissTable,
    /// 直前の `step` と `reroute` で消えた移動の結果。状態ハッシュには入れない。
    finished: BTreeMap<TripId, Finished>,
    /// 直前の `step` の信号を決めた様子。画面に灯火を出すために控える。状態ハッシュには入れない。
    lights: signal::Lights,
}

/// 網を変える前に控えた、主体ごとのリンクの中の車線の番号と描いていたずれ。[`Trips::note_before_change`] で作る。
#[derive(Clone, Debug, Default)]
pub struct Noted(BTreeMap<TripId, (Option<usize>, (i64, i64))>);

/// 消えた移動の結果。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Finished {
    /// 行き先に着いたか。次の区切りの経路が見つからず消えた移動と、通れなくなって消えた移動は `false`。
    pub reached: bool,
    pub departed: Tick,
}

/// 画面に描く移動体。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Mover {
    pub id: TripId,
    pub pos: WorldPos,
    /// 進む向き。x と y はそれぞれ -1、0、1 のどれかである。
    pub heading: (i8, i8),
    /// 歩いているなら `Sidewalk`、車なら `Car`。
    pub mode: LaneMode,
    /// 描くときに `pos` へ足す、車線や歩道に寄せるずれ（mm）。進む向きの左へずれる。
    /// 描画のためだけの値で、状態にも状態ハッシュにも入らない。
    pub shift: (Mm, Mm),
    /// 車なら、ノードの前後を曲線でつないで描くための道筋。歩く人は `None`。
    pub path: Option<CarPath>,
}

/// 歩く人の、ノードの前後で `WalkPath` を求める距離（mm）。リンクの両端からこの距離の中にいる人だけが
/// 道筋を持つ。画面は、この中でだけ位置を写し替える。
pub const WALK_WINDOW_MM: u32 = 10_000;

/// 歩く人の、いまのリンクの上の位置と、リンクの終わりのノードを通る道筋。画面は、交差点の手前の人を
/// 歩道の角を回る道筋の上に、横断歩道を渡る人を縞の上に描くのに使う。
/// 描画のためだけの値で、状態にも状態ハッシュにも入らない。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WalkPath {
    pub link: LinkId,
    /// いまのリンクの始まりからの距離と、リンクの長さ。交差点で車道を渡っているあいだは、
    /// `offset` が `len` を超え、超えた分が渡る腕の横断歩道を順に並べた道のりの上の位置を表す。
    pub offset: LaneOffset,
    pub len: LaneOffset,
    /// いまの歩道の、中心線からのずれ。歩道に寄せる移り変わりは含まない。
    pub lane_shift: (Mm, Mm),
    /// 前のリンクからノードを抜けてきたか。出入口から歩き出して、まだ経路の最初のリンクにいるなら `false`。
    /// 経路を探し直した人も、前の経路でノードを抜けていまのリンクに入っていたなら `true` である。
    pub entered: bool,
    /// 歩き出した出入口の、リンクの始まりからの距離。ノードを抜けてきたなら 0 である。保存データから
    /// 読み込んだ直後と、経路を探し直して前の出入口がいまのリンクの上になくなったときは、0 になる。
    pub from: LaneOffset,
    /// 経路の最後のリンクなら、行き先の出入口の、リンクの始まりからの距離。
    pub to: Option<LaneOffset>,
    /// リンクの終わりのノードを抜ける道筋。終わりの近くにいないか、最後のリンクなら `None`。
    pub ahead: Option<WalkAhead>,
    /// 網が変わって、いまのリンク、次のリンク、渡っている腕のどれかの区間がなくなり、次の `step` で
    /// 探し直すのを待っているか。そのあいだは、網から描く位置を求められない。
    pub stale: bool,
}

/// 歩く人がノードを抜けるときの道筋。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WalkAhead {
    pub node: NodeId,
    /// 回る角を通る順に並べたもの（[`crossing::corner_route`]）。
    pub corners: SmallVec<[(SegmentId, bool); 8]>,
    /// 渡る腕の区間。渡る順に並ぶ。
    pub arms: crossing::Arms,
    /// 次のリンクの長さと、そこで歩き終わる距離（最後のリンクなら行き先の出入口、そうでなければリンクの長さ）。
    pub next_len: LaneOffset,
    pub next_end: LaneOffset,
}

/// 車の、いまのリンクの上の位置と、抜けてきたリンク。画面は、リンクの終わりのノードの手前で待つ車を
/// 停止線の手前に描き、ノードを抜けたばかりの車を曲線の上に描くのに使う。
/// 描画のためだけの値で、状態にも状態ハッシュにも入らない。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CarPath {
    pub link: LinkId,
    /// いまのリンクの始まりからの距離と、リンクの長さ。
    pub offset: LaneOffset,
    pub len: LaneOffset,
    /// 抜けてきたリンクと、ノードを越える直前に描いていたずれ。経路の最初のリンクにいるか、
    /// 抜けてきたリンクが網から消えていれば `None`。
    pub prev: Option<(LinkId, (Mm, Mm))>,
    /// いまの車線のずれ。車線を移る前のずれからの移り変わりは含まない。
    pub lane_shift: (Mm, Mm),
}

/// リンクと、その始まりからの距離。
type Spot = (LinkId, LaneOffset);

/// 出発の位置と行き先の位置。
type Ends = (Spot, Spot);

/// 歩道の位置 `(link, offset)` を、同じ区間の逆向きの歩道のリンクの上で表したもの。
fn flip(net: &Network, (link, offset): Spot) -> Option<Spot> {
    let back = LinkId {
        dir: match link.dir {
            Dir::Forward => Dir::Backward,
            Dir::Backward => Dir::Forward,
        },
        ..link
    };
    let len = pos::link_geometry(net, back)?.2;
    net.link_lanes(back).next()?;
    Some((back, LaneOffset(len.0.checked_sub(offset.0)?)))
}

/// 経路 `route` で `from` から `to` まで歩く距離。
fn walked_mm(net: &Network, route: &Route, (from, to): Ends) -> Option<u64> {
    let len = |l: &LinkId| pos::link_geometry(net, *l).map(|g| u64::from(g.2.0));
    let total = route
        .links
        .iter()
        .try_fold(0_u64, |acc, l| acc.checked_add(len(l)?))?;
    let rest = len(route.links.last()?)?.checked_sub(u64::from(to.1.0))?;
    total.checked_sub(u64::from(from.1.0))?.checked_sub(rest)
}

/// 経路 `route` で `from` から `to` まで車で走る費用（ms）。リンクの費用を、走る長さの割合で按分する。
fn driven_ms(net: &Network, g: &Graph, route: &Route, (from, to): Ends) -> Option<u64> {
    let last = route.links.len().checked_sub(1)?;
    route
        .links
        .iter()
        .enumerate()
        .try_fold(0_u64, |acc, (i, l)| {
            let len = u64::from(pos::link_geometry(net, *l)?.2.0);
            let start = if i == 0 { u64::from(from.1.0) } else { 0 };
            let end = if i == last { u64::from(to.1.0) } else { len };
            let part = g
                .link_cost(*l)?
                .checked_mul(end.checked_sub(start)?)?
                .checked_div(len)?;
            acc.checked_add(part)
        })
}

/// 手段 `mode` で 2 点を結ぶ経路を、まとめて探し、経路と、使った出発と行き先の位置を返す。
///
/// 歩道は 1 本の歩道を 2 つの向きに歩けるものとし、区間の 2 本の歩道のリンクを、その 2 つの向きとして使う。
/// 歩くときは、出発と行き先のそれぞれで 2 つの向きを試し、歩く距離のいちばん短い組を選ぶ。
/// 距離の同じ組は、位置を表したままの向きを先に選ぶ。同じリンクの手前へ戻る組は、向きを入れ替えた
/// まっすぐの組に必ず負けるので、ほかの組があれば探さない。
/// 歩く人は、どちらの向きに歩いても建物の側の歩道を歩き、交差点の横断歩道で側を変える（`crossing`）。
/// 出発と行き先が同じリンクの上にあり、建物が反対の側どうしなら、出発の位置で道路を横切る。そのときは
/// 歩く距離に [`crossing::JAYWALK_DETOUR_MM`] を足して比べるので、横断歩道までの遠回りがそれより短ければ横切らない。
///
/// 車も CS2 と同じく、建物の前からどちらの向きの車線へも出られ、どちらの向きからも着ける。
/// 出発と行き先のそれぞれで 2 つの向きを試し、走る費用のいちばん小さい組を選ぶ。こうしないと、
/// 道路の向こう側へ向かう車が、行き止まりや区画をぐるりと回って余計な車線を埋める。
/// `fixed_start` なら出発の向きは変えない。走っている車の経路を探し直すときに使う。
fn plan(
    router: &mut AstarRouter,
    net: &Network,
    mode: LaneMode,
    ends: &[Ends],
    fixed_start: bool,
) -> Vec<Option<(Arc<Route>, Ends, crossing::Walk)>> {
    let candidates: Vec<Vec<Ends>> = ends
        .iter()
        .map(|e| candidates(net, *e, fixed_start))
        .collect();
    let flat: Vec<(Ends, u8)> = candidates
        .iter()
        .zip(ends)
        .flat_map(|(c, e)| {
            let v = variant(*e);
            c.iter().map(move |c| (*c, v))
        })
        .collect();
    let mut found = plan_exact(router, mode, &flat).into_iter();
    let g = router.graph(mode);
    candidates
        .iter()
        .zip(ends)
        .map(|(c, (a, b))| {
            c.iter()
                .filter_map(|e| {
                    // 候補ごとに探した経路を 1 つずつ受け取るので、どの候補でも先に取り出す。
                    let r = found.next().expect("探した経路の数が合わない")?;
                    // 出発と行き先の建物が、選んだリンクの進む向きの左にあるか。同じリンクの上で
                    // 側が違えば、出発の位置で道路を横切る。
                    let (far, far_end) = (e.0.0 != a.0, e.1.0 != b.0);
                    let jay = mode == LaneMode::Sidewalk && r.links.len() == 1 && far != far_end;
                    // 車では、出発の建物と反対の向きの車線から出るかを `far` に持つ。車から歩きに
                    // 切り替えるときに、建物の側から歩き出すのに使う。
                    let walk = crossing::Walk {
                        far,
                        jay: jay.then_some(0),
                        far_end,
                        hold: false,
                    };
                    let key = match mode {
                        LaneMode::Car => driven_ms(net, g, &r, *e)?,
                        LaneMode::Sidewalk => walked_mm(net, &r, *e)?
                            .checked_add(u64::from(jay) * crossing::JAYWALK_DETOUR_MM)?,
                    };
                    let jay = walk;
                    Some((key, r, *e, jay))
                })
                .min_by_key(|(key, _, _, _)| *key)
                .map(|(_, r, e, jay)| (r, e, jay))
        })
        .collect()
}

/// 2 点を結ぶ経路で、費用の同じ経路のどの候補を選ぶか。出発と行き先の位置だけから決めるので、同じ 2 点のあいだは
/// いつも同じ経路を通り、同じ道に面した隣の家からは別の経路に分かれる。
fn variant(((a, ao), (b, bo)): Ends) -> u8 {
    let link =
        |l: LinkId| (u64::from(l.segment.index) << 2) | ((l.dir as u64) << 1) | l.mode as u64;
    let h = mix_words(
        0,
        RngPurpose::RouteVariant,
        &[link(a), u64::from(ao.0), link(b), u64::from(bo.0)],
    );
    (h % u64::from(crate::routing::ROUTE_VARIANTS)) as u8
}

/// [`plan`] が試す、出発と行き先の向きの組。同じリンクの手前へ戻る組は、ほかの組があれば除く。
fn candidates(net: &Network, (a, b): Ends, fixed_start: bool) -> Vec<Ends> {
    let starts = [Some(a), flip(net, a).filter(|_| !fixed_start)];
    let goals = [Some(b), flip(net, b)];
    let all: Vec<Ends> = starts
        .iter()
        .flatten()
        .flat_map(|s| goals.iter().flatten().map(|g| (*s, *g)))
        .collect();
    let ahead: Vec<Ends> = all.iter().filter(|e| !behind(e)).copied().collect();
    if ahead.is_empty() { all } else { ahead }
}

/// 行き先が出発と同じリンクの、出発より手前にあるか。
fn behind(((a, ao), (b, bo)): &Ends) -> bool {
    a == b && bo < ao
}

/// 手段 `mode` で 2 点を結ぶ経路を、それぞれの候補でまとめて探す。行き先が同じリンクの手前にあるときは、
/// 次のリンクへ出て戻ってくる経路にする。
fn plan_exact(
    router: &mut AstarRouter,
    mode: LaneMode,
    ends: &[(Ends, u8)],
) -> Vec<Option<Arc<Route>>> {
    let reqs: Vec<RouteRequest> = ends
        .iter()
        .filter(|(e, _)| !behind(e))
        .map(|(((a, _), (b, _)), variant)| RouteRequest {
            from: *a,
            to: *b,
            mode,
            variant: *variant,
        })
        .collect();
    let mut found = router.route_batch(&reqs).into_iter();
    let g = router.graph(mode);
    let mut s = Scratch::default();
    ends.iter()
        .map(|(e, _)| {
            if !behind(e) {
                return found.next().expect("探した経路の数が合わない");
            }
            let link = e.0.0;
            let v = g.vertex(link)?;
            let next: Vec<LinkId> = g.edges(v).iter().map(|x| g.link(x.to)).collect();
            let back = one_to_many(g, link, &next, Toward::Backward, &mut s);
            // 費用の同じ戻り道は、先に並ぶ（番号の小さい）リンクを選ぶ。
            let best = back.into_iter().flatten().min_by_key(|r| r.cost)?;
            let mut links = vec![link];
            links.extend(best.links);
            Some(Arc::new(Route {
                links,
                cost: best.cost.checked_add(g.link_cost(link)?)?,
            }))
        })
        .collect()
}

impl Trips {
    pub fn len(&self) -> usize {
        self.slots.live_count()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// 交差点の詰まりをほどく救済が働いた回数。
    pub fn rescues(&self) -> Rescues {
        self.rescues
    }

    /// 直前の `step` で信号の灯火を決めた tick と様子。画面とテストは、ここから効いた灯火を求める。
    pub fn lights(&self) -> &signal::Lights {
        &self.lights
    }

    /// 直前の `step` と `reroute` で消えた移動 `id` の結果。
    pub fn finished(&self, id: TripId) -> Option<Finished> {
        self.finished.get(&id).copied()
    }

    pub fn get(&self, id: TripId) -> Option<&Trip> {
        self.slots
            .contains(id.index, id.generation)
            .then(|| self.rows[id.slot()].as_ref())
            .flatten()
    }

    /// 移動の主体を ID の順に返す。
    pub fn iter(&self) -> impl Iterator<Item = (TripId, &Trip)> {
        self.rows.iter().enumerate().filter_map(|(i, t)| {
            let (index, generation) = self.slots.current(i)?;
            Some((TripId { index, generation }, t.as_ref()?))
        })
    }

    fn insert(&mut self, trip: Trip) -> TripId {
        let ((index, generation), new) = self.slots.alloc();
        if new {
            self.rows.push(Some(trip));
        } else {
            self.rows[index as usize] = Some(trip);
        }
        TripId { index, generation }
    }

    /// `from` から `to` へ歩く主体を作る。出入口が歩道の上にないか、つながっていなければ `None`。
    pub fn spawn(
        &mut self,
        net: &Network,
        router: &mut AstarRouter,
        from: SidewalkPos,
        to: SidewalkPos,
        now: Tick,
    ) -> Option<TripId> {
        self.spawn_many(net, router, &[(from, to)], now)
            .pop()
            .flatten()
    }

    /// 歩く主体をまとめて作り、依頼の順に ID を返す。経路はまとめて探す。経路探索が古い網のままなら、
    /// 先に作り直す。
    pub fn spawn_many(
        &mut self,
        net: &Network,
        router: &mut AstarRouter,
        walks: &[(SidewalkPos, SidewalkPos)],
        now: Tick,
    ) -> Vec<Option<TripId>> {
        self.spawn_mode(net, router, LaneMode::Sidewalk, walks, now)
    }

    /// 車で走る主体をまとめて作り、依頼の順に ID を返す。位置は、同じ区間の同じ向きの車のリンクの上に置く。
    /// 車は止まった状態で、リンクの最初の車線から走り出す。
    pub fn spawn_drives(
        &mut self,
        net: &Network,
        router: &mut AstarRouter,
        drives: &[(SidewalkPos, SidewalkPos)],
        now: Tick,
    ) -> Vec<Option<TripId>> {
        self.spawn_mode(net, router, LaneMode::Car, drives, now)
    }

    fn spawn_mode(
        &mut self,
        net: &Network,
        router: &mut AstarRouter,
        mode: LaneMode,
        walks: &[(SidewalkPos, SidewalkPos)],
        now: Tick,
    ) -> Vec<Option<TripId>> {
        router.sync(net);
        let resolved: Vec<Option<Ends>> = walks
            .iter()
            .map(|(a, b)| Some((a.resolve_on(net, mode)?, b.resolve_on(net, mode)?)))
            .collect();
        let ends: Vec<Ends> = resolved.iter().flatten().copied().collect();
        let mut routes = plan(router, net, mode, &ends, false).into_iter();
        // 車は、出発のリンクの車線のうち車の少ない車線に置く。前の車と重なるなら、その後ろの空いた
        // 位置まで下げる。どの車線にも空きがなければ、空くまで待たせる。
        let mut ix = car::LaneIndex::of(&self.rows, None);
        let (zones, entering) = if mode == LaneMode::Car {
            (
                crossing::jay_zones(net, &self.rows),
                car::Entering::of(&self.rows, net, router.connectors()),
            )
        } else {
            (BTreeMap::new(), car::Entering::default())
        };
        resolved
            .iter()
            .zip(walks)
            .map(|(e, (_, to))| {
                (*e)?;
                let (route, e, jay) = routes.next().expect("探した経路の数が合わない")?;
                let id = self.insert(make_trip(net, route, e, *to, now, jay));
                if mode == LaneMode::Car {
                    car::place(&mut self.rows, net, &mut ix, id.slot(), &zones, &entering);
                }
                Some(id)
            })
            .collect()
    }

    /// tick `now` から 1 tick 進め、移動を終えた主体を消して、その ID の順に返す。区切りの終わりに着いた主体は、
    /// その場で次の区切りの経路を探して進む。経路が見つからなかった主体も消して返す。
    /// `router` は `net` に合わせてあること。`now` は信号の段階を決めるのに使う。
    pub fn step(&mut self, net: &Network, router: &mut AstarRouter, now: Tick) -> Vec<TripId> {
        self.finished.clear();
        let done = self.advance(net, router.connectors(), now);
        let mut ended = self.next_legs(net, router, done);
        let give_up: Vec<usize> = (0..self.rows.len())
            .filter(|i| self.rows[*i].as_ref().is_some_and(car::given_up))
            .collect();
        self.walk_instead(net, router, give_up, &mut ended);
        ended.sort_unstable();
        ended
            .into_iter()
            .map(|i| {
                let (index, generation) = self.slots.current(i).expect("着いた主体がない");
                let id = TripId { index, generation };
                let t = self.rows[i].as_ref().expect("着いた主体がない");
                // 最後の歩く区切りを終えたときだけ着いたことになる。経路がなくなって消えた主体は、
                // 区切りの途中の手段と次にすることが残っている。
                let reached = t.mode == LaneMode::Sidewalk && t.then == parking::Then::Arrive;
                self.finished.insert(
                    id,
                    Finished {
                        reached,
                        departed: t.departed,
                    },
                );
                self.remove(id);
                id
            })
            .collect()
    }

    /// 歩く人と車を 1 tick 進め、区切りの終わりに着いた行を番号の順に返す。
    fn advance(&mut self, net: &Network, connectors: &[Connector], now: Tick) -> Vec<usize> {
        // 信号の灯火と、歩く人が渡り始めてよいかは、前の tick の車と歩く人の様子だけから決める。
        let gate = car::gate(
            &self.rows,
            net,
            connectors,
            now,
            car::signal_demand(&self.rows, net),
        );
        let advance = |(i, t): (usize, &mut Option<Trip>)| {
            let t = t.as_mut().filter(|t| t.mode == LaneMode::Sidewalk)?;
            let (arrived, occ) = t.advance(net, &gate);
            (arrived || occ.is_some()).then_some((i, arrived, occ))
        };
        #[cfg(feature = "parallel")]
        let walked: Vec<(usize, bool, Option<crossing::Occ>)> = self
            .rows
            .par_iter_mut()
            .with_min_len(PAR_MIN_WALKS)
            .enumerate()
            .filter_map(advance)
            .collect();
        #[cfg(not(feature = "parallel"))]
        let walked: Vec<(usize, bool, Option<crossing::Occ>)> = self
            .rows
            .iter_mut()
            .enumerate()
            .filter_map(advance)
            .collect();
        let mut arrived: Vec<usize> = walked.iter().filter(|w| w.1).map(|w| w.0).collect();
        let mut occupied = crossing::Occupied::of(net, walked.iter().filter_map(|w| w.2));
        // 長く待った人のいる横断歩道でも、止まりきれる車を止めて、人が渡り始められるようにする。
        occupied.crosswalks.extend(gate.claimed.iter().copied());
        arrived.extend(car::step(
            &mut self.rows,
            net,
            connectors,
            &mut self.rescues,
            now,
            &occupied,
            &gate.signals,
        ));
        self.lights = signal::Lights {
            now,
            demand: gate.signals,
        };
        arrived.sort_unstable();
        arrived
    }

    /// 網が変わったあとに呼ぶ。残りの経路に消えたリンクがある主体を、いまの位置から探し直す。
    /// いまの位置か行き先が歩道の上になくなったか、つながらなくなった主体は消し、その ID を返す。
    /// 経路探索が古い網のままなら、先に作り直す。
    pub fn reroute(&mut self, net: &Network, router: &mut AstarRouter) -> Vec<TripId> {
        self.reroute_noted(net, router, &Noted::default())
    }

    /// 網から区間 `segments` を消す前に呼ぶ。その区間を残りの経路に持つ主体の、リンクの中の車線の番号と
    /// 描いていたずれを控える。区間を消したあとの網では、どちらも求められないためである。
    pub fn note_before_change(&self, net: &Network, segments: &[SegmentId]) -> Noted {
        let rows = self
            .iter()
            .filter(|(_, t)| {
                t.route.links[t.leg as usize..]
                    .iter()
                    .any(|l| segments.contains(&l.segment))
            })
            .map(|(id, t)| {
                let index = net.link_lanes(t.link()).position(|l| l == t.lane);
                (id, (index, t.shift_on_leg(net, t.leg as usize)))
            })
            .collect();
        Noted(rows)
    }

    /// [`Trips::reroute`] と同じく探し直す。`noted` に控えた主体は、控えた車線の番号と描いていたずれを使う。
    pub fn reroute_noted(
        &mut self,
        net: &Network,
        router: &mut AstarRouter,
        noted: &Noted,
    ) -> Vec<TripId> {
        router.sync(net);
        self.finished.clear();
        let broken: Vec<(TripId, LaneMode, SidewalkPos, SidewalkPos)> = self
            .iter()
            .filter(|(_, t)| {
                t.route.links[t.leg as usize..]
                    .iter()
                    .any(|l| net.segment(l.segment).is_none())
            })
            .map(|(id, t)| (id, t.mode, t.position(), t.dest))
            .collect();
        let mut lost = Vec::new();
        for mode in [LaneMode::Car, LaneMode::Sidewalk] {
            let mut found = Vec::new();
            for (id, _, at, dest) in broken.iter().filter(|b| b.1 == mode) {
                match (at.resolve_on(net, mode), dest.resolve_on(net, mode)) {
                    (Some(a), Some(b)) => found.push((*id, (a, b), *dest)),
                    _ => lost.push(*id),
                }
            }
            let ends: Vec<_> = found.iter().map(|f| f.1).collect();
            for ((id, _, dest), route) in
                found
                    .into_iter()
                    .zip(plan(router, net, mode, &ends, mode == LaneMode::Car))
            {
                let Some((route, ends, jay)) = route else {
                    lost.push(id);
                    continue;
                };
                let old = self.get(id).expect("探し直す主体がない");
                let (departed, speed, waiting, rejoining, waited, yielded, stalled, then) = (
                    old.departed,
                    old.speed,
                    old.waiting,
                    old.rejoining,
                    old.waited,
                    old.yielded,
                    old.stalled,
                    old.then,
                );
                // 探し直す前の、リンクの中の車線の番号。網が変わると車線の ID も変わるので番号で覚える。
                // 探し直す前に描いていたずれ。車線や経路が変わっても、そこから少しずつ移す。
                // いまのリンクの区間が消えていると、どちらも網からは求められないので、控えた値を使う。
                let (index, shown) = match noted.0.get(&id) {
                    Some(&(index, shown)) => (index.unwrap_or(0), shown),
                    None => (
                        net.link_lanes(old.link())
                            .position(|l| l == old.lane)
                            .unwrap_or(0),
                        old.shift_on_leg(net, old.leg as usize),
                    ),
                };
                let old_far = old.walk.far;
                let start = old.walk_start();
                let mut t = make_trip(net, route, ends, dest, departed, jay);
                // 探し直しても、描く位置は前の出入口かノードから続ける（描画のためだけの値）。
                t.origin = t.rerouted_origin(net, start);
                (
                    t.speed,
                    t.waiting,
                    t.rejoining,
                    t.waited,
                    t.yielded,
                    t.stalled,
                    t.then,
                ) = (speed, waiting, rejoining, waited, yielded, stalled, then);
                // いまの位置は、いまのリンクと同じ向きのリンクで表してあるので、そのリンクから
                // 向きを変えたなら、側も入れ替わる。
                t.walk.far ^= old_far;
                if mode == LaneMode::Sidewalk {
                    // 渡りかけていた横切りは、歩道の端からやり直す。
                    let mismatch = t.route.links.len() == 1 && t.walk.far != t.walk.far_end;
                    t.walk.jay = mismatch.then_some(0);
                    t.walk.hold = false;
                }
                self.rows[id.slot()] = Some(t);
                if mode == LaneMode::Car && !waiting {
                    car::keep_lane(&mut self.rows, net, id.slot(), index);
                }
                if let Some(t) = self.rows[id.slot()].as_mut() {
                    t.blend = Some(lane_shift::Blend {
                        from: shown,
                        start: t.offset.0,
                    });
                }
            }
        }
        lost.sort();
        for id in &lost {
            if let Some(t) = self.get(*id) {
                let departed = t.departed;
                self.finished.insert(
                    *id,
                    Finished {
                        reached: false,
                        departed,
                    },
                );
            }
            self.remove(*id);
        }
        lost
    }

    /// ワールド座標の範囲 `[min, max)` にいる移動体を ID の順に返す。
    pub fn movers_in(&self, net: &Network, min: WorldPos, max: WorldPos) -> Vec<Mover> {
        self.iter()
            .filter(|(_, t)| !t.waiting)
            .filter_map(|(id, t)| {
                let pos = t.position().world();
                let inside = (min.x..max.x).contains(&pos.x) && (min.y..max.y).contains(&pos.y);
                inside.then(|| Mover {
                    id,
                    pos,
                    heading: t.step,
                    mode: t.mode,
                    shift: t.shift(net),
                    path: t.car_path(net),
                })
            })
            .collect()
    }

    /// 歩く人 `id` の、交差点の前後で歩道の角を回って描くための道筋。車と、交差点の近くにいない人は `None`。
    /// 角の並びを求めるので軽くない。画面に描く人だけに呼ぶ。
    pub fn walk_path(&self, net: &Network, id: TripId) -> Option<WalkPath> {
        self.get(id)?.walk_path(net)
    }
}

fn make_trip(
    net: &Network,
    route: Arc<Route>,
    ((_, from), (_, to)): Ends,
    dest: SidewalkPos,
    now: Tick,
    walk: crossing::Walk,
) -> Trip {
    let mode = route.links[0].mode;
    let first = route.links[0];
    let mut t = Trip {
        mode,
        lane: net.link_lanes(first).next().expect("リンクに車線がない"),
        speed: MmPerTick::ZERO,
        waiting: false,
        rejoining: false,
        waited: 0,
        yielded: 0,
        stalled: 0,
        route,
        leg: 0,
        offset: from,
        link_len: LaneOffset::START,
        start: TilePos::default(),
        step: (0, 0),
        dest,
        dest_offset: to,
        then: parking::Then::Arrive,
        walk,
        across: None,
        departed: now,
        blend: None,
        origin: Origin::Door(from),
    };
    t.enter(net, from, None);
    t
}

impl StateHash for Trip {
    fn write_state(&self, h: &mut StateHasher) {
        h.write(&(self.mode as u8));
        h.write(&self.lane);
        h.write(&self.speed);
        h.write(&self.waiting);
        h.write(&self.rejoining);
        h.write(&self.waited);
        h.write(&self.yielded);
        h.write(&self.stalled);
        h.write_len(self.route.links.len());
        for l in &self.route.links {
            h.write(&l.segment);
            h.write(&(l.dir as u8));
            h.write(&(l.mode as u8));
        }
        h.write(&self.route.cost);
        h.write(&self.leg);
        h.write(&self.offset);
        h.write(&self.link_len);
        h.write(&(self.start.x, self.start.y));
        h.write(&self.step);
        h.write(&self.dest);
        h.write(&self.dest_offset);
        h.write(&self.then);
        h.write(&self.walk.far);
        h.write(&self.walk.jay);
        h.write(&self.walk.far_end);
        h.write(&self.walk.hold);
        h.write(&self.across.is_some());
        if let Some((far, arms)) = &self.across {
            h.write(far);
            h.write_len(arms.len());
            for s in arms {
                h.write(s);
            }
        }
        h.write(&self.departed);
    }
}

impl StateHash for Trips {
    fn write_state(&self, h: &mut StateHasher) {
        h.write(&self.slots);
        h.write(&self.rows);
        h.write(&self.parking);
        h.write(&self.commuters);
    }
}
