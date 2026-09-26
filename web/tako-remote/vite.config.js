import { defineConfig } from 'vite';
import preact from '@preact/preset-vite';
// 版の正はルート Cargo.toml。読み方は e2e のモックと 1 実装（#283 / #1749）
import { workspaceVersion } from './workspace-version.js';

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
