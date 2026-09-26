// e2e の spec が共有する 2 つの口（#1749）。
//
// 1. `evidencePath(name)` — スクショの置き場。既定は Playwright の outputDir
//    （`test-results/<テストごとの dir>/`。run の開始時に Playwright が消す）。
//    以前は spec ごとに `~/Desktop/…` / `~/dev/tako-evidence/…` を組み立てていて、
//    `npm run e2e` を回すだけでユーザーのホームへ PNG が 80 枚書かれた。
//    証拠をリポの外へ残したいときだけ `TAKO_EVIDENCE_DIR` を明示で渡す
//    （渡した人が置き場を決める。spec の側でホームを組み立てない）。
//    番犬: `crates/tako-control/tests/issue1749_pwa_e2e_output_watchdog.rs`
// 2. `TAKO_VERSION` — モックの `/api/me` / `/api/health` が返す版。PWA に埋め込まれる版
//    （vite.config.js の `__TAKO_VERSION__`）と同じ読み方なので、版を上げても
//    「アプリの表示が古い可能性があります」のバナーがモックの画面に出ない
import { join } from 'node:path';
import { test } from '@playwright/test';
import { workspaceVersion } from '../workspace-version.js';

export const TAKO_VERSION = workspaceVersion();

export function evidencePath(name) {
  const dir = process.env.TAKO_EVIDENCE_DIR;
  return dir ? join(dir, name) : test.info().outputPath(name);
}
