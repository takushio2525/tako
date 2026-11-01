//! **#2633 の番犬**: 打ち切ったログインシェルのグループを、いきなり SIGKILL で畳む形へ戻さない。
//!
//! ## 何が起きていたのか
//!
//! ログインシェルへの問い合わせ（`platform::exe::ask_login_shell` → `probe::group_output_with_timeout`）は、
//! 上限（既定 5 秒）で打ち切るとグループごと SIGKILL を送っていた。rc（`.zprofile` など）の途中の処理は
//! 後片付けを走らせられず、`pyenv rehash` がロック（`<pyenv root>/shims/.pyenv-shim`）を持つ間に当たると
//! EXIT の trap が走らずにロックが残る（段 1 の実測で 5/5）。以後のログインシェルの rehash は残ったロックを
//! 60 秒待つので、上限 5 秒の tako の問い合わせは自分では片付けられず、残っている間の問い合わせが全部
//! 打ち切りになる（同じ check-health の後続 5 本が全部「5 秒で応答なし」）。
//!
//! いまはグループへ **SIGTERM を 1 回だけ**送って呼び手へはその場で返し、刈り取り係
//! （`probe::reap_group_in_background`）が猶予（`probe::GROUP_TERM_GRACE`）の間グループを見張って、
//! 残っていれば SIGKILL で畳む。頭（直接の子）は判定が済むまで刈り取らない（グループの番号を使い回させない）。
//!
//! ## ここで止めるもの
//!
//! 1. [`打ち切りの畳み方はterm_猶予_killの形`] — `crates/tako-core/src/probe.rs` の形を file:line で拘束する:
//!    `kill_on_timeout` は A/B の腕（`if issue2633_legacy() {`）の外でグループへ SIGTERM を送り SIGKILL を
//!    送らない・刈り取り係は猶予の後にだけ SIGKILL を送り、それより前に頭を刈り取らない・待ちの 1 実装が
//!    TERM を送ったグループを刈り取り係へ渡す・A/B のゲートは製品コードに 1 か所
//! 2. [`修正前の形の注入を名指す`] — 1 の検査が修正前の形（グループへの即 SIGKILL・猶予の前の刈り取り・
//!    刈り取り係を通さない）の注入を file:line で名指すこと
//! 3. 実プロセス（unix）: [`termで後片付けが走りロックが消える`] / [`termを無視する子は猶予の後に消える`] /
//!    [`止まった子も猶予の後に消える`] / [`legacyではロックが残る`]（A/B の旧経路 = 修正前の再現）
//!
//! 判定は状態（印のファイル・ロックの有無・プロセスの有無）で書く。所要は報告だけ
//! （`.agent/conventions.md`「効果を測る単体テストは実時間で比べない」）。
//!
//! ## 相方
//!
//! 実経路（砂場の HOME の偽の pyenv で、製品の CLI の `tako check-health` の問い合わせを打ち切る）は
//! `bash scripts/measure-shutdown-hup-2633.sh`（`ONLY=c`。修正前 5/5 でロックが残る → 修正後 0/5）。
//! 上限で打ち切ること自体は `issue1533_login_shell_timeout_watchdog`。

use std::path::{Path, PathBuf};

#[path = "common/production_range.rs"]
mod production_range;

#[path = "common/call_graph.rs"]
mod call_graph;

const PROBE: &str = "crates/tako-core/src/probe.rs";

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

/// テスト領域を空白へ潰した本文（行番号・バイト位置は原文と同じ）
fn product_text(raw: &str) -> String {
    production_range::scan(raw).text
}

fn probe_raw() -> String {
    std::fs::read_to_string(workspace_root().join(PROBE)).expect("probe.rs が読める")
}

/// `fn <name>` の最初の定義（cfg で 2 つあるときは unix 版が先に並ぶ）の製品の経路の眺め
/// （A/B の旧経路の腕 `if …legacy() {` と入れ子の関数を除いた範囲）
struct View<'g> {
    source: &'g call_graph::Source,
    /// 製品の経路の範囲（原文のバイト位置・昇順）
    ranges: Vec<(usize, usize)>,
    /// 本文の全体（A/B の腕も含む）
    body: (usize, usize),
    /// 関数の頭の行
    line: usize,
}

impl View<'_> {
    fn new<'g>(graph: &'g call_graph::Graph, name: &str) -> Result<View<'g>, String> {
        let f = graph
            .find(PROBE, name)
            .into_iter()
            .min_by_key(|&f| graph.fns[f].line)
            .ok_or_else(|| {
                format!("{PROBE} に `fn {name}` が無い（改名したなら番犬も追うこと）")
            })?;
        let def = &graph.fns[f];
        Ok(View {
            source: &graph.sources[def.file],
            ranges: graph.product_ranges(f),
            body: (def.body.0, def.body.1 + 1),
            line: def.line,
        })
    }

    /// 製品の経路での最初の出現（原文のバイト位置）
    fn find(&self, needle: &str) -> Option<usize> {
        self.ranges
            .iter()
            .find_map(|&(s, e)| self.source.code[s..e].find(needle).map(|i| s + i))
    }

    /// A/B の腕も含めた本文に在るか
    fn body_has(&self, needle: &str) -> bool {
        self.source.code[self.body.0..self.body.1].contains(needle)
    }

    fn line_of(&self, pos: usize) -> usize {
        self.source.line_of(pos)
    }
}

/// 1 の本体: probe.rs の本文（注入で差し替えられる）から、修正前の形へ戻った箇所を file:line で並べる
fn offenders(probe: &str) -> Vec<String> {
    let text = product_text(probe);
    let graph = call_graph::Graph::build(vec![call_graph::Source::new(PROBE, &text)]);
    let mut out = Vec::new();

    // kill_on_timeout: 腕の外はグループへ TERM だけ・腕はゲートの中
    match View::new(&graph, "kill_on_timeout") {
        Err(e) => out.push(e),
        Ok(view) => {
            if view.find("libc::kill(-pid, libc::SIGTERM)").is_none() {
                out.push(format!(
                    "{PROBE}:{}（fn kill_on_timeout）がグループへ SIGTERM を送っていない（#2633: 畳むときは TERM を 1 回 → 猶予の後に刈り取り係が SIGKILL）",
                    view.line
                ));
            }
            if let Some(at) = view.find("SIGKILL") {
                out.push(format!(
                    "{PROBE}:{}（fn kill_on_timeout）が A/B の腕（`if issue2633_legacy() {{`）の外で SIGKILL を送る = rc の EXIT の trap が走れない（#2633）",
                    view.line_of(at)
                ));
            }
            if !view.body_has("issue2633_legacy()") {
                out.push(format!(
                    "{PROBE}:{}（fn kill_on_timeout）に A/B のゲート `issue2633_legacy()` が無い（修正前の形を同じバイナリで再現できない）",
                    view.line
                ));
            }
        }
    }

    // reap_group_in_background: 猶予の後にだけ SIGKILL・それより前に頭を刈り取らない
    match View::new(&graph, "reap_group_in_background") {
        Err(e) => out.push(e),
        Ok(view) => {
            let kill = view.find("libc::kill(-pgid, libc::SIGKILL)");
            let grace = view.find("GROUP_TERM_GRACE");
            match (kill, grace) {
                (Some(k), Some(g)) if g < k => {}
                _ => out.push(format!(
                    "{PROBE}:{}（fn reap_group_in_background）に「猶予（GROUP_TERM_GRACE）を過ぎたら SIGKILL」の形が無い（#2633）",
                    view.line
                )),
            }
            let reap = ["try_wait", ".wait()", "waitpid"]
                .iter()
                .filter_map(|w| view.find(w).map(|i| (i, *w)))
                .min();
            if let (Some((r, what)), Some(k)) = (reap, kill) {
                if r < k {
                    out.push(format!(
                        "{PROBE}:{}（fn reap_group_in_background）が SIGKILL の判定より前に頭を刈り取る（`{what}`）= グループの番号が空き、別のグループへ SIGKILL を送りうる（#2633）",
                        view.line_of(r)
                    ));
                }
            }
        }
    }

    // 待ちの 1 実装: TERM を送ったグループは刈り取り係へ
    match View::new(&graph, "wait_with_timeout") {
        Err(e) => out.push(e),
        Ok(view) => {
            for needle in [
                "let group = kill_on_timeout(&mut child);",
                "reap_group_in_background(child)",
            ] {
                if view.find(needle).is_none() {
                    out.push(format!(
                        "{PROBE}:{}（fn wait_with_timeout）に `{needle}` が無い（打ち切ったグループの猶予と SIGKILL を刈り取り係へ渡していない。#2633）",
                        view.line
                    ));
                }
            }
        }
    }
    out
}

/// 1: 今の probe.rs は TERM → 猶予 → KILL の形
#[test]
fn 打ち切りの畳み方はterm_猶予_killの形() {
    let hits = offenders(&probe_raw());
    assert!(
        hits.is_empty(),
        "打ち切ったログインシェルのグループの畳み方が修正前の形へ戻っている（#2633）:\n  {}\n\n\
         いきなり SIGKILL を送ると rc の途中の処理（pyenv rehash のロック）が後片付けを走らせられない。\
         `.agent/conventions.md`「打ち切ったログインシェルのグループは TERM → 猶予 → KILL で畳む」を参照",
        hits.join("\n  ")
    );
}

/// 1: A/B のゲートは製品コードに 1 か所（`TAKO_2633_LEGACY` を読むのは `probe::issue2633_legacy` だけ）
#[test]
fn ab_のゲートは1か所() {
    fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
        for entry in std::fs::read_dir(dir).expect("読める").flatten() {
            let path = entry.path();
            if path.is_dir() {
                walk(&path, out);
            } else if path.extension().is_some_and(|x| x == "rs") {
                out.push(path);
            }
        }
    }
    let root = workspace_root();
    let mut files = Vec::new();
    for dir in ROOTS {
        walk(&root.join(dir), &mut files);
    }
    let mut hits = Vec::new();
    for path in files {
        let raw = std::fs::read_to_string(&path).expect("読める");
        let text = product_text(&raw);
        for (i, _) in text.match_indices("\"TAKO_2633_LEGACY\"") {
            let rel = path
                .strip_prefix(&root)
                .expect("中")
                .to_string_lossy()
                .replace('\\', "/");
            hits.push(format!("{rel}:{}", text[..i].matches('\n').count() + 1));
        }
    }
    assert_eq!(
        hits.len(),
        1,
        "`TAKO_2633_LEGACY` を読む製品コードは probe::issue2633_legacy の 1 か所だけ: {hits:?}"
    );
    assert!(hits[0].starts_with(PROBE), "{hits:?}");
}

/// 2: 修正前の形の注入を 1 の検査が file:line で名指す（同じテストの中の A/B）
#[test]
fn 修正前の形の注入を名指す() {
    let raw = probe_raw();
    let cases: [(&str, &str, &str); 3] = [
        (
            "グループへの即 SIGKILL（修正前の kill_on_timeout）",
            "libc::kill(-pid, libc::SIGTERM);",
            "libc::kill(-pid, libc::SIGKILL);",
        ),
        (
            "猶予の判定より前の刈り取り",
            "            if let Some(pgid) = pgid {\n                let start = Instant::now();",
            "            let _ = child.wait();\n            if let Some(pgid) = pgid {\n                let start = Instant::now();",
        ),
        (
            "刈り取り係を通さない（修正前の待ちの 1 実装）",
            "                        reap_group_in_background(child);",
            "                        reap_in_background(child);",
        ),
    ];
    for (what, from, to) in cases {
        assert!(
            raw.contains(from),
            "注入の起点が probe.rs に無い（{what}。番犬も追うこと）: {from}"
        );
        let injected = raw.replacen(from, to, 1);
        let hits = offenders(&injected);
        assert!(
            hits.iter().any(|h| h.starts_with(&format!("{PROBE}:"))),
            "注入（{what}）を file:line で名指せない: {hits:?}"
        );
    }
}

// ---- 3: 実プロセス ---------------------------------------------------------------------------------

#[cfg(unix)]
mod live {
    use std::path::Path;
    use std::process::Command;
    use std::time::{Duration, Instant};
    use tako_core::probe::{group_output_with_timeout, Outcome, GROUP_TERM_GRACE};
    use tako_core::test_residue::ScratchDir;

    /// 印が現れる / 消えるのを待つ外側の締め切り（「残っている」を判定するための上限。所要の assert ではない）
    const OUTER: Duration = Duration::from_secs(15);

    fn write_script(path: &Path, body: &str) {
        std::fs::write(path, body).expect("書ける");
        let status = Command::new("chmod")
            .arg("+x")
            .arg(path)
            .status()
            .expect("chmod");
        assert!(status.success());
    }

    /// pyenv の rehash の取り方を写した偽物（EXIT の trap を先に置き、noclobber でロックを作って眠る）
    fn fake_rehash(dir: &Path) -> std::path::PathBuf {
        let path = dir.join("pyenv-rehash");
        write_script(
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

    /// ログインシェルの代役（非対話の sh = rc の子は同じグループ）で `body` を走らせ、`budget` で打ち切る
    fn run_login_like(body: &str, budget: Duration) -> (Outcome, Duration) {
        let mut cmd = Command::new("/bin/sh");
        cmd.arg("-c").arg(body);
        let t0 = Instant::now();
        let got = group_output_with_timeout(&mut cmd, "sh -c (issue2633)", budget);
        (got, t0.elapsed())
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

    fn pid_in(path: &Path) -> u32 {
        read(path).trim().parse().unwrap_or_else(|_| {
            panic!(
                "{} に pid が無い（打ち切りより前に起きていない）",
                path.display()
            )
        })
    }

    /// 1 回分: 偽の rehash がロックを持つ間に打ち切る。返り値は (印, ロックが在るか, 呼び手が返るまで)
    pub fn rehash_case(tag: &str) -> (String, bool, Duration) {
        let dir = ScratchDir::new(tag);
        let rehash = fake_rehash(dir.path());
        let lock = dir.path().join(".pyenv-shim");
        let mark = dir.path().join("mark");
        // rc の rehash の後にも行がある形（= sh は rehash を exec しない。本物の .zprofile と同じ）
        let body = format!(
            "'{}' '{}' '{}'; echo after",
            rehash.display(),
            lock.display(),
            mark.display()
        );
        // 上限は rehash がロックを取るのに十分な長さ（取る前に打ち切ると何も確かめられない）
        let (got, took) = run_login_like(&body, Duration::from_secs(3));
        assert!(
            matches!(got, Outcome::TimedOut { .. }),
            "上限で打ち切る: {got:?}"
        );
        assert!(
            read(&mark).contains("locked"),
            "打ち切りより前に rehash がロックを取っていない（負荷で 3 秒に間に合わなかった）: {:?}",
            read(&mark)
        );
        let rehash_pid = pid_in(&mark.with_extension("pid"));
        wait_until("rehash のプロセスが残っている", || {
            !tako_core::platform::process::pid_alive(rehash_pid)
        });
        (read(&mark), lock.exists(), took)
    }

    pub fn grace() -> Duration {
        GROUP_TERM_GRACE
    }

    pub fn term_ignoring_case() {
        let dir = ScratchDir::new("issue2633-term-ignore");
        let pid = dir.path().join("pid");
        let mark = dir.path().join("mark");
        // TERM を受けたら印を書いて生き続ける（子の sleep は TERM で死ぬがループが起こし直す）
        let body = format!(
            "trap 'echo got-term >> \"{m}\"' TERM; echo $$ > \"{p}\"; while :; do sleep 0.05; done",
            m = mark.display(),
            p = pid.display()
        );
        let (got, took) = run_login_like(&body, Duration::from_secs(2));
        assert!(matches!(got, Outcome::TimedOut { .. }), "{got:?}");
        let leader = pid_in(&pid);
        wait_until(
            "TERM を無視する子が猶予の後も残っている（SIGKILL が届かない）",
            || !tako_core::platform::process::pid_alive(leader),
        );
        assert!(
            read(&mark).contains("got-term"),
            "先に SIGTERM が届いて trap が走っている（いきなり SIGKILL ではない）: {:?}",
            read(&mark)
        );
        eprintln!("[報告] TERM を無視する子: 呼び手が返るまで {took:?}（上限 2s・猶予 {:?} は刈り取り係が持つ）", grace());
    }

    pub fn stopped_case() {
        let dir = ScratchDir::new("issue2633-stopped");
        let child = dir.path().join("child");
        let leader = dir.path().join("leader");
        // 子は自分を止める（rc が端末を読んで SIGTTIN で止まった形の代役）。頭は TERM を無視して生き続ける
        // = グループは孤児にならない（カーネルの HUP + CONT は来ない）。止まった子へ届いた TERM は保留のまま
        // なので、子を消せるのは猶予の後の SIGKILL だけ
        let body = format!(
            "/bin/sh -c 'echo $$ > \"{c}\"; kill -STOP $$; sleep 30' & \
             trap '' TERM; echo $$ > \"{l}\"; while :; do sleep 0.05; done",
            c = child.display(),
            l = leader.display()
        );
        let (got, _) = run_login_like(&body, Duration::from_secs(2));
        assert!(matches!(got, Outcome::TimedOut { .. }), "{got:?}");
        let c = pid_in(&child);
        let l = pid_in(&leader);
        wait_until("止まった子が猶予の後も残っている", || {
            !tako_core::platform::process::pid_alive(c)
        });
        wait_until(
            "TERM を無視する頭が猶予の後も残っている",
            || !tako_core::platform::process::pid_alive(l),
        );
    }
}

/// 3-1: TERM で rc の EXIT の trap が走り、ロックが消える
#[cfg(unix)]
#[test]
fn termで後片付けが走りロックが消える() {
    let (mark, lock_left, took) = live::rehash_case("issue2633-term");
    assert!(
        mark.contains("trap-enter") && mark.contains("trap-rm-ok"),
        "rehash の EXIT の trap が最後まで走る: {mark:?}"
    );
    assert!(!lock_left, "打ち切りの後にロックが残っていない: {mark:?}");
    eprintln!("[報告] 修正後: 呼び手が返るまで {took:?}（上限 3s）");
}

/// 3-2: TERM を無視する子は、猶予の後の SIGKILL で消える（TERM が先に届いている）
#[cfg(unix)]
#[test]
fn termを無視する子は猶予の後に消える() {
    live::term_ignoring_case();
}

/// 3-3: 止まった子（TERM は保留のまま）も、猶予の後の SIGKILL で消える
#[cfg(unix)]
#[test]
fn 止まった子も猶予の後に消える() {
    live::stopped_case();
}

/// 3-4: A/B の旧経路（`TAKO_2633_LEGACY=1`）は修正前の形 = trap が走らずロックが残る
#[cfg(unix)]
#[test]
fn legacyではロックが残る() {
    if tako_core::run_alone!(legacyではロックが残る) {
        return;
    }
    // 子の中だけ（他のテストは居ない。#2400）。ゲートは最初に読んだ値を持つので、呼ぶ前に立てる
    std::env::set_var("TAKO_2633_LEGACY", "1");
    let (mark, lock_left, took) = live::rehash_case("issue2633-legacy");
    assert!(
        !mark.contains("trap-enter"),
        "旧経路はグループへの即 SIGKILL = trap は走らない: {mark:?}"
    );
    assert!(
        lock_left,
        "旧経路ではロックが残る（#2633 の再現）: {mark:?}"
    );
    eprintln!("[報告] 修正前（LEGACY）: 呼び手が返るまで {took:?}（上限 3s）");
}
