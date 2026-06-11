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

use std::collections::hash_map::DefaultHasher;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

/// 検知ループのポーリング間隔
pub const POLL_INTERVAL: Duration = Duration::from_secs(2);
/// 素材が変化しなくなってからリネームを発火するまでの静穏時間（デバウンス）
const DEBOUNCE: Duration = Duration::from_secs(4);
/// 同じタブへの連続発火を抑える最小間隔（claude 呼び出しの浪費防止）
const COOLDOWN: Duration = Duration::from_secs(30);
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

/// タブごとの監視状態（指紋 + デバウンス + クールダウン）
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

    /// 1 tick 分の判定。`tabs` は (タブ ID, 素材指紋) のスナップショット。
    /// 戻り値は「静穏が確認でき、リネームを発火すべきタブ ID」
    pub fn tick(&mut self, tabs: &[(u64, u64)], now: Instant) -> Vec<u64> {
        // 閉じられたタブの監視を捨てる
        self.watches
            .retain(|id, _| tabs.iter().any(|(t, _)| t == id));
        if !self.enabled {
            return Vec::new();
        }
        let mut fire = Vec::new();
        for &(tab, fingerprint) in tabs {
            let watch = self.watches.entry(tab).or_insert(TabWatch {
                fingerprint,
                since: now,
                done_fingerprint: 0,
                last_run: None,
            });
            if watch.fingerprint != fingerprint {
                watch.fingerprint = fingerprint;
                watch.since = now;
                continue;
            }
            let calm = now.duration_since(watch.since) >= DEBOUNCE;
            let fresh = watch.done_fingerprint != fingerprint;
            let cooled = watch
                .last_run
                .is_none_or(|t| now.duration_since(t) >= COOLDOWN);
            if calm && fresh && cooled {
                // 失敗時の連打を防ぐため、結果を待たず発火済みとして記録する
                watch.done_fingerprint = fingerprint;
                watch.last_run = Some(now);
                fire.push(tab);
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
/// 無い・失敗した場合はヒューリスティック命名へフォールバックする（FR-2.12.5）
pub fn generate(materials: &TabMaterials) -> RenamePlan {
    if let Some(bin) = claude_bin() {
        if let Some(plan) = run_claude(bin, materials) {
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
    let output = std::process::Command::new(shell)
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
fn run_claude(bin: &Path, materials: &TabMaterials) -> Option<RenamePlan> {
    use std::io::{Read, Write};
    use std::process::{Command, Stdio};

    let prompt = build_prompt(materials);
    let mut child = Command::new(bin)
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
    parse_plan(&output, materials)
}

/// プロンプト 1 本（FR-2.12.2。判断・調整はすべてこの文面に閉じる）
fn build_prompt(materials: &TabMaterials) -> String {
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
    let tab_line = if materials.rename_tab {
        "タブ全体（tab）と各ペイン"
    } else {
        "各ペイン（タブ名は不要）"
    };
    format!(
        "あなたはターミナルのタブ・ペインに短い名前を付ける係。\
         以下の JSON は 1 つのタブ内の各ペインの状況（作業ディレクトリ cwd、実行状態 state、\
         OSC タイトル osc_title、画面末尾の出力 tail）。\n\
         {tab_line}に、いま何をしているかがひと目で分かる短い日本語名を付けること\
         （タブは {MAX_TAB_TITLE} 文字以内、ペインは {MAX_PANE_TITLE} 文字以内。\
         コマンド名・プロジェクト名・ツール名は原文のまま使ってよい）。\n\
         出力は次の形式の JSON だけ。説明文・コードフェンスは書かない:\n\
         {{\"tab\":\"...\",\"panes\":{{\"<pane id>\":\"...\"}}}}\n\
         名前を変える必要がないペインは省略してよい。\n\n{}",
        serde_json::json!({ "tab": materials.tab, "panes": panes })
    )
}

/// claude の応答から JSON を取り出して RenamePlan へ写す。
/// 素材に無いペイン ID は無視し、タイトルは上限へ丸める
fn parse_plan(output: &str, materials: &TabMaterials) -> Option<RenamePlan> {
    let start = output.find('{')?;
    let end = output.rfind('}')?;
    let value: serde_json::Value = serde_json::from_str(output.get(start..=end)?).ok()?;
    let tab = value["tab"]
        .as_str()
        .map(str::trim)
        .filter(|t| !t.is_empty() && materials.rename_tab)
        .map(|t| clamp_chars(t, MAX_TAB_TITLE));
    let mut panes = Vec::new();
    if let Some(map) = value["panes"].as_object() {
        for (key, title) in map {
            let Ok(id) = key.parse::<u64>() else { continue };
            if !materials.panes.iter().any(|p| p.pane == id) {
                continue;
            }
            if let Some(title) = title.as_str().map(str::trim).filter(|t| !t.is_empty()) {
                panes.push((id, clamp_chars(title, MAX_PANE_TITLE)));
            }
        }
    }
    if tab.is_none() && panes.is_empty() {
        return None;
    }
    Some(RenamePlan { tab, panes })
}

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

/// 素材用に画面末尾の行を整える（空行を落とし、長い行を切り詰める）
pub fn trim_tail(lines: Vec<String>) -> Vec<String> {
    let mut tail: Vec<String> = lines
        .into_iter()
        .filter(|l| !l.trim().is_empty())
        .map(|l| clamp_chars(&l, TAIL_CHARS))
        .collect();
    if tail.len() > TAIL_LINES {
        tail.drain(..tail.len() - TAIL_LINES);
    }
    tail
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
        )
        .unwrap();
        assert_eq!(plan.tab.as_deref(), Some("tako テスト"));
        assert_eq!(plan.panes, vec![(3, "cargo test".into())]);
        // 上限超えは切り詰め
        let long = format!("{{\"tab\":\"{}\"}}", "あ".repeat(40));
        let plan = parse_plan(&long, &m).unwrap();
        assert_eq!(plan.tab.as_deref().map(|t| t.chars().count()), Some(16));
        // JSON が無い・空の応答は None
        assert_eq!(parse_plan("名前は付けられません", &m), None);
        assert_eq!(parse_plan("{\"panes\":{}}", &m), None);
    }

    #[test]
    fn タブが手動リネーム済みならタブ名は採用しない() {
        let mut m = materials();
        m.rename_tab = false;
        let plan = parse_plan("{\"tab\":\"勝手な名前\",\"panes\":{\"3\":\"x\"}}", &m).unwrap();
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
        let prompt = build_prompt(&materials());
        assert!(prompt.contains("cargo test"));
        assert!(prompt.contains("\"pane\":3") || prompt.contains("\"pane\": 3"));
        assert!(prompt.contains("JSON"));
        // タブ名不要の指定が伝わる
        let mut m = materials();
        m.rename_tab = false;
        assert!(build_prompt(&m).contains("タブ名は不要"));
    }

    #[test]
    fn tickは静穏と未処理と冷却を満たしたタブだけ発火する() {
        let mut renamer = AutoRenamer::new(true);
        let t0 = Instant::now();
        // 初回観測 → まだ発火しない
        assert!(renamer.tick(&[(1, 100)], t0).is_empty());
        // 静穏時間経過 → 発火
        assert_eq!(renamer.tick(&[(1, 100)], t0 + DEBOUNCE), vec![1]);
        // 同じ指紋には再発火しない
        assert!(renamer.tick(&[(1, 100)], t0 + DEBOUNCE * 2).is_empty());
        // 指紋が変わると起点リセット → 静穏 + 冷却後に再発火
        let t1 = t0 + DEBOUNCE * 2;
        assert!(renamer.tick(&[(1, 200)], t1).is_empty());
        assert!(
            renamer.tick(&[(1, 200)], t1 + DEBOUNCE).is_empty(),
            "クールダウン中は発火しない"
        );
        assert_eq!(renamer.tick(&[(1, 200)], t0 + COOLDOWN + DEBOUNCE), vec![1]);
        // 無効化中は何もしない
        renamer.enabled = false;
        assert!(renamer
            .tick(&[(1, 300)], t0 + COOLDOWN * 2 + DEBOUNCE * 2)
            .is_empty());
    }

    #[test]
    fn 閉じたタブの監視は捨てられる() {
        let mut renamer = AutoRenamer::new(true);
        let t0 = Instant::now();
        renamer.tick(&[(1, 100), (2, 200)], t0);
        renamer.tick(&[(2, 200)], t0 + Duration::from_secs(1));
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
}
