//! 番犬: 背景作業が残る入力待ちを busy に戻さない（#1273 / #289 の再発）
//!
//! ## なぜ止めるのか
//!
//! #289（2026-07-16）は「入力待ちなのに `has_running_children` で busy に上書きされる」
//! と**推定**して `status == "idle"` の腕を直したが、その推定は実測されていなかった。
//! 2026-09-09 に実物を採ると生 status は **busy** で（claude 2.1.258 の
//! `agents --json` は `isLoading || delegatedActive` で決め、背景シェル / Monitor が
//! 生きているあいだ busy を返し続ける）、#289 の腕には一度も入っていなかった。
//! 単体テストは「いまの判定結果」を固定するが、**同じ形の再発**
//! （上書き経路を消す・空の `❯` だけで倒す・待機中の変種を完了と読む）は
//! ソースの形と境界の実測でしか止まらない。
//!
//! ## 6 本立て
//!
//! 1. [`一次シグナルのbusyを画面で覆す経路がある`] — `apply_worker_status_corrections` に
//!    `status == "busy"` から `input_waiting_with_background_work` を呼ぶ腕があること
//!    （消すと watch は永久に `WORKER_IDLE` を出せない）
//! 2. [`空の入力欄だけでは倒さない`] — 判定が `screen_looks_busy` を必ず通ること。
//!    claude は生成中も入力欄を描くので、`❯` の実在だけでは入力待ちと言えない
//! 3. [`判定器はwatchの再検査と同じものを使う`] — 予測器が agent 指定版
//!    （`screen_looks_busy_for`）へすり替わらないこと。watch の `"idle"` の腕は
//!    自動判別版で再検査するので、食い違うと `idle_streak` が永久に積まれない
//! 4. [`背景作業の完了待ちは完了と読まない`] — `Waiting for … to finish` の変種で
//!    倒さないこと（claude 自身が「終わった」と「待っている」を描き分けている）
//! 5. [`childrenの生存だけでbusyへ倒さない`] — #289 が直した形の据え置き。
//!    画面が入力欄を映していれば、子プロセスが生きていても busy にしない
//! 6. [`内訳は数で始まる項目だけを認める`] — 「どこかに数がある」へ緩めないこと。
//!    会話本文の `· step 3 still running` を背景作業の申告と読むと、
//!    生成中の worker を完了と誤報する経路ができる

use std::path::{Path, PathBuf};

use tako_control::orchestrator::wait;
use tako_core::agent_support::{keys, supports, Agent};

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

/// コメント行を落とす（説明文に書いた引用を拾わない）
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
    without_comments(&rest[..end])
}

/// マッチした行を「ファイル:行番号」つきで名指しする
fn locate(path: &str, src: &str, needle: &str) -> String {
    src.lines()
        .enumerate()
        .filter(|(_, l)| l.contains(needle))
        .map(|(i, l)| format!("{path}:{} {}", i + 1, l.trim()))
        .collect::<Vec<_>>()
        .join("\n")
}

/// 実採取の形（#927 に沿ってラベルはプレースホルダ）。ターンは終わり、
/// 背景シェルと Monitor だけが生きている
const IDLE_WITH_BACKGROUND: &str = "\
⏺ リリースビルド中です。完了を待ちます。

✻ Worked for 1m 11s · done 2:54 PM · 1 shell, 1 monitor still running

─────────────────────────────────────────────
❯
─────────────────────────────────────────────
  [model placeholder]  worker: placeholder task
  ctx  40% ████░░░░░░
  ⏵⏵ auto mode on · 1 shell, 1 monitor · ← for agents";

#[test]
fn 一次シグナルのbusyを画面で覆す経路がある() {
    let path = "crates/tako-control/src/dispatch.rs";
    let src = std::fs::read_to_string(crate_file("src/dispatch.rs")).expect("dispatch.rs を読める");
    let body = fn_body(
        production_source(&src),
        "fn apply_worker_status_corrections(",
    );
    assert!(
        body.contains("input_waiting_with_background_work"),
        "一次シグナルの busy を画面で覆す腕が消えている。\
         これが無いと背景シェルが残るあいだ watch は永久に WORKER_IDLE を出せない\n{}",
        locate(path, &src, "fn apply_worker_status_corrections(")
    );
    // 呼ぶ前に busy であることを確かめていること（idle を busy へ倒す腕と取り違えない）
    let arm = body
        .split("input_waiting_with_background_work")
        .next()
        .expect("split は必ず 1 要素以上");
    assert!(
        arm.rsplit("if status ==")
            .next()
            .is_some_and(|s| s.contains("\"busy\"")),
        "上書きの腕が `status == \"busy\"` を見ていない（別の状態から倒している）"
    );
}

#[test]
fn 空の入力欄だけでは倒さない() {
    let path = "crates/tako-control/src/orchestrator/wait.rs";
    let src =
        std::fs::read_to_string(crate_file("src/orchestrator/wait.rs")).expect("wait.rs を読める");
    let body = fn_body(
        production_source(&src),
        "pub fn input_waiting_with_background_work_in(",
    );
    assert!(
        body.contains("screen_looks_busy"),
        "判定が生成中の目印を見ていない。claude は生成中も入力欄を描くので、\
         `❯` の実在だけで倒すと本当に走っている worker を完了と誤報する\n{}",
        locate(path, &src, "pub fn input_waiting_with_background_work_in(")
    );
    assert!(
        body.contains("background_work_summary"),
        "背景作業の申告を見ていない。説明のつかない busy まで覆すことになる"
    );
    // #1297: 「空か」は**属性込み**の分類器が決める（文字列だけに戻したら
    // ゴースト提案が下書きに見えて覆せなくなる。詳細は issue1297 の番犬）
    assert!(
        body.contains("classify_input_draft"),
        "入力欄に人の下書きが無いことを見ていない（人間の下書きを踏み潰す）"
    );
    assert!(
        body.contains("collapsed"),
        "折りたたみ画面を除外していない（本文が欠けた画面を根拠にしてしまう）"
    );

    // 実挙動でも: 生成中の画面（スピナーあり + 背景作業の申告あり）では倒さない
    let generating = IDLE_WITH_BACKGROUND.replace(
        "✻ Worked for 1m 11s · done 2:54 PM · 1 shell, 1 monitor still running",
        "✻ Cooking… (12s · ↓ 1.2k tokens)\n\n✻ … · 1 shell, 1 monitor still running",
    );
    assert_eq!(
        wait::input_waiting_with_background_work(&generating, false, Some(Agent::Claude), None),
        wait::BackgroundIdle::No,
        "スピナーが出ている画面を入力待ちと読んでいる"
    );
}

#[test]
fn 判定器はwatchの再検査と同じものを使う() {
    let src =
        std::fs::read_to_string(crate_file("src/orchestrator/wait.rs")).expect("wait.rs を読める");
    let body = fn_body(
        production_source(&src),
        "pub fn input_waiting_with_background_work_in(",
    );
    assert!(
        !body.contains("screen_looks_busy_for") && !body.contains("screen_looks_busy_in"),
        "agent 指定版へすり替わっている。watch の \"idle\" の腕は自動判別版 \
         `screen_looks_busy(recent)` で再検査するので、判定器が食い違うと \
         dispatch が idle と言った画面を watch が busy と読み、idle_streak が永久に積まれない"
    );
    // watch 側が自動判別版で再検査していることも固定する（片側だけ替わっても壊れる）
    let watch = fn_body(production_source(&src), "pub fn wait_for_worker(");
    assert!(
        watch.contains("screen_looks_busy(recent)"),
        "watch の再検査が自動判別版でなくなっている"
    );
}

#[test]
fn 背景作業の完了待ちは完了と読まない() {
    let waiting = IDLE_WITH_BACKGROUND.replace(
        "✻ Worked for 1m 11s · done 2:54 PM · 1 shell, 1 monitor still running",
        "✻ Waiting for 2 background agents and 1 dynamic workflow to finish",
    );
    assert_eq!(
        wait::background_work_summary(&waiting),
        None,
        "`Waiting for … to finish`（= 背景作業の完了を待って止まっている）を \
         完了として読んでいる"
    );
    assert_eq!(
        wait::input_waiting_with_background_work(&waiting, false, Some(Agent::Claude), None),
        wait::BackgroundIdle::No
    );
    // 対照: 終わっている形はちゃんと読める（番犬が常に通るだけの形になっていない）
    assert_eq!(
        wait::background_work_summary(IDLE_WITH_BACKGROUND).as_deref(),
        Some("1 shell, 1 monitor")
    );
    assert!(supports(Agent::Claude, keys::WORKER_IDLE_WITH_BACKGROUND));
}

#[test]
fn childrenの生存だけでbusyへ倒さない() {
    // #289 が直した形の据え置き。画面推定の腕は「判断できないとき」だけ
    // has_children を見る（`screen_looks_idle` が真なら idle が勝つ）
    let src = std::fs::read_to_string(crate_file("src/dispatch.rs")).expect("dispatch.rs を読める");
    let body = fn_body(
        production_source(&src),
        "fn apply_worker_status_corrections(",
    );
    assert!(
        body.contains("has_children && !agents_authoritative"),
        "#289 の不変条件（一次シグナルの idle をプロセスツリーで覆さない）が消えている"
    );
    // `if status == "unknown"` は 2 か所（source の降格 + 画面推定）。後者を見る
    let unknown_arms: Vec<&str> = body.split("if status == \"unknown\"").collect();
    assert!(unknown_arms.len() >= 3, "画面推定の腕が見つからない");
    let unknown_arm = unknown_arms.last().expect("直前で長さを確かめている");
    let idle_at = unknown_arm
        .find("screen_looks_idle")
        .expect("画面推定の腕が入力欄を見ていない");
    let children_at = unknown_arm
        .find("has_children")
        .expect("画面推定の腕から has_children が消えている");
    assert!(
        idle_at < children_at,
        "画面推定の腕で has_children が入力欄より先に判定されている \
         （エージェント CLI 自身が常に「子」なので、先に見ると worker は永久に完了しない）"
    );
}

#[test]
fn 内訳は数で始まる項目だけを認める() {
    // claude の内訳生成は「`<数> <名詞>` をカンマで連ねる」形。**どこかに数がある**へ
    // 緩めると、会話本文の似た並びを申告と読んでしまう
    for prose in [
        "⏺ ビルド · step 3 still running",
        "⏺ ログ · retry 2 still running",
        "⏺ 状況 · the dev server is still running",
    ] {
        let screen = IDLE_WITH_BACKGROUND.replace(
            "✻ Worked for 1m 11s · done 2:54 PM · 1 shell, 1 monitor still running",
            prose,
        );
        assert_eq!(
            wait::background_work_summary(&screen),
            None,
            "本文の並びを背景作業の申告として読んでいる: {prose}"
        );
    }
    // 対照: claude が実際に出す形は読める（数を持たない固定語も含む）
    for (line, want) in [
        (
            "✻ Worked for 12s · done 2:54 PM · 1 shell still running",
            "1 shell",
        ),
        (
            "✻ Worked for 12s · done 2:54 PM · 3 background tasks still running",
            "3 background tasks",
        ),
        ("✻ Worked for 12s · dreaming still running", "dreaming"),
    ] {
        let screen = IDLE_WITH_BACKGROUND.replace(
            "✻ Worked for 1m 11s · done 2:54 PM · 1 shell, 1 monitor still running",
            line,
        );
        assert_eq!(
            wait::background_work_summary(&screen).as_deref(),
            Some(want),
            "claude の実形を読めていない: {line}"
        );
    }
}
