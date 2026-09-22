//! setup を完走させたあとに人へ残る作業の宣言（#1501）
//!
//! ## 何が起きていたのか
//!
//! `tako setup` は詰まった段でそのまま `Err` を返して **exit 1** していた。
//! 一番手前の段が認証（bootstrap [3/3]）なので、新品の Mac では
//! `error: Claude アカウントへのログインが必要です` で終わり、**以降の段が
//! 1 つも走らなかった**（`profiles/default.yaml`・`~/.claude/CLAUDE.md`・
//! テンプレ・MCP 登録が全部未作成。#1500 の棚卸し R1 / R2 / R3 で実測）。
//! 「setup を走らせても何も整わない」が新品環境の既定の結末だった。
//!
//! ログイン自体はブラウザ操作なので tako は代行しない（#1129。ブラウザ待ちの
//! プロセスは自分では終わらず、Windows では孤児として積み上がった）。
//! 代行できないこと自体は変わらないが、**代行できない 1 件のために
//! 代行できる 10 件を捨てる**必要はない。
//!
//! ## この形（残り作業として脇に置く）
//!
//! 詰まった段は `Remaining` を積んで**続ける**。認証不要な段（依存 / MCP 登録 /
//! 指示ファイル / プロファイル / テンプレ / シェル統合 / PATH 設置）を全部やってから、
//! 最後に「残り」を**人が次に打つ 1 コマンド**つきで出して **exit 0** で終わる。
//! 再実行すると、済んだ段は冪等に素通りして残りから再開する。
//!
//! - **判断はここ（純粋関数）**。CLI は積んで、[`summarize`] に渡して、
//!   [`render`] の結果を表示するだけ（#1485 / #1499 と同じ作法）
//!   [`summarize`]: summarize
//! - **1 件ごとに「次に打つ 1 行」を必ず持つ**（#322 の最簡形）。
//!   持てないものは [`RemainingKind::command`] が `None` を返し、
//!   代わりに理由の行を出す（**黙って消さない**）
//! - 同じ道は 1 本にまとめる（認証の詰まりは bootstrap 段と
//!   エージェント選択の 2 か所から積まれるので、素だと 2 行出る）
//!
//! A/B は `TAKO_1501_LEGACY=1`（[`legacy_stop`] が #1501 前の exit 1 へ戻す）。

use tako_core::platform::agent_install::AgentKind;

use crate::setup_bootstrap::{self, Step};

/// 人の手でしか終わらない残り作業の種類。
///
/// **setup を止める理由にはしない**（脇に置いて残りの段を全部やる）
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RemainingKind {
    /// エージェント CLI の導入（自動導入が通らなかった / 断られた）
    AgentInstall(AgentKind),
    /// エージェント CLI の PATH 通し
    AgentPath(AgentKind),
    /// エージェント CLI へのログイン（ブラウザ操作。tako は代行しない = #1129）
    AgentLogin(AgentKind),
    /// エージェント CLI が 1 つも無い（どの系統でもよい）
    AgentMissing,
    /// 導入状況そのものを読めない
    AgentStatusUnknown,
    /// tako MCP の登録が通らなかった
    Mcp { agent: String },
    /// 依存ツールの導入
    Dep { bin: String },
    /// シェル統合の配置（Windows の `$PROFILE`。#1504）
    ShellIntegration,
}

impl RemainingKind {
    /// 一覧に出す 1 行の見出し（**何が残っているか**）
    pub fn title(&self) -> String {
        match self {
            Self::AgentInstall(agent) => format!("{} の導入", agent.product()),
            Self::AgentPath(agent) => {
                format!(
                    "{} コマンドをどのターミナルからも使えるようにする",
                    agent.as_str()
                )
            }
            Self::AgentLogin(agent) => {
                format!("{}へのログイン", setup_bootstrap::account_label(*agent))
            }
            Self::AgentMissing => {
                "エージェント CLI（claude / codex / agy のいずれか）の導入".to_string()
            }
            Self::AgentStatusUnknown => "エージェント CLI の導入状況の確認".to_string(),
            Self::Mcp { agent } => format!("{agent} への tako MCP 登録"),
            Self::Dep { bin } => format!("{bin} の導入"),
            Self::ShellIntegration => {
                "シェル統合の配置（ペインの cwd 追従とコマンド実行状態）".to_string()
            }
        }
    }

    /// 人が次に打つ 1 行（**最簡形**。既定で済む引数は付けない = #322）。
    ///
    /// `None` = そのまま打てるコマンドが無いもの（系統ごとのログイン手順が
    /// 取れない場合など）。呼び手は代わりに [`Remaining::detail`] を出す
    pub fn command(&self) -> Option<String> {
        match self {
            Self::AgentInstall(agent) => Some(format!(
                "tako setup bootstrap install{}",
                setup_bootstrap::agent_flag(*agent)
            )),
            Self::AgentPath(agent) => Some(format!(
                "tako setup bootstrap path{}",
                setup_bootstrap::agent_flag(*agent)
            )),
            // ログインコマンドの正本は `agent_cli::auth_command`（無い系統は推測で埋めない）
            Self::AgentLogin(agent) => {
                crate::orchestrator::agent_cli::auth_command((*agent).into()).map(str::to_string)
            }
            Self::AgentMissing => Some("tako setup bootstrap install".to_string()),
            Self::AgentStatusUnknown => Some("tako setup bootstrap status-all".to_string()),
            Self::Mcp { .. } => Some("tako setup-mcp".to_string()),
            Self::Dep { .. } => Some("tako setup deps install".to_string()),
            Self::ShellIntegration => Some("tako shell-integration install".to_string()),
        }
    }

    /// 既定の理由・注意（段が固有の理由を持たないときに出す）
    pub fn default_detail(&self) -> Vec<String> {
        match self {
            Self::AgentLogin(_) => {
                vec!["ブラウザでの操作が要るため tako は代行しません".to_string()]
            }
            Self::AgentMissing => {
                vec!["どれか 1 つ入れてログインすれば master / worker が動きます".to_string()]
            }
            Self::Mcp { .. } => {
                vec!["登録が無いと AI から tako の画面を操作できません".to_string()]
            }
            Self::ShellIntegration => vec![
                "無くても tako は動きますが、ペインの cwd 追従・コマンド実行状態・入力予測が働きません"
                    .to_string(),
            ],
            _ => Vec::new(),
        }
    }

    /// ゼロスタート導入の段（[`Step`]）から残り作業へ（**この写しが 1 実装**）。
    ///
    /// `tako setup` は段の中で詰まった位置から積み、`tako setup --check` は
    /// 読み取った段から積む。同じ段が違う名前で出ないよう、写しはここだけに置く
    pub fn for_step(step: Step, agent: AgentKind) -> Option<Self> {
        match step {
            Step::Install => Some(Self::AgentInstall(agent)),
            Step::Path => Some(Self::AgentPath(agent)),
            Step::Auth => Some(Self::AgentLogin(agent)),
            Step::Ready => None,
        }
    }

    /// 並び順。**一番手前の段から**（導入 → PATH → ログイン → MCP → 依存）。
    /// 人は上から順に片付ければよい形にする
    fn order(&self) -> u8 {
        match self {
            Self::AgentStatusUnknown => 0,
            Self::AgentMissing => 1,
            Self::AgentInstall(_) => 2,
            Self::AgentPath(_) => 3,
            Self::AgentLogin(_) => 4,
            Self::Mcp { .. } => 5,
            Self::Dep { .. } => 6,
            // 一番後ろ（無くても tako は動く上積み。ここが手前に来ると
            // 「まず tako を使える状態にする」という並びの意図が崩れる）
            Self::ShellIntegration => 7,
        }
    }
}

/// 残り作業 1 件
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Remaining {
    pub kind: RemainingKind,
    /// 理由・注意の行（段が固有の理由を返したときはそれを持つ）
    pub detail: Vec<String>,
}

impl Remaining {
    /// 既定の理由で 1 件作る
    pub fn new(kind: RemainingKind) -> Self {
        let detail = kind.default_detail();
        Self { kind, detail }
    }

    /// 段が返した理由を持たせて 1 件作る（**黙って捨てない**）
    pub fn with_detail(kind: RemainingKind, detail: Vec<String>) -> Self {
        Self { kind, detail }
    }
}

/// 積まれた残り作業を「人が上から片付ける一覧」へ整える（**純粋関数**）。
///
/// 1. 同じ道は 1 本にする（認証の詰まりは bootstrap 段とエージェント選択の
///    2 か所から積まれる = 素だと同じログインが 2 行出る）
/// 2. 系統が特定できている導入が在るなら、総称の「1 つも無い」は落とす
///    （同じ道の粗い言い方なので、2 行に見えると「2 件やることがある」と誤解する）
/// 3. 手前の段から並べる（[`RemainingKind::order`]）
pub fn summarize(items: Vec<Remaining>) -> Vec<Remaining> {
    let has_specific_install = items
        .iter()
        .any(|item| matches!(item.kind, RemainingKind::AgentInstall(_)));
    let mut out: Vec<Remaining> = Vec::new();
    for item in items {
        if has_specific_install && item.kind == RemainingKind::AgentMissing {
            continue;
        }
        // 先に積まれたほうの理由を残す（段が返した具体的な理由は手前にある）
        if out.iter().any(|kept| kept.kind == item.kind) {
            continue;
        }
        out.push(item);
    }
    out.sort_by_key(|item| item.kind.order());
    out
}

/// 「残り」の表示（**この 1 実装だけが文面を持つ**）。
///
/// `tako setup` の末尾と `tako setup --check` の末尾が同じものを出す
/// （CLI / MCP の 1:1 = 開発不変条件）。空なら 1 行も出さない
pub fn render(items: &[Remaining]) -> Vec<String> {
    if items.is_empty() {
        return Vec::new();
    }
    let mut out = vec![
        String::new(),
        format!("残り {} 件（ここから先は人の操作が必要です）:", items.len()),
    ];
    for (index, item) in items.iter().enumerate() {
        out.push(format!("  {}. {}", index + 1, item.kind.title()));
        if let Some(command) = item.kind.command() {
            out.push(format!("     {command}"));
        }
        for line in &item.detail {
            out.push(format!("     {line}"));
        }
    }
    out.push(
        "済んだら tako setup をもう一度実行してください（残りはここから再開します）".to_string(),
    );
    out
}

/// `TAKO_1501_LEGACY=1` で #1501 前（詰まった段で exit 1）へ戻す。
///
/// **製品の既定経路ではない**。同一バイナリで A/B を取るための逃げ道で、
/// 実経路テスト `scripts/test-setup-continue-1501.sh` が「旧アームでは
/// 何も整わない」ことを毎回確かめる
pub fn legacy_mode() -> bool {
    std::env::var_os("TAKO_1501_LEGACY").is_some()
}

/// 旧アームのときだけ、積まれた残り作業の 1 件目で止める。
///
/// 呼び手（CLI）は `legacy_stop(&remaining)?` と書く。**現行では常に `Ok`** で、
/// 「詰まったら止める」判断がここ 1 か所にしか無いことを番犬が縛る
pub fn legacy_stop(items: &[Remaining]) -> Result<(), String> {
    if !legacy_mode() {
        return Ok(());
    }
    let Some(first) = items.first() else {
        return Ok(());
    };
    let mut lines = vec![first.kind.title()];
    lines.extend(first.detail.clone());
    if let Some(command) = first.kind.command() {
        lines.push(format!("次の 1 手: {command}"));
    }
    Err(lines.join("\n"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 同じログインが二重に出ない() {
        let items = summarize(vec![
            Remaining::new(RemainingKind::AgentLogin(AgentKind::Claude)),
            Remaining::new(RemainingKind::AgentLogin(AgentKind::Claude)),
        ]);
        assert_eq!(
            items.len(),
            1,
            "bootstrap 段と選択段の両方から積まれても 1 行"
        );
        assert_eq!(items[0].kind, RemainingKind::AgentLogin(AgentKind::Claude));
    }

    #[test]
    fn 系統が特定できている導入があれば総称は落ちる() {
        let items = summarize(vec![
            Remaining::new(RemainingKind::AgentMissing),
            Remaining::with_detail(
                RemainingKind::AgentInstall(AgentKind::Claude),
                vec!["ネットワークに届きません".to_string()],
            ),
        ]);
        assert_eq!(items.len(), 1);
        assert_eq!(
            items[0].kind,
            RemainingKind::AgentInstall(AgentKind::Claude),
            "粗い言い方（1 つも無い）ではなく、具体的な系統の導入が残る"
        );
        assert_eq!(
            items[0].detail,
            vec!["ネットワークに届きません".to_string()]
        );
    }

    #[test]
    fn 総称だけなら残る() {
        let items = summarize(vec![Remaining::new(RemainingKind::AgentMissing)]);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].kind, RemainingKind::AgentMissing);
    }

    #[test]
    fn 手前の段から並ぶ() {
        let items = summarize(vec![
            Remaining::new(RemainingKind::Dep {
                bin: "tmux".to_string(),
            }),
            Remaining::new(RemainingKind::AgentLogin(AgentKind::Claude)),
            Remaining::new(RemainingKind::AgentPath(AgentKind::Codex)),
        ]);
        let order: Vec<String> = items.iter().map(|item| item.kind.title()).collect();
        assert!(
            order[0].contains("codex")
                && order[1].contains("ログイン")
                && order[2].contains("tmux"),
            "PATH → ログイン → 依存 の順に並ぶ: {order:?}"
        );
    }

    #[test]
    fn 段から残り作業への写しが全段を覆う() {
        // Ready 以外は必ず 1 件へ写る（写し漏れがあると --check が残りを黙って落とす）
        for (step, expect) in [
            (
                Step::Install,
                Some(RemainingKind::AgentInstall(AgentKind::Claude)),
            ),
            (
                Step::Path,
                Some(RemainingKind::AgentPath(AgentKind::Claude)),
            ),
            (
                Step::Auth,
                Some(RemainingKind::AgentLogin(AgentKind::Claude)),
            ),
            (Step::Ready, None),
        ] {
            assert_eq!(
                RemainingKind::for_step(step, AgentKind::Claude),
                expect,
                "{} の写しが違う",
                step.as_str()
            );
        }
    }

    /// シェル統合は「無くても tako は動く上積み」なので一番後ろ（[`RemainingKind::order`]）
    #[test]
    fn シェル統合は一番後ろに並ぶ() {
        let items = summarize(vec![
            Remaining::new(RemainingKind::ShellIntegration),
            Remaining::new(RemainingKind::AgentLogin(AgentKind::Claude)),
            Remaining::new(RemainingKind::Dep {
                bin: "tmux".to_string(),
            }),
        ]);
        let order: Vec<&RemainingKind> = items.iter().map(|item| &item.kind).collect();
        assert_eq!(
            order,
            vec![
                &RemainingKind::AgentLogin(AgentKind::Claude),
                &RemainingKind::Dep {
                    bin: "tmux".to_string()
                },
                &RemainingKind::ShellIntegration,
            ],
            "ログイン → 依存 → シェル統合 の順に並ぶ"
        );
    }

    #[test]
    fn 全種別が次に打つ1行を持つ() {
        let kinds = [
            RemainingKind::AgentInstall(AgentKind::Claude),
            RemainingKind::AgentPath(AgentKind::Codex),
            RemainingKind::AgentLogin(AgentKind::Claude),
            RemainingKind::AgentLogin(AgentKind::Codex),
            RemainingKind::AgentLogin(AgentKind::Agy),
            RemainingKind::AgentMissing,
            RemainingKind::AgentStatusUnknown,
            RemainingKind::Mcp {
                agent: "claude".to_string(),
            },
            RemainingKind::Dep {
                bin: "tmux".to_string(),
            },
            RemainingKind::ShellIntegration,
        ];
        for kind in kinds {
            assert!(
                kind.command().is_some(),
                "{kind:?} に「次に打つ 1 行」が無い（#322）"
            );
            assert!(!kind.title().is_empty(), "{kind:?} の見出しが空");
        }
    }

    #[test]
    fn 表示は最簡形のコマンドと再開の案内を持つ() {
        let lines = render(&summarize(vec![Remaining::new(RemainingKind::AgentLogin(
            AgentKind::Claude,
        ))]));
        let text = lines.join("\n");
        assert!(text.contains("残り 1 件"), "件数を言う: {text}");
        assert!(text.contains("ログイン"), "何が残っているかを言う: {text}");
        assert!(text.contains("claude auth login"), "次に打つ 1 行: {text}");
        assert!(
            text.contains("tako setup をもう一度実行"),
            "再開のしかたを言う: {text}"
        );
        // 既定で済む引数を付けない（#322）
        assert!(!text.contains("tako setup --yes"), "最簡形でない: {text}");
    }

    #[test]
    fn 残りが無ければ1行も出さない() {
        assert!(render(&[]).is_empty());
    }

    #[test]
    fn 旧アームでない限り止めない() {
        // このプロセスは env を触らない（並行テストへ漏らさない）ので、
        // 既定アーム = 何が積まれていても Ok であることだけを見る
        let items = vec![Remaining::new(RemainingKind::AgentLogin(AgentKind::Claude))];
        if legacy_mode() {
            assert!(legacy_stop(&items).is_err(), "旧アームでは止まる");
        } else {
            assert!(legacy_stop(&items).is_ok(), "既定アームでは止まらない");
        }
    }
}
