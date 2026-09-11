//! autorename — タブ・ペイン名の AI 自動リネーム（FR-2.12）
//!
//! 方式（2026-06-12 ユーザー承認の「tako 常駐」方式）: UI 層のポーリングループが
//! タブごとの素材指紋（cwd / OSC タイトル / 実行状態）の変化を検知し、静穏（デバウンス）後に
//! `claude -p --model <haiku>` を子プロセスで 1 回叩いて短い名前を生成、結果を
//! tako-core の `set_title_auto`（手動リネーム優先。FR-2.12.3）へ反映する。
//! **判断ロジックは持たず、プロンプト 1 本に閉じる**（FR-2.12.2）。
//! claude CLI が見つからない環境では OSC タイトル・cwd からのヒューリスティック命名へ
//! フォールバックする（FR-2.12.5）。ON/OFF は dispatch の `AutoRename`（FR-2.12.4）。
//!
//! このモジュールは GPUI 非依存（ループの駆動と素材収集だけ main.rs 側）。
//!
//! 品質改善（#552。新規ユーザー視点レビューで「10 分でタブ名が 5 回変化」「打ち間違い
//! 1 回で `claude失敗`」「簡体字 `开発` の混入」が観測された）:
//! ① 命名済みタブの再命名は 5 分に 1 回まで（`RENAME_MIN_INTERVAL`）
//! ② 一時的な失敗（command not found・非ゼロ終了）は素材にしない
//!    （`is_transient_failure` / `material_state`）
//! ③ 生成言語を UI 言語に固定し、出力を字種で検査する（`sanitize_title`）
//! ④ 自動命名直後だけ出る「この名前を固定」の印（`PIN_HINT_TTL`。UI は tab_bar.rs）

mod jis_kanji;

use std::collections::hash_map::DefaultHasher;
use std::collections::HashMap;
use std::ffi::OsString;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use tako_core::i18n::Lang;
use tako_core::CommandState;

/// 検知ループのポーリング間隔
pub const POLL_INTERVAL: Duration = Duration::from_secs(2);
/// 素材が変化しなくなってからリネームを発火するまでの静穏時間（デバウンス）
const DEBOUNCE: Duration = Duration::from_secs(4);
/// **命名済み**タブを再命名するまでの最小間隔（#552 案 1）。
/// 数十秒おきに名前が書き換わるとタブバーが目印として機能しなくなるため、
/// 2 回目以降は 5 分に 1 回へ落とす
const RENAME_MIN_INTERVAL: Duration = Duration::from_secs(300);
/// まだ名前が付いていないタブの再試行間隔。初回の命名を 5 分待たせないための例外で、
/// 従来のクールダウン（claude 呼び出しの浪費防止）をそのまま使う
const FIRST_NAME_COOLDOWN: Duration = Duration::from_secs(30);
/// 「この名前を固定」の印を出しておく時間（#552 案 4）。自動命名の**直後だけ**出す
pub const PIN_HINT_TTL: Duration = Duration::from_secs(120);
/// claude 子プロセスの待ち時間上限（超過は kill してヒューリスティックへ）。
///
/// #722 の実測（Windows 11・claude 実呼び出し・この関数と同じ起動形）で 30 秒では
/// 足りないと判明したので広げた。**アイドル状態でも 19.8 / 25.2 / 31.0 秒**
/// （起動そのものは 0.5 秒なので待ち時間の大半は応答待ち）で 3 回に 1 回は 30 秒を超え、
/// 隔離 GUI では 80.2 秒かかった。上限に張り付くと「AI 命名が黙ってヒューリスティックへ
/// 落ちる」= #722 と見分けの付かない症状になる。
///
/// 呼び出しはバックグラウンドスレッドで、同じタブへは [`RENAME_MIN_INTERVAL`] /
/// [`FIRST_NAME_COOLDOWN`] 以内に再発火しない。伸ばして困るのは「claude が本当に
/// ハングしたとき、そのスレッドが待ち続ける時間」だけなので、取りこぼしを無くす側へ倒す。
///
/// #758 で [`STRICT_MCP_FLAG`] を入れたあとに**縮められるか再検討したが、縮めない**。
/// macOS の実測（同一プロンプト・順序を交互にした 10 ラウンド）はフラグ付きでも
/// **中央値 16.9 秒 / 最大 43.6 秒**、隔離 GUI の実命名でも 38.7 秒かかった回がある。
/// 遅い側の裾はフラグでは短くならない（MCP の起動ぶんが消えるだけで、応答待ちは残る）ので、
/// 上限を裾の近くまで下げると「AI 命名が黙ってヒューリスティックへ落ちる」= #722 と
/// 見分けの付かない症状を macOS でも起こす。Windows は macOS より遅い側で、
/// この上限を決めた #722 の実測から取り直せていない（実機 offline）
const CLAUDE_TIMEOUT: Duration = Duration::from_secs(120);
/// claude が終わったあと、その stdout が閉じるのを待つ上限（#758）。
///
/// 通常は即座に閉じる。閉じないのは claude の**孫プロセス**（MCP サーバー）が
/// パイプを握ったまま残っているときで、[`STRICT_MCP_FLAG`] が効いていれば起こらない。
/// ここに上限が無いと命名スレッドがそのまま止まり続けるので、諦めて
/// ヒューリスティック命名へ落とす
const READ_GRACE: Duration = Duration::from_secs(10);
/// 安価・高速なモデルを固定で使う（FR-2.12.2）
const MODEL: &str = "claude-haiku-4-5-20251001";
/// プロンプトに含めるペイン末尾の行数と 1 行の最大文字数
const TAIL_LINES: usize = 6;
const TAIL_CHARS: usize = 120;
/// 生成タイトルの上限文字数（モデルの暴走出力対策）
const MAX_TAB_TITLE: usize = 16;
const MAX_PANE_TITLE: usize = 24;

/// 1 ペイン分の命名素材（FR-2.12.1 で list にも公開している情報の写し + 画面末尾）
#[derive(Debug, Clone)]
pub struct PaneMaterials {
    pub pane: u64,
    pub role: Option<String>,
    pub osc_title: Option<String>,
    pub cwd: Option<String>,
    pub state: &'static str,
    /// 画面末尾の数行（指紋には含めない。プロンプトの文脈用）
    pub tail: Vec<String>,
}

/// 1 タブ分の命名素材。手動リネーム済みのタブ / ペインは収集側で除外する（FR-2.12.3）
#[derive(Debug, Clone)]
pub struct TabMaterials {
    pub tab: u64,
    /// タブ名の生成も求めるか（タブが手動リネーム済みなら false）
    pub rename_tab: bool,
    pub panes: Vec<PaneMaterials>,
}

/// 生成された名前。ペインは (id, 新タイトル)
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RenamePlan {
    pub tab: Option<String>,
    pub panes: Vec<(u64, String)>,
}

/// 1 タブ分の検知入力（`AutoRenamer::tick` のスナップショット要素）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TabSignal {
    pub tab: u64,
    /// 素材指紋（cwd / OSC タイトル / 実行状態 / 手動フラグ）
    pub fingerprint: u64,
    /// すでに自動命名済みか。命名済みだけが最小間隔（#552 案 1）の対象で、
    /// まだ名無しのタブは待たせずに名付ける
    pub named: bool,
}

/// タブごとの監視状態（指紋 + デバウンス + 最小間隔）
struct TabWatch {
    fingerprint: u64,
    /// この指紋を最初に観測した時刻（静穏判定の起点）
    since: Instant,
    /// 発火済みの指紋（同じ状態への再発火を防ぐ）
    done_fingerprint: u64,
    last_run: Option<Instant>,
}

/// 検知ループの状態。`enabled` は dispatch の `AutoRename`（FR-2.12.4）から切り替わる
pub struct AutoRenamer {
    pub enabled: bool,
    watches: HashMap<u64, TabWatch>,
}

impl AutoRenamer {
    pub fn new(enabled: bool) -> Self {
        Self {
            enabled,
            watches: HashMap::new(),
        }
    }

    /// 1 tick 分の判定。`tabs` は各タブのスナップショット。
    /// 戻り値は「静穏が確認でき、リネームを発火すべきタブ ID」。
    ///
    /// 発火条件は 3 つ揃ったとき: 素材が静穏（デバウンス）/ その指紋がまだ未処理 /
    /// 前回の発火から最小間隔が空いている（命名済み = 5 分、名無し = 30 秒）。
    /// **ユーザーの手動リネームはこの経路を通らない**（dispatch 直行）ので、
    /// 最小間隔に関係なくいつでも即座に反映される
    pub fn tick(&mut self, tabs: &[TabSignal], now: Instant) -> Vec<u64> {
        // 閉じられたタブの監視を捨てる
        self.watches
            .retain(|id, _| tabs.iter().any(|t| t.tab == *id));
        if !self.enabled {
            return Vec::new();
        }
        let mut fire = Vec::new();
        for signal in tabs {
            let watch = self.watches.entry(signal.tab).or_insert(TabWatch {
                fingerprint: signal.fingerprint,
                since: now,
                done_fingerprint: 0,
                last_run: None,
            });
            if watch.fingerprint != signal.fingerprint {
                watch.fingerprint = signal.fingerprint;
                watch.since = now;
                continue;
            }
            let calm = now.duration_since(watch.since) >= DEBOUNCE;
            let fresh = watch.done_fingerprint != signal.fingerprint;
            let interval = if signal.named {
                RENAME_MIN_INTERVAL
            } else {
                FIRST_NAME_COOLDOWN
            };
            let cooled = watch
                .last_run
                .is_none_or(|t| now.duration_since(t) >= interval);
            if calm && fresh && cooled {
                // 失敗時の連打を防ぐため、結果を待たず発火済みとして記録する
                watch.done_fingerprint = signal.fingerprint;
                watch.last_run = Some(now);
                fire.push(signal.tab);
            }
        }
        fire
    }
}

/// 素材指紋（変化検知用）。出力末尾は含めない（実行中は毎 tick 変わり静穏にならないため、
/// cwd / OSC タイトル / 実行状態の「節目」だけで判定する）
pub fn fingerprint<T: Hash>(value: &T) -> u64 {
    let mut hasher = DefaultHasher::new();
    value.hash(&mut hasher);
    hasher.finish()
}

/// 診断出力（`TAKO_AUTORENAME_DIAG=1` のときだけ stderr へ）。
///
/// この機能の失敗は全部「黙ってヒューリスティックへ落ちる」形で出る。#722 は claude が
/// 解決できていないせいだったが、解決不能・非ゼロ終了・タイムアウト・パース失敗が
/// 外から一切見分けられないため、突き止めるのに毎回ビルドし直す羽目になった。
///
/// **中身は絶対に出さない**（プロンプト・claude の出力・画面末尾はペイン内容そのもの。
/// 診断ログへ出さないのは AGENTS.md の絶対ルール）。出すのは解決したパス・終了コード・
/// 所要時間・出力バイト数といったメタ情報だけ
fn diag(args: std::fmt::Arguments<'_>) {
    static ON: OnceLock<bool> = OnceLock::new();
    if *ON.get_or_init(|| std::env::var_os("TAKO_AUTORENAME_DIAG").is_some()) {
        eprintln!("[autorename] {args}");
    }
}

/// 名前の生成。claude CLI があればプロンプト 1 本で生成し、
/// 無い・失敗した場合はヒューリスティック命名へフォールバックする（FR-2.12.5）。
/// 生成言語は UI 言語に固定する（#552 案 3）
pub fn generate(materials: &TabMaterials) -> RenamePlan {
    let lang = tako_core::i18n::lang();
    if let Some(bin) = claude_bin() {
        if let Some(plan) = run_claude(bin, materials, lang) {
            diag(format_args!("tab {}: AI 命名を採用", materials.tab));
            return plan;
        }
        diag(format_args!(
            "tab {}: AI 命名に失敗 → ヒューリスティックへ",
            materials.tab
        ));
    }
    heuristic_plan(materials)
}

/// claude CLI の場所（プロセス内で 1 回だけ解決してキャッシュする）。
/// 探索の作法は OS で違うので抽象境界 B16（`platform::exe`）に任せる
pub fn claude_bin() -> Option<&'static Path> {
    static BIN: OnceLock<Option<PathBuf>> = OnceLock::new();
    BIN.get_or_init(|| {
        let found = detect_claude();
        match &found {
            Some(p) => diag(format_args!("claude を解決: {}", p.display())),
            None => diag(format_args!(
                "claude を解決できない → 以後ヒューリスティック命名のみ（#722）"
            )),
        }
        found
    })
    .as_deref()
}

fn detect_claude() -> Option<PathBuf> {
    resolve_claude(
        std::env::var_os("TAKO_SELF_TEST").is_some(),
        std::env::var_os("TAKO_CLAUDE_BIN"),
        &|| tako_core::platform::exe::find("claude"),
        &|p| p.is_file(),
    )
}

/// claude の解決手順（純粋関数。探索と実ファイル判定を注入して**両プラットフォームから
/// テストできる**ようにしてある）。
///
/// 探索そのものは抽象境界 B16（`platform::exe`）に委ねる。**ここでログインシェル経由の
/// `command -v claude` を直に叩いてはいけない**（#722）: Windows には `SHELL` も
/// `/bin/sh` も無いので起動が失敗し、`.ok()?` で静かに `None` へ化ける。
/// `claude_bin()` は `OnceLock` なので、以後プロセスが生きている限り AI 命名は永久に無効
/// （ログにも UI にも何も出ない）。B16 なら macOS はログインシェル経由
/// （`.app` の痩せた PATH 対策）、Windows は PATH + `PATHEXT` + ユーザー導入先の走査、
/// と作法が分かれ、npm 由来の `claude.cmd` シムでも解決できる
fn resolve_claude(
    self_test: bool,
    explicit: Option<OsString>,
    lookup: &dyn Fn() -> Option<String>,
    is_file: &dyn Fn(&Path) -> bool,
) -> Option<PathBuf> {
    // セルフテスト中は実 LLM を呼ばない（ヒューリスティック経路のみ機械検証する）
    if self_test {
        return None;
    }
    // 明示指定（検証・差し替え用）。指定があるのに実在しないなら探索へ落とさず None
    // ＝ 差し替えたつもりで本物が動く事故を防ぐ
    if let Some(path) = explicit {
        let path = PathBuf::from(path);
        return is_file(&path).then_some(path);
    }
    let path = PathBuf::from(lookup()?);
    is_file(&path).then_some(path)
}

/// `claude -p` へ渡す「MCP サーバーを一切使わない」フラグ（#758）。
///
/// `--mcp-config` を伴わずに渡すと、ユーザーの MCP 設定（tako 自身の MCP サーバーを含む）を
/// **1 つも起動しない**。タブ名を 1 個作るだけの使い捨て呼び出しには、全サーバーの起動と
/// 接続は過剰で、遅いうえに命名ヘルパーへペイン操作ツール一式を持たせてしまう。
///
/// macOS の実測（claude 2.1.258・MCP サーバー 9 本・順序を交互にした 10 ラウンド）:
///
/// | 測り方 | 既定 | このフラグ付き |
/// |---|---|---|
/// | 命名プロンプト（本番と同じ文面）の所要 p50 | 18.4s | **16.9s**（対応差の中央値 **-3.8s**・10 回中 9 回速い） |
/// | 最小プロンプト（応答 1 文字）の所要 p50 = ほぼ固定費 | 5.8s | **3.7s**（同 **-2.1s**・分布がほぼ重ならない） |
/// | 起動する子孫プロセスのピーク | **17**（node / uv / python / tako 等） | **2** |
///
/// 所要のばらつきは応答待ちが支配するので、**速さより「17 → 2」のほうが本題**（#758）:
/// 命名は純粋なテキスト変換なのに、既定では tako の MCP サーバーごと起きていて、
/// 命名ヘルパーにペイン操作ツール一式が生えていた
const STRICT_MCP_FLAG: &str = "--strict-mcp-config";

/// [`STRICT_MCP_FLAG`] を付けてよいかの学習状態（#758）。
///
/// 未知のフラグを足すと**古い claude CLI では使い方エラーで即座に非ゼロ終了**し、
/// 自動命名が黙ってヒューリスティックへ落ちる（#722 と見分けの付かない症状になる）。
/// バージョン番号で分岐するより、1 回だけ実際に渡して確かめるほうが確実
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum StrictMcp {
    /// まだ 1 回も渡していない
    Unknown = 0,
    /// フラグ付きで通った（以後フラグ付きで固定。再試行しない）
    Supported = 1,
    /// フラグ付きが非ゼロ終了し、フラグ無しなら通った（古い CLI。以後フラグを付けない）
    Unsupported = 2,
}

impl StrictMcp {
    fn load(cell: &AtomicU8) -> Self {
        match cell.load(Ordering::Relaxed) {
            1 => Self::Supported,
            2 => Self::Unsupported,
            _ => Self::Unknown,
        }
    }

    fn store(self, cell: &AtomicU8) {
        cell.store(self as u8, Ordering::Relaxed);
    }
}

/// 学習状態の置き場（プロセス内で 1 つ。`claude_bin()` と同じ方針）
fn strict_state() -> &'static AtomicU8 {
    static STATE: AtomicU8 = AtomicU8::new(StrictMcp::Unknown as u8);
    &STATE
}

/// A/B の逃げ道（#758）: `TAKO_758_LEGACY=1` で [`STRICT_MCP_FLAG`] を一切付けない旧経路へ戻す
fn issue758_legacy() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("TAKO_758_LEGACY").is_some())
}

/// claude を 1 回起こした結果。**非ゼロ終了だけ**を他の失敗と区別するのは、
/// 未知のフラグを拒否した古い CLI がそこへ来るから（#758）
#[derive(Debug, PartialEq, Eq)]
enum ClaudeRun {
    /// 正常終了（中身は stdout）
    Ok(String),
    /// 非ゼロ終了
    NonZero,
    /// 起動できない / 上限まで応答が来ない。**引数の綴りとは無関係**なので再試行しない
    Failed,
}

/// claude -p を叩いて応答をパースする。失敗（起動不可・タイムアウト・パース不能）は
/// None（呼び出し側がヒューリスティックへ落とす）
fn run_claude(bin: &Path, materials: &TabMaterials, lang: Lang) -> Option<RenamePlan> {
    run_claude_with(strict_state(), issue758_legacy(), bin, materials, lang)
}

/// [`run_claude`] の本体（学習状態と A/B を注入して**偽 CLI でテストできる**ようにしてある）
fn run_claude_with(
    state: &AtomicU8,
    legacy: bool,
    bin: &Path,
    materials: &TabMaterials,
    lang: Lang,
) -> Option<RenamePlan> {
    let prompt = build_prompt(materials, lang);
    let output = run_learning_strict(state, legacy, &mut |strict| {
        spawn_claude(bin, &prompt, strict, CLAUDE_TIMEOUT)
    })?;
    let plan = parse_plan(&output, materials, lang);
    diag(format_args!(
        "claude 応答のパース{}",
        if plan.is_some() { "成功" } else { "失敗" }
    ));
    plan
}

/// [`STRICT_MCP_FLAG`] を学習しながら起こす（#758）。`run(strict)` が 1 回の起動。
///
/// - 未学習: フラグ付きで起こす。**非ゼロ終了のときだけ**フラグ無しで 1 回再試行し、
///   その再試行が通ったときにだけ「このフラグは使えない」と決める。
///   リミット・認証切れのような一時的な非ゼロ終了で速い経路を捨てないための条件で、
///   古い CLI なら使い方エラーが即座に返るので再試行の代償もほぼ無い
/// - 学習済み: 覚えた側だけを 1 回起こす（= 余計な再試行は初回だけ）
/// - タイムアウト・起動失敗では再試行しない。プロセスが立った時点でフラグは通っており、
///   もう一度待つと上限のぶんだけ二重に待たせるだけ
fn run_learning_strict(
    state: &AtomicU8,
    legacy: bool,
    run: &mut dyn FnMut(bool) -> ClaudeRun,
) -> Option<String> {
    if legacy {
        return match run(false) {
            ClaudeRun::Ok(output) => Some(output),
            _ => None,
        };
    }
    let learned = StrictMcp::load(state);
    let strict = learned != StrictMcp::Unsupported;
    match run(strict) {
        ClaudeRun::Ok(output) => {
            if strict && learned == StrictMcp::Unknown {
                StrictMcp::Supported.store(state);
                diag(format_args!("{STRICT_MCP_FLAG} は使える（以後付けたまま）"));
            }
            Some(output)
        }
        ClaudeRun::NonZero if strict && learned == StrictMcp::Unknown => {
            match run(false) {
                ClaudeRun::Ok(output) => {
                    StrictMcp::Unsupported.store(state);
                    diag(format_args!(
                        "{STRICT_MCP_FLAG} を拒否された → 以後付けない（古い claude CLI）"
                    ));
                    Some(output)
                }
                // フラグ無しでも駄目 = フラグのせいだと決められない。学習せず次回また試す
                _ => None,
            }
        }
        _ => None,
    }
}

/// claude -p を 1 回だけ起こして stdout を読む。
///
/// `timeout` は本番では [`CLAUDE_TIMEOUT`] 固定で、テストだけが短い値を渡す
/// （上限に当たったときの結果が [`ClaudeRun::Failed`] = 再試行しない側であることを
/// 実際に起こして確かめるため。ここを `NonZero` と取り違えると、上限まで待ったあとに
/// もう一度上限まで待つ = 命名 1 回が最悪 2 倍になる）
fn spawn_claude(bin: &Path, prompt: &str, strict: bool, timeout: Duration) -> ClaudeRun {
    use std::io::{Read, Write};
    use std::process::{Command, Stdio};

    let started = Instant::now();
    let mut command = Command::new(bin);
    command.args(["-p", "--model", MODEL]);
    if strict {
        command.arg(STRICT_MCP_FLAG);
    }
    // #586: GUI プロセスからの起動なので Windows でコンソールウィンドウを出させない
    let mut child = match tako_core::platform::process::no_console_window(&mut command)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
    {
        Ok(child) => child,
        Err(e) => {
            diag(format_args!("claude の起動に失敗: {e}"));
            return ClaudeRun::Failed;
        }
    };
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(prompt.as_bytes());
        // drop で stdin が閉じ、-p は EOF までをプロンプトとして読む
    }
    // stdout はパイプ詰まり防止のため別スレッドで吸い出しつつ、タイムアウト付きで待つ
    let Some(mut stdout) = child.stdout.take() else {
        let _ = child.kill();
        let _ = child.wait();
        return ClaudeRun::Failed;
    };
    // **`join` で待たない**（#758 の上限テストで見つけた）。`read_to_string` が返るのは
    // パイプが閉じたときで、閉じるのは**握っている全員**が消えたとき。claude は MCP
    // サーバーを子プロセスとして抱えるので、親を kill しても孫が stdout を握ったままなら
    // 待ちは孫が死ぬまで返らない（実測: 孫が 30 秒眠る偽 CLI で、上限 0.6 秒の打ち切りが
    // 30.3 秒待たされた = 上限が上限として効いていない）。期限付きで受け取り、
    // 間に合わなければ読み出しは諦める（スレッドはパイプが閉じた時点で自然に終わる）
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut buf = String::new();
        let _ = stdout.read_to_string(&mut buf);
        let _ = tx.send(buf);
    });
    let deadline = Instant::now() + timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(200));
            }
            _ => {
                diag(format_args!(
                    "claude を打ち切り: {:.1}s（上限 {}s）",
                    started.elapsed().as_secs_f32(),
                    timeout.as_secs()
                ));
                let _ = child.kill();
                let _ = child.wait();
                break None;
            }
        }
    };
    let flag = if strict { STRICT_MCP_FLAG } else { "既定" };
    // 打ち切ったときは出力を捨てるので 1 秒も待たない
    let Some(status) = status else {
        return ClaudeRun::Failed;
    };
    let Ok(output) = rx.recv_timeout(READ_GRACE) else {
        // claude は終わったのに stdout が閉じない = 孫が握ったまま。従来はここで
        // 命名スレッドが**永久に**止まっていた。ヒューリスティックへ落として先へ進む
        diag(format_args!(
            "claude の出力が {}s 以内に閉じない（孫プロセスが握っている）",
            READ_GRACE.as_secs()
        ));
        return ClaudeRun::Failed;
    };
    if status.success() {
        diag(format_args!(
            "claude 応答: {:.1}s / {} バイト / {flag}",
            started.elapsed().as_secs_f32(),
            output.len()
        ));
        ClaudeRun::Ok(output)
    } else {
        diag(format_args!(
            "claude が非ゼロ終了: code={:?}（{:.1}s / {flag}）",
            status.code(),
            started.elapsed().as_secs_f32()
        ));
        ClaudeRun::NonZero
    }
}

/// プロンプト 1 本（FR-2.12.2。判断・調整はすべてこの文面に閉じる）。
/// 出力言語は UI 言語に固定する（#552 案 3。日本語 UI に簡体字が出る事故を防ぐ）
fn build_prompt(materials: &TabMaterials, lang: Lang) -> String {
    let panes: Vec<serde_json::Value> = materials
        .panes
        .iter()
        .map(|p| {
            serde_json::json!({
                "pane": p.pane,
                "role": p.role,
                "osc_title": p.osc_title,
                "cwd": p.cwd,
                "state": p.state,
                "tail": p.tail,
            })
        })
        .collect();
    let data = serde_json::json!({ "tab": materials.tab, "panes": panes });
    match lang {
        Lang::Ja => {
            let target = if materials.rename_tab {
                "タブ全体（tab）と各ペイン"
            } else {
                "各ペイン（タブ名は不要）"
            };
            format!(
                "あなたはターミナルのタブ・ペインに短い名前を付ける係。\
                 以下の JSON は 1 つのタブ内の各ペインの状況（作業ディレクトリ cwd、実行状態 state、\
                 OSC タイトル osc_title、画面末尾の出力 tail）。\n\
                 {target}に、いま何をしているかがひと目で分かる短い名前を付けること\
                 （タブは {MAX_TAB_TITLE} 文字以内、ペインは {MAX_PANE_TITLE} 文字以内。\
                 コマンド名・プロジェクト名・ツール名は原文のまま使ってよい）。\n\
                 制約:\n\
                 - 言語は日本語。ひらがな・カタカナ・日本語の漢字（常用漢字）と半角英数字だけを使う。\
                 簡体字・繁体字など中国語専用の字体（开 发 图 环 单 时 验 など）は絶対に使わない。\n\
                 - コマンドの打ち間違い・command not found・一度きりの非ゼロ終了は\
                 「作業内容」ではないので名前にしない。失敗そのものを名前にせず、\
                 そのペインで進めている作業を表す名前を付ける。\n\
                 - 名前を変える必要がないペインは省略してよい。\n\
                 出力は次の形式の JSON だけ。説明文・コードフェンスは書かない:\n\
                 {{\"tab\":\"...\",\"panes\":{{\"<pane id>\":\"...\"}}}}\n\n{data}"
            )
        }
        Lang::En => {
            let target = if materials.rename_tab {
                "the tab (tab) and each pane"
            } else {
                "each pane (no tab name needed)"
            };
            format!(
                "You name terminal tabs and panes. The JSON below describes the panes of one tab \
                 (working directory cwd, run state state, OSC title osc_title, \
                 the last lines of the screen tail).\n\
                 Give {target} a short name that makes the current work obvious at a glance \
                 (tab: at most {MAX_TAB_TITLE} characters, pane: at most {MAX_PANE_TITLE} \
                 characters. Command, project and tool names may be used verbatim).\n\
                 Rules:\n\
                 - Write in English only. Use ASCII letters, digits and simple punctuation; \
                 never use CJK characters.\n\
                 - Typos, `command not found` and one-off non-zero exits are not \"work\": \
                 never name a pane after a failure. Name the work being done instead.\n\
                 - Panes that do not need a new name may be omitted.\n\
                 Reply with this JSON and nothing else. No prose, no code fences:\n\
                 {{\"tab\":\"...\",\"panes\":{{\"<pane id>\":\"...\"}}}}\n\n{data}"
            )
        }
    }
}

/// claude の応答から JSON を取り出して RenamePlan へ写す。
/// 素材に無いペイン ID は無視し、UI 言語に合わない字種のタイトルは捨て、
/// 残ったタイトルを上限へ丸める（#552 案 3）
fn parse_plan(output: &str, materials: &TabMaterials, lang: Lang) -> Option<RenamePlan> {
    let start = output.find('{')?;
    let end = output.rfind('}')?;
    let value: serde_json::Value = serde_json::from_str(output.get(start..=end)?).ok()?;
    let tab = value["tab"]
        .as_str()
        .filter(|_| materials.rename_tab)
        .and_then(|t| sanitize_title(t, lang))
        .map(|t| clamp_chars(&t, MAX_TAB_TITLE));
    let mut panes = Vec::new();
    if let Some(map) = value["panes"].as_object() {
        for (key, title) in map {
            let Ok(id) = key.parse::<u64>() else { continue };
            if !materials.panes.iter().any(|p| p.pane == id) {
                continue;
            }
            if let Some(title) = title.as_str().and_then(|t| sanitize_title(t, lang)) {
                panes.push((id, clamp_chars(&title, MAX_PANE_TITLE)));
            }
        }
    }
    if tab.is_none() && panes.is_empty() {
        return None;
    }
    Some(RenamePlan { tab, panes })
}

/// 生成タイトルの字種検査（#552 案 3）。UI 言語に合わない文字が混ざった名前は
/// 採用しない（呼び出し側は残りが空ならヒューリスティック命名へ落ちる）。
///
/// 日本語 UI では、実際に観測された簡体字（`开発` の `开`）のように**日本語字体が
/// 存在する簡体字は置き換えてから**検査する。置換表に無い中国語専用字が残っていれば
/// その名前ごと捨てる（誤った字で固定するより名無しのほうがまし）。
///
/// 限界: 判定は「日本語の漢字集合（CP932）に無い字が混ざっていないか」なので、
/// 日本語にも存在する字だけで書かれた中国語（`那个` 等）は通る。狙いは
/// **日本語の名前に簡体字が滑り込むこと**の遮断であり、中国語の検出ではない
fn sanitize_title(title: &str, lang: Lang) -> Option<String> {
    let title: String = title
        .trim()
        .chars()
        .map(|ch| match lang {
            Lang::Ja => localize_han(ch),
            Lang::En => ch,
        })
        .collect();
    let title = title.trim();
    if title.is_empty() || !title.chars().all(|ch| is_allowed_char(ch, lang)) {
        return None;
    }
    Some(title.to_string())
}

/// UI 言語で許してよい文字か。記号・約物は言語共通、文字体系だけを言語で切り分ける
fn is_allowed_char(ch: char, lang: Lang) -> bool {
    if ch.is_ascii() {
        return !ch.is_control();
    }
    // 言語に依存しない記号（ラテン補助・一般約物・矢印・罫線）
    if matches!(ch,
        '\u{00A0}'..='\u{00FF}' | '\u{2010}'..='\u{206F}' | '\u{2190}'..='\u{21FF}'
        | '\u{2500}'..='\u{25FF}')
    {
        return true;
    }
    match lang {
        // 全角記号（々〜「」）/ ひらがな / カタカナ / 全角英数 / 日本語の漢字
        Lang::Ja => {
            matches!(ch,
                '\u{3000}'..='\u{303F}' | '\u{3040}'..='\u{30FF}'
                | '\u{FF01}'..='\u{FF60}' | '\u{FFE0}'..='\u{FFE6}')
                || jis_kanji::CP932_KANJI.contains(ch)
        }
        Lang::En => false,
    }
}

/// 簡体字 → 日本語字体（#552 案 3）。狙いは「ほぼ日本語なのに 1〜2 字だけ簡体字が
/// 混ざる」滑り（実観測: `开発`）の救済なので、ターミナルの命名に出る技術語
/// （開発・環境・検証・実行・設定・接続…）を構成する字に絞る。
///
/// 中国語の機能語・量詞（`个` `为` `这` など）は**意図的に入れない**。入れると
/// 全体が中国語の名前まで日本語の字体へ化けて「那個」のような無意味な名前が
/// 通ってしまう。表に無い字が残れば `sanitize_title` がその名前ごと捨てる。
/// 日本語の漢字集合にも在る字（`并` `冲` `决` `网` 等）も入れない
/// （置換しなくても検査を通るので、書き換えるだけ余計）
fn localize_han(ch: char) -> char {
    SIMPLIFIED
        .chars()
        .position(|c| c == ch)
        .and_then(|i| JAPANESE.chars().nth(i))
        .unwrap_or(ch)
}

/// 置換表の左辺（簡体字）。`JAPANESE` と**同じ位置**の文字が対応する
/// （2 本の文字列にしているのは、100 件超の対を 1 対 1 行で並べずに読めるようにするため。
/// 長さと対応の正しさは `簡体字の置換表は左右が同じ長さで対応が正しい` が機械検査する）
const SIMPLIFIED: &str =
    "开关门问间闻阅闭发东车见页风马长说请读语论记认识议讨计设访证评词试话该详课调谈谢\
     讲译变对观欢汉权劝难树术图团园环现实军农边过达运连进远违迟选应样还错误败检报处执\
     库类结构编载传输备择击动态优测复归项务络单时验题线显标录视义习专业仓产";
/// 置換表の右辺（日本語字体）
const JAPANESE: &str =
    "開関門問間聞閲閉発東車見頁風馬長説請読語論記認識議討計設訪証評詞試話該詳課調談謝\
     講訳変対観歓漢権勧難樹術図団園環現実軍農辺過達運連進遠違遅選応様還錯誤敗検報処執\
     庫類結構編載伝輸備択撃動態優測復帰項務絡単時験題線顕標録視義習専業倉産";

/// ヒューリスティック命名（FR-2.12.5）: OSC タイトル > cwd の末尾ディレクトリ名。
/// どちらも無いペインは触らない。タブ名は最初に命名できたペインの名前を使う
pub fn heuristic_plan(materials: &TabMaterials) -> RenamePlan {
    let mut panes = Vec::new();
    for pane in &materials.panes {
        let title = pane
            .osc_title
            .as_deref()
            .map(str::trim)
            .filter(|t| !t.is_empty())
            .map(|t| clamp_chars(t, MAX_PANE_TITLE))
            .or_else(|| {
                pane.cwd
                    .as_deref()
                    .map(Path::new)
                    .and_then(Path::file_name)
                    .and_then(|n| n.to_str())
                    .map(|n| clamp_chars(n, MAX_PANE_TITLE))
            });
        if let Some(title) = title {
            panes.push((pane.pane, title));
        }
    }
    let tab = materials
        .rename_tab
        .then(|| panes.first().map(|(_, t)| clamp_chars(t, MAX_TAB_TITLE)))
        .flatten();
    RenamePlan { tab, panes }
}

/// 文字数上限への切り詰め（char 境界安全）
fn clamp_chars(text: &str, max: usize) -> String {
    text.chars().take(max).collect()
}

/// 素材用に画面末尾の行を整える（空行と一時的な失敗行を落とし、長い行を切り詰める）
pub fn trim_tail(lines: Vec<String>) -> Vec<String> {
    let mut tail: Vec<String> = lines
        .into_iter()
        .filter(|l| !l.trim().is_empty() && !is_transient_failure(l))
        .map(|l| clamp_chars(&l, TAIL_CHARS))
        .collect();
    if tail.len() > TAIL_LINES {
        tail.drain(..tail.len() - TAIL_LINES);
    }
    tail
}

/// 一時的な失敗の行か（#552 案 2）。打ち間違い・存在しないコマンド・シェルの使用法
/// エラーは「いま何をしているか」ではないので命名の材料から落とす。
/// テストの red やビルドエラーのような**作業の結果**は落とさない
/// （`error` `failed` 単体では判定しない）
pub fn is_transient_failure(line: &str) -> bool {
    let lower = line.to_lowercase();
    const NEEDLES: &[&str] = &[
        // シェル・OS（英語ロケール）
        "command not found",
        "no such file or directory",
        "permission denied",
        "operation not permitted",
        "not a git repository",
        "no matches found",
        "bad substitution",
        "event not found",
        "syntax error near unexpected token",
        // よくある CLI の使用法エラー
        "unknown command",
        "unknown option",
        "unrecognized option",
        "invalid option",
        "illegal option",
        "did you mean",
        // 日本語ロケールのシェル・coreutils
        "コマンドが見つかりません",
        "そのようなファイルやディレクトリはありません",
        "許可がありません",
        "権限がありません",
    ];
    if NEEDLES.iter().any(|n| lower.contains(n)) {
        return true;
    }
    // `usage: cmd ...` / `Usage:` は誤用時のヘルプ表示
    let trimmed = lower.trim_start();
    trimmed.starts_with("usage:") || trimmed.starts_with("使い方:")
}

/// 素材に載せる実行状態（#552 案 2）。直前のコマンドが失敗しただけの `failed` は
/// 「いま何をしているか」ではないので `idle` と同一視する。
/// これにより**打ち間違い 1 回では素材指紋が変わらず、リネームも発火しない**
/// （失敗そのものはタブバーの赤ドット + `N fail` が既に伝えている）
pub fn material_state(state: CommandState) -> &'static str {
    match state {
        CommandState::Unknown => "unknown",
        CommandState::Running => "running",
        CommandState::Idle | CommandState::Failed(_) => "idle",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// #722: Windows で AI 命名が一度も走らなかったのは、この解決がログインシェル経由の
    /// `command -v claude` 直呼びで、`SHELL` も `/bin/sh` も無い Windows では起動に失敗し、
    /// 静かに `None` へ化けていたため。探索を B16 へ委ねたので、**この関数自身に
    /// OS 分岐が無い**ことを型で示す（探索結果を注入して両プラットフォームぶん検査する）
    #[test]
    fn claudeの解決は探索結果をそのまま使い実在するものだけ返す() {
        let found = |p: &'static str| move || Some(p.to_string());
        let exists = |want: &'static str| move |p: &Path| p == Path::new(want);

        // Windows のシム（.cmd）でも .exe でも、B16 が返した値をそのまま採る
        let win = resolve_claude(
            false,
            None,
            &found("C:\\Users\\u\\.local\\bin\\claude.exe"),
            &exists("C:\\Users\\u\\.local\\bin\\claude.exe"),
        );
        assert_eq!(
            win.as_deref(),
            Some(Path::new("C:\\Users\\u\\.local\\bin\\claude.exe"))
        );
        let mac = resolve_claude(
            false,
            None,
            &found("/opt/homebrew/bin/claude"),
            &exists("/opt/homebrew/bin/claude"),
        );
        assert_eq!(mac.as_deref(), Some(Path::new("/opt/homebrew/bin/claude")));

        // 探索が空振り（claude 未導入）→ None。呼び出し側はヒューリスティックへ落ちる
        assert_eq!(resolve_claude(false, None, &|| None, &|_| true), None);
        // 見つかったのに実ファイルでない（シェル関数・壊れたシム）も採らない
        assert_eq!(
            resolve_claude(false, None, &found("/opt/homebrew/bin/claude"), &|_| false),
            None
        );
    }

    #[test]
    fn セルフテストと明示指定は探索より先に効く() {
        // セルフテスト中は実 LLM を呼ばない。探索すら走らせない
        let looked_up = std::cell::Cell::new(false);
        let plan = resolve_claude(
            true,
            Some(OsString::from("C:\\bin\\claude.exe")),
            &|| {
                looked_up.set(true);
                None
            },
            &|_| true,
        );
        assert_eq!(plan, None);
        assert!(!looked_up.get(), "セルフテスト中に探索を走らせない");

        // 明示指定があれば探索しない
        let explicit = resolve_claude(
            false,
            Some(OsString::from("/tmp/fake-claude")),
            &|| panic!("明示指定があるのに探索した"),
            &|_| true,
        );
        assert_eq!(explicit.as_deref(), Some(Path::new("/tmp/fake-claude")));

        // 明示指定が実在しないときは探索へ落とさない
        // （差し替えたつもりで本物の claude が動く事故を防ぐ）
        let missing = resolve_claude(
            false,
            Some(OsString::from("/tmp/does-not-exist")),
            &|| panic!("明示指定があるのに探索した"),
            &|_| false,
        );
        assert_eq!(missing, None);
    }

    /// 偽 claude を書く（#758）。実 CLI は起こさない（`.agent/conventions.md`
    /// 「テストの書き先は本番の外」）。`mode`:
    ///
    /// - `"reject-strict"`: **古い CLI 相当**。`--strict-mcp-config` を渡されたら
    ///   使い方エラー（非ゼロ終了）。渡されなければ命名 JSON を返す
    /// - `"accept"`: どちらでも命名 JSON を返す
    /// - `"always-fail"`: 何を渡しても非ゼロ終了（リミット・認証切れ相当）
    ///
    /// 返すのは (偽 claude のパス, 渡された引数のログ)。呼ばれるたびに 1 行増える。
    /// Windows は**バッチ経由の起動を実機で確かめられない**ので、この形のテストは
    /// unix だけに置く（学習の規則そのものは下の `run_learning_strict` の
    /// テストが両プラットフォームで拘束する）
    #[cfg(unix)]
    fn write_fake_claude(dir: &Path, mode: &str) -> (PathBuf, PathBuf) {
        let bin = dir.join("claude");
        let log = dir.join("args.log");
        let script = r#"#!/bin/sh
echo "$@" >> 'ARGS_LOG'
cat >/dev/null
for a in "$@"; do
  if [ "$a" = "--strict-mcp-config" ]; then
    STRICT_BRANCH
  fi
done
TAIL
"#
        .replace("ARGS_LOG", &log.display().to_string())
        .replace(
            "STRICT_BRANCH",
            if mode == "reject-strict" {
                "exit 2"
            } else {
                ":"
            },
        )
        .replace(
            "TAIL",
            if mode == "always-fail" {
                "exit 3"
            } else {
                r#"printf '%s\n' '{"tab":"偽命名","panes":{"3":"偽ペイン"}}'"#
            },
        );
        std::fs::write(&bin, script).expect("偽 claude を書ける");
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755))
            .expect("偽 claude に実行権を付けられる");
        (bin, log)
    }

    /// テスト用の一時ディレクトリ（本番の data dir・ホームには触らない）
    #[cfg(unix)]
    fn temp_dir_for(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "tako-758-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or_default()
        ));
        std::fs::create_dir_all(&dir).expect("一時ディレクトリを作れる");
        dir
    }

    #[cfg(unix)]
    fn arg_lines(log: &Path) -> Vec<String> {
        std::fs::read_to_string(log)
            .unwrap_or_default()
            .lines()
            .map(str::to_string)
            .collect()
    }

    /// 受け入れ 2（#758）: **フラグを拒否する古い CLI でも自動命名が黙って無効化されない**。
    /// フラグ無しで 1 回だけ再試行して成功し、2 回目以降は再試行が走らない
    #[test]
    #[cfg(unix)]
    fn 古いclaudeがフラグを拒否してもフォールバックで命名できる() {
        let dir = temp_dir_for("reject");
        let (bin, log) = write_fake_claude(&dir, "reject-strict");
        let state = AtomicU8::new(StrictMcp::Unknown as u8);

        let plan = run_claude_with(&state, false, &bin, &materials(), Lang::Ja)
            .expect("フォールバックで命名できる（黙って無効化されない）");
        assert_eq!(plan.tab.as_deref(), Some("偽命名"));
        assert_eq!(plan.panes, vec![(3, "偽ペイン".to_string())]);
        let calls = arg_lines(&log);
        assert_eq!(
            calls.len(),
            2,
            "フラグ付き → フラグ無しの 2 回のはず: {calls:?}"
        );
        assert!(
            calls[0].contains(STRICT_MCP_FLAG),
            "1 回目は付ける: {calls:?}"
        );
        assert!(
            !calls[1].contains(STRICT_MCP_FLAG),
            "再試行は外す: {calls:?}"
        );
        assert_eq!(StrictMcp::load(&state), StrictMcp::Unsupported);

        // 2 回目以降はフラグ無しの 1 回だけ（余計な再試行は初回だけ）
        let plan = run_claude_with(&state, false, &bin, &materials(), Lang::Ja);
        assert!(plan.is_some());
        let calls = arg_lines(&log);
        assert_eq!(calls.len(), 3, "2 回目に再試行が走っている: {calls:?}");
        assert!(
            !calls[2].contains(STRICT_MCP_FLAG),
            "学習後に付け直さない: {calls:?}"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 受け入れ 2（#758）: フラグを受け付ける CLI では 2 回目以降もフラグ付きのままで、
    /// 再試行は 1 度も走らない
    #[test]
    #[cfg(unix)]
    fn フラグが通るclaudeでは再試行せず付けたままになる() {
        let dir = temp_dir_for("accept");
        let (bin, log) = write_fake_claude(&dir, "accept");
        let state = AtomicU8::new(StrictMcp::Unknown as u8);

        for _ in 0..3 {
            assert!(run_claude_with(&state, false, &bin, &materials(), Lang::Ja).is_some());
        }
        let calls = arg_lines(&log);
        assert_eq!(calls.len(), 3, "1 回の命名につき 1 回の起動: {calls:?}");
        assert!(
            calls.iter().all(|c| c.contains(STRICT_MCP_FLAG)),
            "全てフラグ付きのはず: {calls:?}"
        );
        assert_eq!(StrictMcp::load(&state), StrictMcp::Supported);

        // A/B（`TAKO_758_LEGACY=1` 相当）: 旧経路はフラグを一切付けない
        let (legacy_bin, legacy_log) = write_fake_claude(&dir, "accept");
        assert!(run_claude_with(&state, true, &legacy_bin, &materials(), Lang::Ja).is_some());
        let legacy_calls = arg_lines(&legacy_log);
        assert_eq!(
            legacy_calls.len(),
            4,
            "同じログへ 4 行目が付く: {legacy_calls:?}"
        );
        assert!(
            !legacy_calls[3].contains(STRICT_MCP_FLAG),
            "legacy でフラグを付けている: {legacy_calls:?}"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// エッジ（#758）: claude が何をしても失敗する（リミット・認証切れ相当）なら
    /// 従来どおり `None` = ヒューリスティックへ落ちる。学習もしないので次回また速い側から試す
    #[test]
    #[cfg(unix)]
    fn どちらでも失敗するなら従来どおりヒューリスティックへ落ちる() {
        let dir = temp_dir_for("fail");
        let (bin, log) = write_fake_claude(&dir, "always-fail");
        let state = AtomicU8::new(StrictMcp::Unknown as u8);

        assert_eq!(
            run_claude_with(&state, false, &bin, &materials(), Lang::Ja),
            None
        );
        assert_eq!(
            StrictMcp::load(&state),
            StrictMcp::Unknown,
            "失敗で学習しない"
        );
        let calls = arg_lines(&log);
        assert_eq!(
            calls,
            vec![
                format!("-p --model {MODEL} {STRICT_MCP_FLAG}"),
                format!("-p --model {MODEL}"),
            ]
        );

        // claude が PATH に無い（実在しないパス）= 起動失敗も従来どおり None。
        // 引数の綴りとは無関係なので再試行もしない
        let missing = dir.join("does-not-exist");
        assert_eq!(
            run_claude_with(&state, false, &missing, &materials(), Lang::Ja),
            None
        );
        assert_eq!(arg_lines(&log).len(), 2, "起動失敗で再試行している");

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// エッジ（#758）: **上限に当たった起動は [`ClaudeRun::Failed`]**（再試行しない側）。
    /// ここを `NonZero` と取り違えると、上限まで待ったあとにもう一度上限まで待つ。
    /// 実際に子を起こして上限を当て、打ち切りと後始末まで見る
    #[test]
    #[cfg(unix)]
    fn 上限に当たった起動は再試行しない側になる() {
        let dir = temp_dir_for("timeout");
        let bin = dir.join("claude");
        let log = dir.join("args.log");
        std::fs::write(
            &bin,
            format!(
                "#!/bin/sh\necho \"$@\" >> '{}'\ncat >/dev/null\nsleep 30\n",
                log.display()
            ),
        )
        .expect("眠る偽 claude を書ける");
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755))
                .expect("実行権を付けられる");
        }

        let started = Instant::now();
        let run = spawn_claude(&bin, "prompt", true, Duration::from_millis(600));
        let waited = started.elapsed();
        assert_eq!(run, ClaudeRun::Failed, "打ち切りが非ゼロ終了に化けている");
        assert!(
            waited < Duration::from_secs(10),
            "上限で打ち切っていない（{waited:?} 待った）"
        );
        assert_eq!(arg_lines(&log).len(), 1, "打ち切りなのに起動が 2 回ある");

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// `run_learning_strict` を偽の起動結果で回し、(結果, 渡した strict の並び) を返す
    fn drive(
        state: &AtomicU8,
        legacy: bool,
        replies: Vec<ClaudeRun>,
    ) -> (Option<String>, Vec<bool>) {
        let replies = std::cell::RefCell::new(std::collections::VecDeque::from(replies));
        let calls = std::cell::RefCell::new(Vec::new());
        let out = run_learning_strict(state, legacy, &mut |strict| {
            calls.borrow_mut().push(strict);
            replies
                .borrow_mut()
                .pop_front()
                .expect("起動回数が想定より多い")
        });
        (out, calls.into_inner())
    }

    /// 学習の規則（#758）を両プラットフォームで拘束する。
    /// **非ゼロ終了だけ**が再試行の引き金で、再試行が通ったときにだけ学習する
    #[test]
    fn フラグの学習は非ゼロ終了のときだけ再試行する() {
        let unknown = || AtomicU8::new(StrictMcp::Unknown as u8);

        // 古い CLI: フラグ付き → 非ゼロ、フラグ無し → 通る。以後フラグを付けない
        let state = unknown();
        let (out, calls) = drive(
            &state,
            false,
            vec![ClaudeRun::NonZero, ClaudeRun::Ok("plain".into())],
        );
        assert_eq!(out.as_deref(), Some("plain"));
        assert_eq!(calls, vec![true, false]);
        assert_eq!(StrictMcp::load(&state), StrictMcp::Unsupported);
        let (_, calls) = drive(&state, false, vec![ClaudeRun::Ok("plain".into())]);
        assert_eq!(calls, vec![false], "学習後に再試行が走っている");

        // 通る CLI: 1 回で決まり、以後も付けたまま。学習後の非ゼロ終了では再試行しない
        let state = unknown();
        let (out, calls) = drive(&state, false, vec![ClaudeRun::Ok("strict".into())]);
        assert_eq!(out.as_deref(), Some("strict"));
        assert_eq!(calls, vec![true]);
        assert_eq!(StrictMcp::load(&state), StrictMcp::Supported);
        let (out, calls) = drive(&state, false, vec![ClaudeRun::NonZero]);
        assert_eq!(out, None);
        assert_eq!(calls, vec![true], "学習済みなのに再試行した");
        assert_eq!(StrictMcp::load(&state), StrictMcp::Supported);

        // 打ち切り・起動失敗はフラグと無関係なので再試行しない（上限のぶん二重に待たせない）
        let state = unknown();
        let (out, calls) = drive(&state, false, vec![ClaudeRun::Failed]);
        assert_eq!(out, None);
        assert_eq!(calls, vec![true]);
        assert_eq!(
            StrictMcp::load(&state),
            StrictMcp::Unknown,
            "打ち切りで速い経路を諦めている"
        );

        // フラグ無しでも失敗 = フラグのせいだと決められない。学習せず次回もフラグから試す
        let state = unknown();
        let (out, calls) = drive(&state, false, vec![ClaudeRun::NonZero, ClaudeRun::NonZero]);
        assert_eq!(out, None);
        assert_eq!(calls, vec![true, false]);
        assert_eq!(StrictMcp::load(&state), StrictMcp::Unknown);
        let (_, calls) = drive(
            &state,
            false,
            vec![ClaudeRun::NonZero, ClaudeRun::Ok("plain".into())],
        );
        assert_eq!(
            calls,
            vec![true, false],
            "一時的な失敗で速い経路を捨てている"
        );

        // A/B: legacy はフラグを一切付けず、学習もしない
        let state = unknown();
        let (out, calls) = drive(&state, true, vec![ClaudeRun::Ok("legacy".into())]);
        assert_eq!(out.as_deref(), Some("legacy"));
        assert_eq!(calls, vec![false]);
        assert_eq!(StrictMcp::load(&state), StrictMcp::Unknown);
    }

    /// #722 の受け入れ 2: **実 claude を呼ぶ** e2e。AI 経路が実際に走り、
    /// ヒューリスティックとは違う名前が返ることを確かめる。
    /// 認証と課金が要るので既定では走らせない:
    ///
    /// ```text
    /// cargo test -p tako-app --bin tako-app -- --ignored --nocapture ai命名
    /// ```
    #[test]
    #[ignore = "実 claude CLI を呼ぶ（認証必須・課金あり）"]
    fn ai命名が実claudeで走りヒューリスティックと異なる名前になる() {
        let bin = claude_bin().expect("claude を解決できない（#722 の回帰）");
        println!("解決した claude: {}", bin.display());
        let m = materials();
        let ai = generate(&m);
        let heuristic = heuristic_plan(&m);
        println!("AI 経路        = {ai:?}");
        println!("ヒューリスティック = {heuristic:?}");
        assert!(
            ai.tab.is_some() || !ai.panes.is_empty(),
            "名前が 1 つも返っていない"
        );
        assert_ne!(
            ai, heuristic,
            "AI 経路が走らずヒューリスティックへ落ちている（#722 の回帰）"
        );
    }

    fn materials() -> TabMaterials {
        TabMaterials {
            tab: 1,
            rename_tab: true,
            panes: vec![
                PaneMaterials {
                    pane: 3,
                    role: None,
                    osc_title: Some("tako — cargo test".into()),
                    cwd: Some("/Users/x/Documents/tako".into()),
                    state: "running",
                    tail: vec!["running 36 tests".into()],
                },
                PaneMaterials {
                    pane: 5,
                    role: Some("dev-server".into()),
                    osc_title: None,
                    cwd: Some("/Users/x/web/app".into()),
                    state: "idle",
                    tail: Vec::new(),
                },
            ],
        }
    }

    #[test]
    fn 応答のパースは素材外のidを捨て上限へ丸める() {
        let m = materials();
        let plan = parse_plan(
            "前置きの説明\n{\"tab\":\"tako テスト\",\"panes\":{\"3\":\"cargo test\",\"99\":\"無関係\",\"5\":\"\"}}\n後置き",
            &m,
            Lang::Ja,
        )
        .unwrap();
        assert_eq!(plan.tab.as_deref(), Some("tako テスト"));
        assert_eq!(plan.panes, vec![(3, "cargo test".into())]);
        // 上限超えは切り詰め
        let long = format!("{{\"tab\":\"{}\"}}", "あ".repeat(40));
        let plan = parse_plan(&long, &m, Lang::Ja).unwrap();
        assert_eq!(plan.tab.as_deref().map(|t| t.chars().count()), Some(16));
        // JSON が無い・空の応答は None
        assert_eq!(parse_plan("名前は付けられません", &m, Lang::Ja), None);
        assert_eq!(parse_plan("{\"panes\":{}}", &m, Lang::Ja), None);
    }

    #[test]
    fn タブが手動リネーム済みならタブ名は採用しない() {
        let mut m = materials();
        m.rename_tab = false;
        let plan = parse_plan(
            "{\"tab\":\"勝手な名前\",\"panes\":{\"3\":\"x\"}}",
            &m,
            Lang::Ja,
        )
        .unwrap();
        assert_eq!(plan.tab, None);
        let plan = heuristic_plan(&m);
        assert_eq!(plan.tab, None);
    }

    #[test]
    fn ヒューリスティックはoscタイトル優先でcwdへ落ちる() {
        let plan = heuristic_plan(&materials());
        assert_eq!(
            plan.panes,
            vec![
                (3, "tako — cargo test".into()),
                (5, "app".into()), // cwd の末尾ディレクトリ名
            ]
        );
        assert_eq!(plan.tab.as_deref(), Some("tako — cargo tes")); // タブ上限 16 文字
    }

    #[test]
    fn プロンプトは素材と形式指定を含む() {
        let prompt = build_prompt(&materials(), Lang::Ja);
        assert!(prompt.contains("cargo test"));
        assert!(prompt.contains("\"pane\":3") || prompt.contains("\"pane\": 3"));
        assert!(prompt.contains("JSON"));
        // タブ名不要の指定が伝わる
        let mut m = materials();
        m.rename_tab = false;
        assert!(build_prompt(&m, Lang::Ja).contains("タブ名は不要"));
    }

    /// #552 案 3: 生成言語は UI 言語に固定する（プロンプト側の指定）
    #[test]
    fn プロンプトは出力言語と失敗の扱いをui言語で指定する() {
        let ja = build_prompt(&materials(), Lang::Ja);
        assert!(ja.contains("日本語"), "{ja}");
        assert!(ja.contains("簡体字"), "簡体字の禁止を明示する: {ja}");
        assert!(ja.contains("command not found"), "一時的失敗の除外: {ja}");

        let en = build_prompt(&materials(), Lang::En);
        assert!(en.contains("English only"), "{en}");
        assert!(en.contains("never use CJK"), "{en}");
        assert!(en.contains("command not found"), "{en}");
        // 英語 UI のプロンプトに日本語が混ざっていない（素材の JSON 部分を除く）
        let instructions = en.split("{\"panes\"").next().unwrap_or(&en);
        assert!(
            !instructions.chars().any(|c| matches!(c,
                '\u{3040}'..='\u{30FF}' | '\u{4E00}'..='\u{9FFF}')),
            "英語 UI の指示文に日本語が残っている: {instructions}"
        );
    }

    /// #552 案 3: 日本語 UI に簡体字が混ざったら、置換できるものは置換し、
    /// できないものは名前ごと捨てる（実観測は `开発`）
    #[test]
    fn 日本語uiでは簡体字を日本語字体へ寄せ残れば名前を捨てる() {
        // 置換で救えるケース（Issue で実観測された `开発`）
        assert_eq!(sanitize_title("开発", Lang::Ja).as_deref(), Some("開発"));
        assert_eq!(
            sanitize_title("环境検証", Lang::Ja).as_deref(),
            Some("環境検証")
        );
        // 置換表に無い中国語専用字が残る名前は採用しない
        assert_eq!(sanitize_title("这个任务", Lang::Ja), None);
        assert_eq!(sanitize_title("한글 작업", Lang::Ja), None);
        // 通常の日本語・英数・記号は素通し
        assert_eq!(
            sanitize_title(" cargo test 実行 ", Lang::Ja).as_deref(),
            Some("cargo test 実行")
        );
        assert_eq!(
            sanitize_title("tako — ビルド", Lang::Ja).as_deref(),
            Some("tako — ビルド")
        );
        // 英語 UI では CJK を含む名前を採用しない
        assert_eq!(sanitize_title("ビルド", Lang::En), None);
        assert_eq!(sanitize_title("开発", Lang::En), None);
        assert_eq!(
            sanitize_title("cargo build", Lang::En).as_deref(),
            Some("cargo build")
        );
        // 空・空白のみは None
        assert_eq!(sanitize_title("   ", Lang::Ja), None);
    }

    /// #552 案 3: 応答パースの段階で字種検査が効き、全滅なら None
    /// （呼び出し側がヒューリスティック命名へ落ちる）
    #[test]
    fn 字種検査に落ちたタイトルは採用されない() {
        let m = materials();
        let plan = parse_plan(
            "{\"tab\":\"开発环境\",\"panes\":{\"3\":\"这个\",\"5\":\"サーバ起動\"}}",
            &m,
            Lang::Ja,
        )
        .unwrap();
        assert_eq!(plan.tab.as_deref(), Some("開発環境"), "置換で救える");
        assert_eq!(
            plan.panes,
            vec![(5, "サーバ起動".into())],
            "救えない `这个` は落ちる"
        );
        // 全部落ちれば None（= ヒューリスティックへフォールバック）
        assert_eq!(
            parse_plan(
                "{\"tab\":\"这个\",\"panes\":{\"3\":\"删除缓存\"}}",
                &m,
                Lang::Ja
            ),
            None
        );
    }

    /// 置換表は 2 本の文字列の**位置対応**で成り立っているので、長さのズレ・
    /// 重複・自己対応（置換になっていない）を機械検査する
    #[test]
    fn 簡体字の置換表は左右が同じ長さで対応が正しい() {
        let simp: Vec<char> = SIMPLIFIED.chars().collect();
        let jp: Vec<char> = JAPANESE.chars().collect();
        assert_eq!(simp.len(), jp.len(), "左右の長さが違うと対応がずれる");
        let mut seen = std::collections::HashSet::new();
        for (i, (s, j)) in simp.iter().zip(jp.iter()).enumerate() {
            assert!(seen.insert(*s), "{s} が重複している（{i} 文字目）");
            assert_ne!(s, j, "{s} は置換になっていない");
            assert!(
                !jis_kanji::CP932_KANJI.contains(*s),
                "{s} は日本語の漢字集合にあるので置換対象にしない"
            );
            assert!(
                jis_kanji::CP932_KANJI.contains(*j),
                "{j} は日本語の漢字集合に無い（置換先として不適切）"
            );
        }
        // 実際に引ける
        assert_eq!(localize_han('开'), '開');
        assert_eq!(localize_han('あ'), 'あ', "対象外はそのまま");
    }

    /// 検査の**既知の限界**を仕様として固定する（#552）。日本語の漢字集合にも
    /// 存在する字だけで書かれた中国語（`那个` = すべて JIS X 0208 内）は
    /// 素通しする。狙いは「日本語の名前に簡体字が混ざる滑り」の遮断であって、
    /// 中国語判定ではない
    #[test]
    fn 字種検査は日本語漢字だけで書かれた中国語までは弾かない() {
        assert_eq!(sanitize_title("那个", Lang::Ja).as_deref(), Some("那个"));
        // 一方、簡体字専用の字が 1 つでもあれば落ちる
        assert_eq!(sanitize_title("这个", Lang::Ja), None);
    }

    fn signal(tab: u64, fingerprint: u64, named: bool) -> TabSignal {
        TabSignal {
            tab,
            fingerprint,
            named,
        }
    }

    #[test]
    fn tickは静穏と未処理と冷却を満たしたタブだけ発火する() {
        let mut renamer = AutoRenamer::new(true);
        let t0 = Instant::now();
        // 初回観測 → まだ発火しない
        assert!(renamer.tick(&[signal(1, 100, false)], t0).is_empty());
        // 静穏時間経過 → 発火
        assert_eq!(
            renamer.tick(&[signal(1, 100, false)], t0 + DEBOUNCE),
            vec![1]
        );
        // 同じ指紋には再発火しない
        assert!(renamer
            .tick(&[signal(1, 100, false)], t0 + DEBOUNCE * 2)
            .is_empty());
        // 指紋が変わると起点リセット → 静穏 + 冷却後に再発火（まだ名無し = 30 秒）
        let t1 = t0 + DEBOUNCE * 2;
        assert!(renamer.tick(&[signal(1, 200, false)], t1).is_empty());
        assert!(
            renamer
                .tick(&[signal(1, 200, false)], t1 + DEBOUNCE)
                .is_empty(),
            "クールダウン中は発火しない"
        );
        assert_eq!(
            renamer.tick(
                &[signal(1, 200, false)],
                t0 + FIRST_NAME_COOLDOWN + DEBOUNCE
            ),
            vec![1]
        );
        // 無効化中は何もしない
        renamer.enabled = false;
        assert!(renamer
            .tick(
                &[signal(1, 300, false)],
                t0 + FIRST_NAME_COOLDOWN * 2 + DEBOUNCE * 2
            )
            .is_empty());
    }

    /// **#552 案 1**: 命名済みタブの再命名は 5 分に 1 回まで。
    /// 名無しのタブは初回を待たされない（体験を壊さないための例外）
    #[test]
    fn 命名済みタブの再命名は5分に1回へ制限される() {
        let mut renamer = AutoRenamer::new(true);
        let t0 = Instant::now();
        // 初回: 名無しなので静穏だけで発火する（5 分待たない）
        assert!(renamer.tick(&[signal(1, 100, false)], t0).is_empty());
        assert_eq!(
            renamer.tick(&[signal(1, 100, false)], t0 + DEBOUNCE),
            vec![1],
            "名無しのタブは最初の命名を待たされない"
        );

        // 以後は命名済み。素材が変わっても 5 分たつまで発火しない
        let mut t = t0 + DEBOUNCE;
        for (i, step) in [30, 60, 120, 240].iter().enumerate() {
            let now = t0 + Duration::from_secs(*step);
            let fp = 200 + i as u64;
            assert!(renamer.tick(&[signal(1, fp, true)], now).is_empty());
            t = now + DEBOUNCE;
            assert!(
                renamer.tick(&[signal(1, fp, true)], t).is_empty(),
                "{step} 秒後（最小間隔 5 分の内側）に再命名が走った"
            );
        }
        // 直近の発火から 5 分経過 → 再命名が通る
        let after = t0 + DEBOUNCE + RENAME_MIN_INTERVAL;
        assert!(after > t);
        assert!(renamer.tick(&[signal(1, 900, true)], after).is_empty());
        assert_eq!(
            renamer.tick(&[signal(1, 900, true)], after + DEBOUNCE),
            vec![1],
            "5 分経てば再命名できる"
        );
    }

    #[test]
    fn 閉じたタブの監視は捨てられる() {
        let mut renamer = AutoRenamer::new(true);
        let t0 = Instant::now();
        renamer.tick(&[signal(1, 100, false), signal(2, 200, false)], t0);
        renamer.tick(&[signal(2, 200, false)], t0 + Duration::from_secs(1));
        assert!(!renamer.watches.contains_key(&1));
        assert!(renamer.watches.contains_key(&2));
    }

    #[test]
    fn 末尾整形は空行を落とし行数と長さを絞る() {
        let lines: Vec<String> = (0..10)
            .map(|i| {
                if i % 2 == 0 {
                    format!("line-{i}-{}", "x".repeat(200))
                } else {
                    "   ".into()
                }
            })
            .collect();
        let tail = trim_tail(lines);
        assert_eq!(tail.len(), 5); // 空行 5 本を除いた残り
        assert!(tail.iter().all(|l| l.chars().count() <= TAIL_CHARS));
    }

    /// **#552 案 2**: 一時的な失敗は命名の材料にしない。
    /// 作業の結果としての失敗（テストの red 等）は材料に残す
    #[test]
    fn 一時的な失敗の行は素材から落ちる() {
        let dropped = [
            "zsh: command not found: claudee",
            "bash: cargoo: command not found",
            "cat: foo.txt: No such file or directory",
            "-bash: ./run.sh: Permission denied",
            "fatal: not a git repository (or any of the parent directories): .git",
            "error: unrecognized option '--fooo'",
            "usage: git [-v | --version] [-h | --help]",
            "zsh: no matches found: *.rss",
            "zsh: command not found: gti",
            "ls: そのようなファイルやディレクトリはありません",
        ];
        for line in dropped {
            assert!(is_transient_failure(line), "落とすべき行: {line}");
        }
        let kept = [
            "test result: FAILED. 3 passed; 2 failed",
            "error[E0308]: mismatched types",
            "warning: unused variable `x`",
            "Compiling tako-app v0.5.11",
            "1 test failed in src/lib.rs",
        ];
        for line in kept {
            assert!(!is_transient_failure(line), "残すべき行: {line}");
        }
        // trim_tail が実際に落とす
        let tail = trim_tail(vec![
            "cargo test".into(),
            "zsh: command not found: cargoo".into(),
            "test result: ok. 42 passed".into(),
        ]);
        assert_eq!(tail, vec!["cargo test", "test result: ok. 42 passed"]);
    }

    /// **#552 案 2**: 打ち間違い 1 回（Idle → Failed）では素材指紋が変わらないので
    /// リネームが発火しない。実行中（Running）は従来どおり区別する
    #[test]
    fn 失敗状態は素材上idleと同一視される() {
        assert_eq!(material_state(CommandState::Failed(127)), "idle");
        assert_eq!(material_state(CommandState::Idle), "idle");
        assert_eq!(material_state(CommandState::Running), "running");
        assert_eq!(material_state(CommandState::Unknown), "unknown");
        // 指紋（= 発火トリガー）が失敗の前後で同じであること
        let before = fingerprint(&material_state(CommandState::Idle));
        let after = fingerprint(&material_state(CommandState::Failed(1)));
        assert_eq!(
            before, after,
            "打ち間違いだけでリネームが発火してはならない"
        );
    }
}
