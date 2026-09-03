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

    /// 省略時は全プラットフォームが対象（既存エントリの後方互換）。
    ///
    /// **`platforms:` を実際に使うエントリが入ってからは「全部が未指定」では縛れない**
    /// （#525 の revision 15 が Windows 限定になった）。縛るべき不変条件は
    /// 「未指定のものは両方に出る」ことなので、そこだけを見る
    #[test]
    fn platforms省略は全プラットフォーム対象() {
        let mac = setup::changes_for(Platform::MacOs).expect("パースできること");
        let win = setup::changes_for(Platform::Windows).expect("パースできること");
        assert!(!mac.is_empty(), "changelog が空");

        let shared: Vec<u32> = mac
            .iter()
            .filter(|c| c.platforms.is_none())
            .map(|c| c.revision)
            .collect();
        assert!(
            !shared.is_empty(),
            "platforms 未指定のエントリが 1 件も無い"
        );
        for rev in &shared {
            assert!(
                win.iter().any(|c| c.revision == *rev),
                "platforms 未指定の revision {rev} が Windows 側で消えている"
            );
        }
        // 逆向き: Windows 側の未指定エントリも macOS に出る
        for c in win.iter().filter(|c| c.platforms.is_none()) {
            assert!(
                mac.iter().any(|m| m.revision == c.revision),
                "platforms 未指定の revision {} が macOS 側で消えている",
                c.revision
            );
        }
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

/// **#522 受け入れ条件 2**: OS シェル連携（`open` / `osascript` / `cmd /C start` /
/// `xdg-open`）の直接呼び出しが、抽象境界 B8 の外に残っていないこと。
///
/// 呼び出し側に `#[cfg(target_os = …)]` を足して塞ぐ変更を構造的に防ぐための番犬。
/// 新しい直呼びが増えたらここで落ちる（設計原則 1）。
#[test]
fn os連携の直呼びが境界の外に残っていない() {
    // 境界そのもの、および「B8 ではない別の境界の内側」だけを許す。
    // 許可には必ず理由を書く（黙って穴を開けない）
    const ALLOWED: &[(&str, &str)] = &[
        (
            "crates/tako-control/src/platform/os_integration.rs",
            "境界 B8 の実装本体",
        ),
        (
            "crates/tako-control/src/sleep_guard.rs",
            "B9（スリープ防止）の権限昇格。OS シェルに何かを開かせる操作ではなく、\
             汎用の昇格 API を B8 に置くと危険なため境界 B9 の内側に留める",
        ),
    ];
    // 実際にプロセスを起こす形・シェル API を直接叩く形だけを対象にする
    // （コメント・ドキュメント中の言及は無視）
    const PATTERNS: &[&str] = &[
        "Command::new(\"open\")",
        "Command::new(\"osascript\")",
        "Command::new(\"xdg-open\")",
        "\"/C\", \"start\"",
        // Windows 側（#617）。プロセス起動ではなく FFI なので関数名で見る
        "Command::new(\"explorer",
        "ShellExecuteW(",
        "SHFileOperationW(",
    ];

    let root = repo_root();
    let mut offenders = Vec::new();
    for crate_dir in ["tako-core", "tako-control", "tako-app", "tako-cli"] {
        let src = root.join("crates").join(crate_dir).join("src");
        collect_os_shell_calls(&src, &root, PATTERNS, ALLOWED, &mut offenders);
    }
    assert!(
        offenders.is_empty(),
        "OS シェル連携の直呼びが境界の外にある:\n  {}\n\
         → tako_control::platform::os_integration へ寄せてください（#522 / 設計 §2 の B8）",
        offenders.join("\n  ")
    );
}

/// **ゴミ箱への移動が完全削除へ劣化していない**（#617。データ消失の再発防止）。
///
/// 修正前は非 macOS の `move_to_trash` が `remove_file` / `remove_dir_all` で、
/// ラベルは「Move to Trash」なのに復元できなかった。UI にもコマンドにも確認が無いので、
/// 誰かが「未対応環境では消すだけにしておこう」と戻したら **無音でデータを失う**。
/// 境界の実装本体（テストコードより前）に削除の直呼びが 1 つも無いことを固定する。
/// ソース走査なので **macOS からも Windows CI からも同じ判定が走る**
#[test]
fn ゴミ箱移動が完全削除へ劣化していない() {
    let path = repo_root().join("crates/tako-control/src/platform/os_integration.rs");
    let src = std::fs::read_to_string(&path).expect("境界 B8 の実装本体を読める");

    // 走査対象は実装だけ（テストは後始末で remove_file を使う）。
    // 目印が無くなったら「走査範囲が空 = いつでも通る」になるので先に落とす
    let (impl_src, _) = src
        .split_once("#[cfg(test)]")
        .expect("os_integration.rs に #[cfg(test)] の目印がある");

    // 走査先を間違えていないこと（ファイル移動・改名で空振りするのを防ぐ）
    assert!(
        impl_src.contains("pub fn move_to_trash"),
        "move_to_trash が見つからない: 走査先が間違っている"
    );
    assert!(
        impl_src.contains("FOF_ALLOWUNDO"),
        "Windows のごみ箱フラグが見つからない: 走査先が間違っている"
    );

    let mut offenders = Vec::new();
    for (i, line) in impl_src.lines().enumerate() {
        // コメント行の言及（「削除へ劣化させない」の説明）は対象外
        let code = line.trim_start();
        if code.starts_with("//") || code.starts_with("///") {
            continue;
        }
        for pattern in ["remove_file(", "remove_dir_all(", "remove_dir("] {
            if code.contains(pattern) {
                offenders.push(format!("os_integration.rs:{}: {}", i + 1, line.trim()));
            }
        }
    }
    assert!(
        offenders.is_empty(),
        "ゴミ箱への移動が完全削除になりうる（復元できない）:\n  {}\n         → ゴミ箱へ入れられない環境では削除せずエラーにしてください（#617）",
        offenders.join("\n  ")
    );
}

fn collect_os_shell_calls(
    dir: &Path,
    root: &Path,
    patterns: &[&str],
    allowed: &[(&str, &str)],
    out: &mut Vec<String>,
) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_os_shell_calls(&path, root, patterns, allowed, out);
            continue;
        }
        if path.extension().is_none_or(|e| e != "rs") {
            continue;
        }
        // ALLOWED と報告文はどちらも `/` 区切りで書く。Windows の `strip_prefix` は
        // `\` 区切りを返すので、ここで正規化しないと許可が 1 件も一致せず
        // 境界の実装本体まで違反として報告される（Windows 実機で実測）
        let rel = path
            .strip_prefix(root)
            .unwrap_or(&path)
            .display()
            .to_string()
            .replace('\\', "/");
        if allowed.iter().any(|(p, _)| rel == *p) {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        for (idx, line) in text.lines().enumerate() {
            if patterns.iter().any(|p| line.contains(p)) {
                out.push(format!("{rel}:{}", idx + 1));
            }
        }
    }
}

/// **#877 の番犬**: エージェント走査（`claude agents --json`）が、tako 自身で
/// POSIX シェルを起こす形へ戻っていないこと。
///
/// `$SHELL -l -c <シェル片>` は Windows で必ず失敗する（実測: `SHELL` 未設定なら
/// `/bin/sh` へ落ちて `CreateProcess` が「指定されたファイルが見つかりません」、
/// `SHELL=powershell.exe` でも `-l : The term '-l' is not recognized`）。
/// 走査は抽象境界 B21（`tako_core::platform::child_cmd`）を通すのが正で、
/// Windows は PATH で解決した実体を直接起動する。
///
/// **macOS のゲートは倒しても全部緑になる**ので、ソース走査で塞いでおく
/// （同型の一族は #875 / スライス 8 の棚卸しが対象。ここはオーケストレーション層だけを見る）
#[test]
fn agents走査がposixシェルの直起動へ戻っていない() {
    const PATTERNS: &[&str] = &["\"-l\", \"-c\"", "var(\"SHELL\")"];

    let dir = repo_root()
        .join("crates")
        .join("tako-control")
        .join("src")
        .join("orchestrator");
    let mut offenders = Vec::new();
    collect_os_shell_calls(&dir, &repo_root(), PATTERNS, &[], &mut offenders);
    assert!(
        offenders.is_empty(),
        "オーケストレーション層に POSIX シェルの直起動が残っている:\n  {}\n\
         → tako_core::platform::child_cmd::user_env_cli へ寄せてください（#877 / 境界 B21）",
        offenders.join("\n  ")
    );
}

/// **#722 の番犬**: 実行ファイルの探索が、ログインシェルへ `command -v` を尋ねる形で
/// 抽象境界 B16（`tako_core::platform::exe::find`）の外に残っていないこと。
///
/// この直呼びは **Windows で必ず失敗するのに `Option` へ化けて握り潰される**のが厄介で、
/// 「機能が無い」ではなく「機能が黙って無効になる」形で現れる。#525 は `tako setup` が
/// 全滅し、#722 は AI 自動命名が一度も走らなかった（`SHELL` も `/bin/sh` も無いので
/// `CreateProcess` が失敗 → `.ok()?` で `None`。`claude_bin()` は `OnceLock` なので
/// プロセスが生きている限り復活しない）。**どちらもエラーは 1 つも出ていない**。
///
/// #898 の番犬（`which` / `where` の直起動）とは形が違うので別に見る。あちらは
/// 「Windows に無いコマンドを起こす」形、こちらは「Windows に無いシェルを起こす」形
#[test]
fn 実行ファイルの探索がログインシェル経由で境界の外に残っていない() {
    // 素の名前（`Command::new("gh")`）へ先にフォールバックする実装は Windows でも
    // 解決できるので許可する。**握り潰す形だけ**を落とす
    const ALLOWED: &[(&str, &str)] = &[
        (
            "crates/tako-core/src/platform/exe.rs",
            "境界 B16 の実装本体（unix 経路）",
        ),
        (
            "crates/tako-core/src/lib.rs",
            "resolve_bin(): 素の名前へフォールバックするので Windows でも解決できる\
             （PATHEXT とユーザー導入先を見ないぶん B16 より弱いだけ）",
        ),
        (
            "crates/tako-app/src/preview.rs",
            "resolve_bin() と同型のヘルパー。理由も同じ（素の名前へフォールバックする）",
        ),
        (
            "crates/tako-control/src/config_share/env.rs",
            "find_gh(): `gh --version`（素の名前）を先に試し、シェル経路は #[cfg(unix)] の中",
        ),
    ];

    let root = repo_root();
    let mut offenders = Vec::new();
    for crate_dir in ["tako-core", "tako-control", "tako-app", "tako-cli"] {
        collect_login_shell_lookups(
            &root.join("crates").join(crate_dir).join("src"),
            &root,
            ALLOWED,
            &mut offenders,
        );
    }
    assert!(
        offenders.is_empty(),
        "実行ファイルの探索がログインシェル経由で境界の外にある:\n  {}\n\
         → tako_core::platform::exe::find へ寄せてください（#525 / #722 / 設計 §2 の B16）",
        offenders.join("\n  ")
    );
}

/// 「ログインシェルに `command -v` で場所を尋ねる」行だけを拾う。
/// **2 条件の AND** にしてあるのは、ペインへ打ち込む文字列（セルフテストの
/// `type_text(.., "command -v tako", ..)`）や doc コメント中の言及を巻き込まないため
fn collect_login_shell_lookups(
    dir: &Path,
    root: &Path,
    allowed: &[(&str, &str)],
    out: &mut Vec<String>,
) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_login_shell_lookups(&path, root, allowed, out);
            continue;
        }
        if path.extension().is_none_or(|e| e != "rs") {
            continue;
        }
        // 許可リストも報告文も `/` 区切りで書く（Windows の strip_prefix は `\` を返す）
        let rel = path
            .strip_prefix(root)
            .unwrap_or(&path)
            .display()
            .to_string()
            .replace('\\', "/");
        if allowed.iter().any(|(p, _)| rel == *p) {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        for (idx, line) in text.lines().enumerate() {
            let code = line.trim_start();
            if code.starts_with("//") {
                continue;
            }
            if code.contains("\"-l\", \"-c\"") && code.contains("command -v") {
                out.push(format!("{rel}:{}", idx + 1));
            }
        }
    }
}

/// **#866 の番犬**: tmux の完全一致ターゲット（`=name`）の直書きが、ターゲット構文の境界
/// （`tako_core::tmux::exact_target` / `session_pane_target`）の外に残っていないこと。
///
/// `=` は tmux の「前方一致ではなく完全一致」指定で、#181 / #32 で意図的に入れたもの。
/// ところが tmux 互換を名乗る別実装はこれを解釈しないことがある。実測（psmux 3.3.7）:
/// `kill-session -t =keepa` は **5.1 秒ブロックしたうえで exit 1**（1 つも消えない）、
/// 素の `-t keepa` なら 181ms で対象だけが消える。**macOS では気づけない**種類の差なので
/// ソース走査で塞ぐ（新しい直書きが増えたらここで落ちる）
#[test]
fn tmuxの完全一致ターゲットの直書きが境界の外に残っていない() {
    // 境界そのものだけを許す（許可には必ず理由を書く）
    const ALLOWED: &[(&str, &str)] = &[(
        "crates/tako-core/src/tmux.rs",
        "ターゲット構文の境界（TmuxTargetSyntax / exact_target / session_pane_target）の実装本体",
    )];
    // ターゲット文字列を組み立てる形だけを対象にする（純関数へ渡す固定入力は対象外）
    const PATTERNS: &[&str] = &["format!(\"="];

    let root = repo_root();
    let mut offenders = Vec::new();
    for crate_dir in ["tako-core", "tako-control", "tako-app", "tako-cli"] {
        let base = root.join("crates").join(crate_dir);
        for sub in ["src", "tests"] {
            collect_os_shell_calls(&base.join(sub), &root, PATTERNS, ALLOWED, &mut offenders);
        }
    }
    assert!(
        offenders.is_empty(),
        "`=` 付き tmux ターゲットの直書きが境界の外にある:\n  {}\n\
         → tako_core::tmux::exact_target / session_pane_target を通してください（#866）",
        offenders.join("\n  ")
    );
}

/// **#898 の番犬**: コマンド解決の `which` / `where` 直起動が、実行ファイル探索の境界
/// （B16 = `tako_core::platform::exe::find`）の外に残っていないこと。
///
/// `which` は **Windows に存在しない**（実測: 「用語 'which' は…認識されません」）。
/// 旧実装は `which` をコマンドとして 4 箇所で起こしており、Windows では例外なく
/// `None` を返していた = tako.exe が PATH 上に居るのに tako 自身には「無い」ように見え、
/// MCP 自動登録・stale claude 検知（#498）・設定画面のエージェント検出が丸ごと死んでいた。
/// `where` へ替えるのも誤り（unix に無いので裏返るだけ）。**どちらも起こさず境界へ寄せる**。
///
/// 許可リストは**空**にしてある。境界の unix 実装が起こすのはログインシェルであって
/// `which` ではないので、例外を持つ必要がない
#[test]
fn コマンド解決のwhich直起動が境界の外に残っていない() {
    const ALLOWED: &[(&str, &str)] = &[];
    const PATTERNS: &[&str] = &[
        "Command::new(\"which\")",
        "Command::new(\"where\")",
        "Command::new(\"where.exe\")",
    ];

    let root = repo_root();
    let mut offenders = Vec::new();
    for crate_dir in ["tako-core", "tako-control", "tako-app", "tako-cli"] {
        let base = root.join("crates").join(crate_dir);
        for sub in ["src", "tests"] {
            collect_os_shell_calls(&base.join(sub), &root, PATTERNS, ALLOWED, &mut offenders);
        }
    }
    assert!(
        offenders.is_empty(),
        "`which` / `where` の直起動が境界の外にある:\n  {}\n\
         → tako_core::platform::exe::find を通してください（#898 / 設計 §2 の B16）",
        offenders.join("\n  ")
    );
}

/// **#628 の番犬**: コンソールウィンドウ抑止（`platform::process::no_console_window`）を
/// 通していない子プロセス起動が、いま把握している数より増えていないこと。
///
/// ## なぜ「数」で見るのか
///
/// GUI サブシステムの tako-app から console サブシステムの子を素で起動すると、
/// Windows が**子のためにコンソールウィンドウを新規作成する**（`Stdio::piped()` でも
/// 防げない）。ポーリング経路がこれをやると窓が明滅し続け、フォーカスまで奪われる。
///
/// 残っている未適用箇所はすべて **macOS / unix 限定・テスト専用・意図的に見せる起動**の
/// いずれかで、Windows の GUI からは到達しない（下表の理由を参照）。ファイル単位の
/// 許可リストにすると「そのファイルなら何個でも増やせる」穴になるので、**件数を固定**して
/// 新しい素の起動が 1 つでも増えたら落ちるようにしている。
///
/// 落ちたときの直し方:
/// - Windows の GUI から到達しうるなら `no_console_window` を通す（= 件数は増えない）
/// - 到達しないなら、この表の件数を理由つきで更新する（黙って数字だけ増やさない）
#[test]
fn コンソール窓を抑止していない子プロセス起動が増えていない() {
    // (パス, 未適用の件数, 理由)
    const BASELINE: &[(&str, usize, &str)] = &[
        (
            "crates/tako-app/src/main.rs",
            17,
            "セルフテスト（`self_test::run`）と visual-test feature 限定の検証コード、\
             および `#[cfg(unix)]` の単体テスト。製品の描画・入力経路に子プロセスは無い。\
             #865 で tmux の版数検出を `no_console_window` 経由へ寄せたので 22 → 21、\
             #866 でセルフテストの tmux 直起動 4 箇所（項目 48 / 68 / 73）を \
             `tako_core::tmux::tmux_command` 経由へ寄せたので 21 → 17",
        ),
        (
            "crates/tako-app/src/open_files.rs",
            1,
            "Launch Services ヘルパのモックテスト（macOS 限定。#837）",
        ),
        (
            "crates/tako-app/src/preview.rs",
            1,
            "ffmpeg / ffprobe をログインシェル経由で探す経路（`#[cfg(unix)]`）",
        ),
        (
            "crates/tako-app/src/update_checker.rs",
            1,
            "macOS 限定の `ditto`（zip 展開）1 箇所。Windows インストーラーの起動は \
             GUI アプリなので窓を見せるのが正。バンドルの差し替え側は #1042 で \
             `tako_core::platform::bundle_install` へ移り、そちらは境界を通している",
        ),
        (
            "crates/tako-cli/src/setup.rs",
            1,
            "対話起動 1 箇所（setup アシスタント本体 = 引き継ぎ先エージェントの起動と共用）。\
             対話子は端末を継承させる必要があるので塞がない。\
             #1057 で `brew install` の起動は `setup_deps`（境界経由）へ移した",
        ),
        (
            "crates/tako-control/src/agents.rs",
            1,
            "`ps`（`#[cfg(unix)]`）",
        ),
        (
            "crates/tako-control/src/config_share/env.rs",
            2,
            "ログインシェル経由の探索（`#[cfg(unix)]`）とテストモジュールの `git init`",
        ),
        (
            "crates/tako-control/src/dispatch.rs",
            6,
            "tmux e2e テスト内（`#[cfg(unix)]`）",
        ),
        (
            "crates/tako-control/src/platform/os_integration.rs",
            7,
            "境界 B8 の macOS / Linux 実装（open / osascript / xdg-open）+ テストの osascript 1 件。\
             Windows 側（explorer / cmd start）は #617 で適用済みなので数に入らない",
        ),
        (
            "crates/tako-control/src/remote.rs",
            6,
            "`/bin/sh` / `/bin/sleep`（`#[cfg(unix)]` とテスト）。ゾンビ判定の `/bin/ps` は              #1067 で境界（`platform::process::is_zombie`）へ移し、抑止を通している",
        ),
        (
            "crates/tako-control/src/remote_setup.rs",
            1,
            "`brew install` の対話実行。進捗をユーザーの端末に出す必要があるため素で起動する",
        ),
        (
            "crates/tako-control/src/sleep_guard.rs",
            6,
            "macOS 限定（pmset / osascript / defaults）",
        ),
        (
            "crates/tako-control/src/telemetry.rs",
            3,
            "`hostname`（`#[cfg(unix)]`）と macOS 限定の OS 情報取得（sw_vers / uname）",
        ),
        ("crates/tako-core/src/git.rs", 6, "テストモジュール内"),
        (
            "crates/tako-core/src/lib.rs",
            1,
            "ログインシェル経由のフォールバック（`#[cfg(unix)]`）",
        ),
        (
            "crates/tako-core/src/platform/exe.rs",
            1,
            "境界 B16 の unix 実装（ログインシェル経由）。Windows 実装は子プロセスを起こさない",
        ),
        (
            "crates/tako-core/src/platform/locale.rs",
            1,
            "`defaults`（macOS 限定）",
        ),
        (
            "crates/tako-core/src/platform/process.rs",
            1,
            "境界 B14 の実装本体。単体テストが windows / unix 用に Command を 2 個組み立て、\
             抑止は 1 回だけ通す（テストの構造上の 1 件で、製品コードの起動ではない）",
        ),
        (
            "crates/tako-core/src/platform/release_assets.rs",
            4,
            "テストモジュール内。シェル関数との一致を見る `sh` 2 件（#594 のアセット名 / \
             #965 の片肺判定）と、PowerShell 側の写しとの一致を見る `pwsh` / \
             インストーラー検査（#587）",
        ),
        (
            "crates/tako-core/src/tmux.rs",
            1,
            "tmux e2e テスト内（`#[cfg(unix)]`）",
        ),
        (
            "crates/tako-core/src/tmux_backend.rs",
            4,
            "tmux e2e テスト内（`#[cfg(unix)]`）",
        ),
    ];

    let root = repo_root();
    let mut actual: std::collections::BTreeMap<String, usize> = Default::default();
    for crate_dir in ["tako-core", "tako-control", "tako-app", "tako-cli"] {
        collect_unguarded_spawns(
            &root.join("crates").join(crate_dir).join("src"),
            &root,
            &mut actual,
        );
    }

    let expected: std::collections::BTreeMap<String, usize> = BASELINE
        .iter()
        .map(|(p, n, _)| ((*p).to_string(), *n))
        .collect();

    let mut diffs = Vec::new();
    for (path, count) in &actual {
        let want = expected.get(path).copied().unwrap_or(0);
        if *count > want {
            diffs.push(format!(
                "{path}: {want} 件のはずが {count} 件（増えている）"
            ));
        }
    }
    // 減った / 消えた分も知らせる（表を実態に合わせて縮められる）
    for (path, want) in &expected {
        let got = actual.get(path).copied().unwrap_or(0);
        if got < *want {
            diffs.push(format!(
                "{path}: {want} 件の想定だが {got} 件（減ったので表を更新してよい）"
            ));
        }
    }

    // 落ちたときに表をそのまま貼り替えられるよう、実測値を全部出す
    let actual_table = actual
        .iter()
        .map(|(p, n)| format!("        (\"{p}\", {n}, \"理由を書く\"),"))
        .collect::<Vec<_>>()
        .join("\n");

    assert!(
        diffs.is_empty(),
        "コンソール窓を抑止していない子プロセス起動の件数が想定と違う:\n  {}\n\
         → Windows の GUI から到達するなら \
         tako_core::platform::process::no_console_window を通してください（#628 / #586）\n\
         \n現在の実測値:\n{}",
        diffs.join("\n  "),
        actual_table
    );
}

/// **#936 の番犬**: 実行中プロセスの実行ファイルパスの解決が、プロセス検査の境界
/// （B5 = `tako_core::platform::procinfo::image_path`）の外に無いこと。
///
/// ## なぜ直呼びが危険か
///
/// 旧実装は `stale_binary` が `#[cfg(target_os = "macos")]` で `proc_pidpath`、
/// それ以外で `/proc/<pid>/exe` を読んでいた。**Windows には `/proc` が無い**ので
/// 常に `None` へ落ち、「いま動いている claude」を特定できず
/// **古い claude の警告バナーが一度も出なかった**（#936）。エラーは 1 つも出ない。
/// #722 / #525 と同じ「機能が黙って無効になる」形で、**macOS のゲートは全部緑**。
///
/// ## コメント行は見ない
///
/// 解説の中で API 名に触れるのは正しい（どのフラグで呼ぶか・なぜ 0 なのかは
/// 実測の記録なので消せない）。**実際に呼んでいる行だけ**を落とす
#[test]
fn 実行中プロセスのパス解決が境界の外に残っていない() {
    // 呼ぶ形（macOS の libproc / Windows の Win32 / Linux の procfs）
    const PATTERNS: &[&str] = &[
        "proc_pidpath",
        "QueryFullProcessImageNameW",
        "/proc/{pid}/exe",
    ];
    const ALLOWED: &[(&str, &str)] = &[(
        "crates/tako-core/src/platform/procinfo.rs",
        "境界 B5 の実装本体（3 プラットフォームぶん）",
    )];

    let root = repo_root();
    let mut offenders = Vec::new();
    for crate_dir in ["tako-core", "tako-control", "tako-app", "tako-cli"] {
        collect_code_lines(
            &root.join("crates").join(crate_dir).join("src"),
            &root,
            PATTERNS,
            ALLOWED,
            &mut offenders,
        );
    }
    assert!(
        offenders.is_empty(),
        "実行中プロセスのパス解決が境界の外にある:\n  {}\n\
         → tako_core::platform::procinfo::image_path へ寄せてください\
         （#936 / 設計 §2 の B5）",
        offenders.join("\n  ")
    );
}

/// `collect_os_shell_calls` と同じ走査だが、**コメント行を飛ばす**。
/// 解説の中で API 名に触れている行を違反として報告しないため
fn collect_code_lines(
    dir: &Path,
    root: &Path,
    patterns: &[&str],
    allowed: &[(&str, &str)],
    out: &mut Vec<String>,
) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_code_lines(&path, root, patterns, allowed, out);
            continue;
        }
        if path.extension().is_none_or(|e| e != "rs") {
            continue;
        }
        // 報告文と ALLOWED はどちらも `/` 区切りで書く（Windows の strip_prefix は `\`）
        let rel = path
            .strip_prefix(root)
            .unwrap_or(&path)
            .display()
            .to_string()
            .replace('\\', "/");
        if allowed.iter().any(|(p, _)| rel == *p) {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        for (idx, line) in text.lines().enumerate() {
            let trimmed = line.trim_start();
            if trimmed.starts_with("//") {
                continue;
            }
            if patterns.iter().any(|p| line.contains(p)) {
                out.push(format!("{rel}:{}", idx + 1));
            }
        }
    }
}

/// **#970 の番犬**: `canonicalize` の直呼びが、パス解決の境界
/// （B26 = `tako_core::platform::path`）の外で**増えていない**こと。
///
/// ## なぜ直呼びが危険か
///
/// `Path::canonicalize` は Windows で **verbatim 形式**（`\\?\C:\Users\…`）を返す。
/// これを保存したり子プロセスへ渡したりすると、シェル統合が `\` → `/` へ置換した
/// 段階で `//?/C:/…` になり、ペインの cwd が **`///?/C:/…`（実在しないパス）**へ壊れる。
/// `tako open-in dir` した Windows のタブでは git 操作が全滅していた（#970 の実測）。
/// **macOS では `canonicalize` の戻りに prefix が付かないので、テストも含めて全部緑になる**
/// 種類の差なのでソース走査で塞ぐ。
///
/// ## なぜ「件数」で見るのか
///
/// `canonicalize` には**正しい直呼び**が残る:
///
/// - パストラバーサルの防止（`remote_files` / `remote` のアップロード）は
///   「実体を見て配下か判定する」という `canonicalize` の意味そのものが要件で、
///   両辺を同じように解決するので prefix は打ち消し合う
/// - 比較キー専用の解決（両辺とも素の `canonicalize` を通すもの）
/// - テスト・セルフテスト・visual-test の中でリポジトリや一時ディレクトリを指すもの
///
/// ファイル単位の許可リストにすると「そのファイルなら何個でも増やせる」穴になるので、
/// **件数を固定**して新しい直呼びが 1 つでも増えたら落ちるようにしている
/// （#628 の `コンソール窓を抑止していない子プロセス起動が増えていない` と同じ形）。
///
/// 落ちたときの直し方:
/// - 解決結果を**保存する / 子プロセスへ渡す / 応答へ出す**なら
///   `tako_core::platform::path::canonicalize`（または `canonicalize_or_self`）を通す
///   （= 件数は増えない）
/// - 比較キー・トラバーサル判定・テストなら、この表の件数を**理由つきで**更新する
#[test]
fn canonicalizeの直呼びが境界の外に残っていない() {
    // (パス, 直呼びの件数, 理由)
    const BASELINE: &[(&str, usize, &str)] = &[
        (
            "crates/tako-app/src/filetree.rs",
            1,
            "性能計測テストがリポジトリルートを指す 1 箇所",
        ),
        (
            "crates/tako-app/src/main.rs",
            3,
            "セルフテスト内（git セクションの pinned フォルダ 1 / #772 の偽 claude の \
             置き場 1 / 一時ディレクトリ配下であることの確認 1）。製品側の入口 \
             （`add_tree_root` / `open_dir_in_new_tab`）は #970 で境界へ寄せた",
        ),
        (
            "crates/tako-app/src/open_files.rs",
            3,
            "番犬テストが `scripts/` 配下のシェルスクリプトを指す 3 箇所（#708 / #837）",
        ),
        (
            "crates/tako-app/src/preview_watch.rs",
            1,
            "OS 監視の結合テストが一時ファイルを指す 1 箇所",
        ),
        (
            "crates/tako-app/src/update_checker.rs",
            5,
            "配布系統の判別（`/Caskroom/` / `/Cellar/` を含むかの macOS 限定判定）と、\
             PATH 上の tako CLI 重複検知の比較。一覧へ積むのは解決前のパスなので \
             解決結果は外へ出ない",
        ),
        (
            "crates/tako-control/src/config_share/env.rs",
            2,
            "共有対象が外部 git 管理下かの比較（#513）。`git rev-parse` の戻りと \
             突き合わせるだけで保存しない",
        ),
        (
            "crates/tako-control/src/remote.rs",
            2,
            "アップロード先のトラバーサル防止（#287 の P2-4）。`canonicalize` の \
             「実体を見る」意味が要件そのもの",
        ),
        (
            "crates/tako-control/src/remote_files.rs",
            6,
            "リモートファイル操作の認可（ルート配下かをコンポーネント単位で判定 = \
             #1085 の脅威モデル）とプレビューの同一ファイル判定。両辺を同じように \
             解決するので prefix は打ち消し合う",
        ),
    ];

    let root = repo_root();
    let mut actual: std::collections::BTreeMap<String, usize> = Default::default();
    for crate_dir in ["tako-core", "tako-control", "tako-app", "tako-cli"] {
        collect_raw_canonicalize(
            &root.join("crates").join(crate_dir).join("src"),
            &root,
            &mut actual,
        );
    }

    let expected: std::collections::BTreeMap<String, usize> = BASELINE
        .iter()
        .map(|(p, n, _)| ((*p).to_string(), *n))
        .collect();

    let mut diffs = Vec::new();
    for (path, count) in &actual {
        let want = expected.get(path).copied().unwrap_or(0);
        if *count > want {
            diffs.push(format!(
                "{path}: {want} 件のはずが {count} 件（増えている）"
            ));
        }
    }
    for (path, want) in &expected {
        let got = actual.get(path).copied().unwrap_or(0);
        if got < *want {
            diffs.push(format!(
                "{path}: {want} 件の想定だが {got} 件（減ったので表を更新してよい）"
            ));
        }
    }

    let actual_table = actual
        .iter()
        .map(|(p, n)| format!("        (\"{p}\", {n}, \"理由を書く\"),"))
        .collect::<Vec<_>>()
        .join("\n");

    assert!(
        diffs.is_empty(),
        "`canonicalize` の直呼びの件数が想定と違う:\n  {}\n\
         → 解決結果を保存する / 子プロセスへ渡す / 応答へ出すなら \
         tako_core::platform::path::canonicalize を通してください（#970 / 設計 §2 の B26）\n\
         \n現在の実測値:\n{}",
        diffs.join("\n  "),
        actual_table
    );
}

/// ファイルごとの「境界を通らない `canonicalize`」の件数を数える。
///
/// コメント行と `watchdog-allow` を書いた行は除く（境界の実装本体と、旧挙動を
/// 再現する対照はそこに置く）
fn collect_raw_canonicalize(
    dir: &Path,
    root: &Path,
    out: &mut std::collections::BTreeMap<String, usize>,
) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_raw_canonicalize(&path, root, out);
            continue;
        }
        if path.extension().is_none_or(|e| e != "rs") {
            continue;
        }
        let rel = path
            .strip_prefix(root)
            .unwrap_or(&path)
            .display()
            .to_string()
            .replace('\\', "/");
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        for line in text.lines() {
            let code = line.trim_start();
            if code.starts_with("//") || line.contains("watchdog-allow") {
                continue;
            }
            if line.contains(".canonicalize()") || line.contains("fs::canonicalize(") {
                *out.entry(rel.clone()).or_insert(0) += 1;
            }
        }
    }
}

/// **#586 の番犬**: `tako-app` が release で GUI サブシステムへリンクされること。
///
/// Rust 既定の console サブシステムのままだと、コンソールを持たない親
/// （エクスプローラー / スタートメニュー）から起動されたときに OS が exe 用の
/// コンソールを新規作成し、GUI とは別に黒い窓が開いて診断ログが流れる。
/// サブシステムは**リンク時の属性**なので実行時には検査できない。属性がソースから
/// 消えたことだけは機械で分かるので、ここで固定する（`tako.exe` = CLI は console の
/// ままが正なので、対象は `tako-app` だけ）。
///
/// 併せて、GUI サブシステムでは stdout / stderr が捨てられるため、**起動を中断する
/// `fatal:` が persist.log にも残る**ことを見る（残さないと Windows release で無音死する）。
#[test]
fn tako_appはreleaseでguiサブシステムへリンクされる() {
    let main_rs = repo_root().join("crates/tako-app/src/main.rs");
    let src = std::fs::read_to_string(&main_rs).expect("tako-app の main.rs を読めない");

    assert!(
        src.contains(r#"#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]"#),
        "crates/tako-app/src/main.rs に windows_subsystem 属性が無い\n\
         → `#![cfg_attr(not(debug_assertions), windows_subsystem = \"windows\")]` を戻してください（#586）"
    );

    // 起動を中断する fatal は persist.log にも残す（GUI サブシステムでは stderr が届かない）
    let mut unlogged = Vec::new();
    for (i, line) in src.lines().enumerate() {
        if !line.contains(r#"eprintln!("fatal:"#) {
            continue;
        }
        // 直後 3 行以内に persist_diag があること
        let logged = src
            .lines()
            .skip(i + 1)
            .take(3)
            .any(|l| l.contains("persist_diag"));
        if !logged {
            unlogged.push(format!("main.rs:{}: {}", i + 1, line.trim()));
        }
    }
    assert!(
        unlogged.is_empty(),
        "起動中断の fatal が persist.log に残っていない:\n  {}\n\
         → GUI サブシステム（Windows release）では stderr が捨てられ無音死するので \
         persist_diag も呼んでください（#586）",
        unlogged.join("\n  ")
    );
}

/// `Command::new(` の出現のうち、対応する `no_console_window` が無いものをファイル単位で数える。
///
/// **1 個の抑止呼び出しは 1 個の起動しか守れない**ので、近傍にあるかを見るだけでは足りない
/// （素の起動を守られている起動の隣へ足すと、同じ抑止を二重に数えて見逃す）。
/// 抑止呼び出しを「消費」しながら順に対応づけ、相手の見つからなかった起動だけを数える。
fn collect_unguarded_spawns(
    dir: &Path,
    root: &Path,
    out: &mut std::collections::BTreeMap<String, usize>,
) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_unguarded_spawns(&path, root, out);
            continue;
        }
        if path.extension().is_none_or(|e| e != "rs") {
            continue;
        }
        let rel = path
            .strip_prefix(root)
            .unwrap_or(&path)
            .display()
            .to_string()
            .replace('\\', "/");
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        let lines: Vec<&str> = text.lines().collect();
        let is_comment = |l: &str| l.trim_start().starts_with("//");

        // 抑止呼び出しの行番号（コメント中の言及は数えない）
        let mut guards: Vec<(usize, bool)> = lines
            .iter()
            .enumerate()
            .filter(|(_, l)| l.contains("no_console_window") && !is_comment(l))
            .map(|(i, _)| (i, false))
            .collect();

        for (idx, line) in lines.iter().enumerate() {
            if !line.contains("Command::new(") || is_comment(line) {
                continue;
            }
            let from = idx.saturating_sub(3);
            let to = idx + 11;
            // まだ誰にも使われていない抑止呼び出しを 1 個だけ確保する
            let matched = guards
                .iter_mut()
                .find(|(g, used)| !*used && *g >= from && *g <= to);
            match matched {
                Some((_, used)) => *used = true,
                None => *out.entry(rel.clone()).or_insert(0) += 1,
            }
        }
    }
}
