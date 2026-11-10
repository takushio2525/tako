#!/usr/bin/env node
// setup-facts.mjs — 対話のセットアップの台本（personas/setup.json）が、画面に出す値を実際の設定から読むための補助。
//
// 何のためか:
//   アシスタントの発言は台本だが、発言の中の値（指示ファイルの場所と突き合わせの結果・master / worker の
//   エージェントと effort・登録したプロジェクト・設定共有のリポジトリと件数）は台本に書き込まず、
//   `tako setup` と台本が打った本物のコマンドが砂場に書いた設定から読む。値が変われば画面も変わる。
//
// 読むもの（どれも本物の tako が書いたファイルと、本物の tako / git の出力）:
//   ./setup-context.yaml                          tako setup がアシスタントへ渡す現状（カレントは setup の置き場）
//   <data>/orchestrator/profiles/default.yaml     master / worker の起動設定
//   `tako orchestrator projects list`             登録済みのプロジェクト
//   `tako config status --json` と git の origin   設定共有の配線先・件数
//
// 使い方（台本の exec から）: node "$DEMO_FACTS"
//   出力は平たい JSON 1 つ。台本は 1 回の exec で 1 つの欄を取り出して覚える（agent-sim.mjs の capture）

import { execFileSync } from 'node:child_process';
import { existsSync, readFileSync } from 'node:fs';
import { join } from 'node:path';

const home = process.env.HOME || '';
const data = process.env.TAKO_DATA_DIR || join(home, 'Library/Application Support/tako');
const tilde = (p) => (home && String(p).startsWith(home) ? `~${String(p).slice(home.length)}` : String(p));

// 入れ子の対応だけを読む小さな YAML 読み（tako が書く設定の形 = 字下げ 2 つ・値は 1 行）
function yaml(file) {
	const root = {};
	if (!existsSync(file)) return root;
	const stack = [{ indent: -1, node: root }];
	for (const raw of readFileSync(file, 'utf8').split('\n')) {
		const m = /^(\s*)([^\s#:-][^:]*):\s*(.*)$/.exec(raw);
		if (!m) continue;
		const indent = m[1].length;
		while (stack[stack.length - 1].indent >= indent) stack.pop();
		const parent = stack[stack.length - 1].node;
		const value = m[3].trim().replace(/^['"]|['"]$/g, '');
		if (value === '') {
			parent[m[2]] = {};
			stack.push({ indent, node: parent[m[2]] });
		} else {
			parent[m[2]] = value;
		}
	}
	return root;
}

const run = (cmd, args) => {
	try {
		return execFileSync(cmd, args, { encoding: 'utf8', stdio: ['ignore', 'pipe', 'ignore'] });
	} catch {
		return '';
	}
};

const context = yaml('setup-context.yaml');
const profile = yaml(join(data, 'orchestrator/profiles/default.yaml'));
const coverage = (context.instruction_coverage || {}).status || '';
const policy = profile.worker_model_policy || 'inherit';
const master = profile.master_agent || 'claude';
const worker = profile.worker_agent || master;

// 登録済みのプロジェクト（1 行 = キー・フォルダ）
const projects = run('tako', ['orchestrator', 'projects', 'list'])
	.split('\n')
	.map((l) => l.trim().split(/\s+/))
	.filter((w) => w.length >= 2 && w[1].startsWith('/'));

// 設定共有: 配線先と、共有したファイルの数。リモートは git に登録した URL そのもの
let share = {};
try {
	share = JSON.parse(run('tako', ['config', 'status', '--json']) || '{}');
} catch {
	share = {};
}
const files = Array.isArray(share.files) ? share.files : [];
const remote = share.repo ? run('git', ['-C', share.repo, 'config', '--get', 'remote.origin.url']).trim() : '';

process.stdout.write(
	`${JSON.stringify({
		agent: context.selected_agent || '',
		plan: (context.provider_plans || {})[context.selected_agent] || '',
		instruction_file: context.instruction_file || '',
		// instruction_coverage.status の言い方は system-prompt.md の Step 1 のとおり
		coverage:
			{ created_default: '同梱の推奨ルールで新しく作成済みです（同梱推奨ルールとの差分なし）', full: '同梱推奨ルールとの差分なし', partial: '不足の可能性があります' }[coverage] ||
			coverage,
		gh_can_create_repo: String((context.config_share || {}).gh_can_create_repo || ''),
		master,
		worker,
		model: profile.model || '未指定',
		effort: profile.effort || '',
		// 言い方は tako setup の「次の一歩」と同じ（profile_summary_lines）
		policy: { inherit: 'master に合わせる', fixed: '固定値', delegate: 'master がタスクごとに判断' }[policy] || policy,
		worker_effort: ((profile.worker_agents || {})[worker] || {}).effort || profile.effort || '',
		projects: String(projects.length),
		project_key: projects.length ? projects[projects.length - 1][0] : '',
		project_dir: projects.length ? tilde(projects[projects.length - 1][1]) : '',
		share_linked: String(share.linked === true),
		share_repo: share.repo ? tilde(share.repo) : '',
		share_remote: remote.replace(/^https:\/\//, ''),
		share_files: String(files.length),
		share_differs: String(files.filter((f) => f.state !== 'same').length),
	})}\n`,
);
