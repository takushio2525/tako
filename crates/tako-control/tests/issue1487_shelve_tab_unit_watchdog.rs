//! タブ単位の退避・復帰が「1 単位のまま」で在り続けることを縛る番犬（Issue #1487）
//!
//! # なぜ要るか
//!
//! ユーザー原文は「タブを BG で最小化したら中のペインが全部分かれて単体でしか
//! 復帰できなくなる／タブで最小化したらタブ単位で BG に移るようにして欲しい」。
//! 直したあと同じ形へ戻る道が 4 つあるので、ここで塞ぐ:
//!
//! 1. `shelve_tab` が `into_panes()` で**平坦化**へ戻る（#1487 そのもの）。
//!    A/B の腕（`TAKO_1487_LEGACY_ARM`）だけは平坦化してよいので、そこは除外する
//! 2. 「バックグラウンドに居るペイン」を数える**寿命に関わる経路**が
//!    `shelved_panes()`（= 平坦な退避だけ）へ戻る。退避タブ配下が漏れると
//!    tmux の掃除が**生きている器を kill する**・ペインのビューが刈られる
//! 3. 永続（`layout.json`）から `shelved_tabs` が落ちる = 再起動で分割が戻らない
//! 4. CLI / MCP の口（`tako foreground --tab` / `tako_foreground_pane` の `tab`）が
//!    消える = 開発不変条件「UI でできることは AI からもできる」が壊れる
//!
//! # 見逃す側へ倒れないための作り
//!
//! 走査が空振りすれば全部が無意味に緑になるので、[`走査が空振りしていない`] で
//! 対象の窓が採れていることを固定し、[`逆戻りを名指しできる`] で**注入 8 通り**が
//! `file:line` で名指しされることを確かめる。範囲取りは #1420 の 1 実装
//! （`production_range`）を通す（テスト領域の綴りを本番と取り違えない）。

use std::path::{Path, PathBuf};

#[path = "common/production_range.rs"]
mod production_range;

const WORKSPACE: &str = "crates/tako-core/src/workspace.rs";
const LAYOUT: &str = "crates/tako-control/src/layout.rs";
const DISPATCH: &str = "crates/tako-control/src/dispatch.rs";
const APP: &str = "crates/tako-app/src/main.rs";
const CLI: &str = "crates/tako-cli/src/main.rs";
const CATALOG: &str = "crates/tako-control/src/mcp/catalog.rs";
const TAB_BAR: &str = "crates/tako-app/src/tab_bar.rs";
const DRAWER: &str = "crates/tako-app/src/drawer.rs";
const CARD: &str = "crates/tako-app/src/command_card_ui.rs";
const STATUS: &str = "crates/tako-app/src/status_bar.rs";
const PREVIEW: &str = "crates/tako-app/src/preview_render.rs";

/// タブを退避へ送る**ユーザー操作の入口**。どれも `background_tab` の 1 経路を通る
/// （経路ごとに退避のしかたを書くと、片方だけ #1487 前の平坦化へ戻る道ができる）
const SHELVE_ENTRYPOINTS: &[(&str, &str, &str)] = &[
    (
        TAB_BAR,
        "this.background_tab(id, cx)",
        "タブバーの「ー」ボタン",
    ),
    (
        DRAWER,
        "this.background_tab(drag.tab, cx)",
        "たまり場へのタブ D&D（TabDrag のドロップ）",
    ),
];

/// 「バックグラウンドに居るペイン」を**全部**見なければならない関数（寿命に関わる経路）。
/// ここが平坦な `shelved_panes()` だけを見ると、退避タブ配下の器が保護対象から落ちる
const LIFECYCLE_FNS: &[(&str, &str, &str)] = &[
    (
        APP,
        "fn tmux_unlisted_sessions(&self)",
        "退避タブ配下の器が「管理外 / kill 漏れ?」に出て、掃除の対象になる",
    ),
    (
        DISPATCH,
        "fn collect_live_panes(host: &dyn ControlHost)",
        "退避タブ配下のペインが worker レジストリから「消えた」と見なされる",
    ),
];

/// 「まだ生きているペインか」を判定する側。`all_pane_ids()`（表示中 + バックグラウンド
/// 全部）を通さないと、タブを「ー」で送った瞬間に配下ペインの持ち物が道連れで消える。
/// **#1487 以前は平坦化されていたので `shelved_panes()` だけで足りていた** =
/// 直し漏れがそのまま「旧実装より後退」になる箇所
const ALIVE_FNS: &[(&str, &str, &str)] = &[
    (
        CARD,
        "fn prune_command_cards(&mut self)",
        "退避したタブ配下ペインのコマンドカード（`tako show-command` の論理文字列ごと）が消える",
    ),
    (
        APP,
        "fn prune_pane_body_views(&mut self)",
        "退避タブ配下のペインのビューが刈られる",
    ),
];

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("crates/tako-control の 2 つ上がワークスペースルート")
        .to_path_buf()
}

fn read(root: &Path, rel: &str) -> String {
    std::fs::read_to_string(root.join(rel)).unwrap_or_else(|e| panic!("{rel} が読める: {e}"))
}

/// 本番コードだけの眺め（#1420 の 1 実装。**切らずにテスト領域だけを潰す**）
fn prod(root: &Path, rel: &'static str) -> String {
    production_range::production(&read(root, rel), rel)
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

/// 関数の窓（1-based の宣言行と本文）。終わりは**宣言行と同じ字下げの `}`**
fn fn_window(src: &str, needle: &str) -> Option<(usize, String)> {
    let lines: Vec<&str> = src.lines().collect();
    let start = lines.iter().position(|l| l.contains(needle))?;
    let indent = lines[start].len() - lines[start].trim_start().len();
    let close = format!("{}}}", " ".repeat(indent));
    let end = lines
        .iter()
        .enumerate()
        .skip(start + 1)
        .find(|(_, l)| **l == close)
        .map(|(i, _)| i)
        .unwrap_or(lines.len() - 1);
    Some((start + 1, lines[start..=end].join("\n")))
}

struct Sources {
    workspace: String,
    tab_bar: String,
    drawer: String,
    card: String,
    status: String,
    preview: String,
    layout: String,
    dispatch: String,
    app: String,
    cli: String,
    catalog: String,
}

impl Sources {
    fn load(root: &Path) -> Self {
        Self {
            workspace: prod(root, WORKSPACE),
            tab_bar: prod(root, TAB_BAR),
            drawer: prod(root, DRAWER),
            card: prod(root, CARD),
            status: prod(root, STATUS),
            preview: prod(root, PREVIEW),
            layout: prod(root, LAYOUT),
            dispatch: prod(root, DISPATCH),
            app: prod(root, APP),
            cli: prod(root, CLI),
            catalog: prod(root, CATALOG),
        }
    }

    fn get(&self, file: &str) -> &str {
        match file {
            WORKSPACE => &self.workspace,
            TAB_BAR => &self.tab_bar,
            DRAWER => &self.drawer,
            CARD => &self.card,
            STATUS => &self.status,
            PREVIEW => &self.preview,
            LAYOUT => &self.layout,
            DISPATCH => &self.dispatch,
            APP => &self.app,
            CLI => &self.cli,
            CATALOG => &self.catalog,
            other => panic!("未知のファイル: {other}"),
        }
    }
}

/// 1: `shelve_tab` はタブを分解しない（A/B の腕の中だけは平坦化してよい）
fn scan_shelve_keeps_tab(src: &str) -> Vec<Offender> {
    let Some((at, window)) = fn_window(src, "pub fn shelve_tab_in(") else {
        return vec![Offender {
            file: WORKSPACE,
            line: 0,
            why: "`shelve_tab_in` が見つからない（走査が空振り）".into(),
        }];
    };
    let mut out = Vec::new();
    // Tab をそのまま退避列へ積んでいるか
    if !window.contains("BackgroundTab::new(") {
        out.push(Offender {
            file: WORKSPACE,
            line: at,
            why: "`shelve_tab_in` が `BackgroundTab` を作っていない（#1487。\
                  タブを 1 単位で退避しないと、分割ツリー・比率・並び位置が失われる）"
                .into(),
        });
    }
    // A/B の腕の外で平坦化していないか（腕は ARM コメントで囲ってある）
    let mut in_arm = false;
    for (i, line) in window.lines().enumerate() {
        if line.contains("TAKO_1487_LEGACY_ARM 開始") {
            in_arm = true;
            continue;
        }
        if line.contains("TAKO_1487_LEGACY_ARM 終了") {
            in_arm = false;
            continue;
        }
        if in_arm || line.trim_start().starts_with("//") {
            continue;
        }
        if line.contains("into_panes()") {
            out.push(Offender {
                file: WORKSPACE,
                line: at + i,
                why: "`shelve_tab_in` が A/B の腕の外でタブを平坦化している\
                      （#1487 が戻っている。ユーザー原文「中にあるペインが全部分かれて\
                      単体でしか復帰できなくなる」そのもの）"
                    .into(),
            });
        }
    }
    out
}

/// 2: 寿命に関わる経路は「バックグラウンドに居るペイン**全部**」を見る
fn scan_lifecycle(s: &Sources) -> Vec<Offender> {
    let mut out = Vec::new();
    for (file, needle, consequence) in LIFECYCLE_FNS {
        let Some((at, window)) = fn_window(s.get(file), needle) else {
            out.push(Offender {
                file,
                line: 0,
                why: format!("`{needle}` が見つからない（走査が空振り）"),
            });
            continue;
        };
        if window.contains("all_background_panes()") {
            continue;
        }
        out.push(Offender {
            file,
            line: at,
            why: format!(
                "`{needle}` が `all_background_panes()` を通っていない（#1487。{consequence}）"
            ),
        });
    }
    out
}

/// 2b: 「まだ生きているペインか」を判定する側は `all_pane_ids()` を通る
fn scan_alive(s: &Sources) -> Vec<Offender> {
    let mut out = Vec::new();
    for (file, needle, consequence) in ALIVE_FNS {
        let Some((at, window)) = fn_window(s.get(file), needle) else {
            out.push(Offender {
                file,
                line: 0,
                why: format!("`{needle}` が見つからない（走査が空振り）"),
            });
            continue;
        };
        if window.contains("all_pane_ids()") {
            continue;
        }
        out.push(Offender {
            file,
            line: at,
            why: format!("`{needle}` が `all_pane_ids()` を通っていない（#1487。{consequence}）"),
        });
    }
    // 「⏏ 退避」バッジの件数は「バックグラウンドに居るペイン数」で揃える
    if !s
        .status
        .contains("let bg_count = self.workspace.all_background_panes().len();")
    {
        out.push(Offender {
            file: STATUS,
            line: 0,
            why: "退避バッジの件数が `all_background_panes()` ではない（#1487。\
                  退避タブしか無いときに 0 と出てドロワーの中身と矛盾する）"
                .into(),
        });
    }
    // 一括プレビューの見出しはタイトルの出どころを 1 本にする
    let Some((at, window)) = fn_window(&s.preview, "fn background_group_label(&self, tab: TabId)")
    else {
        out.push(Offender {
            file: PREVIEW,
            line: 0,
            why: "`background_group_label` が無い（#1487。見出しのタイトルの出どころが\
                  散ると、退避タブで空文字になる形へ戻る）"
                .into(),
        });
        return out;
    };
    if !window.contains("self.workspace.shelved_tab(tab)") {
        out.push(Offender {
            file: PREVIEW,
            line: at,
            why: "`background_group_label` が退避タブのタイトルを見ていない（#1487。\
                  平坦な退避の `origin_tab_title` しか探さないと見出しが空文字になる）"
                .into(),
        });
    }
    if let Some((label_at, label_window)) = fn_window(&s.preview, "fn preview_label(&self") {
        if label_window.contains("shelved_panes()") {
            out.push(Offender {
                file: PREVIEW,
                line: label_at,
                why: "`preview_label` が平坦な退避を直に読んでいる（#1487。\
                      見出しは `background_group_label` の 1 本を通すこと）"
                    .into(),
            });
        }
    }
    // 退避タブと同じ由来を持つ平坦な退避が「閉じたタブ」へ二重に出ない
    let Some((at, window)) = fn_window(&s.app, "fn tmux_view_closed_origin_background(&self)")
    else {
        out.push(Offender {
            file: APP,
            line: 0,
            why: "`tmux_view_closed_origin_background` が見つからない（走査が空振り）".into(),
        });
        return out;
    };
    if !window.contains("is_shelved_tab(origin)") {
        out.push(Offender {
            file: APP,
            line: at,
            why: "由来タブが**退避中**のものを「閉じたタブ」グループから外していない\
                  （#1487。ペイン単位で退避したあとタブごと退避すると、同じペインが\
                  退避タブのカードと「閉じたタブ」の 2 か所に出て、しかも閉じていない）"
                .into(),
        });
    }
    out
}

/// 3: 永続（`layout.json`）が退避タブを往復させる
fn scan_persistence(s: &Sources) -> Vec<Offender> {
    let mut out = Vec::new();
    for (needle, why) in [
        (
            "pub shelved_tabs: Vec<ShelvedTabLayout>",
            "`LayoutFile` に `shelved_tabs` が無い（再起動で退避タブが消える）",
        ),
        (
            "shelved_tabs: ws",
            "`capture` が退避タブを書き出していない（保存されない = 再起動で消える）",
        ),
        (
            "for entry in &file.shelved_tabs",
            "`restore` が退避タブを読み戻していない（保存はされるが復元されない）",
        ),
    ] {
        if !s.layout.contains(needle) {
            out.push(Offender {
                file: LAYOUT,
                line: 0,
                why: format!("{why}（#1487）"),
            });
        }
    }
    // 変化検出キー（#1425）に載っていないと「保存されないフィールド」になる
    if !s.layout.contains("ws.shelved_tabs().len().hash(&mut h)") {
        out.push(Offender {
            file: LAYOUT,
            line: 0,
            why: "`change_key` が退避タブを見ていない（#1425 の保存漏れ。\
                  退避タブだけ増減しても「変わっていない」と判定され保存されない）"
                .into(),
        });
    }
    out
}

/// 4: CLI / MCP の口（開発不変条件「UI でできることは AI からもできる」）
fn scan_ai_surface(s: &Sources) -> Vec<Offender> {
    let mut out = Vec::new();
    if !s
        .cli
        .contains("conflicts_with_all = [\"pane\", \"target\", \"direction\"]")
    {
        out.push(Offender {
            file: CLI,
            line: 0,
            why: "`tako foreground --tab` が無い（#1487。タブ単位の復帰が CLI から呼べない）"
                .into(),
        });
    }
    if !s.catalog.contains("復帰させる退避タブの ID") {
        out.push(Offender {
            file: CATALOG,
            line: 0,
            why: "`tako_foreground_pane` の catalog に `tab` が無い（#1487。\
                  catalog は MCP の許可リストなので、無い引数はどのクライアントからも渡せない）"
                .into(),
        });
    }
    // 退避タブもホバープレビューのピン留め対象（GUI が解決するのに dispatch が拒まない）
    if let Some((at, window)) = fn_window(&s.dispatch, "Request::Pin {") {
        if !window.contains("is_shelved_tab(tab)") {
            out.push(Offender {
                file: DISPATCH,
                line: at,
                why: "`Request::Pin { group_tab }` が退避タブを知らない（#1487。\
                      GUI の `background_entries_of_tab` は解決するのに CLI / MCP からは\
                      TabNotFound になる = 設計原則 5 の 1:1 の穴）"
                    .into(),
            });
        }
    } else {
        out.push(Offender {
            file: DISPATCH,
            line: 0,
            why: "`Request::Pin` の腕が見つからない（走査が空振り）".into(),
        });
    }
    // 平坦な一覧を残すこと（今動いているクライアントを壊さない = #1467 と同じ理屈）
    let Some((at, window)) = fn_window(&s.dispatch, "Request::BackgroundList => {") else {
        out.push(Offender {
            file: DISPATCH,
            line: 0,
            why: "`BackgroundList` の腕が見つからない（走査が空振り）".into(),
        });
        return out;
    };
    if !window.contains("\"backgrounded\": items") {
        out.push(Offender {
            file: DISPATCH,
            line: at,
            why: "`BackgroundList` が平坦な `backgrounded` を返していない（#1487。\
                  既存の契約を壊すと今動いているクライアントが読めなくなる）"
                .into(),
        });
    }
    if !window.contains("\"tabs\": tabs") {
        out.push(Offender {
            file: DISPATCH,
            line: at,
            why: "`BackgroundList` が `tabs`（退避タブ）を返していない（#1487）".into(),
        });
    }
    if !window.contains("items.extend(") {
        out.push(Offender {
            file: DISPATCH,
            line: at,
            why: "退避タブ配下のペインが平坦な一覧に出ていない（#1487。\
                  1 本だけ取り出したいクライアントが対象を見つけられない）"
                .into(),
        });
    }
    out
}

/// 4b: 退避の入口（「ー」ボタンと D&D）は同じ 1 経路を通る
fn scan_entrypoints(s: &Sources) -> Vec<Offender> {
    let mut out = Vec::new();
    for (file, needle, label) in SHELVE_ENTRYPOINTS {
        if s.get(file).contains(needle) {
            continue;
        }
        out.push(Offender {
            file,
            line: 0,
            why: format!(
                "{label} が `background_tab` を通っていない（#1487。入口ごとに\
                 退避のしかたを書くと、片方だけ平坦化へ戻る道ができる）"
            ),
        });
    }
    // 「ー」を実マウスで押せる実矩形（visual-test 項目 153）
    if !s.tab_bar.contains("tab-bg-") {
        out.push(Offender {
            file: TAB_BAR,
            line: 0,
            why: "タブバーの「ー」に実矩形（probe）が無い（#1487。\
                  合成マウスで押せないと #496 型の「押しても発火しない」を検出できない）"
                .into(),
        });
    }
    // たまり場からタブごと戻す導線（#1491 でテキストボタン → タブ形カードへ変わった）。
    // 実矩形（probe）そのものは `issue1491_shelved_tab_card_watchdog` が縛るので、
    // ここは「描画の呼び出しが在る」ことだけを見る
    if !s.drawer.contains("render_shelved_tab_card(") {
        out.push(Offender {
            file: DRAWER,
            line: 0,
            why: "たまり場に退避タブをタブごと戻す導線が無い（#1487 / #1491）".into(),
        });
    }
    out
}

/// 5: A/B の口（同一バイナリで #1487 前の症状を再現できる）
fn scan_ab(s: &Sources) -> Vec<Offender> {
    if s.workspace.contains("TAKO_1487_LEGACY") {
        return Vec::new();
    }
    vec![Offender {
        file: WORKSPACE,
        line: 0,
        why: "`TAKO_1487_LEGACY` を読む場所が 0 か所（A/B で旧挙動を再現できない）".into(),
    }]
}

fn scan_all(s: &Sources) -> Vec<Offender> {
    let mut out = scan_shelve_keeps_tab(&s.workspace);
    out.extend(scan_lifecycle(s));
    out.extend(scan_alive(s));
    out.extend(scan_persistence(s));
    out.extend(scan_ai_surface(s));
    out.extend(scan_entrypoints(s));
    out.extend(scan_ab(s));
    out
}

/// 本体: 現行 main に逆戻りが無い
#[test]
fn タブ単位の退避が1単位のまま保たれている() {
    let root = workspace_root();
    let offenders = scan_all(&Sources::load(&root));
    assert!(
        offenders.is_empty(),
        "#1487 の逆戻り:\n{}",
        offenders
            .iter()
            .map(Offender::report)
            .collect::<Vec<_>>()
            .join("\n")
    );
}

/// 走査が空振りしていない（窓が採れないと全部が無意味に緑になる）
#[test]
fn 走査が空振りしていない() {
    let root = workspace_root();
    let s = Sources::load(&root);
    for (label, file, needle) in [
        ("タブ退避", WORKSPACE, "pub fn shelve_tab_in("),
        ("タブ復帰", WORKSPACE, "pub fn unshelve_tab("),
        ("一覧", DISPATCH, "Request::BackgroundList => {"),
    ]
    .into_iter()
    .chain(LIFECYCLE_FNS.iter().map(|(f, n, _)| ("寿命経路", *f, *n)))
    .chain(ALIVE_FNS.iter().map(|(f, n, _)| ("alive 判定", *f, *n)))
    .chain([
        (
            "見出し",
            PREVIEW,
            "fn background_group_label(&self, tab: TabId)",
        ),
        (
            "閉じたタブ群",
            APP,
            "fn tmux_view_closed_origin_background(&self)",
        ),
        ("ピン留め", DISPATCH, "Request::Pin {"),
    ]) {
        assert!(
            fn_window(s.get(file), needle).is_some(),
            "{label}: `{needle}` の窓を採れない（{file}。綴りを変えたら番犬も直すこと。\
             見失うとこの番犬は 1 つも検査しない）"
        );
    }
    // A/B の腕の目印（囲みが消えると (1) の除外が効かなくなる）
    assert!(
        s.workspace.contains("TAKO_1487_LEGACY_ARM 開始")
            && s.workspace.contains("TAKO_1487_LEGACY_ARM 終了"),
        "A/B の腕を囲む目印が無い（#1487。除外が効かず本番の平坦化を見逃す）"
    );
}

fn expect_hit(s: &Sources, file: &str, needle: &str) {
    let offenders = scan_all(s);
    assert!(
        offenders
            .iter()
            .any(|o| o.file == file && o.why.contains(needle)),
        "注入したのに名指しされない（needle={needle:?}）:\n{}",
        offenders
            .iter()
            .map(Offender::report)
            .collect::<Vec<_>>()
            .join("\n")
    );
}

/// 直す前の形へ戻す注入が、すべて `file:line` で名指しされること
#[test]
fn 逆戻りを名指しできる() {
    let root = workspace_root();
    assert!(
        scan_all(&Sources::load(&root)).is_empty(),
        "注入前が既に汚れている"
    );

    // (1) タブを平坦化して退避する（#1487 そのもの）
    let mut s = Sources::load(&root);
    s.workspace = s.workspace.replace(
        "        self.shelved_tabs\n            .push(BackgroundTab::new(tab, origin_window, origin_index));",
        "        self.shelved.extend(\n\
         \x20           tab.into_tree()\n\
         \x20               .into_panes()\n\
         \x20               .into_iter()\n\
         \x20               .map(|p| BackgroundPane::from_pane(p, tab_id, origin_title.clone())),\n\
         \x20       );",
    );
    expect_hit(&s, WORKSPACE, "`BackgroundTab` を作っていない");
    expect_hit(&s, WORKSPACE, "A/B の腕の外でタブを平坦化している");

    // (2) tmux の掃除が平坦な退避だけを見る（生きている器を kill する道）
    let mut s = Sources::load(&root);
    s.app = s.app.replace(
        "        let bg_sessions: std::collections::HashSet<String> = self\n\
         \x20           .workspace\n\
         \x20           .all_background_panes()",
        "        let bg_sessions: std::collections::HashSet<String> = self\n\
         \x20           .workspace\n\
         \x20           .shelved_panes()",
    );
    expect_hit(&s, APP, "「管理外 / kill 漏れ?」に出て");

    // (3) ビューの刈り取りが平坦な退避だけを見る
    let mut s = Sources::load(&root);
    let (_, window) = fn_window(&s.app, "fn prune_pane_body_views(&mut self)").expect("窓");
    s.app = s.app.replace(
        &window,
        &window.replace("all_pane_ids()", "shelved_pane_ids_only()"),
    );
    expect_hit(&s, APP, "ビューが刈られる");

    // (4) worker レジストリの現存判定が平坦な退避だけを見る
    let mut s = Sources::load(&root);
    let (_, window) =
        fn_window(&s.dispatch, "fn collect_live_panes(host: &dyn ControlHost)").expect("窓");
    s.dispatch = s.dispatch.replace(
        &window,
        &window.replace("all_background_panes()", "shelved_panes()"),
    );
    expect_hit(&s, DISPATCH, "「消えた」と見なされる");

    // (5) 永続から退避タブが落ちる（再起動で分割が戻らない）
    let mut s = Sources::load(&root);
    s.layout = s
        .layout
        .replace("    pub shelved_tabs: Vec<ShelvedTabLayout>,", "");
    expect_hit(&s, LAYOUT, "`shelved_tabs` が無い");

    // (6) 変化検出キーから落ちる（#1425 の保存漏れ）
    let mut s = Sources::load(&root);
    s.layout = s
        .layout
        .replace("ws.shelved_tabs().len().hash(&mut h);", "");
    expect_hit(&s, LAYOUT, "`change_key` が退避タブを見ていない");

    // (7) MCP から tab を渡せなくなる（catalog は許可リスト）
    let mut s = Sources::load(&root);
    s.catalog = s.catalog.replace("復帰させる退避タブの ID", "（削除）");
    expect_hit(&s, CATALOG, "catalog に `tab` が無い");

    // (8) D&D の入口が別実装へ分かれる
    let mut s = Sources::load(&root);
    s.drawer = s.drawer.replace(
        "this.background_tab(drag.tab, cx);",
        "let _ = this.workspace.shelve_pane(PaneId::from_raw(0));",
    );
    expect_hit(&s, DRAWER, "`background_tab` を通っていない");

    // (9) 「ー」の実矩形が消える（合成マウスで押せなくなる）
    let mut s = Sources::load(&root);
    s.tab_bar = s.tab_bar.replace("tab-bg-", "tab-bg2-");
    expect_hit(&s, TAB_BAR, "実矩形（probe）が無い");

    // (10) コマンドカードの alive 判定が平坦な退避だけを見る（旧実装より後退する道）
    let mut s = Sources::load(&root);
    let (_, window) = fn_window(&s.card, "fn prune_command_cards(&mut self)").expect("窓");
    s.card = s.card.replace(
        &window,
        &window.replace(
            "let alive = self.workspace.all_pane_ids();",
            "let alive: std::collections::HashSet<PaneId> = self\n\
             \x20           .workspace\n\
             \x20           .shelved_panes()\n\
             \x20           .iter()\n\
             \x20           .map(|s| s.pane().id())\n\
             \x20           .collect();",
        ),
    );
    expect_hit(&s, CARD, "コマンドカード");

    // (11) 退避バッジの件数が平坦な退避だけを数える
    let mut s = Sources::load(&root);
    s.status = s.status.replace(
        "let bg_count = self.workspace.all_background_panes().len();",
        "let bg_count = self.workspace.shelved_panes().len();",
    );
    expect_hit(&s, STATUS, "ドロワーの中身と矛盾する");

    // (12) ピン留めが退避タブを知らない（CLI / MCP から触れない）
    let mut s = Sources::load(&root);
    s.dispatch = s.dispatch.replace(
        "let known = host.workspace().is_shelved_tab(tab)",
        "let known = false",
    );
    expect_hit(&s, DISPATCH, "設計原則 5 の 1:1 の穴");

    // (13) 見出しのタイトルが平坦な退避しか見ない（退避タブで空文字）
    let mut s = Sources::load(&root);
    let (_, window) =
        fn_window(&s.preview, "fn background_group_label(&self, tab: TabId)").expect("窓");
    s.preview = s.preview.replace(
        &window,
        &window.replace(
            "if let Some(entry) = self.workspace.shelved_tab(tab) {",
            "if None::<()>.is_some() { let entry = self.workspace.active_tab();",
        ),
    );
    expect_hit(&s, PREVIEW, "見出しが空文字になる");

    // (14) 退避中の由来タブを「閉じたタブ」から外さない（二重表示 + 誤表示）
    let mut s = Sources::load(&root);
    s.app = s.app.replace(
        "if self.workspace.get_tab(origin).is_some() || self.workspace.is_shelved_tab(origin) {",
        "if self.workspace.get_tab(origin).is_some() {",
    );
    expect_hit(&s, APP, "2 か所に出て");

    // (15) A/B の口が消える（症状を再現できなくなる）
    let mut s = Sources::load(&root);
    s.workspace = s.workspace.replace("TAKO_1487_LEGACY", "TAKO_NONE_LEGACY");
    expect_hit(&s, WORKSPACE, "を読む場所が 0 か所");
}
