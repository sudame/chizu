//! 交差点で、どの車をノードに入れるかを決める。
//!
//! 3 本以上の区間が集まるノードを交差点とする。交差点の手前まで来た車線の先頭の車（候補）は、
//! 入ってくるリンクから出ていくリンクへの「動き」と、出ていくリンクで入る車線を持つ。入ってくるリンクの
//! 違う 2 つの動きは、出ていく車線が同じ（合流）か、交差点の中で道筋が交わるときに衝突する。
//! 同じリンクへ入っても車線が違えば合流しないので、左と右から曲がって入る車は同時に入れる（`car`）。
//!
//! ノードごとに、交差点の中にいる車（出ていくリンクに入ったばかりの車）の動きを先に置く。
//! そのあと候補を優先の順に 1 台ずつ見て、置いた動きのどれとも衝突せず、出ていく車線に
//! 交差点を抜けきるだけの空きがあれば入れる。入れない候補はノードの手前で止める。
//! 止めた候補も、出ていく車線に空きがあれば動きを置く。こうすると、先の順番の車が交差点の中の
//! 車を待つあいだに、後の順番の車が割り込んで、先の車をいつまでも待たせることがない。
//! 優先の順は次のとおりで、前の項目ほど強い。
//!
//! 1. 止まりきれない車。ノードの手前で止めると減速の上限を破るので、衝突していても入れる。
//! 2. `PATIENCE` tick 以上待った車。格の高い道路の車が途切れなくても、いつかは入れる。
//! 3. 格の高い道路から来た車。
//! 4. 直進と、対向の車線をまたがない曲がり（左折）。対向の車線をまたぐ曲がり（右折）と
//!    折り返しは後にする。
//! 5. 長く待った車。
//! 6. 車線の番号の小さい車。
//!
//! この順は全順序なので、止まりきれない車を除けば、いちばん先の候補は交差点の中の車と
//! 衝突しないかぎり必ず入れる。交差点の中の車はいずれ抜けるので、互いに譲り合って
//! 誰も動かない行き詰まりは起きない。
//!
//! 交差点の中の車が抜けきれるように、出ていく車線の最後尾までに、車間と交差点を抜ける距離を
//! 足した空きがなければ、ノードをまたがせない（`car`）。車はその空きの足りない所を前の車の
//! 代わりとみなして、減速の上限の内で手前に止まる。入った車は、前の車が止まっていても
//! 交差点を抜けきれる。
//!
//! それでも交差点の中で止まり続ける車は出うる。出ていくリンクが空きの距離より短く、その先の
//! 交差点で待たされるときと、止まりきれずに空きの足りない車線へ入ったときである。
//! そのときは、`GIVE_UP` tick より長く待った候補がその車を無視して入る。
//!
//! 出ていく車線が詰まった交差点がつながって輪になると、どの車も動けなくなる（グリッドロック）。
//! これは譲り合いでなく車線の空きの不足で起きるので、`RESCUE` tick より長く位置の変わらない車を
//! 行き先に着いたことにして、輪をほどく。輪は交差点でないノードや行き止まりの折り返しも通るので、
//! 救済は交差点の手前の車に限らず、車線の上のどの車にも効かせる（`car`）。
//!
//! 道筋が交わるかは、ノードを中心とした円の上に、腕ごとの入口と出口の点を置いて決める。
//! 左側通行とし、マスの y は画面の下向きなので、腕の向きの角度（x から y へ回る向き）から
//! 入口は少し進めた所、出口は少し戻した所に置く。2 つの動きの弦の端が円の上で互い違いに
//! 並べば、道筋が交わる。
//!
//! 入ってくる腕か出ていく腕の横断歩道を人が渡っているか、長く待った人が渡ろうとしている候補は、
//! 止まりきれない車を除いて止める（`crossing`）。そのうち、横断歩道の人がいなければ入れた候補は、
//! 人に譲った候補として返し、人に譲った tick 数を数えさせる。
//!
//! 信号（`signal`）のある交差点では、先に黄と赤の腕から来た候補を、止まりきれない車を除いて
//! ノードの手前で止め、残りの候補をこの順で決める。止めた候補は動きを置かないので、青の腕の車を
//! 待たせない。信号で止めたあいだも、待った tick 数に数える。交差点の中で止まった車をすり抜ける
//! `GIVE_UP` が、赤の待ちを含めて数えないと、軸の多い交差点で `RESCUE` より遅れるためである。
//! 一方で 2. の順番は、赤の待ちだけで取れないよう、`PATIENCE` に腕が 1 周期に赤である tick 数を
//! 足した長さで決める。赤を 1 回待っただけの車が、同じ青で来た直進より先に右折しないようにする。

use core::cmp::Reverse;
use serde::{Deserialize, Serialize};

use super::signal::{self, Light};
use crate::command::{NodeId, RoadKind};
use std::collections::BTreeSet;

use crate::network::{LaneId, LinkId, Network, SegmentId, SegmentKind};
use crate::units::Tick;

/// この tick 数より長く待った車は、道路の格に関わらず先に入れる。
pub(super) const PATIENCE: u32 = 30;

/// この tick 数より長く待った車は、交差点の中で止まった車を待たずに入る。
pub(super) const GIVE_UP: u32 = 2 * PATIENCE;

/// この tick 数より長く車線の上で位置の変わらない車は、詰まりから抜け出せないものとして、
/// 行き先に着いたことにする。CS2 で、長く詰まった車が消えるのにならう。
pub(super) const RESCUE: u32 = 300;

/// ノード `n` が、車どうしを譲らせる交差点か。
pub(super) fn is_junction(net: &Network, n: NodeId) -> bool {
    net.node(n).is_some_and(|x| x.segments.len() >= 3)
}

/// 1 歩の向きの角度。x の正の向きを 0 とし、y の正の向きへ 45 度ごとに 1 ずつ増える。
fn heading(step: (i8, i8)) -> u8 {
    match step {
        (1, 0) => 0,
        (1, 1) => 1,
        (0, 1) => 2,
        (-1, 1) => 3,
        (-1, 0) => 4,
        (-1, -1) => 5,
        (0, -1) => 6,
        (1, -1) => 7,
        _ => panic!("8 方向でない向き {step:?}"),
    }
}

/// 道路の格。大きいほど優先する。
pub(super) fn class(kind: SegmentKind) -> u8 {
    match kind {
        SegmentKind::Road(RoadKind::Small) => 1,
        SegmentKind::Road(RoadKind::Medium) => 2,
        SegmentKind::Road(RoadKind::Large) => 3,
        SegmentKind::Highway => 4,
    }
}

/// 交差点の中を、入ってくるリンクから出ていくリンクへ進む動き。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Move {
    from: LinkId,
    to: LinkId,
    /// ノードから見た、入ってくる腕と出ていく腕の向きの角度。
    arm_in: u8,
    arm_out: u8,
}

impl Move {
    pub(super) fn new(net: &Network, from: LinkId, to: LinkId) -> Move {
        let step = |l| super::pos::link_geometry(net, l).expect("リンクがない").1;
        let (f, t) = (step(from), step(to));
        Move {
            from,
            to,
            arm_in: heading((-f.0, -f.1)),
            arm_out: heading(t),
        }
    }

    /// 円の上の入口と出口の位置。1 周を 32 に分ける。
    fn ends(&self) -> (u8, u8) {
        (self.arm_in * 4 + 1, (self.arm_out * 4 + 31) % 32)
    }

    /// 入ってくる腕から見た、出ていく腕の向き。1〜3 は左折、4 は直進、5〜7 は右折、0 は折り返しである。
    pub(super) fn bend(&self) -> u8 {
        (self.arm_out + 8 - self.arm_in) % 8
    }

    /// 曲がり方の順位。直進と左折は 0、右折は 1、折り返しは 2 である。
    fn turn(&self) -> u8 {
        match self.bend() {
            0 => 2,
            5..=7 => 1,
            _ => 0,
        }
    }

    fn crosses(&self, other: &Move) -> bool {
        let (p, q) = self.ends();
        let (a, b) = other.ends();
        let inside = |x: u8| (x + 32 - p) % 32 < (q + 32 - p) % 32;
        inside(a) != inside(b)
    }

    /// 車線を問わずに見た衝突。同じリンクへ入れば合流とみなす。
    #[cfg(test)]
    pub(super) fn conflicts(&self, other: &Move) -> bool {
        self.from != other.from && (self.to == other.to || self.crosses(other))
    }

    /// 出ていくリンクの車線 `lane` に入るこの動きが、車線 `other_lane` に入る `other` と衝突するか。
    /// 同じリンクへ入っても、車線が違えば合流しない。
    fn conflicts_in(&self, lane: LaneId, other: &Move, other_lane: LaneId) -> bool {
        self.from != other.from
            && ((self.to == other.to && lane == other_lane) || self.crosses(other))
    }
}

/// 交差点の手前まで来た、車線の先頭の車。
#[derive(Clone, Copy, Debug)]
pub(super) struct Approach {
    /// 車を見分ける番号。呼ぶ側で決め、止める候補を返すときに使う。
    pub id: usize,
    pub node: NodeId,
    /// 入ってくるリンクと、出ていくリンク。
    pub from: LinkId,
    pub to: LinkId,
    pub lane: LaneId,
    /// 出ていくリンクで入る車線。
    pub to_lane: LaneId,
    /// 入ってくるリンクの道路の格。
    pub class: u8,
    /// ノードの手前で止めると、減速の上限を破るか。
    pub committed: bool,
    /// 出ていく車線に、交差点を抜けきるだけの空きがありそうか。
    pub room: bool,
    /// ノードの手前で待った tick 数。
    pub waited: u32,
}

/// 交差点の中にいる車。
#[derive(Clone, Copy, Debug)]
pub(super) struct Inside {
    pub node: NodeId,
    /// 入ってきたリンクと、いまのリンク。
    pub from: LinkId,
    pub to: LinkId,
    /// いまの車線。
    pub lane: LaneId,
    /// 止まっているか。
    pub stopped: bool,
}

/// 交差点で止める候補。
#[derive(Debug, Default, PartialEq, Eq)]
pub(super) struct Held {
    /// ノードの手前で止める候補の `id`。小さい順に並ぶ。
    pub ids: Vec<usize>,
    /// 止める候補のうち、横断歩道の人がいなければ入れた候補の `id`。小さい順に並ぶ。
    pub yielded: Vec<usize>,
    /// 交差点の中で止まった車を無視して入れた候補の数。
    pub passed: u64,
}

/// 交差点ごとに候補を優先の順に見て、tick `now` にノードの手前で止める候補を決める。
/// `walks` は人のいる横断歩道と、長く待った人のいる横断歩道（交差点と腕の区間）で、入ってくる腕か
/// 出ていく腕がそこにかかる候補は止める。
pub(super) fn holds(
    net: &Network,
    mut cands: Vec<Approach>,
    mut inside: Vec<Inside>,
    now: Tick,
    walks: &BTreeSet<(NodeId, SegmentId)>,
    signals: &signal::Demand,
) -> Held {
    let mut passed = 0_u64;
    cands.sort_unstable_by_key(|c| (c.node, c.id));
    // 同じノードの中の並びは結果に効かない。
    inside.sort_unstable_by_key(|x| x.node);
    let mut held = Vec::new();
    let mut yielded = Vec::new();
    let mut group: Vec<(Approach, Move)> = Vec::new();
    // 置いた動きと、その車線と、それが交差点の中で止まった車のものか。
    let mut placed: Vec<(Move, LaneId, bool)> = Vec::new();
    for same in cands.chunk_by(|a, b| a.node == b.node) {
        let node = same[0].node;
        let lo = inside.partition_point(|x| x.node < node);
        let hi = inside.partition_point(|x| x.node <= node);
        group.clear();
        group.extend(same.iter().map(|c| (*c, Move::new(net, c.from, c.to))));
        let mut patience = PATIENCE;
        if let Some(st) = signal::state(net, node, now, signals) {
            patience = patience.saturating_add(st.red_ticks);
            // 青でない腕の候補は、止まりきれるかぎり停止線で止め、動きも置かない。
            group.retain(|(c, mv)| {
                let stop = !c.committed && st.light_for(mv.arm_in % 4) != Light::Green;
                if stop {
                    held.push(c.id);
                }
                !stop
            });
        }
        // 入ってくる腕か出ていく腕の横断歩道に人がいる。
        let peds = |c: &Approach| {
            !walks.is_empty()
                && (walks.contains(&(node, c.from.segment))
                    || walks.contains(&(node, c.to.segment)))
        };
        if group.len() <= 1 && lo == hi {
            // ほかに車がいなければ、先の車線の空きと横断歩道の人だけで決まる。
            if let Some((c, _)) = group.first()
                && !(c.committed || (c.room && !peds(c)))
            {
                held.push(c.id);
                if c.room {
                    yielded.push(c.id);
                }
            }
            continue;
        }
        group.sort_unstable_by_key(|(c, mv)| {
            (
                !c.committed,
                c.waited < patience,
                Reverse(c.class),
                mv.turn(),
                Reverse(c.waited),
                c.lane,
            )
        });
        placed.clear();
        placed.extend(
            inside[lo..hi]
                .iter()
                .map(|x| (Move::new(net, x.from, x.to), x.lane, x.stopped)),
        );
        for (c, mv) in &group {
            let blocked = |skip_stopped: bool| {
                placed
                    .iter()
                    .filter(|(_, _, stopped)| !(skip_stopped && *stopped))
                    .any(|(m, lane, _)| m.conflicts_in(*lane, mv, c.to_lane))
            };
            // 待ちすぎた車は、交差点の中で止まった車の脇をすり抜けて入る。
            let clear = c.room && !blocked(c.waited >= GIVE_UP);
            let free = clear && !peds(c);
            if !(c.committed || free) {
                held.push(c.id);
                if clear {
                    yielded.push(c.id);
                }
            } else if !c.committed && blocked(false) {
                passed = passed.saturating_add(1);
            }
            // 待たせた候補も、先の車線に空きがあれば動きを押さえ、後の候補に割り込ませない。
            if c.committed || c.room {
                placed.push((*mv, c.to_lane, false));
            }
        }
    }
    held.sort_unstable();
    yielded.sort_unstable();
    Held {
        ids: held,
        yielded,
        passed,
    }
}

/// 詰まりをほどく救済が働いた回数。状態には入れず、状態ハッシュにも含めない。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rescues {
    /// `GIVE_UP` を越えて待った車が、交差点の中で止まった車を無視して入った回数。
    pub passed_stuck: u64,
    /// `RESCUE` を越えて止まったままの車を、行き先に着いたことにした回数。
    pub teleported: u64,
    /// 車線に出られないまま `RESCUE` を越えて待った車を、出発の建物から歩く移動に切り替えた回数。
    pub walked_instead: u64,
}
