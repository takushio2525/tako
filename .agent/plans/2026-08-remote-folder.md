# リモートからフォルダを開く（#919 / #65）— 設計と実測の記録

> 2026-08-24。`feat/919-open-remote-folder` の作業記録。
> 仕様の正は `.agent/requirements.md` FR-3.24 / FR-3.24.1。使い方は `AGENTS.md` のコマンド表。

## 1. 出発点: ユーザー報告の正体

> 今，ファイルから，リモート接続ってとこ押して出てきたタブ，なんか何も入力できないけどそういうもん？

**当たりはユーザーの追加観察のほう**（「接続失敗しても何も出てこない」）だった。
現行「リモート接続…」は `ssh` を**素のペインのプログラム**にしていたため:

| ケース | before の実測 |
|---|---|
| 到達不能（名前解決できない） | `open-in remote` は tab/pane を返すが **1 秒後にはタブごと消えている**（`tabs=['1'] panes=[]`）。`Could not resolve hostname` はどこにも残らない |
| タイムアウト（TCP ブラックホール） | タブは残るが **25 秒間画面に文字が 1 つも無い**。ssh の既定 `ConnectTimeout` は約 75 秒 = 「何も入力できない」の正体 |
| 認証失敗 | `password:` プロンプトのみ（鍵認証が落ちたことは分からない） |
| **happy path** | **正常**（PowerShell プロンプトが出て `send` も通る）= ここは壊れていなかった |

再現手順（隔離のみ・本番に触れない）は `/tmp/tako919-before2.sh` 相当:
`TAKO_ISOLATED=1` の tako-app を起こし、`tako open-in remote <host>` のあと
`tako list` でタブの生死、`tako read --pane N` で画面の非空行数を数える。
**「非空行数 0」を明示的に判定する**のが要点（`read` が空文字を返すのを
「読めなかった」と読み間違えると原因を取り違える）。

## 2. バックエンドの選定（#65 の宿題への回答）

3 案を比較して **システムの `ssh` / `sftp` + ControlMaster** を採った。

| 案 | 認証の再利用 | 依存 | Windows | 判定 |
|---|---|---|---|---|
| システムの ssh / sftp | `~/.ssh/config` / 鍵 / agent / known_hosts / 2FA / FIDO / ProxyJump / **ControlMaster** をそのまま | なし（OS 同梱） | 10 以降が OpenSSH クライアント同梱 | **採用** |
| russh | 自前で作り直し。**ControlMaster には相乗りできない** | pure Rust | ○ | 却下 |
| ssh2（libssh2） | 同上 | libssh2 + OpenSSL の C 依存 | クロスビルドが重い（#467） | 却下 |

決め手は #919 要件 6 / #65 要件 1 の「**ControlMaster 共有で追加認証なし**」。
ControlMaster は OpenSSH クライアント間の私的な多重化なので、crate からは原理的に
相乗りできない。crate を採ると「ユーザーが `ssh <host>` で入れる先に tako だけ
入れない」状態が残る。`git.rs` / `tmux.rs` が CLI を子プロセスで呼ぶのと同じ構え。

**FUSE マウントには逃げていない**（#65 の方針）。tako が SFTP プロトコルを話す
クライアントを駆動し、ツリー・プレビュー・キャッシュを自分で持つ。

## 3. 実測で決めた仕様（推測でなく計測）

すべて OpenSSH 10.2p1 / macOS 26 と、実ホスト 2 台（Linux = `cloud-computing-class`、
Windows 11 + PowerShell = `win`）で確かめた。

- **`sftp -b -` はログインシェルに依存しない**: `win` の既定シェルは PowerShell だが
  `ls -la` / `get` がそのまま通る。Windows のドライブは `/C:/Users/...` の形で見える
- **ControlMaster は `sftp` では作られない**: `-o ControlMaster=auto` を渡しても
  ソケットができない。**明示的に `ssh -M -N -f`** で張ると以後 `sftp` が相乗りする
  （0.603s vs 1.102s。速さより **再認証が起きない**ことが本題）
- **`-o ControlPath=<空白入り>` は OpenSSH の設定パーサが空白で切る** →
  `keyword controlpath extra arguments at end of line` で全操作が失敗する。
  macOS の既定 data_dir は `Application Support` を含むので**既定構成で必ず踏む**
  （#833 と同型）。値を二重引用符で包めば通る
- **sftp は二重引用符の中で glob 展開をしない**: `ls "…/.b*"` は `.b*` を literal と
  して扱い not found。空白・`*`・`?`・`[` 入りのパスは引用だけで安全に渡せる
- **`ls -l` をファイルに掛けるとフルパスが返る**（nlink は `?`）。ディレクトリなら
  basename。**Windows は owner / group が `-`、権限部が `*`、名前に空白が入る**
  （`Application Data`）→ 列を自前で辿って末尾を名前にする
- **`-q` でも `sftp> <cmd>` のエコーは消えない** → 剥がしてから解析する
- **symlink の実体判定は末尾スラッシュで効く**: `ls -1 <link>/` はディレクトリなら
  中身、ファイルなら `Can't ls: … not found`（バッチへ `-` 前置でまとめて投げる）
- **ssh 自身の失敗は exit 255**（man 記載）。リモートシェルの `exit 1` と区別できる

## 4. 静かな失敗を作らない構造

- `RemoteError`（13 種別）が**日英で**「何が起きたか」「次に何をすべきか」を持つ。
  握り潰しても空にならないことを、全種別 × 両言語の総当たりテストで固定
- ツリーの読み込み失敗は**行として**出る（`RowNote::Error`。3 行を折り返して出す）
- 接続・一覧の失敗はサイドバー上部の通知へ。**失敗は自動で消さない**（成功は 8 秒）
- SSH ペインは `ssh_pane_script` で包み、接続前バナー + `ConnectTimeout=10` +
  exit 255 のときだけ理由を出して入力待ち（成功して `exit` したら従来どおり閉じる）
- 「読んでいない」を「失敗」と混同しない: `sidebar_closed` / `not_displayed` /
  `pending` / `loading` / `loaded` / `error: <理由>` を状態として区別する

## 5. 段階の切り方

- **段階 1（この Issue）**: 閲覧 + プレビュー。**編集は構造的に禁じる**
  （本体は `<data_dir>/remote-cache/` のローカルな写しなので、止めないと
  「保存できた気になる」= リモートには何も書かれない）
- **段階 2（別 Issue）**: 書き戻し（SFTP put）。ヘッダの「読み取り専用」表示と
  `set_preview_editing_local` のガードを外す形になる
- リモートツリーの**ポーリングはしない**（展開したときだけ読む）。ネットワーク I/O を
  毎秒叩かないため。手動の再読み込みは右クリックから

## 6. この機での検証手段（画面が撮れない）

clamshell 閉 + 画面 OFF なので `screencapture` は**全面黒しか撮れない**（#828 の既知）。
代わりに:

- **セルフテスト項目 122**: 実 render でツリーの器を検査（ネットワーク非依存）。
  (a) ルート行 + 読み込み中 (b) 中身の行が**全部 remote 印を持つ**（ローカル FS の
  経路へ落ちない）(c) 失敗が理由つきの行 (d) 空ディレクトリ (e) 通知の期限
  (f) フォルダ選択がフォルダだけ並べる (g) **リモートは編集できずローカルは編集できる**
- **visual-test の `remote-tree` 節**: 実ピクセル。`changed` / `red_before` /
  `red_after` を出し、`TAKO_VISUAL_DUMP=<path>` でサイドバーを切り出して**目視できる**
- **`remote_fs_e2e --ignored`**: 実 SSH 先との通し（`TAKO_REMOTE_E2E_HOST=<host>`）

### 検出力の実測（3 通りの revert で FAILED を確認）

| 壊した箇所 | 落ちる検査 | 出た値 |
|---|---|---|
| 読み取り専用ガードを外す | 122 (g) | `blocked=false` |
| 失敗を行として出さない | 122 (c) | `rows=0 reason=false` |
| リモート行の `remote` 印を落とす | 122 (b) | `all_remote=false` |

## 7. after の実測（受け入れの証拠）

- **無言失敗の解消**: 到達不能 = タブが残り 7 行の理由 / タイムアウト = t=2s で
  「接続しています…」・10 秒で理由 / 認証失敗 = 「先に SSH ペインでログイン」まで案内
- **フォルダを開く**: `win:/C:/Users/<user>/dev` を開いて 117 件、`pending` →
  `loaded` が約 2 秒。Linux ホスト（`/` = symlink 混在）も同様
- **SSH ペイン導線**: `cd "C:/Users/<user>/dev"` が実行され PowerShell が移動
  （`shell_path` が `/C:/…` の先頭スラッシュを落とす）
- **MCP 1:1**: 138 ツール。`open` / `ls` / `list` / `open-file`（`read_only: true`）/
  `ssh-pane` / `close` すべて実行。失敗は `isError` + 理由つき
- **永続化**: layout.json に `remote_folders` が載り、再起動後に自動で読み込まれる
- **回帰なし**: visual-test 98 checkpoint が main と**完全一致**（差は md の load ms のみ）

## 8. 踏んだ罠（後続への申し送り）

- **`s.index('"close" => {')` のような索引置換は同名の match アームを壊す**。
  1 回やって無関係な箇所を破壊した（`git checkout` で復旧）。**一意なアンカー**
  （前後 2 行つき）で置換し、`git diff --stat` で行数を必ず確認する
- **`#[cfg(...)]` の直下へ関数を挿すと属性が新しい関数へ移る**。visual-test の
  ヘルパが「見つからない」になったのはこれ。挿入後は**両方の feature でビルド**する
- **セルフテストは「先に別の理由で落ちる」形になりやすい**。項目 122 (g) は最初
  「プレビューペインではない」で落ちていて、ガードを外しても通る = 検出力ゼロだった。
  **対照（ローカルなら編集できる）を同じテストに入れる**と気づける
- **実測から採った fixture には相手のユーザー名がそのまま入る**。`ls -la` の出力を
  そのままテストへ貼ったので、コミット前に `user` へ置換した（グローバル CLAUDE.md の
  「個人情報のコミット禁止」。owner / group 列の幅は解析に効かないので置換して問題ない）
- `crates/tako-control/src/claude_tui.rs` に**main 由来**の実ユーザー名が 2 箇所残っている
  （テストの fixture パスと、その文字列を `contains` する判定）。#919 の射程外なので
  触っていない = 別途起票が要る

## 9. 未検証・既知の限界

- **Windows 実機で 1 度も測っていない**（設計上は同梱の OpenSSH で動くが、
  ControlMaster のソケット・`ControlPath` の引用・PowerShell 版 `ssh_pane_script` は
  未実測）。対応マトリクスは `tako_remote_folder` を **Pending / issue 919** で登録
- **パスワード認証しか無い相手での通し**は未実測（鍵認証で入れるホストしか無い）。
  設計は「対話 SSH ペインで一度ログイン → 同じ ControlPath を共有」で、
  ペイン側の argv がツリーと同じ ControlPath を通ることはユニットテストで固定
- **実 IME・実マウスでのリモート行の右クリック**は未検証（画面 OFF）
- リモートの **git status / 検索 / D&D** は対象外（ローカル FS 前提の機能なので
  リモート行では出さない）

## 10. 後続への申し送り（#916 worker からの調整）

- `<data_dir>/ssh/`（ControlMaster ソケット）と `<data_dir>/remote-cache/`（SFTP で
  落としたリモートファイル）は #513 のカタログへ **`Class::Local`** で宣言済み。
  #916 の被覆テスト（`共有される設定は移行の番地にも載っている`）は `Class::Shared`
  だけを見るので何も要求しない（実測で確認済み）
- **`remote-cache/` の中身は SSH 先のファイルの写し**。将来この番地を移行機構へ載せる
  ことがあれば **`preserve_unreadable: false`** 側（退避 = `.unreadable.bak` への写しも
  作らない）。tako が作り直せる短命なキャッシュであり、かつ相手のソースを含むため
- `remote_fs` は **`config_io` を通らないので `.lock` を作らない**。後で通す経路を足す
  なら「**書くと決まってからロックを取る**」（`.agent/conventions.md` の該当節。
  無条件に取ると最新のときも空ロックが増える）
- **#915（PR #922）と項目番号がぶつかった**: セルフテストの項目 122 を両方が同じ場所へ
  足していたので、#919 のぶんを **123** へ繰り下げた。MCP のツール数は
  137 → 138（#915）→ **139**（#919）

---

## 11. 段階 2: 編集・保存（#966。2026-08-27）

§5 で「別 Issue」としていた書き戻しを実装した。**禁止を外す代わりに置いたもの**が本体で、
機能追加より安全設計のほうが分量が多い。仕様の正は `.agent/requirements.md` FR-3.24.2。

### 実測で決めた仕様（推測でなく計測。OpenSSH 10.2p1 / Linux + Windows 11 の 2 台）

- **`rename` は既存ファイルを上書きする**（`posix-rename@openssh.com`）。Linux も
  **Windows の sftp-server も**上書きした（`put b.txt <tmp>` → `rename <tmp> <target>` で
  中身が入れ替わることを両方で確認）。よって「一時ファイル + rename」で
  **途中で切れても元のファイルが壊れない**書き戻しが成立する
- **`put` は元の mode を引き継がない**: `-rwxr-xr-x` のファイルへ書き戻すと
  `-rw-r--r--` になる（実測）= **実行権が落ちる**。POSIX として読める mode なら
  書き戻し後に `chmod <8 進数>` で戻す
- **Windows は `chmod` が通っても効かない**（`-rw-******` のまま・exit 0）。
  権限欄に `*` が混ざる形は「判定材料が無い」として `chmod` を送らない。
  ただし**同じ相手でも `-rw-------` に見えることがある**（作られ方で変わる）ので、
  `*` の有無で分岐する形にしてある
- **`ls -la` の日時は分の分解能**（`Aug 27 14:58`）。サイズと mtime だけで競合を見ると
  **同じ分・同じサイズの書き換えを見逃す** → 競合検知は**内容そのもの**を突き合わせる
  （e2e はまさに「5 バイト → 5 バイトの書き換え」で見抜けることを測っている）
- **`sftp -b` は最初の失敗でバッチを打ち切り exit 1**（`-` 前置のコマンドは打ち切らない）。
  権限が無い `put` は `dest open "...": Permission denied` → `permission_denied` へ分類、
  消えたファイルの `get` は `File "..." not found.` → `not_found` へ分類される
- **rename は同一ファイルシステム内でしか成立しない** → 一時ファイルは**対象と同じ
  ディレクトリ**へ置く（`/tmp` 経由にはできない）。ドット始まりにするので
  ツリーの既定表示（#550）には出ない

### 同期と非同期を分けた理由

1 バッチが **ControlMaster に相乗りしても 1〜2 秒**（Azure の相手で実測 0.92 / 2.08 / 1.98 秒）。
保存は 3 バッチ（素性 → 内容の突き合わせ → 書き戻し）なので、⌘S ごとに UI スレッドで
待つと #212 / #772 と同じ体感の悪化になる。

- **GUI（⌘S / 保存ボタン / 自動保存）**: ローカルの写しへ同期で書く → リモートへは背景。
  進行中にもう一度保存されたら終わってから 1 回だけ押し直す（積み上げない）
- **dispatch（CLI / MCP）**: 同期。#919 の `open` / `ls` / `open-file` と同じ
  「明示的に起こした 1 回の操作」で、**AI が応答だけで「リモートへ書けたか」を
  判断できる**ことのほうが重要。`ControlHost::save_preview` は `cx` を持たないので
  そもそも背景へ投げられない（構造がそうなっている）

### 検出力の実測（revert で FAILED を確認）

| 壊した箇所 | 落ちる検査 | 出た値 |
|---|---|---|
| 段階 1 へ戻す（`TAKO_966_LEGACY=1`） | セルフテスト 123 (g) | `editable=false blocked=true` |
| `save_preview` から押し出しを外す | セルフテスト 123 (g2) | `failed=false listed=false body=false retry=false` |
| （e2e）競合検知を通さない | `開いた後にリモートが変わっていたら上書きしない` | 同サイズの書き換えを見逃す |

### 後続への申し送り

- **TOCTOU の窓は残っている**: 「内容を確かめる」と「書き戻す」の間に相手が書くと
  上書きしてしまう。SFTP に atomic compare-and-swap が無いので構造的に閉じられない
  （VSCode / Zed も同じ）。窓は 1 バッチぶん（1〜2 秒）
- **`remote-cache/` は掃除していない**（段階 1 から）。段階 2 で
  `<写し>.tako-base`（開いた時点の内容）が増えたので**1 ファイルあたり 2 倍**になる。
  退避（`pending/`）はユーザーの未送信データなので**閉じても消さない**設計
- **退避の一覧に「読めない断片」を出す**形にしてある（`.json` が壊れていても
  `.body` は残るので、行として見せて手で救える）
- 段階 3 の候補: 競合したときの**差分表示と 3 方向マージ**（いまは「読み直す」か
  「上書き」の二択）。リモート側のファイル作成・削除・リネーム（いまは読み書きだけ）

---

# 段階 3: ペインの `ssh` を自動検知する（#976 / #65 要件 1）

> 2026-08-27。`feat/976-ssh-auto-detect` の作業記録。仕様の正は
> `.agent/requirements.md` FR-3.24.3。

## 11. 何が新規で、何を使い回したか

基盤（`remote_fs` の SFTP 経路・ツリー・プレビュー・編集保存 #966）は完成済みだったので、
新規は**検知**と**見た目の統合**の 2 つだけ。

| 層 | 置いたもの |
|---|---|
| `tako_core::ssh_detect` | コマンド行 → 宛先（純関数。**両プラットフォームぶんを macOS からテストできる**） |
| `tako_control::ssh_detect` | 「どのペインの配下か」+ **再走査の間引き** |
| `tako-app::ssh_folders` | 追加してよいかの判断・background の接続確認・切断の見せ方 |
| `dispatch::attach_remote_root` | `open` から**器づけだけ**を切り出し、自動追加と共有 |

## 12. 検知の設計（推測ではなく既存の実測に乗せた）

- **`ps` を増やさない**: 親子マップは `agents::process_parent_map` が既に 1 回だけ
  採っていたので、同じ `ps` へ `command=` の列を足して argv も一緒に採る形へ変えた
  （`capture_process_table`）。番犬（`コンソール窓を抑止していない子プロセス起動が
  増えていない`）のベースラインが agents.rs = 1 のままで済む
- **間引きの指紋は OSC 133**: 対話 `ssh` はフォアグラウンドのコマンドなので
  `Idle → Running`（入った）/ `Running → Idle`（抜けた）が必ず出る。指紋 =
  (ペイン集合, PTY 直下の子 pid, コマンド状態) にすると、**検知も切断も指紋の変化で拾え**、
  ssh が生きている間・全ペインが idle の間は走査そのものが起きない
- **見送る側に倒す**: 宛先を取り違えると別のマシンの中身をそのホスト名で見せてしまう。
  `-p` / `-o Port=` / `-J` / `-W` / `-o ProxyJump=` / `-o ProxyCommand=` /
  `-o Hostname=` / `ssh host <コマンド>` / `-N` / `-s` は全部見送り、
  理由（`SkipReason` 7 種別・日英）を `auto` の応答と `persist.log` に残す。
  `-l user` / `-o User=` だけは `user@host` へ**畳み込む**（見送るより忠実）
- **`-N` を弾くのが効く場面**: tako 自身の ControlMaster（`ssh -M -N -f`）はペイン配下に
  居ないが、ユーザーの `ssh -N -L 8080:…` はペイン配下に居る。転送だけの接続で
  フォルダを開かない
- **入れ子は外側**: `descendants_with_root` を幅優先にして、`ssh a` の中で `ssh b` した
  ときに外側（このマシンから届く方）を採る

## 13. 見た目の統合（#919 の独自形をやめた）

| | #919（前） | #976（後） |
|---|---|---|
| 並び | リモートを**先頭へ hoist** | ローカルの後ろに普通に並ぶ |
| ルート名 | `host: 末尾要素` | 末尾要素だけ（ローカルと同じ） |
| アイコン | 地球（mauve） | フォルダ（accent。ローカルと同じ） |
| ホスト名 | 名前に混ぜる | **行末のバッジ**（`SSH <host>`。絵文字なし = SVG マスク + テキスト） |
| 切断 | 表示なし | 同じバッジが赤 + 「切断」（**行は消さない**） |

`FileTree::add_remote_root` を `insert(0)` → `push` にしたので、リモートも
**開いた順**に並ぶ（ローカルルートの「並んだ順」と同じ規則）。タブ側
（`Tab::add_remote_folder` = layout.json の順）は触っていないので永続化は不変。

## 14. 安全側の判断（実装で決めたこと）

- **パスワードを聞きに行かない**: 接続は `ensure_master`（`BatchMode=yes`）のまま。
  鍵・agent で入れない相手は理由を出して見送る。ただし**FIDO 鍵のタッチ要求は
  BatchMode でも起こりうる**（ssh-agent 越しの通常運用では起きない）。気になるなら
  `tako remote-folder auto off`
- **試行は 1 エピソード 1 回**。失敗して繰り返し `ssh` を撃たない。抜けて入り直したら再試行
- **同じホストのルートがそのタブにあれば何もしない**（明示経路と共存。「リモート接続…」の
  隣に home が二重に並ばない）
- **設定を切っても開いたルートは閉じない**（ユーザーのワークスペースなので勝手に消さない）
- 自動処理の失敗は**画面を奪わない**（期限つき通知 + `persist.log`。エラー通知にすると
  サイドバーを勝手に開いて居座る）

## 15. 検証の手段（この機は画面が撮れない）

- **セルフテスト項目 130**: プロセス表を `ProcessSnapshot::from_parts_for_test` で組み、
  (a) オプトアウトで走査対象が空 (b) 検知して仕事が 1 件出る (c) 器づけ後に
  **ローカルの後ろ**に並び名前に `host: ` が混ざらない (d) 同じホストは二重に並べない
  (e) 切断でルートが消えず状態が出る (f) `auto` が照会と切替を返す
- **visual-test `remote-tree`**: `TAKO_VISUAL_PIXEL: remote-badge …` に
  `order_ok` / `badge_live`（mauve の実ピクセル）/ `badge_lost_red`（切断で赤へ）/
  `rows_after_lost`（行が消えない）を出す
- **実 SSH**: `TAKO_REMOTE_E2E_HOST=<host> cargo test -p tako-core --test remote_fs_e2e -- --ignored`
  は基盤側。検知の通しは隔離 GUI + 実 `ssh` ペインで測る（下記 16）

## 16. 実測（受け入れの証拠）

### 実 SSH での通し（隔離インスタンス + 実ホスト = Windows 11 / PowerShell）

隔離起動（`TAKO_ISOLATED=1` + 明示の data / discovery / tmux socket。**persist ON** =
器つきの既定構成）で、CLI から `tako send --pane 1 "ssh <host>"` を打っただけの状態から:

| 測ったもの | 実測 |
|---|---|
| 検知（`auto` の `sessions` に出る） | **5 秒** |
| ツリーへ出る（`list` にルートが載る） | **12 秒** |
| 自動追加されたルート | `<host>:/C:/Users/<user>`（Windows のホーム。sftp の初期 cwd） |
| 中身 | 91 件（`list` の `entries`） |
| 切断の検知（`exit` から） | **約 8 秒**（`ssh` 送信から 20 秒） |
| 切断後のルート | **残る**（`state: loaded` / 91 件 / `sessions[].state: disconnected`） |
| `persist.log` | `ssh 自動追加: <host>:/C:/Users/<user> （91 件・追加=true）` / `ssh 切断を検知: <host>` |

`detection` は `pending`（起動直後・走査前）→ `active`（走査後）と動く。

### 自動追加されたルートの操作感（スコープ 3。実 SSH）

自動追加で出たルートの配下で、**展開 → プレビュー → 編集 → 保存**が #919 / #966 の経路
そのままで通ることを実ホストで確認した（リモートには一時ファイルを 1 つ作り最後に消した）:

| 段 | 実測 |
|---|---|
| 自動追加 | `AUTO_ROOT=/C:/Users/<user>`（**8 秒**） |
| 展開（`ls`） | 一時ファイルが見える |
| プレビュー（`open-file`） | `read_only: false` / `remote_path` / `mode: -rw-------` / `size: 17` |
| 編集 → 保存（`edit apply` → `edit save`） | `remote: {state: "saved", conflict_checked: true, bytes: 31, pending_write: false}` |
| リモートの実体 | `Get-Content` が編集後の内容を返す（`hello from 976 (edited by tako)`） |
| 後片付け | 一時ファイルを削除し `Test-Path` = False |

### アイドル時のコスト（受け入れ条件 6）

`ps` を PATH で中継して**採取回数そのもの**を数えた（`ps -axo pid=,ppid=,command=` =
`ProcessSnapshot` の採取だけを数える）。ペインはプロンプト待ち（OSC 133 = `idle`）:

| 窓 | ProcessSnapshot の採取 | プロセスの CPU 時間（別 run・180 秒窓） |
|---|---|---|
| 自動追加 **ON** | **6 回 / 120 秒** | 2.60 秒（1.44%） |
| 自動追加 **OFF** | **6 回 / 120 秒** | 2.50 秒（1.39%） |

採取回数が**同数** = 検知由来の増加はゼロ（6 回は sleep_guard #779 と stale binary #772 の
60 秒保険で、位相がずれて交互に出る既存分）。CPU の差 0.10 秒 / 180 秒（0.06 ポイント）は
計測ノイズの範囲（ON の窓が先で起動直後の一時的な作業を含む。`ps` を 1 回も余分に
起動していない事実と合わせて読む）。フレーム要求も増えない（`apply_ssh_scan` は
`cx.notify()` を呼ばず、通知するのは実際に追加・切断した瞬間だけ）。

### 見た目（実ピクセル。`TAKO_VISUAL_ONLY=remote-tree`）

```
TAKO_VISUAL_PIXEL: remote-badge order_ok=true local_y=254.0 remote_y=281.5
  badge_live=547 badge_lost_red=619 badge_live_after_lost=60 rows_after_lost=6
```

- `order_ok=true` / `local_y=254.0` → `remote_y=281.5`: リモートルートが
  **ローカルルートの 1 行下**に並ぶ（#919 は先頭へ hoist していた）
- `badge_live=547`: ルート行に mauve のバッジが実際に描かれている
- `badge_lost_red=619` / `badge_live_after_lost=60`: 切断でバッジが赤へ変わる
- `rows_after_lost=6`: 行数は変わらない（**消さない**）

切り出した画像は `TAKO_VISUAL_DUMP` で保存できる（ユーザー名が写るので**リポジトリ外**へ）。
実際の絵は「ローカルの見出し（フォルダ名 + フォルダアイコン）の下に、同じ形の
リモート見出し + 行末に `SSH <host>` バッジ」で、子の行はローカルと同じアイコン。

### セルフテスト（項目 130）

```
TAKO_SELF_TEST_976: legacy=false targets(off/on)=0/6 jobs=1 live=true after_local=true
  root_name="home" dup_jobs=0 rows_kept=true disconnected=true auto(on/off)=true/false sessions=true
```

`TAKO_APP_SELF_TEST_OK`（全項目完走）。skip 3 件は「ウィンドウが隠れて未描画」の既知。

### 検出力（同一バイナリの A/B）

| 壊した箇所 | 落ちる検査 | 出た値 |
|---|---|---|
| `TAKO_976_LEGACY=1`（検知しない） | 項目 130 (a) | `off=0 on=0` |
| `TAKO_976_LEGACY=1`（`host: ` 付きの旧ルート名。項目 124 を A/B 対応にする前） | 項目 124 (b) | `root_like_local=false` |
| ローカルルートを実在させないまま並びを測る（visual-test の場面不備） | `remote-badge` | `order_ok=false local_y=-1.0` |

## 18. visual-test 全節の状況（main 由来の失敗を切り分けた）

`TAKO_VISUAL_TEST=1`（全節）は **main でも** ちらつき節の
「出力中のペインの外側は前の絵へ戻らない（#932）」で落ちる。同一 worktree で
`git checkout origin/main -- crates/` して同じ節を回した A/B:

| | 実測 |
|---|---|
| main（`origin/main` の crates） | `output-running … reverted_tiles=5 spots=[(320,0),(320,32),(256,64),(288,64),(320,64)]` → FAILED |
| このブランチ | `output-running … reverted_tiles=7 spots=[(320,0),(352,0),(384,0),(384,32),(320,64),(352,64),(384,64)]` → FAILED |

`spots` はどちらも**タブバーの帯**（y=0〜64）で、#945 の「走り始めのドット脈動」が
反転タイルとして数えられている。件数の 5 / 7 は run ごとの揺れ（脈動の位相）で、
このブランチの変更はタブバーに触っていない = **main 由来**。

なお `TAKO_VISUAL_ONLY=flicker` 単独では `idle-4pane` は `distinct=1 changed=0`
（きれい）で、全節通しのときだけ汚れる: 前の節が残したペイン（7 枚）と出力が
「静止画面」の前提を崩している。これも main と同じ形。

## 17. 未検証・既知の限界

- **Windows は自動検知が働かない**: 境界（`platform::procinfo`）が実行ファイル名しか
  返さないので argv が無い。`auto` の応答は `detection: "unavailable"` を返すので
  「検知していない」と混同しない。対応マトリクスの注記へ明記済み
- **リモート側の cwd 追従は範囲外**（#65 要件 2）。開くのは常に sftp の初期 cwd
- シェル統合が効いていないペイン（`CommandState::Unknown`）は指紋が動かないので
  検知が最大 60 秒遅れる（保険の間隔）

---

## 17. ネット断からの自動復帰（#1040。2026-08-29）

### 17.1 「タブが閉じてリモートが消える」の正体

報告（v0.8.0）は**切断そのものではなく tako が出していた案内**だった。実測:

| 測ったもの | before |
|---|---|
| 切断でペインが消えるか | **消えない**（split / tab とも 31〜33 秒観測して `pane=True tab=True`）|
| 画面 | `Shared connection to … closed.` → `tako: … 接続に失敗しました（ssh exit 255）` → **`tako: Enter でこのペインを閉じます`** |
| **その Enter を打つと** | `before tabs=2 panes=2` → **`after tabs=1 panes=1`**。`tab` 経路はペイン 1 枚なので**タブごと消え、そのタブのリモートフォルダも消える** |
| 回線復帰後 | 何も起きない（自動再接続が無い）|
| `ssh_connect`（#1010）| 接続成立時に `Opened` で破棄 = **切断後は `null`** |

**もう 1 つの壊れ方**: ツリーでフォルダを開いていると ControlMaster が張られ、ペインの ssh は
その slave になる。ところが `ensure_master` に `ServerAlive*` が無かったため——

| 条件 | ブラックホール 66 秒の実測 |
|---|---|
| master 無し | 17 秒で検知 → exit 255 |
| **master 有り（普段の使い方）** | **66 秒経っても未検知**。ペインが無言で凍る |

`ServerAlive*` は**多重化の master 側の設定**で、相乗りしている slave は自分では送らない。

### 17.2 ネットワークを握る隔離ハーネス（本番に触れずに切断を作る）

実ホストでは「回線だけを落とす」ができない（`pfctl` は sudo 必須 / クライアントの kill は
**exit code が変わって別物**になる = スクリプトの `255` 判定を通らない）。そこで:

- 非 root の `sshd`（127.0.0.1:2222）+ 自前の TCP 中継（:2223 → :2222）+ 中継越しの host alias
- 切断は 2 種類: **ブラックホール**（中継を `SIGSTOP`）/ **硬切断**（中継を kill = RST）
- ssh / sftp の解決だけ検証用シムへ差し替え（`platform::exe::find` 経由）。
  **`~/.ssh/config` は触らない**（OpenSSH は `~` を `getpwuid()` で解決するので `HOME` では曲がらない）

**踏んだ罠**: 検証用シェルを `#!/bin/zsh` で書くと、**その層が tako の `ZDOTDIR` を消費して
復元する**ため内側の zsh へシェル統合が届かない（`state: unknown` / `exit_code: null` になり、
`pane` 経路の検知が測れない）。`#!/bin/sh` にすれば素通しになる。

### 17.3 設計（3 経路を 1 本の再接続へ寄せる）

切断後のペインは**どの経路でもローカルシェルのプロンプト**に居る。そこで
「`ssh …` の 1 行を #640 の送達確認つき経路で打ち直す」だけで 3 経路とも戻せる
（`pane` 経路が元々この形なので、`split` / `tab` をそこへ寄せた = スクリプトの尾を
`read` から `exec "${SHELL:-/bin/sh}" -l` へ変えた）。

**切断の見分けは文言だけでは決められない**（`exit` でも `Shared connection … closed.` が出る）。
根拠は 3 つ: スクリプトのマーカー行（exit 255 のときだけ出る）/ シェル統合の終了コード 255 /
統合が届かないペイン向けの保険（`BROKEN_LINK_PATTERNS` = 壊れたときにしか出ない行）。
**mux slave は master の `Timeout, …` を画面に出さない**ので、保険は非 mux のときだけ効く
（本番は統合があるので終了コードで決まる）。

### 17.4 after の実測（受け入れの証拠）

| 測ったもの | after |
|---|---|
| 切断中に Enter を 3 回 | `tabs=3 panes=3` → **`tabs=3 panes=3`**（消えない）|
| 切断の検知（硬切断） | **2 秒** |
| 切断の検知（ブラックホール・master 有り） | **20 秒**（before: 66 秒観測して未検知）|
| 回線復帰 → リモートのプロンプト | **split 2〜6 秒 / tab 5 秒 / pane 2 秒** |
| フォルダ + 保留の自動復帰 | **6 秒**（`pending=1 → 0` / `connected=False → True` / リモートの中身が実際に更新）|
| 上限に到達 | 6 回・181 秒で `gave_up` + `next_step`（ペインは残る）|
| A/B（`TAKO_1040_LEGACY=1`） | 同じ手順で **36 秒経っても `failed` のまま**再接続しない |

**実装で 1 度踏んだ**: 打ち直しの結果を `last_attempt_at.is_some()` で「結果待ち」と読むと、
失敗が確定したあとも結果待ちへ戻り、次の tick で「新しい行が無い = まだ分からない」と読んで
**結果待ちの上限（45 秒）まで固まる**（実測: 回線が戻っているのに復帰が 39 秒遅れた）。
`attempt_pending` を別に持って解決した。

**もう 1 度踏んだ**: 見張り（`Connected`）で `fresh_pane` の「画面全体を見る」近道を使うと、
**前回の切断マーカーが画面に残り続ける**ので繋ぎ直した直後に同じ行をもう一度「切断」と読む。
見張り以降は必ず起点（`baseline_index`）から先だけを見る。

---

## 18. #1041: 「リモートからフォルダを開く」を VSCode Remote 風へ

2026-09-01。`feat/1041-remote-open-vscode-like`。要望は 2 つ = ①明示的に開いた
リモートフォルダをツリーの先頭へ ②そのホストへターミナルを自動接続。

### 18.1 経路（origin）を同一性に混ぜない

「明示 open だけ前に出す」には**どの経路で載ったか**が要る。`RemoteRef`
（host + path）は**同一性のキー**で、ツリーの展開状態（`remote_expanded`）・
取得キャッシュ（`remote_cache`）・プレビューの出どころ・layout.json の同定に
使われている。ここへ経路を混ぜると「自動で開いた home」と「明示で開いた同じ home」が
**別のフォルダとして 2 行並ぶ**。

そこで器を 1 段足した:

```rust
pub struct RemoteFolder { pub remote: RemoteRef, pub origin: RemoteOrigin }
// PartialEq / Hash は remote だけで決める（origin は属性）= 手で実装する
```

`Tab.remote_folders` / `FileTree.remote_roots` の要素型をこれに替えた。
`add_remote_folder` は既存フォルダを**明示的に**開き直したときだけ
`Auto → Explicit` へ格上げする（逆へは落とさない）。

### 18.2 並び規則の正本は 1 実装

3 世代あるので取り違えやすい:

| 世代 | 並び |
|---|---|
| #919 | 全部ローカルより前へ hoist |
| #976 | 全部ローカルの後ろ |
| **#1041** | **明示 open は前・自動検知は後ろ** |

`tako_core::sidebar::remote_root_order(&[RemoteFolder], RemoteRootPlacement)` が
`{ leading, trailing }` を返す純関数で、**GUI（`FileTree::build_rows`）と
CLI / MCP（`remote-folder list`）が同じ関数を通る**。A/B の env の解決は
`ssh_folders::remote_root_placement()` の 1 か所（`ControlHost::remote_root_placement`
もそこから引くので、`list` の並びと画面の並びが構造的に一致する）。

**旧 layout.json の既定は `auto`**。経路を記録していない世代のファイルは明示 / 自動を
区別できないので、`explicit` へ倒すと**更新後の初回起動で自動検知ぶんまで先頭へ跳ねる**
（#1041 受け入れ条件 5「自動検知の挙動に回帰ゼロ」に反する）。移行 Step は不要
（`#[serde(default)]` で読めて、既定値は書き出さないので旧ファイルとバイト一致）。

**指紋の穴を 1 つ塞いだ**: `RemoteFolderLayout` は layout.json へ直に serde される
永続構造体なのに `migration_registry` の TARGETS から漏れていた（`origin` を足しても
指紋が動かず素通りする）。#728 が `PendingSpawn` を足したのと同じ穴。

### 18.3 自動接続で既存ペインを乗っ取らない

Issue の設計メモは「アイドルな素のシェルペインがあれば #1006 の pane モードを優先」を
案として挙げていたが、**自動経路では採らなかった**（判断は実装者に委ねられていた）:

1. 自動経路は「どのペインを使うか」のユーザーの意思を持たない。右クリックの
   「このペインでリモート接続…」は対象を指で選んでいるので事情が違う
2. 素のシェルに**打ちかけの行**が残っていても見分ける手段が無い。シェルのプロンプトの
   終端は OSC 133 の有無に依存し、器（psmux）つきでは取れない（#766）。
   #640 の送達フローは打ちかけの行へ続けて書くので `<打ちかけ> ssh <host>` が走りうる
3. 新しいペインなら失うものが無い（接続に失敗しても #919 のとおり理由が残る）

ペインの SSH 化は右クリック / `--target pane` の**明示操作としてそのまま使える**。
`tako_core::remote_open::auto_terminal_target()` が `Split` を返すことを
ユニットテストで固定してある。

### 18.4 重複の判定材料は 2 つとも要る

`ControlHost::live_ssh_pane(tab, host)` の実体（GUI）は次の 2 つを見る:

- **tako が開いた SSH ペイン**（#1010 の `ssh_connect`。`ConnectPhase::occupies_host()`
  が true のもの = `Failed` / `GaveUp` 以外）
- **ユーザーが手で `ssh` したペイン**（#976 の `ssh_links`。`live` のもの）

片方だけでは足りない: 1 だけだと「自分で ssh したペインの隣にもう 1 枚」ができ、
2 だけだと「tako が開いた直後（まだ `ps` を走らせていない）」に二重に作る。
`Failed` / `GaveUp` を数えないのは、**前の試行が死んでいるペインを理由に開き直しを
断ると、ユーザーが開き直しても何も起きない**から（`open` は SFTP で繋がったときにしか
来ないので、その時点で相手は到達可能）。

### 18.5 自動接続は名前のある 1 実装にした

`dispatch::auto_connect_terminal(host, origin, ssh_host, dir, tab, focus, requested)`
が `open` 応答の `terminal` を返す。`pub` にしているのは、**`open` そのものは実 SFTP
接続が要るのでセルフテストから通せない**ため（GUI のセルフテスト項目 138 がこの関数を
直接叩いて「新しいペインが立つ / 2 回目は増えない / 切ったら理由が返る」を機械検証する）。

### 18.6 実 SSH 先で見つけた副作用: 自動検知が二重に並べる

実ホストで通したら、**同じホストが 2 行**並んだ:

```
<host>:/home/<remoteuser>             explicit  leading
<remoteuser>@<host>:/home/<remoteuser>  auto    trailing   ← 増えた
```

`remote_ssh_argv` は `~/.ssh/config` の `User` を宛先へ反映する（`ssh -o … user@host`）ので、
**#976 の検知が argv から採る宛先は `user@host`**。明示 open のルートは別名（`host`）なので
`tab_has_remote_host` が突き合わせられず、自動追加が走っていた。

`ssh-pane` / `open-in remote` でも起きる**前からある**形だが、#1041 は
「フォルダを開いたら必ず SSH ペインが立つ」ので**毎回起きる**。

直し方は**キーの正規化**（検知の抑止ではない）: `apply_ssh_scan` の入口で、
その pane が `ssh_connect` に居れば（= tako が開いた SSH ペイン）宛先を
**tako が知っているホスト名**へ読み替える。ルートを持たないペイン
（`tako open-in remote <host>` で繋いだだけ）はこれまでどおり自動追加され、
名前が別名になるぶん明示経路と突き合わせられるようになる。

### 18.7 セルフテスト項目 137 (d)（#1040）が main で止まっていた

#1041 の項目 138 は 137 の後ろなので、**137 で止まると 1 回も走らない**。
同じマシンで HEAD だけ替えた A/B（`origin/main` = FAILED / 本ブランチ = FAILED、
3 回とも同じ場所）で main 由来と確定させ **#1062** へ起票したうえで、
検証を成立させるために test-only の 1 行を同梱した:

(b) が `command_flows` へ積んだ打ち直し（`echo TAKO_1040_RETRY`）を (d) の前に畳む。
同じペインへ 2 系統が書くと、送達フローの書き直し（Ctrl+C + 本文の再書き込み）が
(d) の `printf` を壊し、12 秒の待ちを使い切る。(b) は「積まれたこと」を見る検査なので
実行させる必要がない。併せて (d) の `check` へ診断（phase と画面末尾）を足した（#796 の作法）。

### 18.8 検証

- **セルフテスト項目 138**: (a) 並び (b) `list` の並びと `origin` / `placement`
  (c) 自動接続でペイン +1 と `cd` の積み込み (d) 2 回目は増えない（結果待ち・成立後の
  両方）(d2) 失敗したペインは占有しない (e) `terminal=false` (f) #976 が二重に並べない
- 既存の項目 130 (c)（**自動検知ぶんがローカルの後ろ**）が緑のまま = #976 に回帰ゼロ
- **実 SSH 先での通し 24/24**（`scripts` には置かない使い捨て検証。隔離インスタンス +
  実ホスト 2 台 = Linux / Windows）: 開く → 先頭 + ペイン自動接続 + `cd` 実行 →
  2 回目は増えない → `--no-terminal` → 到達不能でツリーは残る →
  手で `ssh` した別ホストは後ろに並ぶ。最後の `list` の並びが要点:

  ```
  <linux>:/home/<remoteuser>   explicit  leading
  <linux>:/tmp                 explicit  leading
  <linux>:/var                 explicit  leading
  <win>:/C:/Users/<winuser>    auto      trailing   ← 手で ssh した分は後ろ
  ```
- A/B: `TAKO_1041_LEGACY=1` で項目 138 (a) と (c) が FAILED になる

### 18.9 実 SSH 検証の作法（次に測る人へ）

- **隔離インスタンスの data_dir は短いパスにする**: IPC の Unix domain socket は
  `SUN_LEN`（104 バイト）制限があり、長い scratchpad パスだと
  `IPC サーバーを起動できない: path must be shorter than SUN_LEN` で **CLI が一切通らない**
- 起動情報は `<discovery>/control.json`（`instances/control-<pid>.json` は併記のほう）。
  `TAKO_DISCOVERY_DIR` は**先に作っておく**
- `tako read` / `tako split` は**素のテキスト**を返す（JSON ではない）。
  `tako send` に `--enter` は無い（改行が既定）
- ペインの画面には `Last login: … from <IP>` が出る。**この機の公開 IP なので
  貼る前に必ずマスクする**（#927）

---

## 19. #1090: Windows の OpenSSH には接続多重化が無い（2026-09-03）

### 19.1 症状と機序

Windows 実機で SSH ペインが**無言で死ぬ**（バナーの直後に 1 行だけ
`getsockname failed: Not a socket` が出て、理由も次の一手も出ないままペインが消える。
接続中チップ（#1010）も失敗へ置き換わらず消える）。#1073 の worker が
セルフテスト項目 133 (d) の確定失敗として見つけ、#1090 として分離した。

原因は **#65 の設計そのもの**。`remote_fs::common_opts` は `control_path(host)` が
取れれば **必ず** `-o ControlPath / ControlMaster=auto / ControlPersist` を渡す。
これは「ツリー（sftp）と対話ペインが同じソケットを共有し、パスワード認証しか無い相手でも
一度ログインすれば以後追加認証が要らない」ための設計だが、**Windows の OpenSSH は
接続多重化を実装していない**。

同じ機・同じ相手でオプションだけを変えた実測（`C:\Program Files\OpenSSH\ssh.exe`、
OpenSSH_for_Windows_10.0p2 / LibreSSL 4.2.0）:

| 相手 | 多重化 | exit | 所要 | 出力 |
|---|---|---|---|---|
| 名前解決できない `.invalid` | なし | **255** | 79ms | `ssh: Could not resolve hostname …` |
| 同上 | あり | **-1** | 49ms | `getsockname failed: Not a socket` / `Read from remote host …` |
| 単一ラベルの不在ホスト | なし | **255** | 1329ms | `ssh: Could not resolve hostname …` |
| 同上 | あり | **-1** | 70ms | 同上 |
| **到達できる `localhost`（ssh）** | なし | **255** | 168ms | `Host key verification failed.`（= 相手まで届いている） |
| 同上 | あり | **-1** | 41ms | `getsockname failed: Not a socket` |
| **到達できる `localhost`（sftp）** | なし | **255** | 169ms | `Host key verification failed.` |
| 同上 | あり | **255** | 62ms | `getsockname failed: Not a socket` |

**多重化を渡すと相手に届く前に死ぬ**（ホスト鍵の検証にすら進まない）ことがここで確定する。
`sftp` は `ssh` を包むので終了コードが 255 になり、対話 `ssh` だけが `-1` になる。

この `-1` が 2 つの層を同時に壊していた:

1. `remote_fs::ssh_pane_script` の失敗判定が `-eq 255` なので、理由 + 次の一手 +
   「ローカルへ戻ります」が **1 行も出ない**（#919 / #1040 の契約が成立しない）
2. `getsockname failed` が `ssh_progress::SSH_ERROR_PATTERNS` に無いので、`classify` の
   規則 ④（まっさらなペインに tako 以外の行が出たら畳む）が先に当たって **`Opened`**
   を返す（#1010 の接続中チップが失敗へ置き換わらない）

### 19.2 直し方（候補 1 + 2 の併用。3 = 相手ごとの実測記憶は不採用）

- **能力を境界で宣言する**: `tako_core::platform::ssh_client`（B26）。
  `multiplexing(Platform)` は純粋関数なので **macOS 上から Windows 側の形を検証できる**。
  縮退の文言 `NO_MULTIPLEXING` もここが正本で、対応マトリクスは参照するだけ
- `common_opts` / `ssh_pane_argv` を `*_with(.., multiplexing)` の純粋関数へ割り、
  能力が偽なら ControlMaster 系を 1 つも渡さない。`ensure_master` / `close_master` /
  `ensure_control_dir` も器を作らない。**失敗検知は消えない**（直後の sftp / ssh が
  同じ分類済みエラーを返す = #919 の「開く前に捕まえる」はそのまま）
- **生死は 3 値にした**: `remote_fs::Liveness`（Live / Dead / Unknown）。多重化が無いと
  「繋がっているか」を安く判定する材料が無いので `false` で埋めない。埋めると
  ツリーが常に「切断」になり、#1040 の自動再接続が平常時に延々と probe を撃つ
- **失敗判定は 255 だけでなくする**（1 とは独立）: `is_client_failure(code)` =
  `255` または `0..=255` の外。**POSIX の `$?` は常に 0..=255 なので macOS では
  `-eq 255` と厳密に同値** = 挙動不変（`posix_の条件はis_client_failureと同値` が固定）。
  「0 以外」へ広げると**リモートのログインシェルが `exit 1` で抜けただけ**のペインに
  「接続に失敗しました」が出るので、そこまでは広げない
- スクリプトの失敗行は**実際の終了コード**を載せる。マーカーは `ssh exit ` へ短縮したので
  新旧どちらの文面も拾える（世代をまたいだペインでも壊れない）
- `SSH_ERROR_PATTERNS` に `getsockname failed` / `Read from remote host` を追加

### 19.3 マトリクス

`tako_open_remote` は `Supported` → **`Degraded`**（#937 の根拠は #1040 より前の
「入力待ちで止まる」時代のもので、内容自体が stale だった）。`tako_remote_folder` は
`Pending(#919)` → **`Degraded`**（多重化が無いぶん + #976 の自動検知が働かないぶん）。

### 19.5 実機実測で出てきた別口 2 件（#1090 の PR に同梱）

無言死を直した**あと**でしか見えない欠陥が 2 つ出た。どちらも #1090 の原因とは別物で、
「ペインが即死していたので誰も踏めなかった」だけ。

#### (a) `--target pane` の 1 行が PowerShell で必ず構文エラー（#1006）

`launch_cmd::ps_quote` は**常に**引用するので第 1 語が
`'C:\Program Files\OpenSSH\ssh.exe'` になり、PowerShell は引用符で始まる語を
コマンド名として解釈しない:

```
PS C:\Users\<winuser> 'C:\Program Files\OpenSSH\ssh.exe' '-o' 'ConnectTimeout=10' …
ParserError: Unexpected token ''-o'' in expression or statement.
```

つまり **Windows では `--target pane` の ssh が 1 度も起きていなかった**。
呼び出し演算子 `&` を付ける組み立てを `launch_cmd::command_line` へ 1 本化した
（POSIX 側は演算子を足さない = 文字列は従来と同一）。

#### (b) 折り返されたバナーの続き行が「ssh が何か言った」と読まれる（#1010）

`is_tako_line` は行の頭（`tako: `）だけを見るが、物理行は端末幅で折り返されるので
**続き行には前置きが付かない**。44 桁のペインでは日本語のバナーが

```
tako: selftest-nonexistent-1010 へ接続してい
ます…（中止は Ctrl+C）
```

の 2 行になり、2 行目が `classify` の規則 ④（まっさら + tako 以外の行 = ssh が
何か言った）に当たって `Opened` → `Connected` → `ever_connected = true` へ進む。
直後にスクリプトの失敗マーカーを「切断」と読み、**#1040 の自動再接続が armed** になる
（`persist.log`: `ssh 切断を検知: pane=114 host=selftest-nonexistent-1010 自動再接続=する`。
画面には tako が打ち直した ssh が 2 回並ぶ）。項目 133 (d) は `phase=failed` を待つので
`reconnecting` で落ちる。

**macOS では踏まない**（ペインが広くバナーが 1 行に収まる + `/bin/sh` が何も印字しない）。
直し方はバナーの文面を `remote_fs::pane_connecting_banner` へ切り出して正本を 1 本にし、
`pane_prints`（日英）を `ssh_progress` が引いて**部分文字列で続き行を見分ける**。
併せて理由の切り出しを「直前の非空行」から「ssh の失敗行を優先」へ変えた
（折り返された理由の尻尾 `…\202\305\202\267\201B` が理由として出ていた）。

**教訓**: 「ペインが死ぬ」バグを直すと、その後ろに隠れていた層が初めて動く。
1 回の実機実測で終わらせず、**直したあとに同じ経路をもう一度測る**こと。

### 19.4 実機で SSH の相手を用意する（次に測る人へ）

Windows 実機には `~/.ssh` そのものが無く（鍵・known_hosts・config が 1 つも無い）、
Mac からのログインは `%ProgramData%\ssh\administrators_authorized_keys` が受けている。
実機のユーザー（`<winuser>`）は Administrators なので **`~/.ssh/authorized_keys` は sshd_config の
`Match Group administrators` に上書きされて読まれない**。

検証用に localhost を SSH の相手にするなら:

1. `administrators_authorized_keys` を**バックアップしてから `Add-Content` で 1 行追記**
   （ACL は触らない = 追記は ACL を変えない）。追記後に**元のバイト列が前方一致で
   残っているか**を同じスクリプトの中で確かめ、崩れていたらその場で復元する
   （リモートから直すと間に合わない = ロックアウト）
2. `ssh-keygen -t ed25519 -N '""'`（PowerShell から空パスフレーズを渡す形はこれ）
3. `icacls <key> /inheritance:r /grant:r "$env:USERNAME:(R)"`（緩い ACL は OpenSSH が拒む）
4. `ssh-keyscan -H localhost >> ~/.ssh/known_hosts`（`BatchMode=yes` はホスト鍵を聞けない）
5. `~/.ssh/config` に `Host <名前> / HostName localhost / IdentityFile … / IdentitiesOnly yes`

**検証が終わったら追記した 1 行と鍵・config・known_hosts を消す**。
