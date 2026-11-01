#!/usr/bin/env bash
# 人間が遊ぶ用の開発サーバーを立てっぱなしにし、origin の main が進んだらビルドし直して配信し直す。
#
# 遊ぶ用のコピーは作業ツリーとは別の場所（既定は ~/.cache/chizu-play）に 2 つ clone し、交互に使う。
# 新しい版は配信していない方でビルドし、通ったときだけサーバーをそちらで立て直す。
# 落ちたときは前の版のサーバーにも、そのファイルにも触らないので、前の版を配信し続けられる。
# cargo のビルド結果は 2 つの clone で分け合う。
#
# 使い方: scripts/play.sh [--port <番号>] [--branch <ブランチ>]
# 環境変数: CHIZU_PLAY_PORT、CHIZU_PLAY_BRANCH、CHIZU_PLAY_REMOTE（clone 元）、
#           CHIZU_PLAY_DIR（コピーの置き場所）、CHIZU_PLAY_INTERVAL（版を見る間隔の秒）、CHIZU_FEEDBACK_DIR
set -euo pipefail

log() {
  local line
  line="[play $(date '+%Y-%m-%d %H:%M:%S')] $*"
  echo "$line"
  if [ -n "${log_file:-}" ]; then
    echo "$line" >>"$log_file"
  fi
}

# 配信している版、最新の版、ビルドに失敗した版を受け、ビルドし直すべきかを返す。
# 失敗した版は、次の版が出るまでビルドし直さない
needs_update() {
  local served="$1" latest="$2" failed="$3"
  [ -n "$latest" ] && [ "$latest" != "$served" ] && [ "$latest" != "$failed" ]
}

other_slot() {
  if [ "$1" = a ]; then echo b; else echo a; fi
}

# clone の lockfile が、前に依存を入れたときから変わったか
lock_changed() {
  local dir="$1" current
  current="$(git -C "$dir" rev-parse HEAD:web/pnpm-lock.yaml)"
  [ ! -d "$dir/web/node_modules" ] || [ "$(cat "$dir/.play-lock" 2>/dev/null)" != "$current" ]
}

version_label() {
  git -C "$1" log -1 --format='%h %cd' --date='format:%Y-%m-%d %H:%M'
}

# 依存を入れ、WASM をビルドする。set -e は if の中で効かないので、手順ごとに失敗を返す
prepare_slot() {
  local dir="$play_dir/$1" sha="$2"
  if [ ! -d "$dir/.git" ]; then
    git clone --quiet "$remote" "$dir" || return 1
  fi
  git -C "$dir" fetch --quiet origin "$branch" || return 1
  git -C "$dir" reset --quiet --hard "$sha" || return 1
  if [ ! -L "$dir/target" ]; then
    mkdir -p "$play_dir/target"
    rm -rf "$dir/target"
    ln -s "$play_dir/target" "$dir/target"
  fi
  (
    cd "$dir"
    mise trust --quiet "$dir/mise.toml"
    if lock_changed "$dir"; then
      log "依存を入れる"
      mise run install:web
      git rev-parse HEAD:web/pnpm-lock.yaml >"$dir/.play-lock"
    fi
    mise run build:wasm
  ) || return 1
}

start_server() {
  local dir="$play_dir/$1" label
  label="$(version_label "$dir")"
  # 自分のプロセスグループで動かし、止めるときに vite まで含めてまとめて止める
  set -m
  (
    cd "$dir/web"
    CHIZU_PLAY_VERSION="$label" CHIZU_FEEDBACK_DIR="$feedback_dir" \
      exec mise exec -- pnpm exec vite --port "$port" --strictPort
  ) &
  server_pid=$!
  set +m
  local _
  for _ in $(seq 60); do
    if curl --silent --output /dev/null "http://localhost:$port/"; then
      log "配信している: $label（http://localhost:$port/）"
      return 0
    fi
    if ! kill -0 "$server_pid" 2>/dev/null; then
      break
    fi
    sleep 1
  done
  log "サーバーが立たなかった: $label"
  stop_server
  return 1
}

stop_server() {
  if [ -n "${server_pid:-}" ]; then
    kill -TERM -- "-$server_pid" 2>/dev/null || true
    wait "$server_pid" 2>/dev/null || true
    server_pid=""
  fi
}

on_exit() {
  trap - INT TERM
  log "止める"
  stop_server
  exit 0
}

main() {
  server_pid=""
  port="${CHIZU_PLAY_PORT:-5173}"
  branch="${CHIZU_PLAY_BRANCH:-main}"
  while [ $# -gt 0 ]; do
    case "$1" in
      --port) port="$2"; shift 2 ;;
      --branch) branch="$2"; shift 2 ;;
      *) echo "知らない引数: $1" >&2; exit 2 ;;
    esac
  done
  remote="${CHIZU_PLAY_REMOTE:-$(git remote get-url origin)}"
  play_dir="${CHIZU_PLAY_DIR:-${XDG_CACHE_HOME:-$HOME/.cache}/chizu-play}"
  local interval="${CHIZU_PLAY_INTERVAL:-60}"
  # worktree から起動しても、リポジトリ本体の feedback/ に書く
  local common
  common="$(git rev-parse --path-format=absolute --git-common-dir)"
  feedback_dir="${CHIZU_FEEDBACK_DIR:-$(dirname "$common")/feedback}"
  mkdir -p "$play_dir" "$feedback_dir"
  log_file="$play_dir/play.log"
  trap on_exit INT TERM

  local active="" served="" failed="" latest
  log "$remote の $branch を見張る。フィードバックは $feedback_dir に書く"
  while true; do
    latest="$(git ls-remote "$remote" "refs/heads/$branch" | cut -f1)" || latest=""
    if [ -z "$latest" ]; then
      log "$branch の版を取れなかった。次に見るときまで待つ"
    elif needs_update "$served" "$latest" "$failed"; then
      local next
      next="$(other_slot "$active")"
      log "ビルドする: ${latest:0:7}"
      if prepare_slot "$next" "$latest"; then
        stop_server
        if start_server "$next"; then
          active="$next"
          served="$latest"
        else
          failed="$latest"
          if [ -n "$active" ]; then
            start_server "$active" || true
          fi
        fi
      else
        failed="$latest"
        log "ビルドに失敗した: ${latest:0:7}。前の版を配信し続け、次の版を待つ"
      fi
    elif [ -n "$active" ] && ! kill -0 "$server_pid" 2>/dev/null; then
      log "サーバーが止まっていたので立て直す"
      start_server "$active" || true
    fi
    sleep "$interval" &
    wait $! || true
  done
}

if [ "${BASH_SOURCE[0]}" = "$0" ]; then
  main "$@"
fi
