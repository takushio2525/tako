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
        let canonical = path.canonicalize().unwrap_or_else(|_| path.clone());
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
