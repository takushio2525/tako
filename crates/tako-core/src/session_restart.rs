//! session_restart — エージェントペインを「会話を失わずに」建て直す判断（Issue #1067）
//!
//! # 2 つのモード
//!
//! | モード | 何が変わるか | 何が残るか |
//! |---|---|---|
//! | [`SessionRestartMode::Harness`] | エージェント CLI のプロセス（= 実行中のバイナリ・env） | **会話そのまま**（`--resume`） |
//! | [`SessionRestartMode::Handoff`] | セッション（新しい会話） | 引き継ぎファイル経由の**要約** |
//!
//! ハーネス更新は #498（claude の自動更新後もプロセスが古い版のまま残る）の
//! ワンクリック解決手段で、**会話コンテキストを 1 文字も失わない**のが要点。
//! 引き継ぎ再起動は #749 の自動ハンドオフ（ctx% 高騰）を人の操作で起こせるようにしたもので、
//! **ctx をリセットできる**代わりに引き継ぎファイルに書いた分しか残らない。
//!
//! # ここに置くもの / 置かないもの
//!
//! - 置く: モードの語彙・実行してよいかの判断・後始末の段取り・画面から拾える手がかり（すべて純関数）
//! - 置かない: session_id の解決（`tako-control::agents` / `sessions`）・
//!   プロセスの終了（`tako-control::platform::process`）・
//!   コマンドの送達（`tako-core::shell_send` + `tako-app` の駆動）
//!
//! # 出た項目が断られない（#1006）と「理由が出る」（#1067）を両立させる
//!
//! 判断材料は 2 種類ある:
//!
//! - **構造的**（このペインで原理的に可能か）: セッションの有無・role・agent 系統・
//!   会話の解決可否。GUI のメニュー項目の**出し分け**はこれで決める
//!   （出た項目が断られない = #1006 の原則）
//! - **一時的**（今この瞬間は待つべきか）: 生成中・キュー滞留・入力欄の下書き・
//!   選択肢ダイアログ。これは**実行時に断って理由を返す**（メニューから消すと
//!   ユーザーが機能そのものを見つけられなくなるうえ、右クリックした瞬間の状態で
//!   項目が消えたり出たりする）
//!
//! この 2 段は [`can_restart`]（両方見る = 実行判断）と
//! [`menu_modes`]（構造だけ見る = 出し分け）で表してある。

use crate::agent_support::{self, keys, Agent};

/// 再起動の種別（CLI の possible values / MCP の enum / GUI の分岐の正本）
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionRestartMode {
    /// エージェント CLI のプロセスだけを建て直し、`--resume` で**同じ会話**を続ける
    Harness,
    /// 引き継ぎを書かせてから**新しいセッション**へ交代する（#749 の手動版）
    Handoff,
}

impl SessionRestartMode {
    /// 受け付ける値の一覧
    pub const VALUES: [&'static str; 2] = ["harness", "handoff"];

    /// ワイヤ表記（応答 JSON・CLI の表示）
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Harness => "harness",
            Self::Handoff => "handoff",
        }
    }

    /// 文字列から解釈する（大文字小文字と前後空白は無視。`remote_open` と同じ寛容さ）
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "harness" => Some(Self::Harness),
            "handoff" => Some(Self::Handoff),
            _ => None,
        }
    }

    /// 不正値のエラー文に添える案内
    pub fn values_hint() -> String {
        Self::VALUES.join(" | ")
    }

    /// この agent 系統でそのモードが使えるか（判断は能力マトリクス #982 の 1 マス）
    pub fn capability_key(self) -> &'static str {
        match self {
            Self::Harness => keys::SESSION_RESTART_HARNESS,
            Self::Handoff => keys::SESSION_RESTART_HANDOFF,
        }
    }
}

/// 再起動できない理由。**「できない」を黙って無視しない**ための型（`PaneSshBlock` と同じ思想）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RestartBlock {
    /// ターミナルセッションが無い（プレビュー・Web ビュー等）
    NoSession,
    /// AI エージェントのペインではない（role が付いていない）
    NotAgent,
    /// その agent 系統では手段が違う（claude 以外の resume・引き継ぎ）
    AgentUnsupported { agent: Agent },
    /// claude の会話（session_id）を解決できない = resume 先が分からない
    SessionUnresolved,
    /// エージェント CLI のプロセスが見つからない = 終了させる相手が分からない
    AgentProcessNotFound,
    /// 引き継ぎ再起動は master ペインだけ（worker / solo は引き継ぎ機構を持たない）
    HandoffNeedsMaster,
    /// 生成中・コマンド実行中（一時的）
    Busy,
    /// キューに未送信の指示が残っている（一時的。#572）
    QueuedMessages,
    /// 入力欄に人間の下書きがある（一時的）
    UserDraft,
    /// 選択肢ダイアログを表示中（一時的。#748）
    Dialog,
}

impl RestartBlock {
    /// ワイヤ表記（応答の `reason`）
    pub fn as_str(self) -> &'static str {
        match self {
            Self::NoSession => "no_session",
            Self::NotAgent => "not_agent",
            Self::AgentUnsupported { .. } => "agent_unsupported",
            Self::SessionUnresolved => "session_unresolved",
            Self::AgentProcessNotFound => "agent_process_not_found",
            Self::HandoffNeedsMaster => "handoff_needs_master",
            Self::Busy => "busy",
            Self::QueuedMessages => "queued_messages",
            Self::UserDraft => "user_draft",
            Self::Dialog => "dialog",
        }
    }

    /// 状態が変われば通るようになる理由か（= メニューからは消さない。モジュールの説明を参照）
    pub fn is_transient(self) -> bool {
        matches!(
            self,
            Self::SessionUnresolved
                | Self::AgentProcessNotFound
                | Self::Busy
                | Self::QueuedMessages
                | Self::UserDraft
                | Self::Dialog
        )
    }

    /// 理由 + 次の一手（dispatch / CLI / MCP / GUI のバナーにそのまま出す。規約どおり日本語）
    pub fn message(self, pane: u64, mode: SessionRestartMode) -> String {
        match self {
            Self::NoSession => format!(
                "pane {pane} にはターミナルセッションが無い（プレビュー等）ので再起動できない。\
                 エージェントが動いているペインを指定する"
            ),
            Self::NotAgent => format!(
                "pane {pane} は AI エージェントのペインではない（role が無い）ので\
                 セッション再起動の対象外。`tako list` の role で対象ペインを確認する"
            ),
            Self::AgentUnsupported { agent } => match mode {
                SessionRestartMode::Harness => format!(
                    "pane {pane} の系統（{}）は tako からの resume に未対応なので\
                     ハーネス更新できない（claude のみ）。手動なら codex は `codex resume`、\
                     agy は `agy --conversation` を使う。対応状況は `tako agent-support` を参照",
                    agent.as_str()
                ),
                SessionRestartMode::Handoff => format!(
                    "pane {pane} の系統（{}）は tako からの引き継ぎ再起動に未対応\
                     （claude のみ実測済み）。対応状況は `tako agent-support` を参照",
                    agent.as_str()
                ),
            },
            Self::SessionUnresolved => format!(
                "pane {pane} の claude の会話（session_id）を解決できないので\
                 ハーネス更新できない（resume 先が分からないまま終了させると会話を失う）。\
                 `tako sessions list` で会話を確認し、必要なら `tako sessions resume <id>` を使う"
            ),
            Self::AgentProcessNotFound => format!(
                "pane {pane} で動いているエージェント CLI のプロセスが見つからないので\
                 ハーネス更新できない（終了させる相手が分からないまま resume の行を打つと、\
                 動いているエージェントへの指示として入力されてしまう）。\
                 すでに終了しているなら `tako sessions resume <id>` で会話を別ペインへ開く"
            ),
            Self::HandoffNeedsMaster => format!(
                "pane {pane} は master ペインではないので引き継ぎ再起動できない\
                 （引き継ぎファイルの機構は master だけが持つ）。\
                 会話を保ったまま建て直すなら mode=harness を使う"
            ),
            Self::Busy => format!(
                "pane {pane} は生成中 / コマンド実行中なので再起動しない\
                 （途中の作業を失う）。終わってからもう一度実行する"
            ),
            Self::QueuedMessages => format!(
                "pane {pane} には未送信の指示がキューに残っているので再起動しない\
                 （その指示が失われる）。エージェントが処理し終えてからもう一度実行する"
            ),
            Self::UserDraft => format!(
                "pane {pane} の入力欄に下書きが残っているので再起動しない\
                 （打ちかけの指示が失われる）。送信するか消してからもう一度実行する"
            ),
            Self::Dialog => format!(
                "pane {pane} は選択肢ダイアログを表示中なので再起動しない。\
                 `tako orchestrator respond --pane {pane}` で応答してからもう一度実行する"
            ),
        }
    }
}

/// 判断の材料（すべて呼び出し側が観測して詰める。ここでは I/O をしない）
#[derive(Debug, Clone)]
pub struct RestartFacts {
    /// ターミナルセッションを持っている
    pub has_session: bool,
    /// AI エージェントの role が付いている
    pub is_agent: bool,
    /// master role（引き継ぎ機構を持つ）
    pub is_master: bool,
    /// このペインで動いている agent 系統
    pub agent: Agent,
    /// claude の会話（session_id）を解決できた（harness の**実行**に必須。
    /// 解決には I/O が要るので、メニューの出し分け（[`is_eligible`]）では見ない）
    pub session_resolved: bool,
    /// 終了させるエージェント CLI のプロセスが見つかった（harness の**実行**に必須。
    /// `session_resolved` と同じ理由でメニューの出し分けでは見ない）。
    ///
    /// **見つからないときに進めてはいけない**: 相手を終了させないまま resume の行を
    /// 打つと、動いているエージェントへの「指示」として入力される（#694 / #1006 と同じ罠）
    pub agent_process_found: bool,
    /// エージェントが生成中（画面の中断ヒント由来）。
    ///
    /// **OSC 133 の `Running` は使えない**（#1067 の実機実測）: エージェントは
    /// ペインのシェルが起動した前景コマンドなので、**エージェントが立っている間は
    /// ずっと `Running`** になる。これを busy と読むと、ハーネス更新の対象である
    /// エージェントペインが**常に** `busy` で断られる（実測: アイドルな worker で
    /// `reason=busy`）。「そのペインでコマンドが走っているか」は素のシェルを相手に
    /// する判定（`remote_open::can_ssh_pane`）では正しいが、ここでは意味が逆になる
    pub agent_busy: bool,
    /// キューに未送信の指示がある（#572）
    pub queued_messages: bool,
    /// 入力欄に人間の下書きがある
    pub user_draft: bool,
    /// 選択肢ダイアログを表示中（#748）
    pub dialog: bool,
}

impl Default for RestartFacts {
    fn default() -> Self {
        Self {
            has_session: false,
            is_agent: false,
            is_master: false,
            agent: Agent::Claude,
            session_resolved: false,
            agent_process_found: false,
            agent_busy: false,
            queued_messages: false,
            user_draft: false,
            dialog: false,
        }
    }
}

/// このペインでそのモードが**原理的に**可能か（構造的な条件だけを見る）。
///
/// GUI のメニュー項目の出し分けはこちらを使う（#1006 の「出た項目が断られない」は
/// 構造の話で、生成中のような一時的な状態で項目が出たり消えたりするのは別問題）。
///
/// **`session_resolved` はここでは見ない**（[`can_restart`] が見る）。理由は 2 つ:
/// 会話 ID の解決は `claude agents --json` の起動やカタログ読みを伴い**メニューの
/// 描画中に払える処理ではない**（#772 の教訓）うえ、その解決は**間欠的に失敗する**
/// （#1011 が sticky 解決を入れた理由）ので、構造の可否として扱うと項目が出たり
/// 消えたりする
pub fn is_eligible(mode: SessionRestartMode, facts: &RestartFacts) -> Result<(), RestartBlock> {
    if !facts.has_session {
        return Err(RestartBlock::NoSession);
    }
    if !facts.is_agent {
        return Err(RestartBlock::NotAgent);
    }
    if !agent_support::supports(facts.agent, mode.capability_key()) {
        return Err(RestartBlock::AgentUnsupported { agent: facts.agent });
    }
    if mode == SessionRestartMode::Handoff && !facts.is_master {
        return Err(RestartBlock::HandoffNeedsMaster);
    }
    Ok(())
}

/// 実行してよいか（構造 + 会話の解決 + 一時的な状態）。dispatch はこちらを通す
pub fn can_restart(mode: SessionRestartMode, facts: &RestartFacts) -> Result<(), RestartBlock> {
    is_eligible(mode, facts)?;
    // **画面の状態を先に見る**（順序は「失うものが大きい順」: 生成中 > キュー > 下書き）。
    // 「いま手を出すな」は待てば解ける第一の安全規則で、しかもここで断るときは
    // 会話 ID を引く必要すら無い。逆順にすると、生成中のペインに対して
    // 「`tako sessions list` で会話を確認せよ」という**その瞬間には無意味な**案内が出る
    if facts.agent_busy {
        return Err(RestartBlock::Busy);
    }
    if facts.queued_messages {
        return Err(RestartBlock::QueuedMessages);
    }
    if facts.user_draft {
        return Err(RestartBlock::UserDraft);
    }
    if facts.dialog {
        return Err(RestartBlock::Dialog);
    }
    // 会話 ID が引けないまま / 相手が分からないまま終了させると会話を失う
    // （引き継ぎは新しい会話を立てるのでどちらも要らない）
    if mode == SessionRestartMode::Harness {
        if !facts.session_resolved {
            return Err(RestartBlock::SessionUnresolved);
        }
        if !facts.agent_process_found {
            return Err(RestartBlock::AgentProcessNotFound);
        }
    }
    Ok(())
}

/// このペインで**メニューに出す**モード（構造的に可能なものだけ）。
///
/// GUI と `menu` 相当の応答が同じ関数を通るので、「画面に出ている項目」と
/// 「AI が引ける選択肢」がずれない
pub fn menu_modes(facts: &RestartFacts) -> Vec<SessionRestartMode> {
    [SessionRestartMode::Harness, SessionRestartMode::Handoff]
        .into_iter()
        .filter(|m| is_eligible(*m, facts).is_ok())
        .collect()
}

// ─────────────── 旧プロセスの終了と建て直しの段取り ───────────────
//
// ハーネス更新は「エージェントを終わらせる → 素のシェルへ resume の行を打つ」の 2 段。
// 実測（claude 2.1.258 / tmux 3.6 / #1067）:
//
// - **SIGTERM で 1 秒以内に落ち、代替画面も戻る**（`#{alternate_on}` が 1 → 0）。
//   シェルのプロンプトが画面に戻るので、#640 の送達フロー（画面のエコーを見る）が
//   そのまま噛み合う
// - 終了時に claude 自身が `Resume this session with:` と
//   `claude --resume <session-id>` を**画面へ印字する**。これは終了したまさにその
//   プロセスの会話なので、カタログ由来の推定より強い手がかりになる
// - `--resume` で会話が続くことと、**session_id が resume をまたいで変わらない**ことも実測
//
// プロセスが落ちきる前に打つと、resume の行が**動いている claude の入力欄へ**
// 流れ込む（#694 / #1006 で踏んだ「代替画面のまま書く」と同じ事故）。
// そのため「落ちたことを確かめてから打つ」を型にしてある。

/// SIGTERM から SIGKILL へ上げるまでの猶予
pub const TERMINATE_GRACE_SECS: u64 = 5;

/// 建て直しを諦めるまでの上限（相手が落ちない・落ちても打てない状況で永久に待たない）
pub const RELAUNCH_TIMEOUT_SECS: u64 = 30;

/// 次にやること（1 tick ぶん）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RelaunchStep {
    /// まだ落ちていない。待つ
    Wait,
    /// 猶予を過ぎたので強制終了する（SIGKILL）
    Force,
    /// 落ちた。resume の行を送達フローへ積む
    Launch,
    /// 上限に達した。理由を残して諦める
    GiveUp,
}

/// 旧プロセスの終了を待つ 1 tick ぶんの判断（純関数）。
///
/// `forced` = すでに SIGKILL を送ったか（二重送出を避ける）
pub fn relaunch_step(agent_alive: bool, elapsed_secs: u64, forced: bool) -> RelaunchStep {
    if !agent_alive {
        return RelaunchStep::Launch;
    }
    if elapsed_secs >= RELAUNCH_TIMEOUT_SECS {
        return RelaunchStep::GiveUp;
    }
    if elapsed_secs >= TERMINATE_GRACE_SECS && !forced {
        return RelaunchStep::Force;
    }
    RelaunchStep::Wait
}

/// claude が終了時に印字する案内の見出し（実測 2.1.258）
const RESUME_HINT_HEADING: &str = "Resume this session with:";

/// 終了した claude が画面へ残した `claude --resume <session-id>` を拾う（純関数）。
///
/// カタログ / `claude agents --json` 由来の session_id は**世代がずれることがある**
/// （同一ペイン番号に複数世代が堆積する = #466 の実測）。終了直後の画面に出る案内は
/// 「いま終わったプロセスの会話」なので、食い違ったらこちらを採る。
///
/// 画面には**過去の終了ぶんも残っている**ので、必ず最後の 1 件を返す。
/// 見出しの直後の行だけを見る（本文中の `claude --resume ...` を拾わないため）
pub fn parse_resume_hint(screen: &[String]) -> Option<String> {
    let mut found: Option<String> = None;
    for (i, line) in screen.iter().enumerate() {
        if !line.contains(RESUME_HINT_HEADING) {
            continue;
        }
        // 見出しと同じ行に続けて書かれている場合も拾う
        let same_line = line.split_once(RESUME_HINT_HEADING).map(|(_, rest)| rest);
        let candidates = [same_line.unwrap_or(""), screen.get(i + 1).map_or("", |s| s)];
        for cand in candidates {
            if let Some(id) = extract_resume_id(cand) {
                found = Some(id);
            }
        }
    }
    found
}

/// `claude --resume <id>` の `<id>` を取り出す（前後に余計な語があっても拾う）
fn extract_resume_id(line: &str) -> Option<String> {
    let rest = line.split_once("--resume")?.1;
    let token = rest.split_whitespace().next()?;
    is_session_id_shape(token).then(|| token.to_string())
}

/// claude の session_id の形（英数とハイフンのみ・十分な長さ）。
/// **`tako-control::transcript::is_valid_session_id` と同じ寛容さ**にそろえてある
/// （こちらはパス操作をしないので、ここでは長さだけを足して誤検出を防ぐ）
fn is_session_id_shape(s: &str) -> bool {
    s.len() >= 8
        && s.len() <= 128
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// 組み上がった resume コマンドから会話 ID を取り出す（差し替え検査の基準にする）。
///
/// **コマンドの形は `sessions::resume_command` が正**なので、こちらは
/// `--resume <id>` の 1 か所だけを読む
pub fn resume_id_of(command: &str) -> Option<String> {
    extract_resume_id(command)
}

/// 拾った手がかりで resume コマンドの session_id を差し替える。
///
/// 差し替えたら `true`。**コマンドの形は組み立て側（`sessions::resume_command`）が正**なので、
/// ここでは id の文字列だけを置き換える（モデル・effort・env 前置きはそのまま残る）
pub fn apply_resume_hint(command: &str, old_id: &str, hint: &str) -> (String, bool) {
    if hint == old_id || !command.contains(old_id) {
        return (command.to_string(), false);
    }
    (command.replace(old_id, hint), true)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn agent_pane() -> RestartFacts {
        RestartFacts {
            has_session: true,
            is_agent: true,
            is_master: false,
            agent: Agent::Claude,
            session_resolved: true,
            agent_process_found: true,
            ..RestartFacts::default()
        }
    }

    #[test]
    fn 語彙は往復する() {
        for v in SessionRestartMode::VALUES {
            let parsed = SessionRestartMode::parse(v).expect("VALUES は必ず解釈できる");
            assert_eq!(parsed.as_str(), v);
        }
        // 大文字・前後空白も受ける（CLI の手打ち）
        assert_eq!(
            SessionRestartMode::parse(" Harness "),
            Some(SessionRestartMode::Harness)
        );
        assert_eq!(SessionRestartMode::parse("resume"), None);
        assert_eq!(SessionRestartMode::values_hint(), "harness | handoff");
    }

    #[test]
    fn ワイヤ表記は語彙と一致する() {
        let json = serde_json::to_string(&SessionRestartMode::Handoff).unwrap();
        assert_eq!(json, "\"handoff\"");
        let parsed: SessionRestartMode = serde_json::from_str("\"harness\"").unwrap();
        assert_eq!(parsed, SessionRestartMode::Harness);
    }

    #[test]
    fn claudeのエージェントペインはハーネス更新できる() {
        assert_eq!(
            can_restart(SessionRestartMode::Harness, &agent_pane()),
            Ok(())
        );
        // 生成中の判断材料は画面の中断ヒントだけ（下の番犬が OSC 133 の復活を止める）
        let generating = RestartFacts {
            agent_busy: true,
            ..agent_pane()
        };
        assert_eq!(
            can_restart(SessionRestartMode::Harness, &generating),
            Err(RestartBlock::Busy)
        );
    }

    #[test]
    fn 構造的に無理なペインは理由つきで断る() {
        let preview = RestartFacts {
            has_session: false,
            ..agent_pane()
        };
        assert_eq!(
            can_restart(SessionRestartMode::Harness, &preview),
            Err(RestartBlock::NoSession)
        );
        let plain = RestartFacts {
            is_agent: false,
            ..agent_pane()
        };
        assert_eq!(
            can_restart(SessionRestartMode::Harness, &plain),
            Err(RestartBlock::NotAgent)
        );
        let unresolved = RestartFacts {
            session_resolved: false,
            ..agent_pane()
        };
        assert_eq!(
            can_restart(SessionRestartMode::Harness, &unresolved),
            Err(RestartBlock::SessionUnresolved)
        );
        // 会話の解決は**メニューの出し分けには使わない**（I/O が要る + 間欠的に失敗する）
        assert_eq!(
            is_eligible(SessionRestartMode::Harness, &unresolved),
            Ok(())
        );
    }

    /// claude 以外の系統は**マトリクス（#982）が Pending なら**断る。
    /// この判定を `if agent == claude` で散らさないのが #982 の規約
    #[test]
    fn 対応していない系統はマトリクス由来で断る() {
        for agent in [Agent::Codex, Agent::Agy, Agent::Local] {
            let facts = RestartFacts {
                agent,
                ..agent_pane()
            };
            assert_eq!(
                can_restart(SessionRestartMode::Harness, &facts),
                Err(RestartBlock::AgentUnsupported { agent }),
                "{} はまだ resume を配線していない",
                agent.as_str()
            );
        }
    }

    #[test]
    fn 引き継ぎ再起動はmasterだけ() {
        let worker = agent_pane();
        assert_eq!(
            can_restart(SessionRestartMode::Handoff, &worker),
            Err(RestartBlock::HandoffNeedsMaster)
        );
        let master = RestartFacts {
            is_master: true,
            ..agent_pane()
        };
        assert_eq!(can_restart(SessionRestartMode::Handoff, &master), Ok(()));
        // 引き継ぎは会話の解決を要らない（新しい会話を立てるので resume 先が無くてよい）
        let master_no_session = RestartFacts {
            is_master: true,
            session_resolved: false,
            ..agent_pane()
        };
        assert_eq!(
            can_restart(SessionRestartMode::Handoff, &master_no_session),
            Ok(())
        );
    }

    #[test]
    fn 一時的な状態は実行時に断るがメニューからは消えない() {
        let cases = [
            (
                RestartFacts {
                    agent_busy: true,
                    ..agent_pane()
                },
                RestartBlock::Busy,
            ),
            (
                RestartFacts {
                    queued_messages: true,
                    ..agent_pane()
                },
                RestartBlock::QueuedMessages,
            ),
            (
                RestartFacts {
                    user_draft: true,
                    ..agent_pane()
                },
                RestartBlock::UserDraft,
            ),
            (
                RestartFacts {
                    dialog: true,
                    ..agent_pane()
                },
                RestartBlock::Dialog,
            ),
        ];
        for (facts, want) in cases {
            assert_eq!(
                can_restart(SessionRestartMode::Harness, &facts),
                Err(want),
                "実行時には断る: {want:?}"
            );
            assert!(want.is_transient(), "{want:?} は一時的な理由");
            assert_eq!(
                is_eligible(SessionRestartMode::Harness, &facts),
                Ok(()),
                "メニューからは消さない（機能を見つけられなくなる）: {want:?}"
            );
            assert_eq!(
                menu_modes(&facts),
                vec![SessionRestartMode::Harness],
                "一時的な理由でメニューの項目数が揺れない: {want:?}"
            );
        }
    }

    /// 関門の順序は固定する（#1067）。生成中のペインに「会話を確認せよ」を出さない
    /// **番犬**（#1067 の実機実測）: エージェントは「ペインのシェルが起動した前景
    /// コマンド」なので、立っている間ペインの OSC 133 は**ずっと `Running`**。
    /// これを busy の材料に戻すと、ハーネス更新の対象であるエージェントペインが
    /// **常に** `busy` で断られる（実測: アイドルな worker で `reason=busy`）。
    /// 判断材料を増やすときはこの理由を読んでから増やすこと
    #[test]
    fn osc133のコマンド状態を判断材料に戻していない() {
        let src = include_str!("session_restart.rs");
        // テストモジュールより前（= 実装本体）だけを見る
        let body = src.split("#[cfg(test)]").next().unwrap_or(src);
        assert!(
            !body.contains("CommandState"),
            "OSC 133 のコマンド状態を判断材料へ戻してはいけない（理由はこのテストの doc）"
        );
    }

    #[test]
    fn 画面の状態を会話の解決より先に見る() {
        let busy_unresolved = RestartFacts {
            agent_busy: true,
            session_resolved: false,
            agent_process_found: false,
            ..agent_pane()
        };
        assert_eq!(
            can_restart(SessionRestartMode::Harness, &busy_unresolved),
            Err(RestartBlock::Busy),
            "待てば解ける理由を先に返す"
        );
        // 画面が静かになれば会話の解決の話になる
        let idle_unresolved = RestartFacts {
            agent_busy: false,
            ..busy_unresolved
        };
        assert_eq!(
            can_restart(SessionRestartMode::Harness, &idle_unresolved),
            Err(RestartBlock::SessionUnresolved)
        );
    }

    #[test]
    fn メニューは構造的に可能なものだけ並べる() {
        // 素のシェル: 何も出ない
        assert!(menu_modes(&RestartFacts {
            has_session: true,
            ..RestartFacts::default()
        })
        .is_empty());
        // worker: ハーネス更新だけ
        assert_eq!(menu_modes(&agent_pane()), vec![SessionRestartMode::Harness]);
        // master: 両方
        let master = RestartFacts {
            is_master: true,
            ..agent_pane()
        };
        assert_eq!(
            menu_modes(&master),
            vec![SessionRestartMode::Harness, SessionRestartMode::Handoff]
        );
        // 会話が解決できない master でもメニューは変わらない（実行時に理由が出る）
        let master_unresolved = RestartFacts {
            session_resolved: false,
            ..master
        };
        assert_eq!(
            menu_modes(&master_unresolved),
            vec![SessionRestartMode::Harness, SessionRestartMode::Handoff]
        );
    }

    #[test]
    fn 断る理由には次の一手が入る() {
        let blocks = [
            RestartBlock::NoSession,
            RestartBlock::NotAgent,
            RestartBlock::AgentUnsupported {
                agent: Agent::Codex,
            },
            RestartBlock::SessionUnresolved,
            RestartBlock::AgentProcessNotFound,
            RestartBlock::HandoffNeedsMaster,
            RestartBlock::Busy,
            RestartBlock::QueuedMessages,
            RestartBlock::UserDraft,
            RestartBlock::Dialog,
        ];
        for block in blocks {
            for mode in [SessionRestartMode::Harness, SessionRestartMode::Handoff] {
                let msg = block.message(7, mode);
                assert!(msg.contains("pane 7"), "対象ペインを名指しする: {msg}");
                // 「どうすれば通るか」を必ず添える（コマンドか、待つ / 別モードの案内）
                let has_next_step = msg.contains("tako ")
                    || msg.contains("もう一度")
                    || msg.contains("mode=harness")
                    || msg.contains("指定する");
                assert!(has_next_step, "次の一手を添える: {msg}");
            }
            assert!(!block.as_str().is_empty());
        }
    }

    #[test]
    fn 落ちるまで待ち猶予を過ぎたら強制終了する() {
        assert_eq!(relaunch_step(true, 0, false), RelaunchStep::Wait);
        assert_eq!(
            relaunch_step(true, TERMINATE_GRACE_SECS - 1, false),
            RelaunchStep::Wait
        );
        assert_eq!(
            relaunch_step(true, TERMINATE_GRACE_SECS, false),
            RelaunchStep::Force
        );
        // 二重に SIGKILL は撃たない
        assert_eq!(
            relaunch_step(true, TERMINATE_GRACE_SECS, true),
            RelaunchStep::Wait
        );
        assert_eq!(
            relaunch_step(true, RELAUNCH_TIMEOUT_SECS, true),
            RelaunchStep::GiveUp
        );
        // 落ちていれば上限を過ぎていても打つ（会話を宙ぶらりんにしない）
        assert_eq!(
            relaunch_step(false, RELAUNCH_TIMEOUT_SECS + 10, true),
            RelaunchStep::Launch
        );
        assert_eq!(relaunch_step(false, 0, false), RelaunchStep::Launch);
    }

    /// 実測の画面（claude 2.1.258 が SIGTERM で終了した直後。#1067）。
    /// **過去の終了ぶんも残っている**ので最後の 1 件を採る
    #[test]
    fn 終了時の案内から会話idを拾う() {
        let screen: Vec<String> = [
            "direnv: unloading",
            "[testuser@host:/tmp]$ claude",
            "",
            "Resume this session with:",
            "claude --resume 0e7ec5d5-80e8-4070-9968-99b111391068",
            "[testuser@host:/tmp]$ claude --resume 0e7ec5d5-80e8-4070-9968-99b111391068",
            "",
            "Resume this session with:",
            "claude --resume 11112222-3333-4444-5555-666677778888",
            "[testuser@host:/tmp]$",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        assert_eq!(
            parse_resume_hint(&screen).as_deref(),
            Some("11112222-3333-4444-5555-666677778888"),
            "最後に印字された案内を採る"
        );
    }

    #[test]
    fn 案内が無い画面からは拾わない() {
        let screen: Vec<String> = ["$ ls", "Cargo.toml  src", "$ "]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert_eq!(parse_resume_hint(&screen), None);
        // 会話の本文に出てくる `--resume` は見出しが無いので拾わない
        let chat: Vec<String> = ["⏺ claude --resume abcdefgh を実行してください"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert_eq!(parse_resume_hint(&chat), None);
        // 見出しはあるが id の形でないもの
        let broken: Vec<String> = ["Resume this session with:", "claude --resume <id>"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert_eq!(parse_resume_hint(&broken), None);
    }

    #[test]
    fn resumeコマンドから会話idを読む() {
        assert_eq!(
            resume_id_of("claude --model opus --resume abcd-1234 ").as_deref(),
            Some("abcd-1234")
        );
        // 会話 ID を持たないコマンドからは読まない
        assert_eq!(resume_id_of("claude --model opus"), None);
        assert_eq!(resume_id_of(""), None);
    }

    #[test]
    fn 手がかりで会話idを差し替える() {
        let cmd = "unset CLAUDE_CONFIG_DIR; TAKO_ORCHESTRATOR_ROLE=worker:tako \
                   claude --model opus --effort high --resume old-1234-id";
        let (out, changed) = apply_resume_hint(cmd, "old-1234-id", "new-5678-id");
        assert!(changed);
        assert!(out.contains("--resume new-5678-id"), "{out}");
        // モデル・effort・env 前置きは残る（コマンドの形は組み立て側が正）
        assert!(out.contains("--model opus --effort high"), "{out}");
        assert!(out.starts_with("unset CLAUDE_CONFIG_DIR;"), "{out}");
        // 同じ id なら触らない
        let (same, changed) = apply_resume_hint(cmd, "old-1234-id", "old-1234-id");
        assert!(!changed);
        assert_eq!(same, cmd);
        // 元 id がコマンドに無ければ触らない（取り違えて別の語を壊さない）
        let (kept, changed) = apply_resume_hint(cmd, "absent-id", "new-5678-id");
        assert!(!changed);
        assert_eq!(kept, cmd);
    }
}
