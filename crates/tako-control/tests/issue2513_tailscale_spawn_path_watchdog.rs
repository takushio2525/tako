//! #2513 の番犬: 長生きする remote daemon へ tailscale の CLI を**素の名前のまま**渡さない
//!
//! 素の名前の CLI は起こすたびに PATH を手前から空振りする（`posix_spawnp`）。空振り 1 回ごとに
//! カーネルが**親の実行ファイルのまま**の子を一瞬作るので、install の差し替え（`bundle_install` の
//! `.tako-replace-<pid>`）で daemon の実行ファイルが消えた後は、その子が AppleSystemPolicy に
//! 1 本ずつ拒否される（実測: serve の監視 1 周期 2 回 × PATH の手前 11 エントリ = 22 本 / 31 秒。
//! daemon は install も GUI の再起動もまたいで生き残るので、daemon を止めるまで続いた）。
//!
//! 検出の正本（`tailscale::detect_tailscale` → `pick_candidate`）が、PATH で見つけた CLI を
//! **絶対パスで返す**ことを、実ファイルと実 PATH で確かめる。素の名前へ戻すと
//! `pick_candidate` の file:line を名指しして落ちる。

#![cfg(unix)]

use std::ffi::OsString;
use std::path::PathBuf;
use tako_control::tailscale::{detect_tailscale, Detection, ISSUE2513_LEGACY_ENV};
use tako_core::test_residue::ScratchDir;

/// 検出の正本の場所（落ちたときに名指す file:line）
fn pick_candidate_location() -> String {
    let src = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/src/tailscale.rs"))
        .expect("tailscale.rs を読める");
    let line = src
        .lines()
        .position(|l| l.starts_with("fn pick_candidate("))
        .map(|i| i + 1)
        .expect("tailscale.rs に pick_candidate が在る");
    format!("crates/tako-control/src/tailscale.rs:{line}")
}

/// 空振りする 3 つの dir の後ろに偽の `tailscale` を置いた PATH（daemon の PATH の縮小版）
struct Fixture {
    _dir: ScratchDir,
    fake: PathBuf,
    path: OsString,
}

fn fixture(tag: &str) -> Fixture {
    let dir = ScratchDir::new(tag);
    let mut entries = Vec::new();
    for name in ["miss-0", "miss-1", "miss-2", "bin"] {
        let entry = dir.path().join(name);
        std::fs::create_dir_all(&entry).expect("一時 dir");
        entries.push(entry);
    }
    let fake = dir.path().join("bin").join("tailscale");
    tako_core::fake_exec::write_script(
        &fake,
        "#!/bin/sh\n[ \"$1\" = --version ] && echo 1.0.0-fake\nexit 0\n",
    );
    let path = std::env::join_paths(&entries).expect("PATH");
    Fixture {
        _dir: dir,
        fake,
        path,
    }
}

#[test]
fn pathで見つけたtailscaleは絶対パスで返す() {
    if tako_core::run_alone!(pathで見つけたtailscaleは絶対パスで返す) {
        return;
    }
    let fx = fixture("2513-abs");
    // 子の中だけ（run_alone! の子には他のテストが居ない）
    std::env::set_var("PATH", &fx.path);
    std::env::set_var("TAKO_TAILSCALE_BIN", "tailscale");
    std::env::remove_var(ISSUE2513_LEGACY_ENV);

    let got = detect_tailscale();
    assert_eq!(
        got,
        Detection::Runnable(fx.fake.display().to_string()),
        "{}: PATH で見つけた tailscale の CLI を素の名前のまま返している（#2513）。\
         remote daemon はこの答えで 30 秒ごとに tailscale を起こすので、素の名前だと毎回 PATH の\
         手前のエントリを空振りし、install で daemon の実行ファイルが消えた後は空振り 1 回ごとに\
         AppleSystemPolicy の拒否が出る。素の名前の候補は `exe::find_in_process_path` で\
         絶対パスへ引いてから試し、その絶対パスを返すこと",
        pick_candidate_location()
    );
}

/// A/B（`TAKO_2513_LEGACY=1`）が効くこと = 修正前の答え（素の名前）を同じ PATH で再現できる
#[test]
fn 旧挙動は素の名前のまま返す() {
    if tako_core::run_alone!(旧挙動は素の名前のまま返す) {
        return;
    }
    let fx = fixture("2513-legacy");
    std::env::set_var("PATH", &fx.path);
    std::env::set_var("TAKO_TAILSCALE_BIN", "tailscale");
    std::env::set_var(ISSUE2513_LEGACY_ENV, "1");

    assert_eq!(detect_tailscale(), Detection::Runnable("tailscale".into()));
}
