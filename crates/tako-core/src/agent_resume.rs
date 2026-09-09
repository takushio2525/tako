//! agent_resume — 再起動後の復元で「どの系統のどの会話へ戻すか」の規則（Issue #1238）
//!
//! #1076 は claude の復元を直したが、**会話ごと戻せるのは claude だけ**だった。
//! codex / agy のペインは「タブのタイトルだけ残って新規シェル」になり、
//! 復元の内訳ログでは `ID なし` として claude の検出漏れと混ざっていた。
//!
//! ここには 3 つの純粋な規則を置く（実際の解決・コマンド組み立て・保存は上の層）。
//!
//! 1. [`AgentResumeIds`] — ペイン → (系統, 会話 ID) の**保持規則**。
//!    #1076 の「確認してから外す」をそのまま一般化したもの
//! 2. [`resume_spec`] — 系統ごとの**復元コマンドの骨格**（プログラム名・
//!    サブコマンド・ID のフラグ）
//! 3. [`restore_support`] — その系統をこの環境で復元できるか
//!
//! # 会話 ID の所在（2026-09-09 の実測）
//!
//! | 系統 | ID の在り処 | 現れる時期 | 復帰コマンド |
//! |---|---|---|---|
//! | claude | `claude agents --json`（`transcript` が正本） | 起動後まもなく | `claude --resume <id>` |
//! | codex | `$CODEX_HOME/thread-writer-locks/<id>.lock` を**開いたまま持つ** | **起動直後** | `codex resume <id>` |
//! | agy | `~/.gemini/antigravity-cli/presence/<id>.lock` を**開いたまま持つ** | **最初のターンの後** | `agy --conversation <id>` |
//!
//! 実測は codex-cli 0.153.0 / agy 1.1.27。どちらもロックファイルは
//! **プロセス終了後も残る**ので、「ファイルが在る」ではなく
//! 「生きたプロセスが開いている」（`lsof`）を根拠にする。
//!
//! # なぜ Windows では成立しないか
//!
//! codex / agy の ID 解決は `lsof`（POSIX の道具）で「その pid が開いているファイル」を
//! 引く。Windows に `lsof` は無く、同等の手段（handle.exe 等）は同梱できないので、
//! **Windows では ID を保存できない** = 復元できない。これを `ID なし`（保存経路の
//! 取りこぼし）と混ぜないために [`restore_support`] が別の答えを返す。

use std::collections::HashMap;

use crate::agent_support::Agent;
use crate::platform::support::Platform;
use crate::PaneId;

/// 確認済みのペインの ID を落とすまでに必要な連続不在回数。
/// 1 回の取りこぼし（スキャンが失敗した等）で落とさないための猶予
pub const FORGET_AFTER_MISSES: u32 = 2;

/// 1 ペインぶんの保持状態
#[derive(Debug, Clone, PartialEq, Eq)]
struct Entry {
    agent: Agent,
    /// 会話 ID。**`None` = その系統が動いていることは分かるが ID を採れていない**
    /// （agy は最初のターンまで会話が生まれない / Windows には `lsof` が無い）。
    /// 復元の内訳ログが `ID なし` と `resume 非対応` を言い分けるための材料になる
    id: Option<String>,
    /// このプロセスの生存中に一度でもスキャンで検出できたか。
    /// **false（= `layout.json` 由来のまま）はスキャンの不在で落とさない**
    confirmed: bool,
    /// 連続で検出できなかった回数（検出できたら 0 に戻る）
    misses: u32,
}

/// ペインごとの復元用の会話 ID（系統つき）。
///
/// 規約は #1076 の「確認してから外す」と同じ。**不在は「終了した」の証拠にならない**
/// （起動途中・スキャンの取りこぼし・ダイアログで待っている状態はどれも検出に出ない）。
///
/// - [`seed`](Self::seed)（`layout.json` 由来）で入った ID は**未確認**として持ち、
///   スキャンの不在では**絶対に落とさない**
/// - スキャンで一度でも検出できたペインだけが、連続 [`FORGET_AFTER_MISSES`] 回の
///   不在で落ちる（ユーザーが終了してシェルへ戻したペインを勝手に resume しない）
/// - 生きているペインの一覧から消えたペイン（閉じた）は即座に落ちる
#[derive(Debug, Clone, Default)]
pub struct AgentResumeIds {
    entries: HashMap<PaneId, Entry>,
}

impl AgentResumeIds {
    pub fn new() -> Self {
        Self::default()
    }

    /// `layout.json` から復元した ID を**未確認**として入れる。
    /// すでに確認済みの記録があるペインは触らない（検出結果のほうが新しい）
    pub fn seed(&mut self, pane: PaneId, agent: Agent, id: Option<&str>) {
        if self.entries.get(&pane).is_some_and(|e| e.confirmed) {
            return;
        }
        self.entries.insert(
            pane,
            Entry {
                agent,
                id: id.map(str::to_string),
                confirmed: false,
                misses: 0,
            },
        );
    }

    /// スキャン 1 回ぶんの結果を反映する。
    ///
    /// - `panes`: いま生きているペインの一覧（ここに無いペインの記録は捨てる）
    /// - `detected`: そのスキャンで会話を検出できたペインとその (系統, ID)
    ///
    /// **スキャンが失敗した回は呼ばない**（呼ぶと全ペインが不在扱いになる）。
    /// 「検出できたペインが 0 件」は呼んでよい（それが不在の 1 回になる）
    pub fn apply_scan(
        &mut self,
        panes: &[PaneId],
        detected: &HashMap<PaneId, (Agent, Option<String>)>,
    ) {
        self.entries.retain(|pane, _| panes.contains(pane));
        for pane in panes {
            match detected.get(pane) {
                Some((agent, id)) => {
                    // **ID の取りこぼしで既知の ID を捨てない**（同じ系統が動いている
                    // かぎり、`lsof` が一度失敗しただけで復元の種を失わないため）
                    let id = id.clone().or_else(|| {
                        self.entries
                            .get(pane)
                            .filter(|e| e.agent == *agent)
                            .and_then(|e| e.id.clone())
                    });
                    self.entries.insert(
                        *pane,
                        Entry {
                            agent: *agent,
                            id,
                            confirmed: true,
                            misses: 0,
                        },
                    );
                }
                None => {
                    let drop = match self.entries.get_mut(pane) {
                        // 未確認（= layout.json 由来）は不在では落とさない
                        Some(entry) if !entry.confirmed => false,
                        Some(entry) => {
                            entry.misses += 1;
                            entry.misses >= FORGET_AFTER_MISSES
                        }
                        None => false,
                    };
                    if drop {
                        self.entries.remove(pane);
                    }
                }
            }
        }
    }

    /// **A/B 専用**。検出結果でマップを丸ごと置き換える（= #1076 の壊れ方）を再現する。
    /// 製品経路では使わない
    pub fn replace_all_legacy(&mut self, detected: &HashMap<PaneId, (Agent, Option<String>)>) {
        self.entries = detected
            .iter()
            .map(|(pane, (agent, id))| {
                (
                    *pane,
                    Entry {
                        agent: *agent,
                        id: id.clone(),
                        confirmed: true,
                        misses: 0,
                    },
                )
            })
            .collect();
    }

    /// ペインを閉じたときに記録を外す
    pub fn remove(&mut self, pane: PaneId) {
        self.entries.remove(&pane);
    }

    /// そのペインで動いている系統と、採れていれば会話 ID
    pub fn get(&self, pane: PaneId) -> Option<(Agent, Option<&str>)> {
        self.entries.get(&pane).map(|e| (e.agent, e.id.as_deref()))
    }

    /// 会話 ID まで採れているペインの数（診断ログ用）
    pub fn with_id_count(&self) -> usize {
        self.entries.values().filter(|e| e.id.is_some()).count()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// 確認済み（スキャンで実際に見えた）ペインの数。診断ログ用
    pub fn confirmed_count(&self) -> usize {
        self.entries.values().filter(|e| e.confirmed).count()
    }
}

/// 復元コマンドの骨格（**純粋な形の宣言**。実際の文字列化は
/// `tako_control::sessions::resume_command` が 1 実装で行う）。
///
/// 系統ごとに「サブコマンドか、フラグか」「起動条件を足せるか」が違うだけで、
/// 組み立て自体は共通にできる。ここを表にしておかないと、
/// `if agent == "codex"` が呼び出し側へ散る（#982）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResumeSpec {
    /// 実行するプログラム名（= CLI のコマンド名）
    pub program: &'static str,
    /// プログラム直後に置く引数（codex の `resume` サブコマンド）
    pub head: &'static [&'static str],
    /// 会話 ID の直前に置くフラグ。`None` = 位置引数として直接渡す
    pub id_flag: Option<&'static str>,
    /// `--model` / `--effort` のような**起動条件を足してよいか**。
    ///
    /// codex の `resume` は `--model` を受け取らない（`codex resume --help` 実測。
    /// モデルは記録済みのセッション側にある）。agy の `--conversation` は
    /// グローバルフラグと併用できるが、会話に記録された設定を上書きしないよう
    /// **足さない**側へ倒してある
    pub accepts_launch_flags: bool,
}

/// 系統ごとの復元コマンドの骨格。`None` = その系統には復元の手段が無い
pub const fn resume_spec(agent: Agent) -> Option<ResumeSpec> {
    match agent {
        Agent::Claude => Some(ResumeSpec {
            program: "claude",
            head: &[],
            id_flag: Some("--resume"),
            accepts_launch_flags: true,
        }),
        // `codex resume [OPTIONS] [SESSION_ID]`（codex-cli 0.153.0 の `--help` 実測）
        Agent::Codex => Some(ResumeSpec {
            program: "codex",
            head: &["resume"],
            id_flag: None,
            accepts_launch_flags: false,
        }),
        // `agy --conversation <id>`（agy 1.1.27 の `--help` 実測）
        Agent::Agy => Some(ResumeSpec {
            program: "agy",
            head: &[],
            id_flag: Some("--conversation"),
            accepts_launch_flags: false,
        }),
        // ローカル LLM は会話の器そのものがまだ定まっていない（#991）
        Agent::Local => None,
    }
}

/// その系統を**この環境で**復元できるか（#1238）。
///
/// 「保存されていない（`ID なし`）」と「そもそも手段が無い（`resume 非対応`）」を
/// 復元の内訳ログで区別するための判定。**判断はここ 1 か所**に置く
/// （呼び出し側が `if agent == …` を持たない = #982）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RestoreSupport {
    /// 会話 ID を保存でき、復元コマンドも組める
    Wired,
    /// 手段が無い。`reason` は復元の内訳ログにそのまま出る
    Unwired { reason: &'static str },
}

impl RestoreSupport {
    pub fn is_wired(self) -> bool {
        matches!(self, Self::Wired)
    }
}

/// 復元の可否（**純粋関数**。`platform` を引数に取るので macOS から Windows 側を検査できる）
pub fn restore_support(agent: Agent, platform: Platform) -> RestoreSupport {
    let Some(_) = resume_spec(agent) else {
        return RestoreSupport::Unwired {
            reason: "この系統には会話を復元する手段がまだ無い",
        };
    };
    // claude の ID は `claude agents --json` から取るので OS に依らない。
    // codex / agy は「生きたプロセスが開いているロック」を `lsof` で引くしかなく、
    // Windows には同等の手段を同梱できない
    if agent != Agent::Claude && platform == Platform::Windows {
        return RestoreSupport::Unwired {
            reason: "Windows では会話 ID を採れない（lsof が無い）",
        };
    }
    RestoreSupport::Wired
}

/// 実行中の環境での復元の可否
pub fn restore_support_here(agent: Agent) -> RestoreSupport {
    restore_support(agent, Platform::current())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pane(n: u64) -> PaneId {
        PaneId::from_raw(n)
    }

    fn detected(pairs: &[(u64, Agent, Option<&str>)]) -> HashMap<PaneId, (Agent, Option<String>)> {
        pairs
            .iter()
            .map(|(p, a, id)| (pane(*p), (*a, id.map(str::to_string))))
            .collect()
    }

    /// 「系統も ID も分かっている」形（読みやすさのための短縮）
    fn hit(pane: u64, agent: Agent, id: &str) -> (u64, Agent, Option<&str>) {
        (pane, agent, Some(id))
    }

    /// #1076 の根因の一般形: 復元した ID が「起動途中で検出できない」だけで消えない
    #[test]
    fn 復元した未確認idはスキャンの不在で落ちない() {
        let mut ids = AgentResumeIds::new();
        ids.seed(pane(1), Agent::Codex, Some("aaa"));
        ids.seed(pane(2), Agent::Agy, Some("bbb"));
        for _ in 0..10 {
            ids.apply_scan(&[pane(1), pane(2)], &detected(&[]));
        }
        assert_eq!(ids.get(pane(1)), Some((Agent::Codex, Some("aaa"))));
        assert_eq!(ids.get(pane(2)), Some((Agent::Agy, Some("bbb"))));
        assert_eq!(ids.confirmed_count(), 0);
    }

    #[test]
    fn 確認済みidは連続不在で落ちる() {
        let mut ids = AgentResumeIds::new();
        ids.apply_scan(&[pane(1)], &detected(&[hit(1, Agent::Codex, "aaa")]));
        ids.apply_scan(&[pane(1)], &detected(&[]));
        assert!(ids.get(pane(1)).is_some(), "1 回目の不在で落ちている");
        ids.apply_scan(&[pane(1)], &detected(&[]));
        assert_eq!(ids.get(pane(1)), None);
    }

    /// ペインの系統が入れ替わったら（codex を終了して agy を起動した等）
    /// 検出結果の系統で置き換わる
    #[test]
    fn 系統が入れ替わったら検出側が勝つ() {
        let mut ids = AgentResumeIds::new();
        ids.seed(pane(1), Agent::Codex, Some("old"));
        ids.apply_scan(&[pane(1)], &detected(&[hit(1, Agent::Agy, "new")]));
        assert_eq!(ids.get(pane(1)), Some((Agent::Agy, Some("new"))));
    }

    #[test]
    fn 生きていないペインの記録は捨てる() {
        let mut ids = AgentResumeIds::new();
        ids.seed(pane(1), Agent::Codex, Some("aaa"));
        ids.seed(pane(2), Agent::Agy, Some("bbb"));
        ids.apply_scan(&[pane(1)], &detected(&[]));
        assert!(ids.get(pane(1)).is_some());
        assert_eq!(ids.get(pane(2)), None);
    }

    #[test]
    fn closeで即座に外れる() {
        let mut ids = AgentResumeIds::new();
        ids.seed(pane(1), Agent::Codex, Some("aaa"));
        ids.remove(pane(1));
        assert!(ids.is_empty());
    }

    #[test]
    fn seedは確認済みidを巻き戻さない() {
        let mut ids = AgentResumeIds::new();
        ids.apply_scan(&[pane(1)], &detected(&[hit(1, Agent::Codex, "new")]));
        ids.seed(pane(1), Agent::Codex, Some("old"));
        assert_eq!(ids.get(pane(1)), Some((Agent::Codex, Some("new"))));
    }

    /// agy は最初のターンまで会話が生まれない = 「系統は分かるが ID が無い」状態を通る。
    /// そのあと ID が採れたら埋まり、**一度採れた ID は取りこぼしで消えない**
    #[test]
    fn 系統だけ分かる状態から後でidが埋まる() {
        let mut ids = AgentResumeIds::new();
        ids.apply_scan(&[pane(1)], &detected(&[(1, Agent::Agy, None)]));
        assert_eq!(ids.get(pane(1)), Some((Agent::Agy, None)), "系統は分かる");
        assert_eq!(ids.with_id_count(), 0);
        ids.apply_scan(&[pane(1)], &detected(&[hit(1, Agent::Agy, "conv")]));
        assert_eq!(ids.get(pane(1)), Some((Agent::Agy, Some("conv"))));
        // lsof の取りこぼしで既知の ID を捨てない
        ids.apply_scan(&[pane(1)], &detected(&[(1, Agent::Agy, None)]));
        assert_eq!(
            ids.get(pane(1)),
            Some((Agent::Agy, Some("conv"))),
            "ID の取りこぼしで復元の種を失っている"
        );
        assert_eq!(ids.with_id_count(), 1);
    }

    /// 骨格は上流 CLI の実測（codex-cli 0.153.0 / agy 1.1.27）と一致する
    #[test]
    fn resume_specは系統ごとの実際の書式を表す() {
        let claude = resume_spec(Agent::Claude).expect("claude");
        assert_eq!(claude.program, "claude");
        assert_eq!(claude.id_flag, Some("--resume"));
        assert!(claude.accepts_launch_flags);

        let codex = resume_spec(Agent::Codex).expect("codex");
        assert_eq!((codex.program, codex.head), ("codex", &["resume"][..]));
        assert_eq!(codex.id_flag, None, "codex は ID を位置引数で取る");
        assert!(
            !codex.accepts_launch_flags,
            "codex resume は --model を受け取らない"
        );

        let agy = resume_spec(Agent::Agy).expect("agy");
        assert_eq!(agy.program, "agy");
        assert_eq!(agy.id_flag, Some("--conversation"));

        assert_eq!(resume_spec(Agent::Local), None);
    }

    /// 「保存されていない」と「手段が無い」を分ける判定（#1238 の要件 3）
    #[test]
    fn restore_supportはosと系統で答えを変える() {
        for agent in [Agent::Claude, Agent::Codex, Agent::Agy] {
            assert!(
                restore_support(agent, Platform::MacOs).is_wired(),
                "{agent:?} が macOS で非対応になっている"
            );
        }
        assert!(restore_support(Agent::Claude, Platform::Windows).is_wired());
        for agent in [Agent::Codex, Agent::Agy] {
            let support = restore_support(agent, Platform::Windows);
            assert!(
                !support.is_wired(),
                "{agent:?} は Windows で ID を採れない（lsof が無い）"
            );
            let RestoreSupport::Unwired { reason } = support else {
                unreachable!()
            };
            assert!(reason.contains("Windows"), "理由が読めない: {reason}");
        }
        assert!(!restore_support(Agent::Local, Platform::MacOs).is_wired());
    }
}
