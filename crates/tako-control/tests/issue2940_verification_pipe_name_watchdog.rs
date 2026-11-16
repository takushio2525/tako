//! **#2940 の番犬**: 検証プロセスの Windows の本体の受け口が、本番の固定名 `\\.\pipe\tako-<ユーザー>` にならない。
//!
//! ## 何が起きていたか
//!
//! Windows の `ipc.rs` の `preferred_pipe_name` が一時名（pid 入り）を使うのはセカンダリ・`cfg!(test)`・
//! `TAKO_SELF_TEST` のときだけで、固定名は data dir に依らなかった。検証起動の GUI（`TAKO_ISOLATED=1` と
//! 専用の置き場）が本番の GUI と同じ固定名を取り、隔離の GUI が先に立つと本番の GUI が一時名へ逃げ、
//! 本番の器のペインのシェル（固定名の `TAKO_SOCKET` を持つ）から叩いた CLI が隔離の GUI へ繋がった。
//! パイプ名の名前空間はマシン全体で 1 つなので、unix（data dir ごとのソケット）と違って名前が衝突する。
//!
//! ## 何を固定するか（どの OS の CI でも落ちる）
//!
//! 1. [`検証プロセスの名前はどの組み合わせでも本番の固定名にならない`] — 決め方の純関数
//!    `tako_core::ipc_socket::plan_pipe_name` を、利用者名・data dir・セカンダリ / セルフテストの組み合わせで引き、
//!    検証プロセス（A/B でない）の答えがどの利用者の固定名とも一致しないこと
//! 2. [`名前の決め方は検証プロセスの判定の1実装を引く`] — env を読む入口 `pipe_name_plan_from_env` が
//!    検証プロセスの判定を `tako_core::paths::is_verification_process()` から引き、そのまま決め方へ渡す
//!    （判定を外すと `file:line` で名指す）
//! 3. [`windowsの本体の受け口の名前はtako_coreの決め方だけを通る`] — `ipc.rs` の本番コードは名前を組まず、
//!    `preferred_pipe_name` が入口を通る（修正前の `format!(r"\\.\pipe\tako-…")` が戻ると名指す）
//! 4. [`abの逃げ道は1つのenvに閉じている`] — `TAKO_2940_LEGACY` を読むのは tako-core の 1 か所
//!
//! 実際に受け口を張って CLI が届くことは `crates/tako-cli/tests/issue2940_isolated_pipe_name.rs` が見る。

use std::path::{Path, PathBuf};

use tako_core::ipc_socket::{
    fixed_pipe_name, pipe_user_tag, plan_pipe_name, temp_pipe_name, PipeNameInputs, PipeNamePlan,
};

#[path = "common/production_range.rs"]
mod production_range;

const IPC: &str = "crates/tako-control/src/ipc.rs";
const PLAN_RS: &str = "crates/tako-core/src/ipc_socket.rs";

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("crates/tako-control の 2 つ上がワークスペースルート")
        .to_path_buf()
}

/// 本番コードだけの眺め（テスト領域は空白へ潰す。行番号は保たれる）
fn production(rel: &str) -> Vec<String> {
    let src = std::fs::read_to_string(workspace_root().join(rel))
        .unwrap_or_else(|e| panic!("{rel} を読めない: {e}"));
    production_range::production(&src, rel)
        .lines()
        .map(str::to_string)
        .collect()
}

/// `fn <名前>(` の行から、頭と同じ字下げの `}` までの行番号の範囲（0 始まり・両端を含む）
fn fn_body(lines: &[String], rel: &str, head: &str) -> (usize, usize) {
    let start = lines
        .iter()
        .position(|l| l.contains(head))
        .unwrap_or_else(|| panic!("{rel} に `{head}` が見つからない（番犬の前提が崩れた）"));
    let indent = lines[start].len() - lines[start].trim_start().len();
    let closing = format!("{}}}", " ".repeat(indent));
    let end = lines[start + 1..]
        .iter()
        .position(|l| l.starts_with(&closing))
        .map(|i| start + 1 + i)
        .unwrap_or_else(|| panic!("{rel}:{} の `{head}` の終わりが見つからない", start + 1));
    (start, end)
}

fn is_comment(line: &str) -> bool {
    line.trim_start().starts_with("//")
}

#[test]
fn 検証プロセスの名前はどの組み合わせでも本番の固定名にならない() {
    let raw_users = [
        None,
        Some("winuser"),
        Some(""),
        Some("win.user"),
        Some(r"DOMAIN\winuser"),
        Some("山田"),
        Some("winuser-0123456789abcdef"),
    ];
    let tags: Vec<String> = raw_users.iter().map(|u| pipe_user_tag(*u)).collect();
    let dirs = [
        None,
        Some(Path::new(
            r"C:\Users\winuser\AppData\Local\Temp\tako-iso\data",
        )),
        Some(Path::new("/tmp/tako-iso/data")),
        Some(Path::new("")),
    ];
    let mut checked = 0;
    for tag in &tags {
        for dir in dirs {
            for prefer_temp in [false, true] {
                let plan = plan_pipe_name(PipeNameInputs {
                    user_tag: tag,
                    data_dir: dir,
                    prefer_temp,
                    verification: true,
                    legacy: false,
                });
                let name = match &plan {
                    PipeNamePlan::Fixed(name) => panic!(
                        "検証プロセス（user={tag:?} data_dir={dir:?} prefer_temp={prefer_temp}）が本番の固定名 \
                         {name} になった（#2940 の修正前）。決め方は {PLAN_RS} の plan_pipe_name"
                    ),
                    PipeNamePlan::Temp => continue,
                    PipeNamePlan::Verification(name) => name,
                };
                for other in &tags {
                    assert_ne!(
                        name,
                        &fixed_pipe_name(other),
                        "検証プロセスの名前が利用者 {other:?} の本番の固定名と一致した"
                    );
                    assert_ne!(name, &temp_pipe_name(other, 4242, 0));
                }
                checked += 1;
            }
        }
    }
    assert!(checked > 0, "data dir ごとの名前を 1 つも検査していない");
    // 本番（検証プロセスでない）は今どおり固定名（変えない）
    assert_eq!(
        plan_pipe_name(PipeNameInputs {
            user_tag: "winuser",
            data_dir: dirs[1],
            prefer_temp: false,
            verification: false,
            legacy: false,
        }),
        PipeNamePlan::Fixed(r"\\.\pipe\tako-winuser".into())
    );
}

#[test]
fn 名前の決め方は検証プロセスの判定の1実装を引く() {
    let lines = production(PLAN_RS);
    let (start, end) = fn_body(&lines, PLAN_RS, "pub fn pipe_name_plan_from_env(");
    let body = &lines[start..=end];
    let at = |i: usize| format!("{PLAN_RS}:{}", start + i + 1);
    let judged = body
        .iter()
        .position(|l| l.trim() == "let verification = crate::paths::is_verification_process();");
    let passed = body.iter().position(|l| l.trim() == "verification,");
    assert!(
        judged.is_some(),
        "{} の pipe_name_plan_from_env が検証プロセスの判定を tako_core::paths::is_verification_process() から \
         引いていない（判定を外すと検証の GUI が本番の固定名を取る = #2940）",
        at(0)
    );
    assert!(
        passed.is_some(),
        "{} の pipe_name_plan_from_env が検証プロセスの判定を決め方（PipeNameInputs の verification）へ \
         そのまま渡していない",
        at(0)
    );
    // 判定の後で別の値に差し替えていない
    let rebound: Vec<String> = body
        .iter()
        .enumerate()
        .filter(|(i, l)| {
            Some(*i) != judged
                && !is_comment(l)
                && (l.contains("let verification") || l.contains("verification:"))
        })
        .map(|(i, _)| at(i))
        .collect();
    assert!(
        rebound.is_empty(),
        "検証プロセスの判定を is_verification_process() 以外の値で上書きしている: {rebound:?}"
    );
}

#[test]
fn windowsの本体の受け口の名前はtako_coreの決め方だけを通る() {
    let lines = production(IPC);
    // 名前（`\\.\pipe\…`）を ipc.rs の本番コードで組まない（組むのは tako-core の 1 実装）
    let built: Vec<String> = lines
        .iter()
        .enumerate()
        .filter(|(_, l)| !is_comment(l) && l.contains(r"\pipe\"))
        .map(|(i, _)| format!("{IPC}:{}", i + 1))
        .collect();
    assert!(
        built.is_empty(),
        "ipc.rs の本番コードでパイプ名を組んでいる（固定名を data dir に依らずに取る修正前の形 = #2940）。\
         名前は tako_core::ipc_socket の plan_pipe_name / temp_pipe_name を通す: {built:?}"
    );
    let (start, end) = fn_body(&lines, IPC, "fn preferred_pipe_name(");
    assert!(
        lines[start..=end]
            .iter()
            .any(|l| !is_comment(l) && l.contains("tako_core::ipc_socket::pipe_name_plan_from_env(")),
        "{IPC}:{} の preferred_pipe_name が tako_core::ipc_socket::pipe_name_plan_from_env を通っていない",
        start + 1
    );
    let callers: Vec<String> = lines
        .iter()
        .enumerate()
        .filter(|(_, l)| !is_comment(l) && l.contains("preferred_pipe_name(prefer_temp_socket)"))
        .map(|(i, _)| format!("{IPC}:{}", i + 1))
        .collect();
    assert_eq!(
        callers.len(),
        1,
        "本体の受け口（windows_imp::start）が preferred_pipe_name を通っていない: {callers:?}"
    );
}

#[test]
fn abの逃げ道は1つのenvに閉じている() {
    let readers: Vec<String> = production_range::product_sources()
        .iter()
        .flat_map(|rel| {
            let src = std::fs::read_to_string(workspace_root().join(rel))
                .unwrap_or_else(|e| panic!("{rel} を読めない: {e}"));
            production_range::scan(&src)
                .text
                .lines()
                .enumerate()
                .filter(|(_, l)| l.contains("\"TAKO_2940_LEGACY\""))
                .map(|(i, _)| format!("{rel}:{}", i + 1))
                .collect::<Vec<_>>()
        })
        .collect();
    assert!(
        readers.len() == 1 && readers[0].starts_with(&format!("{PLAN_RS}:")),
        "TAKO_2940_LEGACY を読むのは tako_core::ipc_socket::legacy_2940 だけにする: {readers:?}"
    );
}
