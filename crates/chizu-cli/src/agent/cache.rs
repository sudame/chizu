//! 記録の再生を途中から始めるための、セッションのディレクトリに置く街の状態のキャッシュ。
//!
//! 正しい記録は `state.json` と `commands.rec` で、キャッシュはそれを速く作り直すためだけにある。
//! 次のどれかに当たるキャッシュは使わずに捨て、種から作った街に記録を先頭から再生する。
//!
//! - 書いた chizu-cli の実行ファイルが、いまのものと違う。シミュレーションの規則が変わったかもしれないからである。
//! - 種が違う。
//! - キャッシュの街に適用済みのコマンドの並びが、いまの記録の先頭と違う。
//! - キャッシュの街の tick より前に適用するはずのコマンドが、記録にまだ残っている。
//! - 街の状態（`World::to_snapshot`）を読み戻せないか、読み戻した街の状態ハッシュが書いたときと違う。
//!
//! # バイト列の形式
//!
//! 整数はすべてリトルエンディアンの `u64` で書く。
//!
//! ```text
//! 先頭           b"CHIZUCAC"（8 バイト）
//! 版             u32（いまは 1）
//! 実行ファイル   xxh3（64 ビット）
//! 種
//! 適用した件数   キャッシュの街の next_seq
//! 適用した記録   記録の先頭からその件数までを CommandLog::encode で書いたものの xxh3
//! 状態ハッシュ   キャッシュの街の state_hash
//! 街の状態       World::to_snapshot のバイト列（残りすべて）
//! ```
//!
//! chizu-cli だけが読み書きし、画面の保存データには使わないので、形式を変えたら版を上げて古いものを捨てる。

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use chizu_core::{CommandLog, World};
use xxhash_rust::xxh3::xxh3_64;

pub const CACHE_FILE: &str = "replay-cache.bin";

/// 直近の乗る人を数えるために、数日前の街を置くキャッシュのファイル。形式は [`CACHE_FILE`] と同じである。
pub const PAST_CACHE_FILE: &str = "replay-cache-past.bin";

const MAGIC: &[u8; 8] = b"CHIZUCAC";
const VERSION: u32 = 1;
const HEADER_LEN: usize = 8 + 4 + 8 * 6;

/// いま動いている chizu-cli の実行ファイルの xxh3。読めなければ `None` で、キャッシュを使わない。
///
/// 初めて呼んだときの値をプロセスのあいだ使い続ける。実行ファイルはあとから作り直されうるので、
/// `main` の先頭で 1 回呼んで、ロックを待つ前の値に決めておく。
pub fn build_key() -> Option<u64> {
    static KEY: OnceLock<Option<u64>> = OnceLock::new();
    *KEY.get_or_init(|| {
        let exe = std::env::current_exe().ok()?;
        std::fs::read(exe).ok().map(|bytes| xxh3_64(&bytes))
    })
}

/// 記録の先頭 `n` 件を書いたバイト列の xxh3。`n` が件数より多ければ `None`。
fn prefix_hash(log: &CommandLog, n: u64) -> Option<u64> {
    let n = usize::try_from(n).ok()?;
    let entries = log.entries().get(..n)?;
    let mut prefix = CommandLog::new();
    for e in entries {
        prefix.push(*e).expect("記録の先頭は並びを保つ");
    }
    Some(xxh3_64(&prefix.encode()))
}

/// セッションのディレクトリのキャッシュ。
pub struct Cache {
    path: PathBuf,
    key: u64,
}

impl Cache {
    pub fn new(dir: &Path, key: u64) -> Cache {
        Cache {
            path: dir.join(CACHE_FILE),
            key,
        }
    }

    /// 別のファイル名で置くキャッシュ。
    pub fn named(dir: &Path, key: u64, file: &str) -> Cache {
        Cache {
            path: dir.join(file),
            key,
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// いまの実行ファイルで使うキャッシュ。実行ファイルを読めなければ `None`。
    pub fn here(dir: &Path) -> Option<Cache> {
        build_key().map(|key| Cache::new(dir, key))
    }

    /// `world` をキャッシュのバイト列にする。`log` は `world` を作った記録（その先にコマンドが続いてもよい）。
    pub fn encode(&self, world: &World, log: &CommandLog) -> Vec<u8> {
        let applied = world.next_seq();
        let prefix = prefix_hash(log, applied).expect("街に適用したコマンドが記録にない");
        let snapshot = world.to_snapshot();
        let mut out = Vec::with_capacity(HEADER_LEN + snapshot.len());
        out.extend_from_slice(MAGIC);
        out.extend_from_slice(&VERSION.to_le_bytes());
        for v in [self.key, world.seed(), applied, prefix, world.state_hash()] {
            out.extend_from_slice(&v.to_le_bytes());
        }
        out.extend_from_slice(&snapshot);
        out
    }

    /// バイト列から、種 `seed` の街に `log` を再生する途中の街として使える、tick が `limit` 以下の街を作る。
    /// 使えなければ `None`。
    pub fn decode(&self, bytes: &[u8], seed: u64, log: &CommandLog, limit: u64) -> Option<World> {
        let rest = bytes.strip_prefix(MAGIC.as_slice())?;
        let (version, rest) = rest.split_first_chunk::<4>()?;
        if u32::from_le_bytes(*version) != VERSION {
            return None;
        }
        let mut fields = [0_u64; 5];
        let mut rest = rest;
        for f in &mut fields {
            let (v, r) = rest.split_first_chunk::<8>()?;
            *f = u64::from_le_bytes(*v);
            rest = r;
        }
        let [key, cached_seed, applied, prefix, state_hash] = fields;
        if key != self.key || cached_seed != seed || prefix_hash(log, applied) != Some(prefix) {
            return None;
        }
        let world = World::from_snapshot_where(rest, |w| {
            let tick = w.tick().0;
            // 適用していないコマンドは、どれもキャッシュの tick 以後に適用するものでなければならない。
            let pending_ok = usize::try_from(applied)
                .ok()
                .and_then(|n| log.entries().get(n))
                .is_none_or(|e| e.tick.0 >= tick);
            w.seed() == seed && w.next_seq() == applied && tick <= limit && pending_ok
        })
        .ok()?;
        (world.state_hash() == state_hash).then_some(world)
    }

    /// ファイルから読む。ないか使えなければ `None`。
    pub fn load(&self, seed: u64, log: &CommandLog, limit: u64) -> Option<World> {
        let bytes = std::fs::read(&self.path).ok()?;
        self.decode(&bytes, seed, log, limit)
    }

    /// 一時ファイルに書いてから置き換える。キャッシュなので、書けなくても失敗にはしない。
    pub fn store(&self, bytes: &[u8]) {
        let tmp = super::tmp_path(&self.path);
        if std::fs::write(&tmp, bytes).is_err() || std::fs::rename(&tmp, &self.path).is_err() {
            let _ = std::fs::remove_file(&tmp);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chizu_core::{Command, RoadKind, Tick, TilePos, ZoneKind, replay};

    const SEED: u64 = 11;

    fn road(x: i64) -> Command {
        Command::BuildRoad {
            from: TilePos::new(x, 10),
            to: TilePos::new(x + 20, 10),
            kind: RoadKind::Small,
        }
    }

    fn tax(permille: u16) -> Command {
        Command::SetTaxRate {
            zone: ZoneKind::Office,
            permille,
        }
    }

    /// tick 0 に道路を引き、tick 50 に税率を変えて、tick 80 まで進めた街と記録。
    fn played() -> (World, CommandLog) {
        let mut log = CommandLog::new();
        log.record(Tick(0), road(0)).unwrap();
        log.record(Tick(50), tax(120)).unwrap();
        let mut world = World::new(SEED);
        replay(&mut world, &log, 80);
        (world, log)
    }

    fn cache() -> Cache {
        Cache::new(Path::new("unused"), 7)
    }

    /// 書いたキャッシュは読み戻せ、元の街と同じになる。その先にコマンドが足されても使える。
    #[test]
    fn round_trips_and_allows_later_commands() {
        let (world, mut log) = played();
        let bytes = cache().encode(&world, &log);
        let back = cache().decode(&bytes, SEED, &log, 80).expect("読めない");
        assert_eq!(back, world);
        log.record(Tick(80), tax(130)).unwrap();
        log.record(Tick(90), road(40)).unwrap();
        let mut back = cache().decode(&bytes, SEED, &log, 80).expect("読めない");
        let mut fresh = World::new(SEED);
        replay(&mut back, &log, 20);
        replay(&mut fresh, &log, 100);
        assert_eq!(back, fresh);
        assert_eq!(back.state_hash(), fresh.state_hash());
    }

    /// 実行ファイル、種、記録の先頭が違うキャッシュや、壊れたキャッシュは使わない。
    #[test]
    fn rejects_stale_or_broken_caches() {
        let (world, log) = played();
        let bytes = cache().encode(&world, &log);
        let other_build = Cache::new(Path::new("unused"), 8);
        assert!(other_build.decode(&bytes, SEED, &log, 80).is_none());
        assert!(cache().decode(&bytes, SEED + 1, &log, 80).is_none());
        // 求める tick より先の街は使えない。
        assert!(cache().decode(&bytes, SEED, &log, 79).is_none());

        // 記録の先頭が違う（同じ件数で中身が違う）。
        let mut changed = CommandLog::new();
        changed.record(Tick(0), road(0)).unwrap();
        changed.record(Tick(50), tax(150)).unwrap();
        assert!(cache().decode(&bytes, SEED, &changed, 80).is_none());
        // 記録が短い。
        let mut short = CommandLog::new();
        short.record(Tick(0), road(0)).unwrap();
        assert!(cache().decode(&bytes, SEED, &short, 80).is_none());

        // 適用していないコマンドが、キャッシュの tick より前にある。
        let mut early = CommandLog::new();
        early.record(Tick(0), road(0)).unwrap();
        let mut world = World::new(SEED);
        replay(&mut world, &early, 30);
        let at_30 = cache().encode(&world, &early);
        early.record(Tick(20), tax(140)).unwrap();
        assert!(cache().decode(&at_30, SEED, &early, 80).is_none());

        // どこか 1 バイトでも壊れていれば使わない。途中で切れていても使わない。
        for i in [
            0,
            9,
            13,
            21,
            29,
            37,
            45,
            53,
            bytes.len() / 2,
            bytes.len() - 1,
        ] {
            let mut broken = bytes.clone();
            broken[i] ^= 0x40;
            assert!(
                cache().decode(&broken, SEED, &log, 80).is_none(),
                "{i} バイト目を壊しても読めた"
            );
        }
        assert!(
            cache()
                .decode(&bytes[..bytes.len() - 1], SEED, &log, 80)
                .is_none()
        );
        assert!(cache().decode(&[], SEED, &log, 80).is_none());
    }
}
