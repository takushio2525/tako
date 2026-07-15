# Issue #258 メモリ安定性検証レポート

検証日: 2026-07-15
対象: `fix/258-memory-audit`（#257 取り込み済み）

## 結論

PDF の倍率変更、ページ移動、ライブリロードを繰り返す30分相当シナリオで、
physical footprint の peak は途中から増加せず、終了後のアイドル RSS は84,816KiB、
プレビュー close 後は68,672KiBまで回収された。旧 v0.5.2 は少数回の操作だけで
physical footprint が2.93GBへ増えたのに対し、修正後は120回のファイル変更を含む
30サイクルで795MBを上限に安定した。

## 条件

- `TAKO_ISOLATED=1`、専用 discovery / data / tmux socket、`TAKO_PERSIST=0`
- 612×792pt、6ページの合成 PDF
- 1サイクルを「400%へズーム、ページ移動、400ms間隔のファイル変更4回、100%へ復帰」
  とし、1分間の高負荷操作に相当するブロックを待ち時間を圧縮して30回実行
- 同じ debug プロセスを全サイクルで継続使用し、各サイクル末尾で `ps` RSS、
  5サイクルごとに `footprint`、最後に `leaks` と `tako preview-cache` を採取
- 本番アプリと本番 tmux セッションは操作・計測対象に含めない

## 30サイクルの RSS 系列

単位はKiB。

| cycle | 1 | 2 | 3 | 4 | 5 | 6 | 7 | 8 | 9 | 10 |
|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| RSS | 519,504 | 461,632 | 394,256 | 404,560 | 392,000 | 316,512 | 496,256 | 376,544 | 279,760 | 401,360 |

| cycle | 11 | 12 | 13 | 14 | 15 | 16 | 17 | 18 | 19 | 20 |
|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| RSS | 367,024 | 372,160 | 450,240 | 506,096 | 392,304 | 505,040 | 360,432 | 302,032 | 435,248 | 442,224 |

| cycle | 21 | 22 | 23 | 24 | 25 | 26 | 27 | 28 | 29 | 30 |
|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| RSS | 507,888 | 505,648 | 428,192 | 306,528 | 441,488 | 502,336 | 497,088 | 457,568 | 367,776 | 366,528 |

- 最小: 279,760KiB
- 最大: 519,504KiB
- 平均: 418,540.8KiB
- 線形回帰傾き: +699.2KiB / cycle
- 前半15回平均: 408,680.5KiB
- 後半15回平均: 428,401.1KiB（差 +19.3MiB、系列の振れ幅内）

傾きは30サイクルで約20MiBに相当し、最大値は1回目、終了値は開始値より
152,976KiB低い。旧実装のような操作回数に比例する単調増加はない。

## physical footprint と明示解放

`footprint` の current / peak（MB）は次のとおり。

| cycle | 5 | 10 | 15 | 20 | 25 | 30 |
|---:|---:|---:|---:|---:|---:|---:|
| current | 406 | 547 | 479 | 483 | 542 | 411 |
| peak | 783 | 795 | 795 | 795 | 795 | 795 |

peak は10サイクル目から30サイクル目まで795MBで不変だった。全操作後に12秒アイドルすると
RSS 84,816KiB、footprint 137MB、LRU は8,314,880 bytes / 2 entriesとなった。
プレビューを close すると RSS 68,672KiB、footprint 129MB、LRU 0 bytes / 0 entriesまで
減少した。`leaks` はObjective-Cオブジェクト3件、合計112 bytesだけだった。

比較対象の v0.5.2 は、ライブリロード8回だけで RSS 808,656KiB、physical footprint peak
2,932,427,992 bytesまで増加した。修正後は120回の変更を含む30サイクルでも795MBであり、
GPUI asset / atlas eviction とバイト予算が効いている。

## #257 取り込み後の追加確認

#257 のファイルスタンプ比較とダブルバッファ化を取り込んだ現行 HEAD でも、`touch` ではなく
PDF の実データを書き戻す同じシナリオを21サイクル追加実行した。

- RSS: 最小98,384KiB、最大535,184KiB、平均331,038.5KiB
- 線形回帰傾き: -4,266.1KiB / cycle
- footprint: cycle 5 / 10 / 15 / 20 の current は348 / 419 / 355 / 424MB
- footprint peak: cycle 5以降812MBで不変
- 有効区間の perf 集計: `preview_watch_event` 89件、`preview_reload_apply` 44件、
  `pdf_rasterize` 89件、`render` 298回

この追加プロセスは21サイクル後、インストール済みアプリの起動と同時刻に終了コード0で
終了した。DiagnosticReport、panic、signal、OSメモリ強制終了の痕跡はないため、異常終了とは
扱わない。一方、原因を外部起動と断定せず、30サイクル受け入れ値には完走した先行系列を使う。

## perf_span 非退行

30サイクル区間の `render` は p50 1ms、rolling-window p95 / p99 最大7ms、最大16msだった。
旧 v0.5.2 の同条件は p50 1ms、p95最大6ms、最大13msであり、中央値は同じ、最大値も
1フレーム16msの範囲内である。`preview_watch_event` は旧実装の最大61msに対して修正後0ms。

#257 取り込み後の追加区間も `render` rolling-window p50最大2ms、p95 / p99最大6ms、
最大15msだった。LRU accounting と eviction はrenderごとの全件走査を行わず、保留された
除去対象だけをrender冒頭で処理するため、定常描画の退行は見られない。

PDFの全ページラスタライズ自体はbackgroundで74〜1,617msを要するが、render専有時間には
含まれず、UIフレームを停止させていない。

## 判定

- RSS / physical footprint: 安定
- 既定512MiBのバイト予算 + LRU eviction: 動作確認
- close時のCPU asset / GPU atlas解放: 動作確認
- ライブリロードsingle-flight: 4イベントに対し概ね2適用まで集約
- perf_span: 非退行

## 品質ゲート

- `cargo build --workspace`: 成功
- `cargo fmt --all --check`: 成功
- `cargo clippy --workspace --all-targets -- -D warnings`: 成功
- `cargo test --workspace`: 成功
  - tako-app: 91 passed / 2 ignored
  - tako-cli: 25 passed
  - tako-control: 424 passed / 1 ignored
  - tako-core: 276 passed
- `TAKO_ISOLATED=1 TAKO_SELF_TEST=1 target/debug/tako-app`: 終了コード0、
  `TAKO_APP_SELF_TEST_OK`
