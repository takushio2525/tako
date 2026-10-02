//! 隣のテストの起動中に最後の 1 本が返っても、器（tmux サーバー）を畳まない（Issue #1866）
//!
//! 実 tmux の e2e は器を**プロセスごと**に持ち（`tmux_e2e::socket_for`）、同じバイナリの
//! テストはそれを参照カウントで共有する。旧実装は「`new-session` が返ってから数に入る」
//! 「数を減らしてから別に 0 か見て畳む」だったので、3 本目の `new-session` の最中に
//! 先の 2 本が返ると、最後に返った側が数 0 を見て**起動中の器ごと**畳み、ソケットファイル
//! まで消していた。3 本目は `error connecting to …/tako-e2e-1259-<pid> (No such file or
//! directory)` で落ちる（PR #1862 の全体テストで観測し、`new-session` の返りを遅らせる
//! 注入で main は 1/1・同じ数え方の旧アームは 3/3 で再現した）。
//!
//! ここでは時刻ではなく**差し込み口**でその瞬間を作る:
//!
//! 1. A が 1 本借りる
//! 2. B が `new-session` を叩く。器にセッションができ、呼び手へ返る直前で止まる
//! 3. その間に A を返す（= A が「最後の 1 本」に見えるかどうかが分かれ目）
//! 4. B を進め、B のセッションが器に生きているかを見る
//!
//! 既定（修正後）は B が数に入っているので器は残る。`TAKO_1866_LEGACY=1`（修正前の
//! 数え方）では A の返却が器を畳み、B のセッションもソケットも消える。env はプロセス
//! 全体の状態なので、このバイナリはテスト 1 本だけを持ち、その中で既定 → 旧の順に見る
//! （`issue1259_legacy_ab.rs` と同じ作り）。

use std::sync::mpsc;
use std::sync::Mutex;
use std::time::Duration;

#[path = "common/tmux_e2e.rs"]
mod tmux_e2e;

fn socket() -> &'static str {
    tmux_e2e::socket_for("1866")
}

/// 返ってこない相手を待ち続けないための上限（待ち時間の見積もりではない）。
/// 正常なら差し込み口の往復は数十 ms で済む
fn hang_guard() -> Duration {
    tmux_e2e::budget_for(tmux_e2e::BASE) * 2
}

fn real_tmux() -> bool {
    std::process::Command::new("tmux")
        .arg("-V")
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| tako_core::tmux::announces_only_tmux(&String::from_utf8_lossy(&o.stdout)))
        .unwrap_or(false)
}

/// 器のソケットファイルがあるか（`=` 付きの別名も見る。消すのは 1 実装だけ）
fn socket_file_exists() -> bool {
    let Some(dir) = tako_core::tmux_backend::socket_dir() else {
        return false;
    };
    dir.join(socket()).exists() || dir.join(format!("{}=", socket())).exists()
}

/// 1 回分の顛末
struct Outcome {
    /// A を返した後も B のセッションが器に生きていたか
    b_alive: bool,
    /// そのときの `has-session` の stderr（消えていればここに理由が出る）
    b_probe: String,
    /// B の最中に A を返した直後、ソケットファイルが残っていたか
    socket_after_a: bool,
    /// B も返した後（= 本当に最後の 1 本）にソケットファイルが消えたか
    socket_after_b: bool,
}

/// 差し込み口で「B の起動中に A が最後の 1 本として返る」瞬間を作って 1 回流す
fn race_once(tag: &str) -> Outcome {
    let a = format!("tako1866{tag}a");
    let b = format!("tako1866{tag}b");
    if let Err(diag) = tmux_e2e::new_session(socket(), &["-d", "-s", &a, "sleep", "600"]) {
        panic!("A のセッションを作れない: {diag}");
    }

    // B の `new-session` が器にセッションを作り、呼び手へ返る直前で止める
    let (created_tx, created_rx) = mpsc::channel::<()>();
    let (go_tx, go_rx) = mpsc::channel::<()>();
    let gate = Mutex::new(Some((created_tx, go_rx)));
    let target = b.clone();
    let guard = hang_guard();
    tmux_e2e::set_after_new_session(move |args| {
        if !args.contains(&target.as_str()) {
            return;
        }
        let taken = gate.lock().unwrap_or_else(|p| p.into_inner()).take();
        if let Some((created, go)) = taken {
            let _ = created.send(());
            let _ = go.recv_timeout(guard);
        }
    });
    let b_thread = {
        let b = b.clone();
        std::thread::spawn(move || {
            tmux_e2e::new_session(socket(), &["-d", "-s", &b, "sleep", "600"])
        })
    };
    created_rx
        .recv_timeout(hang_guard())
        .unwrap_or_else(|e| panic!("B の new-session が器にセッションを作らない: {e}"));

    // ここが #1866 の窓: B のセッションは器にあるが、B はまだ呼び手へ返っていない。
    // A を返す（修正前はここで数 0 を見て器を畳んでいた）
    tmux_e2e::release_session(socket(), &a);
    let socket_after_a = socket_file_exists();
    let _ = go_tx.send(());
    if let Err(diag) = b_thread.join().expect("B のスレッドが panic しない") {
        panic!("B のセッションを作れない: {diag}");
    }

    let probe = tmux_e2e::run_tmux(
        socket(),
        &["has-session", "-t", &tako_core::tmux::exact_target(&b)],
        tmux_e2e::BASE,
    );
    tmux_e2e::release_session(socket(), &b);
    Outcome {
        b_alive: probe.ok,
        b_probe: probe.stderr.trim().to_string(),
        socket_after_a,
        socket_after_b: socket_file_exists(),
    }
}

#[test]
fn 隣の起動中に最後の1本が返っても器を畳まない() {
    if !real_tmux() {
        eprintln!("[e2e-1866] 本物の tmux が無いのでスキップ");
        return;
    }
    std::env::remove_var("TAKO_1866_LEGACY");

    // --- 既定（修正後）: B が先に数に入っているので、A の返却は器を畳まない ---
    let fixed = race_once("new");
    assert!(
        fixed.b_alive && fixed.socket_after_a,
        "起動中の隣（B）がいるのに、A の返却が器を畳んだ（#1866 の再発）。\
         `crates/tako-control/tests/common/tmux_e2e.rs` の `new_session` は器を叩く前に \
         数に入り、`release_session` は減算・0 判定・kill-server を 1 つのロックの中で行うこと。\
         B: has-session={:?} / A を返した直後のソケットファイル={}",
        fixed.b_probe,
        fixed.socket_after_a
    );
    // 「畳まない」に倒しすぎていない: 本当に最後の 1 本が返ったら器もソケットも消える
    assert!(
        !fixed.socket_after_b,
        "最後の 1 本（B）が返った後もソケットファイル {} が残っている（器を畳む性質まで消えた）",
        socket()
    );
    eprintln!(
        "[1866-ab] 既定: A を返した直後 B={} ソケット={} / B を返した後 ソケット={}",
        if fixed.b_alive { "生存" } else { "消滅" },
        fixed.socket_after_a,
        fixed.socket_after_b
    );

    // --- 旧（修正前の数え方）: 同じ瞬間に A の返却が B ごと器を畳む = この検査の感度 ---
    std::env::set_var("TAKO_1866_LEGACY", "1");
    let legacy = race_once("old");
    std::env::remove_var("TAKO_1866_LEGACY");
    eprintln!(
        "[1866-ab] TAKO_1866_LEGACY=1: A を返した直後 B={} ソケット={} has-session={:?}",
        if legacy.b_alive { "生存" } else { "消滅" },
        legacy.socket_after_a,
        legacy.b_probe
    );
    assert!(
        !legacy.b_alive && !legacy.socket_after_a,
        "旧アーム（TAKO_1866_LEGACY=1）で #1866 が再現しない = この検査が窓を作れていない。\
         差し込み口 `tmux_e2e::set_after_new_session` の位置を確かめること。\
         B: has-session={:?} / A を返した直後のソケットファイル={}",
        legacy.b_probe,
        legacy.socket_after_a
    );
    assert!(
        !legacy.socket_after_b,
        "旧アームの後片付けでソケットファイル {} が残った",
        socket()
    );
}
