//! **#2501 の番犬**: CI 待ちの既定の上限を文章に書いた所（md・コメント・`--help`）の数字が、
//! 実値（`scripts/wait-pr-checks.sh` の `DEFAULT_TIMEOUT`・`.github/workflows/ci.yml` の
//! macOS / Windows の job の `timeout-minutes`）とずれない。
//!
//! #2450 で job の上限を上げて既定の待ちも伸ばしたとき、`.agent/commands.md` の
//! 「既定の待ちは N 秒」と `DEFAULT_TIMEOUT` の上のコメントの `timeout-minutes: M` が
//! 古い値のまま残った（#2498 で直した）。`test-wait-pr-checks.sh` の Test 35 は
//! 「既定 ≥ macOS の上限」だけを見ていて、文章に書いた数字は見ていなかった。
//!
//! 置き場をシェルのモックテストではなくここにした理由: `test-wait-pr-checks.sh` は macOS の
//! job の中でしか走らず、`.agent/` の md だけを直す PR では skip される（#2084）。この番犬は
//! `.agent/` の md を読むので `scripts/ci-docs-watchdogs.sh` が拾い、ubuntu で毎回走る。
//!
//! 読み方: 記述ごとに「範囲（表の行・コメントの塊・md の節・使い方の heredoc）」と
//! 「前置き N 後置き」の型を持つ。空白・改行・`**`・`` ` `` の揺れは無視し、前置きと数字の
//! あいだの数文字（「も同じ」等）も許す。`=` のように前置きだけでは既定の記述と実測の記述を
//! 分けられない型は、同じ文に要る語（「既定」）も持つ。型に当たる記述が範囲に 1 つも無いときも落とす
//! （言い回しを変えて番犬が黙って外れるのを防ぐ）。数字はソースから読み、ここには書かない

use std::ops::Range;
use std::path::{Path, PathBuf};

const WAIT_SH: &str = "scripts/wait-pr-checks.sh";
const MERGE_SH: &str = "scripts/merge-pr.sh";
const CI_YML: &str = ".github/workflows/ci.yml";
const COMMANDS_MD: &str = ".agent/commands.md";
const CONVENTIONS_MD: &str = ".agent/conventions.md";

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("リポジトリルート")
}

/// 読むファイルの本文（自己検査はこれを書き換えて注入する）
#[derive(Clone)]
struct Sources(Vec<(&'static str, String)>);

impl Sources {
    fn load() -> Self {
        let root = repo_root();
        Sources(
            [WAIT_SH, MERGE_SH, CI_YML, COMMANDS_MD, CONVENTIONS_MD]
                .into_iter()
                .map(|f| {
                    let text = std::fs::read_to_string(root.join(f))
                        .unwrap_or_else(|e| panic!("{f} を読めない: {e}"));
                    (f, text)
                })
                .collect(),
        )
    }

    fn get(&self, file: &str) -> &str {
        &self
            .0
            .iter()
            .find(|(f, _)| *f == file)
            .expect("読むファイル")
            .1
    }

    fn get_mut(&mut self, file: &str) -> &mut String {
        &mut self
            .0
            .iter_mut()
            .find(|(f, _)| *f == file)
            .expect("読むファイル")
            .1
    }
}

/// 1 行（`no` は 1 始まり・`start` はファイル先頭からのバイト位置・`text` は改行を除いた本文）
struct Line<'a> {
    no: usize,
    start: usize,
    text: &'a str,
}

fn lines(text: &str) -> Vec<Line<'_>> {
    let mut out = Vec::new();
    let mut start = 0;
    for (i, raw) in text.split_inclusive('\n').enumerate() {
        out.push(Line {
            no: i + 1,
            start,
            // Windows の checkout で CRLF になっていても同じに読む
            text: raw.trim_end_matches(['\n', '\r']),
        });
        start += raw.len();
    }
    out
}

/// 実値とそのファイル上のバイト位置（自己検査が書き換える）
type Located = (u64, Range<usize>);

/// `head` で始まる行の残りの数字（とそのバイト位置）。`DEFAULT_TIMEOUT=<秒>` のように、残り全部が数字のときだけ
fn number_after(line: &Line<'_>, head: &str) -> Option<Located> {
    let rest = line.text.strip_prefix(head)?.trim_end();
    if rest.is_empty() || !rest.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let at = line.start + head.len();
    Some((rest.parse().ok()?, at..at + rest.len()))
}

/// `scripts/wait-pr-checks.sh` の `DEFAULT_TIMEOUT=<秒>`（A/B の腕の `then DEFAULT_TIMEOUT=…` は行頭でないので拾わない）
fn default_timeout(wait_sh: &str) -> Option<Located> {
    lines(wait_sh)
        .iter()
        .find_map(|l| number_after(l, "DEFAULT_TIMEOUT="))
}

/// ci.yml の `jobs:` の下の job（字下げ 2）の `timeout-minutes`（字下げ 4）。Test 35 の awk と同じ読み方
fn job_timeout(ci_yml: &str, job: &str) -> Option<Located> {
    let header = format!("{job}:");
    let mut in_job = false;
    for l in lines(ci_yml) {
        let body = l.text.trim_start();
        let indent = l.text.len() - body.len();
        if body.is_empty() || body.starts_with('#') {
            continue;
        }
        if indent < 2 {
            in_job = false;
        } else if indent == 2 && body.ends_with(':') && !body.contains(' ') {
            in_job = body == header;
        } else if in_job && indent == 4 {
            if let Some(found) = number_after(&l, "    timeout-minutes: ") {
                return Some(found);
            }
        }
    }
    None
}

/// 実値（どれもソースから読む）
struct Actual {
    default_secs: u64,
    mac_min: u64,
    win_min: u64,
}

fn actual(src: &Sources) -> Result<Actual, Vec<String>> {
    let d = default_timeout(src.get(WAIT_SH));
    let m = job_timeout(src.get(CI_YML), "macos");
    let w = job_timeout(src.get(CI_YML), "windows");
    match (d, m, w) {
        (Some((d, _)), Some((m, _)), Some((w, _))) => Ok(Actual {
            default_secs: d,
            mac_min: m,
            win_min: w,
        }),
        (d, m, w) => Err(vec![format!(
            "実値を読めない（{WAIT_SH} の DEFAULT_TIMEOUT={:?} / {CI_YML} の macos の timeout-minutes={:?} / \
             windows の timeout-minutes={:?}）。置き場か書き方が変わったらこの番犬の読み方も直す",
            d.map(|x| x.0),
            m.map(|x| x.0),
            w.map(|x| x.0),
        )]),
    }
}

/// 記述を探す範囲
#[derive(Clone, Copy)]
enum Scope {
    /// md の表の行のうち、この文字列を含むもの
    Row(&'static str),
    /// この文字列で始まる行の直前に続く `#` コメントの塊
    CommentAbove(&'static str),
    /// 見出しにこの文字列を含む md の節（次の同じ深さ以上の見出しまで。コードブロックの中の `#` は見出しにしない）
    Section(&'static str),
    /// `<<'X'` から `X` までの heredoc（全部）
    Heredoc(&'static str),
}

fn heading_level(text: &str) -> Option<usize> {
    let n = text.bytes().take_while(|b| *b == b'#').count();
    (n > 0 && text[n..].starts_with(' ')).then_some(n)
}

fn scope_lines<'a>(text: &'a str, scope: &Scope) -> Vec<Line<'a>> {
    let all = lines(text);
    match scope {
        Scope::Row(marker) => all
            .into_iter()
            .filter(|l| l.text.contains(*marker))
            .collect(),
        Scope::CommentAbove(head) => {
            let Some(at) = all.iter().position(|l| l.text.starts_with(*head)) else {
                return Vec::new();
            };
            let from = all[..at]
                .iter()
                .rposition(|l| !l.text.trim_start().starts_with('#'))
                .map_or(0, |i| i + 1);
            all.into_iter().skip(from).take(at - from).collect()
        }
        Scope::Section(marker) => {
            let mut out = Vec::new();
            let mut level = None;
            let mut fence = false;
            for l in all {
                if l.text.trim_start().starts_with("```") {
                    fence = !fence;
                }
                let h = if fence { None } else { heading_level(l.text) };
                match (level, h) {
                    (None, Some(n)) if l.text.contains(*marker) => level = Some(n),
                    (Some(cur), Some(n)) if n <= cur => break,
                    (Some(_), _) => out.push(l),
                    _ => {}
                }
            }
            out
        }
        Scope::Heredoc(tag) => {
            let open = format!("<<'{tag}'");
            let mut out = Vec::new();
            let mut inside = false;
            for l in all {
                if inside && l.text == *tag {
                    inside = false;
                } else if inside {
                    out.push(l);
                } else if l.text.trim_end().ends_with(&open) {
                    inside = true;
                }
            }
            out
        }
    }
}

/// 範囲の本文の 1 文字（空白・`*`・`` ` ``・行頭の `#` は外してある）
struct Ch {
    c: char,
    line: usize,
    byte: usize,
    /// 直前に外した文字（または行の境目）がある = 数字をここでつなげない
    gap_before: bool,
}

fn is_noise(c: char) -> bool {
    c.is_whitespace() || c == '*' || c == '`'
}

fn compact(lines: &[Line<'_>]) -> Vec<Ch> {
    let mut out = Vec::new();
    for l in lines {
        let body = l.text.trim_start().trim_start_matches('#');
        let lead = l.text.len() - body.len();
        let mut gap = true;
        for (i, c) in body.char_indices() {
            if is_noise(c) {
                gap = true;
                continue;
            }
            out.push(Ch {
                c,
                line: l.no,
                byte: l.start + lead + i,
                gap_before: gap,
            });
            gap = false;
        }
    }
    out
}

/// 前置きと数字のあいだに許す言い回しの文字数（「上限も同じ N 分」の「も同じ」）
const GAP: usize = 4;
/// 前置きと数字のあいだに来たら別の文とみなす文字
const BREAKS: &[char] = &[
    '。', '、', '，', '．', '（', '）', '(', ')', '「', '」', '|', '=', ';',
];

/// 範囲で見つけた 1 つの記述（`digits` は数字のファイル上のバイト位置）
struct Hit {
    line: usize,
    value: u64,
    digits: Range<usize>,
    /// 前置きから後置きまでの、詰めた文字列の上の位置（文の条件を見るのに使う）
    span: Range<usize>,
}

fn starts_with(chars: &[Ch], at: usize, pat: &[char]) -> bool {
    chars.len() >= at + pat.len()
        && chars[at..at + pat.len()]
            .iter()
            .zip(pat)
            .all(|(a, b)| a.c == *b)
}

/// 「前置き N 後置き」の N を全部拾う（前置き・後置きの空白は無視する）
fn hits(chars: &[Ch], prefix: &str, suffix: &str) -> Vec<Hit> {
    let p: Vec<char> = prefix.chars().filter(|c| !is_noise(*c)).collect();
    let s: Vec<char> = suffix.chars().filter(|c| !is_noise(*c)).collect();
    let mut out = Vec::new();
    for i in 0..chars.len() {
        if !starts_with(chars, i, &p) {
            continue;
        }
        let mut j = i + p.len();
        let gap_end = (j + GAP).min(chars.len());
        while j < gap_end && !chars[j].c.is_ascii_digit() && !BREAKS.contains(&chars[j].c) {
            j += 1;
        }
        if j >= chars.len() || !chars[j].c.is_ascii_digit() {
            continue;
        }
        let first = j;
        let mut value: u64 = 0;
        while j < chars.len() && chars[j].c.is_ascii_digit() && (j == first || !chars[j].gap_before)
        {
            value = value
                .saturating_mul(10)
                .saturating_add(u64::from(chars[j].c.to_digit(10).unwrap_or(0)));
            j += 1;
        }
        if !starts_with(chars, j, &s) {
            continue;
        }
        out.push(Hit {
            line: chars[first].line,
            value,
            digits: chars[first].byte..chars[j - 1].byte + 1,
            span: i..j + s.len(),
        });
    }
    out
}

#[derive(Clone, Copy)]
enum Unit {
    Sec,
    Min,
}

impl Unit {
    fn secs(self) -> u64 {
        match self {
            Unit::Sec => 1,
            Unit::Min => 60,
        }
    }

    fn show(self, secs: u64) -> String {
        match self {
            Unit::Sec => format!("{secs} 秒"),
            Unit::Min if secs.is_multiple_of(60) => format!("{} 分", secs / 60),
            Unit::Min => format!("{secs} 秒（分で割り切れない）"),
        }
    }
}

/// 記述が表している実値
#[derive(Clone, Copy, PartialEq)]
enum Expect {
    /// `DEFAULT_TIMEOUT`
    DefaultWait,
    /// ci.yml の macOS の job の `timeout-minutes`
    MacJob,
    /// ci.yml の Windows の job の `timeout-minutes`
    WinJob,
    /// 既定の待ち − macOS の job の上限（判定と runner 待ちの余裕）
    MarginOverMac,
}

impl Expect {
    /// (秒, どこから来た値か)
    fn secs(self, a: &Actual) -> (u64, String) {
        let (d, m, w) = (a.default_secs, a.mac_min, a.win_min);
        match self {
            Expect::DefaultWait => (d, format!("{WAIT_SH} の DEFAULT_TIMEOUT={d}")),
            Expect::MacJob => (m * 60, format!("{CI_YML} の macos の timeout-minutes: {m}")),
            Expect::WinJob => (
                w * 60,
                format!("{CI_YML} の windows の timeout-minutes: {w}"),
            ),
            Expect::MarginOverMac => (
                d.saturating_sub(m * 60),
                format!("DEFAULT_TIMEOUT={d} 秒 − macOS の job の上限 {m} 分"),
            ),
        }
    }
}

/// 文章に書いた既定の 1 つ
#[derive(Clone, Copy)]
struct Claim {
    file: &'static str,
    scope: Scope,
    prefix: &'static str,
    suffix: &'static str,
    unit: Unit,
    expect: Expect,
    /// 数字と同じ文（`。` で区切る）にこの語が要る。`=` のように前置きだけでは既定の記述と
    /// 実測の記述（`PR #2141 = 54 分 30 秒`）を分けられない型に付ける
    sentence: Option<&'static str>,
}

const fn claim(
    file: &'static str,
    scope: Scope,
    prefix: &'static str,
    suffix: &'static str,
    unit: Unit,
    expect: Expect,
) -> Claim {
    Claim {
        file,
        scope,
        prefix,
        suffix,
        unit,
        expect,
        sentence: None,
    }
}

impl Claim {
    const fn in_sentence_with(self, word: &'static str) -> Claim {
        Claim {
            sentence: Some(word),
            ..self
        }
    }

    /// 失敗の文言に出す型（数字の位置は N）
    fn label(&self) -> String {
        format!("{} N {}", self.prefix, self.suffix)
            .trim_end()
            .to_string()
    }
}

const WAIT_ROW: Scope = Scope::Row("scripts/wait-pr-checks.sh <PR>");
const DEFAULT_COMMENT: Scope = Scope::CommentAbove("DEFAULT_TIMEOUT=");
const WAIT_SECTION: Scope = Scope::Section("CI 完了の判定は待ちスクリプトを通す");
const USAGE: Scope = Scope::Heredoc("USAGE");

const CLAIMS: &[Claim] = &[
    // .agent/commands.md の wait-pr-checks.sh の行（#2498 で直した所）
    claim(
        COMMANDS_MD,
        WAIT_ROW,
        "既定の待ちは",
        "秒",
        Unit::Sec,
        Expect::DefaultWait,
    ),
    claim(
        COMMANDS_MD,
        WAIT_ROW,
        "timeout-minutes:",
        "",
        Unit::Min,
        Expect::MacJob,
    ),
    claim(
        COMMANDS_MD,
        WAIT_ROW,
        "余裕",
        "分",
        Unit::Min,
        Expect::MarginOverMac,
    ),
    claim(
        COMMANDS_MD,
        WAIT_ROW,
        "Windows の job の上限",
        "分",
        Unit::Min,
        Expect::WinJob,
    ),
    // DEFAULT_TIMEOUT の上のコメント（#2498 で直した所）
    claim(
        WAIT_SH,
        DEFAULT_COMMENT,
        "timeout-minutes:",
        "",
        Unit::Min,
        Expect::MacJob,
    ),
    claim(
        WAIT_SH,
        DEFAULT_COMMENT,
        "=",
        "分",
        Unit::Min,
        Expect::DefaultWait,
    )
    .in_sentence_with("既定"),
    claim(
        WAIT_SH,
        DEFAULT_COMMENT,
        "余裕",
        "分",
        Unit::Min,
        Expect::MarginOverMac,
    ),
    claim(
        WAIT_SH,
        DEFAULT_COMMENT,
        "Windows の job の上限",
        "分",
        Unit::Min,
        Expect::WinJob,
    ),
    // .agent/conventions.md の待ちの節
    claim(
        CONVENTIONS_MD,
        WAIT_SECTION,
        "既定の上限は",
        "秒",
        Unit::Sec,
        Expect::DefaultWait,
    ),
    claim(
        CONVENTIONS_MD,
        WAIT_SECTION,
        "timeout-minutes:",
        "",
        Unit::Min,
        Expect::MacJob,
    ),
    // 両スクリプトの --help
    claim(WAIT_SH, USAGE, "既定", "秒", Unit::Sec, Expect::DefaultWait),
    claim(
        MERGE_SH,
        USAGE,
        "既定",
        "秒",
        Unit::Sec,
        Expect::DefaultWait,
    ),
];

/// `span` を含む文（`。` から `。` まで）にこの語があるか
fn sentence_has(chars: &[Ch], span: &Range<usize>, word: &str) -> bool {
    let w: Vec<char> = word.chars().filter(|c| !is_noise(*c)).collect();
    let start = chars[..span.start]
        .iter()
        .rposition(|c| c.c == '。')
        .map_or(0, |i| i + 1);
    let end = chars[span.end..]
        .iter()
        .position(|c| c.c == '。')
        .map_or(chars.len(), |i| span.end + i);
    (start..end).any(|i| i + w.len() <= end && starts_with(chars, i, &w))
}

fn claim_hits(src: &Sources, claim: &Claim) -> Vec<Hit> {
    let chars = compact(&scope_lines(src.get(claim.file), &claim.scope));
    hits(&chars, claim.prefix, claim.suffix)
        .into_iter()
        .filter(|h| {
            claim
                .sentence
                .is_none_or(|w| sentence_has(&chars, &h.span, w))
        })
        .collect()
}

/// ずれ 1 件 = 1 行（先頭は `ファイル:行 `）
fn check(src: &Sources) -> Vec<String> {
    let a = match actual(src) {
        Ok(a) => a,
        Err(e) => return e,
    };
    let mut errs = Vec::new();
    for claim in CLAIMS {
        let found = claim_hits(src, claim);
        if found.is_empty() {
            errs.push(format!(
                "{} 「{}」の記述が範囲に見つからない（言い回しや置き場を変えたら、この番犬の型も直す）",
                claim.file, claim.label()
            ));
            continue;
        }
        let (want, from) = claim.expect.secs(&a);
        for h in found {
            let got = h.value * claim.unit.secs();
            if got != want {
                errs.push(format!(
                    "{}:{} 「{}」が {} だが、実値は {}（{}）",
                    claim.file,
                    h.line,
                    claim.label(),
                    claim.unit.show(got),
                    claim.unit.show(want),
                    from
                ));
            }
        }
    }
    errs
}

#[test]
fn 待ちの既定を書いた記述が実値と一致する() {
    let errs = check(&Sources::load());
    assert!(
        errs.is_empty(),
        "CI 待ちの既定を書いた記述が実値とずれている（#2501。DEFAULT_TIMEOUT か ci.yml の job の上限を\
         変えたら、md・コメント・--help の数字も同じ PR で直す）:\n  {}",
        errs.join("\n  ")
    );
}

/// 検出力の自己検査（メモリ上の注入）: 記述の数字を 1 か所ずつずらすと、その行を名指しして落ちる
#[test]
fn 記述の数字を1か所ずらすとその行を名指しして落ちる() {
    let src = Sources::load();
    let mut injected = 0;
    for claim in CLAIMS {
        for h in claim_hits(&src, claim) {
            let mut bad = src.clone();
            bad.get_mut(claim.file)
                .replace_range(h.digits.clone(), &(h.value + 1).to_string());
            let at = format!("{}:{} ", claim.file, h.line);
            let errs = check(&bad);
            assert!(
                errs.iter().any(|e| e.starts_with(&at)),
                "{at}「{}」の {} → {} を拾えない: {errs:?}",
                claim.label(),
                h.value,
                h.value + 1
            );
            injected += 1;
        }
    }
    assert!(
        injected >= CLAIMS.len(),
        "注入した記述が {injected} 件しかない"
    );
}

/// 実値の側（DEFAULT_TIMEOUT・ci.yml の macOS / Windows の上限）だけを変えると、
/// それに依る記述が全部、ファイルと行を名指しして落ちる
#[test]
fn 実値だけを変えると依る記述が全部落ちる() {
    let src = Sources::load();
    let cases: [(&str, Option<Located>, &[Expect]); 3] = [
        (
            WAIT_SH,
            default_timeout(src.get(WAIT_SH)),
            &[Expect::DefaultWait, Expect::MarginOverMac],
        ),
        (
            CI_YML,
            job_timeout(src.get(CI_YML), "macos"),
            &[Expect::MacJob, Expect::MarginOverMac],
        ),
        (
            CI_YML,
            job_timeout(src.get(CI_YML), "windows"),
            &[Expect::WinJob],
        ),
    ];
    for (file, value, affected) in cases {
        let (v, range) = value.expect("実値を読める");
        let mut bad = src.clone();
        // 秒の実値も分で割り切れるまま動かす（「分で割り切れない」で落ちたのを拾えたと数えない）
        let bumped = if file == WAIT_SH { v + 60 } else { v + 1 };
        bad.get_mut(file).replace_range(range, &bumped.to_string());
        let errs = check(&bad);
        let want: Vec<String> = CLAIMS
            .iter()
            .filter(|c| affected.contains(&c.expect))
            .flat_map(|c| {
                claim_hits(&bad, c)
                    .into_iter()
                    .map(move |h| format!("{}:{} 「{}」", c.file, h.line, c.label()))
            })
            .collect();
        assert!(!want.is_empty(), "{file} の実値に依る記述が 1 つも無い");
        for w in &want {
            assert!(
                errs.iter().any(|e| e.starts_with(w.as_str())),
                "{file} の実値 {v} → {bumped} で {w} が落ちない: {errs:?}"
            );
        }
        // 依らない記述は巻き込まない
        for e in &errs {
            assert!(
                want.iter().any(|w| e.starts_with(w.as_str())),
                "{file} の実値だけを変えたのに依らない記述まで落ちた: {e}"
            );
        }
    }
}

/// 読み方の境界: 空白をまたいだ数字をつなげない・前置きと数字のあいだに句読点が来たら別の文
#[test]
fn 読み方の境界() {
    let text = "余裕 2 0 分\n余裕。20 分\n上限も同じ 45 分\n既定の待ちは\n  1234 秒\n";
    let ls = lines(text);
    let ch = compact(&ls);
    assert!(
        hits(&ch, "余裕", "分").is_empty(),
        "空白をまたいで 2 と 0 をつないだ / 句点を越えた"
    );
    let w: Vec<u64> = hits(&ch, "上限", "分").iter().map(|h| h.value).collect();
    assert_eq!(w, vec![45], "「も同じ」をはさんだ数字を拾う");
    let d: Vec<(usize, u64)> = hits(&ch, "既定の待ちは", "秒")
        .iter()
        .map(|h| (h.line, h.value))
        .collect();
    assert_eq!(
        d,
        vec![(5, 1234)],
        "改行で折り返した記述も拾い、数字の行を名指しする"
    );

    let ls = lines("実測（PR #1 = 54 分 30 秒）。\n既定は余裕 20 分 = 77 分。\n");
    let ch = compact(&ls);
    let k: Vec<u64> = hits(&ch, "=", "分")
        .into_iter()
        .filter(|h| sentence_has(&ch, &h.span, "既定"))
        .map(|h| h.value)
        .collect();
    assert_eq!(
        k,
        vec![77],
        "文の条件が付いた型は、その語のある文の数字だけを拾う"
    );
}
