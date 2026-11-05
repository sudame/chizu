//! 測り方の補助。古い記録の住宅の塗りを、低密度の住宅の塗りに書き換える。
//!
//! `docs/design.md` の 3.9「測り方」で、低密度の効きを古い記録の街で測るのに使う。書き換えるのは
//! `PaintZone` の区画の番号を 1（高密度の住宅）から 5（低密度の住宅）にすることだけで、tick と並びは変えない。
//! 誰が回しても同じ記録になるよう、ファイルに書き出すときも [`to_low_density`] を通す。
//!
//! ```text
//! REC=<元の記録> OUT_ALL=<すべて書き換えた記録> OUT_HALF=<半分を書き換えた記録> \
//!   cargo test -p chizu-core --test low_density_rewrite -- --ignored write_rewritten_records
//! ```

use chizu_core::{Command, CommandLog, Tick, TilePos, TileRect, Zone};

/// どの住宅の塗りを書き換えるか。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Rewrite {
    /// すべての住宅の塗り。
    All,
    /// 記録の中の住宅の塗りのうち、0 から数えて偶数番めのもの。
    Even,
}

/// `log` の住宅（高密度）の塗りを、`which` に従って低密度の住宅の塗りに書き換えた記録。
pub fn to_low_density(log: &CommandLog, which: Rewrite) -> CommandLog {
    let mut out = CommandLog::new();
    let mut nth = 0_u64;
    for e in log.entries() {
        let mut command = e.command;
        if let Command::PaintZone {
            area,
            zone: Some(Zone::Residential),
        } = command
        {
            if which == Rewrite::All || nth.is_multiple_of(2) {
                command = Command::PaintZone {
                    area,
                    zone: Some(Zone::ResidentialLow),
                };
            }
            nth += 1;
        }
        out.record(e.tick, command)
            .expect("書き換えた記録が受け付けられない");
    }
    out
}

fn paint(x: i64, zone: Option<Zone>) -> Command {
    Command::PaintZone {
        area: TileRect::from_corners(TilePos::new(x, 0), TilePos::new(x + 3, 5)),
        zone,
    }
}

#[test]
fn rewrites_only_the_residential_paints() {
    let mut log = CommandLog::new();
    let commands = [
        paint(0, Some(Zone::Residential)),
        paint(10, Some(Zone::Commercial)),
        paint(20, Some(Zone::Residential)),
        paint(30, None),
        paint(40, Some(Zone::Residential)),
    ];
    for (i, c) in commands.iter().enumerate() {
        log.record(Tick(i as u64 * 10), *c).unwrap();
    }
    let zones = |log: &CommandLog| -> Vec<Option<Zone>> {
        log.entries()
            .iter()
            .map(|e| match e.command {
                Command::PaintZone { zone, .. } => zone,
                _ => unreachable!(),
            })
            .collect()
    };
    let low = Some(Zone::ResidentialLow);
    let all = to_low_density(&log, Rewrite::All);
    assert_eq!(
        zones(&all),
        [low, Some(Zone::Commercial), low, None, low].to_vec()
    );
    let half = to_low_density(&log, Rewrite::Even);
    assert_eq!(
        zones(&half),
        [
            low,
            Some(Zone::Commercial),
            Some(Zone::Residential),
            None,
            low
        ]
        .to_vec()
    );
    for out in [&all, &half] {
        let ticks: Vec<(Tick, u64)> = out.entries().iter().map(|e| (e.tick, e.seq)).collect();
        let before: Vec<(Tick, u64)> = log.entries().iter().map(|e| (e.tick, e.seq)).collect();
        assert_eq!(ticks, before);
    }
}

/// 環境変数で渡した記録を書き換えて、ファイルに書き出す。測るときに手で回す。
#[test]
#[ignore]
fn write_rewritten_records() {
    let var = |k: &str| std::env::var(k).unwrap_or_else(|_| panic!("{k} がない"));
    let log = CommandLog::decode(&std::fs::read(var("REC")).unwrap()).unwrap();
    std::fs::write(var("OUT_ALL"), to_low_density(&log, Rewrite::All).encode()).unwrap();
    std::fs::write(var("OUT_HALF"), to_low_density(&log, Rewrite::Even).encode()).unwrap();
}
