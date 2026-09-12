//! #1425 の番犬 — `save_layout` の変化検出キーが永続フィールドを取りこぼさない
//!
//! **狙い**: 「変化していないから書かない」の判定材料（[`layout::change_key`]）に
//! 載せ忘れたフィールドは**保存されなくなる**。レビューの目ではなくテストで落とす。
//! 「消えた」系（#30 / #177 / #770）の根はすべてこの保存漏れなので、
//! 速さ（#1001 C6）より先に正しさを縛る。
//!
//! ## 4 本立て
//!
//! 1. [`宣言が永続スキーマの指紋と一致する`] — `layout.json` に載る全フィールドが
//!    `CHANGE_KEY_FIELDS` に宣言されている。永続フィールドを増減・改名した PR は
//!    #916 の指紋（`persisted_schema_fingerprint.txt`）が先に動き、ここが落ちる
//! 2. [`宣言ごとに動かして確かめる検査がある`] — 宣言 1 件ごとに下の検査表
//!    （`CASES`）へ 1 件。宣言だけ足してキーに流し忘れた、を許さない
//! 3. [`フィールドを動かすとキーも保存内容も動く`] — 検査表を実際に回す。
//!    **キーが動く ⟺ JSON が動く**を 1 フィールドずつ確かめる（本体の検出力）
//! 4. [`変化検出はcaptureと直列化の前にある`] / [`PaneMetaRefはPaneMetaと1対1`] —
//!    直し方そのもの（#1001 C6）が戻っていないことと、借用版・保存形のズレを見る
//!
//! ## 検査表の作り方
//!
//! 基準は **`LayoutFile` そのもの**（全フィールドが非既定値）。`restore` で
//! Workspace へ戻し、ペイン付帯情報・UI 付帯情報も同じファイルから組む。こうすると
//! 「保存 → 復元 → 保存」が原文と**バイト一致**するので、ファイルの 1 フィールドを
//! 動かした差分がそのまま「保存内容の差分」になる（= 検査表の 1 行が 1 フィールドを
//! 正確に射る）。ID のような実行時採番のフィールドまで動かせるのもこの形だから。

use std::borrow::Cow;
use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};

use tako_control::layout::{
    self, AgentResumeLayout, ChangeKey, LayoutExtras, LayoutFile, NodeLayout, PaneLayout,
    PaneMetaRef, PreviewLayout, RemoteFolderLayout, TabLayout, WindowFrame, WindowLayout,
    CHANGE_KEY_FIELDS,
};
use tako_core::PaneId;

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("リポジトリルートを解決できない")
        .to_path_buf()
}

/// `layout.json` へ直に serde される構造体（#916 の指紋の TARGETS と同じ並び）
const LAYOUT_STRUCTS: &[&str] = &[
    "LayoutFile",
    "WindowLayout",
    "WindowFrame",
    "TabLayout",
    "NodeLayout",
    "PaneLayout",
    "AgentResumeLayout",
    "PreviewLayout",
    "RemoteFolderLayout",
];

/// #916 の指紋スナップショットから layout 側の `(構造体, フィールド)` を読む。
/// 指紋はソースから生成されて CI で一致を強制されているので、ここが実体の正
fn persisted_layout_fields() -> BTreeSet<(String, String)> {
    let path = repo_root().join("crates/tako-control/testdata/persisted_schema_fingerprint.txt");
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("指紋を読めない {}: {e}", path.display()));
    let mut out = BTreeSet::new();
    let mut seen_structs = BTreeSet::new();
    for line in text.lines() {
        let Some((name, fields)) = line.split_once(':') else {
            continue;
        };
        let name = name.trim();
        if !LAYOUT_STRUCTS.contains(&name) {
            continue;
        }
        seen_structs.insert(name.to_string());
        for f in fields.split(',') {
            let f = f.trim();
            if !f.is_empty() {
                out.insert((name.to_string(), f.to_string()));
            }
        }
    }
    // 検出力: 構造体を 1 つでも見失っていたら「何も見ていない番犬」になる
    for name in LAYOUT_STRUCTS {
        assert!(
            seen_structs.contains(*name),
            "{name} が指紋 {} に無い（改名したなら LAYOUT_STRUCTS も直す。\
             見失うとこの番犬は永続フィールドを 1 つも検査しない）",
            path.display()
        );
    }
    out
}

fn declared() -> BTreeSet<(String, String)> {
    CHANGE_KEY_FIELDS
        .iter()
        .map(|(s, f)| ((*s).to_string(), (*f).to_string()))
        .collect()
}

/// 1: `layout.json` に載る全フィールドが変化検出キーの宣言に載っている
#[test]
fn 宣言が永続スキーマの指紋と一致する() {
    let persisted = persisted_layout_fields();
    let declared = declared();
    let missing: Vec<_> = persisted.difference(&declared).collect();
    let stale: Vec<_> = declared.difference(&persisted).collect();
    assert!(
        missing.is_empty(),
        "layout.json に載るのに変化検出キーの宣言に無い: {missing:?}\n\
         crates/tako-control/src/layout.rs の CHANGE_KEY_FIELDS へ足し、\
         change_key に実際に流す 1 行と、この番犬の CASES を同時に足すこと（#1425）"
    );
    assert!(
        stale.is_empty(),
        "宣言にあるが layout.json には無い（消えたフィールドの取り残し）: {stale:?}\n\
         crates/tako-control/src/layout.rs の CHANGE_KEY_FIELDS から外すこと（#1425）"
    );
    assert_eq!(
        CHANGE_KEY_FIELDS.len(),
        declared.len(),
        "CHANGE_KEY_FIELDS に重複がある"
    );
}

// ---------------------------------------------------------------------------
// 検査表
// ---------------------------------------------------------------------------

/// フィールドの被覆のしかた
enum Cover {
    /// 実際に書き換えて「キーも保存内容も動く」ことを確かめる
    Mutate(fn(&mut LayoutFile)),
    /// 実行時には動かない値（理由を明記する）
    Constant(&'static str),
}

/// 宣言 1 件につき 1 行。**新しい永続フィールドはここにも追加する**
#[allow(clippy::type_complexity)]
fn cases() -> Vec<((&'static str, &'static str), Cover)> {
    vec![
        // ---- LayoutFile ----
        (
            ("LayoutFile", "version"),
            Cover::Constant(
                "LAYOUT_VERSION は定数（restore がバージョン不一致を拒否するので動かせない）。\
                 change_key は先頭でこれを流しているので、上げれば全キーが動く",
            ),
        ),
        (
            ("LayoutFile", "active_tab"),
            Cover::Mutate(|f| {
                f.active_tab = f.tabs[1].id;
            }),
        ),
        (
            ("LayoutFile", "tabs"),
            Cover::Mutate(|f| {
                let mut extra = f.tabs[1].clone();
                extra.id = 9001;
                extra.tree = NodeLayout::Pane(Box::new(pane(9101, "extra")));
                extra.focused = 9101;
                f.tabs.push(extra);
            }),
        ),
        (("LayoutFile", "window"), Cover::Mutate(|f| f.window = None)),
        (
            ("LayoutFile", "backgrounded"),
            Cover::Mutate(|f| {
                let mut extra = f.backgrounded[0].clone();
                extra.id = 9201;
                f.backgrounded.push(extra);
            }),
        ),
        (
            ("LayoutFile", "collapsed"),
            Cover::Mutate(|f| f.collapsed.push(9301)),
        ),
        (
            ("LayoutFile", "webview_dock"),
            Cover::Mutate(|f| f.webview_dock.push("https://example.invalid/dock2".into())),
        ),
        (
            ("LayoutFile", "windows"),
            Cover::Mutate(|f| f.windows.clear()),
        ),
        // ---- WindowLayout ----
        (
            ("WindowLayout", "id"),
            Cover::Mutate(|f| f.windows[1].id = 77),
        ),
        (
            ("WindowLayout", "tabs"),
            Cover::Mutate(|f| {
                // 3 枚目のタブを 2 番目のウィンドウから 1 番目へ移す（**集合**を変える。
                // window_tab_ids は全体のタブ順で返すので、並べ替えは往復で消える）
                let moved = f.windows[1].tabs.remove(1);
                f.windows[0].tabs.push(moved);
            }),
        ),
        (
            ("WindowLayout", "active_tab"),
            Cover::Mutate(|f| {
                f.windows[1].active_tab = f.windows[1].tabs[1];
            }),
        ),
        (
            ("WindowLayout", "frame"),
            Cover::Mutate(|f| f.windows[1].frame = None),
        ),
        // ---- WindowFrame ----
        (("WindowFrame", "x"), Cover::Mutate(|f| frame(f).x += 3.0)),
        (("WindowFrame", "y"), Cover::Mutate(|f| frame(f).y += 3.0)),
        (
            ("WindowFrame", "width"),
            Cover::Mutate(|f| frame(f).width += 3.0),
        ),
        (
            ("WindowFrame", "height"),
            Cover::Mutate(|f| frame(f).height += 3.0),
        ),
        (
            ("WindowFrame", "state"),
            Cover::Mutate(|f| frame(f).state = "fullscreen".into()),
        ),
        // ---- TabLayout ----
        (
            ("TabLayout", "id"),
            Cover::Mutate(|f| {
                // 参照（active_tab / windows[].tabs / 退避の由来タブ）ごと付け替える
                let old = f.tabs[1].id;
                let new = 8001;
                f.tabs[1].id = new;
                if f.active_tab == old {
                    f.active_tab = new;
                }
                for w in &mut f.windows {
                    for t in &mut w.tabs {
                        if *t == old {
                            *t = new;
                        }
                    }
                    if w.active_tab == old {
                        w.active_tab = new;
                    }
                }
                for b in &mut f.backgrounded {
                    if b.origin_tab == Some(old) {
                        b.origin_tab = Some(new);
                    }
                }
            }),
        ),
        (
            ("TabLayout", "title"),
            Cover::Mutate(|f| f.tabs[0].title = "別の題".into()),
        ),
        (
            ("TabLayout", "title_source"),
            Cover::Mutate(|f| f.tabs[0].title_source = "auto".into()),
        ),
        (
            ("TabLayout", "focused"),
            Cover::Mutate(|f| f.tabs[0].focused = 102),
        ),
        (
            ("TabLayout", "tree"),
            Cover::Mutate(|f| {
                // 葉を分割へ差し替える（構造そのものの変化）
                f.tabs[1].tree = NodeLayout::Split {
                    axis: "y".into(),
                    ratio: 0.4,
                    first: Box::new(f.tabs[1].tree.clone()),
                    second: Box::new(NodeLayout::Pane(Box::new(pane(8101, "生えた")))),
                };
            }),
        ),
        (
            ("TabLayout", "pinned_folders"),
            Cover::Mutate(|f| {
                let more = fixture_dir("pin-c");
                f.tabs[0].pinned_folders.push(more);
            }),
        ),
        (
            ("TabLayout", "remote_folders"),
            Cover::Mutate(|f| {
                f.tabs[0].remote_folders.push(RemoteFolderLayout {
                    host: "host-c".into(),
                    path: "/srv/c".into(),
                    origin: "auto".into(),
                });
            }),
        ),
        // ---- NodeLayout ----
        (
            ("NodeLayout", "Pane"),
            Cover::Mutate(|f| {
                // 葉 → 分割（このノードのバリアントが変わる）
                f.tabs[1].tree = NodeLayout::Split {
                    axis: "x".into(),
                    ratio: 0.5,
                    first: Box::new(f.tabs[1].tree.clone()),
                    second: Box::new(NodeLayout::Pane(Box::new(pane(8201, "追加")))),
                };
            }),
        ),
        (
            ("NodeLayout", "Split"),
            Cover::Mutate(|f| {
                // 分割 → 葉（畳む。上と逆向き）
                f.tabs[0].tree = NodeLayout::Pane(Box::new(pane(101, "左")));
                f.tabs[0].focused = 101;
            }),
        ),
        (
            ("NodeLayout", "axis"),
            Cover::Mutate(|f| set_axis(&mut f.tabs[0].tree, "y")),
        ),
        (
            ("NodeLayout", "ratio"),
            Cover::Mutate(|f| set_ratio(&mut f.tabs[0].tree, 0.25)),
        ),
        (
            ("NodeLayout", "first"),
            Cover::Mutate(|f| swap_children(&mut f.tabs[0].tree)),
        ),
        (
            ("NodeLayout", "second"),
            Cover::Mutate(|f| swap_children(&mut f.tabs[0].tree)),
        ),
        // ---- PaneLayout ----
        (
            ("PaneLayout", "id"),
            Cover::Mutate(|f| leaf(f, 102).id = 8301),
        ),
        (
            ("PaneLayout", "session"),
            Cover::Mutate(|f| leaf(f, 101).session = Some("tako-other".into())),
        ),
        (
            ("PaneLayout", "title"),
            Cover::Mutate(|f| leaf(f, 101).title = Some("別のペイン名".into())),
        ),
        (
            ("PaneLayout", "title_source"),
            Cover::Mutate(|f| leaf(f, 101).title_source = "manual".into()),
        ),
        (
            ("PaneLayout", "role"),
            Cover::Mutate(|f| leaf(f, 101).role = Some("worker".into())),
        ),
        (
            ("PaneLayout", "origin"),
            Cover::Mutate(|f| leaf(f, 101).origin = "mcp".into()),
        ),
        (
            ("PaneLayout", "cwd"),
            Cover::Mutate(|f| leaf(f, 101).cwd = Some("/srv/other".into())),
        ),
        (
            ("PaneLayout", "claude_session_id"),
            Cover::Mutate(|f| {
                leaf(f, 101).claude_session_id = Some("11111111-2222-3333-4444-555555555555".into())
            }),
        ),
        (
            ("PaneLayout", "agent_resume"),
            Cover::Mutate(|f| leaf(f, 102).agent_resume = None),
        ),
        (
            ("PaneLayout", "logged_history"),
            Cover::Mutate(|f| leaf(f, 101).logged_history = Some(4242)),
        ),
        (
            ("PaneLayout", "preview"),
            Cover::Mutate(|f| leaf(f, 102).preview = None),
        ),
        (
            ("PaneLayout", "webview"),
            Cover::Mutate(|f| leaf(f, 101).webview = Some("https://example.invalid/other".into())),
        ),
        (
            ("PaneLayout", "origin_tab"),
            Cover::Mutate(|f| f.backgrounded[0].origin_tab = Some(f.tabs[1].id)),
        ),
        (
            ("PaneLayout", "origin_tab_title"),
            Cover::Mutate(|f| f.backgrounded[0].origin_tab_title = Some("別の由来".into())),
        ),
        (
            ("PaneLayout", "limit_autoresume"),
            Cover::Mutate(|f| leaf(f, 101).limit_autoresume = true),
        ),
        // ---- AgentResumeLayout ----
        (
            ("AgentResumeLayout", "agent"),
            Cover::Mutate(|f| leaf(f, 102).agent_resume.as_mut().unwrap().agent = "agy".into()),
        ),
        (
            ("AgentResumeLayout", "id"),
            Cover::Mutate(|f| {
                leaf(f, 102).agent_resume.as_mut().unwrap().id = Some("conv-other".into())
            }),
        ),
        // ---- PreviewLayout ----
        (
            ("PreviewLayout", "path"),
            Cover::Mutate(|f| leaf(f, 102).preview.as_mut().unwrap().path = "/srv/other.md".into()),
        ),
        (
            ("PreviewLayout", "mode"),
            Cover::Mutate(|f| leaf(f, 102).preview.as_mut().unwrap().mode = "code".into()),
        ),
        // ---- RemoteFolderLayout ----
        (
            ("RemoteFolderLayout", "host"),
            Cover::Mutate(|f| f.tabs[0].remote_folders[0].host = "host-z".into()),
        ),
        (
            ("RemoteFolderLayout", "path"),
            Cover::Mutate(|f| f.tabs[0].remote_folders[0].path = "/srv/z".into()),
        ),
        (
            ("RemoteFolderLayout", "origin"),
            Cover::Mutate(|f| {
                // 既定（auto）は JSON へ出さない = 出力の有無ごと変わる
                f.tabs[0].remote_folders[0].origin = "auto".into()
            }),
        ),
    ]
}

/// 2: 宣言 1 件ごとに検査表の 1 行がある
#[test]
fn 宣言ごとに動かして確かめる検査がある() {
    let cases = cases();
    let covered: BTreeSet<(String, String)> = cases
        .iter()
        .map(|((s, f), _)| ((*s).to_string(), (*f).to_string()))
        .collect();
    assert_eq!(
        covered.len(),
        cases.len(),
        "検査表に同じフィールドが 2 回ある"
    );
    let declared = declared();
    let missing: Vec<_> = declared.difference(&covered).collect();
    assert!(
        missing.is_empty(),
        "宣言だけあって「動かしたらキーが変わる」検査が無い: {missing:?}\n\
         この番犬の cases() へ 1 行足すこと（宣言だけ足してキーへ流し忘れた、を許さない）"
    );
    let stale: Vec<_> = covered.difference(&declared).collect();
    assert!(stale.is_empty(), "検査表にあるが宣言に無い: {stale:?}");
}

/// 3: **本体の検出力**。1 フィールドずつ動かして「キーが動く ⟺ 保存内容が動く」を見る
#[test]
fn フィールドを動かすとキーも保存内容も動く() {
    let base = base_file();

    // 往復の忠実さ: 復元 → capture が原文とバイト一致する（= 差分がそのまま
    // 「保存内容の差分」になる）。ここが崩れると以降の 1 行 1 フィールドが成立しない
    assert_eq!(
        json_of(&base),
        serde_json::to_string(&base).unwrap(),
        "基準シーンの往復が原文と一致しない（検査表の前提が崩れている）"
    );
    // 同じ入力なら同じキー（安定性）
    assert_eq!(key_of(&base), key_of(&base), "同じ入力でキーが揺れる");

    let base_key = key_of(&base);
    let base_json = json_of(&base);
    let mut constants = 0usize;
    for ((s, field), cover) in cases() {
        let Cover::Mutate(mutate) = cover else {
            let Cover::Constant(reason) = cover else {
                unreachable!()
            };
            println!("{s}.{field}: 定数として扱う（{reason}）");
            constants += 1;
            continue;
        };
        let mut moved = base.clone();
        mutate(&mut moved);
        let moved_json = json_of(&moved);
        assert!(
            moved_json != base_json,
            "{s}.{field}: 検査表の書き換えが保存内容を動かしていない（検査になっていない）"
        );
        assert_ne!(
            key_of(&moved),
            base_key,
            "{s}.{field}: 保存内容は変わるのに変化検出キーが動かない = \
             このフィールドは保存されなくなる（layout.rs の change_key へ流すこと。#1425）"
        );
    }
    assert_eq!(
        constants, 1,
        "Constant 扱いの件数が変わった（理由を見直す）"
    );
}

/// 3b: 何も動かさなければキーは同じ（スキップが効く side）。
/// 「常に違うキーを返す」実装は検査表を全部通してしまうので、逆向きも縛る
#[test]
fn 何も変えなければキーは同じ() {
    let base = base_file();
    let same = base.clone();
    assert_eq!(
        key_of(&base),
        key_of(&same),
        "同じ内容でキーが変わる = 変化が無い tick でも毎回 capture を払う"
    );
}

/// 4: 直し方そのもの（#1001 C6）。キーの算出が capture / 直列化より**前**にある
#[test]
fn 変化検出はcaptureと直列化の前にある() {
    let src = std::fs::read_to_string(repo_root().join("crates/tako-app/src/main.rs"))
        .expect("main.rs を読めない");
    let body = save_layout_body(&src);
    let key_at = body
        .find("layout::change_key(")
        .expect("save_layout が change_key を呼んでいない（#1425 が戻っている）");
    let capture_at = body
        .find("layout::capture(")
        .expect("save_layout が capture を呼んでいない");
    let json_at = body
        .find("serde_json::to_string(&layout)")
        .expect("save_layout が直列化していない");
    assert!(
        key_at < capture_at && key_at < json_at,
        "変化検出キーの算出が capture / 直列化より後ろにある = \
         変化が無い tick でも両方を払っている（#1001 C6 が戻っている）"
    );
    // スキップの早期 return と、載せ忘れの保険が残っている
    assert!(
        body.contains("self.save_layout_stats.skipped += 1")
            && body[key_at..capture_at].contains("return;"),
        "キー一致で capture を省く早期 return が無い"
    );
    assert!(
        body.contains("layout::RECONCILE_AFTER_SKIPS"),
        "連続スキップの上限（載せ忘れの保険）が外れている。#1425 の不変条件"
    );
}

/// 4b: 借用版と保存形は 1:1（片方にだけフィールドが増えると保存漏れになる）
#[test]
fn パネメタの借用版と保存形は1対1() {
    let src = std::fs::read_to_string(repo_root().join("crates/tako-control/src/layout.rs"))
        .expect("layout.rs を読めない");
    let owned = struct_field_names(&src, "PaneMeta");
    let borrowed = struct_field_names(&src, "PaneMetaRef");
    assert!(!owned.is_empty(), "PaneMeta のフィールドを読めていない");
    assert_eq!(
        owned, borrowed,
        "PaneMeta と PaneMetaRef のフィールドがズレている。\
         借用版に無い項目は変化検出キーへ流れない（#1425）"
    );
}

// ---------------------------------------------------------------------------
// 基準シーンと道具
// ---------------------------------------------------------------------------

/// `pinned_folders` は**実在ディレクトリ**でないと復元で落ちる（`restore` が
/// `is_dir()` で濾す）。作って消す形にすると、並列に走る別のテストの後片付けが
/// 走った瞬間だけ復元結果が変わる（= キーが揺れる）ので、リポジトリ内の
/// 消えないディレクトリを借りる。復元は正規パスへ寄せるので基準側も正規化する
fn fixture_dir(name: &str) -> String {
    let path = match name {
        "pin-a" => repo_root(),
        "pin-b" => repo_root().join("crates"),
        "pin-c" => repo_root().join(".agent"),
        other => panic!("未知の fixture: {other}"),
    };
    assert!(path.is_dir(), "fixture が実在しない: {}", path.display());
    tako_core::platform::path::canonicalize_or_self(&path)
        .display()
        .to_string()
}

fn pane(id: u64, title: &str) -> PaneLayout {
    PaneLayout {
        id,
        session: Some(format!("tako-{id}")),
        title: Some(title.to_string()),
        title_source: "auto".into(),
        role: Some("main".into()),
        origin: "cli".into(),
        cwd: Some("/srv/work".into()),
        claude_session_id: Some("aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee".into()),
        agent_resume: Some(AgentResumeLayout {
            agent: "codex".into(),
            id: Some("conv-1".into()),
        }),
        logged_history: Some(12),
        preview: Some(PreviewLayout {
            path: "/srv/work/README.md".into(),
            mode: "markdown".into(),
        }),
        webview: Some("https://example.invalid/pane".into()),
        origin_tab: None,
        origin_tab_title: None,
        limit_autoresume: false,
    }
}

/// 全フィールドが非既定値の基準ファイル（2 ウィンドウ / 3 タブ / 分割 / 退避つき）
fn base_file() -> LayoutFile {
    let mut left = pane(101, "左");
    left.limit_autoresume = false;
    let right = pane(102, "右");
    let mut bg = pane(103, "退避");
    bg.origin_tab = Some(1);
    bg.origin_tab_title = Some("一枚目".into());

    LayoutFile {
        version: 1,
        active_tab: 1,
        window: Some(WindowFrame {
            x: 10.0,
            y: 20.0,
            width: 1200.0,
            height: 800.0,
            state: "windowed".into(),
        }),
        tabs: vec![
            TabLayout {
                id: 1,
                title: "一枚目".into(),
                title_source: "manual".into(),
                focused: 101,
                tree: NodeLayout::Split {
                    axis: "x".into(),
                    ratio: 0.5,
                    first: Box::new(NodeLayout::Pane(Box::new(left))),
                    second: Box::new(NodeLayout::Pane(Box::new(right))),
                },
                pinned_folders: vec![fixture_dir("pin-a"), fixture_dir("pin-b")],
                remote_folders: vec![RemoteFolderLayout {
                    host: "host-a".into(),
                    path: "/srv/a".into(),
                    origin: "explicit".into(),
                }],
            },
            TabLayout {
                id: 2,
                title: "二枚目".into(),
                title_source: "auto".into(),
                focused: 201,
                tree: NodeLayout::Pane(Box::new(pane(201, "単独"))),
                pinned_folders: Vec::new(),
                remote_folders: Vec::new(),
            },
            TabLayout {
                id: 3,
                title: "三枚目".into(),
                title_source: "default".into(),
                focused: 301,
                tree: NodeLayout::Pane(Box::new(pane(301, "三つ目"))),
                pinned_folders: Vec::new(),
                remote_folders: Vec::new(),
            },
        ],
        backgrounded: vec![bg],
        collapsed: vec![2],
        webview_dock: vec!["https://example.invalid/dock".into()],
        windows: vec![
            WindowLayout {
                id: 1,
                tabs: vec![1],
                active_tab: 1,
                frame: Some(WindowFrame {
                    x: 10.0,
                    y: 20.0,
                    width: 1200.0,
                    height: 800.0,
                    state: "windowed".into(),
                }),
            },
            WindowLayout {
                id: 2,
                tabs: vec![2, 3],
                active_tab: 2,
                frame: Some(WindowFrame {
                    x: 400.0,
                    y: 60.0,
                    width: 900.0,
                    height: 600.0,
                    state: "maximized".into(),
                }),
            },
        ],
    }
}

fn meta_of(p: &PaneLayout) -> PaneMetaRef<'_> {
    PaneMetaRef {
        session: p.session.as_deref(),
        cwd: p.cwd.as_deref().map(Path::new),
        claude_session_id: p.claude_session_id.as_deref(),
        agent_resume: p
            .agent_resume
            .as_ref()
            .map(|a| (a.agent.as_str(), a.id.as_deref())),
        logged_history: p.logged_history,
        preview: p
            .preview
            .as_ref()
            .map(|pv| (Path::new(pv.path.as_str()), pv.mode.as_str())),
        webview: p.webview.as_deref().map(Cow::Borrowed),
    }
}

fn collect_metas(file: &LayoutFile) -> HashMap<u64, PaneMetaRef<'_>> {
    fn walk<'a>(node: &'a NodeLayout, out: &mut HashMap<u64, PaneMetaRef<'a>>) {
        match node {
            NodeLayout::Pane(p) => {
                out.insert(p.id, meta_of(p));
            }
            NodeLayout::Split { first, second, .. } => {
                walk(first, out);
                walk(second, out);
            }
        }
    }
    let mut out = HashMap::new();
    for t in &file.tabs {
        walk(&t.tree, &mut out);
    }
    for p in &file.backgrounded {
        out.insert(p.id, meta_of(p));
    }
    out
}

fn extras_of(file: &LayoutFile) -> LayoutExtras {
    LayoutExtras {
        window: file.window.clone(),
        window_frames: file
            .windows
            .iter()
            .map(|w| (w.id, w.frame.clone()))
            .collect(),
        collapsed: file.collapsed.clone(),
        webview_dock: file.webview_dock.clone(),
    }
}

fn key_of(file: &LayoutFile) -> ChangeKey {
    let (ws, _) = layout::restore(file).expect("基準シーンを復元できる");
    let metas = collect_metas(file);
    layout::change_key(
        &ws,
        &|p: PaneId| metas.get(&p.as_u64()).cloned().unwrap_or_default(),
        &extras_of(file),
    )
}

fn json_of(file: &LayoutFile) -> String {
    let (ws, _) = layout::restore(file).expect("基準シーンを復元できる");
    let metas = collect_metas(file);
    let extras = extras_of(file);
    let mut out = layout::capture(
        &ws,
        &|p: PaneId| {
            metas
                .get(&p.as_u64())
                .cloned()
                .unwrap_or_default()
                .to_meta()
        },
        extras.window.clone(),
    );
    extras.apply(&mut out);
    serde_json::to_string(&out).expect("直列化できる")
}

fn leaf(file: &mut LayoutFile, id: u64) -> &mut PaneLayout {
    fn walk(node: &mut NodeLayout, id: u64) -> Option<&mut PaneLayout> {
        match node {
            NodeLayout::Pane(p) => (p.id == id).then_some(&mut **p),
            NodeLayout::Split { first, second, .. } => walk(first, id).or_else(|| walk(second, id)),
        }
    }
    for t in &mut file.tabs {
        // borrow checker の都合で二度引く（テスト専用の探索なので素直に書く）
        if walk(&mut t.tree, id).is_some() {
            return walk(&mut t.tree, id).unwrap();
        }
    }
    panic!("葉 {id} が無い");
}

fn frame(file: &mut LayoutFile) -> &mut WindowFrame {
    file.window.as_mut().expect("基準シーンにフレームがある")
}

fn set_axis(node: &mut NodeLayout, value: &str) {
    if let NodeLayout::Split { axis, .. } = node {
        *axis = value.to_string();
    } else {
        panic!("分割ではない");
    }
}

fn set_ratio(node: &mut NodeLayout, value: f32) {
    if let NodeLayout::Split { ratio, .. } = node {
        *ratio = value;
    } else {
        panic!("分割ではない");
    }
}

fn swap_children(node: &mut NodeLayout) {
    if let NodeLayout::Split { first, second, .. } = node {
        std::mem::swap(first, second);
    } else {
        panic!("分割ではない");
    }
}

/// `fn save_layout` の本体（次の `    fn ` まで）を切り出す
fn save_layout_body(src: &str) -> &str {
    let start = src
        .find("    fn save_layout(&mut self) {")
        .expect("save_layout が無い");
    let rest = &src[start..];
    let end = rest[1..]
        .find("\n    fn ")
        .map(|i| i + 1)
        .unwrap_or(rest.len());
    &rest[..end]
}

/// `pub struct <name>` のフィールド名を宣言順に読む（属性・doc コメントは飛ばす）
fn struct_field_names(src: &str, name: &str) -> Vec<String> {
    let head = format!("pub struct {name}");
    let start = src
        .match_indices(&head)
        .find(|(i, _)| {
            // `PaneMeta` が `PaneMetaRef` に当たらないよう、直後の文字で切る
            let after = src[i + head.len()..].chars().next();
            matches!(after, Some(' ') | Some('<') | Some('{'))
        })
        .map(|(i, _)| i)
        .unwrap_or_else(|| panic!("struct {name} が無い"));
    let body_start = start + src[start..].find('{').expect("本体が無い");
    let mut depth = 0usize;
    let mut end = body_start;
    for (i, c) in src[body_start..].char_indices() {
        match c {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    end = body_start + i;
                    break;
                }
            }
            _ => {}
        }
    }
    src[body_start..end]
        .lines()
        .filter_map(|line| {
            let line = line.trim();
            let rest = line.strip_prefix("pub ")?;
            let field = rest.split(':').next()?.trim();
            (!field.is_empty() && field.chars().all(|c| c.is_alphanumeric() || c == '_'))
                .then(|| field.to_string())
        })
        .collect()
}
