#!/usr/bin/env bash
# #1915 のテスト: CI のビルドキャッシュのキーへ混ぜる「ルートの Cargo.toml の指紋」
# （scripts/lib/cargo-root-manifest-key.sh）が、
#   - ビルドに効く変更（[profile.*] / [workspace.dependencies] の features）で変わる
#   - 夜間リリースの版の bump・コメント・空行・改行コードでは変わらない
#     （変わるとほぼ毎日キャッシュが当たらなくなる = 効率が落ちる）
# ことと、rust-cache を使う workflow がすべてこの指紋を key に渡していること、
# タグで起動する workflow が rust-cache を使わないこと（#1921）を確かめる。
# 本物の Cargo.toml は読むだけで、書き換えは一時 dir の写しにだけ行う。
set -uo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
KEY="$ROOT/scripts/lib/cargo-root-manifest-key.sh"
PASS=0; FAIL=0

ok()   { PASS=$((PASS+1)); echo "  PASS: $1"; }
ng()   { FAIL=$((FAIL+1)); echo "  FAIL: $1"; }
same() { if [ "$2" = "$3" ]; then ok "$1"; else ng "$1 (基準 '$2' / 実際 '$3')"; fi; }
diff_() { if [ "$2" != "$3" ]; then ok "$1"; else ng "$1 (どちらも '$2')"; fi; }

SANDBOX="$(mktemp -d "${TMPDIR:-/tmp}/tako-1915-XXXXXX")"
cleanup() { rm -rf "$SANDBOX"; }
trap cleanup EXIT

# 正規化の性質は**字面を固定した Cargo.toml**で見る。本物の中身に依存する置換（「serde の
# features が derive だけ」等）で書くと、本物を正当に変えた無関係な PR が偽の赤になる
# （#1915 の検証で、features を足した一時コミットがまさにそれで落ちた）
cat > "$SANDBOX/fixture.toml" <<'TOML'
# 先頭のコメント
[workspace]
resolver = "2"
members = ["crates/*"]

[workspace.package]
version = "1.2.3"
edition = "2021"

# リリース最適化
[profile.release]
lto = "thin"
codegen-units = 1

[workspace.dependencies]
local = { path = "crates/local" }
serde = { version = "1", features = ["derive"] }
serde_json = "1"

[workspace.dependencies.table_dep]
version = "1"
TOML

# 固定の Cargo.toml を 1 か所だけ書き換えて指紋を採る
fx() {
  sed "$2" "$SANDBOX/fixture.toml" > "$SANDBOX/fx-${1}.toml"
  bash "$KEY" "$SANDBOX/fx-${1}.toml"
}

# [workspace.package] の version だけを上げる（節を見て書き換える。本物の字面に依らない）
bump_package_version() {
  awk '
    /^[[:space:]]*\[/ { sect = $0; sub(/#.*/, "", sect); gsub(/[[:space:]]/, "", sect) }
    sect == "[workspace.package]" && /^[[:space:]]*version[[:space:]]*=/ { print "version = \"99.0.0-test\""; next }
    { print }
  ' "$1"
}

echo "== 指紋の形と決定性（本物の Cargo.toml）"
base=$(bash "$KEY" "$ROOT/Cargo.toml")
if printf '%s' "$base" | grep -Eq '^[0-9a-f]{8}$'; then ok "8 桁の 16 進（${base}）"; else ng "8 桁の 16 進ではない（'${base}'）"; fi
same "同じ入力から同じ指紋" "$base" "$(bash "$KEY" "$ROOT/Cargo.toml")"
same "引数を省くとカレントの Cargo.toml" "$base" "$(cd "$ROOT" && bash "$KEY")"

echo "== 本物の Cargo.toml: キーを変えない変更（変わると毎日キャッシュが外れる）"
bump_package_version "$ROOT/Cargo.toml" > "$SANDBOX/real-bump.toml"
if cmp -s "$ROOT/Cargo.toml" "$SANDBOX/real-bump.toml"; then
  ng "前提: [workspace.package] に version が無い（夜間リリースが上げる行が見つからない）"
else
  same "[workspace.package] の version の bump（夜間リリース）" "$base" "$(bash "$KEY" "$SANDBOX/real-bump.toml")"
fi
{ echo "# 先頭に足した検証用のコメント"; echo; cat "$ROOT/Cargo.toml"; } > "$SANDBOX/real-cmt.toml"
same "先頭へのコメントと空行の追加" "$base" "$(bash "$KEY" "$SANDBOX/real-cmt.toml")"
tr -d '\r' < "$ROOT/Cargo.toml" | sed 's/$/'"$(printf '\r')"'/' > "$SANDBOX/real-crlf.toml"
same "改行が CRLF（Windows のチェックアウト）" "$base" "$(bash "$KEY" "$SANDBOX/real-crlf.toml")"

echo "== 本物の Cargo.toml: キーを変える変更"
{ cat "$ROOT/Cargo.toml"; printf '\n[profile.test]\nopt-level = 1\n'; } > "$SANDBOX/real-prof.toml"
diff_ "末尾への [profile.*] の追加" "$base" "$(bash "$KEY" "$SANDBOX/real-prof.toml")"

echo "== 固定の Cargo.toml: キーを変えない変更"
fbase=$(bash "$KEY" "$SANDBOX/fixture.toml")
same "[workspace.package] の version の bump" "$fbase" "$(fx bump 's/^version = "1\.2\.3"$/version = "9.9.9"/')"
same "行全体のコメントの追加" "$fbase" "$(fx cmt-add '/^\[profile\.release\]/i\
# 検証用のコメント
')"
same "行全体のコメントの書き換え" "$fbase" "$(fx cmt-edit 's/^# リリース最適化$/# 書き換えたコメント/')"
same "字下げしたコメント" "$fbase" "$(fx cmt-indent 's/^# リリース最適化$/    # 字下げしたコメント/')"
same "空行の追加" "$fbase" "$(fx blank '/^\[workspace\.dependencies\]$/i\

')"
# 見出しの行末コメントは行全体のコメントではないので、それ自体は指紋を変えてよい（安全側）。
# ここで見たいのは「それでも節を見分けて version の bump を混ぜない」ことなので、
# 見出しだけ変えた写しと比べる
hdr_only=$(fx hdr-only 's/^\[workspace\.package\]$/[workspace.package] # 見出しのコメント/')
same "見出しに行末コメントがあっても version の bump は効かない" "$hdr_only" \
  "$(fx hdr-bump 's/^\[workspace\.package\]$/[workspace.package] # 見出しのコメント/; s/^version = "1\.2\.3"$/version = "9.9.9"/')"

echo "== 固定の Cargo.toml: キーを変える変更（変わらないと古いキャッシュが使われ続ける）"
diff_ "[profile.release] の値" "$fbase" "$(fx prof 's/^codegen-units = 1$/codegen-units = 16/')"
diff_ "[profile.dev] の追加" "$fbase" "$(fx prof-dev '/^\[workspace\.dependencies\]$/i\
[profile.dev]\
opt-level = 1
')"
diff_ "[workspace.dependencies] の features" "$fbase" "$(fx feat 's/features = \["derive"\]/features = ["derive", "rc"]/')"
diff_ "[workspace.dependencies] の依存の追加" "$fbase" "$(fx dep-add '/^serde_json = /a\
itoa = "1"
')"
diff_ "[workspace] の resolver" "$fbase" "$(fx resolver 's/^resolver = "2"$/resolver = "3"/')"
diff_ "行末コメント（行全体ではない）は変わる側に倒す" "$fbase" "$(fx trail 's/^lto = "thin"$/lto = "thin" # 行末/')"
# version を落とすのは [workspace.package] の中だけ（表形式の依存の version は残す）
diff_ "[workspace.package] 以外の節の version は落とさない" "$fbase" "$(fx table-dep 's/^version = "1"$/version = "2"/')"

echo "== 読めないときは止まる（空のキーで黙って通さない）"
if bash "$KEY" "$SANDBOX/no-such.toml" >/dev/null 2>"$SANDBOX/err"; then
  ng "Cargo.toml が無いのに 0 で抜けた"
else
  if grep -q 'エラー: .* が無い' "$SANDBOX/err"; then ok "Cargo.toml が無ければ非 0 + エラー文"; else ng "非 0 だがエラー文が無い"; fi
fi

echo "== workflow の配線（rust-cache を使うすべての箇所が指紋を key に渡す）"
for wf in "$ROOT"/.github/workflows/*.yml; do
  uses=$(grep -c 'uses: Swatinem/rust-cache@' "$wf")
  [ "$uses" -eq 0 ] && continue
  rel=${wf#"$ROOT"/}
  steps=$(grep -c 'run: .*scripts/lib/cargo-root-manifest-key.sh' "$wf")
  keys=$(grep -c 'key: .*\${{ steps.cargo-root.outputs.key }}' "$wf")
  same "${rel}: rust-cache ${uses} 本に指紋のステップが同数" "$uses" "$steps"
  same "${rel}: rust-cache ${uses} 本に指紋の key が同数" "$uses" "$keys"
  # 指紋のステップは rust-cache の直前に置く（間に別のステップを挟まない）
  bad=$(awk '
    /run: .*scripts\/lib\/cargo-root-manifest-key\.sh/ { armed = 1; next }
    /^[[:space:]]*- (uses|name|run):/ {
      if ($0 ~ /uses: Swatinem\/rust-cache@/) { if (!armed) print NR; armed = 0 }
      else if (armed) { print NR; armed = 0 }
    }
  ' "$wf")
  same "${rel}: 指紋のステップは rust-cache の直前" "" "$bad"
done

# GitHub のキャッシュは ref ごとに分かれ、タグの run が読めるのはそのタグと既定ブランチの
# ものだけ（別のタグのものは読めない）。夜間リリースはタグが毎晩変わるので、タグで走る
# rust-cache は一度も当たらないまま約 1.6 GB を毎回保存し、上限 10 GB の中で main / PR の
# キャッシュを押し出す（#1921 で 13 回すべて「No cache found.」を実測）。
# ref がタグになりうる起動: push の tags / tags-ignore、push の絞り込み無し
# （`on: push` / `on: [push, …]` / branches も tags も無い push:）、release、create。
# branches だけを書いた push はタグでは起動しない（GitHub の仕様）
tag_trigger() {
  awk '
    /^on:/ {
      inon = 1; rest = $0; sub(/^on:/, "", rest); sub(/#.*/, "", rest)
      if (rest ~ /(^|[^A-Za-z_])(push|release|create)([^A-Za-z_]|$)/) tag = 1
      next
    }
    inon && /^[^[:space:]#]/ { inon = 0 }
    !inon { next }
    /^  [A-Za-z_]+:/ {
      ev = $0; sub(/^  /, "", ev); sub(/:.*/, "", ev)
      if (ev == "push") push = 1
      if (ev == "release" || ev == "create") tag = 1
      next
    }
    ev == "push" && /^[[:space:]]+tags(-ignore)?:/ { tag = 1 }
    ev == "push" && /^[[:space:]]+branches(-ignore)?:/ { branches = 1 }
    END { if (tag || (push && !branches)) print "tag" }
  ' "$1"
}

echo "== タグで起動するかの判定（固定の workflow で自己検査。#1921）"
mkdir -p "$SANDBOX/wf"
printf 'on:\n  push:\n    tags: [%s]\n  workflow_dispatch:\n' "'v*'" > "$SANDBOX/wf/tags.yml"
printf 'on:\n  push:\n    branches: [main]\n  pull_request:\n' > "$SANDBOX/wf/branches.yml"
printf 'on: push\njobs:\n  a:\n    runs-on: x\n' > "$SANDBOX/wf/bare.yml"
printf 'on: [push, pull_request]\n' > "$SANDBOX/wf/inline.yml"
printf 'on:\n  push:\n  pull_request:\n    branches: [main]\n' > "$SANDBOX/wf/push-unfiltered.yml"
printf 'on:\n  push:\n    branches-ignore: [gh-pages]\n' > "$SANDBOX/wf/branches-ignore.yml"
printf 'on:\n  release:\n    types: [published]\n' > "$SANDBOX/wf/release.yml"
printf 'on:\n  pull_request:\n  workflow_dispatch:\njobs:\n  push:\n    runs-on: x\n' > "$SANDBOX/wf/pr.yml"
same "push の tags はタグで起動" "tag" "$(tag_trigger "$SANDBOX/wf/tags.yml")"
same "push の branches だけはタグで起動しない" "" "$(tag_trigger "$SANDBOX/wf/branches.yml")"
same "on: push（絞り込み無し）はタグでも起動" "tag" "$(tag_trigger "$SANDBOX/wf/bare.yml")"
same "on: [push, pull_request] はタグでも起動" "tag" "$(tag_trigger "$SANDBOX/wf/inline.yml")"
same "絞り込みの無い push:（branches は pull_request 側）はタグでも起動" "tag" "$(tag_trigger "$SANDBOX/wf/push-unfiltered.yml")"
same "push の branches-ignore だけはタグで起動しない" "" "$(tag_trigger "$SANDBOX/wf/branches-ignore.yml")"
same "release はタグで起動" "tag" "$(tag_trigger "$SANDBOX/wf/release.yml")"
same "pull_request / workflow_dispatch だけ（jobs の名前が push でも）はタグで起動しない" "" "$(tag_trigger "$SANDBOX/wf/pr.yml")"

echo "== タグで起動する workflow は rust-cache を使わない（#1921）"
same "前提: release-windows.yml はタグで起動すると判定できる" "tag" \
  "$(tag_trigger "$ROOT/.github/workflows/release-windows.yml")"
for wf in "$ROOT"/.github/workflows/*.yml; do
  [ "$(tag_trigger "$wf")" = "tag" ] || continue
  rel=${wf#"$ROOT"/}
  uses=$(grep -c 'uses: Swatinem/rust-cache@' "$wf")
  same "${rel}: タグで起動するので rust-cache は 0 本（タグのキャッシュは別のタグから読めず、保存は main / PR のキャッシュを押し出すだけ）" "0" "$uses"
done

echo
echo "PASS=${PASS} FAIL=${FAIL}"
[ "$FAIL" -eq 0 ]
