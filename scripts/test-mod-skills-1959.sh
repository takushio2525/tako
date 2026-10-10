#!/usr/bin/env bash
# test-mod-skills-1959.sh — tako mod を Claude Code の設定 dir の skills/tako へ入れる（#1959）の実経路テスト
#
# 実物の Claude Code（PATH の claude。下限 2.1.294 以上）と、一時の HOME・設定 dir・data dir で回す。
# **本物の設定 dir（~/.claude*）には書かない**:
#   - HOME / TAKO_DATA_DIR / 設定 dir はすべて mktemp の下。TAKO_ISOLATED=1 = tako の検証プロセス
#     （置く先が一時 dir の外を指したら tako 自身が書かない = claude_mod_install::write_guard）
#   - 書く前に毎回 `tako mod install --dry-run --json` の置く先がすべて一時 dir（か書けない /var/empty）
#     であることを確かめ、外を指していたら何も書かずに止まる（STOP）
#   - 前後で本物の ~/.claude*/skills・plugins の一覧とハッシュを比べる
#   - claude には認証を写さない（未認証の `claude -p` でもプラグインの読み込みとフックは走る =
#     デバッグログで見る）。本番 tako が配る env（TAKO_SOCKET / TAKO_PANE_ID / TAKO_CLI /
#     CLAUDE_CODE_PLUGIN_DIRS 等）は最初に落とす
#
# 見るもの（#1959 の受け入れ。番号は出力の [n]）:
#   1  setup の段で 2 つの設定 dir（既定 = 一時 HOME の ~/.claude・accounts.yaml のアカウント）へ写しが入り
#      印が付く。plugin list に tako@skills-dir（enabled）・デバッグログに hooks module tako@skills-dir loaded。
#      settings*.json / plugins/*.json のハッシュが前後で同じ
#   2  中身が同じなら書かない（mtime 不変）・版が変われば差し替わる・旧版へ戻すと戻る（plugin list の version）
#   3  env の注入（CLAUDE_CODE_PLUGIN_DIRS）と同時でも hooks module tako@ は 1 行（tako@inline）
#   4  版不足（2.1.280 のスタブ）・未判定（claude 無し）なら入れない（claude_too_old / claude_unknown）
#   5  利用者の skills/tako（印なし）・skills/<別名> の name: tako・installed の tako@market には触らず
#      name_conflict。既に置いた写しは退く
#   6  tako mod uninstall（dry-run は何も消さない）で印つきの写しだけが消え、他の skills と設定は不変
#   7  写しだけが残り data dir が無くても claude は普通に動く（mod は休眠 = tako を呼ばない）
#   8  利用者が /plugin で止めた → env の注入で読まれても休眠し、最後の報告に dormant: user_disabled
#   9  書き込みの途中で落ちても（TAKO_1959_INJECT_CRASH）次で置き直せ、claude には 1 つしか見えない
#   10 読み取り専用の skills/・印が壊れている
#   11 検証プロセスは一時 dir の外の設定 dir（accounts.yaml が /var/empty を指す）へ書かない（refused）
#   12 GUI（隔離・仮想ディスプレイ・TAKO_PERSIST=0・自動リネーム off）: 起動時の同期・mod の報告で
#      見えた設定 dir への導入と persist.log・同名の衝突がある設定 dir のペインへは env を注入しない
#
# 使い方: bash scripts/test-mod-skills-1959.sh   （ONLY=cli|gui で片方だけ）。**CI には登録しない**
# （実物の claude と仮想ディスプレイが要る）。CI 側の担保は Rust の単体テスト・番犬・`claude plugin test`
set -uo pipefail

# 本番 GUI を指す env を最初に落とす（#1449 / #1450 / #1970）
unset TAKO_SOCKET TAKO_TOKEN TAKO_PANE_ID TAKO_TAB_ID TAKO_ORCHESTRATOR_ROLE TAKO_MCP_URL \
  CLAUDE_CODE_PLUGIN_DIRS TAKO_CLI TAKO_1877_NO_MOD TAKO_1959_LEGACY TAKO_1959_CONFIG_DIRS \
  TAKO_1959_MOD_VERSION TAKO_1959_INJECT_CRASH TAKO_ORCHESTRATOR_DIR TAKO_DISCOVERY_DIR
unset CLAUDE_CODE_CHILD_SESSION CLAUDE_CODE_SESSION_ID CLAUDECODE CLAUDE_CODE_ENTRYPOINT
# 自分の claude が使っている本物の設定 dir を置く先へ混ぜない（置く先は accounts.yaml と一時 HOME だけ）
unset CLAUDE_CONFIG_DIR

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
ONLY="${ONLY:-}"
PASS=0
FAIL=0
UNMEASURED=0
pass() { PASS=$((PASS + 1)); echo "  [OK] $1"; }
fail() { FAIL=$((FAIL + 1)); echo "  [NG] $1"; }
unmeasured() { UNMEASURED=$((UNMEASURED + 1)); echo "  [未実測] $1"; }
check_eq() { if [ "$2" = "$3" ]; then pass "$1"; else fail "$1（期待 '${2}' / 実際 '${3}'）"; fi; }
want() { [ -z "$ONLY" ] || [ "$ONLY" = "$1" ]; }

REAL_HOME="$HOME"
REAL_CLAUDE="$(command -v claude 2>/dev/null || true)"
if [ -z "$REAL_CLAUDE" ]; then
  echo "未実測: PATH に claude が無い"
  exit 3
fi

# --- 本物の設定 dir の前後比較（skills / plugins の一覧とハッシュ）---------------------
# 走っている Claude Code（master / worker のセッション）が自分で書く記帳は外す: プロセスごとの
# `.in_use/<pid>`・同期の記録（`synced/`）・`.last_inuse_sweep`・`install-counts-cache.json`。
# これらはこの検証と無関係に数分おきに変わる（2026-10-09 の実測で別セッションの pid が出入りした）
REAL_NOISE='/\.in_use/|/synced/|/\.last_inuse_sweep$|/install-counts-cache\.json$'
snap_real_list() {
  local d
  for d in "$REAL_HOME"/.claude*/skills "$REAL_HOME"/.claude*/plugins; do
    [ -d "$d" ] || continue
    find "$d" -type f -print0 2>/dev/null | sort -z | xargs -0 shasum -a 256 2>/dev/null
    find "$d" \( -type d -o -type l \) 2>/dev/null | sort
  done | grep -Ev "$REAL_NOISE"
}
snap_real() { snap_real_list | shasum -a 256 | cut -c1-16; }
REAL_BEFORE="$(snap_real)"
snap_real_list > "${TMPDIR:-/tmp}/tako-1959-real-before.$$"

# --- ビルドは HOME を差し替える前に（ツールチェーンを一時 HOME へ取り直さない）--------------
# shellcheck source=lib/isolated-gui.sh
. "$REPO_ROOT/scripts/lib/isolated-gui.sh"
isolated_gui_bins || exit 1

WORK="$(mktemp -d "${TMPDIR:-/tmp}/tako-1959-XXXXXX")"
WORK="$(cd "$WORK" && pwd)"
APP_PID=""
cleanup() {
  [ -n "$APP_PID" ] && stop_isolated_gui "$APP_PID"
  chmod -R u+w "$WORK" 2>/dev/null
  if [ -n "${KEEP:-}" ]; then echo "作業 dir を残した（KEEP）: $WORK"; else rm -rf "$WORK"; fi
}
trap cleanup EXIT
trap 'exit 130' INT TERM

H="$WORK/home"
DATA="$WORK/data"
BIN="$WORK/bin"
CFG_DEFAULT="$H/.claude"
CFG_ACCT="$WORK/cfg-acct"
mkdir -p "$H" "$DATA/orchestrator" "$BIN" "$CFG_DEFAULT" "$CFG_ACCT"
# 実物の claude を symlink で PATH へ（tako は symlink の先のパスから版を読む）
ln -s "$REAL_CLAUDE" "$BIN/claude"
printf 'accounts:\n  work:\n    config_dir: %s\n  main:\n    inherit: true\n' "$CFG_ACCT" \
  > "$DATA/orchestrator/accounts.yaml"
# 利用者の設定（読むだけで書かれないことを見る）
for c in "$CFG_DEFAULT" "$CFG_ACCT"; do
  mkdir -p "$c/plugins" "$c/skills/mine"
  printf '{"permissions":{"allow":["Bash(ls:*)"]},"enabledPlugins":{"other@market":true}}\n' > "$c/settings.json"
  printf '{"version":2,"plugins":{"other@market":[{"scope":"user"}]}}\n' > "$c/plugins/installed_plugins.json"
  printf -- '---\nname: mine\ndescription: x\n---\nhello\n' > "$c/skills/mine/SKILL.md"
done

iso_env() {
  env -u TAKO_SOCKET -u TAKO_PANE_ID -u TAKO_TAB_ID -u TAKO_TOKEN -u TAKO_CLI -u TAKO_MCP_URL \
    -u CLAUDE_CODE_PLUGIN_DIRS -u TAKO_ORCHESTRATOR_ROLE -u CLAUDE_CONFIG_DIR -u TAKO_ORCHESTRATOR_DIR \
    HOME="$H" TAKO_DATA_DIR="$DATA" TAKO_ISOLATED=1 PATH="$BIN:/usr/bin:/bin" "$@"
}
tk() { iso_env "$TAKO_BIN" "$@"; }

# 上限つきで走らせる（bash 3.2 に timeout は無い）。<秒> <出力> <コマンド…>
bounded() {
  local secs="$1" out="$2" i=0 rc=0 pid
  shift 2
  "$@" < /dev/null > "$out" 2>&1 &
  pid=$!
  while kill -0 "$pid" 2>/dev/null; do
    if [ "$i" -ge $((secs * 5)) ]; then
      kill -9 "$pid" 2>/dev/null
      wait "$pid" 2>/dev/null
      return 124
    fi
    sleep 0.2
    i=$((i + 1))
  done
  wait "$pid" || rc=$?
  return "$rc"
}
# 一時の設定 dir の claude（認証を写さない・自動更新と不要な通信を止める）。<設定 dir> [VAR=VAL…] -- <引数…>
iso_claude() {
  local cfg="$1"
  shift
  local -a extra=()
  while [ $# -gt 0 ] && [ "$1" != "--" ]; do extra+=("$1"); shift; done
  [ "${1:-}" = "--" ] && shift
  (cd "$WORK" && env -u CLAUDECODE -u CLAUDE_CODE_SESSION_ID -u CLAUDE_CODE_CHILD_SESSION \
    -u CLAUDE_CODE_ENTRYPOINT -u CLAUDE_CODE_PLUGIN_DIRS -u TAKO_PANE_ID -u TAKO_CLI -u TAKO_SOCKET \
    -u TAKO_TOKEN -u TAKO_TAB_ID -u TAKO_MCP_URL -u TAKO_ORCHESTRATOR_ROLE -u ANTHROPIC_API_KEY \
    HOME="$H" CLAUDE_CONFIG_DIR="$cfg" DISABLE_AUTOUPDATER=1 CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC=1 \
    ${extra[@]+"${extra[@]}"} "$BIN/claude" "$@")
}
# `plugin list --json` の出力から JSON だけを取り出す（claude は設定 dir の .claude.json が無く
# 控えだけがあると、復元の案内を stdout の JSON の前に出す = 一時 HOME で setup を回した後に実測）
json_of() { sed -n '/^\[/,$p' "$1"; }
plugin_entry() { # <設定 dir> → tako@skills-dir の項目（JSON 1 行）か空（空なら生の出力を stderr へ）
  local rc=0 entry
  bounded 60 "$WORK/list.out" iso_claude "$1" -- plugin list --json || rc=$?
  entry="$(json_of "$WORK/list.out" | jq -c '.[] | select(.id == "tako@skills-dir")' 2>/dev/null | head -1)"
  if [ -z "$entry" ]; then
    echo "    (plugin list: exit ${rc}) $(head -c 600 "$WORK/list.out" | tr '\n' ' ' | sed "s#$WORK#<work>#g")" >&2
  fi
  printf '%s' "$entry"
}
loaded_modules() { # <設定 dir> [VAR=VAL…] → 読み込まれた tako の hooks module（1 行 1 つ）
  local cfg="$1" log="$WORK/debug-last.log"
  shift
  rm -f "$log"
  bounded 60 "$WORK/p.out" iso_claude "$cfg" ${@+"$@"} -- -p hi --debug-file "$log"
  grep -Eo 'hooks module tako@[a-z-]+ loaded' "$log" 2>/dev/null | sort
}
cfg_hashes() { # <設定 dir> → settings*.json と plugins/*.json のハッシュ
  find "$1" -maxdepth 2 \( -name 'settings*.json' -o -path '*/plugins/*.json' \) -type f -print0 \
    | sort -z | xargs -0 shasum -a 256 | shasum -a 256 | cut -c1-16
}
mtime() { stat -f %m "$1" 2>/dev/null || echo -; }
state_of() { # <JSON> <設定 dir の末尾> → state
  printf '%s' "$1" | jq -r --arg d "$2" '.skills.targets[] | select(.config_dir | endswith($d)) | .state' | head -1
}

# 書く前の確認: dry-run の置く先がすべて一時 dir（か /var/empty）か。外なら止まる
guard_targets() {
  local out bad
  out="$(tk mod install --dry-run --json 2>/dev/null)"
  bad="$(printf '%s' "$out" | jq -r '.skills.targets[].config_dir' \
    | grep -v -e '^~/' -e "^${WORK}" -e '^/var/empty$' || true)"
  if [ -n "$bad" ]; then
    echo "STOP: 置く先が一時 dir の外を指している（何も書かずに止める）:"
    printf '%s\n' "$bad" | sed 's/^/    /'
    exit 1
  fi
  printf '%s' "$out"
}

VERSION="$(bounded 30 "$WORK/v.out" "$BIN/claude" --version; grep -Eo '[0-9]+\.[0-9]+\.[0-9]+' "$WORK/v.out" | head -1)"
echo "claude ${VERSION} / 作業 dir: (mktemp)/tako-1959-*"

if want cli; then
  echo ""
  echo "[1] setup の段で 2 つの設定 dir へ入れる"
  before_default="$(cfg_hashes "$CFG_DEFAULT")"
  before_acct="$(cfg_hashes "$CFG_ACCT")"
  dry="$(guard_targets)"
  check_eq "dry-run の既定の設定 dir は書く予定" "write" \
    "$(printf '%s' "$dry" | jq -r '.skills.targets[] | select(.config_dir == "~/.claude") | .action')"
  check_eq "dry-run は何も書かない" "no" "$([ -e "$CFG_DEFAULT/skills/tako" ] && echo yes || echo no)"
  bounded 300 "$WORK/setup.out" iso_env "$TAKO_BIN" setup
  setup_rc=$?
  grep -E 'tako mod|skills/tako' "$WORK/setup.out" | sed "s#$WORK#<work>#g; s/^/    | /"
  check_eq "setup は完走する" "0" "$setup_rc"
  for c in "$CFG_DEFAULT" "$CFG_ACCT"; do
    name="${c#"$WORK"/}"
    check_eq "${name}: 管理印が付く" "tako" "$(jq -r .managed_by "$c/skills/tako/.tako-managed" 2>/dev/null)"
    entry="$(plugin_entry "$c")"
    check_eq "${name}: plugin list に tako@skills-dir（enabled）" "true" "$(printf '%s' "$entry" | jq -r .enabled 2>/dev/null)"
    check_eq "${name}: errors が無い" "null" "$(printf '%s' "$entry" | jq -r .errors 2>/dev/null)"
    check_eq "${name}: デバッグログに hooks module tako@skills-dir loaded" \
      "hooks module tako@skills-dir loaded" "$(loaded_modules "$c")"
  done
  check_eq "既定: settings*.json / plugins/*.json のハッシュが前後で同じ" "$before_default" "$(cfg_hashes "$CFG_DEFAULT")"
  check_eq "アカウント: settings*.json / plugins/*.json のハッシュが前後で同じ" "$before_acct" "$(cfg_hashes "$CFG_ACCT")"
  check_eq "他の skills（mine）は残る" "hello" "$(tail -1 "$CFG_DEFAULT/skills/mine/SKILL.md")"

  echo ""
  echo "[2] 中身が同じなら書かない・版の上げ下げで差し替わる"
  reg="$CFG_DEFAULT/skills/tako/hooks/register.ts"
  m1="$(mtime "$reg")"
  sleep 1
  out="$(tk mod install --json)"
  check_eq "2 回目は書かない（outcome）" "unchanged" "$(printf '%s' "$out" | jq -r '.skills.targets[] | select(.config_dir == "~/.claude") | .outcome')"
  check_eq "2 回目は書かない（mtime）" "$m1" "$(mtime "$reg")"
  cur="$(jq -r .tako_version "$CFG_DEFAULT/skills/tako/.tako-managed")"
  guard_targets > /dev/null
  out="$(TAKO_1959_MOD_VERSION=0.0.1 tk mod install --json)"
  check_eq "旧版の tako（0.0.1）へ戻すと写しも戻る" "0.0.1" "$(printf '%s' "$(plugin_entry "$CFG_DEFAULT")" | jq -r .version)"
  out="$(tk mod install --dry-run --json)"
  check_eq "版が違えば outdated" "outdated" "$(state_of "$out" "~/.claude")"
  tk mod install > /dev/null
  check_eq "版を上げると差し替わる（${cur}）" "$cur" "$(printf '%s' "$(plugin_entry "$CFG_DEFAULT")" | jq -r .version)"

  echo ""
  echo "[3] env の注入と同時でも 1 つしか読まれない"
  mods="$(loaded_modules "$CFG_DEFAULT" CLAUDE_CODE_PLUGIN_DIRS="$DATA/claude-mod/tako")"
  check_eq "hooks module tako@ は 1 行で inline が勝つ" "hooks module tako@inline loaded" "$mods"

  echo ""
  echo "[4] 版不足・未判定なら入れない"
  C4="$WORK/cfg-gate"
  mkdir -p "$C4" "$WORK/bin-old" "$WORK/bin-none"
  printf '#!/bin/sh\necho "2.1.280 (Claude Code)"\n' > "$WORK/bin-old/claude"
  chmod +x "$WORK/bin-old/claude"
  out="$(env TAKO_1959_CONFIG_DIRS="$C4" HOME="$H" TAKO_DATA_DIR="$DATA" TAKO_ISOLATED=1 PATH="$WORK/bin-old:/usr/bin:/bin" "$TAKO_BIN" mod install --json)"
  check_eq "2.1.280 は claude_too_old" "claude_too_old" "$(state_of "$out" "cfg-gate")"
  out="$(env TAKO_1959_CONFIG_DIRS="$C4" HOME="$H" TAKO_DATA_DIR="$DATA" TAKO_ISOLATED=1 PATH="$WORK/bin-none:/usr/bin:/bin" "$TAKO_BIN" mod install --json)"
  check_eq "claude が無ければ claude_unknown" "claude_unknown" "$(state_of "$out" "cfg-gate")"
  check_eq "どちらも置かない" "no" "$([ -e "$C4/skills/tako" ] && echo yes || echo no)"

  echo ""
  echo "[5] 利用者の tako・別の出どころの同名 tako には触らない"
  C5a="$WORK/cfg-user"
  C5b="$WORK/cfg-same"
  C5c="$WORK/cfg-market"
  mkdir -p "$C5a/skills/tako" "$C5b/skills/my-tako/.claude-plugin" "$C5c/plugins"
  printf 'user\n' > "$C5a/skills/tako/NOTE"
  printf '{"name":"tako"}\n' > "$C5b/skills/my-tako/.claude-plugin/plugin.json"
  out="$(env TAKO_1959_CONFIG_DIRS="$C5c" HOME="$H" TAKO_DATA_DIR="$DATA" TAKO_ISOLATED=1 PATH="$BIN:/usr/bin:/bin" "$TAKO_BIN" mod install --json)"
  check_eq "（先に置く）cfg-market に置いた" "installed" "$(state_of "$out" "cfg-market")"
  printf '{"version":2,"plugins":{"tako@my-market":[{"scope":"user"}]}}\n' > "$C5c/plugins/installed_plugins.json"
  m5="$(cfg_hashes "$C5c")"
  out="$(env TAKO_1959_CONFIG_DIRS="$C5a:$C5b:$C5c" HOME="$H" TAKO_DATA_DIR="$DATA" TAKO_ISOLATED=1 PATH="$BIN:/usr/bin:/bin" "$TAKO_BIN" mod install --json)"
  check_eq "印の無い skills/tako は name_conflict" "name_conflict" "$(state_of "$out" "cfg-user")"
  check_eq "  理由は user_skills_dir" "user_skills_dir" "$(printf '%s' "$out" | jq -r '.skills.targets[] | select(.config_dir | endswith("cfg-user")) | .conflicts[0].code')"
  check_eq "  利用者のファイルはそのまま" "user" "$(cat "$C5a/skills/tako/NOTE")"
  check_eq "skills/my-tako の name: tako は name_conflict" "name_conflict" "$(state_of "$out" "cfg-same")"
  check_eq "  skills/tako を置かない" "no" "$([ -e "$C5b/skills/tako" ] && echo yes || echo no)"
  check_eq "installed の tako@my-market は name_conflict" "name_conflict" "$(state_of "$out" "cfg-market")"
  check_eq "  既に置いた写しは退いた" "removed" "$(printf '%s' "$out" | jq -r '.skills.targets[] | select(.config_dir | endswith("cfg-market")) | .outcome')"
  check_eq "  installed_plugins.json は書かない" "$m5" "$(cfg_hashes "$C5c")"

  echo ""
  echo "[6] tako mod uninstall は印つきの写しだけを外す"
  guard_targets > /dev/null
  tk mod uninstall --dry-run > /dev/null
  check_eq "dry-run は何も消さない" "yes" "$([ -e "$CFG_DEFAULT/skills/tako" ] && echo yes || echo no)"
  h_before="$(cfg_hashes "$CFG_DEFAULT")"
  out="$(tk mod uninstall --json)"
  check_eq "既定から外した" "removed" "$(printf '%s' "$out" | jq -r '.skills.targets[] | select(.config_dir == "~/.claude") | .outcome')"
  check_eq "skills/tako が無い" "no" "$([ -e "$CFG_DEFAULT/skills/tako" ] && echo yes || echo no)"
  check_eq "他の skills（mine）は残る" "yes" "$([ -f "$CFG_DEFAULT/skills/mine/SKILL.md" ] && echo yes || echo no)"
  check_eq "設定は不変" "$h_before" "$(cfg_hashes "$CFG_DEFAULT")"
  check_eq "plugin list から消える" "" "$(plugin_entry "$CFG_DEFAULT")"
  check_eq "残骸の dot dir も残らない" "" "$(find "$CFG_DEFAULT/skills" -maxdepth 1 -name '.tako.*' | head -1)"
  tk mod install > /dev/null

  echo ""
  echo "[7] 写しだけが残り data dir が無くても claude は普通に動く"
  mv "$DATA" "$DATA.gone"
  mods="$(loaded_modules "$CFG_ACCT")"
  check_eq "写しは読まれる" "hooks module tako@skills-dir loaded" "$mods"
  check_eq "claude の終わり方は mod なしと同じ（未認証の案内）" "true" "$(grep -q 'Not logged in' "$WORK/p.out" && echo true || echo false)"
  check_eq "mod の失敗の行が無い" "" "$(grep -iE 'tako@skills-dir.*(error|fail)' "$WORK/debug-last.log" | head -1)"
  mv "$DATA.gone" "$DATA"

  echo ""
  echo "[8] 利用者が /plugin で止めたら tako のペインでも休眠する"
  C8="$WORK/cfg-off"
  mkdir -p "$C8"
  env TAKO_1959_CONFIG_DIRS="$C8" HOME="$H" TAKO_DATA_DIR="$DATA" TAKO_ISOLATED=1 PATH="$BIN:/usr/bin:/bin" "$TAKO_BIN" mod install > /dev/null
  bounded 60 "$WORK/disable.out" iso_claude "$C8" -- plugin disable tako@skills-dir
  check_eq "plugin disable は settings.json に false を書く（利用者の操作）" "false" "$(jq -r '.enabledPlugins["tako@skills-dir"]' "$C8/settings.json")"
  out="$(env TAKO_1959_CONFIG_DIRS="$C8" HOME="$H" TAKO_DATA_DIR="$DATA" TAKO_ISOLATED=1 PATH="$BIN:/usr/bin:/bin" "$TAKO_BIN" mod install --dry-run --json)"
  check_eq "tako mod に user_disabled" "user_disabled" "$(state_of "$out" "cfg-off")"
  printf '#!/bin/sh\ncat >> "%s/reports.jsonl"\necho >> "%s/reports.jsonl"\necho "{}"\n' "$WORK" "$WORK" > "$WORK/fake-tako"
  chmod +x "$WORK/fake-tako"
  : > "$WORK/reports.jsonl"
  mods="$(loaded_modules "$C8" CLAUDE_CODE_PLUGIN_DIRS="$DATA/claude-mod/tako" TAKO_PANE_ID=7 TAKO_CLI="$WORK/fake-tako")"
  check_eq "env の注入では inline が読まれる" "hooks module tako@inline loaded" "$mods"
  check_eq "報告は 1 回だけ（休眠の印）" "1" "$(grep -c '"schema"' "$WORK/reports.jsonl")"
  check_eq "最後の報告に dormant: user_disabled" "user_disabled" "$(grep '"schema"' "$WORK/reports.jsonl" | tail -1 | jq -r .dormant)"
  bounded 60 "$WORK/enable.out" iso_claude "$C8" -- plugin enable tako@skills-dir
  : > "$WORK/reports.jsonl"
  loaded_modules "$C8" CLAUDE_CODE_PLUGIN_DIRS="$DATA/claude-mod/tako" TAKO_PANE_ID=7 TAKO_CLI="$WORK/fake-tako" > /dev/null
  check_eq "戻すと休眠しない（ended の報告が届く）" "true" "$(grep '"schema"' "$WORK/reports.jsonl" | tail -1 | jq -r .ended)"

  echo ""
  echo "[9] 書き込みの途中で落ちても壊れない"
  C9="$WORK/cfg-crash"
  mkdir -p "$C9"
  run9() { env TAKO_1959_CONFIG_DIRS="$C9" HOME="$H" TAKO_DATA_DIR="$DATA" TAKO_ISOLATED=1 PATH="$BIN:/usr/bin:/bin" "$@"; }
  run9 "$TAKO_BIN" mod install > /dev/null
  rc=0
  run9 TAKO_1959_MOD_VERSION=0.0.2 TAKO_1959_INJECT_CRASH=retired "$TAKO_BIN" mod install > /dev/null 2>&1 || rc=$?
  check_eq "退避の後で落ちた（終了コード 86）" "86" "$rc"
  check_eq "  plugin list に tako は出ない（2 つ目にも見えない）" "0" "$(bounded 60 "$WORK/l9.out" iso_claude "$C9" -- plugin list --json; json_of "$WORK/l9.out" | jq '[.[] | select(.id | startswith("tako"))] | length')"
  run9 "$TAKO_BIN" mod install > /dev/null
  check_eq "  次の install で置き直せる" "true" "$(printf '%s' "$(plugin_entry "$C9")" | jq -r .enabled)"
  rc=0
  run9 TAKO_1959_MOD_VERSION=0.0.2 TAKO_1959_INJECT_CRASH=staged "$TAKO_BIN" mod install > /dev/null 2>&1 || rc=$?
  check_eq "一時 dir を書いた後で落ちた（終了コード 86）" "86" "$rc"
  check_eq "  前の写しのまま 1 つだけ見える" "1" "$(bounded 60 "$WORK/l9.out" iso_claude "$C9" -- plugin list --json; json_of "$WORK/l9.out" | jq '[.[] | select(.id | startswith("tako"))] | length')"

  echo ""
  echo "[10] 読み取り専用の skills/・壊れた印"
  C10="$WORK/cfg-ro"
  mkdir -p "$C10/skills"
  chmod 555 "$C10/skills"
  rc=0
  out="$(env TAKO_1959_CONFIG_DIRS="$C10" HOME="$H" TAKO_DATA_DIR="$DATA" TAKO_ISOLATED=1 PATH="$BIN:/usr/bin:/bin" "$TAKO_BIN" mod install --json)" || rc=$?
  check_eq "読み取り専用は error を返す（CLI は非ゼロ）" "1" "$rc"
  chmod 755 "$C10/skills"
  check_eq "  何も残さない" "0" "$(find "$C10/skills" -mindepth 1 | wc -l | tr -d ' ')"
  printf '{ 壊れた' > "$CFG_ACCT/skills/tako/.tako-managed"
  out="$(tk mod install --dry-run --json)"
  check_eq "壊れた印は outdated（tako の写しとして置き直す）" "outdated" "$(state_of "$out" "cfg-acct")"
  tk mod install > /dev/null
  check_eq "  置き直すと印が読める" "tako" "$(jq -r .managed_by "$CFG_ACCT/skills/tako/.tako-managed")"

  echo ""
  echo "[11] 検証プロセスは一時 dir の外の設定 dir へ書かない"
  printf '  outside:\n    config_dir: /var/empty\n' >> "$DATA/orchestrator/accounts.yaml"
  out="$(guard_targets)"
  check_eq "dry-run で /var/empty は refused" "refused" "$(state_of "$out" "/var/empty")"
  out="$(tk mod install --json)"
  check_eq "install でも refused（書こうとしない = error にならない）" "refused" "$(state_of "$out" "/var/empty")"
  check_eq "/var/empty は空のまま" "0" "$(find /var/empty -mindepth 1 2>/dev/null | wc -l | tr -d ' ')"
  # 戻す（GUI 部で使う accounts.yaml は別に作る）
  printf 'accounts:\n  work:\n    config_dir: %s\n' "$CFG_ACCT" > "$DATA/orchestrator/accounts.yaml"
fi

if want gui; then
  echo ""
  echo "[12] GUI: 起動時の同期・報告で見えた設定 dir・衝突のあるペインへは注入しない"
  G="$WORK/gui"
  GH="$G/home"
  mkdir -p "$GH/.claude" "$G/data/orchestrator" "$G/disc" "$G/cfg-acct" "$G/cfg-new"
  printf 'accounts:\n  work:\n    config_dir: %s\n' "$G/cfg-acct" > "$G/data/orchestrator/accounts.yaml"
  TAKO_SOCKET_DIR="$G/sock"
  mkdir -p "$TAKO_SOCKET_DIR"
  gui_env() {
    env -u CLAUDE_CONFIG_DIR -u TAKO_ORCHESTRATOR_DIR HOME="$GH" TAKO_DATA_DIR="$G/data" TAKO_DISCOVERY_DIR="$G/disc" \
      TAKO_ISOLATED=1 PATH="$BIN:/usr/bin:/bin" "$@"
  }
  # 隔離 GUI は自分の data dir の control.json から CLI を繋ぐ（TAKO_SOCKET は落としてある）
  launch_isolated_gui "$G/app.log" HOME="$GH" TAKO_DATA_DIR="$G/data" TAKO_DISCOVERY_DIR="$G/disc" \
    TAKO_PERSIST=0 TAKO_AUTO_RENAME=0 PATH="$BIN:/usr/bin:/bin" || exit $?
  APP_PID="$ISOLATED_GUI_PID"
  gtk() { gui_env "$TAKO_BIN" "$@"; }
  ok=no
  for _ in $(seq 1 150); do
    if gtk list > /dev/null 2>&1; then ok=yes; break; fi
    sleep 0.2
  done
  if [ "$ok" != yes ]; then
    unmeasured "隔離 GUI へ繋がらない（$(tail -3 "$G/app.log" | tr '\n' ' ')）"
  else
    # 起動時の同期（版の判定の後に背景で 1 回）
    for _ in $(seq 1 100); do
      [ -f "$GH/.claude/skills/tako/.tako-managed" ] && [ -f "$G/cfg-acct/skills/tako/.tako-managed" ] && break
      sleep 0.2
    done
    check_eq "起動時に既定へ入る" "yes" "$([ -f "$GH/.claude/skills/tako/.tako-managed" ] && echo yes || echo no)"
    check_eq "起動時にアカウントへ入る" "yes" "$([ -f "$G/cfg-acct/skills/tako/.tako-managed" ] && echo yes || echo no)"
    check_eq "persist.log に起動時の導入" "true" "$(grep -q 'skills/tako へ写しを置いた' "$G/data/persist.log" && echo true || echo false)"
    root="$(gtk list --json 2>/dev/null | jq -r '[.. | objects | select(has("pane_id")) | .pane_id][0] // empty')"
    [ -n "$root" ] || root="$(gtk mod --json | jq -r '.panes[0].pane')"
    # 報告で見えた設定 dir: pane の中の claude（env で注入された mod）が CLAUDE_CONFIG_DIR を報告する
    gtk split --pane "$root" -- /bin/sh -c "CLAUDE_CONFIG_DIR='$G/cfg-new' '$BIN/claude' -p hi --debug-file '$G/new.log' > '$G/new.out' 2>&1; sleep 30" > /dev/null
    for _ in $(seq 1 100); do
      [ -f "$G/cfg-new/skills/tako/.tako-managed" ] && break
      sleep 0.2
    done
    check_eq "pane の mod は inline で読まれた" "true" "$(grep -q 'hooks module tako@inline loaded' "$G/new.log" 2>/dev/null && echo true || echo false)"
    check_eq "報告で見えた設定 dir（cfg-new）へ入る" "yes" "$([ -f "$G/cfg-new/skills/tako/.tako-managed" ] && echo yes || echo no)"
    check_eq "persist.log に報告で見えた設定 dir" "true" "$(grep -q '報告で見えた設定 dir' "$G/data/persist.log" && echo true || echo false)"
    out="$(gtk mod --json)"
    check_eq "tako mod の skills に reported の出どころ" "reported" "$(printf '%s' "$out" | jq -r '.skills.targets[] | select(.config_dir | endswith("cfg-new")) | .sources[0]')"
    # 同名の衝突: 既定の設定 dir に tako@my-market を入れた後に作ったペインには注入しない
    mkdir -p "$GH/.claude/plugins"
    printf '{"version":2,"plugins":{"tako@my-market":[{"scope":"user"}]}}\n' > "$GH/.claude/plugins/installed_plugins.json"
    newp="$(gtk split --pane "$root" 2>/dev/null | grep -Eo '[0-9]+' | tail -1)"
    sleep 1
    out="$(gtk mod --json)"
    check_eq "衝突のある設定 dir のペインは not_injected" "not_injected" "$(printf '%s' "$out" | jq -r --argjson p "${newp:-0}" '.panes[] | select(.pane == $p) | .state')"
    check_eq "  理由は name_conflict" "name_conflict" "$(printf '%s' "$out" | jq -r --argjson p "${newp:-0}" '.panes[] | select(.pane == $p) | .reason.code')"
    check_eq "  最初のペインは注入済み" "true" "$(printf '%s' "$out" | jq -r --argjson p "$root" '.panes[] | select(.pane == $p) | .injected')"
    check_eq "  tako mod の skills の既定は name_conflict" "name_conflict" "$(state_of "$out" "~/.claude")"
  fi
  stop_isolated_gui "$APP_PID"
  APP_PID=""
fi

echo ""
REAL_AFTER="$(snap_real)"
check_eq "本物の ~/.claude*/skills・plugins の一覧とハッシュが前後で同じ（${REAL_BEFORE}。Claude Code 自身の記帳は除く）" \
  "$REAL_BEFORE" "$REAL_AFTER"
if [ "$REAL_BEFORE" != "$REAL_AFTER" ]; then
  snap_real_list | diff "${TMPDIR:-/tmp}/tako-1959-real-before.$$" - | sed "s#$REAL_HOME#~#g; s/^/    /" | head -20
fi
rm -f "${TMPDIR:-/tmp}/tako-1959-real-before.$$"
check_eq "本物の skills/ に tako の写し・残骸が無い" "" \
  "$(find "$REAL_HOME"/.claude*/skills -maxdepth 1 \( -name tako -o -name '.tako.*' \) 2>/dev/null | head -1)"
echo ""
echo "結果: ${PASS} PASS / ${FAIL} FAIL / 未実測 ${UNMEASURED}"
[ "$FAIL" -eq 0 ]
