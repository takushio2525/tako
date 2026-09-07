//! `tako master` の起動前 1 行が system prompt の超過を出すこと（Issue #1154 / FR-2.36.6）
//!
//! `tako master` は claude を起こしてしまうので CLI からは通しで測れない。
//! **その呼び出し先そのもの**（`context_budget::startup_line`。呼び出し点は
//! `crates/tako-cli/src/main.rs` の master 起動処理）を、隔離した `HOME` と
//! `TAKO_DATA_DIR` の上で叩いて、超過時に何が超えているかまで出ることを確かめる。
//!
//! **このファイルにテストは 1 本だけ置く**（`HOME` / `TAKO_DATA_DIR` はプロセス全体の
//! 状態なので、同じバイナリに他のテストを混ぜると並列実行で干渉する）。

use std::path::Path;

/// 予算をわざと超えさせる追記（`prompt_blocks.append` は `~/` 始まりのときだけ
/// ファイルとして読まれるので、`HOME` ごと隔離して置く）
fn write_fixture(home: &Path, data: &Path) {
    std::fs::create_dir_all(home).expect("home");
    std::fs::create_dir_all(data.join("orchestrator/profiles")).expect("profiles");

    // 追記なし = tako が配る既定の姿（予算内であることまで確かめる）
    std::fs::write(
        data.join("orchestrator/profiles/lean.yaml"),
        "model: claude-fable-5-1\neffort: high\n",
    )
    .expect("lean");

    // 個人環境固有のルールが育った状態
    let rules: String = std::iter::repeat_n("- 個人環境固有の規則の行です。\n", 600).collect();
    std::fs::write(home.join("local-rules.md"), &rules).expect("rules");
    std::fs::write(
        data.join("orchestrator/profiles/fat.yaml"),
        "model: claude-fable-5-1\neffort: high\nprompt_blocks:\n  append: ~/local-rules.md\n",
    )
    .expect("fat");
}

#[test]
fn master起動前の1行がsystem_promptの超過を名指しする() {
    let base = std::env::temp_dir().join(format!("tako-1154-startup-{}", std::process::id()));
    let home = base.join("home");
    let data = base.join("data");
    write_fixture(&home, &data);
    // 隔離: グローバル指示ファイルも作業ログも無い `HOME` に閉じるので、
    // 出る超過は system prompt のものだけになる（本番の設定には一切触らない）
    unsafe {
        std::env::set_var("HOME", &home);
        std::env::set_var("TAKO_DATA_DIR", &data);
    }
    tako_core::i18n::set_lang(tako_core::i18n::Lang::Ja);

    // 既定の姿は黙って素通りする（#322: 予算内なら 1 行も出さない）
    assert_eq!(
        tako_control::context_budget::startup_line(&home, Some("lean")),
        None,
        "既定 blocks だけなら予算内なので何も出さない"
    );

    // 超過したら 1 行で「何がどれだけ超えているか」を出す。
    // `fix` は作業ログしか直せないので、ここで `fix` を勧めてはいけない（嘘の案内になる）
    let line = tako_control::context_budget::startup_line(&home, Some("fat"))
        .expect("超過しているので 1 行出る");
    assert!(
        line.contains("master system prompt（fat）"),
        "何が超えているか名指ししていない: {line}"
    );
    assert!(line.contains("bytes"), "超過の軸が出ていない: {line}");
    assert!(
        !line.contains("context-budget fix"),
        "自動で直せないのに fix を勧めている: {line}"
    );
    assert!(
        line.contains("tako context-budget"),
        "内訳の引き方が出ていない: {line}"
    );
    assert_eq!(line.lines().count(), 1, "1 行に収める: {line}");

    // 内訳はユーザー側の追記をファイル名で名指しする（分離先を案内できるように）
    let report = tako_control::context_budget::report(&home, Some("fat")).expect("report");
    let proposal = report["proposals"]
        .as_array()
        .and_then(|v| v.first())
        .expect("提案");
    let biggest = proposal["pieces"]
        .as_array()
        .and_then(|v| v.first())
        .expect("内訳");
    assert_eq!(
        biggest["name"], "append (local-rules.md)",
        "最も重い断片をファイル名で名指しする: {proposal}"
    );

    let _ = std::fs::remove_dir_all(&base);
}
