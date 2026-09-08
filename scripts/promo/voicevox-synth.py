#!/usr/bin/env python3
# tako:run: python3 scripts/promo/voicevox-synth.py --help
"""VOICEVOX ENGINE で 1 区間を合成する（narrate.sh の voicevox バックエンド）。

エンジンは HTTP API なので、合成は 2 段になる:
  1. POST /audio_query  … テキストを読み（アクセント句）へ変換した AudioQuery を得る
  2. POST /synthesis    … AudioQuery のパラメータを差し替えて wav を得る

1 段目の戻り値には `kana` が入っており、**エンジンが実際にどう読むか**を耳で聴かずに
確認できる（--print-kana）。外来語の読み違いはここで先に見つけられる。

使い方:
  voicevox-synth.py --text <本文> --out <out.wav> [--speaker N] [調整オプション]
  voicevox-synth.py --text <本文> --print-kana        # 読みだけを出す（合成しない）

読みの誤りは `reading-overrides.tsv`（同じディレクトリ）で直す。合成と --print-kana の
**両方が同じ置換を通る**ので、耳で聴かずに読みを検証できる形を保つ。

ライセンス: 生成音声の利用にはキャラクターごとのクレジット表記が必要。
既定のずんだもん（speaker=3）は「VOICEVOX:ずんだもん」。--print-credit で引ける。
"""
from __future__ import annotations

import argparse
import json
import os
import pathlib
import sys
import urllib.error
import urllib.parse
import urllib.request

DEFAULT_URL = os.environ.get("TAKO_PROMO_VV_URL", "http://127.0.0.1:50021")
OVERRIDES_TSV = pathlib.Path(__file__).with_name("reading-overrides.tsv")


def load_overrides(path: pathlib.Path = OVERRIDES_TSV) -> list[tuple[str, str]]:
    """読み替え表を「表記の長い順」で返す（短い表記が長い表記の一部を先に食わないため）。"""
    if not path.exists():
        return []
    pairs = []
    for line in path.read_text(encoding="utf-8").splitlines():
        if not line.strip() or line.startswith("#"):
            continue
        cols = line.split("\t")
        if len(cols) < 2 or not cols[0] or not cols[1]:
            continue
        pairs.append((cols[0], cols[1]))
    return sorted(pairs, key=lambda kv: len(kv[0]), reverse=True)


def apply_overrides(text: str, pairs: list[tuple[str, str]]) -> tuple[str, list[str]]:
    """置換後のテキストと、実際に当たった表記の一覧を返す。"""
    hit = []
    for surface, reading in pairs:
        if surface in text:
            text = text.replace(surface, reading)
            hit.append(surface)
    return text, hit


def _post(url: str, body: bytes | None = None, timeout: float = 180.0):
    req = urllib.request.Request(
        url, data=b"" if body is None else body, method="POST",
        headers={} if body is None else {"Content-Type": "application/json"})
    return urllib.request.urlopen(req, timeout=timeout)


def audio_query(url: str, text: str, speaker: int) -> dict:
    q = urllib.parse.urlencode({"text": text, "speaker": speaker})
    with _post(f"{url}/audio_query?{q}", timeout=60) as r:
        return json.load(r)


def speaker_credit(url: str, speaker: int) -> str:
    """話者の policy からクレジット表記を引く（当てずっぽうで書かないため）。"""
    with urllib.request.urlopen(f"{url}/speakers", timeout=30) as r:
        for s in json.load(r):
            for st in s["styles"]:
                if st["id"] == speaker:
                    return f"VOICEVOX:{s['name']}"
    raise SystemExit(f"ERROR: speaker={speaker} が見つからない")


def main() -> int:
    p = argparse.ArgumentParser(add_help=True)
    p.add_argument("--text")
    p.add_argument("--out")
    p.add_argument("--url", default=DEFAULT_URL)
    p.add_argument("--speaker", type=int, default=int(os.environ.get("TAKO_PROMO_VV_SPEAKER", "3")))
    p.add_argument("--print-kana", action="store_true", help="読みだけを出して合成しない")
    p.add_argument("--no-overrides", action="store_true",
                   help="reading-overrides.tsv を適用しない（修正前との A/B 用）")
    p.add_argument("--report-overrides", action="store_true",
                   help="当たった読み替えを stderr へ出す")
    p.add_argument("--print-credit", action="store_true", help="クレジット表記を出して終わる")
    # 調整パラメータ。既定値の根拠は .agent/plans/2026-09-youtube-explainer.md「声の選定」
    p.add_argument("--speed", type=float, default=float(os.environ.get("TAKO_PROMO_VV_SPEED", "1.05")))
    p.add_argument("--pitch", type=float, default=float(os.environ.get("TAKO_PROMO_VV_PITCH", "-0.05")))
    p.add_argument("--intonation", type=float,
                   default=float(os.environ.get("TAKO_PROMO_VV_INTONATION", "1.28")))
    p.add_argument("--pause-scale", type=float,
                   default=float(os.environ.get("TAKO_PROMO_VV_PAUSE", "1.1")))
    a = p.parse_args()

    try:
        if a.print_credit:
            print(speaker_credit(a.url, a.speaker))
            return 0
        if not a.text:
            p.error("--text が必要")
        if a.no_overrides:
            spoken, hit = a.text, []
        else:
            spoken, hit = apply_overrides(a.text, load_overrides())
        if a.report_overrides and hit:
            print("読み替え: " + " / ".join(hit), file=sys.stderr)
        query = audio_query(a.url, spoken, a.speaker)
        if a.print_kana:
            print(query["kana"])
            return 0
        if not a.out:
            p.error("--out が必要")
        query.update(speedScale=a.speed, pitchScale=a.pitch,
                     intonationScale=a.intonation, pauseLengthScale=a.pause_scale,
                     outputSamplingRate=48000, outputStereo=False)
        with _post(f"{a.url}/synthesis?speaker={a.speaker}",
                   json.dumps(query).encode()) as r:
            data = r.read()
        with open(a.out, "wb") as f:
            f.write(data)
        return 0
    except urllib.error.URLError as e:
        print(f"ERROR: VOICEVOX ENGINE ({a.url}) へ繋がらない: {e}\n"
              f"       エンジンを起動してから再実行する（narrate.sh の先頭コメント参照）",
              file=sys.stderr)
        return 1


if __name__ == "__main__":
    sys.exit(main())
