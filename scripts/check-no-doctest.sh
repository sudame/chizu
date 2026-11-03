#!/usr/bin/env bash
# ドキュメントのコメントに、rustdoc がテストとして回すコードのブロックがないかを確かめる。
# test:rust は nextest で回し、nextest は doctest を回さない。doctest を回すには cargo test --doc を足すが、
# cdylib と bin のクレートを外してビルドするので、依存まで別の成果物になり、ビルドし直しになる。doctest がないうちは回さず、ここで気づけるようにする。
# 言語の指定がないブロックと、rust や no_run などの属性だけのブロックを doctest とみなす。text や ignore、ほかの言語のブロックは回らない。
set -euo pipefail

dir="${1:-crates}"

hits="$(
  rg --files --type rust "$dir" | sort | while read -r file; do
    awk -v file="$file" '
      /^[[:space:]]*\/\/[\/!]/ {
        line = $0
        sub(/^[[:space:]]*\/\/[\/!][[:space:]]?/, "", line)
        if (match(line, /^[[:space:]]*(```+|~~~+)/)) {
          if (in_block) {
            in_block = 0
            next
          }
          in_block = 1
          info = substr(line, RSTART + RLENGTH)
          gsub(/[[:space:]{}.]/, "", info)
          n = split(info, tokens, ",")
          tested = 1
          for (i = 1; i <= n; i++) {
            t = tokens[i]
            if (t == "") continue
            if (t == "ignore") { tested = 0; break }
            if (t !~ /^(rust|no_run|should_panic|compile_fail|test_harness|standalone_crate|edition[0-9]+)$/) { tested = 0; break }
          }
          if (tested) print file ":" NR ": " $0
        }
        next
      }
      { in_block = 0 }
    ' "$file"
  done
)"

if [ -n "$hits" ]; then
  echo "doctest になるコードのブロックがある。doctest として回すなら、mise.toml の test:rust に cargo test --workspace --doc を足し、この検査を外す。回さないなら、text などの言語を指定する：" >&2
  echo "$hits" >&2
  exit 1
fi
