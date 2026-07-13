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
use tako_core::{
    BackgroundPane, Pane, PaneId, PaneNode, PaneOrigin, PaneTree, Tab, TabId, TitleSource,
    Workspace,
};

/// レイアウトファイルのフォーマットバージョン（互換のない変更で上げる）
pub const LAYOUT_VERSION: u32 = 1;

/// 読み込み・復元の失敗理由（Issue #30: 黙って空のワークスペースに
/// フォールバックせず、理由をログ・`tako persist` の診断に出すための型）
#[derive(Debug, PartialEq, thiserror::Error)]
pub enum LayoutError {
    #[error("レイアウトファイルが無い（初回起動または明示クローズ・OFF 切替後）")]
    NotFound,
    #[error("レイアウトファイルを読めない: {0}")]
    Io(String),
    #[error("レイアウトファイルを解釈できない（破損）: {0}")]
    Parse(String),
    #[error("フォーマットバージョン不一致（ファイル={0}, 対応={LAYOUT_VERSION}）")]
    Version(u32),
    #[error("タブが空")]
    Empty,
    #[error("ペイン / タブ ID が重複している（破損・継ぎ接ぎされたファイル）")]
    DuplicateId,
    #[error("分割軸が不正")]
    InvalidAxis,
    #[error("ワークスペースを再構成できない")]
    Workspace,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LayoutFile {
    pub version: u32,
    pub active_tab: u64,
    pub tabs: Vec<TabLayout>,
    /// OS ウィンドウのフレーム（サイズ・位置・フルスクリーン等。2026-06-12 追加。
    /// 旧ファイルには無いので serde default で後方互換）
    #[serde(default)]
    pub window: Option<WindowFrame>,
    /// バックグラウンドのペイン（FR-2.15.5）
    #[serde(default, alias = "shelved")]
    pub backgrounded: Vec<PaneLayout>,
    /// サイドバー tmux ビューで折りたたみ中のタブ ID（FR-2.16.14）。
    /// 旧ファイルには無いので serde default、空なら出力省略で後方互換
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub collapsed: Vec<u64>,
    /// Web ビュー dock で退避中のページ URL（FR-3.8 / #155。表示中のものは
    /// PaneLayout.webview に載る）。旧ファイル後方互換のため default + 空省略
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub webview_dock: Vec<String>,
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
    /// AI が明示追加したフォルダ（#134。旧ファイル後方互換のため default + 空なら省略）
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pinned_folders: Vec<String>,
}

/// 分割ツリーのノード（dispatch の list が返す tree 表現と同じ語彙: axis は "x" / "y"）
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum NodeLayout {
    Pane(Box<PaneLayout>),
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
    /// 実行中だった Claude Code の session ID。PC 再起動等で tmux セッション自体が
    /// 消えていた場合だけ `claude --resume` へ渡す。旧ファイルは None で後方互換
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub claude_session_id: Option<String>,
    /// プレビューペイン（FR-3.2 / FR-3.3）の表示内容。None = ターミナルペイン。
    /// 旧ファイルには無いので serde default で後方互換
    #[serde(default)]
    pub preview: Option<PreviewLayout>,
    /// Web ビューペイン（FR-3.8 / #155）の表示 URL。None = ターミナル / プレビュー。
    /// 復元時は URL を開き直す（ページ内状態までは復元しない）。旧ファイル後方互換
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub webview: Option<String>,
    /// バックグラウンドペインの由来タブ ID（FR-2.15.6。タブ別分離表示用）。tree 内のペインでは
    /// 常に None。旧ファイル後方互換のため default + 出力時は None を省略する
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin_tab: Option<u64>,
    /// バックグラウンドペインの由来タブ名（同上。閉じたタブ由来でも親を明記できるよう保持）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin_tab_title: Option<String>,
}

/// プレビューペインの保存内容（復元時はファイルを開き直す。PTY は起動しない）
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PreviewLayout {
    pub path: String,
    /// "code" | "markdown" | "image" | "pdf" | "video"
    pub mode: String,
}

/// capture 時にアプリ側から渡すペイン付帯情報
#[derive(Debug, Clone, Default)]
pub struct PaneMeta {
    pub session: Option<String>,
    pub cwd: Option<String>,
    pub claude_session_id: Option<String>,
    pub preview: Option<PreviewLayout>,
    /// Web ビューペインなら表示中の URL（FR-3.8 / #155）
    pub webview: Option<String>,
}

/// 復元されたペインの spawn 指示（Workspace へ挿入済み。セッション起動は呼び出し側）
#[derive(Debug, Clone, PartialEq)]
pub struct RestoredPane {
    pub pane: u64,
    pub session: Option<String>,
    pub cwd: Option<String>,
    /// tmux セッション消失時に復旧する Claude Code の session ID
    pub claude_session_id: Option<String>,
    /// Some ならプレビューペインとして復元する（spawn しない）
    pub preview: Option<PreviewLayout>,
    /// Some なら Web ビューペインとして復元する（spawn しない。URL を開き直す）
    pub webview: Option<String>,
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
                pinned_folders: tab
                    .pinned_folders()
                    .iter()
                    .map(|p| p.display().to_string())
                    .collect(),
            })
            .collect(),
        backgrounded: ws
            .shelved_panes()
            .iter()
            .map(|shelved| {
                let pane = shelved.pane();
                let m = meta(pane.id());
                PaneLayout {
                    id: pane.id().as_u64(),
                    session: m.session,
                    title: pane.title().map(str::to_string),
                    title_source: title_source_str(pane.title_source()).to_string(),
                    role: pane.role().map(str::to_string),
                    origin: origin_str(pane.origin()).to_string(),
                    cwd: m.cwd,
                    claude_session_id: m.claude_session_id,
                    preview: m.preview,
                    webview: m.webview,
                    // 由来タブ（FR-2.15.6）。再起動後もタブ別分離表示を保つ
                    origin_tab: Some(shelved.origin_tab().as_u64()),
                    origin_tab_title: Some(shelved.origin_tab_title().to_string()),
                }
            })
            .collect(),
        // 折りたたみ状態（FR-2.16.14）と Web ビュー dock（#155）は Workspace に
        // 無い UI 状態なので capture では空にし、save 時に UI 層が埋める
        collapsed: Vec::new(),
        webview_dock: Vec::new(),
    }
}

fn capture_node(node: &PaneNode, meta: &dyn Fn(PaneId) -> PaneMeta) -> NodeLayout {
    match node {
        PaneNode::Leaf(pane) => {
            let m = meta(pane.id());
            NodeLayout::Pane(Box::new(PaneLayout {
                id: pane.id().as_u64(),
                session: m.session,
                title: pane.title().map(str::to_string),
                title_source: title_source_str(pane.title_source()).to_string(),
                role: pane.role().map(str::to_string),
                origin: origin_str(pane.origin()).to_string(),
                cwd: m.cwd,
                claude_session_id: m.claude_session_id,
                preview: m.preview,
                webview: m.webview,
                // tree 内のペインは退避ではないので由来タブを持たない
                origin_tab: None,
                origin_tab_title: None,
            }))
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
/// tako-core 側で先へ進む）。バージョン不一致・空・ID 重複・不正値は理由付きで拒否し、
/// 呼び出し側が新規ワークスペースへフォールバック + 理由をログに残す
pub fn restore(file: &LayoutFile) -> Result<(Workspace, Vec<RestoredPane>), LayoutError> {
    if file.version != LAYOUT_VERSION {
        return Err(LayoutError::Version(file.version));
    }
    if file.tabs.is_empty() {
        return Err(LayoutError::Empty);
    }
    // ID 重複（壊れた・継ぎ接ぎされたファイル）の拒否
    let mut pane_ids = HashSet::new();
    let mut tab_ids = HashSet::new();
    for tab in &file.tabs {
        if !tab_ids.insert(tab.id) {
            return Err(LayoutError::DuplicateId);
        }
        let mut stack = vec![&tab.tree];
        while let Some(node) = stack.pop() {
            match node {
                NodeLayout::Pane(p) => {
                    if !pane_ids.insert(p.id) {
                        return Err(LayoutError::DuplicateId);
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
        let (root, focused) = restore_node(&tab_layout.tree, tab_layout.focused, &mut restored)
            .ok_or(LayoutError::InvalidAxis)?;
        let tree = PaneTree::from_root(root, focused);
        let pinned: Vec<PathBuf> = {
            let mut seen = std::collections::HashSet::new();
            tab_layout
                .pinned_folders
                .iter()
                .map(PathBuf::from)
                .filter_map(|p| {
                    let canon = p.canonicalize().unwrap_or_else(|_| p.clone());
                    if !canon.is_dir() || !seen.insert(canon.clone()) {
                        return None;
                    }
                    Some(canon)
                })
                .collect()
        };
        let tab = Tab::restore(
            tab_layout.id,
            tab_layout.title.clone(),
            parse_title_source(&tab_layout.title_source),
            tree,
            pinned,
        );
        if tab_layout.id == file.active_tab {
            active = Some(tab.id());
        }
        tabs.push(tab);
    }
    // たまり場ペインの復元（FR-2.15.5 / FR-2.15.6）。由来タブ無しの旧ファイルは
    // アクティブ（無ければ先頭）タブを由来とみなしてフォールバックする
    let fallback_tab = active.unwrap_or_else(|| tabs[0].id());
    let fallback_title = tabs
        .iter()
        .find(|t| t.id() == fallback_tab)
        .map(|t| t.title().to_string())
        .unwrap_or_default();
    let mut bg_panes = Vec::new();
    for p in &file.backgrounded {
        if !pane_ids.insert(p.id) {
            return Err(LayoutError::DuplicateId);
        }
        let pane = Pane::restore(
            p.id,
            parse_origin(&p.origin),
            p.title.clone(),
            parse_title_source(&p.title_source),
            p.role.clone(),
        );
        restored.push(RestoredPane {
            pane: p.id,
            session: p.session.clone(),
            cwd: p.cwd.clone(),
            claude_session_id: p.claude_session_id.clone(),
            preview: p.preview.clone(),
            webview: p.webview.clone(),
        });
        let origin_tab = p.origin_tab.map(TabId::from_raw).unwrap_or(fallback_tab);
        let origin_title = p
            .origin_tab_title
            .clone()
            .unwrap_or_else(|| fallback_title.clone());
        bg_panes.push(BackgroundPane::from_pane(pane, origin_tab, origin_title));
    }

    let active = active.unwrap_or(tabs[0].id());
    let ws =
        Workspace::restore_with_shelved(tabs, active, bg_panes).ok_or(LayoutError::Workspace)?;
    Ok((ws, restored))
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
                claude_session_id: p.claude_session_id.clone(),
                preview: p.preview.clone(),
                webview: p.webview.clone(),
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

/// 読み込み。不在・破損は None（理由が要らない呼び出し向け。窓ジオメトリ復元等）
pub fn load() -> Option<LayoutFile> {
    try_load().ok()
}

/// 読み込み（理由付き）。不在と破損を区別できるので、起動時の復元は
/// こちらを使って失敗理由をログに残す（Issue #30）
pub fn try_load() -> Result<LayoutFile, LayoutError> {
    let path = layout_path().ok_or_else(|| {
        LayoutError::Io("データディレクトリを解決できない（HOME 未設定等）".into())
    })?;
    load_from(&path)
}

fn load_from(path: &Path) -> Result<LayoutFile, LayoutError> {
    let json = std::fs::read_to_string(path).map_err(|e| {
        if e.kind() == io::ErrorKind::NotFound {
            LayoutError::NotFound
        } else {
            LayoutError::Io(e.to_string())
        }
    })?;
    serde_json::from_str(&json).map_err(|e| LayoutError::Parse(e.to_string()))
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
                claude_session_id: Some(format!("claude-session-{}", pane.as_u64())),
                preview: Some(PreviewLayout {
                    path: format!("/tmp/p{}.md", pane.as_u64()),
                    mode: "markdown".into(),
                }),
                webview: Some(format!("http://localhost:300{}", pane.as_u64())),
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
        assert!(restored.iter().all(|r| r.claude_session_id.as_deref()
            == Some(format!("claude-session-{}", r.pane).as_str())));
        // プレビュー情報（FR-3.2）も往復する
        assert!(restored.iter().all(|r| r
            .preview
            .as_ref()
            .is_some_and(|p| p.mode == "markdown" && p.path == format!("/tmp/p{}.md", r.pane))));
        // Web ビュー URL（FR-3.8 / #155）も往復する
        assert!(restored
            .iter()
            .all(|r| r.webview.as_deref()
                == Some(format!("http://localhost:300{}", r.pane).as_str())));
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
    fn 退避ペインの由来タブが永続化で往復する() {
        let mut ws = sample_workspace();
        // tab1（"作業"）の root を退避（2 ペインあるのでタブは残る）
        let shelve_target = ws.tabs()[0].tree().panes()[0].id();
        let origin_tab = ws.tabs()[0].id();
        ws.shelve_pane(shelve_target).unwrap();
        let layout = capture(&ws, &|_| PaneMeta::default(), None);
        // serde 往復後も origin_tab フィールドが保たれる
        let json = serde_json::to_string(&layout).unwrap();
        let back: LayoutFile = serde_json::from_str(&json).unwrap();
        assert_eq!(back, layout);
        assert_eq!(back.backgrounded.len(), 1);
        assert_eq!(back.backgrounded[0].origin_tab, Some(origin_tab.as_u64()));
        assert_eq!(
            back.backgrounded[0].origin_tab_title.as_deref(),
            Some("作業")
        );
        // 復元後も由来タブが一致する
        let (restored_ws, _) = restore(&back).unwrap();
        assert_eq!(restored_ws.shelved_panes().len(), 1);
        let restored = &restored_ws.shelved_panes()[0];
        assert_eq!(restored.origin_tab().as_u64(), origin_tab.as_u64());
        assert_eq!(restored.origin_tab_title(), "作業");
    }

    #[test]
    fn 由来タブ無しの旧ファイルはフォールバックする() {
        // 旧フォーマット（shelved に origin_tab 無し）でも読めて、由来は
        // アクティブタブへフォールバックする（後方互換）
        let mut ws = sample_workspace();
        let shelve_target = ws.tabs()[0].tree().panes()[0].id();
        ws.shelve_pane(shelve_target).unwrap();
        let layout = capture(&ws, &|_| PaneMeta::default(), None);
        // origin_tab 系を取り除いた JSON を作る（skip_serializing_if で None は出ないので
        // 文字列から該当キーを抜くだけで旧ファイルを再現できる）
        let json = serde_json::to_string(&layout).unwrap();
        let legacy_json = json
            .replace(
                &format!(
                    "\"origin_tab\":{}",
                    layout.backgrounded[0].origin_tab.unwrap()
                ),
                "\"_ot\":0",
            )
            .replace("\"origin_tab_title\":\"作業\"", "\"_ott\":\"x\"");
        let legacy: LayoutFile = serde_json::from_str(&legacy_json).unwrap();
        assert_eq!(legacy.backgrounded[0].origin_tab, None);
        let (restored_ws, _) = restore(&legacy).unwrap();
        let active = restored_ws.active_tab_id();
        assert_eq!(restored_ws.shelved_panes()[0].origin_tab(), active);
    }

    #[test]
    fn webビューdockが永続化で往復する() {
        let ws = sample_workspace();
        let mut layout = capture(&ws, &|_| PaneMeta::default(), None);
        layout.webview_dock = vec!["http://localhost:5173".into(), "https://docs.rs".into()];
        let json = serde_json::to_string(&layout).unwrap();
        let back: LayoutFile = serde_json::from_str(&json).unwrap();
        assert_eq!(back.webview_dock, layout.webview_dock);
        // dock の無い旧ファイルも読める（後方互換）
        let legacy: LayoutFile =
            serde_json::from_str(&json.replace("\"webview_dock\":", "\"_wd\":")).unwrap();
        assert!(legacy.webview_dock.is_empty());
    }

    #[test]
    fn 壊れたレイアウトは理由付きで拒否して新規作成へ倒す() {
        // 空タブ
        assert_eq!(
            restore(&LayoutFile {
                version: LAYOUT_VERSION,
                active_tab: 1,
                tabs: vec![],
                window: None,
                backgrounded: vec![],
                collapsed: vec![],
                webview_dock: vec![],
            })
            .err(),
            Some(LayoutError::Empty)
        );
        // バージョン不一致
        let ws = sample_workspace();
        let mut layout = capture(&ws, &|_| PaneMeta::default(), None);
        layout.version = 99;
        assert_eq!(restore(&layout).err(), Some(LayoutError::Version(99)));
        // ペイン ID 重複
        let mut layout = capture(&ws, &|_| PaneMeta::default(), None);
        layout.version = LAYOUT_VERSION;
        let dup = layout.tabs[0].clone();
        layout.tabs.push(TabLayout {
            id: dup.id + 1000,
            ..dup
        });
        assert_eq!(restore(&layout).err(), Some(LayoutError::DuplicateId));
    }

    #[test]
    fn 保存と読み戻しが往復する() {
        let dir = std::env::temp_dir().join(format!("tako-layout-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("layout.json");
        let ws = sample_workspace();
        let layout = capture(&ws, &|_| PaneMeta::default(), None);
        save_to(&path, &layout).unwrap();
        assert_eq!(load_from(&path), Ok(layout));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn 読み込み失敗は不在と破損を区別する() {
        // Issue #30: 復元が黙って空になる原因を診断できるよう、理由を型で返す
        let dir = std::env::temp_dir().join(format!("tako-layout-err-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("layout.json");
        // 不在 = NotFound（初回起動。異常ではない）
        assert_eq!(load_from(&path).err(), Some(LayoutError::NotFound));
        // 破損 = Parse（理由文字列付き）
        std::fs::write(&path, "{ こわれた json").unwrap();
        assert!(matches!(
            load_from(&path).err(),
            Some(LayoutError::Parse(_))
        ));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn 折りたたみ状態が永続化で往復し旧ファイルは空になる() {
        // FR-2.16.14: capture は空、save 側が埋めた collapsed が往復する
        let ws = sample_workspace();
        let mut layout = capture(&ws, &|_| PaneMeta::default(), None);
        assert!(layout.collapsed.is_empty());
        layout.collapsed = vec![ws.active_tab_id().as_u64()];
        let json = serde_json::to_string(&layout).unwrap();
        let back: LayoutFile = serde_json::from_str(&json).unwrap();
        assert_eq!(back.collapsed, vec![ws.active_tab_id().as_u64()]);
        // collapsed フィールドの無い旧ファイルは空で読める（後方互換）
        let legacy = json.replace(
            &format!(",\"collapsed\":[{}]", ws.active_tab_id().as_u64()),
            "",
        );
        let legacy: LayoutFile = serde_json::from_str(&legacy).unwrap();
        assert!(legacy.collapsed.is_empty());
    }
}
