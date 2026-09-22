#!/usr/bin/env bash
# #1499 の実経路テスト: `tako setup` の依存チェック段で未検出の CLI 依存に
# 「インストールしますか？ [y/N]」が出て、y で**同じ実行の中で**導入 → 再検出まで
# 通ることを、**隔離した HOME / TAKO_DATA_DIR / PATH** で実測する。
#
# 本番の ~/Library/Application Support/tako・~/.claude・実機の tmux には一切触れない
# （`env -i` で環境ごと差し替え、brew と claude はスタブ、実機の tmux は隠すだけ）。
#
# 未検出を作るには 2 本塞ぐ（#1499 の実測）:
#   - `exe::find`（境界 B16）は unix で `$SHELL -l -c "command -v X"`。`-l` が
#     /etc/profile を読んで実機の PATH へ戻すので、SHELL を `-l` 剥ぎのラッパへ
#   - `resolve()` の器フォールバック `backend::binary()` は /opt/homebrew/bin/tmux 等の
#     既知の置き場を直接見るので、TAKO_TMUX_BIN を未作成のパスへ向ける
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
# 前提: git は隔離 PATH（/usr/bin）から引ける。無いと未検出の依存が 3 つになり、
# 依存の並び（器 / git / tailscale）に対する答えの並びがずれる
[ -x /usr/bin/git ] || { echo "/usr/bin/git が無い環境では走らない"; exit 1; }

SANDBOX="$(mktemp -d "${TMPDIR:-/tmp}/tako-1499-XXXXXX")"
trap 'rm -rf "$SANDBOX"' EXIT
BIN="$SANDBOX/bin"

# --- スタブを置く -----------------------------------------------------------
mkstubs(){                       # $1 = brew の振る舞い（ok / fail / noop / none）
  rm -rf "$BIN" "$SANDBOX/home" "$SANDBOX/d"
  mkdir -p "$BIN" "$SANDBOX/home" "$SANDBOX/d"
  # exe::find のログインシェル経由の解決を隔離 PATH に閉じ込める
  printf '#!/bin/sh\nif [ "$1" = "-l" ]; then shift; fi\nexec /bin/sh "$@"\n' > "$BIN/isosh"
  # claude スタブ（bootstrap 段を素通りさせる = 導入済み・認証済み扱い）
  cat > "$BIN/claude" <<'EOF'
#!/bin/sh
case "$*" in
  "auth status --json") echo '{"loggedIn":true,"authMethod":"claude.ai","subscriptionType":"Max"}'; exit 0;;
  "--version") echo "2.1.252 (Claude Code)"; exit 0;;
esac
exit 0
EOF
  case "$1" in
    ok)   cat > "$BIN/brew" <<'EOF'
#!/bin/sh
[ "$1" = "install" ] || exit 0
echo "==> Fetching $2"
printf '#!/bin/sh\necho "%s 3.5a (stub)"\n' "$2" > "$(dirname "$0")/$2"
chmod +x "$(dirname "$0")/$2"
EOF
          ;;
    fail) printf '#!/bin/sh\necho "Error: No available formula" >&2\nexit 1\n' > "$BIN/brew" ;;
    noop) printf '#!/bin/sh\necho "==> Pouring $2"\nexit 0\n' > "$BIN/brew" ;;  # 成功と言うが何も置かない
    none) : ;;                                                                  # brew 自体が無い
  esac
  chmod +x "$BIN"/* 2>/dev/null
}

# 隔離環境で `tako` を走らせる 2 通り。`LEGACY=1` で #1499 前の A/B アーム、
# `STOP_AFTER=<文字列>` で「観たいものが出たら打ち切る」（`--review` は依存の段より
# 後ろでも聞くので、これが無いと PTY の時間切れに頼る形になる）
setup_pty(){   # setup_pty <出力先> <answers...> -- <tako の引数...>
  local out="$1"; shift
  local answers=() ; while [ "${1:-}" != "--" ]; do answers+=(--answer "$1"); shift; done; shift
  env -i HOME="$SANDBOX/home" TAKO_DATA_DIR="$SANDBOX/d" TAKO_ISOLATED=1 \
      SHELL="$BIN/isosh" TAKO_TMUX_BIN="$BIN/tmux" ${LEGACY:+TAKO_1499_LEGACY=1} \
      PATH="$BIN:/usr/bin:/bin:/usr/sbin:/sbin" TERM=dumb LANG=ja_JP.UTF-8 \
      python3 "$PTY" "${answers[@]}" ${STOP_AFTER:+--stop-after "$STOP_AFTER"} \
      --timeout 60 -- "$TAKO" "$@" > "$out" 2>&1
  echo $?
}
setup_pipe(){  # setup_pipe <出力先> -- <tako の引数...>
  local out="$1"; shift; shift
  env -i HOME="$SANDBOX/home" TAKO_DATA_DIR="$SANDBOX/d" TAKO_ISOLATED=1 \
      SHELL="$BIN/isosh" TAKO_TMUX_BIN="$BIN/tmux" ${LEGACY:+TAKO_1499_LEGACY=1} \
      PATH="$BIN:/usr/bin:/bin:/usr/sbin:/sbin" TERM=dumb LANG=ja_JP.UTF-8 \
      "$TAKO" "$@" < /dev/null > "$out" 2>&1
  echo $?
}

echo "== 1) 端末ありの標準 tako setup: tmux は y で導入 / tailscale は N でスキップ =="
mkstubs ok
RC=$(setup_pty "$SANDBOX/1.log" y N -- setup)
contains "導入前に「何を・どの導入器で・どこへ」を見せる" "導入: brew install tmux（導入器" "$SANDBOX/1.log"
contains "tmux に [y/N] が出る"                  "tmux をインストールしますか？ [y/N]:" "$SANDBOX/1.log"
contains "y で導入が走る"                        "==> Fetching tmux" "$SANDBOX/1.log"
contains "同じ実行の中で見つかるようになる"      "[OK] tmux: $BIN/tmux（インストール完了）" "$SANDBOX/1.log"
contains "複数未検出でも 1 件ずつ聞く"           "tailscale をインストールしますか？ [y/N]:" "$SANDBOX/1.log"
contains "N は案内だけ出して続行する"            "スキップしました（後から \`tako setup deps install\` で導入できます）" "$SANDBOX/1.log"
absent   "N のものは導入しない"                  "==> Fetching tailscale" "$SANDBOX/1.log"
contains "setup は最後まで通る"                  "セットアップが完了しました" "$SANDBOX/1.log"
if [ -x "$BIN/tmux" ]; then ok "tmux の実体が置かれた"; else ng "tmux の実体が無い"; fi
if [ -e "$BIN/tailscale" ]; then ng "N なのに tailscale が置かれた"; else ok "N のものは置かれない"; fi
check "終了コード 0" "$RC" "0"

echo "== 2) 2 回目は聞かない（冪等）=="
RC=$(setup_pty "$SANDBOX/2.log" -- setup)
absent "導入済みの tmux は聞かれない" "tmux をインストールしますか？" "$SANDBOX/2.log"
contains "導入済みは [OK] で出る"     "[OK] tmux: $BIN/tmux" "$SANDBOX/2.log"
check "終了コード 0" "$RC" "0"

echo "== 3) --yes は同意扱いで導入し、質問しない =="
mkstubs ok
RC=$(setup_pty "$SANDBOX/3.log" -- setup --yes)
absent   "質問は出ない"                 "しますか？ [y/N]:" "$SANDBOX/3.log"
contains "省略の理由を言う"             "--yes のため確認を省略してインストールします" "$SANDBOX/3.log"
contains "実際に導入される"             "[OK] tmux: $BIN/tmux（インストール完了）" "$SANDBOX/3.log"
contains "setup は最後まで通る"         "セットアップが完了しました" "$SANDBOX/3.log"
check "終了コード 0（止まらない）" "$RC" "0"

echo "== 4) 非 TTY（stdin がパイプ）は聞かずに素通りする =="
mkstubs ok
RC=$(setup_pipe "$SANDBOX/4.log" -- setup)
absent   "質問は出ない"           "しますか？ [y/N]:" "$SANDBOX/4.log"
contains "最簡形の案内へ落ちる"   "いま入れる: tako setup deps install" "$SANDBOX/4.log"
contains "理由を言う"             "（端末が無いので確認を省きました）" "$SANDBOX/4.log"
absent   "勝手に導入しない"       "==> Fetching tmux" "$SANDBOX/4.log"
contains "setup は最後まで通る"   "セットアップが完了しました" "$SANDBOX/4.log"
check "終了コード 0（止まらない）" "$RC" "0"

echo "== 5) 非 TTY + --yes は導入する（GUI 初回起動の経路）=="
mkstubs ok
RC=$(setup_pipe "$SANDBOX/5.log" -- setup --yes)
contains "導入される" "[OK] tmux: $BIN/tmux（インストール完了）" "$SANDBOX/5.log"
check "終了コード 0" "$RC" "0"

echo "== 6) 導入コマンドが失敗しても案内へ落ちて setup は続く =="
mkstubs fail
RC=$(setup_pty "$SANDBOX/6.log" y y -- setup)
contains "失敗を警告として出す"   "[警告] brew install tmux が失敗しました（exit 1）" "$SANDBOX/6.log"
contains "次の一手へ落とす"       "いま入れる: tako setup deps install" "$SANDBOX/6.log"
contains "setup は最後まで通る"   "セットアップが完了しました" "$SANDBOX/6.log"
check "終了コード 0" "$RC" "0"

echo "== 7) 「入れたのに見つからない」も止めずに案内へ落とす =="
mkstubs noop
RC=$(setup_pty "$SANDBOX/7.log" y y -- setup)
contains "見つからないことを言う" "が見つかりません" "$SANDBOX/7.log"
contains "次の一手へ落とす"       "いま入れる: tako setup deps install" "$SANDBOX/7.log"
contains "setup は最後まで通る"   "セットアップが完了しました" "$SANDBOX/7.log"
check "終了コード 0" "$RC" "0"

echo "== 8) brew が無い環境では聞かず、打つべきコマンドを見せる =="
mkstubs none
RC=$(setup_pty "$SANDBOX/8.log" -- setup)
absent   "質問は出ない"       "しますか？ [y/N]:" "$SANDBOX/8.log"
contains "人が打つ手を見せる" "導入方法: brew install tmux（要 Homebrew: https://brew.sh）" "$SANDBOX/8.log"
contains "setup は最後まで通る" "セットアップが完了しました" "$SANDBOX/8.log"
check "終了コード 0" "$RC" "0"

echo "== 9) --check は読み取りだけ（何も聞かない・何も入れない）=="
mkstubs ok
RC=$(setup_pty "$SANDBOX/9.log" y y -- setup --check)
absent "質問は出ない"   "しますか？ [y/N]:" "$SANDBOX/9.log"
absent "導入は走らない" "==> Fetching tmux" "$SANDBOX/9.log"
if [ -e "$BIN/tmux" ]; then ng "--check が tmux を入れた"; else ok "--check は何も入れない"; fi

echo "== 10) --review は従来どおり聞ける（#1057 の経路に回帰なし）=="
mkstubs ok
RC=$(STOP_AFTER="スリープ防止:" setup_pty "$SANDBOX/10.log" y N -- setup --review)
contains "[y/N] が出る" "tmux をインストールしますか？ [y/N]:" "$SANDBOX/10.log"
contains "y で導入される" "[OK] tmux: $BIN/tmux（インストール完了）" "$SANDBOX/10.log"

echo "== 11) A/B: TAKO_1499_LEGACY=1 は #1499 前（端末でも聞かない）へ戻る =="
mkstubs ok
RC=$(LEGACY=1 setup_pty "$SANDBOX/11.log" y y -- setup)
absent   "端末でも質問が出ない（= Issue の症状）" "しますか？ [y/N]:" "$SANDBOX/11.log"
contains "案内 1 行だけになる" "いま入れる: tako setup deps install" "$SANDBOX/11.log"
contains "理由を言う"         "（TAKO_1499_LEGACY=1 のため確認を省きました）" "$SANDBOX/11.log"
if [ -e "$BIN/tmux" ]; then ng "legacy なのに導入された"; else ok "legacy では導入されない"; fi
RC=$(LEGACY=1 STOP_AFTER="スリープ防止:" setup_pty "$SANDBOX/11b.log" y N -- setup --review)
contains "legacy でも --review は聞く（#1057 の契約を壊さない）" "tmux をインストールしますか？ [y/N]:" "$SANDBOX/11b.log"

echo "== 12) tako setup deps install も同じ導入関数を通る =="
mkstubs ok
RC=$(setup_pipe "$SANDBOX/12.log" -- setup deps install --dep tmux)
# 非対話の `deps install` は導入器の出力を握る（進捗は流さない）ので、結果行で確かめる
contains "明示コマンドでも入る" "[OK] tmux: $BIN/tmux（インストール完了）" "$SANDBOX/12.log"
if [ -x "$BIN/tmux" ]; then ok "実体が置かれた"; else ng "実体が無い"; fi
check "終了コード 0" "$RC" "0"

echo
echo "PASS=$PASS FAIL=$FAIL"
[ "$FAIL" -eq 0 ]
