//! #2519 の番犬: 長生きする GUI が周期的に起こす外部コマンドを**素の名前のまま**起こさない
//!
//! # 何が起きていたか
//!
//! 素の名前の spawn（`posix_spawnp`）は PATH の空振り 1 回ごとにカーネルが**親の実行ファイルのまま**の
//! 子を一瞬作る。install の差し替え（`bundle_install` の `.tako-replace-<pid>`）で GUI の実行ファイルが
//! 消えた後は、その子が AppleSystemPolicy に 1 本ずつ拒否された。git / tmux の場所の解決
//! （`bin_resolve`）が PATH で当たった名前を素の名前のまま持っていたので、2 秒ごとの定期更新が
//! git / tmux を約 7 回 × PATH の手前 11 エントリぶん空振りし、install 1 回あたり 876〜1,448 件の拒否を
//! GUI の再起動まで積んだ（10/29〜10/30 の本番の記録）。remote daemon の tailscale（#2513）と同じ仕組み。
//!
//! # 何を縛るか
//!
//! 1. git / tmux は PATH で当たっても**絶対パス**で持つ（実ファイルと実 PATH で `git_bin()` / `tmux_bin()`
//!    を引く）。戻すと `bin_resolve.rs` の `fn resolve_within` を file:line で名指す
//! 2. GUI の周期から起こす口（[`PERIODIC`]）は素の名前のリテラルで起こさず、`exe::resolve_for_spawn` を通る
//! 3. git / tmux を素の名前のリテラルで起こす本番コードが無い（`git_bin()` / `tmux_bin()` を通す）
//! 4. `git_bin()` / `tmux_bin()` は持った場所を `Held::current` 越しに返す（消えた絶対パスを起こし続けない）
//! 5. GUI の 2 秒ごとの remote の状態の更新が通る口（[`REMOTE_REFRESH`]）は素の名前のリテラルで起こさない
//! 6. このプロセスの PATH に無い素の名前の tailscale を起こさない（実ファイルと実 PATH で `detect_tailscale` を
//!    引く）。戻すと `tailscale.rs` の `fn pick_candidate` を file:line で名指す
//!
//! # 開け直し（10/30 17:40 の本番）で見つかった 6 の形
//!
//! Dock から起こした GUI の PATH は `/usr/bin:/bin:/usr/sbin:/sbin` の 4 エントリで、tailscale はどこにも無い。
//! remote daemon が動いている間、2 秒ごとの `refresh_remote_state` が `remote::daemon_status` を 2 回
//! （そのまま 1 回 + `admin_request` の中で 1 回）呼び、そのたびに `survey_variants` → `find_tailscale` →
//! `pick_candidate` が「PATH で引けなかった素の名前は素の名前のまま試す」で `tailscale --version` を起こして
//! 4 エントリぶん空振りしていた（答えは後ろの `/opt/homebrew/bin/tailscale` で決まるので毎回の無駄撃ち）。
//! install の差し替えの後は 2 秒ごとに 4 件 × 2 の拒否を GUI の再起動まで積んだ（1,220 件 / 315 秒）。
//!
//! [`逆戻りを名指しできる`] で、修正前の形を注入した本文が実際に名指しされることを確かめる。
//! A/B（`TAKO_2519_LEGACY=1`）は [`旧挙動は素の名前のまま持つ`] と
//! [`旧挙動はpathに無い素の名前のtailscaleを起こす`] が同じ PATH で再現する。
//!
//! # 洗い出しで対象外にしたもの
//!
//! 周期で起きない口（ユーザー操作・1 日 1 回・状態が変わったときに 1 回）と、絶対パスかログインシェルで
//! 引いた場所で起こす口は縛らない。表は PR #2519 の本文と `.agent/conventions.md`
//! 「長生きするプロセスは素の名前で子を起こし続けない」節にある。

use std::path::{Path, PathBuf};

#[path = "common/production_range.rs"]
mod production_range;

use production_range::code_view::without_comments;

const BIN_RESOLVE: &str = "crates/tako-core/src/bin_resolve.rs";
const GIT: &str = "crates/tako-core/src/git.rs";
const TMUX: &str = "crates/tako-core/src/tmux.rs";

/// GUI の周期から起こす口（ファイル・関数・起こすもの・どの周期か）。
/// ここに載せた関数は素の名前のリテラルで子を起こしてはならない
const PERIODIC: &[(&str, &str, &str, &str)] = &[
    (
        "crates/tako-control/src/codex_session.rs",
        "thread_id_for_pid",
        "lsof",
        "GUI の会話 ID の走査（30 秒ごと × codex のペイン）",
    ),
    (
        "crates/tako-control/src/agy_session.rs",
        "conversation_id_for_pid",
        "lsof",
        "GUI の会話 ID の走査（30 秒ごと × agy のペイン）",
    ),
    (
        "crates/tako-control/src/sleep_guard.rs",
        "set_disablesleep",
        "sudo",
        "スリープ防止の周期（状態の変わり目ごと）",
    ),
    (
        "crates/tako-control/src/sleep_guard.rs",
        "force_display_sleep",
        "pmset",
        "スリープ防止の周期（蓋を閉じるたび）",
    ),
];

/// GUI の 2 秒ごとの remote の状態の更新（`main.rs` の定期更新の先頭 → `refresh_remote_state`）が
/// remote daemon の稼働中に毎回通る口（ファイル・関数）。ここに載せた関数は素の名前のリテラルで
/// 子を起こしてはならない（tailscale は検出の正本 `detect_tailscale` が引いた場所で起こす）
const REMOTE_REFRESH: &[(&str, &str)] = &[
    (
        "crates/tako-app/src/remote_panel.rs",
        "refresh_remote_state",
    ),
    ("crates/tako-control/src/remote.rs", "daemon_status"),
    ("crates/tako-control/src/remote.rs", "admin_request"),
    ("crates/tako-control/src/tailscale.rs", "survey_variants"),
    ("crates/tako-control/src/tailscale.rs", "setup_status_on"),
    ("crates/tako-control/src/tailscale.rs", "detect_tailscale"),
    ("crates/tako-control/src/tailscale.rs", "pick_candidate"),
    ("crates/tako-control/src/tailscale.rs", "probe_candidate"),
    (
        "crates/tako-control/src/tailscale.rs",
        "run_tailscale_within",
    ),
];

/// 6 の名指し先（検出の正本の候補の選び方）
#[cfg(unix)]
const TAILSCALE: &str = "crates/tako-control/src/tailscale.rs";

/// git / tmux の素の名前のリテラルを探す範囲（GUI が呼ぶライブラリと GUI 本体）
const SCAN_ROOTS: &[&str] = &[
    "crates/tako-core/src",
    "crates/tako-control/src",
    "crates/tako-app/src",
];

/// 走査から外す領域（ファイル・モジュールの頭の行・理由）。
/// GUI の本体のセルフテストは `TAKO_SELF_TEST=1` のときだけ走る検証の台本で、周期の経路ではない
const EXEMPT_MODULES: &[(&str, &str, &str)] = &[(
    "crates/tako-app/src/main.rs",
    "mod self_test {",
    "セルフテスト（TAKO_SELF_TEST=1 のときだけ走る台本。隔離のソケットの tmux を素で叩く）",
)];

/// 子を起こす口（ここへ素の名前のリテラルを渡す形を探す）
const SPAWN_CALLS: &[&str] = &["Command::new(", "output_with_timeout("];

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("crates/tako-control の 2 つ上がワークスペースルート")
        .to_path_buf()
}

/// テスト領域とコメントを空白へ潰した本文（行番号は保たれる）
fn read(rel: &str) -> String {
    let path = workspace_root().join(rel);
    let src = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("{} が読める: {e}", path.display()));
    without_comments(&production_range::scan(&src).text)
}

fn line_of(src: &str, at: usize) -> usize {
    src[..at].matches('\n').count() + 1
}

/// `at` から始まる子を起こす口へ渡した第 1 引数が素の名前のリテラルなら、その名前
fn bare_literal_after(src: &str, at: usize) -> Option<&str> {
    let rest = src[at..].trim_start();
    let body = rest.strip_prefix('"')?;
    let name = &body[..body.find('"')?];
    let bare = !name.is_empty()
        && !name.contains(['/', '\\'])
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'));
    bare.then_some(name)
}

/// 本文の中で、素の名前のリテラルを子を起こす口へ渡している所（行と名前）
fn bare_spawns(src: &str) -> Vec<(usize, String)> {
    let mut out = Vec::new();
    for call in SPAWN_CALLS {
        let mut from = 0;
        while let Some(found) = src[from..].find(call) {
            let at = from + found + call.len();
            if let Some(name) = bare_literal_after(src, at) {
                out.push((line_of(src, at), name.to_string()));
            }
            from = at;
        }
    }
    out.sort();
    out
}

/// `fn 名前(` の行（字下げ・可視性・前置キーワードを問わない。読み方は共有の 1 実装 = #1496）
fn is_fn_line(line: &str, name: &str) -> bool {
    tako_core::source_scan::fn_head_name(line) == Some(name)
}

/// 関数の窓（宣言行の 1-based 行番号と、窓のバイト範囲）。終わりは宣言行と同じ字下げの `}`
fn fn_window(src: &str, name: &str) -> Option<(usize, std::ops::Range<usize>)> {
    let mut offset = 0;
    let mut start: Option<(usize, usize, usize)> = None;
    for (i, line) in src.split_inclusive('\n').enumerate() {
        match start {
            None if is_fn_line(line, name) => {
                let indent = line.len() - line.trim_start().len();
                start = Some((i + 1, offset, indent));
            }
            Some((at, begin, indent)) => {
                let body = line.trim_end_matches(['\n', '\r']);
                if body.len() == indent + 1 && body.ends_with('}') && body.trim() == "}" {
                    return Some((at, begin..offset + line.len()));
                }
            }
            None => {}
        }
        offset += line.len();
    }
    None
}

/// 2 の検査（本文を受け取る形 = 注入した本文でも同じ検査を回せる）
fn scan_periodic(rel: &str, func: &str, src: &str) -> Vec<String> {
    let Some((at, range)) = fn_window(src, func) else {
        return vec![format!(
            "{rel} — `fn {func}` が見つからない（走査が空振り。名前を変えたらこの表も直す）"
        )];
    };
    let window = &src[range.clone()];
    let mut out = Vec::new();
    for (line, name) in bare_spawns(window) {
        out.push(format!(
            "{rel}:{} — `fn {func}` が `{name}` を素の名前のまま起こしている",
            at + line - 1
        ));
    }
    if !window.contains("resolve_for_spawn(") {
        out.push(format!(
            "{rel}:{at} — `fn {func}` が `exe::resolve_for_spawn` を通っていない"
        ));
    }
    out
}

/// 5 の検査（本文を受け取る形 = 注入した本文でも同じ検査を回せる）
fn scan_remote_refresh(rel: &str, func: &str, src: &str) -> Vec<String> {
    let Some((at, range)) = fn_window(src, func) else {
        return vec![format!(
            "{rel} — `fn {func}` が見つからない（走査が空振り。名前を変えたらこの表も直す）"
        )];
    };
    bare_spawns(&src[range])
        .into_iter()
        .map(|(line, name)| {
            format!(
                "{rel}:{} — `fn {func}` が `{name}` を素の名前のまま起こしている",
                at + line - 1
            )
        })
        .collect()
}

/// 3 の検査: git / tmux を素の名前のリテラルで起こす所
fn scan_git_tmux(rel: &str, src: &str) -> Vec<String> {
    bare_spawns(src)
        .into_iter()
        .filter(|(_, name)| name == "git" || name == "tmux")
        .map(|(line, name)| {
            format!(
                "{rel}:{line} — `{name}` を素の名前のまま起こしている（`{}` を通す）",
                if name == "git" {
                    "tako_core::git::git_bin()"
                } else {
                    "tako_core::tmux::tmux_bin() / tmux_command()"
                }
            )
        })
        .collect()
}

/// 4 の検査: `git_bin()` / `tmux_bin()` は持った場所を `Held::current` 越しに返す
fn scan_held(rel: &str, func: &str, src: &str) -> Vec<String> {
    match fn_window(src, func) {
        None => vec![format!(
            "{rel} — `fn {func}` が見つからない（走査が空振り）"
        )],
        Some((at, range)) => {
            let window = &src[range];
            let mut out = Vec::new();
            if !window.contains("OnceLock<crate::bin_resolve::Held>")
                || !window.contains(".current()")
            {
                out.push(format!(
                    "{rel}:{at} — `fn {func}` が `bin_resolve::Held` を持って `.current()` で返していない \
                     （PATH で当たった絶対パスが消えたときに素の名前へ戻れない）"
                ));
            }
            out
        }
    }
}

/// 外す領域を空白へ潰す（行番号は保つ）
fn blank_exempt(rel: &str, src: String) -> String {
    let mut src = src;
    for (file, head, _why) in EXEMPT_MODULES {
        if *file != rel {
            continue;
        }
        let at = src
            .find(head)
            .unwrap_or_else(|| panic!("{rel}: 外す領域の頭 `{head}` が無い（表を直すこと）"));
        let line_start = src[..at].rfind('\n').map_or(0, |i| i + 1);
        let indent = at - line_start;
        let close = format!("\n{}}}\n", " ".repeat(indent));
        let end = src[at..]
            .find(&close)
            .map(|i| at + i + close.len())
            .unwrap_or_else(|| panic!("{rel}: `{head}` の閉じ括弧が見つからない"));
        let blanked: String = src[at..end]
            .chars()
            .map(|c| if c == '\n' { '\n' } else { ' ' })
            .collect();
        src.replace_range(at..end, &blanked);
    }
    src
}

fn rust_files_under(rel: &str) -> Vec<String> {
    let root = workspace_root();
    let mut out = Vec::new();
    let mut stack = vec![root.join(rel)];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).expect("読める") {
            let path = entry.expect("読める").path();
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().is_some_and(|e| e == "rs") {
                let rel = path
                    .strip_prefix(&root)
                    .expect("ワークスペースの中")
                    .to_string_lossy()
                    .replace('\\', "/");
                out.push(rel);
            }
        }
    }
    out.sort();
    out
}

/// 2: GUI の周期から起こす口は素の名前のリテラルで起こさない
#[test]
fn 周期で子を起こす口は素の名前で起こさない() {
    let mut offenders = Vec::new();
    for (rel, func, name, why) in PERIODIC {
        for hit in scan_periodic(rel, func, &read(rel)) {
            offenders.push(format!("{hit}（{name}・{why}）"));
        }
    }
    assert!(
        offenders.is_empty(),
        "GUI の周期から起こす外部コマンドが素の名前のまま（#2519）。素の名前は起こすたびに PATH の\
         手前のエントリを空振りし、install の差し替えで GUI の実行ファイルが消えた後は空振り 1 回ごとに\
         AppleSystemPolicy の拒否が出る。`tako_core::platform::exe::resolve_for_spawn(\"<名前>\")` で\
         起こすこと:\n  {}",
        offenders.join("\n  ")
    );
}

/// 3: git / tmux を素の名前のリテラルで起こす本番コードが無い
#[test]
fn gitとtmuxは素の名前のリテラルで起こさない() {
    let mut offenders = Vec::new();
    let mut scanned = 0;
    for root in SCAN_ROOTS {
        for rel in rust_files_under(root) {
            let src = blank_exempt(&rel, read(&rel));
            offenders.extend(scan_git_tmux(&rel, &src));
            scanned += 1;
        }
    }
    assert!(scanned > 100, "走査したファイルが少なすぎる（{scanned}）");
    assert!(
        offenders.is_empty(),
        "git / tmux を素の名前のまま起こす本番コードがある（#2519）。場所の解決（`bin_resolve`）が\
         PATH で引いた絶対パスを持っているので、そこを通すこと:\n  {}",
        offenders.join("\n  ")
    );
}

/// 4: `git_bin()` / `tmux_bin()` は持った場所を `Held::current` 越しに返す
#[test]
fn gitとtmuxの場所は使う直前に在るかを確かめる() {
    let mut offenders = Vec::new();
    for (rel, func) in [(GIT, "git_bin"), (TMUX, "tmux_bin")] {
        offenders.extend(scan_held(rel, func, &read(rel)));
    }
    assert!(offenders.is_empty(), "{}", offenders.join("\n"));
}

/// 5: GUI の 2 秒ごとの remote の状態の更新が通る口は素の名前のリテラルで起こさない
#[test]
fn remoteの状態の更新が通る口は素の名前で起こさない() {
    let mut offenders = Vec::new();
    for (rel, func) in REMOTE_REFRESH {
        offenders.extend(scan_remote_refresh(rel, func, &read(rel)));
    }
    assert!(
        offenders.is_empty(),
        "GUI の 2 秒ごとの remote の状態の更新（daemon の稼働中は 1 tick に `daemon_status` を 2 回）が通る口で、\
         外部コマンドを素の名前のまま起こしている（#2519）。素の名前は起こすたびに PATH を空振りし、install の\
         差し替えで GUI の実行ファイルが消えた後は空振り 1 回ごとに AppleSystemPolicy の拒否が出る。\
         tailscale は検出の正本（`tailscale::detect_tailscale`）が引いた場所で起こすこと:\n  {}",
        offenders.join("\n  ")
    );
}

/// 番犬そのものの検査: 修正前の形を注入した本文が名指しされる（見逃す側へ倒れていない）
#[test]
fn 逆戻りを名指しできる() {
    // 2: lsof を素の名前へ戻す
    let (rel, func, ..) = PERIODIC[0];
    let src = read(rel);
    let fixed = "tako_core::platform::exe::resolve_for_spawn(\"lsof\")";
    assert!(
        src.contains(fixed),
        "注入の目印が本文に無い（番犬の注入を追従させること）"
    );
    let hits = scan_periodic(rel, func, &src.replace(fixed, "\"lsof\""));
    assert!(
        hits.iter().any(|h| h.contains("`lsof` を素の名前のまま"))
            && hits.iter().any(|h| h.contains("resolve_for_spawn")),
        "lsof を素の名前へ戻しても名指しされない: {hits:?}"
    );
    let named = hits
        .iter()
        .find(|h| h.contains("`lsof` を素の名前のまま"))
        .expect("名指し");
    let line: usize = named
        .split(" — ")
        .next()
        .and_then(|loc| loc.rsplit(':').next())
        .and_then(|n| n.parse().ok())
        .expect("file:line");
    assert!(
        src.lines()
            .nth(line - 1)
            .is_some_and(|l| l.contains("lsof")),
        "名指しした行が違う: {named}"
    );

    // 3: git を素の名前で起こす行を本番コードへ足す
    let rel = "crates/tako-control/src/config_share/mod.rs";
    let src = read(rel);
    let fixed = "tako_core::git::git_bin(),";
    assert!(src.contains(fixed), "注入の目印が {rel} に無い");
    let hits = scan_git_tmux(rel, &src.replacen(fixed, "\"git\",", 1));
    assert!(
        hits.iter().any(|h| h.contains("`git` を素の名前のまま")),
        "git を素の名前へ戻しても名指しされない: {hits:?}"
    );

    // 4: `Held::current` を外して素の文字列を持つ形へ戻す
    let src = read(GIT);
    let fixed = "BIN.get_or_init(resolve_git_bin).current()";
    assert!(src.contains(fixed), "注入の目印が {GIT} に無い");
    let hits = scan_held(
        GIT,
        "git_bin",
        &src.replace(fixed, "BIN.get_or_init(resolve_git_bin)")
            .replace("OnceLock<crate::bin_resolve::Held>", "OnceLock<String>"),
    );
    assert!(!hits.is_empty(), "Held を外しても名指しされない");

    // 5: remote の状態の更新が通る口へ素の名前の起動を足す
    let (rel, func) = REMOTE_REFRESH[1];
    let src = read(rel);
    let head = "pub fn daemon_status() -> Value {";
    let at = src.find(head).expect("remote.rs に daemon_status が在る");
    let mut injected = src.clone();
    injected.insert_str(
        at + head.len(),
        "\n    let _ = std::process::Command::new(\"tailscale\");",
    );
    let hits = scan_remote_refresh(rel, func, &injected);
    let want = format!("{rel}:{} —", line_of(&src, at) + 1);
    assert!(
        hits.iter()
            .any(|h| h.starts_with(&want) && h.contains("`tailscale` を素の名前のまま")),
        "daemon_status へ素の tailscale を足しても {want} で名指しされない: {hits:?}"
    );

    // 外す領域の外にある素の名前は名指しされる（外す範囲が広がりすぎていない）
    let rel = EXEMPT_MODULES[0].0;
    let src = read(rel);
    let marker = "fn main() {";
    let at = src.find(marker).expect("main.rs に fn main が在る");
    let mut injected = src.clone();
    injected.insert_str(
        at + marker.len(),
        " let _ = std::process::Command::new(\"tmux\");",
    );
    let hits = scan_git_tmux(rel, &blank_exempt(rel, injected));
    assert!(
        hits.iter().any(|h| h.contains("`tmux` を素の名前のまま")),
        "セルフテストの外の素の tmux が名指しされない（外す範囲が広すぎる）: {hits:?}"
    );
}

/// `fn resolve_within` の場所（1 の名指し先）
fn resolve_within_location() -> String {
    let src = read(BIN_RESOLVE);
    let (at, _) =
        fn_window(&src, "resolve_within").expect("bin_resolve.rs に resolve_within が在る");
    format!("{BIN_RESOLVE}:{at}")
}

/// 空振りする 3 つの dir の後ろに偽の git / tmux / lsof を置いた PATH（GUI の PATH の縮小版）
#[cfg(unix)]
struct Fixture {
    _dir: tako_core::test_residue::ScratchDir,
    bin: PathBuf,
    path: std::ffi::OsString,
}

#[cfg(unix)]
fn fixture(tag: &str) -> Fixture {
    let dir = tako_core::test_residue::ScratchDir::new(tag);
    let mut entries = Vec::new();
    for name in ["miss-0", "miss-1", "miss-2", "bin"] {
        let entry = dir.path().join(name);
        std::fs::create_dir_all(&entry).expect("一時 dir");
        entries.push(entry);
    }
    let bin = dir.path().join("bin");
    for name in ["git", "tmux", "tako-2519-tool"] {
        tako_core::fake_exec::write_script(&bin.join(name), "#!/bin/sh\necho 1.0-fake\nexit 0\n");
    }
    let path = std::env::join_paths(&entries).expect("PATH");
    Fixture {
        _dir: dir,
        bin,
        path,
    }
}

/// 子の env を本番の解決の入口へ揃える（run_alone! の子には他のテストが居ない）
#[cfg(unix)]
fn isolate_env(fx: &Fixture) {
    tako_core::test_child::assert_alone("isolate_env");
    std::env::set_var("PATH", &fx.path);
    for key in [
        "TAKO_GIT_BIN",
        "TAKO_TMUX_BIN",
        "TAKO_2151_LEGACY",
        "TAKO_2151_INJECT",
        tako_core::platform::exe::ISSUE2519_LEGACY_ENV,
    ] {
        std::env::remove_var(key);
    }
}

/// 1: git / tmux は PATH で当たっても絶対パスで持つ（周期の口の `resolve_for_spawn` も同じ）
#[cfg(unix)]
#[test]
fn pathで当たったgitとtmuxは絶対パスで持つ() {
    if tako_core::run_alone!(pathで当たったgitとtmuxは絶対パスで持つ) {
        return;
    }
    let fx = fixture("2519-abs");
    isolate_env(&fx);
    for (name, got) in [
        ("git", tako_core::git::git_bin()),
        ("tmux", tako_core::tmux::tmux_bin()),
    ] {
        assert_eq!(
            got,
            fx.bin.join(name).display().to_string(),
            "{}: PATH で当たった {name} を素の名前のまま持っている（#2519）。GUI は 2 秒ごとの定期更新で\
             この名前の子を起こすので、素の名前だと毎回 PATH の手前のエントリを空振りし、install で GUI の\
             実行ファイルが消えた後は空振り 1 回ごとに AppleSystemPolicy の拒否が出る。\
             `exe::find_in_process_path` で引いた絶対パスで試して持つこと",
            resolve_within_location()
        );
    }
    assert_eq!(
        tako_core::platform::exe::resolve_for_spawn("tako-2519-tool"),
        fx.bin.join("tako-2519-tool").display().to_string()
    );
    // PATH に無い名前は素の名前のまま（従来と同じ起こし方）
    assert_eq!(
        tako_core::platform::exe::resolve_for_spawn("tako-2519-absent"),
        "tako-2519-absent"
    );
    // 持った絶対パスが消えたら素の名前へ戻る（修正前の起こし方）
    std::fs::remove_file(fx.bin.join("git")).expect("消す");
    assert_eq!(tako_core::git::git_bin(), "git");
}

/// A/B（`TAKO_2519_LEGACY=1`）が効くこと = 修正前の答え（素の名前）を同じ PATH で再現できる
#[cfg(unix)]
#[test]
fn 旧挙動は素の名前のまま持つ() {
    if tako_core::run_alone!(旧挙動は素の名前のまま持つ) {
        return;
    }
    let fx = fixture("2519-legacy");
    isolate_env(&fx);
    std::env::set_var(tako_core::platform::exe::ISSUE2519_LEGACY_ENV, "1");
    assert_eq!(tako_core::git::git_bin(), "git");
    assert_eq!(tako_core::tmux::tmux_bin(), "tmux");
    assert_eq!(
        tako_core::platform::exe::resolve_for_spawn("tako-2519-tool"),
        "tako-2519-tool"
    );
}

/// `fn pick_candidate` の場所（6 の名指し先）
#[cfg(unix)]
fn pick_candidate_location() -> String {
    let src = read(TAILSCALE);
    let (at, _) = fn_window(&src, "pick_candidate").expect("tailscale.rs に pick_candidate が在る");
    format!("{TAILSCALE}:{at}")
}

/// 6 の PATH: 空振りする dir 2 つ + 相対エントリ `rel-bin`。`exe::find_in_process_path` は相対エントリを
/// 見ない（= この PATH では tailscale は「PATH に無い」）が、`posix_spawnp` はカレントディレクトリから見る。
/// その先にだけ偽の tailscale を置き、起こされたら印を残させる = 「PATH に無い素の名前を起こしたか」を
/// 実際の起動の有無で見る（空振りそのものはテストから見えない）。ログインシェルの段は何も答えない
#[cfg(unix)]
struct BareFixture {
    dir: tako_core::test_residue::ScratchDir,
    marker: PathBuf,
    shell: PathBuf,
    path: std::ffi::OsString,
}

#[cfg(unix)]
fn bare_fixture(tag: &str) -> BareFixture {
    let dir = tako_core::test_residue::ScratchDir::new(tag);
    let mut entries = Vec::new();
    for name in ["miss-0", "miss-1"] {
        let entry = dir.path().join(name);
        std::fs::create_dir_all(&entry).expect("一時 dir");
        entries.push(entry);
    }
    std::fs::create_dir_all(dir.path().join("rel-bin")).expect("一時 dir");
    let marker = dir.path().join("spawned");
    tako_core::fake_exec::write_script(
        &dir.path().join("rel-bin").join("tailscale"),
        &format!(
            "#!/bin/sh\necho spawned >> '{}'\necho 1.0.0-fake\nexit 0\n",
            marker.display()
        ),
    );
    let shell = dir.path().join("fake-login-shell");
    tako_core::fake_exec::write_script(&shell, "#!/bin/sh\nexit 1\n");
    let mut path = std::env::join_paths(&entries).expect("PATH");
    path.push(":rel-bin");
    BareFixture {
        dir,
        marker,
        shell,
        path,
    }
}

/// 子の env とカレントディレクトリを 6 の PATH へ揃える（run_alone! の子には他のテストが居ない）
#[cfg(unix)]
fn isolate_tailscale_env(fx: &BareFixture) {
    tako_core::test_child::assert_alone("isolate_tailscale_env");
    std::env::set_current_dir(fx.dir.path()).expect("カレントディレクトリ");
    std::env::set_var("PATH", &fx.path);
    std::env::set_var("SHELL", &fx.shell);
    std::env::set_var("TAKO_TAILSCALE_BIN", "tailscale");
    for key in [
        tako_control::tailscale::ISSUE2513_LEGACY_ENV,
        tako_core::platform::exe::ISSUE2519_LEGACY_ENV,
    ] {
        std::env::remove_var(key);
    }
}

/// 6: このプロセスの PATH に無い素の名前の tailscale を起こさない（GUI の 2 秒ごとの remote の状態の更新が
/// `daemon_status` → `survey_variants` → `find_tailscale` で毎回ここを通る）
#[cfg(unix)]
#[test]
fn pathに無い素の名前のtailscaleは起こさない() {
    if tako_core::run_alone!(pathに無い素の名前のtailscaleは起こさない) {
        return;
    }
    let fx = bare_fixture("2519-ts-bare");
    isolate_tailscale_env(&fx);
    let got = tako_control::tailscale::detect_tailscale();
    assert!(
        !fx.marker.exists(),
        "{}: このプロセスの PATH に無い素の名前の tailscale を素の名前のまま起こした（#2519）。unix では \
         `exe::find_in_process_path` が `posix_spawnp` と同じ PATH を同じ順で見るので、引けない名前を起こしても \
         PATH の空振りしか起きない。Dock から起こした GUI は 2 秒ごとの remote の状態の更新でここを 2 回通り、\
         install の差し替えの後は空振り 1 回ごとに AppleSystemPolicy の拒否が出る（本番で 2 秒ごとに 4 件 × 2）。\
         引けない素の名前は Windows と A/B（`try_unresolved_bare`）のときだけ試すこと",
        pick_candidate_location()
    );
    assert_eq!(got, tako_control::tailscale::Detection::Absent);
}

/// A/B（`TAKO_2519_LEGACY=1`）が効くこと = 修正前の起こし方（引けない素の名前も起こす）を同じ PATH で
/// 再現できる。印が残ること自体が、6 の観測（相対エントリの先の偽物）が生きている証拠を兼ねる
#[cfg(unix)]
#[test]
fn 旧挙動はpathに無い素の名前のtailscaleを起こす() {
    if tako_core::run_alone!(旧挙動はpathに無い素の名前のtailscaleを起こす) {
        return;
    }
    let fx = bare_fixture("2519-ts-legacy");
    isolate_tailscale_env(&fx);
    std::env::set_var(tako_core::platform::exe::ISSUE2519_LEGACY_ENV, "1");
    let got = tako_control::tailscale::detect_tailscale();
    assert!(
        fx.marker.exists(),
        "旧腕で素の名前の tailscale が起こされていない（相対エントリの先の偽物が印を残さない = 6 の観測が死んでいる）"
    );
    assert_eq!(
        got,
        tako_control::tailscale::Detection::Runnable("tailscale".into())
    );
}
