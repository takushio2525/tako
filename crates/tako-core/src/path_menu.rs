//! パスに対するコンテキストメニューの組み立て（Issue #1182）
//!
//! ターミナル内のパスリンク（`links.rs` が検出したもの）を cmd+右クリックしたときに
//! 出す項目の**並びと出し分け**をここで決める。GPUI 非依存の純粋関数なので、
//! GUI を立てずに「何が出るか」を検査できる。
//!
//! ## なぜここに置くか
//!
//! ファイルツリーの右クリック（FR-3.12 / #314）と**同じ操作**を同じ順で出すのが要件
//! （二重実装を作らない）。ツリー側は `tako-app` の render に項目が直書きされているので、
//! **共有できるのは「どの操作をどの順で出すか」の規則**と、その操作を実行する
//! dispatch（`FileOpKind`）の 2 つになる。前者をここへ、後者は既存の
//! `tako-control::dispatch` の `FileOp` へ寄せた。
//!
//! 文言（日英）は画面に描く側の関心なので `tako-app::ui_text` が持つ
//! （ツリーと**同じ関数**を引くので表記がずれない）。

/// パスメニューの 1 項目。`Separator` は区切り線
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PathMenuItem {
    Action(PathMenuAction),
    Separator,
}

/// パスメニューから実行できる操作。
///
/// **どれも既存の `FileOpKind` と 1:1**（= CLI `tako file …` / MCP `tako_file_op` から
/// 同じことができる）。新しい操作を足すときは対応する `FileOpKind` も足すこと
/// （UI にだけある操作を作らない = 設計原則 5）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PathMenuAction {
    /// tako の中で開く（ファイル = プレビューペイン / ディレクトリ = ターミナルを分割）。
    /// **現行の cmd+クリックと同じ動作**（#147 / #153）
    OpenInTako,
    /// OS の既定アプリで開く
    OpenDefault,
    /// OS のアプリ選択 UI を出して開く
    OpenWith,
    /// ファイルマネージャ（Finder / エクスプローラー）で選択表示する
    Reveal,
    /// 相対パスをクリップボードへ
    CopyRelativePath,
    /// 絶対パスをクリップボードへ
    CopyAbsolutePath,
}

impl PathMenuAction {
    /// メニュー項目の id（UI のクリックハンドラ・CLI の `--action` で使う安定キー）
    pub fn id(self) -> &'static str {
        match self {
            Self::OpenInTako => "open-in-tako",
            Self::OpenDefault => "open-default",
            Self::OpenWith => "open-with",
            Self::Reveal => "reveal",
            Self::CopyRelativePath => "copy-rel",
            Self::CopyAbsolutePath => "copy-abs",
        }
    }

    /// id から復元する（未知の id は `None`）
    pub fn parse(id: &str) -> Option<Self> {
        Self::ALL.iter().copied().find(|a| a.id() == id)
    }

    /// 全操作（パリティ検査用）
    pub const ALL: [Self; 6] = [
        Self::OpenInTako,
        Self::OpenDefault,
        Self::OpenWith,
        Self::Reveal,
        Self::CopyRelativePath,
        Self::CopyAbsolutePath,
    ];
}

/// パスメニューの項目列。`is_dir` で出し分ける。
///
/// - 先頭は `OpenInTako`（= cmd+クリックと同じ動作）。**押し間違えても従来と同じ結果**に
///   なる位置に置く
/// - `OpenDefault` / `OpenWith` は**ファイルのみ**。ディレクトリを既定アプリで開くのは
///   ファイルマネージャで開くのと同義なので、`Reveal` と重複する項目を並べない
///   （ファイルツリー側（#314）の出し分けと同じ規則）
/// - コピー系は末尾（破壊的でない操作を下に置く VSCode 流）
pub fn items(is_dir: bool) -> Vec<PathMenuItem> {
    let mut out = vec![
        PathMenuItem::Action(PathMenuAction::OpenInTako),
        PathMenuItem::Separator,
    ];
    if !is_dir {
        out.push(PathMenuItem::Action(PathMenuAction::OpenDefault));
        out.push(PathMenuItem::Action(PathMenuAction::OpenWith));
    }
    out.push(PathMenuItem::Action(PathMenuAction::Reveal));
    out.push(PathMenuItem::Separator);
    out.push(PathMenuItem::Action(PathMenuAction::CopyRelativePath));
    out.push(PathMenuItem::Action(PathMenuAction::CopyAbsolutePath));
    out
}

/// 実行できる操作だけを並べたもの（CLI / MCP の一覧用）
pub fn actions(is_dir: bool) -> Vec<PathMenuAction> {
    items(is_dir)
        .into_iter()
        .filter_map(|i| match i {
            PathMenuItem::Action(a) => Some(a),
            PathMenuItem::Separator => None,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ファイルとディレクトリで出し分ける() {
        let file = actions(false);
        let dir = actions(true);
        // 要件（#1182）: どちらにも「tako で開く」「Finder で表示」「パスをコピー」が出る
        for a in [
            PathMenuAction::OpenInTako,
            PathMenuAction::Reveal,
            PathMenuAction::CopyAbsolutePath,
            PathMenuAction::CopyRelativePath,
        ] {
            assert!(file.contains(&a), "ファイルに {a:?} が無い");
            assert!(dir.contains(&a), "ディレクトリに {a:?} が無い");
        }
        // 既定アプリ系はファイルのみ（ツリー側 #314 と同じ規則）
        assert!(file.contains(&PathMenuAction::OpenDefault));
        assert!(file.contains(&PathMenuAction::OpenWith));
        assert!(!dir.contains(&PathMenuAction::OpenDefault));
        assert!(!dir.contains(&PathMenuAction::OpenWith));
    }

    #[test]
    fn 先頭はcmdクリックと同じ動作() {
        for is_dir in [false, true] {
            assert_eq!(
                items(is_dir).first(),
                Some(&PathMenuItem::Action(PathMenuAction::OpenInTako)),
                "is_dir={is_dir}"
            );
        }
    }

    #[test]
    fn 区切り線が端に来ない_連続しない() {
        for is_dir in [false, true] {
            let it = items(is_dir);
            assert_ne!(it.first(), Some(&PathMenuItem::Separator));
            assert_ne!(it.last(), Some(&PathMenuItem::Separator));
            for w in it.windows(2) {
                assert!(
                    !(w[0] == PathMenuItem::Separator && w[1] == PathMenuItem::Separator),
                    "区切り線が連続する: is_dir={is_dir}"
                );
            }
        }
    }

    #[test]
    fn idが往復する_重複しない() {
        let mut ids: Vec<&str> = PathMenuAction::ALL.iter().map(|a| a.id()).collect();
        ids.sort_unstable();
        let before = ids.len();
        ids.dedup();
        assert_eq!(before, ids.len(), "id が重複している: {ids:?}");
        for a in PathMenuAction::ALL {
            assert_eq!(PathMenuAction::parse(a.id()), Some(a));
        }
        assert_eq!(PathMenuAction::parse("open-in-vscode"), None);
    }

    #[test]
    fn 出る項目はすべてallに載っている() {
        for is_dir in [false, true] {
            for a in actions(is_dir) {
                assert!(PathMenuAction::ALL.contains(&a), "{a:?} が ALL に無い");
            }
        }
    }

    // --- リンク検出との突き合わせ（#1182 受け入れ条件 3） ---
    //
    // 「cmd+クリックが開けるものは cmd+右クリックでメニューが出る」を、UI を立てずに
    // **同じ材料**（`links::detect_links_with_cwd` → `links::link_at`）で確かめる。
    // メニューの対象は検出結果の `target`（= 解決済みの絶対パス）1 つなので、
    // 相対 / `~` / 行番号つき / 絶対のどれを押しても同じパスに落ちることまで見る

    fn screen_of(line: &str) -> crate::screen::Screen {
        crate::screen::Screen {
            cols: 400,
            rows: 1,
            lines: vec![crate::screen::ScreenLine {
                text: line.to_string(),
                runs: Vec::new(),
                cell_cols: line.chars().enumerate().map(|(i, _)| i).collect(),
                has_wide: false,
            }],
            cursor: None,
            ime_cursor: None,
            display_offset: 0,
            fract: 0.0,
            extra_bottom: None,
        }
    }

    /// 画面の `line` のうち `needle` が始まる列でリンクを引く（クリック位置の代わり）。
    /// 返すのは「そのクリックでメニューが対象にするパス」と「そこに出る項目」
    fn menu_at(
        line: &str,
        needle: &str,
        cwd: Option<&std::path::Path>,
    ) -> Option<(String, Vec<PathMenuAction>)> {
        let byte = line.find(needle)?;
        let col = line[..byte].chars().count();
        let links = crate::links::detect_links_with_cwd(&screen_of(line), cwd);
        let link = crate::links::link_at(&links, 0, col)?;
        if link.kind != crate::links::LinkKind::Path {
            return None;
        }
        let is_dir = std::path::Path::new(&link.target).is_dir();
        Some((link.target.clone(), actions(is_dir)))
    }

    /// **画面に出す形は `/` 区切りで書く**（`is_path_like` は `/` を含まない Windows の
    /// バックスラッシュ形式を候補にしない = #153 の既存制約）。ここで見たいのは
    /// 「cmd+クリックが解決できる形すべてにメニューが出るか」なので、両 OS で成立する
    /// 相対形で書き、絶対パスは unix 限定の別テストに分ける
    #[test]
    fn 相対_行番号つき_ディレクトリ_空白入りのどれでもメニューが出る() {
        let dir = std::env::temp_dir().join(format!("tako_path_menu_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("src")).unwrap();
        // 空白 + 日本語を含む名前（#1182 のエッジケース）。画面には引用符つきで出る
        std::fs::write(dir.join("src/読み 込み.txt"), "x").unwrap();
        std::fs::write(dir.join("src/plain.txt"), "y").unwrap();

        // ① 相対パス（cwd 基準）
        let (t1, a1) =
            menu_at("edit src/plain.txt here", "src/plain.txt", Some(&dir)).expect("相対が出ない");
        assert_eq!(
            std::path::Path::new(&t1),
            dir.join("src/plain.txt").as_path()
        );
        assert!(a1.contains(&PathMenuAction::OpenInTako) && a1.contains(&PathMenuAction::Reveal));
        assert!(a1.contains(&PathMenuAction::OpenDefault), "{a1:?}");

        // ② 行番号つき（`:42:5` を剥がして ① と同じパスへ落ちる）
        let (t2, _) = menu_at(
            "error at src/plain.txt:42:5 bad",
            "src/plain.txt",
            Some(&dir),
        )
        .expect("行番号つきが出ない");
        assert_eq!(
            std::path::Path::new(&t2),
            dir.join("src/plain.txt").as_path(),
            "行番号を剥がしたパスにならない"
        );

        // ③ 空白入り + 日本語（画面では引用符で囲まれている）
        let (t3, _) = menu_at(
            "open \"src/読み 込み.txt\" now",
            "src/読み 込み.txt",
            Some(&dir),
        )
        .expect("空白入りが出ない");
        assert_eq!(
            std::path::Path::new(&t3),
            dir.join("src/読み 込み.txt").as_path()
        );

        // ④ ディレクトリは既定アプリ系を出さない（`actions(true)` と同じ）
        let (t4, a4) = menu_at("cd ./src", "./src", Some(&dir)).expect("ディレクトリが出ない");
        assert_eq!(std::path::Path::new(&t4), dir.join("src").as_path());
        assert!(a4.contains(&PathMenuAction::OpenInTako), "{a4:?}");
        assert!(a4.contains(&PathMenuAction::Reveal), "{a4:?}");
        assert!(!a4.contains(&PathMenuAction::OpenDefault), "{a4:?}");

        // ⑤ 存在しないパスはリンクにならない = メニューも出ない
        assert!(
            menu_at("cat src/missing.txt now", "src/missing.txt", Some(&dir)).is_none(),
            "存在しないパスにメニューが出る"
        );
        // ⑥ URL はパスメニューの対象にしない（既存の URL リンクの挙動を奪わない）
        assert!(
            menu_at(
                "see https://example.com/a/b now",
                "https://example.com/a/b",
                Some(&dir)
            )
            .is_none(),
            "URL がパスメニューの対象になっている"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 絶対パス（`/` 始まり）。Windows のバックスラッシュ形式は検出対象外なので unix 限定
    #[cfg(unix)]
    #[test]
    fn 絶対パスでもメニューが出る() {
        let dir = std::env::temp_dir().join(format!("tako_path_menu_abs_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let abs = dir.join("読み 込み.txt");
        std::fs::write(&abs, "x").unwrap();
        let line = format!("open \"{}\"", abs.display());
        let (target, actions) =
            menu_at(&line, &abs.display().to_string(), None).expect("絶対パスが出ない");
        assert_eq!(std::path::Path::new(&target), abs.as_path());
        assert!(actions.contains(&PathMenuAction::OpenInTako));
        assert!(actions.contains(&PathMenuAction::OpenDefault));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// `~` 起点も同じ 1 実装で解決される（HOME を解決できない環境では skip）
    #[test]
    fn ホーム起点でもメニューが出る() {
        let Some(home) = crate::paths::home_dir() else {
            eprintln!("skip: この環境はホームを解決できない");
            return;
        };
        let name = format!(".tako_path_menu_{}", std::process::id());
        let file = home.join(&name);
        if std::fs::write(&file, "").is_err() {
            eprintln!("skip: ホームへ書けない環境");
            return;
        }
        // 画面に出す文字列（シェルが `~/…` と印字した形の再現）を組む。
        // **`paths::shorten_home` は通さない**: あれは「実パス → 表示用」の変換で、
        // ここは逆向き（表示形をリンク検出に食わせる）= #893 の番犬の対象外
        let mut needle = String::from("~/");
        needle.push_str(&name);
        let line = format!("cat {needle}");
        let (target, actions) = menu_at(&line, &needle, Some(std::path::Path::new("/")))
            .expect("~ 起点のリンクが出ない");
        assert_eq!(std::path::Path::new(&target), file.as_path());
        assert!(actions.contains(&PathMenuAction::OpenInTako));
        let _ = std::fs::remove_file(&file);
    }
}
