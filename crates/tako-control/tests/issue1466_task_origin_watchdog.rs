//! ユーザータスクの戻り先（#1466）を縛る番犬
//!
//! # なぜ要るか
//!
//! `tako todo add` を **worker から**呼ぶと、以前は起票元のプロファイルが解けずに
//! `default` へ落ち、返答が「たまたま生きている別プロジェクトの master」の入力欄へ
//! 入っていた。届く先が間違っているだけでなく、
//!
//! - 受け取った master には**自分が起票していないタスク**の返答が届く（文脈が無い）
//! - 起票した worker / master には**何も届かない**（待ち続ける）
//! - どちらの側にも「別の宛先へ行った」と分かる手がかりが無い（無言）
//!
//! という三重の無言の失敗になる。直し方は「spawn 時点の事実（`spawned_by` の枝）から
//! 解く」+「それでも解けなければ `default` へ落とさず宛先不明にする」の 2 本立てで、
//! **どちらか片方でも戻ると症状が再発する**（前者が戻れば別人へ届き、
//! 後者が戻れば「解けなかった」が既定へ化ける）。静的に固定するのはそこ。
//!
//! # 何を縛るか
//!
//! 1. **解決は 1 実装を通る** — `user_task_origin` が
//!    `user_tasks::resolve_origin_profile` へ判断を渡し、材料として
//!    spawn 元（`spawn_chain_master_profile`）と管轄（`profile_governing_project`）を
//!    集める。dispatch の中で条件分岐を書き直さない
//! 2. **既定へ落とさない** — `origin_profile` は `Option` を返し、`default` を
//!    名乗れるのは A/B の legacy アームだけ
//! 3. **宛先不明は失敗として残る** — `deliver_user_task_response` が
//!    `unresolved_delivery` を返し、理由が `tako todo show` に出る
//! 4. **名乗りは戻り先から作らない** — `user_task_created_by` が `origin.profile` を
//!    読むと、worker の起票が `master:<profile>` を騙る（画面の「誰が頼んだか」が嘘になる）
//! 5. **role の語彙は 1 実装** — worker のプロジェクトキーは
//!    `handoff::worker_project_of_any_role` から読む（`strip_prefix` の手書きを増やすと
//!    表示用 / env 用のどちらかだけ解ける実装が生えて #761 の取り違えが戻る）
//! 6. **A/B の逃げ道は #1466 専用の env** — 他 Issue のアームと軸を混ぜない（#1422）
//!
//! # 見逃す側へ倒れないための作り
//!
//! 走査が空振りすれば 1〜6 はすべて無意味に緑になるので、
//! [`走査が空振りしていない`] で窓が採れていることを固定し、
//! [`逆戻りを名指しできる`] で**注入 8 通り**が `file:line` で名指しされることを
//! 確かめる。範囲取りは #1420 の 1 実装（`production_range`）を通す

use std::path::{Path, PathBuf};

#[path = "common/production_range.rs"]
mod production_range;

const DISPATCH: &str = "crates/tako-control/src/dispatch.rs";
const USER_TASKS: &str = "crates/tako-control/src/user_tasks.rs";
const HANDOFF: &str = "crates/tako-core/src/handoff.rs";
const ORCHESTRATOR: &str = "crates/tako-control/src/orchestrator/mod.rs";

/// #1466 の A/B の軸（他 Issue のアームと混ぜない）
const LEGACY_ENV: &str = "TAKO_1466_LEGACY";

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

/// 本番コードだけの眺め（#1420 の 1 実装）
fn prod(root: &Path, rel: &'static str) -> String {
    production_range::production(&read(root, rel), rel)
}

/// 1 つの違反（`ファイル:行 — 理由`）
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

/// 関数の窓（宣言行の 1-based 行番号と本文）。
/// 終わりは**宣言行と同じ字下げの `}`**（「宣言行から N 行」で切ると隣の実装を数える）
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

/// 注釈（`//` で始まる行）を落とした窓。検査は**コードだけ**を見る
fn code_only(window: &str) -> String {
    window
        .lines()
        .filter(|l| !l.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// 窓の中で `needle` を含む最初の**コードの**行（0-based の相対位置）
fn code_line_with(window: &str, needle: &str) -> Option<usize> {
    window
        .lines()
        .position(|l| !l.trim_start().starts_with("//") && l.contains(needle))
}

// ---------------------------------------------------------------------------
// 検査本体（注入テストから同じ関数を呼べるよう、材料は引数で受ける）
// ---------------------------------------------------------------------------

struct Sources {
    dispatch: String,
    user_tasks: String,
    handoff: String,
    orchestrator: String,
}

impl Sources {
    fn load(root: &Path) -> Self {
        Self {
            dispatch: prod(root, DISPATCH),
            user_tasks: prod(root, USER_TASKS),
            handoff: prod(root, HANDOFF),
            orchestrator: prod(root, ORCHESTRATOR),
        }
    }
}

/// 1 + 5: 起票元の解決が 1 実装を通り、材料を全部集めている
fn scan_origin(src: &str) -> Vec<Offender> {
    let mut out = Vec::new();
    let Some((at, window)) = fn_window(src, "fn user_task_origin(") else {
        out.push(Offender {
            file: DISPATCH,
            line: 0,
            why: "`user_task_origin` が見つからない（走査が空振り）".into(),
        });
        return out;
    };
    let code = code_only(&window);
    for (mark, why) in [
        (
            "user_tasks::resolve_origin_profile(",
            "起票元の判断が `user_tasks::resolve_origin_profile` を通っていない\
             （dispatch の中で解き直すと、順序と「解けない」の扱いが 2 か所に割れる。#1466）",
        ),
        (
            "spawn_chain_master_profile(",
            "spawn 元（`spawned_by` の枝）を材料に集めていない。worker の起票が\
             master を名乗る role だけで解かれ、無関係な `default` の master へ返答が届く（#1466）",
        ),
        (
            "profile_governing_project",
            "管轄プロファイルの保険が無い（GUI 再起動で `spawned_by` が失われた worker の\
             起票が、以後ずっと宛先不明になる。#1466）",
        ),
        (
            "worker_project_of_any_role",
            "worker のプロジェクトキーを `handoff::worker_project_of_any_role` から\
             読んでいない（表示用 / env 用のどちらかだけ解ける実装が生える。#761 / #1466）",
        ),
    ] {
        if !code.contains(mark) {
            out.push(Offender {
                file: DISPATCH,
                line: at,
                why: why.into(),
            });
        }
    }
    // 既定プロファイルを名乗るのは「空 suffix を綴り直す」1 か所だけ
    // （`spawn_chain_master_profile` の中。ここで既定へ落とすと #1466 が戻る）
    if let Some(i) = code_line_with(&window, "DEFAULT_PROFILE") {
        out.push(Offender {
            file: DISPATCH,
            line: at + i,
            why: "`user_task_origin` が既定プロファイルを名乗っている\
                  （解けなかった起票が `default` へ落ちる = #1466 の症状そのもの）"
                .into(),
        });
    }
    // spawn 元の綴りを揃える 1 実装が在る（空 suffix と「解けない」を混ぜない）
    match fn_window(src, "fn spawn_chain_master_profile(") {
        None => out.push(Offender {
            file: DISPATCH,
            line: 0,
            why: "`spawn_chain_master_profile` が無い（#1466）".into(),
        }),
        Some((at, window)) => {
            let code = code_only(&window);
            for (mark, why) in [
                (
                    "find_master_suffix_from(",
                    "spawn 元の探索が `find_master_suffix_from`（worker spawn の既定と\
                     同じ 1 実装）を通っていない",
                ),
                (
                    "DEFAULT_PROFILE",
                    "既定 master の空 suffix を `default` へ綴り直していない\
                     （「解けなかった」と区別できなくなる。#1466）",
                ),
            ] {
                if !code.contains(mark) {
                    out.push(Offender {
                        file: DISPATCH,
                        line: at,
                        why: why.into(),
                    });
                }
            }
        }
    }
    out
}

/// 2: `origin_profile` は `Option` で、既定を名乗れるのは legacy アームだけ
fn scan_origin_profile(src: &str) -> Vec<Offender> {
    let mut out = Vec::new();
    let Some((at, window)) = fn_window(src, "pub fn origin_profile(") else {
        out.push(Offender {
            file: USER_TASKS,
            line: 0,
            why: "`origin_profile` が見つからない（走査が空振り）".into(),
        });
        return out;
    };
    let head = window.lines().next().unwrap_or_default();
    if !head.contains("-> Option<String>") {
        out.push(Offender {
            file: USER_TASKS,
            line: at,
            why: "`origin_profile` が `Option` を返していない（解けていない起票を\
                  必ず何かのプロファイルとして扱う = `default` へ落ちる。#1466）"
                .into(),
        });
    }
    // 既定を名乗る行があるなら、それは legacy アームでなければならない
    for (i, line) in window.lines().enumerate() {
        if line.trim_start().starts_with("//") || !line.contains("DEFAULT_PROFILE") {
            continue;
        }
        if !line.contains("legacy_origin_resolution()") {
            out.push(Offender {
                file: USER_TASKS,
                line: at + i,
                why: "`origin_profile` が legacy アームの外で既定プロファイルを\
                      名乗っている（#1466 の症状が戻る）"
                    .into(),
            });
        }
    }
    // 純粋な解決関数が在り、「解けない」を表現できる
    match fn_window(src, "pub fn resolve_origin_profile(") {
        None => out.push(Offender {
            file: USER_TASKS,
            line: 0,
            why: "`resolve_origin_profile`（起票元を解く純粋関数）が無い。#1466".into(),
        }),
        Some((at, window)) => {
            let head_end = window.find('{').unwrap_or(window.len());
            let signature = &window[..head_end];
            for arg in [
                "caller_role",
                "pane_master_profile",
                "spawn_chain_profile",
                "jurisdiction_profile",
                "legacy",
            ] {
                if !signature.contains(arg) {
                    out.push(Offender {
                        file: USER_TASKS,
                        line: at,
                        why: format!(
                            "`resolve_origin_profile` が材料 `{arg}` を受け取っていない\
                             （解決順のどれかが落ちている。#1466）"
                        ),
                    });
                }
            }
            if !signature.contains("Option<(String, OriginSource)>") {
                out.push(Offender {
                    file: USER_TASKS,
                    line: at,
                    why: "`resolve_origin_profile` が「解けない」を返せない\
                          （`Option` でないと既定へ落とすしかなくなる。#1466）"
                        .into(),
                });
            }
        }
    }
    // 宛先不明の記録は宛先を持たない
    match fn_window(src, "pub fn unresolved_delivery(") {
        None => out.push(Offender {
            file: USER_TASKS,
            line: 0,
            why: "`unresolved_delivery` が無い（宛先不明の配送を記録できない。#1466）".into(),
        }),
        Some((at, window)) => {
            let code = code_only(&window);
            for (mark, why) in [
                (
                    "DeliveryState::Failed",
                    "宛先不明を失敗として記録していない",
                ),
                ("profile: None", "宛先不明なのにプロファイルを名乗っている"),
                ("pane: None", "宛先不明なのにペインを選んでいる"),
                (
                    "UNRESOLVED_ORIGIN_REASON",
                    "理由が正本（`UNRESOLVED_ORIGIN_REASON`）から来ていない\
                     （画面と CLI で文言が割れる）",
                ),
            ] {
                if !code.contains(mark) {
                    out.push(Offender {
                        file: USER_TASKS,
                        line: at,
                        why: format!("{why}。#1466"),
                    });
                }
            }
        }
    }
    out
}

/// 3: 配送は「解けていなければどこへも送らない」
fn scan_delivery(src: &str) -> Vec<Offender> {
    let mut out = Vec::new();
    let Some((at, window)) = fn_window(src, "fn deliver_user_task_response(") else {
        out.push(Offender {
            file: DISPATCH,
            line: 0,
            why: "`deliver_user_task_response` が見つからない（走査が空振り）".into(),
        });
        return out;
    };
    let code = code_only(&window);
    for (mark, why) in [
        (
            "unresolved_delivery(",
            "起票元が解けないときに `unresolved_delivery` を返していない\
             （宛先不明の返答が誰かのペインへ配送される = #1466 の症状）",
        ),
        (
            "origin_unresolved",
            "宛先不明が persist.log に残らない（後から「どこへ行ったか」を追えない。#1466）",
        ),
    ] {
        if !code.contains(mark) {
            out.push(Offender {
                file: DISPATCH,
                line: at,
                why: why.into(),
            });
        }
    }
    // 既定プロファイルで配送先を選び直す抜け道を塞ぐ
    if let Some(i) = code_line_with(&window, "DEFAULT_PROFILE") {
        out.push(Offender {
            file: DISPATCH,
            line: at + i,
            why: "配送が既定プロファイルへ落ちている（#1466）".into(),
        });
    }
    out
}

/// 4: 起票者の名乗りは戻り先（`origin.profile`）から作らない
fn scan_created_by(src: &str) -> Vec<Offender> {
    let mut out = Vec::new();
    let Some((at, window)) = fn_window(src, "fn user_task_created_by(") else {
        out.push(Offender {
            file: DISPATCH,
            line: 0,
            why: "`user_task_created_by` が見つからない（走査が空振り）".into(),
        });
        return out;
    };
    if let Some(i) = code_line_with(&window, "origin.profile") {
        out.push(Offender {
            file: DISPATCH,
            line: at + i,
            why: "名乗りを `origin.profile`（返答の戻り先）から作っている。#1466 で\
                  worker の戻り先が master のプロファイルへ解けるようになったので、\
                  worker の起票が `master:<profile>` を騙る"
                .into(),
        });
    }
    if !code_only(&window).contains("caller_role") {
        out.push(Offender {
            file: DISPATCH,
            line: at,
            why: "名乗りが呼び出し元自身の役割を見ていない（#1466）".into(),
        });
    }
    out
}

/// 5: role の語彙は tako-core の 1 実装 / 6: 管轄は一意のときだけ引く
fn scan_vocabulary(handoff: &str, orchestrator: &str) -> Vec<Offender> {
    let mut out = Vec::new();
    match fn_window(handoff, "pub fn worker_project_of_any_role(") {
        None => out.push(Offender {
            file: HANDOFF,
            line: 0,
            why: "`worker_project_of_any_role` が無い（worker の role からプロジェクトを\
                  読む 1 実装。#1466）"
                .into(),
        }),
        Some((at, window)) => {
            let code = code_only(&window);
            for vocab in ["orchestrator-worker", "\"worker\""] {
                if !code.contains(vocab) {
                    out.push(Offender {
                        file: HANDOFF,
                        line: at,
                        why: format!(
                            "`worker_project_of_any_role` が語彙 {vocab} を解いていない\
                             （表示用 / env 用の片方だけ解ける = #761 の取り違え）"
                        ),
                    });
                }
            }
        }
    }
    match fn_window(orchestrator, "pub fn profile_governing_project(") {
        None => out.push(Offender {
            file: ORCHESTRATOR,
            line: 0,
            why: "`profile_governing_project` が無い（#1466）".into(),
        }),
        Some((at, window)) => {
            let code = code_only(&window);
            // 2 件目に当たったら諦める（「候補のどれか 1 つ」を選ばない）
            if !code.contains("if found.is_some()") || !code.contains("return None") {
                out.push(Offender {
                    file: ORCHESTRATOR,
                    line: at,
                    why: "管轄が複数あるとき 1 つを選んでいる（曖昧なまま配ると\
                          「無関係な master へ届く」が戻る。#1466）"
                        .into(),
                });
            }
        }
    }
    out
}

fn scan_all(sources: &Sources) -> Vec<Offender> {
    let mut out = scan_origin(&sources.dispatch);
    out.extend(scan_origin_profile(&sources.user_tasks));
    out.extend(scan_delivery(&sources.dispatch));
    out.extend(scan_created_by(&sources.dispatch));
    out.extend(scan_vocabulary(&sources.handoff, &sources.orchestrator));
    out
}

// ---------------------------------------------------------------------------
// テスト
// ---------------------------------------------------------------------------

#[test]
fn 現行コードは戻り先の解き方を守っている() {
    let root = workspace_root();
    let offenders = scan_all(&Sources::load(&root));
    assert!(
        offenders.is_empty(),
        "#1466 の構造が崩れている:\n{}",
        offenders
            .iter()
            .map(Offender::report)
            .collect::<Vec<_>>()
            .join("\n")
    );
}

/// 走査が空振りしていれば上のテストは無意味に緑になるので、窓が採れていることを固定する
#[test]
fn 走査が空振りしていない() {
    let root = workspace_root();
    let sources = Sources::load(&root);
    for (label, src, needle) in [
        ("起票元の解決", &sources.dispatch, "fn user_task_origin("),
        (
            "spawn 元の綴り",
            &sources.dispatch,
            "fn spawn_chain_master_profile(",
        ),
        ("配送", &sources.dispatch, "fn deliver_user_task_response("),
        ("名乗り", &sources.dispatch, "fn user_task_created_by("),
        (
            "戻り先の読み",
            &sources.user_tasks,
            "pub fn origin_profile(",
        ),
        (
            "解決の純粋関数",
            &sources.user_tasks,
            "pub fn resolve_origin_profile(",
        ),
        (
            "role の語彙",
            &sources.handoff,
            "pub fn worker_project_of_any_role(",
        ),
        (
            "管轄の引き",
            &sources.orchestrator,
            "pub fn profile_governing_project(",
        ),
    ] {
        let (at, window) = fn_window(src, needle)
            .unwrap_or_else(|| panic!("{label}（{needle}）の窓が採れない = 走査が壊れている"));
        assert!(at > 0, "{label} の行番号が 0");
        assert!(
            window.lines().count() > 4,
            "{label} の窓が {} 行しかない（走査が壊れている）",
            window.lines().count()
        );
    }
}

/// **修正前を再現した注入**が `file:line` で名指しされることを確かめる。
/// 検出力の実証がここ（緑を作るのは簡単だが、落とせることの証明は注入でしかできない）
#[test]
fn 逆戻りを名指しできる() {
    let root = workspace_root();
    assert!(
        scan_all(&Sources::load(&root)).is_empty(),
        "注入前が既に汚れている"
    );

    // (1) spawn 元を見ない旧経路へ戻す（= #1466 の真因そのもの）
    let mut s = Sources::load(&root);
    s.dispatch = s.dispatch.replace(
        "    let spawn_chain = caller.and_then(|pane| spawn_chain_master_profile(host.workspace(), pane));",
        "    let spawn_chain: Option<String> = None;",
    );
    expect_hit(
        &s,
        DISPATCH,
        "spawn 元（`spawned_by` の枝）を材料に集めていない",
    );

    // (2) 管轄の保険が消える
    let mut s = Sources::load(&root);
    s.dispatch = s.dispatch.replace(
        ".and_then(crate::orchestrator::profile_governing_project);",
        ";",
    );
    expect_hit(&s, DISPATCH, "管轄プロファイルの保険が無い");

    // (3) 解決を dispatch の中で書き直す（判断が 2 か所へ割れる）
    let mut s = Sources::load(&root);
    s.dispatch = s.dispatch.replace(
        "    let resolved = crate::user_tasks::resolve_origin_profile(",
        "    let resolved = inline_resolve(",
    );
    expect_hit(
        &s,
        DISPATCH,
        "`user_tasks::resolve_origin_profile` を通っていない",
    );

    // (4) 解けなかった起票を `default` へ落とす
    let mut s = Sources::load(&root);
    let (_, window) = fn_window(&s.dispatch, "fn user_task_origin(").expect("窓");
    s.dispatch = s.dispatch.replace(
        &window,
        &window.replace(
            "        profile: resolved.as_ref().map(|(p, _)| p.clone()),",
            "        profile: Some(tako_core::handoff::DEFAULT_PROFILE.to_string()),",
        ),
    );
    expect_hit(&s, DISPATCH, "既定プロファイルを名乗っている");

    // (5) 配送が宛先不明を握りつぶす（誰かへ届く）
    let mut s = Sources::load(&root);
    s.dispatch = s.dispatch.replace(
        "        return crate::user_tasks::unresolved_delivery(index);",
        "        return delivery_record(DeliveryState::Failed, \"default\", None, None, None, index);",
    );
    expect_hit(&s, DISPATCH, "`unresolved_delivery` を返していない");

    // (6) 名乗りを戻り先から作る（worker が master を騙る）
    let mut s = Sources::load(&root);
    let (_, window) = fn_window(&s.dispatch, "fn user_task_created_by(").expect("窓");
    s.dispatch = s.dispatch.replace(
        &window,
        &window.replace(
            "    let self_role = params",
            "    let _ = &origin.profile;\n    let self_role = params",
        ),
    );
    expect_hit(&s, DISPATCH, "名乗りを `origin.profile`");

    // (7) `origin_profile` が `String` を返す（解けていないものを既定で埋める）
    let mut s = Sources::load(&root);
    s.user_tasks = s.user_tasks.replace(
        "pub fn origin_profile(task: &UserTask) -> Option<String> {",
        "pub fn origin_profile(task: &UserTask) -> String {",
    );
    expect_hit(&s, USER_TASKS, "`Option` を返していない");

    // (8) 管轄が曖昧でも 1 つ選ぶ
    let mut s = Sources::load(&root);
    s.orchestrator = s.orchestrator.replace(
        "        if found.is_some() {",
        "        if false && found.is_some() {",
    );
    expect_hit(&s, ORCHESTRATOR, "管轄が複数あるとき 1 つを選んでいる");
}

/// 注入した材料で走査し、**その file:line と理由**が名指しされることを確かめる
fn expect_hit(sources: &Sources, file: &str, fragment: &str) {
    let offenders = scan_all(sources);
    let report = offenders
        .iter()
        .map(Offender::report)
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        offenders
            .iter()
            .any(|o| o.file == file && o.why.contains(fragment)),
        "注入したのに名指しされない（{file} / {fragment:?}）。実際の検出:\n{report}"
    );
    assert!(
        offenders
            .iter()
            .filter(|o| o.file == file && o.why.contains(fragment))
            .all(|o| o.line > 0),
        "行番号が 0 のまま名指ししている（file:line で追えない）:\n{report}"
    );
}

/// A/B の逃げ道は **Issue ごとに別の env**（#1422 の規約）。
/// 同じ env を読むと、片方のアームがもう片方の回帰を隠す
#[test]
fn abの逃げ道は1466専用のenvを読む() {
    let root = workspace_root();
    let src = prod(&root, USER_TASKS);
    let (at, window) = fn_window(&src, "pub fn legacy_origin_resolution(")
        .expect("`legacy_origin_resolution` が無い");
    assert!(
        window.contains(LEGACY_ENV),
        "{USER_TASKS}:{at} #1466 の A/B（`{LEGACY_ENV}`）が無い。\
         同一バイナリで「worker の起票が default へ落ちる」を再現できないと、\
         戻り先の検出力を実証できない"
    );
    for other in [
        "TAKO_1450_LEGACY",
        "TAKO_1453_LEGACY",
        "TAKO_1445_LEGACY",
        "TAKO_944_LEGACY",
    ] {
        assert!(
            !window.contains(other),
            "{USER_TASKS}:{at} #1466 のアームが {other} を読んでいる（軸が混ざる。#1422）"
        );
    }
    // legacy アームを名乗る行は、その env を読む 1 実装だけを通る
    for (i, line) in src.lines().enumerate() {
        if line.trim_start().starts_with("//") {
            continue;
        }
        if line.contains(LEGACY_ENV) && !line.contains("var_os") {
            panic!(
                "{USER_TASKS}:{} が `{LEGACY_ENV}` を直に読んでいる\
                 （逃げ道は `legacy_origin_resolution` の 1 実装だけ）",
                i + 1
            );
        }
    }
}
