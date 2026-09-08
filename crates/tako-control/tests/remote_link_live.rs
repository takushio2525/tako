//! 実 transcript に対するリンク解決の実測テスト（#1069）
//!
//! 合成 fixture（`claude_remote_link` の unit テスト）は形を固定するが、
//! **上流が実際に書く形**とはずれうる。ここは `~/.claude/projects/` の
//! 実ファイルを読んで、`connected` / `not_connected` の両方が実物で成立することを見る。
//!
//! ## 実行環境に依存する
//!
//! claude を使っていないマシン（CI）では材料が無いので**理由を出して skip する**
//! （落とすとクリーンな環境で常に赤くなる）。材料があるときだけ本物を検査する。
//!
//! ## 出力に実値を出さない
//!
//! 見つけた session id / URL は**ログへ出さない**（#1069 の番犬と同じ基準）。
//! 出すのは件数と判定結果だけ。

use tako_control::claude_remote_link::{self, LinkState};

/// 実 transcript のパスを数本返す（**読み直しのコスト測定用**）
fn sample_paths() -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    for dir in tako_control::transcript::claude_config_dirs() {
        let Ok(projects) = std::fs::read_dir(dir.join("projects")) else {
            continue;
        };
        for project in projects.flatten() {
            let Ok(files) = std::fs::read_dir(project.path()) else {
                continue;
            };
            for file in files.flatten() {
                let path = file.path();
                if path.extension().is_none_or(|e| e != "jsonl") {
                    continue;
                }
                if path.metadata().map(|m| m.len()).unwrap_or(0) < 64 * 1024 {
                    continue; // 小さすぎるものは末尾優先の効果が測れない
                }
                out.push(path);
                if out.len() >= 5 {
                    return out;
                }
            }
        }
    }
    out
}

/// `n` 件ぶんのダミー行（`session_id` は実在のものを使い回す）
fn rows_for(ids: &[String], n: usize) -> Vec<serde_json::Value> {
    (0..n)
        .map(|i| serde_json::json!({ "session_id": ids[i % ids.len()] }))
        .collect()
}

/// 実 transcript を (bridge 行あり, bridge 行なし) に分けて session id を返す。
/// **中身は返さない**（呼び出し側が id を出さないようにするため件数だけ持たせる）
fn sample_sessions() -> (Vec<String>, Vec<String>) {
    let mut with_bridge = Vec::new();
    let mut without_bridge = Vec::new();
    for dir in tako_control::transcript::claude_config_dirs() {
        let Ok(projects) = std::fs::read_dir(dir.join("projects")) else {
            continue;
        };
        for project in projects.flatten() {
            let Ok(files) = std::fs::read_dir(project.path()) else {
                continue;
            };
            for file in files.flatten() {
                let path = file.path();
                if path.extension().is_none_or(|e| e != "jsonl") {
                    continue;
                }
                // 巨大な会話は読み飛ばす（このテストの目的は形の確認）
                if path.metadata().map(|m| m.len()).unwrap_or(u64::MAX) > 8 * 1024 * 1024 {
                    continue;
                }
                let Ok(text) = std::fs::read_to_string(&path) else {
                    continue;
                };
                let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
                    continue;
                };
                if !tako_control::transcript::is_valid_session_id(stem) {
                    continue;
                }
                let has_bridge =
                    text.contains("\"bridge-session\"") || text.contains("\"bridge_status\"");
                if has_bridge {
                    if with_bridge.len() < 5 {
                        with_bridge.push(stem.to_string());
                    }
                } else if without_bridge.len() < 5 {
                    without_bridge.push(stem.to_string());
                }
                if with_bridge.len() >= 5 && without_bridge.len() >= 5 {
                    return (with_bridge, without_bridge);
                }
            }
        }
    }
    (with_bridge, without_bridge)
}

#[test]
fn 実transcriptで接続済みと未接続を言い分ける() {
    let (with_bridge, without_bridge) = sample_sessions();
    if with_bridge.is_empty() && without_bridge.is_empty() {
        eprintln!("skip: この環境に claude の transcript が無い（材料が無いので実測できない）");
        return;
    }

    // ① bridge 行を持つ会話は connected になり、**claude.ai/code の形の URL** が出る
    for sid in &with_bridge {
        let link = claude_remote_link::link_for_agent_session("claude", Some(sid));
        assert_eq!(
            link.state,
            LinkState::Connected,
            "bridge 行を持つ会話が connected にならない（id は出さない）"
        );
        let url = link.url.as_deref().expect("connected なら URL が要る");
        assert!(
            url.starts_with("https://claude.ai/code/session_"),
            "URL の形が違う（先頭 30 文字: {}）",
            &url[..url.len().min(30)]
        );
        let session_id = link.session_id.as_deref().expect("id が要る");
        assert!(session_id.starts_with("session_"), "互換形式になっていない");
        // **アカウント UUID を混ぜていない**（実ファイルには入っている）
        let rendered = link.to_json().to_string();
        assert!(
            !rendered.contains("owner"),
            "応答にアカウント情報が混ざっている"
        );
    }

    // ② bridge 行が無い会話は not_connected（**URL を捏造しない**）。
    // ただしこのマシン全体に阻害要因（DISABLE_TELEMETRY 等）があると
    // ineligible になるのが正しいので、どちらかであることを見る
    for sid in &without_bridge {
        let link = claude_remote_link::link_for_agent_session("claude", Some(sid));
        assert!(
            matches!(
                link.state,
                LinkState::NotConnected | LinkState::Ineligible { .. }
            ),
            "bridge 行が無い会話が {:?} になっている（URL を捏造している）",
            link.state
        );
        assert!(link.url.is_none(), "未接続なのに URL がある");
        assert!(link.session_id.is_none(), "未接続なのに id がある");
    }

    eprintln!(
        "実測: connected {} 件 / not_connected（または ineligible）{} 件",
        with_bridge.len(),
        without_bridge.len()
    );
}

/// claude 以外の系統は**会話を特定できても** ineligible（マトリクスの宣言と同じ）
#[test]
fn claude以外の系統はineligibleになる() {
    let (with_bridge, _) = sample_sessions();
    let sid = with_bridge.first().cloned();
    for agent in ["codex", "agy", "local", "plain"] {
        let link = claude_remote_link::link_for_agent_session(agent, sid.as_deref());
        assert!(
            matches!(link.state, LinkState::Ineligible { .. }),
            "{agent} が {:?} になっている",
            link.state
        );
        assert_eq!(link.state.as_wire(), "ineligible: agent_unsupported");
        assert!(link.url.is_none(), "{agent} に URL を出している");
    }
}

/// 存在しない会話は unknown（**not_connected と言い切らない**）
#[test]
fn 見つからない会話はunknownになる() {
    // UUID の形だが実在しない
    let link = claude_remote_link::link_for_agent_session(
        "claude",
        Some("00000000-0000-4000-8000-000000000000"),
    );
    assert_eq!(link.state, LinkState::Unknown);
    assert!(link.url.is_none());
    // 空・None も unknown
    assert_eq!(
        claude_remote_link::link_for_agent_session("claude", None).state,
        LinkState::Unknown
    );
    assert_eq!(
        claude_remote_link::link_for_agent_session("claude", Some("")).state,
        LinkState::Unknown
    );
}

/// 一覧経路のコスト（`/api/v2/panes` は PWA がポーリングする）。
/// **UI スレッドではない**（daemon 側）が、ペイン数ぶん transcript を探して読むので
/// コストを固定しておく。
///
/// ## 実時間では測らない（#1220 / #1167）
///
/// 旧実装は「2 回目の付与が初回より速いこと」を `Instant::elapsed` の比較
/// （`warm <= cold`）で固定していた。これは片方の計測窓にだけスケジューリングの
/// 待ちが入った回に落ちる（実測: 20 行の初回 23.8ms / 2 回目 1.7ms なので、
/// 2 回目に 22ms 止まれば反転する。他 worker のビルドと同時に走って 1 回 FAILED）。
/// 規約は `.agent/conventions.md`「効果を測る単体テストは実時間で比べない」。
///
/// ## 「コストが桁で問題ない」を量で書き直す
///
/// 1. **行数に比例しない**: 20 行の付与でも走査は distinct な会話数ぶんだけ
///    （同じ会話を行ごとに読み直さない = 行内で memo が効く）
/// 2. **定常状態は追記ぶんだけ**: 2 回目（= ポーリングの実態）の読み出しは
///    **上限 64 KiB**。全走査は 1 件で MB 級なので桁で開いている
/// 3. **所在探索は 1 会話 1 回**: `projects/` の全走査（cold の支配項）は 2 回目に 0 回
///
/// どれも混み具合に依らないので、他 worker のビルドと同時でも揺れない。
/// 生きている会話は計測中も書かれ続けるので、**窓の中で実際に追記されたぶんは
/// 予算へ足す**（読むのが正しい量なので、そこは失敗にしない）。
///
/// **限界**: 数えているのは transcript の走査（`scan_counters`）だけなので、
/// 付与の中の別のコスト（`accounts.yaml` の読み直し等）はここでは見ていない。
/// そちらを固定したくなったら、同じ形の口をその層へ開ける
#[test]
fn 一覧付与のコストが桁で問題ないこと() {
    let (with_bridge, without_bridge) = sample_sessions();
    let mut ids: Vec<String> = with_bridge;
    ids.extend(without_bridge);
    if ids.is_empty() {
        eprintln!("skip: 材料が無い");
        return;
    }
    // 20 ペイン相当（実運用の上限に近い）。同じ会話が何度も並ぶ = ポーリングの実態
    let rows = rows_for(&ids, 20);
    let used: Vec<String> = rows
        .iter()
        .filter_map(|r| r["session_id"].as_str())
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .map(str::to_string)
        .collect();
    let distinct = used.len() as u64;
    // 計測窓のあいだの追記量を見るための実サイズ（**パスは出さない**。#927）
    let sizes_before = transcript_sizes(&used);

    // 1 回目（memo が空。同じ binary の他テストが先に温めていれば 0 回になりうる = 上限で見る）
    let at = claude_remote_link::scan_counters();
    let mut first = serde_json::json!({ "agents": rows.clone() });
    claude_remote_link::attach_to_agents(&mut first);
    let cold = claude_remote_link::scan_counters().since(at);

    // 2 回目以降（= PWA のポーリングの実態。mtime が動いていなければ読まない）
    let at = claude_remote_link::scan_counters();
    let mut second = serde_json::json!({ "agents": rows });
    claude_remote_link::attach_to_agents(&mut second);
    let warm = claude_remote_link::scan_counters().since(at);

    let sizes_after = transcript_sizes(&used);
    let (appended_bytes, appended_sessions) = appended(&sizes_before, &sizes_after);

    // 初回の全走査 1 件ぶん（会話ごとに 1 回だけ通る経路）を**同じカウンタ**で測る。
    // **生きている会話のポーリングはここを通らない**（追記ぶんだけ読む形なので、
    // その計測は `claude_remote_link` の `追記ぶんだけ読むと定常コストが増えない` にある）
    let paths = sample_paths();
    let full = {
        let at = claude_remote_link::scan_counters();
        for path in &paths {
            let _ = claude_remote_link::read_link_at(path);
        }
        let cost = claude_remote_link::scan_counters().since(at);
        (!paths.is_empty()).then(|| cost.bytes / paths.len() as u64)
    };

    eprintln!(
        "20 行 / distinct {distinct} 会話の付与: 初回 {cold:?}｜2 回目 {warm:?}｜\
         窓の中の追記 {appended_bytes} B / {appended_sessions} 会話｜\
         全走査 1 件の読み出し {full:?} B"
    );

    // 結果は同じ（memo が値を変えていない）
    assert_eq!(first, second, "memo が結果を変えている");

    // ① 行数ぶん読まない（20 行 → 走査は distinct 会話数ぶん）
    assert!(
        cold.scans <= distinct + appended_sessions,
        "20 行の付与で {} 回走査している（distinct は {distinct} 会話 + 窓の中で\n\
         追記された {appended_sessions} 会話）。同じ会話を行ごとに読み直している\n\
         = 行内で memo が効いていない",
        cold.scans
    );
    assert!(
        cold.locates <= distinct,
        "20 行の付与で所在探索が {} 回（distinct は {distinct} 会話）。\n\
         `projects/` の全走査を会話ごとに 1 回より多くやっている = 所在の memo が効いていない",
        cold.locates
    );

    // ② 定常状態は追記ぶんだけ（**絶対上限**。比で書くと両方が同じ理由で伸びたときに
    // 成立してしまう = #1167 の規約）
    const STEADY_BUDGET: u64 = 64 * 1024;
    assert!(
        warm.bytes <= STEADY_BUDGET + appended_bytes,
        "2 回目のポーリングで {} B 読んでいる（上限 {STEADY_BUDGET} B + 窓の中の追記 {appended_bytes} B）。\n\
         mtime が動いていない会話を読み直している、または追記ぶんだけ読む形が壊れている\n\
         （全走査は 1 件 {full:?} B）",
        warm.bytes
    );

    assert!(
        warm.scans <= appended_sessions,
        "2 回目のポーリングで {} 回走査している（窓の中で追記されたのは {appended_sessions} 会話）。\n\
         mtime が動いていない会話を読みに行っている = memo の 1 段目が効いていない",
        warm.scans
    );

    // ③ 所在探索は 1 会話 1 回（2 回目は探し直さない）
    assert_eq!(
        warm.locates, 0,
        "2 回目のポーリングで所在探索を {} 回やっている（`projects/` の全走査は cold の支配項）",
        warm.locates
    );

    // ② の上限が桁で開いていること（材料が小さいと空振りの検査になる）
    if let Some(full) = full {
        assert!(
            full > STEADY_BUDGET,
            "全走査 1 件が {full} B しかない = 上限 {STEADY_BUDGET} B と桁が開いていない\n\
             （`sample_paths` が 64 KiB 以上のファイルを選べていない）"
        );
    }
}

/// 会話 id → transcript の実サイズ（**パスも id も出さない**）。
/// 計測窓のあいだに追記された量を出すために使う
fn transcript_sizes(ids: &[String]) -> Vec<u64> {
    ids.iter()
        .map(|id| {
            tako_control::transcript::locate_transcript(id)
                .and_then(|loc| loc.path.metadata().ok())
                .map(|m| m.len())
                .unwrap_or(0)
        })
        .collect()
}

/// (追記された合計バイト数, 追記された会話数)。縮んだものは 0 扱い
fn appended(before: &[u64], after: &[u64]) -> (u64, u64) {
    let deltas = before
        .iter()
        .zip(after.iter())
        .map(|(b, a)| a.saturating_sub(*b));
    deltas.fold((0, 0), |(bytes, sessions), d| match d {
        0 => (bytes, sessions),
        d => (bytes + d, sessions + 1),
    })
}
