//! 共有分類カタログの被覆テスト（Issue #513 要件 1・2）
//!
//! **狙い**: 新しい設定ファイルを足した人が共有分類を書き忘れたら、
//! レビューの目ではなく**テストが落ちて**気付くこと。
//! `platform::support::MATRIX` の T1 被覆（#515）と同じ考え方を、
//! 「MCP ツール」ではなく「データディレクトリへ書くファイル」に適用する。
//!
//! 分類を忘れたファイルは fail-closed で共有されないので**漏えいはしない**が、
//! 「共有したいのに共有されない」に気付けないままになる。だからテストで止める。

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use tako_control::config_share::catalog::{self, Class, Root};

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("リポジトリルートを解決できない")
        .to_path_buf()
}

/// `data_dir()` / `config_dir()` の直後に現れる最初の `join(…)` を集める。
/// 「tako がデータディレクトリ配下に作るファイル名」の実測値になる
fn referenced_paths() -> BTreeSet<(Root, String)> {
    const MARKERS: &[(&str, &str)] = &[
        // (コード中のマーカー, カタログ上の親ディレクトリ)
        ("data_dir()", ""),
        // orchestrator::config_dir() は <data_dir>/orchestrator を指す
        ("config_dir()", "orchestrator/"),
    ];
    let mut out = BTreeSet::new();
    for crate_dir in ["tako-core", "tako-control", "tako-app", "tako-cli"] {
        let src = repo_root().join("crates").join(crate_dir).join("src");
        for file in rust_files(&src) {
            let Ok(text) = std::fs::read_to_string(&file) else {
                continue;
            };
            for (marker, prefix) in MARKERS {
                let mut from = 0usize;
                while let Some(found) = text[from..].find(marker) {
                    let at = from + found + marker.len();
                    from = at;
                    // マーカー直後の限られた範囲だけを見る（無関係な join を拾わない）。
                    // 日本語コメントを含むので、必ず char 境界で切る
                    let mut end = text.len().min(at + 300);
                    while end > at && !text.is_char_boundary(end) {
                        end -= 1;
                    }
                    let window = &text[at..end];
                    if let Some(name) = first_join_arg(window) {
                        out.insert((Root::TakoData, format!("{prefix}{name}")));
                    }
                }
            }
        }
    }
    out
}

/// マーカー直後の**同じ式**に現れる最初の `join(…)` の引数を取り出す。
/// `join("name")` と `join(format!("name_{x}.md"))` の両方を拾い、
/// 後者は可変部分を `*` に畳む（カタログの前方一致エントリと突き合わせるため。#792）。
///
/// 「近くにあるだけの無関係な join」を拾うとテストが誤って落ちて信用されなくなるので、
/// 走査は**ステートメント / ブロック境界で打ち切る**（`;` / 行頭の `}` / 空行）。
/// 関数定義（`fn data_dir() -> …`）はそもそも呼び出しではないので除外する
fn first_join_arg(window: &str) -> Option<String> {
    if window.trim_start().starts_with("->") {
        return None;
    }
    let scope = cut_at_statement_end(window);
    let at = find_join_arg_start(scope)?;
    let rest = &scope[at..];
    let end = rest.find('"')?;
    let name = &rest[..end];
    if name.is_empty() {
        return None;
    }
    Some(collapse_placeholders(name))
}

/// 式の終わりで切る（`;` / 行頭の `}` / 空行のいずれか最初のところ）
fn cut_at_statement_end(window: &str) -> &str {
    let mut end = window.len();
    for pat in [";", "\n}", "\n\n"] {
        if let Some(i) = window.find(pat) {
            end = end.min(i);
        }
    }
    &window[..end]
}

/// `join("` / `join(format!("` の**文字列リテラルが始まる位置**（`"` の次）を返す
fn find_join_arg_start(scope: &str) -> Option<usize> {
    let mut from = 0usize;
    while let Some(found) = scope[from..].find("join(") {
        let mut i = from + found + "join(".len();
        i += leading_space(&scope[i..]);
        if scope[i..].starts_with("format!(") {
            let mut j = i + "format!(".len();
            j += leading_space(&scope[j..]);
            if scope[j..].starts_with('"') {
                return Some(j + 1);
            }
        } else if scope[i..].starts_with('"') {
            return Some(i + 1);
        }
        from = i;
    }
    None
}

/// 先頭の空白のバイト数
fn leading_space(s: &str) -> usize {
    s.len() - s.trim_start().len()
}

/// `format!` の可変部分（`{profile}`）を `*` に畳む。
/// カタログ側は `_system_prompt_*` のような前方一致エントリで受ける
fn collapse_placeholders(name: &str) -> String {
    let mut out = String::new();
    let mut in_placeholder = false;
    for c in name.chars() {
        match c {
            '{' => {
                in_placeholder = true;
                out.push('*');
            }
            '}' => in_placeholder = false,
            c if !in_placeholder => out.push(c),
            _ => {}
        }
    }
    out
}

fn rust_files(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return out;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            out.extend(rust_files(&path));
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
    out
}

/// そのパスがカタログで扱われているか。
/// 中間ディレクトリ（`orchestrator` のように配下にエントリを持つもの）も被覆とみなす
fn covered(root: Root, rel: &str) -> bool {
    if catalog::classify(root, rel).is_some() {
        return true;
    }
    let as_dir = format!("{rel}/");
    catalog::CATALOG
        .iter()
        .any(|e| e.root == root && e.path.starts_with(&as_dir))
}

/// **被覆**: tako がデータディレクトリ配下へ作るファイルはすべて分類済みであること
#[test]
fn データディレクトリへ書くパスがすべて分類されている() {
    let missing: Vec<String> = referenced_paths()
        .into_iter()
        .filter(|(root, rel)| !covered(*root, rel))
        .map(|(root, rel)| format!("{}/{rel}", root.as_str()))
        .collect();
    assert!(
        missing.is_empty(),
        "共有分類が無いパスがある: {missing:?}\n\
         → crates/tako-control/src/config_share/catalog.rs の CATALOG に追加し、\n\
           shared / local / secret のどれかと理由（日英）を宣言してください"
    );
}

/// 走査そのものが機能していることの確認。
/// スキャナが壊れて 0 件になると、被覆テストは「常に緑」の張りぼてになる
#[test]
fn 走査が実際にパスを拾えている() {
    let found = referenced_paths();
    assert!(
        found.len() >= 15,
        "データディレクトリ参照の走査結果が少なすぎる（{}件）。\
         スキャナが壊れていないか確認してください",
        found.len()
    );
    for expected in [
        "layout.json",
        "token",
        "orchestrator/projects.yaml",
        // #792: 名前が実行時に決まるファイル（`join(format!(…))`）も網に掛かる
        "orchestrator/_system_prompt_*.md",
    ] {
        assert!(
            found.contains(&(Root::TakoData, expected.to_string())),
            "既知のパス {expected} を走査で拾えていない"
        );
    }
}

/// **#792**: 走査が拾えない・拾いにくい「名前が実行時に決まるファイル」を名指しで固定する。
/// 走査の実装が変わっても、この families が未分類になったら落ちる
#[test]
fn 動的に名前が決まるファイルも分類されている() {
    const DYNAMIC: &[(&str, &str, Class)] = &[
        // master / solo 起動ごとに書き出す system prompt の実体（生成物）
        (
            "tako",
            "orchestrator/_system_prompt_default.md",
            Class::Local,
        ),
        (
            "tako",
            "orchestrator/_system_prompt_takodev.md",
            Class::Local,
        ),
        // プロファイルごとの引き継ぎファイル（このマシンの実行状態を含む）
        ("tako", "orchestrator/handoff/default.md", Class::Local),
        // ペインごとの平文ログ
        ("tako", "pane-logs/pane-42.log", Class::Local),
        // プロファイル定義（宣言的設定なので共有する）
        ("tako", "orchestrator/profiles/takodev.yaml", Class::Shared),
        (
            "tako",
            "orchestrator/solo-profiles/docs.yaml",
            Class::Shared,
        ),
    ];
    for (root_name, rel, want) in DYNAMIC {
        let root = Root::parse(root_name).expect("ルート名");
        let entry = catalog::classify(root, rel)
            .unwrap_or_else(|| panic!("{root_name}/{rel} が未分類（動的名でも分類は必要）"));
        assert_eq!(
            entry.class,
            *want,
            "{root_name}/{rel} の分類が想定と違う（{} != {}）",
            entry.class.as_str(),
            want.as_str()
        );
    }
}

/// **要件 1**: 秘匿情報とマシンローカル状態が共有対象に入っていないこと。
/// 名前で名指しして固定する（分類を書き換えたら落ちる）
#[test]
fn 秘匿とランタイム状態が共有対象に入っていない() {
    const NEVER_SHARED: &[(&str, &str)] = &[
        ("tako", "token"),
        ("tako", "control.json"),
        ("tako", "relay_secret"),
        ("tako", "machine_id"),
        ("tako", "remote/state.json"),
        ("tako", "layout.json"),
        ("tako", "sessions.yaml"),
        ("tako", "workers.yaml"),
        ("tako", "recent.json"),
        ("tako", "telemetry.log"),
        ("tako", "telemetry_queue.jsonl"),
        ("tako", "pane-logs/pane-1.log"),
        ("tako", "orchestrator/handoff/master.md"),
        ("tako", "orchestrator/ledger.yaml"),
        ("claude", ".claude.json"),
        ("claude", ".credentials.json"),
        ("claude", "credentials.json"),
        ("claude", "history.jsonl"),
        ("claude", "projects/x/session.jsonl"),
        ("claude", "sessions/x.json"),
        ("claude", "settings.json"),
    ];
    for (root_name, rel) in NEVER_SHARED {
        let root = Root::parse(root_name).expect("ルート名");
        let entry = catalog::classify(root, rel).unwrap_or_else(|| {
            panic!("{root_name}/{rel} が未分類（fail-closed だが明示すること）")
        });
        assert!(
            !entry.class.is_shared(),
            "{root_name}/{rel} が共有対象になっている（クラス {}）",
            entry.class.as_str()
        );
    }
}

/// **要件 1**: 資格情報の在り処と秘匿になりうる env は、共有コピーから外れる宣言があること
#[test]
fn 資格情報パスとenvがローカルフィールドとして宣言されている() {
    let accounts = catalog::classify(Root::TakoData, "orchestrator/accounts.yaml").unwrap();
    assert!(
        accounts.local_fields.contains(&"accounts.*.config_dir"),
        "accounts.yaml の config_dir が共有から外れていない"
    );
    for dir in ["orchestrator/profiles/", "orchestrator/solo-profiles/"] {
        let entry = catalog::classify(Root::TakoData, &format!("{dir}x.yaml")).unwrap();
        assert!(
            entry.local_fields.contains(&"env"),
            "{dir} の env が共有から外れていない"
        );
    }
}

/// 共有対象は「宣言的な設定」だけであること。
/// ログ・キャッシュ・履歴の匂いがする名前が Shared に混ざっていたら落とす
#[test]
fn 共有対象にログや履歴の匂いがする名前が無い() {
    const SMELLS: &[&str] = &[
        "log",
        "cache",
        "history",
        "session",
        "token",
        "secret",
        "credential",
        "layout",
        "worker",
        "instance",
    ];
    for entry in catalog::CATALOG.iter().filter(|e| e.class == Class::Shared) {
        let lower = entry.path.to_ascii_lowercase();
        for smell in SMELLS {
            assert!(
                !lower.contains(smell),
                "共有対象 {} に危険な名前 '{smell}' が含まれる。分類を見直してください",
                entry.path
            );
        }
    }
}
