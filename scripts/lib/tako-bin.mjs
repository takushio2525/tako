// docs 生成が使う tako CLI を 1 か所で選ぶ（Issue #1548）。
//
// **なぜ版を突き合わせるのか**: 以前は `target/debug/tako` → `target/release/tako` の
// 順に「存在する方」を返すだけで、版もタイムスタンプも見なかった。開発ツリーの
// `target/debug/tako` は最後に `cargo build` した時点で止まるので、release だけ新しい
// 状態（実測: debug v0.8.13 / release v0.8.17）では
//   - `--check` が「マトリクスと同期していません」の**偽の赤**を出す
//   - `--check` 無しで打つと docs が数日前の内容へ**静かに巻き戻る**
// という形で嘘をつく。CI は毎回フレッシュビルドなので緑のまま、**手元だけが壊れる**。
// そこで「選んだバイナリの版 = Cargo.toml の `[workspace.package] version`」を
// 生成の前提条件として検査し、違えば理由つきで落とす。
//
// 使い方（既定で済む形 = #322。引数もオプションも足さない）:
//   import { resolveTakoBin } from './lib/tako-bin.mjs';
//   const bin = resolveTakoBin(REPO);          // → { path, version, source }
//
// 別の場所のバイナリを使いたいときだけ環境変数で明示する（リポジトリの他の
// 検証スクリプトと同じ `TAKO_BIN`。相対パスはリポジトリルート基準）:
//   TAKO_BIN=target/release/tako node scripts/gen-windows-support-docs.mjs

import { execFileSync } from 'node:child_process';
import { readFileSync, statSync } from 'node:fs';
import { isAbsolute, join, relative, resolve } from 'node:path';

/** 既定の探索順。Windows は拡張子が付く */
const CANDIDATES = process.platform === 'win32'
  ? ['target/debug/tako.exe', 'target/release/tako.exe']
  : ['target/debug/tako', 'target/release/tako'];

/** リポジトリルートからの相対パス（メッセージ用。実ホームパスを出さない = #927） */
function rel(repo, path) {
  const r = relative(repo, path);
  return r && !r.startsWith('..') ? r : path;
}

function isFile(path) {
  try {
    return statSync(path).isFile();
  } catch {
    return false;
  }
}

/**
 * `Cargo.toml` の `[workspace.package] version` を読む。
 *
 * 節を切り出してから拾う（`[package]` など他の節の `version` を掴まないため）。
 */
export function workspaceVersion(repo) {
  const path = join(repo, 'Cargo.toml');
  let toml;
  try {
    toml = readFileSync(path, 'utf8');
  } catch {
    throw new Error(`Cargo.toml を読めません: ${rel(repo, path)}`);
  }
  let inSection = false;
  for (const line of toml.split('\n')) {
    const header = line.match(/^\s*\[([^\]]+)\]/);
    if (header) {
      inSection = header[1].trim() === 'workspace.package';
      continue;
    }
    if (!inSection) continue;
    const m = line.match(/^\s*version\s*=\s*"([^"]+)"/);
    if (m) return m[1];
  }
  throw new Error('Cargo.toml に [workspace.package] の version が見つかりません');
}

/** バイナリを実際に起こして版を読む（ファイル名や mtime では判定しない） */
export function binaryVersion(bin, repo) {
  let raw;
  try {
    raw = execFileSync(bin, ['--version'], { encoding: 'utf8', timeout: 30_000 });
  } catch (e) {
    throw new Error(`${rel(repo, bin)} --version を実行できません: ${e.message.split('\n')[0]}`);
  }
  const first = raw.trim().split('\n')[0] ?? '';
  const m = first.match(/(\d+\.\d+\.\d+[^\s]*)/);
  if (!m) throw new Error(`${rel(repo, bin)} --version の出力から版を読めません: ${first}`);
  return m[1];
}

/**
 * 生成に使う tako CLI を決める。**版が合わなければ例外**（黙って古いものを使わない）。
 *
 * @param {string} repo リポジトリルートの絶対パス
 * @returns {{ path: string, version: string, source: 'TAKO_BIN' | 'default' }}
 */
export function resolveTakoBin(repo, env = process.env) {
  const explicit = (env.TAKO_BIN ?? '').trim();
  let path;
  let source;
  if (explicit) {
    path = isAbsolute(explicit) ? explicit : resolve(repo, explicit);
    source = 'TAKO_BIN';
    if (!isFile(path)) {
      throw new Error(`TAKO_BIN が指す tako が見つかりません: ${explicit}`);
    }
  } else {
    path = CANDIDATES.map((c) => join(repo, c)).find(isFile);
    source = 'default';
    if (!path) {
      throw new Error('tako CLI が見つかりません。`cargo build -p tako-cli` を先に実行してください');
    }
  }

  const want = workspaceVersion(repo);
  const got = binaryVersion(path, repo);
  if (got !== want) {
    // 1 行目だけで理由が読めるようにする（偽の赤・静かな巻き戻しの代わりに出る）
    const hint = source === 'default'
      ? '\n別の場所のバイナリを使うなら TAKO_BIN=<path> で明示できます'
      : '\n（TAKO_BIN で明示されたパスです）';
    throw new Error(
      `${rel(repo, path)} が古いバイナリです（v${got} ≠ Cargo.toml の v${want}）。`
        + '`cargo build -p tako-cli` を実行してから再実行してください'
        + hint,
    );
  }
  return { path, version: got, source };
}
