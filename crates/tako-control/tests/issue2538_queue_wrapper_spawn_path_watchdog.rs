//! #2538 の番犬: `tako queue` の包みは待ち明けに子を**素の名前のまま**起こさない
//!
//! # 何が起きていたか
//!
//! 素の名前の spawn（`posix_spawnp`）は PATH の空振り 1 回ごとにカーネルが**親の実行ファイルのまま**の
//! 子を一瞬作る（#2513 / #2519 と同じ仕組み）。install（`build-app.sh --install`）の前から順番待ちしていた
//! 旧版の包みは、install の終わりで枠が空いた瞬間に利用者のコマンド（`cargo`）を素の名前で起こし、
//! 差し替えで消えた自分の実行ファイルのまま作られた空振りの子が 1 本ずつ AppleSystemPolicy に拒否された。
//! 10/30 の本番は 20:30 と 21:23 の 2 回の install とも、install の前から待っていた包み 2 つが
//! 41 本ずつ（包みを起こしたペインの PATH は 42 エントリで cargo が最後）= 82 件。
//!
//! # 何を縛るか
//!
//! 1. 子を起こす `Command::new(` は `fn command`（`Program::command`）の 1 か所だけ。そこでも argv[0] を
//!    素のまま渡さない（待つ前に引いた絶対パスを渡す）
//! 2. `fn run_with` は受付（`request("enqueue"`）より前に `Program::pin(` で子の場所を引く
//! 3. 実際の起動: 待っている間に PATH の手前へ同じ名前の別物が入っても、待ち明けは待つ前に引いた場所を
//!    起こす（素の名前で引き直すと手前の別物が当たる = 終了コードで見分ける）
//! 4. 子の argv[0] は利用者が打った形のまま（`$0` と `ps` の見え方）
//!
//! [`逆戻りを名指しできる`] で、修正前の形を注入した本文が file:line で名指しされることを確かめる。
//! A/B（`TAKO_2519_LEGACY=1`）は [`旧挙動は待ち明けに素の名前で引き直す`] が同じ PATH で再現する。

use std::path::{Path, PathBuf};

#[path = "common/production_range.rs"]
mod production_range;

use production_range::code_view::without_comments;

const WRAPPER: &str = "crates/tako-control/src/queue_wrapper.rs";

/// 子を起こしてよい唯一の関数
const SPAWN_FN: &str = "command";
/// 受付から待ち明けまでを持つ関数
const RUN_FN: &str = "run_with";
/// 待つ前に子の場所を引く呼び出し
const PIN_CALL: &str = "Program::pin(";
/// 受付（ここから待ちが始まる）
const ENQUEUE: &str = "request(\"enqueue\"";

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("crates/tako-control の 2 つ上がワークスペースルート")
        .to_path_buf()
}

/// テスト領域とコメントを空白へ潰した本文（行番号は保たれる）
fn read_raw(rel: &str) -> String {
    let path = workspace_root().join(rel);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{} が読める: {e}", path.display()))
}

fn production(src: &str) -> String {
    without_comments(&production_range::scan(src).text)
}

fn line_of(src: &str, at: usize) -> usize {
    src[..at].matches('\n').count() + 1
}

/// 関数の窓（宣言行の 1-based 行番号と、窓のバイト範囲）。終わりは宣言行と同じ字下げの `}`
fn fn_window(src: &str, name: &str) -> Option<(usize, std::ops::Range<usize>)> {
    let mut offset = 0;
    let mut start: Option<(usize, usize, usize)> = None;
    for (i, line) in src.split_inclusive('\n').enumerate() {
        match start {
            None if tako_core::source_scan::fn_head_name(line) == Some(name) => {
                let indent = line.len() - line.trim_start().len();
                start = Some((i + 1, offset, indent));
            }
            Some((at, begin, indent)) => {
                let body = line.trim_end_matches(['\n', '\r']);
                if body.len() == indent + 1 && body.trim() == "}" {
                    return Some((at, begin..offset + line.len()));
                }
            }
            None => {}
        }
        offset += line.len();
    }
    None
}

/// 1 と 2 の検査（本文を受け取る形 = 注入した本文でも同じ検査を回せる）
fn scan(rel: &str, src: &str) -> Vec<String> {
    let mut out = Vec::new();
    let spawn_fn = fn_window(src, SPAWN_FN);
    if spawn_fn.is_none() {
        out.push(format!(
            "{rel} — `fn {SPAWN_FN}` が見つからない（走査が空振り。名前を変えたらこの番犬も直す）"
        ));
    }
    // 1: `Command::new(` は `fn command` の中だけ・argv[0] を素のまま渡さない
    let call = "Command::new(";
    let mut from = 0;
    while let Some(found) = src[from..].find(call) {
        let at = from + found;
        let line = line_of(src, at);
        let inside = spawn_fn
            .as_ref()
            .is_some_and(|(_, range)| range.contains(&at));
        if !inside {
            out.push(format!(
                "{rel}:{line} — `fn {SPAWN_FN}` の外で `Command::new` が子を起こしている \
                 （待つ前に引いた場所を通らない = 待ち明けに PATH を空振りする）"
            ));
        }
        let arg_start = at + call.len();
        let arg_end = src[arg_start..]
            .find([')', '\n'])
            .map_or(src.len(), |i| arg_start + i);
        if src[arg_start..arg_end].contains("argv") {
            out.push(format!(
                "{rel}:{line} — `Command::new` へ argv[0] を素の名前のまま渡している \
                 （`Program::pin` が待つ前に引いた絶対パスを渡すこと）"
            ));
        }
        from = arg_start;
    }
    // 2: 受付より前に子の場所を引く
    match fn_window(src, RUN_FN) {
        None => out.push(format!(
            "{rel} — `fn {RUN_FN}` が見つからない（走査が空振り。名前を変えたらこの番犬も直す）"
        )),
        Some((head, range)) => {
            let window = &src[range.clone()];
            let pin = window.find(PIN_CALL);
            let enqueue = window.find(ENQUEUE);
            match (pin, enqueue) {
                (None, _) => out.push(format!(
                    "{rel}:{head} — `fn {RUN_FN}` が `{PIN_CALL}` で子の場所を引いていない"
                )),
                (Some(_), None) => out.push(format!(
                    "{rel}:{head} — `fn {RUN_FN}` に受付（`{ENQUEUE}`）が見つからない（走査が空振り）"
                )),
                (Some(p), Some(e)) if p > e => out.push(format!(
                    "{rel}:{} — `{PIN_CALL}` が受付（{rel}:{}）より後ろ \
                     （待っている間に自分の実行ファイルが install で置き換わると、引く前に消えている）",
                    line_of(src, range.start + p),
                    line_of(src, range.start + e)
                )),
                _ => {}
            }
        }
    }
    out
}

/// 1 と 2: 本番の本文に逆戻りが無い
#[test]
fn 包みは待つ前に引いた場所で子を起こす() {
    let hits = scan(WRAPPER, &production(&read_raw(WRAPPER)));
    assert!(
        hits.is_empty(),
        "`tako queue` の包みが待ち明けに子を素の名前のまま起こす形へ戻っている（#2538）。素の名前は起こすたびに \
         PATH の手前のエントリを空振りし、待ちの間の install で包みの実行ファイルが消えた後は空振り 1 回ごとに \
         AppleSystemPolicy の拒否が出る（10/30 の本番で 41 本 × 2）。受付の前に `Program::pin` で引き、\
         `Program::command` から起こすこと:\n  {}",
        hits.join("\n  ")
    );
}

/// 番犬そのものの検査: 修正前の形を注入した本文が file:line で名指しされる
#[test]
fn 逆戻りを名指しできる() {
    let raw = read_raw(WRAPPER);
    let src = production(&raw);

    // 待ち明けの起動を修正前の `Command::new(&argv[0])` へ戻す
    let fixed = "let mut cmd = program.command();";
    let at = src
        .find(fixed)
        .expect("注入の目印が本文に無い（番犬の注入を追従させること）");
    let want = format!("{WRAPPER}:{} —", line_of(&src, at));
    let hits = scan(
        WRAPPER,
        &src.replacen(fixed, "let mut cmd = Command::new(&argv[0]);", 1),
    );
    assert!(
        hits.iter()
            .any(|h| h.starts_with(&want) && h.contains("の外で `Command::new`")),
        "待ち明けの起動を修正前へ戻しても {want} で名指しされない: {hits:?}"
    );
    assert!(
        hits.iter()
            .any(|h| h.starts_with(&want) && h.contains("argv[0] を素の名前のまま")),
        "argv[0] を素のまま渡しても {want} で名指しされない: {hits:?}"
    );
    assert!(
        raw.lines()
            .nth(line_of(&src, at) - 1)
            .is_some_and(|l| l.contains(fixed)),
        "名指しした行が原文の起動の行と違う"
    );

    // `fn command` の中で argv[0] を素のまま渡す
    let fixed = "let mut cmd = Command::new(program);";
    let at = src.find(fixed).expect("注入の目印が fn command に無い");
    let want = format!("{WRAPPER}:{} —", line_of(&src, at));
    let hits = scan(
        WRAPPER,
        &src.replacen(fixed, "let mut cmd = Command::new(&self.argv[0]);", 1),
    );
    assert!(
        hits.iter()
            .any(|h| h.starts_with(&want) && h.contains("argv[0] を素の名前のまま")),
        "fn command で argv[0] を素のまま渡しても {want} で名指しされない: {hits:?}"
    );

    // 引くのを待ちの後ろ（待ち明けの起動の直前）へ動かす
    let pin = "let program = Program::pin(argv);";
    let marker = "// ---- 走らせる ----";
    assert!(src.contains(pin), "注入の目印が fn run_with に無い");
    // コメントは潰れているので、待ち明けの目印は原文で位置を取ってから同じ位置へ差す
    let run_at = raw.find(marker).expect("待ち明けの目印が原文に在る");
    let mut injected = src.replacen(pin, &" ".repeat(pin.len()), 1);
    injected.insert_str(run_at, pin);
    let hits = scan(WRAPPER, &injected);
    assert!(
        hits.iter()
            .any(|h| h.contains("が受付") && h.contains("より後ろ")),
        "引くのを待ちの後ろへ動かしても名指しされない: {hits:?}"
    );
}

/// 空振りする 2 つの dir の後ろに、終了コード 3 で終わる偽のコマンドを置いた PATH
#[cfg(unix)]
struct Fixture {
    _dir: tako_core::test_residue::ScratchDir,
    miss: [PathBuf; 2],
    bin: PathBuf,
}

#[cfg(unix)]
const TOOL: &str = "tako-2538-tool";

#[cfg(unix)]
fn fixture(tag: &str) -> Fixture {
    let dir = tako_core::test_residue::ScratchDir::new(tag);
    let miss = [dir.path().join("miss-0"), dir.path().join("miss-1")];
    let bin = dir.path().join("bin");
    for d in miss.iter().chain([&bin]) {
        std::fs::create_dir_all(d).expect("一時 dir");
    }
    tako_core::fake_exec::write_script(&bin.join(TOOL), "#!/bin/sh\nexit 3\n");
    Fixture {
        _dir: dir,
        miss,
        bin,
    }
}

/// 子の env を揃える（run_alone! の子には他のテストが居ない）。`/bin:/usr/bin` は argv[0] の検査の `sh` 用
#[cfg(unix)]
fn isolate_env(fx: &Fixture) {
    tako_core::test_child::assert_alone("isolate_env");
    let mut entries: Vec<PathBuf> = fx.miss.to_vec();
    entries.push(fx.bin.clone());
    entries.push("/bin".into());
    entries.push("/usr/bin".into());
    std::env::set_var("PATH", std::env::join_paths(entries).expect("PATH"));
    // このテスト自体が `tako queue cargo test` の中で回っても「入れ子 = 待たない」へ倒れない
    std::env::remove_var(tako_control::queue::ENV_JOB);
    std::env::remove_var(tako_core::platform::exe::ISSUE2519_LEGACY_ENV);
}

/// 調停役の役: 受付は待たせ、最初の poll で `during_wait` を走らせてから割り当てる
#[cfg(unix)]
fn run_across_wait(argv: &[&str], during_wait: impl FnOnce()) -> i32 {
    use serde_json::json;
    use tako_control::protocol::Request;
    use tako_control::queue_wrapper::{run, Options, SendError};

    let argv: Vec<String> = argv.iter().map(|s| s.to_string()).collect();
    let mut during_wait = Some(during_wait);
    let mut send = |req: Request| -> Result<serde_json::Value, SendError> {
        let Request::Queue { action, .. } = &req else {
            panic!("順番待ち以外の要求")
        };
        Ok(match action.as_deref() {
            Some("enqueue") => {
                json!({ "state": "waiting", "position": 1, "waiting": 1, "running": 1, "max_jobs": 1 })
            }
            Some("poll") => {
                if let Some(f) = during_wait.take() {
                    f();
                }
                json!({ "state": "granted", "share": 0 })
            }
            Some("done") => json!({ "state": "done" }),
            _ => json!({ "state": "running", "cancel": false }),
        })
    };
    run(&argv, &Options::default(), &mut send)
}

/// 3: 待っている間に PATH の手前へ同じ名前の別物（終了コード 4）が入っても、待ち明けは待つ前に引いた
/// 場所（終了コード 3）を起こす = 待ち明けに PATH を引き直していない（空振りが無い）
#[cfg(unix)]
#[test]
fn 待ち明けは待つ前に引いた場所を起こす() {
    if tako_core::run_alone!(待ち明けは待つ前に引いた場所を起こす) {
        return;
    }
    let fx = fixture("2538-pin");
    isolate_env(&fx);
    let shadow = fx.miss[0].join(TOOL);
    let code = run_across_wait(&[TOOL], || {
        tako_core::fake_exec::write_script(&shadow, "#!/bin/sh\nexit 4\n");
    });
    assert_eq!(
        code,
        3,
        "{WRAPPER}: 待ち明けに素の名前で PATH を引き直した（#2538。手前に入った別物 = 4 が当たった）。\
         待ちの間の install で包みの実行ファイルが消えると、この引き直しの空振り 1 回ごとに \
         AppleSystemPolicy の拒否が出る。受付の前に `Program::pin` で引いた場所を起こすこと"
    );
}

/// A/B（`TAKO_2519_LEGACY=1`）が効くこと = 修正前の起こし方（待ち明けに素の名前で引き直す）を同じ PATH で
/// 再現できる。4 が返ること自体が、3 の観測（手前に入った別物）が生きている証拠を兼ねる
#[cfg(unix)]
#[test]
fn 旧挙動は待ち明けに素の名前で引き直す() {
    if tako_core::run_alone!(旧挙動は待ち明けに素の名前で引き直す) {
        return;
    }
    let fx = fixture("2538-legacy");
    isolate_env(&fx);
    std::env::set_var(tako_core::platform::exe::ISSUE2519_LEGACY_ENV, "1");
    let shadow = fx.miss[0].join(TOOL);
    let code = run_across_wait(&[TOOL], || {
        tako_core::fake_exec::write_script(&shadow, "#!/bin/sh\nexit 4\n");
    });
    assert_eq!(code, 4, "旧腕で待ち明けに PATH を引き直していない");
}

/// 待つ前に引いた場所が待ち明けに消えていたら、素の名前で起こす（= 修正前の起こし方。後ろへ移った版が当たる）
#[cfg(unix)]
#[test]
fn 引いた場所が消えていれば素の名前で起こす() {
    if tako_core::run_alone!(引いた場所が消えていれば素の名前で起こす) {
        return;
    }
    let fx = fixture("2538-gone");
    isolate_env(&fx);
    let pinned = fx.bin.join(TOOL);
    let moved = fx.miss[1].join(TOOL);
    let code = run_across_wait(&[TOOL], || {
        std::fs::remove_file(&pinned).expect("引いた場所を消す");
        tako_core::fake_exec::write_script(&moved, "#!/bin/sh\nexit 5\n");
    });
    assert_eq!(code, 5, "消えた場所を起こそうとして素の名前へ戻っていない");
}

/// 4: 子の argv[0] は利用者が打った形のまま（`/bin/sh` は argv[0] を `$0` と `ps` へそのまま出す）。
/// PATH に無い名前は修正前と同じく 127
#[cfg(unix)]
#[test]
fn 子のargv0は打った形のまま() {
    if tako_core::run_alone!(子のargv0は打った形のまま) {
        return;
    }
    let fx = fixture("2538-argv0");
    isolate_env(&fx);
    let out = fx.bin.join("argv0.txt");
    let script = format!(
        "echo \"$0\" > '{0}'; /bin/ps -o command= -p $$ >> '{0}'",
        out.display()
    );
    assert_eq!(run_across_wait(&["sh", "-c", &script], || {}), 0);
    let seen = std::fs::read_to_string(&out).expect("子が argv[0] を書いた");
    let mut lines = seen.lines();
    assert_eq!(
        lines.next(),
        Some("sh"),
        "子の $0 が打った形（sh）でない（絶対パスへ引いた場所が argv[0] へ漏れている）: {seen:?}"
    );
    assert!(
        lines.next().is_some_and(|l| l.starts_with("sh -c ")),
        "ps の見え方が打った形（sh -c …）でない: {seen:?}"
    );

    assert_eq!(
        run_across_wait(&["tako-2538-absent"], || {}),
        tako_core::queue::EXIT_SPAWN_FAILED,
        "PATH に無い名前の終了コードが変わった"
    );
}
