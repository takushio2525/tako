#!/usr/bin/env node
// claude-stub.mjs — 入れ方・使い方動画の収録で `claude` の名前に置く台。LLM は一切呼ばない。
//
// 何のためか:
//   動画は「Claude Code が入っていてログイン済みの人が、tako を入れて使い始める」流れを見せる。
//   収録の砂場には本物の claude を置かない（実アカウントで起こさない）ので、tako の初回起動の導線と
//   `tako setup` が claude へ問い合わせる 3 つにだけ、この台が答える:
//     - 版            `claude --version`
//     - 認証          `claude auth status --json`（tako は `loggedIn` と `subscriptionType` を読む）
//     - MCP の登録    `claude mcp add|remove|get|list`（本物と同じく $HOME/.claude.json の mcpServers を書く）
//   対話の起動（`tako master` / `tako setup` の対話アシスタント / worker）は、#2864 の偽エージェント
//   （scripts/promo/remake/demo/agent-sim.mjs）へ引数ごとそのまま渡す。
//   tako の側（検出 → 登録 → プロファイル → 起動 → 指示の送達 → 完了の検知）は本物の経路のまま動く。
//
// 砂場の HOME にしか書かない: 書き先は $HOME/.claude.json だけで、HOME は record.sh が砂場へ向ける。
//
// 使い方（record.sh が砂場の bin/claude から呼ぶ）:
//   claude-stub.mjs [本物の claude へ渡されるはずだった引数…]
//   AGENT_SIM_MJS       対話を渡す先（既定は ../../remake/demo/agent-sim.mjs）
//   CLAUDE_STUB_BOOT_MS 対話を渡す前に待つ ms（本物の起動の間。既定 0）
//   CLAUDE_STUB_LOG     あれば、呼ばれ方を 1 行ずつ残す（調べもの用）

import { appendFileSync, existsSync, readFileSync, writeFileSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';

const args = process.argv.slice(2);
const here = dirname(fileURLToPath(import.meta.url));

if (process.env.CLAUDE_STUB_LOG) {
	try {
		appendFileSync(process.env.CLAUDE_STUB_LOG, `${Date.now()}\ttty=${Boolean(process.stdin.isTTY && process.stdout.isTTY)}\t${JSON.stringify(args)}\n`);
	} catch {
		// 記録できなくても続ける
	}
}

const configFile = join(process.env.HOME || '.', '.claude.json');
const readConfig = () => {
	try {
		return JSON.parse(readFileSync(configFile, 'utf8'));
	} catch {
		return {};
	}
};
const servers = (config) => (config.mcpServers && typeof config.mcpServers === 'object' ? config.mcpServers : {});

function mcp(rest) {
	const sub = rest[0];
	const config = readConfig();
	if (sub === 'add') {
		// claude mcp add --scope user --transport stdio <名前> -- <コマンド> [引数…]
		const split = rest.indexOf('--');
		if (split < 0 || split + 1 >= rest.length) {
			console.error('usage: claude mcp add [options] <name> -- <command> [args...]');
			process.exit(1);
		}
		const name = rest[split - 1];
		if (servers(config)[name]) {
			console.error(`MCP server ${name} already exists in user config`);
			process.exit(1);
		}
		config.mcpServers = { ...servers(config), [name]: { type: 'stdio', command: rest[split + 1], args: rest.slice(split + 2), env: {} } };
		writeFileSync(configFile, `${JSON.stringify(config, null, 2)}\n`);
		console.log(`Added stdio MCP server ${name} to user config`);
		process.exit(0);
	}
	if (sub === 'remove') {
		const name = rest.filter((a, i) => i > 0 && !a.startsWith('-') && rest[i - 1] !== '--scope').pop();
		if (!name || !servers(config)[name]) {
			console.error(`No MCP server found with name: ${name}`);
			process.exit(1);
		}
		delete config.mcpServers[name];
		writeFileSync(configFile, `${JSON.stringify(config, null, 2)}\n`);
		console.log(`Removed MCP server ${name} from user config`);
		process.exit(0);
	}
	if (sub === 'get') {
		const entry = servers(config)[rest[1]];
		if (!entry) {
			console.error(`No MCP server found with name: ${rest[1]}`);
			process.exit(1);
		}
		console.log(`${rest[1]}:\n  Scope: User config\n  Type: stdio\n  Command: ${entry.command}\n  Args: ${(entry.args || []).join(' ')}`);
		process.exit(0);
	}
	if (sub === 'list') {
		const names = Object.keys(servers(config));
		if (!names.length) console.log('No MCP servers configured.');
		for (const name of names) console.log(`${name}: ${servers(config)[name].command} ${(servers(config)[name].args || []).join(' ')}`);
		process.exit(0);
	}
	process.exit(0);
}

if (args.some((a) => a === '--version' || a === '-v' || a === '-V')) {
	// tako は先頭の版数を読み、古い版には mod を入れない旨を出す。動画の前提（新しい Claude Code が入っている）に
	// 合わせて、その下限より上の版数を名乗る。括弧の中で本物でないことを明かす
	console.log('2.1.300 (Claude Code demo stand-in)');
	process.exit(0);
}
if (args[0] === 'auth' && args[1] === 'status') {
	// デモの前提: ログイン済み。個人を指す欄（メール・組織）は持たない
	console.log(JSON.stringify({ loggedIn: true, authMethod: 'claude.ai', subscriptionType: 'pro' }));
	process.exit(0);
}
if (args[0] === 'mcp') mcp(args.slice(1));

// ここから先は対話の起動。端末の無い呼び出し（tako の背景の問い合わせ）は何も描かずに終わる
if (!process.stdin.isTTY || !process.stdout.isTTY) process.exit(0);

const agentSim = resolve(process.env.AGENT_SIM_MJS || join(here, '../../remake/demo/agent-sim.mjs'));
if (!existsSync(agentSim)) {
	console.error(`偽のエージェントが見つからない: ${agentSim}（#2864 の scripts/promo/remake が要る。AGENT_SIM_MJS で置き場を渡せる）`);
	process.exit(1);
}
// 本物の claude は起動して画面を描くまでに 1〜2 秒かかる。偽物は一瞬で画面を描き直すので、
// 直前の出力（tako setup の結果など）が読めるだけの間をおいてから渡す（CLAUDE_STUB_BOOT_MS。既定は待たない）
const bootMs = Number(process.env.CLAUDE_STUB_BOOT_MS || 0);
if (bootMs > 0) await new Promise((r) => setTimeout(r, bootMs));
// agent-sim.mjs は argv[2] をエージェント名、その後ろを本物へ渡されるはずだった引数として読む
process.argv = [process.argv[0], agentSim, 'claude', ...args];
await import(pathToFileURL(agentSim).href);
