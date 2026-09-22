#!/bin/bash
# test-setup-check-single-source-1505.sh — 診断の正本が 1 つであることの実経路テスト（#1505）
#
# 何を確かめるか:
#   `tako setup --check` と `tako check-health` が**同じ 1 実装**（`tako_control::diagnostics`）
#   から項目を出すか。#1505 前は診断が 2 本あり、`--check` にはシェル統合・tako CLI の
#   PATH・更新・remote の行が無く、PATH は `check-health` だけが別口で見ていた（棚卸し Z19）。
#
#   ここで見るのは 4 通りの経路が**同じ状態に対して同じ答え**を出すこと:
#     1. `tako setup --check`（CLI・人向けの行）
#     2. `tako check-health --json`（アプリへ届かないとき = ローカル経路）
#     3. `tako check-health --json`（アプリへ届くとき = dispatch 経路）
#     4. MCP `tako_check_health`（`tako mcp serve` の stdio ブリッジ = claude と同じ経路）
#
#   項目の一致は**キーの集合**と**行の字面**の両方で見る（`lines` は `--check` が
#   そのまま出す文字列なので、片方だけ文言を変えたらここで落ちる）。
#
# どう隔離するか（**本番の設定・rc・GUI へ 1 バイトも触らない**）:
#   - `HOME` / `TAKO_DATA_DIR` / `TAKO_ORCHESTRATOR_DIR` / `TAKO_DISCOVERY_DIR` を一時へ
#   - 依存（claude / codex / agy / tmux / git / tailscale）はすべてスタブ
#   - GUI は**仮想ディスプレイ上の隔離インスタンス**だけ（#1141 / #1490）
#
# 使い方: bash scripts/test-setup-check-single-source-1505.sh
#         TAKO_1505_SKIP_GUI=1 bash scripts/…  # CLI 経路だけ（GUI を立てない）
#
# 配列は `${arr[@]+"${arr[@]}"}` で展開する（macOS 同梱の bash 3.2 は `set -u` の下で
# 空配列の `"${arr[@]}"` を未定義として落とす。`.agent/conventions.md`）

set -uo pipefail

# **本番 GUI を指す env を最初に落とす**（tako のペインの中から走らせると継承される）
unset TAKO_SOCKET TAKO_TOKEN TAKO_PANE_ID TAKO_MCP_URL

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TAKO="${TAKO_BIN:-$ROOT/target/debug/tako}"
PASS=0; FAIL=0

ok()   { PASS=$((PASS+1)); echo "  PASS: $1"; }
ng()   { FAIL=$((FAIL+1)); echo "  FAIL: $1"; }
check(){ if [ "$2" = "$3" ]; then ok "$1"; else ng "$1 (期待 '$3' / 実際 '$2')"; fi; }
contains(){ if grep -qF -- "$2" "$3" 2>/dev/null; then ok "$1"; else ng "$1 ('$2' が出ていない)"; fi; }
absent(){ if grep -qF -- "$2" "$3" 2>/dev/null; then ng "$1 ('$2' が出ている)"; else ok "$1"; fi; }

[ -x "$TAKO" ] || { echo "tako が無い: ${TAKO}（cargo build -p tako-cli）"; exit 1; }
[ -x /usr/bin/python3 ] || { echo "/usr/bin/python3 が無い環境では走らない"; exit 1; }

SANDBOX="$(mktemp -d "${TMPDIR:-/tmp}/tako-1505-XXXXXX")" || exit 1
APP_PID=""
cleanup() {
    [ -n "$APP_PID" ] && stop_isolated_gui "$APP_PID"
    # **一時ディレクトリ配下であることを確かめてから消す**（実環境破壊の再発防止）
    case "$SANDBOX" in */tako-1505-*) rm -rf "$SANDBOX" ;; esac
}
trap cleanup EXIT

BIN="$SANDBOX/bin"
HOMEDIR="$SANDBOX/home"
DATADIR="$SANDBOX/d"

# --- スタブと隔離 HOME -------------------------------------------------------
# `authed`（claude を認証済みにするか）/ `withdeps`（任意依存を置くか）で状態を作り分ける。
# **tmux は不在の側に使えない**: 器の解決（`setup_deps::resolve`）は PATH 外の実体も
# 拾うので、走らせる機に tmux が入っていると「不在」を作れない（実測で踏んだ）
mkstubs(){
  local authed="${1:-1}" withdeps="${2:-1}" dep
  rm -rf "$BIN" "$HOMEDIR" "$DATADIR"
  mkdir -p "$BIN" "$HOMEDIR/.local/bin" "$DATADIR"
  # exe::find のログインシェル経由の解決を隔離 PATH に閉じ込める
  printf '#!/bin/sh\nif [ "$1" = "-l" ]; then shift; fi\nexec /bin/sh "$@"\n' > "$BIN/isosh"
  if [ "$authed" = "1" ]; then
    cat > "$BIN/claude" <<'EOF'
#!/bin/sh
case "$*" in
  "auth status --json") echo '{"loggedIn":true,"authMethod":"claude.ai","subscriptionType":"Pro"}'; exit 0;;
  "--version") echo "2.1.258 (Claude Code)"; exit 0;;
  "mcp list"*) echo "tako: /stub/tako mcp serve - Connected"; exit 0;;
esac
exit 0
EOF
  else
    cat > "$BIN/claude" <<'EOF'
#!/bin/sh
case "$*" in
  "auth status --json") echo '{"loggedIn":false}'; exit 0;;
  "--version") echo "2.1.258 (Claude Code)"; exit 0;;
  "mcp list"*) echo "No MCP servers configured."; exit 0;;
esac
exit 0
EOF
  fi
  # codex / agy も置く（MCP 登録の項目が 3 系統ぶん出ることを見る）
  cat > "$BIN/codex" <<'EOF'
#!/bin/sh
case "$*" in
  "login status") echo "Logged in using ChatGPT"; exit 0;;
  "mcp list"*) echo '[]'; exit 0;;
esac
exit 0
EOF
  cat > "$BIN/agy" <<'EOF'
#!/bin/sh
case "$*" in
  "models") echo "gemini-2.5-pro"; exit 0;;
  "mcp list"*) echo '{}'; exit 0;;
esac
exit 0
EOF
  printf '#!/bin/sh\necho "git (stub)"\nexit 0\n' > "$BIN/git"
  printf '#!/bin/sh\necho "tmux (stub)"\nexit 0\n' > "$BIN/tmux"
  [ "$withdeps" = "1" ] && printf '#!/bin/sh\necho "tailscale (stub)"\nexit 0\n' > "$BIN/tailscale"
  chmod +x "$BIN"/*
}

iso_env(){
  echo "HOME=$HOMEDIR" "TAKO_DATA_DIR=$DATADIR" "TAKO_ORCHESTRATOR_DIR=$DATADIR/orch" \
       "TAKO_TMUX_SOCKET=$SANDBOX/tmux.sock" "TAKO_DISCOVERY_DIR=$SANDBOX/nowhere" \
       "TAKO_ISOLATED=1" "SHELL=$BIN/isosh" "CODEX_HOME=$HOMEDIR/.codex" \
       "PATH=$BIN:/usr/bin:/bin:/usr/sbin:/sbin" "TERM=dumb" "LANG=ja_JP.UTF-8"
}
run_iso(){  # run_iso <出力先> -- <tako の引数...>（stdout + stderr）
  local out="$1"; shift; shift
  env -i $(iso_env) "$TAKO" "$@" < /dev/null > "$out" 2>&1
  echo $?
}
# JSON は **stdout だけ**を採る（`check-health` は届かないとき stderr にも理由を出す）
run_iso_json(){  # run_iso_json <出力先> -- <tako の引数...>
  local out="$1"; shift; shift
  env -i $(iso_env) "$TAKO" "$@" < /dev/null > "$out" 2>"$out.err"
  echo $?
}

# ディレクトリ配下の中身の指紋（相対パスで取るので置き場所が違っても比較できる）
tree_hash(){ ( cd "$1" 2>/dev/null && find . -type f | LC_ALL=C sort | xargs shasum 2>/dev/null ) | shasum | cut -d' ' -f1; }

# --- JSON から診断を読む小道具 ----------------------------------------------
PY_KEYS='import json,sys
d=json.load(open(sys.argv[1]))
print("\n".join((d.get("diagnostics") or {}).get("keys") or []))'
PY_LINES='import json,sys
d=json.load(open(sys.argv[1]))
for item in ((d.get("diagnostics") or {}).get("items") or []):
    for line in item.get("lines") or []:
        print(line)'
PY_STATUS='import json,sys
d=json.load(open(sys.argv[1]))
for item in ((d.get("diagnostics") or {}).get("items") or []):
    print("%s=%s" % (item["key"], item["status"]))'
PY_REMAIN='import json,sys
d=json.load(open(sys.argv[1]))
for r in ((d.get("diagnostics") or {}).get("remaining") or []):
    print(r.get("command") or r.get("title"))'
# MCP の応答（stdio ブリッジ）から tako_check_health の本文を取り出す
PY_MCP='import json,sys
for line in open(sys.argv[1]):
    line=line.strip()
    if not line:
        continue
    try:
        msg=json.loads(line)
    except Exception:
        continue
    if msg.get("id")!=2:
        continue
    content=(msg.get("result") or {}).get("content") or []
    sys.stdout.write(content[0].get("text") if content else "{}")
    sys.exit(0)
sys.stdout.write("{}")'

keys_of(){ /usr/bin/python3 -c "$PY_KEYS" "$1"; }
lines_of(){ /usr/bin/python3 -c "$PY_LINES" "$1"; }
status_of(){ /usr/bin/python3 -c "$PY_STATUS" "$1"; }
remain_of(){ /usr/bin/python3 -c "$PY_REMAIN" "$1"; }

# `--check` の出力に、JSON の各項目の行が**字面どおり**現れるか
lines_match(){  # lines_match <ラベル> <json> <check の出力>
  local label="$1" json="$2" out="$3" missing=0 seen=0 line
  while IFS= read -r line; do
    [ -z "$line" ] && continue
    seen=$((seen+1))
    grep -qxF -- "$line" "$out" || { missing=$((missing+1)); echo "      欠けている行: $line"; }
  done <<EOF
$(lines_of "$json")
EOF
  # **0 行での空振りを緑にしない**（JSON が読めなかったときに気づけないため）
  if [ "$seen" -lt 5 ]; then ng "${label}（JSON から読めた行が $seen 本しかない）"; return; fi
  if [ "$missing" = "0" ]; then ok "${label}（$seen 行）"; else ng "${label}（$missing / $seen 行が --check に無い）"; fi
}

echo "診断の正本が 1 つであること（#1505）— sandbox: $SANDBOX"
echo

# =============================================================================
echo "== A) CLI: setup --check と check-health が同じ項目を出す（アプリ無し）=="
mkstubs 1 1
RC=$(run_iso "$SANDBOX/a-check.log" -- setup --check)
check "setup --check は完走する（#1501 の契約）" "$RC" "0"
HOME_BEFORE=$(tree_hash "$HOMEDIR")
DATA_BEFORE=$(tree_hash "$DATADIR")
RC=$(run_iso_json "$SANDBOX/a-health.json" -- check-health --json)
check "アプリへ届かないときは 1（理由は JSON に載る）" "$RC" "1"
check "HOME は 1 バイトも変わらない" "$(tree_hash "$HOMEDIR")" "$HOME_BEFORE"
check "データディレクトリも変わらない" "$(tree_hash "$DATADIR")" "$DATA_BEFORE"

KEYS_CHECK="$SANDBOX/a-keys-health.txt"
keys_of "$SANDBOX/a-health.json" > "$KEYS_CHECK"
check "診断の項目が 1 件以上ある" "$([ -s "$KEYS_CHECK" ] && echo yes || echo no)" "yes"
lines_match "check-health の全行が --check にそのまま出る" "$SANDBOX/a-health.json" "$SANDBOX/a-check.log"

echo "   項目: $(tr '\n' ' ' < "$KEYS_CHECK")"
for key in agent_cli tako_cli_path shell_integration agents_detected dep.tmux \
           mcp.claude mcp.codex mcp.agy setup update remote ipc; do
  if grep -qxF "$key" "$KEYS_CHECK"; then ok "項目 $key がある"; else ng "項目 $key が無い"; fi
done
# #1505 前に `--check` から落ちていた 4 種（Z19 の実測）
contains "--check にシェル統合の行が出る" "シェル統合" "$SANDBOX/a-check.log"
contains "--check に tako CLI の PATH の行が出る" "tako CLI の PATH" "$SANDBOX/a-check.log"
contains "--check にリモート公開の行が出る" "リモート公開" "$SANDBOX/a-check.log"
contains "--check に IPC の受け口の行が出る" "IPC の受け口" "$SANDBOX/a-check.log"
# 検出・プランの表示（#989 の非退行）
contains "--check が認証とプランを表示する" "認証済み / pro" "$SANDBOX/a-check.log"
absent   "--check は何も聞かない" "[y/N]" "$SANDBOX/a-check.log"

# =============================================================================
echo "== B) 欠けている状態でも同じ答え（tailscale 無し・claude 未ログイン）=="
mkstubs 0 0
RC=$(run_iso "$SANDBOX/b-check.log" -- setup --check)
check "詰まっていても完走する（#1501）" "$RC" "0"
RC=$(run_iso_json "$SANDBOX/b-health.json" -- check-health --json)
check "check-health も答えを返す" "$RC" "1"
lines_match "欠けている状態でも行が一致する" "$SANDBOX/b-health.json" "$SANDBOX/b-check.log"
status_of "$SANDBOX/b-health.json" > "$SANDBOX/b-status.txt"
check "tailscale は不在として申告される" "$(grep '^dep.tailscale=' "$SANDBOX/b-status.txt")" "dep.tailscale=optional"
contains "--check が tailscale の不在を出す" "[任意] tailscale: 見つかりません（任意）" "$SANDBOX/b-check.log"
contains "--check がログイン待ちを出す" "ログイン" "$SANDBOX/b-check.log"
remain_of "$SANDBOX/b-health.json" > "$SANDBOX/b-remain.txt"
check "残り作業が JSON にも載る" "$([ -s "$SANDBOX/b-remain.txt" ] && echo yes || echo no)" "yes"
while IFS= read -r cmd; do
  [ -z "$cmd" ] && continue
  grep -qF -- "$cmd" "$SANDBOX/b-check.log" || ng "残りの 1 手が --check に無い: $cmd"
done < "$SANDBOX/b-remain.txt"
ok "残りの 1 手は --check の末尾と一致する"

# =============================================================================
if [ -n "${TAKO_1505_SKIP_GUI:-}" ]; then
  echo "== C/D) 隔離 GUI は TAKO_1505_SKIP_GUI=1 のため省略 =="
else
echo "== C) dispatch 経路（隔離 GUI へ届くとき）=="
# shellcheck source=lib/isolated-gui.sh
. "$ROOT/scripts/lib/isolated-gui.sh"
isolated_gui_bins || exit 1
mkstubs 1 1
export HOME="$HOMEDIR"
export TAKO_DATA_DIR="$DATADIR"
export TAKO_ORCHESTRATOR_DIR="$DATADIR/orch"
export TAKO_DISCOVERY_DIR="$SANDBOX/disc"
export TAKO_TMUX_SOCKET="$SANDBOX/tmux.sock"
export CODEX_HOME="$HOMEDIR/.codex"
export SHELL="$BIN/isosh"
export PATH="$BIN:/usr/bin:/bin:/usr/sbin:/sbin"
mkdir -p "$TAKO_DISCOVERY_DIR"
launch_isolated_gui "$SANDBOX/app.log"
APP_PID="$ISOLATED_GUI_PID"
if wait_isolated_gui "$SANDBOX/app.log"; then
  ok "隔離 GUI が立った（pid ${APP_PID}）"
  "$TAKO" check-health --json > "$SANDBOX/c-health.json" 2>"$SANDBOX/c-health.err"
  check "届くときの check-health は 0" "$?" "0"
  "$TAKO" setup --check > "$SANDBOX/c-check.log" 2>&1
  lines_match "dispatch の答えも --check と字面まで一致する" "$SANDBOX/c-health.json" "$SANDBOX/c-check.log"
  keys_of "$SANDBOX/c-health.json" > "$SANDBOX/c-keys.txt"
  check "IPC の受け口が立っていると申告する" \
        "$(grep '^ipc=' <(status_of "$SANDBOX/c-health.json"))" "ipc=ok"

  echo "== D) MCP tako_check_health（stdio ブリッジ = claude と同じ経路）=="
  # ブリッジは**環境変数だけ**で接続先を決める（tako の外で 0 ツールにする設計 =
  # FR-2.3.2）。ペインの中と同じになるよう、隔離インスタンスの接続情報を渡す
  CONTROL_JSON="$TAKO_DISCOVERY_DIR/control.json"
  if [ -f "$CONTROL_JSON" ]; then
    TAKO_SOCKET="$(/usr/bin/python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["socket"])' "$CONTROL_JSON")"
    TAKO_TOKEN="$(/usr/bin/python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["token"])' "$CONTROL_JSON")"
    export TAKO_SOCKET TAKO_TOKEN
  else
    ng "隔離インスタンスの接続情報が無い（${CONTROL_JSON}）"
  fi
  cat > "$SANDBOX/mcp-in.jsonl" <<'JSONL'
{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"test-1505","version":"0"}}}
{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"tako_check_health","arguments":{}}}
JSONL
  "$TAKO" mcp serve < "$SANDBOX/mcp-in.jsonl" > "$SANDBOX/mcp-out.jsonl" 2>"$SANDBOX/mcp.err"
  /usr/bin/python3 -c "$PY_MCP" "$SANDBOX/mcp-out.jsonl" > "$SANDBOX/d-health.json"
  keys_of "$SANDBOX/d-health.json" > "$SANDBOX/d-keys.txt"
  if [ -s "$SANDBOX/d-keys.txt" ]; then
    check "MCP の項目集合が dispatch と同じ" \
          "$(shasum < "$SANDBOX/d-keys.txt" | cut -d' ' -f1)" \
          "$(shasum < "$SANDBOX/c-keys.txt" | cut -d' ' -f1)"
    lines_match "MCP の答えも --check と字面まで一致する" "$SANDBOX/d-health.json" "$SANDBOX/c-check.log"
  else
    ng "MCP tako_check_health が診断を返さない（$(head -c 200 "$SANDBOX/mcp.err")）"
  fi
else
  ng "隔離 GUI が立たない（$(tail -3 "$SANDBOX/app.log" | tr '\n' ' ')）"
fi
stop_isolated_gui "$APP_PID"; APP_PID=""
fi

echo
echo "結果: PASS=$PASS FAIL=$FAIL"
[ "$FAIL" = "0" ] || exit 1
