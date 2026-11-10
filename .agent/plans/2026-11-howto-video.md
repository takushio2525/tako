# tako の入れ方・使い方動画（#2866）

> 2 分 50 秒の入れ方・使い方動画の台本・根拠・作り方・検査の正本。
> 見た人がこの動画だけを頼りに、tako を入れて master で仕事を回し始められることが目的。
> ソースは `scripts/promo/howto/`、成果物（mp4・収録フレーム）はリポジトリの外 `~/Movies/tako-promo/`（コミットしない）。
> 合成・描画・音・収録の仕組みはプロモーション動画（#2864）の `scripts/promo/remake/` をそのまま使う
> （仕組みの説明は `.agent/plans/2026-11-promo-remake.md`）。ここに書くのは、この動画の差分だけ。

## 構成と長さ

| 秒 | 章 | 中身 |
|---|---|---|
| 0–7.5 | 題字 | 入れ方と、使い方。/ 1 入れる → 2 整える → 3 任せる |
| 7.5–40 | 1 入れる | Homebrew（12.5 秒）/ zip（10 秒）/ Windows（7.5 秒） |
| 40–100 | 2 整える | 初回起動（15 秒）/ エージェントが無いとき（7.5 秒）/ `tako setup`（15 秒）/ 更新の 3 つの方法（12.5 秒）/ `tako update check`（7.5 秒） |
| 100–157.5 | 3 任せる | `tako master`（45 秒）/ 覚えておくと便利なこと（10 秒） |
| 157.5–170 | まとめ | 3 つのコマンド・リンク・断り |

長さを 170 秒にした理由: ナレーションが無く、手順を字幕で読ませる動画なので、1 つの字幕を 3.5 秒以上は出している。
入れ方は道が 3 本（Homebrew / zip / Windows）あり、起動後は「自動で済むこと」と「自分で打つコマンド」を分けて見せる必要がある。
master の章は、実際の流れ（起動 → プロジェクトの登録 → 依頼 → 担当が並ぶ → 報告）を省かずに通すと 45 秒になる。
どの章も 60 秒以内なので、章ごとに切り出しても使える（X の通常アカウントの上限 140 秒は、全編では超える）。

## プロモーション動画（#2864 の v1）から揃えた点・変えた点

| | 揃えた | 変えた（理由） |
|---|---|---|
| 色・書体・地 | 紙色の地・コーラルの差し色・同じ書体（`comp.css` をそのまま読む） | なし |
| 字幕・テロップ | 左上にチップと見出し、右に 1 行の字幕、実画面の窓・寄り・吹き出し・右上の注記（`comp.js` が描く） | チップを章の番号（「1 入れる」）にした。図解の場面にも同じ位置にチップと見出しを置いた（章の中で実画面と図解を行き来するため） |
| 実画面 | 隔離した実際の tako を読み戻しで収録・エージェントは偽物（`agent-sim.mjs`）・1280x720pt の窓 | フォントの大きさとタブ名を既定のままにした（初めて起動した人が見る画面にそろえるため）。HOME の下を実機と同じ形にした（パスが `~/Library/Application Support/tako/…` と出る） |
| 音 | ナレーション無し・同じ楽器と残響（`bgm.mjs`）・96 BPM・場面の切り替わりは小節の頭・効果音・目標 -15 LUFS | 曲の構成（どの小節で何を鳴らすか）は 170 秒用に書いた（`arrangement.js`）。図解の場面は刻みを軽くした（読ませる場面が多いため） |
| 解像度・fps | 1920x1080・30fps・H.264 + AAC 48kHz stereo | なし |

## 台本（画面に出る文字の全文）

時刻は映像の秒。字幕の正本は `scripts/promo/howto/comp/index.html`（図解の場面）と
`scripts/promo/howto/comp/timeline.json`（実画面の場面）。

| 秒 | 場面 | 画面に出る文字 |
|---|---|---|
| 0–7.5 | 題字 | tako / 入れ方と、使い方。/ 1 入れる（ダウンロード）→ 2 整える（セットアップと更新）→ 3 任せる（master に頼む） |
| 7.5–10 | 章 1 | 1 入れる / ダウンロードとインストール |
| 10–22.5 | Homebrew | 1 入れる / Mac は、Homebrew で 1 行。/ Apple Silicon の Mac（macOS 11 以降）/ （ターミナル）`~ % brew install --cask takushio2525/tako/tako` / `==> Downloading tako-v0.8.40-macos-arm64.zip` / `==> Moving App 'tako.app' to '/Applications/tako.app'` / `==> Linking Binary 'tako' to '/opt/homebrew/bin/tako'` / `tako was successfully installed!` / `~ % tako --version` / `tako 0.8.40` / アプリは「アプリケーション」に入り、`tako` コマンドも一緒に使えるようになります / （注記）ターミナルの出力は再現です（抜粋） |
| 22.5–32.5 | zip | 1 入れる / Homebrew を使わないなら、zip で。/ 1 GitHub Releases から `tako-vX.X.X-macos-arm64.zip` をダウンロード（github.com/takushio2525/tako/releases）/ 2 展開して、`tako.app` を「アプリケーション」へ / 3 初回に警告が出たら「このまま開く」（システム設定 → プライバシーとセキュリティ にあります。署名していない配布物のためで、一度許可すれば次からはそのまま開けます） |
| 32.5–40 | Windows | 1 入れる / Windows は、インストーラーを実行。/ 1 GitHub Releases の `tako-vX.X.X-windows-x86_64.exe` を実行（Windows 10（1809 以降）/ 11・x64。管理者権限は要りません）/ Windows 版は、移植の途中です。使える機能と、まだのものは「Windows 対応状況」にまとめています ・ tako.takushio2525.com/windows-support |
| 40–42.5 | 章 2 | 2 整える / 起動したあとのセットアップとアップデート |
| 42.5–57.5 | 初回起動（実画面） | 2 整える / 初回は、初期設定が自動で。/ 初めて起動すると、タブバーの下に案内が出ます / PATH・AI 連携の登録・指示ファイルまで自動です / 準備ができたら「master を起動」ボタンが出ます |
| 57.5–65 | エージェントが無いとき | 2 整える / Claude Code が、まだ無いときは。/ 案内に「Claude Code を入れる」ボタンが出ます（押すと、表示されたコマンドを新しいタブで実行します（管理者権限は使いません））/ ログインがまだなら「ログイン」ボタン（ブラウザが開きます。済むと、案内は自動で次へ進みます）/ Codex CLI / Antigravity CLI でも使えます（一部の機能は Claude Code だけ） |
| 65–80 | `tako setup`（実画面） | 2 整える / tako setup で確かめ直す。/ 案内を閉じても、`tako setup` でいつでもやり直せます / エージェントの検出から、AI 連携（MCP）の登録まで / 2 回目からは、変わった所だけ。ここでは「変更なし」/ 最後に対話アシスタント。設定の変更を日本語で頼めます |
| 80–92.5 | 更新の方法 | 2 整える / アップデートは、3 つの方法で。/ アプリの中で: 新しい版が出ると、画面の上に「アップデートがあります」（「詳細を見る」→「今すぐ更新」。終わると自動で再起動します）/ コマンドで: `tako update check` → `tako update apply` / Homebrew で: `brew upgrade --cask takushio2525/tako/tako` / 更新のあとは `tako setup` をもう一度。新しい設定項目に追従します |
| 92.5–100 | 更新の確認（実画面） | 2 整える / 更新の確認は、1 コマンド。/ `tako update check` は確認だけ。何も入れません / `available: false` なら最新。あれば `tako update apply` |
| 100–102.5 | 章 3 | 3 任せる / master を起動して、仕事を頼む |
| 102.5–147.5 | `tako master`（実画面） | 3 任せる / tako master で頼む。/ `tako master` と打つと、今いるペインが司令塔（master）に / まず、作業するフォルダを日本語で登録します / あとは、やってほしいことをそのまま書くだけ / 担当（worker）が同じタブに並びます。全体は右パネルで（吹き出し: 司令塔（master）/ 担当（worker）× 2 / 全体の見取り図）/ 終わると master が確かめて、結果をまとめて報告します |
| 147.5–157.5 | 便利なこと | 3 任せる / 覚えておくと、便利なこと。/ `tako master --tab` 専用のタブで、司令塔を起動 / `tako solo` 担当を立てずに、1 対 1 で相談 / `brew install tmux` tmux があると、tako を閉じても作業が続きます（開き直すと、実行中の画面ごと元に戻ります）/ プロジェクトの追加や設定の変更も、master に日本語で頼めます |
| 157.5–170 | まとめ | tako / この 3 つで、始められます。/ 1 `$ brew install --cask takushio2525/tako/tako` / 2 起動する（初期設定は自動。あとからは `tako setup`）/ 3 `$ tako master` / tako.takushio2525.com/getting-started ・ github.com/takushio2525/tako / macOS（Apple Silicon）で使えます。Windows 版は対応を進めています。/ この動画の画面は実際の tako です（デモ用の環境で収録）。エージェントの出力はデモ用の台本、brew の出力は再現です。 |

実画面の場面の右上には注記を出し続ける。エージェントが映る場面（`tako setup`・`tako master`）は
「画面は実際の tako。エージェントの出力はデモ用の台本です」、映らない場面（初回起動・更新の確認）は
「画面は実際の tako です（デモ用の環境で収録）」。

実画面の中でペインへ打つ文字: `tako setup` / `tako update check` / `tako master` /
`~/shop-app を管理対象に追加して` / `決済 API にリトライを入れて、README も更新して`。

## 手順ごとの根拠

語る手順・コマンド・画面は、今の main に実在するものだけにした。実画面の場面は**収録の中で実際に動かした**。

| 手順・主張 | 根拠 |
|---|---|
| `brew install --cask takushio2525/tako/tako`・Apple Silicon / macOS 11 以降 | `README.md`「インストール」。公開の cask（`takushio2525/homebrew-tako` の `Casks/tako.rb`）は 2026-11-10 時点で `version "0.8.40"` |
| アプリは「アプリケーション」に入り、`tako` コマンドも使える | `README.md`「tako CLI も同時に PATH へ入る」/ cask の `app "tako.app"` と `binary ".../tako"` |
| `tako --version` → `tako 0.8.40` | 収録用のバイナリで実行した出力そのまま。最新のリリースも v0.8.40（`gh release list`） |
| zip: Releases から `tako-vX.X.X-macos-arm64.zip` → 展開して `/Applications` → 警告は システム設定 →「このまま開く」 | `README.md`「macOS: zip を手動で」。v0.8.40 のリリースに `tako-v0.8.40-macos-arm64.zip` がある |
| Windows: `tako-vX.X.X-windows-x86_64.exe`・Windows 10 1809 以降 / x64・管理者権限は不要 | `README.md`「Windows: インストーラー / ポータブル zip」。v0.8.40 のリリースに `tako-v0.8.40-windows-x86_64.exe` がある |
| Windows 版は移植の途中 | `README.md`「移植は途中です」。`tako platform --platform windows` は supported 120 / degraded 23 / pending 20 / unsupported 2（収録用のバイナリ）。**「対応済み」とは言っていない** |
| 初めて起動すると案内が出て、初期設定が自動で進む | 収録 `firstrun`: 設定の無い HOME で起動 → タブバーの下に「tako へようこそ」→「初期設定を自動で進めています（PATH・AI 連携の登録・指示ファイル。ネットワークからの導入はしません）」→「初期設定を自動で済ませました（変更 8 件）」。実装は `crates/tako-control/src/welcome.rs`（#1510） |
| 準備ができたら「master を起動」ボタン | 同じ収録の「準備ができました。master（AI 司令塔）を起動して、あとは日本語で頼むだけです」と「master を起動」ボタン |
| Claude Code が無ければ「Claude Code を入れる」、未ログインなら「ログイン」のボタン | `docs/src/content/docs/getting-started/index.mdx`「2. 起動」/ 文言は `crates/tako-app/src/ui_text/welcome.rs`。**この 2 つの状態は収録していない**（偽の claude を「入っていてログイン済み」にしたため）ので、図解で語っている |
| Codex CLI / Antigravity CLI でも使える（一部は Claude Code だけ） | `tako agent-support`: Claude Code supported 67 / Codex supported 38・degraded 8・pending 18・unsupported 3 / Antigravity supported 25・degraded 5・pending 26・unsupported 11 |
| `tako setup` でやり直せる・検出から MCP の登録まで・2 回目は「変更なし」 | 収録 `setup`: ペインへ `tako setup` を打つ（本物の CLI）。出力に `[検出] claude: ~/.local/bin/claude（認証済み / pro）`・`[OK] Claude MCP: tako が登録済み`・`セットアップ結果: 変更なし（前回の設定は最新）`・`セットアップが完了しました。` |
| 最後に対話アシスタントが開く | 同じ収録の `対話アシスタントを起動します。設定の変更・解説・次の一歩の相談ができます。` の後、選んだエージェント（ここでは偽の claude）が同じペインで起動する。**アシスタントの画面の文面は台本** |
| 新しい版が出ると画面の上に「アップデートがあります」→「詳細を見る」→「今すぐ更新」→ 自動で再起動 | `crates/tako-app/src/ui_text/update.rs`（`card_title` / `card_details` / `update_now`）・`.agent/commands.md`「アプリ内更新」（上部通知カード + 専用画面。#616）・`crates/tako-app/src/update_checker.rs`（更新完了後は自動再起動）。**通知カードは収録していない**（収録用のバイナリが最新版と同じ 0.8.40 で、更新が無いため）ので、図解で語っている |
| `tako update check` は確認だけ → `available: false` | 収録 `update`: ペインへ `tako update check` を打った実際の出力（公開のリリース一覧を読むだけ） |
| `tako update apply`・`brew upgrade --cask takushio2525/tako/tako` | `docs/…/getting-started/index.mdx`「アップデート」・`README.md`。**実行はしていない**（本番を更新しないため） |
| 更新のあとは `tako setup` をもう一度 | `docs/…/getting-started/index.mdx`「アップデート後の追従: `tako setup` の再実行」 |
| `tako master` で今いるペインが司令塔になる | 収録 `master`: ペインへ `tako master` を打つ（本物の CLI）。出力に `master を起動しました: ペイン 1（インライン）`。起動されるエージェントだけが偽物 |
| 作業するフォルダを日本語で登録 | 収録 `master`: 司令塔の台本が本物の `tako orchestrator projects add --key shop-app --cwd ~/shop-app` を打つ。「プロジェクト登録などの設定変更は、すべて master に頼めます」は `tako setup` の「次の一歩」の文 |
| 担当が同じタブに並ぶ・全体は右パネルで | 収録 `master`: 司令塔の台本が本物の `tako orchestrator spawn --project shop-app --label … --prompt …` を 2 回と `tako panel --show --view orch` を打つ。指示の送達も本物の経路 |
| 終わると master が確かめて報告 | 完了の検知は本物の `tako orchestrator status --pane N`（`status` が `idle`）。**確かめた内容と報告の文面は台本**（LLM を呼んでいないため）。本物の作法は `docs/src/content/docs/features/orchestration.mdx` |
| `tako master --tab`・`tako solo` | `README.md`「使い始める」 |
| tmux があると tako を閉じても作業が続く・`brew install tmux` | `docs/…/getting-started/index.mdx`「事前に必要なもの」 |

### 再現したカット（実際には流していない出力）

- **Homebrew の場面のターミナル（10–22.5 秒）**: `brew install` を本物へ向けて打てないので、出力を再現した。
  形は docs の導入の場面（`docs/src/components/scenes/InstallMac.astro` の 22〜27 行）と同じ 4 行で、版数は公開の cask と
  最新のリリース（0.8.40）に合わせた。本物の出力の最後の行にある絵文字は入れていない（docs と同じ）。
  `tako --version` の行は実際の出力と同じ。実画面と見分けがつくよう、窓の形を実画面の窓と変え、右上に
  「ターミナルの出力は再現です（抜粋）」を出している。まとめの断りにも書いた
- それ以外の図解の場面（zip / Windows / エージェントが無いとき / 更新の方法 / 便利なこと）は、画面の絵を作らず文字だけで語っている

### 実画面で、時間を伸ばしている所

読ませるために、収録の一部を止めて見せている（中身は変えていない）。

- 初回起動: 自動の初期設定は実時間で約 1 秒で終わる。「進めています」の 1 枚を 4 秒、「済ませました」を 11 秒見せている
- `tako setup`: 出力は約 0.5 秒で流れ切る。上半分の 1 枚を 4 秒、下半分の 1 枚を 2.8 秒見せている
- 偽の claude は起動の前に 2.5 秒待つ（`CLAUDE_STUB_BOOT_MS`）。本物の claude が画面を描くまでの間に当たり、
  その間 `tako setup` の結果や `tako master` の起動の行が読める

## 作り方

| 段 | もの | コマンド |
|---|---|---|
| 0 | 収録用の読み戻しを入れて建てる | `git apply scripts/promo/remake/promo-rec.patch`（建てるのは `record.sh` が `isolated_gui_bins visual-test` で行う） |
| 1 | 実画面を収録する | `bash scripts/promo/howto/record.sh`（場面を選ぶなら `record.sh master` など） |
| 2 | 本体を戻す | `git checkout crates/tako-app/src/main.rs` |
| 3 | 書き出す（舞台を組む → 音 → 映像 → 合わせる → 検査） | `bash scripts/promo/howto/build.sh` → `~/Movies/tako-promo/tako-howto-v1.mp4` |
| 確認 | 指定した秒の 1 枚だけ描く | `bash scripts/promo/howto/build.sh --stills 13,48,120 <dir>` |
| 音だけ直す | 映像は描き直さずに、音を合成して合わせ直す | `TAKO_HOWTO_REUSE_VIDEO=1 bash scripts/promo/howto/build.sh` |

| ファイル | 役割 |
|---|---|
| `record.sh` | #2864 の `record.sh` の関数（砂場・隔離 GUI・読み戻し・連番の JPEG と時刻の表・片付け）を読み込み、この動画の砂場の差分と 4 場面（`firstrun` / `setup` / `update` / `master`）を足す。収録の前後で本物の HOME と `/Applications/tako.app` が変わっていないことを確かめる |
| `demo/claude-stub.mjs` | `claude` の名前で砂場の `~/.local/bin` に置く台。版・認証・MCP の登録の問い合わせにだけ自分で答え、対話の起動は #2864 の `agent-sim.mjs` へ渡す |
| `demo/personas/master.json`・`solo.json` | 司令塔と、`tako setup` の対話アシスタントの台本。担当（api / docs）の台本は #2864 のものを収録時に砂場へ写して使う |
| `comp/index.html`・`howto.css`・`howto.js`・`timeline.json` | この動画の場面・字幕・時刻。`howto.js` は `comp.js` に 3 つ足す（注記の差し替え・図解の場面の消し方・ターミナルの打鍵） |
| `arrangement.js` | 曲の構成（68 小節）。楽器は #2864 の `bgm.mjs` のもの |
| `stage.mjs` | #2864 の `render.mjs` / `timeline.mjs` / `comp.js` / `comp.css` / `bgm.mjs` と、この動画の `comp/`・`arrangement.js` を作業用の置き場に並べる（下の節） |
| `build.sh` | 書き出しと検査（音量のそろえ方と検査の項目は #2864 の `build.sh` と同じ） |

新しい依存は足していない（Node・ffmpeg・`web/tako-remote` の Playwright はどれも既存）。

### #2864 の仕組みを、複製せずに使う

#2864 の `render.mjs` / `timeline.mjs` / `bgm.mjs` は、自分の隣の `comp/index.html` と `comp/timeline.json` を固定で読む。
この動画の場面を渡す口が無いので、書き出すたびに `stage.mjs` が作業用の置き場（`<出力>/work/stage/`）へ
#2864 の 4 本とこの動画の `comp/` を並べ、そこで #2864 の `render.mjs` / `bgm.mjs` を走らせる。
リポジトリに入るのはこの動画の差分だけで、#2864 の `scripts/promo/remake/` は 1 行も変えていない
（#2864 の設計書に、この動画への案内を 1 行足しただけ）。

音は 1 か所だけ差し替える。`bgm.mjs` は楽器と曲の構成が 1 本になっていて、構成は 95 秒の場面割りに合わせてある。
`stage.mjs` は「構成」の節（`// ---- 構成` から `// ---- 効果音` の手前まで）を `arrangement.js` に差し替えた `bgm.mjs` を
舞台に置く。節の見出しが見つからなければ、黙って続けずに止まる。

収録は、#2864 の `record.sh` の「本体」より前（関数の定義）だけを読み込む。

**#2864 の側をこう変えれば、`stage.mjs` と上の読み込み方は要らなくなる**（この PR では変えていない）:

- `render.mjs` / `timeline.mjs`: comp の置き場を引数で受ける（既定は今の `comp/`）
- `bgm.mjs`: 楽器・残響・書き出しと、曲の構成を分ける（構成を引数で受ける）
- `record.sh`: 関数の定義を別ファイルへ出す（か、読み込まれたときは本体を走らせない）
- `comp.js`: `#tiles` が無いページでも動く（今は空の `#tiles` を置いて通している）。実画面の場面の注記を `timeline.json` から受ける

### 砂場と、本物の HOME を変えないこと

- HOME は砂場の `demo`（`/private/tmp/tako-demo/home/demo`）。tako の置き場はその下の既定と同じ形
  （`~/Library/Application Support/tako`）にして、画面のパスを実機と同じ見え方にしている
- 初回起動の導線（#1510）は、検証用 GUI では既定で動かない（HOME へ書くため）。HOME を砂場へ向けたうえで
  `TAKO_FIRST_RUN=1` を明示して動かしている。`tako setup` も同じ砂場の HOME にだけ書く
  （`.zprofile`・`.claude.json`・`.claude/CLAUDE.md`・`.claude/skills/tako`・`.local/bin/tako`）
- 収録用のバイナリは砂場へ写してから起こす（リンク先や PATH として画面へ出ても、利用者のホームのパスにならない）
- `record.sh` は収録の前後で、本物の HOME の 28 項目（シェルの設定・エージェントの設定と MCP の登録・指示ファイル・
  tako CLI のリンク・tako の設定・`/Applications/tako.app`）の更新時刻と大きさを控えて比べ、変わっていたら終了コード 3 で終わる
- 本物の `brew install` / `upgrade`・`tako update apply` は打っていない（`tako update check` は公開のリリース一覧を読むだけ）

### 個人情報を出さない

- プロンプトは固定（`demo ~ %`）、プロジェクトは架空の `shop-app`、偽の claude の認証の答えにメールや組織の欄は無い
- `tako update status` は画面に出さない（重複した CLI の置き場として実際のパスを列挙するため）
- 検査は 2 重: 全フレームの OCR（`scripts/promo/pii-scan.sh`。検査語は環境から組み立てる）と、等間隔のフレームの目視

## 検査（v1・2026-11-10）

| 項目 | 値 |
|---|---|
| 動画 | `~/Movies/tako-promo/tako-howto-v1.mp4`（18.3 MB） |
| 長さ / 形式 | 170.000 秒 / 1920x1080 / 30fps / H.264 + AAC 48kHz stereo（`ffprobe`。映像 5100 枚） |
| デコードの全走査 | `ffmpeg -v error -i <mp4> -f null -` の出力 0 行（映像 5100 枚と音声を最後までデコード） |
| 音量 | -15.1 LUFS / LRA 2.8 LU / True Peak -1.5 dBFS（目標 -15 LUFS。合成後に 1 回測って固定のゲイン +0.30 dB）。プロモーション動画の v1 は -15.1 LUFS / LRA 5.7 LU / True Peak -1.5 dBFS（同じ測り方で測り直した値）。LRA が小さいのは、静かな導入や抜きの無い、一定の伴奏にしたため |
| 音の調整 | 最初の構成（ドラムが #2864 の本編と同じ強さ）は -15.0 LUFS / True Peak **-0.1 dBFS** だった。AAC にする前は -1.5 dBFS で、実画面の場面がリミッタへ当たり続けていたのが原因。`arrangement.js` の実画面の場面のドラムを 0.8 倍にして測り直し、上の値になった |
| 台本との突き合わせ | 画面に出す 91 句のうち 89 句が OCR（1 fps・170 フレーム・3681 行）と一致。残り 2 句（「初回に警告が出たら「このまま開く」」の「警」・「担当（worker）× 2」の「×」）は OCR の読み違いで、フレームを目で見て確認した |
| 個人情報の検査（OCR） | email / home_path / tailnet / private_ip / token / uuid は 0 件。環境由来の語は 47 件で、**すべて公開情報の形**（tap 名つきの `brew install` / `brew upgrade`・GitHub の URL・docs のドメイン）。出ている秒は 14–22 / 24–32 / 36–40 / 87–92 / 161–170（Homebrew・zip・Windows・更新の方法・まとめ）で、それ以外は 0 件 |
| 目視 | 等間隔の 12 枚と、章ごとの 6 枚（`~/Movies/tako-promo/checks/tako-howto-v1/`）を 1 枚ずつ見た。字幕のはみ出し・重なり無し、HOME は `~`、プロンプトは `demo`、パスは砂場のもの |
| 収録 | 隔離 GUI は `tako-vd` の上（窓 1280x720pt = 2560x1440px・位置 1270,700）。終了時に隔離 GUI・tmux の隔離ソケット・砂場を消している（`record.sh` の「片付け」の行）。本物の HOME と `/Applications/tako.app` は収録の前後で変化なし（28 項目） |
| 番犬 | `shell_scripts` 39 passed / `verification_isolation_watchdog` 17 passed / `issue1490_isolated_gui_launch_watchdog` 27 passed / `no_personal_data` 6 passed / `context_budget` 13 passed ほか |

未確認: **音を耳で聴いていない**（数値だけ。この環境に音声出力が無い）。曲の好みや効果音の大きさは試聴して決める必要がある。
直すときは `arrangement.js` を変えて `TAKO_HOWTO_REUSE_VIDEO=1 bash scripts/promo/howto/build.sh`（映像は描き直さない）。

収録で見つけた docs と今の画面の食い違い（更新の通知の場所・Gatekeeper の手順・初回起動バナーの図）は #2869 に起票した。
動画は今の画面・実装の側に合わせている。
