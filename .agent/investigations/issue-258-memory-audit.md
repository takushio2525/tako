# Issue #258 メモリ監査レポート

調査日: 2026-07-15
対象: v0.5.2（`8d80be3`）
結論: 60GB 肥大の主因は、PDF の倍率・幅世代ごとのデコード済み画像を GPUI の
プロセス全体アセットキャッシュから除去していなかったこと。ライブリロードの
同時ラスタライズと、バックグラウンド退避後も画像を保持する挙動がピークを増幅する。

## 調査条件

- macOS 26.2 / Apple Silicon / 物理メモリ 16GB
- `TAKO_ISOLATED=1`、専用 discovery / data / tmux socket、`TAKO_PERSIST=0`
- 960×600 の自前 debug build、device scale 2.0
- 612×792 pt、6ページの合成 PDF
- `TAKO_PERF_VERBOSE=1`、`ps` RSS、`footprint`、`heap`、`leaks` を併用
- 本番アプリと本番 tmux セッションは計測対象に含めない

## 主因の実測

6ページ PDF のズームを順に変え、各ラスタライズ完了後の physical footprint を採取した。

| zoom | 1ページの pixel size | RSS（KiB） | physical footprint（bytes） |
|---:|---:|---:|---:|
| 150% | 1344×1740 | 257,392 | 616,793,480 |
| 200% | 1792×2320 | 337,056 | 854,099,480 |
| 250% | 2240×2899 | 523,872 | 1,072,973,504 |
| 300% | 2688×3479 | 833,776 | 1,563,920,448 |
| 350% | 3136×4059 | 815,824 | 2,254,178,872 |
| 400% | 3584×4639 | 930,464 | 2,633,223,000 |

詳細採取時の steady footprint は 2,448,526,096 bytes で、主要カテゴリは次のとおり。

- `MALLOC_LARGE`: 1,300,791,296 bytes
- `IOAccelerator (graphics)`: 1,080,623,104 bytes
- `leaks`: 48 bytes だけ

したがって、解放不能な malloc リークではなく、CPU 側の BGRA と GPU テクスチャが
到達可能なキャッシュとして保持されている。6つの倍率世代の理論 BGRA 合計は
1.156GiB、CPU + GPU は 2.312GiBで、実測増分約 2.0GiB と整合する。

71ページへ線形換算すると同じ6世代で CPU + GPU 約 27.35GiB。4096px 幅では
1世代だけで CPU + GPU 約 11.49GiB となるため、高解像度側の異なる数世代で
60GB を説明できる。

## 所有経路

1. `preview::rasterize_pdf` が全ページ PNG を生成する。
2. `PreviewImageCache` が各 PNG を `gpui::Image::from_bytes` へ渡す。
3. `gpui::img` は GPUI の `App::fetch_asset` を使い、デコード済み `RenderImage` を
   `loading_assets` に保持する。
4. tako は `PreviewImageCache` の `HashMap` エントリを置換・削除していたが、
   `gpui::Image::remove_asset` を呼んでいなかった。
5. GPUI のアセットタスクと sprite atlas が旧 `RenderImage` / texture を保持し続ける。

注意: tako の `preview_image_cache: HashMap<PaneId, PreviewImageCache>` 自体はペインごとに
1世代で、倍率キーを複数保持していない。無制限だったのはその下の GPUI 全体キャッシュ。

## 容疑 1〜7 の判定

### 1. PreviewImageCache の無制限肥大: 主因

上記のとおり、旧 `gpui::Image` のアセット除去がなく、倍率・表示幅・ライブリロードで
内容ハッシュが変わるたび、デコード済み CPU 画像と GPU texture が永続保持された。

### 2. ライブリロードの世代管理: ピーク増幅要因

300ms デバウンス後は同じパスの実行中ジョブを管理していない。400ms 間隔の `touch` 8回で
同一 3584×4639×6ページのラスタライズが7回完走した。RSS は 87,760KiB から
最大 808,656KiB、CPU は最大 274.7%、physical footprint peak は 2,932,427,992 bytes。
古い完成結果は世代照合で表示には適用されず最終的に drop されるため恒久リークではないが、
ジョブ数が入力頻度に比例する。パス単位の single-flight が必要。

### 3. プレビューのバックグラウンド退避: 保持期間の増幅要因

退避前後の physical footprint は 2,188,413,592 → 2,188,397,208 bytes で不変。
さらにペインを復帰後 close しても 2,205,567,664 → 2,210,171,568 bytes で解放されなかった。
退避中も `PreviewState` と描画キャッシュを保持する設計に加え、close 後も GPUI アセットが
残るため、利用者が見えない PDF が予算を占有し続ける。

### 4. ターミナルグリッド / scrollback / alt screen: 白

- 直接 PTY と tmux の history 上限はともに 10,000行。
- 200,000行を2回出力しても history は 10,000で飽和し、2回目の前後で
  physical footprint は 2,205,518,512 bytes のまま不変。
- alt screen は表示グリッド寸法内で、履歴へ蓄積しない。

### 5. sessions / pane logs: 白

- sessions はメタデータだけを最大500件保持し、会話本文は transcript を参照する。
- pane log の RAM 常駐はペインごとの末尾アンカー8行。本文はディスクへ逐次追記する。
- 既定上限はペイン5MB、全体200MB、ローテーション1世代。200,000行出力後の
  隔離ログは 5,275 bytes だった。

### 6. worker_status / watch / MCP: 60GB の原因ではない

- `worker_status` の events は応答ごとに生成する一時 `Vec` で、履歴を保持しない。
- `watch` の画面出力は末尾行だけを都度取得する。
- `run` レジストリは `run_result` 回収まで完了エントリを保持し、件数上限がない。
  1件は小さいため今回の GB 級主因ではないが、未回収クライアントに対する上限を設ける。
- perf verbose samples は10秒ごとに drain され、watchdog 不在時も100,000件で縮退する。

### 7. GPUI レイヤ: 主因の保持先

`leaks` では実リークがほぼない一方、`heap` の non-object 約 1.06GB と
`IOAccelerator` 約 1.08GB が対になった。GPUI の asset cache と sprite atlas へ
明示的な除去を行わない限り、tako 側の `Arc` / `HashMap` を drop しても減らない。

## 修正方針

- プレビュー画像をデコード後バイト数で会計する、設定可能なバイト予算つき LRU にする。
- 既定予算は 512MiB。単一 PDF が超える場合も可視ページを優先し、予算を超えた旧画像は
  `remove_asset` と atlas drop まで行う。
- PDF は全ページを一括 `gpui::img` 化せず、表示近傍だけをデコード対象にする。
- バックグラウンド退避を LRU の低優先度へ落とし、close は即時除去する。
- ライブリロードをパス単位 single-flight + 最新要求1件へ直列化する。
- 未回収 `run` レジストリとペイン別補助キャッシュにも件数上限 / close cleanup を加える。
