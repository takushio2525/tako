//! Windows のテストが**コンパイルできること**を CI が守っている番犬（#1264）
//!
//! `crates/tako-core/tests/shell_integration_powershell.rs` のように
//! `#![cfg(windows)]` で始まるテストファイルは、macOS では**空クレート**として
//! コンパイルされる。そのうえ CI の Windows ジョブは
//!
//! - `cargo build --workspace` が blocking — だが `tests/` のターゲットは作らない
//! - `cargo test --workspace` は `continue-on-error: true`（#583。POSIX 前提の
//!   既存テストが落ちているあいだテスト結果を非ブロッキングにしていた。
//!   **#1278 で解除した**ので現在は blocking）
//!
//! という組み合わせだったので、**Windows のテストが 1 件も走らない壊れ方が
//! 両ジョブをすり抜けた**（#1264。#1199 の検証ブロックが変数の無い関数へ入り
//! `E0425` × 3 で 102 秒後にコンパイルエラー終了。実機のベースライン照合・
//! フレーク調査が全部止まった）。
//!
//! 手当ては「テストの**コンパイル**だけを blocking にする」1 ステップ
//! （`cargo test --workspace --no-run`）。テスト結果の扱い（#583）は変えない。
//! そのステップが消える / 非ブロッキングへ倒されると同じ穴が開くので、ここで拘束する。
//!
//! ## 現行契約（#1278 で更新）
//!
//! 1. `cargo test --workspace --no-run` = **コンパイルだけ**を見る blocking ステップ（#1264）
//! 2. `cargo test --workspace --no-fail-fast` = **結果まで** blocking（#1278）。
//!    `--no-fail-fast` が要るのは、既定の cargo が最初に落ちたテストバイナリで打ち切り、
//!    その先のクレートを 1 件も走らせないため（実測 2026-09-22 の main: tako-app の
//!    1 件で止まり tako-control / tako-core は 0 件）。全数が採れないと
//!    「直したら次が出る」を 1 PR 1 往復でしか進められない
//! 3. 名指しの小さな実行ステップ（Win32 FFI = #1282 / psmux = #1314）は blocking のまま
//!    **`--workspace` を付けてはいけない**。全数ステップを 1 本に保ち、
//!    「どのステップが何を見たか」をログから読めるようにするため
//! 4. コンパイラの警告 0 件のゲート（#1537 / #1827）= `scripts/warning-gate.sh` に
//!    `cargo build --workspace` と `cargo test --workspace --no-run` の両方を通す blocking ステップ。
//!    Windows の cfg の内側だけで出る警告は macOS の clippy（`-D warnings`）からは見えず、
//!    ゲートが無いあいだに 36 件まで積み上がった。1 の「コンパイル検査」の数には入れない
//! 5. clippy の指摘 0 件のゲート（#2850）= 同じ `scripts/warning-gate.sh` に、macOS ジョブの clippy と
//!    **同じ feature 宇宙**の `cargo clippy` を全部通す blocking ステップ。Windows の cfg の内側は
//!    macOS の clippy から見えず、Windows の clippy は着手時まで一度も回っていなかった（13 件）。
//!    **windows ジョブの段ではなく、並走する別ジョブ `windows-clippy`** に置く（clippy は build の成果物を
//!    使い回せず、段にすると windows ジョブが約 13 分伸びて上限 90 分の手前まで来た）
//!
//! Windows に存在しない仕組みを見るテストは、**理由つきで名指し skip**
//! （`#[cfg(...)]` / `#[cfg_attr(windows, ignore = "理由")]`）へ寄せた。
//! 理由の無い skip が増えないことは `issue1278_ignore_reason_watchdog` が見る。

use std::path::{Path, PathBuf};

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("リポジトリルート")
}

fn ci_yaml() -> String {
    let path = repo_root().join(".github/workflows/ci.yml");
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{} を読めない: {e}", path.display()))
}

/// `windows:` ジョブの本文だけを切り出す（macOS ジョブのステップと混ぜない）。
/// ジョブの終わりは「インデントが 2 未満の行」= 次のトップレベルキー
fn windows_job(ci: &str) -> String {
    let (_, rest) = ci
        .split_once("\n  windows:")
        .expect("CI に windows ジョブが無い（#1264）");
    rest.lines()
        .skip(1)
        .take_while(|line| line.trim().is_empty() || line.starts_with("    "))
        .collect::<Vec<_>>()
        .join("\n")
}

/// ステップ（`      - ` で始まる行）ごとに割る。`- name:` の無い
/// `- uses:` も 1 ステップとして数えるので、`continue-on-error` を隣のステップの
/// ものと読み違えない
fn steps(job: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for line in job.lines() {
        if line.starts_with("      - ") || out.is_empty() {
            out.push(String::new());
        }
        let step = out.last_mut().expect("直前に必ず 1 本ある");
        step.push_str(line);
        step.push('\n');
    }
    out
}

/// 警告ゲート（#1537）の 1 実装。CI の Windows ジョブと `scripts/check-windows.sh` が通す
const WARNING_GATE: &str = "scripts/warning-gate.sh";

/// job id（字下げ 2）を指定してその本文だけを切り出す（[`windows_job`] と同じ読み方）。
/// 見出しの行まで一致させるので、`windows:` と `windows-clippy:` を取り違えない
fn job_body(ci: &str, id: &str) -> String {
    let (_, rest) = ci
        .split_once(&format!("\n  {id}:\n"))
        .unwrap_or_else(|| panic!("CI に {id} ジョブが無い"));
    rest.lines()
        .take_while(|line| line.trim().is_empty() || line.starts_with("    "))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Windows の clippy のゲートを持つ job（#2850。windows ジョブと並走する）
const CLIPPY_JOB: &str = "windows-clippy";

/// コメント行（`#` 始まり）を落とす。説明文の中の文字列に当たって誤検知しないため
fn without_comments(source: &str) -> String {
    source
        .lines()
        .filter(|line| !line.trim_start().starts_with('#'))
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn windowsジョブがテストのコンパイルをblockingで検査する() {
    let ci = ci_yaml();
    let job = without_comments(&windows_job(&ci));
    let steps = steps(&job);
    assert!(
        !steps.is_empty(),
        "windows ジョブのステップを切り出せていない（この番犬の読み取りが壊れている）"
    );

    // 警告ゲート（#1537）も `cargo test --workspace --no-run` を通すが、テストのコンパイルの
    // 担当ではない（済んだ単位の警告を再生させて数えるだけ）ので数に入れない
    let compile: Vec<&String> = steps
        .iter()
        .filter(|s| s.contains("cargo test") && s.contains("--no-run") && !s.contains(WARNING_GATE))
        .collect();
    assert_eq!(
        compile.len(),
        1,
        "Windows ジョブに「テストのコンパイルだけを検査する」ステップが無い（#1264）。\
         `cargo build --workspace` は tests/ を作らず `cargo test` は非ブロッキング（#583）\
         なので、これが無いと Windows のテストが 1 件も走らない壊れ方がすり抜ける\n{job}"
    );
    let compile = compile[0];
    assert!(
        compile.contains("--workspace"),
        "コンパイル検査がワークスペース全体を見ていない（#1264）\n{compile}"
    );
    assert!(
        !compile.contains("continue-on-error"),
        "コンパイル検査が非ブロッキングになっている（#1264。これでは #583 と同じく\
         すり抜ける。**結果**の非ブロッキングは下のテスト実行ステップの担当）\n{compile}"
    );
}

/// #1278: ワークスペース全体のテスト結果が **blocking** で、しかも
/// **全数が採れる形**（`--no-fail-fast`）で回っていること。
///
/// #583 はここを `continue-on-error: true` で据え置いていたが、そのあいだに
/// Windows の未検出が溜まり、実機のベースラインが 19 → 24 件まで増えた（#1278）。
/// 倒し戻されると同じことが起きるので、両方をここで拘束する
#[test]
fn windowsジョブのテスト結果がblockingで全数を採る() {
    let ci = ci_yaml();
    let job = without_comments(&windows_job(&ci));
    let run: Vec<String> = steps(&job)
        .into_iter()
        .filter(|s| s.contains("cargo test") && !s.contains("--no-run"))
        .collect();
    let (workspace, scoped): (Vec<&String>, Vec<&String>) =
        run.iter().partition(|s| s.contains("--workspace"));
    assert_eq!(
        workspace.len(),
        1,
        "Windows ジョブの `cargo test --workspace` 実行ステップが 1 本でない\
         （#1278 / #583 の扱いが変わった？）\n{job}"
    );
    assert!(
        !workspace[0].contains("continue-on-error"),
        "Windows のテスト結果が非ブロッキングへ戻されている（#1278 で blocking 化した。\
         非ブロッキングだと壊れても緑のまま通り、実機のベースラインが黙って増える）\n{}",
        workspace[0]
    );
    assert!(
        workspace[0].contains("--no-fail-fast"),
        "全数が採れない（#1278）。既定の cargo は最初に落ちたテストバイナリで打ち切るので、\
         その先のクレートが 1 件も走らない = 1 回の CI で 1 件しか直せない\n{}",
        workspace[0]
    );
    // #1282 / #1314: 名指しの小さな検査は blocking で置いてよいが、**ワークスペース全体を
    // 巻き込んではいけない**（上で 1 本に絞ってあるので、ここは念のための二重化）
    for step in scoped {
        assert!(
            !step.contains("--workspace"),
            "狭く絞ったはずのテスト実行が --workspace を持っている\n{step}"
        );
    }
}

/// #1282: macOS で 1 行も実行できない Windows 固有コードが、CI で**実際に走る**こと。
///
/// `cargo test --workspace` は tako-app の既知失敗（#583）でそこで打ち切られ
/// tako-core まで届かないので、名指しのステップが無いと Win32 FFI
/// （`NtQueryInformationProcess` でのコマンドライン / `GetProcessTimes` での起動時刻）は
/// **コンパイルされるだけで一度も呼ばれない**。構造体レイアウトの転記ミス・
/// 情報クラス番号・`FILETIME` の起点はそこでしか捕まらない
#[test]
fn windows固有ffiの実行検査がblockingで置かれている() {
    let ci = ci_yaml();
    let job = without_comments(&windows_job(&ci));
    let ffi: Vec<String> = steps(&job)
        .into_iter()
        .filter(|s| s.contains("cargo test") && s.contains("platform::procinfo"))
        .collect();
    assert_eq!(
        ffi.len(),
        1,
        "Windows の FFI を実際に叩く検査ステップが無い（#1282）。\
         `cargo test -p tako-core --lib platform::procinfo` を windows ジョブへ置くこと\n{job}"
    );
    assert!(
        !ffi[0].contains("continue-on-error"),
        "FFI の実行検査が非ブロッキングになっている（#1282。壊れても緑のまま通る）\n{}",
        ffi[0]
    );
}

/// #1537 / #1827: Windows の cfg の内側だけで出るコンパイラの警告を **0 件に保つ** ゲートが、
/// ビルドとテストのコンパイルの両方を見る形で blocking に置かれていること。
///
/// macOS は clippy の `-D warnings` で 0 件を保っているが、Windows の cfg の内側（unix 向けの補助が
/// Windows で未使用 など）は見えない。ゲートが無いあいだに 9 月の 31 件から 11 月の 36 件まで
/// 積み上がり、新しい警告が埋もれていた。
///
/// - `cargo build` も数える: 統合テストの無い tako-app の**非テストの bin** は `--no-run` では
///   作られないので、製品コードの dead_code（11 月の 36 件のうち 11 件）が見えなくなる
/// - テストのコンパイルが通っていれば、テストの全数が赤でも走る（`!cancelled()` + `test-build`）
/// - Windows ジョブで `RUSTFLAGS` を env に置かない: `.cargo/config.toml` の
///   `[target.x86_64-pc-windows-msvc] rustflags`（`+crt-static` = #587）を丸ごと上書きし、
///   配布物が VC++ の DLL を要求する形に変わる（`-D warnings` を RUSTFLAGS で渡す形へ「簡単に」しない）
/// - 手元の `scripts/check-windows.sh` も同じ 1 実装を通す（手元と CI で数え方をずらさない）
#[test]
fn windowsジョブがコンパイラの警告を0件に保つ() {
    let ci = ci_yaml();
    let raw = windows_job(&ci);
    let job = without_comments(&raw);
    let steps = steps(&job);
    // clippy の段（#2850）も同じゲートを通すので、`cargo build` を数える段だけを拾う
    let gate: Vec<(usize, &String)> = steps
        .iter()
        .enumerate()
        .filter(|(_, s)| s.contains(&format!("{WARNING_GATE} cargo build")))
        .collect();
    assert_eq!(
        gate.len(),
        1,
        "Windows ジョブに警告 0 件のゲート（{WARNING_GATE}）の段が 1 本ない（#1537）。\
         ゲートが無いと Windows の cfg の内側だけの警告が積み上がる\n{job}"
    );
    let (gate_at, gate) = gate[0];
    for command in ["cargo build --workspace", "cargo test --workspace --no-run"] {
        assert!(
            gate.contains(&format!("{WARNING_GATE} {command}")),
            "警告ゲートが `{command}` を数えていない（#1537。build は tako-app の非テストの bin、\
             test --no-run はテストの cfg の内側と統合テストの担当）\n{gate}"
        );
    }
    assert!(
        !gate.contains("continue-on-error"),
        "警告ゲートが非ブロッキングになっている（#1537。警告が積み上がった元の形）\n{gate}"
    );
    assert!(
        gate.contains("exit \"$rc\""),
        "警告ゲートが片方の結果を捨てている（#1537。`shell: bash` は最初の失敗で抜けるので、\
         両方を回してから終了コードを返す）\n{gate}"
    );
    assert!(
        gate.contains("!cancelled()") && gate.contains("steps.test-build.outcome == 'success'"),
        "警告ゲートがテストのコンパイルの成否だけで走る形になっていない（#1537。\
         テストが赤でも警告の結果を同じ CI で読むため）\n{gate}"
    );
    let compile_at = steps
        .iter()
        .position(|s| s.contains("id: test-build"))
        .expect("テストのコンパイルの段に id: test-build が無い（#1537 のゲートの if が参照する）");
    assert!(
        steps[compile_at].contains("cargo test --workspace --no-run"),
        "id: test-build がテストのコンパイルの段に付いていない\n{}",
        steps[compile_at]
    );
    let full_at = steps
        .iter()
        .position(|s| s.contains("cargo test --workspace --no-fail-fast"))
        .expect("テストの全数の段がある（#1278）");
    assert!(
        compile_at < full_at && full_at < gate_at,
        "警告ゲートはテストのコンパイルと全数の後ろに置く（#1537。再生させる単位が揃ってから数える）\n{job}"
    );
    assert!(
        !job.contains("RUSTFLAGS"),
        "Windows ジョブが RUSTFLAGS を置いている（#1537。.cargo/config.toml の +crt-static = #587 を\
         上書きする。警告は {WARNING_GATE} で数える）\n{job}"
    );
    let check = std::fs::read_to_string(repo_root().join("scripts/check-windows.sh"))
        .expect("scripts/check-windows.sh を読める");
    assert!(
        without_comments(&check).contains(WARNING_GATE),
        "scripts/check-windows.sh が警告ゲートを通していない（#1537。手元の xwin check と CI で\
         数え方がずれる）"
    );
}

/// #2850: Windows の cfg の内側を clippy の指摘 0 件に保つ。
///
/// - **windows ジョブの段ではなく、並走する別ジョブ [`CLIPPY_JOB`] に置く**: clippy は check の単位で組むので
///   windows ジョブのビルドの成果物を使い回せず、段にすると windows ジョブが約 13 分伸びる（PR #2862 の実測で
///   81.5 分 = 上限 90 分の手前）。windows ジョブへ戻したらここで落ちる
/// - macOS ジョブの clippy と**同じ feature 宇宙を全部**回す（片側だけ宇宙を足し引きしたらここで落ちる。
///   着手時の 13 件のうち 1 件は visual-test の宇宙にしか出なかった）
/// - `-- -D warnings` を付けない: 最初に指摘の出たクレートで止まり、依存するクレートが検査されない。
///   件数と位置はゲート（[`WARNING_GATE`]）が数えて名指す（CI の注釈にも出る）
/// - Windows のランナーで回し、走らせる判定（`if:`）は macOS ジョブの lint と同じ行にする
///   （docs だけなら skip = #2084・定期実行では走らない）
/// - ツールチェーンに clippy を入れ（dtolnay の既定は minimal で clippy が無い）、版は windows ジョブと揃える
/// - 手元の `scripts/check-windows.sh` も `cargo xwin clippy` で同じ宇宙を回す
#[test]
fn windowsのclippyの指摘を並走するジョブで0件に保つ() {
    let ci = ci_yaml();
    let windows = without_comments(&windows_job(&ci));
    assert!(
        !windows.contains("cargo clippy"),
        "windows ジョブに clippy の段がある（#2850。clippy は build の成果物を使い回せず、段にすると windows ジョブが\
         約 13 分伸びて上限 90 分の手前まで来る。並走する {CLIPPY_JOB} ジョブに置く）\n{windows}"
    );
    let job = without_comments(&job_body(&ci, CLIPPY_JOB));
    let mac = without_comments(&job_body(&ci, "macos"));

    // ジョブの見出し: Windows のランナー・変更範囲の判定を待つ・走らせる判定は macOS の lint と同じ行
    let header = |body: &str, key: &str| -> Option<String> {
        body.lines()
            .find(|line| line.starts_with(&format!("    {key}:")))
            .map(|line| line.trim_end().to_string())
    };
    assert_eq!(
        header(&job, "runs-on").as_deref(),
        Some("    runs-on: windows-latest"),
        "{CLIPPY_JOB} ジョブが Windows のランナーで回っていない（#2850。Windows の cfg の内側は macOS の clippy から見えない）\n{job}"
    );
    assert_eq!(
        header(&job, "needs").as_deref(),
        Some("    needs: changes"),
        "{CLIPPY_JOB} ジョブが変更範囲の判定（#2084）を待っていない\n{job}"
    );
    let mac_if = header(&mac, "if").expect("macOS ジョブに走らせる判定（if:）がある（#2084）");
    assert_eq!(
        header(&job, "if"),
        Some(mac_if.clone()),
        "{CLIPPY_JOB} ジョブの走らせる判定が macOS ジョブの lint と違う（#2850。docs だけなら skip = #2084・\
         定期実行では走らない、に揃える）\n{job}"
    );

    let job_steps = steps(&job);
    let gate: Vec<&String> = job_steps
        .iter()
        .filter(|s| s.contains(&format!("{WARNING_GATE} cargo clippy")))
        .collect();
    assert_eq!(
        gate.len(),
        1,
        "{CLIPPY_JOB} ジョブに clippy の指摘 0 件のゲート（{WARNING_GATE} cargo clippy）の段が 1 本ない（#2850）。\
         Windows の cfg の内側は macOS の clippy から見えない\n{job}"
    );
    let gate = gate[0];

    // macOS ジョブの clippy の宇宙（`run: cargo clippy … -- -D warnings`）をすべて Windows でも回す
    let universes: Vec<String> = mac
        .lines()
        .filter_map(|line| line.trim().strip_prefix("run: "))
        .filter(|cmd| cmd.starts_with("cargo clippy"))
        .map(|cmd| cmd.trim_end_matches("-- -D warnings").trim().to_string())
        .collect();
    assert!(
        universes.len() >= 3,
        "macOS ジョブの clippy の宇宙を読み取れない（この番犬の読み取りが壊れている）: {universes:?}"
    );
    for universe in &universes {
        assert!(
            gate.contains(&format!("{WARNING_GATE} {universe}")),
            "Windows の clippy のゲートが macOS の宇宙 `{universe}` を回していない（#2850。\
             feature の統合で片方の宇宙にしか出ない指摘がある）\n{gate}"
        );
    }
    assert!(
        !gate.contains("-D warnings"),
        "Windows の clippy のゲートに -D warnings が付いている（#2850。最初に指摘の出たクレートで止まり、\
         依存するクレートが検査されない。数えるのは {WARNING_GATE}）\n{gate}"
    );
    assert!(
        !job.contains("continue-on-error"),
        "Windows の clippy のゲートが非ブロッキングになっている（#2850）\n{job}"
    );
    assert!(
        gate.contains("exit \"$rc\""),
        "Windows の clippy のゲートが途中の宇宙の結果を捨てている（#2850。全部回してから終了コードを返す）\n{gate}"
    );

    // ツールチェーン: clippy を入れ、版は windows ジョブ（build / test）と揃える
    let toolchain_of = |body: &str| -> String {
        steps(body)
            .into_iter()
            .find(|s| s.contains("dtolnay/rust-toolchain"))
            .unwrap_or_else(|| panic!("ツールチェーンの段が無い\n{body}"))
    };
    let toolchain = toolchain_of(&job);
    assert!(
        toolchain.contains("components:") && toolchain.contains("clippy"),
        "{CLIPPY_JOB} ジョブのツールチェーンに clippy が入っていない（#2850。dtolnay の既定は minimal）\n{toolchain}"
    );
    let version = |step: &str| -> Option<String> {
        step.lines()
            .find_map(|line| line.trim().strip_prefix("toolchain: "))
            .map(str::to_string)
    };
    let windows_toolchain = toolchain_of(&windows);
    assert!(
        version(&toolchain).is_some() && version(&toolchain) == version(&windows_toolchain),
        "{CLIPPY_JOB} ジョブと windows ジョブのツールチェーンの版がずれている（#2850。lint と build / test で\
         違う rustc を見ることになる）\n{toolchain}\n{windows_toolchain}"
    );

    let check = without_comments(
        &std::fs::read_to_string(repo_root().join("scripts/check-windows.sh"))
            .expect("scripts/check-windows.sh を読める"),
    );
    assert!(
        check.contains("cargo xwin clippy"),
        "scripts/check-windows.sh が clippy で見ていない（#2850。手元と CI で見る lint がずれる）"
    );
    for universe in &universes {
        let args = universe.trim_start_matches("cargo clippy").trim();
        assert!(
            check.contains(args),
            "scripts/check-windows.sh が macOS / CI の宇宙 `{universe}` を回していない（#2850）"
        );
    }
}
