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
//! - ほかの軸の腕の横断歩道を渡ろうと待つ人は、段階の軸が青のあいだに渡る番である。ただし、信号の
//!   ない交差点と同じく、来た車を先に通す。青を渡さないのは、その人が
//!   [`PED_WAIT`](super::crossing::PED_WAIT) tick 以上待ったときだけである。
//! - ほかの軸に車が来ていなければ、段階の軸の車と、ほかの軸の腕を渡ろうと待つ人のために、骨組みの黄と
//!   全方向の赤のあいだも段階の軸を青のままにする。来ていない向きのために、車と人を止めない。
//! - どの軸にも車が来ておらず、段階の軸の腕を渡ろうと待つ人だけがいれば、次の軸を青にして、その人を
//!   渡らせる。押しボタンの信号と同じく、人の求めで青を渡す。
//! - 渡る腕の軸に車が来ていなければ、歩く人は骨組みの青の終わり際（[`WALK_CLEAR`]）でも渡り始める。
//!   その軸が青になっても、待たせる車がいないからである。
//! - 止まりきれない車のいる軸は、どの段階でも、その tick は黄にする。青をほかの軸へ移すのは、
//!   ほかの軸に止まりきれない車がいなくなってからにする。どの車も赤では入らない。止まりきれない車が
//!   2 つの軸にいれば、どちらも黄になり、どちらの車も入る（止まりきれない車は譲り合いでも止めない）。
//!
//! 様子は、その tick に車と歩く人を動かす直前の位置だけから作る。信号は状態を持たず、同じ状態からは
//! どこで計算しても同じ灯火になる。車も人もいなければ、固定の周期のとおりに回る。
//! 画面に出す灯火は、移動の主体の表が控えた、直前の tick に効いた様子（[`Lights`]）から求める。
//!
//! 車は、青の腕からだけ交差点に入る。黄と赤の腕の車は、減速の上限の内で止まれるなら停止線で止まり、
//! 止まれないならそのまま入る。CS2 と同じく、黄は「止まれるなら止まる」の合図である。青の腕どうしのあいだは、信号のない交差点と同じ譲り合いの規則で決める（`junction`）。

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

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
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AxisDemand {
    /// その軸の腕に、交差点の手前まで来た車がいる。
    pub cars: bool,
    /// そのうち、止まりきれない車がいる。
    pub committed: bool,
    /// その軸の腕の横断歩道を、渡り始めようと待つ人がいる。
    pub walkers: bool,
    /// そのうち、[`PED_WAIT`](super::crossing::PED_WAIT) tick 以上待った人がいる。
    pub patient: bool,
}

/// tick を進める前の、信号のある交差点ごとの軸の様子。状態には持たず、tick ごとに作り直す。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
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

/// 直前に進めた tick と、その tick の信号を決めた様子。灯火を画面に出すために控える。
/// 読み取り用で、次の tick の動きには効かないので、状態ハッシュには入れない。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Lights {
    pub now: Tick,
    pub demand: Demand,
}

impl Lights {
    /// 交差点 `node` の、直前の tick に効いた信号の段階。
    pub fn state(&self, net: &Network, node: NodeId) -> Option<SignalState> {
        state(net, node, self.now, &self.demand)
    }

    /// 交差点 `node` の腕ごとの、直前の tick に効いた灯火（[`arm_lights`]）。
    pub fn arm_lights(&self, net: &Network, node: NodeId) -> Vec<((i64, i64), Light)> {
        arm_lights(net, node, self.now, &self.demand)
    }
}

/// ある tick の信号の段階。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SignalState {
    /// 軸ごとの灯火。交差点にない軸は赤である。
    pub lights: [Light; 4],
    /// 1 つの腕が 1 周期のうちに青でない tick 数の上限。
    pub red_ticks: u32,
    /// 骨組みの周期で、いまの段階に入ってからの tick 数。
    pub elapsed: u64,
    /// 軸ごとの、交差点の手前まで来た車がいるか。
    pub cars: [bool; 4],
}

impl SignalState {
    /// 軸 `axis` の腕から入る車への灯火。
    pub fn light_for(&self, axis: u8) -> Light {
        self.lights[usize::from(axis)]
    }

    /// 軸 `axis` の腕の横断歩道を、歩く人が渡り始めてよいか。その腕の車が赤で、ほかの軸が青のあいだ、
    /// 骨組みの段階の青が終わる [`WALK_CLEAR`] tick 前までである。その腕の軸に車が来ていなければ、
    /// 終わり際でも渡り始めてよい。
    pub fn walk(&self, axis: u8) -> bool {
        self.light_for(axis) == Light::Red
            && self.lights.contains(&Light::Green)
            && (!self.cars[usize::from(axis)] || self.elapsed + WALK_CLEAR < GREEN)
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
    let d = demand.at(node);
    Some(SignalState {
        lights: actuate(&order, phase, fixed, &d),
        red_ticks: u32::try_from(cycle - GREEN).expect("赤の長さが u32 に収まらない"),
        elapsed: within,
        cars: d.map(|a| a.cars),
    })
}

/// 骨組みの段階の軸 `order[phase]` と灯火 `fixed` から、交差点の様子 `d` に応じて、軸ごとの灯火を決める。
fn actuate(order: &[u8], phase: usize, fixed: Light, d: &[AxisDemand; 4]) -> [Light; 4] {
    let own = order[phase];
    let at = |a: u8| d[usize::from(a)];
    // 車の来ている、次の順の軸。
    let next = (1..order.len())
        .map(|j| order[(phase + j) % order.len()])
        .find(|a| at(*a).cars);
    // 段階の軸でない腕を渡ろうと待つ人。その人は、段階の軸が青のあいだに渡る番である。
    let others_walk = order.iter().any(|a| *a != own && at(*a).walkers);
    let others_patient = order.iter().any(|a| *a != own && at(*a).patient);
    // 止まりきれない車を考えずに決めた、灯火を受ける軸と灯火。
    let (lit, light) = match next {
        // 黄と赤の段階では、車の来ている次の軸へ早めに渡す。
        Some(b) if fixed != Light::Green => (b, Light::Green),
        // 段階の軸に車が来ていなければ、車の来ている次の軸へ渡す。ほかの軸の腕を渡ろうと待つ人は、
        // 信号のない交差点と同じく来た車を先に通し、長く待った人がいるときだけ渡さない。
        Some(b) if !at(own).cars && !others_patient => (b, Light::Green),
        Some(_) => (own, Light::Green),
        // ほかの軸に車が来ておらず、段階の軸の腕を渡ろうと待つ人だけがいれば、ほかの軸を青にして渡らせる。
        None if !at(own).cars && at(own).walkers && !others_walk => {
            (order[(phase + 1) % order.len()], Light::Green)
        }
        // ほかの軸に車が来ていなければ、段階の軸の車と、ほかの軸の腕を渡る人のために、骨組みの黄と赤の
        // あいだも段階の軸を青のままにする。
        None if at(own).cars || others_walk || fixed == Light::Green => (own, Light::Green),
        None => (own, fixed),
    };
    let mut lights = [Light::Red; 4];
    for &a in order {
        if at(a).committed {
            lights[usize::from(a)] = Light::Yellow;
        }
    }
    // ほかの軸に止まりきれない車がいるあいだは、灯火を受ける軸を赤（止まりきれない車がいれば黄）のままにする。
    if !order.iter().any(|a| *a != lit && at(*a).committed) && light != Light::Red {
        lights[usize::from(lit)] = light;
    }
    lights
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
