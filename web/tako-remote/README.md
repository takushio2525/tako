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

`e2e/` の spec がすべて走る（件数は spec を足すたびに増えるので書かない。末尾の
`N passed` が正）。API はすべて `page.route` でモックするので、
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

`npm run e2e:install` で復旧する（#1357 時点の実測: 空キャッシュから 14 秒 → 50 項目 PASS）。
Playwright を上げたときも同じコマンドで追従する。

### スクリーンショットの出力先

spec はカンプ比較用に PNG を撮る（PASS / FAIL には影響しない）。置き場は
`e2e/support.js` の `evidencePath('<名前>.png')` の 1 実装で決まり、**既定は Playwright の
outputDir**（`test-results/<テストごとの dir>/`。`.gitignore` 対象で、run の開始時に
Playwright が消す）。失敗時のトレース・エラー文脈も同じ dir に残る。

リポの外へ残したいときだけ、実行する人が置き場を渡す（spec の側でホームを組み立てない）:

```
TAKO_EVIDENCE_DIR=/tmp/pwa-shots npm run e2e -- e2e/panes-621.spec.js
```

#1749 までは spec ごとに `~/Desktop/tako-28{4,5}-evidence/` や `~/dev/tako-evidence/<番号>/`
を組み立てていて、`npm run e2e` を回すだけでホームへ PNG が 80 枚書かれていた。
`crates/tako-control/tests/issue1749_pwa_e2e_output_watchdog.rs` が、spec へホームの
組み立て（`process.env.HOME` / `homedir()` / `Desktop`）・`evidencePath(` を通らない
スクショ・版の直書きが戻ると file:line を名指しして落とす。

ホームへ何も書かないことは一時 `HOME` で確かめられる（ブラウザと npm のキャッシュは
一時 `HOME` の外を向けておく。向けないと chromium が見つからず全項目が落ちる）:

```
H=$(mktemp -d); B="$HOME/Library/Caches/ms-playwright"   # B は macOS の既定の置き場
HOME="$H" PLAYWRIGHT_BROWSERS_PATH="$B" npm_config_cache="$(mktemp -d)" npm run e2e
find "$H" -mindepth 1   # 何も出なければよい
```

### モックが返す版

PWA は `/api/me` の版とビルドへ埋め込んだ版（`__TAKO_VERSION__`）が違うと「アプリの表示が
古い可能性があります」のバナーを出す。モックの版は `e2e/support.js` の `TAKO_VERSION` を
使う（vite と同じ `workspace-version.js` でルートの `Cargo.toml` から読む）ので、版を
上げてもモックの画面にバナーは写らない。数字を spec へ直書きしない。

### CI

`.github/workflows/ci.yml` の macOS ジョブ末尾で `npm run e2e:install` →
`npm run e2e` が **blocking** で走る（#1357）。PWA の実装契約が変わって spec が
取り残されたら、そこで落ちる。追加の所要は #1357 時点の実測で 46〜87 秒（`50 passed` が 33 秒〜1.2 分。
Playwright の既定 worker 数がランナーの CPU 数に従うので run ごとに幅が出る）。
