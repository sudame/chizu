#!/usr/bin/env bash
# scripts/play.sh の小さな関数を確かめる。サーバーを立てる全体の動きは手で確かめる
set -euo pipefail

# shellcheck source=scripts/play.sh
source "$(dirname "$0")/play.sh"

failures=0
check() {
  local name="$1"
  shift
  if "$@"; then
    echo "ok: $name"
  else
    echo "NG: $name" >&2
    failures=$((failures + 1))
  fi
}
negate() { ! "$@"; }

check "まだ何も配信していなければビルドする" needs_update "" aaa ""
check "版が同じならビルドしない" negate needs_update aaa aaa ""
check "版が進んだらビルドする" needs_update aaa bbb ""
check "失敗した版はビルドし直さない" negate needs_update aaa bbb bbb
check "失敗の次の版はビルドする" needs_update aaa ccc bbb
check "版を取れなければビルドしない" negate needs_update aaa "" ""

check "最初は a を使う" [ "$(other_slot "")" = a ]
check "a の次は b" [ "$(other_slot a)" = b ]
check "b の次は a" [ "$(other_slot b)" = a ]

repo="$(mktemp -d)"
trap 'rm -rf "$repo"' EXIT
git -C "$repo" init --quiet
mkdir -p "$repo/web/node_modules"
echo one >"$repo/web/pnpm-lock.yaml"
git -C "$repo" add web/pnpm-lock.yaml
git -C "$repo" -c user.name=t -c user.email=t@example.com -c commit.gpgsign=false commit --quiet -m one
check "依存を入れた記録がなければ入れる" lock_changed "$repo"
git -C "$repo" rev-parse HEAD:web/pnpm-lock.yaml >"$repo/.play-lock"
check "lockfile が同じなら入れない" negate lock_changed "$repo"
echo two >"$repo/web/pnpm-lock.yaml"
git -C "$repo" -c user.name=t -c user.email=t@example.com -c commit.gpgsign=false commit --quiet -am two
check "lockfile が変わったら入れる" lock_changed "$repo"
git -C "$repo" rev-parse HEAD:web/pnpm-lock.yaml >"$repo/.play-lock"
rm -rf "$repo/web/node_modules"
check "node_modules がなければ入れる" lock_changed "$repo"

label_is_hash_and_time() {
  [[ "$(version_label "$1")" =~ ^[0-9a-f]{7,}\ [0-9]{4}-[0-9]{2}-[0-9]{2}\ [0-9]{2}:[0-9]{2}$ ]]
}
check "版はハッシュと日時で出す" label_is_hash_and_time "$repo"

# prepare_slot は、mise のタスクをまねた偽物の mise で回す
work="$(mktemp -d)"
trap 'rm -rf "$repo" "$work"' EXIT
mkdir -p "$work/bin"
cat >"$work/bin/mise" <<'EOF'
#!/usr/bin/env bash
if [ "$*" = "run install:web" ] && [ -n "${FAKE_INSTALL_FAIL:-}" ]; then
  exit 1
fi
EOF
chmod +x "$work/bin/mise"
PATH="$work/bin:$PATH"
remote="$repo"
branch="$(git -C "$repo" symbolic-ref --short HEAD)"
play_dir="$work/play"
sha="$(git -C "$repo" rev-parse HEAD)"
export FAKE_INSTALL_FAIL=1
check "依存を入れられなければ失敗にする" negate prepare_slot a "$sha"
unset FAKE_INSTALL_FAIL
check "依存を入れられなければ lock を書かない" [ ! -e "$play_dir/a/.play-lock" ]
check "依存を入れられれば通す" prepare_slot a "$sha"
check "依存を入れられれば lock を書く" [ -s "$play_dir/a/.play-lock" ]

check "空いていればロックを取れる" take_lock "$work"
check "動いている持ち主のロックは取れない" negate bash -c "source '$(dirname "$0")/play.sh'; take_lock '$work'"
echo 999999 >"$work/.play-running/pid"
check "持ち主が死んでいればロックを取り直す" take_lock "$work"
release_lock "$work"
check "ロックを外せる" [ ! -e "$work/.play-running" ]

if [ "$failures" -gt 0 ]; then
  echo "$failures 件が通らなかった" >&2
  exit 1
fi
