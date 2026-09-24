//! パスを tako の中で開くときの行き先（Issue #1283）
//!
//! cmd+クリック（#147 / #153）・cmd+右クリックの「tako で開く」（#1182）・
//! CLI `tako file open-in-tako` / MCP `tako_file_op` は**同じ振り分け**を通る。
//! その「拡張子 → どのプレビューで開くか」の対応表がこのモジュールの正本で、
//! `tako-control::dispatch` の `OpenFile` と、リンク検出の応答
//! （`tako links` / `tako_links` の `open`）が同じ表を引く。
//!
//! #1283 で「`.mp4` はプレビュー非対応だから無言で失敗しているのでは」という
//! 見立てが立ったが、実測では **`.mp4` は動画プレビュー（`PreviewRoute::Video`）へ
//! 行く**（真因はリンク検出側だった）。同じ問いが二度立たないよう、対応表を
//! 1 箇所へ置いて機械で読める形にしてある。

use std::path::Path;

/// パスを tako の中で開いた結果どこへ行くか
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpenRoute {
    /// ディレクトリ → そのディレクトリのシェルをペイン分割して開く
    Terminal,
    /// ファイル → プレビューペインで開く
    Preview(PreviewRoute),
}

/// プレビューの種別（`tako-control::protocol::PreviewModeWire` と 1:1）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PreviewRoute {
    Code,
    Markdown,
    Image,
    Pdf,
    Video,
}

impl PreviewRoute {
    /// 応答・ログに出す安定キー（`PreviewModeWire::as_str` と同じ綴り）
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Code => "code",
            Self::Markdown => "markdown",
            Self::Image => "image",
            Self::Pdf => "pdf",
            Self::Video => "video",
        }
    }

    /// 全種別（パリティ検査用）
    pub const ALL: [Self; 5] = [
        Self::Code,
        Self::Markdown,
        Self::Image,
        Self::Pdf,
        Self::Video,
    ];
}

impl OpenRoute {
    /// 応答・ログに出す安定キー
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Terminal => "terminal",
            Self::Preview(mode) => mode.as_str(),
        }
    }
}

/// 拡張子からプレビューの種別を決める（純粋関数。I/O をしない）。
///
/// 未知の拡張子と拡張子なし（`README` / `Makefile` / dotfile）は `Code`。
/// テキストとして読めないものは preview 側が「バイナリファイル」を出すので、
/// **どの拡張子でも無言にはならない**
pub fn preview_route_for_extension(ext: Option<&str>) -> PreviewRoute {
    let Some(ext) = ext else {
        return PreviewRoute::Code;
    };
    match ext.to_ascii_lowercase().as_str() {
        "md" | "markdown" => PreviewRoute::Markdown,
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "svg" => PreviewRoute::Image,
        "pdf" => PreviewRoute::Pdf,
        "mp4" | "webm" | "mov" | "avi" | "mkv" => PreviewRoute::Video,
        _ => PreviewRoute::Code,
    }
}

/// パスの拡張子からプレビューの種別を決める
pub fn preview_route(path: &Path) -> PreviewRoute {
    preview_route_for_extension(path.extension().and_then(|e| e.to_str()))
}

/// 拡張子から HTTP の `Content-Type` を決める（純粋関数。I/O をしない）。
///
/// **[`preview_route_for_extension`] と同じ表**（下のテストが「Image の拡張子は
/// `image/` で始まる」等を機械で拘束する）。分けてあるのは粒度だけで、
/// プレビューの振り分けは 5 種別・HTTP は `image/png` と `image/jpeg` を
/// 区別する必要があるため。
///
/// 使い道は **daemon が添付をその場で見せるとき**（#1472）。`<img>` / `<video>` は
/// `application/octet-stream` では鳴らない（とくに Safari は `<video>` の
/// MIME を見る）ので、インライン配信ではここが返す型を載せる。
/// 分からない拡張子は `None` = 呼び出し側が `application/octet-stream` へ倒す
pub fn media_type_for_extension(ext: Option<&str>) -> Option<&'static str> {
    let ext = ext?;
    Some(match ext.to_ascii_lowercase().as_str() {
        // 画像（`PreviewRoute::Image` と同じ集合）
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "svg" => "image/svg+xml",
        // 動画（`PreviewRoute::Video` と同じ集合）
        "mp4" => "video/mp4",
        "webm" => "video/webm",
        // QuickTime は `video/quicktime`。iOS / macOS の Safari はこれで鳴る
        "mov" => "video/quicktime",
        "avi" => "video/x-msvideo",
        "mkv" => "video/x-matroska",
        // 文書（`PreviewRoute::Pdf` / `Markdown`）
        "pdf" => "application/pdf",
        "md" | "markdown" => "text/markdown; charset=utf-8",
        _ => return None,
    })
}

/// パスの拡張子から HTTP の `Content-Type` を決める
pub fn media_type(path: &Path) -> Option<&'static str> {
    media_type_for_extension(path.extension().and_then(|e| e.to_str()))
}

/// 行番号が 1 始まりであることの説明（0 以下を弾いたときの文言の正本）。
///
/// CLI（負値を型で持てる `i64`）と dispatch（`usize` なので 0 だけ）の両方が
/// この 1 本を出すので、入口が違っても同じ文言になる
pub const LINE_ONE_BASED: &str = "行番号は 1 始まり（0 以下は指定できない）";

/// 桁番号が 1 始まりであることの説明（[`LINE_ONE_BASED`] の桁版）
pub const COLUMN_ONE_BASED: &str = "桁番号は 1 始まり（0 以下は指定できない）";

/// 行指定つきで開くときの表示種別を決める（純粋関数。Issue #1676）。
///
/// `line` が指す先は**原文の行**なので、着地できるのは「原文の 1 行が
/// そのまま 1 item になる」`Code` だけ。`Markdown` はレンダリング表示の
/// 1 item = 1 ブロック（#826）で原文の行が残らない（空行は消え、表は
/// 1 ブロックに複数行が入る）ため、**`line` を渡された時点でソース表示へ倒す**。
/// 画像・PDF・動画は行の概念を持たないのでエラーにする（PDF のページ送りは
/// `PreviewView` の `page` が持つ別の操作）
pub fn preview_route_with_line(route: PreviewRoute) -> Result<PreviewRoute, String> {
    match route {
        PreviewRoute::Code | PreviewRoute::Markdown => Ok(PreviewRoute::Code),
        PreviewRoute::Image | PreviewRoute::Pdf | PreviewRoute::Video => Err(format!(
            "行を指定して開けるのはテキスト（code / markdown）だけ: {}",
            route.as_str()
        )),
    }
}

/// 要求された行を文書の実寸へ丸める（純粋関数。Issue #1676）。
///
/// 戻りは `(着地する行, 丸めたか)` で、行は 1 始まり。行数を超える要求は
/// **末尾行へ丸める**（エラーにしない = 定義ジャンプの相手が古い行番号でも
/// ファイルは開いて見せる）。空ファイルは行 1 を指す
pub fn clamp_line(total_lines: usize, line: usize) -> Result<(usize, bool), String> {
    if line == 0 {
        return Err(LINE_ONE_BASED.to_string());
    }
    let last = total_lines.max(1);
    if line > last {
        Ok((last, true))
    } else {
        Ok((line, false))
    }
}

/// 要求された桁をその行の実寸へ丸める（純粋関数。Issue #1676）。
///
/// `line_chars` はその行の**文字数**（UTF-8 のバイト数ではない）。行末の
/// 次（`line_chars + 1`）までを有効な桁として受ける（キャレットが行末に立てる）
pub fn clamp_column(line_chars: usize, column: usize) -> Result<(usize, bool), String> {
    if column == 0 {
        return Err(COLUMN_ONE_BASED.to_string());
    }
    let last = line_chars + 1;
    if column > last {
        Ok((last, true))
    } else {
        Ok((column, false))
    }
}

/// パスを tako の中で開いたときの行き先。`is_dir` は呼び出し側が調べた実体の種別
pub fn route(path: &Path, is_dir: bool) -> OpenRoute {
    if is_dir {
        OpenRoute::Terminal
    } else {
        OpenRoute::Preview(preview_route(path))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 拡張子ごとの行き先() {
        assert_eq!(preview_route(Path::new("/a/clip.mp4")), PreviewRoute::Video);
        assert_eq!(preview_route(Path::new("/a/clip.MOV")), PreviewRoute::Video);
        assert_eq!(preview_route(Path::new("/a/doc.pdf")), PreviewRoute::Pdf);
        assert_eq!(preview_route(Path::new("/a/x.PNG")), PreviewRoute::Image);
        assert_eq!(
            preview_route(Path::new("/a/README.md")),
            PreviewRoute::Markdown
        );
        assert_eq!(preview_route(Path::new("/a/main.rs")), PreviewRoute::Code);
        // 拡張子なし・未知はテキストとして開く（読めなければ preview が理由を出す）
        assert_eq!(preview_route(Path::new("/a/Makefile")), PreviewRoute::Code);
        assert_eq!(
            preview_route(Path::new("/a/archive.zip")),
            PreviewRoute::Code
        );
    }

    #[test]
    fn ディレクトリはターミナル() {
        assert_eq!(route(Path::new("/a/dir"), true), OpenRoute::Terminal);
        assert_eq!(
            route(Path::new("/a/clip.mp4"), false),
            OpenRoute::Preview(PreviewRoute::Video)
        );
        assert_eq!(OpenRoute::Terminal.as_str(), "terminal");
        assert_eq!(OpenRoute::Preview(PreviewRoute::Video).as_str(), "video");
    }

    /// 拡張子の一覧（2 つの表が同じ集合を見ていることを確かめる材料）
    const KNOWN_EXTENSIONS: &[&str] = &[
        "png", "jpg", "jpeg", "gif", "webp", "svg", "mp4", "webm", "mov", "avi", "mkv", "pdf",
        "md", "markdown", "rs", "toml", "zip", "",
    ];

    /// #1472: **プレビューの振り分けと MIME の表がずれない**。
    /// 片方だけ拡張子を足すと（例: `.heic` を Image に足して MIME を忘れる）ここで落ちる
    #[test]
    fn mimeとプレビュー種別が同じ表を見ている() {
        for ext in KNOWN_EXTENSIONS {
            let ext = if ext.is_empty() { None } else { Some(*ext) };
            let route = preview_route_for_extension(ext);
            let mime = media_type_for_extension(ext);
            match route {
                PreviewRoute::Image => {
                    let m = mime.unwrap_or_else(|| panic!("{ext:?}: 画像なのに MIME が無い"));
                    assert!(m.starts_with("image/"), "{ext:?}: 画像なのに {m}");
                }
                PreviewRoute::Video => {
                    let m = mime.unwrap_or_else(|| panic!("{ext:?}: 動画なのに MIME が無い"));
                    assert!(m.starts_with("video/"), "{ext:?}: 動画なのに {m}");
                }
                PreviewRoute::Pdf => assert_eq!(mime, Some("application/pdf"), "{ext:?}"),
                PreviewRoute::Markdown => {
                    let m = mime.unwrap_or_else(|| panic!("{ext:?}: md なのに MIME が無い"));
                    assert!(m.starts_with("text/markdown"), "{ext:?}: md なのに {m}");
                }
                // 未知・コードは MIME を名乗らない（呼び出し側が octet-stream へ倒す）
                PreviewRoute::Code => assert_eq!(mime, None, "{ext:?}"),
            }
        }
    }

    /// #1676: 行指定は「原文の行」なので md はソース表示へ倒れ、
    /// 行を持たない種別はエラーになる
    #[test]
    fn 行指定つきの種別は原文が残るcodeへ倒れる() {
        assert_eq!(
            preview_route_with_line(PreviewRoute::Code).unwrap(),
            PreviewRoute::Code
        );
        assert_eq!(
            preview_route_with_line(PreviewRoute::Markdown).unwrap(),
            PreviewRoute::Code
        );
        for route in [PreviewRoute::Image, PreviewRoute::Pdf, PreviewRoute::Video] {
            let err = preview_route_with_line(route).unwrap_err();
            assert!(err.contains(route.as_str()), "{route:?}: {err}");
        }
    }

    /// #1676: 行数を超える要求は末尾行へ丸め、0 は 1 始まりの文言で弾く
    #[test]
    fn 行の丸めと境界() {
        assert_eq!(clamp_line(5000, 42).unwrap(), (42, false));
        assert_eq!(clamp_line(5000, 5000).unwrap(), (5000, false));
        assert_eq!(clamp_line(5000, 5001).unwrap(), (5000, true));
        assert_eq!(clamp_line(1, 9999).unwrap(), (1, true));
        // 空ファイル（行が 1 本も無い）でも行 1 は範囲内（キャレットの置き場）
        assert_eq!(clamp_line(0, 1).unwrap(), (1, false));
        assert_eq!(clamp_line(0, 7).unwrap(), (1, true));
        assert_eq!(clamp_line(5000, 0).unwrap_err(), LINE_ONE_BASED);
    }

    /// #1676: 桁は行末の次まで受ける（キャレットが行末に立てる）
    #[test]
    fn 桁の丸めと境界() {
        assert_eq!(clamp_column(10, 1).unwrap(), (1, false));
        assert_eq!(clamp_column(10, 11).unwrap(), (11, false));
        assert_eq!(clamp_column(10, 12).unwrap(), (11, true));
        assert_eq!(clamp_column(0, 1).unwrap(), (1, false));
        assert_eq!(clamp_column(10, 0).unwrap_err(), COLUMN_ONE_BASED);
    }

    #[test]
    fn mimeは大文字小文字と拡張子なしを吸収する() {
        assert_eq!(media_type(Path::new("/a/x.PNG")), Some("image/png"));
        assert_eq!(media_type(Path::new("/a/clip.MP4")), Some("video/mp4"));
        assert_eq!(
            media_type(Path::new("/a/clip.mov")),
            Some("video/quicktime")
        );
        assert_eq!(media_type(Path::new("/a/Makefile")), None);
        assert_eq!(media_type(Path::new("/a/main.rs")), None);
    }
}
