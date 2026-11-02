//! 信号のない交差点で、どの車をノードに入れるかを決める。
//!
//! 3 本以上の区間が集まるノードを交差点とする。交差点の手前まで来た車線の先頭の車（候補）は、
//! 入ってくるリンクから出ていくリンクへの「動き」を持つ。入ってくるリンクの違う 2 つの動きは、
//! 出ていくリンクが同じ（合流）か、交差点の中で道筋が交わるときに衝突する。
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
//! 信号を入れるときは、いまの段階で赤の動きを候補から外し、残りをこの順で決める形にする。

use core::cmp::Reverse;

use crate::command::{NodeId, RoadKind};
use crate::network::{LaneId, LinkId, Network, SegmentKind};

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

    /// 曲がり方の順位。直進と左折は 0、右折は 1、折り返しは 2 である。
    fn turn(&self) -> u8 {
        match (self.arm_out + 8 - self.arm_in) % 8 {
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

    pub(super) fn conflicts(&self, other: &Move) -> bool {
        self.from != other.from && (self.to == other.to || self.crosses(other))
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
    /// 止まっているか。
    pub stopped: bool,
}

/// 交差点ごとに候補を優先の順に見て、ノードの手前で止める候補の `id` を小さい順に返す。
/// あわせて、交差点の中で止まった車を無視して入れた候補の数を返す。
pub(super) fn holds(
    net: &Network,
    mut cands: Vec<Approach>,
    mut inside: Vec<Inside>,
) -> (Vec<usize>, u64) {
    let mut passed = 0_u64;
    cands.sort_unstable_by_key(|c| (c.node, c.id));
    // 同じノードの中の並びは結果に効かない。
    inside.sort_unstable_by_key(|x| x.node);
    let mut held = Vec::new();
    let mut group: Vec<(Approach, Move)> = Vec::new();
    // 置いた動きと、それが交差点の中で止まった車のものか。
    let mut placed: Vec<(Move, bool)> = Vec::new();
    for same in cands.chunk_by(|a, b| a.node == b.node) {
        let node = same[0].node;
        let lo = inside.partition_point(|x| x.node < node);
        let hi = inside.partition_point(|x| x.node <= node);
        if same.len() == 1 && lo == hi {
            // ほかに車がいなければ、先の車線の空きだけで決まる。
            let c = &same[0];
            if !(c.committed || c.room) {
                held.push(c.id);
            }
            continue;
        }
        group.clear();
        group.extend(same.iter().map(|c| (*c, Move::new(net, c.from, c.to))));
        group.sort_unstable_by_key(|(c, mv)| {
            (
                !c.committed,
                c.waited < PATIENCE,
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
                .map(|x| (Move::new(net, x.from, x.to), x.stopped)),
        );
        for (c, mv) in &group {
            let blocked = |skip_stopped: bool| {
                placed
                    .iter()
                    .filter(|(_, stopped)| !(skip_stopped && *stopped))
                    .any(|(m, _)| m.conflicts(mv))
            };
            // 待ちすぎた車は、交差点の中で止まった車の脇をすり抜けて入る。
            let free = c.room && !blocked(c.waited >= GIVE_UP);
            if !(c.committed || free) {
                held.push(c.id);
            } else if !c.committed && blocked(false) {
                passed = passed.saturating_add(1);
            }
            // 待たせた候補も、先の車線に空きがあれば動きを押さえ、後の候補に割り込ませない。
            if c.committed || c.room {
                placed.push((*mv, false));
            }
        }
    }
    held.sort_unstable();
    (held, passed)
}

/// 詰まりをほどく救済が働いた回数。状態には入れず、状態ハッシュにも含めない。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Rescues {
    /// `GIVE_UP` を越えて待った車が、交差点の中で止まった車を無視して入った回数。
    pub passed_stuck: u64,
    /// `RESCUE` を越えて止まったままの車を、行き先に着いたことにした回数。
    pub teleported: u64,
    /// 車線に出られないまま `RESCUE` を越えて待った車を、出発の建物から歩く移動に切り替えた回数。
    pub walked_instead: u64,
}
