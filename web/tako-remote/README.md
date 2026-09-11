# tako-remote（PWA）

スマホから tako の会話・ペイン・ファイルを操作する Web クライアント（Preact + Vite）。
ビルド成果物 `dist/` は `rust_embed` で tako 本体へコンパイル時に埋め込まれ、`tako remote` が
配信する。配信経路・認証・脅威モデルは `.agent/plans/tako-remote-plan.md` と
`.agent/threat-model-remote.md` を参照。

## 開発

```
npm ci
npm run dev
```

Vite の dev サーバーが 5174 で上がる（`vite.config.js` でポートを固定している）。
API は tako 本体（`tako remote`）が返すので、ブラウザから実データを見るには本体を別に
起動しておく。**e2e は本体を必要としない**（後述のとおり API を `page.route` でモックする）。

## ビルド

```
npm run build
```

`dist/` を作り直す。**普段は手で叩かなくてよい**: `crates/tako-control/build.rs` が
`dist/index.html` の有無を見て同じ手順を自動で走らせる（#1309）。ツリーを跨いで
作り直したいときはリポジトリルートの `scripts/build-pwa.sh` を使う。

## e2e（Playwright）

```
npm run e2e:install   # 初回だけ（chromium を取得）
npm run e2e
```

`e2e/` の 6 spec（50 項目）が走る。API はすべて `page.route` でモックするので、
**tako 本体も claude も実機のエージェントも要らない**（dev サーバーだけで完結する）。
`playwright.config.js` が `headless: true` 固定なので**画面に窓は出ない**。
dev サーバーは Playwright が自分で起こして終了時に落とす。

絞り込み:

```
npm run e2e -- e2e/panes-621.spec.js          # 1 spec だけ
npm run e2e -- -g "承認カード"                 # 項目名で絞る
```

### `TAKO_PWA_PORT`

e2e が使う dev サーバーのポート（既定 5174 = `npm run dev` と同じ）。Playwright の
`webServer` は `reuseExistingServer: true` なので、**そのポートで既に何かが listen して
いれば、中身を検証せずにそれを dev サーバーとみなして再利用する**。自分で開けた
`npm run dev` を使い回せるのはこの設定のおかげだが、裏返しに、別 worktree の残骸や
無関係なサーバーが 5174 を掴んでいると**その中身に対して検査が走る**。失敗は
ポートの衝突には見えず、「セレクタが見つからない」形で出る（実測: 無関係な HTTP
サーバーに当てると `page.waitForSelector: Timeout 10000ms exceeded`。#621 の検証で遭遇）。

並行して回すとき・失敗が怪しいときは空きポートを渡す:

```
TAKO_PWA_PORT=5199 npm run e2e
```

塞がっているかどうかは `lsof -nP -iTCP:5174 -sTCP:LISTEN` が空かで分かる。
Playwright 自身が起こす場合は `--strictPort` 付きなので、**黙って別ポートへ逃げることはない**。

### chromium が無いとき

`@playwright/test` のバージョンが要求するビルドがローカルキャッシュに無いと、
全項目が次の形で落ちる（#1357 / #632 の実測）:

```
browserType.launch: Executable doesn't exist at .../chromium_headless_shell-<番号>/...
```

`npm run e2e:install` で復旧する（実測: 空キャッシュから 14 秒 → 50 項目 PASS）。
Playwright を上げたときも同じコマンドで追従する。

### スクリーンショットの出力先

spec はカンプ比較用に PNG を撮る（PASS / FAIL には影響しない）。

- `panes-621` / `remote-link-1077` / `master-launch-1078` / `ssh-1080` —
  `TAKO_EVIDENCE_DIR`（既定 `~/dev/tako-evidence/<Issue 番号>/`）
- `screenshots` / `screenshots-5b` — `~/Desktop/tako-284-evidence/` /
  `~/Desktop/tako-285-evidence/` 固定（`TAKO_EVIDENCE_DIR` を見ない）

失敗時のトレース・エラー文脈は `test-results/`（`.gitignore` 対象）に残る。

### CI

`.github/workflows/ci.yml` の macOS ジョブ末尾で `npm run e2e:install` →
`npm run e2e` が **blocking** で走る（#1357）。PWA の実装契約が変わって spec が
取り残されたら、そこで落ちる。追加の所要は実測 46〜87 秒（`50 passed` が 33 秒〜1.2 分。
Playwright の既定 worker 数がランナーの CPU 数に従うので run ごとに幅が出る）。
