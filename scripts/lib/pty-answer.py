#!/usr/bin/env python3
"""擬似端末（PTY）でコマンドを走らせ、出てきたプロンプトへ順に答える。

`tako setup` の「インストールしますか？ [y/N]」は **stdin が端末のときだけ**
出る（#1499）ので、パイプで流し込むと経路そのものを検査できない。
`script(1)` は入力が届く時機を制御できず、先頭の EOF と混ざって 1 つずれる
（#1499 の実測）ため、ここで PTY を自前で開いて**プロンプトを見てから**答える。

    pty-answer.py [--expect '[y/N]:'] [--answer y] [--answer N]
                  [--stop-after '<この文字列が出たら打ち切る>'] [--timeout 120]
                  -- <コマンド> [引数...]

`--expect` は複数渡せる（どれか 1 つが出るたびに次の答えを返す。#1506 の
`tako setup --review` は `[y/N]:` と `選択 [N]:` が混ざって出る）。
省略時は `[y/N]:` だけ。

子プロセスが PTY へ書いたものをそのまま stdout へ流し、終了コードを引き継ぐ。
答えを使い切ったあとのプロンプトには改行だけ（= 既定の N）を返す。
時間切れは子を殺して 124（`timeout(1)` と同じ）。

`--stop-after` は「観たいものが出たので、その先の対話は別の検証の担当」という
ときに使う（例: `tako setup --review` は依存の段より後ろでも聞くので、
指定しないと**時間切れに頼る**形になる）。打ち切りは終了コード 0。
"""

import argparse
import codecs
import os
import pty
import select
import signal
import subprocess
import sys
import time


def main() -> int:
    parser = argparse.ArgumentParser(add_help=False)
    # 既定は append の外で入れる（default に置くと指定したぶんが既定へ足される）
    parser.add_argument("--expect", action="append", default=None)
    parser.add_argument("--answer", action="append", default=[])
    parser.add_argument("--stop-after")
    parser.add_argument("--timeout", type=float, default=120.0)
    parser.add_argument("command", nargs=argparse.REMAINDER)
    args = parser.parse_args()

    command = args.command[1:] if args.command[:1] == ["--"] else args.command
    if not command:
        print("pty-answer.py: コマンドがありません", file=sys.stderr)
        return 2

    master, slave = pty.openpty()
    child = subprocess.Popen(
        command, stdin=slave, stdout=slave, stderr=slave, close_fds=True
    )
    os.close(slave)

    answers = list(args.answer)
    expects = args.expect or ["[y/N]:"]
    # 4096 バイト境界で UTF-8 が割れても文字を壊さない（出力は日本語なので
    # 壊れると `--expect` / `--stop-after` の突き合わせが空振りする）
    decoder = codecs.getincrementaldecoder("utf-8")("replace")
    pending = ""  # プロンプトは改行で終わらないので、行末を跨いで持ち越す
    deadline = time.monotonic() + args.timeout
    out = sys.stdout
    timed_out = False
    stopped = False
    seen = ""  # --stop-after の判定用（プロンプトと違い改行を跨ぐので別に持つ）
    while True:
        remaining = deadline - time.monotonic()
        if remaining <= 0:
            timed_out = True
            break
        if not select.select([master], [], [], min(remaining, 0.5))[0]:
            if child.poll() is not None:
                break
            continue
        try:
            chunk = os.read(master, 4096)
        except OSError:  # 子が終わって PTY が閉じた
            break
        if not chunk:
            break
        text = decoder.decode(chunk)
        out.write(text)
        out.flush()
        if args.stop_after:
            # 突き合わせに要る長さだけ持つ（長い出力で際限なく伸ばさない）
            seen = (seen + text)[-2 * len(args.stop_after) :]
            if args.stop_after in seen:
                stopped = True
                break
        pending += text
        while True:
            # 出てきた順に答える（複数の --expect のうち先に現れたものから）
            hits = [(pending.find(e), e) for e in expects if e in pending]
            if not hits:
                break
            at, expect = min(hits)
            pending = pending[at + len(expect) :]
            answer = answers.pop(0) if answers else ""
            os.write(master, (answer + "\n").encode())

    if stopped:
        child.terminate()
        try:
            child.wait(timeout=5)
        except subprocess.TimeoutExpired:
            child.kill()
            child.wait()
        os.close(master)
        return 0
    if timed_out:
        child.send_signal(signal.SIGKILL)
        child.wait()
        os.close(master)
        print("\npty-answer.py: 時間切れ（止まったまま返らない）", file=sys.stderr)
        return 124
    os.close(master)
    return child.wait()


if __name__ == "__main__":
    sys.exit(main())
