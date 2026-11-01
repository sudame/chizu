//! 画面なしで遊ぶための `agent` サブコマンド。
//!
//! 遊んでいる街はセッションのディレクトリに、種と tick を書いた `state.json` と、
//! 操作の記録の `commands.rec` で持つ。フィードバックの `feedback/<日時>-<番号>/` と同じ形である。
//! 呼び出しのあいだに状態は持たず、毎回、種から作った街に記録を再生していまの tick の街を作り直す。

mod command_json;

use std::path::{Path, PathBuf};

use chizu_core::building::{Building, CityStats, demand};
use chizu_core::service::supply_balance;
use chizu_core::world::economy::Finances;
use chizu_core::world::highway_line;
use chizu_core::{
    ApplyOutcome, CHUNK_SIDE, CommandLog, GameTime, ServiceKind, World, ZoneKind, replay,
};
use serde_json::{Map, Value, json};

pub const USAGE: &str = r#"使い方:
  chizu-cli agent new <dir> <種>             セッションを作る（tick 0、空の記録）
  chizu-cli agent act <dir> <commands.json>  いまの tick でコマンドを順に適用し、記録に足す
  chizu-cli agent advance <dir> <tick 数>    時間を進める（1 tick はゲーム内の 12 秒、1 日は 7200 tick）
  chizu-cli agent observe <dir>              街の様子を JSON で出す

<dir> は state.json と commands.rec を置くディレクトリで、feedback/<日時>-<番号>/ もそのまま渡せる。
<commands.json> に - を渡すと標準入力から読む。中身はコマンドの配列か、コマンド 1 つである。
どのサブコマンドも、結果を 1 行の JSON で標準出力に書く。observe の time.day は画面と同じく 1 から数える。

座標:
  マスの座標 [x, y] は整数で、x は右へ、y は下へ増える。
  土地はチャンク（64×64 マス）ごとに買う。チャンク [cx, cy] はマス x が cx*64〜cx*64+63、y も同様の範囲である。
  始めはチャンク [-1,-1]〜[1,1] の 9 つ（マス -64〜127）を買ってある。外とつなぐ高速道路の内側の端は
  observe の highway.end にあり、そこへ道路をつながないと人が来ない。
  長方形 area は 2 つの角 [[x0, y0], [x1, y1]] で書き、両端のマスを含む。角の順番は問わない。

コマンド（type で種類を選ぶ）:
  {"type":"BuildRoad","from":[x,y],"to":[x,y],"kind":"Small"|"Medium"|"Large"}
      道路を引く。縦か横か 45 度の斜めの直線だけで、1 マスあたり 250、450、750 かかる。
  {"type":"Bulldoze","area":[[x,y],[x,y]]}
      範囲の道路、施設、建物を取り壊す。
  {"type":"PaintZone","area":[[x,y],[x,y]],"zone":"Residential"|"Commercial"|"Industrial"|"Office"|null}
      区画を塗る。null は消す。道路から 6 マス以内の空きマスだけが塗られる。1 辺は 256 マスまで。
  {"type":"PlaceBuilding","kind":"PowerPlant"|"WaterPump","at":[x,y],"rotation":0..3}
      施設を置く。発電所は 4×4 で 60000、浄水場は 2×2 で 25000。at は間口の角のマスで、
      rotation 0、1、2、3 のとき、at から y-1、x+1、y+1、x-1 の向きに 1 歩のマスが道路でなければならない。
  {"type":"SetSignal","node":{"index":n,"generation":g},"on":true|false}
      信号を付け外しする（まだ中身がない）。
  {"type":"BuyChunk","chunk":[cx,cy]}
      買った土地に辺を接するチャンクを買う。値段は observe の land.nextChunkPrice。
  {"type":"SetTaxRate","zone":"Residential"|...,"permille":0..300}
      税率を千分率で決める。
  {"type":"SetSimSpeed","speed":1..48}
      画面での速さの倍率。advance の進み方は変わらない。

act の結果:
  results に、コマンドごとに outcome を返す。
    Applied        受け付けた
    Denied         いまの状態では受け付けなかった。reason に理由（NotOwned、NotEnoughMoney など）がある
    NotImplemented 中核にまだ中身がない
    Invalid        形か値が正しくない。message に理由がある。記録には足さない
  Applied、Denied、NotImplemented は記録に足す。画面で遊んだときと同じ扱いである。
"#;

const STATE_FILE: &str = "state.json";
const RECORD_FILE: &str = "commands.rec";

/// 読み込んだセッション。
struct Session {
    dir: PathBuf,
    /// `state.json` の中身。知らないキー（カメラの位置など）も書き戻すときに残す。
    state: Map<String, Value>,
    seed: u64,
    tick: u64,
    log: CommandLog,
}

fn read_u64(state: &Map<String, Value>, key: &str) -> Result<u64, String> {
    let v = state
        .get(key)
        .ok_or_else(|| format!("{STATE_FILE} に {key} がない"))?;
    // 画面の側は種を文字列で書く。JavaScript の数では u64 を表せないためである。
    match v {
        Value::String(s) => s.parse().ok(),
        v => v.as_u64(),
    }
    .ok_or_else(|| format!("{STATE_FILE} の {key} {v} を読めない"))
}

/// ファイルを書きかけのまま残さないよう、隣に書いてから置き換える。
fn write_file(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, bytes).map_err(|e| format!("{} に書けない: {e}", tmp.display()))?;
    std::fs::rename(&tmp, path).map_err(|e| format!("{} に書けない: {e}", path.display()))
}

impl Session {
    fn create(dir: &Path, seed: u64) -> Result<Session, String> {
        if dir.join(STATE_FILE).exists() || dir.join(RECORD_FILE).exists() {
            return Err(format!("{} にはもうセッションがある", dir.display()));
        }
        std::fs::create_dir_all(dir).map_err(|e| format!("{} を作れない: {e}", dir.display()))?;
        Ok(Session {
            dir: dir.to_path_buf(),
            state: Map::new(),
            seed,
            tick: 0,
            log: CommandLog::new(),
        })
    }

    fn load(dir: &Path) -> Result<Session, String> {
        let path = dir.join(STATE_FILE);
        let text = std::fs::read_to_string(&path)
            .map_err(|e| format!("{} を読めない: {e}", path.display()))?;
        let state: Map<String, Value> =
            serde_json::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))?;
        let seed = read_u64(&state, "seed")?;
        let tick = read_u64(&state, "tick")?;
        let path = dir.join(RECORD_FILE);
        let bytes =
            std::fs::read(&path).map_err(|e| format!("{} を読めない: {e}", path.display()))?;
        let log = CommandLog::decode(&bytes).map_err(|e| format!("{}: {e}", path.display()))?;
        if let Some(last) = log.entries().last()
            && last.tick.0 > tick
        {
            return Err(format!(
                "記録に tick {} のコマンドがあり、{STATE_FILE} の tick {tick} より先にある",
                last.tick.0
            ));
        }
        Ok(Session {
            dir: dir.to_path_buf(),
            state,
            seed,
            tick,
            log,
        })
    }

    /// 種から作った街に記録を再生して、いまの tick の街を作る。
    fn world(&self) -> World {
        let mut world = World::new(self.seed);
        replay(&mut world, &self.log, self.tick);
        world
    }

    /// `world` を、このセッションのいまの街として書き出す。
    fn save(&mut self, world: &World) -> Result<(), String> {
        debug_assert_eq!(world.tick().0, self.tick);
        let s = &mut self.state;
        s.insert("seed".into(), Value::String(self.seed.to_string()));
        s.insert("tick".into(), json!(self.tick));
        s.insert("money".into(), Value::String(world.money().0.to_string()));
        s.insert("stateHash".into(), json!(hash_text(world)));
        let mut text = serde_json::to_string_pretty(&self.state).expect("JSON に書けない");
        text.push('\n');
        write_file(&self.dir.join(RECORD_FILE), &self.log.encode())?;
        write_file(&self.dir.join(STATE_FILE), text.as_bytes())
    }
}

fn hash_text(world: &World) -> String {
    format!("{:016x}", world.state_hash())
}

/// コマンドを 1 つずつ記録してから、記録の番号で適用する。`replay` と同じ経路なので、
/// あとで記録を再生しても同じ街になる。
fn act(world: &mut World, log: &mut CommandLog, commands: &[Value]) -> Vec<Value> {
    commands
        .iter()
        .enumerate()
        .map(|(i, value)| {
            let kind = value.get("type").cloned().unwrap_or(Value::Null);
            let invalid = |message: String| json!({"index": i, "type": kind, "outcome": "Invalid", "message": message});
            let command = match command_json::parse(value) {
                Ok(c) => c,
                Err(e) => return invalid(e),
            };
            let seq = match log.record(world.tick(), command) {
                Ok(seq) => seq,
                Err(e) => return invalid(e.to_string()),
            };
            // 角の順番などをそろえたあとの、記録に入った形を返す。
            let mut out = json!({"index": i, "type": kind, "command": command_json::to_json(command)});
            // 結果の種類が増えたら、ここで JSON の名前を決めさせる。
            #[deny(clippy::wildcard_enum_match_arm)]
            let outcome = match world.apply_recorded(seq, &command) {
                ApplyOutcome::Applied => "Applied",
                ApplyOutcome::NotImplemented => "NotImplemented",
                ApplyOutcome::Rejected => "Rejected",
                ApplyOutcome::Denied(reason) => {
                    out["reason"] = json!(format!("{reason:?}"));
                    "Denied"
                }
            };
            out["outcome"] = json!(outcome);
            out
        })
        .collect()
}

fn read_commands(path: &str) -> Result<Vec<Value>, String> {
    let text = if path == "-" {
        std::io::read_to_string(std::io::stdin()).map_err(|e| format!("標準入力を読めない: {e}"))?
    } else {
        std::fs::read_to_string(path).map_err(|e| format!("{path} を読めない: {e}"))?
    };
    match serde_json::from_str(&text).map_err(|e| format!("{path}: {e}"))? {
        Value::Array(items) => Ok(items),
        one => Ok(vec![one]),
    }
}

fn by_zone<T: Into<Value>>(f: impl Fn(ZoneKind) -> T) -> Value {
    let map: Map<String, Value> = ZoneKind::ALL
        .into_iter()
        .map(|z| (format!("{z:?}"), f(z).into()))
        .collect();
    Value::Object(map)
}

fn by_service<T: Into<Value>>(f: impl Fn(ServiceKind) -> T) -> Value {
    let map: Map<String, Value> = ServiceKind::ALL
        .into_iter()
        .map(|k| (format!("{k:?}"), f(k).into()))
        .collect();
    Value::Object(map)
}

/// 画面の時計と同じ書き方。日は 1 から数える。
fn clock_text(time: GameTime) -> String {
    format!(
        "{} 日目 {:02}:{:02}",
        time.day + 1,
        time.hour(),
        time.minute()
    )
}

/// 街の様子。`World` の公開の読み出し口から取れるものをそのまま並べる。
fn observe(world: &World) -> Value {
    let tick = world.tick();
    let time = GameTime::at(tick);
    let stats = world.city_stats();
    let demand = demand(&stats);
    // 中核に欄が足されたらここで止まり、observe に出すかを決められるよう、`..` なしで分解する。
    let CityStats {
        citizens,
        households,
        working_age,
        employed,
        homes,
        workplaces,
        filled,
        zoned,
        happiness,
    } = stats;
    let buildings = world.buildings();
    let count = |f: &dyn Fn(&Building) -> bool| buildings.iter().filter(|(_, b)| f(b)).count();
    let balance = supply_balance(world.network(), world.facilities(), buildings);
    let utility = |slot: usize| {
        let (mut capacity, mut need, mut short) = (0, 0, 0);
        for c in &balance.components {
            capacity += c[slot].capacity;
            need += c[slot].demand;
            short += u64::from(c[slot].demand > 0 && !c[slot].enough());
        }
        json!({"capacity": capacity, "demand": need, "shortNetworks": short})
    };
    let facilities = by_service(|k| world.facilities().iter().filter(|f| f.kind == k).count());
    let finances = world.finances();
    let Finances {
        taxes,
        road_upkeep,
        facility_upkeep,
    } = finances;
    let highway = highway_line(world.seed());
    let owned: Vec<Value> = world
        .chunks()
        .filter(|c| c.owned())
        .map(|c| json!([c.coord().cx, c.coord().cy]))
        .collect();
    json!({
        "seed": world.seed().to_string(),
        "tick": tick.0,
        "time": {
            "day": time.day + 1,
            "hour": time.hour(),
            "minute": time.minute(),
            "text": clock_text(time),
        },
        "stateHash": hash_text(world),
        "money": world.money().0,
        "financesPerDay": {
            "income": finances.income().0,
            "expenses": finances.expenses().0,
            "net": finances.net().0,
            "taxes": by_zone(|z| taxes[z.index()].0),
            "roadUpkeep": road_upkeep.0,
            "facilityUpkeep": by_service(|k| facility_upkeep[k.index()].0),
        },
        "taxPermille": by_zone(|z| world.tax_permille(z)),
        "population": {
            "citizens": citizens,
            "housedCitizens": world.housed_citizens(),
            "households": households,
            "homes": homes,
            "workingAge": working_age,
            "employed": employed,
            "happiness": happiness,
        },
        "demand": by_zone(|z| demand[z.index()]),
        "zoned": by_zone(|z| zoned[z.index()]),
        "jobs": {
            "workplaces": by_zone(|z| workplaces[z.index()]),
            "filled": by_zone(|z| filled[z.index()]),
        },
        "buildings": {
            "total": buildings.len(),
            "byZone": by_zone(|z| count(&|b| b.zone == z)),
            "powered": count(&|b| b.supply.power),
            "watered": count(&|b| b.supply.water),
        },
        "facilities": facilities,
        "utilities": {"power": utility(0), "water": utility(1)},
        "land": {
            "chunkSide": CHUNK_SIDE,
            "ownedChunks": owned,
            "nextChunkPrice": world.next_chunk_price().0,
            "chunkSlots": world.chunk_slots(),
            "milestone": world.milestone(),
        },
        "highway": {
            "end": [highway.start.x, highway.start.y],
            "outward": [highway.step.0, highway.step.1],
        },
    })
}

fn parse_u64(text: &str, what: &str) -> Result<u64, String> {
    text.parse()
        .map_err(|e| format!("{what} {text:?} を読めない: {e}"))
}

fn print(value: &Value) -> String {
    serde_json::to_string(value).expect("JSON に書けない")
}

/// `agent` のあとの引数を受けて、標準出力に書く文字列を返す。
pub fn run(args: &[String]) -> Result<String, String> {
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    match args.as_slice() {
        [] | ["help" | "--help" | "-h"] => Ok(USAGE.trim_end().to_string()),
        ["new", dir, seed] => {
            let mut s = Session::create(Path::new(dir), parse_u64(seed, "種")?)?;
            let world = s.world();
            s.save(&world)?;
            Ok(print(&observe(&world)))
        }
        ["act", dir, commands] => {
            let commands = read_commands(commands)?;
            let mut s = Session::load(Path::new(dir))?;
            let mut world = s.world();
            let results = act(&mut world, &mut s.log, &commands);
            s.save(&world)?;
            Ok(print(&json!({
                "tick": s.tick,
                "results": results,
                "money": world.money().0,
                "stateHash": hash_text(&world),
            })))
        }
        ["advance", dir, ticks] => {
            let ticks = parse_u64(ticks, "tick 数")?;
            let mut s = Session::load(Path::new(dir))?;
            s.tick = s
                .tick
                .checked_add(ticks)
                .ok_or_else(|| "tick があふれる".to_string())?;
            let world = s.world();
            s.save(&world)?;
            Ok(print(&json!({
                "tick": s.tick,
                "time": clock_text(GameTime::at(world.tick())),
                "money": world.money().0,
                "stateHash": hash_text(&world),
            })))
        }
        ["observe", dir] => {
            let s = Session::load(Path::new(dir))?;
            let world = s.world();
            let hash = hash_text(&world);
            if let Some(saved) = s.state.get("stateHash").and_then(Value::as_str)
                && saved != hash
            {
                eprintln!(
                    "{STATE_FILE} の stateHash {saved} と、再生した街のハッシュ {hash} が食い違う"
                );
            }
            Ok(print(&observe(&world)))
        }
        _ => Err(USAGE.trim_end().to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn commands() -> Vec<Value> {
        serde_json::from_value(json!([
            {"type":"BuildRoad","from":[0,10],"to":[30,10],"kind":"Small"},
            {"type":"PaintZone","area":[[0,11],[30,16]],"zone":"Residential"},
            {"type":"BuyChunk","chunk":[5,5]},
            {"type":"SetTaxRate","zone":"Office","permille":999},
            {"type":"Fly"},
            {"type":"SetTaxRate","zone":"Office","permille":150},
        ]))
        .unwrap()
    }

    /// act で適用した街と、記録を種から再生し直した街が同じになる。
    #[test]
    fn act_matches_replay() {
        let seed = 7;
        let mut log = CommandLog::new();
        let mut world = World::new(seed);
        let results = act(&mut world, &mut log, &commands());
        let outcomes: Vec<&str> = results
            .iter()
            .map(|r| r["outcome"].as_str().unwrap())
            .collect();
        assert_eq!(
            outcomes,
            [
                "Applied", "Applied", "Denied", "Invalid", "Invalid", "Applied"
            ]
        );
        assert_eq!(log.len(), 4);

        // 時間を進めてから、もう一度 act する。
        replay(&mut world, &log, 300);
        let more = [json!({"type":"BuildRoad","from":[30,10],"to":[30,40],"kind":"Medium"})];
        act(&mut world, &mut log, &more);
        replay(&mut world, &log, 200);

        let log = CommandLog::decode(&log.encode()).unwrap();
        let mut again = World::new(seed);
        replay(&mut again, &log, 500);
        assert_eq!(again, world);
        assert_eq!(again.state_hash(), world.state_hash());
    }
}
