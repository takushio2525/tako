//! #2539 の番犬: GUI の 2 秒ごとの remote の状態の更新は tailscale を起こさない
//!
//! remote daemon が動いている間、GUI の 2 秒ごとの `refresh_remote_state` は daemon の生死と admin state
//! だけを使う。修正前は生死の判定に `remote::daemon_status` を丸ごと引き（そのまま 1 回 + `admin_request`
//! の中で 1 回）、そのたびに表示の欄（2 系統の同居の警告）のための `tailscale::survey_variants` が
//! tailscale の CLI を最大 3 本（`--version` / `status --json` / `--socket … status --json`）起こしていた
//! = **2 秒ごとに 6 本**。GUI は調べた結果を 1 つも使っていなかった。
//!
//! 2 段で縛る:
//! 1. 走査: 状態の更新が通る口（[`POLL_PATH`]）が `daemon_status` や tailscale の検出を引いていれば、
//!    その file:line を名指す。GUI は検査した口（`remote::poll_admin_state`）を通ること
//! 2. 実測（unix）: 偽の tailscale（呼ばれるたびに記録する）と生きている PID ファイルで
//!    `poll_admin_state` を回し、1 本も起こさないこと。A/B（`TAKO_2539_LEGACY=1`）は同じ条件で
//!    修正前の本数（1 tick に系統の調査 2 回）を再現する。`tako remote status`（= `daemon_status`）は
//!    今も系統を調べる（表示の欄を変えない）
//!
//! [`逆戻りを名指しできる`] で、修正前の形を注入した本文が実際に名指しされることを確かめる。

use std::path::{Path, PathBuf};

#[path = "common/production_range.rs"]
mod production_range;

use production_range::code_view::without_comments;

const REMOTE: &str = "crates/tako-control/src/remote.rs";
const PANEL: &str = "crates/tako-app/src/remote_panel.rs";

/// GUI の 2 秒ごとの状態の更新が daemon の稼働中に毎回通る口（ファイル・関数）。
/// ここに載せた関数は [`FORBIDDEN`] を直接引いてはならない
const POLL_PATH: &[(&str, &str)] = &[
    (PANEL, "refresh_remote_state"),
    (REMOTE, "poll_admin_state"),
    (REMOTE, "daemon_running"),
    (REMOTE, "liveness"),
    (REMOTE, "admin_request"),
];

/// 状態の更新の口から引いてはならないもの（引くと tailscale の CLI を起こす）
const FORBIDDEN: &[&str] = &[
    "daemon_status(",
    "survey_variants(",
    "find_tailscale(",
    "detect_tailscale(",
    "setup_status_on(",
    "resolve_serve_handle(",
];

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

/// 1 の検査（本文を受け取る形 = 注入した本文でも同じ検査を回せる）
fn scan(rel: &str, func: &str, src: &str) -> Vec<String> {
    let Some((at, range)) = fn_window(src, func) else {
        return vec![format!(
            "{rel} — `fn {func}` が見つからない（走査が空振り。名前を変えたらこの表も直す）"
        )];
    };
    let window = &src[range.clone()];
    let mut out = Vec::new();
    for needle in FORBIDDEN {
        let mut from = 0;
        while let Some(found) = window[from..].find(needle) {
            let hit = from + found;
            // `x.daemon_status(` のようなメソッドや別名の一部は数えない（識別子の頭だけ）
            let head = window[..hit]
                .chars()
                .next_back()
                .is_none_or(|c| !(c.is_alphanumeric() || c == '_'));
            if head {
                out.push(format!(
                    "{rel}:{} — `fn {func}` が `{}` を引いている",
                    at + line_of(window, hit) - 1,
                    needle.trim_end_matches('(')
                ));
            }
            from = hit + needle.len();
        }
    }
    if func == "refresh_remote_state" && !window.contains("poll_admin_state(") {
        out.push(format!(
            "{rel}:{at} — `fn {func}` が `remote::poll_admin_state` を通っていない（番犬の実測が GUI の中身を見ていない）"
        ));
    }
    out
}

#[test]
fn 状態の更新の口はtailscaleを起こすものを引かない() {
    let mut offenders = Vec::new();
    for (rel, func) in POLL_PATH {
        offenders.extend(scan(rel, func, &read(rel)));
    }
    assert!(
        offenders.is_empty(),
        "GUI の 2 秒ごとの remote の状態の更新が通る口で、tailscale の CLI を起こすものを引いている（#2539）。\
         `daemon_status` は表示の欄（2 系統の同居の警告）のために tailscale を最大 3 本起こすので、生死だけを\
         知りたい口は `remote::daemon_running` を引くこと（修正前は daemon の稼働中、2 秒ごとに 6 本）:\n  {}",
        offenders.join("\n  ")
    );
}

/// 番犬そのものの検査: 修正前の形を注入した本文が名指しされる（見逃す側へ倒れていない）
#[test]
fn 逆戻りを名指しできる() {
    // 1: GUI の状態の更新を修正前の形（daemon_status で生死を見てから admin_request）へ戻す
    let src = read(PANEL);
    let fixed = "tako_control::remote::poll_admin_state()";
    let at = src
        .find(fixed)
        .expect("注入の目印が remote_panel.rs に無い（番犬の注入を追従させること)");
    let injected = src.replacen(
        fixed,
        "if tako_control::remote::daemon_status()[\"running\"].as_bool() == Some(true) { \
         tako_control::remote::admin_request(\"GET\", \"/api/admin/state\", None) } else { todo!() }",
        1,
    );
    let hits = scan(PANEL, "refresh_remote_state", &injected);
    let want = format!(
        "{PANEL}:{} — `fn refresh_remote_state` が `daemon_status`",
        line_of(&src, at)
    );
    assert!(
        hits.iter().any(|h| h.starts_with(&want)),
        "GUI の状態の更新へ daemon_status を戻しても {want} で名指しされない: {hits:?}"
    );
    assert!(
        hits.iter()
            .any(|h| h.contains("poll_admin_state` を通っていない")),
        "GUI が検査した口を外れても名指しされない: {hits:?}"
    );

    // 2: admin_request の中の生死の判定を daemon_status へ戻す
    let src = read(REMOTE);
    let fixed = "if !daemon_running() {";
    let at = src
        .find(fixed)
        .expect("注入の目印が remote.rs の admin_request に無い");
    let injected = src.replacen(
        fixed,
        "if daemon_status()[\"running\"].as_bool() != Some(true) {",
        1,
    );
    let hits = scan(REMOTE, "admin_request", &injected);
    let want = format!(
        "{REMOTE}:{} — `fn admin_request` が `daemon_status`",
        line_of(&src, at)
    );
    assert!(
        hits.iter().any(|h| h.starts_with(&want)),
        "admin_request へ daemon_status を戻しても {want} で名指しされない: {hits:?}"
    );

    // 3: 生死の判定そのものが tailscale の検出を毎回やり直す形
    let head = "fn liveness() -> Liveness {";
    let at = src.find(head).expect("remote.rs に liveness が在る");
    let mut injected = src.clone();
    injected.insert_str(
        at + head.len(),
        "\n    let _ = crate::tailscale::survey_variants();",
    );
    let hits = scan(REMOTE, "liveness", &injected);
    let want = format!(
        "{REMOTE}:{} — `fn liveness` が `survey_variants`",
        line_of(&src, at) + 1
    );
    assert!(
        hits.iter().any(|h| h.starts_with(&want)),
        "liveness へ系統の調査を足しても {want} で名指しされない: {hits:?}"
    );

    // 識別子の一部（`legacy_running_via_status(` の中の `status(` など）は数えない
    assert!(
        scan(REMOTE, "daemon_running", &src).is_empty(),
        "A/B の口（legacy_running_via_status）を名指ししている = 走査が広すぎる"
    );
}

/// 偽の tailscale（呼ばれるたびに引数を 1 行記録する）と、生きている PID ファイル（このテストの子の pid）
#[cfg(unix)]
mod measured {
    use super::*;
    use tako_control::remote::ISSUE2539_LEGACY_ENV;
    pub(super) use tako_control::remote::{daemon_running, daemon_status, poll_admin_state};
    use tako_core::test_residue::ScratchDir;

    pub(super) struct Fixture {
        _dir: ScratchDir,
        pub(super) state: PathBuf,
        pub(super) calls: PathBuf,
    }

    impl Fixture {
        /// 記録された tailscale の呼び出し
        pub(super) fn calls(&self) -> Vec<String> {
            std::fs::read_to_string(&self.calls)
                .unwrap_or_default()
                .lines()
                .map(str::to_string)
                .collect()
        }
        pub(super) fn reset(&self) {
            std::fs::write(&self.calls, "").expect("記録を空にできる");
        }
    }

    /// `run_alone!` の子の中だけで呼ぶ（env を書き換える）
    pub(super) fn fixture(tag: &str, pid: u32) -> Fixture {
        tako_core::test_child::assert_alone("measured::fixture");
        let dir = ScratchDir::new(tag);
        let state = dir.path().join("state");
        std::fs::create_dir_all(&state).expect("一時の state dir");
        let calls = dir.path().join("calls.log");
        std::fs::write(&calls, "").expect("記録の置き場");
        let fake = dir.path().join("tailscale");
        tako_core::fake_exec::write_script(
            &fake,
            &format!(
                "#!/bin/sh\necho \"$*\" >> '{}'\n\
                 [ \"$1\" = --version ] && {{ echo 1.0.0-fake; exit 0; }}\n\
                 echo '{{\"BackendState\":\"Running\",\"Version\":\"1.0.0-fake\",\
                 \"CertDomains\":[\"fake-2539.tailfake.ts.net\"],\
                 \"Self\":{{\"DNSName\":\"fake-2539.tailfake.ts.net.\"}}}}'\nexit 0\n",
                calls.display()
            ),
        );
        // PID ファイル（PID / 実行ファイル / 起動時刻）。掃除は隔離の印で止める（#445）
        std::fs::write(
            state.join("tako-remote.pid"),
            format!("{pid}\n/nonexistent/tako-2539\n0\n"),
        )
        .expect("PID ファイル");
        std::env::set_var("TAKO_REMOTE_STATE_DIR", &state);
        std::env::set_var("TAKO_TAILSCALE_BIN", &fake);
        std::env::set_var("TAKO_ISOLATED", "1");
        std::env::remove_var(ISSUE2539_LEGACY_ENV);
        std::env::remove_var("TAKO_TAILSCALE_SOCKET");
        assert!(
            state.starts_with(std::env::temp_dir())
                || state.starts_with("/tmp")
                || state.starts_with("/private"),
            "state dir が一時 dir の外: {}",
            state.display()
        );
        Fixture {
            _dir: dir,
            state,
            calls,
        }
    }

    /// 系統の調査 1 回で起こす本数（`--version` + `status --json` + standalone の socket があれば
    /// `--socket … status --json`）。socket の有無は機による（本番の Mac は在る = 3 本）
    pub(super) fn per_survey() -> usize {
        2 + usize::from(tako_control::tailscale::standalone_socket_path().is_some())
    }

    pub(super) fn location(rel: &str, func: &str) -> String {
        let (at, _) = fn_window(&read(rel), func).expect("関数が在る");
        format!("{rel}:{at}")
    }
}

#[cfg(unix)]
#[test]
fn guiの状態の更新はtailscaleを起こさない() {
    if tako_core::run_alone!(guiの状態の更新はtailscaleを起こさない) {
        return;
    }
    use measured::*;
    let fx = fixture("2539-poll", std::process::id());
    assert!(
        daemon_running(),
        "生きている PID ファイルなのに daemon_running が false"
    );
    fx.reset();
    // GUI の 3 tick ぶん（待ち受けの記録は無いので admin API までは届かない = 起こす数だけを見る）
    for _ in 0..3 {
        let _ = poll_admin_state();
    }
    let calls = fx.calls();
    assert!(
        calls.is_empty(),
        "{} / {}: GUI の状態の更新（3 tick）が tailscale を {} 本起こした（#2539。修正前は 1 tick に \
         系統の調査 2 回 = {} 本 / tick）。生死の判定は `daemon_running`（PID ファイルだけ）で見ること: {calls:?}",
        location(PANEL, "refresh_remote_state"),
        location(REMOTE, "poll_admin_state"),
        calls.len(),
        2 * per_survey()
    );
}

/// A/B（`TAKO_2539_LEGACY=1`）が効くこと = 修正前の本数（1 tick に系統の調査 2 回）を同じ条件で再現できる
#[cfg(unix)]
#[test]
fn 旧挙動は1tickに系統の調査を2回() {
    if tako_core::run_alone!(旧挙動は1tickに系統の調査を2回) {
        return;
    }
    use measured::*;
    let fx = fixture("2539-legacy", std::process::id());
    std::env::set_var(tako_control::remote::ISSUE2539_LEGACY_ENV, "1");
    fx.reset();
    let _ = poll_admin_state();
    let calls = fx.calls();
    assert_eq!(
        calls.len(),
        2 * per_survey(),
        "旧挙動の 1 tick は系統の調査 2 回のはず: {calls:?}"
    );
    assert_eq!(
        calls.iter().filter(|c| c.as_str() == "--version").count(),
        2,
        "検出（--version）を 1 tick に 2 回やり直すのが旧挙動: {calls:?}"
    );
}

/// `tako remote status`（= `daemon_status`）は今も系統を調べる（表示の欄 = 2 系統の同居の警告を変えない）。
/// 生死の答えはどの状態でも `daemon_status()["running"]` と同じ
#[cfg(unix)]
#[test]
fn statusは今も系統を調べ生死の答えは同じ() {
    if tako_core::run_alone!(statusは今も系統を調べ生死の答えは同じ) {
        return;
    }
    use measured::*;
    // 生きている
    let fx = fixture("2539-status", std::process::id());
    fx.reset();
    let st = daemon_status();
    assert_eq!(st["running"], serde_json::json!(true));
    assert_eq!(
        fx.calls().len(),
        per_survey(),
        "daemon_status は系統を 1 回調べる（表示の欄を落としていない）: {:?}",
        fx.calls()
    );
    assert_eq!(daemon_running(), st["running"].as_bool() == Some(true));

    // 死んでいる（終わった子の pid。隔離の印があるので残骸は消さない）
    let mut child = std::process::Command::new("/usr/bin/true")
        .spawn()
        .expect("true を起こせる");
    let dead = child.id();
    child.wait().expect("true が終わる");
    std::fs::write(
        fx.state.join("tako-remote.pid"),
        format!("{dead}\n/nonexistent/tako-2539\n0\n"),
    )
    .expect("PID ファイル");
    fx.reset();
    assert!(!daemon_running(), "死んだ pid を生きていると読んだ");
    assert_eq!(daemon_status()["running"], serde_json::json!(false));
    assert!(
        fx.calls().is_empty(),
        "死んだ daemon の判定で tailscale を起こした"
    );
    assert!(
        fx.state.join("tako-remote.pid").exists(),
        "隔離の印があるのに残骸を消した（#445）"
    );

    // PID ファイルが無い
    std::fs::remove_file(fx.state.join("tako-remote.pid")).expect("PID ファイルを消せる");
    assert!(!daemon_running());
    assert_eq!(daemon_status()["running"], serde_json::json!(false));
}
