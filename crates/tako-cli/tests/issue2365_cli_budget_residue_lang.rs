//! 実経路: `tako context-budget`（check / fix）と `tako test-residue` の人向けの表示が表示言語に従う
//! （Issue #2365 の続き）。
//!
//! 修正前（実測）: 英語表示でも context-budget は「起動時ロード: N 項目 / 概算 N トークン」「超過 entries: …（…）」
//! 「予算内」「提案 …」「（dry-run: 書き込まない）」「…: 25 件 → 残 20 件 / archive へ 5 件（総数保存: はい）」
//! 「移送予定 …」「変更したファイル: N」、test-residue は「一時ディレクトリ: …」「残骸: N 件 / …」
//! 「N 件 / …（消せる N）  <種別の説明>」「見送り N 件: <判定の理由>」「消せる: …（まだ 1 つも消していない）」
//! 「いま消す: …」のように、枠と種別の説明・判定の理由が日本語の固定文のまま出た（`main.rs` の
//! `print_context_budget_*` / `test_residue_local` の直書きと、tako-core の `KINDS[].note` / `Verdict::detail`）。
//! context-budget の提案の `advice`（system prompt の追記を何行動かせば収まるか）は、項目の側は表示言語で
//! 選んでいたのに提案の側だけ日本語に固定だった。system prompt の項目の名前は英語表示でも全角の括弧だった。
//!
//! **ビルドした `tako` をそのまま**叩く（どちらも GUI の要らない手元の処理）。見ること:
//! - 英語表示: 出力に日本語（かな・漢字・全角の記号）が 1 字も無い
//! - 日本語表示: #2365 より前の出力とバイト一致（数字だけ `0` に均す。`<SB>` は砂場の dir）
//! - 行の数と字下げは日英で同じ・終了コードは日英で同じ
//! - `--json`: test-residue は日英で同じ（`note` / `detail` は前のまま日本語）。context-budget の提案の
//!   `advice` は表示言語の側（`advice_ja` / `advice_en`）と同じ
//!
//! A/B: `TAKO_2365_HUMAN_BIN=<修正前の tako>` で別のバイナリを叩ける（修正前は英語表示の検査で落ちる）。
//! `I2365B_DUMP=<dir>` を付けると全ケースの出力を書き出す（修正前後の `diff -r` 用）。
//! 本番の data dir・GUI へ届かないよう、`TAKO_*` を全部外して一時の置き場だけを渡す。
//!
//! unix だけで走らせる（残骸の判定に使う pid の生死・権限で消せない dir の作り方が unix の形）。
//! 文の日英の対は OS に依らない 1 か所（`ReplyNote` / tako-core の `Kind::note` / `Verdict::detail_in`）で、
//! 両 OS の番犬（`crates/tako-control/tests/issue1762_dispatch_error_lang.rs`）が表で見る
#![cfg(unix)]

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::OnceLock;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::Value;
use tako_core::test_residue::ScratchDir;

/// 叩くバイナリ（既定はこの木でビルドした `tako`。A/B は `TAKO_2365_HUMAN_BIN`）
fn tako() -> PathBuf {
    std::env::var_os("TAKO_2365_HUMAN_BIN")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_BIN_EXE_tako")))
}

/// 生きていない pid（どの OS の pid の上限より大きく、`i32` には収まる = 判定は「居ない」）
const DEAD_PID: u32 = 2_000_000_000;

/// 1 ケース分の砂場: data（表示言語・プロファイル）・発見の置き場・ホーム `hm`（作業 dir `w` もこの下）・
/// 一時ディレクトリ（test-residue が見る `TMPDIR`）。ホームの dir を `home` と名付けないのは、その下の `w` が
/// 個人情報の番犬（`no_personal_data`）にホームパスのユーザー名と読まれるため。パスの表示は砂場の実体の
/// パス（40 字を超えるので `context-budget` の桁揃えの空白は入らない）を `<SB>` に置換したもの
struct Sandbox {
    dir: ScratchDir,
    real: PathBuf,
}

impl Sandbox {
    fn new(lang: &str) -> Self {
        let dir = ScratchDir::new("i2365b");
        for d in ["data/orchestrator/profiles", "disc", "hm/w/.agent", "tmp"] {
            let d = dir.path().join(d);
            std::fs::create_dir_all(&d).unwrap_or_else(|e| panic!("{} を作れる: {e}", d.display()));
        }
        // 書かないと system = 実行環境のロケールで決まり、CI と手元で答えが変わる
        std::fs::write(
            dir.path().join("data/settings.json"),
            format!("{{\"language\":\"{lang}\"}}"),
        )
        .expect("settings.json を書ける");
        let real = std::fs::canonicalize(dir.path()).expect("砂場の実体を引ける");
        Self { dir, real }
    }

    fn path(&self) -> &Path {
        self.dir.path()
    }

    /// 砂場の中へファイルを書く（親 dir も作る）
    fn put(&self, rel: &str, text: &str) {
        let p = self.path().join(rel);
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent).expect("親 dir を作れる");
        }
        std::fs::write(&p, text).unwrap_or_else(|e| panic!("{} を書ける: {e}", p.display()));
    }

    /// `TAKO_*` を外して隔離の置き場だけを渡した `tako <args>`（cwd は `hm/w`。`<SB>` に置換）
    fn run(&self, args: &[&str]) -> Out {
        let mut command = Command::new(tako());
        command
            .args(args)
            .current_dir(self.path().join("hm/w"))
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        for (key, _) in std::env::vars_os() {
            let key = key.to_string_lossy().to_string();
            if key.starts_with("TAKO_") || key.starts_with("CLAUDE") || key == "CODEX_HOME" {
                command.env_remove(key);
            }
        }
        command
            .env("TAKO_ISOLATED", "1")
            .env("TAKO_DATA_DIR", self.path().join("data"))
            .env("TAKO_DISCOVERY_DIR", self.path().join("disc"))
            .env("HOME", self.path().join("hm"))
            .env("TMPDIR", self.path().join("tmp"));
        let out = command.output().expect("tako が終わる");
        let real = self.real.display().to_string();
        let root = self.path().display().to_string();
        let norm = |b: &[u8]| {
            String::from_utf8_lossy(b)
                .replace(&real, "<SB>")
                .replace(&root, "<SB>")
        };
        Out {
            code: out.status.code().unwrap_or(-1),
            stdout: norm(&out.stdout),
            stderr: norm(&out.stderr),
        }
    }
}

impl Drop for Sandbox {
    /// 消せない dir（`r_apply_fail` の読み取り専用の dir）を書ける形へ戻してから `ScratchDir` に消させる
    fn drop(&mut self) {
        let mut stack = vec![self.path().to_path_buf()];
        while let Some(dir) = stack.pop() {
            let _ = std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755));
            for entry in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
                if entry.file_type().is_ok_and(|t| t.is_dir()) {
                    stack.push(entry.path());
                }
            }
        }
    }
}

struct Out {
    code: i32,
    stdout: String,
    stderr: String,
}

/// pid の再利用（`reused_pid`）の材料: 先に作って 3 秒置いた dir と、その後に起こした子のプロセス。
/// 子の pid を名前に持たせて dir を砂場の一時ディレクトリへ移す（移しても作成時刻は変わらない）。
/// 作成時刻より後に始まった生きているプロセス = 判定は「pid の再利用」
struct Reused {
    pool: ScratchDir,
    next: AtomicUsize,
    child: Child,
}

impl Reused {
    fn new(count: usize) -> Self {
        let pool = ScratchDir::new("i2365p");
        for i in 0..count {
            std::fs::create_dir_all(pool.path().join(format!("pre{i}"))).expect("dir を作れる");
        }
        std::thread::sleep(
            tako_core::test_residue::REUSE_SLACK + std::time::Duration::from_secs(1),
        );
        let child = Command::new("sleep")
            .arg("120")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("sleep を起こせる");
        Self {
            pool,
            next: AtomicUsize::new(0),
            child,
        }
    }

    /// 次の dir を `<一時ディレクトリ>/<prefix><子の pid>` へ移す
    fn place(&self, sb: &Sandbox, prefix: &str) {
        let i = self.next.fetch_add(1, Ordering::Relaxed);
        let to = sb
            .path()
            .join("tmp")
            .join(format!("{prefix}{}", self.child.id()));
        std::fs::rename(self.pool.path().join(format!("pre{i}")), &to)
            .unwrap_or_else(|e| panic!("pre{i} を移せる（用意した数が足りない?）: {e}"));
    }
}

impl Drop for Reused {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// 1 ケース: 名前・引数・砂場の用意・`--json` か
struct Case {
    name: &'static str,
    args: &'static [&'static str],
    setup: fn(&Sandbox, &Reused),
    json: bool,
}

const fn case(
    name: &'static str,
    args: &'static [&'static str],
    setup: fn(&Sandbox, &Reused),
) -> Case {
    Case {
        name,
        args,
        setup,
        json: false,
    }
}

const fn json_case(
    name: &'static str,
    args: &'static [&'static str],
    setup: fn(&Sandbox, &Reused),
) -> Case {
    Case {
        name,
        args,
        setup,
        json: true,
    }
}

/// 今日から `ago` 日前の `YYYY-MM-DD`（保持期間の判定は今日からの日数なので、日付は実行日から組む）
fn day(ago: i64) -> String {
    let today = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64 / 86_400)
        .unwrap_or(0);
    tako_core::context_budget::days_to_date(today - ago)
}

/// `count` 件の作業ログ（1 件 1 日・新しいものほど下）と、それを読み込む規約
fn progress(sb: &Sandbox, count: i64) {
    let mut text = String::from("# Progress\n");
    for n in 1..=count {
        // 見出しの中身は利用者の文なので ASCII にしておく（英語表示の検査は枠だけを見たい）
        text.push_str(&format!(
            "\n## {} (#{n} entry {n})\n- line {n}\n",
            day(count + 1 - n)
        ));
    }
    sb.put("hm/w/.agent/progress.md", &text);
    sb.put("hm/w/AGENTS.md", "# AGENTS\n\n@.agent/progress.md\n");
}

fn nothing(_: &Sandbox, _: &Reused) {}

fn budget_small(sb: &Sandbox, _: &Reused) {
    progress(sb, 3);
}

fn budget_over(sb: &Sandbox, _: &Reused) {
    progress(sb, 25);
}

/// アーカイブへ移したはずの 1 件が作業ログに戻っている（rebase の後の形。`fix` が作業ログ側を落とす）
fn budget_revived(sb: &Sandbox, _: &Reused) {
    progress(sb, 3);
    sb.put(
        "hm/w/.agent/progress-archive.md",
        &format!("# Archive\n\n- {} #1 entry 1\n", day(3)),
    );
}

/// アーカイブに保持期間（90 日）を過ぎた行がある
fn budget_aged(sb: &Sandbox, _: &Reused) {
    progress(sb, 3);
    sb.put(
        "hm/w/.agent/progress-archive.md",
        &format!(
            "# Archive\n\n- {} #90 old\n- {} #91 recent\n",
            day(200),
            day(30)
        ),
    );
}

/// 個人の追記が育って system prompt が予算を超えたプロファイル（`advice` が出る）
fn budget_fat(sb: &Sandbox, _: &Reused) {
    progress(sb, 3);
    let rules: String =
        std::iter::repeat_n("- local rule line for this machine only.\n", 400).collect();
    sb.put("hm/local-rules.md", &rules);
    sb.put(
        "data/orchestrator/profiles/fat.yaml",
        "model: claude-fable-5-1\neffort: high\nprompt_blocks:\n  append: ~/local-rules.md\n",
    );
}

/// 残骸の判定が一通り: 消せる（所有者が居ない）・使用中（このテストのプロセス）・判定できない（pid 0）・
/// pid の再利用・種別の形でない名前（数えない）
fn residue_mixed(sb: &Sandbox, reused: &Reused) {
    sb.put(&format!("tmp/tako-test-data-{DEAD_PID}/x/f"), "abc");
    sb.put(
        &format!("tmp/tako-agent-config-{}/history", std::process::id()),
        "ab",
    );
    std::fs::create_dir_all(sb.path().join("tmp/tako-test-scratch-0")).expect("dir を作れる");
    std::fs::create_dir_all(sb.path().join("tmp/tako-test-data-keep")).expect("dir を作れる");
    reused.place(sb, "tako-test-orchestrator-");
}

/// 消せる残骸のうち 1 つが消せない（中に書き込めない dir がある = 失敗の理由が出る）
fn residue_fail(sb: &Sandbox, _: &Reused) {
    sb.put(&format!("tmp/tako-test-data-{DEAD_PID}/x/f"), "abc");
    let locked = sb
        .path()
        .join(format!("tmp/tako-test-supervisor-{}/ro", DEAD_PID + 1));
    std::fs::create_dir_all(&locked).expect("dir を作れる");
    std::fs::write(locked.join("f"), "z").expect("書ける");
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o555))
        .expect("権限を変えられる");
}

fn cases() -> Vec<Case> {
    vec![
        case("b_check_ok", &["context-budget"], budget_small),
        case("b_check_over", &["context-budget"], budget_over),
        case("b_check_revived", &["context-budget"], budget_revived),
        case(
            "b_check_fat",
            &["context-budget", "--profile", "fat"],
            budget_fat,
        ),
        case("b_fix_noop", &["context-budget", "fix"], budget_small),
        case(
            "b_fix_dry",
            &["context-budget", "fix", "--dry-run"],
            budget_over,
        ),
        case("b_fix", &["context-budget", "fix"], budget_over),
        case("b_fix_revived", &["context-budget", "fix"], budget_revived),
        case("b_fix_aged", &["context-budget", "fix"], budget_aged),
        case(
            "b_fix_fat",
            &["context-budget", "fix", "--profile", "fat"],
            budget_fat,
        ),
        json_case(
            "b_check_fat_json",
            &["context-budget", "--profile", "fat", "--json"],
            budget_fat,
        ),
        json_case(
            "b_fix_dry_json",
            &["context-budget", "fix", "--dry-run", "--json"],
            budget_over,
        ),
        case("r_empty", &["test-residue"], nothing),
        case("r_dry", &["test-residue"], residue_mixed),
        case("r_apply", &["test-residue", "--apply"], residue_mixed),
        case("r_apply_fail", &["test-residue", "--apply"], residue_fail),
        json_case("r_dry_json", &["test-residue", "--json"], residue_mixed),
        json_case(
            "r_apply_fail_json",
            &["test-residue", "--apply", "--json"],
            residue_fail,
        ),
    ]
}

fn has_japanese(s: &str) -> Option<char> {
    s.chars()
        .find(|c| matches!(*c as u32, 0x3000..=0x30FF | 0x4E00..=0x9FFF | 0xFF00..=0xFFEF))
}

/// 数字を `0` に均す（MCP カタログ・system prompt の大きさ・日付は版と実行日で変わる。桁揃えの幅は保つ）
fn digits0(s: &str) -> String {
    s.chars()
        .map(|c| if c.is_ascii_digit() { '0' } else { c })
        .collect()
}

fn run_lang(lang: &str, reused: &Reused) -> Vec<(&'static str, bool, Out)> {
    let dump = std::env::var_os("I2365B_DUMP").map(PathBuf::from);
    cases()
        .into_iter()
        .map(|case| {
            let sb = Sandbox::new(lang);
            (case.setup)(&sb, reused);
            let out = sb.run(case.args);
            if let Some(dir) = &dump {
                let _ = std::fs::create_dir_all(dir);
                let _ = std::fs::write(
                    dir.join(format!("{lang}-{}.txt", case.name)),
                    format!(
                        "exit={}\n--- stdout\n{}--- stderr\n{}",
                        out.code, out.stdout, out.stderr
                    ),
                );
            }
            (case.name, case.json, out)
        })
        .collect()
}

type Runs = (
    Vec<(&'static str, bool, Out)>,
    Vec<(&'static str, bool, Out)>,
);

/// 全ケースを日英で 1 回だけ流す（pid の再利用の材料は 3 秒置くので 1 回で済ませる）
fn runs() -> &'static Runs {
    static RUNS: OnceLock<Runs> = OnceLock::new();
    RUNS.get_or_init(|| {
        // 用意する数 = pid の再利用の dir を置くケースの数 × 2 言語
        let reused = Reused::new(6);
        let ja = run_lang("ja", &reused);
        let en = run_lang("en", &reused);
        drop(reused);
        (ja, en)
    })
}

/// #2365 より前の日本語の出力（修正前のバイナリで採った。数字は `0` に均してある）
fn old_ja() -> Vec<(&'static str, &'static str)> {
    vec![
        (
            "b_check_ok",
            r#"起動時ロード: 0 項目 / 概算 00000 トークン
  <SB>/hm/w/AGENTS.md      00 bytes / 約     00 tok  [agents_guide]
  <SB>/hm/w/.agent/progress.md     000 bytes / 約     00 tok  [progress_log]
  MCP ツールカタログ（000 本）                        000000 bytes / 約  00000 tok  [mcp_catalog]

予算内
"#,
        ),
        (
            "b_check_over",
            r#"起動時ロード: 0 項目 / 概算 00000 トークン
  <SB>/hm/w/AGENTS.md      00 bytes / 約     00 tok  [agents_guide]
! <SB>/hm/w/.agent/progress.md     000 bytes / 約    000 tok  [progress_log]
      超過 entries: 00 > 00（毎ターン全文が読み込まれる作業ログが予算を超えている。`tako context-budget fix` で古いエントリを archive へ 0 行に畳んで移せる（本文は git 履歴に残るので失われない））
      超過 work_days: 00 > 0（毎ターン全文が読み込まれる作業ログが予算を超えている。`tako context-budget fix` で古いエントリを archive へ 0 行に畳んで移せる（本文は git 履歴に残るので失われない））
  MCP ツールカタログ（000 本）                        000000 bytes / 約  00000 tok  [mcp_catalog]

起動時ロードが予算超過: 0 件（概算 00000 トークン）。いま直す: tako context-budget fix（自動で直せる 0 件）
"#,
        ),
        (
            "b_check_revived",
            r#"起動時ロード: 0 項目 / 概算 00000 トークン
  <SB>/hm/w/AGENTS.md      00 bytes / 約     00 tok  [agents_guide]
! <SB>/hm/w/.agent/progress.md     000 bytes / 約     00 tok  [progress_log]
      超過 revived_entries: 0 > 0（アーカイブへ移したはずのエントリが作業ログに戻っている（rebase / merge で移送の削除だけが失われた形）。`tako context-budget fix` が作業ログ側を落とす（アーカイブの 0 行はそのまま・本文は git 履歴に残る））
        <SB>/hm/w/.agent/progress.md:0 アーカイブ済みのエントリが復活: ## 0000-00-00 (#0 entry 0) ← <SB>/hm/w/.agent/progress-archive.md:0 「- 0000-00-00 #0 entry 0」
  MCP ツールカタログ（000 本）                        000000 bytes / 約  00000 tok  [mcp_catalog]

作業ログに rebase で戻ったエントリ: 0 件（<SB>/hm/w/.agent/progress.md:0）。いま直す: tako context-budget fix（自動で直せる 0 件）
"#,
        ),
        (
            "b_check_fat",
            r#"起動時ロード: 0 項目 / 概算 000000 トークン
  <SB>/hm/w/AGENTS.md      00 bytes / 約     00 tok  [agents_guide]
  <SB>/hm/w/.agent/progress.md     000 bytes / 約     00 tok  [progress_log]
! master system prompt（fat）                  00000 bytes / 約  00000 tok  [system_prompt]
      超過 bytes: 00000 > 00000（master / solo の system prompt が予算を超えている。長寿命セッションの起動直後の固定費なので、手順の詳細は `tako orchestrator guide <topic>` で必要なときだけ引く形へ移す。プロファイルの `prompt_blocks.append`（個人環境固有のルール）が大きい場合は、常に要る規則だけを残して残りを別ファイルへ分け、そこは AI が必要なときだけ読む）
  MCP ツールカタログ（000 本）                        000000 bytes / 約  00000 tok  [mcp_catalog]

起動時ロードが予算超過: 0 件（概算 000000 トークン）。master system prompt（fat）: bytes 00000 > 00000。内訳: tako context-budget
  提案 master system prompt（fat）: master / solo の system prompt が予算を超えている。長寿命セッションの起動直後の固定費なので、手順の詳細は `tako orchestrator guide <topic>` で必要なときだけ引く形へ移す。プロファイルの `prompt_blocks.append`（個人環境固有のルール）が大きい場合は、常に要る規則だけを残して残りを別ファイルへ分け、そこは AI が必要なときだけ読む
        append (local-rules.md)              00000 bytes
        behavior                              0000 bytes
        guides                                0000 bytes
        monitoring                            0000 bytes
        task-intake                           0000 bytes
        → 追記に区切りがまだ無い。`tako migrate run`（`tako setup` と起動時にも自動で走る）が見出し境界へ `<!-- tako:on-demand -->` を 0 行入れ、その行より後ろは `tako orchestrator guide local-rules` で引く形になる（内容は 0 文字も消えない）。切れない場合は自分で `<!-- tako:on-demand -->` を 000 行ぶん手前へ置く
"#,
        ),
        (
            "b_fix_noop",
            r#"<SB>/hm/w/.agent/progress.md: 変更なし（予算内）
変更したファイル: 0
"#,
        ),
        (
            "b_fix_dry",
            r#"（dry-run: 書き込まない）
<SB>/hm/w/.agent/progress.md: 00 件 → 残 0 件 / archive へ 00 件（総数保存: はい）
  000 → 000 bytes（約 000 → 000 トークン）
    移送予定 - 0000-00-00 #0 entry 0
    移送予定 - 0000-00-00 #0 entry 0
    移送予定 - 0000-00-00 #0 entry 0
    移送予定 - 0000-00-00 #0 entry 0
    移送予定 - 0000-00-00 #0 entry 0
    移送予定 - 0000-00-00 #0 entry 0
    移送予定 - 0000-00-00 #0 entry 0
    移送予定 - 0000-00-00 #0 entry 0
    移送予定 - 0000-00-00 #0 entry 0
    移送予定 - 0000-00-00 #00 entry 00
    移送予定 - 0000-00-00 #00 entry 00
    移送予定 - 0000-00-00 #00 entry 00
    移送予定 - 0000-00-00 #00 entry 00
    移送予定 - 0000-00-00 #00 entry 00
    移送予定 - 0000-00-00 #00 entry 00
    移送予定 - 0000-00-00 #00 entry 00
    移送予定 - 0000-00-00 #00 entry 00
    移送予定 - 0000-00-00 #00 entry 00
    移送予定 - 0000-00-00 #00 entry 00
    移送予定 - 0000-00-00 #00 entry 00
変更したファイル: 0
"#,
        ),
        (
            "b_fix",
            r#"<SB>/hm/w/.agent/progress.md: 00 件 → 残 0 件 / archive へ 00 件（総数保存: はい）
  000 → 000 bytes（約 000 → 000 トークン）
変更したファイル: 0
"#,
        ),
        (
            "b_fix_revived",
            r#"<SB>/hm/w/.agent/progress.md: 0 件 → 残 0 件 / archive へ 0 件（総数保存: はい）
  000 → 00 bytes（約 00 → 00 トークン）
  作業ログ側を落とす（アーカイブに既に在る = rebase で戻ったエントリ）: 0 件
    <SB>/hm/w/.agent/progress.md:0 アーカイブ済みのエントリが復活: ## 0000-00-00 (#0 entry 0) ← <SB>/hm/w/.agent/progress-archive.md:0 「- 0000-00-00 #0 entry 0」
変更したファイル: 0
"#,
        ),
        (
            "b_fix_aged",
            r#"<SB>/hm/w/.agent/progress.md: 0 件 → 残 0 件 / archive へ 0 件（総数保存: はい）
  000 → 000 bytes（約 00 → 00 トークン）
  保持期間を過ぎて archive から削除: 0 行
変更したファイル: 0
"#,
        ),
        (
            "b_fix_fat",
            r#"<SB>/hm/w/.agent/progress.md: 変更なし（予算内）
変更したファイル: 0
  提案 master system prompt（fat）: master / solo の system prompt が予算を超えている。長寿命セッションの起動直後の固定費なので、手順の詳細は `tako orchestrator guide <topic>` で必要なときだけ引く形へ移す。プロファイルの `prompt_blocks.append`（個人環境固有のルール）が大きい場合は、常に要る規則だけを残して残りを別ファイルへ分け、そこは AI が必要なときだけ読む
        append (local-rules.md)              00000 bytes
        behavior                              0000 bytes
        guides                                0000 bytes
        monitoring                            0000 bytes
        task-intake                           0000 bytes
        → 追記に区切りがまだ無い。`tako migrate run`（`tako setup` と起動時にも自動で走る）が見出し境界へ `<!-- tako:on-demand -->` を 0 行入れ、その行より後ろは `tako orchestrator guide local-rules` で引く形になる（内容は 0 文字も消えない）。切れない場合は自分で `<!-- tako:on-demand -->` を 000 行ぶん手前へ置く
"#,
        ),
        (
            "r_empty",
            r#"一時ディレクトリ: <SB>/tmp
残骸はありません
"#,
        ),
        (
            "r_dry",
            r#"一時ディレクトリ: <SB>/tmp
残骸: 0 件 / 0 B
  tako-test-data-             0 件 /       0 B（消せる 0）  cargo test の data dir（#000 の隔離先）
  tako-agent-config-          0 件 /       0 B（消せる 0）  検証プロセスの外部エージェント設定・シェル履歴（#0000 の隔離先）
  tako-test-scratch-          0 件 /       0 B（消せる 0）  テスト本体が作る使い捨ての作業ディレクトリの親（#0000 の ScratchDir）
  tako-test-orchestrator-     0 件 /       0 B（消せる 0）  cargo test のオーケストレーター設定（tako_control::orchestrator の隔離先）
  見送り 0 件: 所有プロセスが生きている = 使用中
  見送り 0 件: pid は生きているが dir より後に始まった = pid 再利用（見送る）
  見送り 0 件: 所有プロセスの生死を判定できない（見送る）

消せる: 0 件 / 0 B（まだ 0 つも消していない）
いま消す: tako test-residue --apply
"#,
        ),
        (
            "r_apply",
            r#"一時ディレクトリ: <SB>/tmp
残骸: 0 件 / 0 B
  tako-test-data-             0 件 /       0 B（消せる 0）  cargo test の data dir（#000 の隔離先）
  tako-agent-config-          0 件 /       0 B（消せる 0）  検証プロセスの外部エージェント設定・シェル履歴（#0000 の隔離先）
  tako-test-scratch-          0 件 /       0 B（消せる 0）  テスト本体が作る使い捨ての作業ディレクトリの親（#0000 の ScratchDir）
  tako-test-orchestrator-     0 件 /       0 B（消せる 0）  cargo test のオーケストレーター設定（tako_control::orchestrator の隔離先）
  見送り 0 件: 所有プロセスが生きている = 使用中
  見送り 0 件: pid は生きているが dir より後に始まった = pid 再利用（見送る）
  見送り 0 件: 所有プロセスの生死を判定できない（見送る）

消した: 0 件 / 0 B（失敗 0）
"#,
        ),
        (
            "r_apply_fail",
            r#"一時ディレクトリ: <SB>/tmp
残骸: 0 件 / 0 B
  tako-test-data-             0 件 /       0 B（消せる 0）  cargo test の data dir（#000 の隔離先）
  tako-test-supervisor-       0 件 /       0 B（消せる 0）  cargo test の supervisor 監査ログ（tako_control::orchestrator の隔離先）

消した: 0 件 / 0 B（失敗 0: Permission denied (os error 00)）
"#,
        ),
    ]
}

#[test]
fn context_budgetとtest_residueの人向けの表示は表示言語に従い日本語は2365より前と同じ() {
    let (ja, en) = runs();
    let mut failures = Vec::new();
    assert_eq!(ja.len(), en.len());
    for ((name, json, j), (_, _, e)) in ja.iter().zip(en.iter()) {
        if j.code != e.code {
            failures.push(format!(
                "{name}: 終了コードが日英で違う（ja {} / en {}）\n{}{}",
                j.code, e.code, j.stderr, e.stderr
            ));
        }
        if *json {
            continue;
        }
        for (stream, text) in [("stdout", &e.stdout), ("stderr", &e.stderr)] {
            if let Some(c) = has_japanese(text) {
                failures.push(format!(
                    "{name}: 英語表示の {stream} に日本語（{c}）が混ざる:\n{text}"
                ));
            }
        }
        if e.stdout.trim().is_empty() {
            failures.push(format!("{name}: 英語表示の出力が空"));
        }
    }
    let old = old_ja();
    for (name, json, _) in ja.iter() {
        if !*json && !old.iter().any(|(n, _)| n == name) {
            failures.push(format!("{name}: 修正前の日本語の出力が表に無い"));
        }
    }
    for (name, stdout) in old {
        let Some((_, _, j)) = ja.iter().find(|(n, _, _)| *n == name) else {
            failures.push(format!("{name}: ケースが無い"));
            continue;
        };
        if digits0(&j.stdout) != stdout || !j.stderr.is_empty() {
            failures.push(format!(
                "{name}: 日本語表示が #2365 より前と違う\n--- want\n{stdout}--- got\n{}--- stderr\n{}",
                digits0(&j.stdout),
                j.stderr
            ));
        }
    }
    assert!(
        failures.is_empty(),
        "{} 件:\n{}",
        failures.len(),
        failures.join("\n\n")
    );
}

/// `--json`: test-residue は日英で同じ（`note` / `detail` は前のまま）。context-budget の提案と項目の
/// `advice` は表示言語の側の文（修正前は提案の側だけ日本語に固定だった）
#[test]
fn jsonはtest_residueが日英で同じでcontext_budgetのadviceは表示言語() {
    let (ja, en) = runs();
    let parse = |s: &str| serde_json::from_str::<Value>(s).unwrap_or(Value::Null);
    let mut failures = Vec::new();
    for ((name, json, j), (_, _, e)) in ja.iter().zip(en.iter()) {
        if !*json {
            continue;
        }
        let (jv, ev) = (parse(&j.stdout), parse(&e.stdout));
        if jv.is_null() || ev.is_null() {
            failures.push(format!(
                "{name}: JSON として読めない\n{}\n{}",
                j.stdout, e.stdout
            ));
            continue;
        }
        if name.starts_with("r_") {
            if jv != ev {
                failures.push(format!(
                    "{name}: --json が日英で違う\n  ja: {jv}\n  en: {ev}"
                ));
            }
            continue;
        }
        for (lang, v, key) in [("ja", &jv, "advice_ja"), ("en", &ev, "advice_en")] {
            let mut seen = 0;
            let empty = vec![];
            let props = v["proposals"].as_array().unwrap_or(&empty).iter();
            let items = v["items"].as_array().unwrap_or(&empty).iter();
            let violations =
                items.flat_map(|it| it["violations"].as_array().unwrap_or(&empty).iter());
            for o in props.chain(violations) {
                if o.get("advice").is_none() {
                    continue;
                }
                seen += 1;
                if o["advice"] != o[key] {
                    failures.push(format!(
                        "{name}（{lang}）: advice が表示言語の側（{key}）と違う: {o}"
                    ));
                }
            }
            if name.contains("fat") && seen < 2 {
                failures.push(format!(
                    "{name}（{lang}）: advice が提案と項目の両方に出ていない（{seen} 件）"
                ));
            }
        }
    }
    assert!(
        failures.is_empty(),
        "{} 件:\n{}",
        failures.len(),
        failures.join("\n\n")
    );
}

/// 英語表示でも行の数と字下げは日本語表示と同じ（文だけが入れ替わる。`--json` は除く）。
/// 英語の見本は数ケースだけ字面まで見る（数字は `0` に均す）
#[test]
fn 英語表示の行の数と字下げは日本語と同じ() {
    let (ja, en) = runs();
    let shape = |s: &str| -> Vec<usize> {
        s.lines()
            .map(|l| l.len() - l.trim_start_matches(' ').len())
            .collect()
    };
    let mut failures = Vec::new();
    for ((name, json, j), (_, _, e)) in ja.iter().zip(en.iter()) {
        if !*json && shape(&j.stdout) != shape(&e.stdout) {
            failures.push(format!(
                "{name}: 行の数か字下げが日英で違う\n--- ja\n{}--- en\n{}",
                j.stdout, e.stdout
            ));
        }
    }
    let en_of = |name: &str| {
        en.iter()
            .find(|(n, _, _)| *n == name)
            .map(|(_, _, o)| digits0(&o.stdout))
            .unwrap_or_default()
    };
    for (name, want) in en_samples() {
        let got = en_of(name);
        if got != want {
            failures.push(format!("{name}: 英語表示\n--- want\n{want}--- got\n{got}"));
        }
    }
    assert!(
        failures.is_empty(),
        "{} 件:\n{}",
        failures.len(),
        failures.join("\n\n")
    );
}

/// 英語表示の見本（数字は `0` に均してある）
fn en_samples() -> Vec<(&'static str, &'static str)> {
    vec![
        (
            "r_dry",
            r#"temp directory: <SB>/tmp
residue: 0 item(s) / 0 B
  tako-test-data-             0 item(s) /       0 B (0 removable)  cargo test data dir (isolated by #000)
  tako-agent-config-          0 item(s) /       0 B (0 removable)  agent settings and shell history of verification processes (isolated by #0000)
  tako-test-scratch-          0 item(s) /       0 B (0 removable)  parent of the throwaway work dirs that tests create (ScratchDir of #0000)
  tako-test-orchestrator-     0 item(s) /       0 B (0 removable)  cargo test orchestrator settings (isolated by tako_control::orchestrator)
  skipped 0: the owning process is alive = in use
  skipped 0: the pid is alive but started after the dir = pid reuse (skipped)
  skipped 0: cannot tell whether the owning process is alive (skipped)

removable: 0 item(s) / 0 B (nothing removed yet)
remove them now: tako test-residue --apply
"#,
        ),
        (
            "r_apply_fail",
            r#"temp directory: <SB>/tmp
residue: 0 item(s) / 0 B
  tako-test-data-             0 item(s) /       0 B (0 removable)  cargo test data dir (isolated by #000)
  tako-test-supervisor-       0 item(s) /       0 B (0 removable)  cargo test supervisor audit log (isolated by tako_control::orchestrator)

removed: 0 item(s) / 0 B (0 failed: Permission denied (os error 00))
"#,
        ),
        (
            "b_fix_revived",
            r#"<SB>/hm/w/.agent/progress.md: 0 entries → 0 kept / 0 to the archive (total preserved: yes)
  000 → 00 bytes (about 00 → 00 tokens)
  Drop from the work log (already in the archive = entries revived by a rebase): 0
    <SB>/hm/w/.agent/progress.md:0 archived entry revived: ## 0000-00-00 (#0 entry 0) ← <SB>/hm/w/.agent/progress-archive.md:0 "- 0000-00-00 #0 entry 0"
files changed: 0
"#,
        ),
        (
            "b_fix_aged",
            r#"<SB>/hm/w/.agent/progress.md: 0 entries → 0 kept / 0 to the archive (total preserved: yes)
  000 → 000 bytes (about 00 → 00 tokens)
  removed from the archive after the retention period: 0 line(s)
files changed: 0
"#,
        ),
    ]
}
