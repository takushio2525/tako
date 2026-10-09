#!/usr/bin/env python3
"""#1960: 軽いモデル（Sonnet / Haiku）に `tako mod ui` の口だけで画面の UI の調整を 20 件頼み、
ui.json のスキーマ違反の数と依頼との一致率を測る。`scripts/test-mod-ui-llm.sh` から呼ぶ（CI 外）。

    mod-ui-llm.py --tako <tako> --mode oracle|null|claude [--claude <claude>] [--model sonnet]
                  --env-file <隔離の env を 1 行 1 個で書いたファイル> --work <一時 dir>

- oracle: 依頼ごとの「正解の口」を叩く（採点器そのものの検証。20 / 20 一致・違反 0 になるはず）
- null:   何もしない（判定が既定値で通ってしまう依頼が無いかの検出力の確認）
- claude: `claude -p` に依頼を渡す。使ってよい道具は `Bash(tako mod ui:*)` だけ

本物の設定 dir・本物の data dir には触れない（env はシェルが一時 dir で組んだものだけを使う）。
依頼の文と判定は下の CASES が正本。
"""

import argparse
import json
import os
import subprocess
import sys

RECOMMENDED = ["compact", "context", "split-right", "session-restart-handoff"]


def has(d, kind, key, value):
    return any(b["action"]["kind"] == kind and b["action"].get(key) == value for b in d["buttons"])


CASES = [
    dict(request="/context を開くボタンを足して",
         oracle=[["button", "add", "context"]],
         check=lambda d: has(d, "slash", "command", "context")),
    dict(request="ペインを右に分割するボタンを追加して",
         oracle=[["button", "add", "split-right"]],
         check=lambda d: has(d, "tako", "op", "split-right")),
    dict(request="下にペインを分割するボタンを足して。押すキーは d にして",
         oracle=[["button", "add", "split-down", "--hotkey", "d"]],
         check=lambda d: any(b["action"].get("op") == "split-down" and b["hotkey"] == "d" for b in d["buttons"])),
    dict(request="npm test を実行するボタンを test という名前で足して",
         oracle=[["button", "add", "shell", "npm test", "--label", "test"]],
         check=lambda d: any(b["action"].get("command") == "npm test" and b["label"] == "test" for b in d["buttons"])),
    dict(request="入力欄に「テストを書いて」と入れるボタンを足して（送信はしない）",
         oracle=[["button", "add", "prompt", "テストを書いて"]],
         check=lambda d: any(b["action"]["kind"] == "prompt" and "テストを書いて" in b["action"]["text"] for b in d["buttons"])),
    dict(request="compact のボタンを消して",
         oracle=[["button", "remove", "compact"]],
         check=lambda d: not has(d, "slash", "command", "compact")),
    dict(request="使用量のバーを消して",
         oracle=[["set", "usage_bar.place", "off"]],
         check=lambda d: d["usage_bar"]["place"] == "off"),
    dict(request="使用量のバーをプロンプトの上の帯の中に出して",
         oracle=[["set", "usage_bar.place", "band"]],
         check=lambda d: d["usage_bar"]["place"] == "band"),
    dict(request="バーには 5 時間の使用制限だけを出して",
         oracle=[["set", "usage_bar.items", "five_hour"]],
         check=lambda d: d["usage_bar"]["items"] == ["five_hour"]),
    dict(request="プロンプトの上の tako の帯を隠して",
         oracle=[["set", "band.hidden", "true"]],
         check=lambda d: d["band"]["hidden"] is True),
    dict(request="帯にはペイン名と要注意とボタンだけを出して",
         oracle=[["set", "band.segments", "pane,attention,buttons"]],
         check=lambda d: set(d["band"]["segments"]) == {"pane", "attention", "buttons"}),
    dict(request="強調の色を claude のオレンジにして",
         oracle=[["set", "colors.accent", "claude"]],
         check=lambda d: d["colors"]["accent"] == "claude"),
    dict(request="チャットの吹き出しを GUI モード以外でも常に出して",
         oracle=[["set", "chat.bubble", "always"]],
         check=lambda d: d["chat"]["bubble"] == "always"),
    dict(request="コードブロックの下のコピーボタンは要らない",
         oracle=[["set", "chat.code_copy", "false"]],
         check=lambda d: d["chat"]["code_copy"] is False),
    dict(request="おすすめの設定にして",
         oracle=[["preset", "recommended"]],
         check=lambda d: [b["id"] for b in d["buttons"]] == RECOMMENDED),
    dict(request="いちばん控えめな最小の設定にして",
         oracle=[["preset", "minimal"]],
         check=lambda d: d["band"]["segments"] == ["pane", "attention", "buttons"] and d["usage_bar"]["items"] == ["ctx"]),
    dict(request="split right のボタンをいちばん左に移して",
         pre=[["button", "add", "split-right"]],
         oracle=[["button", "move", "split-right", "first"]],
         check=lambda d: d["buttons"][0]["id"] == "split-right"),
    dict(request="UI の設定を全部初期状態に戻して",
         pre=[["set", "usage_bar.place", "off"], ["button", "add", "context"], ["set", "colors.dim", "claude"]],
         oracle=[["reset"]],
         check=lambda d: d["usage_bar"]["place"] == "prompt_hint" and [b["id"] for b in d["buttons"]] == ["compact"]
         and d["colors"]["dim"] == "subtle"),
    dict(request="clear・context・cost・model・effort・tako・split right・split down・close のボタンを全部足して。"
                 "入りきらなければ入るところまででいい",
         oracle=[["button", "add", v] for v in ["clear", "context", "cost", "model", "effort", "tako", "split-right"]],
         check=lambda d: len(d["buttons"]) == 8),
    dict(request="シェルの /bash を送るボタンを slash で足して。できなければ何も変えないで",
         oracle=[],
         check=lambda d: [b["id"] for b in d["buttons"]] == ["compact"] and not any(
             b["action"]["kind"] == "slash" and b["action"]["command"] not in
             ("compact", "clear", "context", "cost", "model", "effort", "tako") for b in d["buttons"])),
]

PROMPT = (
    "あなたは tako（ターミナル）の Claude Code の画面の UI 設定を調整する係です。"
    "使ってよいのは `tako mod ui` コマンドだけです（ファイルを直接読んだり書いたりしない）。"
    "まず `tako mod ui` を実行して今の値と選べる値を確かめてから、次の依頼をそのとおりに実行してください。"
    "コマンドが断られたら、返ってきた「使える値」から選び直してください。\n\n依頼: {request}"
)


def read_env(path):
    env = {}
    with open(path, encoding="utf-8") as f:
        for line in f:
            line = line.rstrip("\n")
            if "=" in line:
                k, v = line.split("=", 1)
                env[k] = v
    return env


def tako_ui(tako, env, args):
    return subprocess.run([tako, "mod", "ui", *args], env=env, capture_output=True, text=True, timeout=60)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--tako", required=True)
    ap.add_argument("--mode", choices=["oracle", "null", "claude"], required=True)
    ap.add_argument("--claude")
    ap.add_argument("--model", default="sonnet")
    ap.add_argument("--env-file", required=True)
    ap.add_argument("--work", required=True)
    ap.add_argument("--only", type=int, default=0, help="1 件だけ（1 始まり）")
    a = ap.parse_args()
    base = read_env(a.env_file)
    rows = []
    for i, case in enumerate(CASES, start=1):
        if a.only and a.only != i:
            continue
        data = os.path.join(a.work, f"case-{i:02d}")
        os.makedirs(data, exist_ok=True)
        env = dict(base, TAKO_DATA_DIR=data)
        for cmd in case.get("pre", []):
            tako_ui(a.tako, env, cmd)
        note = ""
        if a.mode == "oracle":
            for cmd in case["oracle"]:
                tako_ui(a.tako, env, cmd)
        elif a.mode == "claude":
            # 認証は呼び手が渡した API キーだけ（ファイルへは書かない・本物の設定 dir の認証は使わない）
            env["ANTHROPIC_API_KEY"] = os.environ.get("ANTHROPIC_API_KEY", "")
            try:
                r = subprocess.run(
                    [a.claude, "-p", "--model", a.model, "--max-turns", "10",
                     "--allowedTools", "Bash(tako mod ui:*)", PROMPT.format(request=case["request"])],
                    env=env, capture_output=True, text=True, timeout=300, cwd=data)
                note = f"claude rc={r.returncode}"
            except subprocess.TimeoutExpired:
                note = "claude timeout"
        state = json.loads(tako_ui(a.tako, env, ["--json"]).stdout)
        violation = state["state"] not in ("absent", "valid")
        try:
            match = bool(case["check"](state["ui"]))
        except Exception as e:  # 判定が形の崩れで落ちたら不一致
            match = False
            note += f" check error: {type(e).__name__}"
        rows.append((i, match, violation, note.strip(), case["request"]))
    total = len(rows)
    matched = sum(1 for r in rows if r[1])
    violations = sum(1 for r in rows if r[2])
    for i, match, violation, note, request in rows:
        mark = "一致" if match else "不一致"
        bad = " スキーマ違反" if violation else ""
        print(f"  {i:2d}. {mark}{bad}  {request}" + (f"  ({note})" if note else ""))
    print(f"RESULT mode={a.mode} model={a.model if a.mode == 'claude' else '-'} "
          f"cases={total} matched={matched} violations={violations}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
