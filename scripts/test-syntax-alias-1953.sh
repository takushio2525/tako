#!/bin/bash
# test-syntax-alias-1953.sh — 既存の構文への別名で色の穴を埋めた（#1953）を、CLI / MCP の応答と
# visual-test で新旧比べて確かめる
#
# 何を確かめるか:
#   ① CLI（`tako open`）と MCP（`tako mcp serve` の `tako_open_file`）の応答の `syntax` が
#      `.h` = C++・`.m` = MATLAB（MATLAB の本文）/ Objective-C（Objective-C の本文）・
#      `.pde` = Java・`.jsonc` = JSON・`.json5` = JavaScript。`.S` / `.s` は寄せない（Plain Text のまま）
#   ② A/B: `TAKO_1953_LEGACY=1`（#1953 以前）では同じ応答が `.h` = Objective-C・`.m` = 常に
#      Objective-C・`.pde` / `.jsonc` / `.json5` = Plain Text に戻る（= ① の変化が応答で読める）
#   ③ visual-test 節 syntax-alias が緑（応答の syntax・複数の構文色・JSON5 の字がダークで消えない・
#      手掛かりの無い `.m` を編集で `%` の行つきにすると MATLAB へ切り替わる）。
#      旧では ① の syntax で落ちる。画像は 置き場/new と 置き場/legacy へ分けて落とす
#
# 使い方: bash scripts/test-syntax-alias-1953.sh [画像の置き場]
#   置き場を省くと一時 dir に落として終了時に消す。
#
# 窓は仮想ディスプレイ tako-vd へ出す（`scripts/lib/isolated-gui.sh` の 1 実装。#1141 / #1490）。
# 面を用意できなければ起動せずに 4 で返る（#1744。報告には「未実測」と書く）。
# 落とすのは自分で起こした pid と、自分が名前を決めた tmux ソケットだけ。
set -uo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PASS=0
FAIL=0
pass() { PASS=$((PASS + 1)); echo "  [OK] $1"; }
fail() { FAIL=$((FAIL + 1)); echo "  [NG] $1"; }

. "$REPO_ROOT/scripts/lib/isolated-gui.sh"

# visual-test 版の tako-app が要る（素のビルドだと「未知の節」で exit 1）。
# バイナリを渡された A/B ではビルドしない。HOME を差し替える前に済ませる（ツールチェーンを取り直さない）
if [ -z "${APP_BIN:-}" ]; then
  echo "visual-test 版の tako / tako-app をビルドします…"
  (cd "$REPO_ROOT" && cargo build -p tako-cli -p tako-app --features tako-app/visual-test --quiet) || exit 1
fi
isolated_gui_bins || exit 1

# ペインの中から走らせても本番へ届かないように、継承した接続情報を落とす
unset TAKO_SOCKET TAKO_TOKEN TAKO_PANE_ID TAKO_TAB_ID TAKO_MCP_URL TAKO_ORCHESTRATOR_ROLE

# UNIX ソケットのパス長の上限に当たらないよう短い一時 dir にする
TMP="$(mktemp -d /tmp/tk1953-XXXXXX)"
DUMP_ROOT="${1:-$TMP/dump}"
TMUX_SOCK="tako-iso-1953-$$"
cleanup() {
  stop_isolated_gui "${ISOLATED_GUI_PID:-}"
  # 明示した tmux ソケットは tako の終了時に片付けられない（#770）ので自分で落とす
  tmux -L "$TMUX_SOCK" kill-server >/dev/null 2>&1 || true
  rm -rf "$TMP"
}
trap cleanup EXIT

export HOME="$TMP/home"
export TAKO_ISOLATED=1
export TAKO_DATA_DIR="$TMP/data"
export TAKO_DISCOVERY_DIR="$TMP/disc"
export TAKO_ORCHESTRATOR_DIR="$TMP/orch"
export TAKO_SESSIONS_FILE="$TMP/sessions.yaml"
export TAKO_PANE_LOG_DIR="$TMP/panelogs"
export TAKO_WORKERS_FILE="$TMP/workers.yaml"
export TAKO_PERSIST=0
export TAKO_TMUX_SOCKET="$TMUX_SOCK"
mkdir -p "$HOME" "$TAKO_DATA_DIR" "$TAKO_DISCOVERY_DIR" "$TAKO_ORCHESTRATOR_DIR"
# 本番へ書かない不変条件（env が効いていなければここで落とす）
for d in "$HOME" "$TAKO_DATA_DIR" "$TAKO_DISCOVERY_DIR" "$TAKO_ORCHESTRATOR_DIR"; do
  case "$d" in "$TMP"/*) : ;; *) echo "隔離されていない: $d"; exit 1 ;; esac
done

# 検査する本文（単体・visual-test と同じ手掛かりを入れた最小形）
FX="$TMP/fx"
mkdir -p "$FX"
printf '#ifndef GEO_H\n#define GEO_H\nint add(int a, int b);\n#endif\n' > "$FX/geo.h"
printf '#import <Foundation/Foundation.h>\n@interface Counter : NSObject\n@end\n' > "$FX/Counter.h"
printf '%% 二次方程式を解く\nfunction x = solve(a, b, c)\n    x = -b / (2*a);\nend\n' > "$FX/solve.m"
printf '#import "Counter.h"\n@implementation Counter\n@end\n' > "$FX/Counter.m"
printf 'x = linspace(0, 1, 10);\nplot(x, x.^2)\n' > "$FX/plot.m"
printf 'void setup() {\n  size(400, 400);\n}\n' > "$FX/sketch.pde"
printf '{\n  // コメント\n  "a": 1,\n}\n' > "$FX/settings.jsonc"
printf "{\n  // コメント\n  key: 'value',\n  n: 0xFF,\n}\n" > "$FX/renovate.json5"
printf '    .text\n_start:\n    mov r0, #1  @ ARM\n' > "$FX/boot.S"

# 形式 → 新の syntax / 旧の syntax / 新の file_type（`-` = null）
EXPECT="geo.h C++ Objective-C cpp
Counter.h Objective-C Objective-C cpp
solve.m MATLAB Objective-C -
Counter.m Objective-C Objective-C -
plot.m Objective-C Objective-C -
sketch.pde Java Plain_Text processing
settings.jsonc JSON Plain_Text jsonc
renovate.json5 JavaScript Plain_Text json5
boot.S Plain_Text Plain_Text -"

# CLI と MCP で全形式を開き、`file 経路 file_type syntax` を 1 行ずつ出す
cat > "$TMP/drive.py" <<'PY'
import json, os, subprocess, sys

tako, fx = sys.argv[1], sys.argv[2]
files = sys.argv[3:]

# 呼び出し元 = 隔離 GUI の最初のペイン（tako のペインの中の AI が叩く形。pane を省いた
# open は呼び出し元ペインで埋まる = #1958）
# `tako mcp serve` は接続情報を env で受ける（隔離 GUI の control.json から。#1958 の検査と同じ）
listing = json.loads(subprocess.run([tako, "list"], capture_output=True, text=True).stdout)
info = json.load(open(os.path.join(os.environ["TAKO_DISCOVERY_DIR"], "control.json")))
env = dict(os.environ, TAKO_PANE_ID=str(listing["tabs"][0]["panes"][0]["id"]),
           TAKO_SOCKET=info["socket"], TAKO_TOKEN=info["token"])


def cli(path):
    p = subprocess.run([tako, "open", path], capture_output=True, text=True, env=env)
    try:
        return json.loads(p.stdout)
    except ValueError:
        return {"error": (p.stdout + p.stderr).strip()}


class Stdio:
    def __init__(self):
        self.p = subprocess.Popen([tako, "mcp", "serve"], stdin=subprocess.PIPE,
                                  stdout=subprocess.PIPE, text=True, bufsize=1, env=env)
        self.n = 0
        self.rpc("initialize", {"protocolVersion": "2025-06-18"})
        self.p.stdin.write(json.dumps({"jsonrpc": "2.0", "method": "notifications/initialized"}) + "\n")

    def rpc(self, method, params):
        self.n += 1
        self.p.stdin.write(json.dumps({"jsonrpc": "2.0", "id": self.n, "method": method,
                                       "params": params}) + "\n")
        self.p.stdin.flush()
        return json.loads(self.p.stdout.readline())

    def open(self, path):
        r = self.rpc("tools/call", {"name": "tako_open_file", "arguments": {"path": path}})
        if "error" in r:
            return {"error": r["error"]["message"]}
        try:
            return json.loads(r["result"]["content"][0]["text"])
        except (KeyError, ValueError):
            return {"error": str(r)}

    def close(self):
        self.p.stdin.close()
        self.p.wait(timeout=10)


def show(name, route, body):
    ft = body.get("file_type") or "-"
    syn = (body.get("syntax") or "-").replace(" ", "_")
    print("%s %s %s %s%s" % (name, route, ft, syn,
                              "" if "error" not in body else " ERROR=" + body["error"]), flush=True)


for name in files:
    show(name, "cli", cli(os.path.join(fx, name)))
mcp = Stdio()
for name in files:
    show(name, "mcp", mcp.open(os.path.join(fx, name)))
mcp.close()
PY

FILES="$(echo "$EXPECT" | awk '{print $1}' | tr '\n' ' ')"

# 隔離 GUI を立てて応答を採る（$1 = ログ, 残り = GUI へ渡す VAR=VAL）
probe_responses() {
  local log="$1" out="$2"
  shift 2
  launch_isolated_gui "$log" ${1+"$@"} || exit $?
  if ! wait_isolated_gui "$log"; then
    echo "隔離 GUI が立たない（ログ: ${log}）"
    exit 1
  fi
  "$TAKO_BIN" autorename off >/dev/null 2>&1 || true
  # shellcheck disable=SC2086
  python3 "$TMP/drive.py" "$TAKO_BIN" "$FX" $FILES > "$out"
  stop_isolated_gui "$ISOLATED_GUI_PID"
}

# 採った応答を期待と突き合わせる（$1 = 応答, $2 = 列 2 = 新 / 3 = 旧, $3 = 見出し）
judge_responses() {
  local out="$1" col="$2" label="$3" name want ft_want route ft syn rest
  sed 's/^/    /' "$out"
  while read -r name route ft syn rest; do
    [ -n "$name" ] || continue
    want="$(echo "$EXPECT" | awk -v n="$name" -v c="$col" '$1 == n {print $c}')"
    ft_want="$(echo "$EXPECT" | awk -v n="$name" '$1 == n {print $4}')"
    # file_type は旧でも新の行が無い形式（.pde / .json5）だけ変わる = 新の値だけ縛る
    if [ "$syn" = "$want" ] && { [ "$col" != 2 ] || [ "$ft" = "$ft_want" ]; } && [ -z "$rest" ]; then
      pass "$label $name ($route): syntax=$syn file_type=$ft"
    else
      fail "$label $name ($route): syntax=${syn}（期待 ${want}）file_type=${ft}（期待 ${ft_want}）${rest}"
    fi
  done < "$out"
}

echo "① CLI / MCP の応答（新）"
probe_responses "$TMP/new-cli.log" "$TMP/new.txt"
judge_responses "$TMP/new.txt" 2 "新"

echo "② CLI / MCP の応答（旧 = TAKO_1953_LEGACY=1）"
probe_responses "$TMP/legacy-cli.log" "$TMP/legacy.txt" TAKO_1953_LEGACY=1
judge_responses "$TMP/legacy.txt" 3 "旧"

# 節を 1 回走らせて、終わるまで**状態で**待つ（プロセスが消えるまで。上限は 120 秒）
run_section() {
  local log="$1" dump="$2"
  shift 2
  mkdir -p "$dump"
  launch_isolated_gui "$log" TAKO_VISUAL_TEST=1 TAKO_VISUAL_ONLY=syntax-alias \
    TAKO_VISUAL_DUMP_DIR="$dump" ${1+"$@"} || exit $?
  local pid="$ISOLATED_GUI_PID" i
  for i in $(seq 1 1200); do
    kill -0 "$pid" 2>/dev/null || break
    sleep 0.1
  done
  stop_isolated_gui "$pid"
  return 0
}

echo "③ visual-test 節 syntax-alias（新）"
run_section "$TMP/new.log" "$DUMP_ROOT/new"
grep -E "TAKO_VISUAL_PIXEL: syntax-alias|TAKO_VISUAL_DUMP_FILE" "$TMP/new.log" | sed 's/^/    /'
if grep -q "TAKO_VISUAL_TEST_OK" "$TMP/new.log"; then
  pass "全相が緑（TAKO_VISUAL_TEST_OK）"
else
  fail "節が緑にならない"
  grep -E "FAILED|panicked|ERROR|SKIPPED" "$TMP/new.log" | tail -5 | sed 's/^/    /'
fi

echo "③ visual-test 節 syntax-alias（旧 = TAKO_1953_LEGACY=1）"
run_section "$TMP/legacy.log" "$DUMP_ROOT/legacy" TAKO_1953_LEGACY=1
grep -E "TAKO_VISUAL_PIXEL: syntax-alias|TAKO_VISUAL_DUMP_FILE" "$TMP/legacy.log" | sed 's/^/    /'
if grep -q "TAKO_APP_SELF_TEST_FAILED: ① geo.h の応答の syntax が C++" "$TMP/legacy.log"; then
  pass "旧経路では ① で落ちる: $(grep -o 'TAKO_APP_SELF_TEST_FAILED: .*' "$TMP/legacy.log" | head -1)"
else
  fail "旧経路でも ① で落ちない（節に検出力が無い）"
  grep -E "FAILED|panicked" "$TMP/legacy.log" | tail -3 | sed 's/^/    /'
fi

echo
echo "PASS=$PASS FAIL=$FAIL"
[ "$FAIL" -eq 0 ]
