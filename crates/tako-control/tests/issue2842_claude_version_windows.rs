//! Windows の本物の `claude.exe` に対して、起動時の版の控え（`stale_binary::quick_claude_version`）が
//! 版を返すかの実測（#2842）。
//!
//! **本物の claude を使う**（起こすのは `claude --version` だけ）。`scripts/windows/test-claude-mod-1886.ps1` が
//! 砂場へ claude を入れ（製品の口 `tako setup bootstrap install`）、このテストを `--ignored` で名指しで回す。
//! claude を外部（claude.ai）から取るので、CI では `agent_install` の門の中でだけ走る（#1886 の段と同じ）。
//!
//! Windows のランチャ `<home>\.local\bin\claude.exe` は symlink ではなく実体のコピーで、パスに版が入らない。
//! 修正前の `quick_claude_version` はパスからしか版を読まず `None` を返していたので、GUI の起動直後
//! （`initial_claude_mod_hub` が版を控えてから、背景の `bounded_claude_version` が返るまで）に作るペインの
//! 注入判定が `claude_unknown` へ落ちていた。修正後は exe の版リソースから読む。
//!
//! 見るもの（どれも `run_alone!` の子で env を差し替えて、製品の口をそのまま呼ぶ）:
//!
//! | 腕 | env | 期待 |
//! |---|---|---|
//! | home | `USERPROFILE` = 砂場・`HOME` なし・PATH に claude なし（インストーラ直後の形） | `claude --version` と同じ版 |
//! | path | PATH の先頭 = 砂場の `.local\bin` | 同じ版 |
//! | legacy | home の腕 + `TAKO_2842_LEGACY=1`（修正前） | 記録だけ（パスに版が無ければ `None`） |
//!
//! あわせて、控えた版で GUI と同じ注入の判断（`ModHub::decide`）が新旧でどう変わるかと、
//! 背景の判定（`bounded_claude_version`）の答えを記録する。結果は `TAKO2842_*` の行で CI のログへ残す
//! （パスは出さない）。

#![cfg(windows)]

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Instant;

use tako_control::stale_binary;
use tako_core::claude_mod::{self as cm, Injection, ModHub};

/// 砂場に入れた本物の claude.exe（スクリプトが渡す。`<home>\.local\bin\claude.exe`）
const CLAUDE_EXE_ENV: &str = "TAKO_2842_CLAUDE_EXE";
/// claude を入れた砂場の USERPROFILE
const HOME_ENV: &str = "TAKO_2842_HOME";

fn required(name: &str) -> PathBuf {
    PathBuf::from(std::env::var_os(name).unwrap_or_else(|| {
        panic!(
            "{name} が無い。このテストは scripts/windows/test-claude-mod-1886.ps1 が砂場を用意してから --ignored で回す"
        )
    }))
}

/// claude の起動とは無関係の PATH（OS の置き場だけ）
fn system_only_path() -> std::ffi::OsString {
    let root = PathBuf::from(std::env::var_os("SystemRoot").expect("SystemRoot がある"));
    std::env::join_paths([root.join("System32"), root]).expect("PATH を組める")
}

/// 製品の口を env の腕 1 つで呼ぶ（呼ぶたびの所要も返す）
fn quick(path: &std::ffi::OsStr, legacy: bool) -> (Option<String>, u128) {
    tako_core::test_child::assert_alone("quick");
    std::env::set_var("PATH", path);
    if legacy {
        std::env::set_var(stale_binary::QUICK_VERSION_LEGACY_ENV, "1");
    } else {
        std::env::remove_var(stale_binary::QUICK_VERSION_LEGACY_ENV);
    }
    let start = Instant::now();
    let version = stale_binary::quick_claude_version();
    (version, start.elapsed().as_millis())
}

/// GUI の起動時（`initial_claude_mod_hub`）と同じ形の hub で、新しいペインを作るなら注入するか
fn decide_with(version: Option<String>) -> String {
    let mut hub = ModHub::new(true);
    hub.plugin_dir = Some(PathBuf::from(r"C:\tako-2842\claude-mod\tako"));
    hub.cli = Some(PathBuf::from(r"C:\tako-2842\tako.exe"));
    hub.claude_version = version;
    match hub.decide(false) {
        Injection::On { .. } => "on".into(),
        Injection::Off(reason) => format!("off:{}", reason.code()),
    }
}

fn triple(version: Option<&str>) -> Option<(u32, u32, u32)> {
    cm::parse_version(version?)
}

fn has_versions_segment(path: &Path) -> bool {
    path.components()
        .any(|c| c.as_os_str().to_string_lossy() == "versions")
}

#[test]
#[ignore = "本物の claude.exe が要る（scripts/windows/test-claude-mod-1886.ps1 が --ignored で回す）"]
fn issue2842_quick_claude_version_reads_real_claude_exe() {
    if tako_core::run_alone!(issue2842_quick_claude_version_reads_real_claude_exe) {
        return;
    }
    let claude = required(CLAUDE_EXE_ENV);
    let home = required(HOME_ENV);
    assert!(claude.is_file(), "claude.exe が無い");
    assert_eq!(
        claude,
        home.join(".local").join("bin").join("claude.exe"),
        "前提: claude はネイティブインストーラの置き場（<home>\\.local\\bin\\claude.exe）にある"
    );

    // 実機の Windows と同じく HOME は無く、ホームは USERPROFILE（子の中だけ。他のテストは居ない）
    std::env::remove_var("HOME");
    std::env::set_var("USERPROFILE", &home);
    std::env::set_var("DISABLE_AUTOUPDATER", "1");

    // 置き場の形（修正前に読めなかった理由）
    let is_symlink = std::fs::symlink_metadata(&claude)
        .map(|m| m.file_type().is_symlink())
        .unwrap_or(false);
    let canonical =
        tako_core::platform::path::canonicalize(&claude).expect("claude.exe を正規化できる");
    let file_version = tako_core::platform::exe::file_version(&claude);

    // 正解（claude 自身の名乗り）
    let out = Command::new(&claude)
        .arg("--version")
        .stdin(Stdio::null())
        .output()
        .expect("claude --version を起こせること");
    let raw = String::from_utf8_lossy(&out.stdout).trim().to_string();
    let truth =
        cm::parse_version(&raw).unwrap_or_else(|| panic!("claude --version の版を読めない: {raw}"));

    let system = system_only_path();
    let local_bin = home.join(".local").join("bin");
    let mut via_path = std::ffi::OsString::from(&local_bin);
    via_path.push(";");
    via_path.push(&system);

    let (home_new, home_ms) = quick(&system, false);
    let (path_new, path_ms) = quick(&via_path, false);
    let (home_legacy, _) = quick(&system, true);
    std::env::remove_var(stale_binary::QUICK_VERSION_LEGACY_ENV);
    std::env::set_var("PATH", &system);
    let bounded = stale_binary::bounded_claude_version();

    let decide_new = decide_with(home_new.clone());
    let decide_legacy = decide_with(home_legacy.clone());

    println!(
        "TAKO2842_CLAUDE raw=[{raw}] file_version={file_version:?} is_symlink={is_symlink} canonical_has_versions_segment={}",
        has_versions_segment(&canonical)
    );
    println!("TAKO2842_QUICK arm=home new={home_new:?} took_ms={home_ms}");
    println!("TAKO2842_QUICK arm=path new={path_new:?} took_ms={path_ms}");
    println!(
        "TAKO2842_QUICK arm=legacy({}=1) new={home_legacy:?}",
        stale_binary::QUICK_VERSION_LEGACY_ENV
    );
    println!("TAKO2842_BOUNDED version={bounded:?}");
    println!("TAKO2842_DECIDE new={decide_new} legacy={decide_legacy}");

    assert_eq!(
        triple(home_new.as_deref()),
        Some(truth),
        "quick_claude_version（PATH に無く ~\\.local\\bin にだけある claude.exe）が claude --version の版を返さない（#2842）: {home_new:?} / {raw}"
    );
    assert_eq!(
        triple(path_new.as_deref()),
        Some(truth),
        "quick_claude_version（PATH の先頭の claude.exe）が claude --version の版を返さない（#2842）: {path_new:?} / {raw}"
    );
    assert_eq!(
        decide_new, "on",
        "控えた版で注入の判断が on にならない（#2842）"
    );
    if !has_versions_segment(&canonical) && !is_symlink {
        // 修正前の形（パスだけ）では読めない置き場であることの記録（A/B の腕が旧挙動を再現している）
        assert_eq!(home_legacy, None, "A/B の腕が修正前の答えを再現していない");
        assert_eq!(decide_legacy, "off:claude_unknown");
    }
    assert_eq!(
        triple(bounded.as_deref()),
        Some(truth),
        "bounded_claude_version が claude --version の版を返さない: {bounded:?} / {raw}"
    );
    println!(
        "TAKO2842_VERDICT quick_reads_real_claude=true claude={}.{}.{} decide={decide_new} legacy_decide={decide_legacy}",
        truth.0, truth.1, truth.2
    );
}
