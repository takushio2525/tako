use gpui::{
    div, point, prelude::*, px, BoxShadow, Context, FontWeight, Keystroke, MouseButton,
    SharedString,
};
use tako_core::PaneId;

use super::*;

impl TakoApp {
    pub(crate) fn sync_filetree_roots(&mut self) {
        if !self.filetree.visible {
            return;
        }
        let mut roots: Vec<std::path::PathBuf> = Vec::new();
        let active_tab_id = self.workspace.active_tab().id();

        // フォアグラウンドペイン
        for pane in self.workspace.active_tab().tree().panes() {
            let Some(cwd) = self.terminals.get(&pane.id()).and_then(|s| s.cwd()) else {
                continue;
            };
            let cwd = cwd.to_path_buf();
            if !roots.contains(&cwd) {
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
            if !roots.contains(&cwd) {
                roots.push(cwd);
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
        let tab_title = self.workspace.active_tab().title().to_string();
        let sidebar_path = self.active_tab_cwd().map(|p| {
            if let Ok(home) = std::env::var("HOME") {
                if let Ok(rel) = p.strip_prefix(&home) {
                    return format!("~/{}", rel.display());
                }
            }
            p.display().to_string()
        });
        // プレビュー表示中のファイル（開いている行を控えめにハイライトする）
        let open_paths: std::collections::HashSet<std::path::PathBuf> =
            self.previews.values().map(|p| p.path.clone()).collect();
        let mut rows = self.filetree.rows();
        // 新規ファイル/フォルダ用の仮行を親の直後に挿入
        let inline_new_insert = match &self.inline_edit {
            Some(edit) if edit.kind != InlineEditKind::Rename => {
                let parent = &edit.parent;
                // 親ディレクトリの子の末尾（展開済み子孫をすべて飛ばした直後）に挿入
                let insert_pos =
                    rows.iter()
                        .position(|r| r.entry.path == *parent)
                        .map(|parent_idx| {
                            let parent_depth = rows[parent_idx].depth;
                            let mut end = parent_idx + 1;
                            while end < rows.len() && rows[end].depth > parent_depth {
                                end += 1;
                            }
                            end
                        });
                insert_pos.map(|pos| {
                    let depth = rows
                        .get(pos.saturating_sub(1))
                        .filter(|r| r.entry.path == *parent)
                        .map(|r| r.depth + 1)
                        .unwrap_or_else(|| {
                            rows.get(pos.saturating_sub(1))
                                .map(|r| r.depth)
                                .unwrap_or(1)
                        });
                    (pos, depth)
                })
            }
            _ => None,
        };
        if let (Some((pos, depth)), Some(edit)) = (inline_new_insert, self.inline_edit.as_ref()) {
            rows.insert(
                pos,
                filetree::Row {
                    entry: filetree::Entry {
                        path: edit.parent.join("__inline_new__"),
                        name: String::new(),
                        is_dir: edit.kind == InlineEditKind::NewDir,
                    },
                    depth,
                    expanded: false,
                    root: false,
                    git_status: None,
                },
            );
        }
        let inline_edit_snapshot = self.inline_edit.clone();
        Some(
            div()
                .w(px(SIDEBAR_WIDTH))
                .h_full()
                .flex()
                .flex_col()
                .bg(rgba(theme.mantle))
                .border_r_1()
                .border_color(hsla(theme.border_subtle))
                .text_size(px(12.0))
                .text_color(hsla(theme.foreground))
                .overflow_hidden()
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(px(6.0))
                        .h(px(38.0))
                        .px(px(12.0))
                        .flex_none()
                        .child(
                            div()
                                .w(px(14.0))
                                .h(px(11.0))
                                .flex_none()
                                .relative()
                                .child(
                                    div()
                                        .absolute()
                                        .top(px(0.0))
                                        .left(px(0.0))
                                        .w(px(6.0))
                                        .h(px(4.0))
                                        .rounded_t(px(1.5))
                                        .bg(hsla(theme.accent)),
                                )
                                .child(
                                    div()
                                        .absolute()
                                        .top(px(3.0))
                                        .left(px(0.0))
                                        .w(px(14.0))
                                        .h(px(8.0))
                                        .rounded(px(1.5))
                                        .bg(hsla(theme.accent)),
                                ),
                        )
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .overflow_hidden()
                                .child(
                                    div()
                                        .text_size(px(12.0))
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(hsla(theme.text_secondary))
                                        .child(SharedString::from(truncate(&tab_title, 20))),
                                )
                                .children(sidebar_path.map(|path| {
                                    div()
                                        .text_size(px(10.5))
                                        .font_family("Monaco")
                                        .text_color(hsla(theme.tab_inactive_foreground))
                                        .overflow_hidden()
                                        .whitespace_nowrap()
                                        .text_ellipsis()
                                        .child(SharedString::from(truncate(&path, 28)))
                                })),
                        ),
                )
                .child(
                    div()
                        .id("filetree-list")
                        .flex_1()
                        .flex()
                        .flex_col()
                        .overflow_y_scroll()
                        .children(rows.into_iter().enumerate().map(|(index, row)| {
                            let path = row.entry.path.clone();
                            let is_dir = row.entry.is_dir;
                            // インライン編集中の行を検出
                            let is_inline = match &inline_edit_snapshot {
                                Some(edit) if edit.kind == InlineEditKind::Rename => {
                                    path == edit.parent
                                }
                                Some(edit) if path == edit.parent.join("__inline_new__") => true,
                                _ => false,
                            };
                            if let (true, Some(edit)) = (is_inline, inline_edit_snapshot.as_ref()) {
                                let depth = row.depth;
                                let indent = 8.0 + 12.0 * depth as f32;
                                let icon = match edit.kind {
                                    InlineEditKind::Rename => {
                                        if is_dir {
                                            "🗂 "
                                        } else {
                                            ""
                                        }
                                    }
                                    InlineEditKind::NewFile => "📄 ",
                                    InlineEditKind::NewDir => "🗂 ",
                                };
                                let before_cursor = &edit.text[..edit.cursor];
                                let after_cursor = &edit.text[edit.cursor..];
                                return div()
                                    .id(("filetree-row", index as u64))
                                    .flex()
                                    .flex_row()
                                    .items_center()
                                    .w_full()
                                    .px_1()
                                    .pl(px(indent))
                                    .bg(rgba_alpha(theme.tab_active_background, 0.8))
                                    .child(SharedString::from(icon.to_string()))
                                    .child(
                                        div()
                                            .flex_1()
                                            .flex()
                                            .flex_row()
                                            .border_1()
                                            .border_color(hsla(theme.accent))
                                            .rounded_sm()
                                            .px(px(2.0))
                                            .bg(rgba(theme.background))
                                            .child(SharedString::from(before_cursor.to_string()))
                                            .child(
                                                div()
                                                    .w(px(1.0))
                                                    .h(px(14.0))
                                                    .bg(hsla(theme.foreground))
                                                    .flex_none(),
                                            )
                                            .child(SharedString::from(after_cursor.to_string())),
                                    );
                            }
                            let is_open = !is_dir && open_paths.contains(&path);
                            let drag_path = path.clone();
                            let base = div()
                                .id(("filetree-row", index as u64))
                                .flex()
                                .flex_row()
                                .items_center()
                                .w_full()
                                .py(px(1.0))
                                .cursor_pointer()
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
                                        move |this, e: &MouseDownEvent, _, cx| {
                                            cx.stop_propagation();
                                            this.context_menu = Some(ContextMenu {
                                                path: ctx_path.clone(),
                                                is_dir,
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
                                        format!(
                                            "{} {}",
                                            if is_dir { "🗂" } else { "📄" },
                                            truncate(&row.entry.name, 24)
                                        ),
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
                                let indent = 12.0 + 12.0 * row.depth as f32;
                                let mut row_el = base
                                    .pl(px(indent))
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
                ),
        )
    }

    /// コンテキストメニューの描画（FR-3.12）
    pub(crate) fn render_context_menu(&self, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        let ctx = self.context_menu.as_ref()?;
        let theme = &self.theme;
        let path = ctx.path.clone();
        let is_dir = ctx.is_dir;
        let pos = ctx.position;
        let items: Vec<(&str, &str)> = vec![
            ("copy-rel", "相対パスをコピー"),
            ("copy-abs", "絶対パスをコピー"),
            ("reveal", "Finder で表示"),
            ("open-term", "ターミナルで開く"),
            ("sep1", ""),
            ("rename", "名前変更"),
            ("new-file", "新しいファイル"),
            ("new-dir", "新しいフォルダ"),
            ("sep2", ""),
            ("trash", "削除"),
        ];
        let menu = div()
            .absolute()
            .left(pos.x)
            .top(pos.y)
            .w(px(180.0))
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
        let _ = tako_control::dispatch(
            self,
            Request::FileOp {
                op,
                path: path_str,
                name: Some(name),
                pane: None,
            },
            PaneOrigin::User,
        );
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
        let (new_state, raw) = preview::load_fast(&path, mode);
        self.previews.insert(pane_id, new_state);
        if let Some(text) = raw {
            self.spawn_highlight(pane_id, path, text, cx);
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
    }
}
