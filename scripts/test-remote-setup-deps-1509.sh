#!/usr/bin/env bash
# #1509 の実経路テスト: `tako remote setup` の [1/5] が、未導入の Tailscale を
# **`tako setup` と同じ 1 実装**（`setup_deps::offer_for` / `install` / `resolve`）で
# 入れることを、隔離した HOME / TAKO_DATA_DIR / PATH で実測する。
#
# 本番の ~/Library/Application Support/tako・実機の Tailscale / Homebrew・
# tailnet の serve 設定には一切触れない:
#   - `env -i` で環境ごと差し替え、brew と tailscale はスタブ
#   - `TAKO_TAILSCALE_BIN` を隔離 PATH の中へ向けるので実機の Tailscale は見えない
#     （未導入の偽装 = そのパスに実体が無い状態）
#   - スタブの tailscale は `status --json` に **NeedsLogin** を返すので、
#     ウィザードは [2/5]「未ログイン」で止まる（serve には進まない）
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

SANDBOX="$(mktemp -d "${TMPDIR:-/tmp}/tako-1509-XXXXXX")"
trap 'rm -rf "$SANDBOX"' EXIT
BIN="$SANDBOX/bin"

# --- スタブを置く -----------------------------------------------------------
# $1 = brew の振る舞い（ok / fail / noop / none）
# $2 = tailscale を最初から置くか（installed なら置く）
mkstubs(){
  rm -rf "$BIN" "$SANDBOX/home" "$SANDBOX/d"
  mkdir -p "$BIN" "$SANDBOX/home" "$SANDBOX/d"
  # exe::find は `$SHELL -l -c "command -v X"`。`-l` が /etc/profile を読んで
  # 実機の PATH へ戻すので、`-l` を剥ぐラッパへ差し替えて隔離 PATH に閉じ込める
  printf '#!/bin/sh\nif [ "$1" = "-l" ]; then shift; fi\nexec /bin/sh "$@"\n' > "$BIN/isosh"
  case "$1" in
    ok)   cat > "$BIN/brew" <<'EOF'
#!/bin/sh
[ "$1" = "install" ] || exit 0
echo "==> Fetching $2"
dir="$(dirname "$0")"
if [ "$2" = "tailscale" ]; then
  cat > "$dir/tailscale" <<'TS'
#!/bin/sh
for a in "$@"; do
  if [ "$a" = "--version" ]; then echo "1.80.0"; exit 0; fi
done
echo '{"BackendState":"NeedsLogin"}'
exit 0
TS
else
  printf '#!/bin/sh\necho "%s (stub)"\n' "$2" > "$dir/$2"
fi
chmod +x "$dir/$2"
EOF
          ;;
    fail) printf '#!/bin/sh\necho "Error: No available formula" >&2\nexit 1\n' > "$BIN/brew" ;;
    noop) printf '#!/bin/sh\necho "==> Pouring $2"\nexit 0\n' > "$BIN/brew" ;;  # 成功と言うが何も置かない
    none) : ;;                                                                  # brew 自体が無い
  esac
  if [ "${2:-}" = "installed" ]; then
    cat > "$BIN/tailscale" <<'TS'
#!/bin/sh
for a in "$@"; do
  if [ "$a" = "--version" ]; then echo "1.80.0"; exit 0; fi
done
echo '{"BackendState":"NeedsLogin"}'
exit 0
TS
  fi
  chmod +x "$BIN"/* 2>/dev/null
}

# 隔離環境で `tako` を走らせる 2 通り（端末あり / パイプ）
setup_pty(){   # setup_pty <出力先> <answers...> -- <tako の引数...>
  local out="$1"; shift
  # 答えを 1 つも渡さない呼び出しがあるので**空配列の展開**を守る形で書く
  # （macOS 同梱の bash 3.2 は `set -u` 下で空配列の `"${arr[@]}"` を未定義扱いにする）
  local answers=() ; while [ "${1:-}" != "--" ]; do answers+=(--answer "$1"); shift; done; shift
  env -i HOME="$SANDBOX/home" TAKO_DATA_DIR="$SANDBOX/d" TAKO_ISOLATED=1 \
      SHELL="$BIN/isosh" TAKO_TAILSCALE_BIN="$BIN/tailscale" TAKO_TAILSCALE_SOCKET= \
      TAKO_SOCKET="$SANDBOX/none.sock" TAKO_TMUX_SOCKET="$SANDBOX/t.sock" \
      PATH="$BIN:/usr/bin:/bin:/usr/sbin:/sbin" TERM=dumb LANG=ja_JP.UTF-8 \
      python3 "$PTY" ${answers[@]+"${answers[@]}"} --timeout 60 -- "$TAKO" "$@" > "$out" 2>&1
  echo $?
}
setup_pipe(){  # setup_pipe <出力先> -- <tako の引数...>
  local out="$1"; shift; shift
  env -i HOME="$SANDBOX/home" TAKO_DATA_DIR="$SANDBOX/d" TAKO_ISOLATED=1 \
      SHELL="$BIN/isosh" TAKO_TAILSCALE_BIN="$BIN/tailscale" TAKO_TAILSCALE_SOCKET= \
      TAKO_SOCKET="$SANDBOX/none.sock" TAKO_TMUX_SOCKET="$SANDBOX/t.sock" \
      PATH="$BIN:/usr/bin:/bin:/usr/sbin:/sbin" TERM=dumb LANG=ja_JP.UTF-8 \
      "$TAKO" "$@" < /dev/null > "$out" 2>&1
  echo $?
}

echo "== 1) 端末あり・brew あり: 導入先を見せてから [y/N]、y で導入 → 同じ実行の中で再検出 =="
mkstubs ok
RC=$(setup_pty "$SANDBOX/1.log" y -- remote setup)
contains "未導入を言う"                    "[1/5] Tailscale を検出中... 未導入" "$SANDBOX/1.log"
contains "必要だと言う"                    "Tailscale が必要です。" "$SANDBOX/1.log"
absent   "代行できるときは入れ方を並べない" "App Store で「Tailscale」を検索" "$SANDBOX/1.log"
contains "導入前に「何を・どの導入器で・どこへ」を見せる" "導入: brew install tailscale（導入器 $BIN/brew → $BIN へ入ります）" "$SANDBOX/1.log"
contains "標準 setup と同じ [y/N] が出る"  "tailscale をインストールしますか？ [y/N]:" "$SANDBOX/1.log"
contains "y で導入が走る"                  "==> Fetching tailscale" "$SANDBOX/1.log"
contains "同じ実行の中で引けるようになる"  "導入しました: $BIN/tailscale" "$SANDBOX/1.log"
contains "ウィザードは次の段へ進む"        "[2/5] ログイン状態を確認中..." "$SANDBOX/1.log"
absent   "ネットワーク設定までは進まない"  "[4/5] serve を設定中" "$SANDBOX/1.log"
if [ -x "$BIN/tailscale" ]; then ok "tailscale の実体が置かれた"; else ng "tailscale の実体が無い"; fi
check "未ログインで止まるので終了コード 1" "$RC" "1"

echo "== 2) N はスキップして案内で終わる（導入しない）=="
mkstubs ok
RC=$(setup_pty "$SANDBOX/2.log" N -- remote setup)
contains "スキップを言う"       "スキップしました（後から \`tako setup deps install\` で導入できます）" "$SANDBOX/2.log"
contains "次の一手を出す"       "インストール後に再度 \`tako remote setup\` を実行してください。" "$SANDBOX/2.log"
absent   "導入は走らない"       "==> Fetching tailscale" "$SANDBOX/2.log"
if [ -e "$BIN/tailscale" ]; then ng "N なのに tailscale が置かれた"; else ok "N のものは置かれない"; fi
check "終了コード 1" "$RC" "1"

echo "== 3) --yes は同意扱いで導入し、質問しない =="
mkstubs ok
RC=$(setup_pty "$SANDBOX/3.log" -- remote setup --yes)
absent   "質問は出ない"       "しますか？ [y/N]:" "$SANDBOX/3.log"
contains "省略の理由を言う"   "--yes のため確認を省略してインストールします" "$SANDBOX/3.log"
contains "実際に導入される"   "導入しました: $BIN/tailscale" "$SANDBOX/3.log"
if [ -x "$BIN/tailscale" ]; then ok "実体が置かれた"; else ng "実体が無い"; fi

echo "== 4) 非 TTY（パイプ）は聞かずに案内へ落ちて止まらない =="
mkstubs ok
RC=$(setup_pipe "$SANDBOX/4.log" -- remote setup)
absent   "質問は出ない"           "しますか？ [y/N]:" "$SANDBOX/4.log"
contains "最簡形の案内へ落ちる"   "いま入れる: tako setup deps install   （brew install tailscale 相当）" "$SANDBOX/4.log"
contains "理由を言う"             "（端末が無いので確認を省きました）" "$SANDBOX/4.log"
absent   "勝手に導入しない"       "==> Fetching tailscale" "$SANDBOX/4.log"
contains "次の一手を出す"         "インストール後に再度 \`tako remote setup\` を実行してください。" "$SANDBOX/4.log"
check "入力待ちで固まらず終わる" "$RC" "1"

echo "== 5) 非 TTY + --yes は導入する（自動化の経路）=="
mkstubs ok
RC=$(setup_pipe "$SANDBOX/5.log" -- remote setup --yes)
contains "導入される" "導入しました: $BIN/tailscale" "$SANDBOX/5.log"
if [ -x "$BIN/tailscale" ]; then ok "実体が置かれた"; else ng "実体が無い"; fi

echo "== 6) brew が無い環境では聞かず、打つべきコマンドを見せる =="
mkstubs none
RC=$(setup_pty "$SANDBOX/6.log" -- remote setup)
absent   "質問は出ない"       "しますか？ [y/N]:" "$SANDBOX/6.log"
contains "人が打つ手を見せる" "導入方法: brew install tailscale（要 Homebrew: https://brew.sh）" "$SANDBOX/6.log"
contains "他の入れ方も残る"   "App Store で「Tailscale」を検索" "$SANDBOX/6.log"
contains "次の一手を出す"     "インストール後に再度 \`tako remote setup\` を実行してください。" "$SANDBOX/6.log"
check "終了コード 1" "$RC" "1"

echo "== 7) 導入コマンドが失敗しても案内へ落とす（黙って中断しない）=="
mkstubs fail
RC=$(setup_pty "$SANDBOX/7.log" y -- remote setup)
contains "失敗を警告として出す" "[警告] brew install tailscale が失敗しました（exit 1）" "$SANDBOX/7.log"
contains "次の一手へ落とす"     "いま入れる: tako setup deps install   （brew install tailscale 相当）" "$SANDBOX/7.log"
contains "再実行を促す"         "インストール後に再度 \`tako remote setup\` を実行してください。" "$SANDBOX/7.log"
check "終了コード 1" "$RC" "1"

echo "== 8) 「入れたのに見つからない」も理由つきで案内へ落とす =="
mkstubs noop
RC=$(setup_pty "$SANDBOX/8.log" y -- remote setup)
contains "見つからないことを言う" "が見つかりません" "$SANDBOX/8.log"
contains "次の一手へ落とす"       "いま入れる: tako setup deps install   （brew install tailscale 相当）" "$SANDBOX/8.log"
check "終了コード 1" "$RC" "1"

echo "== 9) 導入済みの環境では出力が変わらない（回帰なし）=="
mkstubs ok installed
RC=$(setup_pty "$SANDBOX/9.log" -- remote setup)
contains "検出できたら OK を出すだけ" "[1/5] Tailscale を検出中... OK ($BIN/tailscale)" "$SANDBOX/9.log"
absent   "導入の案内は出ない"         "導入: brew install tailscale" "$SANDBOX/9.log"
absent   "質問は出ない"               "しますか？ [y/N]:" "$SANDBOX/9.log"
absent   "導入は走らない"             "==> Fetching tailscale" "$SANDBOX/9.log"
contains "ウィザードは次の段へ進む"   "[2/5] ログイン状態を確認中..." "$SANDBOX/9.log"
check "未ログインで止まるので終了コード 1" "$RC" "1"

echo "== 10) MCP / 非対話（--answers）は従来どおり導入せず不足を返す =="
mkstubs ok
RC=$(setup_pipe "$SANDBOX/10.log" -- remote setup --answers '{}')
contains "不足として報告する" '"status": "missing"' "$SANDBOX/10.log"
absent   "導入は走らない"     "==> Fetching tailscale" "$SANDBOX/10.log"
if [ -e "$BIN/tailscale" ]; then ng "非対話経路が tailscale を入れた"; else ok "非対話経路は何も入れない"; fi

echo
echo "PASS=$PASS FAIL=$FAIL"
[ "$FAIL" -eq 0 ]
