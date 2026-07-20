import { defineConfig } from 'vite';
import preact from '@preact/preset-vite';
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';

// PWA は daemon（Rust バイナリ）に埋め込まれて配信されるため、バージョンの正は
// Cargo workspace version（#283: /api/me の version と突き合わせて SW キャッシュの
// 古いシェルを検出する）。package.json ではなくルート Cargo.toml から読む
function workspaceVersion() {
  const cargoToml = readFileSync(
    fileURLToPath(new URL('../../Cargo.toml', import.meta.url)),
    'utf-8'
  );
  const m = cargoToml.match(/^version\s*=\s*"([^"]+)"/m);
  return m ? m[1] : 'dev';
}

export default defineConfig({
  plugins: [preact()],
  define: {
    __TAKO_VERSION__: JSON.stringify(workspaceVersion()),
  },
  server: {
    port: 5174,
    host: true,
  },
  build: {
    outDir: 'dist',
  },
});
