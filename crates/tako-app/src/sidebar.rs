use gpui::{
    div, point, prelude::*, px, BoxShadow, Context, FontWeight, Keystroke, MouseButton,
    SharedString,
};
use tako_core::PaneId;

use super::*;

/// 新規作成のインライン入力欄を表す仮行のファイル名（#559）。
/// 行の判定は挿入位置（index）で行うのでパスは表示にも一致判定にも使わない
const INLINE_NEW_MARKER: &str = "__tako_inline_new__";

/// 1 階層ぶんのインデント幅（カンプ: margin-left 17px）
const INDENT_STEP: f32 = 17.0;

/// インデントガイド線（#589）。
///
/// 旧実装は行ボックスの `border-left` 1 本だけを引いていたため、**その行の深さの線しか
/// 描かれず**、子孫の行が挟まった瞬間に祖先の深さの線が途切れた（深い木ほど破線に
/// 見える）。行ごとに祖先ぶんの縦線も描くことで、深さごとに切れ目のない 1 本の
/// 縦線になる（VSCode / Zed のインデントガイドと同じ見え方）。
///
/// 行ボックスは `ml(INDENT_STEP * depth)` に置かれる。絶対配置の子は行の padding box
/// （枠線を持たせないのでボックス左端そのもの）が基準なので、深さ k の線は
/// `left = -INDENT_STEP * (depth - k)` に来る。**自分の深さ（k == depth）も同じ仕組みで
/// 描く**ので、線の太さ・位置が深さによってずれない（枠線と矩形で描き分けない）。
///
/// 高さは `top_0 + bottom_0` で行の高さちょうどに合わせる。行は隙間なく縦に積まれるので、
/// 隣接する行の線どうしがそのまま繋がる（実測: 同じ深さの連続行は 1 本の run になる）
fn indent_guides(
    depth: usize,
    own: gpui::Hsla,
    ancestor: gpui::Hsla,
) -> impl Iterator<Item = gpui::Div> {
    (1..=depth).map(move |level| {
        div()
            .absolute()
            .top_0()
            .bottom_0()
            .left(px(-INDENT_STEP * (depth - level) as f32))
            .w(px(1.0))
            .bg(if level == depth { own } else { ancestor })
    })
}

/// 新規作成のインライン入力欄を差し込む場所（#559）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct InlineInsertSlot {
    /// 作成先ディレクトリの行番号（挿入前の rows における位置）
    pub parent_index: usize,
    /// 入力欄の行番号（挿入後の rows における位置）
    pub row_index: usize,
    /// 入力欄の深さ（= 作成先の子と同じ）
    pub depth: usize,
}

/// 入力欄を **作成先ディレクトリの子として、確定後に並ぶのと同じ位置** へ置く（#559）。
///
/// 旧実装は「展開済み子孫をすべて飛ばした末尾」へ入れていたため、深い木では
/// 入力欄が親から何十行も離れた位置に出て「どこに作られるのか」が読めなかった。
///
/// 位置は VSCode の Explorer に合わせる（実機で挙動を確認済み）。ツリーの並びが
/// 「ディレクトリ先 → 名前順」なので、
///
/// - 新規フォルダ: 親の真下（ディレクトリ群の先頭）
/// - 新規ファイル: 同じ深さのディレクトリ兄弟（とその展開済み子孫）を飛ばした直後
///   = ファイル群の先頭
///
/// とする。深さは常に親 +1 で、通常行と同じインデント規則で描く
pub(crate) fn inline_insert_position(
    rows: &[filetree::Row],
    parent: &std::path::Path,
    new_is_dir: bool,
) -> Option<InlineInsertSlot> {
    let parent_index = rows.iter().position(|r| r.entry.path == parent)?;
    let depth = rows[parent_index].depth + 1;
    let mut row_index = parent_index + 1;
    if !new_is_dir {
        // ディレクトリ兄弟（depth 一致 + is_dir）とその子孫（depth 超過）を飛ばす
        while let Some(row) = rows.get(row_index) {
            let is_descendant = row.depth > depth;
            let is_dir_sibling = row.depth == depth && row.entry.is_dir;
            if is_descendant || is_dir_sibling {
                row_index += 1;
            } else {
                break;
            }
        }
    }
    Some(InlineInsertSlot {
        parent_index,
        row_index,
        depth,
    })
}

impl TakoApp {
    pub(crate) fn sync_filetree_roots(&mut self) {
        if !self.filetree.visible {
            return;
        }

        // 実体が消えた pinned フォルダを自動除去（#171）
        let tab_id = self.workspace.active_tab().id();
        if let Some(tab) = self.workspace.get_tab_mut(tab_id) {
            tab.prune_dead_folders();
        }

        // 正規パス（symlink 解決済み）でデデュープ（#171: cwd と pinned の重複排除）
        let mut canonical_set: std::collections::HashSet<std::path::PathBuf> =
            std::collections::HashSet::new();
        let mut roots: Vec<std::path::PathBuf> = Vec::new();

        let active_tab_id = self.workspace.active_tab().id();

        // フォアグラウンドペイン
        for pane in self.workspace.active_tab().tree().panes() {
            let Some(cwd) = self.terminals.get(&pane.id()).and_then(|s| s.cwd()) else {
                continue;
            };
            let cwd = cwd.to_path_buf();
            let canon = cwd.canonicalize().unwrap_or_else(|_| cwd.clone());
            if canonical_set.insert(canon) {
                roots.push(cwd);
            }
        }

        // バックグラウンドペイン（同タブ由来のみ）
        for bp in self.workspace.shelved_panes() {
            if bp.origin_tab() != active_tab_id {
                continue;
            }
            let Some(cwd) = self.terminals.get(&bp.id()).and_then(|s| s.cwd()) else {
                continue;
            };
            let cwd = cwd.to_path_buf();
            let canon = cwd.canonicalize().unwrap_or_else(|_| cwd.clone());
            if canonical_set.insert(canon) {
                roots.push(cwd);
            }
        }

        // #134: AI が明示追加したフォルダを合流（正規パスでデデュープ）
        for folder in self.workspace.active_tab().pinned_folders() {
            let canon = folder.canonicalize().unwrap_or_else(|_| folder.clone());
            if canonical_set.insert(canon) {
                roots.push(folder.clone());
            }
        }

        if roots.is_empty() {
            if let Some(home) = std::env::var_os("HOME").map(std::path::PathBuf::from) {
                roots.push(home);
            }
        }
        self.filetree.set_roots(roots);
    }
    pub(crate) fn render_sidebar(&mut self, cx: &mut Context<Self>) -> Option<gpui::Div> {
        if !self.filetree.visible {
            return None;
        }
        let theme = self.theme.clone();
        // プロジェクト名 = アクティブタブ cwd のフォルダ名（カンプ: 「tako」）
        let cwd = self.active_tab_cwd();
        let project_name = cwd
            .as_ref()
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| self.workspace.active_tab().title().to_string());
        // フルパス表示（カンプ: ~/projects/ + 末尾強調。クリックでコピー）
        let sidebar_path = cwd.as_ref().map(|p| {
            let full = p.display().to_string();
            let short = if let Ok(home) = std::env::var("HOME") {
                if let Ok(rel) = p.strip_prefix(&home) {
                    format!("~/{}", rel.display())
                } else {
                    full.clone()
                }
            } else {
                full.clone()
            };
            // 末尾要素を分離（親部分は muted、末尾は明るく）
            let (parent, leaf) = match short.rfind('/') {
                Some(i) if i + 1 < short.len() => {
                    (short[..=i].to_string(), short[i + 1..].to_string())
                }
                _ => (String::new(), short.clone()),
            };
            (parent, leaf, full)
        });
        let git_summary = self.sidebar_git.clone();
        let show_hidden = self.filetree.show_hidden();
        // プレビュー表示中のファイル（開いている行を控えめにハイライトする）
        let open_paths: std::collections::HashSet<std::path::PathBuf> =
            self.previews.values().map(|p| p.path.clone()).collect();
        let mut rows = self.filetree.rows();
        let inline_new_insert = self
            .inline_edit
            .as_ref()
            .filter(|edit| edit.kind != InlineEditKind::Rename)
            .and_then(|edit| {
                inline_insert_position(&rows, &edit.parent, edit.kind == InlineEditKind::NewDir)
            });
        // 作成先ハイライトの対象パス（入力欄そのものではなく親の行）
        let inline_parent_path = inline_new_insert
            .and_then(|slot| rows.get(slot.parent_index))
            .map(|r| r.entry.path.clone());
        // 挿入後の行番号（入力欄の判定はパスではなく位置で行う）
        let inline_row_index = inline_new_insert.map(|slot| slot.row_index);
        if let (Some(slot), Some(edit)) = (inline_new_insert, self.inline_edit.as_ref()) {
            rows.insert(
                slot.row_index,
                filetree::Row {
                    entry: filetree::Entry {
                        path: edit.parent.join(INLINE_NEW_MARKER),
                        name: String::new(),
                        is_dir: edit.kind == InlineEditKind::NewDir,
                    },
                    depth: slot.depth,
                    expanded: false,
                    root: false,
                    git_status: None,
                },
            );
        }
        let inline_edit_snapshot = self.inline_edit.clone();
        let sidebar_w = self.sidebar_width;
        let drop_highlight = self.sidebar_drop_highlight;
        Some(
            div()
                .w(px(sidebar_w))
                .h_full()
                .relative()
                .flex()
                .flex_col()
                .bg(if drop_highlight {
                    rgba_alpha(theme.accent, 0.06)
                } else {
                    rgba(theme.mantle)
                })
                .border_r_1()
                .border_color(if drop_highlight {
                    hsla(theme.accent)
                } else {
                    hsla(theme.border_subtle)
                })
                .text_size(px(12.0))
                .text_color(hsla(theme.foreground))
                .overflow_hidden()
                // Issue #219: 外部ファイルドラッグ中のハイライト + ドロップ処理
                .on_drag_move::<ExternalPaths>(cx.listener(|this, _, _, cx| {
                    if !this.sidebar_drop_highlight {
                        this.sidebar_drop_highlight = true;
                        cx.notify();
                    }
                }))
                .on_drop::<ExternalPaths>(cx.listener(|this, paths: &ExternalPaths, _, cx| {
                    this.sidebar_drop_highlight = false;
                    this.drop_files_to_sidebar(paths.paths(), cx);
                }))
                // プロジェクトヘッダ（カンプ: 名前 + ブランチチップ + フルパスボックス）
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(6.0))
                        .pt(px(10.0))
                        .px(px(12.0))
                        .pb(px(8.0))
                        .flex_none()
                        .border_b_1()
                        .border_color(hsla(theme.border_inner))
                        .child(
                            div()
                                .flex()
                                .flex_row()
                                .items_center()
                                .gap(px(8.0))
                                .child(
                                    svg()
                                        .path(file_icons::ui_icon::FOLDER)
                                        .size(px(14.0))
                                        .flex_none()
                                        .text_color(hsla(theme.accent)),
                                )
                                .child(
                                    div()
                                        .text_size(px(12.5))
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(hsla(theme.foreground))
                                        .overflow_hidden()
                                        .whitespace_nowrap()
                                        .text_ellipsis()
                                        .child(SharedString::from(truncate(&project_name, 18))),
                                )
                                // git ブランチチップ（カンプ: green / bg 10% / mono 10px）
                                .children(git_summary.as_ref().map(|g| {
                                    div()
                                        .flex()
                                        .flex_none()
                                        .flex_row()
                                        .items_center()
                                        .gap(px(4.0))
                                        .px(px(7.0))
                                        .py(px(2.0))
                                        .rounded(px(5.0))
                                        .bg(rgba_alpha(theme.green, 0.10))
                                        .font_family(theme.font_family.clone())
                                        .text_size(px(10.0))
                                        .text_color(hsla(theme.green))
                                        .child(
                                            svg()
                                                .path(file_icons::ui_icon::GIT_BRANCH)
                                                .size(px(10.0))
                                                .flex_none()
                                                .text_color(hsla(theme.green)),
                                        )
                                        .child(SharedString::from(truncate(&g.branch, 14)))
                                }))
                                .child(div().flex_grow(1.0))
                                // 目 = 隠しファイル（ドット始まり）の表示トグル（#550）
                                .child(
                                    div()
                                        .id("sidebar-toggle-hidden")
                                        .flex_none()
                                        .cursor_pointer()
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.toggle_hidden_files(cx);
                                        }))
                                        .child(
                                            svg()
                                                .path(if show_hidden {
                                                    file_icons::ui_icon::EYE
                                                } else {
                                                    file_icons::ui_icon::EYE_OFF
                                                })
                                                .size(px(14.0))
                                                .text_color(hsla(if show_hidden {
                                                    theme.accent
                                                } else {
                                                    theme.text_muted
                                                })),
                                        ),
                                )
                                // + = ファイルツリーにルート追加（#268）
                                .child(
                                    div()
                                        .id("sidebar-add-folder")
                                        .flex_none()
                                        .cursor_pointer()
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.add_tree_root(cx);
                                        }))
                                        .child(
                                            svg()
                                                .path(file_icons::ui_icon::PLUS)
                                                .size(px(14.0))
                                                .text_color(hsla(theme.text_muted)),
                                        ),
                                ),
                        )
                        // フルパスボックス（カンプ: クリックでコピー + コピーアイコン）
                        .children(sidebar_path.map(|(parent, leaf, full)| {
                            div()
                                .id("sidebar-path")
                                .flex()
                                .flex_row()
                                .items_center()
                                .gap(px(5.0))
                                .px(px(7.0))
                                .py(px(4.0))
                                .rounded(px(6.0))
                                .bg(rgba(theme.surface_1))
                                .border_1()
                                .border_color(hsla(theme.border_subtle))
                                .font_family(theme.font_family.clone())
                                .text_size(px(10.5))
                                .text_color(hsla(theme.text_muted))
                                .cursor_pointer()
                                .hover(|d| {
                                    d.border_color(hsla(theme.border_heavy))
                                        .text_color(hsla(theme.text_tertiary))
                                })
                                .on_click(cx.listener(move |_, _, _, cx| {
                                    cx.write_to_clipboard(gpui::ClipboardItem::new_string(
                                        full.clone(),
                                    ));
                                }))
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w(px(0.0))
                                        .overflow_hidden()
                                        .whitespace_nowrap()
                                        .text_ellipsis()
                                        .flex()
                                        .flex_row()
                                        .child(SharedString::from(parent))
                                        .child(
                                            div()
                                                .text_color(hsla(theme.text_secondary))
                                                .child(SharedString::from(leaf)),
                                        ),
                                )
                                .child(
                                    svg()
                                        .path(file_icons::ui_icon::COPY)
                                        .size(px(11.0))
                                        .flex_none()
                                        .text_color(hsla(theme.text_muted)),
                                )
                        })),
                )
                .child(
                    div()
                        .id("filetree-list")
                        .flex_1()
                        .flex()
                        .flex_col()
                        .track_scroll(&self.filetree_scroll_handle)
                        .overflow_y_scroll()
                        .children(rows.into_iter().enumerate().map(|(index, row)| {
                            let path = row.entry.path.clone();
                            let is_dir = row.entry.is_dir;
                            // インライン編集中の行を検出
                            let is_inline = match &inline_edit_snapshot {
                                Some(edit) if edit.kind == InlineEditKind::Rename => {
                                    path == edit.parent
                                }
                                Some(_) => Some(index) == inline_row_index,
                                None => false,
                            };
                            if let (true, Some(edit)) = (is_inline, inline_edit_snapshot.as_ref()) {
                                // 種別アイコン（絵文字全廃 #217: SVG マスク描画）
                                let icon_path = match edit.kind {
                                    InlineEditKind::Rename => {
                                        if is_dir {
                                            Some(file_icons::ui_icon::FOLDER)
                                        } else {
                                            Some(file_icons::ui_icon::FILE_GENERIC)
                                        }
                                    }
                                    InlineEditKind::NewFile => {
                                        Some(file_icons::ui_icon::FILE_GENERIC)
                                    }
                                    InlineEditKind::NewDir => Some(file_icons::ui_icon::FOLDER),
                                };
                                let placeholder = match edit.kind {
                                    InlineEditKind::Rename => {
                                        crate::ui_text::sidebar::rename_placeholder()
                                    }
                                    InlineEditKind::NewFile => {
                                        crate::ui_text::sidebar::new_file_placeholder()
                                    }
                                    InlineEditKind::NewDir => {
                                        crate::ui_text::sidebar::new_dir_placeholder()
                                    }
                                };
                                let before_cursor = &edit.text[..edit.cursor];
                                let after_cursor = &edit.text[edit.cursor..];
                                let empty = edit.text.is_empty();
                                // #559: インデントは通常行とまったく同じ規則で置く
                                // （ml 17*depth + 左ガイド線 + pl 14）。ここが揃っていないと
                                // 「どの階層に作られるのか」が読めない。自分の深さの線だけ
                                // accent にして「ここに作られる」を強調する（#589）
                                return div()
                                    .id(("filetree-row", index as u64))
                                    .flex()
                                    .flex_row()
                                    .items_center()
                                    .gap(px(4.0))
                                    .py(px(1.0))
                                    .when(row.depth >= 1, |d| {
                                        d.ml(px(INDENT_STEP * row.depth as f32))
                                            .pl(px(14.0))
                                            .children(indent_guides(
                                                row.depth,
                                                hsla(theme.accent),
                                                hsla(theme.border_subtle),
                                            ))
                                    })
                                    .when(row.depth == 0, |d| d.pl(px(12.0)))
                                    .pr(px(6.0))
                                    // chevron 分のスペーサー（兄弟行とアイコン位置を揃える）
                                    .child(div().w(px(14.0)).flex_none())
                                    .children(icon_path.map(|p| {
                                        svg()
                                            .path(p)
                                            .size(px(16.0))
                                            .flex_none()
                                            .text_color(hsla(theme.accent))
                                    }))
                                    .child(
                                        div()
                                            .flex_1()
                                            .min_w(px(0.0))
                                            .flex()
                                            .flex_row()
                                            .items_center()
                                            .border_1()
                                            .border_color(hsla(theme.accent))
                                            .rounded_sm()
                                            .px(px(3.0))
                                            .py(px(1.0))
                                            .bg(rgba(theme.background))
                                            .shadow(vec![BoxShadow {
                                                color: hsla_alpha(theme.accent, 0.35),
                                                offset: point(px(0.), px(0.)),
                                                blur_radius: px(0.),
                                                spread_radius: px(1.),
                                                inset: false,
                                            }])
                                            .when(empty, |d| {
                                                d.child(
                                                    div()
                                                        .w(px(1.5))
                                                        .h(px(13.0))
                                                        .bg(hsla(theme.accent))
                                                        .flex_none(),
                                                )
                                                .child(
                                                    div()
                                                        .pl(px(3.0))
                                                        .text_color(hsla(theme.text_muted))
                                                        .child(SharedString::from(
                                                            placeholder.to_string(),
                                                        )),
                                                )
                                            })
                                            .when(!empty, |d| {
                                                d.child(SharedString::from(
                                                    before_cursor.to_string(),
                                                ))
                                                .child(
                                                    div()
                                                        .w(px(1.5))
                                                        .h(px(13.0))
                                                        .bg(hsla(theme.accent))
                                                        .flex_none(),
                                                )
                                                .child(SharedString::from(after_cursor.to_string()))
                                            }),
                                    );
                            }
                            let is_open = !is_dir && open_paths.contains(&path);
                            // #559: 作成先の親ディレクトリ行を強調し「ここの直下に作る」を明示する
                            let is_inline_parent =
                                inline_parent_path.as_ref().is_some_and(|p| *p == path);
                            let drag_path = path.clone();
                            let base = div()
                                .id(("filetree-row", index as u64))
                                .flex()
                                .flex_row()
                                .items_center()
                                .py(px(1.0))
                                .cursor_pointer()
                                .when(is_inline_parent, |d| {
                                    d.bg(rgba_alpha(theme.accent, 0.16))
                                        .text_color(hsla(theme.foreground))
                                })
                                .hover(|d| d.bg(rgba(theme.surface_hover)))
                                .on_click(cx.listener({
                                    let ctx_path = path.clone();
                                    move |this, _: &gpui::ClickEvent, _, cx| {
                                        if is_dir {
                                            this.filetree.toggle_dir(&ctx_path);
                                        } else {
                                            this.open_file_row(&ctx_path, cx);
                                        }
                                        cx.notify();
                                    }
                                }))
                                .on_mouse_down(
                                    MouseButton::Right,
                                    cx.listener({
                                        let ctx_path = path.clone();
                                        let is_root = row.root;
                                        move |this, e: &MouseDownEvent, _, cx| {
                                            cx.stop_propagation();
                                            let is_pinned_root = is_root && {
                                                let canon = ctx_path
                                                    .canonicalize()
                                                    .unwrap_or_else(|_| ctx_path.clone());
                                                this.workspace
                                                    .active_tab()
                                                    .pinned_folders()
                                                    .iter()
                                                    .any(|f| {
                                                        f.canonicalize()
                                                            .unwrap_or_else(|_| f.clone())
                                                            == canon
                                                    })
                                            };
                                            this.context_menu = Some(ContextMenu {
                                                path: ctx_path.clone(),
                                                is_dir,
                                                is_pinned_root,
                                                position: e.position,
                                            });
                                            cx.notify();
                                        }
                                    }),
                                )
                                // ファイルは D&D でドロップ位置にプレビューとして開ける（FR-3.11）
                                .on_drag(
                                    FileDrag { path: drag_path },
                                    self.drag_ghost_builder(
                                        DragKind::File,
                                        truncate(&row.entry.name, 24),
                                        cx,
                                    ),
                                );
                            if row.root {
                                // ワークスペースフォルダの見出し行: 太字 + 上仕切り線（2 つ目以降）
                                base.when(index > 0, |d| {
                                    d.border_t_1()
                                        .border_color(hsla_alpha(theme.pane_border, 0.6))
                                        .mt_1()
                                })
                                .py(px(2.0))
                                .gap(px(4.0))
                                .font_weight(FontWeight::BOLD)
                                .text_color(hsla(theme.tab_active_foreground))
                                // chevron (SVG)
                                .child(
                                    svg()
                                        .path(file_icons::chevron_icon(row.expanded).svg_path())
                                        .size(px(14.0))
                                        .flex_none()
                                        .text_color(hsla(theme.tab_inactive_foreground)),
                                )
                                // folder icon (SVG)
                                .child(
                                    svg()
                                        .path(file_icons::folder_icon(row.expanded).svg_path())
                                        .size(px(16.0))
                                        .flex_none()
                                        .text_color(hsla(theme.accent)),
                                )
                                .child(
                                    div()
                                        .flex_1()
                                        .overflow_hidden()
                                        .whitespace_nowrap()
                                        .text_ellipsis()
                                        .child(SharedString::from(truncate(&row.entry.name, 22))),
                                )
                            } else {
                                let git_marker = row.git_status.map(|gs| match gs {
                                    filetree::GitChange::Modified => ("M", theme.yellow),
                                    filetree::GitChange::Added => ("A", theme.green),
                                    filetree::GitChange::Deleted => ("D", theme.red),
                                    filetree::GitChange::Renamed => ("R", theme.accent),
                                    filetree::GitChange::Untracked => {
                                        ("?", theme.tab_inactive_foreground)
                                    }
                                });
                                // インデントガイド線（カンプ: margin-left 17px + 1px の縦線）。
                                // 祖先の深さの線も一緒に描いて 1 本の連続線にする（#589）
                                let mut row_el = base
                                    .when(row.depth >= 1, |d| {
                                        d.ml(px(INDENT_STEP * row.depth as f32))
                                            .pl(px(14.0))
                                            .children(indent_guides(
                                                row.depth,
                                                hsla(theme.border_subtle),
                                                hsla(theme.border_subtle),
                                            ))
                                    })
                                    .when(row.depth == 0, |d| d.pl(px(12.0)))
                                    .py(px(2.0))
                                    .gap(px(4.0))
                                    .when(!is_dir, |d| d.text_color(hsla(theme.text_tertiary)))
                                    .when(is_open, |d| {
                                        d.bg(rgba_alpha(theme.accent, 0.13))
                                            .text_color(hsla(theme.foreground))
                                            .shadow(vec![BoxShadow {
                                                color: hsla(theme.accent),
                                                offset: point(px(2.), px(0.)),
                                                blur_radius: px(0.),
                                                spread_radius: px(0.),
                                                inset: true,
                                            }])
                                    });
                                if is_dir {
                                    let folder_color = if row.expanded {
                                        theme.accent
                                    } else {
                                        theme.tab_inactive_foreground
                                    };
                                    // chevron (SVG)
                                    row_el = row_el.child(
                                        svg()
                                            .path(file_icons::chevron_icon(row.expanded).svg_path())
                                            .size(px(14.0))
                                            .flex_none()
                                            .text_color(hsla(theme.tab_inactive_foreground)),
                                    );
                                    // folder icon (SVG)
                                    row_el = row_el.child(
                                        svg()
                                            .path(file_icons::folder_icon(row.expanded).svg_path())
                                            .size(px(16.0))
                                            .flex_none()
                                            .text_color(hsla(folder_color)),
                                    );
                                } else {
                                    // file: chevron 分のスペーサー + SVG file icon
                                    row_el = row_el.child(div().w(px(14.0)).flex_none());
                                    let icon_kind = file_icons::resolve_file_icon(
                                        std::path::Path::new(&row.entry.name),
                                    );
                                    let icon_color = match icon_kind.color_category() {
                                        file_icons::IconColor::Green => theme.green,
                                        file_icons::IconColor::Accent => theme.accent,
                                        file_icons::IconColor::Peach => theme.peach,
                                        file_icons::IconColor::Mauve => theme.mauve,
                                        file_icons::IconColor::Yellow => theme.yellow,
                                        file_icons::IconColor::Dim => theme.tab_inactive_foreground,
                                    };
                                    row_el = row_el.child(
                                        svg()
                                            .path(icon_kind.svg_path())
                                            .size(px(16.0))
                                            .flex_none()
                                            .text_color(hsla(icon_color)),
                                    );
                                }
                                // ファイル/フォルダ名
                                row_el = row_el.child(
                                    div()
                                        .flex_1()
                                        .overflow_hidden()
                                        .whitespace_nowrap()
                                        .text_ellipsis()
                                        .child(SharedString::from(truncate(&row.entry.name, 24))),
                                );
                                // git status マーカー
                                row_el = row_el.children(git_marker.map(|(label, color)| {
                                    div()
                                        .text_size(px(10.5))
                                        .font_family("Monaco")
                                        .font_weight(FontWeight::BOLD)
                                        .text_color(hsla(color))
                                        .flex_none()
                                        .pr(px(8.0))
                                        .child(SharedString::from(label.to_string()))
                                }));
                                row_el
                            }
                        })),
                )
                // フッター: git 変更サマリ（カンプ: N modified +A −R / diff →）
                .children(git_summary.filter(|g| g.modified > 0).map(|g| {
                    div()
                        .flex()
                        .flex_none()
                        .flex_row()
                        .items_center()
                        .gap(px(8.0))
                        .px(px(12.0))
                        .py(px(8.0))
                        .border_t_1()
                        .border_color(hsla(theme.border_inner))
                        .text_size(px(11.0))
                        .text_color(hsla(theme.text_muted))
                        .font_family(theme.font_family.clone())
                        .child(
                            div()
                                .text_color(hsla(theme.yellow))
                                .child(SharedString::from(format!("{} modified", g.modified))),
                        )
                        .child(
                            div()
                                .text_color(hsla(theme.green))
                                .child(SharedString::from(format!("+{}", g.added_lines))),
                        )
                        .child(
                            div()
                                .text_color(hsla(theme.red))
                                .child(SharedString::from(format!("\u{2212}{}", g.removed_lines))),
                        )
                        .child(div().flex_grow(1.0))
                        .child(
                            div()
                                .id("sidebar-diff-link")
                                .cursor_pointer()
                                .hover(|d| d.text_color(hsla(theme.foreground)))
                                .on_click(cx.listener(|this, _, _, cx| {
                                    // 右パネルの git ビューを開く（diff への導線）
                                    this.panel_visible = true;
                                    this.panel_view = PanelView::Git;
                                    this.refresh_tmux_data();
                                    cx.notify();
                                }))
                                .child("diff \u{2192}"),
                        )
                }))
                // 右端のリサイズハンドル（Issue #307。右パネルと同方式）
                .child(
                    div()
                        .id("sidebar-resize")
                        .absolute()
                        .right(px(0.0))
                        .top(px(0.0))
                        .w(px(BORDER_HANDLE))
                        .h_full()
                        .cursor(gpui::CursorStyle::ResizeLeftRight)
                        .occlude()
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _: &gpui::MouseDownEvent, _, cx| {
                                this.dragging_sidebar = true;
                                cx.stop_propagation();
                            }),
                        ),
                ),
        )
    }

    /// コンテキストメニューの描画（FR-3.12）
    pub(crate) fn render_context_menu(
        &self,
        window: &gpui::Window,
        cx: &mut Context<Self>,
    ) -> Option<gpui::AnyElement> {
        let ctx = self.context_menu.as_ref()?;
        let theme = &self.theme;
        let path = ctx.path.clone();
        let is_dir = ctx.is_dir;
        let is_pinned_root = ctx.is_pinned_root;
        let pos = ctx.position;
        let mut items: Vec<(&str, &str)> = vec![
            ("copy-rel", crate::ui_text::sidebar::menu_copy_rel()),
            ("copy-abs", crate::ui_text::sidebar::menu_copy_abs()),
            ("reveal", crate::ui_text::sidebar::menu_reveal()),
            ("open-term", crate::ui_text::sidebar::menu_open_term()),
        ];
        if !is_dir {
            items.push(("open-default", crate::ui_text::sidebar::menu_open_default()));
            items.push(("open-with", crate::ui_text::sidebar::menu_open_with()));
        }
        items.push(("sep1", ""));
        items.push(("rename", crate::ui_text::sidebar::menu_rename()));
        items.push(("new-file", crate::ui_text::sidebar::menu_new_file()));
        items.push(("new-dir", crate::ui_text::sidebar::menu_new_dir()));
        items.push(("sep2", ""));
        // #550: ユーザーがまず探す場所（右クリック）にも表示トグルを置く
        items.push((
            "toggle-hidden",
            if self.filetree.show_hidden() {
                crate::ui_text::sidebar::hidden_hide()
            } else {
                crate::ui_text::sidebar::hidden_show()
            },
        ));
        items.push(("sep3", ""));
        items.push(("trash", crate::ui_text::sidebar::menu_trash()));
        if is_pinned_root {
            items.push(("sep4", ""));
            items.push(("remove-root", crate::ui_text::sidebar::menu_remove_root()));
        }

        let menu_width: f32 = 200.0;
        let item_height: f32 = 20.0;
        let sep_height: f32 = 5.0;
        let padding_y: f32 = 8.0;
        let menu_height: f32 = items
            .iter()
            .map(|(id, _)| {
                if id.starts_with("sep") {
                    sep_height
                } else {
                    item_height
                }
            })
            .sum::<f32>()
            + padding_y;
        let adjusted = clamp_menu_position(pos, menu_width, menu_height, window);

        let menu = div()
            .absolute()
            .left(adjusted.x)
            .top(adjusted.y)
            .w(px(menu_width))
            .py(px(4.0))
            .bg(rgba(theme.tab_bar_background))
            .border_1()
            .border_color(hsla(theme.pane_border))
            .rounded_md()
            .text_size(px(12.0))
            .text_color(hsla(theme.foreground))
            .occlude()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_mouse_down(MouseButton::Right, |_, _, cx| cx.stop_propagation())
            .children(items.into_iter().enumerate().map(|(i, (id, label))| {
                if id.starts_with("sep") {
                    return div()
                        .h(px(1.0))
                        .mx_1()
                        .my(px(2.0))
                        .bg(hsla_alpha(theme.pane_border, 0.5))
                        .into_any_element();
                }
                let path = path.clone();
                div()
                    .id(("ctx-item", i as u64))
                    .w_full()
                    .px_2()
                    .py(px(2.0))
                    .cursor_pointer()
                    .hover(|d| d.bg(rgba(theme.tab_active_background)))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.context_menu = None;
                        this.handle_context_action(id, &path, is_dir, cx);
                    }))
                    .when(id == "trash", |d| d.text_color(hsla(theme.red)))
                    .child(SharedString::from(label.to_string()))
                    .into_any_element()
            }));
        let backdrop = div()
            .id("ctx-backdrop")
            .absolute()
            .left(px(0.0))
            .top(px(0.0))
            .size_full()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    this.context_menu = None;
                    cx.notify();
                }),
            )
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(|this, _, _, cx| {
                    this.context_menu = None;
                    cx.notify();
                }),
            )
            .child(menu);
        Some(backdrop.into_any_element())
    }

    pub(crate) fn handle_inline_edit_key(&mut self, ks: &Keystroke, cx: &mut Context<Self>) {
        match ks.key.as_str() {
            "enter" => {
                self.commit_inline_edit(cx);
            }
            "escape" => {
                self.inline_edit = None;
                cx.notify();
            }
            "backspace" => {
                if let Some(ref mut edit) = self.inline_edit {
                    if edit.cursor > 0 {
                        let prev = edit.text[..edit.cursor]
                            .char_indices()
                            .next_back()
                            .map(|(i, _)| i)
                            .unwrap_or(0);
                        edit.text.drain(prev..edit.cursor);
                        edit.cursor = prev;
                    }
                }
                cx.notify();
            }
            "delete" => {
                if let Some(ref mut edit) = self.inline_edit {
                    if edit.cursor < edit.text.len() {
                        let next = edit.text[edit.cursor..]
                            .char_indices()
                            .nth(1)
                            .map(|(i, _)| edit.cursor + i)
                            .unwrap_or(edit.text.len());
                        edit.text.drain(edit.cursor..next);
                    }
                }
                cx.notify();
            }
            "left" => {
                if let Some(ref mut edit) = self.inline_edit {
                    if edit.cursor > 0 {
                        edit.cursor = edit.text[..edit.cursor]
                            .char_indices()
                            .next_back()
                            .map(|(i, _)| i)
                            .unwrap_or(0);
                    }
                }
                cx.notify();
            }
            "right" => {
                if let Some(ref mut edit) = self.inline_edit {
                    if edit.cursor < edit.text.len() {
                        edit.cursor = edit.text[edit.cursor..]
                            .char_indices()
                            .nth(1)
                            .map(|(i, _)| edit.cursor + i)
                            .unwrap_or(edit.text.len());
                    }
                }
                cx.notify();
            }
            "home" => {
                if let Some(ref mut edit) = self.inline_edit {
                    edit.cursor = 0;
                }
                cx.notify();
            }
            "end" => {
                if let Some(ref mut edit) = self.inline_edit {
                    edit.cursor = edit.text.len();
                }
                cx.notify();
            }
            _ => {
                if let Some(ch) = ks.key_char.as_ref() {
                    if !ch.is_empty() && !ks.modifiers.control && !ks.modifiers.platform {
                        if let Some(ref mut edit) = self.inline_edit {
                            edit.text.insert_str(edit.cursor, ch);
                            edit.cursor += ch.len();
                        }
                        cx.notify();
                    }
                }
            }
        }
    }

    /// 隠しファイル（ドット始まり）の表示トグル（#550）。
    /// CLI `tako panel --show-hidden` / MCP `tako_panel` と同じ dispatch 経路を通す
    pub(crate) fn toggle_hidden_files(&mut self, cx: &mut Context<Self>) {
        let next = !self.filetree.show_hidden();
        let _ = tako_control::dispatch(
            self,
            tako_control::protocol::Request::Panel {
                visible: None,
                width: None,
                view: None,
                filetree: None,
                sidebar_width: None,
                show_hidden: Some(next),
            },
            PaneOrigin::User,
        );
        cx.notify();
    }

    pub(crate) fn commit_inline_edit(&mut self, cx: &mut Context<Self>) {
        use tako_control::protocol::{FileOpKind, Request};
        let Some(edit) = self.inline_edit.take() else {
            return;
        };
        let name = edit.text.trim().to_string();
        if name.is_empty() {
            cx.notify();
            return;
        }
        let (op, path_str) = match edit.kind {
            InlineEditKind::Rename => (FileOpKind::Rename, edit.parent.display().to_string()),
            InlineEditKind::NewFile => (FileOpKind::CreateFile, edit.parent.display().to_string()),
            InlineEditKind::NewDir => (FileOpKind::CreateDir, edit.parent.display().to_string()),
        };
        let result = tako_control::dispatch(
            self,
            Request::FileOp {
                op,
                path: path_str,
                name: Some(name.clone()),
                pane: None,
            },
            PaneOrigin::User,
        );
        if result.is_ok() {
            // #550 × #559: ドット始まりを作ったのに非表示設定で消える（= 何も起きて
            // いないように見える）のを防ぐ。明示的に作った物は必ず見せる
            if filetree::is_hidden_name(&name) && !self.filetree.show_hidden() {
                self.toggle_hidden_files(cx);
            }
            // #559: 2 秒ポーリングを待たず、作った項目を正しい並び順の位置へ即座に出す
            let dir = match edit.kind {
                InlineEditKind::Rename => edit
                    .parent
                    .parent()
                    .map(|p| p.to_path_buf())
                    .unwrap_or_else(|| edit.parent.clone()),
                _ => edit.parent.clone(),
            };
            self.filetree.refresh_dir(&dir);
        }
        self.sync_filetree_roots();
        cx.notify();
    }

    /// コンテキストメニューのアクション実行（FR-3.12）
    pub(crate) fn handle_context_action(
        &mut self,
        action: &str,
        path: &std::path::Path,
        _is_dir: bool,
        cx: &mut Context<Self>,
    ) {
        use tako_control::protocol::{FileOpKind, Request};
        let path_str = path.display().to_string();
        match action {
            "copy-abs" => {
                if let Ok(result) = tako_control::dispatch(
                    self,
                    Request::FileOp {
                        op: FileOpKind::CopyAbsolutePath,
                        path: path_str,
                        name: None,
                        pane: None,
                    },
                    PaneOrigin::User,
                ) {
                    if let Some(p) = result["path"].as_str() {
                        cx.write_to_clipboard(ClipboardItem::new_string(p.to_string()));
                    }
                }
            }
            "copy-rel" => {
                let pane = self.focused_pane().as_u64();
                if let Ok(result) = tako_control::dispatch(
                    self,
                    Request::FileOp {
                        op: FileOpKind::CopyRelativePath,
                        path: path_str,
                        name: None,
                        pane: Some(pane),
                    },
                    PaneOrigin::User,
                ) {
                    if let Some(p) = result["path"].as_str() {
                        cx.write_to_clipboard(ClipboardItem::new_string(p.to_string()));
                    }
                }
            }
            "reveal" => {
                let _ = tako_control::dispatch(
                    self,
                    Request::FileOp {
                        op: FileOpKind::Reveal,
                        path: path_str,
                        name: None,
                        pane: None,
                    },
                    PaneOrigin::User,
                );
            }
            "open-term" => {
                let pane = self.focused_pane().as_u64();
                let _ = tako_control::dispatch(
                    self,
                    Request::FileOp {
                        op: FileOpKind::OpenTerminal,
                        path: path_str,
                        name: None,
                        pane: Some(pane),
                    },
                    PaneOrigin::User,
                );
            }
            "rename" | "new-file" | "new-dir" => {
                let init_text = if action == "rename" {
                    path.file_name()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .into_owned()
                } else {
                    String::new()
                };
                let cursor = init_text.len();
                self.inline_edit = Some(InlineEdit {
                    parent: if action == "rename" || path.is_dir() {
                        path.to_path_buf()
                    } else {
                        path.parent().unwrap_or(path).to_path_buf()
                    },
                    kind: match action {
                        "rename" => InlineEditKind::Rename,
                        "new-file" => InlineEditKind::NewFile,
                        _ => InlineEditKind::NewDir,
                    },
                    text: init_text,
                    cursor,
                });
                if action != "rename" {
                    if let Some(parent_path) = if path.is_dir() {
                        Some(path.to_path_buf())
                    } else {
                        path.parent().map(|p| p.to_path_buf())
                    } {
                        self.filetree.expand_dir(&parent_path);
                    }
                }
            }
            "trash" => {
                let _ = tako_control::dispatch(
                    self,
                    Request::FileOp {
                        op: FileOpKind::Trash,
                        path: path_str,
                        name: None,
                        pane: None,
                    },
                    PaneOrigin::User,
                );
                self.sync_filetree_roots();
            }
            "open-default" => {
                let _ = tako_control::dispatch(
                    self,
                    Request::FileOp {
                        op: FileOpKind::OpenDefault,
                        path: path_str,
                        name: None,
                        pane: None,
                    },
                    PaneOrigin::User,
                );
            }
            "open-with" => {
                let path_owned = path.to_path_buf();
                cx.spawn(async move |_, _| {
                    let _ = pick_app_and_open(&path_owned);
                })
                .detach();
            }
            "toggle-hidden" => {
                self.toggle_hidden_files(cx);
            }
            "remove-root" => {
                let _ = tako_control::dispatch(
                    self,
                    Request::TreeFolder {
                        action: "remove".into(),
                        path: Some(path_str),
                        tab: None,
                        pane: None,
                    },
                    PaneOrigin::User,
                );
                self.sync_filetree_roots();
            }
            _ => {}
        }
        cx.notify();
    }

    /// ファイルツリーのファイル行クリック → プレビューペインで開く（FR-3.2）。
    /// CLI / MCP（`tako open` / `tako_open_file`）と同じ dispatch 経路を通す
    /// （開発不変条件の UI 側の一貫性。OpenFile はセッション起動を伴わないため
    /// pending_attach の後処理は不要）
    pub(crate) fn open_file_row(&mut self, path: &std::path::Path, cx: &mut Context<Self>) {
        let pane = self.focused_pane().as_u64();
        let result = tako_control::dispatch(
            self,
            tako_control::protocol::Request::OpenFile {
                pane: Some(pane),
                path: path.display().to_string(),
                mode: None,
                direction: None,
                focus: Some(true),
            },
            PaneOrigin::User,
        );
        if let Err(e) = result {
            eprintln!("warning: ファイルを開けない: {e}");
        }
        self.drain_pending_highlights(cx);
        cx.notify();
    }

    /// プレビューの「コード ⇔ Markdown」トグル（目アイコン。FR-3.3）。
    /// 同じ状態は dispatch（OpenFile の mode 指定）= CLI / MCP からも切り替えられる。
    /// Image / Pdf モードではトグルしない
    pub(crate) fn toggle_preview_mode(&mut self, pane_id: PaneId, cx: &mut Context<Self>) {
        if self
            .preview_edits
            .get(&pane_id)
            .is_some_and(preview::EditState::dirty)
        {
            if let Some(edit) = self.preview_edits.get_mut(&pane_id) {
                edit.message = Some(crate::ui_text::sidebar::note_save_before_mode_switch().into());
            }
            cx.notify();
            return;
        }
        self.preview_edits.remove(&pane_id);
        let Some(state) = self.previews.get(&pane_id) else {
            return;
        };
        let mode = match state.mode {
            preview::PreviewMode::Code => preview::PreviewMode::Markdown,
            preview::PreviewMode::Markdown => preview::PreviewMode::Code,
            preview::PreviewMode::Image
            | preview::PreviewMode::Pdf
            | preview::PreviewMode::Video => return,
        };
        let path = state.path.clone();
        if mode == preview::PreviewMode::Markdown {
            self.previews
                .insert(pane_id, preview::PreviewState::loading(&path, mode));
            self.spawn_preview_load(pane_id, path, mode, cx);
        } else {
            let (new_state, raw) = preview::load_fast(&path, mode);
            self.previews.insert(pane_id, new_state);
            if let Some(text) = raw {
                self.spawn_highlight(pane_id, path, text, cx);
            }
        }
        cx.notify();
    }

    /// syntect ハイライトを background executor で実行し、完了後にプレビューを差し替える
    pub(crate) fn spawn_highlight(
        &self,
        pane: PaneId,
        path: std::path::PathBuf,
        text: String,
        cx: &mut Context<Self>,
    ) {
        cx.spawn(async move |this, cx| {
            let p = path.clone();
            let task = cx
                .background_executor()
                .spawn(async move { preview::highlight_text(&p, &text) });
            let lines = task.await;
            let _ = this.update(cx, |app, cx| {
                if let Some(state) = app.previews.get_mut(&pane) {
                    if state.path == path {
                        state.content = preview::PreviewContent::Code(lines);
                        cx.notify();
                    }
                }
            });
        })
        .detach();
    }

    /// UI から直接 dispatch した場合の pending_highlights を処理する
    pub(crate) fn drain_pending_highlights(&mut self, cx: &mut Context<Self>) {
        for (pane, path, text) in std::mem::take(&mut self.pending_highlights) {
            self.spawn_highlight(pane, path, text, cx);
        }
        self.drain_pending_preview_loads(cx);
    }

    /// Markdown 目次構築・PDF ラスタライズ・動画 ffmpeg を background executor で
    /// 読み込み、完了後に Loading プレースホルダを本内容へ差し替える（#168 / #232）
    pub(crate) fn spawn_preview_load(
        &self,
        pane: PaneId,
        path: std::path::PathBuf,
        mode: preview::PreviewMode,
        cx: &mut Context<Self>,
    ) {
        let logical_width = self
            .pane_text_areas
            .iter()
            .find(|(id, _)| *id == pane)
            .map(|(_, bounds)| f32::from(bounds.size.width) - (PANE_PADDING + 4.0) * 2.0)
            .unwrap_or(612.0);
        let pdf_raster_key =
            preview::PdfRasterKey::for_view(self.preview_device_scale, 1.0, logical_width);
        cx.spawn(async move |this, cx| {
            let p = path.clone();
            let task = cx.background_executor().spawn(async move {
                match mode {
                    preview::PreviewMode::Pdf => preview::load_pdf_with_key(&p, pdf_raster_key),
                    preview::PreviewMode::Markdown => {
                        preview::load_for_reload(&p, mode, None).state
                    }
                    _ => preview::load_fast(&p, mode).0,
                }
            });
            let state = task.await;
            let _ = this.update(cx, |app, cx| {
                // 読み込み中に別ファイルへ差し替わっていたら破棄（後勝ち）
                let still_loading = app.previews.get(&pane).is_some_and(|s| {
                    s.path == path && matches!(s.content, preview::PreviewContent::Loading)
                });
                if still_loading {
                    app.previews.insert(pane, state);
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// dispatch 中に積まれた重量プレビュー読み込みを background へ流す（Issue #168）
    pub(crate) fn drain_pending_preview_loads(&mut self, cx: &mut Context<Self>) {
        for (pane, path, mode) in std::mem::take(&mut self.pending_preview_loads) {
            self.spawn_preview_load(pane, path, mode, cx);
        }
    }

    /// 表示中かつ対応形式のパスだけを親ディレクトリの非再帰監視へ同期する。
    /// render からは呼ばず、open / close / 設定切替時だけ実行する。
    /// BG 退避中のプレビューは監視対象から除外する（#230）。
    pub(crate) fn sync_preview_watches(&mut self) {
        let _span = tako_control::diag::perf_span("preview_watch_sync");
        let paths: Vec<std::path::PathBuf> = if self.preview_reload.enabled() {
            self.previews
                .iter()
                .filter(|(pane_id, state)| {
                    preview::live_reload_supported(state.mode)
                        && !self.workspace.is_shelved(**pane_id)
                })
                .map(|(_, state)| state.path.clone())
                .collect()
        } else {
            Vec::new()
        };
        let keep: std::collections::HashSet<_> = paths.iter().cloned().collect();
        self.pending_preview_reloads
            .retain(|path, _| keep.contains(path));
        self.active_preview_reloads
            .retain(|path| keep.contains(path));
        self.preview_reload_generations
            .retain(|path, _| keep.contains(path));
        if let Some(watcher) = self.preview_file_watcher.as_mut() {
            if watcher.sync_paths(paths).is_err() {
                eprintln!("warning: プレビューの監視対象を更新できない");
            }
        }
    }

    pub(crate) fn handle_preview_watch_signal(
        &mut self,
        signal: preview_watch::PreviewWatchSignal,
        cx: &mut Context<Self>,
    ) {
        let _span = tako_control::diag::perf_span("preview_watch_event");
        if !self.preview_reload.enabled() {
            return;
        }
        let paths = match signal {
            preview_watch::PreviewWatchSignal::Paths(paths) => paths,
            preview_watch::PreviewWatchSignal::Rescan => self
                .previews
                .values()
                .filter(|state| preview::live_reload_supported(state.mode))
                .map(|state| state.path.clone())
                .collect(),
        };
        for path in paths {
            self.schedule_preview_reload(path, cx);
        }
    }

    fn schedule_preview_reload(&mut self, path: std::path::PathBuf, cx: &mut Context<Self>) {
        let now = std::time::Instant::now();
        self.next_preview_reload_generation = self.next_preview_reload_generation.wrapping_add(1);
        self.preview_reload_generations
            .insert(path.clone(), self.next_preview_reload_generation);
        self.pending_preview_reloads.insert(path.clone(), now);
        if !self.active_preview_reloads.insert(path.clone()) {
            return;
        }

        cx.spawn(async move |this, cx| loop {
            cx.background_executor()
                .timer(preview_watch::RELOAD_DEBOUNCE)
                .await;
            let ready = this.update(cx, |app, cx| {
                let Some(last_event) = app.pending_preview_reloads.get(&path).copied() else {
                    app.active_preview_reloads.remove(&path);
                    return true;
                };
                if last_event.elapsed() < preview_watch::RELOAD_DEBOUNCE {
                    return false;
                }
                app.pending_preview_reloads.remove(&path);
                app.active_preview_reloads.remove(&path);
                let Some(generation) = app.preview_reload_generations.get(&path).copied() else {
                    return true;
                };
                let targets: Vec<_> = app
                    .previews
                    .iter()
                    .filter(|(_, state)| {
                        state.path == path && preview::live_reload_supported(state.mode)
                    })
                    .map(|(pane, state)| {
                        let pdf_key = match &state.content {
                            preview::PreviewContent::Pdf(data) => Some(data.raster_key),
                            _ => None,
                        };
                        (*pane, state.mode, pdf_key, state.file_stamp)
                    })
                    .collect();
                for (pane, mode, pdf_key, old_stamp) in targets {
                    app.spawn_preview_reload(
                        pane,
                        path.clone(),
                        mode,
                        pdf_key,
                        old_stamp,
                        generation,
                        cx,
                    );
                }
                true
            });
            match ready {
                Ok(true) | Err(_) => break,
                Ok(false) => {}
            }
        })
        .detach();
    }

    #[allow(clippy::too_many_arguments)]
    fn spawn_preview_reload(
        &mut self,
        pane: PaneId,
        path: std::path::PathBuf,
        mode: preview::PreviewMode,
        pdf_key: Option<preview::PdfRasterKey>,
        old_stamp: Option<preview::FileStamp>,
        generation: u64,
        cx: &mut Context<Self>,
    ) {
        let job_key = (pane, path.clone());
        if !self.active_preview_reload_jobs.insert(job_key.clone()) {
            return;
        }
        cx.spawn(async move |this, cx| {
            let reload_path = path.clone();
            let loaded = cx
                .background_executor()
                .spawn(async move {
                    // mtime + size が変わっていなければ再ロードをスキップする。
                    // テキストは source_bytes 比較があるが PDF / 画像には無いため、
                    // ファイルスタンプで不要な再ラスタライズを防ぐ。(#257)
                    if let Some(old) = old_stamp {
                        if preview::FileStamp::from_path(&reload_path) == Some(old) {
                            return None;
                        }
                    }
                    Some(preview::load_for_reload(&reload_path, mode, pdf_key))
                })
                .await;
            let _ = this.update(cx, |app, cx| {
                app.active_preview_reload_jobs.remove(&job_key);
                if let Some(loaded) = loaded {
                    app.apply_preview_reload(pane, &path, mode, generation, loaded, cx);
                }
                let Some(latest_generation) = app.preview_reload_generations.get(&path).copied()
                else {
                    return;
                };
                if latest_generation == generation
                    || !app.preview_reload.enabled()
                    || !app
                        .previews
                        .get(&pane)
                        .is_some_and(|state| state.path == path && state.mode == mode)
                {
                    return;
                }
                let Some((pdf_key, old_stamp)) = app.previews.get(&pane).map(|state| {
                    let pdf_key = match &state.content {
                        preview::PreviewContent::Pdf(data) => Some(data.raster_key),
                        _ => None,
                    };
                    (pdf_key, state.file_stamp)
                }) else {
                    return;
                };
                app.spawn_preview_reload(
                    pane,
                    path.clone(),
                    mode,
                    pdf_key,
                    old_stamp,
                    latest_generation,
                    cx,
                );
            });
        })
        .detach();
    }

    fn apply_preview_reload(
        &mut self,
        pane: PaneId,
        path: &std::path::Path,
        mode: preview::PreviewMode,
        generation: u64,
        loaded: preview::ReloadedPreview,
        cx: &mut Context<Self>,
    ) {
        let _span = tako_control::diag::perf_span("preview_reload_apply");
        if !self.preview_reload.enabled()
            || self.preview_reload_generations.get(path) != Some(&generation)
            || !self
                .previews
                .get(&pane)
                .is_some_and(|state| state.path == path && state.mode == mode)
        {
            return;
        }

        if let Some(edit) = self
            .preview_edits
            .get_mut(&pane)
            .filter(|edit| edit.editing || edit.dirty())
        {
            // 自分自身の保存でも OS イベントは発生する。同じ内容なら競合にせず、
            // 真の外部変更または削除だけを既存 FR-3.5 の競合表示へ接続する。
            if loaded.source_bytes.as_deref() == Some(edit.buffer.text().as_bytes()) {
                return;
            }
            edit.save_status = Some(preview::SaveStatus::Conflict);
            edit.message = Some(crate::ui_text::sidebar::note_external_change().into());
            cx.notify();
            return;
        }

        self.preview_edits.remove(&pane);
        self.preview_selections.remove(&pane);
        self.preview_line_bounds.remove(&pane);
        self.preview_pdf_char_bounds.remove(&pane);
        self.preview_pdf_highlight_paint_count.remove(&pane);
        self.preview_text_layouts.remove(&pane);
        self.preview_line_texts.remove(&pane);
        // preview_image_cache は除去しない。次フレームの ensure_preview_image_cache が
        // 新旧の path / raster_key を比較して自動更新する。旧キャッシュは新キャッシュ
        // 構築開始まで表示に使い、差し替え時に #258 の LRU / GPUI eviction へ送る。
        // これにより暗転（空 div フォールバック）と旧資源残留を同時に防ぐ。(#257 / #258)
        self.pending_pdf_rasters.remove(&pane);
        self.previews.insert(pane, loaded.state);
        // Code Runner: リロードで tako:run 宣言の追加・削除を反映する（#453）
        self.detect_preview_run_profiles(pane, path);
        self.preview_reload_apply_count = self.preview_reload_apply_count.saturating_add(1);
        cx.notify();
    }
}

/// OS のアプリ選択ダイアログでアプリを選び、指定ファイルをそのアプリで開く。
/// プラットフォーム差は境界 B8（`platform::os_integration`）の内側にある
fn pick_app_and_open(path: &std::path::Path) -> Result<(), String> {
    use tako_control::platform::os_integration as os;
    let app = os::pick_application()?;
    os::open_with(&app.to_string_lossy(), path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn row(path: &str, depth: usize, root: bool, is_dir: bool) -> filetree::Row {
        let path = PathBuf::from(path);
        filetree::Row {
            entry: filetree::Entry {
                name: path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default(),
                path,
                is_dir,
            },
            depth,
            expanded: true,
            root,
            git_status: None,
        }
    }

    /// 木の形（`filetree` のソート = ディレクトリ先 → 名前順を再現）:
    /// ```text
    /// /r                 depth 0 root
    ///   sub              depth 1 dir
    ///     deep           depth 2 dir
    ///       x.txt        depth 3 file
    ///     a.txt          depth 2 file
    ///     z.txt          depth 2 file
    ///   other.txt        depth 1 file
    /// ```
    fn sample_rows() -> Vec<filetree::Row> {
        vec![
            row("/r", 0, true, true),
            row("/r/sub", 1, false, true),
            row("/r/sub/deep", 2, false, true),
            row("/r/sub/deep/x.txt", 3, false, false),
            row("/r/sub/a.txt", 2, false, false),
            row("/r/sub/z.txt", 2, false, false),
            row("/r/other.txt", 1, false, false),
        ]
    }

    /// #559: 新規ファイルの入力欄は作成先の子の**ファイル群の先頭**（VSCode と同じ）。
    /// 展開済み子孫を全部飛ばした末尾ではない
    #[test]
    fn 新規ファイルの入力欄はファイル群の先頭に入る() {
        let rows = sample_rows();
        let slot = inline_insert_position(&rows, std::path::Path::new("/r/sub"), false).unwrap();
        assert_eq!(slot.parent_index, 1);
        assert_eq!(slot.row_index, 4, "deep とその子孫を飛ばし a.txt の手前");
        assert_eq!(slot.depth, 2, "sub の子と同じ深さ");

        // ルート見出しを作成先にした場合は深さ 1・sub を飛ばして other.txt の手前
        let slot = inline_insert_position(&rows, std::path::Path::new("/r"), false).unwrap();
        assert_eq!((slot.parent_index, slot.row_index, slot.depth), (0, 6, 1));

        // 折りたたみ中などで作成先が行に無ければ入力欄は出さない
        assert!(inline_insert_position(&rows, std::path::Path::new("/r/none"), false).is_none());
    }

    /// #559: 新規フォルダの入力欄は作成先の**真下**（ディレクトリ群の先頭）
    #[test]
    fn 新規フォルダの入力欄は作成先の真下に入る() {
        let rows = sample_rows();
        let slot = inline_insert_position(&rows, std::path::Path::new("/r/sub"), true).unwrap();
        assert_eq!((slot.parent_index, slot.row_index, slot.depth), (1, 2, 2));

        let slot = inline_insert_position(&rows, std::path::Path::new("/r"), true).unwrap();
        assert_eq!((slot.parent_index, slot.row_index, slot.depth), (0, 1, 1));
    }

    /// 子がまったく無い（空 / 折りたたみ済み）作成先でも真下に入る
    #[test]
    fn 子が無い作成先でも直下に入る() {
        let rows = vec![row("/r", 0, true, true), row("/r/empty", 1, false, true)];
        for new_is_dir in [true, false] {
            let slot = inline_insert_position(&rows, std::path::Path::new("/r/empty"), new_is_dir)
                .unwrap();
            assert_eq!((slot.row_index, slot.depth), (2, 2));
        }
    }
}
