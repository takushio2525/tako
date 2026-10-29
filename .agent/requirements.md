# requirements.md — 機能要件

> 「何ができるべきか」を定義する。実現手段は `architecture.md`、実装順序は `roadmap.md`。
> 優先度は MoSCoW（M = Must / S = Should / C = Could）。
> コンセプトの背景は `concept.md` を参照。

## 設計原則（全要件に優先）

1. **ゼロコンフィグ**: インストール直後、設定なしでコア価値（エージェント集約監視）が動く
2. **Zed 級の速度**: ターミナルとしての体感性能で既存ネイティブ勢に劣らない
3. **強制しない**: 自動ペイン生成は「アプリ内エージェントの明示操作」か「提案チップへの同意」のみ。勝手に画面を分割しない
4. **クロスプラットフォーム**: macOS 先行だが、Windows で動かない設計判断はしない
5. **AI フルコントロール（開発不変条件）**: **現在および将来のすべての機能は、追加した時点で
   MCP / CLI から操作可能でなければならない**。設定・テーマ・レイアウト・タブ・ファイル操作・
   プレビュー等、機能の種別を問わず、**UI でできることはすべて AI からもできる**。
   新機能の Definition of Done に「対応する MCP / CLI 操作の提供」を含める。
   技術的制約等で例外とする場合は、理由をこの仕様書に明記する。
   ユーザーが抽象的に「あれやって」と言えば AI が tako を全部操作できる、
   **AI 主体のアプリであることが tako の存在意義**である。合言葉は
   「**過保護なくらい、AI に頼めば何でも分かりやすくやってくれる**」。
   例:「あのファイルどこにあるか分かんないから開いて見せて」→ AI が探してプレビューペインで開いて見せる
   （原則 3 と矛盾しない: AI 操作は「アプリ内エージェントの明示操作」に含まれる）

> 例外（設計原則 5・2026-07-08）: `tako master` / `tako solo` は claude セッションを
> **起動する CLI エントリポイント**であり、ペイン / レイアウト操作ではないため MCP ツールに
> しない（master も同様に CLI のみ）。起動後に solo / master が行う操作（split / send /
> read 等）はすべて既存 MCP ツールで等価に実行可能なので、フルコントロール不変条件は保たれる。

> 例外（設計原則 5・2026-07-17・#283 remote 機器ペアリング認証）:
> **リモート端末のペアリング承認・role 昇格は Mac 側 GUI のダイアログ限定**とし、
> MCP / CLI に承認 API を提供しない。理由 = これは「tako を操作する権限を新しい端末へ
> 付与する」セキュリティ境界の操作であり、AI（= すでに tako を操作している主体）が
> 自分で承認できてしまうと二層認証の第二層（人間による物理的な承認）が無意味になる。
> 承認だけは必ず Mac の前にいる人間の手を経由させる。
> 一方、リモートの **start / stop / status / devices list / devices revoke** は
> MCP（`tako_remote_start` / `tako_remote_stop` / `tako_remote_status` /
> `tako_remote_devices`）と CLI（`tako remote …`）に 1:1 で公開し、この 5 操作については
> フルコントロール不変条件を維持する（tailnet 内限定露出 + 失効は AI からも行える方が安全側）。
> 承認 API（`/api/admin/pair/approve` / `deny`）はローカル管理トークン（`<state_dir>/
> tako-remote.token`、0600）で保護され、同一 Mac 上の tako-app GUI からしか到達できない。
>
> 例外の範囲（2026-09-14・#1452 で明確化）: **上げる操作だけ**が GUI 限定で、
> **降格・削除・一覧・保留中の要求一覧は CLI / MCP から 1:1 で行える**
> （`tako remote devices role / revoke / list` = `tako_remote_devices`）。
> 「上げられない」ことを実測できるように**経路は 1:1 で在る**（`action="role"` に
> 強い role を渡すと 403 相当で断る）。理由 = 権限を**弱める**方向は、AI から行える方が
> 安全側（置き忘れた権限をその場で畳める）で、二層認証の第二層を損なわない。
> 昇格を PC の人へ縛る仕組みは 3 段（方向の判定 / 呼び出し元プロセスのゲート /
> dispatch の手前での拒否）で、限界と受容した残りは `.agent/threat-model-remote.md`
> 「権限の更新リクエストと昇格のゲート（#1452）」に書く。
> **MCP / CLI の IPC は tako-app の中の dispatch で走る**ので、呼び出し元プロセスの
> ゲートだけでは AI と GUI を区別できない —— この一段が抜けると例外が崩れる。

## FR-1 ターミナル基盤

| ID | 要件 | 優先度 |
|---|---|---|
| FR-1.1 | VT100/xterm 互換のターミナルエミュレーション（alacritty_terminal 準拠） | M |
| FR-1.2 | タブ（= エージェントグループ）の作成・切替・並べ替え・クローズ | M |
| FR-1.3 | ペイン分割（水平/垂直、ネスト可、リサイズ、フォーカス移動） | M |
| FR-1.4 | GPU 描画によるスクロール・出力のスムーズ表示 | M |
| FR-1.5 | フォント設定・カラーテーマ・リガチャ（カタログは FR-4 を正とする） | S |
| FR-1.6 | 検索（ペイン内スクロールバック検索） | S |
| FR-1.7 | hyperlink（OSC 8）・URL 自動検出とクリックで開く | S |
| FR-1.7.1 | **ターミナル内のパスリンクの ⌘+右クリックメニュー**（#1182）。⌘+クリックで開けるパス（#147 / #153。絶対 / cwd 相対 / `~` 起点 / `path:行:桁`）に対して、⌘+右クリックで「このパスに対して何をするか」のメニューを出す。項目は**ファイルツリーの右クリック（FR-3.12 / #314）と同じ操作・同じ文言**を共有する（`tako-app::ui_text::path_menu` がツリーの `sidebar::menu_*` を委譲で引くので表記が 2 つに割れない）: `tako で開く`（ファイル = プレビュー / ディレクトリ = ターミナル分割 = ⌘+クリックと同じ動作。文言は種別で入れ替える）/ `デフォルトアプリで開く`・`このアプリで開く...`（**ファイルのみ** = ツリーと同じ出し分け。ディレクトリを既定アプリで開くのはファイルマネージャで開くのと同義で `Reveal` と重複するため）/ `Finder で表示`（呼び名は #617 の 1 実装で OS 出し分け = **Windows でも項目は消えず「エクスプローラーで表示」へ入れ替わるだけ**）/ `相対パスをコピー` / `絶対パスをコピー`。**並びと出し分けの正は `tako_core::path_menu`**（GPUI 非依存の純粋関数。GUI を立てずに検査できる）。**⌘ 無しの右クリックとリンクの無い場所では何もしない**（ターミナル本体はこれまで右クリックを扱っておらず、従来の挙動をそのまま残す）。ミラースクロール表示中は判定しない（#159 の既知制約 = ⌘+クリックと同じ）。**全項目が既存の dispatch `FileOp` へ 1:1**（開発不変条件）で、新規は「tako で開く」= `FileOpKind::OpenInTako` の 1 つだけ（実体は `OpenFile` / `Split` への振り分けで、**⌘+クリック自身もこの 1 実装を通る** = GUI / CLI / MCP で「リンクを開く」の意味が必ず一致する）。CLI `tako file open-in-tako` / MCP `tako_file_op` の `op: "open_in_tako"`。**Windows の `\` の形のパス**（`C:\…` / UNC / `src\main.rs:10:5`）も #2435 から前段の検出の候補になる（`links::is_path_like` が `Platform` を引数で受ける純関数で、macOS 上から Windows の形を検査できる。残る狭さ = 拡張子の無い `\` 区切りの相対パスは `.\` 付きのときだけ・Debug 表示の `C:\\…` は対象外は `tako_links` の宣言 `WIN_PATH_LINK_BACKSLASH` が正本）。セルフテスト項目 147 は unix 限定のまま（Windows での実走は未確認）で、メニューの並び・文言・出し分けは純粋関数の単体が macOS 上から両 OS 分を検査する。前提として**リンク検出の soft wrap 判定を実画面に合わせて直した**（実画面の `ScreenLine.text` は行末まで空白詰めなので、旧判定では隣り合う 2 行に何か書かれているだけでパスリンクが 1 つも検出されなかった。`conventions.md`「実画面の行は空白詰め」節）（✅ 2026-09-09、#1182） | S |
| FR-1.7.2 | **地の文に埋まったパスもリンクにする**（#1283）。日本語の文の中にパスが出ている形（`` 動画は`~/…/x.mp4`、確認して。`` / `~/…/x.mp4このリンクが…`）でも ⌘+クリックで開ける。真因はトークンの区切りが ASCII の空白と `()[]{}<>,;` だけだったこと（**文ごと 1 トークン**になり実在チェックで落ちる = 素の形しか飛べなかった）。**区切り集合は増やさない**（全角の括弧・読点は `資料（最新）.pdf` のような実在するファイル名にも現れるので、区切りにすると今まで引けていた形を落とす）。代わりに**トークン全体を先に試し、駄目なときだけ**文字種の切り替わり位置で前後を削った候補を長い順に試す（`links::path_candidates`。実在するかどうかで決めるので引けていた形は定義上そのまま引ける / 全 ASCII のトークンでは候補を増やさない = ⌘+ホバー毎に走る実在チェックの syscall を増やさない / `/` で終わる断片は採らない = 存在するディレクトリへ誤って飛ばさない）。⌘+ホバーの下線とスパンは**採用した候補の範囲だけ**を指す（地の文まで下線が伸びない）。検出結果は CLI `tako links` / MCP `tako_links` で読める（`--text` で画面の写しから再現可能。開発不変条件）。`~user/…` はホーム展開しない（リンクにしない）。A/B は `TAKO_1283_LEGACY=1`（✅ 2026-09-09、#1283） | M |
| FR-1.7.3 | **⌘+クリックの行き先は 1 つの表で決まり、失敗は必ず読める**（#1283）。拡張子 → プレビュー種別の対応表の正本は `tako_core::open_plan`（md / 画像 / pdf / **動画（mp4・webm・mov・avi・mkv）** / それ以外は code）で、dispatch の `OpenFile` の既定モードと `tako links` の `open` 欄が同じ表を引く（`.mp4` は**動画プレビュー**で開く。既定アプリへは回さない = FR-1.7.1 のメニューが `デフォルトアプリで開く` を別項目として持っているので、`tako で開く` をそこへ差し替えると 2 項目が同義になり文言が実態と食い違う）。テキストとして読めないものは preview 側が理由（「バイナリファイル」）を出すので**どの拡張子でも無言にはならない**。開けなかったとき（検出とクリックの間に消えた等）は理由をサイドバーの通知欄へ出し、同じ 1 行を `persist.log` へ残す（旧実装は `eprintln!` だけ = GUI の stderr は誰も読めないので**押しても無言**だった）（✅ 2026-09-09、#1283） | M |
| FR-1.8 | 設定ファイル（GUI 設定画面は後回し可） | S |
| FR-1.9 | IME 変換中表示: 日本語等の未確定文字列・変換候補をペイン内にインライン表示する | M |
| FR-1.10 | **ペインの D&D 移動**（iTerm2 流）: ペインのタイトルバー（ターミナル / プレビュー共通）をドラッグ&ドロップして**同タブ内でペインの位置を移動**できる。ドロップ位置の象限（上下左右）で挿入位置が変わり、**ドラッグ中はドロップ先ハイライト + 「この位置に入る」挿入プレビューを表示**する（FR-2.16.10 / FR-3.11 と共通の表示系）。同等操作は `MovePane` の拡張（`target` ペイン + `direction` 指定）= CLI `tako tab move-pane --target N --down` + MCP `tako_move_pane_to_tab` の `target` / `direction`（開発不変条件。✅ 2026-06-13） | M |
| FR-1.11 | **複数ウィンドウ（ビューポート方式。#339）**: 単一アプリ状態（タブ・ペイン・tmux バックエンド・discovery）を複数の OS ウィンドウで共有し、各ウィンドウは「どのタブを表示するか」だけを持つ。各タブはちょうど 1 つのウィンドウに属する（同一タブの複数ウィンドウ同時表示はしない）。New Window（⌘⇧N）= 新規タブ付き追加ウィンドウ、タブのウィンドウ間移動、persist でのウィンドウ配置保存・復元（layout.json `windows`、単一ウィンドウ時は省略で後方互換）。赤ボタン close は複数枚ならタブを残存ウィンドウへ合流（プロセス維持）、最後の 1 枚は #312 の Dock 復帰と整合。操作は CLI `tako window list/new/close/move-tab/focus` + MCP `tako_window` で 1:1（開発不変条件）。実装 = 同一 `TakoApp` entity を全ウィンドウの root view として共有（GPUI は entity を描画中の全ウィンドウを notify で invalidate する）。✅ 2026-07-18 | M |
| FR-1.11.1 | **窓の位置・寸法を外から決める（#1442）**: 起動時は `TAKO_WINDOW_BOUNDS=x,y,w,h`（`w,h` だけなら置き先の中央）、起動後は CLI `tako window move/resize` + MCP `tako_window` の `move` / `resize` で 1:1（開発不変条件）。結果は `tako window list` の `bounds` / `display` で読める。座標は**そのディスプレイ内**（左上が原点）で、`TAKO_DISPLAY`（#1141）と併用したときはその面の中として解釈する。解釈・検査は `tako_core::platform::window_bounds::resolve` の 1 実装（env / CLI / MCP が同じ関数を通る）。撥ねるのは最小寸法（200x150）未満と置き先からのはみ出しだけで、起動時は**既定へ落ちて理由を persist.log に残し**、CLI / MCP からはエラーとして返す。指定が無いときの既定（置き先の中央 960x600）は不変。**動機は「AX で窓を動かすと本番 tako に当たる」**（AX は複数の tako-app を unix id にかかわらず同一プロセスとして返す = ユーザーの窓を動かす事故が実際に起きた）。GPUI には移動の口が無いので位置は OS へ直に頼む（macOS = NSWindow `setFrame:display:` / Windows = user32 `SetWindowPos`。**Windows は実機未検証**）。旧挙動の A/B は `TAKO_1442_LEGACY=1`、番犬は `crates/tako-control/tests/issue1442_window_bounds_watchdog.rs`。✅ 2026-09-13 | M |
| FR-1.12 | **Windows のペインは文字コードを UTF-8 に固定し、固定の決着まで入力を預かる**（#655 / #659 / #2092）。ConPTY は OEM コードページ（英語版 437・日本語版 932）で始まり、子が UTF-8 のバイト列で書くと tako が受け取る前に化けるので、ペインの起動時に疑似コンソールを 65001 へ固定する（境界 B19 = `platform::console`）。器（psmux）のペインは器の中のシェルにも固定を当てる（`encoding_pin::ContainerPin`。中の pid は器へ問い合わせて探す。**#2092 前は本番の spawn 経路でこの固定が呼ばれていなかった**）。固定はシェルと同期していない（CI 実測: プロンプトの 7〜34 ms 前）ので、**入力（打鍵・送信・マウス報告・ダイアログ応答）は固定がすべて決着するまで門（`encoding_pin::PinGate`）が預かり**、決着で預かった順に流す。上限はペインの起動から直接 500 ms・器 1500 ms（Windows CI の実測 25 回: 直接は固定 5〜16 ms・プロンプト 26〜46 ms、器は内側の固定 154〜180 ms・器の中のプロンプトの表示 226〜310 ms。実機は子の接続の成立が 43〜97 ms・器のセッションが 1 秒未満なので、その数倍）で、過ぎたら決着を待たずに流す。端末の応答（DSR 等）は預からない（子がそれを待っていると固定が決着しない）。入力に依らない出力（起動バナー・最初のプロンプト）は守れない。固定の結果は `tako check-health` / MCP `tako_check_health` の `encoding_pin` 節（ペインごとの対象・状態・理由・決着の ms・いまのコードページの読み直し・保留の記録）、諦めたペインは `issues` に名指し（直し方は `chcp 65001`）。諦めた・上限で流したことは persist.log へ 1 行（ペイン ID・対象・pid・経過・理由。送った文字は書かない）。unix は固定が要らない（`LC_CTYPE` の注入）ので門も記録も無い。A/B は `TAKO_2092_LEGACY=1`（門を作らない）、注入は `TAKO_2092_INJECT_PIN_DELAY_MS` / `TAKO_2092_INJECT_PIN_GIVE_UP` / `TAKO_2092_INJECT_PIN_TARGET`。検証は `crates/tako-core/tests/issue2092_pin_hold.rs`（Windows・器は psmux があるときだけ）+ `crates/tako-control/tests/issue2092_encoding_pin_health.rs` | M |
| FR-1.13 | **端末の問い合わせに答える**（#2350）。ペインの中のプログラムが端末へ尋ねる**色**（OSC 10 / 11 / 12 の前景・背景・カーソル、OSC 4 のパレット）・**明暗**（`CSI ? 996 n` → `CSI ? 997 ; 1\|2 n`）・**文字領域のピクセル数**（CSI 14 t）に答える（セル数の CSI 18 t は以前から alacritty の Term が答えている）。背景色で明暗を決める TUI は答えが無いと既定へ落ちるか、答え待ちで起動が遅れる。**#2350 前の実測**（main `d7d2a8a`・隔離 GUI・問い合わせ + DA1 で読む）: 直接のペインは OSC 10 / 11 / 12 / 4 と CSI 14 t が無応答（CSI 18 t だけ返る）、器（tmux 3.6b）越しは tmux が外側の端末（tako）から色を学べず OSC 10 / 11 / 12 / 4 が無応答（CSI 14 t / 18 t は tmux が答える）で、さらに tmux が tako へ転送した OSC 4 の答えを待つ間、後続の応答が約 5.5 秒止まった。**答え方の正本は `tako_core::terminal_query`**: 色は**画面の色と同じ解決**（プログラムが OSC で差し替えた色が勝ち、無ければ今のテーマ = `tako theme colors` と同じ解決。テーマを切り替えた後の問い合わせには切り替えた後の色）、明暗は OSC 11 で答える背景色の輝度（Rec. 709・128 未満がダーク）、寸法は行数 × セルの高さ・列数 × セルの幅（子へ TIOCSWINSZ で見せている値と同じ）。色と明暗はテーマが要るので tako-core が `SessionNotice::ThemeQuery` で UI 層へ渡し、UI 層は**受け取ったその場で**テーマを添えて `answer_theme_query` へ戻す（後回しにすると同じ読み取りの DA1 等と答えの順序が入れ替わる）。PTY へ書く応答の口は `send_reply` の 1 本（#2092 の番犬が数える。入力の門は通さない）。**器越しのペインの追従**: tmux は外側の端末から学んだ色をキャッシュし、テーマを切り替えても古い色を答え続ける（実測）。tmux は attach 時に外側へライト / ダークの通知（mode 2031。`CSI ? 2031 h`）を購読し、`CSI ? 997 ; 1\|2 n` を受けると色を問い合わせ直す（tmux 3.6b の `server_client_report_theme`）ので、tako は**購読を OSC タップで覚え**（`OscEvent::ColorSchemeUpdates`。追うのは `ESC [ ?` の列だけ）、テーマの差し替え（`TakoApp::apply_theme` の 1 か所。答えが変わる切り替えだけ）で購読しているペインへ知らせる。**購読していないペインへは送らない**（997 を知らない古い tmux へ送ると中のペインへ文字として漏れ得る）。器越しで二重に答えることは無い（中のプログラムの問い合わせは tmux が受けて答え、tako は tmux の問い合わせに答える）。**OSC 52 の読み出しには答えない**（クリップボードの中身 = 利用者がコピーした秘密を、ssh 先や信頼できないスクリプトを含むペインの中のプログラムへ渡さない。opt-in も作らない。1 段目は Term の設定 `osc52` = コピーだけ（既定と同じ値を `term_config` で明示して固定 = alacritty が既定を変えても通らない）で、読み出しは tako まで届かない。`ClipboardLoad` の断りは 2 段目）。何に答えるかは `tako check-health` / MCP `tako_check_health` の `terminal_queries` 節（種類ごとの答える / 答えない・`by`（tako / emulator / declined）・出どころ・理由・起動からの回数・購読しているペイン `color_scheme_panes`・要約 1 行）で読める（開発不変条件。MCP は既存ツールの説明に 1 項目 = カタログ +63 B）。**既知の限り**: 器越しの OSC 12（カーソル色）は tmux が答えない・OSC 52 の読み出しは tmux が自分のバッファで答える（どちらも tmux 側の挙動）・DECRQM（`CSI ? 2031 $ p`）は alacritty が「未対応 = 0」と答える（mode 2031 を DECRQM で確かめてから購読するプログラムは購読しない = 今までと同じ）・側路（psmux の OSC 側路ファイル）経由の `CSI ? 996 n` には答えない・**Windows（ConPTY）は実機未検証**（ConPTY が問い合わせを子のまま tako へ渡すかは conhost の版に依る。コードは OS で分岐しない）。A/B は `TAKO_2350_LEGACY=1`（色・明暗・CSI 14 t に答えず、mode 2031 の購読も覚えない）。検証は `crates/tako-core/tests/issue2350_terminal_queries.rs`（実 PTY と実 tmux の器。テーマの切り替えで器越しの答えが追従する）+ `scripts/test-terminal-queries-2350.sh`（隔離 GUI・直接 / 器越し・A/B） | M |

FR-1.9 は当初 Phase 1 の残骨格（優先度未定）だったが、Phase 3 完了を機にユーザーが
tako を日常ターミナルとして使い始めるため、**日本語ユーザーの日常使いブロッカーとして
Must に格上げ**（2026-06-11）。実装は Phase 3.5「日常使い品質」（`roadmap.md`）。

実装メモ（Phase 3.5、2026-06-11）: GPUI の `EntityInputHandler` で実装済み
（未確定文字列のインライン表示 + 注目文節の太下線 + 候補ウィンドウ位置出し）。
機械検証はセルフテスト 37〜39、見た目の確認は `.agent/manual-checks.md`。
設計原則 5（AI フルコントロール）との関係: IME は OS の入力メソッド統合であり
MCP / CLI 対応の対象外（AI からのテキスト入力は `tako_send_input` / `tako send` で
既に等価以上の操作が可能）。

アプリ内の入力欄の宛先（#561 / #1725 / #1750）: **打鍵を握る入力欄は IME の変換も同じ宛先へ束縛する**
（GPUI はかなモードの印字キーを `on_key_down` より先に IME へ渡すので、宛先が無いと 1 打鍵目から
ターミナルで変換が始まり、下線・候補窓はターミナルのカーソル位置に出て、確定 / unmark の文字列は
ターミナルの PTY へ流れる）。宛先は `AppTextInput`（git のコミット欄・ブランチ名欄・返答コメント欄・
ファイルツリーのインライン入力・⌘K パレット・Web ビューのアドレスバー・Web dock の URL 欄）で、
優先順位は打鍵の振り分け（`handle_key`）と同じ（**パレットが最優先** = 開いている間は全キーを握る
モーダルなので、裏の git の欄に変換を渡さない）。各入力欄は打鍵・⌘V・IME の確定・unmark の 4 経路を
1 つの挿入関数に通し、開く入口が他の入力欄を畳み、閉じる出口がその欄宛ての変換を捨てる。見えていない
入力欄（閉じた dock・閉じられた / 別タブの Web ペインのアドレスバー）は打鍵も変換も奪わない。
アドレスバーは現在の URL が入った状態で編集が始まるので、長い URL はキャレット側を残して `…` で詰める
（ツリーの入力欄と同じ `inline_input_window`）。機械検証はセルフテスト項目 154（#1725）/ 155（#1750）+
番犬 `issue1725_tree_inline_input_watchdog` / `issue1750_palette_web_ime_watchdog`、実 IME の見た目は
`manual-checks.md`。宛先に載せていないのはチャット入力欄（#719。TUI の入力行を映すだけなので
ターミナルペイン宛ての経路をそのまま使う）と、プレビュー編集の検索・置換欄（ペイン束縛の既存経路。
確定と unmark は欄に入る）

## FR-2 コンセプト①: エージェント集約監視

### FR-2.1 共通基盤

| ID | 要件 | 優先度 |
|---|---|---|
| FR-2.1.1 | アプリ内ターミナルの全プロセスに `TAKO_PANE_ID` / `TAKO_TAB_ID` / IPC エンドポイント等の環境変数を注入する | M |
| FR-2.1.2 | 自動生成されたペインは「呼び出し元ペインと同じタブ」に生える（1 グループ = 1 タブ） | M |
| FR-2.1.3 | ペインにタイトル・役割ラベル（例: worker-1, dev-server）を外部から設定できる | S |
| FR-2.1.4 | タブ単位でペイン群の状態（実行中/終了/エラー）をひと目で把握できる UI | S |

### FR-2.2 Layer 1: 汎用 CLI

| ID | 要件 | 優先度 |
|---|---|---|
| FR-2.2.1 | `tako split [--right\|--down] [-- <command>]` — 呼び出し元ペインの隣に新ペインを生成しコマンド実行 | M |
| FR-2.2.2 | `tako send <pane> <text>` — 指定ペインへテキスト/キー入力を送信 | M |
| FR-2.2.3 | `tako focus <pane>` — 指定ペインへフォーカス移動 | M |
| FR-2.2.4 | `tako list` — タブ/ペインの一覧を JSON で出力 | M |
| FR-2.2.5 | `tako read <pane>` — ペインの画面内容/スクロールバックを取得 | S |
| FR-2.2.6 | `tako close <pane>` / `tako title <pane> <text>` | S |
| FR-2.2.7 | 呼び出し元ペインを環境変数から自動特定（pane 指定省略時のデフォルト） | M |
| FR-2.2.8 | tako の外（通常のターミナル・tmux 内）から実行した場合は明確なエラーを返す | M |
| FR-2.2.9 | 接続情報（ソケット・トークン）をユーザー専用ファイル（0600）へ永続化し、CLI は環境変数が無い・古い（接続不可/認証失敗）場合にフォールバックする。アプリ再起動後も外部の長寿命プロセス（tmux 内のエージェント等）から手作業ゼロで接続できる | S |
| FR-2.2.10 | **IPC の受け口はデータディレクトリの深さで失われない**（#1441）: Unix ドメインソケットの実体は `sun_path` の上限（macOS 103 / Linux 107 バイト）に収まる場所へ置き（収まるうちは `<data_dir>/tako.sock` のまま・収まらなければ `$TMPDIR` / `/tmp` の短いパスへ逃がし、data dir 側には参照ファイルだけ残す）、**立たなかったときは黙らない**（通知欄 + persist.log + `check_health` の `ipc` 節に `bound` / `kind` / `path_bytes` / `limit`）。診断は CLI / MCP の両方から引ける（`tako check-health` / `tako_check_health`。**アプリへ届かないときは CLI がローカルで受け口だけを診断する** = 届かない理由そのものを読める）。旧挙動は警告 1 行だけで GUI が普通に立ち、CLI / MCP から一切操作できない状態が無言で始まっていた（2026-09-13 実測: data dir 150 バイト = ソケット 160 バイト）（✅ 2026-09-13） | M |
| FR-2.2.11 | **パイプの読み手が先に閉じても CLI は panic しない**（#1758）: `tako … \| head -1` のように標準出力の読み手が先に閉じたら、そこで打ち切って**終了コード 0 で静かに終わる**（panic メッセージを出さない。修正前は panic 2 つ + 終了コード 101）。標準エラーの読み手が閉じていても処理は続け、終了コードはそのまま（失敗は 1）。**CLI の経路で走るライブラリ側（tako-core / tako-control）の出力も同じ規則**（#1805。修正前は stderr の読み手が閉じていると `tako task gate check` が `[gate check] command:` で panic して 101・結果を出さず、`tako mcp serve` も gate check の `tools/call` に応答せず 101 で落ちた）。GUI のプロセスで走るライブラリの出力は変えない（規則は `tako` / `tako-mcp` の入口が採ったときだけ効く）。`tako mcp serve` は**クライアントが stdout の読み手を先に閉じたら静かに 0**（#1805。stdin の EOF と同じ扱い。痕跡は persist.log へ 1 行。修正前は `error: stdout への書き込みに失敗: Broken pipe` を出して 1）。macOS / Windows 共通の 1 実装（`tako_core::stdio`。SIGPIPE を既定へ戻す案を採らない理由と、MCP クライアント 3 系統の閉じ方の実測は `.agent/conventions.md`「CLI の出力はパイプの切断で panic させない」）（✅ 2026-09-27 / #1805 ✅ 2026-10-18） | M |
| FR-2.2.12 | **繋がったまま返事をしない GUI で CLI が永久に待たない**（#2123）: IPC の 1 往復に応答を待つ上限を掛け、過ぎたら「tako アプリが N 秒応答しない（接続は受けたが返事が無い = GUI が固まっている可能性。要求は届いているので、GUI が戻ると後から処理されることがある。待つ上限は env TAKO_IPC_TIMEOUT_SECS（秒・0 で上限なし））」で失敗を返す（CLI・MCP の stdio ブリッジ `tako-mcp`・`tako queue` の状況が同じ 1 行 = `protocol::reply_timeout_message`）。上限は要求の種類から決まる 1 実装（`protocol::reply_timeout`）: 順番待ちの受付・poll・状況は 30 秒、届かなくても構わない心拍（`started`）と `done` は 5 秒、**外の仕事の終わりを待つ要求**（言語サーバの取得・更新・setup の各段・リモートの setup・ファイル操作・git の書き込み（同じリポジトリで 1 本ずつなので stage も前の push を待つ）・設定の共有）は上限なし、それ以外は安全網の 300 秒（UI スレッドで答える要求が 300 秒かかるなら GUI が固まっている。background の要求は各自の上限 = 言語サーバ 120 秒・検索の時間上限を持つ）。時間切れは他の候補（発見ファイル）へ回らない（固まった GUI で待ちが候補の数だけ伸びるため）。unix は「繋がってから上限まで」の締め切りを読み書きの度に入れ直す（少しずつ届く応答で延びない。入れ直しを拒まれた読み = macOS は相手が閉じて送受信の両方が閉じたソケットへの setsockopt に EINVAL を返す、は待たない読みを 1 回だけして届いている応答を読み、未着なら待たずに元の誤りを返す = 上限を越えて待たない。#2169）、windows の named pipe（同期ハンドルに読み取りの時間切れが無い）は往復を別スレッドで回して待つ。env `TAKO_IPC_TIMEOUT_SECS`（正の整数で全要求をその秒数・0 は上限なし）・A/B `TAKO_2123_LEGACY=1`（上限なし = 修正前）。実経路テスト `bash scripts/test-ipc-timeout-2123.sh`（偽の受け口 = 返事をしない / 応答を半分で止める / accept しない）（✅ #2123 / #2169） | M |
| FR-2.2.13 | **立った受け口がパスから切り離されても GUI が自分で張り直す**（#2002）: 同じパスを別の受け口が作り直して閉じる・ソケットファイルを消されると、GUI は生きているのに CLI / MCP は `Connection refused` / `No such file or directory` のまま戻らなかった（2026-10-12 の本番。GUI の accept スレッドは accept で待機・CPU 0%）。いまは accept スレッドが 10 秒ごと（`TAKO_IPC_WATCH_MS`）にパスの実体を bind した時の値と比べ、消えていた・誰も受けない別の実体に替わっていたら同じパスへ張り直して persist.log に 1 行残す。**生きた別の受け口が握っている間は奪わない**・ソケットではないファイルには触らない・置き場の dir が消えていたら作り直さない・終了の最中は張り直さない・drop は自分の実体のときだけファイルを消す。記録は `check_health` の `ipc.watch`（CLI / MCP `tako_check_health`）と、切り離されたままなら `issues` の warning。**アプリへ届かないときの `tako check-health` は接続情報の pid で「GUI は動いているのに受け口が応答しない」と「GUI が動いていない」を分け、次の一手を出す**。A/B `TAKO_2002_LEGACY=1`。実経路テスト `bash scripts/test-ipc-listener-recover-2002.sh`、設計は `.agent/architecture.md` の IPC ソケットの置き場 6（✅ 2026-10-24） | M |
| FR-2.2.14 | **GUI の fd 上限を起動時に引き上げ、使用数を読めるようにする**（#2359）: Finder / Dock から起きた tako は launchd の既定（`launchctl limit maxfiles` = soft 256・hard 無制限）のまま動き、GUI は 1 ペインで fd を 4 本使う（PTY の master・alacritty の kqueue・子の終了を知らせる UnixStream の対。本番 28 ペインで 121 本 = 基礎 9 + 4 × 28）ので約 60 ペインで上限に届き、IPC の接続を受けても接続ごとのスレッドの `try_clone` が EMFILE で落ちてすぐ閉じるので CLI / MCP は届かず（字面は送信の ENOTCONN / EPIPE / EINVAL か「応答が返らなかった」）・新しいペインの PTY も開けなかった（#2002 の段 1 の注入 a2。IPC の接続 1 本は GUI の fd 2 本）。いまは起動時（最初の PTY より前・隔離の env を決めた後）に soft を **min(hard, OPEN_MAX = 10240, kern.maxfilesperproc)** へ上げ（hard は触らない。macOS の setrlimit(2) が RLIMIT_NOFILE に `min(OPEN_MAX, rlim_max)` を求める・先例は Ghostty / WezTerm / rustc の compiletest）、結果を persist.log に 1 行残す。上げられなくても起動は続ける（`failed` を記録）。**ペインの PTY の子（シェル・tmux の attach と、それが起こす器の tmux サーバー = 器のペインのシェル）には起動時の soft を渡す**（器の tmux サーバーだけは引き上げた値を受け継ぎ、器のペインのシェルへ段が起動時の値を戻す = FR-2.2.16・#2367）（fork の子ハンドラ `fd_inherit::on_fork_child` が掃除の後に `fd_limit::restore_for_child`。ユーザーのシェルの `ulimit -n` は修正前と同じ = Ghostty #4232 と同じ判断）。tako が自分で起こす裏方（言語サーバ・git・`remote serve` の daemon）は引き上げた値を受け継ぐ。`check_health`（CLI / MCP `tako_check_health`）の `fd_limit` 節に使用数（`open`）・上限（`soft` / `hard`）・使用率・起動時の結果（`startup.outcome` = raised / already_enough / hard_capped / failed / legacy、`children_soft`）を出し、使用が soft の **80%** 以上なら `issues` に warning（次の一手 = 使っていないペイン・タブを閉じる。警告の後に残る 20% は check-health 自身の探針の分）、引き上げに失敗したら warning。**Windows は対象外**（ハンドルは 1 プロセス 2^24 本まで・std は CRT の fd を使わない）で `fd_limit.applicable=false`。A/B `TAKO_2359_LEGACY=1`（上げない）・注入 `TAKO_2359_INJECT=setrlimit-fail`。実経路テスト `bash scripts/test-nofile-limit-2359.sh`、実 PTY のテスト `crates/tako-core/tests/pty_nofile_child.rs`、設計は `.agent/architecture.md` の IPC ソケットの置き場 7（✅ 2026-10-24） | M |
| FR-2.2.15 | **一時的に繋がらないだけの生きた受け口を残骸と読んで奪わない**（#2360）: 起動時の IPC ソケットの生存確認は connect 1 回だけで、繋がらなければ固定パスを unlink して張り直していたので、生きた GUI の受け口が満杯（backlog あふれ = macOS は ECONNREFUSED。#2002 の段 1 の注入 a1 で 300 本中 27 本）の瞬間に、多重起動のガード（#113 の control.json の pid）が盲目な（control.json が無い・壊れている）2 つ目の tako-app が起動すると、生きた受け口を奪った（隔離 GUI 2 本で実測: 奪われた側は生きたままパスから切り離され、そのペインのシェルの CLI / MCP は 2 つ目へ届く）。いまは繋がらなければ接続情報（control.json と instances/）にそのパスの持ち主として載った自分以外の生きた tako-app を探し（pid の再利用はプロセス名で外す）、居れば奪わない。居なければ 50 / 100 / 200 ms の間を置いて繋ぎ直し、最後まで繋がらないときだけ残骸として作り直す（落ちた後の初回起動だけ最大 350 ms 遅れる）。判定は `tako_core::ipc_socket::judge_occupant` の 1 実装で、明示パスの受け口（#1994）と #2002 の見張りの「別の実体の生死」も同じものを通る。**奪わずに一時パスへ逃げた 2 つ目はセカンダリで起動する**（多重起動の第三のガード: 固定パスを握る生きた受け口は同じ data dir の別の tako = 復元・保存・tmux・control.json を奪わない）。判定は persist.log に `[pid N]` つきで 1 行（奪わなかった・残骸を作り直したの両方。パスは載せない）、`check_health`（CLI / MCP `tako_check_health`）の `ipc.fixed_path`（`state` = live / busy / dead・`held`・`pid`・`reason_text`）と CLI の「固定パス:」の行。A/B `TAKO_2360_LEGACY=1`（connect 1 回・第三のガード無し）。番犬 `crates/tako-control/tests/issue2360_socket_liveness_watchdog.rs`、実経路テスト `bash scripts/test-ipc-liveness-2360.sh`、設計は `.agent/architecture.md` の IPC ソケットの置き場 4（✅ 2026-10-24） | M |
| FR-2.2.16 | **器の tmux サーバーにも引き上げた fd 上限を渡し、器のペインのシェルには起動時の値を戻す**（#2367）: FR-2.2.14 はペインの PTY の子へ起動時の soft を戻すので、attach クライアントが起こす器の tmux サーバーも起動時の値（Finder 起動なら 256）で動いた。サーバーは器の 1 ペインで fd を 3 本（ペインの PTY の master・attach クライアントのソケットとその tty。基礎 6 本。本番 27 ペインで 87 本 = 6 + 3 × 27）使い、tmux 3.6b は自分の上限を変えず（`spawn.c` は rlimit に触れない）、上限に届くと新しいペインは `create window failed: fork failed: Too many open files` で作れず（tako では新しいタブのペインが何も残さず消える。既存のペインは生きて応答する）、ちょうど上限では生きたまま**すべての問い合わせ**が `server exited unexpectedly` で返らなくなる（soft を絞った隔離の tmux サーバーでの実測）。soft 256 では約 80 ペインで詰まる。いまは**器の attach クライアントだけ起動時の値へ戻さず**（`fd_limit::with_children_raised`。それが起こすサーバーが GUI の引き上げた値を受け継ぐ）、**器の中のペインの子は GUI 自身の実行ファイルを通して起こす**（`wrap_options` が `new-session` の残余引数を `<GUI の実行ファイル> __tako-container-exec <起動時の soft> [<コマンド>]` にする。2 語以上なので tmux はシェルの `-c` を挟まず `execvp` で直に起こす。段 = main の 2 文目の `fd_limit::exec_container_child_if_requested` は soft を起動時の値まで下げ（上げない・hard は触らない）、fd 3 以上へ CLOEXEC を立ててから、tmux が自分で起こすはずだった形 = `$SHELL` を `-<名前>` でログイン起動 / `$SHELL -c <コマンド>` を exec する。exec の上乗せは debug ビルドで約 11 ms（同じフレームワークをリンクした空の実行ファイルで約 3 ms））。ユーザーのシェルの `ulimit -n`・`$0`・`SHLVL`・`SHELL` は修正前と同じ。段を通さない（クライアントも戻す = 修正前と同じ）のは: A/B `TAKO_2367_LEGACY=1`・GUI が引き上げていない（`hard_capped` / `already_enough` / `failed`）・段の実行ファイルが置かれていないか、走っている間に置き換わった（dev / inode が変わった = 更新・古い版へ戻した `.app`・再ビルド。印を知らない版の GUI は印を無視して窓を開いてしまうため）。**既に走っているサーバーの上限は変わらない**（上がるのは次にサーバーが起きたとき = 器のセッションがすべて終わった後。tako の再起動では器が残るので上がらない）。`check_health` の `fd_limit.container` に器のサーバーの `state`（running / not_running / unreachable）・pid・使用数（`proc_pidinfo`）・`soft` / `hard`（pid は `display-message`、上限は同じサーバーで 1 度だけ `run-shell 'ulimit -Sn; ulimit -Hn'` = `/bin/sh -c` のサーバーの子が受け継ぐ値を読んで覚える。run-shell は fork とパイプが要るので上限の間際では返らない = 分けておけば間際でも使用数と覚えた上限で申告できる。どちらの命令もサーバーを起こさない）・`limit`（raised / predates / legacy / launch_value = `fd_limit::classify_container`）・`pane_capacity`・`children_soft` を出し、使用が soft の 80% 以上・問い合わせが届かないときは `issues` に warning（次の一手 = 使っていないペインを閉じる。predates なら「次にサーバーが起きたとき上がる」）。CLI の人が読む出力に「器の tmux サーバー: …」の 1 行。persist.log には器のペインを起こした直後に器のサーバーの上限を 1 行（サーバーが変わったときだけ・裏のスレッドで読む）。**Windows（psmux）は対象外**（FR-2.2.14 と同じ理由で上限の仕組みが無い。`container_child_argv` は常に None）。実経路テスト `bash scripts/test-tmux-server-nofile-2367.sh`、実 tmux の結合テスト `crates/tako-cli/tests/issue2367_tmux_server_nofile.rs`（CLI の実行ファイルを段に置く）、設計は `.agent/architecture.md` の IPC ソケットの置き場 8（✅ 2026-10-24） | M |
| FR-2.2.17 | **Windows の名前付きパイプの待ち受けを作れなくても accept ループを止めず、間を置いて作り直す**（#2361）: Windows の受け口は名前付きパイプで、accept スレッドは 1 本受けるたびに次のインスタンスを作って待つが、その作成（`CreateNamedPipeW`）が 1 回でも失敗すると accept ループが `break` してスレッドごと終わり、待ち受けのインスタンスが 1 つも残らなかった（以後の CLI / MCP の stdio は届かず、GUI は気づかないので再起動するまで戻らない = #2002 と同じ「GUI は生きているのに受け口が応答しない」の Windows 側。コードと注入で再現・実機では未確認）。いまは作れなければ **100 ms から倍々で最長 5 秒**の間を置いて作り直し（失敗 1 回につき眠りは 1 回 = 作成は最長 5 秒に 1 回まで・accept スレッドの中で眠るので UI スレッドは止まらない）、作れたら受け口が戻る。作り直しを待っている間に drop が止めにかかったら作らずに抜ける（待ち受けを残さない）。間の決め方と記録は `tako_core::ipc_socket::create_with_retry` / `PipeInstances` の 1 実装。続いた失敗は 1 回の途切れとして persist.log に**始まりと戻りの 2 行**（理由・続いた回数・途切れていた長さ・`[pid N]`。パイプ名はユーザー名を含むので載せない）、前に書いた途切れの始まりから 60 秒以内に始まった途切れは書かずに数え、次の 1 行に「畳んだ N 回」として載せる。記録は `check_health`（CLI / MCP `tako_check_health`）の `ipc.pipe`（`listening`・`failures`・`streak`・`outages`・`folded_outages`・`retry_ms`・`down_secs`・`error`・`last_recovery`）と CLI の「パイプの待ち受け:」の行、作れないままなら `issues` の warning（この間は名前付きパイプ越しには届かないので、読めるのは HTTP の MCP と GUI 自身の診断。戻った後は記録で読む）。アプリへ届かないときの CLI のローカル診断の次の一手も Windows はこの作り直しを案内する。**macOS / Linux は対象外**（unix の受け口は FR-2.2.13 の見張り）。A/B `TAKO_2361_LEGACY=1`（作れないと止まる・記録しない）・注入 `TAKO_2361_INJECT=<回数>`（本体の受け口が 1 本受けた後のインスタンスの作成をその回数だけ `ERROR_NO_SYSTEM_RESOURCES` で失敗させる。本番と同じ `named_pipe::create_server_instance` の入口で効く）。Windows CI のテスト `crates/tako-control/src/ipc.rs` の `windows_tests::issue2361_*`、設計は `.agent/architecture.md` の IPC ソケットの置き場 9（✅ 2026-10-24） | M |
| FR-2.2.18 | **Windows でペインを閉じる・言語サーバを止めると、コンソールの孫まで止まる（ペインから起こした GUI アプリは残す）**（#2454。境界 B30 = `tako_core::platform::job`）: Windows の `TerminateProcess` は子孫を道連れにしないので、ConPTY を閉じても `CREATE_NO_WINDOW` で起きた孫（stdio をパイプにした MCP サーバなど = 疑似コンソールに繋がらない）が残り、npm の `.cmd` の言語サーバは `Child::kill` で cmd.exe だけが止まって node.exe が残った（FR-2.33.9 の実機で 1 日 46 本）。いまは**器なしのペインの PTY 直下の子**と**言語サーバ 1 つ**ごとに `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE` の Job を作って起こした直後に入れ、**寿命の決まり**: ペインを閉じる（`TerminalSession` の drop）と**コンソールのプロセスは止め、GUI のアプリとその子孫は残す**（`code .`・`notepad` をペインと一緒に消さない = Windows Terminal と macOS の `open` と同じ。GUI かは実行ファイルの PE ヘッダのサブシステムで見分け、親を Job の中でたどって GUI に当たるもの = VS Code の統合ターミナルのシェルや言語サーバも残す。止めたあと `KILL_ON_JOB_CLOSE` を外してから閉じる）。`tako lsp stop` / GUI の終了（`ServerProcess::kill`）は言語サーバの子孫を全部止める。tako を普通に終了したときはペインを閉じたのと同じ（`on_app_quit` が全ペインの Job を `release_descendants_on_quit` で手放す。呼び出しの位置は tako-app の番犬が固定）。全部止まるのは tako が落ちて OS がハンドルを閉じたときだけ。**器（psmux）のクライアントは入れない**（器のサーバーはクライアントの子として起き、入れると GUI の終了で器ごと消える。判定は `tmux_backend::is_container_client`）。`CREATE_BREAKAWAY_FROM_JOB` で明示的に外へ出る子は出られる（`BREAKAWAY_OK`。付けないとその子の起動が失敗する）。入れられないときは従来の止め方（直下の子だけ）へ落ち、panic しない。unix は変えない（PTY の HUP / stdin の EOF と kill）。A/B `TAKO_2454_LEGACY=1`（Job を作らない）・`TAKO_2454_KILL_GUI=1`（ペインを閉じるときも GUI ごと全部止める）・注入 `TAKO_2454_INJECT=assign_fail`。Windows CI の実プロセスのテスト `crates/tako-core/tests/issue2454_pane_job_windows.rs`（PowerShell が `CREATE_NO_WINDOW` で起こした孫）・`crates/tako-control/tests/issue2454_pane_gui_windows.rs`（同じペインの GUI の孫 = テスト専用の `tako-gui-sleeper` は残り、コンソールの孫は止まる。普通の終了の経路を通って drop せずに終わる子でも同じで、通さずに終わる子 = 落ちたときは GUI も止まる）・`crates/tako-control/tests/issue2454_lsp_job_windows.rs`（`.cmd` → cmd.exe → `exit` を無視する偽サーバ）。設計は `.agent/architecture.md` の「子孫の回収（境界 B30）」。**Windows 実機での `Win32_Process` の数え上げは未実測** | M |

tmux ベースのオーケストレーター（spawn-worker.sh 等）はこの CLI を呼ぶことで連携する。
専用の連携機能は作らない（CLI が十分に汎用であることが要件）。
レイアウト操作（リサイズ・一括調整・詳細な読み取り）の完全なカタログは **FR-2.5** を正とする。

FR-2.2.2 実装メモ（Issue #32・2026-07-02）: claude 等の**全画面 TUI への改行つき送信は
送達確認ループ**で配送する — 信頼ダイアログ検出 → 承諾（orchestrator spawn は
`~/.claude.json` への事前信頼でダイアログ自体を抑止）→ bracketed paste 貼り付け →
送信 Enter は貼り付けと分離した単独キーとして遅延送信 → 入力欄が空へ戻ったことを検証、
残っていれば Enter 単独再送（最大 4 回）。一括書き込みは claude TUI が貼り付けと誤認して
改行を「送信」と解釈せず入力欄に残留するため（実測）。シェルへの送信は従来どおり即時
書き込み（挙動・レイテンシ据え置き）。実装は `tako-control::claude_tui`（検出は実 TUI の
採取画面フィクスチャでテスト、送達は実 claude E2E `claude_tui_e2e` で検証）。

FR-2.2.2 追補（Issue #572・2026-07-27）: **claude は生成中に打たれた入力を入力欄では
なく内部の「メッセージキュー」へ入れる**（ターン終了時に送信される）。この間の入力欄は
空で、代わりに dim のヒント `Press up to edit queued messages` が出る。tako はこれを
「残留テキスト」と誤認していたため、Enter 単独送達が発火しない Enter を 5 回空撃ちして
未検証で終わり、`tako read` の `input_status` も `style=ghost` + テキストありに見えて
master が「送達に失敗した」と読み違えていた。是正:

- 入力欄が「空か」は **dim かどうか**で判定する（`claude_tui::input_text_is_all_dim` /
  `input_has_user_text`。tmux 経路は `capture-pane -e` で属性つき採取）。dim =
  プレースホルダ / AI のゴースト提案 = ユーザー入力ではない。文言リストでは
  AI 生成のゴースト提案を原理的に網羅できないため属性を根拠にする
- キュー滞留は `claude_tui::queued_messages_pending` で検知し、`tako read` の
  `queued_messages_pending` / `worker_status` の同名フィールド + `queued_messages_pending`
  イベントとして公開する（キューはペインが消えると失われるため、master が
  ペインを閉じる前に気付けるようにする）
- **生成が止まっているのにキューが残っている**ときは tako が送り出す（`Up` で
  キュー先頭を入力欄へ戻して `Enter`。claude 自身の案内どおりの操作）。生成中かは
  `is_busy` の文言ではなく **画面が変化していないこと**（`claude_tui::screen_settled`）
  で判定する（実測で 120 行のリスト生成中に `is_busy` が false を返した）

FR-2.2.2 追補 2（Issue #790・2026-08-14）: **claude worker への送達は 2 層**にする。
第 1 層は claude の Cross-Session Messaging（peer messaging。v2.1.224+）= 受信箱の Unix
domain socket へ直送する経路で、画面解析もキー操作も伴わない。第 2 層は従来のキー操作経路
（上記の送達確認ループ）。第 1 層が使えなければ第 2 層へ落ちる。

- 判定材料: claude のバージョン / `peerProtocol` / `kind == "interactive"` / 受信箱 socket の
  実在 / 資格情報の可読性。**可用性はサーバー側 gate に依存し env で強制できない**ため、
  実行時に見て落ちる（実装は `tako-control::peer_messaging` / `delivery`）
- 発見: `<config dir>/sessions/<pid>.json`（`messagingSocketPath` 等）と
  `<config dir>/sessions/<pid>.<hash>.key`（`peerToken`。0600）。**config dir ごとに別**なので、
  アカウント切替（#504 / #512）の worker も `transcript::claude_config_dirs()` で解決する。
  鍵は `procStart` がレジストリと一致するものだけ使う（pid 再利用の残骸を弾く）
- **適用は「エージェント管理下の worker」に限る**: 受信側の本文には tako から抑制できない
  定型の前置き（「別の claude セッションから届いた」「peer は権限昇格を与えない」「保留中
  プロンプトの承認として扱うな」）が付く。master → worker の指示はその関係そのものなので
  正確だが、人間由来の送達（master への指示・承認の代行）では意味が変わる。判定は
  ペインの role（`peer_messaging::agent_managed_role`）。ペインが解決できない経路では
  worker レジストリ（#390）に active で載っているかで見る
- **送り切ったらフォールバックしない**（二重投函の防止）。落ちてよいのは可用性判定と
  接続失敗（受信側が 1 行も読めていないと言える段階）だけ
- 送達検証は transcript で行う。受信の姿は 2 形態あり**両方**を受理する:
  `type:"user"` + `origin.kind == "peer"`（ターンとして処理）と `type:"attachment"` の
  `queued_command`（生成中・ダイアログ中に進行中ターンへ折り込まれた）。加えて
  `queue-operation`（enqueue）はキュー投函として扱う。判定は「送信直前に控えた
  ファイル長より後ろの追記分」だけを見る（`sessions::now_iso()` は秒精度で transcript は
  ミリ秒なので、時刻文字列の比較では同じ秒の痕跡を取りこぼす）
- どちらの経路で送ったかは `<data_dir>/persist.log` に残す（無音で経路が変わらない）。
  `TAKO_PEER_MESSAGING=off`（常に第 2 層）/ `only`（落ちずにエラー）で経路を固定でき、
  フォールバックを機械検証できる
- 受信箱は claude の起動途中に開く（実測 1.1 秒）。spawn 直後だけ数秒の猶予を置いて
  再試行してから第 2 層へ落ちる（1 回で諦めると長文プロンプトで最も効く場面を取り逃す）
- codex / agy と Windows は従来経路のまま（Windows は `cfg(unix)` で構造的に対象外）

FR-2.2.8 の「明確なエラー」は FR-2.2.9 の解決順（環境変数 → 接続情報ファイル）を
すべて試した上での不在を意味する（tako が起動していれば、tako の外からでも接続できる）。
MCP stdio ブリッジ（`tako mcp serve`）のフォールバックは「環境変数がある = tako 内で
起動されたブリッジ」に限る（tako 外のセッションで 0 ツールを保つ。FR-2.3.2 と整合）。

FR-2.2.7 実装メモ（Issue #567・2026-07-27）: `TAKO_PANE_ID` はアプリ再起動やシェルの
再利用をまたぐと古くなる。**`tako master` / `tako solo` の起動先ペインは古い ID で
止まらない**: `resolve_pane` 操作（pid 祖先辿り → pane そのまま → stale pane map（#210）。
role 検索へは落とさない）で現世代へ読み替え、それでも特定できなければ「呼び出し元不明」
として新規タブへ起動する。アプリへ接続できないときだけエラーで止め、復旧手順
（アプリ起動 / `unset TAKO_PANE_ID`）を添える。master 消失からの復旧という最も急ぐ場面で
「ペイン N が見つからない」で手が止まらないことを優先する。他の CLI 操作は従来どおり
FR-2.2.7 の環境変数解決のまま（誤ったペインへ副作用を起こさない方を優先）。

安全制約（Phase 2 で確定）: CLI / MCP からの `close` は「最後のタブの最後のペイン」を拒否する。
アプリ終了に等しい操作は UI（cmd+W / cmd+Q）のみに許す（FR-2.5.9 の安全性方針の具体化）。

### FR-2.3 Layer 2: 内蔵 MCP サーバー（最大の差別化点）

| ID | 要件 | 優先度 |
|---|---|---|
| FR-2.3.1 | アプリ内に MCP サーバーを内蔵し、ペイン操作（split/send/read/focus/list/title）をツールとして公開 | M |
| FR-2.3.2 | アプリ内で起動したエージェントが**環境変数経由で自動発見**でき、ユーザー設定ゼロで使える | M |
| FR-2.3.3 | 接続元がどのペインかをサーバー側で特定でき、操作は同タブ内がデフォルトスコープ | M |
| FR-2.3.4 | アプリ外プロセスからの接続は拒否する（最低限のアクセス制御） | M |
| FR-2.3.5 | 破壊的操作（close 等）の許可ポリシーを設定で制御できる | S |

公開ツールはペイン生成・テキスト送信に限らず、**FR-2.5 のレイアウト操作セット**
（削除・フォーカス・サイズ調整・レイアウト一括調整・構造化された読み取り）を網羅する。

実装メモ（Phase 3、2026-06-11。経緯と検証結果は `architecture.md`「Layer 2」節）:

- FR-2.3.1: 実装済み。MCP エンジン（`tako-control::mcp`）はトランスポート非依存で、
  実行は CLI と同じ `dispatch` を呼ぶ。公開ツールは 12 個（FR-2.5 の操作セットと 1:1）
- FR-2.3.2 の「設定ゼロ」の現実解: Claude Code（2.1.x）には**環境変数だけから MCP サーバーを
  自動発見する機構が無い**。そのため (a) Streamable HTTP（`TAKO_MCP_URL` 注入）に加えて
  (b) stdio ブリッジ `tako mcp serve` を提供し、**初回 1 回だけ**
  `claude mcp add --scope user tako -- <tako のパス> mcp serve` を登録すれば、以後は
  どのプロジェクト・どのペインでも設定なしで使える（ブリッジが起動毎に TAKO_* を読むため、
  セッション毎に変わる URL / トークンに依存しない）。tako の外では 0 ツールを返し無害
- FR-2.3.2 のブリッジの常駐コスト（#2021）: ブリッジは claude 1 本につき 1 本常駐するので、
  **登録のコマンド（`tako mcp serve`）を変えずに**常駐する側を軽くする（clap を通らない・カタログを
  使い捨ての子プロセスで組む・macOS の .app では libSystem だけをリンクする `tako-mcp` へ exec で
  引き渡す）。応答のバイト列は変えない（番犬 `crates/tako-cli/tests/issue2021_mcp_bridge.rs`）。
  登録を変えないので移行は要らない。仕組みは `architecture.md`「Layer 2」の stdio ブリッジ
- FR-2.3.2 の worker への適用（#986）: **master だけでなく worker / git resolve でも
  ゼロコンフィグで繋がる**。codex は MCP 子プロセスへ親環境を渡さない
  （実測 0.153.0: `env_vars` に挙げた名前だけが届く）ので、起動コマンドへ
  `-c mcp_servers.tako.command/args/env_vars` を**一時注入**する（正本は
  `orchestrator::agent::codex_mcp_args` の 1 実装で、master / worker / git resolve が共有）。
  恒久登録（`tako setup-mcp` = #979）ではなく `-c` なのは、`~/.codex/config.toml` を汚さず
  **tako の外で起動した codex にツールを出さない**ため。恒久登録がある機でも `-c` が
  後勝ちするので、登録が古いバイナリを指していても worker は正しいブリッジへ繋がる。
  claude / agy は親環境をそのまま子へ渡す（agy は実測 1.1.27 で `TAKO_*` 15 個が届く）ので
  一時注入は不要・agy の CLI にその手段も無い（`agy --help` に `-c` / `--mcp-config` が無い）
- FR-2.3.3: 呼び出し元ペインの特定は stdio = `TAKO_PANE_ID`、HTTP = `X-Tako-Pane` ヘッダ、
  **どちらも無いときは pid 祖先辿り**（#986。`tako mcp serve` が `Request::ResolvePane` で
  アプリに問い、#288 / #567 と同じ 1 実装で backend セッション → ペイン ID を解く）。
  env が在るときは 1 往復も増やさない（claude 経路は 1 バイトも変わらない）し、
  接続情報が無いとき（= tako の外）は問い合わせに行かない（FR-2.3.2 の 0 ツールを保つ）。
  pane 省略時のデフォルト対象が呼び出し元（= 同タブ）になる。タブを越える操作は
  ID の明示指定が必要。**ハードなスコープ強制は未実装**（FR-2.3.5 のポリシー制御と併せて後段）
- FR-2.3.4: 実装済み。localhost バインド + Bearer トークン（IPC と共有）+ Origin 検証。
  不正トークンは 401、非 localhost Origin は 403
- FR-2.3.5: 未実装（S。後段フェーズ）

### FR-2.4 Layer 3: パッシブ検知（opt-in）

| ID | 要件 | 優先度 |
|---|---|---|
| FR-2.4.1 | OSC 7（cwd 通知）/ OSC 133（プロンプトマーク）のシェル統合。**ユーザーのファイルを書かずに届ける**（unix = 環境変数の注入 / Windows = ペインの PowerShell の起動引数。#2432） | M |
| FR-2.4.2 | ペイン内の子プロセスが TCP ポートを listen したことを検知 | S |
| FR-2.4.3 | 検知時に「プレビューを開く？」等の**提案チップ**を表示。承諾時のみペイン生成（強制分割はしない） | S |
| FR-2.4.4 | パッシブ検知全体を設定で無効化できる | M |
| FR-2.4.5 | シェル統合経路で tako 内の zsh に**履歴ベースの入力予測**（ゴーストテキスト）を提供する | S |
| FR-2.4.5.1 | 入力予測の**確定キーをその場で案内**し（回数制限つきチュートリアル）、**ゴースト表示中だけ Tab でも確定**できる | S |
| FR-2.4.6 | シェル統合経路で tako が開いたシェルの PATH に**同梱 CLI のディレクトリ**を自動追加する（tako の中では常に `tako` が打てる。FR-2.14.5 の tako 内側の解） | M |

実装メモ（FR-2.4.2〜2.4.4 は 2026-06-12 完成）:

- 検知は `tako-core::ports`（libproc + tty 突き合わせ、3 秒ポーリング）→ list / MCP の
  `listen_ports`。詳細は `architecture.md`「Layer 3」節
- チップは検知ペイン下端のインライン表示（新規ポートごとに 1 件、却下でそのポートが
  消えるまで再提案しない）。承諾アクション = **Web ビューペイン生成**（FR-3.8。
  検知元ペインの右に開く。✅ 2026-07-13 #155 で外部ブラウザ `open` から差し替え。
  webview を作れない場合は外部ブラウザへフォールバック）
- OFF は `settings.json` の `port_detect` + dispatch `PortDetect`
  （`tako portdetect on|off` / MCP `tako_port_detect`、計 18 ツール）+
  env `TAKO_PORT_DETECT=0|false|off`。無効化時は検知済み情報（listen_ports・チップ・
  却下記録）も掃除する

実装メモ（FR-2.4.5。2026-07-27 / Issue #600 完成）:

- 実体は **zsh-autosuggestions v0.7.1（MIT）をバージョン固定で同梱**したもの
  （`crates/tako-core/shell-integration/zsh-autosuggestions/`。実行時ダウンロードはしない。
  出所は同ディレクトリの `PROVENANCE.md`、告知は `THIRD-PARTY-NOTICES.md`）。
  予測ソースは上流既定（history）、確定は右矢印（`forward-char`）
- **注入経路は FR-2.4.1 と同じ ZDOTDIR 一本**。`~/.zshrc` は書き換えず、
  `TAKO_PANE_ID` があるインタラクティブ zsh だけが読み込む = **tako の外の zsh は不変**
- **読み込みは `.zshenv` 直下ではなく最初の `precmd`**（= ユーザーの `.zshrc` の後）。
  理由は 2 つあり、どちらも `.zshenv` 直下では成立しない:
  1. 二重注入ガード（ユーザーが自前で導入済みか）は `.zshrc` を読み終えるまで判定できない。
     判定できたら tako は**一切手を出さない**（この設定も効かない）
  2. 上流の推奨読み込み順が「他プラグイン（zsh-syntax-highlighting 等）より後」
- ON/OFF は `settings.json` の `autosuggest`（既定 ON）+ dispatch `Autosuggest`
  （`tako autosuggest on|off` / MCP `tako_autosuggest`）+ env `TAKO_AUTOSUGGEST=0|false|off`。
  シェル側の完全な逃げ道は `TAKO_NO_AUTOSUGGESTIONS=1`
- 切替は**稼働中のペインにも次のプロンプトから効く**。環境変数は spawn 時に凍結して
  既存シェルへ届かないので、状態は `<data_dir>/shell-integration/autosuggest`
  （`on` / `off`。**不在は ON**）に置き、zsh 側が毎プロンプト読む
- 対象は zsh のみ。fish は autosuggestions 標準搭載のため何もしない。bash は対象外
  （ble.sh は重量級。将来検討）。Windows は PowerShell が PSReadLine の予測入力を
  標準搭載しているため注入不要（対応マトリクスでは `Unsupported` + 理由を明示）
- 既知の限界: zinit turbo のような**遅延ローダー**でユーザーが後から
  zsh-autosuggestions を読み込む構成では、最初のプロンプト時点で判定できないため
  二重に読み込まれうる。その場合は `TAKO_NO_AUTOSUGGESTIONS=1` で tako 側を止める

実装メモ（FR-2.4.5.1。2026-07-27 / Issue #614 完成）:

- 解決するのは**確定キーの発見不能**。実機報告は「予測は出るが → で確定できると分からない。
  多分みんな Tab を押す」。対処は「その場で案内する」と「実際に Tab でも確定できる」の 2 本立て
- **案内はゴーストの直後に薄く出す**（`[→ か Tab で確定]` / `[→ or Tab to accept]`）。
  ゴースト本体と同じ `POSTDISPLAY` へ足し、`ZSH_AUTOSUGGEST_HIGHLIGHT_STYLE` で同じ薄さに塗る。
  下段への `zle -M` 方式は採らない（1 打鍵遅れる・画面下端でスクロールが跳ね、
  scrollback に空行が残る。実測で確認）
- `POSTDISPLAY` は**プラグインが「確定するテキスト」として読む変数**でもあるので、
  プラグインが触る前に必ず外す。関門は `_zsh_autosuggest_highlight_reset`（全 widget の入口）と
  `_zsh_autosuggest_highlight_apply`（全 widget の出口・`zle -R` の直前）の 2 つだけを包む方式。
  accept 系 widget を個別に包むより漏れが無く、**非同期で予測が届く経路**
  （`zle -F` → `autosuggest-suggest` → `zle -R`）も同じ関門を通る
  （`zle-line-pre-redraw` フックはこの非同期経路で発火しないため使えない。実測で確認）
- 案内は**コマンドライン 1 本につき 1 回**残り回数を消費し、既定 10 回で自動的に消える。
  「出す / 出さない」は**その行の最初の 1 回で決めて行のあいだ変えない**（残り回数を毎描画で
  見ると、消費した瞬間＝残り 0 になった瞬間に同じ行から案内が消える）。
  残り回数は `<data_dir>/shell-integration/autosuggest-hint`（`off` = 恒久 OFF。
  **不在・壊れた値は既定回数**）に置き、zsh 側が減らして書き戻す。
  文言は tako が `autosuggest-hint-text`（1 行目 = Tab 確定あり / 2 行目 = なし）へ言語別に書く
  （i18n は Rust 側に閉じ、`tako lang` の切替がそのまま効く）
- **Tab 確定はゴースト表示中かつカーソルが行末のときだけ**。それ以外は
  `^I` に元々割り当てられていた widget へ委譲する（`.zshrc` の後に `bindkey '^I'` で捕まえるので、
  fzf-tab 等でユーザーが張り替えていればそれを尊重する）。補完メニュー内の Tab は
  `menuselect` キーマップなので触っていない = 巡回は不変。自前 widget は
  `ZSH_AUTOSUGGEST_IGNORE_WIDGETS` に入れる（包まれると `POSTDISPLAY` が空で渡り判定できない）
- **ゴーストが出ているが補完したいときの逃げ道**（この順で軽い）:
  ① カーソルを 1 つ左へ動かす（行末でなくなるので Tab は補完に戻る）
  ② 設定 / `tako autosuggest tab off` / MCP `tako_autosuggest {"tab": false}` で恒久的に OFF
  ③ そのシェルだけなら `TAKO_NO_AUTOSUGGEST_TAB=1`
- ON/OFF は `settings.json` の `autosuggest_hint` / `autosuggest_tab`（どちらも既定 ON）+
  dispatch `Autosuggest { hint, tab }`（`tako autosuggest hint|tab [on|off]` /
  MCP `tako_autosuggest` の `hint` / `tab`）+ env `TAKO_AUTOSUGGEST_HINT` / `TAKO_AUTOSUGGEST_TAB`。
  シェル側の逃げ道は `TAKO_NO_AUTOSUGGEST_HINT=1` / `TAKO_NO_AUTOSUGGEST_TAB=1`。
  `hint` を ON へ戻すと残り回数が既定へ戻る（= もう一度チュートリアルを見せる）
- 案内が行を溢れさせるくらいなら出さない（`$#BUFFER + $#POSTDISPLAY + 案内 + 余白 <= COLUMNS`）

実装メモ（FR-2.4.6。2026-07-27 / Issue #601 完成）:

- 解決するのは配布形態の非対称性。zip / .app 配布では CLI の実体が
  `tako.app/Contents/MacOS/tako` にしか無く、docs のクイックスタートが案内する
  「tako 内のターミナルで `tako setup`」が `command not found` になりうる
- **注入経路は FR-2.4.1 / FR-2.4.5 と同じシェル統合一本**。rc ファイル（`~/.zshrc` 等）は
  書き換えない。zsh は ZDOTDIR の `.zshenv` が直後に ZDOTDIR を戻すので、走るのは
  **各ペインの先頭シェルだけ**（孫シェル・tako の外のシェルは不変）
- **足すのは PATH の末尾、しかも `tako` が他に見つからないときだけ**。ユーザーが自分で
  通した実体（Homebrew cask の `/opt/homebrew/bin/tako` 等）の解決順は変えない
- **判定はシェル側で、rc を読み終えた後に行う**。tako-app のプロセス環境は Dock 起動だと
  PATH が最小構成（`/usr/bin:/bin:…`）なので、アプリ側で「PATH に tako があるか」を
  判定すると必ず誤る。したがって spawn 時の env で PATH を足す方式は採らない:
  - zsh 対話: 最初の `precmd`（= `.zshrc` / `.zprofile` の後）。一度足したらフックを外す
  - zsh の**プロンプトが出ないシェル**（`$SHELL -l -i -c <コマンド>` = コマンドペイン・
    エージェントペイン）: `.zshenv`。この時点の PATH で判定すると取りこぼしうるが、
    足すのは末尾なので解決順は変わらない。**判定材料は「対話か」ではなく `-c` か**
    （zsh は `-c` のときだけ `ZSH_EXECUTION_STRING` を設定する）: #1031 でこのラッパーが
    `-i` 付き（= 対話）になったので、`! -o interactive` だけを見ると条件が外れて
    コマンドペインから `tako` が消える
  - bash: `PROMPT_COMMAND` から source される最初のプロンプト（= `~/.bashrc` の後）
  - fish: 最初の `fish_prompt`（= `config.fish` の後。`vendor_conf.d` はその前に読まれる）
- 追加するディレクトリは **`current_exe()` の隣**を起動時に解決した値（直書きしない）。
  .app なら `Contents/MacOS`、dev ビルドなら `target/debug`。隣に CLI が無ければ何もしない。
  値は `<data_dir>/shell-integration/cli-dir`（空 = 注入しない）に置き、シェルが最初の
  プロンプトで読む。アプリを移動しても次に開くシェルからは正しい実体が引ける
- 逃げ道は `TAKO_NO_PATH_INJECTION=1`（3 シェル共通）。設定 UI は持たない
  （既定動作を賢くする方向で、新しいオプションは足さない。AGENTS.md #322）
- 診断は `tako_check_health` の `injected_cli_dir`。PATH に tako が無くても注入が効いて
  いれば `error` ではなく `info`（tako の中では使えるため）で、外部ターミナル向けの案内を出す
- 既知の限界: **非対話の入口を持つのは zsh だけ**（`.zshenv`）。`SHELL` が bash / fish の
  ユーザーは、コマンドペイン・エージェントペイン（`$SHELL -l -c`）に注入が届かない。
  bash の非対話フックは `BASH_ENV` しかなく、これは tako と無関係なスクリプト実行まで
  巻き込むので採らない（対話シェルからは従来どおり届く）
- **外部シェル向けの PATH 設置（symlink 等）は FR-2.14.5**（#601 案 2 → **#1502 で実装**）。
  tako 内側（ここ）は env 注入、外側は `$HOME/.local/bin/tako` の symlink + profile の
  マーカーブロックで、**置き場所の解決だけ `tako_core::shell_integration::cli_dir()` を共有する**

### FR-2.5 AI レイアウト操作セット（Layer 1 / Layer 2 共通）

> 方針: **AI での開発がやりやすく、かつ AI が開発をアシストできる**こと（設計原則 5「AI フルコントロール」の具体化）。
> アプリ内エージェントが人間と同等に画面構成（分割・削除・フォーカス・サイズ・レイアウト・タブ・表示内容）を
> 操作できる操作セットを、CLI（Layer 1）と MCP（Layer 2）の両方に **1:1 対応**で持たせる。
> 読み取り系（現在のレイアウト・各ペインの状態の取得）も操作セットの一部として設計する
> —— AI は現状を読めなければ適切なレイアウト操作を判断できない。
>
> 想定ユースケース:
> - 作業が終わった分割ターミナルを、起動したエージェント自身が片付ける（自分のペインの削除を含む）
> - 成果物をユーザーに見せるとき、プレビューペインを大きく開く・不要ペインを畳むなど
>   レイアウトを整えた状態で提示する

| ID | 要件 | 優先度 |
|---|---|---|
| FR-2.5.1 | 読み取り: タブ / ペインの**ツリー構造・ジオメトリ**（位置・サイズ・分割比率）・フォーカス位置を構造化データ（JSON）で取得できる | M |
| FR-2.5.2 | 読み取り: 個別ペインの状態（タイトル・role・origin・実行中/終了・cwd・画面内容）を取得できる | M |
| FR-2.5.3 | 操作: ペイン分割（方向・比率指定、ネスト可。FR-2.2.1 と同一操作） | M |
| FR-2.5.4 | 操作: ペイン削除（**呼び出し元自身の削除 = 自己片付け**を含む。削除後のツリー再構成・フォーカス引き継ぎは自動）。master が別の master を閉じる close の守りは FR-2.24.19（#2330） | M |
| FR-2.5.5 | 操作: フォーカス移動（ペイン ID 指定・方向指定の両方） | M |
| FR-2.5.6 | 操作: サイズ調整（分割比率の直接指定と、対象ペインの相対的な拡大・縮小） | S |
| FR-2.5.7 | 操作: レイアウト一括調整プリセット（均等化、対象ペインの最大化 / 復帰 など） | S |
| FR-2.5.8 | 操作: ペインの位置入れ替え・ツリー上の移動 | C |
| FR-2.5.9 | 安全性: 破壊的操作（削除・レイアウト一括変更）は許可ポリシー（FR-2.3.5）に従い、操作スコープのデフォルトは同タブ内（FR-2.3.3） | M |
| FR-2.5.10 | 操作: タブの作成・切替・クローズ、ペインの別タブへの移動（振り分け）。同タブスコープ（FR-2.3.3）を越えるため明示指定が必要 | S |
| FR-2.5.11 | 操作: ファイルを開いてプレビューペインで表示する（FR-3.2 系操作の AI 実行。「探して開いて見せて」のコア操作。✅ 2026-06-13: dispatch `OpenFile` + CLI `tako open` + MCP `tako_open_file` = 計 21 ツール） | M |
| FR-2.5.12 | 操作: Web ビューペイン（FR-3.8）で任意の URL を開いてユーザーに見せる（✅ 2026-07-13、#155。CLI `tako web open` + MCP `tako_web` action=open） | S |
| FR-2.5.13 | 操作: スクロールバック表示の取得・移動（絶対位置 / 相対行数。0 = 最下部）。**#159 でピクセル単位のスムーススクロールへ刷新**: 表示位置は行小数（直接ペイン = `display_offset - fract`、`TerminalSession::scroll_pixels`）で保持し、描画は行スタック全体をサブラインシフト（Chrome / iTerm2 相当のリニア描画）。バックエンドペイン（FR-5）・ネスト tmux（ペイン内 attach）は **tmux 履歴のローカルミラー**（`tako-core::scroll_mirror`。スクロール開始時に `capture-pane -e` でチャンク取得 → 以降の描画は完全ローカル）で、copy-mode には入らない（旧 copy-mode 駆動の ① 行単位 ② tmux 往復レイテンシ ③ キー飲まれ、を構造的に解消。2026-07-13）。マウス要求アプリ（vim / claude 等）へのレポートは **tmux サーバーへの直接注入（`send-keys -H`）+ 転送レート制限**で届ける（#167。外側 PTY 書き込みの断片化で `4;45;18M` 等が入力欄へ平文混入する事故の根治。`architecture.md`「マウスレポート転送」節）。CLI / MCP の `Scroll` は `ControlHost::backend_scroll_view` で UI と同一経路。**TmuxOpen ビューペイン（`tako tmux open` の再アタッチ・`tako-view-*` ラッパー経由）も同じミラー経路に乗る**（#181: 外側 alacritty は alt screen（履歴なし）のため、分岐が `backend_sessions` のみだと直接ペイン扱いに落ちてスクロール不能だった。判定は `mirror_scroll_pane`（backend ∪ tmux_view）に統一、実体は wrapper（無ければ元セッション）@ socket を `ScrollTarget::Nested` として**ビュー先優先**で解決（persist ON では外側 PTY も backend ラップされるため backend 優先だと外側 = history 0 へ誤解決する）。persist 復元で戻ったビューペインは `tmux_view_panes` に載らないが、ネスト検出候補に backend socket 自身を含めることで tty 突き合わせで実体へ解決する。既知制約: カスタム `-L` 外部サーバーのビューは復元後にネスト検出できずスクロール不可（ビューを開き直せば回復）。2026-07-13）。UI はペイン右端のオーバーレイスクロールバーで、iTerm2 流に**スクロール中だけ表示 → 約 1 秒でフェードアウト**、ホバー / ドラッグ中は表示維持 + サム強調。スクロール中のキー入力は最下部へ戻してから流す（iTerm2 流）。既知制約: ミラー表示中（バックエンドの過去閲覧中）の選択・cmd+クリックは無効（視覚位置とライブ viewport が不一致のため。最下部へ戻れば可）。**ドラッグ選択のオートスクロール**（#310）: 上下端 40px（`DRAG_SCROLL_MARGIN`）でマウスを止めると表示を送りながら選択の端を画面端へ伸ばす。**送れる履歴が無い・既に最上端にいるときは上へ送らず、選択の端を動かさない**（#2025。旧は毎ティック端を (0, 0) へ寄せ、履歴の無いペインで選択が左上まで縮んでいた）。**下端も対称に、既に最下部（`display_offset == 0`）にいるときは下へ送らず、選択の端を動かさない**（#2153。旧は毎ティック端を画面の右下へ寄せ、直接ペインの普段の状態 = 最下部で最後の行を途中まで選べなかった。履歴の途中からは #310 のまま最下部まで送る）。判定は `drag_scroll_step` の 1 本（番犬 `issue2025_tick_が判定を通す_watchdog` / `issue2153_tick_が下端で寄せない_watchdog`）。器のペインと alt screen は外側の端末に履歴が無いのでオートスクロール自体が立たない（実測）。A/B は `TAKO_2025_LEGACY=1`（上下とも #310 の旧挙動）/ `TAKO_2153_LEGACY=1`（下へ送る側だけ旧挙動）、実クリックは visual-test `drag-autoscroll`（`scripts/test-drag-autoscroll-2025.sh`） | S |
| FR-2.5.14 | **明示コマンドのペインはユーザーの対話環境で起き、失敗しても消えない**（#1031）。①起動シェルは**素のペインと同じ**（`platform::shell::login_shell_command` = `$SHELL -l -i -c`。素のペインは `$SHELL -l` を tty で起こすので zsh から見ると対話ログイン = `.zprofile` も `.zshrc` も読まれる。`-i` が無いと `.zshrc` だけ読まれず、nodebrew / fnm / Homebrew の PATH を `.zshrc` で通しているユーザーには「素のペインでは `npm` が引けるのに実行ペインでは command not found」になる）。②`tako split --command` のコマンドが**非 0 で終わったときだけ**、終了コードを伝えて入力待ちで止める（`hold_on_failure_command`）。伝え方は実行ペイン（FR-3.18.1）と**同じ 1 実装**（#1778。`platform::shell::posix_exit_report` / `powershell_exit_report` と `*_exit_hint_line`）: 終了コードは側路 `<data_dir>/run-exit/<pane>.code` へ書き（Split の腕が `prepare_exit_file` で用意して**ペインに控える** = `Pane::exit_file`。実行ペインのメタ `InteractiveMeta` にはしないので、バッジ・`list` の `run`・auto_close の対象にはならない）、画面には「`[tako] 終了コード N / Enter でこのペインを閉じます`」だけを出す。#1778 以前は画面へ `__TAKO_EXIT=<code>` と別文言の案内を出していた。止まっているあいだの終了コードは `RunInteractiveStatus`（`tako run-interactive-status <pane>` / MCP `tako_run_interactive_status`）が `run_pane_exit_code` の 1 実装で読む。側路を用意できなかったときだけ画面へマーカーを出す（退避路）。成功時は従来どおり即 close（側路にも書かない）。閉じたペインの出力は FR-5.13 のペインログ（`tako logs show <pane>`）で後から読める。A/B は `TAKO_1031_LEGACY=1` | M |
| FR-2.5.15 | **マウス報告を要求する TUI（全画面の claude・vim・htop 等）へ左クリック / ドラッグを渡す**（#1961。既定 ON = Issue のコメントの判断）。#1961 前はホイールしか渡さず、mod のボタン・Markdown のリンク・「Jump to bottom (click)」がクリックで押せなかった。①判断と符号化は `tako_core::mouse_forward` の 1 実装（`route` → `decide` → `click_report_bytes`）。SGR（1006）/ UTF-8（1005）/ 素の X10 を内側アプリの要求どおりに組み、押す・離す・押したままの移動（1002 / 1003 を要求したアプリだけ）を渡す。X10 で表せない座標（223 桁超え）は丸めずに渡さない。②**tako が取る場合**（理由の語は `tako mouse` の `last.reason`）: Option（⌥）/ Shift + ドラッグ（`bypass_modifier`）= tako の選択・cmd（Windows は Ctrl）+ クリック = リンク・ペインのフォーカスを移すクリック（`focus_change`）とウィンドウを前面へ出すクリック（`window_activation`）= 作業ペインを選ぶつもりのクリックで TUI のボタンを押さない・履歴を遡って見ているあいだ（`scrolled_back`）・設定 off（`disabled`）・マウスを要求しないアプリ（`not_requested` = シェルでは挙動が変わらない）・器の copy mode（`copy_mode`）。③届け方: 直接ペインは端末モードから同期に分かるので PTY へ。器のペインは外側の端末モードが器の `mouse on` で常に要求ありに見える（#1252）ので、器のペイン側のフラグを background で問い合わせ（答えまでのイベントは溜めて、答えに合わせて渡すか tako の選択として再生する）、`send-keys -H` で注入する（#167 と同じ = 断片化が構造的に起きない）。ペインごとに 1 本へ直列化し、送信中の移動は最新 1 つへ畳む。移動は #167 と同じトークンバケット、押す・離すは捨てない。窓の外で離した（MouseUp が届かない）ときも TUI へ離すを渡す。④設定は `settings.json` の `mouse_click_forward`（serde default = 旧ファイルはそのまま読める）。CLI `tako mouse [on\|off]` / MCP `tako_scrollback` の `mouse_forward` / 設定画面「一般」のトグルが同じ `Request::Scrollback` を通り、次のクリックから効く。応答の `mouse_forward` に累計（press / drag / release）と最後の行き先。⑤AI からのクリック: CLI `tako click --pane N --row R --col C` / MCP `tako_send_input` の `click: {row, col}` = 押す + 離す（`Request::PaneClick`。同じ判断・同じバイト列。器のペインは `prepare_offload` が background へ回す）。要求していないアプリへは渡さずエラー（制御列を打たない）。**ドラッグそのもの（ポインタのジェスチャ）は CLI へ写さない**（FR-2.23.10 と同じ理由）。A/B は `TAKO_1961_LEGACY=1`（#1961 前 = 常に tako の選択）。e2e: `tmux_backend::クリックのtmux直接注入は断片を混ぜず押す離すを欠かさない` / `マウスを要求しない器のペインへはクリックを渡さない`、実クリック: visual-test `click-forward`（直接 / 器）・`click-forward-claude`（実物の claude で Jump to bottom・mod の帯の compact / split right・AI のコマンドカード（#1963）の Run / 閉じる）= `scripts/test-click-forward-1961.sh`。Claude Code（2.1.294）は**描かれて 300 ms 以内のクリックを迷いクリックとして捨てる**（`mount_settle`）ので、出た瞬間のボタンを押す自動化は 1 秒置く。既知制約: ネストした tmux（ペイン内で attach）と取り込みビューの座標は器のペインの左上を原点とみなす（ホイールと同じ）。Windows（psmux）は未実測 | S |

Phase 3 設計時の指針:

- MCP ツール名は `tako_` 接頭辞 + snake_case（`conventions.md`）。CLI サブコマンドと操作セマンティクスを共有する
- 操作のセマンティクスは `tako-core` の PaneTree ドメイン API（split / close / focus / resize / equalize / layout 取得）と 1:1 対応させ、実装を共有する
- FR-2.2（CLI）/ FR-2.3（MCP）の既存要件は本カタログの部分集合。レイアウト操作の完全なカタログはこの節を正とする
- レイアウトに限らず、テーマ・外観などの**設定操作も同じ操作セットの対象**（カタログは FR-4 を正とする）

### FR-2.6 AI 誘導・注釈レイヤ

> AI が「操作する」だけでなく「**ユーザーに提示する・誘導する**」ための手段。
> 設計原則 5 の「過保護なくらい分かりやすく」を画面上で実現する。
> 例: 「このエラーログのここを見て」とペイン上の該当行をハイライトする、
> 成果物のプレビューを開いた上で注目してほしい箇所を指し示す。
> 実現方式の検討は `architecture.md`、実装は後段フェーズ（`roadmap.md`）。

| ID | 要件 | 優先度 |
|---|---|---|
| FR-2.6.1 | ペイン上に描画オーバーレイで注目箇所をハイライト・指し示しできる（矩形/行ハイライト、ポインタ、短い注釈テキスト） | S |
| FR-2.6.2 | オーバーレイの表示・消去・対象指定（ペイン + 領域）を MCP / CLI から操作できる | S |
| FR-2.6.3 | 注釈はユーザーの操作を妨げない（入力はペインへ透過し、ユーザー操作または明示消去・タイムアウトで消える） | M |

### FR-2.7 AI 成果物プレゼンテーション

> ビジョン（`concept.md`）の中核体験。単なる show 系ツールの羅列ではなく、
> 「**AI とユーザーのスムーズな開発対話を画面が支える**」ことが目的。
> AI が**受動的に操作できる**（FR-2.5）だけでなく、**能動的にユーザーへ見せにくる**ことを
> 標準にし、「人間は AI との会話のみで開発が完結する」を画面側から成立させる。
> 注釈ハイライト（FR-2.6）と組み合わせて「どこを見てほしいか」まで指し示す。
>
> 代表ユースケース（MCP ツール説明文に埋め込む行動規範はこの 3 つに 1:1 対応させる）:
>
> - **(a) レビュー依頼** — AI が作業を終えてユーザーにレビューを求めるとき、成果物
>   （diff・ファイル・実行結果）を自らプレビューペインで開いて提示し、その上でレビューをもらう。
>   規範: 「**レビューを求めるときは見せろ**」
> - **(b) 理解の同期** — AI がユーザーにちゃんと読んでほしい md（設計方針・README・注意事項）が
>   あるとき、プレビューで表示して理解を求める。規範: 「**読んでほしければ開け**」
> - **(c) 複数案の比較検討** — UI / デザイン等の方針相談で、AI が何パターンか例を作り、
>   プレビューを**複数ペインで並べて同時に見せる**（A/B/C 案の並列比較表示も操作セットの想定に
>   含める）。ユーザーは「今見せてくれたデザインいいね」→「じゃあそれで行きましょう」と
>   会話だけで意思決定が進む。規範: 「**方針相談は例を作って並べろ**」

| ID | 要件 | 優先度 |
|---|---|---|
| FR-2.7.1 | `show_file`: 指定ファイルをプレビューペインで開いてユーザーに見せる（FR-2.5.11 / FR-3.2 の提示用途） | M |
| FR-2.7.2 | `show_diff`: git diff・コミット差分を diff ビューア（FR-3.9）で開いて見せる | M |
| FR-2.7.3 | `show_url`: URL を Web ビューペイン（FR-3.8）で開いて見せる（FR-2.5.12 と同一操作。✅ 2026-07-13、#155。専用ツール名ではなく `tako_web` action=open が担う） | S |
| FR-2.7.4 | 提示系操作は注釈ハイライト（FR-2.6）と組み合わせられる（開いた上で該当行を指し示す） | S |
| FR-2.7.5 | tako の MCP ツール説明文に提示の行動規範（タスク完了・レビュー依頼時は見せろ / 読んでほしければ開け / 方針相談は例を作って並べろ）を埋め込み、エージェントが自然に提示行動を取るよう誘導する（プロンプト工学もプロダクトの一部。上記ユースケース (a)〜(c) に対応） | M |
| FR-2.7.6 | 複数案の並列比較提示: 複数のプレビューペインを並べて同時に表示するレイアウト操作（ユースケース (c)。FR-2.5 のレイアウト操作セットで実現） | S |

### FR-2.8 ワンクリックフィードバック（提示の双方向化）

> AI 成果物プレゼンテーション（FR-2.7）の逆方向。AI が見せにきたものへ、ユーザーが
> **画面上でそのまま応答**でき、会話ループが画面上で閉じる。
> GitHub PR のインラインレビューのローカル版イメージ。

| ID | 要件 | 優先度 |
|---|---|---|
| FR-2.8.1 | 提示された diff・プレビュー上で範囲選択してコメントできる（「ここ直して」） | S |
| FR-2.8.2 | 「OK」「やり直し」等のワンクリック応答ボタンを提示に添えられる | S |
| FR-2.8.3 | フィードバックは MCP 経由で該当エージェントの入力に届く | S |

### FR-2.9 どこでも AI 呼び出し（cmd+K）

| ID | 要件 | 優先度 |
|---|---|---|
| FR-2.9.1 | アプリ内のどこからでも AI に話しかけられる入力 UI（cmd+K） | S |
| FR-2.9.2 | 現在のペイン内容・選択テキスト・cwd を文脈として自動添付する | S |

### FR-2.10 待ちエージェント集約センター

> 「1 グループ = 1 タブ」（FR-2.1.2）のタブ横断版。FR-2.1.4（タブ単位の状態把握）を
> アプリ全体へ拡張する。

| ID | 要件 | 優先度 |
|---|---|---|
| FR-2.10.1 | 全タブのエージェント状態（入力待ち / 完了 / 質問あり）を 1 か所に集約表示する | S |
| FR-2.10.2 | 集約表示のエントリをクリックすると該当ペインへジャンプする | S |

表示場所は 2026-06-12 に固定タブ → **右サイドバー情報パネルの agents ビュー**へ変更
（FR-2.16。固定タブ 0 個方針）。同日のパネル UI 刷新で agents ビューは
**統合 tmux ビュー**（FR-2.16.6。タブ枠ごとの全ペイン表示 + クリックでジャンプ）へ統合した。

### FR-2.11 AI 活動タイムライン

> 人間がコードを見なくなる世界（`concept.md` のビジョン）での**監査可能性・信頼の土台**。
> v0.2 以降の後段フェーズで実装する。

| ID | 要件 | 優先度 |
|---|---|---|
| FR-2.11.1 | エージェントの実行コマンド・変更ファイル・コミットを時系列一覧するペイン | C |

### FR-2.12 タブ・ペインの AI 自動リネーム

> タブ名・ペイン名を、ペイン内の実際の作業内容に追従する短い要約タイトルへ
> 自動で更新する仕組み（2026-06-12 ユーザー要望・同日完成）。
> **tako 本体に LLM を埋め込まない**: tako は素材と rename API を公開し、名前の生成は
> 外部の claude CLI（子プロセス）に委ねる。tako 側に判断ロジックは置かず
> プロンプト 1 本に閉じる（実行体は方式 1 = tako 常駐。ユーザー承認済み）。

| ID | 要件 | 優先度 |
|---|---|---|
| FR-2.12.1 | tako は命名の素材（ペイン内容・cwd・実行コマンド・状態・現タイトル）と rename API（ペイン title は既存、タブは `tako tab rename` / `tako_rename_tab`）を MCP / CLI で公開する | S |
| FR-2.12.2 | 変化検知（コマンド実行・cwd 変化・出力の節目）はデバウンスし、安価・高速なモデル（例: `claude -p --model claude-haiku-4-5-20251001`）で短い要約タイトルを生成する | S |
| FR-2.12.3 | ユーザーの手動リネームは自動より優先する（手動設定されたタブ / ペインは自動で上書きしない） | M |
| FR-2.12.4 | 自動リネーム全体を設定で OFF にできる | M |
| FR-2.12.5 | AI が使えない環境では OSC タイトル・cwd・実行コマンドからのヒューリスティック命名へフォールバックする | S |
| FR-2.12.6 | 命名済みタブの再命名には最小間隔（5 分）を設ける。まだ名前が無いタブの初回命名は待たせない | M |
| FR-2.12.7 | 一時的な失敗（打ち間違い・`command not found`・一度きりの非ゼロ終了）を命名の材料にしない | M |
| FR-2.12.8 | 生成する名前の言語を UI 言語に固定し、出力の字種を検査して合わないものは採用しない | M |
| FR-2.12.9 | 自動命名の直後だけタブに印を出し、ワンクリックでその名前を手動固定できる（CLI / MCP からも同じ操作ができる） | M |
| FR-2.12.10 | **claude CLI の探索は抽象境界 B16（`platform::exe::find`）を通す**。ログインシェルへ `command -v` を尋ねる形は Windows で必ず失敗し、`Option` へ化けて握り潰されるため AI 命名が永久に無効になる（ログにも UI にも出ない。#722）。失敗が全部「黙ってヒューリスティックへ落ちる」形で出る機能なので、`TAKO_AUTORENAME_DIAG=1` で**解決結果・終了コード・所要時間・出力バイト数のメタ情報だけ**を stderr へ出せるようにする（プロンプト・応答・画面末尾は出さない = AGENTS.md の絶対ルール。✅ 2026-08-27、#722） | M |
| FR-2.12.11 | 命名の呼び出しは **MCP サーバーを 1 つも使わない**（`claude -p` へ `--mcp-config` を伴わない `--strict-mcp-config` を渡す）。タブ名 1 個のための使い捨て呼び出しに全 MCP サーバーの起動・接続は過剰で、遅いうえに**命名ヘルパーへ tako のペイン操作ツール一式が生える**（用途に対して権限が過剰）。未知のフラグを拒否する古い CLI のために、**非ゼロ終了のときだけ**フラグ無しで 1 回再試行し、その再試行が通ったときにだけ「このフラグは使えない」とプロセス内で覚える（＝ AI 命名が黙って無効化されない。#722 の再来を防ぐ）。A/B は `TAKO_758_LEGACY=1`（✅ 2026-09-11、#758） | M |
| FR-2.12.12 | **ヒューリスティック命名は「シェル自身 / 端末多重化ツールの実行ファイル」を素材にしない**。Windows のコンソールタイトルはシェル / psmux 自身のフルパス（`C:\Program Files\PowerShell\7\pwsh.exe` / WinGet 配下の `...\tmux.exe`）なので、OSC タイトルを最優先で採ると全タブが同じ名前（16 文字で切った `C:\Program Files` / `C:\Users\<user>\A`）になり、タブ名が識別に使えない。該当するタイトルは捨てて cwd の末尾ディレクトリ名へ落とす。cwd の区切りは**動いている OS ではなく文字列の形**で見る（`Path::file_name` は Unix で `\` を区切りにしないので、Windows 形の素材が丸ごと 1 要素になる）（✅ 2026-09-22、#760） | M |
| FR-2.12.13 | **素材が凍ったままのタブは時間で命名をやり直す**。指紋は cwd / OSC タイトル / 実行状態でできているが、シェル統合（OSC 7 / 133）が無いプラットフォーム（Windows。#525）ではその 3 つが起動時の値のまま一度も変わらない（cwd は `cd` しても追従せず、状態は常に `unknown`）。指紋の変化だけを発火条件にすると命名は**ペインを開いた直後の 1 回**（＝画面がまだ空で素材がほぼ無い時点）で終わるので、前回の発火から一定時間が経ったら指紋が同じでもやり直す。**回数は表で区切り、使い切ったら素材が動くまで静まる**（FR-2.12.6 の趣旨を壊さない）（✅ 2026-09-22、#760） | M |
| FR-2.12.14 | **master のいるタブの名前はプロジェクト名か作業の主題**（`tako` / `my-site` / 1 つのリポジトリで複数の科目を扱うなら `アジャイル`）。役割名・プロファイル名（`master` / `master-takodev`）は使わない。① 起動（`tako master` / スマホの「+ master」/ 人タスクの返答で立てる master）は**管轄プロジェクトの名前を自動名で**付ける（汎用 master はタブ名に触らず、採用 `adopt` の時点でそのプロジェクト名）② 以後は master 自身が `tako_rename_tab(source: "auto")` で主題へ付け直す（system prompt の項目 10）③ アプリの自動命名（FR-2.12.2）は master のいるタブの名前を作らない（画面の素材からはプロジェクトも科目も分からず、master の付けた主題を潰すため）④ 自動名なので**ユーザーが手で付けた名前は上書きしない**（FR-2.12.3）⑤ 引き継ぎの後任は同じタブに立つのでタブ名に触らない。#2120 以前は起動経路が `master` / `master-<名前>` を**手動名**で付けており、master の自動名が全部弾かれて全タブが `master` で固定されていた。旧版が付けたこの手動名は**復元の時点で**外す（手動名 + 役割名の形 + master のペインがいる の 3 つが揃うタブだけ。ユーザーが付けた別の名前は残る）。layout.json は動いている GUI が持って書き戻すので、#916 のファイル移行ではなく読み込み側で直す（GUI の再起動・`tako recover`・退避タブが同じ 1 か所を通る）。正本は `tako_core::master_tab`（初期名・役割名の判定・アプリが命名してよいか）と `tako_control::layout::release_legacy_master_title`。A/B は `TAKO_2120_LEGACY=1`（✅ 2026-10-15、#2120） | M |
| FR-2.12.15 | **solo のいるタブの名前も役割名にしない**（FR-2.12.14 と同じ形）。`tako solo` は起動で**solo が作業するフォルダのリポジトリ名を自動名で**付ける（リポジトリの奥で立てても `.git` の段のフォルダ名。リポジトリの外ならそのフォルダ名。HOME・ファイルシステムのルートでは**タブ名に触らない**。辿る範囲は `tako_core::project_root` の境界 = HOME を天井にして git プロセスを起こさない）。solo には管轄プロジェクトが無い（必要なプロジェクトへ自分で `cd` する）のでプロファイルの `projects` は見ない。以後は solo 自身が `tako_rename_tab(source: "auto")` で作業内容へ付け直す（solo の system prompt の項目 8。prompt は変えていない）。`--tab` / 呼び出し元ペインが解けないときの新しいタブは**そのフォルダで開く**（#2127 前はアプリの起動フォルダで開いており、名前と solo の作業場所が食い違うため）。アプリの自動命名は solo のいるタブの名前も作らない（role は `solo[:<名前>]` と再起動で tmux の env から戻した `orchestrator-solo[:<名前>]` の両方を solo とみなす = `master_tab::is_solo_role`）。自動名なので**ユーザーが手で付けた名前は上書きしない**。#2127 以前は `solo` / `solo-<名前>` を**手動名**で付けており、solo の自動名が全部弾かれていた。旧版が付けたこの手動名は**復元の時点で**外す（手動名 + **そのタブの solo のペインの role から組み立てた旧い名前と一致**（`solo:fast` のペインなら `solo-fast` だけ。master の判定より狭い） + solo のペインがいる の 3 つが揃うタブだけ）。外したあとは layout が持つ solo のペインの cwd からリポジトリ名を自動名で付ける（引けなければ名前は残して手動だけ外す）。正本は `tako_core::master_tab`（`solo_initial_title` / `is_legacy_solo_title` / `app_may_name_tab`）・`tako_control::orchestrator::master_launch::solo_tab_naming`・`tako_control::layout::release_legacy_solo_title`。A/B は FR-2.12.14 と同じ `TAKO_2120_LEGACY=1`（✅ 2026-10-16、#2127） | M |

実装メモ（2026-06-12 実装完了。実行体は**方式 1 = tako 常駐**をユーザーが承認。
FR-2.12.6〜9 は 2026-07-27 に #552 で追加）:

- 実行体: tako-app 内の検知ループ（`autorename.rs`、2 秒ポーリング）。素材指紋
  （cwd / OSC タイトル / 実行状態 / 手動フラグ。**画面末尾は含めない** = 実行中に
  静穏にならないため）の変化 → 静穏 4 秒（デバウンス）+ タブ毎の最小間隔で
  発火し、`claude -p --model claude-haiku-4-5-20251001` を子プロセスで 1 回叩く
  （stdin にプロンプト、タイムアウト 120 秒で kill。#722 の実測で 30 秒では足りず広げた）。
  タブ単位の 1 呼び出しで
  `{"tab":"...","panes":{"<id>":"..."}}` を生成させ、応答パース → `set_title_auto` で反映
- 手動優先（FR-2.12.3）: `TitleSource`（default / auto / manual）を Pane / Tab が持ち、
  `set_title_auto` は manual を上書きしない。明示リネーム（CLI / MCP / UI）= manual、
  空文字指定で manual 解除 → 自動再開。`list` に `title_source` を公開
- タブ表示名: title_source が default のタブのみ従来の「フォーカスペインの OSC タイトル」
  フォールバックを使い、auto / manual はタブ名そのものを表示する
- OFF 設定（FR-2.12.4）: `<data_dir>/settings.json` の `auto_rename` + dispatch
  `AutoRename`（`tako autorename on|off` / MCP `tako_auto_rename`）で永続トグル。
  env `TAKO_AUTO_RENAME=0|false|off` は設定ファイルより優先して無効化
- フォールバック（FR-2.12.5）: claude CLI 不在（ログインシェル経由 `command -v claude`
  で解決。`TAKO_CLAUDE_BIN` で差し替え可）・失敗・タイムアウト時は OSC タイトル >
  cwd 末尾ディレクトリ名のヒューリスティック命名
- セルフテスト中（`TAKO_SELF_TEST`）は検知ループ無効 + claude 不使用 + 設定を永続化しない
- MCP を使わせない（FR-2.12.11・#758）: 起動は `spawn_claude(bin, prompt, strict)` の 1 本で、
  フラグを付けるかどうかは `run_learning_strict` が `StrictMcp`（Unknown / Supported /
  Unsupported）としてプロセス内に 1 つだけ持つ。**再試行の引き金は非ゼロ終了だけ**で、
  打ち切り・起動失敗では再試行しない（プロセスが立った時点でフラグは通っているので、
  もう一度待つと上限のぶん二重に待たせるだけ）。学習するのも「フラグ無しの再試行が通った」
  ときだけで、リミット・認証切れのような一時的な非ゼロ終了で速い経路を捨てない。
  macOS の実測（claude 2.1.258 / MCP サーバー 9 本 / 順序を交互にした 10 ラウンド）は
  命名プロンプトの p50 18.4s → 16.9s（対応差の中央値 -3.8s・10 回中 9 回速い）、
  固定費だけを切り出した最小プロンプトで 5.8s → 3.7s、**起動する子孫プロセスのピークが
  17 → 2**。`CLAUDE_TIMEOUT` は**縮めない**（フラグ付きでも最大 43.6s・隔離 GUI の
  実命名で 38.7s の回があり、遅い側の裾はフラグでは短くならない）
- **上限が上限として効いていなかった**（#758 の上限テストで判明）: 打ち切りは
  `child.kill()` のあと stdout の読み出しスレッドを `join` で待っていたが、パイプが閉じるのは
  **握っている全員**が消えたときなので、claude の孫（= MCP サーバー）が残っていると
  上限を過ぎても返らない（実測: 孫が 30 秒眠る偽 CLI で、上限 0.6 秒の打ち切りが 30.3 秒）。
  読み出しは `mpsc` の期限付き受信にし、打ち切りでは 1 秒も待たず、正常終了でも
  `READ_GRACE`（10 秒）で諦めてヒューリスティックへ落とす。
  MCP を使わせれば孫はそもそも居ないので、FR-2.12.11 はこの穴自体も塞ぐ。
  **非ゼロ終了は終了コードだけで決め、出力が閉じるのを待たない**（#1705）。出力は使わないうえ、
  待つとパイプを受け継いだ別のプロセスが握っているあいだ `READ_GRACE` まで止まり、
  「起動できない / 応答なし」の側に化けて FR-2.12.11 の再試行が走らなかった

品質改善の実装メモ（#552。新規ユーザー視点レビューで 10 分の操作中にタブ名が 5 回変化し、
`claude失敗` の外れ命名と簡体字 `开発` の混入が観測された）:

- 最小間隔（FR-2.12.6）: `AutoRenamer::tick` の入力を `TabSignal { tab, fingerprint, named }`
  にし、**命名済み**（`title_source != default`）のタブだけ 5 分間隔（`RENAME_MIN_INTERVAL`）を
  課す。名無しのタブは従来どおり 30 秒（`FIRST_NAME_COOLDOWN`）で再試行する。
  ユーザーの手動リネームはこの経路を通らない（dispatch 直行）ので常に即時
- 一時的失敗の除外（FR-2.12.7）: ① `material_state()` が `Failed` を `idle` に写す
  = **打ち間違い 1 回では素材指紋が変わらず発火しない** ② `trim_tail()` が
  `is_transient_failure()`（command not found / no such file / usage: / 日本語ロケール版 等）に
  当たる行を素材から落とす ③ プロンプトにも「失敗そのものを名前にしない」を明記。
  作業の結果としての失敗（テストの red・`error[E0308]` 等）は材料に残す
- 言語固定（FR-2.12.8）: プロンプトを `i18n::lang()` で日英に出し分け（日本語版は
  「簡体字・繁体字を使わない」を明記）、応答は `sanitize_title()` が検査する。
  日本語 UI では技術語の簡体字（`开` → `開` 等 115 対）を日本語字体へ置換したうえで、
  **日本語の漢字集合（CP932。`autorename/jis_kanji.rs` に埋め込み。生成手順はファイル冒頭）**
  に無い字が残る名前を捨てる。英語 UI は CJK を含む名前を捨てる。捨てた結果 空になれば
  ヒューリスティック命名へ落ちる。既知の限界: 日本語漢字にも在る字だけの中国語（`那个`）は通る
- 固定ワンクリック（FR-2.12.9）: 自動命名の時刻を `TakoApp::auto_title_hints` に記録し、
  `PIN_HINT_TTL`（2 分）だけタブにピン印を出す。クリックで `Tab::pin_title()`
  = 名前を変えずに `title_source` を manual へ（以後 自動が上書きしない）。
  同じ操作を dispatch `TabPinTitle` / `tako tab pin [--off] [--status]` /
  MCP `tako_pin_tab_title` / コマンドパレット「このタブ名を固定」でも提供する

Windows で命名の質が落ちる構造的な理由への手当ての実装メモ（#760。#722 で AI 経路自体は
Windows でも走るようになったあとの実測。2026-09-09 の実機 UI レビューでは
**初回起動の第一印象そのもの**になっていた）:

- 素材の関門（FR-2.12.12）: `autorename.rs` の `shell_exe_material()` が OSC タイトルの
  末尾要素を見て `SHELL_EXE_STEMS`（pwsh / powershell / cmd / conhost / wt / POSIX の
  シェル / tmux / psmux / screen / zellij）に当たれば捨てる。表に足すのは
  「**それ自身がシェル / 器**であって作業内容ではない」ものだけで、ユーザーが打った
  コマンド（`cargo` / `vim`）は入れない。捨てた事実は `TAKO_AUTORENAME_DIAG=1` のときだけ
  「どの語に当たったか」を 1 行出す（**当たった語は tako 側の定数**であって画面の中身では
  ないので診断へ出してよい。ペイン内容を出さないのは AGENTS.md の絶対ルール）。
  macOS は OSC 7 / 133 が入るので素材が実行ファイルパスにならず、影響しない
- 区切りの見方: `last_segment()` が `looks_like_windows_path()`（`C:\...` / `C:/...` /
  UNC `\\server\share`）のときだけ `\` も区切りとして見る。Unix のパスでは `/` しか
  見ないので、ディレクトリ名に `\` を含む実在のパスを壊さない。ルートだけ（`/` / `C:\`）は
  名前にならないので `None`
- 凍った素材のやり直し（FR-2.12.13）: `AutoRenamer::tick` が「同じ指紋のまま
  やり直した回数」を持ち、`STALE_RETRY_DELAYS`（5 分 → 20 分 → 60 分）を使い切るまで
  前回の発火からの経過で再発火する。素材が動いたら回数は仕切り直す。
  **画面末尾そのものは指紋に入れていない**（実行中は毎 tick 変わり静穏にならない）ので、
  これは「凍っているかどうか」を時間で近似した手当てで、画面が変わったときだけ
  やり直す形にはなっていない（#1504 で `tako setup` が `$PROFILE` へ配置するように
  なったので、配置済みの Windows では指紋が動く。FR-2.14.14）

### FR-2.13 tmux セッションの見える化（tmuxview）

> 消し忘れて裏で動き続ける tmux に早く気づいて殺せるようにする（2026-06-12 要望）。
> AI が「消し忘れ tmux の検知 → 片付け提案」を行う将来機能（Layer 3 連携）の土台。

| ID | 要件 | 優先度 |
|---|---|---|
| FR-2.13.1 | ~~タブバーの右端に固定の専用タブ~~ → **右サイドバー情報パネルの統合 tmux ビュー**として表示する（2026-06-12 変更。FR-2.16.6 で旧 tmuxview は削除統合され、tmux セッションは「タブ枠内のペイン行」+「タブ未表示セクション」FR-2.16.8 で表示される） | S |
| FR-2.13.2 | 実行中の全 tmux セッションを一覧表示する: セッション名 / window 一覧（window 名・ペイン）/ 作成日時 / attach 状態 / tako のどのタブ・ペインに表示されているか（tako 外＝外部ターミナル由来の区別を含む対応付け） | S |
| FR-2.13.3 | 一覧から個別に kill できる（kill-session / kill-window。誤爆防止の確認つき） | S |
| FR-2.13.4 | 一覧・対応付け・kill はコマンド層（dispatch）に乗せ、CLI / MCP から同じ操作ができる（開発不変条件） | M |
| FR-2.13.5 | データ取得層（tmux クエリ・対応付け）と表示を分離する（表示方法は変わる前提とユーザーが明言） | M |
| FR-2.13.6 | **window 操作の対象は「そのペインが attach しているセッション」**（#1185）。`tako tmux open` の取り込みペインは「外側 = tako のバックエンドセッション（FR-5）」「内側 = 取り込んだセッション（表示用の `tako-view-*` grouped ラッパー）」の二重ネストなので、`backend_session` だけで解決すると**別セッションの window を切り替えて成功を返す**。解決は `tako_core::tmux::window_target`（ビュー優先 → バックエンド）の 1 実装に集約し、応答は `session`（論理名）/ `target`（実際に打った相手）/ `socket` を返す。どちらも無いペインは成功を返さずエラー。取り込み時の window 指定（`TmuxOpen.window`）は CLI `--window` と MCP `tako_tmux_open` の `window` の**両方**から到達できること（開発不変条件。到達性は番犬 `mcp_param_reachability` が機械検証する） | M |
| FR-2.13.7 | **`socket` 省略時の解決を系統内で揃える**（#1190）。`tako tmux list` は socket 省略時に既定サーバーと tako バックエンドの**両方**を併記するので、`kill` / `resize` / `open` も同じ順（既定サーバー → tako バックエンド）で探す（`tako_core::tmux::resolve_session_socket` の 1 実装）。明示された socket はそのまま尊重する。`--socket` を付けさせないのが #322 の原則。**tmux の生 stderr を応答へ素通ししない**: `friendly_error` で日本語へ包み、ソケットの絶対パスを出さない（#927 の方針）。応答には解決後の `socket` を含める。なお `kill` が backend セッションへ届くようになるため、生きたペインを壊す前の確認 / `--force` が別途必要（#1196） | M |
| FR-2.13.8 | **`resize --reset` は実際に window を戻す**（#1186）。`set-window-option -u window-size` は「オプションを未設定へ戻す」だけで**その場ではリサイズしない**ため、`resize-window -A` を**先に**打つ（`-A` は window-size を manual にするので順序は逆にできない）。`-A` に失敗してもオプション解除は行い、**エラーはそのまま返す**（成功を返しながら効かないのが #1186 の症状）。クライアントがその window を見ているあいだは `-u` だけでも戻るため、**別 window を見ている / クライアント不在**のときだけ症状が出る（`resize` はスマホリモートのビューポート連動 #23 用なので、PC に戻ったとき縮んだままだと復旧手段が無い） | M |
| FR-2.13.9 | **`backend_windows` は UI の表示状態に依存しない**（#1191）。`tako list` / MCP `tako_list_panes` の各ペインの `backend_windows`（backend セッション内の window 一覧）は、右パネル（fleet ビュー）を開いているかどうかに関わらず**要求時点の実態**を返す。旧実装は fleet ビューの 2 秒ポーリングだけがこの値を更新していたため、パネルを開いたことがなければ常に `null`、閉じているあいだは陳腐化していた（同じ事実に `tako tmux list` と 2 経路があって片方だけ嘘をつく状態）。値の意味は **`null` = そのペインは backend セッションを持たない / tmux から採取できなかった、`[]` = backend セッションはあるが window が 1 枚も無い、`[..]` = window 全件（1 枚でも載る）**。採取は `Request::List` の直前に 1 回だけ行い、コストは `list-windows -a` 1 回（実測 6.9ms 中央値。定期採取の `fetch_tmux_sessions` は 37ms）。backend ペインが 1 つも無ければ tmux を起動せず、直前の採取から 500ms 以内は使い回すので連打・リモートのポーリングでも 2 回/秒を超えない | M |

実装メモ（着手時に設計する）:

- データ取得は `tmux list-sessions -F` / `list-windows -F` の CLI 呼び出し
  （tmux 不在・サーバー未起動時は空一覧で無害に劣化）
- tako との対応付けは「ペイン配下のプロセスツリーに居る tmux クライアント」と
  tmux の `client_tty` / ペインの PTY スレーブの突き合わせを候補に、実装時に検証する

### FR-2.14 MCP ゼロコンフィグオンボーディング（配布後ユーザー向け）

> 配布後のユーザーが手動で `claude mcp add` を打たないと AI 連携が始まらないのは
> tako の思想（ゼロコンフィグで AI フルコントロール）に反する、という課題への要件
> （2026-06-12 要望。実装は Phase 4 後半の現タスク群の後）。

| ID | 要件 | 優先度 |
|---|---|---|
| FR-2.14.1 | 初回起動時（または設定導線から）、インストール済み MCP クライアント（Claude Code 等）を検出し、tako の MCP サーバー登録（`claude mcp add --scope user tako -- tako mcp serve` 相当）を提案・ワンクリック実行できる | S |
| FR-2.14.2 | 登録の有無・接続状態を tako 側で診断・表示できる（未登録・未接続時はその案内を含む） | S |
| FR-2.14.3 | MCP の instructions / ツール説明文を「AI がアプリの趣旨を理解して能動的に機能を使う」品質に整備する（成果物はプレビューペインで提示・子エージェント起動は同タブ内分割、等の tako 流の振る舞いを誘導。FR-2.7.5 の行動規範の拡張） | M |
| FR-2.14.4 | 登録・診断は CLI / MCP からも可能にする（開発不変条件） | M |
| FR-2.14.5 | `tako` CLI バイナリの PATH 設置もオンボーディングに含める。tako 内のシェルから `tako` コマンドが素で使えることが Layer 1（CLI）の前提（2026-06-12 追記）。**tako 内側は FR-2.4.6 のシェル統合注入で解決済み**（Issue #601）。**外部ターミナル向けは #1502 で実装**: `tako setup` の段（および `tako setup bootstrap path` / MCP `tako_setup_bootstrap`）が **`$HOME/.local/bin/tako` へ symlink を張り**、そのディレクトリをログインシェルの profile（`~/.zprofile` のマーカーブロック **1 組**）へ通す。**実体のディレクトリ（`.app` の `Contents/MacOS` / dev の `target/debug`）は PATH へ入れない**（同居物ごと出るうえ `.app` を動かすと黙って切れる）。`$HOME/.local/bin` は claude / codex / agy のランチャーと同じ置き場所なので、macOS ではブロックの中身は 1 ディレクトリのまま。`tako setup bootstrap undo-path` でブロックと symlink の両方を外す（**symlink でない実体は消さない**）。`.app` を移動して切れたリンクは GUI 起動時に張り直す。判定は「**新しいターミナルが見る PATH**」で行う（自分のプロセスの PATH は #601 の注入で必ず「通っている」に見える）。Windows は `HKCU\Environment\Path`（境界 B23）へ**実体のディレクトリ**（インストーラ管理の `%LOCALAPPDATA%\Programs\tako`）を足す = symlink を使わない（権限が要るため）。**Windows 実機未検証**（コード分岐のみ） | S |
| FR-2.14.6 | **セットアップ画面**: 必要なもの（claude CLI の存在 / MCP 登録 / tako CLI の PATH 設置）を**自動診断してチェックリスト表示**し、不足項目は「**セットアップ実行**」ボタン一発でまとめて自動導入する（FR-2.14.1 / 2.14.2 / 2.14.5 の UI 統合。手動手順をユーザーに踏ませない）。診断・セットアップ実行は CLI / MCP からも可能（FR-2.14.4 の対象に含める。2026-06-12 追記） | S |
| FR-2.14.7 | `tako setup` は claude / codex / agy を全検出する。取得可能な認証・プランは source を `detected` と表示して即時採用し、未導入・未認証プロバイダは設定対象にしない。検出不能でも安全な既定値を置ける項目は source `default` で自動解決する。CLI 1 つ・認証済みの標準ケースは質問ゼロで完走し、プラン規模から profiles/default.yaml の master / worker、CLI 既定モデル、effort、worker ポリシーを推奨生成する。質問は「検出不能かつ誤ると実害がある」項目だけに限定し、Enter で進める既定値を必ず示す（Issue #226 / #262） | S |
| FR-2.14.8 | setup 完了済みなら config の agent / provider plan、既存 profile・指示・プロジェクト、設定済み依存・FDA・スリープ設定を確認なしで引き継ぎ、冪等な再実行は質問ゼロ・実変更ゼロで完了する。source は `previous` と表示し、新しい検出値と食い違う場合は `detected` を優先して旧値も通知する。破損 config は既定値で上書きせず中断する。個別対話による見直しは明示的な `--review` だけで起動する（Issue #262）。**`--review` のプランの問いは既定が前回値**で、Enter・範囲外の番号・非 TTY の EOF では保存済みの値を保つ（`選択 [N]` の既定表記も前回値を指し、選択肢に無い値は値そのもので見せる）。Enter を押し続けた結果は標準 setup が前回値から選ぶ値と一致する（Claude Max 検出時の倍率で保てるのは max 系だけ = 検出値と食い違う前回値は検出値を優先）。#1506 前は既定が「不明」固定で、Enter だけで `max-5x` が `max` へ戻った（Issue #1506） | S |
| FR-2.14.9 | setup の全入力（agent、provider plan、グローバル指示、default profile、projects、orchestrator 挙動、sleep guard）は `tako setup --answers <json\|@file\|->` で非対話指定できる。`--yes` は省略値を detected → previous → default で解決して標準入力を読まない。同じ JSON を dispatch `SetupRun` / MCP `tako_setup` へ 1:1 公開し、AI がユーザーの自然言語の希望を回答 JSON に変換して setup を代行できる。標準実行は最終サマリを表示し、項目別確認も最終確認も要求しない（Issue #262） | S |
| FR-2.14.10 | **初回起動のウェルカムバナー**: 初回起動時だけタブバー直下に全幅のバナーを出し、`tako setup`（初期設定）→ `tako master`（AI 司令塔）の 2 ステップと、その場で実行するボタン・「設定を開く」導線を示す。初回判定は `<data_dir>/settings.json` の**実在**で行う（新フィールドの serde default で判定すると既存ユーザー全員に出るため。破損ファイルはファイルが在る以上「初回ではない」= 設定を失った既存ユーザーへ的外れな案内を出さない）。閉じると `welcome_dismissed` を永続化して以後出さない。表示状態の照会・再表示・非表示は dispatch `Welcome` / CLI `tako welcome` / MCP `tako_welcome` へ 1:1 公開する（Issue #549）。**#1510 で初回起動の導線（FR-2.14.18）へ広げた**（`TAKO_1510_LEGACY=1` でこの行の形へ戻る） | S |
| FR-2.14.11 | ⌘K コマンドパレットに「セットアップを実行」「master を起動」「設定を開く」を固定コマンドとして常設する（バナーを閉じた後も、また既存ユーザーにも導線が残る）。バナー・パレットの「その場実行」は素のログインシェルのタブを立ててコマンド行を送る（`tako master` は**呼び出し元ペインを自分で解決して inline 起動する**仕様のため、ペインの直接プロセスや `sh -c` ラッパー越しでは起動コマンドの送り先が壊れる）。コマンド行は `resolve_tako_binary()` の解決結果で組み立て、`tako` が PATH に無い zip 配布でも動くようにする。バナー本文の案内は最簡形（`tako setup` / `tako master`）を保つ（#322。Issue #549） | S |
| FR-2.14.12 | **詰まった段で setup 全体を止めない**（Issue #1501）。ログイン（ブラウザ操作なので tako は代行しない = #1129）・エージェント CLI の導入失敗・PATH 通しの失敗・必須依存の不足・MCP 登録の失敗は、**人へ残る作業 1 件として脇に置き**、認証不要な段（依存導入 / MCP 登録 / グローバル指示ファイル / `profiles/default.yaml` / テンプレ展開 / シェル統合 / tako CLI の PATH 設置）を**全部やってから exit 0** で終わる。末尾に「残り N 件」を**人が次に打つ 1 コマンド**つきで出し（#322 の最簡形）、再実行は済んだ段を冪等に素通りして残りから再開する。1 つも検出できない環境でも、設定の書き先は前回値 → ゼロスタート導入の対象 → claude の順で決めて生成物を先に揃える。**「完了しました」と言い切るのは残り 0 件のときだけ**。判断（同じ道の畳み込み・並び順・文面・最簡コマンド）は `tako_control::setup_remaining` の純粋関数が正本で、`tako setup` / `tako setup --check` / dispatch `SetupRun` / MCP `tako_setup` は同じ 1 実装を通る。A/B は `TAKO_1501_LEGACY=1`（#1501 前 = 詰まった段で exit 1）。**setup.completed は残りがあっても記録する**（setup 自身はやれることを全部やり切ったので、次回は残りの確認から始まる） | M |
| FR-2.14.13 | **外部コマンドへの問い合わせ（probe）は待ち時間に上限を持つ**（Issue #1503）。`tako setup` はエージェント CLI の状態を `claude auth status --json` / `codex login status` / `agy models` / `claude mcp list` へ聞いて決めるが、**`claude mcp list` は登録済みの MCP サーバへ 1 台ずつ繋いで健全性を見る**ので、サーバが 1 つ無応答だと返らなくなる（#1500 の棚卸し R4 = `tako setup` が**無言で 6 分固まり続けた**）。上限（既定 **15 秒**。実測 = `claude mcp list` が MCP 13 サーバ登録の実機で 4.57〜5.26 秒 / `auth status` が 0.12 秒なので最も遅い probe の約 3 倍）を超えたら**打ち切って「確認できません（N 秒応答なし）」を出し、その段は確認できないまま次へ進む**（FR-2.14.12 と同じく setup 全体は完走する）。dispatch `SetupRun` / MCP `tako_setup` は `tako setup` を子として待つので**そちらにも上限**（既定 600 秒 = 中で導入器が走るため桁が違う）を持ち、打ち切りは `completed: false` + `timed_out: true` + `waited_secs`、子の中で打ち切った probe は `probe_timeouts: [{label, waited_secs}]` として応答へ載せる（開発不変条件: UI で分かることは MCP / CLI からも分かる）。待ちの 1 実装は `tako_core::probe`（`wait_with_timeout` / `output_with_timeout`）で、**上限を外す指定は用意しない**（env `TAKO_SETUP_PROBE_TIMEOUT_SECS` / `TAKO_SETUP_RUN_TIMEOUT_SECS` は値を変えるだけで、0 / 不正 / 空は既定へ落ちる）。A/B は `TAKO_1503_LEGACY=1`（#1503 前 = 上限なしで固まる） | M |

| FR-2.14.14 | **シェル統合（OSC 7 / 133）の配置を setup の段として持つ**（Issue #1504）。`shell_integration::install()` の呼び手は CLI `tako shell-integration` と MCP だけで、**setup も installer も呼んでいなかった**（#1500 の棚卸し Z9）。macOS / Linux は spawn 時の環境変数注入で届くので人の手はゼロだが、**Windows は `$PROFILE` へブロックを追記しないと 1 行も効かない**ので、ペインの cwd 追従・コマンド実行状態・入力予測（FR-2.4.1 / 2.4.5）・自動命名の素材（FR-2.12.13）が「人が `tako shell-integration install` を自分で見つけて打つまで欠けたまま」だった。`tako setup` は**ゼロスタート導入より前**（エージェントの導入状況に依存しないため）にこの段を通し、`tako setup --check` は同じ 1 実装で状況を報告する。**配置が要るかは `shell_integration::Delivery`（automatic / profile）が宣言する**ので `cfg!(windows)` で分岐しない（= macOS 上でも Profile 経路の表示・冪等・失敗を検査できる。実機を持たない CI で永久に未検査になるのを避ける）。**ユーザーのファイルへ書く前に「何をどこへ」（配置先の `$PROFILE` のフルパス・置くのは管理ブロック 1 個だけ・これで効くもの・`tako shell-integration uninstall` で戻せること）を出して同意扱いで続行する**（#868 / #1502 と同じ作法。`[y/N]` は出さない = #262 の質問ゼロなので `--yes` / 非 TTY / GUI の初回起動で分岐せず同じ結果になる）。**2 回目は予告も出さず差分ゼロ**（FR-2.14.8）。**配置に失敗しても setup は完走**し、`setup_remaining::RemainingKind::ShellIntegration`（次に打つ 1 行は `tako shell-integration install`）として末尾の「残り」へ載る（FR-2.14.12 の契約。段は `Result` を返さないので呼び手が `?` で上へ投げられない）。判断と文面の正本は `tako_control::shell_integration`（`run_setup_stage` / `check_line` / `check_remaining`）1 か所で、`install()` を呼ぶ本番コードもここだけ（番犬 `crates/tako-control/tests/issue1504_setup_shell_integration_watchdog.rs` が二重化・質問の再生・cfg 分岐・`Result` 化を file:line で落とす）。A/B は `TAKO_1504_LEGACY=1`（#1504 前 = setup が配置に触らない）。**Windows 実機未検証**（macOS 上の Profile 経路は `Status` を組んだ単体テスト、配線は番犬、実経路は `scripts/test-setup-shell-integration-1504.sh` の 54 項目）。**#2432 で Windows も `$PROFILE` へ書かなくなった**（10/27 のユーザー判断）: 実行ポリシーが既定（クライアント OS は Restricted）の PC では、書いたブロックのせいで tako の外の PowerShell まで起動のたびにエラーを出しうるため。統合は tako がペインの PowerShell を起こすときの起動引数（`-NoExit -EncodedCommand`。スクリプトをテキストで読んで実行するので実行ポリシーの対象外・`-NoProfile` を付けないのでユーザーの profile も読まれる）で読ませ、インストール直後・setup の前から効く（#1595 も解消）。`Delivery` は automatic / launch になり、段は「スクリプトを書き出し、どう届くかを 1 行で言う」だけ（予告・質問・ユーザーのファイルへの書き込みは無い）。以前の版が置いたブロックは自動移行 `SchemaId::PowerShellProfile`（`tako setup` の移行の段・GUI / master の起動時）がブロックだけを取り除き（他の行はバイトのまま・ブロックだけのファイルは消す・`.pre-v1.bak` に退避）、`tako shell-integration uninstall` でも外せる | M |
| FR-2.14.15 | **環境診断の項目は 1 実装から出る**（Issue #1505）。`tako setup --check`（tako-cli）と `tako check-health` / MCP `tako_check_health`（dispatch）が**同じ「この環境で tako は使えるか」を別々の実装で答えていた**: `--check` にはシェル統合・tako CLI の PATH・アップデート追従・リモート公開・IPC の受け口の行が 1 つも無く、PATH は `check_health` だけが別口で見ていた（#1500 の棚卸し Z19 / R8 の実測）。片方へ項目を足してももう片方は黙って古いままになる形で、「UI でできることは CLI / MCP でもできる」（設計原則 5）の前に**答えが 2 種類**あった。いまは `tako_control::diagnostics::collect()` が**項目の集合・判定・人へ出す行**をまとめて持ち、`--check` は `Report::lines()` を出して `Report::remaining()`（FR-2.14.12 の 1 実装）で締める / `check_health` は `Report::to_json()` を応答の `diagnostics` 節（`items[].key` / `status` / `summary` / `lines` / `remaining`）として載せる。**行は字面まで同じ**（`lines` が `--check` の出力そのもの）なので、4 経路（`--check` / `check-health` のローカル・dispatch・MCP）の一致を機械照合できる。判定は 1 つも二重に書かない（tmux の有無は `dep.<器>` の項目から引く・tako CLI の PATH は `tako_cli_path_check_line` / シェル統合は `shell_integration::check_line` / 依存の案内は `setup_deps::offer_and_install` を**読み取り専用の文脈**で通す）。**打ち切りの知らせは落とさない**（#1503 / FR-2.14.13 の契約）: 上限つきの問い合わせは `agent_probe::output_with_timeout` の 1 実装を通り、打ち切ったら `[確認できません] <label>（N 秒応答なし）。打ち切って次へ進みます` を stderr へ出す（dispatch / MCP が `probe::parse_notices` で読み戻す形と対）。同じものを診断の `diagnostics.probe_timeouts` からも読める（設計原則 5）。**IPC の項目は観測者で文面を変えない**（GUI の中では起動時の記録、外からは繋ぎ先の実測を読むが、届いているときは同じ 1 行。受け口の詳細は `ipc` 節 = FR-2.2.10 が持つ）。正本はエージェント CLI へ問い合わせるので**秒単位で掛かる**（実測: 3 系統そろった Mac で 10.8 秒。#1505 前の `--check` は認証 probe を 2 度打っていたので 12.4 秒。#1622 で外部の問い合わせを重ねて約 6 秒 = FR-2.14.19）ため、GUI 経路は `prepare_offload` で background executor へ出す（UI スレッドで走らせると窓が止まる）。番犬 `crates/tako-control/tests/issue1505_diagnostics_single_source.rs` が「`--check` が正本以外から項目を組む」「`check_health` が `diagnostics` を落とす」「tmux の判定を持ち直す」「`CheckHealth` を offload から外す」を file:line で落とし、実経路は `scripts/test-setup-check-single-source-1505.sh`（隔離 HOME + 隔離 GUI で 38 項目） | M |
| FR-2.14.16 | **setup の末尾はリモート（スマホ）の状態を見て 1 行を出す**（Issue #1507）。#1507 前の末尾は `スマホからリモート接続するには: tako remote setup` の**固定文 1 行**で、Tailscale が未導入でも公開まで済んでいても同じ案内だった（#1500 の棚卸し Z17 / 実測 R5 / R9。setup 本体は remote の状態を 1 つも見ていなかった）。いまは `remote_setup::setup_summary_lines` の 1 実装が `remote_setup::check_status`（#1816 から依存段の検出結果を `check_status_with` へ渡し、検出を引き直さない = FR-2.33.13。**読み取りだけ**: `tailscale status --json` / `tailscale serve status --json` を聞くだけで、導入・起動・設定の書き換えはしない）を 1 回読み、`phone_readiness`（JSON だけを入力にする純粋関数）がウィザードの段の順（導入 → 起動 → ログイン → 証明書 → 公開）で**最初に欠けている段を 1 つだけ**決め、`phone_readiness_line` が `スマホからの接続: …` の 1 行にする（未導入 / 起動していない / 未ログイン / 接続が有効でない（状態つき）/ 証明書が未有効 / 準備済みで未公開 / 公開済み（URL）/ tako 以外の serve 設定 / serve を読めない / 確認できない の 10 通り。字面は単体テスト `状態ごとの1行を固定する` が固定）。**Tailscale の導入を勧める・聞くのは依存チェック段の 1 回だけ**（#1499 / #1524。同じ実行で 2 度聞かない）で、末尾は未導入なら**依存の導入口**を指すだけにする: 次の一手は `setup_deps::next_step_line`（依存チェック段が案内で終えるときと字面まで同じ = 代行できれば `いま入れる: tako setup deps install`、brew が無ければ `導入方法: …`）から引き、`--yes` なら依存チェック段がその場で入れた結果を末尾が読む。入っていて途中まで済んでいる状態の次の一手は `tako remote setup`（#322 の最簡形。引数を付けない）。setup からは導入も起動もしないので、`--yes` / 非 TTY / `--answers` / 端末ありのどれでも質問は増えず止まらない・何度走らせても同じ 1 行（冪等）。**待ちに上限を持つ**（#1503 / FR-2.14.13 の規約）: `tailscale --version`（検出）は素の `.status()` だったので `tako_core::probe::output_with_timeout` の 1 実装へ寄せ、`status` / `serve status` は tailscale 側の上限（1 回 10 秒）で打ち切る。打ち切りは文字列に埋めず `tailscale::RunError::TimedOut` として型で持ち、`SetupStatus.timed_out` と `check_status` の `timeouts`（`label` / `waited_secs`。GUI のリモートパネル・dispatch `RemoteSetup { action: "check" }` からも読める）へ載せる。#1507 前は `status --json` の打ち切りが `DaemonNotRunning` へ畳まれていたので、そのまま読むと応答しないデーモンを「起動していません」と言い切っていた。打ち切ったら #1503 の文面（`[確認できません] tailscale status --json（10 秒応答なし）。打ち切って次へ進みます`）を出してから「状態を確認できませんでした」の 1 行で締める（dispatch `SetupRun` / MCP `tako_setup` が `probe::parse_notices` で読み戻して `probe_timeouts` へ載せる形と対）。番犬 `crates/tako-control/tests/issue1507_setup_remote_status_watchdog.rs` が「固定文への巻き戻し」「寄せ先を呼ばない」「tailscale を別口で聞く」「導入口のコマンドを自前で書く」「検出の待ちが素の `.status()`」「打ち切りを未起動へ畳む」を file:line で落とし、実経路は `scripts/test-setup-remote-status-1507.sh`（隔離 HOME + tailscale / brew スタブで 41 項目。`TAKO_BIN=<#1507 前の tako>` で 15 項目が落ちる） | M |
| FR-2.14.17 | **環境診断の認証確認は上限つきで、系統を並べて聞く**（Issue #2255）。環境診断（FR-2.14.15 の `diagnostics::collect` = `tako check-health` / MCP `tako_check_health` / `tako setup --check`）は導入状況を通して認証を `claude auth status --json` / `codex login status` / `agy models` へ聞くが、#2255 前は setup と同じ**上限なしの** `agent_probe::run` で 1 系統ずつ待っていたので、CLI が 1 つ返らないと check-health が返らなかった（GUI 経由は CLI の応答の上限 300 秒まで待ち、その後に CLI 自身が回す診断も同じ問い合わせで止まる。check-health は固まった GUI の切り分けに使う道具 = #1441 / #2002 なので、それ自体が固まると切り分けができない）。いまは `setup_bootstrap::status_all_within` が 3 系統を並べて `auth_state_within`（上限つきの門番 `agent_probe::output_within`）で聞き、上限（既定 **10 秒** = `probe::auth_probe_timeout`。根拠: いちばん遅い `agy models` の実測 3.0 秒の約 3 倍。env `TAKO_AUTH_PROBE_TIMEOUT_SECS` は値を変えるだけで 0 / 不正 / 空は既定へ落ちる = 外せない）を超えた系統は子を kill して「**認証: 不明（N 秒で応答なし）**」と出す（`agent_cli.<系統>` は `[警告]`・`agents_detected` の行・`diagnostics.probe_timeouts` に `{label, waited_secs}`。未認証とは言い分け、ログインや導入の残り作業も積まない）。他の項目はそのまま出る。setup（`tako setup bootstrap`）の対話経路は上限なしのまま（`status_for` / `auth_state_for`）。番犬 `crates/tako-control/tests/issue2255_diagnostics_auth_probe_watchdog.rs` が `collect` から呼び出しを辿って上限なしの `agent_probe::run` に届く行を file:line で名指しする。実経路は `scripts/test-check-health-auth-timeout-2255.sh`。A/B は `TAKO_2255_LEGACY=1`（上限なしで 1 系統ずつ = 固まる） | M |
| FR-2.14.18 | **初回起動の導線: 副作用の小さい setup の段を自動で走らせ、残り作業のあいだは「次にやること」を出し続ける**（Issue #1510。#1500 の棚卸し Z1・10/27 のユーザー判断）。#1510 前のバナーは `tako setup` → `tako master` を並べるだけで、**どのボタンを押しても以後出ず**、setup は claude を入れた後「次の 1 手: claude auth login」で止まった（ログイン → setup のやり直し → `tako master` を人が自分で打つ）。①**初回起動**（settings.json が無い）で `welcome_onboarding`（settings.json・serde default false = 既存ユーザーには立たない）を立て、setup が一度も最後まで走っていなければ（`config.setup.completed`）**自動の段**を background で 1 回走らせる。自動の段は MCP `tako_setup` と同じ `tako setup --yes --answers -`（上限つき）で、answers は `welcome::auto_setup_answers` = **`install: false`**（エージェント CLI の公式インストーラも brew / winget の依存導入も走らせず残り作業にする）+ `launch_agent: none`。起こす CLI は **GUI と同じ版**（実行ファイルの隣 = `welcome::auto_setup_binary`。`/Applications` の古い版は `install` を知らず `deny_unknown_fields` で断る = ネットワークへは出ないが導線が進まない）。走るのは PATH・シェル統合・MCP 登録・指示ファイル（無ければ作る）・プロファイル・テンプレ・移行で、どれも冪等。2 回目の起動では繰り返さない（失敗していれば次の起動で残りから再開）。全文は `<data_dir>/setup/first-run.log`。②**「次にやること」**は `welcome::next_step`（純関数）が導入状況（`setup_bootstrap::status_all_within`）から決める: master を起こせる系統（能力マトリクスの `master_launch` = claude / codex）が 1 つでも使えれば **master を起動**、無ければ途中まで入っている系統 → 推奨（claude）の順で **入れる / PATH / ログイン**、確かめられなければ **unknown**（`tako setup` のタブ）。③**導入とログインはバナーのボタン 1 つの同意**で行う: ボタンは新しいタブ（導線のタブが残っていればそこ）へ `tako setup bootstrap install` / `<解決済みの実行ファイル> auth login` を**打つ**（利用者が自分で打つのと同じ経路 = #1129 の「見えない子プロセスでログインを待たない」を守る）。バナーは「何をどこに」（公式コマンドと置き場所）をボタンの前に見せる。PATH の段は自動の段が受け持つ（1 回だけ走らせ直す）。ボタンの後は 5 秒おきに最大 20 分見張り、段が進んだら自動の段を当て直して（入れた直後の MCP 登録など）次の段を出す。④バナーは導線が続いているあいだ**毎回**出る。「あとで」はいまだけ閉じ、「今後表示しない」は `welcome_dismissed`、「master を起動」で `welcome_onboarding` を倒して終わる。⑤**検証プロセス**（テスト・隔離 GUI・セルフテスト = `paths::is_verification_process`）は HOME が利用者のホームのままのことが多いので、`TAKO_FIRST_RUN=1` を明示したときだけ導線を動かす（既定は #549 の形）。セカンダリのインスタンスは走らせない。⑥状態と操作は dispatch `Welcome`（CLI `tako welcome [status|next|auto|show|hide|dismiss]` / MCP `tako_welcome`）の 1 実装で、GUI のボタンも同じ要求を投げる。確かめ（ログインシェル・認証の問い合わせ）とボタンの中身の組み立て（tako の実行ファイルの解決もログインシェルを待ちうる）は offload の background、段を GUI へ渡す・押すのは UI スレッドの着地（#1984 の番犬が縛る）。縛り: `welcome` の `issue1510_*`（自動の段の answers が導入を許さない・段の判定・ボタンの中身の方言・遷移）と tako-cli の `issue1510_*`（`install: false` なら導入の段も依存の段もネットワークへ出ない）。A/B は `TAKO_1510_LEGACY=1` | M |
| FR-2.14.19 | **環境診断の外部の問い合わせは重ねて走らせ、答えは 1 バイトも変えない**（Issue #1622）。FR-2.14.15 で `--check` と `check-health` を `diagnostics::collect` の 1 実装へ寄せた代償で、`check-health` が 1 秒台から約 10 秒へ伸びていた（2026-10-30 の main で再現: 9.5〜12.0 秒。内訳は導入状況 = 3 系統の認証確認が並んで約 3 秒、その**後に**逐次の `claude mcp list` 約 5.7 秒・依存の検出 0.6 秒・設定共有 0.4 秒・tako CLI の PATH 0.2 秒）。CLI / MCP の呼び手は応答まで待たされる。いまは `collect` が外部プロセスを起こす問い合わせ（MCP 登録 3 系統・依存・設定共有・tako CLI の PATH）を `collect_with` へ渡し、導入状況を呼び出し元のスレッドで採る間に、残りを背景で**最大 3 本**（`PROBE_CONCURRENCY`。ログインシェルと CLI を一度に起こしすぎて遅い機で個々の上限に掛からないよう抑える）ずつ、**長い順**（`claude mcp list` が先頭）に走らせる。項目は今まで通りの並びで組み（`assemble` は問い合わせを 1 つも起こさない）、MCP の項目は今まで通り**導入済みの系統だけ**出す（MCP の問い合わせは導入状況を待たずに始まるので、未導入の系統の答えは組み立てで捨てる）。打ち切りの知らせ（`probe_timeouts`・項目・stderr の `[確認できません] …`）も落とさない。stderr の即時の 1 行は打ち切った順に出る（既定の上限 = 認証 10 秒 < `claude mcp list` 15 秒なら逐次と同じ順）。実測（3 系統の Mac・隔離の data dir）: `check-health --json` 9.7〜12.0 秒 → 5.8〜6.4 秒、`setup --check` 9.0〜9.9 秒 → 5.4〜5.9 秒、出力は修正前のバイナリ・A/B と sha256 まで一致。番犬は `crates/tako-control/src/diagnostics.rs` の単体テスト `外部の問い合わせは導入状況と重ねて走り答えは逐次と同じ`（偽の問い合わせが互いの始まりを待ち合う = **時間で比べない**。直列へ戻すと待ち合いが成立せず `fn collect_with` を file:line で名指し・同時数が上限を超えたら `PROBE_CONCURRENCY` を名指し・逐次に組んだものとの一致）と `本番の入口はゲートから同時数を決める`。実経路は `scripts/test-check-health-parallel-1622.sh`（偽の claude / codex / agy の砂場で、偽の `agy models` が偽の `claude mcp list` の始まりを印のファイルで待つ = 重なりを状態で見る・打ち切り・導入済みだが PATH に無い系統・CLI が 1 つも無い砂場）。A/B は `TAKO_1622_LEGACY=1`（重ねない = 導入状況の後に 1 本ずつ。問い合わせの集合と組み立ては同じ） | M |

実装メモ（着手時に設計する）:

- クライアント検出は claude CLI の存在（ログインシェル経由の `command -v`。
  autorename の `claude_bin()` と同じ手口）+ `claude mcp list` のパースが候補。
  Issue #226 で codex / agy まで実装済み。Claude は `auth status --json` の loggedIn /
  subscriptionType、Codex は `login status` + `~/.codex/auth.json` の OAuth JWT にある
  chatgpt_plan_type（ローカル復号のみ）、agy は `models` の終了状態から認証を判定する。
  agy のプラン、Claude Max の 5x / 20x、3者の利用量は安定取得できないため質問する。
  token・メールアドレス・account ID は保持・表示・config 保存しない
- モデル名はリリースで陳腐化するため固定せず各 CLI の既定へ委ねる。プラン規模は
  Limited / Standard / High に正規化し、master effort と複数 CLI 時の delegate 方針へ反映する。
  agy は master 非対応なので、claude / codex があればそちらを master、agy を worker とする
- FR-2.14.3 は既存の `mcp.rs` INSTRUCTIONS / ツール説明文（FR-2.7.5 行動規範）が土台。
  プレビュー系（FR-2.7 / FR-3.8〜3.10）の実装が進むたびに instructions も同期させる

### FR-2.15 ターミナルのたまり場（タブ外プール。2026-06-14 実装）

> 「見た目はタブから消したいが、処理は生かしておきたい」ターミナルの置き場。
> Phase 5.5 の tmux バックエンド（FR-5）により「表示ペインを持たない生きたセッション」が
> 存在できる構造（orphan セッション = tmuxview に区別表示）が前提として整っている。

| ID | 要件 | 優先度 | 状況 |
|---|---|---|---|
| FR-2.15.1 | ペインタイトルバーに**退避（ー）と kill（×）を分離**配置する。ー = たまり場へ退避（プロセス・tmux セッションは生かしたまま画面から外す）、× = kill（タブの × と統一。backend `tako-*` / view `tako-view-*` セッションごと破棄し「管理外」に残さない） | M | ✅ 2026-06-15 改 |
| FR-2.15.2 | たまり場内カードの × = 「完全に破棄?」確認（はい/いいえ）→ kill（tmux セッションごと破棄。外部由来セッションは `drop_tmux_view_session` のラッパー限定 kill に従う） | M | ✅ 2026-06-15 改 |
| FR-2.15.3 | ドラッグ & ドロップでたまり場 → タブ内ペインへ復帰できる | M | ✅ 2026-06-14 |
| FR-2.15.4 | AI も MCP / CLI から出し入れできる（邪魔なペインを退避させる・成果物をたまり場から取り出してユーザーに見せる、という tako のコンセプト操作。開発不変条件） | M | ✅ 2026-06-14 |
| FR-2.15.5 | たまり場の中身もレイアウト永続化（FR-5.2）の対象にする（再起動で復元）。**再起動後も退避のまま・同じ器で戻す（#1576）**: たまり場・退避タブ（FR-2.15.7）のペインは、復元ループで表のペインと**同じ経路**（器が生きていれば再 attach・消えていれば保存 cwd の新シェル + 会話の resume。判断は `sessions::restore_plan`）で器へ繋ぎ直し、**同じ pane id・タイトル・role・limit_resume・由来タブのまま退避に置く**（退避中も端末を持つのは再起動前と同じ = FR-2.15.1 の「生かしたまま画面から外す」）。#1576 以前は「表に出すときに起こす」つもりで素通りさせていたが、表に出す経路（`reattach_backgrounded`）は端末を起こさず、器も `backend_sessions` へ登録しなかったので、①起動時の orphan 自動復帰（#191）が器を「復帰」タブへ**別 pane id** で拾い（role・limit_resume・worker の紐付けが切れ、閉じると器ごと kill = 2026-09-23 / 10-09 に worker の会話が消えた）②退避エントリは端末の無い**幽霊**として残り、次の保存で器の名前が `null` になっていた。起こす / 残す / 外すの判断は `tako_control::shelved_restore::plan` の 1 実装で、**起こせないものは退避から外して** `復元失敗（ペイン N）: <分類>` を persist.log に残す（器も戻す手掛かり = cwd / 会話 / プレビューも無いエントリ = 旧版が残した幽霊。器が永続しない環境では外さない / 同じ器を別のペインが持つエントリ。タブ配下が優先）。Web ビューのペインは dock の仕組みなので起こさない。「復帰」タブの close の意味（器ごと kill）は変えない（#1576 以後そこへ出るのは layout に載っていない本物の orphan だけ）。内訳は FR-5.7.1。A/B は `TAKO_1576_LEGACY=1`、番犬は `crates/tako-control/tests/issue1576_shelved_restore_watchdog.rs`、実経路テストは `scripts/test-shelved-restore-1576.sh` | S | ✅ 2026-10-09 改 |
| FR-2.15.6 | **退避のタブ別分離**: 退避ペインは退避元（由来）タブを記録し、たまり場を**由来タブごとに分離**して見せる。① 統合 tmux ビュー（FR-2.16.6）では各タブ枠内に「そのタブ由来の退避ペイン」を**バックグラウンド行**（親タブはタブ枠が明示）として表示し、由来タブが既に閉じている退避は「タブ \<名前\>（閉じたタブ）」グループにまとめる ② たまり場ドロワーは由来タブごとにグループ見出し付きで横並び表示する ③ 復帰は既定で由来タブへ戻す（閉じていればアクティブタブ）。状態取得・操作は `tako_shelved_list` の `origin_tab` / `origin_tab_title` と `tako_unshelve_pane`（target 省略＝由来タブ）で AI からも可能（開発不変条件） | M | ✅ 2026-06-15 |
| FR-2.15.7 | **タブ単位の退避・復帰（#1487）**: タブバーの「ー」/ タブの D&D でバックグラウンドへ送ると、タブは**分解されず 1 単位**で退避される（`Workspace::shelved_tabs`。分割ツリー・比率・ペイン属性・由来ウィンドウ・元の並び位置を保つ）。たまり場・右パネルには**タブバーのタブと同じ形のカード 1 枚**（状態ドット + タブ名 + ペイン数バッジ + ×）で出て、**カードのどこを押しても**そのタブが元の位置・元の分割で復帰する（#1491。配下ペインのサムネイルカードは並べない。右パネルはカードの下へペイン行を畳み `▸` で開く）。カード右端の × はタブバーのタブの × と同じ意味（タブごと kill）で、ペインカードと同じ 2 段確認を通し `stop_propagation` で本体クリックと分ける。一括プレビューは FR-2.16.16 のホバーを流用する。復帰は元のウィンドウの元の位置へ（ウィンドウが閉じていればアクティブウィンドウ、位置が超えていれば末尾）。退避タブから**1 ペインだけ**取り出す / kill することもでき、空になった退避タブは消える。CLI / MCP は `tako background --tab` / `tako foreground --tab` / `tako backgrounded` の `tabs` / `tako list` の `shelved_tabs`（開発不変条件）。永続は `layout.json` の `shelved_tabs`（serde default で旧ファイル互換）。A/B は `TAKO_1487_LEGACY=1`（#1487 前の平坦化）と `TAKO_1491_LEGACY=1`（#1491 前の「見出し + 「タブごと復帰」ボタン + ペインカード列」） | M | ✅ 2026-09-21 |
| FR-2.15.8 | **たまり場の見え方と後始末（#1946）**: ①**縮小で全体を見せる**: カードはペインの画面**全体**（全行・全列）を、セルと字を同じ比で縮めた端末グリッド（#787 と同じ描画経路）で描く。**退避ペインを resize しない**（#1946 以前はドロワーが開くたびにカード寸法 = 40 列 × 8 行程度へ縮めていたので、claude は下端だけを描き直し入力欄と帯しか残らなかった）。カードの幅は画面の縦横比に合わせる。プレビューはコード / Markdown の頭を同じ縮小で、画像・動画のサムネ・PDF の現在ページは画像で描く。カードのホバーで画面全体を窓の 6 割まで（元の大きさは超えない）読める大きさで出す（FR-2.16.13 のポップアップ。閉じたタブの一括プレビューも縮小で全体）。②**由来タブ → 親 master の見出し**: 由来タブごとのまとまりの中を、親 master ごとの見出し（master アイコン + プロファイル名 · タイトル、退避中なら「退避中」）で分け、master 自身のカードは見出しの先頭に `master` の印付きで置く。親は `spawned_by` の枝を辿って着いた master（枝の無い worker は master が 1 枚のときだけそこへ寄せる = `Workspace::workers_of` と同じ規則）、枝の先が居なければ「親 #N は閉じた」、決まらなければ「親 master なし」。`spawned_by` は `layout.json` の `PaneLayout.spawned_by` に残し（serde default で旧ファイル互換・移行 Step 不要）、再起動でもまとまりが崩れない。タブ単位の退避（FR-2.15.7）はタブ形カード 1 枚のまま。③**器の無い退避エントリ（幽霊）**: 器（tmux / 直の PTY / プレビュー / Web ビュー）の判定は `tako_control::dispatch::background_vessel` の 1 実装（GUI も同じ関数を呼ぶ）。幽霊は赤の破線の枠 + 「器なし（GUI 再起動で失われた）」で見分け、「復帰」は出さない。× → はい（`BackgroundKill` も同じ）で**一覧から外すだけ**にし、器の kill もレジストリへの「閉じた」の記録もしない（本物が別 pane id で生きていることがある = #1576 の機序。死んでいれば #658 の GC が閉じる）。判断は `tako_core::background_groups::close_plan`。④**軽さ**: サムネイルの行は出力・版・テーマが変わったペインだけ組み直し（`TerminalGrid` の行は `Rc` で使い回す）、サムネイルにだけ映っているペインの出力はアプリ全体の描き直しを 4 回/秒へ間引く（`PaneVisibility::Thumbnail`）。退避タブ配下はドロワーを開いていても出力で描き直さない（以前は閉じていても全体を描き直していた）。⑤**CLI / MCP**（開発不変条件）: `tako backgrounded` / `tako_background_list` の各ペインに `vessel`（`tmux` / `direct` / `preview` / `web` / `none`）と `parent_master`、全体に画面と同じまとまり `groups`（`origin_tab` / `title` / `origin` = `tab` / `shelved_tab` / `closed_tab` / `count` / `masters[{master, panes}]`）を載せる。`tako_background_kill` の応答に `vessel`。グループ化は `tako_core::background_groups::groups` の 1 実装。A/B は `TAKO_1946_LEGACY=1`（`TAKO_1491_LEGACY=1` も #1946 前のたまり場で描く）、番犬は `crates/tako-control/tests/issue1946_background_drawer_watchdog.rs`、実フレームの検証は `scripts/test-background-drawer-1946.sh`（visual-test 節 `background-drawer`） | M | ✅ 2026-10-10 |

実装メモ（2026-06-14）:

- UI（2026-06-15 横並びプレビューへ刷新）: 下部ステータスバーの「⏏ 退避」ボタン（退避中
  ペイン数バッジ付き）でドロワーを開閉（既定 240px）。ドロワーは退避中ターミナルを
  **横並びのプレビューカード**で並べ、**横スクロール**で全件閲覧できる。各カードは通常
  ペインと同じ実画面サムネイル（`terminal_screen_lines()` を render_pane と共用。カード
  本文の cols/rows に合わせて退避ターミナルを resize。**#1946 で廃止**: resize せず画面全体を
  縮小で描く = FR-2.15.8）+ 上部タイトルバー（状態色ドット +
  ラベル + 「復帰」ボタン + 右上 × kill）。× は「完全に破棄?」確認 → tmux セッションごと
  kill。タイトルバー D&D / ペインエリアへのドロップで復帰
- ペイン側の操作分離（2026-06-15）: ペインタイトルバーに ー（退避 = `shelve_pane_button`）と
  ×（kill = `close_pane_button` → `remove_pane`）を併置。タブバーの ー / × と挙動を統一
- データモデル: `Workspace::shelved: Vec<ShelvedPane>`（`ShelvedPane` = ペイン + 由来タブ
  `origin_tab` / `origin_tab_title` のスナップショット。FR-2.15.6 タブ別分離用。由来タブは
  退避でタブごと閉じることがあるため ID に加えタブ名を保持）。`shelve_pane()` /
  `unshelve_pane()`（target 省略＝由来タブへ復帰）/ `remove_shelved()` /
  `shelved_origin_tab()`。ターミナルセッションは `terminals` HashMap に残したまま（kill しない）
- dispatch: `Shelve` / `Unshelve` / `ShelvedList` / `ShelvedKill` の 4 リクエスト
- CLI: `tako shelve` / `tako unshelve <pane>` / `tako shelved`（一覧）
- MCP: `tako_shelve_pane` / `tako_unshelve_pane` / `tako_shelved_list` / `tako_shelved_kill`
  （計 27 ツール）
- 永続化: `layout.json` に `shelved: [PaneLayout]` を追加（serde default で後方互換）
- 最後のタブの最後のペインの退避時は、新しいシェルペインを自動生成してから退避する

実装メモ（2026-09-21・FR-2.15.7 / #1487）:

- データモデル: `Workspace::shelved_tabs: Vec<BackgroundTab>`（`Tab` 本体 + `origin_window` +
  `origin_index`）。**ペイン単位の `shelved` はそのまま残す**ので、過去の平坦な退避
  （旧 layout.json）は「閉じたタブ」グループとして従来どおり読める
- 判断は純粋関数 1 本ずつ: 戻す先 = `unshelve_tab_placement()`、空になった退避タブの去就 =
  `shelved_tab_fate()`。UI / dispatch / 復元のどこから来ても同じ結論になることを単体テストで固定する
- 「バックグラウンドに居るペイン」を数える経路（tmux の掃除・ペインログ・ビューの刈り取り・
  worker レジストリ）は `Workspace::all_background_panes()` を通す。1 か所でも漏らすと
  **生きている器を殺す**（#1487 で退避タブへ移った分がここから消える）
- 永続: `LayoutFile.shelved_tabs: Vec<ShelvedTabLayout>`（`TabLayout` を入れ子）。
  旧ファイルは serde default でそのまま読める = 移行 Step は不要（指紋は更新する）
- **タブ形の描画語彙は `tako-app::tab_shape` の 1 か所**（#1491）。寸法・状態ドット・
  小バッジ・ボタンスロットをタブバー（`tab_bar.rs`）と退避タブカード
  （`render_shelved_tab_card`）が共用するので、タブバーを触ったときに
  たまり場のカードだけ取り残される（= 同じものが 2 つの見た目になる）ことが起きない。
  カードはドロワーと右パネルで同じ 1 実装を通り、復帰は既存の `unshelve_tab_clicked`
  （= `Request::Foreground { tab }`）へ寄せる（**新しい経路は作らない**）

### FR-2.16 右サイドバー情報パネル（切り替え式コンテナ。2026-06-12 実装）

> 固定タブを 0 個にし、tmux 一覧（FR-2.13）と集約センター（FR-2.10）を右サイドバーの
> **内部タブ**として統合する（ユーザー指示）。将来 git graph（FR-3.6）もここに足すため、
> パネルは最初から「切り替え式情報パネルのコンテナ」として設計する。

| ID | 要件 | 優先度 | 状況 |
|---|---|---|---|
| FR-2.16.1 | 右サイドバーとして表示 / 非表示を切り替えられ、左端ハンドルのドラッグで幅を調整できる | M | ✅ 2026-06-12 |
| FR-2.16.2 | 内部タブで表示ビューを切り替える（現状 tmux / git。将来 git graph が git ビューの中身になる） | M | ✅ 2026-06-12 |
| FR-2.16.3 | 表示 / 非表示・幅・ビュー切替は dispatch に乗せ、`tako panel` / MCP `tako_panel` から操作できる（開発不変条件。計 20 ツール） | M | ✅ 2026-06-12 |
| FR-2.16.4 | **下部ステータスバー**（Zed / VSCode 風）を新設: **左にファイルツリー（FR-3.1）のトグル**、**右に tmux 管理・git 管理のトグルボタン**を置き、押すと各サイドバー（左 = ファイルツリー / 右 = 情報パネルの該当ビュー）をトグル表示する。上部タブバーの「◧ panel」ボタンは**廃止**しステータスバーへ集約する | M | ✅ 2026-06-12 |
| FR-2.16.5 | ステータスバーの各トグル状態（ファイルツリー表示 / パネル表示・選択ビュー）は dispatch に乗せ、CLI / MCP から**取得・操作**できる（開発不変条件。**ファイルツリーのトグルを含む** — 従来は cmd+B のみで CLI / MCP 経路が無かった） | M | ✅ 2026-06-12 |
| FR-2.16.6 | **パネル内部タブの 1 本化**: 中身が表示される現 agents ビューを「**tmux**」へリネームし、空表示バグのある旧 tmuxview を削除して統合する。統合後の理想表示 = tako の**タブごとに「タブ名ラベル付きの四角枠」で囲い、枠内にそのタブの全ペインを並べる入れ子表示**。**2026-07-26 改称（#553）**: このビューは #217 の UI 刷新で GUI 上「**fleet**」になっていたが CLI / MCP の値は `tmux` のままで食い違っていた。**正式値を `fleet` に統一**する（GUI のタブ表示名 = `tako panel --view` / `tako_panel` の値。AI が画面表示から操作値を推測できることが設計原則 5 の前提）。旧称 `tmux` は既存スクリプト互換のため受理し続けるが、応答 JSON・案内文には出さない | M | ✅ 2026-06-12 / 2026-07-26 改称 |
| FR-2.16.7 | 統合ビューの各ペイン行の右に**ゴミ箱ボタン** →「kill していいですか?」**確認 → kill**（dispatch 経由）。行が横幅を超える場合は**折り返しまたは省略（…）で見切れさせない**。tmux セッション列挙が正しく動くことを保証する（**旧 tmuxview の空表示バグの解消**を含む） | M | ✅ 2026-06-12 |
| FR-2.16.8 | 統合 tmux ビューに**どのタブにも表示されていないものを検知・表示するセクション**を設ける。対象は (a) **tako 管理外**の tmux セッション（ユーザーが直接立てたもの等）と (b) **kill 漏れ?** = tako から起動されたが対応ペインを失った orphan バックエンドセッション。両者をラベルで見た目区別し、どちらも**確認つき kill** ができる（FR-2.18 の自動サーフェスとは別物 = こちらは可視化と手動 kill） | M | ✅ 2026-06-12 |
| FR-2.16.9 | **タブ内 attach 済みセッションのタブ紐付け表示**: tako ペイン内で `tmux attach` されている外部 tmux セッション（例: orchestrator の master セッションを別サーバーから attach）は「管理外」へ落とさず、attach クライアントの tty と tako ペインの tty 突き合わせで**該当タブの枠内に紐付けて表示**する（セッション名 + window 一覧。window 単位の確認つき kill も可能）。**2026-06-15 二重化解消**: セッションを独立ブロックで重複表示せず、**ホストペイン行の下にインデントして入れ子表示**し（ホスト行が「どのペインが attach しているか」を示すので「ペイン N で attach 中」は省く）、ホスト行の detail にセッション名を出して **1 セッション = タブツリー上 1 箇所**に統合する。CLI / MCP は既存 `tako tmux list` / `tako_tmux_list` の `clients[].tab/pane` で同じ対応情報を取得できる（新規操作なし = 開発不変条件充足） | M | ✅ 2026-06-13 / 2026-06-15 統合 |
| FR-2.16.10 | **tmux セッションの D&D 取り込み**: 統合 tmux ビューに並ぶセッション（タブ枠内の attach 済み・管理外・kill 漏れ? のすべて）を**ドラッグ&ドロップで現在のタブ内へ取り込み、ドロップ位置のペインを分割して表示**できる。実体は新ペインで `TMUX= tmux [-L socket] attach-session` クライアントを起動する（= ネスト attach。多重 attach は tmux の通常動作として許容）。**ドラッグ中はドロップ先ペインのハイライト + 挿入プレビュー必須**: カーソル位置の象限（上下左右）から分割方向を判定し、「ドロップしたら新ペインが占める半面」を強調表示してラベルで結果を示す。同等操作は dispatch `TmuxOpen` + CLI `tako tmux open` + MCP `tako_tmux_open`（開発不変条件） | M | ✅ 2026-06-13 |
| FR-2.16.11 | **orphan tmux セッションの一括クリーンアップ**: 前回クラッシュ等で取り残された tako 由来の裸のバックエンドセッションを掃除する。**起動時に自動実行**するほか、同等操作を dispatch `TmuxCleanup` + CLI `tako tmux cleanup` + MCP `tako_tmux_cleanup`（開発不変条件）で公開する。対象は対象サーバー上の **`tako-` プレフィックス・detached・グループに生きた仲間がいない・protected 外**のみで、使用中（attached）・表示中ビュー・ユーザーの実セッション（既定サーバー・非 `tako-` 名）には構造上一切触れない。**「表示中ビューがあるか」は `#{session_group_size}` > 1 で見る（#1188）**: tmux のセッショングループは**メンバーが 1 つになっても残る**ので、`session_grouped` を代用すると `tako tmux open` で 1 度取り込んだセッションが以後永久に掃除対象から外れる（実測 tmux 3.6b: ビュー kill 後も `grouped=1` のまま `group_size=1`）。**起動時の自動実行のみ第四のガード**として、最終アクティビティが直近 1 時間以内のセッションは kill しない（Issue #113: layout.json が多重起動事故で巻き戻った場合に protected から漏れる実行中 worker を巻き込まないための猶予。明示操作は従来どおり全対象）。**多重起動時の見送りはソケットの所有者で決め、理由を必ず返す（#1187）**: 明示操作が見送るのは「対象 tmux ソケットを**共有する**別 tako-app が生きている」ときと「相手のソケットを特定できない」ときだけ（隔離インスタンスは別ソケットなので本番の掃除を止めない）。起動時の自動実行は従来どおり別 tako-app が 1 つでも生きていれば見送る。応答は `{socket, killed, skipped, detail}` で、`skipped` に理由コード（`peer_shares_socket` / `peer_socket_unknown` / `other_tako_running` / `persist_disabled` / `secondary` / `backend_not_persistent`）と根拠（pid・ソケット名）が入り、同じ 1 行が persist.log にも残る。`--socket` / `socket` は対象サーバーとして**実際に効く**（それまで dispatch が捨てていた） | M | ✅ 2026-06-15（猶予は 2026-07-08 / 理由と `--socket` は 2026-09-09） |
| FR-2.16.12 | **子の表示分類（表示中 / バックグラウンド）**: 統合 tmux ビューの各ペイン行に、前面表示中か裏で実行中かのバッジを付ける。**foreground（表示中）= アクティブタブ所属（タイル表示で画面に出ている）**、**background（バックグラウンド）= 非アクティブタブ所属または退避中**。バックグラウンドの子は親（タブ枠 / 由来タブ）が表示位置で明示される。状態取得は `tako list` / `tako_list_panes` の各ペイン `surface`（`foreground` / `background`）で AI からも可能（開発不変条件。退避ペインは `tako_shelved_list` の `surface` が常に `background`） | M | ✅ 2026-06-15 |
| FR-2.16.13 | **バックグラウンド行のホバープレビュー**: 統合 tmux ビューの**バックグラウンド行**（前面に出ていないペイン）をホバーすると、そのペイン / ターミナルの**実画面サムネイル**（`terminal_screen_lines` をクリップ表示）をマウス位置の左側にポップアップ表示する。**ソース端末はリサイズしない**（バックグラウンドのプログラムを乱さない）ため現在グリッドのクリップになり、**ライブ更新**（`on_term_event` の出力ごとの再描画）で常に最新を映す。ポップアップは読み取り専用（ピン留め操作は行 / カード側のボタン = FR-2.16.15）。前面表示中の行はペインエリアで見えるため対象外 | M | ✅ 2026-06-15 |
| FR-2.16.14 | **タブ枠の折りたたみ（バックグラウンド + 退避を隠す）**: タブ見出しの逆三角トグル（▾/▸）で各タブ枠を折りたたむ。折りたたむと、そのタブ配下の**バックグラウンド項目（裏で実行中のペイン行 + 退避行）を隠し、前面表示中（アクティブタブ）の行は残す**（非アクティブタブは全行がバックグラウンドなので見出しのみになる）。折りたたみ状態は**タブ並べ替え / クローズに強い `TabId` キー**で管理し、`layout.json` に永続化（再起動後も保たれる）。状態取得・操作は `tako list`／`tako_list_panes` の各タブ `collapsed` と、`tako collapse [--tab N] [on/off]`／MCP `tako_collapse_tab`（`collapsed` 省略でトグル）で AI からも可能（開発不変条件） | M | ✅ 2026-06-15 |
| FR-2.16.15 | **プレビューのピン留め（ライブ更新ウィンドウ）**: ホバープレビュー（FR-2.16.13 / FR-2.16.16）の対象を、行 / カードの **📌 ボタン**で**アプリ内フローティングウィンドウ**として常駐させる。ピンウィンドウはタイトルバー D&D で移動、× で解除でき、中身は**ライブ更新し続ける**（`on_term_event` の再描画）。OS マルチウィンドウは使わず（Windows 対応必須要件のため）アプリ内オーバーレイで実現。複数ピン可。状態取得・操作は `tako list`／`tako_list_panes` の `pinned`（`kind` = `pane`/`group` + `id` + `x`/`y`）と、`tako pin [--pane N \| --group-tab N] [on/off]`／MCP `tako_pin_preview`（`pinned` 省略でトグル）で AI からも可能（開発不変条件）。ピンはランタイム状態で再起動はまたがない | M | ✅ 2026-06-15 |
| FR-2.16.16 | **閉じたタブグループのホバープレビュー**: 「閉じたタブ」グループ（FR-2.15.6 で由来タブごとに分離済み）のカードをホバーすると、**そのグループの全退避ペインを均等高で縦に積んだ一括プレビュー**（各ペインのラベル + 実画面サムネイル）をポップアップ表示する。FR-2.16.13 と同じくソース端末はリサイズせずライブ更新。閉じたタブごとの分離管理自体は FR-2.15.6（✅ 2026-06-15）で実装済みで、本項目はその**グループ単位プレビュー**を追加するもの。ピン留め可（FR-2.16.15） | M | ✅ 2026-06-15 |
| FR-2.16.17 | **tmux サーバー（ソケット）単位の回収**: 隔離起動（`TAKO_ISOLATED=1`）やテストが立てた tmux サーバーは 1 本ごとに別ソケットなので、セッション単位の掃除（FR-2.16.11）では**構造的に届かない**（実測 2026-09-09: 生存 93 本 / 388 MB / 最古 11 日 / 残骸ソケット 1440 個）。`tako tmux cleanup --servers`（dispatch `TmuxCleanup{servers}` + MCP `tako_tmux_cleanup`）で**ソケット置き場ごと**回収する。**既定は dry-run**（1 本ずつ「所有 pid / 生死 / セッション数 / attach 数 / 最終更新 / サイズ / 判定理由」を返すだけ）で、実際に kill / ソケット削除を行うのは `--apply` を明示したときだけ。判定は**名前ではなく所有プロセスの生死**（自分の backend → 既定サーバー → 生きた tako-app が使っている（#1187 の env 復元で名前が pid と一致しなくても当たる）→ tako 由来でない → サーバー不在 → attach 中 → 名前から所有 pid を特定できない → 所有 pid が生きている → 出来たて（60 秒）を順に除いた残りだけが回収可）。kill の直前に attach の有無を測り直す。**溜めない側**として、`tako-iso-<自分の pid>` / `tako-st-<自分の pid>` の使い捨て backend は終了時（`on_app_quit`）に自分で kill する（明示指定の `TAKO_TMUX_SOCKET` は再起動をまたぐ検証があるので触らない = #770） | M | ✅ 2026-09-09 |

実装メモ（FR-2.16.4〜2.16.8。2026-06-12 仕様化・同日実装）:

- git 管理トグルの表示先は将来の git graph（FR-3.6 = FR-2.16.2 の git ビューの中身）。
  実装まではプレースホルダビュー（`render_git_view`）を表示する
- ステータスバーのトグル操作は既存 `Request::Panel` の拡張として dispatch に載せた:
  `filetree` パラメータ追加（`tako panel --filetree on/off` / MCP `tako_panel` の
  `filetree`。応答 JSON にも `filetree` を含む。新ツールは増やさず計 20 のまま）。
  パネルビューの wire 値は `tmux | git`（`agents` は廃止）
- 統合 tmux ビューのデータ層は `tmux_view_groups()`（タブ枠。FR-2.16.9 の attach 済み
  セッション紐付け = `tmux_sessions_attached_to()` を含む）+
  `tmux_unlisted_sessions()`（FR-2.16.8 の分類。attach 済みは除外）として表示と分離
  （FR-2.13.5 の継承）。ペイン行の kill は dispatch `Close`、attach 済み / 未表示
  セッションの kill は dispatch `TmuxKill`
- kill 確認 UI は**メッセージ行（折り返し）+ ボタン行の縦積み**（`render_kill_confirm`）。
  flex_row 一列だと長い確認文言でボタンごとパネル右端へ見切れる（2026-06-13 実機バグ②）
- localhost ポートパネル（FR-2.19）もこの情報パネルのビューとして追加予定

実装メモ（tmux ビュー取り込みの orphan / 無限ネスト根治。2026-06-15）:

- **表示用ラッパーは「元」と分離して追跡する**。`TmuxOpen` の grouped session
  `tako-view-<元>-<paneId>` は表示専用の使い捨て。`TmuxViewTarget` は監視・再 attach
  対象の**元セッション**（`session`）と**ラッパー名**（`wrapper: Option`）を別々に持ち、
  ペイン close 時に **kill するのはラッパーだけ**（`drop_tmux_view_session`。`tako-view-`
  接頭辞ガードで元セッションを誤爆しない）。直接 attach した復帰経路は `wrapper = None` で
  「close 時も kill しない」になる。旧実装は元セッション名を登録していたため①ラッパーが
  orphan 化②閉じると**ユーザーの実セッションを kill**しかねない二重バグだった
- **無限ネストの根治**（`tako-view-tako-view-...`）: `TmuxOpen` は開く前に tmux の
  `session_group`（= 最初に作られた元セッション名）へ正規化する。ラッパー名や grouped
  session を渡されても group を辿れば必ず元へ戻るため再ラップが起きない。さらに名前が
  `tako-view-` で始まる「ラッパーの開き直し（退避からの復帰等）」は**新しいラッパーを
  作らず元セッションを直接 attach** する
- 起動時 orphan 掃除（FR-2.16.11）は復元で `backend_sessions` が出揃った後に実行。
  protected = 現存ペイン・退避ペインの backend 名 + 表示中ビューの元/ラッパー名。
  **グループに生きた仲間がいるものと attached は対象外**にすることで、protected の
  構築タイミングに依らず使用中セッションを誤爆しない三重ガードにしている。
  行のパースと orphan 判定は `tako_core::tmux_cleanup`（`LIST_FORMAT` /
  `parse_session_row` / `is_orphan`）の 1 実装で、掃除（`cleanup_orphans`）と
  発見（`find_orphans` = 起動時の自動復帰）が同じ規則を見る（#1188 まで同じ規則が
  2 か所に写経されていた）。用途の違いは `OrphanPurpose` だけ
  （発見はラッパーを対象にせず attached も見ない = 前のインスタンスは死んでいる）
- 多重起動時の見送りの判定は `tako_core::tmux_cleanup`（#1187）。**プロセス名ではなく
  ソケットの所有者**を見る: 生きている別 tako-app の**初期環境**（macOS は
  `KERN_PROCARGS2` = `ports::process_env_vars`）から `TAKO_TMUX_SOCKET` /
  `TAKO_ISOLATED` / `TAKO_SELF_TEST` を読み、一括隔離と同じ順序（`resolve_socket_name`）で
  相手の backend ソケット名を復元して対象と突き合わせる。**読めなければ「不明」= 見送る**
  （安全側）。ソケット名の生成（`tako-iso-<pid>` / `tako-st-<pid>`）は core と main.rs で
  1 実装（ズレると所有者判定が静かに外れるので番犬
  `crates/tako-control/tests/tmux_cleanup_watchdog.rs` が落とす）
- `--socket` で自分の backend 以外を対象にしたときの protected は
  **そのソケットのビューだけ**（`tmux_view_panes` の `socket` で絞る）。
  自分の backend セッションは他サーバーには存在しないので混ぜない

### ペインタイトルバー（FR-2.1.3 の表示更新。2026-06-12 実装）

各ペインの上部に iTerm2 風のタイトルバーを常設: 左に × ボタン（close）、状態ドット
（FR-2.1.4）、ペイン名（手動 / AI リネーム > role > OSC タイトル）。従来の右上バッジ
オーバーレイはタイトルバー表示に置き換えた。

### FR-2.17 ネスト tmux の検出・診断・ワンタップ設定適用（2026-06-12 要件化）

> ユーザーが自前の tmux セッションを tako ペイン内で attach する構成（ネスト tmux）は
> tako の主要ユースケース（エージェント集約監視）そのものだが、ユーザーサーバーが
> 既定値（`mouse off` + `extended-keys off`）のままだと、**ホイールのスクロールバック
> 遡りと Shift+Enter（CSI u）がネスト境界で握り潰される**（2026-06-12 実機バグ (1)(4) の
> 根因。tako 側の実装だけでは原理的に直せない）。tako はこれを検出して案内する。
> **勝手に書き換えない。「案内 + ワンタップ適用」**（ユーザー指示）。

| ID | 要件 | 優先度 |
|---|---|---|
| FR-2.17.1 | ペイン内のネスト tmux クライアント（ユーザー自前サーバーへの attach）を検出する（tty 突き合わせ。tmuxview FR-2.13 の基盤を流用） | M |
| FR-2.17.2 | 検出したサーバーの設定がネスト透過に不足している場合（mouse / extended-keys / copy-mode-position-format）、診断結果を情報パネル等で表示する | M |
| FR-2.17.3 | 必要な conf 変更の内容を提示し、ユーザーがボタン一発で安全に適用できる（`~/.tmux.conf` への追記 + 稼働サーバーへの `source-file`。適用前に差分を見せる） | M |
| FR-2.17.4 | 検出・診断・適用は MCP / CLI からも可能にする（開発不変条件。AI がセットアップを代行できる = FR-2.14 のオンボーディング思想と同系） | M |
| FR-2.17.5 | 推奨スニペットはコード側の単一定義（`tmux_backend::NESTED_TMUX_SNIPPET`）を正とし、ネストチェーンの e2e（ホイール / CSI u）で品質を保証する | M |

実装メモ（2026-06-12 調査の結論）:

- 必要設定は `set -g mouse on`（外から転送される SGR ホイールをネスト側 copy-mode に
  乗せる）/ `set -s extended-keys always` + `terminal-features 'tmux*:extkeys'`（CSI u）/
  `set -gq copy-mode-position-format ''`（右上の時刻インジケータ抑止）
- **extended-keys は `on` では足りない**: tmux 3.6 はペインからの kitty keyboard 要求
  （`\e[>1u`。Claude Code が使う）を認識せず pane_key_mode が VT10x のままになる
  （modifyOtherKeys 形式 `\e[>4;2m` のみ認識）。`always` でモード 1 を強制する。
  また `terminal-features` の extkeys 明示が無いとネスト側が外からの CSI u 入力を
  解釈せず**捨てる**（素の Enter 劣化ですらなく無反応になる）
- バックエンド側 conf の `extended-keys always` も同じ理由（両者が揃って初めて
  チェーンが通る）
- スクロールは方式転換（2026-06-12 夜）で `tako-core::scroll` が copy-mode を直接
  駆動するため、**ネスト先の `mouse on` はホイール遡りの必須条件ではなくなった**
  （ネスト内のマウス要求アプリ = vim 等への SGR 転送には引き続き必要なので
  スニペットには残す）
- 適用済みでも**適用前から起動していた内側アプリ**には効かない（kitty 要求は起動時
  1 回のため）。診断表示にはこの注意書きを含めること

### FR-2.18 表示されていない子の自動サーフェス（2026-06-12 要件化）

> エージェントが spawn した子エージェントや dev サーバーが、**どのタブにも表示されない
> まま裏で動き続ける**と、ユーザーはその存在に気づけない（エージェント集約監視の
> コンセプトに反する）。tako は「未表示の生きた作業」を検知し、自動で見える場所に出す。

| ID | 要件 | 優先度 |
|---|---|---|
| FR-2.18.1 | spawn された子エージェント・サーバー（バックエンドセッション等）が、どのタブのどのペインにも表示されていないことを検知する | S |
| FR-2.18.2 | 未表示の子を tako が**自動で表示**する（spawn 元と同タブ内の分割等。表示位置・タイミングのポリシーは実装時に設計） | S |
| FR-2.18.3 | 「**未表示の子一覧**」「**指定した子を今のタブに表示**」を MCP / CLI に公開する（開発不変条件。AI が自分の子を見せに来られる） | M |
| FR-2.18.4 | たまり場（FR-2.15）との整合: たまり場へ**意図的に退避**されたターミナルは自動サーフェスの**対象外**。本 FR の対象は**意図せず**未表示になっているもの。一覧では両者を区別して見せる | M |

実装メモ（着手時に設計する）: 検知の土台は Phase 5.5 の「表示ペインを持たない生きた
バックエンドセッション」（orphan）+ tmuxview の tty 突き合わせ（FR-2.13）。
たまり場実装後は「退避済みフラグ」で FR-2.18.4 の区別をつける。

### FR-2.19 localhost ポート使用状況パネル（2026-06-12 要件化）

> dev サーバーの**消し忘れに気づける**ようにし、AI が**サーバー起動前にポートの空きを
> 確認できる**ようにする。FR-2.4.2 の listen ポート検知を、右サイドバー情報パネル
> （FR-2.16）のビューとして可視化する。

| ID | 要件 | 優先度 |
|---|---|---|
| FR-2.19.1 | localhost の listen ポート使用状況を情報パネルのビューとして一覧表示する: **ポート / プロセス名 / PID / 対応する tako ペイン / tako 管理外**（外部ターミナル等のプロセス）**の区別** | S |
| FR-2.19.2 | 一覧から**確認つき**で該当プロセスを kill できる | S |
| FR-2.19.3 | 「**使用中ポート一覧**」「**空きポート提案**」「**指定ポートの kill**」を MCP / CLI に公開する（開発不変条件。AI は起動前にポート確認 → 空きポートで起動、ができる） | M |

実装メモ（着手時に設計する）: データ源は `tako-core::ports`（FR-2.4.2、libproc +
tty 突き合わせ）。現状はペイン配下のみ検知のため、**tako 管理外プロセスの列挙**
（システム全体の LISTEN 走査）が追加実装点。kill は確認 UI + dispatch 経由
（FR-2.13.3 の tmux kill と同型）。

### FR-2.20 worker spawn のレイアウトエンジン（✅ 2026-07-13、#165）

> spawn の「呼び出し元の右に等分割」を繰り返すと worker が増えるほど全ペインが
> 横に圧縮され、master も worker も読めなくなる。**master を見やすく保ちつつ、
> worker を右側の「worker 領域」に賢く配置する**レイアウトエンジンに差し替える。

| ID | 要件 | 優先度 |
|---|---|---|
| FR-2.20.1 | spawn 配置ポリシーを設定で切替できる: `master-reserved`（**既定**。spawn 元 = master の取り分を維持し、worker は master 右の worker 領域内に配置）/ `legacy`（従来の右等分割） | M |
| FR-2.20.2 | worker 領域内の配置アルゴリズム: `grid`（**既定**。1 体=全面 → 2 体=上下 → 3〜4 体=十字四分割 → 以降は列を増やす格子）/ `spiral`（縦横交互に半分ずつの再帰分割、黄金比風） | M |
| FR-2.20.3 | 設定は config.yaml の `spawn_layout` セクション（policy / master_ratio 0.1〜0.9 / algorithm）。取得・変更を CLI `tako orchestrator layout` + MCP `tako_orchestrator_layout` で 1:1 公開する（開発不変条件。計 59 ツール） | M |
| FR-2.20.4 | **worker 領域の判定は origin / spawned_by による**: 領域 = spawn 由来ペイン（`spawned_by` チェーンが spawn 元に到達）だけのサブツリー。ユーザーが手動で開いたペインが混在するサブツリーは領域と見なさず再構築しない（**master とユーザー由来ペインの矩形を勝手に潰さない**） | M |
| FR-2.20.5 | worker close 時（dispatch `Close` / UI ×・exit）に空いた場所を残り worker で再配分する。リフローは worker 領域内に限定し、master・ユーザー由来ペインの矩形は不変 | M |
| FR-2.20.6 | master / solo のデフォルト system prompt に「レイアウト操作時は master とユーザー由来ペインの可読性を最優先する」行動規範を持つ | S |
| FR-2.20.7 | **worker ペイン 1 枚に最小幅（桁数）を保証する**（#1132 → #1439 で確保の仕方を変更）。worker は**必ず spawn 元と同じタブへ置く**（別タブへ逃がす配置は廃止）。同じタブで `min_worker_cols`（既定 60 / 0 = 保証しない / 20〜400）を割るなら、**その worker 領域のペインのフォントを自動で縮めて**桁数を確保する。床（`min_worker_font_scale`。既定 0.6 = 既定サイズの 60% / 0.4〜1.0）まで縮めても届かなければ床のサイズで置き、spawn 応答に `cols_short=true` と実桁数を載せる。結果は spawn 応答（`tab` / `placement`（常に `same_tab`）/ `placement_reason` / `pane_cols` / `min_worker_cols` / `font_scale` / `font_size` / `cols_short` / `font_refit`）に載る。設定は config.yaml の `spawn_layout.min_worker_cols` / `auto_shrink_font` / `min_worker_font_scale` で、CLI `tako orchestrator layout --min-worker-cols / --auto-shrink-font / --min-worker-font-scale` + MCP `tako_orchestrator_layout` の同名パラメータで 1:1 公開する | M |
| FR-2.20.8 | **master の引き継ぎ（#749 / #917）の後も、後任 master の worker は worker 領域へ入り、master の取り分（`master_ratio`）を割らない**（#2133）。後任（master の role・`spawned_by = 前任`）は、前任 master（生きている master の role のペイン）か閉じたペインまでの spawn 元を同じ持ち主として扱い、前任の worker を自分の worker と見なす。master の role のペインは worker に数えない。前任 master を閉じたとき（dispatch `Close` / UI ×・exit）は、前任を指していた worker の `spawned_by` を後任へ付け替え、後任の `spawned_by` を前任の spawn 元へ繋ぎ直す（引き継ぎを重ねても切れない）。後任がちょうど 1 枚に決まらなければ付け替えも読み替えもしない。A/B は `TAKO_2133_LEGACY=1`。新しい操作は増やさない（既存の spawn / close / handoff の結果が変わるだけ） | M |

実装メモ（2026-07-13）: レイアウト計算は `tako-core::spawn_layout`（ポリシー型 +
領域構築の純関数）と `PaneTree::spawn_worker` / `reflow_workers`（領域判定 + 再構築）。
spawn 経路は `dispatch_orchestrator_spawn`、close リフローは dispatch `Close` と
tako-app `remove_pane_with` の両方から `reflow_workers` を呼ぶ。設定読み込みは
`tako-control::setup::spawn_layout_config()`（不正値は既定へフォールバックし spawn を
止めない）。機械検証は tako-core 単体テスト（rect 検証 10 本）+ セルフテスト項目 72
（dispatch 経由 spawn 1→4 → close リフロー → ユーザーペイン不変）。

実装メモ（2026-09-04。FR-2.20.7 = #1132）: **狭いペインでは claude TUI が
メッセージをハード折り返しする**ので、上限の見出し（#1123）・選択肢ダイアログ・
報告の読み取りが同時に壊れる（実測: master + worker 7〜8 体で worker が 18〜25 桁。
解除後 7.5 時間 worker 4 体が止まったまま）。master-reserved の「master の取り分を
守る」は効いていたが、**worker 領域の 1 ペインの下限幅**が無かった。

- 見積もりは 2 段: 「worker を 1 体足したときにいちばん狭い worker がタブ幅の何割に
  なるか」は `tako_core::spawn_layout::prospective_narrowest_worker_share`（**純関数**。
  `build_even` / `build_spiral` の写しで、実際に木を組んで `PaneTree::layout` と
  突き合わせるテストが拘束する）。**桁数への変換だけ**がセル幅・枠と余白という実測に
  依るので `ControlHost::pane_cols_for_width_fraction`（GUI だけが答えられる）へ分けた
  （GUI の式は `pane_text_area_rect` + `grid_cells` と同一。デバイスピクセル丸めのぶん
  最大 1 桁ずれることを番犬テストが固定する）
- 領域の探索は `PaneTree::worker_area_panes`（読み取り専用）で、再構築
  （`rebuild_worker_area`）と**同じ領域**を指すことを単体テストが固定する
- 既定 60 桁の根拠は 2 つ: ①tako が読む最長の通知（111 文字）が 60 桁なら 3 行以内に
  収まり、折り返しの結合（`unwrap_wrapped_lines`）の予算に十分収まる（実害が出た
  21〜25 桁では 5〜7 行に割れる。テストで固定）②実測のタブ（約 124 桁）で master の
  隣に worker 列が 1 本残る上限（これより大きくするとノート PC でどの worker も
  同タブに置けなくなる）
- `spawn_worker`（tako-core）は下限を見ない。配置の決定は dispatch の
  `plan_worker_placement` に閉じているので、#165 の rect テストは 1 ビットも変わらない

実装メモ（2026-09-13。FR-2.20.7 の**確保の仕方の変更** = #1439）: #1132 の
「別タブへ逃がす」は下限幅を守れていたが、**1 グループ = 1 タブで集約監視する**
という tako のコンセプトを壊した（実測: 8 体 spawn でタブが 8 枚に散り、master の
タブから worker が 1 枚も見えない）。ユーザー指示（2026-09-13）は
「見切れそうなら子ペインの文字サイズを小さくする。タブ分けはマジでしない」。

- 配置は**常に `same_tab`**。別タブ配置（`new_tab` / `overflow_tab`）のコードは
  A/B の対照として `TAKO_1439_LEGACY=1` のときだけ通る（腕は `PlacementArm` の型で
  表し、env の読み取りと判断を分けてある = 3 腕を単体テストから決定的に回せる）
- 桁数の確保は `tako-control::worker_font::refit_worker_area` の 1 実装。
  **worker 領域まるごと**を当て直す（grid は 1 体足すと既存の列も細くなるので、
  新しいペインだけ縮めても残りが下限を割ったまま残る）。呼び口は spawn 直後・
  close 後のリフロー・復元 / リサイズ（tako-app の render から取りこぼしぶんだけ）
- 段の選び方（5% 刻み・床で止める・届かなければ床 + `cols_short`）は
  `tako_core::spawn_layout::fit_worker_font`（**純関数**）。「その倍率で何桁入るか」は
  `ControlHost::pane_cols_for_width_fraction(tab, fraction, font_scale)` として
  GUI が答える。GUI は**段ごとのセル幅を実測**して控える（`refresh_font_scale_cells`。
  倍率を線形に割ると実フォントの丸めと 8pt クランプでずれる）
- 床 0.6 の根拠: 既定 13pt に対して 7.8pt = ペイン単位ズームの下限（8pt）とほぼ同じ。
  これより小さい倍率を許しても実セル幅は 8pt で頭打ちになり**桁数は増えない**
- 戻し方は自動。worker が閉じて幅が戻れば当て直しで倍率 1.0 が選ばれ、縮小が外れる。
  **ユーザーが cmd +/- で手で決めたペインは触らない**（`auto_font_panes` で出自を分ける）
- 限界: 同じタブに 8 体入れると 119 桁のタブでは worker が約 17 桁 → 床まで縮めても
  約 28 桁で下限 60 桁に届かない。**これは方針上避けられない**ので、spawn 応答の
  `cols_short=true` と理由文（ウィンドウを広げる / worker を減らす /
  `--min-worker-cols` を下げる）で master へ伝える

### FR-2.21 アプリ内更新の通知と専用画面（✅ 2026-07-28、#616。基盤は #36 / #403 / #50 / #595）

> 更新表示は当初ステータスバーのバナー + ドロップダウンだったが、下部バーの表示物が
> 増えて置き場が無くなった（2026-07-27 ユーザー要望）。**下部バーから撤去し、
> 「気づかせる場所」と「操作する場所」を分ける**: 通知は画面上部のカード、
> 内容と操作は設定画面と同じ独立ウィンドウの専用画面に置く。

| ID | 要件 | 優先度 |
|---|---|---|
| FR-2.21.1 | ステータスバーにはアップデート関連の表示を**一切置かない**。更新状態（`UpdateState`）を status_bar が参照しないことを機械検査で固定する（読まなければ描けない = 構造的保証） | M |
| FR-2.21.2 | **上部通知カード**: 更新を検知したとき（自動チェック・起動時・手動チェック）にタブバー直下へ全幅のカードを出す。「詳細を見る」（専用画面へ）と「×」（閉じる）を必ず持ち、**自動では消えない**。ターミナル出力を覆わないようオーバーレイにせず、レイアウトに積む（ウェルカムバナー FR-2.14.10 と同方針） | M |
| FR-2.21.3 | × で閉じた抑止は**案内していたバージョン単位**。キーは両チャンネルを含む（例 `stable:0.6.1 test:0.7.0-test.1`）ので、片方でも新しい版を検知すればカードは出直す。抑止は settings.json の `update_card_dismissed` に永続化し、**再起動をまたいでも同じバージョンでは出ない** | M |
| FR-2.21.4 | **専用画面**（独立 GPUI ウィンドウ。設定画面 FR-4 / #459 と同じ方式）に、現在バージョン / 現在チャンネル / 配布系統（homebrew / zip / broken-brew / windows-installer / windows-portable）/ 実行環境（platform・arch・掴む配布物の命名規則。#595）/ チャンネル別の最新版と `asset_name` / リリースノート（`/releases` 応答の `body`。追加リクエスト無し）/ リリースページ導線 / 「今すぐ更新」を載せる | M |
| FR-2.21.5 | 更新フローの**全状態**（テスト版警告・更新確認・進行・完了・失敗・brew 失敗の zip フォールバック・チェック失敗）は専用画面に出す。ステータスバーへ戻る経路は残さない。tako メニュー・About の「アップデートを確認」も結果の行き先として専用画面を開く | M |
| FR-2.21.6 | 自動チェックの結果でユーザーの確認・更新フロー（TestWarning / ConfirmPending / Updating）や未読の失敗表示を上書きしない（`UpdateState::is_replaceable_by_check`） | M |
| FR-2.21.7 | 専用画面を開く操作とカードの状態照会・閉じる・出し直しを CLI `tako update open` / `tako update card [dismiss\|show]` と MCP `tako_update`（action = open / card / card-dismiss / card-show）へ 1:1 公開する。⌘K パレットにも「アップデートを開く」を常設し、画面が開いているかは `tako update status` の `window_open` で分かる（開発不変条件） | M |
| FR-2.21.8 | リリースノートは **Markdown としてレンダリング**する（✅ 2026-07-31、#690）。ノートの生成元は `scripts/release.sh` のノート生成（#594）が作る md（見出し・ダウンロード表・リスト・リンク・コード・日英併記）なので、生テキストでは読めない。描画は**プレビューペイン（FR-3.3 / FR-3.3.1）と同じ実装**を通し、見た目の一貫性を構造で担保する。リンクは ⌘+クリックで既定ブラウザ（FR-3.3.2 / #680 と同じ規則 = http / https のみ）。ノートが空のリリースは案内文へ落とし、壊れた md・極端に長いノートでも描画が落ちない | M |
| FR-2.21.9 | **Windows の適用**（#2434）: インストーラー（`tako-<tag>-windows-x86_64.exe`）を取得元（tako の GitHub Releases の URL）・大きさ・sha256（API の asset の `digest`。示されていなければ断る）・PE ヘッダで検査してから `/SILENT /SUPPRESSMSGBOXES /NORESTART /CLOSEAPPLICATIONS /LOG=… /TAKOWAITPID=… /TAKORELAUNCH=…` で起こし、tako は layout を保存して正規の終了経路で抜ける。インストーラーは tako の pid が消えるのを待ってから上書きし、終わり際に成功なら新しい版・失敗ならロールバックした今の版を起動し直す（`tako.iss` の `InitializeSetup` / `DeinitializeSetup`）。取得・検査・起動の失敗では何も上書きせず理由とリリースページを返す。インストーラーで入れた先の外から動く tako（ポータブル版・開発ビルド）は断る。CLI / MCP の `apply` は `installer_started` / `quitting` / `installer_log` を返して約 1 秒後に抜ける（`updated` は false = まだ上書きしていない）。`apply-zip` は macOS 専用で、Windows では断る | M |

実装メモ（2026-07-28）: UI は `tako-app/src/update_window.rs`（`UpdateWindow` = 専用画面、
`TakoApp::render_update_card` = 通知カード、更新フローの実体もここ）。表示判定は
`update_checker::card_key` / `card_should_show`（純関数・単体テスト付き）。永続化は
dispatch 側（`crate::settings`）で行い、`TAKO_SELF_TEST` 中は書かない（Theme / Welcome と同方針）。
機械検証はセルフテスト項目 90（カード表示 → × → 新版で再表示 / settings 往復 /
パレット・dispatch から専用画面が開き更新フロー 9 状態を描画）+ 番犬テスト
`ステータスバーにアップデート表示が残っていない`。

実装メモ（2026-07-31、FR-2.21.8 / #690）: md の**幾何とテーマ色は
`tako-app/src/md_view.rs` の 1 実装**（`render_block`）に集約し、プレビューペインと
専用画面はインタラクション層（選択・検索ハイライト・`TextLayout` の控え・コピーボタン）
だけを `MdTextSink` で差し替える。パースは `preview::markdown_blocks`
（`pulldown-cmark`）、リンク索引は `md_document_links` が正で、いずれもプレビューと共有。
**GPUI の `TextLayout::bounds()` は prepaint 前に呼ぶと panic してアプリごと落ちる**ので、
ヒットテストは「描き終わった世代」だけを対象にする（`ParsedNotes::painted` を
canvas の paint で立てる）。機械検証はセルフテスト項目 90(f)（実描画でブロック数 / 表 /
リンク数 / 行レイアウト数 + ノートなし・壊れた md・巨大ノートのエッジ）と
visual-test「update-notes」節（**実リリース v0.6.2 のノート本文**を流し込み、
scene の実ピクセルで見出しサイズ・インラインコード・コードパネル・表のヘッダ帯・
リンクの accent 色・⌘ホバー差分・テーマ往復を検査）。

### FR-2.22 AI コマンド提案カード（✅ 2026-07-30、#666）

> FR-2.7（AI 成果物プレゼンテーション）のコマンド版。**AI が会話本文に書いたコマンドは
> ユーザーが使えない**: claude 等の TUI は会話をペイン幅に合わせて物理改行して描画するため、
> 画面から選択してコピーすると改行が混入して壊れる。画面出力からの復元は
> 「折り返しか本物の改行か」を区別できないので方式として成立しない
> （2026-07-30 ユーザー要望）。**AI がコマンド文字列そのものをツールで渡し、
> tako がネイティブカードとして出す**ことで根治する。Web 版 Claude の
> 「コピーできるコードブロック」に相当する体験を tako ネイティブで提供する。

| ID | 要件 | 優先度 |
|---|---|---|
| FR-2.22.1 | AI（master / solo / worker）が「ユーザーに実行してほしいコマンド」を渡すと、**対象ペインのターミナル領域の下に作った専用帯**へネイティブカードとして表示する（既定の対象は呼び出し元ペイン = AI が対話しているペイン）。位置の規則は FR-2.22.10。**claude のペインで tako mod が描いているカードは Claude Code の画面（会話の行と入力欄の上の帯）に出し、この帯には出さない**（#1963 = FR-2.42.41〜45。保管と操作はこの節のまま） | M |
| FR-2.22.2 | カードは**論理文字列**を保管する。表示は等幅 + 背景パネルで折り返してよい（狭幅ペインでも全文が読める）が、**コピー・実行に使うのは常に保管した論理文字列**でありペイン幅の影響を受けない。改行を含む複数行コマンドは改行を保持する | M |
| FR-2.22.3 | 「コピー」ボタンで論理文字列をクリップボードへ入れ、成功をボタン表示で返す（アイコンとラベルが「コピーしました」へ変わる） | M |
| FR-2.22.4 | 「新規ペインで実行」ボタンで**同じタブに新しいペインを分割して実行**する（Code Runner FR-3.18 の `spawn_command_pane` 経路を再利用。複合コマンドの包み方は`platform::shell::run_pane_command`（境界 B1）が決める = POSIX は `/bin/sh -c` の引数として渡し#453 の 127 即死を避ける / Windows は PowerShell へ `-EncodedCommand` で渡す。#875）。**対話中のペインには一切書き込まない**。既定でフォーカスも移さない。起動シェルは FR-2.5.14 の規則（ユーザーの対話環境）に従う。ボタン押下自体が明示操作なので追加の確認ダイアログは出さない | M |
| FR-2.22.5 | × でカードを閉じる。1 カードに複数コマンドを載せられ（上限 10 件）、各コマンドが個別のコピー / 実行ボタンと番号ラベルを持つ。1 ペインあたりのカードは上限 3 枚で、超えたら古いものから落とす（提示を重ねても画面が埋まらない） | M |
| FR-2.22.6 | カードは**揮発**（セッション内使い捨て）。レイアウト永続化の対象外で、ペインが閉じたら掃除する | M |
| FR-2.22.7 | コマンド文字列は受け入れ時に正規化・検証する: CRLF → LF、前後の空白のみ除去（内側の改行・インデントは保持）、空文字列は拒否、**改行とタブ以外の制御文字は拒否**（クリップボードとシェルへ渡る文字列にエスケープシーケンスが混入する経路を構造的に閉じる）、1 コマンド 4096 バイト上限 | M |
| FR-2.22.8 | 操作は CLI `tako show-command <コマンド>...`（`--label` / `--pane` / `--list` / `--copy` / `--run` / `--dismiss` / `--card` / `--index` / `--focus`）と MCP `tako_show_command`（action = show / list / copy / run / dismiss）へ 1:1 公開する。**UI のボタンも同じ dispatch を呼ぶ**ので UI 操作と AI 操作の挙動が食い違わない（開発不変条件）。**pane の省略は呼び出し元**（CLI = `TAKO_PANE_ID` / MCP = stdio の `TAKO_PANE_ID`・HTTP の `X-Tako-Pane`）を入口が埋め、呼び出し元も無い show は送らずに断る。card 指定の list / copy / run / dismiss は dispatch がカードの所在ペインを引くので呼び出し元を混ぜない（#1958。番犬 `issue1958_caller_pane_watchdog` がカタログの「省略時は呼び出し元」と MCP の変換の食い違いを全ツールで落とす） | M |
| FR-2.22.9 | master / solo のデフォルト system prompt と worker プロンプト雛形に「ユーザーに実行してほしいコマンドはこのツールで提示する」を行動規範として書く（FR-2.7.5 と同じ流儀。例外 = 自分で実行するコマンド・対話が必要なコマンド（FR-2.5 の `tako_run_interactive`）・実行を求めていない説明中の言及） | M |
| FR-2.22.10 | **表示中はターミナル領域を縮めて作った専用帯にカードを置く**（#703）。①**重なりゼロはレイアウトの不変条件**: カードが 1 枚でもあるあいだ、ペインを「ターミナル領域 + カード帯」の縦 2 段に分ける。帯の高さはテキスト領域（= PTY の行数・マウス座標変換・IME 位置の正）から差し引くので、**カードが会話・入力欄・フッターのどの行にも重ならないことが座標計算ではなく構造で保証される**。オーバーレイ方式（#666 の下端固定 / #681 の内容アンカー）は claude のように画面全体を使う TUI で会話文を隠してしまい実用にならなかった（2026-07-31 ユーザー実使用フィードバック）。②**帯はターミナル領域の下**（claude のフッターの直下）に置き、上辺に区切り線を引く。③**高さは行単位に量子化**する（`rows = floor((領域高 - 帯高) / 行高)` なので px で決めると 1px の測定揺れが行数を跨いだ瞬間に PTY resize を呼び、TUI が画面を作り直す）。④**測り直すのはカード内容とペイン幅から作る署名が変わったときだけ**。コピー成功表示のようにカード内の見た目だけが変わっても PTY resize は起きない（= 出現・消滅・幅変化以外で端末を揺らさない）。高さそのものは実描画から採取する（#684 と同じ prepaint プローブ = `Cell` へ書くだけでエンティティを触らない）。採取前の 1 フレームだけ推定値を使い、採取が走らない経路が生まれても機能が死なないようにする。⑤**上限**はテキスト領域の 34%（カード 1 枚ぶんの下駄 112px つき。**下駄も領域の半分まで**で、半分の高さへ分割したペインで端末を飲み込まない）で、かつ**ターミナルには必ず 3 行残す**。超過したカードは帯の中でスクロールして読む（新しいカードほど上なので、溢れても最新のものが見えている）。1 行も割けない極端に低いペインでは帯を作らず端末を優先する（カードは保管庫に残るので CLI / MCP の操作は効く）。⑥ポート検知チップ（FR-2.4.3）は帯の上へ逃がす。プレビュー / Web ビュー / スターター表示のペインでは帯を作らない（描き先が無い） | M |
| FR-2.22.11 | **コマンドごとの実行記録と再実行の断り**（#1724）: `run` はカードへ「どのペインで実行したか / 実行中・終了コード・終了を見る前にペインが閉じた / 回数」を残し、`list` の `runs`（`commands` と同じ並び・未実行は null）で返す。PC のカードはボタン行に「実行中 / 実行済み（終了コード N）/ 実行済み（ペインは閉じた）」を出し、スマホ（FR-6.24）も同じ記録を読む = **どちらで押しても状態が揃う**。確定は `dispatch::refresh_command_card_runs` の 1 本（実行ペインが消えた = closed / 画面に終了マーカー = exited。判定は `RunInteractiveStatus` と同じ `find_exit_marker`）で、`list` / `run` の直前と GUI の 2 秒ポーリングが呼ぶ。**同じコマンドの前回の実行がまだ走っているあいだの `run` はペインを割る前に断る**（`CommandCardError::StillRunning`。PC のダブルクリック・スマホの押し直し・CLI / MCP の重ね打ちのどれでも 2 本走らせない。終わった / 閉じた実行は何度でも再実行できる）。記録は揮発（FR-2.22.6 のまま・永続化しない） | M |

実装メモ（2026-07-31）: モデルと操作は `tako-core::command_card`（GPUI 非依存。保管・正規化・
上限・解決、帯の幾何 = `band_rows` / `band_height` / `MAX_BAND_RATIO` / `MIN_BAND_HEIGHT` /
`MAX_BAND_SHARE` / `MIN_TERMINAL_ROWS`）、判定と実行は `tako-control::dispatch` の `dispatch_show_command` 1 箇所、
描画は `tako-app/src/command_card_ui.rs`（帯の高さ決定 = `TakoApp::card_band_height`、
実測 = `CardBandProbe`、署名 = `card_band_signature`、初回推定 = `estimate_card_stack_height`）。
**テキスト領域から帯を差し引くのは `render()` のペイン矩形計算 1 箇所**（`new_areas`）で、
`render_pane` は同じフレームの `card_bands` を読んで帯を描くだけ = 2 箇所が食い違わない。
ホスト（GUI）は保管庫の受け渡しとクリップボードの保留キューだけを持つ
（`ControlHost::command_cards` / `command_cards_mut` / `queue_clipboard_copy`）。
機械検証は tako-core 20 本（帯の幾何 7 本を含む）+ tako-app 3 本（署名・推定）+ dispatch 7 本 +
セルフテスト項目 91（長いコマンド・複数行のコピー往復 = 実クリップボード照合、新規ペイン実行、
狭幅描画、UI ボタンと dispatch の同一経路）+ 項目 91b（出現で行数が帯のぶんちょうど減る /
テキスト領域と帯が縦を分け合う / 落ち着いた後は再描画・コピーで行数が動かない /
3 枚スタックでも上限内かつ端末に最低行数が残る / 閉じると完全に元へ戻る）+
visual-test「カード帯」（**実ピクセル**でカードの地色が帯にはあってテキスト領域には 1px も無い /
インクの行数 = PTY 行数 / 閉じると戻る）。実 claude での見た目は隔離インスタンスのスクショで確認する。

### FR-2.23 GUI ライク表示モード（初心者向け UI。G1 ✅ 2026-07-31、#691 / #694）

> **詳細仕様の正は `.agent/plans/2026-07-gui-mode.md`**（エピック #691）。ここには
> 不変条件だけを置く。狙いはターミナルに抵抗感のあるユーザーが黒い画面を直視せずに
> AI オーケストレーションを使い始められること（設計原則「ゼロコンフィグで一般ユーザーが
> 使える」の中核強化）。G2 = チャットビュー / G3 = チャット操作 / G4 = 磨き込みは
> 同仕様書 §4 のフェーズで追加する。

| ID | 要件 | 優先度 |
|---|---|---|
| FR-2.23.1 | 表示モードは `ui_mode`（`terminal`（既定）/ `gui`）の**グローバル 1 値**。settings.json へ永続化する。既定が terminal なので**この項目を知らない既存ユーザーの表示は変わらない**（旧ファイルは serde default で terminal） | M |
| FR-2.23.2 | 切替は**表示レイヤだけ**を変える。PTY / tmux バックエンドセッション / TerminalSession / layout.json（persist）には一切触らない = トグルをターミナル側へ倒せば「バックで動いていたターミナル」がそのまま見える。分岐は `render_pane` 内の 1 箇所（プレビューの code ⇔ md トグル FR-3.3 と同じ方式）で、**ペインの PTY リサイズ追従より後**に置く（表示中にウィンドウを変えても、起動するエージェントは正しい端末サイズで始まる） | M |
| FR-2.23.3 | GUI モードでも全ペインが変貌するのではなく、**ペイン種別ごとに表示を決める**（判定表 = 仕様書 §2.1）。判定は**保守的**に倒し、置き換えるのは確信がある場合だけ（不明・実行中・エラー保持中・alt screen TUI はターミナル表示のまま）。判定は純関数（`tako_core::ui_mode::pane_display`）で unit test 可能にし、**毎 render 呼ばれるので新しいサブプロセスを要する材料は使わない**（子プロセス判定は sleep_guard の 2 秒評価で共有するキャッシュを流用し、tmux / ps の実走査は変化時 + 60 秒保険 + busy の集合が変わった回の確かめ直し 1 回に限定する。#779 / #2477） | M |
| FR-2.23.4 | **スターター**: 子プロセスの無いアイドルシェルのペインは「AI チームに任せる（`tako master`）/ AI と 1 対 1 で話す（`tako solo`）/ コマンド入力へ」の 3 カードにする。master / solo は**シェルへコマンド行を書き込む**（ウェルカムバナー FR-2.14.10 と同じ方式。エージェント CLI の起動そのものは CLI_ONLY なのでこれが正。副次効果としてターミナル表示に戻すと実行されたコマンドが履歴に見える = 学習経路）。ヘッダに × を残し、GUI モードでもペインを閉じられること | M |
| FR-2.23.5 | 「コマンド入力へ」は**そのペインだけ**をターミナル表示に戻す**揮発フラグ**（永続化しない = 再起動で GUI 表示に戻る）。ペイン単位・タブ単位の永続的なモードは持たない（初心者にモードの入れ子を作らない） | M |
| FR-2.23.6 | 操作は dispatch `UiMode`（status / set / toggle / release / restore）+ CLI `tako ui-mode [gui\|terminal\|toggle\|release\|restore] [--pane N]` + MCP `tako_ui_mode` へ 1:1 公開する。GUI のトグルボタン（タブバーのテーマボタン左隣）と ⌘K パレット・表示メニューの「表示モードを切替」（パレットは「（かんたん・ターミナル）」を補足に足す。#2322）も**同じ dispatch を呼ぶ**ので UI 操作と AI 操作が食い違わない（開発不変条件）。**#2340: パレットは旧表記「表示モードを切り替え（かんたん / ターミナル）」/ "Switch view mode (simple / terminal)" でも引ける**（検索だけに効く別名 `ui_text::palette::cmd_aliases`。表示はしない。テーマ・表示言語・新規タブの旧表記も同じ。規則は `.agent/conventions.md` の UI 文字列の節）。**パレットの絞り込み（別名を含む）は CLI / MCP に写さない**（設計原則 5 の例外）: 絞り込みは人が語を打って項目を探すための入力補助で、操作そのものではない。AI は探さずに操作を名前で直接呼べる（固定コマンドの操作は `tako ui-mode toggle` / `tako theme toggle` / `tako lang` / `tako panel` / `tako split` 等で 1:1 に出ており、パレットを開くのは `tako menu invoke コマンドパレット`）。別名の解決をメニュー（`tako menu invoke`）へ広げないのは、表記の正本をメニューの 1 つに保つため | M |
| FR-2.23.7 | 文言は日英必須（`ui_text::ui_mode`）・絵文字禁止（SVG アイコン）・色は Theme 経由で dark / light 両対応（FR-4）。ライト側だけ潰れる事故（#669 と同じ形）を防ぐため、**スターターの可読性は実ピクセルで機械検証する** | M |
| FR-2.23.8 | **チャット本文のテキスト選択とコピー**（✅ 2026-08-02、#725）: 会話の本文はドラッグで選択でき ⌘C でコピーできる。選択の座標系はプレビュー（FR-3.2 / FR-3.3）と同じ **(行番号, UTF-8 byte)** で、1 ペインぶんの会話を通しで採番するので**複数の発話にまたがる選択**がそのまま成立する。ヒットテストは実 shaping の逆写像（#145 / #656 の資産）を共有し、日本語・太字・見出しサイズ・表のセルでも位置がずれない。選択できるのは会話本文（ユーザー発話・assistant の md・開いた thinking / ツール出力）で、ヘッダ・入力欄・カードのボタンは対象外（各ボタンが mouse down の伝播を止めるので、押下で選択が始まらない）。**ドラッグ中は新着で下端へ飛ばない**（掃いている途中の選択を壊さない）。⌘A は編集中バッファ → チャット本文 → プレビュー本文の順に効く | M |
| FR-2.23.9 | **発話単位のコピーボタン**（✅ 2026-08-02、#725）: ユーザー発話 / assistant 発話の右に固定幅の列を取り、そこへコピーボタンを置く（**絶対配置で本文へ被せない** = 本文が右端まで来ても文字と重ならない）。常時表示だが淡色で、ホバーとコピー直後だけ濃くする（`opacity(0)` + `group_hover` の「ホバーで初めて現れる」方式は GPUI で実機のホバー復帰が発火しないことを #680 で実測済み）。渡すのは**画面と同じプレーンテキスト**（md ソースではない。ブロック間は空行・ブロック内の行は改行で連結）で、**折りたたみ（#716 の 1200 字制限）は無視して全文**。本文の無い発話（画像だけ・ツールだけ）にはボタンを出さない。assistant の md コードブロックには #680 と同じコピーボタンが出る | M |
| FR-2.23.10 | 発話のコピーは CLI `tako chat copy [--pane N] [--message N] [--code K] [--markdown] [--list]` + MCP `tako_chat_copy`（計 133 ツール）へ 1:1 公開する（開発不変条件）。`--message` 省略時は**最後の assistant 発話**（「いまの答えをコピーしたい」がほぼ唯一の用途なので既定を賢くする。#322）、`--list` は添字・role・文字数・コードブロック数の下見、`--markdown` は md ソースをそのまま渡す逃げ道。UI ボタン・CLI・MCP は同じ `copy_chat_message` を通る。**ドラッグ選択そのもの（ポインタのジェスチャ）は CLI へ写さない**: AI にとっての等価物は既存の `tako read` / `tako orchestrator report` / この `tako chat copy` であり、端末・プレビューのドラッグ選択も同じ理由で GUI 限定という既存の前例に合わせる | M |
| FR-2.23.11 | **入力欄の描画ソースは 1 つ**（✅ 2026-08-02、#737）: チャット入力欄に見える文字列は常に **TUI の入力行のミラー 1 本**で、tako が自前の文字列を重ねてよいのは「TUI が箱の中に何も描いていない」ときだけ。claude は空欄でも dim の案内文（実採取 = `Try "how does <filepath> work?"`）を、キュー滞留時は `Press up to edit queued messages` を**箱の中へ**描くので、「ユーザーが打った文字があるか」で判断すると自前のプレースホルダが**同じ座標に重なって読めなくなる**（#737 の実測根因）。判定は `tako_core::screen::input_box_has_content` の 1 実装。上限行数を超えたぶんの案内（「上に N 行」）も箱の中へ absolute で重ねず、箱の**上の行**として出す | M |
| FR-2.23.12 | **IME はチャット入力欄の実座標に出す**（✅ 2026-08-02、#737）: チャット表示のペインはターミナルグリッドを描かないので、セル座標由来のアンカー（`pane_text_areas` + カーソルセル）は画面上のどこも指していない。未確定文字列は**入力欄の中へインライン描画**し（ウィンドウ側のオーバーレイは出さない = 二重描画しない）、候補ウィンドウ（`bounds_for_range`）も同じキャレット矩形を返す（#497 / #561 と同型の是正）。キャレットは「描き終わったミラー行の実 bounds + TUI のカーソルセル」から求める（写像は `tako_core::screen::input_caret_cell`）。未確定文字列は箱の `overflow_hidden` の内側に置くので、長い変換中テキストでもスラッシュボタンや隣のペインへはみ出さない。未確定文字列の**見た目は `ime_preedit_text` の 1 実装**をターミナル経路と共有する（片方だけ直して食い違う #29 / #497 の壊れ方を構造で防ぐ） | M |
| FR-2.23.13 | **作業中インジケータは会話末尾の AI 側**（✅ 2026-08-02、#737 追加要件 3）: 生成中は TUI のスピナー行（作業内容 + 経過時間 + 受信トークン数）を**メッセージ列の末尾**に assistant 側の 1 ブロックとして出す（Web 版 Claude のタイピングインジケータと同じ位置感）。下端追従スクロールにそのまま乗り、生成が終われば本文の発話に置き換わる。ヘッダは「いまどっちの手番か」だけの簡素な表示に留める（同じ情報を 2 か所で動かさない）。**下端追従の判断材料にはインジケータの有無だけを入れ、文言は入れない**（経過秒が毎秒変わるので、文言を鍵にすると毎フレーム「新着」になり手動スクロールを巻き戻す） | M |
| FR-2.23.14 | **assistant 発話も枠付きブロック**（✅ 2026-08-02、#737 追加要件 4）: 地の文だと①どこまでが 1 発話か分からない②コピーボタンの対象範囲が見えない。user（右寄せ・`surface_1` の濃い背景）と assistant（左寄せ・`surface_0` + subtle な枠）で区別しつつ、どちらも枠で発話境界を示す。thinking / ツールカード / コードブロック / インラインカードは枠の内側に収まり、コピーボタンの列は枠の外（ホバー領域と枠が一致して見える） | M |
| FR-2.23.15 | **生成中に送った指示も自分の吹き出しとして見える**（✅ 2026-08-02、#737 追加要件 5）: busy 中の入力は claude のキューへ入り、transcript には `queue-operation`（`operation: "enqueue"`、content つき）として**その時点で**記録される。本物の `user` 行になるのは配送後（長いターンでは数分後）で、ターン内へ差し込まれた場合は**一生 user 行にならない**（実 transcript 3416 本の全数走査で確定。「`<system-reminder>` ラッパーを過剰フィルタしている」という当初の推定は、user 行に `tool_result` と `text` が同居する例が 0 件であることをもって棄却した）。したがって正規化層は `enqueue` を user 発話として拾い、「送信待ち」を添えて表示する。配送されたら印を落とし、同じ本文の本物の user 行とは **1 対 1 で重複排除**する（同じ文面を 2 回送れば 2 個出る）。キューへ入るのはユーザー発話だけではない（`<task-notification>` 等のシステム注入が実測 1760 件）ので、中身は user 行と同じ `classify_user_content` を通し、生 XML が吹き出しにならないこと（#715 の保証）を維持する。加えて **Enter 素通し経路でも楽観 echo を積む**（#735 で送信ボタン以外が既定の送信経路になったため。送るバイト列には触らず表示だけ足す） | M |

実装メモ（2026-07-31 / G1）: モードと判定表は `tako-core::ui_mode`、状態と操作は
`tako-control::dispatch` の `Request::UiMode` 1 本（ホスト側は `ControlHost::ui_mode` /
`set_ui_mode` / `starter_released_panes` / `set_starter_released` の 4 メソッドだけ）、
描画は `tako-app/src/starter.rs`。機械検証は tako-core 8 本 + dispatch 2 本 + settings 2 本 +
セルフテスト項目 93（settings 往復と旧ファイル後方互換 / MCP toggle と GUI 状態の一致 /
判定表 / 「AI チームに任せる」→ 対象ペインへ `tako master` 送達 / 揮発解除と restore）+
visual-test「スターター」節（terminal では空の中央帯に GUI モードでカードが描かれる ×
dark / light の可読ピクセル）。

実装メモ（2026-08-02 / FR-2.23.8〜10、#725）: 選択の座標系は
`chat_view::ChatTextIndex`（行 → プレーンテキスト + 実描画 `TextLayout`）で、描画のたびに
作り直す。**本文はすべて `ChatTextIndex::push` を通す**ので、素の `SharedString` で描いた
行だけ選択できない穴ができない。ヒットテスト（`preview_text_layout_hit_test`）と切り出し
（`selection_text`）はプレビューと同一実装で、この 2 本を共有することが「見えているものと
コピーされるものが一致する」ことの構造的な担保になっている。機械検証はユニット 4 本 +
セルフテスト項目 98（発話をまたぐ採番 / 実ヒットテスト / ⌘C とクリップボード / コピー
ボタン / MCP の list・code・markdown / 折りたたみ境界 / 空発話 / ドラッグ中の自動スクロール
抑止 / ターミナル表示では ⌘C がチャットを返さない）+ visual-test「チャット」節（**合成
マウスの `PlatformInput` でドラッグ**して選択を作り、dark / light の塗りとコピーボタンの
実ピクセル、pbpaste 一致まで見る）。

### FR-2.24 master の自動ハンドオフ（✅ 2026-08-04、#749。基盤は #123 / #193 / #210）

master のコンテキストが埋まると判断が劣化する。`/compact` の自動実行は
「明らかに話が通じなくなる」ため**採らない**（ユーザー方針 2026-08-04）。代わりに
**新しい master へ乗り換える**（引き継ぎファイル更新 → 後任 spawn → 引き継ぎ確認 →
前任のペインを後任が閉じる）を、master 自身が閾値超過で自動的に行えるようにする。

| ID | 要件 | 優先度 |
|---|---|---|
| FR-2.24.1 | 引き継ぎ閾値は **50〜60%** の範囲で設定できる（既定 60）。解決順は **プロファイル（`profiles/<name>.yaml` の `ctx_threshold`）→ config.yaml の `ctx_threshold` → 既定 60** で、`tako_orchestrator_self` が実効値と出どころ（`ctx_threshold_source`）を返す。CLI / MCP から範囲外を指定したら**黙って丸めずエラー**にする（設定したつもりの値と実効値を食い違わせない）。一方**手書きの設定ファイル**に範囲外の値が入っていたときは丸めて動き続け、丸めたことを応答の `warnings` に出す（発動判定を壊さない） | M |
| FR-2.24.2 | **閾値超過に master が確実に気づく**: tako が 2 秒 tick で master ペインの ctx% を見張り、超過したら「引き継ぎを始めろ」という指示をそのペインへ送り込む（`【tako 自動通知】` / `[tako auto-notice]` で始まる文面）。ctx% は**画面（TUI フッター）由来**なので新しいポーリングもサブプロセスも増えない（NFR-8）。判定は純関数（`tako_core::handoff::nudge_decision`）で、`auto_handoff=false` / ctx% 不明 / 閾値未満 / 起動直後 60 秒 / 前回から 10 分未満 / 同一ペインで 3 回送信済み / すでに後任が立っている、のいずれかなら送らない。busy かどうかは**見ない**（claude は生成中の入力をキューへ入れターン終了時に処理するので、待つと長時間走る master に永遠に届かない）。送った事実は監査ログ（`<data_dir>/supervisor.log`）へ残す（黙って動かさない） | M |
| FR-2.24.3 | **前任のペインは後任が閉じる**。`tako_orchestrator_handoff` は後任の初期プロンプトへ順序つきの手順を埋め込む: ①`tako_orchestrator_workers` / `tako_list_panes` で引き継ぎファイルと**実態**を突き合わせる ②「引き継ぎ完了」と食い違いを報告する ③前任ペインを `tako_read_pane` で読み、**入力欄にユーザーの未送達の指示が残っていないか**（`input_status` = user / mixed、`queued_messages_pending`）を確認して引き取る ④そのあとで `tako_close_pane`。**この呼び出し自体は前任を閉じない**ので、後任の起動が失敗しても master を失わない（順序が安全側に倒れる） | M |
| FR-2.24.4 | 閉じる対象は **role が `orchestrator-master*` のペインだけ**に限る（`tab` 指定で呼ばれたときの分割元はユーザーのペインになりうる）。特定できなければ後任へ close を指示せず、応答の `previous_master_pane_id` を null にして「閉じない」ことを明示する | M |
| FR-2.24.5 | 後任の role / プロファイル / タブは前任と同一を引き継ぐ（#210 の維持）。文面（ナッジ・後任プロンプト）は `tako_core::handoff` の純関数 1 本が正で、日英対応（FR-4 の i18n 規約） | M |
| FR-2.24.6 | master の default system prompt に**自動発動の規範**を書く: 閾値超過で（a）**ユーザーの許可を求めない** （b）**区切りの良いタイミング**を選ぶ（返しかけの報告は先に片付ける） （c）**引き継ぎファイルの最新化が前提条件**（このツールは中身の鮮度を確認しないので、古いファイルのまま呼ぶと後任が盲目で始まる） （d）自分のペインは自分で閉じない。実効閾値は `{CTX_THRESHOLD}` として prompt へ焼き込む（毎回 self を呼ばなくても自分の閾値が分かる） | M |
| FR-2.24.7 | 設定は CLI `tako orchestrator profiles set --ctx-threshold N` / `--auto-handoff <bool>`（+ `--clear-*`）と MCP `tako_orchestrator_profiles` の同名パラメータへ 1:1 公開する（**新しい MCP ツールは増やさない** = 既存のプロファイル操作の一部として載せる）。GUI は設定画面「プロファイル」タブの「自動ハンドオフ」節（FR-4.7 と同じ dispatch 経路） | M |
| FR-2.24.8 | **引き継ぎファイルは「マシン非依存の知識」と「このマシンの実行状態」の 2 節に分ける**（#792）。見出しは `## 知識（マシン非依存）` / `## 実行状態（このマシン限定）`（英語表示なら `## Knowledge (machine-independent)` / `## Runtime state (this machine only)`）で、判定は表記ゆれ（番号付き・半角括弧・強調・語尾省略）を吸収する寛容な前方一致。pane / tab 番号は実行状態節にだけ書く（知識に混ざったまま別デバイスへ運ぶと、後任が存在しないペインへ指示を出す。FR-5.14 の設定共有で現実に起きる）。**旧書式（節なし）は従来どおり読める**: `tako_orchestrator_handoff` は書式に関わらず全文を後任へ渡し、旧書式なら後任プロンプトへ「番号への参照はすべて実態で確認」+「次の更新で 2 節へ書き直す」を添えて自然な移行に任せる（既存ファイルの一括変換はしない）。新書式なら節ごとの扱い（知識 = 前提にしてよい / 実行状態 = 必ず実態で確認）を添える。書式の判定結果は `tako_orchestrator_self` と `tako_orchestrator_handoff` の応答（`handoff_format` = `sectioned` / `legacy`、`handoff_sections`）で機械可読に返す。書式の正本は `tako_core::handoff`（`section_of_line` / `split_handoff` / `project_handoff_template` / `profile_memo_template` + 見出し定数）で、master system prompt の規範がその定数と食い違ったらテストで落とす | M |
| FR-2.24.9 | **引き継ぎの置き場はプロジェクト単位**（#915）。`handoff/projects/<project-key>.md` が projects.yaml のキーと 1:1 で、`handoff/<profile>.md` は**プロファイル運用メモ**（プロジェクトに紐付かない運用知識の共通置き場）。`tako_orchestrator_handoff` が後任へ渡すのは**管轄プロジェクトの分 + 運用メモだけ**で、管轄の判定は ①呼び出しの `projects` 引数 ②プロファイルの担当プロジェクト + 同じタブで稼働中 worker のプロジェクト ③稼働中 worker だけ の順（明示引数を最優先にするのは、worker が 1 体でもいれば推測が常に勝って引数が死ぬため）。どれも決まらなければ**本文を貼らず**一覧とパスだけを渡す（無関係な全文で後任の文脈を食わない）。判定結果は応答の `projects` / `jurisdiction_source` で機械可読に返す。運用メモが 80 行を超えたら `warnings` で「プロジェクト固有の内容を projects/ へ移せ」と促す（肥大の再発経路を残さない）。プロジェクトキーはファイル名として安全なものだけ受理する（両 OS で通る文字だけ。キー由来のパス脱出を構造的に防ぐ） | M |
| FR-2.24.10 | **旧形式（プロファイル単位の混在ファイル）は自動移行する**（#915 / #916 の第一適用例）。発火点は 2 つで、①`tako setup` の実行時 ②master が引き継ぎを読む / 書く経路（`tako_orchestrator_self` / `tako_orchestrator_handoff`）で旧形式を検知したとき。**手動移行という結末を作らない**。割り方はトップレベル `##` 見出し単位で、持ち主は マーカー（`<!-- tako:project: <key> -->`）→ 見出しに出るプロジェクトキー → 見出しの語とキーの先頭要素の一致 の順に決める（本文からキーを探さない: `tako` のようなキーはどの断片にも出るため実測で 6 件誤ヒットした）。担当プロジェクトがちょうど 1 つのプロファイルは見出しの無い先頭断片もそのプロジェクトへ移る。安全要件 4 つ = **冪等** / **原本を消さない**（`handoff/archive/<profile>-pre915-N.md` へ退避してから書き換え、退避できなければ移行しない）/ **実施が見える**（応答の `handoff_migration` と setup の `[migrated]` 行）/ **持ち主不明の断片を捨てない**（運用メモへ残す） | M |
| FR-2.24.11 | **後任は前任と同じ起動条件で立つ**（#854）。プロファイルの解決は呼び出し元の `TAKO_ORCHESTRATOR_ROLE` と**ペインの role ラベル**の両方から行い、非既定を優先する。インライン前置き（`TAKO_ORCHESTRATOR_ROLE=... claude`）はシェルへ export しないので claude を撃ち直した master は env を失い、以前は後任が既定プロファイル扱い（= `master_account` が乗らずユーザーの個人枠を消費・引き継ぎファイルも別 master のもの）になっていた。tako 自身が spawn / handoff / 復元のたびに書いている role ラベルを第 2 の出どころにする。どちらから決めたかは応答の `profile_source`（`caller_role` / `pane_role` / `default`）で返す | M |
| FR-2.24.12 | 引き継ぎファイルの一覧・読み・書き・移行を CLI `tako orchestrator handoffs <list\|show\|write\|migrate>` と MCP `tako_orchestrator_handoffs` へ 1:1 公開する（開発不変条件）。GUI も IPC も要らないローカルのファイル操作なので CLI は dispatch 関数を直接呼び、二重実装を作らない（`layout` / `accounts` と同じ形） | M |
| FR-2.24.13 | **プロジェクト追加でプロファイルを自動生成する**（#1453）。`tako orchestrator projects add <key>`（CLI / MCP）は `profiles/<key>.yaml` を `profiles/default.yaml` から継承して作り、**上書きするのは `projects: [<key>]` と `cwd`（projects.yaml の値）の 2 項目だけ**。既存ファイルには触らない（冪等・`create_profile_at` のロック内存在確認で TOCTOU 安全）。プロファイル名に使えないキーは生成せず理由を返すが、**プロジェクト登録そのものは成功させる**。登録済みプロジェクトは自動マイグレーション（FR-5.16 / #916）の発火点（`tako setup` / GUI 起動 / CLI）でまとめて揃え、**手動移行という結末を作らない**。**継承元の default.yaml を読めないときは作らない**（#1825）: 組み込み既定で作ると個人ルール・モデルの抜けたプロファイルが冪等ゆえに残り続けるため。登録は成功させ、理由を応答の `warnings`・persist.log・CLI の警告行に出し、壊れた中身は `.unreadable.bak` へ退避する（default.yaml には触らない）。直した後の発火点が継承して作る。空の default.yaml は「設定なし」として読める。A/B は `TAKO_1825_LEGACY=1` | M |
| FR-2.24.14 | **走っている master を専用プロファイルへその場で寄せる**（#1453）。`tako orchestrator adopt <名前>` / MCP `tako_orchestrator_adopt` は**セッションを立て直さない**（pid・会話・コンテキストは不変。ユーザー要望「シームレス・トークン節約」）。書き換えるのは**ペインの role ラベル 1 つ**で、`self` の profile / handoff_path / project_handoffs・worker spawn の既定・`handoff` の後任と宛先・自動ハンドオフ（#749）はFR-2.24.11 の解決（非既定の pane_role 優先）からそのまま追従する = **新しい永続状態を足さない**（role は layout.json に載るので GUI 再起動もまたぐ）。`<名前>` はプロファイル名か projects.yaml のキー（後者は FR-2.24.13 の生成を通してから採用）。同じ名前の 2 回目は `changed=false`、`adopt default` で汎用へ戻る。**呼べるのは汎用プロファイル（`projects` 未指定）で起動した master だけ**で、判定は名前でなく中身（`codex` も汎用の入口）。専用起動の master の乗り換えと master 系統（`master_agent`）の食い違いは**拒否して直す 1 コマンドを返す**（会話の途中で system prompt は差し替えられず、採用だけ通すと後任が別系統で立ち上がるため）。採用で専用になった master は起動が汎用のままなので採用し直せる | M |
| FR-2.24.15 | **読めない default.yaml でも master は立てるが黙らない**（#2227）。`tako master` / スマホの「+ master」/ 返答の配送（#1450）はプロファイルを `master_launch::load_profile` の 1 実装で読む。壊れた YAML・型違い・UTF-8 でない `profiles/default.yaml` は**組み込み既定で立てる**（止めると YAML を直せない利用者が master を立てられない = ゼロコンフィグ最優先。#916 の「退避して既定値で動き、知らせる」と同じ）が、個人ルール・モデルが効いていないことと直し方を CLI の `警告:` 行 / 起動の応答の `warnings`（PWA は起動結果の先頭に出す。#2233）・persist.log・master の system prompt の末尾へ出し、中身は `.unreadable.bak` へ退避する（default.yaml には触らない）。無い・空は従来どおり黙って立つ。`tako master -<key>` で #1825 のため専用プロファイルが無い（登録済み・未生成・継承元を読めない）ときは、「見つからない」に `projects add` と同じ理由と直し方（直したら `tako migrate run`）を足して止まる。A/B は `TAKO_2227_LEGACY=1` | M |
| FR-2.24.16 | **引き継ぎの後任・`tako solo`・表示だけの経路も読めないプロファイルで黙らない**（#2233）。引き継ぎの後任は `load_profile` で読み、読めない default・無い / 読めない専用プロファイルのどれでも**引き継ぎは止めずに**組み込み既定で立てて、前任への応答の `warnings`・persist.log・後任の system prompt へ理由を出す（止めると ctx の上限に近い前任が master ごと失われうる）。`tako solo` は `solo-profiles/default.yaml` を同じ形で読む（solo の組み込み既定で立てて `警告:` 行・persist.log・solo の system prompt・`.unreadable.bak` へ退避）。`orchestrator self` / `supervisor status` / `tako setup` の表示は読むだけ（退避しない）で既定値を返しつつ理由を `warnings` / 画面の行へ出す。GUI の自動ハンドオフの周期判定（`handoff_policy_for`）は出し先が無いので出さない。A/B は `TAKO_2233_LEGACY=1` | M |
| FR-2.24.17 | **worker の spawn・コンフリクト解消エージェント・adopt・`orchestrator guide`・自動復帰の既定も読めないプロファイルで黙らない**（#2256）。spawn とコンフリクト解消エージェント（起動）は呼び出し元 master のプロファイルを `master_launch::load_caller_profile`（default の起動と同じ退避つきの読み方）で読み、**止めずに**組み込み既定で立てて（master は人の居ない時間帯にも spawn する。master 自身も同じ既定で立って警告済み）、応答の `warnings`（spawn は kind `profile_unreadable`）・persist.log（`[spawn]` / `[git-resolve]`）へ理由を出し、中身は `.unreadable.bak` へ退避する。GUI の「解消エージェント」ボタンは警告をカードに添えて自動で消さない。adopt の起動時プロファイル（判定）・`orchestrator guide`（表示）・master / solo 本人の自動復帰の既定（判定）は `orchestrator::read_for_judgment`（読むだけ・退避しない）で既定値を使いつつ、adopt は応答の `warnings`（断るときは理由に添える）、guide は `warnings`・CLI の `警告:` 行・**`local-rules` の本文を「読めない」へ差し替え**（「on-demand のルールは無い」を返さない）、自動復帰は role を貼る瞬間に 1 回だけ persist.log の `[limit-autoresume]` 行へ出す。A/B は `TAKO_2256_LEGACY=1` | M |
| FR-2.24.18 | **届いた送達を届いていないと言わない**（#2330）。狭いペインの claude は入力欄の高さに上限があり、長い本文はカーソル（末尾）へスクロールして描く。`❯` は見えている最初の行に付くので、本文の先頭も先頭の `❯` 行も画面から消え、keys 経路の送達フローは届いたのに `gave_up / paste_not_reflected` を返していた（10/18 の引き継ぎ・10/20 の追試で実測。54 桁で `❯ 段落2: …` から描かれる）。画面で確かめられずに諦める直前（`VerifySubmitted` の未確認・残留の 2 枝）に**受け手の会話記録**を最大 8 秒見て、貼り付けの前に控えた位置より後ろへ送った本文を運ぶ利用者の発話（`type: user`）か生成中のキュー投函（`queue-operation` の `enqueue`）が入っていれば `delivered`（顛末 `received`・経路 `keys`）。照合は空白を除いた本文の先頭 48 文字。所在は tako mod の報告の会話 ID（設定 dir の手がかりつき）→ 走査で覚えた会話 ID の順に引き、**引けないときだけ** mod のターン遷移（送信前 idle → Enter の後に届いた新しい報告で busy / permission / question）を証拠にする（ターンは別の理由でも始まるので、会話記録を読めるのに本文が無いなら届いていない）。本当に届いていない送信には記録が出来ないので従来どおり `gave_up`。persist.log には証拠の種類だけ（本文は書かない）。実装は `tako_control::delivery_receipt`。A/B は `TAKO_2330_LEGACY=1`、検証用の注入 `TAKO_2330_INJECT_DROP_PASTE=1`（貼り付けを捨てる = 本当に届かない） | M |
| FR-2.24.19 | **master 同士の close でタブの master を消さない**（#2330）。10/18 に前任（後任へ届いていないと読んだ）と後任（手順 4）が 0.2 秒差で互いを `force` で閉じ、タブから master が消えた（約 2 日誰も気付かなかった）。dispatch の close（CLI / MCP の両方）は、呼び出し元が master（名乗りの role かそのペインの role）で別の master のペインを閉じるとき、①**引き継ぎを受け取った後任**（前任が生きている）②**閉じるとそのタブにそのプロファイルの master が 1 人も残らない**ペイン を **force でも**断り、理由と次の一手（待つ・`tako_read_pane` で確かめる・人が画面から閉じる）を返す。引き継ぎの状態は GUI プロセスの帳面（後任 → 前任・受領の証拠）で、発行（handoff）・送達の決着（`delivered` = `verified` / `received` / peer の `delivered`。「送ったかもしれない」は受領にしない）・ペインを閉じたときに更新する（GUI の再起動で空になり、そのときは ② だけが効く）。受領前の後任は前任が閉じ直せる（やり直しの道を残す）。**後任が自分の前任を閉じる close**（帳面に前任 → 後任があり、呼び出し元がその後任の生きた master のペイン）は**タブの人数に関係なく通す**: `--tab` で別のタブへ後任を立てた引き継ぎ（帳面へは呼び出し元の master を前任として書く）や、後から別タブへ動かした後任では前任のタブに同じプロファイルの master が残らないが、後任自身が残るので断ると前任が幽霊になる。受領は条件にしない（前任を閉じに来たこと自体が引き継ぎ文を読んだ証拠で、受領の検知が外れた系統で幽霊を作らない）。帳面が空（GUI の再起動後）なら「最後の master」の規則のまま。**対象外**: 自分のペインを閉じる close・role を名乗らない呼び出し（人のシェルの CLI・スマホ）・master でない対象・**人が画面から閉じる経路**（人の明示の操作で、守りが AI を止めたときの逃げ道でもあるので止めない）。呼び出し元のペインは `Request::Close` の `caller_pane`（MCP は接続時の `TAKO_PANE_ID` / `X-Tako-Pane`、CLI は `TAKO_PANE_ID`。新しい MCP ツールは増やさない）。判断は `tako_core::handoff_guard::decide`、帳面は `tako_control::handoff_guard`。手順は guide `handoff`。A/B は `TAKO_2330_LEGACY=1` | M |

実装メモ（2026-08-04 / #749）: 判定と文面は `tako-core::handoff`（GPUI / I/O 非依存の
純関数 + 定数）、閾値の解決は `tako-control::orchestrator::resolve_ctx_threshold`
（プロファイル / config の値を引数で受ける純関数）と `Profile::resolved_ctx_threshold`
（I/O 込みの唯一のアクセサ）、通知の送信は `tako-app` の 2 秒 tick
（`drive_handoff_nudge`）。**機構（tako が気づいて促す）と規範（prompt に自動発動を書く）
の両輪**で成立させる設計で、MCP 応答へ ctx 警告を同梱する案は採らなかった（ctx% を MCP
ハンドラスレッドで得るには `claude agents --json` のサブプロセスか UI スレッドへの往復が
必要で、tick + 画面由来のほうが安く確実。加えて JSON の 1 フィールドはユーザーターンの
指示より弱い）。機械検証は tako-core 16 本 + tako-control 8 本 + セルフテスト項目 101
（実画面 55% × 閾値 50 / 60 で送る・黙るの両方向 / 再送間隔 / auto_handoff=false /
master 以外のペイン / 後任が立っている場合 / self の実効値）。

### FR-2.25 worker の選択肢ダイアログの検知・構造化取得・応答（✅ 2026-08-04、#748。基盤は #319 / #530 / #577 / #157）

> エージェント TUI が出すダイアログは permission（ツール承認）だけではない。usage limit の
> 対処選択・`/model` のモデル選択・`/mcp` の一覧・plan モードの実行確認・AskUserQuestion の
> 質問がいずれも**入力欄を奪う**。旧実装は permission だけを構造化していたため、
> それ以外は「idle + question」= 完了として通知され、master は本文へ返信しようとして
> 送達が入力欄と混線した（2026-08-04 の実運用観測: limit ダイアログの選択肢テキストが
> `style=user` の残留入力として報告され続けた）。

| ID | 要件 | 優先度 |
|---|---|---|
| FR-2.25.1 | **ダイアログの存在判定は文言ではなく構造**で行う（#530 の教訓を全種別へ拡張）。判定は 3 経路: ①番号つき（カーソル行が `N. …` + 画面に番号つき行が 2 つ以上。番号は多桁も可）②番号なし（カーソル行の中身の開始桁に揃った兄弟行が隣接して 2 つ以上。ただしカーソル行が上下とも罫線で挟まれていれば入力ボックスとみなして棄却する）③**カーソルなしの番号つき**（#1143。FR-2.25.11）。実装は `tako-core::dialog` の 1 本で、**入力欄判定・送達フロー・worker 状態がすべて同じ関数を通る** | M |
| FR-2.25.2 | **種別は文言で分類する**（`permission` / `trust` / `bypass` / `usage_limit` / `plan_confirm` / `select`）。分類できないものは `select` に落ちるだけで、ダイアログとしては検知され続ける（未知のダイアログを素通りさせない）。`trust` / `bypass` は tako 自身が承諾するので `auto_accepted` を立て、master には「触るな」を伝える | M |
| FR-2.25.3 | `worker_status` / `read_pane` が **`choice_dialog`**（kind / title / options[number,label,highlighted,label_truncated] / numbered / cursor_visible / labels_truncated / recommended_action / auto_accepted）を返す。ダイアログ実在時は `input_status` を null にする（選択カーソルは入力欄と同じ字面なので、選択肢テキストを残留入力として報告してはならない） | M |
| FR-2.25.4 | watch は permission を `WORKER_PERMISSION`、それ以外を **`WORKER_DIALOG`（種別つき）** として発火する。ダイアログ実在時は `question` イベントを出さない（本文への返信では解けない）。`trust` / `bypass` は停止として通知しない（待てば消える）。usage limit の対処ダイアログは `WORKER_ERROR`（#157 の kind）のまま扱い、`choice_dialog` を併せて返す（「解除まで待つ」復旧経路を迂回させない） | M |
| FR-2.25.5 | **応答は番号かラベルで指定する**。番号は画面の番号（無ければ 1 始まりの順番）、ラベルは部分一致（大小無視）で、複数一致は曖昧として拒否する（黙って推測しない）。**画面上で切り詰められたラベル（`label_truncated`）への一致指定は拒否して番号を要求する**（#1143。FR-2.25.11。狭いペインでラベルと説明の境目が読めない選択肢も同じ = #2302・FR-2.25.15）。`choice` を省略すると**送信せず構造だけ返す**（下見。ツールを増やさず一覧と応答を同じコマンドで賄う） | M |
| FR-2.25.6 | キー送出は実測に従う: **番号つきダイアログは番号キーだけで確定する**（Enter を足すと解消後の入力欄へ抜ける）。番号キーで確定しない TUI 向けに、ダイアログが残っているときだけ Enter を追送する。**番号なしダイアログでは番号キーが無反応**なので `↑`/`↓` で移動し、**ラベル一致で着地を検証してから** Enter を送る（着地できなければ Enter を送らずエラー = 誤選択を確定させない） | M |
| FR-2.25.7 | 応答前にダイアログの実在を再検証し（#319 の原則を維持）、応答後に解消を検証して `resolved` を返す。応答は `persist.log` へ監査記録する（種別・選択・送ったキー・解消結果） | M |
| FR-2.25.8 | **ダイアログ表示中の `Send`（テキスト / Enter 単独）は選択肢一覧つきのエラーで拒否する**。テキストは 1 文字ずつダイアログのキー操作として食われ、数字なら選択が確定し、Enter はハイライト中の選択肢を確定させるため。生のエスケープシーケンス（矢印キー等の低レベルキー送信）と、tako 自身が承諾する `trust` / `bypass` は通す | M |
| FR-2.25.9 | supervisor（#401）の limit 系自動復旧は**盲目的な Enter / 固定番号をやめ**、下見 → 「解除まで待つ / 現状維持 / 停止」をラベルで選ぶ。課金・モデル変更しか選べないダイアログでは自動応答せず通知のみに落ちる（黙って課金プランやモデルを変えない） | M |
| FR-2.25.10 | master のデフォルト system prompt に種別ごとの対応規範を書く（下見してから答える / usage_limit は待つ選択肢を選び課金系はユーザーへエスカレーション / plan_confirm は計画の一致を確認 / select は AskUserQuestion なら自分が答える / trust・bypass は触らない / 番号なしは矢印移動で tako が確定させる） | M |
| FR-2.25.11 | **狭いペインでは選択カーソルもラベルの全文も画面に残らない**（✅ 2026-09-07、#1143。実測 25 桁 × 40 行 / claude 2.1.258）。ダイアログがペインより高いと claude は箱を上端から描いて下を切り捨てるため、`❯` も操作キーの案内も画面外へ出る → **番号つき選択肢の連なりそのものを anchor にする**。誤検知（応答本文の箇条書き・シェルで開いた Markdown）を防ぐため、「入力欄のプロンプトが画面に 1 つも無い」「同じ桁に揃った番号つき行が 2 つ以上あり番号が 1 ずつ増える」「連なりの上に罫線がある」「連なりのあとは空行・キー案内・罫線・より深い字下げだけ = 画面が一覧の途中で尽きている」を**すべて**満たすときだけ採る。位置は知らないので `cursor_visible: false` / `highlighted: null` を返す（**でっち上げない**）。併せて claude 自身が `…` で切り詰めたラベルは**復元できない**（折り返しの結合 = #1131 では戻せない）ので `label_truncated` として申告し、`respond` のラベル一致を**番号確定へ倒す**。2 列レイアウト（ラベル列 + 説明列）の説明列は「折り返しの続きがその桁へ字下げされている」ことで判定して**ラベルへ混ぜない**（狭幅では説明列がセル単位で割れるため、空白で継ぐと `1M co ntext` という壊れた語がラベルに入る）。A/B は `TAKO_1143_LEGACY=1` | M |
| FR-2.25.12 | **本文（`command` / `title`）は「承認を求めている操作」から始める**（✅ 2026-09-11、#633）。承認カードは本文を 1 行に詰めて出すので、直前のアシスタント発話が先頭に残ると本題（実行されるコマンド）へ到達する前に省略される。本文の境界は「当たったら捨てる」形（罫線 / 0 桁の非空行 = #1293 / 選択肢に採らなかった番号つき行）しか無く、**罫線を引かず箱ごと 0 桁で描く許可ダイアログ**（agy の `Requesting permission for:` / claude の `Claude wants to run:`）では捨てる材料が 1 つも無い → **本体の開始マーカー（`dialog::BODY_START_MARKERS`）を起点にし、一致が無ければ従来のブロック抽出へ落ちる**。起点は選択肢に最も近い一致で、**起点を下げるだけなので結果は必ず従来の本文の接尾辞**（拾えていた本文は減らない）。マーカーは実採取の画面に在る文言だけを載せる（当て推量で足さない）。A/B は `TAKO_633_LEGACY=1` | M |
| FR-2.25.13 | **選択肢の右へ副画面が並ぶ 2 カラム配置も検知する**（✅ 2026-09-14、#1447。実測 2026-09-13 / claude 2.1.258 / 60 桁）。AskUserQuestion の `preview` つき（side-by-side layout）は、選択肢の行末に副画面の枠線が同居し、**長いラベルの折り返しが中身の桁（`❯ 1. ` の後ろ = 5）より 1 桁浅い 4 桁**へ字下げされる。真因は後者で、あいだの行が「ダイアログの外」に分類されて `numbered_block` が選択肢 1 個で打ち切られ、`choice_dialog: null` / watch が `WORKER_DIALOG` も `WORKER_IDLE` も出さない / `respond` が「画面に存在しない」になっていた（実測: 枠を剥がしただけでは直らず、継続行を 5 桁へ揃えると修正前でも採れる）。**折り返しの下限は「マーカー（`N.`）より右」の 1 実装（`dialog::wrap_indent_floor`）から採る**（揃える桁は描画側の都合で変わるので下限だけを条件にする）。緩めたぶんの歯止めとして、選択カーソル行と番号つき行は続き（`Continuation`）にしない。副画面は判定の前に剥がす（`side_panel_cuts` → `strip_side_panel`。枠がラベルへ混ざると `respond` のラベル一致と #813 の安全な選択肢の選別が静かに外れる）。誤爆を避けるため剥がしは**枠の左上角が本文の右に在る / 同じ形が 2 行以上 / 番号つき選択肢と同居**の 3 条件をすべて満たすときだけ（全幅の箱の右端 `│` では発火しない）。A/B は `TAKO_1447_LEGACY=1` | M |
| FR-2.25.14 | **ペインより高いダイアログも、status も画面全体で読む**（✅ 2026-10-18、#1847。実測 claude 2.1.280 / 2.1.301・32 桁）。狭いペインの AskUserQuestion は説明の折り返しで選択肢 1 つが 6〜10 行になり、4 択で箱が 45 行を超える。ダイアログがペインより高いと claude は**選択カーソル `❯ 1.` を画面の最上段に残して**下を描くので、カーソルは走査の窓（下端から 40 行 = `SCAN_ROWS`）の外・箱の上端の罫線は画面の外になり、経路 1 も FR-2.25.11 の経路 3（罫線か 2 列レイアウトが要る）も外れて `read_pane` の `choice_dialog: null` / `respond` が「見つからない」だった（9/29 の実害: 8 体並べた 32 桁の worker）。**経路 5**（`dialog::numbered_above_scan`）: 窓の中に選択カーソルが 1 つも無いときだけ、窓より上で最も下の選択カーソル行を起点に番号つきの並びを組み、並びから画面の下端までがダイアログの一部だけ（空行・説明の折り返し・確定キーの案内・罫線）で案内が 1 行はあるときに採る（経路 4 と同じ `dialog_reaches_input_box`。窓を広げないので、下に入力欄がある解決済みのダイアログは拾わない）。経路 3 より先に見る（カーソルと選択肢 1 が採れる）。**worker_status は `choice_dialog` / `permission_dialog` / events のダイアログを画面全体で判定する**（それまでは `recent_output` = 末尾 30 行で、36 行の 4 択でもカーソルと罫線が窓の外 = read_pane / respond と食い違った。器越しのフォールバックも画面全体を残す）。狭いペインのラベルと説明の境目は FR-2.25.15（#2302）。A/B は `TAKO_1847_LEGACY=1`。**#2343（2026-10-23。実採取 claude 2.1.301・24 桁）**: 確定キーの案内そのものが折れる（24 桁で `Enter to select · ↑/↓ to` / `navigate · Esc to` / `cancel` の 3 行、42〜44 桁で `… · Esc to` / `cancel`）。案内を 1 行ずつ `is_key_hint` で読んでいたので 2 行目以降がダイアログの外の行になり、24 桁の背の高い AskUserQuestion を経路 5 が採らなかった（`dialog_reaches_input_box` を共有する経路 4 も同じ）。`dialog::WrappedHint`: 案内は `<キー> to <動詞>` を ` · ` で並べた 1 文（キーは ASCII と矢印の 1〜2 語・動詞は小文字の英単語 1〜2 語。全 fixture の案内がこの形）なので、繋いだ文がその形のままで最後の区切りに動詞がまだ無いあいだだけ、直下の同じ桁の行を案内の続きとして読む（空行の後・桁違い・形の崩れる行 = 会話の本文は従来どおりダイアログの外）。全 fixture の 214 画面と、その末尾に本文を足した / カーソルを消した変種で、修正の前後に変わったのは huge-24 の不検知だけ（`issue2343_wrapped_key_hint`）。経路 3（カーソルなし）の末尾の判定は折れた案内を読まない（カーソルの描かれない 24 桁の採取が無い）。A/B は `TAKO_2343_LEGACY=1` | M |
| FR-2.25.15 | **狭いペインでもラベルと説明の境目を読み、読めなければ申告する**（✅ 2026-10-18、#2302。実測 claude 2.1.301・25 / 32 / 40 / 80 桁の AskUserQuestion と permission）。AskUserQuestion のラベルの折り返しと説明は**同じ桁へ字下げされ、同じ右端で語単位に折り返す**ので字下げでは見分けられず、#1131 の結合は 32 桁で説明 7 行をラベルへ飲み、char 数の見積もりは全角の続きを落としていた（どちらも `label_truncated` が立たず、respond のラベル指定 = 部分一致が説明の文言で別の選択肢を掴みえた）。判定は `dialog::label_ends`: 選択肢の下で**前の行に入る語が次の行へ回った境目**（段落の切れ目。表示幅で測り、右端は並びの行が実際に届いた桁。右端ちょうどまで埋まった行の次の行は 1 桁狭い = 実測）を探す。説明の中の改行は U+FFFD で 1 行に流れる（実測）ので、切れ目はラベルと説明の境目の 1 か所しかない。①並びのどこにも切れ目が無ければ従来の結合のまま（permission / 上限 / 確認ダイアログ。右端に余白があり切れ目も出ない）。ただし結合が**右端まで埋まった区切りの無い長い語**の行を飲むときは説明を飲んだかもしれないので申告する。**説明を必ず持つと分かる並び**（AskUserQuestion = claude が自由入力の選択肢 `Type something.` を足す並び。#2320）では、切れ目が出なくても従来の結合が続きの行を飲むなら**ラベルの 1 行目だけ + `label_truncated=true`**、続きが 1 行ならそれが説明（どのラベルも右端近くまで届き説明の最初の語が入らないと切れ目が並びのどこにも出ない。実採取 claude 2.1.301 の 32〜37 桁で説明が丸ごとラベルへ混ざり、respond のラベル指定が説明の語で選択肢を確定した。説明は色 = 256 色の 246 で描き分けられているが、検知器が受け取る画面テキスト = GUI のグリッド・`capture-pane -p` には残らない。自由入力の選択肢が無い並びは従来どおり） ②切れ目が 1 つでもあれば説明つきの並びとして、選択肢ごとに切れ目の手前までをラベルにする。切れ目の無い選択肢は、次の行が長い語の途中**と証拠で言える**なら説明の始まり（日本語の説明は語の区切りが少なく広いペインでもここに当たる。証拠は #2324 の段落）、続きが 1 行ならそれが説明（AskUserQuestion の schema は説明を required にしている）、それ以外は**ラベルの 1 行目だけ + `label_truncated=true`**（respond のラベル指定は拒否・番号は効く）。60 桁以上の画面は修正前と同じラベルを申告なしで返す。**#2324（2026-10-23。実測 claude 2.1.301・24〜44 / 50 / 60 / 80 桁 × 8 種）**: 狭いペインの全角ラベルで、行幅ちょうどの全角の語で終わるラベルの続きを「長い語の途中 = 説明」と読んで落とし（27 桁の `3. Issue` + `を閉じて調査だけにする`）、字の間で割れた全角のラベルを空白を挟んで繋いでいた（`ダイアログの検知を先に 直す`。どちらも申告なし）。claude は空白で折り返すと空白を消し、空白の無い語は行幅を超えるときだけ字の間で割る。**前の行が右端まで埋まっていても空白の有無は決まらない**（全角どうしで埋まった繋ぎ目 40 か所のうち 2 か所は空白あり = 字面が同じ）ので、ラベルへ繋ぐ行ごとに `dialog::seam` が証拠で決める: 前の行に次の行の 1 文字目がまだ入る / 繋ぎ目をまたぐ語が行幅に収まる → 空白、次の行が行頭に来ない字（閉じ括弧・句読点・小書きの仮名）で始まる → 空白なし、それ以外は推測で繋がず**確実な先頭まで + `label_truncated=true`**（`label` は本当のラベルの接頭辞）。長い語の規則は `dialog::long_word_evidence`（2 行目が行頭に来ない字で始まる = 字の間で割った語の途中か、1 行目の語がラベルに入る語の幅 = 40 桁 = 全角 20 字を超える。schema の label は「concise (1-5 words)」）のときだけ説明の始まりと読み、それ以外は申告する。説明を必ず持つ並びでは char 数の見積もりが続きを「別の要素」と読んでも申告する（表示幅では切れ目が無い）。半角だけのラベルは繋ぎ目をまたぐ語が行幅に収まるので従来どおり空白で繋ぐ。全採取の 984 選択肢で「60 桁と一致するか申告する」が成り立つ（`issue2324_narrow_cjk_label`。24 桁の huge は確定キーの案内が 3 行に折れる別の原因で未検知だった = #2343 で検知。FR-2.25.14）。A/B は `TAKO_2302_LEGACY=1`（#2320 の分は `TAKO_2320_LEGACY=1`、#2324 の分は `TAKO_2324_LEGACY=1`） | M |

実装メモ（2026-08-04）: 構造検知は `tako-core::dialog`（`detect_choice_list` / `is_choice_dialog` /
`numbered_choice` / `is_rule_line`。実採取画面 = permission / `/model` / plan 確認 / `/mcp` /
AskUserQuestion / codex・agy を fixture 化）、種別分類と JSON 化は
`tako-control::claude_tui`（`DialogKind` / `ChoiceDialog` / `detect_choice_dialog`。
`detect_permission_dialog` は kind=permission の薄いラッパとして互換維持）、
公開は `worker_status` / `Read` / `wait::WatchOutcome::ChoiceWaiting` /
`dispatch_orchestrator_respond`（`choice: Option<String>`）/ CLI `tako orchestrator respond`
（`--choice` 省略可）/ MCP `tako_orchestrator_respond`（ツール数は不変）。
機械検証は tako-core 13 本 + claude_tui 6 本 + wait 3 本 + dispatch 8 本 + supervisor 2 本 +
セルフテスト項目 102（実ペインに描いたダイアログで read の `choice_dialog` / `input_status`=null /
テキスト送信と Enter 単独の拒否 / respond の下見 / 解消後に送信が通る）+
実 claude e2e 2 本（permission の構造取得と**番号キーだけの確定** / `/model` を select として検知）。
#1143（2026-09-07）で経路 3 と切り詰め申告を追加: fixture は**幅と高さだけを変えて採った実 3 枚**
（25 桁 × 40 行 = カーソル画面外 / 25 桁 × 70 行 = 同じダイアログでカーソルあり / 80 桁 × 40 行 = 説明列が折り返す）で、
「同じ一覧なら高さが変わってもラベルが変わらない」を機械検証に据えている（tako-core 7 本 + claude_tui 2 本 +
wait 1 本 + dispatch 3 本）。
limit ダイアログは実機を limit まで使い切らずに再現できないため、
**文言は claude v2.1.220 バイナリ内の実文字列**（`What do you want to do?` /
`Stop and wait for limit to reset` / `Wait for limit to reset`）、
**レイアウトは実採取の `/model` ダイアログ**から合成した fixture で固定している。

### FR-2.26 worker レジストリの寿命と GC（✅ 2026-08-15、#658。基盤は #390）

> `workers.yaml` は「ペインが消えても worker を追跡し続ける」ための台帳（#390）で、
> **生死をこれ自身では観測しない**。そのため実体の無いエントリが `status: active` のまま
> 積み上がる。2026-07-30 の実測では、セルフテスト由来の 3 件（project `tako-selftest-165`）が
> **9 日間** active のまま `tako orchestrator workers` に出続けていた。原因は 2 つで、
> ①セルフテストの spawn が本番のレジストリへ書いていた ②死んだエントリを畳む主体が
> どこにも居なかった（#390 の GC は件数上限だけを見ており生死は見ていない）。

| ID | 要件 | 優先度 |
|---|---|---|
| FR-2.26.1 | **セルフテストは本番の orchestrator 状態へ書かない**。`TAKO_SELF_TEST=1` は data_dir を本番のまま使う設計なので、逃がす対象を 1 か所（`self_test_isolation_defaults`）で宣言し、増やし忘れをテストで固定する。対象は sessions（#112）/ ペインログ（#112）/ **workers.yaml（#390）** / **orchestrator ディレクトリ（ledger・projects）**。隔離は**セルフテスト項目 0**（最初に走る）が実プロセスで検査する — 隔離は他の全項目の前提であり、どれか 1 つでも本番へ書いてからでは遅い | M |
| FR-2.26.2 | **明示 close はレジストリへ記録する**。CLI / MCP の `Close` だけでなく **GUI 経路（× ボタン・cmd+W・タブ close）も** 対象（器ごと殺した worker が active で残る主因だった）。`CloseReason::Exited`（PTY 死亡）は**倒さない** — 「ペインが消えても worker は生きている」という #390 の追跡意図を壊さないため。セカンダリインスタンス（FR-5.8）はプライマリのレジストリを触らない。worker でないペインの close では**ファイルを書かない**（全ペインの close 経路から呼ばれるため） | M |
| FR-2.26.3 | **消してよい条件を限定する**: active かつ **ペインも器（tmux / psmux）も観測できない**状態が **`DEAD_CONFIRM_SECS`（= 300 秒）以上続いた**エントリだけを `status: closed` / `close_reason: gone` へ倒す。**1 回の観測では倒さない**（`dead_since` を刻んで確認期間を待つ）。器の列挙が一時的に失敗した・アプリ再起動直後でペインがまだ復元されていない、といった過渡状態で生きている worker を落とさないための安全側設計。生存が再観測されたら `dead_since` は取り消す | M |
| FR-2.26.4 | **消してはならないもの**: ①生きているエントリ（ペインか器のどちらかが見えれば生存） ②すでに closed のエントリ（GC の対象外）。この GC は **status を倒すだけでエントリを削除しない**ので、closed でも `resume_command` / `report --worker` / `workers --all` は引ける（**突然死からの復旧材料（#390）は失われない**）。なお実体の削除は**別機構**（`MAX_WORKERS` = 200 の保持上限。spawn 登録時に**古い closed から**削る）が担っており、active は上限では削られない。倒した直後のエントリが上限に押し出されうる点は許容する（実体の無い worker の復旧材料は価値が無い） | M |
| FR-2.26.5 | **同一性は「ペイン番号 + 器」で判定する**。ペイン番号は再利用されるため、追跡キー（`tmux_session`）を持つエントリは器の一致まで確認する（番号だけ一致する別ペインを生存とみなさない）。器なし spawn は番号のみで判定する。**一覧の `pane_alive` / `tmux_alive` と GC の判定は同じ関数（`liveness`）を通す** — 「一覧では死んで見えるのに GC は動かない / 一覧では生きているのに GC が殺す」という食い違いを構造的に防ぐ | M |
| FR-2.26.6 | GC は `tako orchestrator workers` / MCP `tako_orchestrator_workers` の**列挙のついでに**走る（掃除専用のコマンドを増やさない = #322 の最簡形）。**GC の失敗（ロック競合・書き込み不能）で一覧を落とさない**（読むだけへ縮退する）。セカンダリインスタンスはプライマリのペインを持たないため GC を回さない（他人の worker を殺さない） | M |
| FR-2.26.7 | **宣言したモデルと実際に使われたモデルを分けて持つ**（#1638）。spawn の `--model` は宣言に過ぎず、アカウントの entitlement・モデルの廃止で claude が無言で別のモデルへ解決する（2026-09-23 実測: 5.5 → 5）。レジストリと委任台帳に `model_actual`（claude の transcript の最初の assistant 行の `message.model`。確定したら書き換えない）を持ち、`workers` / `worker_status` は `model_declared` / `model_actual` / `model_check` を返し、食い違いは `warnings` と persist.log へ 1 行。ledger の `stats` は実際のモデルで数える（確かめられないものは宣言で別の行 = `model_source: declared`）。codex / agy は `agent_support` の `worker_model_actual` が degraded（宣言だけ = `not_observed`）。旧ファイルは serde の default で読める（移行なし）。A/B は `TAKO_1638_LEGACY=1` | M |

実装メモ（2026-08-15）: 判定は `tako-control::orchestrator::registry` の純関数
（`liveness` / `plan_sweep` → `SweepPlan { mark, close, revive }`）で、ファイル適用は
`sweep_dead_at`（変更が無ければ**書き込まない**）。計画はロックの内側で立て直すので、
読み取り後に他プロセスが更新していても勝たない。機械検証は unit 8 本
（初回観測では倒れない / 確認期間超過で倒れる / 生存 3 通りは倒れず revive される /
ペイン番号再利用でも器が違えば倒れる / closed は対象外 / 追跡キーと resume が残る /
掃除対象が無ければ書き換えない / 一覧と GC の判定一致）+ セルフテスト項目 0（隔離）。

### FR-2.27 利用上限後のペイン単位の自動復帰（✅ 2026-08-15、#813。基盤は #748 / #157 / #357 / #401 / #749）

> エージェントが 5 時間制限・週次制限に当たると、リセットまで止まったまま人間の再開操作を
> 待つ。長時間タスクを自律で回したいときはここが唯一の人間依存点になる。**リセット時刻が
> 来たら tako が自動で再開させる**が、上限ダイアログの他の選択肢は課金プラン変更・従量課金・
> モデル切替を伴うので、自動操作が**構造的に**それらを選べないようにする必要がある。
> オーケストレーション配下の worker は supervisor（#401）が面倒を見るが、こちらは
> **どのペインでも使える**（master 自身・ユーザーが自分で立てたエージェントも対象）。

| ID | 要件 | 優先度 |
|---|---|---|
| FR-2.27.1 | **ペイン単位のオプトイン（既定 OFF）**。ペインの右クリックメニューに「リミット後の自動復帰を有効 / 無効にする」トグルを置く。状態は `Pane` の属性として持ち **layout.json へ永続化**して再起動・復元・バックグラウンド退避をまたいで維持する（既定 OFF は JSON へ出さず、旧ファイルは serde default で OFF に読める） | M |
| FR-2.27.2 | **検知は既存のものだけを使う**。ダイアログ型 = `DialogKind::UsageLimit`（#748）、idle 型 = `detect_worker_error` の `UsageLimit`（#157）。新しい検知規則は増やさない（#748 / #157 と食い違わせない）。リセット時刻は画面の「reset at …」「try again at …」から解決し、**観測時刻から見て次に来る同じ時刻**として扱う（時刻表記は日付を持たないため。ローカルタイムのオフセットで換算する）。観測時刻は**その告知を最初に観測した時刻**（FR-2.27.18） | M |
| FR-2.27.3 | **発動条件**（すべて満たしたときだけ動く）: ①ペインでオプトイン済み ②上限由来の停止である ③**画面が変化していない**状態が一定時間続いた（生成中を文言ではなく変化の有無で判定する = #572 の教訓） ④リセット時刻 + 安全マージン（数分）を過ぎた（時刻不明なら初観測からの固定猶予に落とす） ⑤入力欄に**人間の下書きが無い**（`input_status` が user / mixed なら触らない） ⑥このエピソードの試行回数が上限（3 回）未満 | M |
| FR-2.27.4 | **復帰動作**: ダイアログ型は #748 の respond 経路で「解除まで待つ」相当の選択肢を**ラベル一致**で確定する。idle 型は継続ナッジを既存の送達確認つき経路（#32 / #790）へ積む。**ダイアログにテキストを送らない**（キー操作として食われる。#748） | M |
| FR-2.27.5 | **選ばない選択肢を構造で保証する**。選択肢の選別は許可リスト（「wait for limit to reset」「keep current model」等）**かつ**拒否リスト（upgrade / credit / billing / subscribe / switch to / change model / pay / `$` 等）で、許可に当たっていても拒否語を含めば選ばない。安全な選択肢が無ければ**何も送らずに諦める**。実装は `tako_core::limit_resume::safe_choice` の 1 本で、supervisor（#401）の limit 復旧も同じ関数を通る | M |
| FR-2.27.6 | **発動しないもの**: permission ダイアログ・plan 確認・モデル選択・API エラー・通常の idle。これらは自動復帰の対象外（**接続系の** API エラーは FR-2.27.17 = ネット復帰の即時再開の担当。それ以外の API エラーは supervisor の担当） | M |
| FR-2.27.7 | **2 秒 tick を重くしない**。有効なペインが 1 つも無ければ判断そのものを走らせず、有効なペインがあるときだけそのペインの可視画面（メモリ上のグリッド）を読む。新しいポーリング・サブプロセス起動を増やさない（#772 / #779）。キー送出を伴うダイアログ応答は background へ逃がす | M |
| FR-2.27.8 | **黙って動かさない**。実行と結果を `<data_dir>/supervisor.log` へ `action=limit_autoresume` で記録する（#749 の `ctx_handoff_nudge` と同じ場所・同じ形式）。ダイアログ応答は respond 経路の persist.log 監査（`[dialog-respond]`）もそのまま残る | M |
| FR-2.27.9 | **AI フルコントロール**: `tako limit-resume [on\|off] [--pane N] [--all]`（引数なしで現在値 = #322 の最簡形）と MCP `tako_limit_resume` へ 1:1 公開し、`list` / `read` / `worker_status` に状態フィールド（オプトインと実行状態 = 停止の型・復帰予定時刻・試行回数）を載せる。右クリック・CLI・MCP の 3 経路は**同じ dispatch** を通る | M |
| FR-2.27.10 | **表示**: 有効なペインはヘッダに控えめな SVG インジケータを出し（絵文字禁止 = #217）、上限で止まって待機しているあいだは色を強める。文言は日英（#435） | S |
| FR-2.27.11 | **プロファイル既定と spawn 適用**（✅ 2026-08-21、#822）。master / solo プロファイルに `limit_resume`（既定 false）を持たせ、**そのプロファイルから spawn した worker ペインへ自動適用**する（長時間の自律運転で「spawn するたび手で ON」が最後の人間依存点になるため）。解決順は **spawn 引数 → プロファイル → false** で、spawn 引数の `false` は「未指定」ではなく**明示 OFF**（プロファイルが ON でもその worker だけ切れる）。既定 OFF は FR-2.27.1 のペイン単位オプトインをそのまま保つ。適用されたことは `orchestrator spawn` の応答（`limit_resume`）・`orchestrator workers` の各行（ペインが居なければ `null`。番号再利用の別ペインを誤って有効と報告しない）・`worker_status` / `read` / `list` とヘッダインジケータ（FR-2.27.10 と同じペイン属性）で読める。設定は `tako orchestrator profiles set <名前> --limit-resume <bool>` / `--clear-limit-resume` と MCP `tako_orchestrator_profiles`、GUI は設定画面 → プロファイル → 「worker のリミット後自動復帰」の 3 経路が**同じ dispatch** を通る（`profiles show` の `resolved_limit_resume` が実効値で、GUI のトグルもこれを読む）。**solo プロファイルは worker を spawn しない**ので ON にしても効かず、`profiles list` / `show` / `set` が警告を返す（黙って死んだ設定にしない）。`orchestrator run` / task checkpoint resume（#242）も spawn と同じ経路なのでプロファイル既定がそのまま効く（個別の `--オプション` は増やさない = #322） | M |
| FR-2.27.12 | **上限の文言は 1 箇所で持つ**（✅ 2026-09-03、#1093）。停止判定（`detect_worker_error`）と**ステータスバーのメーター**が同じ規則を通す（正本 = `tako_core::limit_resume::is_limit_exhausted_line`）。判定が散っていたため、組織のクレジット上限で止まった 3 worker が**解除後 1 時間以上復帰せず `supervisor.log` に 1 件も記録が残らなかった**（旧規則は `hit your usage limit` 決め打ちで、実際の見出し`You've hit your session limit · resets 7:50pm (Asia/Tokyo)` に 1 文字も当たらない）。受ける形は claude の見出し**テンプレート**`You've hit your <限度の名前>limit…`（限度の名前は `session limit` / `weekly limit` / `Opus limit` / `Sonnet limit` / `Fable limit` / `usage credit limit` / 各種 spend limit / `usage limit` / `limit` で**すべて `limit` で終わる** = 版で名前が増えても規則で追従する。claude 2.1.258 のバイナリ内テンプレートと `nF` 表で実測）+ codex の同型 + 旧来の`usage limit reached` / `limit reached … reset`。**`limit reached, now using …` は除く**（自動モデル切替の告知で worker は止まらない）。解除時刻は既存の `resets ` アンカーでそのまま読める（`resets 7:50pm (Asia/Tokyo)`）。**ステータスバーは上限中に `--` にしない**: フッターが `5h NN%` を出さなくなるので、見出しから枠を読んで（`session limit` → 5h / `weekly limit` ・ `Opus` ・ `Sonnet` → 7d）そのメーターを 100% で埋める。**フッターが数値を出しているときは触らず**（実データが正）、**枠へ対応づけられない上限（`usage credit limit` / spend limit）は`--` のまま**にする（メーターに嘘を書かない）。旧挙動は `TAKO_1093_LEGACY=1` | M |
| FR-2.27.13 | **上限の告知は claude 自身の分類に合わせる + 解除時刻の日付を読む**（✅ 2026-09-03、#1096）。FR-2.27.12 のテンプレート（`You've hit your <名前>limit…`）は **claude が出す告知の一部でしかない**。同バイナリは阻害 / 警告 / 情報の前置きを自分で 列挙しており（`dCt` / `pCt` = 阻害、`fCt = ["You've used", "You're close to"]` = 警告、`mCt = ["You're now using usage credits", …]` = 情報）、tako は**阻害のうち時間で解けるぶんだけ**を受ける: 動詞 2 種（`hit your` / `reached your`。`You've reached your Fable limit.` は `hit` 決め打ちでは当たらなかった）+ `out of usage credits` + `org is out of usage`。**座席種別・組織の無効化・`group's usage limit is set to $0` は受けない** —— 時間では解けないので `WorkerErrorKind::UsageLimit` の助言（`wait_reset` = 解除まで待つ）が嘘になる（別種として扱う設計が必要なので切り出す）。**警告 / 情報は受けない**（まだ動けるペインへナッジを撃たない）。クレジット系は 5h / 週へ対応づけられないのでステータスバーは `--` のまま（FR-2.27.12 の「メーターに嘘を書かない」をそのまま適用）。**解除時刻は日付つきを絶対時刻として読む**: claude の時刻整形は 24 時間より先だと日付を前置きする（`Sep 8, 3pm` / `Sep 8, 3:05pm` / 年が違えば `Sep 8, 2027, 3:05pm`。codex は `Aug 28th, 2026 4:24 AM`）ため、**週枠（最大 7 日先）は日付つきが通常形**なのに時刻だけを読んで「次に来る同じ時刻」へ丸めており、解除の数日前からナッジを撃って3 回で諦めていた（#985 のテスト自体がこの丸めを 13 / 13 / 19 日早い期待値で固定していた）。暦の変換は外部クレートを増やさない自前実装（`days_from_civil` / `civil_from_days`）で、**日付なしの表記の解釈は 1 ビットも変えない**（24 時間上限も維持）。**日付が書いてあるのに読めない形は時刻だけの解釈へ落とさず「不明」にする**（時刻だけの値は確実に間違っているので、自信のある誤りより不明を返して`reset_at: null` から書式の変化に気づけるようにする。ただし不明の猶予は 15 分なので初回試行は早まる = 遅らせるには「読めない」を表す状態が必要で別の話）。旧挙動は `TAKO_1096_LEGACY=1` | M |

| FR-2.27.14 | **時間では解けない利用阻害は上限とは別種として扱う**（✅ 2026-09-04、#1106）。FR-2.27.13 が意図的に外した残り —— claude 2.1.258 の阻害の前置き（`dCt` / `pCt` / `Par`）のうち **6 分類 / 8 文言**: 座席種別（`Your seat type doesn't include usage credits` / `… include usage` / `… include extra usage`）・管理者による無効化（`Your usage allocation has been disabled by your admin`）・グループ枠 $0（`Your group's usage limit is set to $0`）・クレジット要求（`Fable 5 requires usage credits` + `Par` の総称形）・追加利用ぶんの枯渇（`You're out of extra usage`）・組織でのサービス無効（`This service is disabled for your org`）—— を **`WorkerErrorKind::EntitlementBlocked`（slug `entitlement_blocked` / 推奨アクション `needs_human`）**として検知する。#1106 前はどの規則にも当たらず `detect_worker_error` が `None` を返すため、**worker が止まっているのに `status` が idle = 「作業完了」に見え、`WORKER_IDLE` が出て `supervisor.log` にも何も残らなかった**（#1093 と同じ形の実害。しかも待っても直らないので**人が気づくまで永久に止まったまま**になる）。判定は正本の隣（`tako_core::limit_resume::entitlement_block_line`）に置き、**`is_limit_exhausted_line` には混ぜない**（混ぜると自動復帰が「解除まで待つ」で撃ち始め、試行上限まで空撃ちして諦める）。2 つの規則が**排他**であることは単体テストで固定する（種別が検査順に依らず一意に決まる）。**アポストロフィは判定に使わない**（ASCII `'` と U+2019 が同じバイナリ内で混在するため、`doesn't` / `group's` のような語はアポストロフィの前後で割った必須語の組で表す）。**自動復帰（#813）は発動しない**（`detect_limit_stop` が `None` を返す = 追跡そのものを作らない）。**supervisor（#401）は自動復旧を試みず**、監査ログと履歴へ理由を残して未復旧として上げる（`action=entitlement_blocked_report`。#983 の起動失敗と同じ `report_needs_human` 1 実装）。**ステータスバーのメーターは `--` のまま**（`exhausted_limit_window` が `None` = FR-2.27.12 / 13 の「メーターに嘘を書かない」をそのまま適用）。master の system prompt には「時間では解けない。ナッジ・待機・respawn では直らないので、`detail` をユーザーへ見せて管理者 / プラン / クレジットの対処を伝える」を載せ、**全種別が prompt に載っていること**を機械検証する（種別を増やして prompt を直し忘れると master が対処を推測する）。系統差は `agent_support::MATRIX` の `worker_entitlement_detect` として宣言する（**claude / codex / agy とも対応**。ローカル LLM はベンダーの座席・クレジット・組織ポリシーが無いので対象外）。**codex / agy の文言も同じ 1 本で受ける**（#1107 の実物調査。codex 0.153.0 = クレジット残高の枯渇・spend cap・workspace のクレジット上限の 6 実文言 / agy 1.1.25 = `AI: Out of credits` と `No license available …` の 2 実文言。座席種別・組織のサービス無効に相当する停止文言は両系統に無く、codex の構造化ソース（`rate_limits`）は枠の使用率と解除時刻しか持たないので**画面の文言が唯一の根拠**）。codex の `You've reached your workspace credit limit` は `reached your <名前>limit` のテンプレートにも当たるため、**排他は偶然ではなく構造で保証する**（`is_limit_exhausted_line` は `entitlement_block_line` に当たる行を必ず false にする = 種別が検査順に依らず一意に決まり、メーターも自動的に `--` のままになる）。旧挙動は `TAKO_1106_LEGACY=1` | M |

| FR-2.27.15 | **ペイン幅で折り返された見出しも読む**（✅ 2026-09-04、#1123）。FR-2.27.12〜14 の規則はどれも **1 論理行**を入力に取るが、claude TUI は本文を**自分で**折り返して続きを字下げする。狭いペイン（実発生は 21〜25 桁）では見出しが 3〜4 行に割れ、**どの物理行も規則に当たらない** —— 停止判定・解除時刻・ステータスバーのメーターが同時に外れ、worker 4 体が解除後 **7.5 時間**止まったまま `supervisor.log` に 1 件も記録が残らなかった（#1093 / #1106 と同じ形の実害が、文言ではなく**幅**で起きる）。折り返しは**端末のソフトラップではない**ので `tmux capture-pane -J` でも alacritty の `WRAPLINE` でも 1 本には戻らない（claude 2.1.258 を 25 桁のペインで動かし、`-J` で採っても割れたままなのを実測）。**続きの字下げ幅は内容の列とは限らない**（同じ画面で `⎿` の塊の続きが 5 桁で出るものと 2 桁で出るものを実測）ので幅の一致では判定せず、「字下げがあり、目印（`⎿ ⏺ ❯` と罫線）で始まらない非空行」を続きとみなす。結合は `tako_core::limit_resume::unwrap_wrapped_lines` の**1 箇所**だけで行い、判定する 4 経路（メーター / `detect_worker_error` / 解除時刻 / ダイアログ種別）は**物理行を先に走査し、外れたときだけ**論理行へ落ちる —— 結合は候補を増やすだけなので**折り返しの無い画面では判定が 1 ビットも変わらない**（FR-2.27.12〜14 の既存テストが不変で緑であることが受け入れ条件）。`is_limit_exhausted_line` と `entitlement_block_line` の**排他は結合後も保たれる**（同じ 1 本の文字列に両方を当てるため）。空行は塊の切れ目として結合を打ち切り、行数（12）と文字数（1000）に歯止めを置く（字下げが続くだけの領域を丸ごと 1 本へ畳まない）。旧挙動は `TAKO_1123_LEGACY=1` | M |
| FR-2.27.16 | **一括の ON / OFF と全体の既定**（✅ 2026-10-09、#1945）。ステータスバーの「スリープ防止中」の隣に一括ボタンを置き（SVG アイコン + 短い言葉。全部 ON = 「自動復帰 ON」/ 一部 = 「自動復帰 一部 N/M」/ 全部 OFF = 「自動復帰 OFF」。ツールチップに数・以後の既定・押したときの向き）、押すと**退避中を含む全エージェントのペイン**の `limit_autoresume` を揃え、**以後に立つペインの既定**（settings.json の `limit_resume_all`。既定 OFF・旧ファイルは serde default で OFF に読める）も同じ値にする。押す向きは「全部 ON なら OFF、それ以外は ON」。**エージェントのペイン** = role が master / worker / solo か、会話の検出（`claude agents --json` / codex・agy のプロセス表 = #1076 / #1238 の保持規則つき）に載っているペイン。シェル・プレビュー・実行ペインは対象外（継続ナッジをシェルに打つとコマンドとして実行される）。**以後のペイン**は、エージェントになった瞬間（spawn・role の付与・会話の検出）にまだ値が決まっていなければ既定を採る。解決順は spawn 引数 → プロファイル → 全体の既定で、プロファイルの明示 OFF は全体の ON より強い。人が個別に切ったペインは「決定済み」（ペインの非永続の印）なので次の一括まで既定で上書きしない。復元したエージェントのペインは保存時の値を保つ（決定済み）、復元したシェルは後で claude を起動したら既定を採る。**退避中のペインも `--pane N` で指定できる**（#1945 前は「ペインが見つからない」）。`orchestrator worker_status` / `read` の `limit_resume.enabled` も退避中の値を読む（#1945 前は表示中のタブしか探さず、表で ON にして退避した worker を false と答えていた = 値は消えていない）。自動復帰の駆動（2 秒 tick）も退避中を見る（端末を持つものだけ）。CLI は `tako limit-resume on\|off --all`（`--all` 単独は従来どおり一覧 + `default` / `summary`。`--all` と `--pane` は排他）、MCP は `tako_limit_resume` の `all` + `enabled`（ツールは増やさない）。集計は**描画のたびに数えない**（2 秒 tick と切り替えの直後だけ）。効き方の系統差は FR-2.27 本体（`worker_limit_autoresume` のマス）がそのまま正。A/B は `TAKO_1945_LEGACY=1`（一括は併用エラー・退避中は指定不可・既定を配らない・駆動は表示中だけ） | M |
| FR-2.27.17 | **ネット復帰の即時再開**（✅ 2026-10-15、#2112）。回線が戻ったら、**接続系のエラーで止まった tako 内の全エージェントのセッション**（worker・master・solo・手起動の claude。**退避中を含む**。全タブ・全プロファイル）へ、送達確認つきの経路（FR-2.27.4 と同じ `queue_prompt_flow`）で「続き」（【tako 自動通知】で始まる文）を送る。**全体の設定**（settings.json の `network_resume`。**既定 ON** = ユーザー判断。項目の無い旧ファイルも serde default で ON = 移行不要）で、ペインの `limit_autoresume`（#813）とは独立。**対象の判定**は画面から: 入力欄（`❯` + すぐ上の区切り線）を錨に上へ辿り、最後の会話の項目（`⏺` / `●` / `⎿`）が接続系の `API Error`（折り返しを結合した 1 論理行）で、生成中・再試行中（中断の案内）でなく、最後のターンにログイン失効（#757）・使用制限（#813）・利用阻害（#1106）の行が無いもの。**ターンの締めの後の通知（#2157）**: claude はターンの外の出来事（plugin / hooks の再読み込み等）を `⏺` の項目として会話の末尾へ足す（実測: エラーの 0.2 秒後に `tako: plugin.json changed — reloaded` が出て、最初の 2 秒 tick の時点で最後の項目がもう通知 = 2026-10-16 の DNS 断で 1 本を取りこぼした）。ターンの締めの行（`✻ Cooked for 31m 35s · done 9:20 AM` = スピナーの記号・動詞 1 語・`for`・数字の並び）より下が `⏺` / `●` の項目だけなら通知として読み飛ばし、締めの行の上の項目を見る（人の発言 `❯`・ツールの結果 `⎿` が混ざる / 締めの行が無いなら読み飛ばさない = 新しいターンは必ず締めの行で終わるか生成中）。中断の案内（生成中・再試行中）は**最後の項目より下**（スピナーの行・フッター）でだけ探す（会話の本文に写った `esc to interrupt` で止まった画面を見落とさない）。**次の停止（#2157）**: 会話が進んだ（最後の項目が人の発言・生成中）のを画面で見たら、または送った「続き」の送達が届いた（`delivered`）と決着したら、送った停止の鍵を捨てる（「続き」を受けてすぐ同じ形で止まり直すと、直前の人の発言 = 同じ「続き」・締めの行は分の精度なので鍵が前と重なり、#2112 では 2 回目の止まり直しから永久に送らなかった。歯止めは 30 分で 3 回の打ち切りのまま）。A/B は `TAKO_2157_LEGACY=1`。**締めの行より上が画面の外（#2181）**: 低いペイン（worker を並べると 20 行前後）では締めの行・通知行・入力欄だけで画面が埋まり、締めの行の上の項目（エラー）が画面の外へ押し出される（2026-10-16 の DNS 断で 1 本を取りこぼした）。見えている行が「入力欄の上は締めの行と `⏺` / `●` の項目（通知。0 個でもよい）だけで、締めの行より上に項目が無い」形のときだけ、画面の外の直近 200 行（器 = tmux / psmux のペインは器の履歴を**背景のスレッド**で読む = UI スレッドで子プロセスを起こさない。器の無い直接のペインは外側の端末の履歴をメモリから）を足して同じ判定を通す。足した材料の締めの行と通知の頭の行が見えている形と一致するときだけ採り（読むあいだに会話が動いたら止まっていると言わない）、答えはその形のあいだ覚える（止まって動かないペインで器を起こすのは 1 回。普段の 2 秒 tick は器を起こさない）。材料に tako mod の報告（ターンの終わりの理由はエラーの種類 = 接続系か 5xx かを運ばない）・会話記録（claude だけの第 2 の判定になり、画面と時間差で食い違いうる）を使わないのは、誤って送らないことを最優先にしたため。A/B は `TAKO_2181_LEGACY=1`。**接続系の範囲**は実物の文言で決めた（会話記録の `isApiErrorMessage` 300 件超 + claude 2.1.294 を到達できない API 先へ向けた実画面）: `Can't reach the API server … (ENOTFOUND)` / `Connection refused … (ECONNREFUSED)` / `Connection lost mid-response` / `Your computer went to sleep mid-response` / `The response stopped arriving` + 旧い版の `Unable to connect to API (…)` / `Connection closed` と `ECONNRESET` / `ETIMEDOUT` / `EHOSTUNREACH` 等。HTTP の状態コード（`500` / `529` 等）・`Server error mid-response`・認証 / 上限 / 課金の語を含むものは含めない（回線が戻っても直らない）。**送らないもの**: 入力欄に人の未送信の文字（文字の属性で判定 = #1297 と同じ述語）・選択ダイアログ / メニュー中・ほかの送達が走っている・**同じ停止へ 2 回目**（停止の鍵 = 直前の人の発言 + エラーの 1 論理行 + ターンの締めの行。スクロール等で追跡が途切れても同じ鍵なら送らない。会話が進んだ・送達が届いた後の同じ鍵は次の停止 = #2157）・直近 30 分で同じペインを 3 回再開した（打ち切り）。**回線復帰の検知**は到達確認（`api.anthropic.com:443` への名前解決 + TCP 接続だけ。データは送らない。`HTTPS_PROXY` / `ANTHROPIC_BASE_URL` があればその先、`TAKO_NET_PROBE=host:port` で明示）を**送ってよい停止が 1 つ以上あるあいだだけ** 1 本ずつ背景のスレッドで走らせる（断が続くほど 3 → 10 → 30 秒と間を空ける。止まったセッションが無いあいだは 1 回も走らない = 常時のポーリングをしない）。結果は 500ms ループが拾うので、復帰から送達までの遅れは到達確認の間隔ぶん。**記録**: persist.log の `[net-resume]` に停止の検知（ペインと種別の slug）・到達確認の開始 / 失敗（断の最初の 1 回）/ 停止・**回線の復帰と再開した本数・見送り（ペイン:理由）**・設定の切替（画面の文字と送った文は出さない）。**AI フルコントロール**: CLI `tako limit-resume --network [on\|off]`（単独で設定と状態）、MCP `tako_limit_resume` の `network`（読むのは `all=true` の `network` / 各ペインの `network_stop`。ツールは増やさない）、GUI は設定画面「一般」の「ネット復帰で再開」。3 経路とも dispatch の 1 実装。系統差は `agent_support` の `network_autoresume` のマス（claude のみ実測。codex / agy は実物の接続エラーの文言を採れていないので pending） | M |
| FR-2.27.18 | **解除時刻は告知を最初に見た時刻から解く**（✅ 2026-10-17、#1710）。GUI の追跡状態はメモリにしか無く、作り直すたび（GUI の再起動・スリープ明けの描き直しで告知が見えなかった tick の後）に画面の `resets 7am` を**いま**から見た次の 7 時へ解いていた。9/24 12:31 に週次上限で止まった worker が 9/25 12:43 の再起動後に `reset_at` = 9/26 07:00 で待ち、**9/25 07:00 に解けた上限を 18 時間待った**。①告知を最初に観測した時刻をペインの属性 `limit_seen`（告知の見出しの要約 = FNV-1a 64 の 16 桁と検知時刻。**画面の文字列は保存しない**）として持ち **layout.json へ永続化**する（旧ファイルは serde default で記憶なし = 移行不要。表のペインも退避中のペインも同じ経路）。告知は見出し（`is_limit_exhausted_line` に当たる最も下の 1 行。行頭の `⎿` / `⏺` と空白の揺れは落とす）で見分けるので、同じエピソードでダイアログ型と idle 型が入れ替わっても同じ記憶を引く ②画面の告知が記憶と同じなら**記憶の時刻から**解き、違えば新しい告知として今の時刻で書き換える。告知が見えない tick が続いても **10 分（`SEEN_FORGET_SECS`）までは記憶を捨てない**（再起動直後・スリープ明けの描き直しで失わない。同じ文面の告知が別のエピソードで出るには解除時刻が 1 周する = 24 時間かかる） ③**追跡を（再）開始した時点でもう解けていた上限は `その時刻 + 安全マージン` に再開する**（`resume_due`。過去の解除時刻 + マージンで即撃ちせず、TUI の解除反映・回線の戻りを待つ）。通常の検知（検知 = 追跡の開始・解除は未来）は従来と同じ値 ④画面から解いた値で追跡が始まった後に**構造化ソース（tako mod の `resets_at` / codex の rollout）の時刻が届いたら置き換える**（GUI の再起動直後は mod の報告が最初の heartbeat まで届かず、画面の値で追跡が始まっていた）。状態照会（`tako limit-resume --pane N` / `worker_status` / `read`）の `limit_resume.state` に `detected_at` を足した。純関数（`detected_at` / `resume_due` / `step_seen` / `notice_digest`）は時計を注入してテストし、検知は `tako_control::limit_stop::observe_limit_stop`。A/B は `TAKO_1710_LEGACY=1`、番犬は `crates/tako-control/tests/issue1710_limit_reset_detected_base.rs` | M |
| FR-2.27.19 | **worker_status の `api_error` も入力欄を錨に読む**（✅ 2026-10-18、#1847）。`detect_worker_error` の `api_error` は「空行を除いた末尾 15 行に `API Error`」で見ており、32 桁ではエラー 4 行・フッター 8 行・長いターンの締めの行 2 行で頭の行が下から 14 行目になる（実画面）。tmux の focus-events の案内・tako mod の帯（FR-2.42.13）・締めの後の通知行（FR-2.27.17 の #2157）のどれか 2 行で窓の外へ出て `status: idle`・`recommended_action: null` に戻っていた（9/29 の回線断で 7 体を朝まで放置。再開そのものは FR-2.27.17 が担う）。窓の規則で外れたとき（自動の再試行中でないとき）だけ、FR-2.27.17 と**同じ判定**（`net_stop::detect`）を通し、止まっていると言えるとき（生成中でない・最後の項目が人の発言でない・同じターンに失効 / 上限 / 阻害の行が無い）に `api_error` / `resume`・`detail` は折り返しを結合した 1 論理行で返す。worker_status はこの判定へ画面全体を渡す（`detect_worker_error_on`。背の高いペインでは会話とフッターのあいだが空行で埋まり、末尾 30 行からエラーの頭が欠ける）。対象は接続系だけ（5xx 等は窓の外なら従来どおり拾わない）。エラーの項目が画面の外へ押し出された低いペイン（FR-2.27.17 の #2181）は status では読まない（器の履歴を読むのは再開の側だけ）。A/B は `TAKO_1847_LEGACY=1` | M |

実装メモ（2026-08-15）: 判断は `tako_core::limit_resume`（`decide` / `due_at` /
`safe_choice` / `parse_reset_at`。すべて純関数で `now` とタイムゾーンは注入）、
検知の束ねは `tako_control::limit_stop::detect_limit_stop`、駆動は
`tako-app::limit_autoresume`（2 秒 tick から `drive_limit_autoresume`）。
プロファイル既定の解決は `tako_control::orchestrator::resolve_worker_limit_resume`
（純関数）1 本で、spawn は解決結果をペイン属性へ入れるだけ（#822。機械検証は
unit 6 本 + セルフテスト項目 117 = 適用を外すと FAILED になる）。
リセット時刻は**エピソード開始時に確定して途中で更新しない**（画面に残った古い
上限メッセージで復帰予定が後ろへずれないようにするため）。tako を上限中に
再起動して画面のメッセージだけが古いままだと復帰予定が最大で翌日まで延びていた
（#1710）ので、告知を最初に見た時刻をペインの属性として layout.json に残し、
そこから解く（FR-2.27.18）。機械検証は unit 26 本 +
セルフテスト項目 111（正例 2 型 / 負例 3 型 / 試行上限 / list・read の一致）。

### FR-2.28 codex のサンドボックス解除は明示 opt-in（✅ 2026-08-27、#981）

| ID | 要件 | 優先度 |
|---|---|---|
| FR-2.28.1 | **既定でサンドボックスを外さない**。`master_agent: codex` の master / solo と codex worker の起動コマンドに `--dangerously-bypass-approvals-and-sandbox` を付けるのは、プロファイルの `bypass_sandbox`（既定 false）が明示的に true のときだけとする。claude master に相当するフラグは無く、外部ユーザーが codex を選んだ瞬間に承認とサンドボックスが両方外れるのは系統間で非対称な既定だった（#132 は開発者 1 人の環境を前提に無条件で入れた） | M |
| FR-2.28.2 | **codex は「承認だけ外す」を選べない**（同一フラグ）。実測（codex 0.144.4 の `codex sandbox`）で 既定 = read-only は cwd 内への書き込みすら不可 / `workspace-write` は cwd 内のみ可でネットワークと cwd 外は不可 / `danger-full-access` で両方可。worker の実務（cargo・gh・data dir への書き込み）が成立しないため中間状態は用意せず 2 択にし、`skip_permissions: true` だけでフラグが付かないことを `profiles show` の `warnings` と master プロンプトで見せる（効かない設定を黙って抱えさせない） | M |
| FR-2.28.3 | **起動時に状態を 1 行出す**。外れているときは何が外れているか（フラグ名）、外れていないときは「承認プロンプトが出る」と外し方を出す。承認プロンプトで止まったときに理由が画面に残る形にする（日英） | M |
| FR-2.28.4 | **既存ユーザーの挙動を変えない**。`bypass_sandbox` を書いていないプロファイル（= 変更前に作られたファイル）には自動マイグレーション（FR-5.16 / #916）で `true` を明示する。世代判定は**キーの有無**なので `Profile::bypass_sandbox` に `skip_serializing_if` を付けない。手書きテンプレート（`ensure_defaults` の default.yaml / solo の default.yaml）も現行世代で書き出す（**書き忘れると新規インストールが移行対象になり危険な旧既定が書き込まれる**。実測で踏んだ） | M |
| FR-2.28.5 | 操作は CLI `tako orchestrator profiles set <名前> --bypass-sandbox <bool>` / MCP `tako_orchestrator_profiles` の `bypass_sandbox` / 設定画面 → プロファイル → 「codex のサンドボックスを外す」の 3 経路が**同じ dispatch** を通る（設計原則 5）。値は `profiles show` が常に返す（安全に関わる設定なので既定値でも隠さない） | M |

実装メモ（2026-08-27）: 判定は `Profile::bypass_sandbox` 1 本で、master は
`orchestrator::build_master_cmd_in`、worker は `ResolvedWorkerLaunch::allow_sandbox_bypass`
→ `agent::build_worker_cmd_in` が読む。文言は `orchestrator::sandbox_bypass_line`（`Note` =
日英）と `sandbox_bypass_hint_command`。移行は `migrations` の profiles v2 → v3
（`pin_sandbox_bypass`。`once: false` = `once_markers` が from を問わず「当たった」と
答えるため #27 を通った利用者が飛ばされるのを避ける）で、solo プロファイルも同じ spec
に載せた。A/B は `TAKO_981_LEGACY=1`（同一バイナリで無条件バイパスへ戻す）。
機械検証は unit 12 本（両方向・claude / agy の回帰ゼロ・テンプレート番犬）+
実機 e2e（codex 0.144.4 / gpt-5.6-sol: 既定は MCP ツール承認ダイアログで停止しファイル
未作成 / opt-in はコマンド実行まで通りファイル作成）。

### FR-2.29 agent 能力マトリクス（✅ 2026-08-27、#982。エピック #975 / 設計の写し元は #515 / #591）

> tako は claude を基準に実装してきたため、codex / agy では落ちる機能・まだ使えない機能がある。
> その判断が**コードのあちこちへ散っている**（棚卸し #975 §1: 監視・送達・resume・setup・MCP は
> `WorkerAgent` を通らず `"claude"` 文字列か claude 固有モジュールへ直結）ので、
> 「どの系統でどこまで使えるか」を機械可読の正本 1 本にする。OS 軸（`platform::support`）で
> 一度解いた問題の agent 軸版。

| ID | 要件 | 優先度 | 状態 |
|---|---|---|---|
| FR-2.29.1 | **正本は 1 箇所**（`tako-core::agent_support`）。`AgentFeature { key, summary, claude, codex, agy, local, evidence }` の表で、状態は `Supported` / `Degraded` / `Pending` / `Unsupported` の 4 値（`platform::support::Support` と同じ意味） | M | ✅ |
| FR-2.29.2 | **根拠なき断定を禁止**。claude 以外について `Supported` / `Degraded` / `Unsupported` を宣言するなら `AgentEvidence`（コード本文の引用 / 上流の仕様 / 実測 / テスト名）を持つ。持たずに倒すとテストが落ちる（#591 の T7 と同じ構造） | M | ✅ |
| FR-2.29.3 | **「上流に手段が無い」と「まだ調べていない」を混ぜない**。未調査は `Pending`（追跡 Issue つき）に置く。未調査を `Unsupported` へ倒すと、宣言を読む AI が open な道を永久に避ける | M | ✅ |
| FR-2.29.4 | 縮退の理由文は `Note`（日英対）で**マトリクス 1 箇所に定義**し、診断・docs・将来の system prompt がすべてそこから引く（#435 / #516 と同じ方針） | M | ✅ |
| FR-2.29.5 | **同じ概念の enum が 5 つ並存している**現状（`WorkerAgent` / `SetupAgent` / `agents_sync::AgentKind` / `agent_install::AgentKind` / `LimitService`）を統合ではなく**対応の機械検証**で縛る。どれかに値が増減したらテストが落ちる。値の集合が本当に違う（`Local` は `WorkerAgent` に無いのが正しい）ため統合しない。対応表は `.agent/agent-enums.md` | M | ✅ |
| FR-2.29.6 | 操作は CLI `tako agent-support` + MCP `tako_agent_support` で 1:1（開発不変条件）。**CLI 側はローカル処理**（GUI が無くても引ける = 「worker が動かない」を調べるときに tako 本体が生きている前提を置かない。`tako platform` と同じ判断） | M | ✅ |
| FR-2.29.7 | docs（`docs/src/content/docs/agent-support.md`）は**生成物**で、コミットするのは能力マトリクスの写し `docs/src/data/agent-support.json`（ページは docs のビルドが写しから組む。並列の PR が件数の行で衝突しないため = #2104）。CI が `node scripts/gen-agent-support-docs.mjs --check` で写しの同期を検査する。カテゴリ未分類の能力があると生成が失敗する（分類漏れの検出） | M | ✅ |
| FR-2.29.8 | 能力判断の呼び出し側は**各スライスがその機能を実装するときに**マトリクス経由へ寄せる（一斉置換しない）。#982 では `WorkerAgent::has_agents_api()` を吸収した（`TAKO_982_LEGACY=1` で吸収前へ戻せる）。残りの寄せ先一覧は `.agent/agent-enums.md` | M | ✅ |

初期値（40 能力 × 4 系統）は棚卸しレポートの §1 / §9 から引いた。**実機を動かしていない**ため
根拠は原則としてコード本文の引用と既存 Issue の実測記録に限り、実機で確かめるべきものは
`Pending` + 追跡 Issue のまま置いている（過大申告しない。#591 / #617 と同じ基準）。

### FR-2.30 エージェント起動の失敗は必ず分類して返す（✅ 2026-08-28、#983）

| ID | 要件 | 優先度 |
|---|---|---|
| FR-2.30.1 | **agent CLI の実在をペインを作る前に確かめる**。`codex` / `agy` / `claude` の実行ファイルが無い状態で spawn すると、従来は組み立てたコマンドがそのままシェルへ流れ、ペインに `command not found` が出るだけで tako は**成功**と報告していた（送達検査 #640 は「エコーが返って実行された」までしか見ず、`registry::prompt_delivery_assessment` は claude 以外を `NotApplicable` で即返す）。検査は境界 B16（`platform::exe::find`。#898）1 本で行い、claude は #868 の設置先も見る | M |
| FR-2.30.2 | **失敗は「理由 + 次の一手」で返す**。何が無いかだけでなく、公式の導入コマンド（実物で確認した形だけ。無い agent は「導入して PATH へ通す」の言い換え）・参考 URL・`tako setup` の案内を含める。文言は日英で、種別は機械可読（`kind`）にする。将来の系統（未認証 / 事前信頼の書き込み失敗 / 起動直後の異常終了 / ローカル LLM 未起動）を足す前提で enum にしておく | M |
| FR-2.30.3 | **失敗しても副作用を残さない**。ペイン分割・レジストリ登録・委任台帳より前に落とすので、空ペインも active エントリも残らない（不正 agent の検証と同じ位置） | M |
| FR-2.30.4 | 対象は agent CLI を起動するすべての経路: worker spawn（`orchestrator run` / checkpoint resume も同じ経路）/ `tako master` / `tako solo` / 引き継ぎの後任 master（#193）/ コンフリクト解消エージェント（#496）。**後任 master が黙って死ぬと前任を閉じる主体が居なくなる**ので、引き継ぎは特に起動前に落とす | M |
| FR-2.30.5 | 解決した実行ファイルを応答（`agent_path`）に出す。「どの binary を起動したか」が読めれば、PATH の食い違い（#498 / #772 の stale claude）と CLI 不在を利用者が切り分けられる | S |

| FR-2.30.6 | **送達の観測手段は系統ごとに違うので、マトリクス（FR-2.29）から引く**。旧実装は `registry::prompt_delivery_assessment` が `agent != "claude"` を `NotApplicable` で即返しており、**観測手段が無い = 何も言わない**になっていた。一次シグナルを持つ系統（claude の transcript / codex の rollout）だけが未達を断定し、持たない系統（agy）は `unverified`（未確認）+ `verify_then_resend` を返す。**未達と断定しないので supervisor の自動再送は撃たれない**（撃つと同じ依頼を二度渡す = #1015 の事故） | M |
| FR-2.30.7 | **ターンが走った証拠は画面検証の失敗より強い**。codex の rollout に `task_started` があれば、画面の送達確認が失敗と記録していても `delivered` へ覆す（ターンは投入されたプロンプトでしか始まらない）。`response_item` の `role: "user"` は数えない（rollout の先頭に指示文が同じ形で載りうるため、起動直後に「届いた」と誤る側へ倒れる） | M |
| FR-2.30.8 | **起動失敗は画面からも分類する**。CLI 不在（`command not found` に agent 自身の名前が同居）・未認証（実採取の文言のみ）を `status = "error"` / `error.kind = launch_failed` / `error.launch_problem` として返す。**送達の証拠がまだ無い worker に限る**（仕事が始まった worker の scrollback には agent 自身が実行したコマンドの `command not found` が普通に流れる）。supervisor は自動復旧を試みない（同じ失敗を繰り返すだけなので未復旧として上げる） | M |
| FR-2.30.9 | **spawn を止めない失敗も黙らない**。事前信頼の書き込み失敗は致命ではない（tako が信頼ダイアログを検出して承諾する）が、そのぶん最初の指示が届くまで遅くなるので、応答の `warnings[]` に「分類 + 理由 + 次の一手 + 生の詳細」を載せる | S |
| FR-2.30.10 | **起動コマンドは消していない行へ重ねて打たない**（#1940）。起動コマンドの送達（#640 の `shell_send`）は全文一致でエコーを確かめるが、worker ペインが 2〜3 行まで潰れると zsh は入力行を 1 行の横スクロール（`<…`）/ `>....` に畳むので**全文が一度も画面に出ない**（実測）。旧実装はそこで書き直しを使い切り、最後に**消していない行へ**本文 + Enter を書き足して `--permission-mode autoexport` に化け、起動先が化けた引数と正しい引数で 2 回起動していた（10/9 本番の 4 件・隔離の実測で再現）。①寸法から全文を出せないと分かるとき（`echo_unviewable`: 3 行未満 / 本文が「行数 − 1」× 桁数を超える）は書き直さず、反映が落ち着いてから Enter を 1 回だけ送る ②書き直しを使い切った後の書き切りは必ず Ctrl+C で行を捨ててから ③実行はシェル統合の印（OSC 133;C の回数）で確かめる（画面の見え方に依らない）④同じプロセスの別ペインで印を観測済みなら、新しいペインは最初のプロンプトの印（direnv 等の precmd の後に出る）を待ってから書く（上限 15 秒。2 本目以降のフロー = ssh 後の `cd` 等には掛けない）⑤起動フックの最中に打った Enter は先行入力なので、画面の変化（kernel のエコー）を実行とみなさず Enter も撃ち直さない。A/B は `TAKO_1940_LEGACY=1` | M |
| FR-2.30.11 | **依頼文が届く前に `delivered` と言わない**（#1940）。`prompt_delivered_at` を書くのは送達フローの確認（貼り付けの反映 + 送信後の空 / peer の受信確認）だけ。会話の検出（`claude agents --json` は起動直後に返す）は「起動した」証拠に留め、猶予内は `pending`・猶予後は `unverified`（自動再送は撃たない）。旧実装は会話の検出で `delivered` → 送達フローが諦めた後に `undelivered` と反転した | M |
| FR-2.30.12 | **起動したエージェントがすぐ終わったら待たずに失敗として出す**（#1940）。起動コマンドを送った後にシェル統合の終了の印（OSC 133;D）が来て TUI も動いていなければ、依頼文の送達を `agent_exited` で決着し、`workers` の `launch=failed` / `launch_failure=agent_exited` / `launch_exit_code` に出す。起動コマンドを送り届けられなかった打ち切りは `command_flow_timeout`。起動に失敗した worker には `resend_command` を出さず、`worker_status` も `launch_failure` / `launch_exit_code` を返して再送の引き金（`prompt_undelivered` = supervisor の自動再送）を出さない（出すと依頼文がシェルへ打ち込まれる）。統合が効かないシェルでは判定できない（`pending` のまま） | M |
| FR-2.30.13 | **入力欄を描けない短いペインでは peer 送達だけを試し続ける**（#1940）。claude は 2 行で空白・3 行でステータス 1 行だけになり（本番の実測。器の tmux 側も同じ寸法なので capture へ落としても読めない）、旧実装は `no_input_box` のまま 120 秒で諦めていた。8 行未満で入力欄が見えなければ貼り付けは使わず、peer 送達（#790。画面に依らない）を 5 秒おきに試す。届かなければ `pane_too_short` で決着する（ペインを広げるまで送れない）。行数の下限を保証するレイアウト側の手当ては別件 | M |

実装メモ（2026-08-27 → 2026-08-28 に変更 2 / 3 を追加）:
`tako_control::orchestrator::agent_cli`（`AgentCliProblem` / `guidance` / `locate` / `preflight` /
`detect_launch_failure` / `problem_message_in`）。文言は `Note`（日英）で、`message_in(lang)` が
純粋関数なので**表示言語グローバルに触らず検査できる**（#608 / #807 の競合対策と同じ作法）。
claude の導入手順は境界 B17（`platform::agent_install`）の正本から引き、ログインのコマンドは
`agent_cli::auth_command` が正本（#1002 の `agent_models` もここを引く = 二重管理しない）。
`cfg(test)` では実探索をしない（spawn 系テストの成否をその機械に agent CLI が入っているかに
左右させない・unix の実探索はログインシェルを起動する。探索そのものは
`tako_core::platform::exe` のテストが担保する）。

失敗の種別は **6 つ**（`cli_not_found` / `not_authenticated` / `trust_write_failed` /
`exited_immediately` / `local_runtime_down` / `local_model_missing`）で、**4 系統 × 起こりうる
組み合わせ**の文言をテストが総当たりで固定する。起こりえない組み合わせは `applies_to` で宣言して
外す（claude に「`ollama serve` を確認」と言わない / ローカルモデルに「ログイン」と言わない）。
**`exited_immediately` / `local_*` は語彙と文言だけがあり、検知の配線はまだ無い**
（ローカル LLM の起動経路そのものが #990 / #991 待ち）。

送達の観測手段は `tako_core::agent_support::delivery_observation`（マトリクスの
`WORKER_STATUS_STRUCTURED` を引く純粋関数）。A/B は `TAKO_983_LEGACY=1` で
**変更 1 / 2 / 3 がまとめて旧挙動へ戻る**。

### FR-2.31 codex worker の状態監視を claude 同等へ（✅ 2026-08-27、#984。エピック #975）

> 「codex worker は不安定」という体感の正体は**完了検知の遅さと誤爆**だった。
> claude だけが一次シグナル（`claude agents --json`）を持ち、他系統は画面推定に落ちるため
> 確定に要する連続一致が 8 回（claude は 3 回）で、さらに弱マーカーを全系統の和集合で
> 当てていたので `(Thinking)` 型の誤爆が起こりうる状態だった。

| ID | 要件 | 優先度 | 状態 |
|---|---|---|---|
| FR-2.31.1 | **codex の構造化された状態を読む**。`$CODEX_HOME/sessions/**/rollout-<ts>-<thread_id>.jsonl` の `task_started` / `task_complete` を一次シグナルとし、`status_source` に `codex-session` を足す。`need_streak` は既存の「画面推定以外は 3」規則にそのまま乗る | M | ✅ |
| FR-2.31.2 | **一次シグナルは権威**。`codex-session` も `agents` / `agents-auto` と同じく `has_children` で覆さない。エージェント CLI の TUI 自身がペインシェルの子なので、これを外すと idle が必ず busy へ上書きされ構造化ソースの意味が消える（#571 で claude について踏んだ形） | M | ✅ |
| FR-2.31.3 | **ターン未実行なら何も言わない**。rollout が無い / ターンのイベントが 0 件のときは `status` を `unknown` にし `status_source` を `screen` へ降格する。ここで idle と言うとプロンプト投入前を完了と誤認する | M | ✅ |
| FR-2.31.4 | **解決は 1 ペイン 1 回**。ペイン → codex プロセス → 開いているロック（`thread-writer-locks/<thread_id>.lock`）で thread_id を引く。`lsof` は 40〜70ms かかるので sticky に持ち、ペインが消えたら捨てる（#772 / #779 / #816 で削った subprocess を戻さない） | M | ✅ |
| FR-2.31.5 | **弱マーカーを agent 別に分離**する。和集合を全系統へ当てるのをやめ、`(Thinking)`（agy フッターのモデル名・常時表示）を agy の判定から構造的に外す。**強マーカーは和集合のまま**（誤検知の実例はすべて弱マーカー由来で、分けると推測外れ時に本物の busy を見落とす） | M | ✅ |
| FR-2.31.6 | codex の transcript アダプタ。rollout の `response_item`（`role == "assistant"`）を読み `orchestrator report --messages N` が codex でも実データを返す。応答の `transcript_agent` でどちらを読んだか分かる | M | ✅ |
| FR-2.31.7 | agy は画面推定のまま。会話が SQLite（`~/.gemini/antigravity-cli/conversations/<id>.db`）で読むには新しい依存が要るため、可否と理由を能力マトリクス（FR-2.29）へ根拠つきで載せる | M | ✅ |

`TAKO_984_LEGACY=1` で構造化ソースを無効化し画面推定のみ（旧挙動）へ戻せる。
実測は Issue #984 のコメント（同一タスクの A/B と 3 系統の比較）。

### FR-2.32 setup のエージェント選択とモデルの実取得ピッカー（✅ 2026-08-27、#1002。エピック #975）

> `tako setup` はモデルを「各 CLI の既定値」へ任せるだけで（#27 で `[1m]` 固定をやめた経緯）、
> **どのモデルが選べるのかを一度も見せていなかった**。選ぶには CLI の中へ入って `/model` を
> 叩くしかなく、tako の外の知識が要る状態だった。エージェント選択も**導入済みの CLI しか
> 選択肢に出ない**ので「他にもある」ことが利用者に伝わらない。

| ID | 要件 | 優先度 | 状態 |
|---|---|---|---|
| FR-2.32.1 | **モデル一覧は各 CLI の一覧コマンドから実取得する**。argv の正本は `agent_models::catalog_argv`（codex = `codex debug models` / agy = `agy models`）で、表示・実行・テストが同じ 1 本を見る。パーサは純粋関数で、実出力を fixture に固定する（上流が知らないフィールドを増やしても壊れない） | M | ✅ |
| FR-2.32.2 | **一覧コマンドを持たない系統は静的リスト + 取得不可の明示**。claude は該当サブコマンドが無い（`claude models` は**プロンプトとして解釈される** = 実測）ので、同梱の**エイリアス**（`opus` / `sonnet` / `fable`。`claude --model` の help が documented している語）を並べ、`failure.kind = no_list_command` で実取得でないことを言う。バージョン付き id は焼かない（エイリアスは「その系統の最新」を指すので陳腐化しない） | M | ✅ |
| FR-2.32.3 | **取得の失敗は必ず分類する**。`cli_not_found`（未導入。導入手順は #983 の `agent_cli` から引く = 文言を二重に持たない）/ `not_authenticated`（未ログイン）/ `no_list_command` / `command_failed` / `parse_failed` の 5 種で、どれも「理由 + 次の一手 + 生の詳細」を日英で返す。**「取得不可」を 1 種類へ潰さない**（#982 の `Pending` と `Unsupported` を混ぜない規約と同じ思想） | M | ✅ |
| FR-2.32.4 | **effort はその系統・そのモデルが受け付ける語彙から選ばせる**。語彙の正本は `WorkerAgent::effort_options`（起動時に実際に使う値）で、codex は**モデルごとに違う**（実測: `gpt-5.6-luna` は `ultra` を持たない）ので `supported_reasoning_levels` を優先する。**agy にも `--effort low\|medium\|high` が実在する**（実測 1.1.22: 不正値で CLI 自身が `invalid --effort "bogus" (valid: low, medium, high)` を返す）ので渡すようにし、能力マトリクス（FR-2.29）の `effort_control` を根拠つきで Supported へ倒す。旧挙動は `TAKO_1002_LEGACY=1` | M | ✅ |
| FR-2.32.5 | **未導入の系統も選択肢に並べる**。番号は検出済みの後ろに続き、選ぶと #983 の「理由 + 次の一手（導入コマンド・参考 URL）」が返る。「選べない」ことと「存在しない」ことを混ぜない | M | ✅ |
| FR-2.32.6 | **対話ピッカーは `--review` だけ**。標準 `tako setup` は #262 で「最終サマリだけの質問ゼロ」にしたので、そこへ質問を足すのは回帰。標準経路は**いま何が選ばれているか + 選択肢の件数 + 変えたいときの最簡コマンド**（`tako setup models` / `tako setup --review`）を出すだけにする（#322） | M | ✅ |
| FR-2.32.7 | **1 番は常に「CLI の既定に任せる」**。tako がモデルを固定しない既定を守る（#27 / #67 の教訓）。選択の反映はロック付き read-modify-write（#169）で **model / effort（と選択系統の `worker_agents` 項目）だけ**を書くので、既存プロファイルの他のカスタマイズは壊れない = 再実行の冪等性 | M | ✅ |
| FR-2.32.9 | **GUI も同じ候補から選べる**（設定画面 → プロファイル）。model 行に「候補を取得」を置き、**押したときだけ** background で一覧を取ってチップにする。**タブを開いた時点では取らない**（`agy models` はネットワーク取得で実測数秒。同期で呼ぶと #168 / #212 と同じ「画面が固まる」を作る）。自由入力は残す（上流のリリースで増えたモデル名を打てないと困る）。実取得でない一覧には「同梱の既知モデル」と添える。押下は既存の `commit_profile_field` = CLI / MCP と同じ dispatch を通る | M | ✅ |
| FR-2.32.8 | **1:1 公開**: 一覧の読み取りは CLI `tako setup models [--agent <系統>] [--json]` / MCP `tako_setup_models`（どちらも GUI 不要のローカル処理）。**反映は既存の 1 経路**（`tako orchestrator profiles set … --model … --effort …` / MCP `tako_orchestrator_profiles`）に任せ、新しい書き込みツールを増やさない（応答の `apply_command` が形を示す） | M | ✅ |

実測は Issue #1002 のコメント（3 系統の取得コマンドと実出力・失敗 3 種の分類・冪等性）。

FR-2.8〜2.32 はいずれも設計原則 5（AI フルコントロール）の不変条件に従い、
対応する MCP / CLI 操作（表示・読み取り・応答送信）を同時に提供する。
提示系の体験は FR-2.7（AI 成果物プレゼンテーション）と一体で設計する。

### FR-2.33 依存導入は案内で終わらせず実行を代行する（✅ 2026-09-01、#1057。基盤は #868 / #88 / #525）

> #868 のゼロスタートは **macOS だけ**実行代行で、Windows は「状態照会と公式手順の案内」で
> 止まっていた（`InstallRecipe.tako_can_run = false`）。実機の利用者は
> `error: この環境では自動インストールに対応していません。` で詰まった（Issue #1057 の報告）。
> 併せて棚卸しで、#88 が入れた任意依存のその場導入が **#262 の質問ゼロ化以降
> `run_dependency_check(false)` 固定で一度も通らない死んだコード**になっていたことが判明した。

| ID | 要件 | 優先度 | 状態 |
|---|---|---|---|
| FR-2.33.1 | **Windows でも公式インストーラを tako が走らせる**。取得は curl / wget / PowerShell の `Invoke-WebRequest` の順、実行は `install.ps1` を `-ExecutionPolicy Bypass -File` で。公式の `irm \| iex` は文字列を食わせるので ExecutionPolicy の対象外だが、**ファイルへ落として `-File` で走らせる経路は既定 `RemoteSigned` に弾かれる**（実機実測）。`Bypass` はプロセス限定でマシンの設定を変えない | M | ✅ |
| FR-2.33.2 | **インタプリタと取得物の見分け方はプラットフォーム境界がデータで持つ**（`agent_install::InterpreterSpec` / `ScriptSignature`）。`install.ps1` は shebang を持たないので「先頭が `#!`」では判定できない。判定は純粋関数で、**macOS から Windows 側の形も検証できる** | M | ✅ |
| FR-2.33.3 | **PATH の通し方も OS で別**。unix はログインシェルの profile（#868）、Windows は**ユーザー環境変数 `Path`**（`HKCU\Environment`。境界 B23 = `platform::user_path`）。末尾へ追記し、値の種別（`ExpandString`）を保ち、`undo-path` で元の値へ戻す。**他のエントリは 1 つも触らない** | M | ✅ |
| FR-2.33.4 | **Windows の PATH 判定に `exe::find` を使わない**。境界 B16 は PATH の外（`~\.local\bin` 等）まで走査する保険なので、これを on_path の判定に使うと「PATH に無いが tako からは見つかる」を「PATH に在る」と誤答し、**PATH を通す段が丸ごと飛ぶ** | M | ✅ |
| FR-2.33.5 | **自動導入が通らなければ別のエージェント CLI へ引き継ぐ**。導入済みの codex / agy が居れば候補と指示文（何を・どこへ・どうやって + 禁止事項）を返し、端末があれば確認のうえ起こす。1 つも居なければ従来の公式コマンド案内へ落ちる。**認証は代行させない**（ブラウザ操作が要るのでユーザーへ依頼させる）。代行のあとは「入ったか」を確かめてから続行する（報告を信じない） | M | ✅ |
| FR-2.33.6 | **任意依存のその場導入を復活させる**。依存表と導入実行の正本は `setup_deps` 1 か所で、CLI（`tako setup deps [install]`）・MCP（`tako_setup_deps`）・`tako setup` 本体が同じ実装を通る（**#1499 で標準経路も聞くようになった**。復活当初は `--review` だけで、実機の利用者には検出止まりのままだった = FR-2.33.10） | M | ✅ |
| FR-2.33.7 | **代行するのは実測済みの手段だけ**。macOS = `brew install`、Windows の winget は**案内のみ**（`can_run: false`）。導入済みは触らない（冪等）。手段が無い・代行できない・導入器が無いものは実行せず理由つきで `skipped` に載せる（黙って飛ばさない）。導入後は「引けるようになったか」を確かめてから成功と言う | M | ✅ |
| FR-2.33.8 | **依存の名前はプラットフォームで変わる**。永続化の器は macOS = `tmux` / Windows = `psmux` で、`tmux` を探すと器が動いていても「見つかりません」になる。器の解決は `exe::find` → `backend::binary()`（`TAKO_TMUX_BIN` / 既知の置き場 / 別名の psmux を拾う）の順で、**検出と導入後の確認が同じ規則を通る** | M | ✅ |
| FR-2.33.9 | **ブラウザ操作待ちのプロセスは tako が起こさない**（#1129）。`tako setup` の認証段は `claude auth login` を**実行せず案内だけ**を出して止まる（文面の正本は `setup_bootstrap::auth_instructions_for`、コマンドの正本は #983 の `agent_cli::auth_command`。#989 で 3 系統ぶんになった）。理由は寿命の持ち主が居なくなること: ログインは自分では終わらないので tako が `.status()` で待つ形になり、Windows は子プロセスの終了要求（#1067 の境界 B5）が未実装なのでペイン close も隔離インスタンスの終了も**孫を回収しない**（実機で 1 日 46 本・CPU 100%。#2454 でペインと言語サーバの孫は Job で回収するようになった = FR-2.2.18。起こさない方針はそのまま）。**stdin が端末かで「人が見ているか」は判別できない**（セルフテストがペインへ打ち込む `tako setup` も PTY 上では端末に見える）ので条件で絞らず構造的に起こさない。番犬は `crates/tako-control/tests/agent_auth_launch_watchdog.rs`（argv として `auth` + `login` を渡す形をソース走査で禁じる。A/B 用の 1 か所だけ `watchdog-allow`）、run ごとの機械検証はセルフテスト項目 144（**tako の直接の子**になっているエージェント CLI が 0 件。判定は純粋関数 `platform::procinfo::agent_children_of_tako` なので macOS から Windows の名前も検査できる）。**祖先ではなく直接の親だけ**を見る（器なし構成ではペインのシェルが `tako-app` の子になるので、祖先を辿るとシェルから正しく起動したエージェントまで拾う）うえ、**そのインスタンスのペイン配下へ絞る**（`agent_children_of_tako_under`。`tako → claude` は #391 の setup 対話エージェントとして本番でも正当に存在するので、機械全体で見ると人が `tako setup` を開いているだけで落ちる = 開発機で実測） | M | ✅ 2026-09-04 |
| FR-2.33.10 | **標準 `tako setup` が未検出の CLI 依存をその場で入れる**（#1499）。エージェント起動前のチェック段で、代行できる未検出依存ごとに「**何を・どの導入器で・どこへ入れるか**」（`導入: brew install tmux（導入器 /opt/homebrew/bin/brew → /opt/homebrew/bin へ入ります）`）を見せて `tmux をインストールしますか？ [y/N]` を出し、y で導入 → **同じ実行の中で再検出**して `[OK] tmux: …（インストール完了）` へ変える。再検出は検出と同じ規則（`setup_deps::resolve`）を通す（FR-2.33.8）。**`--yes` は同意扱いで導入まで進む**（`-y` の一般的な意味。bootstrap 段が `--yes` で claude を実際に入れる先例と体験を揃える）。**非 TTY では聞かずに案内 1 本へ落として止まらない**（GUI の初回起動・パイプ経由）。N・導入失敗・「入れたのに見つからない」はすべて `いま入れる: tako setup deps install` へ落として **setup は完走**する。`tako setup --check` は読み取り専用で何も入れない。**#262 との線引き**: 質問ゼロが守るのは**設定値の個別確認**（前回値の再確認・プロファイルの選び直し）で、「無い物をいま入れるか」はそれに当たらない（入れない限り機能が欠けたままで、次に打つコマンドを利用者が自分で考えることになる = #322 の逆）。FDA・スリープ設定の見直しは従来どおり `--review` だけが対話になる。判断は**理由を返す純粋関数** `setup_deps::offer_for`（`Ask` / `AutoInstall` / `Guide(理由)`）の 1 本。**#1524 で表示と入力も含めて `setup_deps::offer_and_install` の 1 実装へ寄せた**ので、`tako setup` と `tako remote setup` は同じ文面・同じ落とし方を共有し、呼び手が渡すのは段組みの字下げだけ（体験を 2 か所で組み立てない）。A/B は `TAKO_1499_LEGACY=1`（標準 setup が聞かない = #1499 前） | M | ✅ 2026-09-22 |
| FR-2.33.11 | **`tako remote setup` の Tailscale 導入も同じ 1 実装を通る**（#1509）。ウィザードの [1/5] は `brew install tailscale` を**自前で提案・実行**していたので、「brew が無い」「非 TTY」「入れたのに引けない」の扱いが標準 setup と割れていた（実測: brew が無い機では**聞いてから起動に失敗して中断**／非 TTY は EOF を N と読むだけ／どこへ入るかを出さない）。案内 → `[y/N]` → 導入 → 再検出のひと続きを `setup_deps::offer_and_install`（字下げだけ呼び手が決める）へ寄せ、**導入器のプロセス起動は `setup_deps::run_installer` の 1 か所**にした。代行できるときは入れ方を並べない（#322）／代行できないときだけ依存表の hint（App Store 版を含む）を理由つきで出す。MCP `tako_remote_setup`（非対話）は従来どおり導入せず不足を返す。番犬 = `crates/tako-control/tests/issue1509_dep_install_watchdog.rs`、実経路テスト = `bash scripts/test-remote-setup-deps-1509.sh` | M | ✅ #1509 |
| FR-2.33.12 | **tailscale の「在るか」は remote と同じ検出の正本で決める**（#1797）。依存段（`setup_deps::resolve`）は tailscale を **PATH だけ**（`exe::find`）で探していたので、CLI が PATH の外にある App Store 版 / GUI 版（`/Applications/Tailscale.app/Contents/MacOS/Tailscale`）を「見つかりません」と判定し、`--yes` では確認を省いて `brew install tailscale` まで進んだ（2 系統の同居 = #1038 で tako remote が 502 になる条件）。同じ setup の末尾の 1 行（`remote_setup::check_status` → `find_tailscale`）は正しく見つけるので、**1 回の実行の中で 2 つの検出が食い違っていた**。正本は `tailscale::detect_tailscale` の 1 本で、3 値を返す（#1816 で「確かめられなかった」を足して 4 値 = FR-2.33.13）: `Runnable`（`--version` が上限内に正常終了）/ `Unrunnable`（起動できたが固まる・非 0）/ `Absent`（どの候補も起動できない）。実行できる候補があれば先頭のそれを採る（#1797 前の `find_tailscale` と同じ答え）。remote（`find_tailscale`）は `Runnable` だけを使い、依存段は `Runnable` と `Unrunnable` を**導入済み**と読む（在るのに「未導入」と言うと 2 系統目を入れさせる）。末尾の 1 行は `Unrunnable` を「実行できませんでした」と言い分けて導入を勧めない（#1507 まではこの言い分けを 2 つの検出の食い違いで作っていた）。素の名前（PATH の `tailscale`）で見つかったときだけ表示用に絶対パスへ直す（判定には使わない）。`tako setup --check` / `tako setup deps` / MCP `tako_setup_deps` も同じ答えを返す。`tako remote setup` の [1/5] も `Unrunnable` を「未導入」と言わず「実行できません（パス）」で止め、導入口へ渡さない（渡すと導入口は在るものを導入済みとして飛ばすので、「確認を省略してインストールします」と言ったまま何もせずに終わる）。併せて tailscale コマンドの待ち（`run_tailscale_within`）を `tako_core::probe::output_with_timeout` の 1 実装へ寄せた（自前の待ちは子の終了後に吸い出しを join していたので、tailscale が孫を残して終わると上限を持たずに固まった = FR-2.14.13 の規約）。番犬 = `crates/tako-control/tests/issue1797_tailscale_detect_single_source.rs`、実経路テスト = `bash scripts/test-setup-tailscale-detect-1797.sh`（隔離 HOME + tailscale / brew スタブで 55 項目。`TAKO_BIN=<#1797 前の tako>` で 25 項目が落ちる） | M | ✅ #1797 |
| FR-2.33.13 | **tailscale の検出の続き: 「在るが動かない」を全経路で言い分け、検出を使い回し、ログインシェルの PATH も見る**（#1816）。① `remote_setup::check_status` の `tailscale` 項目の `status` は正本の答えをそのまま出す: `ok` / `unrunnable`（在るが `--version` が固まる・非 0）/ `missing` / `unknown`（④で無いと確かめられなかった。打ち切りは `timeouts` にも載る）。GUI のリモートパネル（`setup_blockers` は `ok` 以外を不足に数え、ラベルは `setup_item_label(item, status)` で言い分ける）と MCP `tako_remote_setup`（check はこの JSON、run の `tailscale_detect` 段も同じ語彙 + 「入れ直す前に、Tailscale の起動し直しか更新を試してください」= `tailscale::UNRUNNABLE_HINT`）は在るものを「未導入」と言わない（言うと 2 系統目を入れさせる = #1038）。② `tako setup` は依存段で 1 回引いた答え（`DepStatus::detection`）を末尾の 1 行（`setup_summary_lines`）まで使い回す。`--version` が固まる CLI で上限 10 秒の検出が 3 回走り setup が約 33 秒に延びていた（実測）のが 1 回（約 11 秒）になる。この段で導入したときだけ導入後の状態を引き直す。`tako remote setup` の [1/5] も検出 1 回（20 → 10 秒）。③ 依存段・`tako setup deps`・`tako setup --check` は「在るが動かない」を `[OK]` と言わず `[警告] tailscale: 在りますが実行できません（パス）` + 次の一手で出す（印は診断の `Warn` = 壊れている。新しい印は作らない）。`tako setup deps --json` / MCP `tako_setup_deps` は `runnable`（true / false / null = 確かめていない・無い）を返す。**残り作業（末尾の「残り N 件」）には数えない**: tailscale は任意依存で、FR-2.41 の線引き（残りに数えるのは整わないと tako が動かない詰まりだけ。任意の上積みを数えると本当に要る残りが埋もれる）と同じ扱い。入れ直しも勧めない。④ 正本は固定の候補で動く CLI が見つからなかったときだけ、素の名前 `tailscale` を**最後に**ログインシェルの PATH で引き直す（`exe::find_bounded` = 上限つきの 1 実装。#1532 / #1533）。`.app` を Dock から起こした痩せた PATH では MacPorts / nix の置き場がプロセスの PATH に無く、GUI のパネル・MCP・MCP `tako_setup`（GUI の環境のまま `tako setup --yes` を起こす）が「未導入」と判定し、`--yes` では brew 版まで重ねて入れていた（痩せた PATH + ログインシェルにだけ在るスタブで `tako setup --yes` が `brew install tailscale` を 1 回呼ぶのを実測）。固定の候補で見つかる人（App Store 版・brew 版）にはログインシェルを起こさない。ログインシェルが答えた置き場が固定の候補と同じなら試し直さない。ログインシェルを打ち切ったら `Detection::Unconfirmed` = 無いと言い切らず導入へ進まない（他の依存の #1533 と同じ扱い。依存段は `[警告] tailscale: 確認できません（ログインシェルが N 秒で応答なし）`）。`TAKO_TAILSCALE_BIN` に素の名前を渡すと既定の `tailscale` 候補と同じ規則（PATH → ログインシェル）で引く（検証用）。単体テスト = `tailscale::tests`（引き方・試し方を差し替えた `pick_candidate`）/ `remote_setup::tests::check_statusはtailscaleの検出を4通りで言い分ける`、実経路テスト = `bash scripts/test-tailscale-detect-1816.sh`（隔離 HOME + tailscale / brew / ログインシェルのスタブで 46 項目。`TAKO_BIN=<#1816 前の tako>` で 25 項目が落ちる） | M | ✅ #1816 |
| FR-2.33.14 | **Windows の新しいペインはレジストリの PATH を読み直す**（#2428。境界 B23 = `platform::user_path::pane_path`）。ペインは tako-app の起動時の env を継ぐので、tako を開いたままインストーラが `HKCU\Environment\Path` へ `~\.local\bin` を足しても新しいタブには載らず、setup が出す「次の 1 手」（`claude auth login`）が tako の再起動まで通らなかった（#2428 前は `launcher_dir_on_path` がレジストリだけで「在る」と答え、PATH の段も飛んだ）。ペインを起こすたびにマシン → ユーザーの `Path` を API（advapi32）で生の値のまま読み、`%VAR%` を展開して、**起動時の PATH に無いエントリだけを末尾へ足す**（起動時の値は 1 バイトも変えない = 起動時にしか無い項目を落とさず、引き先の順位も動かさない。重複は大小・`/`・末尾の `\` を無視して突き合わせる。環境変数 1 本の上限 32,767 を超えるぶんは足さない。足すものが無い・読めないときは PATH を渡さず継承のまま）。呼び出し側が PATH を明示したペインは触らない。setup の判定は「新しいターミナル（tako の新しいタブを含む）で引ける」（`launcher_dir_on_path`）と「いまのプロセスで引ける」（`launcher_dir_on_process_path`）を分け、前者だけが立つときは `needs_new_tab` を立てて、認証の案内（`auth_instructions_in`）・残り作業・`status` の表示に「tako で新しいタブを開いてから打ってください（再起動は要りません）」を添える。PATH の段の `verified` は「このプロセスで引ける」ときだけ立つ（前は「レジストリに既に在った」だけで立ち、このタブで引けないのに「どのターミナルからも使えます」と言った）。unix はペインがログインシェルとして profile を読み直すので経路ごと通らない（判定も文面も前のまま）。**器（psmux）越しのペイン**は中のシェルが器のサーバーの環境を継ぐので、起動済みのサーバーへは届かない（`-e PATH=…` で固定しない理由: psmux の `-e` はサーバーのグローバル環境へ入る = #1199、Windows の PATH は末尾が `;` のことが多く、tmux の引数解釈では末尾の `;` がコマンドの区切りになる。psmux は未実測）。A/B は `TAKO_2428_LEGACY=1`。**Windows 実機未検証**（単体テストと Windows CI のみ） | M | 実装済み（実機未検証） |

A/B は `TAKO_1057_LEGACY=1`（Windows は代行しない / PATH 判定は `exe::find` / 引き継ぎ先を探さない /
依存の質問をしない = #1057 前の挙動）。

### FR-2.34 Claude 公式 Remote Control への会話の委譲は明示 opt-in（✅ 2026-09-02、#1068。エピック #1059）

> tako が spawn する master / worker / solo は、既定では Remote Control に繋がらない
> （実測: tako 管轄の 5 セッションすべて未接続。
> `research/2026-09-01-remote-renewal-claude-official.md` §2.4）。
> **繋げば会話をスマホから操作できる**が、その会話は Anthropic のサーバーにも保存され、
> 認証は claude.ai アカウントへ移る（tako の機器ペアリング二層認証と role 4 段はその会話に効かない）。
> **静かに外へ同期させ始めないため**、プロファイル単位の明示 opt-in にする。

| ID | 要件 | 優先度 | 状態 |
|---|---|---|---|
| FR-2.34.1 | **プロファイル `remote_control`（既定 false）で gate する**。true のときだけ claude の起動コマンドへ `--remote-control` が付く。対象は spawn 経路 4 本（`tako master` / `tako solo` / 引き継ぎの後任 master / worker spawn + コンフリクト解消）。CLI（`profiles set --remote-control`）/ MCP（`tako_orchestrator_profiles`）/ 設定画面の 3 経路 1:1 | M | ✅ |
| FR-2.34.2 | **既定の挙動は 1 バイトも変わらない**。`Option<bool>` + `skip_serializing_if` なので既存 profiles/\*.yaml はバイト単位で不変、serde default で false として読める（= #916 の規約に対する「移行は要らない」の明示的な選択。回帰は `プロファイルの旧形式はremote_controlが無くても読める`） | M | ✅ |
| FR-2.34.3 | **起動前に適格性を判定し、不適格ならフラグを付けない**。不適格な環境で渡すと claude 自身が起動時に落ちる（= ペインが即死する）ため。判定は**ローカルの読み取りだけで確定する事実**に限る: 阻害 env（`CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC` / `DISABLE_TELEMETRY` / `DO_NOT_TRACK` / `DISABLE_GROWTHBOOK`）・エンドポイント差し替え（`ANTHROPIC_BASE_URL` の host ≠ `api.anthropic.com` / Bedrock / Vertex / Foundry）・非サブスク認証（`ANTHROPIC_API_KEY` / `CLAUDE_CODE_OAUTH_TOKEN`）・組織ポリシー（managed settings の `disableRemoteControl`） | M | ✅ |
| FR-2.34.4 | **証明できるときだけ断る**。プラン・組織のエントイトルメント・Trusted Devices・ZDR はローカルから分からないので**不適格と言わない**（フラグを付けて claude 自身に言わせる）。過剰に断ると「使えるのに使えない」を作る | M | ✅ |
| FR-2.34.5 | **判定材料は「起動先プロセスが実際に見る env」**。プロファイルの env 計画（`export` は後勝ち・`unset` は届かない。#500 / #512）を反映してから評価する。真偽の解釈は上流と同じ（`DISABLE_TELEMETRY` は空でなければ効く / `DO_NOT_TRACK` は `1` / `true` / `yes` / `on` だけ）ので、`DO_NOT_TRACK=0` で誤って断らない | M | ✅ |
| FR-2.34.6 | **状態は有効でも無効でも 1 行出す**（#981 の `sandbox_bypass_line` と同じ思想）。opt-in なのに付かなかったときは**理由 + 次の一手**を出す（CLI は起動前の行、spawn は応答の `warnings`、プロファイル照会は `remote_control_blocked`）。「スマホから見えない」の切り分けを可能にするため | M | ✅ |
| FR-2.34.7 | **claude 以外には渡さない**。codex の `remote-control` は自前ホストの app-server デーモン経路で別物、agy には手段が無い（`agent_support::keys::REMOTE_CONTROL` の宣言が正。実測根拠つき） | M | ✅ |
| FR-2.34.8 | **`remoteControlAtStartup` は tako が書かない**。ユーザー設定（#513 の共有分類にも関わる）なので案内だけにする | M | ✅ |

A/B は `TAKO_1068_LEGACY=1`（opt-in を無視して `--remote-control` を一度も付けない = #1068 前の挙動）。

### FR-2.35 Remote Control の session URL は 1 実装で 3 経路へ（✅ 2026-09-02、#1069。エピック #1059）

| ID | 要件 | 優先度 | 状態 |
|---|---|---|---|
| FR-2.35.1 | **取得は 2 段**。正 = transcript の `bridge_status` 行の `url`（完成形）、予備 = `bridge-session` 行の `bridgeSessionId` を `cse_` → `session_` へ変換して `https://claude.ai/code/<id>` を組む。**実測（claude 2.1.232）では `bridge_status` は常に出ない**（`bridge-session` を持つ transcript 84 件に対し `bridge_status` は 0 件）ので、予備段が実質の主役 | M | ✅ |
| FR-2.35.2 | **URL を捏造しない**。繋がっていなければ `not_connected`、判断材料が無ければ `unknown`、ローカルで不適格が確定していれば `ineligible: <理由>`。どの状態も `url` / `session_id` を持たない（型で強制） | M | ✅ |
| FR-2.35.3 | **アカウント UUID を保持しない**。`bridge-session` 行の `ownerAccountUuid` / `ownerOrganizationUuid` は読まない。どのアカウント配下かは accounts.yaml の**名前**（`account_label`）で表す。スマホが別アカウントだとそのセッションは一覧に出ないので、この表示が無いと切り分け不能になる | M | ✅ |
| FR-2.35.4 | **公開は 3 経路 1:1 で同じ 1 実装**（`RemoteLink::to_json`）。`GET /api/agents` / `GET /api/v2/panes` の `remote_link` / CLI `tako sessions link [--pane N]` / MCP `tako_sessions` の `action=link`。id も pane も省くと呼び出し元ペインの会話（#1958。入口が埋める。呼び出し元も無い = tako の外の CLI だけ、dispatch がアクティブタブのフォーカスペインへ倒す） | M | ✅ |
| FR-2.35.5 | **URL を診断ログへ出さない**。開くには claude.ai ログインが要る（実測 403）ので秘密ではないが、セッション id は follow-up 送信の宛先になるためペイン内容と同基準で扱う（番犬 = `crates/tako-control/tests/remote_link_watchdog.rs`） | M | ✅ |
### FR-2.38 会話を引き継いだセッション再起動（✅ 2026-09-02、#1067。#1967 で再開の組み立て・見張り・記録を直した。基盤は #498 / #390 / #112 / #640 / #749 / #915 / #982）

> claude CLI は symlink の張り替えで更新されるので、長生きのセッションは**起動時の
> 旧バイナリを握り続ける**（#498 の stale 警告）。直す手段が「ペインを閉じて立て直す」
> しかないと会話を失うため、実際には放置される。#498 は張り直しの受け口まで作って
> あったが実装が成立しておらず（Ctrl+C 1 回・器つきペインでは resume の行が動いている
> TUI の入力欄へ流れ込む・アカウント / モデル / effort が落ちる）、**merge 以来一度も
> 意図どおり動いていなかった**。併せて #749 の自動ハンドオフ（ctx% 高騰）を人の操作で
> 起こせるようにする。

| ID | 要件 | 優先度 | 状態 |
|---|---|---|---|
| FR-2.38.1 | **2 種を明確に分ける**。`harness` = エージェント CLI のプロセスだけを建て直し `--resume` で**同じ会話**を続ける（会話コンテキストを 1 文字も失わない）。`handoff` = 引き継ぎを書かせてから**新しいセッション**へ交代する（ctx をリセットできるが引き継ぎファイルに書いた分しか残らない）。語彙の正本は `tako_core::session_restart::SessionRestartMode` で、CLI の possible values / MCP の enum / GUI の分岐がすべてこの表から引く | M | ✅ |
| FR-2.38.2 | **落ちたことを確かめてから打つ**。旧プロセスへ SIGTERM を送り、**終了を確認してから** resume の行を #640 / #1940 の送達確認つき経路へ渡す。落ちる前に打つと動いている TUI の入力欄へ流れ込む（#694 / #1006 と同じ罠）。猶予（5 秒）を過ぎたら SIGKILL、上限（30 秒）で断念して**理由をペインのバナーへ出す**（黙って諦めない）。ゾンビは終了済みとして扱う（#619）。**#1967: 落ちても、シェルがプロンプトへ戻る（OSC 133;A の回数が終了要求の前より増える）までは打たない**（上限 10 秒。印を観測していないペインは待たない = `session_restart::shell_back`）。戻る前に打つと起動フックの最中の先行入力になる（#1940 と同じ型） | M | ✅ |
| FR-2.38.3 | **resume コマンドは元の起動と同じ組み立てで組む（#1967）**。正本は `tako_control::resume_launch`（ペインの再起動・`tako sessions resume` が同じ 1 実装を通る）: master / solo は `build_master_cmd`（`tako master -<名前>` / `tako solo -<名前>` と同じ = model・effort・`--append-system-prompt-file`・Remote Control）、worker は `build_worker_cmd`（spawn と同じ = 起動した master のプロファイルの許可モード・追加引数 + レジストリに記録した model / effort）に `--resume <id>` を足すだけ。system prompt のファイルは起動の直前に書き直す（消えていても起動できる。下見では書かない）。アカウントだけは**会話の記録がある設定 dir** へ寄せ、プロファイルのアカウントと食い違えば `warnings` に出す。**壊れたプロファイルを既定へ化けさせない**（ファイルが無いとき = ラベルだけで起動した旧い master だけ既定で組んで `warnings` に出す）。**#2473: worker は再起動をまたぐ GUI の復元（FR-5.9.2）も同じ 1 実装（`resume_launch::worker_resume_launch` → spawn の `build_worker_cmd_in` に会話 ID を渡す）で組み、codex の worker も spawn と同じ引数（`--model` / `-c model_reasoning_effort=` / プロファイルの args と env）を `codex resume` の後ろへ並べる。model / effort はレジストリに spawn の記録があればそれが正（`None` も spawn の決めた値 = カタログの model は `claude agents` の実モデルで上書きされるので混ぜない）**。agy・worker 以外の codex・role の無いペインは従来のカタログのメタ（`sessions::resume_command`）。応答の `recipe`（`master_profile` / `solo_profile` / `worker_profile` / `catalog`）でどの組み立てかが分かる。**#2470: 突然死の復旧案内（worker_status / watch / workers の `resume_command`）も同じ 1 実装を通す**: 案内は `tako orchestrator resume <worker_id>` の 1 行で、**打った時点で** `resume_launch::registry_resume_launch`（レジストリの spawn の記録と、spawn した master のプロファイル名 = `WorkerEntry.profile`。#2470 前の記録は既定プロファイルで組んで警告）→ `worker_resume_launch` が組み、MCP の設定ファイルを書いて**このシェルのプロセスを置き換えて**起動する（新しいペインを作らない = 死んだペインのシェルへ送る supervisor の自動復旧と、ペイン番号での追跡がそのまま続く。GUI が無くても動く。作業 dir が無ければ起動せずに止まる）。照会はファイルを読まない（user scope の `.claude.json` を読むのは打った時点の 1 回）。**無人で打ち込む経路（supervisor の `recover_dead`）は PATH の `tako` に頼らず**、動いている tako と同じ場所の CLI（`tako_cli_path::current_cli`）を絶対パスで方言どおりに打つ（PowerShell は `& '<path>' …`。隣に CLI が無いときだけ素の `tako`。PATH の先頭に `orchestrator resume` を知らない古い開発ビルドが居る機械で必ず落ちるため。`registry::unattended_resume_line`）。#2470 前の案内は `cd <cwd> && claude --model … --effort … --resume <id>` の手組みで、`--settings`（#2412）・プロファイルの args（本番は全プロファイルが `--permission-mode auto`）と env・`TAKO_ORCHESTRATOR_ROLE` が落ちていた。MCP からは案内を `tako_send_input` で死んだペインへ送る（シェルの中でプロセスを置き換える操作なので専用の MCP ツールは足さない。`tako master` と同じ扱い）。codex / agy は能力マトリクス `worker_death_resume`（#975）。A/B `TAKO_2470_LEGACY=1`・番犬 `issue2470_resume_command_watchdog`・実経路 `crates/tako-cli/tests/issue2470_resume_command.rs`（偽 claude で argv / env を採る）。#1967 前は master の会話がカタログに model / effort を一度も持たず（本番で 38 件中 0 件）、再開すると effort が medium・system prompt が落ちた claude が立っていた | M | ✅ |
| FR-2.38.4 | **会話 ID を解決できないまま終了させない**。解決は 生きた claude（`agents` 経由）→ セッションカタログの逆引きの順。どちらでも決まらなければ**プロセスに触らずに**断る（resume 先が分からないまま殺すと会話を失う）。**#1967: どちらの出どころでも会話の記録（transcript）の実在を確かめ**、無ければ `conversation_missing` で断る。終了直後に claude 自身が画面へ印字する `Resume this session with: claude --resume <id>`（実測 2.1.258）を拾えたら ID を差し替える（同一ペイン番号に世代が堆積する = #466 の実測への保険）が、**#1967 から案内は折り返しに依らず UUID の全長で読み（空白を落として行を連結する）、その ID の記録が実在するときだけ採る**。旧実装は 1 行の中だけを読み形の検査も「8 文字以上」だけだったので、細いペインで折り返した途中までの ID を権威として正しい ID を壊した（本番 10/9: `…-b897-065b9` の resume が実行され claude は会話が見つからずに終わった） | M | ✅ |
| FR-2.38.5 | **関門は 2 段で意味が違う**。**構造的**（セッションの有無・role・agent 系統・handoff は master のみ）は**メニューの出し分け**に使う（#1006 の「出た項目が断られない」）。**一時的**（会話 ID 未解決・生成中・キュー滞留・入力欄の下書き・選択肢ダイアログ）は**実行時に断って理由 + 次の一手を返す**（メニューから消すと機能そのものを見つけられず、右クリックした瞬間の状態で項目が出たり消えたりする）。判断は `is_eligible` / `can_restart` の 2 関数で、GUI・CLI・MCP が同じものを通る | M | ✅ |
| FR-2.38.6 | **メニューの出し分けに重い処理を置かない**。会話 ID の解決は `claude agents --json`（Node 起動）とカタログ読みを伴うので、**右クリックした瞬間に 1 度だけ**軽い材料（画面・role・レジストリ / カタログの agent 種別）を集め、メニューの描画中には読み直さない（#772 の「メインスレッド専有」を作らない） | M | ✅ |
| FR-2.38.7 | **生成中の判定に `is_busy` を使わない**。あれは advisory で経過秒トークンを拾うため、**生成が終わった後も画面に残る**完了行（実測 `✻ Brewed for 2s · done 9:22 PM`）を busy と読む = アイドルなペインを永久に busy と申告する。プロセスを終わらせてよいかの判断は中断ヒント（`esc to interrupt` / `esc to cancel` / `Generating`）だけを見る（`claude_tui::interrupt_hint_visible`） | M | ✅ |
| FR-2.38.8 | **引き継ぎ再起動は tako が代行しない**。#749 の自動ナッジと**同じ 1 実装の文面**（見出しだけ差し替え）でエージェント自身へ「引き継ぎを書き直す → `tako_orchestrator_handoff` を呼ぶ」を依頼する。tako が直接 handoff を呼ぶと**更新されていない引き継ぎファイル**で後任を立てることになる。前任のペインを閉じるのは後任（#749 の順序をそのまま使う） | M | ✅ |
| FR-2.38.9 | **AI フルコントロール**: `tako session-restart [--mode harness\|handoff] [--pane N]`（**引数なしで下見** = 何ができるか + できない理由を返すだけで何も起こさない。#748 の respond と同じ形・#322 の最簡形）と MCP `tako_session_restart` へ 1:1 公開する。ペインの右クリックの 2 項目も**同じ dispatch** を通る。#498 の張り直しボタン（`tako stale-binary restart`）も同一実装へ寄せ、**会話を保つ harness を優先**して会話 ID を解決できないときだけ handoff へ落ちる | M | ✅ |
| FR-2.38.10 | **系統差はマトリクスの 1 マスで宣言する**（#982）。`session_restart_harness` / `session_restart_handoff` を新設し、claude 以外は `Pending`（手段は上流に在る = `codex resume` / `agy --conversation` ので `Unsupported` にはしない）。判定を `if agent == claude` で散らさず `agent_support::supports` を通す。#1967 で `resume_launch_args`（再開が元の起動と同じ引数を持つ。claude のみ）を足した。#2473 で codex を worker だけの `Degraded`（master / solo の再開はカタログのメタ）にした | M | ✅ |
| FR-2.38.11 | **送った後を見張り、失敗を黙らない（#1967）**。起動コマンドを送った後 10 秒、エージェントが**すぐ終わらないか**を見る（終了の印 OSC 133;D が増えた / `No conversation found` が増えた = `session_restart::launch_verdict`）。終わったら、会話の記録が実在するときだけ tako が組んだ行で **1 回だけ打ち直し**、だめなら理由（`conversation_not_found` / `agent_exited` / `command_flow_timeout` / `process_stuck` / `pane_closed`）と終了コードを **ペインのバナー・persist.log・下見の `last_restart`**（`phase` = waiting_exit / launching / watching / started / failed・`reason`・`attempts`・`message`）の 3 か所へ出す。会話の記録の有無は画面の文言だけでなく**送った ID の記録の実在**でも判断する（2 行のペインでは文言がプロンプトに押し出されて残らない = 実測）。persist.log には開始（モード・agent・会話の短縮 ID・組み立て・プロファイル・終了要求の有無）・断った理由・打ち直し・結果を残し、**コマンドの本文は出さない**。引き継ぎの依頼の届き方は下見の `handoff_delivery` | M | ✅ |
| FR-2.38.12 | **重ねない・届け先の無い依頼を積まない・シェルだけのペインを戻せる（#1967）**。建て直しの最中（`last_restart` が決着する前）の再起動は `restart_in_progress` で断る（立ち上がったばかりのエージェントをまた終了させない）。引き継ぎ再起動は、**エージェントが見つからずシェルが入力待ち**のペインでは `agent_process_not_found` で断り harness を案内する（依頼が届かないまま時間切れにしない。前景で何かが動いているなら従来どおり依頼を積む = プロセスの特定は実行ファイルのパス頼みなので、名前の違うエージェント CLI まで止めない）。ハーネス更新は、エージェントが既に終わっていても**シェルが入力待ち（OSC 133 が Idle / Failed）なら終了させずに打つ**（下見の `shell_idle`。本番 10/9 は zsh だけが残り master が手で再開した）。OSC 133 は「シェルが入力待ちか」にだけ使い、生成中の判断には使わない（FR-2.38.7） | M | ✅ |

実装メモ（2026-09-02）: 判断は `tako_core::session_restart`（モードの語彙・`is_eligible` /
`can_restart`・`relaunch_step`・`parse_resume_hint` / `apply_resume_hint`。すべて純関数）、
材料集めと実行は `tako_control::dispatch`（`session_restart_menu_facts` = 軽い /
`build_session_restart_plan` = 重い）、建て直しの駆動は `tako-app` の
`drive_agent_relaunches`（500ms tick）。文面は `tako_core::handoff::restart_prompt`。
Windows のハーネス更新は、境界 B5 の `platform::process::terminate` の Windows の腕が
#1599 で入って動く経路になったが、Windows には穏当な終了要求が無いので CLI は強制終了
（`TerminateProcess`）になり、通しも実機で未実測なので対応マトリクスは `Degraded` のまま
（引き継ぎ再起動は影響を受けない）。
A/B は `TAKO_1067_LEGACY` を持たない（#498 の旧経路は意図どおり動いていなかったので
戻す価値が無い）。機械検証は unit 16 本 + セルフテスト項目 140
（出し分け / 下見 / 関門 3 種 / 会話未解決での不実行 / 引き継ぎ依頼）。

実装メモ（2026-10-10・#1967）: 再開の組み立ては `tako_control::resume_launch`（1 実装）、
シェルの戻り・見張り・失敗の語彙・記録は `tako_core::session_restart`（`shell_back` /
`launch_verdict` / `RelaunchFailure` / `RestartRecord`。純関数）、駆動は `tako-app` の
`drive_agent_relaunches`（WaitingExit → Launching → Watching の 3 段）。A/B は
`TAKO_1967_LEGACY=1`（カタログのメタだけで組む・案内を 1 行で読む・落ちたら即打つ・見張らない）。
番犬は `crates/tako-control/tests/issue1967_session_restart_watchdog.rs`（注入 15 通りを
file:line で名指し）、実測は `scripts/test-session-restart-1967.sh`（隔離 GUI・偽 claude・
「細く低い」と「細く高い」の 2 形 × 新旧の 2 腕。旧挙動は細く低い形で #1940 型の化け、
細く高い形で本番と同じ途中までの ID の resume を再現する）。

### FR-2.36 起動時ロードの予算と自動修正（✅ 2026-09-06、#1139）

AI エージェントが**起動した瞬間に強制ロードされるもの**に数値の上限を置き、超えたぶんを
機械的にアーカイブへ落とす。実測（2026-09-06）では master が何も作業しないうちに
コンテキストの 31%（約 31 万トークン）を消費しており、その 18 万トークンは `@import` された
作業ログ（332 エントリ / 56 作業日 / 407 KB）だった。「30 件超でアーカイブを**提案**する」
規約は在ったが、提案ベースなので 3 か月積もっていた。

| ID | 要件 | 優先度 | 状態 |
|---|---|---|---|
| FR-2.36.1 | **予算の正本は 1 つ**（`tako_core::context_budget`）。種別ごとの上限を純粋関数の表で持ち、判定・規約文・CI 番犬が**すべてこの表を引く**。数値の二重管理を作らない | M | ✅ |
| FR-2.36.2 | **毎ターン読む範囲**: 現在状態 80 行 / 作業ログ **5 作業日 かつ 20 エントリ かつ 12 KB**（1 エントリ 3 行以内）/ タスクリスト 1 本。**アーカイブとする範囲**: 予算から外れたエントリは `progress-archive.md` へ 1 行で移し、**90 日を過ぎたら消す**（git log・Issue・PR が正本なので情報は失われない） | M | ✅ |
| FR-2.36.3 | **規約文は予算表から生成する**。tako が配る 3 か所（リポジトリの `AGENTS.md` / `.agent/conventions.md` / `tako setup` が配る CLAUDE.md 節）へマーカー付きで埋め込み、一致をテストで固定する。既存ユーザーへは `changes.yaml` rev 18（guided）で届く | M | ✅ |
| FR-2.36.4 | **自動で直すのは安全に機械化できるものだけ** = `## YYYY-MM-DD` の見出しが並ぶ作業ログの移送。**冪等・本文の要約も改変もしない・移送で総エントリ数が合わなければ書き込まない**。自動化できない超過（規約の肥大・`@import` の増殖・system prompt）は `proposals` として**直し方つきで**返す | M | ✅ |
| FR-2.36.5 | **1 エントリの行数超過は自動修正の対象にしない**。本文を削るのは要約の捏造になるため、`check` が提案として名指しするにとどめ、書く側の規約として守らせる（CI もこの軸では落とさない） | M | ✅ |
| FR-2.36.6 | **発火点は 3 つ**: ①`tako setup` 実行時（状態 + `いま直す: tako context-budget fix` の 1 行。**質問はしない** = #262・#322 の最簡形）②`tako master` / `tako solo` の起動前 1 行 + master / solo system prompt の behavior ブロック ③CI 番犬 `crates/tako-control/tests/context_budget.rs` が tako リポ自身を**同じ関数で**判定して落とす | M | ✅ |
| FR-2.36.7 | **AI フルコントロール**: `tako context-budget [check\|fix] [--cwd] [--profile] [--dry-run] [--json]` と MCP `tako_context_budget`（action = check / fix）へ 1:1 公開する。GUI も IPC も要らないローカル処理（**壊れた設定で GUI が起動しないときにも引ける**ことが本質） | M | ✅ |
| FR-2.36.8 | **`@import` は行頭に限らない**。実際の `AGENTS.md` は箇条書きの途中に `@.agent/progress.md` と書く（そう書いても展開される）ので、行頭限定で数えると**毎ターン全文ロードされている当のファイルを 1 つも数えられない**。誤検出はバックティック参照の除外と**実在するファイルに解決できること**の 2 段で落とす | M | ✅ |
| FR-2.36.9 | **master / solo の system prompt も予算の対象**（✅ 2026-09-07、#1154）。上限は **24 KB**。これは tako 自身の生成物なので、超えたらユーザーに我慢させるのではなく tako を直す。手順の詳細（monitoring のイベント別対処表・acceptance・worker prompt テンプレート・引き継ぎの書き方など）は `tako orchestrator guide <topic>` / MCP `tako_orchestrator_guide` で**引く条件が満たされたときだけ**取得する形へ移し、既定 blocks には「いつ引くか」と常に握っておく規則だけを残す（#322 の「既定を賢く」）。**本文は 1 文字も捨てない**（移送前のブロックと byte 一致。プレースホルダだけ prompt と同じ解決を通す）。内訳は**組み立てと同じ 1 実装**から採る（`build_prompt_pieces` が断片の列を返し `join_prompt_pieces` が prompt になる）ので、超過時の `proposals` は **どの block / どの追記が何バイトか**を大きい順に出す（ユーザーの `prompt_blocks.append` は解決したファイル名で名指しする = 個人環境固有のルールの分離先を案内できる）。欠落ゼロは移送前テンプレート全文の fixture と多重集合で突き合わせて拘束する（`tests/prompt_guides.rs`）。A/B は `TAKO_1154_LEGACY=1`（本文を prompt へ差し戻す。予算は外さないので番犬が超過で落ちる） | M | ✅ |
| FR-2.36.10 | **system prompt の取り分を tako と利用者で分ける**（✅ 2026-09-14、#1477）。24 KB のうち **tako 自身が作る部分（base）は 18.5 KB 以内**（`SYSTEM_PROMPT_BASE_MAX_BYTES`）で、残り **5.5 KB（`PROMPT_APPEND_MAX_BYTES` = 引き算で導出）**はプロファイルの `prompt_blocks.append`（個人環境のルール）の取り分。#1154 のあとも base が 21.8 KB あり追記に 2.7 KB しか残らなかったため、委任の判断材料（`delegate_guidance` + `ledger::build_judgment_section()`）を新 topic `delegation` へ出し、behavior / monitoring / guides / context-budget の案内文を締めた。**ルールは 1 つも消さない**（全文は guide 側にあり、prompt には「いつ引くか」が残る） | M | ✅ |
| FR-2.36.11 | **予算を超えた追記は自動で常時部と on-demand 部に分かれる**（✅ 2026-09-14、#1477）。区切りは `<!-- tako:on-demand -->` の 1 行で、prompt に載るのは**その手前 + 自動生成の索引 1 行**（`append index (…)` piece）、後ろは `tako orchestrator guide local-rules` / MCP `tako_orchestrator_guide` で引く。区切りを入れるのは #916 の自動マイグレーション（`SchemaId::PromptAppend`）で、**ユーザーに手作業を要求しない**。安全要件: **挿入するのはマーカー行 1 本だけ**（`strip_marker(新) == 旧` が機械確認できる）/ 見出し境界でしか切らない /切れないもの（見出しが無い・先頭の節だけで超過）は触らない / 小さい追記と区切り済みの追記は触らない / 冪等 /旧ファイルは `.pre-v1.bak` へ退避。**索引はファイルへ書かない**（正本 1 箇所 = 利用者のファイルの見出し）。分割しても常時部が超えるときは `context-budget` の violations / proposals が**「区切りを何行上へ動かすか」を実測の平均行長から具体値で**返す。A/B は `TAKO_1477_LEGACY=1` | M | ✅ |
| FR-2.36.12 | **プラットフォームの縮退一覧は prompt に載せず引く**（✅ 2026-09-23、#1571）。`{{platform_notes}}` に入るのは「この環境は何か」「縮退が N 件あること」「引き方（`tako_orchestrator_guide({ topic: "platform" })` / CLI `tako orchestrator guide platform`）」「最新の対応状況は `tako platform --status pending`」の 4 点だけで、理由の全文は手順書 `platform` が返す。一覧は対応マトリクス（FR-2.29 / #515）の生成物で**縮退を 1 件足すたびに prompt が伸びる**構造だったため、Windows では `platform` 片が 4110 B・tako が作る側が 18944 B の取り分を 1.7〜2.7 KB 超え（CI 実測 20708 / 21420 / 21639）、**Windows の利用者だけが追記の取り分を黙って削られていた**。短縮形・手順書・A/B は `PlatformFacts` の 1 実装（`notes_section_in` / `full_section_in`）を共有し、写しを持たない（= 「prompt には無いが手順書にはある」ズレが構造的に起きない）。番犬は `prompt_budget_1477` で、**macOS 上から Windows 形を組んで**測る（`system_prompt_pieces_on(profile, platform)`。実機でしか測れないと片方の OS が野放しになる）。A/B は `TAKO_1571_LEGACY=1`（全文を prompt へ差し戻す。予算は外さないので番犬が超過で落ちる） | M | ✅ |
| FR-2.36.13 | **区切りのある追記の常時部が後から伸びて取り分を超えたら、区切りを上げる案を 1 か所に出し、明示の承認でだけ動かす**（✅ 2026-10-16、#1981）。FR-2.36.11 の自動分割は区切りがまだ無いファイルへ 1 回入れるだけなので、利用者が常時部へ規則を書き足して `常時部 + 索引` が `PROMPT_APPEND_MAX_BYTES` を超えても `tako migrate status` は up_to_date のままだった。どこまでを常時に残すかは利用者の判断なので**世代（自動で当たる移行）にはしない**: `tako migrate`（status）が `files[].state = proposed` として「N バイト超過・区切りを M 行目の『## X』の前へ上げる案（移る節・動かした後のバイト数）」を出し、明示の `tako migrate run` / MCP `tako_migrate` の run でだけ当てる。**自動の発火点（`tako setup` / GUI・master・CLI の起動時）は出さない・動かさない**。上げる先は常時部の中の見出し境界（題名を除く最も浅いレベル・コードフェンスの中の `#` 行は見出しにしない）のうち取り分に収まる**区切りに最も近いもの**。上げても収まらない（常時部に題名以外の見出しが無い / 先頭の節だけで超過）ものは `approvable: false` の案内だけ出して動かさない。安全要件は FR-2.36.11 と同じ: やるのは区切りの行を抜いて見出しの前へ入れ直すことだけ（`strip_marker(新) == strip_marker(旧)` を書く前にデータで確かめる）/ 旧内容は移行と同じ `write_with_backup` で `.pre-v2.bak` へ / 冪等。`context-budget` の助言（区切りのある追記）は `tako migrate` の案を先に案内する。番犬は `crates/tako-control/tests/issue1981_marker_move_watchdog.rs`、実経路は `scripts/test-local-rules-marker-move-1981.sh` | M | ✅ |

### FR-2.37 ゼロスタート導入を 3 系統へ（✅ 2026-09-09、#989。基盤は #868 / #1057、エピック #975）

> #868 のゼロスタート導入（インストール → PATH → 認証誘導）は **claude 専用**だった。
> `platform::agent_install::AgentKind` が Claude 1 値で、`setup_bootstrap` の全 API が
> 引数なし（= claude 固定）。codex / agy は「URL を見せるだけ」で、
> **codex だけ入れている人が `tako setup` を叩くと claude の導入を勧められた**
> （A/B `TAKO_989_LEGACY=1` で再現できる）。#975 の基準 2（他人の環境で通るセットアップ）の
> 一番大きなギャップ。

| ID | 要件 | 優先度 | 状態 |
|---|---|---|---|
| FR-2.37.1 | **手順の正本は境界 B17 の 1 表**（`platform::agent_install::recipe(platform, agent)` = 3 系統 × 2 プラットフォームの 6 マス）。取得元 URL・公式コマンド・置き場所・以後の更新のされ方・トラブルシュート先・インストーラへ渡す env を**データとして**持ち、`Platform` を引数で受ける純粋関数なので **macOS から Windows 側も検証できる**。値は 2026-09-09 に実物で確認したものだけ（codex = `https://chatgpt.com/codex/install.sh` → 302 → `releases.openai.com`・sudo 参照 0 件・SHA256 検証はインストーラ自身 / agy = 公式 docs の `https://antigravity.google/cli/install.sh`・SHA512 検証・単一バイナリ） | M | ✅ |
| FR-2.37.2 | **段の実装は 1 本で、系統は引数で受ける**（`status_for` / `install_for` / `ensure_path_for` / `auth_instructions_for` …）。**引数なしの claude 既定 API は置かない**。claude 既定の入口を残すと新しい機能がそこへ吸い寄せられて 1 系統だけ通る形に戻る（`AgentKind` が 1 値だった理由がそれ）。番犬 `crates/tako-control/tests/setup_bootstrap_agent_watchdog.rs` がソース走査で禁じる | M | ✅ |
| FR-2.37.3 | **単一選択を強制しない**（#988 と整合）。`tako setup` は「1 つ選ばせる」のではなく **1 つでも使える系統があれば導入の段を素通りする**。1 つも使えないときだけ 1 系統を仕上げ、**途中まで入っているもの（PATH / 認証だけ足りない）を優先**する。仕上げたあとは残りを足すコマンドを出すだけで、選択は迫らない | M | ✅ |
| FR-2.37.4 | **認証誘導は系統ごとの実コマンド**（正本は #983 の `agent_cli::auth_command`）。claude = `claude auth login` / codex = `codex login` / **agy = 引数なしの `agy`**（専用のログインサブコマンドを持たず、公式 docs の sign-in も引数なし起動。keyring に有効なセッションがあれば無言で通る）。**3 系統とも tako は代行しない**（#1129 の理由がそのまま効く） | M | ✅ |
| FR-2.37.5 | **PATH のマーカーブロックは 1 個**。macOS は 3 系統とも `~/.local/bin` なので、#868 が置くブロック 1 個で足りる（実測: claude の段で `installed`、codex / agy の段は `unchanged`）。**Windows は系統ごとに置き場が違う**（claude = `~\.local\bin` / codex = `%LOCALAPPDATA%\Programs\OpenAI\Codex\bin` / agy = `%LOCALAPPDATA%\agy\bin`）ので、PATH へ通すディレクトリは `launcher_rel` から導出する（プラットフォームで `match` しない） | M | ✅ |
| FR-2.37.6 | **Windows で実行を代行するのは実測済みの組み合わせだけ**。claude は #1057 で実機実測済み = `tako_can_run: true`、codex / agy は `install.ps1` が実在するが未実測なので **状態照会と公式手順の案内まで**（`tako_can_run: false`。実行代行は #525 の範囲）。宣言は `platform::support` の `tako_setup_bootstrap`（Degraded + 理由）と `agent_support::MATRIX` の 2 か所へ載り、`tako platform --platform windows` / `tako agent-support` から読める | M | ✅ |
| FR-2.37.7 | **CLI 解決のフォールバックも 3 系統へ**。入れた直後は profile へ書いた PATH がまだ届かないので、`find_agent_command`（`tako-cli`）と `agent_cli::locate`（worker 起動の実在検査）の両方が**インストーラの置き場所**を見る。#989 まではどちらも claude 専用（`_ => None` / `if agent == WorkerAgent::Claude`）で、codex / agy は「入れたのに見つかりません」で止まっていた | M | ✅ |
| FR-2.37.8 | **失敗の案内も系統ごと**。インストーラが失敗したときのトラブルシュート先は recipe が持つ（claude のページを codex の失敗に出さない。エッジ検証で実際に踏んだ）。codex のインストーラは最後に `Start Codex now? [y/N]` を `/dev/tty` から聞くので、代行時は `CODEX_NON_INTERACTIVE=1` を渡す（無いと codex の TUI が立って返ってこない） | M | ✅ |
| FR-2.37.9 | **AI フルコントロール**: `tako setup bootstrap [status\|status-all\|install\|path\|undo-path\|handoff] [--agent claude\|codex\|agy]` と MCP `tako_setup_bootstrap`（`agent` パラメータ）へ 1:1 公開する。`--agent` 省略は claude（#868 からの既定を変えない）で、**未知の名前は黙って claude にせず選択肢つきで拒否する**。`status-all` は 3 系統ぶんを 1 回で返し `tako setup --check` の材料になる。案内するコマンドは最簡形（claude は `--agent` を付けない = #322） | M | ✅ |

A/B は `TAKO_989_LEGACY=1`（面倒を見る系統が claude 1 つへ戻る = #989 前の挙動。
実測: codex だけ入っている HOME で既定は `Codex CLI を使えるところまで…` → `codex login`、
legacy は `Claude Code をインストールします` へ戻る）。


### FR-2.39 ログイン失効は接続断・上限とは別種として扱う（✅ 2026-09-11、#757）

> worker アカウントのログイン失効（OAuth リフレッシュトークンの無効化）は、
> 画面には先に `API Error: Unable to connect to API (ENOTFOUND / ECONNRESET)` として出る。
> #757 前はそれを `api_error`（推奨 `resume`）に分類していたので、master は
> 「ネットワーク断」と誤診して続行ナッジを撃ち続けた —— **`/login` をやり直すまで
> 1 リクエストも通らない**ので永久に復帰せず、2026-08-01 / 08-03 / 08-05 に 3 回、
> 同じ空回りを踏んだ。利用上限のダイアログの背後に隠れ、解除してから露出するケースもある。
> 先行事例は #1106（時間では解けない利用阻害を上限から分離した）で、骨格を揃えてある。

| ID | 要件 | 優先度 | 状態 |
|---|---|---|---|
| FR-2.39.1 | **専用種別 `WorkerErrorKind::LoginExpired`**（slug `login_expired` / 推奨アクション **`relogin`**）を `ApiError` から分離する。文言は #757 の実観測 3 件（`OAuth refresh token is no longer valid` / `Login expired · Please run /login` / `Please run /login`）。`needs_human`（#1106）と分けるのは「誰が何をすれば直るか」が違うため —— 管理者やプランの話ではなく**本人が 1 コマンド打てば直る**ので、master は待たず・ナッジせず・respawn せずに**再ログインを依頼する** | M | ✅ |
| FR-2.39.2 | **文言の正本は 1 か所**（`orchestrator::agent_cli::login_expired_line`）。#983 の起動時未認証検知（`looks_unauthenticated_screen`）も同じ語句を使うので、正本へ委譲させて二重管理をなくす。**どちらの種別になるかは呼び出し側のゲートで決まる**: 送達の証拠がまだ無い worker は `launch_failed`（まだ一度も起動できていない）、動いていた worker が途中で失効したのが `login_expired`（文脈は残っているので再ログイン後は同じペインで続けられる） | M | ✅ |
| FR-2.39.3 | **判定順序は「ライブのダイアログ > 失効 > 上限メッセージ」**。上限ダイアログが出ているあいだは応答が要る UI なので `usage_limit` のまま（その応答こそが失効を露出させる = Issue の提案 2）。解除後は上限のメッセージ行が画面に残っていても失効を返す（残骸で `wait_reset` を返し続けると解除時刻を待っても永久に復帰しない）。逆に**上限行のほうが新しいときは見送る**ので、再ログイン後に上限へ当たった画面で古い失効行が現在の停止を奪うことも無い。窓は末尾 15 行（`api_error` と同じ「いまの阻害」窓）で、スクロールバックへ流れた古い行や agent 自身が読んだコード・ドキュメントの引用は拾わない。狭いペインで中黒の前後が割れた形は #1123 の結合で 1 本へ戻してから同じ規則へ通す | M | ✅ |
| FR-2.39.4 | **対象アカウントを併記する**。複数アカウント運用では「どれを再ログインするのか」が分からないと直せないので、`error.config_dir`（その worker の `CLAUDE_CONFIG_DIR`）と `error.account`（accounts.yaml の名前）を返す。逆引きは会話（session_id）の transcript の所在から行う（`orchestrator::login_expired_account`。#652 の resume と同じ根拠）。**引けなければキーごと出さない**（推測の名前を出すと、ユーザーは正しい方を再ログインしないまま「直らない」と思うことになる）。逆引きは transcript のディレクトリ走査を伴うので**失効を検知したときだけ**呼ぶ（watch は 5 秒ごとに回るため、毎ポーリングの I/O にはしない） | M | ✅ |
| FR-2.39.5 | **`api_error` は従来どおり `resume`**（回帰）。`Unable to connect to API (ENOTFOUND / ECONNRESET)` だけの画面は失効へ倒さない。**自動復帰（#813）も発動しない**（`detect_limit_stop` が `None` = 解除を待つ相手が居ないので追跡そのものを作らない）。**supervisor（#401）は自動復旧を試みず**、`action=login_expired_report` として未復旧で上げ、`SUPERVISOR_DETECTED` の 1 行へ対象アカウントを載せる | M | ✅ |
| FR-2.39.6 | **AI フルコントロール**: `tako orchestrator watch` の `WORKER_ERROR` と MCP `tako_orchestrator_worker_status` の `error` が**同じ内容**を返す（`kind` / `recommended_action` / `config_dir` / `account`。組み立ては `wait::error_json` の 1 実装で、watch 側は dispatch が逆引きした結果をそのまま運ぶ = 引き直さない）。master の手順書（`tako orchestrator guide monitoring`）に「`relogin` は `resume` では解けない。対象アカウントを名指しして `/login` をユーザーへ依頼する」を明記し、**全種別が手順書に載っていること**を機械検証する（`master_promptは全ての異常種別の対処を書いている`）。系統差は `agent_support::MATRIX` の `worker_login_expired_detect` として宣言する（claude = 実観測ずみ / codex・agy = 失効時の実画面を採れていないので `pending`（**推測の文言を置かない**）/ ローカル LLM は失効する資格情報が無いので対象外） | M | ✅ |

A/B は `TAKO_757_LEGACY=1`（#757 前の分類へ戻る = 接続エラー行があれば `api_error` +
`resume`、無ければ**検知なし** = idle（作業完了）に見える）。番犬は
`issue757_login_expired_watchdog.rs`（検知段の位置・正本の一本化・手順書の言い切り・
**手順書が自分の文言で自分を釣らない**こと）と `issue757_legacy_ab.rs`。
**実機で失効させる再現はしていない**（同一アカウントを別マシンから使った直後に起きるもので
任意のタイミングでは作れない）ので、検証は実観測の文言を描いた fixture と
隔離 tmux のペインで行った。

### FR-2.40 ユーザー向けタスク（人がやること）の正本と返答の配送（✅ 2026-09-13、#1450 の分割 B1 / B2）

> master は「ユーザー確認待ち」を引き継ぎファイル（`handoff/projects/*.md`）と会話に
> 溜めていて、**ユーザーが一覧で見る場所が無かった**。宣伝系（#1081 の動画 / #1284 の
> X ショート）も「完成したがユーザー投稿待ち」が可視化されない。会話の中の 1 行は待ち行列に
> ならない —— スマホからは見えず、次の master は引き継がず、答えたかどうかも分からない。
> worker の task 系（`tako task checkpoint` / `gate` = #242 / #244）は **AI のタスク**なので
> 名前空間を分ける（id の綴りも `task-N` / `u-N` で目で区別できる形にした）。
> B1 はその**正本と操作**で、PC の画面（B2）・PWA（B3）・初期データ（B4）はこの API だけを叩く。

| ID | 要件 | 優先度 | 状態 |
|---|---|---|---|
| FR-2.40.1 | **データモデルは tako-core**（`user_task`。GPUI 非依存）。`id`（`u-N`）/ `title` / `body`（markdown）/ `kind`（review / confirm / permission / post / other）/ `status`（open / done / dismissed）/ `created_by` / `project` / `attachments[]` / `copy_texts[{label,text}]` / `links[]` / `due` / `created_at` / `updated_at` / `origin` / `responses[]` / `delivery`。**空タイトルは拒否**（一覧で選べないものを作らせない）。**添付の実在は起票時に問わない**（動画を書き出す前に起票する運用が普通にある）代わりに、一覧・詳細の応答が `attachments[].exists` を返して「消えた添付」を画面に出せるようにする。`copy_texts` をラベルつきの配列にしてあるのは、投稿文・タイトル・説明・タグが**別々の入力欄へ貼られる**ため（1 本の本文に混ぜるとスマホ側で編集作業が発生する） | M | ✅ |
| FR-2.40.2 | **永続は `<data_dir>/orchestrator/user-tasks.yaml`**。書き込みは `config_io` の排他 flock + アトミック書き込み（`task_checkpoints` / `ledger` と同じ read-modify-write）なので、**2 プロセスが同時に起票しても取りこぼさない**。読めないファイルは 0 件へ丸めず Err にして退避へ回す（#169）。#916 の番地 `SchemaId::UserTasks` に載せ、共有分類は `Class::Local`（ペイン ID と master のプロファイル = **この機械の状態**を持つのでデバイス間共有しない） | M | ✅ |
| FR-2.40.3 | **操作は 1 実装**（設計原則 5）。`add` / `list` / `show` / `update` / `done` / `dismiss` / `respond` を `Request::UserTask` の 1 経路へ集約し、CLI（`tako todo <action>`）と MCP（`tako_todo` の `action`）が 1:1 で載る。CLI をローカル処理にしない（= store を直接触らない）のは、**起票の通知と返答の配送が GUI の中でしか起こせない**ため。MCP を 1 ツールへ畳んだのは、ツール一覧が master の起動時ロード（#1139）に毎回乗るから。`list` は既定で未完了のみを返し、**`open_count` と種類別の内訳**（B2 / B3 のバッジの正本）を同じ応答に載せる | M | ✅ |
| FR-2.40.4 | **起票元は自動で入る**。`origin`（profile / session_id / pane / project）を呼び出し元の role（env 由来と ペインの role ラベルの**両方の語彙**を `handoff::master_profile_of_any_role` の 1 実装で解く = #761 の取り違えを構造的に避ける）と、セッションカタログの逆引き（#284）から埋める。master は「起票する」とだけ言えばよく、返答の戻り先を指定しない | M | ✅ |
| FR-2.40.5 | **返答は起票 master の入力欄へ届く**。`respond`（`decision` = approve / reject / needs_change / answered、`comment`、`via` = pc / pwa / cli / mcp）は `responses[]` にスレッドとして積み、**新しい送達経路を作らず**既存を呼んで配る: ①同じプロファイルの master ペインが生きていれば `Request::Send`（= `tako_send_input` と同じ腕。#1259 の送達フローに乗る）で「【ユーザー返答】…」を送る ②居なければ `master_launch::plan`（= `tako master` / PWA の「+ master」= #1078 と同じ組み立て）で**新しいタブに起動**し、`queue_prompt_flow` で起動を待ってからタスク本文 + 返答を初回メッセージとして渡す。**本文にペインの内容・添付の中身は載せない**（絶対ルール）。①では本文を送らず②でだけ載せるのは、生きている master は文脈を持っているため | M | ✅ |
| FR-2.40.6 | **配送の顛末が残り、無言にならない**。`delivery`（`sent` / `launched` / `delivered` / `failed` + profile / pane / tab / reason / 何番目の返答か）をタスクに持ち、決着は `host.prompt_delivery_state`（#1259 の 1 実装）を**読むたびに畳み込んで**確定する。**分からないものを「届いた」とも「失敗した」とも言わない**（未決着は `sent` のまま）。配送できないとき（プロファイル不明・起動失敗・タブ作成失敗）は `failed` + 理由をタスクへ残し、persist.log にも 1 行出す。既知の限界: `prompt_delivery_state` は 300 秒で消える（`still_visible`）ので、5 分以上誰も一覧を引かないと `sent` のまま残る（画面は数秒周期で `list` を叩くので実運用では埋まる） | M | ✅ |
| FR-2.40.7 | **起票は画面に 1 行出る**。共有の通知欄（#1399 / #1417 / #1422 / #1432 と**同じ出し口**の成功系 = `notify_ui_info`）へ「新しいユーザータスク u-N: タイトル」を出す。**本文・添付・コメントは出さない**（通知は消える前提で、続きは右パネルと `tako todo list` で読む）。診断（persist.log）は**起きた場所**（dispatch）に置く = sidebar.rs から診断へ書く口は `log_ui_failure` の 1 つのまま。A/B は `TAKO_1450_LEGACY=1` | M | ✅ |
| FR-2.40.8 | **master の運用へ組み込む**。`tako orchestrator guide user-tasks` に「ユーザーに聞くこと・見てもらうこと・許可をもらうことは会話や引き継ぎに溜めずここへ起票する」「返答は入力欄に届くので、それを起点に作業を続ける」「投稿系は `copy_texts` を貼り先ごとに分ける」を置き、system prompt の topic 表から引けるようにする。`respond` の状態遷移は approve / reject = 閉じる、needs_change / answered = **open のまま**（AI 側の作業がこれから続くので、閉じると直した物を見てもらう先が消える） | M | ✅ |


A/B は `TAKO_1450_LEGACY=1`（起票しても通知欄に何も出ない = 「起票されたことに気づけない」の再現）。
番犬は `issue1450_user_tasks_watchdog.rs`（CLI が store を直接触らない / CLI と MCP の action 語彙が一致 /
永続の番地が 4 か所に揃う / 通知の出し口が増殖しない / 配送が既存経路を呼ぶ。**注入 7 通りで `file:line` 名指し**）。
実測は `scripts/test-user-task-delivery.sh`（隔離した実 tako-app + 実 CLI + claude スタブで
①〜⑦ を通す）。スマホ（PWA）側は下の **B3** 節。

#### B2: PC の画面（右パネルの `tasks` ビュー。✅ 2026-09-14）

| ID | 要件 | 優先度 | 状態 |
|---|---|---|---|
| FR-2.40.9 | **右パネルに 4 本目のビュー `tasks`**。切替は `Request::Panel { view: tasks }` の 1 経路で、CLI `tako panel --show --view tasks` / MCP `tako_panel` / GUI のタブ / コマンドパレット `panel-tasks` が同じ `PanelViewWire` を通る（表示メニューの「パネル」の「tasks ビュー」はパレットと同じ `toggle_panel_view` = #2341）。語彙の正本は `PanelViewWire::VALUES` なので、CLI の possible values・`--help`・不正値の案内・MCP の説明は**追加の分岐なしで追従する** | M | ✅ |
| FR-2.40.10 | **一覧は B1 の `list` だけを叩く**。既定は未完了のみ（「完了も見る」で `all` を立てる = 取得条件）。種類・プロジェクトの絞り込みは**取ってきた行の表示側**で当てる（`list` を絞って撃つと、チップに出す候補が絞り込んだ結果からしか作れず、一度絞ると戻れない）。候補は**今ある行に出てくるものだけ**を宣言順で出し、プロジェクトは 2 つ以上あるときだけ行を出す。並びは `updated_at` 降順・同時刻は id の新しい順（読み込みごとに揺れない）。未完了件数のバッジは **`open_count` をそのまま出す**（画面で数え直さない） | M | ✅ |
| FR-2.40.11 | **詳細は 1 枚の縦並び**（パネルは 300px 前後なので一覧と詳細を左右に割らない）。markdown 本文は `md_view::render_document` = プレビュー / アップデート画面と**同じ 1 実装**で描く。添付は `attachments[].exists` を見て、在るものには「Finder で表示」（`Request::FileOp { Reveal }`）と**行そのものを押してのプレビュー**（`Request::OpenFile`。#1472 で小さなボタンから行へ広げた = FR-2.40.22）、**消えたものには「見つかりません」**を出す。`copy_texts` は貼り先ごとに 1 つずつのボタン、`links` は http / https を既定ブラウザ・それ以外は tako の中で開く | M | ✅ |
| FR-2.40.12 | **返答フォームとスレッド**。`decision`（4 択チップ）+ `comment`（複数行・`⌘Enter` で送信）を `respond` へ渡し、`via` は `pc`。**判断を選ぶまで送れない**（誤爆防止。理由も画面に出す）。返したらフォームは空に戻る（次のタスクへ前の判断を持ち越さない）。`responses[]` はスレッド（#2016 から**古い順・時刻つきでフォームの上**に置く = FR-2.40.31）で、`delivery` は状態（`sent` / `launched` / `delivered` / `failed` + 理由）を色と字で出す。**`sent` を「届いた」と書かない・緑にしない**（B1 と同じ物差し） | M | ✅ |
| FR-2.40.13 | **ポーリングは「止める処理」を持たない**。新しいタイマーを作らず既存の 2 秒定期更新へ 1 段足し、撃つ判断は `tick_user_tasks` の 1 か所（`panel_visible` と A/B を見る）。右パネルを閉じればその tick から空振りになるので**止め忘れが構造的に起きない**。B1 は配送を `host.prompt_delivery_state` から**dispatch が走るたびに**畳み込んで確定させるので、一覧の更新は host に触る段を UI スレッドに残す。**#2031: 置き場は UI スレッドで読まない**。2 秒ループ・タブを開いた瞬間・「完了も表示」は `spawn_user_tasks_list` を通り、dispatch の "list" と共有の 2 段 = ① 配送の観測（`user_task_list_observe`。送達フローを読むだけ = I/O なし・UI スレッド）→ ② 決着した配送の書き戻し + 置き場の読み・解釈（`user_task_list_load` + `parse_list`。background）。② は置き場の版（更新時刻・大きさ・inode。**読む前に**採る）が前回と同じなら読まずに前回の正本から組み直す（添付の `exists` は毎回見直す）。前のジョブが走っている間は撃たない（読みが 2 秒を超えても積み上げない）。後から頼んだ引き直しの結果を、先に頼んだ遅い結果で潰さない（通し番号）。同期の引き直し（`refresh_user_tasks`）は右パネルの返答の直後だけ（状態遷移の直後は #2052、AI からの展開は #2086 で background へ）。旧実装は 2 秒ごとに UI スレッドで YAML を 2 度読み、本番で 1 回 551 ms 止まった。A/B は `TAKO_2031_LEGACY=1`、読みの遅さの注入は `TAKO_2031_INJECT=slow:<ミリ秒>`、番犬は `issue1984_ui_thread_offload_watchdog` の 10 節（注入 7 通り）、実測は visual-test `tasks-tick` + `scripts/test-tasks-tick-2031.sh`。**#2052: IPC の一覧と完了・却下も UI スレッドで置き場を読み書きしない**。CLI / MCP / PWA（5 秒ごと）の `list` と、IPC・右パネルのボタンの `done` / `dismiss` は `prepare_offload` の腕（`user_task_offload_prepare`）で background のジョブになる: ① 送達フローの観測（`user_task_flows_observe`。生きている全ペインの決着したフローを読むだけ = I/O なし。IPC の要求は前回の正本を持たないので宛先を名指しせず全ペインを見る。閉じたペインの顛末は host が落とすので旧実装と同じものが採れる）と画面の展開状態の写し（UI）→ ② 決着した配送の書き戻し + 一覧の読み / 状態の書き換え（background。決着したフローが無ければ畳み込みのための読みはしない）→ ③ 完了・却下だけ、開いていた行を畳む（UI。`finish_offload`）。段の中身は dispatch の同期の腕と共有の 1 実装。右パネルのボタンは `dispatch_off_ui` を通り、書き換えを待つ間に別の行を開いて打ちかけたフォームは着地で消さない。コマンドパレットの `panel-tasks`（`toggle_panel_view`）と `tako panel --show --view tasks`（`set_panel` → `after_dispatch`）は、タブのクリックと同じく開いた瞬間に一覧を background で引き直す（2 秒ループを待たない）。A/B は `TAKO_2052_LEGACY=1`、番犬は同じ `issue1984_ui_thread_offload_watchdog` の 11 節（注入 10 通り）、実測は visual-test `tasks-offload` + `scripts/test-user-task-offload-2052.sh`。**#2086: IPC の起票・詳細・更新・返答（PWA を含む）・展開 / 折りたたみも UI スレッドで置き場を読み書きしない**。同じ腕（門は `user_task_offloads`）で background のジョブ（`OffloadJob::UserTaskStore`）になる: show / update は ① 引数の検査と送達フローの観測（UI）→ ② 書き戻し + 読み / 書き換え（background。そのまま応答）。add は ① 起票元の材料（呼び出し元ペイン・role・spawn 元 = workspace）の写し（`user_task_origin_facts`。UI）→ ② 起票元の解決（管轄 = プロファイルの読み・セッションカタログの読み）と書き込み（`user_task_add_store`。background）→ ③ 通知欄へ 1 行（UI）。respond は ① 届いた順に札を取る（`RespondTurn`。UI）→ ② 番を待って積む（background）→ ③ 起票 master へ運ぶ（UI。`land_offload`）→ ④ 配送を記録して読み直す（background。受け口は ③ の後処理 = master を起こしたペインの PTY の起動を済ませてから出す `OffloadLanding::Settled`。後処理の失敗は同期の腕と同じく最後の応答）。札は ④ の終わりで手放すので返答は 1 本ずつ通り（続けて届いた返答が段の途中で追い越さない = 返答の番号の順と master の入力欄へ届く順が届いた順のまま・運ぶのは 1 本につき 1 回）、失敗・アプリの終了で落ちた札は次へ回る。番待ちの上限は 60 秒（超えたら persist.log へ 1 行残して進む）。expand / collapse は ② 在ることの確かめ（background）→ ③ 開く / 畳む（UI）。#2031 が残した AI からの展開の同期の引き直し（Context が無いことが理由だった）は、#2052 でできた `list_poll.pending` → `after_dispatch` → `spawn_user_tasks_list` へ載せ替え、行を見える位置へ運ぶのはその行が載った一覧が着いたとき。経路（`via`）を省いた返答だけは同期の腕に残す（省いたときの既定は呼び出し元で決まり、準備の段は呼び出し元を知らない。MCP の入口は `mcp` と埋めて送るので、ここへ来るのは古い CLI の stdio ブリッジと素の IPC だけ = 記録の正しさを取る）。段の中身は同期の腕と共有の 1 実装。A/B は `TAKO_2086_LEGACY=1`（#2052 の軸とは別）、番犬は同じ `issue1984_ui_thread_offload_watchdog` の 12 節（注入 10 通り）、実測は `scripts/test-user-task-offload-2086.sh`。**#2182: 返答の配送で起票 master が居ないときの起動の組み立ても UI スレッドで待たない**。③ 運ぶ（`user_task_respond_deliver`。UI）は宛先を決めるだけで、起動が要れば組み立て（`master_launch::plan` = CLI の所在確認のログインシェル・上限 15 秒 + tako の実行ファイルの解決）を `OffloadJob::UserTaskLaunch` でもう一巡 background へ出し、次の着地（`user_task_launch_land`。UI）でタブを作って起動コマンドと返答を流してから ④ へ進む。着地では宛先を決め直す（組み立ての間に起票 master が現れていればそちらへ送り、2 つ目を起こさない）。札は組み立ての間もジョブが持ち回る。右パネルの「返す」も同じ段を background で回す。もう一巡を出せない直の呼び手（`finish_offload`。製品の経路は通らない）は組み立てを待たずに配送の失敗として記録する。A/B は `TAKO_2182_LEGACY=1`、番犬は同じ番犬の 15 節（呼び出しグラフ。既知の残りから外した）+ 16 節、実測は visual-test `tasks-launch-2182` + `scripts/test-respond-master-launch-2182.sh` | M | ✅ |
| FR-2.40.14 | **絵文字を使わない**（#217 の全面禁止）。ビューのアイコンは svg のパス（`icons/ui/tasks.svg`）、種類と配送の別は `rounded_full` の図形と色で出す。番犬がソース走査で 1 つも無いことを固定する | M | ✅ |
| FR-2.40.15 | **設計原則 5 の例外は 2 つだけ**（明示）: クリップボードへの書き込みと URL を既定ブラウザで開く操作は dispatch を通らない。前者は GPUI の `App` が要り（`tako chat copy` / `tako preview-copy-code` も同じ理由で**実書き込みは render 側**）、後者は端末リンク・PDF・md プレビューの 3 経路がすべて `os_integration::open_url` 直呼びで統一されているため（新しい Request を足すとその統一が 4 経路に割れる）。**読める形**（`copy_texts[].text` / `links[]`）は `tako todo show` が返すので、AI から内容は取れる | M | ✅ |

B2 の A/B は **`TAKO_1450B2_LEGACY=1`**（B1 の `TAKO_1450_LEGACY` とは別の軸）。立てるとポーリングが止まり、
「起票しても一覧に出てこない」「配送が `sent` のまま固まる」が同一バイナリで再現する。
番犬は `issue1450b2_tasks_panel_watchdog.rs`（画面が正本を直読みしない / 絵文字 0 / ビュー切替が
`tako panel` と同じ語彙を通る / ポーリングが止まる / 手書きのテキスト入力を増やさない。**注入 8 通りで
`file:line` 名指し**）。実測は隔離セルフテスト**項目 150** と visual-test の
`TAKO_VISUAL_ONLY=tasks-panel`（Metal の最終 scene を読み戻すので**画面収録権限が要らない**）。

返答コメント欄の編集ロジックは `tako-app::text_field::TextField`（GPUI 非依存の純ロジック）。
右パネルに既にある手書き入力 2 本（git のコミットメッセージ / 新規ブランチ名）の移行は **#1459** で追う
（番犬が「**新しい**手書きカーソル操作を足せない」を縛り、既存 2 本は名指しの猶予にしてある）。

#### B3: スマホの画面（PWA の `#/tasks`。✅ 2026-09-14）

| ID | 要件 | 優先度 | 状態 |
|---|---|---|---|
| FR-2.40.16 | **スマホ（PWA）から片付けられる**（#1450 の分割 B3）。`#/tasks` に一覧（既定 = 未完了・種類で絞り込み・並びは `updated_at` 降順）と詳細（markdown 本文 / 添付 / コピー用テキスト / リンク / 返答フォーム / やりとり / 配送の状態 / 完了・却下）を出し、ペイン一覧のナビに**未完了件数のバッジ**を出す。件数の正本は `list` の `open_count`（画面で数え直さない）。**語彙と状態表示は PC 版（B2）と同一**で、`sent` を「届いた」と書かない | M | ✅ |
| FR-2.40.17 | **daemon の受け口は宣言表**（`tako_control::remote_tasks::TASK_ROUTES`）。`GET /api/tasks` = Observe、`POST /api/tasks/<id>/{respond,done,dismiss}` = Interact で、**表に無い `/api/tasks…` は Manage へ落とす床**を置く（#1405 の「未知の経路が弱い role へこぼれる」を塞ぐ）。中身は B1 の `Request::UserTask` を**素通し**するだけで、データモデル・永続・配送はこの分割で 1 行も変えない。返答の `via` は `pwa` | M | ✅ |
| FR-2.40.18 | **添付のダウンロードは新しい配信経路を作らない**。daemon が添付の絶対パスを `remote_files::shortcut_target`（#1451 の 1 実装）で `{root, path_rel}` へ解決して返し、PWA は既存の `GET /api/files/download?root=…&path=…`（#1079。Interact）を叩く。したがって**認可は `resolve_in_root` の 1 実装のまま**で、解決できる範囲は role で決まる（manage = `fs` ルートでどこでも / interact = tako のツリーに出ているフォルダ配下だけ）。**タスク専用の root は作らない**: 添付のパスは AI が書く欄なので、それを根拠に配ると AI が弱い端末の読める範囲を広げられる（#1451 の判断と食い違う）。解決できない添付・消えた添付は**理由を出し**、権限が足りないだけなら #1452 の権限リクエストへ繋ぐ | M | ✅ |
| FR-2.40.19 | **ワンタップのコピーと共有**。`copy_texts` は 1 件ずつ「コピー」でき（`navigator.clipboard` → 使えなければ `textarea` + `execCommand` のフォールバック。**成否を画面に出す**）、`navigator.share` がある端末では投稿文・リンクを共有シートへ渡せる。添付の共有シート（`navigator.canShare({files})`）は **64 MB 以下に限る**（`remote_tasks::SHARE_MAX_BYTES` と PWA の定数が一致していることを番犬が見る）: Blob を丸ごとメモリへ載せるので、数百 MB の動画はタブごと落ちる。大きい添付は「端末に保存 → ファイル / 写真アプリ → 投稿アプリで選ぶ」に倒す | M | ✅ |
| FR-2.40.20 | **ポーリングは画面を離れると止まる**。周期は 5 秒の 1 定数で、`useEffect` の cleanup で `clearInterval`・裏へ回っているあいだ（`visibilityState === 'hidden'`）は撃たない。**画面とナビのバッジが同じ 1 実装**（`usePolling`）を使う（止め方が 2 か所に割れない）。ポーリングには意味があり、B1 が `list` のたびに配送を畳み込むので**見ているあいだに `sent` が `delivered` へ確定する**。A/B は `TAKO_1450B3_LEGACY=1`（daemon は `/api/tasks…` が 404）と `?tako_1450b3_legacy=1`（PWA は画面もバッジも出ない） | M | ✅ |
| FR-2.40.21 | **添付をその場で見せるのに経路を増やさない**（#1472 B）。PWA の `#/tasks` の詳細で、画像は `<img>`・動画は `<video controls preload="metadata">` としてその場に出す（画像はタップで全画面）。`src` は **`GET /api/files/download` に `disposition=inline` を足しただけ**で、受け口も認可も FR-2.40.18 のまま（したがって**落とせない添付は見られない** = observe は 403・interact はツリー配下だけ）。daemon 側は同じ経路が ①`?disposition=inline` のとき `Content-Type` を実体の型にして `Content-Disposition: inline` を返す ②`Range: bytes=…` に **206 + `Content-Range`** で答える（`Accept-Ranges: bytes` は常時）の 2 つを増やしただけで、応答を組むのは `respond_file` の 1 実装のまま。**Range が要るのは飾りではない**: Safari の `<video>` は部分取得に 206 が返らないと再生に進まない。**画像 / 動画の判定と MIME の表は `tako_core::open_plan` の 1 本**（cmd+クリック・`tako open` と同じ表）で、daemon が `attachments[].preview` に載せて渡す = PWA に拡張子の表を作らない。**一覧では 1 バイトも取りに行かない**（描くのは詳細だけ・動画は `preload="metadata"`）ので、300 MB の添付が並んでいても一覧は軽い。PDF / Markdown など PWA にビューアが無い種別は `preview` を名乗らず従来どおり保存と共有だけ | M | ✅ |

B3 の A/B は **`TAKO_1450B3_LEGACY=1`**（daemon が `/api/tasks…` を 404 にする）と
**`?tako_1450b3_legacy=1`**（PWA が画面もバッジも出さない）。どちらも「スマホからは
タスクが見えない・片付けられない」= B3 以前を同一バイナリで再現する。
番犬は `issue1450b3_tasks_pwa_watchdog.rs`（PWA が表外の API を呼ばない / observe に操作を出さない /
添付の配信が認可 1 実装を通る / ポーリングが止まる / 語彙が PC 版とずれない / 絵文字 0。
**注入 6 通りで `file:line` 名指し**）。実測は `scripts/test-remote-tasks-1450b3.sh`
（隔離した実 daemon + 実 tako-app で role ごとの 200 / 403・添付の解決範囲・
**数百 MB の添付の実転送**・返答と配送・完了までを通す）と
e2e `web/tako-remote/e2e/tasks-1450b3.spec.js`（iPhone viewport の実 DOM・偽の木だけを見る）。

#1472 B（その場プレビュー）の番犬は `issue1472_attachment_preview_watchdog.rs`
（添付の URL を組む場所が 1 か所 / `FILE_ROUTES` の外に配信の受け口が生えない /
`Range` を読むのが 1 実装 / MIME の表が `open_plan` の外へ写らない /
画像・動画の判定を PWA がやり直さない / 一覧が添付に触らない。**注入 8 通りで `file:line` 名指し**）。
実測は `scripts/test-remote-attachment-preview-1472.sh`（隔離した実 daemon で
206 + `Content-Range`・末尾チャンクの中身・416・150 MB の部分取得での常駐量・
role ごとの 200 / 403 を通す）と e2e `web/tako-remote/e2e/tasks-preview-1472.spec.js`
（実 PNG / webm を配って `naturalWidth` と `readyState` まで見る）。

### 追補: 人がやることの添付をその場で見る（✅ 2026-09-14、#1472 の A = PC 側）

| ID | 要件 | 優先度 | 状態 |
|---|---|---|---|
| FR-2.40.22 | **添付は行を押すと既存のプレビューで開く**。押す先は `Request::OpenFile` の 1 経路（= `tako open` / MCP `tako_open_file`）で、拡張子 → 画像 / 動画 / PDF / Markdown / コードの振り分けは `tako_core::open_plan::preview_route` の 1 実装（#1283）。**画面は自前のビューアも拡張子の表も持たない**。#1450 B2 の小さな「プレビューで開く」ボタンは**行そのもの**へ広げた（300px の帯では狙いにくく、ユーザーからは「押しても中身が見られない」に見えていた = #1472 の起票理由）。行の左には**何が開くか**（画像 / 動画 / PDF …）を出し、ホバーで「プレビューで開く」と言葉でも出す。**消えた添付（`exists=false`）は押せない**（配線ごと付けない = 実描画矩形も登録されない）。同じ添付を何度押してもプレビューのペインは増えない（`OpenFile` が同じタブのプレビューを再利用する） | M | ✅ |
| FR-2.40.23 | **画像の添付にはサムネイルを出す**。**縮小後だけを持つ**（320x180 の枠へ `thumbnail` で収め、原寸は保持しない）ので、原寸が大きくても詳細を開く速さは変わらない。上限は 2 段で、**ファイルのバイト数**（32 MB）と**ヘッダの画素数**（64M 画素）を見る。画素の検査は `decode` の**前**に置く（PNG は小さいファイルでも展開すると巨大になりうる = 解凍爆弾）。上限に当たった / 読めないときは「サムネイルなし」と出すが、**行は押せるまま**（原寸はプレビューペインが見せる）。読み込みは背景スレッドで、**描画は止めない**（載るまでは枠だけ出す）。**動画のサムネイルは作らない**: 既存の動画プレビューは ffmpeg で 1 フレーム抜くので、任意依存が要る処理を一覧の描画に混ぜない（行を押せばそのまま再生できる） | M | ✅ |
| FR-2.40.24 | **サムネイルは溜まらない**。持つのは**いま開いている 1 件のぶんだけ**で、欲しい集合を毎フレーム作って `retain` する = 「捨てる処理」をどこにも書かずに溜まらない（ポーリングの止め方と同じ形 = FR-2.40.13）。読み終わったときに選択が移っていれば捨てる | M | ✅ |

#1472 A の番犬は `issue1472a_attachment_open_watchdog.rs`（開く経路が `Request::OpenFile` 1 本 /
画面が拡張子の表を持たない / 押下が一括 dismiss から守られている / サムネイルに上限がある /
溜まらない。**注入 9 通りで `file:line` 名指し**）。発火そのものは **visual-test 項目 151**
（`TAKO_VISUAL_ONLY=task-attachment`）が**合成マウス**で押さえる: ハンドラ直呼びのテストでは
#496 の型（押下の `mouse_down` で自分が消えて `on_click` が発火しない）を検出できないため。

| ID | 要件 | 優先度 | 状態 |
|---|---|---|---|
| FR-2.40.25 | **一覧はその場で開く（アコーディオン）**。行を押すと**その行の器の子**として詳細が開き、もう一度押すと畳む・別の行を押すと乗り換える（**同時に開くのは 1 件**・設定で複数可にはしない）。#1450 B2 は一覧を全部描き切ってから**列の末尾に 1 枚だけ**詳細を積み、さらに「選択が無ければ先頭」へ落としていた（= 畳んだ状態を表せない）ので、22 件あると押した行から詳細までパネル 1 枚ぶん以上離れ「押しても何も起きない」に見えていた（#1479 の起票理由）。**展開の解決は先頭へ落とさない**（`expanded_task` は id 一致だけ）。開いた行が画面外なら `ScrollHandle::scroll_to_item` で運ぶ（背の高い詳細は行の上端がビューポートの上端へ来るので**押した行が残る**）。絞り込み（種類 / プロジェクト / 完了も見る）を変えたら畳む・**ポーリング（2 秒）では畳まない**（id で追従するので内容だけが新しくなる）・`done` / `dismiss` で消える行は畳む（畳むのは **dispatch の 1 実装**なので CLI / MCP から閉じても同じ）。開閉の矢印は svg（`CHEVRON_*`）= 絵文字を使わない。詳細の部品（`md_view::render_document` / 添付の行押下 = FR-2.40.22 / 返答フォーム / `TextField` / スレッド / 配送）は**そのまま流用**で、詳細を描く呼び出しは**画面に 1 か所だけ**（番犬が数える）。詳細は id とタイトルを繰り返さない（行が見出しなので、**展開中の行はタイトルを折り返して全文**を出す） | M | ✅ |
| FR-2.40.26 | **展開は AI からも操作できる**（設計原則 5 の例外にしない）。`tako todo expand <id>` / `collapse`（MCP は `tako_todo` の `action`）を `Request::UserTask` の同じ 1 経路へ足し、`list` の応答に `expanded` を載せて**いまどれが開いているかを読める**ようにした。`expand` は右パネルが閉じていれば `set_panel`（= `tako panel` と同じ 1 実装）で開いて tasks ビューへ切り替える（開いても画面に出ないなら操作として無意味なため）。**無い id は開いたと言わない**（存在の判定は `show` の 1 実装を通し、エラーで返して画面も動かさない）。画面が無い host（CLI 単独・テスト）では `expanded` は `null` = 嘘をつかない。展開状態は**画面の一時状態**なので永続へは書かない（`user-tasks.yaml` は無変更） | M | ✅ |
| FR-2.40.27 | **タブのバッジは幅で欠けさせない**。右パネルのタブ列（`fleet` / `orch` / `git` / `tasks` + 閉じる）が幅に収まらないと、最後に積んである tasks のバッジが右端で切れて **22 が 2 に見えた**（#1479 のユーザー原文。数字の一部だけが見えるのは「出ない」より悪い）。直し方は 2 段構えで、①**幅から詰め方を決める純関数**（`panel_tab_density`。`Full`（padding 12 + ラベル）→ `Compact`（padding 6 + ラベル）→ `IconsOnly`（ラベルを落としてアイコン + バッジ・名前はツールチップ）の順に**収まる最初の段**を選ぶ。寸法は `panel_tab_metrics` の定数表 1 か所で、字の幅の見積りは `tab_bar::CHAR_WIDTH_PX` と**同じ 1 つの数**を読む）②**構造の belt**（バッジは `flex_none`・ラベルは `min_w(0)` + `text_ellipsis`・閉じるは `flex_none`。見積りが甘くても**縮むのはラベル側**で数字は常に全桁出る。旧実装はラベルに `min_w(0)` が無く taffy が縮められないぶんがそのまま右へ溢れていた）。件数の文字は `panel_tab_badge` の 1 実装（0 件は描かない・100 件以上は `99+`。桁で列が伸び続けない。**正確な件数は一覧のヘッダに出たまま**）。保証の範囲は `PANEL_MIN_WIDTH`（240px。#1694 で条件つきのタブが 3 本とも出た 7 本でも収まるよう 220 から上げた）〜900px の全幅 × 件数 0 / 9 / 22 / 150 / 4000 で、選んだ段が収まることを単体テストが 1px 刻みで固定し、**実フレームでバッジの矩形がタブ列の内側に収まっているか**を visual-test 項目 152 が 4 幅 × 4 件数で読む。A/B は `TAKO_1479_LEGACY=1`（幅を見ずに既定 padding で並べる = 切れた形が再現する） | M | ✅ |

#1479 の番犬は `issue1479_tasks_accordion_watchdog.rs`（詳細を描く場所が 1 か所で行の器の子 /
展開が先頭へ落ちない / 押下が `toggle` と `stop_propagation` を通る / 絞り込みが畳む /
ポーリングが畳まない / バッジが幅を見て `flex_none` で描かれる。**注入 11 通りで `file:line` 名指し**）。
CLI / MCP の語彙は #1450 B1 の番犬（`ACTIONS`）が 1:1 を縛る。

### 追補: 返答欄の IME・折り返し・送信の間と返答の履歴（✅ 2026-10-13、#2016）

ユーザーの声（2026-10-13）「IME がでてこない・範囲外になったとき折り返さない・送信後に消えるまでラグ・返信履歴も
作ってほしい」の 4 症状を、visual-test `tasks-input` の A/B（`TAKO_2016_LEGACY=1` = 修正前）で真因まで実測してから直した。

| ID | 要件 | 優先度 | 状態 |
|---|---|---|---|
| FR-2.40.28 | **返答欄の IME は欄の中に出る**。真因は宛先ではなく**描画**: 返答欄は #1450 B2 から IME の宛先（`AppTextInput::TaskComment`）で確定も欄に入っていたのに、未確定文字列を描く部品もキャレット矩形の記録も無かった。変換中の読みは欄のどこにも描かれず（欄の画素差 0）、候補窓は `bounds_for_range` の既定 = ターミナルの原点（欄から約 650px 左上）に出ていた。いまは本文のキャレット位置へ未確定文字列を差し込んだ 1 本の `StyledText` を描き（下線と注目文節は `ime_highlight_ranges` の 1 実装）、キャレットは**実描画のレイアウト**（`TextLayout::position_for_index`）から採ってその矩形を `text_input_caret_bounds` へ書く（= 候補窓は欄のキャレットの下）。**IME の宛先（`enum AppTextInput`）のどの値も、その値を名指して未確定文字列を描く関数とキャレット矩形を記録する関数を持つ**ことを番犬が縛る（宛先だけ足して描画を忘れる型を次の欄で踏まない） | M | ✅ |
| FR-2.40.29 | **返答欄は欄の幅で折り返し、行数ぶん伸びて上限で止まる**。旧実装は行を縮まない子の横並びで描いていたので 183 文字でも 1 行のまま右へはみ出し、改行を積むと際限なく伸びた（31 行で 566px）。いまは 1 本の折り返すテキストで、高さは 1 行 16px × 最大 8 行（`COMMENT_MAX_LINES`）で止まり、超えたら**欄の中でスクロール**する。キャレットは**打った・動かしたときだけ**見える範囲へ寄せる（毎フレーム寄せるとホイールで遡って読めない） | M | ✅ |
| FR-2.40.30 | **送信は押した次のフレームで欄が空になり、往復を UI スレッドで待たない**。旧実装は「返す」のハンドラが UI スレッドで 積む → 運ぶ → 記録 → 一覧 を回し終えてから欄を空にしていた（debug・約 100 KB の置き場で押下に +67〜91 ms。本番の perf.log にも `dispatch:UserTask` 33〜42 ms が当日 7 件）。いまは押した瞬間に欄を空にして待ち行列へ積み、**1 本ずつ順に**（並行に積むと YAML のロックを取る順で返答の並びが入れ替わる）dispatch の "respond" と**共有の 3 段**（`user_task_respond_store` = 積む / `_deliver` = 運ぶ / `_finish` = 記録して読み直す）を ① ③ は background・host に触る ② だけ UI スレッドで呼ぶ（CLI / MCP は同じ 3 つを同じスレッドで順に呼ぶ = 1 実装）。積み終わるまでの返答は**やりとりの末尾に「送信中」**で出す（欄を先に空にしても消えたように見せない）。**積めなかったら**（対象が他所で消えた等）押した項目をまだ開いていれば**その場で欄と判断を戻し**、別の項目へ移っていたら下書きとして持って**その項目を開き直したときに戻す**（別の項目の欄へは入れない = 宛先を取り違えて送らせない）。理由は 1 行で出す。master を起こして運んだときのペインの PTY 起動（#1023 の後始末）も ② で消化する | M | ✅ |
| FR-2.40.31 | **返答の履歴は古い順・時刻つきでフォームの上に出る**。旧実装は新しい順・相対時刻でフォームと配送の行の**さらに下**に出していたので、開いても目に入らなかった。いまは会話の並び（上が古く、下へ積まれ、その下に次に書く欄）で、1 件は「時刻 · 経路（PC / スマホ / CLI / AI（MCP））· 判断」と本文（折り返し・改行もそのまま）。0 件のときは出さない（詳細の背丈を修正前と同じに保つ。最初の 1 件は押した瞬間に「送信中」で現れる）。時刻は `tako_core::user_task::response_stamp` の 1 実装（今日 = `HH:MM` / 今年 = `MM/DD HH:MM` / それより前 = 年つき。タイムゾーンは引数）で、**`tako todo show` の既定表示も同じ表記・古い順**（`  返答 11:02 [pc] needs_change: 本文`。複数行の本文は字下げで続く）。`tako todo show --json` / MCP `tako_todo` の `show` は `responses`（古い順・`at` / `via` / `decision` / `comment`）をそのまま返す = 画面と同じ履歴を AI も読める（MCP の口・引数・カタログは無変更）。スマホ版 `#/tasks` は B3（FR-2.40.16）から「やりとり」を出している（新しい順・相対時刻のまま。今回は触らない） | M | ✅ |

#2016 の A/B は **`TAKO_2016_LEGACY=1`**（返答欄・送信・履歴を修正前の形へ戻す。宛先の登録と確定の行き先は
変えない = 旧実装でも確定は欄へ入っていた）。番犬は `issue2016_tasks_input_watchdog.rs`（IME の宛先はどれも
未確定文字列とキャレット矩形を描く / 返答欄が折り返して上限で止まる / 送信が UI スレッドで dispatch を回さず
欄を先に空にする / 返答が dispatch の 3 段の 1 実装を通る / 履歴が古い順でフォームの上 / CLI と画面の時刻が
同じ 1 実装。**注入 14 通りで `file:line` 名指し**）。実測は `scripts/test-tasks-input-2016.sh`
（隔離 GUI・仮想ディスプレイ上で CLI / MCP の show と visual-test `tasks-input` を新旧で回す。IME は
GPUI の NSView へ NSTextInputClient のメッセージ = `setMarkedText:` / `insertText:` / `unmarkText` /
`firstRectForCharacterRange:` を直接送る = macOS の IME が呼ぶのと同じ入口）。


### FR-2.41 setup が意図して人に残す段（✅ 2026-09-22、#1512。エピック #1500）

> #1500 の棚卸し（[実測ログ](https://github.com/takushio2525/tako/issues/1500#issuecomment-5769886979)。
> 隔離環境での実走 R1〜R11 と 21 行の表）で洗い出した「検出・案内止まりで人の手が要る箇所」の
> うち、**自動化しないと決めた段**をここへ記録する。#1501 で「詰まった段で setup 全体を
> 止めない」（FR-2.14.12）になった以上、残る手作業は**手当ての漏れなのか意図した設計なのかが
> 区別できないと次の判断ができない**。だから 1 件ずつ「人に残す理由（技術的な壁）」
> 「tako が代わりにやること」「何が変わったら見直すか」を持たせる。
> 理由が書けない段はここに置かず、自動解決の子 Issue へ回す。
>
> **ここに並ぶ 3 件は `setup_remaining::RemainingKind`（= 末尾の「残り N 件」）には載せない**。
> あちらが数えるのは「整わないと tako が動かない」詰まり（エージェント CLI の導入 / PATH /
> ログイン / MCP 登録 / 必須依存）だけで、この 3 件は**任意の上積み**（付与・登録・配線が
> 無くても tako は動く）だから。数えると新品環境が毎回「残り 3 件」で終わり、本当に要る
> 残りが埋もれる。3 件とも棚卸しの優先度は「低」。

| ID | 段 | 人に残す理由（技術的な壁） | tako が代わりにやること | 見直す条件 |
|---|---|---|---|---|
| FR-2.41.1 | **FDA（macOS のフルディスクアクセス）**（Z12。#118） | macOS の TCC は**付与を要求する API を持たない**。付与は「システム設定 → プライバシーとセキュリティ → フルディスクアクセス」で人がチェックを入れる操作で、tako から起こせるのは**設定画面を開くところまで**。判定にも直接の API が無く、`fda::is_granted()` は保護パス（`~/Library/Mail` 等）の `read_dir` が `PermissionDenied` を返すかで代用している。さらに**付与は tako を再起動するまで反映されない**（`run_fda_check` のコメントと案内文が根拠。付与直後の再チェックは「過去に付与済みで検出が遅延していた場合」しか拾えない）。棚卸しの再現欄は**未実測**（検証機が付与済みで未付与状態を作れない） | 状態と理由を出す（`tako setup` / `tako setup --check` の `[任意] フルディスクアクセス: 未付与（推奨）` + 「許可ダイアログが頻発する原因」の説明 + `付与方法: tako fda open でシステム設定を開き、tako を追加してください`。`tako setup` は `run_dependency_check`、`--check` は診断の正本 `diagnostics::collect` の `fda` 項目で、**判定はどちらも `fda::is_granted()` の 1 実装**（#1505））。単体の口は `tako fda status` / `tako fda open` と MCP `tako_fda`（設計原則 5）。**常設導線**は設定画面の「セットアップ」タブ（状態 + 「開く」ボタン = `Request::Fda`）。`--review` だけ `[y/N]` で設定画面を開き、**付与後に再起動が要ることを打鍵つきで**出す（打鍵は `platform::keys::quit` から引く = 案内へ直書きしない）。人に残るのは「チェックを入れる」と「tako を再起動する」の 2 つだけ | ① macOS が FDA の付与要求 API（または承認フロー）を公開したら ② 付与が**再起動なしで** `is_granted()` に反映されると実測できたら（そのときは付与直後の再チェックと再起動の提案を案内へ組み込む） ③ 組織配布（MDM の PPPC プロファイル）を配布形態の要件に入れるとき |
| FR-2.41.2 | **蓋閉じ継続の sudoers 登録**（Z13。#218 / #697 / #1473） | `/etc/sudoers.d/tako-sleep-guard` の作成には root 権限が要り、**管理者パスワードの入力は人しかできない**（初回のみ）。tako はパスワードを受け取らない・保持しない（入力させるのは OS の認証ダイアログ）。実測は R8（`[情報] 蓋閉じ防止: 未設定（tako sleep-guard install-lid-sleep で有効化）`）。**Windows にはこの段が無い**（電源プランを非管理者で書けるため。呼び出し側は手段ではなく「未完了か」= `lid_setup_pending()` で分岐する = #697） | 状態行を出す（`tako setup --check` の `[情報] 蓋閉じ防止: 未設定（tako sleep-guard install-lid-sleep で有効化）`。`while-agents-running` を選んでいるのに未登録なら `[OK]` ではなく `[不足]` で区別する）。標準 `tako setup` はスリープ防止の現在値と変更コマンドまでで、蓋閉じの案内は `--review` で L3（蓋閉じでも稼働）を選んだときに出る（**初回のみ管理者パスワードが要ること**と解除コマンドを併記する）。登録の実行そのものは `sleep_guard::install_sudoers()` が osascript の `with administrator privileges` で **OS にパスワードを聞かせる**（tako の画面にパスワード欄を作らない）。書くのは 1 ファイルで、`pmset -a disablesleep 0` / `1` の 2 行だけを `visudo -cf` で構文検証してから 0440 root:wheel で配置する。解除は `tako sleep-guard remove-lid-sleep`。3 口（CLI `tako sleep-guard install-lid-sleep` / MCP `tako_sleep_guard` の `action=install-lid-sleep` / 設定画面「スリープ防止」タブのボタン）が**同じ dispatch** を通る。人に残るのは「管理者パスワードを 1 回入れる」だけ | ① 管理者権限を要さずに蓋閉じを制御できる手段が macOS 側に生まれたら ② 「`while-agents-running` にしたのに登録していないので効いていない」に利用者が気づけない実例が出たら、**分類は変えずに案内の出し方**（`[不足]` 行の見せ場所・通知欄 = FR-4.10.4）を見直す |
| FR-2.41.3 | **設定共有の配線先**（Z16。#513 / #793） | **どの git リポジトリへ置くかは人の意思**（既存の dotfiles に相乗りするか / 新しく作るか / public か private か）で、検出だけでは一意に決まらない。しかも配線は**外部への副作用**（リポジトリ作成・push）を伴うので、`--yes` / 非 TTY では動作も代行の案内も出さないと FR-5.14.10 が決めている。実測は R5（``[情報] 設定共有: 未配線（複数デバイスで同じ AI 設定を使うなら `tako config init`）``）と R7（`--review` で y/N + リポジトリ / リモート URL の対話入力） | 読み取りだけの検出（`config_share::env::detect`）と 4 種の案内（`guidance` = `linked` / `broken` / `adopt_existing` / `fresh`。setup サマリと `--check` が同じ判定を通る = FR-5.14.9）。**既存の外部 git 運用があれば相乗り（`tako config link <既存リポジトリ>`）を第一案として出す**（別リポジトリを作ると同じ内容が 2 箇所に並ぶ）。配線済みなら勧誘文言を出さない（冪等）。配線の口は `--review` の y/N 1 回 / `--answers` / MCP `tako_setup` の `config_share` / `tako config init`・`link`（MCP `tako_config_share`）。setup 対話アシスタントは `setup-context.yaml` の `config_share` を読んで代行でき、リポジトリ作成と push は**合意を得てから・private 既定**（FR-5.14.11）。人に残るのは「どのリポジトリを使うか決める」だけ | ① 置き場を人が選ばずに済む既定（tako 管理の private リポジトリの自動作成など）を**同意なしの外部副作用として許す**と判断が変わったとき（= FR-5.14.10 の見直しとセット） ② `adopt_existing` の検出が「相乗り先が 1 つに決まる」確度まで上がったら、その場合に限り `--yes` でも配線してよいかを再検討する |

#### 判断待ち（#1511。分類が決まっていない行）

**「意図して人に残す」にも「自動解決する」にもまだ分類しない**。sudo と他アプリの起動という
既存方針（#868 / #1129）の変更を伴うので、#1511 でユーザーが決める。

| # | 段 | 現状（棚卸しの実測） | 決めること |
|---|---|---|---|
| Z7 | Homebrew 自体が無い機での依存導入 | `導入方法: brew install tailscale（要 Homebrew: https://brew.sh）` の案内どまり。`tako setup deps install` も `[skip] … brew が見つかりません`（R11） | Homebrew のインストーラが管理者パスワードを求めるため #868 で意図的に代行していない。① brew 以外の経路を持つ dep はそちらを使う ②「Homebrew を入れますか？」まで出して**パスワードは OS に聞かせる**（FR-2.41.2 と同じ形）のどちらを採るか |
| Z18 ① | Tailscale デーモンの起動代行 | `tako remote setup` の [2/5] が `デーモンが起動していません` で exit 1（R9 / R10。未ログインの側は**未実測** = 検証機がログイン済み） | GUI 版が入っていれば `open -a Tailscale` で代行できるが、「他人のアプリを起こす」判断が要る。②の系統選択（既定 1）は現状のまま非対話で進む |

結論が出たら、自動化するなら #1511 の子 Issue で実装し、人に残すならこの節へ 4 列の行として移す。

補足: 棚卸し表で「済」として Issue 化しなかった行は Z14（スリープ防止。既定
`while-agents-running` / `ac-only` で実質設定済み）と Z15（MCP 登録。自動登録され、未導入の
codex / agy は理由つき skip）。自動解決へ回った行は #1499（Z5）/ #1501（Z2 + Z3 + Z4）/
#1502（Z10）/ #1509（Z6）/ #1506（Z11）/ #1503 / #1504（Z9）/ #1505（Z19）/ #1524 / #1507（Z17）が着地済みで、
#1508 / #1510 が進行中。

### FR-2.42 tako mod（Claude Code の mod 連携）の基盤（✅ 2026-10-08、#1879。エピック #1877 の S1。S2 #1880 / S3 #1881 / S7-1 #1959 / S7-2 #1960 / S7-3 #1962 / S7-4 #1963 / 監視の走査の置き換え #2017 / 残りの走査 #2041 を含む）

> tako が同梱する Claude Code の mod（プラグインの関数フック。2.1.287 以降）をペインの claude に
> 読ませ、ctx・使用制限・ターンの状態を**構造で**受け取る。設計の正本は
> `.agent/plans/2026-10-tako-mod.md`（通信路の比較 §2・置き場 §3・報告 §4・mod の規約 §5）。
> S1 は「届くようにする」まで。届いた値を一次ソースに使う（S2 = FR-2.42.9〜12、#1880。続きの #1903 =
> FR-2.42.11 の束ね方・FR-2.42.18 のステータスバー）・
> Claude Code の画面に出す（S3 = FR-2.42.13〜17、#1881）・定型の UI 設定 ui.json（S7-2 = FR-2.42.19〜24、#1960。
> 設計は同じ設計書の §9.7）・設定 dir の `skills/tako` へ setup で入れる（S7-1 = FR-2.42.25〜33、#1959。
> 設計は同じ設計書の §9.4〜9.6）・帯を他の mod と共存させ、カスタムボタンと使用制限 / ctx のバーを mod で描く（S7-3 = FR-2.42.34〜40、#1962。
> 設計は同じ設計書の §9.3 / §9.5 / §9.6）・コマンドカードを mod で描き、描いたカードだけ GPUI の帯から外す（S7-4 = FR-2.42.41〜45、#1963。
> 設計は同じ設計書の §9.2 / §9.3 / §9.6）・会話の行のカードをクリック 1 回で実行する（FR-2.42.53、#2159。設計は同じ設計書の §9.12）・
> Claude Code から tako を操作する（S4）・チャット表示（S5）は後続のスライス。

| ID | 要件 | 実装 |
|---|---|---|
| FR-2.42.1 | **同梱と展開**: mod のソースは `crates/tako-core/claude-mod/`（`shell-integration/` と同じ並び）。GUI 起動時と `tako setup` で `<data_dir>/claude-mod/tako/` へ展開する（冪等・中身が同じなら書かない・違えば一時 dir に書いてから差し替える）。**`.app` の中には置かない**（Claude Code が読むたびに `.claude-plugin/types/` を書くので署名済みの bundle が改変される） | `tako_core::claude_mod::install` / `tako_control::claude_mod::run_setup_stage`。`plugin.json` の版と `register.ts` の `MOD_VERSION` は展開時に tako の版へ揃える |
| FR-2.42.2 | **注入はペインの env**: 新しく作るペインに `CLAUDE_CODE_PLUGIN_DIRS`（展開先。既存値は上書きせず連結）と `TAKO_CLI`（実行中の tako CLI の絶対パス）を足す。**settings 系のファイル（`settings*.json`・`plugins/*.json`）は書かない。Claude Code の設定 dir に置くのは `skills/tako` の管理印つきの写しだけ**（#1959 = FR-2.42.25〜。写しは tako の外の claude にも読まれるが、`TAKO_PANE_ID` / `TAKO_CLI` が無ければ休眠する）。その claude の設定 dir に別の出どころの同名 `tako` があるか、利用者が `/plugin` で止めていれば注入しない（FR-2.42.27 / 2.42.31）。tmux ペインはセッション作成時に `-e` で固定し（注入しないペインは「注入しない」を固定 = サーバーのグローバル環境に残った前の値が漏れない）、`update-environment` にも足す。app プロセスから起こす `claude -p`（自動リネーム等）には注入しない | 判断は `claude_mod::decide`、env は `claude_mod::pane_env` / `neutral_pairs` の 1 実装。直接ペインは `spawn_session`、tmux は `backend::session_pinned_pairs` |
| FR-2.42.3 | **版の下限と止め方**: 下限は**実測した 2.1.294**（2.1.287〜2.1.293 は未実測。旧版に読ませると transcript に読み込み失敗の 1 行が出る）。版はパスから読む安い判定を起動時に、読めなければ上限つきの `claude --version` を背景で 1 回（以後は stale 検知の走査が追従）。未判定・下限未満なら注入しない。設定 `claude_mod`（settings.json。既定 true。旧ファイルはキーが無くても true で読める）と A/B の `TAKO_1877_NO_MOD=1`（同一バイナリで旧挙動。展開もしない） | `stale_binary::quick_claude_version` / `bounded_claude_version`。`tako mod on\|off` は次に作るペインから効く |
| FR-2.42.4 | **報告と鮮度**: mod は `tako mod report`（stdin の JSON。`schema: 1`）で ctx・使用制限・model・effort・turn（idle / busy / permission / question）・待っているツール名・直前のターンを送る。変化があれば最大 1 秒に 1 回、無くても 15 秒の heartbeat。tako は**GUI のメモリだけ**に持ち（永続化しない）、受け取りから 45 秒で失効・`session.end` で即座に捨てる・閉じたペインの分も捨てる。送り主は `TAKO_PANE_ID`（tako の再起動をまたいだ claude は #210 の旧 ID → 新 ID で読み替える。接続は CLI の control.json フォールバック = FR-2.2.9）。**会話の本文・プロンプト・ツールの引数は載せない**（未知のキーは serde が捨てる） | `tako_core::claude_mod::{ModReport, ModHub}`・`tako_control::claude_mod`。番犬 `issue1879_claude_mod_watchdog.rs` |
| FR-2.42.5 | **状態を黙らずに出す**: `tako mod`（MCP `tako_mod`）は展開・claude の版・新しいペインへ注入するか（しないなら理由）と、ペインごとの行（`reporting` / `stale` / `no_report` / `not_injected` / `waiting`）を返す。claude が画面に居るのに報告が無いペインは理由（`--safe-mode` / `--bare` / `disableAllHooks` / 組織の `allowManagedModsOnly` / 注入前から動いていた claude）を出す。#1891 からは候補を並べず判別した 1 つ（FR-2.42.54）| `claude_mod::status` |
| FR-2.42.6 | **設計原則 5 の例外**: `report` は CLI だけで MCP に載せない。AI が叩くと**自分の状態を偽って注入できるだけ**で、AI にとっての等価物（読む側の `tako_mod` / `tako_orchestrator_self`）は別にあるため。`tako mod report` は `--help` の候補にも出さない（受け付けはする） | `claude_mod::MCP_ACTIONS`（status / on / off / band-on / band-off / ui / install / uninstall） |
| FR-2.42.7 | **classic 系が届かない環境**: 2.1.294 の**組織アカウントでは `classic.*` が mod へ届かない**ことを実測した（`PreToolUse` / `PermissionRequest` / `Notification` を `on('*')` で観測して 1 つも来ない。関数フックの `tool.call` / `tool.check` / `turn.*` は届く。個人アカウントでは同じ版で 3 つとも届く）。effort は classic 系が運ぶので、この環境では欠ける（S2 で関数フックの `turn.step` からも拾うようにした = FR-2.42.12）。権限待ちは PermissionRequest（届けば正確）を正とし、届かないときは**ダイアログが確かに出るものだけ**を拾う: AskUserQuestion は `tool.call` の開始で `question`、ask ルールに当たった判定と ExitPlanMode は `tool.check` で `permission`。ルールの無い `ask` は auto モードの分類器が黙って通しうるので拾わない。どちらの環境かは報告の `classic_events` に載せる（S2 が画面と突き合わせる材料）。承認の瞬間を知らせるイベントは無いので、`permission` はそのツールが返るまで続く | `claude-mod/hooks/register.ts` |
| FR-2.42.8 | **mod の作法**（設計書 §5）: すべての登録は `next(e)` へ流して `.catch(($, e, next) => next(e))` を付ける（判断を奪わない。ストリームのフック `turn.step` は async generator でしか書けないので `.catch(async function* ($, e, next) { return yield* next(e) })` = 同じ意味の素通し）・フックの中で CLI を待たない（1 秒の flush が叩く。`session.end` の最後の 1 回だけ待つ）・`TAKO_PANE_ID` か `TAKO_CLI` が無ければ休眠・失敗は debug ログだけ・サブエージェントのターンは数えない。`claude plugin validate` / `claude plugin test`（`tests/*.test.ts`）が通ること。検査の本体は `scripts/check-claude-mod.sh` の 1 本（夜間リリースの前段と `scripts/test-claude-mod-1879.sh` の段 0 が呼ぶ。注記の文言が変わったら落ちる自己検査つき = #1903） | `scripts/check-claude-mod.sh` |
| FR-2.42.9 | **ctx% の一次ソース（S2 #1880）**: 新鮮な報告（45 秒以内）の `context.percent` を**画面より先**に採る（`ctx_source: "mod"`。窓 `ctx_window` も claude が答えた値）。statusLine を設定していない claude（画面に ctx が出ない構成）でも取れる。初回応答の前（`percent` が欠ける）は落ちるが、窓は mod の値を transcript の計算に使う。4 経路（`orchestrator self` / `worker_status` / #749 の自動ハンドオフの tick / チャットヘッダの残量バー）が同じ 1 実装を通る。画面も数値を出していれば差を `ctx_screen_delta` と `warnings` に出す | `ctx_usage::resolve_full` / `claude_ctx::resolve`。tick の値は `orchestrator self` の `auto_handoff_tick`、ヘッダの値は `tako ui-mode` の `chat_header` で読める |
| FR-2.42.10 | **落ちた理由を黙らない**: mod を使えなかったときは今の経路（画面 → transcript）へ落ち、理由を `ctx_mod_reason` / `mod_reason` に出す（`mod_absent` / `mod_stale` / `disabled` / `claude_too_old` / `legacy_env` 等。`tako mod` の行の `reason.code` と同じ語彙。`mod_no_usage` = 報告は新鮮だが ctx% がまだ無い）。`ctx_reason` は「ctx% が null の理由」のまま変えない。A/B は `TAKO_1877_S2_LEGACY=1`（報告は受け取るが一次ソースに使わない = #1880 前の挙動）。**`tako mod`（status / on / off / band on\|off / install / uninstall）の応答の文と CLI の表示は表示言語に従う**（`reason.message`・行の理由・skills の状態と衝突の文・`applies_to`・`install` の失敗・mod を扱わない tako への断り。語彙の `code` は言語で変わらない）。枠が日本語の `tako check-health`・setup・persist.log は同じ理由を日本語で引く = 1 つの出力に日英を混ぜない（#2365） | `claude_mod::lookup` の 1 本（番犬 `issue1880_mod_primary_watchdog.rs` が `fresh_report(` の直呼びを落とす） |
| FR-2.42.11 | **使用制限**: 5h / 7d の % と解除時刻を mod から採る（`orchestrator self` / `worker_status` の `rate_limits`。キー `limited` / `reset_at` は codex #985 と揃える。`reset_at` は unix 秒・**秒精度**）。使用制限は**アカウント単位**なので同じ `config_dir` の新鮮な報告を束ね、窓の種類ごとに**最新の観測**（`observed_at` が新しい。同時刻なら `resets_at` が遅い → % が大きい）を採る。mod は**値（% か `resets_at`）が変わったときだけ** `observed_at` を打つ（#1903。heartbeat では前の時刻のまま。窓ごとの時刻は `$.state` に置くのでホットリロードをまたいでも保つ）ので、放置したペインの古い % は古い観測のまま届き、動いているペインの値が勝つ（上限の引き上げ・早めのリセットで % が下がっても追従する）。鮮度の判定は tako の受信時刻で別に持つ。A/B の `TAKO_1903_LEGACY=1` は #1880 の束ね方（`resets_at` が遅い → % が大きい）。**上限での停止の判定は画面のまま**（#813 の安全条件）: 自動復帰には上限に当たった窓の解除時刻を `LimitHint` として渡すだけで、mod の値だけでは停止と判定しない | `ModHub::account_rate_limits` / `limit_reset_at` / `LimitHint::from_mod` |
| FR-2.42.12 | **ターン状態・権限待ち・質問待ち**: `worker_status` は新鮮な報告の `turn` を `claude agents --json` より先に見る（`status_source: "mod"`。watch の確定は一次シグナルと同じ 3 回）。`permission` / `question` は承認・回答の後もツールが返るまで残るので、画面が生成中なら busy を採る。**答え方（respond）は画面のダイアログを読むので**、mod と画面のダイアログ検出が食い違えば画面を正にして `warnings` に出す（`classic_events: false` の環境 = 組織アカウントでは、ルールの無い ask の権限ダイアログを mod から拾えない）。`read` は mod が待っていると言い画面が生成中でなければ `input_status` を null にする（ダイアログの選択カーソルを入力欄の下書きと読み違えない）。mod のモデル・effort は `mod_turn` に載る（effort は関数フックの `turn.step` から拾う = classic 系が届かない環境でも欠けない） | `ModTurn::status_word` / `ModSnapshot`（UI スレッドで写す） |
| FR-2.42.13 | **帯（S3 #1881）**: プロンプトの上の帯（`ui.render` の `AbovePrompt`）に **1 行**で `tako \| ペイン名 \| タブ <名> \| worker N \| 要注意 M \| ctx P% \| 5h P% \| 7d P%` を出す。**ctx / 使用制限は tako が閾値（80% / 80%。チャットヘッダの残量バーの警告色と同じ）を超えたと判断したときだけ**（statusLine を持つ利用者の画面で二重にならない既定）。worker 0 本なら worker 数も要注意も出さず、タブ名がペイン名と同じなら出さない。名前は 24 桁で切り詰める。幅（`bodyColumns` = 端末の幅 − Claude Code の `[-]` の 5 桁）に収まらなければ**優先度の低い区切りから落とす**（タブ名 → worker 数 → 使用制限の小さい方 → ctx → 要注意。`tako` とペイン名は落とさない）、それでも溢れたら `wrap="truncate-end"` で末尾を切る = 必ず 1 行（S0 で 21 行のペインが `↓ 2 more` に畳んだ問題の解）。worker は右パネル orch と同じ規則（`Workspace::workers_of` = `spawned_by` + 唯一の master への寄せ）。**要注意** = 承認待ち（mod）・質問待ち（mod）・選択肢ダイアログ（画面。mod の無い codex / agy の worker も拾う）・使用制限の到達・非ゼロ終了。**「承認待ち」は帯に出さない**（権限ダイアログ・AskUserQuestion の表示中は Claude Code が帯ごと隠す。閉じれば戻る）。tako の外（`TAKO_PANE_ID` / `TAKO_CLI` が無い）・tako の応答が 45 秒途切れた・調査票が帯を使っている間は描かない。帯の材料は報告の応答で届くので、**worker の状態の変化はこのペインの次の報告（変化があれば 1 秒・無ければ 15 秒の heartbeat）で帯に出る**（tako からの常時 push は S6 #1884）。**S7-3（#1962）から**は他の mod の行を包み、tako の行はボタン込み・並びは ui.json の `band.segments`・バーを描くときは ctx / 使用制限の警告を外す（FR-2.42.34 / 36） | 判断は `tako_core::claude_mod::{band_view, classify_worker, band_warnings}`、詰めるのは `register.ts` の `fitBand` |
| FR-2.42.14 | **`/tako` のサイドバー**: `$.ui.open` のペインに詳細（ペイン名・タブ・ctx の tokens / window・使用制限の全窓とリセットまでの残り・worker の一覧（要注意を先に、状態の語つき・最大 24 本）・帯に出す閾値・帯のトグルのボタン）。**頼まれずには開かない**（`/tako` からだけ。コマンドから開けば幅を問わず置かれる。置き方は Claude Code が決める = 実測では隔離 tmux の 144 桁で右に docked）。**設計原則 5 の例外**: サイドバーを開く操作そのものは CLI / MCP に無い（中身は AI にとって等価な `tako mod` / `tako orchestrator workers` で読める。Claude Code の画面の中の表示を tako から開くには tako → mod の常時 push = S6 #1884 が要る） | `register.ts` の `ui.render`（`Pane`）と `command.run`（`/tako`。`immediate`） |
| FR-2.42.15 | **帯のトグル**（#1960 で正本を ui.json の `band.hidden` / `toggled_at` へ移した = FR-2.42.24。以下は S3 の記録）: mod の `$.store` の `band`（`{hidden, at}`）が正本で、claude の再起動後も保たれる（実体は `<設定 dir>/plugins/store/tako_inline-<hash>.json`。hash は mod の名前で決まり置き場のパスに依らない = 設定 dir ごとに 1 つ）。切り替えの口は `/tako band on\|off`・サイドバーのボタン・**`tako mod band on\|off`**（MCP `tako_mod` の `band-on` / `band-off`。報告の応答の `view.band_request` で全 mod へ中継 = 次の報告（最大 15 秒）で届く。中継は GUI のメモリだけ）で、**時刻の新しいものが勝つ**（後から Claude Code の中で切り替えればそちらが勝つ）。同じ設定 dir の別セッションは報告のたびに `$.store` を読み直して揃う。`$.store` は Claude Code の設定ファイル（`settings*.json`）ではなく、トグルを変えたときにだけ書かれる | `ModHub::request_band` / `register.ts` の `setHidden` / `syncStore` |
| FR-2.42.16 | **観測**: 報告に帯の状態 `band`（`hidden` / `shown` / `columns` / `segments` = 描いた区切りの種類 / `toggled_at`。**描いた文字列は載せない**）を載せ、`tako mod` の行（`report.band`）に出る。status の `band` に中継（`request`）・ui.json の `hidden`（#1960）・A/B（`legacy`）・閾値、`ui` に ui.json の様子（`state` / ボタンの id）。報告の応答の `tako.view` が帯・サイドバーの材料（`claude-mod/types/index.d.ts` の `TakoView` = `BandView`。#1960 から `ui` = `TakoUi` も載る） | `claude_mod::status` / `snapshot` |
| FR-2.42.17 | **A/B と頑健さ**: `TAKO_1877_S3_LEGACY=1` で応答に `view` を載せない（報告と一次ソース化はそのまま = mod は描かない = #1881 前の挙動）。検証用の `TAKO_1881_BAND_THRESHOLD=<%>` で閾値（ctx / 使用制限の両方）を差し替える（`0` で区切りを全部並べて 1 行に収まるかを測る）。帯の準備（`$.store` の読み・`/tako` の登録）が失敗しても報告（S1 / S2）は止めない | `scripts/test-claude-mod-band-1881.sh` / `claude-mod/tests/band.test.ts`（terminal と desktop の両 surface） |
| FR-2.42.18 | **ステータスバーの 5h / 7d（#1903）**: 画面下のステータスバーの使用制限メーターも mod の値を先に見る。フォーカス順（フォーカス → 他のペイン）で**最初に mod の使用制限が引けたペインのアカウント**の束ねた値（FR-2.42.11 と同じ `account_rate_limits` の 1 実装）を採り、`five_hour` / `seven_day` の窓が 1 つでもあれば mod だけで決める（画面の値と窓ごとに混ぜない）。% は帯と同じ四捨五入。どのペインにも無い・報告が古い（45 秒）なら画面の値（#217 の `5h NN%` / `7d NN%`）へ戻る。取得元は `tako limit-service --refresh`（MCP `tako_limit_service` の `action=refresh`）の `claude.source`（`mod` / `screen` / null）で読める。A/B の `TAKO_1903_LEGACY=1` では画面の値だけ（#1903 前）。`TAKO_1877_S2_LEGACY=1` でも mod を見ない | 判断は `tako_core::claude_mod::bar_limits`、ペインの選び方は `tako_control::claude_mod::status_bar_limits`、GUI は `refresh_agent_metrics` |
| FR-2.42.19 | **定型の UI 設定の置き場と正本（S7-2 #1960）**: `<data_dir>/claude-mod/ui.json`（`schema_version: 1`。**tako の data dir**に置き、Claude Code の設定 dir へは書かない = FR-2.42.2 と矛盾しない）。既定・語彙・検証・操作・書き込みは `tako_core::claude_mod_ui` の 1 実装で、書き込みは同じ dir の一時ファイル → rename（読み → 当て → 書きは `config_io::lock_exclusive` で直列）。**読むだけでは書かない**（ファイルが無ければ既定で動き、最初に変えたときに作る）。設定共有（#513）では settings.json と同じ宣言的な設定（`Shared`。`claude-mod/` の残りは生成物で `Local`） | `tako_core::claude_mod_ui` / `tako_control::claude_mod_ui` |
| FR-2.42.20 | **選ぶだけの語彙（任意の JS は持たない）**: 帯の区切りと順番 `band.segments`（pane / tab / workers / attention / ctx / limits / buttons / card。重複なし・1 つ以上）・`band.hidden`・使用量のバー `usage_bar.place`（prompt_hint / band / off）と `usage_bar.items`（five_hour / seven_day / ctx）・`cards.place` / `chat.place`（auto / mod / tako）・`chat.bubble` / `chat.tool_summary`（always / gui_only / off）・`chat.code_copy`・色 `colors.accent` / `warn` / `dim`（Claude Code 2.1.294 の `ThemeKey` のうち UI の部品に使う 14 個。RGB は持たない）。ボタンは **8 個まで**・label **16 桁**（全角 2 桁）・hotkey は**数字か英小文字 1 字で重複不可**（省くと label の文字から空いている英字を選ぶ = 空の入力欄で数字が帯のボタンを押すエンジンの仕様に当たらない）・id は `[a-z0-9-]` 24 字。動作は 4 種: `slash`（compact / clear / context / cost / model / effort / tako の許可リスト）・`tako`（#1882 の対応表の操作 id 9 つ）・`shell`（FR-2.22.7 と同じ 1 実装 `command_card::normalize_command_text` で検証し、新しい tako のペインで実行）・`prompt`（入力欄へ入れるだけで送らない。1000 文字・制御文字不可）。**既定はボタン `compact`（`/compact`）1 個**。語は大文字小文字・`-` / `_`・先頭の `/` の揺れを同じ値へ落とす（`5h` / `7d` の別名も） | `claude_mod_ui::{Vocab, SetKey, ButtonAction}` |
| FR-2.42.21 | **口は 3 つで同じ 1 実装（設計原則 5）**: CLI `tako mod ui`（引数なしで今の値と**選べる値**・例）/ `set <キー> <値>` / `button add [<kind>] <中身> [--label] [--hotkey] [--id]`（kind は slash / tako の語なら省ける = `tako mod ui button add compact`。#322）/ `button remove <id>` / `button move <id> <1〜8\|first\|last\|left\|right>` / `preset <default\|recommended\|minimal>` / `reset`（**ローカル処理 = GUI が無くても変えられる**）、MCP `tako_mod` の `action=ui`（`op` / `key` / `kind` / `preset` は inputSchema の enum）、`tako setup`（FR-2.42.23）。**不正な値は書かずに**理由と許される値の一覧を返す（文言の組み立ては 3 つの口で同じ 1 実装で、日本語表示では字面も同じ。CLI は終了コード 1、MCP は isError。**CLI / MCP の文言は表示言語に従い**（断り・引数の読み方の断り・成功の応答の枠 `applies_to` / `note` / `choices` の説明と `problems`・CLI の人向けの表示まで）、setup は枠が日本語なので日本語で返す = 1 つの応答に日英を混ぜない。#2365）。操作の列は 1 つでも断られたら何も変えない。同じ動作のボタンを足すのは冪等 | `claude_mod_ui::{UiRequest, UiOp, apply_all}` / `tako_control::claude_mod_ui::{cli_request, run}`。番犬 `issue1960_mod_ui_watchdog.rs`（3 つの口で ui.json が字面で一致・注入を file:line で名指し） |
| FR-2.42.22 | **壊れた・新しい ui.json**: 部分ごとに読み、壊れた部分（語彙の外・形の違い・余計なキー・重複・9 個目以降のボタン）だけ既定へ落とす。元の中身は `.unreadable.bak` へ保全し（#916 の `quarantine_unreadable`。文字として読めない中身 = UTF-8 でない・権限で読めないものはバイトのまま番号付きの退避へ移す = `LoadIssue::read_failed` / `quarantine_raw`。2 回目に別の中身で壊れても残る = #2238）、persist.log へ 1 行（**壊れた中身ごとに 1 回**。文言にはキーの位置と種類だけを載せ、利用者が書いた値は載せない）。GUI・CLI・MCP・setup のどれも落ちずに既定で動き、次の変更の操作で直した形を書く。`schema_version` が大きい（新しい tako が書いた）ファイルは読める範囲で使い、**書き換えない**（変更の操作は理由つきで断る）。#916 の登録簿は `SchemaId::ClaudeModUi`（版数は `schema_version`。`tako migrate status` の健康診断に載る） | `claude_mod_ui::{parse_lenient, load_from, validate_text, detect_version}` / `migrations::SPECS` |
| FR-2.42.23 | **setup の段**: 標準の `tako setup` は今の値を 1 行で見せるだけ（質問ゼロ = #262）。`--answers` の `mod_ui`（`preset` → `set`（キー → 値）→ `buttons`（並びの置き換え）の順に当てる。MCP `tako_setup` も同じキー）は `SetupAnswers::validate` が書く前に検証する（setup の途中で半端に止まらない）。`--review` の端末では setup の**最後**に雛形の 3 択（既定 / おすすめ = compact・context・split right・handoff / 最小）とボタンの番号選び（組み込みコマンド 7 + tako の操作 9）を聞き、`mod_ui` と同じ形にして同じ 1 実装で当てる（既存の対話の答えの順を変えない。問いは `選択 [` を含む）。setup の対話アシスタントには `tako mod ui` の口だけで調整するよう案内する（軽いモデルでも語彙の外を書けない） | `tako_control::claude_mod_ui::run_setup_stage` / `tako-cli` の `review_mod_ui` / `resources/setup/system-prompt.md` |
| FR-2.42.24 | **mod へ届く形**: mod はファイルを読まず、報告の応答 `tako.view.ui` で**検証済みの値**を受け取る（`types/index.d.ts` の `TakoUi`。変更は次の報告 = 最大 15 秒で届く）。帯のトグルの正本は ui.json の `band.hidden` / `toggled_at`（epoch ms）で、応答の `band_request` はここから作る（`tako mod band on\|off` = `tako mod ui set band.hidden`。今の mod は `band_request.at` が自分の `$.store` より新しいときだけ従う）。mod の中で切り替えた（報告の `band.toggled_at` が新しい）ぶんは tako が ui.json へ取り込む（`$.store` は読み込み元ごとに別ファイル = 設定 dir や導入方式が変わってもトグルが消えない）。A/B の `TAKO_1877_S3_LEGACY=1` では取り込まない。ボタン・バー・区切り・色の描画は #1962（FR-2.42.34〜36） | `claude_mod::{band_view, BandView::ui}` / `tako_control::claude_mod_ui::{view, import_band}` |
| FR-2.42.25 | **設定 dir へ入れる（S7-1 #1959）**: 設定 dir ごとに `<設定 dir>/skills/tako/` へ同梱の mod の**写し**を置く（Claude Code 2.1.294 は `tako@skills-dir` として読み、`/plugin` に `✔ loaded` で出る・走っているセッションにもホットリロード）。**symlink にしない**（Windows の権限・dotfiles 同期でホームパス入りの link が他の機械へ渡る）。中身は data dir の展開先ではなく埋め込みから作るので、tako を消しても写しは自立して休眠する。一時 dir（`skills/.tako.tmp-*`）に全部書いて**印を最後に**置き、rename で差し替える（dot で始まる dir を Claude Code は plugin として読まない = 実測）。中身が同じなら書かない。Claude Code が書く `.claude-plugin/types/` は比較から外して引き継ぐ。**設定 dir が無ければ作らない**（`no_config_dir`）。`tako setup` ではこの段を**指示ファイルの段の後**に置く（新品の HOME の `~/.claude` は指示ファイルの段が CLAUDE.md と一緒に作る = 1 回目から入る。claude 以外の系統を選んで`~/.claude` が作られないときは入れない）。setup で書いた・外した dir は最終サマリ（「セットアップ結果」）に載り、置き直しは前後の版で出る（#2292。前に置いていた頃は 1 回目に黙って入らず、2 回目に入っても「変更なし」と出ていた）。`skills/` が無ければ作る。置く先は accounts.yaml の設定 dir（`AccountsConfig::list_resolved`。inherit は既定）+ 既定 `~/.claude` + プロセスの `CLAUDE_CONFIG_DIR` + mod の報告の `config_dir`（FR-2.42.29） | `tako_core::claude_mod_install::{write_copy, plan_sync}` / `tako_control::claude_mod_install::collect_targets` |
| FR-2.42.26 | **管理印と利用者のもの**: `skills/tako/.tako-managed`（JSON: `schema` / `managed_by: "tako"` / `tako_version` / `content_hash` = 置いたファイルの FNV-1a / `files`）。**印の無い `skills/tako`・symlink は利用者のもの = 触らない**。印があっても中身が印のハッシュと違えば利用者が手を入れたもの = 上書きも削除もしない。印が壊れている（読めない・形が違う）ものは tako の写しとして置き直す・外す（印の名前は tako しか使わない）。印のファイル一覧が設定 dir の外を指すもの（`..`・絶対パス）は印として読まない。書式は #916 の `migrations::SPECS` に `claude_mod_mark` として登録（**形式の検査を持たない** = 退避すると写しが利用者のものに見えて以後触れなくなる。形を変えるときも移行の手順は要らない = 同期が印ごと書き直す） | `CopyState` / `parse_mark` / `SchemaId::ClaudeModMark` |
| FR-2.42.27 | **同名の衝突**: 別の出どころの `tako`（`skills/<別名>/.claude-plugin/plugin.json` の `name: tako`・`plugins/installed_plugins.json` の `tako@<ほか>`・settings.json の `enabledPlugins` で有効な `tako@<ほか>`。どれも**読むだけ**）があればその設定 dir には置かず（既に置いた写しは**退く**）、`name_conflict` を出す。Claude Code は同名の plugin を先に来た方だけ黙って読み（2.1.294 で実測: `skills/tako` が `skills/my-tako` の `name: tako` に勝つ）、inline（env）は installed / skills-dir を上書きするので、**その設定 dir の claude のペインへは env も注入しない**（`not_injected` / `name_conflict`。ペインの設定 dir はペインの env → tako 自身の env → 既定の順に見立てる）。plugin ではない `skills/tako`（`SKILL.md` だけ）は置けないが注入は止めない | `inspect` / `Conflict::blocks_injection` / `claude_mod_install::refine_injection` |
| FR-2.42.28 | **入れない条件**: claude の版が下限（FR-2.42.3 の 2.1.294）未満（`claude_too_old`）・未判定（`claude_unknown`）・`tako mod off`（`disabled`）なら置かない・置き直さない（置いた写しはそのまま休眠）。A/B の `TAKO_1959_LEGACY=1`（`TAKO_1877_NO_MOD=1` も）で #1959 前 = 入れない・衝突でも注入を止めない | `claude_mod_install::gate` |
| FR-2.42.29 | **発火点（#916 と同じ二段構え）**: `tako setup` の段（claude を検出した後。MCP `tako_setup` も同じ CLI を通る）と GUI 起動時（claude の版が分かった後に背景で 1 回）。GUI 起動後に mod の報告で**新しい `CLAUDE_CONFIG_DIR`** が見えたら、その dir だけを背景で同期し persist.log に「報告で見えた設定 dir」と残す（版が分かるまでは積んで待つ）。setup は初めて置くときだけ「何をどこへ・効くもの・戻し方」を出してから置き（#262 の質問ゼロ。FR-2.14.14 と同じ作法）、2 回目は 1 行。**止めない**（置けなくても env の注入で動く）。書いた・外した・失敗は persist.log へ | `run_setup_stage` / `sync_for_gui` / `ModHub::take_pending_config_dirs` |
| FR-2.42.30 | **外し方**: `tako mod uninstall`（MCP `tako_mod` の `action=uninstall`）= 印つきの写しだけを外す（退避名へ rename してから消す = 半分消えた plugin を見せない）。手の入った写し・利用者のもの・他の skills と設定は残す。`--dry-run`（MCP `dry_run`）は判断だけ。CLI は**ローカル処理**（GUI が動いていなくても外せる = tako を消す前の後片付け）。setup に全体の undo は無いので、setup が置いたものの戻し方はこの 1 本（setup の案内もこれを出す）。`tako mod install` は置く側の同じ口 | `claude_mod_install::run_action` / `plan_uninstall` |
| FR-2.42.31 | **利用者が Claude Code 側で止めたら尊重する**: `enabledPlugins["tako@skills-dir"] === false`（`/plugin` の disable）なら、mod は env の注入で読まれていても `$.settings.read()` で見て休眠し（起動時と heartbeat ごと）、最後の報告に `dormant: "user_disabled"` を載せる。tako は休眠の報告を一次ソースから外し、ペインの行を `user_disabled` にする（鮮度で消さない）。設定 dir の状態も `user_disabled`、そのペインへは注入しない。写しは最新に保つ（戻したら最新が読まれる） | `register.ts` の `userDisabled` / `goDormant`・`ModHub::accept`（`Accepted::Dormant`） |
| FR-2.42.32 | **状態を出す**: `tako mod`（MCP `tako_mod`）の `skills.targets[]` に設定 dir ごとの `state`（`installed` / `outdated` / `name_conflict` / `user_disabled` / `claude_too_old` / `claude_unknown` / `not_installed` / `no_config_dir` / `disabled` / `legacy` / `refused`）・出どころ（`default` / `account:<名>` / `env` / `reported`）・衝突の理由（`user_skills_dir` / `modified_copy` / `skills_same_name` / `installed_plugin` / `enabled_plugin`）・印の版。status は**読むだけ**（UI スレッドで書かない）。check-health への載せ方は #1891 が同じ 1 実装から | `claude_mod_install::status_json` |
| FR-2.42.33 | **検証で本物の設定 dir へ書かない**: 検証プロセス（テスト・`TAKO_ISOLATED`・セルフテスト）は一時 dir の外の設定 dir へ書かない（`refused`。どの出どころの dir でも = 報告で見えた本物の dir も）。既定の `~/.claude` も HOME が一時 dir のときだけ（そうでなければ外部エージェントの設定ホームと同じ一時の置き場へ倒す）。検証用 env は**検証プロセスだけ**で効く: `TAKO_1959_CONFIG_DIRS`（置く先の差し替え）/ `TAKO_1959_MOD_VERSION`（写しの版）/ `TAKO_1959_INJECT_CRASH=staged\|retired`（差し替えの途中で exit 86）。夜間検査（`scripts/check-claude-mod.sh`）は使い捨ての設定 dir の写しが `tako@skills-dir` として読まれることも見る | `write_guard` / `default_config_dir`・`scripts/test-mod-skills-1959.sh`・番犬 `issue1959_mod_skills_install_watchdog.rs` |
| FR-2.42.34 | **帯は他の mod の行を包む（S7-3 #1962）**: 帯（`AbovePrompt`）のフックは `next(e)` の答え（内側の他の mod が描いた行。下に誰も居なければエンジン自身の描画 `{type: "engine"}` = 空）を**必ず受け取り**、他の mod の行を上・tako の行を下に縦に並べる（エンジンだけなら tako の行だけ）。隠している・材料が無い / 古い・調査票の表示中は `next(e)` へ譲る（他の mod の行は残る）。tako の行は**ボタン込みで 1 行**（区切りの Text とボタンの横並び。帯の行数は他の mod と共有 = 全画面の 21 行のペインで 1 行）で、並びと出すものは ui.json の `band.segments`（ui.json が届かない古い tako の応答には S3 の並び）。幅（`bodyColumns`）に収まらなければ区切りもボタンも同じ物差しで優先度の低いものから落とす（tako・ペイン名は残す → 要注意 → ボタン（左ほど残す）→ ctx / 使用制限の警告 → worker 数 → タブ名の順に残りやすい）。ボタンの桁は `[ ` ラベル ` ]` + 間の 1 桁で数える。色は ui.json の `colors`（テーマのキー）。実測（2.1.294・隔離 tmux・21 行）: 80 桁で tako・ペイン名・ボタン 3 つ（79 桁）、144 / 300 桁でタブ名とボタン 4 つ、どれも 1 行で `↓ N more` に畳まれない。帯を描く別の mod と読み込み順を入れ替えても両方の行が出る（next を呼ばない mod が内側でも包む） | `register.ts` の `bandRow` / `stackBand` / `fitBand` / `segmentsOf` / `buttonsOf`。番犬 `issue1962_mod_band_buttons_watchdog.rs`（帯のフックの `return` は `next(e)` / `stackBand(` / A/B の `drawBandS3(` だけ）・`issue1881_claude_mod_band_watchdog.rs`（tako の行は横に 1 行） |
| FR-2.42.35 | **カスタムボタン**: ui.json の `buttons` を帯の tako の行に描き、ctrl+x → Tab で帯へフォーカスしてホットキーで押す（**クリックは tako が TUI へ渡すまで届かない = #1961**）。押したら語彙どおりに実行する: `slash` = `$.command.run`（`/compact` で会話が圧縮される = 実測）・`tako` = 既存の CLI（`TakoOp::cli_args`。対象のペインを `--pane` で名指し = split right / down・session-restart harness / handoff・limit-resume on / off・background・close・`file open <cwd>`）・`shell` = 新しい tako のペインで実行（`run-interactive --pane N --down --ratio 0.35 --auto-close never -- <コマンド>` = コマンドカードの実行と同じ `spawn_command_pane`）・`prompt` = `$.prompt.fill`（`insert`。送らない）。**`tako` / `shell` の引数は tako が組む**（応答の `view.button_args` = ボタンの id → argv。mod の中でシェルの文字列を組まない）。押せないものは描かない（`$.command.list()` に無い slash・cwd が分からない open-cwd・band.segments に buttons が無い）。押した結果は報告の `last_press`（`kind` / `ok` / `at` だけ。ラベル・コマンド・文は載せない） | `register.ts` の `pressButton` / `buttonsOf`・`claude_mod_ui::{TakoOp::cli_args, shell_cli_args, button_args}`。CLI が引数を受け付けることは `tako-cli` の `issue1962_ボタンの引数はcliが受け付ける` |
| FR-2.42.36 | **使用制限・ctx のバー**: 入力欄の下の行（`PromptHint`）の `tail` に `5h ▁ 4% 7d ▂ 22% ctx ▁ 6%`（`<ラベル> <棒> <N>%`。棒は 8 段 `▁▂▃▄▅▆▇█`・項目と並びは ui.json の `usage_bar.items`・値は応答の `view.ctx` と束ねた `view.rate_limits`）。`hint` は書き換えず、他の mod が足した `tail` の後ろへ ` · ` で足して必ず `next` へ流す。置き場が `band` なら tako の行の中（ctx / limits の位置、無ければボタンの前）。値が 1 つも無い（最初の応答の前・API キーで使用制限が無い等の欠け）・desktop（`tail` を描かない）では描かない。**描くかは tako が決める**（応答の `view.usage_bar` = `{draw, reason}`）: 置き場が `off` → `off`、報告の `status_line` が真（利用者の statusLine が設定されている）**かつ**画面の末尾 8 行に ctx% / `5h` / `7d` / `週` の数値が出ている → `status_line`（**mod 自身のバーの形は数えない** = 描いたバーを見て止まる往復を作らない）。statusLine が数値を出さない構成・statusLine が無い構成では描く。バーを描くときは帯の ctx / 使用制限の警告（FR-2.42.13）を外す（同じ値を 2 か所で動かさない） | `register.ts` の `barText` / PromptHint のフック・`claude_mod::{usage_bar_decision, screen_shows_usage, BAR_GLYPHS}`（`BAR_GLYPHS` は register.ts と単体テストで突き合わせる） |
| FR-2.42.37 | **報告の `renders` / `status_line` / `last_press`**（`schema: 1` に後方互換で追加。無い = 何も描いていない = S7-3 前の mod）: `renders` = `band`（直近の帯で tako の行を描いたか）・`usage_bar`（描いた置き場 `prompt_hint` / `band`。描いていなければ欠ける）・`buttons`（描いたボタンの数）・`band_hook` / `hint_hook`（起きてから帯 / 入力欄の下の行のフックが呼ばれたか）。`status_line` は `$.settings.read()` の `statusLine` の有無だけ（中身は載せない）。`tako mod` の行の `report` にそのまま出る | `claude_mod::{ModRenders, ModPress, ModReport}` / `types/index.d.ts` の `TakoRenders` / `TakoPress`（番犬が TS と Rust のキーの一致を見る） |
| FR-2.42.38 | **ステータスバーの claude の区画を止める条件**（§9.6）: **フォーカス中のペイン**の新鮮な報告（45 秒以内）の `renders.usage_bar` に置き場があるときだけ、画面下のステータスバーから claude の 5h / 7d のメーター（サービスが claude のとき。サービスの選択チップは残す = codex へ切り替えられる）と ctx のメーターを出さない。codex / agy / シェルにフォーカス・mod なし・描いていない（statusLine が出している・off・値がまだ無い）・報告が古い・A/B では今のまま（報告が途切れれば次の 2 秒の tick で戻る = 実測 45 秒）。値の取り方（FR-2.42.18）は変えない。判断は `tako mod` の `status_bar`（`drawn_by` = `mod` / `tako`・`place`・`reason` = `no_fresh_report` / `mod_not_drawing` / `legacy_env`・`pane`）と `tako limit-service --refresh` の `claude.status_bar`（MCP `tako_mod` / `tako_limit_service` の応答。引数の追加なし = MCP のカタログは増えない） | `claude_mod::{claude_bar_owner, ClaudeBarOwner}` の 1 本を `tako_control::claude_mod::status_bar_owner` が通り、GUI は `refresh_agent_metrics` でそれを控えて `status_bar.rs` が区画を外す。visual-test `mod-bar`（実ピクセル） |
| FR-2.42.39 | **帯が他の mod に隠されたことを黙らない**: 外側の他の mod が `next` を呼ばずに帯を描くと tako の帯のフックは呼ばれない（tako が env で外側なら起きないが、skills-dir で内側に読まれると起きる）。報告の `renders.hint_hook` が真で `band_hook` が偽なら、`tako mod` の行の `band.state` を `hidden_by_other_mod`（理由の文つき）にする。描けていれば `drawn` / `not_drawn`（ボタンの数・バーの置き場つき）。帯の代わりは出さない（入力欄の下の行のバーと `/tako` のサイドバーはそのまま使える） | `claude_mod::band_hidden_by_other_mod` / `tako_control::claude_mod::band_state_json` |
| FR-2.42.40 | **A/B と検証**: `TAKO_1877_S7_LEGACY=1` で応答の `view` に `band_style: "s3"` を載せ、`usage_bar` / `button_args` を載せない = mod は S3 の描き方（`next` を包まない Text 1 本・ボタンとバーなし・`renders` を送らない）へ戻り、ステータスバーも止めない（`tako mod` の `band.s7_legacy`）。検証: `claude plugin test`（`claude-mod/tests/s7.test.ts`。terminal / desktop の両 surface・内側の他の mod の代役・エンジンだけの帯・押下の 4 経路・バーの tail・A/B）、実経路 `bash scripts/test-mod-band-buttons-1962.sh`（実物の claude を一時の設定 dir と偽 Messages API `scripts/lib/fake-messages-api.mjs` で動かす = 本物の設定 dir に書かない・前後で `~/.claude*/skills`・`plugins` のハッシュを比べる。`ONLY=widths\|coexist\|buttons\|bar\|status\|ab\|visual\|static`。CI 外）。**罠**: `kill -STOP` した 2.1.294 の claude はすぐ起き直して heartbeat を送り続ける（実測）ので、報告を止める検証は mod が叩く CLI を包んで `mod report` だけを失敗させる | `claude_mod::{s7_legacy, S7_LEGACY_ENV}`（env の名前を読むのはここだけ = 番犬）・visual-test `mod-bar`（`TAKO_1877_S7_LEGACY=1` で ① が FAILED） |
| FR-2.42.41 | **コマンドカードを会話の行に描く（S7-4 #1963）**: 報告の応答の `view.cards`（このペインの tako の store のカード。コマンドごとの**論理文字列** `text`・tako が組んだ `copy_args` / `run_args`・実行記録 `run`、カードの `dismiss_args`、帯のホットキー `hotkeys`。`tako show-command --list` と同じ正本）で描く。`tako_show_command`（`mcp__<登録名>__tako_show_command` の show。action 省略を含む）を含む `ToolGroup`（「Called tako」の 1 行）は `isExpanded: true` にして `next` へ流し、その `ToolUse` の行を枠つきのカード（`tako  card N  ラベル`・コマンドを `Code` で = 見た目だけ折り返す・`→ pane M exited 0` の実行記録・帯で押す案内）に描き替える。下の mod が描いていればその下に足す（`next(e)` の答えを包む）。カードの id は呼び出しの結果（MCP の content の text = `{"card":{"id":N}}`）から拾い、store に無い（閉じた・押し出された）なら引数から描いて「閉じた」と添える。エラーで終わった呼び出し・list 等の他の action・tako が描かせない（`view.cards` が無い）ときはエンジンの行のまま。カードの番号 N は tako のカード id（`tako show-command --card N` と帯の `card N` と同じ） | `register.ts` の `ToolGroup` / `ToolUse` のフック・`drawCardRow` / `cardIdOf`・`claude_mod::{cards_view, CardsView}` |
| FR-2.42.42 | **帯の 1 行で押す**: 帯の tako の行の `card` の区切り（ui.json の `band.segments`。既定で入っている）に `card N`（複数コマンドは `card N (i/n)`）と選んでいるコマンドの 1 行目（32 桁で切る）、続けてボタン `y: copy` / `r: run` / `n: next`（選べるコマンドが 2 つ以上のとき）/ `x: close` を `plain`（ホットキーを見せる形）で並べる。**ホットキーは tako が選ぶ**（ui.json のボタンと重ならない英小文字。好みは y / r / n / x。**数字は使わない** = 空の入力欄で打った数字が帯のボタンを押すエンジンの仕様で Run が走らない）。新しいカードが届いたら選択をその 1 件目へ移し、「次」は新しいカードから古いカードへ（カードの中はコマンドの順に）巡る。押し方は ctrl+x → Tab → ホットキーで、**全画面と main screen（`CLAUDE_CODE_NO_FLICKER=0`）の両方**で押せる（会話の行のボタンはクリックでしか押せない。tako がクリックを渡す全画面の claude では会話の行のカードにもクリックで押す口を置く = FR-2.42.53。帯はキーの口として残る）。幅に収まらない（Copy / Run が落ちる・複数あるのに「次」が落ちる）ならカードは帯に描かない（中途半端に押せない形で二重に出さない）。実測（2.1.294・201 桁）: `tako \| pane 1 \| tab 1 \| card 1 (1/2) \| cargo build -p tako-cli && TAKO… y: copy r: run n: next x: close [ compact ]` | `register.ts` の `cardSelection` / `cardButtonsOf` / `cardPressable` / `fitBand`・`claude_mod::card_hotkeys` |
| FR-2.42.43 | **押下**: Copy = `$.ui.copy`（論理文字列そのまま。157 字の 1 行は改行なしで渡る = FR-2.22.2 を Claude Code の中で満たす）。他の mod の `ui.copy` フックに拒まれる等で `isCopied: false` なら `tako show-command --copy --card N --index i`（dispatch のコピー = GUI がクリップボードへ書く）へ落とす。Run = `show-command --run --card N --index i`（新しいペインで実行。実行記録はスマホ・GPUI のカードと同じ dispatch の `run` が付ける = #1724）・閉じる = `show-command --dismiss --card N`。**引数は tako が組む**（mod の中でシェルの文字列も CLI の引数も組まない）。結果は報告の `last_press`（`card_copy` / `card_copy_cli` / `card_run` / `card_dismiss` と成否・時刻だけ）。`tako_show_command` の呼び出しが返ったら mod はすぐ報告する（heartbeat の 15 秒を待たない = 応答の `view.cards` を 1 秒で取りに行く） | `register.ts` の `pressCard`・`claude_mod::card_cli_args` |
| FR-2.42.44 | **GPUI のカード帯を止める条件（§9.6）**: 報告の `renders.cards`（帯に押せる形で描いたカードの id。tako が `view.cards` を送ったときだけ載る = 送っていないのに「描いた」と言わない）に載ったカードは GPUI のカード帯へ出さない（**store には残る** = CLI / MCP / スマホは今と同じ・帯のぶん claude の viewport を縮めない）。ui.json の `cards.place`: `auto`（既定）= 描いたと言ったカードだけ外す / `mod` = mod が効いている間は全部外す（帯を隠している等で描けなくても）/ `tako` = mod に描かせず GPUI の帯のまま（S7-4 前）。**mod がまだ拾っていない新しいカード**（カードの後に届いた報告が 1 つだけ = その応答でカードを受け取ったところ。初めての報告も同じ）は `CARD_PICKUP_GRACE`（5 秒）まで外したまま（AI が自分のペインへ出したカードは実測 1〜2.5 秒で拾われる。その間に GPUI の帯を出すと viewport が一瞬縮んで戻る）。新鮮な報告が無い（mod なし・45 秒途切れた・claude が終わった = 最後の報告で即座に消える・休眠）・カードを描かない mod（`renders.cards` が無い）・拾ったのに描いていない（帯を隠している・区切りに card が無い・幅が足りない・他の mod に帯を隠された）・猶予切れ（別のペインから CLI で出したカードは mod が次の heartbeat まで知らない）→ GPUI の帯。判断は GPUI が帯を組むたびに引き、時間で変わるもの（猶予・失効・終了）は **2 秒 tick** が外す集合の変化を見て描き直す = claude の終了から 1 tick 以内で GPUI の帯へ戻る（実測: 終了の 0.2 秒後 = claude の画面の出力で描き直しが先に起きた。報告を止めたときは 45 秒）。持ち主は `tako show-command --list` / `show` の各カードの `owner`（`drawn_by` = `mod` / `tako`・`reason` = `drawn` / `pending` / `place_mod` / `legacy_env` / `place_tako` / `no_fresh_report` / `mod_not_drawing_cards` / `mod_not_drawn`。MCP `tako_show_command` の応答も同じ = 引数の追加なし。説明文に「drawn_by で案内の一言を変える」を足した）と `tako mod` の `cards`（`place` / `legacy` / `pickup_grace_ms`）。`cards.place` は報告を受けるたびに ui.json から写す（`tako mod ui set cards.place …` は次の報告 = 最大 15 秒で効く） | `claude_mod::{card_owner, CardOwner, CARD_PICKUP_GRACE, StoredReport::prev_received, ModHub::cards_place}` の 1 本を `tako_control::claude_mod::{card_owner, cards_drawn_by_mod, card_owner_json}` が通り、GUI は `command_card_ui.rs` の `command_card_rows`（帯を組むたび）と `refresh_mod_card_owners`（2 秒 tick）で引く |
| FR-2.42.45 | **A/B と検証**: `TAKO_1963_LEGACY=1`（と S3 の描き方へ戻す `TAKO_1877_S7_LEGACY=1`）で応答に `view.cards` を載せず GPUI の帯を止めない = S7-4 前（GPUI のカード帯が出て viewport が縮む・mod はカードを描かない。`tako mod` の `cards.legacy`）。検証: `claude plugin test`（`claude-mod/tests/cards.test.ts`。terminal / desktop の両 surface・Copy の拒否と CLI への退避・Run / 閉じる / 次・狭い幅・0 枚・描かせない）、visual-test `mod-card`（実ピクセル: mod が描いた = 帯 0 px・行数そのまま / 描かない mod・古い報告・claude の終了 = 帯あり / 拾う前 = 帯なし。A/B で ① が FAILED）、実経路 `bash scripts/test-mod-command-cards-1963.sh`（実物の claude を一時の設定 dir と偽 Messages API で動かし、`Show this card: {JSON}` で tako の MCP の `tako_show_command` を呼ばせる。ペインの行数を 0.1 秒おき（`tako list` 1 回ぶんを足した間隔。実測 47 回）に採って一瞬の縮みも見る。`ONLY=static\|show\|keys\|mainscreen\|stop\|place\|copy\|ab\|visual`。CI 外）。**罠**: tako の GUI のペインの claude の `$.ui.copy` は偽の pbcopy を PATH に置いても OSC 52 で tako 経由のシステムのクリップボードへ届く（実測）ので、Copy の実経路は隔離 tmux（`set-clipboard off`・クライアントなし）の claude で押し、前後で pbpaste のハッシュを比べる | `claude_mod::{cards_legacy, CARDS_LEGACY_ENV}`（env の名前を読むのはここだけ = 番犬 `issue1963_mod_command_cards_watchdog.rs`）・visual-test `mod-card` |
| FR-2.42.46 | **監視の走査を報告で置き換える（#2017。エピック #2011 の S2）**: `worker_status` / `orchestrator self` / `report` は、そのペインに**新しい報告**（受け取りから 20 秒以内 = heartbeat 15 秒 + 余裕）が**会話 ID つき**で来ていれば、会話 ID も状態も報告から採り `claude agents --json` を起こさない（応答の `agents_scan` = `{skipped: true, reason: "mod_fresh"}`。CLI / MCP は同じ JSON）。報告が無い / 20 秒より古い / 会話 ID が無い / 名指しの会話 ID（レジストリ）と食い違うときは従来どおり走査へ落ち、理由を `agents_scan.reason` に出す（`mod_absent` / `mod_stale` / `mod_aging` / `mod_no_session` / `session_mismatch` / 注入しなかった理由 / `legacy_env`）。完了・権限待ち・idle の判定は FR-2.42.12 のまま。**走査を起こすときは絞る**: 答えが要る claude（その会話 ID / そのペインの子孫）が台帳 `<config dir>/sessions/<pid>.json` に居る config dir と既定だけを起こし（会話 ID で見つからなければ全部で引き直す）、解決済みの `claude` を**ログインシェルを通さず**直接起こす（起こせなければ以後ログインシェル）。GUI のセッションカタログの走査（30 秒）も、報告が会話 ID を運ぶペインは報告から埋めて残りのペインだけを絞って走査する。`orchestrator self` の走査は先取り（background）に載せ、UI スレッドでは走査しない。A/B は `TAKO_2017_LEGACY=1`（mod の有無にかかわらず全アカウントをログインシェル経由・1 本のキャッシュで走査 = 修正前）。実測（隔離 GUI・4 アカウント × mod 入りの実 claude 4 本・監視相当の 60 秒）: 走査 44 → 0 本・回収済みの子の CPU 6.68 → 0.29 秒（判定は新旧で同じ）。詳細は `.agent/architecture.md`「`claude agents --json` の起動コスト」の #2017 節 | `claude_mod::{monitor_plan, MonitorPlan, MONITOR_SKIP_FOR}`・`orchestrator::{ScanScope, run_claude_agents_json_scoped}`・`sessions::{detected_from_mod, merge_detected}`。番犬 `issue2017_agent_scan_mod_watchdog.rs`・`issue1984_ui_thread_offload_watchdog.rs`（㉔ ㉕）。実経路 `bash scripts/test-agent-scan-mod-2017.sh`（CI 外） |
| FR-2.42.47 | **残りの走査を報告で埋める（#2041。エピック #2011）**: (1) **GUI モードのチャット表示**は、報告が新しい（20 秒以内・会話 ID つき = FR-2.42.46 と同じ `monitor_plan`）ペインの会話 ID・モデル・状態を報告から採り、`claude agents --json` を起こさない。走査は子プロセスが動いていて報告の無いペインが残るときだけで、起こすのもそのペインの子孫に claude が居る config dir と既定だけ（`ScanScope::Under`）。修正前は 2 秒 tick のたびに全アカウントを問い、鮮度 30 秒ごとにアカウント数ぶん起こしていた。(2) **`/clear`・`/resume` の後のレジストリの会話 ID**: `worker_status` の名指しがレジストリの記録そのもので、新しい報告が別の会話 ID を運んでいれば、同じ claude の会話が移ったとみなして報告の会話 ID を採り（走査しない）、レジストリへ書き戻す（応答の `agents_scan.registry_followed: true`）。以後は名指しと報告が一致して `skipped: true`。修正前は `session_mismatch` で 5 秒ごとに走査し、答える会話 ID（`resolved_session_id`）も `/clear` 前の会話のままだった（器ありはセッションカタログの 30 秒走査が書き戻すまで・器なしはずっと）。呼び出し側が記録と違う会話 ID を明示したときは従来どおり走査（FR-2.42.46）。A/B は `TAKO_2041_LEGACY=1`（チャット表示は全アカウントを走査・レジストリを追従させない = 修正前）。実測は `.agent/architecture.md`「`claude agents --json` の起動コスト」の #2041 節 | `claude_mod::{follow_session, session_legacy, SESSION_LEGACY_ENV}`・`agents::{LiveClaudeSession::from_mod, live_claude_sessions_asking, LiveAsk}`・`chat_view::{chat_live_sessions, chat_scan_ask}`。番犬 `issue2041_session_scan_mod_watchdog.rs`。実経路 `bash scripts/test-session-scan-mod-2041.sh`（CI 外） |

| FR-2.42.48 | **チャット風の描画を mod で描く（S7-6 #1964）**: mod は報告の応答の `view.chat`（`mode` / `bubble` / `code_copy` / `tool_summary` / `hotkey` / `notify`）どおりに描く。**吹き出し** = 利用者の発話（`UserMessage` の `origin` が `composer` / `bridge`。通知・他のエージェントの行は触らない）を右寄せ（左に幅の 1/5 以上を空ける）の枠つきの箱に（エンジンの行なら本文の Text で描き直し、他の mod が描いていればその描画を箱の中へ包む）。**コードのコピー** = AI の返答（`AssistantMessage`。要約の行 `isSummary` は除く）の囲みのコードブロック（``` / ~~~。閉じたものだけ = 生成中は数えない）ごとに Copy のボタンを返答の下へ足す（`コピー 1 bash` = 番号は 2 つ以上のときだけ・言語つき。**クリックで押す** = 会話の行はフォーカスできない）。key は `tako-code-<返答の id>-<順番>` で固定（描き直しで mount し直さない = Claude Code が描いた直後 300 ms のクリックを捨てる窓を作り直さない）。キーで押す口は帯の tako の行の `o: copy last code`（最新の本筋の返答 = `turn.complete` の `answer` の最後のコードブロック。返答が 1 ブロックなら `copy code`。ホットキーは tako が選ぶ = ui.json のボタンとカードのホットキーに重ならない英小文字・数字は使わない = `claude_mod::chat_hotkey`。優先度はカスタムボタンより低い = 狭い帯では先に落とす）。Copy は `$.ui.copy`（**論理文字列** = 画面の折り返しは混ざらない）。他の mod の `ui.copy` フックに拒まれたらその選択を尊重して写さず、トーストで知らせる（カードと違い tako の store に無い文字列なので CLI へ落とさない）。**ツールの要約** = `ToolGroup` を `isExpanded: true` で `next` へ流し、`ToolUse` を 1 行（名前・何に対してか = Bash の command / Read 等の file_path / Grep の pattern / MCP は `<登録名>.<名前>` 等・状態 = 完了 / 実行中 / エラー / 中断・「詳細」のボタン）に描く。「詳細」で開いた呼び出しは要約の下にエンジンの行（出力つき）を戻す（`$.state` の `toolOpen`。64 件まで）。`tako_show_command` の行はカード（FR-2.42.41）のまま。**どれも `next(e)` の答えを包む**（他の mod の行は残す。外側 / 内側のどちらに読まれても両方の描画が残ることを実物で確かめた）。今あてている組は報告の `renders.chat`（`bubble` / `code_copy` / `tool_summary` / `notify` = tako が頼んだものそのまま。応答に `view.chat` が無ければ欠ける・tako が 45 秒応答しなければ描くのをやめて引っ込める）、押した結果は `last_press` の `code_copy`（種類と成否だけ） | `register.ts` の `UserMessage` / `AssistantMessage` / `ToolGroup` / `ToolUse` のフック・`drawBubble` / `drawCodeCopy` / `drawToolSummary` / `codeBlocks` / `copyCode`・`claude_mod::{ChatView, ModChatRenders, chat_hotkey}` |
| FR-2.42.49 | **何を描くかは tako が決める（`view.chat`）**: `tako_core::claude_mod::chat_view` の 1 本（ui.json の `chat` × 表示モード）。`chat.place = tako` → `mode: off`（何も描かない = #1964 前の見た目）・**ターミナル表示 → `terminal` の組 = ui.json の `always` だけを描く（既定ではコードのコピーだけ。ターミナル表示の利用者の見た目を勝手に変えない）**・GUI モードで mod がチャットを持つ → `gui` の組（`gui_only` も描く）。`chat.place = auto` の GUI モードでの持ち主は**判断点**（#1964 の受け入れ 4）が決まるまで tako（`AUTO_CHAT_OWNER_GUI` = GPUI のチャットのまま・mod は `terminal` の組）。決まったらこの定数 1 行を変える（ui.json の値は `auto` のままなので書き換え・移行は要らない）。mod の描画は `tako mod ui set chat.place mod` で選べる。A/B `TAKO_1964_LEGACY=1`（と `TAKO_1877_S7_LEGACY=1`）は応答に `view.chat` を載せない = mod はチャットを描かず、GUI モードのチャットは mod を見ない（#1964 前） | `claude_mod::{chat_view, resolve_chat_place, AUTO_CHAT_OWNER_GUI, chat_legacy, CHAT_LEGACY_ENV}`（env の名前を読むのはここだけ = 番犬 `issue1964_mod_chat_watchdog.rs`） |
| FR-2.42.50 | **GUI モードでチャットを mod に任せる（§9.6）**: `claude_mod::chat_owner` の 1 本 =「A/B でない ∧ GUI モード ∧ 置き場が mod（`auto` は上の判断点）∧ 新鮮な報告（45 秒）∧ `renders.chat` がある」なら、GPUI はそのペインをチャットビューで覆わずターミナル表示（= mod が描いた本物の Claude Code。入力欄のミラー・生成中やキューの画面の推測が要らない）にする（`ui_mode::PaneDisplayInput::mod_chat`。チャット確定・過渡期の準備中・alt screen より先に見る。理由は `mod_chat`・次の一手は `tako mod ui set chat.place tako`）。表示モードを変えた直後に mod があてている組が `terminal` のまま（次の報告 = 最大 15 秒まで）でもターミナル表示にする（表示を 2 回入れ替えない）。**戻す**: 時間で変わるもの（45 秒の失効・claude の終了 = 最後の報告で即座に消える・置き場の写し）は 2 秒 tick の `refresh_mod_chat_owners` が持ち主の集合の指紋を取り、変わったら描き直す = 1 tick 以内に GPUI のチャットへ戻る（GPUI のチャットの読み取りは mod が持っている間も続けるので、戻った瞬間から会話が出る）。過渡期（#720）は mod がチャットを持てば着いた扱い。外から読む口: `tako mod` の `chat`（`place` / `auto_in_gui` / `ui_mode` / `legacy`）と各ペインの `chat`（`drawn_by` = `mod` / `tako`・`reason` = `drawn` / `legacy_env` / `mode_terminal` / `place_tako` / `place_auto` / `no_fresh_report` / `mod_not_drawing_chat`）・`tako ui-mode` の `pane_display_reason[N].materials.mod_chat` と `reason: mod_chat`（MCP `tako_mod` / `tako_ui_mode` も同じ JSON = ツール・引数の追加なし） | `claude_mod::{chat_owner, ChatOwner}`・`tako_control::claude_mod::{chat_owner, chat_owner_json}`・`ui_mode::{PaneDisplayInput::mod_chat, TerminalReason::ModChat}`・GUI は `pane_display_input` と `refresh_mod_chat_owners`（2 秒 tick） |
| FR-2.42.51 | **フォールバック側の改善（#1883 の置き換え）**: (1) **GPUI のチャットを mod の通知で読み直す**: mod は本筋（サブエージェントでない）の user / assistant の行が足されるたびに `chat_seq` を数え（`session.append` の `next` の後 = transcript に書かれた後。**数だけ**で本文・uuid は載せない）、`view.chat.notify`（GUI モードで GPUI のチャットが表に出ているペイン）なら dirty を立てて 1 秒以内に報告する（ターミナル表示・mod がチャットを持つペインでは報告を増やさず、数は他の報告に相乗り）。GPUI のチャットは新しい報告（20 秒以内）の `renders.chat.notify` が真のときだけ `chat_seq` を合図にし、変わったときだけ transcript を読み直す（変わっていなければ stat もしない = transcript をポーリングしない。変わった後の 2 tick だけは書き込みの遅れを拾うため mtime も見る）。報告が無い・途切れかけ・通知していない mod は今の mtime とサイズの判断（`claude_mod::chat_reread`）。(2) **未登録の設定 dir**: transcript を報告の `config_dir`（`CLAUDE_CONFIG_DIR`）から先に探す（`transcript::find_transcript_hinted`）= accounts.yaml にも GUI の env にも無い設定 dir の claude でも、mod の報告が会話 ID（#2041）と設定 dir を運ぶので GPUI のチャットに会話が出る。A/B `TAKO_1964_LEGACY=1` は両方とも使わない | `claude_mod::{chat_reread, ChatReread}`・`chat_view::{chat_seq_gate, CHAT_SEQ_STAMP_CHECKS}`・`transcript::find_transcript_hinted` |
| FR-2.42.52 | **判断点と検証**: 受け入れ 4（GUI モードの既定を GPUI のチャット / mod の描画のどちらにするか）は**ユーザーの判断**。材料は同じ会話を両方で撮った画像（visual-test `mod-chat` の `mod-chat-gpui.png` / `mod-chat-mod.png`）。判断が出るまで既定は GPUI のチャット（FR-2.42.49）。検証: `claude plugin test`（`claude-mod/tests/chat.test.ts`。terminal / desktop の両 surface・他の mod の描画を包む・Copy の論理文字列と拒否・帯のキー・狭い幅・`chat_seq` と通知）、visual-test `mod-chat`（実物の claude の同じ会話を GPUI のチャット → mod の描画で撮り、Copy をクリック・帯をキーで押す。A/B で ② が FAILED）、実経路 `bash scripts/test-mod-chat-1964.sh`（`ONLY=static\|visual\|term\|gui\|ab\|tmux`。一時の設定 dir と偽 Messages API。CI 外）。**罠**: GUI のペインの claude の `$.ui.copy` は OSC 52 で tako 経由のシステムのクリップボードへ届くので、GUI で Copy を押す検証は ui.copy を拒む mod を同居させ、写った中身は隔離 tmux（`set-clipboard off`・クライアントなし・偽の pbcopy）の claude で見る。Claude Code 2.1.294 の全画面は Bash の呼び出しも `ToolGroup`（「Ran 1 shell command」）に畳むので、ツールの要約はグループを展開して描く。**既知の限り**: 報告が途切れたペインの会話は `claude agents --json` の走査で引くので、未登録の設定 dir の claude は mod が止まると GPUI のチャットにならずターミナル表示へ落ちる（mod が無ければ未登録の設定 dir は見えない = #1964 前と同じ）。実測（2.1.294・偽 Messages API・隔離 GUI）: ターミナル表示の既定で `renders.chat` はコードのコピーだけ・吹き出し / 要約なし。GUI モード（auto）で未登録の設定 dir の会話を GPUI のチャットが読み（5 件）、新しい発話は送ってから 2.0〜2.2 秒で映った（そのターンの報告 1 回）。`chat.place mod` から 11〜12 秒（次の報告）でターミナル表示（mod_chat）になり吹き出し / Copy / 要約が出た。報告を止めると最後の報告から 45.2 秒（止めて 44.1 秒）、claude を終えると即座（0.0 秒）に GPUI のチャットへ戻った。visual-test は ①〜④ OK・A/B は ② で FAILED | `claude-mod/tests/chat.test.ts`・visual-test `mod-chat`・`scripts/test-mod-chat-1964.sh`・番犬 `issue1964_mod_chat_watchdog.rs` |
| FR-2.42.53 | **会話の行のカードをクリック 1 回で実行する（#2159）**: tako がそのペインの TUI へ左クリックを渡す（`tako mouse` が on ∧ #1961 / #2159 の A/B でない = `tako_control::claude_mod::card_click`）なら、応答の `view.cards.click` を真にし、mod は**全画面の端末**（`viewport.isFullscreen === true`。main screen はマウス報告を要求しないのでクリックが届かない）の会話の行のカードに、コマンドごとに `[ run ]`（`variant: primary`。ja は `[ 実行 ]`）と `[ copy ]` を置く（実行記録はボタンの横・帯で押す案内は「キーでは帯（ctrl+x → Tab）の y / r」へ）。押した後は帯のボタンと**同じ `pressCard`** = Run は tako が組んだ `run_args`（`show-command --run --card N --index i`）で、新しいペイン（手元のペインの下 35%・タイトル `(>) …`・フォーカスは手元のまま）と実行記録が GPUI のカードの「実行」・スマホと同じ dispatch の `run` で付く。key はカード id と番号で固定（`tako-card-<id>-<i>-run`。描き直しで mount し直さない = Claude Code が描いた直後 300 ms のクリックを捨てる窓を作り直さない）。**二重実行の防止**: mod は同じコマンドの Run を、押した CLI が返るまでと返ってから `RUN_REPEAT_MS`（1 秒 = ダブルクリックの 2 打目）の間は捨てる（帯の `r` とも同じ番）。その後の押し直しは tako の dispatch が判断する（実行中なら `このコマンドはまだ実行中` で断り、終わっていれば再実行 = GPUI のカードと同じ）。断られたら理由をトーストで出し、番を外してすぐ押し直せる。**押せないボタンを見せない**: `view.cards.click` が偽（`tako mouse off`・A/B）・main screen・全画面か分からない・desktop 等の端末以外・閉じたカードはボタンを置かず、#1963 の帯で押す案内のまま。帯のホットキーはどの場合も今どおり描く（キーの口）。外から読む口は `tako mod` の `cards.click` / `cards.click_legacy`（MCP `tako_mod` も同じ JSON = ツール・引数の追加なし）。A/B `TAKO_2159_LEGACY=1` は `view.cards.click` を常に偽にする（#1963 の形）。`cards.place = tako`・mod なしのペインは GPUI のカード帯のまま（変更なし）。検証: `claude plugin test`（`cards.test.ts` の #2159 の 5 本: ボタンと案内・Copy の論理文字列と拒否・同時 / 直後 / 帯の 2 打目・断られたときのトースト・クリックが届かない 5 通り）、visual-test `mod-card-click`（実物の claude の `[ run ]` を実 OS と同じ PlatformInput のクリック 1 回で押す・150 ms 後の 2 打目と実行中の押し直しで 2 本目が走らない・A/B で ① が FAILED）、実経路 `bash scripts/test-mod-card-click-2159.sh`（`ONLY=static\|steps\|mouseoff\|place\|copy\|visual`。手順の A/B = 新はクリック 1 回 / 旧は帯の ctrl+x → Tab → r の 3 打。Copy は ui.copy を拒む mod を同居させ退避先の CLI を包みで控える = 本物のクリップボードへ書かない。CI 外）。**罠**: 隔離 tmux の claude では MCP は繋がるのに偽 API へ渡るツール一覧に `tako_show_command` が載らず、AI の呼び出しの行（ToolUse）を作れなかった（Copy のクリックは GUI のペインで確かめる） | `claude_mod::{CardsView::click, card_click_legacy, CARD_CLICK_LEGACY_ENV}`（env の名前を読むのはここだけ）・`tako_control::claude_mod::card_click`・`register.ts` の `drawCardRow` / `pressCard` / `runBusy`。番犬 `issue2159_mod_card_click_watchdog.rs` |
| FR-2.42.54 | **`tako check-health` にも tako mod の項目を出し、報告の無い理由を 1 つに決める（#1891。設計書 §8）**: `tako check-health`（MCP `tako_check_health`）の `claude_mod` 節に、`tako mod --json` と**同じキー・同じ値**（`enabled` / `injecting` / `reason` / `claude_version` / `min_claude_version` / `summary` / `no_report_probe` / `skills` / `panes[]`。行は `pane` / `title` / `state` / `injected` / `claude_on_screen` / `reason` だけで、報告の中身 = ctx・使用制限・帯は載せない）を載せる。**判定は持たない**: `tako mod`（status）と同じ 2 段（UI スレッドの `claude_mod::status_ctx` → 背景の `finish_status_in_background`）の応答から `health_section` で写すだけ。人が読む行（1 行目・設定 dir ごとの skills/tako・報告が届いていないペインの理由）は CLI の両コマンドが `claude_mod::text` の同じ関数で出す = 字面が一致する。**報告の無いペインの理由**（`reason.cause`。`reason.code` は S2 の語彙 `mod_absent` のまま）は背景の段がペインの claude（`resources::kind_of` の 1 実装で見つける）のコマンド行と**起動時の env**（`ports::process_env_vars`）を調べ、`tako_core::claude_mod::classify_no_report` の 1 本で決める: `safe_mode` / `bare`（フラグは `--` より前の語だけ）→ `before_injection`（env にこの tako の展開先が無い = 注入より前から動いていた claude。新しいペインで起動すれば載る）→ `not_loaded`（注入は届いている = 設定・組織のポリシー・読み込みの失敗）→ `undetermined`（調べられない = S1 の候補を並べた文）。調べるのは**上限 8 ペイン**（`no_report_probe` の `panes` / `limit` / `over_limit`）・調べるペインが無ければ走査もしない・**macOS だけ**（起動時の env を読めるのが macOS だけ。ほかの OS は `undetermined`）。`tako mod`（status）も同じ理由で `OffloadJob::ModStatus` として背景で仕上げる（`on` / `off` / `band-*` の応答と同期の腕は調べない = `undetermined`）。`issues` には直す手がある「注入しない」（`not_installed` / `no_cli` / `claude_too_old`）を `check=claude_mod`、報告の無いペインを `check=claude_mod_no_report`（いずれも `info`）で出す（`tako mod off`・claude 未検出は利用者の選択 / 環境なので出さない）。**GUI に届かない・mod を扱わない tako では `available: false` と `reason.message`（理由つきの不明）で、ほかの項目はそのまま出る**。A/B は `TAKO_1891_LEGACY=1`（調べない・節を載せない）。実経路テスト `bash scripts/test-check-health-mod-1891.sh`・番犬 `issue1891_check_health_mod_watchdog.rs` | `claude_mod::status_ctx` / `finish_status_in_background` / `health_section` |
対応状況: claude は Supported（`agent_support` の `claude_mod_state` / `claude_mod_band` / `claude_mod_buttons` / `claude_mod_cards` / `claude_mod_chat` / `claude_mod_ui` / `claude_mod_install`。ui.json のうちボタン・バー・帯の区切り・色の描画は #1962、カード（`cards.place`）は #1963 で効く。チャット風の描画（`claude_mod_chat`。`chat.place`・`chat.bubble` / `code_copy` / `tool_summary`）は #1964、codex / agy は同等の拡張点を未調査で Pending（#1885）。
Windows は Pending（#1886。区切り `;` の連結は単体テスト済み、実機の Claude Code で読むかは未実測）。

### FR-2.43 PC リソース管理（エピック #1992。S1 = 計測の土台と読み取り口 ✅ #1993 / S2 = 順番待ち ✅ #1994 / agent の起動待ち ✅ #2413）

> あちこちのペインで同時に走るビルド・テスト・隔離 GUI が PC を落とす（9/25 swap 6.8 / 8 GB・load 86 で
> WindowServer ごと落ちた / 10/10 load 242・swap 8.4 / 9.2 GB で再起動）。PC 全体と「どのペイン / worker /
> tako の外が何をしていて、どれだけ食っているか」を見せ、優先度つきの順番待ちで重い処理を回す。
> 設計の正本は `.agent/plans/2026-10-resource-manager.md`（判断点 §11 は 10/12 にすべて推奨どおりで確定）。

| ID | 要件 | 優先度 | 状況 |
|---|---|---|---|
| FR-2.43.1 | **PC 全体の計測**: メモリの内訳（Activity Monitor と同じ式 = 使用は app + wired + 圧縮、空きは全体 − 使用）・swap・メモリ圧（normal / warn / critical）・CPU%（全コアの差分）・load・論理 CPU 数。OS への問い合わせは**境界 B29**（`tako_core::platform::resources`。sysctl / mach / libproc だけで**子プロセスを起こさない**）、解釈は純関数（`tako_core::resources`）。Windows は `None`（対応マトリクス `tako_queue` = Pending、#2000） | M | ✅ #1993 |
| FR-2.43.2 | **プロセス単位の計測と帰属**: 全プロセスの親・名前・物理フットプリント・CPU 時間（**mach tick を timebase で換算**。Apple Silicon は 125/3）・開始時刻（pid の再利用は開始時刻で見分ける）。argv と env（`TAKO_PANE_ID` / `TAKO_SOCKET` / `TAKO_ISOLATED` / 順番待ちの `TAKO_QUEUE_JOB` = #1994）は**新しいプロセスだけ**読む。帰属は ①ペインの根（器の無いペインは PTY 直下の子、器のペインは `pane_pid`）の子孫 → ②env の `TAKO_PANE_ID`（`TAKO_SOCKET` が自分の受け口と一致するときだけ = 隔離の tako のペイン番号と取り違えない）→ ③tako の外（launchd 直下の祖先の名前を添える）。分類は agent（claude は実行ファイル名が版数なので argv で見る）/ ビルド / ブラウザ / 隔離 GUI。**agent の補助プロセス**（祖先を辿ってシェルより先に agent に当たる = MCP サーバーとその子）は重い処理に数えず agent の常駐へ入れる。重い処理 1 件 = 重い祖先を持たない重いプロセスとその木 | M | ✅ #1993 |
| FR-2.43.3 | **採取は背景スレッド**（`tako_control::resources::start`）: 機全体 2 秒・プロセスの走査は誰も見ていなければ 30 秒（status を受けてから 10 秒は 2 秒。設計書 §2.2 の 5 秒は隔離 GUI（debug・器あり・90 秒のアイドル × 2 回）の A/B で 1 コアの +0.57% と見込みを超えたので 30 秒へ落とした。30 秒では採取あり 0.71 / 0.99% 対 なし 0.85 / 0.90% = 差がばらつきの中。見ている間の 2 秒ごとは走査 1 回 22〜24 ms（debug）ぶん増える）。UI スレッドは 2 秒 tick でペインの一覧を写して渡す（`publish_resource_panes`）と覚えを読む（`cached_status`）だけ。番犬 `crates/tako-control/tests/issue1993_resources_ui_thread_watchdog.rs`（+ #1968 / #1984 の番犬の禁止リスト）。A/B（採取の費用の比較）は `TAKO_1993_NO_SAMPLER=1` | M | ✅ #1993 |
| FR-2.43.4 | **読み取りの口**: CLI `tako queue [--json]` / MCP `tako_queue`（`action: status`）/ dispatch `Request::Queue`。**`ipc::submit` が UI の受け口へ渡す前に答える**（覚えが 2 秒より古ければその接続のスレッドで採り直す = UI が固まっていても答えられる）。応答は `machine`（`memory_mb` / `swap_mb` / `pressure` / `cpu_percent` / `load` / `cpus`）・`heavy`（誰 / 何 / メモリ / CPU% / 本数）・`panes`（メモリの多い順に上位 10。`agent_mb` は agent + MCP サーバー）・`external`・`scan`・`queue`（#1994: 実行中・待ち・上限・先頭が待つ理由・tako の外の件数・直近に終わった件）。表示用のコマンド行は第 1 語とパスを basename に畳み、鍵・トークンらしい値は伏せる | M | ✅ #1993 |
| FR-2.43.5 | **GUI が居ないときも機の状態は答える**: CLI は自分で機の値を採り（CPU% は 250 ms 空けて 2 回）、「順番待ちは無効（tako が起動していない）」を添える。**繋がったまま 30 秒応答しない GUI**（#2123）でも同じく機の値を採って返し、「順番待ちの状況は読めない（tako が 30 秒応答しない …）」を添える（`gui: false`）。上限の書き込み（`tako queue limits`）は GUI が居なくても応答しなくても設定ファイルへ書く | S | ✅ #1993 / #2123 |
| FR-2.43.6 | **順番待ちで走らせる**: `tako queue <コマンド>`（wrapper）が受付 → 1 秒ごとの poll（心拍）→ 割り当て → 子を起こす（env `TAKO_QUEUE_JOB=<受付番号>`、`CARGO_BUILD_JOBS` / `RUST_TEST_THREADS` が未設定なら取り分 = max(1, 論理 CPU ÷ 実行中の数)）→ 2 秒ごとの心拍 → done。**終了コードは子のまま**（シグナルで終わったら 128 + 番号）・Ctrl+C / SIGTERM は子へ転送・**入れ子は待たない**（`TAKO_QUEUE_JOB` がある中は即実行）・待ちの表示は stderr（TTY は 1 行の書き換え、非 TTY は 60 秒ごと。即座に入れたときは何も出さない）・待ちの上限（既定 30 分・`--max-wait` / env `TAKO_QUEUE_MAX_WAIT`）で 75（**待っていた理由と、枠とメモリを押さえていた処理と量**（使用・最大・まだ使っていない見込み・見込みとその出どころ）を stderr に出し、調停役の `recent` に `gave_up` / `reason` / `holders` で残す = CLI `tako queue` の「直近の諦め」・MCP `tako_queue` の `queue.recent`。#2254）・待ちの取り消しで 125・起動できない子は 127。受付番号 = wrapper の pid。**tako が居ない / 調停役でない tako（隔離・旧版）に当たったら待たずに走る**（1 行だけ知らせる = fail-open）。**繋がったまま 1 往復の上限（30 秒）まで応答しない調停役**（GUI が固まった。#2123 = FR-2.2.12）も受付なら待たずに走る（夜間リリースの `run_heavy` が止まらない）。**例外: 順番待ちで走らせる操作は CLI だけ**（待ち行列に入るのは呼び手のシェルの子プロセスで、MCP の呼び出しには包む対象が無い。UI にも「走らせる」操作は無いので「UI でできることは AI からも」に反しない。状況・取り消し・上限は CLI / MCP `tako_queue` の `action: status / cancel / limits` で 1:1） | M | ✅ #1994 |
| FR-2.43.7 | 右パネルの `resources` ビューとステータスバーの 1 区画 / 優先度と終わりの見込み / AI の運用（spawn の応答の `machine` / `advice`・fleet の常駐メモリ。**spawn を止めない判断は #2413 = FR-2.43.12 で改めた**）/ cargo の共有 jobserver / スマホ / Windows | M | #1995〜#2000 |
| FR-2.43.8 | **調停役**（`tako_control::queue`。プロセスに 1 つ）: **本番の GUI の 1 インスタンスだけ**が調停する（隔離 `TAKO_ISOLATED=1`・セルフテスト・セカンダリは調停しない = 隔離ごとに別の待ち行列ができると全体で絞れない）。隔離の調停役は `TAKO_QUEUE_SOCKET` を**明示したときだけ**立てる（GUI 無しの `tako queue serve` / その env を付けた GUI。wrapper もこれがあればそこだけへ繋ぐ）。受け答えは **`ipc::submit` が UI の受け口へ渡す前**（UI スレッドが固まっても割り当ては進む）。**待ち行列の正本は wrapper**: 毎回の poll が 1 件の全体を送る upsert なので、調停役を落として立て直しても待ちは元の受付時刻・実行中は子の pid で組み直る（ディスクに持たない。立ち上げ直後 6 秒は誰も入れない = 後から戻る古い待ちに追い越されない）。wrapper が消えたら（pid が居ない / 心拍 20 秒途絶）枠を返す。待ちの途中で見失った wrapper は 1 → 5 秒で繋ぎ直し、60 秒で待たずに走る（繋がったまま応答しない = 1 往復が上限の 30 秒で切れた場合も同じ扱いで、60 秒は失敗した往復を始めた時刻から数える。#2123）。待ちは 1 秒ごとの短い往復の連続なので、**応答が届いている限り長い待ち（夜間の上限 3 時間）は切れない**。実行中は止めない（心拍と done は 5 秒で切るので、固まった調停役でも子の終わりから 10 秒の内に終わる）。**停滞**（木の CPU が 10 分続けて 1% 未満）は枠を返すだけで殺さない。**取り消し**は印だけで、止めるのは wrapper（子とその子孫へ SIGTERM → 10 秒で SIGKILL）。番犬 `issue1993_resources_ui_thread_watchdog`（受け口の順序・UI から届く口は採らない）・`issue1994_queue_ui_stall`（UI の受け口を誰も読まない注入で割り当てが進む） | M | ✅ #1994 |
| FR-2.43.9 | **割り当ての条件**（純関数 `tako_core::queue::admit`）: 並びは優先度 → 受付の古い順。並びの先頭から 1 件ずつ ①同じ target dir の処理が走っている（停滞も含む）→ 待つ ②メモリ圧 critical → 待つ ③**管理下の実行中（停滞を除く）が 0 件なら入れる**（最低限の前進）④メモリ圧 warn / swap が警戒線以上 → 待つ（= 1 件ずつ）⑤実行中 + tako の外の重い処理（順番待ちを通っていないもの。1 件 = 1 枠）≥ 同時数 → 待つ ⑥空き − 走り出したばかりの処理がまだ使っていない見込み − 見込み < 予約 → 待つ。機の値が採れなければ同時数だけ。**追い越し（#2243）**: ②〜⑥（枠・メモリ）で入れない件で止まり後ろも待つが、①（target dir）だけで入れない件は後ろに追い越させる（その dir が空くまでどのみち走れない。以前は別の target dir の後ろまで空き枠の前で待った）。追い越す側は自分で①〜⑥を通り、追い越された件と同じ target dir の後ろの件は追い越さない（同じ dir の中の順番を保つ）。1 件の待ちを追い越せるのは累計 同時数 × 2 件まで（達したらその件で止まる）。dir が空いた時点で先頭は入るか②〜⑥で止まるので、空いた後に先頭より先に入る処理は無い（飢えない）。待ちの理由は件ごと（status の `waiting[].reason` / `text` / `overtaken`、wrapper の 1 行は自分の理由 / 並びが止まった件だけ「前に N 件（先頭: …）」）。A/B は調停役の env `TAKO_2243_LEGACY=1`・実経路 `bash scripts/test-queue-head-of-line-2243.sh`・番犬 `issue2243_queue_head_of_line_watchdog`。**見込みは過去の記録から（#2254）**: 調停役が終わった処理ごとに木の最大と最大になった時点を `<data_dir>/resources/history.jsonl` へ 1 行足し（鍵は「repo の根 + 正規化した argv」のハッシュ・git の worktree は本体の根へ寄せる・表示名はプログラム + サブコマンド = **生の argv とパスは書かない**・512 KB で `history.1.jsonl` へ回す・共有しない Local）、受付で同じコマンドの直近 5 回の最大 → 無ければ同じ形（cargo の `--test` の名前・テストの絞り込み・パスを伏せ、`-p` は伏せない）の直近 10 回の最大（2 回以上）→ それも無ければ区分の既定（build 3.5 GB・test 7 GB・browser 2 GB・その他 1 GB。build / test は tako 自身の `cargo build --workspace` / `cargo test --workspace` の木の最大メモリの実測 = 設計書 §4.7）。128 MB 刻みへ切り上げ・下限 256 MB。**⑥の「まだ使っていない見込み」は走り出してからピークまで（#2254）**: ピークを過ぎた実行中（見込みに届いた、または過去の記録の山の時刻と自分の最大の更新からどちらも 60 秒過ぎた）は 0（以前は「見込み − 今の使用量」を最後まで押さえ、ピーク後に数百 MB の `cargo test --workspace` が 6.9 GB を抱えたまま後ろを 30 分止めた）。メモリ圧・swap・予約・空きの実測の歯止めは変えない。status の `running[]` に `estimate_mb` / `estimate_from`（default / history / shape）/ `pending_mb` / `peak_passed`、`waiting[]` に `estimate_mb` / `estimate_from`、空きメモリ不足の理由に必要の内訳（実行中のまだ使っていない見込み + この処理の見込み）。A/B は調停役の env `TAKO_2254_LEGACY=1`（区分の既定だけ・最後まで押さえる）・実経路 `bash scripts/test-queue-reservation-2254.sh`（注入 `TAKO_QUEUE_INJECT` の `jobs` で実行中の木の使用量・最大を与える）・番犬 `issue2254_queue_reservation_watchdog`。上限は `settings.json` の `resources` 節（`max_jobs` / `reserve_gb` / `swap_guard_gb` / `max_wait_min`。**キーが無ければ機に合わせた既定** = 同時数 RAM 12 GB あたり 1（1〜4）・予約 max(2 GB, RAM の 10%)・swap 2 GB・30 分。旧ファイルは serde default でそのまま読める）。CLI `tako queue limits [--max-jobs N] [--reserve-gb X] [--swap-guard-gb Y] [--max-wait-min M] [--reset]`（範囲外は弾く。GUI が居なくても設定ファイルへ書ける）/ MCP `tako_queue` の `action: limits` | M | ✅ #1994 |
| FR-2.43.10 | **この repo の入口で採用**: AGENTS.md のコマンド表（build / test / lint）は `tako queue cargo …`。`scripts/lib/queue.sh` の `run_heavy` を build-app / isolated-gui のビルド / cargo を呼ぶ検証スクリプトが通し、夜間リリースは env `TAKO_QUEUE_PRIORITY=low` / `TAKO_QUEUE_MAX_WAIT=3h` で中のビルドを low で並べる。**tako が無い（CI）・順番待ちを知らない古い tako・入れ子はそのまま走る** | S | ✅ #1994 |
| FR-2.43.11 | **worker は tako の MCP だけで起動する**（#2412。10/25 のメモリ枯渇の主因 = claude 1 本ごとに user scope の MCP が 12〜15 プロセス・約 400 MB）: tako が起こす claude の worker（spawn / run / git resolve / 会話を保った再起動 / 再起動をまたぐ復元）は `--settings <data_dir>/worker-mcp/deny-<hash>.json` の `deniedMcpServers` で user scope の MCP を止める。止める名前 = 知っている config dir 全部（起動するアカウント・tako の既定・accounts.yaml）の user scope の和 − tako のブリッジ − プロジェクトの `mcp_servers` − `.mcp.json`（cwd と祖先）/ local scope と同名のもの（deny は名前で効くのでプロジェクト側まで止まる = 実測）。`.mcp.json` / local scope / plugin / claude.ai のコネクタは今までどおり立つ。**足すのはプロジェクト単位**: projects.yaml の `mcp_servers`（`*` = 全部持ち込む）を CLI `tako orchestrator projects mcp <key> [add\|remove <名前>...]` / MCP `tako_orchestrator_projects` の `action: mcp / mcp_add / mcp_remove` + `servers` で 1:1。spawn の応答は `mcp`（mode・denied・kept・missing・shadowed）、効かないとき（プロファイルの args の `--settings` が後勝ちする / 設定ファイルを書けない）は `warnings`（`kind: worker_mcp`）と persist.log。**移行（#916）**: projects.yaml v1 → v2 で直近 30 日の会話で呼ばれた user scope の `mcp__<server>__*` をそのプロジェクトの `mcp_servers` へ写す（名前だけ。本文は残さない）。codex / agy は能力マトリクス `worker_mcp_scope`（codex = 対応 #2418 / agy = 起動ごとの手段が無い）。**再開（#2447）**: 再起動をまたいだ復元（`codex resume <id>` / `claude --resume <id>`）とカタログの再開も spawn と同じ判断（`worker_mcp::prepare_for_resume`）・同じ断片（`agent::worker_mcp_args`）を載せる（読む設定は会話の記録がある場所。A/B は `TAKO_2447_LEGACY=1`・番犬 `issue2447_resume_worker_mcp_watchdog`。突然死の復旧案内 `resume_command` は #2470 で `tako orchestrator resume <worker_id>` = 打った時点で同じ判断を通す。FR-2.38.3）。対象外: master / solo・人が手で開く claude。A/B は `TAKO_2412_LEGACY=1`・番犬 `issue2412_worker_mcp_scope_watchdog` | M | ✅ #2412 |
| FR-2.43.12 | **agent の起動待ち**（#2413。2026-10-26 のユーザー判断で設計書 §11-7 の「spawn は助言だけ」を改めた。設計書 §9.6）: メモリが逼迫しているあいだ（メモリ圧 warn / critical・swap が警戒線以上・空き − 立ち上がり中の agent のまだ使っていない見込み − 見込み < 予約）は agent を起こす入口を**待たせる**。判断は重い処理と**同じ `admit`**（`tako_core::queue::admit_launches` = 最低限の前進なし・同時数の上限なし・実行中 = 立ち上がり中の agent。予約と swap の警戒線は `resources` 節の同じ設定）。待たせる入口は `tako_orchestrator_spawn`（`run` も）・GUI の起動時の会話の復元・`tako sessions resume`・`tako git resolve` の CLI / MCP（人の GUI 操作は明示とみなして待たせない）。待たせない入口とその理由（一括の自動復帰・再起動・引き継ぎ・人が直接起こす入口 等）は設計書 §9.6 の表。spawn はペイン・レジストリ・台帳を今作り、起動コマンドと依頼文を `tako_control::launch_gate` へ預けて**すぐ返る**（MCP の呼び出しを固めない）。応答の `launch_gate`（`state` waiting / starting・`code` / `reason`・`position` / `waiting`・`waited_s`・`start_now`）、worker の状態は `launch_waiting`（素のシェルを完了と読まない・未達 / 突然死 / 起動失敗 / 停滞の事象を出さない）、`workers` の `launch` は `waiting` + `launch_gate`、`run_status` は `launch_waiting`（**時間切れの時計は起きてから数える**。起こした後は `starting` = #2426）、`tako queue` の `launches` 節（待ち・立ち上がり中）。圧が戻ると GUI の 500 ms tick が待たずに起こすときと同じフローで起こし、worker レジストリの `spawned_at` を起こした時刻へ直す（送達の猶予を待ちに使わない）。**再起動後の復元は時差**: 復元の列で 1 本ずつ・起こしてから立ち上がり（20 秒か見込みに届くまで）を終えたら次・master / solo が先・spawn は列の待ちを追い越せる。agent 1 本の見込みは立ち上がりの最大の過去の記録（`history.jsonl` の区分 `agent`・系統ごと。無ければ 640 MB）。**明示の上書き**: CLI `tako queue start <ペイン>` / MCP `tako_queue` の `action: start`。**走っている agent は止めない・殺さない**。有効なのは本番の GUI とセカンダリ（隔離 / セルフテストは注入 `TAKO_2413_INJECT` があるときだけ）。Windows（#2000 まで機の値が無い）は時差だけ。A/B `TAKO_2413_LEGACY=1`・実経路 `bash scripts/test-launch-gate-2413.sh`・番犬 `issue1993_resources_ui_thread_watchdog`（⑨⑩） | M | ✅ #2413 |

### FR-2.44 usage 分析（エピック #2192。S1 = 利用制限の観測とログインの鍵 ✅ #2200 / S2 = 使用量の索引と内訳 ✅ #2201 / S3 = % の区間按分 ✅ #2202 / S4 = 右パネルの画面 ✅ #2203 / S5 = codex ✅ #2204）

> アカウント（ログイン）ごとに、どのプロジェクト・どの機能の実装が利用制限をどれだけ使ったかを見る。
> 利用制限の % の履歴はこの機のどこにも残っておらず後から遡れないので、観測を残す S1 を最初に入れた。
> 設計の正本は `.agent/plans/2026-10-usage-analytics.md`（§9 にスライスと受け入れ条件）。

| ID | 要件 | 優先度 | 状況 |
|---|---|---|---|
| FR-2.44.1 | **利用制限の観測を残す**: tako mod の報告を受けるたび（FR-2.42）、同じペインの前の報告から値（% か解除時刻）が変わった枠と初めての会話だけを `ModHub` に積み（純関数 `tako_core::usage::pending_from_report`。heartbeat では積まない・上限 256）、GUI の 2 秒の定期処理（`drive_usage_record`）が背景で `tako_control::usage::record_for_gui` へ渡す。記録器はログイン × 枠ごとに最後に書いた（解除時刻, %, `observed_at`）を覚え、**同じ値と、`observed_at` が古い観測（放置したペインの古い値）は書かない**（`LimitTracker`）。書くのは `<data_dir>/usage/limits.jsonl`（`login` / `key_from` / `kind` / `percent` / `resets_at`（unix 秒）/ `observed_at` / `recorded_at` / `source: mod`）と `sessions.jsonl`（`session_id` → `login` / `key_from` / `seen_at`。後ろの行が正）。**起動時は何も読まない**: 覚えは最初の追記の直前にファイルの末尾 256 KB から作る（読めない行・書きかけの行は飛ばす）。置き場は設定共有のカタログで `Local`（鍵の塩がマシン固有）。行の型は `migration_registry` の指紋で縛る（追記だけのファイルなので版数の番地は持たない）。A/B は `TAKO_2192_S1_LEGACY=1`（記録しない・置き場も作らない） | M | ✅ #2200 |
| FR-2.44.2 | **ログインの鍵**: 利用制限はログインごとにかかり、tako のアカウント（設定 dir）は同じログインを重ねうる（この機では 4 アカウントが 2 ログイン）。鍵は資格情報のファイル（報告の `config_dir` = `CLAUDE_CONFIG_DIR` があれば `<それ>/.claude.json`、無ければ `<ホーム>/.claude.json`。**既定の設定 dir の 2 通り = `CLAUDE_CONFIG_DIR` 無しと `=~/.claude` は別のファイル**）の `oauthAccount.accountUuid` を `<data_dir>/machine_id`（無ければ OS の乱数で作る・0600）で塩付けした SHA-256 の先頭 12 桁。資格情報が無い・読めない・accountUuid が無いときは設定 dir の塩付きハッシュ（`key_from: config_dir`）。資格情報のファイルは更新時刻と大きさが同じ間は読み直さない。**生の accountUuid・メールは保存しない・表示しない**。診断ログ（persist.log）には最初の記録の件数と最初の失敗の種類だけを出す（cwd・label・鍵・設定 dir を出さない） | M | ✅ #2200 |
| FR-2.44.3 | **読み取りの口**: CLI `tako usage [--json]`（ローカル処理 = GUI が動いていなくても読める）/ MCP `tako_sessions` の `action: "usage"`（新しいツールは足さない）/ dispatch `Request::Sessions { action: "usage" }`（IPC / MCP は `prepare_offload` が background のジョブ `OffloadJob::UsageSummary` にする）。すべて `tako_control::usage::summary_payload` の 1 実装。応答はログインごとに `label`（初めて見た順に A, B, …）・`login`（鍵）・`key_from`・枠ごとの最新の観測（`used_percent` / `resets_at` / `resets_at_iso` / `observed_at` / `expired` / 点数）・観測と会話の数。番犬 `crates/tako-control/tests/issue2200_usage_watchdog.rs`（生の識別子・本文・設定 dir が `usage/` と persist.log に出ない・行の型の欄・UI スレッドと起動の段で usage のファイルに触らない）、実経路 `scripts/test-usage-limits-2200.sh` | M | ✅ #2200 |
| FR-2.44.4 | **使用量の索引**（S2）: 設定 dir（既定 → tako 自身の `CLAUDE_CONFIG_DIR` → accounts.yaml の `config_dir`。**検証プロセスでは既定を検証用のホームへ倒し本物の `~/.claude*` を読まない**・差し替えは `TAKO_USAGE_CONFIG_DIRS`）の `projects/` の下の `*.jsonl`（サブエージェントの `<sessionId>/subagents/` も。行の `sessionId` は親の会話）から **assistant 行の使用量のメタデータだけ**を読む（読み手の構造体は `message.id` / `requestId` / `timestamp` / `model` / `usage` の 5 成分と `speed` / `sessionId` / `cwd` / `uuid` だけを持ち、本文・`wireToolInputs` は serde が読み飛ばす。部分一致 `"type":"assistant"` で絞ってからパース）。同じ `message.id` の連続する行は成分ごとの最大へ寄せて増えた分だけ足し（仮の output）、重複の鍵は `msg_…` ならその id・それ以外は id + requestId + sessionId + 時刻で全体に 1 つ（写し・読み直しを 2 回数えない）。`<synthetic>` と時刻の無い行は数えない。書きかけの最終行（改行なし）は読まずに残す。ファイルが縮んだ・inode が変わったら最初から読み直し、数え済みの応答は鍵で飛ばす。置き場は `<data_dir>/usage/` の `cursor.jsonl`（ファイルごとの読み位置 + 新しく数えた応答の鍵。パスはハッシュで持つ・追記で後ろの行が正・生きているファイルの 2 倍 + 500 行を超えたら書き直す）/ `buckets/<YYYY-MM>.jsonl`（5 分刻み × 会話 × モデル × fast の 5 成分と応答数。金額は持たない。追記で、1 MiB を超えて前の書き直しの 2 倍に育ったら同じ鍵を合わせる。月の終わりから 35 日を過ぎた月はローカル時刻の日単位へ・365 日を過ぎた月は消す）/ `keys.jsonl`。書く順は鍵 → 集約 → 読み位置で、更新は `cursor.jsonl.lock` の排他 flock で 1 本。行の型は `migration_registry` の指紋で縛る。A/B は `TAKO_2192_S2_LEGACY=1`（索引を作らない・読まない） | M | ✅ #2201 |
| FR-2.44.5 | **鍵は取り込んだ時点で写す**（S2）: 触った会話ごとに、ログイン（置き場から推す = 既定の設定 dir は `<ホーム>/.claude.json`、それ以外は `<dir>/.claude.json` の accountUuid。S1 の `sessions.jsonl` に mod の報告由来の対応があればそちらが正）・tako のアカウント（accounts.yaml の `config_dir` との一致・既定は inherit のアカウント・当たらなければ dir の名前）・役割（カタログの `kind`）・プロジェクト（カタログの `project` → レジストリの `project` → カタログの `profile`（「プロファイル: X」）→ cwd の最長前方一致（区切り単位。`<project>-wt-N` は元へ寄せる））・主の Issue（**ラベルに書いた順で最初の `#N`**）・`task_type`（レジストリの `ledger_id` → 委任台帳）・ラベル・最初の応答の時刻を `keys.jsonl` へ書く。後から引き直した鍵は**良くなったときだけ**足す（プロジェクトの出どころが強くなった・空欄が埋まった・ログインが設定 dir の鍵から accountUuid の鍵になった）。カタログ（500 件）・レジストリ（200 件）が追い出しても、transcript が 30 日で消えても鍵を保つ。cwd・gitBranch・`prompt_head` は写さない。**カタログ・台帳のスキーマは変えない**（`primary_issue` / `ledger_id` は足さない。理由は設計書 §9 S2 の着地） | M | ✅ #2201 |
| FR-2.44.6 | **内訳の口**（S2）: CLI `tako usage [--by <project\|issue\|task-type\|session\|model\|account\|day\|role\|login>] [--window <7d\|5h\|day\|week\|month\|all>] [--account <名前>] [--project <キー>] [--limit N] [--json]`（ローカル処理。**答える前に索引を読み終える** = GUI が動いていなくても同じ関数で追いつく）/ MCP `tako_sessions` の `action: "usage"` + `by` / `window` / `account`（既存の `project` / `limit` も効く。新しいツールは足さない。答える前に最大 10 秒読み進める）/ dispatch は `prepare_offload` が `OffloadJob::UsageSummary(UsageQuery)` にする。すべて `tako_control::usage::usage_payload` の 1 実装（純関数 `tako_core::usage::report::breakdown`）。既定は**今の 7d の窓・プロジェクトの段・上位 10**。`5h` / `7d` はログインごとの実窓（観測した解除時刻 − 長さ。過ぎた・観測が無いときは今から遡る = `basis: rolling`）、`day` / `week`（月曜始まり）/ `month` はローカル時刻の暦。応答はログインごとに S1 の要約 + `usage`（`range` / `total` / `groups[]` = `key` / `name` / 5 成分 / `total_tokens` / `api_usd` / `unpriced_tokens` / `responses` / `sessions` / `more_groups`）と、`unpriced_models` / `notes` / `index`（`enabled` / `sessions` / `update` = 読んだ本数・バイト数・所要・読み残し）。API 換算は単価表（`tako_core::usage::price`。設計書 §2.3。fast は 2 倍）で問い合わせのたびに計算し、**表に無いモデルは 0 円にせず `unpriced_tokens` に数えて注記を出す**。MATRIX `usage_tokens`（claude Supported・codex Degraded = FR-2.44.11・agy #2205 / local は Pending） | M | ✅ #2201 |
| FR-2.44.7 | **負荷**（S2）: GUI は**起動の段で何も読まない**。2 秒の定期処理は材料を背景へ渡すだけで（`drive_usage_record` → `tako_control::usage::drive_for_gui`）、索引の番の判断も背景で行う。最初の更新は起動から 60 秒後、1 回 5 秒まで読んだら区切り、読み途中なら 2 秒後・追いついたら 60 秒ごとに追記分だけを読む。メモリ圧が warn 以上なら止めて 60 秒後にもう一度。MCP の問い合わせと GUI の背景は同じプロセスの索引を使い回し、CLI とは flock で 1 本にする。persist.log には最初に追いついたときの件数・バイト数・所要と、最初の失敗・メモリ圧で止めたことだけ（cwd・label・Issue・トークンの内訳を出さない）。番犬 `crates/tako-control/tests/issue2201_usage_index_watchdog.rs`（本文・ツールの入力・`wireToolInputs`・gitBranch・cwd・設定 dir の印が `usage/` と応答に出ない・読み手と行の型の欄・tako-app は `drive_for_gui` の背景だけ・最初の更新は 60 秒以上後）、実経路 `scripts/test-usage-index-2201.sh` | M | ✅ #2201 |
| FR-2.44.8 | **利用制限の % の区間按分**（S3）: ログイン × 枠（5h / 7d）ごとに `limits.jsonl` の観測を窓（解除時刻。窓の長さの半分までの揺れは同じ窓へ寄せる）ごとに並べ、観測時刻の順に**値の上がった点だけ**を残し（同じ値は最初の点が正・下がった値は古い報告・窓の外の時刻は捨てる）、窓の開始（解除時刻 − 長さ。前の窓の解除時刻より前には置かない）を 0% の点として先頭に置く。隣り合う点の区間の Δ を、区間に重なる同じログインの行（5 分刻み。35 日より古い月は日単位。端の刻みは重なった時間の比・今の刻みは今まで）へ **API 換算の比**で配る（単価表に無いモデルの行を含む区間は生トークン総量の比）。区間のローカルの使用量が 0 で Δ > 0 なら「**tako の外**」。観測の区間の外（最後の観測より後・観測の無い窓）は、係数（直近 7 日の区間の 1% あたりの API 換算の中央値。無ければすべての区間）で割った値を「**推定**」として別に持ち、係数が無ければ % へ直さず `unestimated_usd` に数える。按分は期間・絞り込みの前のログインの全行で行い、段へ足すときに `account` / `project` で絞る（絞ってから配ると Δ を独り占めする）。純関数 `tako_core::usage::share`（`windows_for` / `current_window` / `allocate`）。実窓（`5h` / `7d`）の期間の判定も同じ 1 実装に寄せ、観測した今の窓が無いときは最後に解けた窓の解除時刻より前へ遡らない | M | ✅ #2202 |
| FR-2.44.9 | **% 列の口**（S3）: `tako usage` / MCP `tako_sessions` の `action: "usage"`（新しい引数は無い。説明 +32 B）の応答のログインごとの `usage.limit_share[]`（枠ごとに `available` / `scope` = `current_window`（実窓の問い合わせ: 枠ごとの今の窓の、期間に重なる分）か `period`（暦と `all`: 期間に重なる窓の延べ）/ `from` / `to` / `resets_at` / `latest_percent` / `observed`（段 + tako の外。今の窓を丸ごと見るときは `latest_percent` と一致）/ `allocated` / `outside` / `estimated` / `unestimated_usd` / `coefficient` / `windows[]`（窓ごとの当てはまり = 区間の 1% あたりの API 換算の中央値と四分位。新しい方から 10 個）/ `ignored_observations`）と、段の行の `limit_share.{five_hour,seven_day}.{percent,estimated}`。既定（今の 7d の窓）は 7d の列の合計が今の 7d の観測・5h の列の合計が今の 5h の観測に一致する。段は主の枠（`--window 5h` なら 5h・それ以外は 7d）の % の大きい順。CLI の既定出力は段の左に主の枠から 2 列（推定は `+`）、「tako の外」「推定の合計」の行、枠ごとの按分の要約（観測 = 段 + tako の外・推定・係数と直近の窓の四分位）。索引は期間の始まりの 7 日前（直近 7 日の係数の区間まで）から読む（`report::read_from`）。観測の無いログイン・枠は `available: false` で % を出さず注記する。使用量が無くても観測のあるログイン（すべて tako の外）は絞り込みの無いときに出す。MATRIX `usage_limit_share`（claude Supported・codex Degraded = FR-2.44.12・agy / local は窓つきの利用上限が無いので Unsupported） | M | ✅ #2202 |
| FR-2.44.10 | **mod の無いペインの画面の値（`source: screen`）は残さない**（S3 の判断）: 画面の値は解除時刻を持たず窓の切り替え（0% へ戻る）を切れない・ステータスバーはそのペインのログインを知らない・別のログインの係数を借りるとプランの比（S0 の実測で 5h と 7d の 1% の比が 1/4 と 1/6）だけずれる。tako の claude のペインは mod が既定で入る（FR-2.42.3）ので、観測の無いログインは版が下限未満・mod を切った・tako の外で起動した場合に限られ、% を出さずに注記する（FR-2.44.9）/ 右パネルの `usage` ビュー（S4 #2203）/ agy の調査（S6 #2205）/ スマホ（S7 #2206） | M | #2203〜 |
| FR-2.44.11 | **codex の使用量**（S5）: 同じ索引が `CODEX_HOME`（`CODEX_HOME` → `<ホーム>/.codex`。**検証プロセスでは検証用のホームの `.codex` へ倒し本物も `CODEX_HOME` も読まない**・差し替えは `TAKO_USAGE_CODEX_HOMES`）の `sessions/**/rollout-*.jsonl` と `archived_sessions/` を読む（純関数 `tako_core::usage::codex::ingest_codex_lines`）。読む行は `session_meta`（会話 = `id`・cwd・fork の印）/ `turn_context`（モデル・cwd）/ `event_msg` の `token_count` だけで、部分一致で絞ってからパースし、発話・推論・ツール・`base_instructions` は serde が読み飛ばす。使用量は `total_token_usage` の**累積の差分**（差の和 = 最後の累積。欠けた行は次の差が埋め、同じ累積の重複は 0。累積が下がったらその行の `last_token_usage` から数え直す）を行の時刻の 5 分刻みへ、5 成分は入力 − 読み出し − 書き込み / 読み出し / 作成（5 分）= 書き込み / 出力（推論を含む）で、和は codex の `total_tokens`。fork・サブエージェントの rollout（`parent_thread_id` / `forked_from_id` / `source.subagent`）は `session_meta` の時刻から 1 秒以内に続く使用量の行を親の履歴の写しとして数えず基準だけ進める（ccusage と同じ見分け方）。読み位置は `cursor.jsonl` の行の `codex`（会話・fork・モデル・最後の累積・写しの束・読み直しの数え済みの累積。cwd は書かない）で、鍵はファイル名（`archived_sessions/` へ移っても続きから）。縮んだ・置き換わったら最初から読み直し、数え済みの累積を超えた分だけ数える。鍵は `agent: codex`・ログイン = `<CODEX_HOME>/auth.json` の `tokens.account_id` の塩付きハッシュ（区切りの語を claude と分ける。読めなければ `CODEX_HOME` の鍵）・アカウント名 = `codex`（既定の `~/.codex`）か `CODEX_HOME` の名前・プロジェクトはカタログ → cwd の最長前方一致。**rollout はどのログインで使ったかを持たないので、過去の分も今のログインへ寄せる**。codex のモデルは単価表に無いので API 換算は 0 で `unpriced_tokens` に数える。応答のログインに `agent`（`codex` / `claude`）、CLI の見出しは `ログイン X（codex・鍵 …）`。A/B は `TAKO_2192_S5_LEGACY=1`（codex を読まない）。番犬 `crates/tako-control/tests/issue2204_usage_codex_watchdog.rs`（発話・指示・ツール・cwd・ブランチ・トークン・account_id・`CODEX_HOME` のパスが `usage/` と応答に出ない・読み手と読み位置の型の欄）、実経路 `scripts/test-usage-codex-2204.sh`。MATRIX `usage_tokens` の codex は Degraded（API 換算が無い・ログインを持たない） | M | ✅ #2204 |
| FR-2.44.12 | **codex の利用制限の % の按分**（S5）: 同じ `token_count` の行の `rate_limits` の `primary` / `secondary` を `window_minutes`（300 = `five_hour`・10080 = `seven_day`。**primary が週の枠のプランがある**ので位置では決めない）で枠へ写し、`limits.jsonl` へ `source: codex` で書く（`limit_id` が `codex` 以外 = モデル別の別の枠と知らない窓の長さは書かない）。書くのは「ログイン × 枠 × 解除時刻 × %」ごとにいちばん早い観測の 1 行（同じ値の 2 回目と読み直しでは書かない。後から別のファイルでより早い観測が見つかったときだけ書き足す）。**過去の rollout の分もすべて観測になるので、過去の窓の % も配れる**。窓の切り方（`share::windows_for`）は codex の観測だけのログイン・枠で変える: 解除時刻の揺れは 1 時間まで寄せ（codex は報告ごとに解除時刻を計算し直し、別の会話は数分ずれる。実測で最大 298 秒）、窓の開始は「揺れの中でいちばん早い解除時刻 − 長さ」で前の窓の最後の点より前には置かない（**前の窓の解除より前に新しい窓が始まる** = プランの切り替え・早い解除。claude の「長さの半分」で寄せると別の窓を束ねて延べを落とす）。按分（`share::allocate_with`）は codex では**推定しない**（使用量と観測が同じ行に載るので、区間の外は % の上がらなかった続きか、その枠の無いプラン = 週の枠だけの期間）。重みは生トークンの比（単価表に無い）。MATRIX `usage_limit_share` の codex は Degraded（重みが生トークン・ログインを持たない） | M | ✅ #2204 |
| FR-2.44.13 | **右パネルの usage ビュー**（S4）: `PanelViewWire::Usage`（`tako panel --view usage` / MCP `tako_panel` の `view: usage` / パレット `panel-usage` / 表示メニューの「パネル」の「usage ビュー」= #2341 / タブ）。入口はステータスバーの 5h / 7d のメーター（その期間で最上段から開く。サービス名の側は従来どおり切り替えメニューで、メニューの末尾にも同じ入口 = mod がバーを描いてメーターが出ていないとき）。中身はログインごとに最新の観測（主の枠が先）・解除時刻・期間・量の合計と、段の行（主の枠の % + 推定・棒・もう一方の枠・API 換算・トークン・会話数）・「ほか N 件」+「さらに表示」・tako の外・推定・観測の無い枠・注記（既定は畳む）。行を押すと **プロジェクト → Issue → 会話**と掘り（`tako_core::usage::report::drill` の 1 実装）、パンくずで戻る。期間は 7d / 5h / 今日 / 今週 / 今月 / 全期間。タブは条件つき（このビューを選んでいるか、このセッションで開いたとき）で、8 本 + 3 桁のバッジが最も詰めた段で収まるよう右パネルの最小幅を 240 → 264px（幅は永続しない = 移行なし） | M | ✅ #2203 |
| FR-2.44.14 | **画面と CLI は同じ数値**（S4）: 画面は集計しない。読みは CLI / MCP と同じ `Request::Sessions { action: "usage" }` を `dispatch_off_ui` で投げ（`prepare_offload` → background のジョブ `OffloadJob::UsageSummary` = `usage_payload`。索引は 10 秒まで読み進める）、届いた JSON を描くだけ。数字の書式も CLI と同じ `tako_core::usage::display`（`tako usage` の既定出力の書式をここへ寄せた）。開いている間だけ 2 秒の定期処理（`tick_usage_panel`）が 60 秒ごと（索引が読み途中なら 5 秒ごと）に読み直し、段・期間を切り替えた直後は古い答えを新しい見出しで描かない（集計中を出す）。答えの取り違えは頼んだ順の番号で防ぐ。番犬: `issue1984_ui_thread_offload_watchdog` の呼び出しグラフの葉に索引の読み（`usage::usage_payload(` / `index::read_rows(` / `index::read_keys(`）を足し UI スレッドの入口から届かないこと、`issue2201_usage_index_watchdog` が `usage_panel.rs` は読みの口を直に呼ばず dispatch 経由だけであること | M | ✅ #2203 |
| FR-2.44.15 | **掘り下げの段の CLI / MCP の口**（S4。設計原則 5）: 画面の段は `tako usage --by <段> --project <値> --issue <値>` と 1:1。`--project` は `<キー>`（S2 からの既定 = プロジェクトか同名のプロファイル）に加えて `project:<キー>`（プロジェクトだけ）/ `profile:<名前>`（master・solo のプロファイルだけ）/ `none`（未分類）で段の行を 1 つだけ選べ（`report::ProjectFilter`）、`--issue` は番号（`#` 付き可）か `none`（Issue なし。`report::IssueFilter`）。読めない値は書き方つきで断る。応答に効いた絞り込み `filter`。`--show`（MCP `tako_sessions` の `show: true`）は同じ段・期間・絞り込みで画面を開く（UI スレッドは引数を確かめて段を置くだけ = `ControlHost::show_usage_view`。答えは同じ内訳 + `shown`。CLI は tako が動いていなければ理由を言って止まる）。いま開いている段は `tako panel` の応答の `usage`（`by` / `window` / `project` / `issue` / `depth` / `loading`。開いていなければ null）。MCP カタログは `tako_sessions` の `issue` / `show` と `project` の説明・`tako_panel` の `usage` で増分（PR に記載） | M | ✅ #2203 |

## FR-3 コンセプト②: 軽量 IDE 的ワークスペース

| ID | 要件 | 優先度 |
|---|---|---|
| FR-3.1 | 左サイドバー: 「**タブ = ワークスペース**」のファイルツリー。**アクティブタブ内の全ペインの cwd**（OSC 7）をワークスペースフォルダとして**マルチルート表示**する（VSCode の「フォルダをワークスペースに追加」相当。ルート見出し行 + 展開折りたたみ。重複 cwd は 1 つに。2026-06-13 に「フォーカスペインの cwd 連動」から変更 = ✅ 実装済み）。**ディレクトリへのシンボリックリンクはディレクトリとして扱う**（種別は**辿った先**で決める = 開く側の `Path::is_file()` / `open_plan::route` と同じ向き。辿れないリンクはファイル行のまま = 押せば `OpenFile` が理由を返す。リンクの循環は canonical パスの照合で打ち切り、打ち切りは行として見せる。判定の 1 実装は `filetree::entry_is_dir`。規約は `conventions.md`「「ディレクトリか」の判定はリンクを辿る側に揃える」。✅ 2026-09-12、#1398）。**1 ディレクトリの表示上限（500 件）で切り詰めたぶんと、読めなかったディレクトリは行として申告する**（切り詰めは「N 件まで表示（全 M 件）」の情報行 / 読めないときは理由の行 = 空ディレクトリと区別する。切り詰めの判断は CLI / MCP の `tree git-status` の `truncated` と同じ 1 実装 `tako_core::sidebar::Truncation`。✅ 2026-09-12、#1402） | M |
| FR-3.19 | **ドット項目の既定非表示と新ルートの追随**: ファイルツリーはドット始まり（`.git` / `.env` 等）を既定で隠す（ルート見出しは対象外 = ユーザーが明示的に開いた `~/.claude` 等は隠さない。VSCode / Zed は `.git` 等の個別除外でドット全体は隠さないが、tako はホームを開いた初回印象を優先して全体を隠しワンクリックで戻せる形にする）。表示切替はサイドバー見出しの目アイコン / ツリーの右クリックメニュー / 設定画面（外観）の 3 経路。状態は settings.json（`show_hidden_files`）へ永続化し、CLI `tako panel --show-hidden on\|off` / MCP `tako_panel` の `show_hidden` へ 1:1 公開する。**新しく増えたワークスペースフォルダは自動展開のうえ先頭へ出す**（見出しだけ切り替わって本体が追随しない状態を作らない。既存ルートの並びは維持するのでポーリング同期で順序は暴れない）（✅ 2026-07-27、#550） | M |
| FR-3.20 | **新規作成のインライン入力**: 新規ファイル / フォルダの入力欄は**作成先ディレクトリ行の真下**に、その子と同じ深さ（通常行と同一のインデント規則）で挿入する。作成先の行を強調し、空のときは用途に応じたプレースホルダを出す。確定後は該当ディレクトリを即座に読み直し、ポーリングを待たずに正しい並び順の位置へ項目を出す。入力欄の位置は VSCode の Explorer に合わせ、ツリーの並び（ディレクトリ先 → 名前順）に沿って新規フォルダは親の真下、新規ファイルはディレクトリ兄弟を飛ばしたファイル群の先頭へ置く（✅ 2026-07-27、#559） | M |
| FR-3.21 | **インデントガイド線の連続性**: 同じ深さのガイド縦線は、その深さに子が続くあいだ**切れ目のない 1 本の縦線**として描く（VSCode / Zed と同じ見え方）。行ごとに「自分の深さの線」だけを引くと、子孫の行が挟まった瞬間に祖先の線が途切れて破線に見えるため、**各行が祖先ぶんの縦線もまとめて描く**。線の太さ（1px）・色（`border_subtle`）・インデント幅（17px）・本文の開始位置は据え置き。行の高さ・テーマ（dark / light）・スクロール位置に依存せず連続すること（✅ 2026-07-27、#589） | M |
| FR-3.22 | **Finder の「このアプリケーションで開く」対応（macOS）**: tako.app が `CFBundleDocumentTypes` を宣言し、Finder の「このアプリケーションで開く」候補に並ぶ。選ばれたものは**新しいタブ**で開き、tako を前面に出す（#835）。**ファイルは「そのファイルだけが載ったタブ 1 枚」**（タブ名 = ファイル名の手動タイトル。プレビュー 1 ペインのみで PTY は起動しない）、**フォルダはそのフォルダでシェルを起動したタブ**（ターミナルに渡された「そこで作業を始めたい」と読む）、**存在しないパスは警告して読み飛ばす**（他のファイルを巻き添えにしない）。宣言外・未知の形式もファイルとして受け、表示モードの決定と巨大ファイルの切り詰めは FR-3.2 / FR-3.17 の既存経路が持つ。**複数選択は 1 ファイル = 1 タブ**（同じプレビューペインを順に差し替えると最後の 1 枚しか残らず、選んだファイルの大半が「開かなかった」ように見えるため）で、最後に開いたタブが前に出る。既存のタブ・ペインには一切触らない。**`LSHandlerRank` は全宣言で `Alternate` 固定**（既定アプリを一切奪わない = 候補に出るだけ）で、これは `tako-app::open_files` のテストが `build-app.sh` を読んで機械検証する。宣言は UTI（`LSItemContentTypes`）のみで行い、`CFBundleTypeExtensions` は使わない（macOS が UTI を持たない拡張子は他アプリの宣言が 1 つも無いと Alternate でも tako が既定ハンドラになり得るため。実測で `.rs` / `.toml` / `.go` / `.conf` 等が `dyn.*` = `public.data` 止まり）。**動的 UTI の形式（45 形式中 22。`.rs` `.go` `.toml` `.ino` 等）は候補に出ないままにする**（#1955 で 7 案を本物の Launch Services で実測: 開く役で候補に出る宣言は、その拡張子を他に宣言するアプリが無い Mac で必ず既定になり、rank None / role None / `dyn.*` の宣言は開く候補に出ず、UTI の輸入は適合先の全アプリを候補に入れて既定も動かす。比較は `.agent/plans/2026-10-language-coverage.md` §6.3）。それらは tako の中のファイルツリーと `tako open` で開く。**Windows のエクスプローラーの「プログラムから開く」には出ない**（インストーラが関連付けを登録しない。`tako platform` では `tako_open_file` を Windows で degraded と申告。既定を奪わない OpenWithProgids の案と実機確認は #2134）。対象は text / plain-text / source-code / script / json / yaml / xml / markdown（Editor）と pdf / image / movie（Viewer）。**フォルダ（`public.folder`）は宣言しない**ので Finder の候補には出ないが、「その他…」・`open -a` から渡されれば上記のとおり受ける。受け口は macOS の `application:openURLs:`（GPUI の `App::on_open_urls`）で、未起動から開かれると `run` より先に届き得るためコールバック登録は `run` の前・受け取りは channel 経由にして取りこぼさない。**復元（FR-5）は `TakoApp::new` の中で同期的に終わる**ので、channel の消費側が動く時点でタブ・ペインは揃っている = 復元と新規タブが混ざらない。表示は**既存 dispatch のみ**に載せる（ファイル = `OpenFile` の `new_tab`、フォルダ = `TabNew` の `cwd`）ので、AI 側の同等操作は `tako open --new-tab` / MCP `tako_open_file` の `new_tab`、`tako tab new --cwd` / MCP `tako_create_tab` の `cwd` がそのまま担う（新ツールを増やさない = 開発不変条件を既存経路で満たす）。`new_tab` と `direction` は分割元が無いため排他。ウィンドウが 0 枚のときは Dock 復帰と同じ経路で開き直す。**候補に並ぶのは `/Applications/tako.app` 1 つだけにする**（#837）: 同じ identity の `.app` がディスク上に 2 つあると Launch Services は両方を登録し、候補に tako が 2 つ並ぶ（`/Applications` 側とビルド出力側でバージョン表記まで食い違う）。実測（macOS 26 / Darwin 25.4）で`lsregister -u` は**ファイルを一切触らなくても 1 分前後で取り消され**（48〜70 秒後に再登録）、`*.noindex` / `.metadata_never_index` / `chflags hidden` / ドット始まりの隠しディレクトリはどれも登録を止められず（LS は Spotlight とは独立に `.app` を拾う）、逆に実体を消しただけでは登録が残骸として残ったので、**「実体を消す + `lsregister -u`」の両方**が唯一の恒久対策。`build-app.sh --install` と**公開が成立した `release.sh`**（#166 の夜間リリースは 2 段目で公開するのでそこで片付く。ここを外すと毎晩再発する）がビルド出力を消し、実体の無い登録（他 worktree の残骸）も外す。実体が残る別の `tako.app` は消さずに掃除手順を提示する（`CFBundleDocumentTypes` を宣言していない古いバンドルは登録されても候補には出ないので注記して区別する）。LS 操作は `scripts/lib/launch-services.sh` に集約し、`tako-app::open_files` のテストが「削除 → 登録解除」の順序と両スクリプトからの呼び出しを機械検証する（✅ 2026-08-01 候補表示とプレビュー表示、#708 / ✅ 2026-08-17 新規タブ化・フォルダ・複数選択・CLI / MCP 1:1、#835 / ✅ 2026-08-21 候補の重複排除、#837 / ✅ 2026-10-16 動的 UTI の形式へは広げないと実測で決定、#1955） | S |
| FR-3.23 | **左サイドバー幅の可変とクランプ規則**: 右端ハンドルのドラッグで幅を変えられ、値は settings.json（`sidebar_width`。既定 244）へ永続化し、CLI `tako panel --sidebar-width <px>` / MCP `tako_panel` の `sidebar_width` へ 1:1 公開する（✅ 2026-07-17、#307）。**クランプ規則は全経路で 1 つ**（下限 120px / 上限は**ウィンドウ幅の 50%**）で、正は `tako_core::sidebar`（`MIN_WIDTH` / `MAX_RATIO` / `clamp_width`）。上限を固定 px にしないのは、①UI でできることは AI からもできる（設計原則 5。固定 600px では広いウィンドウで CLI からドラッグ相当の幅に届かない）②固定 px は狭いウィンドウで過大になる（600px は 800px の窓の 75%）の 2 点。状態として持つ・永続化するのは**要求値**で、描画・座標計算に使うのは実効幅（要求値をそのウィンドウ幅でクランプした値）とし、**ウィンドウが狭くなっても要求値は書き換えない**（狭い窓では収まる幅で描き、広げ直す・再起動すると元の幅へ戻る）。dispatch 経路はウィンドウを持たないので上限は最後に描いたビューポート幅から取り、応答は実効幅 + `sidebar_width_max` / `sidebar_width_min` を返す。永続化されるのは要求値ではなく**適用された幅**（画面と settings.json を食い違わせない）（✅ 2026-08-14、#789） | M |
| FR-3.24 | **リモートからフォルダを開く（SSH 先のワークスペース化）**: ファイルメニュー「リモートからフォルダを開く…」/ ⌘K パレットから **ホスト選択（`~/.ssh/config` の Host）→ フォルダ選択 → ファイルツリーへ**。開いたリモートフォルダはツリーのルートとして**ローカルルートと同じ形の見出し**（フォルダ名 + フォルダアイコン）で**ローカルの後ろに**並び、SSH であることと相手のホスト名は行末のバッジが示す（#976 でこの形へ。#919 当時は `host: 末尾要素` + 地球アイコン + ローカルより先に出す独自形だった）、展開でリモートのディレクトリ構造を辿れ、ファイルは**プレビューで開けて編集・保存できる**（書き戻しは FR-3.24.2）。右クリックから「リモートのパスをコピー / このフォルダで SSH ペインを開く / 再読み込み / 閉じる」。**バックエンドはシステムの `ssh` / `sftp` + ControlMaster**（russh / ssh2 は ControlMaster に相乗りできず `~/.ssh/config` / known_hosts / agent / FIDO / 2FA を作り直すことになるため採らない。#65 の宿題への回答）で、`sftp -b -` はログインシェルに依存しないので**相手が PowerShell（Windows OpenSSH）でも同じ経路で動く**。対話 SSH ペインもツリーと**同じ ControlPath** を通すので、パスワード認証しか無い相手でも一度ログインすれば以後ツリーが追加認証なしで開く（#65 要件 1 の「パスワード認証しかない場合の UX」）。プレビューは SFTP で `<data_dir>/remote-cache/` へ落として**既存プレビュー（構文色 / md / 画像 / PDF / 目次 / リンク）をそのまま使う**（編集・保存は FR-3.24.2）。**すべての失敗は 13 種別へ分類し「何が起きたか」+「次に何をすべきか」+ 生の詳細を持つ**（ツリーの読み込み失敗は行として出し、接続・一覧の失敗はサイドバー上部の通知へ。**失敗は自動で消さない**）。ネットワーク I/O は **UI スレッドで待たない**（ツリーの展開は background へ投げる）し、**見えていないツリーのためには走らせない**（`sidebar_closed` / `not_displayed` を状態として区別する）。開いたリモートフォルダは layout.json（タブ単位）へ永続化し、**ローカル FS の存在検査は通さない**（`is_dir()` に掛けるとリモートのパスは必ず全部消える）。1 本でも残っていれば起動時にツリーを見せる。CLI `tako remote-folder open/close/list/ls/open-file/ssh-pane` / MCP `tako_remote_folder` へ 1:1 公開し、`ls` は**ツリーを開かずにリモートの構造を覗ける**（#65 要件 5 = AI がリモートの構造を把握する経路）。`close` は既定で全タブ横断（`--tab` で絞る）。**接続多重化（ControlMaster）は OS の OpenSSH の能力なので境界（`platform::ssh_client`）でゲートする**（#1090。Windows の OpenSSH は多重化を実装しておらず、渡すと接続が成立する前に `getsockname failed: Not a socket` / exit `-1` で死ぬ = SSH 系の全経路が無言で壊れる。渡さない形にすると同じ相手へ握手が進む = 実機実測）。多重化が無いプラットフォームでは**操作ごとに独立した接続**になり、パスワード認証しか無い相手は展開のたびに認証が要るうえ、接続の生死も安く判定できない（`remote_fs::liveness` が `Unknown` を返し、`tako remote-folder list` の `connected` は `null` になる）。この縮退は `platform::support` のマトリクス（`tako_open_remote` / `tako_remote_folder` = Degraded）と `NO_MULTIPLEXING` の日英文言が正本（✅ 2026-08-24、#919。バックエンドは #65。多重化のゲートは 2026-09-03、#1090） | S |
| FR-3.24.1 | **SSH ペインの接続失敗を無言にしない**（#919 のバグ側）。旧実装は `ssh` を**素のペインのプログラム**にしていたため ①接続待ちのあいだ画面が完全に空（実測: TCP ブラックホールで 25 秒間 1 文字も出ない = 「何も入力できない」に見える）②ssh が即死するとペインの死とともにタブごと閉じて理由が残らない（実測: 名前解決できないホストは 1 秒でタブ消滅）。`remote_fs::ssh_pane_script` で包み、①接続前にバナー（どのホストへ / 中止の仕方）②`ConnectTimeout` を明示（ssh 既定の約 75 秒 → 10 秒）③**ssh 自身の失敗（exit 255）だけ**理由 + 次の一手を出して入力待ちで止まる（リモートシェルの `exit 1` は従来どおり素通しで閉じる = 誤報しない）。スクリプトの方言は境界（`platform::shell::script_pane_command` / `script_dialect`）が決めるので POSIX / PowerShell の両方で成立する | M |
| FR-3.24.2 | **リモートファイルの編集・保存（SFTP 書き戻し。段階 2）**: リモート由来のプレビューを既存の編集 UI（FR-3.5）でそのまま編集し、保存で **SFTP の書き戻し**を行う。#919 段階 1 が編集を構造的に禁じていたのは「キャッシュを本物と思って保存する」事故（リモートには何も書かれない）を防ぐためなので、**禁止を外す代わりに次の 3 つを置く**。①**アトミック**: 対象と同じディレクトリへ一時ファイル（`.<名前>.tako-<ハッシュ>.tmp`）を `put` してから `rename` で被せる（OpenSSH の `posix-rename@openssh.com` は既存を上書きする = Linux / Windows の実機で実測。**同一ファイルシステム内でしか rename できない**ので `/tmp` 経由にはしない）。`put` は元の mode を引き継がない（実測: `-rwxr-xr-x` → `-rw-r--r--` = 実行権が落ちる）ので、POSIX として読める mode なら書き戻し後に `chmod` で戻す（Windows の sftp-server は権限欄が `*` 埋めで `chmod` が効かないので送らない）。②**競合検知**: 「開いた時点のリモートの内容」を `<写し>.tako-base` として持ち（取得のたびに更新するので「読み直して競合を解消する」導線がそのまま効く）、書く前にリモートの実体と突き合わせる。**サイズと mtime ではなく内容そのもの**を見る（`ls -la` の日時は分の分解能しかなく、同じ分・同じサイズの書き換えを見逃す）。食い違えば書かずに `RemoteErrorKind::Conflict`（14 種別目）を返し、`force` を明示したときだけ上書きする。相手から消えていれば `NotFound` で止まる（黙って作り直さない）。③**失われない失敗**: 押し出せなければ書きたかった内容を `<data_dir>/remote-cache/pending/<id>.body` へ退避し（メタは `.json`。識別子は host + path で決まるので同じファイルを何度保存しても増えない）、`push` で再試行できる。**GUI の ⌘S はローカルの写しへ同期で書いてからリモートへは背景で押し出す**（実測: ControlMaster に相乗りしても 1 バッチ 1〜2 秒。⌘S ごとに UI が固まるのを避ける。進行中にもう一度保存されたら終わってから 1 回だけ押し直す）。**dispatch = CLI / MCP 経路は同期**で押し出し、応答の `remote`（`state` = idle / uploading / saved / failed / pending（= 前のセッションの退避が残っている）・`pending_write`・`conflict_checked`）で「リモートへ書けたか」まで返す（ローカルの写しへ書けただけで `saved` と言わせない）。**書けないと分かっているファイル**（mode のどこにも `w` が無い。Windows は判定材料が無いので対象外）は従来どおり読み取り専用で、編集開始の時点で拒否する。リモートの編集セッションは**自動保存を既定 OFF**（1 保存が SFTP 3 バッチなので打鍵ごとに走らせない。Zed / VSCode も既定は手動保存）。CLI `tako remote-folder pending` / `push [--force]` と MCP `tako_remote_folder` の同 action / `force` へ 1:1 公開する。同一バイナリで段階 1 の挙動へ戻す A/B は `TAKO_966_LEGACY=1`（✅ 2026-08-27、#966） | S |
| FR-3.24.3 | **ペインの `ssh` を検知してリモートフォルダを自動追加する（#976 / #65 要件 1）**: ユーザーがペインで `ssh <host>` に入ったら、**明示的な「リモートからフォルダを開く」操作なしに**そのホストのホーム（sftp の初期 cwd）をツリーのルートとして開く。検知は「コマンド行 → 宛先」（`tako_core::ssh_detect`。純関数）と「どのペインの配下か」（`tako_control::ssh_detect`。器を持つペインは器のセッション名から、持たないペインは PTY 直下の子 pid から辿る二段構え）に分かれる。**取り違えたら別のマシンの中身をそのホスト名で見せてしまう**ので、tako が `ssh <宛先>` を打ち直しても同じ相手に届くと確信できない形は全部見送る（`-p` / `-o Port=` / `-J` / `-W` / `-o ProxyJump=` / `-o ProxyCommand=` / `-o Hostname=` / リモートコマンドの一発実行（`git` / `rsync` が内部で使う形）/ `-N`（転送・ControlMaster）/ `-s`）。見送った理由は `SkipReason`（7 種別・日英）として `auto` の応答と `persist.log` に残す。接続は既存の `remote_fs::ensure_master`（`BatchMode=yes`）なので**パスワードを聞きに行かない**（鍵・agent で入れない相手は理由を出して見送る = ユーザーが一度ログインすれば ControlMaster を共有して次から開く）。**走査は毎 tick 走らせない**: 材料はメモリ上のもの（ペイン集合・PTY 直下の子 pid・**OSC 133 のコマンド状態**）だけで、指紋が変わったときと 60 秒の保険でのみプロセス表を採る。対話 `ssh` は入るときに `Idle → Running`、抜けるときに `Running → Idle` へ動くので**検知も切断もこの変化で拾え**、全ペインが idle で追跡中のホストも無ければ走査自体が起きない（アイドルのコストがゼロ）。採取は #772 / #779 の `ProcessSnapshot` へ相乗りするので**子プロセスの起動回数は増えない**（`ps` の列を 1 つ増やすだけ）。接続の確認（`connect` + `list_dir`）は**必ず background** で行い、成功したら `open` と同じ器づけ（`dispatch::attach_remote_root`）を通す（UI スレッドでネットワークを待たない）。**同じホストのルートがそのタブに既にあれば何もしない**（#919 の明示経路と共存し二重に並べない）。試行は 1 エピソード 1 回（失敗しても繰り返さず、抜けて入り直したら再試行）。**切断してもルートは消さない**（バッジが「切断」へ変わり、右クリックの「再読み込み」で復帰できる）。既定 ON で、`settings.json` の `ssh_auto_folders` / CLI `tako remote-folder auto [on\|off]` / MCP `tako_remote_folder` の `action: "auto"` から無効化できる（**無効化すると走査対象が空になる** = プロセス表の採取そのものが起きない。既に開いたルートは設定の切替で勝手に閉じない）。`auto` の応答は「検知していない」と「検知できない」を区別する（`detection: "unavailable"` = プロセスのコマンド行を採れない環境。Windows は境界が実行ファイル名しか返さないので自動検知は働かず、明示経路だけが使える）（✅ 2026-08-27、#976） | S |
| FR-3.24.4 | **ネット断からの自動復帰（ペイン・フォルダ・保留中の保存）**: 回線が切れても**ペインもタブもリモートフォルダも失わせず**、回線が戻ったらユーザーが何もしなくても続きから使えるようにする。①**消さない**: `ssh_pane_script` の尾を「Enter でこのペインを閉じます」+ 入力待ちから**ローカルのログインシェルへ落ちる**形へ変えた。旧文面は**接続前の失敗**には正しいが、**繋がったあとに切れた**ときも同じ画面になるため、案内どおり Enter を打ったユーザーがペイン → タブ → そのタブのリモートフォルダを一度に失っていた（実測: `before tabs=2 panes=2` → `after tabs=1 panes=1`。「ネットが切れたらタブ閉じてリモート消える」の正体）。②**自動再接続**: 切断を検知したら**ローカルシェルのプロンプトへ `ssh …` を打ち直す**（#640 の送達確認つき経路）。切断後は 3 経路（split / tab / pane。#1006）とも同じ形になるので**再接続の実装は 1 本**。バックオフ（2 / 5 / 10 / 20 / 30 / 30 秒）+ 上限 6 回で、越えたら**理由 + 次の一手**（`ssh <host>` を実行してください）を出して静かに止まる。**一度でも接続が成立したペインだけ**が対象（届かない相手を延々と叩かない）で、鍵・ホスト鍵・設定の失敗（`is_recoverable_reason`）は繰り返さない。ユーザーがそのペインで打ち始めたら黙って降りる。③**切断の見分け**: 正常終了（`exit`）でも ssh は `Shared connection to <host> closed.` を出すので**文言だけで判定してはいけない**。根拠は経路ごとに スクリプトのマーカー行（exit 255 のときだけ出る）/ シェル統合（OSC 133）が返す終了コード 255 / 統合が届かないペイン向けの保険（**壊れたときにしか出ない行**だけを見る）の 3 つ。④**フォルダと保留の自動復帰**: 切断中は `master_alive`（ソケットの stat）で待ちを持ち、バックオフ（5 / 10 / 20 / 40 / 60 / 120 秒・上限 8 回）で繋ぎ直す。戻ったらツリーを読み直し、#966 の `pending`（切断中の保存）を**自動で push** する（`force` はしないので競合は既存の分類に従う）。ペインの再接続が通った合図でも同じ復帰を起こす。⑤**master にも keepalive**: `ensure_master` は `ServerAlive*` を渡していなかったため、ツリーでフォルダを開いている（= 普段の使い方）と**ブラックホール型の切断を 66 秒観測しても検知できず**ペインが無言で凍っていた（実測）。⑥**状態は CLI / MCP 1:1**: `ssh_connect`（#1010）に `connected` / `reconnecting` / `gave_up` を足し、`attempt` / `max_attempts` / `retry_in_secs` / `disconnected_secs` / `reason` / `next_step` を `tako list` / `read` から読める。**スコープ外**: リモート側のシェルセッションの中身の継続（実行中プロセスの生存）はリモートに tmux 等の器が要るので扱わない。同一バイナリで自動再接続を切る A/B は `TAKO_1040_LEGACY=1`。**⑥の追記（#1446）**: `Connected` のペインでも `ssh_connect` は `reconnect` だけを返すようになった（進行状況は従来どおり異常時だけ）。追跡そのものの永続・引き取りは FR-3.24.6（✅ 2026-08-29、#1040） | S |
| FR-3.24.6 | **自動再接続の対象であり続ける（追跡の永続と引き取り）**（#1446）。FR-3.24.4 の判断（`should_arm` / `detect_disconnect`）は正しかったが、**「このペインは SSH だ」の記憶がプロセスの寿命しか持たなかった**ため、発火しないペインが実在した（実機報告）。①**追跡を `layout.json` へ永続**: `PaneLayout.ssh`（`host` / `reconnect_line`）へ落とし、復元時に引き継ぐ。落とすのは**接続が成立した実績のあるペインだけ**（届かない相手で開いただけのペインを再起動後に叩き始めない）。旧ファイルは `serde(default)` でそのまま読めるので移行 Step は不要。引き継ぐのは**器（tmux）が生きていたペインだけ**（器ごと消えていたら新しいシェルが開く = 見張る相手が居ない）。②**手打ちの `ssh` を引き取る**: dispatch を通らないペイン（ユーザーが自分で `ssh <host>` と打った）は追跡されず、切断しても無言でローカルへ落ちていた。#976 の ssh 検知が**既に見つけている**ので、その材料で未追跡ペインを引き取る（新しい走査は増やさない）。打ち直す 1 行は `remote_ssh_argv` + `launch_cmd` = **dispatch と同じ 1 実装**で組む（`ps` のコマンド行をシェルへ流し込まない）。③**諦めたペインの復帰**: 上限到達（`gave_up`）のまま案内どおり手で繋ぎ直したペインは、引き取りが「既に追跡している」で降りるため見張りが再開せず、**次の切断で撃たない**ペインが残っていた。検知が ssh を見つけたら失敗表示を生きている事実で置き換える。④**撃たないときに無言にしない**: 切断を拾って繋ぎ直さないとき（接続の証拠が無い / 自動再接続が無効 / 待っても変わらない理由）は、理由と次の一手を**共有の通知欄 1 行 + persist.log**へ出す（#1399 系の `notify_ui_failure` の 1 実装・`NoticeArea::SshPane`）。⑤**対象かどうかを CLI / MCP 1:1 で読める**: `ssh_connect.reconnect`（`state` / `armed` / `reason` / `source`）を `tako list` / `read` に載せる。**普通に繋がっているペインでも返す**（FR-3.24.4 は `Connected` で応答を捨てていたため、切れる前に「守られているか」を読む手段が無かった = 報告が遅れた原因）。ただし進行状況（`attempt` / `retry_in_secs`）は従来どおり異常時だけ。`source` は `opened` / `restored` / `detected` で、発火しない報告が来たときに**どの入口が抜けたか**を実測抜きで切り分けられる。同一バイナリで①〜⑤を旧挙動へ戻す A/B は `TAKO_1446_LEGACY=1`（`TAKO_1040_LEGACY` とは独立）（✅ 2026-09-14、#1446） | S |
| FR-3.24.5 | **「リモートからフォルダを開く」を VSCode Remote 風にする**（#1041）。①**明示的に開いたリモートフォルダはツリーの先頭**（ローカルルートより前）へ出す。#976 が「ローカルの後ろ」へ寄せたのは**自動検知**（ペインの `ssh` から勝手に増える分）が主対象だったので、**ユーザーが選んで開いた分だけ**を主作業対象として前へ戻す。自動検知ぶん（FR-3.24.3）とローカルの並びは**不変**。経路は `RemoteOrigin`（`explicit` / `auto`）として `RemoteFolder` に持ち、layout.json へも保存する（**経路を持たない旧ファイルは `auto`** = 従来の並びのまま。`explicit` へ倒すと更新後の初回起動で自動検知ぶんまで先頭へ跳ねる）。自動検知で載っていたフォルダをユーザーが明示的に開き直したら `explicit` へ**格上げ**する（逆へは落とさない）。**並び規則の正本は `tako_core::sidebar::remote_root_order` の 1 実装**で、GUI（`FileTree::build_rows`）と CLI / MCP（`remote-folder list`）が同じ関数を通る = 画面の並びと応答の並びが食い違わない。`list` の各行は `origin`（explicit / auto）と `placement`（leading / trailing）を返す。②**フォルダを開いたら同じタブへターミナルも用意する**（SSH 済み + そのフォルダへ `cd` 済み）。**既定 ON**（#322「既定を賢く」）で、`open` の応答 `terminal`（`connected` / `reason` / `pane` / `note`）から結果が読める。同じタブに**そのホストへ繋がっている生きたペイン**があれば作らない（材料は #1010 の接続状態と #976 の `ssh` 検知の両方。`Failed` / `GaveUp` は数えない = 前の試行が死んでいるペインを理由に開き直しを断らない）。**開き先は常に新しいペイン**（`target=split`）で、#1006 の「既存ペインを SSH 化」は自動経路では採らない: 自動経路は対象ペインを選ぶユーザーの意思を持たず、素のシェルに**打ちかけの行**が残っていても見分ける手段が無い（プロンプトの終端は OSC 133 依存で器つきでは取れない）ので、#640 の送達フローが `<打ちかけ> ssh <host>` を実行しうる。ペインの SSH 化は右クリック / `--target pane` の**明示操作**としてそのまま使える。**ペインを作れなくてもフォルダは開いたまま残す**（理由は応答と通知に出す）。抑止は `tako remote-folder open <host> [path] --no-terminal` / MCP の `terminal: false`（新しいツールは増やさない = 既存 `open` の応答と 1 パラメータで表す）。**復元時には繋がない**（起動のたびにペインが増えるのを避ける。復元されたペインが既にssh を持っていることもある）。同一バイナリで #1041 の前へ戻す A/B は `TAKO_1041_LEGACY=1`（並びを全部後ろへ + 自動接続を切る）（✅ 2026-09-01、#1041） | S |
| FR-3.2 | ファイルクリックでペイン分割し、コードプレビュー（シンタックスハイライト + 行番号）。プレビューペインは**タブ内で 1 枚を再利用**（VSCode のプレビュータブ相当）し、PTY は起動しない（✅ 2026-06-13。実装メモは下記）。**ライトテーマでは syntect（`base16-eighties.dark` 固定）の構文色を描画時に輝度クランプして可読性を確保する**: 変換は `Theme::adapt_syntax_color`（ライトのみ相対輝度を `SYNTAX_LIGHT_MAX_LUMINANCE` = 0.12 まで落とし、チャンネル比 = 色相は保つ）1 本で、FR-3.3.1 の Markdown コードブロックと**同一経路**にして見た目を揃える。描画時変換なので `tako theme` の切替はハイライトの作り直し無しに即時反映され、ダークの色は 1 ビットも変えない（✅ 2026-07-30、#669）。**`.ino`（Arduino のスケッチ）は C++ の構文で塗る**: two-face に Arduino の構文は無く、VS Code 同梱の cpp 言語定義も `.ino` を C++ に含める。Markdown の ```` ```ino ```` / ```` ```arduino ```` も C++、ファイルツリーのアイコンも C++。寄せるのは色だけで、言語サーバは別（clangd へ渡すと正しいスケッチに嘘のエラーが出る = #1951）。構文は 1 本も増えない（#815。`.ino` の表示中だけ `.cpp` と同額が載り、同じ猶予で手放す）。寄せ先は言語の同定の正本 `tako_core::file_type` の `arduino` 行（`syntax` = C++・`fence_tokens` = ino / arduino。#1948 で `preview.rs` の補いから移した）。A/B は `TAKO_1949_LEGACY=1`（Plain Text へ戻る）・実測は visual-test `TAKO_VISUAL_ONLY=ino-highlight`（✅ 2026-10-09、#1949）。**既存の構文への別名で色の穴を埋める**: `.h` は C++（VS Code と同じ。two-face は Objective-C が勝っていた）、ただし本文に `#import` / `@interface` 等がある Objective-C のヘッダは Objective-C のまま。`.m` は表の行を持たず（`file_type` は null）、色だけを純粋関数 `file_type::syntax_by_content` が本文の先頭 200 行の手掛かりで決める（Objective-C の印 → Objective-C を先に・行頭の `%` / `function` / `classdef` → MATLAB・無ければ今のまま Objective-C。編集中は打鍵ごとに引き直す）。`.pde` は Java（`processing` の行。`java` の行にしない = 言語サーバを渡さない）・`.jsonc` は JSON・`.json5` は JavaScript（JSON の構文は JSON5 にしか無い書き方を `invalid` にし、テーマが地の色で描くのでダークで字が消える。GitHub の Linguist も `source.js`）。Markdown の ```` ```pde ```` / ```` ```processing ```` / ```` ```jsonc ```` / ```` ```json5 ```` も同じ。`.S` / `.s` は `x86_64 Assembly` へ寄せない（ARM の `@` コメントが `invalid` で消え・ARM64 の `//` がコメントにならない）。構文は 1 本も増えない（213 のまま。#815）。応答の `syntax` で読める。A/B は `TAKO_1953_LEGACY=1`・実測は `scripts/test-syntax-alias-1953.sh`（CLI / MCP の応答と visual-test `syntax-alias`）。判断の根拠は `.agent/plans/2026-10-language-coverage.md` §6.1（✅ 2026-10-14、#1953）。**two-face に構文が無い PowerShell を足す**: two-face は fancy-regex で組めない構文を外していて（bat 同梱の PowerShell はサブルーチン呼び出しを使う）、`.ps1` は Code Runner の既定が両 OS にあるのに色が無かった。fancy-regex で全部の正規表現が組める Sublime Text 公式パッケージの定義（MIT。上流とバイト単位で同一のまま `crates/tako-app/syntaxes/PowerShell/` に同梱し、告知は `THIRD-PARTY-NOTICES.md`）を、ビルド時に `build.rs` が構文定義へ読み（正規表現が組めなければビルドを落とす・`first_line_match` の変数を展開する・上流の別ファイルを名指す埋め込みの C# を two-face の C# へ向ける）、起動時に `syntax_extra` が既存の構文セットの**末尾へ継ぎ足す**（既存の 213 構文は 1 バイトも変えない。同じ並びの身代わりで syntect の builder に参照を結ばせ、全体を作り直した結果とバイト単位で一致することを単体が見る）。`.ps1` / `.psm1` / `.psd1` と 1 行目が `#!… pwsh` のスクリプトが PowerShell で塗られ、`file_type` の `powershell` 行も 3 つの拡張子を持つ。構文は 214 になり、器は +15.2 KB（#815）・起動時の継ぎ足しは #1917 の構築と同じ専用スレッドで 1 回（release 9〜11 ms）。YAML のパーサはビルドスクリプトにだけ入り配布バイナリへは入らない。`.astro` は上流の構文が Sublime Text 4 の継承で書かれていて syntect が読めず、既存の構文への別名もフロントマターを読み違えるので足さない。応答の `syntax` で読める。A/B は `TAKO_1954_LEGACY=1`・実測は `scripts/test-extra-syntax-1954.sh`（CLI / MCP の応答と visual-test `extra-syntax`）。比較と判断の根拠は `.agent/plans/2026-10-language-coverage.md` §6.2（✅ 2026-10-16、#1954）。**言語の同定の正本は `tako_core::file_type` の 1 枚の表**（1 形式 = 1 行。ID・拡張子・ファイル名・two-face の構文名・コードブロックの別名・プレビューの種別・画像の形式・HTTP の型・インデントの既定・Enter の規則）で、引く順は**ファイル名 → 拡張子**（どちらも大文字小文字を問わない）。色分け（表が構文を名指せばそれ、無ければ two-face の表 = ファイル名 → 拡張子 → 1 行目）・Markdown のコードブロック（表の別名 → two-face）・プレビューの種別と HTTP の型（`open_plan`）・画像の形式と Markdown 判定（app）・インデントと Enter（`text_edit`）がすべてこれを引く。表に行があるのは「既定（code・two-face 任せ・4 スペース・括弧で深く）から外れる形式」と「挙動の表（言語サーバ・実行の既定・実行のプロジェクト）が名指す形式」だけで、全 214 構文は並べない。挙動の表は分けたまま（#1726 設計 §9.2）で、番犬 `issue1948_file_type_watchdog` が「挙動の表の拡張子・ファイル名は `file_type` に行がある」と「`preview.rs` / `text_edit.rs` / `open_plan.rs` に拡張子の文字列リテラルの分岐を書かない」を file:line で強制する。**`tako open` の結果と `tako edit` の応答（`preview_edit_reply` の 1 実装 = 全サブコマンド）に `file_type`（形式の ID。表に無ければ null）と `syntax`（その表示を塗った構文名。背景の塗りがまだ戻っていなければ塗りと同じ解決をその場で引く。コード表示でなければ null）**を載せ、CLI と MCP は同じ dispatch の 1 本（ツールは増やさない）。寄せる前後で two-face の全拡張子と設計書 §2 の 45 形式の「構文・プレビュー種別・HTTP の型・画像の形式・インデント・Enter」が全数一致することを単体（`preview/file_type_parity_tests.rs`。寄せる前の判定の写しと比べる）が縛る。意図した差分は 2 つ: `CMakeLists.txt` の Enter を「継承だけ」（拡張子 `txt` 扱い）から「括弧で深く」へ・特殊なファイル名の色の補い（`.npmrc` / `Justfile` 等）を大文字小文字違いの綴りにも当てる（two-face のファイル名の照合と同じ）。ファイルツリーのアイコンも拡張子を小文字にして引く（`Sketch.INO` が C++ のアイコン）。設計は `.agent/plans/2026-10-language-coverage.md` §4（✅ 2026-10-13、#1948） | M |
| FR-3.3 | Markdown レンダリングプレビュー（見出し / リスト / コードブロック / 引用 / 罫線 / インライン強調 / **GFM テーブル**）。`.md` / `.markdown` は既定でレンダリング表示し、プレビューのタイトルバーの**目アイコントグル**で「コードとして表示」⇔「md レンダリング表示」をワンクリック切替できる。**トグル状態（mode）は CLI / MCP からも操作・取得できる**（`tako open --mode` / `tako_open_file` の mode / list の `preview.mode`。開発不変条件。✅ 2026-06-13。表示品質は FR-3.3.1 ✅ 2026-07-30、#656） | S |
| FR-3.3.1 | **Markdown の表示品質**（#656）。①**GFM テーブルは表形式**で描く: 外枠 + セル間罫線 + セルパディング + ヘッダ行の強調（`surface_highlight` の帯 + 太字）+ 偶数行のゼブラ + 列アライメント（`:---` / `:---:` / `---:`）。列幅は内容の表示幅（全角 = 2）から比で配り、**狭いペインでも横に溢れず**セル内で折り返す。行ごとにセル数が違う壊れた表は列数をヘッダへ正規化する。②要素の描き分け: H1〜H6 はサイズ・太さ・色の 3 点でレベル差を付け H1 / H2 は下罫線、コードブロックは `mantle` のパネル + 枠 + 言語指定フェンスの syntect ハイライト（**ライトテーマでは輝度を落として可読性を確保**）、インラインコードは `surface_highlight` の背景 + `peach`、リンクは `accent` + 下線、引用は左ボーダー + 淡い面（ネスト対応。引用内のリスト・コードも同じ帯に入る）、リストはネスト段ごとにマーカー字形（● ○ ■）を変え番号付きは `accent_muted`、タスクリストは**図形のチェックボックス**（絵文字禁止）、水平線は `border_heavy`。③**選択・コピーは表のセル単位**で成立する（セル 1 つ = 選択 1 行。同じ y 帯に並ぶセルは x でヒットテストを解決する）。色はすべて Theme 経由（FR-4）で dark / light 両対応 | S |
| FR-3.3.2 | **Markdown のリンク ⌘+クリックとコードブロックのコピー**（#680）。①`[text](url)` のリンクは ⌘+ホバーで下線を実線化 + `accent` 背景 + ポインタカーソル、⌘+クリックで **OS 既定ブラウザ**を開く（ターミナル内 URL = #146 / PDF 内リンク = #315 と同じ UX）。**開くのは http / https だけ**（`tako_core::url_guard::check_browser_url` が正。`md_links::browser_url` はその再公開。`javascript:` / `data:` / `file:` / 相対パス / アンカーは一覧には出るが開かない）。**この規則は Markdown 専用ではない**: **PDF のリンク注釈**（#271 / #315）と**ポート検知の提案チップ**（FR-2.4.3）も同じ 1 実装を通す（#1376。PDF の注釈は中身が任意の文字列で、OS の既定ハンドラは `file:` URL・UNC パス・ローカルの実行ファイルパスも開くため、素通りさせるとクリック 1 回で任意のプログラムが起動する）。経路側を落としても穴にならないよう**境界 B8（`os_integration::open_url`）にも保険**を置き、そこは http / https に加えて tako 自身が組み立てる OS 固有スキーム（`url_guard::OS_HANDLER_SCHEMES` = `x-apple.systempreferences:`）だけを通す。**弾いたリンクは黙って捨てず**通知へ出し、診断へはリンク文字列ではなく理由の分類だけを残す。当たり判定は「文字の上にあるか」= `TextLayout::index_for_position` の `Ok` のみを採り、⌘ 無しのクリック・ドラッグは従来のテキスト選択のまま。②コードブロックは右上に**コピーボタン**（常時表示だが待機中は SVG アイコンのみ・淡色の控えめな表示。ホバーとコピー直後だけ濃くする）を持ち、押すと**装飾なしの全文**（インデント・空行保持）がクリップボードへ入り一定時間チェックアイコン + 日英ラベルの成功表示になる。**`opacity(0)` + `group_hover` の「ホバーで初めて現れる」方式は採らない**（実機でホバー復帰が発火せずボタンが一度も見えないことを実測。#680）。③リンク索引の正は `md_document_links` 1 本（render のホバーと CLI / MCP の一覧が同じ並びを共有する）。④CLI / MCP は `tako preview-link-list`（応答の `kind` が `markdown` / `pdf`）・`tako preview-follow-link`・`tako preview-copy-code` / `tako_preview_copy_code`（開発不変条件） | S |
| FR-3.3.3 | **Markdown の編集と描画の往復**（✅ 2026-10-02、#1661）。①描画表示の md を編集して抜けると**元の表示モード（markdown）へ戻り、目次を作り直す**。組むのは編集セッションの**本文**から（ディスクではない）なので、未保存のまま抜けても書いたとおりに描かれ、書き換えた見出しが目次に出る。抜けた後の `tako edit save` / `tako edit reload` （外部変更の競合 FR-3.5 / #1659 の帯の「読み直す」を含む）でも描画のまま。コード表示から編集した md・コードのファイルは今までどおり抜けても code。表示をエディタの行へ落とすかの判定は `refresh_preview_from_editor` の 1 か所（`EditState::shows_editor_lines` = 編集中か検索欄を開いているあいだだけ）。②小さい文書（編集の全文の塗りと同じ線引き = 5,000 行以下）はその場で組み、大きい文書は background で組む（その間は読み込み中の表示・GUI は応答する）。③描画は**編集中に見ていた節の見出しから**続ける（キャレットが見えていればその行、見えていなければ器の先頭可視行の節）。④**編集中も目次が使える**: 目次ボタンが出て、項目のジャンプ先は原文の行（`target.kind = source_line`）。項目を押すとその見出しの行へ飛びキャレットも置く（FR-3.27 の着地と同じ）。項目の並びとタイトルは描画の目次と同じ規則（同じパーサ設定・BOM の剥がし方・タイトルの組み方）なので k 番目どうしが同じ見出しを指す。作るのは目次パネルを開いているときと CLI / MCP が読むときだけ（打鍵ごとには解かない。実測 release・同じパーサとオプションで見出しだけを走査: 5,000 行 0.58ms / 10 万行 13.8ms）。CLI / MCP は既存の `tako preview-outline` / `tako_preview_outline` がそのまま編集中も効く（ツールは増やさない）。⑤レイアウト（`layout.json`）へは編集中の表示（code）ではなく抜けた先のモードを書く（スキーマは不変 = 値の選び方だけ）。**並置（編集しながら右に描画）**は入れていない: 1 ペイン = 1 表示の前提の状態（md の仮想リスト・リンクの当たり判定・選択がペイン ID で引かれる）を割るか、本文に追従する連動ペインを足す必要があり小さく入らない。保存で追従する並置は既存の `tako open <file> --right` + 自動保存 + ライブリロードで既にできる（本文に追従する並置は #1872）。A/B は `TAKO_1661_LEGACY=1`。⑥**閲覧中の ⌘F 検索を閉じたら描画へ戻す**（✅ 2026-10-08、#1873）: 検索欄を開いて探すあいだはエディタの行（code）の上にヒットを描く（意図どおり）。閉じる口（Escape・⌘F のトグル。Windows は Ctrl+Shift+F）は `close_preview_search_bar` の 1 実装で、閲覧中に描画から開いた検索なら（`EditState::search_close_resumes_rendered`）①と同じ `restore_rendered_preview` で描画へ戻して目次を作り直し（ヒットのあった節の見出しから描く）、未保存の変更が無ければ編集セッションも畳む（目アイコンが戻る）。描き直すのは表示が実際に code へ落ちていたときだけ（探さずに開いて閉じただけでは描き直さない = 全文を解き直さない）。編集中に開いた検索は閉じても編集を保つ。コードのファイル・PDF は今までどおり。CLI / MCP は `tako edit search --open` / `--close` + MCP `tako_preview_search` の `visible`（dispatch `PreviewSearch` の引数を足しただけでツールは増やさない）で開閉し、応答の `search.visible` と `mode` で読める（開くのは探す前・閉じるのは探した後。開閉だけなら移動しない）。開いた欄の上で CLI / MCP が探すと打鍵と同じく表示を組む。描画のまま検索する（ヒットを md の行テキストの上に描く）のは入れていない。A/B は `TAKO_1873_LEGACY=1` | S |
| FR-3.4 | PDF プレビュー。PDFKit テキストレイヤによる選択・コピー、ウィンドウの device scale と表示幅に合わせた background 再ラスタライズに対応する。行間・ページ余白から選択を開始しない（#231） | C |
| FR-3.5 | プレビューペインでの軽い編集と保存。タイトルバーで編集モードを切替え、文字入力・削除・改行・カーソル移動・選択・貼り付け・⌘S 保存・dirty 表示に対応する。UTF-8 テキストのみ対象。保存時に読み込み後の外部変更を検知したら上書きしない。**完全自動保存（既定 ON、500ms デバウンス）、⌘Z/⇧⌘Z の undo/redo、⌘F 検索（インクリメンタル・ラップ）+ 置換（1 件/全置換）を追加（#195）**。dispatch `PreviewEdit` / `PreviewApply` / `PreviewSave` / `PreviewUndo` / `PreviewRedo` / `PreviewSearch` / `PreviewReplace` / `PreviewAutosave` + CLI `tako edit` + MCP 8 ツールで同等操作を公開（✅ 2026-07-14、#195）。**編集中の再ハイライトは差分**（NFR-8。1 打鍵で塗り直すのは変わった行と状態が収束するまでの後続行だけで、残りは前回塗った行を使い回す。release 実測 4,666 行 344.6ms → 0.56ms。仕組みは `architecture.md`「編集中の差分ハイライト」。✅ 2026-09-23、#1648）。**undo 履歴は差分**（#1651。積むのは「置き換えた範囲 + 置換前後の文字列」で、全文のスナップショットは持たない。release 実測: 1 MB のバッファへ 1000 打鍵で RSS 増分 1022.0MB → 1.1MB）。**連続タイプは 1 塊**なので `hello` は undo 1 回で消える。まとまる条件は「同じ種類・位置が連なる・前回から 500ms 以内・改行をまたがない」で、**カーソル移動・改行・種別の変更・時間切れ・保存・undo / redo** が塊を切る。**上限は「操作数 1000」と「履歴 8MiB」の先に効いたほう**（1 操作で本文 2 本ぶんを積む編集 = 全文差し替え・全置換があるので操作数だけでは上限にならない。画像は FR-3.17 で512MiB の予算を持っているのにテキストだけ無予算だった）。1 回の編集が単独で予算を超えても直前の 1 件は必ず残す。粒度が変わっても口は同じ（`PreviewUndo` / `PreviewRedo` + `tako edit undo\|redo` + MCP `tako_preview_undo` / `_redo`）で、1 回の呼び出しが戻す量が「1 文字」から「1 塊」になる。仕組みは `architecture.md`「undo 履歴は差分で持つ」。✅ 2026-09-23、#1651）。**行・桁で指す範囲編集とカーソル操作、および文書の版**を追加（全文置換しか無かったので 1 行直すのに本文を丸ごと IPC で送っていた。dispatch `PreviewEditRange` / `PreviewCursor` + CLI `tako edit replace-range` / `cursor` + MCP `tako_preview_edit_range` / `tako_preview_cursor`。座標と拒否の契約は下の実装メモ。✅ 2026-09-24、#1658）。**undo 履歴は差分**（#1651。積むのは「置き換えた範囲 + 置換前後の文字列」で、全文のスナップショットは持たない。release 実測: 1 MB のバッファへ 1000 打鍵で RSS 増分 1022.0MB → 1.1MB）。**連続タイプは 1 塊**なので `hello` は undo 1 回で消える。まとまる条件は「同じ種類・位置が連なる・前回から 500ms 以内・改行をまたがない」で、**カーソル移動・改行・種別の変更・時間切れ・保存・undo / redo** が塊を切る。**上限は「操作数 1000」と「履歴 8MiB」の先に効いたほう**（1 操作で本文 2 本ぶんを積む編集 = 全文差し替え・全置換があるので操作数だけでは上限にならない。画像は FR-3.17 で512MiB の予算を持っているのにテキストだけ無予算だった）。1 回の編集が単独で予算を超えても直前の 1 件は必ず残す。粒度が変わっても口は同じ（`PreviewUndo` / `PreviewRedo` + `tako edit undo\|redo` + MCP `tako_preview_undo` / `_redo`）で、1 回の呼び出しが戻す量が「1 文字」から「1 塊」になる。仕組みは `architecture.md`「undo 履歴は差分で持つ」。✅ 2026-09-23、#1651）。**編集カーソルは常に可視範囲へ追従する**（打鍵・矢印・⌘F のヒットへ飛ぶ・undo / redo・貼り付け・IME の変換開始のいずれの後も、カーソル行が上下 3 行の余白つきで見える。視野が狭いときは余白を詰め、文書の端では余白より可視化を優先する。**編集開始時のキャレットは「見ている行」へ置く**ので、スクロールした状態で編集を始めても器が先頭へ戻らない）。判断は `tako_core::editor_scroll` の純関数 1 実装が持ち、器（#821 の仮想リストと `TAKO_821_NO_VIRTUAL_LIST=1` の div スクロール）の両方を論理位置で動かす。連鎖して消えていた **IME の未確定下線は、行のレイアウトがまだ無いフレームでもプレビュー本文の中にアンカーを残す**（ターミナル枝へ落とさない）。効きは編集系の応答（`preview_edit_reply` の 1 実装）と `tako_preview_search` に乗る `viewport`（`first_visible_line` / `last_visible_line` / `visible_lines` / `visible`。カーソルの位置自体は #1658 の `document.cursor` が正本）で GUI の外から読める。A/B は `TAKO_1649_LEGACY=1`。仕組みは `architecture.md`「編集カーソルの追従スクロール」（✅ 2026-09-23、#1649）。**可視行数は描いた行の実寸から数える 1 実装**（追従の余白とページ移動の歩幅が別々に数えていた。追従は器の高さを `theme.line_height` = 17px で割っていたが、コード行は実測 21px + 上余白 14px で、661px の器で実矩形 30 行を 37 行と数え、↓ で進むとカーソルが画面の外へ出た。文字サイズの最大・最小 / 折り返しのある行でも実矩形と一致する。✅ 2026-09-26、#1741）。**修飾キー付きの移動・削除**を追加（入口が ⌥ / ⌃ / ⌘ 付きの打鍵を全部捨てていた。単語移動 ⌥←→ / Ctrl+←→・単語削除 ⌥⌫⌦ / Ctrl+Backspace / Delete・文書端 ⌘↑↓ / Ctrl+Home / End・行頭 / 行末 ⌘←→・行頭 / 行末まで削除 ⌘⌫⌦・Page Up / Down・⇧ 付きの選択拡張・smart Home・上下移動の桁の記憶。打鍵の意味は `tako_core::platform::editor_keys` の 1 枚の表が両 OS の列で持つ。dispatch `PreviewMove` / `PreviewDelete` + CLI `tako edit move` / `delete` + MCP `tako_preview_move` / `tako_preview_delete` が GUI の打鍵と同じ口を通る。契約は下の実装メモ。✅ 2026-09-26、#1652）。**Tab / ⇧Tab のインデントと Enter のインデント継承**を追加（Tab が打鍵表に無く落ちていた・改行すると次行が桁 0 だった。単位は既存行から推定、開き括弧の直後の Enter は 1 段深く。括弧の自動閉じは #1654 では入れなかった = 理由は下の実装メモ（#1759 で飛び越しと対・設定つきで入れた）。dispatch `PreviewEditCommand` + CLI `tako edit indent` / `outdent` / `newline` + MCP `tako_preview_edit` の `command`（ツールは増やさない）。✅ 2026-09-26、#1654）。**キー操作の残り 3 点**（上下移動の桁を文字数ではなく表示幅で覚える・選択中の素の ←→ で選択の端へ畳む・macOS の ⌃A / ⌃E / ⌃K。新しい操作は足さず、既存の `PreviewMove` / `PreviewDelete` + `tako edit move` / `delete` + MCP `tako_preview_move` / `tako_preview_delete` で GUI の打鍵と同じ結果になる。契約は下の実装メモ。✅ 2026-09-26、#1742）。**検索・置換は大文字小文字を区別するのが既定**（`find_all` が常に小文字化していたので、`replace_all("value"→"item")` が `let value = Value::new();` を `let item = item::new();` にした = コード編集ではデータ破壊。**区別の有無と単語単位**を検索欄の SVG トグル 2 つ + dispatch `PreviewSearch` / `PreviewReplace` の `case_sensitive`（既定 true）/ `whole_word`（既定 false）+ CLI `tako edit search\|replace` の `-i` / `-w` + MCP `tako_preview_search` / `tako_preview_replace` の同名引数で 1:1。省略したときの決め方と小文字写しの扱いは下の実装メモ。✅ 2026-09-27、#1653）。**置換のあとのカーソルは最後の置換の末尾**（1 件置換は置き換えた箇所の末尾・全置換は最後のヒットを置き換えた末尾で、選択は外れる。0 件なら本文もカーソルも動かない。undo 1 回で本文とカーソル・選択が置換前へ戻る。旧実装の全置換は編集前の位置を置換後の長さで切り詰めていたので、先頭付近を置換すると置き場がずれた。A/B は `TAKO_1674_LEGACY=1`。✅ 2026-10-17、#1674）。**検索でヒットへ飛ぶと選択の写し（ハイライトと次の打鍵の起点）もヒットへ揃う**（GUI の検索欄と CLI / MCP の `PreviewSearch` が同じ 1 実装 = `preview::search_jump` を通る。旧実装は検索欄を開かない CLI / MCP の検索でキャレットだけがヒットへ動き、前の選択のハイライトが残った。A/B は `TAKO_1701_LEGACY=1`。✅ 2026-10-17、#1701）。**編集できる大きさを 10 MB / 10 万行へ引き上げ**（旧 1 MB / 5,000 行では tako 自身の `main.rs` = 8 万行超が編集できなかった。値の正本は `tako_core::preview_limit`。**上限を超えたら理由と値を必ず言う**: 画面のフッタ・`tako edit start` のエラー・`tako edit` 応答の `limit`（`reason` = `bytes` / `lines`・実寸か数えた行数・`max_bytes` / `max_lines`・`message`）が同じ 1 実装から組まれ、CLI と MCP で同じ文面。大きい文書（5,000 行超）は編集開始時の全文ハイライトを background へ出し（それまでは平文で、打鍵は即時）、1 回の差分の塗りを 250 行で打ち切る。応答の `document.highlight_pending` で塗り待ちかを読める（読み取り表示の塗りが走っている間は応答に `highlighting: true`。塗りの間は App Nap に間引かせない = #1916）。A/B は `TAKO_1660_SYNC_SEED=1`。仕組みと release 実測は `architecture.md`「大きいファイルの編集」。✅ 2026-09-27、#1660）。**外部変更を検知した後の逃げ道**を追加（編集中のファイルがディスク側で変わると保存が永久に断られ、`--force` も読み直しも差分も無く、未保存なので別のファイルも開けない = ペインを閉じる以外に抜けられなかった。自動保存が 500ms ごとに同じ競合を出し続けた）。2 択（**自分の変更で上書き** / **ディスクから読み直す** = undo 1 回で自分の変更へ戻せる）と**差分**（ディスク → 編集中の unified diff）を、GUI の帯・dispatch `PreviewSave { force }` / `PreviewRevert` / `PreviewDiff`・CLI `tako edit save --force` / `reload` / `diff`・MCP `tako_preview_save` の `action`（`overwrite` / `reload` / `diff`。ツールは増やさない）から選べる。**競合中は自動保存を止め、知らせるのは新しく分かったときだけ**。未編集ならディスクへ黙って追従し、未保存のプレビューへ別のファイルを開くと差し替えずに分割して開く。契約は下の実装メモ。A/B は `TAKO_1659_LEGACY=1`（✅ 2026-09-27、#1659）。**行の操作**を追加（複製 ⇧⌥↑↓・移動 ⌥↑↓・行ごとのインデント ⌘] ⌘[・行コメントの付け外し ⌘/。Windows は Ctrl 段。どれも undo 1 回で戻り、CRLF のファイルに LF を混ぜない。コメント記号は表の 1 行で決まり、記号の無い形式は理由を返して本文を触らない。dispatch `PreviewEditCommand` + CLI `tako edit line <操作>` + MCP `tako_preview_edit` の `command`（`line-<操作>`。ツールは増やさない）。契約は下の実装メモ。✅ 2026-10-13、#1692）。**対応する括弧の強調**を追加（カーソルの直後 / 直前の括弧と対の括弧の 2 文字に背景。文字列とコメントの中の括弧は数えず、記号は表の `comment` 列と `lexical` 列で決まる。選択中・括弧に接しない・対が無いときは出ない。位置は編集系の応答の `document.bracket_match` で CLI / MCP から読める（ツールは増やさない）。契約は下の実装メモ。✅ 2026-10-13、#1691）。**Enter の続きと括弧の自動閉じ**を追加（①ブロックを抜ける文 = Python の `return` / `pass` / `break` / `continue` / `raise` の行の Enter で 1 段浅く ②Markdown のリストの行の Enter で印を引き継ぐ（番号は 1 つ進め、チェックボックスは空）・中身が空の項目では印を消して抜ける ③開き括弧で閉じ括弧も入れ、**自分で入れた**閉じ括弧の手前の閉じ括弧は飛び越し、`(\|)` の ⌫ は対で消す。文字列・コメントの中と文書（Markdown・プレーンテキスト）では閉じない。規則は言語の同定の正本 `file_type` の列 `dedent_after` / `list_continuation` / `auto_close` で決まり、入口に言語名は無い。③は settings.json の `editor_auto_close`（既定 ON・serde default で旧ファイルはそのまま読める）+ 設定画面 + CLI `tako edit auto-close [on\|off]` + MCP `tako_preview_edit` の `auto_close` で切れる。打鍵 1 文字は CLI `tako edit type <文字>` / MCP `tako_preview_edit` の `command: "type"` + `text` が GUI の文字入力と同じ口（`TextBuffer::type_char`）を通る（ツールは増やさない）。応答の `document.auto_close` で効いているかを読める。契約は下の実装メモ。A/B は `TAKO_1759_LEGACY=1`。✅ 2026-10-15、#1759） | S |
| FR-3.6 | 右サイドバー: git graph（コミットグラフ・ブランチ可視化、cwd のリポジトリに連動。✅ 2026-06-14） | S |
| FR-3.25 | **git タブのブランチ操作**: ブランチ一覧の行クリックでチェックアウト、行ホバーの「マージ」で取り込み、セクション見出しの「新規」でブランチ作成。現在ブランチを明示し、リモート追跡ブランチはローカルと分けて表示する（リモート行のクリックは detached HEAD ではなく同名ローカル追跡ブランチを作る）。**破壊的になり得る操作（未コミット変更ありの切替・マージ）は実行前に「何が起きるか」をカードで提示し、承諾するまで実行しない**（黙って stash / 強制切替しない）。提示は切替なら「持ち越される変更」と「切替を妨げる変更」を分け、マージなら種別・取り込みコミット数・変更ファイル数・`git merge-tree` による**コンフリクト予測**を出す（予測不能な git ではその旨を明示し「コンフリクトなし」と誤解させない）。git が拒否すると分かっている操作は実行ボタン自体を出さない。dispatch `GitCheckout` / `GitBranchCreate` / `GitMerge` + CLI `tako git checkout/branch/merge`（既定は提示のみ・`--yes` で実行）+ MCP `tako_git_checkout` / `tako_git_branch_create` / `tako_git_merge`（`confirm` パラメータ）へ 1:1 公開する（✅ 2026-07-26、#496） | M |
| FR-3.26 | **コンフリクトカードと解消エージェント**: merge / rebase / cherry-pick / revert の進行中状態を git タブ上部の専用カードで提示する。進行中の操作・取り込み先/元（ours ← theirs）・未解決ファイル一覧（porcelain v2 の `u` レコード）・中止導線を含む。カードから **claude / codex / agy を選んでコンフリクト解消エージェントを同じタブに起動**でき、リポジトリパス・未解決ファイル一覧・マージ元/先ブランチ・「解消したら報告し、勝手に commit / push しない」制約を含むプロンプトを自動投入する。エージェント起動は既存 spawn 基盤（`orchestrator::agent` のコマンド構築 + 事前信頼 + PromptFlow）を再利用し、新系統を作らない。プロンプト文面はバイナリ埋め込みの雛形を `<data_dir>/orchestrator/conflict-resolver.md` で差し替えられる。dispatch `GitConflicts` / `GitMergeAbort` / `GitResolveAgent` + CLI `tako git conflicts/abort/resolve` + MCP `tako_git_conflicts` / `tako_git_merge_abort` / `tako_git_resolve_agent` へ 1:1 公開する（✅ 2026-07-26、#496） | M |
| FR-3.27 | **行・桁を指定してファイルを開く（定義ジャンプの着地点）**（#1676 / #1007 S0-b）: `OpenFile` が `line` / `column`（どちらも **1 始まり**）を受け、開くのと同時にその行を画面へ出す。CLI `tako open <file> --line L [--column C]` / MCP `tako_open_file` の `line` / `column` が**同じ dispatch** を通る（ツールは増やさない = 開発不変条件を既存経路で満たす）。**写像の決定（md モードの扱い）**: `line` が指すのは常に**原文の行**なので、`line` を渡された時点で表示種別は `code` へ倒す（`mode: markdown` を明示していても `line` が勝つ）。Markdown のレンダリング表示は **1 item = 1 ブロック**（#826）で原文の行が 1:1 で残らない（空行は消え、GFM テーブルは 1 ブロックに複数行が入る）ため、「行 N」をブロック番号へ読み替えると**別の場所へ着地する**。倒したあとは `item = line - 1` の 1:1 で、応答の `mode` が `code` になることで呼び出し側からも分かる。行を持たない種別（画像 / PDF / 動画）は**開く前に**エラー（PDF のページ送りは `PreviewView` の `page` が持つ別の操作）。**丸め**: 文書の行数を超える `line` は末尾行へ、行の文字数 + 1（行末の次）を超える `column` はそこへ丸め、応答の `clamped` で知らせる（定義ジャンプの相手が古い行番号でもファイルは開いて見せる）。0 以下は `line` / `column` とも**開く前に**エラー（負値は CLI の `i64` だけが見える境界）。**省略時は従来と完全に同じ**で、`line` / `column` は `skip_serializing_if` により wire にも現れない（旧い tako-cli と新しい tako-app が混ざっても JSON がバイト単位で変わらない）。**着地の機序**: 仮想リストの `ListState` は描画のときに作られるので、dispatch は「次の描画で 1 度だけ飛ぶ」予約（`preview_pending_reveal`）を置き、`consume_pending_reveal` が消化する。`ListState::scroll_to` は論理位置で飛ぶので**一度も描かれていない行へも届く**（#826 の実測）。`column` は受け取り・検査・丸め・応答までを担うが、**キャレットの描画は行わない**（カーソル追従は #1649 の担当）。応答は `line` / `column` / `item` / `total_lines` / `clamped`（✅ 2026-09-24、#1676） | M |
| FR-3.28 | **言語サーバ（LSP）の基盤**（#1678 / #1007 S1）: コードプレビューを**編集モードにしたときだけ**、拡張子に合う言語サーバを起こして握手し（`initialize` → `initialized`）、文書を同期する（`didOpen` / `didChange`（incremental）/ `didClose`）。**言語機能は出さない**（診断の描画・補完・定義ジャンプは #1679 以降のスライス）。サーバは検出表 `tako_core::lsp::servers::SERVERS` の 1 行で決まり（段階 1 = rust-analyzer / clangd / typescript-language-server / pyright。第 2 波 = texlab（`.tex`）/ tombi（`.toml`）/ sourcekit-lsp（`.swift`）/ dart（`.dart`）/ vscode-css-language-server（`.css` `.scss` `.less`）/ vscode-html-language-server（`.html` `.htm`）= #1956。Arduino = arduino-language-server（`.ino`）= #1951 / FR-3.46。候補の比較・採った順・見送った言語と理由は `.agent/plans/2026-10-language-coverage.md` §8）、**言語の追加は表への行追加だけ**（番犬 `issue1678_lsp_watchdog` が表以外への言語名の直書きを落とす）。実行ファイルは `platform::exe::find_with_timeout`（#1769。Code Runner の #1730 と同じ 1 実装。unix のログインシェルは `probe::output_with_timeout` の上限 = `probe_timeout` 既定 15 秒を通り、打ち切ったら「見つからない」ではなく `timed_out` と理由・次の一手を返す）→ 起動は `platform::child_cmd`（unix は `$SHELL -l -c 'exec …'`）で、`TAKO_LSP_BIN_<ID>` で差し替えられる。**解決の結果はキャッシュする**（#1769。`tako lsp servers` と起動が同じものを使い、2 回目以降の `servers` はログインシェルを起こさない = 各行の `cached`）。引き直すのは `restart`（名指しならその 1 つ）・PATH が変わりうる出来事（シェル統合が知らせた cwd の変化・コマンドの終わり = `tako_core::shell_activity`。シェル統合は PATH そのものを知らせない）・見つかったパスが実行できなくなったときだけで、キャッシュに無いものは並行して引く。位置は **UTF-16 だけを申告**し、UTF-8 ⇄ UTF-16 の変換は `tako_core::lsp::position` だけが持つ。**行は LSP の仕様どおり `\n` / `\r\n` / 単独の `\r` で数え、送る本文は単独の `\r` を `\n` に替える**（#1769。実測: rust-analyzer と clangd は問い合わせの位置を `\n` だけで数え、clangd の診断・pyright・TypeScript は仕様どおり = 替えないと単独 CR の後ろで行がずれる。1 バイト → 1 バイトなので位置は 1 対 1 のまま、`TextBuffer` と写しは替えない = 保存はバイト一致。限界（単独 CR を含むファイルだけ）: サーバがディスクから自分で読んだ開いていないファイルは替えられないので、単独 CR を含む飛び先は rust-analyzer に限って行がずれうる。rust-analyzer の flycheck（cargo check = rustc）の診断は rustc が `\n` だけで数えた行で届くので、単独 CR より後ろの flycheck の診断は行がずれる = 実測）。`didChange` の版は #1658 の `TextBuffer::version` をそのまま使い、中身は**サーバへ送った本文の写し**との差分（`tako_core::lsp::sync`）。状態は 未起動 / 起動中 / 稼働 / 停止中 / 止めた / 未導入 / 取得中（#1944）/ クラッシュ / 諦めた / 前提が未設定（#1951 = FR-3.46）の 10 個で、再起動は 3 回まで（1・2・4 秒）、最後の文書を閉じて 60 秒で shutdown → exit、要求のタイムアウトは 120 秒（`TAKO_LSP_REQUEST_TIMEOUT_SECS`）。**未導入は理由 + 次の一手（導入コマンド）を返す**。取れるサーバは開いた時点で tako の data dir へ取って起こす（#1944 = FR-3.41）。**隔離起動（`TAKO_ISOLATED`）では編集モードにしても起こさない**（#2048。検証の GUI で rust-analyzer がワークスペース全体の `cargo check` を走らせて CPU / メモリを汚さない）: 器は「止めた」に置き（利用者の stop と同じ遷移。理由は「隔離起動なので自動では起こさない」・次の一手は `tako lsp restart`）、起こすのは明示の `restart` だけ（問い合わせ・シェル統合の合図・基板の選択・`install` では起こさない）。画面は帯ではなくタイトルの控えめな 1 行（「<サーバ> 自動起動なし（隔離）」= 本文の高さを変えない）。`TAKO_LSP_AUTOSTART=1` で従来どおり起こす（LSP の検証スクリプトはこれを export する）・`=0` は隔離でなくても起こさない・`tako lsp status` の全体の `autostart` に出る。隔離でない起動の既定は変わらない。A/B は `TAKO_2048_LEGACY=1`。**同じファイルを複数のペインで編集しても 1 文書**（#1769。1 URI = 1 文書を持ち手 = 編集セッションで共有する、エディタで広く使われる形）: `didOpen` は最初の 1 つ、`didClose` は最後の 1 つが閉じたときだけ送り、どのペインの編集も同じ文書の版を進める（版は文書ごとに単調 = 2 つ目のペインのバッファの版が若くても下回らない）。持ち手ごとに最後に取り込んだ自分のバッファの版を覚え、**自分の本文が変わったペインだけが送る**（カーソル移動だけの同期で相手の編集を巻き戻さない）。ペインのバッファは別々なので、サーバが見るのは最後に編集したペインの本文で、診断はその座標で両方のペインへ出る（加わった直後のペインは表からその場で読む）。読み取り表示のペインは持ち手にならない。`tako lsp status` の各サーバの `views` が持ち手の数（問い合わせ中の一時的な持ち手を含む）。診断（`publishDiagnostics`）は**開いている文書のぶんだけ保持**し、閉じたら捨てる。GUI の終了で全サーバを止め、kill -9 で落ちてもサーバは stdin の EOF で終わる（孤児を残さない）。Windows は cmd.exe 経由の孫まで Job で止める（#2454 = FR-2.2.18）。アイドル中は 1 通も送らない（#772）。**診断ログに本文を出さない**: perf.log は要求の method 名・所要・結果の種別だけ、生の JSON-RPC は `TAKO_LSP_DIAG=1` のときだけ `<data_dir>/lsp/<id>.log`。**サーバが無くても編集の結果はバイト一致**（`scripts/test-lsp-1678.sh` の ⑦）。操作は dispatch `LspServer { action, name }` + CLI `tako lsp status / servers / restart / stop / logs / install` + MCP `tako_lsp_server`（action 引数の 1 ツール）の 1:1。A/B は `TAKO_1007_LEGACY=1`（LSP を丸ごと止める）。Windows は偽サーバの e2e が根拠で、実サーバとの握手が未実測なので MATRIX は Pending（#1007）。**初期化オプション（`initializationOptions`）は検出表の `init_options` 列が既定**（#2035。言語の追加 = 行追加だけを保つ）で、rust-analyzer は **`cargo.targetDir = true`** = 初回の build script と `cargo check` を `<target>/rust-analyzer` で走らせ、利用者の `cargo build` と build directory のロックを取り合わない（実測: 旧は `Blocking waiting for file lock` で rust-analyzer の初回の check が終わるまで待たされた・新は待たない。費用は build script と proc-macro の成果物が 2 重になること = tako 自身で約 0.71 GB。比べた案と実測は `.agent/plans/2026-09-lsp-s1.md` §29）。上書きは settings.json の `lsp_init_options`（サーバ ID → ドット区切りのキー → JSON の値）で、`tako lsp options [--name <ID>] [--set KEY=VALUE]`（値は JSON か文字列・`null` で tako の既定へ戻す・tako が起動していなくても読み書きできる）/ MCP `tako_lsp_server` の action=options（`set`）が書く。効くのは次にサーバを起こしたときから（動いているサーバは `running[].current` が偽）。実際に送った値は `tako lsp status` の各サーバの `init_options`。A/B は `TAKO_2035_LEGACY=1`（何も送らない） | M |
| FR-3.29 | **ジャンプ履歴（戻る / 進む）**（#1677 / #1007 S0-d）: 行を指定した `OpenFile`（FR-3.27 = CLI `tako open --line` / MCP `tako_open_file` の `line`。S3 の定義ジャンプ #1680 も同じ経路を通る）が、**飛ぶ前にいた場所**と**着地点**の 2 つを履歴へ積む。飛ぶ前の場所 = 差し替えられるプレビュー（再利用するとき）か、新しく生やすなら基準ペイン（それがプレビューなら）の「いま見ている行」（着地予約 > 編集カーソル > 可視先頭行。**`set_preview` の前に採る**）。行は code 表示だけが持ち、Markdown のレンダリング表示・画像・PDF は行なしで積んで、戻るときは行を指定せずに開く（表示種別を code へ倒さない）。飛ぶ前の場所を積まないと最初の 1 回目のジャンプから戻れない（着地点しか無い）。**積む契機はこの 1 つだけ（決定）**: 行を渡さない open（ファイルツリーで眺める・cmd+クリック）・目次ジャンプ・検索は積まない。眺めるたびに積むと履歴が「さっき見たファイル」で埋まり、戻るが「飛ぶ前の場所」を指さなくなる。行の指定はそれ自体が「ここへ飛ぶ」という意図の表明で、S3 もこの 1 か所を通るので契機を増やさずに済む。スタックは `tako_core::jump_history`（GPUI 非依存の純粋関数）: 項目の列 + 現在位置、戻ってから別の場所へ飛ぶと進む側を捨てる（ブラウザと同じ）、**連続する同じ場所（同じファイルの同じ行）は 1 つに畳む**（桁・ペインは新しいほうで上書き。同じファイル内の別の行は畳まない = 同じファイル内の定義ジャンプを潰さない）、上限 100 件（vim の jumplist と同じ。古い側から落とす）。**閉じたペインの扱い（決定 = 開き直す）**: ペインを閉じても項目は消さない（履歴の単位は「ファイルのどの行か」で、ペインは表示先に過ぎない）。戻る / 進むで閉じたペインの項目に当たったら、基準ペイン（呼び出し元。GUI のキーはフォーカス中のペイン）のタブのプレビューを使い回し、無ければ右に分割して開き直す（= `OpenFile` の既定の表示先）。開き直した先へ、同じ閉じたペインを指す項目をまとめて付け替える（次に当たったときペインを増やさない）。読み飛ばす案を採らないのは、tako がプレビューを差し替えて使い回す以上、閉じたペインの項目は日常的に生じ、読み飛ばすと戻る先が黙って消える = #1677 が直したい迷子そのものになるため。**消えたファイル**の項目は開き直せないので読み飛ばして履歴から捨て、応答 `dropped` で知らせる（残すと押すたびに同じ失敗を繰り返す）。戻る / 進むの着地は `OpenFile` と同じ実装（`dispatch::open_file`）を通り、そのときは積まない（`JumpRecord::Skip`。積むと戻った先で履歴を書き換える）。端・空の履歴では `moved: false`（失敗にしない）。**キー（衝突確認済み）**: macOS = ⌃- / ⌃⇧-（VS Code と同じ）。ctrl 付きの記号は GPUI macOS が `key_char` を None で届けるので PTY へ 1 バイトも送っておらず、奪っても端末の入力手段は減らない。⌃⇧- は shift が落ちてシフト後の字面で届く（US = `ctrl-_` / JIS = `ctrl-=`）ので 2 本張る（`ctrl-shift--` はどの配列でも一致しない）。Windows = Ctrl+Alt+← / →（JetBrains 系と同じ）。Windows の Ctrl+- は縮小・JIS の Ctrl+Shift+- は `ctrl-=` として拡大、Alt+← / → はペインのフォーカス移動に使っているため。キーの割当は tako-app の `keybindings::jump_bindings`、案内の表記は `platform::keys::jump_back` / `jump_forward` が正本で、番犬が両 OS ぶん突き合わせる。GUI のキーは着地したペインへフォーカスを移す（別タブならタブも）。CLI / MCP の既定は移さない（`--focus` / `focus`）。操作は dispatch `Jump { action, pane, focus }` + CLI `tako jump back / forward / list` + MCP `tako_jump`（action 引数の 1 ツール）の 1:1。**永続化しない**（GUI のメモリだけで、再起動で消える = 永続ファイルの形式は増えない）。A/B は `TAKO_1677_LEGACY=1`（行指定の open が何も積まない）（✅ 2026-09-26、#1677） | M |
| FR-3.30 | **言語サーバの診断**（#1679 / #1007 S2）: `publishDiagnostics`（**push 型だけ**。pull 型 `textDocument/diagnostic` のクライアント能力は申告しない = rust-analyzer が `diagnosticProvider` を持っていても push を続ける。flycheck = cargo check の診断は push でしか来ず、pull はクライアントが要求の間隔を持つ = タイマーが要るので「アイドル時は何も投げない」（#772）と衝突する）を受けた時点で、**サーバへ送った本文の写し**を使って tako の座標（0 起点の行 + 行内 UTF-8 バイト。変換は `tako_core::lsp::position::LineIndex` = 数え直し版と全位置で一致）へ写し、manager の URI ごとの表に保つ（1 文書 5,000 件で頭打ち・軽いものから捨てて `dropped`）。モデルは `tako_core::lsp::diagnostic`（重大度 4 段 / 範囲 / 出所 / コード）。**重大度の省略・範囲外はエラー扱い**（VS Code / Zed と同じ・見落とさない側）。UI へは **bounded 128 のキュー**で URI だけを運び、溢れたら「全部読み直す」印を立てる（`diagnostics_events` / `take_events_overflow`）。GUI は届いた分を 1 回にまとめて `EditState::diagnostics`（表の `Arc` の写し）を差し替える。**波線**はコード行の `HighlightStyle.underline { wavy }` で、重大度で色を変える（エラー = red / 警告 = yellow / 情報 = accent / ヒント = text_muted。`preview_render::diagnostic_color` の 1 つを右パネルも読む）。重なったら重い色が勝つ。複数行は行ごとに切り、幅 0（「ここに `;` が要る」型）は 1 文字ぶん・行末なら行末の外に 1 文字ぶん引く。**右パネルの `diagnostics` ビュー**（`tako panel --view diagnostics`）は文書ごとに 1 件 1 行で並べ、押すと `tako edit cursor` と同じ要求でその範囲を選ぶ。タブは **LSP につながった文書があるとき（かそのビューを選んでいるとき）だけ**出す（常設すると #1479 の段の選び方で既定幅 320px のラベルが全員ぶん落ちるため。5 本のときは段が詰まる側へ動き、最小幅の受け皿に `IconsTight` を足した）。**閉じたら捨てる**: 文書を閉じる（ペインを閉じる・編集モードを抜ける。同じファイルを複数のペインで開いているときは最後の 1 つ = #1769）と表から消え、**サーバが止まった・落ちた・起こし直すときもそのサーバの診断を捨てる**（直したのに消えない波線を残さない）。操作は dispatch `LspDiagnostics { pane, severity }` + CLI `tako lsp diagnostics [--pane N] [--severity error\|warning\|info\|hint] [--json]` + MCP `tako_lsp`（`action` = diagnostics。定義ジャンプ等は各スライスが action を足す）の 1:1。`pane` 省略は**つながった文書すべて**（呼び出し元は端末で診断を持たないので「呼び出し元のペイン」を既定にしない）。`severity` は**その重大度以上**。位置は `tako edit replace-range` と同じ（行 1 始まり・桁 0 始まりの UTF-8 バイト）。応答の `counts` は絞る前、`retained` は manager の保持総数（閉じたペインのぶんが残っていないかの観測口）、各文書の `drawn` はそのペインがいま波線として描いている数（GUI の写し。#1769 = 同じファイルの 2 ペイン目にも出ているかを外から読む口）。A/B は `TAKO_1007_LEGACY=1`（波線が消える）。Windows は偽サーバの e2e が根拠で、実サーバの診断は未実測なので MATRIX は Pending（#1007）。**制約**: 位置はサーバが publish した版の本文で写すので、打鍵の直後は次の publish まで数文字ずれうる（Zed のアンカー追従は持たない） | M |
| FR-3.31 | **定義ジャンプ（⌘クリックで定義先を新しいペインに）**（#1680 / #1007 S3）: コードプレビューの識別子を **修飾 + クリック**（macOS = ⌘ / Windows = Ctrl。#763 の 1 実装）すると、言語サーバへ `textDocument/definition` を問い合わせて飛ぶ。`declaration` / `typeDefinition` / `implementation` も同じ引数で CLI / MCP から使える。**⌘ホバー**で識別子に下線 + accent を引く（装飾は Markdown のリンク #680 と**同じ 1 実装** `md_view::push_hovered_link_highlight`。検出は `tako_core::lsp::goto::symbol_at` = 識別子の字面、`#include "x"` / `#include <x>` の行は引用符の中を 1 つの対象にする）。下線は検出表にその拡張子のサーバがあるファイルにだけ引く（押しても何も起きない下線を出さない）。**着地の規則（正本 = `tako_core::lsp::goto::plan_landing`）**: 同じファイル = 問い合わせたペインのまま飛ぶ / 同じタブの別のプレビューがそのファイルを開いている = そのペインを使い回す（同じ定義へ 2 回飛んでもペインが増えない）/ それ以外 = **新しいペイン**（既定は右。`open` で `down` / `new-tab`、`none` は開かずに場所だけ返す）/ 候補が複数 = 一覧（GUI は押した位置に出し、CLI / MCP は `status: choose` と `locations` を返して `choice`（1 始まり）で選ぶ）。着地は FR-3.27 の行指定 open と**同じ入口**（`dispatch::open_file`）を通り、FR-3.29 のジャンプ履歴へ積む（飛ぶ前の場所は**問い合わせたペイン**。着地先ではない = 戻るで問い合わせた位置へ帰る）。**行を指定して同じファイルを開くときは読み直さない**（編集セッションを捨てない = 同じファイル内の定義ジャンプ・戻る / 進むで編集モードが抜けない。編集中ならキャレットも着地点へ置く）。**編集モードでなくても使える（決定）**: 文書が開いていなければ**問い合わせのあいだだけ** `didOpen`（編集セッションがあればその全文、無ければディスクの中身）し、答えのあと `didClose` する。「開いただけではサーバを起こさない」（FR-3.28）は保ち、利用者が明示的に問い合わせたときだけ起こす。**UI をブロックしない**: 準備（引数の検査）は UI スレッド、位置の UTF-16 への変換（**サーバへ送った写しの全文**で数える = 単独の `\r` の後ろは LSP では次の行。#1769）とサーバの起動と応答は background（`OffloadJob::LspGoto`）で待ち、着地だけを UI スレッドへ戻す（`OffloadOutcome::OnUi` → `finish_offload`。IPC の受け口と GUI の ⌘クリックが同じここを通る）。上限は起動を含めて 30 秒（`TAKO_LSP_GOTO_TIMEOUT_SECS`）。答えは失敗ではなく `status` で返す: `found` / `choose` / `not-found`（見つからない）/ `timeout`（未応答。起動が終わらなかったときは別の理由文）/ `not-installed`（未導入。理由 + 導入コマンド）/ `unsupported`（能力に無い）/ `crashed` / `unavailable`（止めた・諦めた）/ `source-gone`（待つあいだに問い合わせたペインが閉じられた・差し替わった）等、どれも `reason` / `next_step`（日英は `tako_control::lsp::text`）。GUI は見つからないをヘッダに一時表示し、直す手のあるもの（未導入・未応答・落ちた）は通知欄にも理由と次の一手を出す。応答の `LocationLink` は `targetSelectionRange`（識別子そのもの）へ着地する。操作は dispatch `LspGoto { action, pane, line, column, open, choice, focus }` + CLI `tako lsp definition\|declaration\|type-definition\|implementation --pane N --line L --column C [--open right\|down\|new-tab\|none] [--choice K] [--focus]` + MCP `tako_lsp`（`action` 引数の 1 ツール。ツールは増やさない）の 1:1。位置は `tako edit replace-range` と同じ（行 1 始まり・桁 0 始まりの UTF-8 バイト）で、範囲外・文字の途中は**丸めずに拒否**する。CLI / MCP の既定はフォーカスを移さない（GUI の ⌘クリックは移す）。**⌘ 無しのクリックの選択は変更前とバイト一致**（セルフテスト項目 156 (i)）。A/B は `TAKO_1680_LEGACY=1`（⌘ホバー / ⌘クリックが定義を探さない・同じファイルの行指定 open が読み直す）。Windows は偽サーバの e2e と URI → パス（ドライブ文字・UNC）の単体が根拠で、実サーバの定義へ飛んだことは未実測なので MATRIX は Pending（#1007）。**読み込み中の空の答え（決定）**: rust-analyzer はプロジェクトの読み込みの前の問い合わせに**空で答える**（実測: tako のリポジトリで初回 0 秒で空・読み込み後は正しく飛ぶ）。空を「見つからない」と伝えないよう、`experimental/serverStatus` を申告して受け、サーバが読み込み中（`quiescent: false`）と知らせていれば済むまで上限内で待って問い直す（実測: 初回 14 秒で定義へ着地）。**読み込み中に送った要求の空は、manager が見る前に読み込みが済んでいても古い答えとして 1 回だけ問い直す**（#1930。定義ジャンプ・補完・ホバーの 3 つが `wait_after_empty` → `EmptyAnswer` の 1 実装を通る。旧挙動は見た時点で済んでいれば古い空を最終の 0 件にしていた。A/B は `TAKO_1930_LEGACY=1`）。状態を送らないサーバ（clangd 等）は握手の直後 2 秒だけ知らせを待ち、以後は空 = 見つからない（#1930 の問い直しも効かない）。LSP 標準の `$/progress` を待たないのは、検査（`cargo check`）の進捗まで含んで答えが分単位で遅れるため。シンボルの右クリックメニューは S7（#1684）が同じ口へ載せる | M |
| FR-3.32 | **ファイルツリーのドラッグ＆ドロップによる移動**（#1834）: ツリーの行（ファイル・フォルダ）をフォルダの行・ワークスペースのフォルダの見出しへドラッグすると、そのフォルダへ移す。ファイルの行の上に落とすとそのファイルのあるフォルダへ（VSCode と同じ = 狙う行が広い）。ドラッグ中は**落とし先のフォルダの行を accent で縁取って「ここへ移動」の札**を出し、その中に見えている行を薄く塗る。**移せない行は red で縁取って理由の札**（自分自身へ / 自分の配下へ / 同じ名前がある / リモート（SSH）/ 見出しのフォルダ）を出し、落とすと通知欄に理由を出す。同じ場所・掴んだ行へ戻したときは何も出さず何も起きない。縁取りは内側の影・札は絶対配置なので**行の高さを 1px も動かさない**。受け口（`on_drag_move` / `on_drop`）は**ツリーの行からドラッグしている間だけ**付ける（gpui の `on_drag_move` はカーソルの位置に関わらず全リスナーへ届くので、常時付けると行数ぶんのリスナーがマウス移動のたびに走る）。ペインへ落とす既存の D&D（FR-3.11 / FR-3.13）と Finder からのドロップ（#219）はペイン側・サイドバーの外枠が受けるので経路が交わらない。**判定と実行の正本は `tako_core::file_move`**: 字面の判定 `lexical_verdict` を画面のホバー（`drop_verdict`）と dispatch の段取り（`plan` = 実体の形で比べる。`..`・シンボリックリンク・macOS / Windows の大文字小文字（呼び手の綴りと実体の綴りの違い）で素通りさせない）の両方が通る。移せる / 移せないの**最終判断は dispatch** で、画面が先に止めるのは dispatch へ渡せないリモートの行と画面の方針で断る見出し行だけ（断る文面が CLI / MCP と揃う）。**同名は上書きしない**（`rename(2)` は既存のファイルや空のフォルダを黙って置き換えるので、段取りでリンクを辿らずに見たうえで、**実行は OS の上書きしない rename**（`tako_core::platform::fs_move::rename_noreplace` = 境界 B28 の core 分。macOS `renamex_np` + `RENAME_EXCL` / Linux `renameat2` + `RENAME_NOREPLACE` / Windows `MoveFileExW` のフラグ 0）で行う。見てから rename までの隙間に別のプロセスが同名を作っても OS が原子的に断り（`AlreadyExists` → 同名の理由）、元のファイルも割り込んだ同名も中身が残る = #1842。排他の rename を持たないファイルシステム（macOS の `ENOTSUP` 等）だけは見てから rename する形へ落とし、移動そのものは止めない）。**別のボリューム（EXDEV / `ERROR_NOT_SAME_DEVICE`。std がどちらも `CrossesDevices` へ寄せる）は理由を返して断る**（コピーしてから元を消す形は途中で失敗すると両側が半端に残り、画面にもコマンドにも確認の段が無いので採らない）。シンボリックリンクはリンクそのものを移す（指す先は動かさない）。**開いているファイルの付け替え**: 移したファイル（フォルダならその配下）をプレビュー・編集中のペインは、**移す前に**決めた付け替え先（`file_move::follows` = 字面 → 実体の形の順に照合。実体の形は元の場所にあるうちしか引けない）へパスを差し替える。編集バッファは本文・undo・版・基準を保ったままパスだけ（`TextBuffer::retarget`）を替え、未保存の変更は残る（移動は保存しない）。言語サーバの文書は旧 URI の `didClose` → 新 URI の `didOpen`、Code Runner の宣言・ジャンプ履歴（FR-3.29）・ディスクの監視・読み込み中のプレビューも新しいパスへ追う（読み込み中の結果は後勝ちで捨てられるので新しいパスで読み直す）。付け替えないと監視が元の場所を見て #1659 の「外で削除された」になる。**最近使った場所（`recent.json` のフォルダ・リポジトリ。ファイルは記録されない）も移したフォルダ（と配下）を移動先へ付け替える**（#1842。移した後に `file_move::moved_path` = 字面 → 段取りで引いた実体の形の順に照合し、実体の形の記録は実体の形のまま追う。並びは保ち、重なったら新しい方を残す。1 件も付け替えなければ書かない = 読めなかった一覧を空で上書きしない。付け替えないと「最近使った項目」から開けなくなる）。ツリーは展開状態を新しい場所へ持ち越し、元の親と移動先をその場で読み直す（移動先は開く = どこへ行ったか見える）。GUI の後始末は `PreviewHost::file_moved` の 1 本で、D&D・CLI・MCP のどれでも dispatch の中から 1 回だけ呼ばれる。**リモート（SSH）の行は対象外**: 掴むとゴーストに理由を出し、どこにも落とせない（ペインの落とし先も出さない）。ローカルのファイルをリモートの行へ落とすと理由を出して断る。**見出し（ワークスペースのフォルダ）は画面からは移さない**（ペインの cwd やピン留めの実体なので、ドラッグの弾みで丸ごと動かさない。CLI / MCP はパスを名指しした時点で意図が明らかなので断らない）。選んだ行をまとめて運ぶドラッグは FR-3.38（#1867）。移動の undo・⌥ ドラッグのコピー・リモートの中での移動は対象外（別 Issue）。操作は dispatch `FileOp { op: move, path, dest }` + CLI `tako file move <path> <dest>`（どちらも CLI の cwd 基準で絶対化）+ MCP `tako_file_op` の `op=move` + `dest`（**ツールは増やさない**）の 1:1。応答は `moved` / `from` / `to` / `kind`（`file` / `dir` / `symlink`）/ `followed`（付け替えたペインと前後のパス）/ `recent_followed`（付け替えた最近使った場所の件数。1 件以上のときだけ載る）、同じ場所は `moved: false` + `note`、断ったときはエラー文に理由（GUI の通知欄・CLI・MCP が同じ文面）。`dest` は省略時に wire へ現れない（旧クライアントの JSON がそのまま通る）。A/B は `TAKO_1834_LEGACY=1`（付け替えだけを外す = 移した編集ペインが元のパスを指したまま）。#1842 の A/B は `TAKO_1842_LEGACY=1`（実行を見てからの `std::fs::rename` へ戻し、最近使った場所を付け替えない）、注入は `TAKO_1842_INJECT_RACE=<名前>[,<名前>…]`（移した後の名前が当たる移動で、最後の確認の後・rename の直前に同名（ファイルなら中身入りのファイル・フォルダなら空のフォルダ）を作る）。機械検証は visual-test `tree-move`（実マウスの 13 場面）+ `scripts/test-tree-move-1834.sh`（CLI と MCP の字面比較・別のボリューム・シンボリックリンク）+ 番犬 `issue1834_tree_move_watchdog`（規則 7〜9 = 上書きしない rename・OS ごとの排他の口・最近使った場所の付け替え）+ `scripts/test-move-noreplace-1842.sh`（隔離 GUI の CLI と MCP で隙間の注入の A/B と最近使った場所の付け替え）。Windows は CI の Windows ランナーの `tako_core::file_move` の単体（実ファイルの移動・同名・配下）が根拠で、実機の D&D は未実測 | M |
| FR-3.33 | **整形（文書全体 / 範囲）と保存時整形**（#1683 / #1007 S6）: 言語サーバの `textDocument/formatting` / `rangeFormatting` で編集中のコードを整形し、編集バッファへ当てる（保存はしない）。**入口**: GUI の編集メニュー「コードを整形」「選択範囲を整形」・**⇧⌘I（Windows は Ctrl+Shift+I。Zed と同じ。Ctrl+Shift+I は Ctrl+I と同じ 0x09 へ潰れるので端末の Tab を奪わない = #585）**・パレット、CLI `tako lsp format [--pane N] [--range L1:C1-L2:C2]`、MCP `tako_lsp` の `action=format`（範囲は `line`・`column`〜`end_line`・`end_column` の 4 つそろえて。**ツールは増やさない**）。どれも同じ 3 段（`prepare_offload` = UI スレッドで**編集モードへ入り**（言語サーバもここで起きる = FR-3.28）全文・版・範囲を採る → `OffloadJob::LspFormat` = background で起動と応答を待つ（上限 30 秒。`TAKO_LSP_FORMAT_TIMEOUT_SECS`）→ `finish_offload` = UI スレッドで当てる）を通る。受け持つサーバが無い種類は編集モードへ入る前に `no-server`。**サーバの写しを頼む本文へ揃えてから頼む**（打鍵の直後で同期がまだ・同じファイルの別のペインが後から編集した、のどちらでも答えの座標がずれない）。**当て方（正本 = `tako_core::text_edit::TextBuffer::apply_changes` / `order_changes` の 1 実装）**: 並べ方は範囲の始まり → 終わりの**安定ソート**（同じ位置の挿入は配列の順 = LSP の規定。幅 0 の挿入は同じ位置から始まる置き換えより先）、**重なりは丸ごと拒否**（`invalid-edits`。どちらを採っても誰かの意図を壊す）、**隣接は可**、置き換える前後で同じ頭と尻は削って最小化（**削るのは並べた後** = 挿入どうしの順が入れ替わらない）。書き換えの箇所だけを差分 1 件に並べて**1 回の `apply_edit`（種類 Replace）**で当てる（離れた箇所のあいだの本文は undo へ積まない = 10 MB / 10 万行の文書の整形でも undo の予算 8 MiB に収まり、その前の履歴も残る。当て方は補完の確定と同じ `splices_of` の 1 本 = #1869。全文を 1 つの書き換えで返すサーバの答えも、行の前後の空白を除いた中身で突き合わせて行ごとの置き換えへ割ってから当てる = undo へ積むのは行ごとの差だけ・当てた本文は割らないときとバイト一致・書き換えの中のカーソルも同じ行の同じ中身に留まる = #1899）ので **undo 1 回で整形の前の全文とバイト一致へ戻り**、版も 1 つだけ進む（#1651 の塊・#1658 の版と整合。前後の打鍵の塊とはまとまらない）。入れる本文の改行はバッファの流儀へ揃え（#1650。範囲の外の改行は触らない = CRLF のファイルは CRLF のまま）、カーソルと選択端は書き換えに合わせてずらす。中身の同じ書き換えだけなら何もしない（`unchanged`。版も履歴も進まない）。**頼んだときの版にだけ当てる**: 待つあいだに打鍵・範囲編集で版が進んだら当てない（`stale`。manager 側でもサーバの写しが変わったら答えを捨てる）、頼んだペインが閉じられた / 別のファイルへ差し替わったら `source-gone`。**範囲の整形は範囲の外を 1 バイトも変えない**（決め: 外にかかる書き換えが空白だけなら捨てて残りを当てて `dropped_note`、空白以外を含むなら行の入れ替えを片方だけ当てると行が消える / 増えるので**全体を当てない** = `outside-range`）。範囲の整形の能力が無いサーバ（rust-analyzer の既定）は `unsupported` と「範囲を付けずに全体を整形する」の次の一手。`FormattingOptions` はバッファの字下げの単位から（スペース n 個 = tabSize n / insertSpaces、タブ = 4 / false。プロジェクトの rustfmt.toml 等はサーバ側が優先）。答えは失敗ではなく `status` で返す（formatted / unchanged / stale / source-gone / no-server / not-installed / unsupported / timeout / server-error / crashed / invalid-edits / outside-range / disabled と `note` か `reason` / `next_step`。未導入は導入コマンドつき）。変える所が無いのにエラーの診断があれば「構文エラーがあるとサーバは整形しないことがある」と注記する（rust-analyzer は rustfmt が読めないとき空で答える）。GUI は結果をプレビューのヘッダに一時表示する（定義ジャンプと同じ 1 本）。**保存時整形（ユーザー確定で既定 OFF）**: settings.json の `lsp_format_on_save`（`#[serde(default)]` = 旧ファイルはキーが無くても false で読める = 移行 Step 不要。指紋スナップショットだけ更新）。ON のとき**明示的な保存だけ**（⌘S / `tako edit save` / MCP `tako_preview_save` の save）で整形してから保存し、**自動保存（500ms のデバウンス）では整形しない**（打鍵の 500ms 後に勝手に整形されない。VS Code の formatOnSave が afterDelay の自動保存では動かないのと同じ）。整形を挟むのは「上書き保存（`--force`）でない・編集モード・外部変更の競合が無い・ローカルのファイル・受け持つサーバがある」ときだけで、どれかが欠ければ従来の保存とバイト一致。整形できなくても**保存は止めない**（上限 5 秒 = `TAKO_LSP_FORMAT_ON_SAVE_TIMEOUT_SECS`。応答の `format.note` に「整形せずに保存した（理由）」）。設定は CLI `tako lsp format-on-save [on\|off]` / MCP `tako_lsp` の `action=format-on-save`（`enabled`）/ 設定画面の「保存時に整形」の 1:1。操作は dispatch `LspFormat { pane, range }` / `LspFormatOnSave { enabled }`（範囲は `LineColRange` の数値のまま運ぶ = `行:桁` を解くのは CLI の入口だけ）。番犬 `issue1683_lsp_format_watchdog` が「`apply_edit` 1 回・安定ソート → 重なり → 最小化の順・自動保存が整形を通らない・既定 OFF・GUI が自前で当てない」を file:line で縛る。Windows は偽サーバの e2e（`issue1683_lsp_format`）が根拠で、実サーバの整形は未実測なので MATRIX は Pending（#1007） | M |
| FR-3.34 | **ファイルツリーのコピー / 切り取り / 貼り付け**（#1860）: ツリーの行を押す（左 / 右）と**選んだ行**になり（accent の枠。絶対配置の重ねなので行の高さを動かさない）、その間だけ ⌘C / ⌘X / ⌘V（Windows は Ctrl+C / Ctrl+X / Ctrl+V。修飾の判定は `tako_core::platform::keys::tree_clip_key` = リンクと同じ規則で、shift / alt が混ざると当たらない）がツリーへ向く。右クリックメニューにも「切り取り」「コピー」「貼り付け」（右端に打鍵の表記。貼るものが無いと押せない見た目で、押しても何も起きない）。**選択はタブとフォーカスペインを覚え、どちらかが動いた・ツリーを閉じた・ツリーの外を押した（ルートの捕捉フェーズで外す）・ほかの打鍵があった・Esc のどれかで外れ**、⌘C / ⌘V はペイン（端末・プレビューのコピー・貼り付け）へ戻る（Windows の Ctrl+C を端末の SIGINT から奪いっぱなしにしない。Esc は端末へ流さない）。入力欄（パレット・インライン入力・コミット欄等）が開いている間は入力欄が勝つ。**コピー**: 判定・別名・実行の正本は `tako_core::file_copy`。フォルダは中身ごと（権限・拡張属性は OS の写す口 = FR-3.39 の `fs_copy::copy_file_exclusive`、フォルダの権限は写し終えてから深い方から付ける）、**シンボリックリンクはリンクのまま**（祖先を指すリンクで潜らない）、FIFO・ソケット・デバイスは理由つきで断る。**同名は上書きしない**: Finder と同じ「名前 のコピー」「名前 のコピー 2」…（英語は `copy` / `copy 2`、Windows はエクスプローラーと同じ「名前 - コピー」「名前 - コピー (2)」。Finder はもう一度複製すると印を外して数え直す）の空いている名前に置き、置き場は**排他的に作る**（フォルダ = `create_dir` / ファイル = OS の写す口が排他的に作る（FR-3.39。clone = `fclonefileat` / 中身 = `create_new` / Windows = `COPY_FILE_FAIL_IF_EXISTS`）/ リンク = `symlink`）ので、名前を決めてから作るまでに誰かが同じ名前を作っても上書きせず断る。**途中で失敗したら（読めない項目・空き容量・特殊ファイル）この呼び出しで作った置き場だけを消して戻す**。自分自身・自分の配下へは実体の形（`file_move::lexical_verdict` を canonicalize 済みの形で）で断る。大きなフォルダで UI を止めないよう、写す本体は background（`OffloadJob::FileCopy` → `OffloadOutcome::OnUi` → `finish_offload`。IPC と GUI の ⌘V が同じ 3 段）。**切り取り → 貼り付けは移動**で、#1834 の `run_file_move` の 1 実装をそのまま呼ぶ（開いているペインの付け替え・同名 / 配下 / 別のボリュームの断りもそのまま。2 つ目の移動を作らない）。切り取り中の行は薄く描き（不透明度 0.45）、全部移せたら tako と OS のクリップボードを空にする（移せなかったものは切り取り中のまま = 別の場所へ貼り直せる）。見出しのフォルダの切り取りは画面からは断る（#1834 と同じ方針）。**貼り付け先**（正本 `tako_core::file_clipboard::paste_target` = VSCode と同じ）: フォルダの行 = その中 / ファイルの行 = そのフォルダ / 貼るもの自身の行 = その親（同じフォルダに複製）。貼ったら最後に置いたものを選び直す。**OS のクリップボード（境界 B28 = `tako-control::platform::file_clipboard`）**: ⌘C / ⌘X は tako の中身に加えて OS へもファイルとして書く（macOS = NSPasteboard の `public.file-url` を項目ごと + 1 つ目に `public.utf8-plain-text` のパス。旧形式の `NSFilenamesPboardType` は AppKit が自動で出す / Windows = `CF_HDROP` + `Preferred DropEffect`（切り取り = `DROPEFFECT_MOVE`。エクスプローラーはこれを見て移動する）+ `CF_UNICODETEXT`）。⌘V は**tako が書いた後に OS のクリップボードが書き換わっていなければ（変更番号が一致）tako の中身**（切り取りはここにしか無い）、書き換わっていれば OS の中身（Finder / エクスプローラーでコピーしたファイル = 複製。エクスプローラーの切り取りは移動）を貼る（正本 `file_clipboard::resolve`）。2 秒のポーリングでも古い切り取りを捨てて薄い行を戻す。**中身（ファイル URL）を読むのは実際に貼るとき（⌘V・`paste`）と `clipboard` だけ**で、2 秒のポーリングと右クリックの「貼り付け」を押せるかは変更番号と型の一覧（macOS = `changeCount` / `types`、Windows = `GetClipboardSequenceNumber` / `IsClipboardFormatAvailable(CF_HDROP)`）だけで決める（`file_clipboard::OsPeek` / `can_paste`。ユーザーが貼っていないのに中身を読むと、新しい macOS は pasteboard のプライバシーの確認を出しうる）。gpui のクリップボードは読みだけファイルを返し書きはファイルを黙って捨てるので、読み書きとも B28 の 1 組に持つ（外部クレートは足さず objc / Win32 を手書きで宣言）。host の既定はどちらも何もしない（テストのホストがユーザーのクリップボードを触らない）。検証用に `TAKO_FILE_PASTEBOARD=<名前>`（macOS のみ）で名前付きのペーストボードへ逃がせる。**ツリーの読み直し**: 写した・移した先はその場で読み直し、2 秒ポーリングが**読み直しより前に読み始めた古い一覧**は当てない（世代 `FileTree::generation` / `apply_refresh_since`。当てると貼ったものが次のポーリングまで消える = 実画面で観測した。#1834 の移動にも効く）。**リモート（SSH）の行は対象外**: 押すと選べるが、⌘C / ⌘X / ⌘V は理由を出して断る（端末へも流さない）。失敗はすべて #1399 の通知欄（一部だけ貼れなかったら件数 + 1 件目の理由）。複数選択でまとめて置くのは FR-3.38（#1867）。貼るときは 1 件ずつ写し（Finder から来た複数も同じ）、断られたものは `failed` に載る。操作は dispatch `FileOp` の `copy`（`dest` 必須）/ `clipboard_copy` / `clipboard_cut` / `clipboard`（中身と `path` で貼ったときの貼り付け先。何も変えない）/ `paste`（`path` = 貼る行）+ CLI `tako file copy <src>... <dest>` / `tako file clipboard [copy\|cut\|show]` / `tako file paste [<dest>]`（どれも CLI の cwd 基準で絶対化）+ MCP `tako_file_op` の同名の op（**ツールは増やさない**）の 1:1。応答は `copy` = `copied` / `from` / `to` / `kind` / `renamed` / `entries` / `bytes`、`paste` = `mode`（`copy` / `cut`）/ `source`（`tako` / `os`）/ `pasted`（コピーは copy の応答・切り取りは move の応答）/ `failed`（1 件も貼れなければエラー）、`clipboard_*` = `mode` / `paths` / `os`（OS へ書けたか。書けなければ `os_note`）。A/B は `TAKO_1860_LEGACY=1`（ツリーがキーを受けない・メニューの 3 項目が出ない = #1860 の前。CLI / MCP は同じ経路のまま）。機械検証は visual-test `tree-clipboard`（実マウス・実キーの 7 場面 + 別プロセスの AppKit との往復）+ `scripts/test-tree-clipboard-1860.sh`（CLI と MCP の字面比較・一般のペーストボードでの往復はユーザーのクリップボードを保存・復元・エッジ）+ 番犬 `issue1860_tree_clipboard_watchdog`。Windows は CI の Windows ランナーの `tako_core::file_copy` / `file_clipboard` の単体と DROPFILES の組み立て・読みの単体（macOS の上でも走る）が根拠で、**実機のエクスプローラーとの往復は未実測**。undo・⌥ ドラッグのコピー・リモートの中でのコピーは対象外 | M |
| FR-3.35 | **補完（予測変換）**（#1682 / #1007 S5）: コードプレビューを**編集モードで打つ**と、言語サーバへ `textDocument/completion` を問い合わせて、打っている語の頭の真下に候補の一覧を出す。**起こすきっかけ**（正本 = `tako_core::lsp::completion::trigger_for`）は 1 文字の打鍵だけ: ASCII の英数字か `_`（`Invoked`）か、サーバが申告した `triggerCharacters`（`.` / `:` 等 = `TriggerCharacter`）。貼り付け・IME の長い確定・日本語の地の文では起こさない。**デバウンス 80ms**（`TAKO_LSP_COMPLETION_DEBOUNCE_MS`）: 打ち続けている間は問い合わせず、指が止まった 1 回だけ。**古い要求は `$/cancelRequest` で捨てる**: 打鍵の要求（`superseding`）は manager の取り消しの列を通り、次の打鍵の要求が来た時点で前の 1 つを取り消す（待っていた側は `Superseded` で返る。CLI / MCP の要求は互いに取り消さない）。一覧を閉じたときも待っている要求を取り消す。**取り消しの番号は UI スレッドで先に取る**（#1909。`LspManager::reserve_completion` / `reserve_resolve` → 要求の `ticket`。manager の背景は補完・説明の補い・ホバーとも `enter_lane` の 1 つの口から列へ入る）: 背景で取ると、背景が走り出す前に閉じた・語の外へ出た取り消しを追い越して読み込みが済むまで待ち続け、サーバが答えずに待たせていれば待ちの表にも残る。列へ入ってまだ答えを返していない数は `tako lsp status` / MCP `tako_lsp_server` の `inflight`（`completion` / `resolve` / `hover`）。**古い答えは版の照合で捨てる**（`tako_core::lsp::completion::Session`）: 打鍵ごとに番号を進め、答えは「番号が最新」かつ「要求を出したときの本文の版 = 今の版」のときだけ出す。manager の側でも、要求を送ってから答えが届くまでに文書の版が進んでいたら `Edited` で捨てる（候補の範囲は送った本文の座標）。**一覧は `gpui::list` で仮想化**: 可視の 10 行 + 1 行ぶんだけを組む（1000 件でも組む行は 10〜11。#821 と同じ「1 フレーム数千要素」を作らない）。行は種類の印（色つきの角丸に 1 字。絵文字は使わない）+ label（等幅）+ 型の要約（detail）、非推奨は打ち消し線、選んでいる行の説明（detail + documentation の最初の 4 行）を下の帯に出す。**絞り込みと並べ替え**（正本 = `rank`）: 照合する問い合わせは候補ごと（置き換えの始点がカーソルと同じ行の手前ならそこからカーソルまで = LSP の仕様、そうでなければカーソルの直前の語）、相手は `filterText`（無ければ `label`）。段は 前方一致（大文字小文字まで）→ 前方一致（無視）→ 部分一致 → 語の切れ目からの飛び飛び、同じ段は `sortText` → `label`。`preselect` の候補を最初に選ぶ。一覧を出したまま打ち足すとその場で絞り直す（`isIncomplete` の一覧だけ問い直す）。Backspace も絞り直し、語の外へ出る・移動・改行・打鍵以外の本文の変化（undo・CLI / MCP の編集）・フォーカスの移動・編集モードを抜ける・ペインを閉じる / 差し替えるで閉じる。**キーの優先順位**（正本 = `route_key`。全 20 通りを単体が固定値で縛る）: 一覧だけ = Esc で閉じる（編集モードは抜けない）・Enter / Tab で確定・↑↓ で選ぶ（端で回る）/ 検索バーだけ = 従来どおり検索欄 / 両方 = Esc は**一覧を先に**閉じ（次の Esc で検索バー）、Enter / Tab / ↑↓ は検索欄へ渡して一覧は閉じる（キーボードは検索欄にある）。⌘ / Ctrl / ⌥ 付きは表に乗らない（キーバインドが先に取る）。⇧ 付きは一覧を閉じてから従来の経路へ。IME の変換中は奪わない。**確定**（dispatch `lsp_completion_apply` = GUI の Enter と CLI / MCP の `choice` の 1 本）: 候補の始点 〜 確定したときのカーソル（+ 候補がカーソルの後ろまで置き換えるぶん）を `insert_text` で置き換え、追加の編集（`additionalTextEdits` = 自動 import）も**同じ 1 回の操作**で入れる（`TextBuffer::replace_position_ranges` = undo 1 回で全部戻る。並べ方・重なり・最小化は整形（FR-3.33）と同じ `order_changes`、当て方も整形と同じ = 離れた範囲を差分 1 件に並べて、あいだの本文を undo へ載せない。#1869）。答えの後に打ち足していたら、置き換えの始点より手前の追加の編集だけを入れる（後ろは位置がずれている）。**スニペットは申告しない**（`snippetSupport: false`）。申告を無視して届いたスニペットは跳び先を落として既定の文字だけを平文で入れる（`snippet_to_plain`）。**説明の補い**: 一覧の答えに型の要約と自動 import を最初から載せさせ（`resolveSupport` は `documentation` だけ = 実測: `detail` を回すと rust-analyzer は選んだ 1 行にしか型を返さなかった）、選んだ候補に説明が無ければ `completionItem/resolve` で補う（前の説明の要求は取り消す）。**読み込み中の空の答え**: CLI / MCP は定義ジャンプと同じく `experimental/serverStatus` を見て済むまで待って問い直す。**GUI の打鍵の要求も待って問い直す**（#1869。実測: rust-analyzer は読み込みの前半は補完に即座に null、後半は答えずに待たせて済んだ直後に答える。#1869 前は打鍵の要求だけ待たずに 0 件で返していた = 一覧が出ず、済んでも問い直さなかった）。待つあいだに次の打鍵・一覧を閉じる・編集セッションが文書を閉じると抜ける。打鍵の問い合わせを出す時点でサーバが起動中か読み込み中（`LspManager::server_loading`）なら、一覧の代わりに 1 行の「読み込み中」を語の頭の真下へ出し、答えが届いたら一覧へ差し替える（表示だけ = 5 キーは奪わない。語の外へ出る・閉じるで消える）。上限まで読み込み中なら `status: loading`（理由・次の一手）、読み込みを待ってから答えたら `found` / `none` に `waited_for_loading_ms`、`tako lsp status` のサーバごとの `loading` も同じ判定。A/B は `TAKO_1869_LEGACY=1`（打鍵は待たない・「読み込み中」を出さない・整形も #1869 前の 1 範囲）で、機械検証は e2e `issue1869_lsp_followup`（偽サーバの `loading` シナリオ）+ 番犬 `issue1869_lsp_followup_watchdog` + visual-test `completion-loading` + `scripts/test-lsp-followup-1869.sh`。失敗（未導入・未応答・落ちた）は GUI では一覧を出さないだけ（打鍵のたびに通知しない）。操作は dispatch `LspCompletion { pane, line, column, limit, choice, resolve }` + CLI `tako lsp completion --pane N --line L --column C [--limit 50] [--choice K] [--resolve]` + MCP `tako_lsp`（`action=completion`。ツールは増やさない）の 1:1。位置は `tako edit replace-range` と同じ（行 1 始まり・桁 0 始まりの UTF-8 バイト）で範囲外・文字の途中は拒否、`limit` / `choice` の 0 も拒否。`limit` の既定は 50（`DEFAULT_LIMIT`。#322 の最簡形 = 何も付けずに読める件数）で、切った件数を `truncated`・全件を `total` に載せる。`choice` は件数の上限に関わらず全体の並びの番号。答えは `status`（found / applied / none / edited / source-gone / loading / not-installed / unsupported / timeout / crashed / unavailable / no-server …）と `reason` / `next_step`。A/B は `TAKO_1682_LEGACY=1`（打鍵で問い合わせない・一覧を出さない）と `TAKO_1682_NO_VIRTUAL_LIST=1`（一覧を全件ぶん組む）と `TAKO_1909_LEGACY=1`（取り消しの番号を背景で取る）。機械検証は e2e `issue1682_lsp_completion`（偽サーバ）+ 番犬 `issue1682_lsp_completion_watchdog` + visual-test `completion` / `completion-real`（実の rust-analyzer）+ `scripts/test-lsp-completion-1682.sh`、取り消しの番号は e2e `issue1909_lsp_completion_ticket` + 番犬 `issue1909_lsp_completion_ticket_watchdog` + visual-test `completion-cancel`（背景の走り出しを合図まで止める注入 `TAKO_1909_INJECT_HOLD`）+ `scripts/test-lsp-completion-cancel-1909.sh`。Windows は偽サーバの e2e が根拠で、実サーバの補完は未実測なので MATRIX は Pending（#1007） | M |
| FR-3.36 | **右クリックメニューの言語サーバの項目**（#1684 / #1007 S7）: コードプレビューの**本文**を右クリックすると、ペインのメニュー（ヘッダの右クリックと同じ `pane_context_menu`）をその位置に開き、**識別子の上でだけ**先頭へ言語サーバの項目を足す。項目は 定義へ移動 / 宣言へ移動 / 型定義へ移動 / 実装へ移動 / ホバー情報を表示（#1893）/ コードを整形 / 選択範囲を整形 の 7 つ（並びも含めて正本は `tako_core::lsp::menu::items`）。**出し分けはサーバの申告（`ServerCapabilities` の `…Provider`）だけ**で決め、申告に無い項目は出さない（押しても何も起きない項目 = 過大申告を作らない）。「選択範囲を整形」は**右クリックした位置が選択の中**のときだけ（`selection_covers`）。`#include` のパスは「定義へ移動」と「ホバー情報を表示」だけ（⌘クリックと同じくヘッダを開く・マウスを乗せたときと同じカード）。識別子かどうかは ⌘ホバーと同じ字面の判定（`goto::symbol_at`）なので、**コメント・文字列の中の語も対象**（他のエディタと同じ。⌘クリックの下線とも揃う）。識別子でない位置（空白・記号・数値・空行・行末）・受け持つサーバが無い種類・未導入・Markdown では**従来のメニューのまま**（LSP の項目は 0 件）。画像・PDF・動画の本文の右クリックは従来どおり何もしない。**項目はすべて dispatch を持つ**: 押すと `tako_control::lsp::menu::item_request` が組んだ要求（`LspGoto` / `LspHover { show: true }` / `LspFormat`）を ⌘クリック・編集メニューと同じ 3 段（`prepare_offload` → `run_staged` → `finish_offload`）へ渡す（GUI の側で操作を実装しない）。番犬は「項目の action + 載せない action（`lsp::menu::NOT_IN_MENU`。理由つき）= `dispatch::LSP_FEATURE_ACTIONS`」と「全項目の `args` を MCP の入口へ通すと GUI のクリックと同じ要求」。新しい言語機能の action（ホバー・参照の検索・リネーム…）を足すと、項目にするか載せない理由を書くかを決めるまで落ちる。**サーバがまだ握手していないとき**（閲覧中のファイルを初めて右クリックした。サーバは編集モードで起きる = FR-3.28）もメニューは**すぐ開き**、LSP の節に押せない 1 行「言語サーバに問い合わせています…」を出して背景でサーバを起こして待つ（定義ジャンプと同じ一時の didOpen）。答えが届いたとき同じメニューがまだ開いていれば項目へ差し替え、出す項目が無ければ節ごと消す。握手済みなら待たない読み口（`LspManager::menu_capabilities_now`）がその場で返す。メニューを開くだけでは握手以外の要求を送らない。**メニューの寸法**: 行の高さを描画と見切れ判定で同じ定数にした（`pane_menu_size`。以前は見積もり 20px・実高さ 23.5px で、項目の多いメニューを下端で開くと下へ見切れていた）。文言は `ui_text::pane_menu::lsp_item` / `lsp_pending` の日英（整形の 2 つは編集メニューと同じ関数）。操作は CLI `tako lsp menu --pane N --line L --column C [--json]` + MCP `tako_lsp`（`action=menu`。ツールは増やさない）の 1:1 で、GUI と同じ準備 `dispatch::lsp_menu_prepare`（位置の検査・識別子・位置を含む選択）と同じ出し分けを通り、項目ごとに押したときと同じ `tako_lsp` の引数（`items[].args`）を返す。位置は `tako edit replace-range` と同じ（行 1 始まり・桁 0 始まりの UTF-8 バイト）。応答の `status` は ok / not-symbol / no-server / not-installed / unavailable / timeout / disabled と `reason` / `next_step`。A/B は `TAKO_1684_LEGACY=1`（本文の右クリックで何もしない）。**範囲外**: 参照の検索（#1685）・リネーム（#1686）・コードアクション（#1687）は dispatch がまだ無いので出さない（着地したら項目にするか `NOT_IN_MENU` へ理由を書く。ホバーは #1893 で項目にした）。Windows は偽サーバの e2e が根拠で実サーバは未実測（MATRIX は Pending） | M |
| FR-3.37 | **ホバー（型・doc のカード）**（#1681 / #1007 S4）: コードプレビューの識別子に**マウスを乗せる**と、デバウンス（既定 300ms = VS Code / Zed と同じ。`TAKO_LSP_HOVER_DELAY_MS`。0 = 待たない）の後に言語サーバへ `textDocument/hover` を問い合わせ、語の行の真下にカードを 1 枚出す。**乗せただけではサーバを起こさない**（FR-3.28 の「編集モードで起こす」を保つ）: マウスの要求は文書が言語サーバにつながっているとき（編集モード / 別のペインが編集中）だけ出し、manager でも `open: false` = 開いている文書に持ち手として加わるだけ（つながっていなければ `NotOpen`）。**古い要求は `$/cancelRequest` で捨てる**（補完と同じ取り消しの列 `Lane::Hover`。別の語へ移った・カードを閉じたときも待っている要求を取り消す）。答えは番号（乗せ直すたびに進む）と編集バッファの版で照合し、離れた語・打ち足した後の答えは出さない。**本文の読み取り**（正本 = `tako_core::lsp::hover::parse_response`）: `MarkupContent`（`plaintext` は平文・それ以外は Markdown）/ `MarkedString`（文字列 = Markdown・`{ language, value }` = その言語のコードブロック）/ `MarkedString[]`（空行でつなぐ）、`null`・空白だけ・空のコードは「何も無い」。本文は **16,000 字**（`MAX_CHARS`）で行の切れ目まで切って印を付ける（カードの下に「全 N 字のうち先頭」・CLI / MCP は `truncated` / `total_chars` / `note`）。**切るのは出口ごと**（#1893）: manager の答えは全文で、カードはいつも 16,000 字、CLI / MCP は `limit`（省略 = 16,000・0 = 全文・N = N 字。`hover::char_limit` → `Hover::limited` の 1 本）。CLI は `--full` / `--limit N`（同時は拒否）、MCP は `tako_lsp` の既存の `limit`（ツールも引数名も増やさない）。`note` は全文の取り方を言う。**描画**: Markdown は `preview::markdown_blocks` → `md_view::render_block`（プレビュー・アップデート詳細と同じ 1 実装。見出し・構文色つきのコードブロック・リンクの下線）、平文は段落 1 つに素の文字列 1 本（解釈もエスケープもしない）。リンクは押すとブラウザで開く（開けない URL は対象外 = #680 の規則）。**置き場**（正本 = `hover_card_placement`）: 語の行の真下、下に入らなければ語の行の真上へ返す（返しの判定は `compute_menu_position`。語の行は覆わない）、右へはみ出すなら左へずらす（語の真下からカードへマウスを下ろせる）、どちらにも入らなければ広い方へ入る高さまで縮める（本文はカードの中でスクロール。高さの上限 360px・幅 520px）。高さは 1 フレーム目に見えないまま測って 2 フレーム目から置く。**閉じる**: マウスで出したカードは語とカードの外へ出たら閉じる（カードの上では閉じない）。どのカードも本文が変わったら・語の行が画面の外へ出たら・カードの外を押したら（押下そのものは止めない）・そのペインで打ったら閉じる。メニュー / CLI で出したカードはマウスの位置でもフォーカスでも閉じない（CLI の `--show` はフォーカスが端末のまま出す）。Esc はカードだけを閉じる（編集モードは抜けない。補完の一覧が出ていれば 1 回目の Esc は一覧）。補完の一覧を出しているあいだ・右クリックメニュー（FR-3.36）を開いているあいだはカードを出さない（一覧が手前。メニューを開けばカードは閉じる）。失敗（未導入・未応答…）はカードを出さないだけ。**サーバの読み込み中**（#1893）: マウスの要求も読み込みが済むのを待って問い直す（補完の打鍵 = #1869 と同じ manager の `wait_loaded`。次のマウスの要求・カードを閉じる・文書を閉じるで抜ける。取り消しの番号は UI スレッドで先に取る = `reserve_hover`。補完と同じ `enter_lane` の口 = #1909）。待つあいだは語の真下に「読み込み中」の 1 行（補完と同じ見た目。押下は通す）を出し、答えが届いたらカードへ差し替える。上限（30 秒）まで読み込み中なら 1 行を下ろして `loading`。**キーボードの口と右クリック**: 編集メニュー・パレット・**⇧⌘H（Windows は Ctrl+Shift+H）**の「ホバー情報を表示」（編集カーソル / 選択の先頭の位置）と、本文の右クリックメニュー（FR-3.36）の「ホバー情報を表示」（識別子の先頭。`hoverProvider` の申告があるときだけ）。キーの選び方（#1893）: VS Code / Zed の ⌘K ⌘I は ⌘K = パレットと 2 打鍵にすると発火が遅れるので採らず、整形の ⇧⌘I と同じ段の H。Windows の Ctrl+Shift+H は Ctrl+H（0x08）と同じバイトへ潰れるので端末の入力手段を奪わない（#585）。どれも CLI `tako lsp hover --show` と同じ dispatch の 3 段（`request_lsp_hover`）を通り、待つあいだはヘッダに「問い合わせています」（読み込み中ならその旨）、理由もヘッダに出す。補完の一覧が出ているあいだのキーは一覧を閉じてからカードを出す。本文の右クリックは待っているマウスのホバーを捨ててからメニューを開く（捨てないとメニューを閉じた後に古い語のホバーが発火し、明示のカードを置き換えて消す）。操作は dispatch `LspHover { pane, line, column, show, limit }` + CLI `tako lsp hover --pane N --line L --column C [--show] [--full \| --limit N]` + MCP `tako_lsp`（`action=hover`・`show` / `limit`。ツールは増やさない）の 1:1。位置は `tako edit replace-range` と同じ（行 1 始まり・桁 0 始まりの UTF-8 バイト）で範囲外・文字の途中は拒否。明示の問い合わせ（CLI / MCP / メニュー）は開いていない文書も問い合わせのあいだだけ開き、読み込み中の空の答えは定義ジャンプと同じく済むまで待って問い直す（待ったら `waited_for_loading_ms`）。答えは `status`（found / none / not-open / not-installed / unsupported / loading / timeout / crashed / unavailable / no-server …）・`kind`・`contents`（本文のまま）・`range` と `reason` / `next_step`、`show` なら `shown`。A/B は `TAKO_1681_LEGACY=1`（マウスで問い合わせない・カードを出さない・メニューの口も出さない。CLI / MCP は残る）と `TAKO_1893_LEGACY=1`（右クリックの項目・キーを出さない・マウスの要求は読み込みを待たない。CLI / MCP の `limit` は残る）。機械検証は e2e `issue1681_lsp_hover` / `issue1893_lsp_hover_followup`（偽サーバ）+ 番犬 `issue1681_lsp_hover_watchdog` / `issue1893_lsp_hover_followup_watchdog` + 単体（core の 3 形と `limited`・GUI の `render_block` 経由と四隅・dispatch の `show` と `limit`・keybindings の衝突）+ visual-test `hover` / `hover-real` / `hover-1893` / `hover-loading` / `hover-loading-real`（実の rust-analyzer）+ `scripts/test-lsp-hover-1681.sh` / `scripts/test-lsp-hover-1893.sh`。Windows は偽サーバの e2e が根拠で、実サーバのホバーは未実測なので MATRIX は Pending（#1007） | M |
| FR-3.38 | **ファイルツリーの複数選択・移動として貼る・コピーの進み具合**（#1867。FR-3.34 の続き）: ①**複数選択**: ⌘クリック（Windows は Ctrl+クリック）で行を足す / 外す・⇧クリックで起点からの範囲（それまでの選択は捨てる）・⌘⇧クリックで範囲を足す（Finder / VS Code と同じ。押し方の判定・状態遷移・範囲の正本は `tako_core::tree_select` の `click_kind` / `apply` / `range`）。修飾つきの押下は**選ぶだけ**でファイルを開かずフォルダも開閉しない。最後の 1 行を外すと選択が無くなり ⌘C / ⌘V はペインへ戻る。⇧クリックの起点が見えていなければその行だけ。選んだ行すべてに accent の枠。#1860 の「押下の捕捉フェーズで選択を外す」はそのままに、外した選択を退避して（`sidebar::TreeMulti::stash`）⌘クリックの基準・選んだ行を掴んだときの保持・右クリックメニューの対象に使う（選んだ行の素の押下は離したときにその行だけを選び直す = Finder と同じ）。②**まとめた操作**: 選んだ行の上の ⌘C / ⌘X・右クリックの「切り取り」「コピー」「削除（ごみ箱）」（件数を添える）・選んだ行を掴んだ D&D（ゴーストは「N 項目」）が選んだもの全部へ効く（見えている行だけ。畳んだフォルダの中は外す）。**フォルダとその配下を同時に選んだら配下は親と一緒に扱う**（`tree_select::distinct_roots` = VS Code の distinctParents。2 回写す・親を移した後に「見つからない」を起こさない。応答の `skipped`）。1 件ずつは単数と同じ口（クリップボード = `clipboard_put` / ごみ箱 = `trash_one` / 移動 = #1834 の `run_file_move`）で、断られたものは `failed` に理由つきで残り、移せたものは移す（全部断られたらエラー）。まとめて運ぶドラッグの判定は `file_move::drop_verdict_many`（落とし先を壊す理由 = 選んだフォルダの上・その配下・見出し・リモートが 1 つでもあれば全部を断る / 同名だけなら移せるものを移す）。貼った・移したものは全部を選び直す。③**移動として貼る = ⌥⌘V**（Finder の「項目をここに移動」。Windows は Ctrl+Alt+V で、AltGr（Ctrl+Alt として届く）が文字を生む打鍵 = 欧州配列の `@` 等は奪わない。判定は `platform::keys::tree_clip_key`）+ 右クリックの「項目をここに移動」: コピーしたものも切り取りと同じ #1834 の `run_file_move` で移し（同名・配下・別のボリュームは理由つきで断る・開いているペインは付け替わる）、移し終えたら tako と OS のクリップボードを空にする。④**大きなコピーの進み具合と取り消し**: 写す前に件数とバイトを数え（`file_copy::measure`）、1 項目ごとに `file_copy::Progress`（atomics）を進める。走っているコピーはプロセスの一覧（`file_copy::jobs`）に載り（GUI の ⌘V・CLI・MCP・同期の経路のすべて）、ツリーの上端に帯（「コピー中 12 / 340 項目・1.2 MB / 5.0 GB」+ 進み具合の棒 + 「取り消し」。数えている間は「コピーの準備中」。始めて 300 ms 経ったものだけ = 短いコピーでちらつかせない。CLI / MCP から始めたコピーも 2 秒のポーリングで拾って帯に出す）。取り消しは**次の項目へ進む前に**見て、写している 1 件は #1860 の「作った置き場だけを消して戻す」で作りかけを残さず、まだ始めていないものは始めない（写し終えたものは残す = Finder と同じ）。1 つのファイルの途中でも止まる・バイトが進む・残り時間の目安は FR-3.39（#1895）。**口は 1:1**: dispatch `FileOpMany { op, paths, dest }`（`op` は clipboard_copy / clipboard_cut / trash / move。copy は FR-3.39）と `FileOp` の `paste_move` / `copy_progress`（`copies` = id / paths / dest / entries_done / entries_total / bytes_done / bytes_total / counting / cancelled）/ `copy_cancel`（`name` = id、省略で全部。応答 `cancelled`）→ CLI `tako file clipboard copy\|cut <path>...` / `tako file move <src>... <dest>` / `tako file trash <path>...`（複数ならまとめた 1 要求・1 件でも断られたら結果を出してから終了コード 1 = mv / rm と同じ）/ `tako file paste --move [<dest>]` / `tako file progress` / `tako file cancel [<id>]` → MCP `tako_file_op` の `paths` 引数と同名の op（**ツールは増やさない**。`path` は進み具合・取り消しでは要らないので必須から外した）。応答は `FileOpMany` = `done`（1 件ずつの単数の応答）/ `failed`（`path` / `reason`）/ `skipped`（配下を重ねたもの。あるときだけ）、`clipboard_*` は単数と同じ形。検証用に `TAKO_TRASH_DIR=<dir>`（ゴミ箱の代わりにそのフォルダへ = 同名は番号を付ける。ユーザーのゴミ箱へ fixture を入れない）と `TAKO_1867_COPY_DELAY_MS=<ms>`（1 項目ごとに待つ = 小さな fixture で帯と取り消しを見る。CPU を焼く負荷を使わない）。A/B は `TAKO_1867_LEGACY=1`（⌘ / ⇧クリックが素の押下・⌥⌘V がツリーへ向かない・帯とメニューの「項目をここに移動」を出さない = #1867 の前。CLI / MCP は同じ経路のまま）。機械検証は visual-test `tree-multiselect`（実マウス・実キーの 8 場面）+ `scripts/test-tree-multiselect-1867.sh`（A/B・CLI と MCP の字面比較・CLI で始めたコピーを MCP で読んで取り消す / その逆・権限エラー）+ 番犬 `issue1867_tree_multiselect_watchdog`。Windows は CI の単体（`tree_select` / `file_copy` の進み具合・取り消し / `keys` の Ctrl+Alt+V と AltGr）が根拠で、**実機の Ctrl+クリック・Ctrl+Alt+V は未実測** | M |
| FR-3.39 | **ファイルツリーのキーでの範囲選択・ごみ箱・1 つのファイルの途中での取り消し・複数のコピーを 1 つのジョブへ・残り時間の目安**（#1895。FR-3.38 の続き）: ①**⇧↑ / ⇧↓**（両 OS 同じ。⇧ 以外の修飾が混ざると当たらない）= 最後に押した行の 1 行上 / 下を ⇧クリックしたのと同じ（起点からの範囲。伸ばしてから逆へ打つと縮み、起点を越えると反対側へ伸びる。正本 `tako_core::tree_select::extend` → `apply` の `Range`）。先頭で ⇧↑・末尾で ⇧↓ は何も変えず、最後に押した行が見えていなければ伸ばさない。畳んだフォルダは見出しの 1 行だけ（中は入らない = まとめた操作もフォルダごと）。伸ばした行はスクロールして見せ、ファイルは開かない。②**⌘⌫（Windows は修飾無しの Delete）**で選んだもの（見えている行だけ）をごみ箱へ（Finder / エクスプローラーと同じ。判定 `platform::keys::tree_select_key`。⌫ 単独・Shift+Delete = 完全に削除は奪わない）。右クリックの「削除」と同じ口（`trash_tree_paths` = 1 件は `FileOp` / 複数は `FileOpMany` の `trash` = CLI `tako file trash` と同じ要求）で、入れたら選択を外す。**見出しのフォルダとリモート（SSH）の行は理由を出して断る**（打ち間違いでワークスペースごとごみ箱へ入れない。右クリックの「削除」・CLI / MCP はそのまま）。右クリックの「削除」の右端に打鍵の表記（`⌘⌫` / `Delete`）。①② のキーはツリーの行を選んでいる間だけ（FR-3.34 と同じ条件）で、選んでいなければ端末へ流れる。③**1 つのファイルの途中でも止まる・バイトが進む**: 1 つのファイルは `platform::fs_copy::copy_file_exclusive`（境界 B28）で写す。同じ APFS ボリュームは先に OS の複製（`fclonefileat` = clone。中身を写さず一瞬で終わる = 途中が無い）、できなければ（別のボリューム・APFS 以外）macOS は `fcopyfile` + 状態コールバック（1 MiB ごと・`COPYFILE_QUIT` で止める）/ Windows は `CopyFileExW` + 進み具合ルーチン（`PROGRESS_CANCEL`）で、写す単位ごとにバイトを進め（OS の累計が戻っても減らさない = 単調）取り消しを見る。止めた・失敗した作りかけのファイルはその口が消す。**置き場は排他的に作る**（clone = `fclonefileat` の EEXIST / 中身 = `create_new` / Windows = `COPY_FILE_FAIL_IF_EXISTS`。FR-3.34 の上書きしない不変条件はそのまま）。#1860〜#1867 は `create_new` で名前を押さえてから `std::fs::copy` していたので、std の clone（写す先が無いときだけ試す）が EEXIST で外れて**同じボリュームでも常に中身を写していた**。**実測**（M 系の Mac・256 MiB・中央値）: 同じボリュームは clone 0.1 ms（#1895 の前 = 中身を写して 37 ms）、中身を写す道は進み具合のコールバックの有無で差が無い（同じボリューム 37〜40 ms / 別のボリューム = APFS のディスクイメージ 173〜195 ms）ので、ファイルの大きさの閾値で道を分けない。④**`tako file copy a b dst` / MCP `paths` の `op=copy` は 1 要求・1 つのジョブ**（`FileOpMany` の `copy`。`dest` 必須。札 1 枚 = 進み具合も取り消しも 1 つ。配下を重ねたものは親と一緒に写して `skipped`。応答は他のまとめた操作と同じ `done`（1 件ずつ `copy` の応答）/ `failed` / `skipped`、全部断られたらエラー、CLI は 1 件でも断られたら結果を出してから終了コード 1）。IPC は `prepare_offload` → `FileCopyJob::many` で background へ回す。⑤**残り時間の目安**: 帯の 2 行目（棒の右端）に「あと約 25 秒」等（1 行目は狭いと末尾が省略されるので置かない。2 行目の高さは棒が出た時点で決めるので、目安が出てもツリーの行は動かない）。正本は `file_copy::eta`（数え終えてからの平均の速さで残りのバイトを割る = 直近の速さの揺れを見せない）で、**写し始めて 2 秒（`ETA_MIN_ELAPSED`）経ち 1%（`ETA_MIN_PERMILLE`）以上写すまでは出さない**。数えている / 取り消した / バイトが無い / 何日もかかる見積もりも出さない。帯の粒度は `eta_label`（10 秒未満 =「まもなく完了」/ 1 分未満 = 5 秒単位へ切り上げ / 1 時間未満 = 分へ四捨五入 / それ以上 = 10 分単位）。CLI / MCP の `copy_progress` の各コピーに `eta_secs`（秒・切り上げ。出せないうちは null）。**口は 1:1**: ①② は画面の入口で、ごみ箱そのものは既存の `trash`（CLI `tako file trash <path>...` / MCP `tako_file_op` の `trash` + `paths`。選択は画面の状態なので、FR-3.38 と同じく CLI / MCP はパスを名指す）、③④⑤ は dispatch の `FileOpMany` の `copy` と `copy_progress` の `eta_secs` → CLI `tako file copy <src>... <dest>` / `tako file progress` → MCP `tako_file_op` の `paths` と `copy_progress`（**ツールは増やさない**。カタログは +7 B）。検証用に `TAKO_1895_COPY_CHUNK_DELAY_MS=<ms>`（1 つのファイルの中身を写す単位ごとに待つ = 数十 MiB の fixture で途中の進み具合と取り消しを見る。立てた回は clone を試さない）。A/B は `TAKO_1895_LEGACY=1`（⇧↑ / ⇧↓ / ⌘⌫ がツリーへ向かない・1 つのファイルは `create_new` + `std::fs::copy` = 途中で止まらずバイトは写し終えてから進む・帯と `copy_progress` に残り時間を出さない・CLI の複数指定は 1 件ずつ別の要求 = #1895 の前）。機械検証は visual-test `tree-keyboard-copy`（実マウス・実キーの 4 場面）+ `scripts/test-tree-keyboard-copy-1895.sh`（A/B・CLI と MCP の字面比較・1 つのジョブ・途中のバイト・相互の取り消し・権限エラー・取り消しの直後の再開）+ 番犬 `issue1895_tree_keyboard_copy_watchdog`。Windows は CI の単体（`keys` の Delete / Shift+Delete、`file_copy` の途中の取り消し・同名・0 バイト、`fs_copy` の `CopyFileExW` の累計と止め）が根拠で、**実機の Delete キー・`CopyFileExW` の途中の取り消しは未実測**。選択そのものを CLI / MCP から読む・動かす口は FR-3.40（#1908） | M |
| FR-3.40 | **ファイルツリーのキーでの選択の移動・端までの範囲・選択の CLI / MCP・残り時間の数え下ろし**（#1908。FR-3.39 の続き）: ①**↑ / ↓**（修飾無し・両 OS）= 最後に押した行の 1 行上 / 下の行だけを選ぶ（Finder / VS Code と同じ。複数選んでいても動いた先の 1 行。端で止まり折り返さない。ファイルは開かない）。**← / →** = 開いたフォルダを畳む・それ以外（ファイル・畳んだフォルダ）は親のフォルダの行へ（見出しは上がらない）/ 畳んだフォルダを開く・開いたフォルダは最初の子へ（空のフォルダ・ファイルは何もしない）。**Enter** = その行を素で押したのと同じ（ファイルは `OpenFile` でフォーカスペインから開き、開いた後のフォーカスペインで選び直す / フォルダは開閉。複数選んでいても最後に押した行だけ。macOS の Finder / VS Code は Enter = 名前の変更だが、Issue の指定どおり開くに揃えた）。②**⇧⌘↑ / ⇧⌘↓（Windows は Shift+Ctrl+Home / End）** = 先頭 / 末尾の行を ⇧クリックしたのと同じ（起点は動かない = 続けて ⇧↓ で縮む。正本 `tree_select::extend_to_edge`）。最後に押した行が見えていなければどのキーも何も変えない（`extend` と同じ）。判定は `platform::keys::tree_select_key`（修飾が混ざると当たらない = ⌥↑ のペイン移動・⌘← 等を奪わない。Windows の Shift+Home / End 単独・Ctrl+Home は奪わない）、状態遷移の正本は `tako_core::tree_select::on_key`（`RowShape` = 見えている行の形 → `KeyOutcome` = 選択 / 開閉 / 開く）。キーはツリーの行を選んでいる間だけ（FR-3.34 と同じ条件）で、選んでいなければ端末へ流れる（↑ / ↓ はシェルの履歴・Enter は実行のまま）。リモート（SSH）の行の上ではどのキーも動かさない（#1895 の ⇧↑ / ⇧↓ と同じ。範囲の並びに載らない）。③**Windows の Shift+Delete（ごみ箱を通らない削除）は扱わない**（端末へ流れる）: CLI / MCP（`tako_file_op`）に完全に削除する op が無く、画面だけの削除の経路を作ると設計原則 5（AI フルコントロール）と 1:1 が崩れるため（master の判断。新しい削除の口を足すかは別 Issue で決める）。④**残り時間の数え下ろし**: `file_copy::eta` の速さを「いま」までではなく**最後にバイトが進んだ時点**（`Progress` の `bytes_at` = 数え終えてからの時間・`advances` = 進んだ回数）までの平均で測り、そこから見積もった写し終える時刻までを数え下ろす（式は `estimate`）。#1895 は「いま」までの平均で割っていたので、次の進み（macOS は 1 MiB ごと）が届くまで分母の時間だけが伸びて見積もりが増え、届くと減る = 一定の速さでも 1 チャンクごとにのこぎり状に揺れ、帯の表記（5 秒単位）が「あと約 15 秒」と「まもなく完了」を行き来した。数え下ろしは状態を持たない（GUI の帯・CLI / MCP の `eta_secs` が同じ値を読む）。進みが途切れて `stall_after`（進みの平均の間隔の 3 倍と 2 秒の大きい方）を越えたら止まったと見なし、越えた分だけ #1895 と同じ伸び方（残り ÷ 写した）で後ろへずらす（つなぎ目で値が飛ばない・止まっている間に 0 へ落ちない）。帯の描き直しの刻み（150 ms）と表記の粒度（`eta_label`）は変えない（揺れの元は刻みではなく値ののこぎりだった）。**実測**（単体の合成・1 MiB を 300 ms ごと・150 ms ごとに読む）: 一定の速さで逆戻りの最大 0.71 秒 → 0・逆戻りの合計 5.87 秒 → 0・表記の戻り 3 回 → 0・真の残りとの差の最大 1.47 秒 → 0、±30% で揺らした速さで逆戻りの合計 11.79 秒 → 1.18 秒。⑤**口は 1:1**: dispatch `TreeSelection { path, key, tab }`（どちらも省略 = 読むだけ / `path` = その行だけを選ぶ（見えている行だけ・開かない。字面で当たらなければ実体の形で照合）/ `key` = `up` / `down` / `left` / `right` / `enter` / `extend_up` / `extend_down` / `extend_top` / `extend_bottom`）→ CLI `tako tree selection [<path>] [--key K]` → MCP `tako_tree_folder` の `action=selection` + `key`（**ツールは増やさない**）。画面のキー（#1895 の ⇧↑ / ⇧↓ も）は同じ dispatch を通る（`tree_select_key_action` → `Request::TreeSelection`。UI が範囲を自前で決めない）。host のプリミティブは `UiStateHost` の `tree_rows` / `tree_selection` / `set_tree_selection` / `set_tree_expanded`。応答は `selection`（`paths` / `lead` / `anchor` / `remote`。無ければ null）+ キーと行を選んだときは `changed`、開閉したら `expanded`（`path` / `expanded`）、ファイルを開いたら `opened`（`OpenFile` の応答）、リモートの行の上のキーは `note`。断るとき: ツリーが閉じている（`tako panel --filetree on` を案内）/ 何も選んでいないのに `key` / 見えていない行 / `path` と `key` の同時指定 / 知らない `key`（語彙を全部挙げる）/ アクティブでないタブ。A/B は `TAKO_1908_LEGACY=1`（④ を「いま」までの平均へ戻す・①② のキーがツリーへ向かずペインへ流れる。CLI / MCP は同じ経路のまま）。機械検証は visual-test `tree-keys`（実キーの場面）+ `scripts/test-tree-keys-1908.sh`（A/B・CLI と MCP の字面比較・実 GUI の一定速度のコピーでの `eta_secs` の振れ）+ 番犬 `issue1908_tree_keys_watchdog`。Windows は CI の単体（`keys` の Shift+Ctrl+Home / End と奪わない打鍵、`tree_select::on_key`、dispatch の応答）が根拠で、**実機の Windows のキーは未実測** | M |
| FR-3.41 | **言語サーバの自動取得とエディタの状態表示**（#1944。FR-3.28 の続き）: 編集モードで開いたファイルのサーバが**グローバルに無ければ、tako が data dir へ取って起こす**（Zed / VS Code と同じ体験。操作なしで補完・診断が出る）。解決は **差し替えの env → 利用者の PATH（表の `probe_args` があれば動くことを確かめる = rustup の代理だけが居て component が無い rust-analyzer を「入っている」と読まない）→ tako の置き場 `<data_dir>/lsp-servers/<id>/<版>` → 取る** の順で、**グローバルに入っている版があればそちらを使う**。取得の表は検出表の各行の `fetch`（`tako_core::lsp::fetch`）: pyright 1.1.414 と typescript-language-server 5.1.3 + typescript 5.9.3 は npm レジストリの tarball（Node.js で起こす。利用者の PATH に `engines` を満たす node が無ければ Node.js 24.21.0 LTS の実行ファイル 1 つを nodejs.org から取る）、rust-analyzer 2026-09-21 は GitHub Releases の単体バイナリ（macOS は gzip、Windows は zip の中の 1 つ）、texlab 5.26.0 と tombi 1.7.0 も GitHub Releases の単体バイナリ（macOS は tar.gz の中の 1 つ、Windows は zip の中の 1 つ。#1956）、clangd は取らない（macOS で 100 MB の zip + コンパイラのヘッダが別に要る。macOS の案内は `xcode-select --install` = #1956）。sourcekit-lsp / dart は言語の道具に同梱なので取らず、CSS / HTML（vscode-langservers-extracted）は依存 32 包み（scoped を含む・展開 93 MB）を取る口がまだ無いので取らない（どちらも導入コマンドの案内だけ）。**クライアントは `workspace.configuration` を申告しない**（申告して `null` で答えると vscode-langservers-extracted の検証が例外で止まり診断が 0 件になる = #1956 の実測。申告するなら `{}` で答える）。**版を固定し、配布元が公開しているハッシュをそのまま固定して検証する**（npm は `dist.integrity` の SHA-512、nodejs.org は `SHASUMS256.txt`、GitHub は asset の `digest`）。合わなければ捨てる。展開は一時の段へ行い、**済んだ印 `.tako-installed` を書いてから版の段へ rename**（印の無い段は使わない）。**利用者の環境を汚さない**（npm / brew / winget を起こさない・PATH へ足さない・npm の install script を走らせない）。同じ置き場は錠で 1 本だけが取り（同じサーバを 2 つのペイン・器が同時に求めても 1 回）、別プロセス（GUI と CLI）が重なったら rename の勝った側を使う。取っているあいだの状態は「取得中」（`fetching`）で、`tako lsp status` の各サーバの `fetch`（取得物・段階・バイト数・経過）と画面に出る。**失敗**（オフライン・404・ハッシュ不一致・大きすぎる・書き込めない・展開できない）は理由（`fetch_error` の種別 + 文）を記録して**未導入へ落ち、今の案内（導入コマンド）を残す**。失敗の記録があるあいだは開き直しても自動では取り直さない（`tako lsp install` / `restart` / 画面の「もう一度取得」で取り直す）。取っているあいだに文書がすべて閉じられたら起こさない（取得物は置き場に残る）・stop したら取れても起こさない。**導入を検知したら restart を待たずに起こす**（#1823 の 2）: 取れたとき、`tako lsp install` が済んだとき、未導入のサーバがシェル統合の合図（`tako_core::shell_activity::subscribe`）で引き直して見つかったとき（引き直しは背景のスレッド 1 本で、間隔は直前の 1 巡にかかった時間 × 50 を 15 秒〜5 分に収める = ログインシェルをコマンドごとに起こさない。#1977）。断っていた文書は UI への状態の知らせ（`LspManager::state_events`。診断の知らせとは別の口）で同期し直してつながる。**エディタの状態表示**: 編集中のコードのタイトルに「<サーバ> を取得中（<取得物> n / m MB）/ 起動中 / 読み込み中 / 動作中」、未導入・取得の失敗・諦めた・止めたは**タイトルの下の帯**に見出し（補完・診断が出ないこと）+ 理由 + 次の一手 + ボタン（**入れる / もう一度取得 / 起こし直す**。押すと manager の `install` / `restart` を背景で呼ぶ = CLI / MCP と同じ口）。状態の口 `LspManager::document_server` は**待たない**（ロックと置き場の印の stat だけ）。操作は dispatch `LspServer { action: "install", name }` + CLI `tako lsp install [--name <id>]`（**tako が起動していなくても CLI のプロセスで同じ実装を走らせる** = setup やスクリプトから前もって入れられる）+ MCP `tako_lsp_server` の `action=install` の 1:1（ツールは増やさない）。`tako lsp servers` の各行に `source`（path / override / managed）と `fetch.available`。`tako setup deps` には載せない（あちらはグローバルへ入れる導入器の段。言語サーバは開いた時点で取るので setup の段を増やさない）。**env**: `TAKO_LSP_AUTO_FETCH=0`（開いた時点で取らない = `tako lsp install` / ボタンだけ。隔離起動（#2048 = FR-3.28。判定は窓の置き場と同じ `platform::display::is_verification_gui`）・セルフテスト・visual-test は `=1` が無ければ取らない = 検証からネットワークへ出ない）/ `TAKO_LSP_FETCH_BASE=<url>`（ミラー。`https://<host>/…` を `<url>/<host>/…` へ替える。ハッシュは同じ値で検証）/ A/B は `TAKO_1944_LEGACY=1`（取得も状態の表示も出さない = FR-3.28 の案内だけ）。**続き（#1977）**: ① **状態を送らないサーバ（pyright）は最初の診断が届くまで読み込み中**（`lsp::goto::loading_of`。上限 10 秒 = `WARMUP_LIMIT`）。pyright 1.1.414 は握手の直後、自動 import の索引ができるまで補完に `{"isIncomplete":true,"items":[]}` で答え、索引ができる頃に最初の診断を送る（実測で両方とも握手から 0.5〜0.7 秒）。#1680 の猶予（2 秒）だけで判定していたので、遅い機・初回で索引が猶予を越えると空のまま確定していた（猶予を 0 にする注入 `TAKO_1977_INJECT_GRACE_MS=0` で実の pyright を測ると、旧は 3/3 で 0 件・次の打鍵で 2 件、新は 3/3 で 2 件）。最初の診断の前に送った要求の空は、診断が先に処理されても 1 回だけ問い直す（#1930 の `EmptyAnswer`）。状態を送るサーバは知らせのとおりで変わらない。注入なしでも、診断が届けば猶予の 2 秒を待たない（実の pyright で 2.0 秒 → 0.54〜0.62 秒）。② **表の版を上げたら古い版の段を片付ける**（`fetch::sweep_old_versions`。判定は `tako_core::lsp::fetch::sweep_plan`）: 今の版の段が済んだとき（取れた・置き場から起こした・`tako lsp install`）、そのサーバの `<id>/` と npm のサーバなら `node/` を見て、**tako の印がある・今の版より前に入った（印の `installed_at`。壊れていれば印の mtime）・生きている tako が使っていない** 段だけを消す（名前を外してから消す = 半端な版を残さない）。サーバを置き場から起こす tako は版の段に使っている印 `.tako-lease-<pid>` を置き、その pid が生きているあいだは消さない（古い GUI が古い版で動いているあいだに新しい tako が消さない）。今の版より後に入った段（新しい tako が入れた）・印の無い段・symlink は残し、消せなければ理由（`failed`）を返す。結果は `tako lsp status` の `fetch.cleanup`（removed / kept / freed_bytes）と `tako lsp install` の各行の `cleanup`。③ 未導入の引き直しの様子を `tako lsp status` の `recheck`（targets / interval_secs / last_cost_ms / count）へ出す（実測: 普通の rc のログインシェルは実時間 0.28〜0.52 秒・CPU 約 0.2 秒 = 15 秒のまま、conda の hook を読む重い rc は 1.8〜2.5 秒・CPU 約 1.5 秒 = 約 100 秒）。合図は UI スレッドの OSC の処理から来るが、ログインシェルは背景のスレッドだけが起こす（番犬 `issue1944_lsp_fetch_watchdog` + e2e で 1 秒の解決でも解決は背景の `lsp-recheck` で走る）。MCP は `tako_lsp_server` の応答に足しただけ（ツール・引数は増やさない。カタログ +126 B）。A/B は `TAKO_1977_LEGACY=1`（猶予だけ・片付けない・固定の 15 秒）。Windows は偽サーバの e2e が根拠で、実機で配布元から取って起こしたことが無いので MATRIX は Pending（実機の手順は #1977 のコメント） | M |
| FR-3.42 | **参照検索・呼び出し階層・型階層**（#1685 / #1007 S8）: コードの識別子の右クリックメニュー（FR-3.36。移動の後・ホバーの前）に**「参照を検索」「呼び出し元を表示」「呼び出し先を表示」「スーパータイプを表示」「サブタイプを表示」**を、サーバが申告した能力（`referencesProvider` / `callHierarchyProvider` / `typeHierarchyProvider`）のぶんだけ出す（`#include` のパスの上には出さない）。押すと右パネルの **references ビュー**（結果があるときだけ出る 6 本目のタブ。`tako panel --view references`）を開いて「問い合わせています」を出し、答えを**問い合わせたペインの結果**として置く（同じペインの次の検索で置き換わる・**ペインを閉じたら消える**）。参照は `textDocument/references`（宣言を含める）の `Location[]` を重複を畳んで（ファイル, 行, 桁）で並べ、**保持の上限 2,000 件**（全件数は `total`・切った数は `truncated`）。一覧はファイルごとの見出しの下に `行:桁 抜粋`。階層は prepare（`textDocument/prepareCallHierarchy` / `prepareTypeHierarchy`）の根それぞれを**1 段だけ開いた**ツリーで返し、呼び出し元のノードは**呼び出しの位置**（`fromRanges` の先頭）へ・それ以外は定義（`selectionRange`）へ着地する。子を読んでいないノードの展開は受けた要素（`data` を含む）をそのまま送り返して 1 段読む（上限: 1 回 500・全体 2,000 ノード・深さ 32）。**展開状態の正本は `tako_core::lsp::xref::Tree`**で、画面の開閉の印も CLI / MCP の `expand` / `collapse` も同じ口を通り、`tako lsp results` の `expanded` で読める（#1407 と同じ穴を作らない）。**行を押す（⌘クリック・Windows は Ctrl+クリックも同じ）と定義ジャンプと同じ着地**（FR-3.31 の `plan_landing` を `dispatch::land_on_place` の 1 本で共有: 同じファイル = 同じペイン / 同じタブで開いている = そのペイン / それ以外 = 新しいペイン。ジャンプ履歴へ積む）。問い合わせ・子の読み込みは定義ジャンプと同じく background（`prepare_offload` → `run_staged` → `finish_offload`。UI スレッドで待たない。上限 60 秒 = `TAKO_LSP_XREF_TIMEOUT_SECS`）で、読み込み中のサーバの空の答えは済むのを待って問い直す（#1930 の `wait_after_empty`）。0 件・階層の対象でない位置は `status: none` と理由・次の一手、未対応・未導入・未応答・落ちた・止めたは定義ジャンプと同じ status で返し、失敗の答えは置かない（前の結果を残す）。握手の能力の読み直しで `lsp-types` 0.97 が知らない `typeHierarchyProvider` を落としていたのを、型に無いキーを生の値のまま残す形に直した。操作は dispatch `LspXref { action, pane, line, column, show, limit }` / `LspResults { action, pane, item, open, focus, limit }` + CLI `tako lsp references\|incoming-calls\|outgoing-calls\|supertypes\|subtypes --pane N --line L --column C [--show] [--limit N]` / `tako lsp results\|expand\|collapse\|open-result\|clear-results [--item N] [--pane N]` + MCP `tako_lsp` の同名の action（新しいツールは増やさない。results 系の `pane` は結果を問い合わせたペインで、省略は右パネルの結果 = 呼び出し元では埋めない）。保持件数の口は応答の `retained` / `retained_rows`。A/B は `TAKO_1685_LEGACY=1`（メニューに項目を出さない・能力の読み直しを旧形へ。CLI / MCP は残る）。リネーム・コードアクション・シンボルの一覧は別 Issue（#1686 / #1687 / #1689） | M |
| FR-3.43 | **ワークスペース横断検索**（#1694 / エピック #1647 の SE-6。⌘F（#195）は開いている 1 ファイルの中だけ）: macOS ⇧⌘F / Windows Ctrl+Shift+G（Ctrl+Shift+F はファイル内の検索 = FindPreview が先に取っているので空いている G = grep。Ctrl+G と同じ C0 バイトへ潰れるので端末の Ctrl+G は奪わない）・編集メニュー・パレットで右パネルの **search ビュー**を開いて検索欄へフォーカスし（プレビュー / ターミナルの 1 行の選択があれば検索語へ入れる）、Enter で探す。**範囲**はそのタブのファイルツリーのルート（サイドバーと `tako tree git-status` と同じ `tree_roots_of_tab` の 1 実装。入れ子のルートは 1 つにまとめる。**ホームとファイルシステムのルートは暗黙の範囲にしない** = 新しいタブの cwd は既定でホームなので、何気ない ⇧⌘F + Enter がホーム全体を上限まで走査しない。外した結果ルートが無ければ理由と次の一手（cd / `tako tree add` / path）を返す）か `path`（明示ならホームも探す）。**走査**は `tako_core::workspace_search`（ripgrep の walker = `ignore` クレート + `regex`。外部コマンドに依存しない）: `.gitignore`（入れ子・否定・`.git/info/exclude`・global excludes）は **git のリポジトリの中でだけ**効く（ripgrep の既定。ホームの `.gitignore` が全部を隠す事故を作らない）/ 隠しファイルは探し、`.git` `.hg` `.svn` `.jj` の中は探さない / 先頭 8000 バイトに NUL があるファイル（git と同じ判定）と 8 MB を超えるファイルは探さず数だけ `skipped` に載せる / シンボリックリンクは**ファイルだけ**辿る（ディレクトリは辿らない = ループもワークスペースの外への脱出も無い）/ 照合は行ごと（改行を跨がない。検索語に改行は不可。幅 0 の一致は数えない）・CRLF の `\r` は落とす。**条件**は `regex`（既定 false = 文字どおり）と `case_sensitive`（既定 false = VS Code / Zed と同じ）で、画面は検索欄の右の `Aa` / `.*`（⌥C / ⌥R）。**結果**は 1 行 1 件（パス・行・最初の一致の桁 = `tako open --line --column` と同じ 1 始まりの文字数・抜粋 200 字・抜粋の中の一致の範囲）で、（ルートの順, パス, 行）に並ぶ（並列度に依らない）。**上限**: 保持 `MAX_RESULTS` = 2000 行・所要 30 秒・走査のスレッド 4 本。上限で止めたら `stopped`（limit / time / cancelled）に理由が載り、画面にも出す（黙って欠けない）。**UI スレッドで走査しない**: dispatch の `prepare_offload`（範囲の解決と、出す検索なら置き場への登録だけ）→ background（`WorkspaceSearchJob::run`）→ 出す検索は `finish_offload` で置き場へ置く。GUI の Enter も `dispatch_off_ui` で同じ 3 段（番犬 `issue1694_workspace_search_watchdog`）。**置き場**は `SearchStore`（右パネルに出している最後の 1 回ぶん）: 次の検索で前を取り消し（追い越された答えは置かない）、× / `clear` で走っている検索も取り消して手放す（`retained` 0）。壊れた正規表現は理由を置き場に残して画面に出す。**結果の行を押す**（⌘クリックも同じ）と、定義ジャンプと同じ規則（`plan_landing`: 起点のペインが同じファイル = そのペイン / 同じタブで開いている = そのペイン / それ以外 = 新しいペインを右へ）で `open_file` の行指定（= ジャンプ履歴へ積む）を通して開く。**CLI / MCP**: `tako search <検索語> [--path P] [--regex] [--case-sensitive] [--show] [--limit N]` / `--results` / `--open N [--focus]` / `--clear` = MCP `tako_search`（action = search / results / open / clear）。出さない検索（`show` なし）は応答を返すだけで画面の結果を上書きしない（AI の検索がユーザーの結果を消さない）。CLI の出力は grep と同じ `パス:行:桁: 抜粋`（パスは自分の cwd からの相対 = ターミナルのリンクとして開ける）。横断置換・LSP の `workspace/symbol`（#1689）はこの範囲の外 | M |
| FR-3.44 | **tako の外で変わったファイルを言語サーバへ知らせる**（#2034。FR-3.28 の続き）: `initialize` で `workspace.didChangeWatchedFiles`（`dynamicRegistration` + `relativePatternSupport`）を申告し、サーバが `client/registerCapability` で登録したグロブ（rust-analyzer なら根ごとに `**/*.rs` / `**/Cargo.{lock,toml}`）をサーバごとに覚え、その**根だけ**を OS のファイル監視（`notify` = macOS は FSEvents・Windows は ReadDirectoryChangesW。**走査しない**）で見て、当たった変化を `workspace/didChangeWatchedFiles`（URI + 作成 1 / 変更 2 / 削除 3）でまとめて送る（静かになって 100ms・続いても 1 秒ごと = `git checkout` の何百ファイルを 1 通に）。申告しないと開いていないファイルの変化を拾えるかは**サーバ自前の監視次第**で、rust-analyzer 1.95 は根がシンボリックリンクを通ると（macOS の `/var` → `/private/var` 等）`cargo fmt` / `git checkout` / 別エディタの書き換えの後も古い行番号で参照・定義を返す（実測。実パスの根では自前の監視が拾う）。グロブの読み取り・照合（`globset`。`*` は区切りを越えない・Windows は大文字小文字を区別しない）・根の整理は `tako_core::lsp::watch`、監視の実体は `tako_control::lsp::watch`（**manager 全体で 1 つ** = 動いているあいだのスレッドは監視 1 本 + まとめ送り 1 本で、サーバの本数に比例しない。どのサーバも登録していなければ落とす）。種別は**最後に在るか**で決める（OS のイベントの種別は rename / 削除 + 作成で取り違える）。**HOME とその祖先・ファイルシステムの根は監視しない**（FSEvents は非再帰でも配下全体のイベントを届けるので費用が減らない）、`.git/objects` / `.git/subtree-cache` / `.hg/store` は知らせない（VS Code の既定と同じ）。登録はプロセスのものなので、落ちた・止めた・起こし直したら捨てる。macOS の FSEvents が実パス（`/private/var/…`）で知らせるのは、渡した綴りへ戻してから照合・送信する（サーバの知っている URI と食い違わない）。**開いている文書**は従来どおりクライアントのもの: エディタの読み直し（FR-3.15 / #1659。未編集なら追従・編集中なら競合）が本文を替え、`didChange` で送る（知らせは VS Code / Zed と同じく開いている文書にも送り、サーバが捨てる）。様子は `tako lsp status`（MCP `tako_lsp_server` の `status`）の `file_watch`（根・監視しない根と理由・振り分けた回数・溢れた回数）と各サーバの `watched_files`（登録・グロブ・送った回数・変化の数・送れなかった数）。A/B は `TAKO_2034_LEGACY=1`（申告しない・登録を受けても見ない）。e2e `issue2034_lsp_watched_files`（偽サーバの `--disk-model` = 実サーバと同じ持ち方）・実経路 `scripts/test-lsp-watched-files-2034.sh`（`TAKO_2034_REAL=1` で実の rust-analyzer）。**既知の限界**: OS がイベントを落とした（Windows のバッファ溢れ等）ときは走査で取り戻さない（`overflows` に数えるだけ）・Linux の inotify は再帰の監視に配下のディレクトリを辿る（配布対象外）（✅ 2026-10-14、#2034） |  |
| FR-3.45 | **名前の変更（リネーム）**（#1686 / #1007 S9）: `textDocument/prepareRename` → `textDocument/rename` → `WorkspaceEdit`（`changes` と `documentChanges` の両方を読む。両方あれば `documentChanges` だけ）。**既定は dry-run**（#496 の git 操作と同じ作法）: CLI `tako lsp rename --pane N --line L --column C --new-name NAME` / MCP `tako_lsp` の `action=rename`（`new_name`）は何も書き換えずに `status: dry-run`・`file_count`・`edits`・ファイルごとの `places`（行・桁・前・後）・`applicable` を返す（編集モードも変えない）。`--yes` / `confirm=true` で頼んだペインの編集バッファへ当てる（`TextBuffer::apply_changes` の 1 実装 = **undo 1 回で戻る**・保存はしない・頼んだときの版にだけ当てる = 進んでいたら `stale`）。**段階 1 は頼んだペインのファイルだけ**: 別のファイル（別のペインで開いていても = 応答の `open_in`）・ファイルの作成 / 名前の変更 / 削除に及ぶなら `--yes` でも当てず `not-applied`（dry-run でも `applicable: false`）と `blocked`・理由・次の一手を返す（複数ファイルの適用・undo・競合検知は段階 2）。`prepareRename` が断った位置（`null` / エラー）では `rename` を送らない（`rejected`）。新しい名前は空・制御文字だけを tako が断り、識別子として正しいかはサーバが決める（`refused` とサーバの文）。能力（`renameProvider`）の無いサーバは問い合わせずに `unsupported`。GUI は右クリックメニューの「名前の変更…」→ タイトルの下の帯の欄（今の名前で開く。Enter で当てる・Esc でやめる。IME の宛先は欄）。上限は `TAKO_LSP_RENAME_TIMEOUT_SECS`（既定 60。起動を含む）。A/B は `TAKO_1686_LEGACY=1`（メニューに項目を出さない）。設計は `.agent/plans/2026-09-lsp-s1.md` §27 |  |
| FR-3.46 | **Arduino のスケッチ（`.ino`）の言語サーバ**（#1951。エピック #1943 の S4。FR-3.28 / FR-3.41 の続き。設計 `.agent/plans/2026-10-language-coverage.md` §5.3）: `.ino` を **arduino-language-server（ALS）** の行が受け持つ（languageId `cpp`。clangd の行には入れない = 単体の clangd は `.ino` を解けず、arduino-cli の compile db を渡すと正しいスケッチに嘘のエラー 10 件）。起動引数は `-cli <arduino-cli> -clangd <clangd> -cli-config <空のファイル>` で、検出表の `args` を `Arg`（固定の文字列 / 伴走の実行ファイル / tako が用意する空のファイル）の列にして表の行だけで書く（組み立ては `tako_core::lsp::launch` の純粋関数 1 つ。`-cli-config` が無いと ALS は即終了し、0 バイトのファイルで足りる = 実測。置き場は `<data_dir>/lsp-config/<id>/`、在れば触らない）。**探す順**: サーバは 差し替えの env（`TAKO_LSP_BIN_ARDUINO_LANGUAGE_SERVER`）→ ログインシェルの PATH → **Arduino IDE の同梱**（行の `bundled` = `tako_core::app_bundle::ARDUINO_IDE`。Code Runner の #1950 と同じ 1 実装）。伴走は **見つけた ALS と同じディレクトリ** → PATH → 同梱（IDE が並べる版の組み合わせを先に採る）。状態の `source` は `bundled`。**ルートはスケッチのフォルダ**（行の `root_scope = FileDir` = ファイルのディレクトリ。印も `.git` も見ない）。**基板（`sketch.yaml` の `default_fqbn` か `default_profile`）が無ければ起こさない**（基板の無い ALS は 15 秒待っても診断を 1 回も送らずホバーも時間切れ = 黙って何も出ない）: 状態 `unconfigured` で `reason`（Code Runner の「基板が選ばれていない」と同じ文 = `runner_project::board_not_selected_note`）/ `next_step` / `setup_command`（`arduino-cli board attach -b <基板> <スケッチ>`。伴走が ALS と同じ場所や同梱にあればフルパス、PATH なら名前のまま = #322）を `tako lsp status` / MCP `tako_lsp_server` / 画面の帯（見出し + 理由 + 次の一手 + 「もう一度確かめる」= manager の restart）に出す。確かめ直すのは シェル統合の合図（tako のターミナルで board attach を打てば起きる。読むのはファイル 1 つだけで背景のスレッド）・開き直し・restart。前提が未設定の器への問い合わせ（ホバー・定義ジャンプ・補完・メニュー）は待たずに `status=unconfigured` で返る。基板の正本は sketch.yaml のまま（`-fqbn` は渡さない）。**ALS が無い**（PATH にも同梱にも無い）は未導入で、理由は同梱も探したことを名指し、導入コマンドは IDE ごと（macOS `brew install --cask arduino-ide` / Windows `winget install --id ArduinoSA.IDE.stable`。ALS 単体の配布は無い）。**伴走が無い**（`Launch::CompanionMissing`）も未導入と同じ扱いで、理由に見つけた ALS のパスと伴走の名前（応答の `missing_companion`）。**スケッチ内の `.h` / `.cpp` も ALS**（#2113）: 行の `folder`（`servers::FolderClaim` = 本体 `ino`・入れ子 `src`・受け持つ C / C++ の拡張子）で、ファイルのあるフォルダか上へ辿った `src/` の親に `<フォルダ名>.ino`（大文字小文字まで一致）があれば拡張子だけの行（clangd）より先にこの行を採る（ルートはそのフォルダ = `.ino` と同じ器）。ALS は同じ器で先に `.ino` を開いた順でだけ正しく解く（`.h` だけ先だと疑わしい 1 件 = 実測）ので、利用者が本体を開いていなければ **tako がディスクから読んで仲間より先に `didOpen`** する（持ち手なし・最後の仲間が閉じたら `didClose`・利用者が開いた本体を閉じても仲間が開いていれば器に残して本文をディスクへ戻す・起こし直しでも本体が先。診断は持ち手のペインにしか出ないので画面に出ず、ユーザーのファイルも書き換えない。`tako lsp status` の `hidden_documents` が隠れた本体の数で、`documents` / `diagnostics` には数えない）。表の解決がファイルシステムを見るので、**開いている文書は開いたときに決めた器で引く**（`LspManager::resolve`。ホバー・補完・定義ジャンプの有効判定・状態の帯・メニュー・整形・名前の変更が同じ口 = 開いた後に本体を足し引きしても器がぶれない）。基板が無ければ `.h` も `unconfigured`（色だけ）、スケッチでないフォルダの C / C++ は今どおり clangd。A/B は `TAKO_2113_LEGACY=1`（拡張子だけで引く）。常駐は ALS + clangd の別プロセスで実測 72.3 MB（tako 経由 = ALS 20.0 MB + clangd 52.3 MB。使い捨てのクライアントでは 83.4 MB。arduino:avr・13 行のスケッチ・読み込み後 3 秒。arduino-cli は常駐しない = 組み直しのたびに起きて終わる。tako のヒープではない）。MCP はツールを増やさない（`tako_lsp_server` の説明に unconfigured を足しただけ）。検証の注入 `TAKO_1951_INJECT_NO_BUNDLE=1`（同梱を探さない = IDE の無い PC と同じ解決）。Windows は IDE 同梱の置き場（`%LOCALAPPDATA%\Programs\Arduino IDE\…` / `%ProgramFiles%\Arduino IDE\…`）と `.exe` の伴走を macOS の単体と偽の同梱の e2e で固定しただけで、実機は未確認（`tako platform` の `tako_lsp_server` の Pending に申告） | M |
| FR-3.7 | サイドバーは両方とも折りたたみ可能。閉じれば純粋なターミナル | M |
| FR-3.8 | Web ビューペイン: 分割ペインの一種としてブラウザ表示（webview）を持ち、任意の URL を開ける（dev サーバーのプレビュー、ドキュメント提示。AI からの操作は FR-2.5.12）。**✅ 2026-07-13（#155）wry = OS ネイティブ WKWebView で実装**。ユーザーはペイン内でクリック・スクロール・文字入力（IME 込み）を直接行える。ページはペインから独立に管理され、ー = dock 退避（ページ生存 = ブラウザタブの維持）/ × = 完全破棄。ステータスバー 🌐 → dock パネルで一覧・ワンクリック復帰・破棄。タイトルバーに ← / → / ⟳ とタイトル + URL 表示。layout.json 永続化（再起動で URL を開き直し）。dispatch `Web`（open / list / show / hide / close / navigate / eval / eval_result / read）+ CLI `tako web` + MCP `tako_web` で 1:1 公開（開発不変条件）。ページ内の AI 操作（クリック・入力・テキスト取得）は eval の JS で行う。**キー入力の宛先（#1481）**: ページをクリックすると打鍵は OS から直接 webview へ行く（AppKit の first responder が移る）ため、tako 側のフォーカス操作（ペインのクリック・⌘数字・`tako focus`・タブ切替）と Web ビューの破棄では**宛先を GPUI へ戻す**（戻さないとターミナルへ戻っても打鍵がページに吸われ続ける）。戻すのは「入力対象が変わる経路」= `clear_text_input_focus`（#503 と同じ集合）と破棄の直前で、**毎フレームの状態判定にはしない**（「宛先が webview でフォーカスは別ペイン」は「いまページをクリックした」と同じ状態なので、level で戻すとページへ文字が打てなくなる）。**窓を閉じた後（#1837）**: 最初の窓を閉じる（#339）・macOS で最後の窓を閉じて Dock から開き直す（#381）と、Web ビューは**いま開いている窓**へ付け替わり、次の `open` もその窓に開く。窓が 1 枚も無いあいだの `open` は理由つきのエラー（プロセスは落ちない）。`web list` の `attached` = いま開いている窓の子か。**Windows で開いても落ちない（#724）**: 生成は gpui の App の借用の外で 1 本ずつ行い（`open` はすぐ `state: queued` で返り、作り終えると `ready`。それまでの eval / focus は理由つきで断る）、Windows では ipc を付けない（タイトル・URL は 2 秒ごとの eval が拾う）。生成中（最初の読み込みまで）に tako が落ちた Web ビューは次の起動で自動では開かず**保留**（`state: held`。ペインか dock に URL を残し、「開く」/ `web navigate` で生成し直す）。`web list` / `read` / `open` / `navigate` の `state` = ready / queued / creating / held / failed。詳細は `architecture.md`「Windows の生成は App の借用の外で 1 本ずつ（#724）」。**要素の全画面表示（Fullscreen API・動画の全画面ボタン）は切る（#2445）**: wry 0.56 から macOS で常に入るが、全画面のあいだ WKWebView が gpui の描画ビューから外れて位置合わせ・閉じる・キー入力の宛先の前提が崩れるので、手当てが済むまで 0.55 と同じく入れない（ページの `document.fullscreenEnabled` は偽）。理由は `architecture.md`「要素の全画面表示は切る（#2445）」。**Windows のユーザーデータの置き場と生成の失敗の案内（#2431）**: WebView2 のユーザーデータは `%LOCALAPPDATA%\tako\WebView2`（隔離・テストは data dir の下）へ置き、実行ファイルの隣（`tako-app.exe.WebView2`）にある旧いデータは自動移行（`migrate` の `webview2_data`）が丸ごと移す（Web ビューが開いていて移せないあいだは旧い置き場を使い続け、次の起動で移す）。生成の失敗は「ランタイムが無い / 古い（145.0.3800.47 未満）/ その他」に分けて次にやることを案内し（`web list` / `read` の `create_failure`）、check-health の `webview2` 節と診断項目に WebView2 ランタイムの版を出す（macOS は対象外）。詳細は `architecture.md`「WebView2 のユーザーデータの置き場と生成の失敗の案内（#2431）」。実装詳細と z オーダー制約は `architecture.md`「Web ビューペイン」節 | S |
| FR-3.9 | diff ビューア: git diff / コミット差分の表示ペイン（AI の成果物提示 `show_diff`（FR-2.7.2）の表示先。✅ 2026-06-14 パネル内インライン表示として実装。ペイン種別版は後段） | M |
| FR-3.10 | 画像プレビュー: ペインのプレビュー種別として画像（PNG / JPEG / SVG / GIF / WebP）を表示できる。`show_file` 系の MCP 操作（FR-2.7.1 / FR-2.5.11）は画像ファイルにも対応する | S |
| FR-3.11 | **ファイルツリーからの D&D プレビュー**: ファイルツリーのファイル行を**ドラッグ&ドロップで任意のペイン位置に落とし、その位置にプレビューペイン（FR-3.2 / FR-3.3）として開ける**。ドロップ位置の象限（上下左右）= 分割方向、**中央 = 既存セマンティクス**（対象がプレビューなら差し替え / 同タブの既存プレビュー再利用 / 無ければ右分割）。**ドラッグ中はドロップ先ハイライト + 挿入プレビュー必須**（FR-2.16.10 と同じ表示系）。同等操作は `OpenFile` の `direction` 拡張 = CLI `tako open --right/--down/--up/--left` + MCP `tako_open_file` の `direction`（開発不変条件。✅ 2026-06-13） | M |
| FR-3.12 | **ファイルツリーの右クリックコンテキストメニュー**: VSCode 風。相対/絶対パスコピー・ファイルマネージャ表示・ターミナルで開く・名前変更・新規ファイル/フォルダ・削除（ゴミ箱）。全操作は dispatch `FileOp` + CLI `tako file` + MCP `tako_file_op`（計 23 ツール。開発不変条件）。**「ゴミ箱へ移動」は必ず復元可能でなければならない**（macOS = Finder 委譲 / Windows = `SHFileOperationW` + `FOF_ALLOWUNDO`）。UI にもコマンドにも確認ダイアログが無い操作なので、**ゴミ箱の概念が無い環境では削除へ劣化させずエラーにする**（#617）。ファイルマネージャ・ごみ箱の**呼び名は OS で出し分ける**（Finder / エクスプローラー、ゴミ箱 / ごみ箱）。判定は境界 B8 の `os_integration::file_manager()` 1 か所で、ラベルは値を受け取るだけにする（表記と実装のずれを構造で防ぐ。✅ 2026-08-26、#617）。**「Finder で表示」はどの入口からも 1 実装**（#2247）: ツリーの右クリック・ターミナルのパスの ⌘+右クリック（FR-1.7.1）・プレビューのペインの右クリック（ヘッダ / 本文 = FR-3.36）は `TakoApp::reveal_in_file_manager` から、設定画面の「設定ファイルを Finder で表示」と右パネルの添付（FR-2.40.11）はそれぞれの画面から、どれも dispatch `FileOp`（op=reveal = CLI `tako file reveal` / MCP `tako_file_op`）を通り、失敗（開いた後に消えた・動いたファイル等）は押した画面の通知（設定画面はその画面のメッセージ）と persist.log へ出す。dispatch の先の実体は `os_integration::reveal` の 1 つで、**macOS は tako の GUI 自身から `NSWorkspace activateFileViewerSelectingURLs:` を呼び、その前に Finder へ前面を譲る**（`yieldActivationToApplicationWithBundleIdentifier:` = macOS 14 以降。`platform::finder_reveal`）。以前は子プロセスの `open -R` に任せていて、本番では `open` が成功（終了状態 0）していたのに Finder が前に出てこなかった（macOS 14 からの協調的なアクティベーションでは、子の `open` は前面のアプリではないので前面を譲れない、という見立て。本物の Finder での確認は install 後）。また以前はプレビューのメニューと設定画面だけが `os_integration::reveal` を直に呼んで結果を捨てていたので、消えたファイルのプレビューで押しても何も起きなかった。**CLI は GUI へ頼むだけで自分のプロセスからは Finder を呼ばない**（GUI が居なければ他の操作と同じく「繋がらない」で断る。前面のアプリである GUI から頼まないと前に出られないため）。Windows は従来どおり `explorer.exe /select,`。番犬 `issue2247_preview_reveal_watchdog`、URL の組み立ては `finder_reveal` の単体、実経路は `scripts/test-preview-reveal-2247.sh` + visual-test `preview-reveal-2247`（`TAKO_REVEAL_RECORD=<ファイル>` を渡すと NSWorkspace へ渡す直前の URL を 1 行書くだけで Finder を開かない = 検証で本物の Finder を出さない口）、A/B は `TAKO_2247_LEGACY=1`（子の `open -R` と、プレビュー・設定画面の直呼びへ戻る）。**名前変更・新規ファイル / フォルダのインライン入力は、開いた直後から打鍵・⌘V・IME の変換がすべて入力欄へ向く**（#1725）: 変換の宛先は `AppTextInput::TreeName`（コミット欄・ブランチ欄・返答欄と同じ器。以前はインライン編集だけ宛先から外れていて、かなモードの変換が隣のターミナルペインに束縛され、下線・候補窓がターミナルのカーソル位置に出て、未確定のまま確定した文字列はターミナルの PTY へ流れていた）。未確定文字列とキャレットは入力欄の中に共有部品で描き、長い名前はキャレット側を残して `…` で詰める（候補窓がサイドバーの外へ出ない）。挿入は 4 経路とも `tree_name_insert` の 1 関数、状態は `TextField`、開く / 閉じるは `open_inline_edit` / `close_inline_edit` の 1 本ずつ。Esc・空 Enter・確定で閉じたら打鍵は開いたときのペインへ戻り（CLI / MCP の focus で動かされていても戻る。右クリックで立った行の選択（FR-3.34）も畳む = 閉じた直後の Enter / ↑↓（FR-3.40）・⌘C / ⌘V をツリーへ向けない。#1938。A/B は `TAKO_1938_LEGACY=1`）、入力欄の外の押下（`on_mouse_down_out`）でも閉じる（押した先が次のフォーカスを決める）。**CLI / MCP の focus / tab / panel 操作では閉じない**（エージェントの操作がユーザーの入力中の名前を消さない）が、ツリーを閉じる・タブを切り替えて作成先がルートから外れるなど**見えなくなった入力欄は打鍵も変換も奪わない**。操作の実体は従来どおり dispatch `FileOp`（新しい操作は足していない）。機械検証はセルフテスト項目 154（実マウスの入口 × 36 回 × 出力が流れている最中・A/B は `TAKO_1725_LEGACY=1`）+ 番犬 `issue1725_tree_inline_input_watchdog`、実 IME の見た目は `manual-checks.md` | M |
| FR-3.13 | **D&D パス挿入**: ファイル・フォルダをターミナルペインへ D&D するとパス文字列を PTY に send。プレビューペインなら FR-3.11 の既存挙動を維持。**ドラッグ元はファイルツリーと Finder / エクスプローラー等の外部アプリの両方**（外部は gpui の `ExternalPaths`。複数選択は空白区切り、クォートは `tako_core::quote_paths_for_shell` の 1 実装、送達は既存の `Request::Send` 経路なので直接ペイン / 器つきペインの双方で効く。✅ 2026-08-29、#1043）。**外部ドラッグは `on_drag` を通らない**（gpui が `FileDropEvent::Entered` で `active_drag` を自分で立てるだけ）ので、ドロップ先オーバーレイの生成を内部ドラッグの種別（`drag_kind`）だけで決めてはならない —— 決めると `on_drop::<ExternalPaths>` が描画ツリーに存在せず**無反応**になる（#1043 の根因）。判定は純関数 `drop_overlay_kind` に集約し、セルフテスト項目 136 が合成 `PlatformInput::FileDrop` で実経路を押さえる | M |
| FR-3.14 | **PDF・画像プレビューのコンテンツズーム**: ペイン寸法を変えず、ピンチ・⌘+/⌘-・修飾キー付きスクロールで 25〜400% に拡大縮小する。2 本指スクロールでパン、⌘0 / ヘッダ倍率クリックで幅フィット（100%）+ パン位置をリセットし、ヘッダに倍率を表示する。PDF の文字選択座標とラスタライズ解像度はズーム・パンへ追従する。同等操作は dispatch `PreviewView` + CLI `tako preview` + MCP `tako_preview_view`（全 75 ツール）で倍率・ページ・パンを 1:1 公開する（✅ 2026-07-14、#234） | M |
| FR-3.14.1 | **コード / Markdown プレビュー（エディタ）の本文はペインの文字サイズに従う**（#1772）。⌘+ / ⌘- / ⌘0（Windows は Ctrl）はコード / md のプレビューでは FR-3.14 の内容ズームへ分岐せず、ターミナルと同じ**ペイン単位の文字サイズ**（8〜32pt・1pt 刻み。別のペインは動かない）を動かす。本文（コード行・行番号・md の段落）の字と行の高さはその値に従い、行の高さは文字サイズの φ 倍（既定 13pt で 21px。ターミナルのセル高 = 1.3 倍とは別の値で、コード行をそちらへ詰めるかは #611 の判断事項）。md の見出し・余白・行間・コードブロックのコピーボタンも同じ文字サイズを基準にする。可視行数（#1741）・追従（#1649）・Page Up / Down・クリック位置・折り返しは描いた行の実寸から数えるのでそのまま追従する（tako-vd 実測: 13 → 16pt で行 21 → 26px・可視 30 → 24 行・1 行を選んだ帯 158×21 → 194×26px・md の段落の行 22 → 27px。10 万行の末尾付近も同じ）。旧実装は本文がペインの値を読まずルートの `theme.font_size` を継承していた（値は 13 → 16 に変わるのに字も行も動かない）。CLI `tako menu invoke 文字を大きく`（`文字を小さく` / `文字サイズを戻す`）/ MCP `tako_menu` が同じ口（ツールは増やさない。編集系の応答の `viewport.visible_lines` で効きを読める）。A/B は `TAKO_1772_LEGACY=1`（✅ 2026-09-28、#1772） | S |
| FR-3.15 | **表示中プレビューのライブリロード**: 表示中のコード / Markdown / 画像 / PDF の外部変更を OS ネイティブイベントで検知し、300ms デバウンス後にすべての読み込み・パース・ハイライト・PDF ラスタライズを background で完成させてから差し替える。スクロール位置・code/Markdown モード・画像/PDF の zoom/pan を保持する。編集モード中は上書きせず FR-3.5 の競合状態で通知する（#1659 から: 未保存の変更が無ければディスクへ追従し、競合にするのは未保存の変更があるときだけ。知らせるのは 1 回）。動画は再生状態保持のため対象外。設定は既定 ON・`settings.json` 永続化で、dispatch `PreviewReload` + CLI `tako preview-reload [on\|off]` + MCP `tako_preview_reload`（全 80 ツール）へ 1:1 公開する（✅ 2026-07-15、#233） | M |
| FR-3.16 | **プレビューの目次ナビゲーション**: Markdown は H1〜H6 の見出しアウトライン、PDF は PDFKit のアウトラインをヘッダのドロップダウンに表示し、項目クリックで対応する見出し / ページへジャンプする。PDF の `n / 全ページ数` 表示はクリックで全ページ一覧を開き、任意ページへジャンプできる。目次は初回ロードとライブリロード時に本文と同じ background 処理で 1 回だけ構築し、render では完成済みデータだけを参照する。dispatch `PreviewOutline` + CLI `tako preview-outline [--pane N] [--item N]` + MCP `tako_preview_outline`（全 84 ツール）へ一覧と 1 始まり項目ジャンプを 1:1 公開し、ページ番号指定は既存 `PreviewView` / `tako preview --page` / `tako_preview_view` を共有する（✅ 2026-07-15、#232） | M |
| FR-3.17 | **プレビュー画像のメモリ上限**: PDF・画像・動画サムネのデコード済み画像をプロセス全体のバイト予算つき LRU で管理する。既定 512MiB、設定範囲 256〜8192MiB。PDF は表示ページの前後だけを遅延デコードし、退避時は tako の `Arc` に加えて GPUI asset cache と sprite atlas の CPU / GPU 資源を明示解放する。動画の置換済みフレームも次フレームで atlas から除去する。close は即時退避、BG 退避は LRU の非参照候補とする。ライブリロードの重量読み込みは `(pane, path)` 単位 single-flight + 最新要求 1 件へ直列化する。dispatch `PreviewCache` + CLI `tako preview-cache [max_mb]` + MCP `tako_preview_cache`（全 88 ツール）へ上限・現在使用 bytes・entry 数を 1:1 公開し、`settings.json` に永続化する（✅ 2026-07-15、#258）。**#2009**: 設定値は天井とし、LRU へ入れる実効上限は「見えているプレビューが描くのに要る量 × 2」（下限 128MiB・天井まで。見えている量そのものは必ず収まる）にする。見えていない（どのウィンドウの表示タブ・ピン・ホバー・開いたドロワーにも描かれていない）プレビューは 30 秒で手放す: デコード済み画像（GPUI の asset と GPU テクスチャ）・PDF は表示中のページと前後以外のページ PNG（並びには同じ寸法の空白で残す）・動画は描画用フレームと CPU の BGRA、止まっていればプレイヤーごと（位置・速度・音量・ミュート・ループを控える）。表に戻したら PDF は同じ表示条件で background 再ラスタ、画像はデコードし直し、動画は控えた位置で開き直す（CLI / MCP の動画操作も同じ 1 本）。再生中の動画は裏でもフレームを写さない（音と位置は進む）。`tako preview-cache` / `tako_preview_cache` の応答に実効上限・見えている / 見えていないぶん・元データ bytes・手放した / 作り直した回数・ペインごとの持ち分（`panes`）を足す。A/B は `TAKO_2009_LEGACY=1` | M |

| FR-3.18 | **Code Runner（再生ボタン）**: プレビューペインで開いているファイルを再生ボタンでワンクリック実行する。ファイル先頭 64 行 / 16 KiB の `tako:run:` 宣言（プロファイル `[name]` 複数定義可、変数展開 `${file}` / `${fileBase}` / `${fileNoExt}` / `${fileDir}` / `${ext}`）→ 拡張子既定（settings.json `runner_defaults` + 組み込み既定）→ CLI `--command` の優先順位で解決。**変数の値は実行ペインのシェルの方言で引用する**（`ShellDialect::quote_fragment`。#1668）: POSIX は単引用符、PowerShell は特別な文字の前にだけバッククオート（値は `-o ${fileNoExt}.exe` のように語の途中にも埋まるので、単引用符で囲むと語の先頭の `'my file'.exe` が文字列のメンバー参照 = 空の引数になる。番犬 `issue1668_runner_quote_dialect_watchdog`）。**組み込み既定は OS 別の 1 枚の表**（`platform::runner_defaults::TABLE`。41 拡張子 × macOS / Windows。#1655）: 初版は macOS の形だけを持っていたため 21 種のうち 10 種（`python3` / `cc` / `c++` / `rustc` + `./<出力>` / `bash` / `zsh`）が Windows で不成立で、`.ps1` / `.bat` のように Windows でこそ走らせたい拡張子には既定が無かった。分岐は `Platform` 引数 + `cfg!`（属性の `#[cfg]` で分けると macOS の単体から Windows の列を検査できない）。Windows 列は実行ペインの PowerShell が 5.1 へ落ちうるので `&&` と `./` を使わず（`gcc ${fileBase} -o ${fileNoExt}.exe; if ($?) { .\${fileNoExt}.exe }`）、その OS で**意図して既定を置かない** 9 マス（Windows の `.zsh`、macOS の `.bat` / `.cmd`、両 OS の `.sql` / `.ipynb` / `.vb`）は理由を日英で持ち、実行コマンドが見つからなかったときの案内へ載る。置かない基準は「**その OS に解釈系が無い / 決まらない**」であって「その拡張子の慣習が無い」ではない（`.command` は macOS 発祥でも中身はシェルスクリプトなので Windows でも `bash`）。cwd はファイルのあるディレクトリが既定（宣言で上書き可）。**プロジェクト既定**（#1656）: 宣言が無く、ユーザーが `tako run-default` でその拡張子の既定を設定していないとき、ファイルから上へ辿って印（`Cargo.toml` / `go.mod` / `package.json` / `pyproject.toml`・`setup.py` / `*.csproj`・`*.fsproj`・`*.vbproj` / `Makefile` / Arduino のスケッチ = `sketch.yaml`・`*.ino`（FR-3.18.4））を探し、種別の既定を**そのルートを cwd にして**走らせる（`source = project_default`）。初版はファイル 1 枚しか見ておらず、cargo プロジェクトの `.rs` を `rustc main.rs -o main && ./main` で走らせて必ず失敗していた。優先順位は `command` の上書き → 宣言 → **ユーザー設定の拡張子既定** → プロジェクト既定 → 組み込みの拡張子既定（明示の設定を tako の推定で上書きしない。空文字列 = 組み込みの無効化はプロジェクト既定を止めない）。種別の表は `tako_core::runner_project::KINDS`（**種別の追加 = 行追加**。表の外に種別名・印を書かない）、上へ辿る範囲は `tako_core::project_root`（HOME の段は見ない = `~/package.json` の置き忘れを拾わない / `.git` の段で止まる / 深さ上限 32。git プロセスは起こさない。LSP のルート検出 #1678 と共有する部品。「このファイルのプロジェクトのルート」は `project_root::detect` の 1 本で、種別の表へ委ねて**実行の cwd と同じ答え**を返す = #1726 の実行設定のキー。述語で辿る `find_upward_map` も同じ境界）。種別ごとの既定: cargo は workspace member なら `cargo run -p <pkg>`（cwd = cargo と同じ規則で見つけたワークスペースのルート。`exclude` されたパッケージは単独）・**bin の無いライブラリは `cargo test -p <pkg> --lib <module>::`**（`cargo run` は「a bin target must be available」で必ず落ちる。Issue の例 `tako-core/src/runner.rs` がこの形）・`--bin` は bin が複数あり `default-run` でないときだけ・examples / tests / benches / `build.rs` はそれぞれの形。Go は `go run ./<pkg>`（main でないパッケージとテストファイルは `go test`）。npm はそのファイルを走らせている script（`start` → `dev` → `serve` → 名前順）があれば `<pm> run <script>`（pm は `packageManager` 欄 → 近いロックファイル → npm）、無ければパッケージのルートから `node <rel>` / `npx tsx <rel>`。Python はパッケージ内なら `-m <module>`（src レイアウトは `src` の下が根）・`test_*.py` / `*_test.py` は `-m pytest <rel>`・解釈系は `uv.lock` → `poetry.lock` → `.venv` → OS の既定（拡張子既定の表の `py` の 1 マスから引く）。.NET は `dotnet run`（テストプロジェクトは `dotnet test`・複数あれば言語の合うものを `--project`）。C / C++ は `run` 規則があれば `make run`、無ければ `make`（`Makefile` 自身も対象）。Windows の列は Python の解釈系（`python` / `.\.venv\Scripts\python.exe`）だけが違い、`Platform` 引数で macOS の単体から固定する。変数 `${workspaceRoot}`（プロジェクトのルート → git のルート → ファイルのディレクトリ）。変数展開の文脈は `runner::RunVars`（#1726 S2 が `${python}` / `${args}` を同じ構造体へ足す）。`RunResolve` / `Run` の応答に `project`（kind / root / marker / command。無ければ null）と `workspace_root` を載せる。入口は `runner::resolve_file` の 1 本で、プロジェクトを見ない `resolve` / `resolve_for` はテスト専用（番犬 `issue1656_runner_project_watchdog` が探索を切る `resolve_file_in` の本番呼び出しと表の外の印も止める）。A/B は `TAKO_1656_LEGACY=1`。`.sql` / `.ipynb` は置かないまま（#1655 からの申し送りを検討した結果: 同じフォルダの `*.db` から推定すると、推定を誤ったときに別の DB へ `DROP` / `DELETE` を流す / ノートブックは実行形が一意に決まらない）。実行は既存 `RunInteractive` と同一の split → spawn 経路（`spawn_command_pane` ヘルパー共通化）。起動コマンドの組み立てと終了コードの受け渡し（#1657 からは側路ファイルが正で、`__TAKO_EXIT=<code>` の画面マーカーは側路へ書けなかったときの退避路。FR-3.18.1）は`platform::shell::run_pane_command`（境界 B1）が決め、`tako:shell` 宣言の包み方は`declared_shell_command` が決める（フラグ `-c` / `-Command` は宣言されたシェル = #875、本体を囲む引用は実行ペインのシェルの方言 = #2286。プロジェクト既定がコマンドへ埋める名前も実行ペインの方言の `quote_fragment`、宣言の `tako:cwd` は両 OS とも引用しない展開で、値全体を囲む単引用符・二重引用符は展開の前に実行ペインの方言の規則で外す = 二重引用符は #2299 まで外れず `"${fileDir}"` が相対パスになっていた。番犬 `issue2286_runner_outer_quote_dialect_watchdog`）。**読む側（`dispatch::find_exit_marker`）は折り返しに耐える**（#651）: マーカーは最短 13 文字（`__TAKO_EXIT=0`）なので、幅がそれより狭いペインでは端末が物理行を割り、`--wait` が永久待機していた（実測: 幅 10 桁で `__TAKO_EXI` / `T=0` に割れて 40 秒返らない。当時のポーリングには上限が無かった = #1662 で FR-3.18.2 の上限を持った）。画面行は「**右端まで埋まっているか**」と対で採り（`TerminalSession::visible_lines_filled` = alacritty の `line_length()` で**列**で測る。文字数で測ると全角プロンプトが載った行を取りこぼす）、**埋まった行の行末 → 次の非空行の行頭**だけをまたいでマーカーを再構成する（幅 6 桁の 3 行割れ・`__TAKO_EXIT=1` + `27` = 127 まで拾う）。数字のあとは行の残りが空白であることを要求するので、ちょうど埋まったマーカー行の次に別の出力（`5 files changed`）が来ても繋がない。A/B は `TAKO_651_LEGACY=1`。起動シェルは FR-2.5.14 の規則（ユーザーの対話環境）に従う（#1031）。dispatch `Run` / `RunResolve` / `RunnerDefaults` + CLI `tako run <file>` / `tako run-default` + MCP `tako_run` / `tako_run_resolve` / `tako_run_defaults`（全 108 ツール）。MCP description に宣言書式を含み、エージェントが参照して正しいヘッダを書ける。INSTRUCTIONS / master・solo prompt にファイル作成時の `tako:run` 付与を行動規範化（✅ M1〜M3・M5・M6、2026-07-22、#453。M4 UI は別 PR）。**`focus` の既定は false = 実行ペインを作ってもフォーカスを移さない**（ユーザーの打鍵を奪わない。`PaneTree::split_with_ratio` が無条件で新ペインへフォーカスを移すため、`spawn_command_pane` が分割前のフォーカスへ戻すことで担保する。`Request::Split` と同一規約。入力待ちの `RunInteractive` と再生ボタンだけが明示 true で移す。✅ 2026-07-30、#676） | M |
| FR-3.18.1 | **実行ペインを積まず、終わりを人の言葉とバッジで伝える**（#1657）。①**再利用**: `Run`（再生ボタン / `tako run` / MCP `tako_run`）は、実行先のタブに**同じファイル + 同じプロファイル**（`run_pane::RunKey`。コマンドで見分けないのは、宣言の引数を直しながら押し直すたびに積む形へ戻るため）の実行ペインがあれば、分割せずに**その位置へ新しいペインを差し込み**、旧ペインは close と同じ後始末（`detach_session` = セッション破棄・器の kill・ペインログ）で片付ける（`PaneTree::replace`）。**同じ PaneId のままセッションだけ差し替えない**: 旧セッションの終了イベントが同じ ID の新しいセッションへ届いてペインごと閉じる競合と、器のセッション名がペイン ID に結び付いていて `new-session -A` が旧セッションへ繋ぎ直す問題があるため。前の実行がまだ走っていれば止めて走らせ直す（IDE の再実行と同じ。応答 `stopped_running: true`。確認ダイアログは出さない = FR-2.22.4 と同じ理由）。応答に `reused_from`（差し替えた旧ペイン。null = 新しく分割）。別タブの実行ペインは使わない（押したのに何も見えない）。`tako run-interactive` とコマンドカードの実行は従来どおり毎回新しいペイン（カードの実行記録は押した回のペインを指す = #1724）。明示的に増やすのは CLI `--new-pane` / MCP `tako_run` の `new_pane`（**ツールは増やさない**）。②**マーカーを画面に出さない**: 実行ペインのスクリプトは終了コードを**側路ファイル** `<data_dir>/run-exit/<pane>.code` へ `<code>\n` で書き（末尾の改行までそろって初めて読める = 書き込み途中を読まない）、画面には「`[tako] 終了コード N / Enter でこのペインを閉じます`」（日英。`platform::shell::run_exit_hint`）を淡色で出す。画面へ `__TAKO_EXIT=<code>` を出すのは**側路を用意できなかった（`run_pane::prepare` が `None`）ときだけ**（#1778。それ以前は「書けなかったとき」= POSIX の `\|\|` / PowerShell の `catch` でも出していた。制約付き言語モードで止められる .NET 呼び出しは使わない = `Set-Content`）。OSC で運ばないのは、tmux では DCS の包み直しが要り psmux は素通ししない（`osc_sink`）うえ、パススルーは GUI が繋がっているときしか届かないため。側路はペインが持つ（`Pane::exit_file`。`split --command` の保持も同じ = FR-2.5.14）。読む側は `dispatch::run_pane_exit_code` の 1 実装（確定済みの値 → 側路。**側路を持つペインでは画面を読まない** = #1778: 実行中のプログラム自身が `__TAKO_EXIT=7` を印字すると、側路が書かれる前に画面から拾って終了を早め、確定は 1 回きりなので偽の値のまま固まっていた。画面のマーカー = #651 の `find_exit_marker` は側路を持たないペインの退避路だけ。代わりに、側路を用意できたのに書けなかった回（ディスク満杯等）は確定せず、`--wait` は上限で打ち切る）で、`RunInteractiveStatus`（`--wait` / `tako_run_interactive_status`）・コマンドカードの実行記録（#1724）・`list` の `run`・GUI のバッジが同じ値を見る。側路は Enter のあとスクリプト自身が消し、差し替え・auto_close では読む側が消し、残骸は次の実行ペインを作るときに「居ないペインのぶんで 24 時間より古いもの」だけ消す（`run_pane::prepare`）。③**バッジ**: 実行ペインのタイトルバーに「実行中」「完了」「失敗 (N)」（`ui_text::pane_menu::run_badge_*`。角丸の面 + 丸 + 文言の描画プリミティブ、絵文字なし）。失敗はヘッダも赤みの面。終了は出力のたびの `settle_run_pane`（見張りの一覧 `run_exit_watch` に居るペインだけ）と 2 秒ごとの `settle_run_panes` で確定する。CLI / MCP からは `tako list` / `tako_list_panes` の各ペインの `run`（`status` / `exit_code` / `outcome` / `command` / `auto_close` / `path` / `profile`。実行ペインでなければ null）。A/B は `TAKO_1657_LEGACY=1`（毎回分割・画面にマーカー・案内なし）。番犬 `issue1657_run_pane_watchdog`、実経路は `scripts/test-run-pane-reuse-1657.sh`（バッジは visual-test の `run-badge` 節）。#1778（split の保持の側路・画面を読まない・`--wait` の 124・`close:auto`）の番犬は `issue1778_run_pane_followup_watchdog`、実経路は `scripts/test-run-pane-followup-1778.sh`（修正前のバイナリでも同じスクリプトが走り、症状は FAIL で出る） | M |
| FR-3.18.2 | **`--wait` は上限つき・auto_close は聞きに来なくても効く・走らせる前に保存する**（#1662）。①**`--wait` の上限**: `tako run --wait` / `tako run-interactive --wait` は実行ペインの終わりを IPC で 2 秒ごとに聞き直す（相手は子プロセスではない）が、#1662 以前は**上限の無い無限ループ**で、サーバーや入力待ちを走らせると CLI が永久に返らなかった（`.agent/conventions.md`「外部コマンドを待つときは上限を持つ」違反）。聞き直しは `tako_core::probe::poll_with_timeout` の 1 実装（眠りは残り時間で頭打ち = 上限を踏み越えない・上限に達した回も打ち切る前に 1 回は聞く = 上限ちょうどに終わったものを取りこぼさない）、上限は `probe::run_wait_timeout`（既定 **600 秒** = このリポジトリの冷えたビルド `cargo build -p tako-cli -p tako-app` 実測 128.8 秒の約 4.7 倍で、`--wait` を叩く AI のシェル実行ツールの上限 600 秒とも揃う。env `TAKO_RUN_WAIT_TIMEOUT_SECS`。**0 / 不正は既定へ落ちる** = 外せない）。超えたら `{"status":"running","timed_out":true,"waited_secs","limit_secs"}` と「まだ実行中です…続きは `tako run-interactive-status <pane>`」を出して **exit 124**（**実行は止めない**）。124 は `timeout(1)` と同じ値で、#1778 以前は exit 1 = コマンド自身の失敗（非 0 はすべて 1 に畳む）と見分けられなかった。コマンドが自分で 124 で終わっても CLI は 1 を返すので、124 は打ち切りだけを指す（結末 → 終了コードの写しは tako-cli の `WaitEnd::exit_code` の 1 か所）。CLI の 2 つの `--wait` は `wait_run_pane` の 1 本へ寄せた。②**auto_close は GUI の終了検知で効く**: #1662 以前は閉じる処理が `RunInteractiveStatus` の中にしか無く、`--wait` 無しの `--auto-close success` は誰も聞きに来ないので閉じなかった。閉じるのは `dispatch::auto_close_run_pane` の 1 実装（判定は `run_pane::InteractiveMeta::wants_close` = success は 0 のときだけ・always は常に・never と不明な綴りは閉じない）で、GUI の出力のたびの `settle_run_pane` の直後と 2 秒ごとの `settle_run_panes` の直後（`auto_close_run_panes` = **確定済みも見る**。`tako list` などの読み取りが先に確定させたぶんを取りこぼさない）、`RunInteractiveStatus` が同じものを呼ぶ。ペインログのクローズマーカーは引き金（GUI の終了検知 / `--wait` の聞き直し）に依らず **`close:auto`**（`CloseOrigin::AutoClose`。#1778 以前は `close:internal` / `close:dispatch(cli)` で、手で閉じたものと見分けられなかった）。閉じたら結末を `Workspace::closed_runs`（`run_pane::ClosedRuns`。上限 64 件・layout.json には保存しない）へ控え、閉じたあとに来た `RunInteractiveStatus`（`--wait` / `tako_run_interactive_status`）は控えから `exited` / `closed: true` を返す（「成功したので閉じた」を「そんなペインは無い」にしない）。タブ最後の 1 枚は閉じない（タブごと消すのは auto_close の範囲を超える）。`run-interactive`（既定 success）も同じ経路で、`--wait` 無しでも成功で閉じる。コマンドカードの実行は `never` 固定なので変わらない。③**走らせる前に保存する**: #1662 以前は再生ボタンだけが GUI 側で保存し、CLI / MCP の `tako run` は保存しなかった（AI が `tako_run` を呼ぶとディスク上の古い内容が走る）。dispatch `Run` の `save_previews_before_run` の 1 実装が、**そのファイルを表示している未保存のプレビュー**（パスは正規化して比べる・起点のペインを先に）を `host.save_preview`（⌘S / `tako edit save` と同じ = リモート由来はリモートへ書けるまで）で保存してから先頭 16 KiB を読む（書き換えた `tako:run:` 宣言も効く）。保存できなければ**走らせない**（「実行前の保存に失敗したので実行しない」）。応答に `saved_panes`。再生ボタンの自前の保存は外した（保存の口が 2 つに割れると CLI / MCP だけ古い内容が走る）。番犬 `issue1662_run_wait_save_watchdog`（上限を外す / 保存を外す / GUI 側の auto_close を外す注入をそれぞれ file:line で落とす）、単体は `probe::tests`（時計を差し替える）と `dispatch::tests::issue1662_*`、実経路は `scripts/test-run-wait-save-1662.sh`（修正前のバイナリでも同じスクリプトが走り、症状は FAIL で出る） | M |
| FR-3.18.3 | **設定なしでプロジェクトの実行環境（`.venv` / uv / poetry / pipenv / conda / pyenv・node の nvm / fnm・Rust の rust-toolchain）で走る**（#1730 / #1735。#1726 S2。設計 `.agent/plans/2026-09-runner-settings.md` §5・§6・§8）。#1730 以前は `.py` が常に PATH の `python3` / `python` で走り、プロジェクトに `.venv` を作っても使われなかった（「import できない」の原因がたどれない）。①**自動選択**: 実行環境の表 `tako_core::runtime_env::KINDS`（#1729 S1。**言語を足す = 行を足すだけ**・表の外に言語名を書かない = 番犬 `issue1729_runtime_env_table_watchdog` が `runtime_env/tier_p.rs` と `tako-control/src/runtime_probe.rs` も見る）の順（uv のプロジェクト → プロジェクト内の venv（近い段が勝つ）→ poetry → pipenv → conda（`environment.yml` の名前が一覧とちょうど 1 つ一致したときだけ）→ pyenv → システム）で 1 つを選ぶ。上へ辿る範囲は `project_root::candidate_dirs`（#1656 と同じ境界）。**interpreter の消えた環境（壊れた `.venv`）は自動で選ばず**次へ進み、理由を `warnings` に載せる（`Detection::demote_missing_interpreters`）。何も無ければシステム = 今と同じ（名前のまま・解くのは実行ペインのシェル）。②**当て方**: 変数 `${python}`（表の `variable`）を足し、組み込み既定の `py` 行を `${python} ${fileBase}` にした（**実行環境が無いときの展開は #1730 以前と両 OS でバイト一致** = `runner::tests::実行環境が無いときのpyの展開は1730以前と両osでバイト一致`）。宣言・ユーザーの拡張子既定・`command` の上書き・プロジェクト既定（#1656 の `python -m <モジュール>`）のどれでも同じ値に展開する。語は置き先のシェルの方言で囲む（PowerShell で囲んだパスは `& '…'`。`tako:shell` の宣言があればそのシェルの方言 = `ShellDialect::command_words`）。activation（PATH の前置 + `VIRTUAL_ENV` / `CONDA_PREFIX` 等）は **`SpawnOptions.env` ではなくコマンド文字列の先頭へ埋める**（器の中へ届き、rc が PATH を組み直した後に効く。境界 B1 `platform::shell::compose_run_script`。前に付けるものが無ければコマンドをそのまま返す）。包む形（`uv run python`）が効かない宣言（`tako:run: pytest`）で activation も無いときは `warnings` で `${python} -m <モジュール>` の書き方を案内する。③**2 段の検出**: Tier F（stat と小さなファイルの先頭読み。`Run` と `RunResolve` の両方で毎回。隔離 GUI の実測 0.08〜0.31 ms / 各段に子 300 を並べたいちばん重い形で 1.6〜2.6 ms = 目標 50 ms の 1/20 以下。assert にはしない）と Tier P（ログインシェルの `command -v` = `platform::exe::find_with_timeout`・`poetry env info -p` / `pipenv --venv`・一覧ファイルが無いときの `conda info --envs --json`・`<interpreter> --version`）。**Tier P は `probe::output_with_timeout` の 1 実装だけを通し**（1 回の上限は `probe::runtime_probe_timeout` = 既定 5 秒・env `TAKO_RUN_PROBE_TIMEOUT_SECS`。0 / 不正は既定）、打ち切ったら `probe.timeouts` と `warnings` に載せる。**`Run` の経路では子プロセスを起こさない**: Tier P の答えは開始ディレクトリごとに「事実」（見つかった道具 / env の一覧 / 環境の置き場 / 版）として覚え、`Run` は毎回の Tier F に重ねるだけ。事実はそれぞれ指紋で古さを見る（道具 = ファイルが在るか / 環境の置き場 = 印のファイルの mtime の組 / 版 = interpreter の mtime）。Tier F の結果は覚えないので `.venv` を作った・消したは次の実行ですぐ効く。テストプロセスでは実物の道具を起こさない（偽の口で検査する）。④**口**: dispatch `Run` は実行環境を自動で重ね、応答へ `runtime`（`{ id, kind, manager, key, label, version, path, program, auto, selected, tier, source, path_prepend, env }`）・`config_sources`（項目ごとの出典）・`warnings` を足す。`RunResolve` は応答へ `runtime` / `runtimes`（候補の全部・自動選択の順）/ `config`（`runner_config::merge` の実効値を項目ごとの `{ value, source }` で）/ `project_root` / `probe`（`tier_f_ms` / `tier_p_ms` / `cached` / `timeouts`）を足し、引数 `refresh`（覚えた答えを捨てて聞き直す）を受ける。**Tier P を含むので `prepare_offload`（`OffloadJob::RunResolve`）で background へ出す**（UI スレッドで採るのは相対パスの基準 = ペインの cwd だけ）。CLI `tako run <file> --list [--refresh]` / MCP `tako_run_resolve`（`refresh`）/ `tako_run`（説明に 1 行）と 1:1。A/B は `TAKO_1730_LEGACY=1`（実行環境の層を丸ごと通さない = `${python}` は `python3` / `python`・前置なし・プロジェクト既定は #1656 の推定）。実経路は `scripts/test-runner-runtime-1730.sh`（隔離 GUI）。**制約**: conda は PATH の前置 + `CONDA_PREFIX` / `CONDA_DEFAULT_ENV` だけで `activate.d` は走らない（設計 J3）/ 再生ボタンのドロップダウンの表示は実行環境を重ねる前の値（`python3 a.py`）のまま = 表示の寄せ先は #1726 S5 / 保存した選択（`run-configs.json`）は #1726 S3 / Windows の置き場（conda の PATH 5 本・pyenv-win）は macOS の表テストだけで実機未実測。⑤**Node / Rust の行**（#1735。#1726 S7。設計 §9.1・§10 S7）: **node**（`.js` / `.mjs` / `.cjs` / `.ts` / `.mts` / `.cts` / `.tsx` / `.jsx`。変数 `${node}`・既定表の `js` / `mjs` 行を `${node} ${fileBase}` にした）は nvm（`$NVM_DIR` → `$XDG_CONFIG_HOME/nvm` → `~/.nvm` の `versions/node/<版>`。Windows は nvm-windows の `NVM_HOME\<版>`）→ fnm（`$FNM_DIR` → `$XDG_DATA_HOME/fnm` → `~/.local/share/fnm` → `~/.fnm` → `~/Library/Application Support/fnm` の在る最初のものの `node-versions/<版>/installation`。Windows は `%APPDATA%\fnm`）→ PATH の `node` の順。版は `.nvmrc` → `.node-version`（近い段が勝ち・同じ段は `.nvmrc` = fnm 1.38.1 で実測）を**数字の成分の前方一致で一番新しい版**へ解く（`VersionMatch::NumericPrefix`。`20` → `v20.11.1`・先頭の `v` は読み飛ばす・版の形でないディレクトリは候補にしない）。`lts/*` 等の別名は解かずに `warnings` で理由を出して自動では選ばず、`system` は飛ばす。**道具の置き場が無ければ版の不足を知らせず、どれかの道具が版を見つけたら他の道具の不足も知らせない**（同じファイルを読む nvm / fnm の使っていない側が騒がない。前者は pyenv の行も同じ規則にした）。`node_modules/.bin` は候補ではなく表の欄 `RuntimeKind::project_bins` で、**どの候補を選んでも** PATH の先頭へ近い段から足す（`npm run` と同じ並び。選んだ候補の置き場はその後ろ = `runtime_probe::Snapshot::path_prepend`）。TS / JSX は既定が `npx tsx` で `${node}` を使わないが、PATH の前置で npx / tsx がその版で走る。volta はシムが `package.json` を読むので触らない（PATH の node = シム）。**Rust**（`.rs`。変数 `${rustc}`・既定表の `rs` 行を `${rustc} …` にした）は `rust-toolchain` → `rust-toolchain.toml`（同じ段に両方あれば rustup と同じく旧い名前 = rustup 1.28.2 で実測）の `channel` / 素の 1 行を候補（`runtimes` / `runtime`）に出すが、**自動で見つけただけなら `RUSTUP_TOOLCHAIN` は入れない**（rustup がファイルを自分で読み、`components` / `targets` / `path` も生きる）。**利用者が明示に選んだ（保存した）ときだけ** `RUSTUP_TOOLCHAIN` で上書きする（`EnvSwitch` の当て方は `runtime_env::explicit_env` が返し、`runtime_probe::Snapshot::env` が保存した選択にだけ重ねる。一覧と同じ鍵を選んでも入る）。プロジェクト種別 cargo の `cargo run` にも単体の `rustc` にも実行ペインの先頭で届く。保存した選択（`tako run-config a.rs --runtime rustup:<toolchain>`）は一覧に無い toolchain でもそのまま上書きする（解くのは rustup）。**実行環境が無いときの展開は #1735 以前と両 OS でバイト一致**（`runner::tests::実行環境が無いときのjsとrsの展開は1735以前と両osでバイト一致`）。Tier P は 1 ファイル = 1 kind で、node は最大 2 本（PATH の node の場所と版）・Rust は 0 本なので**並列化はしない**（理由は設計 §10 S7）。実経路 `scripts/test-runner-runtime-1735.sh`（偽の nvm・本物の fnm を読み取りだけ・A/B は修正前のバイナリ）。**制約**: toolchain を明示に選ぶと上書きの出どころが環境変数になり（rustup 1.28.2 の `rustup show active-toolchain` で実測）、toolchain のファイルの `components` / `targets` は効かない（自動のときは入れないので効く）/ nvm-windows と fnm の Windows の置き場は macOS の表テストだけで実機未実測 | M |
| FR-3.18.4 | **Arduino のスケッチ（`.ino`）を再生ボタン / `tako run` で compile・書き込みできる**（#1950。#1943 S3。設計 `.agent/plans/2026-10-language-coverage.md` §5.4 / §5.5）。#1950 以前は `.ino` が拡張子既定にもプロジェクト種別にも無く「実行コマンドが見つからない」で終わっていた。①**プロジェクト種別 `arduino`**（`runner_project::KINDS` の 1 行。印 `sketch.yaml` / `*.ino`、受け持つ拡張子は `ino` だけ）: 既定は **`arduino-cli compile`**（#322 の最簡形・cwd = スケッチのフォルダ = `.ino` のあるディレクトリ。上の段の `.ino` / `sketch.yaml` を自分のスケッチと読まない）、プロファイル `upload` は `arduino-cli compile --upload`（ポートは `sketch.yaml` の `default_port`）。**拡張子既定の表には足さない**（スケッチはフォルダ単位でしか走らない）。プロジェクト既定の追加プロファイルは `ProjectMatch.profiles` に載り、`resolve_core` が `default` の後ろへ並べる（再生ボタンのドロップダウン・`--profile upload`・`tako_run` の `profile`）。②**arduino-cli の探索**は実行環境の表（`runtime_env::KINDS` の `arduino` 行・変数 `${arduino_cli}`。FR-3.18.3 と同じ層）: PATH（`PathLookup`。`eligible_when_missing: false` = 見つかったときだけ選んで後ろへ譲る）→ **Arduino IDE 同梱**（新しい戦略 `Detector::Bundled`。置き場の知識は `tako_core::app_bundle::ARDUINO_IDE` の表 1 枚、探すのは純粋関数 `app_bundle::find` / `dirs` = `Platform` と `FsProbe` を引数で受ける。言語サーバの伴走 #1951 も同じ 1 実装を使う）→ 名前のまま（表の `fallback`）。macOS は `/Applications/Arduino IDE.app/Contents/Resources/app/lib/backend/resources/` → `~/Applications/…`、Windows は `%LOCALAPPDATA%\Programs\Arduino IDE\resources\app\lib\backend\resources\` → `%ProgramFiles%\Arduino IDE\…`（**未実機** = `tako platform` の `tako_run` / `tako_run_resolve` を Windows で `degraded`・`notes::WIN_RUN_ARDUINO_UNVERIFIED`）。宣言からも `${arduino_cli}` で同じ実体を使える。③**3 つの失敗を人の言葉にする**（日英。`runner_project::ProjectNote`）: 基板が未選択（`sketch.yaml` に `default_fqbn` も `default_profile` も無い → `<arduino-cli> board attach -b <基板> <スケッチ>`）・フォルダ名と `.ino` 名の不一致（arduino-cli は**大文字小文字まで**一致を求める = 大小を区別しない APFS でも `blinkc/Blinkc.ino` は missing。実測 1.2.0。旧形式の `<フォルダ名>.pde` も本体と認める）・arduino-cli が無い（macOS `brew install --cask arduino-ide` → `brew install arduino-cli`、Windows `winget install --id ArduinoSA.IDE.stable` → `ArduinoSA.CLI`）。`upload` だけに効く 4 つ目として書き込みのポートが未選択（`default_port` が無い → `board attach -p <ポート>`）。出し方: 実行ペインでは**コマンドの前に**案内の行を出す（`platform::shell::RunNotice` を `compose_run_script` が env / PATH の後・コマンドの前へ置く。POSIX は `printf '%s\n'`、PowerShell は `Write-Host`。終了コードはコマンド本体のまま）。arduino-cli が無い案内は **`unless_found`** つきで、実行ペインのシェルでもその名前が見つからないときだけ出る（POSIX `command -v` / PowerShell `Get-Command`。GUI の PATH は Dock 起動だと痩せているので、tako が見つけられなかっただけで「無い」と言わない）。案内を出すのはプロジェクト既定で走るときだけ（宣言・上書きが勝ったら出さない）。CLI / MCP は `Run` / `RunResolve` の応答の `project.notes`（`id` / `message` / `commands` / `profile` / `unless_found`）。④**繋がった基板を候補に入れる**: `RunResolve`（background）でだけ、実行環境の層が見つけた arduino-cli の実体へ `board list --format json` を聞き（`ProjectKind::followup` = 表の行が持つ問い合わせ。起こすのは `runtime_probe::apply_project_followup` = Tier P と同じ上限）、FQBN がちょうど 1 つに決まればその基板とポートを案内のコマンドへ入れる（複数なら候補を本文に並べる。`Run` では聞かない）。⑤**基板の正本は `sketch.yaml`**（Arduino IDE / arduino-cli と同じファイル。tako の保存先 `run-configs.json` には持たない。書くのも `arduino-cli board attach`）。`tako setup` の標準の依存表（`setup_deps`）には載せない（Arduino を使わない人にも聞くことになる）。コアの導入（`arduino-cli core install`）は代行しない。基板とポートを選ぶ口（ドロップダウン・`tako run-config --board/--port`・MCP）は FR-3.18.8（#1952）、`.ino` の言語サーバは #1951。A/B は `TAKO_1950_LEGACY=1`（arduino の行を外す = `.ino` は「実行コマンドが見つからない」へ戻る。`ProjectKind::legacy_env`）。単体は `runner_project::tests`（`arduino*`）/ `runtime_env::tests`（`arduino_cli*`）/ `app_bundle::tests` / `platform::shell::tests_1730`（案内を `/bin/sh` へ通す）/ `dispatch::tests`（`run_resolveはスケッチを…` / `runは基板が未選択なら…`）/ `runtime_probe::tests::基板の案内は見つけた実体へ聞いて詰める`。実経路は `scripts/test-runner-arduino-1950.sh`（隔離 GUI・一時の HOME と APFS で複製した data dir の上で compile まで通し、3 つの失敗の案内と `TAKO_1950_LEGACY=1` の旧挙動を見る。本物の `~/Library/Arduino15` の mtime を前後で比べる） | M |
| FR-3.18.5 | **選んだ実行環境・プロファイルをファイル / プロジェクト単位で覚え、`tako run` がそれで走る**（#1731。#1726 S3。設計 `.agent/plans/2026-09-runner-settings.md` §4.2〜§4.5・§5.5・§8）。#1731 以前は選択がメモリ上だけ（`preview_run_selected`）で、CLI / MCP から見えず再起動で消えた。①**置き場**: `<data_dir>/run-configs.json`（`tako-control::run_configs`。`{version, projects, files}`。キーは境界 B26 を通したファイルのパスと、プロジェクトのルート = `project_root::scope_root`（**`${workspaceRoot}` と同じ答え** = 種別の印のルート → 境界内の git のルート → ファイルの段。`runner::resolve_file_in` も同じ `project_root::fallback_root` を通す = テスト `実行設定のプロジェクトのキーはworkspace_rootと同じ`）。`files` は 1,000 件まで（`updated_at` の古い順に落とす。消えたファイルは掃除しない）。書き込みは `config_io` の排他ロック + tmp → rename、読めない内容は `.unreadable.bak` へ退避してから空で始める（文字として読めない中身はバイトのまま番号付きの退避へ移し、理由・退避先を persist.log へ `移行できず: run_configs …（…・発生源 load）` の 1 行 = #2238）。中身がマシン固有のパスなので `settings.json`（Shared）に相乗りせず、共有カタログは `Local`（`notes::RUN_CONFIGS`）。#916: `SchemaId::RunConfigs` + `versioned`（版数 1・Step なし。項目を足すときは serde の default で旧ファイルがそのまま読める）。②**重ね方**: `Run` / `RunResolve` は `run_configs::saved_for` → `apply_runtime` で、ファイル → プロジェクトの順に保存値を重ねる（`runner_config::merge`。応答の `runtime.source` / `config.*.source` が `file` / `project`）。プロファイルは明示（`--profile`）が勝ち、**`command` の上書き（一回だけ全部変える逃げ道）は保存した層を素通りする**（設計書 §4.3。自動の実行環境は効く）。応答の `scope_root` がプロジェクトのスコープのキー。**保存した実行環境が解けなければ（`.venv` を消した・interpreter だけ消えた）自動選択へ落とさず**（#1466）、`Run` はエラーにして本文に次の一手（`tako run-config <file> --reset runtime`）を載せる。一覧（`RunResolve` / `tako run-config`）は止めずに `runtime_unresolved: {manager, key, reason, source, next_step}` を返す。保存したプロファイルが宣言から消えていたら同じく `--reset profile` を案内して止める。③**口**: CLI `tako run-config <file>`（項目なし = 実効値・候補・保存値を読む。`--runtime` は候補の ID（`<検出器>:<鍵>`）・短縮（ディレクトリ名・検出器 ID・版名）・パスを受け、**短縮が複数の候補に当たったら選ばず候補を並べてエラー** = `runtime_env::match_query`。解けない指定は保存しない。`--profile` は宣言にある名前だけ。`--reset [項目|all]`（値なし = 全項目）。保存先の既定は実行環境 = project・他 = file、`--project` / `--file` で明示。`--reset` は保存先を省くと両方から消す = 実効値が自動へ戻る）。MCP は**新しいツールを足さず** `tako_run_defaults` の `path` / `scope` / `profile` / `runtime` / `reset`（`ext` と排他。enum は `RunScope::VALUES` / `RunConfigField::reset_values()` から生成 = #1467）。dispatch は `RunnerDefaults { path, … }` の 1 本で、候補は `Run` と同じ材料（Tier F + 一覧で覚えた Tier P の答え）= 子プロセスを起こさない。A/B は `TAKO_1731_LEGACY=1`（`Run` / `RunResolve` が保存値を読まない）。実経路は `scripts/test-run-config-1731.sh` |
| FR-3.18.6 | **実行設定に引数・環境変数・作業ディレクトリ・実行前コマンドを持ち、`tako run` がそれで走る**（#1732。#1726 S4。設計 `.agent/plans/2026-09-runner-settings.md` §6・§10 S4）。JetBrains の Run Configuration と同じ 4 項目を FR-3.18.5 の `run-configs.json`（形は変えない = `RunConfig` に既にあった欄。#916 の移行なし）へファイル / プロジェクト単位で覚える。①**引数**: シェルへそのまま渡る 1 本の文字列（引用しない = `--name 'a b'` はシェルが割る）。`${args}` でテンプレートの位置を決め、**無ければ末尾へ足す**（宣言・拡張子既定・プロジェクト既定・全プロファイル）。Windows のコンパイル行（`; if ($?) { .\x.exe }`）は末尾だと `}` の外になるので表の側で `${args}` をブロックの中に明示する（テスト「`if ($?) {` を含む行は `${args}` を含む」）。**引数が無ければ `${args}` は直前の空白ごと消え、展開結果は #1732 以前と 1 バイトも変わらない**（両 OS の全行で固定）。②**環境変数**: キー単位で重ねる（ファイルが勝つ）。値はリテラルで、tako の変数（`${fileDir}` 等）だけ引用せずに展開する（`runner::expand_literal`）。名前は識別子（`runner_config::valid_env_name`）だけを保存し、手で書き換えた不正な名前は `Run` が走らせずに止める（一覧は `warnings`）。**`SpawnOptions.env` ではなくコマンド文字列へ埋める**（境界 B1 の `platform::shell::compose_run_script`。器 = tmux の中へ届き、対話シェルの rc の後に効く）。実行環境の env の後ろに置く = 同じ名前なら利用者の値が勝つ。③**作業ディレクトリ**: 宣言の `tako:cwd` より強い（相対はファイルの段から）。**移した先がファイルの段でなければ `${fileBase}` はファイルの絶対パスで展開する**（組み込み既定の `python3 ${fileBase}` が移った先でも引ける = プログラムの場所と作業ディレクトリを独立させる）。プロジェクト既定はルート基準で組んであるので、移したときは一覧の `warnings` で知らせる。今無いディレクトリは保存しない（保存後に消えたら `Run` は既定へ落とさず `--reset cwd` を案内して止める）。④**実行前コマンド**: 失敗したら本体を走らせない。POSIX は `{ <before>⏎} && { <本体>⏎}`、PowerShell は `<before>⏎if ($?) { <本体>⏎}`（群にするのは本体が複合文 `make; ./a` でも後半まで止めるため、閉じを改行の後に置くのは行末コメント・末尾 `&` が閉じを飲み込まないため）。終了コードは失敗した before のもの（`false` なら 1）。⑤**口**: CLI `tako run-config <file> --args <s> --env K=V（繰り返し）--unset-env K --cwd <d> --before <cmd>`（保存先の既定はファイル。`--unset-env` は保存先を省くと両方から消す。空文字の `--args ""` / `--before ""` はプロジェクトの値を塞ぐ明示の値）。MCP は**ツールを足さず** `tako_run_defaults` の `args` / `env`（object。値 `null` = 消す）/ `cwd` / `before`。応答は既存の `config.{args,env,cwd,before}`（出典つき）と `updated`。`command` の上書きは保存した層を素通りする（FR-3.18.5 と同じ）。⑥**診断ログへ出さない**: env の値・引数・実行前コマンドはユーザーデータ（番犬 `crates/tako-control/tests/issue1732_run_config_log_watchdog.rs` + 実経路の grep）。A/B は `TAKO_1732_LEGACY=1`（保存しても実行に重ねない）。実経路は `scripts/test-run-config-1732.sh` |
| FR-3.18.7 | **実行ボタン横のドロップダウンで、実行されるコマンド・プロファイル・実行環境・引数・環境変数・作業ディレクトリ・実行前コマンドを見て変えられる**（#1733。#1726 S5。設計 `.agent/plans/2026-09-runner-settings.md` §7・§10 S5）。#1733 以前はシェブロンがプロファイル 2 つ以上のときだけ出て設定の入口が無く、一覧は実行環境を重ねる前の `python3 a.py` のまま（実際は `.venv` の python で走る）、選んだプロファイルはメモリ上だけ（`preview_run_selected`）だった。①**ヘッダ**: シェブロンは走らせるコマンドがあるか実行環境の種類があるときは常に出す（`PreviewHeaderVisibility.run_chevron` = 再生ボタンと一緒に畳む）。再生ボタンの右に選んだ実行環境の短いラベル（`.venv 3.12`。`run_runtime_label` = 幅 360 以上）、再生ボタンのツールチップに実行されるコマンドの全文。②**状態は dispatch `RunResolve` の答えをそのまま持つ**（`run_menu_ui::RunView`。プレビューを開いた / 差し替えた / リロードした / メニューを開いた時点で background の`dispatch_off_ui` で取り、取得中は前回の答えを出す）。描画は完成品を読むだけで、`render_*` の本文から `runtime_env::` / `runtime_probe::` を呼ばない（番犬 `crates/tako-control/tests/issue1733_run_menu_watchdog.rs`）。一覧のコマンドは `Run` と同じ層（実行環境・保存した引数）を重ねたものなので、一覧と実際に走るコマンドが一致する。③**メニュー**: 実行されるコマンド（全文・折り返し）と作業ディレクトリ・注意 / プロファイル（2 つ以上のとき。行 = 選んで保存して走らせる）/ 実行環境（候補・自動の印・版。5 件を超えたら畳んで「他 N 件を表示」= 選んでいる候補は畳んでも出す・再検出 = `RunResolve { refresh }`・パスを指定・解けない理由）/ 引数・環境変数（1 変数 1 行 + 追加）・作業ディレクトリ・実行前コマンド （行を押すとその場の `TextField`。IME の宛先 `AppTextInput::RunMenu`）/ Code Runner の設定を開く（`Settings { tab: "runner" }`）。④**書き込みは dispatch `RunnerDefaults { path, … }` を直に呼ぶ**（CLI `tako run-config` / MCP `tako_run_defaults` と同じ口）。項目ごとの出典タグ（ファイル / プロジェクト / 宣言 / 自動 / 既定）を押すと保存した値の保存先を file ↔ project で移し、× でそのスコープから消す。新しく入れた値の保存先は dispatch の既定（実行環境 = project・それ以外 = file）、既に保存した項目はその保存先。空のまま Enter・`=` の無い環境変数は保存せずに理由を出す（名前の検め・作業ディレクトリの実在・実行環境が解けるかは dispatch が断り、その理由を欄の下に出す）。⑤**MCP / CLI は増やさない**（操作はすべて既存の `RunResolve` / `RunnerDefaults` / `Settings` の 1:1）。A/B は `TAKO_1733_LEGACY=1`（一覧を実行環境・保存した設定を重ねない同期の解決で作る = venv の例で一覧と `Run` がずれる）。検証は visual-test `run-dropdown-1733`（目視の項目は `.agent/manual-checks.md`） |
| FR-3.18.8 | **`.ino` の基板（FQBN）とポートを、実行ボタン横のドロップダウン・`tako run-config`・MCP のどれからでも選べる**（#1952。#1943 S5。設計 `.agent/plans/2026-10-language-coverage.md` §5.4 の「実装で決めたこと（#1952）」）。#1952 以前は `arduino-cli board attach -b <基板>` を人か AI がペインで打つ案内だけで、画面から選ぶ口が無かった。①**正本は `sketch.yaml`・書くのは arduino-cli**: tako の保存先（`run-configs.json`）には持たず、実行環境の層が見つけた arduino-cli（`${arduino_cli}`）で `board attach --fqbn=<基板> --port=<ポート> <スケッチ>` を起こす（値は 1 語 = `-` で始まるポートを別のフラグと読ませない。書式を tako が真似しない = コメント・`profiles:` は arduino-cli が保つ）。②**書く前に検める**: `board attach` は入っているコアに無い基板・入っていないコアの基板・形の崩れた文字列まで rc=0 で書く（実測 1.2.0）ので、`board listall --format jsonmini`（入っているコアの基板。5 コア 545 基板の機で 0.4 秒・11 MB。`--format json` は 21 MB・2 秒なので使わない。使う欄だけの型で読む）でコアの有無と基板の実在を確かめる。**コアが無ければ書かずに `<arduino-cli> core install <コア>` を示すだけ**（数百 MB。代行しない）・基板が無ければ近い候補・形が違えば理由。断ったら実行設定の他の項目も書かない。③**読み**: `RunResolve`（`tako run <f> --list` / `tako_run_resolve`）と `.ino` の `RunnerDefaults { path }`（`tako run-config <f>` / `tako_run_defaults`）の応答の `board` 欄 = `fqbn` / `name` / `port` / `profile`（`default_profile` は compile で `default_fqbn` に勝つ = 実測。注意を `notes` へ）/ `core_installed` / `candidates`（繋がっている基板 + 今の基板）/ `ports`（`board list` のポート）/ `installed_boards`（件数）/ `matches`（`RunResolve { boards: "<語>" }` = `tako run <f> --list --boards [語]` で入っているコアの基板を名前か FQBN で探す。空 = 全部。ドロップダウンは全部をもらって手元で絞る）/ `errors`（聞けなかった理由）。聞いていない欄は `null`。`board list` は #1950 の案内（繋がった基板が 1 台ならコマンドへ入れる）と 1 回で共有する。スケッチでないファイルは `board` が `null`。④**UI スレッドで待たない**: `RunResolve` は既に background、`.ino` の `RunnerDefaults` は `prepare_offload` の `RunConfigSketch` で background（IPC = CLI / MCP）、ドロップダウンの書き込みは `dispatch_off_ui`。offload を使わない host の同期の腕は、読むだけなら子プロセスを起こさない（`sketch.yaml` の読みだけ・候補は `null`）。⑤**言語サーバを起こし直す**: 基板を書いたら、そのスケッチをルートに持つ**起こす前提つきの器だけ**（`LspManager::restart_requiring`）を起こし直す（unconfigured → 起動・動いていれば新しい基板で組み直す。ポートだけなら起こし直さない。他のスケッチ・clangd 等は触らない）。⑥**ドロップダウン**: 「基板」の節（今の基板・注意・繋がっている基板 = 押すと基板とそのポート・「基板を探す…」= 打つと入っているコアの基板を 8 件まで絞り、行か Enter（1 件に絞れていればその基板、でなければ打った文字列を FQBN として）で選ぶ・コアの無い基板に「コア未導入」の印）と「ポート」の節（見つかったポート・選んであるが繋がっていないポート・「ポートを指定…」）。出典は `sketch.yaml` の固定のタグ（移す・消すは無い = arduino-cli に消す口が無く、tako は書式を真似しないので）。書いている間は「sketch.yaml へ書いています…」、断られた理由は欄の下（欄が無ければメニューの下）。⑦CLI は `tako run-config <f> --board <FQBN> --port <ポート>`、MCP は `tako_run_defaults` の `board` / `port` と `tako_run_resolve` の `boards`（新しいツールは作らない）。Windows の `COM3` 等は macOS の単体（`board list` の形・argv・PowerShell の導入コマンド）で固定、**実機は未検証**。A/B は `TAKO_1952_LEGACY=1`（欄と口が無い #1952 以前）。番犬 `crates/tako-control/tests/issue1952_sketch_board_watchdog.rs`（tako が sketch.yaml を書かない・起こす arduino-cli は board list / listall / attach だけ・UI と IPC の書き込みは background）。単体 `tako_core::sketch_board::tests` / `tako_control::sketch_board::tests` / `run_menu_ui::tests`、実経路 `scripts/test-run-board-1952.sh`（visual-test `run-board-1952` = 実マウス・実キー・本物の arduino-cli） | M |
| FR-3.18.9 | **実行先（同じペインで再実行 / 新しいペイン）をファイル / プロジェクト単位で覚え、再生ボタン・`tako run` がそれで走る**（#1734。#1726 S6。設計 `.agent/plans/2026-09-runner-settings.md` §8.4・§10 S6）。#1734 以前は FR-3.18.1 の再利用（既定）と一回限りの `--new-pane` だけで、毎回並べたいファイルでも押すたびに付け直すしかなかった。①**保存**: FR-3.18.5 の `run-configs.json` の `RunConfig.target`（`reuse` / `new` = `runner_config::RunTarget`。語の正本は `RunTarget::VALUES` = serde の `snake_case` と同じ字面をテストで縛る）。欄は `#[serde(default)]` なので **#1734 以前のファイルは移行の Step なしでそのまま読める**（`target_version` は上げず、指紋スナップショットに `RunTarget` を足しただけ = #916）。保存先の既定はファイル（`RunConfigField::Target.default_scope`）。②**`Run` の決め方**: 一回限りの `new_pane`（CLI `--new-pane` / MCP `tako_run` の `new_pane`。**`false` も明示の値** = 保存が new でも今回は使い回す）→ 保存値（ファイル → プロジェクト）→ 既定（同じペインで再実行）の順（`run_configs::Saved::new_pane` の 1 実装）。`new` は FR-3.18.1 の再利用の探索をしないだけで、差し替えの規則は変えない。応答に `target`（`reuse` / `new` = 今回の判断）と `config_sources.target`（出典）。`command` の上書きは保存した層を素通りする（FR-3.18.5）ので実行先も効かない。③**口**: CLI `tako run-config <file> --target reuse|new`（`--reset target` で既定へ）、MCP は**ツールを足さず** `tako_run_defaults` の `target`（enum は `RunTarget::VALUES` から生成 = #1467 の番犬に登録）。読みは `config.target`（`value` null = 既定）。ドロップダウン（FR-3.18.7）の末尾に「実行先」の行（同じペイン / 新しいペインの 2 択・出典タグで保存先を移す・× で既定へ）。今と同じ選択肢を押しても書かない。A/B は `TAKO_1734_LEGACY=1`（保存しても `Run` は使い回す）。実経路は `scripts/test-run-target-1734.sh`（CLI / MCP・旧形のファイル・A/B・visual-test `run-target-1734` で実マウスの再生ボタン 2 回 = 2 枚 / 増えない） |

FR-3.1〜3.3 実装メモ（2026-06-13）:

- **プレビューはペイン種別**: tako-app の `previews: HashMap<PaneId, PreviewState>` に
  載っているペインはターミナルの代わりにファイル内容を描く（PTY なし）。読み込み・
  整形は GPUI 非依存の `tako-app::preview` モジュール（ユニットテストあり）
- **ハイライタは `Highlighter` trait で抽象化**（ユーザー指示）: 現実装は syntect
  （純 Rust 構成 = `regex-fancy`、oniguruma の C 依存なし）、将来 tree-sitter へ差し替え可。
  Markdown は pulldown-cmark でブロック列へ写して GPUI で描画
- **操作経路は dispatch `OpenFile` に一元化**（開発不変条件）: UI のファイルツリークリック /
  CLI `tako open <path> [--pane] [--mode code|markdown|md]` / MCP `tako_open_file`
  （**計 21 ツール**）が同じセマンティクス。表示先は「対象がプレビュー > 同タブの既存
  プレビュー（再利用）> 分割で新設」。相対パスは対象ペインの cwd（CLI は実行時 cwd）基準。
  list の各ペインに `preview: {path, mode}` を公開
- **永続化**: layout.json の `preview`（path / mode）で再起動後もプレビューペインとして
  復元する（FR-5 と整合。セッションは起動しない）

FR-3.5 実装メモ（2026-07-12、#126）:

- **編集モデル**: `tako-core::TextBuffer` が UTF-8 文字境界上のカーソル / 選択、入力、
  BS / Delete、改行、上下左右・行頭末移動を担う。GUI は `preview_line_texts` と、GPUI が
  実描画に使った行別 `TextLayout` を同モデルへ同期する。固定セル幅の近似は使わず、行頭・
  行末・日本語・タブ・スクロール後の座標を描画の shaping 結果から逆写像する（#145）
- **構文色（#152）**: 読み取り / 編集は同じ syntect 構文解決器を使い、同梱標準セット全体を
  拡張子・特殊ファイル名・shebang から判定する。`load_defaults_newlines` に合わせてパーサへ
  行末改行を渡し、C++ / Python / Rust / JavaScript / Shell 等の複数色を維持する。標準セットに
  TypeScript 文法が無い場合は JavaScript へ劣化し、特定言語だけのホワイトリストにはしない
- **改行コード（#1650）**: バッファは**ファイルのバイト列をそのまま**持ち、`\r\n` を
  「1 つの行区切り」として扱う（カーソルは CR と LF のあいだに入らない・行末は CR の手前・
  BS / Delete は 2 バイトまとめて消す）。新しく足す改行（Enter・挿入・置換）だけが
  `LineEnding::detect` の**多数派**に揃うので、**既存行の改行は 1 バイトも書き換わらない**
  （混在ファイルは編集した行だけが多数派になり、他の行は原文のまま = 開いて保存すれば
  バイト一致）。改行を 1 つも持たないファイル（空ファイル / 改行 0 の 1 行）だけは
  `LineEnding::for_new_file(Platform)` = **OS の流儀**（macOS = LF / Windows = CRLF。
  VS Code の `files.eol: auto` と同じ）。`Platform` を引数で受ける純関数なので
  **macOS 上からでも Windows の腕を検査できる**（`platform::keys` と同じ型）。
  単独の `\r` は行区切りではなく行内の制御文字として素通しする。番犬は
  `crates/tako-control/tests/issue1650_line_ending_watchdog.rs`
- **行・桁の編集 API と文書の版（#1658）**: 編集の口が全文置換 1 つだけだったので、
  1 行直すのに本文を丸ごと IPC で送り、`set_text` でカーソルが末尾へ飛び undo も
  全文 1 個になっていた。`PreviewEditRange` / `PreviewCursor` を足して**変える範囲だけ**を
  送れるようにし、応答へ**文書の版**を載せる。
  - **座標は「行 1 始まり / 桁 0 始まりの行内 UTF-8 バイト」**。向きが違うのは、行が人と AI が
    読む行番号（エディタ・`grep -n`・コンパイラの診断がすべて 1 始まり）で、桁が本文を切る
    バイト位置（`&text[..column]` がそのまま通る）だから。桁の上限は**その行の改行コードを
    除いた長さ**なので、CRLF の行では CR の手前が上限になる（= 「CR と LF のあいだ」は
    そもそも指せない）。この綴りは `tako_core::text_edit::TextPosition` が正本で、
    dispatch / CLI / MCP はすべてこれを通る
  - **UTF-16 の桁へは変換しない**。LSP は `positionEncoding` の既定が UTF-16 だが、変換の
    責務は**LSP クライアント側**（#1007 S1）に置く。tako の編集 API は本文のバイト列を扱う
    層で、ここで UTF-16 を持つと本文を触るたびに再計算が要る
  - **解けない指定は丸めずに拒否する**（本文を 1 バイトも触らない）。0 行目 / 範囲外の行 /
    行の長さを超えた桁 / 文字の途中を指す桁 / 終わりが始まりより前、の 5 つが
    `RangeEditError` の拒否理由。丸めると送り手は「3 行目を直した」と思っているのに
    別の場所が変わるので、**版で競合を弾いても意味が無くなる**
  - **版（`version`）は本文が変わるたびに 1 つ増える**単調増加のカウンタで、GUI の打鍵・
    `PreviewApply`・範囲編集・undo / redo のすべてで進む（undo でも**戻らずに進む**。
    戻すと「別の中身なのに同じ版」ができる）。カーソル移動は本文を変えないので進まない。
    `expected_version` を渡すと、版が違うときは何もせず拒否する（楽観ロック）。
    これは LSP の `didChange` が要求する文書の版の前払いでもある（#1007）
  - 応答の `document` は `version` / `line_count` / `bytes` / `line_ending` / `cursor` /
    `selection` / `undo_depth` / `undo_history_bytes` で、**編集系の応答すべて**
    （`PreviewEdit` の状態取得を含む）に載る。組み立ては `dispatch::preview_edit_reply` の
    1 実装。番犬は `crates/tako-control/tests/issue1658_edit_range_watchdog.rs`
- **修飾キーの移動・削除（#1652）**: 入口（`handle_preview_edit_key`）の頭で
  `modifiers.platform || control || alt` の打鍵を `return false` していたので、⌥← も ⌥⌫ も
  ⌘↑ も Windows の Ctrl+← も何も起きなかった。
  - **打鍵の意味は 1 枚の表**（`tako_core::platform::editor_keys::TABLE`）が両 OS の列で持ち、
    入口は `resolve(Platform, key, KeyMods)` を引くだけ（キー名も修飾キーも入口で見ない）。
    `Platform` を引数で受ける純関数なので **Windows の列も macOS の単体で検査できる**（#763 の作法）。
    macOS は Cocoa のテキスト欄の慣習（⌥ = 語 / ⌘ = 行・文書）、Windows は Windows 標準
    （Ctrl = 語 / Ctrl+Home・End = 文書端）。**Windows の Alt+矢印はペインのフォーカス移動に
    張ってある**ので使わない（GPUI はキーバインドを入口より先に発火する）。表とキーバインドが
    重ならないことは tako-app の `エディタの打鍵表はキーバインドと衝突しない` が両 OS で見る。
    その OS で押せない打鍵（macOS の ⌃←→ / Windows の Win キー）は受理もしない。
    shift は移動の行だけが読み「選択を伸ばす」、削除・改行では無視する（⇧⌫ = ⌫）
  - **語の境目**は「同じ文字種が続くあいだが 1 語」: 英数字と `_` / 記号の並び / 漢字 / ひらがな /
    カタカナ（長音符・半角カナを含む）。空白を飛ばして次の塊の端で止まる。日本語は分かち書き
    しないので文字種の切り替わりを境目にする（辞書分割は持たない）。**行頭の ⌥← は前の行の
    最後の語の頭へ、行末の ⌥→ は次の行の最初の語の末尾へ**またぐ（VS Code / macOS と同じ）。
    **語・行単位の削除は行の境目で止まる**: 行頭で前へ消すと改行だけが消えて前の行とつながり、
    行末で後ろへ消すと改行だけが消える（`\r\n` は 1 単位 = #1650）。文書の端では何も消さず
    版も進まない。語・行単位の削除は undo の塊にまとめない（1 回で 1 単位ずつ戻る）
  - **smart Home**: 最初の非空白（半角スペースとタブのインデントの直後）⇄ 桁 0 を行き来する。
    桁 0 の行頭は macOS の ⌃A と CLI / MCP の `line-start` が持つ（#1742）
  - **桁の記憶（desired column）**: 上下移動（↑↓ / Page Up・Down）だけが「狙う桁」（行頭からの
    **表示幅**。#1742）を読んで置き直し、短い行で行末へ寄せられても次の長い行では元の桁へ戻る。
    上下以外でカーソルが動いたら・編集したら・undo / redo したら捨てる。GUI は 1 打鍵ごとに
    画面の選択をバッファへ写すので、写し戻しは `set_selection` で**最終形だけを比べて 1 回で**
    置く（`set_cursor` の 2 段だと途中でカーソルが anchor へ飛ぶ = 選択があるときだけ桁の記憶と
    undo の塊が毎打鍵で切れる）
  - **キー操作の残り（#1742）**:
    - **桁は表示幅で数える**: 全角（UAX #11 の W / F）= 2・タブ = 次のタブストップ（4 桁ごと。
      `TAB_COLUMNS` が正本）・結合文字と制御文字 = 0。文字数で覚えると全角やタブが混ざった行を
      またいだとき見た目の位置がずれた（`abcd|` の ↓ が `あいう|` へ飛ぶ）。全角やタブの途中に
      当たったら近いほうの端へ、等距離なら手前へ寄せる（VS Code の `columnFromVisibleColumn` と同じ）。
      全角の判定は端末のセル幅と同じ `unicode-width`（alacritty_terminal 経由で既に依存グラフに
      いたものを tako-core の直接依存にした）。計算は `text_edit::advance_display_column` の 1 実装
    - **選択中の素の ←→ は畳む**: ← は選択の始点・→ は終点へ移り、1 文字は動かない
      （VS Code / macOS の標準）。⇧ 付きは従来どおり 1 文字ずつ伸ばし、上下と語の移動は
      カーソル（選択の先端）から動く。畳んだら桁の記憶は捨てる
    - **macOS の ⌃A / ⌃E / ⌃K**（Cocoa のテキスト欄の Emacs 系）: ⌃A は smart Home ではなく
      桁 0（`line-start`）、⌃E は行末、⌃K は行末まで削除（行末なら改行を 1 つ = `to-line-end`）で
      undo 1 回で戻る。**Windows 列には置かない**（Ctrl+A は「すべて選択」で、Ctrl+E / K も
      行の操作の意味を持たない。理由は打鍵表の行に書いてある）。⌃A / ⌃E / ⌃K はキーバインドに
      張っていない（`エディタの打鍵表はキーバインドと衝突しない` が見る）
    - 実測は `bash scripts/test-editor-keys-1652.sh`（visual-test 節 `editor-keys` の 3 相 +
      CLI / MCP の同じ操作の照合）
  - **ページ移動の歩幅**は「器に見える行数 − 1」（前の画面の端の 1 行が残る）。行数は GUI が
    追従スクロールと同じ `preview_row_geometry`（#1741。描いた行の実寸から数える 1 実装）で
    測って `set_viewport_lines` で渡す（本文ではないので版は進まない）。着地のあとは追従が
    効くので、着地行の下に余白 3 行が見える（最下段には貼り付かない）。未計測（CLI から
    開いた直後）は 20 行。文書の端の行で止まる
  - **IME の変換中は本文にもカーソルにも触らない**（打鍵は飲み込む）。未確定文字列は本文の外
    （`self.ime`）にあってカーソル位置へ重ねて描くだけなので、動かすと確定の挿入先が変わる
  - **打鍵と CLI / MCP は同じ口**: 表の結果（`EditorCommand`）を `EditorCommand::apply`
    （tako-core の 1 実装）が `TextBuffer` へ当てる。`PreviewMove { movement, select }` /
    `PreviewDelete { motion }` も同じ `run_editor_command_local` を通り、綴りは
    `CursorMovement::name` / `DeleteMotion::name` の 1 か所（MCP の enum もここから作る）。
    `expected_version` で楽観ロックできる。A/B は `TAKO_1652_LEGACY=1`（入口で修飾キー付きの
    打鍵を捨てる旧経路）。実測は `bash scripts/test-editor-keys-1652.sh`（visual-test 節
    `editor-keys` を tako-vd 上の隔離 GUI で通常 / 旧経路の 2 回回す）。番犬は
    `crates/tako-control/tests/issue1652_editor_keys_watchdog.rs`
- **インデント（#1654）**: Tab が打鍵表に無く入口を素通りし、`newline` は改行 1 つだけを
  挿していた（`fn main() {` の中で改行すると次行が桁 0 = Issue の実測 E8）。
  - **単位は既存行から推定する**（`IndentUnit::detect`）。タブで始まる行がスペースで始まる行より
    多ければタブ、そうでなければスペースで、幅は隣り合う行のインデントの差（2〜8 だけ）の最頻値
    （差が取れなければいちばん浅い幅）。1 桁の差（`/** */` の ` * `）は数えない。インデントされた
    行が無ければ Makefile / Go はタブ（文法・gofmt で決まっている）、それ以外は 4 スペース。
    **推定は開いたときと全文を差し替えたとき（`set_text`）だけ**で、バッファが持つ。打つたびに
    推定し直すと、小さなファイルでは Tab で行を深くしただけで推定が変わる（4 桁の 2 行を 8 桁に
    すると差が 8 になり、次の ⇧Tab が 8 桁外した = visual-test で実測）。VS Code も同じ置き方
  - **Tab**: 選択が無ければカーソル位置へ 1 段ぶん（スペースなら次のタブ位置まで）挿す。選択が
    あれば選択が触れる行をまとめて 1 段深くする。**⇧Tab** は選択が触れる行（無ければカーソルの行）
    を 1 段浅くする（スペースは 1 つ前のタブ位置まで = 6 桁なら 2 つ・行頭のタブは 1 つ）。
    選択の終わりが行頭にあればその行は触らない（行を丸ごと選ぶと終わりが次の行頭に来る）。
    深くするとき空行・空白だけの行は触らない。どちらも**1 回の編集**なので undo 1 回で戻り、
    選択は同じ文字を指し続けるようにずらす。浅くできる行が無ければ本文も版も変えない
  - **Enter**（`newline_and_indent`）: カーソルより前にあるこの行のインデントを引き継ぐ。
    行が開き括弧（`{` `[` `(`）で終わっていれば 1 段深くし、`{|}` のように対応する閉じ括弧の
    手前なら閉じ括弧を次の行（元のインデント）へ送る。Python / YAML は行末の `:` でも深くし、
    **Markdown とプレーンテキストは継承だけ**（`[` はリンク・`(` は括弧書き）。規則はファイル名 → 拡張子で
    決まる（`IndentRules` = 言語の同定の正本 `tako_core::file_type` の `enter`。`CMakeLists.txt` はコード扱い = #1948）。空白だけの行で押したらその行の空白は残さない（行末の空白を作らない）。
    挿す改行はファイルの改行コード（#1650）。**素の `newline()` は API として残し**、Enter の打鍵と
    `newline` コマンドだけが引き継ぐ側を通る
  - **括弧の自動閉じは入れない**（#1654 の判断。#1759 でこの条件を満たして入れた = 下の
    「Enter の続きと括弧の自動閉じ（#1759）」）。閉じ側を入れてよいかは言語と文脈で割れる（Rust のライフタイム
    `'a`・Markdown の `[`（リンク）・文字列やコメントの中・すでに閉じ括弧がある位置で打つと二重になる）。
    正しく入れるには言語ごとの規則（文字列 / コメントの中かどうか = 構文の判定）と、打った閉じ括弧を
    飛び越す動作（type-over）の対が要り、片方だけ入れると**手で打った閉じ括弧が二重になる**事故の
    ほうが多い。入れるときは設定で切れる形（既定 OFF か言語ごとの既定）にする
  - **打鍵と CLI / MCP は同じ口**: 打鍵表に `Tab`（⇧ で向きが変わる）の行を足しただけで、入口は
    触っていない。`PreviewEditCommand { command }`（`indent` / `outdent` / `newline`。綴りは
    `EditorCommand::edit_names` の 1 か所）が `run_editor_command_local` を通る。MCP はカタログの
    予算が残り少ないので**ツールを増やさず** `tako_preview_edit` の `command` 引数へ写す
    （`enabled` と同時には受けない）。応答の `document.indent`（`tab` / `spaces:4`）で推定した単位を
    読める。実測は `bash scripts/test-editor-keys-1652.sh`（visual-test 節 `editor-keys` の
    Enter / 選択行の Tab・⇧Tab / 選択の無い Tab の 3 相）。番犬は
    `crates/tako-control/tests/issue1652_editor_keys_watchdog.rs` の 8・9
- **Enter の続きと括弧の自動閉じ（#1759）**: #1654 の Enter（継承と開き括弧の直後の 1 段）に
  足す 2 つの規則と、#1654 が見送った括弧の自動閉じ。どれも**言語名を持たない**: 使うかと語は
  言語の同定の正本 `file_type` の行の列（`dedent_after` = 戻すキーワード・`list_continuation` =
  リストの印・`auto_close` = 自動閉じ）で決まり、ロジック（`text_edit/enter.rs` /
  `text_edit/auto_close.rs`）は列の値を受けて判定するだけ
  - **字下げを戻すキーワード**: 行（行頭〜カーソル）の先頭の語が `dedent_after` 列の語（Python は
    `return` / `pass` / `break` / `continue` / `raise`）なら、次の行の字下げを ⇧Tab と同じ幅
    （`outdent_width`）だけ浅くする。照合は語の単位（`returned` / `pass_count` は別の語）。
    **文が続いている行では戻さない**: 開き括弧で終わる（#1654 どおり深くする）・`\` で終わる・
    行の中に閉じていない括弧がある（`return f(a,`。文字列・コメントの中の括弧は数えない = #1691 の
    走査 `has_unclosed_open`）。`else:` / `elif` の自動の浅くは入れていない（打った後に行を直す
    動作で、Enter の規則ではない）
  - **リストの印の引き継ぎ**: 行頭〜カーソルが CommonMark のリストの印（`-` / `*` / `+` /
    1〜9 桁の数字 + `.` か `)`、その後ろに空白 1 つ以上）で始まれば、改行 + 同じ字下げ + 次の印。
    番号は 1 つ進め（`9.` → `10.` の繰り上がりで桁が増える）、`[ ]` / `[x]` は空の箱で引き継ぎ、
    印の後ろの空白は元のまま。カーソルより後ろの中身は新しい項目へ移す（手前の空白は捨てる）。
    **中身が空の項目**（印だけ）では印と字下げを消して行を空にし、改行は入れない = リストを抜ける
    （GitHub の編集欄と同じ。入れ子の空項目も親へ戻さず空にする）。区切り線（`---` / `* * *`）と
    コードブロック（```` ``` ```` / `~~~` の囲み。字下げ 3 桁まで・閉じは同じ記号を開き以上の数）の
    中は項目にしない（YAML のブロックの `- ` を継がない）。囲みを探すのは行頭から
    `SCAN_WINDOW` = 1 MB 前まで（#1691 と同じ近似）。どちらも 1 回の編集 = undo 1 回で戻る
  - **括弧の自動閉じ**: 開き括弧（`(` `[` `{`）を打つと閉じ括弧も入れてカーソルを間に置く
    （1 回の編集 = undo 1 回で両方消える）。閉じるのは**次の文字が空白・行末・`;:.,=}])>`** のとき
    だけ（VS Code の `autoCloseBefore` の既定。`foo|bar` で `()bar` にしない）。**文字列・コメントの
    中では閉じない**: 打つ位置へ開き括弧を差し込んだ写し（窓の頭〜その行の終わり）を #1691 の走査に
    読ませ、差し込んだ括弧が括弧として返るかを見る（`opens_in_code`。打つ前の本文では `'(|'` の
    ような「打つと閉じる形」を見分けられないため）。引用符の自動閉じは入れていない（アポストロフィ・
    Rust のライフタイムで割れる）。選択中の開き括弧は選択を 1 文字で差し替えるだけ（囲みは入れていない）
  - **飛び越しは自分で入れた閉じ括弧だけ**（VS Code の `autoClosingOvertype: "auto"`）。入れた
    閉じ括弧の位置を `TextBuffer` が追い（`AutoPairs`。本文を書き換える唯一の口 `apply_edit` で
    位置を追従させ、括弧の 1 バイトに触れた編集が来たらその件を捨てる）、その手前で同じ閉じ括弧を
    打つと入れずにカーソルだけ進める（本文も版も変えない）。**手で書いた閉じ括弧は飛び越さない**
    = 打った数だけ入る（#1654 が恐れた「二重になる / 数が狂う」の両方を避ける）。カーソルが対の外へ
    出たら追わない（戻ってきても飛び越さない）・undo / redo の後は追わない。`(|)` の間の ⌫ は対で
    消す（入れた閉じ括弧を残骸にしない）
  - **形式と設定**: 自動閉じは `file_type` の `auto_close` 列（文書 = Markdown・mdx・プレーン
    テキスト・reST・AsciiDoc は false）× settings.json の `editor_auto_close`（既定 ON）。
    どちらかが OFF なら #1654 と同じ 1 文字の挿入（飛び越しも対の削除もしない）。GUI は編集
    セッションを作るたびに設定をバッファへ渡し（`TextBuffer::set_auto_close`）、切り替えたら
    開いている全セッションへ渡す。効いているかは応答の `document.auto_close`
  - **打鍵と CLI / MCP は同じ口**: 打鍵 1 文字は `EditorCommand::Type(char)` →
    `TextBuffer::type_char` の 1 実装で、GUI の文字入力（`replace_text_in_range` で 1 文字・範囲の
    指定なし。IME の長い確定と貼り付けは従来どおり `insert`）と、dispatch `PreviewEditCommand` の
    `command: "type"` + `text`（1 文字ずつ同じ口へ通す。版の照合は最初の 1 打鍵の前だけ。1 回
    1000 文字まで）が通る。CLI は `tako edit type <文字>` / `tako edit auto-close [on|off]`、MCP は
    `tako_preview_edit` の `command: "type"` + `text` / `auto_close`（**ツールは増やさない**）。
    設定は dispatch `PreviewAutoClose`。Enter の 2 規則は既存の `newline`（`tako edit newline`）の
    中身なので口は増えない。A/B は `TAKO_1759_LEGACY=1`（3 つとも #1654 の挙動へ戻る）
- **行の操作（#1692）**: 行の複製・移動・行ごとのインデント・行コメントの付け外しが無かった。
  本体は `tako_core::text_edit::line_ops`（`TextBuffer::apply_line_operation`）。
  - **対象の行**は選択が触れる行（無ければカーソルの行）。選択の終わりが行頭ならその行は含めない。
    決めるのは `TextBuffer::line_block` の 1 実装で、⇧Tab / 選択行の Tab（#1654 の `shift_lines`）も
    同じこれを引く
  - **複製**（`duplicate-up` / `duplicate-down`）は「改行 + 塊」を塊の後ろへ 1 つ足すだけで、
    上 / 下で本文は同じ・違うのはカーソルが上の写し（元の位置）に残るか下の写しへ移るか（VS Code と
    同じ）。足す改行はファイルの多数派（#1650）。**移動**（`move-up` / `move-down`）は隣の行と
    入れ替えるだけで**既存の改行を並べ替え、新しい改行を作らない**（CRLF のファイルに LF が混ざらない）。
    先頭行の上移動・末尾行の下移動は本文も版も履歴も変えない。選択は動かした行についていく
  - **行インデント**（`indent` / `outdent`）は選択が無くても行ごと（⌘]）。Tab（選択が無ければ
    カーソル位置へ挿す）とは別の操作で、中身は #1654 の `shift_lines`（空行は深くしない）
  - **コメント**（`comment` / `uncomment` / `toggle-comment`）は空行を飛ばし、いちばん浅い
    インデントの桁へ「記号 + 空白 1 つ」を挿す（VS Code と同じ。深い行の記号だけ右へずれない）。
    外すときは記号と直後の空白 1 つを外す。トグルは「空行を除く全行がコメントなら外す / そうでなければ
    付ける」。対象が空行だけなら空行へ挿す（コメントを書き始められる）。行コメントの無い形式
    （HTML・CSS・Markdown）は**行ごとに**囲む（複数行を 1 組で囲まない = 付け外しが行で閉じる）。
    **記号は言語の同定の正本 `file_type` の行の `comment` 列**（#1948 の表。別の表は持たない）で決まり、
    ロジックは記号の文字列を持たない（行を足すだけで言語が増える。HTML / XML / Vue / CSS / SCSS /
    Less / JSONC / TOML / Dockerfile は記号を言うためだけの行を足した = 色・インデント・Enter は既定のまま）。
    コードの行で記号を持たないのは JSON とノートブックだけ（単体が固定する）。表に無い形式（プレーンテキスト・JSON）は
    本文を触らず理由を返す（GUI はフッタ・CLI / MCP はエラー）。そのため `EditorCommand::apply` は
    `Result` を返す
  - どの操作も **1 回の編集**（種類は `Replace`）なので undo 1 回で元の全文へバイト一致で戻り、
    直前の打鍵の塊（#1651）とはまとまらない。コメントの付け外しは各行の書き換えを離れた箇所の並び
    （#1869）として 1 件にまとめる
  - **打鍵**（`platform::editor_keys` の表）: macOS は ⌥↑↓（移動）/ ⇧⌥↑↓（複製）/ ⌘/（トグル）/
    ⌘] ⌘[（行ごと深く / 浅く）。Windows は Alt+↑↓ がペインのフォーカス移動・Alt+Shift+↑↓ が
    リサイズに張ってある（#585）ので、語の移動と同じく ⌥ → Ctrl に読み替えて Ctrl+↑↓ /
    Ctrl+Shift+↑↓ / Ctrl+/ / Ctrl+] Ctrl+[。⌘/ ⌘] ⌘[ の行は **shift を受けない**
    （`Action::accepts_shift`。⇧⌘] / ⇧⌘[ はタブ切替が持つので、受理すると「表にあるのに届かない」
    打鍵になる）。「付けるだけ」「外すだけ」は打鍵を持たず CLI / MCP で指す
  - **CLI / MCP は 1:1**: CLI `tako edit line <操作>` / MCP `tako_preview_edit` の
    `command`（`line-<操作>`。**ツールは増やさない**）/ dispatch `PreviewEditCommand` が打鍵と同じ
    `run_editor_command_local` を通る。綴りは `LineOperation::name` の 1 か所で、`line-` の頭は
    `EditorCommand::line_edit_name` が付ける。番犬は
    `crates/tako-control/tests/issue1692_line_ops_watchdog.rs`（記号の直書き・対象行の 1 実装・
    理由を捨てない・口の 1 本化）。実測は `bash scripts/test-line-ops-1692.sh`（visual-test 節
    `editor-keys` の (15) で実打鍵 6 相 + CLI と MCP を同じ手順で撃って保存後のファイルと応答を突き合わせる
    + 空ファイル・改行の無い 1 行・CRLF・記号の無い形式）
- **保存競合**: 編集開始時の元バイト列を保持し、保存直前のファイル内容と完全比較する。
  不一致なら外部変更として保存を拒否し、dirty バッファを保持する。Unix は同一ディレクトリの
  一時ファイルを `sync_all` 後に rename、Windows は既存依存だけで置換 API を使えないため
  比較後に truncate + `sync_all`（既知の原子性差）。読み取り専用属性も明示拒否
- **外部変更の後の逃げ道（#1659）**: 突き合わせる相手は**基準**（開いた / 保存した / 読み直した
  時点のファイルの中身。`TextBuffer::observe_disk` / `refresh_disk_state`）。ディスクの中身が
  基準と同じ = 変わっていない、**編集中の本文と同じ = どちらを採っても同じなので競合にせず
  基準へ取り込む**。消されたファイルは `TextEditError::ExternalDeleted`（`DiskState::Deleted`）で
  書き換わりと区別する（通常の保存は作り直さない）
  - **上書き**: `TextBuffer::save_overwrite`（dispatch `PreviewSave { force: true }` / CLI
    `tako edit save --force` / MCP `tako_preview_save` の `action=overwrite` / 帯の「上書き保存」）。
    消されていれば作り直す（権限は新規ファイルの既定）。読み取り専用は上書きしない。
    リモート由来（FR-3.24.2）は `force` を `remote_fs::save_file` へそのまま渡す。
    `force: false` は wire に現れない（旧 GUI とバイト一致）。スマホの保存（FR-6.16）は常に false
  - **読み直し**: `TextBuffer::reload_from_disk`（`PreviewRevert` / `tako edit reload` /
    `action=reload` / 帯の「ディスクから読み直す」）。**書き換えるのは食い違った範囲だけ**
    （共通の先頭・末尾を削った中央 = `changed_span`）で、`apply_edit` を通る 1 回の編集として
    積むので **undo 1 回で自分の変更へ戻せる**（戻した変更は基準がディスクの中身なので競合せずに
    保存できる）。カーソルは同じ場所に残し、改行コードとインデントは読み直した中身から取り直す。
    上限（#1660）超過・バイナリ・非 UTF-8・消された・リモート由来（写しを読み直しても最新に
    ならない）は本文を触らずに断る
  - **差分**: `TextBuffer::disk_diff` → `tako_core::text_diff::diff_lines`（`PreviewDiff` /
    `tako edit diff` / `action=diff` / 帯の「差分」）。向きは**ディスク → 編集中**（= 上書きすると
    何が変わるか）。依存を増やさず自前で、共通の先頭・末尾の行を削ってから中央だけを Myers で
    比べる（5 万行・2.8 MB で 2 行違い = debug ビルドの `tako edit diff` 往復込みで 30 ms）。編集距離 1,000 超は中央を 1 塊にした差分へ
    落とし（`approximate`）、出力は 5,000 行で打ち切る（`truncated`。件数は全体を数える）。
    応答の `diff` は `state` / `identical` / `added` / `removed` / `truncated` / `approximate` / `unified`
  - **競合の状態**: `EditState::conflict`（`note_conflict` / `clear_conflict` の 1 か所で書く）。
    **知らせるのは新しく分かったときだけ**（同じ状態は何度検知しても `notices` を増やさない。
    書き換わった → 消されたのように状態が変わったらもう 1 回）。**競合中は自動保存の対象にしない**
    （`autosave_due`。500ms 待った後の `run_autosave` も見る）。解けるのは上書き・読み直しの成功と、
    監視が「外で元へ戻された / 自分と同じ中身に書かれた」を見たとき。編集系の応答すべてに
    `conflict`（`state` / `notices` / `autosave_paused`）が載り（`preview_edit_reply` の 1 実装。
    競合していなければキーごと無い）、保存の失敗には抜け方（`Reason::RecoveryHint`。表示言語に従う = #2365）が添わる
  - **GUI の帯**: タイトルバーの直下に文面と「差分 / 上書き保存（消されていれば作り直して保存）/
    ディスクから読み直す」を出す。印は GPUI の矩形（絵文字を使わない）。差分はチェンジログと同じ
    描画（`render_changelog_diff`）で帯の中に広げる。タイトルの「· 競合」は自動保存の設定によらず出す
  - **未保存のプレビューへ別のファイルを開く**: `OpenFile` は未保存の変更があるプレビューを
    再利用の候補から外し、右へ分割して開く（応答の `kept_unsaved` に残したペイン。VS Code の
    プレビュータブが未保存になると差し替えられないのと同じ）。同じファイルを開き直したときは
    読み直さずにそのペインへ着地する。`set_preview` を直接叩いたときの拒否にも抜け方を添える
  - **検証**: 単体（`tako_core::text_edit` / `text_diff` / `tako-app` の `preview::tests` /
    `dispatch::tests::issue1659_*`）・番犬 `crates/tako-control/tests/issue1659_external_change_watchdog.rs`・
    実経路 `bash scripts/test-external-change-1659.sh`（隔離 GUI の CLI / MCP + A/B +
    visual-test 節 `external-change` = 帯のボタンを実マウスで押す）
- **安全制限**: PDF / 画像 / 動画 / バイナリ / 非 UTF-8、および 10 MB・10 万行上限（#1660。
  旧 1 MB・5,000 行）で末尾を省略したファイルは編集不可。省略表示を保存して元ファイル末尾を
  失う事故を構造的に防ぐ。**断るときは理由と値を返す**（`Truncation::edit_refusal` の 1 実装。
  例: 「行数が上限を超えるため末尾を省略して表示しており、編集できない: 100,001 行
  （上限 100,000 行）」）
- **AI フルコントロール**: dispatch `PreviewEdit` / `PreviewApply` / `PreviewEditRange` /
  `PreviewCursor` / `PreviewSave` / `PreviewRevert` / `PreviewDiff` / `PreviewUndo` / `PreviewRedo` /
  `PreviewSearch` / `PreviewReplace` / `PreviewAutosave`、
  CLI `tako edit start|status|apply|replace-range|cursor|save|reload|diff|stop|undo|redo|search|replace|autosave`、
  MCP `tako_preview_edit` / `tako_preview_apply` / `tako_preview_edit_range` /
  `tako_preview_cursor` / `tako_preview_save` / `tako_preview_undo` /
  `tako_preview_redo` / `tako_preview_search` / `tako_preview_replace` /
  `tako_preview_autosave`。個々の物理キーイベントは GUI/IME 固有で API 化せず、
  自然な操作境界（全文適用・行桁の範囲編集・カーソル / 選択）を公開する。
  list の `preview.editing` / `preview.dirty` で状態取得
- **自動保存（#195 / #973）**: 既定 ON。テキスト変更後 500ms のデバウンスタイマーで
  `TextBuffer::save()` を呼ぶ。保存競合（外部変更検知）時は上書きせずタイトルバーに
  「· 競合」を表示し、**競合が解けるまで自動保存を止める**（#1659。上の「外部変更の後の逃げ道」）。
  `PreviewAutosave` で ON/OFF 切替。OFF 時は従来の ⌘S 手動保存に戻る。
  **対象は「誰が編集したか」ではなく編集セッションの状態（autosave が有効 + 編集中 +
  dirty）から導く**（`preview::autosave_due`）ので、**GUI の入力経路と dispatch = CLI / MCP
  の編集が同じ 1 本の入口（`drive_autosave`）を通る**。#973 まではフラグを立てる
  `schedule_autosave` とタイマーを回す `start_autosave_timer` が 2 本に分かれており、
  後者を呼ぶのは GUI の入力経路（キー / ペースト / IME）だけだったため、
  `edit replace` / `apply` / `undo` / `redo` は**保留に入ったまま誰も保存しなかった**
  （`EditState::open` の既定が `autosave: true` なので、AI にプレビュー編集を任せると
  「自動保存 ON なのに保存されていない」= データを失いかねない見え方になっていた）。
  dispatch はペインの状態を変えるところまでで 500ms のタイマーには GPUI の `Context` が
  要るので、消化するのは**すべての dispatch が通る 1 箇所**（IPC の 1 ターンの後処理）。
  リモートは `autosave` が既定 OFF（FR-3.24.2）なので状態から自然に外れる。同一バイナリで
  #973 前へ戻す A/B は `TAKO_973_LEGACY=1`（✅ 2026-09-04、#973）
- **undo/redo（#195）**: `TextBuffer` に操作スナップショットスタック（上限 1000）を実装。
  insert / delete / set_text の前に全状態を push。⌘Z / ⇧⌘Z で GUI 操作、dispatch 経由で
  CLI / MCP からも操作可能。自動保存と独立（保存済みでも undo 可）
- **検索・置換（#195 / #1653）**: `TextBuffer::find_all(query, SearchOptions)` でインクリメンタル検索。
  1 回の `find_all` で得たヒットを `next_hit` / `prev_hit`（ラップ移動）・件数・ハイライトが
  使い回す。⌘F で検索バー表示（編集モード外でも使用可。開閉は `open_preview_search_bar` /
  `close_preview_search_bar` の 1 実装で、CLI `tako edit search --open|--close`・MCP の `visible` も
  同じ口を通る。閉じたら閲覧中の Markdown は描画へ戻る = FR-3.3.3 ⑥ / #1873）。1 件置換は `replace_next`
  （カーソル以降の最初 → 先頭へラップ）、全置換は `replace_all`（undo 1 回で戻る）。
  dispatch / CLI / MCP で 1:1 公開
- **条件（#1653）**: `SearchOptions { case_sensitive, whole_word }`。**既定は「区別する・単語単位なし」**
  （`SearchOptions::DEFAULT`。GUI の検索欄の初期値・CLI / MCP の省略時が同じこれを使う）。
  省略したときの決め方は `SearchOptions::resolve` の 1 実装:
  - `PreviewSearch` に **query を渡した** = 既定から組み直す（省略した項目は既定）。呼び手が
    画面の条件を知らなくても結果が決まる
  - **query を省略した**（次へ / 前へ・条件だけ変える）= 今の検索の条件を引き継ぐ。
    `tako edit search --direction prev` が画面のトグルを黙って戻さない
  - `PreviewReplace` は query を毎回持つので**常に既定から**（検索欄の条件を引き継がない）。
    検索欄の置換欄で Enter を押したときだけ画面のトグルの条件を使う
  - 応答の `search` / `replace` に実際に使った `case_sensitive` / `whole_word` が載る
  - CLI は `-i` / `--ignore-case`（区別しない）と `-w` / `--whole-word`。付けたときだけ送る
    （#322 の最簡形 = 既定の「区別する」は引数なしで得られる）
- **単語単位（#1653）**: 一致の前後が「単語の文字」（Unicode の英数字と `_`）に続く位置を外す。
  **漢字・かなも単語の文字**に数えるので `値value` の `value` は当たらない（識別子として 1 語）。
  日本語の文の中の語は空白で区切られないため単語単位では切り出せない。端の文字が単語の文字で
  ないクエリ（`.foo` / `foo(`）はその側の隣を問わない（VS Code と同じ）。外れた候補は 1 文字だけ
  進めて探し直すので `xfoo foo` の 2 つ目を取りこぼさない
- **小文字写し（#1016 / #1653）**: 区別しない検索は本文の小文字写し（1 文字ずつ小文字化 =
  Σ の文脈依存を避ける）を探し、位置を元テキストへ戻す（`İ` は 2 → 3 バイト・`ẞ` は 3 → 2
  バイトなので写しの位置をそのまま使わない）。**写しを作るのは区別しない検索をしたときだけ**で、
  本文が変わるまで使い回す（`TextBuffer::folded`。捨てるのは版を進める `bump_version` と、
  検索欄を閉じたときの `release_search_cache`）。区別する検索は写しを作らずに本文を探す。
  release 実測（1 MB・クエリを 1 文字ずつ 10 打鍵 × 20 往復）: 検索欄の 1 打鍵 = 修正前 中央値
  4.449ms（`find_all` と `find_next` がそれぞれ写しを作る = 1 打鍵 2 回）→ 区別する 0.509ms
  （写し 0 回）/ 区別しない 1.251ms（200 打鍵で写し 1 回）。単体テストは時間ではなく
  `search_fold_builds`（作った回数）で固定する。番犬は
  `crates/tako-control/tests/issue1653_search_case_watchdog.rs`（区別する分岐が写しより前で返る /
  写しを作るのは `folded` だけ / 版を進める口で捨てる / GUI の全文検索は
  `EditState::refresh_search_hits` の 1 箇所 / 省略時の条件は dispatch の 1 実装）。
  実 GUI の経路は `bash scripts/test-search-case-1653.sh`（visual-test 節 `search-case` =
  トグルを合成マウスで押して件数 3 → 5 → 4 → 2 → 3・置換で `Value` が残る + CLI と MCP の
  応答の字面一致）
- **対応する括弧の強調（#1691）**: カーソルを括弧に置いても対の括弧が光らなかった。
  本体は `tako_core::text_edit::brackets`（LSP・構文木と独立。純粋関数 `find_partner` /
  `match_at_cursor` と `TextBuffer::bracket_match`）。
  - **採り方は VS Code の `matchBracket`**: 選択中は採らない・カーソルの**直後**の括弧を先に見て、
    対が無いときだけ**直前**の括弧（`)|(` は右の `(`。Zed も同じ長さなら右の組を残す）・
    **括弧に接していなければ採らない**（Zed はカーソルを囲む最内の組を出すが、打っているあいだ
    離れた括弧が光り続けるので採らない）・種類ごとに数える（`( [ ) ]` でも `(` と `)` が対）
  - **文字列とコメントの中の括弧は数えない**。記号は言語の同定の正本 `file_type` の行から引く
    （行コメント = `comment` 列 #1692、文字列とブロックコメント = 新しい `lexical` 列。別の表は
    持たない）。`lexical` は `"…"`（`"""…"""`・Rust の `r#"…"#` も）/ `'` の扱い（文書は区切りに
    しない・C の系譜は 1 文字のリテラルだけ = Rust のライフタイム `'a` を文字列にしない・スクリプトは
    同じ行で閉じるときだけ文字列 = `don't` はアポストロフィ）/ `` `…` `` / ブロックコメント。
    `#` の行コメントは行頭か空白の直後だけ（シェルの `${#arr[@]}`）、`\%` `\#` は記号そのもの。
    見分けない近似: JS の正規表現リテラル・テンプレートの `${…}` の中・ヒアドキュメント・入れ子の
    ブロックコメント・C++ の `R"(…)"`
  - **所要（NFR-8）**: 文字列・コメントの状態は先頭から読まないと決まらないので、走査の起点から
    対の括弧まで 1 度だけ走る。答えは (版, カーソル) が同じあいだバッファが使い回す（描画は表示中の
    行ごとに引くが探すのは 1 回 = 単体は `bracket_searches` の回数で固定）。カーソルの前後
    `SCAN_WINDOW`（1 MiB）より離れた所は読まない（10 MB の文書の上限。VS Code も 20ms で打ち切る）。
    release 実測（`perf_1691_対応括弧の探索`。5,000 行 / 222,683 バイト）: 末尾の括弧（先頭から
    全文を読む最悪）中央値 0.152ms・最大 0.237ms、1 打鍵（insert + 探索）中央値 0.156ms・
    最大 0.268ms、10 MB の 1 行 0.369ms
  - **描画**: エディタの行（`preview_code_line_sel`）へ対の 2 文字の背景（teal 40%）を重ねる。
    重ね順は構文色 → 診断の波線 → ⌘ホバー → 対応括弧 → 検索 → 選択（検索・選択が重なればそちら）
  - **AI から読む口**: 編集系の応答の `document.bracket_match`（`bracket` = カーソルの隣の括弧・
    `match` = その対。行・桁は `cursor` と同じ座標。無ければ null）。`document_state` の 1 実装に
    載せたので CLI `tako edit cursor` / `status`・MCP `tako_preview_cursor` 等のすべてで読める
    （ツールは増やさない）。A/B は `TAKO_1691_LEGACY=1`（探さない = 強調せず常に null）。
    実 GUI は visual-test 節 `bracket-match`（対の 2 つの括弧の矩形が基準から変わり、あいだの文字は
    変わらない・実キーの → で直後へ動かしても同じ組・離れると基準の画素へ戻る・対の無い `(` では
    変わらない）

FR-3.15 実装メモ（2026-07-15、#233）:

- `notify` のネイティブバックエンド（macOS FSEvents / Windows
  ReadDirectoryChangesW）で、表示中対象の親ディレクトリだけを非再帰監視する。
  ファイル自身ではなく親を監視し、一時ファイルの rename で置換する原子保存・削除後の
  再作成でも監視を失わない。アイドル時は UI タイマーもポーリングも起動しない
- callback はパスを channel へ送るだけとし、300ms のパス別デバウンス後に
  `preview::load_for_reload` を background executor で実行する。世代番号で遅い完了結果を破棄し、
  UI スレッドは完成状態の HashMap 差し替えだけを行う。巨大テキストは上限（#1660 で 10 MB）
  + 1 byte しか読まず、既存の省略表示へ劣化する
- 競合時は編集バッファと閲覧状態を一切差し替えず、FR-3.5 の競合（`EditState::note_conflict`。
  知らせるのは新しく分かったときだけ）を記録する。突き合わせる相手は基準（開いた / 保存した /
  読み直した時点の中身）なので、自分の保存が生む OS イベント・touch・外で元へ戻された回は
  競合にしない（#1659）。**未編集の編集セッションはディスクへ黙って追従する**（background で
  読んだバイト列で `TextBuffer::reload_from`。undo で戻せる）

FR-3.16 実装メモ（2026-07-15、#232）:

- `tako-core::PreviewOutline` は表示ラベル・階層・ジャンプ先を GPUI / PDFKit 非依存で保持する。
  Markdown のジャンプ先は描画済みブロック番号、PDF は 1 始まりページ番号で、GUI・CLI・MCP が
  同じ 1 始まり項目番号を解決する。重複見出しはラベルで同一視せず別ブロックとして残す
- Markdown は pulldown-cmark のブロック列完成直後、PDF は PDFKit の `outlineRoot` を再帰走査して
  background ロード結果へ `Arc<PreviewOutline>` として同梱する。見出し / 目次がない文書は空配列へ
  正常劣化する。ライブリロードも同じロード関数を使うため、本文と目次が同じ世代で差し替わる
- render は `Arc` をスナップショットするだけでパースや PDFKit 呼び出しを行わない。目次パネルが
  閉じている間は行要素も生成せず、#231/#234 の `PreviewImageCache`・background ラスタライズ・
  zoom/pan と #233 の監視・世代照合・スクロール保持を維持する

FR-2.16.10 / FR-3.11 実装メモ（D&D。2026-06-13）:

- **GPUI のドラッグ API**（gpui ソース Apache-2.0 を参照。cmux は不参照）:
  ドラッグ元 = Stateful 要素の `on_drag(payload, ghost_constructor)`（カーソル追従の
  ゴーストビューを返す）、ドロップ先 = `on_drag_move::<T>`（capture phase で全 move に
  発火。`bounds` 内判定 + 象限計算は自前）+ `on_drop::<T>`。ペイロード型は
  `TmuxSessionDrag { name, socket }` / `FileDrag { path }` の 2 種
- **ドロップ先の状態は `drop_target: Option<(PaneId, DropZone)>`** に持ち、render で
  各ペイン矩形に重ねた透明オーバーレイ（ドラッグ中のみ生成）が更新する。挿入プレビュー =
  ドロップ後に新ペインが占める半面を accent 色で強調 + 結果ラベル。ドラッグ種別は
  自前の `drag_payload`（on_drag 開始時にセット、drop / mouse-up でクリア。
  `App::has_active_drag()` と併用）で追跡する
- **tmux 取り込みの実体は attach クライアント**（dispatch `TmuxOpen`）: ドロップ位置で
  分割した新ペインに `env TMUX= tmux [-L socket] attach-session -t =name` を流す。
  `TMUX=` はネストガード回避（バックエンドペイン内でも attach できる）、`=name` は
  完全一致指定。orphan バックエンドの「バックエンド機構への再 adopt」は採らない
  （backend_sessions の対応再構築が必要で複雑。attach 表示で目的 = 見える化は足りる）。
  ペイン close は通常どおりクライアント終了 = セッションは残る（kill はしない）
- **ゾーン判定**: tmux ドラッグ / ペイン移動ドラッグ（FR-1.10）= 4 象限（対角線分割）、
  ファイルドラッグ = 中央 40% 矩形（= direction なし → FR-3.2 の既存再利用セマンティクス）+
  周辺 4 象限。純関数 `drop_zone()` としてユニットテスト
- **ペイン移動（FR-1.10）のコア API は `Workspace::move_pane_to`**（pane を target の
  direction 側へ挿し直す。同タブ = ツリー内の close → split、別タブ = 既存 move_pane と
  同じ取り出し + 指定位置へ挿入）。dispatch は既存 `MovePane` の拡張（`tab` = 末尾追加の
  従来動作と `target` + `direction` の排他）で、新ツールは増やさない。
  自分自身へのドロップは no-op（UI 側で握り潰す）

FR-3.6 / FR-3.9 実装メモ（2026-06-14。git graph + diff ビューア）:

- **データ層は `tako-core::git`**（tmux.rs と同パターン。git CLI 子プロセス + 純関数パース）。
  `git_bin()` 解決（`TAKO_GIT_BIN` > PATH > 既知パス > ログインシェル）、
  `log_commits`（`--format` + SOH 区切り + `--all`）、`list_branches`（`-a --sort=-committerdate`）、
  `status`（`--porcelain=v2`）、`diff`（unstaged / staged / commit）。パーサは全て unit test 済み
- **dispatch `GitLog` / `GitDiff`**: pane の cwd → `repo_root` → データ取得 → JSON 応答。
  CLI `tako git log` / `tako git diff` + MCP `tako_git_log` / `tako_git_diff`（**計 25 ツール**）
- **git パネル（`render_git_view`）**: 右サイドバーの git タブを置き換え。
  4 セクション（ブランチ一覧 / 変更ファイル / コミットグラフ / diff）のアコーディオン構成。
  コミットクリックでそのコミットの diff を表示（即座に background fetch）。
  cwd 連動は 2 秒ポーリング（tmux / filetree と同ループ。パネル表示中のみ取得）+
  パネル開いた瞬間の即時 fetch。全てスクロール可能（`overflow_y_scroll`）
- **diff 表示**: 追加行（緑）/ 削除行（赤）/ コンテキスト行の着色。ファイルパス + ハンクヘッダ付き。
  FR-3.9 の「diff ビューアペイン」（独立ペイン種別）は将来の `show_diff` 実装で差し替え可能

FR-3.10 のユースケース: AI が生成・取得した画像（アイコン案、スクリーンショット、グラフ、
デザイン案）をユーザーに見せるとき、Finder や外部ビューアを開かせず tako のペイン内に
直接表示して提示する（FR-2.7 の成果物プレゼンテーションの一部。複数案の並列比較
FR-2.7.6 は画像ペインを並べて実現する）。
経緯: アイコン選定で AI が Finder 経由で画像を見せる体験になったが、本来 tako 内で
完結すべきだった、という実例からの要件（2026-06-11）。実装は Phase 5 のプレビュー系。

## FR-4 テーマ・外観・設定

> よくある設定系（カラースキーム、ライト/ダークテーマ、アクセントカラー、フォント・フォントサイズ等）を
> ひととおり持ち、**全項目が設計原則 5「AI フルコントロール」の対象**。
> ユーザーは設定ファイルを直接編集してもよいし、AI に頼んでもよい。
> 例:「ライトテーマにして」→ AI がライトテーマへ切替、
> 「もうちょっと暗く」→ AI が背景色を灰色寄りに微調整、
> 「差し色を青に」→ AI がアクセントカラーだけ変更。

| ID | 要件 | 優先度 |
|---|---|---|
| FR-4.1 | カラースキーム（背景・前景・ANSI 16 色・カーソル・選択・アクセントカラー・タブバー等の UI 色）のカスタマイズ | M |
| FR-4.2 | ライト / ダークテーマの切替（プリセット同梱を含む） | M |
| FR-4.3 | フォント（ファミリ・サイズ・行間）の変更 | M |
| FR-4.4 | 設定は人間も編集可能な**構造化された設定ファイル**に永続化する（FR-1.8 と同一ファイル） | M |
| FR-4.5 | 全設定項目を MCP / CLI から**読み取り・変更**できる（設計原則 5。個別色の微調整単位まで） | M |
| FR-4.6 | AI / 設定ファイルからの変更は**再起動なしで即時反映**される | M |

実装指針: 描画色・フォントはコードに直書きせず、`tako-core` の `Theme` 構造体（GPUI 非依存）を
一枚通して参照する。Phase 1 はデフォルトダークテーマ 1 つでよいが、全描画色が Theme 経由で
引かれる構造を最初から守り、後の AI テーマ操作（Phase 3 以降）で手戻りしないようにする。
設定ファイルの形式・読み込み・テーマ操作の MCP / CLI 公開は Phase 3 以降で設計する。

実装メモ（2026-09-27、FR-4.4 / FR-4.6 / #1756 / #1763）: settings.json の**読めない色の上書き**
（`#赤色` / `#12345` 等）は起動も読み直しも止めず、その色だけ既定へ落とす（settings.json は
書き換えない）。理由は persist.log へ `テーマの色上書きを無視: <キー>: <理由> [pid N]` の形で残す。
**起動時と実行中の読み直し**（`ControlHost::reload_theme` = `tako theme` / MCP `tako_theme` /
設定画面、およびタブバー・メニュー・⌘K のトグル）は同じ 1 実装（tako-app の `load_theme_logged`
→ `tako_control::settings::ThemeWarningLog`）を通る。読み直しは色を 1 つ変えるたび・テーマを
切り替えるたびに走るので、**前回と同じ警告は出さない**。増えた警告は上の形式で、消えた警告は
`テーマの色上書きの警告が解消（読み直した設定では出ない）: <キー>: <理由>` で出す（手で直したとき
のほか、テーマを切り替えてその上書きを読まなくなったときも出る）。settings.json 自体が壊れた JSON
のときは、dispatch が読み直しの前に既定値で保存し直す（壊れた中身は `.unreadable.bak` へ退避。
#916）ので、記録済みの警告の「解消」が出る。構造の番犬は
`crates/tako-control/tests/issue1763_theme_reload_warnings_watchdog.rs`、実経路は
`scripts/test-theme-reload-1763.sh`（CLI / MCP の読み直し・トグル・壊れた JSON）。
**応答にも載せる**（#1820）: `tako theme` / MCP `tako_theme` の status / set / toggle の応答へ
`warnings`（`<キー>: <理由>` の配列 = persist.log の行の本体と同じ文字列）を足す。引くのは
GUI が**適用中のテーマ**の帳簿（`ThemeWarningLog::current` → `ControlHost::theme_warnings`）で、
settings.json を読み直した値ではない（監視は無いので、手で直して未反映の値を応答にだけ
出すと persist.log と食い違う）。警告が無いときはキーごと出さない（応答は #1820 以前と
バイト一致）。実経路は `scripts/test-menu-theme-1820.sh`。
**`colors` も応答だけで見分けられる形にする**（#1830）: `tako theme colors` / MCP `tako_theme colors`
の各色の `source` は `builtin`（上書き無し）/ `override`（上書きが効いている）/ `ignored`（上書きは
あるが読めず、既定の色を使っている）。以前は上書きの有無だけで決めていたので、読めない色も
`override` と答えていた（hex は既定の色なので、応答だけを見ると上書きが効いて見える）。
判定は `apply_overrides` と同じ `parse_hex_color` を通す `tako_core::theme::ColorSource::of`、
上書きの表は `resolve_theme` と同じ `Settings::active_theme_overrides` の 1 本で、応答は
`dispatch::theme_colors_response` が組む（CLI / MCP 共通）。`warnings` も status と同じ
`<キー>: <理由>` で載せる（未知のキーはここにだけ出る）が、引くのは hex と同じ settings.json の
解決で、GUI の帳簿ではない（`colors` の hex はもともと settings.json から解いている。帳簿から
引くと、手で直して未反映の値で hex と warnings が食い違う）。無いときはキーごと出さない
（応答は #1830 以前とバイト一致）。実経路は `scripts/test-theme-colors-ignored-1830.sh`。
**`colors` の `target` はそのテーマを引く**（#2321）: `tako theme colors --target <dark|light|プリセット名>` /
MCP `tako_theme colors` の `target` は、いまのテーマ（settings.json の theme・GUI のモード）に依らず、
そのテーマの色・出どころ・`warnings` を返し、`theme` もそのテーマの名前（ビルトインは正規形の
`dark` / `light`）になる。以前は CLI も MCP も target を送るのに dispatch が読み捨て、常にいまの
テーマを返していた。解決は `Settings::resolve_theme_named` / `theme_overrides_named`（名前で引く 1 本。
`resolve_theme` / `active_theme_overrides` はいまの theme を渡す薄い包み）で、応答は同じ
`theme_colors_response`。知らない名前は黙ってダークへ落とさずエラー（理由にプリセットの一覧）。
target を省いた応答は #2321 以前とバイト一致。実経路は `scripts/test-theme-colors-target-2321.sh`。
**書き込みの `target` も同じ解決を通す**（#2331）: `tako theme color <キー> <値> --target <名前>` /
`tako theme color <キー> --reset --target <名前>` / `tako theme reset-colors --target <名前>`（MCP `tako_theme` の
set-color / reset-color / reset-colors の `target`）は、colors と同じ `dispatch::theme_target_name`
（`Settings::known_theme_name` で解く 1 本）で書く表のテーマを決め、プリセットならその colors、ビルトインなら
`theme_colors` のそのモードの表へ書く。知らない名前は**何も書かずに** colors と同じ文言のエラー（CLI は非 0・
MCP は isError）で断る。以前は書き込みだけ別の経路で、知らない名前を GUI のいまのモードの表へ落とし、
打ち間違えた名前でいまのテーマの上書きを書き換え・消していた。表への当て方は `dispatch::apply_color_write` の
1 本で、保存しない経路（セルフテスト）でも知らない名前は断る。target を省いたら従来どおり settings.json の
theme の表（theme が知らない名前なら GUI のモードの表）で、応答と書いた結果は #2331 以前とバイト一致。
実経路は `scripts/test-theme-write-target-2331.sh`。

### FR-4.7 プロファイルの GUI 編集（Issue #721。✅ 2026-08-01）

> `tako master` / `tako solo` の起動設定（`profiles/*.yaml` / `solo-profiles/*.yaml`）は
> これまでセットアップエージェントか手編集でしか触れなかった。スキーマは #127 / #500 / #504 で
> 出揃っているので、設定画面（FR-4 / #459）の 1 タブとしてフォームで編集できるようにする。

| ID | 要件 | 優先度 |
|---|---|---|
| FR-4.7.1 | 設定画面に「プロファイル」タブを置き、**種別（master / solo）で一覧を切り替え**て選択・編集する。保存形式（yaml のキー構成）は変えない（#513 の設定共有と互換維持） | M |
| FR-4.7.2 | 編集可能項目は `tako orchestrator profiles set` の全項目 + `projects`: master_agent / model / effort / worker_model_policy / worker_model / worker_effort / worker_agent / エージェント別設定（model・effort・skip_permissions・args）/ master_account・worker_account / projects / tab_naming_convention / env | M |
| FR-4.7.3 | **書き込みは既存 dispatch（`OrchestratorProfiles`）経由**とし、UI から yaml を直接書かない（#169 の config_io のロック・アトミック書き込み・世代バックアップと、CLI / MCP 側の検証がそのまま効く） | M |
| FR-4.7.4 | 新規作成・複製・削除を GUI から行える。**削除は確認つき**で、`default` は起動時のフォールバック先なので削除できない | M |
| FR-4.7.5 | 選択肢が既知の項目（エージェント種別・effort・worker ポリシー・アカウント・プロジェクト）は**選択式**にして自由入力を排除する。モデル名は上流 CLI のリリースごとに変わるため既知の選択肢を持てず、自由入力 + 空欄 = 「その CLI の既定」とする（#27 の推奨を既定に置く） | M |
| FR-4.7.6 | **参照整合性の警告**（未登録 project キー / 未登録アカウント名 / `[1m]` モデル）を保存前に表示する。アカウント・プロジェクトは登録済みの値しか選べないので GUI から未登録参照を作れず、手編集・CLI 由来の既存の不整合は警告として見せて直せるようにする。判定は `orchestrator::profile_warnings` の 1 実装で、list / show / set の応答に `warnings` として載る（CLI / MCP も同じ文言を得る） | M |
| FR-4.7.7 | 変更は**次回の起動から有効**（実行中の master / worker には影響しない）ことを UI に明記する。プロファイルは起動時に 1 度だけ読まれるので、使用中プロファイルの編集・削除でも稼働中セッションは落ちない | M |
| FR-4.7.8 | パースできない yaml のプロファイルは一覧に**エラー付きで表示**し（隠さない）、フォームの編集は止める（既定値に丸めた上書きで設定を失わせない。#169 と同じ方針） | M |
| FR-4.7.9 | GUI で増えた操作は CLI / MCP へ 1:1 公開する（開発不変条件）: `tako orchestrator profiles create/copy/delete` と `--solo` / `--projects`、MCP `tako_orchestrator_profiles` の `action: create/copy/delete` + `kind` / `from` / `projects` / `clear_projects` | M |
| FR-4.7.10 | プロファイル名は英数字と `-` `_` `.` のみ許す。先頭の `.`（隠しファイル・`..` の親参照）と先頭の `-`（`tako master --x` としてオプション扱いされる）は拒否する | M |
| FR-4.7.11 | フォームは「ラベル（左）+ コントロール（右）」で整列し、**チップ群は行の残り幅で折り返す**。重なり・隣の行への食い込み・本文幅からのはみ出しはゼロで、低いウィンドウ高さでもスクロールで全項目へ到達できる（Issue #738）。`flex_wrap` のコンテナは**幅が確定していないと折り返し幅を決められない**: 幅 auto の親（`row()` の `flex_none` セル）の中では taffy が wrap コンテナの max-content 幅を「一番広い項目 1 個ぶん」として返すため、チップが 1 個ずつ縦に折り返される一方で行の高さは 1 行ぶんで見積もられ、伸びたチップ群が次の行の上へ重なって描かれた（実測: effort 群が 5 段 149.5px に伸びる一方、行の高さは 39.5px）。チップ行は `row_wrapping()` の「行の残り幅を持つ右寄せセル」に入れて幅を確定させる。回帰は visual-test の重なり検査（実測矩形の総当たり）で押さえる | M |

### FR-4.8 検証用 GUI をユーザーのメイン画面に出さない（Issue #1141。✅ 2026-09-06）

> 隔離 GUI（`TAKO_ISOLATED=1` の tako-app・セルフテスト・visual-test）は検証のたびに何度も立ち、
> そのつど**ユーザーのメイン画面の前面に窓が出て作業を妨げる**（2026-09-06 のユーザー指示:
> 「録画の UI が画面に出てきて邪魔すぎる」→「ほかのデバッグ系も仮想ディスプレイでやらせて。
> 邪魔すぎる。**今後もずっと**」）。窓を出す先を tako 自身が決められるようにする。
>
> **別 Space へ逃がすのでは代わりにならない**: GPUI は窓が完全に隠れる（他窓に覆われる /
> 表示中でない Space にある）と描画を止める（#470 の実測。同じ絵が撮れ続け、描画依存の
> セルフテスト項目が進まない）。退避先は OS から**実ディスプレイとして見える面**が要る。

| ID | 要件 | 優先度 |
|---|---|---|
| FR-4.8.1 | `TAKO_DISPLAY=<名前 \| UUID \| index>` で起動時の窓をそのディスプレイの中央へ置く。当てる順は **UUID → 名前 → 記録済み uuid（名前が読めない面だけ。FR-4.8.17）→ index**（UUID は構造的に紛れない / 数字の名前を付けられても名前が勝つ）。判定の正は `tako_core::platform::display`（抽象境界 B26。候補の一覧を受け取る**純粋関数**なので macOS 上から他プラットフォームの挙動も検証できる） | M |
| FR-4.8.2 | **検証用の起動は指定が無くても**常設の仮想ディスプレイ（既定名 `tako-vd`）を狙う。対象は `TAKO_ISOLATED` / `TAKO_SELF_TEST` / `TAKO_VISUAL_TEST` の**いずれか**。隔離だけに絞ると「セルフテストはユーザーの画面」という半端な状態が残る（実装中に 1 度踏んだ） | M |
| FR-4.8.3 | **通常起動の挙動は 1 ビットも変えない**。`TAKO_DISPLAY` 未指定 かつ 非検証なら置き先は未解決のままで、以降の分岐がすべて既定へ落ちる。名前引きの費用（macOS は `system_profiler`）も指定が要るときにしか払わない | M |
| FR-4.8.4 | **通常起動は見つからなくても止めない**。`TAKO_DISPLAY` が外れたら既定の面へ落として理由を persist.log へ 1 行残す（指定が外れるのは検証の都合であって、tako が起動できない理由ではない）。行には**候補の一覧**も載せる。**検証用の起動は落とさない**（FR-4.8.15） | M |
| FR-4.8.5 | tako が開く窓は**全部同じ面**へ出す（メイン + 追加ビューポート + 設定 / About / アップデート）。1 枚でも素の中央寄せが残ると、セルフテストが開く設定画面などがユーザーの画面へ飛び出す | M |
| FR-4.8.6 | 置き先が決まっているときは**保存済みウィンドウフレーム（FR-5）より置き先が勝つ**。保存座標はメイン画面のものなので、そのまま使うと隔離起動の窓がユーザーの画面へ戻る。ユーザーの保存値は書き換えない（`TAKO_DISPLAY` を外せば元の位置に戻る） | M |
| FR-4.8.7 | どこへ置いたかを **CLI / MCP から読める**（`tako_check_health` の `display_placement`: `requested` / `resolved` / `matched_by` / `reason` / `available` / `name_lookup_supported` / **`retries`** / **`refused`** / **`miss_policy`** / **`verification`** / **`explicit`**（#1697: 指定が `TAKO_DISPLAY` の明示か）。`matched_by` は `uuid` / `name` / **`recorded_uuid`**（FR-4.8.17）/ `index`）。狙って外したときだけ `issues` に出す（当たっているときは黙る）。**既定の面へ落ちた = warning / 窓を開かずに終わった = error** と重さを分ける（前者はユーザーの画面に窓が出ている状態、後者は目的どおりの拒否）。`miss_policy` は起動時に決めた値をそのまま持ち回る（診断側で再計算しない = 判定を散らさない）。設計原則 5 を満たすために**新しい MCP ツールは増やさない** | M |
| FR-4.8.8 | 面を用意するのは tako 本体ではなく `scripts/lib/virtual-display.sh`（`ensure` / `bounds` / `status` / `recorded-uuid` / `move-window` / `cleanup-orphans`）。**仮想ディスプレイを消す機能を持たない**（`tako-vd` は常設。作業のたびに作り直さない）。ユーザーのディスプレイ構成・解像度・配置・ミラーリングにも触らない。落とせるのは器の管理外へ外れた残骸（孤児）だけで、それも FR-4.8.13 の実行条件を満たすときに限る | M |
| FR-4.8.9 | 仮想ディスプレイの器（現状は BetterDisplay）の応答文は**当てにしない**。`set -connected=on` が「Failed.」と言いながら実際は繋がり、`get -connected` と identifiers の `displayID` はどちらも接続中に off / 0 を返す（実測）。**繋がっているか = 窓を置けるか**なので判定は NSScreen を正にする。接続は tagID 指定で 1 回だけ（`-name=` 指定は複数に当たり同じ画面が何枚も繋がりうる）で、tagID は操作のたびに並べ替わるので**打つ直前に引き直す** | M |
| FR-4.8.10 | Windows は仮想ディスプレイを作る手段自体が別なので**名前引きを配線していない**（`name_lookup_supported` が false）。`TAKO_DISPLAY` に UUID か index を渡せば選択そのものは効く。追跡は #1141 | S |
| FR-4.8.11 | **`ensure` は同名の面を増やさない**（#1150）。用意できた直後に「その名前の NSScreen がちょうど 1 枚か」を確かめ、2 枚以上なら理由（枚数・各行・器の実体数・下見のコマンド）を出して**非ゼロで止まる**。macOS は同名の面に枝番（`tako-vd（1）`）を付けて見せるので、完全一致で数えると「その名前は無い」に見えてもう 1 枚繋ぎ、増殖に気づけない | M |
| FR-4.8.12 | **仮想ディスプレイを Main にしたままにしない**（#1150）。`ensure` の締めで「Main が `tako-vd` かつ 内蔵が居る」ときだけ内蔵を Main へ戻す。**内蔵が居ないとき（蓋閉じ）は何もしない**（戻す先が無い）／外部モニタが Main のときも触らない（ユーザーの構成）。判定は stdin のテーブルだけを見る純関数（器に触らずスタブで検証できる）で、戻せなくても `ensure` は成功させる（面は用意できているので検証は進められる） | M |
| FR-4.8.13 | **孤児の後片付けは既定が下見**（#1150）。`cleanup-orphans` は列挙と実行条件の判定だけを行い何も変えない。掃除（`--apply`）が走るのは **内蔵が NSScreen に居る かつ 蓋が開いている**ときだけで、満たさなければ理由を出して終了 2 で拒否する。器の再起動は一瞬すべての仮想ディスプレイを落とすので、条件を満たさないまま撃つと**画面が 0 枚になり機械が眠って走っている worker が全部巻き添えになる**。内蔵の有無と蓋の状態は**独立に**見る（片方の読み違いで通らない） | M |
| FR-4.8.14 | **列挙が空でも 1 回で諦めない**（#1160）。macOS の `cx.displays()` は `CGGetActiveDisplayList` そのもので、gpui 自身が「眠っている機では active な一覧が返るとは限らない」と書いている（実測 2026-09-07: ディスプレイスリープ中は `NSScreen` に 2 枚残ったまま `CGDisplayIsActive` が両方 0 = **列挙 0 件**）。**面が 1 枚も見えないあいだだけ**短い予算でやり直す（間隔 100ms / **検証用は 20 回 = 2 秒**・通常起動は 3 回 = 0.3 秒）。予算を分けるのは、開いてしまうと取り返せない側だけ長く待てばよいから。**面が見えているのに当たらないときは待たずに決める**（読めている一覧に無いなら答えは変わらないので、置き先を配線していない環境で毎回待たせない）。やり直した回数は persist.log と `display_placement.retries` に残る | M |
| FR-4.8.15 | **面が 1 枚も見えないとき、検証用の起動は既定の面へ落ちない**（#1160）。やり直しても列挙が空なら**窓を 1 枚も開かずに終了する**（終了コード 4）。ユーザーのメイン画面へ検証用の窓を出すのは「開かない」より悪く、それを許すと FR-4.8.2 の既定が黙って縮退する（#1160 の症状）。**分かれ目は「空か」で、検証用かどうかだけでは決めない**: 面が見えているのに当たらないのは（**`TAKO_DISPLAY` 未指定の暗黙の既定なら**。明示なら FR-4.8.18）「その機に置き先が無い」ということで、そこで開かない構えにすると `build-app.sh --verify` と **Windows 実機のセルフテスト**（FR-4.8.10 により `tako-vd` を名前で引けない）が起動できなくなる。落ちる側も**黙らせない**（起動時に stderr へ警告 + `issues` に warning）。理由は persist.log だけでなく**起動時に stderr へ**出す（persist.log を読むまで気づけなかったのが症状の一部）。セルフテストのときは `TAKO_APP_SELF_TEST_FAILED` 行も出して無音終了にしない。**通常起動（明示 `TAKO_DISPLAY`）の挙動は変えない**（FR-4.8.4 のまま既定の面へ落ちる） | M |
| FR-4.8.16 | **`ensure` の完了条件に「面が起きている」を含める**（#1160）。NSScreen に居る = 窓を置ける、ではない（眠っている面は CoreGraphics の active 一覧から落ちる = tako から見えない）。締めで描画可能かを確かめ、眠っていれば**ユーザー活動の宣言（`caffeinate -u`）だけ**で起こして待つ。起こせなければ理由を出して**非ゼロ**（面を用意したと嘘をつかない）。判定は「証明できるときだけ断る」= 材料が 1 行も読めない環境（CI・`osascript` 不可）では眠っていると決めつけない。`status` は「在る」と「置ける」を別に出し、**`status --snapshot` には眠りを入れない**（前後比較は構成の diff を見る道具で、眠りは構成ではない） | M |
| FR-4.8.17 | **名前が読めない瞬間に面を見失わない**（#1697）。名前の出どころ（macOS は `system_profiler`）が読めない起動では面は見えているのに全部の面が `name=?` になり、生きている `tako-vd` を名前だけで探して外した（2026-09-24 の実測: 候補 2 枚とも `name=?`・うち 1 枚の uuid が `tako-vd` そのもの・直後の `status` は「使用可」）。`ensure` の締めが**面が起きていると確かめたあとで**その面の uuid を `~/Library/Caches/tako/virtual-display/<小文字の名前>.uuid`（`TAKO_VD_RECORD_DIR` で差し替え可・**tako 側 `platform::display::record_dir` と同じ場所**）へ残し、tako は名前で当たらないとき**名前が読めない面に限って**その uuid で当てる（`matched_by=recorded_uuid`）。名前が読めていて別の名前の面には当てない（記録より、いま読めた名前が正 = 作り直し・改名のあとに古い記録で別の面へ出さない）。data dir に置かないのは隔離起動ごとに一時 dir へ変わる（= 読めない）ため。記録は冪等（同じ値なら書かない）で、uuid が読めなくても `ensure` は失敗させない（記録が無いときは名前だけで探す = #1697 前と同じ）。`status` に記録の先頭 8 桁と「いまの面と一致 / 不一致」を出す。uuid は **NSScreen に居る面にだけ**聞く（無効な displayID を `CGDisplayCreateUUIDFromDisplayID` へ渡すと JXA が NULL を見分けられず osascript が SIGSEGV で落ちる = 実測） | M |
| FR-4.8.18 | **明示の指定を見失ったら検証用の起動は既定の面へ落ちない**（#1697）。`TAKO_DISPLAY` を明示したのに、面は見えているのにどれにも当たらないのは「その機に置き先が無い」ではなく「狙った面を見失った」なので、**窓を開かずに終了する**（終了コード 4・stderr に理由と**候補の全行**・persist.log・`issues` に error）。**検証用 GUI が既定の面へ落ちるのは `TAKO_DISPLAY` 未指定（暗黙の既定）のときだけ**（置き先を配線していない CI・他人の機・Windows のため。FR-4.8.15）。通常起動は明示でも止めない（FR-4.8.4）。`scripts/lib/isolated-gui.sh` は面を用意できたら**常に明示する**（用意できなければそもそも起動しない = FR-4.8.19）。A/B は `TAKO_1697_LEGACY=1`（記録を見ず、明示でも面が見えていれば落ちる）、名前が読めない瞬間の再現は `TAKO_1697_INJECT_NO_NAMES=1`（名前引きを空として扱う）。実経路は `scripts/test-display-uuid-1697.sh` | M |
| FR-4.8.19 | **検証スクリプトは面を用意できなければ隔離 GUI を起動しない**（#1744）。`scripts/lib/isolated-gui.sh` の `launch_isolated_gui` は起動の直前に `virtual-display.sh ensure` を通し、**失敗した・成功と言ったのに面が OS の一覧に無い**（`bounds` で読み戻す）ときは窓を開かずに**終了コード 4**（tako 本体の `REFUSED_EXIT_CODE` と同じ番号）で返し、stderr へ「未実測: …」を 1 行出す。以前は「起動は続ける」で素通しする作りで、面の指定を持たない起動が tako の暗黙の既定（FR-4.8.15 の「見えている面へ落ちる」）に乗ってユーザーの画面へ窓が出うる潜在経路があった（この道で窓が出た実例は確認されていない）。蓋閉じ + ディスプレイスリープでは `ensure` が面を起こせないのが日常なので、未配線の機（器が無い・CI）も含めて**既定の面で続行する道は作らない**。呼び出し側は `launch_isolated_gui … \|\| exit $?` で止めて報告に「未実測」と書く（戻り値を受けて自分で未実測を明記して続ける形も可）。番犬は `crates/tako-control/tests/issue1490_isolated_gui_launch_watchdog.rs`（ヘルパを `/bin/bash` で実際に走らせて「用意できない面」を注入し、偽の GUI が起きないことを見る + `scripts/test-*.sh` の起動行が失敗を拾っているか） | M |
| FR-4.8.20 | **検証スクリプトは `tako-vd` 以外の面の明示を通さない**（#1760 / #1783）。`launch_isolated_gui` は GUI へ渡す `TAKO_DISPLAY`（呼び出し側の `VAR=VAL` → export → 既定の `tako-vd` の順で 1 つに決めた値）を判定し、通すのは `tako-vd` の名前（大文字小文字・前後の空白は無視）/ `ensure` が記録した `tako-vd` の uuid / 空（tako の定義で未指定）/ 実在し得ない index（N ≥ 100）だけ。それ以外（`0` などの実在しうる index・別の面の名前・当たらないはずの名前・記録と違う uuid）は窓を開かずに**終了コード 2**で返し、stderr へ「ERROR: TAKO_DISPLAY=… は通さない…」を 1 行出す（4 = 未実測とは分ける）。判定は**面を起こす前**に行う（#1783。面を用意できない機で書き方の誤りを 4 に紛れさせない）。**uuid の記録との突き合わせだけは面を起こした後**に残す: 記録は `ensure` の締めが書くので、起こす前に読むと初回（隔離した HOME）や面の作り直しでいまの `tako-vd` の uuid を断る（起こす前は uuid の形だけを見る）。GUI へ渡すのは判定した値で、呼び出し側の `VAR=VAL` より後ろに置く。表と例は `.agent/conventions.md`「窓を出す検証はユーザーの画面に出さない」。番犬は同上（面を用意できない係の下で書き方の誤りが 2 で断られ、係の `ensure` が呼ばれないこと + `ensure` が初めて書いた記録の uuid が通ること） | M |
| FR-4.8.21 | **検証スクリプトは検証用 GUI の印（`TAKO_ISOLATED`）を偽にさせない**（#1784）。`launch_isolated_gui` は GUI へ常に `TAKO_ISOLATED=1` を呼び出し側の `VAR=VAL` より後ろで渡す。呼び出し側が偽（tako の `is_verification_gui` が真と読む `1` / `true` / `on` 以外。空も含む。値は `VAR=VAL` → export の順で 1 つに決める）を渡したら、**面を起こす前に**窓を開かずに**終了コード 2**で返し、stderr へ「ERROR: TAKO_ISOLATED='…'（出どころ）は tako では偽と読む…」を 1 行出す。真へ書き換えて続行しないのは、偽を求めた検査を隔離の挙動で測って PASS と読ませないため。番犬は同上（値ごとにヘルパの判定を `is_verification_gui` と突き合わせる） | M |
| FR-4.8.22 | **面の名前が物理画面を指していたら使わない**（#1783）。面の名前の正本は `TAKO_VD_NAME` なので、そこへ実在の物理画面の名前（内蔵ディスプレイ・外部モニタ）を入れると、以前の `virtual-display.sh ensure` はその面を `tako-vd` と見なして成功し（締めがその面の uuid を記録し、内蔵なら Main 保護が器へ main を撃つ）、`isolated-gui.sh` もその名前を通していた。`ensure` の締めは**最初に**「器が作った仮想ディスプレイか」を確かめ（`vd_assert_virtual`）、**内蔵（`CGDisplayIsBuiltin`。器に聞かずに決まる）**か**器の仮想スクリーン一覧にその名前が無い**なら**終了コード 3**（`VD_RC_NOT_VIRTUAL`）と理由 1 行で断る。器に聞けず確かめられないときも通さない（終了コード 1。窓を出す側へは倒さない）。ヘルパは 3 を「使い方の誤り」として**終了コード 2**で返し、stderr へ「ERROR: TAKO_VD_NAME=… は仮想ディスプレイではない…」を 1 行出す（4 = 未実測と読ませない）。判定は stdin のテーブルと引数だけを見る純関数（`vd_virtual_verdict`）。番犬は `crates/tako-control/tests/virtual_display_watchdog.rs`（本物の `virtual-display.sh` を `/bin/bash` で走らせ、画面と器をスタブにして物理画面の名前で 3・記録なし・main を撃たないこと）+ 同上（係の 3 がヘルパで 2 になること・番号の突き合わせ）+ `scripts/test-virtual-display-guard.sh` | M |
| FR-4.8.23 | **visual-test / セルフテストの節は落ちても一時ファイルを残さない**（#2051）。`fail()` は即 `exit(1)` するので節の末尾の後片付けは落ちた経路で走らず、fixture や受け手のファイル（#2046 では偽の鍵を受けたファイル）が `$TMPDIR` に残っていた。節は作る場所を `remove_on_fail` で登録し、`fail()` が exit の前に登録簿を空にしながら消す（`TAKO_APP_SELF_TEST_CLEANUP: registered / removed / left` の 1 行）。登録できるのは一時置き場（`std::env::temp_dir()` / `TAKO_DATA_DIR`）より下だけで、置き場そのもの・`..`・外のパスは `fail()` で止める。登録は足すだけで外さない（`let _ = guard` で黙って消える形を作らない）。A/B は `TAKO_2051_LEGACY=1`（消さずに exit）。実経路は `bash scripts/test-visual-fail-cleanup-2051.sh`（既存の A/B で paste-guard と tree-multiselect を本当に落とし、data dir の下と `$TMPDIR` の下の両方で新は残らず旧は残る・正常に通る節は変わらない） | M |
| FR-4.8.24 | **検証スクリプトの隔離 GUI は本物の claude・本番の tako へ届かない既定で起こす**（#1970）。`launch_isolated_gui` は本番 tako と呼び出し元の claude がペインへ配る env（`ISOLATED_GUI_DROP_ENV`。制御プレーン・器・mod・orchestrator の役割・claude のセッションの印）を GUI へ渡さず、自動リネームを止め（`TAKO_AUTO_RENAME=0`）、export された `CLAUDE_CONFIG_DIR` を使わない（GUI の HOME が一時の dir なら外し、利用者のホームのままならログの置き場の隣の一時 dir）。GUI の ZDOTDIR か HOME が利用者のホームの外で zsh の rc が無ければ既定の `.zshrc`（固定プロンプト）を置く。どれも呼び出し側が `VAR=VAL` を並べれば外せる（本物の認証で実物の claude を動かす検証は `CLAUDE_CONFIG_DIR="$CCD"` を明示する）。番犬は `verification_isolation_watchdog`（落とす一覧を本番の注入箇所と突き合わせ、ヘルパを偽の GUI へ起こして env を見る。検証スクリプトが自分のプロセスで claude / tako を起こす前に本番の env を落としていなければ file:line で名指す）。実経路は `scripts/test-isolated-gui-defaults-1970.sh` | M |
| FR-4.8.25 | **器に作らせる・繋がせるのは常設の `tako-vd` だけ・`bounds` / `status` / `move-window` も物理画面を断る**（#1817）。以前の `virtual-display.sh ensure` は OS の一覧に居ない名前なら何でも器（BetterDisplay）に仮想スクリーンを作らせて繋ぎ（存在しない名前・大文字違い・蓋閉じで一覧に居ない内蔵の名前でも）、作った面は器の一覧に載るので FR-4.8.22 の判定も通って成功していた。作る・繋ぐ道は**器に触る前に**名前が `VD_DEFAULT_NAME`（= `tako-vd`。Rust の `DEFAULT_VIRTUAL_DISPLAY_NAME` と同じ）と完全一致かを確かめ（純関数 `vd_create_verdict`）、違えば**終了コード 3**（`VD_RC_NOT_VIRTUAL`）と理由 1 行で断る（器の起動・一覧の読み出し・作成・接続のどれもしない。ヘルパ経由なら 2）。器が作って繋いである別名の面を使うのは止めない（判定は FR-4.8.22 の締め）。`bounds` / `move-window` は仮想ディスプレイと確かめられた面にだけ座標を返す・窓を動かす（物理画面は 3、確かめられなければ 1）、`status` は物理画面の名前に「仮想ディスプレイではない」と答えて 3（Main 保護も判定しない）。この 3 つは読むだけなので器を起動しない（走っていれば 1 回だけ聞く）。番犬は `crates/tako-control/tests/virtual_display_watchdog.rs`（本物の `virtual-display.sh` を `/bin/bash` で走らせ、画面・器・AX をスタブにして作成・接続を撃たないこと / 座標・使用可を返さないこと / 器を起こさないこと）+ `scripts/test-virtual-display-guard.sh` | M |
| FR-4.8.26 | **セルフテストが FAILED で抜けても、成功の出口と同じ後片付けを通す**（#2388）。失敗の出口（`self_test::fail`）は `exit(1)` で終わって `on_app_quit` を通らないので、FAILED のたびに器のサーバー（`tako-iso-<pid>` / `tako-st-<pid>`）・ソケットファイル・検証用の設定置き場（`$TMPDIR/tako-agent-config-<pid>`）が残っていた（`tako tmux cleanup --servers` が後から拾う型だが、落ちるたびに増える）。片付けは `cleanup_process_scoped` の 1 実装（自分の pid から作った使い捨てのソケットだけを落とす = 明示の `TAKO_TMUX_SOCKET` には触らない #770・検証起動の設定置き場を消す #1253）で、`on_app_quit` と `fail()` の両方がこれを呼ぶ。**設定置き場を先に消してから器を落とす**（ペインのシェルの `HISTFILE` はこの置き場なので、器を先に落とすと器の中のシェルが SIGHUP で履歴を書き、書いている最中の rmdir が断られて `shell_history` 入りの dir が残る = #2388 の実測。器の中にシェルが居るまま落ちる FAILED の時点で起きる。成功の出口は最終項目までに器の中のシェルが居なくなるので、修正前の main でも残らなかった）。`fail()` は FAILED の行を出した**後**に別スレッドで上限 5 秒（#777 の終了の猶予と同じ）まで待ち、何を片付けたかを `TAKO_APP_SELF_TEST_EXIT_CLEANUP: backend=<ソケット> agent_home=removed left=none` の 1 行で出す。器が応答しない・後片付けが panic しても上限で置いて exit するので、落ちた理由の行と終了コード 1 は変わらない。配線は番犬 `selftest_exit_cleanup_watchdog`（`fail` と `on_app_quit` が呼ぶ・FAILED の行を `fail` の外で出さない（逃げ道は `// tako:exit-cleanup-ok <理由>`）・使い捨ての判定は 1 実装の中だけ）が `main.rs:<行>` で名指しする。A/B は `TAKO_2388_LEGACY=1`（片付けずに exit）、上限の検証は `TAKO_2388_INJECT=hang`。実経路は `bash scripts/test-selftest-failed-cleanup-2388.sh`（既存の注入 `TAKO_2371_INJECT=leak` で項目 154 を本当に落とし、新は残らず旧は残る・固まっても理由と終了コードは変わらない・成功の経路は変わらない） | M |

### FR-4.9 スクロールバック保持上限の設定（Issue #818。✅ 2026-09-09）

> 直接ペイン（tmux バックエンドを使わないペイン）の履歴は alacritty の `Grid` が持ち、
> `Row<Cell>` が**列数ぶんのセルを常に確保**する（1 セル 24 B）。#814 の実測監査で、
> 履歴が飽和したペインが **+33 MB / ペイン**（119 桁 10,000 行。理論値 28.6 MB と 15% 以内で一致）
> を保持していることが分かった。既定は据え置き、**軽量運用向けに下げられる**ようにする。
>
> **既定運用では影響が小さい**: persist ON の tmux バックエンドペインは外側端末が
> alternate screen になるため行が履歴へ押し出されず、実測でも 0（12,000 行流して変化なし）。
> 効くのは persist OFF・tmux 不在環境・最初のペイン。

| ID | 要件 | 優先度 |
|---|---|---|
| FR-4.9.1 | 保持上限を `settings.json`（`scrollback_lines`）へ持つ。**既定は従来と同じ 10,000 行**で、キーが無い旧ファイルはそのまま読める（`#[serde(default)]` なので移行 Step は不要 = #916）。範囲は 100〜100,000。値の正本は `tako_core::scrollback`（既定・下限・上限・24 B/セルの見積り） | M |
| FR-4.9.2 | 明示指定（CLI / MCP / 設定画面）の範囲外は**黙って丸めず理由つきで弾く**。手書きの `settings.json` 由来の値だけは丸めて起動を成立させる（0 を書かれて履歴ゼロのペインが立つより、下限で起動するほうが直せる） | M |
| FR-4.9.3 | 変更は**生存中のペインへもその場で当たる**。alacritty の `set_options` → `Grid::update_history` が下げたぶんの `Row` を解放するので、下げれば RAM が実際に戻る（「次回作成から」にすると「設定したのに減らない」になる）。alt screen 中は非アクティブ側 = 履歴を持つ主グリッドへ当たるので TUI 表示中でも取りこぼさない | M |
| FR-4.9.4 | CLI `tako scrollback [lines]` / MCP `tako_scrollback` / 設定画面（一般タブ「ターミナル」節）の 3 経路が**同じ dispatch** を通る（設計原則 5）。最簡形（#322）は引数なしで現在値・既定・下限・上限・適用中のペイン数・飽和時の見積りが読めること | M |
| FR-4.9.5 | ペインログ（FR-5.13 / #112）の飽和判定は `TerminalSession::scrollback_limit()` = 設定値を見る（定数を別に持たない）。スクロール（FR-2.5.13）の可視範囲は `history_size()` 由来なので上限に自動追従する | M |

### FR-4.10 蓋閉じ継続をバッテリー駆動でも続ける（Issue #1473。✅ 2026-09-14）

> 蓋閉じ継続（#218 / #697。macOS = sudoers + `pmset disablesleep` / Windows = 電源プランの
> `GUID_LIDCLOSE_ACTION`）は **AC 接続時にしか適用されなかった**。実発の症状は
> 「iPhone でテザリングしながら蓋を閉じると、回線も master も worker も止まる」。
> スマホから操作する運用（#1059）はまさにバッテリー + テザリングで使う。
>
> **既定は変えない**（設定を触っていない人の Mac を鞄の中で走らせない）。明示的に
> 切り替えた人だけバッテリーでも継続し、そのときは安全弁を必ず通す。

| ID | 要件 | 優先度 |
|---|---|---|
| FR-4.10.1 | 蓋閉じ継続の電源条件を**アイドルスリープ側とは別の軸**として持つ（`settings.json` の `lid_sleep_power`。既定 `ac-only` = 従来どおり）。まとめて切り替えないのは、蓋を閉じて持ち歩くリスクとアイドルスリープ防止のリスクが釣り合わないため。キーが無い旧ファイルはそのまま読める（`#[serde(default)]` なので移行 Step は不要 = #916） | M |
| FR-4.10.2 | バッテリーで継続するあいだは**安全弁を必ず通す**: ①エージェント稼働中のみ（`while-agents-running` の意味は不変）②残量が下限（`lid_battery_floor`。既定 20%・5〜90% で設定可）**まで下がったら**解除 ③本体が高温なら解除（**物差しは電源で変える**: バッテリーは `fair` 以上 / AC は従来どおり `serious` 以上）④残量を読めない機械では継続しない（止める条件を持てないまま走らない）。条件が戻れば再適用する | M |
| FR-4.10.3 | 判定は `sleep_guard::lid_decision`（`Result<(), LidSkipReason>`）の **1 実装**で、真偽値ではなく**理由**を返す。`status` の description・CLI・設定画面の状態行・通知欄・`persist.log` はすべてこの理由を引く（画面側が条件を書き直さない）。倒した / 戻したは `persist.log` に `lid-sleep: … reason=<tag>` として 1 行ずつ残る | M |
| FR-4.10.4 | **通知欄へ出すのは安全弁による解除と、その回復だけ**（`lid_notice`）。エージェントが一段落した・AC を抜いたは設定どおりの動きなので出さない（作業のたびにバナーが出る）。`ac-only`（既定）の利用者には何も出さない | S |
| FR-4.10.5 | CLI `tako sleep-guard set --lid-power-condition <ac-only\|always> --lid-battery-floor <percent>` / MCP `tako_sleep_guard`（同名の引数）/ 設定画面「スリープ防止」タブの 3 口が**同じ dispatch** を通る（設計原則 5）。MCP の enum は正本 `PowerCondition::VALUES` から生成する（#1467）。残量下限 0（= 安全弁なし）は 3 口とも理由つきで弾く | M |
| FR-4.10.6 | 検証用の注入（`TAKO_1473_INJECT_BATTERY` / `TAKO_1473_INJECT_THERMAL`）は**隔離起動・セルフテストでのみ効く**（`inject_allowed`）。本番の GUI が env で `pmset` の判断を左右できてはいけない。旧挙動の A/B は `TAKO_1473_LEGACY=1`（材料を `ac-only` へ丸めるので判定の本体は 1 本のまま）。番犬は `crates/tako-control/tests/issue1473_lid_battery_watchdog.rs` | M |
| FR-4.10.7 | Windows も同じ判定を通り、`always` のときだけ電源プランの**バッテリーレールも倒す**（`set_stay_awake(_, include_battery)`）。ただし残量の取得は未実装なので、実質はバッテリーでは安全弁（`battery-unknown`）で降りる。**実機未検証**（#467 の一部） | S |

### FR-4.11 フォーカスの無いペインの再描画の上限（Issue #1979）

> 本番の tako-app の CPU 約 12% の正体は、エージェントの出力による描画（約 37 fps）だった
> （蓋を閉じて描画が止まると約 5%）。出力を見ているのはたいていフォーカス中の 1 枚なので、
> **フォーカスの無いペイン**（同じタブの別のペイン・裏のタブのタイトル / OSC の変化）の出力による
> 再描画にだけ上限を設ける。フォーカス中のペインは今のまま（判断は master・ユーザーから一任）。

| ID | 要件 | 優先度 |
|---|---|---|
| FR-4.11.1 | フォーカスの無いペインの出力による再描画を**上限の間隔でまとめる**（既定 30 fps = 33ms）。フォーカス中のペインは従来どおり 60 fps（16ms）で変えない。汚す範囲（ペイン本体だけ / アプリ全体）の決め方は #786 のまま。フォーカス中の再描画の時点で上限の間隔が来ていれば同じフレームへ相乗りさせる（フレームを増やさない）。配送側（#816）も Wakeup でメインスレッドへ渡る最小間隔を同じ上限へ揃える（渡っても描かないぶんの往復を省く） | M |
| FR-4.11.2 | 上限を `settings.json`（`unfocused_redraw_fps`）へ持つ。範囲は 1〜60。キーが無い旧ファイルはそのまま読める（`#[serde(default)]` なので移行 Step は不要 = #916）。手書きの 0 は既定へ、上限超えは上限へ丸める（起動は必ず成立させる）。値の正本は `tako_core::redraw_limit` | M |
| FR-4.11.3 | CLI `tako redraw-limit [fps]` / MCP `tako_scrollback` の `unfocused_fps`（ツールは増やさない）が**同じ dispatch**（`Request::Scrollback`）を通る（設計原則 5）。明示指定の範囲外（0 を含む）は理由つきで弾く。変更は**生存中のペインへその場で効く**。応答の `unfocused_redraw` に現在値・既定・範囲と、起動からの再描画の要求回数（`flushes.focused` / `flushes.unfocused`）・ペイン本体を実際に描いた回数（`body_renders`）が載る = 2 回読んだ差で実際の fps と「フレームが組まれているか」を外から測れる。**ペインごとの出力イベントの配送の記録 `delivery`**（`pane` / `hops` = メインスレッドへ渡った回数 / `forced` = そのうち描き直しの合図ではない必ず渡すイベントが乗っていた回数 / `skipped` = 省いた回数 / `hidden` = 最後の可視判定で画面に出ていなかったか。どれも起動からの累計。#816 / #2344）も載る = 2 回読んだ差で「画面に出ていないペインの出力がメインスレッドを何回起こしているか」を外から測れる | M |
| FR-4.11.4 | A/B は同一バイナリの `TAKO_1979_LEGACY=1`（上限なし = 全ペインが 16ms）。上限以下であることとフォーカス中が変わらないことは `scripts/test-ui-thread-wait-1979.sh` の③（実測: 30 fps で 29.3・10 fps へ変えると 9.8・フォーカス中 59.7〜60.8）。CPU の前後は `scripts/measure-unfocused-redraw-1979.sh`（同じ release の A/B を交互）。**蓋を閉じた機ではフレームが組まれない**（仮想ディスプレイ上の窓の display link が動かず `body_renders` が 0 回/秒）ので、描画の CPU は蓋を開けた機で測る（2026-10-10 の蓋閉じの実測は要求 55〜72 → 29.5 回/秒・CPU は差なし = 描画が含まれていない。2026-10-11 の再測（#1984、`measure-unfocused-redraw-1979.sh 60 2 tui`）も蓋閉じで 4 本とも `body_renders` 0.0 回/秒 = 既定値の見直しは蓋を開けた機の実測待ち）。**2026-10-12 に蓋を開けた機で実測**（#1991、同じ `60 2 tui`・`AppleClamshellState = No`・面は描画可能・他の worker のビルドで負荷平均 17〜34）: 上限なし 14.35% / 16.35%（出力起因の再描画 58.2 / 60.8 回/秒）→ 既定 30 fps 11.15% / 13.85%（29.9 / 29.9 回/秒）= CPU は平均 15.4% → 12.5%（約 19% 減）。**既定は 30 fps のまま**とした: ペイン本体の描画（`body_renders`）は両腕とも 78〜235 回/秒とぶれが大きく上限で減っておらず（出力のまとめ以外の描き直しが本体の描画を駆動している）、上限をさらに下げても削れるのは既に半分になった出力起因の要求だけで、フォーカスの無いエージェントの TUI の見え方を犠牲にする根拠にならない。次に削るのは本体の描画を駆動している側（アプリ全体の描き直しの出どころ） | M |
| FR-4.11.5 | **画面に出ていないペインの描き直しの合図はメインスレッドへ渡さない**（#816 / #2344）。合図は `Wakeup` と、tako では何もしない `CursorBlinkingChange` / `MouseCursorDirty`（分類の正本は `tako_core::terminal::redraw_hint`。網羅の match で alacritty が種類を足すとコンパイルで止まる）。tmux の器越しのペインは描き直しのたびに `\e[?12l` が来るので、`Wakeup` だけを見ていた #2344 前は不可視のペインでも 1 行ごとに 1 往復していた（実測 33.4 → 4.8 回/秒）。見えているペインは Wakeup と同じ間引き（再描画の時刻は変わらない）。`Bell` と問い合わせ（`ColorRequest` / `TextAreaSizeRequest` / `ClipboardLoad`）は合図にしない。A/B は `TAKO_2344_LEGACY=1`（`Wakeup` だけが合図）、ゲートを外す注入は `TAKO_2344_INJECT=nogate`。回帰はセルフテスト項目 113（3 本の判定）と `tako-core` の実 PTY のテストが落とす | M |

### FR-4.12 貼り付けの鍵らしき文字の確認（Issue #2046）

> ユーザーの依頼（2026-10-13）:「テキスト貼り付け時、API キーやパスワードっぽいものを貼られたら
> 警告を出す機能。機械的に判断して、普通の長文は回避して」。確定事項（Issue のコメント）:
> 判定は tako 側（貼り付け経路）・設定でオン / オフ・**既定オン**・**アップデート後は既存の利用者全員に適用**。

| ID | 要件 | 優先度 |
|---|---|---|
| FR-4.12.1 | GUI の貼り付け（⌘V / Ctrl+V / Shift+Insert / メニューの「貼り付け」）が端末ペインへ流れる前に、本文を判定する。関所は `TakoApp::paste_to_terminal` の 1 か所（`paste()` の振り分け → ここ）で、PTY の手前なので claude / codex / agy / 素のシェル / SSH 先のどれへの貼り付けも同じく守る（agent 系統の能力差は無い = MATRIX に行を足さない）。アプリ内の入力欄（パレット・コミット欄等）とコードプレビューの編集は対象外（PTY へ流れない） | M |
| FR-4.12.2 | 当たったら「送る / やめる」の確認ダイアログを出す（閉じる確認 #172 と同じ部品の形・絵文字なし）。**やめると PTY へ 1 バイトも届かない**。送るは貼った時点のペインへ、そのペインのブラケットペーストの要求に従って送る。**Enter と Esc はどちらも「やめる」**（貼り付けの直後に送信のつもりで押した Enter で鍵を送らない）・カードの外を押してもやめる・送るのはボタンだけ。確認中の次の貼り付けは積まない | M |
| FR-4.12.3 | 判定の正本は `tako_core::paste_guard::scan` の 1 実装（GUI・CLI・MCP が同じ関数）。三段: ①既知の形の表 `SHAPES`（前置詞 + 本体の文字集合 + 長さ。Anthropic / OpenAI / GitHub / GitLab / Slack / Discord の Webhook / AWS / Google / Stripe / npm / Hugging Face / SendGrid）+ 秘密鍵の PEM・JWT・URL に埋め込んだパスワード ②鍵らしい名前への代入の値（`password=…` / `"api_key": "…"`。引用符の無い値は設定・コマンドの書き方 = 詰めた代入か行頭の名前だけ・ASCII・数字か大小混在・置き物の語と式を外す）③空白を含まない 20〜512 文字の塊のうち、英大文字・小文字・数字が混ざり、camelCase で分けた 2 文字以下の語が 3 割以上で、エントロピー 3.3 以上のもの。除外表 `EXCLUSIONS`（URL・data: URI・ファイルパス・UUID・16 進・integrity・SSH の公開鍵・公開の PEM・折り返した base64・ハッシュの見出し・base64 の JSON・区切りの多い名前・base64 のファイルの頭）は行を足すだけで増える。regex は足さない（tako-core の依存に無い） | M |
| FR-4.12.4 | 画面と応答に出すのは規則名と伏せ字（前置詞 + 末尾 4 文字まで・前置詞が無ければ先頭 3 文字も・本体の 1/3 未満）だけ。`Finding` は生の値を持たない。**本文・伏せ字を診断ログへ出さない**（persist.log へは `paste_guard::log_line` = 規則名・件数・バイト数の 1 行だけ）。番犬 `crates/tako-control/tests/issue2046_paste_guard_watchdog.rs`（関所の迂回・ログ・`PendingPaste` の `Debug`） | M |
| FR-4.12.5 | 所要: 先頭 64 KB（`MAX_SCAN_BYTES`）だけを見て、採る件数は 100 件（`MAX_FINDINGS`）で打ち切る。最適化ビルドの実測（2026-10-13）: 鍵 1 行 3 µs・普通の 4 KB 44 µs・64 KB のソースコード 0.8 ms・1 MiB 0.5 ms・塊だらけの最悪寄り 0.5 ms（上限を入れる前は代入だらけの 64 KB で 6.6 ms）。打ち切ったことは確認ダイアログに出す | M |
| FR-4.12.6 | 設定は `settings.json` の `paste_guard`（serde の default = true。**項目の無い旧ファイルも ON で読む** = 古い版が書いた設定で off にならない・移行 Step 不要 = #916）。CLI `tako paste-guard [on\|off\|warn\|check\|send\|cancel\|allow\|unallow\|rule]` / MCP `tako_settings` の action=paste_guard（`enabled` / `text` / `respond`、#2050 で `mode` / `allow` / `unallow` / `rules` / `pane`。ツールは増やさない）/ 設定画面「一般」のトグルが同じ dispatch（`Request::PasteGuard`）を通る。次の貼り付けから効く。A/B は同一バイナリの `TAKO_2046_LEGACY=1`（確認なしで貼る）。実 GUI の検証は `scripts/test-paste-guard-2046.sh`（visual-test `paste-guard`。本物のクリップボードに書かず、visual-test だけの差し込み口から `paste()` を通す） | M |

実装メモ（2026-10-13、#2046）: 実データ（このリポジトリのソース・docs・git log を 4 KB ずつ 9378 片）で
当たるのは 7 片で、すべてテストコードの偽の鍵・`token=` を含む URL の断片・PowerShell の base64
コマンドだった。ランダムな英数 32 文字の検出率は 97.6%（1 万本）。すべて小文字の合言葉・16 進だけの鍵・
引用符の無いコードの代入は見逃す側に倒している。mod の送信時の 2 段目（Issue 本文の案）と許可リストは範囲外
（→ #2050 = FR-4.12.7〜）

#### 入力の経路ごとの扱い（Issue #2050 の判断）

判定はどの経路も `tako_core::paste_guard::scan_with` の 1 実装。経路によって変えるのは**当たったときの振る舞い**だけで、
軸は「送り手が人か（確認に答えられるか）」と「tako 自身が組んだ文字か」。

| 経路 | 送り手 | 当たったとき | 理由 |
|---|---|---|---|
| GUI の貼り付け（#2046） | 人 | 段に従う（確認 = ダイアログ / 警告だけ = 送って通知） | 既存 |
| mod の送信時（`prompt.submit`。出どころ `composer` = その端末での Enter・`bridge` = Remote Control） | 人（打ち込み・tako を通らない貼り付け） | 段に従う（確認 = Claude Code の選択ダイアログで「やめる / 送る」・やめたら送信を落とす / 警告だけ = トースト） | tako を通らない入力の最後の関所。claude の中でしか見えない |
| mod の送信時・出どころがそれ以外（peer・plugin・通知・予定のトリガー等） | AI・仕組み | **見ない** | 人の入力ではない。止めると自動化が詰まる |
| スマホ（PWA）の入力（`/api/panes/<id>/input`） | 人 | 段に従う（確認 = **送らずに** 409 と伏せ字 → PWA の確認シート →「送る」で `confirm_secret` を付けて送り直す / 警告だけ = 送って通知） | 人が画面を持っている経路。PC の貼り付けと同じ扱い |
| `tako send` / MCP `tako_send_input` | AI・スクリプト | **止めずに送り**、応答に `paste_guard`（規則名と伏せ字）を付けるだけ（段に関係なく警告） | AI は確認ダイアログに答えられない。拒否すると正当な自動化（トークンを渡すセットアップ等）が壊れる。値は送り手の AI の文脈に既にあり、止めても守れるものが無い。応答に出せば AI と人の目に入る |
| GUI 自身の送信（スラッシュのボタン・ファイルのドロップ）・tako が流すプロンプト（worker の初回・引き継ぎ・送達確認つきの送信） | tako | **見ない** | tako が組んだ文字 |

tako がペインへ流した値（AI の送信・worker の初回プロンプト）と人が確かめて送った値は、そのペインの**確認済み**として
指紋だけをメモリに 10 分覚え、mod の送信時の確認は二度確かめない（GUI で「送る」を選んだ直後の Enter・worker の claude が
初回プロンプトを送るときに止めない）。

| ID | 要件 | 優先度 |
|---|---|---|
| FR-4.12.7 | **段**: settings.json の `paste_guard_policy.mode` = `confirm`（既定・確認）/ `warn`（止めずに警告だけ）。on / off（`paste_guard`）と合わせて「off / 警告だけ / 確認」の 3 段。CLI `tako paste-guard on` = 確認・`warn` = 警告だけ・`off` = 切る。GUI の貼り付けは警告だけの段なら PTY へ流した上で画面の上に小さな通知（規則と伏せ字・数秒で消える・押すと消える）を出す | M |
| FR-4.12.8 | **許可リスト**: `tako paste-guard allow`（標準入力）/ MCP `allow` に渡した文字の当たりを許可リストへ足す。持つのは**指紋（区切りの語つきの SHA-256）と伏せ字と規則名だけ**（settings.json が `tako config` で他の機器へ渡っても値は渡らない）。外すのは `unallow <ID\|all>`（ID = 指紋の先頭 12 文字。一覧は `tako paste-guard` の `allow`）。上限 200 件。許可した値は範囲だけ取って報告しない（同じ文字を後ろの段が拾い直さない）ので、許可した値と秘密が 1 回の貼り付けに混ざっても秘密だけが当たる | M |
| FR-4.12.9 | **規則ごとの無効化**: `tako paste-guard rule <規則名> on\|off` / MCP `rules`（規則名 → 有効か）。名前は `rule_names`（応答）の綴り。無効にした規則の当たりも範囲だけ取って報告しない（無効にした `openai_api_key` の鍵を `high_entropy` が拾い直さない）。知らない名前は断る（綴りの一覧を返す） | M |
| FR-4.12.10 | **mod の送信時の確認（2 段目）**: tako mod の `prompt.submit` フックが、人の出どころ（`composer` / `bridge`）の送信を `tako paste-guard check --pane <TAKO_PANE_ID>` へ**標準入力で**渡して判定させ（argv・ログに本文を残さない）、`verdict` に従う。`check` に `pane` を添えると、そのペインの確認済みの値を外した上の `verdict`（`pass` / `warn` / `confirm`）と `acknowledged`（外した数）を返す。CLI の失敗・時間切れ（3 秒）・mod の例外では**送信を通す**（壊れた mod が送信を止めない）。mod のデバッグログへは終了コードだけ | M |
| FR-4.12.11 | **AI の送信**: `Request::Send` は印（`paste_guard`）が無ければ AI・スクリプトの送信として判定し、当たれば送った上で応答に `paste_guard`（`verdict: warn`・規則名・伏せ字）を足す。スマホの中継は印 `human`（段に従う。確認の段なら `sent: false` と伏せ字で返し、送達の経路へ入らない）/ `confirmed`（人が確かめた = 送る）を付ける。GUI 自身の送信（出どころ `User`）は見ない。届けた値はそのペインの確認済みにする（送達に失敗したら覚えない） | M |
| FR-4.12.12 | 経路ごとの累計は `tako paste-guard` の `routes`（`send` / `remote` / `mod` ごとの `checked` / `warned` / `prompted` / `acknowledged`）と `stats.warned`（GUI の警告だけ）。persist.log へは `paste_guard::log_line` の 1 行（`send:warn` / `remote:confirm` / `remote:confirmed` / `mod:<verdict>` / `warned`）だけ。番犬 `crates/tako-control/tests/issue2050_paste_guard_routes_watchdog.rs`（新しい経路の診断ログ・判定の 1 実装・mod の標準入力）。A/B は同一バイナリの `TAKO_2050_LEGACY=1`（段・許可・無効化を当てず、send / スマホ / mod は判定しない）。実経路テストは `scripts/test-paste-guard-2050.sh` | M |

設定のファイル形式: `paste_guard_policy` 節（`mode` / `allow` / `disabled_rules`）は節ごと `#[serde(default)]` で、
**節の無い旧ファイルも既定（確認・許可なし・全規則）で読める**ので移行 Step は足していない（#916。既定のままなら書き出さない）。

## FR-5 セッション永続性（tmux バックエンド。Phase 5.5 で再設計・実装済み）

> **長寿命のエージェントセッションをアプリの生死から切り離す**。全ペインの PTY を
> tako 専用 tmux サーバー（`tmux -L tako`）のセッションとして保持し、tako を再起動しても
> 実行中プロセス・画面内容ごと復元する（2026-06-12 方式承認・同日実装。
> 旧 FR-5.1 = 構成のみ保存・シェル新規起動 はこの方式の下位互換として吸収）。

| ID | 要件 | 優先度 | 状況 |
|---|---|---|---|
| FR-5.1 | persist 有効時、各ペインのシェルは tmux バックエンドセッション（`tako-<rand>`）として spawn し、アプリ終了・クラッシュでは**セッションを残す**（detach のみ） | M | ✅ 2026-06-12 |
| FR-5.2 | タブ / ペイン構成（分割ツリー・比率・タイトル・role・cwd・**ID**）を `<data_dir>/layout.json` へ保存し、再起動時に**同じ ID** で復元 + 各セッションへ再 attach する。消えていたセッションは保存 cwd で開き直す | M | ✅ 2026-06-12 |
| FR-5.3 | 再起動をまたいで AI からの操作が途切れない: ペイン / タブ ID の復元（`TAKO_PANE_ID` / `TAKO_TAB_ID` が有効なまま）+ CLI / MCP ブリッジの接続情報フォールバック（FR-2.2.9）で吸収する | M | ✅ 2026-06-12 |
| FR-5.4 | tmux 不在環境では **PTY のみ**直接 spawn へ無害に劣化し、タブ / ペイン構成の保存・復元（FR-5.2）は tmux 無しでも機能する（保存時 `session: None`、復元時は保存 cwd で新シェル = 構造のみ復元。Issue #30 で分離）。設定 OFF は永続化全体を停止する（ゼロコンフィグ原則） | M | ✅ 2026-06-12（2026-07-02 改） |
| FR-5.5 | ON/OFF は settings（`tmux_persist`、既定 ON）+ `tako persist` / MCP `tako_persist` から操作可能（開発不変条件。計 19 ツール） | M | ✅ 2026-06-12 |
| FR-5.6 | tmuxview（FR-2.13）で tako 自身のバックエンドセッションを**区別表示**する（`backend: true` + 保持ペイン / orphan 表示。kill 確認文言も専用） | M | ✅ 2026-06-12 |
| FR-5.7 | 保存・復元の結果を**黙殺せず診断可能**にする（Issue #30）: 復元の成否・理由（不在 / 破損 / バージョン不一致等）・layout.json の明示削除を `<data_dir>/persist.log` に記録（256KB で `.old` ローテート）。破損ファイルは番号付きの退避（`layout.json.unreadable[.<n>].bak`。#916 / #1819 の `quarantine_unreadable` と同じ口。前回分を上書きしない・起動時の移行を先に通したかに頼らない・UTF-8 でない中身もバイトのまま残す = #1824。以前の `layout.json.corrupt` への rename は前回分を上書きしていた）へ退避し（文字として読めない中身を移すのは `migration::quarantine_raw` の 1 実装で、recent.json / remote/shortcuts.json の読み手も同じ口を通る。満杯なら 1 本目を残して 2 本目を押し出す = #2231）、理由には読んだ値を載せない（種類と行・列だけ）。`tako persist` / MCP `tako_persist` の応答に `layout_path` / `layout_exists` / `last_restore` / `log_path` を含める | M | ✅ 2026-07-02 |
| FR-5.7.1 | **復元の内訳は全ペインを説明する**（✅ 2026-09-22、#1554）: FR-5.7 の「復元成功: N タブ / M ペイン（…）」は、**カテゴリの合計が M と一致する**（`tmux 再 attach` / `Claude resume`（+ 系統ごとの resume）/ `新規シェル` / `プレビュー` / `Web ビュー` / `たまり場・退避` / `復元失敗`）。#1554 以前は復元ループの 5 経路（**たまり場ペイン（FR-2.15.5）・退避タブ配下のペイン（#1487）・Web ビューペイン・`spawn_session` の失敗・resume 入力の宛先が無いペイン**）がどのカウンタにも入らないまま `continue` していたため、合計が M より小さい行が残り、**差の正体がどの経路かログから区別できなかった**（実測 2026-09-22: 本番 persist.log 52 行中 8 行が不一致で、**すべて #1487 が着地した 9/21 以降**。そのときの `layout.json` は 17 ペイン中 1 件がたまり場・1 件が退避タブ配下で、Web ビューとプレビューは 0 件だった）。さらに本当の失敗（`spawn_session` の `Err`）は `eprintln!` 止まりで、GUI の stderr はどこにも出ない（`log show` で 0 件）ので persist.log に痕跡が無く、AGENTS.md が「再起動後にエージェントが戻らないとき」の正本として案内している診断そのものに穴があった（**無言で壊れる**）。①件数・1 行目・2 行目の正本は `tako_control::restore_report::RestoreBreakdown` の 1 実装で、**区間（ラベルと件数）の列が合計の出どころ**なので、カテゴリを 1 つ落とすと合計も落ちる（「表示だけ増やして数え忘れる」形にならない）。②**「復元しなかった」と「復元できなかった」を分ける**: たまり場・退避タブ配下は「表に出すときに起こす」設計で `spawn_session` を通らないのが正しいので `たまり場・退避` として数え、**失敗にはしない**（毎起動 `復元失敗 2` と出ると本当の失敗が埋もれる）。タブ配下に居ないのにそのどちらでもないペインだけが `復元失敗` になる。**#1576 で前提を訂正**: 表に出す経路は端末を起こさないので、素通りさせた退避は幽霊になっていた。いまは表と同じ経路で器へ繋ぎ直し、`たまり場・退避` は「退避のまま戻した」件数で、2 行目に `戻し方: tmux 再 attach N / resume N / 新規シェル N / プレビュー N` を添える。起こせなかった退避（spawn の失敗・器も手掛かりも無い・同じ器の重複）は `復元失敗` の側で数える（FR-2.15.5）。③**個別の失敗は 1 行ずつ persist.log へ残す**（`復元失敗（ペイン N）: <分類>`）。載せるのはペイン ID と理由の分類、および spawn のエラー文だけで、**ペイン内容・送信テキスト・トークンは出さない**（conventions の診断ログ規約）。④**それでも合計が M と合わなければ、その食い違い自体を 1 行残す**（結末を記録しない経路が将来また増えても無言にならない）。1 行目は `tako persist` / MCP `tako_persist` の `last_restore` にそのまま載る（FR-5.7）。旧挙動の A/B は `TAKO_1554_LEGACY=1`、番犬は `crates/tako-control/tests/issue1554_restore_breakdown_watchdog.rs` | M | ✅ 2026-09-22 |
| FR-5.8 | **多重インスタンスガード（セカンダリモード）**（Issue #113）: 別の生きたインスタンス（別プロセス = control.json の主が生存 tako-app / 同一プロセス = NewWindow の 2 窓目以降）がある間、後発は**セカンダリモード**で起動する — 復元しない・layout.json を書かない/消さない・tmux バックエンドに乗らない・固定ソケット（tako.sock）と control.json の current を乗っ取らない・persist 切替を拒否（`tako persist` 応答の `secondary` で判別可）。ガード無しの旧挙動では、後発の復元 spawn（`new-session -A -D`）がプライマリの全クライアントを強奪 → Exited 連鎖の途中状態が layout.json を上書き → 次回起動の orphan cleanup が実行中セッションを kill する三段連鎖が起きた（2026-07-08 実機: 19→13 ペイン消失）。`TAKO_FORCE_PRIMARY=1` は検証・緊急脱出用オーバーライド | M | ✅ 2026-07-08 |
| FR-5.9 | **PC 再起動後の Claude 会話復旧**（Issue #139 / #652）: persist 有効中は、PID 祖先照合で確定できた Claude Code session ID をペインに紐づけて `layout.json` へ保存する。復元時は保存済み tmux backend が生存していれば従来どおりプロセスごと再 attach し、PC 再起動等で backend が消失している場合だけ、transcript の存在と session ID 形式を検証して新シェルから `claude --resume <session-id>` を実行する。検出不能なペイン・明示終了済み Claude・Codex 等を推測で resume しない。**transcript の探索と resume 実行は claude の config ディレクトリ単位で行う**（#652）: 会話は `<config dir>/projects/` に置かれるため、既定 `~/.claude` に加えて accounts.yaml（#504）の `config_dir` と tako 自身のプロセス env の `CLAUDE_CONFIG_DIR` も走査し、見つかった所在に応じて `export CLAUDE_CONFIG_DIR=<所在>; ` / `unset CLAUDE_CONFIG_DIR; ` を resume コマンドへ前置する（rc / direnv / tako 側 env の値に勝つ。#500 / #512 と同型）。既定だけを見ていた旧実装では、アカウント運用の master ペインが「会話なし」と判定され毎回新規シェルに落ちていた。新規設定は追加せず、既存 `tako persist` / MCP `tako_persist` が 1:1 の制御点 | M | ✅ 2026-07-13（#652 対応 2026-07-30） |
| FR-5.9.1 | **保存済み session ID は「確認してから外す」**（✅ 2026-09-09、#1076）: FR-5.9 の ID は `claude agents --json` の定期スキャン（30 秒間隔・起動 5 秒後に 1 回目）で更新するが、**検出は「いま動いているか」しか答えないので不在は「終了した」の証拠にならない**（起動途中・スキャンの取りこぼし・承諾ダイアログ待ちの claude はどれも出ない）。旧実装は検出結果でマップを**丸ごと置き換え**、前段ガード（子プロセス不在）では**全消し**していたため、**再起動直後の数秒**（resume した claude が Node 起動 + 巨大 transcript 読み込みの途中）に `layout.json` の `claude_session_id` を全部捨てていた（実測 2026-09-09: 復元直後 12 件 → 2 秒後 0 件）。一度捨てると次の再起動は「タブのタイトルだけ残って claude が居ない」になる = ユーザー報告の症状。保持規則の正本は `tako_core::claude_resume::ResumeIds` で、**復元由来の未確認 ID はスキャンの不在では落とさず**、スキャンで一度でも確認できたペインだけが連続 2 回の不在で落ちる（「ユーザーが claude を終了したペインを次回起動で勝手に resume しない」という FR-5.9 の意図は保つ）。閉じたペインは即座に落ちる。倒す向きは非対称なコストで決めてある（残しすぎ = 閉じた会話が 1 度復活する / 捨てすぎ = **会話への唯一の入口を失う**）。旧挙動は `TAKO_1076_LEGACY=1`。番犬は `crates/tako-control/tests/claude_resume_watchdog.rs` | M | ✅ 2026-09-09 |
| FR-5.9.2 | **復元も resume コマンドを 1 実装で組む**（✅ 2026-09-09、#1076）: 復元経路は独自の最小形（`<config dir 前置>claude --resume <id>`）を組んでいて、FR-2.38.3 が `session_restart` で使う `sessions::resume_command` と**2 系統に分かれていた**。最小形は役割 env（`TAKO_ORCHESTRATOR_ROLE`）・`--model`・`--effort` を落とすので、再起動後に戻ってきた claude は **master / worker として認識されない**（オーケストレーターからも MCP からも見えず、モデルも既定へ落ちる）。判断とコマンドの形は `tako_control::sessions::restore_plan`（`resume_command` と同じ組み立てを共有）へ寄せ、セッションカタログ（FR-5.12）の記録から起動条件ごと復元する。カタログに記録が無いペインだけ最小形へ落ちる（役割 env の載らなかった件数は `役割なし` として数える。手動起動の claude はここに出るのが正常）。**#2473: worker は FR-2.38.3 と同じ 1 実装（`resume_launch::worker_resume_launch` = spawn の `build_worker_cmd_in`）で組む**。カタログのメタだけでは `--permission-mode auto`（プロファイルの args）・`--remote-control`・プロファイルの env が落ち、`--model` は実モデルへ化けていた。起動した master のプロファイルは復元した layout の role / spawned_by から、model / effort は worker レジストリの spawn の記録から引く（`resume_launch::restore_hints`）。組めないときはカタログのメタの形で戻し、理由を persist.log の `[restore] pane=N worker …` へ。A/B は `TAKO_2473_LEGACY=1`。**新規シェルへ落ちた理由**（ID なし / 形式不正 / 会話が見つからない）と役割つき resume の件数は `persist.log` の「復元の内訳」1 行に出す（FR-5.7 の診断可能性を復元経路へ広げる。「新規シェル N」だけでは同じ報告が何度も上がっても切り分けられなかった） | M | ✅ 2026-09-09 |
| FR-5.9.3 | **claude 以外も会話ごと復元する**（✅ 2026-09-09、#1238）: FR-5.9.2 まで、再起動後に会話ごと戻せるのは claude だけだった（`layout.json` が持つのは `claude_session_id` のみ）。codex / agy のペインは「タブのタイトルだけ残って新規シェル」になり、内訳では `ID なし` として claude の検出漏れと混ざっていた。**生きたプロセスが開いているロック**から会話 ID を採り（codex = `$CODEX_HOME/thread-writer-locks/<id>.lock` / agy = `~/.gemini/antigravity-cli/presence/<id>.lock`。どちらも `lsof` で pid から引く）、`layout.json` の `agent_resume`（系統 + 会話 ID）へ `claude_session_id` と対称に保存する。復元は同じ `restore_plan` の分岐で `codex resume <id>` / `agy --conversation <id>` を投入する（書式の正本は `tako_core::agent_resume::resume_spec`。`codex resume` は `--model` を受け取らないので起動条件は claude だけに載せる）。役割 env は spawn 記録（pending）から復元する（codex / agy はカタログ本体へ昇格しないため。FR-5.12）。**「保存されていない」と「この環境には手段が無い」を分ける**: `lsof` の無い Windows は ID を採れないので、内訳の理由を `ID なし` ではなく `resume 非対応` として出す（判定の正本は `tako_core::agent_resume::restore_support`、宣言は能力マトリクスの `restore_after_reboot`）。旧挙動の A/B は `TAKO_1238_LEGACY=1` | M | ✅ 2026-09-09 |
| FR-5.10 | **復元強奪ガード**（Issue #177）: 復元 attach の前に layout.json 記載セッションの tmux クライアント（`client_pid`）を走査し、**生きた別 tako-app 配下のクライアント**が attach 中ならセカンダリモードへ降格する。FR-5.8 の discovery 判定が `TAKO_DISCOVERY_DIR` の隔離・control.json 消失で盲目になる穴を、判定材料を「守るべき資源そのもの」へ置くことで塞ぐ（2026-07-13 実機: discovery だけ隔離した dev 起動が本番 13 セッションを強奪 → 本番 GUI の PTY 一斉死亡）。実験・検証起動には `TAKO_ISOLATED=1`（discovery / persist / tmux socket の一括隔離）を使うこと | M | ✅ 2026-07-13 |
| FR-5.11 | **喪失前バックアップと復旧**（Issue #177 / #770）: **何かを失う** layout.json 保存は、上書き前に直前内容を `.bak.1`〜`.bak.3` へ世代退避する。対象は ①ペイン数が「直前 4 以上 → 半分未満」に減る縮退保存（#177 の PTY 一斉死亡）②**バックエンドセッションを持つペインが消える保存**（#770 のタブ close。close は 1 ペインずつ届くため 12→11→10 のどの段階も半減にならず、①だけでは実機で 16 日間 1 度も世代が残らなかった）。連鎖する喪失で健全世代が押し出されないよう bak.1 が 10 分以内は回転しない。たまり場への退避は「失った」に含めない（セッションは生きている）。復旧は CLI `tako recover`（一覧）/ `tako recover --apply <世代>`（稼働中 tako 検出で拒否、`--force` で明示上書き）。**MCP 非公開の例外**: 本コマンドは GUI 死亡・縮退保存後にアプリ外から使う復旧手段であり、GUI 内蔵 MCP サーバー経由では原理的に提供できない（GUI が正常稼働している間は復旧不要） | M | ✅ 2026-07-13（#770 で条件拡張 2026-08-06） |
| FR-5.15 | **セッション破棄の監査記録**（Issue #770）: tmux バックエンドセッションの kill と、明示 close によるタブ close は、発生源（`close:gui-tab` / `close:kbd` / `close:dispatch(mcp, caller=…)` 等）つきで `persist.log` に残す。復元・quit と同じ 1 ファイルに並ぶので「再起動が消したのか、明示 close が消したのか」を事後に切り分けられる（実機 #770 では痕跡がペインログのクローズマーカーだけで、これは `pane_logs` 設定で OFF にできるため調査が原理的に不可能になりうる状態だった）。記録するのはペイン ID・タブ ID・セッション名・発生源だけで、**ペイン内容・cwd・トークンは出さない**（conventions の診断ログ規約）。セカンダリモードでは書かない | M | ✅ 2026-08-06 |
| FR-5.12 | **セッションカタログ**（Issue #112 A）: 会話本文は保存せず（claude が `<config dir>/projects/` に永続化済み・二重保存回避）、session_id への参照 + メタデータ（ラベル・ロール master/worker/solo/pane・プロファイル・agent 種別・プロンプト由来の Issue 番号・プロジェクト・タブ/ペイン・cwd・開始/最終時刻・ペインログとの突き合わせ）を `<data_dir>/sessions.yaml`（config_io 保護。`TAKO_SESSIONS_FILE` で隔離可）へ索引化する。spawn 時は tmux セッション名キーの pending 記録 → GUI の定期スキャン（`claude agents --json` × pid 祖先照合）が session_id 検出時に昇格。`tako sessions list/show/resume` + MCP `tako_sessions` で 1:1。resume は記録 cwd で新ペインを分割し `claude --resume <id>` を注入（ペイン/タブ/tmux 全滅 + アプリ再起動後も会話文脈ごと復元できることを e2e 実証済み）。所在の config ディレクトリの解決と env 前置は FR-5.9 と共通（#652。`resumable` 判定も全 config ディレクトリを走査する）。**制限**: `tako sessions resume` の一覧に出るのは claude のみ（codex / agy は spawn 時の pending 記録にしか残らず、カタログ本体へ昇格しない = session_id の検出経路が claude 専用）。**再起動後の復元は 3 系統とも会話ごと戻る**（#1238。ID の出どころは生きたプロセスが開いているロックで、カタログとは別経路。FR-5.9.3） | M | ✅ 2026-07-14 |
| FR-5.12.1 | **走査ごとの `last_seen_at` だけの更新は書かずに溜める**（Issue #2020）: GUI の走査（30 秒ごと）は検出した会話の `last_seen_at` を進めるだけで、修正前は毎回カタログ全体（上限 500 件・約 395 KB）を YAML 化し、`.bak` 3 世代を回して書き直していた（本番で 1 回約 0.8 MB × 2,880 回/日 = 約 2.3 GB/日）。いまは `sessions::LastSeenThrottle` が `last_seen_at` だけの変化を溜め、**① `last_seen_at` 以外の変化**（新しい会話・pane / tab / 器 / cwd / model / ペインログ・pending・GC）は即時に書いて `.bak` を回す、**② 検出した会話の集合の変化**（始まった / 終わった / 戻ってきた。会話がすべて終わった走査を含む）は即時に書く、**③ 最後に書いてから 600 秒**で書く、**④ GUI の終了**（`on_app_quit`。SIGTERM も通る = FR-5.17）で書く。②〜④は溜めた各会話の「最後に見えた走査の時刻」をそのまま書くので、**書いた時点のファイルは修正前と同じ内容**になり、並び順（`tako sessions list`）・GC（古い順に落とす）・ペインからの逆引き（#466）は書いていない間も修正前と同じ。`last_seen_at` だけの書き込みは `.bak` を回さない（会話も項目も消えないので退避の意味が無い。#169 の世代は①の変化のためのもの）。中身が同じなら書かない。保存形式は変えない。毎回ロックを取って読み直すので他プロセスの書き込み（spawn の pending 記録・手直し）を潰さない。失うのは**異常終了（kill -9・電源断）のときの `last_seen_at` の値の鮮度だけ**（最大 600 秒。並び順は保たれる）。A/B は `TAKO_2020_LEGACY=1`、実測は `bash scripts/test-sessions-write-2020.sh` | M | ✅ 2026-10-13 |
| FR-5.13 | **ペインの平文ターミナルログ**（Issue #112 B）: 全ペインのスクロールバック**確定行**（直接ペイン = alacritty history 増分、tmux バックエンド = `#{history_size}` 増分 + `capture-pane -p`）を平文でローテーション保存し、ペイン / タブ / アプリが死んでも `tako logs` / MCP `tako_logs` で遡れる。alt screen（TUI）中は history が増えないため claude 等の描画スパムは構造的に除外（「TUI 実行中」マーカーのみ）。close / exit 時は可視画面をフラッシュ。**器のペインの取り込みは間引く**（#2018）: tmux を起こすのは出力の合図（ペインの PTY に届いた Wakeup）が動いた器だけで、出力が続く間は 10 秒ごと・止まったら次の tick・合図が無ければ 30 秒ごとの保険（`pane_log::BackendPace`）。probe と出力のあった器の capture は 1 回の tmux（`;` のコマンド列。psmux は別々）で読み、別々に読む経路は取り込み済みの末尾で probe 後のずれを揃える。器を kill する閉じ方は kill の直前に「履歴の増分 + 画面」を1 回で読んでから書く。**器が自ら消える閉じ方（中のコマンドの終了。`tako split -- <cmd>` の成功 = FR-2.5.14 を含む）**は、器の `remain-on-exit on` + `pane-died` フックが消える前に「全履歴 + 画面」を `<data_dir>/pane-died/<セッション>.<history_size>.<history_limit>.<history_bytes>.<alternate_on>.dump` へ書き出してから `kill-pane` し、tako は attach クライアントの終了（`CloseOrigin::ProcessExit`）で閉じるときにそれを読んで最後の増分と死んだ瞬間の画面を書く（#2085。書くのは tmux サーバー自身 = 子プロセスは増えない。フックの形・失敗しても `kill-pane` が必ず走る理由は `tmux_backend::pane_died_conf`）。書き出しは読んだら消し、読まれずに 10 分を過ぎたものは `sync_conf` が、同じ名前の器を作り直すときは `wrap_options` が消す。psmux は未実測なので立てない（`BackendCapabilities::keeps_exit_output` = false。最後の取り込みの後の行・最大 10 秒は入らない = `tako platform` の `tako_logs` に縮退として申告）。A/B は `TAKO_2018_LEGACY=1` / `TAKO_2085_LEGACY=1`、実経路は `scripts/test-pane-log-pace-2018.sh`。サイズ管理: ペインあたり上限（既定 5MB・`.1` 世代ローテーション）+ 全体上限（既定 200MB・古い順削除）+ 1 tick（2 秒）あたり 400 行上限（超過は省略マーカー。器は間引いたぶん経過 tick 数を掛けた行数まで読む）。settings.json（`pane_logs` / `pane_log_max_mb` / `pane_log_total_max_mb`）で ON/OFF・上限調整可（既定 ON）。**プライバシー**: ログにはトークン等が写り込みうるため `<data_dir>/pane-logs/`（ユーザーローカル）限定・リポジトリ / 同期対象に置かない。「ペイン内容をログに出さない」規約（conventions）は tako 自身の診断ログに対するもので、本機能はユーザーが管理するローカルデータとして明示の例外とする | M | ✅ 2026-07-14 |
| FR-5.17 | **`SIGTERM` でも終了処理を通る**（Issue #777 / #770。✅ 2026-09-09）: `kill -TERM` は既定でプロセスを即死させるため、`on_app_quit`（layout 保存 = FR-5.2 / ペインログ flush = FR-5.13 / 蓋閉じ防止の解除 / discovery の後片付け）を**まるごと飛ばして**いた。保存は 2 秒ポーリング + 操作ごとなので、`kill -TERM`・スクリプトからの停止・一部の OS シャットダウン経路では**直前 2 秒ぶんの分割やタブが復元されない**（tmux セッションは生きるので喪失ゼロ保証 = #30 は破れず、「構成だけ古い」になる）。`SIGTERM` は quit 要求フラグへ読み替え、UI 側の **500ms ポーリング**が Cmd+Q と同じ `cx.quit()` を撃つ（2 秒 tick ではなく既存の 500ms ループで見るのは、新しいタイマーを増やさずに最悪待ちを 1/4 にできるため。追加コストはアトミック 1 回）。**シグナルを握るなら「猶予を過ぎたら必ず死ぬ」ことと対で入れる**: 専用スレッドが `SIGTERM` 到着から `quit_signal::DEFAULT_GRACE`（5 秒）で `std::process::exit(143)` する。猶予は**到着から**測るので、メインスレッドが固まっていて quit を撃てないとき（= まさに `pkill` したくなる状況）も同じ上限で終わる。`SIGTERM` 連打では終了処理を二度走らせない（`take_quit_request` はプロセスを通じて 1 度だけ true）。読み替えたこと・強制終了したことは `persist.log` に 1 行ずつ残す（FR-5.7）。**Windows は対象外**（`SIGTERM` が無い。ウィンドウを閉じる経路は `WM_CLOSE` として GPUI の通常終了へ入り、`taskkill /F` はプロセス側にフックできない）。旧挙動は `TAKO_777_LEGACY=1`、ハングの再現は `TAKO_777_INJECT=hang-main\|hang-quit`、猶予の差し替えは `TAKO_777_GRACE_MS`。番犬は `crates/tako-control/tests/quit_signal_watchdog.rs` | M | ✅ 2026-09-09 |
| FR-5.18 | **attach クライアントだけが終わってもペインを閉じない**（Issue #1857）: tmux バックエンドのペインで attach クライアントが外から終わった（SIGTERM / SIGHUP / SIGKILL）とき、器（`list-clients -t =名`。上限 3 秒・background）へ生死を聞き、セッションが生きていて他のクライアントが attach していなければ**同じペイン ID のまま** attach し直す（直近 60 秒に 5 回まで・待ちは 250ms から倍々で 4 秒まで）。上限を超えたら閉じずに「再接続できない」をヘッダのチップ（クリックで再試行）と画面の 2 行で出す。セッション / サーバーが終わっていれば従来どおり閉じる（セッションは kill しない = 上の close 整合）。別のクライアントが attach 中（多重起動の `-D` の収束）も閉じる。生死を確かめられなかった回は「無い」と読まない（#1597）。再 attach は attach 専用（`attach-session`）で、終わっていたセッションに新しいシェルを生やさない。起きたことは persist.log に 1 行（ペイン ID・セッション名・終了ステータス・判断だけ。**ペインの内容・送信テキストは書かない**。セッションごと終わった普通の終わり方は書かない）。状態は `tako list` / `tako read` の `backend_reattach`、手動は `tako persist reattach --pane N` / MCP `tako_persist` の `reattach`（ツールは増やさない。開発不変条件）。サーバーごと落ちた場合（#1197）・psmux（Windows。未実測なので経路を変えない）は対象外。A/B は `TAKO_1857_LEGACY=1` | M | ✅ 2026-09-30 |
| FR-5.19 | **`tako recover --apply` はバックアップより後のタブを落とさない**（Issue #1858）: 世代のファイルを layout.json へ写すだけだった `recover --apply` は、バックアップより後に作られたタブを消していた（2026-09-30 の本番の復旧では、復旧をしている master 自身のタブも落ちかけ、手で layout.json に足した）。器は生きているので起動時の orphan 自動復帰（#191）が「復帰」タブへ拾うが、**別のペイン ID・別のタブ**になり、タブ名・分割・役割・会話の記録は戻らない。いまは現在の layout.json にあって世代に無いタブ（ID が世代の表のタブ・退避タブのどちらにも無い）を世代の末尾へ合流させる。①**落とすのは、どのペインも FR-5.20 の刈り込む側（器が消えたと確かめられた master / worker）のタブだけ**（確かめ方は FR-5.20 と同じ 2 条件。言い切れないとき・利用者のペインは合流させる = 落とさない側へ倒す。合流したタブの中の刈り込む側のペインは起動時に FR-5.20 が外す）②世代に同じペイン ID・同じ器があるペインは外す（世代の後で別のタブへ移したペイン。2 か所に置くと復元が `DuplicateId` で丸ごと失敗する）③複数ウィンドウなら元のウィンドウが世代にもあればそこへ戻す ④合流が無ければ従来どおり世代とバイト一致のコピー。合流・見送りの結果は `recover` の出力（タブ ID・タブ名・ペイン数・器の有無）と persist.log の 1 行（タブ ID と件数だけ。タブ名は載せない）に出る。退避（たまり場・退避タブ）の差分は扱わない（器が生きていれば orphan 自動復帰が拾う）。CLI 専用なのは FR-5.11 の例外のまま（GUI を止めてから使う操作）。判断の正本は `tako_control::layout_reconcile::merge_after_backup`、A/B は `TAKO_1858_LEGACY=1`（世代をそのまま写す） | M | ✅ 2026-10-16 |
| FR-5.20 | **復元時に tmux セッションが無い master / worker のペインを刈り込む**（Issue #1858）: 器が無いペインは復元で新しいシェル / resume として開き直されて木に残っていた（2026-09-30 の復旧で完了済みの worker 3 枚を手で外した）。**刈り込むのは master / worker の役割を持つペインだけ**: 器が無いペインには「master が片付けたペイン（完了した worker・引き継ぎで閉じた前任の master。開き直すと役割つきの会話が resume されて幽霊になる）」と「利用者が閉じすぎたペイン（FR-5.11 の `recover` で戻す用途。閉じたときに終わったプロセスは同じ作業フォルダの新しいシェルで開き直すと docs の SceneRecover が約束している）」があり、どちらも閉じたときに tako が器を kill しているので**器の状態では区別できず、違いは役割にしか現れない**。役割なし・solo のペインは従来どおり開き直し、persist.log に `tmux セッションが無いが外さずに開き直す: ペイン …` を 1 行残す。**消すのは器が消えたと確かめられたペインだけ**で、確かめられるのは ① tmux がセッション一覧を返した（`tako_core::tmux::live_session_names` が `Some`。サーバー未起動・ソケットが無い・tmux 不在・上限切れは不明）② その一覧に layout の器が 1 つ以上居る（= layout を書いたのと同じサーバー）の 2 つが揃うときだけ。一覧は **spawn より前に 1 回だけ**取る（最初の spawn がサーバーを起こした後に聞くと、PC の再起動で器ごと消えた全ペインを「消えた」と読む）。PC の再起動後の resume / 新規シェル（FR-5.9）は ① を欠くので従来どおり。対象は表のタブの葉だけで、プレビュー・Web ビュー・器を持たない直接のペインと退避（FR-2.15.5 / #1576 の正本に任せる）は触らない。葉がすべて外れたタブは閉じ、**表のタブがすべて空になる刈り込みはしない**（タブ 0 枚の layout は復元ごと失敗し `.corrupt` へ退避される）。刈り込んだペインは persist.log に 1 枚 1 行（`刈り込み（ペイン N・タブ T）: tmux セッション <名> が無い（役割 …・会話の記録 claude = tako sessions list から戻せる）`。会話 ID・タブ名は載せない）、件数は FR-5.7.1 の 1 行目の括弧の外（`・刈り込み N（tmux セッションが無い。空になったタブ K 枚を閉じた）`。復元していないので M の内訳には入れない）と 2 行目「復元の内訳」の末尾に出る。見送ったときは理由を 1 行（`刈り込みを見送った: …`）。`tako persist` / MCP `tako_persist` の `restore_pruned` に 1 枚ずつ（pane / tab / session / role / conversation）。判断の正本は `tako_control::layout_reconcile::prune_dead_backends`、A/B は `TAKO_1858_LEGACY=1`（器の無いペインも開き直す） | M | ✅ 2026-10-16 |
| FR-5.21 | **復元の直後に全ペインが PTY 死亡しても終了しない**（Issue #641）: 起動時の復元・orphan 自動復帰で器（tmux / psmux）へ繋いだペインが、繋いでから **30 秒以内**（`restore_guard::WINDOW`）に**入力の無いまま**すべて PTY 死亡で閉じたら、最後のタブで終了せず**救済**する。修正前は「全ペイン終了（PTY 死亡）」として layout.json を残して終了したので、次の起動も同じ復元で即終了するループになり GUI が立ち上がらなかった（Windows 実機 2026-07-28: 強制終了した旧クライアントの残骸が残った psmux へ attach すると即 EOF。器の中の claude は生きていた）。救済 = ① 実際に復元へ使った layout を `layout.json.rescued` へ退避（前の退避は `.bak.1`〜`.bak.3` へ回る。`tako recover --apply rescued` で戻せる）② **器を使わない**新しいシェルのタブ（日英のタブ名「復元できず」）を立てる（同じ器の不調に巻き込まれない。以後の分割は従来どおり器に乗る）③ 死んだタブは PTY 死亡由来の close で外す（**器は kill しない** = 中のプロセスは次の起動の orphan 自動復帰が拾う）④ persist.log に 1 行（`復元の直後の全滅（#641）: 器へ繋いだ直後に入力の無いまま器のペインがすべて PTY 死亡で閉じた（器へ繋いでから N 秒）。…（捨てた器 K: tako-…・器は kill しない）。…へ退避…。同じ理由で連続 C 回目…`）⑤ 連続回数を `<data_dir>/restore-guard.json` へ。**同じ理由の救済が 2 回続いたら（`SKIP_AFTER`）次の起動は layout の復元を見送る**（layout.json は消さずに `layout.json.rescued` へ移す・記録を消す = 見送りは 1 回だけ・orphan 自動復帰はそのまま走る）。記録は窓を救済なしで過ぎた起動・器へ繋がなかった起動・救済せずに終わった起動で消す。**普通の終了は変えない**: 入力（人のキー・貼り付け・明示の close・CLI / MCP の `Send` / `PaneClick` / `OrchestratorRespond` と、器を dispatch が片付ける `Close` / `WindowClose` / `TmuxKill`）の後の全滅（`exit` で最後のシェルを閉じた等）・窓を過ぎた全滅・最後のタブに器のペインが無い・この起動で一度救済した後の全滅は従来どおり終了し、「全ペイン終了」の行へ理由を添える。セカンダリ・セルフテストでは働かない。状態は `tako persist` / MCP `tako_persist` の `restore_guard`（窓・救済の行・記録・退避先）と `tako recover` の一覧。判断の正本は `tako_control::restore_guard`、A/B は `TAKO_641_LEGACY=1`（ガード・見送り・記録をすべて止める） | M | ✅ 2026-10-28（macOS の隔離 GUI で A/B。Windows 実機は未確認） |
| FR-5.22 | **復元で 2 つ以上の表のペインが同じ器の名前を持っても、ペインを黙って閉じない**（Issue #2486）: `layout.json` で表のペインどうしが同じ器（tmux セッション）の名前を持つと、修正前は両方を同じ器へ繋いだ。器のクライアントは `new-session -A -D` なので 2 本目が 1 本目を切り離し、1 本目は #1857 の判定（別のクライアントが attach 中）でペインごと閉じた（3 本なら 2 本。persist.log には #1857 の 1 行だけで、多重起動の後発に切り離されたのと区別できなかった）。いまは**復元の順で最初のペインが器を持ち**、2 本目以降はその名前を使わずに新しい器（`backend::wrap_spawn_for_pane` が払い出す = #885 の入口・#428 の名前の検査も通る）で保存 cwd の新しいシェルを起こし、persist.log へ `器の名前の重複: pane=N session=S はペイン H が先に持つので、このペインは新しい器の新しいシェルで起こす（ペインは閉じない）` を 1 行残す。**会話は器が生きているとき（中で動いているのがどちらの会話か分からない）と先のペインと同じ会話のときは resume せず**、会話の記録もそのペインへ引き継がない（内訳の新規シェルの理由は `器の名前の重複`。会話そのものはカタログに残る = `tako sessions list` から戻せる）。2 本目が前に映していた画面は器が 1 つしか無いので戻らない（器が生きていれば先のペインに映る）。次の保存で 2 本目の新しい器の名前が書かれ、以後の再起動は普通の再 attach。退避（たまり場・退避タブ）の重複は FR-2.15.5（#1576）が外す。判断の正本は `tako_control::restore_vessels`、番犬は `crates/tako-control/tests/issue2486_duplicate_vessel_watchdog.rs`、実経路は `bash scripts/test-duplicate-vessel-2486.sh`。A/B は `TAKO_2486_LEGACY=1` | M | ✅ 2026-10-29（macOS の隔離 GUI で A/B。Windows（psmux）は未確認） |

### FR-5.14 AI 系設定の git ベース共有（Issue #513。2026-07-28 実装）

複数デバイス（mac ⇔ Windows）で「AI の記憶」を揃えるための任意機能。
`tako config` / MCP `tako_config_share` で、git リポジトリ 1 本を介して同期する。

| ID | 要件 | 優先度 | 状況 |
|---|---|---|---|
| FR-5.14.1 | **共有対象はホワイトリスト**。分類カタログ（`config_share::catalog::CATALOG`）が正本で、`shared` / `local` / `secret` を日英の理由つきで宣言する。**カタログに無いファイルは共有しない**（fail-closed）。tako がデータディレクトリ配下へ書くパスの分類漏れは `tests/config_share_catalog.rs` が検出する（#515 のサポートマトリクスと同じ「忘れたらテストが落ちる」構造） | M | ✅ 2026-07-28 |
| FR-5.14.2 | **秘匿情報を構造的に分離**。token / control.json / relay_secret / `~/.claude/.claude.json` / credentials は `secret` 分類で共有しない。さらに書き出し直前に内容を走査し、資格情報らしき文字列を見つけたら push を**止める**（多層防御。値はエラー文にも出さない） | M | ✅ 2026-07-28 |
| FR-5.14.3 | **マシンローカル状態を分離**。layout.json / sessions.yaml / workers.yaml / ペインログ / handoff / 各種ログは `local` 分類。ファイル単位で切れないものは**フィールド単位**で切る（`accounts.*.config_dir` = 資格情報の在り処、profile の `env` = API キーが入りうる、`welcome_dismissed` 等の UI 操作履歴、`setup.completed_at` 等の実施記録）。取り込み時はローカル値を残し、埋める必要がある値を `needs_local` として報告する（`inherit: true` のアカウントのように「無くて当然」のものは免除） | M | ✅ 2026-07-28 |
| FR-5.14.4 | **パス可搬性**: 設定内の絶対パスは、ホーム配下ならリポジトリ上で `~/…`（区切りは `/`）に正規化し、取り込み時にそのデバイスのホーム + そのプラットフォームの区切りへ戻す。markdown は `~` のままにする（人と AI が読む表記であり、展開すると実パスが本文へ混ざるため）。ホーム配下でない絶対パスは可搬化できないので `status` が警告する。判定は区切り文字を引数で受ける純粋関数で、**macOS 上から Windows 側の変換を検証できる** | M | ✅ 2026-07-28 |
| FR-5.14.5 | **配線と同期**: `tako config init`（新規作成 + 初回書き出し）/ `link <パス\|URL>`（既存へ接続・URL は clone）/ `unlink` / `push`（変換 → 検査 → commit → push）/ `pull`（`--ff-only` + 世代バックアップつき取り込み）/ `status`（差分）/ `list`（分類表）。実体は**コピー同期**であって symlink ではない（`config_io::atomic_write` の rename が symlink を実ファイルに置き換えて配線を壊すため。Windows の symlink 権限問題も回避） | M | ✅ 2026-07-28 |
| FR-5.14.6 | **リポジトリ側を信用しない**: pull は分類が `shared` のパスだけを実体へ書く（リポジトリに `tako/token` を置いても実体は書き換わらない）。利用者が手で置いたファイルは削除も取り込みもせず「管理外」として報告する。削除の伝播は tako が書いた `manifest.yaml` の記録に限る | M | ✅ 2026-07-28 |
| FR-5.14.7 | **setup ではオプション**（#262 の質問ゼロ原則を壊さない）: 標準 `tako setup` は表示だけで質問を増やさない（表示内容は FR-5.14.9 で状態つきに拡張）。`--review` の対話でだけ y/N を 1 回聞き、`--answers` / MCP `tako_setup` の `config_share` で非対話配線もできる | M | ✅ 2026-07-28 |
| FR-5.14.8 | Windows 実機での配線確認（#467 と合流）。実装はプラットフォーム共通で、変換の Windows 表記は macOS 上の単体テストで検証済み | S | ⏳ #513 継続 |
| FR-5.14.9 | **setup が状況を検出して案内する**（Issue #793。「導線が無いと使われない」の是正）: `config_share::env::detect` が ①配線済みか（`config-share.json`）②共有対象が既に外部 git（dotfiles 等）で管理されていないか ③gh CLI の認証状態、を**読み取りだけ**で調べる。検出は `~/.claude` / データディレクトリの本体を先に見て、管理外なら symlink の子だけを追う（git 起動を増やさない）。案内の種類は純粋関数 `guidance`（`linked` / `broken` / `adopt_existing` / `fresh`）で決め、setup サマリと `--check` が同じ判定から文言を作る | M | ✅ 2026-08-10 |
| FR-5.14.10 | **質問は増やさない**（#262）: 標準 setup での扱いは表示のみ。**配線済みなら勧誘文言を出さない**（冪等）。`--yes` / 非 TTY では外部への副作用（リポジトリ作成・push）を伴う動作も、代行の案内も出さない | M | ✅ 2026-08-10 |
| FR-5.14.11 | **代行は対話アシスタント側**（Issue #793）: 検出結果を `setup-context.yaml` の `config_share`（`guidance` / `next_command` / `gh_can_create_repo` / `external[]`）へ渡し、setup agent が案内と配線を代行する。**既存の外部 git 運用があれば相乗り**（`tako config link <既存リポジトリ>`）が第一案。別リポジトリを作ると同じファイルが 2 か所で管理され、pull の書き込み（`atomic_write` の rename）が symlink を実ファイルへ置き換えて既存の配線を壊すため。リポジトリ作成・push は**合意を得てから**行い、共有リポジトリは private を既定とする。既存ユーザーへは `changes.yaml` rev 14（guided）で届く | M | ✅ 2026-08-10 |
| FR-5.14.12 | **名前が実行時に決まるファイルも分類する**（Issue #792）: カタログの `path` は末尾 `*` で**ファイル名の前方一致**を表せる（同じディレクトリの中だけに効く）。`orchestrator/_system_prompt_*`（master / solo 起動ごとに書き出す system prompt の実体）は `local` + 理由 GENERATED。被覆テストの走査は `join("…")` に加えて `join(format!("…"))` も拾い、可変部分を `*` に畳んでカタログと突き合わせる（`join(format!(…))` を素通りさせていたため、動的名のファイルは「未分類 = fail-closed で安全だが気づけない」状態だった） | M | ✅ 2026-08-14 |

### close 操作とバックエンドセッションの整合（2026-06-12 仕様化）

- **ペインの明示 close**（× ボタン / cmd+W / CLI・MCP の close）= 対応するバックエンド
  セッションも kill し、孤児を残さない（kill 対象は tako が `backend_sessions` に登録した
  tako 管理セッションのみ）
- **アプリ終了・クラッシュ** = detach のみ（セッションは残る。これが再起動復元の根拠）
- **PTY 死亡由来の close**（シェル exit / tmux クライアント死・サーバー異常）= セッションを
  **kill せず**登録だけ外す（2026-07-03 追加。シェル exit なら既に消えており、クライアント
  kick / サーバー死ならセッションは他インスタンス管理下で生存中の可能性がある）。全ペインが
  PTY 死亡で尽きた場合（例: バックエンド tmux サーバーが外部から kill された）も
  layout.json は**保持**して終了し、次回起動で構成を復元する。ファイルを消すのは
  **明示 close の LastTab だけ**。残骸セッションは起動時 orphan クリーンアップ
  （FR-2.16.11）と tmux ビューの「kill漏れ?」表示が拾う
- **外部 tmux セッションに attach しているだけのペイン**（ユーザーが `tmux attach` した等）は
  close しても tako は何も kill しない（クライアントが切れる = detach に留まる。
  外部セッションは tako 管理ではないため）
- 「ターミナルのたまり場」（FR-2.15）では退避を **ー ボタン**へ、kill を **× ボタン**へ
  分離した（2026-06-15。一時期 × = 退避としていたが、× で閉じても tmux セッションが
  残り「管理外」化する不具合と、タブの × との不整合のため kill へ戻した）。退避された
  バックエンドセッションは表示ペインを失っても生き続けられる（orphan として tmuxview に
  出る）構造が、ー 退避の前提となる

### close の確認ガードと発生源の監査（Issue #566。2026-07-27 仕様化）

2026-07-27、稼働中の master ペインが cmd+W の誤爆で確認なしに消え、原因調査に
master 1 セッション分の工数がかかった。事故の内訳は「確認が無い」と「事後に追えない」の 2 つで、
両方をここで塞ぐ。

- **確認ガード**（既定 ON。`confirm_close` = config.yaml / `tako confirm-close` /
  MCP `tako_confirm_close` で OFF 可）: GUI の × ボタン・**cmd+W** の close は、対象が
  次のいずれかなら確認ダイアログを挟む。⌘ を押しながらの × クリックは従来どおりスキップ
  - role 付きペイン（`orchestrator-master` / `orchestrator-worker` / `solo` 等 = エージェント）
  - シェルがコマンド実行中（OSC 133 の Running）
  - **ペインで子プロセスが動いている**（TUI エージェントは OSC 133 を
    出さないため。判定は sleep guard（FR-5 系）の 2 秒ごとの評価結果を再利用する。実際の
    tmux / ps 走査は backend・role・OSC 状態の変化時 + 60 秒保険に限定し（#779）、
    UI スレッドから tmux / ps を起こさない）。
    **busy の集合が変わった回は次の tick でもう 1 度だけ採って確かめる**（#2477）: プロセス表は
    1 瞬間の写しなので、シェルがプロンプトのたびに起こす数 ms の子（direnv の precmd・入力予測の
    非同期の子シェル・打ったばかりの短いコマンド）を拾うことがあり、短いコマンドは C と D が同じ
    tick の間に終わって指紋が Idle のまま動かないので、確かめ直しが無いと 60 秒の保険まで
    アイドルのシェルを busy と言い続ける（close 確認・GUI モードのスターター・スリープ防止がずれる）。
    増える採取は変化 1 回につき 1 回（毎 tick は採らない）。判定は
    `RunningChildrenScanState::settled_against` + `should_rescan_running_children_in`、
    A/B は `TAKO_2477_LEGACY=1`、注入は `TAKO_2477_INJECT=transient`（セルフテスト項目 93 (e)）。
    **器の有無で分岐しない**（#1367）: 判定は `RunningChildrenScanState::is_pane_busy` の
    1 実装を通し、器あり = 器のセッション名 / 器なし = tako のペイン ID という
    「数え方の違い」は答える側で吸収する。ここで `busy_sessions`（器のセッション名）を
    呼び出し側から引くと、**器を持たないペイン**（tmux 未導入 / persist OFF =
    Homebrew cask の既定構成）は必ず false になり、稼働中のエージェントが cmd+W 一撃で
    消える = #566 の事故がそのまま再現する（2026-09-12 の隔離 GUI で実測。走査側は
    `busy_agents=1` と知っていた）。同じ材料を引く GUI モードの判定（#694 の
    `busy_children`）とチャットの `agent_running` も同じ 1 実装を通す。
    A/B は `TAKO_1367_LEGACY=1`、番犬は
    `crates/tako-control/tests/issue1367_direct_pane_busy_watchdog.rs`、
    run ごとの機械検証はセルフテスト項目 73g
  - タブの × は、タブ内に上記のペインが 1 つでもあれば確認する
  - 上記に当たらない**空のシェルペインは確認なしで即 close**（日常操作の邪魔をしない）
- **CLI / MCP の close は確認を挟まない**（設計原則 5「AI フルコントロール」を維持）。
  代わりに発生源を必ず記録する
- **発生源の記録**: ペインログ（FR-5.13）のクローズマーカーへ発生源を書く。
  `close:kbd`（cmd+W）/ `close:gui`（ペイン ×）/ `close:gui-tab`（タブ ×）/
  `close:dispatch(cli)` / `close:dispatch(mcp[, caller=<role>])` / `close:internal`
  （アプリ内部の後始末）/ `close:auto`（実行ペインの auto_close。#1778）/
  `close:app-quit`（アプリ終了）/ `exit`（PTY 死亡）。
  `caller` は接続時の `TAKO_ORCHESTRATOR_ROLE` 由来で、ツール引数からは詐称できない。
  マーカー書式を壊さないよう英数と `-_:.` へ正規化し 64 文字で打ち切る

実装詳細は `architecture.md`「Phase 5.5: tmux バックエンド永続化」節。

## FR-6 リモートアクセス（スマホから）

> スマホのブラウザから tako のペインを閲覧・操作する。transport は Tailscale Serve
> 一本（tailnet 内限定・WireGuard E2E 暗号化・恒久固定 URL）。認証は機器ペアリングの
> 二層認証。全面刷新の計画は `.agent/plans/tako-remote-plan.md`（Issue #278 エピック）が正。

| ID | 要件 | 優先度 | 状況 |
|---|---|---|---|
| FR-6.1 | daemon は `127.0.0.1` のみ bind し、`tailscale serve` が HTTPS:443 → localhost へプロキシして tailnet 内へ公開する。未セットアップ（未導入・未ログイン・HTTPS 未有効）の `remote start` は不足項目を列挙して拒否し `tako remote setup` へ誘導する | M | ✅ #282 |
| FR-6.2 | PWA は daemon 自身が静的配信する（同一 origin・バージョン一致）。公開 Pages 配信は廃止。SW キャッシュの古いシェルは `/api/health` の version と PWA 埋め込み版の不一致で検出しキャッシュ破棄 + 再読み込みを促す | M | ✅ #283 |
| FR-6.3 | **層① Tailscale identity 検証**: `tailscale serve` が付与する `X-Forwarded-For` の接続元 IP を `tailscale whois` で照合し、tailnet 上の実在ノードのみ通す。ヘッダ無し（serve を経由しないローカル直結）・`peer not found`（偽装 IP）は拒否する（弾 0 実測: serve はクライアントの偽ヘッダを完全上書きする） | M | ✅ #283 |
| FR-6.4 | **層② 機器ペアリング**: ノードの恒久 ID（`Node.StableID`）をデバイス識別子として `<state_dir>/devices.json`（0600・atomic 書き込み・破損時は起動拒否）と照合する。未登録端末はペアリング要求（`POST /api/pair`）だけができ、画面データを 1 バイトも受け取れない。role = **Observe**（画面閲覧のみ・既定）⊂ **Interact**（+ 入力）⊂ **Manage**（+ close / resize）⊂ **Admin**（+ 端末管理）。role 不足の操作は 403 | M | ✅ #283 |
| FR-6.5 | **ペアリング承認・role 昇格は Mac 側 GUI ダイアログ限定**（設計原則 5 の例外。冒頭に理由明記）。承認 API はローカル管理トークン（`<state_dir>/tako-remote.token`、0600）+ ローカル直結限定で保護し、serve 経由（`X-Forwarded-For` あり）の管理 API アクセスは管理トークンが正しくても拒否 + 監査記録する | M | ✅ #283 |
| FR-6.6 | **端末失効の即時反映**: `tako remote devices revoke <id>` / MCP `tako_remote_devices` / GUI から失効でき、接続中端末の WS を即時切断する（broadcaster の subscriber をデバイス単位で drop）。長寿命 bearer token / QR への token 埋め込み / `--show-token` は全廃（QR は恒久固定 URL のみ） | M | ✅ #283 |
| FR-6.7 | **interact session の idle timeout**（既定 15 分）+ **接続開始終了・操作開始の macOS 通知**（osascript・`TAKO_REMOTE_NO_NOTIFY=1` で抑止）+ **status bar インジケータ**（接続端末数・承認待ちバッジ）とクリックで端末一覧・kill switch（全遮断 = `tako remote stop` 相当） | M | ✅ #283 |
| FR-6.8 | **監査 metadata**: 接続開始終了・ペアリング・入力 byte 数・revoke を `<state_dir>/audit.log`（JSONL・256KB ローテート）へ記録する。**ペイン内容・入力テキストは記録しない**（診断ログ規約の維持。ペインログ FR-5.13 と異なりこちらは内容を持たない） | M | ✅ #283 |
| FR-6.9 | start / stop / status / devices list / devices revoke / **devices role（降格）** は MCP・CLI に 1:1 公開（フルコントロール維持）。**承認と role 昇格のみ** GUI 限定（FR-6.5 / FR-6.21） | M | ✅ #283 / #1452 |
| FR-6.10 | **status bar インジケータは daemon 停止中も常時表示**する（消えるとリモート機能の存在ごと GUI から消え、`tako remote start` を知らないユーザーが到達できない）。停止中は中空リング + オフ文言で稼働中（塗りドット）と区別し、クリックで**起動パネル**（起動ボタン + `remote setup` の check と同じ不足項目の列挙 + `tako remote setup` のコピー行）を出す。起動失敗は daemon が返した理由をパネルへ出す（黙って失敗しない）。稼働中クリックは従来どおり端末一覧 + kill switch（+ 接続 URL のコピー行）。GUI の起動 / 停止 / 不足項目確認は dispatch と同じ操作関数（`remote::spawn_daemon` / `remote::daemon_stop` / `remote_setup::check_status`）を background で呼ぶ = CLI / MCP と 1:1（`dispatch::dispatch` は同期 API で UI スレッドを最長 30 秒塞ぐため直呼びしない。NFR-8） | M | ✅ #590 |
| FR-6.11 | **カードはインジケータ直上へアンカーし、起動 ⇔ 停止をカード内のトグルで往復できる**。カードの左端はボタンの実描画位置（paint フックで採取）にそろえ、右端がウィンドウ外へ出るぶんだけ左へ引き戻す（コンテキストメニュー #314 のようなフリップはしない = ボタンとの隣接を壊さないため）。主アクションは起動 / 停止で同じ形・同じ場所に置き、**停止は確認を挟む**（接続中の端末が切断されるため台数を文言に出す。0 件でも「以後すべての接続を受け付けない」ことを明示）。停止しても**カードは開いたまま**起動パネルへ切り替わり、その場で起動へ戻れる。停止失敗は起動失敗と同じく理由をカードに出す（黙って失敗しない） | M | ✅ #615 |
| FR-6.12 | **ペイン選択画面は「どれがどれだか分かる」ことを最優先にする**。各カードに ①**中身のスニペット** ②**誰か**（master / solo / worker / shell + agent 種別 + cwd）③**今どうなのか**（承認待ち / 実行中 / 待機 / 停止）を出し、カードは**タブごとにグループ化**する。スニペットは daemon が `/api/v2/panes` に `preview` として同梱する（ペインごとに screen API を N+1 で叩かない）。**エージェント TUI では入力欄より下（入力欄 + フッター）を落として会話の末尾を見せる** — 素の `capture-pane` 末尾はどのペインも入力欄とフッターで、行数を増やしても識別に寄与しない（#433 で増量しても「どれがどれだか分からない」が残った要因）。素のシェルには切り落としをかけない（`> vite dev` のような通常出力を入力欄と誤認して以降が消える）。状態はエージェントなら画面由来の `activity`（優先順位 `permission` > `busy` > `error` > `idle`。判定関数は watch と共有）、素のシェルは OSC 133 の `state` が正。承認待ちは**何を聞かれているか**、停止は**種別と対処**をカード上に出す。`preview` が無いペインは screen API へフォールバックし、それも失敗したら明示表示にする（無言の空欄にしない） | M | ✅ #621 |
| FR-6.13 | **serve 設定の自己検査と自動 re-assert**（#1049）。daemon は「どの tailscaled（= どのノード）へ serve を張ったか」を固定し、稼働中も一定間隔（既定 30 秒）で **公開した URL のノードに対して** `tailscale serve status` を確かめる。① 消えていれば張り直す ② **tako 形状でも応答している相手なら奪い返さない**（生きている別世代の daemon との :443 の奪い合いを構造的に作らない）③ ユーザーが張った設定（tako 管理外）は触らない ④ 張り直しは**上限つき**（既定 5 回。連続 10 回健全なら予算を戻す）で、上限に達したら止めて劣化を表に出す。既定探索（`--socket` 無し）は tailscaled 側の都合で応答するノードが変わる（実測 #1049: GUI 版の LocalAPI 発見ファイルが作り直された瞬間に standalone → GUI 版へ切り替わった）ので、**固定済みでも毎周期ノード名を照合**し、ずれたら公開ノードへ解決し直す。後始末（daemon 終了 / `tako remote stop`）も同じ handle を通す = 相手が入れ替わっても**本物の設定を消し残さない** | M | ✅ #1049 |
| FR-6.14 | **`running` だけを名乗らない**（#1049）。`tako remote status` / MCP `tako_remote_status` に `serve_ok` / `serve_state` / `serve_checked_at` / `serve_node` / `serve_reasserts` と、劣化時の `degraded`（`reason` + `next_step`）を載せる。**古い検査結果を健全と言わない**（間隔の 3 倍 + 30 秒を過ぎたら `stale`）。劣化は `warnings` にも 1 行入れて GUI のリモートカードへも届く。serve の増減（設定・解除・張り直し・上限で断念・相手の入れ替わり・ノード消失）は `<state_dir>/audit.log` へ **pid と実行ファイルつき**で記録する（世代違いの tako が犯人なら名指しできる） | M | ✅ #1049 |
| FR-6.15 | **リモートからのファイル参照（ブラウズ / プレビュー / ダウンロード）**（#1079。柱 3-E）: スマホから PC のファイルを見て・中身を確かめて・保存できるようにする。`GET /api/files`（`root` 省略でルート一覧・指定でディレクトリ一覧）/ `/api/files/content`（プレビュー本文）/ `/api/files/download`（添付としてストリーミング）。**読み出しでも Interact 以上**にする（画面の閲覧と「ファイルの中身を丸ごと持ち出せる」ことは危険度が別物なので、Mac 側で明示的に昇格した端末にだけ許す）。**認可の正は「ファイルツリーに現に出ているルートの配下だけ」**で、ルート一覧は daemon が溜め込まず**毎リクエスト** tako app へ問い合わせる（`TreeFolder { action: "roots" }` → サイドバーと同じ `tree_roots_of_tab`）ので、Mac 側でペインを閉じるかピン留めを外せば**その瞬間に読めなくなる**（daemon 側に独自の許可リストが育たない = 画面に出ていないものは配らない、を構造で守る）。拒否は 2 段構え（`tako_control::remote_files`）: ①**FS に触らない純粋関数**が絶対パス（POSIX `/` / Windows ドライブ `C:` / UNC `\\`）と `..` と制御文字をホスト OS に関係なく落とす（ドライブ形は**全コンポーネント**で落とす: `PathBuf::push` は Windows で prefix つきの断片を渡すと**それまでのパスを捨てる**ので `a/C:/x` を素通しにすると外へ出られる）②`canonicalize` した実体がルートの配下かを**コンポーネント単位**で照合する（文字列の前方一致では `/a/bc` が `/a/b` の配下に見える。**ルート内の symlink が外を指していてもここで落ちる** = リンク自身もリンク先も 403 で、一覧では `escapes_root` の印を付けて「押しても開けない」と分かるようにする）。符号化は認可の前に**1 回だけ**解く（`%2e%2e` は traversal として拒否・二重符号化は実在しない名前として 404）。**認可の失敗は一律 403**（404 と撃ち分けるとツリー外のファイルの存在が漏れる）。`Content-Disposition` のファイル名はヘッダに載せる前に正規化する（ASCII 側は英数と `.-_` だけ + RFC 5987 の `filename*` で本来の名前を渡す = 改行・引用符でヘッダを割れない）。応答は `Cache-Control: no-store, private`。**監査ログにはパス・ファイル名・ルート名を 1 つも載せない**（ペイン内容と同基準。FR-6.8 / #287 P2-2）: 残すのは種別（roots / list / content / download）と**バイト数・件数**だけで、番犬テストが「監査への追記は `audit_payload` 経由だけ」をソース走査で機械検証する。上限はプレビュー本文 512KB（超えたら本文を返さず保存へ倒す）/ ダウンロード 512MB / 一覧 1000 件。PWA 側は `#/files`（ツリー → プレビュー → 保存。ホームのヘッダに導線 1 個）。**設計原則 5（AI フルコントロール）との関係**: これは既存機能の**リモート転送路**であり、「ローカルのファイルを一覧・プレビューする」capability 自体は AI から既に完全に操作できる（`tako open` / MCP `tako_open_file`・`tako tree list` / `tako_tree_folder`・および AI は Mac 上でファイルを直接読める）。この API を新しい MCP / CLI ツールとして重複公開しないのは、**転送路の入口が「ペアリング済みの機器」であって AI ではない**ため（FR-6.5 と同じ理由で、AI がリモート端末の権限を自律的に扱えるようにしない）。既存の `/api/upload`（FR-6 弾 5b）と同じ扱い。ただし**認可の材料**（どのルートがツリーに出ているか）は `tako orchestrator` 系ではなく既存の `TreeFolder` へ `roots` action として足したので、AI からも `tako tree` 経路で同じ一覧を引ける。書き戻しは FR-6.16（柱 3-F）、SSH 先のファイルは FR-6.17（柱 3-G）で、**どちらもこの認可の 1 実装をそのまま通す**（✅ 2026-09-03、#1079） | S |  |
| FR-6.16 | **リモートからのファイル編集**（#1084。柱 3-F）: スマホで開いたファイルをその場で直して保存できるようにする。`PUT /api/files/content`（body = `{ text, etag }`）。認可は FR-6.15 の 1 実装をそのまま通し、**role も読み出しと同じ Interact 以上**（`/api/upload` と同じ基準。observe は 403）。**書くのは daemon ではなく PC 側の編集経路**（`PreviewEdit` → `PreviewApply` → `PreviewSave`）: 書き戻しの難しいところ（アトミックな置き換え・外部変更の検知・リモートの mode 復元・押し出せなかった保存の退避）は #966 が PC 側に持っているので、daemon が自分で書くと保証が 2 つになり**片方だけ直る**形になる。daemon が足すのは「認可」と「スマホが読んだ時点との突き合わせ」だけ。**競合検知は 2 段（窓が違う）**: ①**スマホが読んだ → 保存を送った**の窓は検証子（`content_etag` = 長さ + 内容ハッシュ）で見る。PC 側はこの窓を見られない（編集セッションの基準は「編集を始めた時点」なので、保存の直前に開くと必ず「変わっていない」に見える = **これを入れないと他人の変更を黙って踏み潰す**）②**適用した → 書いた**の窓は PC 側の `TextBuffer::save` /（リモートは）`remote_fs::save_file` が見る。検証子は**daemon が読み出しの応答に載せてスマホに返させる**（スマホ側で計算させると同じ算法を JS と Rust の両方へ実装することになり、片方だけ壊れたときに「競合を見落とす」側へ倒れる）。暗号学的ハッシュにしていないのは、守るのが「うっかり踏み潰さないこと」で偽造耐性は要らない（検証子を偽れる端末は Interact 以上なので本文に何を書いても通る = 偽造で新たに得られる権限が無い）ため。**検証子が無い / 合わない保存は書かずに 409**。他の 3 つの断り方: **Mac 側に未保存の編集があれば 409 `busy_editing`**（ユーザーの書きかけを踏み潰さない）/ テキストとして編集できない種別は 400 `not_text`（**拡張子の表を daemon 側に持たず app の分類 `preview.mode` を使う**ので対応形式が増えても食い違わない）/ リモートが読み取り専用なら 403 `read_only`。**保存先のペインは「既にそのファイルを出しているペイン」を先に探して使い回す**（`OpenFile` を呼ぶとユーザーが見ている別のプレビューを差し替えてしまう。照合は両辺 canonicalize = macOS の `/var` と `/private/var` で取りこぼさない）。フォーカスは奪わず、**呼ぶ前に編集モードでなければ保存後に戻す**。PWA は `#/files` のファイル画面に編集モード（編集 → 保存 / 取り消す、競合なら「読み直す」）を足す。**設計原則 5 との関係は FR-6.15 と同じ**（既存機能のリモート転送路。編集そのものは `tako edit` / MCP `tako_preview_edit` 系で AI から完全に操作できる）（✅ 2026-09-03、#1084） | S |  |
| FR-6.17 | **リモートから SSH 先のファイルを扱う**（#1085。柱 3-G）: `remote-folder open` 済みの SSH 先を**ローカルのルートと同じ 1 本の一覧**に並べ、プレビュー・編集・押し出しをスマホからできるようにする。並びは **app の答え（`placement`）をそのまま使う**（#1041 の規則を daemon 側で二重に決めない）。ルート一覧は `RemoteFolder { action: "list" }` を**毎リクエスト**引くので、Mac 側でフォルダを閉じればその瞬間に読めなくなる（FR-6.15 と同じ）。`ls` / `open-file` / `push` / `pending` は daemon が **proxy するだけ**（SFTP を daemon 側に生やさない = **スマホは SSH 鍵に触らない**。2 ホップの後段は Mac の ControlMaster 経路のまま）。**#966 の保証をそのまま素通しさせる**: アトミックな `put` → `rename`・**内容そのもの**での競合検知・mode 復元・押し出せなかった保存の退避。読み出しでも `open-file` を通すのは、取得のたびに #966 の競合検知の基準が進む = **「読み直してやり直す」の導線がそのまま効く**ため。**`force` はスマホから出さない**（競合を承知で踏み潰す操作を作らない。ボディに `force` を入れても受け取るキーは root / path だけ）。押し出せなければ **502 + `kind: "remote_pending"` + `pending: true`**（「書けていない」を成功と言わない）で、退避は `GET /api/files/pending` で見え `POST /api/files/push` で送り直せる。失敗の分類は**退避の記録から取る**（`kind` が `conflict` なら読み直し / それ以外は送り直し）ので、app のエラー文言に依存しない。**SSH 側の配下判定は文字列**（`check_relative_shape` で絶対パス・`..`・制御文字を落として要素ごとに継ぐ）: リモートには安く実体を問う手段が無いため、**相手側の symlink がルートの外を指していれば追える**。これは Mac のツリー自身が同じホストに対して持っている権限と同じで、受容するリスクとして `.agent/threat-model-remote.md` に明記した（書き込み側は #966 が「開いた記録が無ければ書かない」でさらに閉じている）。id は接頭辞 `s-` で分けてあり、ローカルの id（16 進数）と**構造的に取り違えない**（✅ 2026-09-03、#1085） | S |  |
| FR-6.18 | **接続元プロセスの検証**（#841）。ループバック TCP（既定）で待ち受けているとき、`X-Forwarded-For` を層①の identity として読む前に**接続元プロセスを引き当てて検証**する。接続の 4 つ組から所有 pid を引き（macOS = `net.inet.tcp.pcblist_n` / Windows = `GetExtendedTcpTable`）、①ソケットの所有ユーザーが自分自身か root ②実行ファイル**名**が tailscale デーモンのもの（設置場所では判定しない）、の両方を満たすときだけ信じる。材料が 1 つでも欠けたら 403（fail-closed）で、理由コードを監査ログ・`persist.log`・`tako remote status` の `peer_verification` に残す（ヘッダの中身・XFF の IP・トークンは残さない）。**分岐はプラットフォームではなくエンドポイントの形**で行い、`TAKO_REMOTE_ENDPOINT=unix` を明示した UDS 経路は（0600 でカーネルが同じ境界を強制済みなので）検証を通さない。Windows は所有ユーザーを引く手段が無いので所有者ゲートを強制できず、`owner_check` が `unavailable` と名乗る（残存リスクは `.agent/threat-model-remote.md`） | M | ✅ #841 |
| FR-6.19 | **リモートからの新規起動は「+」の 3 択**（#1449）: PWA の「+」で **master を起動 / ターミナル / SSH ターミナル**を選び、選んだものが**新しいタブ**に立って PWA がそのペインへ移る。**操作は 1 つも新設しない**（設計原則 5 の 1:1 を壊さない）: master = `POST /api/tabs` → `POST /api/tabs/:id/master`（FR-6 の #1078 経路）/ ターミナル = `POST /api/tabs` だけ（`TabNew` がシェル 1 枚付きのタブを返すので、ペインを作る操作を別に足さない）/ SSH = `GET /api/ssh-hosts` → `POST /api/ssh { target: "tab" }`（#1080 / #1006 の `OpenRemote` がタブごと作るので `/api/tabs` は通さない）。CLI の `tako tab new` / `tako open-in remote <host> --target tab` と**同じ dispatch**を通り、ホスト一覧は `Request::SshHosts`（= `tako ssh-hosts`）の答えをそのまま出す（PWA 側で `~/.ssh/config` を解釈し直さない）。**role は 3 種とも Manage 以上**で FR-6 の #1078 / #1080 の判断を据え置く（新しいタブとプロセスを作れる = 実質シェルアクセスなので close / resize と同じ強さ。「起動は Interact 以上」案は、既にペアリング済みの Interact 端末の権限が黙って広がるため不採用）。observe には**選択肢を出さず理由を出す**（押してから 403 にしない）。経路は `tako_control::remote_launch::LAUNCH_ROUTES` の 1 表が正本で、`required_role` がそこを引き、番犬が「PWA が呼ぶ API が表に在るか」をソース走査で照合する（未知の経路が Observe へこぼれる = #1405 の懸念を構造で塞ぐ）。接続に失敗してもペインは残り理由が `ssh_connect` に出続ける（#919 / #1040）（✅ 2026-09-13、#1449） | S |  |
| FR-6.20 | **リモートのファイル閲覧を Finder 風の全体閲覧へ広げる**（#1451）: `#/files` を 3 節（**ショートカット** / **tako のツリー**（#1079 の見え方そのまま）/ **このマシン**）にし、「このマシン」から `/`（Windows はドライブごと）を辿れるようにする。**認可は role で表す**: 全体閲覧とショートカットの読み書きは **Manage 以上**（判定の正は `tako_control::remote_files::FULL_BROWSE_ROLE`）で、**Interact 以下は #1079 のままツリーのルート配下だけ**（既にペアリング済みの端末の権限が UI の都合で黙って広がらない = #1449 で同型の案を却下したのと同じ理由）。`manage` を選んだのは、①role 昇格が Mac 画面の GUI ダイアログでしか行えない（FR-6.5）ので「全体を読める端末 = 所有者がその場で承認した端末」が構造で保たれる ②`manage` は既に close / resize とタブ / プロセスの新規作成（FR-6.19）ができ、そこから `cat` できる = 全体閲覧が新しく与えるのは「楽に読める」だけ、の 2 つ。採らなかった案は「全 role で全体閲覧」（既存 Interact 端末の権限が広がる）と「設定で ON/OFF」（見せる範囲の正が role と設定の 2 本に割れる）。**通る経路は 1 本も増やさない**: 疑似ルート `fs`（Windows は `fs-c`）を一覧へ **1 件足すだけ**で、プレビュー（FR-6.15）/ 編集（FR-6.16）/ SSH 先（FR-6.17）は認可もパス判定も **1 実装をそのまま通る**（`/api/fs?path=<絶対パス>` の新設は content / download / PUT / pending / push を二重化することになるので採らない）。id の空間は構造的に分かれている（ツリー = 12 桁 16 進 / SSH = `s-` / 全体閲覧 = `fs`）。**認可の門は `local_roots_for` の 1 か所**で、一覧に載らなければ `resolve_in_root` が `unknown_root` で 403 にする = id を推測して直接叩いても通らない。**経路の宣言は `FILE_ROUTES` の 1 表**（FR-6.19 の `LAUNCH_ROUTES` と同じ作法。`required_role` が表を引き、番犬が「PWA が呼ぶ API がすべて表に在るか」をソース走査で照合する。**表に無い `/api/files…` は安全側の Manage へ落ちる** = 未知の経路が Observe へこぼれない）。**ショートカット**（お気に入り）は PWA・CLI（`tako remote shortcuts [add|remove]`。引数なしで一覧 = #322 の最簡形）・MCP（`tako_remote_shortcuts`）の 3 口が `tako_core::remote_shortcuts` の 1 実装を通り、永続は `<data_dir>/remote/shortcuts.json`（0600・移行の番地 `SchemaId::RemoteShortcuts`）。**既定（ホーム / デスクトップ / ダウンロード）はファイルに書かず毎回計算する**（書くとホームが変わった環境で腐り、「消したのに復活する / 消したまま戻らない」が両方起きる）ので既定は消せず、登録分だけ消せる。追加は**冪等**（id がパス由来なので同じパスは 1 件のまま）で、既定と登録分は**同じ正規化基準**（`platform::path::canonicalize`）を通す（違えると同じフォルダが 2 行に出る = 実測で踏んだ）。飛び先は**常に `fs` ルート**を選ぶ（ツリーのルートの方が相対は短いが、選ぶと Mac 側でそのフォルダを閉じた瞬間に 403 になる）。**読めないものは黙らせない**（FR-3.24 系）: フォルダごと読めなければ理由と戻り道、一覧の中の 1 件が読めなければ `unreadable` の印。一覧は**上限まで切ってから実体を問う**（旧実装は全件に `symlink_metadata` + `canonicalize` + `metadata` を掛けてから 1000 件へ切っており、全体閲覧では万単位のディレクトリで確実に踏む）。絶対パスを応答へ載せるのは全体閲覧のときだけ（ツリー由来では従来どおりルート名 + 相対パス）。監査ログは FR-6.15 と同じ基準（パス・ファイル名を 1 つも載せず、種別と件数だけ）。A/B は `TAKO_1451_LEGACY=1`（PWA は `?tako_1451_legacy=1`）（✅ 2026-09-14、#1451） | S |
| FR-6.21 | **権限の更新リクエストと端末ごとの権限編集**（#1452）: スマホの「権限が足りません」から**「権限の更新をリクエスト」**（要求 role・理由・端末名）を送れるようにし、PC 側に**端末ごとの権限編集画面**（設定 → リモート。role の選び直しと登録の削除）を置く。**新しい API は 1 本も作らない**: リクエストは #283 からある `POST /api/pair`（登録済み端末が現 role と違う role を要求すると `RequestKind::Upgrade` の保留ができる）で、足したのは `GET /api/me` が**登録済み端末にも** `pending` / `requested_role` / `denied` を返すこと（無いと「送れたのか」を知る手段が無い）と、role を直接置く `POST /api/admin/devices/role` の 1 本だけ。PWA は承認待ちの**間だけ** `/api/me` を 2 秒で見に行き、承認 / 拒否 / 打ち切りで必ず止める（承認されたら**再読込なしで**続きができる）。リクエストは**人が答えるものの記録**として `tako todo`（#1450 の kind=permission）へ 1 件だけ起票し（同じ端末・同じ要求では再起票しない）、承認・拒否で畳む。**授権の正本は `DeviceRegistry.pending`（メモリのみ = daemon 再起動で消える安全側）のままで、タスクを done / dismiss しても権限は 1 ミリも動かない**（逆向きの経路が無いことを番犬が縛る）。承認 UI は #283 の `render_pairing_dialog` を**そのまま共有**し（設定画面に 2 つ目の承認口を作らない）、端末が書いた理由はダイアログとタスク本文にだけ出す（**監査ログには載せない** = FR-6.8）。**昇格は PC の人の操作でしか成立しない**（FR-6.5 の例外を維持・詳細は下記）。降格・削除・一覧・保留中の要求一覧は CLI（`tako remote devices role / revoke / list`）と MCP （`tako_remote_devices`）から 1:1 で行える（保留は `list` の応答の `pending` に入るので、専用のサブコマンドは足さない = #322）（✅ 2026-09-14、#1452） | S |  |
| FR-6.22 | **「起動していた」を覚えて GUI 起動時に立て直す**（#1485）: remote daemon は GUI とは別プロセスなので Mac の再起動で消え、pid ファイルだけが残る。タブ・ペイン・プレビュー・SSH の追跡（#1446）は persist が戻すのに remote だけが戻らず、スマホからは「PWA が繋がらない」としか見えなかった（実測 2026-09-21: 復元成功のあと `running: false` のまま・PC 側にも何も出ない）。**persist が layout を戻すのと同じ思想**で、`tako remote start` の成功だけが `<data_dir>/remote/tako-remote.desired` を作り（移行の番地 `SchemaId::RemoteDesired`。**ファイルが在ること自体が意図**なので形式の検査は持たない = 壊れた中身を理由に移行が退避するとユーザーの意図が黙って消える）、GUI の起動時（persist 復元のあと）に daemon が居なければ `spawn_daemon` を呼ぶ。**消すのは「止まっていること」の 1 つ**（`tako remote stop` が `Ok` を返したときだけでなく「もともと止まっていた」ときも消す。`Ok` だけを見ると Mac 再起動後に `stop` した人の意図を取りこぼして次回勝手に戻る）で、daemon の死（`cleanup_state_files`）では消えない。Mac の再起動直後は tailscaled が上がりきっていないので**バックオフで数回**（既定 2/5/10/20/40 秒 = 合計 77 秒。検証用の上書きは `TAKO_1485_BACKOFF_SECS` / `TAKO_1485_ATTEMPTS`）試し、**試行のたびに判断を引き直す**（待っている間に `stop` されたら降りる）。判断は理由を返す純粋関数 1 本（`remote_autostart::autostart_decision` → `not-desired` / `already-running` / `unsupported` / `disabled`）で、読む側（CLI・チップ・persist.log）は**再計算せず状態を読むだけ**（#372 / #1473 と同じ）。**無言にしない**（FR-6.14 と同じ方針）: 途中の失敗は persist.log へ 1 行ずつ（再試行の 1 分以上を無音にしない）、諦めたら `notify_ui_failure` の 1 実装でステータスバーのリモートカードへ理由を出し、`tako remote status` / MCP `tako_remote_status` に `desired` / `desired_since` / `last_autostart`（`result` = started / skipped / failed・`attempts`・`reason`・`detail`）を **`running: false` の応答にも必ず**載せる。**権限は 1 つも増やさない**（戻すのは daemon だけで `devices.json` と role は触らない）。OS の可否は `platform::support` の `tako_remote_start` の宣言をそのまま引く（Windows は daemon が unix ソケット前提で `Pending` = #971。**マトリクスへ専用キーは足さない** —— キーの正は MCP ツール名なので `platform_parity` の T2 が落ちる）。A/B は `TAKO_1485_LEGACY=1`（再起動後に `running: false` のまま = Issue の症状）（✅ 2026-09-21、#1485） | M |  |
| FR-6.23 | **`remote setup` は依存導入を自前で持たない**（#1509）: [1/5] の Tailscale 導入は `tako setup` の依存チェック段と**同じ 1 実装**（判断 `setup_deps::offer_for` → 実行 `setup_deps::install` → 再検出 `setup_deps::resolve`）を通る。詳細は FR-2.33.11 | M | ✅ #1509 |
| FR-6.24 | **スマホからコマンド提案カードを実行する**（#1724）: ペイン画面（`#/panes/<id>`）に、AI がそのペインへ出したカード（FR-2.22）を出し、カードごとに「実行」を押せる。**実行は PC のカードの「新規ペインで実行」と同じ dispatch**（`Request::ShowCommand` の `run` を daemon が素通しする）なので、実行先（同じタブに割る新しいペイン・フォーカスは動かさない）も実行記録（FR-2.22.11）も PC と揃う。受け口は宣言表 `remote_cards::CARD_ROUTES` の 2 本だけ: `GET /api/cards?pane=<id>` = **Observe**（カードは PC の画面に出ているもの）/ `POST /api/cards/<id>/run`（本文は `{index}` だけ）= **Interact**（ペインへ打鍵できる役割と同じ強さ。打鍵は任意の文字列を送れるので、AI が置いた文字列しか走らない実行をそれより強くする理由が無い）。**スマホからコマンドの本文を渡す口は作らない**（`show` / `copy` / `dismiss` は表に載せず、表に無い `/api/cards…` は床の Manage → 404）。受け口の中でも表の role に届かない端末を断る（二重の門）。スマホでは**全文を見せる確認を 1 回**挟み、送信中はボタンを止める。observe の端末には実行ボタンを出さず #1452 の権限リクエストへつなぐ。失敗は画面が出し分ける: 実行中 = 409 `still_running` / PC 側で閉じられた = 404 `not_found` / その場で降格・失効 = 403（me を取り直して権限の導線へ）。監査（`audit.log` の `card_run`）と persist.log には「どの端末から・どのカードの何件目を・結果」だけを残し、コマンド本文・cwd・ペインの内容は残さない | M |  |
| FR-6.25 | **停止は止まったのを観測したときだけ成功と言う**（#1599）: `tako remote stop` の正規経路（PID ファイル）と stale 経路（PID ファイルが無い。#2455）は**同じ 1 実装の段**（`remote::stop_verified_daemon`）を通る。正体確認（#329 / #1401 / #1616）の後、① admin API の `POST /api/admin/shutdown`（ローカル直結 + 管理トークンのみ。daemon 自身が serve の解除と state の掃除をして畳む = **OS を問わない穏当な停止**。応答の pid が確かめた pid と一致したときだけ受け付けとみなす）→ ② 境界 `platform::process::terminate`（unix は SIGTERM。Windows は `TerminateProcess`）→ ③ stale 経路と `--force` だけ強制終了、の順に進む。**stale 経路の手掛かりは管理トークンの停止要求（#2455）**: `/api/health` は層①（tailnet の身元）を通った要求にしか答えずローカル直結は 401 なので pid を聞けない（#2455 以前は「起動していない」と返し、daemon が生き残った）。記録された待ち受けへ管理トークンで ① を送り、**受け付けた daemon が答えた pid** を手掛かりにする（トークンはその state の daemon しか持たないので、別の data dir の daemon は 401 で止めない）。受け付けたらまず畳み終えるのを待ち、予算内に終わらなければ正体確認のうえ ② → ③ へ上げる。**pid が終わっても待ち受けが応答していれば止めたことにしない**。トークンが無い・合わない・受け付けた印が無い相手には何も撃たず、相手を確かめる手順（`lsof` / `Get-NetTCPConnection`）を添えた Err で state を残す（「起動していない」とは言わない）。PID ファイルが無いのに待ち受けが応答している間、`tako remote status` は `running: false` のまま `pidless_listener`（到達先）と `warnings` で知らせる。通常 stop は強制へ上げず `--force` を案内する（従来の約束）。**要求を出せない・予算内に終わらないときは Err で state を残す**（撃たずに `stopped: true` を返す嘘を作らない。#1599 以前の stale 経路は撃つ部分が `#[cfg(unix)]` で、Windows は何も撃たずに成功を返していた）。応答の `via`（`shutdown` / `terminate` / `force_terminate`）でどの段で止まったかが分かる。撃つのは境界だけで、`remote.rs` に `libc::kill` を書かない（番犬 `issue1599_remote_stop_boundary_watchdog`）。**正規経路も、正体を確かめられずに断るときは state を残す**（#2466）: 断る前に PID ファイルと管理トークンを消すと、相手が本物の daemon だったとき PID ファイル無しの daemon が残り、`forget_if_stopped` が起動の意図まで消していた。断る応答は相手を確かめる手順（`ps -p` / `tasklist`）・止める手順・記録を消す手順を添える。起動時刻は**片側**で照合する（`procinfo::started_before_record`。記録より後に起動 = pid の再利用。記録より前ならどれだけ前でも本人）。両側 ±5 秒だった頃は、起動直後の tailscale が遅く起動 → 記録が 5 秒を超えた daemon を本物と認めず止められなかった（実測: 8 秒で再現） | M | ✅ #1599 / #2466 |

## NFR 非機能要件

| ID | 要件 | 目標値 |
|---|---|---|
| NFR-1 | 起動時間（コールドスタート → 入力可能） | < 500ms 目安 |
| NFR-2 | キー入力レイテンシ | 既存ネイティブターミナル（Alacritty/Zed terminal）と同等 |
| NFR-3 | アイドル時メモリ（1 タブ 1 ペイン） | Electron 系より明確に軽いこと（目安 < 200MB） |
| NFR-4 | 描画 | スクロール・大量出力で 60fps 維持 |
| NFR-5 | 対応 OS | macOS（先行）、Windows（必須）。Linux は将来検討 |
| NFR-6 | ライセンス | GPL-3.0-or-later（依存クレート zlog/ztracing が GPL-3.0 のため）。cmux のコード参照は引き続き禁止（`concept.md` 参照）。**配布物には `LICENSE`・`THIRD-PARTY-NOTICES.md`・`THIRD-PARTY-LICENSES.md` の 3 本を同梱する**（macOS = `tako.app/Contents/Resources/`、Windows = zip の `tako/` とインストール先。Windows では `LICENSE` を `LICENSE.txt` にする。#1709 / #1845）。組み立ての全箇所は番犬 `crates/tako-control/tests/license_bundle_watchdog.rs` が、実物の中身は Windows のリリース検査（`installer/windows/lib/verify-assets.ps1`）が、`THIRD-PARTY-LICENSES.md` が今の依存と一致するかは CI の `licenses` ジョブ（`scripts/third-party-licenses.sh --check`。#1851）が見る。詳細は `.agent/release.md`「配布物に同梱するライセンス」 |
| NFR-7 | 配布 | 単一バイナリ/アプリバンドル。ランタイム依存なし |
| NFR-8 | メインスレッド非ブロック（#168） | UI スレッド（GPUI イベントループ）でサブプロセス実行・重 I/O・重計算をしない。dispatch は文脈収集のみ UI で行い実行は background（`dispatch::prepare_offload`）。32ms 超の専有は `diag::perf_span` が perf.log に記録し、新機能はこの記録が出ないことを確認する（実装指針は `architecture.md`「メインスレッド非ブロック化」節）。子プロセスの待ちには必ず上限を付け（#1979）、UI スレッドの同期の dispatch で子プロセスを待たない（#1984: `RemoteAgents` / `TmuxList` は background のジョブ、呼び出し元の pid 祖先辿り・spawn / handoff の CLI の所在確認・セッション再起動の計画は**先取り** = `OffloadJob::Probe`。番犬は `issue1968_ui_thread_subprocess_watchdog` / `issue1979_ui_thread_unbounded_wait_watchdog` / `issue1984_ui_thread_offload_watchdog`、実経路は `scripts/test-ui-thread-offload-1984.sh`、A/B は `TAKO_1984_LEGACY=1`。#1989: コンフリクト解消エージェント・`StaleBinary status`・tmux の書き込み系も同じ形、agent CLI の所在確認は上限つきで打ち切りは `cli_lookup_timed_out`。実経路は `scripts/test-ui-thread-offload-1989.sh`、A/B は `TAKO_1989_LEGACY=1`。#1991: git の書き込み系（commit / pull / push / stage / unstage / checkout / branch / merge / merge-abort）と conflicts は cwd だけ写した background のジョブ（`OffloadJob::GitWrite`。同じリポジトリへの書き込みは background でも 1 本ずつ）、起動時の orphan の一括回収は計画だけ UI スレッドで本体は background、起動時の自動復帰の tmux の 3 口は上限つき。実経路は `scripts/test-ui-thread-offload-1991.sh`、A/B は `TAKO_1991_LEGACY=1`。#2014: プレビューの変更履歴を開く切替（CLI / MCP / 「履歴」ボタン）はパスだけ写した background のジョブ（`OffloadJob::PreviewChangelog`）で、`git rev-parse` + `git log --follow` は合わせて上限 30 秒（`TAKO_CHANGELOG_GIT_TIMEOUT_SECS`）、上限に達したら状態を変えずに失敗として知らせる。実経路は `scripts/test-changelog-offload-2014.sh`、A/B は `TAKO_2014_LEGACY=1`。#2150: 変更履歴の行の diff の展開（CLI / MCP の `expand` / コミット行のクリック）も同じ形（`OffloadJob::PreviewChangelogDiff`）で、`git diff` / `git diff-tree` は変更履歴と同じ上限、上限に達したら行を畳んだまま失敗として知らせる。実経路は `scripts/test-changelog-expand-2150.sh`、A/B は `TAKO_2150_LEGACY=1`。#2042: `issue1984_ui_thread_offload_watchdog` の 15 節は UI スレッドの入口から呼び出しを辿り、別名の関数で包んだ口も名指しする（既知の残りは返答の配送で起票 master を起動するときの `master_launch::plan` = CLI の所在確認）） |
