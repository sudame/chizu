//! 画面なしで遊ぶための `agent` サブコマンド。
//!
//! 遊んでいる街はセッションのディレクトリに、種と tick を書いた `state.json` と、
//! 操作の記録の `commands.rec` で持つ。フィードバックの `feedback/<日時>-<番号>/` と同じ形である。
//! 呼び出しのたびに、記録を再生していまの tick の街を作り直す。`agent new` で作ったセッションでは、
//! 再生の途中の街を [`cache`] に置き、次の呼び出しはそこから続きを再生する。

mod cache;
mod command_json;
mod export;
mod map;

use std::fs::File;
use std::path::{Path, PathBuf};

use cache::Cache;
use chizu_core::building::{
    Building, CityStats, demand, demand_factors, jobs_kept_permille, residential_tax_pull,
};
use chizu_core::service::supply_balance;
use chizu_core::world::economy::Finances;
use chizu_core::world::growth::{GrowthBlock, StalledNetwork};
use chizu_core::world::highway_line;
use chizu_core::{
    ApplyOutcome, CHUNK_SIDE, CarSample, ChunkCoord, CitizenPlaces, Command, CommandLog,
    CommuteDistances, DayTraffic, DenyReason, GameTime, Heading, Line, MigrationOutlook,
    NodeControl, ParkingTally, Rescues, ServiceKind, TilePos, TileRect, Traffic, World, ZoneKind,
    replay,
};
use serde_json::{Map, Value, json};

pub const USAGE: &str = r#"使い方:
  chizu-cli agent new <dir> <種>             セッションを作る（tick 0、空の記録）
  chizu-cli agent new <dir> --from <fb>      フィードバックのディレクトリ <fb> の種、tick、記録を写してセッションを作る
  chizu-cli agent act <dir> <commands.json> [--verbose]
                                             いまの tick でコマンドを順に適用し、記録に足す
  chizu-cli agent advance <dir> <tick 数>    時間を進める（1 tick はゲーム内の 12 秒、1 日は 7200 tick）
  chizu-cli agent observe <dir>              街の様子を JSON で出す（前の observe からの増減つき）
  chizu-cli agent map <dir> <x0> <y0> <x1> <y1> [--scale N] [--layer traffic]
                                             範囲の地図を、1 マス 1 文字のマス目のテキストで出す
  chizu-cli agent export <dir>               セッションの再現手順を、Issue にそのまま貼れる markdown で出す

<dir> は state.json と commands.rec を置くディレクトリである。act と advance は agent new で作ったディレクトリにだけ書く。
feedback/<日時>-<番号>/ は observe ではそのまま見られ、続きを遊ぶときは new --from で写してから遊ぶ。
act と advance は、記録を再生したハッシュが state.json の stateHash と食い違うときは何も書かずに止まる。
agent new で作ったセッションには、再生の途中の街を replay-cache.bin に置き、次の呼び出しはそこから続きを再生する。
正しい記録は state.json と commands.rec で、キャッシュは消してもよい。chizu-cli を作り直したときや、
記録の先頭と合わないとき、壊れているときは、捨てて種から再生し直す。
<commands.json> に - を渡すと標準入力から読む。中身はコマンドの配列か、コマンド 1 つである。
map と export のほかのサブコマンドは、結果を 1 行の JSON で標準出力に書く。observe の time.day は画面と同じく 1 から数える。

observe の sinceLastObserve（前の observe からの増減）:
  agent new で作ったセッションでは、observe を打つたびに主な数をセッションの last-observe.json に残し、
  次の observe で fromTick（前に見た tick）と toTick のあいだに変わった数を changes に出す。
  changes は {"money": {"from": a, "to": b, "delta": b-a}, ...} の形で、変わっていない数は出さない。
  見る数は、お金、収支、人口、就業、幸福度、車、転入、需要、建物の数、電気と水道の余裕、施設の数、買ったチャンクの数である。
  new の直後の observe は、new が見た街との増減になる。前の observe がないとき（フィードバックのディレクトリなど）は null で、何も書かない。

observe の shortages: 主の網の余りが負の種類（"power"、"water"）の一覧。足りているときは空。lowHeadroom は足りないと偽になるので、足りないことはここか main.shortage で読む。
observe の utilities（power と water）:
  capacity と demand 道路網をまたいだ供給量と需要の合計。この合計から余りは出さない。つながっていない施設の分まで足されて、
  主の網が足りないのに余っているように見えるからである。余りは次の 2 つに分けて読む。
  main 主の網（需要がいちばん大きい網。どの網にも需要がなければ null）の capacity、demand、headroom（その差。負なら足りない）、
  headroomPermille（供給量に対する余りの千分率。供給量 0 なら null）、lowHeadroom（余りが 0 以上 1 割未満。画面の前もっての知らせと同じ。足りない、つまり headroom < 0 のときは偽）、
  shortage（headroom < 0。足りなくて新築が止まる）、shortBy（足りない量。足りているときは 0）、
  at（網の中で需要が最も大きい建物の道路に面したマス）。
  others 主の網以外で、施設か建物のある道路網の networks（数）と capacity、demand、headroom（その差）。
  shortNetworks 足りない道路網の数、shortAt 足りない網ごとの capacity、demand と、代表のマス at。at を map で見れば、どの網かが分かる。
  isolatedFacilities 孤立した施設の at の一覧。電気は発電所、水道は浄水場について数える。次のどちらかに当たる施設である。
    入口がない（面した道路が消えた）。
    主の網ではない網にあり、その網の余りが正で、主の網の余りが少ないか足りない（main の lowHeadroom か headroom が負）。
  この施設は主の網に届かないので、余りに数えてはならない。道路でつなぐか、置き直す。
  主の網に十分な余りがあるあいだは、ほかの網に施設があっても数えない（別の町を作る遊び方で、誤った知らせを出さないため）。
  切れた道路に区画を塗って家が建っても、主の網が足りないあいだは数える。

observe の growth（新築が電気か水道の余裕で止まっている間口）:
  区画の種類ごとに fronts 候補の間口、blocked 止まっている数、noPower と noWater そのうち電気、水道の余裕がない数。
  noFreeLot 需要が正なのに、建てる候補の間口（fronts）が 1 つもない。塗った区画が建ち尽くしたときだけ真で、塗ったことのない種類は偽。電気や水道の余裕とは関係がない。
    画面は、これが半日（ゲーム内）続くと「○○の需要があるのに、空いている○○の区画がない」と知らせる。区画を塗り足すと偽になる。
  stalledAt 止まっている道路網ごとの at（止まっている間口のうち座標が最小のマス）と、noPower、noWater（その網に足りないもの）。
  画面の知らせを押したときに映る場所と同じ。止まっている網がなければ空の配列。at を map で見れば、どの網かが分かる。

observe の人と車の欄（原因が交通にあるかを見分けるためのもの）:
  citizenPlaces  住民のいまの居場所ごとの人数。atHome 家、atWork 職場か学校、atShop 帰りに寄った店、
                 walking 歩いて移動中（車を降りて歩く区切りを含む）、driving 車で移動中、homeless 家のない世帯の住民。
  traffic.cars   車の台数。total 全部、stopped 道路の上で、直前の 100 tick（ゲーム内の約 20 分）のあいだ同じ位置にいる車
                 （道路の種類を下げて車線を外れ、車線に戻るのを待つ車も含む）、
                 waitingAtCurb 建物の駐車場の中で、車線に出る順番を待っている車、moving それ以外。
                 着く車は駐車場の出入りの位置で車線から外れ、車線の上では止まらない。
                 stopped が多いまま増える街は、詰まって動かなくなっているおそれがある。waitingAtCurb が多いのは、
                 建物の前の車線が埋まって出られない車が多いということである。
  traffic.walkers 車道を渡るところの歩く人。atCrosswalks 交差点の横断歩道を渡っているか渡るのを待っている人、
                 midblock 区間の途中で道路を横切っているか横切るのを待っている人。横断歩道は 3 本以上の道路が集まる
                 交差点の、高速道路でないどの腕にも自動で置かれ、人が渡っているあいだ、その腕から入る車と出る車は止まる。
                 区間の途中を横切る人の手前でも車は止まる。歩く人は、近くの横断歩道までの遠回りが 100m より
                 短ければ横切らない。信号のある交差点では、歩く人も信号に従う。
                 横断歩道は SetCrosswalk で腕ごとに外せる。外した腕は渡らずに交差点のまわりを回り、
                 どちらに回っても越えるときだけ、横断歩道があるときと同じく車を止めて渡る。
  traffic.trips  通勤や買い物の移動。inProgress 移動中の数（歩くのも含む）。today と yesterday は
                 ゲーム内の日ごとの結果で、arrived 着いた数、abandoned 道路が変わるなどして途中で着けなくなった数、
                 noRoute 出発のときに経路がなく始められなかった数、avgTravelTicks と avgTravelMinutes 着いた移動の平均の移動時間
                 （1 tick は 12 秒）。着いた移動がなければ null。yesterday は始まって 1 日たつまでは null。
  parking        建物の駐車場。byZone 区画の種類ごとに、spaces 停められる台数の合計（surface 敷地を切り欠いた平面の駐車場、
                 indoor 建物の中の駐車場の内訳）、parked 停めてある台数、reserved 向かっている車のために押さえた台数、
                 occupiedPermille 停めてある台数と押さえた台数の和の、台数に対する千分率（台数 0 なら null）、
                 fullLots 満車の駐車場の数。
                 台数は建物の種類、大きさ、段階から決まる。住宅の段階 1〜2 は世帯の数、段階 3〜5 は世帯の数の 0.7 倍、
                 商業と工業は職場の数、オフィスは職場の数の 0.5 倍（端数は切り上げ）。住宅の駐車場は、その建物の住民だけが使う。
                 redirectedCars いま移動中の車のうち、16 マスの中のほかの建物の駐車場へ停めに向かっている車。行き先の建物の
                 駐車場が満車のときのほか、そこへ車か歩きで届かないときも、近くのほかの駐車場に回る。
                 次の 4 つは始まりからの積み上げで、日ごとの数は 2 つの observe の差で求める。
                 drives 車で走り始めた移動の数、parkedElsewhere そのうちほかの建物の駐車場に回った数（redirectedCars と同じ決まり）、
                 lotsFull 行き先の近くの駐車場がどこも満車で、車で向かうのをやめた数（やめた移動は、歩けるなら歩く）、
                 noCar 車は世帯に 1 台で、ほかの住民が乗っているか、家族が家でない所に停めてあって、車で向かえなかった数（歩けるなら歩く）。
                 drives、noCar、lotsFull は別々の数で、足すと車で行こうとした移動の数になる（noCar は drives に含まれない）。
                 ただし、道路網が分かれていて車で届かないなど、3 つのどれも増やさずに歩きへ回る場合は数えない。
                 noCarBreakdown noCar の内訳で、4 つの和が noCar に等しい。inUse 世帯の車を家族が乗っている、
                 parkedAway 家族が家でない所（職場など）に停めている（停めた車が本人のいる建物にあっても、
                 同じ職場で働く家族が停めた車なら、ここに入る）、carAtHome 車は家にあるが、出発する本人が家にいない、
                 other そのほか（家のない住民の通勤で、車が停めてあった場合）。
                 fullLots、redirectedCars、lotsFull が多いのは、駐車場が足りないということである。
  traffic.rescues 詰まりをほどく救済の、始まりからの積み上げ。teleported 止まったままの車を行き先に着いたことにした数、
                 walkedInstead 車線に出られないまま待ち続けた車を、歩く移動に切り替えた数、
                 passedStuck 交差点の中で止まった車を無視して入った数。
  commute        通勤の距離の分布。職のある住民の、家から職場までの出入口の直線距離（道のりではない）で数える。
                 commuters 数えた住民、medianMeters 中央値（m）、under500m 500m 未満、from500mTo1km 500m 以上 1km 未満、
                 from1km 1km 以上の人数。職のない住民は数えない。住民は職のない間、家から近い空きのある職場を選ぶので、
                 職住を近づける区画の置き方は、この分布が短いほうへ寄ることで確かめられる。
  migration      転入。wantingHouseholds 移住の処理（60 tick ごと）1 回で外から来ようとする世帯の数、
                 vacantHomes 外の世界と道路でつながった住宅の空き戸数、homelessHouseholds と homelessCitizens 家がなく空き家を待つ世帯と住民、
                 blockedHouseholds 来ようとしても空き家が足りず入れない世帯の数。
                 vacantHomes が 0 なのに wantingHouseholds があるときは、住宅が足りないか、外の世界とつながっていない。

observe の demandFactors（demand の内訳）:
  区画の種類ごとに、demand を足し引きしている要因を [{factor, value}] で並べる。value を足し合わせると demand になる。値が 0 の要因も省かない。
  Residential  base いつもある需要、vacantJobs 空いている職場（多いほど正）、happiness 幸福度、tax 税率（基準より高いと負）、
               vacantHomes 空き家（多いほど負）、jobless 職のない住民（多いほど負）。
  Commercial   shoppers 住民の買い物に要る職場、tax 税で必要な職場が減る分（0 以下）、existing いまの商業の職場の数（負）。
  Industrial   base いつもある需要、workers 商業で働かない働き手のうち工業に回る分に要る職場、tax 税で減る分（0 以下）、existing いまの工業の職場の数（負）。
  Office       workers 同じくオフィスに回る分に要る職場、tax 税で減る分（0 以下）、existing いまのオフィスの職場の数（負）。
  職場の区画の demand は「要る職場 − いまの職場」なので、existing が要る職場（shoppers か workers）を上回ると負になる。失業者の数では決まらない。
  負のときの案内（種類ごと）:
  Residential  vacantHomes が大きく負なら、空き家が埋まるのを待つ（住宅を塗り足さない）。jobless が負なら職場を増やす。
               tax が負なら住宅の税を下げる。happiness が負なら幸福度を上げる。
  Commercial   shoppers は住民全体の 1/4 で、働く年齢の人の数ではない。existing が上回るなら、住民を増やす（住宅を建てる）か、商業の職場が減るのを待つ。
               tax が負なら商業の税を下げると、要る職場が増える。
  Industrial   workers は、働く年齢の住民から商業の分を引いた残りのうち工業へ回る分。existing が上回るなら、住民を増やす。商業を塗ると工業に回る分はさらに減る。
  Office       workers は同じ残りのうちオフィスへ回る分（工業とオフィスが両方塗られていれば 4 割）。existing が上回るなら、住民を増やす。
               働き手といまの職場が釣り合うと、demand は小さな負（-1〜-18 ほど）で揺れる。これは行き詰まりではなく釣り合いの印で、
               空き間口があれば、正になったときに建つ。
               工業とオフィスの税が負なら、その区画の税を下げる。

observe の税の影響（税率を上げたときの代償）:
  税率は基準の 10% までは影響がなく、それを超えると次の 3 つが効く。
  jobs.keptPermille  区画の種類ごとの、職場のうち雇える割合（‰）。基準を超えた分が 8.5%（税率 18.5%）のとき半分になり、なだらかに 0 へ近づく（15% で約 63%、20% で約 46%）。住宅は職場を持たないので出さない。
                     jobs.slots が雇える職場の数で、workplaces（建物の上限）より少なければ、税で人を雇えなくなっている。
                     雇えない分だけ職のない住民が増え、税収と幸福度が下がる。商業・工業・オフィスの demand もこの割合だけ小さくなる。
  taxEffect.residentialDemandShift  住宅の demand に足される税率の分。基準より高ければ負で、2‰ ごとに 1 減る。
  幸福度が低い世帯は街を出ていき、越してくる世帯も減る（migration.wantingHouseholds が減る）。

observe の signals:
  信号のある交差点ごとに、at マス、node ノードの ID、arms 腕ごとの toward（交差点から出ていく 1 歩の向き）と
  light（Green、Yellow、Red。直前の tick に効いた灯火）。

observe の removedCrosswalks:
  横断歩道を外した腕のある交差点ごとに、at マス、node ノードの ID、arms 外した腕の向き（SetCrosswalk の arm と同じ名前）。

地図（map）:
  observe と同じく何も書かず、フィードバックのディレクトリにも打てる。角 (x0,y0)〜(x1,y1) は両端を含み、順番は問わない。
  1 文字が 1 マスで、先頭に凡例、上に x、行頭に y の目盛りが付く。座標は act のコマンドと同じである。
  --scale N を付けると、N×N マスを 1 文字にまとめる（既定は 1）。まとめた文字は、
  公共施設、信号のある交差点（S s）、信号のない交差点（Z Y）、道路、駐車場（p）、建物、区画、水辺、買っていない土地、空きマスの順で、
  最初に当てはまるものにする。ただし --layer traffic では、道路の混み具合を交差点の文字より先に出す。
  建物は、電気か水が届いていないものが 1 つでもあれば * ^ % にする（まとめた文字では、セルの中の建物のうち 1 つでも届いていなければ印が付く）。
  どの網の電気や水が足りないかは observe の utilities の shortAt を見て、その at を map で眺める。
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
      既存の道路に重ねると Denied（Overlap）になる。種類を変えるときは UpgradeRoad を使う。
      始点か終点が道路のないマスで、引く向きの 1 歩外に道路（高速道路は内側の端だけ）があれば、そのマスまで 1 マス
      延ばして交差点でつなぐ（延ばした 1 マスも費用がかかる）。延ばしたときは結果の joined に実際の from と to が出る。
      平行に隣り合う道路や、1 マス離れた道路にはつながない。所持金が延ばしたぶんに足りないときも延ばさない。
      "join":false を足すと延ばさない（道路の 1 マス手前で止める行き止まりを引くときに使う。省くと true）。引いた時点でつながらなくても、同じ配列のあとのコマンドでつながれば出ない（配列の最後でどの道路にもつながらない道路にだけ、結果に warning が出る）。
  {"type":"UpgradeRoad","from":[x,y],"to":[x,y],"kind":"Small"|"Medium"|"Large"}
      直線と同じ 1 歩を通る既存の道路の区間を、区間ごと kind に変える（上げるのにも下げるのにも使う）。
      直線の形は BuildRoad と同じ決まりで、直線が区間の一部しか通らなくても、ノードからノードまでの区間全体が変わる。
      形、ノード、信号、区画、建物はそのままで、走っている車と歩く人は、いまの位置から経路を探し直す。
      費用は区間ごとの建設費の差額で、上げるときだけかかり、下げても払い戻さない。高速道路は変えない。
      変える道路がなければ Denied（NoRoadToChange）、どれもすでに kind なら Denied（RoadKindUnchanged）。
  {"type":"Bulldoze","area":[[x,y],[x,y]]}
      範囲の道路、施設、建物を取り壊す。
  {"type":"PaintZone","area":[[x,y],[x,y]],"zone":"Residential"|"Commercial"|"Industrial"|"Office"|null}
      区画を塗る。null は消す。道路から 6 マス以内の空きマスだけが塗られる。1 辺は 256 マスまで。
  {"type":"PlaceBuilding","kind":"PowerPlant"|"WaterPump","at":[x,y],"rotation":0..3}
      施設を置く。発電所は 4×4 で 60000、浄水場は 2×2 で 25000。at は間口の角のマスで、
      rotation 0、1、2、3 のとき、at から y-1、x+1、y+1、x-1 の向きに 1 歩のマスが道路でなければならない。
      敷地は at から道路と反対へ延び、間口は at から道路に沿って延びる。浄水場（2×2）の図で、A が at、# が敷地、= が道路である。
        rotation 0（道路は上）   rotation 1（道路は右）   rotation 2（道路は下）   rotation 3（道路は左）
            ==                       #A=                      ##                       =##
            A#                       ##=                      #A                       =A#
            ##                                                ==
      敷地は、rotation 0 では at から x+ と y+ へ、1 では x- と y+ へ、2 では x- と y- へ、3 では x+ と y- へ延びる。
      発電所（4×4）も同じ向きで、間口が 4 マスになる。間口のすべてのマスの 1 歩先が道路でなければならない。
  {"type":"SetSignal","at":[x,y],"on":true|false}
      マス at の交差点（3 本以上の道路が集まるノード）に信号を置くか、外す。費用はかからない。
      at の代わりに "node":{"index":n,"generation":g} でノードを指してもよい（記録に入るのはこの形）。
      マスにノードがなければ Invalid、交差点でなければ Denied（NotJunction）、変わらなければ Denied（SignalUnchanged）。
      信号は交差点の腕を向き（東西、南北、2 つの斜め）ごとにまとめ、順に青（30 tick まで）、黄 3 tick、全方向の赤 2 tick にする。
      感応式で、青の向きに交差点の手前まで来た車がいなければ、車の来ている向きへ早めに青を渡す。
      黄と赤の腕の車は、止まれるかぎり交差点の手前で止まる。いまの灯火は observe の signals にある。
  {"type":"SetCrosswalk","at":[x,y],"arm":"E"|"SE"|"S"|"SW"|"W"|"NW"|"N"|"NE","on":true|false}
      マス at の交差点の、向き arm の腕の横断歩道を外す（on が false）か、戻す（on が true）。費用はかからない。
      arm は交差点から出ていく 1 歩の向きで、E は x+1、S は y+1、W は x-1、N は y-1、SE は x+1 と y+1 である。
      at の代わりに "node":{"index":n,"generation":g} でノードを指してもよい（記録に入るのはこの形）。
      マスにノードがなければ Invalid、交差点でなければ Denied（NotJunction）、その向きに高速道路でない腕がなければ
      Denied（NoCrosswalkArm）、変わらなければ Denied（CrosswalkUnchanged）。
      外した腕では、歩く人のために車が止まらなくなる。歩く人はその腕を渡らずに交差点のまわりを回るので、遠回りになる。
      どちらに回っても越えるとき（向かい合う 2 本の腕を外して、その間を抜けるときなど）だけ、横断歩道があるときと
      同じく車を止めて渡る。経路を選ぶときは、そうして越える腕 1 本ごとに 100m 歩くぶんの時間を足すので、ほかの交差点を
      回る遠回りがそれより短ければ、その交差点を避ける。外した腕は、区間を分けても種類を変えても外したまま残り、
      腕を取り壊すと印も消える。外した腕は observe の removedCrosswalks と、map の Y と s で分かる。
  {"type":"BuyChunk","chunk":[cx,cy]}
      買った土地に辺を接するチャンクを買う。値段は observe の land.nextChunkPrice。
  {"type":"SetTaxRate","zone":"Residential"|...,"permille":0..300}
      税率を千分率で決める。
  {"type":"SetSimSpeed","speed":1..288}
      画面での速さの倍率。1 倍は実時間の 1 秒に 5 tick で、288 倍は 1 日を 5 秒で回す。advance の進み方は変わらない。

act の結果:
  results に、コマンドごとに outcome を返す。
    Applied        受け付けた
    Denied         いまの状態では受け付けなかった。reason に理由（NotOwned、NotEnoughMoney など）がある
    NotImplemented 中核にまだ中身がない
    Invalid        形か値が正しくない。message に理由がある。記録には足さない
  Applied、Denied、NotImplemented は記録に足す。画面で遊んだときと同じ扱いである。
  reason が NotOwned のときは notOwned も付き、買っていない土地のマスを示す。
    count 買っていないマスの数、tiles そのマス（先頭の 16 個まで）、
    chunks そのマスを含むチャンクのうち、いま BuyChunk で買えるもの（買った土地と辺で接する。先頭の 16 個まで）、
    chunksNotAdjacent 含むが、いまは辺で接していないので買えないもの（手前のチャンクを先に買うと買えるようになる）。
  結果は短く、index、type、outcome（と reason、message）だけを返す。--verbose を付けると、
  角の順番などをそろえて記録に入れた形のコマンドを command に添える。

export:
  種、tick、stateHash、ビルドしたコミット、commands.rec の base64 を、Issue にそのまま貼れる markdown で出す（JSON ではない）。
  貼った手順を実行すると、base64 -d で記録に戻し、再現に要る欄だけの state.json を作り、agent new --from で続きを遊べる。
  何も書かないので、フィードバックのディレクトリにも打てる。
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
    /// 再生の途中の街を置くキャッシュ。実行ファイルを読めなければ `None`。
    cache: Option<Cache>,
    /// [`Session::save`] でキャッシュに書くバイト列。
    checkpoint: Option<Vec<u8>>,
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

/// tick `tick` の街を作るときに、止まっている車の見本とキャッシュに置く街を取る tick。
fn mark_of(tick: u64, window: u64) -> u64 {
    tick - window.min(tick)
}

/// 作り直した街。
struct Rebuilt {
    world: World,
    /// 止まっている車を見分けるための、[`STOPPED_TICKS`] tick 前の車の位置。
    before: CarSample,
    /// 次の呼び出しのために書くキャッシュ。書かなくてよいときは `None`。
    checkpoint: Option<Vec<u8>>,
}

impl Saved {
    /// いまの tick の街を作る。`read` のキャッシュが使えればそこから、使えなければ種から作った街に記録を再生する。
    /// `keep` を渡すと、[`STOPPED_TICKS`] tick 前の街をそのキャッシュに書くバイト列にして返す。
    fn rebuild(&self, read: Option<&Cache>, keep: Option<&Cache>) -> Rebuilt {
        self.rebuild_over(STOPPED_TICKS, read, keep)
    }

    fn rebuild_over(&self, window: u64, read: Option<&Cache>, keep: Option<&Cache>) -> Rebuilt {
        let mark = mark_of(self.tick, window);
        let loaded = read.and_then(|c| c.load(self.seed, &self.log, mark));
        let from_cache = loaded.as_ref().map(|w| (w.tick(), w.next_seq()));
        let mut world = loaded.unwrap_or_else(|| World::new(self.seed));
        let start = world.tick().0;
        replay(&mut world, &self.log, mark - start);
        let before = world.car_sample();
        // 読んだキャッシュと同じ街を、同じファイルに書き直すことはしない。
        let unchanged = from_cache == Some((world.tick(), world.next_seq()))
            && read.zip(keep).is_some_and(|(r, k)| r.path() == k.path());
        let checkpoint = keep
            .filter(|_| !unchanged)
            .map(|c| c.encode(&world, &self.log));
        replay(&mut world, &self.log, self.tick - mark);
        Rebuilt {
            world,
            before,
            checkpoint,
        }
    }

    /// `agent new` で作ったセッションか。ロックのファイルがあるかは `locked` で受け取る。
    fn is_session(&self, locked: bool) -> bool {
        locked && self.state.contains_key(SESSION_KEY)
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
    fn create(dir: &Path, saved: Saved, checkpoint: Option<Vec<u8>>) -> Result<Session, String> {
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
            cache: Cache::here(dir),
            checkpoint,
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
        let cache = Cache::here(dir);
        let Rebuilt {
            world, checkpoint, ..
        } = saved.rebuild(cache.as_ref(), cache.as_ref());
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
            cache,
            checkpoint,
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
        // キャッシュは記録の先頭と突き合わせて使うので、記録より先に書いても後に書いてもよい。
        if let (Some(cache), Some(bytes)) = (&self.cache, self.checkpoint.take()) {
            cache.store(&bytes);
        }
        Ok(())
    }
}

fn hash_text(world: &World) -> String {
    format!("{:016x}", world.state_hash())
}

/// コマンドを 1 つずつ記録してから、記録の番号で適用する。`replay` と同じ経路なので、
/// あとで記録を再生しても同じ街になる。
///
/// 結果は短く、`verbose` のときだけ記録に入った形のコマンドを添える。
fn act(world: &mut World, log: &mut CommandLog, commands: &[Value], verbose: bool) -> Vec<Value> {
    // 引いた時点でどの道路にもつながらなかった道路。配列の最後に、あとのコマンドでつながったかを判定し直す。
    let mut isolated_roads: Vec<(usize, TilePos, TilePos)> = Vec::new();
    let mut results: Vec<Value> = commands
        .iter()
        .enumerate()
        .map(|(i, value)| {
            let kind = value.get("type").cloned().unwrap_or(Value::Null);
            let invalid = |message: String| json!({"index": i, "type": kind, "outcome": "Invalid", "message": message});
            // join は CLI だけが読む欄で、記録には入らない。
            let (value, join) = match take_join(value) {
                Ok(v) => v,
                Err(e) => return invalid(e),
            };
            let value = match command_json::resolve_node_at(&value, |t| world.network().node_at(t)) {
                Ok(v) => v,
                Err(e) => return invalid(e),
            };
            let mut command = match command_json::parse(&value) {
                Ok(c) => c,
                Err(e) => return invalid(e),
            };
            // 端のすぐ外に道路があれば、そこまで延ばしてつなぐ。記録に入るのも延ばした形である。
            let mut joined = None;
            let mut isolated = false;
            if let Command::BuildRoad { from, to, kind } = command {
                let (f, t) = if join {
                    world.joined_road(from, to, kind)
                } else {
                    (from, to)
                };
                if (f, t) != (from, to) {
                    joined = Some(json!({"from": [f.x, f.y], "to": [t.x, t.y]}));
                    command = Command::BuildRoad { from: f, to: t, kind };
                }
                isolated = !is_joined_to_network(world, f, t);
            }
            let seq = match log.record(world.tick(), command) {
                Ok(seq) => seq,
                Err(e) => return invalid(e.to_string()),
            };
            let mut out = json!({"index": i, "type": kind});
            if let Some(joined) = joined {
                out["joined"] = joined;
            }
            if verbose {
                // 角の順番などをそろえたあとの、記録に入った形を返す。
                out["command"] = command_json::to_json(command);
            }
            // 結果の種類が増えたら、ここで JSON の名前を決めさせる。
            #[deny(clippy::wildcard_enum_match_arm)]
            let outcome = match world.apply_recorded(seq, &command) {
                ApplyOutcome::Applied => "Applied",
                ApplyOutcome::NotImplemented => "NotImplemented",
                ApplyOutcome::Rejected => "Rejected",
                ApplyOutcome::Denied(reason) => {
                    out["reason"] = json!(format!("{reason:?}"));
                    if reason == DenyReason::NotOwned {
                        out["notOwned"] = not_owned(world, &command);
                    }
                    "Denied"
                }
            };
            out["outcome"] = json!(outcome);
            if isolated
                && outcome == "Applied"
                && let Command::BuildRoad { from, to, .. } = command
            {
                isolated_roads.push((i, from, to));
            }
            out
        })
        .collect();
    for (i, from, to) in isolated_roads {
        if !is_joined_after(world, from, to) {
            results[i]["warning"] = json!(ISOLATED_ROAD_WARNING);
        }
    }
    results
}

/// 引いたあとの網で、道路 `from` → `to` が、自分の線の外へ延びる別の区間につながっているか。
/// 区間の両端が線の上にあれば、その区間は自分の一部である。
fn is_joined_after(world: &World, from: TilePos, to: TilePos) -> bool {
    let net = world.network();
    let Ok(line) = Line::between(from, to) else {
        return true;
    };
    let on_line = |t: TilePos| line.position_of(t).is_some();
    line.tiles().any(|t| {
        net.segments_through(t).into_iter().any(|id| {
            net.segment(id).is_some_and(|seg| {
                [seg.a, seg.b]
                    .into_iter()
                    .any(|n| net.node(n).is_some_and(|n| !on_line(n.tile())))
            })
        })
    })
}

/// BuildRoad の `"join"`（延ばさない指定）を取り除いて返す。省くと true である。BuildRoad 以外には書けない。
fn take_join(value: &Value) -> Result<(Value, bool), String> {
    let mut out = value.clone();
    let Some(join) = out.as_object_mut().and_then(|o| o.remove("join")) else {
        return Ok((out, true));
    };
    if value.get("type").and_then(Value::as_str) != Some("BuildRoad") {
        return Err("join は BuildRoad にだけ書ける".into());
    }
    join.as_bool()
        .map(|b| (out, b))
        .ok_or_else(|| "join は true か false で書く".into())
}

/// 配列の最後になっても、引いた道路がどの道路ともつながらなかったときの知らせ。
const ISOLATED_ROAD_WARNING: &str = "既存の道路につながっていない。電気と水道が届かず、区画が建たない（端を既存の道路や高速道路の端に合わせるとつながる）";

/// 道路 `from` → `to` が、引く前の網のどこかにつながるか。道路が 1 本もない網では比べるものがないので真にする。
fn is_joined_to_network(world: &World, from: TilePos, to: TilePos) -> bool {
    let net = world.network();
    if net.segments().count() == 0 {
        return true;
    }
    Line::between(from, to).is_ok_and(|line| line.tiles().any(|t| net.is_joinable(t)))
}

/// `notOwned` に載せるマスとチャンクの数の上限。
const NOT_OWNED_LIST: usize = 16;

/// 施設の敷地のマス。`at` が間口の角で、敷地は道路（`rotation` が指す向き）と反対へ延びる。
/// 中核の敷地の求め方と同じで、テストで置いた施設の範囲と突き合わせている。
fn facility_tiles(kind: ServiceKind, at: TilePos, rotation: u8) -> Vec<TilePos> {
    let (tx, ty) = [(0, -1), (1, 0), (0, 1), (-1, 0)][usize::from(rotation & 3)];
    let (along, back) = ((-ty, tx), (-tx, -ty));
    let (w, k) = kind.size();
    let (w, k) = (w as i64, k as i64);
    (0..k)
        .flat_map(|j| {
            (0..w).map(move |i| {
                TilePos::new(
                    at.x + along.0 * i + back.0 * j,
                    at.y + along.1 * i + back.1 * j,
                )
            })
        })
        .collect()
}

/// `NotOwned` で断られたコマンドが通ろうとした、買っていない土地のマス。
/// 断られても土地の持ち主は変わらないので、断られたあとの街で調べてよい。
fn not_owned(world: &World, command: &Command) -> Value {
    let tiles: Vec<TilePos> = if let Command::BuildRoad { from, to, .. } = command {
        Line::between(*from, *to)
            .map(|line| line.tiles().collect())
            .unwrap_or_default()
    } else if let Command::PlaceBuilding { kind, at, rotation } = command {
        facility_tiles(*kind, *at, *rotation)
    } else {
        Vec::new()
    };
    let missing: Vec<TilePos> = tiles
        .into_iter()
        .filter(|t| t.chunk().is_none_or(|c| !world.is_owned(c)))
        .collect();
    let mut coords: Vec<ChunkCoord> = missing.iter().filter_map(|t| t.chunk()).collect();
    coords.sort_unstable_by_key(|c| (c.cx, c.cy));
    coords.dedup();
    // いま買えるもの（買った土地と辺で接する）と、手前を買わないと買えないもの。
    let (buyable, later): (Vec<ChunkCoord>, Vec<ChunkCoord>) =
        coords.into_iter().partition(|c| world.can_buy_chunk(*c));
    let list = |v: Vec<ChunkCoord>| -> Vec<[i64; 2]> {
        v.into_iter()
            .take(NOT_OWNED_LIST)
            .map(|c| [i64::from(c.cx), i64::from(c.cy)])
            .collect()
    };
    json!({
        "count": missing.len(),
        "tiles": missing.iter().take(NOT_OWNED_LIST).map(|t| json!([t.x, t.y])).collect::<Vec<_>>(),
        "chunks": list(buyable),
        "chunksNotAdjacent": list(later),
    })
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

/// 職場を持つ区画だけの、雇える割合（‰）。住宅は職場を持たないので出さない。
fn jobs_kept_by_zone(tax_permille: &[u16; 4]) -> Value {
    let map: Map<String, Value> = ZoneKind::ALL
        .into_iter()
        .filter(|z| *z != ZoneKind::Residential)
        .map(|z| {
            (
                format!("{z:?}"),
                jobs_kept_permille(tax_permille[z.index()]).into(),
            )
        })
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

/// 車の台数。`before` の時点から車線の同じ位置にいる車を止まっている車と数え、路肩で待つ車は別に数える。
fn cars(world: &World, before: &CarSample) -> Value {
    let now = world.car_sample();
    let stopped = now.stopped_on_road_since(before);
    let waiting = now.waiting_at_curb();
    json!({
        "total": now.len(),
        "moving": now.len() - stopped - waiting,
        "stopped": stopped,
        "waitingAtCurb": waiting,
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
    let (mut crosswalks, mut midblock) = (0, 0);
    for (_, t) in world.trips().iter() {
        match t.crossing() {
            Some(true) => midblock += 1,
            Some(false) => crosswalks += 1,
            None => {}
        }
    }
    let Rescues {
        passed_stuck,
        teleported,
        walked_instead,
    } = world.trips().rescues();
    json!({
        "cars": cars(world, before),
        "rescues": {
            "teleported": teleported,
            "walkedInstead": walked_instead,
            "passedStuck": passed_stuck,
        },
        "walkers": {"atCrosswalks": crosswalks, "midblock": midblock},
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

fn commute(c: CommuteDistances) -> Value {
    let CommuteDistances {
        commuters,
        median_m,
        under_500m,
        under_1km,
        from_1km,
    } = c;
    json!({
        "commuters": commuters,
        "medianMeters": median_m,
        "under500m": under_500m,
        "from500mTo1km": under_1km,
        "from1km": from_1km,
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

/// 建物の駐車場の埋まり具合。区画の種類ごとの台数と停めてある台数、満車の駐車場の数と、
/// いま近くのほかの建物の駐車場へ回されている車の数。
fn parking(world: &World) -> Value {
    #[derive(Default)]
    struct Zone {
        spaces: u64,
        surface: u64,
        indoor: u64,
        parked: u64,
        reserved: u64,
        full: u64,
    }
    let mut zones: [Zone; 4] = Default::default();
    for (id, b) in world.buildings().iter() {
        let layout = b.parking_layout();
        let lot = world.trips().lot(id);
        let z = &mut zones[b.zone.index()];
        let used = u64::from(lot.parked) + u64::from(lot.reserved);
        z.spaces += layout.total();
        z.surface += layout.surface;
        z.indoor += layout.indoor;
        z.parked += u64::from(lot.parked);
        z.reserved += u64::from(lot.reserved);
        z.full += u64::from(layout.total() > 0 && used >= layout.total());
    }
    // 中核に欄が足されたらここで止まり、observe に出すかを決められるよう、`..` なしで分解する。
    let tally @ ParkingTally {
        drives: _,
        parked_elsewhere: _,
        lots_full: _,
        no_car: _,
        no_car_in_use: _,
        no_car_parked_away: _,
        no_car_at_home: _,
        no_car_other: _,
    } = world.trips().parking_tally();
    let redirected = world
        .trips()
        .iter()
        .filter(|(_, t)| t.parks_elsewhere(world.network(), world.buildings()))
        .count();
    json!({
        "byZone": by_zone(|k| {
            let z = &zones[k.index()];
            json!({
                "spaces": z.spaces,
                "surface": z.surface,
                "indoor": z.indoor,
                "parked": z.parked,
                "reserved": z.reserved,
                "occupiedPermille": (z.spaces > 0).then(|| (z.parked + z.reserved) * 1000 / z.spaces),
                "fullLots": z.full,
            })
        }),
        "redirectedCars": redirected,
        "drives": tally.drives,
        "parkedElsewhere": tally.parked_elsewhere,
        "lotsFull": tally.lots_full,
        "noCar": tally.no_car,
        "noCarBreakdown": {
            "inUse": tally.no_car_in_use,
            "parkedAway": tally.no_car_parked_away,
            "carAtHome": tally.no_car_at_home,
            "other": tally.no_car_other,
        },
    })
}

/// 区画の種類ごとの止まっている間口の数に、止まっている道路網の一覧 `stalledAt` を添える。
fn growth_json(
    growth: &[GrowthBlock; 4],
    stalled: &[StalledNetwork],
    no_free_lot: &[bool; 4],
) -> Value {
    let mut out = by_zone(|z| {
        let g = growth[z.index()];
        json!({
            "fronts": g.fronts,
            "noFreeLot": no_free_lot[z.index()],
            "blocked": g.blocked,
            "noPower": g.no_power,
            "noWater": g.no_water,
        })
    });
    let stalled_at: Vec<Value> = stalled
        .iter()
        .map(|n| {
            json!({
                "at": [n.at.x, n.at.y],
                "noPower": n.no_power,
                "noWater": n.no_water,
            })
        })
        .collect();
    if let Value::Object(map) = &mut out {
        map.insert("stalledAt".to_string(), Value::Array(stalled_at));
    }
    out
}

/// 街の様子。`World` の公開の読み出し口から取れるものをそのまま並べる。
/// `before` は、止まっている車を見分けるための、少し前の車の位置である。
fn observe(world: &World, before: &CarSample) -> Value {
    let tick = world.tick();
    let time = GameTime::at(tick);
    let stats = world.city_stats();
    let demand = demand(&stats);
    let factors = demand_factors(&stats);
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
    let isolated = world.isolated_facilities();
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
        // 余りは供給量から需要を引いた値で、負なら足りない。網をまたいだ合計の余りは出さない。
        // つながっていない施設の分まで足されて、足りない網があるのに余っているように見えるからである。
        // 余りは、主の網（需要がいちばん大きい網）と、それ以外の網に分けて出す。
        let main_idx = balance.main_network(slot);
        let main = main_idx.map(|i| {
            let b = balance.components[i][slot];
            let headroom = i128::from(b.capacity) - i128::from(b.demand);
            // 割合は供給量に対する千分率で、供給量が 0 なら null。余りが 1 割を切ったら lowHeadroom を立てる（画面の知らせと同じ基準）。
            let permille = (b.capacity > 0).then(|| headroom * 1000 / i128::from(b.capacity));
            json!({
                "capacity": b.capacity,
                "demand": b.demand,
                "headroom": headroom,
                "headroomPermille": permille,
                "lowHeadroom": b.demand > 0 && headroom >= 0 && permille.is_some_and(|p| p < 100),
                // 足りない（headroom < 0）。lowHeadroom は足りないと偽になるので、足りないことはこちらで読む。
                "shortage": headroom < 0,
                "shortBy": (-headroom).max(0),
                "at": balance.spots[i].map(|p| json!([p.x, p.y])),
            })
        });
        let (mut o_networks, mut o_capacity, mut o_demand) = (0_u64, 0_u64, 0_u64);
        for (i, c) in balance.components.iter().enumerate() {
            if Some(i) != main_idx && (c[slot].capacity > 0 || c[slot].demand > 0) {
                o_networks += 1;
                o_capacity += c[slot].capacity;
                o_demand += c[slot].demand;
            }
        }
        let kind = [ServiceKind::PowerPlant, ServiceKind::WaterPump][slot];
        let isolated: Vec<Value> = isolated
            .iter()
            .filter(|f| f.kind == kind)
            .map(|f| json!({"at": [f.front.x, f.front.y]}))
            .collect();
        json!({
            "capacity": capacity,
            "demand": need,
            "main": main,
            "others": {
                "networks": o_networks,
                "capacity": o_capacity,
                "demand": o_demand,
                "headroom": i128::from(o_capacity) - i128::from(o_demand),
            },
            "shortNetworks": short.len(),
            "networksWithoutFacility": bare,
            "shortAt": short,
            "isolatedFacilities": isolated,
        })
    };
    let utilities = json!({"power": utility(0), "water": utility(1)});
    // 主の網の余りが負の種類（power、water）。lowHeadroom は足りないと偽になるので、足りないことは shortage と shortages で知らせる。
    let shortages: Vec<&str> = ["power", "water"]
        .into_iter()
        .filter(|k| utilities[k]["main"]["shortage"] == true)
        .collect();
    let (growth, stalled) = world.growth_report();
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
        "parking": parking(world),
        "commute": commute(world.commute_distances()),
        "migration": migration(world.migration_outlook()),
        "demand": by_zone(|z| demand[z.index()]),
        "demandFactors": by_zone(|z| {
            factors.zone(z.index())
                .iter()
                .map(|f| json!({"factor": f.kind.name(), "value": f.value}))
                .collect::<Vec<Value>>()
        }),
        "zoned": by_zone(|z| zoned[z.index()]),
        "jobs": {
            "workplaces": by_zone(|z| workplaces[z.index()]),
            "filled": by_zone(|z| filled[z.index()]),
            "slots": by_zone(|z| job_slots[z.index()]),
            "keptPermille": jobs_kept_by_zone(&tax_permille),
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
        "utilities": utilities,
        "shortages": shortages,
        // 需要があるのに建たないとき、新築の候補の間口のうち、電気か水道の余裕がなくて建たないものの数。
        // その網の電気か水道の余裕が足りない。区画の需要が 0 以下なら、余裕の有無にかかわらず数えない。施設を足すか、施設のある網へ道路でつなぐ。
        // 施設のない網（道路が 1 マス離れているなど）かどうかは、utilities の networksWithoutFacility で見分ける。
        // stalledAt は、新築が止まっている道路網ごとの、止まっている間口のうち座標が最小のマス at と、足りないもの。
        // 画面の「建たない理由」の知らせを押したときに映る場所と同じ。at を map で見れば、どの網かが分かる。
        "growth": growth_json(&growth, &stalled, &world.lot_shortage()),
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
        "signals": signals(world),
        "removedCrosswalks": removed_crosswalks(world),
    })
}

/// 横断歩道を外した腕のある交差点と、外した腕の向き。
fn removed_crosswalks(world: &World) -> Value {
    let net = world.network();
    let out = net
        .nodes()
        .filter(|(_, n)| n.no_crosswalk != 0)
        .map(|(id, n)| {
            let t = n.tile();
            let arms: Vec<Value> = Heading::ALL
                .into_iter()
                .filter(|h| n.crosswalk_removed(*h))
                .map(|h| json!(format!("{h:?}")))
                .collect();
            json!({
                "at": [t.x, t.y],
                "node": {"index": id.index, "generation": id.generation},
                "arms": arms,
            })
        })
        .collect();
    Value::Array(out)
}

/// 信号のある交差点と、腕ごとのいまの灯火。
fn signals(world: &World) -> Value {
    let net = world.network();
    let lights = world.trips().lights();
    let lit =
        net.nodes()
            .filter(|(_, n)| n.control == NodeControl::Signal)
            .map(|(id, n)| {
                let t = n.tile();
                let arms: Vec<Value> = lights.arm_lights(net, id)
                .into_iter()
                .map(|((dx, dy), light)| json!({"toward": [dx, dy], "light": format!("{light:?}")}))
                .collect();
                json!({
                    "at": [t.x, t.y],
                    "node": {"index": id.index, "generation": id.generation},
                    "arms": arms,
                })
            })
            .collect();
    Value::Array(lit)
}

/// 前の observe とくらべる数。observe の JSON の中の位置で指す。
const WATCHED: &[&str] = &[
    "/money",
    "/financesPerDay/income",
    "/financesPerDay/expenses",
    "/financesPerDay/net",
    "/population/citizens",
    "/population/housedCitizens",
    "/population/households",
    "/population/homes",
    "/population/employed",
    "/population/happiness",
    "/citizenPlaces/homeless",
    "/traffic/cars/total",
    "/traffic/cars/stopped",
    "/traffic/cars/waitingAtCurb",
    "/parking/redirectedCars",
    "/migration/wantingHouseholds",
    "/migration/vacantHomes",
    "/migration/homelessHouseholds",
    "/migration/blockedHouseholds",
    "/demand/Residential",
    "/demand/Commercial",
    "/demand/Industrial",
    "/demand/Office",
    "/buildings/total",
    "/buildings/byZone/Residential",
    "/buildings/byZone/Commercial",
    "/buildings/byZone/Industrial",
    "/buildings/byZone/Office",
    "/buildings/powered",
    "/buildings/watered",
    "/facilities/PowerPlant",
    "/facilities/WaterPump",
    "/utilities/power/main/headroom",
    "/utilities/power/others/headroom",
    "/utilities/power/shortNetworks",
    "/utilities/water/main/headroom",
    "/utilities/water/others/headroom",
    "/utilities/water/shortNetworks",
    "/land/milestone",
];

/// 前の observe と比べる数を、`/money` を `money`、`/demand/Office` を `demand.Office` と読み替えた名前で集める。
fn watched_values(seen: &Value) -> Map<String, Value> {
    let mut out = Map::new();
    for path in WATCHED {
        if let Some(v) = seen.pointer(path).filter(|v| v.is_number()) {
            out.insert(path[1..].replace('/', "."), v.clone());
        }
    }
    if let Some(chunks) = seen.pointer("/land/ownedChunks").and_then(Value::as_array) {
        out.insert("land.ownedChunks".into(), json!(chunks.len()));
    }
    out
}

/// 値が数なら、`to` から `from` を引いた差。
fn delta(from: &Value, to: &Value) -> Value {
    if let (Some(a), Some(b)) = (from.as_i64(), to.as_i64()) {
        // 差は i64 に収まらないことがあるので、飽和させる。
        let d = (i128::from(b) - i128::from(a)).clamp(i128::from(i64::MIN), i128::from(i64::MAX));
        return json!(d as i64);
    }
    match (from.as_f64(), to.as_f64()) {
        (Some(a), Some(b)) => json!(((b - a) * 1000.0).round() / 1000.0),
        _ => Value::Null,
    }
}

/// 前に見た数（`last`）と、いまの数の増減。
fn changes_since(last: &Value, now: &Map<String, Value>, to_tick: &Value) -> Value {
    let before = last.get("values").and_then(Value::as_object);
    let mut changes = Map::new();
    for (key, to) in now {
        // 数でない値は、前の値がなかったものとして扱う。
        let from = before.and_then(|b| b.get(key)).filter(|v| v.is_number());
        match from {
            Some(from) if from == to => {}
            Some(from) => {
                changes.insert(
                    key.clone(),
                    json!({"from": from, "to": to, "delta": delta(from, to)}),
                );
            }
            None => {
                changes.insert(key.clone(), json!({"from": null, "to": to, "delta": null}));
            }
        }
    }
    json!({
        "fromTick": last.get("tick"),
        "toTick": to_tick,
        "changes": changes,
    })
}

const LAST_OBSERVE_FILE: &str = "last-observe.json";

/// 前の observe から、いまの observe までの増減を返し、いまの数を次のために残す。
/// 書くのは `agent new` で作ったセッションだけで、そうでなければ何も書かず `Null` を返す。
fn since_last_observe(dir: &Path, seen: &Value, is_session: bool) -> Result<Value, String> {
    if !is_session {
        return Ok(Value::Null);
    }
    let path = dir.join(LAST_OBSERVE_FILE);
    let now = watched_values(seen);
    let tick = seen.get("tick").cloned().unwrap_or(Value::Null);
    // 読めない、形が違うファイルは、前の値がないものとして上書きする。
    let last: Option<Value> = std::fs::read_to_string(&path)
        .ok()
        .and_then(|t| serde_json::from_str::<Value>(&t).ok())
        .filter(|v| {
            v.get("tick").is_some_and(Value::is_u64)
                && v.get("values").is_some_and(Value::is_object)
        });
    let diff = last.map(|last| changes_since(&last, &now, &tick));
    write_last_observe(dir, &tick, now)?;
    Ok(diff.unwrap_or(Value::Null))
}

fn write_last_observe(dir: &Path, tick: &Value, values: Map<String, Value>) -> Result<(), String> {
    let path = dir.join(LAST_OBSERVE_FILE);
    let tmp = tmp_path(&path);
    let text = print(&json!({"tick": tick, "values": values}));
    std::fs::write(&tmp, text).map_err(|e| format!("{} に書けない: {e}", tmp.display()))?;
    std::fs::rename(&tmp, &path).map_err(|e| format!("{} に書けない: {e}", path.display()))
}

fn parse_u64(text: &str, what: &str) -> Result<u64, String> {
    text.parse()
        .map_err(|e| format!("{what} {text:?} を読めない: {e}"))
}

fn print(value: &Value) -> String {
    serde_json::to_string(value).expect("JSON に書けない")
}

/// 新しいセッションを作り、街の様子を返す。`from` があれば、そのディレクトリの種、tick、記録を写す。
///
/// 写すときは写し元のキャッシュを使わず、記録を先頭から再生する。写し直すのは、食い違ったセッションから
/// 記録だけで立ち直るための手段だからである。写し先には、その再生の結果でキャッシュを書く。
fn new_session(dir: &str, from: Saved, from_dir: Option<&str>) -> Result<Value, String> {
    let keep = Cache::here(Path::new(dir));
    let Rebuilt {
        world,
        before,
        checkpoint,
    } = from.rebuild(None, keep.as_ref());
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
    let mut s = Session::create(Path::new(dir), from, checkpoint)?;
    s.save(&world)?;
    let mut out = observe(&world, &before);
    // new が見せた街を、次の observe の増減の起点にする。
    write_last_observe(Path::new(dir), &out["tick"], watched_values(&out))?;
    if let Some(c) = copied {
        out["copiedFrom"] = c;
    }
    Ok(out)
}

/// 読むだけで開く。人間のフィードバックも見られるよう、セッションでなくても読む。ロックは返り値が持つ。
/// 最後の欄は、`agent new` で作ったセッションかどうか。
/// `exclusive` はロックを排他にする（observe が増減の記録を書くため）。
/// セッションならキャッシュから作り直し、排他のロックを取ったときだけキャッシュを書き直す。
fn open_for_read(
    dir: &Path,
    exclusive: bool,
) -> Result<(Option<File>, World, CarSample, bool), String> {
    let lock = lock(dir, exclusive, false)?;
    let saved = read_saved(dir)?;
    let is_session = saved.is_session(lock.is_some());
    let cache = Cache::here(dir).filter(|_| is_session);
    let keep = cache.as_ref().filter(|_| exclusive);
    let Rebuilt {
        world,
        before,
        checkpoint,
    } = saved.rebuild(cache.as_ref(), keep);
    if let (Some(c), Some(bytes)) = (keep, checkpoint) {
        c.store(&bytes);
    }
    let hash = hash_text(&world);
    if let Some(recorded) = saved.saved_hash()
        && recorded != hash
    {
        eprintln!(
            "{STATE_FILE} の stateHash {recorded} と、再生した街のハッシュ {hash} が食い違う"
        );
    }
    Ok((lock, world, before, is_session))
}

/// 再現の手順を markdown にする。何も書かない。
fn export_markdown(dir: &Path) -> Result<String, String> {
    let lock = lock(dir, false, false)?;
    let saved = read_saved(dir)?;
    let cache = Cache::here(dir).filter(|_| saved.is_session(lock.is_some()));
    let world = saved.rebuild(cache.as_ref(), None).world;
    let hash = hash_text(&world);
    let record = saved.log.encode();
    let state_json = export::repro_state(
        &saved.state,
        saved.seed,
        saved.tick,
        saved.saved_hash().unwrap_or(&hash),
    );
    Ok(export::markdown(&export::Export {
        seed: saved.seed,
        tick: saved.tick,
        recorded_hash: saved.saved_hash(),
        replayed_hash: &hash,
        commands: saved.log.len(),
        record: &record,
        state_json: &state_json,
    }))
}

/// キャッシュの鍵（実行ファイルの xxh3）をいまの実行ファイルで決める。`main` の先頭で、ロックを待つ前に呼ぶ。
pub fn fix_build_key() {
    cache::build_key();
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
        ["act", dir, commands, rest @ ..] => {
            let verbose = match rest {
                [] => false,
                ["--verbose"] => true,
                _ => return Err(USAGE.trim_end().to_string()),
            };
            let commands = read_commands(commands)?;
            let (mut s, mut world) = Session::open_for_write(Path::new(dir))?;
            let results = act(&mut world, &mut s.log, &commands, verbose);
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
            // 読み込んだときに tick まで適用してあるので、続きを回す。キャッシュは進めた先の手前で取り直す。
            let mark = mark_of(s.tick, STOPPED_TICKS);
            let start = world.tick().0;
            if mark > start {
                replay(&mut world, &s.log, mark - start);
                s.checkpoint = s.cache.as_ref().map(|c| c.encode(&world, &s.log));
            }
            let now = world.tick().0;
            replay(&mut world, &s.log, s.tick - now);
            s.save(&world)?;
            Ok(print(&json!({
                "tick": s.tick,
                "time": clock_text(GameTime::at(world.tick())),
                "money": world.money().0,
                "stateHash": hash_text(&world),
            })))
        }
        ["observe", dir] => {
            let (_lock, world, before, is_session) = open_for_read(Path::new(dir), true)?;
            let mut seen = observe(&world, &before);
            seen["sinceLastObserve"] = since_last_observe(Path::new(dir), &seen, is_session)?;
            Ok(print(&seen))
        }
        ["export", dir] => export_markdown(Path::new(dir)),
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
            let (_lock, world, _, _) = open_for_read(Path::new(dir), false)?;
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
        let results = act(&mut world, &mut log, &commands(), false);
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
        let more = [
            json!({"type":"BuildRoad","from":[30,10],"to":[30,40],"kind":"Medium"}),
            json!({"type":"UpgradeRoad","from":[0,10],"to":[30,10],"kind":"Large"}),
            json!({"type":"UpgradeRoad","from":[0,10],"to":[30,10],"kind":"Large"}),
        ];
        let results = act(&mut world, &mut log, &more, false);
        let outcomes: Vec<(&str, Option<&str>)> = results
            .iter()
            .map(|r| (r["outcome"].as_str().unwrap(), r["reason"].as_str()))
            .collect();
        assert_eq!(
            outcomes,
            [
                ("Applied", None),
                ("Applied", None),
                ("Denied", Some("RoadKindUnchanged"))
            ]
        );
        replay(&mut world, &log, 200);

        let log = CommandLog::decode(&log.encode()).unwrap();
        let mut again = World::new(seed);
        replay(&mut again, &log, 500);
        assert_eq!(again, world);
        assert_eq!(again.state_hash(), world.state_hash());
    }

    /// ヘルプの図と NotOwned のマスに使う敷地の求め方が、中核が置いた施設の範囲と同じになる。
    #[test]
    fn facility_tiles_match_the_core_site() {
        for kind in ServiceKind::ALL {
            for rotation in 0..4_u8 {
                let mut world = World::new(1);
                let mut log = CommandLog::new();
                // 道路に面するよう、向きごとに at から 1 歩先に、間口より長い道路を引く。
                let at = TilePos::new(20, 20);
                let (dx, dy) = [(0, -1), (1, 0), (0, 1), (-1, 0)][usize::from(rotation)];
                let (from, to) = if dx == 0 {
                    (TilePos::new(10, 20 + dy), TilePos::new(40, 20 + dy))
                } else {
                    (TilePos::new(20 + dx, 10), TilePos::new(20 + dx, 40))
                };
                let road = [
                    json!({"type":"BuildRoad","from":[from.x,from.y],"to":[to.x,to.y],"kind":"Small"}),
                ];
                act(&mut world, &mut log, &road, false);
                let place = [
                    json!({"type":"PlaceBuilding","kind":format!("{kind:?}"),"at":[at.x,at.y],"rotation":rotation}),
                ];
                let out = act(&mut world, &mut log, &place, false);
                assert_eq!(out[0]["outcome"], "Applied", "{kind:?} {rotation}: {out:?}");
                let area = world.facilities().last().unwrap().area;
                let mut expected: Vec<TilePos> = facility_tiles(kind, at, rotation);
                expected.sort_by_key(|t| (t.y, t.x));
                let mut actual: Vec<TilePos> = chizu_core::zone::tiles_of(area).collect();
                actual.sort_by_key(|t| (t.y, t.x));
                assert_eq!(expected, actual, "{kind:?} {rotation}");
            }
        }
    }

    /// 家と職場を車で行くほど（500m）離して置いた、長い 1 本道の街を、車が走っている朝のどこかまで進める。
    fn commuting_town(seed: u64) -> (World, CommandLog) {
        let line = chizu_core::world::highway_line(seed);
        let (x, y) = (line.start.x, line.start.y);
        let inward = -line.step.0 as i64;
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
        act(&mut world, &mut log, &cmds, false);
        let mut tick = 0;
        while tick < 5 * TICKS_PER_DAY_FOR_TEST {
            replay(&mut world, &log, 25);
            tick += 25;
            let day_tick = tick % TICKS_PER_DAY_FOR_TEST;
            if tick > 3 * TICKS_PER_DAY_FOR_TEST
                && day_tick > 2000
                && !world.car_sample().is_empty()
            {
                return (world, log);
            }
        }
        panic!("車が走らなかった");
    }

    fn saved_at(seed: u64, tick: u64, log: &CommandLog) -> Saved {
        Saved {
            state: Map::new(),
            seed,
            tick,
            log: log.clone(),
        }
    }

    /// キャッシュから作り直した街と車の見本は、種から再生したものと同じになる。
    /// キャッシュを書いたあとにコマンドを足し、時間を進めても同じである。
    #[test]
    fn rebuild_from_cache_matches_full_replay() {
        let seed = 42;
        let dir = std::env::temp_dir().join(format!("chizu-agent-cache-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let cache = Cache::new(&dir, 1);
        let (mut world, mut log) = commuting_town(seed);
        let first = world.tick().0;
        let built = saved_at(seed, first, &log).rebuild(None, Some(&cache));
        assert_eq!(built.world, world);
        cache.store(&built.checkpoint.expect("キャッシュに書くものがない"));
        let cached = saved_at(seed, first, &log).rebuild(Some(&cache), Some(&cache));
        assert_eq!(cached.world, world);
        assert_eq!(cached.before, built.before);
        assert!(!built.before.is_empty(), "車が走っていない");
        // 書いたキャッシュと同じ街なら、書き直すものはない。
        assert!(cached.checkpoint.is_none());

        // キャッシュを書いた tick より後に、コマンドを足して進める。
        let more = [json!({"type":"SetTaxRate","zone":"Residential","permille":130})];
        act(&mut world, &mut log, &more, false);
        replay(&mut world, &log, 200 - STOPPED_TICKS);
        let before = world.car_sample();
        replay(&mut world, &log, STOPPED_TICKS);
        let second = world.tick().0;
        let cached = saved_at(seed, second, &log).rebuild(Some(&cache), Some(&cache));
        assert_eq!(cached.world, world);
        assert_eq!(cached.world.state_hash(), world.state_hash());
        assert_eq!(cached.before, before);
        assert!(cached.checkpoint.is_some());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// new --from は写し元のキャッシュを使わず、記録を先頭から再生する。
    /// 写し元に、検査は通るが別の街の入ったキャッシュがあっても、写した街は記録の再生と同じになる。
    #[test]
    fn new_from_ignores_the_source_cache() {
        let seed = 42;
        let base = std::env::temp_dir().join(format!("chizu-agent-from-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let (src, dst) = (base.join("src"), base.join("dst"));
        std::fs::create_dir_all(&src).unwrap();
        // 件数の同じ 2 つの記録。道路の長さだけが違う。
        let town = |len: i64| {
            let mut log = CommandLog::new();
            let mut world = World::new(seed);
            let cmds = [
                json!({"type":"BuildRoad","from":[0,0],"to":[len,0],"kind":"Medium"}),
                json!({"type":"PaintZone","area":[[0,1],[24,6]],"zone":"Residential"}),
            ];
            act(&mut world, &mut log, &cmds, false);
            replay(&mut world, &log, 300);
            (world, log)
        };
        let (real, log) = town(20);
        let (other, _) = town(30);
        assert_ne!(real.state_hash(), other.state_hash());
        // 別の街を、写し元の記録の先頭に合わせた見出しで書く。
        let cache = Cache::here(&src).expect("実行ファイルを読めない");
        cache.store(&cache.encode(&other, &log));
        let from = saved_at(seed, 400, &log);
        // 写し元のキャッシュを読めば、別の街から続けてしまう。
        let fooled = from.rebuild(Some(&cache), None).world;
        assert_ne!(fooled.state_hash(), {
            let mut w = World::new(seed);
            replay(&mut w, &log, 400);
            w.state_hash()
        });

        let made = new_session(dst.to_str().unwrap(), from, Some(src.to_str().unwrap())).unwrap();
        let mut fresh = World::new(seed);
        replay(&mut fresh, &log, 400);
        assert_eq!(made["stateHash"], hash_text(&fresh));
        // 写し先のキャッシュは、記録の再生と同じ街になる。
        let copied = saved_at(seed, 400, &log)
            .rebuild(Cache::here(&dst).as_ref(), None)
            .world;
        assert_eq!(copied, fresh);
        std::fs::remove_dir_all(&base).unwrap();
    }

    /// 止まっている車を見分ける見本は、いまでなく `window` tick 前の車の位置である。
    #[test]
    fn sample_is_taken_a_window_before_now() {
        let seed = 42;
        let (world, log) = commuting_town(seed);
        let tick = world.tick().0;
        let saved = saved_at(seed, tick, &log);
        let Rebuilt {
            world: replayed,
            before,
            ..
        } = saved.rebuild_over(50, None, None);
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
