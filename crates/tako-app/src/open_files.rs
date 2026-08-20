//! Finder の「このアプリケーションで開く」から渡されたファイルを開く
//! （FR-3.22 / Issue #708・#835）。
//!
//! ## 経路
//!
//! macOS は開く対象を `NSApplicationDelegate` の `application:openURLs:` で渡す
//! （10.13 以降はドキュメントを開く経路もここに集約され、`application:openFile:` は
//! 呼ばれない）。GPUI はこれを [`gpui::App::on_open_urls`] として公開しているが、
//! コールバックのシグネチャは `FnMut(Vec<String>)` で `App` を持たない。
//! そのため受け取った URL は channel でメインループへ渡し、そこで
//! dispatch（= CLI `tako open --new-tab` / MCP `tako_open_file` の `new_tab`、
//! フォルダは `tako tab new --cwd`）に載せる。**新しい操作系は作らない**ので、
//! 開発不変条件（UI でできることは AI からもできる）は既存ツールで満たされる。
//!
//! ## 何がどこに開くか（#835）
//!
//! | 渡されたもの | 動作 |
//! |---|---|
//! | プレビューできるファイル | **新しいタブ 1 枚**をそのファイル専用のプレビューにする |
//! | 宣言外・未知の形式 | 同上（`OpenFile` がコードプレビューへ落とす。巨大ファイルは切り詰め） |
//! | フォルダ | **新しいタブ**でそのフォルダにシェルを起動する（ターミナルとして自然な既定） |
//! | 存在しないパス | 警告して読み飛ばす（他のファイルの処理は続ける） |
//!
//! 複数ファイルを一度に渡されたら **1 ファイル = 1 タブ**。同じプレビューペインを
//! 順に差し替えると最後の 1 枚しか残らず、選んだファイルの大半が「開かなかった」
//! ように見えるため（#708 の挙動を #835 で是正）。
//!
//! ## タイミング
//!
//! 未起動から開かれた場合、`application:openURLs:` は
//! `applicationDidFinishLaunching:`（= GPUI の `run` クロージャ）より**先に**
//! 届くことがある。`on_open_urls` を `run` の前に登録し、受け取りを
//! unbounded channel に積むことで、消費側（`run` の中で spawn）が動き出すまで
//! 取りこぼさない。復元は `TakoApp::new` の中で同期的に終わるため、消費側が
//! 動く時点でタブ・ペインは揃っている（= 復元と新規タブが混ざらない）。

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

/// 渡されたパスをどう開くか（#835）。分類は純粋関数（[`plan_open`]）にしてあるので、
/// GUI なしで規則そのものを検査できる
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum OpenTarget {
    /// 新しいタブ 1 枚をこのファイル専用のプレビューにする
    PreviewInNewTab(PathBuf),
    /// 新しいタブでこのフォルダにシェルを起動する
    ShellInNewTab(PathBuf),
}

/// 渡されたパス群を開き方へ振り分ける。存在しないものは落とす（他を巻き添えにしない）。
/// 順序は入力どおり = Finder で選んだ順にタブが並ぶ
pub(crate) fn plan_open(paths: &[PathBuf]) -> Vec<OpenTarget> {
    paths
        .iter()
        .filter_map(|p| {
            if p.is_dir() {
                // ターミナルアプリにフォルダを渡す = 「そこで作業を始めたい」と読む。
                // プレビューはファイル専用なので、フォルダはシェルで受ける
                Some(OpenTarget::ShellInNewTab(p.clone()))
            } else if p.is_file() {
                // 宣言外の形式もここへ来る。表示モードの決定と巨大ファイルの
                // 切り詰めは dispatch / プレビュー側が持っているので判定しない
                Some(OpenTarget::PreviewInNewTab(p.clone()))
            } else {
                eprintln!("warning: 開けるものが見つからない: {}", p.display());
                None
            }
        })
        .collect()
}

/// メインループ側の受け口。Finder から渡されたものを新しいタブで開き、tako を前面に出す。
///
/// ウィンドウが 1 枚も無い（赤ボタン close 後にプロセスだけ生存）場合は
/// Dock 復帰と同じ [`crate::reopen_or_restore`] でウィンドウを開き直してから開く。
pub(crate) fn open_paths(paths: Vec<PathBuf>, cx: &mut gpui::App) {
    let targets = plan_open(&paths);
    if targets.is_empty() {
        return;
    }
    if cx.windows().is_empty() {
        crate::reopen_or_restore(cx);
    }
    cx.activate(true);

    let open = move |app: &mut crate::TakoApp, cx: &mut gpui::Context<crate::TakoApp>| {
        for target in &targets {
            app.open_from_finder(target, cx);
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

    /// #835: 何を渡されたら何が開くかの規則。GUI なしで規則そのものを固定する
    #[test]
    fn 渡されたものの種類で開き方が決まる() {
        let dir = std::env::temp_dir().join(format!("tako-openfiles-plan-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        std::fs::write(dir.join("a.md"), "# x").unwrap();
        // 宣言外の拡張子（UTI を持たない = Finder の候補には出ないが「その他…」で来る）
        std::fs::write(dir.join("b.bin"), [0u8, 1, 2]).unwrap();

        let plan = plan_open(&[
            dir.join("a.md"),
            dir.join("sub"),
            dir.join("b.bin"),
            dir.join("no-such"),
        ]);
        assert_eq!(
            plan,
            vec![
                // ファイルは 1 枚 = 1 タブ。宣言外の形式も同じ扱い（表示モードの
                // 決定と巨大ファイルの切り詰めはプレビュー側が持つ）
                OpenTarget::PreviewInNewTab(dir.join("a.md")),
                // フォルダはシェル
                OpenTarget::ShellInNewTab(dir.join("sub")),
                OpenTarget::PreviewInNewTab(dir.join("b.bin")),
                // 存在しないものは落とすが、他は巻き添えにしない
            ],
            "選んだ順にタブが並び、開けないものだけが落ちる"
        );
        assert!(plan_open(&[]).is_empty());
        assert!(plan_open(&[dir.join("no-such")]).is_empty());
        let _ = std::fs::remove_dir_all(&dir);
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

    /// 番犬テスト（Issue #837）: ビルド出力の tako.app を**消してから Launch Services の
    /// 登録も外す**後始末が、install（build-app.sh）とリリース（release.sh）の両方に
    /// 入っていること。
    ///
    /// 同じ identity の .app がディスク上に 2 つあると LS は両方を登録し、Finder の
    /// 「このアプリケーションで開く」に tako が 2 つ並ぶ。macOS 26 実測では
    /// `lsregister -u` だけだと**ファイルを触らなくても約 40 秒後に自動で再登録**され、
    /// 逆に実体を消しただけでは登録が残骸として残る。**両方**やって初めて恒久的に消える。
    /// 置き場所を変える回避（`*.noindex` / `.metadata_never_index` / 隠しディレクトリ）は
    /// どれも効かなかったので、これが唯一の恒久対策。片方だけに退行しないよう固定する。
    #[test]
    fn ビルド出力の_app_は_install_とリリースの後に片付けられる() {
        let scripts = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../scripts")
            .canonicalize()
            .expect("scripts/ が見つからない");
        let read = |name: &str| {
            std::fs::read_to_string(scripts.join(name))
                .unwrap_or_else(|e| panic!("{name} を読めない: {e}"))
        };
        let lib = read("lib/launch-services.sh");
        let build_app = read("build-app.sh");
        let release = read("release.sh");

        // 後始末の実装は共有ライブラリ 1 本（呼び出し側で lsregister を直叩きしない）
        for name in [
            "ls_registered_tako_paths()",
            "ls_unregister()",
            "ls_sweep_stale_registrations()",
            "ls_drop_build_output()",
        ] {
            assert!(
                lib.contains(name),
                "lib/launch-services.sh に {name} が無い（#837）"
            );
        }
        const LSREGISTER_PATH: &str = "LaunchServices.framework/Support/lsregister";
        for (name, text) in [("build-app.sh", &build_app), ("release.sh", &release)] {
            assert!(
                !text.contains(LSREGISTER_PATH),
                "{name} が lsregister を直叩きしている。LS 操作は \
                 lib/launch-services.sh に集約すること（#837）"
            );
            assert!(
                text.contains("ls_drop_build_output"),
                "{name} がビルド出力の後始末（ls_drop_build_output）を呼んでいない。\
                 残すと LS が拾って Finder の候補が二重化する（#837）"
            );
        }

        // 実体を消す → 登録を外す、の順序。逆だと消す前の -u が約 40 秒後に取り消される
        let drop_fn = lib
            .split_once("ls_drop_build_output() {")
            .expect("ls_drop_build_output の定義が見つからない")
            .1;
        let rm_at = drop_fn
            .find("rm -rf \"$app\"")
            .expect("ls_drop_build_output がビルド出力を消していない（#837）");
        let sweep_at = drop_fn
            .find("ls_sweep_stale_registrations")
            .expect("ls_drop_build_output が登録の掃除をしていない（#837）");
        assert!(
            rm_at < sweep_at,
            "実体の削除は登録解除より前に行うこと（順序が逆だと再登録される。#837）"
        );

        // 掃除は「実体が無いものだけ」を対象にする（-u が恒久的に効くのはそれだけ）
        let sweep_fn = lib
            .split_once("ls_sweep_stale_registrations() {")
            .expect("ls_sweep_stale_registrations の定義が見つからない")
            .1;
        assert!(
            sweep_fn.contains("ls_unregister"),
            "掃除が ls_unregister を呼んでいない（#837）"
        );
    }

    /// `scripts/test-launch-services.sh`（偽 lsregister を使う密閉モックテスト）を
    /// CI で回す（Issue #837）。本番の Launch Services データベースには触らない。
    ///
    /// シェル側でしか検証できないことを見ている: ビルド出力を消すこと / 登録解除は
    /// 実体の無いパスだけに限ること / 警告が実際に出力されること（`$var（` のように
    /// 全角が続く箇所は bash が UTF-8 のバイトを変数名へ取り込むので `${}` で括らないと
    /// `set -u` で落ちる。実際に踏んだ）。
    #[cfg(target_os = "macos")]
    #[test]
    fn launch_services_ヘルパのモックテストが通る() {
        let script = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../scripts/test-launch-services.sh")
            .canonicalize()
            .expect("test-launch-services.sh が見つからない");
        let out = std::process::Command::new("bash")
            .arg(&script)
            .output()
            .expect("bash を起動できない");
        assert!(
            out.status.success(),
            "test-launch-services.sh が失敗した:\n{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
    }
}
