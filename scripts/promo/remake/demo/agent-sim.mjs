#!/usr/bin/env node
// agent-sim.mjs — プロモ動画の収録に使う「偽のエージェント」。LLM は一切呼ばない。
//
// 何のためか:
//   収録では本物の claude / codex / agy を実アカウントで起こさない。代わりにこのスクリプトを
//   `claude` などの名前で PATH の先頭へ置く（置くのは record.sh）。tako は本物の CLI と同じ手順
//   （入力欄を見つける → 貼り付け → Enter → 稼働中の表示を見る → 待機に戻るのを見る）で扱うので、
//   `tako master` / `tako orchestrator spawn` / `tako send` / 右パネルの orch ビューは
//   **本物の経路のまま**動く。差し替わっているのは「考える中身」だけで、そこは台本（persona）が流す。
//
// tako が見ているもの（crates/tako-core/src/screen.rs・crates/tako-control/src/claude_tui.rs）:
//   - 入力欄: 画面の下 24 行にある、プロンプト記号（ここでは `> `）で始まる行。上下を罫線で挟む
//   - 稼働中: 画面に `esc to interrupt` がある間
//
// 使い方:
//   agent-sim.mjs <エージェント名> [本物の CLI へ渡されるはずだった引数…]
//   persona は AGENT_SIM_PERSONA、無ければ TAKO_ORCHESTRATOR_ROLE（`master` / `worker:<project>:<label>`）から決める。
//   台本は AGENT_SIM_DIR（既定はこのファイルの隣の personas/）の <persona>.json。
//   AGENT_SIM_BEATS があれば、台本の `beat` を `名前\tepoch ms` で 1 行ずつ足す（合成のタイムラインの材料）。

import { execFile } from 'node:child_process';
import { appendFileSync, existsSync, readFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const agent = process.argv[2] || 'agent';
const args = process.argv.slice(3);
const here = dirname(fileURLToPath(import.meta.url));

// 版の問い合わせと、端末の無い呼び出し（tako の背景の問い合わせ）は何も描かずに終わる
if (args.some((a) => a === '--version' || a === '-v' || a === '-V')) {
	console.log(`0.0.0 (${agent} demo stand-in)`);
	process.exit(0);
}
if (!process.stdin.isTTY || !process.stdout.isTTY) process.exit(0);

const role = process.env.TAKO_ORCHESTRATOR_ROLE || '';
const personaName =
	process.env.AGENT_SIM_PERSONA || (role.startsWith('worker:') ? role.split(':').pop() : role) || 'solo';
const personaDir = process.env.AGENT_SIM_DIR || join(here, 'personas');
const personaFile = join(personaDir, `${personaName}.json`);
const persona = existsSync(personaFile)
	? JSON.parse(readFileSync(personaFile, 'utf8'))
	: { title: personaName, turns: [[{ say: '（台本がありません）' }]] };

const ESC = '\x1b';
// 色は端末の 16 色に任せる（tako のテーマを切り替えても読める）。差し色と依頼の帯だけ固定の色
const C = {
	reset: `${ESC}[0m`,
	dim: `${ESC}[90m`,
	text: `${ESC}[39m`,
	accent: `${ESC}[38;2;232;90;110m`,
	ok: `${ESC}[32m`,
	info: `${ESC}[34m`,
	user: `${ESC}[38;2;240;240;244m${ESC}[48;2;58;63;78m`,
	bold: `${ESC}[1m`,
};

// 表示幅（全角は 2）。罫線・記号は 1 として数える
function charWidth(cp) {
	if (cp < 0x1100) return 1;
	if (
		(cp >= 0x1100 && cp <= 0x115f) ||
		(cp >= 0x2e80 && cp <= 0xa4cf) ||
		(cp >= 0xac00 && cp <= 0xd7a3) ||
		(cp >= 0xf900 && cp <= 0xfaff) ||
		(cp >= 0xfe30 && cp <= 0xfe4f) ||
		(cp >= 0xff00 && cp <= 0xff60) ||
		(cp >= 0xffe0 && cp <= 0xffe6)
	)
		return 2;
	return 1;
}
function fit(text, max) {
	let w = 0;
	let out = '';
	for (const ch of text) {
		const cw = charWidth(ch.codePointAt(0));
		if (w + cw > max) return out;
		out += ch;
		w += cw;
	}
	return out;
}

function wrap(text, max) {
	const out = [];
	let rest = text;
	while (rest.length) {
		const head = fit(rest, max);
		if (!head.length) break;
		out.push(head);
		rest = rest.slice(head.length);
	}
	return out.length ? out : [''];
}

const transcript = []; // { text, color, indent, wrap }
let input = '';
let busySince = 0;
let busyLabel = '';
let turn = 0;
let running = false;
const vars = {};

function push(text, color = C.text, indent = 0, wrapped = false) {
	transcript.push({ text, color, indent, wrap: wrapped });
	paint();
}

function paint() {
	const cols = Math.max(20, process.stdout.columns || 80);
	const rows = Math.max(8, process.stdout.rows || 24);
	const rule = '─'.repeat(cols);
	const head = [` ${C.accent}●${C.reset} ${C.bold}${C.text}${fit(persona.title || personaName, cols - 4)}${C.reset}`, ''];
	const status = busySince
		? ` ${C.accent}●${C.reset} ${C.text}${fit(`${busyLabel} ${Math.floor((Date.now() - busySince) / 1000)}s`, cols - 22)}${C.reset} ${C.dim}· esc to interrupt${C.reset}`
		: '';
	const foot = [
		'',
		status,
		`${C.dim}${rule}${C.reset}`,
		`> ${C.text}${fit(input, cols - 3)}${C.reset}`,
		`${C.dim}${rule}${C.reset}`,
		`${C.dim}${fit(`  ${persona.footer || 'demo'}`, cols)}${C.reset}`,
	];
	const room = rows - head.length - foot.length;
	const body = transcript
		.flatMap((l) => {
			const pad = ' '.repeat(1 + l.indent);
			const width = cols - pad.length - 1;
			const parts = l.wrap ? wrap(l.text, width - 2).map((t) => ` ${t} `) : [fit(l.text, width)];
			return parts.map((t) => `${pad}${l.color}${t}${C.reset}`);
		})
		.slice(-Math.max(0, room));
	const lines = [...head, ...body, ...foot];
	// 全面を描き直す（分割でペインの寸法が変わっても崩れない）。最後の行で改行しない
	process.stdout.write(`${ESC}[?25l${ESC}[H${ESC}[2J${lines.join('\r\n')}${ESC}[${head.length + body.length + 4};${3 + Math.min(input.length * 2, cols - 3)}H${ESC}[?25h`);
}

function beat(name) {
	const file = process.env.AGENT_SIM_BEATS;
	if (!file) return;
	try {
		appendFileSync(file, `${personaName}:${name}\t${Date.now()}\n`);
	} catch {
		// 収録の材料が書けなくても画面は続ける
	}
}

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const scale = Number(process.env.AGENT_SIM_SPEED || 1); // 待ちの倍率（撮るときに間を伸ばす）

function subst(text) {
	return text.replace(/\$\{(\w+)\}/g, (_, k) => (k in vars ? String(vars[k]) : ''));
}

// 本物のコマンドを走らせる（司令塔の台本が `tako orchestrator spawn` などを実際に打つ）
function run(cmd) {
	return new Promise((resolve) => {
		execFile('/bin/sh', ['-c', cmd], { encoding: 'utf8', maxBuffer: 4 << 20 }, (err, stdout, stderr) => {
			resolve({ code: err ? (err.code ?? 1) : 0, stdout, stderr });
		});
	});
}

// ほかのペインの台本が打った目印（`<persona>:<名前>`）を待つ。司令塔が担当の開始を待つのに使う
async function waitBeat(name, timeoutMs = 60000) {
	const file = process.env.AGENT_SIM_BEATS;
	const until = Date.now() + timeoutMs;
	while (file && Date.now() < until) {
		try {
			if (readFileSync(file, 'utf8').split('\n').some((l) => l.startsWith(`${name}\t`))) return;
		} catch {
			// まだ誰も書いていない
		}
		await sleep(150);
	}
}

async function play(steps) {
	running = true;
	for (const s of steps) {
		if (s.wait) await sleep(s.wait * scale);
		if (s.waitBeat) await waitBeat(s.waitBeat);
		if (s.until) {
			// 本物のコマンドの応答（JSON）の 1 つの欄が、待っている値になるまで見に行く
			// （司令塔が `tako orchestrator status` で担当の待機を見る）
			const until = Date.now() + (s.timeout || 90000);
			while (Date.now() < until) {
				const r = await run(subst(s.until));
				try {
					if (String(JSON.parse(r.stdout)[s.field]) === String(s.equals)) break;
				} catch {
					// 応答が読めない間は待ち続ける
				}
				await sleep(600);
			}
		}
		if (s.busy !== undefined) {
			busySince = s.busy ? Date.now() : 0;
			busyLabel = typeof s.busy === 'string' ? s.busy : '作業中';
			paint();
		}
		if (s.beat) beat(s.beat);
		if (s.say !== undefined) push(subst(s.say), C[s.color] || C.text, s.indent || 0);
		if (s.tool) push(`${s.tool.padEnd(5)} ${subst(s.arg || '')}`, C.dim, 2);
		if (s.exec) {
			const r = await run(subst(s.exec));
			if (s.capture) {
				// 応答の JSON から 1 つ取り出して覚える（例: spawn の応答の pane）
				try {
					vars[s.capture] = JSON.parse(r.stdout)[s.field || s.capture];
				} catch {
					vars[s.capture] = '';
				}
			}
			if (s.code) vars[s.code] = r.code;
		}
	}
	busySince = 0;
	running = false;
	paint();
}

function submit() {
	const text = input.trim();
	if (!text || running) return;
	input = '';
	const lines = text.split(/\r?\n/).filter((l) => l.trim());
	// 届いた依頼は冒頭だけ見せる（tako が送る型つきの指示は長い）
	for (const l of lines.slice(0, persona.promptLines || 2)) push(l, C.user, 0, true);
	if (lines.length > (persona.promptLines || 2)) push(`  … ほか ${lines.length - (persona.promptLines || 2)} 行`, C.dim);
	push('');
	beat(`prompt${turn + 1}`);
	const steps = (persona.turns || [])[Math.min(turn, (persona.turns || []).length - 1)] || [];
	turn += 1;
	play(steps);
}

// 入力: 貼り付けの目印と制御列は読み捨て、Enter で送る
let pasting = false;
function feed(chunk) {
	// 調べもの用: 届いた入力をそのまま残す（AGENT_SIM_RAW にファイルを指定したときだけ）
	if (process.env.AGENT_SIM_RAW) appendFileSync(process.env.AGENT_SIM_RAW, `${personaName}\t${Date.now()}\t${JSON.stringify(chunk)}\n`);
	let s = chunk;
	while (s.length) {
		if (s.startsWith(`${ESC}[200~`)) {
			pasting = true;
			s = s.slice(6);
			continue;
		}
		if (s.startsWith(`${ESC}[201~`)) {
			pasting = false;
			s = s.slice(6);
			continue;
		}
		if (s[0] === ESC) {
			const m = /^\x1b(\[[0-9;?<>=]*[ -/]*[@-~]|O.|.)?/.exec(s);
			s = s.slice(Math.max(1, m ? m[0].length : 1));
			continue;
		}
		const ch = s[0];
		s = s.slice(1);
		if (ch === '\x03' || ch === '\x04') return quit();
		if (ch === '\r' || ch === '\n') {
			if (pasting) input += '\n';
			else submit();
			continue;
		}
		if (ch === '\x7f' || ch === '\b') {
			input = [...input].slice(0, -1).join('');
			continue;
		}
		if (ch < ' ') continue;
		input += ch;
	}
	paint();
}

function quit() {
	process.stdout.write(`${ESC}[?2004l${ESC}[?25h${C.reset}\r\n`);
	process.exit(0);
}

process.stdin.setRawMode(true);
process.stdin.setEncoding('utf8');
process.stdin.on('data', feed);
process.stdin.on('end', quit);
process.stdout.on('resize', paint);
process.on('SIGHUP', quit);
process.on('SIGTERM', quit);
process.stdout.write(`${ESC}[?2004h`);
setInterval(() => busySince && paint(), 1000);
paint();
beat('ready');
if (persona.greeting) for (const l of persona.greeting) push(l, C.dim);
