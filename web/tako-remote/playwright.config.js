import { defineConfig } from '@playwright/test';

// 検証用 dev サーバーのポート。worktree を並行で回すと別 worktree の残骸が
// 5174 を掴んだままになり、reuseExistingServer が**別リポの中身**を配ってしまう
// （#621 の検証で遭遇）。`TAKO_PWA_PORT` で衝突を避けられるようにしておく
const PORT = Number(process.env.TAKO_PWA_PORT || 5174);

export default defineConfig({
  testDir: './e2e',
  timeout: 30000,
  use: {
    headless: true,
    screenshot: 'off',
    baseURL: `http://localhost:${PORT}`,
  },
  webServer: {
    command: `npm run dev -- --port ${PORT} --strictPort`,
    port: PORT,
    reuseExistingServer: true,
    timeout: 15000,
  },
  projects: [
    {
      name: 'chromium',
      use: { browserName: 'chromium' },
    },
  ],
});
