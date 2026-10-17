#!/usr/bin/env bash
# #2292 の実経路テスト: 新品の HOME（`~/.claude` が無い）の 1 回目の `tako setup` が tako mod を
# claude の設定 dir の `skills/tako` へ入れ、最終サマリにその変更を出すことを、隔離した
# HOME / TAKO_DATA_DIR / PATH で実測する。入れない条件（claude が無い・版が下限未満・
# 設定 dir が無い）では 1 回目も入れない = 判定を緩めていないことも見る。
#
# #2292 前は mod の段が `~/.claude/CLAUDE.md` を書く段より前に走り、1 回目は設定 dir が無いので
# 黙って入らず、2 回目に入っても最終サマリが「変更なし」と言っていた。
#
# 本番の ~/Library/Application Support/tako・~/.claude・シェルの rc には触れない
# （`env -i` で環境ごと差し替え、claude / codex / tmux はスタブ）。
# 修正前の tako で落ちることは `TAKO_BIN=<修正前の tako> bash scripts/test-setup-mod-first-run-2292.sh` で見る
set -uo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TAKO="${TAKO_BIN:-$ROOT/target/debug/tako}"
PASS=0; FAIL=0

ok()   { PASS=$((PASS+1)); echo "  PASS: $1"; }
ng()   { FAIL=$((FAIL+1)); echo "  FAIL: $1"; }
check(){ if [ "$2" = "$3" ]; then ok "$1"; else ng "$1 (期待 '$3' / 実際 '$2')"; fi; }
contains(){ if grep -qF -- "$2" "$3" 2>/dev/null; then ok "$1"; else ng "$1 ('$2' が出ていない)"; fi; }
absent(){ if grep -qF -- "$2" "$3" 2>/dev/null; then ng "$1 ('$2' が出ている)"; else ok "$1"; fi; }
matches(){ if grep -qE -- "$2" "$3" 2>/dev/null; then ok "$1"; else ng "$1 (/$2/ に合う行が無い)"; fi; }
exists(){ if [ -e "$2" ]; then ok "$1"; else ng "$1 (${2#"$SANDBOX"/} が無い)"; fi; }
missing(){ if [ -e "$2" ]; then ng "$1 (${2#"$SANDBOX"/} が在る)"; else ok "$1"; fi; }
# 固定文字列が最初に現れる行番号（無ければ 0）
line_of(){ awk -v pat="$1" 'index($0, pat) { print NR; found = 1; exit } END { if (!found) print 0 }' "$2"; }
no_launch(){ # スタブが答えなかった呼び出し（= 対話起動）が無い
  if [ -e "$HOMEDIR/unexpected-agent-launch" ]; then
    ng "$1 ($(tr '\n' ' ' < "$HOMEDIR/unexpected-agent-launch"))"
  else
    ok "$1"
  fi
}

# 管理印のチェックサム（無ければ -）
mark_sum(){ if [ -f "$MARK" ]; then cksum < "$MARK"; else echo "-"; fi; }

[ -x "$TAKO" ] || { echo "tako が無い: ${TAKO}（cargo build -p tako-cli）"; exit 1; }

# TMPDIR の末尾の / を落とす（HOME に // が入ると `~` の短縮が効かず、サマリのキーが比べられない）
TMPROOT="${TMPDIR:-/tmp}"
SANDBOX="$(mktemp -d "${TMPROOT%/}/tako-2292-XXXXXX")"
trap 'rm -rf "$SANDBOX"' EXIT
BIN="$SANDBOX/bin"
HOMEDIR="$SANDBOX/home"
DATADIR="$SANDBOX/d"
MARK="$HOMEDIR/.claude/skills/tako/.tako-managed"
SUMMARY_KEY="~/.claude/skills/tako:"

# mkenv <claude の版 | none> [codex]
mkenv(){
  rm -rf "$BIN" "$HOMEDIR" "$DATADIR"
  mkdir -p "$BIN" "$HOMEDIR" "$DATADIR"
  # exe::find のログインシェル経由の解決を隔離 PATH に閉じ込める
  printf '#!/bin/sh\nif [ "$1" = "-l" ]; then shift; fi\nexec /bin/sh "$@"\n' > "$BIN/isosh"
  # tmux は在る扱い（依存の段の [y/N] を出さない）
  printf '#!/bin/sh\necho "tmux 3.5a"\n' > "$BIN/tmux"
  if [ "$1" != none ]; then
    # 認証済みの claude（setup の非対話の問い合わせに即答し、それ以外 = 対話起動を残す）
    cat > "$BIN/claude" <<EOF
#!/bin/sh
if [ "\$#" -eq 1 ] && [ "\$1" = "--version" ]; then echo "$1 (Claude Code)"; exit 0; fi
case "\${1:-} \${2:-}" in
  "auth status") echo '{"loggedIn":true,"authMethod":"claude.ai","subscriptionType":"pro"}'; exit 0;;
  "mcp list") echo "tako"; exit 0;;
esac
echo "claude argv=[\$*]" >> "\$HOME/unexpected-agent-launch"
exit 0
EOF
  fi
  if [ "${2:-}" = codex ]; then
    cat > "$BIN/codex" <<'EOF'
#!/bin/sh
case "${1:-} ${2:-}" in
  "login status") exit 0;;
  "debug models") exit 0;;
esac
[ "${1:-}" = "mcp" ] && exit 0
echo "codex argv=[$*]" >> "$HOME/unexpected-agent-launch"
exit 0
EOF
    mkdir -p "$HOMEDIR/.codex"
    printf '%s\n' '{"tokens":{"id_token":"header.eyJodHRwczovL2FwaS5vcGVuYWkuY29tL2F1dGgiOnsiY2hhdGdwdF9wbGFuX3R5cGUiOiJwbHVzIn19.signature"}}' \
      > "$HOMEDIR/.codex/auth.json"
  fi
  chmod +x "$BIN"/*
}

# run_setup <出力先> <stdin> [VAR=値 ...] -- <tako の引数...>
run_setup(){
  local out="$1"; local input="$2"; shift 2
  local extra=(); while [ "${1:-}" != "--" ]; do extra+=("$1"); shift; done; shift
  printf '%s' "$input" | env -i HOME="$HOMEDIR" TAKO_DATA_DIR="$DATADIR" TAKO_ISOLATED=1 \
      SHELL="$BIN/isosh" TAKO_TMUX_BIN="$BIN/tmux" \
      PATH="$BIN:/usr/bin:/bin:/usr/sbin:/sbin" TERM=dumb LANG=ja_JP.UTF-8 \
      ${extra[@]+"${extra[@]}"} "$TAKO" "$@" > "$out" 2>&1
  echo $?
}

echo "== 1) 新品の HOME（claude 2.1.301 / 認証済み）: 1 回目で入り、サマリに出る =="
mkenv 2.1.301
missing  "前提: ~/.claude が無い"                     "$HOMEDIR/.claude"
RC=$(run_setup "$SANDBOX/1a.log" "" -- setup)
check    "1 回目: 終了コード 0"                       "$RC" "0"
exists   "1 回目: skills/tako に管理印つきの写しが在る" "$MARK"
contains "1 回目: 入れたと出す"                       "[ok] tako mod を Claude Code へ入れた" "$SANDBOX/1a.log"
matches  "1 回目: 最終サマリに mod の導入が載る"      "^  - ~/\.claude/skills/tako: \(未設定\) -> tako mod [0-9][0-9.]* の写し \[default\]$" "$SANDBOX/1a.log"
absent   "1 回目: 変更なしと言わない"                 "セットアップ結果: 変更なし" "$SANDBOX/1a.log"
INSTR=$(line_of "グローバル指示ファイルを作成: ~/.claude/CLAUDE.md" "$SANDBOX/1a.log")
MODOK=$(line_of "[ok] tako mod を Claude Code へ入れた" "$SANDBOX/1a.log")
if [ "$INSTR" -gt 0 ] && [ "$MODOK" -gt "$INSTR" ]; then
  ok "1 回目: mod の段は指示ファイルの段（~/.claude を作る）の後ろ（${INSTR} 行目 → ${MODOK} 行目）"
else
  ng "1 回目: 段の順（指示ファイル ${INSTR} 行目 / mod ${MODOK} 行目）"
fi
no_launch "1 回目: agent を対話起動しない"
SUM1=$(mark_sum)

RC=$(run_setup "$SANDBOX/1b.log" "" -- setup)
check    "2 回目: 終了コード 0"                       "$RC" "0"
contains "2 回目: 本当に変化が無ければ変更なし"       "セットアップ結果: 変更なし" "$SANDBOX/1b.log"
contains "2 回目: mod は最新と出す"                   "tako mod（Claude Code）は最新" "$SANDBOX/1b.log"
absent   "2 回目: 入れ直さない"                       "[ok] tako mod を Claude Code へ入れた" "$SANDBOX/1b.log"
check    "2 回目: 管理印は書き直さない"               "$(mark_sum)" "$SUM1"
no_launch "2 回目: agent を対話起動しない"

echo "== 2) 写しの版が変わった（置き直し）もサマリに出る =="
RC=$(run_setup "$SANDBOX/2a.log" "" TAKO_1959_MOD_VERSION=0.0.1 -- setup)
check    "版を差し替え: 終了コード 0"                 "$RC" "0"
matches  "版を差し替え: 置き直しが前後の版で載る"     "^  - ~/\.claude/skills/tako: tako mod [0-9][0-9.]* の写し -> tako mod 0\.0\.1 の写し \[default\]$" "$SANDBOX/2a.log"
RC=$(run_setup "$SANDBOX/2b.log" "" -- setup)
check    "戻す: 終了コード 0"                         "$RC" "0"
matches  "戻す: 置き直しが前後の版で載る"             "^  - ~/\.claude/skills/tako: tako mod 0\.0\.1 の写し -> tako mod [0-9][0-9.]* の写し \[default\]$" "$SANDBOX/2b.log"
RC=$(run_setup "$SANDBOX/2c.log" "" -- setup)
contains "その次: 変更なし"                           "セットアップ結果: 変更なし" "$SANDBOX/2c.log"

echo "== 3) claude が無い（codex だけ）: 入れない =="
mkenv none codex
RC=$(run_setup "$SANDBOX/3.log" "" -- setup)
check    "終了コード 0"                               "$RC" "0"
missing  "skills/tako を置かない"                     "$HOMEDIR/.claude/skills/tako"
missing  "claude の設定 dir を作らない"               "$HOMEDIR/.claude"
contains "入れない理由を出す"                         "[skip] tako mod を Claude Code へ入れない" "$SANDBOX/3.log"
absent   "サマリに mod を載せない"                    "$SUMMARY_KEY" "$SANDBOX/3.log"
no_launch "agent を対話起動しない"

echo "== 4) claude の版が下限未満（2.1.200）: 入れない =="
mkenv 2.1.200
RC=$(run_setup "$SANDBOX/4.log" "" -- setup)
check    "終了コード 0"                               "$RC" "0"
exists   "指示ファイルは作る（~/.claude は在る）"     "$HOMEDIR/.claude/CLAUDE.md"
missing  "skills/tako を置かない"                     "$HOMEDIR/.claude/skills/tako"
contains "入れない理由を出す"                         "[skip] tako mod を Claude Code へ入れない" "$SANDBOX/4.log"
absent   "サマリに mod を載せない"                    "$SUMMARY_KEY" "$SANDBOX/4.log"
no_launch "agent を対話起動しない"

echo "== 5) claude は在るが setup の系統は codex（~/.claude は無いまま）: 設定 dir を作らず入れない =="
mkenv 2.1.301 codex
RC=$(run_setup "$SANDBOX/5a.log" '{"selected_agent":"codex"}' -- setup --yes --answers -)
check    "終了コード 0"                               "$RC" "0"
exists   "codex の指示ファイルは作る"                 "$HOMEDIR/.codex/AGENTS.md"
missing  "claude の設定 dir を作らない（FR-2.42.25）" "$HOMEDIR/.claude"
absent   "サマリに mod を載せない"                    "$SUMMARY_KEY" "$SANDBOX/5a.log"
no_launch "agent を対話起動しない"
# claude を使い始めて設定 dir ができた後の setup（#2292 の「2 回目」と同じ状況）
mkdir -p "$HOMEDIR/.claude"
RC=$(run_setup "$SANDBOX/5b.log" '{"selected_agent":"codex"}' -- setup --yes --answers -)
check    "設定 dir ができた後: 終了コード 0"          "$RC" "0"
exists   "設定 dir ができた後: 入る"                  "$MARK"
matches  "設定 dir ができた後: サマリに載る（変更なしと言わない）" "^  - ~/\.claude/skills/tako: \(未設定\) -> tako mod [0-9][0-9.]* の写し \[default\]$" "$SANDBOX/5b.log"
absent   "設定 dir ができた後: 変更なしと言わない"    "セットアップ結果: 変更なし" "$SANDBOX/5b.log"

echo
echo "PASS=$PASS FAIL=$FAIL"
[ "$FAIL" -eq 0 ]
