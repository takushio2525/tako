# リリース運用（詳細）

> `AGENTS.md`「リリース運用」節の**詳細**。両 OS 同時リリースの仕組み・配布物に同梱する
> ライセンス・配布物に個人情報を入れない仕組み・夜間リリース・次回バージョンの予約はここにある。**毎ターンは読まない** —
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
- 待ち合わせ: `release.sh` は Windows の添付を待ってから、実アセットを読み直して
  ノートを作り直す（ダウンロード表 / 動作要件 / Windows 手順 / Known limitations が揃う）
- 片肺の検出: `release.sh` の終了コード **3**（= Release は作られたが揃っていない）。
  公開済みリリースは `scripts/release.sh --check-assets [tag]` でいつでも検査できる。
  判定の正は `tako-core::platform::release_assets`（`missing_platforms` / `is_complete`）で、
  シェル側の写しは同期テストが拘束する。モックテスト `scripts/test-release-retry.sh` は
  **CI の macOS ジョブで毎 PR 走る**（片肺の検出が壊れたらそこで落ちる）
- 動作要件の数値（macOS 11.0 / Windows 10.0.17763）も `release_assets` が正で、
  `tako.iss` の `MinVersion` と `build-app.sh` の `LSMinimumSystemVersion` との一致を
  テストが検証する（ノートの要件と配布物の実際の下限がズレない）

## 配布物に同梱するライセンス（#1709 / #1845）

GPL-3.0 第 4 条・Apache-2.0 第 4 条 (a)・MIT / BSD の表示義務は、バイナリの受け取り手へ
本文と著作権表示を渡すことを求める。**両 OS の配布物が同じ 3 本を持つ**のが正常な状態。

| 元ファイル（リポジトリ直下） | macOS（`tako.app/Contents/Resources/`） | Windows（zip の `tako/`・インストール先） |
|---|---|---|
| `LICENSE` | `LICENSE` | `LICENSE.txt`（メモ帳で開けるように） |
| `THIRD-PARTY-NOTICES.md` | 同名 | 同名 |
| `THIRD-PARTY-LICENSES.md` | 同名 | 同名 |

- 組み立て: macOS = `scripts/build-app.sh`（署名より前に置く）/ Windows の zip =
  `installer/windows/build-installer.ps1` / Windows のインストーラー = `installer/windows/tako.iss`
  の `[Files]`。**3 か所とも 1 本ずつ明示で並べる**（まとめて glob にしない）
- 番犬: `crates/tako-control/tests/license_bundle_watchdog.rs` が組み立ての全箇所と
  リリース検査の表を 1 つの表へ突き合わせ、抜けたら置くべき場所の file:line を名指しして落ちる
- 実物の検査: `installer/windows/lib/verify-assets.ps1` の `$TakoLicenseBundle` が同梱表。
  zip は `Test-TakoWindowsAssets` が展開してリポジトリの元ファイルとのバイト一致を見る
  （CI・実機の両経路）。インストール先は `Test-TakoInstalledPayload` が**使い捨ての CI
  ランナーでだけ**無人インストールして見る（実機で走らせると同じ AppId の実インストールの
  登録を上書きするので、`release-windows.ps1` からは呼ばない。番犬もこれを固定する）
- **Windows の配布物のドライラン**: PR の CI は Windows の配布物を組まない（タグ push 時だけ）。
  組み立てを変えた PR は `gh workflow run release-windows.yml --ref <ブランチ>` で確かめる。
  タグ以外の ref では Cargo.toml の版数で組み立て・検査・artifact 保存（`tako-windows-dryrun-<run id>`）
  までを行い、**Release へは添付しない**。ログの「zip の tako/ の中身」「インストール先の中身」が
  生成物の一覧になる
- `THIRD-PARTY-LICENSES.md` の中身は `cargo about`（`about.toml` / `about.hbs`）で生成する。
  依存が変わったときの再生成は手動（自動化は未着手）

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
- ジョブ登録は `scripts/nightly-release.sh --install-launchd`（解除は `--uninstall-launchd`、
  確認は `launchctl list | grep tako-nightly`）。plist はリポに置かず実行時に生成する
- Homebrew cask 更新・リリースノートの日英併記は従来どおり手動で行う

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
