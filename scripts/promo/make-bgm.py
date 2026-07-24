#!/usr/bin/env python3
# tako:run: python3 scripts/promo/make-bgm.py
"""紹介動画 (#470) の BGM を合成する。

外部音源を使わず波形から組み立てる。理由は権利関係の単純化で、
生成物は tako リポジトリの一部（GPL-3.0-or-later）として扱える。
出典・ライセンスの記録は .agent/plans/2026-07-promo-video.md にある。

トーン: 開発者向けの落ち着いたミニマル・エレクトロニカ。
A マイナーの 4 コード循環（Am - F - C - G）に、キック・ハイハット・
ベース・アルペジオ・パッドを重ねる。台本の 7 シーンに合わせて
音数を出し入れし、S5（再起動シーン）でいったん抜いて復帰させる。

使い方: python3 scripts/promo/make-bgm.py [出力パス]
既定の出力先は ~/Desktop/tako-promo/audio/bgm.wav
"""
from __future__ import annotations

import array
import math
import os
import random
import struct
import sys
import wave

SR = 44100
BPM = 100.0
BEAT = 60.0 / BPM  # 0.6s
BAR = BEAT * 4  # 2.4s
TOTAL = 115.0  # 秒。106 秒の本編（v3 構成）+ 前後の余白

# A マイナー。数値は A4=440 を基準にした周波数
NOTE = {
    "A2": 110.00, "C3": 130.81, "E3": 164.81, "F2": 87.31, "G2": 98.00,
    "A3": 220.00, "C4": 261.63, "E4": 329.63, "F3": 174.61, "G3": 196.00,
    "A4": 440.00, "C5": 523.25, "E5": 659.26, "F4": 349.23, "G4": 392.00,
    "B4": 493.88, "D5": 587.33,
}

# コード進行（2 小節ごと）: Am - F - C - G
PROGRESSION = [
    ("A2", ["A3", "C4", "E4"], ["A4", "C5", "E5", "C5"]),
    ("F2", ["F3", "A3", "C4"], ["F4", "A4", "C5", "A4"]),
    ("C3", ["C4", "E4", "G4"], ["C5", "E5", "G4", "E5"]),
    ("G2", ["G3", "B4", "D5"], ["G4", "B4", "D5", "B4"]),
]


def env(t: float, dur: float, a: float, d: float, s: float, r: float) -> float:
    """ADSR エンベロープ。t/dur は秒"""
    if t < 0 or t > dur:
        return 0.0
    if t < a:
        return t / a if a > 0 else 1.0
    if t < a + d:
        return 1.0 - (1.0 - s) * ((t - a) / d if d > 0 else 1.0)
    if t < dur - r:
        return s
    return s * max(0.0, (dur - t) / r if r > 0 else 0.0)


def saw(phase: float) -> float:
    return 2.0 * (phase - math.floor(phase + 0.5))


def tri(phase: float) -> float:
    return 2.0 * abs(2.0 * (phase - math.floor(phase + 0.5))) - 1.0


class Track:
    def __init__(self, seconds: float):
        self.n = int(SR * seconds)
        self.buf = array.array("d", [0.0]) * self.n

    def add(self, start: float, samples, gain: float = 1.0) -> None:
        i0 = int(start * SR)
        for k, v in enumerate(samples):
            i = i0 + k
            if 0 <= i < self.n:
                self.buf[i] += v * gain


def gen_pad(freqs: list[str], dur: float) -> list[float]:
    """ゆっくり立ち上がるパッド。3 音を軽くデチューンして重ねる"""
    out = []
    n = int(dur * SR)
    fs = [NOTE[f] for f in freqs]
    for i in range(n):
        t = i / SR
        e = env(t, dur, 0.55, 0.3, 0.75, 0.7)
        v = 0.0
        for f in fs:
            for det in (-0.16, 0.0, 0.16):
                v += saw(((f + det) * t) % 1.0)
        # 高域を落として耳あたりを柔らかくする（1 次ローパス相当の移動平均）
        out.append(v / (len(fs) * 3) * e)
    sm = []
    prev = 0.0
    for v in out:
        prev = prev * 0.82 + v * 0.18
        sm.append(prev)
    return sm


def gen_arp(note: str, dur: float) -> list[float]:
    """短い減衰のアルペジオ音"""
    f = NOTE[note]
    n = int(dur * SR)
    out = []
    for i in range(n):
        t = i / SR
        e = env(t, dur, 0.004, 0.10, 0.22, 0.10)
        v = tri((f * t) % 1.0) * 0.7 + math.sin(2 * math.pi * f * 2 * t) * 0.12
        out.append(v * e)
    return out


def gen_bass(note: str, dur: float) -> list[float]:
    f = NOTE[note]
    n = int(dur * SR)
    out = []
    for i in range(n):
        t = i / SR
        e = env(t, dur, 0.012, 0.16, 0.6, 0.14)
        v = math.sin(2 * math.pi * f * t)
        v += 0.22 * math.sin(2 * math.pi * f * 2 * t)
        out.append(math.tanh(v * 1.25) * e)
    return out


def gen_kick(dur: float = 0.32) -> list[float]:
    n = int(dur * SR)
    out = []
    for i in range(n):
        t = i / SR
        # 110Hz から 42Hz へ落ちるピッチエンベロープ
        f = 42.0 + 68.0 * math.exp(-t * 28.0)
        e = math.exp(-t * 9.0)
        out.append(math.sin(2 * math.pi * f * t) * e)
    return out


def gen_hat(dur: float = 0.055, seed: int = 0) -> list[float]:
    rnd = random.Random(seed)
    n = int(dur * SR)
    out = []
    prev = 0.0
    for i in range(n):
        t = i / SR
        e = math.exp(-t * 55.0)
        w = rnd.uniform(-1.0, 1.0)
        hp = w - prev  # 簡易ハイパス
        prev = w
        out.append(hp * e)
    return out


def build() -> Track:
    tr = Track(TOTAL)
    bars = int(TOTAL / BAR) + 1

    for b in range(bars):
        t0 = b * BAR
        if t0 >= TOTAL:
            break
        root, pad_notes, arp_notes = PROGRESSION[(b // 2) % len(PROGRESSION)]

        # セクションごとの音量（台本 v3 のシーン割りに対応）
        # 導入 → 画面操作で厚く → setup（36〜65s）は対話を読ませるため軽く →
        # master + プロジェクト文脈（65〜98s）で最も厚く → アウトロ（98s〜）で抜く
        pad_g, arp_g, bass_g, drum_g = 0.30, 0.0, 0.0, 0.0
        if t0 >= 5.0:
            arp_g = 0.16
        if t0 >= 10.0:
            drum_g, bass_g = 0.55, 0.30
        if t0 >= 17.0:
            arp_g, pad_g = 0.20, 0.34
        if 36.0 <= t0 < 65.0:  # setup: テロップと対話画面を読ませるため軽くする
            drum_g, bass_g, arp_g = 0.30, 0.20, 0.12
        if t0 >= 65.0:  # master + プロジェクト文脈: 一番厚くする
            drum_g, bass_g, arp_g, pad_g = 0.60, 0.32, 0.22, 0.36
        if t0 >= 98.0:  # アウトロ
            drum_g, bass_g = 0.0, 0.16
            arp_g = 0.10

        if pad_g > 0:
            tr.add(t0, gen_pad(pad_notes, BAR * 1.02), pad_g)
        if bass_g > 0:
            for k in (0, 2):
                tr.add(t0 + k * BEAT, gen_bass(root, BEAT * 1.6), bass_g)
        if drum_g > 0:
            for k in range(4):
                tr.add(t0 + k * BEAT, gen_kick(), drum_g)
                tr.add(t0 + k * BEAT + BEAT / 2, gen_hat(seed=b * 7 + k), drum_g * 0.30)
        if arp_g > 0:
            step = BEAT / 2
            for k in range(8):
                note = arp_notes[k % len(arp_notes)]
                tr.add(t0 + k * step, gen_arp(note, step * 1.5), arp_g)
    return tr


def write_wav(tr: Track, path: str) -> None:
    peak = max(abs(v) for v in tr.buf) or 1.0
    norm = 0.82 / peak
    frames = array.array("h", [0]) * (tr.n * 2)
    fade = int(1.2 * SR)
    for i in range(tr.n):
        v = tr.buf[i] * norm
        # 先頭と末尾を必ずフェードして繋ぎ目のプチノイズを避ける
        if i < fade:
            v *= i / fade
        if i > tr.n - fade:
            v *= max(0.0, (tr.n - i) / fade)
        s = int(max(-1.0, min(1.0, v)) * 32000)
        frames[i * 2] = s
        frames[i * 2 + 1] = s
    with wave.open(path, "wb") as w:
        w.setnchannels(2)
        w.setsampwidth(2)
        w.setframerate(SR)
        w.writeframes(frames.tobytes())


def main() -> None:
    out = sys.argv[1] if len(sys.argv) > 1 else os.path.expanduser(
        "~/Desktop/tako-promo/audio/bgm.wav")
    os.makedirs(os.path.dirname(out), exist_ok=True)
    print(f"合成中: {TOTAL:.0f} 秒 / {BPM:.0f} BPM ...")
    tr = build()
    write_wav(tr, out)
    print(f"書き出し: {out}")


if __name__ == "__main__":
    main()
