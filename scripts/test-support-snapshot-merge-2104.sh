#!/usr/bin/env bash
# tako:run: bash scripts/test-support-snapshot-merge-2104.sh
# test-support-snapshot-merge-2104.sh — 対応状況ページのデータが、並列の PR どうしで衝突しない形を保つ（#2104）
#
# なぜ要るか:
#   以前は生成したページ（windows-support.md / agent-support.md）そのものをコミットしていた。
#   ページには「全体」の件数・節ごとの件数のように対応表のどの行を直しても書き換わる行と、
#   対応表では離れているのにページでは隣り合う行があり、対応表の別々の行を直す PR が並ぶと
#   後の PR が毎回そこで衝突した（2026-10-15 の 1 晩で 3 回）。今はマトリクスの写し
#   （docs/src/data/*.json。件数を持たず 1 行 = 1 フィールド）をコミットし、ページはビルドが組む。
#   写しを 1 行 = 1 機能に詰める・件数を戻す、といった変更で同じ衝突が戻ったらここで落ちる。
#
# 何をするか:
#   偽のリポジトリに生成器（scripts/gen-*-support-docs.mjs と docs/scripts/support-pages.mjs）を置き、
#   tako のスタブ（マトリクスを JSON で返す）で「対応表の別々の行を直す PR」を 2 本模す。
#   base からブランチを 2 本切り、それぞれ生成し直してコミットし、main へ順に merge する。
#   後の方が衝突しないこと・merge 後の写しが「両方を直したマトリクス」と同期している
#   （--check が緑）こと・ページに両方の変更が載ることを見る。同じ行を両方が直したとき
#   （対応表そのものも衝突する）は、生成し直すだけで解けることを見る。
#   本物の tako は使わない（git と node だけで動く。マトリクスは今の写しから組む）。
#
# 使い方:
#   bash scripts/test-support-snapshot-merge-2104.sh
#   TAKO_2104_GEN_REF=<ref> bash scripts/test-support-snapshot-merge-2104.sh
#       その版の生成器で同じ実演をする（修正前の再現用。期待は「後の PR が衝突するか、
#       merge は通っても同期していない」に反転する）
set -uo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
GEN_REF="${TAKO_2104_GEN_REF:-}"
PASS=0; FAIL=0

ok() { PASS=$((PASS + 1)); echo "  PASS: $1"; }
ng() { FAIL=$((FAIL + 1)); echo "  FAIL: $1"; }

command -v node >/dev/null 2>&1 || { echo "node が無い環境では走らない"; exit 1; }
for f in docs/src/data/windows-support.json docs/src/data/agent-support.json; do
  [ -f "${ROOT}/${f}" ] || { echo "写しが無い: ${f}（node scripts/gen-*-support-docs.mjs）"; exit 1; }
done

SB="$(mktemp -d "${TMPDIR:-/tmp}/tako-2104-XXXXXX")"
trap 'rm -rf "${SB}"' EXIT
REPO="${SB}/repo"
FX="${SB}/fx"
mkdir -p "${REPO}/scripts/lib" "${REPO}/docs/scripts" "${REPO}/docs/src/data" \
  "${REPO}/docs/src/content/docs" "${SB}/bin" "${FX}"

# 生成器を偽のリポジトリへ置く。GEN_REF があればその版のものを git から取る
put() { # $1=リポジトリ内のパス。その版に無ければ置かない
  if [ -n "${GEN_REF}" ]; then
    git -C "${ROOT}" show "${GEN_REF}:$1" > "${REPO}/$1" 2>/dev/null || rm -f "${REPO}/$1"
  else
    [ -f "${ROOT}/$1" ] && cp "${ROOT}/$1" "${REPO}/$1"
  fi
  return 0
}
for p in scripts/gen-windows-support-docs.mjs scripts/gen-agent-support-docs.mjs scripts/lib/tako-bin.mjs \
  docs/scripts/support-pages.mjs docs/.gitignore; do
  put "${p}"
done
if [ -n "${GEN_REF}" ]; then
  LABEL="${GEN_REF} の生成器"; EXPECT_PARALLEL="broken"
else
  LABEL="作業ツリーの生成器"; EXPECT_PARALLEL="clean"
fi
echo "生成器: ${LABEL}（別々の行を直す 2 本の期待: ${EXPECT_PARALLEL}）"

cat > "${REPO}/Cargo.toml" <<'TOML'
[workspace.package]
version = "9.9.9"
TOML

# tako のスタブ。返すマトリクスは TAKO_2104_FX が指す dir の JSON
cat > "${SB}/bin/tako" <<'STUB'
#!/bin/bash
case "$1" in
  --version) echo "tako 9.9.9" ;;
  platform) cat "${TAKO_2104_FX}/windows.json" ;;
  agent-support) cat "${TAKO_2104_FX}/agent.json" ;;
  *) echo "想定外の呼び出し: $*" >&2; exit 2 ;;
esac
STUB
chmod +x "${SB}/bin/tako"
export TAKO_BIN="${SB}/bin/tako"

# マトリクスの組み立てと書き換え（tako の --json と同じ形。counts / total は数え直す）
cat > "${SB}/fx.mjs" <<'JS'
import { mkdirSync, readFileSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';

const [, , cmd, dir, ...ops] = process.argv;
const STATUSES = ['supported', 'degraded', 'pending', 'unsupported'];
const tally = (xs) => Object.fromEntries(STATUSES.map((s) => [s, xs.filter((x) => x === s).length]));
const NOTE = '（#2104 の実演）';

function save(win, agent) {
  mkdirSync(dir, { recursive: true });
  win.counts = tally(win.features.map((f) => f.status));
  win.total = win.features.length;
  agent.counts = Object.fromEntries(agent.agents.map((a) => [a.key, tally(agent.features.map((f) => f.agents[a.key].status))]));
  agent.matrix_total = agent.features.length;
  agent.total = agent.features.length;
  writeFileSync(join(dir, 'windows.json'), JSON.stringify(win));
  writeFileSync(join(dir, 'agent.json'), JSON.stringify(agent));
}
const load = (from) => [
  JSON.parse(readFileSync(join(from, 'windows.json'), 'utf8')),
  JSON.parse(readFileSync(join(from, 'agent.json'), 'utf8')),
];
const win1 = (win, key) => win.features.find((f) => f.key === key) ?? fail(`windows に ${key} が無い`);
function fail(msg) { console.error(msg); process.exit(1); }

if (cmd === 'init') {
  // ops[0] = 写しのある dir（docs/src/data）
  const w = JSON.parse(readFileSync(join(ops[0], 'windows-support.json'), 'utf8'));
  const a = JSON.parse(readFileSync(join(ops[0], 'agent-support.json'), 'utf8'));
  save({ platform: 'windows', features: w.features }, { agents: a.agents, features: a.features });
} else if (cmd === 'edit') {
  // ops[0] = 元の dir、以降は「操作:引数」
  const [win, agent] = load(ops.shift());
  for (const op of ops) {
    const [name, key, arg] = op.split(':');
    if (name === 'flip') {
      // 状態を変える（対応 ⇄ 一部対応）。件数の行が必ず変わる
      const f = win1(win, key);
      if (f.status === 'supported') Object.assign(f, { status: 'degraded', note_ja: f.note_ja ?? NOTE });
      else f.status = 'supported';
    } else if (name === 'detail') {
      // 機能の最後のフィールド（根拠）だけを書き換える
      const f = win1(win, key);
      f.evidence_detail = `${f.evidence_detail ?? ''}${arg}`;
    } else if (name === 'note') {
      const f = win1(win, key);
      f.note_ja = `${f.note_ja ?? ''}${arg}`;
    } else if (name === 'add') {
      // 行を足す（対応表は key の辞書順に並ぶ = support.rs の並び）
      win.features.push({ key, status: 'pending', note_ja: NOTE, evidence: 'unverified', evidence_detail: NOTE });
      win.features.sort((x, y) => (x.key < y.key ? -1 : x.key > y.key ? 1 : 0));
    } else if (name === 'agent-flip') {
      // agent 能力マトリクスの 1 マス（arg = 系統）
      const f = agent.features.find((g) => g.key === key) ?? fail(`agent に ${key} が無い`);
      const cell = f.agents[arg] ?? fail(`${key} に ${arg} の列が無い`);
      if (cell.status === 'supported') Object.assign(cell, { status: 'pending', note_ja: NOTE, issue: 2104 });
      else f.agents[arg] = { status: 'supported' };
    } else {
      fail(`知らない操作: ${op}`);
    }
  }
  save(win, agent);
} else {
  fail(`知らないコマンド: ${cmd}`);
}
JS
fx() { node "${SB}/fx.mjs" "$@" || exit 1; }
fx init "${FX}/base" "${ROOT}/docs/src/data"

gen() { # $1=マトリクスの dir。生成し直す（ページか写しのどちらがコミット対象かは生成器の版しだい）
  ( cd "${REPO}" && TAKO_2104_FX="$1" node scripts/gen-windows-support-docs.mjs \
    && TAKO_2104_FX="$1" node scripts/gen-agent-support-docs.mjs ) >"${SB}/gen.log" 2>&1
}
check() { # $1=マトリクスの dir。--check の終了コードを返す（出力は ${SB}/check.log）
  ( cd "${REPO}" && TAKO_2104_FX="$1" node scripts/gen-windows-support-docs.mjs --check \
    && TAKO_2104_FX="$1" node scripts/gen-agent-support-docs.mjs --check ) >"${SB}/check.log" 2>&1
}
g() { git -C "${REPO}" "$@"; }
commit() { g add -A && g commit -q -m "$1"; }

# 分類（CATEGORIES）へ新しいキーを 1 行足す。分類を持つファイルは生成器の版で違うので探す
add_category() { # $1=節の見出し $2=キー
  local file
  file="$(cd "${REPO}" && grep -lF "['$1', [" scripts/gen-windows-support-docs.mjs docs/scripts/support-pages.mjs 2>/dev/null | head -1)"
  [ -n "${file}" ] || { echo "節「$1」の分類が見つからない"; exit 1; }
  node -e '
const fs = require("node:fs");
const [file, head, key] = process.argv.slice(1);
const lines = fs.readFileSync(file, "utf8").split("\n");
const i = lines.findIndex((l) => l.includes(`[\x27${head}\x27, [`));
lines.splice(i + 1, 0, `    \x27${key}\x27,`);
fs.writeFileSync(file, lines.join("\n"));
' "${REPO}/${file}" "$1" "$2"
}

g init -q -b main
# 手元の git の設定（rerere・署名・フック）に結果を左右させない。CI のランナーには
# 名前の設定が無いので、merge コミットが作れるよう偽の名前を置く
g config user.name t
g config user.email t@example.invalid
g config rerere.enabled false
g config commit.gpgsign false
g config core.hooksPath /dev/null
gen "${FX}/base" || { cat "${SB}/gen.log"; exit 1; }
commit "base"
BASE="$(g rev-parse HEAD)"
TRACKED="$(g ls-files docs | grep -E 'support\.(md|json)$' | tr '\n' ' ')"
echo "コミットされる生成物: ${TRACKED}"

# 1 本の実演: base から A / B を切って main へ順に merge する。結果は 3 通り:
#   clean    = 後の PR も衝突せず merge でき、merge 後が両方を直したマトリクスと同期している（--check 緑）
#   unsynced = git は衝突させないが、merge 後が同期していない（両側の件数の行が**同じ値**に変わると
#              git は同じ変更として通すが、正しい値は両方を足したもの = main の CI が赤になる）
#   conflict = 後の PR が衝突する
# $1=名前 $2=A の操作（空白区切り） $3=B の操作 $4=A の分類追加（節|キー） $5=B の分類追加
# $6=期待（clean / broken = unsynced か conflict / conflict）
scenario() {
  local name="$1" ops_a="$2" ops_b="$3" cat_a="$4" cat_b="$5" expect="$6" result files spec side what
  echo "== ${name} =="
  # shellcheck disable=SC2086 # 操作は空白区切りで渡す
  fx edit "${FX}/${name}-a" "${FX}/base" ${ops_a}
  # shellcheck disable=SC2086
  fx edit "${FX}/${name}-b" "${FX}/base" ${ops_b}
  # shellcheck disable=SC2086
  fx edit "${FX}/${name}-ab" "${FX}/base" ${ops_a} ${ops_b}
  for side in a b; do
    g checkout -q -b "${name}-${side}" "${BASE}"
    if [ "${side}" = a ]; then spec="${cat_a}"; else spec="${cat_b}"; fi
    [ -n "${spec}" ] && add_category "${spec%%|*}" "${spec#*|}"
    gen "${FX}/${name}-${side}" || { cat "${SB}/gen.log"; ng "${name}: ${side} の生成が通らない"; return; }
    commit "${name}-${side}"
  done
  g checkout -q -b "main-${name}" "${BASE}"
  g merge -q --no-ff --no-edit "${name}-a" >/dev/null 2>&1 || { ng "${name}: 先の PR の merge が通らない"; return; }
  files=""
  if g merge -q --no-ff --no-edit "${name}-b" >"${SB}/merge.log" 2>&1; then
    if check "${FX}/${name}-ab"; then result="clean"; else result="unsynced"; fi
  else
    result="conflict"
    files="$(g diff --name-only --diff-filter=U | tr '\n' ' ')"
  fi
  case "${result}" in
    clean) what="後の PR も衝突せず merge でき、merge 後も同期している（--check 緑）" ;;
    unsynced) what="git は衝突させないが merge 後が同期していない（--check 赤: $(head -1 "${SB}/check.log")）" ;;
    conflict) what="後の PR が衝突する（${files% }）" ;;
  esac
  case "${expect}:${result}" in
    clean:clean | conflict:conflict | broken:unsynced | broken:conflict) ok "${name}: ${what}" ;;
    *) ng "${name}: ${what}（期待 ${expect}）" ;;
  esac
  [ "${result}" = "conflict" ] || return 0
  if [ -n "${GEN_REF}" ]; then g merge --abort; return 0; fi
  # 対応表そのものも衝突する形（同じ行を両方が直す）。対応表の衝突を解いた後の
  # マトリクスで生成し直すだけで解けることを見る（写しを手で直さない）
  gen "${FX}/${name}-ab" || { cat "${SB}/gen.log"; ng "${name}: 生成し直しが通らない"; g merge --abort; return; }
  if [ -n "$(g diff --name-only --diff-filter=U -- . ':!docs/src/data')" ]; then
    ng "${name}: 写しの外にも衝突が残る"; g merge --abort; return
  fi
  commit "${name}: merge"
  if check "${FX}/${name}-ab"; then
    ok "${name}: 生成し直すだけで衝突が解け、両方を直したマトリクスと同期する（--check 緑）"
  else
    ng "${name}: 生成し直しても同期しない: $(head -1 "${SB}/check.log")"
  fi
}

if [ -z "${GEN_REF}" ]; then
  case "${TRACKED}" in
    *support.md*) ng "ページ（*.md）がコミットされている: ${TRACKED}" ;;
    *) ok "コミットされるのは写し（*.json）だけ" ;;
  esac
fi

# 10/15 の #2093 → #2095 型: 別々の節の行の状態を変える（件数の行が両方で変わる）
scenario status-two-categories "flip:tako_split_pane" "flip:tako_git_log" "" "" "${EXPECT_PARALLEL}"
# 10/15 の #2099 / #2102 型: 対応表で隣り合う 2 行。A は前の行の最後のフィールド、
# B は次の行の最初のフィールド（状態）= 写しで最も近い 2 か所
scenario adjacent-rows "detail:tako_lsp:（A）" "flip:tako_lsp_server" "" "" "${EXPECT_PARALLEL}"
# 行を足す（対応表の別々の場所 + 分類の別々の節）
scenario add-rows "add:tako_demo_a" "add:tako_tmux_demo_b" "OS 連携|tako_demo_a" "永続化とセッション|tako_tmux_demo_b" "${EXPECT_PARALLEL}"
# agent 能力マトリクスの別々のマス（系統ごとの件数の行が両方で変わる）
scenario agent-cells "agent-flip:worker_spawn:codex" "agent-flip:master_launch:agy" "" "" "${EXPECT_PARALLEL}"
# 同じ行を両方が直す（対応表 support.rs そのものも衝突する形）
scenario same-row "detail:tako_persist:（A）" "detail:tako_persist:（B）" "" "" "conflict"

if [ -z "${GEN_REF}" ]; then
  echo "== ページと番犬 =="
  # merge 後のページに両方の変更が載る（ページはビルドと同じ部品で写しから組む）
  g checkout -q main-status-two-categories
  if ( cd "${REPO}" && node docs/scripts/support-pages.mjs ) >"${SB}/pages.log" 2>&1; then
    page="${REPO}/docs/src/content/docs/windows-support.md"
    # 期待する状態の表示は「両方を直したマトリクス」から引く（今の状態に依らない）
    label() { node -e '
const j = JSON.parse(require("node:fs").readFileSync(process.argv[1], "utf8"));
const f = j.features.find((x) => x.key === process.argv[2]);
console.log(f.status === "supported" ? "対応" : "一部対応");
' "${FX}/status-two-categories-ab/windows.json" "$1"; }
    if grep -qF "| \`tako_split_pane\` | $(label tako_split_pane) |" "${page}" \
      && grep -qF "| \`tako_git_log\` | $(label tako_git_log) |" "${page}"; then
      ok "merge 後のページに両方の PR の行が載る"
    else
      ng "merge 後のページに片方の行しか無い"
    fi
  else
    ng "ページを組めない: $(head -1 "${SB}/pages.log")"
  fi
  # 番犬の意味: マトリクスが写しより 1 行先へ進んだら --check が落ちる
  fx edit "${FX}/drift" "${FX}/status-two-categories-ab" "flip:tako_persist"
  if check "${FX}/drift"; then
    ng "写しより進んだマトリクスで --check が緑のまま"
  elif grep -qF '同期していません' "${SB}/check.log"; then
    ok "写しより 1 行進んだマトリクスで --check が落ちる"
  else
    ng "--check の落ち方が違う: $(head -1 "${SB}/check.log")"
  fi
fi

echo
echo "PASS=${PASS} FAIL=${FAIL}"
[ "${FAIL}" -eq 0 ]
