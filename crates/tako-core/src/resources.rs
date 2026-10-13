//! PC 全体と各プロセスの資源の見方（純関数。FR-2.43 / Issue #1993。エピック #1992 の S1）
//!
//! 設計の正は `.agent/plans/2026-10-resource-manager.md`（§2）。
//!
//! ## ここに置くもの・置かないもの
//!
//! OS へ問い合わせるのは境界 B29（[`crate::platform::resources`]）で、ここは**採った値の解釈**
//! だけを持つ: メモリの内訳（Activity Monitor と同じ式）・CPU% の差分・mach tick の換算・
//! プロセスの分類（agent / ビルド / ブラウザ / 隔離 GUI）・帰属（どのペインの処理か / tako の外か）・
//! 集計（重い処理 1 件 = 分類に当たった一番上の祖先と、その木の合計）。
//! OS 分岐を持たないので、Windows の縮退表も macOS 上の単体テストで固定できる。
//!
//! ## 罠（実測。設計書 §1）
//!
//! - Apple Silicon の `ri_user_time` / `ri_system_time` は **mach tick**（timebase 125/3）。
//!   秒として読むと CPU% が 41.7 分の 1 になる → [`mach_to_ns`]
//! - claude の実行ファイル名（`pbsi_comm`）は**版数**（`2.1.294`）。名前では見分けられないので
//!   argv の第 1 語で見る → [`kind_of`]
//! - pid は再利用される。前回の CPU 時間との差分は **pid と開始時刻の組**（[`ProcKey`]）で引く

use std::collections::{HashMap, HashSet};

/// 1 MiB（表示の単位）
pub const MIB: u64 = 1024 * 1024;

// ===== 機全体 =====

/// 仮想記憶のページ数（macOS の `host_statistics64(HOST_VM_INFO64)` の値。単位はページ）
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct VmPages {
    /// 1 ページのバイト数（Apple Silicon は 16 KiB）
    pub page_size: u64,
    pub free: u64,
    pub active: u64,
    pub inactive: u64,
    pub wired: u64,
    pub purgeable: u64,
    pub speculative: u64,
    /// 圧縮器が持っているページ（圧縮後の実体）
    pub compressor: u64,
    /// ファイルに裏打ちされたページ（キャッシュ）
    pub external: u64,
    /// 匿名ページ（アプリのメモリ）
    pub internal: u64,
}

/// メモリの内訳（バイト）。式は Activity Monitor と同じ
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct MemoryBreakdown {
    pub total: u64,
    /// 使用 = app + wired + 圧縮
    pub used: u64,
    /// アプリのメモリ = 匿名ページ − purgeable
    pub app: u64,
    pub wired: u64,
    pub compressed: u64,
    /// キャッシュされたファイル = ファイル裏打ち + purgeable
    pub cached: u64,
    /// 空き = 全体 − 使用
    pub available: u64,
}

/// Activity Monitor と同じ式でメモリの内訳を出す（設計書 §2.1）
pub fn memory_breakdown(total: u64, vm: &VmPages) -> MemoryBreakdown {
    let page = vm.page_size;
    let app = vm
        .internal
        .saturating_sub(vm.purgeable)
        .saturating_mul(page);
    let wired = vm.wired.saturating_mul(page);
    let compressed = vm.compressor.saturating_mul(page);
    let cached = vm
        .external
        .saturating_add(vm.purgeable)
        .saturating_mul(page);
    let used = app.saturating_add(wired).saturating_add(compressed);
    MemoryBreakdown {
        total,
        used,
        app,
        wired,
        compressed,
        cached,
        available: total.saturating_sub(used),
    }
}

/// メモリ圧（macOS の `kern.memorystatus_vm_pressure_level`）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pressure {
    Normal,
    Warn,
    Critical,
}

impl Pressure {
    /// `kern.memorystatus_vm_pressure_level` の値（1 normal / 2 warn / 4 critical）から
    pub fn from_level(level: i32) -> Option<Self> {
        match level {
            1 => Some(Self::Normal),
            2 => Some(Self::Warn),
            4 => Some(Self::Critical),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Normal => "normal",
            Self::Warn => "warn",
            Self::Critical => "critical",
        }
    }
}

/// CPU の累積 tick（macOS の `HOST_CPU_LOAD_INFO`。全コアの合計）
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CpuTicks {
    pub user: u64,
    pub system: u64,
    pub idle: u64,
    pub nice: u64,
}

/// 2 回の累積 tick の差から、機全体の CPU 使用率（0〜100%。全コアが埋まって 100）。
/// 進んでいなければ `None`（時計の粒度に負けた / 同じ値を 2 回採った）。
/// カウンタが巻き戻った（32 bit の折り返し）ときも `None`
pub fn cpu_busy_percent(prev: CpuTicks, cur: CpuTicks) -> Option<f64> {
    let busy = |t: CpuTicks| t.user + t.system + t.nice;
    let d_busy = busy(cur).checked_sub(busy(prev))?;
    let d_idle = cur.idle.checked_sub(prev.idle)?;
    let total = d_busy + d_idle;
    (total > 0).then(|| (d_busy as f64 / total as f64 * 100.0).clamp(0.0, 100.0))
}

/// mach tick → ナノ秒（timebase の `numer / denom` を掛ける）。
/// Apple Silicon は 125/3（1 tick = 41.67 ns）、Intel は 1/1。
/// `denom = 0` の壊れた timebase は 1/1 とみなす（換算しないより悪くしない）
pub fn mach_to_ns(ticks: u64, numer: u32, denom: u32) -> u64 {
    if denom == 0 {
        return ticks;
    }
    (u128::from(ticks) * u128::from(numer) / u128::from(denom)).min(u128::from(u64::MAX)) as u64
}

/// プロセス 1 本（または木）の CPU% = CPU 時間の差 ÷ 経過。
/// 複数スレッドで 100 を超える（Activity Monitor の「% CPU」と同じ流儀）
pub fn cpu_percent(prev_ns: u64, cur_ns: u64, elapsed_ns: u64) -> Option<f64> {
    let used = cur_ns.checked_sub(prev_ns)?;
    (elapsed_ns > 0).then(|| used as f64 / elapsed_ns as f64 * 100.0)
}

/// 機全体の状態（1 回の採取ぶん）
#[derive(Debug, Clone, Default, PartialEq)]
pub struct MachineStats {
    /// メモリの内訳。採れない環境では `None`
    pub memory: Option<MemoryBreakdown>,
    /// swap の (使用, 全体) バイト
    pub swap: Option<(u64, u64)>,
    pub pressure: Option<Pressure>,
    /// 前回との差で出す機全体の CPU%。初回は `None`
    pub cpu_percent: Option<f64>,
    /// load average（unix だけ。Windows には無いので作らない）
    pub load: Option<[f64; 3]>,
    /// 論理 CPU 数
    pub cpus: u32,
}

// ===== プロセス =====

/// pid の再利用を見分ける鍵（pid と開始時刻）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ProcKey {
    pub pid: u32,
    /// 開始時刻（OS の単調時計。比較にだけ使う）
    pub start: u64,
}

/// プロセス 1 本の採取結果
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ProcSample {
    pub pid: u32,
    pub ppid: u32,
    /// 開始時刻（[`ProcKey::start`]）
    pub start: u64,
    /// 実行ファイル名（macOS の `pbsi_comm`。16 文字で切れる）
    pub name: String,
    /// 物理フットプリント（バイト。Activity Monitor の「メモリ」）。読めなければ `None`
    pub footprint: Option<u64>,
    /// 累積 CPU 時間（ナノ秒。換算済み）。読めなければ `None`
    pub cpu_ns: Option<u64>,
    /// コマンド行（argv を空白で繋いだもの）。読めなければ `None`
    pub argv: Option<String>,
    /// env の `TAKO_PANE_ID`（**この tako の**ペインだと確かめた後の値。[`env_pane_of`]）
    pub env_pane: Option<u64>,
    /// env に `TAKO_ISOLATED=1`（隔離の tako-app）
    pub isolated: bool,
}

impl ProcSample {
    pub fn key(&self) -> ProcKey {
        ProcKey {
            pid: self.pid,
            start: self.start,
        }
    }
}

/// env から「この tako のペイン」の番号を取り出す（純関数）。
///
/// **Apple のプラットフォームバイナリ（`/bin/sleep` 等）は env を他のプロセスから読めない**
/// （`KERN_PROCARGS2` に env が載らない。実測 0 キー）ので、それらには効かない。
/// 本物の cargo / rustc / node はプラットフォームバイナリではないので読める。
///
/// 隔離の tako（`TAKO_ISOLATED=1`）のペインも同じ番号の空間（1, 2, 3, …）を使うので、
/// `TAKO_PANE_ID` だけを信じると**別インスタンスのペイン 3 を本番のペイン 3 と取り違える**。
/// `TAKO_SOCKET` が自分の受け口と一致するときだけ信じる。自分の受け口が分からなければ信じない
pub fn env_pane_of(
    pane_id: Option<&str>,
    socket: Option<&str>,
    own_socket: Option<&str>,
) -> Option<u64> {
    let own = own_socket.filter(|s| !s.is_empty())?;
    if socket? != own {
        return None;
    }
    pane_id?.trim().parse().ok()
}

/// プロセスの種類（設計書 §2.4）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProcKind {
    /// AI エージェント本体（claude / codex / agy）
    Agent(&'static str),
    /// シェル（agent の補助プロセスとビルドを見分ける境目）
    Shell,
    /// ビルド・テスト（cargo / rustc / link / テストの実行体 / node のビルド系）
    Build,
    /// テスト用のブラウザ（playwright の Chromium）
    Browser,
    /// 隔離の tako-app（`TAKO_ISOLATED=1`）
    Gui,
    Other,
}

impl ProcKind {
    /// 「重い処理」として数える種類か
    pub fn is_heavy(self) -> bool {
        matches!(self, Self::Build | Self::Browser | Self::Gui)
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Agent(_) => "agent",
            Self::Shell => "shell",
            Self::Build => "build",
            Self::Browser => "browser",
            Self::Gui => "gui",
            Self::Other => "other",
        }
    }
}

/// argv の第 1 語の basename（`/a/b/claude --x` → `claude`）
fn argv0_base(argv: Option<&str>) -> Option<&str> {
    let first = argv?.split_whitespace().next()?;
    Some(first.rsplit('/').next().unwrap_or(first))
}

const SHELLS: &[&str] = &[
    "zsh", "bash", "sh", "fish", "dash", "ksh", "tcsh", "nu", "pwsh", "login",
];

const BUILD_NAMES: &[&str] = &[
    "cargo",
    "rustc",
    "clippy-driver",
    "cargo-clippy",
    "rustdoc",
    "ld",
    "ld64",
    "ld-prime",
    "clang",
    "clang++",
    "cc",
    "c++",
    "gcc",
    "g++",
    "swift-frontend",
    "swiftc",
    "xcodebuild",
    "make",
    "ninja",
    "cmake",
];

/// node / bun / deno の上で動くビルド系（argv の語の basename で見る）
const NODE_BUILD_TOOLS: &[&str] = &[
    "npm",
    "npx",
    "pnpm",
    "yarn",
    "vite",
    "webpack",
    "tsc",
    "esbuild",
    "jest",
    "vitest",
    "playwright",
    "next",
    "astro",
    "rollup",
];

/// プロセスの種類を決める（純関数。設計書 §2.4）。
///
/// `name` は実行ファイル名（16 文字で切れる）、`argv` はコマンド行（読めなければ `None`）。
/// agent の補助プロセス（MCP サーバー）かどうかは**系図で**決める（[`summarize`]）ので、
/// ここでは見ない（`npm exec @playwright/mcp` もここでは Build になる）
pub fn kind_of(name: &str, argv: Option<&str>, isolated: bool) -> ProcKind {
    let base = argv0_base(argv).unwrap_or(name);
    let login_trimmed = |s: &str| s.trim_start_matches('-').to_string();
    for agent in ["claude", "codex", "agy"] {
        if base == agent || name == agent {
            return ProcKind::Agent(agent);
        }
    }
    // claude を版数のディレクトリから直に起こした形（`…/claude/versions/2.1.294`）
    if argv
        .and_then(|a| a.split_whitespace().next())
        .is_some_and(|first| first.contains("/claude/versions/"))
    {
        return ProcKind::Agent("claude");
    }
    // codex は node のスクリプトとして動くこともある（`node …/bin/codex.js`）
    if argv.is_some_and(|a| {
        a.split_whitespace()
            .take(2)
            .any(|w| w.ends_with("/bin/codex.js") || w.ends_with("/bin/codex"))
    }) {
        return ProcKind::Agent("codex");
    }
    if SHELLS.contains(&login_trimmed(name).as_str())
        || SHELLS.contains(&login_trimmed(base).as_str())
    {
        return ProcKind::Shell;
    }
    if base == "tako-app" || name == "tako-app" {
        return if isolated {
            ProcKind::Gui
        } else {
            ProcKind::Other
        };
    }
    let browser = |s: &str| {
        s.starts_with("chrome-headless")
            || s.starts_with("headless_shell")
            || s.starts_with("Chromium")
            || s.starts_with("Google Chrome for Testing")
    };
    if browser(name) || browser(base) {
        return ProcKind::Browser;
    }
    if BUILD_NAMES.contains(&base) || BUILD_NAMES.contains(&name) {
        return ProcKind::Build;
    }
    // cargo のビルドスクリプト（build-script-build）とテストの実行体（target/**/deps/*）
    if base.starts_with("build-script-") || name.starts_with("build-script-") {
        return ProcKind::Build;
    }
    if let Some(first) = argv.and_then(|a| a.split_whitespace().next()) {
        if first.contains("/target/") && (first.contains("/deps/") || first.contains("/build/")) {
            return ProcKind::Build;
        }
    }
    if matches!(base, "node" | "bun" | "deno") || matches!(name, "node" | "bun" | "deno") {
        let tool = argv.is_some_and(|a| {
            a.split_whitespace().skip(1).take(3).any(|w| {
                let w = w.rsplit('/').next().unwrap_or(w);
                let w = w
                    .trim_end_matches(".js")
                    .trim_end_matches(".cjs")
                    .trim_end_matches(".mjs");
                NODE_BUILD_TOOLS.contains(&w) || w.starts_with("npm-cli")
            })
        });
        if tool {
            return ProcKind::Build;
        }
    }
    ProcKind::Other
}

/// 誰の処理か
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Owner {
    /// この tako のペイン
    Pane(u64),
    /// tako の外。`origin` は launchd（pid 1）の直下にある祖先の名前（Terminal / iTerm2 など）
    External { origin: Option<String> },
}

/// 祖先を辿る上限（壊れた親子表で回り続けない）
const MAX_HOPS: usize = 64;

/// 帰属を決める（純関数。設計書 §2.3）。
///
/// 1. 自分か祖先が**ペインの根**（直接ペインのシェル / 器のペインの `pane_pid`）なら、そのペイン
/// 2. 根へ辿れない（launchd へ付け替わった `nohup` やデーモン化）なら、自分か祖先の
///    env の `TAKO_PANE_ID`（[`env_pane_of`] で確かめた後の値）
/// 3. どちらも無ければ tako の外。launchd の直下の祖先の名前を添える
pub fn owner_of(
    pid: u32,
    parents: &HashMap<u32, u32>,
    roots: &HashMap<u32, u64>,
    env_pane: &HashMap<u32, u64>,
    names: &HashMap<u32, String>,
) -> Owner {
    let mut chain = Vec::with_capacity(8);
    let mut current = pid;
    for _ in 0..MAX_HOPS {
        if let Some(&pane) = roots.get(&current) {
            return Owner::Pane(pane);
        }
        chain.push(current);
        match parents.get(&current) {
            Some(&ppid) if ppid != 0 && ppid != current => current = ppid,
            _ => break,
        }
    }
    if let Some(pane) = chain.iter().find_map(|p| env_pane.get(p)) {
        return Owner::Pane(*pane);
    }
    // launchd（pid 1）の直下 = 親が 1 の祖先
    let origin = chain
        .iter()
        .find(|p| parents.get(*p) == Some(&1))
        .or(chain.last())
        .and_then(|p| names.get(p))
        .cloned();
    Owner::External { origin }
}

/// 表示用の短いコマンド行（純関数）。
///
/// - 第 1 語は basename（ホームパスやユーザー名を出さない）
/// - 絶対パスの引数も basename に畳む
/// - `KEY=値` / `--token=値` のうち鍵・トークン・パスワードらしいものは値を伏せる
/// - 最大 `max_chars` 文字（超えたら `…`）
pub fn short_label(argv: &str, max_chars: usize) -> String {
    let secret = |k: &str| {
        let k = k.trim_start_matches('-').to_ascii_lowercase();
        [
            "token",
            "secret",
            "password",
            "passwd",
            "key",
            "auth",
            "credential",
            "otp",
        ]
        .iter()
        .any(|w| k.contains(w))
    };
    let words: Vec<String> = argv
        .split_whitespace()
        .enumerate()
        .map(|(i, w)| {
            if let Some((k, _)) = w.split_once('=') {
                if secret(k) {
                    return format!("{k}=***");
                }
            }
            if i == 0 || w.starts_with('/') || w.starts_with('~') {
                return w.rsplit('/').next().unwrap_or(w).to_string();
            }
            w.to_string()
        })
        .collect();
    let mut out = words.join(" ");
    if out.chars().count() > max_chars {
        out = out
            .chars()
            .take(max_chars.saturating_sub(1))
            .collect::<String>()
            + "…";
    }
    out
}

/// 重い処理 1 件（分類に当たった一番上の祖先と、その木）
#[derive(Debug, Clone, PartialEq)]
pub struct HeavyJob {
    pub root: ProcKey,
    pub kind: ProcKind,
    /// [`short_label`] のコマンド行（argv が読めなければ実行ファイル名）
    pub label: String,
    pub owner: Owner,
    /// 木の物理フットプリントの合計（バイト）
    pub memory: u64,
    /// 木の CPU% の合計（前回の採取が無い木は `None`）
    pub cpu_percent: Option<f64>,
    /// 木のプロセス数
    pub processes: u32,
}

/// ペイン 1 つの消費（木の合計）
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PaneUsage {
    pub pane: u64,
    pub memory: u64,
    /// agent 本体 + 補助プロセス（MCP サーバー等）
    pub agent_memory: u64,
    /// 重い処理の木の合計
    pub heavy_memory: u64,
    pub cpu_percent: f64,
    pub processes: u32,
    /// 動いている agent（あれば）
    pub agent: Option<&'static str>,
    /// 重い処理の件数
    pub heavy: u32,
}

/// tako の外の消費（自分の uid のプロセスだけ。root 等は読めない）
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ExternalUsage {
    pub memory: u64,
    pub cpu_percent: f64,
    pub processes: u32,
    pub heavy: u32,
}

/// 1 回の走査の集計
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Report {
    /// 重い処理（メモリの多い順）
    pub heavy: Vec<HeavyJob>,
    /// ペインごと（メモリの多い順）
    pub panes: Vec<PaneUsage>,
    pub external: ExternalUsage,
}

/// 走査の結果を集計する（純関数。設計書 §2.3・§2.4）。
///
/// - `roots`: ペインの根の pid → ペイン
/// - `cpu`: 前回の採取との差で出した CPU%（鍵は [`ProcKey`]。前回が無いプロセスは入っていない）
///
/// 重い処理 1 件 = 種類が重い（[`ProcKind::is_heavy`]）プロセスのうち、**重い祖先を持たない**もの。
/// ただし agent の補助プロセス（祖先を辿ってシェルより先に agent に当たる = MCP サーバーとその子）は
/// 重い処理に数えず agent の常駐へ入れる（`npm exec @playwright/mcp` とその Chromium は agent のもの）
pub fn summarize(
    procs: &[ProcSample],
    cpu: &HashMap<ProcKey, f64>,
    roots: &HashMap<u32, u64>,
) -> Report {
    let parents: HashMap<u32, u32> = procs.iter().map(|p| (p.pid, p.ppid)).collect();
    let names: HashMap<u32, String> = procs.iter().map(|p| (p.pid, p.name.clone())).collect();
    let env_pane: HashMap<u32, u64> = procs
        .iter()
        .filter_map(|p| p.env_pane.map(|pane| (p.pid, pane)))
        .collect();
    let kinds: HashMap<u32, ProcKind> = procs
        .iter()
        .map(|p| (p.pid, kind_of(&p.name, p.argv.as_deref(), p.isolated)))
        .collect();
    let by_pid: HashMap<u32, &ProcSample> = procs.iter().map(|p| (p.pid, p)).collect();
    let root_pids: HashSet<u32> = roots.keys().copied().collect();

    // 祖先を辿って「agent の補助か」「重い祖先を持つか」を決める（自分は含めない）
    let ancestry = |pid: u32| -> (bool, bool) {
        let mut helper = false;
        let mut decided_helper = false;
        let mut heavy_ancestor = false;
        let mut current = pid;
        for _ in 0..MAX_HOPS {
            let Some(&ppid) = parents.get(&current) else {
                break;
            };
            if ppid == 0 || ppid == current || ppid == 1 {
                break;
            }
            match kinds.get(&ppid) {
                Some(ProcKind::Shell) if !decided_helper => decided_helper = true,
                Some(ProcKind::Agent(_)) if !decided_helper => {
                    helper = true;
                    decided_helper = true;
                }
                Some(k) if k.is_heavy() => heavy_ancestor = true,
                _ => {}
            }
            if root_pids.contains(&ppid) {
                break;
            }
            current = ppid;
        }
        (helper, heavy_ancestor)
    };

    // 子の表（木の合計に使う）
    let mut children: HashMap<u32, Vec<u32>> = HashMap::new();
    for p in procs {
        if p.ppid != p.pid {
            children.entry(p.ppid).or_default().push(p.pid);
        }
    }
    let tree = |root: u32| -> Vec<u32> {
        let mut out = Vec::new();
        let mut stack = vec![root];
        let mut seen = HashSet::new();
        while let Some(pid) = stack.pop() {
            if !seen.insert(pid) {
                continue;
            }
            out.push(pid);
            if let Some(kids) = children.get(&pid) {
                stack.extend(kids.iter().copied());
            }
        }
        out
    };

    let mut panes: HashMap<u64, PaneUsage> = HashMap::new();
    let mut external = ExternalUsage::default();
    let mut heavy = Vec::new();

    for p in procs {
        let kind = kinds[&p.pid];
        let (helper, heavy_ancestor) = ancestry(p.pid);
        // シェル自身は補助に数えない（Bash ツールのシェル = ここから先は agent が走らせたコマンド）
        let helper = helper && kind != ProcKind::Shell;
        let is_agentish = matches!(kind, ProcKind::Agent(_)) || helper;
        let owner = owner_of(p.pid, &parents, roots, &env_pane, &names);
        let memory = p.footprint.unwrap_or(0);
        let cpu_now = cpu.get(&p.key()).copied().unwrap_or(0.0);
        match &owner {
            Owner::Pane(pane) => {
                let entry = panes.entry(*pane).or_insert_with(|| PaneUsage {
                    pane: *pane,
                    ..PaneUsage::default()
                });
                entry.memory += memory;
                entry.cpu_percent += cpu_now;
                entry.processes += 1;
                if is_agentish {
                    entry.agent_memory += memory;
                }
                if let ProcKind::Agent(a) = kind {
                    entry.agent.get_or_insert(a);
                }
            }
            Owner::External { .. } => {
                external.memory += memory;
                external.cpu_percent += cpu_now;
                external.processes += 1;
            }
        }
        if kind.is_heavy() && !heavy_ancestor && !helper {
            let members = tree(p.pid);
            let mut job_memory = 0u64;
            let mut job_cpu: Option<f64> = None;
            for pid in &members {
                let Some(m) = by_pid.get(pid) else { continue };
                job_memory += m.footprint.unwrap_or(0);
                if let Some(c) = cpu.get(&m.key()) {
                    *job_cpu.get_or_insert(0.0) += c;
                }
            }
            let label = p
                .argv
                .as_deref()
                .map(|a| short_label(a, 80))
                .unwrap_or_else(|| p.name.clone());
            heavy.push(HeavyJob {
                root: p.key(),
                kind,
                label,
                owner: owner.clone(),
                memory: job_memory,
                cpu_percent: job_cpu,
                processes: members.len() as u32,
            });
        }
    }
    for job in &heavy {
        match &job.owner {
            Owner::Pane(pane) => {
                if let Some(entry) = panes.get_mut(pane) {
                    entry.heavy += 1;
                    entry.heavy_memory += job.memory;
                }
            }
            Owner::External { .. } => external.heavy += 1,
        }
    }
    heavy.sort_by(|a, b| b.memory.cmp(&a.memory).then(a.root.pid.cmp(&b.root.pid)));
    let mut panes: Vec<PaneUsage> = panes.into_values().collect();
    panes.sort_by(|a, b| b.memory.cmp(&a.memory).then(a.pane.cmp(&b.pane)));
    Report {
        heavy,
        panes,
        external,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(pid: u32, ppid: u32, name: &str, argv: &str, mb: u64) -> ProcSample {
        ProcSample {
            pid,
            ppid,
            start: u64::from(pid) * 1000,
            name: name.to_string(),
            footprint: Some(mb * MIB),
            cpu_ns: Some(0),
            argv: (!argv.is_empty()).then(|| argv.to_string()),
            env_pane: None,
            isolated: false,
        }
    }

    #[test]
    fn メモリの内訳はactivity_monitorと同じ式() {
        // 2026-10-12 の実測（16 KiB ページ）を丸めた値
        let page = 16 * 1024;
        let vm = VmPages {
            page_size: page,
            free: 1000,
            active: 600_000,
            inactive: 590_000,
            wired: 314_000,
            purgeable: 10_000,
            speculative: 2_000,
            compressor: 950_000,
            external: 340_000,
            internal: 660_000,
        };
        let total = 36 * 1024 * MIB;
        let m = memory_breakdown(total, &vm);
        assert_eq!(m.app, (660_000 - 10_000) * page);
        assert_eq!(m.wired, 314_000 * page);
        assert_eq!(m.compressed, 950_000 * page);
        assert_eq!(m.cached, (340_000 + 10_000) * page);
        assert_eq!(m.used, m.app + m.wired + m.compressed);
        assert_eq!(m.available, total - m.used);
    }

    #[test]
    fn 使用が全体を超えても空きは0で止まる() {
        let vm = VmPages {
            page_size: 4096,
            internal: 10,
            wired: 10,
            compressor: 10,
            ..VmPages::default()
        };
        let m = memory_breakdown(4096, &vm);
        assert_eq!(m.available, 0);
    }

    #[test]
    fn メモリ圧の段() {
        assert_eq!(Pressure::from_level(1), Some(Pressure::Normal));
        assert_eq!(Pressure::from_level(2), Some(Pressure::Warn));
        assert_eq!(Pressure::from_level(4), Some(Pressure::Critical));
        assert_eq!(Pressure::from_level(0), None);
        assert_eq!(Pressure::from_level(3), None);
    }

    #[test]
    fn 機全体のcpu使用率は差分から出す() {
        let a = CpuTicks {
            user: 100,
            system: 50,
            idle: 850,
            nice: 0,
        };
        let b = CpuTicks {
            user: 160,
            system: 70,
            idle: 1_000,
            nice: 20,
        };
        // busy の差 100 / 全体の差 250 = 40%
        assert_eq!(cpu_busy_percent(a, b), Some(40.0));
        assert_eq!(cpu_busy_percent(a, a), None, "進んでいなければ出さない");
        assert_eq!(cpu_busy_percent(b, a), None, "巻き戻りは出さない");
    }

    #[test]
    fn mach_tickはtimebaseで換算する() {
        // 2026-10-12 の実測: raw 4,233,668,561 tick × 125/3 = 176.40 秒（ps の 2:56.39）
        let ns = mach_to_ns(4_233_668_561, 125, 3);
        assert_eq!(ns / 10_000_000, 17_640, "176.40 秒");
        assert_eq!(mach_to_ns(1_000, 1, 1), 1_000, "Intel は 1/1");
        assert_eq!(
            mach_to_ns(1_000, 125, 0),
            1_000,
            "壊れた timebase は換算しない"
        );
        assert_eq!(mach_to_ns(u64::MAX, 125, 3), u64::MAX, "溢れは飽和");
    }

    #[test]
    fn プロセスのcpu率() {
        assert_eq!(
            cpu_percent(1_000_000_000, 1_500_000_000, 1_000_000_000),
            Some(50.0)
        );
        assert_eq!(
            cpu_percent(0, 4_000_000_000, 1_000_000_000),
            Some(400.0),
            "4 スレッドなら 400"
        );
        assert_eq!(cpu_percent(5, 4, 1), None);
        assert_eq!(cpu_percent(0, 1, 0), None);
    }

    #[test]
    fn claudeは実行ファイル名が版数でもargvで見分ける() {
        assert_eq!(
            kind_of("2.1.294", Some("claude --model opus"), false),
            ProcKind::Agent("claude")
        );
        assert_eq!(
            kind_of(
                "2.1.294",
                Some("/opt/x/.local/share/claude/versions/2.1.294 -c"),
                false
            ),
            ProcKind::Agent("claude"),
            "版数のディレクトリから直に起こした形"
        );
        assert_eq!(
            kind_of("2.1.294", Some("/opt/tool/2.1.294 --run"), false),
            ProcKind::Other,
            "版数の名前だけでは claude と言わない"
        );
        assert_eq!(
            kind_of("node", Some("node /x/server.js --claude"), false),
            ProcKind::Other,
            "引数の中の語では誤爆しない"
        );
        assert_eq!(kind_of("codex", None, false), ProcKind::Agent("codex"));
        assert_eq!(
            kind_of(
                "node",
                Some("node /usr/lib/node_modules/@openai/codex/bin/codex.js"),
                false
            ),
            ProcKind::Agent("codex")
        );
        assert_eq!(
            kind_of("agy", Some("agy chat"), false),
            ProcKind::Agent("agy")
        );
    }

    #[test]
    fn ビルドとブラウザと隔離guiの分類() {
        assert_eq!(
            kind_of("cargo", Some("cargo test -p tako-core"), false),
            ProcKind::Build
        );
        assert_eq!(
            kind_of("rustc", Some("rustc --crate-name x"), false),
            ProcKind::Build
        );
        assert_eq!(
            kind_of(
                "tako_core-0a1b2",
                Some("/w/target/debug/deps/tako_core-0a1b2 --test-threads=2"),
                false
            ),
            ProcKind::Build,
            "テストの実行体（実行ファイル名は 16 文字で切れるので argv で見る）"
        );
        assert_eq!(kind_of("build-script-bui", None, false), ProcKind::Build);
        assert_eq!(
            kind_of("node", Some("node /x/bin/npm-cli.js run build"), false),
            ProcKind::Build
        );
        assert_eq!(
            kind_of("node", Some("node /x/.bin/vite build"), false),
            ProcKind::Build
        );
        assert_eq!(
            kind_of("node", Some("node /x/server.js"), false),
            ProcKind::Other
        );
        assert_eq!(kind_of("chrome-headless-", None, false), ProcKind::Browser);
        assert_eq!(kind_of("tako-app", None, true), ProcKind::Gui);
        assert_eq!(
            kind_of("tako-app", None, false),
            ProcKind::Other,
            "本番の tako 自身は重い処理ではない"
        );
        assert_eq!(kind_of("-zsh", None, false), ProcKind::Shell);
        assert_eq!(
            kind_of("Google Chrome He", None, false),
            ProcKind::Other,
            "普段の Chrome は数えない"
        );
    }

    #[test]
    fn 帰属はペインの根の祖先が先で次にenvで最後が外() {
        let parents: HashMap<u32, u32> = [(10, 1), (11, 10), (12, 11), (20, 1), (30, 1), (31, 30)]
            .into_iter()
            .collect();
        let names: HashMap<u32, String> = [
            (10, "tmux"),
            (11, "zsh"),
            (12, "cargo"),
            (20, "node"),
            (30, "Terminal"),
            (31, "cargo"),
        ]
        .into_iter()
        .map(|(p, n)| (p, n.to_string()))
        .collect();
        let roots: HashMap<u32, u64> = [(11, 7)].into_iter().collect();
        // launchd へ付け替わった子（nohup）は env の TAKO_PANE_ID で
        let env: HashMap<u32, u64> = [(20, 9)].into_iter().collect();
        assert_eq!(owner_of(12, &parents, &roots, &env, &names), Owner::Pane(7));
        assert_eq!(
            owner_of(11, &parents, &roots, &env, &names),
            Owner::Pane(7),
            "根そのもの"
        );
        assert_eq!(owner_of(20, &parents, &roots, &env, &names), Owner::Pane(9));
        assert_eq!(
            owner_of(31, &parents, &roots, &env, &names),
            Owner::External {
                origin: Some("Terminal".into())
            }
        );
    }

    #[test]
    fn envのペイン番号は自分の受け口のときだけ信じる() {
        let own = Some("/tmp/tako.sock");
        assert_eq!(env_pane_of(Some("3"), Some("/tmp/tako.sock"), own), Some(3));
        assert_eq!(
            env_pane_of(Some("3"), Some("/tmp/iso/tako.sock"), own),
            None,
            "隔離の tako のペイン 3 を本番のペイン 3 と取り違えない"
        );
        assert_eq!(env_pane_of(Some("3"), None, own), None);
        assert_eq!(env_pane_of(Some("3"), Some("/tmp/tako.sock"), None), None);
        assert_eq!(env_pane_of(Some("x"), Some("/tmp/tako.sock"), own), None);
    }

    #[test]
    fn pidの再利用は開始時刻で見分ける() {
        let a = ProcSample {
            pid: 42,
            start: 100,
            ..ProcSample::default()
        };
        let b = ProcSample {
            pid: 42,
            start: 200,
            ..ProcSample::default()
        };
        assert_ne!(a.key(), b.key());
        // 前回の CPU% が別の開始時刻の鍵で入っていても、今回の 42 には効かない
        let cpu: HashMap<ProcKey, f64> = [(a.key(), 90.0)].into_iter().collect();
        let report = summarize(
            &[ProcSample {
                name: "cargo".into(),
                argv: Some("cargo build".into()),
                ..b
            }],
            &cpu,
            &HashMap::new(),
        );
        assert_eq!(report.heavy.len(), 1);
        assert_eq!(report.heavy[0].cpu_percent, None);
    }

    #[test]
    fn 重い処理は一番上の祖先で1件にまとめ木の合計を出す() {
        let procs = vec![
            sample(10, 1, "tmux", "tmux", 5),
            sample(11, 10, "zsh", "-zsh", 3),
            sample(12, 11, "cargo", "cargo test -p tako-core", 50),
            sample(13, 12, "rustc", "rustc --crate-name a", 1000),
            sample(14, 12, "rustc", "rustc --crate-name b", 800),
            sample(15, 12, "sh", "sh -c cc", 1),
            sample(16, 15, "cc", "cc -c x.c", 20),
        ];
        let roots: HashMap<u32, u64> = [(11, 4)].into_iter().collect();
        let cpu: HashMap<ProcKey, f64> = procs
            .iter()
            .filter(|p| p.pid >= 12)
            .map(|p| (p.key(), 10.0))
            .collect();
        let r = summarize(&procs, &cpu, &roots);
        assert_eq!(
            r.heavy.len(),
            1,
            "rustc / cc は cargo の木に入る: {:?}",
            r.heavy
        );
        let job = &r.heavy[0];
        assert_eq!(job.root.pid, 12);
        assert_eq!(job.memory, (50 + 1000 + 800 + 1 + 20) * MIB);
        assert_eq!(job.processes, 5);
        assert_eq!(job.cpu_percent, Some(50.0));
        assert_eq!(job.owner, Owner::Pane(4));
        assert_eq!(job.label, "cargo test -p tako-core");
        let pane = &r.panes[0];
        assert_eq!(pane.pane, 4);
        assert_eq!(pane.heavy, 1);
        assert_eq!(
            pane.memory,
            (3 + 50 + 1000 + 800 + 1 + 20) * MIB,
            "ペインの根から下"
        );
    }

    #[test]
    fn agentの補助プロセスは重い処理に数えずbashツールのビルドは数える() {
        let procs = vec![
            sample(11, 1, "zsh", "-zsh", 3),
            sample(20, 11, "2.1.294", "claude --model opus", 300),
            // MCP サーバー（シェルを挟まない）とその Chromium
            sample(
                21,
                20,
                "node",
                "node /x/bin/npm-cli.js exec @playwright/mcp@latest",
                30,
            ),
            sample(22, 21, "chrome-headless-", "", 200),
            // Bash ツール（シェルを挟む）から起きたビルド
            sample(23, 20, "zsh", "/bin/zsh -c cargo build", 2),
            sample(24, 23, "cargo", "cargo build", 40),
        ];
        let roots: HashMap<u32, u64> = [(11, 5)].into_iter().collect();
        let r = summarize(&procs, &HashMap::new(), &roots);
        assert_eq!(r.heavy.len(), 1, "{:?}", r.heavy);
        assert_eq!(r.heavy[0].root.pid, 24);
        let pane = &r.panes[0];
        assert_eq!(pane.agent, Some("claude"));
        assert_eq!(
            pane.agent_memory,
            (300 + 30 + 200) * MIB,
            "本体 + MCP + その Chromium"
        );
    }

    #[test]
    fn tako_の外は外として数える() {
        let procs = vec![
            sample(30, 1, "Terminal", "", 100),
            sample(31, 30, "login", "login -pf x", 1),
            sample(32, 31, "zsh", "-zsh", 2),
            sample(33, 32, "cargo", "cargo build --release", 60),
        ];
        let r = summarize(&procs, &HashMap::new(), &HashMap::new());
        assert_eq!(r.external.heavy, 1);
        assert_eq!(r.external.processes, 4);
        assert_eq!(
            r.heavy[0].owner,
            Owner::External {
                origin: Some("Terminal".into())
            }
        );
        assert!(r.panes.is_empty());
    }

    #[test]
    fn 表示用のコマンド行はパスと秘密を出さない() {
        assert_eq!(
            short_label(
                "/Users/testuser/dev/tako/target/debug/deps/tako_core-ab12 --test-threads=2",
                80
            ),
            "tako_core-ab12 --test-threads=2"
        );
        assert_eq!(
            short_label("node /home/u/x/cli.js --token=abc API_KEY=xyz --port=3", 80),
            "node cli.js --token=*** API_KEY=*** --port=3"
        );
        let long = short_label(&format!("cargo {}", "x".repeat(200)), 20);
        assert_eq!(long.chars().count(), 20);
        assert!(long.ends_with('…'));
    }
}
