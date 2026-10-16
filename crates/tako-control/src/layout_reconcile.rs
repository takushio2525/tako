//! layout と器（tmux セッション）の突き合わせ（Issue #1858）。
//!
//! 2026-09-30 の本番の復旧（#1857）で踏んだ 2 つを直す判断の正本:
//!
//! 1. **recover の合流**（[`merge_after_backup`]）: `tako recover --apply <世代>` は世代の
//!    ファイルを layout.json へ写すだけだったので、バックアップより後に作られたタブ
//!    （復旧をしている master 自身のタブを含む）が消えた。器は生きているので起動時の
//!    orphan 自動復帰（#191）が「復帰」タブへ**別の ID・別のタブ**として拾うが、
//!    タブ名・分割・役割・会話の記録は戻らない。現在の layout.json にあって世代に無い
//!    タブを、どのペインも刈り込む側（下の [`prunable`]）のタブ以外は世代の後ろへ合流させる
//! 2. **復元時の刈り込み**（[`prune_dead_backends`]）: 器が無いペインは、復元で
//!    新しいシェル / resume として開き直されて木に残った（完了済みの worker 3 枚を手で
//!    外した）
//!
//! # 刈り込むのは master / worker のペインだけ
//!
//! 器が無いペインには、器の状態だけでは見分けられない 2 種類がある:
//!
//! - **master が片付けたペイン**（完了した worker・引き継ぎで閉じた前任の master）。
//!   古い世代へ戻すと木へ戻ってくるが、開き直すと役割つきの会話が resume されて
//!   **幽霊の worker / master** になる（2026-09-30 の 3 枚はこれ）
//! - **利用者が閉じすぎたペイン**。`tako recover` は「閉じすぎた構成を戻す」手段で、
//!   閉じたときに終わったプロセスは同じ作業フォルダの新しいシェルで開き直すと
//!   約束している（FR-5.2 / docs の SceneRecover。#770 / #2063）
//!
//! どちらも「閉じたときに tako が器を kill した」ので器の生死では区別できず、違いは
//! 誰がそのペインを持っていたか = 役割にしか現れない。だから刈り込むのは
//! **器が消えたと確かめられた master / worker のペイン**（[`prunable`]）だけで、
//! それ以外（役割なし・solo）は従来どおり開き直す
//!
//! # 消すのは「消えたと確かめられた器」だけ
//!
//! 判断を誤ると本物のペインを消すので、器の生死を言い切れるのは次の 2 つが揃うときだけ
//! （[`TrustedLive::new`]）:
//!
//! - tmux がセッション一覧を返した（`tako_core::tmux::live_session_names` が `Some`）。
//!   サーバー未起動・ソケットが無い・tmux 不在・上限切れは**不明**で、何も消さない
//!   （PC の再起動で器ごと消えたときの resume / 新規シェル（#1076）はこちらに入る）
//! - その一覧に layout の器が 1 つ以上居る = **layout を書いたときと同じサーバー**。
//!   別の何かが同じ名前のソケットでサーバーを立て直した直後は layout の器が 1 つも
//!   居ないので、不明として扱う（全部を「消えた」と読んで全部を刈り込まない）
//!
//! 刈り込むのは表のタブの葉だけ。退避（たまり場・退避タブ）は `crate::shelved_restore` の
//! 正本に任せる（#1576）。プレビュー・Web ビュー・器を持たない直接のペインは器の生死と
//! 関係が無いので触らない。**表のタブがすべて空になる刈り込みはしない**
//! （タブ 0 枚の layout は `LayoutError::Empty` で復元ごと失敗し、layout.json が
//! `.corrupt` へ退避される）
//!
//! # 診断へ載せるもの
//!
//! ペイン ID・タブ ID・器の名前（`tako-<hex12>` の乱数）・役割・会話の記録の系統名だけ。
//! タブ名・ペイン内容・会話 ID は persist.log へ載せない（`.agent/conventions.md` の
//! 診断ログ規約）。タブ名は利用者へ見せる `tako recover` の出力にだけ出す

use std::collections::HashSet;

use serde::Serialize;

use crate::layout::{LayoutFile, NodeLayout, PaneLayout, TabLayout};

/// #1858 の A/B。`TAKO_1858_LEGACY=1` で修正前の挙動
/// （recover は世代をそのまま写す・復元は器の無いペインも開き直す）
pub fn legacy() -> bool {
    std::env::var_os("TAKO_1858_LEGACY").is_some()
}

/// 器の生死を言い切れなかった理由
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Untrusted {
    /// tmux が一覧を返さなかった（サーバー未起動・ソケットが無い・tmux 不在・上限切れ）
    NoAnswer,
    /// 一覧は返ったが layout の器が 1 つも居ない（layout を書いたのと別のサーバーかもしれない）
    NoLayoutSession,
}

impl Untrusted {
    /// 診断・CLI 出力用の日本語ラベル
    pub const fn label(self) -> &'static str {
        match self {
            Self::NoAnswer => "tmux に問い合わせられない（サーバー未起動・ソケットが無い等）",
            Self::NoLayoutSession => {
                "tmux の一覧に layout の器が 1 つも無い（layout を書いたのと別のサーバーかもしれない）"
            }
        }
    }
}

/// 器の生死を言い切れる一覧（[モジュール doc](self) の 2 条件を満たしたもの）
#[derive(Debug, Clone)]
pub struct TrustedLive(HashSet<String>);

impl TrustedLive {
    /// `live` = `tako_core::tmux::live_session_names` の答え、`layout_sessions` = 突き合わせる
    /// layout が持つ器の名前。どちらかの条件を欠けば理由を返す
    pub fn new<'a>(
        live: Option<HashSet<String>>,
        layout_sessions: impl IntoIterator<Item = &'a str>,
    ) -> Result<Self, Untrusted> {
        let live = live.ok_or(Untrusted::NoAnswer)?;
        if !layout_sessions.into_iter().any(|s| live.contains(s)) {
            return Err(Untrusted::NoLayoutSession);
        }
        Ok(Self(live))
    }

    /// その器が生きているか
    pub fn is_alive(&self, session: &str) -> bool {
        self.0.contains(session)
    }
}

/// ペインの器の名前。プレビュー・Web ビューは器の生死と関係が無いので `None`
fn backend_of(pane: &PaneLayout) -> Option<&str> {
    if pane.preview.is_some() || pane.webview.is_some() {
        return None;
    }
    pane.session.as_deref()
}

/// 器が消えたと**確かめられた**ペインか（不明・器なしは偽）
fn confirmed_dead(pane: &PaneLayout, live: Option<&TrustedLive>) -> bool {
    match (backend_of(pane), live) {
        (Some(session), Some(live)) => !live.is_alive(session),
        _ => false,
    }
}

/// master が持つペインか（master / worker の役割。片付けるのは master）
fn orchestrator_owned(pane: &PaneLayout) -> bool {
    pane.role.as_deref().is_some_and(|r| {
        tako_core::workspace::is_master_role(r) || tako_core::workspace::is_worker_role(r)
    })
}

/// 刈り込むペインか = 器が消えたと確かめられた master / worker のペイン
/// （[モジュール doc](self) の「刈り込むのは master / worker のペインだけ」）
pub fn prunable(pane: &PaneLayout, live: Option<&TrustedLive>) -> bool {
    confirmed_dead(pane, live) && orchestrator_owned(pane)
}

/// 木の葉（左から順）
fn leaves(node: &NodeLayout) -> Vec<&PaneLayout> {
    match node {
        NodeLayout::Pane(p) => vec![p],
        NodeLayout::Split { first, second, .. } => {
            let mut out = leaves(first);
            out.extend(leaves(second));
            out
        }
    }
}

/// 木から `drop` が真の葉を外す。片側が空になった分割は残った側へ畳む
/// （残った側が分割の場所を丸ごと使う）。全部外れたら `None`
fn without(node: &NodeLayout, drop: &dyn Fn(&PaneLayout) -> bool) -> Option<NodeLayout> {
    match node {
        NodeLayout::Pane(p) => (!drop(p)).then(|| node.clone()),
        NodeLayout::Split {
            axis,
            ratio,
            first,
            second,
        } => match (without(first, drop), without(second, drop)) {
            (Some(a), Some(b)) => Some(NodeLayout::Split {
                axis: axis.clone(),
                ratio: *ratio,
                first: Box::new(a),
                second: Box::new(b),
            }),
            (Some(rest), None) | (None, Some(rest)) => Some(rest),
            (None, None) => None,
        },
    }
}

/// タブから `drop` が真の葉を外す。フォーカスが外れたら残った先頭の葉へ移す
fn retain_tab(tab: &TabLayout, drop: &dyn Fn(&PaneLayout) -> bool) -> Option<TabLayout> {
    let tree = without(&tab.tree, drop)?;
    let kept = leaves(&tree);
    let focused = if kept.iter().any(|p| p.id == tab.focused) {
        tab.focused
    } else {
        kept[0].id
    };
    Some(TabLayout {
        tree,
        focused,
        ..tab.clone()
    })
}

/// layout の全ペイン ID（表・たまり場・退避タブ）
fn pane_ids(file: &LayoutFile) -> HashSet<u64> {
    file.tabs
        .iter()
        .map(|t| &t.tree)
        .chain(file.shelved_tabs.iter().map(|t| &t.tab.tree))
        .flat_map(leaves)
        .map(|p| p.id)
        .chain(file.backgrounded.iter().map(|p| p.id))
        .collect()
}

// ---------------------------------------------------------------------------
// 1. recover の合流
// ---------------------------------------------------------------------------

/// 合流したタブ 1 枚
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct MergedTab {
    pub id: u64,
    /// タブ名（CLI の出力にだけ出す。persist.log には載せない）
    pub title: String,
    /// 合流したペイン数
    pub panes: usize,
    /// そのうち器が生きていたペイン数
    pub alive: usize,
    /// そのうち器が消えていたペイン数（起動時に master / worker は外し、それ以外は開き直す）
    pub dead: usize,
    /// 世代に同じペイン / 器があったので外したペイン数（同じ ID を 2 か所に置かない）
    pub duplicates: usize,
}

/// 合流しなかった理由
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SkipReason {
    /// どのペインも刈り込む側（器が消えた master / worker のペイン）だった
    AllDead,
    /// どのペインも世代の側にある（世代の後で別のタブへ移しただけ）
    AlreadyInBackup,
}

impl SkipReason {
    /// CLI 出力・診断用の日本語ラベル
    pub const fn label(self) -> &'static str {
        match self {
            Self::AllDead => "master / worker のペインで、tmux セッションがすべて終わっている",
            Self::AlreadyInBackup => "どのペインもバックアップの側にある",
        }
    }
}

/// 合流しなかったタブ 1 枚
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SkippedTab {
    pub id: u64,
    /// タブ名（CLI の出力にだけ出す）
    pub title: String,
    pub reason: SkipReason,
}

/// 合流の結果
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct MergeReport {
    pub merged: Vec<MergedTab>,
    pub skipped: Vec<SkippedTab>,
    /// 器の生死を言い切れなかった理由（`None` = 言い切れた）。言い切れないときは
    /// 器が消えたとは扱わず、世代の後のタブをすべて合流させる（落とさない側へ倒す）
    pub untrusted: Option<Untrusted>,
}

impl MergeReport {
    /// persist.log の 1 行（合流も見送りも無ければ `None`）。タブ名は載せない
    pub fn log_line(&self, generation: &str) -> Option<String> {
        if self.merged.is_empty() && self.skipped.is_empty() {
            return None;
        }
        let merged: Vec<String> = self
            .merged
            .iter()
            .map(|t| {
                format!(
                    "タブ {}（{} ペイン・器あり {}・器なし {}）",
                    t.id, t.panes, t.alive, t.dead
                )
            })
            .collect();
        let skipped: Vec<String> = self
            .skipped
            .iter()
            .map(|t| format!("タブ {}（{}）", t.id, t.reason.label()))
            .collect();
        Some(format!(
            "recover --apply {generation}: バックアップより後のタブを合流 {} 枚{}・合流しない {} 枚{}{}（#1858）",
            self.merged.len(),
            bracket(&merged),
            self.skipped.len(),
            bracket(&skipped),
            self.untrusted
                .map(|u| format!("・器の生死は確かめていない: {}", u.label()))
                .unwrap_or_default(),
        ))
    }
}

fn bracket(items: &[String]) -> String {
    if items.is_empty() {
        String::new()
    } else {
        format!("[{}]", items.join(" / "))
    }
}

/// 世代 `backup` へ戻すとき、現在の layout（`current`）にあって世代に無いタブを合流させる。
///
/// - 世代に無い = タブ ID が世代の表のタブ・退避タブのどちらにも無い
/// - 世代に同じペイン ID・同じ器があるペインは外す（世代の後で別のタブへ移したペイン。
///   同じ ID を 2 か所に置くと復元が `DuplicateId` で丸ごと失敗する）
/// - どのペインも刈り込む側（[`prunable`]）のタブは合流させない（起動時に外れるだけ）。
///   生死を言い切れないとき・利用者のペインは合流させる（落とさない側へ倒す）
/// - 合流したタブは世代の末尾へ。複数ウィンドウなら元のウィンドウが世代にもあれば
///   そこへ、無ければ割当なし（復元が先頭ウィンドウへ置く）
///
/// 退避（たまり場・退避タブ）の差分は扱わない（器が生きていれば orphan 自動復帰が拾う）
pub fn merge_after_backup(
    backup: &LayoutFile,
    current: &LayoutFile,
    live: Option<HashSet<String>>,
) -> (LayoutFile, MergeReport) {
    let layout_sessions: Vec<&str> = backup
        .sessions()
        .into_iter()
        .chain(current.sessions())
        .collect();
    let trusted = TrustedLive::new(live, layout_sessions.iter().copied());
    let live = trusted.as_ref().ok();

    let known_tabs: HashSet<u64> = backup
        .tabs
        .iter()
        .map(|t| t.id)
        .chain(backup.shelved_tabs.iter().map(|t| t.tab.id))
        .collect();
    let known_panes = pane_ids(backup);
    let known_sessions: HashSet<&str> = backup.sessions().into_iter().collect();

    let mut merged_file = backup.clone();
    let mut report = MergeReport {
        merged: Vec::new(),
        skipped: Vec::new(),
        untrusted: trusted.as_ref().err().copied(),
    };
    for tab in current.tabs.iter().filter(|t| !known_tabs.contains(&t.id)) {
        let duplicate = |p: &PaneLayout| {
            known_panes.contains(&p.id)
                || p.session
                    .as_deref()
                    .is_some_and(|s| known_sessions.contains(s))
        };
        let skip = |reason| SkippedTab {
            id: tab.id,
            title: tab.title.clone(),
            reason,
        };
        let Some(kept) = retain_tab(tab, &duplicate) else {
            report.skipped.push(skip(SkipReason::AlreadyInBackup));
            continue;
        };
        let panes = leaves(&kept.tree);
        if panes.iter().all(|p| prunable(p, live)) {
            report.skipped.push(skip(SkipReason::AllDead));
            continue;
        }
        let dead = panes.iter().filter(|p| confirmed_dead(p, live)).count();
        let alive = panes
            .iter()
            .filter(|p| backend_of(p).is_some_and(|s| live.is_some_and(|l| l.is_alive(s))))
            .count();
        report.merged.push(MergedTab {
            id: tab.id,
            title: tab.title.clone(),
            panes: panes.len(),
            alive,
            dead,
            duplicates: leaves(&tab.tree).len() - panes.len(),
        });
        // 元のウィンドウが世代にもあればそこへ（無ければ割当なし = 先頭ウィンドウ）
        if let Some(origin) = current.windows.iter().find(|w| w.tabs.contains(&tab.id)) {
            if let Some(w) = merged_file.windows.iter_mut().find(|w| w.id == origin.id) {
                w.tabs.push(tab.id);
            }
        }
        if current.collapsed.contains(&tab.id) {
            merged_file.collapsed.push(tab.id);
        }
        merged_file.tabs.push(kept);
    }
    (merged_file, report)
}

// ---------------------------------------------------------------------------
// 2. 復元時の刈り込み
// ---------------------------------------------------------------------------

/// 刈り込んだペイン 1 枚（`tako persist` / MCP `tako_persist` の `restore_pruned`）
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PrunedPane {
    pub pane: u64,
    /// 居たタブ
    pub tab: u64,
    /// 消えていた器の名前
    pub session: String,
    /// 役割（worker / master 等）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    /// 会話の記録があった系統（claude / codex / agy）。`tako sessions list` から戻せる
    #[serde(skip_serializing_if = "Option::is_none")]
    pub conversation: Option<String>,
}

impl PrunedPane {
    /// persist.log の 1 行（会話 ID・タブ名・ペイン内容は載せない）
    pub fn log_line(&self) -> String {
        let mut notes = Vec::new();
        if let Some(role) = &self.role {
            notes.push(format!("役割 {role}"));
        }
        if let Some(agent) = &self.conversation {
            notes.push(format!(
                "会話の記録 {agent} = `tako sessions list` から戻せる"
            ));
        }
        format!(
            "刈り込み（ペイン {}・タブ {}）: tmux セッション {} が無い{}",
            self.pane,
            self.tab,
            self.session,
            if notes.is_empty() {
                String::new()
            } else {
                format!("（{}）", notes.join("・"))
            }
        )
    }
}

/// 刈り込みを見送った理由
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PruneHeld {
    /// 器の生死を言い切れない
    Untrusted(Untrusted),
    /// 刈り込むと表のタブがすべて空になる（`candidates` = 器が消えていたペイン数）
    WouldEmptyAllTabs { candidates: usize },
}

/// 刈り込みの結果
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct PruneReport {
    pub pruned: Vec<PrunedPane>,
    /// 葉がすべて外れて閉じたタブ
    pub closed_tabs: Vec<u64>,
    /// 器は消えていたが master / worker ではないので外さず開き直すペイン
    /// （「閉じすぎた構成を戻す」用途。[モジュール doc](self)）
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub reopened: Vec<u64>,
    /// 見送った理由（`None` = 判断した。刈り込みが 0 件でも `None`）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub held: Option<PruneHeld>,
}

impl PruneReport {
    /// 外さずに開き直すペインの 1 行（persist.log。無ければ `None`）
    pub fn reopened_line(&self) -> Option<String> {
        if self.reopened.is_empty() {
            return None;
        }
        let ids: Vec<String> = self
            .reopened
            .iter()
            .map(|p| format!("ペイン {p}"))
            .collect();
        Some(format!(
            "tmux セッションが無いが外さずに開き直す: {}（master / worker ではない = \
             閉じた構成を戻す用途を潰さない。#1858）",
            ids.join(" / ")
        ))
    }

    /// 見送りの 1 行（persist.log）。刈り込んだ分は [`PrunedPane::log_line`] と
    /// 復元の内訳（`crate::restore_report`）が出す
    pub fn held_line(&self) -> Option<String> {
        match self.held? {
            PruneHeld::Untrusted(u) => Some(format!(
                "刈り込みを見送った: {}（器の無いペインも開き直す。#1858）",
                u.label()
            )),
            PruneHeld::WouldEmptyAllTabs { candidates } => Some(format!(
                "刈り込みを見送った: tmux セッションが無い master / worker のペイン \
                 {candidates} 枚を外すと表のタブがすべて空になる（開き直す。#1858）"
            )),
        }
    }
}

/// 復元の前に、刈り込むペイン（[`prunable`]）を表のタブから外す（[モジュール doc](self)）。
/// 葉がすべて外れたタブは閉じ、アクティブタブ・ウィンドウの割当・折りたたみから外す。
/// A/B（[`legacy`]）は env から読む
pub fn prune_dead_backends(file: &mut LayoutFile, live: Option<HashSet<String>>) -> PruneReport {
    if legacy() {
        return PruneReport::default();
    }
    prune_dead_backends_with(file, live)
}

/// [`prune_dead_backends`] の本体（単体テストは env を触らずにこちらを使う）
pub fn prune_dead_backends_with(
    file: &mut LayoutFile,
    live: Option<HashSet<String>>,
) -> PruneReport {
    // 刈り込みうるペイン（器を持つ master / worker）が無ければ判断することが無い。
    // 見送りの行も出さない（worker を使わない人の persist.log に毎回の雑音を足さない）
    let has_candidates = file
        .tabs
        .iter()
        .flat_map(|t| leaves(&t.tree))
        .any(|p| backend_of(p).is_some() && orchestrator_owned(p));
    let live = match TrustedLive::new(live, file.sessions()) {
        Ok(_) if !has_candidates => return PruneReport::default(),
        Ok(live) => live,
        Err(u) => {
            return PruneReport {
                held: has_candidates.then_some(PruneHeld::Untrusted(u)),
                ..PruneReport::default()
            }
        }
    };
    let dead = |p: &PaneLayout| prunable(p, Some(&live));
    let mut pruned = Vec::new();
    let mut closed_tabs = Vec::new();
    let mut tabs = Vec::new();
    let reopened: Vec<u64> = file
        .tabs
        .iter()
        .flat_map(|t| leaves(&t.tree))
        .filter(|p| confirmed_dead(p, Some(&live)) && !orchestrator_owned(p))
        .map(|p| p.id)
        .collect();
    for tab in &file.tabs {
        for p in leaves(&tab.tree).into_iter().filter(|p| dead(p)) {
            pruned.push(PrunedPane {
                pane: p.id,
                tab: tab.id,
                session: p.session.clone().unwrap_or_default(),
                role: p.role.clone(),
                conversation: if p.claude_session_id.is_some() {
                    Some(tako_core::agent_support::Agent::Claude.as_str().to_string())
                } else {
                    p.agent_resume
                        .as_ref()
                        .filter(|a| a.id.is_some())
                        .map(|a| a.agent.clone())
                },
            });
        }
        match retain_tab(tab, &dead) {
            Some(kept) => tabs.push(kept),
            None => closed_tabs.push(tab.id),
        }
    }
    if pruned.is_empty() {
        return PruneReport {
            reopened,
            ..PruneReport::default()
        };
    }
    if tabs.is_empty() {
        return PruneReport {
            held: Some(PruneHeld::WouldEmptyAllTabs {
                candidates: pruned.len(),
            }),
            reopened,
            ..PruneReport::default()
        };
    }
    file.tabs = tabs;
    if closed_tabs.contains(&file.active_tab) {
        file.active_tab = file.tabs[0].id;
    }
    for w in &mut file.windows {
        w.tabs.retain(|t| !closed_tabs.contains(t));
        if closed_tabs.contains(&w.active_tab) {
            if let Some(first) = w.tabs.first() {
                w.active_tab = *first;
            }
        }
    }
    file.collapsed.retain(|t| !closed_tabs.contains(t));
    PruneReport {
        pruned,
        closed_tabs,
        reopened,
        held: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::{PreviewLayout, ShelvedTabLayout, WindowLayout, LAYOUT_VERSION};

    fn pane(id: u64, session: Option<&str>) -> PaneLayout {
        PaneLayout {
            id,
            session: session.map(String::from),
            title: None,
            title_source: "default".into(),
            role: None,
            origin: "user".into(),
            cwd: None,
            claude_session_id: None,
            agent_resume: None,
            logged_history: None,
            preview: None,
            webview: None,
            origin_tab: None,
            origin_tab_title: None,
            limit_autoresume: false,
            ssh: None,
            spawned_by: None,
        }
    }

    /// master が spawn した worker のペイン
    fn worker(id: u64, session: Option<&str>) -> PaneLayout {
        PaneLayout {
            role: Some("orchestrator-worker:tako".into()),
            ..pane(id, session)
        }
    }

    fn leaf(p: PaneLayout) -> NodeLayout {
        NodeLayout::Pane(Box::new(p))
    }

    fn split(a: NodeLayout, b: NodeLayout) -> NodeLayout {
        NodeLayout::Split {
            axis: "x".into(),
            ratio: 0.5,
            first: Box::new(a),
            second: Box::new(b),
        }
    }

    fn tab(id: u64, title: &str, tree: NodeLayout) -> TabLayout {
        let focused = leaves(&tree)[0].id;
        TabLayout {
            id,
            title: title.into(),
            title_source: "manual".into(),
            focused,
            tree,
            pinned_folders: Vec::new(),
            remote_folders: Vec::new(),
        }
    }

    fn file(tabs: Vec<TabLayout>) -> LayoutFile {
        LayoutFile {
            version: LAYOUT_VERSION,
            active_tab: tabs[0].id,
            tabs,
            window: None,
            backgrounded: Vec::new(),
            collapsed: Vec::new(),
            webview_dock: Vec::new(),
            windows: Vec::new(),
            shelved_tabs: Vec::new(),
        }
    }

    fn live(names: &[&str]) -> Option<HashSet<String>> {
        Some(names.iter().map(|s| s.to_string()).collect())
    }

    fn tab_ids(f: &LayoutFile) -> Vec<u64> {
        f.tabs.iter().map(|t| t.id).collect()
    }

    // --- 器の生死を言い切れる条件 ---

    #[test]
    fn 一覧が無ければ言い切らない() {
        assert_eq!(
            TrustedLive::new(None, ["tako-a"]).err(),
            Some(Untrusted::NoAnswer)
        );
    }

    #[test]
    fn layoutの器が1つも居ない一覧では言い切らない() {
        // 同じ名前のソケットで別のサーバーが立った直後 = 全部を「消えた」と読まない
        assert_eq!(
            TrustedLive::new(live(&["tako-other"]), ["tako-a", "tako-b"]).err(),
            Some(Untrusted::NoLayoutSession)
        );
        assert_eq!(
            TrustedLive::new(live(&[]), ["tako-a"]).err(),
            Some(Untrusted::NoLayoutSession)
        );
    }

    // --- 2. 復元時の刈り込み ---

    #[test]
    fn 器の消えたペインを刈り込み_空になったタブを閉じる() {
        let mut f = file(vec![
            tab(
                1,
                "master",
                split(
                    leaf(pane(10, Some("tako-alive"))),
                    leaf(worker(11, Some("tako-gone1"))),
                ),
            ),
            tab(2, "worker", leaf(worker(20, Some("tako-gone2")))),
        ]);
        f.active_tab = 2;
        f.collapsed = vec![2];
        f.windows = vec![WindowLayout {
            id: 1,
            tabs: vec![1, 2],
            active_tab: 2,
            frame: None,
        }];
        let report = prune_dead_backends_with(&mut f, live(&["tako-alive"]));
        assert_eq!(report.held, None);
        assert_eq!(
            report.pruned.iter().map(|p| p.pane).collect::<Vec<_>>(),
            vec![11, 20]
        );
        assert_eq!(report.closed_tabs, vec![2]);
        assert_eq!(tab_ids(&f), vec![1]);
        // 分割は残った側へ畳まれる
        assert!(matches!(&f.tabs[0].tree, NodeLayout::Pane(p) if p.id == 10));
        assert_eq!(f.active_tab, 1);
        assert_eq!(f.windows[0].tabs, vec![1]);
        assert_eq!(f.windows[0].active_tab, 1);
        assert!(f.collapsed.is_empty());
        // 刈り込んだ後の layout は復元できる
        let (_, restored) = crate::layout::restore(&f).expect("刈り込み後も復元できる");
        assert_eq!(restored.len(), 1);
    }

    #[test]
    fn フォーカスが外れたら残った先頭の葉へ移す() {
        let mut t = tab(
            1,
            "t",
            split(
                leaf(pane(10, Some("tako-alive"))),
                split(
                    leaf(worker(11, Some("tako-gone"))),
                    leaf(pane(12, Some("tako-alive2"))),
                ),
            ),
        );
        t.focused = 11;
        let mut f = file(vec![t]);
        prune_dead_backends_with(&mut f, live(&["tako-alive", "tako-alive2"]));
        assert_eq!(f.tabs[0].focused, 10);
        assert_eq!(
            leaves(&f.tabs[0].tree)
                .iter()
                .map(|p| p.id)
                .collect::<Vec<_>>(),
            vec![10, 12]
        );
    }

    #[test]
    fn 問い合わせられなければ刈り込まない() {
        let mut f = file(vec![tab(1, "t", leaf(worker(10, Some("tako-gone"))))]);
        let before = f.clone();
        let report = prune_dead_backends_with(&mut f, None);
        assert_eq!(report.held, Some(PruneHeld::Untrusted(Untrusted::NoAnswer)));
        assert!(report.pruned.is_empty());
        assert_eq!(f, before);
        assert!(report.held_line().unwrap().contains("問い合わせられない"));
    }

    #[test]
    fn 刈り込みうるペインが無ければ見送りも言わない() {
        // worker を使わない人の起動（PC の再起動後を含む）に毎回の雑音を足さない
        let mut f = file(vec![tab(1, "t", leaf(pane(10, Some("tako-gone"))))]);
        let report = prune_dead_backends_with(&mut f, None);
        assert_eq!(report, PruneReport::default());
        assert_eq!(report.held_line(), None);
    }

    #[test]
    fn layoutの器が1つも居なければ刈り込まない() {
        // PC の再起動後に別の何かがサーバーを立てた等。全ペインを消さない
        let mut f = file(vec![
            tab(1, "a", leaf(worker(10, Some("tako-a")))),
            tab(2, "b", leaf(worker(20, Some("tako-b")))),
        ]);
        let before = f.clone();
        let report = prune_dead_backends_with(&mut f, live(&["tako-unrelated"]));
        assert_eq!(
            report.held,
            Some(PruneHeld::Untrusted(Untrusted::NoLayoutSession))
        );
        assert_eq!(f, before);
    }

    #[test]
    fn 表のタブがすべて空になるなら刈り込まない() {
        // 生きている器が退避の側にしか無い
        let mut f = file(vec![tab(1, "t", leaf(worker(10, Some("tako-gone"))))]);
        f.backgrounded = vec![pane(30, Some("tako-shelved"))];
        let before = f.clone();
        let report = prune_dead_backends_with(&mut f, live(&["tako-shelved"]));
        assert_eq!(
            report.held,
            Some(PruneHeld::WouldEmptyAllTabs { candidates: 1 })
        );
        assert_eq!(f, before);
    }

    #[test]
    fn 器を持たないペインと退避は触らない() {
        let mut preview = pane(11, None);
        preview.preview = Some(PreviewLayout {
            path: "/tmp/a.md".into(),
            mode: "markdown".into(),
        });
        let mut webview = pane(12, None);
        webview.webview = Some("https://example.com".into());
        let direct = pane(13, None);
        let mut f = file(vec![tab(
            1,
            "t",
            split(
                leaf(pane(10, Some("tako-alive"))),
                split(leaf(preview), split(leaf(webview), leaf(direct))),
            ),
        )]);
        f.backgrounded = vec![worker(30, Some("tako-gone-bg"))];
        f.shelved_tabs = vec![ShelvedTabLayout {
            tab: tab(5, "shelved", leaf(worker(50, Some("tako-gone-shelved")))),
            origin_window: 1,
            origin_index: 0,
        }];
        let before = f.clone();
        let report = prune_dead_backends_with(&mut f, live(&["tako-alive"]));
        assert!(report.pruned.is_empty(), "{report:?}");
        assert_eq!(f, before);
    }

    /// 利用者のペイン（役割なし・solo）は器が消えていても外さず開き直す
    /// （`tako recover` で閉じすぎた構成を戻す用途。#770 / #2063）。master も外す側
    #[test]
    fn 刈り込むのはmasterとworkerのペインだけ() {
        let mut solo = pane(12, Some("tako-gone-solo"));
        solo.role = Some("solo".into());
        let mut master = pane(13, Some("tako-gone-master"));
        master.role = Some("orchestrator-master:takodev".into());
        let mut f = file(vec![
            tab(
                1,
                "t",
                split(
                    leaf(pane(10, Some("tako-alive"))),
                    split(leaf(pane(11, Some("tako-gone-user"))), leaf(solo)),
                ),
            ),
            tab(2, "m", leaf(master)),
        ]);
        let report = prune_dead_backends_with(&mut f, live(&["tako-alive"]));
        assert_eq!(
            report.pruned.iter().map(|p| p.pane).collect::<Vec<_>>(),
            vec![13]
        );
        assert_eq!(report.closed_tabs, vec![2]);
        assert_eq!(report.reopened, vec![11, 12]);
        assert_eq!(
            leaves(&f.tabs[0].tree)
                .iter()
                .map(|p| p.id)
                .collect::<Vec<_>>(),
            vec![10, 11, 12]
        );
        let line = report.reopened_line().unwrap();
        assert!(line.contains("ペイン 11 / ペイン 12"), "{line}");
    }

    #[test]
    fn 刈り込みの行は会話の系統だけを載せる() {
        let mut p = worker(11, Some("tako-gone"));
        p.role = Some("worker:tako".into());
        p.claude_session_id = Some("0b5c-secret-like-id".into());
        let mut f = file(vec![
            tab(1, "t", leaf(pane(10, Some("tako-alive")))),
            tab(2, "私のタブ", leaf(p)),
        ]);
        let report = prune_dead_backends_with(&mut f, live(&["tako-alive"]));
        let line = report.pruned[0].log_line();
        assert!(line.contains("ペイン 11"), "{line}");
        assert!(line.contains("tako-gone"), "{line}");
        assert!(line.contains("worker:tako"), "{line}");
        assert!(line.contains("claude"), "{line}");
        assert!(
            !line.contains("0b5c-secret-like-id"),
            "会話 ID を載せない: {line}"
        );
        assert!(!line.contains("私のタブ"), "タブ名を載せない: {line}");
    }

    // --- 1. recover の合流 ---

    fn backup_and_current() -> (LayoutFile, LayoutFile) {
        let backup = file(vec![
            tab(1, "base", leaf(pane(10, Some("tako-base")))),
            tab(2, "old-worker", leaf(pane(20, Some("tako-old")))),
        ]);
        // 世代の後: タブ 2 を閉じ、タブ 3（master 自身）と 4（完了済み worker）を作った
        let current = file(vec![
            tab(1, "base", leaf(pane(10, Some("tako-base")))),
            tab(3, "司令塔", leaf(pane(30, Some("tako-master")))),
            tab(4, "done", leaf(worker(40, Some("tako-done")))),
        ]);
        (backup, current)
    }

    #[test]
    fn 世代の後に作られた生きているタブを合流させる() {
        let (backup, current) = backup_and_current();
        let (merged, report) = merge_after_backup(
            &backup,
            &current,
            live(&["tako-base", "tako-old", "tako-master"]),
        );
        assert_eq!(tab_ids(&merged), vec![1, 2, 3]);
        assert_eq!(report.merged.len(), 1);
        assert_eq!(report.merged[0].id, 3);
        assert_eq!(report.merged[0].alive, 1);
        assert_eq!(
            report.skipped,
            vec![SkippedTab {
                id: 4,
                title: "done".into(),
                reason: SkipReason::AllDead
            }]
        );
        assert_eq!(report.untrusted, None);
        // 世代の中身はそのまま（アクティブタブも世代のもの）
        assert_eq!(merged.tabs[..2], backup.tabs[..]);
        assert_eq!(merged.active_tab, backup.active_tab);
        crate::layout::restore(&merged).expect("合流後も復元できる");
        let line = report.log_line("1").unwrap();
        assert!(line.contains("合流 1 枚[タブ 3"), "{line}");
        assert!(line.contains("合流しない 1 枚[タブ 4"), "{line}");
        assert!(!line.contains("司令塔"), "タブ名を載せない: {line}");
    }

    #[test]
    fn 利用者のタブは器が終わっていても合流させる() {
        // 起動時に開き直される側（master / worker ではない）なので落とさない
        let (backup, mut current) = backup_and_current();
        current
            .tabs
            .push(tab(5, "notes", leaf(pane(50, Some("tako-notes-gone")))));
        let (merged, report) =
            merge_after_backup(&backup, &current, live(&["tako-base", "tako-master"]));
        assert_eq!(tab_ids(&merged), vec![1, 2, 3, 5]);
        let notes = report.merged.iter().find(|t| t.id == 5).unwrap();
        assert_eq!((notes.alive, notes.dead), (0, 1));
    }

    #[test]
    fn 器の生死を言い切れなければ世代の後のタブをすべて合流させる() {
        let (backup, current) = backup_and_current();
        let (merged, report) = merge_after_backup(&backup, &current, None);
        assert_eq!(tab_ids(&merged), vec![1, 2, 3, 4]);
        assert_eq!(report.untrusted, Some(Untrusted::NoAnswer));
        assert!(report.skipped.is_empty());
        assert!(report
            .log_line("2")
            .unwrap()
            .contains("器の生死は確かめていない"));
    }

    #[test]
    fn 世代の側にあるペインは二重に置かない() {
        // 世代の後でペイン 20 をタブ 2 から新しいタブ 5 へ移し、タブ 5 にペイン 50 を足した
        let backup = file(vec![
            tab(1, "base", leaf(pane(10, Some("tako-base")))),
            tab(2, "w", leaf(pane(20, Some("tako-moved")))),
        ]);
        let current = file(vec![
            tab(1, "base", leaf(pane(10, Some("tako-base")))),
            tab(
                5,
                "new",
                split(
                    leaf(pane(20, Some("tako-moved"))),
                    leaf(pane(50, Some("tako-new"))),
                ),
            ),
            tab(6, "only-moved", leaf(pane(21, Some("tako-base")))),
        ]);
        let (merged, report) = merge_after_backup(
            &backup,
            &current,
            live(&["tako-base", "tako-moved", "tako-new"]),
        );
        assert_eq!(tab_ids(&merged), vec![1, 2, 5]);
        assert_eq!(report.merged[0].duplicates, 1);
        assert!(matches!(&merged.tabs[2].tree, NodeLayout::Pane(p) if p.id == 50));
        assert_eq!(merged.tabs[2].focused, 50);
        assert_eq!(report.skipped[0].reason, SkipReason::AlreadyInBackup);
        crate::layout::restore(&merged).expect("ID が重複しない");
    }

    #[test]
    fn 世代の後のタブが無ければ世代そのまま() {
        let (backup, _) = backup_and_current();
        let (merged, report) = merge_after_backup(&backup, &backup, live(&["tako-base"]));
        assert_eq!(merged, backup);
        assert!(report.merged.is_empty() && report.skipped.is_empty());
        assert_eq!(report.log_line("1"), None);
    }

    #[test]
    fn 合流したタブは元のウィンドウへ戻す() {
        let (mut backup, mut current) = backup_and_current();
        backup.windows = vec![
            WindowLayout {
                id: 1,
                tabs: vec![1],
                active_tab: 1,
                frame: None,
            },
            WindowLayout {
                id: 2,
                tabs: vec![2],
                active_tab: 2,
                frame: None,
            },
        ];
        current.windows = vec![
            WindowLayout {
                id: 1,
                tabs: vec![1, 4],
                active_tab: 1,
                frame: None,
            },
            WindowLayout {
                id: 2,
                tabs: vec![3],
                active_tab: 3,
                frame: None,
            },
        ];
        current.collapsed = vec![3];
        let (merged, _) = merge_after_backup(&backup, &current, None);
        assert_eq!(merged.windows[0].tabs, vec![1, 4]);
        assert_eq!(merged.windows[1].tabs, vec![2, 3]);
        assert_eq!(merged.collapsed, vec![3]);
    }

    #[test]
    fn 世代の退避タブにあるタブは合流させない() {
        let (mut backup, mut current) = backup_and_current();
        backup.shelved_tabs = vec![ShelvedTabLayout {
            tab: tab(3, "master", leaf(pane(30, Some("tako-master")))),
            origin_window: 1,
            origin_index: 0,
        }];
        current.tabs.retain(|t| t.id != 4);
        let (merged, report) =
            merge_after_backup(&backup, &current, live(&["tako-base", "tako-master"]));
        assert_eq!(tab_ids(&merged), vec![1, 2]);
        assert!(report.merged.is_empty());
    }
}
