//! **#2484 の番犬**: Windows の器（psmux）へ書く conf は既定のシェルの先読み（`warm`）を止め、
//! conf の正本は 1 つで、器のクライアントは器の他の操作と同じ実行ファイルで起きる。
//!
//! ## なぜ縛るのか
//!
//! psmux は既定（`warm on`）でサーバーごとに次のペイン用の pwsh（warm pane）と待機サーバー
//! （`<ソケット>____warm__`）を先に起こす。tako は器を `-f <conf>` で起こすので psmux は待機サーバーを
//! 拾わず、先読みは丸ごと無駄になる（Windows 実機: 器 2 本で pwsh 10 本 → `warm off` で 4 本・
//! private 291 MB → 115 MB）。`warm off` は psmux 版の conf にだけあり、本番の spawn が書く conf
//! （器の能力で組む #974 の 1 本）には無かった = **2 つの conf が別々に育った**のが原因。
//!
//! ## 何を確かめるか（外れたら file:line を名指して落とす）
//!
//! 1. 本番の conf の行の表に `set -g warm off` があり、能力 `Need::DisablesShellPrewarm` で書く
//!    （`Need::DisablesShellPrewarm` は `caps.disables_shell_prewarm` を見る = tmux へは書かない）
//! 2. conf の本文（`conf_body`）が A/B のゲート `issue2484_legacy()` を通る
//! 3. psmux の能力が `disables_shell_prewarm: true` / `direct_inner_argv: true`、tmux は両方 false
//! 4. psmux 版の conf（定数・書き手）を持ち直さない
//! 5. 本番の器のクライアントは `bin`（A/B の腕でだけ `tmux_bin()`）
//! 6. 走っている器への再適用は器ごとに `-t` を付け（`source_conf_argvs`）、待機サーバーを落とす（`warm_server_socket`）
//! 7. GUI は器の再適用を `backend().sync_config()` で呼ぶ（`tmux_backend::sync_conf` を直に呼ぶと psmux の
//!    器 1 本にしか届かない）
//! 8. A/B のゲートは `TAKO_2484_LEGACY` を読む 1 か所

use std::path::{Path, PathBuf};

const TMUX_BACKEND: &str = "crates/tako-core/src/tmux_backend.rs";
const TMUX_CAPS: &str = "crates/tako-core/src/backend/tmux.rs";
const PSMUX: &str = "crates/tako-core/src/backend/psmux.rs";
const APP_MAIN: &str = "crates/tako-app/src/main.rs";

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("リポジトリルート")
}

// 本番コードの範囲取りは 1 実装（#1420）。**切らずにテスト領域だけを潰す**ので行番号はずれない
#[path = "common/production_range.rs"]
mod production_range;

/// 本番コードだけの眺め（テスト領域とコメントを同じバイト数の空白へ潰す）
fn read(rel: &str) -> String {
    // tmux_backend.rs と psmux.rs はテストが本番より厚いので下限を下げて読む（#2741 の番犬と同じ扱い）
    let floor = if rel == TMUX_BACKEND || rel == PSMUX {
        0.20
    } else {
        production_range::MIN_COVERAGE
    };
    let src = std::fs::read_to_string(repo_root().join(rel))
        .unwrap_or_else(|e| panic!("{rel} を読めない: {e}"));
    production_range::code_view::without_comments(&production_range::production_with_floor(
        &src, rel, floor,
    ))
}

/// バイト位置の行番号（1 始まり）
fn line_at(src: &str, at: usize) -> usize {
    src[..at].matches('\n').count() + 1
}

/// `open` の位置から始まる `{` に対応する `}` の位置（文字列の中の括弧は数えない）
fn block_end(src: &str, open: usize) -> Option<usize> {
    let code = production_range::code_view_of(src);
    let bytes = code.as_bytes();
    let start = open + code[open..].find('{')?;
    let mut depth = 0usize;
    for (i, b) in bytes.iter().enumerate().skip(start) {
        match b {
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(i);
                }
            }
            _ => {}
        }
    }
    None
}

/// `sig` で始まる関数の本体（無ければ None）
fn body_of<'a>(src: &'a str, sig: &str) -> Option<(usize, &'a str)> {
    let at = src.find(sig)?;
    let end = block_end(src, at)?;
    Some((at, &src[at..end]))
}

/// `sig` の本体に `needles` が全部あるか。無ければ名指しの 1 行を積む
fn need(hits: &mut Vec<String>, rel: &str, src: &str, sig: &str, needles: &[&str], why: &str) {
    match body_of(src, sig) {
        None => hits.push(format!("{rel}:1 `{sig}` が無い（{why}）")),
        Some((at, body)) => {
            for needle in needles {
                if !body.contains(needle) {
                    hits.push(format!(
                        "{rel}:{} `{sig}` に `{needle}` が無い（{why}）",
                        line_at(src, at)
                    ));
                }
            }
        }
    }
}

/// 1・2: 本番の conf（`tmux_backend.rs`）
fn check_conf(rel: &str, src: &str) -> Vec<String> {
    let mut hits = Vec::new();
    match src.find("\"set -g warm off\"") {
        None => hits.push(format!(
            "{rel}:1 本番の conf の行の表に `set -g warm off` が無い（psmux の器が既定の先読みで pwsh を余計に抱える）"
        )),
        Some(at) => {
            // 同じ Directive の中（直前の `Directive {` から行まで）で能力を求めている
            let open = src[..at].rfind("Directive {").unwrap_or(0);
            if !src[open..at].contains("Need::DisablesShellPrewarm") {
                hits.push(format!(
                    "{rel}:{} `set -g warm off` を能力 `Need::DisablesShellPrewarm` なしで書いている（tmux の器へも書く = tmux が知らない設定）",
                    line_at(src, at)
                ));
            }
        }
    }
    need(
        &mut hits,
        rel,
        src,
        "fn met_by(",
        &["Need::DisablesShellPrewarm => caps.disables_shell_prewarm"],
        "能力の対応は器の申告 1 マスへ 1:1",
    );
    need(
        &mut hits,
        rel,
        src,
        "fn conf_body(",
        &["issue2484_legacy()"],
        "A/B（TAKO_2484_LEGACY=1）で修正前の conf へ戻せる",
    );
    hits
}

/// 3: 器の能力の申告
fn check_caps(rel: &str, src: &str, expect: bool) -> Vec<String> {
    ["disables_shell_prewarm", "direct_inner_argv"]
        .iter()
        .filter(|field| !src.contains(&format!("{field}: {expect},")))
        .map(|field| {
            let at = src.find(*field).unwrap_or(0);
            format!(
                "{rel}:{} の器の能力に `{field}: {expect}` が無い（psmux は warm を止められ・包まずに起こせる = true、tmux は false。#2484）",
                line_at(src, at)
            )
        })
        .collect()
}

/// 4〜6・8: psmux の器（`backend/psmux.rs`）
fn check_psmux(rel: &str, src: &str) -> Vec<String> {
    let mut hits = Vec::new();
    for gone in [
        "const BACKEND_CONF",
        "fn ensure_conf(",
        "fn write_conf_in(",
        "psmux-backend.conf",
    ] {
        if let Some(at) = src.find(gone) {
            hits.push(format!(
                "{rel}:{} psmux 版の conf（`{gone}`）が戻っている。器の conf の正本は `tmux_backend::backend_conf` の 1 本（#2484）",
                line_at(src, at)
            ));
        }
    }
    match body_of(src, "pub fn new(") {
        None => hits.push(format!("{rel}:1 `pub fn new(` が無い")),
        Some((at, body)) => {
            if body.contains("tmux_bin()") && !body.contains("issue2484_legacy()") {
                hits.push(format!(
                    "{rel}:{} 本番の器のクライアントが `tmux_bin()`（素の名前の `tmux`）のまま。器の他の操作と同じ `bin` で起こす（#2484）",
                    line_at(src, at)
                ));
            }
            if !body.contains("bin.clone()") {
                hits.push(format!(
                    "{rel}:{} `pub fn new(` が器のクライアントを `bin` から作っていない（#2484）",
                    line_at(src, at)
                ));
            }
        }
    }
    need(
        &mut hits,
        rel,
        src,
        "fn sync_config(",
        &["sync_conf_all(", "issue2484_legacy()"],
        "走っている器への再適用は器ごとの本体（sync_conf_all）を通す・A/B で戻せる",
    );
    need(
        &mut hits,
        rel,
        src,
        "pub fn sync_conf_all(",
        &["source_conf_argvs(", "warm_server_socket("],
        "psmux はセッションごとに別サーバー。-t なしの再適用は器 1 本にしか届かず、修正前に立った待機サーバーも残る",
    );
    need(
        &mut hits,
        rel,
        src,
        "fn wrap_spawn(",
        &["direct_inner_argv", "issue2484_legacy()"],
        "素のペインのシェルを包みの pwsh を挟まずに起こす（段 2）・A/B で戻せる",
    );
    need(
        &mut hits,
        rel,
        src,
        "pub fn issue2484_legacy(",
        &["ISSUE2484_LEGACY_ENV"],
        "A/B のゲートはこの 1 か所",
    );
    if !src.contains("pub const ISSUE2484_LEGACY_ENV: &str = \"TAKO_2484_LEGACY\";") {
        hits.push(format!(
            "{rel}:1 A/B の env 名 `TAKO_2484_LEGACY` の定義が無い"
        ));
    }
    hits
}

/// 7: GUI の再適用の呼び方
fn check_app(rel: &str, src: &str) -> Vec<String> {
    src.match_indices("tmux_backend::sync_conf(")
        .map(|(at, _)| {
            format!(
                "{rel}:{} GUI が器の再適用を `tmux_backend::sync_conf` で直に呼んでいる。`backend().sync_config()` を通す\
                 （psmux はセッションごとに別サーバーで、直呼びは器 1 本にしか届かない。#2484）",
                line_at(src, at)
            )
        })
        .collect()
}

#[test]
fn issue2484_psmuxの器のconfはwarmを止め正本は1つでクライアントはbackendの解決() {
    let mut hits = check_conf(TMUX_BACKEND, &read(TMUX_BACKEND));
    hits.extend(check_caps(PSMUX, &read(PSMUX), true));
    hits.extend(check_caps(TMUX_CAPS, &read(TMUX_CAPS), false));
    hits.extend(check_psmux(PSMUX, &read(PSMUX)));
    hits.extend(check_app(APP_MAIN, &read(APP_MAIN)));
    assert!(
        hits.is_empty(),
        "#2484: psmux の器の conf / 再適用 / クライアント / 能力が外れている:\n{}",
        hits.join("\n")
    );
}

/// 検出力: 修正前の形（warm off が無い・能力なしで書く・psmux 版の conf が戻る・クライアントが tmux_bin・
/// 再適用が -t なし・GUI が直呼び）を名指す
#[test]
fn issue2484_番犬は外れた形を名指しする() {
    let conf = r#"
const DIRECTIVES: &[Directive] = &[
    Directive {
        needs: None,
        line: "set -g history-limit 10000",
    },
    Directive {
        needs: Some(Need::DisablesShellPrewarm),
        line: "set -g warm off",
    },
];
impl Need {
    fn met_by(self, caps: &BackendCapabilities) -> bool {
        match self {
            Need::DisablesShellPrewarm => caps.disables_shell_prewarm,
        }
    }
}
fn conf_body() -> String {
    backend_conf(&apply_issue2484_legacy(caps, issue2484_legacy()))
}
"#;
    assert_eq!(check_conf("c.rs", conf), Vec::<String>::new());
    // 修正前: warm off の行が無い
    let hits = check_conf(
        "c.rs",
        &conf.replace(
            "        line: \"set -g warm off\",\n",
            "        line: \"x\",\n",
        ),
    );
    assert_eq!(hits.len(), 1, "{hits:?}");
    assert!(
        hits[0].starts_with("c.rs:1 本番の conf の行の表に `set -g warm off` が無い"),
        "{hits:?}"
    );
    // 能力なしで書く
    let hits = check_conf(
        "c.rs",
        &conf.replace("needs: Some(Need::DisablesShellPrewarm)", "needs: None"),
    );
    assert_eq!(hits.len(), 1, "{hits:?}");
    assert!(
        hits[0].starts_with("c.rs:9 `set -g warm off` を能力"),
        "{hits:?}"
    );
    // A/B のゲートを通らない
    let hits = check_conf("c.rs", &conf.replace("issue2484_legacy()", "false"));
    assert_eq!(hits.len(), 1, "{hits:?}");

    let psmux = r#"
pub const ISSUE2484_LEGACY_ENV: &str = "TAKO_2484_LEGACY";
pub fn issue2484_legacy() -> bool { std::env::var(ISSUE2484_LEGACY_ENV).is_ok() }
impl PsmuxBackend {
    pub fn new(bin: String) -> Self {
        let client = if issue2484_legacy() { tmux_bin().to_string() } else { bin.clone() };
        Self { client }
    }
}
impl SessionBackend for PsmuxBackend {
    fn wrap_spawn(&self) -> SpawnOptions {
        wrap_options_for(caps.direct_inner_argv && !issue2484_legacy())
    }
    fn sync_config(&self) {
        if issue2484_legacy() { return; }
        std::thread::spawn(move || sync_conf_all(&bin, &socket, &conf, true));
    }
}
pub fn sync_conf_all(bin: &str, socket: &str, conf: &Path, kill_warm: bool) {
    for argv in source_conf_argvs(conf, &sessions) {}
    let _ = run_with(bin, Some(&warm_server_socket(socket)), &["kill-server"]);
}
"#;
    assert_eq!(check_psmux("p.rs", psmux), Vec::<String>::new());
    // クライアントが tmux_bin のまま（#885〜#2484 の形）
    let hits = check_psmux(
        "p.rs",
        &psmux.replace(
            "let client = if issue2484_legacy() { tmux_bin().to_string() } else { bin.clone() };",
            "let client = tmux_bin().to_string();",
        ),
    );
    assert_eq!(hits.len(), 2, "{hits:?}");
    assert!(
        hits[0].starts_with("p.rs:5 本番の器のクライアントが `tmux_bin()`"),
        "{hits:?}"
    );
    // 再適用が -t なしで 1 回（修正前の形）
    let hits = check_psmux(
        "p.rs",
        &psmux.replace("source_conf_argvs(conf, &sessions)", "vec![conf]"),
    );
    assert_eq!(hits.len(), 1, "{hits:?}");
    assert!(
        hits[0].contains("`pub fn sync_conf_all(` に `source_conf_argvs(` が無い"),
        "{hits:?}"
    );
    // psmux 版の conf が戻る
    let hits = check_psmux(
        "p.rs",
        &format!("{psmux}\nconst BACKEND_CONF: &str = \"\";\n"),
    );
    assert_eq!(hits.len(), 1, "{hits:?}");
    assert!(
        hits[0].contains("psmux 版の conf（`const BACKEND_CONF`）が戻っている"),
        "{hits:?}"
    );

    // 能力の申告
    assert!(check_caps(
        "t.rs",
        "disables_shell_prewarm: false,\ndirect_inner_argv: false,",
        false
    )
    .is_empty());
    let hits = check_caps(
        "p.rs",
        "disables_shell_prewarm: false,\ndirect_inner_argv: true,",
        true,
    );
    assert_eq!(hits.len(), 1, "{hits:?}");
    assert!(
        hits[0].contains("`disables_shell_prewarm: true`"),
        "{hits:?}"
    );

    // GUI が直呼び
    let hits = check_app(
        "m.rs",
        "fn a() {\n    tako_core::backend::backend().sync_config();\n}\nfn b() {\n    tako_core::tmux_backend::sync_conf(&s);\n}\n",
    );
    assert_eq!(hits.len(), 1, "{hits:?}");
    assert!(hits[0].starts_with("m.rs:5 GUI が器の再適用を"), "{hits:?}");
}
