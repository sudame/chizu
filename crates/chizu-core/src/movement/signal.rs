//! 感応式の信号。
//!
//! 信号は、交差点の腕を「軸」（向き合う 2 つの向きを 1 つにまとめたもの）ごとに分け、軸を順に
//! 青にする。軸は 0〜3 で、x の向き、右下と左上の斜め、y の向き、左下と右上の斜めの順である。
//!
//! 骨組みは固定の周期である。1 つの軸の段階は、青 `GREEN`、黄 `YELLOW`、全方向が赤 `ALL_RED` の
//! tick 数からなり、周期は軸の数 × `PHASE` になる。いまどの軸の段階にいるかは
//! `(tick + ノードごとのずらし) mod 周期` で決まる。ずらしはノードのマスから決めるので、ノードの ID を
//! 使い回しても変わらない。
//!
//! そのうえで、その tick の交差点のまわりの様子（[`Demand`]）から、青を渡す先を決める。
//! - 段階の軸に、交差点の手前まで来た車がいなければ、車の来ている次の軸に青を渡す。
//!   空いた軸のために、ほかの軸の車を待たせない。段階の軸の青が終わる前に、車が来なくなった時点で
//!   次へ渡すので、`GREEN` は青の上限になる。
//! - ただし、ほかの軸の腕の横断歩道を渡ろうと待つ人がいれば、段階の軸の青を渡さない。その人は、
//!   段階の軸が青のあいだに渡る番だからである。
//! - 青を段階の軸へ戻すときに、渡していた軸に止まりきれない車がいれば、その tick はその軸を黄にする。
//!   黄と赤の段階でも、段階の軸に止まりきれない車がいれば黄のままにする。どの車も赤では入らない。
//!
//! 様子は、tick を進める前の車と歩く人の位置だけから作る。信号は状態を持たず、同じ状態からは
//! どこで計算しても同じ灯火になる。車も人もいなければ、固定の周期のとおりに回る。
//!
//! 車は、青の腕からだけ交差点に入る。黄と赤の腕の車は、減速の上限の内で止まれるなら停止線
//! （ノードの手前）で止まり、止まれないならそのまま入る。CS2 と同じく、黄は「止まれるなら止まる」の
//! 合図である。青の腕どうしのあいだは、信号のない交差点と同じ譲り合いの規則で決める（`junction`）。

use std::collections::BTreeMap;

use crate::command::NodeId;
use crate::network::{Network, NodeControl};
use crate::units::Tick;

/// 1 つの軸を青にする tick 数の上限。
pub const GREEN: u64 = 30;

/// 青のあとの黄の tick 数。
pub const YELLOW: u64 = 3;

/// 黄のあと、次の軸を青にするまでの、どの腕も赤の tick 数。交差点の中の車が抜ける間にする。
pub const ALL_RED: u64 = 2;

/// 1 つの軸の段階の長さ。
pub const PHASE: u64 = GREEN + YELLOW + ALL_RED;

/// 歩く人は、渡る腕と交わる軸の青が終わるこの tick 数前からは、新しく渡り始めない。渡っている人が
/// 黄と全方向の赤のうちに渡り終えるようにする。
pub const WALK_CLEAR: u64 = 8;

/// 腕から交差点に入る車への灯火。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Light {
    Green = 0,
    Yellow = 1,
    Red = 2,
}

/// 信号のある交差点の、軸ごとの様子。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AxisDemand {
    /// その軸の腕に、交差点の手前まで来た車がいる。
    pub cars: bool,
    /// そのうち、止まりきれない車がいる。
    pub committed: bool,
    /// その軸の腕の横断歩道を、渡り始めようと待つ人がいる。
    pub walkers: bool,
}

/// tick を進める前の、信号のある交差点ごとの軸の様子。状態には持たず、tick ごとに作り直す。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Demand(BTreeMap<NodeId, [AxisDemand; 4]>);

impl Demand {
    /// 交差点 `node` の軸 `axis` の様子を書き換える。
    pub fn note(&mut self, node: NodeId, axis: u8, f: impl FnOnce(&mut AxisDemand)) {
        f(&mut self.0.entry(node).or_default()[usize::from(axis)]);
    }

    fn at(&self, node: NodeId) -> [AxisDemand; 4] {
        self.0.get(&node).copied().unwrap_or_default()
    }
}

/// 1 歩の向きの軸。
pub fn axis(step: (i64, i64)) -> u8 {
    match step {
        (1, 0) | (-1, 0) => 0,
        (1, 1) | (-1, -1) => 1,
        (0, 1) | (0, -1) => 2,
        (-1, 1) | (1, -1) => 3,
        _ => panic!("8 方向でない向き {step:?}"),
    }
}

/// ノード `node` から区間 `segment` へ出ていく 1 歩の向き。
fn arm_step(net: &Network, node: NodeId, segment: crate::network::SegmentId) -> Option<(i64, i64)> {
    let s = net.segment(segment)?;
    let line = net.segment_line(segment)?;
    Some(if s.a == node {
        line.step
    } else {
        (-line.step.0, -line.step.1)
    })
}

/// ある tick の信号の段階。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SignalState {
    /// いま灯火を受ける軸。
    pub axis: u8,
    /// その軸への灯火。ほかの軸はすべて赤である。
    pub light: Light,
    /// 1 つの腕が 1 周期のうちに青でない tick 数の上限。
    pub red_ticks: u32,
    /// 骨組みの周期で、いまの段階に入ってからの tick 数。
    pub elapsed: u64,
}

impl SignalState {
    /// 軸 `axis` の腕から入る車への灯火。
    pub fn light_for(&self, axis: u8) -> Light {
        if axis == self.axis {
            self.light
        } else {
            Light::Red
        }
    }

    /// 軸 `axis` の腕の横断歩道を、歩く人が渡り始めてよいか。その腕の車が赤で、ほかの軸が青のあいだ、
    /// 骨組みの段階の青が終わる [`WALK_CLEAR`] tick 前までである。
    pub fn walk(&self, axis: u8) -> bool {
        axis != self.axis && self.light == Light::Green && self.elapsed + WALK_CLEAR < GREEN
    }
}

/// ノード `node` の信号の、tick `now` の段階。`demand` は tick `now` を進める前の様子である。
/// 信号がないか、3 本以上の区間が集まる交差点でなければ `None`。
pub fn state(net: &Network, node: NodeId, now: Tick, demand: &Demand) -> Option<SignalState> {
    let n = net.node(node)?;
    if n.control != NodeControl::Signal || n.segments.len() < 3 {
        return None;
    }
    let mut axes = [false; 4];
    for s in &n.segments {
        axes[usize::from(axis(arm_step(net, node, *s)?))] = true;
    }
    let order: Vec<u8> = (0..4_u8).filter(|a| axes[usize::from(*a)]).collect();
    let count = u64::try_from(order.len()).expect("軸の数が u64 に収まらない");
    let cycle = count * PHASE;
    let tile = n.tile();
    let cycle_i = i64::try_from(cycle).expect("周期が i64 に収まらない");
    // マスの座標から決めるずらし。隣り合う交差点で、段階がそろいすぎないようにする。
    let shift = u64::try_from(
        (tile.x.rem_euclid(cycle_i) * 7 + tile.y.rem_euclid(cycle_i) * 13).rem_euclid(cycle_i),
    )
    .expect("ずらしが負になった");
    // 区間を足したり消したりして軸の数が変わると、周期も変わり、段階が飛ぶ。黄と全方向の赤を経ずに
    // 別の軸が青になることがあるが、交差点の中の車とは譲り合いで衝突しないので、そのままにする。
    let t = (now.0 % cycle + shift) % cycle;
    let phase = usize::try_from(t / PHASE).expect("段階の番号が usize に収まらない");
    let within = t % PHASE;
    let fixed = if within < GREEN {
        Light::Green
    } else if within < GREEN + YELLOW {
        Light::Yellow
    } else {
        Light::Red
    };
    let (axis, light) = actuate(&order, phase, fixed, &demand.at(node));
    Some(SignalState {
        axis,
        light,
        red_ticks: u32::try_from(cycle - GREEN).expect("赤の長さが u32 に収まらない"),
        elapsed: within,
    })
}

/// 骨組みの段階の軸 `order[phase]` と灯火 `fixed` から、交差点の様子 `d` に応じて、灯火を受ける軸と灯火を決める。
fn actuate(order: &[u8], phase: usize, fixed: Light, d: &[AxisDemand; 4]) -> (u8, Light) {
    let own = order[phase];
    let at = |a: u8| d[usize::from(a)];
    // 車の来ている、次の順の軸。
    let next = (1..order.len())
        .map(|j| order[(phase + j) % order.len()])
        .find(|a| at(*a).cars);
    if fixed != Light::Green {
        // 黄と赤の段階では、止まりきれない車のいる軸だけを黄で通し、それ以外は次の軸へ早めに渡す。
        if at(own).committed {
            return (own, Light::Yellow);
        }
        return next.map_or((own, fixed), |b| (b, Light::Green));
    }
    let wanted = at(own).cars || order.iter().any(|a| *a != own && at(*a).walkers);
    if !wanted {
        return next.map_or((own, Light::Green), |b| (b, Light::Green));
    }
    // 渡していた軸に止まりきれない車がいれば、抜けるまで黄にする。段階の軸にも止まりきれない車が
    // いれば、段階の軸を青のままにする。
    if !at(own).committed
        && let Some(b) = order.iter().copied().find(|a| *a != own && at(*a).committed)
    {
        return (b, Light::Yellow);
    }
    (own, Light::Green)
}

/// ノード `node` の腕ごとの、tick `now` の灯火。`demand` は tick `now` を進める前の様子である。
/// 腕はノードから出ていく 1 歩の向きで表し、区間の ID の順に並べる。信号が働いていなければ空である。
pub fn arm_lights(
    net: &Network,
    node: NodeId,
    now: Tick,
    demand: &Demand,
) -> Vec<((i64, i64), Light)> {
    let (Some(st), Some(n)) = (state(net, node, now, demand), net.node(node)) else {
        return Vec::new();
    };
    n.segments
        .iter()
        .filter_map(|s| arm_step(net, node, *s))
        .map(|step| (step, st.light_for(axis(step))))
        .collect()
}
