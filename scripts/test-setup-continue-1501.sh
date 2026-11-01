#!/usr/bin/env bash
# #1501 の実経路テスト: 未認証・未導入でも `tako setup` が**最後まで走って exit 0**
# し、認証不要な段（依存 / MCP 登録 / 指示ファイル / プロファイル / テンプレ）を
# 全部やってから「残り」を最簡コマンドつきで出すことを、**隔離した
# HOME / TAKO_DATA_DIR / PATH** で実測する。
#
# #1501 前は bootstrap [3/3] の認証段で `error: … ログインが必要です` → exit 1 し、
# 以降の段が 1 つも走らなかった（新品 Mac では「setup を走らせても何も整わない」）。
# A/B は `TAKO_1501_LEGACY=1`（同一バイナリで旧アーム = 症状が出る）。
#
# 本番の ~/Library/Application Support/tako・~/.claude・~/.zprofile・実機の tmux には
# 一切触れない（`env -i` で環境ごと差し替え、claude / codex / brew はスタブ）。
# スタブ claude は `mcp add|list|remove` を**本当に実装**してあるので、
# 「MCP 登録が実際に載ったか」を隔離 HOME の .claude.json で確かめられる
# （未認証の実 claude 2.1.258 でも `mcp add --scope user` が通ることは実測済み）。
set -uo pipefail
# check-health / `tako setup --check` は表示言語に従う（#2365）。判定は日本語の字面なので、OS の言語に
# 依らず日本語表示へ固定する（settings.json の language が system のときだけ効く）
export TAKO_LANG=ja

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TAKO="${TAKO_BIN:-$ROOT/target/debug/tako}"
PTY="$ROOT/scripts/lib/pty-answer.py"
PASS=0; FAIL=0

ok()   { PASS=$((PASS+1)); echo "  PASS: $1"; }
ng()   { FAIL=$((FAIL+1)); echo "  FAIL: $1"; }
check(){ if [ "$2" = "$3" ]; then ok "$1"; else ng "$1 (期待 '$3' / 実際 '$2')"; fi; }
contains(){ if grep -qF -- "$2" "$3" 2>/dev/null; then ok "$1"; else ng "$1 ('$2' が出ていない)"; fi; }
absent(){ if grep -qF -- "$2" "$3" 2>/dev/null; then ng "$1 ('$2' が出ている)"; else ok "$1"; fi; }
exists(){ if [ -f "$2" ]; then ok "$1"; else ng "$1 ($2 が無い)"; fi; }
missing(){ if [ -f "$2" ]; then ng "$1 ($2 が在る)"; else ok "$1"; fi; }

[ -x "$TAKO" ] || { echo "tako が無い: ${TAKO}（cargo build -p tako-cli）"; exit 1; }
[ -f "$PTY" ]  || { echo "PTY ドライバが無い: $PTY"; exit 1; }
[ -x /usr/bin/python3 ] || { echo "/usr/bin/python3 が無い環境では走らない"; exit 1; }

SANDBOX="$(mktemp -d "${TMPDIR:-/tmp}/tako-1501-XXXXXX")"
trap 'rm -rf "$SANDBOX"' EXIT
BIN="$SANDBOX/bin"
HOMEDIR="$SANDBOX/home"
DATADIR="$SANDBOX/d"

INSTRUCTION="$HOMEDIR/.claude/CLAUDE.md"
PROFILE="$DATADIR/orchestrator/profiles/default.yaml"
TEMPLATE="$DATADIR/setup/setup-instructions.md"
MCPJSON="$HOMEDIR/.claude.json"

# --- スタブ ------------------------------------------------------------------
# claude スタブ: 認証応答は $1、`mcp add|list|remove` は隔離 HOME の .claude.json を
# 実際に読み書きする（本物と同じ置き場・同じキー）
write_claude(){ # $1 = auth（loggedIn:true）/ unauth（loggedIn:false）
  cat > "$BIN/claude" <<EOF
#!/bin/sh
AUTH_MODE="$1"
EOF
  cat >> "$BIN/claude" <<'EOF'
if [ "${1:-}" = "auth" ] && [ "${2:-}" = "status" ]; then
  if [ "$AUTH_MODE" = auth ]; then
    echo '{"loggedIn":true,"authMethod":"claude.ai","subscriptionType":"Max"}'
  else
    echo '{"loggedIn":false,"authMethod":"none"}'
  fi
  exit 0
fi
if [ "${1:-}" = "--version" ]; then echo "2.1.258 (Claude Code)"; exit 0; fi
if [ "${1:-}" = "mcp" ]; then
  shift
  sub="${1:-}"; [ $# -gt 0 ] && shift
  case "$sub" in
    list)
      /usr/bin/python3 -c '
import json,sys
try: d=json.load(open(sys.argv[1]))
except Exception: d={}
s=d.get("mcpServers") or {}
if not s: print("No MCP servers configured")
for k,v in s.items():
    print("%s: %s - connected" % (k, " ".join([v.get("command","")]+list(v.get("args") or []))))
' "$HOME/.claude.json"
      exit 0;;
    add)
      name=""; cmd=""
      while [ $# -gt 0 ]; do
        case "$1" in
          --scope|--transport) shift 2;;
          --) shift; cmd="$*"; break;;
          *) name="$1"; shift;;
        esac
      done
      /usr/bin/python3 -c '
import json,sys
p,name,cmd=sys.argv[1],sys.argv[2],sys.argv[3].split(" ")
try: d=json.load(open(p))
except Exception: d={}
d.setdefault("mcpServers",{})[name]={"type":"stdio","command":cmd[0],"args":cmd[1:],"env":{}}
json.dump(d,open(p,"w"),ensure_ascii=False)
' "$HOME/.claude.json" "$name" "$cmd"
      echo "Added stdio MCP server $name"
      exit 0;;
    remove)
      name=""
      while [ $# -gt 0 ]; do
        case "$1" in
          --scope) shift 2;;
          *) name="$1"; shift;;
        esac
      done
      /usr/bin/python3 -c '
import json,sys
p,name=sys.argv[1],sys.argv[2]
try: d=json.load(open(p))
except Exception: sys.exit(1)
if name in (d.get("mcpServers") or {}):
    del d["mcpServers"][name]; json.dump(d,open(p,"w"),ensure_ascii=False)
else: sys.exit(1)
' "$HOME/.claude.json" "$name" || exit 1
      exit 0;;
  esac
  exit 0
fi
exit 0
EOF
  chmod +x "$BIN/claude"
}

write_brew_ok(){ # 実際に実体を置く brew（#1499 の [y/N] → 導入 → 再検出を通すため）
  cat > "$BIN/brew" <<'EOF'
#!/bin/sh
[ "$1" = "install" ] || exit 0
echo "==> Fetching $2"
printf '#!/bin/sh\necho "%s 3.5a (stub)"\n' "$2" > "$(dirname "$0")/$2"
chmod +x "$(dirname "$0")/$2"
EOF
  chmod +x "$BIN/brew"
}

mkenv(){ # $1 = none|auth|unauth（claude）, $2 = none|auth（codex。省略は none）
  rm -rf "$BIN" "$HOMEDIR" "$DATADIR"
  mkdir -p "$BIN" "$HOMEDIR" "$DATADIR"
  # exe::find（境界 B16）のログインシェル経由の解決を隔離 PATH に閉じ込める
  printf '#!/bin/sh\nif [ "$1" = "-l" ]; then shift; fi\nexec /bin/sh "$@"\n' > "$BIN/isosh"
  # 既定のログインシェルは同じ中身を zsh の名前で置く（PATH の外）。PATH 通しの書き先は $SHELL の
  # 名前で決まり、種別の分からない名前（isosh）は書かずに断る（#2550）。isosh は 13) で使う
  mkdir -p "$BIN/login"
  printf '#!/bin/sh\nif [ "$1" = "-l" ]; then shift; fi\nexec /bin/sh "$@"\n' > "$BIN/login/zsh"
  chmod +x "$BIN/login/zsh"
  # brew: 呼ばれたら分かるように置くが、依存は N / 非対話で入れない前提
  printf '#!/bin/sh\necho "==> Pouring $2"\nexit 0\n' > "$BIN/brew"
  if [ "$1" = "none" ]; then
    # 「1 つも無い」を**ネットワークに触らずに**作る。非 TTY の `tako setup` は
    # 同意扱いで `curl -fsSL https://claude.ai/install.sh | bash` を実行するので
    # （棚卸し #1500 の実測）、PATH 先頭に落ちる curl を置いて導入を失敗させる
    printf '#!/bin/sh\necho "curl: (7) Could not resolve host (stub)" >&2\nexit 7\n' > "$BIN/curl"
  else
    write_claude "$1"
  fi
  if [ "${2:-none}" = "auth" ]; then
    printf '#!/bin/sh\ncase "$1 ${2:-}" in\n  "login status") echo "Logged in using ChatGPT"; exit 0;;\nesac\nexit 0\n' > "$BIN/codex"
  fi
  chmod +x "$BIN"/* 2>/dev/null
}

# 隔離環境で tako を走らせる 3 通り（パイプ / PTY / dispatch と同条件）
run_pipe(){ # run_pipe <出力先> -- <tako の引数...>
  local out="$1"; shift; shift
  env -i HOME="$HOMEDIR" TAKO_DATA_DIR="$DATADIR" TAKO_ISOLATED=1 \
      SHELL="${LOGIN_SHELL:-$BIN/login/zsh}" TAKO_TMUX_BIN="$BIN/tmux" \
      ${LEGACY:+TAKO_1501_LEGACY=1} ${REQDEP:+TAKO_1501_TEST_REQUIRED_DEP="$REQDEP"} \
      PATH="$BIN:/usr/bin:/bin:/usr/sbin:/sbin" TERM=dumb LANG=ja_JP.UTF-8 \
      "$TAKO" "$@" < /dev/null > "$out" 2>&1
  echo $?
}
run_pty(){ # run_pty <出力先> <answers...> -- <tako の引数...>
  local out="$1"; shift
  # 空配列の展開を守る形で書く（macOS 同梱の bash 3.2 は set -u 下で落ちる）
  local answers=(); while [ "${1:-}" != "--" ]; do answers+=(--answer "$1"); shift; done; shift
  env -i HOME="$HOMEDIR" TAKO_DATA_DIR="$DATADIR" TAKO_ISOLATED=1 \
      SHELL="${LOGIN_SHELL:-$BIN/login/zsh}" TAKO_TMUX_BIN="$BIN/tmux" \
      ${LEGACY:+TAKO_1501_LEGACY=1} ${REQDEP:+TAKO_1501_TEST_REQUIRED_DEP="$REQDEP"} \
      PATH="$BIN:/usr/bin:/bin:/usr/sbin:/sbin" TERM=dumb LANG=ja_JP.UTF-8 \
      /usr/bin/python3 "$PTY" ${answers[@]+"${answers[@]}"} --timeout 90 \
      -- "$TAKO" "$@" > "$out" 2>&1
  echo $?
}
run_dispatch(){ # run_dispatch <出力先> <answers json> — dispatch SetupRun / MCP tako_setup と同条件
  local out="$1"; local answers="$2"
  printf '%s' "$answers" | env -i HOME="$HOMEDIR" TAKO_DATA_DIR="$DATADIR" TAKO_ISOLATED=1 \
      SHELL="${LOGIN_SHELL:-$BIN/login/zsh}" TAKO_TMUX_BIN="$BIN/tmux" \
      ${LEGACY:+TAKO_1501_LEGACY=1} ${REQDEP:+TAKO_1501_TEST_REQUIRED_DEP="$REQDEP"} \
      PATH="$BIN:/usr/bin:/bin:/usr/sbin:/sbin" TERM=dumb LANG=ja_JP.UTF-8 \
      "$TAKO" setup --yes --answers - > "$out" 2>&1
  echo $?
}
mcp_registered(){ /usr/bin/python3 -c '
import json,sys
try: d=json.load(open(sys.argv[1]))
except Exception: sys.exit(1)
sys.exit(0 if "tako" in (d.get("mcpServers") or {}) else 1)
' "$MCPJSON"; }
snapshot(){ # 生成物の指紋（冪等性の比較用）
  for f in "$INSTRUCTION" "$PROFILE" "$TEMPLATE" "$MCPJSON" "$DATADIR/config.yaml"; do
    if [ -f "$f" ]; then shasum "$f" | awk '{print $1}'; else echo "-"; fi
  done
}
LEGACY=""; REQDEP=""

echo "== 1) 未認証 claude / 非 TTY: 止まらず全段やって exit 0（Z2）=="
mkenv unauth
RC=$(run_pipe "$SANDBOX/1.log" -- setup)
check    "終了コード 0（#1501 前は 1）" "$RC" "0"
contains "認証段は残りとして脇に置く"   "この 1 件は人の操作が要るので、残りとして最後にお知らせします" "$SANDBOX/1.log"
contains "設定は先に進むと言う"         "設定は先に進めます" "$SANDBOX/1.log"
contains "完了と言い切らない"           "セットアップはここまで完了しました（人の操作が残り 1 件）。" "$SANDBOX/1.log"
contains "残りの件数を出す"             "残り 1 件（ここから先は人の操作が必要です）:" "$SANDBOX/1.log"
contains "残りはログイン"               "Claude アカウントへのログイン" "$SANDBOX/1.log"
contains "最簡コマンドを 1 行で出す"    "claude auth login" "$SANDBOX/1.log"
contains "再開のしかたを言う"           "済んだら tako setup をもう一度実行してください" "$SANDBOX/1.log"
absent   "同じログインを 2 行出さない"  "残り 2 件" "$SANDBOX/1.log"
exists   "指示ファイルができる"         "$INSTRUCTION"
exists   "プロファイルができる"         "$PROFILE"
exists   "テンプレが展開される"         "$TEMPLATE"
if mcp_registered; then ok "MCP 登録が実際に載る（未認証でも通る）"; else ng "MCP 登録が載っていない"; fi
contains "master の案内は残りに委ねる"  "tako master（オーケストレーション）は、下の残り作業が済むと使えます" "$SANDBOX/1.log"
absent   "agy 前提の文面を出さない"     "agy は worker 専用です" "$SANDBOX/1.log"
# ログイン待ちでも #1502（tako CLI の PATH 設置）と #1499（依存の案内）の段は走る
contains "tako CLI の設置段も走る"     "[設置] tako CLI" "$SANDBOX/1.log"
if [ -L "$HOMEDIR/.local/bin/tako" ]; then ok "tako の symlink が張られる（#1502 と同居）"; else ng "tako の symlink が無い"; fi
contains "profile へ PATH を通す"      "undo-path" "$SANDBOX/1.log"
contains "依存の案内も出る"            "いま入れる: tako setup deps install" "$SANDBOX/1.log"

echo "== 2) 2 回目は冪等（差分なし・残りは同じ）=="
BEFORE="$(snapshot)"
RC=$(run_pipe "$SANDBOX/2.log" -- setup)
check    "終了コード 0"           "$RC" "0"
contains "実変更ゼロ"             "セットアップ結果: 変更なし（前回の設定は最新）" "$SANDBOX/2.log"
contains "残りは同じ 1 件"        "残り 1 件（ここから先は人の操作が必要です）:" "$SANDBOX/2.log"
contains "MCP は登録済みと出る"   "[OK] Claude MCP: tako が登録済み" "$SANDBOX/2.log"
AFTER="$(snapshot)"
if [ "$BEFORE" = "$AFTER" ]; then ok "生成物は 1 バイトも変わらない"; else ng "生成物が変わった"; fi

echo "== 3) --check も同じ結末（読むだけ・残りを同じ文面で出す）=="
RC=$(run_pipe "$SANDBOX/3.log" -- setup --check)
check    "終了コード 0"        "$RC" "0"
contains "残りを出す"          "残り 1 件（ここから先は人の操作が必要です）:" "$SANDBOX/3.log"
contains "残りはログイン"      "claude auth login" "$SANDBOX/3.log"
AFTER3="$(snapshot)"
if [ "$AFTER" = "$AFTER3" ]; then ok "--check は何も書かない"; else ng "--check が生成物を変えた"; fi

echo "== 4) --yes（非対話）でも同じ結末 =="
mkenv unauth
RC=$(run_pipe "$SANDBOX/4.log" -- setup --yes)
check    "終了コード 0"           "$RC" "0"
contains "残りを出す"             "残り 1 件（ここから先は人の操作が必要です）:" "$SANDBOX/4.log"
exists   "指示ファイルができる"   "$INSTRUCTION"
exists   "プロファイルができる"   "$PROFILE"

echo "== 5) dispatch SetupRun / MCP tako_setup と同条件（--yes --answers - をパイプ）=="
mkenv unauth
RC=$(run_dispatch "$SANDBOX/5.log" '{}')
check    "終了コード 0（dispatch は非 0 を失敗として返す）" "$RC" "0"
contains "残りを出す"             "残り 1 件（ここから先は人の操作が必要です）:" "$SANDBOX/5.log"
exists   "プロファイルができる"   "$PROFILE"
if mcp_registered; then ok "MCP 登録が載る"; else ng "MCP 登録が載っていない"; fi

echo "== 6) 端末あり（TTY）でも exit 0・エージェントは起こさない =="
mkenv unauth
RC=$(run_pty "$SANDBOX/6.log" N N -- setup)
check    "終了コード 0"                  "$RC" "0"
contains "残りを出す"                    "残り 1 件（ここから先は人の操作が必要です）:" "$SANDBOX/6.log"
absent   "未認証の CLI を起こさない"     "対話アシスタントを起動します" "$SANDBOX/6.log"
exists   "指示ファイルができる"          "$INSTRUCTION"
exists   "プロファイルができる"          "$PROFILE"

echo "== 7) A/B: TAKO_1501_LEGACY=1 は #1501 前（exit 1・何も整わない）=="
mkenv unauth
LEGACY=1
RC=$(run_pipe "$SANDBOX/7.log" -- setup)
LEGACY=""
check    "終了コード 1（症状）"          "$RC" "1"
contains "ログインで止まる"              "ログイン" "$SANDBOX/7.log"
missing  "指示ファイルができない（症状）" "$INSTRUCTION"
missing  "プロファイルができない（症状）" "$PROFILE"
missing  "テンプレも出ない（症状）"       "$TEMPLATE"
absent   "残りの案内も出ない（症状）"     "残り 1 件" "$SANDBOX/7.log"

echo "== 8) 認証済みスタブでは従来と一致（回帰なし）=="
mkenv auth
RC=$(run_pipe "$SANDBOX/8.log" -- setup)
check    "終了コード 0"              "$RC" "0"
contains "完了と言い切る"            "セットアップが完了しました。" "$SANDBOX/8.log"
absent   "残りの節を出さない"        "残り 1 件" "$SANDBOX/8.log"
absent   "ここまで完了と言わない"    "セットアップはここまで完了しました" "$SANDBOX/8.log"
contains "従来の次の一歩を出す"      "tako master   オーケストレーションを開始します" "$SANDBOX/8.log"
exists   "指示ファイルができる"      "$INSTRUCTION"
exists   "プロファイルができる"      "$PROFILE"

echo "== 9) エージェント CLI が 1 つも無い（Z4 の実経路）=="
mkenv none
RC=$(run_pipe "$SANDBOX/9.log" -- setup)
check    "終了コード 0（#1501 前は 1）" "$RC" "0"
contains "1 つも無いことを言う"         "[不足] claude / codex / agy のいずれも見つかりません" "$SANDBOX/9.log"
contains "残りは導入"                   "Claude Code の導入" "$SANDBOX/9.log"
absent   "ネットワークからは入らない"   "Claude Code を導入しました" "$SANDBOX/9.log"
contains "最簡コマンドを出す"           "tako setup bootstrap install" "$SANDBOX/9.log"
exists   "指示ファイルは先に作る"       "$INSTRUCTION"
exists   "プロファイルは先に作る"       "$PROFILE"
exists   "テンプレは先に展開する"       "$TEMPLATE"
absent   "総称と具体を 2 行出さない"    "残り 2 件" "$SANDBOX/9.log"

echo "== 10) 必須依存が欠けている（Z4。required=true を人工的に作る）=="
mkenv unauth
REQDEP=tmux
RC=$(run_pipe "$SANDBOX/10.log" -- setup)
check    "終了コード 0（#1501 前は 1）" "$RC" "0"
contains "必須として出る"               "[不足] tmux: 見つかりません（必須）" "$SANDBOX/10.log"
contains "残りは 2 件（ログイン + 依存）" "残り 2 件（ここから先は人の操作が必要です）:" "$SANDBOX/10.log"
contains "依存の最簡コマンドを出す"     "tako setup deps install" "$SANDBOX/10.log"
exists   "プロファイルは作られる"       "$PROFILE"
LEGACY=1
RC=$(run_pipe "$SANDBOX/10b.log" -- setup)
LEGACY=""
check    "旧アームは必須依存でも exit 1（症状）" "$RC" "1"
REQDEP=""

echo "== 11) 選んだ系統だけ未認証（Z3。codex は認証済み）=="
mkenv unauth auth
RC=$(run_dispatch "$SANDBOX/11.log" '{"selected_agent":"claude"}')
check    "終了コード 0（#1501 前は 1）" "$RC" "0"
contains "選んだ系統が未認証だと言う"   "[残り] claude は未認証です（この後の設定は先に進めます）" "$SANDBOX/11.log"
contains "残りはログイン"               "claude auth login" "$SANDBOX/11.log"
exists   "claude 側の指示ファイルを作る" "$INSTRUCTION"
exists   "プロファイルを作る"            "$PROFILE"

echo "== 12) 未認証 + 依存も未導入: ログイン待ちでも [y/N] の導入は通る（#1499 と同居）=="
mkenv unauth
write_brew_ok
RC=$(run_pty "$SANDBOX/12.log" y N -- setup)
check    "終了コード 0"                  "$RC" "0"
contains "依存に [y/N] が出る"           "tmux をインストールしますか？ [y/N]:" "$SANDBOX/12.log"
contains "y でその場導入が走る"          "==> Fetching tmux" "$SANDBOX/12.log"
contains "同じ実行の中で再検出される"    "（インストール完了）" "$SANDBOX/12.log"
contains "残りはログインだけ"            "残り 1 件（ここから先は人の操作が必要です）:" "$SANDBOX/12.log"
if [ -x "$BIN/tmux" ]; then ok "tmux の実体が置かれた"; else ng "tmux の実体が無い"; fi
exists   "プロファイルもできる"          "$PROFILE"

echo "== 13) 種別の分からないログインシェル（#2550）: profile へ書かずに見送り、setup は完走 =="
mkenv unauth
RC=$(LOGIN_SHELL="$BIN/isosh" run_pipe "$SANDBOX/13.log" -- setup)
check    "終了コード 0"                  "$RC" "0"
contains "PATH 設置は見送りと言う"       "[警告] tako CLI の PATH 設置を見送りました" "$SANDBOX/13.log"
contains "理由は既存の断り"              "使っているシェル（$BIN/isosh）の設定ファイルが分かりません" "$SANDBOX/13.log"
# 足すディレクトリは書く行と同じ `$HOME/…` の式で 1 回だけ・sh の行を推測して出さない（#2557。
# 修正前は `  export PATH="~/.local/bin:$PATH"` が tako CLI と 3 系統のランチャーの数 = 4 行並んだ）
# shellcheck disable=SC2016
check    "足すディレクトリは 1 回だけ"   "$(grep -cxF '  $HOME/.local/bin' "$SANDBOX/13.log")" "1"
absent   "sh の行を推測しない"           'export PATH=' "$SANDBOX/13.log"
# shellcheck disable=SC2088
absent   "~ 短縮を案内しない"            '~/.local/bin' "$SANDBOX/13.log"
absent   "書いたとは言わない"            "[設置] tako CLI" "$SANDBOX/13.log"
if [ -e "$HOMEDIR/.zprofile" ] || [ -e "$HOMEDIR/.bash_profile" ]; then
  ng "そのシェルが読まない profile を書いた"
else
  ok "profile を書かない"
fi
contains "残りはログインのまま"          "残り 1 件（ここから先は人の操作が必要です）:" "$SANDBOX/13.log"
# `tako setup bootstrap path` も同じ断りで止まるので、そこへ誘導しない（#2564。修正前は
# 「外部ターミナルでも使うなら tako setup bootstrap path を実行してください」= 堂々巡り）
absent   "同じ断りで止まるコマンドへ誘導しない" "tako setup bootstrap path" "$SANDBOX/13.log"
contains "足すディレクトリで閉じる"      "上のディレクトリをご自身のシェルの設定ファイルへ足せば完了です" "$SANDBOX/13.log"

# claude の公式インストーラの置き場（PATH の外）へ claude を移す = bootstrap の段が PATH を通そうとする形
move_claude_off_path(){ mkdir -p "$HOMEDIR/.local/bin" && mv -f "$BIN/claude" "$HOMEDIR/.local/bin/claude"; }

echo "== 14) 種別の分からないシェル + claude が PATH の外（#2564）: 残りも --check も bootstrap path へ誘導しない =="
mkenv unauth
move_claude_off_path
RC=$(LOGIN_SHELL="$BIN/isosh" run_pipe "$SANDBOX/14.log" -- setup)
check    "終了コード 0"                  "$RC" "0"
absent   "同じ断りで止まるコマンドへ誘導しない" "tako setup bootstrap path" "$SANDBOX/14.log"
contains "残りに PATH の 1 件がある"     "claude コマンドをどのターミナルからも使えるようにする" "$SANDBOX/14.log"
# 残りの理由の行（字下げ 5 + 断りの字下げ 2）に足すディレクトリ。要点 1 行へ縮めると落ちていた
# shellcheck disable=SC2016
check    "残りの理由に足すディレクトリ"  "$(grep -cxF '       $HOME/.local/bin' "$SANDBOX/14.log")" "1"
contains "残りはここから再開と言う"      "済んだら tako setup をもう一度実行してください" "$SANDBOX/14.log"
RC=$(LOGIN_SHELL="$BIN/isosh" run_pipe "$SANDBOX/14c.log" -- setup --check)
check    "--check も終了コード 0"        "$RC" "0"
absent   "--check も誘導しない"          "tako setup bootstrap path" "$SANDBOX/14c.log"
# shellcheck disable=SC2016
contains "--check は足すディレクトリを示す" '$HOME/.local/bin をそのシェルの書き方で PATH へ足してください' "$SANDBOX/14c.log"
# shellcheck disable=SC2016
check    "--check の残りにも足すディレクトリ" "$(grep -cxF '       $HOME/.local/bin' "$SANDBOX/14c.log")" "1"

echo "== 15) 種別の分からないシェルで案内どおり足し終えた（#2564）: setup は見送りと言わず、2 回目は使えると言う =="
mkenv unauth
move_claude_off_path
# 利用者が自分の設定ファイルへ足し終えた形（ログインシェルが $HOME/.local/bin を PATH の先頭へ足す）。
# 名前は種別の分からないシェルのまま
printf '#!/bin/sh\nif [ "$1" = "-l" ]; then shift; PATH="$HOME/.local/bin:$PATH"; export PATH; fi\nexec /bin/sh "$@"\n' > "$BIN/isosh-added"
chmod +x "$BIN/isosh-added"
RC=$(LOGIN_SHELL="$BIN/isosh-added" run_pipe "$SANDBOX/15a.log" -- setup)
check    "終了コード 0"                  "$RC" "0"
RC=$(LOGIN_SHELL="$BIN/isosh-added" run_pipe "$SANDBOX/15b.log" -- setup)
check    "2 回目も終了コード 0"          "$RC" "0"
absent   "見送りと言わない"              "PATH 設置を見送りました" "$SANDBOX/15a.log"
absent   "2 回目も見送りと言わない"      "PATH 設置を見送りました" "$SANDBOX/15b.log"
contains "2 回目は使えると言う"          "[OK] tako CLI: どのターミナルからも使えます" "$SANDBOX/15b.log"
# tako はそのシェルの設定ファイルへ書いていない = 戻す PATH のブロックが無い（undo-path も断る）
absent   "書いていない PATH を戻す案内をしない" "bootstrap undo-path" "$SANDBOX/15a.log"
absent   "PATH の残りが無い"             "claude コマンドをどのターミナルからも使えるようにする" "$SANDBOX/15b.log"
if [ -e "$HOMEDIR/.zprofile" ] || [ -e "$HOMEDIR/.bash_profile" ]; then
  ng "そのシェルが読まない profile を書いた"
else
  ok "profile を書かない"
fi

echo
echo "PASS=$PASS FAIL=$FAIL"
[ "$FAIL" -eq 0 ]
