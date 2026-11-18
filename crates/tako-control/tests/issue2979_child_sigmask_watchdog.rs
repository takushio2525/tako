//! **#2979 の番犬**: 待つ子を、起こしたスレッドのシグナルマスクを受け継いだまま起こす形へ戻さない。
//!
//! ## 何が起きていたのか
//!
//! 子のシグナルマスクは起こしたスレッドから受け継がれる（Rust 1.95 の std は posix_spawn 経路も fork 経路も
//! 戻さない）。GUI の背景の段（GPUI の macOS の dispatcher = GCD のスレッド）は TERM / INT / HUP / QUIT /
//! CHLD ほか 20 種をブロックしているので、そこから起こした子（ログインシェルの問い合わせ・`ssh -O check`・
//! `OffloadJob` の外部 CLI）は、上限で打ち切るときの SIGTERM（#2633）を受けられず、猶予の後の SIGKILL で
//! trap なしに消えていた。#2633 が守った「rc の途中の処理（`pyenv rehash` のロック）が EXIT の trap で
//! 後片付けを終える」は、UI スレッドから起こした子にしか効いていなかった（#2976 で生死の判定を背景へ出して
//! 実測。修正前の UI スレッドの経路は trap が走り、背景の経路は走らない）。
//!
//! いまは待つ子を起こす 1 実装 `probe::spawn_for_wait` が、境界
//! `tako_core::platform::process::spawn_with_default_sigmask` の中で `KillScope::spawn` を呼ぶ。境界は
//! `spawn` の間だけ起こすスレッドのマスクを空にし（posix_spawn のまま）、ガードの drop で元へ戻す。
//!
//! ## ここで止めるもの
//!
//! 1. [`待つ子は既定のマスクで起こす`] — file:line で拘束する:
//!    - probe.rs の `fn spawn_for_wait` は `KillScope::spawn(` を境界の
//!      `spawn_with_default_sigmask(issue2979_legacy, …)` の中で呼ぶ
//!    - 境界の `fn spawn_with_default_sigmask` は、A/B の腕（`if issue2979_legacy() {`）の外で
//!      `spawn()` の前にガード `let _restore = SigmaskRestore::unblock_all();` を持つ
//!      （`let _ = …` はその場で drop = マスクを外さないのと同じ）
//!    - ガードの `impl Drop for SigmaskRestore` は外す前のマスクへ戻す（戻さないと GCD のスレッドの
//!      マスクが空のまま残り、GCD の仕事が以後シグナルを受ける）
//!    - `lead_own_group(` を呼ぶ製品の関数は `spawn_for_wait(` で起こす（境界を通らずに `.spawn()` /
//!      `.output()` / `.status()` で起こさない = 打ち切りの TERM を受ける子がマスクを受け継ぐ）
//!    - A/B のゲート（`TAKO_2979_LEGACY`）は製品コードに 1 か所
//! 2. [`修正前の形の注入を名指す`] — 1 の検査が、上の形を外した注入を file:line で名指すこと
//! 3. 実プロセス（unix）: GCD と同じ組をブロックしたスレッド（背景の段の代役）から #2633 の rehash の
//!    打ち切りを走らせ、修正後は trap が走ってロックが消える・A/B の旧経路（`TAKO_2979_LEGACY=1`）は
//!    trap が走らずロックが残る。マスクが空のスレッド（UI スレッドの代役）からは修正前後とも trap が走る
//!
//! 判定は状態（印のファイル・ロックの有無・プロセスの有無）で書く。所要は見ない。
//! Windows は対象外（シグナルマスクが無い。境界の unix 以外の腕はそのまま `spawn` を呼ぶ）。
//!
//! ## 相方
//!
//! 境界の単体テスト（`platform::process::tests::issue2979_*`）と、実経路
//! `bash scripts/test-child-sigmask-2979.sh`（隔離 GUI の背景の段から起こした返らない `ssh -O check` の
//! trap に打ち切りの TERM が届くか。修正前は届かない）。

use std::path::{Path, PathBuf};

#[path = "common/production_range.rs"]
mod production_range;

#[path = "common/call_graph.rs"]
mod call_graph;

const PROBE: &str = "crates/tako-core/src/probe.rs";
/// 子を起こすときの OS 差の境界（B5。#2860）
const PROCESS: &str = "crates/tako-core/src/platform/process.rs";
/// 製品コードを持つ 4 クレート（A/B のゲートの数を数える範囲）
const ROOTS: &[&str] = &[
    "crates/tako-core/src",
    "crates/tako-control/src",
    "crates/tako-cli/src",
    "crates/tako-app/src",
];

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("crates/tako-control の 2 つ上がワークスペースルート")
        .to_path_buf()
}

fn read_rel(rel: &str) -> String {
    std::fs::read_to_string(workspace_root().join(rel))
        .unwrap_or_else(|e| panic!("{rel} が読める（消した / 改名したなら番犬も追うこと）: {e}"))
}

/// テスト領域を空白へ潰した本文（行番号・バイト位置は原文と同じ）
fn product_text(raw: &str) -> String {
    production_range::scan(raw).text
}

/// 検査の材料: probe.rs・境界・`lead_own_group(` を呼ぶ製品のファイル（相対パスと原文）
fn inputs() -> Vec<(String, String)> {
    let mut files: Vec<(String, String)> = Vec::new();
    for rel in production_range::product_sources() {
        let raw = read_rel(&rel);
        if rel == PROBE || rel == PROCESS || raw.contains("lead_own_group(") {
            files.push((rel, raw));
        }
    }
    for must in [PROBE, PROCESS] {
        assert!(
            files.iter().any(|(rel, _)| rel == must),
            "{must} が製品のファイルの列挙に無い（改名したなら番犬も追うこと）"
        );
    }
    files
}

/// `files` のうち `rel` の原文だけを差し替えた写し（注入用）
fn with_replaced(
    files: &[(String, String)],
    rel: &str,
    from: &str,
    to: &str,
) -> Vec<(String, String)> {
    let mut hit = false;
    let out = files
        .iter()
        .map(|(r, raw)| {
            if r == rel {
                assert!(
                    raw.contains(from),
                    "注入の起点が {rel} に無い（形を変えたなら注入も追うこと）: {from}"
                );
                hit = true;
                (r.clone(), raw.replacen(from, to, 1))
            } else {
                (r.clone(), raw.clone())
            }
        })
        .collect();
    assert!(hit, "{rel} が材料に無い");
    out
}

/// `fn <name>` の最初の定義の製品の経路の範囲（A/B の旧経路の腕と入れ子の関数を除く）と頭の行
fn product_view(
    graph: &call_graph::Graph,
    rel: &str,
    name: &str,
) -> Result<(usize, Vec<(usize, usize)>), String> {
    let f = graph
        .find(rel, name)
        .into_iter()
        .min_by_key(|&f| graph.fns[f].line)
        .ok_or_else(|| format!("{rel} に `fn {name}` が無い（改名したなら番犬も追うこと）"))?;
    Ok((f, graph.product_ranges(f)))
}

/// 製品の経路で `needle` が最初に現れる位置（原文のバイト位置）
fn first_in(code: &str, ranges: &[(usize, usize)], needle: &str) -> Option<usize> {
    ranges
        .iter()
        .find_map(|&(s, e)| code[s..e].find(needle).map(|i| s + i))
}

/// 1 の本体: 材料（注入で差し替えられる）から、修正前の形へ戻った箇所を file:line で並べる
fn offenders(files: &[(String, String)]) -> Vec<String> {
    let graph = call_graph::Graph::build(
        files
            .iter()
            .map(|(rel, raw)| call_graph::Source::new(rel, &product_text(raw)))
            .collect(),
    );
    let mut out = Vec::new();
    let src_of = |rel: &str| {
        graph
            .sources
            .iter()
            .find(|s| s.rel == rel)
            .unwrap_or_else(|| panic!("{rel} が材料に無い"))
    };

    // (a) spawn_for_wait は KillScope::spawn を境界のマスクの付け外しの中で呼ぶ
    match product_view(&graph, PROBE, "spawn_for_wait") {
        Err(e) => out.push(e),
        Ok((f, ranges)) => {
            let src = src_of(PROBE);
            let head = graph.fns[f].line;
            let wrap = first_in(
                &src.code,
                &ranges,
                "spawn_with_default_sigmask(issue2979_legacy",
            );
            match first_in(&src.code, &ranges, "KillScope::spawn(") {
                None => out.push(format!(
                    "{PROBE}:{head}（fn spawn_for_wait）が `KillScope::spawn(` で子を起こしていない（起こし方を変えたなら番犬も追うこと）"
                )),
                Some(at) if wrap.is_none_or(|w| w > at) => out.push(format!(
                    "{PROBE}:{}（fn spawn_for_wait）が境界の `spawn_with_default_sigmask(issue2979_legacy, …)` を通さずに子を起こす = 背景の段（GCD のスレッド）から起こした子が TERM / INT / HUP をブロックしたまま動く（#2979）",
                    src.line_of(at)
                )),
                Some(_) => {}
            }
        }
    }

    // (b) 境界はガードを spawn の前に持ち、drop で元のマスクへ戻す
    match product_view(&graph, PROCESS, "spawn_with_default_sigmask") {
        Err(e) => out.push(e),
        Ok((f, ranges)) => {
            let src = src_of(PROCESS);
            let head = graph.fns[f].line;
            let guard = first_in(
                &src.code,
                &ranges,
                "let _restore = SigmaskRestore::unblock_all()",
            );
            let call = first_in(&src.code, &ranges, "spawn()");
            match (guard, call) {
                (Some(g), Some(c)) if g < c => {}
                (_, Some(c)) => out.push(format!(
                    "{PROCESS}:{}（fn spawn_with_default_sigmask）が A/B の腕の外で、マスクを外すガード（`let _restore = SigmaskRestore::unblock_all();`）を持たずに `spawn()` を呼ぶ = 子が起こしたスレッドのマスクを受け継ぐ（#2979。`let _ = …` はその場で戻す）",
                    src.line_of(c)
                )),
                (_, None) => out.push(format!(
                    "{PROCESS}:{head}（fn spawn_with_default_sigmask）が A/B の腕の外で `spawn()` を呼んでいない（形を変えたなら番犬も追うこと）"
                )),
            }
        }
    }
    {
        let src = src_of(PROCESS);
        // `impl Drop for SigmaskRestore` の中の drop（ほかの型の drop と取り違えない）
        let restore_drop = graph.find(PROCESS, "drop").into_iter().find(|&f| {
            let head = graph.fns[f].head;
            src.code[..head]
                .rfind("impl ")
                .is_some_and(|i| src.code[i..head].starts_with("impl Drop for SigmaskRestore"))
        });
        match restore_drop {
            None => out.push(format!(
                "{PROCESS}: `impl Drop for SigmaskRestore` の `fn drop` が無い = 外したマスクを戻さない（GCD のスレッドのマスクが空のまま残る。#2979）"
            )),
            Some(f) => {
                let ranges = graph.product_ranges(f);
                if first_in(&src.code, &ranges, "pthread_sigmask(libc::SIG_SETMASK, &self.0")
                    .is_none()
                {
                    out.push(format!(
                        "{PROCESS}:{}（impl Drop for SigmaskRestore の fn drop）が外す前のマスク（`self.0`）へ戻していない = GCD のスレッドのマスクが空のまま残り、以後の GCD の仕事がシグナルを受ける（#2979）",
                        graph.fns[f].line
                    ));
                }
            }
        }
    }

    // (c) lead_own_group で起こす子（打ち切りで TERM を受ける子）は spawn_for_wait で起こす
    for (fi, def) in graph.fns.iter().enumerate() {
        let src = &graph.sources[def.file];
        if src.rel == PROCESS && def.name == "lead_own_group" {
            continue;
        }
        let ranges = graph.product_ranges(fi);
        let Some(lead) = first_in(&src.code, &ranges, "lead_own_group(") else {
            continue;
        };
        let after: Vec<(usize, usize)> = ranges
            .iter()
            .filter(|&&(_, e)| e > lead)
            .map(|&(s, e)| (s.max(lead), e))
            .collect();
        let bare = [".spawn()", ".output()", ".status()"]
            .iter()
            .filter_map(|n| first_in(&src.code, &after, n).map(|i| (i, *n)))
            .min();
        let via = first_in(&src.code, &after, "spawn_for_wait(");
        if let Some((at, how)) = bare.filter(|&(at, _)| via.is_none_or(|v| v > at)) {
            out.push(format!(
                "{}:{}（fn {}）が自分のグループで起こす子（`lead_own_group`）を境界を通らずに `{how}` で起こす = 打ち切りの TERM（#2633）を受ける子が起こしたスレッドのマスクを受け継ぐ。`probe::spawn_for_wait` で起こすこと（#2979）",
                src.rel,
                src.line_of(at),
                def.name
            ));
        } else if via.is_none() {
            out.push(format!(
                "{}:{}（fn {}）が `lead_own_group` の後に `spawn_for_wait(` で子を起こしていない（#2979。起こし方を変えたなら番犬も追うこと）",
                src.rel,
                src.line_of(lead),
                def.name
            ));
        }
    }
    out
}

#[test]
fn 待つ子は既定のマスクで起こす() {
    let hits = offenders(&inputs());
    assert!(hits.is_empty(), "{}", hits.join("\n"));
}

#[test]
fn ab_のゲートは1か所() {
    let mut found = Vec::new();
    for rel in production_range::product_sources() {
        if !ROOTS.iter().any(|r| rel.starts_with(r)) {
            continue;
        }
        // env を読む形だけを数える（コメント・doc の言及は数えない）
        for (i, line) in product_text(&read_rel(&rel)).lines().enumerate() {
            if line.contains("std::env::var(\"TAKO_2979_LEGACY\")") {
                found.push(format!("{rel}:{}", i + 1));
            }
        }
    }
    assert_eq!(
        found.len(),
        1,
        "A/B のゲート `TAKO_2979_LEGACY` は製品コードに 1 か所（probe::issue2979_legacy）: {found:?}"
    );
    assert!(
        found[0].starts_with(PROBE),
        "ゲートは probe.rs に置く: {found:?}"
    );
}

#[test]
fn 修正前の形の注入を名指す() {
    let files = inputs();
    let cases: &[(&str, &str, &str, &str)] = &[
        (
            PROBE,
            "spawn_for_wait が境界を通らずに起こす",
            "crate::platform::process::spawn_with_default_sigmask(issue2979_legacy, || {",
            "crate::platform::process::spawn_with_default_sigmask(|| true, || {",
        ),
        (
            PROCESS,
            "ガードをその場で drop する",
            "let _restore = SigmaskRestore::unblock_all();",
            "let _ = SigmaskRestore::unblock_all();",
        ),
        (
            PROCESS,
            "drop がマスクを戻さない",
            "libc::pthread_sigmask(libc::SIG_SETMASK, &self.0, std::ptr::null_mut());",
            "let _ = &self.0;",
        ),
        (
            PROBE,
            "lead_own_group の子を素の spawn で起こす",
            "    crate::platform::process::lead_own_group(command);\n    command\n        .stdin(Stdio::null())\n        .stdout(Stdio::piped())\n        .stderr(Stdio::piped());\n    match spawn_for_wait(command) {",
            "    crate::platform::process::lead_own_group(command);\n    command\n        .stdin(Stdio::null())\n        .stdout(Stdio::piped())\n        .stderr(Stdio::piped());\n    match command.spawn().map(Spawned::from) {",
        ),
    ];
    for &(rel, what, from, to) in cases {
        let injected = with_replaced(&files, rel, from, to);
        let hits = offenders(&injected);
        assert!(
            hits.iter().any(|h| h.starts_with(&format!("{rel}:"))),
            "注入（{what}）を {rel} の file:line で名指せない: {hits:?}"
        );
    }
    // 今の形は名指さない
    assert!(offenders(&files).is_empty());
}

// ---- 3: 実プロセス ---------------------------------------------------------------------------------

#[cfg(unix)]
mod live {
    use std::path::Path;
    use std::process::Command;
    use std::time::{Duration, Instant};
    use tako_core::probe::{group_output_with_timeout, Outcome};
    use tako_core::test_residue::ScratchDir;

    /// 印が現れる / 消えるのを待つ外側の締め切り（「残っている」を判定するための上限。所要の assert ではない）
    const OUTER: Duration = Duration::from_secs(15);

    /// GCD のワーカースレッドがブロックしているシグナル（macOS の `dispatch_async` の上で読んだ組から、
    /// macOS にしか無い SIGINFO を外したもの）
    const GCD_BLOCKED: [libc::c_int; 19] = [
        libc::SIGHUP,
        libc::SIGINT,
        libc::SIGQUIT,
        libc::SIGABRT,
        libc::SIGALRM,
        libc::SIGTERM,
        libc::SIGURG,
        libc::SIGTSTP,
        libc::SIGCONT,
        libc::SIGCHLD,
        libc::SIGTTIN,
        libc::SIGTTOU,
        libc::SIGIO,
        libc::SIGXCPU,
        libc::SIGXFSZ,
        libc::SIGVTALRM,
        libc::SIGWINCH,
        libc::SIGUSR1,
        libc::SIGUSR2,
    ];

    /// マスクを `sigs` の組にした新しいスレッドで `f` を走らせる（背景の段 / UI スレッドの代役）
    pub fn on_thread_with_mask<T: Send + 'static>(
        sigs: &'static [libc::c_int],
        f: impl FnOnce() -> T + Send + 'static,
    ) -> T {
        std::thread::spawn(move || {
            // SAFETY: このスレッドのマスクを書くだけ
            unsafe {
                let mut set: libc::sigset_t = std::mem::zeroed();
                libc::sigemptyset(&mut set);
                for &sig in sigs {
                    libc::sigaddset(&mut set, sig);
                }
                libc::pthread_sigmask(libc::SIG_SETMASK, &set, std::ptr::null_mut());
            }
            f()
        })
        .join()
        .expect("代役のスレッドが panic しない")
    }

    pub fn gcd_like() -> &'static [libc::c_int] {
        &GCD_BLOCKED
    }

    /// pyenv の rehash の取り方を写した偽物（EXIT の trap を先に置き、noclobber でロックを作って眠る。
    /// #2633 の番犬と同じ形）
    fn fake_rehash(dir: &Path) -> std::path::PathBuf {
        let path = dir.join("pyenv-rehash");
        tako_core::fake_exec::write_script(
            &path,
            "#!/bin/bash\n\
             LOCK=\"$1\"; MARK=\"$2\"\n\
             trap 'echo trap-enter >> \"$MARK\"; rm -f \"$LOCK\" && echo trap-rm-ok >> \"$MARK\"' EXIT\n\
             ( set -o noclobber; : > \"$LOCK\" ) 2>/dev/null || exit 1\n\
             echo $$ > \"$MARK.pid\"\n\
             echo locked >> \"$MARK\"\n\
             sleep 30\n",
        );
        path
    }

    fn wait_until(what: &str, mut cond: impl FnMut() -> bool) {
        let deadline = Instant::now() + OUTER;
        while !cond() {
            assert!(Instant::now() < deadline, "{OUTER:?} たっても {what}");
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    fn read(path: &Path) -> String {
        std::fs::read_to_string(path).unwrap_or_default()
    }

    /// 1 回分: 偽の rehash がロックを持つ間に打ち切る（呼んだスレッドから子を起こす）。返り値は (印, ロックが在るか)
    pub fn rehash_case(tag: &str) -> (String, bool) {
        let dir = ScratchDir::new(tag);
        let rehash = fake_rehash(dir.path());
        let lock = dir.path().join(".pyenv-shim");
        let mark = dir.path().join("mark");
        // ログインシェルの代役（非対話の sh = rc の子は同じグループ）。rehash の後にも行がある形
        let body = format!(
            "'{}' '{}' '{}'; echo after",
            rehash.display(),
            lock.display(),
            mark.display()
        );
        let mut cmd = Command::new("/bin/sh");
        cmd.arg("-c").arg(&body);
        // 上限は rehash がロックを取るのに十分な長さ（取る前に打ち切ると何も確かめられない）
        let got = group_output_with_timeout(&mut cmd, "sh -c (issue2979)", Duration::from_secs(3));
        assert!(
            matches!(got, Outcome::TimedOut { .. }),
            "上限で打ち切る: {got:?}"
        );
        assert!(
            read(&mark).contains("locked"),
            "打ち切りより前に rehash がロックを取っていない（負荷で 3 秒に間に合わなかった）: {:?}",
            read(&mark)
        );
        let pid: u32 = read(&mark.with_extension("pid"))
            .trim()
            .parse()
            .expect("rehash の pid");
        wait_until(
            "rehash のプロセスが残っている（猶予の後の SIGKILL も届かない）",
            || !tako_core::platform::process::pid_alive(pid),
        );
        (read(&mark), lock.exists())
    }
}

/// 3-1: 背景の段の代役（GCD と同じ組をブロックしたスレッド）から打ち切っても、TERM で rc の EXIT の trap が走る
#[cfg(unix)]
#[test]
fn 背景の段の代役から打ち切ってもtermで後片付けが走る() {
    let (mark, lock_left) =
        live::on_thread_with_mask(live::gcd_like(), || live::rehash_case("issue2979-bg"));
    assert!(
        mark.contains("trap-enter") && mark.contains("trap-rm-ok"),
        "子は既定のマスクで起き、打ち切りの SIGTERM で EXIT の trap が最後まで走る: {mark:?}"
    );
    assert!(!lock_left, "打ち切りの後にロックが残っていない: {mark:?}");
}

/// 3-2: A/B の旧経路（`TAKO_2979_LEGACY=1`）は修正前の形 = 子がマスクを受け継ぎ、trap が走らずロックが残る
#[cfg(unix)]
#[test]
fn 旧経路では背景の段の代役から打ち切るとロックが残る() {
    if tako_core::run_alone!(旧経路では背景の段の代役から打ち切るとロックが残る)
    {
        return;
    }
    // 子の中だけ（他のテストは居ない。#2400）。ゲートは最初に読んだ値を持つので、呼ぶ前に立てる
    std::env::set_var("TAKO_2979_LEGACY", "1");
    let (mark, lock_left) = live::on_thread_with_mask(live::gcd_like(), || {
        live::rehash_case("issue2979-bg-legacy")
    });
    assert!(
        !mark.contains("trap-enter"),
        "旧経路は子が TERM をブロックしたまま = 猶予の後の SIGKILL で消え、trap は走らない: {mark:?}"
    );
    assert!(
        lock_left,
        "旧経路ではロックが残る（#2979 の再現）: {mark:?}"
    );
}

/// 3-3: UI スレッドの代役（マスクが空のスレッド）からは、修正後も trap が走る
#[cfg(unix)]
#[test]
fn 空のマスクのスレッドから打ち切るとtermで後片付けが走る() {
    let (mark, lock_left) = live::on_thread_with_mask(&[], || live::rehash_case("issue2979-ui"));
    assert!(
        mark.contains("trap-enter") && mark.contains("trap-rm-ok"),
        "{mark:?}"
    );
    assert!(!lock_left, "{mark:?}");
}

/// 3-4: UI スレッドの代役からは、A/B の旧経路でも trap が走る（UI スレッドから起こす子は修正前後で同じ）
#[cfg(unix)]
#[test]
fn 旧経路でも空のマスクのスレッドから打ち切るとtermで後片付けが走る() {
    if tako_core::run_alone!(旧経路でも空のマスクのスレッドから打ち切るとtermで後片付けが走る)
    {
        return;
    }
    std::env::set_var("TAKO_2979_LEGACY", "1");
    let (mark, lock_left) =
        live::on_thread_with_mask(&[], || live::rehash_case("issue2979-ui-legacy"));
    assert!(
        mark.contains("trap-enter") && mark.contains("trap-rm-ok"),
        "{mark:?}"
    );
    assert!(!lock_left, "{mark:?}");
}
