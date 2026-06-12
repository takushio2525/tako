//! layout — タブ / ペイン構成の永続化（Phase 5.5 / FR-5）
//!
//! `<data_dir>/layout.json` に Workspace の構造（タブ・分割ツリー・タイトル・role・
//! tmux バックエンドのセッション名・cwd）を保存し、再起動時に**同じ ID** で復元する。
//! ID を保つことで、tmux セッション内で生き続けるプロセスが持つ `TAKO_PANE_ID` /
//! `TAKO_TAB_ID` が再起動後もそのまま有効になる（AI からの操作が途切れない。FR-5）。
//!
//! 書き出しは tmp + rename（settings / discovery と同方式）。読み込みは
//! 壊れたファイル・不明バージョン・ID 重複を None で拒否し、呼び出し側が
//! 新規ワークスペースへ無害にフォールバックする。

use std::collections::HashSet;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use tako_core::{Pane, PaneId, PaneNode, PaneOrigin, PaneTree, Tab, TitleSource, Workspace};

/// レイアウトファイルのフォーマットバージョン（互換のない変更で上げる）
pub const LAYOUT_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LayoutFile {
    pub version: u32,
    pub active_tab: u64,
    pub tabs: Vec<TabLayout>,
    /// OS ウィンドウのフレーム（サイズ・位置・フルスクリーン等。2026-06-12 追加。
    /// 旧ファイルには無いので serde default で後方互換）
    #[serde(default)]
    pub window: Option<WindowFrame>,
}

/// OS ウィンドウのジオメトリ（復元時は起動時のウィンドウ生成オプションに使う）
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WindowFrame {
    /// 復元サイズ（fullscreen / maximized 中はその解除後のサイズ）の左上座標と寸法（px）
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    /// "windowed" | "maximized" | "fullscreen"
    #[serde(default = "default_window_state")]
    pub state: String,
}

fn default_window_state() -> String {
    "windowed".into()
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TabLayout {
    pub id: u64,
    pub title: String,
    pub title_source: String,
    pub focused: u64,
    pub tree: NodeLayout,
}

/// 分割ツリーのノード（dispatch の list が返す tree 表現と同じ語彙: axis は "x" / "y"）
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum NodeLayout {
    Pane(PaneLayout),
    Split {
        axis: String,
        ratio: f32,
        first: Box<NodeLayout>,
        second: Box<NodeLayout>,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PaneLayout {
    pub id: u64,
    /// tmux バックエンドのセッション名（None = 直接 spawn だったペイン。
    /// 復元時は新しいシェルを cwd で開き直す）
    pub session: Option<String>,
    pub title: Option<String>,
    pub title_source: String,
    pub role: Option<String>,
    pub origin: String,
    /// 保存時点の cwd（OSC 7 由来）。セッションが消えていた場合の開き直しに使う
    pub cwd: Option<String>,
}

/// capture 時にアプリ側から渡すペイン付帯情報
#[derive(Debug, Clone, Default)]
pub struct PaneMeta {
    pub session: Option<String>,
    pub cwd: Option<String>,
}

/// 復元されたペインの spawn 指示（Workspace へ挿入済み。セッション起動は呼び出し側）
#[derive(Debug, Clone, PartialEq)]
pub struct RestoredPane {
    pub pane: u64,
    pub session: Option<String>,
    pub cwd: Option<String>,
}

/// 現在の Workspace 構造をレイアウト表現へ写す。`meta` でペインごとの
/// バックエンドセッション名・cwd を、`window` で OS ウィンドウのフレームを受け取る
/// （いずれも UI 層が保持している情報）
pub fn capture(
    ws: &Workspace,
    meta: &dyn Fn(PaneId) -> PaneMeta,
    window: Option<WindowFrame>,
) -> LayoutFile {
    LayoutFile {
        version: LAYOUT_VERSION,
        active_tab: ws.active_tab_id().as_u64(),
        window,
        tabs: ws
            .tabs()
            .iter()
            .map(|tab| TabLayout {
                id: tab.id().as_u64(),
                title: tab.title().to_string(),
                title_source: title_source_str(tab.title_source()).to_string(),
                focused: tab.tree().focused().as_u64(),
                tree: capture_node(tab.tree().root(), meta),
            })
            .collect(),
    }
}

fn capture_node(node: &PaneNode, meta: &dyn Fn(PaneId) -> PaneMeta) -> NodeLayout {
    match node {
        PaneNode::Leaf(pane) => {
            let m = meta(pane.id());
            NodeLayout::Pane(PaneLayout {
                id: pane.id().as_u64(),
                session: m.session,
                title: pane.title().map(str::to_string),
                title_source: title_source_str(pane.title_source()).to_string(),
                role: pane.role().map(str::to_string),
                origin: origin_str(pane.origin()).to_string(),
                cwd: m.cwd,
            })
        }
        PaneNode::Split {
            axis,
            ratio,
            first,
            second,
        } => NodeLayout::Split {
            axis: match axis {
                tako_core::SplitAxis::Horizontal => "x".to_string(),
                tako_core::SplitAxis::Vertical => "y".to_string(),
            },
            ratio: *ratio,
            first: Box::new(capture_node(first, meta)),
            second: Box::new(capture_node(second, meta)),
        },
    }
}

/// レイアウトから Workspace を復元する。ID はそのまま再現される（採番カウンタは
/// tako-core 側で先へ進む）。バージョン不一致・空・ID 重複・不正値は None
pub fn restore(file: &LayoutFile) -> Option<(Workspace, Vec<RestoredPane>)> {
    if file.version != LAYOUT_VERSION || file.tabs.is_empty() {
        return None;
    }
    // ID 重複（壊れた・継ぎ接ぎされたファイル）の拒否
    let mut pane_ids = HashSet::new();
    let mut tab_ids = HashSet::new();
    for tab in &file.tabs {
        if !tab_ids.insert(tab.id) {
            return None;
        }
        let mut stack = vec![&tab.tree];
        while let Some(node) = stack.pop() {
            match node {
                NodeLayout::Pane(p) => {
                    if !pane_ids.insert(p.id) {
                        return None;
                    }
                }
                NodeLayout::Split { first, second, .. } => {
                    stack.push(first);
                    stack.push(second);
                }
            }
        }
    }

    let mut restored = Vec::new();
    let mut tabs = Vec::new();
    let mut active = None;
    for tab_layout in &file.tabs {
        let (root, focused) = restore_node(&tab_layout.tree, tab_layout.focused, &mut restored)?;
        let tree = PaneTree::from_root(root, focused);
        let tab = Tab::restore(
            tab_layout.id,
            tab_layout.title.clone(),
            parse_title_source(&tab_layout.title_source),
            tree,
        );
        if tab_layout.id == file.active_tab {
            active = Some(tab.id());
        }
        tabs.push(tab);
    }
    let active = active.unwrap_or(tabs[0].id());
    let ws = Workspace::restore(tabs, active)?;
    Some((ws, restored))
}

/// ノードを PaneNode へ写す。戻りの PaneId は「focused 指定に一致した葉」（無ければ先頭葉）
fn restore_node(
    node: &NodeLayout,
    focused: u64,
    restored: &mut Vec<RestoredPane>,
) -> Option<(PaneNode, PaneId)> {
    match node {
        NodeLayout::Pane(p) => {
            let pane = Pane::restore(
                p.id,
                parse_origin(&p.origin),
                p.title.clone(),
                parse_title_source(&p.title_source),
                p.role.clone(),
            );
            let id = pane.id();
            restored.push(RestoredPane {
                pane: p.id,
                session: p.session.clone(),
                cwd: p.cwd.clone(),
            });
            Some((PaneNode::Leaf(pane), id))
        }
        NodeLayout::Split {
            axis,
            ratio,
            first,
            second,
        } => {
            let axis = match axis.as_str() {
                "x" => tako_core::SplitAxis::Horizontal,
                "y" => tako_core::SplitAxis::Vertical,
                _ => return None,
            };
            let (first_node, first_focus) = restore_node(first, focused, restored)?;
            let (second_node, second_focus) = restore_node(second, focused, restored)?;
            // focused の葉を含む側を返す（どちらにも無ければ first 側の先頭）
            let focus = if second_focus.as_u64() == focused {
                second_focus
            } else {
                first_focus
            };
            Some((
                PaneNode::Split {
                    axis,
                    ratio: *ratio,
                    first: Box::new(first_node),
                    second: Box::new(second_node),
                },
                focus,
            ))
        }
    }
}

fn title_source_str(source: TitleSource) -> &'static str {
    match source {
        TitleSource::Default => "default",
        TitleSource::Auto => "auto",
        TitleSource::Manual => "manual",
    }
}

fn parse_title_source(s: &str) -> TitleSource {
    match s {
        "auto" => TitleSource::Auto,
        "manual" => TitleSource::Manual,
        _ => TitleSource::Default,
    }
}

fn origin_str(origin: PaneOrigin) -> &'static str {
    match origin {
        PaneOrigin::User => "user",
        PaneOrigin::Cli => "cli",
        PaneOrigin::Mcp => "mcp",
        PaneOrigin::Suggestion => "suggestion",
    }
}

fn parse_origin(s: &str) -> PaneOrigin {
    match s {
        "cli" => PaneOrigin::Cli,
        "mcp" => PaneOrigin::Mcp,
        "suggestion" => PaneOrigin::Suggestion,
        _ => PaneOrigin::User,
    }
}

/// レイアウトファイルのパス（`<data_dir>/layout.json`）
pub fn layout_path() -> Option<PathBuf> {
    tako_core::paths::data_dir().map(|d| d.join("layout.json"))
}

/// 読み込み。不在・破損は None（呼び出し側で新規作成へフォールバック）
pub fn load() -> Option<LayoutFile> {
    load_from(&layout_path()?)
}

fn load_from(path: &Path) -> Option<LayoutFile> {
    let json = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&json).ok()
}

/// 書き出し（tmp + rename。settings と同方式）
pub fn save(layout: &LayoutFile) -> io::Result<PathBuf> {
    let path = layout_path().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::Unsupported,
            "データディレクトリを解決できない",
        )
    })?;
    save_to(&path, layout)?;
    Ok(path)
}

fn save_to(path: &Path, layout: &LayoutFile) -> io::Result<()> {
    let dir = path
        .parent()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "親ディレクトリが無い"))?;
    std::fs::create_dir_all(dir)?;
    let json = serde_json::to_string_pretty(layout)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, json)?;
    std::fs::rename(&tmp, path)
}

/// レイアウトファイルを消す（永続化 OFF への切替時などに使える。不在は無害）
pub fn remove() {
    if let Some(path) = layout_path() {
        let _ = std::fs::remove_file(path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tako_core::SplitDirection;

    fn sample_workspace() -> Workspace {
        let root = Pane::new(PaneOrigin::User);
        let root_id = root.id();
        let mut ws = Workspace::new("1", root);
        let second = Pane::new(PaneOrigin::Cli);
        ws.active_tab_mut()
            .tree_mut()
            .split(root_id, SplitDirection::Right, second)
            .unwrap();
        ws.active_tab_mut().set_title_manual("作業");
        let tab2_pane = Pane::new(PaneOrigin::Mcp);
        ws.create_tab("2", tab2_pane);
        ws
    }

    #[test]
    fn キャプチャと復元が往復しidが保たれる() {
        let ws = sample_workspace();
        let active = ws.active_tab_id().as_u64();
        let pane_ids: Vec<u64> = ws
            .tabs()
            .iter()
            .flat_map(|t| t.tree().panes().into_iter().map(|p| p.id().as_u64()))
            .collect();
        let frame = WindowFrame {
            x: 10.0,
            y: 20.0,
            width: 1280.0,
            height: 800.0,
            state: "fullscreen".into(),
        };
        let layout = capture(
            &ws,
            &|pane| PaneMeta {
                session: Some(format!("tako-s{}", pane.as_u64())),
                cwd: Some("/tmp".into()),
            },
            Some(frame.clone()),
        );

        // serde 往復（ウィンドウフレーム込み）
        let json = serde_json::to_string(&layout).unwrap();
        let back: LayoutFile = serde_json::from_str(&json).unwrap();
        assert_eq!(back, layout);
        assert_eq!(back.window, Some(frame));
        // ウィンドウフレームの無い旧ファイルも読める（後方互換）
        let legacy: LayoutFile =
            serde_json::from_str(&json.replace(",\"window\":{", ",\"_window\":{")).unwrap();
        assert_eq!(legacy.window, None);

        let (restored_ws, restored) = restore(&back).expect("復元できる");
        assert_eq!(restored_ws.active_tab_id().as_u64(), active);
        assert_eq!(restored_ws.tabs().len(), 2);
        assert_eq!(restored_ws.tabs()[0].title(), "作業");
        assert_eq!(restored_ws.tabs()[0].title_source(), TitleSource::Manual);
        let restored_ids: Vec<u64> = restored_ws
            .tabs()
            .iter()
            .flat_map(|t| t.tree().panes().into_iter().map(|p| p.id().as_u64()))
            .collect();
        assert_eq!(restored_ids, pane_ids);
        // spawn 指示はペイン数ぶん、セッション名と cwd を運ぶ
        assert_eq!(restored.len(), 3);
        assert!(restored
            .iter()
            .all(|r| r.session.as_deref() == Some(format!("tako-s{}", r.pane).as_str())));
        // フォーカスも保たれる（タブ 1 は split 後の新ペイン）
        assert_eq!(
            restored_ws.tabs()[0].tree().focused().as_u64(),
            ws.tabs()[0].tree().focused().as_u64()
        );
        // 復元後の新規採番は既存 ID と衝突しない
        let new_pane = Pane::new(PaneOrigin::User);
        assert!(new_pane.id().as_u64() > *pane_ids.iter().max().unwrap());
    }

    #[test]
    fn 壊れたレイアウトは拒否して新規作成へ倒す() {
        // 空タブ
        assert!(restore(&LayoutFile {
            version: LAYOUT_VERSION,
            active_tab: 1,
            tabs: vec![],
            window: None,
        })
        .is_none());
        // バージョン不一致
        let ws = sample_workspace();
        let mut layout = capture(&ws, &|_| PaneMeta::default(), None);
        layout.version = 99;
        assert!(restore(&layout).is_none());
        // ペイン ID 重複
        let mut layout = capture(&ws, &|_| PaneMeta::default(), None);
        layout.version = LAYOUT_VERSION;
        let dup = layout.tabs[0].clone();
        layout.tabs.push(TabLayout {
            id: dup.id + 1000,
            ..dup
        });
        assert!(restore(&layout).is_none());
    }

    #[test]
    fn 保存と読み戻しが往復する() {
        let dir = std::env::temp_dir().join(format!("tako-layout-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("layout.json");
        let ws = sample_workspace();
        let layout = capture(&ws, &|_| PaneMeta::default(), None);
        save_to(&path, &layout).unwrap();
        assert_eq!(load_from(&path), Some(layout));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
