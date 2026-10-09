#!/usr/bin/env bash
# cargo-root-manifest-key.sh — CI のビルドキャッシュのキーへ混ぜる、ルートの Cargo.toml の指紋（Issue #1915）
#
# Swatinem/rust-cache（v2.9.2 のソースで確認）がキーの末尾へ混ぜるのは、`cargo metadata` が
# 返すワークスペースのメンバーの Cargo.toml・Cargo.lock（レジストリ由来の行だけ）・
# .cargo/config.toml・rust-toolchain(.toml) で、**ルートの仮想マニフェストはメンバーでは
# ないので入らない**（CI ログの「Lockfiles considered」にも無い）。README は
# 「Cargo.toml がどこにあっても」と書くが実装はそうなっていない。そのため [profile.*] や
# [workspace.dependencies] の features を変えてもキーが変わらず、変更前のキャッシュを毎回
# 完全一致で復元して下流を作り直し、完全一致なので保存もされない状態が続く（#1912 で実測）。
#
# この指紋を各 workflow が rust-cache の `key` 入力へ渡す。ただし次の 2 つは指紋に効かせない:
#   - [workspace.package] の version: 夜間リリースがほぼ毎日上げる（90 日で 65 回）。
#     上げて作り直しになるのはキャッシュの対象外のワークスペースのクレートだけで、
#     rust-cache もメンバー側の package.version は 0.0.0 に揃えてから混ぜている
#   - 行全体のコメントと空行: ビルドに効かない
# それ以外は 1 文字でも変われば指紋が変わる（行末コメントや書式の揺れも変わる側 = 安全側）。
# 改行の CRLF / LF は揃える（Windows のチェックアウトでも macOS と同じ指紋になる）。
#
# 使い方: bash scripts/lib/cargo-root-manifest-key.sh [Cargo.toml のパス]
#   → 8 桁の 16 進を 1 行（rust-cache 自身のハッシュ部と同じ桁数）。読めなければ非 0 で止まる
# 検証: bash scripts/test-cargo-root-manifest-key-1915.sh（CI の macOS ジョブで毎 PR 走る）
set -euo pipefail

manifest="${1:-Cargo.toml}"
if [ ! -f "$manifest" ]; then
  echo "エラー: ${manifest} が無い（ワークスペースのルートで実行する）" >&2
  exit 1
fi

# 節の見出し（[a.b] / [[a]]）は行末コメントと空白を落として比べる。
# git hash-object は stdin から読むとき改行の変換をしない（--no-filters 相当）ので、
# どの OS でも同じ入力から同じ値になる
key=$(tr -d '\r' < "$manifest" | awk '
  /^[[:space:]]*\[/ { sect = $0; sub(/#.*/, "", sect); gsub(/[[:space:]]/, "", sect) }
  /^[[:space:]]*(#.*)?$/ { next }
  sect == "[workspace.package]" && /^[[:space:]]*version[[:space:]]*=/ { next }
  { print }
' | git hash-object --stdin | cut -c1-8)

if [ "${#key}" -ne 8 ]; then
  echo "エラー: ${manifest} の指紋を作れなかった（'${key}'）" >&2
  exit 1
fi
echo "$key"
