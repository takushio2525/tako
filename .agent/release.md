# リリース運用（詳細）

> `AGENTS.md`「リリース運用」節の**詳細**。両 OS 同時リリースの仕組み・配布物に同梱する
> ライセンス・配布物に個人情報を入れない仕組み・夜間リリース・次回バージョンの予約・
> 安定版への昇格（Homebrew cask と docs の追従）はここにある。**毎ターンは読まない** —
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
  `claude plugin validate --strict` と `claude plugin test` にかける。合格の条件は終了コード 0 に加えて、
  ゲートになるフックの `.catch` 抜け（validate は注記に出すだけで合格させる = 設計書 §5 の規約違反）が
  無いこと・テストが 1 本以上走ったこと
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
- 終了コード: 0 = 合格 / 1 = 不合格（打ち切り・mod を取り出せないを含む）/ 2 = 引数の誤り / 3 = 未実測
- **launchd の登録は変えない**（既存の `com.takushio.tako-nightly-release` が install_root の
  nightly-release.sh を叩くので、main を pull すれば翌晩から効く）
- 検証は `bash scripts/test-nightly-mod-check-1892.sh`（CI の macOS ジョブ。claude・osascript・release.sh は
  スタブ、HOME も隔離。壊れた登録の A/B・claude 無し・固まる claude・止めない実走・変更なしの夜・
  設定 dir の隔離を見る。実物の claude（下限 2.1.294 以上）があれば本物の mod と壊れた写しも検査し、
  利用者の設定ファイルの mtime が前後で一致することを見る）

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
  （main へ直接は push しない。CI の分だけ昇格が待つ）。その版の系列の節が無い（minor が
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
