//! 固定周期の信号。
//!
//! 信号は、交差点の腕を「軸」（向き合う 2 つの向きを 1 つにまとめたもの）ごとに分け、軸を順に
//! 青にする。軸は 0〜3 で、x の向き、右下と左上の斜め、y の向き、左下と右上の斜めの順である。
//! 1 つの軸の段階は、青 `GREEN`、黄 `YELLOW`、全方向が赤 `ALL_RED` の tick 数からなり、
//! 周期は軸の数 × `PHASE` になる。いまの段階は `(tick + ノードごとのずらし) mod 周期` で決まり、
//! 信号は状態を持たない。ずらしはノードのマスから決めるので、ノードの ID を使い回しても変わらない。
//!
//! 車は、青の腕からだけ交差点に入る。黄と赤の腕の車は、減速の上限の内で止まれるなら停止線
//! （ノードの手前）で止まり、止まれないならそのまま入る。CS2 と同じく、黄は「止まれるなら止まる」の
//! 合図である。青の腕どうしのあいだは、信号のない交差点と同じ譲り合いの規則で決める（`junction`）。

use crate::command::NodeId;
use crate::network::{Network, NodeControl};
use crate::units::Tick;

/// 1 つの軸を青にする tick 数。
pub const GREEN: u64 = 20;

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
    /// いま段階にいる軸。
    pub axis: u8,
    /// その軸への灯火。ほかの軸はすべて赤である。
    pub light: Light,
    /// 1 つの腕が 1 周期のうちに青でない tick 数。
    pub red_ticks: u32,
    /// いまの軸の段階に入ってからの tick 数。
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

    /// 軸 `axis` の腕の横断歩道を、歩く人が渡り始めてよいか。その腕の車が赤で、ほかの軸が青に
    /// なってから、青が終わる [`WALK_CLEAR`] tick 前までである。
    pub fn walk(&self, axis: u8) -> bool {
        axis != self.axis && self.light == Light::Green && self.elapsed + WALK_CLEAR < GREEN
    }
}

/// ノード `node` の信号の、tick `now` の段階。信号がないか、3 本以上の区間が集まる交差点でなければ `None`。
pub fn state(net: &Network, node: NodeId, now: Tick) -> Option<SignalState> {
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
    let light = if within < GREEN {
        Light::Green
    } else if within < GREEN + YELLOW {
        Light::Yellow
    } else {
        Light::Red
    };
    Some(SignalState {
        axis: order[phase],
        light,
        red_ticks: u32::try_from(cycle - GREEN).expect("赤の長さが u32 に収まらない"),
        elapsed: within,
    })
}

/// ノード `node` の腕ごとの、tick `now` の灯火。腕はノードから出ていく 1 歩の向きで表し、区間の ID の順に並べる。
/// 信号が働いていなければ空である。
pub fn arm_lights(net: &Network, node: NodeId, now: Tick) -> Vec<((i64, i64), Light)> {
    let (Some(st), Some(n)) = (state(net, node, now), net.node(node)) else {
        return Vec::new();
    };
    n.segments
        .iter()
        .filter_map(|s| arm_step(net, node, *s))
        .map(|step| (step, st.light_for(axis(step))))
        .collect()
}
