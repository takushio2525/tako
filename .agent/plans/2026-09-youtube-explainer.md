# tako YouTube 解説動画 構成台本（#1081）

> YouTube 公開用の長尺解説動画「tako とは何か / どう使うか / 何がいいか」の台本・
> ナレーション原稿・収録手順・裏取りの正本。#470（106 秒ティザー）の資産
> （`.agent/plans/2026-07-promo-video.md` / `scripts/promo/`）を土台に、
> ナレーション駆動の合成パイプラインへ作り直した。
> 成果物（mp4 / 音声 / サムネ）はリポジトリ外 `~/Desktop/tako-promo/`（コミットしない）。

## 方針

- **正確性が最優先**: 訴求は AGENTS.md / docs / 実装で裏を取ったものだけ（下の「訴求と裏取り」）。
  実装中の刷新（#1077〜#1080 = リモート刷新の C〜H）には触れない。
  「できる」と言えないものは言わない（例: agy の利用上限復帰、Windows の自動 SSH 検知）
- **素材は実 tako の収録のみ**（隔離インスタンス `TAKO_ISOLATED=1` + デモ用 HOME / TAKO_DATA_DIR）。
  唯一の例外は 7 章のスマホ画面で、**PWA の UI そのものは本物**（`web/tako-remote` を
  そのまま配信）だが **API はモック**（実 daemon は tailscale serve を張る = 本番の
  Tailscale / remote 状態に触れるため）。テロップに「画面はデモ用データ」と明記する
- ナレーションは macOS 同梱の日本語 TTS（`say -v Kyoko`。この機で使える唯一の日本語音声）。
  区間の長さは**ナレーションが駆動**する（`max(min_dur, 音声秒 + 0.8)`）
- UI・テロップ・カードに絵文字を使わない（ブランド方針）
- PII ゼロ: 全フレームを Vision OCR にかけ、メール / 実ホームパス / ユーザー名 / ホスト名 /
  tailnet / トークン / UUID の各パターンが 0 件になるまで「完成」と言わない

## 構成（8 章 + オープニング / クロージング）

| # | 章 | 見せる実画面 | 主張（要約） |
|---|---|---|---|
| 0 | オープニング | 完成形（master + worker 3 体 + orch パネル） | AI エージェント時代のための GUI ターミナル |
| 1 | 課題 | 4 タブに散らばった処理 → 1 タブへ集約 | 1 作業が 4 つに分裂して散らばる。AI に操作させる繋ぎ込みも自前 |
| 2 | 思想 | 実 Claude Code が MCP でペインを割る / CLI が GUI を動かす | 1 グループ = 1 タブ / ゼロコンフィグ / AI フルコントロール |
| 3 | 導入 | brew カード → `bootstrap --dry-run` → `tako setup` → 対話アシスタント | brew 1 行。setup 一発。Claude 未導入からでも通る |
| 4 | 基本操作 | 分割 / タブ / ツリー / md ライブリロード / PDF / 画像 / コード / Code Runner | 普通のターミナル + エディタ風の確認手段 |
| 5 | AI に任せる | `tako master` → worker 3 体 → orch ビュー → かんたん表示 → 報告 | 分解・型つき指示・送達確認・監視・検収 |
| 6 | 再起動しても戻る | 終了直前 / 再起動後 | tmux バックエンドで画面ごと復元 |
| 7 | スマホから | PWA（一覧 / チャット + 承認カード）/ Remote Control の opt-in | tailnet 内固定 URL + 機器承認。Claude 公式 Remote Control へ委譲（既定 OFF） |
| 8 | Windows と OSS | 対応状況ページ / LICENSE / README / brew カード | 同一リリースに Windows 同梱。実機実測だけを「対応」。GPL-3.0 |

区間ごとの尺・テロップ・ナレーション原稿の**正本は `scripts/promo/explainer-timeline.tsv`**
（タブ区切り。`build-explainer.sh` がそのまま読む）。ナレーションの読みは TTS 向けに
外来語をカナで書いてある（`speech` 列）。テロップは通常表記（`caption` / `subtitle` 列）。

### ナレーション原稿（timeline.tsv の speech 列の写し。改稿は tsv 側で行う）

<!-- narration:begin -->
| 区間 | 種別 | 音声秒 | テロップ | ナレーション（speech） |
|---|---|---|---|---|
| `op_card` | card | 6.5 | tako / AI エージェント時代の GUI ターミナル | エーアイエージェントに開発を任せる時代。ターミナルは、どう変わるべきでしょうか。 |
| `op_hook` | clip | 11.5 | tako — AI エージェント時代の GUI ターミナル / 1 グループ = 1 タブで、エージェントの群れを集約監視する | この動画では、エーアイエージェント時代のためのジーユーアイターミナル、タコを、何ができて、どう使い、何がいいのかまで、実際の画面で解説します。 |
| `c1_card` | card | 2.2 | 1. 課題 / AI エージェント時代のターミナル | まずは、課題からです。 |
| `c1_scatter` | clip | 10.4 | 1 つの作業が 4 つに分裂する / エージェント本体 / 子エージェント / dev サーバー / ログ | クロードコードのようなエーアイエージェントを使うと、ひとつの作業が、エージェント本体、子エージェント、デブサーバー、ログへと分裂します。 |
| `c1_cycle` | clip | 11.7 | タブやウインドウに散らばる / どこで何が動いているのか、誰が止まっているのか | 既存のターミナルでは、これらがタブやウインドウに散らばります。どこで何が動いているのか、誰が止まっているのか。追いかけるだけで手間がかかります。 |
| `c1_manual` | clip | 10.9 | AI に操作させるにも、繋ぎ込みが要る / tmux・スクリプト・MCP 設定を自前で用意 | しかも、エーアイにターミナルを操作させたければ、ティーマックスやスクリプト、エムシーピーの設定を、自分で繋ぎ込む必要がありました。 |
| `c1_collect` | clip | 9.3 | tako の答え: 1 グループ = 1 タブ / 散らばっていたものを、1 つのタブに集約する | タコの答えはシンプルです。ひとつの作業グループを、ひとつのタブに集約する。そのために作られたターミナルです。 |
| `c2_card` | card | 2.9 | 2. tako の思想 / 集約監視 / ゼロコンフィグ / AI フルコントロール | タコの設計思想は 3 つです。 |
| `c2_agent1` | clip | 18.8 | 1. 1 グループ = 1 タブ / AI が起こした子プロセスは、同じタブのペインに生える | ひとつ目、1 グループ 1 タブ。タコの中でクロードコードに日本語で頼むと、エーアイは内蔵のエムシーピーサーバーを通してペインを分割し、デブサーバーを起動し、リードミーをプレビューで開きます。生えたペインは全部、同じタブの中です。 |
| `c2_agent2` | clip | 15.0 | 2. ゼロコンフィグ / 内蔵 MCP サーバー。初回の登録以外に設定は要らない | ふたつ目、ゼロコンフィグ。エムシーピーサーバーはタコに内蔵されていて、初回に登録するだけで、以後どのプロジェクトでも設定は要りません。公開しているツールは 140 個以上あります。 |
| `c2_control` | clip | 11.6 | 3. AI フルコントロール / UI でできることは、すべて CLI / MCP からもできる | みっつ目、エーアイフルコントロール。人が画面でできることは、すべてシーエルアイとエムシーピーからもできる。これを不変条件として作られています。 |
| `c2_control2` | clip | 12.3 | テーマも表示モードもパネルも、コマンドで / tako theme / tako ui-mode / tako panel | テーマの切り替え、表示モード、サイドパネルの開閉まで、コマンドひとつで動きます。だからエーアイは、人と同じ画面を、同じ手段で組み立てられるのです。 |
| `c3_card` | card | 2.6 | 3. 導入 / tako setup 一発 | 導入の流れを見ていきます。 |
| `c3_brew` | clip | 11.5 | brew install --cask takushio2525/tako/tako / Homebrew なら 1 行。tako CLI も PATH へ入る | インストールはホームブリューなら 1 行です。アプリ本体と一緒に、タココマンドもパスに入ります。ジップを展開して置くだけの方法もあります。 |
| `c3_bootstrap` | clip | 19.3 | Claude Code が無くても、tako setup から始められる / インストール → PATH → ログインの 3 段を案内・代行 | エーアイ連携の設定は、タコ セットアップ 一発です。クロードコードがまだ入っていない環境でも、公式のインストール、パスの設定、ログインの案内までを、タコが順に進めます。何をどこに入れるかは、実行前に必ず表示します。 |
| `c3_setup` | clip | 18.3 | 質問ゼロで検出し、対話アシスタントが立ち上がる / 認証済み CLI が 1 つなら、人間への質問はない | 導入済みなら、検出は質問ゼロで終わります。クロード、コーデックス、ジェミニ系のシーエルアイを見つけ、プランに合わせたプロファイルを作り、エムシーピーの登録まで済ませます。そのあと、対話アシスタントが自動で立ち上がります。 |
| `c3_ask` | clip | 14.1 | あとは日本語で相談するだけ / 反映するのは同意した項目だけ | 設定の相談は日本語でできます。アシスタントは現状の設定を読んでから答え、反映するのは同意した項目だけです。設定ファイルを自分で開く必要はありません。 |
| `c4_card` | card | 4.4 | 4. 基本操作 / タブ / ペイン / ファイルツリー / プレビュー | ここからは、普通のターミナルとしての使い勝手です。 |
| `c4_split` | clip | 11.8 | Cmd+D で右、Cmd+Shift+D で下に分割 / iTerm2 と同じキー。境界のドラッグ、ペインの並べ替えも | ペイン分割は、コマンド ディー で右、コマンド シフト ディー で下。アイタームと同じ操作感で、境界線のドラッグやペインの並べ替えもできます。 |
| `c4_tab` | clip | 7.7 | タブ = 作業グループ / Cmd+T で新規、Cmd+1〜9 で切替 | タブは作業グループの単位です。プロジェクトごとにタブを分け、その中を分割して使います。 |
| `c4_tree` | clip | 8.4 | Cmd+B でファイルツリー / タブ内の作業ディレクトリを自動でワークスペース表示 | コマンド ビー でファイルツリー。タブの中で開いている作業ディレクトリが、自動でワークスペースとして並びます。 |
| `c4_md` | clip | 8.2 | Markdown はレンダリング表示 / ファイルをクリックすると隣のペインに開く | ファイルをクリックすると、隣のペインにプレビューが開きます。マークダウンは既定でレンダリング表示です。 |
| `c4_reload` | clip | 4.5 | ライブリロード / 編集すると、即反映 | ファイルが書き換わると、プレビューは即座に追従します。 |
| `c4_pdf` | clip | 7.6 | PDF もペイン内で / テキスト選択 / 目次ジャンプ / ズーム | ピーディーエフもペインの中でそのまま。テキストの選択や目次からのジャンプ、ズームもできます。 |
| `c4_image` | clip | 1.7 | 画像プレビュー / PNG / JPEG / SVG / GIF / WebP | 画像も同じです。 |
| `c4_code` | clip | 8.2 | 210 以上の形式にシンタックスハイライト / 軽い編集と保存もできる | コードは 210 以上の形式にハイライトが効き、その場で軽く編集して保存することもできます。 |
| `c4_run` | clip | 11.3 | Code Runner / 再生ボタンでスクリプトを新ペインで実行 | スクリプトは再生ボタンひとつで、新しいペインに分割して実行されます。成果物の確認まで、ターミナルの外に出ずに済みます。 |
| `c5_card` | card | 5.2 | 5. AI に任せる / tako master と worker | ここが本題です。エーアイに作業を任せる、オーケストレーション。 |
| `c5_master` | clip | 10.2 | tako master で司令塔を起動 / 今いるペインが master になる | タコ マスター と打つと、今いるペインが司令塔、マスターになります。あなたがやることは、マスターに日本語で話しかけることだけです。 |
| `c5_spawn` | clip | 19.2 | 依頼を分解し、worker を隣のペインに立てる / 1 worker = 1 成果物。指示は型で、送達確認つき | 依頼を受けたマスターは、作業を成果物ごとに分解し、担当のエーアイ、ワーカーを隣のペインに立ち上げます。ワーカーへの指示は、背景、スコープ、受け入れ条件、検証手順まで埋めた型で渡され、送達確認つきで届きます。 |
| `c5_grid` | clip | 14.3 | 同じタブに並ぶ。進捗は画面でそのまま見える / master の取り分を保ったまま、worker 領域をグリッド配置 | ワーカーは同じタブの中にグリッドで並びます。今なにを考え、どのコマンドを打っているか、リアルタイムで見えます。気になるペインをクリックすれば、直接口を出すこともできます。 |
| `c5_orch` | clip | 8.1 | 右パネルの orch ビューで俯瞰 / 親子関係 / 稼働時間 / コンテキスト使用率 | 右パネルのオーケストレーションビューでは、マスターとワーカーの親子関係や稼働状況を俯瞰できます。 |
| `c5_gui` | clip | 15.6 | かんたん表示（GUI モード） / Claude と対話中のペインはチャット画面に。変わるのは見せ方だけ | 黒い画面が苦手なら、かんたん表示に切り替えられます。クロードと対話中のペインはチャット画面になり、空のペインはボタンになります。変わるのは見せ方だけで、裏のターミナルは動いたままです。 |
| `c5_report` | clip | 16.6 | 完了・入力待ち・消滅を自動検知し、検収してから報告 / 監視・回収・片付けも master の仕事 | マスターはワーカーの完了や入力待ちを自動で検知し、完了報告を鵜呑みにせず、証拠を検査してから結果を届けます。ワーカーの起動、監視、片付け。段取りは全部マスターが持ちます。 |
| `c5_solo` | clip | 5.2 | 1 対 1 でよければ tako solo / worker を立てず、その AI 自身が手を動かす | 分担が要らなければ、タコ ソロ で 1 対 1 の対話もできます。 |
| `c6_card` | card | 4.3 | 6. 再起動しても戻る / tmux バックエンドによる永続化 | 長い作業を任せたときに効くのが、永続化です。 |
| `c6_before` | clip | 11.6 | tako を閉じる直前 / tmux が入っていれば、全ペインは tmux セッション経由で動く | タコは、ティーマックスが入っていれば、全ペインをそのセッション経由で動かします。エージェントもデブサーバーも走ったまま、タコを終了してみます。 |
| `c6_after` | clip | 15.4 | 再起動すると、画面ごと戻ってくる / 実行中プロセス / スクロールバック / タブ構成 / ペイン ID | 再起動すると、タブ構成もペインも、画面の中身ごと復元されます。プロセスは裏で生き続けていたので、作業は途切れません。一晩かかる作業を任せて寝る、が現実的にできます。 |
| `c7_card` | card | 4.2 | 7. スマホから / tako remote と Remote Control | 席を離れても、進み具合は手元で見られます。 |
| `c7_pwa1` | clip | 16.9 | tako remote — Tailscale 内の固定 URL / 画面はデモ用データ。到達できるのは同じ tailnet の端末だけ | タコ リモート は、テイルスケールのネットワークの中だけに存在する固定ユーアールエルで、タコの画面をスマホのブラウザに出します。通信は端から端まで暗号化され、公開インターネットには存在しません。 |
| `c7_pwa2` | clip | 13.6 | エージェントの会話も、承認カードも / 二層認証: tailnet identity + Mac 画面での機器承認 | ペインの一覧から、エージェントの会話を読んだり、権限確認の承認カードにその場で答えたりできます。端末は、マックの画面で承認するまで何も見られません。 |
| `c7_rc` | clip | 20.2 | Claude 公式 Remote Control への委譲（opt-in） / claude.ai / Claude モバイルアプリから会話を操作。既定は OFF | さらに、プロファイルでオプトインすると、タコが起動するクロードを、クロード公式のリモートコントロールへ繋げます。クロード ドット エーアイ や モバイルアプリから、その会話を直接操作できます。会話がアンソロピックのサーバーにも保存されるため、既定はオフです。 |
| `c8_card` | card | 3.6 | 8. Windows と OSS / 導入方法とリンク | 最後に、対応環境と入手先です。 |
| `c8_win` | clip | 18.6 | Windows 版も同じリリースに同梱 / 対応状況は、実機で確かめたものだけを「対応」と書く | タコはマックオーエス先行で開発し、ウィンドウズ版はインストーラーとポータブル版を、同じリリースに同梱しています。どの機能が使えるかは、実機で確かめたものだけを対応と書く方針で、ドキュメントに自動生成の表があります。 |
| `c8_oss` | clip | 7.6 | GPL-3.0-or-later のオープンソース / github.com/takushio2525/tako | タコは、ジーピーエル バージョン 3 のオープンソースです。ソースコードはギットハブで公開しています。 |
| `c8_get` | clip | 9.5 | brew install --cask takushio2525/tako/tako / ドキュメント: tako-docs.pages.dev | 導入はホームブリューで 1 行。ドキュメントサイトには、セットアップからオーケストレーションの実践ガイドまで揃っています。 |
| `outro_card` | card | 6.2 | tako / github.com/takushio2525/tako | エーアイに任せる開発を、ひとつの画面で。タコを、ぜひ試してみてください。 |
<!-- narration:end -->

## 訴求と裏取り

「存在しない機能を言わない」ための表。主張ごとに参照先を 1 つ以上置く。

| 章 | 主張 | 裏取り |
|---|---|---|
| 1 | AI エージェント利用で 1 作業が「本体 + 子エージェント + dev サーバー + ログ」に分裂しタブ / ウインドウに散らばる | `README.md`「なぜ tako?」 |
| 1 | AI に操作させるには tmux / スクリプト / MCP 設定を自前で繋ぎ込む必要があった | `docs/src/content/docs/features/orchestration.md`「なぜターミナルに組み込むのか」 |
| 1, 2 | 1 グループ = 1 タブ。AI が起こした子プロセスのペインは同じタブに生える | `README.md`「なぜ tako?」/ `docs/.../features/tabs-and-panes.md`「AI からの操作」（呼び出し元と同じタブに生成） |
| 2 | 内蔵 MCP サーバー。初回登録だけで以後どのプロジェクトでも設定不要 | `docs/.../features/mcp-server.md`「仕組み」/ `README.md`「Claude Code 連携」 |
| 2 | 公開ツールは 140 個以上 | `crates/tako-control/testdata/mcp_tools_full_snapshot.json`（2026-09-03 時点で 144 ツール。docs の「128 個」は古い） |
| 2 | UI でできることはすべて CLI / MCP からもできる（不変条件） | `AGENTS.md`「機能実装時の必須ルール」設計原則 5 / `docs/.../features/mcp-server.md`「設計思想: AI フルコントロール」 |
| 2 | テーマ / 表示モード / パネルがコマンドで切り替わる | `tako theme` / `tako ui-mode` / `tako panel`（`tako --help`。収録で実演） |
| 3 | Homebrew 1 行でアプリと CLI が入る。zip 手動も可 | `README.md`「インストール」/ `docs/.../getting-started/index.md`「1. インストール」 |
| 3 | Claude 未導入でも `tako setup` から始められる（インストール → PATH → ログインの 3 段。実行前に計画を表示） | `AGENTS.md` コマンド表「自動セットアップ」（#868 / #1057）/ `tako setup bootstrap install --dry-run`（収録で実演） |
| 3 | 認証済み CLI が 1 つなら質問ゼロ。claude / codex / agy を検出、プラン別プロファイル生成、MCP 登録 | `docs/.../getting-started/index.md`「3. tako setup」/ `docs/.../getting-started/quickstart.md` |
| 3 | 検出後に対話アシスタントが自動起動。現状を読んでから答え、反映は同意した項目だけ | `crates/tako-cli/src/setup.rs`（`launch_setup_agent`）/ `resources/setup/system-prompt.md`（#470 台本の「③ setup」節に精査記録） |
| 4 | Cmd+D / Cmd+Shift+D の分割、境界ドラッグ、ペイン並べ替え | `docs/.../features/tabs-and-panes.md` |
| 4 | Cmd+B でファイルツリー。タブ内の cwd を自動でワークスペース表示 | `docs/.../features/file-preview.md`「ファイルツリー」 |
| 4 | md レンダリング / ライブリロード / PDF（選択・目次・ズーム）/ 画像 / 210+ 形式のハイライト / 軽い編集 / Code Runner | `docs/.../features/file-preview.md` / `AGENTS.md` コマンド表（#233 / #453 / #124 / #126） |
| 5 | `tako master` で今いるペインが司令塔になる。日本語で話しかけるだけ | `docs/.../features/orchestrator.md`「tako master で何が起きるか」 |
| 5 | 依頼を成果物ごとに分解（1 worker = 1 成果物）。指示は型で、送達確認つき | `docs/.../features/orchestration.md`「品質は仕組みで作り込まれる」/「監視と回収まで自動で起きる」 |
| 5 | worker は同じタブにグリッド配置。master の取り分を保つ | `AGENTS.md` コマンド表「worker spawn のレイアウト設定」（#165）/ `docs/.../features/orchestration.md` |
| 5 | 右パネル orch ビューで親子関係・稼働時間・ctx 使用率を俯瞰 | `docs/.../features/orchestration.md`（orch-panel-detail の説明）/ `tako panel --view orch` |
| 5 | かんたん表示: Claude 対話ペインがチャット画面、空ペインがボタン。表示だけの切替 | `docs/.../features/gui-mode.md` |
| 5 | 完了・入力待ち・消滅を自動検知。証拠を検査してから報告。片付けも master | `docs/.../features/orchestration.md` / `docs/.../features/orchestrator.md`「仕組みの補足」 |
| 5 | `tako solo` は worker を立てず 1 対 1 | `docs/.../features/orchestration.md`「1 対 1 で十分なら tako solo」 |
| 6 | 全ペインを tmux セッション経由で動かし、再起動で実行中プロセス・画面内容・タブ構成を復元。ペイン ID も維持 | `docs/.../features/tmux-backend.md` / `README.md`「セッション永続化」 |
| 7 | `tako remote` は tailnet 内だけの固定 URL。WireGuard で端から端まで暗号化。公開インターネットに存在しない | `docs/.../features/remote.md`「一言でいうと」「層① Tailscale identity」 |
| 7 | Mac 画面で承認するまで端末は何も見られない（機器ペアリング） | `docs/.../features/remote.md`「層② 機器ペアリング」 |
| 7 | エージェントの会話を読める。承認カードにその場で答えられる（Interact 以上） | `docs/.../features/remote.md`「スマホでの見え方」 |
| 7 | プロファイルの opt-in で `--remote-control` を付け、claude.ai / モバイルアプリから会話を操作できる。既定 OFF（transcript が Anthropic のサーバーにも保存） | `AGENTS.md` コマンド表「スマホから会話を操作する（Claude 公式 Remote Control。#1068 / #1069）」/ `tako orchestrator profiles set --help` |
| 8 | Windows 版はインストーラー + ポータブル zip を同じリリースに同梱（v0.7.9 以降） | `docs/.../getting-started/index.md`「方法 C: Windows」/ `AGENTS.md`「両 OS 同時リリース（#965）」 |
| 8 | 対応状況は実機で確かめたものだけを「対応」と書く。docs の表は自動生成 | `docs/.../windows-support.md`（生成物）/ `AGENTS.md`「プラットフォーム対応マトリクス（#515 / #591）」 |
| 8 | GPL-3.0-or-later のオープンソース。GitHub で公開 | `LICENSE` / `Cargo.toml` の `license` / `README.md` |
| 8 | ドキュメントサイト tako-docs.pages.dev | `README.md` 冒頭のリンク |

### 言わなかったこと（裏が取れない・実装中・過大になる）

- リモート刷新の後続（PWA からの「Claude で開く」/ スマホから master 起動 / リモートファイル / SSH 切替 = #1077〜#1080）
- Windows でのリモート（`tako remote` の tailscale serve は Windows 実機で未測 = #971）や自動 SSH 検知（Windows は argv が採れない）
- agy の利用上限自動復帰（agy は窓つき上限を持たない = Unsupported）
- Remote Control の実機画面（スマホ側の claude.ai アプリは収録対象外。CLI での opt-in だけを見せる）

## 収録パイプライン

| スクリプト | 役割 |
|---|---|
| `scripts/promo/lib.sh` | 隔離起動・デモ環境・ウインドウ単体キャプチャ（#470）+ **16:9 ウインドウ seed / ビート記録 / 追加素材**（#1081） |
| `scripts/promo/record-explainer.sh <scene\|all>` | 9 シーンの収録。CLI 操作の瞬間を `<scene>-beats.tsv` へ記録 |
| `scripts/promo/record-pwa.cjs` | PWA（`web/tako-remote`）を iPhone ビューポートでモック API つきに撮る（連番スクショ → mp4） |
| `scripts/promo/narrate.sh` | timeline.tsv の speech 列 → `say -v Kyoko` → 48kHz wav + durations.tsv |
| `scripts/promo/make-bgm.py` | BGM 合成（`TAKO_BGM_TOTAL=660 TAKO_BGM_PROFILE=explainer` で薄い長尺版） |
| `scripts/promo/titlecard.swift` / `caption.swift` | 章カード（全面）/ 下段テロップ（半透明パネル） |
| `scripts/promo/build-explainer.sh` | 切り出し → カード / テロップ → 連結 → ナレーション配置 → BGM ダッキング → mp4 + 章タイムスタンプ |
| `scripts/promo/ocr-frames.swift` / `pii-scan.sh` | 全フレーム Vision OCR → PII パターン検査 |
| `scripts/promo/thumbnail.swift` | サムネイル PNG（1280x720） |

実行順:

```sh
/Applications/tako.app を最新に（scripts/build-app.sh --install）
cd web/tako-remote && npx vite --port 5199 --strictPort &      # PWA 用 dev サーバー
scripts/promo/record-explainer.sh all                           # 9 シーン（実 claude を使う 3 つは後半）
NODE_PATH=web/tako-remote/node_modules node scripts/promo/record-pwa.cjs
scripts/promo/narrate.sh
TAKO_BGM_TOTAL=660 TAKO_BGM_PROFILE=explainer scripts/promo/make-bgm.py ~/Desktop/tako-promo/audio/bgm-explainer.wav
scripts/promo/build-explainer.sh                                # → ~/Desktop/tako-promo/tako-explainer-v1.mp4
scripts/promo/pii-scan.sh ~/Desktop/tako-promo/tako-explainer-v1.mp4
```

### #470 から引き継いだ罠と、今回わかったこと

- #470 の技術制約（`screencapture -v` は黒 / 画面全体を撮らずウインドウ単体 / 隠れると描画停止 →
  定期 activate + 異なるフレーム数検査 / デモ HOME はログインキーチェーンが外れる → 検索リストに
  実ユーザーのキーチェーンを指定 / `tko` に TAKO_DATA_DIR / `--await-prompt` は生成中を中断しうる）
  はすべて有効。詳細は `.agent/plans/2026-07-promo-video.md`
- **16:9 ウインドウ**: tako-app は初回起動時に `layout.json` の `window` フレームを読む
  （`TAKO_SELF_TEST` 以外）。タブが空のレイアウトは復元段で「空」として拒否され新規ワークスペース
  になるので、隔離 data_dir に `{"version":1,"active_tab":0,"tabs":[],"window":{...960x540...}}` を
  置くだけで 960x540pt（Retina 1920x1080px）で開く（`promo_seed_window_frame`）
- **claude 2.1.258 の起動画面にメールアドレスは出ない**（デモ HOME で実測。出るのはプラン名・
  cwd・Remote Control の案内 `/rc active`）。#470 v3 当時の「バナーにメールが出る」は現行では
  該当しないが、`promo_wait_pii_clear` の検査は残してある
- **Playwright の recordVideo は iPhone エミュレーションと組むとページが左上 1/4 に描かれる**
  （実測）。連番スクリーンショット（4 fps・3x）→ ffmpeg の方が確実で鮮明
- PWA の term ビューは WebSocket の画面プッシュ前提なので、モックでは読み込み中のまま。撮らない
- 文字サイズはペイン既定の 13 では 1080p で小さいので `tako theme --size 15` で撮る
- `say -v Kyoko` の `-r` は 160 と 175 で尺が変わらなかった（実測）。180 で使う。ピークは -13dB 程度と
  小さいので合成時に +7dB（limiter つき）持ち上げる
- **かんたん表示のチャット判定は器（tmux バックエンド）が要る**: `chat_session` の材料
  `live_claude_sessions_by_backend` は tmux ペインの pid 対応付けに乗るので、`TAKO_PERSIST=0` の
  隔離では claude ペインが永久に terminal のまま（実測: persist=0 は 40 秒待っても terminal /
  persist=1 は 5 秒で chat）。master 章だけ persist=1 で撮る
- **再起動復元の絵は器を残して止める**: `promo_stop_isolated` は tmux `kill-server` まで行うので、
  それを挟むと再起動が「tmux 再 attach 0 / 新規シェル 3」になる（persist.log で実測）。
  前半のあとは `promo_stop_isolated_keep_sessions`（アプリだけ SIGTERM）
- master（sonnet）は既定プロファイルの effort=max だと最初の spawn まで 40 秒考え、3 体の spawn に
  2 分超かかる（1 体 40 秒前後 = prompt 送達待ち）。収録では `--effort medium` + 尺 420 秒 +
  「ペインが 4 つ揃うまで」「chat が出るまで」「master が idle に戻るまで」を待つ形にした
- ffmpeg は既定で stdin を読むので `while read` ループの中で呼ぶと tsv の次の行を食う → `-nostdin`
- bash の `IFS=$'\t' read` は連続タブを 1 つに潰すので、空欄のある tsv は列がずれる →
  `promo_timeline_rows` で空欄を `-` に埋めてから読む
- docs の md は frontmatter（`--- title ---`）がプレビューで本文として描かれる → 見出しへ置換して写す
- コマンドカード（`tako show-command`）は画面下部に出るので、その区間のテロップは上に置く
  （caption の先頭 `^`）
- `$id（` のように変数の直後に全角を置くと bash が変数名に取り込んで `set -u` で落ちる
  （`shell_scripts` 番犬が CI で落とす）。`${id}（` と書く

## 完成物と検査結果（v1・2026-09-04）

| 項目 | 値 |
|---|---|
| 動画 | `~/Desktop/tako-promo/tako-explainer-v1.mp4`（9:47 = 587.3 秒 / 1920x1080 / 30fps / H.264 + AAC 48kHz / 47 区間） |
| 章タイムスタンプ | `~/Desktop/tako-promo/tako-explainer-chapters.txt`（説明文へ転記済み） |
| 音声 | ナレーション 47 区間 488.7 秒（`say -v Kyoko`）+ BGM 660 秒（`make-bgm.py` explainer） |
| サムネ | `tako-explainer-thumb-a.png` / `tako-explainer-thumb-b.png`（1280x720） |
| 説明文 | `tako-explainer-description.txt` |
| PII 検査 | 587 フレーム（1 fps）を Vision OCR → 認識行 32,531。email / home_path / tailnet / private_ip / token / uuid / 環境由来語（5 語）の 7 カテゴリすべて **0 件** |
| 機械検査 | 無音 8 秒以上なし / 黒は章カードのフェード（0.3〜0.5 秒 × 20）のみ / -18 LUFS（v1 時点。以後 +3dB へ調整）|

素材は `~/Desktop/tako-promo/scenes/<scene>-raw.mp4` + `<scene>-beats.tsv`（#470 の旧素材は `scenes/old-470/`）。

## 付随物（YouTube）

### タイトル案（3 つ）

1. tako — AI エージェント時代の GUI ターミナル【Claude Code を 1 つのタブで動かす】
2. Claude Code の並列作業を 1 画面で監視する OSS ターミナル「tako」を解説
3. AI に開発を任せるためのターミナル tako｜導入から master / worker の使い方まで 10 分で

### 説明文（章立てタイムスタンプ付き）

<!-- description:begin -->
```
tako は、Claude Code のような AI エージェントと、その子エージェント・dev サーバー・ログを
「1 グループ = 1 タブ」で集約監視するための、オープンソース（GPL-3.0）の GUI ターミナルです。
この動画では「tako とは何か」「どう使うか」「何がいいか」を、実際の画面だけで解説します。

■ インストール（macOS / Apple Silicon）
brew install --cask takushio2525/tako/tako
Windows 版（インストーラー / ポータブル zip）と macOS の zip は GitHub Releases から:
https://github.com/takushio2525/tako/releases

■ リンク
GitHub: https://github.com/takushio2525/tako
ドキュメント: https://tako-docs.pages.dev/
セットアップ: https://tako-docs.pages.dev/getting-started/
クイックスタート: https://tako-docs.pages.dev/getting-started/quickstart/
オーケストレーションとは: https://tako-docs.pages.dev/features/orchestration/
tako master 実践ガイド: https://tako-docs.pages.dev/features/orchestrator/
リモートアクセス: https://tako-docs.pages.dev/features/remote/
Windows 対応状況: https://tako-docs.pages.dev/windows-support/

■ 章
00:00 オープニング
00:20 1. 課題 — AI エージェント時代のターミナル
01:10 2. tako の思想 — 集約監視 / ゼロコンフィグ / AI フルコントロール
02:26 3. 導入 — brew 1 行と tako setup
03:45 4. 基本操作 — タブ / ペイン / ファイルツリー / プレビュー
05:14 5. AI に任せる — tako master と worker
07:16 6. 再起動しても戻る — tmux バックエンド
07:50 7. スマホから — tako remote と Remote Control
08:49 8. Windows と OSS — 導入方法とリンク
09:37 まとめ

■ 注記
・ナレーションは合成音声（macOS の日本語音声）です
・7 章のスマホ画面は tako remote の実際の UI に、デモ用データを流し込んで撮影しています
・収録は tako v0.8.3 / Claude Code 2.1.258（macOS）。機能や画面は今後のバージョンで変わることがあります

#tako #ClaudeCode #AIエージェント #ターミナル #Rust #オープンソース
```
<!-- description:end -->

### サムネイル案

- 案 A: 完成形（master + worker 3 体 + orch パネル）を背景に「AI エージェントを / 1 つのタブで動かす」
- 案 B: かんたん表示（チャット画面）を背景に「Claude Code の司令塔を / ターミナルに」
- 生成: `thumbnail.swift`（背景フレーム + 2 行見出し + 小ラベル）。出力 `~/Desktop/tako-promo/tako-explainer-thumb-{a,b}.png`
