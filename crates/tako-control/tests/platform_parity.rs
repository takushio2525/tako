//! プラットフォーム対応マトリクスのパリティテスト（設計 §3.2 の T1 / T2 / T6 と
//! MCP ツール表のドリフト検出）。
//!
//! 狙いは 1 つ: **mac で先行開発している間に Windows への反映漏れが溜まっても、
//! 人間の記憶ではなくテストが落ちて気付く**こと。
//!
//! 設計の正: `.agent/plans/2026-07-windows-port-architecture.md`

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use tako_control::mcp;
use tako_core::platform::support::MATRIX;

/// リポジトリルート（`crates/tako-control` から 2 つ上）
fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("リポジトリルートを解決できない")
        .to_path_buf()
}

/// MCP が実際に公開しているツール名。**これがキーの正**
fn advertised_tools() -> BTreeSet<String> {
    mcp::tools()
        .iter()
        .filter_map(|t| t["name"].as_str().map(String::from))
        .collect()
}

fn matrix_keys() -> BTreeSet<String> {
    MATRIX.iter().map(|f| f.key.to_string()).collect()
}

/// T1 被覆: 公開されている全ツールがマトリクスに分類されていること。
///
/// **新機能を足してマトリクスに分類し忘れると、ここが落ちる。**
/// tako の開発不変条件「新機能は必ず MCP / CLI から操作できる」により
/// 新機能は必ずツールを増やすので、この 1 本で反映漏れを捕まえられる。
#[test]
fn t1_全mcpツールがマトリクスに分類されている() {
    let missing: Vec<_> = advertised_tools()
        .difference(&matrix_keys())
        .cloned()
        .collect();
    assert!(
        missing.is_empty(),
        "マトリクス未分類のツールがある: {missing:?}\n\
         → crates/tako-core/src/platform/support.rs の MATRIX に追加し、\n\
         macOS / Windows それぞれの対応状況を宣言してください"
    );
}

/// T2 逆被覆: マトリクスに、もう存在しない機能が残っていないこと
#[test]
fn t2_マトリクスに存在しない機能が残っていない() {
    let stale: Vec<_> = matrix_keys()
        .difference(&advertised_tools())
        .cloned()
        .collect();
    assert!(
        stale.is_empty(),
        "MCP に存在しないキーがマトリクスに残っている: {stale:?}\n\
         → 機能を削除したなら MATRIX からも消してください"
    );
}

/// MCP ツール表とセルフテスト用スナップショットのドリフト検出。
///
/// スナップショットは GUI セルフテスト（項目 32）でしか照合されないため、
/// 再生成を忘れたまま main に入ると気付けない。実際 2026-07-25 に
/// `tako_git_show`（#495）と `tako_stale_binary`（#498）の 2 件が欠落していた。
/// `cargo test` で落ちるようにして、GUI を起動しなくても検出できるようにする。
#[test]
fn mcpツール表とスナップショットが一致する() {
    let snap_path = repo_root().join("crates/tako-app/testdata/mcp_tools_snapshot.txt");
    let snap = std::fs::read_to_string(&snap_path)
        .unwrap_or_else(|e| panic!("スナップショットを読めない {}: {e}", snap_path.display()));
    let snapshot: BTreeSet<String> = snap
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(String::from)
        .collect();
    let tools = advertised_tools();

    let added: Vec<_> = tools.difference(&snapshot).cloned().collect();
    let removed: Vec<_> = snapshot.difference(&tools).cloned().collect();
    assert!(
        added.is_empty() && removed.is_empty(),
        "MCP ツール表とスナップショットが食い違っている\n\
         スナップショット未登録: {added:?}\n\
         スナップショットに残存: {removed:?}\n\
         → {} を実際のツール表に合わせて更新してください",
        snap_path.display()
    );
}

/// T6 単一ソース: system prompt / setup 配布物をプラットフォーム別に複製していないこと。
///
/// 複製は必ずドリフトする。プラットフォーム差はレンダリング時に注入する（設計 §4）。
#[test]
fn t6_プロンプトと配布物がプラットフォーム別に複製されていない() {
    let root = repo_root();
    // 設計 §4 が対象にしている「正本」の置き場
    let targets = [
        root.join("resources"),
        root.join("crates/tako-control/src/orchestrator"),
    ];
    let mut duplicated = Vec::new();
    for dir in &targets {
        collect_platform_suffixed(dir, &mut duplicated);
    }
    assert!(
        duplicated.is_empty(),
        "プラットフォーム別に複製されたファイルがある: {duplicated:?}\n\
         → 正本は 1 本に保ち、差分は PlatformFacts の注入で表現してください（設計 §4）"
    );
}

/// `*-windows.md` / `*_macos.yaml` のようなプラットフォーム別複製を再帰的に探す
fn collect_platform_suffixed(dir: &Path, out: &mut Vec<String>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_platform_suffixed(&path, out);
            continue;
        }
        let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        let lower = stem.to_ascii_lowercase();
        for suffix in ["-windows", "_windows", "-macos", "_macos", "-win", "-mac"] {
            if lower.ends_with(suffix) {
                out.push(path.display().to_string());
                break;
            }
        }
    }
}

/// **受け入れ条件 4**: `changes.yaml` の `platforms:` が効くこと。
/// setup 配布物を OS ごとに複製しないための仕組み（設計 §4）
mod setup_changes_platforms {
    use tako_control::setup;
    use tako_core::platform::support::Platform;

    /// 省略時は全プラットフォームが対象（既存エントリの後方互換）
    #[test]
    fn platforms省略は全プラットフォーム対象() {
        let mac = setup::changes_for(Platform::MacOs).expect("パースできること");
        let win = setup::changes_for(Platform::Windows).expect("パースできること");
        assert!(!mac.is_empty(), "changelog が空");
        // 現行の changes.yaml は platforms 未指定のみ = 両プラットフォームで同数
        assert_eq!(
            mac.len(),
            win.len(),
            "platforms 未指定のエントリは両プラットフォームに配信されること"
        );
        assert!(mac.iter().all(|c| c.platforms.is_none()));
    }

    /// 指定されたプラットフォームだけに配信されること
    #[test]
    fn platforms指定は対象プラットフォームだけに配信される() {
        let win_only = setup::SetupChange {
            revision: 999,
            version: "0.0.0".into(),
            date: "2026-07-26".into(),
            kind: setup::ChangeKind::Auto,
            title: "Windows 限定の変更".into(),
            description: "テスト用".into(),
            platforms: Some(vec!["windows".into()]),
        };
        assert!(win_only.applies_to(Platform::Windows));
        assert!(!win_only.applies_to(Platform::MacOs));

        let both = setup::SetupChange {
            platforms: Some(vec!["macos".into(), "windows".into()]),
            ..win_only.clone()
        };
        assert!(both.applies_to(Platform::MacOs) && both.applies_to(Platform::Windows));

        let all = setup::SetupChange {
            platforms: None,
            ..win_only
        };
        assert!(all.applies_to(Platform::MacOs) && all.applies_to(Platform::Windows));
    }
}

/// **受け入れ条件 1**: 実際の正本 3 本が、同じファイルから両プラットフォーム向けに
/// レンダリングでき、差分がプレースホルダ部分だけであること
mod prompt_single_source {
    use tako_control::platform::facts::{render_in, PlatformFacts, PLACEHOLDER};
    use tako_core::i18n::Lang;
    use tako_core::platform::support::Platform;

    fn sources() -> Vec<(&'static str, &'static str)> {
        vec![
            ("master", tako_control::orchestrator::DEFAULT_SYSTEM_PROMPT),
            ("solo", tako_control::orchestrator::SOLO_SYSTEM_PROMPT),
            ("setup", tako_control::setup::SYSTEM_PROMPT),
        ]
    }

    /// 正本にプレースホルダが**ちょうど 1 個**あること（複数種類を増やさない）
    #[test]
    fn 正本のプレースホルダは1種類1個だけ() {
        for (name, src) in sources() {
            assert_eq!(
                src.matches(PLACEHOLDER).count(),
                1,
                "{name} の正本に {PLACEHOLDER} がちょうど 1 個ない"
            );
            // 別種のプレースホルダを勝手に増やしていないこと
            let others: Vec<&str> = src
                .match_indices("{{")
                .map(|(i, _)| &src[i..src[i..].find("}}").map(|e| i + e + 2).unwrap_or(i + 2)])
                .filter(|m| *m != PLACEHOLDER)
                .collect();
            assert!(
                others.is_empty(),
                "{name} の正本に未知のプレースホルダがある: {others:?}"
            );
        }
    }

    #[test]
    fn 正本から両プラットフォームを描き分けられ差分は注記部分だけ() {
        for (name, src) in sources() {
            let (prefix, suffix) = src.split_once(PLACEHOLDER).unwrap();
            for lang in [Lang::Ja, Lang::En] {
                let mac = render_in(src, &PlatformFacts::for_platform(Platform::MacOs), lang);
                let win = render_in(src, &PlatformFacts::for_platform(Platform::Windows), lang);
                assert_ne!(mac, win, "{name}: プラットフォームで内容が変わらない");
                for (label, out) in [("macos", &mac), ("windows", &win)] {
                    assert!(
                        !out.contains(PLACEHOLDER),
                        "{name}/{label}: プレースホルダが残っている"
                    );
                    assert!(
                        out.starts_with(prefix),
                        "{name}/{label}: 正本の前半が変わっている"
                    );
                    assert!(
                        out.ends_with(suffix),
                        "{name}/{label}: 正本の後半が変わっている"
                    );
                }
                assert!(win.contains("Windows"), "{name}: Windows 版に OS 名が無い");
                assert!(mac.contains("macOS"), "{name}: macOS 版に OS 名が無い");
            }
        }
    }

    /// 縮退の説明はマトリクス由来なので、Windows 版には理由が列挙される
    #[test]
    fn windows版には縮退理由がマトリクスから入る() {
        let notes = tako_core::platform::support::degraded_note_items(Platform::Windows);
        assert!(!notes.is_empty());
        for (name, src) in sources() {
            let win = render_in(
                src,
                &PlatformFacts::for_platform(Platform::Windows),
                Lang::Ja,
            );
            for note in &notes {
                assert!(
                    win.contains(note.ja()),
                    "{name}: 縮退理由 {:?} が prompt に入っていない",
                    note.ja()
                );
            }
        }
    }
}
