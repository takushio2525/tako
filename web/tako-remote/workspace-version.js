import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';

// PWA は daemon（Rust バイナリ）に埋め込まれて配信されるため、バージョンの正は
// Cargo workspace version（#283: /api/me の version と突き合わせて SW キャッシュの
// 古いシェルを検出する）。package.json ではなくルート Cargo.toml から読む。
// vite.config.js（埋め込む側）と e2e のモック（#1749: /api/me が返す側）の両方が
// ここを通るので、版を上げても「表示が古い」バナーがモックの画面に出ない
export function workspaceVersion() {
  const cargoToml = readFileSync(
    fileURLToPath(new URL('../../Cargo.toml', import.meta.url)),
    'utf-8'
  );
  const m = cargoToml.match(/^version\s*=\s*"([^"]+)"/m);
  return m ? m[1] : 'dev';
}
