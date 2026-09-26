#!/usr/bin/env bash
# #1506 の実経路テスト: `tako setup --review` のプランの問いが**保存済みの値を既定に**し、
# Enter だけ・非 TTY で**前回値を保つ**ことを、隔離した HOME / TAKO_DATA_DIR / PATH で実測する。
#
# #1506 前は Claude Max の倍率の問いの既定が「3) 不明」固定で、Enter だけで
# `config.yaml` の `claude: max-5x` が `max` へ戻った（棚卸し #1500 の Z11 / R7'）。
# 修正前のビルドで症状を見るには `TAKO_BIN=<修正前の tako>` を渡す（該当の項目が FAIL する）。
#
# tako:run: bash scripts/test-setup-plan-keep-previous-1506.sh
#
# 本番の ~/Library/Application Support/tako・~/.claude・~/.zprofile には一切触れない
# （`env -i` で環境ごと差し替え、claude / codex / git / tmux / tailscale / brew はスタブ）。
#
# MCP `tako_setup` は dispatch `SetupRun` が `tako setup --yes --answers -`（回答 JSON を
# stdin）を起こすだけで `--review` の口を持たないので、同じ起動を直接叩いて照合する
# （dispatch は `/Applications/tako.app` の CLI を優先して起こすため、隔離 GUI 越しでは
# このビルドを検査できない）。
set -uo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TAKO="${TAKO_BIN:-$ROOT/target/debug/tako}"
PTY="$ROOT/scripts/lib/pty-answer.py"
PASS=0; FAIL=0

ok()   { PASS=$((PASS+1)); echo "  PASS: $1"; }
ng()   { FAIL=$((FAIL+1)); echo "  FAIL: $1"; }
check(){ if [ "$2" = "$3" ]; then ok "$1"; else ng "$1 (期待 '$3' / 実際 '$2')"; fi; }
contains(){ if grep -qF -- "$2" "$3" 2>/dev/null; then ok "$1"; else ng "$1 ('$2' が出ていない)"; fi; }
absent(){ if grep -qF -- "$2" "$3" 2>/dev/null; then ng "$1 ('$2' が出ている)"; else ok "$1"; fi; }

[ -x "$TAKO" ] || { echo "tako が無い: ${TAKO}（cargo build -p tako-cli）"; exit 1; }
[ -f "$PTY" ]  || { echo "PTY ドライバが無い: $PTY"; exit 1; }
[ -x /usr/bin/python3 ] || { echo "/usr/bin/python3 が無い環境では走らない"; exit 1; }

# 短い置き場（TAKO_DATA_DIR の下に socket を作る経路があるので長くしない）
SANDBOX="$(mktemp -d "/tmp/t1506-XXXXXX")"
trap 'rm -rf "$SANDBOX"' EXIT
BIN="$SANDBOX/bin"
HOMEDIR="$SANDBOX/h"
DATADIR="$SANDBOX/d"
CONFIG="$DATADIR/orchestrator/config.yaml"

# --- スタブ ------------------------------------------------------------------
mkenv(){ # $1 = Max|Pro（claude の subscriptionType）, $2 = codex を置くか（1 / 0。省略は 0）
  rm -rf "$BIN" "$HOMEDIR" "$DATADIR"
  mkdir -p "$BIN" "$HOMEDIR" "$DATADIR"
  cat > "$BIN/claude" <<EOF
#!/bin/sh
if [ "\${1:-}" = "auth" ] && [ "\${2:-}" = "status" ]; then
  echo '{"loggedIn":true,"authMethod":"claude.ai","subscriptionType":"$1"}'; exit 0
fi
if [ "\${1:-}" = "--version" ]; then echo "2.1.258 (Claude Code)"; exit 0; fi
if [ "\${1:-}" = "mcp" ] && [ "\${2:-}" = "list" ]; then echo "tako: /stub/tako mcp serve - Connected"; exit 0; fi
exit 0
EOF
  if [ "${2:-0}" = "1" ]; then
    # auth.json を置かない = プランを自動取得できない codex（GPT の問いが出る）
    printf '#!/bin/sh\ncase "$1 ${2:-}" in\n  "login status") echo "Logged in using ChatGPT"; exit 0;;\nesac\nexit 0\n' > "$BIN/codex"
  fi
  # exe::find（境界 B16）のログインシェル経由の解決を隔離 PATH に閉じ込める
  printf '#!/bin/sh\nif [ "$1" = "-l" ]; then shift; fi\nexec /bin/sh "$@"\n' > "$BIN/isosh"
  # 依存は揃えておく（[y/N] の導入確認を出さない = プランの問いだけを見る）
  for t in git tmux tailscale; do printf '#!/bin/sh\necho "%s (stub)"\nexit 0\n' "$t" > "$BIN/$t"; done
  printf '#!/bin/sh\nexit 0\n' > "$BIN/brew"
  chmod +x "$BIN"/*
}

isoenv(){ # 隔離環境で残りの引数を実行する
  env -i HOME="$HOMEDIR" TAKO_DATA_DIR="$DATADIR" TAKO_ISOLATED=1 \
      CODEX_HOME="$HOMEDIR/.codex" SHELL="$BIN/isosh" TAKO_TMUX_BIN="$BIN/tmux" \
      PATH="$BIN:/usr/bin:/bin:/usr/sbin:/sbin" TERM=dumb LANG=ja_JP.UTF-8 "$@"
}
# setup を前回値つきで完了させる（dispatch SetupRun / MCP tako_setup と同じ起動）
seed(){ # seed <answers json>
  printf '%s' "$1" | isoenv "$TAKO" setup --yes --answers - > "$SANDBOX/seed.log" 2>&1
  echo $?
}
# `--review` を PTY で回す。問いは `選択 [N]:` と `[y/N]:` の 2 形（スリープの
# 「レベルを選択 [0-3]」も前者に当たる）。答えを使い切った後は Enter だけ
review_pty(){ # review_pty <出力先> [答え...]
  local out="$1"; shift
  local answers=(); while [ $# -gt 0 ]; do answers+=(--answer "$1"); shift; done
  isoenv /usr/bin/python3 "$PTY" --expect '選択 [' --expect '[y/N]:' \
      ${answers[@]+"${answers[@]}"} --timeout 120 -- "$TAKO" setup --review > "$out" 2>&1
  echo $?
}
plan_of(){ # plan_of <provider> — config.yaml の provider_plans の値
  /usr/bin/python3 - "$CONFIG" "$1" <<'PY'
import re, sys
text = open(sys.argv[1], encoding="utf-8").read()
m = re.search(r"^\s+provider_plans:\n((?:\s{4,}\S.*\n)+)", text, re.M)
for line in (m.group(1).splitlines() if m else []):
    k, _, v = line.strip().partition(":")
    if k == sys.argv[2]:
        print(v.strip()); break
PY
}

echo "== 1) Claude Max: --review を全問 Enter（R7' の再現）=="
mkenv Max
check "前回値 max-5x で setup を完了" "$(seed '{"provider_plans":{"claude":"max-5x"}}')" "0"
check "前回値が保存されている" "$(plan_of claude)" "max-5x"
check "--review が完走" "$(review_pty "$SANDBOX/r1.log")" "0"
check "Enter だけでは max-5x を保つ" "$(plan_of claude)" "max-5x"
contains "倍率の問いが出た" "契約倍率を選んでください" "$SANDBOX/r1.log"
contains "既定の表記が前回値（1) Max 5x）を指す" "選択 [1]:" "$SANDBOX/r1.log"
absent "既定の表記が「3) 不明」ではない" "選択 [3]:" "$SANDBOX/r1.log"
contains "出どころを previous と出す" "[previous] Claude プラン: max-5x" "$SANDBOX/r1.log"

echo "== 2) --review を非 TTY（stdin = /dev/null）=="
# 段ごとに前回値を置き直す（前の段の結果に引きずられず、修正前のビルドでも段ごとに判定できる）
check "前回値 max-5x を置き直す" "$(seed '{"provider_plans":{"claude":"max-5x"}}')" "0"
isoenv "$TAKO" setup --review < /dev/null > "$SANDBOX/r2.log" 2>&1
check "非 TTY の --review が完走" "$?" "0"
check "EOF でも max-5x を保つ" "$(plan_of claude)" "max-5x"

echo "== 3) 標準 setup --yes と MCP tako_setup と同じ起動（--answers にプラン無し）=="
check "前回値 max-5x を置き直す" "$(seed '{"provider_plans":{"claude":"max-5x"}}')" "0"
isoenv "$TAKO" setup --yes < /dev/null > "$SANDBOX/r3.log" 2>&1
check "--yes が完走" "$?" "0"
check "--yes でも max-5x を保つ" "$(plan_of claude)" "max-5x"
contains "--yes は前回値を引き継ぐ" "[previous] Claude プラン: max-5x（detected: max）" "$SANDBOX/r3.log"
check "--answers '{}' が完走" "$(seed '{}')" "0"
check "MCP と同じ起動でも max-5x を保つ" "$(plan_of claude)" "max-5x"

echo "== 4) 明示の選択は反映される / 範囲外の番号は答えなかった扱い =="
# 問いの順: スリープのレベル → プラン → モデル → effort → 設定共有（y/N）
check "前回値 max-5x を置き直す" "$(seed '{"provider_plans":{"claude":"max-5x"}}')" "0"
check "2) Max 20x を選ぶ" "$(review_pty "$SANDBOX/r4.log" "" "2")" "0"
check "明示の選択が保存される" "$(plan_of claude)" "max-20x"
contains "出どころを input と出す" "[input] Claude プラン: max-20x" "$SANDBOX/r4.log"
check "前回値 max-20x を置き直す" "$(seed '{"provider_plans":{"claude":"max-20x"}}')" "0"
check "範囲外の 4 を打つ" "$(review_pty "$SANDBOX/r5.log" "" "4")" "0"
check "範囲外は前回値 max-20x のまま" "$(plan_of claude)" "max-20x"
contains "範囲外の既定表記は 2" "選択 [2]:" "$SANDBOX/r5.log"
contains "範囲外は previous 扱い" "[previous] Claude プラン: max-20x" "$SANDBOX/r5.log"

echo "== 5) 前回値が未知の文字列（検出値 max が優先 = 標準 setup と同じ）=="
check "未知の前回値 max-50x を置く" "$(seed '{"provider_plans":{"claude":"max-50x"}}')" "0"
check "未知の前回値が保存されている" "$(plan_of claude)" "max-50x"
check "--review が完走" "$(review_pty "$SANDBOX/r6.log")" "0"
check "未知の値は保たず検出値 max" "$(plan_of claude)" "max"
contains "未知の値は既定にしない（3) 不明）" "選択 [3]:" "$SANDBOX/r6.log"

echo "== 6) 前回値が無い初回（従来どおり 3) 不明 = max）=="
mkenv Max
check "初回の --review が完走" "$(review_pty "$SANDBOX/r7.log")" "0"
check "前回値が無ければ max" "$(plan_of claude)" "max"
contains "既定の表記は 3) 不明" "選択 [3]:" "$SANDBOX/r7.log"
contains "出どころは default" "[default] Claude プラン: max" "$SANDBOX/r7.log"

echo "== 7) Max 以外の問い（GPT: codex がプランを返さない）=="
mkenv Pro 1
check "gpt: plus で setup を完了" "$(seed '{"provider_plans":{"gpt":"plus"}}')" "0"
check "--review が完走" "$(review_pty "$SANDBOX/r8.log")" "0"
contains "GPT の問いが出た" "GPT / ChatGPT のプランを選んでください" "$SANDBOX/r8.log"
contains "GPT の既定表記は 2) Plus" "選択 [2]:" "$SANDBOX/r8.log"
check "Enter だけでは plus を保つ" "$(plan_of gpt)" "plus"
check "検出できる claude は検出値 pro" "$(plan_of claude)" "pro"
# 選択肢に無い値（検出由来の business 等）は値そのものを既定に見せて保つ
check "gpt: business で setup を完了" "$(seed '{"provider_plans":{"gpt":"business"}}')" "0"
check "--review が完走" "$(review_pty "$SANDBOX/r9.log")" "0"
contains "選択肢に無い値は値そのもので見せる" "選択 [business]:" "$SANDBOX/r9.log"
check "Enter だけでは business を保つ" "$(plan_of gpt)" "business"

echo
echo "結果: ${PASS} PASS / ${FAIL} FAIL"
[ "$FAIL" -eq 0 ]
