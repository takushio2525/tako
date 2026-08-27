//! #1011 の実機 e2e / 計測ハーネス: `claude agents --json` の Node 起動を
//! 前段ガードと鮮度の用途分離でどれだけ削れたかを、**実物の設定と実物の claude** で測る。
//!
//! 実行（稼働中の claude と登録済みアカウントが要るので `--ignored`）:
//!
//! ```text
//! cargo test -p tako-control --test issue1011_agents_scan_cost_e2e \
//!   -- --ignored --nocapture --test-threads=1
//! ```
//!
//! 旧挙動との A/B は同一バイナリで `TAKO_1011_LEGACY=1` を付けるだけ。
//! CPU は測定側で `/usr/bin/time -l` を被せて取る（`claude` は待ち合わせる子なので
//! 子プロセスの CPU も rusage に載る）。
//!
//! **出力にパスを出さない**（public リポの Issue / PR へそのまま貼るため。#927）。

use std::time::{Duration, Instant};

use tako_control::claude_session::LedgerRead;
use tako_control::orchestrator::{
    agent_scan_targets, agents_scan_counters, agents_scan_plan, AgentScanFreshness,
    AgentScanTarget, ScanDecision,
};

/// 走査先の呼び名（**パスは出さない**）
fn label(index: usize, target: &AgentScanTarget) -> String {
    match target {
        AgentScanTarget::Default => "既定(config dir 明示 unset)".to_string(),
        AgentScanTarget::ConfigDir(_) => format!("明示アカウント#{index}"),
    }
}

fn ledger_label(read: &LedgerRead) -> String {
    match read {
        LedgerRead::Missing => "台帳ディレクトリ無し".to_string(),
        LedgerRead::Unreadable => "台帳が読めない".to_string(),
        LedgerRead::Live(v) => format!("台帳 live {} 件", v.len()),
    }
}

/// 走査計画を実物の設定で出す（Node は起こさない = 何も壊さない読み取りだけ）
#[test]
#[ignore = "実物の accounts.yaml と claude の台帳が要る（実機 e2e）"]
fn 実設定での走査計画とガードの効きを出す() {
    let accounts = tako_control::orchestrator::AccountsConfig::load()
        .expect("accounts.yaml を読めない（実物の設定が要る）");
    let targets = agent_scan_targets(&accounts);
    let dirs: Vec<_> = targets.iter().map(|t| t.config_dir_path()).collect();
    let ledgers: Vec<LedgerRead> = tako_control::claude_session::read_ledgers(&dirs);
    let plan = agents_scan_plan(&targets, &ledgers, true);

    println!("走査先 {} 件", targets.len());
    for (i, ((t, l), d)) in targets
        .iter()
        .zip(ledgers.iter())
        .zip(plan.iter())
        .enumerate()
    {
        println!(
            "  [{i}] {:<28} {:<16} -> {}",
            label(i, t),
            ledger_label(l),
            match d {
                ScanDecision::Launch => "起こす",
                ScanDecision::SkipProvablyEmpty => "省く",
            }
        );
    }
    let launches = plan.iter().filter(|d| **d == ScanDecision::Launch).count();
    println!(
        "計画: 起こす {launches} / 全 {} 件（旧挙動は常に {} 件）",
        targets.len(),
        targets.len()
    );
    assert!(
        plan.first() == Some(&ScanDecision::Launch),
        "既定 config dir を省いてはいけない（#571）"
    );
}

/// 実際に走査を通し、起こした Node の本数を数える。
/// `TAKO_1011_LEGACY=1` を付けた実行と比べるのが A/B
#[test]
#[ignore = "実 claude を起こす（実機 e2e）"]
fn 走査1回で起こしたnodeの本数を数える() {
    let legacy = std::env::var_os("TAKO_1011_LEGACY").is_some();
    let targets = agent_scan_targets(
        &tako_control::orchestrator::AccountsConfig::load()
            .expect("accounts.yaml を読めない（実物の設定が要る）"),
    );
    let (l0, s0, _) = agents_scan_counters();
    let at = Instant::now();
    let agents = tako_control::agents::list_agents();
    let elapsed = at.elapsed();
    let (l1, s1, trusted) = agents_scan_counters();

    println!(
        "arm={} 走査先={} 起こした Node={} 省いた={} 台帳を信頼={} 所要={:?} 結果={} 件",
        match legacy {
            true => "legacy(#1011 前)",
            false => "new(#1011)",
        },
        targets.len(),
        l1 - l0,
        s1 - s0,
        trusted,
        elapsed,
        agents.as_ref().map(|a| a.len()).unwrap_or(0)
    );
    if let Err(e) = &agents {
        println!("  取得失敗: {e}");
    }

    match legacy {
        true => assert_eq!(l1 - l0, targets.len() as u64, "legacy では全走査先を起こす"),
        false => assert!(
            l1 - l0 <= targets.len() as u64,
            "起こす本数が走査先より多いことはない"
        ),
    }
}

/// 鮮度の用途分離が実際に再走査を止めているか。
///
/// 手順: 監視で 1 回走らせ（cold）→ 6 秒待つ → **UI で引くと起こさない**
/// （6 秒 < 30 秒）→ 監視で引くと起こす（6 秒 > 5 秒）。
/// legacy では UI でも起こしてしまうことを同じ手順で見る
#[test]
#[ignore = "実 claude を起こす（実機 e2e。6 秒待つ）"]
fn ui鮮度は監視の走査に相乗りして再走査を起こさない() {
    let legacy = std::env::var_os("TAKO_1011_LEGACY").is_some();

    let (a0, _, _) = agents_scan_counters();
    let _ = tako_control::agents::list_agents_with_freshness(AgentScanFreshness::Monitoring);
    let (a1, _, _) = agents_scan_counters();
    let cold = a1 - a0;
    println!("cold（監視）で起こした Node={cold}");

    std::thread::sleep(Duration::from_secs(6));

    let _ = tako_control::agents::list_agents_with_freshness(AgentScanFreshness::Ui);
    let (a2, _, _) = agents_scan_counters();
    let ui = a2 - a1;

    let _ = tako_control::agents::list_agents_with_freshness(AgentScanFreshness::Monitoring);
    let (a3, _, _) = agents_scan_counters();
    let monitoring = a3 - a2;

    println!("6 秒後: UI で起こした Node={ui} / その直後に監視で起こした Node={monitoring}");
    match legacy {
        true => assert!(
            ui > 0,
            "legacy は鮮度を無視して 5 秒 TTL 固定なので UI でも起こす"
        ),
        false => {
            assert_eq!(ui, 0, "UI（30 秒）は 6 秒前の結果に相乗りする");
            assert!(monitoring > 0, "監視（5 秒）は 6 秒前の結果では足りない");
        }
    }
}

/// 自己検証の検出力: 台帳が CLI の結果を取りこぼしたら**以後ガードを使わない**。
///
/// 上流のレイアウト変更をこちらで再現できないので、`TAKO_1011_INJECT_LEDGER_GAP=1` で
/// 「台帳が CLI の知っている pid を 1 つ知らない」状況を作る（#858 と同じ作法）。
///
/// ```text
/// TAKO_1011_INJECT_LEDGER_GAP=1 cargo test -p tako-control \
///   --test issue1011_agents_scan_cost_e2e -- --ignored --nocapture \
///   --exact '台帳の取りこぼしを検出したらガードを止める'
/// ```
#[test]
#[ignore = "実 claude を起こす + 故障注入が要る（検出力の実証）"]
fn 台帳の取りこぼしを検出したらガードを止める() {
    assert!(
        std::env::var_os("TAKO_1011_INJECT_LEDGER_GAP").is_some(),
        "TAKO_1011_INJECT_LEDGER_GAP=1 を付けて実行する（このテストは故障注入の検出力を見る）"
    );
    let targets = agent_scan_targets(
        &tako_control::orchestrator::AccountsConfig::load()
            .expect("accounts.yaml を読めない（実物の設定が要る）"),
    );

    // 1 回目: ガードは効くが、起こした走査先で取りこぼしを検出して信頼を落とす
    let (l0, s0, t0) = agents_scan_counters();
    assert!(t0, "初期状態は台帳を信頼している");
    let _ = tako_control::agents::list_agents_with_freshness(AgentScanFreshness::Monitoring);
    let (l1, s1, t1) = agents_scan_counters();
    println!(
        "1 回目: 起こした={} 省いた={} → 台帳を信頼={t1}",
        l1 - l0,
        s1 - s0
    );
    assert!(
        !t1,
        "取りこぼしを検出したら信頼を落とす（= 旧挙動へ自動復帰）"
    );

    // 2 回目（鮮度窓を跨がせる）: もうガードは働かず全走査先を起こす
    std::thread::sleep(Duration::from_secs(6));
    let _ = tako_control::agents::list_agents_with_freshness(AgentScanFreshness::Monitoring);
    let (l2, s2, _) = agents_scan_counters();
    println!("2 回目: 起こした={} 省いた={}", l2 - l1, s2 - s1);
    assert_eq!(
        l2 - l1,
        targets.len() as u64,
        "信頼を落としたあとは全走査先を起こす"
    );
    assert_eq!(s2 - s1, 0, "信頼を落としたあとは 1 件も省かない");
}

/// **両アームで結果が 1 バイトも変わらない**ことを差分で見るための安定ダンプ。
///
/// `claude agents --json` の解釈は 1 行も変えていないので、同じ瞬間に測れば
/// 出力は一致するはず。`session_id` で並べ替えて出すので `diff` にかけられる
/// （`cwd` / `name` は実パス・実ユーザー名を含みうるので**出さない**。#927）。
///
/// ```text
/// for arm in new legacy; do … --exact '走査結果の安定ダンプを出す' > /tmp/$arm.txt; done
/// diff /tmp/new.txt /tmp/legacy.txt
/// ```
#[test]
#[ignore = "実 claude を起こす（両アームの結果比較用）"]
fn 走査結果の安定ダンプを出す() {
    let agents = tako_control::agents::list_agents().expect("走査できる");
    let mut rows: Vec<String> = agents
        .iter()
        .map(|a| {
            format!(
                "session_id={} status={} kind={} pid={} ctx={} model={}",
                a["session_id"], a["status"], a["kind"], a["pid"], a["ctx_percent"], a["model"]
            )
        })
        .collect();
    rows.sort();
    println!("DUMP_BEGIN count={}", rows.len());
    for r in &rows {
        println!("{r}");
    }
    println!("DUMP_END");
}
