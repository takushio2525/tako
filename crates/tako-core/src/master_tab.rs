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
//! # ここが持つもの（判定と組み立てだけ。ファイル I/O なし）
//!
//! - [`initial_title`] — 起動・採用時の初期名（管轄プロジェクトのキー）
//! - [`is_legacy_role_title`] — 旧版の tako が付けた役割名か（復元時に手動名を解除する対象）
//! - [`app_may_name_tab`] — アプリの自動命名（FR-2.12）がそのタブの名前を作ってよいか
//! - [`legacy`] — A/B（`TAKO_2120_LEGACY=1` で #2120 前の挙動へ戻す）

/// #2120 の A/B。`TAKO_2120_LEGACY=1` で、起動経路が `master` / `master-<名前>` を
/// 手動名で付け・採用がタブ名に触れず・復元が旧い手動名を解除せず・アプリの自動命名が
/// master のタブも命名する旧挙動へ戻る
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

/// アプリの自動命名（FR-2.12。画面の素材から haiku で名前を作る）がこのタブの名前を
/// 作ってよいか（`roles` はタブ内のペインの role ラベル）。
///
/// **master のいるタブは master 自身が名前を付ける**ので、アプリは作らない。
/// 画面の素材（TUI の末尾・OSC タイトル）からはプロジェクトも科目も分からず、
/// 5 分おきの命名が master の付けた主題（`アジャイル`）を `claude作業` のような名前で
/// 上書きしてしまう。#2120 以前は master のタブが手動名で固定されていたので、
/// アプリが master のタブを命名しないのはそれまでと同じ挙動でもある
pub fn app_may_name_tab<'a>(mut roles: impl Iterator<Item = Option<&'a str>>) -> bool {
    legacy()
        || !roles.any(|role| {
            role.and_then(crate::handoff::master_profile_of_role)
                .is_some()
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
        // worker / solo / role なしのタブは従来どおりアプリが命名する
        assert!(app_may_name_tab(
            [Some("orchestrator-worker:tako"), Some("solo"), None].into_iter()
        ));
        assert!(app_may_name_tab(std::iter::empty()));
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
