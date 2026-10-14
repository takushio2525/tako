# ドキュメントサイトのモーション解説（#2053）

> エピック #2053（ドキュメントサイトの刷新）の S1 で作った土台の手引きと、S2 の機能ページの構成案。
> **S2 の worker はこのファイルだけ読めばページを作れる**ことを目標に書いている。
> 事実（何ができて何ができないか）の正本は tako 本体のコード・`.agent/requirements.md`・
> `docs/src/content/docs/windows-support.md`（生成物）。ページに書く前に必ず突き合わせる。

## 1. 何を作ったか（S1）

- **スクロール連動の場面（Scrolly）**: 左に手順の文章、右に tako の画面を HTML/CSS で再現した図。
  手順を読む位置まで送ると、図がその手順の状態に変わる（狭い画面では図が上に貼り付き、手順がその下をくぐる）
- **自動再生（Autoplay）**: スクロールの手順を持たない場所（トップのヒーロー）用。画面に入ると時間で 1 回だけ進む
- **画面の再現部品**: tako の窓・ペイン・分割・サイドバー・右パネル・コマンドカード・タスク・チャット・
  スターター・初回バナー・スマホ版（PWA）・OS のターミナル・GitHub Releases・SmartScreen・Windows のインストーラー・ポインター
- **ページ**: トップ（`index.mdx`。splash = サイドバー無し）とインストール（`getting-started/index.mdx`）
- **サイト全体**: 全ページのヘッダーに「ダウンロード」（`src/components/HeaderLinks.astro`）。
  本文 Markdown の装飾（リストの余白・行頭記号・段落の色・h2 の「—」）を `.not-content` の中へ効かせない
- **Windows の数字はビルド時に生成ページから読む**（`windowsSupport.ts`）。手書きしない

## 2. 方式の比較と決定

| 候補 | JS | 良い点 | 採らなかった理由 |
|---|---|---|---|
| **IntersectionObserver + scroll（rAF 間引き）+ CSS の遷移（採用）** | gzip 1.9KB・依存なし | 「手順 N の状態」を属性で宣言するだけ。全ブラウザで動く。静止図への切り替えが簡単 | — |
| CSS scroll-driven animations（`animation-timeline: view()`） | 0 | JS 不要 | 対応ブラウザに差がある（Firefox は長く既定で無効だった）。手順の切り替えは離散的な状態なのに、`@keyframes` の % が要素の高さに依存して壊れやすい。reduced-motion の静止図を別に組む必要がある |
| GSAP + ScrollTrigger | gzip 約 45KB | 表現力が高い | 必要なのは「状態の切り替え + CSS の遷移」だけで、重さに見合わない。依存が増える |

計測（刷新前 → 後。chromium headless・gzip 配信・Lighthouse 12 のモバイル既定・各 4 回の中央値）:

| ページ | 転送量 | Performance | FCP（シミュレーション） | FCP（実測） |
|---|---|---|---|---|
| トップ | 728KB → 885KB（うち日本語フォントの分割ファイル +141KB） | 60 → 57 | 6.7 秒 → 8.2 秒 | 約 2.3 秒 → 約 2.3 秒 |
| インストール | 1014KB → 1034KB | 57 → 57 | 9.0 秒 → 9.0 秒 | 約 2.3 秒 → 約 2.3 秒 |

トップのシミュレーション上の FCP の差は、描画を止める CSS が 1 本（モーション用 gzip 8KB）増えたぶん。
実測は外部の同意スクリプト（`consent.js`）の待ちが支配的で、前後で変わらない。CLS はヒーローの分割で 0.023（良好の閾値 0.1 未満）。

## 3. 仕組み（1 つだけ）

「いまの手順番号 N」をステージ（再現した画面）へ写す。制御は `docs/src/scripts/motion.ts`。

- ステージに `data-step="N"` と CSS 変数 `--step` が付く（場面ごとの CSS が必要ならこれを使う）
- ステージの中の **`data-at`** を持つ要素は、指定が N に当たれば `.is-on` が付く（見え方は `motion.css`）
- **`data-pos`**（ポインター）は N に応じた座標へ滑って動く

### `data-at` の書き方（手順番号は 1 始まり = 読者に見える番号）

| 書き方 | 意味 |
|---|---|
| `3` | 手順 3 のときだけ |
| `3+` | 手順 3 から後ずっと |
| `2-4` | 手順 2〜4 |
| `1 3+` | 空白区切りで足し合わせ |
| `!3+` | 全体の否定（手順 3 より前だけ） |

### `data-fx`（出し入れの見え方。省略時は下から浮き上がる）

| 値 | 見え方 | 主な使いどころ |
|---|---|---|
| （なし） | フェード + 6px 上がる | 行が増える |
| `fade` | フェードだけ | 重ねて入れ替える層 |
| `pop` | 少し拡大しながら | カード・バッジ |
| `left` / `right` / `down` | その向きから | 帯・通知 |
| `type` | 文字数ぶんの段で左から開く（打鍵） | コマンド・入力欄（**1 行の短い文に限る**） |
| `unfold` | 高さ 0 から開く（**子要素 1 つで包む**） | 帯・アコーディオン・バナー |
| `split` | 幅（縦分割なら高さ）0 から伸びる | 分割で現れるペイン（`Split` の直下の子） |
| `mark` | 隠さない。`.is-on` の間だけ押された輪 | 押されたボタン・選ばれた項目 |
| `state` | 隠さない。`.is-on` の間の姿は部品が決める | タブの選択中など |

遅れは `style="--d: .4s"`（入るときだけ効く）。手順の中で順に出したい行に付ける。

### 動かさない（`prefers-reduced-motion: reduce`）と JS が無いとき

- reduce: 手順ごとにステージを写して、その手順の状態で止めた図を手順の中に置く（**静止した図の並び**）。
  打鍵は打ち終わった姿、ポインターは押す位置に止まる。自動再生は最後の手順で止まる
- JS が無い: `data-motion` が付かないので、`data-at` の要素は全部見えたまま
- ステージは飾り（`aria-hidden` + `inert`）。**意味は必ず手順の文章で伝える**（図だけに情報を置かない）

## 4. 新しい場面の足し方（S2 の手順）

1. 事実を集める: 台帳（下の 7 節）・`.agent/requirements.md` の該当 FR・実際の UI 文言（`crates/tako-app/src/ui_text/*.rs`）。
   **画面の文言は本体の文言をそのまま写す**（「コピー」「新規ペインで実行」など）
2. `docs/src/components/scenes/Scene<名前>.astro` を作る。下のひな形をコピーして、ステージと手順を書く
3. ページ（`.mdx`）から `import Scene… from '…/components/scenes/Scene….astro';` して `<Scene… />` を 1 行置く。
   `.md` のページは `.mdx` へ改名する（`git mv`。URL は変わらない）
4. 検証（5 節）

```astro
---
import Scrolly from '../motion/Scrolly.astro';
import Step from '../motion/Step.astro';
import TakoWindow from '../motion/TakoWindow.astro';
import Split from '../motion/Split.astro';
import Pane from '../motion/Pane.astro';
import Pointer from '../motion/Pointer.astro';
---

<Scrolly label="（何の流れか。読み上げ用）">
	<TakoWindow slot="stage" tabs={[{ label: 'my-app', state: 'running', active: true }]} height={25}>
		<Split>
			<Pane n={1} title="my-app" state="running" focus>
				<span class="tk-l"><span class="tk-p">my-app ❯</span> <span data-at="1+" data-fx="type">tako master</span></span>
				<span class="tk-l tk-tui-msg" data-at="2+">2 手順目から出る行</span>
			</Pane>
			<Pane n={2} title="worker" at="2+" fx="split">…</Pane>
		</Split>
		<Pointer slot="overlay" path="2:70,40 3:35,80" click="3" />
	</TakoWindow>

	<Step title="（手順 1 の見出し）">
		<p>本文。<code>コマンド</code> や <strong>強調</strong> はそのまま書ける。</p>
	</Step>
	<Step title="（手順 2 の見出し）"><p>…</p></Step>
</Scrolly>
```

書くときの決まり:

- **ステージの高さは手順で変えない**（`height` を固定。中身は `.tk-stack` で重ねて入れ替える）。高さが変わると貼り付いた図が跳ねる
- 1 場面の手順は 3〜5（多くても 7）。1 手順 = 画面の変化 1 つ
- 行は 1 行 1 要素（`<span class="tk-l">`）。**MDX に長い JSX を直書きしない**（行の間の文字が Markdown として解釈される）。場面は必ず `.astro` に閉じ込める
- 絵文字を使わない（`✓` などの文字記号は可。印は `Icon` で描く）。案内するコマンドは最も簡単な形（`tako master` に既定の引数を付けない）
- 実在の名前・パス・ホスト名を書かない。プレースホルダは `my-app` / `my-mac` / `winuser` / `~`、版は `vX.Y.Z`
- Windows で pending のものを見せる場面は、手順の本文の末尾に `<p class="tk-step-note">…</p>` で正直に書く
- ポインターの座標は部品の左上 0,0・右下 100,100 の %。撮って目で合わせる（5 節の撮影スクリプト）

## 5. 検証（S2 の PR で必ずやる）

```sh
cd docs && npm ci && npm run build && npm run og:verify && node scripts/verify-links.mjs && node scripts/verify-seo.mjs
```

- Playwright（chromium・headless のみ。WebKit は使わない）で 1440px / 375px × 動きあり / reduce を撮る。
  見るもの: 横スクロールが出ない（`scrollWidth - clientWidth = 0`）・手順ごとに図が変わる・静止図で同じ内容が読める・
  ダーク / ライト。書き出し先は自分の一時ディレクトリだけ
- 改題したら OG 画像を作り直す（`npm run og`。生成物をコミット）
- 数字を名乗るときは番犬に注意: トップの「162 個」「95 個」は `crates/tako-control/tests/docs_tool_inventory.rs` が
  **index.mdx の生の文字列**で拘束している（書き方を変えると cargo test が落ちる）

## 6. 部品の一覧（`docs/src/components/motion/`）

| 部品 | 置き場 | 主な props | 見た目の正本 |
|---|---|---|---|
| `Scrolly` | ページ | `label`・`flip`（図を左へ）。slot `stage` + 子に `Step` | — |
| `Step` | Scrolly の子 | `title`。本文は子 | — |
| `Autoplay` | ページ | `steps`・`interval`（ms） | — |
| `TakoWindow` | stage | `os`（mac / windows）・`tabs`（`label` / `state` / `active` / `at` / `activeAt`）・`guiAt`（表示モードの印）・`masterAt` / `workers`（ステータスバーの master 表示）・`cwd`・`branch`・`height`。slot: 既定（本体）/ `sidebar` / `banner` / `right` / `overlay` | `tab_bar.rs` / `menu_bar.rs` / `status_bar.rs` / `theme.rs` |
| `Split` | 本体 | `dir`（row / col）・`grow`・`at` / `fx` | — |
| `Pane` | Split の子 | `n`・`title`・`badge`（ORCH / WORKER）・`parent`（↳ master）・`state`（idle / running か `[{at, state, text}]`）・`focus`（true か data-at）・`at` / `fx`・`grow`・`body`（flush で余白なし） | `main.rs` の render_pane_header（状態の表記は running / idle の 2 つ） |
| `Sidebar` | slot sidebar | `project`・`branch`・`path`・`items` | `sidebar.rs` |
| `RightPanel` | slot right | `active`（fleet / orch / git / tasks）・`count` / `countAt`・`width` | `right_panel.rs` |
| `TaskCard` | RightPanel の中 | `kind`・`title`・`openAt`・`body`・`attachment`・`pick` / `pickAt`・`comment` / `commentAt`・`delivered` | `tasks_panel.rs` / `ui_text/panel.rs` |
| `CommandCard` | ペインの帯（`.tk-cardband`） | `title`・`command`・`at`・`running`・`ran` | `command_card_ui.rs` / `ui_text/command_card.rs` |
| `CommandCardFull` | ペインの帯（`.tk-cardband`） | `title`・`commands`（各 `text`（改行可）/ `copied` / `running` / `ran` + `code`（0 以外は赤）/ `closed` / `copyMark` / `runMark` / `boxMark`）・`at`。実物どおり番号・実行記録をボタン行の左に出す（#2057） | `command_card_ui.rs` の render_command_card / `ui_text/command_card.rs` |
| `PhoneCommandCards` | Phone の中 | `label`・`command`・`pressAt`・`confirmAt` / `confirmPressAt`（全文の確認シート）・`startedAt`・`runningAt` / `ranAt` / `code`・`pane`・`host`（#2057） | `web/tako-remote/src/components/command-cards.jsx` |
| `WelcomeBanner` | slot banner | `at`・`pressAt` | `overlays.rs` / `ui_text/welcome.rs` |
| `Chat` | Pane の中 | `typed` / `typedAt`・`chips`。吹き出しは子に `.tk-bubble`（`.is-user`） | `chat_view.rs` / `ui_text/ui_mode.rs` |
| `Starter` | Pane の中 | — | `starter.rs` / `ui_text/ui_mode.rs` |
| `Phone` | stage | `machine`・`tasks` / `tasksAt`・`height`。画面の入れ替えは子を `.tk-phone-layer` で重ねる | `web/tako-remote/src/styles/main.css` |
| `PhoneCard` | Phone の中 | `title`・`role`・`state` / `stateText`。子 = スニペット 1 行 | `web/tako-remote/src/pages/panes.jsx` |
| `OsTerminal` | stage | `os`・`title`・`height` | — |
| `Releases` | stage | `highlight`（mac / win-exe / win-zip）・`highlightAt` | 配布物の命名規約 |
| `SmartScreen` | stage | `detailAt`（「詳細情報」を押した後の手順） | — |
| `Installer` | stage | `tasksAt`・`installAt`（省略可）・`doneAt` | `installer/windows/tako.iss` |
| `Pointer` | slot overlay | `path`（`手順:横%,縦%` を空白区切り。`手順:-` でその手順から隠す）・`click` | — |
| `Cmd` | 手順の本文・ページ本文 | `prompt`（既定 `$`）。子 = コマンド 1 行。コピーのボタン付き | — |
| `WindowsStatus` | ページ | `variant`（full / compact） | `windows-support.md`（生成物） |
| `WindowsLimits` | ページ | `items`（`kind` / `title` / `text` / `tools`）。挙げた機能が「対応」になるとビルドが落ちる | 同上 |
| `Icon` | どこでも | `name`（`icons.ts` の一覧） | `file_icons.rs` |

行の色のクラス（`.tk-l` の中）: `tk-p`（プロンプト）/ `tk-c` / `tk-o` / `tk-dim` / `tk-ok` / `tk-warn` / `tk-err` / `tk-acc` / `tk-b`（太字）/
`tk-cur`（カーソル）。AI の TUI: `tk-tui-msg`（● 付きの発話。`is-tool` で緑）/ `tk-tui-user`（自分の発言）/ `tk-tui-input`（入力欄）。
重ねる器: `.tk-stack`（子が同じ場所に重なる）+ `.tk-fill`（高さいっぱい）。ペインの中を「端末 + 下端の帯」に分けるとき: `.tk-pane-col` > `.tk-pane-term` + 帯。

## 7. S2 の機能ページの構成案

土台は master が作った事実の台帳（作業ツリー外の `2053-facts.md`。節番号はそれに合わせた）。各ページ 1 PR。
**既存ページを作り直すときは、他ページから張られたアンカー（`#見出し`）を残す**（`verify-links` が落とす）。
各節の「書いてはいけないこと」を守る（例: 裏で起きた子プロセスを自動でペインに出す FR-2.18 は未着手、
初回起動での setup の自動実行 #1510 は未着地、agy は master にならない）。

| 優先 | ページ（slug） | 主題 | 見せる場面の並び | 使う部品（足すもの） |
|---|---|---|---|---|
| 1 | セットアップと基本の使い方（`getting-started/quickstart` を作り直し） | 入れたあと 10 分で「AI に画面を任せる」まで | ① バナー →「setup」タブ → `[OK]` が積まれる →「残り N 件」→ 再実行で素通り ② タブ = 作業グループ、分割・フォーカス・リサイズ（macOS / Windows のキーを併記）③ AI に「隣で dev サーバー」→ 同じタブに分割 → ポート検知のチップ（承諾したときだけ開く）④ 右パネルの fleet で全タブを見渡す | TakoWindow / Pane / Split / WelcomeBanner / RightPanel / Pointer / Cmd（**足す**: fleet の一覧・ポートのチップ・キーの表示） |
| 2 | コマンドカード（新設 `features/command-card`） | AI が打ってほしいコマンドを、壊れない形で渡す | ① 画面から選んでコピーすると折り返しの改行が混ざる（比較）② カードの帯が開く（会話に重ならない）③ コピー →「コピーしました」④ 新規ペインで実行 → 実行中 → 実行済み（終了コード 0）⑤ 複数コマンド（N/M 件目）⑥ スマホからも実行 | CommandCard（**足す**: 複数行・失敗の表示）/ Phone |
| 3 | 人がやること（`features/user-tasks` を作り直し） | AI の「確認お願いします」を待ち行列にする | ① 起票 → 通知の 1 行 → tasks の件数 ② 一覧（種類で絞り込み・完了も見る）③ 詳細（本文・添付のサムネイル・コピー用のテキスト）④ 判断を選んで返す ⑤ 配送の 4 状態（送信済み / 届いた / 起動した / 届かなかった）⑥ スマホの `#/tasks` | RightPanel / TaskCard（**足す**: 絞り込みのチップ・コピー用テキスト・やりとりの履歴）/ Phone。**Windows は pending と明記** |
| 4 | オーケストレーション（`features/orchestration` を作り直し） | 頼むのは 1 回、AI がチームで進める | ① `tako master` → 依頼 ② 分担を提示 → worker 1 体（右全面）→ 2 体（上下）→ 4 体（十字）③ 確認待ちのダイアログに master が答える ④ 検収 → 片付け → 詰め直し ⑤ orch ビューのツリー ⑥ 自動ハンドオフ（claude のみ）。**今の ASCII 図（横 3 列）は既定のレイアウトと違うので差し替える** | TakoWindow / Split / Pane（**足す**: orch ビューのツリー）。トップの `SceneOrchestration` は流用してよい |
| 5 | かんたん表示（`features/gui-mode` を作り直し） | 黒い画面が苦手でもボタンとチャットで | ① モードボタン ② スターター 3 枚 →「AI チームに任せる」→ 準備中 → チャット ③ 生成中の「考え中…」と送信待ち ④ 発話のコピー ⑤ ターミナルへ戻す（同じ画面がある） | Chat / Starter（**足す**: 準備中・生成中の表示）/ Pointer。チャットになるのは claude だけと明記 |
| 6 | リモート（`features/remote` を作り直し） | 外出先からスマホで続きを / SSH 先で作業 | (A) ① `tako remote setup` の 5 段 ② `tako remote start` → QR ③ ペアリングの 4 権限 → Mac で承認 ④ ペイン一覧 → チャット → コマンドカードの実行 ⑤「+」の起動シート ⑥ ファイル。(B) SSH ホスト一覧 → SSH ペイン → リモートのフォルダをツリーへ | Phone / PhoneCard / OsTerminal（**足す**: QR・ペアリング画面・Mac の承認ダイアログ・起動シート）。**Windows は pending と明記** |
| 7 | ファイルツリー・プレビュー・エディタ（`features/file-preview` を作り直し） | 開く・読む・直すがペインの中で | ① ツリーから開く → プレビュー ② 編集 → 自動保存 ③ 言語サーバの取得 → 診断の波線 → 補完 → 定義へ ④ 横断検索 | Sidebar / Pane（**足す**: コードのプレビュー・補完の一覧）。**Windows の LSP は pending** |
| 8 | 閉じても消えない（`features/tmux-backend` を作り直し） | 再起動しても作業が戻る | ① 終了 → 再起動で同じタブ・同じ分割 ② PC 再起動後に会話を resume（Windows は claude のみ）③ `tako recover` | TakoWindow（**足す**: 再起動の暗転の演出） |
| 9 | その他（新設か既存ページに 1 場面） | Code Runner・使用制限後の自動復帰・tako mod・テーマ・たまり場・git パネル | 各 2〜3 手順の小さな場面 | 既存の部品 + 必要な小部品 |

トップの場面（`components/scenes/Scene*.astro`）は、機能ページの冒頭の「まず全体像」として流用してよい。
機能ページでは、トップより一段深く（例外の扱い・CLI / MCP での同じ操作・Windows の差）まで見せる。

## 8. 残り・S3 へ

- トップのシミュレーション上の FCP（+1.4 秒）を詰めるなら、モーション用 CSS（gzip 8KB）の分割か、ヒーロー以外の部品の CSS の遅延読み込み
- 日本語フォント（Noto Sans JP）の分割ファイルは本文の字の種類で増える。サイト全体でサブセット化する余地がある（S3）
- `README.md` の「移植は途中です」（Windows）はサイトの外なので S1 では直していない
- splash のトップは Starlight の仕様でスマホ幅のメニューが無い（ヒーローのボタンで導線は確保）
