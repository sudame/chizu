//! 混み具合から見積もった、車のリンクを通り抜けるまでの遅れ。
//!
//! 車の経路の費用は、自由に走れたときの所要時間に、この遅れを足したものにする（設計 5.3）。
//! 遅れは時間帯（75 tick）ごとに 1 回、そのとき車線に並んでいる車の台数から見積もり、次に見積もるまでの探索はすべて同じ値を読む。
//! 時間帯の途中で書き出した街を読み戻しても同じ経路になるよう、遅れは街の状態に入れる。
//! 信号の待ちの見込みは、ここではなく経路探索のグラフが網の信号から足す（[`super::Graph`]）。

use serde::{Deserialize, Serialize};

use crate::network::LinkId;
use crate::state_hash::{StateHash, StateHasher};

/// 1 つのリンクの遅れ。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Delay {
    pub ms: u64,
    /// 1 回の青でさばききれない列で、リンクの終わりを抜けた台数から測った遅れか。測った遅れは、リンクの先の
    /// 信号で待った時間を含むので、経路の費用に信号の待ちの見込みを重ねて足さない（設計 5.3）。
    pub measured: bool,
}

/// 車のリンクごとの遅れ。リンクの順に並び、遅れのないリンクは載せない。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Delays(Vec<(LinkId, Delay)>);

impl Delays {
    /// リンクの順に並べた `(リンク, 遅れ)` から作る。遅れが 0 のリンクは除く。
    pub fn from_rows(rows: impl IntoIterator<Item = (LinkId, Delay)>) -> Delays {
        let rows: Vec<(LinkId, Delay)> = rows.into_iter().filter(|r| r.1.ms > 0).collect();
        debug_assert!(
            rows.windows(2).all(|w| w[0].0 < w[1].0),
            "リンクの順に並んでいない"
        );
        Delays(rows)
    }

    /// リンクの順に並べた `(リンク, 遅れ（ms）)` から、並んでいる台数だけから見積もった遅れとして作る。
    pub fn from_sorted(rows: impl IntoIterator<Item = (LinkId, u64)>) -> Delays {
        Delays::from_rows(rows.into_iter().map(|(l, ms)| {
            (
                l,
                Delay {
                    ms,
                    measured: false,
                },
            )
        }))
    }

    /// リンク `link` の遅れ（ms）。
    pub fn get(&self, link: LinkId) -> u64 {
        self.delay(link).map_or(0, |d| d.ms)
    }

    /// リンク `link` の遅れ。遅れがなければ `None`。
    pub fn delay(&self, link: LinkId) -> Option<Delay> {
        self.0
            .binary_search_by_key(&link, |r| r.0)
            .ok()
            .map(|i| self.0[i].1)
    }

    pub fn iter(&self) -> impl Iterator<Item = (LinkId, Delay)> + '_ {
        self.0.iter().copied()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl StateHash for Delays {
    fn write_state(&self, h: &mut StateHasher) {
        h.write_len(self.0.len());
        for (l, d) in &self.0 {
            h.write(&l.segment);
            h.write(&(l.dir as u8));
            h.write(&(l.mode as u8));
            h.write(&d.ms);
            h.write(&u8::from(d.measured));
        }
    }
}
