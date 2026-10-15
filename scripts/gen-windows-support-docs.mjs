#!/usr/bin/env node
// Windows 対応状況ページ（docs）のデータを対応マトリクスから生成する（Issue #591 / #2104）。
//
// **なぜ生成物なのか**: 機能は 140 件あり、手で書いた表は必ず実装から遅れる。
// マトリクス（crates/tako-core/src/platform/support.rs）は system prompt へも
// 流れる正本（#516）なので、docs をそこから作れば「宣言・prompt・docs」が
// 常に同じ 1 つの事実を指す。
//
// **コミットするのはページではなくマトリクスの写し**（`docs/src/data/windows-support.json`。#2104）。
// ページ（`docs/src/content/docs/windows-support.md`）は docs のビルドが写しから組む
// （`docs/scripts/support-pages.mjs`。Cloudflare Pages には tako が無いので写しが要る）。
// 以前はページそのものをコミットしていて、件数の行が対応表のどの行を直しても書き換わるため、
// 別々の行を直す並列の PR が毎回そこで衝突していた。写しは件数を持たず 1 行 = 1 フィールドなので、
// 別の行を直す PR どうしは写しでも衝突しない。
//
//   node scripts/gen-windows-support-docs.mjs           # 生成（写しを上書きし、手元のページも組み直す）
//   node scripts/gen-windows-support-docs.mjs --check    # 同期検査（CI 用）
//
// **どの tako を使うか**は `scripts/lib/tako-bin.mjs` が 1 か所で決める（#1548）。
// 選んだバイナリの版が `Cargo.toml` とずれていたら、生成も検査もせずに落ちる
// （古い debug ビルドを黙って使うと、偽の赤と docs の静かな巻き戻しが起きる）。
//
// カテゴリ未分類の機能があると**失敗する**（分類は `docs/scripts/support-pages.mjs` の CATEGORIES）。
// 機能を追加したらそこへ 1 行足すこと（分類漏れを検出するための仕掛けで、放置すると表から機能が消える）。

import { execFileSync } from 'node:child_process';
import { readFileSync, writeFileSync, existsSync } from 'node:fs';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

import { resolveTakoBin } from './lib/tako-bin.mjs';
import {
  WINDOWS_DATA,
  WINDOWS_PAGE,
  canonicalJson,
  renderWindowsSupport,
  serialize,
  windowsCounts,
  windowsSnapshot,
  writeIfChanged,
} from '../docs/scripts/support-pages.mjs';

const REPO = resolve(dirname(fileURLToPath(import.meta.url)), '..');

function matrix(platform) {
  const raw = execFileSync(resolveTakoBin(REPO).path, ['platform', '--platform', platform, '--json'], {
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
  const win = matrix('windows');
  const data = windowsSnapshot(win, 'scripts/gen-windows-support-docs.mjs');
  // 写しは件数を持たない（ページが数える）。数え方が本体の集計とずれたら、写しが正しくても
  // ページの数字が嘘になるので、ここで突き合わせて落とす
  const { counts, total } = windowsCounts(data);
  if (canonicalJson(counts) !== canonicalJson(win.counts) || total !== win.total) {
    throw new Error(
      'ページが数える件数が `tako platform --json` の集計と一致しません' +
        `（数えた値 ${JSON.stringify({ counts, total })} / 本体 ${JSON.stringify({ counts: win.counts, total: win.total })}）。\n` +
        'docs/scripts/support-pages.mjs の windowsCounts を本体の集計に合わせてください',
    );
  }
  return { body: serialize(data), page: renderWindowsSupport(data) };
}

function main() {
  const { body, page } = snapshot();
  if (process.argv.includes('--check')) {
    const current = existsSync(WINDOWS_DATA) ? readFileSync(WINDOWS_DATA, 'utf8') : '';
    if (current !== body) {
      console.error(
        'docs/src/data/windows-support.json（Windows 対応状況ページのデータ）が対応マトリクスと同期していません。\n' +
          '`node scripts/gen-windows-support-docs.mjs` で再生成してコミットしてください。',
      );
      process.exit(1);
    }
    console.log('windows-support.json は対応マトリクスと同期しています');
  } else {
    writeFileSync(WINDOWS_DATA, body);
    // 手元のページも組み直す（コミットしない。docs のビルドでも同じものが組まれる）
    writeIfChanged(WINDOWS_PAGE, page);
    console.log('生成しました: docs/src/data/windows-support.json（ページは docs のビルドが組む）');
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
