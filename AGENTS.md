# AGENTS.md

chizu は、Cities: Skylines II（CS2）の軽量版として作る 2D の都市建設シミュレーターである。
要件は `docs/requirements.md` にある。設計書ができたら `docs/design.md` に置く。

## 進め方

このリポジトリでは、マネージャー役の Claude がサブエージェントに作業を任せる。

- サブエージェントは、渡された範囲の作業だけを行う。範囲の外に手を広げたくなったら、作業はせずに報告で挙げる。
- 人間への質問はマネージャーがまとめて行う。サブエージェントは人間に直接質問しない。判断に迷った点は報告に書く。
- PR をマージするのはマネージャーだけである。サブエージェントは PR を作るところまでを行い、マージ、`gh pr review`、リポジトリ設定の変更はしない。
- 要件と食い違う変更をしたくなったら、変更はせずに理由を添えて報告する。

## 作業の手順

1. 作業は、マネージャーが用意した worktree の中で行う。すでに worktree の中にいるので、新しく worktree を作らず、本体のディレクトリにも移らない。`git worktree add` と `gwq` は使わない。
2. 変更したら `mise run test` と `mise run lint` を回し、両方が通ることを確かめる。
3. コミットメッセージは日本語で書き、末尾に `Co-Authored-By:` の行を付ける。
4. `git push -u origin <branch>` のあと `gh pr create --base main` で PR を作る。本文は日本語で短く書く。

## マージの条件

マネージャーは、次をすべて満たした PR だけをマージする。

- CI の必須チェック（`test` と `lint`）が通っている。
- コードを変える PR は、`adversarial-reviewer` のレビューを受け、本物の指摘が直っている。
- 設計の判断は、要件と「CS2 に近づける」という基準の範囲でマネージャーが行う。要件そのものを変える PR と、この基準では決められない判断を含む PR は、人間の了承を得ている。
- 1 つの PR は 1 つの目的に絞る。差分の目安は数百行で、それを大きく超える PR は分ける。

## コマンド

```sh
mise run test          # テストをすべて回す
mise run lint          # 静的検査をすべて回す
mise run test:rust     # cargo test --workspace
mise run lint:md       # markdown の検査（rumdl）
mise run lint:actions  # ワークフローの検査（jactionlint）
mise run lint:rust     # cargo fmt --check、cargo clippy、chizu-core と chizu-scenario の浮動小数点の検査
mise run lint:wasm     # wasm32 のターゲットで chizu-wasm に cargo clippy
mise run lint:web      # Biome と tsc --noEmit
mise run test:web      # Vitest
mise run test:wasm     # ビルドしたページをヘッドレスの Chrome で開き、スレッドつきの WASM が動くかを確かめる
mise run build         # build:wasm と build:web をまとめて回す
mise run build:wasm    # chizu-wasm をスレッドつきでビルドし、wasm-bindgen で web/wasm につなぎのコードを作る
mise run build:web     # Vite で web/dist にビルドする
mise run dev           # WASM をビルドしてから Vite の開発サーバーを立てる
mise run bench         # ネイティブのベンチマーク（criterion）。CI では回さない
mise run build:bench   # ベンチマークをビルドだけする。CI はこれでベンチが壊れていないかを見る
```

`test` と `lint` は、`test:*` と `lint:*` のタスクを束ねたものである。CI はそれぞれを別のジョブで呼ぶ。

WASM のスレッドに要る rustflags は `.cargo/config.toml` で wasm32 のターゲットに限って効かせている。`build-std` は `build:wasm` タスクのコマンドで渡す。
`crates/chizu-wasm` の `wasm-bindgen` の版と、`mise.toml` の `wasm-bindgen` の版はそろえる。

決まった記録を再生した状態ハッシュは `crates/chizu-core/tests/golden/` に置いてある。
シミュレーションや記録の形式を意図して変えたときは、`CHIZU_UPDATE_GOLDEN=1 cargo test -p chizu-core --test golden` で書き直し、同じ PR に含める。

三重バッファの配置は `crates/chizu-wasm/src/snapshot.rs` で決め、`web/src/protocol/snapshot-layout.json` に書き出して画面の側で読む。
配置や書き方を変えたときは `CHIZU_UPDATE_LAYOUT=1 cargo test -p chizu-wasm` でこの JSON と、画面の側のテストが読む `snapshot-fixture.bin` を書き直し、同じ PR に含める。

Rust のツールチェーンは `rust-toolchain.toml` で日付つきの nightly に固定している。
mise は `mise.toml` の `idiomatic_version_file_enable_tools` でこのファイルから版を読むので、`mise.toml` の `[tools]` には `rust` を書かない。

ツールを足すときは、`mise.toml` の `[tools]` に版を固定して書く。`npx` などで実行時に取ってこない。
タスクを足したり変えたりしたら、`.github/workflows/ci.yml` のジョブもそろえる。

## フィードバック

`mise run dev` のページで右上の「フィードバック」を押すと、ゲームが止まり、気づいたことを書く欄が出る。
送ると、開発サーバーが `feedback/<日時>-<番号>/` に次の 4 つを書く。`feedback/` は Git に入れない。

- `note.md`：書いた文
- `screenshot.png`：そのときの地図
- `state.json`：種、tick、所持金、状態ハッシュ、カメラの位置
- `commands.rec`：それまでの操作の記録

人間が見た状態は、記録を再生すれば手元で作り直せる。
`state.json` の `seed` と `tick` を渡して再生し、出たハッシュが `stateHash` と同じことを確かめる。

```sh
cargo run -p chizu-cli -- feedback/<日時>-<番号>/commands.rec <seed> <tick>
```

## 保存

ブラウザの中の保存は、種、tick、コマンドの記録だけを IndexedDB（`chizu` の `saves`）に書く。
読み込むときは、種から作った街に記録を再生して同じ状態を作り直す。

- 枠は自動保存 1 つと手動保存 3 つである。自動保存は、ゲーム内の日が変わったときと、実時間で 1 分たって何か変わっていたときに走る。
- 保存データを作るのと再生するのは `crates/chizu-wasm/src/save.rs`、画面の側は `web/src/save/` にある。IndexedDB に書くのはメインスレッドで、再生は Worker の中で少しずつ回す。
- 記録の形式の版が違う保存データは読めないものとして断る。版の運用は `crates/chizu-core/src/record.rs` の先頭にある。
- 保存データには状態ハッシュを添えてあり、読み込んで作り直した街のハッシュと食い違えば画面に示す。

## 依存の更新

依存は Renovate で上げる。設定は `renovate.json` にある。

- 毎朝 9 時前（Asia/Tokyo）に動く。脆弱性は OSV で見つけ、待たずに PR を作る。
- メジャー以外は 1 つの PR にまとめ、出てから 3 日たった版だけを使い、CI が通ったら自動でマージする。メジャーは依存ごとの PR になり、人が見てマージする。
- wasm-bindgen のクレートと mise の CLI は、別の 1 つの PR でそろえて上げる。
- `rust-toolchain.toml` の nightly は Renovate では上げない。日付を変えるときは手で書き換える。

メジャー以外は人が見ずにマージされるので、壊れたら test か lint が落ちるようにテストを保つ。

## 書き方

- コメント、ドキュメント、コミットメッセージ、PR の本文、画面の文言は日本語で書く。
- コメントには、コードを見れば分かることや、すぐ古くなる情報（日付、作業の番号など）を書かない。
- markdown のドキュメントは常体（だ・である）で書く。

## 守ること

### シミュレーションの中核は決定論的にする

同じ初期状態と同じ操作の列からは、どこで動かしても同じ結果にならなければならない。

- 座標や速度などの状態は固定小数点の整数で持つ。浮動小数点は描画側でだけ使う。
- 整数の桁あふれは、リリースビルドでも panic させる（`overflow-checks = true`）。
- 乱数は種を固定した生成器だけを使う。時刻やアドレスに依存する値を状態に混ぜない。
- 並列に計算するときも、結果が実行順に左右されないようにする。`HashMap` のように反復順が決まらない型は、反復した結果を状態に反映させる箇所では使わない。
- 状態を変える PR では、決定論のテスト（同じ種と操作列から同じ状態のハッシュになるか）が通ることを確かめる。

### 中核は描画と通信に依存しない

シミュレーションの中核のクレートは、WASM の API、DOM、ネットワークに依存しない。
同じクレートをネイティブにビルドしてテストやベンチマークを回せるように保つ。
