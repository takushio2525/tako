#!/usr/bin/env node
// bgm.mjs — プロモ動画（リメイク版）の音を、波形から合成して wav に書く。
//
// 外部の音源・サンプルは使わない（全部この中の式で作る = 権利の確認が要らない）。
// 96 BPM・4/4 で 1 小節 = 2.5 秒。映像の場面の切り替わり（comp/timeline.json）は小節の頭に置いてあり、
// 曲の展開（静かな導入 → 題字で開ける → 4 つ打ちの本編 → 「閉じる」で一度抜く → 終わりの和音）を合わせる。
// 効果音（切り替わりの風・ペインが増える音など）は timeline.json の sfx の時刻に置く。
//
// 使い方: node scripts/promo/remake/bgm.mjs <出力.wav>

import { writeFileSync } from 'node:fs';
import { loadTimeline } from './timeline.mjs';

// 効果音の時刻は収録の出来事から決まるので、映像と同じ読み方（timeline.mjs）を通す
const { timeline } = loadTimeline();
const out = process.argv[2];
if (!out) {
	console.error('使い方: bgm.mjs <出力.wav>');
	process.exit(2);
}

const SR = 48000;
const BEAT = 60 / 96;
const BAR = BEAT * 4;
const DUR = timeline.duration;
const N = Math.ceil(DUR * SR);
const TAU = Math.PI * 2;

const bus = () => [new Float32Array(N), new Float32Array(N)];
const dry = bus(); // そのまま出す音
const duck = bus(); // キックに合わせて少し引く音（和音・ベース）
const send = bus(); // 残響へ送る音
const kickEnv = new Float32Array(N);

// 乱数は種から作る（何度書き出しても同じ波形になる）
let seed = 0x7a6b01;
const rnd = () => {
	seed |= 0;
	seed = (seed + 0x6d2b79f5) | 0;
	let t = Math.imul(seed ^ (seed >>> 15), 1 | seed);
	t = (t + Math.imul(t ^ (t >>> 7), 61 | t)) ^ t;
	return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
};
const noise = () => rnd() * 2 - 1;
const mtof = (m) => 440 * 2 ** ((m - 69) / 12);

function add(target, t, len, fn, pan = 0, toSend = 0) {
	const n0 = Math.floor(t * SR);
	const n = Math.min(Math.floor(len * SR), N - n0);
	const gl = Math.cos(((pan + 1) * Math.PI) / 4);
	const gr = Math.sin(((pan + 1) * Math.PI) / 4);
	for (let i = 0; i < n; i += 1) {
		if (n0 + i < 0) continue;
		const s = fn(i / SR, i);
		target[0][n0 + i] += s * gl;
		target[1][n0 + i] += s * gr;
		if (toSend) {
			send[0][n0 + i] += s * gl * toSend;
			send[1][n0 + i] += s * gr * toSend;
		}
	}
}

// ---- 楽器 ----

// はじく音（2 オペレータの FM。立ち上がりが速く、すぐ丸くなる）
function pluck(t, midi, vel = 1, pan = 0, decay = 5.5, wet = 0.35) {
	const f = mtof(midi);
	add(dry, t, 1.4, (tt) => {
		const env = Math.exp(-tt * decay) * (1 - Math.exp(-tt * 700));
		const mod = Math.sin(TAU * f * 2 * tt) * 1.5 * Math.exp(-tt * 16);
		return (Math.sin(TAU * f * tt + mod) + 0.22 * Math.sin(TAU * f * 2 * tt) * Math.exp(-tt * 10)) * env * vel * 0.26;
	}, pan, wet);
}

// 鈴のような旋律の音（長めに伸びる）
function bell(t, midi, len, vel = 1, pan = 0) {
	const f = mtof(midi);
	add(dry, t, len + 1.2, (tt) => {
		const env = Math.exp(-tt * 2.6) * (1 - Math.exp(-tt * 400));
		const mod = Math.sin(TAU * f * 3.01 * tt) * 0.9 * Math.exp(-tt * 7);
		return Math.sin(TAU * f * tt + mod) * env * vel * 0.2;
	}, pan, 0.6);
}

// 和音の持続音（少しずらした正弦波を重ねて幅を出す）
function pad(t, notes, len, vel = 1) {
	const partials = [1, 0.32, 0.13, 0.05];
	for (const [k, midi] of notes.entries()) {
		for (const det of [-1, 1]) {
			const f = mtof(midi) * 2 ** ((det * 5) / 1200);
			const phase = rnd() * TAU;
			add(duck, t, len + 1.0, (tt) => {
				const env = (1 - Math.exp(-tt / 0.45)) * (tt < len ? 1 : Math.exp(-(tt - len) * 3.2));
				let s = 0;
				for (let p = 0; p < partials.length; p += 1) s += partials[p] * Math.sin(TAU * f * (p + 1) * tt + phase * (p + 1));
				return s * env * vel * 0.024 * (1 + 0.12 * Math.sin(TAU * 0.23 * tt + k));
			}, det * 0.7, 0.5);
		}
	}
}

function bass(t, midi, len, vel = 1) {
	const f = mtof(midi);
	add(duck, t, len + 0.08, (tt) => {
		const env = (1 - Math.exp(-tt * 260)) * (tt < len ? Math.exp(-tt * 1.4) : Math.exp(-len * 1.4) * Math.exp(-(tt - len) * 60));
		const s = Math.sin(TAU * f * tt) + 0.5 * Math.sin(TAU * f * 2 * tt) + 0.22 * Math.sin(TAU * f * 3 * tt);
		return Math.tanh(s * 1.3) * env * vel * 0.2;
	});
}

function kick(t, vel = 1) {
	let phase = 0;
	add(dry, t, 0.45, (tt) => {
		const f = 44 + 86 * Math.exp(-tt * 30);
		phase += (TAU * f) / SR;
		return (Math.sin(phase) * Math.exp(-tt * 8.5) + noise() * 0.25 * Math.exp(-tt * 500)) * vel * 0.5;
	});
	const n0 = Math.floor(t * SR);
	for (let i = 0; i < 0.4 * SR && n0 + i < N; i += 1) kickEnv[n0 + i] = Math.max(kickEnv[n0 + i], vel * Math.exp((-i / SR) * 9));
}

function hat(t, vel = 1, open = false, pan = 0.2) {
	let a = 0;
	let b = 0;
	add(dry, t, open ? 0.3 : 0.07, (tt) => {
		const x = noise();
		const hp = x - a; // 差分を 2 回とって高い帯だけ残す
		a = x;
		const hp2 = hp - b;
		b = hp;
		return hp2 * Math.exp(-tt * (open ? 16 : 75)) * vel * 0.062;
	}, pan);
}

// 指を鳴らすような 2・4 拍の音（帯域を絞った雑音を細かく 3 回）
function snap(t, vel = 1) {
	const w0 = (TAU * 1900) / SR;
	const alpha = Math.sin(w0) / (2 * 1.3);
	const b0 = alpha;
	const a0 = 1 + alpha;
	const a1 = -2 * Math.cos(w0);
	const a2 = 1 - alpha;
	let x1 = 0;
	let x2 = 0;
	let y1 = 0;
	let y2 = 0;
	add(dry, t, 0.28, (tt) => {
		const burst = Math.exp(-tt * 60) + (tt > 0.011 ? Math.exp(-(tt - 0.011) * 60) : 0) + (tt > 0.023 ? Math.exp(-(tt - 0.023) * 24) : 0);
		const x = noise() * burst;
		const y = (b0 * x - b0 * x2 - a1 * y1 - a2 * y2) / a0;
		x2 = x1;
		x1 = x;
		y2 = y1;
		y1 = y;
		return y * vel * 0.4;
	}, -0.1, 0.45);
}

// 場面の前の盛り上げ（雑音の帯域と音量が上がっていく）
function riser(t0, t1, vel = 1) {
	const len = t1 - t0;
	let lp = 0;
	add(dry, t0, len, (tt) => {
		const p = tt / len;
		const cutoff = 300 * (9000 / 300) ** p;
		const k = 1 - Math.exp((-TAU * cutoff) / SR);
		lp += k * (noise() - lp);
		return (lp * 0.5 + 0.12 * Math.sin(TAU * (220 * 4 ** p) * tt)) * p * p * vel * 0.2;
	}, 0, 0.3);
}

// 場面の切り替わりの風
function whoosh(t, vel = 1) {
	let lp = 0;
	const len = 0.55;
	const n0 = Math.floor(t * SR);
	for (let i = 0; i < len * SR && n0 + i < N; i += 1) {
		const p = i / (len * SR);
		const cutoff = 500 + 3800 * Math.sin(Math.PI * p);
		const k = 1 - Math.exp((-TAU * cutoff) / SR);
		lp += k * (noise() - lp);
		const s = lp * Math.sin(Math.PI * p) ** 2 * vel * 0.16;
		const pan = -0.7 + 1.4 * p;
		dry[0][n0 + i] += s * Math.cos(((pan + 1) * Math.PI) / 4);
		dry[1][n0 + i] += s * Math.sin(((pan + 1) * Math.PI) / 4);
	}
}

// 画面で何かが現れたときの小さな音
function pop(t, vel = 1, midi = 81) {
	const f = mtof(midi);
	add(dry, t, 0.2, (tt) => Math.sin(TAU * f * (1 + 0.5 * Math.exp(-tt * 60)) * tt) * Math.exp(-tt * 26) * (1 - Math.exp(-tt * 900)) * vel * 0.11, 0.15, 0.5);
}

// 上がる / 下がる音（閉じる・開き直す）
function glide(t, m0, m1, len, vel = 1) {
	let phase = 0;
	add(dry, t, len + 0.3, (tt) => {
		const p = Math.min(1, tt / len);
		const f = mtof(m0 + (m1 - m0) * (1 - (1 - p) ** 2));
		phase += (TAU * f) / SR;
		return Math.sin(phase) * (1 - Math.exp(-tt * 300)) * Math.exp(-Math.max(0, tt - len) * 12) * (1 - 0.5 * p) * vel * 0.1;
	}, 0, 0.6);
}

// ---- 和音と型 ----

const CH = {
	D: { notes: [50, 57, 62, 66, 69], root: 38 },
	Bm: { notes: [47, 54, 57, 62, 66], root: 35 },
	G: { notes: [43, 50, 54, 59, 62], root: 43 },
	A: { notes: [45, 52, 57, 61, 64], root: 45 },
	As: { notes: [45, 52, 57, 62, 64], root: 45 },
};
const arpTones = (c) => [c.notes[2], c.notes[3], c.notes[4], c.notes[2] + 12, c.notes[3] + 12];
const ARP16 = [0, 2, 1, 3, 2, 4, 3, 2, 0, 2, 1, 3, 4, 3, 2, 1];
const ARP8 = [0, 2, 3, 2, 1, 3, 4, 2];

function drums(t, level = 1, ghost = false) {
	kick(t, 1 * level);
	kick(t + BEAT * 2, 0.9 * level);
	if (ghost) kick(t + BEAT * 3.5, 0.55 * level);
	snap(t + BEAT, 0.9 * level);
	snap(t + BEAT * 3, 1 * level);
	for (let i = 0; i < 8; i += 1) hat(t + (i * BEAT) / 2, (i % 2 ? 1 : 0.55) * level, i === 7, i % 2 ? 0.3 : -0.2);
}

function bassBar(t, c, level = 1) {
	bass(t, c.root, BEAT * 0.8, 1 * level);
	bass(t + BEAT * 1.5, c.root, BEAT * 0.4, 0.8 * level);
	bass(t + BEAT * 2, c.root, BEAT * 0.7, 0.95 * level);
	bass(t + BEAT * 2.75, c.root + 12, BEAT * 0.2, 0.6 * level);
	bass(t + BEAT * 3.5, c.root, BEAT * 0.4, 0.8 * level);
}

function arp(t, c, sixteenth, level = 1) {
	const tones = arpTones(c);
	const pattern = sixteenth ? ARP16 : ARP8;
	const step = sixteenth ? BEAT / 4 : BEAT / 2;
	pattern.forEach((idx, i) => {
		const accent = i % (sixteenth ? 4 : 2) === 0 ? 1 : 0.6;
		pluck(t + i * step, tones[idx], accent * level, i % 2 ? 0.45 : -0.45);
	});
}

function phrase(t, events, level = 1) {
	for (const [beat, midi, len] of events) bell(t + beat * BEAT, midi, len * BEAT, level, 0.1);
}
const PH_A = [[0, 78, 1], [1.5, 81, 0.5], [2, 83, 1.5], [3.5, 81, 0.5], [4, 78, 1], [5, 76, 2], [7.5, 74, 0.5]];
const PH_B = [[0, 76, 1], [1.5, 78, 0.5], [2, 81, 2], [4.5, 78, 0.5], [5, 76, 1], [6, 74, 2]];

// ---- 構成（小節は 1 始まり。bar(b) = その小節の頭の秒） ----

const bar = (b) => (b - 1) * BAR;

// 1–5 小節: 導入（散らばる窓）。落ち着かない感じを、低い和音と細かい刻みで
[CH.Bm, CH.Bm, CH.G, CH.G, CH.As].forEach((c, i) => {
	const t = bar(1 + i);
	pad(t, c.notes, BAR, 0.8);
	pluck(t, c.notes[4] + 12, 0.8, -0.3, 3.5, 0.6);
	pluck(t + BEAT * 2.5, c.notes[3] + 12, 0.55, 0.3, 3.5, 0.6);
	if (i >= 1) for (let k = 0; k < 8; k += 1) hat(t + (k * BEAT) / 2, 0.35 + 0.08 * i, false, k % 2 ? 0.4 : -0.4);
	if (i >= 2) bass(t, c.root, BEAT * 1.6, 0.7);
});
riser(bar(5), bar(6), 1);

// 6–8 小節: 題字。開ける
kick(bar(6), 1);
[62, 66, 69, 74, 78, 81, 86].forEach((m, i) => pluck(bar(6) + i * (BEAT / 4), m, 0.9, -0.5 + i * 0.16, 4, 0.6));
[CH.D, CH.Bm].forEach((c, i) => {
	const t = bar(6 + i);
	pad(t, c.notes, BAR, 1);
	bass(t, c.root, BEAT * 2.5, 0.9);
	if (i === 1) arp(t, c, false, 0.8);
});
pad(bar(8), CH.G.notes, BEAT * 2, 1);
pad(bar(8) + BEAT * 2, CH.A.notes, BEAT * 2, 1);
bass(bar(8), CH.G.root, BEAT * 1.6, 0.9);
bass(bar(8) + BEAT * 2, CH.A.root, BEAT * 1.6, 0.9);
arp(bar(8), CH.G, false, 0.8);
for (let k = 0; k < 8; k += 1) hat(bar(8) + BEAT * 2 + (k * BEAT) / 4, 0.4 + k * 0.09, false, 0);

// 9–16 小節: 違い 1（本編の型。2 巡）
[CH.D, CH.Bm, CH.G, CH.A, CH.D, CH.Bm, CH.G, CH.A].forEach((c, i) => {
	const t = bar(9 + i);
	pad(t, c.notes, BAR, 0.9);
	drums(t, 1, i % 4 === 3);
	bassBar(t, c);
	arp(t, c, i >= 4, i >= 4 ? 0.75 : 0.85);
});

// 17–22 小節: 違い 2（旋律を足す）
[CH.Bm, CH.G, CH.D, CH.A, CH.Bm, CH.G].forEach((c, i) => {
	const t = bar(17 + i);
	pad(t, c.notes, BAR, 0.9);
	drums(t, 1, i % 2 === 1);
	bassBar(t, c);
	arp(t, c, true, 0.6);
	if (i === 0) phrase(t, PH_A);
	if (i === 2) phrase(t, PH_B);
	if (i === 4) phrase(t, PH_A);
});

// 23–26 小節: 違い 3（閉じる所で一度抜き、開き直す所で戻る）
pad(bar(23), CH.D.notes, BAR, 0.9);
drums(bar(23), 1);
bassBar(bar(23), CH.D);
arp(bar(23), CH.D, false, 0.7);
pad(bar(24), CH.G.notes, BAR, 0.8);
pluck(bar(24) + BEAT, 74, 0.6, -0.3, 3, 0.7);
pluck(bar(24) + BEAT * 2.5, 71, 0.5, 0.3, 3, 0.7);
[CH.Bm, CH.A].forEach((c, i) => {
	const t = bar(25 + i);
	pad(t, c.notes, BAR, 0.9);
	drums(t, 1, i === 1);
	bassBar(t, c);
	arp(t, c, true, 0.65);
});

// 27–30 小節: くらべると（少し引く）
[CH.D, CH.Bm, CH.G, CH.A].forEach((c, i) => {
	const t = bar(27 + i);
	pad(t, c.notes, BAR, 0.9);
	drums(t, 0.8);
	bassBar(t, c, 0.9);
	arp(t, c, false, 0.75);
});

// 31–34 小節: 立ち位置（旋律を戻して、終わりへ向かう）
[CH.G, CH.A, CH.Bm, CH.A].forEach((c, i) => {
	const t = bar(31 + i);
	pad(t, c.notes, BAR, 0.95);
	drums(t, 1, i % 2 === 1);
	bassBar(t, c);
	arp(t, c, true, 0.6);
	if (i === 0) phrase(t, PH_B);
	if (i === 2) phrase(t, PH_A);
});
riser(bar(34) + BEAT * 2, bar(35), 0.8);

// 35–38 小節: 入手方法。主和音に着地して、鳴らし切る
kick(bar(35), 1);
[CH.D, CH.G].forEach((c, i) => {
	const t = bar(35 + i);
	pad(t, c.notes, BAR, 1);
	bass(t, c.root, BEAT * 3, 0.9);
	arp(t, c, false, 0.7 - i * 0.15);
	snap(t + BEAT, 0.5);
	snap(t + BEAT * 3, 0.5);
});
kick(bar(37), 0.8);
pad(bar(37), [50, 57, 62, 64, 66, 69], BAR * 1.2, 1);
bass(bar(37), 38, BEAT * 5, 0.9);
[62, 66, 69, 76, 81].forEach((m, i) => pluck(bar(37) + i * (BEAT / 4), m, 0.8, -0.4 + i * 0.2, 2.2, 0.8));

// ---- 効果音（映像の時刻） ----

for (const s of timeline.sfx || []) {
	if (s.kind === 'whoosh') whoosh(s.t, s.vel ?? 1);
	if (s.kind === 'pop') pop(s.t, s.vel ?? 1, s.note ?? 81);
	if (s.kind === 'down') glide(s.t, 76, 57, 0.5, s.vel ?? 1);
	if (s.kind === 'up') glide(s.t, 62, 81, 0.45, s.vel ?? 1);
}

// ---- 残響（くし形 4 本 + 全域通過 2 本） ----

function reverb(input, delaysMs) {
	const outBuf = new Float32Array(N);
	for (const ms of delaysMs) {
		const d = Math.floor((ms / 1000) * SR);
		const buf = new Float32Array(d);
		let lp = 0;
		for (let i = 0, p = 0; i < N; i += 1, p = (p + 1) % d) {
			const y = buf[p];
			lp += 0.28 * (y - lp); // 高い帯は早く消す
			buf[p] = input[i] + lp * 0.8;
			outBuf[i] += y * 0.25;
		}
	}
	for (const [ms, g] of [[5.0, 0.7], [1.7, 0.7]]) {
		const d = Math.floor((ms / 1000) * SR);
		const buf = new Float32Array(d);
		for (let i = 0, p = 0; i < N; i += 1, p = (p + 1) % d) {
			const delayed = buf[p];
			const x = outBuf[i] + delayed * g;
			buf[p] = x;
			outBuf[i] = delayed - x * g;
		}
	}
	return outBuf;
}
const wet = [reverb(send[0], [29.7, 37.1, 41.1, 43.7]), reverb(send[1], [30.9, 36.3, 42.3, 45.1])];

// ---- まとめ ----

const mix = bus();
let peak = 0;
for (let ch = 0; ch < 2; ch += 1) {
	let hp = 0;
	let prev = 0;
	for (let i = 0; i < N; i += 1) {
		const t = i / SR;
		let s = dry[ch][i] + duck[ch][i] * (1 - 0.38 * kickEnv[i]) + wet[ch][i] * 0.55;
		// 30 Hz より下を落とす
		hp = 0.996 * (hp + s - prev);
		prev = s;
		s = hp;
		// 頭と終わりをなだらかに
		s *= Math.min(1, t / 0.25) * Math.min(1, Math.max(0, (DUR - t) / 2.2));
		s = Math.tanh(s * 1.4) / 1.4;
		mix[ch][i] = s;
		peak = Math.max(peak, Math.abs(s));
	}
}
const gain = 0.84 / peak;
const pcm = Buffer.alloc(44 + N * 4);
pcm.write('RIFF', 0);
pcm.writeUInt32LE(36 + N * 4, 4);
pcm.write('WAVEfmt ', 8);
pcm.writeUInt32LE(16, 16);
pcm.writeUInt16LE(1, 20);
pcm.writeUInt16LE(2, 22);
pcm.writeUInt32LE(SR, 24);
pcm.writeUInt32LE(SR * 4, 28);
pcm.writeUInt16LE(4, 32);
pcm.writeUInt16LE(16, 34);
pcm.write('data', 36);
pcm.writeUInt32LE(N * 4, 40);
for (let i = 0; i < N; i += 1) {
	pcm.writeInt16LE(Math.round(Math.max(-1, Math.min(1, mix[0][i] * gain)) * 32767), 44 + i * 4);
	pcm.writeInt16LE(Math.round(Math.max(-1, Math.min(1, mix[1][i] * gain)) * 32767), 46 + i * 4);
}
writeFileSync(out, pcm);
console.log(`書き出した: ${out}（${DUR} 秒 / ${SR} Hz stereo / 合成時のピーク ${peak.toFixed(3)} → 0.84 へそろえた）`);
