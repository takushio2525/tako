//! 復元の内訳（Issue #1554 / FR-5.7.1）。
//! **「復元成功: N タブ / M ペイン（…）」が M を余さず説明する**
//!
//! # なぜ 1 実装で持つか
//!
//! #1554 以前の内訳は復元ループの中の 4 カウンタ（再 attach / Claude resume /
//! 新規シェル / プレビュー）だけで、**どのカウンタにも入らない結末が 5 つ**あった:
//! たまり場ペイン（FR-2.15.5）・退避タブ配下のペイン（#1487）・Web ビューペイン・
//! `spawn_session` の失敗・resume 入力の宛先（PTY）が見つからないペイン。どれも
//! `continue` で抜けるので、内訳の合計が M より小さい行が残っていた。
//!
//! ```text
//! 復元成功: 10 タブ / 23 ペイン（tmux 再 attach 17 / Claude resume 0 / 新規シェル 0 / プレビュー 5）
//!                    ^^ 23                       17 + 0 + 0 + 5 = 22 ← 1 件がどこにも居ない
//! ```
//!
//! **実測（2026-09-22・本番 persist.log 52 行）で差の正体は「たまり場・退避」だった**:
//! 合わない 8 行はすべて #1487（退避タブ）が着地した 9/21 以降で、そのときの
//! `layout.json` は 17 ペインのうち 1 件がたまり場・1 件が退避タブ配下（Web ビューと
//! プレビューは 0 件）。これらは**表に出すときに起こす**設計なので `spawn_session` を
//! 通らないのが正しく、**失敗として数えてはいけない**（毎起動 `復元失敗 2` と出ると
//! 本当の失敗が埋もれる）。だから「復元しなかった」と「復元できなかった」を
//! 別のカテゴリで数える。
//!
//! 一方、本当の失敗（`spawn_session` の `Err`）は `eprintln!` 止まり（GUI の stderr は
//! どこにも出ない = `log show --predicate 'process == "tako"'` で 0 件）で
//! persist.log に痕跡が無かった。
//! AGENTS.md が「再起動後にエージェントが戻らないとき」の**正本として案内している
//! 診断がこの行**なので、説明できないペインが 1 件でもあると調査がそこで止まる。
//!
//! # 落とし忘れが起きない形
//!
//! 内訳の区間（ラベルと件数）は [`RestoreBreakdown::segments`] の 1 本が正本で、
//! 合計（[`RestoreBreakdown::total`]）も 1 行目（[`RestoreBreakdown::summary`]）も
//! そこから作る。**カテゴリを 1 つ落とすと合計も落ちる**ので、「表示だけ増やして
//! 数え忘れる」形にならない。それでも新しい枝が記録を忘れたときのために、
//! 合計と M の食い違いは [`RestoreBreakdown::mismatch`] が**実行時に**1 行残す
//! （二度と無言で壊れない）。記録し忘れた `continue` は番犬
//! `crates/tako-control/tests/issue1554_restore_breakdown_watchdog.rs` が
//! file:line で名指す。
//!
//! # 診断へ載せるのは分類だけ
//!
//! ペイン内容・送信テキスト・トークンは載せない（`.agent/conventions.md` の
//! 診断ログ規約）。個別の失敗行が持つのは**ペイン ID と理由の分類**、そして
//! 起動失敗のときだけ OS / spawn のエラー文（パス止まり）である。

use std::collections::BTreeMap;

use tako_core::agent_support::Agent;

use crate::sessions::FreshShellReason;

/// #1554 で足した 3 カテゴリのラベル。A/B（[`legacy`]）では 1 行目から隠して
/// 修正前の「合計が合わない行」を同一バイナリで再現する
pub const LABEL_WEBVIEW: &str = "Web ビュー";
/// 同上（タブ配下に居ないペイン = たまり場・退避タブ配下の件数）
pub const LABEL_HIDDEN: &str = "たまり場・退避";
/// 同上（復元できなかったペインの件数）
pub const LABEL_FAILED: &str = "復元失敗";

/// #1554 の A/B。`TAKO_1554_LEGACY=1` で修正前の挙動
/// （1 行目は 4 カテゴリのみ = 合計が M と合わない・個別の失敗は残らない）
pub fn legacy() -> bool {
    std::env::var_os("TAKO_1554_LEGACY").is_some()
}

/// 復元できなかった理由。**分類だけ**を持つ（ペイン内容は持たない）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailureReason {
    /// layout には載っているのに、ワークスペースへ同じ ID のペインが無い
    /// （layout の復元と spawn ループの食い違い）
    NotPlaced,
    /// PTY / バックエンドを起こせない（`spawn_session` の失敗）
    SpawnFailed,
    /// 起こせたのに resume 入力の宛先（PTY）が見つからない。
    /// 構造上の保険（`spawn_session` は成功すれば必ず登録する）だが、
    /// **ここを数えないと「エージェントが戻らない」1 件が無言で消える**
    ResumeNotDelivered,
}

impl FailureReason {
    /// 列挙の正本（テスト・網羅の基準）
    pub const ALL: [FailureReason; 3] =
        [Self::NotPlaced, Self::SpawnFailed, Self::ResumeNotDelivered];

    /// 診断ログ用の日本語ラベル（`FreshShellReason::label` と同じ役目）
    pub const fn label(self) -> &'static str {
        match self {
            Self::NotPlaced => "ワークスペースに配置されていない",
            Self::SpawnFailed => "起動できない",
            Self::ResumeNotDelivered => "resume 入力の宛先が無い",
        }
    }
}

/// タブ配下に居ないペインの種別（#1554）。**復元の失敗ではない**（どちらも
/// 「表に出すときに起こす」設計で、`spawn_session` を通らないのが正しい）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HiddenKind {
    /// たまり場ペイン（FR-2.15.5 / FR-2.15.6）
    Backgrounded,
    /// 退避タブ配下のペイン（#1487）
    ShelvedTab,
}

impl HiddenKind {
    /// 列挙の正本（テスト・網羅の基準）
    pub const ALL: [HiddenKind; 2] = [Self::Backgrounded, Self::ShelvedTab];

    /// 診断ログ用の日本語ラベル
    pub const fn label(self) -> &'static str {
        match self {
            Self::Backgrounded => "たまり場",
            Self::ShelvedTab => "退避タブ",
        }
    }
}

/// 1 ペインの結末。復元ループは **1 ペインについてちょうど 1 つ**記録する
/// （`continue` で抜ける枝も含む）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PaneOutcome {
    /// 器（tmux セッション）が生きていた = 実行中プロセスごと再 attach した
    Reattached,
    /// 保存済みの会話を明示 resume した（`with_role` = 役割 env / 起動条件つき）
    Resumed { agent: Agent, with_role: bool },
    /// 新しいシェルを開いた（理由つき）
    FreshShell(FreshShellReason),
    /// プレビューペイン（PTY を起こさない）
    Preview,
    /// Web ビューペイン（PTY を起こさない。wry の生成は初回 render まで遅延する）
    Webview,
    /// タブ配下に居ないので起こさなかった（たまり場 / 退避タブ配下）
    Hidden(HiddenKind),
    /// 復元できなかった
    Failed(FailureReason),
}

/// 復元の内訳。`record` で結末を積み、`summary` / `detail` / `mismatch` が
/// persist.log の行を作る
#[derive(Debug, Clone)]
pub struct RestoreBreakdown {
    /// A/B（[`legacy`]）を構築時に 1 回だけ読む。読む側が再計算しないのは
    /// #372 / #1473 と同じ方針
    legacy: bool,
    reattached: usize,
    /// 系統名 → (件数, 役割つきの件数)。claude も同じ表に入れる
    /// （1 行目での位置だけが決まっている）
    resumed: BTreeMap<&'static str, (usize, usize)>,
    fresh_shells: usize,
    /// 新規シェルへ落ちた理由の内訳（#1076）
    fresh_reasons: BTreeMap<&'static str, usize>,
    previews: usize,
    webviews: usize,
    /// タブ配下に居ないペインの内訳（たまり場 / 退避タブ）。件数の合計が
    /// `たまり場・退避` の件数（別に持つと食い違う）
    hidden: BTreeMap<&'static str, usize>,
    /// 失敗の理由の内訳。件数の合計が `復元失敗` の件数（別に持つと食い違う）
    failures: BTreeMap<&'static str, usize>,
}

impl RestoreBreakdown {
    /// 本番用（A/B は env から読む）
    pub fn new() -> Self {
        Self::with_legacy(legacy())
    }

    /// A/B を明示する（単体テストは env を触らずにこちらを使う）
    pub fn with_legacy(legacy: bool) -> Self {
        Self {
            legacy,
            reattached: 0,
            resumed: BTreeMap::new(),
            fresh_shells: 0,
            fresh_reasons: BTreeMap::new(),
            previews: 0,
            webviews: 0,
            hidden: BTreeMap::new(),
            failures: BTreeMap::new(),
        }
    }

    /// 修正前の挙動で動いているか（個別の失敗行を出すかの判断に呼び出し側が使う）
    pub fn is_legacy(&self) -> bool {
        self.legacy
    }

    /// 1 ペインの結末を記録する
    pub fn record(&mut self, outcome: PaneOutcome) {
        match outcome {
            PaneOutcome::Reattached => self.reattached += 1,
            PaneOutcome::Resumed { agent, with_role } => {
                let e = self.resumed.entry(agent.as_str()).or_insert((0, 0));
                e.0 += 1;
                e.1 += usize::from(with_role);
            }
            PaneOutcome::FreshShell(reason) => {
                self.fresh_shells += 1;
                *self.fresh_reasons.entry(reason.label()).or_insert(0) += 1;
            }
            PaneOutcome::Preview => self.previews += 1,
            PaneOutcome::Webview => self.webviews += 1,
            PaneOutcome::Hidden(kind) => {
                *self.hidden.entry(kind.label()).or_insert(0) += 1;
            }
            PaneOutcome::Failed(reason) => {
                *self.failures.entry(reason.label()).or_insert(0) += 1;
            }
        }
    }

    /// 復元できなかったペインの件数
    pub fn failed(&self) -> usize {
        self.failures.values().sum()
    }

    /// タブ配下に居ないので起こさなかったペインの件数（たまり場 + 退避タブ配下）
    pub fn hidden(&self) -> usize {
        self.hidden.values().sum()
    }

    /// resume できたペインの件数（系統をまとめた数）
    pub fn resumed_total(&self) -> usize {
        self.resumed.values().map(|(n, _)| n).sum()
    }

    fn resumed_of(&self, agent: Agent) -> (usize, usize) {
        self.resumed.get(agent.as_str()).copied().unwrap_or((0, 0))
    }

    /// 1 行目に並ぶ区間（ラベル, 件数）。**内訳の正本**で、合計も 1 行目もここから作る。
    ///
    /// 並びは #1554 以前の 4 カテゴリをそのままにして、末尾へ Web ビュー・
    /// たまり場・退避・復元失敗を足した形（既存の読み手・grep を壊さない）。
    /// **復元失敗は最後**（読む人が最初に見たい「悪い方」を行末で見つけられる）。
    /// claude 以外の resume は
    /// **0 件なら出さない**（codex / agy を使っていない環境の見え方を変えない）
    fn segments(&self) -> Vec<(String, usize)> {
        let mut out = vec![
            ("tmux 再 attach".to_string(), self.reattached),
            (
                "Claude resume".to_string(),
                self.resumed_of(Agent::Claude).0,
            ),
        ];
        for (agent, (n, _)) in &self.resumed {
            if *agent != Agent::Claude.as_str() {
                out.push((format!("{agent} resume"), *n));
            }
        }
        out.push(("新規シェル".to_string(), self.fresh_shells));
        out.push(("プレビュー".to_string(), self.previews));
        out.push((LABEL_WEBVIEW.to_string(), self.webviews));
        out.push((LABEL_HIDDEN.to_string(), self.hidden()));
        out.push((LABEL_FAILED.to_string(), self.failed()));
        out
    }

    /// 内訳の合計。**復元したペイン数と必ず一致する**のが #1554 の不変条件
    pub fn total(&self) -> usize {
        self.segments().iter().map(|(_, n)| n).sum()
    }

    /// persist.log の 1 行目（`tako persist` / MCP `tako_persist` の `last_restore`）
    pub fn summary(&self, tabs: usize, panes: usize) -> String {
        let inner: Vec<String> = self
            .segments()
            .into_iter()
            .filter(|(label, _)| {
                !(self.legacy
                    && matches!(label.as_str(), LABEL_WEBVIEW | LABEL_HIDDEN | LABEL_FAILED))
            })
            .map(|(label, n)| format!("{label} {n}"))
            .collect();
        format!(
            "復元成功: {tabs} タブ / {panes} ペイン（{}）",
            inner.join(" / ")
        )
    }

    /// persist.log の 2 行目（経路の内訳。#1076 / #1238 に #1554 の失敗理由を足したもの）。
    /// 数えるものが 1 件も無ければ [`None`]
    pub fn detail(&self) -> Option<String> {
        let (hidden, failed) = if self.legacy {
            (0, 0)
        } else {
            (self.hidden(), self.failed())
        };
        if self.resumed_total() + self.fresh_shells + hidden + failed == 0 {
            return None;
        }
        let (claude, claude_role) = self.resumed_of(Agent::Claude);
        let others: String = self
            .resumed
            .iter()
            .filter(|(agent, _)| **agent != Agent::Claude.as_str())
            .map(|(agent, (n, with_role))| {
                format!(
                    " / {agent} resume {n}（役割つき {with_role} / 役割なし {}）",
                    n - with_role
                )
            })
            .collect();
        let reasons = join_counts(&self.fresh_reasons);
        let hidden = if hidden == 0 {
            String::new()
        } else {
            format!(
                " / {LABEL_HIDDEN} {hidden}（{}）",
                join_counts(&self.hidden)
            )
        };
        let failures = if failed == 0 {
            String::new()
        } else {
            format!(
                " / {LABEL_FAILED} {failed}（{}）",
                join_counts(&self.failures)
            )
        };
        Some(format!(
            "復元の内訳: Claude resume {claude}（役割つき {claude_role} / 役割なし {}）\
             {others} / 新規シェル {}{}{hidden}{failures}",
            claude - claude_role,
            self.fresh_shells,
            if reasons.is_empty() {
                String::new()
            } else {
                format!("（{reasons}）")
            },
        ))
    }

    /// 内訳が全ペインを説明できていないときの 1 行（#1554 の再発を**実行時に**名指す）。
    /// 説明できていれば [`None`]
    pub fn mismatch(&self, panes: usize) -> Option<String> {
        if self.legacy {
            return None;
        }
        let total = self.total();
        (total != panes).then(|| {
            format!(
                "復元の内訳が全ペインを説明できていない: 内訳 {total} 件 / 復元 {panes} ペイン\
                 （差 {}）。結末を記録しない経路が残っている（#1554）",
                panes.abs_diff(total)
            )
        })
    }
}

impl Default for RestoreBreakdown {
    fn default() -> Self {
        Self::new()
    }
}

/// 個別の復元失敗を persist.log へ残す 1 行。
///
/// 載せるのは**ペイン ID と理由の分類**、および起動失敗のときだけ spawn / OS の
/// エラー文（`detail`）。ペイン内容・送信テキスト・トークンは載せない
/// （`.agent/conventions.md` の診断ログ規約）
pub fn failure_line(pane: u64, reason: FailureReason, detail: Option<&str>) -> String {
    match detail.map(str::trim).filter(|d| !d.is_empty()) {
        Some(detail) => format!(
            "{LABEL_FAILED}（ペイン {pane}）: {}: {detail}",
            reason.label()
        ),
        None => format!("{LABEL_FAILED}（ペイン {pane}）: {}", reason.label()),
    }
}

/// `ラベル N / ラベル M` へ畳む（並びは BTreeMap のラベル順 = 実行ごとに揺れない）
fn join_counts(counts: &BTreeMap<&'static str, usize>) -> String {
    counts
        .iter()
        .map(|(label, n)| format!("{label} {n}"))
        .collect::<Vec<_>>()
        .join(" / ")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 全カテゴリを 1 件ずつ含む内訳（網羅の基準）
    fn every_outcome() -> Vec<PaneOutcome> {
        let mut out = vec![
            PaneOutcome::Reattached,
            PaneOutcome::Resumed {
                agent: Agent::Claude,
                with_role: true,
            },
            PaneOutcome::Resumed {
                agent: Agent::Claude,
                with_role: false,
            },
            PaneOutcome::Resumed {
                agent: Agent::Codex,
                with_role: false,
            },
            PaneOutcome::Preview,
            PaneOutcome::Webview,
        ];
        out.extend(
            [
                FreshShellReason::NoSessionId,
                FreshShellReason::InvalidSessionId,
                FreshShellReason::TranscriptMissing,
                FreshShellReason::ResumeUnsupported,
            ]
            .map(PaneOutcome::FreshShell),
        );
        out.extend(HiddenKind::ALL.map(PaneOutcome::Hidden));
        out.extend(FailureReason::ALL.map(PaneOutcome::Failed));
        out
    }

    fn breakdown_of(outcomes: &[PaneOutcome]) -> RestoreBreakdown {
        let mut b = RestoreBreakdown::with_legacy(false);
        for o in outcomes {
            b.record(*o);
        }
        b
    }

    /// #1554 の不変条件。**記録した件数 = 内訳の合計**（カテゴリを 1 つ
    /// `segments` から落とすとここが落ちる）
    #[test]
    fn 内訳の合計は記録したペイン数と一致する() {
        let outcomes = every_outcome();
        let b = breakdown_of(&outcomes);
        assert_eq!(
            b.total(),
            outcomes.len(),
            "内訳の合計 {} が記録した {} ペインを説明していない（#1554）。\n\
             `segments` から落ちたカテゴリがある:\n{:#?}",
            b.total(),
            outcomes.len(),
            b.segments()
        );
        // 1 件も無い内訳も 0 で成立する（`restored` が空の起動）
        assert_eq!(RestoreBreakdown::with_legacy(false).total(), 0);
    }

    /// 1 行目が M を余さず説明する（AC1 の形）
    #[test]
    fn 一行目は全カテゴリを並べる() {
        let outcomes = every_outcome();
        let b = breakdown_of(&outcomes);
        let line = b.summary(3, outcomes.len());
        for (label, n) in b.segments() {
            assert!(
                line.contains(&format!("{label} {n}")),
                "1 行目にカテゴリ `{label}` が無い: {line}"
            );
        }
        assert!(line.starts_with(&format!("復元成功: 3 タブ / {} ペイン（", outcomes.len())));
        assert!(line.ends_with('）'));
    }

    /// 失敗 0 件の環境でも新カテゴリが 0 で並ぶ（従来の 4 カテゴリは位置も不変）
    #[test]
    fn 失敗ゼロでも新カテゴリが0で並ぶ() {
        let mut b = RestoreBreakdown::with_legacy(false);
        for _ in 0..17 {
            b.record(PaneOutcome::Reattached);
        }
        for _ in 0..5 {
            b.record(PaneOutcome::Preview);
        }
        assert_eq!(
            b.summary(10, 22),
            "復元成功: 10 タブ / 22 ペイン（tmux 再 attach 17 / Claude resume 0 / \
             新規シェル 0 / プレビュー 5 / Web ビュー 0 / たまり場・退避 0 / 復元失敗 0）"
        );
        assert_eq!(b.total(), 22);
        assert_eq!(b.mismatch(22), None);
        // 数えるものが無ければ 2 行目は出さない（従来と同じ条件）
        assert_eq!(b.detail(), None);
    }

    /// Issue #1554 の実測行（22 ≠ 23）は、**たまり場・退避**を数えると説明される。
    /// 本番 persist.log で合わない 8 行はすべて #1487（退避タブ）着地以降で、
    /// そのときの layout.json は Web ビュー 0 件・たまり場 1 件・退避タブ配下 1 件だった
    #[test]
    fn 実測の食い違いはたまり場と退避を数えると解消する() {
        let mut b = RestoreBreakdown::with_legacy(false);
        for _ in 0..17 {
            b.record(PaneOutcome::Reattached);
        }
        for _ in 0..5 {
            b.record(PaneOutcome::Preview);
        }
        b.record(PaneOutcome::Hidden(HiddenKind::ShelvedTab));
        assert_eq!(b.total(), 23, "22 ≠ 23 の 1 件が説明された");
        assert_eq!(b.mismatch(23), None);
        let line = b.summary(10, 23);
        assert!(line.contains("たまり場・退避 1"), "{line}");
        // **失敗として数えない**（毎起動 `復元失敗 1` と出ると本当の失敗が埋もれる）
        assert!(line.contains("復元失敗 0"), "{line}");
        assert_eq!(b.failed(), 0);
        assert_eq!(
            b.detail().as_deref(),
            Some(
                "復元の内訳: Claude resume 0（役割つき 0 / 役割なし 0） / 新規シェル 0 / \
                 たまり場・退避 1（退避タブ 1）"
            )
        );
    }

    /// 2 行目は失敗の理由を言う（#1076 / #1238 の形は保つ）
    #[test]
    fn 二行目は経路と失敗の理由を言う() {
        let b = breakdown_of(&[
            PaneOutcome::Resumed {
                agent: Agent::Claude,
                with_role: true,
            },
            PaneOutcome::Resumed {
                agent: Agent::Codex,
                with_role: false,
            },
            PaneOutcome::FreshShell(FreshShellReason::NoSessionId),
            PaneOutcome::Hidden(HiddenKind::Backgrounded),
            PaneOutcome::Failed(FailureReason::SpawnFailed),
            PaneOutcome::Failed(FailureReason::NotPlaced),
        ]);
        let detail = b.detail().expect("2 行目");
        assert_eq!(
            detail,
            "復元の内訳: Claude resume 1（役割つき 1 / 役割なし 0） / \
             codex resume 1（役割つき 0 / 役割なし 1） / 新規シェル 1（ID なし 1） / \
             たまり場・退避 1（たまり場 1） / \
             復元失敗 2（ワークスペースに配置されていない 1 / 起動できない 1）"
        );
    }

    /// 失敗だけがあるときも 2 行目が出る（1 行目の件数だけでは理由が読めない）
    #[test]
    fn 失敗だけでも二行目が出る() {
        let b = breakdown_of(&[PaneOutcome::Failed(FailureReason::SpawnFailed)]);
        assert_eq!(
            b.detail().as_deref(),
            Some(
                "復元の内訳: Claude resume 0（役割つき 0 / 役割なし 0） / 新規シェル 0 / \
                 復元失敗 1（起動できない 1）"
            )
        );
    }

    /// 説明できないペインが残ったら実行時に名指す（二度と無言で壊れない）
    #[test]
    fn 内訳が全ペインを説明できないときだけ名指す() {
        let b = breakdown_of(&[PaneOutcome::Reattached, PaneOutcome::Preview]);
        assert_eq!(b.mismatch(2), None);
        let line = b.mismatch(3).expect("食い違いを名指す");
        assert!(line.contains("内訳 2 件 / 復元 3 ペイン"), "{line}");
        assert!(line.contains("差 1"), "{line}");
        assert!(line.contains("#1554"), "{line}");
    }

    /// 個別の失敗行はペイン ID と分類だけ（内容は載せない）
    #[test]
    fn 個別の失敗行はペインidと分類を載せる() {
        assert_eq!(
            failure_line(
                1964,
                FailureReason::SpawnFailed,
                Some("No such file or directory")
            ),
            "復元失敗（ペイン 1964）: 起動できない: No such file or directory"
        );
        assert_eq!(
            failure_line(12, FailureReason::NotPlaced, None),
            "復元失敗（ペイン 12）: ワークスペースに配置されていない"
        );
        // 空の詳細は「: 」で終わる行にしない
        assert_eq!(
            failure_line(12, FailureReason::ResumeNotDelivered, Some("   ")),
            "復元失敗（ペイン 12）: resume 入力の宛先が無い"
        );
        for reason in FailureReason::ALL {
            assert!(!reason.label().is_empty());
        }
        for kind in HiddenKind::ALL {
            assert!(!kind.label().is_empty());
        }
    }

    /// A/B: `TAKO_1554_LEGACY=1` 相当は修正前の症状を再現する
    /// （1 行目は 4 カテゴリで合計が合わない・食い違いも黙る）
    #[test]
    fn 旧挙動は合計が合わない行を再現する() {
        let mut b = RestoreBreakdown::with_legacy(true);
        for _ in 0..17 {
            b.record(PaneOutcome::Reattached);
        }
        for _ in 0..5 {
            b.record(PaneOutcome::Preview);
        }
        b.record(PaneOutcome::Webview);
        b.record(PaneOutcome::Hidden(HiddenKind::ShelvedTab));
        assert_eq!(
            b.summary(10, 24),
            "復元成功: 10 タブ / 24 ペイン（tmux 再 attach 17 / Claude resume 0 / \
             新規シェル 0 / プレビュー 5）"
        );
        assert_eq!(b.mismatch(24), None, "旧挙動は食い違いを名指さない");
        b.record(PaneOutcome::Failed(FailureReason::SpawnFailed));
        assert_eq!(b.detail(), None, "旧挙動は失敗・たまり場の内訳を出さない");
    }
}
