//! 状態ハッシュ。
//!
//! 状態を決まった順番のバイト列に書き出し、xxh3 の 64 ビットにかける。
//! `std::hash::Hash` は使わない。あちらは書き出すバイト列が標準ライブラリの版や
//! 環境（`usize` の幅など）で変わりうるためである。
//!
//! 書き出しの決まり：
//! - 整数はリトルエンディアンで、型の幅のまま書く。`usize` は `u64` に広げる。
//! - `bool` は 1 バイトの 0 か 1。
//! - 長さの変わる列（`Vec`、スライス、`BTreeMap`、`BTreeSet`）は、要素数を `u64` で書いてから要素を順に書く。
//!   要素数を先に書くので、`[[1], []]` と `[[], [1]]` は別のバイト列になる。
//! - `Option` は 1 バイトのタグ（`None` は 0、`Some` は 1）のあとに中身を書く。

use std::collections::{BTreeMap, BTreeSet};

use xxhash_rust::xxh3::Xxh3;

use crate::units::{LaneOffset, Mm, MmPerTick, Tick, WorldPos};

/// 状態ハッシュに書き出せる型。
pub trait StateHash {
    fn write_state(&self, h: &mut StateHasher);
}

/// 書き出されたバイト列を xxh3 にかける。
#[derive(Clone)]
pub struct StateHasher {
    inner: Xxh3,
}

impl Default for StateHasher {
    fn default() -> Self {
        StateHasher::new()
    }
}

impl StateHasher {
    pub fn new() -> StateHasher {
        StateHasher { inner: Xxh3::new() }
    }

    pub fn write_bytes(&mut self, bytes: &[u8]) {
        self.inner.update(bytes);
    }

    /// 値を 1 つ書き出す。
    pub fn write<T: StateHash + ?Sized>(&mut self, value: &T) {
        value.write_state(self);
    }

    /// 長さの変わる列の要素数を書く。
    pub fn write_len(&mut self, len: usize) {
        // usize は 64 ビットを超えない環境だけを対象にする。
        let len = u64::try_from(len).expect("要素数が u64 に収まらない");
        self.write_bytes(&len.to_le_bytes());
    }

    pub fn finish(&self) -> u64 {
        self.inner.digest()
    }
}

/// `value` だけを書き出したときのハッシュ。
pub fn state_hash_of<T: StateHash + ?Sized>(value: &T) -> u64 {
    let mut h = StateHasher::new();
    value.write_state(&mut h);
    h.finish()
}

macro_rules! impl_int {
    ($($t:ty),*) => {
        $(
            impl StateHash for $t {
                fn write_state(&self, h: &mut StateHasher) {
                    h.write_bytes(&self.to_le_bytes());
                }
            }
        )*
    };
}

impl_int!(u8, u16, u32, u64, u128, i8, i16, i32, i64, i128);

impl StateHash for usize {
    fn write_state(&self, h: &mut StateHasher) {
        h.write_len(*self);
    }
}

impl StateHash for bool {
    fn write_state(&self, h: &mut StateHasher) {
        h.write_bytes(&[u8::from(*self)]);
    }
}

impl StateHash for () {
    fn write_state(&self, _h: &mut StateHasher) {}
}

impl<T: StateHash + ?Sized> StateHash for &T {
    fn write_state(&self, h: &mut StateHasher) {
        (**self).write_state(h);
    }
}

impl<T: StateHash> StateHash for Option<T> {
    fn write_state(&self, h: &mut StateHasher) {
        match self {
            None => h.write_bytes(&[0]),
            Some(v) => {
                h.write_bytes(&[1]);
                v.write_state(h);
            }
        }
    }
}

impl<T: StateHash> StateHash for [T] {
    fn write_state(&self, h: &mut StateHasher) {
        h.write_len(self.len());
        for v in self {
            v.write_state(h);
        }
    }
}

impl<T: StateHash> StateHash for Vec<T> {
    fn write_state(&self, h: &mut StateHasher) {
        self.as_slice().write_state(h);
    }
}

/// 固定長の配列は長さが型で決まるので、要素数を書かない。
impl<T: StateHash, const N: usize> StateHash for [T; N] {
    fn write_state(&self, h: &mut StateHasher) {
        for v in self {
            v.write_state(h);
        }
    }
}

impl<K: StateHash, V: StateHash> StateHash for BTreeMap<K, V> {
    fn write_state(&self, h: &mut StateHasher) {
        h.write_len(self.len());
        for (k, v) in self {
            k.write_state(h);
            v.write_state(h);
        }
    }
}

impl<T: StateHash> StateHash for BTreeSet<T> {
    fn write_state(&self, h: &mut StateHasher) {
        h.write_len(self.len());
        for v in self {
            v.write_state(h);
        }
    }
}

macro_rules! impl_tuple {
    ($($name:ident),+) => {
        impl<$($name: StateHash),+> StateHash for ($($name,)+) {
            #[allow(non_snake_case)]
            fn write_state(&self, h: &mut StateHasher) {
                let ($($name,)+) = self;
                $($name.write_state(h);)+
            }
        }
    };
}

impl_tuple!(A);
impl_tuple!(A, B);
impl_tuple!(A, B, C);
impl_tuple!(A, B, C, D);

impl StateHash for Mm {
    fn write_state(&self, h: &mut StateHasher) {
        self.0.write_state(h);
    }
}

impl StateHash for WorldPos {
    fn write_state(&self, h: &mut StateHasher) {
        self.x.write_state(h);
        self.y.write_state(h);
    }
}

impl StateHash for Tick {
    fn write_state(&self, h: &mut StateHasher) {
        self.0.write_state(h);
    }
}

impl StateHash for MmPerTick {
    fn write_state(&self, h: &mut StateHasher) {
        self.0.write_state(h);
    }
}

impl StateHash for LaneOffset {
    fn write_state(&self, h: &mut StateHasher) {
        self.0.write_state(h);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;
    use xxhash_rust::xxh3::xxh3_64;

    #[test]
    fn writes_the_documented_bytes() {
        let value = (
            0x0102_u16,
            -1_i32,
            true,
            Some(Mm(5)),
            None::<u8>,
            vec![Tick(7)],
        );
        let mut expected = Vec::new();
        expected.extend_from_slice(&[0x02, 0x01]);
        expected.extend_from_slice(&[0xff; 4]);
        expected.push(1);
        expected.push(1);
        expected.extend_from_slice(&5_i64.to_le_bytes());
        expected.push(0);
        expected.extend_from_slice(&1_u64.to_le_bytes());
        expected.extend_from_slice(&7_u64.to_le_bytes());

        let mut h = StateHasher::new();
        h.write(&value.0);
        h.write(&value.1);
        h.write(&value.2);
        h.write(&value.3);
        h.write(&value.4);
        h.write(&value.5);
        assert_eq!(h.finish(), xxh3_64(&expected));
    }

    #[test]
    fn golden_hash() {
        let mut map = BTreeMap::new();
        map.insert(3_u32, WorldPos::new(Mm(-1), Mm(2)));
        map.insert(1_u32, WorldPos::new(Mm(1_000), Mm(0)));
        let value = (
            Tick(7_200),
            map,
            vec![LaneOffset(4), LaneOffset(0)],
            Some(MmPerTick(-13)),
        );
        assert_eq!(state_hash_of(&value), 2_982_768_884_028_407_751);
    }

    #[test]
    fn empty_input_is_xxh3_of_nothing() {
        assert_eq!(StateHasher::new().finish(), xxh3_64(&[]));
    }

    #[test]
    fn length_prefix_separates_nested_vecs() {
        let a: Vec<Vec<u8>> = vec![vec![1], vec![]];
        let b: Vec<Vec<u8>> = vec![vec![], vec![1]];
        assert_ne!(state_hash_of(&a), state_hash_of(&b));
    }

    #[test]
    fn option_tag_separates_none_from_zero() {
        assert_ne!(state_hash_of(&None::<u8>), state_hash_of(&Some(0_u8)));
    }

    proptest! {
        #[test]
        fn btreemap_hash_ignores_insertion_order(entries in proptest::collection::vec((any::<u32>(), any::<i64>()), 0..32)) {
            let forward: BTreeMap<u32, i64> = entries.iter().copied().collect();
            let mut backward = BTreeMap::new();
            for (k, _) in entries.iter().rev() {
                backward.insert(*k, forward[k]);
            }
            prop_assert_eq!(state_hash_of(&forward), state_hash_of(&backward));
        }

        #[test]
        fn vec_hash_is_slice_hash(v in proptest::collection::vec(any::<u64>(), 0..32)) {
            prop_assert_eq!(state_hash_of(&v), state_hash_of(v.as_slice()));
        }

        #[test]
        fn distinct_i64_hash_differently(a in any::<i64>(), b in any::<i64>()) {
            prop_assume!(a != b);
            prop_assert_ne!(state_hash_of(&Mm(a)), state_hash_of(&Mm(b)));
        }
    }
}
