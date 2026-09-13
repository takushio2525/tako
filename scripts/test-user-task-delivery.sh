#!/usr/bin/env bash
# test-user-task-delivery.sh — ユーザー向けタスクと返答の配送の実経路テスト（#1450 の分割 B1）
#
# 隔離した data / orchestrator ディレクトリで**実 tako-app + 実 tako CLI**を走らせ、
#   ① CLI / MCP の往復（add → list → show → update → done）が同じ 1 実装を通る
#   ② 永続がアプリの再起動をまたいで残る
#   ③ 起票元（profile / pane / session）が呼び出し元から自動で入る
#   ④ 返答が**生きている master のペイン**へ届く（delivery=sent → delivered）
#   ⑤ master が居なければ**新しいタブで起動**して初回メッセージに載る（delivery=launched）
#   ⑥ 配送できないとき（プロファイル不明）は**理由つきで残る**（delivery=failed）
# を実測する。
#
# **本番の tako 設定・本番 GUI・本番の tmux には一切触らない**
# （data / orchestrator / sessions は mktemp 配下、tmux は専用ソケット、
# claude は同梱のスタブへ向く = 実エージェントは起動しない）。
# 窓は隔離起動の既定（#1141 の仮想ディスプレイ tako-vd）へ出る。
#
# 使い方: bash scripts/test-user-task-delivery.sh
#   A/B: TAKO_1450_LEGACY=1 bash scripts/test-user-task-delivery.sh （通知が出ない旧挙動）
set -uo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PASS=0
FAIL=0

pass() { PASS=$((PASS + 1)); echo "  [OK] $1"; }
fail() { FAIL=$((FAIL + 1)); echo "  [NG] $1"; }
check_eq() {
  if [ "$2" = "$3" ]; then pass "$1"; else fail "$1（期待 '${2}' / 実際 '${3}'）"; fi
}
check_contains() {
  case "$2" in
    *"$3"*) pass "$1" ;;
    *) fail "$1（'${3}' を含まない: ${2}）" ;;
  esac
}
# 画面テキストは狭いペインで折り返す（`tako read` は見えている行をそのまま返す）。
# 折り返しで落ちないよう、**空白を全部落としてから**突き合わせる
squash() { printf '%s' "$1" | tr -d ' \t\r\n'; }
check_screen_contains() {
  if [ -n "$(squash "$3")" ] && case "$(squash "$2")" in *"$(squash "$3")"*) true ;; *) false ;; esac; then
    pass "$1"
  else
    fail "$1（'${3}' を含まない）"
  fi
}
check_screen_not_contains() {
  if case "$(squash "$2")" in *"$(squash "$3")"*) true ;; *) false ;; esac; then
    fail "$1（'${3}' を含んでいる）"
  else
    pass "$1"
  fi
}
check_not_contains() {
  case "$2" in
    *"$3"*) fail "$1（'${3}' を含んでいる）" ;;
    *) pass "$1" ;;
  esac
}

TMP="$(mktemp -d "${TMPDIR:-/tmp}/tako-1450-XXXXXX")"
APP_PID=""
cleanup() {
  # 明示 pid だけを落とす（pkill / killall は本番 GUI にも当たる）
  if [ -n "$APP_PID" ]; then kill "$APP_PID" 2>/dev/null || true; fi
  if [ -n "${TAKO_TMUX_SOCKET:-}" ]; then
    tmux -L "$TAKO_TMUX_SOCKET" kill-server >/dev/null 2>&1 || true
  fi
  rm -rf "$TMP"
}
trap cleanup EXIT

# --- claude のスタブ（実エージェントを起動しない）-----------------------------
# エージェント TUI の最小形: 入力欄（`❯`）を出し、打たれた文字をその行へ echo し、
# Enter で行を空へ戻す。送達フロー（#32 / #1259）が見る 3 点
# （入力欄がある / 貼った本文が入力欄に出る / Enter で空へ戻る）だけを満たす
STUB_BIN="$TMP/bin"
mkdir -p "$STUB_BIN"
cat > "$STUB_BIN/claude" <<'STUB'
#!/usr/bin/env bash
echo "STUB_CLAUDE_STARTED args=$*"
stty raw -echo 2>/dev/null
printf '\r\n'
printf '❯ '
buf=""
while IFS= read -r -n1 ch; do
  if [ -z "$ch" ]; then
    # CR / LF = 送信。受け取った本文を画面へ残し、入力欄を空へ戻す
    printf '\r\n[STUB_SUBMITTED] %s\r\n' "$buf"
    buf=""
    printf '❯ '
  else
    buf="$buf$ch"
    printf '%s' "$ch"
  fi
done
STUB
chmod +x "$STUB_BIN/claude"
# master 起動コマンドの中身は `tako` CLI 自身も呼ぶ（#983 の実在検査）ので PATH へ通す

# --- 対象バイナリ -------------------------------------------------------------
TAKO_BIN="${TAKO_BIN:-$REPO_ROOT/target/debug/tako}"
APP_BIN="${APP_BIN:-$REPO_ROOT/target/debug/tako-app}"
if [ ! -x "$TAKO_BIN" ] || [ ! -x "$APP_BIN" ]; then
  echo "バイナリをビルドします…"
  (cd "$REPO_ROOT" && cargo build -p tako-cli -p tako-app --quiet)
fi
for b in "$TAKO_BIN" "$APP_BIN"; do
  [ -x "$b" ] || { echo "バイナリが見つからない: $b"; exit 1; }
done
ln -sf "$TAKO_BIN" "$STUB_BIN/tako"

# --- 隔離した環境 -------------------------------------------------------------
# **最重要**: このスクリプトは tako のペインの中から走ることがある。その環境には
# 本番 GUI の `TAKO_SOCKET` / `TAKO_TOKEN` が入っていて、CLI は**環境変数を最優先**で
# 使う（`send_request_via`）ので、外さないと `tako todo` も `tako split` も
# **本番 GUI へ飛ぶ**（実測: 検証用のペインが本番のタブに生えた）。
# 呼び出し元ペインの id も外す（起票元が本番のペインとして記録されてしまう）
unset TAKO_SOCKET TAKO_TOKEN TAKO_MCP_URL TAKO_PANE_ID TAKO_TAB_ID \
  TAKO_BACKEND_SOCKET TAKO_ORCHESTRATOR_ROLE
export TAKO_ISOLATED=1
export TAKO_DATA_DIR="$TMP/data"
export TAKO_DISCOVERY_DIR="$TMP/disc"
export TAKO_ORCHESTRATOR_DIR="$TMP/orch"
export TAKO_SESSIONS_FILE="$TMP/sessions.yaml"
export TAKO_WORKERS_FILE="$TMP/workers.yaml"
export TAKO_PANE_LOG_DIR="$TMP/panelogs"
export TAKO_TMUX_SOCKET="tako-1450-$$"
export TAKO_PERSIST=0
mkdir -p "$TAKO_DATA_DIR" "$TAKO_DISCOVERY_DIR" "$TAKO_ORCHESTRATOR_DIR/profiles"
for d in "$TAKO_DATA_DIR" "$TAKO_ORCHESTRATOR_DIR"; do
  case "$d" in
    "$TMP"/*) : ;;
    *) echo "隔離されていないディレクトリ: $d"; exit 1 ;;
  esac
done
TASKS_FILE="$TAKO_ORCHESTRATOR_DIR/user-tasks.yaml"

# 検証用プロファイル: PATH をスタブへ差し替える（実 claude を起動しない）
cat > "$TAKO_ORCHESTRATOR_DIR/profiles/t1450.yaml" <<YAML
model: null
effort: high
env:
  PATH: "$STUB_BIN:/usr/bin:/bin"
YAML

# 窓は仮想ディスプレイへ（ユーザーのメイン画面に出さない。#1141 / #1150）
bash "$REPO_ROOT/scripts/lib/virtual-display.sh" ensure >/dev/null 2>&1 || true

jqf() { python3 -c 'import json,sys
try:
    d=json.load(sys.stdin)
except Exception:
    print(""); sys.exit(0)
cur=d
for k in sys.argv[1].split("."):
    if isinstance(cur,list):
        cur = cur[int(k)] if k.lstrip("-").isdigit() and -len(cur) <= int(k) < len(cur) else None
    elif isinstance(cur,dict): cur=cur.get(k)
    else: cur=None
    if cur is None: break
print("null" if cur is None else (str(cur).lower() if isinstance(cur,bool) else cur))' "$1"; }

start_app() {
  # 仮想ディスプレイは検証の合間に眠る（眠ると列挙から落ちて起動が中止される。#1160）。
  # ②「落として起動し直す」の 2 回目で実際に踏むので、起こす手を毎回通す
  bash "$REPO_ROOT/scripts/lib/virtual-display.sh" ensure >/dev/null 2>&1 || true
  "$APP_BIN" >> "$TMP/app.log" 2>&1 &
  APP_PID=$!
  for _ in $(seq 1 300); do
    if "$TAKO_BIN" list >/dev/null 2>&1; then break; fi
    sleep 0.1
  done
  "$TAKO_BIN" list >/dev/null 2>&1 || {
    echo "tako-app へ接続できない:"; tail -30 "$TMP/app.log"; exit 1; }
  # **関門**: 繋がった先が隔離インスタンスか（socket が $TMP の下か）を必ず確かめる。
  # ここを通さないと、env の取りこぼし 1 つで本番 GUI を触ったまま「緑」になる
  local sock
  sock="$(python3 - "$TAKO_DISCOVERY_DIR" <<'PYEOF'
import json,os,sys
base=os.path.join(sys.argv[1],"instances")
out=""
if os.path.isdir(base):
    for name in sorted(os.listdir(base)):
        if name.endswith(".json"):
            try: out=json.load(open(os.path.join(base,name))).get("socket","")
            except Exception: pass
print(out)
PYEOF
)"
  case "$sock" in
    "$TMP"/*|/private"$TMP"/*|/tmp/tako-*|"${TMPDIR%/}"/tako-*) : ;;
    *) echo "隔離インスタンスに繋がっていない（socket=${sock}）。本番 GUI を触る前に中止する"; exit 1 ;;
  esac
  # 本番の master / worker が見えていたら、それは本番 GUI（隔離なら 1 タブ 1 ペイン）
  local panes
  panes="$("$TAKO_BIN" list 2>/dev/null | python3 -c 'import json,sys
d=json.load(sys.stdin)
print(sum(1 for t in d["tabs"] for p in t.get("panes",[]) if (p.get("role") or "").startswith("orchestrator-")))')"
  if [ "${panes:-0}" -gt 0 ]; then
    echo "本番 GUI に繋がっている（orchestrator role のペインが ${panes} 枚見える）。中止する"; exit 1
  fi
  return 0
}

echo "=== 準備: 隔離 tako-app を起動 ==="
start_app
pass "隔離 tako-app が起動して CLI から見える（pid ${APP_PID}）"

# --- ① CLI で起票し、MCP の口（同じ dispatch）から読む ------------------------
echo
echo "=== ① 起票と一覧（CLI / MCP は同じ 1 実装） ==="
MISSING="$TMP/まだ無い動画.mp4"
ADD_JSON="$("$TAKO_BIN" todo add "解説動画 v6 を YouTube へ投稿" \
  --kind post --body "サムネの確認をお願いします" \
  --attach "$MISSING" \
  --copy-text "タイトル=tako v0.9 の解説" \
  --copy-text "説明=AI エージェントを 1 画面で監視する OSS ターミナル" \
  --link "https://example.test/pr/1" --json 2>&1)"
ID="$(printf '%s' "$ADD_JSON" | jqf id)"
check_eq "起票で id が採番される（u-N）" "u-1" "$ID"
check_eq "添付は実在しなくても起票できる（生成前に起票する運用）" "false" \
  "$(printf '%s' "$ADD_JSON" | jqf attachments.0.exists)"
check_eq "コピー用テキストがラベルごとに分かれる" "タイトル" \
  "$(printf '%s' "$ADD_JSON" | jqf copy_texts.0.label)"

LIST_JSON="$("$TAKO_BIN" todo list --json 2>&1)"
check_eq "一覧は未完了 1 件" "1" "$(printf '%s' "$LIST_JSON" | jqf count)"
check_eq "バッジ用の未完了件数が返る" "1" "$(printf '%s' "$LIST_JSON" | jqf open_count)"
# 内訳は `TaskKind::all()` の並び（review / confirm / permission / post / other）で必ず全種類返る
check_eq "種類別の内訳は全種類ぶん返る" "5" \
  "$(printf '%s' "$LIST_JSON" | python3 -c 'import json,sys; print(len(json.load(sys.stdin)["open_counts_by_kind"]))')"
check_eq "起票した種類の件数が数えられている" "1" \
  "$(printf '%s' "$LIST_JSON" | python3 -c 'import json,sys
d=json.load(sys.stdin)
print(next(x["count"] for x in d["open_counts_by_kind"] if x["kind"]=="post"))')"

# 空タイトルは拒否（一覧で選べないものを作らせない）
EMPTY_OUT="$("$TAKO_BIN" todo add "   " 2>&1)"; EMPTY_RC=$?
check_eq "空タイトルは拒否される（終了コード）" "1" "$EMPTY_RC"
check_contains "空タイトルの理由が出る" "$EMPTY_OUT" "タイトルが空"

# --- ② 永続が再起動をまたぐ ---------------------------------------------------
echo
echo "=== ② 永続（アプリを落として起動し直す） ==="
"$TAKO_BIN" todo update u-1 --body "サムネの文字が小さいかもしれません" --json >/dev/null 2>&1
kill "$APP_PID" 2>/dev/null || true
wait "$APP_PID" 2>/dev/null || true
APP_PID=""
check_contains "YAML が置き場に在る" "$(ls "$TAKO_ORCHESTRATOR_DIR")" "user-tasks.yaml"
check_contains "版数フィールドを持つ（#916 の番地）" "$(cat "$TASKS_FILE")" "version: 1"
start_app
SHOW_JSON="$("$TAKO_BIN" todo show u-1 --json 2>&1)"
check_eq "再起動後も同じ id で読める" "u-1" "$(printf '%s' "$SHOW_JSON" | jqf id)"
check_contains "更新した本文が残っている" "$(printf '%s' "$SHOW_JSON" | jqf body)" "文字が小さい"

# --- ③ 起票元が自動で入る -----------------------------------------------------
echo
echo "=== ③ 起票元（profile / pane）の自動記録 ==="
# 呼び出し元ペインが無い（env を外した）ので、分割元は一覧から取る
ROOT_PANE="$("$TAKO_BIN" list 2>/dev/null | python3 -c 'import json,sys
d=json.load(sys.stdin)
print(next(p["id"] for t in d["tabs"] for p in t.get("panes",[])))')"
PANE="$("$TAKO_BIN" split --pane "$ROOT_PANE" 2>&1 | tr -dc '0-9')"
[ -n "$PANE" ] || { echo "ペインを作れない（root=${ROOT_PANE}）"; exit 1; }
"$TAKO_BIN" title --pane "$PANE" --role "orchestrator-master:t1450" >/dev/null 2>&1
ORIGIN_JSON="$(TAKO_ORCHESTRATOR_ROLE="master:t1450" "$TAKO_BIN" todo add "承認をください" \
  --kind confirm --json 2>&1)"
check_eq "2 件目の id" "u-2" "$(printf '%s' "$ORIGIN_JSON" | jqf id)"
check_eq "起票元のプロファイルが入る" "t1450" "$(printf '%s' "$ORIGIN_JSON" | jqf origin.profile)"
check_eq "起票者の名乗りが入る" "master:t1450" "$(printf '%s' "$ORIGIN_JSON" | jqf created_by)"

# 起票の記録が診断に残る（通知欄の 1 行は GUI のセルフテスト項目 84g が見る）
check_contains "起票が persist.log に残る" \
  "$(cat "$TAKO_DATA_DIR/persist.log" 2>/dev/null || echo '')" "ユーザータスクを起票"

# --- ④ 生きている master へ届く -----------------------------------------------
echo
echo "=== ④ 返答の配送（master が生きている） ==="
# master のペインでスタブ claude を動かす（エージェント TUI の最小形）
"$TAKO_BIN" send --pane "$PANE" "PATH=$STUB_BIN:\$PATH claude --stub" >/dev/null 2>&1
# 待つのは**入力欄が出るまで**（起動の 1 行だけでは貼り付けの準備ができていない）。
# 時間で決め打たず状態で待つ（`.agent/conventions.md`「セルフテストの待ち条件の書き方」）
for _ in $(seq 1 300); do
  case "$(squash "$("$TAKO_BIN" read --pane "$PANE" --lines 40 2>/dev/null)")" in
    *STUB_CLAUDE_STARTED*) break ;;
  esac
  sleep 0.2
done
check_screen_contains "master のペインでエージェント TUI が動いている" \
  "$("$TAKO_BIN" read --pane "$PANE" --lines 40 2>/dev/null)" "STUB_CLAUDE_STARTED"

RESP_JSON="$("$TAKO_BIN" todo respond u-2 --decision needs_change \
  --comment "サムネの文字を大きく" --via pwa --json 2>&1)"
check_eq "返答が積まれる" "needs_change" "$(printf '%s' "$RESP_JSON" | jqf responses.0.decision)"
check_eq "どこから返したかが残る" "pwa" "$(printf '%s' "$RESP_JSON" | jqf responses.0.via)"
check_eq "needs_change はタスクを閉じない" "open" "$(printf '%s' "$RESP_JSON" | jqf status)"
check_eq "配送先は生きている master のペイン" "$PANE" "$(printf '%s' "$RESP_JSON" | jqf delivery.pane)"
DELIV_STATE="$(printf '%s' "$RESP_JSON" | jqf delivery.state)"
case "$DELIV_STATE" in
  sent|delivered) pass "配送の入口が記録される（state=${DELIV_STATE}）" ;;
  *) fail "配送の入口が記録されない（state=${DELIV_STATE}）" ;;
esac

# 本文が master の入力欄へ実際に届く（スタブが受け取った行を画面へ残す）
DELIVERED=""
for _ in $(seq 1 200); do
  SCREEN="$("$TAKO_BIN" read --pane "$PANE" --lines 60 2>/dev/null)"
  case "$(squash "$SCREEN")" in
    *"$(squash '【ユーザー返答】todo u-2')"*) DELIVERED="$SCREEN"; break ;;
  esac
  sleep 0.2
done
check_screen_contains "返答が master の入力欄へ届く" "$DELIVERED" "【ユーザー返答】todo u-2"
check_screen_contains "判断が本文に載る" "$DELIVERED" "decision=needs_change"
check_screen_not_contains "タスク本文まで送っていない（生きている master は文脈を持っている）" \
  "$DELIVERED" "サムネの確認をお願いします"

# 送達の顛末が畳み込まれて delivery が決着する（読むたびに 1 回）
SETTLED=""
for _ in $(seq 1 100); do
  SETTLED="$("$TAKO_BIN" todo show u-2 --json 2>&1 | jqf delivery.state)"
  case "$SETTLED" in
    delivered|failed) break ;;
  esac
  sleep 0.3
done
check_eq "配送が delivered で決着する" "delivered" "$SETTLED"

# --- ⑤ master が居なければ起動する --------------------------------------------
echo
echo "=== ⑤ 返答の配送（master が居ない → 起動する） ==="
TABS_BEFORE="$("$TAKO_BIN" list 2>/dev/null | python3 -c 'import json,sys; print(len(json.load(sys.stdin)["tabs"]))')"
"$TAKO_BIN" close --pane "$PANE" --force >/dev/null 2>&1
sleep 0.5
LAUNCH_ID="$(TAKO_ORCHESTRATOR_ROLE="master:t1450" "$TAKO_BIN" todo add "起動して確認してほしい" \
  --kind review --body "これが初回メッセージに載るはず" --json 2>&1 | jqf id)"
LAUNCH_JSON="$("$TAKO_BIN" todo respond "$LAUNCH_ID" --decision answered --comment "起動テスト" --json 2>&1)"
LAUNCH_STATE="$(printf '%s' "$LAUNCH_JSON" | jqf delivery.state)"
if [ "$LAUNCH_STATE" != "launched" ]; then
  echo "    （診断: reason=$(printf '%s' "$LAUNCH_JSON" | jqf delivery.reason)）"
fi
check_eq "master が居なければ launched" "launched" "$LAUNCH_STATE"
LTAB="$(printf '%s' "$LAUNCH_JSON" | jqf delivery.tab)"
LPANE="$(printf '%s' "$LAUNCH_JSON" | jqf delivery.pane)"
TABS_AFTER="$("$TAKO_BIN" list 2>/dev/null | python3 -c 'import json,sys; print(len(json.load(sys.stdin)["tabs"]))')"
check_eq "タブが 1 枚増える" "$((TABS_BEFORE + 1))" "$TABS_AFTER"
TAB_TITLE="$("$TAKO_BIN" list 2>/dev/null | python3 -c '
import json,sys
d=json.load(sys.stdin); t=[x for x in d["tabs"] if str(x["id"])==sys.argv[1]]
print(t[0]["title"] if t else "")' "$LTAB")"
check_eq "タブ名が CLI の tako master と同じ形" "master-t1450" "$TAB_TITLE"
PANE_ROLE="$("$TAKO_BIN" list 2>/dev/null | python3 -c '
import json,sys
d=json.load(sys.stdin)
for tab in d["tabs"]:
    for p in tab.get("panes", []):
        if str(p["id"])==sys.argv[1]: print(p.get("role") or ""); break' "$LPANE")"
check_eq "ペインの role も同じ形" "orchestrator-master:t1450" "$PANE_ROLE"

LSCREEN=""
for _ in $(seq 1 200); do
  LSCREEN="$("$TAKO_BIN" read --pane "$LPANE" --lines 80 2>/dev/null)"
  case "$(squash "$LSCREEN")" in
    *"$(squash "【ユーザー返答】todo $LAUNCH_ID")"*) break ;;
  esac
  sleep 0.2
done
check_screen_contains "起動コマンドが流れている" "$LSCREEN" "STUB_CLAUDE_STARTED"
check_screen_contains "初回メッセージに返答が載る" "$LSCREEN" "【ユーザー返答】todo $LAUNCH_ID"
check_screen_contains "初回メッセージにタスク本文も載る（起動直後は文脈が無い）" \
  "$LSCREEN" "これが初回メッセージに載るはず"

# --- ⑥ 配送できないときは理由つきで残る ---------------------------------------
echo
echo "=== ⑥ 配送できないとき（プロファイル不明） ==="
FAIL_ID="$(TAKO_ORCHESTRATOR_ROLE="master:存在しないプロファイル" "$TAKO_BIN" todo add \
  "届かない返答" --json 2>&1 | jqf id)"
FAIL_JSON="$("$TAKO_BIN" todo respond "$FAIL_ID" --decision approve --json 2>&1)"
check_eq "配送は failed で残る" "failed" "$(printf '%s' "$FAIL_JSON" | jqf delivery.state)"
FAIL_REASON="$(printf '%s' "$FAIL_JSON" | jqf delivery.reason)"
check_contains "理由が残る（無言にしない）" "$FAIL_REASON" "master"
check_eq "approve はタスクを閉じる（配送の可否とは別）" "done" "$(printf '%s' "$FAIL_JSON" | jqf status)"
check_contains "配送の顛末が persist.log に残る" \
  "$(cat "$TAKO_DATA_DIR/persist.log" 2>/dev/null || echo '')" "ユーザータスクの返答配送"

# --- 片付けと状態遷移 ---------------------------------------------------------
echo
echo "=== ⑦ 片付け（done / dismiss） ==="
check_eq "done で閉じる" "done" "$("$TAKO_BIN" todo done u-1 --json 2>&1 | jqf status)"
check_eq "dismiss で却下" "dismissed" "$("$TAKO_BIN" todo dismiss "$LAUNCH_ID" --json 2>&1 | jqf status)"
# 起票したのは 4 件（u-1 = ①/ u-2 = ③・needs_change で open のまま /
# u-3 = ⑤ dismiss / u-4 = ⑥ approve で done）。残る未完了は u-2 だけ
check_eq "needs_change の 1 件だけが未完了で残る" "1" \
  "$("$TAKO_BIN" todo list --json 2>&1 | jqf open_count)"
check_eq "残っているのは返答待ちの u-2" "u-2" "$("$TAKO_BIN" todo list --json 2>&1 | jqf tasks.0.id)"
check_eq "--all なら全件見える" "4" "$("$TAKO_BIN" todo list --all --json 2>&1 | jqf count)"
check_eq "最後の 1 件も片付けられる" "0" \
  "$("$TAKO_BIN" todo done u-2 --json >/dev/null 2>&1; "$TAKO_BIN" todo list --json 2>&1 | jqf open_count)"

echo
echo "================================"
echo "PASS=$PASS FAIL=$FAIL"
echo "================================"
[ "$FAIL" -eq 0 ]
