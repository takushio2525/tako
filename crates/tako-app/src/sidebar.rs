use gpui::{
    div, point, prelude::*, px, BoxShadow, Context, FontWeight, Keystroke, MouseButton,
    SharedString,
};
use tako_core::PaneId;

use super::*;
use crate::platform::user_work::UserWork;

/// 新規作成のインライン入力欄を表す仮行のファイル名（#559）。
/// 行の判定は挿入位置（index）で行うのでパスは表示にも一致判定にも使わない
const INLINE_NEW_MARKER: &str = "__tako_inline_new__";

/// 1 階層ぶんのインデント幅（カンプ: margin-left 17px）
const INDENT_STEP: f32 = 17.0;

/// インライン入力に入る名前の上限（バイト。#1725）。OS の名前の上限（255 バイト）より
/// 十分大きく取り、超えた名前は作成時に OS が理由つきで断る（#1399 の通知欄に出る）。
/// ここは巨大な貼り付けで描画が詰まらないための歯止めだけ
const TREE_NAME_MAX_BYTES: usize = 1024;

/// インライン入力の 1 桁の幅（px。#1725）。ツリーの字（12px の等幅）の半角の送り幅。
/// 全角はこの 2 桁ぶんとして数える（実際の送り幅より広めに見積もる = はみ出さない側）
const INLINE_CELL_PX: f32 = 7.2;

/// 入力欄の中身以外が行の中で取る幅（px。#1725）: chevron の空き 14 + アイコン 16 +
/// 子のあいだの gap 4×2 + 右の余白 6 + 入力欄の左右 padding 3×2 + 枠 1×2
const INLINE_ROW_CHROME_PX: f32 = 52.0;

/// 全角（2 桁）として数える文字か（#1725。表示窓の概算用で、端末の桁計算とは別物）
fn inline_is_wide(c: char) -> bool {
    matches!(c as u32,
        0x1100..=0x115F       // ハングル字母
        | 0x2E80..=0xA4CF     // CJK 部首・記号・かな・漢字
        | 0xAC00..=0xD7A3     // ハングル音節
        | 0xF900..=0xFAFF     // CJK 互換漢字
        | 0xFE30..=0xFE4F     // CJK 互換形
        | 0xFF00..=0xFF60     // 全角英数・記号
        | 0xFFE0..=0xFFE6     // 全角記号
        | 0x1F300..=0x1FAFF   // 絵文字（ファイル名に来うる）
        | 0x20000..=0x3FFFD   // CJK 拡張
    )
}

/// 桁数（半角 = 1 / 全角 = 2）
fn inline_cells(s: &str) -> usize {
    s.chars().map(inline_char_cells).sum()
}

fn inline_char_cells(c: char) -> usize {
    if inline_is_wide(c) {
        2
    } else {
        1
    }
}

/// インライン入力に**見せる**ぶんを切り出す（#1725。GPUI 非依存の純粋関数）。
///
/// 入力欄は行の残り幅しか無いので、長い名前（深い階層のリネーム・長い日本語名）を
/// そのまま並べるとキャレットと変換中の読みが入力欄の外へ押し出され、見えない上に
/// 変換候補窓もサイドバーの外（ターミナルの上）に出る。そこで
///
/// - 変換中の読み（`marked`）とキャレットは**必ず**見せる
/// - キャレットの前は**末尾から**入るぶんだけ、後ろは**先頭から**残りに入るぶんだけ
/// - 切ったほうには `…` を付ける（切れていることを見せる）
///
/// を `cells` 桁（半角 = 1 / 全角 = 2）の中で決める。文字の途中では切らない
pub(crate) fn inline_input_window(
    before: &str,
    marked: &str,
    after: &str,
    cells: usize,
) -> (String, String) {
    // キャレットの 1 桁ぶんを先に取る
    let mut budget = cells.saturating_sub(1).saturating_sub(inline_cells(marked));
    let before_vis: String = if inline_cells(before) <= budget {
        budget -= inline_cells(before);
        before.to_string()
    } else {
        // `…` の 1 桁を差し引いてから末尾を詰める
        let room = budget.saturating_sub(1);
        let mut used = 0;
        let mut tail: Vec<char> = Vec::new();
        for c in before.chars().rev() {
            let w = inline_char_cells(c);
            if used + w > room {
                break;
            }
            used += w;
            tail.push(c);
        }
        budget = 0;
        std::iter::once('…').chain(tail.into_iter().rev()).collect()
    };
    let after_vis: String = if inline_cells(after) <= budget {
        after.to_string()
    } else if budget == 0 {
        String::new()
    } else {
        let room = budget.saturating_sub(1);
        let mut used = 0;
        let mut head = String::new();
        for c in after.chars() {
            let w = inline_char_cells(c);
            if used + w > room {
                break;
            }
            used += w;
            head.push(c);
        }
        head.push('…');
        head
    };
    (before_vis, after_vis)
}

/// インライン入力の対象がいまのツリーに出ているか（#1725。GPUI 非依存の純粋関数）。
///
/// ツリーが閉じている・対象（作成先 / 名前を変える項目）がどのルートの配下にも無い、の
/// どちらでも入力欄は描かれない。描かれない入力欄が打鍵と変換を握ると
/// 「押しても何も起きない」になるので、打鍵の振り分けと変換の宛先はこの判定を共有する
pub(crate) fn inline_edit_target_visible(
    tree_visible: bool,
    roots: &[std::path::PathBuf],
    target: &std::path::Path,
) -> bool {
    tree_visible && roots.iter().any(|root| target.starts_with(root))
}

// ─────────────── git ステータスの見せ方（#1009） ───────────────
//
// 「何色にするか」は分類 1 つだけで決める（`TreeGitState`）。分類そのもの
// （伝播・無視・ステージ済み / 未ステージの切り分け）は `tako_core::git_tree` が正本で、
// ここは色を当てるだけ。CLI / MCP も同じ分類を返すので、画面と応答がずれない。
//
// 色の割り当ては git 自身の `status --short` の読み方に合わせてある:
//   - **バッジ 2 桁 = git の XY**（例: `MM` = ステージ済みの変更 + その後の未ステージ変更）
//   - **左（index 側 = ステージ済み）は緑**、右（worktree 側 = 未ステージ）は種別の色
//   - ディレクトリは文字ではなく**配下の変更件数**（VSCode と同じ）

/// 分類 → 色。UI に出る色はここ 1 箇所でしか決めない
pub(crate) fn git_state_color(state: filetree::TreeGitState, theme: &Theme) -> tako_core::Rgb {
    match state {
        // `.gitignore` 対象は「変更ではない」ので、いちばん薄い文字色へ落とす
        filetree::TreeGitState::Ignored => theme.text_muted,
        // 新規（未追跡 / index 追加済み）は緑
        filetree::TreeGitState::Untracked | filetree::TreeGitState::Added => theme.green,
        filetree::TreeGitState::Modified => theme.yellow,
        // リネームは accent（選択色）と紛れるので mauve
        filetree::TreeGitState::Renamed => theme.mauve,
        // 削除と未解決は「止まって見てほしい」側なので赤
        filetree::TreeGitState::Deleted | filetree::TreeGitState::Conflicted => theme.red,
    }
}

/// 行の名前の色。git 情報が無い行は `base`（従来の色）のまま = 非 git フォルダで
/// 見た目が変わらない
pub(crate) fn git_name_color(
    status: Option<filetree::TreeGitStatus>,
    base: tako_core::Rgb,
    theme: &Theme,
) -> tako_core::Rgb {
    match status {
        None => base,
        Some(s) => git_state_color(s.state, theme),
    }
}

/// 行の右端に出すバッジ。無視・状態なしは何も出さない（`None`）
fn git_badge_spans(status: Option<filetree::TreeGitStatus>, theme: &Theme) -> Option<gpui::Div> {
    let status = status?;
    if status.state == filetree::TreeGitState::Ignored {
        return None;
    }
    let mut badge = div()
        .flex()
        .flex_row()
        .flex_none()
        .items_center()
        .text_size(px(10.5))
        .font_family("Monaco")
        .font_weight(FontWeight::BOLD)
        .pr(px(8.0));
    if status.from_children {
        // ディレクトリ: 配下の変更ファイル数。自分自身の変更ではないので少し落とす
        return Some(
            badge.child(
                div()
                    .text_color(hsla_alpha(git_state_color(status.state, theme), 0.8))
                    .child(SharedString::from(status.badge())),
            ),
        );
    }
    if let Some(c) = status.staged {
        badge = badge.child(
            div()
                .text_color(hsla(theme.green))
                .child(SharedString::from(c.to_string())),
        );
    }
    if let Some(c) = status.unstaged {
        badge = badge.child(
            div()
                .text_color(hsla(git_state_color(status.state, theme)))
                .child(SharedString::from(c.to_string())),
        );
    }
    Some(badge)
}

/// 切り取り中の行の不透明度（#1860。VSCode のエクスプローラーと同じく薄く描く）
pub(crate) const TREE_CUT_OPACITY: f32 = 0.45;

/// ファイルツリーで選んでいる行（⌘C / ⌘X / ⌘V の対象。FR-3.34 / #1860）。
///
/// 行を押す（左 / 右クリック）と立ち、**選んだ時点のタブとフォーカスペインを覚える**。
/// どちらかが動いた・ツリーを閉じた・ツリーの外を押した・ほかのキーを打った、の
/// どれかで効かなくなり、⌘C / ⌘V はペイン（端末のコピー・貼り付け）へ戻る
/// （Windows の Ctrl+C を端末の SIGINT から奪いっぱなしにしない）。
///
/// #1867 で複数選択（⌘クリックで足す / 外す・⇧クリックで範囲）になった。`path` 以下の 4 つは
/// **最後に押した行**（貼り付けの宛先・右クリックの基準）、`paths` が選んでいる行すべて
/// （状態遷移の正本は `tako_core::tree_select`）
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TreeSelection {
    pub(crate) path: std::path::PathBuf,
    pub(crate) is_dir: bool,
    /// ワークスペースのフォルダの見出し行（切り取りは断る = #1834 と同じ方針）
    pub(crate) root: bool,
    /// リモート（SSH）の行（キーは理由を出して断る）
    pub(crate) remote: bool,
    pub(crate) tab: TabId,
    pub(crate) pane: PaneId,
    /// 選んでいる行すべて（ツリーの並び順。1 行なら `[path]`。#1867）
    pub(crate) paths: Vec<std::path::PathBuf>,
    /// ⇧クリックの起点（#1867）
    pub(crate) anchor: std::path::PathBuf,
}

impl TreeSelection {
    pub(crate) fn contains(&self, path: &std::path::Path) -> bool {
        self.paths.iter().any(|p| p == path)
    }

    /// 2 行以上を選んでいる（#1867）
    pub(crate) fn is_multi(&self) -> bool {
        self.paths.len() > 1
    }

    pub(crate) fn as_core(&self) -> tako_core::tree_select::Selection {
        tako_core::tree_select::Selection {
            items: self.paths.clone(),
            anchor: self.anchor.clone(),
            lead: self.path.clone(),
        }
    }
}

/// 複数選択の押下前の選択とコピーの進み具合の帯（FR-3.38 / #1867）
#[derive(Debug, Default)]
pub(crate) struct TreeMulti {
    /// 押下の捕捉フェーズで外した選択（⌘クリックで足す・⇧クリックの起点・選んだ行を掴んで
    /// まとめて運ぶ・右クリックメニューの対象の基準）。押下のたびに上書きされるので、
    /// ツリーの外を押した後の ⌘クリックは何も足さない
    pub(crate) stash: Option<TreeSelection>,
    /// 進み具合の帯を描き直す刻みが回っている
    ticking: bool,
}

/// コピーの進み具合の帯を出し始めるまでの時間（短いコピーで帯をちらつかせない。#1867）
const COPY_PROGRESS_DELAY: std::time::Duration = std::time::Duration::from_millis(300);
/// 帯を描き直す刻み
const COPY_PROGRESS_TICK: std::time::Duration = std::time::Duration::from_millis(150);
/// 帯の「取り消し」の実矩形を行の矩形採取（`tree_row_probe`）へ載せるときの名前
/// （実在のパスと取り違えない相対の名前。visual-test `tree-multiselect` が押す位置の正）
pub(crate) const COPY_CANCEL_PROBE: &str = "<copy-cancel>";
/// 帯の残り時間の目安の実矩形（#1895。出ているときだけ載る = visual-test `tree-keyboard-copy` が見る）
pub(crate) const COPY_ETA_PROBE: &str = "<copy-eta>";

/// いま見えているローカルの行の並び（⇧クリックの範囲・見えない行を外す基準。#1867）
pub(crate) fn tree_visible_order(rows: &[filetree::Row]) -> Vec<std::path::PathBuf> {
    tree_row_shapes(rows).into_iter().map(|r| r.path).collect()
}

/// いま見えているローカルの行の形（キーでの選択 = `tako_core::tree_select::on_key` の材料。
/// [`tree_visible_order`] と同じ行 = リモート（SSH）の行と説明行は載せない。#1908）
pub(crate) fn tree_row_shapes(rows: &[filetree::Row]) -> Vec<tako_core::tree_select::RowShape> {
    rows.iter()
        .filter(|r| r.remote.is_none() && r.note.is_none())
        .map(|r| tako_core::tree_select::RowShape {
            path: r.entry.path.clone(),
            is_dir: r.entry.is_dir,
            expanded: r.expanded,
            root: r.root,
        })
        .collect()
}

/// コピーの断りが「取り消した」か（文面の正本は `tako_core::file_copy::CopyRefusal::Cancelled`）
pub(crate) fn is_copy_cancelled(text: &str) -> bool {
    text.contains(&tako_core::file_copy::CopyRefusal::Cancelled.reason())
}

/// バイト数の表記（帯に出す。1024 刻み・小数 1 桁）
pub(crate) fn format_bytes(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit + 1 < UNITS.len() {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

/// 帯の棒の長さ（0〜1）。数えている間は None（母数が決まっていない）。
/// バイトがあればバイト、無ければ件数で決める（空のフォルダだけのコピーでも進む）
pub(crate) fn copy_ratio(snap: &tako_core::file_copy::ProgressSnapshot) -> Option<f32> {
    if snap.counting {
        return None;
    }
    let (done, total) = if snap.bytes_total > 0 {
        (snap.bytes_done, snap.bytes_total)
    } else {
        (snap.entries_done, snap.entries_total)
    };
    if total == 0 {
        return Some(0.0);
    }
    Some((done as f32 / total as f32).clamp(0.0, 1.0))
}

/// 押した行の修飾（押し下げた時点のもの。キーで押したクリックは修飾なし）
fn click_modifiers(e: &gpui::ClickEvent) -> gpui::Modifiers {
    match e {
        gpui::ClickEvent::Mouse(m) => m.down.modifiers,
        gpui::ClickEvent::Keyboard(_) => gpui::Modifiers::default(),
    }
}

/// 修飾から行の押し方（正本は `tako_core::tree_select::click_kind`）
pub(crate) fn tree_click_kind(m: gpui::Modifiers) -> tako_core::tree_select::ClickKind {
    tako_core::tree_select::click_kind(
        tako_core::platform::support::Platform::current(),
        m.platform,
        m.control,
        m.shift,
    )
}

/// 右クリックメニューの項目 id → コピー / 切り取り / 貼り付け（#1860）
fn tree_clip_menu_key(id: &str) -> Option<tako_core::platform::keys::TreeClipKey> {
    use tako_core::platform::keys::TreeClipKey;
    match id {
        "clip-cut" => Some(TreeClipKey::Cut),
        "clip-copy" => Some(TreeClipKey::Copy),
        "clip-paste" => Some(TreeClipKey::Paste),
        "clip-paste-move" => Some(TreeClipKey::PasteMove),
        _ => None,
    }
}

/// ツリー内のドラッグで、いまカーソルが載っている行と判定（FR-3.32 / #1834）
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TreeDropHover {
    /// カーソルが載っている行
    pub(crate) row: std::path::PathBuf,
    /// その行がリモート（SSH）の行か（パスの字面が同じローカル行と取り違えない）
    pub(crate) row_remote: bool,
    /// 落とし先のフォルダ（ファイル行の上なら親フォルダ）
    pub(crate) dest: std::path::PathBuf,
    /// ドラッグしている項目
    pub(crate) src: std::path::PathBuf,
    /// 判定（`tako_core::file_move::drop_verdict` = dispatch と同じ規則）
    pub(crate) verdict: tako_core::file_move::DropVerdict,
}

/// ドラッグしている項目を行 `row` の上へ持ってきたときの判定（行をまたいだときだけ呼ぶ）。
///
/// 掴んだ行そのものの上では何も出さない（`None`）。そこで離すのは取り消しで、
/// 「自分自身へは移せない」を毎回出すと、ペインへ運ぶ途中にも赤い札が付いて回る
pub(crate) fn tree_drop_hover(
    src: &std::path::Path,
    src_root: bool,
    row: &std::path::Path,
    row_is_dir: bool,
    row_remote: bool,
) -> Option<TreeDropHover> {
    use tako_core::file_move::{drop_verdict, DragItem};
    if row == src && !row_remote {
        return None;
    }
    // ファイル行の上 = そのファイルのあるフォルダへ（VSCode と同じ。狙う行が広くなる）
    let dest = if row_is_dir || row_remote {
        row.to_path_buf()
    } else {
        row.parent()?.to_path_buf()
    };
    let verdict = drop_verdict(
        DragItem {
            path: src,
            workspace_root: src_root,
            remote: false,
        },
        &dest,
        row_remote,
        |to| std::fs::symlink_metadata(to).is_ok(),
    );
    Some(TreeDropHover {
        row: row.to_path_buf(),
        row_remote,
        dest,
        src: src.to_path_buf(),
        verdict,
    })
}

/// まとめてドラッグしているときの判定（#1867）。`items` = （運んでいるもの, 見出しの行か）、
/// `grabbed` = 掴んだ行。掴んだ行そのものの上では何も出さない（離すと取り消し）。
/// ほかの選んだ行の上は「自分自身へ」で断る（判定の正本は `file_move::drop_verdict_many`）
pub(crate) fn tree_drop_hover_many(
    items: &[(std::path::PathBuf, bool)],
    grabbed: &std::path::Path,
    row: &std::path::Path,
    row_is_dir: bool,
    row_remote: bool,
) -> Option<TreeDropHover> {
    use tako_core::file_move::{drop_verdict_many, DragItem};
    if row == grabbed && !row_remote {
        return None;
    }
    let dest = if row_is_dir || row_remote {
        row.to_path_buf()
    } else {
        row.parent()?.to_path_buf()
    };
    let drag_items: Vec<DragItem<'_>> = items
        .iter()
        .map(|(path, root)| DragItem {
            path,
            workspace_root: *root,
            remote: false,
        })
        .collect();
    let verdict = drop_verdict_many(&drag_items, &dest, row_remote, |to| {
        std::fs::symlink_metadata(to).is_ok()
    });
    Some(TreeDropHover {
        row: row.to_path_buf(),
        row_remote,
        dest,
        src: grabbed.to_path_buf(),
        verdict,
    })
}

/// 行ごとの強調の種類（[`tree_drop_mark`] が決める）
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum TreeDropMark {
    /// 落とし先のフォルダの行（「ここへ移動」の札）
    Target,
    /// 落とし先のフォルダの中に見えている行（薄く塗る = どこへ入るかの範囲）
    Inside,
    /// 移せない行（理由の札）
    Refused(tako_core::file_move::MoveRefusal),
}

/// 行（`row` / `remote`）にどの強調を出すか（純関数。描画は写すだけ）
pub(crate) fn tree_drop_mark(
    hover: Option<&TreeDropHover>,
    row: &std::path::Path,
    remote: bool,
) -> Option<TreeDropMark> {
    use tako_core::file_move::DropVerdict;
    let hover = hover?;
    match &hover.verdict {
        DropVerdict::Move if !remote && row == hover.dest => Some(TreeDropMark::Target),
        DropVerdict::Move if !remote && row.starts_with(&hover.dest) => Some(TreeDropMark::Inside),
        DropVerdict::Refused(refusal) if row == hover.row && remote == hover.row_remote => {
            Some(TreeDropMark::Refused(refusal.clone()))
        }
        _ => None,
    }
}

/// ツリー行に移動の強調を重ねる（FR-3.32 / #1834）。
///
/// **行の中身を組み終えた最後に**重ねる（開いているファイルの塗り・ホバーの塗りより
/// 前に置くと上書きされて見えない）。縁取りは内側の影、札は絶対配置なので、
/// 行の高さも並びも 1px も動かさない（ドラッグ中に行が跳ねない）
fn with_tree_drop_mark(
    el: gpui::Stateful<gpui::Div>,
    mark: Option<TreeDropMark>,
    theme: &Theme,
) -> gpui::Stateful<gpui::Div> {
    let Some(mark) = mark else {
        return el;
    };
    let outline = |color: tako_core::Rgb| {
        vec![BoxShadow {
            color: hsla(color),
            offset: point(px(0.), px(0.)),
            blur_radius: px(0.),
            spread_radius: px(1.),
            inset: true,
        }]
    };
    let (el, pill) = match &mark {
        TreeDropMark::Target => (
            el.bg(rgba_alpha(theme.accent, 0.24))
                .shadow(outline(theme.accent)),
            Some((
                theme.accent,
                None,
                crate::ui_text::sidebar::move_here().to_string(),
            )),
        ),
        TreeDropMark::Inside => (el.bg(rgba_alpha(theme.accent, 0.07)), None),
        TreeDropMark::Refused(refusal) => (
            el.bg(rgba_alpha(theme.red, 0.16))
                .shadow(outline(theme.red)),
            Some((
                theme.red,
                Some(file_icons::ui_icon::FAIL_X),
                crate::ui_text::sidebar::move_refused(refusal),
            )),
        ),
    };
    let Some((color, icon, text)) = pill else {
        return el;
    };
    el.relative().child(
        div()
            .absolute()
            .top_0()
            .bottom_0()
            .right(px(4.0))
            .flex()
            .items_center()
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(3.0))
                    .px(px(5.0))
                    .rounded(px(4.0))
                    .bg(rgba(theme.mantle))
                    .border_1()
                    .border_color(hsla(color))
                    .text_size(px(10.0))
                    .text_color(hsla(color))
                    .children(icon.map(|path| {
                        svg()
                            .path(path)
                            .size(px(9.0))
                            .flex_none()
                            .text_color(hsla(color))
                    }))
                    .child(SharedString::from(text)),
            ),
    )
}

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

/// 失敗の通知を出した**画面**（#1417）。
///
/// #1399 の出し口はファイルツリー専用だったが、同じ形（dispatch の `Err` を
/// 捨てる）は右パネルの tmux window 行とプレビューの目次 / ページ移動にも在った。
/// 画面ごとに出し口を増やすと「片方だけ無言」が必ず生まれるので、
/// 口は [`TakoApp::notify_ui_failure`] の 1 つに保ち、**画面はこの値だけで区別**する。
///
/// 用途は persist.log の `area=` に出す識別子（どの画面で起きたかを診断から辿れる）。
/// A/B の逃げ道は [`NoticeArm`] が別に持つ（#1422 で軸を分けた。理由はそちらの doc）
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum NoticeArea {
    /// ファイルツリーのローカル行（#1399）
    Tree,
    /// 右パネルの tmux ビュー・バックグラウンドの復元行（#1417 / #1422）
    RightPanel,
    /// プレビューペインの目次 / ページ移動・コードのコピー・Code Runner（#1417 / #1422）
    Preview,
    /// 下部ドロワー（バックグラウンドのカード・ペインへの D&D 復帰。#1432）
    Drawer,
    /// AI コマンド提案カード（コピー / 新規ペインで実行 / 閉じる。#1432）
    CommandCard,
    /// GUI 表示モードのチャットビュー（発話・コードブロックのコピー。#1432）
    Chat,
    /// Finder の「このアプリケーションで開く」・`tako open` の受け口（#1432）。
    /// 画面ではなく**入口**だが、失敗は同じ共有の通知欄へ出る
    OpenFile,
    /// アップデート専用ウィンドウのリリースノート（#1432）。
    /// この窓は自前の通知欄を持たないので、出し先はメインウィンドウの共有通知欄
    UpdateWindow,
    /// `tako` CLI / MCP の受け口（IPC サーバー）の起動（#1441）。
    /// 画面ではなく**起動時の一段**だが、失敗すると「見た目は正常なのに
    /// AI からは何も操作できない」ので同じ共有の通知欄へ出す
    Ipc,
    /// ユーザー向けタスク（#1450）。**ここだけは成功系**
    /// （`notify_ui_info` = 新しく人の手を待つものが増えたことの通知）で、
    /// 出し先と抑止の作法は失敗通知と同じ 1 実装を共有する
    UserTasks,
    /// SSH ペインの自動再接続（#1446）。**押した操作ではなく回線の事故**だが、
    /// 「切れたのに繋ぎ直さない」は黙って起きてはいけないので同じ通知欄へ出す
    SshPane,
    /// 蓋閉じ継続の安全弁（#1473）。**ここも成功系**（バッテリー残量・温度で
    /// 自動解除したことの申告）。黙って解除すると「蓋を閉じたら止まっていた」に
    /// なるので、バッテリー継続を選んだ人にだけ理由つきで出す
    SleepGuard,
    /// リモート daemon の自動復帰（#1485）。**押した操作ではなく起動時の一段**だが、
    /// 「起動していたのに戻らない」は黙って起きてはいけないので同じ通知欄へ出す
    /// （成功は出さない = チップが running になるので分かる）
    RemoteAutostart,
    /// ステータスバーのボタン（#1945 の自動復帰の一括 ON / OFF）。
    /// 押した操作の失敗と、全体の既定を settings.json へ残せなかったことを出す
    StatusBar,
}

impl NoticeArea {
    /// persist.log へ出す画面の識別子（本文を含まない ASCII 名。`DispatchError::class()` と同じ作法）
    pub(crate) fn tag(self) -> &'static str {
        match self {
            NoticeArea::Tree => "tree",
            NoticeArea::RightPanel => "right_panel",
            NoticeArea::Preview => "preview",
            NoticeArea::Drawer => "drawer",
            NoticeArea::CommandCard => "command_card",
            NoticeArea::Chat => "chat",
            NoticeArea::SshPane => "ssh_pane",
            NoticeArea::OpenFile => "open_file",
            NoticeArea::UpdateWindow => "update_window",
            NoticeArea::Ipc => "ipc",
            NoticeArea::UserTasks => "user_tasks",
            NoticeArea::SleepGuard => "sleep_guard",
            NoticeArea::RemoteAutostart => "remote_autostart",
            NoticeArea::StatusBar => "status_bar",
        }
    }
}

/// 同一バイナリのまま旧挙動（失敗を捨てて無言）へ戻す A/B の逃げ道（#1399 / #1417 / #1422）。
///
/// **画面（[`NoticeArea`]）とは別の軸**にしてある。#1417 までは画面から env を選んで
/// いたが、#1422 で**同じ画面に別の Issue で足した通知が同居**した（右パネルは
/// window 切替 = #1417 と復元ボタン = #1422 がどちらも `right_panel`）。画面で env を
/// 選ぶと、片方のアームを立てたときにもう片方の通知まで消えて **A/B が互いの回帰を
/// 隠す**ので、抑止の単位は「どの Issue で足した通知か」にする
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum NoticeArm {
    /// ファイルツリーのローカル操作（`TAKO_1399_LEGACY`）
    Issue1399,
    /// 右パネルの window 切替 / プレビューの目次・ページ移動（`TAKO_1417_LEGACY`）
    Issue1417,
    /// tmux セッションの復元・バックグラウンド復帰・コードのコピー・Code Runner・
    /// PDF 再ラスタライズ（`TAKO_1422_LEGACY`）
    Issue1422,
    /// ドロワーの復帰・コマンドカード・チャットのコピー・Finder から開く・
    /// リリースノートのリンク（`TAKO_1432_LEGACY`）
    Issue1432,
    /// IPC の受け口が立たなかったときの申告（`TAKO_1441_LEGACY`）
    Issue1441,
    /// ユーザー向けタスクの起票通知（`TAKO_1450_LEGACY`）
    Issue1450,
    /// SSH ペインの追跡の永続・引き取りと「繋ぎ直さない理由」（`TAKO_1446_LEGACY`）
    Issue1446,
    /// 右パネル tasks ビューの操作の失敗（`TAKO_1450B2_LEGACY`）。
    /// B1 の [`NoticeArm::Issue1450`]（起票通知）とは**別の軸**にしてある
    /// （同じ画面に別の Issue で足した通知が同居するときの作法 = #1422）
    Issue1450B2,
    /// 蓋閉じ継続の安全弁による解除・再適用（`TAKO_1473_LEGACY`）。
    /// 判定そのものの A/B と同じ env を使う（旧挙動ではバッテリー継続に
    /// 入らないので、通知だけ残っても意味がない）
    Issue1473,
    /// リモート daemon の自動復帰（`TAKO_1485_LEGACY`）。判定そのものの A/B と
    /// 同じ env を使う（旧挙動では自動復帰しないので、通知だけ残っても意味がない）
    Issue1485,
    /// 右パネル diagnostics ビューの行を押した失敗（`TAKO_1007_LEGACY`）。LSP ごと止める
    /// A/B と同じ env を使う（旧挙動ではビューに行が出ないので、通知だけ残っても意味がない）
    Issue1679,
    /// 定義ジャンプ（⌘クリック）の未導入・未応答・落ちた等（`TAKO_1680_LEGACY`）。
    /// 旧挙動では ⌘クリックが定義を探さないので、通知だけ残っても意味がない
    Issue1680,
    /// ホバーのカードのリンクを開けなかった（`TAKO_1681_LEGACY`）。旧挙動ではカードが出ないので、
    /// 通知だけ残っても意味がない
    Issue1681,
    /// 自動復帰の一括ボタンの失敗・既定を保存できなかった（`TAKO_1945_LEGACY`）。
    /// 旧挙動ではボタンが無いので、通知だけ残っても意味がない
    Issue1945,
}

impl NoticeArm {
    /// この Issue で足した通知を旧挙動（何も出さない）へ戻す A/B が立っているか
    pub(crate) fn suppressed(self) -> bool {
        match self {
            NoticeArm::Issue1399 => TakoApp::legacy_1399(),
            NoticeArm::Issue1417 => TakoApp::legacy_1417(),
            NoticeArm::Issue1422 => TakoApp::legacy_1422(),
            NoticeArm::Issue1432 => TakoApp::legacy_1432(),
            NoticeArm::Issue1441 => TakoApp::legacy_1441(),
            NoticeArm::Issue1450 => TakoApp::legacy_1450(),
            NoticeArm::Issue1446 => TakoApp::legacy_1446(),
            NoticeArm::Issue1450B2 => crate::tasks_panel::legacy_1450_b2(),
            NoticeArm::Issue1473 => tako_control::sleep_guard::legacy_1473(),
            NoticeArm::Issue1485 => tako_control::remote_autostart::legacy_mode(),
            NoticeArm::Issue1679 => tako_control::lsp::legacy(),
            NoticeArm::Issue1680 => tako_control::dispatch::lsp_goto_legacy(),
            NoticeArm::Issue1681 => tako_control::lsp::hover::legacy(),
            NoticeArm::Issue1945 => tako_core::limit_resume_all::legacy(),
        }
    }
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

        let active_tab_id = self.workspace.active_tab().id();

        // フォアグラウンドペイン → 同タブ由来のバックグラウンドペインの順に cwd を拾い、
        // #134 の明示追加フォルダを後ろへ合流する。デデュープ（#171: cwd と pinned の
        // 重複排除）と HOME フォールバックの規則は `tako_core::sidebar::workspace_roots`
        // が正本で、**CLI / MCP の `tree git-status` も同じ関数を通る**（#1009）
        let mut pane_cwds: Vec<std::path::PathBuf> = Vec::new();
        for pane in self.workspace.active_tab().tree().panes() {
            if let Some(cwd) = self.terminals.get(&pane.id()).and_then(|s| s.cwd()) {
                pane_cwds.push(cwd.to_path_buf());
            }
        }
        for bp in self.workspace.shelved_panes() {
            if bp.origin_tab() != active_tab_id {
                continue;
            }
            if let Some(cwd) = self.terminals.get(&bp.id()).and_then(|s| s.cwd()) {
                pane_cwds.push(cwd.to_path_buf());
            }
        }
        let roots = tako_core::sidebar::workspace_roots(
            pane_cwds,
            self.workspace.active_tab().pinned_folders().to_vec(),
            tako_core::paths::home_dir(),
        );
        self.filetree.set_roots(roots);

        // #919: リモート（SSH 先）のルートはアクティブタブが持つものに合わせる。
        // ローカルの `roots` とは別の器（`PathBuf` に POSIX パスを混ぜない）
        let want: Vec<tako_core::remote_fs::RemoteFolder> =
            self.workspace.active_tab().remote_folders().to_vec();
        let have = self.filetree.remote_roots().to_vec();
        for gone in have.iter().filter(|f| !want.contains(f)) {
            self.filetree.remove_remote_root(&gone.remote);
        }
        // #976: タブは「最後に開いたものが先頭」で持つので、**逆順に足して**
        // ツリー側を「開いた順（古いものが上）」へ揃える。ローカルルートが
        // ペインの並び順で出るのと同じ規則にするため。
        // #1041: 前後どちらへ出すかは描画時に経路から決まるので、ここは経路を
        // そのまま運ぶだけ（既にあるものは経路の変化だけ反映する）
        for folder in want.iter().rev() {
            if have.contains(folder) {
                self.filetree
                    .set_remote_root_origin(&folder.remote, folder.origin);
            } else {
                self.filetree.add_remote_root(folder.clone());
            }
        }
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
            let short = tako_core::paths::shorten_home(&full);
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
        // #1867: 複数選んだ行を掴んだらその全部を運ぶ（見えている行だけ。行ごとに引き直さない）
        let multi_drag: Option<Vec<std::path::PathBuf>> = self
            .active_tree_selection()
            .filter(|sel| sel.is_multi() && !sel.remote)
            .and_then(|sel| sel.as_core().retain_visible(&tree_visible_order(&rows)))
            .map(|sel| sel.items);
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
                    remote: None,
                    note: None,
                },
            );
        }
        let inline_edit_snapshot = self.inline_edit.clone();
        // #1725: 入力欄の中身（未確定文字列とキャレット）はコミット欄・ブランチ欄と同じ
        // 共有部品で組む。キャレットの実矩形は変換候補窓の位置出しに使われる
        // （`bounds_for_range`）。入力欄は 1 つしか出ないので行の中で 1 度だけ取り出す
        let mut inline_marked = self.text_input_marked_at(AppTextInput::TreeName, &theme, 12.0);
        // 見える窓の見積もりに使う読み（描く要素は上の共有部品。ここは幅を数えるだけ）
        let inline_marked_text: String = self
            .ime
            .as_ref()
            .filter(|ime| ime.app_input == Some(AppTextInput::TreeName))
            .map(|ime| ime.text.clone())
            .unwrap_or_default();
        let mut inline_caret = Some(self.text_input_caret(AppTextInput::TreeName, &theme));
        // 入力欄の実矩形の採取（`tree_row_probe` のときだけ。項目 154 が使う）
        let mut inline_rect_slot = self
            .tree_row_probe
            .then(|| self.tree_inline_input_rect.clone());
        // #1725: 行の実矩形の採取（セルフテストが立てたフレームだけ。本番は None で要素を増やさない）
        let row_rects = self.tree_row_probe.then(|| {
            self.tree_row_rects.borrow_mut().clear();
            self.tree_row_rects.clone()
        });
        // #919: 期限切れの成功通知は落とす（失敗は expired() が false なので残る）
        if self.remote_notice.as_ref().is_some_and(|n| n.expired()) {
            self.remote_notice = None;
        }
        let remote_notice = self
            .remote_notice
            .as_ref()
            .map(|n| (n.text.clone(), n.is_error));
        let copy_progress = self.render_copy_progress(&theme, cx);
        // #789: 親（root render）が渡す幅と同じ実効幅を使う（要求値ではない）
        let sidebar_w = self.effective_sidebar_width();
        let drop_highlight = self.sidebar_drop_highlight;
        // #1834: ツリーの行からドラッグしている間だけ、行へ移動の受け口を付ける。
        // gpui のドラッグが外部要因（Esc・窓の外で離す）で消えたフレームでは判定を畳む
        let tree_drag_live = cx.has_active_drag() && self.drag_kind == Some(DragKind::File);
        if !cx.has_active_drag() {
            self.tree_drop = None;
        }
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
                                        .child(SharedString::from(truncate_chars(&project_name, 18))),
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
                                        .child(SharedString::from(truncate_chars(&g.branch, 14)))
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
                // #1867: コピーの進み具合（件数・バイト・取り消し）
                .children(copy_progress)
                // #919: リモート操作の通知（成功は数秒、失敗は次の操作まで残す）。
                // 汎用のトーストが無いので、ユーザーが見ている場所へ出す
                .children(remote_notice.map(|(text, is_error)| {
                    div()
                        .id("remote-notice")
                        .flex_none()
                        .px(px(10.0))
                        .py(px(5.0))
                        .text_size(px(11.0))
                        .border_b_1()
                        .border_color(hsla_alpha(theme.pane_border, 0.6))
                        .bg(rgba_alpha(
                            if is_error { theme.red } else { theme.green },
                            0.12,
                        ))
                        .text_color(hsla(if is_error {
                            theme.red
                        } else {
                            theme.foreground
                        }))
                        .cursor_pointer()
                        .on_click(cx.listener(|this, _: &gpui::ClickEvent, _, cx| {
                            this.remote_notice = None;
                            cx.notify();
                        }))
                        .child(SharedString::from(text))
                }))
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
                            // リモート（SSH 先）の行は**ローカル FS の操作を一切通さない**
                            // ので、インライン編集・git マーカー・D&D・Finder 系メニューの
                            // どれにも入る前に分岐する（#919）。`entry.path` は
                            // リモートの POSIX パスを載せた見せかけの PathBuf なので、
                            // ここから下の `canonicalize` / `join` に触れさせない
                            if row.remote.is_some() {
                                let el = self.render_remote_row(index, &row, &theme, cx);
                                // #1834: リモートの行は移せない・移し先にもならない（理由を出す）。
                                // 情報行（読み込み中・失敗）は押せない行なので飾らない
                                if row.note.is_some() {
                                    return el;
                                }
                                return self.decorate_tree_row(el, &row, tree_drag_live, &theme, cx);
                            }
                            // #1398: ローカルの情報行（シンボリックリンクの循環で
                            // 展開を打ち切った説明）。押せない行なので、インライン編集・
                            // 右クリック・D&D のどれにも入る前に分岐する
                            if row.note.is_some() {
                                return Self::render_note_row(index, &row, &theme);
                            }
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
                                let (before_full, after_full) = edit.field.split_at_caret();
                                // 行の残り幅（通常行と同じインデント規則）から見える桁数を出し、
                                // キャレットと変換中の読みが入力欄の中に収まる窓を切り出す
                                let left_px = if row.depth >= 1 {
                                    INDENT_STEP * row.depth as f32 + 14.0
                                } else {
                                    12.0
                                };
                                let cells = ((sidebar_w - left_px - INLINE_ROW_CHROME_PX)
                                    / INLINE_CELL_PX)
                                    .floor()
                                    .max(4.0) as usize;
                                let (before_cursor, after_cursor) = inline_input_window(
                                    before_full,
                                    &inline_marked_text,
                                    after_full,
                                    cells,
                                );
                                let composing = inline_marked.is_some();
                                // 未確定文字列があれば空欄の案内は出さない（変換中の読みが案内と重なる）
                                let empty = edit.field.text().is_empty() && !composing;
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
                                            .id("filetree-inline-input")
                                            .flex_1()
                                            .min_w(px(0.0))
                                            // 見積もりの誤差ではみ出しても行の外へ描かない
                                            .overflow_hidden()
                                            .relative()
                                            .when_some(inline_rect_slot.take(), |d, slot| {
                                                d.child(
                                                    canvas(
                                                        move |bounds, _, _| slot.set(Some(bounds)),
                                                        |_, _, _, _| (),
                                                    )
                                                    .absolute()
                                                    .top_0()
                                                    .left_0()
                                                    .size_full(),
                                                )
                                            })
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
                                            // #1725: 入力欄の外を押したら取り消す（捕捉フェーズで
                                            // 受けるので、押した先が伝播を止める要素でも取りこぼさない）。
                                            // 打鍵の戻り先は押した先が決める（ペインを押せばそのペイン）
                                            .on_mouse_down_out(cx.listener(
                                                |this, _: &gpui::MouseDownEvent, _, cx| {
                                                    if this.inline_edit.is_some()
                                                        && !TakoApp::legacy_1725()
                                                    {
                                                        this.close_inline_edit(false);
                                                        cx.notify();
                                                    }
                                                },
                                            ))
                                            .child(SharedString::from(before_cursor))
                                            .children(inline_marked.take())
                                            .children(inline_caret.take())
                                            .when(empty, |d| {
                                                d.child(
                                                    div()
                                                        .pl(px(3.0))
                                                        .text_color(hsla(theme.text_muted))
                                                        .child(SharedString::from(
                                                            placeholder.to_string(),
                                                        )),
                                                )
                                            })
                                            .child(SharedString::from(after_cursor)),
                                    );
                            }
                            let is_open = !is_dir && open_paths.contains(&path);
                            // #559: 作成先の親ディレクトリ行を強調し「ここの直下に作る」を明示する
                            let is_inline_parent =
                                inline_parent_path.as_ref().is_some_and(|p| *p == path);
                            let drag_path = path.clone();
                            // #1867: 選んでいる行を掴んだら選んだもの全部（ゴーストは件数）
                            let (drag_paths, drag_label) = match &multi_drag {
                                Some(all) if all.contains(&path) => (
                                    all.clone(),
                                    crate::ui_text::sidebar::drag_items(all.len()),
                                ),
                                _ => (vec![path.clone()], truncate_chars(&row.entry.name, 24)),
                            };
                            let base = div()
                                .id(("filetree-row", index as u64))
                                .flex()
                                .flex_row()
                                .items_center()
                                .py(px(1.0))
                                .cursor_pointer()
                                // 何も描かない矩形採取（#1725。`tree_row_probe` のときだけ）
                                .when_some(row_rects.clone(), |d, rects| {
                                    let probe_path = path.clone();
                                    d.relative().child(
                                        canvas(
                                            move |bounds, _, _| {
                                                rects.borrow_mut().push((probe_path, bounds))
                                            },
                                            |_, _, _, _| (),
                                        )
                                        .absolute()
                                        .top_0()
                                        .left_0()
                                        .size_full(),
                                    )
                                })
                                .when(is_inline_parent, |d| {
                                    d.bg(rgba_alpha(theme.accent, 0.16))
                                        .text_color(hsla(theme.foreground))
                                })
                                .hover(|d| d.bg(rgba(theme.surface_hover)))
                                // #1867: 選んでいる行を掴んだら選択を保つ（捕捉フェーズで外れた選択を
                                // 戻す = 続けてドラッグするとまとめて運ぶ。離しただけならクリックが
                                // その行だけを選び直す = Finder と同じ）
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener({
                                        let ctx_path = path.clone();
                                        move |this, e: &MouseDownEvent, _, cx| {
                                            if tree_click_kind(e.modifiers).is_plain()
                                                && this.keep_tree_selection_for(&ctx_path)
                                            {
                                                cx.notify();
                                            }
                                        }
                                    }),
                                )
                                .on_click(cx.listener({
                                    let ctx_path = path.clone();
                                    move |this, e: &gpui::ClickEvent, _, cx| {
                                        // #1867: ⌘クリック（Windows は Ctrl）= 足す / 外す・⇧クリック =
                                        // 範囲。開いたり開閉したりはしない（選ぶだけ = Finder と同じ）
                                        let kind = tree_click_kind(click_modifiers(e));
                                        if !kind.is_plain() {
                                            this.click_tree_row(&ctx_path, kind);
                                            cx.notify();
                                            return;
                                        }
                                        if is_dir {
                                            this.filetree.toggle_dir(&ctx_path);
                                        } else {
                                            this.open_file_row(&ctx_path, cx);
                                        }
                                        // #1860: 開いた後のフォーカスペインで選ぶ（開いたプレビューへ
                                        // フォーカスが移っても ⌘C はこの行へ向く）
                                        this.select_tree_row(&ctx_path, is_dir, false);
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
                                                // 保存側（B26 経由）と同じ形で比べる。
                                                // 素の canonicalize だと Windows では
                                                // verbatim になり一致しない（#970）
                                                let canon =
                                                    tako_core::platform::path::canonicalize_or_self(
                                                        &ctx_path,
                                                    );
                                                this.workspace
                                                    .active_tab()
                                                    .pinned_folders()
                                                    .iter()
                                                    .any(|f| {
                                                        tako_core::platform::path::canonicalize_or_self(f)
                                                            == canon
                                                    })
                                            };
                                            // #1867: 選んでいる行の右クリックは選択を保つ
                                            // （メニューの切り取り / コピー / ごみ箱がまとめて効く）
                                            if !this.keep_tree_selection_for(&ctx_path) {
                                                this.select_tree_row(&ctx_path, is_dir, false);
                                            }
                                            let can_paste = this.tree_can_paste();
                                            this.context_menu = Some(ContextMenu {
                                                path: ctx_path.clone(),
                                                is_dir,
                                                is_pinned_root,
                                                can_paste,
                                                position: e.position,
                                            });
                                            cx.notify();
                                        }
                                    }),
                                )
                                // ファイルは D&D でドロップ位置にプレビューとして開ける（FR-3.11）。
                                // ツリーのフォルダ行へ落とせば移す（FR-3.32 / #1834）
                                .on_drag(
                                    FileDrag {
                                        path: drag_path,
                                        root: row.root,
                                        paths: drag_paths,
                                    },
                                    self.drag_ghost_builder(DragKind::File, drag_label, cx),
                                );
                            let row_el = if row.root {
                                // ワークスペースフォルダの見出し行: 太字 + 上仕切り線（2 つ目以降）
                                base.when(index > 0, |d| {
                                    d.border_t_1()
                                        .border_color(hsla_alpha(theme.pane_border, 0.6))
                                        .mt_1()
                                })
                                .py(px(2.0))
                                .gap(px(4.0))
                                .font_weight(FontWeight::BOLD)
                                .text_color(hsla(git_name_color(
                                    row.git_status,
                                    theme.tab_active_foreground,
                                    &theme,
                                )))
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
                                        .child(SharedString::from(truncate_chars(&row.entry.name, 22))),
                                )
                                // #1009: ワークスペースフォルダ配下の変更件数
                                .children(git_badge_spans(row.git_status, &theme))
                            } else {
                                let git_badge = git_badge_spans(row.git_status, &theme);
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
                                    // #1009: git の状態で名前を色分けする（未変更・
                                    // git 管理外は従来どおりの色のまま）
                                    .text_color(hsla(git_name_color(
                                        row.git_status,
                                        if is_dir {
                                            theme.foreground
                                        } else {
                                            theme.text_tertiary
                                        },
                                        &theme,
                                    )))
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
                                        .child(SharedString::from(truncate_chars(&row.entry.name, 24))),
                                );
                                // git status バッジ（#1009）
                                row_el = row_el.children(git_badge);
                                row_el
                            };
                            // #1834: 移動の強調と受け口は行を組み終えた最後に重ねる
                            self.decorate_tree_row(row_el, &row, tree_drag_live, &theme, cx)
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

    /// 情報行（読み込み中 / 失敗 / 空）の 1 行。**押せない行**。
    ///
    /// リモート行（#919 / #1010）とローカル行（#1398 の展開打ち切り）で同じ形を出す
    /// ための 1 実装。どちらも「黙って空にしない」ための行なので、見え方が
    /// 2 通りに割れると「同じ理由が画面によって違う形で出る」ことになる
    fn render_note_row(
        index: usize,
        row: &filetree::Row,
        theme: &tako_core::theme::Theme,
    ) -> gpui::Stateful<gpui::Div> {
        let base = div()
            .id(("filetree-row", index as u64))
            .flex()
            .flex_row()
            .items_center()
            .py(px(1.0))
            .when(row.depth >= 1, |d| {
                d.ml(px(INDENT_STEP * row.depth as f32))
                    .pl(px(14.0))
                    .children(indent_guides(
                        row.depth,
                        hsla(theme.border_subtle),
                        hsla(theme.border_subtle),
                    ))
            })
            .when(row.depth == 0, |d| d.pl(px(12.0)));
        let Some(note) = &row.note else {
            // 呼び出し側が `row.note.is_some()` で分岐しているので来ないが、
            // **render の中で panic するとアプリごと落ちる**ので空行へ倒す（#828）
            return base;
        };
        let (text, color) = match note {
            filetree::RowNote::Loading => (
                crate::ui_text::remote_folder::row_loading().to_string(),
                theme.text_muted,
            ),
            filetree::RowNote::Empty => (
                crate::ui_text::remote_folder::row_empty().to_string(),
                theme.text_muted,
            ),
            filetree::RowNote::Error(report) => (report.clone(), theme.red),
            // #1402: 切り詰めは**失敗ではない**（上限まで正しく出している）ので
            // red にしない。行があること自体が「続きがある」の合図
            filetree::RowNote::Truncated { shown, total } => (
                crate::ui_text::sidebar::note_truncated(*shown, *total),
                theme.text_muted,
            ),
        };
        // #1010: 読み込み中は回る弧を添える（「止まっている」と区別が付く）
        let loading = matches!(note, filetree::RowNote::Loading);
        // #919: 失敗の理由は 3 行（要約 / 次の一手 / 生の詳細）。サイドバーは狭いので
        // **折り返す**（`whitespace_nowrap` + `text_ellipsis` だと理由が読めない =
        // 静かな失敗に戻ってしまう）。`min_w(0)` は flex の自動最小サイズを外して
        // 折り返し幅を親に合わせるため（#745 と同じ理由で縦積みには効かないので
        // 行方向のここだけに置く）
        base.py(px(2.0))
            .gap(px(4.0))
            .when(loading, |d| {
                d.child(crate::spinner::spinner(
                    ("remote-dir-spin", index as u64),
                    px(11.0),
                    hsla(color),
                ))
            })
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .pr(px(8.0))
                    .text_size(px(11.0))
                    .text_color(hsla(color))
                    .child(SharedString::from(text)),
            )
    }

    /// リモート（SSH 先）ツリーの 1 行（#919）。
    ///
    /// ローカル行と**別の関数**に分けているのは、ローカル行の経路にある
    /// `canonicalize` / `join` / Finder 系メニュー / git マーカー / D&D が
    /// リモートのパスに対して意味を持たないため。分岐を 1 か所に閉じることで
    /// 「リモートのつもりでローカル FS を触る」事故を構造的に防ぐ
    fn render_remote_row(
        &self,
        index: usize,
        row: &filetree::Row,
        theme: &tako_core::theme::Theme,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        // 呼び出し側が `row.remote.is_some()` で分岐しているので None は来ないが、
        // **render の中で panic するとアプリごと落ちる**ので空行へ倒す（#828 の教訓）
        let Some(remote) = row.remote.clone() else {
            return div().id(("filetree-row", index as u64));
        };
        let base = div()
            .id(("filetree-row", index as u64))
            .flex()
            .flex_row()
            .items_center()
            .py(px(1.0))
            .when(row.depth >= 1, |d| {
                d.ml(px(INDENT_STEP * row.depth as f32))
                    .pl(px(14.0))
                    .children(indent_guides(
                        row.depth,
                        hsla(theme.border_subtle),
                        hsla(theme.border_subtle),
                    ))
            })
            .when(row.depth == 0, |d| d.pl(px(12.0)));

        // 状態行（読み込み中 / 失敗 / 空）は押せない情報行。
        // #919 の要点: **失敗を必ず行として見せる**（黙って空にしない）。
        // #1398 でローカル行（展開の打ち切り）も同じ行を出すので 1 実装を共有する
        if row.note.is_some() {
            return Self::render_note_row(index, row, theme);
        }

        let is_dir = row.entry.is_dir;
        // #1010: このファイルをいま SFTP で取得中か
        let loading_file = !is_dir && self.remote_file_loading.contains_key(&remote);
        let is_open = !is_dir
            && self
                .previews
                .values()
                .any(|p| self.preview_remote_origin(p).as_ref() == Some(&remote));
        let mut el = base
            .cursor_pointer()
            .hover(|d| d.bg(rgba(theme.surface_hover)))
            .on_click(cx.listener({
                let remote = remote.clone();
                move |this, _: &gpui::ClickEvent, _, cx| {
                    if is_dir {
                        this.filetree.toggle_remote_dir(&remote);
                    } else {
                        this.open_remote_file_row(&remote, cx);
                    }
                    // #1860: リモートの行でも選ぶ（⌘C などは端末へ流さず理由を出して断る）
                    this.select_tree_row(std::path::Path::new(&remote.path), is_dir, true);
                    cx.notify();
                }
            }))
            .on_mouse_down(
                MouseButton::Right,
                cx.listener({
                    let remote = remote.clone();
                    let is_root = row.root;
                    move |this, e: &MouseDownEvent, _, cx| {
                        cx.stop_propagation();
                        this.remote_context_menu = Some(RemoteContextMenu {
                            remote: remote.clone(),
                            is_dir,
                            is_root,
                            position: e.position,
                        });
                        cx.notify();
                    }
                }),
            )
            // #976: 開いているファイルの強調もローカル行とまったく同じにする
            // （左端の accent 線 + 背景）。片方だけ違うと「別物」に見える
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

        if row.root {
            // #976: リモートルート見出しは**ローカルルートと同じ形**にする
            // （太字 + 仕切り線 + chevron + フォルダアイコン + フォルダ名）。
            // 「SSH のフォルダで、どのホストか」は行末のバッジが担う =
            // 同じ深さに並んだときにローカルと形が揃う
            el = el
                .when(index > 0, |d| {
                    d.border_t_1()
                        .border_color(hsla_alpha(theme.pane_border, 0.6))
                        .mt_1()
                })
                .py(px(2.0))
                .gap(px(4.0))
                .font_weight(FontWeight::BOLD)
                .text_color(hsla(theme.tab_active_foreground))
                .child(
                    svg()
                        .path(file_icons::chevron_icon(row.expanded).svg_path())
                        .size(px(14.0))
                        .flex_none()
                        .text_color(hsla(theme.tab_inactive_foreground)),
                )
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
                        .child(SharedString::from(truncate_chars(&row.entry.name, 22))),
                )
                .child(self.render_ssh_badge(&remote, theme));
            return el;
        }

        el = el.py(px(2.0)).gap(px(4.0));
        if is_dir {
            let folder_color = if row.expanded {
                theme.accent
            } else {
                theme.tab_inactive_foreground
            };
            el = el
                .child(
                    svg()
                        .path(file_icons::chevron_icon(row.expanded).svg_path())
                        .size(px(14.0))
                        .flex_none()
                        .text_color(hsla(theme.tab_inactive_foreground)),
                )
                .child(
                    svg()
                        .path(file_icons::folder_icon(row.expanded).svg_path())
                        .size(px(16.0))
                        .flex_none()
                        .text_color(hsla(folder_color)),
                );
        } else {
            let icon_kind = file_icons::resolve_file_icon(std::path::Path::new(&row.entry.name));
            el = el
                .text_color(hsla(theme.text_tertiary))
                .child(div().w(px(14.0)).flex_none())
                // #1010: 取得中はファイルアイコンを回る弧へ差し替える。
                // **行の位置も幅も変えない**ので、終わったときに並びが動かない
                .child(if loading_file {
                    crate::spinner::spinner(
                        ("remote-file-spin", index as u64),
                        px(16.0),
                        hsla(theme.accent),
                    )
                } else {
                    svg()
                        .path(icon_kind.svg_path())
                        .size(px(16.0))
                        .flex_none()
                        .text_color(hsla(theme.tab_inactive_foreground))
                        .into_any_element()
                });
        }
        el.child(
            div()
                .flex_1()
                .overflow_hidden()
                .whitespace_nowrap()
                .text_ellipsis()
                .child(SharedString::from(truncate_chars(&row.entry.name, 24))),
        )
        // アイコンだけだと「何が起きているか」が伝わらないので短い語を添える
        .when(loading_file, |d| {
            d.child(
                div()
                    .flex_none()
                    .pr(px(8.0))
                    .text_size(px(10.0))
                    .text_color(hsla(theme.text_muted))
                    .child(SharedString::from(
                        crate::ui_text::remote_folder::file_loading(),
                    )),
            )
        })
    }

    /// リモートルート行の SSH バッジ（#976）。
    ///
    /// **絵文字は使わない**（#217）: 地球アイコンの SVG マスク + `SSH` + ホスト名。
    /// 検知していた ssh セッションが消えたホストは色が変わり「切断」が付く
    /// （行そのものは消さない = #976 受け入れ条件 3）
    fn render_ssh_badge(
        &self,
        remote: &tako_core::remote_fs::RemoteRef,
        theme: &tako_core::theme::Theme,
    ) -> gpui::Div {
        // 検知したことのないホスト（明示的に開いただけ）は状態を騙らない
        let disconnected = self
            .ssh_link_of_host(&remote.host)
            .is_some_and(|link| !link.live);
        let tint = if disconnected { theme.red } else { theme.mauve };
        let mut badge = div()
            .flex()
            .flex_none()
            .flex_row()
            .items_center()
            .gap(px(3.0))
            .ml(px(4.0))
            .px(px(4.0))
            .py(px(1.0))
            .rounded(px(4.0))
            .bg(rgba_alpha(tint, 0.16))
            .text_size(px(9.0))
            .font_weight(FontWeight::MEDIUM)
            .text_color(hsla(tint))
            .child(
                svg()
                    .path(file_icons::ui_icon::GLOBE)
                    .size(px(9.0))
                    .flex_none()
                    .text_color(hsla(tint)),
            )
            .child(SharedString::from(
                crate::ui_text::remote_folder::BADGE_LABEL,
            ))
            .child(
                div()
                    .max_w(px(96.0))
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .child(SharedString::from(remote.host.clone())),
            );
        if disconnected {
            badge = badge.child(SharedString::from(
                crate::ui_text::remote_folder::badge_disconnected(),
            ));
        }
        badge
    }

    /// リモート行の右クリックメニュー（#919）。
    /// ローカル用（`render_context_menu`）とは項目がまったく別物なので分ける
    /// （Finder 表示・ゴミ箱・リネームはリモートのパスに対して意味を持たない）
    pub(crate) fn render_remote_context_menu(
        &self,
        window: &gpui::Window,
        cx: &mut Context<Self>,
    ) -> Option<gpui::AnyElement> {
        let ctx = self.remote_context_menu.as_ref()?;
        let theme = &self.theme;
        let remote = ctx.remote.clone();
        let mut items: Vec<(&str, String)> = vec![(
            "copy-path",
            crate::ui_text::remote_folder::menu_copy_remote_path().to_string(),
        )];
        if ctx.is_dir {
            items.push((
                "ssh-pane",
                crate::ui_text::remote_folder::menu_open_ssh_pane().to_string(),
            ));
            items.push((
                "reload",
                crate::ui_text::remote_folder::menu_reload().to_string(),
            ));
        }
        if ctx.is_root {
            items.push((
                "close-root",
                crate::ui_text::remote_folder::menu_close_remote_root().to_string(),
            ));
        }
        // #346 と同じ規則: 画面外へ出ないよう反転・クランプする
        let menu_width = 240.0;
        let menu_height = 8.0 + items.len() as f32 * 22.0;
        let adjusted = clamp_menu_position(ctx.position, menu_width, menu_height, window);
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
                let remote = remote.clone();
                div()
                    .id(("remote-ctx-item", i as u64))
                    .w_full()
                    .px_2()
                    .py(px(2.0))
                    .cursor_pointer()
                    .hover(|d| d.bg(rgba(theme.tab_active_background)))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.remote_context_menu = None;
                        this.remote_menu_action(id, &remote, cx);
                    }))
                    .child(SharedString::from(label))
            }));
        let backdrop = div()
            .id("remote-ctx-backdrop")
            .absolute()
            .left(px(0.0))
            .top(px(0.0))
            .size_full()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    this.remote_context_menu = None;
                    cx.notify();
                }),
            )
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(|this, _, _, cx| {
                    this.remote_context_menu = None;
                    cx.notify();
                }),
            )
            .child(menu);
        Some(backdrop.into_any_element())
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
        let can_paste = ctx.can_paste;
        let pos = ctx.position;
        // ファイルマネージャ / ごみ箱の呼び名は OS で変わる（#617）
        let fm = tako_control::platform::os_integration::file_manager();
        // #1867: 選んだ行の上で開いたら、切り取り / コピー / ごみ箱は選んだもの全部へ効く（件数を添える）
        let multi = self
            .active_tree_selection()
            .filter(|sel| sel.is_multi() && sel.contains(&path))
            .map(|sel| sel.paths.len());
        let counted = |label: &str| match multi {
            Some(n) => crate::ui_text::sidebar::menu_with_count(label, n),
            None => label.to_string(),
        };
        let cut_label = counted(crate::ui_text::sidebar::menu_cut());
        let copy_label = counted(crate::ui_text::sidebar::menu_copy());
        let trash_label = counted(crate::ui_text::sidebar::menu_trash(fm));
        let mut items: Vec<(&str, &str)> = vec![
            ("copy-rel", crate::ui_text::sidebar::menu_copy_rel()),
            ("copy-abs", crate::ui_text::sidebar::menu_copy_abs()),
            ("reveal", crate::ui_text::sidebar::menu_reveal(fm)),
            ("open-term", crate::ui_text::sidebar::menu_open_term()),
        ];
        if !is_dir {
            items.push(("open-default", crate::ui_text::sidebar::menu_open_default()));
            items.push(("open-with", crate::ui_text::sidebar::menu_open_with()));
        }
        // #1860: コピー / 切り取り / 貼り付け（キーと同じ dispatch を通る）
        if !tako_core::file_copy::tree_keys_legacy() {
            items.push(("sep0", ""));
            items.push(("clip-cut", cut_label.as_str()));
            items.push(("clip-copy", copy_label.as_str()));
            items.push(("clip-paste", crate::ui_text::sidebar::menu_paste()));
            // #1867: 移動として貼る（Finder の「項目をここに移動」= ⌥⌘V）
            if !tako_core::tree_select::multi_legacy() {
                items.push((
                    "clip-paste-move",
                    crate::ui_text::sidebar::menu_paste_move(),
                ));
            }
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
        items.push(("trash", trash_label.as_str()));
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
        // 実矩形プローブの器を毎フレーム作り直す（#1725。項目 154 が押す位置の正）
        self.tree_menu_item_rects.borrow_mut().clear();

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
                let rects = self.tree_menu_item_rects.clone();
                // #1860: 貼るものが無ければ押せない見た目にする（押しても何も起きない）
                let disabled = matches!(id, "clip-paste" | "clip-paste-move") && !can_paste;
                let hint = tree_clip_menu_key(id)
                    .map(|key| {
                        tako_core::platform::keys::tree_clip_hint(
                            tako_core::platform::support::Platform::current(),
                            key,
                        )
                    })
                    // #1895: ごみ箱の打鍵（⌘⌫ / Delete）。A/B では受けないので出さない
                    .or_else(|| {
                        (id == "trash" && !tako_core::file_copy::legacy_1895()).then(|| {
                            tako_core::platform::keys::tree_trash_hint(
                                tako_core::platform::support::Platform::current(),
                            )
                            .to_string()
                        })
                    });
                div()
                    .id(("ctx-item", i as u64))
                    .relative()
                    .w_full()
                    .px_2()
                    .py(px(2.0))
                    .flex()
                    .flex_row()
                    .items_center()
                    .when(!disabled, |d| {
                        d.cursor_pointer()
                            .hover(|d| d.bg(rgba(theme.tab_active_background)))
                    })
                    .when(disabled, |d| d.text_color(hsla(theme.text_muted)))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.context_menu = None;
                        if !disabled {
                            this.handle_context_action(id, &path, is_dir, cx);
                        }
                        cx.notify();
                    }))
                    .when(id == "trash", |d| d.text_color(hsla(theme.red)))
                    .child(div().flex_1().child(SharedString::from(label.to_string())))
                    .children(hint.map(|hint| {
                        div()
                            .flex_none()
                            .pl_2()
                            .text_color(hsla(theme.text_muted))
                            .child(SharedString::from(hint))
                    }))
                    // 何も描かない矩形採取（#1182 と同じ作法。見た目にもレイアウトにも出ない）
                    .child(
                        canvas(
                            move |bounds, _, _| rects.borrow_mut().push((id, bounds)),
                            |_, _, _, _| (),
                        )
                        .absolute()
                        .top_0()
                        .left_0()
                        .size_full(),
                    )
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

    /// インライン入力を開く（新規ファイル / 新規フォルダ / 名前を変更。#1725）。
    ///
    /// **開く入口はここ 1 本**（右クリックメニューの 3 項目はすべてこれを呼ぶ）。
    /// 開いた瞬間から打鍵・⌘V・IME の変換がすべてこの入力欄へ向く:
    /// `handle_key` が打鍵を、`app_text_input` が変換の宛先（`AppTextInput::TreeName`）を
    /// 同じ `inline_edit_visible` で決めるので、2 つの判断が割れない。
    ///
    /// 他の入力欄（git のコミット / ブランチ名・返答コメント・Web の URL）は先に畳む。
    /// 残すと、この入力欄を閉じた瞬間に古いフラグが打鍵を拾い直す（#503 と同じ罠）
    pub(crate) fn open_inline_edit(&mut self, kind: InlineEditKind, path: &std::path::Path) {
        self.clear_text_input_focus();
        let text = match kind {
            InlineEditKind::Rename => path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned(),
            _ => String::new(),
        };
        let mut field = crate::text_field::TextField::default();
        field.set_text(text);
        let parent = if kind == InlineEditKind::Rename || path.is_dir() {
            path.to_path_buf()
        } else {
            path.parent().unwrap_or(path).to_path_buf()
        };
        if kind != InlineEditKind::Rename {
            // 作成先を開いておく（入力欄はその直下に出る = #559）
            self.filetree.expand_dir(&parent);
        }
        self.inline_edit = Some(InlineEdit {
            parent,
            kind,
            field,
            origin_pane: self.focused_pane(),
        });
    }

    /// インライン入力を閉じる（#1725。**閉じる出口はここ 1 本**）。
    ///
    /// - この入力欄宛ての変換が残っていたら捨てる（閉じた入力欄を宛先に持ったままだと、
    ///   続く確定がどこにも入らない / 次の変換の宛先がずれる）
    /// - `restore_focus` のとき（Esc / 確定）は、開いたときのペインへ打鍵を戻す。
    ///   外側のクリックで閉じたときは戻さない（押した先が次のフォーカスを決める）
    pub(crate) fn close_inline_edit(&mut self, restore_focus: bool) {
        let Some(edit) = self.inline_edit.take() else {
            return;
        };
        if self
            .ime
            .as_ref()
            .is_some_and(|ime| ime.app_input == Some(AppTextInput::TreeName))
        {
            self.ime = None;
        }
        if restore_focus && self.focused_pane() != edit.origin_pane {
            // 開いていたあいだに外から（CLI / MCP の focus 等）動かされていても戻る。
            // 元のペインがもう無い / 別タブなら何もしない（今のフォーカスのまま）
            let _ = self
                .workspace
                .active_tab_mut()
                .tree_mut()
                .focus(edit.origin_pane);
        }
    }

    /// インライン入力が**画面に出ていて**打鍵・変換を受けるか（#1725）。
    ///
    /// ツリーを閉じた・タブを切り替えて作成先がルートから外れた、のどれでも入力欄は
    /// 描かれない。そのとき打鍵を奪い続けると「押しても何も起きない」になるので、
    /// 打鍵の振り分け（`handle_key`）と変換の宛先（`app_text_input`）はこの 1 判定を使う
    pub(crate) fn inline_edit_visible(&self) -> bool {
        self.inline_edit.as_ref().is_some_and(|edit| {
            inline_edit_target_visible(self.filetree.visible, self.filetree.roots(), &edit.parent)
        })
    }

    /// インライン入力へ文字列を入れる（#1725。**挿入はここ 1 本**）。
    ///
    /// 打鍵（`handle_inline_edit_key`）・⌘V（`paste`）・IME の確定
    /// （`replace_text_in_range`）・未確定のまま確定（`unmark_text`）の 4 経路が
    /// すべて `insert_app_text_input(AppTextInput::TreeName, …)` からここへ来る。
    /// 制御文字（改行・タブ）は `TextField` が落とす = ファイル名に混ざらない
    pub(crate) fn tree_name_insert(&mut self, text: &str, cx: &mut Context<Self>) {
        if let Some(edit) = self.inline_edit.as_mut() {
            // 上限は OS の名前の上限（255 バイト）より十分大きく取る。超えた名前は
            // 作成時に OS が理由つきで断り、#1399 の通知欄に出る（入力欄は残る）。
            // ここは巨大な貼り付けで描画が詰まらないための歯止めだけ
            let _fits = edit.field.insert(text, TREE_NAME_MAX_BYTES, false);
        }
        cx.notify();
    }

    pub(crate) fn handle_inline_edit_key(&mut self, ks: &Keystroke, cx: &mut Context<Self>) {
        // この入力欄だけの割り当て（Enter = 確定 / Esc = 取り消し）を先に見る。
        // 編集操作は `TextField` へ委ねる（#1459 / #1725。手書きのカーソル演算を持たない）
        match ks.key.as_str() {
            "enter" => {
                self.commit_inline_edit(cx);
            }
            "escape" => {
                self.close_inline_edit(true);
                cx.notify();
            }
            key => {
                if let Some(edit) = self.inline_edit.as_mut() {
                    if edit.field.handle_edit_key(key) {
                        cx.notify();
                        return;
                    }
                }
                if ks.modifiers.control || ks.modifiers.platform {
                    return;
                }
                match ks.key_char.as_deref() {
                    Some(ch) if !ch.is_empty() => {
                        self.insert_app_text_input(AppTextInput::TreeName, ch, cx)
                    }
                    // 空白は key_char が来ないことがある（#487 と同じ実機の観測）
                    _ if key == "space" => {
                        self.insert_app_text_input(AppTextInput::TreeName, " ", cx)
                    }
                    _ => {}
                }
            }
        }
    }

    /// 隠しファイル（ドット始まり）の表示トグル（#550）。
    /// CLI `tako panel --show-hidden` / MCP `tako_panel` と同じ dispatch 経路を通す
    pub(crate) fn toggle_hidden_files(&mut self, cx: &mut Context<Self>) {
        let next = !self.filetree.show_hidden();
        let result = tako_control::dispatch(
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
        if let Err(e) = result {
            // #1399: 失敗を黙って捨てない。対象パスを持たない操作なので target は None
            let op = if next {
                crate::ui_text::sidebar::hidden_show()
            } else {
                crate::ui_text::sidebar::hidden_hide()
            };
            self.notify_tree_dispatch_failed(op, None, &e);
        }
        cx.notify();
    }

    pub(crate) fn commit_inline_edit(&mut self, cx: &mut Context<Self>) {
        use tako_control::protocol::{FileOpKind, Request};
        // #1399: **ここで `take()` してはいけない**。`Err` のときに入力欄が閉じて
        // 打った名前ごと消えるので（既存名へのリネームが打ち直しになる）、
        // 閉じるのは成功してからにする
        let Some(edit) = self.inline_edit.clone() else {
            return;
        };
        let name = edit.field.text().trim().to_string();
        if name.is_empty() {
            // 空 Enter は従来どおり「取り消し」（入力欄を閉じる）
            self.close_inline_edit(true);
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
                dest: None,
            },
            PaneOrigin::User,
        );
        match result {
            Ok(_) => {
                self.close_inline_edit(true);
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
            // #1399: 理由を通知欄へ出し、**入力欄と打った文字列は残す**
            // （legacy アームだけ旧挙動 = 閉じて無言）
            Err(e) => {
                if Self::legacy_1399() {
                    self.close_inline_edit(true);
                }
                let op_label = match edit.kind {
                    InlineEditKind::Rename => crate::ui_text::sidebar::menu_rename(),
                    InlineEditKind::NewFile => crate::ui_text::sidebar::menu_new_file(),
                    InlineEditKind::NewDir => crate::ui_text::sidebar::menu_new_dir(),
                };
                self.notify_tree_dispatch_failed(op_label, Some(&name), &e);
            }
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
        // #1399: 失敗の通知に出す対象と操作名。`path_str` は Request へ move するので
        // 先に控える。操作名は**メニューに出ている文言そのもの**を使う（#617 の
        // OS 出し分けもそのまま効く = 押した項目と失敗した項目の名前が一致する）
        let target = path_str.clone();
        let fm = tako_control::platform::os_integration::file_manager();
        match action {
            "copy-abs" => {
                match tako_control::dispatch(
                    self,
                    Request::FileOp {
                        op: FileOpKind::CopyAbsolutePath,
                        path: path_str,
                        name: None,
                        pane: None,
                        dest: None,
                    },
                    PaneOrigin::User,
                ) {
                    Ok(result) => {
                        if let Some(p) = result["path"].as_str() {
                            cx.write_to_clipboard(ClipboardItem::new_string(p.to_string()));
                        }
                    }
                    Err(e) => self.notify_tree_dispatch_failed(
                        crate::ui_text::sidebar::menu_copy_abs(),
                        Some(&target),
                        &e,
                    ),
                }
            }
            "copy-rel" => {
                let pane = self.focused_pane().as_u64();
                match tako_control::dispatch(
                    self,
                    Request::FileOp {
                        op: FileOpKind::CopyRelativePath,
                        path: path_str,
                        name: None,
                        pane: Some(pane),
                        dest: None,
                    },
                    PaneOrigin::User,
                ) {
                    Ok(result) => {
                        if let Some(p) = result["path"].as_str() {
                            cx.write_to_clipboard(ClipboardItem::new_string(p.to_string()));
                        }
                    }
                    Err(e) => self.notify_tree_dispatch_failed(
                        crate::ui_text::sidebar::menu_copy_rel(),
                        Some(&target),
                        &e,
                    ),
                }
            }
            "reveal" => {
                let result = tako_control::dispatch(
                    self,
                    Request::FileOp {
                        op: FileOpKind::Reveal,
                        path: path_str,
                        name: None,
                        pane: None,
                        dest: None,
                    },
                    PaneOrigin::User,
                );
                if let Err(e) = result {
                    self.notify_tree_dispatch_failed(
                        crate::ui_text::sidebar::menu_reveal(fm),
                        Some(&target),
                        &e,
                    );
                }
            }
            "open-term" => {
                let pane = self.focused_pane().as_u64();
                let result = tako_control::dispatch(
                    self,
                    Request::FileOp {
                        op: FileOpKind::OpenTerminal,
                        path: path_str,
                        name: None,
                        pane: Some(pane),
                        dest: None,
                    },
                    PaneOrigin::User,
                );
                if let Err(e) = result {
                    self.notify_tree_dispatch_failed(
                        crate::ui_text::sidebar::menu_open_term(),
                        Some(&target),
                        &e,
                    );
                }
            }
            "rename" | "new-file" | "new-dir" => {
                if let Some(kind) = InlineEditKind::from_menu_id(action) {
                    self.open_inline_edit(kind, path);
                }
            }
            // #1860: キーと同じ入口（選び直してから。続けて ⌘V を押せばこの行へ貼れる）。
            // #1867: 選んだ行の上で開いたメニューは選んだもの全部へ効く（選択を保つ）
            "clip-cut" | "clip-copy" | "clip-paste" | "clip-paste-move" => {
                if let Some(key) = tree_clip_menu_key(action) {
                    if !self.keep_tree_selection_for(path) {
                        self.select_tree_row(path, _is_dir, false);
                    }
                    if let Some(sel) = self.tree_selection.clone() {
                        self.tree_clip_action(key, &sel, cx);
                    }
                }
            }
            "trash" => {
                // #1867: 選んだ行の上で開いたメニューのごみ箱は選んだもの全部（1 要求 = CLI
                // `tako file trash a b` と同じ）。#1895: ⌘⌫（Windows は Delete）と同じ口
                let paths = if self.keep_tree_selection_for(path) && self.tree_multi_selected() {
                    self.tree_selection_paths()
                } else {
                    vec![path.to_path_buf()]
                };
                self.trash_tree_paths(paths, cx);
            }
            "open-default" => {
                let result = tako_control::dispatch(
                    self,
                    Request::FileOp {
                        op: FileOpKind::OpenDefault,
                        path: path_str,
                        name: None,
                        pane: None,
                        dest: None,
                    },
                    PaneOrigin::User,
                );
                if let Err(e) = result {
                    self.notify_tree_dispatch_failed(
                        crate::ui_text::sidebar::menu_open_default(),
                        Some(&target),
                        &e,
                    );
                }
            }
            "open-with" => {
                let path_owned = path.to_path_buf();
                // #1399: OS のアプリ選択ダイアログは dispatch を通らないので、
                // 結果は背景タスクからモデルへ戻して通知欄へ出す
                cx.spawn(async move |this, cx| {
                    let outcome = pick_app_and_open(&path_owned);
                    let target = path_owned.display().to_string();
                    this.update(cx, |this, cx| {
                        if let Err(e) = outcome {
                            this.notify_tree_op_failed(
                                crate::ui_text::sidebar::menu_open_with(),
                                Some(&target),
                                &e,
                            );
                        }
                        cx.notify();
                    })
                    .ok();
                })
                .detach();
            }
            "toggle-hidden" => {
                self.toggle_hidden_files(cx);
            }
            "remove-root" => {
                let result = tako_control::dispatch(
                    self,
                    Request::TreeFolder {
                        action: "remove".into(),
                        path: Some(path_str),
                        tab: None,
                        pane: None,
                        limit: None,
                    },
                    PaneOrigin::User,
                );
                if let Err(e) = result {
                    self.notify_tree_dispatch_failed(
                        crate::ui_text::sidebar::menu_remove_root(),
                        Some(&target),
                        &e,
                    );
                }
                self.sync_filetree_roots();
            }
            _ => {}
        }
        cx.notify();
    }

    // --- リモート（SSH 先）ツリーの操作（#919 / #65） -------------------------

    /// このプレビューがリモート由来ならその位置
    pub(crate) fn preview_remote_origin(
        &self,
        state: &crate::preview::PreviewState,
    ) -> Option<tako_core::remote_fs::RemoteRef> {
        self.preview_remote_origins
            .iter()
            .find(|(pane, _)| {
                self.previews
                    .get(pane)
                    .map(|p| p.path == state.path)
                    .unwrap_or(false)
            })
            .map(|(_, r)| r.clone())
    }

    /// リモートのファイル行をクリックしたときのプレビュー（#919 / #1010）。
    ///
    /// **SFTP の取得だけを背景へ出す**（#1010）。同期で取ると UI スレッドが
    /// 実測 1〜2 秒止まるので、「読み込み中」を出す間もなく画面が固まっていた
    /// （= スピナーを出す余地が構造的に無かった）。取れたあとの扱い
    /// （読み取り専用の判定・プレビューの割り当て・応答）は
    /// `tako_control::remote_open_file_fetched` の 1 実装で、CLI / MCP と共通。
    /// CLI / MCP は従来どおり同期（#966 の切り分けと同じ）
    pub(crate) fn open_remote_file_row(
        &mut self,
        remote: &tako_core::remote_fs::RemoteRef,
        cx: &mut Context<Self>,
    ) {
        if Self::remote_open_sync_legacy() {
            let result = tako_control::dispatch(
                self,
                tako_control::protocol::Request::RemoteFolder {
                    action: "open-file".into(),
                    host: Some(remote.host.clone()),
                    path: Some(remote.path.clone()),
                    tab: None,
                    focus: Some(true),
                    all: false,
                    force: false,
                    enabled: None,
                    terminal: None,
                },
                PaneOrigin::User,
            );
            match result {
                Ok(_) => self.drain_pending_highlights(cx),
                Err(e) => self.set_remote_notice(e.to_string(), true),
            }
            cx.notify();
            return;
        }
        // 同じファイルを 2 回押しても取得は 1 本（スピナーが出ている間は無視）
        if self.remote_file_loading.contains_key(remote) {
            return;
        }
        self.remote_file_loading
            .insert(remote.clone(), std::time::Instant::now());
        let target = remote.clone();
        cx.spawn(async move |this, cx| {
            let host = target.host.clone();
            let path = target.path.clone();
            let fetched = cx
                .background_spawn(async move {
                    tako_core::remote_fs::fetch_file(
                        &host,
                        &path,
                        tako_core::remote_fs::MAX_PREVIEW_BYTES,
                    )
                })
                .await;
            this.update(cx, |this, cx| {
                this.remote_file_loading.remove(&target);
                match fetched {
                    Ok(fetched) => {
                        let opened = tako_control::remote_open_file_fetched(
                            this,
                            PaneOrigin::User,
                            &target.host,
                            &target.path,
                            fetched,
                            Some(true),
                        );
                        match opened {
                            Ok(_) => this.drain_pending_highlights(cx),
                            Err(e) => this.set_remote_notice(e.to_string(), true),
                        }
                    }
                    // #919: 静かに失敗させない。理由 + 次の一手 + 生の詳細を出す
                    Err(e) => this.set_remote_notice(
                        format!("{} / {} / {}", e.summary(), e.next_step(), e.detail),
                        true,
                    ),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    /// リモートファイルの取得を #1010 前（UI スレッドで同期）へ戻す逃げ道
    /// （`TAKO_1010_LEGACY=1`）。同じバイナリでスピナーの有無を A/B するために使う
    pub(crate) fn remote_open_sync_legacy() -> bool {
        static LEGACY: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        *LEGACY.get_or_init(|| std::env::var_os("TAKO_1010_LEGACY").is_some())
    }

    /// リモート操作の通知を出す（#919）。失敗は自動で消さない。
    ///
    /// **名前に remote が入っているが、実体はサイドバー共有の通知欄**
    /// （`render_sidebar` がツリーの上へ無条件で描くバナー。クリックで消える）。
    /// #1376 の URL ブロック・#1399 のローカル操作もここへ出す。呼び出し側の
    /// 差分を増やさないため名前は変えていない（正体はこの doc が正）
    pub(crate) fn set_remote_notice(&mut self, text: String, is_error: bool) {
        if is_error {
            // 理由が読めなければ意味が無いので、閉じているサイドバーを開く
            self.filetree.visible = true;
        }
        self.remote_notice = Some(RemoteNotice {
            text,
            is_error,
            at: std::time::Instant::now(),
        });
    }

    /// ユーザーの操作が失敗したことを画面へ出す唯一の口（#1399 / #1417）。
    ///
    /// ファイルツリーのローカル行は以前 `let _ = dispatch(..)` / `if result.is_ok()` /
    /// `eprintln!` で結果を捨てていたので、**ごみ箱移動やリネームが失敗しても
    /// 画面が無反応**だった（同じサイドバーのリモート行は #919 から通知欄へ
    /// 出していたので、1 つの画面に 2 つのエラー方針が同居していた）。#1417 で
    /// 同じ形が右パネル・プレビューにも在ることが分かったので、口は増やさず
    /// [`NoticeArea`] で画面だけを区別する。
    ///
    /// 出し先はリモート行と同じ [`Self::set_remote_notice`] = 共有の通知欄で、
    /// 同じ失敗を persist.log にも 1 行残す。**診断へ載せるのは画面・操作名と
    /// 理由の分類だけ**（パス・OS のエラー文は載せない = #1376 と同じ作法）。
    /// 通知本文にはユーザーが見て分かる対象（パス・打った名前・window 名）と理由を出す
    pub(crate) fn notify_ui_failure(
        &mut self,
        area: NoticeArea,
        arm: NoticeArm,
        diag_op: &str,
        class: &str,
        text: String,
    ) {
        // A/B: 旧挙動（結果を捨てて画面にも診断にも何も出さない）へ戻す
        if arm.suppressed() {
            return;
        }
        Self::log_ui_failure(area, arm, diag_op, class);
        self.set_remote_notice(text, true);
    }

    /// ユーザーの操作**ではない**が知らせるべき出来事を画面へ出す唯一の口（#1450）。
    ///
    /// [`Self::notify_ui_failure`] の成功系。出し先（共有の通知欄）も抑止の軸
    /// （[`NoticeArm`]）も失敗側と同じで、違うのは **`is_error` が偽**
    /// （サイドバーを勝手に開かない）ことだけ。口を分けずに 1 つに保つのは
    /// #1417 と同じ理由で、画面ごとに増やすと「片方だけ無言」が必ず生まれるため。
    ///
    /// **診断（persist.log）はここでは書かない**。起票は GUI の中の出来事ではなく
    /// dispatch の操作なので、記録は起きた場所（`tako_control::dispatch`）に置く
    /// = sidebar.rs から診断へ書く口は `log_ui_failure` の 1 つのままに保たれる
    fn notify_ui_info(&mut self, area: NoticeArea, arm: NoticeArm, text: String) {
        debug_assert!(
            matches!(area, NoticeArea::UserTasks | NoticeArea::SleepGuard),
            "成功系は「人の手を待つものが増えた」（#1450）と「安全弁が働いた」（#1473）だけ"
        );
        if arm.suppressed() {
            return;
        }
        self.set_remote_notice(text, false);
    }

    /// 新しいユーザータスクが起票されたことを通知欄へ 1 行出す（#1450）。
    ///
    /// **本文・添付・コメントは出さない**（画面に出すのは id とタイトルだけ）。
    /// 通知は消える前提なので、続きは右パネル（B2）と `tako todo list` で読む
    pub(crate) fn notify_user_task_added(&mut self, id: &str, title: &str) {
        self.notify_ui_info(
            NoticeArea::UserTasks,
            NoticeArm::Issue1450,
            crate::ui_text::sidebar::notice_user_task_added(id, title),
        );
    }

    /// 蓋閉じ継続が安全弁で解除された・戻ったことを通知欄へ 1 行出す（#1473）。
    ///
    /// **出すかどうかの判断はここではしない**（`sleep_guard::lid_notice` の
    /// 純粋関数が決める）。ここは文言を選んで共有の通知欄へ渡すだけ
    pub(crate) fn notify_lid_guard(&mut self, notice: tako_control::sleep_guard::LidNotice) {
        use tako_control::sleep_guard::LidNotice;
        let text = match notice {
            LidNotice::Released(reason) => crate::ui_text::sidebar::notice_lid_released(reason),
            LidNotice::Reapplied => crate::ui_text::sidebar::notice_lid_reapplied().to_string(),
        };
        self.notify_ui_info(NoticeArea::SleepGuard, NoticeArm::Issue1473, text);
    }

    /// #1450 の A/B。`TAKO_1450_LEGACY=1` で**同一バイナリのまま**旧挙動へ戻す
    /// （起票しても画面に何も出ない = 「起票されたことに気づけない」の再現）。
    /// 診断（persist.log）は dispatch 側で書くのでこのアームでも残る —— 抑えるのは
    /// **画面の通知だけ**なので、A/B を立てた検証でも起票そのものは追える
    pub(crate) fn legacy_1450() -> bool {
        static LEGACY: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        *LEGACY.get_or_init(|| std::env::var("TAKO_1450_LEGACY").map(|v| v == "1") == Ok(true))
    }

    /// **画面には出さず**診断にだけ 1 行残す（#1422）。
    ///
    /// ユーザーが押していない背景処理（PDF の再ラスタライズ・監視対象の更新）の
    /// 失敗までバナーにすると、押していない操作の失敗が画面に居座る（#1399 が
    /// プレビュー監視で採った物差しと同じ）。それでも `eprintln!` は GUI では
    /// 誰も読めないので、**捨てずに** persist.log へ落とす。
    /// 書式は [`Self::notify_ui_failure`] と共有する（診断の grep が 1 通りで済む）
    pub(crate) fn log_ui_failure(area: NoticeArea, arm: NoticeArm, diag_op: &str, class: &str) {
        if arm.suppressed() {
            return;
        }
        tako_control::diag::persist_log(&format!(
            "UI 操作に失敗: area={} op={diag_op} 分類={class}",
            area.tag()
        ));
    }

    /// dispatch が `Err` を返したときの通知（#1399 / #1417）。
    /// `op` は**ユーザーが押した項目の文言**をそのまま渡す（押したものと失敗した
    /// ものの名前が必ず一致する）。`target` が `None` なのは対象を持たない操作
    pub(crate) fn notify_ui_dispatch_failed(
        &mut self,
        area: NoticeArea,
        arm: NoticeArm,
        op: &str,
        target: Option<&str>,
        err: &tako_control::DispatchError,
    ) {
        let reason = err.to_string();
        self.notify_ui_failure(
            area,
            arm,
            op,
            err.class(),
            crate::ui_text::sidebar::notice_op_failed(op, target, &reason),
        );
    }

    /// dispatch を経由しない経路（PTY 起動・クリップボード・ワークスペース操作）の
    /// 失敗の通知（#1399 の `notify_tree_op_failed` を画面横断へ広げたもの。#1422）。
    /// 理由は `String` しか無いので分類は `operation` 固定
    pub(crate) fn notify_ui_op_failed(
        &mut self,
        area: NoticeArea,
        arm: NoticeArm,
        op: &str,
        target: Option<&str>,
        reason: &str,
    ) {
        self.notify_ui_failure(
            area,
            arm,
            op,
            "operation",
            crate::ui_text::sidebar::notice_op_failed(op, target, reason),
        );
    }

    /// ファイルツリーの右クリックメニュー・トグルの dispatch 失敗（#1399）
    pub(crate) fn notify_tree_dispatch_failed(
        &mut self,
        op: &str,
        target: Option<&str>,
        err: &tako_control::DispatchError,
    ) {
        self.notify_ui_dispatch_failed(NoticeArea::Tree, NoticeArm::Issue1399, op, target, err);
    }

    /// dispatch を経由しない経路（OS ダイアログ・PTY 起動）の失敗の通知（#1399）。
    /// 理由は `String` しか無いので分類は `operation` 固定
    pub(crate) fn notify_tree_op_failed(&mut self, op: &str, target: Option<&str>, reason: &str) {
        self.notify_ui_op_failed(NoticeArea::Tree, NoticeArm::Issue1399, op, target, reason);
    }

    /// ファイル行を開けなかったときの通知（#1283 の cmd+クリックと同じ文言。#1399）
    pub(crate) fn notify_tree_open_failed(&mut self, diag_op: &str, path: &str, reason: &str) {
        self.notify_ui_failure(
            NoticeArea::Tree,
            NoticeArm::Issue1399,
            diag_op,
            "operation",
            crate::ui_text::sidebar::notice_open_failed(path, reason),
        );
    }

    /// #1399 の A/B。`TAKO_1399_LEGACY=1` で**同一バイナリのまま**旧挙動へ戻す
    /// （ローカル操作の失敗を通知欄にも persist.log にも出さず、リネーム / 新規作成の
    /// 失敗では打った名前も捨てる = 「押しても無言」の再現）
    pub(crate) fn legacy_1399() -> bool {
        static LEGACY: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        *LEGACY.get_or_init(|| std::env::var("TAKO_1399_LEGACY").map(|v| v == "1") == Ok(true))
    }

    /// #1725 の A/B。`TAKO_1725_LEGACY=1` で**同一バイナリのまま**旧挙動へ戻す
    /// （インライン入力を IME の宛先に入れない = 変換がターミナルペインに束縛される /
    /// 外側クリックで閉じない）。セルフテスト項目 154 が FAILED になるのが A/B の実測
    pub(crate) fn legacy_1725() -> bool {
        static LEGACY: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        *LEGACY.get_or_init(|| std::env::var("TAKO_1725_LEGACY").map(|v| v == "1") == Ok(true))
    }

    /// #1417 の A/B。`TAKO_1417_LEGACY=1` で**同一バイナリのまま**旧挙動へ戻す
    /// （ツリー以外の画面の dispatch 失敗を通知欄にも persist.log にも出さない
    /// = 「行を押しても無言」の再現）。#1399 と env を分けてあるので、
    /// 片方のアームがもう片方の画面の回帰を隠さない
    pub(crate) fn legacy_1417() -> bool {
        static LEGACY: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        *LEGACY.get_or_init(|| std::env::var("TAKO_1417_LEGACY").map(|v| v == "1") == Ok(true))
    }

    /// #1422 の A/B。`TAKO_1422_LEGACY=1` で**同一バイナリのまま**旧挙動へ戻す
    /// （tmux セッションの復元・復元ペインの PTY 起動・バックグラウンド復帰・
    /// コードのコピー・Code Runner・PDF 再ラスタライズの失敗を、通知欄にも
    /// persist.log にも出さない = 「押しても無言」の再現）。#1399 / #1417 と env を
    /// 分けてあるので、片方のアームがもう片方の回帰を隠さない
    pub(crate) fn legacy_1422() -> bool {
        static LEGACY: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        *LEGACY.get_or_init(|| std::env::var("TAKO_1422_LEGACY").map(|v| v == "1") == Ok(true))
    }

    /// #1432 の A/B。`TAKO_1432_LEGACY=1` で**同一バイナリのまま**旧挙動へ戻す
    /// （ドロワーの復帰・コマンドカードの操作・チャットのコピー・Finder から開く・
    /// リリースノートのリンクの失敗を、通知欄にも persist.log にも出さない =
    /// `eprintln!` しか無かった頃の「押しても無言」の再現）。#1399 / #1417 / #1422 と
    /// env を分けてあるので、片方のアームがもう片方の回帰を隠さない
    pub(crate) fn legacy_1432() -> bool {
        static LEGACY: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        *LEGACY.get_or_init(|| std::env::var("TAKO_1432_LEGACY").map(|v| v == "1") == Ok(true))
    }

    /// #1441 の A/B。`TAKO_1441_LEGACY=1` で**同一バイナリのまま**旧挙動へ戻す
    /// （ソケットの置き場を `<data_dir>/tako.sock` 直置きへ固定し、深い data dir では
    /// bind が `sun_path` で落ちる。失敗を通知欄にも persist.log にも出さない =
    /// 「警告 1 行で黙って縮退する」の再現）。#1399 / #1417 / #1422 / #1432 と env を
    /// 分けてあるので、片方のアームがもう片方の回帰を隠さない。
    ///
    /// **置き場を決める側（`tako_core::ipc_socket::legacy_1441`）と同じ env を読む**。
    /// 宣言がここにも要るのは、番犬（#1422 の `abの逃げ道はissueごとに別のenvを読む`）が
    /// アームごとの宣言を `sidebar.rs` に求めるため。2 つが同じ env を読んでいることは
    /// #1441 の番犬が機械で照合する
    pub(crate) fn legacy_1441() -> bool {
        static LEGACY: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        *LEGACY.get_or_init(|| std::env::var("TAKO_1441_LEGACY").map(|v| v == "1") == Ok(true))
    }

    /// IPC の受け口が立たなかったことを画面へ出す（#1441）。
    /// 理由（バイト長と上限）は `tako_core::ipc_socket` の記録から組み立てる
    pub(crate) fn notify_ipc_unavailable(&mut self, status: &tako_core::ipc_socket::IpcStatus) {
        if status.bound {
            return;
        }
        let text = if status.too_long() {
            crate::ui_text::sidebar::notice_ipc_too_long(status.path_bytes, status.limit)
        } else {
            crate::ui_text::sidebar::notice_ipc_unavailable(
                status.error.as_deref().unwrap_or("理由不明"),
            )
        };
        self.notify_ui_failure(
            NoticeArea::Ipc,
            NoticeArm::Issue1441,
            "IPC サーバーの起動",
            // 診断へ載せるのは分類と長さだけ（パスは載せない）
            &status.length_note(),
            text,
        );
    }

    /// #1446 の A/B。`TAKO_1446_LEGACY=1` で**同一バイナリのまま**旧挙動へ戻す。
    ///
    /// 戻すのは 4 つすべて（**この env 1 つに閉じる**）:
    /// SSH 追跡の `layout.json` への永続 / 復元時の引き継ぎ / #976 の検知が
    /// 見つけた手打ちペインの引き取り / 「繋ぎ直さない理由」の通知。
    /// **#1040 の `TAKO_1040_LEGACY`（自動再接続そのもの）とは独立**に効くので、
    /// 「記憶が残るか」と「残った記憶で撃つか」を別々に倒して確かめられる
    pub(crate) fn legacy_1446() -> bool {
        static LEGACY: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        *LEGACY.get_or_init(|| std::env::var("TAKO_1446_LEGACY").map(|v| v == "1") == Ok(true))
    }

    /// リモート行の右クリックメニューの実行（#919）
    pub(crate) fn remote_menu_action(
        &mut self,
        id: &str,
        remote: &tako_core::remote_fs::RemoteRef,
        cx: &mut Context<Self>,
    ) {
        match id {
            "copy-path" => {
                cx.write_to_clipboard(gpui::ClipboardItem::new_string(remote.path.clone()));
            }
            "reload" => {
                self.filetree.invalidate_remote(remote);
            }
            "ssh-pane" => {
                let result = tako_control::dispatch(
                    self,
                    tako_control::protocol::Request::RemoteFolder {
                        action: "ssh-pane".into(),
                        host: Some(remote.host.clone()),
                        path: Some(remote.path.clone()),
                        tab: None,
                        focus: Some(true),
                        all: false,
                        force: false,
                        enabled: None,
                        terminal: None,
                    },
                    PaneOrigin::User,
                );
                if let Err(e) = result {
                    self.set_remote_notice(e.to_string(), true);
                } else if !Self::attach_drain_legacy() {
                    // #1023: `ssh-pane` は内部で `OpenRemote` を通るので PTY 起動が
                    // `pending_attach` へ積まれる。UI 経路はここで消化しないと
                    // ペインだけ生えてターミナルが立たない
                    if let Err(e) = self.attach_pending_sessions(cx) {
                        self.set_remote_notice(e, true);
                    }
                }
            }
            "close-root" => {
                let result = tako_control::dispatch(
                    self,
                    tako_control::protocol::Request::RemoteFolder {
                        action: "close".into(),
                        host: Some(remote.host.clone()),
                        path: Some(remote.path.clone()),
                        tab: None,
                        focus: None,
                        all: false,
                        force: false,
                        enabled: None,
                        terminal: None,
                    },
                    PaneOrigin::User,
                );
                match result {
                    Ok(_) => self.set_remote_notice(
                        crate::ui_text::remote_folder::closed(&remote.label()),
                        false,
                    ),
                    Err(e) => self.set_remote_notice(e.to_string(), true),
                }
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
                new_tab: false,
                line: None,
                column: None,
            },
            PaneOrigin::User,
        );
        if let Err(e) = result {
            // #1399: `eprintln!` だけだと GUI では誰も読めない（= 行を押しても無言。
            // #1398 のシンボリックリンクはここで完全に無反応になっていた）。
            // 文言は #1283 の cmd+クリックと同じものを共有する
            let reason = e.to_string();
            self.notify_tree_open_failed("open-file", &path.display().to_string(), &reason);
        }
        self.drain_pending_highlights(cx);
        cx.notify();
    }

    /// Finder の「このアプリケーションで開く」から渡されたものを新しいタブで開く
    /// （FR-3.22 / #835）。ファイルは `tako open --new-tab`、フォルダは
    /// `tako tab new --cwd` と同じ dispatch を通る（UI 独自経路を作らない）
    pub(crate) fn open_from_finder(
        &mut self,
        target: &crate::open_files::OpenTarget,
        cx: &mut Context<Self>,
    ) {
        use crate::open_files::OpenTarget;
        // #1399: 失敗の通知に出す対象（どちらの経路でもパス 1 本）
        let target_label = match target {
            OpenTarget::PreviewInNewTab(path) => path.display().to_string(),
            OpenTarget::ShellInNewTab(dir) => dir.display().to_string(),
        };
        let request = match target {
            OpenTarget::PreviewInNewTab(path) => tako_control::protocol::Request::OpenFile {
                pane: None,
                path: path.display().to_string(),
                mode: None,
                direction: None,
                focus: Some(true),
                new_tab: true,
                line: None,
                column: None,
            },
            OpenTarget::ShellInNewTab(dir) => tako_control::protocol::Request::TabNew {
                // タブ名はフォルダ名（無ければパスそのもの）。明示タイトル =
                // 手動リネーム扱いになるので自動リネームに奪われない
                title: Some(
                    dir.file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_else(|| dir.display().to_string()),
                ),
                focus: Some(true),
                cwd: Some(dir.display().to_string()),
            },
        };
        match tako_control::dispatch(self, request, PaneOrigin::User) {
            Ok(_) => {
                // フォルダ経路（TabNew）はシェルを起動する。dispatch を直接呼ぶと
                // 起動依頼は pending_attach へ積まれるだけなので、ここで処理する
                // （残すと空のペインが残り、後続 dispatch が巻き添えを食う）
                for (pane, options) in std::mem::take(&mut self.pending_attach) {
                    if let Err(e) = self.spawn_session(pane, options, cx) {
                        // #1399: ペインを消してしまうので、消した理由を画面へ残す
                        // （残さないと「新しいタブが一瞬出て消えた」だけになる）
                        let reason = e.to_string();
                        self.notify_tree_op_failed(
                            crate::ui_text::sidebar::menu_open_term(),
                            Some(&target_label),
                            &reason,
                        );
                        self.remove_pane(pane, cx);
                    }
                }
            }
            Err(e) => {
                let reason = e.to_string();
                self.notify_tree_open_failed("open-from-finder", &target_label, &reason);
            }
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

    /// syntect ハイライトを background executor で実行し、完了後にプレビューを差し替える。
    /// 走っている数を `view_highlights_running` へ数える（#1890。戻ったら取り込んだか
    /// 捨てたかに依らず減らす）
    pub(crate) fn spawn_highlight(
        &mut self,
        pane: PaneId,
        path: std::path::PathBuf,
        text: String,
        cx: &mut Context<Self>,
    ) {
        *self.view_highlights_running.entry(pane).or_default() += 1;
        let inject = preview::highlight_inject();
        cx.spawn(async move |this, cx| {
            let p = path.clone();
            if let Some(delay) = inject.delay {
                cx.background_executor().timer(delay).await;
            }
            let task = cx.background_executor().spawn(async move {
                // #1916: 塗りの間は App Nap に間引かせない（間引かれると E コアへ寄せられ、
                // 10 MB で 12.0 秒 → 26.9〜33.4 秒になる）
                let _work = UserWork::begin();
                preview::highlight_text(&p, &text)
            });
            let lines = task.await;
            let _ = this.update(cx, |app, cx| {
                if let Some(running) = app.view_highlights_running.get_mut(&pane) {
                    *running = running.saturating_sub(1);
                    if *running == 0 {
                        app.view_highlights_running.remove(&pane);
                    }
                }
                if inject.drop {
                    return;
                }
                // #1660: 編集セッションがあれば表示行はエディタの持ち物（打った分を
                // 反映している）。読み取り表示の塗りで上書きすると、大きいファイルを
                // 開いてすぐ編集を始めたとき（塗りに数秒かかる）打った文字が表示から消える
                if app.preview_edits.contains_key(&pane) {
                    return;
                }
                if let Some(state) = app.previews.get_mut(&pane) {
                    if state.path == path {
                        state.content = preview::PreviewContent::Code(lines);
                        state.content_rev = preview::next_content_rev();
                        cx.notify();
                    }
                }
            });
        })
        .detach();
    }

    /// 編集セッションが全文の塗りを待っていれば、その材料を待ち行列へ積む（#1660）
    pub(crate) fn queue_editor_seed(&mut self, pane: PaneId) {
        if let Some(request) = self
            .preview_edits
            .get_mut(&pane)
            .and_then(preview::EditState::take_seed_request)
        {
            self.pending_editor_seeds.push((pane, request));
        }
    }

    /// 積まれた全文の塗りを background で起こす（#1660。render の入口から呼ぶ）
    pub(crate) fn drain_pending_editor_seeds(&mut self, cx: &mut Context<Self>) {
        for (pane, request) in std::mem::take(&mut self.pending_editor_seeds) {
            self.spawn_editor_seed(pane, request, cx);
        }
    }

    /// 大きい文書の全文を background で塗り、終わったら編集セッションへ取り込む（#1660）。
    ///
    /// 塗っている間に打った分は取り込むときに差分で塗り足す。差分が大きければ
    /// 取り込まずに今の本文で出し直す（`adopt_editor_seed` が印を立て直す）
    pub(crate) fn spawn_editor_seed(
        &self,
        pane: PaneId,
        request: preview::SeedRequest,
        cx: &mut Context<Self>,
    ) {
        let inject = preview::highlight_inject();
        cx.spawn(async move |this, cx| {
            if let Some(delay) = inject.delay {
                cx.background_executor().timer(delay).await;
            }
            let task = cx.background_executor().spawn(async move {
                // #1916: 読み取り表示の塗りと同じ理由で、塗りの間は App Nap に間引かせない
                let _work = UserWork::begin();
                preview::seed_editor_highlight(request)
            });
            let seed = task.await;
            let _ = this.update(cx, |app, cx| {
                let (previews, edits) = (&mut app.previews, &mut app.preview_edits);
                let (Some(state), Some(edit)) = (previews.get_mut(&pane), edits.get_mut(&pane))
                else {
                    return;
                };
                if preview::adopt_editor_seed(state, edit, seed) {
                    cx.notify();
                }
                // 取り込めなかった（差分が大きい / 構文セットを載せ直した）なら出し直す
                if let Some(request) = edit.take_seed_request() {
                    app.spawn_editor_seed(pane, request, cx);
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
                // #1926: 開いた人が待っている読み込みの間は App Nap に間引かせない
                // （117 ページの PDF は間引かれたままだと 7.2〜8.0 秒、握ると 3.0 秒）
                let _work = UserWork::begin_load();
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
        for (pane, path, text, ticket) in std::mem::take(&mut self.pending_md_resumes) {
            self.spawn_md_resume(pane, path, text, ticket, cx);
        }
    }

    /// 編集を抜けた大きい Markdown を background で描き直す（#1661）。
    ///
    /// 取り込むのは**積んだときの読み込み中の表示がまだ出ている**ときだけ（`content_rev` が
    /// 同じ）。その間に編集へ戻った・別のファイルや表示へ差し替わったなら組み終えても捨てる
    fn spawn_md_resume(
        &self,
        pane: PaneId,
        path: std::path::PathBuf,
        text: String,
        ticket: u64,
        cx: &mut Context<Self>,
    ) {
        cx.spawn(async move |this, cx| {
            let state = cx
                .background_executor()
                .spawn(async move {
                    // #1926: 編集を抜けた人が描き直しを待っている
                    let _work = UserWork::begin_load();
                    let _span = tako_control::diag::perf_span("preview_md_resume");
                    preview::markdown_from_text(&path, &text)
                })
                .await;
            let _ = this.update(cx, |app, cx| {
                if app
                    .previews
                    .get(&pane)
                    .is_some_and(|shown| shown.content_rev == ticket)
                {
                    app.previews.insert(pane, state);
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// ツリーの行に移動の強調とドラッグの受け口を付ける（FR-3.32 / #1834）。
    ///
    /// 受け口（`on_drag_move` / `on_drop`）は**ツリーの行からドラッグしている間だけ**付ける。
    /// gpui の `on_drag_move` はカーソルがどこにあっても全リスナーへ届くので、常時付けると
    /// 行の数だけのリスナーがマウス移動のたびに走る。ペインへ落とす既存の D&D
    /// （FR-3.11 / FR-3.13）はペイン側のオーバーレイが受けるので、ここと干渉しない
    fn decorate_tree_row(
        &self,
        el: gpui::Stateful<gpui::Div>,
        row: &filetree::Row,
        drag_live: bool,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let remote = row.remote.is_some();
        let is_dir = row.entry.is_dir;
        let mark = tree_drop_mark(self.tree_drop.as_ref(), &row.entry.path, remote);
        let mut el = with_tree_drop_mark(el, mark, theme);
        // #1860: 切り取り中の行は薄く、選んでいる行は accent の枠（絶対配置の重ね = 行の高さを
        // 1px も動かさない。#1834 の札と同じ作法）
        if !remote
            && self
                .file_clipboard
                .as_ref()
                .is_some_and(|clip| clip.is_cut(&row.entry.path))
        {
            el = el.opacity(TREE_CUT_OPACITY);
        }
        let selected = self.active_tree_selection().is_some_and(|sel| {
            sel.remote == remote
                && match &row.remote {
                    Some(r) => sel.path == std::path::Path::new(&r.path),
                    // #1867: 選んでいる行すべてに枠
                    None => sel.contains(&row.entry.path),
                }
        });
        if selected {
            el = el.relative().bg(rgba_alpha(theme.accent, 0.22)).child(
                div()
                    .absolute()
                    .top_0()
                    .left_0()
                    .size_full()
                    .border_1()
                    .border_color(hsla(theme.accent))
                    .rounded_sm(),
            );
        }
        if remote {
            // 掴んだ時点で理由をゴーストに出す（どこにも落とせない）
            el = el.on_drag(
                RemoteRowDrag,
                self.drag_ghost_builder(
                    DragKind::RemoteRow,
                    crate::ui_text::sidebar::move_refused(
                        &tako_core::file_move::MoveRefusal::Remote,
                    ),
                    cx,
                ),
            );
        }
        if !drag_live {
            return el;
        }
        let over_path = row.entry.path.clone();
        let drop_path = row.entry.path.clone();
        el.on_drag_move::<FileDrag>(cx.listener(
            move |this, e: &gpui::DragMoveEvent<FileDrag>, _, cx| {
                let drag = e.drag(cx).clone();
                let inside = e.bounds.contains(&e.event.position);
                this.track_tree_drop(&over_path, is_dir, remote, inside, &drag, cx);
            },
        ))
        .on_drop::<FileDrag>(cx.listener(move |this, drag: &FileDrag, _, cx| {
            this.drop_on_tree_row(&drop_path, is_dir, remote, drag, cx);
        }))
    }

    /// ドラッグ中のカーソルが行に入った / 出たときの判定の更新（FR-3.32 / #1834）。
    ///
    /// **同じ行の上に居る間は判定し直さない**（同名の stat を 1 行につき 1 回で済ませる）。
    /// 出たときは自分が立てた判定だけを畳む（入った先の行が新しい判定を立てる。
    /// リスナーが呼ばれる順に依らない）
    pub(crate) fn track_tree_drop(
        &mut self,
        row: &std::path::Path,
        row_is_dir: bool,
        row_remote: bool,
        inside: bool,
        drag: &FileDrag,
        cx: &mut Context<Self>,
    ) {
        let mine = |h: &TreeDropHover| h.row == row && h.row_remote == row_remote;
        if !inside {
            if self.tree_drop.as_ref().is_some_and(mine) {
                self.tree_drop = None;
                cx.notify();
            }
            return;
        }
        if self
            .tree_drop
            .as_ref()
            .is_some_and(|h| mine(h) && h.src == drag.path)
        {
            return;
        }
        let next = self.tree_drag_hover(drag, row, row_is_dir, row_remote);
        if self.tree_drop != next {
            self.tree_drop = next;
            cx.notify();
        }
    }

    /// ツリーの行へのドロップ（FR-3.32 / #1834）。
    ///
    /// 移せる・移せないの**最終判断は dispatch の `FileOpKind::Move` の 1 実装**
    /// （断った理由の文面が CLI / MCP と揃う）。画面が先に止めるのは、dispatch へ渡せない
    /// リモートの行と、画面の方針で断る見出し行だけ
    pub(crate) fn drop_on_tree_row(
        &mut self,
        row: &std::path::Path,
        row_is_dir: bool,
        row_remote: bool,
        drag: &FileDrag,
        cx: &mut Context<Self>,
    ) {
        use tako_control::protocol::{FileOpKind, Request};
        use tako_core::file_move::{DropVerdict, MoveRefusal};
        // ペインへのドロップと同じ後始末（`take_drop_zone`）。ここへ来たときは
        // gpui が stop_propagation するので、ルートの `on_mouse_up` は走らない
        self.tree_drop = None;
        self.drag_kind = None;
        self.drop_cmd_held = false;
        self.drop_target = None;
        let Some(hover) = self.tree_drag_hover(drag, row, row_is_dir, row_remote) else {
            // 掴んだ行へ戻した = 取り消し
            cx.notify();
            return;
        };
        let src = drag.path.display().to_string();
        let op = crate::ui_text::sidebar::move_op();
        // #1867: まとめて運んでいる = 1 要求（`FileOpMany`。CLI `tako file move a b dst` と同じ）
        if drag.paths.len() > 1 {
            self.drop_many_on_tree_row(drag, &hover, cx);
            return;
        }
        match hover.verdict {
            DropVerdict::Unchanged => {}
            DropVerdict::Refused(refusal @ (MoveRefusal::Remote | MoveRefusal::WorkspaceRoot)) => {
                self.notify_tree_op_failed(
                    op,
                    Some(&src),
                    &crate::ui_text::sidebar::move_refused(&refusal),
                );
            }
            DropVerdict::Move | DropVerdict::Refused(_) => {
                let result = tako_control::dispatch(
                    self,
                    Request::FileOp {
                        op: FileOpKind::Move,
                        path: src.clone(),
                        name: None,
                        pane: None,
                        dest: Some(hover.dest.display().to_string()),
                    },
                    PaneOrigin::User,
                );
                match result {
                    // 付け替え・ツリーの読み直しは dispatch の中（`file_moved`）で済んでいる。
                    // 読み込み中だったプレビューの読み直しと、パスの変わったレイアウトの保存だけ
                    Ok(_) => {
                        self.drain_pending_preview_loads(cx);
                        self.save_layout();
                    }
                    Err(e) => self.notify_tree_dispatch_failed(op, Some(&src), &e),
                }
            }
        }
        cx.notify();
    }

    /// ファイル・フォルダを移した**後**の追従（FR-3.32 / #1834）。
    ///
    /// dispatch `FileOpKind::Move` が `PreviewHost::file_moved` から 1 回だけ呼ぶ
    /// （ツリーの D&D・CLI・MCP のどれでも同じ）。`follows` は移す前に
    /// `tako_core::file_move::follows` が決めた付け替え先で、ここは写すだけ。
    ///
    /// **付け替えないと**、ディスクの監視が元の場所を見て「外で削除された」（#1659）と読み、
    /// 未保存の変更が競合の帯の向こうへ閉じ込められる。本文・undo・カーソル・版は
    /// バッファに残したまま、パスだけを差し替える
    pub(crate) fn follow_file_move(
        &mut self,
        from: &std::path::Path,
        to: &std::path::Path,
        follows: &[tako_core::file_move::Follow],
    ) {
        use tako_core::file_move::remap;
        // 積んだまま未着手の読み込み・塗りは、新しいパスで起こす
        for (_, path, _) in self.pending_preview_loads.iter_mut() {
            if let Some(moved) = remap(path, from, to) {
                *path = moved;
            }
        }
        for (_, path, _) in self.pending_highlights.iter_mut() {
            if let Some(moved) = remap(path, from, to) {
                *path = moved;
            }
        }
        for follow in follows {
            let pane = PaneId::from_raw(follow.pane);
            let mut reload = None;
            if let Some(state) = self.previews.get_mut(&pane) {
                state.path = follow.to.clone();
                // 読み込み中の結果は「読み込み中に別ファイルへ差し替わった」として
                // 捨てられる（後勝ち）ので、新しいパスで読み直す
                if matches!(state.content, preview::PreviewContent::Loading) {
                    reload = Some(state.mode);
                }
            }
            if let Some(mode) = reload {
                if !self
                    .pending_preview_loads
                    .iter()
                    .any(|(p, _, _)| *p == pane)
                {
                    self.pending_preview_loads
                        .push((pane, follow.to.clone(), mode));
                }
            }
            if let Some(edit) = self.preview_edits.get_mut(&pane) {
                edit.buffer.retarget(follow.to.clone());
                // 言語サーバの文書は URI ごと開き直す（つながりを外す = 旧 URI の didClose、
                // 次の同期 = 新 URI の didOpen）。診断は旧 URI のものなので捨てる
                edit.lsp = tako_control::lsp::DocLink::Unlinked;
                edit.diagnostics = None;
            }
            self.sync_preview_lsp(pane);
            // Code Runner の宣言はパスから決まる（`${file}` の展開先も変わる）
            self.detect_preview_run_profiles(pane, &follow.to);
        }
        // 戻る / 進むの項目も付け替える（残すと「消えていた」として捨てられる）
        self.jump_history.retarget_path(from, to);
        self.filetree.note_moved(from, to);
        self.sync_preview_watches();
    }

    // --- コピー / 切り取り / 貼り付け（FR-3.34 / #1860） --------------------------------

    /// 行を選ぶ（押した行を ⌘C / ⌘X / ⌘V の対象にする。#1860）。
    /// 選んだ時点のタブとフォーカスペインを覚える（動いたら効かない = [`Self::active_tree_selection`]）
    pub(crate) fn select_tree_row(&mut self, path: &std::path::Path, is_dir: bool, remote: bool) {
        let root = !remote && self.filetree.roots().iter().any(|r| r == path);
        self.tree_selection = Some(TreeSelection {
            path: path.to_path_buf(),
            is_dir,
            root,
            remote,
            tab: self.workspace.active_tab_id(),
            pane: self.focused_pane(),
            paths: vec![path.to_path_buf()],
            anchor: path.to_path_buf(),
        });
    }

    /// いま効いている選択（キーの宛先がツリーか。#1860）。
    ///
    /// ツリーが見えていて、選んだときとタブ・フォーカスペインが同じときだけ。
    /// ほかの経路（キーでのペイン移動・CLI / MCP の focus・タブの切り替え）でどちらかが
    /// 動いたら、その時点で ⌘C / ⌘V はペインへ戻る（選択を畳み忘れる経路を作らない）
    pub(crate) fn active_tree_selection(&self) -> Option<&TreeSelection> {
        if tako_core::file_copy::tree_keys_legacy() {
            return None;
        }
        self.tree_selection.as_ref().filter(|sel| {
            self.filetree.visible
                && sel.tab == self.workspace.active_tab_id()
                && sel.pane == self.focused_pane()
        })
    }

    /// ツリーへ向いた打鍵（⌘C / ⌘X / ⌘V・Esc。Windows は Ctrl）を処理する。処理したら真。
    ///
    /// `handle_key` が端末へ流す前に呼ぶ。⌘C / ⌘V はアクション（`CopySelection` /
    /// `PasteClipboard`）が先に受けることもあるので、そちらも同じ [`Self::tree_clip_action`]
    /// を呼ぶ（どちらが先に受けても 1 回だけ走る = 受けた側が伝播を止める）
    pub(crate) fn handle_tree_clip_keystroke(
        &mut self,
        ks: &Keystroke,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(sel) = self.active_tree_selection().cloned() else {
            return false;
        };
        let m = ks.modifiers;
        if ks.key == "escape" && !m.modified() {
            // 選択を外す（Esc を端末へ流さない = エージェントの TUI を止めない）
            self.tree_selection = None;
            cx.notify();
            return true;
        }
        // #1895: ⇧↑ / ⇧↓ で範囲を伸ばす・⌘⌫（Windows は Delete）でごみ箱へ。#1908: ↑ / ↓ /
        // ← / → / Enter / ⇧⌘↑ / ⇧⌘↓（Windows は Shift+Ctrl+Home / End）。A/B
        // （`TAKO_1895_LEGACY=1` / `TAKO_1908_LEGACY=1`）では受けない = その前と同じくペインへ流れる
        if let Some(key) = tako_core::platform::keys::tree_select_key(
            tako_core::platform::support::Platform::current(),
            &ks.key,
            m.platform,
            m.control,
            m.alt,
            m.shift,
        ) {
            if tako_core::file_copy::legacy_1895()
                || (key.since_1908() && tako_core::file_copy::legacy_1908())
            {
                return false;
            }
            self.tree_select_key_action(key, &sel, cx);
            return true;
        }
        let Some(key) = tako_core::platform::keys::tree_clip_key(
            tako_core::platform::support::Platform::current(),
            &ks.key,
            m.platform,
            m.control,
            m.alt,
            m.shift,
            ks.key_char.as_deref().is_some_and(|c| !c.is_empty()),
        ) else {
            return false;
        };
        // #1867 の A/B: ⌥⌘V はツリーへ向けない（#1867 の前 = ペインへ流れる）
        if key == tako_core::platform::keys::TreeClipKey::PasteMove
            && tako_core::tree_select::multi_legacy()
        {
            return false;
        }
        self.tree_clip_action(key, &sel, cx);
        true
    }

    /// コピー / 切り取り / 貼り付け（キー・右クリックメニューの共通の入口。#1860）。
    ///
    /// 中身はどれも dispatch `FileOp`（`clipboard_copy` / `clipboard_cut` / `paste`）=
    /// CLI `tako file clipboard` / `tako file paste`・MCP `tako_file_op` と同じ 1 本
    pub(crate) fn tree_clip_action(
        &mut self,
        key: tako_core::platform::keys::TreeClipKey,
        sel: &TreeSelection,
        cx: &mut Context<Self>,
    ) {
        use tako_control::protocol::{FileOpKind, Request};
        use tako_core::platform::keys::TreeClipKey;
        // 右クリックメニューを開いたままキーで操作したときもメニューは畳む
        self.context_menu = None;
        let op = match key {
            TreeClipKey::Copy => crate::ui_text::sidebar::menu_copy(),
            TreeClipKey::Cut => crate::ui_text::sidebar::menu_cut(),
            TreeClipKey::Paste => crate::ui_text::sidebar::menu_paste(),
            TreeClipKey::PasteMove => crate::ui_text::sidebar::menu_paste_move(),
        };
        let target = sel.path.display().to_string();
        if sel.remote {
            // ローカルのファイルシステムの操作を通さない（#919）。端末へも流さない
            self.notify_tree_op_failed(
                op,
                Some(&target),
                crate::ui_text::sidebar::clip_remote_refused(),
            );
            cx.notify();
            return;
        }
        // #1867: 選んでいる行すべて（見えている行だけ。見出しが 1 つでも混ざれば切り取りは断る）
        let paths = if sel.is_multi() {
            self.tree_selection_paths()
        } else {
            vec![sel.path.clone()]
        };
        let any_root = sel.root || paths.iter().any(|p| self.filetree.roots().contains(p));
        let kind = match key {
            TreeClipKey::Copy => FileOpKind::ClipboardCopy,
            TreeClipKey::Cut if any_root => {
                // 見出しのフォルダは画面からは動かさない（#1834 の D&D と同じ方針。
                // CLI / MCP はパスを名指しした時点で意図が明らかなので断らない）
                self.notify_tree_op_failed(
                    op,
                    Some(&target),
                    &crate::ui_text::sidebar::move_refused(
                        &tako_core::file_move::MoveRefusal::WorkspaceRoot,
                    ),
                );
                cx.notify();
                return;
            }
            TreeClipKey::Cut => FileOpKind::ClipboardCut,
            TreeClipKey::Paste => {
                self.tree_paste(&sel.path, false, cx);
                return;
            }
            // #1867: ⌥⌘V = 移動として貼る（宛先は最後に押した行 = ⌘V と同じ規則）
            TreeClipKey::PasteMove => {
                self.tree_paste(&sel.path, true, cx);
                return;
            }
        };
        let request = if paths.len() > 1 {
            // #1867: まとめて置く（CLI `tako file clipboard copy a b` / MCP `paths` と同じ 1 要求）
            Request::FileOpMany {
                op: kind,
                paths: paths.iter().map(|p| p.display().to_string()).collect(),
                dest: None,
            }
        } else {
            Request::FileOp {
                op: kind,
                path: target.clone(),
                name: None,
                pane: None,
                dest: None,
            }
        };
        let result = tako_control::dispatch(self, request, PaneOrigin::User);
        if let Err(e) = result {
            self.notify_tree_dispatch_failed(op, Some(&target), &e);
        }
        cx.notify();
    }

    /// 右クリックメニューを開くときに「貼り付け」を押せるか（#1860）。
    ///
    /// **OS のクリップボードの中身は読まない**（変更番号と「ファイルがあるか」だけ =
    /// `file_clipboard::can_paste`）。押しただけで中身を読むと、新しい macOS は
    /// pasteboard のプライバシーの確認を出しうる。中身は実際に貼るときに dispatch が読む
    pub(crate) fn tree_can_paste(&self) -> bool {
        if tako_core::file_copy::tree_keys_legacy() {
            return false;
        }
        tako_core::file_clipboard::can_paste(
            self.file_clipboard.as_ref(),
            &tako_control::platform::file_clipboard::peek(),
        )
    }

    /// 貼り付け（#1860）。IPC と同じ 3 段を通る: `prepare_offload`（UI スレッドで
    /// クリップボードを読んで段取り）→ `run_staged`（background で写す）→
    /// `finish_offload`（UI スレッドでツリーの読み直し）。切り取り = 移動は
    /// 付け替えに workspace が要るので同期の dispatch（`prepare_offload` が None を返す）
    pub(crate) fn tree_paste(
        &mut self,
        row: &std::path::Path,
        as_move: bool,
        cx: &mut Context<Self>,
    ) {
        let request = tako_control::protocol::Request::FileOp {
            // #1867: ⌥⌘V は移動として貼る（同期 = 切り取りの貼り付けと同じ経路）
            op: if as_move {
                tako_control::protocol::FileOpKind::PasteMove
            } else {
                tako_control::protocol::FileOpKind::Paste
            },
            path: row.display().to_string(),
            name: None,
            pane: None,
            dest: None,
        };
        let row = row.to_path_buf();
        match tako_control::prepare_offload(self, &request) {
            Some(Err(e)) => {
                self.present_tree_paste(Err(e), &row, cx);
            }
            Some(Ok(job)) => {
                // #1867: 写している間は帯で進み具合を見せる（取り消せる）
                self.kick_copy_progress(cx);
                let staged = cx
                    .background_executor()
                    .spawn(async move { job.run_staged() });
                cx.spawn(async move |this, cx| {
                    let outcome = staged.await;
                    let _ = this.update(cx, |app, cx| {
                        let result = match outcome {
                            tako_control::OffloadOutcome::OnUi(next) => {
                                app.finish_offload_on_ui(next, PaneOrigin::User, cx).0
                            }
                            tako_control::OffloadOutcome::Reply(result) => result,
                        };
                        app.present_tree_paste(result, &row, cx);
                    });
                })
                .detach();
            }
            None => {
                let result = tako_control::dispatch(self, request, PaneOrigin::User);
                // 移動の付け替えは dispatch の中（`file_moved`）で済んでいる。読み込み中だった
                // プレビューの読み直しと、パスの変わったレイアウトの保存だけ（D&D と同じ）
                self.drain_pending_preview_loads(cx);
                self.save_layout();
                match result {
                    Ok(value) => self.present_tree_paste(Ok(value), &row, cx),
                    // #1399: 失敗を黙って捨てない（offload の経路と同じ通知の口）
                    Err(e) => self.notify_tree_dispatch_failed(
                        if as_move {
                            crate::ui_text::sidebar::menu_paste_move()
                        } else {
                            crate::ui_text::sidebar::menu_paste()
                        },
                        Some(&row.display().to_string()),
                        &e,
                    ),
                }
            }
        }
        cx.notify();
    }

    /// 貼り付けの答えの見せ方（#1860）。貼れたら最後に置いたものを選び直し
    /// （続けて ⌘V を押すとそこへ貼れる = Finder / VSCode と同じ）、貼れなかったものは
    /// 通知欄へ理由を出す（一部だけなら件数 + 1 件目の理由）
    fn present_tree_paste(
        &mut self,
        result: Result<serde_json::Value, tako_control::DispatchError>,
        row: &std::path::Path,
        cx: &mut Context<Self>,
    ) {
        let op = crate::ui_text::sidebar::menu_paste();
        let target = row.display().to_string();
        match result {
            Ok(value) => {
                let pasted = value["pasted"].as_array().cloned().unwrap_or_default();
                let failed = value["failed"].as_array().cloned().unwrap_or_default();
                let cancelled = failed
                    .iter()
                    .any(|f| is_copy_cancelled(f["reason"].as_str().unwrap_or_default()));
                if cancelled {
                    // #1867: 自分で押した「取り消し」は失敗として出さない（写し終えたものは残る）
                    self.set_remote_notice(
                        crate::ui_text::sidebar::copy_cancelled().to_string(),
                        false,
                    );
                } else if let Some(first) = failed.first() {
                    let reason = first["reason"].as_str().unwrap_or_default();
                    let text = crate::ui_text::sidebar::clip_paste_partial(
                        failed.len(),
                        failed.len() + pasted.len(),
                        reason,
                    );
                    self.notify_tree_op_failed(op, Some(&target), &text);
                }
                // #1867: 貼ったもの全部を選び直す（Finder と同じ。続けて ⌘X で全部を動かせる）
                let landed: Vec<std::path::PathBuf> = pasted
                    .iter()
                    .filter_map(|v| v["to"].as_str())
                    .map(std::path::PathBuf::from)
                    .collect();
                if !landed.is_empty() && (self.tree_selection.is_some() || self.filetree.visible) {
                    self.select_tree_rows(&landed);
                }
            }
            // #1867: 自分で押した「取り消し」は失敗として出さない（作りかけは消してある）
            Err(e) if is_copy_cancelled(&e.to_string()) => {
                self.set_remote_notice(crate::ui_text::sidebar::copy_cancelled().to_string(), false)
            }
            Err(e) => self.notify_tree_dispatch_failed(op, Some(&target), &e),
        }
        cx.notify();
    }

    /// 切り取り中の tako の中身が古くなっていたら捨てる（#1860。2 秒のポーリングから呼ぶ）。
    ///
    /// Finder やほかのアプリで何かをコピーした = OS のクリップボードの変更番号が進んだら、
    /// 貼るのは OS の中身（`file_clipboard::resolve`）なので、薄く描いた行も戻す
    pub(crate) fn drop_stale_file_clipboard(&mut self) -> bool {
        let Some(clip) = self.file_clipboard.as_ref() else {
            return false;
        };
        // 変更番号だけを見る（中身は読まない = 2 秒ごとに pasteboard を読まない）
        let stamp = tako_control::platform::file_clipboard::peek().stamp;
        if !tako_core::file_clipboard::is_stale(clip, stamp) {
            return false;
        }
        self.file_clipboard = None;
        true
    }

    // --- 複数選択・まとめた操作・コピーの進み具合（FR-3.38 / #1867） -------------------------

    /// 押下の捕捉フェーズで選択を外して退避する（main の `capture_any_mouse_down`）。外したら真。
    /// 行は自分の押下 / クリックで退避した選択を見て選び直す（⌘クリックで足す・選んだ行を掴む）
    pub(crate) fn stash_tree_selection(&mut self) -> bool {
        let had = self.tree_selection.is_some();
        self.tree_multi.stash = self.tree_selection.take();
        had
    }

    /// 押下の前に効いていた選択（[`Self::active_tree_selection`] と同じ条件 = タブ・フォーカス
    /// ペインが同じでツリーが見えている）
    fn tree_selection_base(&self) -> Option<&TreeSelection> {
        if tako_core::file_copy::tree_keys_legacy() {
            return None;
        }
        self.tree_multi.stash.as_ref().filter(|sel| {
            self.filetree.visible
                && sel.tab == self.workspace.active_tab_id()
                && sel.pane == self.focused_pane()
        })
    }

    /// 押した行が押下前の複数選択に含まれていたら、その選択を戻す（右クリック・掴んで運ぶ・
    /// メニューの項目）。戻したら真。1 行だけの選択は戻さない（押した行を選び直すのと同じ）
    pub(crate) fn keep_tree_selection_for(&mut self, path: &std::path::Path) -> bool {
        let Some(base) = self
            .tree_selection_base()
            .filter(|sel| !sel.remote && sel.is_multi() && sel.contains(path))
            .cloned()
        else {
            return false;
        };
        self.tree_selection = Some(base);
        true
    }

    /// いま 2 行以上を選んでいる
    pub(crate) fn tree_multi_selected(&self) -> bool {
        self.active_tree_selection()
            .is_some_and(TreeSelection::is_multi)
    }

    /// まとめて扱うもの（選んでいる行のうち、いま見えている行。畳んだフォルダの中は外す）
    pub(crate) fn tree_selection_paths(&mut self) -> Vec<std::path::PathBuf> {
        let Some(sel) = self.active_tree_selection().cloned() else {
            return Vec::new();
        };
        let order = tree_visible_order(&self.filetree.rows());
        sel.as_core()
            .retain_visible(&order)
            .map(|s| s.items)
            .unwrap_or_else(|| vec![sel.path.clone()])
    }

    /// 修飾つきの押下（⌘ = 足す / 外す・⇧ = 範囲。素の押下は [`Self::select_tree_row`]）。
    /// 状態遷移と範囲の正本は `tako_core::tree_select::apply`
    pub(crate) fn click_tree_row(
        &mut self,
        path: &std::path::Path,
        kind: tako_core::tree_select::ClickKind,
    ) {
        let rows = self.filetree.rows();
        let order = tree_visible_order(&rows);
        let base = self
            .tree_selection_base()
            .filter(|sel| !sel.remote)
            .map(TreeSelection::as_core);
        let next = tako_core::tree_select::apply(base.as_ref(), path, kind, &order);
        self.tree_selection = next.map(|sel| self.tree_selection_of(sel, &rows));
    }

    /// core の選択を画面の選択へ（選んだ時点のタブとフォーカスペインを覚える）
    fn tree_selection_of(
        &self,
        sel: tako_core::tree_select::Selection,
        rows: &[filetree::Row],
    ) -> TreeSelection {
        let is_dir = rows
            .iter()
            .find(|r| r.remote.is_none() && r.entry.path == sel.lead)
            .map_or_else(|| sel.lead.is_dir(), |r| r.entry.is_dir);
        TreeSelection {
            path: sel.lead.clone(),
            is_dir,
            root: self.filetree.roots().contains(&sel.lead),
            remote: false,
            tab: self.workspace.active_tab_id(),
            pane: self.focused_pane(),
            paths: sel.items,
            anchor: sel.anchor,
        }
    }

    // --- キーでの選択・範囲選択・ごみ箱（FR-3.39 / #1895・FR-3.40 / #1908） ---------------

    /// ↑ / ↓ / ← / → / Enter / ⇧↑ / ⇧↓ / ⇧⌘↑ / ⇧⌘↓（Windows は Shift+Ctrl+Home / End）/
    /// ⌘⌫（Windows は Delete）。選んでいる間だけ `handle_tree_clip_keystroke` から来る
    pub(crate) fn tree_select_key_action(
        &mut self,
        key: tako_core::platform::keys::TreeSelectKey,
        sel: &TreeSelection,
        cx: &mut Context<Self>,
    ) {
        // 右クリックメニューを開いたままキーで操作したときもメニューは畳む
        self.context_menu = None;
        let Some(select) = key.select_key() else {
            self.trash_tree_selection(sel, cx);
            cx.notify();
            return;
        };
        // 状態遷移の正本は core（`tree_select::on_key`。⇧↑ / ⇧↓ は ⇧クリックと同じ `apply` を
        // 1 行上 / 下で呼ぶ `extend`）で、CLI `tako tree selection --key` / MCP `tako_tree_folder`
        // の `selection` と同じ dispatch を通す（画面だけの経路を作らない。リモート（SSH）の行は
        // dispatch が動かさない = #1895 と同じ）
        let result = tako_control::dispatch(
            self,
            tako_control::protocol::Request::TreeSelection {
                path: None,
                key: Some(select.as_str().to_string()),
                tab: None,
            },
            PaneOrigin::User,
        );
        match result {
            Ok(value) => {
                if value.get("opened").is_some() {
                    // 行を押して開いたときと同じ後始末（`open_file_row`）
                    self.drain_pending_highlights(cx);
                }
            }
            Err(e) if select == tako_core::tree_select::Key::Enter => {
                // 行を押して開けなかったときと同じ文言（#1283 / #1399）
                let target = sel.path.display().to_string();
                self.notify_tree_open_failed("open-file", &target, &e.to_string());
            }
            Err(e) => {
                let target = sel.path.display().to_string();
                self.notify_tree_dispatch_failed(
                    crate::ui_text::sidebar::tree_key_op(),
                    Some(&target),
                    &e,
                );
            }
        }
        cx.notify();
    }

    /// host のプリミティブ（#1908）: いま見えているローカルの行の形（ツリーが閉じていれば空）
    pub(crate) fn host_tree_rows(&mut self) -> Vec<tako_core::tree_select::RowShape> {
        if !self.filetree.visible {
            return Vec::new();
        }
        tree_row_shapes(&self.filetree.rows())
    }

    /// host のプリミティブ（#1908）: 選択を置き換えて、最後に押した行を見えるところへ
    pub(crate) fn host_set_tree_selection(
        &mut self,
        sel: tako_core::tree_select::Selection,
    ) -> Result<(), String> {
        if !self.filetree.visible {
            return Err("ファイルツリーが閉じている".into());
        }
        let rows = self.filetree.rows();
        let lead = sel.lead.clone();
        self.tree_selection = Some(self.tree_selection_of(sel, &rows));
        // 動いた先の行を見えるところへ（ツリーの子 = 行の並びの添字）
        if let Some(index) = rows
            .iter()
            .position(|r| r.remote.is_none() && r.entry.path == lead)
        {
            self.filetree_scroll_handle.scroll_to_item(index);
        }
        Ok(())
    }

    /// host のプリミティブ（#1908）: フォルダの行を開く / 畳む（行を押したのと同じ開閉）
    pub(crate) fn host_set_tree_expanded(&mut self, dir: &std::path::Path, expanded: bool) {
        let open = self
            .filetree
            .rows()
            .iter()
            .any(|r| r.remote.is_none() && r.entry.path == dir && r.expanded);
        if expanded && !open {
            self.filetree.expand_dir(dir);
        } else if !expanded && open {
            self.filetree.toggle_dir(dir);
        }
    }

    /// ⌘⌫（Windows は Delete）: 選んでいるもの（見えている行だけ）をごみ箱へ。リモートと
    /// 見出しのフォルダは理由を出して断る（見出しは #1834 / #1860 と同じく画面からは動かさない =
    /// 打ち間違いでワークスペースごとごみ箱へ入れない。CLI / MCP はパスを名指しするので断らない）
    fn trash_tree_selection(&mut self, sel: &TreeSelection, cx: &mut Context<Self>) {
        let fm = tako_control::platform::os_integration::file_manager();
        let op = crate::ui_text::sidebar::menu_trash(fm);
        let target = sel.path.display().to_string();
        if sel.remote {
            self.notify_tree_op_failed(
                op,
                Some(&target),
                crate::ui_text::sidebar::trash_remote_refused(),
            );
            return;
        }
        let paths = if sel.is_multi() {
            self.tree_selection_paths()
        } else {
            vec![sel.path.clone()]
        };
        let roots = self.filetree.roots();
        if sel.root || paths.iter().any(|p| roots.contains(p)) {
            self.notify_tree_op_failed(
                op,
                Some(&target),
                crate::ui_text::sidebar::trash_root_refused(),
            );
            return;
        }
        self.trash_tree_paths(paths, cx);
    }

    /// ごみ箱へ（右クリックの「削除」と ⌘⌫ の共通の口）。1 件 = `FileOp`、複数 = `FileOpMany`
    /// （CLI `tako file trash a` / `tako file trash a b` と同じ要求）。入れたら選択を外す
    /// （無くなった行を ⌘C の対象に残さない）
    pub(crate) fn trash_tree_paths(
        &mut self,
        paths: Vec<std::path::PathBuf>,
        cx: &mut Context<Self>,
    ) {
        use tako_control::protocol::{FileOpKind, Request};
        let fm = tako_control::platform::os_integration::file_manager();
        let op = crate::ui_text::sidebar::menu_trash(fm);
        if paths.len() > 1 {
            self.dispatch_tree_many(FileOpKind::Trash, paths, None, op, cx);
        } else if let Some(path) = paths.first() {
            let target = path.display().to_string();
            // #1399: **削除を約束しているラベル**なので、失敗を無言にすると
            // 「消えたのか押せていないのか」がユーザーに区別できない
            let result = tako_control::dispatch(
                self,
                Request::FileOp {
                    op: FileOpKind::Trash,
                    path: target.clone(),
                    name: None,
                    pane: None,
                    dest: None,
                },
                PaneOrigin::User,
            );
            match result {
                Ok(_) => {
                    if self
                        .tree_selection
                        .as_ref()
                        .is_some_and(|s| s.contains(path))
                    {
                        self.tree_selection = None;
                    }
                }
                Err(e) => self.notify_tree_dispatch_failed(op, Some(&target), &e),
            }
        }
        self.sync_filetree_roots();
        cx.notify();
    }

    /// 複数の行を選ぶ（貼った・移したものを選び直す）。最後のものが宛先・最初が起点
    pub(crate) fn select_tree_rows(&mut self, paths: &[std::path::PathBuf]) {
        let Some(last) = paths.last() else {
            return;
        };
        self.select_tree_row(last, last.is_dir(), false);
        if let Some(sel) = self.tree_selection.as_mut() {
            sel.paths = paths.to_vec();
            sel.anchor = paths[0].clone();
        }
    }

    /// ドラッグ中の判定（1 つなら #1834 の `tree_drop_hover`、まとめてなら `tree_drop_hover_many`）
    fn tree_drag_hover(
        &self,
        drag: &FileDrag,
        row: &std::path::Path,
        row_is_dir: bool,
        row_remote: bool,
    ) -> Option<TreeDropHover> {
        if drag.paths.len() <= 1 {
            return tree_drop_hover(&drag.path, drag.root, row, row_is_dir, row_remote);
        }
        let roots = self.filetree.roots();
        let items: Vec<(std::path::PathBuf, bool)> = drag
            .paths
            .iter()
            .map(|p| (p.clone(), roots.contains(p)))
            .collect();
        tree_drop_hover_many(&items, &drag.path, row, row_is_dir, row_remote)
    }

    /// まとめて運んだものをツリーの行へ落とす（#1867）。最終判断は dispatch の `FileOpMany`
    /// （1 件ずつ #1834 の移動を通る）。画面が先に止めるのは、dispatch へ渡せないリモートの行と
    /// 画面の方針で断る見出し・落とし先を壊す理由（選んだフォルダの上・その配下）だけ
    fn drop_many_on_tree_row(
        &mut self,
        drag: &FileDrag,
        hover: &TreeDropHover,
        cx: &mut Context<Self>,
    ) {
        use tako_control::protocol::FileOpKind;
        use tako_core::file_move::{DropVerdict, MoveRefusal};
        let op = crate::ui_text::sidebar::move_op();
        match &hover.verdict {
            DropVerdict::Unchanged => {}
            DropVerdict::Refused(
                refusal @ (MoveRefusal::Remote
                | MoveRefusal::WorkspaceRoot
                | MoveRefusal::IntoSelf
                | MoveRefusal::IntoDescendant),
            ) => {
                let target = hover.dest.display().to_string();
                self.notify_tree_op_failed(
                    op,
                    Some(&target),
                    &crate::ui_text::sidebar::move_refused(refusal),
                );
            }
            DropVerdict::Move | DropVerdict::Refused(_) => {
                self.dispatch_tree_many(
                    FileOpKind::Move,
                    drag.paths.clone(),
                    Some(hover.dest.clone()),
                    op,
                    cx,
                );
            }
        }
        cx.notify();
    }

    /// まとめた操作を dispatch へ渡し、結果を見せる（#1867）。移したものは移した先を選び直し、
    /// 一部できなかったら件数 + 1 件目の理由を通知欄へ（全部は CLI / MCP の `failed`）
    pub(crate) fn dispatch_tree_many(
        &mut self,
        op: tako_control::protocol::FileOpKind,
        paths: Vec<std::path::PathBuf>,
        dest: Option<std::path::PathBuf>,
        label: &str,
        cx: &mut Context<Self>,
    ) {
        let first = paths.first().map(|p| p.display().to_string());
        let result = tako_control::dispatch(
            self,
            tako_control::protocol::Request::FileOpMany {
                op,
                paths: paths.iter().map(|p| p.display().to_string()).collect(),
                dest: dest.map(|d| d.display().to_string()),
            },
            PaneOrigin::User,
        );
        // 移動の付け替えは dispatch の中（`file_moved`）で済んでいる（D&D と同じ後始末）
        self.drain_pending_preview_loads(cx);
        self.save_layout();
        match result {
            Ok(value) => {
                let failed = value["failed"].as_array().cloned().unwrap_or_default();
                if let Some(reason) = failed.first().and_then(|f| f["reason"].as_str()) {
                    let done = value["done"].as_array().map_or(0, Vec::len);
                    let text = crate::ui_text::sidebar::multi_partial(
                        label,
                        failed.len(),
                        done + failed.len(),
                        reason,
                    );
                    self.notify_tree_op_failed(label, first.as_deref(), &text);
                }
                let moved: Vec<std::path::PathBuf> = value["done"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|v| v["to"].as_str())
                    .map(std::path::PathBuf::from)
                    .collect();
                if moved.is_empty() {
                    self.tree_selection = None;
                } else {
                    self.select_tree_rows(&moved);
                }
            }
            Err(e) => self.notify_tree_dispatch_failed(label, first.as_deref(), &e),
        }
        cx.notify();
    }

    /// コピーの進み具合の帯を描き直す刻みを回す（走っているコピーが無くなったら止まる）。
    /// GUI の ⌘V は始めた直後に、CLI / MCP から始まったコピーは 2 秒のポーリングが呼ぶ
    pub(crate) fn kick_copy_progress(&mut self, cx: &mut Context<Self>) {
        if self.tree_multi.ticking || tako_core::file_copy::jobs().list().is_empty() {
            return;
        }
        self.tree_multi.ticking = true;
        cx.spawn(async move |this, cx| loop {
            cx.background_executor().timer(COPY_PROGRESS_TICK).await;
            let alive = this.update(cx, |app, cx| {
                cx.notify();
                let running = !tako_core::file_copy::jobs().list().is_empty();
                if !running {
                    app.tree_multi.ticking = false;
                }
                running
            });
            if !matches!(alive, Ok(true)) {
                break;
            }
        })
        .detach();
    }

    /// コピーの進み具合の帯（件数・バイト・取り消し）。走り始めて [`COPY_PROGRESS_DELAY`] 経った
    /// コピーだけ出す（短いコピーでちらつかせない）。A/B（`TAKO_1867_LEGACY=1`）では出さない
    pub(crate) fn render_copy_progress(
        &self,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> Vec<gpui::AnyElement> {
        if tako_core::tree_select::multi_legacy() {
            return Vec::new();
        }
        let probe = self.tree_row_probe.then(|| self.tree_row_rects.clone());
        tako_core::file_copy::jobs()
            .list()
            .into_iter()
            .filter(|ticket| ticket.elapsed() >= COPY_PROGRESS_DELAY)
            .map(|ticket| {
                let snap = ticket.progress.snapshot();
                let text = if snap.cancelled {
                    crate::ui_text::sidebar::copy_cancelling().to_string()
                } else if snap.counting {
                    crate::ui_text::sidebar::copy_counting(snap.entries_total)
                } else {
                    crate::ui_text::sidebar::copy_progress(
                        snap.entries_done,
                        snap.entries_total,
                        &format_bytes(snap.bytes_done),
                        &format_bytes(snap.bytes_total),
                    )
                };
                let ratio = copy_ratio(&snap);
                // #1895: 残り時間の目安（出せないうちは出さない = `file_copy::eta` の 1 実装。
                // CLI / MCP の `copy_progress` の `eta_secs` と同じ値を丸めて書く）。上の行は
                // 狭いと末尾が省略されるので、棒の行の右端に置く
                let eta = tako_core::file_copy::eta(&snap)
                    .filter(|_| !tako_core::file_copy::legacy_1895())
                    .map(|left| {
                        crate::ui_text::sidebar::copy_eta(tako_core::file_copy::eta_label(left))
                    });
                let eta_probe = probe.clone().filter(|_| eta.is_some());
                let id = ticket.id;
                div()
                    .id(("copy-progress", id))
                    .flex_none()
                    .flex()
                    .flex_col()
                    .gap(px(3.0))
                    .px(px(10.0))
                    .py(px(5.0))
                    .text_size(px(11.0))
                    .border_b_1()
                    .border_color(hsla_alpha(theme.pane_border, 0.6))
                    .bg(rgba_alpha(theme.accent, 0.08))
                    .text_color(hsla(theme.foreground))
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap(px(6.0))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w(px(0.0))
                                    .overflow_hidden()
                                    .whitespace_nowrap()
                                    .text_ellipsis()
                                    .child(SharedString::from(text)),
                            )
                            .when(!snap.cancelled, |d| {
                                d.child(
                                    div()
                                        .id(("copy-cancel", id))
                                        .flex_none()
                                        .px(px(6.0))
                                        .rounded_sm()
                                        .border_1()
                                        .border_color(hsla(theme.pane_border))
                                        .cursor_pointer()
                                        .hover(|d| d.bg(rgba(theme.surface_hover)))
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            this.cancel_copy(id, cx);
                                        }))
                                        // 何も描かない矩形採取（`tree_row_probe` のときだけ）
                                        .when_some(probe.clone(), |d, rects| {
                                            d.relative().child(
                                                canvas(
                                                    move |bounds, _, _| {
                                                        rects.borrow_mut().push((
                                                            std::path::PathBuf::from(
                                                                COPY_CANCEL_PROBE,
                                                            ),
                                                            bounds,
                                                        ))
                                                    },
                                                    |_, _, _, _| (),
                                                )
                                                .absolute()
                                                .top_0()
                                                .left_0()
                                                .size_full(),
                                            )
                                        })
                                        .child(SharedString::from(
                                            crate::ui_text::sidebar::copy_cancel(),
                                        )),
                                )
                            }),
                    )
                    // 進み具合の棒（数えている間は出さない = 母数が決まっていない）と、右端に
                    // 残り時間の目安（#1895）。行の高さは棒が出た時点で決める（目安が後から
                    // 出てもツリーの行を動かさない）
                    .when_some(ratio, |d, ratio| {
                        d.child(
                            div()
                                .flex()
                                .flex_row()
                                .items_center()
                                .gap(px(6.0))
                                .h(px(14.0))
                                .child(
                                    div()
                                        .flex_1()
                                        .h(px(3.0))
                                        .rounded_sm()
                                        .bg(hsla_alpha(theme.pane_border, 0.5))
                                        .child(
                                            div()
                                                .h_full()
                                                .rounded_sm()
                                                .bg(hsla(theme.accent))
                                                .w(relative(ratio)),
                                        ),
                                )
                                .when_some(eta, |d, eta| {
                                    d.child(
                                        div()
                                            .flex_none()
                                            .relative()
                                            .whitespace_nowrap()
                                            .text_color(hsla(theme.text_muted))
                                            .child(SharedString::from(eta))
                                            // 何も描かない矩形採取（`tree_row_probe` のときだけ）
                                            .when_some(eta_probe, |d, rects| {
                                                d.child(
                                                    canvas(
                                                        move |bounds, _, _| {
                                                            rects.borrow_mut().push((
                                                                std::path::PathBuf::from(
                                                                    COPY_ETA_PROBE,
                                                                ),
                                                                bounds,
                                                            ))
                                                        },
                                                        |_, _, _, _| (),
                                                    )
                                                    .absolute()
                                                    .top_0()
                                                    .left_0()
                                                    .size_full(),
                                                )
                                            }),
                                    )
                                }),
                        )
                    })
                    .into_any_element()
            })
            .collect()
    }

    /// 帯の「取り消し」（CLI `tako file cancel <id>` / MCP `copy_cancel` と同じ dispatch）
    pub(crate) fn cancel_copy(&mut self, id: u64, cx: &mut Context<Self>) {
        let result = tako_control::dispatch(
            self,
            tako_control::protocol::Request::FileOp {
                op: tako_control::protocol::FileOpKind::CopyCancel,
                path: String::new(),
                name: Some(id.to_string()),
                pane: None,
                dest: None,
            },
            PaneOrigin::User,
        );
        // #1399: 失敗を黙って捨てない（押した直後に写し終えた = 「走っていない」もここへ出る）
        if let Err(e) = result {
            self.notify_tree_dispatch_failed(crate::ui_text::sidebar::copy_cancel(), None, &e);
        }
        cx.notify();
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
                // #1399: `eprintln!` は GUI では誰も読めないので persist.log へ。
                // **通知欄には出さない**（ユーザーの操作が無い背景の保守処理で、
                // バナーを出すと押していない操作の失敗が画面に居座る）。
                // #1422 で書式ごと 1 実装（`log_ui_failure`）へ寄せた
                Self::log_ui_failure(
                    NoticeArea::Tree,
                    NoticeArm::Issue1399,
                    "preview-watch-sync",
                    "watcher_sync",
                );
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
            if preview::external_change_legacy() {
                // #1659 前の経路（A/B の口）: 中身が本文と違えば毎回競合として知らせ直す
                if loaded.source_bytes.as_deref() == Some(edit.buffer.text().as_bytes()) {
                    return;
                }
                edit.note_conflict(tako_core::DiskState::Changed);
                edit.message = Some(crate::ui_text::sidebar::note_external_change().into());
                cx.notify();
                return;
            }
            // #1659: 基準（開いた / 保存した / 読み直した時点の中身）と突き合わせる。
            // 自分自身の保存でも OS イベントは発生するが、そのときは基準と同じ = 競合ではない。
            // 読めなかった（消された・上限を超えた）ときだけ、ここで読み直して区別する
            let state = match loaded.source_bytes.as_deref() {
                Some(bytes) => edit.buffer.observe_disk(Some(bytes)),
                None => edit
                    .buffer
                    .refresh_disk_state()
                    .unwrap_or(tako_core::DiskState::Changed),
            };
            let mut follow = false;
            match state {
                // 外で元へ戻された・外で自分と同じ中身に書かれた = 競合が解けた
                tako_core::DiskState::Unchanged => {
                    edit.clear_conflict();
                }
                // 未編集ならディスクへ黙って追従する（VS Code / Zed と同じ。読み直しは
                // 1 回の編集として積むので undo で戻せる）。読み直せない中身なら競合として残す
                tako_core::DiskState::Changed if !edit.dirty() => {
                    match loaded
                        .source_bytes
                        .map(|bytes| edit.buffer.reload_from(bytes))
                    {
                        Some(Ok(())) => {
                            edit.clear_conflict();
                            follow = true;
                        }
                        _ => {
                            edit.note_conflict(state);
                        }
                    }
                }
                // 知らせるのは新しく分かったときだけ（同じ競合を何度検知しても 1 回）
                state => {
                    edit.note_conflict(state);
                }
            }
            if follow {
                self.refresh_preview_from_editor(pane);
            }
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

/// OS のアプリ選択 UI でアプリを選び、指定ファイルをそのアプリで開く。
/// プラットフォーム差（macOS = 選択ダイアログ + `open -a` / Windows = `openas` verb）は
/// 境界 B8（`platform::os_integration`）の内側にある（#617）
fn pick_app_and_open(path: &std::path::Path) -> Result<(), String> {
    tako_control::platform::os_integration::open_with_dialog(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    /// #1834: 掴んだ行の上では何も出さず、ファイル行の上はそのフォルダへ、
    /// リモートの行は理由つきで断る（判定は core の `drop_verdict` = dispatch と同じ規則）
    #[test]
    fn ツリーのドロップ判定は掴んだ行とファイル行とリモートを見分ける() {
        use tako_core::file_move::{DropVerdict, MoveRefusal};
        let src = PathBuf::from("/nonexistent-1834/w/a.txt");
        let w = PathBuf::from("/nonexistent-1834/w");
        let other = PathBuf::from("/nonexistent-1834/other");
        assert_eq!(tree_drop_hover(&src, false, &src, false, false), None);
        // 兄弟のファイル行の上 = 同じフォルダ = 何も起きない
        let sibling = tree_drop_hover(&src, false, &w.join("b.txt"), false, false).unwrap();
        assert_eq!(
            (sibling.dest.clone(), sibling.verdict.clone()),
            (w.clone(), DropVerdict::Unchanged)
        );
        // 別のフォルダのファイル行の上 = そのファイルのあるフォルダへ移せる
        let into = tree_drop_hover(&src, false, &other.join("c.txt"), false, false).unwrap();
        assert_eq!(
            (into.dest.clone(), into.verdict.clone()),
            (other.clone(), DropVerdict::Move)
        );
        // リモートの行は字面が同じでも断る
        let remote = tree_drop_hover(&src, false, &other, true, true).unwrap();
        assert_eq!(remote.verdict, DropVerdict::Refused(MoveRefusal::Remote));
        // 見出しの行を掴んだら、どこへ持って行っても断る
        let root = tree_drop_hover(&w, true, &other, true, false).unwrap();
        assert_eq!(
            root.verdict,
            DropVerdict::Refused(MoveRefusal::WorkspaceRoot)
        );

        // 強調: 移せるなら落とし先の行が Target・中の行が Inside、リモートの同じ字面には出さない
        assert_eq!(
            tree_drop_mark(Some(&into), &other, false),
            Some(TreeDropMark::Target)
        );
        assert_eq!(
            tree_drop_mark(Some(&into), &other.join("c.txt"), false),
            Some(TreeDropMark::Inside)
        );
        assert_eq!(tree_drop_mark(Some(&into), &other, true), None);
        assert_eq!(tree_drop_mark(Some(&into), &w, false), None);
        // 断るならカーソルの載っている行だけ（ローカルの同じ字面には出さない）
        assert_eq!(
            tree_drop_mark(Some(&remote), &other, true),
            Some(TreeDropMark::Refused(MoveRefusal::Remote))
        );
        assert_eq!(tree_drop_mark(Some(&remote), &other, false), None);
        // 同じ場所は何も出さない
        assert_eq!(tree_drop_mark(Some(&sibling), &w, false), None);
    }

    /// #1725: 長い名前でもキャレットと変換中の読みは必ず見え、窓からはみ出さない
    #[test]
    fn インライン入力の見える窓はキャレットと読みを必ず含む() {
        // 入り切るなら切らない
        assert_eq!(
            inline_input_window("a.txt", "", "", 30),
            ("a.txt".to_string(), String::new())
        );
        // 前が長いときは末尾を残して `…` を付ける（キャレット直前の字は必ず見える）
        let long = "tako-st1725-12345zqv3x";
        let (b, a) = inline_input_window(long, "しりょう", "", 20);
        let kept = b.strip_prefix('…').expect("切った側に … が付く");
        assert!(long.ends_with(kept) && !kept.is_empty(), "{b}");
        // 窓の幅 = 前 + 読み + キャレット 1 桁 が上限に収まる
        assert!(inline_cells(&b) + inline_cells("しりょう") < 20, "{b}");
        assert!(a.is_empty());
        // 全角は 2 桁で数える（途中で切らない = 文字単位）
        let jp = "資料".repeat(20);
        let (b, _) = inline_input_window(&jp, "", "", 11);
        assert!(inline_cells(&b) <= 10, "{b}");
        assert!(b.chars().skip(1).all(|c| c == '資' || c == '料'), "{b}");
        // 後ろは入るぶんだけ先頭から出して `…` を付ける
        let (b, a) = inline_input_window("ab", "", "cdefghijklmnop", 8);
        assert_eq!(b, "ab");
        let shown = a.strip_suffix('…').expect("切った側に … が付く");
        assert!("cdefghijklmnop".starts_with(shown), "{a}");
        assert!(inline_cells(&b) + inline_cells(&a) < 8, "{b}|{a}");
        // 読みだけで窓を使い切っても落ちない（前は `…` だけ・後ろは空）
        let (b, a) = inline_input_window("abc", "ながいよみがなです", "def", 6);
        assert!(inline_cells(&b) <= 1 && a.is_empty(), "{b}|{a}");
        // 窓が 0 桁でも落ちない
        let _ = inline_input_window("abc", "", "def", 0);
    }

    /// #1725: 見えていない入力欄は打鍵も変換も奪わない。ツリーが閉じている /
    /// 対象がどのルートの配下にも無い（タブを切り替えた等）ときは偽
    #[test]
    fn インライン入力の対象はツリーに出ているときだけ見える() {
        let roots = vec![PathBuf::from("/w/proj"), PathBuf::from("/w/other")];
        // ルートそのもの（ルート行の新規作成 / 名前を変更）とその配下は見える
        assert!(inline_edit_target_visible(
            true,
            &roots,
            std::path::Path::new("/w/proj")
        ));
        assert!(inline_edit_target_visible(
            true,
            &roots,
            std::path::Path::new("/w/proj/src/a.rs")
        ));
        assert!(inline_edit_target_visible(
            true,
            &roots,
            std::path::Path::new("/w/other/日本語")
        ));
        // ツリーを閉じていれば見えない
        assert!(!inline_edit_target_visible(
            false,
            &roots,
            std::path::Path::new("/w/proj/src")
        ));
        // どのルートの配下でもない（タブを切り替えてルートから外れた）
        assert!(!inline_edit_target_visible(
            true,
            &roots,
            std::path::Path::new("/w/elsewhere/x")
        ));
        // 名前の前方一致ではなくパスの要素で見る（`/w/proj2` は `/w/proj` の配下ではない）
        assert!(!inline_edit_target_visible(
            true,
            &roots,
            std::path::Path::new("/w/proj2/x")
        ));
        // ルートが 1 つも無いツリー
        assert!(!inline_edit_target_visible(
            true,
            &[],
            std::path::Path::new("/w/proj")
        ));
    }

    /// 画面の識別子は診断（`area=`）で画面を見分けるためのものなので、
    /// 2 つの画面が同じ札を名乗ったら「どこで起きたか」が消える（#1417 / #1432）
    #[test]
    fn 通知の画面札は重複せず識別子の形をしている() {
        let all = [
            NoticeArea::Tree,
            NoticeArea::RightPanel,
            NoticeArea::Preview,
            NoticeArea::Drawer,
            NoticeArea::CommandCard,
            NoticeArea::Chat,
            NoticeArea::OpenFile,
            NoticeArea::UpdateWindow,
        ];
        let mut tags: Vec<&str> = all.iter().map(|a| a.tag()).collect();
        for tag in &tags {
            assert!(
                !tag.is_empty() && tag.chars().all(|c| c.is_ascii_lowercase() || c == '_'),
                "画面札 {tag:?} が識別子の形でない（#1417）"
            );
        }
        let total = tags.len();
        tags.sort_unstable();
        tags.dedup();
        assert_eq!(tags.len(), total, "画面札が重複している（#1417 / #1432）");
    }

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
            remote: None,
            note: None,
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
