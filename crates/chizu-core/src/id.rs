//! 世代つきの ID と、その空き番号の管理。
//!
//! ID は表の添え字（`u32`）と世代（`u16`）の組である。消した行の添え字は後入れ先出しで使い回し、
//! 使い回すたびに世代を 1 つ上げる。古い ID を持ち続けても、世代が違うので別の行と取り違えない。
//! 世代が `u16::MAX` に達した添え字は、それ以上使い回さずに捨てる。

use crate::state_hash::{StateHash, StateHasher};
use serde::{Deserialize, Serialize};

macro_rules! generational_id {
    ($(#[$meta:meta])* $name:ident) => {
        $(#[$meta])*
        #[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
        pub struct $name {
            pub index: u32,
            pub generation: u16,
        }

        impl $name {
            /// 表の添え字。
            pub fn slot(self) -> usize {
                self.index as usize
            }
        }

        impl StateHash for $name {
            fn write_state(&self, h: &mut StateHasher) {
                self.index.write_state(h);
                self.generation.write_state(h);
            }
        }
    };
}

generational_id!(
    /// 住民の ID。
    CitizenId
);
generational_id!(
    /// 世帯の ID。
    HouseholdId
);
generational_id!(
    /// 建物の ID。
    BuildingId
);
generational_id!(
    /// 移動の主体の ID。
    TripId
);

/// 表の行の割り当て。どの添え字が使われていて、いまの世代がいくつかを持つ。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Slots {
    generation: Vec<u16>,
    live: Vec<bool>,
    /// 空いた添え字。最後に積んだものから使う。
    free: Vec<u32>,
    live_count: usize,
}

impl Slots {
    /// 割り当てた添え字の数。消した行も含む。表の列はこの長さにそろえる。
    pub fn len(&self) -> usize {
        self.live.len()
    }

    pub fn is_empty(&self) -> bool {
        self.live.is_empty()
    }

    /// いま使われている行の数。
    pub fn live_count(&self) -> usize {
        self.live_count
    }

    pub fn is_live(&self, slot: usize) -> bool {
        self.live.get(slot).copied().unwrap_or(false)
    }

    /// `index` と `generation` が、いま使われている行を指すか。
    pub fn contains(&self, index: u32, generation: u16) -> bool {
        let slot = index as usize;
        self.is_live(slot) && self.generation[slot] == generation
    }

    /// 使われている行の添え字と世代。
    pub fn current(&self, slot: usize) -> Option<(u32, u16)> {
        self.is_live(slot)
            .then(|| (slot as u32, self.generation[slot]))
    }

    /// 行を 1 つ割り当てる。2 つ目の値が `true` なら新しい添え字で、列の末尾に 1 行足す必要がある。
    #[track_caller]
    pub fn alloc(&mut self) -> ((u32, u16), bool) {
        self.live_count = self.live_count.checked_add(1).expect("行の数があふれた");
        if let Some(index) = self.free.pop() {
            let slot = index as usize;
            self.live[slot] = true;
            return ((index, self.generation[slot]), false);
        }
        let index = u32::try_from(self.live.len()).expect("添え字が u32 に収まらない");
        self.generation.push(0);
        self.live.push(true);
        ((index, 0), true)
    }

    /// 行を消す。使われていない行なら `false` を返して何もしない。
    pub fn release(&mut self, index: u32, generation: u16) -> bool {
        if !self.contains(index, generation) {
            return false;
        }
        let slot = index as usize;
        self.live[slot] = false;
        self.live_count -= 1;
        if let Some(next) = generation.checked_add(1) {
            self.generation[slot] = next;
            self.free.push(index);
        }
        true
    }
}

impl StateHash for Slots {
    fn write_state(&self, h: &mut StateHasher) {
        h.write(&self.generation);
        h.write(&self.live);
        h.write(&self.free);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn released_slots_are_reused_last_in_first_out_with_a_new_generation() {
        let mut s = Slots::default();
        let ids: Vec<_> = (0..3).map(|_| s.alloc().0).collect();
        assert_eq!(ids, [(0, 0), (1, 0), (2, 0)]);
        assert!(s.release(0, 0));
        assert!(s.release(2, 0));
        assert!(!s.release(2, 0));
        assert_eq!(s.live_count(), 1);
        assert_eq!(s.alloc(), ((2, 1), false));
        assert_eq!(s.alloc(), ((0, 1), false));
        assert_eq!(s.alloc(), ((3, 0), true));
        assert!(!s.contains(0, 0));
        assert!(s.contains(0, 1));
    }

    #[test]
    fn a_slot_at_the_last_generation_is_retired() {
        let mut s = Slots::default();
        s.alloc();
        s.generation[0] = u16::MAX;
        assert!(s.release(0, u16::MAX));
        assert_eq!(s.alloc(), ((1, 0), true));
    }
}
