//! テストが「存在しないペイン / タブ / ウィンドウ」を表す ID を、採番器から 1 つ取る
//! 1 実装（Issue #1868）
//!
//! ## 何が起きていたか
//!
//! ペイン・タブ・ウィンドウの ID は、それぞれ**プロセス全体で共有**の採番器
//! （`PANE_ID_COUNTER` / `TAB_ID_COUNTER` / `WINDOW_ID_COUNTER`）で振られる。テストの
//! バイナリは 1 プロセスで数千のテストを並列に走らせるので、`305` / `9999` のような
//! 決め打ちの「存在しない ID」は、**同じバイナリの他のテストが何枚作ったか**しだいで
//! そのテスト自身の Workspace に実在しうる。実物: #1860 でテストが増えた後、dispatch の
//! テストが stale のつもりの `305` を引き当て、CI の 3 並列で毎回落ちた（2ecab5d）。
//!
//! ## 直し方
//!
//! [`pane`] / [`tab`] / [`window`] は採番器から 1 つ取って、そのまま返す（ペインもタブも
//! 作らない）。採番器は単調増加なので、取った値は**以後どのペイン / タブ / ウィンドウにも
//! 振られない**。復元（`Pane::restore` / `TabId::from_raw` / `WindowId::from_raw`）は
//! 採番器を ID の先へ進めてから使うので、既に Workspace に居る ID とも重ならない。
//!
//! ## 採らなかった形
//!
//! - **`u64::MAX` 系の定数**: `TabId::from_raw` / `WindowId::from_raw` と `Pane::restore` は
//!   採番器を `fetch_max(id + 1)` で進める。`u64::MAX` が dispatch を通って `from_raw` に
//!   届くと採番器が `u64::MAX` に張り付き、次の採番で 0 へ巻き戻って**同じプロセスの
//!   全テストの ID が重複し始める**。ペインだけは `PaneId::from_raw` が進めないので
//!   使えるが、3 種で作法を分けると取り違える
//! - **ホストの最大 ID + 定数**（2ecab5d の `absent_pane_id`）: 呼んだ時点のそのホストに
//!   無いことしか言えず、復元が採番器を飛ばすと後から作るペインと重なりうる。
//!   dispatch の `MockHost` 専用で、tako-core の `Workspace` や tako-app のテストから使えない
//!
//! ## 使ってはいけない場面
//!
//! 取った値は**決め打ちの小さい固定値**（`PaneId::from_raw(2)` 等）とは重なりうる
//! （プロセスの最初の方では採番器も小さい）。固定値で組んだ純粋な表（ペインを採番しない
//! テスト）の「無いもの」は、ここから取らずに表に無い固定値を使い、その行か 1 行上に
//! `// 固定 ID 安全: <理由>` を書く（番犬 `issue1868_stale_id_watchdog` の目印）。

use crate::pane::PaneId;
use crate::tab::TabId;
use crate::workspace::WindowId;

/// どのペインにも振られないと保証されたペイン ID（「存在しないペイン」の代役）
pub fn pane() -> PaneId {
    PaneId::next()
}

/// どのタブにも振られないと保証されたタブ ID（「存在しないタブ」の代役）
pub fn tab() -> TabId {
    TabId::next()
}

/// どのウィンドウにも振られないと保証されたウィンドウ ID（「存在しないウィンドウ」の代役）
pub fn window() -> WindowId {
    WindowId::next()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pane::{Pane, PaneOrigin};
    use crate::workspace::Workspace;

    #[test]
    fn 取ったidは前後に作ったものと重ならない() {
        let mut ws = Workspace::new("t1", Pane::new(PaneOrigin::User));
        let before = ws.active_tab().tree().focused();
        let (pane_id, tab_id) = (pane(), tab());
        let w = window();
        // 取った後に作ったペイン・タブ・ウィンドウとも重ならない（採番器は単調増加）
        let after = Pane::new(PaneOrigin::User);
        assert!(pane_id != before && pane_id != after.id());
        let t2 = ws.create_tab("t2", after);
        assert!(tab_id != ws.active_tab_id() && tab_id != t2);
        let (w2, _) = ws.create_window("t3", Pane::new(PaneOrigin::User));
        assert!(w != ws.active_window_id() && w != w2);
        // Workspace からは引けない
        assert!(ws.pane_anywhere(pane_id).is_none());
        assert!(ws.get_tab(tab_id).is_none());
        assert!(ws.get_window(w).is_none());
        // 2 回取れば別の値
        assert_ne!(pane(), pane());
        assert_ne!(tab(), tab());
        assert_ne!(window(), window());
    }
}
