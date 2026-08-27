//! remote_open — SSH 接続を「どこに開くか」の語彙と、既存ペインを SSH 化してよいかの判定（#1006）
//!
//! # なぜ純粋関数にするのか
//!
//! 開き先は GUI（ファイルメニュー「リモート接続…」/ ペインの右クリック）・CLI
//! （`tako open-in remote`）・MCP（`tako_open_remote`）の 3 経路が指定する。
//! 語彙が 3 箇所に散ると「画面に見えている語で操作できない」（#553 で踏んだ形）に
//! なるので、**正本をここ 1 本**にして CLI の値一覧・MCP の enum・GUI の分岐が
//! すべてこの表から引く。
//!
//! 既存ペインの SSH 化（[`RemoteOpenTarget::Pane`]）の可否も同じ理由でここに置く。
//! 判定材料（セッションの有無 / 代替画面 / OSC 133 の状態 / role）は呼び出し側が
//! 集めるので、**PTY も GUI も無いテストで全組み合わせを検査できる**。

/// SSH 接続の開き先（#1006）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RemoteOpenTarget {
    /// **いま開いているタブへ新しいペインを作って**接続する（既定）。
    ///
    /// #1006 の要望そのもの: 「原則開いてるタブに新ペインで出す」。
    /// 従来の既定は [`RemoteOpenTarget::Tab`] だった（#20）
    #[default]
    Split,
    /// 新しいタブで接続する（#20 の従来動作）
    Tab,
    /// **すでにあるペインをそのまま SSH にする**（#1006）。
    ///
    /// ペインの右クリックメニューの「リモート接続…」がこれ。新しいペインも
    /// タブも増やさず、**ペイン ID も変わらない**。素のシェルへ ssh の行を
    /// 送達確認つきで打つので、接続に失敗しても**シェルのプロンプトへ戻る**
    /// （器が入力を落とす環境でも届く経路 = #640）
    Pane,
}

impl RemoteOpenTarget {
    /// 受け付ける値の一覧（CLI の possible values / MCP の enum / エラー文の正本）
    pub const VALUES: [&'static str; 3] = ["split", "tab", "pane"];

    /// ワイヤ表記（応答 JSON・CLI の表示に使う）
    pub fn as_str(self) -> &'static str {
        match self {
            RemoteOpenTarget::Split => "split",
            RemoteOpenTarget::Tab => "tab",
            RemoteOpenTarget::Pane => "pane",
        }
    }

    /// 文字列から解釈する（大文字小文字と前後空白は無視）
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "split" => Some(RemoteOpenTarget::Split),
            "tab" => Some(RemoteOpenTarget::Tab),
            "pane" => Some(RemoteOpenTarget::Pane),
            _ => None,
        }
    }

    /// 不正値のエラー文に添える案内（`fleet | orch | git` と同じ形。#553）
    pub fn values_hint() -> String {
        Self::VALUES.join(" | ")
    }
}

/// 既存ペインを SSH 化できない理由（#1006）。
///
/// **「できない」を黙って無視しない**のがこの型の目的。理由ごとに次の一手が違う
/// （プレビューなら分割、TUI なら分割、実行中なら待つか分割）ので、
/// エラー文にそのまま出す
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PaneSshBlock {
    /// ターミナルセッションが無い（プレビュー・Web ビュー等）
    NoSession,
    /// 代替画面（全画面 TUI）が出ている = 素のシェルではない
    AltScreen,
    /// OSC 133 が「コマンド実行中」と言っている
    Running,
    /// AI エージェントの role が付いている（master / worker）
    AgentRole,
}

impl PaneSshBlock {
    /// 理由 + 次の一手（dispatch / CLI / MCP のエラー文。規約どおり日本語）
    pub fn message(self, pane: u64) -> String {
        match self {
            PaneSshBlock::NoSession => format!(
                "pane {pane} にはターミナルセッションが無い（プレビュー等）ので SSH 化できない。\
                 target=split で新しいペインを作って接続する"
            ),
            PaneSshBlock::AltScreen => format!(
                "pane {pane} は全画面 TUI を表示中（素のシェルではない）ので SSH 化できない。\
                 TUI を終了させるか、target=split で新しいペインを作って接続する"
            ),
            PaneSshBlock::Running => format!(
                "pane {pane} はコマンド実行中なので SSH 化できない（入力が実行中の\
                 プロセスへ流れる）。終わるのを待つか、target=split で新しいペインを作って接続する"
            ),
            PaneSshBlock::AgentRole => format!(
                "pane {pane} は AI エージェントのペインなので SSH 化できない\
                 （対話が壊れる）。target=split で新しいペインを作って接続する"
            ),
        }
    }
}

/// 既存ペインを SSH 化してよいかの判定（純粋関数）。
///
/// `command_state` は OSC 133 由来。**`Unknown` は拒否しない**（シェル統合が
/// 効かない構成 = 器が OSC を素通ししない psmux でも操作できないと困る。#766）。
/// 明示的に `Running` のときだけ止めるのは、送達フローが取りこぼし時に Ctrl+C を
/// 打つため（実行中のプロセスを落としてしまう）。
///
/// `is_alt_screen` に渡すのは**ペインの中で動いているプログラム**の状態。
/// バックエンド（tmux）ペインの**外側**のフラグを渡してはいけない: tmux クライアント
/// 自身が alt screen へ入るので、中身が素のシェルでも常に true になり、
/// persist が有効な環境（= 既定）の全ペインが対象外になる
/// （#694 が同じ罠を踏んでいる。実測は #1006 の隔離セルフテスト:
/// 素のシェルのバックエンドペインで outer_alt=true / inner_alt=false）。
/// 呼び出し側は `pane_inner_alt_screen` 相当（器つきなら false）を渡す
pub fn can_ssh_pane(
    has_session: bool,
    is_alt_screen: bool,
    command_state: crate::terminal::CommandState,
    role: Option<&str>,
) -> Result<(), PaneSshBlock> {
    if !has_session {
        return Err(PaneSshBlock::NoSession);
    }
    if role.is_some_and(|r| !r.trim().is_empty()) {
        return Err(PaneSshBlock::AgentRole);
    }
    if is_alt_screen {
        return Err(PaneSshBlock::AltScreen);
    }
    if matches!(command_state, crate::terminal::CommandState::Running) {
        return Err(PaneSshBlock::Running);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::terminal::CommandState;

    #[test]
    fn 既定の開き先は現在タブへの新ペイン() {
        // #1006 の要望「原則開いてるタブに新ペインで出す」。従来の既定（Tab）から変えた
        assert_eq!(RemoteOpenTarget::default(), RemoteOpenTarget::Split);
    }

    #[test]
    fn 語彙は往復する() {
        for v in RemoteOpenTarget::VALUES {
            let parsed = RemoteOpenTarget::parse(v).expect("VALUES は必ず解釈できる");
            assert_eq!(parsed.as_str(), v);
        }
        // 大文字・前後空白も受ける（CLI の手打ち）
        assert_eq!(
            RemoteOpenTarget::parse(" Pane "),
            Some(RemoteOpenTarget::Pane)
        );
        assert_eq!(RemoteOpenTarget::parse("window"), None);
        assert_eq!(RemoteOpenTarget::values_hint(), "split | tab | pane");
    }

    #[test]
    fn ワイヤ表記は語彙と一致する() {
        // 応答 JSON と CLI の possible values がずれると「見えている語で操作できない」
        let json = serde_json::to_string(&RemoteOpenTarget::Split).unwrap();
        assert_eq!(json, "\"split\"");
        let parsed: RemoteOpenTarget = serde_json::from_str("\"pane\"").unwrap();
        assert_eq!(parsed, RemoteOpenTarget::Pane);
    }

    #[test]
    fn 素のシェルは_ssh_化できる() {
        assert_eq!(
            can_ssh_pane(true, false, CommandState::Idle, None),
            Ok(()),
            "プロンプト待ちのシェルは対象"
        );
        assert_eq!(
            can_ssh_pane(true, false, CommandState::Unknown, None),
            Ok(()),
            "シェル統合が効かない構成（#766 の器）でも操作できる必要がある"
        );
        assert_eq!(
            can_ssh_pane(true, false, CommandState::Failed(1), None),
            Ok(()),
            "直前のコマンドが失敗して止まっているだけなら対象"
        );
    }

    #[test]
    fn 素のシェルでないペインは理由つきで断る() {
        assert_eq!(
            can_ssh_pane(false, false, CommandState::Idle, None),
            Err(PaneSshBlock::NoSession)
        );
        assert_eq!(
            can_ssh_pane(true, true, CommandState::Idle, None),
            Err(PaneSshBlock::AltScreen)
        );
        assert_eq!(
            can_ssh_pane(true, false, CommandState::Running, None),
            Err(PaneSshBlock::Running)
        );
        assert_eq!(
            can_ssh_pane(
                true,
                false,
                CommandState::Idle,
                Some("orchestrator-worker:1")
            ),
            Err(PaneSshBlock::AgentRole)
        );
        // 空文字の role は「無い」と同じ扱い（layout.json の古い値対策）
        assert_eq!(
            can_ssh_pane(true, false, CommandState::Idle, Some("")),
            Ok(())
        );
    }

    #[test]
    fn 断る理由には次の一手が入る() {
        for block in [
            PaneSshBlock::NoSession,
            PaneSshBlock::AltScreen,
            PaneSshBlock::Running,
            PaneSshBlock::AgentRole,
        ] {
            let msg = block.message(7);
            assert!(msg.contains("pane 7"), "対象ペインを名指しする: {msg}");
            assert!(
                msg.contains("target=split"),
                "回避策（新ペインで開く）を必ず添える: {msg}"
            );
        }
    }
}
