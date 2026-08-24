//! プラットフォーム依存を閉じ込める抽象境界（`tako-control` 分）。
//!
//! 設計の正は `.agent/plans/2026-07-windows-port-architecture.md`。
//! 原則: **`cfg(target_os)` / `cfg(unix)` を書いてよいのはこのモジュール配下だけ**。
//! 呼び出し側（dispatch / remote / CLI）は単一のコードパスを持つ。
//!
//! 新しくプラットフォーム分岐が必要になったら、呼び出し側に `cfg` を足すのではなく
//! ここに境界を追加する。

pub mod facts;
/// 蓋を閉じたまま実行を継続する制御（境界 B9 の蓋ぶん。#697）
pub mod lid;
pub mod local_endpoint;
/// Layer 1 IPC の Windows トランスポート（named pipe。境界 B3）
#[cfg(windows)]
pub mod named_pipe;
pub mod os_integration;
/// アイドルスリープ防止（境界 B9。macOS 以外。#524）
pub mod power;
pub mod process;

/// テスト専用: **機械全体で 1 つしかない状態**を触るテストを直列化する錠。
///
/// 電源要求の保持（`power`）・電源プランの蓋設定と記録キャッシュ（`lid`）・
/// それらを束ねる `sleep_guard::update` は、どれもプロセスまたは機械の
/// グローバル状態を書く。`cargo test` は同一バイナリのテストを並列に走らせるので、
/// 素で並べると「相手が保持しているのに保持していないと期待する」形で確率的に落ちる
/// （言語グローバルで実害を出した #608 と同型。規約は `.agent/conventions.md`）。
///
/// 使う側は**関数の先頭で**取り、後始末より長く生かす（変数は宣言と逆順に落ちる）。
#[cfg(test)]
pub(crate) mod testing {
    use std::sync::{Mutex, MutexGuard};

    static MACHINE_STATE: Mutex<()> = Mutex::new(());

    pub(crate) fn machine_state_lock() -> MutexGuard<'static, ()> {
        MACHINE_STATE
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

use serde_json::{json, Value};
use tako_core::platform::support::{self, Note, Platform};

/// リリースノートの「Known limitations」節を対応マトリクス（#515）から組み立てる（#594）。
///
/// **リリースのたびに人手で書き写さない**ための生成器。マトリクスが唯一の正なので、
/// 機能が Windows 対応するとこの節から自動的に消える（書き換え忘れが構造的に起きない）。
///
/// 出力は**表示言語に依存しない**（日英を必ず併記する）。リリースノートは成果物であり、
/// 実行環境の言語設定で内容が変わってはならないので `Note::en()` / `Note::ja()` を直接使う。
///
/// 縮退が 1 件も無ければ空文字列を返す（呼び出し側は節ごと省略する）。
pub fn known_limitations_markdown(platform: Platform) -> String {
    // 同じ理由を共有する機能が多いので、理由ごとに畳んで「影響機能数 + 追跡 Issue」を出す
    let mut groups: Vec<(Note, usize, Vec<u32>)> = Vec::new();
    for feature in support::MATRIX {
        let s = feature.on(platform);
        let Some(note) = s.note() else { continue };
        if let Some(entry) = groups.iter_mut().find(|(n, _, _)| *n == note) {
            entry.1 += 1;
            if let Some(issue) = s.issue() {
                if !entry.2.contains(&issue) {
                    entry.2.push(issue);
                }
            }
        } else {
            groups.push((note, 1, s.issue().into_iter().collect()));
        }
    }
    if groups.is_empty() {
        return String::new();
    }
    // 影響の大きい順。同数は英文の辞書順で決める（出力を決定的にするため）
    groups.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.en().cmp(b.0.en())));
    for g in &mut groups {
        g.2.sort_unstable();
    }

    let label = tako_core::platform::release_assets::display_label(platform);
    let mut out = format!("### Known limitations ({label}) / {label} の既知の制限\n\n");
    out.push_str(&format!(
        "Generated from the platform support matrix (`tako platform --platform {}`).\n\
         プラットフォーム対応マトリクスから自動生成しています。\n\n",
        platform.as_str()
    ));
    for (note, count, issues) in &groups {
        let tracking = if issues.is_empty() {
            String::new()
        } else {
            let list: Vec<String> = issues.iter().map(|i| format!("#{i}")).collect();
            format!(", tracking: {}", list.join(" / "))
        };
        let unit = if *count == 1 { "feature" } else { "features" };
        out.push_str(&format!(
            "- **{}** ({count} {unit}{tracking})\n  {}\n",
            note.en(),
            note.ja()
        ));
    }
    out
}

/// プラットフォーム対応マトリクスの参照結果を組み立てる。
///
/// CLI（`tako platform`）と MCP（`tako_platform`）の**両方がこの 1 本を通る**ので、
/// 表示が食い違うことがない。`platform` 省略時は実行中のプラットフォーム、
/// `status` 省略時は全件を返す。
///
/// `known_limitations` を立てると、リリースノート用の Known limitations 節
/// （#594）を `known_limitations_markdown` フィールドに載せる。既定で載せないのは
/// AI が診断で叩く経路の応答を膨らませないため
pub fn report(
    platform: Option<&str>,
    status: Option<&str>,
    known_limitations: bool,
) -> Result<Value, String> {
    let target = match platform {
        Some(p) => Platform::parse(p)
            .ok_or_else(|| format!("未知のプラットフォーム: {p}（macos / windows）"))?,
        None => Platform::current(),
    };
    if let Some(s) = status {
        const KNOWN: [&str; 4] = ["supported", "degraded", "pending", "unsupported"];
        if !KNOWN.contains(&s) {
            return Err(format!("未知の状態: {s}（{}）", KNOWN.join(" / ")));
        }
    }
    let features: Vec<Value> = support::features(target, status)
        .into_iter()
        .map(|(f, s)| {
            let mut o = json!({ "key": f.key, "status": s.status() });
            // 理由文は表示言語に追従する（マトリクス 1 箇所定義・#435）
            if let Some(note) = s.note() {
                o["note"] = json!(note.text());
                // 表示言語に依存しない両言語も併せて返す（#591）。
                // **docs の生成物が実行環境の言語で変わってはいけない**
                // （生成した人のロケールで内容が変わると `--check` が CI で必ず落ちる。
                // 実際に踏んだ: 手元は日本語・CI は英語で不一致になった）
                o["note_ja"] = json!(note.ja());
                o["note_en"] = json!(note.en());
            }
            if let Some(issue) = s.issue() {
                o["issue"] = json!(issue);
            }
            // 判定の根拠（#591）。**何をもってそう言えるのか**を応答にも出す。
            // Windows 側だけなのは、macOS が開発機で常に実測されているため
            if target == Platform::Windows {
                o["evidence"] = json!(f.windows_evidence.kind());
                if let Some(citation) = f.windows_evidence.citation() {
                    o["evidence_detail"] = json!(citation);
                }
            }
            o
        })
        .collect();

    let counts = ["supported", "degraded", "pending", "unsupported"]
        .iter()
        .map(|s| {
            (
                s.to_string(),
                json!(support::features(target, Some(s)).len()),
            )
        })
        .collect::<serde_json::Map<_, _>>();

    let mut out = json!({
        "platform": target.as_str(),
        "current": Platform::current().as_str(),
        "filter": status,
        "counts": counts,
        "total": features.len(),
        "features": features,
    });
    if known_limitations {
        out["known_limitations_markdown"] = json!(known_limitations_markdown(target));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// docs の生成物（`scripts/gen-windows-support-docs.mjs`）が**実行環境の言語で
    /// 変わってはいけない**（#591）。
    ///
    /// `note` は表示言語に追従するので、生成器がそれを読むと
    /// 「日本語ロケールで生成 → 英語ロケールの CI で `--check` が落ちる」が起きる。
    /// 実際に踏んだので、言語に依存しない `note_ja` / `note_en` が必ず載ることを固定する
    #[test]
    fn 応答には言語に依存しない理由文が載る() {
        let v = report(Some("windows"), None, false).expect("windows の表を引けない");
        let features = v["features"].as_array().expect("features が配列でない");
        let mut checked = 0;
        for f in features {
            if f["status"] == "supported" {
                continue;
            }
            let key = f["key"].as_str().unwrap_or_default();
            let ja = f["note_ja"].as_str().unwrap_or_default();
            let en = f["note_en"].as_str().unwrap_or_default();
            assert!(!ja.is_empty(), "{key} に note_ja が無い");
            assert!(!en.is_empty(), "{key} に note_en が無い");
            assert!(
                !en.chars()
                    .any(|c| matches!(c as u32, 0x3040..=0x30FF | 0x4E00..=0x9FFF)),
                "{key} の note_en に日本語が残っている: {en}"
            );
            checked += 1;
        }
        assert!(checked > 0, "縮退が 1 件も無い（テストの前提が崩れている）");
    }

    /// 根拠（#591）も応答に載ること。docs の「根拠」列がここから来る
    #[test]
    fn 応答には判定の根拠が載る() {
        let v = report(Some("windows"), None, false).unwrap();
        for f in v["features"].as_array().unwrap() {
            let key = f["key"].as_str().unwrap_or_default();
            let kind = f["evidence"].as_str().unwrap_or_default();
            assert!(
                matches!(
                    kind,
                    "self-test" | "unit-test" | "measured" | "by-design" | "unverified"
                ),
                "{key} の evidence が未知の種別: {kind}"
            );
            if kind == "unverified" {
                assert!(
                    f["status"] == "pending",
                    "{key} は未実測なのに pending でない"
                );
            } else {
                assert!(
                    f["evidence_detail"].as_str().is_some_and(|s| !s.is_empty()),
                    "{key} に根拠の中身が無い"
                );
            }
        }
        // macOS 側は開発機なので根拠欄を持たない
        let mac = report(Some("macos"), None, false).unwrap();
        assert!(mac["features"][0]["evidence"].is_null());
    }

    #[test]
    fn known_limitations_lists_windows_gaps_bilingually() {
        let md = known_limitations_markdown(Platform::Windows);
        assert!(
            md.starts_with("### Known limitations (Windows) / Windows の既知の制限"),
            "見出しが期待と違う: {}",
            md.lines().next().unwrap_or_default()
        );
        // 日英併記（生成物なので実行環境の言語設定に依存してはならない）。
        //
        // **期待値は理由文の直書きではなくマトリクスから作る**。文面を直書きすると
        // 棚卸しで理由を書き換えるたびにここが落ちる（#591 で実際に踏んだ。
        // 同じ轍は #920 の install_plan でも踏んでいる）
        let sample = support::MATRIX
            .iter()
            .find_map(|f| f.on(Platform::Windows).note())
            .expect("Windows に縮退が 1 件も無い（テストの前提が崩れている）");
        assert!(md.contains(sample.en()), "英文が無い: {}", sample.en());
        assert!(md.contains(sample.ja()), "日本語文が無い: {}", sample.ja());
        // 追跡 Issue が付く
        assert!(md.contains("tracking: #"), "追跡 Issue が無い");
    }

    /// 生成物が表示言語に依存しないこと。
    ///
    /// `set_lang` はプロセスグローバルで並列テストと競合しフレークの元になる（過去に実害あり）ので
    /// 触らない。代わりに**日英どちらの文言も必ず含まれる**ことを構造的に検査する
    /// （`Note::text()` を使っていたら、その時点の言語の側しか出ないので落ちる）
    #[test]
    fn known_limitations_contains_both_languages_for_every_note() {
        let md = known_limitations_markdown(Platform::Windows);
        for feature in support::MATRIX {
            let Some(note) = feature.on(Platform::Windows).note() else {
                continue;
            };
            assert!(
                md.contains(note.en()),
                "英文が欠けている（{}）: {}",
                feature.key,
                note.en()
            );
            assert!(
                md.contains(note.ja()),
                "日本語文が欠けている（{}）: {}",
                feature.key,
                note.ja()
            );
        }
    }

    #[test]
    fn known_limitations_is_empty_when_nothing_degraded() {
        // macOS は全機能サポート済みなので節ごと出ない（= 呼び出し側が省略できる）
        let md = known_limitations_markdown(Platform::MacOs);
        assert!(md.is_empty(), "macOS に既知の制限が出ている: {md}");
    }

    #[test]
    fn report_includes_known_limitations_only_when_requested() {
        let plain = report(Some("windows"), None, false).unwrap();
        assert!(plain.get("known_limitations_markdown").is_none());
        let with = report(Some("windows"), None, true).unwrap();
        assert!(with["known_limitations_markdown"]
            .as_str()
            .is_some_and(|s| s.contains("Known limitations")));
    }
}
