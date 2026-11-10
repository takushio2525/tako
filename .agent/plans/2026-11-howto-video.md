# tako の入れ方・使い方動画（#2866 → v2 = #2886）

> 4 分 10 秒の入れ方・使い方動画（v2）の台本・根拠・作り方・検査の正本。
> 見た人がこの動画だけを頼りに、tako を入れ、対話で設定を決め、master で仕事を回し始められることが目的。
> ソースは `scripts/promo/howto/`、成果物（mp4・収録フレーム）はリポジトリの外 `~/Movies/tako-promo/`（コミットしない）。
> 合成・描画・音・収録の仕組みはプロモーション動画（#2864）の `scripts/promo/remake/` をそのまま使う
> （仕組みの説明は `.agent/plans/2026-11-promo-remake.md`）。ここに書くのは、この動画の差分だけ。

## v2 で作り直した理由

v1（#2866・PR #2871・`~/Movies/tako-promo/tako-howto-v1.mp4`）の第 2 章は、当時の main の初回起動（#1510 =
起動した時点で自動の初期設定が走り、そのまま「master を起動」が出る）を見せていた。#2879（PR #2882）で初回起動を
「ボタンを押す → setup のタブで `tako setup` → 対話のセットアップ → master を起動」へ変えたので、v1 は今の画面と食い違う。

利用者の指摘（2026-11-10）は「合ってる？」「セットアップは最初に自動実行ではなく、ボタンを押してセットアップと
チャットではないか」「master / worker の設定や GitHub は対話でないと決められないのではないか」「動画もしっかりつくって」。
「戻して」の部分は #2879 で済んでいる。v2 は仕組み（合成・音・収録）を使い直し、**中身（台本・場面・収録）を作り直した**。

### 利用者の 3 つの指摘への答え（今の main = `c55c08e` の実物で確かめた結果）

| 指摘 | 答え | 確かめ方 |
|---|---|---|
| 最初に自動実行ではなく、ボタンを押してセットアップではないか | v1 を作った時点の main は自動実行だった（#1510）。**今の main は、起動しただけでは何も走らせず、何も書かない**。案内の「セットアップを実行」を押して初めて `tako setup` が走る | 収録 `firstrun`: 設定の無い HOME で起動した直後の `tako welcome` は `onboarding.flow: guided`・`step.kind: setup`・`auto.state: idle`・`active: false`。その時点の HOME は置いたものだけで、tako の置き場に `setup/` も `orchestrator/config.yaml` も無い（`record.sh` の `scene_firstrun` が確かめ、書かれていたら止まる）。実装は `welcome::startup_action`（`Startup::Probe`）・仕様は FR-2.14.21 |
| セットアップとチャット（対話）ではないか | そのとおり。ボタンは新しいタブ（題 setup）で端末の `tako setup` を打ち、検出・登録が済むと**同じタブで対話のアシスタントが開く**。対話を閉じると案内が「master を起動」へ進む | 収録 `guided`: 段は `setup` → `running`（`phase: setup`）→ `running`（`phase: dialogue`）→ `master` の順に進んだ（`tako welcome`）。対話を閉じた後の `config.yaml` に `setup.dialogue_done: true`。アシスタントが受け取る最初のメッセージは「…初回フローを始めます。」（`setup::assistant_greeting`） |
| master / worker の設定や GitHub は、対話でないと決められないのではないか | そのとおり。対話のアシスタントの手順（`resources/setup/system-prompt.md` の初回フロー）のうち、**master / worker の設定は Step 2、GitHub は Step 3.5（設定共有）**に当たる。Step 2 は今の設定を説明して「このまま使うか、調整するか」を聞き、望まれたときだけ変更の案を出す。Step 3.5 は、gh（GitHub CLI）がログイン済みなら、同意と名前の確認を得てから `gh repo create <名前> --private` → `tako config init --remote <URL>` を代行する | `system-prompt.md` の Step 2・Step 3.5 と、案内の文言「最後に対話のアシスタントが開き、master / worker の設定や GitHub との連携などを相談しながら決めます」（`ui_text::welcome::setup_detail`）。動画の対話の場面は、この手順の順に、実在のコマンドを砂場で実際に打って見せている（下の「対話のセットアップの場面」） |

**setup がしないこと**: GitHub へのログイン（`gh auth login`）や git の利用者情報の設定は、`tako setup` も対話のアシスタントも代行しない。
gh が未ログインのときは、ローカルだけで `tako config init` して、GitHub を使うなら `gh auth login` を案内する、という手順になっている
（`system-prompt.md` Step 3.5 の 3）。利用者の言う「GitHub」がこのログインを指していた場合、それは setup の中の操作ではない。
動画は作り話で埋めず、字幕で「`gh` が未ログインなら、ログインの手順を案内します」と言うにとどめた。

## v1 の台本の全行の突き合わせ

v1 の台本（v1 の設計書の表の全行）を、今の main の画面・実装・docs と 1 行ずつ突き合わせた結果。

| v1 の行（秒） | v1 の語り | 今の main の実際 | v2 |
|---|---|---|---|
| 題字（0–7.5） | 入れ方と、使い方。1 入れる（ダウンロード）→ 2 整える（セットアップと更新）→ 3 任せる（master に頼む） | 3 段の流れは同じ。2 の中身が「対話でセットアップ」になった | 2 の説明を「対話でセットアップ」に直した |
| 章 1（7.5–10） | 1 入れる / ダウンロードとインストール | 変わらず | そのまま |
| Homebrew（10–22.5） | `brew install --cask takushio2525/tako/tako`・Apple Silicon / macOS 11 以降・`tako 0.8.40`・アプリは「アプリケーション」に入り `tako` コマンドも使える | 合っている。公開の cask は `version "0.8.40"`・最新のリリースは v0.8.40（2026-11-06）・収録用のバイナリの `tako --version` は `tako 0.8.40` | そのまま |
| zip（22.5–32.5） | Releases の zip → 展開して「アプリケーション」へ → 警告は「このまま開く」（署名していない配布物のため） | 手順は合っている。**理由の言い方と範囲が docs と違う**: docs（#2869 の修正後）は「Apple の公証をまだ受けていないため」「Homebrew で入れた場合も同じ」 | 理由を「Apple の公証をまだ受けていないため」に直し、「Homebrew で入れたときも同じです」を足した |
| Windows（32.5–40） | インストーラーを実行（Windows 10 1809 以降 / 11・x64・管理者権限は不要）。Windows 版は移植の途中 | 合っている。ただし**初回の警告（SmartScreen）の越え方が抜けていた**（docs「SmartScreen の警告が出たら: 詳細情報 → 実行」）。`tako platform --platform windows` は supported 120 / degraded 23 / pending 20 / unsupported 2 | 手順 2 に「警告が出たら「詳細情報」→「実行」」を足し、対応状況を今の数（165 個のうち使える 120・一部制限 23・未確認 20・非対応 2）で言った。「対応済み」とは言っていない |
| 章 2（40–42.5） | 2 整える / 起動したあとのセットアップとアップデート | 章の中身が変わった | 「ボタンから、対話でセットアップ。そしてアップデート」 |
| 初回起動（42.5–57.5） | 初回は、初期設定が自動で。/ 初めて起動すると、タブバーの下に案内が出ます / PATH・AI 連携の登録・指示ファイルまで自動です / 準備ができたら「master を起動」ボタンが出ます | **違う**。案内は出るが、起動しただけでは何も走らない。PATH・MCP の登録・指示ファイルは、ボタンを押した後の `tako setup` が行う。「master を起動」が出るのは対話を閉じた後 | 撮り直した（`firstrun`）。「初回は、ボタンから。」「起動しただけでは、何も実行されません」「「セットアップを実行」を押して始めます」 |
| エージェントが無いとき（57.5–65・図解） | 案内に「Claude Code を入れる」ボタンが出ます / ログインがまだなら「ログイン」ボタン（済むと、案内は自動で次へ進みます）/ Codex CLI / Antigravity CLI でも使えます | **道筋が違う**。最初のボタンは常に「セットアップを実行」で、Claude Code が無ければ **`tako setup` 自身が端末で「この内容でインストールしますか？ [Y/n]」を聞く**。ログインが残ると案内に「ログイン」が出て、済むと「セットアップを続ける」（もう一度 `tako setup` → 対話）が出る。自動で master へは進まない。「Claude Code を入れる」ボタンは、導入を断った後などに残りとして出る段で、通常の道ではない。Codex / Antigravity の一文は合っている（`tako agent-support`: claude 67 / codex 38・8・18・3 / agy 25・5・26・11） | 図解をやめ、Claude Code の無い砂場で**実画面を撮った**（`noagent`）。Codex / Antigravity の一文は「便利なこと」へ移した |
| `tako setup`（65–80） | 案内を閉じても、`tako setup` でいつでもやり直せます / エージェントの検出から、AI 連携（MCP）の登録まで / 2 回目からは、変わった所だけ。ここでは「変更なし」/ 最後に対話アシスタント。設定の変更を日本語で頼めます | 個々の文は今も正しいが、**場面の前提が違う**。v1 は自動の段が先に済ませた後の 2 回目の setup を見せていた。今はボタンが打つ setup が 1 回目で、結果は「変更 8 件」。対話アシスタントは「最後におまけで開く」ものではなく、初回の設定を決める本編 | 撮り直した（`guided`）。1 回目の setup の出力（頭と結果の一覧）を見せ、対話の場面を 4 つ（Step 1・2 / 3 / 3.5 / 4）に分けて通した |
| 更新の方法（80–92.5・図解） | アプリの中で: 新しい版が出ると、画面の上に「アップデートがあります」/ コマンドで / Homebrew で / 更新のあとは `tako setup` をもう一度 | 合っている。場所は docs（#2869 の修正後）の言い方で「タブバーのすぐ下」 | 「画面の上に」を「タブバーの下に」へ直した |
| 更新の確認（92.5–100） | `tako update check` は確認だけ。`available: false` なら最新 | 合っている（撮り直しても同じ出力） | 撮り直した。文はそのまま |
| 章 3（100–102.5） | 3 任せる / master を起動して、仕事を頼む | 変わらず | そのまま |
| `tako master`（102.5–147.5） | `tako master` と打つと、今いるペインが司令塔に / まず、作業するフォルダを日本語で登録します / あとは、やってほしいことをそのまま書くだけ / 担当が同じタブに並びます。全体は右パネルで / 終わると master が確かめて、結果をまとめて報告します | 動きは合っている。**入口と順番が変わった**: 初回は案内の「master を起動」から入れる。プロジェクトの登録は対話のセットアップ（Step 3）で済んでいる（master に頼んで登録することも今もできる） | 冒頭を「「master を起動」を押すと、新しいタブで司令塔が開きます」「ターミナルで `tako master` と打っても同じです」にし、登録の段を外して依頼から始めた |
| 便利なこと（147.5–157.5） | `tako master --tab` / `tako solo` / `brew install tmux` / プロジェクトの追加や設定の変更も、master に日本語で頼めます | 合っている（`tako master --help`・`tako solo --help`・README） | `tako setup`（いつでも、対話で設定を見直せます）を足し、下の 1 行を Codex / Antigravity の一文にした |
| まとめ（157.5–170） | 1 `brew install …` / 2 起動する（初期設定は自動。あとからは `tako setup`）/ 3 `tako master` | **2 が違う**（自動ではない） | 2 を「起動して「セットアップを実行」（`tako setup` → 対話で設定）」、3 を「「master を起動」か `tako master`。あとは日本語で頼む」に直した。断りに「インストーラ・ログイン・GitHub の出力は再現」を足した |

## 構成と長さ

| 秒 | 章 | 中身 |
|---|---|---|
| 0–7.5 | 題字 | 入れ方と、使い方。/ 1 入れる → 2 整える → 3 任せる |
| 7.5–42.5 | 1 入れる | Homebrew（12.5 秒）/ zip（10 秒）/ Windows（10 秒） |
| 42.5–185 | 2 整える | 初回起動（12.5 秒）/ ボタン → `tako setup`（17.5 秒）/ 対話: master / worker（25 秒）/ 対話: プロジェクト（10 秒）/ 対話: GitHub（20 秒）/ 対話を閉じて master へ（15 秒）/ Claude Code が無いとき（20 秒）/ 更新の 3 つの方法（12.5 秒）/ `tako update check`（7.5 秒） |
| 185–237.5 | 3 任せる | `tako master`（37.5 秒）/ 覚えておくと便利なこと（12.5 秒） |
| 237.5–250 | まとめ | 3 つの手順・リンク・断り |

章立ては v1 の 3 章のまま（入れる / 整える / 任せる）。利用者が覚える順番がこの 3 段であることは変わらないので、変えていない。

長さを 250 秒（v1 は 170 秒）にした理由: 増えた 80 秒は、ほぼすべて第 2 章の対話のセットアップ（70 秒）である。
対話は「アシスタントの問い → 利用者の答え → 実行と結果」の往復を省くと、何を対話で決めるのかが伝わらない
（利用者の指摘そのもの）。master / worker の設定は「説明 → 希望 → 案 → 同意 → 反映」の 2 往復、GitHub は
「紹介と名前の確認 → 同意 → 作成・配線・確認」の 1 往復を、読める速さで通すとこの長さになる。
代わりに、v1 で 45 秒あった master の章は、プロジェクトの登録を対話へ移したぶん 37.5 秒になった。
Claude Code が無いときの場面は、図解（7.5 秒）から実画面（20 秒）にした（道筋が変わったので、実物を見せる）。
章ごとに切り出すなら、第 2 章（142.5 秒）は「初回起動〜対話（100 秒）」と「Claude Code が無いとき〜更新（42.5 秒）」に分けられる。

## プロモーション動画（#2864 の v1）から揃えた点・変えた点

| | 揃えた | 変えた（理由） |
|---|---|---|
| 色・書体・地 | 紙色の地・コーラルの差し色・同じ書体（`comp.css` をそのまま読む） | なし |
| 字幕・テロップ | 左上にチップと見出し、右に 1 行の字幕、実画面の窓・寄り・吹き出し・右上の注記（`comp.js` が描く） | チップを章の番号（「1 入れる」）にした。図解の場面にも同じ位置にチップと見出しを置いた（章の中で実画面と図解を行き来するため） |
| 実画面 | 隔離した実際の tako を読み戻しで収録・エージェントは偽物（`agent-sim.mjs`）・1280x720pt の窓 | フォントの大きさとタブ名を既定のままにした（初めて起動した人が見る画面にそろえるため）。HOME の下を実機と同じ形にした（パスが `~/Library/Application Support/tako/…` と出る） |
| 音 | ナレーション無し・同じ楽器と残響（`bgm.mjs`）・96 BPM・場面の切り替わりは小節の頭・効果音・目標 -15 LUFS | 曲の構成（どの小節で何を鳴らすか）は 250 秒用に書いた（`arrangement.js`・100 小節）。図解と対話の場面は刻みを軽くした（読ませる場面のため） |
| 解像度・fps | 1920x1080・30fps・H.264 + AAC 48kHz stereo | なし |

## 台本（画面に出る文字の全文）

時刻は映像の秒。字幕の正本は `scripts/promo/howto/comp/index.html`（図解の場面）と
`scripts/promo/howto/comp/timeline.json`（実画面の場面）。

| 秒 | 場面 | 画面に出る文字 |
|---|---|---|
| 0–7.5 | 題字 | tako / 入れ方と、使い方。/ 1 入れる（ダウンロード）→ 2 整える（対話でセットアップ）→ 3 任せる（master に頼む） |
| 7.5–10 | 章 1 | 1 入れる / ダウンロードとインストール |
| 10–22.5 | Homebrew | 1 入れる / Mac は、Homebrew で 1 行。/ Apple Silicon の Mac（macOS 11 以降）/ （ターミナル）`~ % brew install --cask takushio2525/tako/tako` / `==> Downloading tako-v0.8.40-macos-arm64.zip` / `==> Moving App 'tako.app' to '/Applications/tako.app'` / `==> Linking Binary 'tako' to '/opt/homebrew/bin/tako'` / `tako was successfully installed!` / `~ % tako --version` / `tako 0.8.40` / アプリは「アプリケーション」に入り、`tako` コマンドも一緒に使えるようになります / （注記）ターミナルの出力は再現です（抜粋） |
| 22.5–32.5 | zip | 1 入れる / Homebrew を使わないなら、zip で。/ 1 GitHub Releases から `tako-vX.X.X-macos-arm64.zip` をダウンロード（github.com/takushio2525/tako/releases）/ 2 展開して、`tako.app` を「アプリケーション」へ / 3 初回に警告が出たら「このまま開く」（システム設定 → プライバシーとセキュリティ にあります。Apple の公証をまだ受けていないためで、Homebrew で入れたときも同じです。一度許可すれば、次からはそのまま開けます） |
| 32.5–42.5 | Windows | 1 入れる / Windows は、インストーラーを実行。/ 1 GitHub Releases の `tako-vX.X.X-windows-x86_64.exe` を実行（Windows 10（1809 以降）/ 11・x64。管理者権限は要りません）/ 2 警告が出たら「詳細情報」→「実行」（「Windows によって PC が保護されました」と出ることがあります（署名していない配布物のため））/ Windows 版は、移植の途中です。操作 165 個のうち、使える 120・一部制限 23・未確認 20・非対応 2（`tako platform` の集計）・ tako.takushio2525.com/windows-support |
| 42.5–45 | 章 2 | 2 整える / ボタンから、対話でセットアップ。そしてアップデート |
| 45–57.5 | 初回起動（実画面） | 2 整える / 初回は、ボタンから。/ 初めて起動すると、タブバーの下に案内が出ます / 起動しただけでは、何も実行されません / 「セットアップを実行」を押して始めます |
| 57.5–75 | ボタン → `tako setup`（実画面） | 2 整える / ボタン 1 つで、tako setup。/ 新しいタブが開き、`tako setup` が走ります / エージェントの検出・PATH・AI 連携（MCP）の登録 / 指示ファイルとプロファイルも、ここで用意します / 変更の一覧を出して、対話のアシスタントへ |
| 75–100 | 対話: master / worker（実画面） | 2 整える / 対話で、設定を決める。/ アシスタントが今の設定を読み、順に確かめます / master / worker の設定を説明して、希望を聞きます / 日本語で答えると、変更の案が出ます / 同意してから反映。勝手には変えません |
| 100–110 | 対話: プロジェクト（実画面） | 2 整える / プロジェクトを登録する。/ 作業するフォルダを伝えると、登録されます / あとから master に頼んでも、追加できます |
| 110–130 | 対話: GitHub（実画面） | 2 整える / GitHub で、設定を共有。/ 別の PC でも、同じ AI 設定で始められます / 作るのは private リポジトリ。名前も確かめます / 同意すると、作成から push まで代わりに実行 / `gh` が未ログインなら、ログインの手順を案内します |
| 130–145 | 対話を閉じて master へ（実画面） | 2 整える / 対話を閉じて、master へ。/ 最後に、変更の一覧と次の一歩が出ます / `/exit` で対話を閉じると、setup も終わります / 案内が「master を起動」に変わります |
| 145–165 | Claude Code が無いとき（実画面） | 2 整える / Claude Code が無いときは。/ `tako setup` が、入れてよいかを先に聞きます / 同意すると導入。ログインは自分で行います / 案内の「ログイン」を押すと、ログインが始まります / 済んだら「セットアップを続ける」で、対話へ |
| 165–177.5 | 更新の方法 | 2 整える / アップデートは、3 つの方法で。/ アプリの中で: 新しい版が出ると、タブバーの下に「アップデートがあります」（「詳細を見る」→「今すぐ更新」。終わると自動で再起動します）/ コマンドで: `tako update check` → `tako update apply` / Homebrew で: `brew upgrade --cask takushio2525/tako/tako` / 更新のあとは `tako setup` をもう一度。新しい設定項目に追従します |
| 177.5–185 | 更新の確認（実画面） | 2 整える / 更新の確認は、1 コマンド。/ `tako update check` は確認だけ。何も入れません / `available: false` なら最新。あれば `tako update apply` |
| 185–187.5 | 章 3 | 3 任せる / master を起動して、仕事を頼む |
| 187.5–225 | `tako master`（実画面） | 3 任せる / tako master で頼む。/ 「master を起動」を押すと、新しいタブで司令塔が開きます / ターミナルで `tako master` と打っても同じです / やってほしいことを、そのまま書くだけ / 担当（worker）が同じタブに並びます。全体は右パネルで（吹き出し: 司令塔（master）/ 担当（worker）× 2 / 全体の見取り図）/ 終わると master が確かめて、結果をまとめて報告します |
| 225–237.5 | 便利なこと | 3 任せる / 覚えておくと、便利なこと。/ `tako setup` いつでも、対話で設定を見直せます（2 回目からは、変わった所だけを確かめて対話へ進みます）/ `tako master --tab` 専用のタブで、司令塔を起動 / `tako solo` 担当を立てずに、1 対 1 で相談 / `brew install tmux` tmux があると、tako を閉じても作業が続きます（開き直すと、実行中の画面ごと元に戻ります）/ Codex CLI / Antigravity CLI でも使えます（一部の機能は Claude Code だけ） |
| 237.5–250 | まとめ | tako / この 3 つで、始められます。/ 1 `$ brew install --cask takushio2525/tako/tako` / 2 起動して「セットアップを実行」（`tako setup` → 対話で設定）/ 3 「master を起動」か `tako master`。あとは日本語で頼む / tako.takushio2525.com/getting-started ・ github.com/takushio2525/tako / macOS（Apple Silicon）で使えます。Windows 版は対応を進めています。/ この動画の画面は実際の tako です（デモ用の環境で収録）。エージェントの発言はデモ用の台本、brew・インストーラ・ログイン・GitHub の出力は再現です。 |

実画面の場面の右上には注記を出し続ける。エージェントが映る場面は「画面は実際の tako。エージェントの出力はデモ用の台本です」、
映らない場面（初回起動・`tako setup`・更新の確認）は「画面は実際の tako です（デモ用の環境で収録）」、
GitHub の場面は「画面は実際の tako。アシスタントの発言は台本、GitHub の操作は再現です」、
Claude Code が無い場面は「画面は実際の tako。インストーラとログインの出力は再現です」。

実画面でペインへ打つ文字（`record.sh`）: `利用回数を節約したい` / `はい` / `~/shop-app を登録して` /
`お願いします。名前はそのままで` / `/exit` / `Y` / `tako update check` / `tako master` /
`決済 API にリトライを入れて、README も更新して`。`tako setup`・`claude auth login` はボタン（`tako welcome next`）が打つ。

## 対話のセットアップの場面（台本と、その元）

対話のアシスタントは偽物（`demo/claude-stub.mjs` → #2864 の `agent-sim.mjs`。LLM は呼ばない）。本物の claude を実アカウントで
起こしていない。**発言は台本**（`demo/personas/setup.json`）だが、次の 3 つは本物である。

- 始まり方: `tako setup` が対話アシスタントを `claude "<最初のメッセージ>"` で起こす（`launch_setup_agent`）。台はその文を 1 通目として流す。
  画面の 1 行目「最初に setup-instructions.md を読んでください。セットアップの検出と既定値の適用が済みました。初回フローを始めます。」は
  tako が渡した文そのもの（`setup::assistant_greeting` の初回の形）
- 打つコマンド: 台本は下の表のコマンドを砂場で**実際に実行する**（`gh` だけは台）。結果は砂場の設定に入る（`record.sh` の `verify_guided` が、
  worker の effort・プロジェクトの登録・設定共有の配線と push・`dialogue_done` を確かめ、入っていなければ止まる）
- 発言の中の値: 指示ファイルの場所と突き合わせの結果・master / worker のエージェントと effort・プラン・登録したプロジェクト・
  設定共有のリポジトリと件数は、台本に書き込まず、実際の設定から読む（`demo/setup-facts.mjs`）

問いの順序と決める項目は `resources/setup/system-prompt.md` の初回フロー（Step 1 → 2 → 3 → 3.5 → 4）のとおり。
見出しの文字も同じにした（画面の「Step 2: オーケストレーション設定」は `system-prompt.md` の `## Step 2: オーケストレーション設定`）。

| 画面の順 | アシスタントの発言（台本） | 利用者が打つ文 | 実際に打つコマンド | `system-prompt.md` の元 |
|---|---|---|---|---|
| 読み取り | Read setup-instructions.md · setup-context.yaml / Read ~/.claude/CLAUDE.md / Read profiles/default.yaml · config.yaml · projects.yaml / 現状を読み込みました。初回のセットアップを、順に確かめます。 | （なし。最初のメッセージは tako が渡す） | 台がその 6 つのファイルを実際に読む（`setup-facts.mjs`） | 「最重要ルール: まず現状を読み取る」の 1〜5（6 の `pending-changes.md` は初回には無い） |
| Step 1 | Step 1: グローバル指示ファイル / ~/.claude/CLAUDE.md は、同梱の推奨ルールで新しく作成済みです（同梱推奨ルールとの差分なし）。 | | | Step 1。`setup-context.yaml` の `instruction_coverage.status` が `created_default`（CLI が既定で新規作成済み）。「差分ゼロなら「同梱推奨ルールとの差分なし」と明言する」 |
| Step 2 | Step 2: オーケストレーション設定（プロファイル default）/ セットアップ担当 = claude。master = claude / worker = claude / モデルは未指定（各 CLI の既定のモデルを使います）/ effort = high・worker モデルは master に合わせる（プラン pro に合わせた推奨）/ このまま使いますか？ 品質・速度・利用回数の重視点に合わせて調整もできます。 | `利用回数を節約したい` | | Step 2 の説明の 4 項目（setup agent・master / worker の既定エージェント・モデル未指定・effort と worker ポリシーはプラン規模の推奨）と、問い「このまま使うか、品質・速度・利用回数の重視点に合わせて調整するか」 |
| Step 2（案） | 利用回数を抑える案です（まだ変更していません）: / worker（claude）の effort を high → medium に下げる / master は high のまま（指示と検収の質は保ちます）/ 反映しますか？ | `はい` | | 「調整を希望された場合だけ変更案を出す」 |
| Step 2（反映） | 反映しました: worker（claude）の effort = medium・master は high のまま | | `tako orchestrator profiles set default --agent claude --agent-effort medium` | 「`profiles/*.yaml` — master / worker のエージェント、モデル、effort、worker ポリシーはここに書く」 |
| Step 3 | Step 3: プロジェクト登録 / 登録済みのプロジェクトは 0 件です。開発するフォルダを登録しますか？ /（任意です。あとから master に頼んでも登録できます） | `~/shop-app を登録して` | `tako orchestrator projects add --key shop-app --cwd ~/shop-app` | Step 3「未登録なら、開発プロジェクトのディレクトリを登録するか聞く（任意、スキップ可）」 |
| Step 3.5 | Step 3.5: 設定共有（任意）/ 別の PC でも、同じ AI 設定で始められます。指示ファイルと tako の設定（プロファイル・プロジェクト）を、git リポジトリ 1 本で共有する仕組みです。/ token などの秘匿情報と、この PC 固有の状態は共有の対象から外れます。/ GitHub に private リポジトリを作って配線しますか？ 名前の案: tako-config-sync | `お願いします。名前はそのままで` | | Step 3.5。`config_share.guidance` が `fresh`・`gh_can_create_repo` が true の行。「別の PC でも同じ AI 設定で始められる」という利点で説明する・秘匿情報は除外・「GitHub に private リポジトリを作って配線しますか」と聞く・名前は必ず確認する（既定案 `tako-config-sync`） |
| Step 3.5（実行） | 実行します: GitHub に private リポジトリ tako-config-sync を作り、設定を push します。/ 配線しました: ~/tako-config-sync → github.com/demo/tako-config-sync / 共有 6 ファイル・差分 0 件。2 台目は tako config link <URL> → tako config pull | | `gh repo create tako-config-sync --private`（台）→ `tako config init --remote https://github.com/demo/tako-config-sync` → `tako config status` | 「何をするかを 1 行で示して同意を得てから実行する」・代行の手順 2（`gh repo create <名前> --private` → `tako config init --remote <URL>`・public にしない）・4（`tako config status` の結果を見せる）・5（2 台目の案内） |
| Step 4 | Step 4: 完了サマリー / 変更: profiles/default.yaml（worker の effort）・projects.yaml・設定共有の配線 / tako master 司令塔を起動。やってほしいことを日本語で話しかけるだけです / tako solo worker を使わない 1 対 1 の対話 / プロファイル default: master=claude / worker=claude / effort=high（worker は medium）/ 品質重視・節約などの調整は、master に頼めば変えられます。 | `/exit` | | Step 4（変更の一覧・`tako master`・`tako solo`・プロファイルを 1 行で・「調整は master に頼めばよい」） |

Step 3.7（Claude Code の画面の UI）は「望まれたときだけ」の段なので、通していない。

### 打ったコマンドの実際の出力（砂場で流したもの・ホームは `~` に置き換え）

```
$ tako orchestrator profiles set default --agent claude --agent-effort medium
{ "name": "default", "effort": "high", "worker_model_policy": "inherit", "worker_agent": "claude",
  "worker_agents": { "claude": { "effort": "medium" } }, "master_agent": "claude", … }

$ tako orchestrator projects add --key shop-app --cwd ~/shop-app
追加しました: shop-app → ~/shop-app
専用プロファイルを作りました: ~/Library/Application Support/tako/orchestrator/profiles/shop-app.yaml（default から継承）

$ gh repo create tako-config-sync --private        （砂場の gh の台。GitHub へは出ていない）
https://github.com/demo/tako-config-sync

$ tako config init --remote https://github.com/demo/tako-config-sync
repo:   ~/tako-config-sync
remote: https://github.com/demo/tako-config-sync
書き出し: 5 files
コミット: 60538d2
リモートへ push しました

$ tako config status
repo:   ~/tako-config-sync
branch: main
差分: same 5 / differs 0 / local_only 0 / repo_only 0
```

push された中身は `claude/CLAUDE.md`・`tako/orchestrator/config.yaml`・`tako/orchestrator/profiles/default.yaml`・
`tako/orchestrator/profiles/shop-app.yaml`・`tako/orchestrator/projects.yaml`（と `README.md`・`manifest.yaml`・`.gitignore`）。
収録の回は GUI が `tako/settings.json` も書いていたので、画面の件数は 6。件数は `setup-facts.mjs` が `tako config status --json` から数える。

worker の effort を変える指定を `--agent claude --agent-effort medium` にした理由: 既定のプロファイルは
`worker_agents.claude.effort: high` を持ち、spawn の effort は「明示 → アカウント既定 → エージェント別（`worker_agents`）→
ポリシー」の順で決まる（`orchestrator::pick_launch_value`）。`--worker-effort` は `worker_model_policy: fixed` のときだけ効くので、
既定（`inherit`）のままでは実効値が変わらない（砂場で確かめた）。

## 手順ごとの根拠

語る手順・コマンド・画面は、今の main に実在するものだけにした。実画面の場面は**収録の中で実際に動かした**。

| 手順・主張 | 根拠 |
|---|---|
| `brew install --cask takushio2525/tako/tako`・Apple Silicon / macOS 11 以降 | `README.md`「インストール」。公開の cask（`takushio2525/homebrew-tako` の `Casks/tako.rb`）は 2026-11-10 時点で `version "0.8.40"`・`depends_on macos: :big_sur` |
| アプリは「アプリケーション」に入り、`tako` コマンドも使える | cask の `app "tako.app"` と `binary ".../tako"`・docs「アプリ本体が `/Applications/tako.app` に入り、`tako` コマンドも自動で使えるようになります」 |
| `tako --version` → `tako 0.8.40` | 収録用のバイナリで実行した出力そのまま。最新のリリースも v0.8.40（`gh release list`） |
| zip → 展開して `/Applications` → 警告は システム設定 →「このまま開く」。公証を受けていないため・Homebrew でも同じ | `docs/src/content/docs/getting-started/index.mdx`「つまずきポイント: Gatekeeper の警告」（#2869 の修正後） |
| Windows: `tako-vX.X.X-windows-x86_64.exe`・Windows 10 1809 以降 / x64・管理者権限は不要・警告は「詳細情報」→「実行」 | docs「方法 C: Windows」「SmartScreen の警告が出たら」・`README.md` |
| Windows 版は移植の途中。165 個のうち使える 120・一部制限 23・未確認 20・非対応 2 | `README.md`「移植は途中です」。`tako platform --platform windows` の `counts: supported 120 / degraded 23 / pending 20 / unsupported 2`（収録用のバイナリ）。**「対応済み」とは言っていない** |
| 初めて起動すると案内が出る。起動しただけでは何も実行されない | 収録 `firstrun`。案内の文は「次にやること: 初期設定をします。ボタンを押すと、新しいタブで tako setup を実行します」「最後に対話のアシスタントが開き、master / worker の設定や GitHub との連携などを相談しながら決めます」。何も走らないことは `tako welcome` の `auto.state: idle` と砂場の HOME で確かめた。FR-2.14.21 ① |
| 「セットアップを実行」で新しいタブが開き、`tako setup` が走る | 収録 `guided`: `tako welcome next`（GUI のボタンと同じ要求）→ 題 setup のタブが開き、ログインシェルへ `<GUI の隣の tako> setup` が打たれる（`welcome::launch_for_flow`）。案内は「setup のタブでセットアップを進めています。終わると次にやることが出ます」 |
| エージェントの検出・PATH・MCP の登録・指示ファイル・プロファイル | 同じ収録の `tako setup` の出力: `[設置] tako CLI: どのターミナルからも tako を打てるようにしました`・`[検出] claude: ~/.local/bin/claude（認証済み / pro）`・`[設定] Claude MCP を自動登録します`・`[default] グローバル指示ファイルを作成: ~/.claude/CLAUDE.md`・`[OK] 推奨プロファイルを保存: …/profiles/default.yaml` |
| 変更の一覧を出して、対話のアシスタントへ | 同じ出力の `セットアップ結果（変更したのはこれだけです）:` の 8 行・`セットアップが完了しました。`・`対話アシスタントを起動します。設定の変更・解説・次の一歩の相談ができます。`。案内は「setup のタブで対話のセットアップ中です。終わると「master を起動」が出ます」 |
| 対話で決めること（指示ファイル・master / worker・プロジェクト・GitHub） | 上の「対話のセットアップの場面」 |
| `gh` が未ログインなら、ログインの手順を案内 | `system-prompt.md` Step 3.5 の 3（`gh_can_create_repo` が false のとき: ローカルだけで `tako config init`・GitHub を使うなら `gh auth login` を案内）。**この分かれ道は収録していない**（字幕だけ） |
| `/exit` で対話を閉じると setup も終わり、案内が「master を起動」に変わる | 収録 `guided`: 台は `/exit` で終了コード 0 で終わる → `tako setup` が戻る → `setup.dialogue_done: true` → `tako welcome` の段が `master`。案内は「準備ができました。master（AI 司令塔）を起動して、あとは日本語で頼むだけです」。本物の Claude Code も `/exit` で終わる |
| Claude Code が無いと、`tako setup` が入れてよいかを先に聞く | 収録 `noagent`（claude の無い HOME）: `はじめてのセットアップ`・`[1/3] Claude Code をインストールします`・`実行するコマンド: curl -fsSL https://claude.ai/install.sh \| bash`・`sudo（管理者権限）は使いません`・`この内容でインストールしますか？ [Y/n]:` |
| 同意すると導入。ログインは自分で | 同じ収録: `Y` の後 `[OK] Claude Code を導入しました`、最後に `残り 1 件（ここから先は人の操作が必要です）: 1. Claude アカウントへのログイン / claude auth login / ブラウザでの操作が要るため tako は代行しません`（#1129）。**インストーラは砂場の台**（本物は取りに行っていない） |
| 案内に「ログイン」→ 済んだら「セットアップを続ける」 | 同じ収録: `tako welcome` の段が `login` →（`tako welcome next` で `claude auth login` がタブで走る）→ `continue`。案内は「次にやること: Claude アカウントにログインする（ブラウザが開きます）」→「続けて、対話でセットアップします。ボタンを押すと、setup のタブで tako setup をもう一度実行します」。**ログインは台**（ブラウザは開いていない）。「続ける」を押した先は主の流れ（対話）と同じなので撮っていない |
| 新しい版が出るとタブバーの下に「アップデートがあります」→「詳細を見る」→「今すぐ更新」→ 自動で再起動 | docs「アップデート」（#2869 の修正後）・`crates/tako-app/src/ui_text/update.rs`。**通知カードは収録していない**（収録用のバイナリが最新版と同じ 0.8.40 で、更新が無いため）ので、図解で語っている |
| `tako update check` は確認だけ → `available: false` | 収録 `update`: ペインへ `tako update check` を打った実際の出力（公開のリリース一覧を読むだけ） |
| `tako update apply`・`brew upgrade --cask takushio2525/tako/tako`・更新のあとは `tako setup` をもう一度 | docs「アップデート」「アップデート後の追従」・`README.md`。**実行はしていない**（本番を更新しないため） |
| 「master を起動」を押すと、新しいタブで司令塔が開く | `welcome::launch_for_flow` の `NextStep::Master => Launch::Master`（新しいタブで `tako master`。FR-2.14.11）。**ボタンを押した後は収録していない**（下の「未確認」） |
| `tako master` と打っても同じ・担当が同じタブに並ぶ・右パネル・報告 | 収録 `master`: ペインへ `tako master` を打つ（本物の CLI）。司令塔の台本が本物の `tako orchestrator spawn --project shop-app …` を 2 回と `tako panel --show --view orch` を打つ。完了の検知は本物の `tako orchestrator status`。**確かめた内容と報告の文面は台本** |
| `tako setup` でいつでも対話で見直せる・2 回目からは変わった所だけ | `tako setup` は端末で打つたびに最後に対話アシスタントを開く（2 回目以降フロー = メニュー）。2 回目は `[previous] 前回の設定を引き継ぎます` と差分だけを出す（v1 の収録で確認済みの出力 `セットアップ結果: 変更なし（前回の設定は最新）`） |
| `tako master --tab`・`tako solo`・tmux | `tako master --help`・`tako solo --help`・`README.md`「使い始める」・docs「事前に必要なもの」 |
| Codex CLI / Antigravity CLI でも使える（一部は Claude Code だけ） | `tako agent-support`: claude supported 67 / codex supported 38・degraded 8・pending 18・unsupported 3 / agy supported 25・degraded 5・pending 26・unsupported 11 |

### 再現したカット（実際の出力ではないもの）と、その元

- **Homebrew の場面のターミナル（10–22.5 秒）**: `brew install` を本物へ向けて打てないので、出力を再現した。
  形は docs の導入の場面（`docs/src/components/scenes/InstallMac.astro`）と同じ 4 行で、版数は公開の cask と
  最新のリリース（0.8.40）に合わせた。本物の出力の最後の行にある絵文字は入れていない。`tako --version` の行は実際の出力と同じ。
  実画面と見分けがつくよう、窓の形を実画面の窓と変え、右上に「ターミナルの出力は再現です（抜粋）」を出している
- **対話のアシスタントの発言（75–139 秒）**: 台本（`demo/personas/setup.json`）。元は `resources/setup/system-prompt.md` の
  初回フローで、対応は上の表。発言の中の値と、打たれるコマンドの結果は本物
- **GitHub の操作（110–130 秒）**: `gh repo create` は砂場の台（`demo/gh-stub.sh`）が受け、砂場の中に空のリポジトリを作って
  本物と同じ形の URL（`https://github.com/demo/tako-config-sync`）を 1 行出す。GitHub へは出ていない。その後の
  `tako config init --remote`（git init → 書き出し → コミット → push）は本物で、push は砂場の中のリポジトリが受ける
  （git の `insteadOf` で `https://github.com/demo/` を砂場へ読み替える）。`demo` は砂場の利用者名
- **インストーラの出力（150–152 秒）**: `Claude Code successfully installed!`（デモ用の環境です。本物のインストーラは
  実行していません）。公式インストーラの取得（`curl`）を砂場の台が受け、claude の台を置くだけの偽のインストーラを渡す。
  その前の「この内容でインストールしますか？」までと、後の `[OK] Claude Code を導入しました` 以降は本物の `tako setup` の出力
- **ログインの出力（160–162 秒）**: `Opening browser to sign in…` / `Login successful.`（デモ用の環境です。実際のログインは
  行っていません）。案内の「ログイン」が打つ `claude auth login` を台が受け、砂場の HOME に印を置くだけ
- **司令塔と担当の発言（187.5–225 秒）**: 台本（`demo/personas/master.json` と #2864 の `api.json` / `docs.json`）
- それ以外の図解の場面（zip / Windows / 更新の方法 / 便利なこと）は、画面の絵を作らず文字だけで語っている

### 実画面で、時間を伸ばしている所

読ませるために、収録の一部を止めて見せている（中身は変えていない）。

- 初回起動: 案内が出た 1 枚を 12.5 秒見せている（寄りを動かしているだけ）
- `tako setup`: 出力は約 1 秒で流れ切る。流れ切った後に、ペインを遡って頭（検出・登録）を 9.4 秒、戻って結果の一覧を 4.4 秒見せている
  （遡る操作は収録の中で `tako scroll` で行った）。偽の claude は、その間が読めるよう対話の前に 9 秒待つ（`CLAUDE_STUB_SETUP_BOOT_MS`）
- 対話: 各往復の終わり（反映しました・登録しました・配線しました）を 2〜4 秒止めている。問いと答えの間は実時間（収録で読む間を置いた）
- Claude Code が無いとき: 「インストールしますか？」の 1 枚を 5 秒、ログインが残った画面と「続ける」の画面を止めて寄りを動かしている。
  同意から案内が「ログイン」に変わるまでの約 10 秒（導入と残りの setup）は、導入の直後の 2.4 秒だけを見せて省いている

## 作り方

| 段 | もの | コマンド |
|---|---|---|
| 0 | 収録用の読み戻しを入れて建てる | `git apply scripts/promo/remake/promo-rec.patch`（建てるのは `record.sh` が `isolated_gui_bins visual-test` で行う） |
| 1 | 実画面を収録する | `bash scripts/promo/howto/record.sh`（場面を選ぶなら `record.sh master` など。対話より後ろの場面から始めると、対話までを撮らずに通してから撮る） |
| 2 | 本体を戻す | `git checkout crates/tako-app/src/main.rs` |
| 3 | 書き出す（舞台を組む → 音 → 映像 → 合わせる → 検査） | `bash scripts/promo/howto/build.sh` → `~/Movies/tako-promo/tako-howto-v2.mp4` |
| 確認 | 指定した秒の 1 枚だけ描く | `bash scripts/promo/howto/build.sh --stills 13,48,120 <dir>` |
| 音だけ直す | 映像は描き直さずに、音を合成して合わせ直す | `TAKO_HOWTO_REUSE_VIDEO=1 bash scripts/promo/howto/build.sh` |
| 台本との照合 | 全フレームの OCR と台本を突き合わせる | `bash scripts/promo/pii-scan.sh <mp4>` → `node scripts/promo/howto/check-script.mjs /private/tmp/tako-promo-pii/<名前>/ocr.tsv` |
| 画面を確かめる | 収録せずに立てたまま止める | `bash scripts/promo/howto/record.sh hold`（Claude Code の無い砂場は `hold-noagent`。`<砂場>/STOP` を置くと片付けて終わる） |

Playwright（`web/tako-remote` の node_modules）が今の木に無いときは、入れてある木を `TAKO_HOWTO_WEB_DIR=<木>/web` で渡せる。

| ファイル | 役割 |
|---|---|
| `record.sh` | #2864 の `record.sh` の関数（砂場・隔離 GUI・読み戻し・連番の JPEG と時刻の表・片付け）を読み込み、この動画の砂場の差分と 5 場面（`firstrun` / `guided` / `update` / `master` / `noagent`）を足す。案内の段は `tako welcome` で待ち、ボタンは `tako welcome next` で押す。対話の結果が砂場の設定へ入ったこと・収録の前後で本物の HOME と `/Applications/tako.app` が変わっていないことを確かめる |
| `demo/claude-stub.mjs` | `claude` の名前で砂場の `~/.local/bin` に置く台。版・認証（印の有無で未ログイン / ログイン済み）・ログイン・MCP の登録にだけ自分で答え、対話の起動は #2864 の `agent-sim.mjs` へ渡す。`tako setup` の対話のときは台本を `setup` にして、tako が渡した最初のメッセージを 1 通目として流す |
| `demo/personas/setup.json`・`master.json` | 対話のセットアップのアシスタントと、司令塔の台本。担当（api / docs）の台本は #2864 のものを収録時に砂場へ写して使う |
| `demo/setup-facts.mjs` | 対話の台本が画面に出す値を、実際の設定（`setup-context.yaml`・`profiles/default.yaml`・`tako orchestrator projects list`・`tako config status --json`）から読む |
| `demo/gh-stub.sh` | `gh` の名前で砂場に置く台。`--version`・`auth status`・`repo create <名前> --private` にだけ答え、GitHub へは出ない |
| `comp/index.html`・`howto.css`・`howto.js`・`timeline.json` | この動画の場面・字幕・時刻。`howto.js` は `comp.js` に 3 つ足す（注記の差し替え・図解の場面の消し方・ターミナルの打鍵） |
| `arrangement.js` | 曲の構成（100 小節）。楽器は #2864 の `bgm.mjs` のもの |
| `stage.mjs` | #2864 の `render.mjs` / `timeline.mjs` / `comp.js` / `comp.css` / `bgm.mjs` と、この動画の `comp/`・`arrangement.js` を作業用の置き場に並べる（下の節） |
| `build.sh` | 書き出しと検査（音量のそろえ方と検査の項目は #2864 の `build.sh` と同じ） |
| `check-script.mjs` | 書き出した動画の OCR と台本（`index.html`・`timeline.json`・ペインへ打つ文字）を、場面の秒ごとに突き合わせる |

新しい依存は足していない（Node・ffmpeg・`web/tako-remote` の Playwright はどれも既存）。

### #2864 の仕組みを、複製せずに使う

#2864 の `render.mjs` / `timeline.mjs` / `bgm.mjs` は、自分の隣の `comp/index.html` と `comp/timeline.json` を固定で読む。
この動画の場面を渡す口が無いので、書き出すたびに `stage.mjs` が作業用の置き場（`<出力>/work/stage/`）へ
#2864 の 4 本とこの動画の `comp/` を並べ、そこで #2864 の `render.mjs` / `bgm.mjs` を走らせる。
リポジトリに入るのはこの動画の差分だけで、v2 でも #2864 の `scripts/promo/remake/` は 1 行も変えていない。

音は 1 か所だけ差し替える。`bgm.mjs` は楽器と曲の構成が 1 本になっていて、構成は 95 秒の場面割りに合わせてある。
`stage.mjs` は「構成」の節（`// ---- 構成` から `// ---- 効果音` の手前まで）を `arrangement.js` に差し替えた `bgm.mjs` を
舞台に置く。節の見出しが見つからなければ、黙って続けずに止まる。

収録は、#2864 の `record.sh` の「本体」より前（関数の定義）だけを読み込む。偽のエージェント（`agent-sim.mjs`）も変えていない:
対話の 1 通目を流すのは `claude-stub.mjs` の側（同じプロセスの標準入力へ、打った文と Enter を渡す）で行っている。

**#2864 の側をこう変えれば、`stage.mjs` と上の読み込み方は要らなくなる**（この PR では変えていない）:

- `render.mjs` / `timeline.mjs`: comp の置き場を引数で受ける（既定は今の `comp/`）
- `bgm.mjs`: 楽器・残響・書き出しと、曲の構成を分ける（構成を引数で受ける）
- `record.sh`: 関数の定義を別ファイルへ出す（か、読み込まれたときは本体を走らせない）
- `comp.js`: `#tiles` が無いページでも動く（今は空の `#tiles` を置いて通している）。実画面の場面の注記を `timeline.json` から受ける
- `agent-sim.mjs`: 起動時の引数の文を 1 通目として流す口・台本の `capture` で 1 回の実行から複数の値を取る口

### 砂場と、本物の HOME を変えないこと

- HOME は砂場の `demo`（`/private/tmp/tako-demo/home/demo`。Claude Code の無い人の場面は `/private/tmp/tako-demo/home2/demo`）。
  tako の置き場はその下の既定と同じ形（`~/Library/Application Support/tako`）にして、画面のパスを実機と同じ見え方にしている
- 初回起動の導線は、検証用 GUI では既定で動かない（HOME へ書くため）。HOME を砂場へ向けたうえで
  `TAKO_FIRST_RUN=1` を明示して動かしている。`tako setup` と対話の台本が打つコマンドは同じ砂場の HOME にだけ書く
  （`.zprofile`・`.claude.json`・`.claude/CLAUDE.md`・`.claude/skills/tako`・`.local/bin/tako`・tako の設定・`~/tako-config-sync`）
- 収録用のバイナリ・台・偽のエージェント・node は砂場へ写して（node は繋いで）から起こす（リンク先・PATH・プロセスの引数として
  画面へ出ても、利用者のホームのパスにならない）。ボタンが打つ行が `/private/tmp/tako-demo/app/tako setup` と出るのはこのため
  （実機では `/Applications/tako.app/Contents/MacOS/tako setup`）
- **外へ出る口はすべて砂場の台**: `gh`（GitHub）・`brew`（何も入れずに断る）・Claude Code の無い場面の `curl`（公式インストーラ）。
  台が受けた呼び出しは `<出力>/guard/outside-calls.log` に残る。収録 1 回で `gh --version` / `gh auth status` / `gh repo create` /
  `gh auth token`（更新の確認が GitHub の API を読む前に聞く = `update_checker`。台は断るので、認証なしで読む）/ `curl …install.sh`（1 回）。`brew` は 0 回
- macOS のログイン zsh は `/etc/zprofile` で PATH を並べ替えて `/usr/bin` を前へ出すので、PATH の先頭に置いた `curl` の台より
  本物の `curl` が先に見つかる（tako はログインシェルの `command -v` で探す）。Claude Code の無い HOME だけ `~/.zshenv` に
  `unsetopt GLOBAL_RCS` を置いて並べ替えを止め、**立てる前に同じ聞き方で `curl` が台であることを確かめる**（違えば止まる）。
  撮った後にも、台が 1 回受けたこと・置かれた claude が台であることを確かめる
- `record.sh` は収録の前後で、本物の HOME の 35 項目（シェルの設定・エージェントの設定と MCP の登録・指示ファイル・
  tako CLI と claude のリンク・tako の設定・設定共有の配線とリポジトリ・git と gh の設定・`/Applications/tako.app`）の
  更新時刻と大きさを控えて比べ、変わっていたら終了コード 3 で終わる
- 本物の `brew install` / `upgrade`・`tako update apply` は打っていない（`tako update check` は公開のリリース一覧を読むだけ）
- 案内の「master を起動」は押していない。このボタンは `/Applications/tako.app` の CLI を先に選ぶ（`dispatch::resolve_tako_binary`）ので、
  本番の tako が入っている機で押すと、砂場の中で本番の版の CLI が走る（今の main の動きを撮ったことにならない）。
  `scripts/test-first-run-2879.sh` も同じ理由で押していない。動画ではボタンを枠で示し、同じコマンドを自分で打つ所を見せている

### 個人情報を出さない

- プロンプトは固定（`demo ~ %`）、プロジェクトは架空の `shop-app`、GitHub の利用者名は `demo`、偽の claude の認証の答えにメールや組織の欄は無い
- `tako update status` は画面に出さない（重複した CLI の置き場として実際のパスを列挙するため）
- 検査は 2 重: 全フレームの OCR（`scripts/promo/pii-scan.sh`。検査語は環境から組み立てる）と、等間隔のフレームの目視
- 砂場の外から調べものの CLI を叩くときは、自分のシェルの env を渡さない（`env -i` で HOME・PATH・`TAKO_*` だけを渡す）。
  作業中のシェルは `CLAUDE_CONFIG_DIR` を持っていることがあり、渡すと `tako config init` が本物の `~/.claude` の共有対象を読んで
  砂場のリポジトリへ写す（v2 の制作中に 1 回踏んだ。書き先は砂場の中だけで、外へは出ていない。砂場ごと消した）。
  収録の経路（隔離 GUI のペイン）は `scripts/lib/isolated-gui.sh` が `CLAUDE_CONFIG_DIR` を外すので起きない

## 検査（v2・2026-11-11）

| 項目 | 値 |
|---|---|
| 動画 | `~/Movies/tako-promo/tako-howto-v2.mp4`（36.0 MB）。v1 の `tako-howto-v1.mp4` と収録（`~/Movies/tako-promo/howto/`）は触っていない |
| 長さ / 形式 | 250.000 秒 / 1920x1080 / 30fps / H.264（High・yuv420p）+ AAC（LC）48kHz stereo（`ffprobe`。映像 7500 枚を数えた） |
| デコードの全走査 | `ffmpeg -v error -i <mp4> -f null -` の出力 0 行（映像 7500 枚と音声を最後までデコード） |
| 音量 | -15.1 LUFS / LRA 3.0 LU / True Peak -1.4 dBFS（目標 -15 LUFS。合成後に 1 回測って固定のゲイン +0.30 dB）。v1 は同じ測り方で -15.1 LUFS / LRA 2.8 LU / True Peak -1.5 dBFS |
| 台本との突き合わせ | `check-script.mjs`: 台本の 181 句（図解の文字・チップ・見出し・字幕・吹き出し・注記・ペインへ打つ文字・画面に映るコマンド）のうち 176 句が、その場面の秒の OCR（1 fps・250 フレーム・5839 行）と一致。残り 5 句は OCR の読み違い（「警」を別の字に 2 件・「案」の抜け・`\|` を `i` に・「×」を `x` に）で、フレームを目で見て確認した。見つからない句は 0 |
| 個人情報の検査（OCR） | email / home_path / tailnet / private_ip / token / uuid は 0 件。環境由来の語は 47 件で、**すべて公開情報の形**（tap 名つきの `brew install` / `brew upgrade`・GitHub の URL・docs のドメイン。それ以外は 0 件）。出ている秒は 13–21 / 23–31 / 37–41 / 171–176 / 240–249（Homebrew・zip・Windows・更新の方法・まとめ） |
| 目視 | 等間隔の 12 枚と、章ごとの 19 枚（章 1 が 2 枚・章 2 が 12 枚（対話の場面 5 枚を含む）・章 3 が 4 枚・まとめ 1 枚。`~/Movies/tako-promo/checks/tako-howto-v2/`）を 1 枚ずつ見た。字幕のはみ出し・重なり無し。HOME は `~`、プロンプトは `demo`、パスは砂場のもの、GitHub の利用者名は `demo` |
| 収録 | 隔離 GUI は `tako-vd` の上（窓 1280x720pt = 2560x1440px・位置 1270,700）。5 場面（`firstrun` 3 枚 / `guided` 109 枚・91 秒 / `update` 5 枚 / `master` 57 枚・34 秒 / `noagent` 26 枚・27 秒）。終了時に隔離 GUI・tmux の隔離ソケット・砂場を消している。本物の HOME と `/Applications/tako.app` は収録の前後で変化なし（35 項目）。記録は `~/Movies/tako-promo/howto-v2/guard/`（`record-run.log`・`before.txt`・`after.txt`・`outside-calls.log`） |
| 対話の結果 | 収録の中で `verify_guided` が確認: worker の effort = medium・`shop-app` を登録・設定共有を配線（砂場の中のリポジトリへ push）・`setup.dialogue_done: true` |
| 砂場の台が受けた呼び出し | 7 回: `gh --version` / `gh auth status`（2 回ずつ = 2 つの HOME の setup）・`gh repo create tako-config-sync --private`・`gh auth token`・`curl -fsSL https://claude.ai/install.sh -o <一時ファイル>`。`brew` は 0 回 |
| 番犬 | `shell_scripts` 39 passed / `verification_isolation_watchdog` 17 passed / `issue1490_isolated_gui_launch_watchdog` 27 passed / `virtual_display_watchdog` 20 passed / `issue1866_tmux_socket_name_watchdog` 15 passed / `context_budget` 13 passed / `issue2106_real_tmux_gate_watchdog` 8 passed（1 ignored）/ `issue2228_cargo_bin_first_exec_watchdog` 7 passed / `no_personal_data` 6 passed / `issue2360_socket_liveness_watchdog` 5 passed / `issue2399_tmux_socket_kill` 5 passed / `issue2694_isolated_data_dir_watchdog` 5 passed / `issue1926_app_nap_watchdog` 4 passed / `issue2588_stop_waits_pane_exit` 3 passed |

### 未確認

- **音を耳で聴いていない**（数値だけ。この環境に音声出力が無い）。曲の好みや効果音の大きさは試聴して決める必要がある。
  直すときは `arrangement.js` を変えて `TAKO_HOWTO_REUSE_VIDEO=1 bash scripts/promo/howto/build.sh`（映像は描き直さない）
- **本物の LLM が、台本と同じ順・同じ問いで進むかは確かめていない**（収録で本物の claude を起こしていない）。台本は
  `system-prompt.md` の初回フローを順に書き起こしたもので、Step 1 は読み方が分かれる所がある（#2893）
- **案内の「master を起動」を押した後の画面は撮っていない**（この機ではボタンが `/Applications` の版の CLI を選ぶため）。
  「新しいタブで司令塔が開きます」はコード（`Launch::Master`）と FR-2.14.11 による
- gh が未ログインのときの対話（ローカルだけで `tako config init`・`gh auth login` の案内）と、「セットアップを続ける」を押した後は撮っていない
- アプリ内の更新の通知カードは撮っていない（更新が無いため。図解で語っている）
- Windows の実機では何も確かめていない（docs と `tako platform` の記載による）

### 制作中に見つけて起票したこと（本体は直していない）

- #2891: `tako orchestrator profiles show` の `resolved_worker_effort` が、spawn の実際の effort（`worker_agents`）と食い違う。`--worker-effort` は既定のポリシーでは効かないのに黙って通る
- #2892: `tako setup` の「MCP: 設定を追加しました」の行だけ、設定ファイルの場所をホームからの絶対パスで出す
- #2893: 対話の Step 1 で、CLI が指示ファイルを既定で作った直後（`created_default`）の進め方が `system-prompt.md` に書かれていない
