#!/usr/bin/env bash
# 決定論を守るクレート（既定は chizu-core）のソースに浮動小数点が入っていないかを確かめる。
# clippy の disallowed_types は型の名前を書いた箇所しか拾わず、
# `2.5_f64.floor()` や `let a = 0.5;` のように型を推論させた浮動小数点は通ってしまう。
# そこで、型の名前（接尾辞を含む）と浮動小数点のリテラルをソースから探す。
# 行全体がコメントの行は見ない。
set -euo pipefail

dir="${1:-crates/chizu-core/src}"

# 1 つ目は f32 と f64（`2.5_f64` や `as_secs_f64` も含む）。
# 2 つ目は `0.5` や `1e3` のようなリテラル。前が `.` や英数字のものは、`t.0.1` や `0x1e3` なので除く。
# 3 つ目は `1.` で終わるリテラル。`1..5` と `1.max(2)` は除く。
pattern='(?:\b|_)f(?:32|64)\b|(?<![\w.])\d[\d_]*(?:\.\d|[eE][+-]?\d)|(?<![\w.])\d[\d_]*\.(?![\w.])'

hits="$(rg --pcre2 --line-number --with-filename --type rust "$pattern" "$dir" | rg --invert-match '^[^:]+:[0-9]+:\s*//' || true)"

if [ -n "$hits" ]; then
  echo "$dir では浮動小数点を使えない。固定小数点の整数の型を使う：" >&2
  echo "$hits" >&2
  exit 1
fi
