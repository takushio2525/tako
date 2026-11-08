//! #2433 の番犬: Windows の依存の導入代行で「UAC が出うるか」を**依存表の 1 か所**だけが決め、
//! 同意なしで UAC を起こす形へ戻さない。
//!
//! 依存ごとの権限（psmux = 利用者の権限 / Git・Tailscale = UAC）は `setup_deps::deps` /
//! `container_dep` の `InstallPrivilege` で宣言し、同意を取るかは `DepInstallOptions::consent` と
//! `InstallPrivilege::needs_consent` だけで決まる。ここが崩れる典型は次の 3 つで、どれも
//! `file:line` で名指して落とす:
//!
//! 1. **依存の名前での分岐**（`if state.dep.bin == "git" { … }`）を足す。権限の宣言が 2 か所に割れ、
//!    表を直しても片方が古いまま残る
//! 2. **`consent: true` を直書きする**（dispatch の既定・設定画面・CLI）。利用者の同意なしで UAC が出る。
//!    直書きしてよいのは `tako setup` の依存の段（`offer_and_install`）が y / `--yes` を受けた後だけ
//! 3. **権限の値を依存表の外で組む**（`InstallPrivilege::Elevated` を他のファイルで作って判断に使う）
//!
//! あわせて、Windows の CI が製品の口で winget の実走（外部から取る = 関連パスの PR と定期・手動だけ）と
//! 偽の winget での断りの検査（毎回）を回していることを縛る。

#[path = "common/production_range.rs"]
mod production_range;

use production_range::code_view;
use std::path::{Path, PathBuf};

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("リポジトリルートを解決できない")
        .to_path_buf()
}

fn read(rel: &str) -> String {
    std::fs::read_to_string(repo_root().join(rel)).unwrap_or_else(|e| panic!("{rel} が読める: {e}"))
}

const SETUP_DEPS: &str = "crates/tako-control/src/setup_deps.rs";

/// 依存表に載っている依存の名前（Windows / macOS の器・git・tailscale）
const DEP_NAMES: [&str; 4] = ["tmux", "psmux", "git", "tailscale"];

/// 依存の名前を値として持つ識別子らしいか（`bin` / `dep.bin` / `state.dep.bin` / `dep_name` …）
fn mentions_dep_ident(line: &str) -> bool {
    line.split(|c: char| !(c.is_alphanumeric() || c == '_'))
        .any(|w| w == "bin" || w == "dep" || w.ends_with("_bin") || w.starts_with("dep_"))
}

/// 1 行が「依存の名前のリテラルとの比較」か（**純粋関数**。下の自己検査で当たり方を縛る）
fn compares_dep_name(line: &str) -> bool {
    if !mentions_dep_ident(line) {
        return false;
    }
    // 比べ方（前に置く記号 / 後に置く記号）。`format!` に `=` と `{` を並べて書くと tmux の完全一致の
    // ターゲットの直書きと見分けられない（platform_parity の番犬）ので、記号は値として渡す
    const BEFORE: [&str; 5] = ["==", "!=", "Some(", ",", "|"];
    const AFTER: [&str; 3] = ["==", "!=", "|"];
    DEP_NAMES.iter().any(|name| {
        let lit = ["\"", name, "\""].concat();
        BEFORE.iter().any(|op| {
            let sep = if op.ends_with('(') { "" } else { " " };
            line.contains(&[op, sep, lit.as_str()].concat())
        }) || AFTER
            .iter()
            .any(|op| line.contains(&[lit.as_str(), " ", op].concat()))
    })
}

/// 1 行が依存の名前の match の腕か（`"git" =>`）
fn dep_name_arm(line: &str) -> bool {
    let t = line.trim_start();
    DEP_NAMES
        .iter()
        .any(|name| t.starts_with(&format!("\"{name}\"")) && t.contains("=>"))
}

/// 同意を直書きしているか
fn hardcodes_consent(line: &str) -> bool {
    let compact: String = line.chars().filter(|c| !c.is_whitespace()).collect();
    compact.contains("consent:true") || compact.contains("consent:Some(true)")
}

/// 権限の値を組んでいるか
fn builds_privilege(line: &str) -> bool {
    line.contains("InstallPrivilege::Elevated") || line.contains("InstallPrivilege::User")
}

/// 製品コード（テスト領域とコメントを潰した眺め。文字列は残す）の 1 行ずつ
fn product_lines(rel: &str) -> Vec<String> {
    let raw = read(rel);
    let prod = production_range::scan(&raw).text;
    code_view::without_comments(&prod)
        .lines()
        .map(str::to_string)
        .collect()
}

/// 関数の本体の行の範囲（1 始まり・両端を含む）。`fn <name>(` の行から対応する `}` まで
fn fn_span(lines: &[String], name: &str) -> (usize, usize) {
    let head = lines
        .iter()
        .position(|l| tako_core::source_scan::fn_head_name(l) == Some(name))
        .unwrap_or_else(|| panic!("{SETUP_DEPS} に fn {name} が無い（番犬の許可の範囲を直す）"));
    let mut depth = 0i32;
    let mut opened = false;
    for (i, l) in lines.iter().enumerate().skip(head) {
        for c in l.chars() {
            match c {
                '{' => {
                    depth += 1;
                    opened = true;
                }
                '}' => depth -= 1,
                _ => {}
            }
        }
        if opened && depth == 0 {
            return (head + 1, i + 1);
        }
    }
    panic!("{SETUP_DEPS} の fn {name} の終わりが見つからない");
}

fn within(line: usize, spans: &[(usize, usize)]) -> bool {
    spans.iter().any(|&(a, b)| (a..=b).contains(&line))
}

/// 番犬の検出そのものの自己検査（修正前の形・散らばった分岐を確かに当てる / 正しい形を当てない）
#[test]
fn issue2433_検出の自己検査() {
    for hit in [
        "    if state.dep.bin == \"git\" {",
        "    let elevated = dep.bin == \"tailscale\";",
        "    if \"psmux\" != bin { return; }",
        "    if matches!(state.dep.bin, \"git\" | \"tailscale\") {",
        "    let uac = matches!(bin, \"git\");",
        "    if dep_bin == Some(\"git\") {",
    ] {
        assert!(compares_dep_name(hit), "当たるべき: {hit}");
    }
    for miss in [
        "    if installer.privilege().needs_consent() && !consent {",
        "    let found = tako_core::platform::exe::find(\"git\");",
        "    status_of(\"tailscale\")",
        "    if name == \"tailscale\" {",
    ] {
        assert!(!compares_dep_name(miss), "当たらないべき: {miss}");
    }
    assert!(dep_name_arm(
        "        \"git\" => InstallPrivilege::Elevated,"
    ));
    assert!(!dep_name_arm("        Some(\"git\") => {}"));
    for hit in [
        "consent: true,",
        "consent: Some(true),",
        "  consent:Some( true )",
    ] {
        assert!(hardcodes_consent(hit), "{hit}");
    }
    for miss in [
        "consent: consent.unwrap_or(false),",
        "consent: Some(elevated),",
        "consent,",
    ] {
        assert!(!hardcodes_consent(miss), "{miss}");
    }
}

/// 1 と 3: 依存の名前での分岐と権限の値の組み立ては、依存表とその読み方の関数の中だけ
#[test]
fn issue2433_権限の判断は依存表の1か所() {
    let files = production_range::product_sources();
    let test_files = production_range::test_only_files(&files);
    // 依存の名前を比べてよいのは検出の規則（器は名前の解決が違う・tailscale は検出の正本が別）だけ
    let setup_lines = product_lines(SETUP_DEPS);
    let name_ok = [
        fn_span(&setup_lines, "is_container"),
        fn_span(&setup_lines, "is_tailscale"),
    ];
    // 権限の値を組んでよいのは依存表（宣言）と、その値を読む関数（引数・既定・画面の行）だけ
    let privilege_ok = [
        fn_span(&setup_lines, "container_dep"),
        fn_span(&setup_lines, "deps"),
        fn_span(&setup_lines, "privilege"),
        fn_span(&setup_lines, "args"),
        fn_span(&setup_lines, "from_status"),
    ];
    // 腕で依存の名前を分けるのを見張るのは、導入と同意の経路のファイル
    let arm_watched = [
        SETUP_DEPS,
        "crates/tako-cli/src/setup.rs",
        "crates/tako-app/src/settings_window.rs",
        "crates/tako-control/src/remote_setup.rs",
    ];
    let mut bad = Vec::new();
    let mut scanned = 0usize;
    for rel in files.iter().filter(|f| !test_files.contains(*f)) {
        scanned += 1;
        let lines = product_lines(rel);
        for (i, line) in lines.iter().enumerate() {
            let n = i + 1;
            let in_setup = rel == SETUP_DEPS;
            if compares_dep_name(line) && !(in_setup && within(n, &name_ok)) {
                bad.push(format!("{rel}:{n} 依存の名前で分けている: {}", line.trim()));
            }
            if arm_watched.contains(&rel.as_str()) && dep_name_arm(line) {
                bad.push(format!(
                    "{rel}:{n} 依存の名前の腕で分けている: {}",
                    line.trim()
                ));
            }
            if builds_privilege(line) && !(in_setup && within(n, &privilege_ok)) {
                bad.push(format!(
                    "{rel}:{n} 権限の値を依存表の外で組んでいる: {}",
                    line.trim()
                ));
            }
        }
    }
    assert!(
        scanned > 100,
        "製品コードを走査できていない（{scanned} ファイル）"
    );
    assert!(
        bad.is_empty(),
        "#2433: 依存ごとの「UAC が出うるか」は依存表（{SETUP_DEPS} の deps / container_dep の \
         InstallPrivilege）の 1 か所で宣言し、同意を取るかは InstallPrivilege::needs_consent と \
         DepInstallOptions::consent だけで決める。名前で分けると宣言が 2 か所に割れる:\n{}",
        bad.join("\n")
    );
}

/// 2: 同意を直書きしてよいのは `tako setup` の依存の段（y / `--yes` を受けた後）だけ
#[test]
fn issue2433_同意を直書きしない() {
    let files = production_range::product_sources();
    let test_files = production_range::test_only_files(&files);
    let setup_lines = product_lines(SETUP_DEPS);
    let ok = fn_span(&setup_lines, "offer_and_install");
    let mut bad = Vec::new();
    let mut allowed = 0usize;
    for rel in files.iter().filter(|f| !test_files.contains(*f)) {
        for (i, line) in product_lines(rel).iter().enumerate() {
            if !hardcodes_consent(line) {
                continue;
            }
            if rel == SETUP_DEPS && within(i + 1, &[ok]) {
                allowed += 1;
            } else {
                bad.push(format!("{rel}:{} {}", i + 1, line.trim()));
            }
        }
    }
    assert_eq!(
        allowed, 1,
        "offer_and_install の同意（y / --yes の後）が見つからない = 番犬の前提が崩れた"
    );
    assert!(
        bad.is_empty(),
        "#2433: UAC の出る導入への同意を直書きしている（利用者の同意なしで UAC が出る）。\
         同意は CLI の確認 / --yes・MCP の consent・設定画面のボタンの押下から渡す:\n{}",
        bad.join("\n")
    );
}

/// Windows の CI の段: 製品の口で winget を実走する段は関連パスの PR と定期・手動だけ、
/// 偽の winget で断りを見る段は毎回走る。どちらも同じスクリプトの別の腕
#[test]
fn issue2433_windowsのciが製品の口でwingetを回す() {
    let ci = read(".github/workflows/ci.yml");
    let lines: Vec<&str> = ci.lines().collect();
    let step = |needle: &str| -> (usize, Vec<&str>) {
        let head = lines
            .iter()
            .position(|l| l.trim_start().starts_with("- name:") && l.contains(needle))
            .unwrap_or_else(|| panic!("ci.yml に「{needle}」の段が無い（#2433）"));
        let body: Vec<&str> = lines[head + 1..]
            .iter()
            .take_while(|l| !l.trim_start().starts_with("- name:") && !l.trim().is_empty())
            .copied()
            .collect();
        (head + 1, body)
    };
    let script = "scripts/windows/test-deps-install-2433.ps1";
    let (real_at, real) = step("任意依存の導入代行の実走検査（#2433");
    assert!(
        real.iter().any(|l| l.trim() == "if: needs.changes.outputs.agent_install == 'true'"),
        "ci.yml:{real_at} winget の実走（外部から取る）は関連パスの PR と定期・手動実行だけで走らせる"
    );
    assert!(
        real.iter()
            .any(|l| l.contains(script) && l.contains("-Mode real")),
        "ci.yml:{real_at} 実走の段が {script} -Mode real を呼んでいない"
    );
    let (fake_at, fake) = step("任意依存の導入代行の断りの検査（#2433");
    assert!(
        !fake.iter().any(|l| l.trim_start().starts_with("if:")),
        "ci.yml:{fake_at} 偽の winget の段は外部に依らないので毎回走らせる"
    );
    assert!(
        fake.iter()
            .any(|l| l.contains(script) && l.contains("-Mode fake")),
        "ci.yml:{fake_at} 断りの段が {script} -Mode fake を呼んでいない"
    );
    // 関連パスを変えたら実走が走る（判定の正本は ci-change-scope.sh の AGENT_INSTALL）
    let scope = read("scripts/ci-change-scope.sh");
    let table = scope
        .split_once("AGENT_INSTALL=(")
        .and_then(|(_, rest)| rest.split_once(')'))
        .map(|(table, _)| table)
        .expect("ci-change-scope.sh に AGENT_INSTALL の表が無い");
    for path in [SETUP_DEPS, script] {
        assert!(
            table.lines().any(|l| l.trim().starts_with(&format!("'{path}|"))),
            "ci-change-scope.sh の AGENT_INSTALL に {path} が無い（変えても winget の実走が走らない。#2433）"
        );
    }
}
