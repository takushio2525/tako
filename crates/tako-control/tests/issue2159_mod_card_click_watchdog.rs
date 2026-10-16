//! #2159 の番犬: tako mod が会話の行に描いたコマンドカードをクリック 1 回で押す
//!
//! # なぜ要るか
//!
//! 会話の行の `[ 実行 ]` は、帯のホットキー（#1963）と**同じ押下の経路**（`pressCard` = tako が組んだ
//! `run_args` で `tako show-command --run`）を通るから「新しいペインと実行記録が GPUI のカードと同じ」になる。
//! ボタンの `onPress` が別の経路（mod の中で引数やシェルの文字列を組む）を作れば、記録が付かない・手元の
//! ペインへ書くといった食い違いが `claude plugin test` の「押したら CLI が呼ばれた」だけでは見えない。
//! 二重押しの番（`runBusy`）も、読みと書きの間に `await` を挟めばダブルクリックの 2 打目がすり抜ける
//! （単体テストの `Promise.all` は実行環境の並べ方次第で直列になりうる）。押せないボタン（tako がクリックを
//! 渡さない・main screen）を見せない条件も、片方だけ外れると「押しても何も起きないボタン」が黙って出る。
//! ここは**構造**を縛って file:line で名指す。
//!
//! # 何を縛るか
//!
//! 1. `drawCardRow`: ボタンを置く条件は `view.cards?.click === true`（tako がクリックを渡す）∧ 端末 ∧
//!    `e.viewport?.isFullscreen === true`。`[ 実行 ]` / `[ コピー ]` の `onPress` は `pressCard($, 'run' / 'copy', …)`
//!    だけ（帯と同じ経路）。key はカード id と番号で固定（描き直しで mount し直さない）
//! 2. `pressCard`: Run の番は CLI を叩く前に `Number.POSITIVE_INFINITY` で取り、読み（`runBusy.get`）と書きの
//!    間に `await` を挟まない。走ったら `RUN_REPEAT_MS` まで捨て、断られたら番を外す
//! 3. tako 側: `card_click` は `mouse_forward_status()` の `enabled` と #1961 の A/B（`legacy`）と #2159 の
//!    A/B（`core::card_click_legacy()`）を見て、応答の `view.cards`（`band_view`）へ渡る
//! 4. A/B の env（`TAKO_2159_LEGACY`）の名前を文字列で読むのは `tako_core::claude_mod` だけ
//!
//! # 見逃す側へ倒れないための作り
//!
//! [`走査が空振りしていない`] で窓が採れていることを固定し、[`逆戻りを名指しできる`] で
//! **現行ソースから作り直した注入**が file:line で名指しされることを確かめる。

use std::path::{Path, PathBuf};

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("crates/tako-control の 2 つ上がワークスペースルート")
        .to_path_buf()
}

const REGISTER: &str = "crates/tako-core/claude-mod/hooks/register.ts";
const CONTROL: &str = "crates/tako-control/src/claude_mod.rs";
/// env の名前を直に読んではいけない側（tako-core の外のソース）
const SOURCE_DIRS: &[&str] = &[
    "crates/tako-control/src",
    "crates/tako-app/src",
    "crates/tako-cli/src",
];
const CLICK_ENV: &str = "\"TAKO_2159_LEGACY\"";
const CORE: &str = "crates/tako-core/src/claude_mod.rs";

fn read(rel: &str) -> String {
    std::fs::read_to_string(workspace_root().join(rel))
        .unwrap_or_else(|e| panic!("{rel} が読める: {e}"))
}

fn report(file: &str, line: usize, why: &str) -> String {
    format!("{file}:{line}: {why}")
}

/// `needle` を含む行から、`close` で始まる最初の行までの窓（1 始まりの行番号つき）
fn window(src: &str, needle: &str, close: &str) -> Option<(usize, String)> {
    let lines: Vec<&str> = src.lines().collect();
    let start = lines.iter().position(|l| l.contains(needle))?;
    let end = lines[start + 1..]
        .iter()
        .position(|l| l.starts_with(close))
        .map(|i| start + 1 + i)?;
    Some((start + 1, lines[start..=end].join("\n")))
}

/// 窓の中で `needle` を含む最初の行の番号（無ければ窓の先頭）
fn line_of(window: &(usize, String), needle: &str) -> usize {
    window
        .1
        .lines()
        .position(|l| l.contains(needle))
        .map_or(window.0, |i| window.0 + i)
}

/// 窓に `need` がすべてあるか（無いものを名指す）
fn needs(out: &mut Vec<String>, file: &str, w: &(usize, String), list: &[&str], why: &str) {
    for need in list {
        if !w.1.contains(need) {
            out.push(report(file, w.0, &format!("`{need}` が無い（{why}）")));
        }
    }
}

struct Sources {
    register: String,
    control: String,
    /// [`SOURCE_DIRS`] の .rs（相対パス, 中身）
    others: Vec<(String, String)>,
}

fn rs_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            rs_files(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

fn current() -> Sources {
    let root = workspace_root();
    let mut others = Vec::new();
    for dir in SOURCE_DIRS {
        let mut files = Vec::new();
        rs_files(&root.join(dir), &mut files);
        files.sort();
        for f in files {
            let rel = f
                .strip_prefix(&root)
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/");
            others.push((rel.clone(), read(&rel)));
        }
    }
    Sources {
        register: read(REGISTER),
        control: read(CONTROL),
        others,
    }
}

/// 1. 会話の行のボタン
fn scan_row(register: &str) -> Vec<String> {
    let mut out = Vec::new();
    let Some(w) = window(register, "function drawCardRow(", "}") else {
        out.push(report(
            REGISTER,
            1,
            "drawCardRow が見つからない（走査が空振り）",
        ));
        return out;
    };
    needs(
        &mut out,
        REGISTER,
        &w,
        &[
            "view.cards?.click === true",
            "e.surface === 'terminal'",
            "e.viewport?.isFullscreen === true",
            "if (clickable && card !== undefined",
        ],
        "ボタンは tako がクリックを渡す全画面の端末だけに置く（押せないボタンを見せない = §9.10）",
    );
    needs(
        &mut out,
        REGISTER,
        &w,
        &[
            "onPress: press => pressCard($, 'run', sel, press)",
            "onPress: press => pressCard($, 'copy', sel, press)",
            "key: `tako-card-${card.id}-${i + 1}-run`",
            "key: `tako-card-${card.id}-${i + 1}-copy`",
        ],
        "押下は帯と同じ pressCard（tako が組んだ run_args = GPUI のカードと同じ dispatch の run）・key はカード id と番号で固定",
    );
    // ボタンの押下に別の経路を作らない（CLI を直に叩く・シェルの文字列を組む）
    for bad in ["$.process.run(", "'sh'", "'-c'", ".join(' ')"] {
        if w.1.contains(bad) {
            out.push(report(
                REGISTER,
                line_of(&w, bad),
                &format!("drawCardRow が `{bad}` を使っている（押下は pressCard の 1 本 = 帯と同じ経路）"),
            ));
        }
    }
    out
}

/// 2. 二重押しの番
fn scan_guard(register: &str) -> Vec<String> {
    let mut out = Vec::new();
    let Some(w) = window(register, "async function pressCard(", "}") else {
        out.push(report(
            REGISTER,
            1,
            "pressCard が見つからない（走査が空振り）",
        ));
        return out;
    };
    needs(
        &mut out,
        REGISTER,
        &w,
        &[
            "if ((runBusy.get(runKey) ?? 0) > now) return",
            "runBusy.set(runKey, Number.POSITIVE_INFINITY)",
            "if (ok) runBusy.set(runKey, at + RUN_REPEAT_MS)",
            "else runBusy.delete(runKey)",
        ],
        "Run は CLI を叩く前に番を取り、走ったら RUN_REPEAT_MS まで捨て、断られたら外す（ダブルクリックで 2 本走らせない）",
    );
    // 読みと書きの間に await を挟まない（挟むと同時に届いた 2 打目が両方とも空の番を読む）
    let lines: Vec<&str> = w.1.lines().collect();
    let get = lines.iter().position(|l| l.contains("runBusy.get(runKey)"));
    let set = lines
        .iter()
        .position(|l| l.contains("runBusy.set(runKey, Number.POSITIVE_INFINITY)"));
    if let (Some(get), Some(set)) = (get, set) {
        if set < get {
            out.push(report(
                REGISTER,
                w.0 + set,
                "Run の番を読む前に書いている（2 打目の判定ができない）",
            ));
        }
        for (i, line) in lines.iter().enumerate().take(set).skip(get + 1) {
            if line
                .split("//")
                .next()
                .unwrap_or_default()
                .contains("await ")
            {
                out.push(report(
                    REGISTER,
                    w.0 + i,
                    "Run の番の読みと書きの間に await がある（同時に届いたダブルクリックの 2 打目がすり抜ける）",
                ));
            }
        }
    }
    let process = lines
        .iter()
        .position(|l| l.contains("$.process.run([cli, ...argv]"));
    if let (Some(set), Some(process)) = (set, process) {
        if process < set {
            out.push(report(
                REGISTER,
                w.0 + process,
                "Run の番を取る前に CLI を叩いている",
            ));
        }
    }
    out
}

/// 3 / 4. tako 側の判断と A/B の env
fn scan_control(s: &Sources) -> Vec<String> {
    let mut out = Vec::new();
    match window(&s.control, "pub fn card_click(", "}") {
        Some(w) => needs(
            &mut out,
            CONTROL,
            &w,
            &[
                "host.mouse_forward_status()",
                "mouse.enabled",
                "!mouse.legacy",
                "!core::card_click_legacy()",
            ],
            "クリックを渡すとき（tako mouse on ∧ #1961 / #2159 の A/B でない）だけ会話の行にボタンを置かせる",
        ),
        None => out.push(report(
            CONTROL,
            1,
            "card_click が見つからない（走査が空振り）",
        )),
    }
    match window(&s.control, "fn band_view(", "}") {
        Some(w) => needs(
            &mut out,
            CONTROL,
            &w,
            &["card_click: card_click(host)"],
            "応答の view.cards.click は card_click の 1 本から",
        ),
        None => out.push(report(
            CONTROL,
            1,
            "band_view が見つからない（走査が空振り）",
        )),
    }
    for (file, src) in &s.others {
        for (i, line) in src.lines().enumerate() {
            let code = line.split("//").next().unwrap_or_default();
            if code.contains(CLICK_ENV) {
                out.push(report(
                    file,
                    i + 1,
                    &format!("{CLICK_ENV} を {CORE} の外で読んでいる（A/B の env の名前は 1 か所 = card_click_legacy を通す）"),
                ));
            }
        }
    }
    out
}

fn all(s: &Sources) -> Vec<String> {
    let mut out = scan_row(&s.register);
    out.extend(scan_guard(&s.register));
    out.extend(scan_control(s));
    out
}

#[test]
fn 会話の行のカードの押下と二重押しの番が契約どおり() {
    let found = all(&current());
    assert!(
        found.is_empty(),
        "契約から外れた箇所:\n{}",
        found.join("\n")
    );
}

#[test]
fn 走査が空振りしていない() {
    let s = current();
    let (_, w) = window(&s.register, "function drawCardRow(", "}").unwrap();
    assert!(
        w.contains("Code({ source: command.text") && w.contains("borderStyle: 'round'"),
        "drawCardRow の窓が短すぎる: {w}"
    );
    let (_, w) = window(&s.register, "async function pressCard(", "}").unwrap();
    assert!(
        w.contains("$.ui.copy(") && w.contains("dirty = true"),
        "pressCard の窓が短すぎる: {w}"
    );
    let (_, w) = window(&s.control, "pub fn card_click(", "}").unwrap();
    assert!(w.contains("mouse_forward_status"), "{w}");
    assert!(
        s.others.len() > 50,
        "走査した .rs が {} 本しかない",
        s.others.len()
    );
}

fn assert_named(found: &[String], file: &str, why: &str) {
    assert!(
        found.iter().any(|f| f.starts_with(file) && f.contains(why)),
        "注入を名指しできない（{file} / {why}）:\n{}",
        found.join("\n")
    );
}

/// 注入が現行ソースに当たったか（当たらなければ注入そのものが古い = 見逃す側へ倒れる）
fn replace_once(src: &mut String, from: &str, to: &str) {
    assert!(
        src.contains(from),
        "注入の元の字面が現行ソースに無い: {from}"
    );
    *src = src.replacen(from, to, 1);
}

#[test]
fn 逆戻りを名指しできる() {
    let mutate = |f: &dyn Fn(&mut Sources)| {
        let mut s = current();
        f(&mut s);
        all(&s)
    };
    // 1. tako がクリックを渡さないペイン・main screen にも押せないボタンを置く
    let found = mutate(&|s| {
        replace_once(&mut s.register, "view.cards?.click === true && ", "");
    });
    assert_named(&found, REGISTER, "view.cards?.click === true");
    let found = mutate(&|s| {
        replace_once(&mut s.register, " && e.viewport?.isFullscreen === true", "");
    });
    assert_named(&found, REGISTER, "e.viewport?.isFullscreen === true");
    // 1b. 会話の行の Run が帯と別の経路（mod の中で CLI の引数を組む）
    let found = mutate(&|s| {
        replace_once(
            &mut s.register,
            "onPress: press => pressCard($, 'run', sel, press)",
            "onPress: () => void $.process.run([cli ?? '', 'show-command', '--run', '--card', String(card.id)])",
        );
    });
    assert_named(&found, REGISTER, "pressCard($, 'run', sel, press)");
    assert_named(&found, REGISTER, "`$.process.run(`");
    // 1c. key が描くたびに変わる（mount し直す = 描いた直後 300 ms のクリックを捨てる窓を作り直す）
    let found = mutate(&|s| {
        replace_once(
            &mut s.register,
            "key: `tako-card-${card.id}-${i + 1}-run`",
            "key: `tako-card-${card.id}-${i + 1}-run-${Date.now()}`",
        );
    });
    assert_named(&found, REGISTER, "key: `tako-card-${card.id}-${i + 1}-run`");
    // 2. 二重押しの番を外す・読みと書きの間に await を挟む
    let found = mutate(&|s| {
        replace_once(
            &mut s.register,
            "    if ((runBusy.get(runKey) ?? 0) > now) return\n",
            "",
        );
    });
    assert_named(
        &found,
        REGISTER,
        "if ((runBusy.get(runKey) ?? 0) > now) return",
    );
    let found = mutate(&|s| {
        replace_once(
            &mut s.register,
            "    for (const [key, until] of runBusy) if (until <= now) runBusy.delete(key)\n",
            "    await $.clock.now()\n",
        );
    });
    assert_named(&found, REGISTER, "読みと書きの間に await がある");
    let found = mutate(&|s| {
        replace_once(
            &mut s.register,
            "    if (ok) runBusy.set(runKey, at + RUN_REPEAT_MS)\n",
            "    if (ok) runBusy.delete(runKey)\n",
        );
    });
    assert_named(&found, REGISTER, "RUN_REPEAT_MS");
    // 3. tako mouse off でもボタンを置かせる・#1961 の A/B を見ない
    let found = mutate(&|s| {
        replace_once(&mut s.control, "    mouse.enabled && ", "    ");
    });
    assert_named(&found, CONTROL, "`mouse.enabled`");
    let found = mutate(&|s| {
        replace_once(
            &mut s.control,
            "        card_click: card_click(host),",
            "        card_click: true,",
        );
    });
    assert_named(&found, CONTROL, "card_click: card_click(host)");
    // 4. env を tako-core の外で読む
    let found = mutate(&|s| {
        let (_, src) = s
            .others
            .iter_mut()
            .find(|(f, _)| f == CONTROL)
            .expect("CONTROL は走査の対象");
        src.push_str("\nfn leak() -> bool { std::env::var(\"TAKO_2159_LEGACY\").is_ok() }\n");
    });
    assert_named(&found, CONTROL, "TAKO_2159_LEGACY");
}
