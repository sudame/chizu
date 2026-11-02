//! 画面なしで遊ぶための `agent` サブコマンド。
//!
//! 遊んでいる街はセッションのディレクトリに、種と tick を書いた `state.json` と、
//! 操作の記録の `commands.rec` で持つ。フィードバックの `feedback/<日時>-<番号>/` と同じ形である。
//! 呼び出しのあいだに状態は持たず、毎回、種から作った街に記録を再生していまの tick の街を作り直す。

mod command_json;
mod map;

use std::fs::File;
use std::path::{Path, PathBuf};

use chizu_core::building::{Building, CityStats, demand, jobs_kept_permille, residential_tax_pull};
use chizu_core::service::supply_balance;
use chizu_core::world::economy::Finances;
use chizu_core::world::highway_line;
use chizu_core::{
    ApplyOutcome, CHUNK_SIDE, CarSample, CitizenPlaces, CommandLog, DayTraffic, GameTime,
    MigrationOutlook, ServiceKind, TilePos, TileRect, Traffic, World, ZoneKind, replay,
};
use serde_json::{Map, Value, json};

pub const USAGE: &str = r#"使い方:
  chizu-cli agent new <dir> <種>             セッションを作る（tick 0、空の記録）
  chizu-cli agent new <dir> --from <fb>      フィードバックのディレクトリ <fb> の種、tick、記録を写してセッションを作る
  chizu-cli agent act <dir> <commands.json>  いまの tick でコマンドを順に適用し、記録に足す
  chizu-cli agent advance <dir> <tick 数>    時間を進める（1 tick はゲーム内の 12 秒、1 日は 7200 tick）
  chizu-cli agent observe <dir>              街の様子を JSON で出す
  chizu-cli agent map <dir> <x0> <y0> <x1> <y1> [--scale N] [--layer traffic]
                                             範囲の地図を、1 マス 1 文字のマス目のテキストで出す

<dir> は state.json と commands.rec を置くディレクトリである。act と advance は agent new で作ったディレクトリにだけ書く。
feedback/<日時>-<番号>/ は observe ではそのまま見られ、続きを遊ぶときは new --from で写してから遊ぶ。
act と advance は、記録を再生したハッシュが state.json の stateHash と食い違うときは何も書かずに止まる。
<commands.json> に - を渡すと標準入力から読む。中身はコマンドの配列か、コマンド 1 つである。
map のほかのサブコマンドは、結果を 1 行の JSON で標準出力に書く。observe の time.day は画面と同じく 1 から数える。

observe の utilities（power と water）:
  capacity と demand 道路網をまたいだ供給量と需要の合計、headroom その差（負なら足りない）、
  headroomPermille 供給量に対する余りの千分率（供給量 0 なら null）、lowHeadroom 余りが 0 以上 1 割未満（画面の前もっての知らせと同じ。足りない、つまり headroom < 0 のときは偽）。
  shortNetworks 足りない道路網の数、shortAt 足りない網ごとの capacity、demand と、代表のマス at
  （網の中で需要が最も大きい建物の道路に面したマス）。at を map で見れば、どの網かが分かる。

observe の人と車の欄（原因が交通にあるかを見分けるためのもの）:
  citizenPlaces  住民のいまの居場所ごとの人数。atHome 家、atWork 職場か学校、atShop 帰りに寄った店、
                 walking 歩いて移動中（車を降りて歩く区切りを含む）、driving 車で移動中、homeless 家のない世帯の住民。
  traffic.cars   車の台数。total 全部、stopped 直前の 100 tick（ゲーム内の約 20 分）のあいだ同じ車線の同じ位置にいる車、
                 moving それ以外。stopped が多いまま増える街は、詰まって動かなくなっているおそれがある。
  traffic.trips  通勤や買い物の移動。inProgress 移動中の数（歩くのも含む）。today と yesterday は
                 ゲーム内の日ごとの結果で、arrived 着いた数、abandoned 道路が変わるなどして途中で着けなくなった数、
                 noRoute 出発のときに経路がなく始められなかった数、avgTravelTicks と avgTravelMinutes 着いた移動の平均の移動時間
                 （1 tick は 12 秒）。着いた移動がなければ null。yesterday は始まって 1 日たつまでは null。
  migration      転入。wantingHouseholds 移住の処理（60 tick ごと）1 回で外から来ようとする世帯の数、
                 vacantHomes 外の世界と道路でつながった住宅の空き戸数、homelessHouseholds と homelessCitizens 家がなく空き家を待つ世帯と住民、
                 blockedHouseholds 来ようとしても空き家が足りず入れない世帯の数。
                 vacantHomes が 0 なのに wantingHouseholds があるときは、住宅が足りないか、外の世界とつながっていない。

observe の税の影響（税率を上げたときの代償）:
  税率は基準の 10% までは影響がなく、それを超えると次の 3 つが効く。
  jobs.keptPermille  区画の種類ごとの、職場のうち雇える割合（‰）。基準より 1‰ 高いごとに 7‰ 減り、24% 前後で 0 になる。
                     jobs.slots が雇える職場の数で、workplaces（建物の上限）より少なければ、税で人を雇えなくなっている。
                     雇えない分だけ職のない住民が増え、税収と幸福度が下がる。商業・工業・オフィスの demand もこの割合だけ小さくなる。
  taxEffect.residentialDemandShift  住宅の demand に足される税率の分。基準より高ければ負で、2‰ ごとに 1 減る。
  幸福度が低い世帯は街を出ていき、越してくる世帯も減る（migration.wantingHouseholds が減る）。

地図（map）:
  observe と同じく何も書かず、フィードバックのディレクトリにも打てる。角 (x0,y0)〜(x1,y1) は両端を含み、順番は問わない。
  1 文字が 1 マスで、先頭に凡例、上に x、行頭に y の目盛りが付く。座標は act のコマンドと同じである。
  --scale N を付けると、N×N マスを 1 文字にまとめる（既定は 1）。まとめた文字は、
  公共施設、道路、建物、区画、水辺、買っていない土地、空きマスの順で、最初に当てはまるものにする。
  同じ種類が混ざるときは数の多いほう（道路は大きいほう）にする。水辺と買った土地は、セルの中の見本のマスの多数決である。
  出力は 9216 文字（96×96 など）までで、超える範囲は、必要な --scale を添えて断る。
  いまの買った土地の範囲は observe の land.ownedBounds にある。
  --layer traffic を付けると、道路の文字を種類の代わりに混み具合の段階にする（画面の「交通」のレイヤーと同じ計算）。
  文字は、「-」空いている、「+」やや混んでいる、「#」混んでいる、「X」詰まっている（車が多く、半分以上が止まっている）。
  区間の上の車の台数を、その区間に並べる台数（車線ごとに長さを 7m で割った台数の合計。最低 1 台）で割った占有率で決め、向きの違う 2 方向のうち混んでいるほうを取る。
  車が 1 台以下の区間は空いている（1 台しか並べない短い区間で、その 1 台が止まっているときを除く）。高速道路は H のままである。

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

/// 車が止まっているとみなす時間（tick）。この長さのあいだ車線の同じ位置にいた車を止まっている車と数える。
/// 1 tick は 12 秒なので、約 20 分である。交差点で譲って待つ程度では超えない長さにしてある。
const STOPPED_TICKS: u64 = 100;

const STATE_FILE: &str = "state.json";
const RECORD_FILE: &str = "commands.rec";
const LOCK_FILE: &str = "session.lock";
/// `agent new` で作ったセッションの `state.json` に入れる印。人間のフィードバックを書き換えないよう、
/// 書き込むのは印のあるディレクトリだけにする。
const SESSION_KEY: &str = "agentSession";

/// 読み込んだセッション。
struct Session {
    dir: PathBuf,
    /// `state.json` の中身。知らないキー（カメラの位置など）も書き戻すときに残す。
    state: Map<String, Value>,
    seed: u64,
    tick: u64,
    log: CommandLog,
    /// ロックを外さないよう、書き終えるまで持つ。
    _lock: Option<File>,
}

/// `state.json` と `commands.rec` の中身。
struct Saved {
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

fn read_saved(dir: &Path) -> Result<Saved, String> {
    let path = dir.join(STATE_FILE);
    let text = std::fs::read_to_string(&path)
        .map_err(|e| format!("{} を読めない: {e}", path.display()))?;
    let state: Map<String, Value> =
        serde_json::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))?;
    let seed = read_u64(&state, "seed")?;
    let tick = read_u64(&state, "tick")?;
    let path = dir.join(RECORD_FILE);
    let bytes = std::fs::read(&path).map_err(|e| format!("{} を読めない: {e}", path.display()))?;
    let log = CommandLog::decode(&bytes).map_err(|e| format!("{}: {e}", path.display()))?;
    if let Some(last) = log.entries().last()
        && last.tick.0 > tick
    {
        return Err(format!(
            "記録に tick {} のコマンドがあり、{STATE_FILE} の tick {tick} より先にある",
            last.tick.0
        ));
    }
    Ok(Saved {
        state,
        seed,
        tick,
        log,
    })
}

impl Saved {
    /// 種から作った街に記録を再生して、いまの tick の街を作る。
    fn world(&self) -> World {
        let mut world = World::new(self.seed);
        replay(&mut world, &self.log, self.tick);
        world
    }

    /// `world` に加えて、止まっている車を見分けるための、[`STOPPED_TICKS`] tick 前の車の位置を返す。
    /// 続きを回すだけなので、街は `world` と同じになる。
    fn world_sampled(&self) -> (World, CarSample) {
        self.world_sampled_over(STOPPED_TICKS)
    }

    fn world_sampled_over(&self, window: u64) -> (World, CarSample) {
        let mut world = World::new(self.seed);
        let window = window.min(self.tick);
        replay(&mut world, &self.log, self.tick - window);
        let before = world.car_sample();
        replay(&mut world, &self.log, window);
        (world, before)
    }

    fn saved_hash(&self) -> Option<&str> {
        self.state.get("stateHash").and_then(Value::as_str)
    }
}

/// セッションのディレクトリのロックを取る。書くときは排他、読むだけのときは共有にする。
/// ロックのファイルは `agent new` だけが作る。人間のフィードバックのディレクトリには何も足さないよう、
/// ほかでは作らず、なければ `None` を返す。
fn lock(dir: &Path, exclusive: bool, create: bool) -> Result<Option<File>, String> {
    let path = dir.join(LOCK_FILE);
    let file = match File::options()
        .create(create)
        .truncate(false)
        .write(true)
        .open(&path)
    {
        Ok(f) => f,
        Err(e) if !create && e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(format!("{} を開けない: {e}", path.display())),
    };
    if exclusive {
        file.lock()
    } else {
        file.lock_shared()
    }
    .map_err(|e| format!("{} をロックできない: {e}", path.display()))?;
    Ok(Some(file))
}

fn tmp_path(path: &Path) -> PathBuf {
    let mut name = path.file_name().expect("ファイル名がない").to_os_string();
    name.push(".tmp");
    path.with_file_name(name)
}

impl Session {
    /// 空のディレクトリに、新しいセッションを作る。
    fn create(dir: &Path, saved: Saved) -> Result<Session, String> {
        let taken = || {
            if dir.join(STATE_FILE).exists() || dir.join(RECORD_FILE).exists() {
                Err(format!("{} にはもうセッションがある", dir.display()))
            } else {
                Ok(())
            }
        };
        // ロックのファイルを作る前に断り、フィードバックなどのディレクトリに何も足さない。
        taken()?;
        std::fs::create_dir_all(dir).map_err(|e| format!("{} を作れない: {e}", dir.display()))?;
        let lock = lock(dir, true, true)?;
        // 確かめてからロックを取るまでのあいだに、ほかの new が作ったかもしれない。
        taken()?;
        let mut state = saved.state;
        // 画面の側だけで変えた操作の数で、作り直した街には当てはまらない。
        state.remove("unrecordedOps");
        state.insert(SESSION_KEY.into(), json!(1));
        Ok(Session {
            dir: dir.to_path_buf(),
            state,
            seed: saved.seed,
            tick: saved.tick,
            log: saved.log,
            _lock: lock,
        })
    }

    /// 書き込むために読み込み、いまの tick の街を作る。`agent new` で作ったセッションで、
    /// 記録を再生したハッシュが `stateHash` と一致するときだけ受け付ける。
    fn open_for_write(dir: &Path) -> Result<(Session, World), String> {
        let lock = lock(dir, true, false)?;
        let saved = read_saved(dir)?;
        if lock.is_none() || saved.state.get(SESSION_KEY).is_none() {
            return Err(format!(
                "{} は agent new で作ったセッションではないので書き込まない。\
                 フィードバックなどから続けるときは chizu-cli agent new <新しいディレクトリ> --from {} を使う",
                dir.display(),
                dir.display()
            ));
        }
        let world = saved.world();
        let hash = hash_text(&world);
        if saved.saved_hash() != Some(hash.as_str()) {
            return Err(format!(
                "{STATE_FILE} の stateHash {:?} と、記録を tick {} まで再生したハッシュ {hash} が食い違うので書き込まない。\
                 前の書き込みが途中で止まったか、シミュレーションの規則が変わったおそれがある。\
                 中身は chizu-cli agent observe {} で見られる。\
                 chizu-cli agent new <新しいディレクトリ> --from {} で写し直せば、記録から作り直して続けられる",
                saved.saved_hash().unwrap_or(""),
                saved.tick,
                dir.display(),
                dir.display()
            ));
        }
        let session = Session {
            dir: dir.to_path_buf(),
            state: saved.state,
            seed: saved.seed,
            tick: saved.tick,
            log: saved.log,
            _lock: lock,
        };
        Ok((session, world))
    }

    /// `world` を、このセッションのいまの街として書き出す。
    ///
    /// 2 つのファイルを一時ファイルに書き終えてから、記録、`state.json` の順に置き換える。
    /// 置き換えのあいだで止まると記録と `stateHash` が食い違うので、次の書き込みで気づける。
    fn save(&mut self, world: &World) -> Result<(), String> {
        debug_assert_eq!(world.tick().0, self.tick);
        let s = &mut self.state;
        s.insert("seed".into(), Value::String(self.seed.to_string()));
        s.insert("tick".into(), json!(self.tick));
        s.insert("money".into(), Value::String(world.money().0.to_string()));
        s.insert("stateHash".into(), json!(hash_text(world)));
        let mut text = serde_json::to_string_pretty(&self.state).expect("JSON に書けない");
        text.push('\n');
        let files = [
            (self.dir.join(RECORD_FILE), self.log.encode()),
            (self.dir.join(STATE_FILE), text.into_bytes()),
        ];
        for (path, bytes) in &files {
            let tmp = tmp_path(path);
            std::fs::write(&tmp, bytes)
                .map_err(|e| format!("{} に書けない: {e}", tmp.display()))?;
        }
        for (path, _) in &files {
            std::fs::rename(tmp_path(path), path)
                .map_err(|e| format!("{} に書けない: {e}", path.display()))?;
        }
        Ok(())
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

/// 車の台数。`before` の時点から同じ車線の同じ位置にいる車を、止まっている車と数える。
fn cars(world: &World, before: &CarSample) -> Value {
    let now = world.car_sample();
    let stopped = now.unmoved_since(before);
    json!({
        "total": now.len(),
        "moving": now.len() - stopped,
        "stopped": stopped,
        "stoppedAfterTicks": STOPPED_TICKS,
    })
}

/// 1 日ぶんの移動の結果。
fn day_traffic(d: Option<DayTraffic>) -> Value {
    let Some(d) = d else {
        return Value::Null;
    };
    // 中核に欄が足されたらここで止まり、observe に出すかを決められるよう、`..` なしで分解する。
    let DayTraffic {
        day,
        arrived,
        abandoned,
        no_route,
        travel_ticks,
    } = d;
    let avg = (arrived > 0).then(|| travel_ticks as f64 / arrived as f64);
    json!({
        "day": day + 1,
        "arrived": arrived,
        "abandoned": abandoned,
        "noRoute": no_route,
        "avgTravelTicks": avg.map(|t| (t * 10.0).round() / 10.0),
        // 1 tick は 12 秒である。
        "avgTravelMinutes": avg.map(|t| (t * 12.0 / 60.0 * 10.0).round() / 10.0),
    })
}

fn traffic(world: &World, before: &CarSample) -> Value {
    let Traffic { today, yesterday } = world.traffic();
    json!({
        "cars": cars(world, before),
        "trips": {
            "inProgress": world.trips().len(),
            "today": day_traffic(Some(today)),
            "yesterday": day_traffic(yesterday),
        },
    })
}

fn citizen_places(p: CitizenPlaces) -> Value {
    let CitizenPlaces {
        at_home,
        at_work,
        at_shop,
        walking,
        driving,
        homeless,
    } = p;
    json!({
        "atHome": at_home,
        "atWork": at_work,
        "atShop": at_shop,
        "walking": walking,
        "driving": driving,
        "homeless": homeless,
    })
}

fn migration(m: MigrationOutlook) -> Value {
    let MigrationOutlook {
        wanting_households,
        vacant_homes,
        homeless_households,
        homeless_citizens,
        blocked_households,
    } = m;
    json!({
        "wantingHouseholds": wanting_households,
        "vacantHomes": vacant_homes,
        "homelessHouseholds": homeless_households,
        "homelessCitizens": homeless_citizens,
        "blockedHouseholds": blocked_households,
    })
}

/// 街の様子。`World` の公開の読み出し口から取れるものをそのまま並べる。
/// `before` は、止まっている車を見分けるための、少し前の車の位置である。
fn observe(world: &World, before: &CarSample) -> Value {
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
        job_slots,
        tax_permille,
        filled,
        zoned,
        happiness,
    } = stats;
    let buildings = world.buildings();
    let count = |f: &dyn Fn(&Building) -> bool| buildings.iter().filter(|(_, b)| f(b)).count();
    let balance = supply_balance(world.network(), world.facilities(), buildings);
    let utility = |slot: usize| {
        let (mut capacity, mut need, mut bare) = (0_u64, 0_u64, 0_u64);
        let mut short = Vec::new();
        for (i, c) in balance.components.iter().enumerate() {
            capacity += c[slot].capacity;
            need += c[slot].demand;
            bare += u64::from(c[slot].demand > 0 && c[slot].capacity == 0);
            if c[slot].demand > 0 && !c[slot].enough() {
                let at = balance.spots[i].map(|p| json!([p.x, p.y]));
                short.push(json!({
                    "capacity": c[slot].capacity,
                    "demand": c[slot].demand,
                    "at": at,
                }));
            }
        }
        // 余りは供給量から需要を引いた値で、負なら足りない。割合は供給量に対する千分率で、
        // 供給量が 0 なら null。余りが 1 割を切ったら lowHeadroom を立てる（画面の知らせと同じ基準）。
        let headroom = i128::from(capacity) - i128::from(need);
        let permille = (capacity > 0).then(|| headroom * 1000 / i128::from(capacity));
        json!({
            "capacity": capacity,
            "demand": need,
            "headroom": headroom,
            "headroomPermille": permille,
            "lowHeadroom": need > 0 && headroom >= 0 && permille.is_some_and(|p| p < 100),
            "shortNetworks": short.len(),
            "networksWithoutFacility": bare,
            "shortAt": short,
        })
    };
    let growth = world.growth_blocks();
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
    // 買ったチャンクをすべて含む、マスの長方形。map の範囲を決める目安にする。
    let side = CHUNK_SIDE as i64;
    let owned_bounds = world
        .chunks()
        .filter(|c| c.owned())
        .map(|c| c.coord().origin())
        .fold(None, |b: Option<[i64; 4]>, o| {
            let [x0, y0, x1, y1] = b.unwrap_or([o.x, o.y, o.x + side - 1, o.y + side - 1]);
            Some([
                x0.min(o.x),
                y0.min(o.y),
                x1.max(o.x + side - 1),
                y1.max(o.y + side - 1),
            ])
        })
        .map(|[x0, y0, x1, y1]| json!([[x0, y0], [x1, y1]]));
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
        "citizenPlaces": citizen_places(world.citizen_places()),
        "traffic": traffic(world, before),
        "migration": migration(world.migration_outlook()),
        "demand": by_zone(|z| demand[z.index()]),
        "zoned": by_zone(|z| zoned[z.index()]),
        "jobs": {
            "workplaces": by_zone(|z| workplaces[z.index()]),
            "filled": by_zone(|z| filled[z.index()]),
            "slots": by_zone(|z| job_slots[z.index()]),
            "keptPermille": by_zone(|z| jobs_kept_permille(tax_permille[z.index()])),
        },
        "taxEffect": {
            "residentialDemandShift": residential_tax_pull(tax_permille[ZoneKind::Residential.index()]),
        },
        "buildings": {
            "total": buildings.len(),
            "byZone": by_zone(|z| count(&|b| b.zone == z)),
            "powered": count(&|b| b.supply.power),
            "watered": count(&|b| b.supply.water),
        },
        "facilities": facilities,
        "utilities": {"power": utility(0), "water": utility(1)},
        // 需要があるのに建たないとき、新築の候補の間口のうち、電気か水道の余裕がなくて建たないものの数。
        // その網の電気か水道の余裕が足りない。施設を足すか、施設のある網へ道路でつなぐ。
        // 施設のない網（道路が 1 マス離れているなど）かどうかは、utilities の networksWithoutFacility で見分ける。
        "growth": by_zone(|z| {
            let g = growth[z.index()];
            json!({
                "fronts": g.fronts,
                "blocked": g.blocked,
                "noPower": g.no_power,
                "noWater": g.no_water,
            })
        }),
        "land": {
            "chunkSide": CHUNK_SIDE,
            "ownedChunks": owned,
            "ownedBounds": owned_bounds,
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

/// 新しいセッションを作り、街の様子を返す。`from` があれば、そのディレクトリの種、tick、記録を写す。
fn new_session(dir: &str, from: Saved, from_dir: Option<&str>) -> Result<Value, String> {
    let (world, before) = from.world_sampled();
    let copied = from_dir.map(|d| {
        let hash = hash_text(&world);
        let recorded = from.saved_hash().map(str::to_string);
        let mut c = json!({
            "dir": d,
            "recordedStateHash": recorded,
            "replayedStateHash": hash,
            "matches": recorded.as_deref() == Some(hash.as_str()),
        });
        let unrecorded = match from.state.get("unrecordedOps") {
            Some(Value::Bool(b)) => *b,
            Some(Value::Number(n)) => n.as_u64() != Some(0),
            _ => false,
        };
        if unrecorded {
            c["note"] = json!(
                "記録に残らない操作（unrecordedOps）があるので、ハッシュは食い違うのが当然である"
            );
        }
        c
    });
    let mut s = Session::create(Path::new(dir), from)?;
    s.save(&world)?;
    let mut out = observe(&world, &before);
    if let Some(c) = copied {
        out["copiedFrom"] = c;
    }
    Ok(out)
}

/// 読むだけで開く。人間のフィードバックも見られるよう、セッションでなくても読む。共有ロックは返り値が持つ。
fn open_for_read(dir: &Path) -> Result<(Option<File>, World, CarSample), String> {
    let lock = lock(dir, false, false)?;
    let saved = read_saved(dir)?;
    let (world, before) = saved.world_sampled();
    let hash = hash_text(&world);
    if let Some(recorded) = saved.saved_hash()
        && recorded != hash
    {
        eprintln!(
            "{STATE_FILE} の stateHash {recorded} と、再生した街のハッシュ {hash} が食い違う"
        );
    }
    Ok((lock, world, before))
}

/// `agent` のあとの引数を受けて、標準出力に書く文字列を返す。
pub fn run(args: &[String]) -> Result<String, String> {
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    match args.as_slice() {
        [] | ["help" | "--help" | "-h"] => Ok(USAGE.trim_end().to_string()),
        ["new", dir, "--from", from] => {
            // 写し元がセッションなら、observe と同じく共有ロックを取ってから読む。
            let _from_lock = lock(Path::new(from), false, false)?;
            let saved = read_saved(Path::new(from))?;
            Ok(print(&new_session(dir, saved, Some(from))?))
        }
        ["new", dir, seed] => {
            let saved = Saved {
                state: Map::new(),
                seed: parse_u64(seed, "種")?,
                tick: 0,
                log: CommandLog::new(),
            };
            Ok(print(&new_session(dir, saved, None)?))
        }
        ["act", dir, commands] => {
            let commands = read_commands(commands)?;
            let (mut s, mut world) = Session::open_for_write(Path::new(dir))?;
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
            let (mut s, mut world) = Session::open_for_write(Path::new(dir))?;
            s.tick = s
                .tick
                .checked_add(ticks)
                .ok_or_else(|| "tick があふれる".to_string())?;
            // 読み込んだときに tick まで適用してあるので、続きを回す。
            replay(&mut world, &s.log, ticks);
            s.save(&world)?;
            Ok(print(&json!({
                "tick": s.tick,
                "time": clock_text(GameTime::at(world.tick())),
                "money": world.money().0,
                "stateHash": hash_text(&world),
            })))
        }
        ["observe", dir] => {
            let (_lock, world, before) = open_for_read(Path::new(dir))?;
            Ok(print(&observe(&world, &before)))
        }
        ["map", dir, x0, y0, x1, y1, rest @ ..] => {
            let coord = |s: &str, what: &str| {
                s.parse::<i64>()
                    .map_err(|e| format!("{what} {s:?} を読めない: {e}"))
            };
            let a = TilePos::new(coord(x0, "x0")?, coord(y0, "y0")?);
            let b = TilePos::new(coord(x1, "x1")?, coord(y1, "y1")?);
            let (mut scale, mut layer) = (1, map::MapLayer::Terrain);
            let mut options = rest.iter();
            while let Some(&option) = options.next() {
                match (option, options.next()) {
                    ("--scale", Some(&n)) => scale = parse_u64(n, "--scale")?,
                    ("--layer", Some(&name)) => layer = map::MapLayer::parse(name)?,
                    _ => return Err(USAGE.trim_end().to_string()),
                }
            }
            let (_lock, world, _) = open_for_read(Path::new(dir))?;
            map::render(&world, TileRect::from_corners(a, b), scale, layer)
        }
        _ => Err(USAGE.trim_end().to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TICKS_PER_DAY_FOR_TEST: u64 = chizu_core::TICKS_PER_DAY;

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

    /// 止まっている車を見分ける見本は、いまでなく `window` tick 前の車の位置である。
    #[test]
    fn sample_is_taken_a_window_before_now() {
        let seed = 42;
        let line = chizu_core::world::highway_line(seed);
        let (x, y) = (line.start.x, line.start.y);
        let inward = -line.step.0 as i64;
        // 家と職場を車で行くほど（500m）離して置いた、長い 1 本道。
        let at = |d: i64| x + inward * d;
        let mut log = CommandLog::new();
        let mut world = World::new(seed);
        let cmds: Vec<Value> = serde_json::from_value(json!([
            {"type":"BuildRoad","from":[x,y],"to":[at(185),y],"kind":"Medium"},
            {"type":"PaintZone","area":[[at(2),y+1],[at(20),y+6]],"zone":"Residential"},
            {"type":"PaintZone","area":[[at(150),y-6],[at(185),y-1]],"zone":"Industrial"},
            {"type":"PlaceBuilding","kind":"PowerPlant","at":[at(100),y+1],"rotation":0},
            {"type":"PlaceBuilding","kind":"WaterPump","at":[at(110),y+1],"rotation":0},
        ]))
        .unwrap();
        act(&mut world, &mut log, &cmds);
        // 車が走っている朝のどこかまで進める。
        let (mut found, mut tick) = (None, 0);
        while tick < 5 * TICKS_PER_DAY_FOR_TEST && found.is_none() {
            replay(&mut world, &log, 25);
            tick += 25;
            let day_tick = tick % TICKS_PER_DAY_FOR_TEST;
            let moving = world.car_sample();
            if tick > 3 * TICKS_PER_DAY_FOR_TEST && day_tick > 2000 && !moving.is_empty() {
                found = Some(tick);
            }
        }
        let tick = found.expect("車が走らなかった");
        let saved = Saved {
            state: Map::new(),
            seed,
            tick: world.tick().0,
            log: log.clone(),
        };
        assert_eq!(tick, saved.tick);
        let (replayed, before) = saved.world_sampled_over(50);
        assert_eq!(replayed.state_hash(), world.state_hash());
        // 見本は 50 tick 前の車の位置で、別に 50 tick 手前まで再生した街の位置と一致する。
        let mut earlier = World::new(seed);
        replay(&mut earlier, &log, tick - 50);
        assert_eq!(before, earlier.car_sample());
        // 走っている車は 50 tick のあいだに動いているので、止まっているとは数えない。
        let now = replayed.car_sample();
        assert!(now.unmoved_since(&before) < now.len(), "車が動いていない");
    }
}
