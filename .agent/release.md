# リリース運用（詳細）

> `AGENTS.md`「リリース運用」節の**全文**。両 OS 同時リリースの仕組み・配布物に同梱する
> ライセンス・夜間リリース・次回バージョンの予約はここにある。**毎ターンは読まない** —
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
