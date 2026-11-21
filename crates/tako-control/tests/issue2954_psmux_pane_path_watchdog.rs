//! **#2954 の番犬**: Windows の器（psmux）のペインへ、器なしのペインと同じ PATH を `new-session -e` で固定し、
//! 逃げ道 `PSMUX_BARE_ENV`（psmux #167）が立っていれば素のペインのシェルも包みの経路で渡す。
//!
//! ## なぜ縛るのか
//!
//! psmux 3.3.x は同梱の portable-pty（`cmdbuilder.rs` の `get_base_env`）が、ペインの env をレジストリの
//! `Path`（マシン + ユーザー）で丸ごと置き換える。器のクライアント → サーバーへは tako が組んだ PATH が
//! 継がれているのに、器のペインのシェルには届かない（Windows 実機: GUI の PATH の先頭に置いた偽の claude が
//! 器のペインの `where.exe claude` に出ず、本物を指した）。psmux は `-e` をその置き換えの**後**に当てる
//! （`apply_user_environment`）ので、そこで固定すれば器なしのペインと揃う。固定が外れても macOS の CI は
//! 何も落ちない（tmux の器は固定しない）ので、外れた形をここで名指す。
//!
//! また #2484 で素のペインのシェルを `--` の直接の argv（psmux の `build_raw_command`）で渡すようにした結果、
//! 環境ブロックが大きすぎてペインを起こせない利用者の逃げ道 `PSMUX_BARE_ENV=1` が効かなくなっていた
//! （`build_raw_command` は見ない）。立っていれば包みの経路へ戻す判断も縛る。
//!
//! ## 何を確かめるか（外れたら file:line を名指して落とす）
//!
//! 1. 能力 `rebuilds_pane_path` は psmux が true、tmux と器なしは false
//! 2. psmux の `wrap_spawn` が能力と A/B のゲートを見て、器なしのペインと同じ決め方の PATH
//!    （`user_path::container_pane_path`）を組み立てへ渡し、`PSMUX_BARE_ENV` で直接の argv を外す
//! 3. 組み立ての本体（`wrap_options_for`）が渡された PATH を `-e` で積み、コマンドラインの上限を見る。
//!    tmux の入口（`wrap_options`）は PATH を渡さない
//! 4. PATH の決め方は器なしのペインと同じ材料（`pane_path` = #2428 の読み直しと起動時の PATH）
//! 5. A/B のゲートは `TAKO_2954_LEGACY` を読む 1 か所・逃げ道の env 名は `PSMUX_BARE_ENV`

use std::path::{Path, PathBuf};

const TMUX_BACKEND: &str = "crates/tako-core/src/tmux_backend.rs";
const TMUX_CAPS: &str = "crates/tako-core/src/backend/tmux.rs";
const NULL_CAPS: &str = "crates/tako-core/src/backend/null.rs";
const PSMUX: &str = "crates/tako-core/src/backend/psmux.rs";
const USER_PATH: &str = "crates/tako-core/src/platform/user_path.rs";

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
    // テストが本番より厚いファイルは下限を下げて読む（#2484 / #2741 の番犬と同じ扱い）
    let floor = if [TMUX_BACKEND, PSMUX, TMUX_CAPS, NULL_CAPS].contains(&rel) {
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

/// 1: 器の能力の申告
fn check_caps(rel: &str, src: &str, expect: bool) -> Vec<String> {
    let want = format!("rebuilds_pane_path: {expect},");
    if src.contains(&want) {
        return Vec::new();
    }
    let at = src.find("rebuilds_pane_path").unwrap_or(0);
    vec![format!(
        "{rel}:{} の器の能力に `{want}` が無い（psmux はペインの PATH をレジストリで組み直す = true、\
         tmux と器なしは false。#2954）",
        line_at(src, at)
    )]
}

/// 2・5: psmux の器（`backend/psmux.rs`）
fn check_psmux(rel: &str, src: &str) -> Vec<String> {
    let mut hits = Vec::new();
    need(
        &mut hits,
        rel,
        src,
        "fn wrap_spawn(",
        &[
            "rebuilds_pane_path",
            "container_pane_path(&options.env)",
            "issue2954_legacy()",
            "bare_env_requested(&options.env)",
            "pane_path,",
        ],
        "器のペインへ器なしのペインと同じ PATH を渡し、PSMUX_BARE_ENV なら直接の argv を外す",
    );
    // 能力を見ずに渡す・PATH を渡さない（None の直書き）は、上の needles を満たしても外れた形
    if let Some((at, body)) = body_of(src, "fn wrap_spawn(") {
        if body.contains("wrap_options_for(") && body.contains("None,\n") {
            hits.push(format!(
                "{rel}:{} `fn wrap_spawn(` が組み立てへ PATH の代わりに `None` を直書きしている（#2954）",
                line_at(src, at)
            ));
        }
    }
    need(
        &mut hits,
        rel,
        src,
        "pub fn issue2954_legacy(",
        &["ISSUE2954_LEGACY_ENV"],
        "A/B のゲートはこの 1 か所",
    );
    need(
        &mut hits,
        rel,
        src,
        "fn bare_env_requested(",
        &["BARE_ENV", "bare_env_on("],
        "逃げ道の読み方は psmux と同じ 1 実装",
    );
    for (decl, what) in [
        (
            "pub const ISSUE2954_LEGACY_ENV: &str = \"TAKO_2954_LEGACY\";",
            "A/B の env 名 `TAKO_2954_LEGACY`",
        ),
        (
            "pub const BARE_ENV: &str = \"PSMUX_BARE_ENV\";",
            "psmux の逃げ道の env 名 `PSMUX_BARE_ENV`",
        ),
    ] {
        if !src.contains(decl) {
            hits.push(format!("{rel}:1 {what} の定義が無い"));
        }
    }
    hits
}

/// 3: 組み立ての本体（`tmux_backend.rs`）
fn check_assembly(rel: &str, src: &str) -> Vec<String> {
    let mut hits = Vec::new();
    need(
        &mut hits,
        rel,
        src,
        "pub(crate) fn wrap_options_for(",
        &[
            "pane_path: Option<(String, String)>",
            "pane_path_pin_fits(",
            "args.splice(path_at..path_at, pin)",
        ],
        "渡された PATH を `-e` で積み、コマンドラインの上限を超えるなら積まない",
    );
    need(
        &mut hits,
        rel,
        src,
        "pub(crate) fn pane_path_pin_fits(",
        &[
            "command_line_len_upper(",
            "COMMAND_LINE_MAX",
            "PANE_PATH_PIN_MARGIN",
        ],
        "器のサーバーのコマンドラインへ写るぶんの余白を見る",
    );
    // tmux の入口は PATH を渡さない（macOS の器の argv を変えない）
    if let Some((at, body)) = body_of(src, "pub fn wrap_options(") {
        if body.contains("container_pane_path(") || !body.contains("None,") {
            hits.push(format!(
                "{rel}:{} tmux の入口 `wrap_options` が PATH を渡している（tmux の器は継いだ PATH + ログインシェル。#2954）",
                line_at(src, at)
            ));
        }
    }
    hits
}

/// 4: PATH の決め方（`platform/user_path.rs`）
fn check_user_path(rel: &str, src: &str) -> Vec<String> {
    let mut hits = Vec::new();
    need(
        &mut hits,
        rel,
        src,
        "pub fn container_pane_path(",
        &[
            "container_pane_path_in(",
            "explicit, pane_path,",
            "process_path()",
        ],
        "器なしのペインと同じ材料（#2428 の読み直しと起動時の PATH）",
    );
    need(
        &mut hits,
        rel,
        src,
        "pub fn container_pane_path_in(",
        &[
            "eq_ignore_ascii_case(\"PATH\")",
            "reread().or_else(process)",
        ],
        "器なしのペインと同じ順（明示 → 読み直し → 起動時の値）",
    );
    hits
}

#[test]
fn issue2954_psmuxの器のペインへ器なしのペインと同じpathを固定する() {
    let mut hits = check_caps(PSMUX, &read(PSMUX), true);
    hits.extend(check_caps(TMUX_CAPS, &read(TMUX_CAPS), false));
    hits.extend(check_caps(NULL_CAPS, &read(NULL_CAPS), false));
    hits.extend(check_psmux(PSMUX, &read(PSMUX)));
    hits.extend(check_assembly(TMUX_BACKEND, &read(TMUX_BACKEND)));
    hits.extend(check_user_path(USER_PATH, &read(USER_PATH)));
    assert!(
        hits.is_empty(),
        "#2954: 器のペインの PATH の固定 / PSMUX_BARE_ENV の逃げ道 / 能力が外れている:\n{}",
        hits.join("\n")
    );
}

/// 検出力: 修正前の形（PATH を渡さない・能力を見ない・逃げ道を見ない・積まない・tmux へ渡す）を名指す
#[test]
fn issue2954_番犬は外れた形を名指しする() {
    let psmux = r#"
pub const ISSUE2954_LEGACY_ENV: &str = "TAKO_2954_LEGACY";
pub fn issue2954_legacy() -> bool { std::env::var(ISSUE2954_LEGACY_ENV).is_ok() }
pub const BARE_ENV: &str = "PSMUX_BARE_ENV";
fn bare_env_requested(env: &[(String, String)]) -> bool { bare_env_on(std::env::var(BARE_ENV).ok().as_deref()) }
impl SessionBackend for PsmuxBackend {
    fn wrap_spawn(&self, options: SpawnOptions) -> SpawnOptions {
        let fixed_2954 = !issue2954_legacy();
        let pane_path = if caps.rebuilds_pane_path && fixed_2954 {
            crate::platform::user_path::container_pane_path(&options.env)
        } else {
            None
        };
        let bare_env = fixed_2954 && bare_env_requested(&options.env);
        wrap_options_for(
            options,
            caps.direct_inner_argv && !bare_env,
            pane_path,
        )
    }
}
"#;
    assert_eq!(check_psmux("p.rs", psmux), Vec::<String>::new());
    // 修正前: PATH を渡さない（#2484 までの形）
    let hits = check_psmux(
        "p.rs",
        &psmux.replace(
            "crate::platform::user_path::container_pane_path(&options.env)",
            "None",
        ),
    );
    assert_eq!(hits.len(), 1, "{hits:?}");
    assert!(
        hits[0]
            .starts_with("p.rs:7 `fn wrap_spawn(` に `container_pane_path(&options.env)` が無い"),
        "{hits:?}"
    );
    // 組み立てへ None を直書きする
    let hits = check_psmux(
        "p.rs",
        &psmux.replace("            pane_path,\n", "            None,\n"),
    );
    assert!(
        hits.iter().any(|h| h
            .starts_with("p.rs:7 `fn wrap_spawn(` が組み立てへ PATH の代わりに `None` を直書き")),
        "{hits:?}"
    );
    // 逃げ道を見ない（#2484 の形 = 直接の argv で BARE_ENV が効かない）
    let hits = check_psmux(
        "p.rs",
        &psmux.replace("fixed_2954 && bare_env_requested(&options.env)", "false"),
    );
    assert_eq!(hits.len(), 1, "{hits:?}");
    assert!(
        hits[0].contains("`bare_env_requested(&options.env)` が無い"),
        "{hits:?}"
    );
    // A/B の env 名が無い
    let hits = check_psmux("p.rs", &psmux.replace("\"TAKO_2954_LEGACY\"", "\"X\""));
    assert_eq!(hits.len(), 1, "{hits:?}");

    let assembly = r#"
pub fn wrap_options(options: SpawnOptions) -> SpawnOptions {
    wrap_options_for(options, None, socket)
}
pub(crate) fn wrap_options_for(
    options: SpawnOptions,
    pane_path: Option<(String, String)>,
) -> SpawnOptions {
    let path_at = args.len();
    if let Some((key, val)) = pane_path {
        let pin = ["-e".to_string(), format!("{key}={val}")];
        if pane_path_pin_fits(program, &args, &pin).is_ok() {
            args.splice(path_at..path_at, pin);
        }
    }
}
pub(crate) fn pane_path_pin_fits(program: &str) -> Result<(), usize> {
    let need = command_line_len_upper(program, &with_pin);
    need + PANE_PATH_PIN_MARGIN <= COMMAND_LINE_MAX
}
"#;
    assert_eq!(check_assembly("t.rs", assembly), Vec::<String>::new());
    // 渡された PATH を積まない（注入: 固定を外す）
    let hits = check_assembly(
        "t.rs",
        &assembly.replace("            args.splice(path_at..path_at, pin);\n", ""),
    );
    assert_eq!(hits.len(), 1, "{hits:?}");
    assert!(
        hits[0].starts_with("t.rs:5 `pub(crate) fn wrap_options_for(` に `args.splice(path_at..path_at, pin)` が無い"),
        "{hits:?}"
    );
    // tmux の入口が PATH を渡す
    let hits = check_assembly(
        "t.rs",
        &assembly.replace(
            "wrap_options_for(options, None, socket)",
            "wrap_options_for(options, container_pane_path(&options.env), socket)",
        ),
    );
    assert_eq!(hits.len(), 1, "{hits:?}");
    assert!(
        hits[0].starts_with("t.rs:2 tmux の入口 `wrap_options` が PATH を渡している"),
        "{hits:?}"
    );

    // 能力の申告
    assert!(check_caps("t.rs", "rebuilds_pane_path: false,", false).is_empty());
    let hits = check_caps(
        "p.rs",
        "direct_inner_argv: true,\nrebuilds_pane_path: false,",
        true,
    );
    assert_eq!(hits.len(), 1, "{hits:?}");
    assert!(
        hits[0].starts_with("p.rs:2 の器の能力に `rebuilds_pane_path: true,`"),
        "{hits:?}"
    );

    // PATH の決め方が器なしのペインと違う材料を引く
    let user_path = r#"
pub fn container_pane_path_in(explicit: &[(String, String)]) -> Option<(String, String)> {
    let chosen = match explicit.iter().find(|(key, _)| key.eq_ignore_ascii_case("PATH")) {
        Some(pair) => Some(pair.clone()),
        None => reread().or_else(process),
    }?;
}
pub fn container_pane_path(explicit: &[(String, String)]) -> Option<(String, String)> {
    container_pane_path_in(explicit, pane_path, || Some((path_key(), process_path()?)))
}
"#;
    assert_eq!(check_user_path("u.rs", user_path), Vec::<String>::new());
    let hits = check_user_path(
        "u.rs",
        &user_path.replace(
            "container_pane_path_in(explicit, pane_path, ",
            "container_pane_path_in(explicit, || None, ",
        ),
    );
    assert_eq!(hits.len(), 1, "{hits:?}");
    assert!(
        hits[0]
            .starts_with("u.rs:8 `pub fn container_pane_path(` に `explicit, pane_path,` が無い"),
        "{hits:?}"
    );
}
