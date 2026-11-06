//! 踏切を置けるかの決まりと、tick ごとの踏切の受け渡し（設計 3.10 の 1 と 3）。
//!
//! 道路の区間の途中のマスを線路が横切るときだけ、踏切にする。`BuildRoad` で線路を横切っても、`BuildRail` で
//! 道路を横切っても、同じ決まりで確かめる。
//! - 踏切のマスに道路のノードを置かない。線路も、端、分岐、曲がりのノードを置かない
//!   （[`DenyReason::NodeOnCrossing`]）。
//! - 高速道路は横切らない（[`DenyReason::HighwayCrossing`]）。
//! - 道路と線路が平行でない。平行なら同じ 1 歩を通るので、ほかの重なりと同じく `Occupied` で断る。
//! - まわりの 8 マスに、ほかの踏切がない（[`DenyReason::CrossingTooClose`]）。
//! - 道路に沿って 2 マス以内に、交差点と曲がり角のノードがない（[`DenyReason::CrossingNearJunction`]）。
//!   道路を引いて既にある踏切の 2 マス以内に交差点か曲がり角ができるときも断る。
//!
//! 線路の側では、踏切のマスの両隣にノードを置く（[`crate::rail::crossing`]）。

use std::collections::BTreeSet;

use smallvec::SmallVec;

use super::{DenyReason, World};
use crate::command::Heading;
use crate::movement::RailCrossings;
use crate::network::Line;
use crate::tile::TilePos;
use crate::units::Tick;

/// 踏切から道路に沿って、交差点と曲がり角のノードを置かない範囲（マス）。
pub const CROSSING_JUNCTION_GAP: i64 = 2;

/// 踏切の決まりで断った理由と、その場所。断る理由を画面や chizu-cli が説明するための読み取り用で、
/// 状態には入らない。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CrossingDenial {
    pub reason: DenyReason,
    /// 決まりに当たった踏切（になるはずだった）マス。
    pub crossing: Option<TilePos>,
    /// [`DenyReason::CrossingNearJunction`] なら近い交差点か曲がり角のマス、
    /// [`DenyReason::CrossingTooClose`] なら近いほかの踏切のマス。
    pub near: Option<TilePos>,
}

impl CrossingDenial {
    fn plain(reason: DenyReason) -> Self {
        Self {
            reason,
            crossing: None,
            near: None,
        }
    }
}

impl From<DenyReason> for CrossingDenial {
    fn from(reason: DenyReason) -> Self {
        Self::plain(reason)
    }
}

fn heading(step: (i64, i64)) -> Heading {
    Heading::of_step(step).expect("1 歩が 8 方向でない")
}

fn opposite(a: Heading, b: Heading) -> bool {
    (a as u8 + 4) % 8 == b as u8
}

fn parallel(a: (i64, i64), b: (i64, i64)) -> bool {
    a == b || a == (-b.0, -b.1)
}

impl World {
    /// 道路の直線 `line` を引いたあとに、マス `t` が道路のノードになるなら、そこから出ていく道路の腕の向き。
    /// `line` がなければ、いまの網で求める。ノードにならなければ `None`。
    fn road_arms_after(&self, t: TilePos, line: Option<&Line>) -> Option<SmallVec<[Heading; 4]>> {
        let net = &self.network;
        let mut arms = SmallVec::<[Heading; 4]>::new();
        let mut node = false;
        if let Some(n) = net.node_at(t) {
            node = true;
            for s in &net.node(n).expect("ノードがない").segments {
                arms.push(net.arm_heading(n, *s).expect("腕の向きがない"));
            }
        } else if let Some(s) = net.segments_through(t).first() {
            let step = net.segment_line(*s).expect("区間がない").step;
            arms.push(heading(step));
            arms.push(heading((-step.0, -step.1)));
        }
        if let Some(l) = line
            && let Some(k) = l.position_of(t)
        {
            let ends = k == 0 || k == l.len;
            if !ends && arms.is_empty() {
                return None;
            }
            // 既存の道路に当たるか、直線の端なら、そこにノードができる。
            node = true;
            if k > 0 {
                arms.push(heading((-l.step.0, -l.step.1)));
            }
            if k < l.len {
                arms.push(heading(l.step));
            }
        }
        node.then_some(arms)
    }

    /// 踏切 `c`（道路の 1 歩の向き `step`）から道路に沿って [`CROSSING_JUNCTION_GAP`] マス以内に、交差点か
    /// 曲がり角のノードがないか。`line` を引いたあとの網で調べる。まっすぐつなぐだけのノードは越えて調べ、
    /// 道路の端で止める。
    fn check_crossing_gap(
        &self,
        c: TilePos,
        step: (i64, i64),
        line: Option<&Line>,
    ) -> Result<(), CrossingDenial> {
        for sign in [1, -1] {
            for d in 1..=CROSSING_JUNCTION_GAP {
                let t = TilePos::new(c.x + sign * d * step.0, c.y + sign * d * step.1);
                match self.road_arms_after(t, line) {
                    Some(arms) => match arms.as_slice() {
                        [_] | [] => break,
                        [a, b] if opposite(*a, *b) => {}
                        _ => {
                            return Err(CrossingDenial {
                                reason: DenyReason::CrossingNearJunction,
                                crossing: Some(c),
                                near: Some(t),
                            });
                        }
                    },
                    None => {
                        let road = self.network.is_road_tile(t)
                            || line.is_some_and(|l| l.position_of(t).is_some());
                        if !road {
                            break;
                        }
                    }
                }
            }
        }
        Ok(())
    }

    /// マス `c` のまわりの 8 マスにある、既にある踏切か `fresh` の踏切。複数あれば走査の順で最初のもの。
    fn crossing_nearby(&self, c: TilePos, fresh: &[TilePos]) -> Option<TilePos> {
        (-1..=1).find_map(|dx| {
            (-1..=1).find_map(|dy| {
                let t = TilePos::new(c.x + dx, c.y + dy);
                ((dx, dy) != (0, 0) && (self.rail.is_crossing(t) || fresh.contains(&t)))
                    .then_some(t)
            })
        })
    }

    /// `command` が踏切の決まりで断られるなら、その理由と場所。踏切の決まりに関わらないコマンドと、
    /// 断られないコマンドは `None`。状態を変えない。
    /// 断られたあとの街で呼ぶと、断った決まりに当たったマスが分かる。
    pub fn crossing_denial(&self, command: &crate::command::Command) -> Option<CrossingDenial> {
        use crate::command::Command;
        match command {
            Command::BuildRoad { from, to, .. } => {
                let line = Line::between(*from, *to).ok()?;
                self.check_road_crossings_detailed(&line).err()
            }
            Command::BuildRail { from, to } => {
                let line = Line::between(*from, *to).ok()?;
                self.check_rail_crossings_detailed(&line).err()
            }
            _ => None,
        }
    }

    /// 線路の直線 `line` が道路を横切る所を確かめ、踏切の両隣としてノードを置く歩数を返す。
    /// 道路のマスはまだ確かめていない前提で、踏切にできない横切り方なら理由を返す。
    pub(crate) fn check_rail_crossings(&self, line: &Line) -> Result<BTreeSet<u64>, DenyReason> {
        self.check_rail_crossings_detailed(line)
            .map_err(|d| d.reason)
    }

    /// [`Self::check_rail_crossings`] で、断る場所も返すもの。
    fn check_rail_crossings_detailed(&self, line: &Line) -> Result<BTreeSet<u64>, CrossingDenial> {
        let ks: Vec<u64> = (0..=line.len)
            .filter(|k| self.network.is_road_tile(line.tile(*k)))
            .collect();
        let tiles: Vec<TilePos> = ks.iter().map(|k| line.tile(*k)).collect();
        let mut extra = BTreeSet::new();
        for (&k, &c) in ks.iter().zip(&tiles) {
            let seg = self.network.segments_through(c)[0];
            let step = self.network.segment_line(seg).expect("区間がない").step;
            if parallel(step, line.step) {
                return Err(DenyReason::Occupied.into());
            }
            // 既にある踏切のマスを通ると、そこに線路のノードができる。
            if k == 0
                || k == line.len
                || self.network.node_at(c).is_some()
                || self.rail.is_crossing(c)
            {
                return Err(DenyReason::NodeOnCrossing.into());
            }
            let s = self.network.segment(seg).expect("区間がない");
            if s.kind == crate::network::SegmentKind::Highway {
                return Err(DenyReason::HighwayCrossing.into());
            }
            let others: Vec<TilePos> = tiles.iter().copied().filter(|t| *t != c).collect();
            if let Some(near) = self.crossing_nearby(c, &others) {
                return Err(CrossingDenial {
                    reason: DenyReason::CrossingTooClose,
                    crossing: Some(c),
                    near: Some(near),
                });
            }
            self.check_crossing_gap(c, step, None)?;
            extra.insert(k - 1);
            extra.insert(k + 1);
        }
        Ok(extra)
    }

    /// 道路の直線 `line` が線路を横切る所を確かめ、踏切になるマスを返す。道路を引いて、既にある踏切の近くに
    /// 交差点か曲がり角ができないことも確かめる。
    pub(crate) fn check_road_crossings(&self, line: &Line) -> Result<Vec<TilePos>, DenyReason> {
        self.check_road_crossings_detailed(line)
            .map_err(|d| d.reason)
    }

    /// [`Self::check_road_crossings`] で、断る場所も返すもの。
    fn check_road_crossings_detailed(&self, line: &Line) -> Result<Vec<TilePos>, CrossingDenial> {
        let ks: Vec<u64> = (0..=line.len)
            .filter(|k| self.rail.is_rail_tile(line.tile(*k)))
            .collect();
        let tiles: Vec<TilePos> = ks.iter().map(|k| line.tile(*k)).collect();
        let mut fresh = Vec::new();
        for (&k, &c) in ks.iter().zip(&tiles) {
            let step = self.rail.rail_step_at(c).expect("線路がない");
            if parallel(step, line.step) {
                return Err(DenyReason::Occupied.into());
            }
            if k == 0 || k == line.len || self.rail.is_crossing(c) || !self.rail.crossing_fits(c) {
                return Err(DenyReason::NodeOnCrossing.into());
            }
            let others: Vec<TilePos> = tiles.iter().copied().filter(|t| *t != c).collect();
            if let Some(near) = self.crossing_nearby(c, &others) {
                return Err(CrossingDenial {
                    reason: DenyReason::CrossingTooClose,
                    crossing: Some(c),
                    near: Some(near),
                });
            }
            self.check_crossing_gap(c, line.step, Some(line))?;
            fresh.push(c);
        }
        // 道路のノードができるマス（直線の端と、既にある道路に当たるマス）の近くにある、既にある踏切。
        let cuts = (0..=line.len)
            .filter(|k| *k == 0 || *k == line.len || self.network.is_road_tile(line.tile(*k)))
            .map(|k| line.tile(k));
        let gap = CROSSING_JUNCTION_GAP;
        let mut near = BTreeSet::new();
        for u in cuts {
            for dx in -gap..=gap {
                for dy in -gap..=gap {
                    let t = TilePos::new(u.x + dx, u.y + dy);
                    if self.rail.is_crossing(t) {
                        near.insert(t);
                    }
                }
            }
        }
        for c in near {
            let Some(seg) = self.network.segments_through(c).first().copied() else {
                continue;
            };
            let step = self.network.segment_line(seg).expect("区間がない").step;
            self.check_crossing_gap(c, step, Some(line))?;
        }
        Ok(fresh)
    }

    /// マス `tiles` が踏切か（道路と線路の両方が通るか）を、線路の網の踏切の一覧に映す。
    pub(super) fn sync_crossings(&mut self, tiles: &[TilePos]) {
        for t in tiles {
            let on = self.network.is_road_tile(*t) && self.rail.is_rail_tile(*t);
            self.rail.set_crossing(*t, on);
        }
    }

    /// tick の頭に閉じている踏切と、道路の側から見た踏切。踏切がなければ `None`。
    /// 踏切が閉じているかは、tick の頭の列車の位置だけで決める。
    pub(super) fn crossings_at_tick_start(&self) -> Option<(BTreeSet<TilePos>, RailCrossings)> {
        if self.rail.crossing_count() == 0 {
            return None;
        }
        let closed = self.rail.closed_crossings();
        let rail = RailCrossings::new(&self.network, self.rail.crossings(), &closed);
        Some((closed, rail))
    }

    /// 車と歩く人を進めたあとに、列車を 1 tick 進め、踏切を数える。中に車か人がいる踏切の手前では、
    /// 止まりきれる列車が止まる。
    pub(super) fn step_trains(
        &mut self,
        tick: Tick,
        crossings: Option<(BTreeSet<TilePos>, RailCrossings)>,
    ) {
        let Some((closed, rail)) = crossings else {
            self.rail.step(tick);
            return;
        };
        let (occupied, waits) = rail.traffic(&self.trips);
        self.rail.mark_closed(tick.0, &closed);
        self.rail.step_with(tick, &occupied);
        self.rail.note_crossings(&occupied, waits);
    }
}
