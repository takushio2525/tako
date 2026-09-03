// 左サイドバー（ファイルツリー）の幅のクランプ規則（#307 / #789）
//
// #307 でドラッグリサイズ + 永続化 + CLI / MCP 1:1 が入ったが、上限だけが経路で
// 食い違っていた（ドラッグ = ウィンドウ幅の 50% / dispatch = 固定 600px）。
// 下限 120px は両経路で一致していた。ここを規則の**正**にして両方から呼ぶ。
//
// 上限をウィンドウ幅の割合（= ドラッグ側）へ寄せた理由:
//   1. 設計原則 5「AI フルコントロール」= UI でできることは AI からもできる。
//      固定 600px では広いウィンドウでドラッグ相当の幅に CLI から到達できない
//   2. 上限は本来ウィンドウ幅に依存する量で、固定 px は窓が狭いと過大になる
//      （600px は 800px の窓では 75%。サイドバーがペインを潰す）
//
// 呼び出し側の約束（#789 の設計判断）:
//   - 状態として持つのは**要求値**（下限だけ効かせた値）。永続化もこれ
//   - 描画・座標計算に使うのは `clamp_width(要求値, そのウィンドウのビューポート幅)`
//   - ビューポート幅が分からない文脈（起動直後・GUI 非依存の呼び出し元）では
//     上限を課さない。実際の上限は幅が分かる場所（描画時）で必ず掛かる
// これでウィンドウが後から狭くなっても要求値は保たれ（= 再起動・窓の再拡大で
// 意図が戻る）、その時点の描画は必ず窓に収まる。

/// サイドバー幅の下限（px）
pub const MIN_WIDTH: f32 = 120.0;

/// サイドバー幅の上限（ビューポート幅に対する比）
pub const MAX_RATIO: f32 = 0.5;

/// そのビューポート幅における上限（px）。
///
/// ビューポート幅が不明（0 以下・非有限）なら上限なし（`f32::INFINITY`）を返す。
/// 下限を下回る上限は返さない（極端に狭い窓でも幅 0 に潰さない）。
pub fn max_width(viewport_width: f32) -> f32 {
    if !viewport_width.is_finite() || viewport_width <= 0.0 {
        return f32::INFINITY;
    }
    (viewport_width * MAX_RATIO).max(MIN_WIDTH)
}

/// 要求幅をそのビューポート幅で許される値へクランプする。
///
/// `f32::clamp` は `min > max` で panic し NaN をそのまま返すので使わない
/// （設定ファイル由来・IPC 由来の値がそのまま来る経路なので防御する）。
pub fn clamp_width(requested: f32, viewport_width: f32) -> f32 {
    if !requested.is_finite() {
        return MIN_WIDTH;
    }
    requested.max(MIN_WIDTH).min(max_width(viewport_width))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 下限は経路に関係なく一定() {
        for viewport in [0.0, 400.0, 1200.0, 4000.0] {
            assert_eq!(clamp_width(0.0, viewport), MIN_WIDTH);
            assert_eq!(clamp_width(-100.0, viewport), MIN_WIDTH);
            assert_eq!(clamp_width(50.0, viewport), MIN_WIDTH);
        }
    }

    #[test]
    fn 上限はビューポート幅の半分() {
        assert_eq!(max_width(1200.0), 600.0);
        assert_eq!(clamp_width(5000.0, 1200.0), 600.0);
        // #789 の本体: 旧 dispatch 経路の固定 600px では届かなかった領域
        assert_eq!(clamp_width(5000.0, 3000.0), 1500.0);
        assert_eq!(clamp_width(900.0, 3000.0), 900.0);
    }

    #[test]
    fn 狭い窓でも下限を割らない() {
        // 200px の窓なら 50% は 100px だが、下限 120px を優先する
        assert_eq!(max_width(200.0), MIN_WIDTH);
        assert_eq!(clamp_width(180.0, 200.0), MIN_WIDTH);
    }

    #[test]
    fn ビューポート幅が不明なら上限を課さない() {
        for unknown in [0.0, -1.0, f32::NAN, f32::INFINITY] {
            assert_eq!(max_width(unknown), f32::INFINITY);
            assert_eq!(clamp_width(2000.0, unknown), 2000.0);
            assert_eq!(clamp_width(10.0, unknown), MIN_WIDTH);
        }
    }

    #[test]
    fn 非有限の要求値は下限へ落とす() {
        for bad in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            assert_eq!(clamp_width(bad, 1200.0), MIN_WIDTH);
        }
    }

    #[test]
    fn クランプは冪等() {
        for viewport in [300.0, 1200.0, 3000.0] {
            for requested in [0.0, 121.0, 600.0, 5000.0] {
                let once = clamp_width(requested, viewport);
                assert_eq!(clamp_width(once, viewport), once);
            }
        }
    }

    /// #789 の受け入れ条件 1: 値域を表で固定する。ドラッグ経路（入力 = マウスの x 座標）と
    /// dispatch 経路（入力 = 要求 px）はどちらもこの関数を通すので、経路が増えても
    /// 上限・下限が食い違わない。実経路どうしの一致はセルフテスト項目 109 が見る
    #[test]
    fn 値域は入力の意味に関係なく同じ表になる() {
        let viewport = 1440.0; // 上限 = 720
        let table = [
            (-50.0, MIN_WIDTH),
            (0.0, MIN_WIDTH),
            (119.0, MIN_WIDTH),
            (120.0, 120.0),
            (400.0, 400.0),
            (719.0, 719.0),
            (720.0, 720.0),
            (721.0, 720.0),
            (9999.0, 720.0),
        ];
        for (requested, expected) in table {
            assert_eq!(clamp_width(requested, viewport), expected, "{requested}");
        }
    }
}

// ─────────── ワークスペースフォルダ（ツリーのルート行）の解決（#1009） ───────────
//
// 「タブ = ワークスペース」なので、ルート行はそのタブの各ペインの cwd + 明示追加
// フォルダ（#134）を並べたもの。UI（サイドバー）と CLI / MCP（`tako tree git-status`）が
// **この 1 実装**を共有するので、画面に出ている範囲と応答の範囲がずれない。

use std::path::PathBuf;

/// ルート行の並びを作る。symlink を解決したパスで重複を落とし、**渡された順**を保つ
/// （表示順が呼び出しごとに揺れないため）。返すのは解決前のパス = 画面に出る表記のまま。
///
/// どちらも空なら `home` を 1 件だけ返す（初回起動で真っ白にしない）。
pub fn workspace_roots(
    pane_cwds: impl IntoIterator<Item = PathBuf>,
    pinned: impl IntoIterator<Item = PathBuf>,
    home: Option<PathBuf>,
) -> Vec<PathBuf> {
    let mut seen: std::collections::HashSet<PathBuf> = std::collections::HashSet::new();
    let mut roots: Vec<PathBuf> = Vec::new();
    for path in pane_cwds.into_iter().chain(pinned) {
        // 解決は境界（B26）を通す。ピン留めフォルダは非 verbatim で保存されるので、
        // ここで verbatim のキーを作ると同じフォルダを 2 行出す（#970）
        let canonical = crate::platform::path::canonicalize_or_self(&path);
        if seen.insert(canonical) {
            roots.push(path);
        }
    }
    if roots.is_empty() {
        roots.extend(home);
    }
    roots
}

#[cfg(test)]
mod workspace_roots_tests {
    use super::*;

    #[test]
    fn 重複を落として渡された順を保つ() {
        let roots = workspace_roots(
            vec![
                PathBuf::from("/a"),
                PathBuf::from("/b"),
                PathBuf::from("/a"),
            ],
            vec![PathBuf::from("/c"), PathBuf::from("/b")],
            None,
        );
        assert_eq!(
            roots,
            vec![
                PathBuf::from("/a"),
                PathBuf::from("/b"),
                PathBuf::from("/c")
            ]
        );
    }

    #[test]
    fn 何も無ければホームだけを返す() {
        assert_eq!(
            workspace_roots(vec![], vec![], Some(PathBuf::from("/home/testuser"))),
            vec![PathBuf::from("/home/testuser")]
        );
        assert!(workspace_roots(vec![], vec![], None).is_empty());
    }

    #[test]
    fn ルートがあればホームは足さない() {
        assert_eq!(
            workspace_roots(
                vec![PathBuf::from("/a")],
                vec![],
                Some(PathBuf::from("/home/testuser"))
            ),
            vec![PathBuf::from("/a")]
        );
    }
}

// ─────────── リモートルートの並び（#1041 / #976 / #919） ───────────
//
// リモートフォルダをローカルの前に出すか後ろに出すかは、GUI（`FileTree::build_rows`）と
// CLI / MCP（`remote-folder list`）の両方が答えを持つ。散らすと「画面の並びと
// 応答の並びが違う」になるので、**規則の正本をここ 1 本**にする（Issue #1041 の
// 設計メモ「並び規則の正本は 1 箇所に」）。
//
// 経緯（3 世代あるので、どれが今の既定かを取り違えないこと）:
//   #919  = リモートは全部ローカルより前へ hoist（開いた直後に見えないと分からない）
//   #976  = 全部ローカルの後ろ（`ssh` 検知で日常的に増えるので特別扱いをやめた）
//   #1041 = **明示 open は前・自動検知は後ろ**（明示 open はユーザーの主作業対象）

use crate::remote_fs::{RemoteFolder, RemoteRef};

/// リモートルートをローカルルートのどちら側に置くか（#1041）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RemoteRootPlacement {
    /// 既定（#1041）: 明示 open はローカルより前・自動検知は後ろ
    #[default]
    ExplicitFirst,
    /// #976 の挙動（`TAKO_1041_LEGACY=1`）: 経路を問わず全部ローカルの後ろ
    AllTrailing,
    /// #919 の挙動（`TAKO_976_LEGACY=1`）: 経路を問わず全部ローカルより前
    AllLeading,
}

/// ローカルルートの前後に分けたリモートルート（#1041）。
/// どちらのリストも**渡された順**（= 開いた順）を保つ
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RemoteRootOrder {
    /// ローカルルートより前に出すもの
    pub leading: Vec<RemoteRef>,
    /// ローカルルートより後ろに出すもの
    pub trailing: Vec<RemoteRef>,
}

impl RemoteRootOrder {
    /// 表示順（leading → trailing）に並べ直したもの。
    /// ローカルルートを挟まない一覧（`remote-folder list`）で使う
    pub fn display_order(&self) -> Vec<&RemoteRef> {
        self.leading.iter().chain(self.trailing.iter()).collect()
    }

    /// そのフォルダがローカルの前か後ろか（応答の `placement`）
    pub fn placement_of(&self, remote: &RemoteRef) -> &'static str {
        if self.leading.iter().any(|r| r == remote) {
            "leading"
        } else {
            "trailing"
        }
    }
}

/// リモートルートの並びを決める（#1041 の正本）。
///
/// 入力は**表示したい順に並んだ**フォルダ列（呼び出し側が「開いた順」で渡す）。
/// 分けるだけで並べ替えないので、`leading` / `trailing` の中の順序は入力どおり。
pub fn remote_root_order(
    folders: &[RemoteFolder],
    placement: RemoteRootPlacement,
) -> RemoteRootOrder {
    let mut order = RemoteRootOrder::default();
    for folder in folders {
        let lead = match placement {
            RemoteRootPlacement::ExplicitFirst => folder.is_explicit(),
            RemoteRootPlacement::AllTrailing => false,
            RemoteRootPlacement::AllLeading => true,
        };
        if lead {
            order.leading.push(folder.remote.clone());
        } else {
            order.trailing.push(folder.remote.clone());
        }
    }
    order
}

#[cfg(test)]
mod remote_root_order_tests {
    use super::*;
    use crate::remote_fs::RemoteOrigin;

    fn folders() -> Vec<RemoteFolder> {
        vec![
            RemoteFolder::auto(RemoteRef::new("linux", "/srv/home")),
            RemoteFolder::explicit(RemoteRef::new("win", "/C:/Users/winuser/dev")),
            RemoteFolder::explicit(RemoteRef::new("linux", "/srv/work")),
        ]
    }

    #[test]
    fn 既定は明示openだけを前へ出す() {
        let order = remote_root_order(&folders(), RemoteRootPlacement::ExplicitFirst);
        assert_eq!(
            order.leading,
            vec![
                RemoteRef::new("win", "/C:/Users/winuser/dev"),
                RemoteRef::new("linux", "/srv/work"),
            ],
            "明示 open は渡された順のまま前へ"
        );
        assert_eq!(
            order.trailing,
            vec![RemoteRef::new("linux", "/srv/home")],
            "自動検知は後ろのまま（#976 に回帰ゼロ）"
        );
    }

    /// #1041 の A/B（`TAKO_1041_LEGACY=1`）: #976 の「全部後ろ」へ戻る
    #[test]
    fn legacyは経路を問わず全部後ろ() {
        let order = remote_root_order(&folders(), RemoteRootPlacement::AllTrailing);
        assert!(order.leading.is_empty());
        assert_eq!(order.trailing.len(), 3);
        assert_eq!(order.trailing[0], RemoteRef::new("linux", "/srv/home"));
    }

    /// #976 の A/B（`TAKO_976_LEGACY=1`）: #919 の「全部前」へ戻る
    #[test]
    fn 旧919は経路を問わず全部前() {
        let order = remote_root_order(&folders(), RemoteRootPlacement::AllLeading);
        assert!(order.trailing.is_empty());
        assert_eq!(order.leading.len(), 3);
    }

    #[test]
    fn 表示順はleadingのあとにtrailing() {
        let order = remote_root_order(&folders(), RemoteRootPlacement::ExplicitFirst);
        let display: Vec<String> = order.display_order().iter().map(|r| r.label()).collect();
        assert_eq!(
            display,
            vec![
                "win:/C:/Users/winuser/dev".to_string(),
                "linux:/srv/work".to_string(),
                "linux:/srv/home".to_string(),
            ]
        );
        assert_eq!(
            order.placement_of(&RemoteRef::new("win", "/C:/Users/winuser/dev")),
            "leading"
        );
        assert_eq!(
            order.placement_of(&RemoteRef::new("linux", "/srv/home")),
            "trailing"
        );
    }

    #[test]
    fn 空なら空() {
        for placement in [
            RemoteRootPlacement::ExplicitFirst,
            RemoteRootPlacement::AllTrailing,
            RemoteRootPlacement::AllLeading,
        ] {
            assert_eq!(
                remote_root_order(&[], placement),
                RemoteRootOrder::default()
            );
        }
    }

    /// 旧ファイル（経路を記録していない世代）は `Auto` へ倒れる = 従来の並び
    #[test]
    fn 経路不明は自動扱いで後ろへ() {
        let legacy = vec![RemoteFolder::new(
            RemoteRef::new("linux", "/srv/home"),
            RemoteOrigin::default(),
        )];
        let order = remote_root_order(&legacy, RemoteRootPlacement::ExplicitFirst);
        assert!(order.leading.is_empty());
        assert_eq!(order.trailing.len(), 1);
    }
}
