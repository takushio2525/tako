# リリース運用（詳細）

> `AGENTS.md`「リリース運用」節の**詳細**。両 OS 同時リリースの仕組み・配布物に同梱する
> ライセンス・配布物に個人情報を入れない仕組み・夜間リリース・次回バージョンの予約・
> footprint の回帰の見張り・2 台構成と夜間の役・安定版への昇格（Homebrew cask と docs の追従）はここにある。**毎ターンは読まない** —
> リリースを打つ直前・機構を触る直前にその節だけ Read する。

## 両 OS 同時リリース（#965）

**リリース 1 回で macOS / Windows の配布物が揃うのが正常な状態**。片方だけ出ると、
欠けた OS の利用者には「更新が無い」ように見えたままバージョンだけが進む
（更新チェックは自 OS 向けアセットの有無で判定する = #595）。

- 生成場所: macOS = ローカル（`scripts/release.sh`）/ Windows = **CI の windows ランナー**
  （`.github/workflows/release-windows.yml`。タグ push で起動し、同じ Release へ添付する）。
  実機依存を避けるためこの順で、実機経路（`installer/windows/release-windows.ps1`）は
  CI が使えないときの代替として残す。配布物の検査は
  `installer/windows/lib/verify-assets.ps1` の 1 実装を両経路が共有する
- `installer/windows/` の .ps1 は **pwsh 7 必須**（1 行目の `#Requires -Version 7.0`。#2329）。
  実機経路も `pwsh -File` で走らせる。Windows PowerShell 5.1 で起こすと理由を出して止まる
  （BOM の無い UTF-8 の日本語を 5.1 は ANSI として読むため。規則は `.agent/conventions.md` の #2329 の節）
- Windows の配布物のビルド（`release-windows.yml`）はビルドキャッシュ（rust-cache）を使わず、
  毎回コールドで組む（#1921。PR / main の CI の `ci.yml` は使う）。
  タグの run は別のタグのキャッシュを読めないので一度も当たらず、保存が main / PR の
  キャッシュを押し出すだけだった。理由の全文は `.agent/conventions.md`「CI のビルドキャッシュのキー」
- 待ち合わせ: `release.sh` は Windows の添付を待ってから、実アセットを読み直して
  ノートを作り直す（ダウンロード表 / 動作要件 / Windows 手順 / Known limitations が揃う）
- 片肺の検出: `release.sh` の終了コード **3**（= Release は作られたが揃っていない）。
  公開済みリリースは `scripts/release.sh --check-assets [tag]` でいつでも検査できる
  （zip の中のライセンスも見る = 次の節。#1850）。
  判定の正は `tako-core::platform::release_assets`（`missing_platforms` / `is_complete`）で、
  シェル側の写しは同期テストが拘束する。モックテスト `scripts/test-release-retry.sh` は
  **CI の macOS ジョブで走る**（片肺の検出が壊れたらそこで落ちる。docs だけの PR では skip = #2084）
- 動作要件の数値（macOS 11.0 / Windows 10.0.17763）も `release_assets` が正で、
  `tako.iss` の `MinVersion` と `build-app.sh` の `LSMinimumSystemVersion` との一致を
  テストが検証する（ノートの要件と配布物の実際の下限がズレない）

## 配布物に同梱するライセンス（#1709 / #1845 / #1850 / #2305）

GPL-3.0 第 4 条・Apache-2.0 第 4 条 (a)・MIT / BSD の表示義務は、バイナリの受け取り手へ
本文と著作権表示を渡すことを求める。**両 OS の配布物が同じ 3 本を持つ**のが正常な状態。
同梱表の正本は `scripts/lib/license-bundle.txt`（下の表は説明用。番犬・Windows のリリース検査・公開後の検査はこの 1 つを読む）。

| 元ファイル（リポジトリ直下） | macOS（`tako.app/Contents/Resources/`） | Windows（zip の `tako/`・インストール先） |
|---|---|---|
| `LICENSE` | `LICENSE` | `LICENSE.txt`（メモ帳で開けるように） |
| `THIRD-PARTY-NOTICES.md` | 同名 | 同名 |
| `THIRD-PARTY-LICENSES.md` | 同名 | 同名 |

- 組み立て: macOS = `scripts/build-app.sh`（署名より前に置く）/ Windows の zip =
  `installer/windows/build-installer.ps1` / Windows のインストーラー = `installer/windows/tako.iss`
  の `[Files]`。**3 か所とも 1 本ずつ明示で並べる**（まとめて glob にしない）
- 番犬: `crates/tako-control/tests/license_bundle_watchdog.rs` が組み立ての全箇所を
  同梱表へ突き合わせ、抜けたら置くべき場所の file:line を名指しして落ちる。検査する側
  （`verify-assets.ps1` / `release.sh` / `scripts/lib/license-bundle.sh`）に名前を直書き（2 か所目の表）しても落ちる。
  pwsh があれば `verify-assets.ps1` の読み手を実際に動かし、表どおりに読むこと・表が無い / 空 / 崩れた行 /
  置き場に無いを名指しで落ちることを確かめる（CI の macOS / Windows で走る）
- 実物の検査: `installer/windows/lib/verify-assets.ps1` の `Get-TakoLicenseBundle` が同梱表を読む
  （写しを持たない。#2305）。表が無い・1 本も無い・2 列でない行は表の file:line を名指しして throw する
  （空の表のまま素通りさせない）。
  zip は `Test-TakoWindowsAssets` が展開してリポジトリの元ファイルとのバイト一致を見る
  （CI・実機の両経路）。インストール先は `Test-TakoInstalledPayload` が**使い捨ての CI
  ランナーでだけ**無人インストールして見る（実機で走らせると同じ AppId の実インストールの
  登録を上書きするので、`release-windows.ps1` からは呼ばない。番犬もこれを固定する）
- **公開後の検査**（#1850）: `scripts/release.sh --check-assets [tag]` が Release の zip（両 OS）を
  一時 dir へ取得し（`gh release download` の読み取りだけ・確かめたら消す）、同梱表の全部が入って
  **そのタグのファイルと**バイト一致するかを見る。zip の中の置き場は macOS =
  `tako.app/Contents/Resources/`・Windows = `tako/`（v0.8.37 の実物で確認）。`.exe` は開けないので
  見ない。v0.8.25（同梱前）は Windows の zip の 2 本の欠けで exit 1、v0.8.37 は通る。
  モックは `scripts/test-release-retry.sh` の Test 9〜13
- **Windows の配布物のドライラン**: PR の CI は Windows の配布物を組まない（タグ push 時だけ）。
  組み立てを変えた PR は `gh workflow run release-windows.yml --ref <ブランチ>` で確かめる。
  タグ以外の ref では Cargo.toml の版数で組み立て・検査・artifact 保存（`tako-windows-dryrun-<run id>`）
  までを行い、**Release へは添付しない**。ログの「zip の tako/ の中身」「インストール先の中身」が
  生成物の一覧になる
- `THIRD-PARTY-LICENSES.md` の中身は `cargo about`（`about.toml` / `about.hbs`）で生成する。
  **作り直しは `bash scripts/third-party-licenses.sh` の 1 本**。依存を足し引き・更新した PR では
  一緒に作り直してコミットする
- **鮮度の検査（#1851）**: CI の `licenses` ジョブ（ubuntu・docs だけの変更では skip）が `--check` で
  今の Cargo.lock から作り直した物と突き合わせ、ずれていれば載っていない / 余分なクレートと
  作り直しのコマンドを出して落ちる（着手時点の main では LSP の依存 4 クレートが抜けていた）
  - ubuntu 1 本で足りるのは、cargo-about が `about.toml` の `targets`（aarch64-apple-darwin /
    x86_64-pc-windows-msvc）で依存グラフを絞り、走らせる OS に依らず両 OS ぶんを出すため
    （macOS で作り直した物が ubuntu の検査で一致する）
  - cargo-about は install-action の配布バイナリ（sha256 を照合。`cargo install` は毎回数分の
    コンパイル）。キャッシュは持たない（Actions のキャッシュの容量を増やさない）。版は
    スクリプトの `CARGO_ABOUT_VERSION` と ci.yml の `tool: cargo-about@<版>` の 2 か所で、
    違えばスクリプトが入れ直しのコマンドを出して落ちる
  - **罠**: オンラインの cargo-about は LICENSE を同梱しないクレートの本文を上流の git から
    取りに行くので、上流の変更で出力が揺れうる。スクリプトは先に `cargo fetch --locked` で
    ソースを揃え、`--frozen` で走らせる（2026-10-18 の実測ではオンラインの出力と改行コード以外は同一）。
    クレートが同梱する LICENSE の CRLF（lebe ほか）は git が eol=lf で揃えるので、スクリプトが
    LF へ揃えてから書く（揃えないと作業ツリーとインデックスで改行が食い違う）
  - 診断のモックテストは `bash scripts/test-third-party-licenses-1851.sh`（同じジョブで先に走る）

## 配布物に個人情報を入れない（#1848）

v0.8.24 までの macOS 版には、ビルド機のホームパス（実ユーザー名入り）が `tako-app` 1,064 行・
`tako` 395 行（`strings` の行数）と、署名の名義（Apple Development 証明書 = 個人名）が入っていた。
Windows 版は該当なし。既に配った版はそのまま（差し替えない）。

- **パスの付け替え**（`scripts/build-app.sh` の中だけ）: ホームを `~` へ付け替えてビルドする。
  - rustc: `--remap-path-prefix=$HOME=~` を `CARGO_ENCODED_RUSTFLAGS` で渡す（空白入りのホームでも
    割れない。呼び出し元の `RUSTFLAGS` / `CARGO_ENCODED_RUSTFLAGS` は引き継ぐ）。依存 crate・
    rust-src の std のソースパス（panic の位置情報）がここで消える
  - シェーダー: gpui_macos の build.rs が `xcrun metal` / `xcrun metallib` を固定の引数で呼び、
    metallib にソースの絶対パスと metallib 自身のコマンド行が入る。rustc のフラグは届かず、
    `CCC_OVERRIDE_OPTIONS` も metal のドライバには効かない（実測）ので、ビルドの間だけ PATH の
    先頭に `scripts/lib/xcrun-remap/xcrun` を置き、metal へ `-ffile-prefix-map` を足し、metallib は
    出力先へ移って名前だけで呼ぶ（ほかの xcrun の呼び出しは素通し）
  - **パスはリポジトリに書かない**（`.cargo/config.toml` に書くとそのパスが public リポに入る）
  - **`env!("CARGO_MANIFEST_DIR")` は付け替えが効かない**（ただの文字列定数としてバイナリに入る）。
    release に入るコード（実行時のセルフテストを含む）では使わず、実行時に辿る
    （セルフテストは `self_test::source_tree_root`）。入れた実測では、付け替え後も
    セルフテストの 2 か所だけが残り、検査が build-app.sh を止めた。テスト（`#[cfg(test)]`）と
    `visual-test` feature の中は配布物に入らないので対象外
- **専用の target dir**（`target/release-dist`）: フラグが変わると cargo のキャッシュが別物に
  なるので、隔離検証・テストが使う `target/release` とは分ける。フラグにはホームだけを入れる
  （worktree のパスを入れると、夜間リリースの一時 worktree で毎晩フルビルドになる）。
  夜間リリースは `target/` の symlink 経由で共有ツリーの `target/release-dist` を使い回す。
  **入れた直後の最初の 1 回だけ**フルビルドになり、ディスクを約 2GB 足す（新規ビルドの実測 1.9GB）
- **署名は既定で ad-hoc**（`TAKO_CODESIGN_IDENTITY` を指定したときだけ証明書で署名する）。
  2026-09-30 の実測で、Apple Development（公証なし）と比べて悪くなる点は見つからなかった:
  - Gatekeeper: quarantine 付きの zip を展開した .app が、どちらも `spctl --assess` で rejected。
    `syspolicy_check distribution` の Fatal はどちらも「Notary Ticket Missing」だけ
    （ad-hoc には Warning「Adhoc Signed App」が 1 件増える）。利用者の手順（「このまま開く」/
    `xattr -dr com.apple.quarantine`）は README・docs・cask の案内のまま
  - TCC: DR は #54 で identifier 固定。実機の TCC.db（ユーザー + システム）で tako の要件つきの全 19 行について、
    保存された要件を満たすかが両者で一致した（フルディスクアクセス・画面収録・フォルダ等は両方満たし、
    #54 より前の証明書縛りの古い行はどちらも満たさない）
  - アプリ内更新: zip は tako 自身が ureq で落とすので quarantine が付かず、Gatekeeper は関わらない。
    差し替え後も DR は同じなので TCC は保持される
  - キーチェーン: tako 自身はキーチェーンを使わない（子の claude 等の項目の許可は、その子の署名で
    決まる）。ad-hoc は秘密鍵を使わないので、夜間リリース（launchd）が鍵の許可待ちで止まる余地も、
    証明書の失効も無い
- **検査**: `scripts/lib/bundle-privacy.sh` の `check_bundle_privacy` 1 本を、`build-app.sh` の署名の後と
  `release.sh` の zip の直前（`--skip-build` で古い `dist/tako.app` を包む経路も塞ぐ）が呼ぶ。
  バンドル内の全ファイルの中身に `HOME` / `USER`（汎用の名前は除く）/ `id -F` / `TAKO_PII_TERMS` の
  語が無いこと、署名の Authority が個人の開発用証明書（Apple Development 等）でなく名義に語も
  無いことを見て、落ちたら**値を出さずに**種類・件数・場所を出す。テストは
  `bash scripts/test-bundle-privacy-1848.sh`（CI の macOS ジョブ。偽の HOME と ad-hoc 署名の偽 .app だけを使う）
- Phase 7 で Developer ID を入れるときは、個人アカウントなら名義が個人名になる。検査は名義に
  `id -F` 等の語が入れば止まるので、そこで名義を表に出すか（組織アカウントにするか）を判断する

## 夜間リリース（自動。#166 / #1005 / #1136）

- `scripts/nightly-release.sh` が launchd（`com.takushio.tako-nightly-release`、毎日 5:00）から
  実行され、前回タグ以降に main へ変更があった夜だけ自動リリースする
  （version bump → CHANGELOG 自動節 → コミット → annotated tag → release.sh でバイナリ付き
  GitHub Release）。クラウドルーチンでの夜間リリースはバイナリを作れず廃止した（経緯は #166）
- 自動スキップ条件: 変更なし / 共有ツリー dirty / 手動リリース進行中（Cargo.toml version ≠ 最新タグ）/
  プレリリース版 / 多重起動。ログは `~/.claude-orchestrator/logs/tako-nightly-release.log`
- **共有ツリー（install_root）の HEAD は動かさない（#1136）**: リリース作業は毎回
  `git worktree add --detach` した使い捨てのツリーで行い、成功・失敗・シグナルのどれでも
  `trap` で撤去する。ビルド中に origin/main が進んだら**何も作らず中止**する（旧版はここで
  push が拒否されて無言で死に、未 push のリリースコミットごと detached を残していた =
  同じ版が別 SHA で 2 本できる原因）。多重起動ロックは HOME 単位 + **リポジトリ単位**の 2 段
- **戻した main は origin/main へ追従させる（#2262）**: 旧版が残した detached（先端が夜間リリースのコミット・clean）は
  main へ戻し、その main が origin/main より古ければ **fast-forward できるときだけ**追従させる。launchd が翌夜に読むのは
  共有ツリーの nightly-release.sh なので、戻すだけだと翌夜は古い main の夜間が走り、旧版と新版が 1 日おきに入れ替わる
  （旧 Mac の実例 = 下の「今の 2 台」）。戻し先を origin/main にしない（`checkout -B main origin/main` / detach）のは
  #1136 の理由を守るため: HEAD はブランチ main のまま・main のコミットを書き換えも捨てもしない・共有ツリーにコミットを
  作らない。main に origin/main に無いコミットがある（分岐）・untracked が上書きされるので ff できないときは触らず
  WARN と通知（今夜のリリースは止めない）。fetch できなければ手元の origin/main まで追従し、`--dry-run` でも戻すので
  追従もする。モックは `/bin/bash scripts/test-nightly-restore-main-2262.sh`（CI の macOS ジョブ。A/B は
  `TAKO_2262_LEGACY=1`、修正前の版に当てるときは `NIGHTLY_UNDER_TEST=<file>`）
- **リリースする役は毎晩 main を追従させる（#2264）**: 戻した夜だけだと、共有ツリーを誰も pull しない常駐機（旧 Mac）では
  その後の main が戻した夜の origin/main で止まり、launchd が読む nightly-release.sh・`scripts/lib/` と、共有ツリーから呼ぶ
  mod の検査・footprint の見張りの手順が二度と更新されない（交互が固定に変わるだけ）。そこで**全部の役（既定）の夜間は、
  共有ツリーがブランチ main・clean のとき毎晩**同じ判断（fast-forward できるときだけ・独自のコミット / untracked の上書きは
  触らず WARN と通知）を通す。mod の検査より前に置くので、その夜の検査から新しい手順で回る
  - **検査だけの役（`--role check`）は追従させない**（共有ツリーの HEAD も作業ツリーも動かさない）。その PC は master が
    pull するので古くならず、5:00 に共有ツリーが書き換わって master のビルドと混ざる余地も避ける。ただし旧版が残した
    detached を戻した夜の追従（#2262）は #1136 の後始末なので役に依らず行う（戻すだけだと翌夜は古い main の夜間 =
    `--role` を知らない版が走り、検査だけの PC がリリースしうる）
  - dirty（人の作業中。後段の SKIP が知らせる）・main 以外のブランチ（ログに 1 行）・detached は触らない。
    戻した夜は判断と fetch を 1 度だけにする。`--dry-run` は動かさず「夜間なら追従させる」とだけ出す
  - 1 台の利用者（役を設定しない = 全部）では、共有ツリーがブランチ main・clean のとき毎朝 `git pull --ff-only` と
    同じことが起きる（人の作業中 = dirty・別ブランチ・独自のコミットには触らない）
  - モックは `/bin/bash scripts/test-nightly-follow-main-2264.sh`（CI の macOS ジョブ。A/B は `TAKO_2264_LEGACY=1`、
    修正前の版に当てるときは `NIGHTLY_UNDER_TEST=<file>`。古い版と新しい版は検査の代役の版 v1 / v2 で見分ける）
- **中のビルドは順番待ち（#1994）を通る**: `build-app.sh` のリリースビルドは `scripts/lib/queue.sh` の
  `run_heavy` を通り、env の `TAKO_QUEUE_PRIORITY=low` / `TAKO_QUEUE_MAX_WAIT=3h` で人や worker の後ろへ並ぶ
  （3 時間で諦めた `tako queue` は 75 で返り、その夜のリリースは失敗として通知される）。launchd の PATH は先頭が
  `~/.cargo/bin` で、そこに順番待ちを知らない古い tako（`cargo install` の残り）が居ると、旧版の `run_heavy` は
  PATH の先頭の 1 本だけを見て並ばずに走らせていた（#2119）。いまは PATH の tako を上から全部 → `.app` の同梱 CLI の
  順に確かめて最初に知っている 1 本で並び、飛ばした tako をログへ 1 行残す。**知っている tako が無い・応答しない
  （10 秒）ときは今どおり直に走らせる**（夜間を止めない）。モックは `bash scripts/test-nightly-queue-path-2119.sh`
  （夜間の PATH の export 行をそのまま評価して再現する・A/B は `TAKO_2119_LEGACY=1`）
- ジョブ登録は `scripts/nightly-release.sh --install-launchd`（解除は `--uninstall-launchd`、
  確認は `launchctl list | grep tako-nightly`）。plist はリポに置かず実行時に生成する
- Homebrew cask の更新は**安定版への昇格**（下の節）が行う（夜間のテスト版では cask を動かさない）。
  リリースノートの日英併記は従来どおり手動で行う

### 次回バージョンの予約（#1005）

**版数は既定で patch bump**。節目の minor / major を夜間発火に乗せたいときだけ予約する
（Cargo.toml を先に上げると「≠ 最新タグ = 手動リリース進行中」でスキップされるため、
版数の指定は**リポジトリの外**の状態ファイルで持つ）。

| 操作 | コマンド |
|---|---|
| 予約する | `scripts/nightly-release.sh --reserve 0.8.0` |
| 確認する | `scripts/nightly-release.sh --reserve`（引数なし） |
| 取消する | `scripts/nightly-release.sh --unreserve` |

- 正本は `scripts/lib/nightly-reserve.sh`（読み書き・検証・版種判定の 1 実装）。
  予約ファイルは `~/.claude-orchestrator/state/tako-nightly-next-version`
  （ログ / ロックと同じ置き場。**リポジトリの外**なので worktree を dirty にせず、
  ロールバックの `git reset --hard` でも消えず、誤コミットの余地も無い）
- **予約は成立したリリース 1 回で消費**される（タグを push した時点でクリア）。
  版種（patch / minor / major）は CHANGELOG の節・コミット件名・タグ注釈へ自動で載る
- **予約しても配布形態は変わらない**: 夜間リリースは常に**テスト版（prerelease）**として出る
  （#403）。節目の版を安定版として出したいときは、出たあとに
  `scripts/release.sh --promote v<tag>` で昇格させる
- 使えない予約値（semver 外 / プレリリース付き / 現行以下 / タグが既に在る）は
  **予約を無視して patch bump へフォールバック**し、警告ログ + 通知を出す
  （`--reserve` での指定時にも同じ検証で弾く）
- **リリースに至らなかった夜は予約を保持する**。「予約あり + 変更ゼロ」でも消費せず、
  次に変更が入った夜へ持ち越す（dirty / 手動リリース進行中 / プレリリース版 /
  ビルド失敗 / `--dry-run` も同じ）
- 検証は `bash scripts/test-nightly-reserve.sh`（一時ディレクトリに origin + 作業リポを
  作り、launchd と同じ `/bin/bash` で実走させる。release.sh はスタブ・HOME も隔離するので
  **本番のタグ / Release / 予約ファイル / launchd には触らない**）
- **launchd が実行するのは install_root 側のスクリプト**（既定 `~/dev/tako/scripts/nightly-release.sh`）。
  予約機構を直したときは、そのパスへ反映されているか（= main を pull 済みか）まで確認する

### tako mod の検査（#1892）

tako mod（`crates/tako-core/claude-mod`）の API は early access で、Claude Code の更新で mod が
読まれなくなっても tako は画面の読み取りへ落ちるだけで壊れない（`.agent/plans/2026-10-tako-mod.md` §6）。
気づけないと「いつの間にか一次ソースが消えていた」になるので、夜間リリースの前段で毎晩検査する。

- 本体は `scripts/check-claude-mod.sh`（1 実装。手で叩いても同じ）。夜間は
  `--ref origin/main --record` で、**origin/main の mod**（次に出る版）を一時 dir へ取り出し、
  `claude plugin validate --strict` と `claude plugin test` にかける。取り出しは `git archive` を
  **ファイルへ書いてから** `tar -x -f` で開く（#1933。パイプで繋ぐと、tar（bsdtar）が終端の印を読んだ
  時点で git が後ろへ書くゼロ詰めを読まずに抜け、git の最後の書き込みがそれより遅れると git が SIGPIPE で
  黙って 141 → pipefail で「mod を取り出せない」が CI の macOS で間欠的に出ていた。A/B は
  `TAKO_1933_LEGACY=1`）。合格の条件は終了コード 0 に加えて、
  ゲートになるフックの `.catch` 抜け（validate は注記に出すだけで合格させる = 設計書 §5 の規約違反）が
  無いこと・テストが 1 本以上走ったこと
- **文言一致の自己検査**（#1903）: 上の 2 つは注記の文言（`gating hook without .catch` / `Ran 0 tests`）で
  見ているので、Claude Code の更新で文言が変わると黙って効かなくなる。そこで**肯定形の行が出ていること**
  （validate の `gating hook with .catch: <event>` = この mod のゲートになるフックの申告・test の要約
  `Ran N test(s)`）も要求し、出ていなければ「注記の文言が変わった」で不合格にして通知する
  （直す場所は `scripts/check-claude-mod.sh` の文言一致）。追加の claude の呼び出しは無い。
  `scripts/test-claude-mod-1879.sh` の段 0 も同じ本体を呼ぶ（検査の実装は 1 つ）
- **リリースの有無に依らず毎晩**回す（壊すのは tako の変更より Claude Code の更新なので、
  変更なし / dirty / 手動リリース進行中で抜ける前に置く。ロックを取った直後）
- **結果でリリースを止めない**。落ちたらログに `ERROR` + 既存の通知（`tako 夜間リリース`）を出して先へ進む:
  ①配布済みの版も同じ mod を持つので、止めても利用者は守れず無関係な修正の配布だけが止まる
  ②mod が読まれなくても tako は壊れない（§6）③更新を tako が制御できない外部の CLI の揺れで
  夜間リリースを止めない
- **claude が無い**（`TAKO_CLAUDE_BIN` か PATH で見つからない）ときは「未実測」と出して飛ばす
  （終了コード 3。通知はしない。#1879 の受け入れ 8 と同じ扱い）
- **利用者の Claude Code の設定に触らない**: 設定 dir（`CLAUDE_CONFIG_DIR`）・mod の写し・claude の
  作業 dir はすべて毎回作る一時 dir で、終わったら消す。自動更新と不要な通信は止める
  （`DISABLE_AUTOUPDATER` / `CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC`）。実測で HOME 配下は一時の
  設定 dir 以外に何も書かれない（2.1.294）
- **遅らせない**: 実測は 1 段 1 秒前後（2.1.294 で validate 0 秒・test 1 秒）。各段（`--version` /
  validate / test）に上限（`TAKO_MOD_CHECK_TIMEOUT`、既定 60 秒）を付け、超えたら `set -m` で分けた
  プロセスグループごと止める（test は子プロセスでテストを走らせる。launchd の制御端末なしでも効く）。
  前の段で打ち切ったら後ろの段は走らせない
- **記録**: `~/.claude-orchestrator/state/tako-mod-check`（予約ファイルと同じ置き場。`--last` で読める）に、
  検査した claude の版・mod の出どころ（`origin/main@<sha>` と mod の木）・各段の結果と、**最後に合格した
  claude の版と mod の木**を持つ。不合格のときは前回の合格と比べて「Claude Code の更新で壊れた可能性」
  「mod の変更で壊れた可能性」「両方が重なった」を通知とログで出し分ける。夜間のログ
  （`tako-nightly-release.log`）にも毎晩の版と結果が `mod:` の行で残る（履歴はこちら）
- **記録が使えなくても結果の行は必ず出す**（#1969）: 記録が読めない（権限・種類）ときは前回の合格と
  比べずに判定し、結果の行に「前回の記録が読めない」を足して記録を書き直す（合格なら終了コード 0）。
  書けない（置き場・権限）ときは結果の行に「記録を書けない」を足し、合格なら終了コード 4 = 夜間は
  WARN と理由つきの通知を出す。検査が結果の行を出さずに止まったら（引数の誤り・シグナル等）、夜間は
  「途中で止まった（exit N）」で通知する（旧版は本文が空の通知「mod の検査: 」だった）
- 終了コード: 0 = 合格 / 1 = 不合格（打ち切り・mod を取り出せないを含む）/ 2 = 引数の誤り / 3 = 未実測 /
  4 = 合格したが記録を書けない
- **launchd の登録は変えない**（既存の `com.takushio.tako-nightly-release` が install_root の
  nightly-release.sh を叩くので、main を pull すれば翌晩から効く）
- 検証は `bash scripts/test-nightly-mod-check-1892.sh`（CI の macOS ジョブ。claude・osascript・release.sh は
  スタブ、HOME も隔離。壊れた登録の A/B・claude 無し・固まる claude・止めない実走・変更なしの夜・
  設定 dir の隔離・記録が読めない / 書けない・検査が途中で止まるを見る。実物の claude（下限 2.1.294
  以上）があれば本物の mod と壊れた写しも検査し、設定に触らないことを見る。このとき HOME と利用者の
  設定 dir は**砂場に作る**（#1969。利用者の実ファイルの mtime を比べると、並走中の Claude Code が
  `.claude.json` を書いて手元だけ揺れた））。assert が落ちると、その環境の段（夜間 / 検査の 1 回の実行）の
  出力と終了コード・記録ファイル・通知のスタブの記録・claude のスタブの呼び出しがログに出る（#1933。
  `TAKO_1933_INJECT=version|validate|test` で Test 2 の注入前の 1 回を落として確かめられる。
  `TAKO_1933_INJECT=archive-late-tail` は git archive の最後の書き込みを遅らせて「取り出せない」を再現する =
  修正後は緑・`TAKO_1933_LEGACY=1` を足すと落ちる）。上限つきの段で起こすスタブ（claude と、固まる
  claude の hang / hang-child）は作った時点で `scripts/lib/fake-exec.sh` の `fake_exec_warm` で空打ちする
  （#2222。作りたての実行ファイルの初回 exec の検査で、Test 5 の上限 2 秒を食わない）

### footprint の回帰の見張り（#2023 / エピック #2011 の S8）

#814（2026-08）で本番 129 MB まで削ったものが 2 か月で 400〜835 MB に戻り、**誰も気づけなかった**。
夜ごとに origin/main を同じ条件で測り、直近の中央値より閾値を超えて悪化したら `tako todo`（人がやること）へ
1 件載せる。本体は `scripts/nightly-footprint-watch.sh`、記録と判定は `scripts/lib/footprint-watch.py`。

- **起動**: `nightly-release.sh` の EXIT trap の最後（使い捨ての worktree の撤去・ロックの解放の後）から別プロセスで呼ぶ。
  **リリースを遅らせない**（タグ・Release・通知は済んでいる）・**どの結果でも夜間の終了コードを変えない**
  （`run_footprint_watch || true`。番人 `tako_exit_trap` は trap の前に決めた終了コードで抜ける）・
  **リリースの有無に依らず毎晩**（変更なし / dirty / 手動リリース進行中で抜けた夜も回す）。dry-run・多重起動・
  シグナルで中止した実行からは回さない。止めるときは `TAKO_NIGHTLY_FOOTPRINT=0`
- **1 回の流れ**: 見張りのロック → origin/main を `git worktree add --detach` で出す（共有ツリーの HEAD は触らない。
  target/ は共有ツリーのものを symlink で使い回す）→ `scripts/build-pwa.sh --if-missing` + `cargo build --release` を
  1 回の `run_heavy` で（**cargo の前に PWA の dist を作る**: 使い捨ての worktree には dist が無く、tako-control の build.rs に
  作らせると dist の時刻のせいで計測スクリプトの中の 2 回目のビルド = 順番待ちの外で下流が丸ごと組み直しになる。上限 60 分、
  順番待ちはその 2/3 で諦める。**並ぶのは起票に使う tako = `/Applications/tako.app` の tako**（その置き場を PATH の
  先頭へ置く。起票と並ぶ先を同じ 1 本に揃えるため。PATH の先頭の古い tako に負けない点は run_heavy 自身も #2119 で直した））→ `scripts/measure-footprint-2011.sh std`（上限 15 分・偽の claude・tako-vd の上だけ）→
  履歴へ 1 行 → 判定 → 悪化なら起票。**そのコミットを描画ありで測り済みなら数秒で飛ばす**（コードが変わらない夜は測らない）
- **置き場**: `~/.claude-orchestrator/state/tako-footprint/`（`history.tsv` = 1 晩 1 行・未実測の夜も残す /
  `state` = 続いている悪化 / `last-run.log` = 直近の計測の全出力）。data dir ではなく開発側で、`cargo clean`
  （`scripts/clean-target.sh`）で基準が消えないよう target/ の外に置く（予約・mod の検査の記録と同じ所）
- **基準**: origin/main を測った行（と今回と同じ ref の行）のうち、同じ条件（シナリオ・ペインの並び・プレビュー数）・標準の env（`TAKO_FOOTPRINT_ENV` を足していない）・
  中身が全部入った・**描画の有無が同じ**行の直近 5 回の中央値。3 回に満たない間は「基準づくり中」で起票しない。
  注入の行（env を足した行）は判定されるが基準には入らない
- **閾値**（S0 の 2 回のぶれ = 設計書 `.agent/plans/2026-10-lightweight.md` §7.1 から。正本は `footprint-watch.py` の `METRICS`）:

| 項目 | 閾値 | 描画が要る |
|---|---|---|
| 放置の footprint / 出力中の footprint / tmux の footprint | +20%（std のぶれ +7〜10%） | 出力中だけ |
| 放置の CPU | +0.3 ポイント（ぶれ ±0.15） | 要る（アイドルの CPU の大半は毎 vsync の display link） |
| 出力中の CPU | +1.5 ポイント（ぶれ ±0.9） | 要る |
| 出力中の子プロセスの CPU | +1.0 ポイントかつ +50%（2 回のぶれの実測なし） | — |
| スレッド | +5 本（ぶれ ±2） | — |
| 起動時間 | +1000 ms かつ +50%（その日の最初の起動かで 0.5〜2.6 秒） | — |
| 出力中の描画回数 | 31.5 回/秒を超える（#1979 の上限 30 + ぶれ 1.5。基準は要らない） | 要る |

- **蓋閉じの夜**: tako-vd を用意できない（計測の終了コード 4）・ビルド / 計測の時間切れは「未実測」として 1 行残して
  静かに抜ける（起票しない・状態も動かさない）。tako-vd は起きたが画面ロック等でフレームが組まれない夜（`drawn` が
  「なし」）は描画の値を比べず、残りを描画なしの夜どうしで比べる
- **同じ悪化を重ねて積まない**: `state` に前の夜から続く悪化の項目と todo の id を持つ。同じ項目が悪化し続ける夜は
  何もしない。**新しい項目が悪化したときだけ**起票し、未完了の 1 件（title の頭が `[footprint 回帰]`）が残って
  いればそれを書き換える。回帰なしの夜が来たら `state` を消す（todo は人が片付ける）。`tako todo` へ届かない夜
  （GUI が居ない）は `state` を動かさず通知だけ出し、次の夜にやり直す
- **基準は追いつく**: 中央値は悪化した行も含むので、直らない悪化は 3 晩ほどで基準に吸収されて鳴らなくなる
  （起票は最初の夜の 1 回）。**少しずつの増加（1 晩あたり閾値の内側）は拾えない**のが限界
- 直近の結果と基準は `scripts/nightly-footprint-watch.sh --status`（読むだけ）。測り直しは `--force`、
  ブランチを夜間と同じ流れで測るのは `TAKO_FOOTPRINT_WATCH_REF=<コミット>`（その行は origin/main の行と比べられるが、
  履歴の `ref` 欄で分けるので origin/main の基準には入らない）。
  注入は `TAKO_FOOTPRINT_WATCH_SCALE=0.5`（閾値を半分）か `TAKO_FOOTPRINT_ENV="TAKO_2009_LEGACY=1"`
  （#2009 の前の持ち方 = わざと footprint を増やす）
- 検証は `bash scripts/test-nightly-footprint-watch-2023.sh`（CI の macOS ジョブ。計測・tako・cargo・release.sh は
  代役、HOME も隔離。判定の単体・見張りの通し・夜間のフックの終了コードを見る）
- **launchd の登録は変えない**（install_root の nightly-release.sh が呼ぶので、main を pull すれば翌晩から効く）

### 2 台構成と夜間の役（#2252）

LaunchAgent は PC ごとに登録するので、2 台に入れると**同じ時刻に同じ版を 2 台がビルドする**。2026-10-17 に
手元の Mac と旧 Mac（常駐機）の 2 台で動いていると分かった。公開していたのは旧 Mac で、手元は毎朝 7.5 分ビルドしてから
push の直前に「ビルド中に origin/main が進んだ」（= 旧 Mac が push したリリースのコミット）で中止していた。push の前の
先端の照合とタグの有無の検査（#1136）があるので同じ版が 2 本できることは無いが、ビルドの負荷が毎朝無駄になり、
同じタグを取り合う競りの余地も残る。そこで**リリースを出すのは 1 台だけにし、もう 1 台は「検査だけ」の役にする**。

| 役 | 設定 | 回るもの |
|---|---|---|
| 全部（既定） | 何もしない | 変更検知 → 版数 → ビルド → push → タグ → GitHub Release と、mod の検査・footprint の見張り。共有ツリーの main を毎晩 origin/main へ追従させる（#2264） |
| 検査だけ | `scripts/nightly-release.sh --role check` | mod の検査（#1892）と footprint の見張り（#2023）だけ。共有ツリーは動かさない |

- **ゼロコンフィグ**: 役を設定しない 1 台の利用者は今までどおり全部やる（夜間のログも変わらない）。
  確認は引数なしの `--role`、既定へ戻すのは `--role full`（状態ファイルを消す = 何も設定していない 1 台と同じ）
- 役は予約と同じ置き場の状態ファイル `~/.claude-orchestrator/state/tako-nightly-role` の **1 か所**に持つ
  （PC ごとの設定・リポジトリの外なので worktree を dirty にしない・`--install-launchd` で plist を作り直しても
  消えない・launchctl の読み直しが要らない = 次の 5:00 から効く）。launchd が叩くのは install_root の
  nightly-release.sh なので、**main を pull してから** `--role` を打つ（古い版は `--role` を知らない）
- 検査だけの役は **mod の検査の直後で抜ける**: 変更検知・dirty の判定・版数・使い捨ての worktree・ビルド・push・
  タグ・Release・予約の消費のどれにも入らない（cargo も gh も呼ばない）。footprint の見張りはロックを取った時点で
  仕掛け済みなので、全部の役と同じく EXIT trap の最後に回る。多重起動・`--dry-run`・`TAKO_NIGHTLY_FOOTPRINT=0` の
  扱いも全部の役と同じ
- **予約はリリースする PC で打つ**（予約ファイルも PC ごと）。検査だけの PC にある予約は使わず消費もせず、
  夜間のログに WARN、`--reserve` の表示に注意を出す
- 状態ファイルの値が読めない（手で書き壊した）ときは止めずに**既定の全部で回し**、WARN と通知で知らせる
  （リリースする PC が先に出していれば push の前の照合で何も作らずに止まるので、害はビルドの負荷だけ）
- mod の検査は claude、footprint の見張りは仮想ディスプレイ（tako-vd）が要る。どちらの記録
  （`~/.claude-orchestrator/state/` の `tako-mod-check` / `tako-footprint/`）も PC ごとに持つ
- 検証は `bash scripts/test-nightly-role-2252.sh`（CI の macOS ジョブ。release.sh・mod の検査・footprint の見張り・
  cargo・gh・osascript・launchctl は呼ばれたことを記録する代役、origin は一時ディレクトリの bare リポ、HOME も隔離。
  役なしで全部の段が走る / 検査だけで origin の main とタグが動かず検査 2 つが走る / 同じ環境で `--role full` へ
  戻すとリリースする / タグが無い・dirty・予約あり・dry-run・多重起動 / 不正な値と壊れた値 / plist が役で変わらない、を見る）

#### 今の 2 台（2026-10-17 に旧 Mac を読み取りだけで確認）

- **旧 Mac = 全部**（リリースを出す側。2026-10-17 にユーザーが「夜間は旧 Mac で続ける」と決めた）/
  **手元の Mac = 検査だけ**（#2252 の merge 後に master が `--role check` を打つ）
- **旧 Mac では mod の検査も footprint の見張りも回っていない**: `~/.claude-orchestrator/state/` に両方の記録が無く、
  夜間のログにも `mod:` / `footprint:` の行が 1 行も無い。手元を検査だけにしても検査は失われない（今は手元でしか回っていない）
- 原因は旧 Mac の共有ツリーが **1 日おきに 2 つの版を行き来している**こと（リリース自体は毎晩出ている）:
  - 前夜のリリースのコミットで detached の夜は新しい版が走り、`restore_detached_head`（#1136）が共有ツリーを
    **ローカルの main** へ戻す。旧 Mac のローカルの main は 2026-08-14 で止まっていて（origin/main より 688 コミット古い）、
    `scripts/check-claude-mod.sh` も `nightly-footprint-watch.sh` も無いので、mod の検査は「無いので飛ばす」
    （10/12・10/14・10/16 の WARN）。footprint の見張り（v0.8.35 から）も同じ理由で飛ぶ
  - 翌夜は launchd がその古い main の nightly-release.sh（#1136 / #1892 / #2023 より前 = 共有ツリーをその場で detach して
    リリースする版）を読むので検査が無く、共有ツリーをまたリリースのコミットで detached のまま残す
  - 真因は `restore_detached_head` がローカルの main の鮮度を見なかったことで、**#2262 で直した**（上の「夜間リリース」の節の
    「戻した main は origin/main へ追従させる」）。旧 Mac には修正が origin/main に入れば自力で届く: 古い版の夜は共有ツリーを
    最新の origin/main（修正入り）のリリースのコミットで detach し、翌夜の新しい版が main へ戻して追従させる（2〜3 晩で揃う）。
    旧 Mac の main に独自のコミットがあるとそこで WARN と通知が出て止まるので、そのときだけ管轄の master と調整する
  - 戻した夜の後も古くならないよう、リリースする役（旧 Mac）は毎晩追従させる（#2264）。ただし毎晩の追従は**走る夜間の版に
    入っていて初めて効く**: 戻した夜に揃う main（その夜の origin/main）に #2264 が入っていなければ、旧 Mac はその版で
    止まる（その版は毎晩の追従を持たない）。そうなったら人が旧 Mac の main を揃える（管轄の master と調整）

## 安定版への昇格（#403 / #1853 / #1592）

夜間リリースは常にテスト版（prerelease）で出るので、**安定版（Latest）は人が昇格させる**。
昇格は `scripts/release.sh --promote <tag>` の 1 本で、Release・Homebrew cask・docs の
「最新の安定版」を同時に動かす。v0.8.0 の昇格は Release だけを動かし、cask は 1 か月半
v0.7.0 のまま、docs のラベルも取り残された（#1853 / #1546）。

| 受け付けるタグ | Release の扱い |
|---|---|
| 夜間の素のタグ（`v0.8.26`） | その Release の prerelease を外して Latest にする（タグ・アセットはそのまま） |
| テスト版タグ（`v0.6.0-test.1`） | 同じコミットに安定版タグ（`v0.6.0`）を打ち、名前を付け替えたアセットで安定版 Release を作る |

- どちらも最後に `gh release edit <tag> --prerelease=false --latest` を通し、読み直して外れたことを確かめる。
  **今の Latest より古い版・ドラフト・形の違うタグ**（`-rc.1` 等）は何も変えずに exit 1
  （打ち間違いで cask と docs を古い版へ戻さない）
- **後続 1 = Homebrew cask**（tap `takushio2525/homebrew-tako` の `Casks/tako.rb`）:
  公開アセット `tako-<tag>-macos-arm64.zip` を `gh release download` で実際に落として sha256 を出し、
  GitHub の digest と突き合わせ、cask の `url` の雛形がそのアセットを指すかも確かめてから
  version / sha256 を書き換え、ブランチ `update-<版>` を push → PR → squash merge（tap に CI は無い）。
  **成否は gh の終了コードではなく tap の main を読み直して決める**（#1430 と同じ理由）。
  一時 clone の commit の作者はこのリポの `user.name` / `user.email` を写す
- **後続 2 = docs**（`docs/src/content/docs/releases.md`）: origin/main を使い捨ての worktree
  （`--detach`）に出し、`node docs/scripts/check-releases-page.mjs --set-stable=<版> --date=<日付>` で
  見出しの「— 最新の安定版は vX.Y.Z」と本文の「**安定版は vX.Y.Z**（日付）」を書き換える
  （日付はその版の CHANGELOG の節。書き換えと検査は CI の番犬と同じ 1 ファイル）。ブランチ
  `docs/promote-<tag>` で PR を出し、`scripts/merge-pr.sh` で **CI が緑で揃ってから merge** する
  （main へ直接は push しない。CI の分だけ昇格が待つ）。`merge-pr.sh` が **5**（GitHub API に
  届かない = CI の結論ではない。#1704）で終わったときだけ、CI の赤の「CI が緑になってから」ではなく
  「回線が戻ってから `scripts/merge-pr.sh <PR>`」と案内する（#2498。5 の値は `scripts/lib/gh-retry.sh` の
  `GH_UNREACHABLE_EXIT` を source して引く）。その版の系列の節が無い（minor が
  上がった直後）ときは書き換えずに落ちるので、節（読み物）を書いてから打ち直す
- **終了コード**: 0 = Release・cask・docs が揃った / 1 = 昇格しなかった / **4 = Release は Latest に
  なったが後続が終わっていない**（理由と、どの段かを名指しする）。片肺（Windows が無い）は
  従来どおり報告だけで終了コードは変えない
- **打ち直しは同じコマンド**（`scripts/release.sh --promote <安定版タグ>`）。済んだ段は飛ばす
  （cask が既に同じ版 + 同じ sha256 / docs が既に同じ版なら何もしない）。PR が残っていれば使い回す
- **bash 3.2 の罠**: `set -u` の unbound variable で死ぬと EXIT の trap の中の `$?` が 0 になる。
  昇格の trap は成功の印（`PROMOTE_SUCCEEDED`）が無い 0 を 1 にして抜ける
- **モックテスト**: `bash scripts/test-release-promote-1853.sh`（CI の macOS ジョブ）。gh と
  `merge-pr.sh` はスタブ、origin と tap は一時ディレクトリの bare リポ（tap の clone 元は
  `TAKO_HOMEBREW_TAP_REMOTE` で差し替える）、git のグローバル設定も切り離すので、
  **本番の Release・タグ・tap には触らない**。実地での昇格はテストで走らせない
