//! 退避（バックグラウンド）ペインの一覧の組み立て（#1946）
//!
//! たまり場（ドロワー）と `tako background list` / MCP `tako_background_list` が
//! **同じ構造**を見るための 1 実装。ここで決めるのは 3 つ:
//!
//! 1. **器**（[`Vessel`]）: そのペインの中身を抱えているもの（tmux セッション /
//!    tako 直の PTY / プレビュー / Web ビュー / 無し）。「無し」は GUI 再起動などで
//!    器を失った**幽霊**で、画面も入力先も無い（#1576 の機序）。閉じ方も変わる（[`close_plan`]）
//! 2. **親 master**（[`parent_master`]）: `spawned_by` の枝を辿って着いた master。
//!    枝が無い worker は、master が 1 枚だけのときに限りそこへ寄せる
//!    （`Workspace::workers_of` と同じ規則 = #210 の誤認防止）
//! 3. **グループ**（[`groups`]）: 由来タブごと → その中で親 master ごと。
//!    タブ単位の退避（#1487）も同じ形で返す（画面側は 1 枚のタブ形カードのまま描く）
//!
//! 器の有無は Workspace の外（GUI が持つ端末・プレビューの表）にあるので、
//! 呼び出し側が [`VesselFacts`] を渡して [`Vessel::classify`] で決める
//! （dispatch と GUI が別々に「幽霊とは何か」を書かないため）。

use crate::pane::{Pane, PaneId};
use crate::screen::{ScreenLine, StyleRun};
use crate::tab::TabId;
use crate::theme::Rgb;
use crate::workspace::{is_master_role, is_worker_role, Workspace};

/// #1946 の A/B。`TAKO_1946_LEGACY=1` でたまり場を #1946 **前**の形
/// （カード寸法への resize + 切り取り・由来タブだけの平坦な並び・器の区別なし）へ戻す。
/// 出力のたびに通る可視判定から引かれるので、env はプロセスで 1 回だけ読む
pub fn legacy() -> bool {
    static LEGACY: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *LEGACY.get_or_init(|| std::env::var("TAKO_1946_LEGACY").is_ok_and(|v| v == "1"))
}

/// 退避ペインの器（#1946）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Vessel {
    /// tmux バックエンドのセッション（GUI を再起動しても生きている）
    Tmux,
    /// tako が直接抱えている PTY（GUI と寿命が同じ）
    Direct,
    /// プレビューペイン（プロセスを持たない）
    Preview,
    /// Web ビューペイン（プロセスを持たない）
    Web,
    /// 器が無い = 幽霊（GUI 再起動で失われた等。画面も入力先も無い）
    Missing,
}

/// 器を決める材料（GUI / dispatch の host がそれぞれの表から埋める）
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct VesselFacts {
    /// プレビューの状態を持っている
    pub preview: bool,
    /// Web ビューが付いている
    pub web: bool,
    /// 端末セッション（PTY / tmux の attach クライアント）を持っている
    pub terminal: bool,
    /// tmux バックエンドのセッション名が登録されている（再接続中で端末が一時的に
    /// 無いときもここは立つ = 器は生きている扱い。#1857）
    pub backend: bool,
}

impl Vessel {
    /// 材料から器を決める。プレビュー / Web は端末を持たないのが正常なので先に見る
    pub fn classify(facts: VesselFacts) -> Self {
        if facts.preview {
            Self::Preview
        } else if facts.web {
            Self::Web
        } else if facts.backend {
            Self::Tmux
        } else if facts.terminal {
            Self::Direct
        } else {
            Self::Missing
        }
    }

    /// CLI / MCP の応答に出す綴り
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Tmux => "tmux",
            Self::Direct => "direct",
            Self::Preview => "preview",
            Self::Web => "web",
            Self::Missing => "none",
        }
    }

    /// 幽霊（器が無い）か
    pub fn is_missing(self) -> bool {
        self == Self::Missing
    }
}

/// 退避エントリを閉じるときにすること（#1946）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClosePlan {
    /// 器（tmux セッション）を kill してよい
    pub kill_vessel: bool,
    /// worker レジストリへ「明示 close」を記録してよい（#775）
    pub record_worker_close: bool,
}

/// 器に応じた閉じ方（#1946）。
///
/// 幽霊は**一覧から外すだけ**にする。器を持たないので kill する相手が居ないのはもちろん、
/// レジストリへ「閉じた」とも書かない —— #1576 の機序では本物の器が**別の pane id**で
/// 生きていることがあり（orphan 自動復帰で「復帰」タブへ立つ）、ここで closed を書くと
/// 生きている worker を閉じたことにしてしまう。死んでいるならレジストリの GC（#658。
/// 「ペインも器も消えている」の実測）が閉じる
pub fn close_plan(vessel: Vessel) -> ClosePlan {
    match vessel {
        Vessel::Missing => ClosePlan {
            kill_vessel: false,
            record_worker_close: false,
        },
        _ => ClosePlan {
            kill_vessel: true,
            record_worker_close: true,
        },
    }
}

/// 親 master の解決結果（グループの鍵）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MasterKey {
    /// master のペイン（表示中・退避中のどちらか。そのペイン自身が master のときも含む）
    Pane(PaneId),
    /// `spawned_by` の先がもう居ない（親が閉じた）。master だったかは分からない
    Gone(PaneId),
    /// 親が分からない（枝が無く、master も 1 枚に決まらない）
    Unknown,
}

/// `spawned_by` を辿る上限（循環の保険。実際の深さは master → worker → 補助の 2〜3）
const MAX_HOPS: usize = 16;

/// ペインの親 master を決める（#1946）。
///
/// - ペイン自身が master の role なら自分
/// - `spawned_by` を辿り、最初に着いた master の role のペイン
/// - 辿った先のペインが居なければ [`MasterKey::Gone`]
/// - 枝が尽きても決まらない worker の role のペインは、master が 1 枚だけのときに
///   限りそこへ寄せる（`Workspace::workers_of` と同じ規則）
pub fn parent_master(ws: &Workspace, pane: PaneId) -> MasterKey {
    let Some(start) = ws.pane_anywhere(pane) else {
        return MasterKey::Unknown;
    };
    if start.role().is_some_and(is_master_role) {
        return MasterKey::Pane(pane);
    }
    let mut current = start;
    let mut seen = vec![pane];
    for _ in 0..MAX_HOPS {
        let Some(parent) = current.spawned_by() else {
            break;
        };
        if seen.contains(&parent) {
            break;
        }
        seen.push(parent);
        match ws.pane_anywhere(parent) {
            None => return MasterKey::Gone(parent),
            Some(p) if p.role().is_some_and(is_master_role) => return MasterKey::Pane(parent),
            Some(p) => current = p,
        }
    }
    if start.role().is_some_and(is_worker_role) {
        let mut masters = ws
            .all_panes()
            .into_iter()
            .filter(|p| p.role().is_some_and(is_master_role));
        if let (Some(only), None) = (masters.next(), masters.next()) {
            return MasterKey::Pane(only.id());
        }
    }
    MasterKey::Unknown
}

/// master の role からプロファイル名を読む（`orchestrator-master[:<p>]` / `master[:<p>]`）
pub fn master_profile(role: &str) -> Option<String> {
    if let Some(p) = crate::handoff::master_profile_of_role(role) {
        return Some(p.to_string());
    }
    match role.strip_prefix("master") {
        Some("") => Some(crate::handoff::DEFAULT_PROFILE.to_string()),
        Some(rest) => rest
            .strip_prefix(':')
            .filter(|s| !s.is_empty())
            .map(str::to_string),
        None => None,
    }
}

/// 親 master が今どこに居るか
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MasterPlace {
    /// 表示中のタブ
    Tab(TabId),
    /// 退避中
    Background,
    /// もう居ない（[`MasterKey::Gone`]）
    Gone,
}

impl MasterPlace {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Tab(_) => "tab",
            Self::Background => "background",
            Self::Gone => "gone",
        }
    }
}

/// 親 master の表示材料（見出しと CLI / MCP の応答で共用）
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MasterInfo {
    pub pane: PaneId,
    /// role から読んだプロファイル名（居ない親は None）
    pub profile: Option<String>,
    /// ペインのタイトル（居ない親は None）
    pub title: Option<String>,
    pub place: MasterPlace,
}

/// 鍵から親 master の表示材料を引く（[`MasterKey::Unknown`] は None）
pub fn master_info(ws: &Workspace, key: MasterKey) -> Option<MasterInfo> {
    match key {
        MasterKey::Unknown => None,
        MasterKey::Gone(pane) => Some(MasterInfo {
            pane,
            profile: None,
            title: None,
            place: MasterPlace::Gone,
        }),
        MasterKey::Pane(pane) => {
            let p = ws.pane_anywhere(pane)?;
            let place = match ws.find_tab_of_pane(pane) {
                Some(tab) => MasterPlace::Tab(tab),
                None => MasterPlace::Background,
            };
            Some(MasterInfo {
                pane,
                profile: p.role().and_then(master_profile),
                title: p.title().map(str::to_string),
                place,
            })
        }
    }
}

/// グループの由来の種類
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GroupOrigin {
    /// 由来タブが表示中に在る
    Tab,
    /// タブごと退避した（#1487）。画面では 1 枚のタブ形カードのまま描く
    ShelvedTab,
    /// 由来タブが既に閉じている
    ClosedTab,
}

impl GroupOrigin {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Tab => "tab",
            Self::ShelvedTab => "shelved_tab",
            Self::ClosedTab => "closed_tab",
        }
    }
}

/// 親 master ごとのまとまり（グループの中の見出し 1 つ）
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MasterGroup {
    pub master: MasterKey,
    /// 表示順。master 自身がこの中に居れば先頭
    pub panes: Vec<PaneId>,
}

/// 由来タブごとのまとまり
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackgroundGroup {
    pub origin_tab: TabId,
    /// 由来タブ名（閉じたタブは退避時点のスナップショット）
    pub title: String,
    pub origin: GroupOrigin,
    /// 親 master ごと。並びは初出順 → 親が閉じたもの → 親不明
    pub masters: Vec<MasterGroup>,
}

impl BackgroundGroup {
    /// グループ内の全ペイン（見出し順）
    pub fn panes(&self) -> impl Iterator<Item = PaneId> + '_ {
        self.masters.iter().flat_map(|m| m.panes.iter().copied())
    }

    pub fn pane_count(&self) -> usize {
        self.masters.iter().map(|m| m.panes.len()).sum()
    }
}

/// 退避ペインを「由来タブ → 親 master」でまとめる（#1946）。
///
/// 並びはたまり場の左から右と同じ: 退避タブ（退避した順）→ 表示中のタブ（タブ順）→
/// 閉じたタブ（初出順）。退避タブには、同じ由来を持つ平坦な退避も入る
/// （ペイン単位で 1 本退避したあとタブごと退避した形。分けると同じタブが 2 か所に出る）
pub fn groups(ws: &Workspace) -> Vec<BackgroundGroup> {
    let mut out: Vec<BackgroundGroup> = Vec::new();
    for entry in ws.shelved_tabs() {
        let mut panes: Vec<PaneId> = entry.pane_ids();
        panes.extend(
            ws.shelved_panes()
                .iter()
                .filter(|p| p.origin_tab() == entry.id())
                .map(|p| p.id()),
        );
        out.push(build(
            ws,
            entry.id(),
            entry.title(),
            GroupOrigin::ShelvedTab,
            &panes,
        ));
    }
    for tab in ws.tabs() {
        let panes: Vec<PaneId> = ws
            .shelved_panes()
            .iter()
            .filter(|p| p.origin_tab() == tab.id())
            .map(|p| p.id())
            .collect();
        if !panes.is_empty() {
            out.push(build(ws, tab.id(), tab.title(), GroupOrigin::Tab, &panes));
        }
    }
    let mut closed: Vec<(TabId, String, Vec<PaneId>)> = Vec::new();
    for p in ws.shelved_panes() {
        let origin = p.origin_tab();
        if ws.get_tab(origin).is_some() || ws.is_shelved_tab(origin) {
            continue;
        }
        match closed.iter_mut().find(|(t, _, _)| *t == origin) {
            Some((_, _, panes)) => panes.push(p.id()),
            None => closed.push((origin, p.origin_tab_title().to_string(), vec![p.id()])),
        }
    }
    for (tab, title, panes) in closed {
        out.push(build(ws, tab, &title, GroupOrigin::ClosedTab, &panes));
    }
    out
}

fn build(
    ws: &Workspace,
    origin_tab: TabId,
    title: &str,
    origin: GroupOrigin,
    panes: &[PaneId],
) -> BackgroundGroup {
    let mut masters: Vec<MasterGroup> = Vec::new();
    for &pane in panes {
        let key = parent_master(ws, pane);
        match masters.iter_mut().find(|m| m.master == key) {
            Some(m) => m.panes.push(pane),
            None => masters.push(MasterGroup {
                master: key,
                panes: vec![pane],
            }),
        }
    }
    for m in &mut masters {
        if let MasterKey::Pane(master) = m.master {
            if let Some(i) = m.panes.iter().position(|p| *p == master) {
                let head = m.panes.remove(i);
                m.panes.insert(0, head);
            }
        }
    }
    // 親が分かるもの（初出順）→ 親が閉じたもの → 親不明。sort は安定なので初出順は保たれる
    masters.sort_by_key(|m| match m.master {
        MasterKey::Pane(_) => 0,
        MasterKey::Gone(_) => 1,
        MasterKey::Unknown => 2,
    });
    BackgroundGroup {
        origin_tab,
        title: title.to_string(),
        origin,
        masters,
    }
}

/// ペインが master の role か（見出しの「master 自身」の印に使う）
pub fn is_master_pane(pane: &Pane) -> bool {
    pane.role().is_some_and(is_master_role)
}

/// サムネイル用の 1 行（#1946）。プレビューペインの本文（色付きの区間）を端末の
/// 行と同じ形（[`ScreenLine`]）へ写し、端末グリッドの描画経路でそのまま縮小して描けるようにする。
/// `max_cols` を超える分は落とす（全角は 2 列）
pub fn text_line(segments: &[(&str, Rgb, bool, bool)], max_cols: usize) -> ScreenLine {
    let mut text = String::new();
    let mut runs: Vec<StyleRun> = Vec::new();
    let mut cell_cols: Vec<usize> = Vec::new();
    let mut col = 0usize;
    'outer: for (seg, fg, bold, italic) in segments {
        let start = text.len();
        for ch in seg.chars() {
            if ch == '\t' || ch.is_control() {
                continue;
            }
            let w = unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0);
            if w == 0 {
                continue;
            }
            if col + w > max_cols {
                if text.len() > start {
                    runs.push(run(start..text.len(), *fg, *bold, *italic));
                }
                break 'outer;
            }
            cell_cols.push(col);
            text.push(ch);
            col += w;
        }
        if text.len() > start {
            runs.push(run(start..text.len(), *fg, *bold, *italic));
        }
    }
    ScreenLine {
        text,
        runs,
        cell_cols,
    }
}

fn run(range: std::ops::Range<usize>, fg: Rgb, bold: bool, italic: bool) -> StyleRun {
    StyleRun {
        range,
        fg,
        bg: None,
        bold,
        italic,
        underline: false,
        strikeout: false,
        dim: false,
    }
}

/// 文字列を表示幅 `cols` で折り返す（全角は 2 列。空なら空行 1 つ）
pub fn wrap_cols(text: &str, cols: usize) -> Vec<String> {
    let cols = cols.max(1);
    let mut out: Vec<String> = Vec::new();
    let mut line = String::new();
    let mut width = 0usize;
    for ch in text.chars() {
        let w = unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0);
        if width + w > cols && !line.is_empty() {
            out.push(std::mem::take(&mut line));
            width = 0;
        }
        line.push(ch);
        width += w;
    }
    if !line.is_empty() || out.is_empty() {
        out.push(line);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pane::PaneOrigin;
    use crate::pane_tree::SplitDirection;

    fn with_role(role: Option<&str>, spawned_by: Option<PaneId>) -> Pane {
        let mut p = Pane::new(PaneOrigin::User);
        p.set_role(role.map(str::to_string));
        p.set_spawned_by(spawned_by);
        p
    }

    /// タブ `title` にペインを並べて作る（先頭がルート）
    fn add_tab(ws: &mut Workspace, title: &str, panes: Vec<Pane>) -> (TabId, Vec<PaneId>) {
        let ids: Vec<PaneId> = panes.iter().map(|p| p.id()).collect();
        let mut it = panes.into_iter();
        let tab = ws.create_tab(title, it.next().unwrap());
        let mut prev = ids[0];
        for p in it {
            let id = p.id();
            ws.get_tab_mut(tab)
                .unwrap()
                .tree_mut()
                .split(prev, SplitDirection::Right, p)
                .unwrap();
            prev = id;
        }
        (tab, ids)
    }

    #[test]
    fn 器は材料から1つに決まりプレビューとwebは端末なしでも幽霊にならない() {
        let f = |preview, web, terminal, backend| {
            Vessel::classify(VesselFacts {
                preview,
                web,
                terminal,
                backend,
            })
        };
        assert_eq!(f(false, false, false, false), Vessel::Missing);
        assert_eq!(f(false, false, true, false), Vessel::Direct);
        assert_eq!(f(false, false, true, true), Vessel::Tmux);
        // 再接続中（#1857）: 端末が一時的に無くても器の登録があれば生きている
        assert_eq!(f(false, false, false, true), Vessel::Tmux);
        assert_eq!(f(true, false, false, false), Vessel::Preview);
        assert_eq!(f(false, true, false, false), Vessel::Web);
        assert_eq!(Vessel::Missing.as_str(), "none");
        assert!(Vessel::Missing.is_missing() && !Vessel::Tmux.is_missing());
    }

    #[test]
    fn 幽霊を閉じても器もレジストリも触らない() {
        let ghost = close_plan(Vessel::Missing);
        assert!(!ghost.kill_vessel && !ghost.record_worker_close);
        for v in [Vessel::Tmux, Vessel::Direct, Vessel::Preview, Vessel::Web] {
            let plan = close_plan(v);
            assert!(plan.kill_vessel && plan.record_worker_close, "{v:?}");
        }
    }

    #[test]
    fn master_の_role_からプロファイルを読む() {
        assert_eq!(
            master_profile("orchestrator-master").as_deref(),
            Some("default")
        );
        assert_eq!(
            master_profile("orchestrator-master:tako").as_deref(),
            Some("tako")
        );
        assert_eq!(master_profile("master").as_deref(), Some("default"));
        assert_eq!(master_profile("master:sol").as_deref(), Some("sol"));
        assert_eq!(master_profile("orchestrator-worker:tako"), None);
        assert_eq!(master_profile("masterful"), None);
    }

    #[test]
    fn 親masterはspawned_byを辿り居なければgoneで枝の無いworkerは唯一のmasterへ寄せる() {
        let m1 = with_role(Some("orchestrator-master:a"), None);
        let m1_id = m1.id();
        let mut ws = Workspace::new("main", m1);
        let w1 = with_role(Some("orchestrator-worker:x"), Some(m1_id));
        let w1_id = w1.id();
        // worker が split した補助ペイン（孫）も同じ master へ着く
        let helper = with_role(None, Some(w1_id));
        let helper_id = helper.id();
        let (_, _) = add_tab(&mut ws, "work", vec![w1, helper]);
        assert_eq!(parent_master(&ws, w1_id), MasterKey::Pane(m1_id));
        assert_eq!(parent_master(&ws, helper_id), MasterKey::Pane(m1_id));
        assert_eq!(parent_master(&ws, m1_id), MasterKey::Pane(m1_id));

        // 枝の無い worker: master が 1 枚なら寄せる
        let orphan = with_role(Some("orchestrator-worker:y"), None);
        let orphan_id = orphan.id();
        add_tab(&mut ws, "orphan", vec![orphan]);
        assert_eq!(parent_master(&ws, orphan_id), MasterKey::Pane(m1_id));
        // 枝の無いただのシェルは寄せない
        let shell = with_role(None, None);
        let shell_id = shell.id();
        add_tab(&mut ws, "shell", vec![shell]);
        assert_eq!(parent_master(&ws, shell_id), MasterKey::Unknown);

        // master が 2 枚になると枝の無い worker はどちらにも寄せない
        add_tab(&mut ws, "m2", vec![with_role(Some("master:b"), None)]);
        assert_eq!(parent_master(&ws, orphan_id), MasterKey::Unknown);

        // 親が閉じた
        let gone = PaneId::from_raw(9_999_001);
        let lost = with_role(Some("orchestrator-worker:z"), Some(gone));
        let lost_id = lost.id();
        add_tab(&mut ws, "lost", vec![lost]);
        assert_eq!(parent_master(&ws, lost_id), MasterKey::Gone(gone));
    }

    #[test]
    fn 循環したspawned_byでも止まる() {
        let a = with_role(None, None);
        let a_id = a.id();
        let mut ws = Workspace::new("main", a);
        let b = with_role(None, Some(a_id));
        let b_id = b.id();
        add_tab(&mut ws, "b", vec![b]);
        ws.pane_anywhere_mut(a_id)
            .unwrap()
            .set_spawned_by(Some(b_id));
        assert_eq!(parent_master(&ws, a_id), MasterKey::Unknown);
    }

    /// 受け入れ 2 の形: 2 つの master の worker を同じタブから退避すると、
    /// 由来タブの中で master ごとに分かれ、master 自身は自分の見出しの先頭に来る
    #[test]
    fn 由来タブの中で親masterごとに分かれmaster自身は先頭() {
        let base = with_role(None, None);
        let mut ws = Workspace::new("base", base);
        let ma = with_role(Some("orchestrator-master:alpha"), None);
        let ma_id = ma.id();
        let mb = with_role(Some("orchestrator-master:beta"), None);
        let mb_id = mb.id();
        let (masters_tab, _) = add_tab(&mut ws, "masters", vec![ma, mb]);
        let wa1 = with_role(Some("orchestrator-worker:p"), Some(ma_id));
        let wb1 = with_role(Some("orchestrator-worker:q"), Some(mb_id));
        let wa2 = with_role(Some("orchestrator-worker:r"), Some(ma_id));
        let stray = with_role(None, None);
        let (work_tab, ids) = add_tab(&mut ws, "work", vec![wa1, wb1, wa2, stray]);
        // 同じタブに 1 本は残しておく（最後のペインは退避できない）
        let keep = with_role(None, None);
        let keep_id = keep.id();
        ws.get_tab_mut(work_tab)
            .unwrap()
            .tree_mut()
            .split(ids[3], SplitDirection::Down, keep)
            .unwrap();
        for id in &ids {
            ws.shelve_pane(*id).unwrap();
        }
        // master alpha 自身も退避する（別タブ由来）
        ws.shelve_pane(ma_id).unwrap();

        let gs = groups(&ws);
        assert_eq!(gs.len(), 2, "{gs:?}");
        // タブ順: masters → work
        assert_eq!(gs[0].origin_tab, masters_tab);
        assert_eq!(gs[0].origin, GroupOrigin::Tab);
        assert_eq!(gs[0].masters.len(), 1);
        assert_eq!(gs[0].masters[0].master, MasterKey::Pane(ma_id));
        assert_eq!(gs[0].masters[0].panes, vec![ma_id]);
        let work = &gs[1];
        assert_eq!(work.origin_tab, work_tab);
        let keys: Vec<MasterKey> = work.masters.iter().map(|m| m.master).collect();
        assert_eq!(
            keys,
            vec![
                MasterKey::Pane(ma_id),
                MasterKey::Pane(mb_id),
                MasterKey::Unknown
            ]
        );
        assert_eq!(work.masters[0].panes, vec![ids[0], ids[2]]);
        assert_eq!(work.masters[1].panes, vec![ids[1]]);
        assert_eq!(work.masters[2].panes, vec![ids[3]]);
        assert_eq!(work.pane_count(), 4);
        assert!(!work.panes().any(|p| p == keep_id));

        // 表示材料: alpha は退避中・beta は表示中のタブ
        let a = master_info(&ws, MasterKey::Pane(ma_id)).unwrap();
        assert_eq!(a.profile.as_deref(), Some("alpha"));
        assert_eq!(a.place, MasterPlace::Background);
        let b = master_info(&ws, MasterKey::Pane(mb_id)).unwrap();
        assert_eq!(b.place, MasterPlace::Tab(masters_tab));
        assert!(master_info(&ws, MasterKey::Unknown).is_none());
    }

    #[test]
    fn 退避タブは1グループのまま閉じたタブ由来は最後に来る() {
        let base = with_role(None, None);
        let mut ws = Workspace::new("base", base);
        let m = with_role(Some("orchestrator-master"), None);
        let m_id = m.id();
        let w = with_role(Some("orchestrator-worker:p"), Some(m_id));
        let w_id = w.id();
        let (shelf_tab, _) = add_tab(&mut ws, "shelf", vec![m, w]);
        ws.shelve_tab_in(shelf_tab, false).unwrap();
        // 閉じたタブ由来の退避
        let x = with_role(None, None);
        let x_id = x.id();
        let y = with_role(None, None);
        let (closed_tab, _) = add_tab(&mut ws, "gone", vec![x, y]);
        ws.shelve_pane(x_id).unwrap();
        ws.close_tab(closed_tab).unwrap();

        let gs = groups(&ws);
        assert_eq!(gs.len(), 2, "{gs:?}");
        assert_eq!(gs[0].origin, GroupOrigin::ShelvedTab);
        assert_eq!(gs[0].origin_tab, shelf_tab);
        assert_eq!(gs[0].panes().collect::<Vec<_>>(), vec![m_id, w_id]);
        assert_eq!(gs[1].origin, GroupOrigin::ClosedTab);
        assert_eq!(gs[1].title, "gone");
        assert_eq!(gs[1].panes().collect::<Vec<_>>(), vec![x_id]);
    }

    #[test]
    fn 退避0本なら空() {
        let ws = Workspace::new("base", with_role(None, None));
        assert!(groups(&ws).is_empty());
    }

    #[test]
    fn サムネイルの行は列幅で切り全角は2列() {
        let red = Rgb::new(255, 0, 0);
        let blue = Rgb::new(0, 0, 255);
        let line = text_line(&[("ab", red, true, false), ("あい", blue, false, false)], 5);
        assert_eq!(line.text, "abあ");
        assert_eq!(line.cell_cols, vec![0, 1, 2]);
        assert_eq!(line.runs.len(), 2);
        assert_eq!(line.runs[0].range, 0..2);
        assert!(line.runs[0].bold);
        assert_eq!(line.runs[1].fg, blue);
        // タブ・制御文字は落とす
        assert_eq!(
            text_line(&[("a\tb\u{7}", red, false, false)], 10).text,
            "ab"
        );
        assert_eq!(wrap_cols("abcdef", 4), vec!["abcd", "ef"]);
        assert_eq!(wrap_cols("ああa", 4), vec!["ああ", "a"]);
        assert_eq!(wrap_cols("", 4), vec![""]);
    }
}
