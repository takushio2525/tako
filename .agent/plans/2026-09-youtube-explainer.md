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
- ナレーションは VOICEVOX ENGINE のずんだもん / ノーマル（v3 以降。選定の根拠は「声の選定」節）。
  v2 までは macOS 同梱の `say -v Kyoko` で、抑揚が狭く棒読みに聞こえたため降りた。
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
| 5 | AI に任せる | `tako master` → worker 3 体 → orch ビュー → 報告 → **かんたん表示の手順デモ** | 分解・型つき指示・送達確認・監視・検収 / 初心者は同じことをボタンで |
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
| `op_card` | card | 6.8 | tako / AI エージェント時代の GUI ターミナル | エーアイエージェントに開発を任せる時代。ターミナルは、どう変わるべきでしょうか。 |
| `op_hook` | clip | 12.7 | tako — AI エージェント時代の GUI ターミナル / 1 グループ = 1 タブで、エージェントの群れを集約監視する | この動画では、エーアイエージェント時代のためのジーユーアイターミナル、タコを、何ができて、どう使い、何がいいのかまで、実際の画面で解説します。 |
| `c1_card` | card | 2.5 | 1. 課題 / AI エージェント時代のターミナル | まずは、課題からです。 |
| `c1_scatter` | clip | 11.4 | 1 つの作業が 4 つに分裂する / エージェント本体 / 子エージェント / dev サーバー / ログ | クロードコードのようなエーアイエージェントを使うと、ひとつの作業が、エージェント本体、子エージェント、デブサーバー、ログへと分裂します。 |
| `c1_cycle` | clip | 11.4 | タブやウインドウに散らばる / どこで何が動いているのか、誰が止まっているのか | 既存のターミナルでは、これらがタブやウインドウに散らばります。どこで何が動いているのか、誰が止まっているのか。追いかけるだけで手間がかかります。 |
| `c1_manual` | clip | 11.1 | AI に操作させるにも、繋ぎ込みが要る / tmux・スクリプト・MCP 設定を自前で用意 | しかも、エーアイにターミナルを操作させたければ、ティーマックスやスクリプト、エムシーピーの設定を、自分で繋ぎ込む必要がありました。 |
| `c1_collect` | clip | 9.2 | tako の答え: 1 グループ = 1 タブ / 散らばっていたものを、1 つのタブに集約する | タコの答えはシンプルです。ひとつの作業グループを、ひとつのタブに集約する。そのために作られたターミナルです。 |
| `c2_card` | card | 3.3 | 2. tako の思想 / 集約監視 / ゼロコンフィグ / AI フルコントロール | タコの設計思想は 3 つです。 |
| `c2_agent1` | clip | 21.7 | 原則 1: 1 グループ = 1 タブ / AI が起こした子プロセスは、同じタブのペインに生える | ひとつ目、1 グループ 1 タブ。タコの中でクロードコードに日本語で頼むと、エーアイは内蔵のエムシーピーサーバーを通してペインを分割し、デブサーバーを起動し、リードミーをプレビューで開きます。生えたペインは全部、同じタブの中です。 |
| `c2_agent2` | clip | 15.3 | 原則 2: ゼロコンフィグ / 内蔵 MCP サーバー。初回の登録以外に設定は要らない | ふたつ目、ゼロコンフィグ。エムシーピーサーバーはタコに内蔵されていて、初回に登録するだけで、以後どのプロジェクトでも設定は要りません。公開しているツールは 140 個以上あります。 |
| `c2_control` | clip | 11.7 | 原則 3: AI フルコントロール / UI でできることは、すべて CLI / MCP からもできる | みっつ目、エーアイフルコントロール。人が画面でできることは、すべてシーエルアイとエムシーピーからもできる。これを不変条件として作られています。 |
| `c2_control2` | clip | 13.0 | テーマも表示モードもパネルも、コマンドで / tako theme / tako ui-mode / tako panel | テーマの切り替え、表示モード、サイドパネルの開閉まで、コマンドひとつで動きます。だからエーアイは、人と同じ画面を、同じ手段で組み立てられるのです。 |
| `c3_card` | card | 2.7 | 3. 導入 / tako setup 一発 | 導入の流れを見ていきます。 |
| `c3_brew` | clip | 11.7 | brew install --cask takushio2525/tako/tako / Homebrew なら 1 行。tako CLI も PATH へ入る | インストールはホームブリューなら 1 行です。アプリ本体と一緒に、タココマンドもパスに入ります。ジップを展開して置くだけの方法もあります。 |
| `c3_bootstrap` | clip | 20.5 | Claude Code が無くても、tako setup から始められる / インストール → PATH → ログインの 3 段を案内・代行 | エーアイ連携の設定は、タコ セットアップ 一発です。クロードコードがまだ入っていない環境でも、公式のインストール、パスの設定、ログインの案内までを、タコが順に進めます。何をどこに入れるかは、実行前に必ず表示します。 |
| `c3_setup` | clip | 18.6 | 質問ゼロで検出し、対話アシスタントが立ち上がる / 認証済み CLI が 1 つなら、人間への質問はない | 導入済みなら、検出は質問ゼロで終わります。クロード、コーデックス、ジェミニ系のシーエルアイを見つけ、プランに合わせたプロファイルを作り、エムシーピーの登録まで済ませます。そのあと、対話アシスタントが自動で立ち上がります。 |
| `c3_ask` | clip | 13.4 | あとは日本語で相談するだけ / 反映するのは同意した項目だけ | 設定の相談は日本語でできます。アシスタントは現状の設定を読んでから答え、反映するのは同意した項目だけです。設定ファイルを自分で開く必要はありません。 |
| `c4_card` | card | 4.4 | 4. 基本操作 / タブ / ペイン / ファイルツリー / プレビュー | ここからは、普通のターミナルとしての使い勝手です。 |
| `c4_split` | clip | 14.6 | Cmd+D で右、Cmd+Shift+D で下に分割 / iTerm2 と同じキー。境界のドラッグ、ペインの並べ替えも | ペイン分割は、コマンド ディー で右、コマンド シフト ディー で下。アイタームと同じ操作感で、境界線のドラッグやペインの並べ替えもできます。 |
| `c4_tab` | clip | 7.5 | タブ = 作業グループ / Cmd+T で新規、Cmd+1〜9 で切替 | タブは作業グループの単位です。プロジェクトごとにタブを分け、その中を分割して使います。 |
| `c4_tree` | clip | 9.8 | Cmd+B でファイルツリー / タブ内の作業ディレクトリを自動でワークスペース表示 | コマンド ビー でファイルツリー。タブの中で開いている作業ディレクトリが、自動でワークスペースとして並びます。 |
| `c4_md` | clip | 8.1 | Markdown はレンダリング表示 / ファイルをクリックすると隣のペインに開く | ファイルをクリックすると、隣のペインにプレビューが開きます。マークダウンは既定でレンダリング表示です。 |
| `c4_reload` | clip | 4.8 | ライブリロード / 編集すると、即反映 | ファイルが書き換わると、プレビューは即座に追従します。 |
| `c4_pdf` | clip | 7.5 | PDF もペイン内で / テキスト選択 / 目次ジャンプ / ズーム | ピーディーエフもペインの中でそのまま。テキストの選択や目次からのジャンプ、ズームもできます。 |
| `c4_image` | clip | 1.9 | 画像プレビュー / PNG / JPEG / SVG / GIF / WebP | 画像も同じです。 |
| `c4_code` | clip | 8.7 | 210 以上の形式にシンタックスハイライト / 軽い編集と保存もできる | コードは 210 以上の形式にハイライトが効き、その場で軽く編集して保存することもできます。 |
| `c4_run` | clip | 10.7 | Code Runner / 再生ボタンでスクリプトを新ペインで実行 | スクリプトは再生ボタンひとつで、新しいペインに分割して実行されます。成果物の確認まで、ターミナルの外に出ずに済みます。 |
| `c5_card` | card | 5.6 | 5. AI に任せる / tako master と worker | ここが本題です。エーアイに作業を任せる、オーケストレーション。 |
| `c5_master` | clip | 11.3 | tako master で司令塔を起動 / 今いるペインが master になる | タコ マスター と打つと、今いるペインが司令塔、マスターになります。あなたがやることは、マスターに日本語で話しかけることだけです。 |
| `c5_spawn` | clip | 19.9 | 依頼を分解し、worker を隣のペインに立てる / 1 worker = 1 成果物。指示は型で、送達確認つき | 依頼を受けたマスターは、作業を成果物ごとに分解し、担当のエーアイ、ワーカーを隣のペインに立ち上げます。ワーカーへの指示は、背景、スコープ、受け入れ条件、検証手順まで埋めた型で渡され、送達確認つきで届きます。 |
| `c5_grid` | clip | 14.2 | 同じタブに並ぶ。進捗は画面でそのまま見える / master の取り分を保ったまま、worker 領域をグリッド配置 | ワーカーは同じタブの中にグリッドで並びます。今なにを考え、どのコマンドを打っているか、リアルタイムで見えます。気になるペインをクリックすれば、直接口を出すこともできます。 |
| `c5_orch` | clip | 7.7 | 右パネルの orch ビューで俯瞰 / 親子関係 / 稼働時間 / コンテキスト使用率 | 右パネルのオーケストレーションビューでは、マスターとワーカーの親子関係や稼働状況を俯瞰できます。 |
| `c5_report` | clip | 16.4 | 完了・入力待ち・消滅を自動検知し、検収してから報告 / 監視・回収・片付けも master の仕事 | マスターはワーカーの完了や入力待ちを自動で検知し、完了報告を鵜呑みにせず、証拠を検査してから結果を届けます。ワーカーの起動、監視、片付け。段取りは全部マスターが持ちます。 |
| `c5_solo` | clip | 9.5 | 1 対 1 でよければ tako solo / worker を立てず、その AI 自身が手を動かす | 分担が要らなければ、タコ ソロ で 1 対 1 の対話もできます。 |
| `c5_gui1` | clip | 8.4 | かんたん表示を順番に使ってみる / まず「+」で新しいタブを作る | ここからは、かんたん表示を順番に使ってみます。まず、プラスのボタンで新しいタブを作ります。 |
| `c5_gui2` | clip | 11.3 | 右上のボタンでかんたん表示へ / tako ui-mode gui でも同じ | ウインドウの右上にあるボタンを押すと、かんたん表示に切り替わります。タコ ユーアイモード ジーユーアイ、というコマンドでも同じです。 |
| `c5_gui3` | clip | 8.9 | まだ何もしていないペインは、みっつのボタンになる / 何をしますか？ / ボタンを押すと、この画面で AI が動き始めます | かんたん表示では、まだ何もしていないペインが、みっつのボタンになります。何をしますか、と聞かれています。 |
| `c5_gui4` | clip | 13.5 | ① AI チームに任せる（tako master） / 司令塔の AI が話を聞いて、必要なだけ担当 AI を集めて進めます | ひとつ目は、エーアイチームに任せる。押すと司令塔のエーアイが立ち上がって、必要なだけ担当のエーアイを集めて進めます。タコ マスター と打つのと同じです。 |
| `c5_gui5` | clip | 14.2 | ② AI と 1 対 1 で話す（tako solo） / 1 体の AI とじっくり相談したいときはこちら | ふたつ目は、エーアイと 1 対 1 で話す。担当のエーアイを立てず、エーアイ 1 体とじっくり相談したいときはこちらです。 |
| `c5_gui6` | clip | 12.7 | ③ コマンド入力へ / この画面をターミナル表示に戻します（何も止まりません）/ 下に「初期設定をやり直す」も | みっつ目は、コマンド入力へ。このペインだけターミナル表示に戻します。下に書いてあるとおり、これは表示の切り替えだけで、動いているものは何も止まりません。 |
| `c5_gui7` | clip | 9.3 | ① を押すと、その画面で AI が起動する / 「準備中… / AI を起動しています」が出て、そのままチャット画面へ | では、ひとつ目のボタンを押します。エーアイを起動しています、という表示が出て、そのままチャット画面に変わります。 |
| `c5_gui8` | clip | 16.7 | チャット画面になる / 上にモデル名・状態・残りの会話容量 / 下が入力欄 | 上には、動いているモデルの名前と、待機中か考え中かの状態、それに会話の残り容量が出ます。下が入力欄で、エンター で送信、シフト プラス エンター で改行です。 |
| `c5_gui9` | clip | 12.2 | あとは日本語で頼むだけ / よく使う操作もボタンに: 会話を軽くする / 新しい会話 / ヘルプ | あとは、やってほしいことを日本語で書いて送るだけです。よく使う操作は、会話を軽くする、新しい会話、ヘルプ、としてボタンになっています。 |
| `c5_gui10` | clip | 12.9 | 担当 AI も同じチャット画面で隣に並ぶ / 通常は司令塔の AI が指示します（ここから直接お願いすることもできます） | 司令塔が担当のエーアイを立てると、隣のペインにもチャット画面が増えます。担当のエーアイには司令塔が指示を出しますが、ここから直接お願いすることもできます。 |
| `c5_gui11` | clip | 9.3 | 同じボタンでターミナル表示へ戻る / 変わるのは見せ方だけ。裏のターミナルは動いたまま | 同じボタンを押せば、ターミナル表示に戻ります。変わるのは見せ方だけで、裏のターミナルはずっと動いたままです。 |
| `c6_card` | card | 4.4 | 6. 再起動しても戻る / tmux バックエンドによる永続化 | 長い作業を任せたときに効くのが、永続化です。 |
| `c6_before` | clip | 11.4 | tako を閉じる直前 / tmux が入っていれば、全ペインは tmux セッション経由で動く | タコは、ティーマックスが入っていれば、全ペインをそのセッション経由で動かします。エージェントもデブサーバーも走ったまま、タコを終了してみます。 |
| `c6_after` | clip | 15.8 | 再起動すると、画面ごと戻ってくる / 実行中プロセス / スクロールバック / タブ構成 / ペイン ID | 再起動すると、タブ構成もペインも、画面の中身ごと復元されます。プロセスは裏で生き続けていたので、作業は途切れません。一晩かかる作業を任せて寝る、が現実的にできます。 |
| `c7_card` | card | 4.3 | 7. スマホから / tako remote と Remote Control | 席を離れても、進み具合は手元で見られます。 |
| `c7_pwa1` | clip | 17.0 | tako remote — Tailscale 内の固定 URL / 画面はデモ用データ。到達できるのは同じ tailnet の端末だけ | タコ リモート は、テイルスケールのネットワークの中だけに存在する固定ユーアールエルで、タコの画面をスマホのブラウザに出します。通信は端から端まで暗号化され、公開インターネットには存在しません。 |
| `c7_pwa2` | clip | 13.2 | エージェントの会話も、承認カードも / 二層認証: tailnet identity + Mac 画面での機器承認 | ペインの一覧から、エージェントの会話を読んだり、権限確認の承認カードにその場で答えたりできます。端末は、マックの画面で承認するまで何も見られません。 |
| `c7_rc` | clip | 22.8 | Claude 公式 Remote Control への委譲（opt-in） / claude.ai / Claude モバイルアプリから会話を操作。既定は OFF | さらに、プロファイルでオプトインすると、タコが起動するクロードを、クロード公式のリモートコントロールへ繋げます。クロード ドット エーアイ や モバイルアプリから、その会話を直接操作できます。会話がアンソロピックのサーバーにも保存されるため、既定はオフです。 |
| `c8_card` | card | 3.8 | 8. Windows と OSS / 導入方法とリンク | 最後に、対応環境と入手先です。 |
| `c8_win` | clip | 18.3 | Windows 版も同じリリースに同梱 / 対応状況は、実機で確かめたものだけを「対応」と書く | タコはマックオーエス先行で開発し、ウィンドウズ版はインストーラーとポータブル版を、同じリリースに同梱しています。どの機能が使えるかは、実機で確かめたものだけを対応と書く方針で、ドキュメントに自動生成の表があります。 |
| `c8_oss` | clip | 9.3 | GPL-3.0-or-later のオープンソース / github.com/takushio2525/tako | タコは、ジーピーエル バージョン 3 のオープンソースです。ソースコードはギットハブで公開しています。 |
| `c8_get` | clip | 9.9 | brew install --cask takushio2525/tako/tako / ドキュメント: tako-docs.pages.dev | 導入はホームブリューで 1 行。ドキュメントサイトには、セットアップからオーケストレーションの実践ガイドまで揃っています。 |
| `outro_card` | card | 6.8 | tako / github.com/takushio2525/tako | エーアイに任せる開発を、ひとつの画面で。タコを、ぜひ試してみてください。 |
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
| `scripts/promo/lib.sh` | 隔離起動・デモ環境・ウインドウ単体キャプチャ（#470）+ **16:9 ウインドウ seed / ビート記録 / 追加素材**（#1081）+ **仮想ディスプレイの舞台**（`promo_stage_prepare` / `promo_vd_*`。下の「仮想ディスプレイ収録」） |
| `scripts/promo/displays.swift` | 接続中ディスプレイの矩形と実ピクセル（`displayID x y w h pxW pxH main\|secondary`）。仮想ディスプレイの位置決めと HiDPI 判定 |
| `scripts/promo/record-explainer.sh <scene\|all>` | 10 シーンの収録。CLI 操作の瞬間を `<scene>-beats.tsv` へ記録。収録の開始 / 終了は `RECORDING START` / `RECORDING END` の 1 行 |
| `scripts/promo/record-pwa.cjs` | PWA（`web/tako-remote`）を iPhone ビューポートでモック API つきに撮る（連番スクショ → mp4） |
| `scripts/promo/click.swift` / `keytype.swift` | 収録中の実クリック（HID タップ）と実キー入力（`CGEventPostToPid` = フォーカスを奪わない）。#1081 の GUI モード章 |
| `scripts/promo/pointer.swift` / `annotate-clicks.sh` | 押した座標と時刻（`<scene>-clicks.tsv`）からポインタ・波紋・ボタンの枠を焼く（ウインドウ単体キャプチャはカーソルを写さない） |
| `scripts/promo/narrate.sh` | timeline.tsv の speech 列 → VOICEVOX（既定）or `say` → 48kHz wav + durations.tsv。バックエンドは `TAKO_PROMO_TTS` で切替、ピークは声に依らず `TAKO_PROMO_PEAK_DB` へそろえる |
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

### 仮想ディスプレイ収録（2026-09-06・常設 `tako-vd`）

**隔離 tako の窓はユーザーのメイン画面に一切出さない**。09-06 にユーザーが「録画の UI が画面に
出てきて邪魔すぎる」と収録を止めた（従来方式は隔離 tako をメイン画面の前面に出し、描画維持のため
約 2 秒ごとに `activate` = ユーザーのキーフォーカスを奪い続けていた）。GPUI は窓が完全に隠れると
描画を止めるので別 Space への退避では撮れず、**OS に実ディスプレイとして見える仮想ディスプレイ**へ
窓を出す。仮想ディスプレイ上の窓は何にも隠れないので activate が要らない。
ユーザー指示（同日）: **「ほかのデバッグ系も仮想ディスプレイでやらせて。今後もずっと」** =
セルフテスト・GUI 検証・収録の隔離 tako はすべてこの画面へ出す。**常設なので作業後に削除・切断しない**。

- 器は **BetterDisplay**（`/Applications/BetterDisplay.app`・4.3.5。仮想スクリーンの作成・接続に
  Pro は要らない = `get -proAvailable` が off の機で実測）。名前は **`tako-vd`**（16:9・HiDPI）。
  **器の扱い（作る・繋ぐ・増殖の検査・Main を仮想にしない）の正本は全プロジェクト共通の
  `scripts/lib/virtual-display.sh`**（#1141 / #1150。`ensure` / `bounds` / `status` / `move-window`）で、
  `lib.sh` の `promo_stage_prepare` はそれを呼んでから収録固有の仕事（HiDPI の確認・窓の置き場所・
  窓がその中にあることの検査）だけをする。接続後の実測: 配置 `1512,0`・2560x1440pt =
  **5120x2880px**（メイン 1512x982pt の右隣）。displayID は器の再起動で変わる（13 → 17 を実測）ので
  控えず、毎回名前から解く
- 窓の置き方: `layout.json` の `window` を仮想ディスプレイの中の座標で seed するだけで
  **起動の瞬間からそこに出る**（GPUI は `window.x/y` を CG のグローバル座標として
  メインスクリーンの高さで y を反転し Cocoa 座標へ直す = `gpui_macos/src/window.rs`）。
  外にあれば System Events の AX で移す（`set position of window 1 to {x, y}`。GPUI の窓にも効く = 実測）。
  他 worker が同じ画面へ移すときの 1 行:
  `osascript -e 'tell application "System Events" to tell (first application process whose unix id is <PID>) to set position of window 1 to {1552, 60}'`
- **置き場所は空きを探す**（`promo_vd_free_origin`）: 常設の画面は他の worker の隔離 tako と共有するので、
  `winbounds --all` で画面上の全窓を引き、重ならない左上を余白刻みで総当たりする。重ねられて
  隠れると GPUI が描画を止めるのは仮想側でも同じ（09-06 に別 worker の 1512x879 の窓が同じ座標へ
  移されてきたので、この収録は右隣の空き `3080,60` で撮った）。`TAKO_PROMO_WIN_X/Y` で明示もできる
- キーフォーカス: tako-app は起動時に `cx.activate(true)` するので、窓が仮想側でも**キー入力の宛先が
  一瞬そちらへ移る**（実測: 起動直後の frontmost が隔離 pid になる）。`promo_start_isolated` は起動前の
  前面アプリを覚え、隔離 tako が前面になっているときだけ元へ戻す（窓が出た直後と 3 秒後の 2 回）。
  メニューバーの名前は本番と同じ「tako」なので見た目の変化は無い
- 切替: `TAKO_PROMO_STAGE=virtual`（既定）/ `main`（従来方式。A/B 専用）。仮想側では
  `RECORDING START <scene>-raw stage=virtual window=<wid x y w h>` に窓の矩形が残る =
  「メイン画面（`0,0 1512x982`）の外にあった」証拠。収録中の `screencapture -x -D 1`（メイン画面）に
  窓が写らないことも 09-06 に目視で確認した（隔離 tako の窓は仮想側だけ）
- **BetterDisplay CLI の罠（4.3.5 実測）**: ①アプリ未起動だと固まる（`open -g -a BetterDisplay` してから）
  ②応答文は当てにならない（`set -connected=on` が「Failed.」と言いながら繋がる / 無言で成功する）→
  判定は必ず CG の一覧（`displays.swift`）③仮想スクリーンの tagID は操作のたびに並べ替わる →
  使う直前に `get -identifiers` で引き直す ④`-name=` 指定の `set -connected=on` は複数オブジェクトに
  当たって**同じ画面が 5 枚繋がった** → 接続は tagID 指定で 1 回だけ、しかも「CG に居ない」ことを
  確かめてから ⑤`get -connected` は接続中でも off を返す → 接続判定は identifiers の `displayID`
  （未接続は `0`）⑥作成パラメータは `create -type=VirtualScreen -virtualScreenName=… -aspectWidth=16
  -aspectHeight=9 -virtualScreenHiDPI=on -virtualScreenSerial=…`（`-name=` / `-aspectRatio=` は無視され
  「仮想 16:9」の既定名で作られる）⑦`discard` は識別子なしだと**全部**捨てる。使うなら tagID を直前に引く
- 収録中に動くのはユーザーのメイン画面ではなく仮想側だけなので、`caffeinate -d -i -u` で
  ディスプレイスリープ（30 分）→ ロック（= `screencapture` 不能）を防ぐだけでよい

### GUI モード章の撮り方（2026-09-09・手順型デモ）

v3 のユーザー評価「簡単 GUI 表示モードの解説がすごく分かりづらい。**しっかりと**、その章で
新しいタブを立てて、GUI モードで新しいペインを出すとどんなボタンが出て、マスター起動ボタンを
押すとどんな UI で…そんな感じでしっかりやってほしい」への差し替え。旧 `c5_gui`（1 区間 18 秒・
master 章の素材の流用）を捨て、**1 ナレーション = 1 操作の 11 区間**（`c5_gui1`〜`c5_gui11`）を
専用素材 `scenes/guimode-raw.mp4` で撮る。5 章の**最後**（`c5_solo` の後ろ）に置くので、
master の話が完結したあとに「初心者向けの同じこと」を見せる並びになる。

見せる順（`scene_guimode`）: 新しいタブを「+」で作る → タブバー右上のトグルでかんたん表示へ →
3 枚のボタンを 1 枚ずつ枠で囲んで説明 → 「AI チームに任せる」を押す → 準備中 → チャット画面 →
チャット欄へ日本語で依頼 → 担当 AI が隣のペインに生える → 同じボタンでターミナル表示へ戻す。
**ペインは同時に 2 枚まで**（master + worker 1 体）= 「ペインが多くて追えない」という不満の逆。

- **ボタンの文言は実機で確認したものだけを書く**（正本は `crates/tako-app/src/ui_text/ui_mode.rs`）。
  見出し「何をしますか？」/ 副題「ボタンを押すと、この画面で AI が動き始めます」/
  ①「AI チームに任せる」`tako master`「司令塔の AI が話を聞いて、必要なだけ担当 AI を集めて進めます」/
  ②「AI と 1 対 1 で話す」`tako solo`「1 体の AI とじっくり相談したいときはこちら」/
  ③「コマンド入力へ」「この画面をターミナル表示に戻します（何も止まりません）」/
  下部リンク「初期設定をやり直す」`tako setup` / 脚注「これは表示の切り替えだけです。
  ターミナルはいつでも使えます」。**ユーザーの言う「マスター起動ボタン」は画面には無い**ので、
  台本では実名（①）で呼ぶ。プロファイル選択の ▾ は選択肢が複数のときだけ出るので、
  既定 1 個の収録では出ない
- **操作は本物のクリック / キー入力で行う**（`click.swift` / `keytype.swift`）。CLI で代替すると
  絵が変わる: カード押下は `begin_pane_settle`（#720）を張るので「準備中… / AI を起動しています」
  が出るが、シェルへ `tako master` を送るだけでは claude の起動ログが素通しで見える。
  チャット入力欄に文字が入っていく絵も `tako send`（claude の TUI へ送る経路）では出ない
- **マウスは HID タップへ流す。キーは pid へ直送する**。System Events の合成クリックは GPUI に
  届かない（既知）。`CGEventPostToPid` は**キーだけ届き、マウスは届かない**（2026-09-09 実測:
  pid 直送のクリックはタブも増えずモードも変わらない）。したがってクリックは実カーソルを
  ワープさせる必要があり、キー入力はフォーカスを奪わずに送れる
- **押した拍子に窓が前面へ出る**ので、クリックの直後（検査より先）に `promo_give_back_focus` で
  元のアプリへ返す。返さずに数秒置いた回は、**ユーザーのキー入力が隔離ウインドウへ流れ込み**
  シェルに「う」が 1 文字混ざって `う/Applications/…/tako master` になり master が起動しなかった。
  「押す直前に前面化してから押す」案はこの理由で破棄した
- **窓は起動後に AX で置き直す**（`promo_force_window_frame`）。#1149 以降
  `initial_window_bounds` は置き先ディスプレイを解決できた検証起動では保存フレームを捨てて
  「960x600pt をその面の中央へ」置くので、**layout.json の seed（16:9・空きを探した位置）は
  位置もサイズも効かない**。v1〜v3 の素材はこの分岐が入る前に撮ったので 1920x1080px だった。
  サイズが崩れるだけでなく、**どの隔離 tako も同じ中央へ重なる**のが致命的で、
  別 worker の窓が上にあるとクリックがそちらへ吸われる（実測: カード押下が 3 回打ち直しても
  無反応・`frontmost` が相手の pid のまま動かない）。`promo_vd_window_clear` で
  「他の窓と重なっていないこと」まで確かめてから撮る
- **クリックは状態で確かめて打ち直す**（`promo_click_until` + `promo_check_tabs` /
  `promo_check_ui_mode` / `promo_check_pane_display`）。clicks.tsv には**効いた 1 回だけ**残す
  ので、注釈のポインタが空振りを描かない。チャット入力欄は読める状態が無いので、
  **打てたことを OCR で確かめてから Enter を打つ**（`promo_type_verified`。
  1 打目が化けることがあるので `--backspaces` で消してやり直す）
- **カーソルは 1 ピクセルも写らない**（`screencapture -x -o -l<windowID>` はウインドウ単体）。
  手順型デモは「どのボタンを押したか」が本題なので、押した座標と時刻を
  `scenes/guimode-clicks.tsv` に残し、`annotate-clicks.sh` がポインタ（`pointer.swift`）・
  クリック波紋・ボタンの枠を後処理で焼く。**枠はボタン説明の区間が絵として動かない問題も
  同時に解く**（`promo_verify` の「素材がほとんど動いていない」検査と完成動画の
  「15 秒以上の静止なし」に引っかからない）
- **枠の秒数と間合いは実測したナレーション秒から決める**（`audio/narr/durations.tsv`。
  区間の尺 = `max(min_dur, ナレーション秒 + 0.8)`）。GUI 章は 9.2〜17.5 秒で、最長は
  `c5_gui8`（チャット画面の説明 16.7 秒）なので、`chat` の次の操作までは 20 秒あける
  （足りないと説明中に入力の絵が写り込む）
- **収録用アカウントに `TAKO_PROMO_CLAUDE_CONFIG_DIR` は使えない**（この章に限らない）。
  ①univ アカウントの `PreToolUse` フックが tako 開発ディレクトリ以外で tako ツールを拒否し、
  「このアカウントは…専用に制限されており」という拒否文が master のチャットに写り込む
  ②外部 config dir の会話は `claude agents --json` の走査対象（`agent_scan_targets` =
  accounts.yaml + 既定）に入らないので**かんたん表示のチャット判定が永久に立たない**
  （`tako orchestrator accounts add` で登録すれば立つ = `promo_register_recording_account`。
  実測: 登録前 terminal のまま / 登録後 25 秒で chat）。既定アカウント（デモ HOME +
  共有ログインキーチェーン）で撮り、`promo_ensure_oauth_fresh` で更新の競合を避ける
- **信頼はデモ HOME にも要る**。「+」で作った新しいタブの cwd はデモ HOME なので、
  そこで起動した claude が「Is this a project you trust?」を出して収録が壊れた
  （`promo_external_config_ready` / `promo_demo_home_agent_ready` の両方で信頼済みにする）
- **master は放っておくと `tako_orchestrator_run` を選ぶ**（ペインを残さない実行なので
  「隣に生える」絵が撮れない。#1081 の master 章と同じ罠）。画面に出る依頼文は
  ユーザーが実際に打つ自然な日本語にしたいので、steering は profile の
  `prompt_blocks.append`（= ユーザーが置けるローカルルール）へ置き、**run を明示的に禁止**する。
  「確認は不要」と書くと run を選びやすくなるので、その場合も spawn を変えない旨まで書く。
  見せている機能（spawn がペインを作る）は本物で、選び方だけを固定している
- **`min-worker-cols` の既定 60 だと worker が別タブへ出る**（#1132）。960x540pt / フォント 15 の
  窓では割った worker が 59 桁になるため。`tako orchestrator layout --min-worker-cols 40` で下げる
- 押下後にチャット表示にならなかったら**収録を中止する**（壊れたテイクで先へ進まない）

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
- （say バックエンドの実測。v2 まではこれが本番だった）`-r` は 160 と 175 で尺が変わらなかった。180 で使う。ピークは -13dB 程度と
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
- **逆に速すぎても撮れない**（09-06 実測）: 同じ master が `tako_orchestrator_run` を 1 体ずつ直列に回し、
  数秒で終わる `task.sh` を完了直後に auto_close したため、**3 体が並ぶ瞬間が一度も無く**、
  ペイン数 ≥ 4 を待つループが 240 秒待って空振り → orch ビューもかんたん表示も master 1 枚で撮れていた。
  対策 = `task.sh` を約 80 秒（`TAKO_PROMO_TASK_SECS`。テスト結果を 2 秒ごとに流す）にして
  「稼働中」の時間を作る + 依頼文に「`tako_orchestrator_spawn` で同時に（run は使わない）」
  「worker のペインは閉じない」を明記する
- **デモ HOME の claude がユーザーをログアウトさせる事故（09-06 22:52）**: デモ HOME は実ユーザーの
  ログインキーチェーン（`Claude Code-credentials` の 1 項目）を共有するが、claude が HOME 配下で取る
  **更新の排他は共有しない**。共有トークンの期限切れの瞬間にデモ側と本番の claude 群が同時に refresh を
  打ち、負けた側（デモ側）が invalid_grant → claude が**キーチェーンの資格情報を空にして
  「Login expired」**を出した（mdat 22:52:24 = デモ claude の起動直後。accessToken / refreshToken が空・
  expiresAt が epoch 0。実 HOME の `claude auth status` も loggedIn=false になり、ユーザーに
  `claude auth login` をお願いした）。対策 = `promo_ensure_oauth_fresh`（`promo_demo_home_agent_ready`
  から呼ぶ）: 期限フィールドだけを読み、収録のあいだ（既定 900 秒）有効でなければ**実 HOME の claude**
  （排他つき）に 1 回だけ更新させ、それでも足りなければ止める。デモ HOME の claude に更新の機会を与えない
- **収録用 claude のアカウントは env で差し替えられる**: `TAKO_PROMO_CLAUDE_CONFIG_DIR=$HOME/.claude-univ`
  のように別アカウントの**実 config dir** を指すと、隔離 tako の master / worker / setup アシスタントが
  その資格情報（キーチェーン項目 `Claude Code-credentials-<パスの sha256 先頭 8 桁>`）で動く
  （09-06 は personal がログアウト状態のあいだ univ で撮った。画面にアカウント名は出ない）。
  そのアカウントの `.claude.json` にはデモプロジェクトの信頼だけを足し（書く前の写しを /private/tmp へ）、
  権限はデモプロジェクト側の `.claude/settings.local.json` で許可する。**写しやシンボリックリンクは不可**
  （項目名がパス文字列のハッシュなので別パスだと資格情報が見つからない）
- **報告の絵は `report_done` を基準にする**（09-07 実測）: `report` ビートは「master が idle に戻るのを
  待ち始めた時刻」で、実際の完了報告はその 40〜50 秒後（worker 完了 → idle 検知 → 報告本文が出るのは
  `report_done` の 3〜5 秒前）。`c5_report` は `report_done − 10`（idle 検知から報告が出るまでの流れ）、
  `op_hook`（完成形）は `report_done + 2`、`c5_solo` は `report_done + 12` を in 点にする
- worker ペイン冒頭の定型文: #790 の Cross-Session Messaging で届いた指示には
  「別セッションからの指示として扱え」の長い注意書きが付き、視聴者には無関係な英文が
  worker ペインを埋める。master 章だけ `TAKO_PEER_MESSAGING=off`（従来のキー操作経路）で撮る
- ffmpeg は既定で stdin を読むので `while read` ループの中で呼ぶと tsv の次の行を食う → `-nostdin`
- bash の `IFS=$'\t' read` は連続タブを 1 つに潰すので、空欄のある tsv は列がずれる →
  `promo_timeline_rows` で空欄を `-` に埋めてから読む
- docs の md は frontmatter（`--- title ---`）がプレビューで本文として描かれる → 見出しへ置換して写す
- コマンドカード（`tako show-command`）は画面下部に出るので、その区間のテロップは上に置く
  （caption の先頭 `^`）
- `$id（` のように変数の直後に全角を置くと bash が変数名に取り込んで `set -u` で落ちる
  （`shell_scripts` 番犬が CI で落とす）。`${id}（` と書く
- **System Events の合成クリックは GPUI に届かない**（`.agent/activeContext.md` から移した実測）。
  窓の移動・前面化のような AX 経由の操作は効くのにクリックは受け取られないので、GUI の操作を
  機械で検証するときは `self_test::click_at`（実 OS マウスと同じ `PlatformInput` を流す）を通す。
  **ドラッグは合成 `PlatformInput` なら届く**（#725 / #1043 の実測）

## 声の選定（v3・2026-09-07）

v2 まではナレーションが macOS の `say -v Kyoko` で、ユーザー評価は「声をどうにかして欲しい」。
**抑揚の狭さが原因**だと数値で確かめてから替えた（有声フレームの F0 を自己相関で拾い、
音高に依らないセミトーンで散らばりを測る = 棒読み度の代理指標）。

| 候補 | 平均 F0 | 抑揚 SD | 抑揚幅 | 判断 |
|---|---|---|---|---|
| say / Kyoko（v2 まで） | 260.9 Hz | 3.29 半音 | 8.20 半音 | 基準。**これが狭い** |
| ずんだもん ノーマル（id=3） | 374.2 Hz | **4.22 半音** | **10.71 半音** | **採用**。使える中で最も広い（Kyoko 比 +28%） |
| ずんだもん セクシー（id=5） | 377.2 Hz | 3.64 半音 | 9.94 半音 | やや平坦・14% 遅く尺が伸びる |
| ずんだもん ツンツン（id=7） | 358.8 Hz | 2.88 半音 | 7.63 半音 | **Kyoko より狭い**（直したい欠点そのもの） |
| ずんだもん ささやき（id=22） | 315.9 Hz | 8.44 半音 | 23.53 半音 | 数値は大きいが**有声 139 / 1000 フレーム**（ほぼ無声の息）。BGM の下で埋もれる |

採用は **ずんだもん ノーマル（speaker=3）**。パラメータは同一区間で 5 通り測って選んだ:

| 調整 | 平均 F0 | 抑揚 SD | 尺 | |
|---|---|---|---|---|
| 既定（1.0 / 0.0 / 1.0） | 374.2 Hz | 4.22 半音 | 9.1s | |
| speed 1.0 / pitch -0.03 / inton 1.15 / pause 1.1 | 344.6 Hz | 4.20 半音 | 9.3s | |
| **speed 1.05 / pitch -0.05 / inton 1.28 / pause 1.1** | **326.5 Hz** | **4.43 半音** | **8.9s** | **採用** |
| speed 0.95 / pitch -0.03 / inton 1.10 / pause 1.25 | 340.8 Hz | 4.35 半音 | 10.1s | 尺が伸びる |
| speed 1.02 / pitch -0.05 / inton 1.20 / pause 1.1 | 321.5 Hz | 4.02 半音 | 9.2s | 抑揚が落ちる |

採用値は**抑揚が最も広く・平均音高が最も低く（甲高さが和らぐ）・尺が伸びない**の 3 つを同時に満たす。
既定値は `voicevox-synth.py` が持つ（`TAKO_PROMO_VV_*` で上書きできる）。

### 読みの検証（耳を使わずに済む部分）

`POST /audio_query` の戻り値の `kana` が**エンジンが実際にどう読むか**なので、外来語の読み違いは
合成前に確認できる（`voicevox-synth.py --print-kana`）。台本は外来語をすべてカナで書いてあるため
**書き換え不要**だと実測で確認した: `エーアイエージェント` → `エエアイエエジェント` /
`ジーユーアイターミナル` → `ジイユウアイタアミナル` / `ティーマックス` → `ティイマックス` /
`エムシーピー` → `エムシイピイ` / `クロード` → `クロオド` / `ジーピーエル バージョン 3` →
`ジイ/ピイ/エル、バアジョン、サン`（数字も正しい）。

### 声を替えると音量段が壊れる（v3 で直した）

**同じピークにそろえてもラウドネスは一致しない**。クレストファクタが声ごとに違うためで、
ピーク -12.3dB で揃えた同一区間が say は -25.3 LUFS・ずんだもんは **-31.9 LUFS**（6.6dB 差）。
`build-explainer.sh` のナレーション段は `volume=3.0`（+9.5dB）の**固定ゲイン**で、これは
say のピーク中央値 -12.3dB 専用の値だった。そのまま VOICEVOX を通したら最終段が振り切れて
**0.0dBFS まで潰れた**（実測）。

- narrate.sh は**どのバックエンドでも同じピーク**（`TAKO_PROMO_PEAK_DB`、既定 -12.3dBFS）へそろえる。
  測るのは**最終形**（48kHz stereo 変換後）。変換前に測ると ffmpeg の mono → stereo 行列が
  各チャンネルへ 1/√2（-3.01dB）を掛けるぶん狙いから外れる（実測: 目標 -12.3 に対し出力 -15.3）
- build-explainer.sh のナレーション段は**測ってから当てる**（`TAKO_PROMO_NARR_LUFS`、既定 -15.8 LUFS
  = v2 の設計値）。固定ゲインは声を替えるたびに壊れるので置かない
- 最終段で番組全体を **-14 LUFS**（`TAKO_PROMO_LUFS`）へ寄せる。1 パスの `loudnorm` は音楽の下で
  ポンプするので使わず、測って固定ゲイン 1 回 + リミッタ
- **`alimiter` は `level=disabled` が必須**。自動レベルが既定 true で、リミットしたあと 0dB へ
  戻すので `limit` の指定が無かったことになる（実測: `limit=0.84` でも True Peak +0.6dBFS）

### クレジット表記（VOICEVOX 利用規約）

生成音声の公開には話者クレジットの表示が必要。表記は**エンジンの `/speakers` の policy から引く**
（`voicevox-synth.py --print-credit` → `VOICEVOX:ずんだもん`）ので、話者を替えても手で直す場所が無い。
置き場は 2 か所:

1. **動画の末尾カード**（`outro_card` の脚注に `音声: VOICEVOX:ずんだもん`。他の章カードには出さない）
2. **説明文**（`tako-explainer-description.txt` の「注記」。規約 URL も併記）

BGM は `make-bgm.py` が波形から合成した自作音源なので**外部素材のクレジットは不要**
（GPL-3.0-or-later のリポジトリの一部として扱える）。説明文にもその旨を 1 行入れてある。

### エンジンの入手（リポジトリには置かない）

```sh
curl -fL -o /tmp/vv.7z.001 \
  https://github.com/VOICEVOX/voicevox_engine/releases/download/0.25.2/voicevox_engine-macos-arm64-0.25.2.7z.001
7zz x -y -o"$HOME/Desktop/tako-promo/tools" /tmp/vv.7z.001
~/Desktop/tako-promo/tools/macos-arm64/run --host 127.0.0.1 --port 50021   # 合成中だけ起動する
```

配布物は約 1.8GB（展開後も同程度）なので `~/Desktop/tako-promo/tools/` に置き、**リポジトリには入れない**。
合成が終わったらエンジンは停止する。

## 読みの点検と修正（v4・2026-09-08）

v3 の試聴で「読み方に不自然な箇所がある」（例: 「3 つ」が「サン、ツ」）との評価。1 箇所直して
終わりにせず**全 47 区間の読みを機械で取り出して全件読んだ**。点検は
`scripts/promo/check-readings.sh`（`--diff` で読み替えが効いた区間だけ出る）が正本。

### なぜユーザー辞書で直せないか（実測）

誤読はどれも**数詞と助数詞の間に半角空白がある**形（`3 つ` / `1 行`）で起きる。VOICEVOX ENGINE は
辞書を引く前に空白でトークンを割るので、`3 つ` を surface に登録しても
**登録は 200 で通るのに一度も当たらない**（実測。空白なしの `3つ` なら当たる）。
`/user_dict_word` は surface を全角へ正規化するが、当たらない原因は正規化ではなく空白での分割。

したがって直し方は「合成の直前にテキストを読み用のかなへ置換する」形にした。正本は
`scripts/promo/reading-overrides.tsv`（表記 / 読み / 理由の 3 列）で、`voicevox-synth.py` が
**合成と `--print-kana` の両方で同じ置換を通す**（耳で聴かずに読みを検証できる形を保つ）。
`--no-overrides` で修正前と A/B できる。**台本（`explainer-timeline.tsv`）の文面は変えていない**
（画面に出るテロップは caption / subtitle 列で、読みとは無関係）。

### 数詞 + 助数詞は全件を明示的に確認した

`speech` 列に残る半角数字は **10 箇所 / 8 区間**。全部の読みを見た:

| 区間 | 表記 | 読み | 判定 |
|---|---|---|---|
| c2_card | `3 つ` | サン、ツ → **ミッツ** | ❌ 修正（助数詞「つ」は和語数詞） |
| c2_agent1 | `1 グループ` `1 タブ` | イチグルウプ / イチタブ | ○ 標語としてこれが意図 |
| c2_agent2 | `140 個以上` | ヒャクヨンジュウコ → **ヒャクヨンジュッコ** | ❌ 修正（促音化が自然） |
| c3_brew | `1 行` | イチ、クダリ → **イチギョウ** | ❌ 修正（「くだり」は文章の一節） |
| c4_code | `210 以上` | ニヒャクジュウイジョオ | ○ |
| c5_solo | `1 対` `1 の` | イチタイイチ | ○ |
| c8_oss | `バージョン 3` | バアジョンサン | ○ |
| c8_get | `1 行` | イチ、クダリ → **イチギョウ** | ❌ 修正 |

数詞以外に 1 件: **c5_gui `空のペイン`** が「ソラノペイン」→ **カラノペイン**（同訓異字。
意味が変わるので誤読。ユーザー報告には無かったが全件点検で見つけた）。

**英字の略語は構造的に誤読しない**: `speech` 列に生の英字は **0 件**（tako / MCP / GPUI / tmux /
Claude / cwd / macOS / CLI / PR はすべて `タコ` `エムシーピー` `ティーマックス` `クロード`
`マックオーエス` `シーエルアイ` のようにカナで書いてある）。記号（`/` `+` `→` `=`）も **0 件**。

### 尺の変化が映像の同期を壊していないこと

読みが短くなるので 5 区間の秒数が動くが、**3 区間（c2_agent2 / c5_gui / c8_get）は `min_dur` の
床に当たるので採用尺は変わらない**（20.0 / 18.0 / 14.0 秒のまま = 収録素材の見え方は不変）。
実際に尺が動いたのは `c3_brew`（13.31 → 12.45 秒）と `c2_card`（カード）だけで、
どちらも設計どおり 0.80 秒の余白を保っている。**47 区間すべてでナレーションが区間に収まる**
（はみ出し 0 件）。

## 完成物と検査結果（v4・2026-09-08）

v3 との差は**読みだけ**（声・台本の文面・映像素材・BGM・音量段の調整値はどれも変えていない）。

| 項目 | v4 | v3 |
|---|---|---|
| 動画 | `~/Desktop/tako-promo/tako-explainer-v4.mp4`（**9:59** = 599.8 秒 / 45.8MB） | 10:01 = 601.4 秒 |
| ラウドネス | **-14.9 LUFS / True Peak -1.5 dBTP** | -14.8 LUFS / -1.7 dBTP |
| ナレーション | 47 区間 / 合計 512.0 秒 | 514.9 秒 |
| 章 | 00:00 / 00:21 / 01:11 / 02:28 / 03:48 / 05:21 / 07:25 / 07:59 / 09:02 / 09:50 | 09:51 まで |
| 機械検査 | 8 秒以上の無音 0 / 2 秒以上の黒 0 | 同 |
| PII 検査 | 600 フレーム → 34,385 行 → 7 カテゴリすべて **0 件** | 0 件 |
| 読みの聴き比べ | `audio/samples/reading-fixes-compare.mp4`（修正前 → 修正後 × 5 件・121 秒） | — |
| サムネ | 変更なし | 変更なし |

## 完成物と検査結果（v3・2026-09-07）

v2 との差は**ナレーションの声だけ**（映像素材 `scenes/*-raw.mp4` は撮り直していない）。
区間長はナレーション秒で決まるので、**章タイムスタンプは v2 から動く**（下表）。

| 項目 | 値 |
|---|---|
| 動画 | `~/Desktop/tako-promo/tako-explainer-v3.mp4`（10:01 = 601.4 秒 / 1920x1080 / 30fps / H.264 + AAC 48kHz / 47 区間 / 45.9MB） |
| 声 | VOICEVOX ENGINE 0.25.2 / ずんだもん ノーマル（speaker=3）/ speed 1.05・pitch -0.05・intonation 1.28・pause 1.1 |
| ナレーション | 47 区間 / 合計 514.9 秒（v2 の say は 488.7 秒 = +26.2 秒 / +5.4%） |
| 章タイムスタンプ | `tako-explainer-chapters.txt`（00:00 / 00:21 / 01:11 / 02:29 / 03:49 / 05:22 / 07:27 / 08:01 / 09:03 / 09:51） |
| 音量 | **-14.8 LUFS / True Peak -1.7 dBTP / LRA 8.1 LU**（v2 は -15.2 LUFS / -1.8 dBTP。クリップなし） |
| 機械検査 | 8 秒以上の無音なし / 2 秒以上の黒フレームなし / BGM はナレーション外の区間で -17〜-22dB（ダッキング動作） |
| PII 検査 | 601 フレーム（1 fps）を Vision OCR → 34,491 行 → 7 カテゴリすべて **0 件** |
| クレジット | 末尾カードの脚注（595 秒のフレームで目視確認）+ 説明文の「注記」 |
| サムネ | **変更なし**（v2 のものを流用） |

## 完成物と検査結果（v2・2026-09-07）

v1 との差は **master 章の素材だけ**（09-04 の旧素材 = effort max で worker 2 体・チャット表示なし・報告前で
尺切れ → 09-07 に仮想ディスプレイ上で撮り直し = worker 3 体が同時に並ぶ → orch ビュー → かんたん表示
（4 ペインとも chat）→ 完了報告）。区間長はナレーション秒で決まるので**章タイムスタンプは v1 と同一**。

| 項目 | 値 |
|---|---|
| 動画 | `~/Desktop/tako-promo/tako-explainer-v2.mp4`（9:47 = 587.3 秒 / 1920x1080 / 30fps / H.264 + AAC 48kHz / 47 区間） |
| master 章の素材 | `scenes/master-raw.mp4`（420 秒・1060 枚 = 2.52 fps・**異なるフレーム 420/420**・仮想ディスプレイ `tako-vd` 上・personal アカウント）。ビート: request 14.5 / workers_up 35.7 / orch 51.7 / gui 67.7 / report 89.9 / report_done 141.2 |
| 章タイムスタンプ | `tako-explainer-chapters.txt`（v1 と同一: 00:00 / 00:20 / 01:10 / 02:26 / 03:45 / 05:14 / 07:16 / 07:50 / 08:49 / 09:37） |
| 機械検査 | 無音 8 秒以上なし / 黒は章カードのフェード（0.33〜0.5 秒 × 20）のみ / **15 秒以上の静止なし**（v1 の master 章 1 箇所が解消）/ -15.2 LUFS・True Peak -1.8 dBTP |
| PII 検査 | 587 フレーム（1 fps）を Vision OCR → 7 カテゴリ（email / home_path / tailnet / private_ip / token / uuid / 環境由来語）すべて **0 件** |
| サムネ | `tako-explainer-thumb-a.png` を新 master 素材（145 秒 = 完了報告 + worker 3 体。orch パネルはこの時点で閉じているので無し）で作り直し。旧版は `scenes/old-0904/` |
| 収録の見え方 | 収録中の隔離 tako の窓は `1552,60 960x540`（仮想側）で、メイン画面の `screencapture -D 1` に窓は写らず、frontmost はユーザーのアプリのまま（13:27 / 13:32 に実測） |

素材の残骸: master 章の旧素材は `scenes/old-0904/`（master-raw / master-beats / thumb-a）。

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
・収録は tako v0.8.3〜v0.8.6 / Claude Code 2.1.258（macOS）。機能や画面は今後のバージョンで変わることがあります

#tako #ClaudeCode #AIエージェント #ターミナル #Rust #オープンソース
```
<!-- description:end -->

### サムネイル案

- 案 A: 完成形（master の完了報告 + worker 3 体）を背景に「AI エージェントを / 1 つのタブで動かす」
  （v2 で新 master 素材の 145 秒から作り直し。orch パネル込みのフレームは master ペインに permission
  ダイアログが写っていたので採らなかった）
- 案 B: かんたん表示（チャット画面）を背景に「Claude Code の司令塔を / ターミナルに」
- 生成: `thumbnail.swift`（背景フレーム + 2 行見出し + 小ラベル）。出力 `~/Desktop/tako-promo/tako-explainer-thumb-{a,b}.png`
