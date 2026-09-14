//! 蓋閉じ継続の安全弁を**外せなくする**番犬（Issue #1473）
//!
//! # なぜ要るか
//!
//! #1473 でバッテリー駆動でも蓋を閉じたまま走らせられるようにした。倒すのは
//! `pmset disablesleep`（macOS）と電源プランの lid action（Windows）で、どちらも
//! **OS のスリープをまるごと止める**。だから安全弁（エージェント稼働中のみ /
//! 残量が下限以下なら解除 / 本体が高温なら解除）が外れると、症状は
//! 「鞄の中で電池が空になる」「熱を持ったまま閉じ続ける」という**気づいたときには
//! 手遅れの壊れ方**になる。ビルドもテストも緑のまま起こりうるので、ソースの形で縛る。
//!
//! # 何を縛るか
//!
//! - **規則 A**: 判定 `lid_decision` の本体が、安全弁の材料（残量・下限・温度）を
//!   すべて見ている。どれかを消したら `file:line` で落ちる
//! - **規則 B**: 温度の物差しが電源で変わる（バッテリーは `blocks_battery_lid` =
//!   `fair` でも降りる / AC は `is_warning` = 従来どおり）。片方へ寄せたら落ちる
//! - **規則 C**: 残量と下限の比較が `lid_decision` の**外に無い**（判定の写しを作らない）
//! - **規則 D**: 検証用の注入（`TAKO_1473_INJECT_*`）は `inject_allowed()` の
//!   ガードを必ず通る。**本番の GUI が env で `pmset` を倒せてはいけない**
//! - **規則 E**: 材料の取得口（`battery_percent` / `current_thermal`）が注入を通る
//!   （通らないと隔離での検証そのものが空振りする）
//! - **規則 F**: 画面（設定タブ）は自前で条件を書かず、アプリが載せた理由を読む
//! - **規則 G**: 状態を作る経路（`update` / `status`）は理由まで埋めて返す
//!   （読む側が再計算すると、バイナリや A/B のアームが違うときに判断が割れる = #372）
//!
//! # 見逃す側へ倒れないための作り
//!
//! 走査が空振りすれば全部が無意味に緑になるので、[`走査が空振りしていない`] で
//! 採れた本体の数を固定し、[`安全弁を外す注入を名指しできる`] で**注入 9 通り**が
//! `file:line` で名指しされることを確かめる。範囲取りは #1420 の 1 実装
//! （`production_range`）を通すので、テスト内の記述は検査対象に入らない。

use std::path::{Path, PathBuf};

#[path = "common/production_range.rs"]
mod production_range;

const SLEEP_GUARD: &str = "crates/tako-control/src/sleep_guard.rs";
const SETTINGS_SLEEP: &str = "crates/tako-app/src/settings_sleep.rs";

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("crates/tako-control の 2 つ上がワークスペースルート")
        .to_path_buf()
}

fn read(root: &Path, rel: &str) -> String {
    let src =
        std::fs::read_to_string(root.join(rel)).unwrap_or_else(|e| panic!("{rel} を読めない: {e}"));
    // テスト領域は潰す（#1420）。バイト長と行番号は保たれるので file:line が使える
    production_range::production(&src, rel)
}

#[derive(Debug)]
struct Offender {
    file: &'static str,
    line: usize,
    why: String,
}

impl Offender {
    fn report(&self) -> String {
        format!("{}:{} — {}", self.file, self.line, self.why)
    }
}

/// `sig` で始まる関数の本体（`{` 〜 対応する `}`）と、その宣言行の行番号。
///
/// **行頭の定義だけ**を拾う（`iokit` モジュールの中にも同名の
/// `battery_percent` が在るので、素の `find` だと内側に当たる）
fn fn_body<'a>(src: &'a str, sig: &str) -> Option<(usize, &'a str)> {
    let start = if src.starts_with(sig) {
        0
    } else {
        src.find(&format!("\n{sig}"))? + 1
    };
    let line = src[..start].lines().count() + 1;
    let open = start + src[start..].find('{')?;
    let mut depth = 0usize;
    for (i, c) in src[open..].char_indices() {
        match c {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return Some((line, &src[open..open + i]));
                }
            }
            _ => {}
        }
    }
    None
}

/// 本体に `needle` が無ければ違反（安全弁を消した証拠）
fn require(
    out: &mut Vec<Offender>,
    file: &'static str,
    found: Option<(usize, &str)>,
    sig: &str,
    needles: &[(&str, &str)],
) {
    let Some((line, body)) = found else {
        out.push(Offender {
            file,
            line: 0,
            why: format!(
                "`{}` が見つからない（改名したなら番犬も直すこと）",
                sig.trim()
            ),
        });
        return;
    };
    for (needle, why) in needles {
        if !body.contains(needle) {
            out.push(Offender {
                file,
                line,
                why: format!("`{}` が `{needle}` を見ていない: {why}", sig.trim()),
            });
        }
    }
}

fn scan(sleep_guard: &str, settings_sleep: &str) -> Vec<Offender> {
    let mut out = Vec::new();

    // --- 規則 A / B: 判定が安全弁の材料をすべて見ている ---
    require(
        &mut out,
        SLEEP_GUARD,
        fn_body(sleep_guard, "pub fn lid_decision("),
        "lid_decision",
        &[
            (
                "battery_floor",
                "残量の下限を見ないと鞄の中で電池が空になる（#1473 の安全弁①）",
            ),
            (
                "battery_percent",
                "いまの残量を見ないと下限と比べようがない（#1473 の安全弁①）",
            ),
            (
                "BatteryUnknown",
                "残量が読めないときに続けると、止める条件を持てないまま走る（#1473）",
            ),
            (
                "LidSkipReason::Thermal",
                "高温で降りないと閉じたまま熱を持ち続ける（#1473 の安全弁②）",
            ),
            (
                "blocks_battery_lid",
                "バッテリーの物差し（fair で降りる）が消えている（#1473 の安全弁②）",
            ),
            (
                "is_warning",
                "AC の物差し（serious 以上）が消えている（#218 からの挙動）",
            ),
            (
                "NoAgents",
                "エージェントが居なくても倒し続けてはいけない（#1473 の安全弁③）",
            ),
        ],
    );

    // --- 規則 C: 残量と下限の比較が判定の外に無い ---
    let decision_body = fn_body(sleep_guard, "pub fn lid_decision(")
        .map(|(_, b)| b.to_string())
        .unwrap_or_default();
    for (idx, line) in sleep_guard.lines().enumerate() {
        let trimmed = line.trim();
        // 比較の形（`percent <= …battery_floor`）だけを見る。代入・構造体の初期化は対象外
        let compares = (trimmed.contains("<= input.battery_floor")
            || trimmed.contains("< input.battery_floor")
            || trimmed.contains("<= self.lid_battery_floor")
            || trimmed.contains("< self.lid_battery_floor"))
            && !trimmed.starts_with("//");
        if compares && !decision_body.contains(trimmed) {
            out.push(Offender {
                file: SLEEP_GUARD,
                line: idx + 1,
                why: "残量と下限の比較が `lid_decision` の外にある（判定の写しは作らない。#1473）"
                    .to_string(),
            });
        }
    }

    // --- 規則 D: 注入は隔離ガードを通る ---
    for sig in ["fn injected_battery_percent(", "fn injected_thermal("] {
        require(
            &mut out,
            SLEEP_GUARD,
            fn_body(sleep_guard, sig),
            sig.trim_end_matches('('),
            &[(
                "inject_allowed()",
                "本番の GUI が env で `pmset` の判断を左右できてしまう（#1473）",
            )],
        );
    }
    // 注入 env を読む場所は `injected_*` の中だけ
    let injected_bodies: String = ["fn injected_battery_percent(", "fn injected_thermal("]
        .iter()
        .filter_map(|sig| fn_body(sleep_guard, sig).map(|(_, b)| b.to_string()))
        .collect::<Vec<_>>()
        .join("\n");
    for (idx, line) in sleep_guard.lines().enumerate() {
        let trimmed = line.trim();
        if trimmed.contains("TAKO_1473_INJECT")
            && !trimmed.starts_with("//")
            && !trimmed.starts_with("///")
            && !injected_bodies.contains(trimmed)
        {
            out.push(Offender {
                file: SLEEP_GUARD,
                line: idx + 1,
                why: "注入 env を `injected_*`（隔離ガードつき）の外で読んでいる（#1473）"
                    .to_string(),
            });
        }
    }

    // --- 規則 E: 材料の取得口が注入を通る ---
    require(
        &mut out,
        SLEEP_GUARD,
        fn_body(sleep_guard, "pub fn battery_percent("),
        "battery_percent",
        &[(
            "injected_battery_percent()",
            "注入を通らないと隔離での検証が空振りする（#1473）",
        )],
    );
    require(
        &mut out,
        SLEEP_GUARD,
        fn_body(sleep_guard, "fn current_thermal("),
        "current_thermal",
        &[(
            "injected_thermal()",
            "注入を通らないと温度の安全弁を検証できない（#1473）",
        )],
    );

    // --- 規則 G: 状態を作る経路は理由まで埋める（#372 と同じ理屈） ---
    for sig in ["pub fn update(", "pub fn status("] {
        require(
            &mut out,
            SLEEP_GUARD,
            fn_body(sleep_guard, sig),
            sig,
            &[(
                ".with_decision()",
                "理由を載せずに返すと、読む側（CLI / 画面）が再計算する = バイナリや A/B が違うと判断が割れる（#1473 / #372）",
            )],
        );
    }

    // --- 規則 F: 画面は自前で条件を書かない ---
    require(
        &mut out,
        SETTINGS_SLEEP,
        // impl ブロックの中なのでインデントごと指定する（同名の別定義と取り違えない）
        fn_body(settings_sleep, "    pub fn lid_status("),
        "lid_status",
        &[(
            "sleep_guard::lid_decision(",
            "画面が判定を書き直すと、安全弁を足したときに表示だけ古い理由を出す（#1473 / #727）",
        )],
    );

    out
}

fn sources() -> (String, String) {
    let root = workspace_root();
    (read(&root, SLEEP_GUARD), read(&root, SETTINGS_SLEEP))
}

#[test]
fn 蓋閉じ継続の安全弁が外れていない() {
    let (sleep_guard, settings_sleep) = sources();
    let offenders = scan(&sleep_guard, &settings_sleep);
    assert!(
        offenders.is_empty(),
        "蓋閉じ継続の安全弁が外れている（#1473）:\n{}",
        offenders
            .iter()
            .map(Offender::report)
            .collect::<Vec<_>>()
            .join("\n")
    );
}

/// 走査そのものが空振りしていないこと。
/// 本体を 1 つも採れていなければ、上の検査は「何も見ずに緑」になる
#[test]
fn 走査が空振りしていない() {
    let (sleep_guard, settings_sleep) = sources();
    for sig in [
        "pub fn lid_decision(",
        "fn injected_battery_percent(",
        "fn injected_thermal(",
        "pub fn battery_percent(",
        "fn current_thermal(",
    ] {
        let body = fn_body(&sleep_guard, sig);
        assert!(body.is_some(), "{sig} の本体を採れていない");
        assert!(
            body.map(|(line, b)| line > 0 && b.len() > 40)
                .unwrap_or(false),
            "{sig} の本体が短すぎる（範囲取りが壊れている）"
        );
    }
    assert!(
        fn_body(&settings_sleep, "    pub fn lid_status(").is_some(),
        "設定画面側の判定を採れていない"
    );
    // テスト領域は潰れている（テスト内の記述を本番として拾わない）
    assert!(
        !sleep_guard.contains("fn battery_in("),
        "テストヘルパが本番の眺めに残っている（#1420 の範囲取りが効いていない）"
    );
}

fn expect_hit(clean: &(String, String), injected: (String, String), needle: &str) {
    assert!(
        clean.0 != injected.0 || clean.1 != injected.1,
        "注入が空振りしている（アンカーが古い）: {needle}"
    );
    let offenders = scan(&injected.0, &injected.1);
    assert!(
        offenders.iter().any(|o| o.why.contains(needle)),
        "注入「{needle}」を名指しできていない。検出したのは:\n{}",
        offenders
            .iter()
            .map(Offender::report)
            .collect::<Vec<_>>()
            .join("\n")
    );
    assert!(
        offenders.iter().all(|o| o.line > 0),
        "行番号が採れていない（file:line で名指しできない）: {needle}"
    );
}

#[test]
fn 安全弁を外す注入を名指しできる() {
    let clean = sources();
    assert!(
        scan(&clean.0, &clean.1).is_empty(),
        "注入前が既に汚れている"
    );
    let sg = |body: String| (body, clean.1.clone());
    let ss = |body: String| (clean.0.clone(), body);

    // (1) 残量の下限を無視する（鞄の中で電池が空になる形。#1473 の本体）
    expect_hit(
        &clean,
        sg(clean.0.replace("input.battery_floor", "0")),
        "battery_floor",
    );

    // (2) 残量が読めないときに続けてしまう
    expect_hit(
        &clean,
        sg(clean.0.replace(
            "            None => return Err(LidSkipReason::BatteryUnknown),",
            "",
        )),
        "BatteryUnknown",
    );

    // (3) 温度の枝ごと削る
    expect_hit(
        &clean,
        sg(clean.0.replace(
            "        return Err(LidSkipReason::Thermal(input.thermal));",
            "        return Ok(());",
        )),
        "LidSkipReason::Thermal",
    );

    // (4) バッテリーの物差しを AC と同じへ戻す（fair を見逃す）
    expect_hit(
        &clean,
        sg(clean.0.replace(
            "        input.thermal.blocks_battery_lid()",
            "        false",
        )),
        "blocks_battery_lid",
    );

    // (5) エージェント稼働中の条件を落とす
    expect_hit(
        &clean,
        sg(clean
            .0
            .replace("        return Err(LidSkipReason::NoAgents);", "")),
        "NoAgents",
    );

    // (6) 注入の隔離ガードを外す（本番の GUI が env で pmset を倒せる状態）
    expect_hit(
        &clean,
        sg(clean.0.replace(
            "fn injected_battery_percent() -> Option<u8> {\n    if !inject_allowed() {\n        return None;\n    }\n",
            "fn injected_battery_percent() -> Option<u8> {\n",
        )),
        "inject_allowed()",
    );

    // (7) 材料の取得口が注入を見ない（隔離での検証が空振りする）
    expect_hit(
        &clean,
        sg(clean.0.replace(
            "    if let Some(p) = injected_battery_percent() {",
            "    if let Some(p) = None::<u8> {",
        )),
        "injected_battery_percent()",
    );

    // (8) 状態を作る側が理由を載せない（読む側が再計算する形へ逆戻り）
    expect_hit(
        &clean,
        sg(clean.0.replace(".with_decision()", "")),
        ".with_decision()",
    );

    // (9) 画面が判定を書き直す（アプリの理由も材料も読まず、自分で決める）
    expect_hit(
        &clean,
        ss(clean.1.replace(
            "            None => sleep_guard::lid_decision(&self.lid_input()),",
            "            None => Err(LidSkipReason::NoAcPower),",
        )),
        "sleep_guard::lid_decision(",
    );
}
