//! master のいるタブの名前（Issue #2120）
//!
//! タブ名は「そのタブで何をしているか」の目印なので、master のタブは**プロジェクト名か
//! 作業の主題**で呼ぶ（`tako` / `my-site` / 1 つのリポジトリで複数の科目を扱うなら
//! `アジャイル` / `数理モデリング`）。役割名・プロファイル名（`master` / `master-takodev`）は
//! 使わない — それはペインの role ラベルとして別の場所に出ている。
//!
//! # なぜ要るのか
//!
//! #2120 以前は master を立てる 3 経路（`tako master` / スマホの「+ master」/ 人タスクの
//! 返答で立てる master）が、タブ名を `master` / `master-<名前>` の**手動名**で付けていた。
//! 手動名は自動命名より強い（FR-2.12.3）ので、master 自身が
//! `tako_rename_tab(source: "auto")` で主題を付けようとしても全部弾かれ、
//! 本番では master のいるタブ 13 枚がすべて `master` で固定されていた。
//!
//! 今は起動・採用の時点で**管轄プロジェクトの名前を自動名として**付け（[`initial_title`]）、
//! 以後は master が主題で付け直す。ユーザーが手で付けた名前は自動名では上書きされない。
//!
//! # solo のタブも同じ形（Issue #2127）
//!
//! `tako solo` も起動でタブ名を `solo` / `solo-<名前>` の**手動名**で付けていたので、
//! solo の system prompt にある `tako_rename_tab(source: "auto")` が全部弾かれていた。
//! solo には管轄プロジェクトが無い（必要なプロジェクトへ自分で `cd` する）ので、
//! 初期名は **solo が作業するフォルダのリポジトリ名**（[`solo_initial_title`]）を自動名で付ける。
//!
//! # ここが持つもの（判定と組み立てだけ。ファイル I/O は solo の初期名の `.git` の有無だけ）
//!
//! - [`initial_title`] — master の起動・採用時の初期名（管轄プロジェクトのキー）
//! - [`solo_initial_title`] — solo の起動・復元時の初期名（作業フォルダのリポジトリ名）
//! - [`is_legacy_role_title`] / [`is_legacy_solo_title`] — 旧版の tako が付けた役割名か
//!   （復元時に手動名を解除する対象）
//! - [`app_may_name_tab`] — アプリの自動命名（FR-2.12）がそのタブの名前を作ってよいか
//! - [`legacy`] — A/B（`TAKO_2120_LEGACY=1` で #2120 / #2127 前の挙動へ戻す）

use std::path::Path;

use crate::project_root::{self, SearchBounds};

/// #2120 / #2127 の A/B。`TAKO_2120_LEGACY=1` で、起動経路が `master` / `master-<名前>` /
/// `solo` / `solo-<名前>` を手動名で付け・採用がタブ名に触れず・復元が旧い手動名を解除せず・
/// アプリの自動命名が master / solo のタブも命名する旧挙動へ戻る
pub fn legacy() -> bool {
    std::env::var_os("TAKO_2120_LEGACY").is_some_and(|v| !v.is_empty() && v != "0")
}

/// プロジェクトキーの区切りとみなす文字（`shop-api` / `my_app` / `example.com`）
fn is_separator(c: char) -> bool {
    matches!(c, '-' | '_' | '.' | ' ')
}

/// 管轄プロジェクトのキーから、master のタブの初期名を作る。
///
/// - 管轄なし（汎用 master）→ `None`。**タブ名に触らない**（汎用の master は採用
///   = `tako orchestrator adopt <プロジェクト>` した時点でそのプロジェクト名になる）
/// - 1 件 → そのキー（`takodev` プロファイルの管轄が `[tako]` なら `tako`）
/// - 複数 → 区切りの位置で揃う共通の頭（`shop-api` / `shop-web` → `shop`）。
///   揃わなければ ` / ` でつなぐ（どれか 1 つを選ぶと、別のプロジェクトの作業が
///   違う名前のタブで動いて見える）
pub fn initial_title(projects: &[String]) -> Option<String> {
    let keys: Vec<&str> = projects
        .iter()
        .map(|k| k.trim())
        .filter(|k| !k.is_empty())
        .collect();
    let (first, rest) = keys.split_first()?;
    if rest.is_empty() {
        return Some((*first).to_string());
    }
    if let Some(head) = common_head(&keys) {
        return Some(head);
    }
    Some(keys.join(" / "))
}

/// 全キーが区切りの位置で揃う、いちばん長い共通の頭（区切り文字は落とす）
fn common_head(keys: &[&str]) -> Option<String> {
    let first: Vec<char> = keys.first()?.chars().collect();
    let others: Vec<Vec<char>> = keys[1..].iter().map(|k| k.chars().collect()).collect();
    // 文字単位の共通部分の長さ
    let mut len = first.len();
    for other in &others {
        len = len.min(
            first
                .iter()
                .zip(other.iter())
                .take_while(|(a, b)| a == b)
                .count(),
        );
    }
    // 位置 i が「全キーで区切りの位置」= どのキーもそこで終わるか、次が区切り文字
    let at_boundary = |i: usize| {
        std::iter::once(&first)
            .chain(others.iter())
            .all(|k| k.get(i).is_none_or(|c| is_separator(*c)))
    };
    let cut = (1..=len).rev().find(|&i| at_boundary(i))?;
    let head: String = first[..cut].iter().collect();
    let head = head.trim_end_matches(is_separator).to_string();
    (!head.is_empty()).then_some(head)
}

/// 旧版の tako が master の起動で付けたタブ名か（`master` / `master-<名前>`）。
///
/// 復元時、**master のペインがいるタブ**でこの名前が手動名になっていたら、それは
/// #2120 以前の起動経路が付けたものとみなして手動を解除する。ユーザーが付けた
/// 別の名前（`tako` / `アジャイル` など）はここに当たらないので残る
pub fn is_legacy_role_title(title: &str) -> bool {
    title == "master"
        || title
            .strip_prefix("master-")
            .is_some_and(|rest| !rest.trim().is_empty())
}

/// solo のペインの role か（#2127）。
///
/// solo の role には語彙が 2 つある: `tako solo` が貼る `solo` / `solo:<名前>` と、
/// 再起動で tmux の `TAKO_ORCHESTRATOR_ROLE` から戻した `orchestrator-solo[:<名前>]`。
/// どちらも solo として扱う（片方だけ見ると、再起動を挟んだ solo のタブを取りこぼす）
pub fn is_solo_role(role: &str) -> bool {
    solo_suffix_of_role(role).is_some()
}

/// solo の role から起動時の名前（`tako solo -<名前>` / 旧形式のサフィックス）を取り出す。
/// `Some(None)` = 名前なし（`tako solo`）、`None` = solo の role ではない
fn solo_suffix_of_role(role: &str) -> Option<Option<&str>> {
    let rest = role
        .strip_prefix("orchestrator-solo")
        .or_else(|| role.strip_prefix("solo"))?;
    if rest.is_empty() {
        return Some(None);
    }
    rest.strip_prefix(':').filter(|s| !s.is_empty()).map(Some)
}

/// 旧版の tako が solo の起動で付けたタブ名（`tako solo` → `solo`、`tako solo -fast` → `solo-fast`）。
/// `TAKO_2120_LEGACY=1` の起動はこの名前を手動名で付ける（#2127 の A/B）
pub fn legacy_solo_title(suffix: Option<&str>) -> String {
    match suffix.map(str::trim).filter(|s| !s.is_empty()) {
        Some(s) => format!("solo-{s}"),
        None => "solo".to_string(),
    }
}

/// そのタブ名が、タブにいる solo のペイン（`role`）から見て旧版の起動が付けた名前か（#2127）。
///
/// master の [`is_legacy_role_title`] より狭く、**その solo の role から組み立てた名前と
/// 一致するときだけ**当たる（`solo:fast` のペインなら `solo-fast` だけ）。
/// ユーザーが solo のタブへ手で付けた `solo-作業` のような名前は残る
pub fn is_legacy_solo_title(title: &str, role: &str) -> bool {
    solo_suffix_of_role(role).is_some_and(|suffix| title == legacy_solo_title(suffix))
}

/// solo のタブの初期名（#2127）: solo が作業するフォルダの**リポジトリ名**。
///
/// - git リポジトリの中 → リポジトリのルートのフォルダ名（`~/dev/tako/crates` → `tako`）
/// - リポジトリの外 → そのフォルダの名前（`~/Documents/レポート` → `レポート`）
/// - HOME・ファイルシステムのルート → `None`（**タブ名に触らない**。そこからは何の
///   作業か分からないので、solo が主題で付ける）
///
/// 辿る範囲は [`crate::project_root`] の境界（HOME を天井にする・`.git` の段で止まる）で、
/// **git プロセスは起こさない**（段ごとの `.git` の有無だけ。layout の復元からも呼ぶ）
pub fn solo_initial_title(dir: &Path) -> Option<String> {
    solo_initial_title_in(dir, &SearchBounds::for_user())
}

/// [`solo_initial_title`] の本体（境界をテストから差し替える）
pub fn solo_initial_title_in(dir: &Path, bounds: &SearchBounds) -> Option<String> {
    let dirs = project_root::candidate_dirs(dir, bounds);
    // 空 = 開始の段が天井（HOME）かルート
    let start = dirs.first()?;
    // candidate_dirs は `.git` のある段で止まるので、リポジトリならそれが最後の段
    let root = dirs
        .last()
        .filter(|d| d.join(".git").exists())
        .unwrap_or(start);
    root.file_name()
        .map(|n| n.to_string_lossy().trim().to_string())
        .filter(|n| !n.is_empty())
}

/// アプリの自動命名（FR-2.12。画面の素材から haiku で名前を作る）がこのタブの名前を
/// 作ってよいか（`roles` はタブ内のペインの role ラベル）。
///
/// **master / solo のいるタブは本人が名前を付ける**ので、アプリは作らない。
/// 画面の素材（TUI の末尾・OSC タイトル）からはプロジェクトも科目も分からず、
/// 5 分おきの命名が master の付けた主題（`アジャイル`）を `claude作業` のような名前で
/// 上書きしてしまう。#2120 / #2127 以前は master / solo のタブが手動名で固定されていたので、
/// アプリがそれらのタブを命名しないのはそれまでと同じ挙動でもある
pub fn app_may_name_tab<'a>(mut roles: impl Iterator<Item = Option<&'a str>>) -> bool {
    legacy()
        || !roles.any(|role| {
            role.is_some_and(|r| {
                crate::handoff::master_profile_of_role(r).is_some() || is_solo_role(r)
            })
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keys(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn 管轄が無ければ初期名を付けない() {
        assert_eq!(initial_title(&[]), None);
        assert_eq!(initial_title(&keys(&["", "  "])), None);
    }

    #[test]
    fn 管轄が1件ならそのキー() {
        assert_eq!(initial_title(&keys(&["tako"])).as_deref(), Some("tako"));
        assert_eq!(
            initial_title(&keys(&["example.com"])).as_deref(),
            Some("example.com")
        );
    }

    #[test]
    fn 複数なら区切りで揃う共通の頭() {
        assert_eq!(
            initial_title(&keys(&["shop-api", "shop-web"])).as_deref(),
            Some("shop")
        );
        assert_eq!(
            initial_title(&keys(&["blog", "blog-web", "blog-admin"])).as_deref(),
            Some("blog")
        );
    }

    #[test]
    fn 区切りで揃わなければつなぐ() {
        // 文字は揃うが区切りの位置ではない（"ab" は名前の途中）
        assert_eq!(
            initial_title(&keys(&["abc", "abd"])).as_deref(),
            Some("abc / abd")
        );
        assert_eq!(
            initial_title(&keys(&["tako", "my-site"])).as_deref(),
            Some("tako / my-site")
        );
        // 日本語（マルチバイト）でも文字単位で数える
        assert_eq!(
            initial_title(&keys(&["授業-アジャイル", "授業-数理"])).as_deref(),
            Some("授業")
        );
    }

    #[test]
    fn masterのいるタブはアプリが命名しない() {
        if legacy() {
            return; // A/B の旧挙動で回しているときは対象外
        }
        assert!(!app_may_name_tab(
            [None, Some("orchestrator-master")].into_iter()
        ));
        assert!(!app_may_name_tab(
            [Some("orchestrator-master:takodev")].into_iter()
        ));
        // worker / role なしのタブは従来どおりアプリが命名する
        assert!(app_may_name_tab(
            [Some("orchestrator-worker:tako"), None].into_iter()
        ));
        assert!(app_may_name_tab(std::iter::empty()));
    }

    /// #2127: solo のいるタブも solo 自身が名前を付ける（2 つの語彙のどちらでも）
    #[test]
    fn soloのいるタブはアプリが命名しない() {
        if legacy() {
            return; // A/B の旧挙動で回しているときは対象外
        }
        for role in [
            "solo",
            "solo:fast",
            "orchestrator-solo",
            "orchestrator-solo:docs",
        ] {
            assert!(
                !app_may_name_tab([None, Some(role)].into_iter()),
                "{role} のタブをアプリが命名する"
            );
        }
        // solo に似た別の role は solo ではない
        assert!(app_may_name_tab(
            [
                Some("solo-ish"),
                Some("solo:"),
                Some("orchestrator-worker:solo")
            ]
            .into_iter()
        ));
    }

    #[test]
    fn soloのroleは2つの語彙を両方解く() {
        assert!(is_solo_role("solo"));
        assert!(is_solo_role("solo:fast"));
        assert!(is_solo_role("orchestrator-solo"));
        assert!(is_solo_role("orchestrator-solo:fast"));
        assert!(!is_solo_role("solo:"));
        assert!(!is_solo_role("soloist"));
        assert!(!is_solo_role("orchestrator-master"));
        assert!(!is_solo_role("worker:solo"));
    }

    #[test]
    fn soloの旧い名前はそのroleから組み立てた名前だけ() {
        assert_eq!(legacy_solo_title(None), "solo");
        assert_eq!(legacy_solo_title(Some("fast")), "solo-fast");
        assert_eq!(legacy_solo_title(Some(" ")), "solo");
        assert!(is_legacy_solo_title("solo", "solo"));
        assert!(is_legacy_solo_title("solo", "orchestrator-solo"));
        assert!(is_legacy_solo_title("solo-fast", "solo:fast"));
        assert!(is_legacy_solo_title("solo-fast", "orchestrator-solo:fast"));
        // role と名前が合わない・ユーザーが付けそうな名前・solo でない role は当たらない
        assert!(!is_legacy_solo_title("solo-fast", "solo"));
        assert!(!is_legacy_solo_title("solo", "solo:fast"));
        assert!(!is_legacy_solo_title("solo-作業", "solo"));
        assert!(!is_legacy_solo_title("tako", "solo"));
        assert!(!is_legacy_solo_title("solo", "orchestrator-master"));
        assert!(!is_legacy_solo_title("solo", "worker:tako"));
    }

    /// 一時 dir の中に組んだ木と、その一時 dir（= 仮の HOME）を天井にした境界を返す
    /// （本番の HOME を見ない）
    fn solo_tree(tag: &str) -> (std::path::PathBuf, SearchBounds) {
        let home = std::env::temp_dir().join(format!(
            "tako-2127-solo-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&home);
        std::fs::create_dir_all(home.join("dev/my-app/src/deep")).unwrap();
        std::fs::create_dir_all(home.join("dev/my-app/.git")).unwrap();
        std::fs::create_dir_all(home.join("Documents/レポート")).unwrap();
        // worktree / submodule の `.git` はファイル
        std::fs::create_dir_all(home.join("dev/wt-feature")).unwrap();
        std::fs::write(home.join("dev/wt-feature/.git"), "gitdir: ../x\n").unwrap();
        let home = crate::platform::path::canonicalize(&home).unwrap();
        let bounds = SearchBounds::with_ceilings([home.clone()]);
        (home, bounds)
    }

    #[test]
    fn soloの初期名はリポジトリ名() {
        let (home, bounds) = solo_tree("repo");
        let title = |rel: &str| solo_initial_title_in(&home.join(rel), &bounds);
        assert_eq!(title("dev/my-app").as_deref(), Some("my-app"));
        // リポジトリの奥で立てても、リポジトリのルートの名前
        assert_eq!(title("dev/my-app/src/deep").as_deref(), Some("my-app"));
        assert_eq!(title("dev/wt-feature").as_deref(), Some("wt-feature"));
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn soloの初期名はリポジトリの外ならフォルダ名_ホームなら付けない() {
        let (home, bounds) = solo_tree("plain");
        assert_eq!(
            solo_initial_title_in(&home.join("Documents/レポート"), &bounds).as_deref(),
            Some("レポート")
        );
        // HOME（天井）そのもの・ファイルシステムのルートは名前を付けない
        assert_eq!(solo_initial_title_in(&home, &bounds), None);
        assert_eq!(solo_initial_title_in(Path::new("/"), &bounds), None);
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn 役割名だけを旧い名前とみなす() {
        assert!(is_legacy_role_title("master"));
        assert!(is_legacy_role_title("master-takodev"));
        assert!(is_legacy_role_title("master-default"));
        // ユーザーが付けそうな別の名前は当たらない
        assert!(!is_legacy_role_title("tako"));
        assert!(!is_legacy_role_title("アジャイル"));
        assert!(!is_legacy_role_title("master-"));
        assert!(!is_legacy_role_title("masterplan"));
        assert!(!is_legacy_role_title("Master"));
    }
}
