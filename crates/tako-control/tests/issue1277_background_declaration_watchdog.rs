//! 番犬: 背景作業の申告を「ターンが終わった」の根拠にできるのは claude だけ（#1277）
//!
//! ## なぜソースの形で止めるのか
//!
//! #1277 の実測はこうだった（隔離 tmux 上の実 CLI・2026-09-11）:
//!
//! | 系統 | 画面の申告 | 申告が出るのは |
//! |---|---|---|
//! | claude 2.1.258 | `· 1 shell, 1 monitor still running` | **ターンが終わったときだけ** |
//! | codex-cli 0.154.0 | `1 background terminal running · /ps to view · /stop to close` | 背景作業が在るあいだ常時（生成中も） |
//! | Antigravity CLI 1.2.0 | フッターの `· 1 task(s) · /tasks` | 同（生成中も） |
//!
//! codex / agy は一次シグナル（rollout の `task_complete` / 実況 JSONL の終端）が
//! ターン終了で idle へ落ちるので、**画面で覆う必要が無い**。にもかかわらず
//! 覆す根拠に使うと、フッターが幅で切られて `screen_looks_busy` を引けない場面で
//! 偽 idle が出る（#1015 で実際に起きた事故と同じ形 = 自動再送で二重指示）。
//!
//! **この誤りは挙動テストだけでは止まらない**: codex の入力欄は
//! 常にプレースホルダ（`Ask Codex to do anything`）を持つので
//! `input_content_is_empty` が false を返し、覆す腕へ入る前に落ちる。
//! つまり今は「別の理由で」守られているだけで、
//! プレースホルダを空と読むようにした瞬間（別 Issue で十分ありうる）に
//! 静かに偽 idle が出はじめる。だから宣言の形をここで固定する。
//!
//! ## 3 本立て
//!
//! 1. [`画面の申告でターン終了を言えるのはclaudeだけ`] — `declaration_implies_turn_end` の
//!    `true` 側に claude 以外を置かせない（置くなら実測の根拠つきで #1277 を更新すること）
//! 2. [`覆す腕が申告の意味を通っている`] — ゲートの呼び出しが消えていないこと
//! 3. [`agyの実況行を内訳の材料にしていない`] — 実行中のコマンド全文を拾わない（#927）

use std::path::{Path, PathBuf};

fn crate_file(rel: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join(rel)
}

/// テストモジュールより手前だけを見る（番犬自身の文字列や fixture に当たらない）
fn production_source(src: &str) -> &str {
    match src.find("\n#[cfg(test)]\nmod tests") {
        Some(i) => &src[..i],
        None => src,
    }
}

/// コメント行を落とす（説明文に書いた実採取の引用を拾わない）
fn without_comments(body: &str) -> String {
    body.lines()
        .filter(|l| {
            let t = l.trim_start();
            !(t.starts_with("//") || t.starts_with("//!"))
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// 指定した関数の本文（シグネチャから次の行頭 `}` まで）を切り出す
fn fn_body(src: &str, signature: &str) -> String {
    let start = src
        .find(signature)
        .unwrap_or_else(|| panic!("{signature} が見つからない（改名したら番犬も直す）"));
    let rest = &src[start..];
    let end = rest.find("\n}\n").unwrap_or(rest.len());
    rest[..end].to_string()
}

fn wait_production() -> String {
    let src = std::fs::read_to_string(crate_file("src/orchestrator/wait.rs")).expect("wait.rs");
    production_source(&src).to_string()
}

#[test]
fn 画面の申告でターン終了を言えるのはclaudeだけ() {
    let src = wait_production();
    let body = without_comments(&fn_body(&src, "fn declaration_implies_turn_end("));
    assert!(
        body.contains("Agent::Claude => true"),
        "claude の腕が `true` でない（#1273 の判定が死ぬ）:\n{body}"
    );
    assert_eq!(
        body.matches("=> true").count(),
        1,
        "`true` の腕が claude 以外にもある。codex / agy の申告は\
         **背景作業が在ることしか言っておらず生成中も同じ形で出る**（#1277 実採取）。\
         覆す根拠に使うと幅で切られた画面で偽 idle が出る（#1015 と同じ事故）。\
         上げるなら実測の根拠つきで #1277 の evidence を更新すること:\n{body}"
    );
    for agent in ["Agent::Codex", "Agent::Agy", "Agent::Local"] {
        let arm = body
            .lines()
            .find(|l| l.contains(agent))
            .unwrap_or_else(|| panic!("{agent} の腕が無い（系統を増やしたら宣言も足す）"));
        assert!(
            arm.contains("=> false"),
            "{agent} の腕が `false` でない（#1277）:\n{arm}"
        );
    }
}

#[test]
fn 覆す腕が申告の意味を通っている() {
    let src = wait_production();
    let body = without_comments(&fn_body(
        &src,
        "pub fn input_waiting_with_background_work_in(",
    ));
    assert!(
        body.contains("declaration_implies_turn_end"),
        "一次シグナルの busy を覆す腕が「その系統の申告がターン終了を含意するか」を\
         見ていない（#1277）。能力マトリクスの `supports` は\
         **3 系統とも Supported** になったので、これを外すと codex / agy でも\
         状態しか言わない申告で覆すようになる:\n{body}"
    );
    assert!(
        body.contains("WORKER_IDLE_WITH_BACKGROUND"),
        "能力マトリクスの宣言（#982）を通っていない:\n{body}"
    );
}

#[test]
fn agyの実況行を内訳の材料にしていない() {
    let src = wait_production();
    let body = without_comments(&fn_body(&src, "fn agy_background_work_summary("));
    assert!(
        !body.contains('●'),
        "agy の実況行（`● [07:15:49] sleep 600 running`）を読んでいる。\
         実行中のコマンド全文が載るので `worker_status` の応答や persist.log へ\
         作業内容がそのまま出る（#927）。件数はフッターの `· N task(s) · /tasks` で足りる:\n{body}"
    );
    assert!(
        body.contains("/tasks"),
        "フッターの申告であることを構造で確かめていない（本文の `2 tasks` に誤爆する。#1015）:\n{body}"
    );
}
