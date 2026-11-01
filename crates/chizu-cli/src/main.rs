//! 操作の記録を再生して、状態ハッシュを出す。
//!
//! ```text
//! chizu-cli <記録ファイル> <種> <tick 数>
//! ```
//!
//! 標準出力には状態ハッシュを 16 進の 16 桁で 1 行だけ書く。
//! 適用したコマンドの件数は標準エラーに書く。

use std::process::ExitCode;

use chizu_core::{CommandLog, DenyReason, World, replay};

const USAGE: &str = "使い方: chizu-cli <記録ファイル> <種> <tick 数>";

fn run(args: &[String]) -> Result<String, String> {
    let [path, seed, ticks] = args else {
        return Err(USAGE.to_string());
    };
    let seed: u64 = seed
        .parse()
        .map_err(|e| format!("種 {seed:?} を読めない: {e}"))?;
    let ticks: u64 = ticks
        .parse()
        .map_err(|e| format!("tick 数 {ticks:?} を読めない: {e}"))?;
    let bytes = std::fs::read(path).map_err(|e| format!("{path} を読めない: {e}"))?;
    let log = CommandLog::decode(&bytes).map_err(|e| format!("{path}: {e}"))?;

    let mut world = World::new(seed);
    let stats = replay(&mut world, &log, ticks);
    eprintln!(
        "適用 {} 件、未実装で無視 {} 件、範囲外で無視 {} 件、却下 {} 件",
        stats.applied, stats.not_implemented, stats.rejected, stats.denied
    );
    for reason in DenyReason::ALL {
        let n = stats.denied_by[reason.index()];
        if n > 0 {
            eprintln!("  却下の理由 {reason:?}: {n} 件");
        }
    }
    Ok(format!("{:016x}", world.state_hash()))
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match run(&args) {
        Ok(hash) => {
            println!("{hash}");
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("{e}");
            ExitCode::FAILURE
        }
    }
}
