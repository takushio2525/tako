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
//! 並びは「優先度の高い順 → 受付の古い順」で、**先頭が入れなければ後ろも待つ**（追い越し無し）。
//! 先頭について上から順に見る:
//!
//! 1. 同じ target dir の処理が走っている（停滞中も含む = cargo のロックを持っている）→ 待つ
//! 2. メモリ圧 critical → 待つ（走っているものは止めない）
//! 3. **最低限の前進**: 管理下の実行中（停滞を除く）が 0 件なら、ここで入れる
//!    （メモリ・外の処理・warn の条件が欠けていても。外の負荷だけで永久に待たないため）
//! 4. メモリ圧 warn（または swap が `swap_guard` 以上）→ 待つ（= 実行中が 0 件のときだけ 1 件）
//! 5. 実行中（停滞を除く）+ tako の外の重い処理 ≥ `max_jobs` → 待つ
//! 6. 空き − （走り出したばかりの処理がまだ使っていない見込み）− 見込み < 予約 → 待つ
//!
//! 機の値が採れない（Windows は #2000 まで）ときは 2・4・6 を飛ばし、同時数だけで決める。

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::resources::Pressure;

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
            Self::Node | Self::Gui | Self::Other => GIB_MB,
        }
    }
}

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

/// 実行中の 1 件
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunningJob {
    pub id: u64,
    /// 見込みメモリ（MiB）
    pub estimate_mb: u64,
    /// いま木が使っているメモリ（MiB。走査がまだなら `None` = 見込みを丸ごとまだ使っていない扱い）
    pub used_mb: Option<u64>,
    pub target_dir: Option<String>,
    /// 停滞中（枠を返している。同時数とメモリの予約に数えない）
    pub stalled: bool,
}

/// 待ちの 1 件
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WaitingJob {
    pub id: u64,
    pub estimate_mb: u64,
    pub target_dir: Option<String>,
    pub priority: Priority,
    pub enqueued_at_ms: u64,
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
    /// 空きメモリが足りない
    Memory {
        available_mb: u64,
        need_mb: u64,
        reserve_mb: u64,
    },
    /// 自分より前に並んでいる処理の後
    Ahead,
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
            } => format!(
                "空きメモリ不足（空き {}・必要 {}・予約 {}）",
                gb(*available_mb),
                gb(*need_mb),
                gb(*reserve_mb)
            ),
            Self::Ahead => "前に並んでいる処理の後".into(),
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
}

/// [`admit`] の結果
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Admission {
    /// 今回入れる受付番号（並びの順）
    pub grants: Vec<u64>,
    /// 入れなかった先頭が待つ理由（全員入れたら `None`）
    pub blocked: Option<WaitReason>,
    /// 待ちの並び（受付番号。入れた分も含む）
    pub order: Vec<u64>,
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
    // 最低限の前進
    if active.is_empty() {
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
        let pending: u64 = active
            .iter()
            .map(|r| r.estimate_mb.saturating_sub(r.used_mb.unwrap_or(0)))
            .sum();
        let need = pending + head.estimate_mb;
        if available < need + input.limits.reserve_mb {
            return Err(WaitReason::Memory {
                available_mb: available,
                need_mb: need,
                reserve_mb: input.limits.reserve_mb,
            });
        }
    }
    Ok(())
}

/// 割り当て（純関数。設計書 §4.3）。先頭から入れられるだけ入れ、入れられない先頭で止まる
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
        return Admission {
            grants: Vec::new(),
            blocked: Some(WaitReason::Regather),
            order: ids,
        };
    }
    let mut running: Vec<RunningJob> = input.running.to_vec();
    let mut grants = Vec::new();
    for head in order {
        match check(head, &running, input) {
            Ok(()) => {
                grants.push(head.id);
                running.push(RunningJob {
                    id: head.id,
                    estimate_mb: head.estimate_mb,
                    used_mb: None,
                    target_dir: head.target_dir.clone(),
                    stalled: false,
                });
            }
            Err(reason) => {
                return Admission {
                    grants,
                    blocked: Some(reason),
                    order: ids,
                };
            }
        }
    }
    Admission {
        grants,
        blocked: None,
        order: ids,
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
            target_dir: None,
            stalled: false,
        }
    }

    fn wait(id: u64, at: u64) -> WaitingJob {
        WaitingJob {
            id,
            estimate_mb: 3 * GIB_MB,
            target_dir: None,
            priority: Priority::Normal,
            enqueued_at_ms: at,
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

    #[test]
    fn 同じtarget_dirは1件ずつで追い越さない() {
        let l = limits(4);
        let m = Some(machine(30, Pressure::Normal));
        let busy = [RunningJob {
            target_dir: Some("/r/target".into()),
            ..run(1)
        }];
        let ws = [
            WaitingJob {
                target_dir: Some("/r/target".into()),
                ..wait(10, 1)
            },
            WaitingJob {
                target_dir: Some("/other/target".into()),
                ..wait(11, 2)
            },
        ];
        let a = admit(&input(m, l, &busy, &ws, 0));
        assert!(
            a.grants.is_empty(),
            "先頭が待てば後ろも待つ（追い越し無し）"
        );
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
        let both = [
            WaitingJob {
                target_dir: Some("/r/target".into()),
                ..wait(10, 1)
            },
            WaitingJob {
                target_dir: Some("/r/target".into()),
                ..wait(11, 2)
            },
        ];
        let a = admit(&input(m, l, &[], &both, 0));
        assert_eq!(a.grants, vec![10]);
        assert_eq!(a.blocked, Some(WaitReason::TargetDir));
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
        assert!(WaitReason::Memory {
            available_mb: 5 * GIB_MB,
            need_mb: 3 * GIB_MB,
            reserve_mb: 4 * GIB_MB
        }
        .text()
        .contains("空き 5.0 GB"));
    }

    #[test]
    fn 受け渡しの1件は旧い形からも読める() {
        let j: JobWire = serde_json::from_str(r#"{"id":5,"enqueued_at_ms":9}"#).unwrap();
        assert_eq!(j.kind, JobKind::Other);
        assert_eq!(j.priority, Priority::Normal);
        let back = serde_json::to_value(&j).unwrap();
        assert!(back.get("child_pid").is_none(), "空の欄は送らない: {back}");
    }
}
