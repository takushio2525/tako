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
}
