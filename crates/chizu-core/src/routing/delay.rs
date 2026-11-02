//! 混み具合から見積もった、車のリンクを通り抜けるまでの遅れ。
//!
//! 車の経路の費用は、自由に走れたときの所要時間に、この遅れを足したものにする（設計 5.3）。
//! 遅れは時間帯（75 tick）ごとに 1 回、そのとき車線に並んでいる車の台数から見積もり、次に見積もるまでの探索はすべて同じ値を読む。
//! 時間帯の途中で書き出した街を読み戻しても同じ経路になるよう、遅れは街の状態に入れる。

use serde::{Deserialize, Serialize};

use crate::network::LinkId;
use crate::state_hash::{StateHash, StateHasher};

/// 車のリンクごとの遅れ（ms）。リンクの順に並び、遅れのないリンクは載せない。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Delays(Vec<(LinkId, u64)>);

impl Delays {
    /// リンクの順に並べた `(リンク, 遅れ)` から作る。遅れが 0 のリンクは除く。
    pub fn from_sorted(rows: impl IntoIterator<Item = (LinkId, u64)>) -> Delays {
        let rows: Vec<(LinkId, u64)> = rows.into_iter().filter(|r| r.1 > 0).collect();
        debug_assert!(
            rows.windows(2).all(|w| w[0].0 < w[1].0),
            "リンクの順に並んでいない"
        );
        Delays(rows)
    }

    /// リンク `link` の遅れ（ms）。
    pub fn get(&self, link: LinkId) -> u64 {
        self.0
            .binary_search_by_key(&link, |r| r.0)
            .map_or(0, |i| self.0[i].1)
    }

    pub fn iter(&self) -> impl Iterator<Item = (LinkId, u64)> + '_ {
        self.0.iter().copied()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl StateHash for Delays {
    fn write_state(&self, h: &mut StateHasher) {
        h.write_len(self.0.len());
        for (l, ms) in &self.0 {
            h.write(&l.segment);
            h.write(&(l.dir as u8));
            h.write(&(l.mode as u8));
            h.write(ms);
        }
    }
}
