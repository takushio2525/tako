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
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
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
/// claude 子プロセスの待ち時間上限（超過は kill してヒューリスティックへ）
const CLAUDE_TIMEOUT: Duration = Duration::from_secs(30);
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

/// 名前の生成。claude CLI があればプロンプト 1 本で生成し、
/// 無い・失敗した場合はヒューリスティック命名へフォールバックする（FR-2.12.5）。
/// 生成言語は UI 言語に固定する（#552 案 3）
pub fn generate(materials: &TabMaterials) -> RenamePlan {
    let lang = tako_core::i18n::lang();
    if let Some(bin) = claude_bin() {
        if let Some(plan) = run_claude(bin, materials, lang) {
            return plan;
        }
    }
    heuristic_plan(materials)
}

/// claude CLI の場所（プロセス内で 1 回だけ解決してキャッシュする）。
/// GUI アプリの PATH は最小構成のため、ログインシェル経由でユーザーの PATH を引く
pub fn claude_bin() -> Option<&'static Path> {
    static BIN: OnceLock<Option<PathBuf>> = OnceLock::new();
    BIN.get_or_init(detect_claude).as_deref()
}

fn detect_claude() -> Option<PathBuf> {
    // セルフテスト中は実 LLM を呼ばない（ヒューリスティック経路のみ機械検証する）
    if std::env::var_os("TAKO_SELF_TEST").is_some() {
        return None;
    }
    // 明示指定（検証・差し替え用）
    if let Some(path) = std::env::var_os("TAKO_CLAUDE_BIN") {
        let path = PathBuf::from(path);
        return path.is_file().then_some(path);
    }
    let shell = std::env::var("SHELL")
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "/bin/sh".into());
    // #628: GUI プロセスからの起動なのでコンソールウィンドウを出させない
    let output =
        tako_core::platform::process::no_console_window(&mut std::process::Command::new(shell))
            .args(["-l", "-c", "command -v claude"])
            .stdin(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .output()
            .ok()?;
    if !output.status.success() {
        return None;
    }
    let path = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let path = PathBuf::from(path);
    path.is_file().then_some(path)
}

/// claude -p を 1 回叩いて応答をパースする。失敗（起動不可・タイムアウト・パース不能）は
/// None（呼び出し側がヒューリスティックへ落とす）
fn run_claude(bin: &Path, materials: &TabMaterials, lang: Lang) -> Option<RenamePlan> {
    use std::io::{Read, Write};
    use std::process::{Command, Stdio};

    let prompt = build_prompt(materials, lang);
    // #586: GUI プロセスからの起動なので Windows でコンソールウィンドウを出させない
    let mut child = tako_core::platform::process::no_console_window(&mut Command::new(bin))
        .args(["-p", "--model", MODEL])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(prompt.as_bytes());
        // drop で stdin が閉じ、-p は EOF までをプロンプトとして読む
    }
    // stdout はパイプ詰まり防止のため別スレッドで吸い出しつつ、タイムアウト付きで待つ
    let mut stdout = child.stdout.take()?;
    let reader = std::thread::spawn(move || {
        let mut buf = String::new();
        let _ = stdout.read_to_string(&mut buf);
        buf
    });
    let deadline = Instant::now() + CLAUDE_TIMEOUT;
    let finished = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status.success(),
            Ok(None) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(200));
            }
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                break false;
            }
        }
    };
    let output = reader.join().unwrap_or_default();
    if !finished {
        return None;
    }
    parse_plan(&output, materials, lang)
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
