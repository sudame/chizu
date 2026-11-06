//! 街の状態の書き出しと読み戻し。
//!
//! 記録の再生は遊んだ時間に比例して長くなるので、保存データには街の状態そのものも入れる。
//! 書き出すのは [`World`] の欄のすべてで、作り直せる経路探索だけを除く。読み戻した街は、
//! 書き出す前の街と状態ハッシュが一致し、同じ操作と tick を進めれば同じ街になる。
//!
//! # バイト列の形式
//!
//! ```text
//! 先頭       b"CHIZUSTA"（8 バイト）
//! 版         u32（リトルエンディアン）
//! 検査の値   u64（リトルエンディアン）。中身のバイト列の xxh3（64 ビット）
//! 中身       World を postcard で書いたもの
//! ```
//!
//! 読み戻すときは、中身を読む前に検査の値を比べる。書き込みが途中で切れたものや、壊れたものを、
//! 読み違えた中身から街を作り直す前に断るためである。中身の欄の多くは状態ハッシュに入らないので、
//! 読み戻したあとの状態ハッシュの比較だけでは、その欄が壊れていても気づけない。
//!
//! postcard は欄を宣言の順に書き、整数は可変長、列は要素数のあとに要素を順に書く。
//! 状態の表は `BTreeMap` と `Vec` だけなので、同じ街からは同じバイト列になる。
//!
//! 状態に欄を足したり、欄の型や順番を変えたりしたら、古いバイト列は同じようには読めなくなる。
//! そのときは [`SNAPSHOT_VERSION`] を上げる。古い版のバイト列は読み替えずに断り、保存データの側は記録を再生する。
//! 上げ忘れに気づけるよう、ゴールデンの街を書き出したバイト列のハッシュを `tests/golden/snapshot.hash` に固定してある。

use core::fmt;

use xxhash_rust::xxh3::xxh3_64;

use crate::routing::GraphError;
use crate::world::World;

const MAGIC: &[u8; 8] = b"CHIZUSTA";

/// 先頭、版、検査の値を合わせた長さ。
const HEADER_LEN: usize = 8 + 4 + 8;

/// 書き出す形式の版。
pub const SNAPSHOT_VERSION: u32 = 25;

/// 街の状態を読み戻せない理由。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SnapshotError {
    /// 先頭が `CHIZUSTA` でない。
    BadMagic,
    /// 形式の版が違う。
    UnsupportedVersion(u32),
    /// 中身のバイト列が、検査の値と合わない。
    Checksum,
    /// 中身を読めない。
    Decode(String),
    /// 中身のあとに余りがある。
    TrailingBytes(usize),
    /// 読めた中身を、呼び出し側が受け入れなかった。
    Rejected,
    /// 読めた網の中身が食い違っていて、経路探索のグラフを作れない。
    Network(GraphError),
}

impl fmt::Display for SnapshotError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SnapshotError::BadMagic => write!(f, "街の状態の先頭が違う"),
            SnapshotError::UnsupportedVersion(v) => write!(
                f,
                "街の状態の形式の版 {v} は読めない（読めるのは {SNAPSHOT_VERSION}）"
            ),
            SnapshotError::Checksum => write!(f, "街の状態が壊れている（検査の値が合わない）"),
            SnapshotError::Decode(e) => write!(f, "街の状態を読めない：{e}"),
            SnapshotError::Rejected => write!(f, "街の状態が保存データのほかの欄と食い違う"),
            SnapshotError::TrailingBytes(n) => write!(f, "街の状態のあとに {n} バイトの余りがある"),
            SnapshotError::Network(e) => write!(f, "街の状態の道路網が食い違っている：{e}"),
        }
    }
}

impl World {
    /// 街の状態をバイト列に書き出す。
    pub fn to_snapshot(&self) -> Vec<u8> {
        let body = postcard::to_allocvec(self).expect("街の状態を書き出せない");
        let mut out = Vec::with_capacity(HEADER_LEN + body.len());
        out.extend_from_slice(MAGIC);
        out.extend_from_slice(&SNAPSHOT_VERSION.to_le_bytes());
        out.extend_from_slice(&xxh3_64(&body).to_le_bytes());
        out.extend_from_slice(&body);
        out
    }

    /// [`World::to_snapshot`] で書き出したバイト列から街を作り直す。
    pub fn from_snapshot(bytes: &[u8]) -> Result<World, SnapshotError> {
        World::from_snapshot_where(bytes, |_| true)
    }

    /// [`World::from_snapshot`] と同じだが、読んだ中身を `accept` に見せ、`false` なら
    /// [`SnapshotError::Rejected`] で断る。経路探索のグラフは、`accept` が受け入れてから作る。
    /// 保存データのほかの欄と食い違う中身から、グラフを作り始めないためである。
    pub fn from_snapshot_where(
        bytes: &[u8],
        accept: impl FnOnce(&World) -> bool,
    ) -> Result<World, SnapshotError> {
        let rest = bytes
            .strip_prefix(MAGIC.as_slice())
            .ok_or(SnapshotError::BadMagic)?;
        let (version, rest) = rest
            .split_first_chunk::<4>()
            .ok_or_else(|| SnapshotError::Decode("版が途中で切れている".to_owned()))?;
        let version = u32::from_le_bytes(*version);
        if version != SNAPSHOT_VERSION {
            return Err(SnapshotError::UnsupportedVersion(version));
        }
        let (checksum, body) = rest
            .split_first_chunk::<8>()
            .ok_or_else(|| SnapshotError::Decode("検査の値が途中で切れている".to_owned()))?;
        if u64::from_le_bytes(*checksum) != xxh3_64(body) {
            return Err(SnapshotError::Checksum);
        }
        let (mut world, left) = postcard::take_from_bytes::<World>(body)
            .map_err(|e| SnapshotError::Decode(e.to_string()))?;
        if !left.is_empty() {
            return Err(SnapshotError::TrailingBytes(left.len()));
        }
        world.restore_derived();
        if !accept(&world) {
            return Err(SnapshotError::Rejected);
        }
        world.rebuild_router().map_err(SnapshotError::Network)?;
        Ok(world)
    }
}

#[cfg(any(test, feature = "test-util"))]
impl World {
    /// 検査の値は通るが、区間から指されたノードがない道路網を持つ状態を書き出す。
    /// 読み戻すと [`SnapshotError::Network`] で断られる。他の crate のテストが、
    /// 崩れた状態を読ませるために使う。状態と、その状態ハッシュを返す。
    pub fn to_snapshot_with_broken_network(&self) -> (Vec<u8>, u64) {
        let mut world = self.clone();
        let (node, _) = world.network().nodes().next().expect("ノードがない");
        world.network_mut().remove_node_unchecked(node);
        (world.to_snapshot(), world.state_hash())
    }
}

/// 長さを型で決めた配列を、要素を順に並べた列として書く。serde は長い配列をそのままでは扱えない。
pub(crate) mod big_array {
    use serde::de::{Deserialize, Deserializer, Error, SeqAccess, Visitor};
    use serde::ser::{SerializeTuple, Serializer};
    use std::fmt;
    use std::marker::PhantomData;

    pub fn serialize<S, T, const N: usize>(value: &[T; N], s: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
        T: serde::Serialize,
    {
        let mut t = s.serialize_tuple(N)?;
        for v in value {
            t.serialize_element(v)?;
        }
        t.end()
    }

    struct ArrayVisitor<T, const N: usize>(PhantomData<T>);

    impl<'de, T: Deserialize<'de>, const N: usize> Visitor<'de> for ArrayVisitor<T, N> {
        type Value = Box<[T; N]>;

        fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            write!(f, "{N} 個の要素の列")
        }

        fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Box<[T; N]>, A::Error> {
            let mut v = Vec::with_capacity(N);
            for i in 0..N {
                v.push(
                    seq.next_element()?
                        .ok_or_else(|| A::Error::invalid_length(i, &self))?,
                );
            }
            v.into_boxed_slice()
                .try_into()
                .map_err(|_| A::Error::custom("要素の数が違う"))
        }
    }

    pub fn deserialize_boxed<'de, D, T, const N: usize>(d: D) -> Result<Box<[T; N]>, D::Error>
    where
        D: Deserializer<'de>,
        T: Deserialize<'de>,
    {
        d.deserialize_tuple(N, ArrayVisitor::<T, N>(PhantomData))
    }

    pub fn deserialize<'de, D, T, const N: usize>(d: D) -> Result<[T; N], D::Error>
    where
        D: Deserializer<'de>,
        T: Deserialize<'de>,
    {
        let boxed = deserialize_boxed::<D, T, N>(d)?;
        Ok(*boxed)
    }

    /// `Box<[T; N]>` の欄に使う。
    pub mod boxed {
        use serde::{Deserialize, Deserializer, Serializer};

        // serde の `serialize_with` は欄への参照を渡すので、`&Box` で受ける。
        #[allow(clippy::borrowed_box)]
        pub fn serialize<S, T, const N: usize>(value: &Box<[T; N]>, s: S) -> Result<S::Ok, S::Error>
        where
            S: Serializer,
            T: serde::Serialize,
        {
            super::serialize(value, s)
        }

        pub fn deserialize<'de, D, T, const N: usize>(d: D) -> Result<Box<[T; N]>, D::Error>
        where
            D: Deserializer<'de>,
            T: Deserialize<'de>,
        {
            super::deserialize_boxed(d)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 検査の値を通ったまま網の整合が崩れた状態は、panic せず `Network` で断る。
    #[test]
    fn broken_network_is_rejected_without_panic() {
        let mut world = World::new(1);
        let (node, _) = world
            .network()
            .nodes()
            .next()
            .expect("高速道路のノードがない");
        world.network_mut().remove_node_unchecked(node);
        let bytes = world.to_snapshot();
        assert_eq!(
            World::from_snapshot(&bytes).err(),
            Some(SnapshotError::Network(GraphError::MissingNode(node)))
        );
    }
}
