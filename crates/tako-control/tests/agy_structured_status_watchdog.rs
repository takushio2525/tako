//! 番犬: agy の一次シグナルを画面推定へ戻さない（#1033）
//!
//! ## なぜ止めるのか
//!
//! 北極星実測（#975 / 2026-08-29）で agy だけが `status_source = "screen"` のままで、
//! 完了検知が **claude の 2.6 倍・+24 秒**だった。差の正体は `wait.rs` の
//! `need_streak`（画面推定 = 8 回・一次シグナル = 3 回）× ポーリング 5 秒という
//! **実装の 2 定数だけ**で、実測が理論値とぴたり合っていた。
//!
//! #1033 は agy の実況 JSONL
//! （`~/.gemini/antigravity-cli/brain/<id>/.system_generated/logs/transcript.jsonl`）を
//! 読んで `status_source = "agy-session"` を返すようにした。単体テストは
//! 「いまの判定結果」を固定するが、**同じ形の再発**（配線を外す・終端の判定を
//! 「content があれば完了」へ緩める・未達の断定を無条件へ戻す）はソースの形でしか止まらない。
//!
//! ## 4 本立て
//!
//! 1. [`agyの実況を状態ソースとして配線している`] — `finish_worker_status` の
//!    ソース解決に agy の腕があり、`agy-session` を返すこと（外すと画面推定へ逆戻り）
//! 2. [`終端の判定はtool_callsの不在まで見ている`] — 完了マーカーの判定が
//!    `tool_calls` を見ていること。実物に「喋りながらツールを呼ぶ」行が 58 件あり、
//!    `content` の有無だけで決めると**偽 idle**（= #1034 と同じ実害）になる
//! 3. [`終端はstep_index最大で採っている`] — 最終行ではなく `step_index` で採ること
//!    （実物の 28 会話中 5 件で `step_index` はファイル順と一致しない）
//! 4. [`未達の断定は一次シグナルを読めたときだけ`] — agy も #1015 と同じゲートを
//!    通ること（マトリクスを Structured にしただけで断定側へ倒すと、読めなかった
//!    worker へ自動再送が飛んで二重指示になる）

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

fn production(rel: &str) -> String {
    let src = std::fs::read_to_string(crate_file(rel)).unwrap_or_else(|e| panic!("{rel}: {e}"));
    without_comments(production_source(&src))
}

#[test]
fn agyの実況を状態ソースとして配線している() {
    let src = production("src/dispatch.rs");
    assert!(
        src.contains("agy_session::resolve_conversation_id_for_backend"),
        "ペイン → agy の会話 ID の解決が外れている（画面推定へ逆戻りする）"
    );
    assert!(
        src.contains("\"agy-session\""),
        "status_source に agy-session が無い（wait.rs の need_streak が 8 のままになる）"
    );
    assert!(
        src.contains("agy_session::read_turn_state"),
        "実況 JSONL からのターン状態の読み取りが外れている"
    );
    assert!(
        src.contains("agy_session::last_agent_texts"),
        "report の transcript 層から agy が外れている（messages が 0 件へ戻る）"
    );
    // `wait.rs` 側は「screen 以外なら 3」の形なので、agy-session という値が
    // 出ていることと合わせて need_streak が 3 になることを固定する
    let wait = production("src/orchestrator/wait.rs");
    let body = fn_body(&wait, "pub fn wait_for_worker(");
    assert!(
        body.contains("if source == \"screen\" { 8 } else { 3 }"),
        "need_streak の形が変わった（agy-session が 8 側へ落ちていないか確認する）"
    );
}

#[test]
fn 終端の判定はtool_callsの不在まで見ている() {
    let src = production("src/agy_session.rs");
    let body = fn_body(&src, "    fn is_turn_end(&self) -> bool {");
    assert!(
        body.contains("has_tool_calls"),
        "完了マーカーが tool_calls を見ていない。実物に本文とツール呼び出しを同居させる \
         行が 58 件あるので、content だけで決めると作業中を完了と誤読する（偽 idle）"
    );
    assert!(
        body.contains("PLANNER_RESPONSE"),
        "完了マーカーの型が PLANNER_RESPONSE に限定されていない \
         （ツール結果の GENERIC も本文を持つので拾ってしまう）"
    );
}

#[test]
fn 終端はstep_index最大で採っている() {
    let src = production("src/agy_session.rs");
    let body = fn_body(
        &src,
        "pub fn parse_turn_state(lines: &[&str]) -> TurnState {",
    );
    assert!(
        body.contains("max_by_key") && body.contains("step_index"),
        "終端を step_index の最大で採っていない。実物の 28 会話中 5 件で step_index は \
         ファイルの行順と一致しない（非同期ツールの結果が後から追記される）ので、\
         最終行で判定すると完了を取り落とす"
    );
}

#[test]
fn 未達の断定は一次シグナルを読めたときだけ() {
    let src = production("src/dispatch.rs");
    let body = fn_body(
        &src,
        "    let prompt_delivery = registry_worker.as_ref().map(",
    );
    assert!(
        body.contains("agy_transcript_read"),
        "agy の未達判定が「実況を読めたか」を見ていない。マトリクスを Structured に \
         しただけだと、読めなかった worker を未達と断定して自動再送が飛ぶ（二重指示事故）"
    );
    assert!(
        body.contains("agy_turn_observed"),
        "agy の USER_INPUT を送達の証拠に使っていない（画面確認の失敗で未達へ倒れる）"
    );
}

/// 5 本目: **構造化ソースを足しただけでは効かない**（#571 → #984 → #1033 で 3 回踏んだ形）。
///
/// エージェント CLI の TUI 自身がペインシェルの子なので `has_children` は生きている限り
/// 必ず true になり、補正層が idle を busy へ上書きする。実況ログ由来のソースを
/// 「一次情報」の側に入れ忘れると、**`agy-session` が出ているのに永久 busy** になる
/// （実機の隔離 GUI で実際に踏んだ: 画面に `RESULT: 137` が出ていて transcript も
/// 終端なのに `status = busy`）。
#[test]
fn 実況ログ由来のソースを一次情報として扱っている() {
    let src = production("src/dispatch.rs");
    let body = fn_body(&src, "fn is_live_log_source(status_source: &str) -> bool {");
    assert!(
        body.contains("\"agy-session\"") && body.contains("\"codex-session\""),
        "実況ログ由来ソースの判定に agy-session / codex-session が揃っていない"
    );
    // 補正層と降格層の両方がこの 1 実装を通ること（片方だけ直すと永久 busy へ戻る）
    let uses = src.matches("is_live_log_source(").count();
    assert!(
        uses >= 3,
        "is_live_log_source の呼び出しが {uses} 箇所しかない。\
         定義 + 「unknown なら screen へ降格」 + 「idle を has_children で覆さない」の \
         3 箇所すべてが通っている必要がある（片方を直値へ戻すと永久 busy になる）"
    );
    assert!(
        !src.contains(
            "status_source == \"agents-auto\"\n        || status_source == \"codex-session\""
        ),
        "権威の判定が直値の列挙へ戻っている（系統を足したときに足し忘れる）"
    );
}
