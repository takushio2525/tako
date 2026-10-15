#!/usr/bin/env node
// agent 対応状況ページ（docs）のデータを能力マトリクスから生成する（Issue #982 / #2104）。
//
// **なぜ生成物なのか**: OS 軸の Windows 対応状況ページ（#591）と同じ理由。
// マトリクス（crates/tako-core/src/agent_support.rs）は診断・エラーメッセージ・
// system prompt が引く正本なので、docs をそこから作れば
// 「宣言・診断・docs」が常に同じ 1 つの事実を指す。
//
// **コミットするのはページではなくマトリクスの写し**（`docs/src/data/agent-support.json`。#2104）。
// ページ（`docs/src/content/docs/agent-support.md`）は docs のビルドが写しから組む
// （`docs/scripts/support-pages.mjs`）。理由は scripts/gen-windows-support-docs.mjs の冒頭と同じ
// （ページの件数の行が並列の PR を毎回衝突させていた）。
//
//   node scripts/gen-agent-support-docs.mjs           # 生成（写しを上書きし、手元のページも組み直す）
//   node scripts/gen-agent-support-docs.mjs --check    # 同期検査（CI 用）
//
// **どの tako を使うか**は `scripts/lib/tako-bin.mjs` が 1 か所で決める（#1548）。
// 選んだバイナリの版が `Cargo.toml` とずれていたら、生成も検査もせずに落ちる
// （古い debug ビルドを黙って使うと、偽の赤と docs の静かな巻き戻しが起きる）。
//
// カテゴリ未分類の能力があると**失敗する**（分類は `docs/scripts/support-pages.mjs` の CATEGORIES）。
// マトリクスへ行を足したらそこへも 1 行足すこと（分類漏れを検出するための仕掛けで、放置すると表から消える）。

import { execFileSync } from 'node:child_process';
import { readFileSync, writeFileSync, existsSync } from 'node:fs';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

import { resolveTakoBin } from './lib/tako-bin.mjs';
import {
  AGENT_DATA,
  AGENT_PAGE,
  canonicalJson,
  agentCounts,
  agentSnapshot,
  renderAgentSupport,
  serialize,
  writeIfChanged,
} from '../docs/scripts/support-pages.mjs';

const REPO = resolve(dirname(fileURLToPath(import.meta.url)), '..');

function matrix() {
  const raw = execFileSync(resolveTakoBin(REPO).path, ['agent-support', '--json'], {
    encoding: 'utf8',
    maxBuffer: 32 * 1024 * 1024,
  });
  return JSON.parse(raw);
}

/**
 * 写しの中身と、そこから組んだページ。ページが数える件数が tako 本体の集計と一致すること・
 * 分類漏れが無いこと（組むと分かる）も確かめる
 */
function snapshot() {
  const m = matrix();
  const data = agentSnapshot(m, 'scripts/gen-agent-support-docs.mjs');
  // 写しは件数を持たない（ページが数える）。数え方が本体の集計とずれたら、写しが正しくても
  // ページの数字が嘘になるので、ここで突き合わせて落とす
  const { counts, matrix_total } = agentCounts(data);
  if (canonicalJson(counts) !== canonicalJson(m.counts) || matrix_total !== m.matrix_total) {
    throw new Error(
      'ページが数える件数が `tako agent-support --json` の集計と一致しません' +
        `（数えた値 ${JSON.stringify({ counts, matrix_total })} / 本体 ${JSON.stringify({ counts: m.counts, matrix_total: m.matrix_total })}）。\n` +
        'docs/scripts/support-pages.mjs の agentCounts を本体の集計に合わせてください',
    );
  }
  return { body: serialize(data), page: renderAgentSupport(data) };
}

function main() {
  const { body, page } = snapshot();
  if (process.argv.includes('--check')) {
    const current = existsSync(AGENT_DATA) ? readFileSync(AGENT_DATA, 'utf8') : '';
    if (current !== body) {
      console.error(
        'docs/src/data/agent-support.json（エージェント対応状況ページのデータ）が能力マトリクスと同期していません。\n' +
          '`node scripts/gen-agent-support-docs.mjs` で再生成してコミットしてください。',
      );
      process.exit(1);
    }
    console.log('agent-support.json は能力マトリクスと同期しています');
  } else {
    writeFileSync(AGENT_DATA, body);
    // 手元のページも組み直す（コミットしない。docs のビルドでも同じものが組まれる）
    writeIfChanged(AGENT_PAGE, page);
    console.log('生成しました: docs/src/data/agent-support.json（ページは docs のビルドが組む）');
  }
}

// 失敗の理由（どのバイナリが古いか等）が 1 行目で読めるようにする。
// スタックトレースを出しても直し方は分からないので出さない
try {
  main();
} catch (e) {
  console.error(e instanceof Error ? e.message : String(e));
  process.exit(1);
}
