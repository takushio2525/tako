//! 番犬: `~/.ssh/config` のパースが「どの Host にも属さない位置」を持つ（#1400）
//!
//! ## なぜ止めるのか
//!
//! パース結果は表示専用ではない。`dispatch::remote_ssh_argv` が
//! `SshHost::ssh_command()` から `-p <port>` と `user@host` を取り出して
//! `remote_fs::ssh_pane_argv` へ渡すので、**設定を取り違えると繋ぎ先が変わる**。
//! 修正前は `Match` 行を `_ => {}` で素通りさせていたため
//!
//! ```text
//! Host prod
//!   HostName 10.x.x.x
//! Match host bastion
//!   User root
//!   Port 2222
//! ```
//!
//! で `prod` の argv が `ssh -p 2222 root@prod` へ化けていた（本物の `ssh prod` は
//! `Match host bastion` に一致しないのでこの設定を使わない = **tako だけが別の宛先・
//! 別ユーザーへ繋ぐ**）。同時に `Host web1 web2` は先頭 1 つで `break` していて
//! コメントと食い違い、`Include` は解決していなかった。
//!
//! 挙動そのものは `tako_core::ssh_config` の単体 / 結合テスト
//! （`issue1400_tests::*`）が見る。ここが見るのは**形が戻っていないこと**:
//! 分岐を 1 つ消すだけで挙動が静かに元へ戻るので、消えたことを file:line で名指しする。
//!
//! ## 何を違反とするか（**誤検知しない形**）
//!
//! 1. [`ssh_configのディレクティブのアームが揃っている`] — `host` / `match` /
//!    `include` / `hostname` / `user` / `port` のアームが在り、`match` が塊を閉じ、
//!    `include` が取り込みを呼ぶこと
//! 2. [`hostのパターン走査に打ち切りが無い`] — `"host"` のアームの中に `break` が
//!    無いこと（先頭 1 つだけ拾う形そのもの）
//! 3. [`section型は2値のまま`] — 指紋スナップショット。単一カーソル
//!    （`Option<SshHost>`）へ戻したら落ちる
//! 4. [`remote_ssh_argvはssh_commandから組み立てている`] — tako-core 側の結合テストが
//!    dispatch の組み立てを**写して**検査しているので、写しが古びたら落ちる

use std::path::{Path, PathBuf};

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("リポジトリルート")
        .to_path_buf()
}

const SSH_CONFIG_REL: &str = "crates/tako-core/src/ssh_config.rs";
const DISPATCH_REL: &str = "crates/tako-control/src/dispatch.rs";

fn read(rel: &str) -> String {
    std::fs::read_to_string(repo_root().join(rel)).unwrap_or_else(|e| panic!("{rel} を読む: {e}"))
}

/// テストモジュールより手前だけを見る（fixture や期待値の文字列に当たらない）。
/// **最初の `#[cfg(test)]\nmod ` の直前で切る**
fn production_source(src: &str) -> &str {
    match src.find("\n#[cfg(test)]\nmod ") {
        Some(i) => &src[..i],
        None => src,
    }
}

fn is_comment(line: &str) -> bool {
    line.trim_start().starts_with("//")
}

/// `needle` を含む最初の行の番号（0 起点）
fn line_of(src: &str, needle: &str) -> Option<usize> {
    src.lines()
        .position(|line| !is_comment(line) && line.contains(needle))
}

/// `start` 行のあと、`end_needle` を含む行までの範囲（`start` を含み `end` は含まない）。
/// **波括弧を数えない**（文字列リテラルの `{` で対応が崩れる = #893 の教訓）
fn line_window<'a>(src: &'a str, start: usize, end_needle: &str) -> Vec<(usize, &'a str)> {
    let lines: Vec<&str> = src.lines().collect();
    let end = lines
        .iter()
        .enumerate()
        .skip(start + 1)
        .find(|(_, line)| !is_comment(line) && line.contains(end_needle))
        .map(|(i, _)| i)
        .unwrap_or(lines.len());
    (start..end).map(|i| (i, lines[i])).collect()
}

// ------------------------------------- 1. ディレクティブのアームが揃っている

/// 揃っていない要件を `rel:line: 説明` で並べる
fn offending_arms(rel: &str, src: &str) -> Vec<String> {
    let prod = production_source(src);
    let Some(dispatch_line) = line_of(prod, "match key.to_ascii_lowercase().as_str()") else {
        return vec![format!(
            "{rel}:1: ディレクティブを振り分ける match が見つからない"
        )];
    };
    let anchor = format!("{rel}:{}", dispatch_line + 1);
    let mut missing = Vec::new();
    // 6 つのアームすべて（1 つでも消えるとその設定が静かに落ちる）
    for key in ["host", "match", "include", "hostname", "user", "port"] {
        if !prod.contains(&format!("\"{key}\"")) {
            missing.push(format!("{anchor}: `\"{key}\"` のアームが無い"));
        }
    }
    // `Match` は塊を閉じる（`_ => {}` で素通りさせたのが #1400 の (1)）
    if !prod.contains("\"match\" => self.close(") {
        missing.push(format!(
            "{anchor}: `\"match\"` が塊を閉じていない（以降の User / Port が直前の Host へ付く）"
        ));
    }
    // `Include` は取り込みを呼ぶ
    if !prod.contains("self.include(") {
        missing.push(format!(
            "{anchor}: `\"include\"` が取り込みを呼んでいない（Include 配下の Host が一覧から消える）"
        ));
    }
    missing
}

#[test]
fn ssh_configのディレクティブのアームが揃っている() {
    let src = read(SSH_CONFIG_REL);
    let found = offending_arms(SSH_CONFIG_REL, &src);
    assert!(
        found.is_empty(),
        "`~/.ssh/config` のパースの分岐が欠けている（#1400。分岐 1 つで繋ぎ先が変わる）:\n{}",
        found.join("\n")
    );
}

#[test]
fn i1400_注入_消えたアームを名指しで落とせる() {
    // 修正前そのもの（`Match` を `_ => {}` へ落とす = アームが無い）
    let injected = "fn f() {\n    \
        match key.to_ascii_lowercase().as_str() {\n        \
        \"host\" => {}\n        \
        \"hostname\" => {}\n        \
        \"user\" => {}\n        \
        \"port\" => {}\n        \
        _ => {}\n    }\n}\n";
    let got = offending_arms(SSH_CONFIG_REL, injected);
    // `match` / `include` のアーム + 塊を閉じない + 取り込みを呼ばない = 4 件
    assert_eq!(got.len(), 4, "注入を拾えていない: {got:?}");
    assert!(
        got.iter().all(|m| m.contains(":2:")),
        "行番号が合っていない: {got:?}"
    );
    assert!(
        got.iter().any(|m| m.contains("`\"match\"` のアームが無い")),
        "{got:?}"
    );
    assert!(
        got.iter()
            .any(|m| m.contains("`\"include\"` が取り込みを呼んでいない")),
        "{got:?}"
    );

    // `Match` のアームだけ「在るが閉じない」形（名前を変えても落ちる）
    let half = "fn f() {\n    \
        match key.to_ascii_lowercase().as_str() {\n        \
        \"host\" => {}\n        \"match\" => {}\n        \"include\" => self.include(&s),\n        \
        \"hostname\" => {}\n        \"user\" => {}\n        \"port\" => {}\n    }\n}\n";
    let got = offending_arms(SSH_CONFIG_REL, half);
    assert_eq!(got.len(), 1, "{got:?}");
    assert!(got[0].contains("塊を閉じていない"), "{got:?}");

    // 振り分けの match そのものが消えたら 1 行で名指しする
    let gone = "fn f() {\n    let _ = key;\n}\n";
    let got = offending_arms(SSH_CONFIG_REL, gone);
    assert_eq!(got.len(), 1, "{got:?}");
    assert!(got[0].contains("見つからない"), "{got:?}");
}

// --------------------------------- 2. host のパターン走査に打ち切りが無い

/// `"host"` のアームの中の `break` を file:line で並べる
fn offending_pattern_break(rel: &str, src: &str) -> Vec<String> {
    let prod = production_source(src);
    let Some(start) = line_of(prod, "\"host\" =>") else {
        return Vec::new();
    };
    // 次のアーム（`"match" =>`）までを見る
    line_window(prod, start, "\"match\" =>")
        .into_iter()
        .filter(|(_, line)| !is_comment(line))
        .filter(|(_, line)| line.contains("break"))
        .map(|(i, line)| format!("{rel}:{}: {}", i + 1, line.trim()))
        .collect()
}

#[test]
fn hostのパターン走査に打ち切りが無い() {
    let src = read(SSH_CONFIG_REL);
    let found = offending_pattern_break(SSH_CONFIG_REL, &src);
    assert!(
        found.is_empty(),
        "`Host` の複数パターンを先頭 1 つで打ち切る形が復活している（`Host web1 web2` の\n\
         `web2` が一覧から消える = #1400 の (2)。各パターンを独立エントリにする）:\n{}",
        found.join("\n")
    );
}

#[test]
fn i1400_注入_パターンの打ち切りを名指しで落とせる() {
    // 修正前そのもの
    let injected = "fn f() {\n    \
        match k {\n        \
        \"host\" => {\n            \
        for pattern in value.split_whitespace() {\n                \
        if !pattern.contains('*') {\n                    \
        current = Some(h);\n                    \
        break;\n                \
        }\n            \
        }\n        \
        }\n        \
        \"match\" => self.close(&mut section),\n    }\n}\n";
    let got = offending_pattern_break(SSH_CONFIG_REL, injected);
    assert_eq!(got.len(), 1, "注入を拾えていない: {got:?}");
    assert!(got[0].contains(":7:"), "行番号が合っていない: {got:?}");

    // 別のアームの `break` には当たらない
    let elsewhere = "fn f() {\n    \
        match k {\n        \
        \"host\" => {}\n        \
        \"match\" => self.close(&mut section),\n        \
        \"port\" => {\n            break;\n        }\n    }\n}\n";
    assert!(offending_pattern_break(SSH_CONFIG_REL, elsewhere).is_empty());
    // コメントにも当たらない
    let commented = "fn f() {\n    match k {\n        \"host\" => {\n            \
        // break で打ち切っていた\n        }\n        \"match\" => self.close(&mut section),\n    }\n}\n";
    assert!(offending_pattern_break(SSH_CONFIG_REL, commented).is_empty());
}

// --------------------------------------------- 3. Section 型は 2 値のまま

#[test]
fn section型は2値のまま() {
    let src = read(SSH_CONFIG_REL);
    let prod = production_source(&src);
    assert!(
        prod.contains("enum Section {"),
        "{SSH_CONFIG_REL}: 読み進めるあいだの状態が enum Section で表されていない\n\
         （`Option<SshHost>` の単一カーソルへ戻すと、`Match` / `Include` のあとに\n\
         「どの Host にも属さない位置」を表せなくなる = #1400 の再発）"
    );
    // 指紋: 2 値（増減させたら「属さない位置」の意味を考え直すことになる）
    let start = line_of(prod, "enum Section {").expect("enum の先頭");
    let variants: Vec<&str> = line_window(prod, start, "struct Parser<")
        .into_iter()
        .filter(|(_, line)| !is_comment(line))
        .map(|(_, line)| line.trim())
        .filter(|line| line.ends_with(',') || line.ends_with("(Vec<SshHost>),"))
        .collect();
    assert_eq!(
        variants,
        vec!["Unattached,", "Hosts(Vec<SshHost>),"],
        "{SSH_CONFIG_REL}:{}: Section のバリアントが変わった（#1400）",
        start + 1
    );
}

// -------------------- 4. remote_ssh_argv は ssh_command から組み立てている

/// tako-core の結合テストが写している 3 要素のうち欠けたものを並べる
fn offending_argv_shape(rel: &str, src: &str) -> Vec<String> {
    let prod = production_source(src);
    let Some(start) = line_of(prod, "fn remote_ssh_argv(") else {
        return vec![format!("{rel}:1: `remote_ssh_argv` が見つからない")];
    };
    let window: String = line_window(prod, start, "\n/// リモートファイルを")
        .into_iter()
        .filter(|(_, line)| !is_comment(line))
        .map(|(_, line)| format!("{line}\n"))
        .collect();
    let anchor = format!("{rel}:{}", start + 1);
    let mut missing = Vec::new();
    for (needle, why) in [
        (
            "h.ssh_command()",
            "`ssh_command()` から extra を取り出していない",
        ),
        ("cmd[1..", "先頭 `ssh` と末尾の宛先を落とす切り出しが無い"),
        ("h.user", "`User` を宛先へ反映していない"),
    ] {
        if !window.contains(needle) {
            missing.push(format!("{anchor}: {why}（`{needle}` が無い）"));
        }
    }
    missing
}

#[test]
fn remote_ssh_argvはssh_commandから組み立てている() {
    let src = read(DISPATCH_REL);
    let found = offending_argv_shape(DISPATCH_REL, &src);
    assert!(
        found.is_empty(),
        "SSH ペインの argv の組み立てが変わった（#1400）。\n\
         `tako_core::ssh_config::issue1400_tests::i1400_結合_*` がこの組み立てを**写して**\n\
         検査しているので、写しを合わせ直すこと:\n{}",
        found.join("\n")
    );
}

#[test]
fn i1400_注入_argvの組み立ての変化を名指しで落とせる() {
    let injected = "fn remote_ssh_argv(ssh_host: &str) -> Vec<String> {\n    \
        tako_core::remote_fs::ssh_pane_argv(ssh_host, &[])\n}\n\
        \n/// リモートファイルを**取得したあと**の共通経路\nfn next() {}\n";
    let got = offending_argv_shape(DISPATCH_REL, injected);
    assert_eq!(got.len(), 3, "注入を拾えていない: {got:?}");
    assert!(
        got.iter().all(|m| m.contains(":1:")),
        "行番号が合っていない: {got:?}"
    );

    // 関数そのものが消えたら 1 行で名指しする
    let gone = "fn other() {}\n";
    let got = offending_argv_shape(DISPATCH_REL, gone);
    assert_eq!(got.len(), 1, "{got:?}");
    assert!(got[0].contains("見つからない"), "{got:?}");
}
