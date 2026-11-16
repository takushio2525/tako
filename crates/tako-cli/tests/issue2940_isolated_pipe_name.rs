//! 実経路の番犬: 検証起動（`TAKO_ISOLATED=1` + 専用の置き場）の本体の受け口は、Windows でも本番の
//! 固定名 `\\.\pipe\tako-<ユーザー>` を取らず、発見ファイルに書いた名前で CLI が届く（Issue #2940）。
//!
//! 修正前（Windows 11 実機 = 0.8.40）: 隔離の GUI の control.json が `\\.\pipe\tako-<ユーザー>` になった。
//! Windows の `preferred_pipe_name` が一時名を使うのはセカンダリ・`cfg!(test)`・`TAKO_SELF_TEST` のときだけで、
//! 固定名は data dir に依らなかった（unix は `ipc_socket::plan()` で data dir ごとに決まる）。
//!
//! GUI は立てない。GUI が起動時に呼ぶのと**同じ口** `IpcServer::start`（= `start_with(.., false)`）で受け口を張り、
//! GUI と同じ `discovery::write` で接続情報を書いて、**ビルドした `tako` をそのまま**叩く。
//! 統合テスト（別バイナリ）なので tako-control の `cfg(test)` は立たず、本番の名前の決め方を通る
//! （`issue1441_ipc_deep_data_dir` と同じ理由）。
//!
//! - Windows: 名前は data dir ごとの名前（`tako_core::ipc_socket::verification_pipe_name`）で固定名と違う
//! - unix: 名前は data dir ごとのソケット（`ipc_socket::plan()`。修正の前後で同じ）
//! - 同じ data dir の 2 本目は先客の名前を奪わず一時名へ逃げ、1 本目は生きたまま
//! - A/B（`TAKO_2940_LEGACY=1`）で決め方が修正前（固定名）へ戻る。固定名は**張らない**
//!   （走らせた機械の本番の GUI の名前を取り合わない）
//!
//! env を書き換えるので、どのテストも `run_alone!` の子で走らせる（#2400）。名前に出る利用者名は
//! 子の `USERNAME` を `winuser` に差し替えて固定する（走らせた人の名前をテストの出力に出さない）

use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use futures::channel::mpsc::{unbounded, UnboundedReceiver};
use futures::StreamExt;
use serde_json::json;
use tako_control::discovery::ControlInfo;
use tako_control::protocol::Request;
use tako_control::{IncomingRequest, IpcServer};
use tako_core::ipc_socket::{self, PipeNamePlan};
use tako_core::test_residue::ScratchDir;

const TOKEN: &str = "tk2940-token";
/// 子の利用者名（名前に出る。実名を出さない）
const USER: &str = "winuser";

/// 叩くバイナリ（この木でビルドした `tako`）
fn tako() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_tako"))
}

/// 受け口へ届いた `list` を数えて空の一覧で答える（GUI の UI スレッドの代わり）
fn respond(mut rx: UnboundedReceiver<IncomingRequest>) -> Arc<AtomicUsize> {
    let lists = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&lists);
    std::thread::spawn(move || {
        while let Some(incoming) = futures::executor::block_on(rx.next()) {
            if matches!(incoming.request, Request::List) {
                counter.fetch_add(1, Ordering::SeqCst);
            }
            let _ = incoming.reply.send(Ok(json!({ "tabs": [] })));
        }
    });
    lists
}

/// `TAKO_*` を全部外し、隔離の印と置き場だけを渡した `tako list`（`extra` は足す env）
fn tako_list(data: &Path, discovery: &Path, extra: &[(&str, &str)]) -> Output {
    let mut command = Command::new(tako());
    command
        .arg("list")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for (key, _) in std::env::vars_os() {
        if key.to_string_lossy().starts_with("TAKO_") {
            command.env_remove(key);
        }
    }
    command
        .env("TAKO_ISOLATED", "1")
        .env("TAKO_DATA_DIR", data)
        .env("TAKO_DISCOVERY_DIR", discovery);
    for (key, value) in extra {
        command.env(key, value);
    }
    command.output().expect("tako を起こせる")
}

fn describe(output: &Output) -> String {
    format!(
        "status={} stdout={} stderr={}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

/// 落ちたときに名指す決め方の正本
fn plan_site() -> String {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let rel = "crates/tako-core/src/ipc_socket.rs";
    let text = std::fs::read_to_string(root.join(rel)).unwrap_or_default();
    match text
        .lines()
        .position(|l| l.contains("pub fn pipe_name_plan_from_env("))
    {
        Some(i) => format!("{rel}:{}（fn pipe_name_plan_from_env）", i + 1),
        None => format!("{rel}（fn pipe_name_plan_from_env が見つからない）"),
    }
}

#[test]
fn 隔離の受け口は本番の固定名を取らず発見ファイルの名前でcliが届く() {
    if tako_core::run_alone!(隔離の受け口は本番の固定名を取らず発見ファイルの名前でcliが届く)
    {
        return;
    }
    let scratch = ScratchDir::new("tk2940");
    let data = scratch.path().join("d");
    let discovery = scratch.path().join("disc");
    std::fs::create_dir_all(&data).expect("data dir を作れる");
    std::env::set_var("TAKO_ISOLATED", "1");
    std::env::set_var("TAKO_DATA_DIR", &data);
    std::env::set_var("TAKO_DISCOVERY_DIR", &discovery);
    std::env::set_var("USERNAME", USER);
    for key in [
        "TAKO_SELF_TEST",
        "TAKO_2940_LEGACY",
        "TAKO_SOCKET",
        "TAKO_TOKEN",
    ] {
        std::env::remove_var(key);
    }
    ipc_socket::reset_status_for_test();

    // GUI の起動と同じ口（セカンダリでない = `start_with(.., false)`）
    let (tx, rx) = unbounded::<IncomingRequest>();
    let server = IpcServer::start(tx, TOKEN.into()).expect("隔離の受け口が立つ");
    let first_lists = respond(rx);
    let endpoint = server.endpoint().to_string();

    #[cfg(windows)]
    {
        let fixed = ipc_socket::fixed_pipe_name(&ipc_socket::pipe_user_tag(Some(USER)));
        assert_ne!(
            endpoint,
            fixed,
            "検証プロセスの受け口が本番の固定名を取った（#2940 の修正前）。決め方: {}",
            plan_site()
        );
        assert_eq!(
            endpoint,
            ipc_socket::verification_pipe_name(USER, &data),
            "検証プロセスの受け口が data dir ごとの名前になっていない。決め方: {}",
            plan_site()
        );
        // CI のログへ残す（名前に載るのは差し替えた利用者名と data dir のハッシュだけ）
        println!("TAKO2940_ENDPOINT verification={endpoint} fixed={fixed}");
    }
    #[cfg(unix)]
    assert_eq!(
        endpoint,
        ipc_socket::plan()
            .expect("置き場を決められる")
            .path
            .display()
            .to_string(),
        "unix の受け口が data dir ごとのソケットでない（#2940 は unix を変えない）"
    );
    assert_eq!(
        ipc_socket::status().and_then(|s| s.endpoint).as_deref(),
        Some(endpoint.as_str()),
        "立った受け口の記録（check_health が読む）が実際の名前と違う"
    );

    // GUI と同じ書き方で接続情報を書き、CLI が発見ファイルの名前だけで届く
    tako_control::discovery::write(&ControlInfo {
        version: 1,
        pid: std::process::id(),
        socket: endpoint.clone(),
        token: TOKEN.into(),
        mcp_url: None,
    })
    .expect("接続情報を書ける");
    let written = tako_control::discovery::read().expect("control.json を読める");
    assert_eq!(
        written.socket, endpoint,
        "control.json の名前が受け口と違う"
    );
    let output = tako_list(&data, &discovery, &[]);
    assert!(output.status.success(), "{}", describe(&output));
    assert_eq!(
        first_lists.load(Ordering::SeqCst),
        1,
        "CLI が発見ファイルの名前で隔離の受け口へ届いていない: {}",
        describe(&output)
    );

    // エッジ: 同じ data dir の 2 本目は先客の名前を奪わず一時名へ逃げる（1 本目は生きたまま）
    let (tx2, rx2) = unbounded::<IncomingRequest>();
    let second = IpcServer::start(tx2, TOKEN.into()).expect("2 本目も立つ");
    let second_lists = respond(rx2);
    assert_ne!(second.endpoint(), endpoint, "生きた先客の名前を奪っている");
    #[cfg(windows)]
    {
        assert!(
            second
                .endpoint()
                .starts_with(&format!(r"\\.\pipe\tako-{USER}-{}-", std::process::id())),
            "2 本目が一時名へ逃げていない: {}",
            second.endpoint()
        );
        println!("TAKO2940_SECOND temp={}", second.endpoint());
    }
    let output = tako_list(&data, &discovery, &[]);
    assert!(output.status.success(), "{}", describe(&output));
    assert_eq!(
        first_lists.load(Ordering::SeqCst),
        2,
        "2 本目の起動で 1 本目の受け口が死んだ: {}",
        describe(&output)
    );
    // 2 本目へは名前を明示して届く（一時名も CLI の接続の口で繋がる）
    let output = tako_list(
        &data,
        &discovery,
        &[("TAKO_SOCKET", second.endpoint()), ("TAKO_TOKEN", TOKEN)],
    );
    assert!(output.status.success(), "{}", describe(&output));
    assert_eq!(
        second_lists.load(Ordering::SeqCst),
        1,
        "2 本目の受け口へ届いていない: {}",
        describe(&output)
    );
    println!(
        "TAKO2940_CLI first_lists={} second_lists={}",
        first_lists.load(Ordering::SeqCst),
        second_lists.load(Ordering::SeqCst)
    );

    drop(second);
    drop(server);
    ipc_socket::reset_status_for_test();
}

#[test]
fn ab_の腕では検証プロセスの決め方が修正前の固定名へ戻る() {
    if tako_core::run_alone!(ab_の腕では検証プロセスの決め方が修正前の固定名へ戻る)
    {
        return;
    }
    let scratch = ScratchDir::new("tk2940ab");
    let data = scratch.path().join("d");
    std::env::set_var("TAKO_ISOLATED", "1");
    std::env::set_var("TAKO_DATA_DIR", &data);
    std::env::set_var("USERNAME", USER);
    std::env::remove_var("TAKO_SELF_TEST");
    std::env::remove_var("TAKO_2940_LEGACY");
    assert!(
        tako_core::paths::is_verification_process(),
        "テストのプロセスは検証プロセス（番犬の前提）"
    );

    // 修正後: data dir ごとの名前（本番の固定名ではない）
    let fixed = ipc_socket::fixed_pipe_name(USER);
    assert_eq!(
        ipc_socket::pipe_name_plan_from_env(false),
        PipeNamePlan::Verification(ipc_socket::verification_pipe_name(USER, &data)),
        "決め方: {}",
        plan_site()
    );

    // A/B: 修正前の固定名へ戻る（名前を決めるだけで張らない）
    std::env::set_var("TAKO_2940_LEGACY", "1");
    assert_eq!(
        ipc_socket::pipe_name_plan_from_env(false),
        PipeNamePlan::Fixed(fixed.clone()),
        "TAKO_2940_LEGACY=1 で修正前へ戻っていない。決め方: {}",
        plan_site()
    );
    println!("TAKO2940_AB legacy=fixed({fixed})");
    std::env::set_var("TAKO_2940_LEGACY", "0");
    assert!(
        matches!(
            ipc_socket::pipe_name_plan_from_env(false),
            PipeNamePlan::Verification(_)
        ),
        "TAKO_2940_LEGACY=0 は修正後のまま"
    );
    std::env::remove_var("TAKO_2940_LEGACY");

    // セカンダリ・セルフテストは今どおり一時名
    assert_eq!(
        ipc_socket::pipe_name_plan_from_env(true),
        PipeNamePlan::Temp
    );
    std::env::set_var("TAKO_SELF_TEST", "1");
    assert_eq!(
        ipc_socket::pipe_name_plan_from_env(false),
        PipeNamePlan::Temp
    );
    std::env::remove_var("TAKO_SELF_TEST");
}
