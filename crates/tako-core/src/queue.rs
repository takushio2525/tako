//! 重い処理の順番待ち（純関数。FR-2.43.6 / Issue #1994。エピック #1992 の S2）
//!
//! 設計の正は `.agent/plans/2026-10-resource-manager.md`（§4）。
//!
//! ## ここに置くもの・置かないもの
//!
//! 待ち行列の**判断**だけを持つ: 区分（何の処理か）と見込みメモリ・優先度と並び・
//! 上限の実効値（`settings.json` の `resources` 節 + 機に合わせた既定）・割り当て（[`admit`]）・
//! 取り分（`CARGO_BUILD_JOBS`）・停滞の判定・cargo の target dir の推定。
//! 調停役の状態（誰が待っていて誰が走っているか）と IPC は `tako_control::queue`、
//! 子を起こして待つ側（wrapper）は `tako queue <コマンド>`（tako-cli）が持つ。
//! OS 分岐を持たないので、境界値は macOS 上の単体テストで固定できる。
//!
//! ## 割り当ての条件（設計書 §4.3）
//!
//! 並びは「優先度の高い順 → 受付の古い順」で、並びの先頭から 1 件ずつ上から順に見る:
//!
//! 1. 同じ target dir の処理が走っている（停滞中も含む = cargo のロックを持っている）→ 待つ
//! 2. メモリ圧 critical → 待つ（走っているものは止めない）
//! 3. **最低限の前進**: 管理下の実行中（停滞を除く）が 0 件なら、ここで入れる
//!    （メモリ・外の処理・warn の条件が欠けていても。外の負荷だけで永久に待たないため）。
//!    agent の起動（#2413。[`AdmitInput::min_progress`] = `false`）はこの段を飛ばす:
//!    常駐は終わらないので、逼迫中に 1 本ずつでも足すと戻らない
//! 4. メモリ圧 warn（または swap が `swap_guard` 以上）→ 待つ（= 実行中が 0 件のときだけ 1 件）
//! 5. 実行中（停滞を除く）+ tako の外の重い処理 ≥ `max_jobs` → 待つ
//! 6. 空き − （走り出したばかりの処理がまだ使っていない見込み）− 見込み < 予約 → 待つ
//!
//! 機の値が採れない（Windows は #2000 まで）ときは 2・4・6 を飛ばし、同時数だけで決める。
//!
//! ## agent の起動待ち（#2413）
//!
//! spawn / run・再起動後の復元が起こす agent も**同じ [`admit`]** で待たせる（方針を 2 か所に書かない）。
//! 呼ぶのは `tako_control::launch_gate` で、違いは入力だけ: 最低限の前進なし（上の 3）・同時数の上限なし・
//! 実行中 = 立ち上がり中の agent（見込みは [`JobKind::Agent`] の過去の記録）・復元は 1 本の列
//! （target dir の段 1 を「同時に 1 本」の列として使う。spawn は列の待ちを追い越せる）。
//! メモリ圧 warn / critical・swap・空きの予約で待つのは重い処理と同じ段（2・4・6）
//!
//! ## 見込みと「まだ使っていない見込み」（#2254）
//!
//! - **見込み**（1 件の木の最大メモリ）は過去の記録（[`HistoryRow`]。調停役が
//!   `<data_dir>/resources/history.jsonl` に持つ）から引く: 同じコマンドの直近
//!   [`HISTORY_EXACT_RUNS`] 回の最大 → 無ければ同じ形（cargo の `--test <名前>` 等を伏せた形）の直近
//!   [`HISTORY_SHAPE_RUNS`] 回の最大（[`HISTORY_SHAPE_MIN`] 回以上あるとき）→ それも無ければ区分の既定
//!   （[`estimate`]）。小さい `cargo test --test …` に test の既定 7 GB を当てはめない
//! - 6 の「まだ使っていない見込み」は**走り出してからピークまで**の分（[`RunningJob::pending_mb`]）。
//!   ピークを過ぎた処理（見込みに届いた / 過去の記録の山の時刻と自分の最大から [`PEAK_GRACE_SECS`] 過ぎた）は
//!   0 にする。以前は「見込み − 今の使用量」を最後まで押さえ続け、ピーク後に数百 MB しか使っていない
//!   `cargo test --workspace` が 7 GB 近くを抱えたまま後ろを 30 分止めた（#2254）。空きは実測なので、
//!   ピーク後に実際に使っている分は空きから既に減っている。メモリ圧・swap・予約の歯止め（2・4・6）は変えない
//!
//! ## 追い越し（#2243）
//!
//! 2〜6（枠・メモリ）で入れない件があれば**そこで止まり、後ろも待つ**（資源の追い越しは無し）。
//! 1（target dir）だけで入れない件は、その dir が空くまでどのみち走れないので**後ろに追い越させる**
//! （以前は先頭が target dir 待ちのあいだ、別の target dir の後ろの処理まで空き枠の前で待っていた）:
//!
//! - 追い越す側は自分で 1〜6 をすべて通る（今回入れた分も実行中に数える）
//! - 追い越された件と同じ target dir の後ろの件は追い越さない（同じ dir の中の順番は保つ）
//! - 1 件の待ちを追い越せるのは累計 [`max_overtaken`]（同時数の [`OVERTAKE_ROUNDS`] 巡ぶん）まで。
//!   達したらその件で止まる（= 以前と同じく後ろも待つ）
//!
//! 先頭は飢えない: その dir が空いた時点で、先頭は入るか 2〜6 で止まる（= 後ろはもう追い越せない）ので、
//! 空いた後に先頭より先に入る処理は無い。追い越しで走り出した処理は、先頭が走れない間の空き枠を使うだけ。

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::resources::{MachineStats, Pressure, MIB};

/// 1 GiB を MiB で
const GIB_MB: u64 = 1024;

/// 待ちの上限の既定（分）。切れたら wrapper は 75（EX_TEMPFAIL）で終わる
pub const DEFAULT_MAX_WAIT_MIN: u32 = 30;
/// 実行中で木の CPU がこの値（%）未満の状態が [`STALL_AFTER_SECS`] 続いたら停滞
pub const STALL_CPU_PERCENT: f64 = 1.0;
/// tako の外（順番待ちを通っていない）の重い処理は、木の CPU がこの値（%）以上のときだけ 1 枠に数える。
/// 休んでいる dev サーバー（vite / next）や watch（tsc --watch）が常に枠を食うのを避ける
/// （管理下の処理の「停滞は枠を返す」と同じ考え方。ビルドやテストは 1 コア = 100% を超えて走る）
pub const UNMANAGED_ACTIVE_CPU_PERCENT: f64 = 10.0;
/// 停滞とみなすまでの長さ（秒）
pub const STALL_AFTER_SECS: u64 = 600;
/// wrapper が終了コードとして返す値: 待ちの上限で諦めた（EX_TEMPFAIL）
pub const EXIT_MAX_WAIT: i32 = 75;
/// wrapper が終了コードとして返す値: 待っている間に取り消された
pub const EXIT_CANCELLED: i32 = 125;
/// wrapper が終了コードとして返す値: 子を起動できなかった（`sh` と同じ）
pub const EXIT_SPAWN_FAILED: i32 = 127;

// ===== 区分と見込み =====

/// 順番待ちに入る処理の区分（見込みメモリの既定を決める）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JobKind {
    /// cargo などのビルド（シェルスクリプトもここ = 順番待ちへ通すのはビルドや検証の入口）
    Build,
    /// cargo のテスト（`cargo test` / `nextest` / `bench`。テストの実行体の並走でビルドの約 2 倍食う）
    Test,
    /// playwright（Chromium を起こす）
    Browser,
    /// node のビルド系（npm / npx / pnpm / vite …）
    Node,
    /// 隔離の tako-app
    Gui,
    /// agent の起動（#2413。spawn / run / 復元が起こす claude 等）。`tako queue` の wrapper は送らない
    /// （[`classify`] は返さない）。起動待ち（`tako_control::launch_gate`）が見込みと過去の記録に使う
    Agent,
    #[default]
    Other,
}

impl JobKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Build => "build",
            Self::Test => "test",
            Self::Browser => "browser",
            Self::Node => "node",
            Self::Gui => "gui",
            Self::Agent => "agent",
            Self::Other => "other",
        }
    }

    /// 1 件の見込みメモリ（MiB）。過去の記録（#1996）が無いときの既定。
    ///
    /// build / test は tako 自身の `cargo build --workspace` / `cargo test --workspace` の木の
    /// 最大メモリの実測（取り分 6 = 18 コアで 3 件並走のとき）で決めた。根拠は設計書 §4.7
    pub fn default_estimate_mb(self) -> u64 {
        match self {
            Self::Build => BUILD_ESTIMATE_MB,
            Self::Test => TEST_ESTIMATE_MB,
            Self::Browser => 2 * GIB_MB,
            Self::Agent => AGENT_ESTIMATE_MB,
            Self::Node | Self::Gui | Self::Other => GIB_MB,
        }
    }
}

/// agent 1 本の見込みメモリ（MiB）。過去の記録（立ち上がりの最大）が無いときの既定。
/// claude 本体 + 子孫（MCP サーバー）の RSS の中央値 596 MB（2026-10-12 の実測。設計書 §1.3）を
/// [`HISTORY_STEP_MB`] 刻みへ切り上げた
pub const AGENT_ESTIMATE_MB: u64 = 5 * HISTORY_STEP_MB;

/// build の見込みメモリ（MiB）。tako-core を触った後の `cargo build --workspace`（取り分 6）の
/// 木の最大 3,430 MB（2026-10-15 の実測。設計書 §4.7）を 0.5 GB 単位へ切り上げた
pub const BUILD_ESTIMATE_MB: u64 = 3 * GIB_MB + GIB_MB / 2;
/// test の見込みメモリ（MiB）。`cargo test --workspace -- --test-threads=4`（取り分 6）の
/// 木の最大 6,834 MB（2026-10-15 の実測。設計書 §4.7）を切り上げた
pub const TEST_ESTIMATE_MB: u64 = 7 * GIB_MB;

// テストはビルドより食う（実測）。見込みを入れ替えて逆転させたらコンパイルで落とす
const _: () = assert!(TEST_ESTIMATE_MB > BUILD_ESTIMATE_MB);

/// cargo のサブコマンドのうちテストの実行体を走らせるもの
const CARGO_TEST_SUBCOMMANDS: &[&str] = &["test", "t", "nextest", "bench"];

/// argv の第 1 語の basename
fn base(word: &str) -> &str {
    word.rsplit(['/', '\\']).next().unwrap_or(word)
}

/// 前置きのコマンド（`env FOO=1 cargo …` / `nice -n 5 cargo …` / `time cargo …`）を飛ばして
/// 本体の位置を返す
fn command_start(argv: &[String]) -> usize {
    let mut i = 0;
    while i < argv.len() {
        match base(&argv[i]) {
            "env" => {
                i += 1;
                while i < argv.len() && (argv[i].contains('=') || argv[i].starts_with('-')) {
                    i += 1;
                }
            }
            "nice" => {
                i += 1;
                if i < argv.len() && argv[i] == "-n" {
                    i += 2;
                } else if i < argv.len() && argv[i].starts_with('-') {
                    i += 1;
                }
            }
            "time" | "command" | "nohup" => i += 1,
            _ => return i,
        }
    }
    argv.len()
}

const BUILD_PROGRAMS: &[&str] = &[
    "cargo",
    "rustc",
    "rustup",
    "make",
    "gmake",
    "ninja",
    "cmake",
    "xcodebuild",
    "swift",
    "swiftc",
    "clang",
    "clang++",
    "cc",
    "c++",
    "gcc",
    "g++",
];

const SHELLS: &[&str] = &["bash", "sh", "zsh", "dash", "ksh", "pwsh", "powershell"];

const NODE_PROGRAMS: &[&str] = &["node", "npm", "npx", "pnpm", "yarn", "bun", "deno"];

/// 走らせるコマンドの区分（純関数）
pub fn classify(argv: &[String]) -> JobKind {
    let rest = &argv[command_start(argv)..];
    let Some(first) = rest.first() else {
        return JobKind::Other;
    };
    let b = base(first);
    if b == "cargo" {
        // サブコマンド = `+toolchain` とオプションの後の最初の語
        let sub = rest[1..]
            .iter()
            .find(|w| !w.starts_with('-') && !w.starts_with('+'))
            .map(String::as_str);
        if sub.is_some_and(|s| CARGO_TEST_SUBCOMMANDS.contains(&s)) {
            return JobKind::Test;
        }
        return JobKind::Build;
    }
    if b == "cargo-nextest" {
        return JobKind::Test;
    }
    if BUILD_PROGRAMS.contains(&b) || b.starts_with("cargo-") {
        return JobKind::Build;
    }
    if SHELLS.contains(&b) || b.ends_with(".sh") {
        return JobKind::Build;
    }
    if b == "tako-app" {
        return JobKind::Gui;
    }
    if NODE_PROGRAMS.contains(&b) || b == "playwright" {
        if rest.iter().any(|w| w.contains("playwright")) {
            return JobKind::Browser;
        }
        return JobKind::Node;
    }
    JobKind::Other
}

/// cargo の target dir を推定する（純関数。`is_manifest` は「そのディレクトリに Cargo.toml が
/// あるか・それが `[workspace]` を持つか」を返す口 = テストで差し替える）。
///
/// - `CARGO_TARGET_DIR` があればそれ（相対なら `cwd` から）
/// - 無ければ `cwd` から上へ辿り、`[workspace]` を持つ一番上の Cargo.toml の隣の `target`。
///   workspace が無ければ一番近い Cargo.toml の隣
/// - cargo の外（Cargo.toml が無い）なら `None`
///
/// 区分が build でない処理は呼び手が呼ばない（同じ repo の npm と cargo を直列にしない）
pub fn cargo_target_dir(
    cwd: &Path,
    env_target: Option<&str>,
    is_manifest: impl Fn(&Path) -> Option<bool>,
) -> Option<PathBuf> {
    if let Some(t) = env_target.filter(|t| !t.is_empty()) {
        let p = Path::new(t);
        return Some(if p.is_absolute() {
            p.to_path_buf()
        } else {
            cwd.join(p)
        });
    }
    let mut nearest: Option<&Path> = None;
    let mut workspace: Option<&Path> = None;
    for dir in cwd.ancestors() {
        match is_manifest(dir) {
            Some(true) => {
                nearest.get_or_insert(dir);
                workspace = Some(dir);
            }
            Some(false) => {
                nearest.get_or_insert(dir);
            }
            None => {}
        }
    }
    workspace.or(nearest).map(|d| d.join("target"))
}

// ===== 過去の記録（#2254。置き場と鍵は #1996 の「過去の所要時間」と同じ = 設計書 §6）=====

/// 見込みを引く同じコマンドの記録の数（直近）
pub const HISTORY_EXACT_RUNS: usize = 5;
/// 見込みを引く同じ形の記録の数（直近）
pub const HISTORY_SHAPE_RUNS: usize = 10;
/// 同じ形の記録から見込みを引くのに要る数（1 回だけだと形の中の軽い 1 本に引きずられる）
pub const HISTORY_SHAPE_MIN: usize = 2;
/// 過去の記録から引いた見込みの下限（MiB）。走査の 2 秒より短い処理の最大は数 MB に見える
pub const HISTORY_FLOOR_MB: u64 = 256;
/// 過去の記録から引いた見込みの刻み（MiB。切り上げ）
pub const HISTORY_STEP_MB: u64 = 128;

/// wrapper が送る過去の記録の鍵。**生の argv とパスは持たない**（ハッシュと表示名だけ）
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct HistoryKeys {
    /// 同じコマンド = 「repo の根 + 正規化した argv」のハッシュ（16 桁の 16 進）
    pub key: String,
    /// 同じ形 = cargo の `--test <名前>`・テストの絞り込みなど回ごとに変わる語を伏せた argv のハッシュ
    /// （cargo だけ。シェルスクリプトや node は中身が軽重さまざまなので形でまとめない）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shape: Option<String>,
    /// 表示名（プログラム + サブコマンド = `cargo test`。cargo 以外はプログラムの名前だけ）
    #[serde(default)]
    pub name: String,
}

/// `.git` の在り方（[`repo_identity`] の口）
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GitEntry {
    /// ディレクトリ（ふつうの clone）
    Dir,
    /// ファイル（worktree / submodule。中身は `gitdir: <path>` の 1 行）
    File(String),
}

/// 過去の記録の鍵に使う repo の根（純関数。`git_entry` は「そのディレクトリの `.git` の在り方」を返す口 =
/// テストで差し替える）。cwd から上へ辿って最初に `.git` があるディレクトリ。git の worktree
/// （`.git` がファイルで `gitdir: <本体>/.git/worktrees/<名前>`）は**本体の根へ寄せる**（worker が Issue ごとに
/// 作る worktree で記録を引き継ぐため）。git の外なら cwd
pub fn repo_identity(cwd: &Path, git_entry: impl Fn(&Path) -> Option<GitEntry>) -> PathBuf {
    for dir in cwd.ancestors() {
        match git_entry(dir) {
            Some(GitEntry::Dir) => return dir.to_path_buf(),
            Some(GitEntry::File(text)) => {
                return text
                    .lines()
                    .find_map(|l| l.trim().strip_prefix("gitdir:"))
                    .and_then(|p| worktree_main_root(&dir.join(p.trim())))
                    .unwrap_or_else(|| dir.to_path_buf());
            }
            None => {}
        }
    }
    cwd.to_path_buf()
}

/// `<本体>/.git/worktrees/<名前>` → `<本体>`（worktree の gitdir でなければ `None`）
fn worktree_main_root(gitdir: &Path) -> Option<PathBuf> {
    let comps: Vec<_> = gitdir.components().collect();
    let i = comps
        .windows(2)
        .rposition(|w| w[0].as_os_str() == ".git" && w[1].as_os_str() == "worktrees")?;
    let root: PathBuf = comps[..i].iter().collect();
    (!root.as_os_str().is_empty()).then_some(root)
}

/// cargo のオプションのうち値を次の語に取るもの（形を作るときに値を語として数えない）
const CARGO_VALUE_FLAGS: &[&str] = &[
    "-p",
    "--package",
    "--test",
    "--bench",
    "--example",
    "--bin",
    "--features",
    "-F",
    "--target",
    "--target-dir",
    "--manifest-path",
    "-j",
    "--jobs",
    "--profile",
    "--color",
    "--message-format",
    "--config",
    "-Z",
    "--exclude",
];

/// そのうち形では値を伏せるもの（回ごとに変わる名前・パス）。`-p` の値は伏せない
/// （パッケージで重さが大きく違う = `-p tako-app` と `-p tako-cli` を同じ形にしない）
const CARGO_MASKED_FLAGS: &[&str] = &[
    "--test",
    "--bench",
    "--example",
    "--bin",
    "--target-dir",
    "--manifest-path",
    "--config",
];

/// cargo の引数（サブコマンド以降も含む `cargo` の後ろ全部）の形とサブコマンド。
/// 値を伏せるオプションの値・2 語目以降の位置引数（テストの絞り込み）・`--` の後ろの位置引数を `*` にする
fn cargo_shape(args: &[String]) -> (Vec<String>, Option<String>) {
    let mut out = Vec::with_capacity(args.len());
    let mut sub: Option<String> = None;
    let mut after_dashdash = false;
    let mut i = 0;
    while i < args.len() {
        let w = args[i].as_str();
        i += 1;
        if after_dashdash {
            out.push(if w.starts_with('-') {
                w.into()
            } else {
                "*".into()
            });
            continue;
        }
        if w == "--" {
            after_dashdash = true;
            out.push(w.into());
            continue;
        }
        if let Some((flag, _)) = w.split_once('=').filter(|_| w.starts_with("--")) {
            out.push(if CARGO_MASKED_FLAGS.contains(&flag) {
                format!("{flag}=*")
            } else {
                w.into()
            });
            continue;
        }
        if w.starts_with('-') || w.starts_with('+') {
            out.push(w.into());
            if CARGO_VALUE_FLAGS.contains(&w) {
                if let Some(v) = args.get(i) {
                    out.push(if CARGO_MASKED_FLAGS.contains(&w) {
                        "*".into()
                    } else {
                        v.clone()
                    });
                    i += 1;
                }
            }
            continue;
        }
        if sub.is_none() {
            sub = Some(w.into());
            out.push(w.into());
        } else {
            out.push("*".into());
        }
    }
    (out, sub)
}

fn hash_words<S: AsRef<str>>(repo: &str, words: &[S]) -> String {
    let mut buf = Vec::with_capacity(repo.len() + words.len() * 16);
    buf.extend_from_slice(repo.as_bytes());
    for w in words {
        buf.push(0);
        buf.extend_from_slice(w.as_ref().as_bytes());
    }
    format!("{:016x}", crate::fnv::fnv1a64(&buf))
}

/// 過去の記録の鍵（純関数）。`repo` は [`repo_identity`] の値。argv は前置き（`env` / `nice` / `time`）を
/// 飛ばし、プログラムは basename に畳んでから数える（`/usr/local/bin/cargo` と `cargo` は同じコマンド）。
/// 走らせるものが無ければ `None`
pub fn history_keys(argv: &[String], repo: &str) -> Option<HistoryKeys> {
    let rest: &[String] = &argv[command_start(argv)..];
    let program = base(rest.first()?);
    let mut words: Vec<&str> = vec![program];
    words.extend(rest[1..].iter().map(String::as_str));
    let key = hash_words(repo, &words);
    if program != "cargo" {
        return Some(HistoryKeys {
            key,
            shape: None,
            name: program.into(),
        });
    }
    let (mut shape, sub) = cargo_shape(&rest[1..]);
    shape.insert(0, program.into());
    Some(HistoryKeys {
        key,
        shape: Some(hash_words(repo, &shape)),
        name: sub.map_or_else(|| program.to_string(), |s| format!("{program} {s}")),
    })
}

/// agent 1 本の過去の記録の鍵（#2413。純関数）。同じ系統（claude / codex / agy …）の立ち上がりの最大を引く。
/// 形（`shape`）は持たない = 系統をまたいで見込みを借りない（系統ごとに常駐の大きさが違う）
pub fn agent_history_keys(agent: &str) -> HistoryKeys {
    HistoryKeys {
        key: hash_words("agent", &[agent]),
        shape: None,
        name: agent.to_string(),
    }
}

/// 過去の記録の 1 行（`<data_dir>/resources/history.jsonl`。調停役が終わった処理ごとに 1 行足す）。
/// **生の argv とパスは書かない**（鍵はハッシュ、表示名はプログラム + サブコマンドだけ）。
/// 欄を足すときは serde の default で旧い行を読めるようにする（読めない行は飛ばす）
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct HistoryRow {
    /// 終わった時刻（Unix 秒）
    #[serde(default)]
    pub at: u64,
    pub key: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shape: Option<String>,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub kind: JobKind,
    /// 木の最大メモリ（MiB。調停役の 2 秒ごとの走査の最大）
    pub peak_mb: u64,
    /// 最大になった時点（走り出してからの秒数）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub peak_at_s: Option<u64>,
    /// 走った長さ（秒）
    #[serde(default)]
    pub ran_s: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
}

/// 見込みの出どころ
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum EstimateFrom {
    /// 区分の既定（記録が無い）
    #[default]
    Default,
    /// 同じコマンドの記録
    History,
    /// 同じ形の記録
    Shape,
}

impl EstimateFrom {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Default => "default",
            Self::History => "history",
            Self::Shape => "shape",
        }
    }
}

/// 1 件の見込み
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Estimate {
    /// 見込みメモリ（MiB）
    pub mb: u64,
    pub from: EstimateFrom,
    /// 見込みを引いた記録の数（既定なら 0）
    pub runs: u32,
    /// 記録の中で最大になった時点の最大（走り出してからの秒数。[`RunningJob::expected_peak_at_s`]）
    pub peak_at_s: Option<u64>,
}

impl Estimate {
    /// 区分の既定
    pub fn default_for(kind: JobKind) -> Self {
        Self {
            mb: kind.default_estimate_mb(),
            from: EstimateFrom::Default,
            runs: 0,
            peak_at_s: None,
        }
    }
}

/// 1 件の見込み（純関数）: 同じコマンドの直近 [`HISTORY_EXACT_RUNS`] 回の最大 → 同じ形の直近
/// [`HISTORY_SHAPE_RUNS`] 回の最大（[`HISTORY_SHAPE_MIN`] 回以上）→ 区分の既定。
/// `history` は古い順（ファイルへ足した順）。最大 0 の行（走査の前に終わった）は数えない
pub fn estimate(kind: JobKind, keys: Option<&HistoryKeys>, history: &[HistoryRow]) -> Estimate {
    let Some(keys) = keys.filter(|k| !k.key.is_empty()) else {
        return Estimate::default_for(kind);
    };
    let exact: Vec<&HistoryRow> = history
        .iter()
        .rev()
        .filter(|r| r.peak_mb > 0 && r.key == keys.key)
        .take(HISTORY_EXACT_RUNS)
        .collect();
    if !exact.is_empty() {
        return from_rows(&exact, EstimateFrom::History);
    }
    if let Some(shape) = keys.shape.as_deref() {
        let rows: Vec<&HistoryRow> = history
            .iter()
            .rev()
            .filter(|r| r.peak_mb > 0 && r.shape.as_deref() == Some(shape))
            .take(HISTORY_SHAPE_RUNS)
            .collect();
        if rows.len() >= HISTORY_SHAPE_MIN {
            return from_rows(&rows, EstimateFrom::Shape);
        }
    }
    Estimate::default_for(kind)
}

fn from_rows(rows: &[&HistoryRow], from: EstimateFrom) -> Estimate {
    let peak = rows.iter().map(|r| r.peak_mb).max().unwrap_or(0);
    Estimate {
        mb: (peak.div_ceil(HISTORY_STEP_MB) * HISTORY_STEP_MB).max(HISTORY_FLOOR_MB),
        from,
        runs: rows.len() as u32,
        peak_at_s: rows.iter().filter_map(|r| r.peak_at_s).max(),
    }
}

// ===== 優先度 =====

/// 優先度（S2 は並びに効くだけ。変更・エイジング・役割の既定は #1996）
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Priority {
    /// 人が直接
    Top,
    /// master / solo
    High,
    #[default]
    Normal,
    /// ペインの外（夜間リリース等）
    Low,
}

impl Priority {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Top => "top",
            Self::High => "high",
            Self::Normal => "normal",
            Self::Low => "low",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "top" => Some(Self::Top),
            "high" => Some(Self::High),
            "normal" => Some(Self::Normal),
            "low" => Some(Self::Low),
            _ => None,
        }
    }
}

// ===== wrapper と調停役の間の 1 件 =====

/// wrapper（`tako queue <コマンド>`）が調停役へ送る 1 件。
///
/// **正本は wrapper**: 毎回の poll がこの全体を送る（upsert）ので、調停役が落ちて
/// 立て直しても、元の受付時刻（待ち）と子の pid（実行中）で表を組み直せる。
/// 生の argv とパスは送らない（`label` は [`crate::resources::short_label`] 済み）。
/// `target_dir` は同じ dir の判定にだけ使い、status には出さない
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct JobWire {
    /// 受付番号 = wrapper の pid（世代をまたいで衝突しない）
    pub id: u64,
    /// 受付の時刻（Unix ミリ秒。wrapper が持ち続ける）
    pub enqueued_at_ms: u64,
    #[serde(default)]
    pub label: String,
    #[serde(default)]
    pub kind: JobKind,
    #[serde(default)]
    pub priority: Priority,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_dir: Option<String>,
    /// 呼んだペイン（`TAKO_PANE_ID`）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pane: Option<u64>,
    /// 走り出した子の pid（実行中の登録し直しで使う）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub child_pid: Option<u32>,
    /// 走り出した時刻（Unix ミリ秒）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_at_ms: Option<u64>,
    /// 子の終了コード（done のとき。シグナルで終わったら 128 + 番号）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
    /// 過去の記録の鍵（#2254。ハッシュと表示名だけ。旧い wrapper は送らない = 区分の既定）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub history: Option<HistoryKeys>,
}

// ===== 上限 =====

/// `settings.json` の `resources` 節（FR-2.43.6）。`None` = 機に合わせた既定。
///
/// **旧ファイルはキーが無くても読める**（節ごと `#[serde(default)]`、各キーも `Option`）
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct LimitSettings {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_jobs: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reserve_gb: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub swap_guard_gb: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_wait_min: Option<u32>,
    /// cargo の共有 jobserver（#1998 = S6）。`None` = 既定の on（判断点 §11-5）。`Some(false)` で作らず注入しない
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub jobserver: Option<bool>,
}

/// `tako queue limits` / MCP `tako_queue`（`action: "limits"`）の変更
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct LimitsPatch {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_jobs: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reserve_gb: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub swap_guard_gb: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_wait_min: Option<u32>,
    /// cargo の共有 jobserver の on / off（#1998）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub jobserver: Option<bool>,
    /// すべて既定（機に合わせた値）へ戻す。他の項目と一緒なら、戻してから当てる
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub reset: bool,
}

impl LimitsPatch {
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }
}

/// 上限の値の範囲（範囲外は理由つきで弾く）
pub const MAX_JOBS_RANGE: (u32, u32) = (1, 64);
pub const MAX_WAIT_RANGE: (u32, u32) = (1, 24 * 60);
pub const GB_RANGE: (f64, f64) = (0.0, 1024.0);

impl LimitSettings {
    /// cargo の共有 jobserver を使うか（キーが無ければ既定の on）
    pub fn jobserver_on(&self) -> bool {
        self.jobserver != Some(false)
    }

    /// 変更を当てる（範囲外は何も変えずに理由を返す）
    pub fn apply(&mut self, patch: &LimitsPatch) -> Result<(), String> {
        let mut next = if patch.reset {
            Self::default()
        } else {
            self.clone()
        };
        if let Some(n) = patch.max_jobs {
            if !(MAX_JOBS_RANGE.0..=MAX_JOBS_RANGE.1).contains(&n) {
                return Err(format!(
                    "max_jobs は {}〜{}（受け取った値: {n}）",
                    MAX_JOBS_RANGE.0, MAX_JOBS_RANGE.1
                ));
            }
            next.max_jobs = Some(n);
        }
        for (name, value, slot) in [
            ("reserve_gb", patch.reserve_gb, &mut next.reserve_gb),
            (
                "swap_guard_gb",
                patch.swap_guard_gb,
                &mut next.swap_guard_gb,
            ),
        ] {
            if let Some(v) = value {
                if !v.is_finite() || !(GB_RANGE.0..=GB_RANGE.1).contains(&v) {
                    return Err(format!(
                        "{name} は {}〜{} GB（受け取った値: {v}）",
                        GB_RANGE.0, GB_RANGE.1
                    ));
                }
                *slot = Some(v);
            }
        }
        if let Some(m) = patch.max_wait_min {
            if !(MAX_WAIT_RANGE.0..=MAX_WAIT_RANGE.1).contains(&m) {
                return Err(format!(
                    "max_wait_min は {}〜{} 分（受け取った値: {m}）",
                    MAX_WAIT_RANGE.0, MAX_WAIT_RANGE.1
                ));
            }
            next.max_wait_min = Some(m);
        }
        if let Some(on) = patch.jobserver {
            // 既定（on）へ戻すときはキーを消す（既定のままなら設定ファイルへ書かない）
            next.jobserver = (!on).then_some(false);
        }
        *self = next;
        Ok(())
    }
}

/// 上限の実効値
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    pub max_jobs: u32,
    /// 空きメモリの予約（MiB）
    pub reserve_mb: u64,
    /// swap の使用がこれ以上なら warn 扱い（MiB）
    pub swap_guard_mb: u64,
    pub max_wait_min: u32,
}

/// 機の RAM（MiB）から既定の同時数: RAM 12 GB あたり 1 件（1〜4）。RAM が分からなければ 2
pub fn default_max_jobs(total_mb: Option<u64>) -> u32 {
    match total_mb {
        Some(t) => ((t / (12 * GIB_MB)) as u32).clamp(1, 4),
        None => 2,
    }
}

/// 機の RAM（MiB）から既定の予約: max(2 GB, RAM の 10%)
pub fn default_reserve_mb(total_mb: Option<u64>) -> u64 {
    let floor = 2 * GIB_MB;
    total_mb.map_or(floor, |t| (t / 10).max(floor))
}

/// swap の既定の警戒線（2 GB。事故は 6.8 GB・8.4 GB、普段は 0）
pub const DEFAULT_SWAP_GUARD_MB: u64 = 2 * GIB_MB;

fn gb_to_mb(gb: f64) -> u64 {
    (gb * GIB_MB as f64).round().max(0.0) as u64
}

/// 設定と機の RAM から実効値を組む（純関数）
pub fn effective_limits(settings: &LimitSettings, total_mb: Option<u64>) -> Limits {
    Limits {
        max_jobs: settings
            .max_jobs
            .unwrap_or_else(|| default_max_jobs(total_mb)),
        reserve_mb: settings
            .reserve_gb
            .map(gb_to_mb)
            .unwrap_or_else(|| default_reserve_mb(total_mb)),
        swap_guard_mb: settings
            .swap_guard_gb
            .map(gb_to_mb)
            .unwrap_or(DEFAULT_SWAP_GUARD_MB),
        max_wait_min: settings.max_wait_min.unwrap_or(DEFAULT_MAX_WAIT_MIN),
    }
}

// ===== 割り当て =====

/// 割り当ての判断に使う機の値（MiB）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct MachineView {
    pub total_mb: u64,
    /// 空き（Activity Monitor の式。全体 − 使用）。採れなければ `None`
    pub available_mb: Option<u64>,
    pub pressure: Option<Pressure>,
    pub swap_used_mb: Option<u64>,
}

/// 採った機の値を割り当ての判断の形へ（純関数。順番待ちの調停役と agent の起動待ち = #2413 が同じ 1 本を通す）。
/// メモリの内訳が採れなければ `None`（= 同時数だけで決める）
pub fn machine_view(stats: &MachineStats) -> Option<MachineView> {
    stats.memory.map(|m| MachineView {
        total_mb: m.total / MIB,
        available_mb: Some(m.available / MIB),
        pressure: stats.pressure,
        swap_used_mb: stats.swap.map(|(used, _)| used / MIB),
    })
}

/// 実行中の 1 件
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RunningJob {
    pub id: u64,
    /// 見込みメモリ（MiB）
    pub estimate_mb: u64,
    /// いま木が使っているメモリ（MiB。走査がまだなら `None` = 見込みを丸ごとまだ使っていない扱い）
    pub used_mb: Option<u64>,
    pub target_dir: Option<String>,
    /// 停滞中（枠を返している。同時数とメモリの予約に数えない）
    pub stalled: bool,
    /// 走り出してからの木の最大（MiB。走査がまだなら 0。#2254）
    pub peak_mb: u64,
    /// 走り出してからの秒数（#2254）
    pub ran_s: u64,
    /// 自分の最大を最後に更新した時点（走り出してからの秒数。走査がまだなら `None`。#2254）
    pub peak_seen_s: Option<u64>,
    /// 過去の記録で最大になった時点（走り出してからの秒数の、記録の中の最大。記録が無ければ `None`。#2254）
    pub expected_peak_at_s: Option<u64>,
}

/// ピークを過ぎたとみなすまでの猶予（秒。#2254）。走査は 2 秒ごとなので、過去の山の時刻の揺れと
/// 取りこぼしを覆う長さにする
pub const PEAK_GRACE_SECS: u64 = 60;

impl RunningJob {
    /// ピークを過ぎたか（#2254）: 見込みに届いた、または過去の記録の山の時刻から [`PEAK_GRACE_SECS`] 過ぎて
    /// 自分の最大の更新からも同じだけ過ぎた（= 過去にその後で山が来た回が無く、自分も伸びていない）。
    /// 走査がまだ（最大 0）なら過ぎていない
    pub fn peak_passed(&self) -> bool {
        if self.peak_mb == 0 {
            return false;
        }
        if self.peak_mb >= self.estimate_mb {
            return true;
        }
        let past_history = self
            .expected_peak_at_s
            .is_some_and(|h| self.ran_s >= h.saturating_add(PEAK_GRACE_SECS));
        let past_own = self
            .peak_seen_s
            .is_some_and(|p| self.ran_s >= p.saturating_add(PEAK_GRACE_SECS));
        past_history && past_own
    }

    /// まだ使っていない見込み（MiB）: 走り出してからピークまでは「見込み − 今の使用量」、ピークを過ぎたら 0。
    /// `release_after_peak = false` は修正前（最後まで「見込み − 今の使用量」= #2254 の A/B）
    pub fn pending_mb(&self, release_after_peak: bool) -> u64 {
        if release_after_peak && self.peak_passed() {
            return 0;
        }
        self.estimate_mb.saturating_sub(self.used_mb.unwrap_or(0))
    }
}

/// 待ちの 1 件
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WaitingJob {
    pub id: u64,
    pub estimate_mb: u64,
    pub target_dir: Option<String>,
    pub priority: Priority,
    pub enqueued_at_ms: u64,
    /// これまでに追い越された件数（#2243。調停役がメモリに持つ = 立て直すと 0 から）
    pub overtaken: u32,
}

/// 1 件の待ちを追い越せる累計 = 同時数の何巡ぶんか（#2243）
pub const OVERTAKE_ROUNDS: u32 = 2;

/// 1 件の待ちを追い越せる累計の上限（同時数 × [`OVERTAKE_ROUNDS`]。36 GB の機の既定で 6 件）
pub fn max_overtaken(limits: &Limits) -> u32 {
    limits.max_jobs.max(1).saturating_mul(OVERTAKE_ROUNDS)
}

/// 先頭が待つ理由
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WaitReason {
    /// 調停役が立ち上げ直後で、実行中・待ちの登録し直しを待っている
    Regather,
    /// 同じ target dir の処理が走っている
    TargetDir,
    PressureCritical,
    PressureWarn,
    /// swap の使用が警戒線以上（warn 扱い）
    Swap {
        used_mb: u64,
    },
    /// 同時数の上限（tako の外の重い処理も 1 件 = 1 枠）
    MaxJobs {
        running: u32,
        unmanaged: u32,
        max: u32,
    },
    /// 空きメモリが足りない（`need_mb` = `pending_mb` + `estimate_mb`）
    Memory {
        available_mb: u64,
        need_mb: u64,
        reserve_mb: u64,
        /// 実行中の処理がまだ使っていない見込みの合計（#2254）
        pending_mb: u64,
        /// この処理の見込み（#2254）
        estimate_mb: u64,
    },
    /// 自分より前に並んでいる処理の後
    Ahead,
    /// target dir 待ちの処理が追い越しの上限に達した（その処理が入るまで後ろも待つ。#2243）
    OvertakeLimit {
        max: u32,
    },
}

fn gb(mb: u64) -> String {
    format!("{:.1} GB", mb as f64 / GIB_MB as f64)
}

impl WaitReason {
    pub fn code(&self) -> &'static str {
        match self {
            Self::Regather => "regather",
            Self::TargetDir => "target_dir",
            Self::PressureCritical => "pressure_critical",
            Self::PressureWarn => "pressure_warn",
            Self::Swap { .. } => "swap",
            Self::MaxJobs { .. } => "max_jobs",
            Self::Memory { .. } => "memory",
            Self::Ahead => "ahead",
            Self::OvertakeLimit { .. } => "overtake_limit",
        }
    }

    /// 人が読む 1 文
    pub fn text(&self) -> String {
        match self {
            Self::Regather => {
                "調停役の立ち上げ直後（走っている処理の登録し直しを待っている）".into()
            }
            Self::TargetDir => "同じ target dir を使う処理が走っている".into(),
            Self::PressureCritical => "メモリ圧 critical（新しい処理を止めている）".into(),
            Self::PressureWarn => "メモリ圧 warn（実行中が終わってから 1 件ずつ）".into(),
            Self::Swap { used_mb } => {
                format!(
                    "swap 使用 {}（実行中が終わってから 1 件ずつ）",
                    gb(*used_mb)
                )
            }
            Self::MaxJobs {
                running,
                unmanaged,
                max,
            } => {
                if *unmanaged > 0 {
                    format!("同時数の上限（実行 {running} + tako の外 {unmanaged} / 上限 {max}）")
                } else {
                    format!("同時数の上限（実行 {running} / 上限 {max}）")
                }
            }
            Self::Memory {
                available_mb,
                need_mb,
                reserve_mb,
                pending_mb,
                estimate_mb,
            } => {
                // 必要の内訳は実行中の分があるときだけ（何が押さえているかを読めるように。#2254）
                let breakdown = if *pending_mb > 0 {
                    format!(
                        " = 実行中のまだ使っていない見込み {} + この処理の見込み {}",
                        gb(*pending_mb),
                        gb(*estimate_mb)
                    )
                } else {
                    String::new()
                };
                format!(
                    "空きメモリ不足（空き {}・必要 {}{breakdown}・予約 {}）",
                    gb(*available_mb),
                    gb(*need_mb),
                    gb(*reserve_mb)
                )
            }
            Self::Ahead => "前に並んでいる処理の後".into(),
            Self::OvertakeLimit { max } => format!(
                "同じ target dir を待つ処理が追い越しの上限（{max} 件）に達した（その処理が入るまで後ろも待つ）"
            ),
        }
    }
}

/// [`admit`] の入力
#[derive(Debug, Clone)]
pub struct AdmitInput<'a> {
    /// 機の値（採れなければ `None` = 同時数だけで決める）
    pub machine: Option<MachineView>,
    pub limits: Limits,
    pub running: &'a [RunningJob],
    pub waiting: &'a [WaitingJob],
    /// tako の外（順番待ちを通っていない）の重い処理の件数
    pub unmanaged: u32,
    /// 調停役の立ち上げ直後（登録し直しを待つ間は誰も入れない）
    pub regathering: bool,
    /// target dir 待ちの件を後ろに追い越させる（#2243。`false` = 修正前の「先頭が入れなければ
    /// 後ろも待つ」。調停役は env `TAKO_2243_LEGACY=1` のときだけ `false` にする）
    pub overtake: bool,
    /// ピークを過ぎた実行中の「まだ使っていない見込み」を 0 にする（#2254。`false` = 修正前の
    /// 「見込み − 今の使用量」を最後まで押さえる。調停役は env `TAKO_2254_LEGACY=1` のときだけ `false`）
    pub release_after_peak: bool,
    /// 最低限の前進（実行中が 0 件ならメモリ・warn の条件が欠けても先頭を入れる）を使う。
    /// 重い処理 = `true`（終われば空くので、外の負荷だけで永久に待たないため）。
    /// agent の起動待ち（#2413）= `false`（常駐は終わらない = 逼迫中に足した分は戻らない）
    pub min_progress: bool,
}

/// [`admit`] の結果
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Admission {
    /// 今回入れる受付番号（並びの順）
    pub grants: Vec<u64>,
    /// 入れなかった先頭（並びの最初の、入れなかった件）が待つ理由（全員入れたら `None`）
    pub blocked: Option<WaitReason>,
    /// 待ちの並び（受付番号。入れた分も含む）
    pub order: Vec<u64>,
    /// 入れなかった件それぞれが待つ理由（並びの順）。調べた件は自分の理由、並びが止まって
    /// 調べなかった件は [`WaitReason::Ahead`]
    pub reasons: Vec<(u64, WaitReason)>,
    /// 並びを止めた理由（[`WaitReason::Ahead`] の件はこれを待っている）。止まらずに最後まで
    /// 見たら `None`
    pub stopped: Option<WaitReason>,
    /// 今回追い越された件と、その回数（受付番号 → 今回その件を追い越して入った件数）
    pub overtaken: Vec<(u64, u32)>,
}

impl Admission {
    /// 入れなかった件が待つ理由（入れた件・並びに居ない件は `None`）
    pub fn reason_of(&self, id: u64) -> Option<&WaitReason> {
        self.reasons.iter().find(|(i, _)| *i == id).map(|(_, r)| r)
    }

    /// その件を実際に止めているもの: 自分の理由。並びが止まって調べなかった件は並びを止めた理由
    pub fn holding_of(&self, id: u64) -> Option<&WaitReason> {
        match self.reason_of(id)? {
            WaitReason::Ahead => self.stopped.as_ref().or(self.blocked.as_ref()),
            own => Some(own),
        }
    }
}

/// 待ちを並べる: 優先度の高い順 → 受付の古い順 → 受付番号
pub fn order_waiting(waiting: &[WaitingJob]) -> Vec<&WaitingJob> {
    let mut v: Vec<&WaitingJob> = waiting.iter().collect();
    v.sort_by(|a, b| {
        a.priority
            .cmp(&b.priority)
            .then(a.enqueued_at_ms.cmp(&b.enqueued_at_ms))
            .then(a.id.cmp(&b.id))
    });
    v
}

/// メモリ圧の段（swap の警戒線を含めた実効値）。機の値が無ければ `None`
fn effective_pressure(machine: Option<&MachineView>, limits: &Limits) -> Option<WaitReason> {
    let m = machine?;
    match m.pressure {
        Some(Pressure::Critical) => return Some(WaitReason::PressureCritical),
        Some(Pressure::Warn) => return Some(WaitReason::PressureWarn),
        _ => {}
    }
    match m.swap_used_mb {
        Some(used) if used >= limits.swap_guard_mb => Some(WaitReason::Swap { used_mb: used }),
        _ => None,
    }
}

/// 先頭 1 件を入れられるか（設計書 §4.3。順番はモジュールの説明のとおり）
fn check(
    head: &WaitingJob,
    running: &[RunningJob],
    input: &AdmitInput<'_>,
) -> Result<(), WaitReason> {
    if let Some(dir) = &head.target_dir {
        if running.iter().any(|r| r.target_dir.as_ref() == Some(dir)) {
            return Err(WaitReason::TargetDir);
        }
    }
    let pressure = effective_pressure(input.machine.as_ref(), &input.limits);
    if pressure == Some(WaitReason::PressureCritical) {
        return Err(WaitReason::PressureCritical);
    }
    let active: Vec<&RunningJob> = running.iter().filter(|r| !r.stalled).collect();
    // 最低限の前進（agent の起動待ちは使わない = #2413）
    if active.is_empty() && input.min_progress {
        return Ok(());
    }
    if let Some(reason) = pressure {
        return Err(reason);
    }
    let running_n = active.len() as u32;
    if running_n + input.unmanaged >= input.limits.max_jobs {
        return Err(WaitReason::MaxJobs {
            running: running_n,
            unmanaged: input.unmanaged,
            max: input.limits.max_jobs,
        });
    }
    if let Some(available) = input.machine.and_then(|m| m.available_mb) {
        // 走り出したばかりでまだ見込みまで使っていない分は、これから空きから減る
        // （ピークを過ぎた処理は 0 = #2254。今使っている分は空きの実測から既に減っている）
        let pending: u64 = active
            .iter()
            .map(|r| r.pending_mb(input.release_after_peak))
            .sum();
        let need = pending + head.estimate_mb;
        if available < need + input.limits.reserve_mb {
            return Err(WaitReason::Memory {
                available_mb: available,
                need_mb: need,
                reserve_mb: input.limits.reserve_mb,
                pending_mb: pending,
                estimate_mb: head.estimate_mb,
            });
        }
    }
    Ok(())
}

/// 割り当て（純関数。設計書 §4.3 と #2243）。並びの先頭から入れられるだけ入れる。
/// 枠・メモリで入れない件で止まり、target dir だけで入れない件は後ろに追い越させる
/// （条件と上限はモジュールの説明の「追い越し」）
pub fn admit(input: &AdmitInput<'_>) -> Admission {
    let order = order_waiting(input.waiting);
    let ids: Vec<u64> = order.iter().map(|w| w.id).collect();
    if order.is_empty() {
        return Admission {
            order: ids,
            ..Admission::default()
        };
    }
    if input.regathering {
        let reasons = ids
            .iter()
            .enumerate()
            .map(|(i, id)| {
                let r = if i == 0 {
                    WaitReason::Regather
                } else {
                    WaitReason::Ahead
                };
                (*id, r)
            })
            .collect();
        return Admission {
            blocked: Some(WaitReason::Regather),
            stopped: Some(WaitReason::Regather),
            order: ids,
            reasons,
            ..Admission::default()
        };
    }
    let max_over = max_overtaken(&input.limits);
    let mut running: Vec<RunningJob> = input.running.to_vec();
    let mut grants = Vec::new();
    let mut reasons: Vec<(u64, WaitReason)> = Vec::new();
    let mut blocked = None;
    let mut stopped = None;
    // 追い越させている件（target dir だけで入れない件）と、今回その件を追い越して入った件数
    let mut passed: Vec<(&WaitingJob, u32)> = Vec::new();
    for (i, job) in order.iter().enumerate() {
        // 前の件が追い越しの上限に達していたら、ここで止める（後ろはその件が入るまで待つ）
        if passed
            .iter()
            .any(|(p, n)| p.overtaken.saturating_add(*n) >= max_over)
        {
            reasons.extend(order[i..].iter().map(|w| (w.id, WaitReason::Ahead)));
            stopped = Some(WaitReason::OvertakeLimit { max: max_over });
            break;
        }
        // 追い越された件と同じ target dir の件は、その dir を持つ実行中が居るので 1 で待つ
        // （= 同じ dir の中の順番は保たれる）
        let reason = match check(job, &running, input) {
            Ok(()) => {
                grants.push(job.id);
                for (_, n) in passed.iter_mut() {
                    *n += 1;
                }
                running.push(RunningJob {
                    id: job.id,
                    estimate_mb: job.estimate_mb,
                    used_mb: None,
                    target_dir: job.target_dir.clone(),
                    ..RunningJob::default()
                });
                continue;
            }
            Err(reason) => reason,
        };
        blocked.get_or_insert_with(|| reason.clone());
        reasons.push((job.id, reason.clone()));
        if input.overtake && reason == WaitReason::TargetDir {
            passed.push((job, 0));
            continue;
        }
        // 枠・メモリで入れない（または修正前の形）: 後ろも待つ
        reasons.extend(order[i + 1..].iter().map(|w| (w.id, WaitReason::Ahead)));
        stopped = Some(reason);
        break;
    }
    let overtaken = passed
        .iter()
        .filter(|(_, n)| *n > 0)
        .map(|(p, n)| (p.id, *n))
        .collect();
    Admission {
        grants,
        blocked,
        order: ids,
        reasons,
        stopped,
        overtaken,
    }
}

// ===== agent の起動待ち（#2413）=====

/// 再起動後の復元の列（#2413）。復元はこの列で **1 本ずつ**起こす（[`admit`] の段 1「同じ target dir は
/// 同時に 1 件」を列として使う）。列の待ちは段 1 だけで入れない件なので、spawn は追い越せる
pub const RESTORE_LANE: &str = "<agent-restore>";

/// agent の起動待ちの判断（#2413。純関数）。重い処理と**同じ [`admit`]** を agent の方針の入力で呼ぶ:
///
/// - 最低限の前進なし（[`AdmitInput::min_progress`] = `false`）: メモリ圧 warn / critical・swap の警戒線・
///   空きの予約を割るあいだは、立ち上がり中が 0 本でも起こさない（常駐は終わらないので足した分が戻らない）
/// - 同時数の上限なし: agent の本数そのものは数えない（歯止めはメモリの段 2・4・6）
/// - `starting` = 立ち上がり中の agent（まだ使っていない見込みを空きから引く = 立て続けの起動を重ねない）
/// - 復元は [`RESTORE_LANE`] の列で 1 本ずつ（spawn は列の待ちを追い越せる）
///
/// `limits` は重い処理と同じ実効値（`settings.json` の `resources` 節の予約・swap の警戒線）
pub fn admit_launches(
    machine: Option<MachineView>,
    limits: Limits,
    starting: &[RunningJob],
    waiting: &[WaitingJob],
) -> Admission {
    admit(&AdmitInput {
        machine,
        limits: Limits {
            max_jobs: u32::MAX,
            ..limits
        },
        running: starting,
        waiting,
        unmanaged: 0,
        regathering: false,
        overtake: true,
        release_after_peak: true,
        min_progress: false,
    })
}

/// agent の起動が待つ理由の 1 文（#2413）。重い処理の文（[`WaitReason::text`]）は「実行中が終わってから
/// 1 件ずつ」「同じ target dir」など順番待ちの言葉なので、agent の起動の言葉へ言い換える
pub fn launch_reason_text(reason: &WaitReason) -> String {
    match reason {
        WaitReason::PressureCritical => "メモリ圧 critical（agent の起動を止めている）".into(),
        WaitReason::PressureWarn => "メモリ圧 warn（normal へ戻るまで agent を起こさない）".into(),
        WaitReason::Swap { used_mb } => format!(
            "swap 使用 {}（警戒線を下回るまで agent を起こさない）",
            gb(*used_mb)
        ),
        WaitReason::Memory {
            available_mb,
            need_mb,
            reserve_mb,
            pending_mb,
            estimate_mb,
        } => {
            let breakdown = if *pending_mb > 0 {
                format!(
                    " = 立ち上がり中の agent のまだ使っていない見込み {} + この agent の見込み {}",
                    gb(*pending_mb),
                    gb(*estimate_mb)
                )
            } else {
                String::new()
            };
            format!(
                "空きメモリ不足（空き {}・必要 {}{breakdown}・予約 {}）",
                gb(*available_mb),
                gb(*need_mb),
                gb(*reserve_mb)
            )
        }
        WaitReason::TargetDir => {
            "再起動後の復元は 1 本ずつ（前の agent の立ち上がりを待っている）".into()
        }
        WaitReason::Ahead => "前に並んでいる起動の後".into(),
        other => other.text(),
    }
}

/// 取り分（`CARGO_BUILD_JOBS` / `RUST_TEST_THREADS`）: max(1, 論理 CPU ÷ 実行中の数)。
/// `running` は今回入れる分を含めた実行中（停滞を除く）の件数
pub fn share(cpus: u32, running: usize) -> u32 {
    let n = running.max(1) as u32;
    (cpus / n).max(1)
}

/// tako の外の重い処理のうち枠に数える件数（純関数）。CPU の差分がまだ無い件は数える（安全側）
pub fn count_unmanaged(cpu_percent: &[Option<f64>]) -> u32 {
    cpu_percent
        .iter()
        .filter(|c| c.is_none_or(|c| c >= UNMANAGED_ACTIVE_CPU_PERCENT))
        .count() as u32
}

// ===== 停滞 =====

/// 停滞の追跡（純関数）: 木の CPU% が下限未満になった時刻を返す。
///
/// - `cpu` が `None`（まだ差分が採れていない）なら前の状態のまま
/// - 下限以上なら `None`（数え直し）
/// - 下限未満なら、前から低ければその時刻、初めてなら `now`
pub fn track_low_cpu(low_since: Option<u64>, cpu: Option<f64>, now_ms: u64) -> Option<u64> {
    match cpu {
        None => low_since,
        Some(c) if c >= STALL_CPU_PERCENT => None,
        Some(_) => Some(low_since.unwrap_or(now_ms)),
    }
}

/// 停滞しているか（`window_ms` 続けて低い）
pub fn is_stalled(low_since: Option<u64>, now_ms: u64, window_ms: u64) -> bool {
    low_since.is_some_and(|since| now_ms.saturating_sub(since) >= window_ms)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn argv(s: &str) -> Vec<String> {
        s.split_whitespace().map(str::to_string).collect()
    }

    fn limits(max_jobs: u32) -> Limits {
        Limits {
            max_jobs,
            reserve_mb: 4 * GIB_MB,
            swap_guard_mb: 2 * GIB_MB,
            max_wait_min: 30,
        }
    }

    fn machine(available_gb: u64, pressure: Pressure) -> MachineView {
        MachineView {
            total_mb: 36 * GIB_MB,
            available_mb: Some(available_gb * GIB_MB),
            pressure: Some(pressure),
            swap_used_mb: Some(0),
        }
    }

    fn run(id: u64) -> RunningJob {
        RunningJob {
            id,
            estimate_mb: 3 * GIB_MB,
            used_mb: Some(3 * GIB_MB),
            ..RunningJob::default()
        }
    }

    fn wait(id: u64, at: u64) -> WaitingJob {
        WaitingJob {
            id,
            estimate_mb: 3 * GIB_MB,
            target_dir: None,
            priority: Priority::Normal,
            enqueued_at_ms: at,
            overtaken: 0,
        }
    }

    fn input<'a>(
        machine: Option<MachineView>,
        limits: Limits,
        running: &'a [RunningJob],
        waiting: &'a [WaitingJob],
        unmanaged: u32,
    ) -> AdmitInput<'a> {
        AdmitInput {
            machine,
            limits,
            running,
            waiting,
            unmanaged,
            regathering: false,
            overtake: true,
            release_after_peak: true,
            min_progress: true,
        }
    }

    #[test]
    fn 区分はコマンドの本体で決める() {
        assert_eq!(classify(&argv("cargo build --release")), JobKind::Build);
        assert_eq!(classify(&argv("cargo test --workspace")), JobKind::Test);
        assert_eq!(classify(&argv("cargo +nightly -q t -p a")), JobKind::Test);
        assert_eq!(classify(&argv("cargo nextest run")), JobKind::Test);
        assert_eq!(
            classify(&argv("cargo clippy --all-targets")),
            JobKind::Build
        );
        assert_eq!(classify(&argv("cargo")), JobKind::Build, "サブコマンドなし");
        assert_eq!(classify(&argv("/x/y/cargo-clippy")), JobKind::Build);
        assert_eq!(
            classify(&argv("env CARGO_BUILD_JOBS=2 cargo test")),
            JobKind::Test,
            "env の前置きを飛ばして本体（cargo test）で決める"
        );
        assert_eq!(classify(&argv("nice -n 5 cargo build")), JobKind::Build);
        assert_eq!(
            classify(&argv("bash scripts/test-x.sh")),
            JobKind::Build,
            "シェルスクリプト = ビルドや検証の入口"
        );
        assert_eq!(classify(&argv("./scripts/build-app.sh")), JobKind::Build);
        assert_eq!(classify(&argv("npx playwright test")), JobKind::Browser);
        assert_eq!(classify(&argv("npm run e2e")), JobKind::Node);
        assert_eq!(classify(&argv("tako-app")), JobKind::Gui);
        assert_eq!(classify(&argv("sleep 3")), JobKind::Other);
        assert_eq!(classify(&[]), JobKind::Other);
        assert_eq!(classify(&argv("env FOO=1")), JobKind::Other, "本体が無い");
    }

    #[test]
    fn 区分の見込みメモリ() {
        assert_eq!(JobKind::Build.default_estimate_mb(), BUILD_ESTIMATE_MB);
        assert_eq!(JobKind::Test.default_estimate_mb(), TEST_ESTIMATE_MB);
        assert_eq!(JobKind::Browser.default_estimate_mb(), 2 * GIB_MB);
        assert_eq!(JobKind::Other.default_estimate_mb(), GIB_MB);
    }

    #[test]
    fn target_dirはworkspaceの根の隣() {
        let ws = Path::new("/r");
        let member = Path::new("/r/crates/a");
        let probe = |d: &Path| -> Option<bool> {
            if d == ws {
                Some(true)
            } else if d == member {
                Some(false)
            } else {
                None
            }
        };
        assert_eq!(
            cargo_target_dir(Path::new("/r/crates/a/src"), None, probe),
            Some(PathBuf::from("/r/target"))
        );
        assert_eq!(
            cargo_target_dir(Path::new("/r/crates/a/src"), Some("/t"), probe),
            Some(PathBuf::from("/t")),
            "CARGO_TARGET_DIR が勝つ"
        );
        assert_eq!(
            cargo_target_dir(Path::new("/r"), Some("tgt"), probe),
            Some(PathBuf::from("/r/tgt")),
            "相対は cwd から"
        );
        assert_eq!(cargo_target_dir(Path::new("/elsewhere"), None, probe), None);
        let only_member = |d: &Path| (d == member).then_some(false);
        assert_eq!(
            cargo_target_dir(Path::new("/r/crates/a/src"), None, only_member),
            Some(PathBuf::from("/r/crates/a/target")),
            "workspace が無ければ一番近い Cargo.toml"
        );
    }

    #[test]
    fn 既定の上限は機に合わせる() {
        assert_eq!(default_max_jobs(Some(36 * GIB_MB)), 3);
        assert_eq!(default_max_jobs(Some(16 * GIB_MB)), 1);
        assert_eq!(default_max_jobs(Some(8 * GIB_MB)), 1, "下限 1");
        assert_eq!(default_max_jobs(Some(64 * GIB_MB)), 4);
        assert_eq!(default_max_jobs(Some(128 * GIB_MB)), 4, "上限 4");
        assert_eq!(default_max_jobs(None), 2);
        assert_eq!(default_reserve_mb(Some(36 * GIB_MB)), 36 * GIB_MB / 10);
        assert_eq!(
            default_reserve_mb(Some(8 * GIB_MB)),
            2 * GIB_MB,
            "下限 2 GB"
        );
        let l = effective_limits(&LimitSettings::default(), Some(36 * GIB_MB));
        assert_eq!(l.max_jobs, 3);
        assert_eq!(l.swap_guard_mb, 2 * GIB_MB);
        assert_eq!(l.max_wait_min, 30);
        let set = LimitSettings {
            max_jobs: Some(5),
            reserve_gb: Some(1.5),
            swap_guard_gb: Some(4.0),
            max_wait_min: Some(10),
            jobserver: None,
        };
        let l = effective_limits(&set, Some(36 * GIB_MB));
        assert_eq!(
            (l.max_jobs, l.reserve_mb, l.swap_guard_mb, l.max_wait_min),
            (5, 1536, 4096, 10)
        );
    }

    #[test]
    fn limitsの変更は範囲を見てから当てる() {
        let mut s = LimitSettings::default();
        s.apply(&LimitsPatch {
            max_jobs: Some(2),
            reserve_gb: Some(3.0),
            ..LimitsPatch::default()
        })
        .unwrap();
        assert_eq!((s.max_jobs, s.reserve_gb), (Some(2), Some(3.0)));
        let before = s.clone();
        for bad in [
            LimitsPatch {
                max_jobs: Some(0),
                ..LimitsPatch::default()
            },
            LimitsPatch {
                max_jobs: Some(65),
                ..LimitsPatch::default()
            },
            LimitsPatch {
                reserve_gb: Some(-1.0),
                ..LimitsPatch::default()
            },
            LimitsPatch {
                swap_guard_gb: Some(f64::NAN),
                ..LimitsPatch::default()
            },
            LimitsPatch {
                max_wait_min: Some(0),
                ..LimitsPatch::default()
            },
            LimitsPatch {
                max_jobs: Some(3),
                max_wait_min: Some(24 * 60 + 1),
                ..LimitsPatch::default()
            },
        ] {
            assert!(s.apply(&bad).is_err(), "{bad:?}");
            assert_eq!(s, before, "弾いたら何も変えない: {bad:?}");
        }
        s.apply(&LimitsPatch {
            reset: true,
            max_wait_min: Some(5),
            ..LimitsPatch::default()
        })
        .unwrap();
        assert_eq!(
            s,
            LimitSettings {
                max_wait_min: Some(5),
                ..LimitSettings::default()
            },
            "reset は戻してから当てる"
        );
    }

    #[test]
    fn jobserverの切り替えは既定へ戻すとキーを消す() {
        let mut s = LimitSettings::default();
        assert!(
            s.jobserver_on(),
            "キーが無ければ on（#1998 の判断点 §11-5）"
        );
        s.apply(&LimitsPatch {
            jobserver: Some(false),
            ..LimitsPatch::default()
        })
        .unwrap();
        assert_eq!(s.jobserver, Some(false));
        assert!(!s.jobserver_on());
        s.apply(&LimitsPatch {
            jobserver: Some(true),
            ..LimitsPatch::default()
        })
        .unwrap();
        assert_eq!(s, LimitSettings::default(), "既定のままなら書き出さない");
        s.jobserver = Some(false);
        s.apply(&LimitsPatch {
            reset: true,
            ..LimitsPatch::default()
        })
        .unwrap();
        assert!(s.jobserver_on(), "reset で既定へ");
        let old: LimitSettings = serde_json::from_str(r#"{"max_jobs": 2}"#).unwrap();
        assert!(
            old.jobserver_on(),
            "キーの無い旧ファイルはそのまま読めて on"
        );
    }

    #[test]
    fn 同時数の境界() {
        let l = limits(3);
        let m = Some(machine(30, Pressure::Normal));
        let w = [wait(10, 1)];
        let two = [run(1), run(2)];
        assert_eq!(admit(&input(m, l, &two, &w, 0)).grants, vec![10], "2 < 3");
        let three = [run(1), run(2), run(3)];
        let a = admit(&input(m, l, &three, &w, 0));
        assert!(a.grants.is_empty());
        assert_eq!(
            a.blocked,
            Some(WaitReason::MaxJobs {
                running: 3,
                unmanaged: 0,
                max: 3
            })
        );
        // 1 回で入れられるだけ入れる（2 件目で上限）
        let ws = [wait(10, 1), wait(11, 2), wait(12, 3)];
        let one = [run(1)];
        let a = admit(&input(m, l, &one, &ws, 0));
        assert_eq!(a.grants, vec![10, 11]);
        assert!(matches!(a.blocked, Some(WaitReason::MaxJobs { .. })));
    }

    #[test]
    fn 外の重い処理も1枠() {
        let l = limits(3);
        let m = Some(machine(30, Pressure::Normal));
        let w = [wait(10, 1)];
        let one = [run(1)];
        assert_eq!(admit(&input(m, l, &one, &w, 1)).grants, vec![10], "1+1 < 3");
        let a = admit(&input(m, l, &one, &w, 2));
        assert_eq!(
            a.blocked,
            Some(WaitReason::MaxJobs {
                running: 1,
                unmanaged: 2,
                max: 3
            }),
            "1+2 = 3"
        );
    }

    #[test]
    fn 最低限の前進は外の負荷とメモリの不足を越える() {
        let l = limits(2);
        let w = [wait(10, 1)];
        // 外が上限を埋め、空きも予約を割っている
        let m = Some(machine(1, Pressure::Normal));
        assert_eq!(admit(&input(m, l, &[], &w, 5)).grants, vec![10]);
        // warn でも 0 件なら 1 件
        let m = Some(machine(1, Pressure::Warn));
        assert_eq!(admit(&input(m, l, &[], &w, 0)).grants, vec![10]);
        // ただし critical は越えない
        let m = Some(machine(30, Pressure::Critical));
        let a = admit(&input(m, l, &[], &w, 0));
        assert!(a.grants.is_empty());
        assert_eq!(a.blocked, Some(WaitReason::PressureCritical));
        // 停滞中は 0 件と数える
        let stalled = [RunningJob {
            stalled: true,
            ..run(1)
        }];
        let m = Some(machine(1, Pressure::Normal));
        assert_eq!(admit(&input(m, l, &stalled, &w, 5)).grants, vec![10]);
    }

    #[test]
    fn メモリ圧の3段() {
        let l = limits(4);
        let w = [wait(10, 1), wait(11, 2)];
        let one = [run(1)];
        // normal: 条件どおり（2 件入る）
        let a = admit(&input(Some(machine(30, Pressure::Normal)), l, &one, &w, 0));
        assert_eq!(a.grants, vec![10, 11]);
        // warn: 実行中が 1 件あるので入れない
        let a = admit(&input(Some(machine(30, Pressure::Warn)), l, &one, &w, 0));
        assert_eq!(a.grants, Vec::<u64>::new());
        assert_eq!(a.blocked, Some(WaitReason::PressureWarn));
        // warn で 0 件なら 1 件だけ（2 件目は「実行中 1」で止まる）
        let a = admit(&input(Some(machine(30, Pressure::Warn)), l, &[], &w, 0));
        assert_eq!(a.grants, vec![10]);
        assert_eq!(a.blocked, Some(WaitReason::PressureWarn));
        // critical: 0 件でも入れない
        let a = admit(&input(Some(machine(30, Pressure::Critical)), l, &[], &w, 0));
        assert!(a.grants.is_empty());
    }

    #[test]
    fn swapは警戒線からwarn扱い() {
        let l = limits(4);
        let w = [wait(10, 1)];
        let one = [run(1)];
        let mut m = machine(30, Pressure::Normal);
        m.swap_used_mb = Some(2 * GIB_MB - 1);
        assert_eq!(admit(&input(Some(m), l, &one, &w, 0)).grants, vec![10]);
        m.swap_used_mb = Some(2 * GIB_MB);
        let a = admit(&input(Some(m), l, &one, &w, 0));
        assert_eq!(
            a.blocked,
            Some(WaitReason::Swap {
                used_mb: 2 * GIB_MB
            })
        );
        assert_eq!(
            admit(&input(Some(m), l, &[], &w, 0)).grants,
            vec![10],
            "0 件なら 1 件"
        );
    }

    #[test]
    fn 空きメモリの予約の境界() {
        let l = limits(4); // 予約 4 GB
        let w = [wait(10, 1)]; // 見込み 3 GB
        let one = [run(1)]; // 見込みを使い切っている
                            // 空き 7 GB = 3 + 4 ちょうど → 入る
        assert_eq!(
            admit(&input(Some(machine(7, Pressure::Normal)), l, &one, &w, 0)).grants,
            vec![10]
        );
        // 1 MiB 足りない → 待つ
        let mut m = machine(7, Pressure::Normal);
        m.available_mb = Some(7 * GIB_MB - 1);
        let a = admit(&input(Some(m), l, &one, &w, 0));
        assert_eq!(
            a.blocked,
            Some(WaitReason::Memory {
                available_mb: 7 * GIB_MB - 1,
                need_mb: 3 * GIB_MB,
                reserve_mb: 4 * GIB_MB,
                pending_mb: 0,
                estimate_mb: 3 * GIB_MB,
            })
        );
        // 走り出したばかり（まだ 1 GB しか使っていない）の分は空きから引いて見る
        let fresh = [RunningJob {
            used_mb: Some(GIB_MB),
            ..run(1)
        }];
        let a = admit(&input(Some(machine(8, Pressure::Normal)), l, &fresh, &w, 0));
        assert!(
            matches!(a.blocked, Some(WaitReason::Memory { need_mb, .. }) if need_mb == 5 * GIB_MB),
            "{a:?}"
        );
        // 同じ回に入れた分も次の判定で引く（空き 9 GB・予約 4・見込み 3 → 1 件目は 3 + 4 ≤ 9 で入り、
        // 2 件目は「1 件目がまだ使っていない 3 + 自分の 3」+ 4 = 10 > 9 で待つ）
        let ws = [wait(10, 1), wait(11, 2)];
        let a = admit(&input(Some(machine(9, Pressure::Normal)), l, &one, &ws, 0));
        assert_eq!(a.grants, vec![10]);
        assert!(matches!(a.blocked, Some(WaitReason::Memory { .. })));
    }

    #[test]
    fn 機の値が無ければ同時数だけで決める() {
        let l = limits(2);
        let w = [wait(10, 1)];
        let one = [run(1)];
        assert_eq!(admit(&input(None, l, &one, &w, 0)).grants, vec![10]);
        let two = [run(1), run(2)];
        assert!(matches!(
            admit(&input(None, l, &two, &w, 0)).blocked,
            Some(WaitReason::MaxJobs { .. })
        ));
    }

    fn run_in(id: u64, dir: &str) -> RunningJob {
        RunningJob {
            target_dir: Some(dir.into()),
            ..run(id)
        }
    }

    fn wait_in(id: u64, at: u64, dir: &str) -> WaitingJob {
        WaitingJob {
            target_dir: Some(dir.into()),
            ..wait(id, at)
        }
    }

    #[test]
    fn 同じtarget_dirは1件ずつ() {
        let l = limits(4);
        let m = Some(machine(30, Pressure::Normal));
        let busy = [run_in(1, "/r/target")];
        let ws = [wait_in(10, 1, "/r/target")];
        let a = admit(&input(m, l, &busy, &ws, 0));
        assert!(a.grants.is_empty());
        assert_eq!(a.blocked, Some(WaitReason::TargetDir));
        // 停滞中でもロックを持っているので同じ dir は入れない
        let stalled = [RunningJob {
            stalled: true,
            ..busy[0].clone()
        }];
        assert_eq!(
            admit(&input(m, l, &stalled, &ws, 0)).blocked,
            Some(WaitReason::TargetDir)
        );
        // 同じ回に入れた 2 件が同じ dir なら 2 件目は待つ
        let both = [wait_in(10, 1, "/r/target"), wait_in(11, 2, "/r/target")];
        let a = admit(&input(m, l, &[], &both, 0));
        assert_eq!(a.grants, vec![10]);
        assert_eq!(a.blocked, Some(WaitReason::TargetDir));
        assert_eq!(a.reason_of(11), Some(&WaitReason::TargetDir));
    }

    #[test]
    fn issue2243_target_dir待ちの先頭は別のdirの後ろに追い越させる() {
        let l = limits(4);
        let m = Some(machine(30, Pressure::Normal));
        let busy = [run_in(1, "/r/target")];
        let ws = [wait_in(10, 1, "/r/target"), wait_in(11, 2, "/other/target")];
        let a = admit(&input(m, l, &busy, &ws, 0));
        assert_eq!(a.grants, vec![11], "空き枠へ別の dir の後ろが入る");
        assert_eq!(
            a.blocked,
            Some(WaitReason::TargetDir),
            "先頭の理由は変わらない"
        );
        assert_eq!(a.reason_of(10), Some(&WaitReason::TargetDir));
        assert_eq!(a.stopped, None, "並びは止まっていない");
        assert_eq!(a.overtaken, vec![(10, 1)], "先頭は 1 回追い越された");
        // 修正前（TAKO_2243_LEGACY=1）: 先頭が待てば後ろも待つ
        let mut legacy = input(m, l, &busy, &ws, 0);
        legacy.overtake = false;
        let a = admit(&legacy);
        assert!(a.grants.is_empty(), "{a:?}");
        assert_eq!(a.reason_of(11), Some(&WaitReason::Ahead));
        assert_eq!(a.holding_of(11), Some(&WaitReason::TargetDir));
        assert!(a.overtaken.is_empty());
        // target dir を持たない処理（cargo の外）も追い越せる
        let ws = [wait_in(10, 1, "/r/target"), wait(11, 2)];
        assert_eq!(admit(&input(m, l, &busy, &ws, 0)).grants, vec![11]);
        // 持ち主が停滞中（枠を返している）でも dir は空かないので追い越させる。追い越す側は最低限の前進で入る
        let stalled = [RunningJob {
            stalled: true,
            ..busy[0].clone()
        }];
        let ws = [wait_in(10, 1, "/r/target"), wait_in(11, 2, "/other/target")];
        let m_tight = Some(machine(1, Pressure::Normal));
        assert_eq!(admit(&input(m_tight, l, &stalled, &ws, 5)).grants, vec![11]);
    }

    #[test]
    fn issue2243_同じtarget_dirが3本並んでも順番は保つ() {
        let l = limits(4);
        let m = Some(machine(30, Pressure::Normal));
        let busy = [run_in(1, "/r/target")];
        let ws = [
            wait_in(10, 1, "/r/target"),
            wait_in(11, 2, "/r/target"),
            wait_in(12, 3, "/other/target"),
            wait_in(13, 4, "/r/target"),
        ];
        let a = admit(&input(m, l, &busy, &ws, 0));
        assert_eq!(a.grants, vec![12]);
        for id in [10, 11, 13] {
            assert_eq!(a.reason_of(id), Some(&WaitReason::TargetDir), "{id}");
        }
        assert_eq!(
            a.overtaken,
            vec![(10, 1), (11, 1)],
            "後ろの 13 は追い越されていない"
        );
        // 持ち主が終わると、同じ dir の中で一番前の 10 だけが入る（11・13 は 10 の後ろ）
        let ws = [
            wait_in(10, 1, "/r/target"),
            wait_in(11, 2, "/r/target"),
            wait_in(13, 4, "/r/target"),
        ];
        let a = admit(&input(m, l, &[run(12)], &ws, 0));
        assert_eq!(a.grants, vec![10]);
        assert_eq!(a.reason_of(11), Some(&WaitReason::TargetDir));
        assert_eq!(a.reason_of(13), Some(&WaitReason::TargetDir));
    }

    #[test]
    fn issue2243_枠とメモリでは追い越さない() {
        let m = Some(machine(30, Pressure::Normal));
        let busy = [run_in(1, "/r/target")];
        let ws = [
            wait_in(10, 1, "/r/target"),
            wait_in(11, 2, "/a/target"),
            wait_in(12, 3, "/b/target"),
            wait(13, 4),
        ];
        // 同時 2 件: 11 が空き枠へ入り、12 は枠で止まる → 13 は 12 の後ろ（資源の追い越しは無し）
        let a = admit(&input(m, limits(2), &busy, &ws, 0));
        assert_eq!(a.grants, vec![11]);
        assert!(matches!(a.reason_of(12), Some(WaitReason::MaxJobs { .. })));
        assert_eq!(a.reason_of(13), Some(&WaitReason::Ahead));
        assert!(matches!(a.stopped, Some(WaitReason::MaxJobs { .. })));
        assert!(matches!(a.holding_of(13), Some(WaitReason::MaxJobs { .. })));
        assert_eq!(a.holding_of(10), Some(&WaitReason::TargetDir));
        // 枠が全部埋まっている: 誰も入れず、後ろの理由は枠
        let a = admit(&input(m, limits(1), &busy, &ws, 0));
        assert!(a.grants.is_empty());
        assert!(matches!(a.reason_of(11), Some(WaitReason::MaxJobs { .. })));
        // メモリ圧 warn（実行中が居る）/ critical: 追い越す側も止まる
        for (p, want) in [
            (Pressure::Warn, WaitReason::PressureWarn),
            (Pressure::Critical, WaitReason::PressureCritical),
        ] {
            let a = admit(&input(Some(machine(30, p)), limits(4), &busy, &ws, 0));
            assert!(a.grants.is_empty(), "{p:?}");
            assert_eq!(a.reason_of(11), Some(&want));
            assert_eq!(a.stopped, Some(want));
        }
        // 空きメモリ不足（空き 6 GB < 見込み 3 + 予約 4）: 追い越す側も自分の見込みで止まる
        let a = admit(&input(
            Some(machine(6, Pressure::Normal)),
            limits(4),
            &busy,
            &ws,
            0,
        ));
        assert!(a.grants.is_empty());
        assert!(matches!(a.reason_of(11), Some(WaitReason::Memory { .. })));
    }

    #[test]
    fn issue2243_追い越しの上限に達したら先頭が先に入る() {
        let l = limits(3);
        let cap = max_overtaken(&l);
        assert_eq!(cap, 6, "同時数 3 × 2 巡");
        let m = Some(machine(30, Pressure::Normal));
        let busy = [run_in(1, "/r/target")];
        let head = |overtaken| WaitingJob {
            overtaken,
            ..wait_in(10, 1, "/r/target")
        };
        // あと 1 回で上限: 11 は入り、12 は 10 の後ろで待つ
        let ws = [head(cap - 1), wait_in(11, 2, "/a"), wait_in(12, 3, "/b")];
        let a = admit(&input(m, l, &busy, &ws, 0));
        assert_eq!(a.grants, vec![11]);
        assert_eq!(a.reason_of(12), Some(&WaitReason::Ahead));
        assert_eq!(a.stopped, Some(WaitReason::OvertakeLimit { max: cap }));
        assert_eq!(
            a.holding_of(12),
            Some(&WaitReason::OvertakeLimit { max: cap })
        );
        assert_eq!(a.overtaken, vec![(10, 1)]);
        // 上限に達した後: 枠が空いていても後ろは入らない
        let ws = [head(cap), wait_in(12, 3, "/b")];
        let a = admit(&input(m, l, &busy, &ws, 0));
        assert!(a.grants.is_empty());
        assert_eq!(a.reason_of(10), Some(&WaitReason::TargetDir));
        assert_eq!(a.reason_of(12), Some(&WaitReason::Ahead));
        // dir が空けば先頭が先に入る（同時 1 件でも後ろより先）
        let a = admit(&input(m, limits(1), &[], &ws, 0));
        assert_eq!(a.grants, vec![10]);
        assert!(matches!(a.reason_of(12), Some(WaitReason::MaxJobs { .. })));
    }

    /// 長く走る処理が dir を持ち続け、別の dir の短い処理が次々に来る流れを回す。
    /// 上限までは空き枠へ入り、上限の後は誰も入らず、dir が空いたら先頭が最初に入る
    #[test]
    fn issue2243_先頭は追い越され続けて飢えない() {
        let l = limits(3);
        let cap = max_overtaken(&l);
        let m = Some(machine(30, Pressure::Normal));
        let holder = run_in(1, "/r/target");
        let mut head = wait_in(10, 1, "/r/target");
        let mut others: Vec<WaitingJob> = Vec::new();
        let mut running_others: Vec<RunningJob> = Vec::new();
        let mut granted_others = 0;
        for step in 0..30u64 {
            // 走っていた追い越し組は 1 手で終わり、新しい処理が 1 件来る
            running_others.clear();
            others.push(wait_in(100 + step, 2 + step, &format!("/o{step}")));
            let mut running = vec![holder.clone()];
            running.extend(running_others.iter().cloned());
            let mut ws = vec![head.clone()];
            ws.extend(others.iter().cloned());
            let a = admit(&input(m, l, &running, &ws, 0));
            for (id, n) in &a.overtaken {
                assert_eq!(*id, 10);
                head.overtaken += n;
            }
            for id in &a.grants {
                others.retain(|w| w.id != *id);
                running_others.push(run(*id));
                granted_others += 1;
            }
            assert!(
                head.overtaken <= cap,
                "上限を超えて追い越された: {}",
                head.overtaken
            );
        }
        assert_eq!(granted_others, cap, "追い越せたのは上限ちょうど");
        // dir が空いた: 待っていた後ろの処理より先に先頭が入る
        let mut ws = vec![head.clone()];
        ws.extend(others.iter().cloned());
        let a = admit(&input(m, l, &running_others, &ws, 0));
        assert_eq!(a.grants.first(), Some(&10), "{a:?}");
    }

    #[test]
    fn 並びは優先度から受付の古い順() {
        let ws = [
            wait(10, 300),
            WaitingJob {
                priority: Priority::Low,
                ..wait(11, 100)
            },
            WaitingJob {
                priority: Priority::High,
                ..wait(12, 400)
            },
            wait(13, 200),
        ];
        let ids: Vec<u64> = order_waiting(&ws).iter().map(|w| w.id).collect();
        assert_eq!(ids, vec![12, 13, 10, 11]);
        let a = admit(&input(None, limits(1), &[run(1)], &ws, 0));
        assert_eq!(a.order, vec![12, 13, 10, 11]);
    }

    #[test]
    fn 立ち上げ直後は誰も入れない() {
        let w = [wait(10, 1)];
        let mut i = input(Some(machine(30, Pressure::Normal)), limits(3), &[], &w, 0);
        i.regathering = true;
        let a = admit(&i);
        assert!(a.grants.is_empty());
        assert_eq!(a.blocked, Some(WaitReason::Regather));
        assert_eq!(a.order, vec![10]);
        let w = [wait(10, 1), wait(11, 2)];
        let mut i = input(Some(machine(30, Pressure::Normal)), limits(3), &[], &w, 0);
        i.regathering = true;
        let a = admit(&i);
        assert_eq!(a.reason_of(10), Some(&WaitReason::Regather));
        assert_eq!(a.reason_of(11), Some(&WaitReason::Ahead));
        assert_eq!(a.holding_of(11), Some(&WaitReason::Regather));
    }

    #[test]
    fn 取り分は実行中の数で割る() {
        assert_eq!(share(18, 1), 18);
        assert_eq!(share(18, 3), 6);
        assert_eq!(share(18, 4), 4);
        assert_eq!(share(2, 4), 1, "下限 1");
        assert_eq!(share(8, 0), 8, "0 件は 1 件として");
    }

    #[test]
    fn 外の重い処理は動いているときだけ1枠() {
        assert_eq!(count_unmanaged(&[]), 0);
        assert_eq!(
            count_unmanaged(&[Some(250.0), Some(UNMANAGED_ACTIVE_CPU_PERCENT), None]),
            3,
            "動いている 2 件 + 差分がまだ無い 1 件（安全側）"
        );
        assert_eq!(
            count_unmanaged(&[Some(0.4), Some(UNMANAGED_ACTIVE_CPU_PERCENT - 0.1)]),
            0,
            "休んでいる dev サーバー・watch は数えない"
        );
    }

    #[test]
    fn 停滞は低いcpuが続いたときだけ() {
        let window = 10_000;
        let mut low = None;
        low = track_low_cpu(low, None, 0);
        assert_eq!(low, None, "差分がまだ無い");
        low = track_low_cpu(low, Some(0.2), 1_000);
        assert_eq!(low, Some(1_000));
        low = track_low_cpu(low, None, 2_000);
        assert_eq!(low, Some(1_000), "採れない回は状態を保つ");
        low = track_low_cpu(low, Some(0.9), 5_000);
        assert!(!is_stalled(low, 10_999, window));
        assert!(is_stalled(low, 11_000, window));
        low = track_low_cpu(low, Some(STALL_CPU_PERCENT), 12_000);
        assert_eq!(low, None, "戻れば数え直す");
        assert!(!is_stalled(low, 99_000, window));
    }

    #[test]
    fn 待つ理由は人が読める() {
        assert_eq!(
            WaitReason::MaxJobs {
                running: 3,
                unmanaged: 1,
                max: 3
            }
            .text(),
            "同時数の上限（実行 3 + tako の外 1 / 上限 3）"
        );
        assert_eq!(WaitReason::Regather.code(), "regather");
        assert_eq!(
            WaitReason::OvertakeLimit { max: 6 }.text(),
            "同じ target dir を待つ処理が追い越しの上限（6 件）に達した（その処理が入るまで後ろも待つ）"
        );
        assert_eq!(
            WaitReason::Memory {
                available_mb: 5 * GIB_MB,
                need_mb: 3 * GIB_MB,
                reserve_mb: 4 * GIB_MB,
                pending_mb: 0,
                estimate_mb: 3 * GIB_MB,
            }
            .text(),
            "空きメモリ不足（空き 5.0 GB・必要 3.0 GB・予約 4.0 GB）"
        );
        // 実行中が押さえている分があれば、必要の内訳を言う（#2254）
        assert_eq!(
            WaitReason::Memory {
                available_mb: 8 * GIB_MB,
                need_mb: 14 * GIB_MB,
                reserve_mb: 4 * GIB_MB,
                pending_mb: 7 * GIB_MB,
                estimate_mb: 7 * GIB_MB,
            }
            .text(),
            "空きメモリ不足（空き 8.0 GB・必要 14.0 GB = 実行中のまだ使っていない見込み 7.0 GB + \
             この処理の見込み 7.0 GB・予約 4.0 GB）"
        );
    }

    #[test]
    fn 受け渡しの1件は旧い形からも読める() {
        let j: JobWire = serde_json::from_str(r#"{"id":5,"enqueued_at_ms":9}"#).unwrap();
        assert_eq!(j.kind, JobKind::Other);
        assert_eq!(j.priority, Priority::Normal);
        let back = serde_json::to_value(&j).unwrap();
        assert!(back.get("child_pid").is_none(), "空の欄は送らない: {back}");
        assert!(
            back.get("history").is_none(),
            "鍵の無い 1 件は送らない: {back}"
        );
        assert_eq!(j.history, None, "旧い wrapper は鍵を送らない = 区分の既定");
    }

    // ===== #2254: 実行中の予約と過去の記録の見込み =====

    /// 36 GB の機の既定（同時 3・予約 3.6 GB）
    fn limits_36gb() -> Limits {
        effective_limits(&LimitSettings::default(), Some(36 * GIB_MB))
    }

    fn machine_mb(available_mb: u64, pressure: Pressure) -> MachineView {
        MachineView {
            total_mb: 36 * GIB_MB,
            available_mb: Some(available_mb),
            pressure: Some(pressure),
            swap_used_mb: Some(0),
        }
    }

    /// 10/17 の本番: `cargo test --workspace`（見込み 7 GB）が最大 7.3 GB を過ぎて今 0.3 GB
    fn past_peak_test(id: u64) -> RunningJob {
        RunningJob {
            id,
            estimate_mb: TEST_ESTIMATE_MB,
            used_mb: Some(300),
            peak_mb: 7475,
            ran_s: 600,
            peak_seen_s: Some(240),
            ..RunningJob::default()
        }
    }

    fn wait_est(id: u64, at: u64, estimate_mb: u64) -> WaitingJob {
        WaitingJob {
            estimate_mb,
            ..wait(id, at)
        }
    }

    fn admit_with(
        machine: MachineView,
        running: &[RunningJob],
        waiting: &[WaitingJob],
        release_after_peak: bool,
    ) -> Admission {
        admit(&AdmitInput {
            release_after_peak,
            ..input(Some(machine), limits_36gb(), running, waiting, 0)
        })
    }

    #[test]
    fn issue2254_ピークを過ぎた実行中は見込みを押さえない() {
        let run = [past_peak_test(1)];
        // 後ろが test の既定 7 GB のまま: 空き 11 GB で、修正前は「必要 13.7 GB」で待ち・修正後は入る
        let w = [wait_est(10, 1, TEST_ESTIMATE_MB)];
        let m = machine_mb(11 * GIB_MB, Pressure::Normal);
        let legacy = admit_with(m, &run, &w, false);
        assert!(
            matches!(
                legacy.blocked,
                Some(WaitReason::Memory { need_mb, pending_mb, .. })
                    if need_mb == 6868 + TEST_ESTIMATE_MB && pending_mb == 6868
            ),
            "修正前は見込み − 今の使用量（6.7 GB）を押さえ続ける: {legacy:?}"
        );
        assert_eq!(admit_with(m, &run, &w, true).grants, vec![10]);
        // 後ろが過去の記録の 1.1 GB（小さい cargo test --test …）: 空き 8 GB でも入る
        let w = [wait_est(10, 1, 1152)];
        let m = machine_mb(8 * GIB_MB, Pressure::Normal);
        assert!(admit_with(m, &run, &w, false).grants.is_empty());
        assert_eq!(admit_with(m, &run, &w, true).grants, vec![10]);
    }

    #[test]
    fn issue2254_ピークの前は今まで通り押さえる() {
        // 走り出したばかり（最大 2 GB・今 1.5 GB・記録なし）: 見込み − 今の使用量を押さえる
        let ramp = RunningJob {
            id: 1,
            estimate_mb: TEST_ESTIMATE_MB,
            used_mb: Some(1536),
            peak_mb: 2048,
            ran_s: 3600,
            peak_seen_s: Some(10),
            ..RunningJob::default()
        };
        assert!(
            !ramp.peak_passed(),
            "記録が無ければ見込みに届くまで過ぎていない"
        );
        assert_eq!(ramp.pending_mb(true), TEST_ESTIMATE_MB - 1536);
        // 走査の前（最大 0）は丸ごと押さえる
        let fresh = RunningJob {
            used_mb: None,
            peak_mb: 0,
            ..ramp.clone()
        };
        assert_eq!(fresh.pending_mb(true), TEST_ESTIMATE_MB);
        // 見込みに届けば過ぎた（届いた瞬間は修正前も 0）
        let reached = RunningJob {
            used_mb: Some(TEST_ESTIMATE_MB),
            peak_mb: TEST_ESTIMATE_MB,
            ..ramp.clone()
        };
        assert_eq!(
            (reached.pending_mb(true), reached.pending_mb(false)),
            (0, 0)
        );
        let w = [wait_est(10, 1, 1152)];
        // 押さえている 5.5 GB + 1.1 GB + 予約 3.6 GB = 10.2 GB > 空き 10 GB → 待つ
        let a = admit_with(machine_mb(10 * GIB_MB, Pressure::Normal), &[ramp], &w, true);
        assert!(
            matches!(a.blocked, Some(WaitReason::Memory { pending_mb, .. }) if pending_mb == TEST_ESTIMATE_MB - 1536),
            "{a:?}"
        );
    }

    #[test]
    fn issue2254_過去の山の時刻を過ぎて自分も伸びていなければ過ぎた() {
        // 今回は見込み（記録の最大 7 GB）より小さい山（3 GB）で終わった: 記録の山の時刻（240 秒）と
        // 自分の最大の更新から 60 秒たつまでは押さえる
        let job = |ran_s: u64, peak_seen_s: u64| RunningJob {
            id: 1,
            estimate_mb: TEST_ESTIMATE_MB,
            used_mb: Some(400),
            peak_mb: 3 * GIB_MB,
            ran_s,
            peak_seen_s: Some(peak_seen_s),
            expected_peak_at_s: Some(240),
            ..RunningJob::default()
        };
        assert!(!job(299, 100).peak_passed(), "記録の山の時刻 + 60 秒の手前");
        assert!(job(300, 100).peak_passed(), "記録の山の時刻 + 60 秒");
        assert!(
            !job(400, 350).peak_passed(),
            "自分の最大がまだ伸びている（更新から 60 秒たっていない）"
        );
        assert!(job(410, 350).peak_passed());
        assert_eq!(job(300, 100).pending_mb(true), 0);
        assert_eq!(
            job(300, 100).pending_mb(false),
            TEST_ESTIMATE_MB - 400,
            "修正前"
        );
        // 記録の山の時刻が無い（区分の既定）なら、時間では過ぎない
        let no_history = RunningJob {
            expected_peak_at_s: None,
            ..job(3600, 10)
        };
        assert!(!no_history.peak_passed());
    }

    #[test]
    fn issue2254_本当に空きが無ければ修正後も待つ() {
        let run = [past_peak_test(1)];
        let w = [wait_est(10, 1, 1152)];
        // メモリ圧 warn: 実行中が居る間は入れない
        let a = admit_with(machine_mb(30 * GIB_MB, Pressure::Warn), &run, &w, true);
        assert_eq!(a.blocked, Some(WaitReason::PressureWarn));
        // critical
        let a = admit_with(machine_mb(30 * GIB_MB, Pressure::Critical), &run, &w, true);
        assert_eq!(a.blocked, Some(WaitReason::PressureCritical));
        // 空きが実際に足りない: 1.1 GB + 予約 3.6 GB = 4.7 GB > 空き 4 GB
        let a = admit_with(machine_mb(4 * GIB_MB, Pressure::Normal), &run, &w, true);
        assert_eq!(
            a.blocked,
            Some(WaitReason::Memory {
                available_mb: 4 * GIB_MB,
                need_mb: 1152,
                reserve_mb: default_reserve_mb(Some(36 * GIB_MB)),
                pending_mb: 0,
                estimate_mb: 1152,
            })
        );
        // swap が警戒線以上
        let mut m = machine_mb(30 * GIB_MB, Pressure::Normal);
        m.swap_used_mb = Some(3 * GIB_MB);
        let a = admit_with(m, &run, &w, true);
        assert!(matches!(a.blocked, Some(WaitReason::Swap { .. })), "{a:?}");
    }

    fn row(key: &str, shape: Option<&str>, peak_mb: u64, peak_at_s: Option<u64>) -> HistoryRow {
        HistoryRow {
            key: key.into(),
            shape: shape.map(str::to_string),
            name: "cargo test".into(),
            kind: JobKind::Test,
            peak_mb,
            peak_at_s,
            ..HistoryRow::default()
        }
    }

    fn keys(key: &str, shape: Option<&str>) -> HistoryKeys {
        HistoryKeys {
            key: key.into(),
            shape: shape.map(str::to_string),
            name: "cargo test".into(),
        }
    }

    #[test]
    fn issue2254_見込みは同じコマンドの直近5回の最大() {
        // 古い順。直近 5 回（2,3,4,5,6 番目）の最大 = 1,500 → 128 MB 刻みへ切り上げて 1,536
        let h: Vec<HistoryRow> = [9000, 1200, 1500, 900, 1100, 1000]
            .iter()
            .enumerate()
            .map(|(i, mb)| row("k", None, *mb, Some(i as u64 * 10)))
            .collect();
        let e = estimate(JobKind::Test, Some(&keys("k", None)), &h);
        assert_eq!(
            e,
            Estimate {
                mb: 1536,
                from: EstimateFrom::History,
                runs: 5,
                peak_at_s: Some(50),
            },
            "6 回前の 9 GB は数えない"
        );
        // 最大 0（走査の前に終わった）の行は数えない・下限 256 MB
        let h = vec![row("k", None, 0, None), row("k", None, 40, Some(1))];
        let e = estimate(JobKind::Test, Some(&keys("k", None)), &h);
        assert_eq!((e.mb, e.runs), (HISTORY_FLOOR_MB, 1));
        // 記録の無いコマンド・鍵を送らない旧い wrapper は区分の既定
        assert_eq!(
            estimate(JobKind::Test, Some(&keys("other", None)), &h),
            Estimate::default_for(JobKind::Test)
        );
        assert_eq!(
            estimate(JobKind::Build, None, &h),
            Estimate::default_for(JobKind::Build)
        );
        // 記録が既定より大きければそちら（見込みは実測に寄せる）
        let h = vec![row("k", None, 7700, None)];
        assert_eq!(estimate(JobKind::Test, Some(&keys("k", None)), &h).mb, 7808);
    }

    #[test]
    fn issue2254_同じ形の記録は2回からで同じコマンドが優先() {
        let h = vec![
            row("a", Some("s"), 1100, Some(30)),
            row("b", Some("s"), 1400, Some(50)),
        ];
        // 初めての `cargo test --test <新しい名前>`: 同じ形の 2 回の最大
        let e = estimate(JobKind::Test, Some(&keys("new", Some("s"))), &h);
        assert_eq!(
            (e.mb, e.from, e.runs, e.peak_at_s),
            (1408, EstimateFrom::Shape, 2, Some(50))
        );
        // 形の記録が 1 回だけなら区分の既定
        let e = estimate(JobKind::Test, Some(&keys("new", Some("s"))), &h[..1]);
        assert_eq!(e.from, EstimateFrom::Default);
        // 同じコマンドの記録があればそれ
        let e = estimate(JobKind::Test, Some(&keys("a", Some("s"))), &h);
        assert_eq!((e.mb, e.from), (1152, EstimateFrom::History));
    }

    fn git(
        map: &'static [(&'static str, Option<&'static str>)],
    ) -> impl Fn(&Path) -> Option<GitEntry> {
        move |dir: &Path| {
            map.iter()
                .find(|(d, _)| Path::new(d) == dir)
                .map(|(_, file)| match file {
                    Some(text) => GitEntry::File((*text).into()),
                    None => GitEntry::Dir,
                })
        }
    }

    #[test]
    fn issue2254_repoの根はworktreeを本体へ寄せる() {
        let m: &'static [_] = &[
            ("/u/dev/tako", None),
            (
                "/u/dev/tako-wt-2254",
                Some("gitdir: /u/dev/tako/.git/worktrees/tako-wt-2254\n"),
            ),
            ("/u/dev/sub/mod", Some("gitdir: ../.git/modules/mod\n")),
        ];
        assert_eq!(
            repo_identity(Path::new("/u/dev/tako/crates/x"), git(m)),
            PathBuf::from("/u/dev/tako")
        );
        assert_eq!(
            repo_identity(Path::new("/u/dev/tako-wt-2254/crates"), git(m)),
            PathBuf::from("/u/dev/tako"),
            "worktree は本体の根"
        );
        assert_eq!(
            repo_identity(Path::new("/u/dev/sub/mod/src"), git(m)),
            PathBuf::from("/u/dev/sub/mod"),
            "worktree でない gitdir（submodule）はそのディレクトリ"
        );
        assert_eq!(
            repo_identity(Path::new("/tmp/x"), git(m)),
            PathBuf::from("/tmp/x"),
            "git の外は cwd"
        );
    }

    #[test]
    fn issue2254_鍵は同じコマンドで揃い形は回ごとの名前を伏せる() {
        let k = |s: &str| history_keys(&argv(s), "/u/dev/tako").unwrap();
        // 前置きとプログラムのパスは数えない
        assert_eq!(
            k("cargo test --workspace").key,
            k("env CARGO_BUILD_JOBS=4 /x/bin/cargo test --workspace").key
        );
        assert_ne!(
            k("cargo test --workspace").key,
            k("cargo build --workspace").key
        );
        assert_ne!(
            k("cargo test --workspace").key,
            history_keys(&argv("cargo test --workspace"), "/u/dev/other")
                .unwrap()
                .key,
            "別の repo は別の鍵"
        );
        // 形: --test の名前・テストの絞り込み・--target-dir は伏せ、-p は伏せない
        let a = k("cargo test -p tako-control --test issue2254_a -- --test-threads=4");
        let b = k("cargo test -p tako-control --test=issue1994_b -- --test-threads=4");
        let c = k("cargo test -p tako-control --test issue2243_c -- --test-threads=4");
        assert_ne!(a.key, c.key);
        assert_eq!(a.shape, c.shape, "--test の名前だけが違う");
        assert_ne!(a.shape, b.shape, "--test=名前 は別の書き方（形も別）");
        assert_ne!(
            k("cargo test -p tako-app --test x").shape,
            k("cargo test -p tako-cli --test x").shape,
            "パッケージで重さが違うので -p は伏せない"
        );
        assert_eq!(
            k("cargo test -p tako-core queue::tests").shape,
            k("cargo test -p tako-core usage::").shape,
            "絞り込みは伏せる"
        );
        assert_eq!(a.name, "cargo test");
        assert_eq!(k("cargo +nightly -q t -p a").name, "cargo t");
        // cargo の外は形でまとめない（スクリプトは中身で重さが違う）
        let s = k("bash scripts/test-x.sh");
        assert_eq!((s.shape, s.name.as_str()), (None, "bash"));
        assert!(history_keys(&[], "/r").is_none());
        // 鍵に生の argv とパスは載らない（16 桁の 16 進）
        let v = serde_json::to_string(&a).unwrap();
        assert!(!v.contains("tako-control") && !v.contains('/'), "{v}");
        assert_eq!(a.key.len(), 16);
    }

    #[test]
    fn issue2254_記録の1行は欄が欠けても読める() {
        let r: HistoryRow = serde_json::from_str(r#"{"key":"k","peak_mb":5}"#).unwrap();
        assert_eq!(
            (r.key.as_str(), r.peak_mb, r.kind),
            ("k", 5, JobKind::Other)
        );
        let full = row("k", Some("s"), 1200, Some(30));
        let back: HistoryRow =
            serde_json::from_str(&serde_json::to_string(&full).unwrap()).unwrap();
        assert_eq!(back, full);
    }

    // ===== agent の起動待ち（#2413）=====

    fn agent_wait(id: u64, at: u64) -> WaitingJob {
        WaitingJob {
            estimate_mb: AGENT_ESTIMATE_MB,
            ..wait(id, at)
        }
    }

    fn restore_wait(id: u64, at: u64) -> WaitingJob {
        WaitingJob {
            target_dir: Some(RESTORE_LANE.into()),
            ..agent_wait(id, at)
        }
    }

    fn starting(id: u64, lane: bool) -> RunningJob {
        RunningJob {
            id,
            estimate_mb: AGENT_ESTIMATE_MB,
            used_mb: Some(50),
            target_dir: lane.then(|| RESTORE_LANE.to_string()),
            ..RunningJob::default()
        }
    }

    #[test]
    fn issue2413_agentはwarnとcriticalで立ち上がり中が0本でも待つ() {
        let w = [agent_wait(1, 1)];
        for (p, code) in [
            (Pressure::Warn, "pressure_warn"),
            (Pressure::Critical, "pressure_critical"),
        ] {
            let a = admit_launches(Some(machine(20, p)), limits_36gb(), &[], &w);
            assert!(a.grants.is_empty(), "{p:?} で起こした: {a:?}");
            assert_eq!(a.blocked.as_ref().map(WaitReason::code), Some(code));
        }
        // 重い処理（最低限の前進あり）は warn でも実行中 0 件なら入る = 同じ admit の入力の違いだけ
        let heavy = admit(&input(
            Some(machine(20, Pressure::Warn)),
            limits_36gb(),
            &[],
            &w,
            0,
        ));
        assert_eq!(heavy.grants, vec![1]);
        // normal へ戻ると起こす
        let a = admit_launches(Some(machine(20, Pressure::Normal)), limits_36gb(), &[], &w);
        assert_eq!(a.grants, vec![1]);
    }

    #[test]
    fn issue2413_agentはswapの警戒線と空きの予約でも待つ() {
        let w = [agent_wait(1, 1)];
        let swapped = MachineView {
            swap_used_mb: Some(3 * GIB_MB),
            ..machine(20, Pressure::Normal)
        };
        let a = admit_launches(Some(swapped), limits_36gb(), &[], &w);
        assert_eq!(a.blocked.as_ref().map(WaitReason::code), Some("swap"));
        // 予約 3.6 GB + 見込み 0.625 GB に空き 4 GB は足りない（立ち上がり中 0 本でも）
        let a = admit_launches(
            Some(machine_mb(4 * GIB_MB, Pressure::Normal)),
            limits_36gb(),
            &[],
            &w,
        );
        assert_eq!(a.blocked.as_ref().map(WaitReason::code), Some("memory"));
        // 立ち上がり中のまだ使っていない見込みも引く: 空き 5 GB で 1 本目は入り、2 本目は入らない
        let two = [agent_wait(1, 1), agent_wait(2, 2)];
        let a = admit_launches(
            Some(machine_mb(5 * GIB_MB, Pressure::Normal)),
            limits_36gb(),
            &[starting(9, false)],
            &two,
        );
        assert_eq!(a.grants, vec![1], "{a:?}");
        // 2 本目は「今入れた 1 本目」もまだ使っていない見込みとして数える
        assert_eq!(a.reason_of(2).map(WaitReason::code), Some("memory"));
        let a = admit_launches(
            Some(machine_mb(5 * GIB_MB, Pressure::Normal)),
            limits_36gb(),
            &[starting(9, false), starting(8, false)],
            &two,
        );
        let text = launch_reason_text(a.blocked.as_ref().unwrap());
        assert!(
            a.grants.is_empty() && text.contains("立ち上がり中の agent"),
            "{a:?} / {text}"
        );
    }

    #[test]
    fn issue2413_復元は列で1本ずつでspawnは列の待ちを追い越す() {
        let m = Some(machine(20, Pressure::Normal));
        // 復元 3 本: 1 回目は先頭だけ（列は同時に 1 本）
        let w = [restore_wait(1, 1), restore_wait(2, 2), restore_wait(3, 3)];
        let a = admit_launches(m, limits_36gb(), &[], &w);
        assert_eq!(a.grants, vec![1]);
        // 先頭が立ち上がり中なら列は待ち、後から来た spawn は追い越して起きる
        let w = [restore_wait(2, 2), restore_wait(3, 3), agent_wait(4, 4)];
        let a = admit_launches(m, limits_36gb(), &[starting(1, true)], &w);
        assert_eq!(a.grants, vec![4], "{a:?}");
        let text = launch_reason_text(a.reason_of(2).unwrap());
        assert!(text.contains("1 本ずつ"), "{text}");
        // 立ち上がりを終えた（立ち上がり中の一覧から外れた）ら次の 1 本
        let a = admit_launches(m, limits_36gb(), &[], &w);
        assert_eq!(a.grants, vec![2, 4]);
        // 逼迫中は列も spawn も待つ（列の待ちだけが追い越しの対象）
        let a = admit_launches(Some(machine(20, Pressure::Warn)), limits_36gb(), &[], &w);
        assert!(a.grants.is_empty());
        // 優先度: master（high）の復元が先頭
        let mut hi = restore_wait(9, 9);
        hi.priority = Priority::High;
        let a = admit_launches(m, limits_36gb(), &[], &[restore_wait(2, 2), hi]);
        assert_eq!(a.grants, vec![9]);
        // 復元 0 本は何もしない
        assert_eq!(
            admit_launches(m, limits_36gb(), &[], &[]),
            Admission::default()
        );
    }

    #[test]
    fn issue2413_機の値が採れなければ時差だけ() {
        // Windows（#2000 まで）: メモリの段は飛ばし、復元の列だけが効く
        let w = [restore_wait(1, 1), restore_wait(2, 2), agent_wait(3, 3)];
        let a = admit_launches(None, limits_36gb(), &[], &w);
        assert_eq!(a.grants, vec![1, 3]);
    }

    #[test]
    fn issue2413_agentの鍵は系統ごとで生の名前以外を持たない() {
        let c = agent_history_keys("claude");
        assert_ne!(c.key, agent_history_keys("codex").key);
        assert_eq!(c.key, agent_history_keys("claude").key);
        assert_eq!((c.shape.as_deref(), c.name.as_str()), (None, "claude"));
        // 記録が無ければ既定（設計書 §1.3 の実測）、あれば直近の最大
        assert_eq!(
            estimate(JobKind::Agent, Some(&c), &[]).mb,
            AGENT_ESTIMATE_MB
        );
        let rows = [HistoryRow {
            key: c.key.clone(),
            kind: JobKind::Agent,
            peak_mb: 300,
            ..HistoryRow::default()
        }];
        let e = estimate(JobKind::Agent, Some(&c), &rows);
        assert_eq!((e.mb, e.from), (384, EstimateFrom::History));
        assert_eq!(classify(&argv("claude --resume x")), JobKind::Other);
    }
}
