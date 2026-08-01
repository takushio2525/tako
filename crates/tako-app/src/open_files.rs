//! Finder の「このアプリケーションで開く」から渡されたファイルをプレビューで開く
//! （FR-3.22 / Issue #708）。
//!
//! ## 経路
//!
//! macOS は開く対象を `NSApplicationDelegate` の `application:openURLs:` で渡す
//! （10.13 以降はドキュメントを開く経路もここに集約され、`application:openFile:` は
//! 呼ばれない）。GPUI はこれを [`gpui::App::on_open_urls`] として公開しているが、
//! コールバックのシグネチャは `FnMut(Vec<String>)` で `App` を持たない。
//! そのため受け取った URL は channel でメインループへ渡し、そこで
//! [`crate::TakoApp::open_file_row`]（= dispatch `OpenFile` = CLI `tako open` /
//! MCP `tako_open_file` と同一経路）に載せる。**新しい操作系は作らない**ので、
//! 開発不変条件（UI でできることは AI からもできる）は既存ツールで満たされる。
//!
//! ## タイミング
//!
//! 未起動から開かれた場合、`application:openURLs:` は
//! `applicationDidFinishLaunching:`（= GPUI の `run` クロージャ）より**先に**
//! 届くことがある。`on_open_urls` を `run` の前に登録し、受け取りを
//! unbounded channel に積むことで、消費側（`run` の中で spawn）が動き出すまで
//! 取りこぼさない。復元は `TakoApp::new` の中で同期的に終わるため、消費側が
//! 動く時点でタブ・ペインは揃っている。

use std::path::PathBuf;

/// `application:openURLs:` から届いた URL 群を、tako が開けるローカルパスへ変換する。
/// `file://` 以外のスキームは黙って捨てる（tako は URL スキームを登録していないので
/// 通常は届かない。届いたとしても勝手に解釈しない）。
pub(crate) fn file_urls_to_paths(urls: &[String]) -> Vec<PathBuf> {
    urls.iter().filter_map(|u| file_url_to_path(u)).collect()
}

/// 単一の `file://` URL をパスへ。変換できないものは `None`。
pub(crate) fn file_url_to_path(url: &str) -> Option<PathBuf> {
    let rest = strip_file_scheme(url)?;
    let decoded = percent_decode(rest)?;
    // 絶対パスでなければ受け付けない（NSURL の absoluteString は必ず絶対パスになる。
    // そうでないものは想定外の入力なので、相対解決してカレントを触りに行かない）
    if !decoded.starts_with('/') {
        return None;
    }
    Some(PathBuf::from(decoded))
}

/// `file://` スキームを剥がす。`file://localhost/...` 形式にも対応する。
fn strip_file_scheme(url: &str) -> Option<&str> {
    // スキームは大文字小文字を区別しない（RFC 3986）
    let (scheme, rest) = url.split_once("://")?;
    if !scheme.eq_ignore_ascii_case("file") {
        return None;
    }
    // `file://<host>/path`。host が空 or localhost のときだけ受ける
    // （リモートホスト指定のファイル URL はローカルパスに落とせない）
    match rest.strip_prefix("localhost") {
        Some(after) if after.starts_with('/') => Some(after),
        Some(_) => None,
        None => Some(rest),
    }
}

/// パーセントデコード。`%XX` 以外の `%` はそのまま通す（NSURL は本物の `%` を
/// `%25` へエスケープするので通常は現れないが、落として別ファイルを開くより
/// 素通りさせて「開けない」で止まるほうが安全）。
fn percent_decode(s: &str) -> Option<String> {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hi = (bytes[i + 1] as char).to_digit(16);
            let lo = (bytes[i + 2] as char).to_digit(16);
            if let (Some(hi), Some(lo)) = (hi, lo) {
                out.push((hi * 16 + lo) as u8);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    // macOS のファイル名は UTF-8。非 UTF-8 は想定外なので開かない
    String::from_utf8(out).ok()
}

/// メインループ側の受け口。Finder から渡されたファイルをプレビューで開き、
/// tako を前面に出す。
///
/// - ウィンドウが 1 枚も無い（赤ボタン close 後にプロセスだけ生存）場合は
///   Dock 復帰と同じ [`crate::reopen_or_restore`] でウィンドウを開き直してから開く
/// - ディレクトリは v1 では対象外（`CFBundleDocumentTypes` にも宣言しない）。
///   「その他…」で強制指定された場合は警告だけ出して無視する
pub(crate) fn open_paths(paths: Vec<PathBuf>, cx: &mut gpui::App) {
    if paths.is_empty() {
        return;
    }
    if cx.windows().is_empty() {
        crate::reopen_or_restore(cx);
    }
    cx.activate(true);

    let files: Vec<PathBuf> = paths
        .into_iter()
        .filter(|p| {
            if p.is_dir() {
                eprintln!(
                    "warning: フォルダは開けない（プレビューはファイル専用）: {}",
                    p.display()
                );
                return false;
            }
            if !p.exists() {
                eprintln!("warning: ファイルが見つからない: {}", p.display());
                return false;
            }
            true
        })
        .collect();
    if files.is_empty() {
        return;
    }

    let open = move |app: &mut crate::TakoApp, cx: &mut gpui::Context<crate::TakoApp>| {
        for path in &files {
            // ファイルツリーのクリックと同一経路（dispatch OpenFile）。
            // 複数選択で開かれた場合は同じプレビューペインを順に差し替える
            // （タブ内 1 枚再利用は FR-3.2 の既定挙動）
            app.open_file_row(path, cx);
        }
    };

    // プライマリ（#381）優先。多重起動のセカンダリでは PrimaryApp global を
    // 立てないため、ウィンドウのルートビューへフォールバックする
    if cx
        .try_global::<crate::PrimaryApp>()
        .and_then(|g| g.0.upgrade())
        .is_some()
    {
        crate::with_primary_app(cx, open);
    } else if let Some(handle) = cx
        .windows()
        .into_iter()
        .find_map(|w| w.downcast::<crate::TakoApp>())
    {
        if let Err(e) = handle.update(cx, |app, _window, cx| open(app, cx)) {
            eprintln!("warning: 開いたファイルを表示できない: {e}");
        }
    } else {
        eprintln!("warning: 表示先のウィンドウが無いためファイルを開けない");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_url_の基本形をパスへ変換する() {
        assert_eq!(
            file_url_to_path("file:///Users/me/notes.md"),
            Some(PathBuf::from("/Users/me/notes.md"))
        );
    }

    #[test]
    fn パーセントエンコードを復元する() {
        assert_eq!(
            file_url_to_path("file:///Users/me/my%20notes.md"),
            Some(PathBuf::from("/Users/me/my notes.md"))
        );
        // 日本語ファイル名（UTF-8 の %XX 列）
        assert_eq!(
            file_url_to_path("file:///tmp/%E6%97%A5%E6%9C%AC%E8%AA%9E.md"),
            Some(PathBuf::from("/tmp/日本語.md"))
        );
        // `#` `%` を含む名前は NSURL がエスケープして渡してくる
        assert_eq!(
            file_url_to_path("file:///tmp/a%23b%25c.md"),
            Some(PathBuf::from("/tmp/a#b%c.md"))
        );
    }

    #[test]
    fn localhost_付きの形式も受ける() {
        assert_eq!(
            file_url_to_path("file://localhost/tmp/a.md"),
            Some(PathBuf::from("/tmp/a.md"))
        );
    }

    #[test]
    fn スキームは大文字小文字を区別しない() {
        assert_eq!(
            file_url_to_path("FILE:///tmp/a.md"),
            Some(PathBuf::from("/tmp/a.md"))
        );
    }

    #[test]
    fn file_以外のスキームとリモートホストは捨てる() {
        assert_eq!(file_url_to_path("https://example.com/a.md"), None);
        assert_eq!(file_url_to_path("tako://open?path=/tmp/a.md"), None);
        assert_eq!(file_url_to_path("file://example.com/tmp/a.md"), None);
        assert_eq!(file_url_to_path("/tmp/a.md"), None);
        assert_eq!(file_url_to_path(""), None);
    }

    #[test]
    fn 不正なパーセント列は素通りさせる() {
        // `%zz` は 16 進ではない。落として別ファイルを開くより、そのまま渡して
        // 「開けない」で止めるほうが安全
        assert_eq!(
            file_url_to_path("file:///tmp/a%zz.md"),
            Some(PathBuf::from("/tmp/a%zz.md"))
        );
        // 末尾の切れた `%` も同様
        assert_eq!(
            file_url_to_path("file:///tmp/a%2"),
            Some(PathBuf::from("/tmp/a%2"))
        );
    }

    #[test]
    fn ディレクトリの末尾スラッシュは保たれる() {
        // NSURL はディレクトリに末尾 `/` を付ける。open_paths 側の is_dir 判定に回す
        assert_eq!(
            file_url_to_path("file:///Users/me/proj/"),
            Some(PathBuf::from("/Users/me/proj/"))
        );
    }

    #[test]
    fn 複数_url_をまとめて変換し変換不能なものだけ落とす() {
        let urls = vec![
            "file:///tmp/a.md".to_string(),
            "https://example.com".to_string(),
            "file:///tmp/b%20c.rs".to_string(),
        ];
        assert_eq!(
            file_urls_to_paths(&urls),
            vec![PathBuf::from("/tmp/a.md"), PathBuf::from("/tmp/b c.rs")]
        );
    }

    /// 番犬テスト（受け入れ条件 4）: `build-app.sh` が生成する Info.plist の
    /// `CFBundleDocumentTypes` は**すべて `LSHandlerRank = Alternate`** でなければ
    /// ならない。`Default` / `Owner` を書くと既定アプリを奪う（Issue #708 の絶対条件）
    #[test]
    fn info_plist_の_handler_rank_は_すべて_alternate() {
        let script = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../scripts/build-app.sh")
            .canonicalize()
            .expect("build-app.sh が見つからない");
        let text = std::fs::read_to_string(&script).expect("build-app.sh を読めない");
        assert!(
            text.contains("<key>CFBundleDocumentTypes</key>"),
            "Info.plist に CFBundleDocumentTypes が無い（Finder の「このアプリケーションで開く」に出ない）"
        );
        let ranks: Vec<&str> = text
            .match_indices("<key>LSHandlerRank</key>")
            .map(|(i, _)| {
                let after = &text[i..];
                let start = after.find("<string>").expect("LSHandlerRank の値が無い") + 8;
                let end = after[start..].find("</string>").expect("閉じタグが無い") + start;
                &after[start..end]
            })
            .collect();
        assert!(
            !ranks.is_empty(),
            "LSHandlerRank の宣言が無い（既定は Default 相当になり既定アプリを奪い得る）"
        );
        for rank in &ranks {
            assert_eq!(
                *rank, "Alternate",
                "LSHandlerRank は Alternate 固定（既定アプリを奪わない。#708）"
            );
        }
    }
}
