# tako プロモーション動画（リメイク版）

> 95 秒のプロモーション動画の台本・根拠・作り方・検査の正本。
> 既存の動画（#470 のティザー / #1081 の解説 / #1284 の X 向けショート）の**カット・構成・音は使っていない**。
> ソースは `scripts/promo/remake/`、成果物（mp4・収録フレーム）はリポジトリの外 `~/Movies/tako-promo/`（コミットしない）。

## 既存の動画と、変えたこと

既存の動画は `~/Desktop/tako-promo/` にある（`tako-intro-v3.mp4` 106 秒 / `tako-explainer-v6.mp4` 8 分 29 秒 /
`tako-x-short-v1.mp4` 2 分 58 秒）。見直して足りなかったのは次の 4 点。

- **「他と何が違うか」を正面から言う場面が無い**。機能を順に紹介するだけで、選ぶ理由が残らない
- 全編が暗い実画面 + 下段の帯の字幕で、**どこを見ればよいかが文字頼み**。寄りも指し示しも無く、実画面の文字が小さい
- 実物の claude の画面をそのまま使ったので、権限の確認や英語の長い出力など**伝えたいこと以外が画面を占める**
- 音が映像の切り替わりと無関係に流れる（汎用のループ）。ショートは 2 分 58 秒で、X の通常アカウントの上限（2 分 20 秒）を超える

| | 既存 | リメイク |
|---|---|---|
| 構成 | 機能の紹介順（課題 → 思想 → 導入 → 操作 → …） | **課題 → 3 つの違い → 比較表 → 立ち位置 → 入手**（95 秒） |
| 映像 | 暗い実画面を全面に + 下段の帯字幕 / 静止スライド | 明るい紙色の地に実画面の窓。**寄り・吹き出し・図解の動き**を時刻から描く。実画面は全カット撮り下ろし |
| 画面の中身 | 実物の claude（収録のたびに内容が変わる） | **デモ用の台本**（偽のエージェント）。tako の側は本物の経路のまま |
| 音 | 長尺の汎用 BGM（+ 合成音声のナレーション） | **96 BPM の新曲**を小節単位で場面に合わせて合成 + 効果音。ナレーション無し |

長さを 95 秒にした理由: 課題の提示（20 秒）と 3 つの違い（45 秒）と比較・立ち位置・入手（30 秒）を、
字幕を読み切れる速さで収めた下限。X の通常アカウントの上限（140 秒）に収まり、README や docs にも置ける。

音をナレーション無しにした理由: SNS のタイムラインは既定で無音再生なので、意味は字幕と絵だけで運ぶ
（#1284 と同じ判断）。そのうえで 90 秒級のプロモーションでは合成音声の読み上げより、
場面の切り替わりに合った音楽のほうが絵を邪魔しない。**耳での確認はしていない**（この環境に音声出力が無い）ので、
音量と帯域は数値で検査している（下の「検査」）。

## 台本（画面に出る文字の全文）

時刻は映像の秒。字幕の正本は `scripts/promo/remake/comp/index.html`（図解の場面）と
`scripts/promo/remake/comp/timeline.json`（実画面の場面）。

| 秒 | 場面 | 画面に出る文字 |
|---|---|---|
| 0–5 | 課題 | AI エージェントに開発を任せると、/ ターミナルが、足りなくなる。（小さな窓が増えていく: エージェント / 子エージェント ×3 / dev サーバー / テスト / ログ） |
| 5–12.5 | 課題 | （窓が散らばる）どのタブで動いている？ / 誰が止まっている？ / 終わったのは、どれ？ / 見て回るだけで、時間が溶けていく。 |
| 12.5–20 | 答え | （窓が 1 つのタブへ集まる）tako / エージェントの群れを、1 つのタブで。/ AI エージェントのための、オープンソースの GUI ターミナル |
| 20–40 | 違い 1 | **1 グループ = 1 タブ。司令塔つき。** / `tako master` と打つと、そのペインが司令塔に / 日本語で頼むと、作業を分けて担当を立てる / 担当は同じタブに並ぶ。全体は右パネルで（吹き出し: 司令塔 / 担当 × 3 / 全体の見取り図）/ 完了を検知して、検収してから報告 |
| 40–55 | 違い 2 | **AI が、画面を動かせる。** / `tako split` でペインを割り、dev サーバーを起動 / `tako open` で、README を隣にプレビュー / テーマの切り替えも。画面の操作は、CLI / MCP からもできる |
| 55–65 | 違い 3 | **閉じても、続いている。** / 作業の途中で、tako を終了する / （窓が消える）tako を終了 — エージェントも dev サーバーも、裏で動いたまま / 開き直すと、続きから。`tmux` の中で動き続けていた（吹き出し: 閉じている間も、進んでいた） |
| 65–75 | くらべると | ふつうのターミナルで回すと → tako なら / タブとウインドウに散らばる → 1 グループ = 1 タブに並ぶ / 誰が止まったか、見て回る → 司令塔が監視して、検収まで / AI に画面を触らせるには、自分で配線 → MCP サーバー内蔵。設定ゼロ / ウインドウを閉じたら、作業も止まる → 開き直せば、続きから / （注記）左は、エージェント向けの仕組みを持たないターミナルを、そのまま使う場合（tmux などを自分で組み合わせない場合）です。 |
| 75–85 | 立ち位置 | tako は、AI を内蔵しません。/ Claude Code をはじめ、手元の CLI エージェントをそのまま束ねるための「器」です。/ 見る・直す道具も、同じタブの中に。（カード: git の差分と履歴 / ファイルツリーとコード / ボタンから始める「かんたん表示」）/ オープンソース（GPL-3.0）・Rust + GPUI・アカウント登録なし |
| 85–95 | 入手 | tako / エージェントの群れを、1 つのタブで。/ `brew install --cask takushio2525/tako/tako` / github.com/takushio2525/tako ・ tako.takushio2525.com / macOS（Apple Silicon）で使えます。Windows 版は対応を進めています。/ この動画の画面は実際の tako です。エージェントの出力は、デモ用の台本を流しています。 |

実画面の場面には、右上に小さく「画面は実際の tako。エージェントの出力はデモ用の台本です」を出し続ける。

## 主張ごとの根拠

tako の機能として語るものは、今の main に実在し、**収録の中で実際に動かしたもの**だけにした。

| 主張 | 根拠（コマンド・画面・文書） |
|---|---|
| エージェントで開発すると 1 つの作業が複数の窓へ散らばる | `README.md`「なぜ tako？」 |
| `tako master` でそのペインが司令塔になる | 収録 `fleet`: ペインへ `tako master` を打つ（本物の CLI。起動されるエージェントだけが偽物） |
| 作業を分けて担当を立てる・担当は同じタブに並ぶ | 収録 `fleet`: 司令塔の台本が本物の `tako orchestrator spawn --project shop-app --label … --prompt …` を 3 回打つ。応答は `placement: same_tab`。指示の送達も本物の経路（入力欄の検出 → 貼り付け → Enter） |
| 全体は右パネルで | 収録 `fleet`: `tako panel --show --view orch`（司令塔と担当のツリーは本物の台帳から描かれる） |
| 完了を検知 | 収録 `fleet`: 司令塔の台本が本物の `tako orchestrator status --pane N` を見に行き、`status` が `idle` になったら次へ進む |
| 検収してから報告 | 本物の司令塔（LLM）の作法: `docs/src/content/docs/features/orchestration.mdx`「品質は仕組みで作り込まれる」/ 受け入れゲート `tako task gate`。**動画の中の検収の文面は台本**（LLM を呼んでいないため） |
| AI が画面を動かせる（分割・プレビュー・テーマ） | 収録 `drive`: エージェントのペインの中のプロセスが本物の `tako split --down -- …` / `tako open README.md --right` / `tako theme light` を打つ |
| 画面の操作は CLI / MCP からもできる | `AGENTS.md`「機能実装時の必須ルール」設計原則 5。**「すべて」とは言わない**（リモートの機器承認など、意図して人だけに残した操作がある = `.agent/concept.md`） |
| MCP サーバー内蔵。設定ゼロ | `README.md`「特徴」（設定ゼロで使える内蔵 MCP サーバー） |
| 閉じても続いている | 収録 `restore`: tmux バックエンド（`TAKO_PERSIST=1`・隔離ソケット）で 3 ペインを動かし、アプリだけを終了 → 5 秒後に起動 → 同じレイアウトで、テストの回数と編集の進みが先へ進んでいる。`README.md`「セッション永続化」 |
| 比較表の左（ふつうのターミナル） | 製品名を挙げない一般論。条件は注記に書いた（エージェント向けの仕組みを持たないターミナルを、tmux などを組み合わせずに使う場合） |
| AI を内蔵しない / アカウント登録なし | `.agent/concept.md`「やらないこと」（独自 AI 機能の内蔵・クラウド同期とアカウント機能） |
| Claude Code をはじめ、手元の CLI エージェントを束ねる | `tako agent-support`（Claude Code: supported 67。Codex: supported 38 / degraded 8 / pending 18 / unsupported 3。Antigravity: supported 25 …）。Codex などは一部対応なので「Claude Code をはじめ」と書いた |
| git の差分と履歴 / ファイルツリーとコード / かんたん表示 | 収録 `extras-git`（`tako panel --show --view git`）/ `extras-code`（`tako panel --filetree on` + `tako open`）/ `extras-starter`（`tako ui-mode gui`） |
| オープンソース（GPL-3.0）・Rust + GPUI | `LICENSE` / `README.md`「特徴」 |
| `brew install --cask takushio2525/tako/tako`・macOS（Apple Silicon）・Windows は対応中 | `README.md`「インストール」（Windows は「移植は途中」）。**「対応済み」とは言っていない** |

### 近い道具との関係（動画では製品名を挙げていない）

エージェント向けのターミナルは複数あり、どれも更新が速い（調べた当日も README の内容が古い枝と新しい枝で違った）。
「相手に無い」と言うには相手の全機能を確かめる必要があり、数週間で古くなる。そこで動画では
**一般的な使い方（ふつうのターミナルで回す）との違い**と **tako 自身の立ち位置**だけを語り、製品名は出していない。
確かめた公式の自己紹介は次のとおり（2026-11-10 取得）。

| 道具 | 公式の説明（原文） | 出典 |
|---|---|---|
| iTerm2 | "iTerm2 is a replacement for Terminal and the successor to iTerm." | https://iterm2.com/ |
| Ghostty | "Fast, native, feature-rich terminal emulator pushing modern features." | https://github.com/ghostty-org/ghostty |
| tmux | "tmux is a terminal multiplexer: it enables a number of terminals to be created, accessed, and controlled from a single screen. tmux may be detached from a screen and continue running in the background, then later reattached." | https://github.com/tmux/tmux |
| cmux | "A Ghostty-based macOS terminal with vertical tabs and notifications for AI coding agents" | https://github.com/manaflow-ai/cmux |
| Warp | "Warp is an agentic development environment, born out of the terminal. Use Warp's built-in coding agent, or bring your own CLI agent (Claude Code, Codex, Gemini CLI, and others)." | https://github.com/warpdotdev/warp |
| Wave Terminal | "Wave is an open-source, AI-integrated terminal for macOS, Linux, and Windows." | https://github.com/wavetermdev/waveterm |

cmux は README と LICENSE（公開の文書）だけを読んだ。ソースコードは読んでいない（`.agent/concept.md` の絶対ルール）。

## 作り方

| 段 | もの | コマンド |
|---|---|---|
| 0 | 収録用の読み戻しを入れて建てる | `git apply scripts/promo/remake/promo-rec.patch`（建てるのは `record.sh` が `isolated_gui_bins visual-test` で行う） |
| 1 | 実画面を収録する | `bash scripts/promo/remake/record.sh`（場面を選ぶなら `record.sh fleet` など） |
| 2 | 本体を戻す | `git checkout crates/tako-app/src/main.rs` |
| 3 | 書き出す（音 → 映像 → 合わせる → 検査） | `bash scripts/promo/remake/build.sh` → `~/Movies/tako-promo/tako-promo-remake-v1.mp4` |
| 確認 | 指定した秒の 1 枚だけ描く | `node scripts/promo/remake/render.mjs --stills 21.3,46.5 --out <dir>` |

| ファイル | 役割 |
|---|---|
| `record.sh` | 砂場（デモ用の HOME・プロジェクト・偽のエージェント）→ 隔離 GUI（`scripts/lib/isolated-gui.sh`）→ 場面の操作 → 連番の JPEG と時刻の表 → 片付け |
| `demo/agent-sim.mjs` + `demo/personas/*.json` | 偽のエージェントと台本。`claude` などの名前で PATH の先頭に置く |
| `demo/shop-app/`・`demo/dev-server.mjs`・`demo/test-watch.mjs` | 画面に映すデモ用プロジェクトと、それらしい出力 |
| `comp/index.html`・`comp.css`・`comp.js`・`timeline.json` | 合成。`window.__seek(秒)` でその時刻の 1 枚を決定的に描く |
| `timeline.mjs` | `timeline.json` の「出来事の名前 ± 秒」を収録の時刻で確定させる（映像と音が同じ規則で追従する） |
| `render.mjs` | Playwright の chromium（headless）で 1/30 秒ずつ進めて撮り、ffmpeg へ流す |
| `bgm.mjs` | 音の合成（外部の音源は使わない） |
| `build.sh` | 書き出しと検査 |
| `promo-rec.patch` | 収録専用の読み戻しの差分（**コミットしない本体の差分**。下の節） |

新しい依存は足していない（Node・ffmpeg・`web/tako-remote` の Playwright はどれも既存）。

### 偽のエージェント（本物の claude / codex / agy を起こさない）

収録では `demo/agent-sim.mjs` を `claude` / `codex` / `agy` の名前で PATH の先頭に置く。tako は本物の CLI と同じ手順
（入力欄を見つける → 貼り付け → Enter → 稼働中の表示を見る → 待機に戻るのを見る）で扱うので、
`tako master`・`tako orchestrator spawn`・`tako send`・右パネルの orch ビューは**本物の経路のまま**動く。
差し替わっているのは「考える中身」だけで、そこを台本（`personas/*.json`）が流す。司令塔の台本は
本物の `tako orchestrator spawn` / `status` / `tako panel` を実際に打つ。

tako が見ているもの（偽物が満たしている条件）:

- 入力欄: 画面の下 24 行にある、プロンプト記号（`> `）で始まる行（`crates/tako-core/src/screen.rs` の `input_region_in_lines`）
- 稼働中: 画面に `esc to interrupt` がある間（`crates/tako-control/src/claude_tui.rs` の `is_busy`）

**罠**: `tako orchestrator watch` は偽物では待機を拾わなかった（`status` は `idle` を返すのに 40 秒で時間切れ。
`status` の応答は `has_running_children: true` だった = 偽物のプロセス名が `claude` ではなく `node` なので、
エージェントの子が動いていると見なされたと推測している。本物の claude では確かめていない）。
台本は `status` を見に行く形にしている。

### 画面がロックされていても撮る（読み戻し）

画面がロックされていると、OS の画面収録は窓単体でも断られる（`screencapture -l<窓>` は
`could not create image from window`。画面全体はロック画面が写る）。#1081 はこれで 2 回・計 7.5 時間止まった。

今回は visual-test と同じ読み戻し（`window.render_to_image()` = Metal の最終 scene）で窓の絵を一定間隔で書き出した。
画面収録の権限もロックの解除も要らず、ユーザーの画面には何も出ない。ただし今の main にはその入口が無いので、
**収録専用の差分 `promo-rec.patch`**（`TAKO_PROMO_REC_DIR` があるとき、`<dir>/REC` が在る間だけ生の RGBA を書く。
UI の描画と状態には触らない）を手元の木に当てて建てたバイナリで撮った。差分はコミットしていない。

- 読み戻した絵には OS が描く信号機ボタンが無い（GPUI の scene の外）。合成で同じ位置に描き足している
- debug ビルドで 1 枚およそ 0.3 秒。端末の表示は離散的に変わるので、変化のあったフレームだけを残せば足りる
  （同じ絵は書かない。`fleet` は 31 秒で 71 枚）
- 蓋を閉じた運用では `tako-vd` がすぐ眠る。`record.sh` は収録の間だけ `caffeinate -d -u` で起こしておく
- `promo-rec.patch` は `main.rs` の行に依るので、main が進むと当たらなくなる。そのときは差分の 2 か所
  （`self_test` の中の `run_promo_recorder` と、起動時の呼び出し）を今の `main.rs` へ手で移す

### 個人情報を出さない

- HOME は砂場の `demo`、プロンプトは固定（`demo %1~ %#`）、git の名義は `demo`、プロジェクトは架空の `shop-app`
- 画面に `ls -l`（持ち主の列）や絶対パスを出さない。`tako master` の起動の行は偽物が画面を描き直して消える
- 検査は 2 重: 全フレームの OCR（`scripts/promo/pii-scan.sh`。検査語は環境から組み立てる）と、等間隔のフレームの目視

## 検査（v1・2026-11-10）

| 項目 | 値 |
|---|---|
| 動画 | `~/Movies/tako-promo/tako-promo-remake-v1.mp4`（13.7 MB） |
| 長さ / 形式 | 95.000 秒 / 1920x1080 / 30fps / H.264 + AAC 48kHz stereo（`ffprobe`） |
| デコードの全走査 | `ffmpeg -v error -i <mp4> -f null -` の出力 0 行（映像 2850 枚と音声を最後までデコード） |
| 音量 | -15.1 LUFS / LRA 5.7 LU / True Peak -1.5 dBFS（目標 -15 LUFS。合成後に 1 回測って固定のゲイン +0.40 dB） |
| 帯域（調整前の合成） | 120 Hz 未満 -22.0 dB / 120–500 Hz -21.0 dB / 500 Hz–2 kHz -25.2 dB / 2–6 kHz -36.5 dB / 6 kHz 以上 -39.4 dB。高い帯が少なかったので、はじく音・刻みを上げ和音を下げた |
| 台本との突き合わせ | 画面に出す 58 句のうち 57 句が OCR（1 fps・95 フレーム・4503 行）と一致。残り 1 句（「担当 × 3」）は記号の読み違いで、フレームを目で見て確認した |
| 個人情報の検査（OCR） | email / home_path / tailnet / private_ip / token / uuid は 0 件。環境由来の語は 15 件で、**すべて入手方法の場面の公開情報**（`brew install --cask …` の tap・GitHub の URL・docs のドメイン）。それ以外は 0 件 |
| 目視 | 等間隔の 12 枚（`~/Movies/tako-promo/checks/tako-promo-remake-v1/`）と節目の静止画を 1 枚ずつ見た。字幕のはみ出し・重なり無し、HOME は `~`、プロンプトは `demo`、タブ名とプロジェクトは `shop-app` |
| 収録 | 隔離 GUI は毎回 `tako-vd` の上（窓 1280x720pt = 2560x1440px）。終了時に隔離 GUI・tmux の隔離ソケット・砂場を消している（`record.sh` の「片付け」の行） |

未確認: **音を耳で聴いていない**（数値だけ）。曲の好みや効果音の大きさは試聴して決める必要がある。
直すときは `bgm.mjs` の各楽器の係数を変えて `build.sh` を回し直す（映像は描き直さなくてよいなら 3/4 以降だけ手で回せる）。
