//! **#2980 の番犬**: ssh / sftp の実体を、呼ばれるたびにログインシェルで引き直す形へ戻さない。
//!
//! ## 何が起きていたのか
//!
//! unix の `platform::exe::find` は覚えを持たず、呼ばれるたびにログインシェル
//! （`$SHELL -l -c 'command -v ssh'` = 利用者の rc を毎回読む）を起こす。リモートフォルダを出している間は
//! 自動復帰の見張り（2 秒 tick ごと・ホストごとの `remote_fs::liveness`）とツリーの読み（`ensure_master`）が
//! `remote_fs::ssh_bin` を通るので、#2976 の実経路の約 25 秒の窓でログインシェルが 18 本起きていた。
//!
//! いまは `remote_fs::ssh_bin` / `sftp_bin` が覚え（`SSH_BIN_MEMO` / `SFTP_BIN_MEMO`）を通す。見つかったパスは
//! まだ実行できる限り（使うたびに stat 1 回）使い回し、`BIN_HIT_TTL`（600 秒）ごとに引き直す。見つからなかった
//! 答えは `BIN_MISS_TTL`（60 秒）だけ覚える。打ち切り（確かめられなかった）は覚えない。決め方は #1968 の
//! `stale_binary::WHICH_MEMO` と同じ。
//!
//! ## ここで止めるもの
//!
//! 1. [`実体は覚えを通して引く`] — `crates/tako-core/src/remote_fs.rs` の形を file:line で拘束する:
//!    - `fn ssh_bin` / `fn sftp_bin` は覚えを渡して `resolve_bin` を呼ぶ
//!    - `fn resolve_bin` は A/B の腕（`if issue2980_legacy() {`）の外で `memoized_bin` を通し、`exe::find` を直に呼ばない
//!    - `fn memoized_bin` は覚えたパスを返す前に `still_valid` で確かめる（消えた ssh を返さない）
//!    - `fn memoized_bin` は打ち切り（`found.timeout.is_some()`）を覚えに書く前に返す
//!    - 製品コード（4 クレート）で `remote_fs.rs` の外から ssh / sftp の実体を直に引かない
//!    - A/B のゲート（`TAKO_2980_LEGACY`）は製品コードに 1 か所
//! 2. [`修正前の形の注入を名指す`] — 1 の検査が、上の形を外した注入を file:line で名指すこと
//! 3. 実プロセス（unix）: 偽のログインシェルが聞かれた回数で、`ssh_bin` を 2 秒 tick 相当に 10 回呼んでも
//!    1 回だけ・ssh を消すとその場で引き直す・A/B の旧経路（`TAKO_2980_LEGACY=1`）は 10 回
//!
//! ## 相方
//!
//! 覚えの判定の単体テスト（`remote_fs::tests::issue2980_*`。時計を差し替えて TTL と打ち切りを見る）と、
//! 実経路 `bash scripts/test-remote-recovery-offui-2976.sh`（隔離 GUI の窓でログインシェルが聞かれた回数）。

use std::path::{Path, PathBuf};

#[path = "common/production_range.rs"]
mod production_range;

#[path = "common/call_graph.rs"]
mod call_graph;

const REMOTE_FS: &str = "crates/tako-core/src/remote_fs.rs";
/// 製品コードを持つ 4 クレート
const ROOTS: &[&str] = &[
    "crates/tako-core/src",
    "crates/tako-control/src",
    "crates/tako-cli/src",
    "crates/tako-app/src",
];
/// ssh / sftp の実体を直に引く字面（`remote_fs.rs` の外では使わない）
const DIRECT_LOOKUPS: &[&str] = &[
    "find(\"ssh\")",
    "find(\"sftp\")",
    "find_bounded(\"ssh\")",
    "find_bounded(\"sftp\")",
    "find_noting_timeout(\"ssh\")",
    "find_noting_timeout(\"sftp\")",
    "find_fresh_bounded(\"ssh\")",
    "find_fresh_bounded(\"sftp\")",
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

/// 製品の経路で `needle` が最初に現れる位置（原文のバイト位置）
fn first_in(code: &str, ranges: &[(usize, usize)], needle: &str) -> Option<usize> {
    ranges
        .iter()
        .find_map(|&(s, e)| code[s..e].find(needle).map(|i| s + i))
}

/// 1 の本体（remote_fs.rs の形）: 本文（注入で差し替えられる）から、修正前の形へ戻った箇所を file:line で並べる
fn remote_fs_offenders(raw: &str) -> Vec<String> {
    let graph =
        call_graph::Graph::build(vec![call_graph::Source::new(REMOTE_FS, &product_text(raw))]);
    let src = &graph.sources[0];
    let mut out = Vec::new();
    let view = |name: &str| {
        graph
            .find(REMOTE_FS, name)
            .into_iter()
            .min_by_key(|&f| graph.fns[f].line)
            .map(|f| (graph.fns[f].line, graph.product_ranges(f)))
    };

    // 眺め（code）は文字列リテラルを潰すので、名前（"ssh"）を含まない部分で見る
    for (name, want) in [
        ("ssh_bin", "&SSH_BIN_MEMO)"),
        ("sftp_bin", "&SFTP_BIN_MEMO)"),
    ] {
        match view(name) {
            None => out.push(format!(
                "{REMOTE_FS}: `fn {name}` が無い（改名したなら番犬も追うこと）"
            )),
            Some((line, ranges)) => {
                let via = first_in(&src.code, &ranges, "resolve_bin(");
                let memo = first_in(&src.code, &ranges, want);
                if via.is_none() || memo.is_none() {
                    out.push(format!(
                        "{REMOTE_FS}:{line}（fn {name}）が覚え（`{want}`）を渡して `resolve_bin(` を呼んでいない = 呼ばれるたびにログインシェルで引き直す（#2980）"
                    ));
                }
            }
        }
    }

    match view("resolve_bin") {
        None => out.push(format!(
            "{REMOTE_FS}: `fn resolve_bin` が無い（改名したなら番犬も追うこと）"
        )),
        Some((line, ranges)) => {
            if let Some(at) = first_in(&src.code, &ranges, "exe::find(") {
                out.push(format!(
                    "{REMOTE_FS}:{}（fn resolve_bin）が A/B の腕の外で `exe::find(` を直に呼ぶ = 覚えを通さずにログインシェルを起こす（#2980）",
                    src.line_of(at)
                ));
            }
            if first_in(&src.code, &ranges, "memoized_bin(").is_none() {
                out.push(format!(
                    "{REMOTE_FS}:{line}（fn resolve_bin）が A/B の腕の外で覚え（`memoized_bin(`）を通していない（#2980）"
                ));
            }
        }
    }

    match view("memoized_bin") {
        None => out.push(format!(
            "{REMOTE_FS}: `fn memoized_bin` が無い（改名したなら番犬も追うこと）"
        )),
        Some((line, ranges)) => {
            if first_in(&src.code, &ranges, "still_valid(path)").is_none() {
                out.push(format!(
                    "{REMOTE_FS}:{line}（fn memoized_bin）が覚えたパスを返す前に `still_valid` で確かめていない = 消した ssh を返し続ける（#2980）"
                ));
            }
            let timeout = first_in(&src.code, &ranges, "found.timeout.is_some()");
            let write = first_in(&src.code, &ranges, "*memo = Some(");
            match (timeout, write) {
                (Some(t), Some(w)) if t < w => {}
                (_, Some(w)) => out.push(format!(
                    "{REMOTE_FS}:{}（fn memoized_bin）が打ち切り（`found.timeout.is_some()`）を確かめる前に覚えへ書く = rc が一時的に固まっただけで ssh を {} 秒「無い」と答える（#2980）",
                    src.line_of(w),
                    60
                )),
                (_, None) => out.push(format!(
                    "{REMOTE_FS}:{line}（fn memoized_bin）が引いた答えを覚えへ書いていない（`*memo = Some(`。形を変えたなら番犬も追うこと）"
                )),
            }
        }
    }
    out
}

/// 1 の本体（製品コード全体）: `remote_fs.rs` の外で ssh / sftp の実体を直に引く行
fn direct_lookup_offenders(files: &[(String, String)]) -> Vec<String> {
    let mut out = Vec::new();
    for (rel, raw) in files {
        if rel == REMOTE_FS {
            continue;
        }
        let text = product_text(raw);
        for (i, line) in text.lines().enumerate() {
            if line.trim_start().starts_with("//") {
                continue;
            }
            if let Some(hit) = DIRECT_LOOKUPS.iter().find(|n| line.contains(*n)) {
                out.push(format!(
                    "{rel}:{}: `{hit}` で ssh / sftp の実体を直に引いている = 覚えを通さずにログインシェルを起こす。`remote_fs::ssh_bin` / `sftp_bin` を使うこと（#2980）",
                    i + 1
                ));
            }
        }
    }
    out
}

fn product_files() -> Vec<(String, String)> {
    production_range::product_sources()
        .into_iter()
        .filter(|rel| ROOTS.iter().any(|r| rel.starts_with(r)))
        .map(|rel| {
            let raw = read_rel(&rel);
            (rel, raw)
        })
        .collect()
}

#[test]
fn 実体は覚えを通して引く() {
    let mut hits = remote_fs_offenders(&read_rel(REMOTE_FS));
    hits.extend(direct_lookup_offenders(&product_files()));
    assert!(hits.is_empty(), "{}", hits.join("\n"));
}

#[test]
fn ab_のゲートは1か所() {
    let mut found = Vec::new();
    for (rel, raw) in product_files() {
        for (i, line) in product_text(&raw).lines().enumerate() {
            if line.contains("std::env::var(ISSUE2980_LEGACY_ENV)")
                || line.contains("std::env::var(\"TAKO_2980_LEGACY\")")
            {
                found.push(format!("{rel}:{}", i + 1));
            }
        }
    }
    assert_eq!(
        found.len(),
        1,
        "A/B のゲート `TAKO_2980_LEGACY` を読むのは製品コードに 1 か所（remote_fs::issue2980_legacy）: {found:?}"
    );
    assert!(found[0].starts_with(REMOTE_FS), "{found:?}");
}

#[test]
fn 修正前の形の注入を名指す() {
    let raw = read_rel(REMOTE_FS);
    let cases: &[(&str, &str, &str)] = &[
        (
            "ssh_bin が覚えを通さない",
            "    resolve_bin(\"ssh\", &SSH_BIN_MEMO)\n",
            "    crate::platform::exe::find(\"ssh\")\n",
        ),
        (
            "resolve_bin が覚えの外で毎回引く",
            "    memoized_bin(\n        memo,\n",
            "    let _ = crate::platform::exe::find(name);\n    memoized_bin(\n        memo,\n",
        ),
        (
            "覚えたパスを確かめずに返す",
            "Some(path) if age < BIN_HIT_TTL && still_valid(path) => return Some(path.clone()),",
            "Some(path) if age < BIN_HIT_TTL => return Some(path.clone()),",
        ),
        (
            "打ち切りも覚える",
            "    if found.timeout.is_some() {",
            "    if false {",
        ),
    ];
    for &(what, from, to) in cases {
        assert!(
            raw.contains(from),
            "注入の起点が {REMOTE_FS} に無い（{what}。形を変えたなら注入も追うこと）: {from}"
        );
        let hits = remote_fs_offenders(&raw.replacen(from, to, 1));
        assert!(
            hits.iter().any(|h| h.starts_with(&format!("{REMOTE_FS}:"))),
            "注入（{what}）を {REMOTE_FS} の file:line で名指せない: {hits:?}"
        );
    }
    // remote_fs.rs の外で直に引く形
    let injected = vec![(
        "crates/tako-control/src/dispatch.rs".to_string(),
        "fn f() {\n    let ssh = tako_core::platform::exe::find(\"ssh\");\n}\n".to_string(),
    )];
    assert_eq!(
        direct_lookup_offenders(&injected)
            .iter()
            .filter(|h| h.starts_with("crates/tako-control/src/dispatch.rs:2:"))
            .count(),
        1,
        "remote_fs.rs の外で ssh を直に引く注入を file:line で名指せない"
    );
    // 今の形は名指さない
    assert!(remote_fs_offenders(&raw).is_empty());
}

// ---- 3: 実プロセス ---------------------------------------------------------------------------------

#[cfg(unix)]
mod live {
    use std::path::{Path, PathBuf};
    use tako_core::test_residue::ScratchDir;

    /// 砂場: 偽の ssh と、`command -v ssh` に偽物を答えて聞かれた回数を記録する偽のログインシェル
    pub struct Sandbox {
        _dir: ScratchDir,
        pub ssh: PathBuf,
        pub log: PathBuf,
    }

    impl Sandbox {
        /// `SHELL` を偽物へ書き換えるので、`run_alone!` の子の中でだけ呼ぶ（#2400）
        pub fn new(tag: &str) -> Self {
            tako_core::test_child::assert_alone("issue2980 の Sandbox::new");
            let dir = ScratchDir::new(tag);
            let ssh = dir.path().join("bin").join("ssh");
            let log = dir.path().join("asked.log");
            std::fs::create_dir_all(ssh.parent().expect("bin")).expect("bin を作れる");
            tako_core::fake_exec::write_script(&ssh, "#!/bin/sh\nexit 0\n");
            let shell = dir.path().join("fakesh");
            tako_core::fake_exec::write_script(
                &shell,
                &format!(
                    "#!/bin/sh\n\
                     if [ \"$1\" = \"-l\" ]; then shift; fi\n\
                     case \"$*\" in\n\
                       *\"command -v ssh\"*) echo x >> '{log}'; [ -x '{ssh}' ] && echo '{ssh}'; exit 0 ;;\n\
                     esac\n\
                     exec /bin/sh \"$@\"\n",
                    log = log.display(),
                    ssh = ssh.display()
                ),
            );
            // 子の中だけ（他のテストは居ない。#2400）
            std::env::set_var("SHELL", &shell);
            Sandbox {
                _dir: dir,
                ssh,
                log,
            }
        }

        /// 偽のログインシェルが `command -v ssh` を聞かれた回数
        pub fn asked(&self) -> usize {
            std::fs::read_to_string(&self.log)
                .map(|s| s.lines().count())
                .unwrap_or(0)
        }

        pub fn ssh_str(&self) -> String {
            self.ssh.display().to_string()
        }
    }

    pub fn exists(p: &Path) -> bool {
        p.exists()
    }
}

/// 3-1: 2 秒 tick 相当に 10 回呼んでもログインシェルは 1 回だけ・ssh を消すとその場で引き直す
#[cfg(unix)]
#[test]
fn ssh_bin_を繰り返し呼んでもログインシェルは1回だけ() {
    if tako_core::run_alone!(ssh_bin_を繰り返し呼んでもログインシェルは1回だけ)
    {
        return;
    }
    std::env::remove_var("TAKO_2980_LEGACY");
    let sandbox = live::Sandbox::new("issue2980-memo");
    for _ in 0..10 {
        assert_eq!(
            tako_core::remote_fs::ssh_bin(),
            Some(sandbox.ssh_str()),
            "偽のログインシェルが答えた偽の ssh"
        );
    }
    assert_eq!(
        sandbox.asked(),
        1,
        "10 回呼んでもログインシェルに聞いたのは 1 回だけ"
    );
    // ssh を消した → 次の呼び出しで引き直し、見つからない
    std::fs::remove_file(&sandbox.ssh).expect("偽の ssh を消せる");
    assert!(!live::exists(&sandbox.ssh));
    assert_eq!(tako_core::remote_fs::ssh_bin(), None);
    assert_eq!(
        sandbox.asked(),
        2,
        "覚えたパスが消えたのでその場で引き直した"
    );
    // 見つからない答えは BIN_MISS_TTL だけ覚える（入れ直してもその間は聞かない = 単体テストが時計を進めて見る）
    tako_core::fake_exec::write_script(&sandbox.ssh, "#!/bin/sh\nexit 0\n");
    assert_eq!(tako_core::remote_fs::ssh_bin(), None);
    assert_eq!(sandbox.asked(), 2, "見つからない答えは覚えている");
}

/// 3-2: A/B の旧経路（`TAKO_2980_LEGACY=1`）は呼ぶたびにログインシェルで引き直す（修正前の再現）
#[cfg(unix)]
#[test]
fn 旧経路ではssh_binを呼ぶたびにログインシェルを起こす() {
    if tako_core::run_alone!(旧経路ではssh_binを呼ぶたびにログインシェルを起こす)
    {
        return;
    }
    std::env::set_var("TAKO_2980_LEGACY", "1");
    let sandbox = live::Sandbox::new("issue2980-legacy");
    for _ in 0..10 {
        assert_eq!(tako_core::remote_fs::ssh_bin(), Some(sandbox.ssh_str()));
    }
    assert_eq!(
        sandbox.asked(),
        10,
        "旧経路は 10 回とも聞く（#2980 の再現）"
    );
}
