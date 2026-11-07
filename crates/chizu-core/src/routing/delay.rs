//! 混み具合から見積もった、車のリンクを通り抜けるまでの遅れ。
//!
//! 車の経路の費用は、自由に走れたときの所要時間に、この遅れを足したものにする（設計 5.3）。
//! 遅れは時間帯（75 tick）ごとに 1 回、そのとき車線に並んでいる車の台数から見積もり、次に見積もるまでの探索はすべて同じ値を読む。
//! 時間帯の途中で書き出した街を読み戻しても同じ経路になるよう、遅れは街の状態に入れる。
//! 信号の待ちの見込みは、ここではなく経路探索のグラフが、網の信号とリンクの忙しさ（[`Delays::busy`]）から足す
//! （[`super::Graph`]）。

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

/// 車のリンクごとの遅れと忙しさ。どちらもリンクの順に並び、0 のリンクは載せない。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Delays {
    rows: Vec<(LinkId, Delay)>,
    /// リンクの終わりを抜けた車が、車線 1 本あたり、数えた時間のうち何割の時間を使ったか（千分率、1000 まで）。
    /// 1 台が抜けるのに 4 秒かかるとして求める。信号の軸が青を使う長さの見積もりに使う。
    busy: Vec<(LinkId, u64)>,
}

impl Delays {
    /// リンクの順に並べた `(リンク, 遅れ)` から作る。遅れが 0 のリンクは除く。
    pub fn from_rows(rows: impl IntoIterator<Item = (LinkId, Delay)>) -> Delays {
        let rows: Vec<(LinkId, Delay)> = rows.into_iter().filter(|r| r.1.ms > 0).collect();
        debug_assert!(
            rows.windows(2).all(|w| w[0].0 < w[1].0),
            "リンクの順に並んでいない"
        );
        Delays {
            rows,
            busy: Vec::new(),
        }
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

    /// リンクの順に並べた `(リンク, 忙しさ（千分率）)` を忙しさにする。0 のリンクは除き、1000 で切る。
    pub fn with_busy(mut self, rows: impl IntoIterator<Item = (LinkId, u64)>) -> Delays {
        self.busy = rows
            .into_iter()
            .filter(|r| r.1 > 0)
            .map(|(l, b)| (l, b.min(1_000)))
            .collect();
        debug_assert!(
            self.busy.windows(2).all(|w| w[0].0 < w[1].0),
            "リンクの順に並んでいない"
        );
        self
    }

    /// リンク `link` の遅れ（ms）。
    pub fn get(&self, link: LinkId) -> u64 {
        self.delay(link).map_or(0, |d| d.ms)
    }

    /// リンク `link` の遅れ。遅れがなければ `None`。
    pub fn delay(&self, link: LinkId) -> Option<Delay> {
        self.rows
            .binary_search_by_key(&link, |r| r.0)
            .ok()
            .map(|i| self.rows[i].1)
    }

    pub fn iter(&self) -> impl Iterator<Item = (LinkId, Delay)> + '_ {
        self.rows.iter().copied()
    }

    /// リンクごとの忙しさ（千分率）。
    pub fn busy(&self) -> impl Iterator<Item = (LinkId, u64)> + '_ {
        self.busy.iter().copied()
    }

    pub fn is_empty(&self) -> bool {
        self.rows.is_empty() && self.busy.is_empty()
    }
}

impl StateHash for Delays {
    fn write_state(&self, h: &mut StateHasher) {
        h.write_len(self.rows.len());
        for (l, d) in &self.rows {
            h.write(&l.segment);
            h.write(&(l.dir as u8));
            h.write(&(l.mode as u8));
            h.write(&d.ms);
            h.write(&u8::from(d.measured));
        }
        h.write_len(self.busy.len());
        for (l, b) in &self.busy {
            h.write(&l.segment);
            h.write(&(l.dir as u8));
            h.write(&(l.mode as u8));
            h.write(b);
        }
    }
}
