//! 番犬: 脅威モデルの「既定」が `local_endpoint` の既定値と食い違わない（#1406）
//!
//! ## なぜ止めるのか
//!
//! `.agent/threat-model-remote.md` は「何を受容したか」を後から読む文書なので、
//! **受容していないものを『塞いだ』と書いてあるのが一番まずい種類の誤り**。
//! 実際に #1038（待ち受けの既定を UDS からループバック TCP へ）を入れたとき、
//! トレードオフ節は正しく書き換わったのに「残存リスク（受容）」節の最終項は
//! #287 P1-2 当時のまま「別 OS ユーザーによる daemon への到達経路は UDS 化で
//! 消滅した」と残り、同じ文書の中で矛盾していた（= #1406）。
//!
//! 文章は機械検査が無いと、実装が変わった瞬間に黙って嘘になる。だからこの番犬は
//! **コードの既定値を読んでから**、文書がその既定を書いているかを見る。
//!
//! ## 4 本立て（どれか 1 つでは穴が残る）
//!
//! 1. [`コードの既定とlocal_endpointのdocが一致している`] — 既定値の正（`parse_endpoint_spec(None)`）と
//!    境界モジュールの doc が食い違っていないこと
//! 2. [`脅威モデルが現行の既定を書いている`] — **1 で読んだ既定**に応じて、
//!    脅威モデルの 3 節（トレードオフ / listen 範囲 / 残存リスク）が現行の話をしていること
//! 3. [`別osユーザーの到達を無条件に閉じたと書いていない`] — 文書のどこであっても
//!    「別 OS ユーザーは到達できない」を**条件なしに**書いていないこと
//!    （節の needle 検査だけでは、別の節に同型が生えたときに落ちない）
//! 4. [`i1406_注入_旧記述はfile_lineで名指しできる`] — 検出力（旧記述を戻したら
//!    `file:line` で名指しできる）
//!
//! 既定を変えた（= `parse_endpoint_spec(None)` の戻りを変えた）ときは、
//! この番犬が文書側の書き換えを同じコミットで要求する。

use std::path::{Path, PathBuf};

use tako_control::platform::local_endpoint;
use tako_control::remote;

/// 脅威モデル（受容したものを後から読む文書）
const THREAT_MODEL: &str = ".agent/threat-model-remote.md";
/// 待ち受けの実体を持つ境界モジュール（B4）
const LOCAL_ENDPOINT_RS: &str = "crates/tako-control/src/platform/local_endpoint.rs";

/// #1038 のトレードオフを説明する節
const SECTION_TRADEOFF: &str = "### ループバック TCP のトレードオフ";
/// どこから到達できるかを書く節
const SECTION_LISTEN: &str = "### daemon の listen 範囲";
/// 受容したものを並べる節
const SECTION_RESIDUAL: &str = "## 残存リスク（受容）";

/// 「別 OS ユーザー」を指す語（どちらの書き方も文書に在る）
const OTHER_USER: [&str; 2] = ["別 OS ユーザー", "別ユーザー"];
/// 「到達できない」と断じる語
const CLOSED_CLAIM: [&str; 4] = ["消滅", "接続不能", "接続自体が不能", "到達できない"];
/// 到達を塞げる**条件**（これが同じ塊に無いと無条件の断定になる）
const CONDITION: &str = "TAKO_REMOTE_ENDPOINT=unix";
/// 過去の話だと読める印（履歴の記述は脅威モデルに残す価値がある）
const PAST_MARKERS: [&str; 2] = ["v0.8.1", "以前は"];

/// 既定がループバック TCP のときに各節へ要求する needle
const LOOPBACK_TRADEOFF: [&str; 3] = ["ループバック TCP を既定に", "#1038", "#841"];
const LOOPBACK_LISTEN: [&str; 2] = ["既定のループバック TCP では防げない", CONDITION];
const LOOPBACK_RESIDUAL: [&str; 6] = [
    "別 OS ユーザー",
    "受容するリスク",
    CONDITION,
    // opt-in が成立しないプラットフォーム（`unix_supported()` = `cfg!(unix)`）を書いていること
    "unix_supported",
    "#841",
    "#1038",
];
/// 既定を UDS へ戻したときに各節へ要求する needle（逆向きにも縛る）
const UNIX_LISTEN: [&str; 2] = ["UDS が既定", "接続不能"];
const UNIX_RESIDUAL: [&str; 2] = ["UDS が既定", "別 OS ユーザー"];

/// 待ち受けの形（`EndpointSpec` の中身までは見ない = 既定の「種類」だけを比べる）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Loopback,
    Unix,
}

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("リポジトリルート")
        .to_path_buf()
}

fn read(rel: &str) -> String {
    std::fs::read_to_string(repo_root().join(rel)).unwrap_or_else(|e| panic!("{rel} を読む: {e}"))
}

/// **コードの既定**（env を一切読まない純関数の「未指定」= 既定）
fn code_default() -> Kind {
    match remote::parse_endpoint_spec(None).expect("既定の待ち受けは常に解釈できる")
    {
        local_endpoint::EndpointSpec::Loopback => Kind::Loopback,
        local_endpoint::EndpointSpec::Unix(_) => Kind::Unix,
    }
}

/// 切り出した塊（`file:line` で名指しできるよう開始行を持つ）
struct Block {
    /// 1-indexed の開始行
    line: usize,
    text: String,
}

/// 見出し行から次の見出し（`## ` / `### `）までを 1 節として切り出す
fn md_section(src: &str, header: &str) -> Option<Block> {
    let lines: Vec<&str> = src.lines().collect();
    let at = lines.iter().position(|l| l.starts_with(header))?;
    let end = lines[at + 1..]
        .iter()
        .position(|l| l.starts_with("## ") || l.starts_with("### "))
        .map(|i| at + 1 + i)
        .unwrap_or(lines.len());
    Some(Block {
        line: at + 1,
        text: lines[at..end].join("\n"),
    })
}

/// 文書を「トップレベルの箇条書き 1 つ」または「箇条書きでない連続行」へ割る。
///
/// 節単位で見ると、同じ節の別の行に在る `TAKO_REMOTE_ENDPOINT=unix` が
/// 無条件の断定を免責してしまう（#1406 の旧 `:87` がまさにその形）。
fn blocks(src: &str) -> Vec<Block> {
    let mut out: Vec<Block> = Vec::new();
    let mut cur: Option<Block> = None;
    for (i, line) in src.lines().enumerate() {
        let starts_bullet = line.starts_with("- ");
        let breaks = line.trim().is_empty() || line.starts_with('#');
        if breaks || starts_bullet {
            if let Some(b) = cur.take() {
                out.push(b);
            }
        }
        if breaks {
            continue;
        }
        match cur.as_mut() {
            Some(b) => {
                b.text.push('\n');
                b.text.push_str(line);
            }
            None => {
                cur = Some(Block {
                    line: i + 1,
                    text: line.to_string(),
                })
            }
        }
    }
    out.extend(cur);
    out
}

/// 塊から欠けている needle を `file:line` 付きで並べる（空なら合格）
fn missing(rel: &str, block: &Block, needles: &[&str], what: &str) -> Vec<String> {
    needles
        .iter()
        .filter(|n| !block.text.contains(**n))
        .map(|n| format!("{rel}:{} {what}に `{n}` が無い", block.line))
        .collect()
}

/// 「別 OS ユーザーは到達できない」を**条件なしに**書いている塊を並べる
fn unconditional_claims(rel: &str, src: &str) -> Vec<String> {
    blocks(src)
        .into_iter()
        .filter(|b| {
            OTHER_USER.iter().any(|u| b.text.contains(u))
                && CLOSED_CLAIM.iter().any(|c| b.text.contains(c))
                && !b.text.contains(CONDITION)
                && !PAST_MARKERS.iter().any(|p| b.text.contains(p))
        })
        .map(|b| {
            format!(
                "{rel}:{} 別 OS ユーザーの到達を条件なしに閉じたと書いている: {}",
                b.line,
                b.text.lines().next().unwrap_or("").trim()
            )
        })
        .collect()
}

/// `local_endpoint` のモジュール doc が `**既定**` と印を付けている側
fn doc_marked_default(src: &str) -> (usize, Kind) {
    let marked: Vec<(usize, &str)> = src
        .lines()
        .enumerate()
        .filter(|(_, l)| l.starts_with("//!") && l.contains("**既定**"))
        .map(|(i, l)| (i + 1, l))
        .collect();
    assert_eq!(
        marked.len(),
        1,
        "{LOCAL_ENDPOINT_RS}: モジュール doc の `**既定**` の印が {} 個ある（1 個であること）",
        marked.len()
    );
    let (line, text) = marked[0];
    let kind = if text.contains("ループバック TCP") {
        Kind::Loopback
    } else if text.contains("Unix domain socket") || text.contains("UDS") {
        Kind::Unix
    } else {
        panic!("{LOCAL_ENDPOINT_RS}:{line} `**既定**` がどちらの形を指すか読めない: {text}");
    };
    (line, kind)
}

#[test]
fn コードの既定とlocal_endpointのdocが一致している() {
    let (line, marked) = doc_marked_default(&read(LOCAL_ENDPOINT_RS));
    assert_eq!(
        marked,
        code_default(),
        "{LOCAL_ENDPOINT_RS}:{line} モジュール doc が `**既定**` と印を付けた形（{marked:?}）と、\n\
         `remote::parse_endpoint_spec(None)` が返す実際の既定（{:?}）が食い違っている。\n\
         既定を変えたなら doc・{THREAT_MODEL} を同じコミットで直すこと",
        code_default()
    );
}

#[test]
fn 脅威モデルが現行の既定を書いている() {
    let src = read(THREAT_MODEL);
    let section = |header: &str| {
        md_section(&src, header).unwrap_or_else(|| {
            panic!("{THREAT_MODEL}: 節 {header:?} が無い（改題したら番犬も直す）")
        })
    };
    let listen = section(SECTION_LISTEN);
    let residual = section(SECTION_RESIDUAL);

    let gaps: Vec<String> = match code_default() {
        Kind::Loopback => {
            let tradeoff = section(SECTION_TRADEOFF);
            [
                missing(
                    THREAT_MODEL,
                    &tradeoff,
                    &LOOPBACK_TRADEOFF,
                    "トレードオフ節",
                ),
                missing(THREAT_MODEL, &listen, &LOOPBACK_LISTEN, "listen 範囲の節"),
                missing(
                    THREAT_MODEL,
                    &residual,
                    &LOOPBACK_RESIDUAL,
                    "残存リスクの節",
                ),
            ]
            .concat()
        }
        Kind::Unix => [
            missing(THREAT_MODEL, &listen, &UNIX_LISTEN, "listen 範囲の節"),
            missing(THREAT_MODEL, &residual, &UNIX_RESIDUAL, "残存リスクの節"),
        ]
        .concat(),
    };
    assert!(
        gaps.is_empty(),
        "待ち受けの既定は現在 {:?}（`remote::parse_endpoint_spec(None)` の実測）。\n\
         脅威モデルはその既定のもとで**何を受容したか**を書く文書なので、\n\
         既定が変わったら 3 節（トレードオフ / listen 範囲 / 残存リスク）を揃って直す:\n  {}",
        code_default(),
        gaps.join("\n  ")
    );
}

#[test]
fn 別osユーザーの到達を無条件に閉じたと書いていない() {
    let claims = unconditional_claims(THREAT_MODEL, &read(THREAT_MODEL));
    assert!(
        claims.is_empty(),
        "既定のループバック TCP では、同一マシンの別 OS ユーザーも `127.0.0.1:<port>` へ\n\
         接続できる（#1038 / #841）。到達を閉じられるのは `{CONDITION}` を明示した\n\
         UDS のときだけなので、断じるなら**同じ塊に条件を書く**（過去の話なら {PAST_MARKERS:?} の\n\
         どれかで時点を示す）:\n  {}",
        claims.join("\n  ")
    );
}

/// 旧記述を戻したときに `file:line` で名指しできること（検出力の確認）
#[test]
fn i1406_注入_旧記述はfile_lineで名指しできる() {
    // 注入 1: #1406 の旧「残存リスク」最終項（UDS 化で消滅した、と無条件に断じる形）
    let stale_residual = "## 残存リスク（受容）\n\
        \n\
        - 同一 OS ユーザーの悪意あるプロセスと root は、socket への接続・token /\n\
        \u{20} devices.json の直接読み取りが可能（OS レベルの侵害であり tako の防護範囲外。\n\
        \u{20} 従来どおり）。別 OS ユーザーによる daemon への到達経路は UDS 化（#287 P1-2）で\n\
        \u{20} 消滅した\n";
    let claims = unconditional_claims(THREAT_MODEL, stale_residual);
    assert_eq!(claims.len(), 1, "注入 1 を拾えていない: {claims:?}");
    assert!(
        claims[0].starts_with(&format!("{THREAT_MODEL}:3 ")),
        "注入 1 の行番号を名指しできていない: {claims:?}"
    );
    // 節の needle 検査でも落ちる（別 OS ユーザーの受容を書いていない）
    let block = md_section(stale_residual, SECTION_RESIDUAL).expect("節を切り出せる");
    let gaps = missing(THREAT_MODEL, &block, &LOOPBACK_RESIDUAL, "残存リスクの節");
    assert!(
        gaps.iter()
            .all(|g| g.starts_with(&format!("{THREAT_MODEL}:1 "))),
        "節の行番号を名指しできていない: {gaps:?}"
    );
    assert!(
        gaps.iter().any(|g| g.contains("受容するリスク")),
        "旧記述に足りないものを挙げていない: {gaps:?}"
    );

    // 注入 2: 旧「listen 範囲」節（同じ節の別の行に在る条件で免責されないこと）
    let stale_listen = "### daemon の listen 範囲\n\
        \n\
        - daemon は 127.0.0.1 のループバック TCP で待ち受ける（#1038）。\n\
        \u{20} `TAKO_REMOTE_ENDPOINT=unix` で UDS（0600）へ戻せる（#287 P1-2 の形）\n\
        - LAN 上の別端末・同一ホストの別ユーザーからは接続不能（OS のファイルパーミッションで強制）\n\
        - serve 経由（tailnet 内）のアクセスのみが到達する\n";
    let claims = unconditional_claims(THREAT_MODEL, stale_listen);
    assert_eq!(claims.len(), 1, "注入 2 を拾えていない: {claims:?}");
    assert!(
        claims[0].starts_with(&format!("{THREAT_MODEL}:5 ")),
        "注入 2 の行番号を名指しできていない: {claims:?}"
    );

    // 注入 3: `**既定**` の印を付け替えた doc（どちら向きも読めて、
    // コードの既定と逆向きなら必ず食い違うこと = 既定を変える側も縛られる）
    let marked_loopback =
        "//! - ループバック TCP（`127.0.0.1:<エフェメラルポート>`）= **既定**。\n\
        //! - Unix domain socket（0600）= 互換経路。\n";
    let marked_unix = "//! - ループバック TCP（`127.0.0.1:<エフェメラルポート>`）= 互換経路。\n\
        //! - Unix domain socket（0600）= **既定**。\n";
    assert_eq!(
        doc_marked_default(marked_loopback),
        (1, Kind::Loopback),
        "注入 3: ループバック側の印を読めていない"
    );
    assert_eq!(
        doc_marked_default(marked_unix),
        (2, Kind::Unix),
        "注入 3: UDS 側の印を読めていない"
    );
    let opposite = match code_default() {
        Kind::Loopback => marked_unix,
        Kind::Unix => marked_loopback,
    };
    assert_ne!(
        doc_marked_default(opposite).1,
        code_default(),
        "注入 3 が現行の既定と食い違わない = 検出力が無い"
    );

    // 合格の形は 1 件も挙がらない（誤検知の確認）
    let fixed = "- **同一ホストの別ユーザーからの接続は、既定のループバック TCP では防げない**。\n\
        \u{20} 遮断できるのは `TAKO_REMOTE_ENDPOINT=unix` を明示した UDS のときだけ\n\
        \n\
        - v0.8.1 以前は UDS が既定だったので「別 OS ユーザーからは接続不能」と書いてあった\n";
    assert!(
        unconditional_claims(THREAT_MODEL, fixed).is_empty(),
        "合格の形（条件つき / 過去の話）を落としている"
    );
}

/// 走査先を取り違えて**何も見ていない番犬**になっていないこと
#[test]
fn 番犬が走査対象を見つけている() {
    let src = read(THREAT_MODEL);
    for header in [SECTION_TRADEOFF, SECTION_LISTEN, SECTION_RESIDUAL] {
        let block = md_section(&src, header).unwrap_or_else(|| panic!("節 {header:?} が無い"));
        assert!(
            block.text.lines().count() >= 4,
            "{THREAT_MODEL}:{} 節 {header:?} が {} 行しか切り出せていない = 切り出しが壊れている",
            block.line,
            block.text.lines().count()
        );
    }
    let all = blocks(&src);
    assert!(
        all.len() >= 40,
        "塊が {} 個しか割れていない = 走査が壊れている",
        all.len()
    );
    assert!(
        all.iter()
            .any(|b| b.text.contains("別 OS ユーザー") || b.text.contains("別ユーザー")),
        "別 OS ユーザーに触れる塊を 1 つも見つけていない = 走査が壊れている"
    );
    let endpoint_src = read(LOCAL_ENDPOINT_RS);
    for anchor in ["pub enum EndpointSpec", "pub fn unix_supported"] {
        assert!(
            endpoint_src.contains(anchor),
            "{LOCAL_ENDPOINT_RS} に `{anchor}` が無い = 番犬の前提が崩れている（改名したら\n\
             {THREAT_MODEL} の記述と needle も直す）"
        );
    }
}
