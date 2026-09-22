#!/bin/bash
# test-setup-shell-integration-1504.sh — シェル統合を setup の段として配置する（#1504）実経路テスト
#
# 何を確かめるか:
#   `tako setup` が**自分でシェル統合を配置する**か（#1504 前は `tako shell-integration
#   install` を人が打つまで OSC 7 / 133・cwd 追従・入力予測が効かなかった = 棚卸し Z9）。
#   聞かずに続けるか（`--yes` / 非 TTY / 端末ありで同じ結果）。2 回目が冪等か。
#   配置に失敗しても setup が完走して「残り」に載るか（#1501 の契約）。
#
#   **Windows 形（`$PROFILE` へブロックを置く経路）の表示・冪等・残りの判断**は
#   macOS 上で `Status` を組んで通す Rust 側（`shell_integration::stage_tests` と
#   番犬 `issue1504_setup_shell_integration_watchdog`）が担当するので、
#   このスクリプトの最後でまとめて走らせて件数を数える（実機は要求しない）。
#
# どう隔離するか（**本番の ~/.zshrc / ~/.zprofile / ~/Library/... へ 1 バイトも書かない**）:
#   - `HOME` を一時ディレクトリへ振る（`TAKO_DATA_DIR` では `~/.zshrc` を隔離できない）
#   - `TAKO_DATA_DIR` / `TAKO_ORCHESTRATOR_DIR` / `TAKO_TMUX_SOCKET` / `TAKO_SOCKET` も一時へ
#   - `env -i` で本番の env を持ち込まない。依存（claude / tmux / git / tailscale）は
#     すべてスタブで、`SHELL` は `-l` を剥ぐラッパなので解決は隔離 PATH に閉じる
#   - GUI は 1 枚も立てない（CLI 経路のみ）
#
# 使い方: bash scripts/test-setup-shell-integration-1504.sh
#         TAKO_1504_LEGACY=1 bash scripts/test-setup-shell-integration-1504.sh  # A/B（配置しない旧挙動）
#
# 配列は `${arr[@]+"${arr[@]}"}` で展開する（macOS 同梱の bash 3.2 は `set -u` の下で
# 空配列の `"${arr[@]}"` を未定義として落とす。`.agent/conventions.md`）

set -uo pipefail

# **本番 GUI を指す env を最初に落とす**。tako のペインの中から走らせると
# `TAKO_SOCKET` / `TAKO_TOKEN` / `TAKO_PANE_ID` が継承され、CLI が本番 GUI を触る
unset TAKO_SOCKET TAKO_TOKEN TAKO_PANE_ID

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
missing(){ if [ -e "$2" ]; then ng "$1 ($2 が在る)"; else ok "$1"; fi; }

[ -x "$TAKO" ] || { echo "tako が無い: ${TAKO}（cargo build -p tako-cli）"; exit 1; }
[ -f "$PTY" ]  || { echo "PTY ドライバが無い: $PTY"; exit 1; }
[ -x /usr/bin/python3 ] || { echo "/usr/bin/python3 が無い環境では走らない"; exit 1; }

SANDBOX="$(mktemp -d "${TMPDIR:-/tmp}/tako-1504-XXXXXX")" || exit 1
cleanup() {
    # **一時ディレクトリ配下であることを確かめてから消す**（実環境破壊の再発防止）
    case "$SANDBOX" in */tako-1504-*) rm -rf "$SANDBOX" ;; esac
}
trap cleanup EXIT

BIN="$SANDBOX/bin"
HOMEDIR="$SANDBOX/home"
DATADIR="$SANDBOX/d"
SI="$DATADIR/shell-integration"
# tako が `$PROFILE` へ置くブロックのマーカー（unix では**どのファイルにも**現れない）
MARKER='# >>> tako shell integration >>>'

# --- スタブと隔離 HOME -------------------------------------------------------
mkstubs(){
  rm -rf "$BIN" "$HOMEDIR" "$DATADIR"
  mkdir -p "$BIN" "$HOMEDIR/.local/bin" "$DATADIR"
  # exe::find のログインシェル経由の解決を隔離 PATH に閉じ込める
  printf '#!/bin/sh\nif [ "$1" = "-l" ]; then shift; fi\nexec /bin/sh "$@"\n' > "$BIN/isosh"
  # claude スタブ（導入済み・認証済み扱いなので bootstrap 段は素通りする）
  cat > "$BIN/claude" <<'EOF'
#!/bin/sh
case "$*" in
  "auth status --json") echo '{"loggedIn":true,"authMethod":"claude.ai","subscriptionType":"Max"}'; exit 0;;
  "--version") echo "2.1.258 (Claude Code)"; exit 0;;
  "mcp list"*) echo "tako: /stub/tako mcp serve - Connected"; exit 0;;
esac
exit 0
EOF
  # 任意依存は全部「在る」ことにする（#1499 の [y/N] をこのテストへ持ち込まない）
  for dep in tmux git tailscale; do
    printf '#!/bin/sh\necho "%s (stub)"\nexit 0\n' "$dep" > "$BIN/$dep"
  done
  chmod +x "$BIN"/*
  # ユーザーが自分で書いた rc（**1 バイトも変わらないこと**を後で確かめる）
  printf '# user own zshrc\nexport TAKO_1504_USER_ZSHRC=1\n' > "$HOMEDIR/.zshrc"
  printf '# user own zshenv\nexport TAKO_1504_USER_ZSHENV=1\n' > "$HOMEDIR/.zshenv"
}

# 隔離環境で `tako` を走らせる 2 通り（パイプ = GUI 初回起動・dispatch と同条件 / PTY = 端末あり）
iso_env(){
  echo "HOME=$HOMEDIR" "TAKO_DATA_DIR=$DATADIR" "TAKO_ORCHESTRATOR_DIR=$DATADIR/orch" \
       "TAKO_TMUX_SOCKET=$SANDBOX/tmux.sock" "TAKO_SOCKET=$SANDBOX/nowhere.sock" \
       "TAKO_DISCOVERY_DIR=$SANDBOX/nowhere" "TAKO_ISOLATED=1" "SHELL=$BIN/isosh" \
       "PATH=$BIN:/usr/bin:/bin:/usr/sbin:/sbin" "TERM=dumb" "LANG=ja_JP.UTF-8"
}
setup_pipe(){  # setup_pipe <出力先> -- <tako の引数...>
  local out="$1"; shift; shift
  local -a legacy=()
  [ -n "${TAKO_1504_LEGACY:-}" ] && legacy=("TAKO_1504_LEGACY=1")
  env -i $(iso_env) ${legacy[@]+"${legacy[@]}"} \
      "$TAKO" "$@" < /dev/null > "$out" 2>&1
  echo $?
}
setup_pty(){   # setup_pty <出力先> -- <tako の引数...>
  local out="$1"; shift; shift
  local -a legacy=()
  [ -n "${TAKO_1504_LEGACY:-}" ] && legacy=("TAKO_1504_LEGACY=1")
  env -i $(iso_env) ${legacy[@]+"${legacy[@]}"} \
      /usr/bin/python3 "$PTY" --stop-after "スマホからリモート接続するには" --timeout 90 \
      -- "$TAKO" "$@" > "$out" 2>&1
  echo $?
}

# 段の表示（シェル統合の行だけを抜く。字下げ込みで比較する）
stage_lines(){ grep -F "シェル統合" "$1" 2>/dev/null; }
# ディレクトリ配下の中身の指紋（相対パスで取るので置き場所が違っても比較できる）
tree_hash(){ ( cd "$1" 2>/dev/null && find . -type f | LC_ALL=C sort | xargs shasum 2>/dev/null ) | shasum | cut -d' ' -f1; }
# HOME 配下に `$PROFILE` 用のブロックが 1 つも書かれていないこと
home_has_marker(){ grep -rlF "$MARKER" "$HOMEDIR" 2>/dev/null | head -1; }

echo "シェル統合を setup の段として配置する（#1504）— sandbox: $SANDBOX"
[ -n "${TAKO_1504_LEGACY:-}" ] && echo "  (A/B) TAKO_1504_LEGACY=1: 配置しない旧挙動"
echo

# --- A. 非 TTY の標準 setup（GUI 初回起動・dispatch SetupRun と同条件）------
echo "== A) 非 TTY の標準 tako setup =="
mkstubs
ZSHRC_BEFORE=$(shasum "$HOMEDIR/.zshrc" | cut -d' ' -f1)
ZSHENV_BEFORE=$(shasum "$HOMEDIR/.zshenv" | cut -d' ' -f1)
RC=$(setup_pipe "$SANDBOX/a.log" -- setup)
check "終了コード 0" "$RC" "0"
if [ -n "${TAKO_1504_LEGACY:-}" ]; then
  contains "旧アームは何が無効かを名乗る" "[legacy] シェル統合の配置は TAKO_1504_LEGACY=1 で無効" "$SANDBOX/a.log"
  absent   "旧アームは配置しない"         "[OK] シェル統合" "$SANDBOX/a.log"
  missing  "旧アームは統合スクリプトを置かない" "$SI"
else
  contains "注入で効いていることを 1 行で言う" "[OK] シェル統合: 環境変数の注入で有効です" "$SANDBOX/a.log"
  contains "対象シェルを名乗る"               "zsh / bash / fish" "$SANDBOX/a.log"
  contains "ユーザーのファイルを触らないと言う" "設定ファイルは書き換えません" "$SANDBOX/a.log"
  exists   "統合スクリプト（bash）を置く"      "$SI/tako.bash"
  exists   "統合スクリプト（zsh）を置く"        "$SI/zsh/.zshenv"
  exists   "入力予測（#600）も置く"            "$SI/zsh-autosuggestions/zsh-autosuggestions.zsh"
  exists   "PowerShell 用も置く（#525）"       "$SI/tako.ps1"
  # Windows PowerShell 5.1 は BOM 無しの .ps1 を ANSI として読むので BOM が要る
  if [ "$(head -c 3 "$SI/tako.ps1" | od -An -tx1 | tr -d ' \n')" = "efbbbf" ]; then
    ok "PowerShell 用は BOM 付きで置かれる"
  else
    ng "PowerShell 用に BOM が無い"
  fi
fi
absent "段は何も聞かない（[y/N] を出さない）" "[y/N]" "$SANDBOX/a.log"
absent "残りにシェル統合は載らない"           "tako shell-integration install" "$SANDBOX/a.log"
check "ユーザーの .zshrc は不変"   "$(shasum "$HOMEDIR/.zshrc" | cut -d' ' -f1)"  "$ZSHRC_BEFORE"
check "ユーザーの .zshenv は不変"  "$(shasum "$HOMEDIR/.zshenv" | cut -d' ' -f1)" "$ZSHENV_BEFORE"
if [ -z "$(home_has_marker)" ]; then
  ok "unix では \$PROFILE のブロックをどこにも書かない"
else
  ng "HOME にブロックが書かれた: $(home_has_marker)"
fi

# --- B. --yes（明示の同意扱い）---------------------------------------------
echo "== B) --yes でも同じ結果 =="
mkstubs
RC=$(setup_pipe "$SANDBOX/b.log" -- setup --yes)
check "終了コード 0" "$RC" "0"
if [ -z "${TAKO_1504_LEGACY:-}" ]; then
  exists "--yes でも配置は走る" "$SI/tako.bash"
fi
if [ "$(stage_lines "$SANDBOX/b.log")" = "$(stage_lines "$SANDBOX/a.log")" ]; then
  ok "非 TTY と --yes で段の表示が一致する（分岐を持たない）"
else
  ng "段の表示が --yes で変わる"
fi
absent "--yes でも聞かない" "[y/N]" "$SANDBOX/b.log"

# --- C. 端末あり（PTY）でも聞かない -----------------------------------------
echo "== C) 端末あり（PTY）でも入力を待たない =="
mkstubs
RC=$(setup_pty "$SANDBOX/c.log" -- setup)
check "時間切れ（124）にならない" "$([ "$RC" = "124" ] && echo timeout || echo ok)" "ok"
absent "端末ありでも聞かない" "[y/N]" "$SANDBOX/c.log"
if [ "$(stage_lines "$SANDBOX/c.log" | tr -d '\r')" = "$(stage_lines "$SANDBOX/a.log")" ]; then
  ok "端末あり / 非 TTY で段の表示が一致する"
else
  ng "段の表示が端末の有無で変わる: $(stage_lines "$SANDBOX/c.log" | tr -d '\r')"
fi

# --- D. 冪等（2 回目は差分ゼロ）---------------------------------------------
echo "== D) 2 回目は冪等 =="
mkstubs
RC=$(setup_pipe "$SANDBOX/d1.log" -- setup)
check "1 回目の終了コード 0" "$RC" "0"
D1_HASH=$(tree_hash "$SI")
ZSHRC_BEFORE=$(shasum "$HOMEDIR/.zshrc" | cut -d' ' -f1)
RC=$(setup_pipe "$SANDBOX/d2.log" -- setup)
check "2 回目の終了コード 0" "$RC" "0"
if [ "$(stage_lines "$SANDBOX/d2.log")" = "$(stage_lines "$SANDBOX/d1.log")" ]; then
  ok "2 回目も段の表示が同一"
else
  ng "2 回目で段の表示が変わる"
fi
check "2 回目で配置物の中身が変わらない" "$(tree_hash "$SI")" "$D1_HASH"
check "2 回目もユーザーの rc は不変" "$(shasum "$HOMEDIR/.zshrc" | cut -d' ' -f1)" "$ZSHRC_BEFORE"
absent "2 回目も聞かない" "[y/N]" "$SANDBOX/d2.log"

# --- E. 配置に失敗しても完走する（#1501 の契約）------------------------------
# 置き場所をファイルにして `create_dir_all` を失敗させる（実機の Windows で
# 「PowerShell が見つからない」ときと同じ経路 = install() が Err を返す）
echo "== E) 配置に失敗しても setup は完走して「残り」に載る =="
mkstubs
printf 'not a directory\n' > "$SI"
RC=$(setup_pipe "$SANDBOX/e.log" -- setup)
check "配置に失敗しても終了コード 0" "$RC" "0"
if [ -n "${TAKO_1504_LEGACY:-}" ]; then
  absent "旧アームは失敗すらしない（触らないので）" "シェル統合の配置を見送りました" "$SANDBOX/e.log"
else
  contains "見送ったことを言う"       "[警告] シェル統合の配置を見送りました" "$SANDBOX/e.log"
  contains "何が効かなくなるかを言う" "ペインの cwd 追従・コマンド実行状態・入力予測は働きません" "$SANDBOX/e.log"
  contains "あとから配置できると言う" "あとから tako shell-integration install で配置できます" "$SANDBOX/e.log"
  contains "残り 1 件として末尾に載る" "残り 1 件（ここから先は人の操作が必要です）" "$SANDBOX/e.log"
  contains "残りの見出し"             "1. シェル統合の配置（ペインの cwd 追従とコマンド実行状態）" "$SANDBOX/e.log"
  contains "残りの次に打つ 1 行（最簡形）" "     tako shell-integration install" "$SANDBOX/e.log"
  contains "残りに理由も載る"         "統合スクリプトを書き出せません" "$SANDBOX/e.log"
  absent   "言い切らない"             "セットアップが完了しました。" "$SANDBOX/e.log"
fi
# **他の段は全部走り切っている**（#1501: 詰まった 1 件で残りを捨てない）
exists "profiles/default.yaml は作られる"   "$DATADIR/orch/profiles/default.yaml"
exists "テンプレートは展開される"           "$DATADIR/setup/setup-instructions.md"
exists "グローバル指示ファイルは作られる"   "$HOMEDIR/.claude/CLAUDE.md"
# 障害を除いて再実行すると残りから復帰する
rm -f "$SI"
RC=$(setup_pipe "$SANDBOX/e2.log" -- setup)
check "再実行の終了コード 0" "$RC" "0"
if [ -z "${TAKO_1504_LEGACY:-}" ]; then
  contains "再実行で配置へ復帰する" "[OK] シェル統合: 環境変数の注入で有効です" "$SANDBOX/e2.log"
  absent   "残りから消える"         "tako shell-integration install" "$SANDBOX/e2.log"
  exists   "統合スクリプトが置かれる" "$SI/tako.bash"
fi

# --- F. --check は読み取りだけ ----------------------------------------------
echo "== F) tako setup --check は読み取りだけ =="
mkstubs
RC=$(setup_pipe "$SANDBOX/f.log" -- setup --check)
check "終了コード 0" "$RC" "0"
if [ -n "${TAKO_1504_LEGACY:-}" ]; then
  contains "旧アームは --check でも名乗る" "[legacy] シェル統合の配置は TAKO_1504_LEGACY=1 で無効" "$SANDBOX/f.log"
else
  contains "--check が状況を 1 行で出す" "[OK] シェル統合: 環境変数の注入で有効（zsh / bash / fish）" "$SANDBOX/f.log"
fi
missing "--check は 1 バイトも書かない" "$SI"
absent  "--check は何も聞かない" "[y/N]" "$SANDBOX/f.log"

# --- G. A/B の観測（既定アームでのみ）---------------------------------------
if [ -z "${TAKO_1504_LEGACY:-}" ]; then
  echo "== G) A/B: TAKO_1504_LEGACY=1 は #1504 前（setup が配置に触らない）=="
  mkstubs
  RC=$(env -i $(iso_env) TAKO_1504_LEGACY=1 "$TAKO" setup < /dev/null > "$SANDBOX/g.log" 2>&1; echo $?)
  check "旧アームでも終了コード 0" "$RC" "0"
  contains "旧アームは無効を名乗る" "[legacy] シェル統合の配置は TAKO_1504_LEGACY=1 で無効" "$SANDBOX/g.log"
  absent   "旧アームは配置しない"   "[OK] シェル統合" "$SANDBOX/g.log"
  missing  "旧アームは統合スクリプトを置かない（#1504 の症状）" "$SI"
fi

# --- H. Windows 形の分岐と番犬（Rust 側）------------------------------------
# 実機を持たない環境で Profile 経路（`$PROFILE` へブロックを置く）の表示・冪等・
# 残りを通すのはここ。`Status` を組んで純粋関数へ渡すので macOS でも全分岐を通る
echo "== H) Windows 形の分岐と配線の番犬（Rust） =="
for t in "-p tako-control --lib shell_integration::" \
         "-p tako-control --test issue1504_setup_shell_integration_watchdog" \
         "-p tako-control --lib setup_remaining::"; do
  # shellcheck disable=SC2086
  if ( cd "$ROOT" && cargo test $t -- --quiet ) > "$SANDBOX/h.log" 2>&1; then
    ok "cargo test ${t}（$(grep -oE '[0-9]+ passed' "$SANDBOX/h.log" | head -1)）"
  else
    ng "cargo test ${t} が落ちた: $(tail -3 "$SANDBOX/h.log" | tr '\n' ' ')"
  fi
done

echo
echo "===================================="
echo "  PASS=$PASS  FAIL=$FAIL"
echo "===================================="
[ "$FAIL" -eq 0 ]
