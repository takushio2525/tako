//! system prompt へ注入するプラットフォーム事実（設計 §4「単一ソース化」）
//!
//! **狙い**: `master-system-windows.md` のようなプラットフォーム別の正本複製を作らないこと。
//! 複製は必ずドリフトする。正本は 1 本に保ち、差分は**レンダリング時に注入**する。
//!
//! 正本に置くプレースホルダは `{{platform_notes}}` の **1 種類だけ**。
//! 縮退している機能の一覧は対応マトリクス（#515）から自動生成するので、
//! 機能が増減しても prompt 側の記述を直す必要がない。

use tako_core::i18n::{self, Lang};
use tako_core::platform::support::{self, Note, Platform};

/// 正本テンプレートに置いてよい唯一のプレースホルダ
pub const PLACEHOLDER: &str = "{{platform_notes}}";

/// prompt に注入するプラットフォーム事実
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlatformFacts {
    /// 人が読む OS 名
    pub os_label: &'static str,
    /// 既定シェルの呼び名（コマンド例の前提を書くために使う）
    pub shell_label: &'static str,
    /// データ配置の書き方。**実パスではなく表記例**を使う
    /// （prompt にホームパスを埋め込まないため）
    pub data_dir_hint: &'static str,
    /// 縮退している機能の理由（対応マトリクスから自動生成。重複は畳み済み）。
    /// **解決済みの文字列ではなく `Note` のまま持つ**。早期に解決すると
    /// その時点の言語で凍結し、言語切替に追従しなくなる
    pub degraded: Vec<Note>,
}

impl PlatformFacts {
    /// 実行中のプラットフォームの事実
    pub fn current() -> Self {
        Self::for_platform(Platform::current())
    }

    /// 指定プラットフォームの事実。
    /// **macOS 上から Windows 版を組み立てられる**ので、レンダリング結果を
    /// 実機なしでテストできる（設計 §3.1 の「判定は純粋関数」と同じ理由）
    pub fn for_platform(platform: Platform) -> Self {
        let (os_label, shell_label, data_dir_hint) = match platform {
            Platform::MacOs => ("macOS", "zsh", "~/Library/Application Support/tako"),
            Platform::Windows => ("Windows", "PowerShell", "%APPDATA%\\tako"),
        };
        Self {
            os_label,
            shell_label,
            data_dir_hint,
            degraded: support::degraded_note_items(platform),
        }
    }

    /// `{{platform_notes}}` に差し込む本文（実行中の表示言語）
    pub fn notes_section(&self) -> String {
        self.notes_section_in(i18n::lang())
    }

    /// 言語を明示しての本文。**言語グローバルに触らずテストできる**ようにするため、
    /// 実体はこちらの純粋関数に置く。
    ///
    /// 縮退が無ければ「全機能が使える」と 1 行で書く。あれば**件数と引き方だけ**を置き、
    /// 理由の全文は手順書 `platform`（[`Self::full_section_in`]）へ回す（#1571）。
    /// 一覧はマトリクスからの生成物なので、**縮退が 1 件増えるたびに prompt が伸びる**
    /// 構造だった（Windows で 4110 バイト = tako が作る側が 18944 バイトの取り分を
    /// 1.7〜2.7 KB 超え、そのぶん利用者の追記の取り分が削られていた）
    pub fn notes_section_in(&self, lang: Lang) -> String {
        // A/B（#1571）: 立てると理由の全文を prompt へ差し戻す = 移送前の姿。
        // 予算は外さないので、そのとき番犬が超過で落ちるのが「移送の効き」の実測になる
        if legacy_1571() {
            return self.full_section_in(lang);
        }
        let mut s = self.env_line_in(lang);
        if self.degraded.is_empty() {
            s.push_str(match lang {
                Lang::Ja => "この環境では tako の全機能が利用できます。\n",
                Lang::En => "All tako features are available here.\n",
            });
            return s;
        }
        let n = self.degraded.len();
        match lang {
            Lang::Ja => s.push_str(&format!(
                "\nこの環境では {n} 件の理由で使えない・機能が落ちる操作があります。\n\
                 操作が失敗したらまずここを疑い、理由の全文は\n\
                 `tako_orchestrator_guide({{ topic: \"platform\" }})` で引いてください\n\
                 （CLI: `tako orchestrator guide platform`）。\n\
                 機能ごとの最新の対応状況は `tako platform --status pending` で確認できます。\n"
            )),
            Lang::En => s.push_str(&format!(
                "\n{n} reasons make some operations unavailable or degraded here.\n\
                 Suspect this first when an operation fails, and fetch the full list with\n\
                 `tako_orchestrator_guide({{ topic: \"platform\" }})`\n\
                 (CLI: `tako orchestrator guide platform`).\n\
                 Run `tako platform --status pending` for the current per-feature status.\n"
            )),
        }
        s
    }

    /// 縮退の理由の**全文**（手順書 `platform` の本文。#1571）。
    ///
    /// #1571 より前に `{{platform_notes}}` へ載っていた本文そのもので、
    /// prompt 側の短縮形・手順書・A/B の 3 経路がこの 1 実装を共有する
    /// （2 か所に書くと「prompt には無いが手順書にはある」ズレが必ず出る）
    pub fn full_section(&self) -> String {
        self.full_section_in(i18n::lang())
    }

    /// 言語を明示しての全文（[`Self::full_section`] の純粋関数版）
    pub fn full_section_in(&self, lang: Lang) -> String {
        let mut s = self.env_line_in(lang);
        match lang {
            Lang::Ja => {
                if self.degraded.is_empty() {
                    s.push_str("この環境では tako の全機能が利用できます。\n");
                } else {
                    s.push_str(
                        "\nこの環境では次の理由で使えない・機能が落ちる操作があります。\n\
                         操作が失敗したらまずここを疑ってください。\n\n",
                    );
                    for note in &self.degraded {
                        s.push_str(&format!("- {}\n", note.text_in(lang)));
                    }
                    s.push_str(
                        "\n機能ごとの最新の対応状況は `tako platform --status pending` で確認できます。\n",
                    );
                }
            }
            Lang::En => {
                if self.degraded.is_empty() {
                    s.push_str("All tako features are available here.\n");
                } else {
                    s.push_str(
                        "\nSome operations are unavailable or degraded here for the following reasons.\n\
                         Suspect this first when an operation fails.\n\n",
                    );
                    for note in &self.degraded {
                        s.push_str(&format!("- {}\n", note.text_in(lang)));
                    }
                    s.push_str(
                        "\nRun `tako platform --status pending` for the current per-feature status.\n",
                    );
                }
            }
        }
        s
    }

    /// 冒頭の 1 行（OS 名・既定シェル・設定の置き場）。短縮形と全文が共有する
    fn env_line_in(&self, lang: Lang) -> String {
        match lang {
            Lang::Ja => format!(
                "実行環境は {} です（既定シェル: {}、設定の置き場: `{}`）。\n",
                self.os_label, self.shell_label, self.data_dir_hint
            ),
            Lang::En => format!(
                "This environment is {} (default shell: {}, config location: `{}`).\n",
                self.os_label, self.shell_label, self.data_dir_hint
            ),
        }
    }
}

/// #1571 の A/B: 立てると縮退の理由の**全文**を prompt へ差し戻す（= 移送前の姿）。
/// 予算は外さないので、そのとき `prompt_budget_1477` が超過で落ちるのが
/// 「移送しなければ予算を超えていた」ことの実測になる（#1154 / #1477 の A/B と同じ作法）
pub fn legacy_1571() -> bool {
    std::env::var_os("TAKO_1571_LEGACY").is_some_and(|v| !v.is_empty() && v != "0")
}

/// 正本テンプレートのプレースホルダを実際の事実で置き換える。
///
/// プレースホルダが無いテンプレート（ユーザーのカスタム prompt 等）はそのまま返す。
/// **置換はここ 1 箇所**なので、prompt の経路が増えてもこの関数を通せばよい
pub fn render(template: &str, facts: &PlatformFacts) -> String {
    render_in(template, facts, i18n::lang())
}

/// 言語を明示してのレンダリング（テスト用。言語グローバルに依存しない）
pub fn render_in(template: &str, facts: &PlatformFacts, lang: Lang) -> String {
    if !template.contains(PLACEHOLDER) {
        return template.to_string();
    }
    template.replace(PLACEHOLDER, facts.notes_section_in(lang).trim_end())
}

/// 実行中プラットフォームでのレンダリング（呼び出し側の定型を短くするための糖衣）
pub fn render_current(template: &str) -> String {
    render_on(template, Platform::current())
}

/// プラットフォームを明示してのレンダリング（表示言語は実行中のもの。#1571）。
///
/// **macOS 上から Windows 形の prompt を組める**ので、実機なしで
/// `SYSTEM_PROMPT_BASE_MAX_BYTES` の充足を測れる
pub fn render_on(template: &str, platform: Platform) -> String {
    render(template, &PlatformFacts::for_platform(platform))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 縮退一覧はマトリクスから自動生成される（prompt 側に手書きしない）
    #[test]
    fn 縮退一覧はマトリクスから生成される() {
        let win = PlatformFacts::for_platform(Platform::Windows);
        assert_eq!(
            win.degraded,
            support::degraded_note_items(Platform::Windows)
        );
        assert!(
            !win.degraded.is_empty(),
            "Windows は現状 pending があるはず"
        );
        let mac = PlatformFacts::for_platform(Platform::MacOs);
        assert!(mac.degraded.is_empty(), "macOS に縮退は無いはず");
    }

    /// **受け入れ条件 2**: 縮退が 1 件増えると注記も自動で 1 件増える。
    /// prompt 側の記述を直さなくてよいことの担保。
    /// #1571 で一覧の行き先が prompt から手順書（`full_section_in`）へ移ったので、
    /// 自動生成であることの拘束もそちらで見る
    #[test]
    fn 縮退が増えると注記も自動で増える() {
        let mut facts = PlatformFacts::for_platform(Platform::Windows);
        let before = facts.full_section_in(Lang::Ja);
        let before_lines = before.lines().filter(|l| l.starts_with("- ")).count();
        assert_eq!(before_lines, facts.degraded.len());

        facts
            .degraded
            .push(Note::new("追加の縮退理由", "extra degraded reason"));
        let after = facts.full_section_in(Lang::Ja);
        let after_lines = after.lines().filter(|l| l.starts_with("- ")).count();
        assert_eq!(after_lines, before_lines + 1, "注記が自動で増えていない");
        assert!(after.contains("追加の縮退理由"));
        // prompt 側（短縮形）は件数だけが増える = 理由文が 1 件増えても伸びない
        let short = facts.notes_section_in(Lang::Ja);
        assert!(
            short.contains(&format!("{} 件の理由", before_lines + 1)),
            "件数が追従していない: {short}"
        );
        assert!(
            !short.contains("追加の縮退理由"),
            "理由文が prompt に出ている"
        );
    }

    /// **#1571 の本体**: prompt へ入るのは件数と引き方だけで、理由文は 1 行も載らない。
    /// 全文は手順書側（`full_section_in`）にあり、**行は 1 本も失われていない**
    #[test]
    fn 短縮形は理由の全文を載せず引き方を残す() {
        for lang in [Lang::Ja, Lang::En] {
            let facts = PlatformFacts::for_platform(Platform::Windows);
            let short = facts.notes_section_in(lang);
            let full = facts.full_section_in(lang);
            assert!(
                !short.lines().any(|l| l.starts_with("- ")),
                "理由文が prompt に残っている: {short}"
            );
            assert!(short.len() * 3 < full.len(), "短くなっていない: {short}");
            // 引き方（手順書 topic）と最新状態の引き方（最簡形 CLI）が残っている
            assert!(short.contains("topic: \"platform\""), "{short}");
            assert!(
                short.contains("tako orchestrator guide platform"),
                "{short}"
            );
            assert!(short.contains("tako platform --status pending"), "{short}");
            // 全文側は 1 件も欠けていない（マトリクスとの機械照合）
            let listed: Vec<&str> = full.lines().filter_map(|l| l.strip_prefix("- ")).collect();
            let want: Vec<&str> = support::degraded_note_items(Platform::Windows)
                .iter()
                .map(|n| n.text_in(lang))
                .collect();
            assert_eq!(listed, want, "手順書の本文がマトリクスと一致しない");
        }
    }

    /// 縮退が無い環境（macOS）でも手順書の本文は空にしない
    /// （空を返すと「引いたのに何も無い」= 引き方を誤ったのか縮退が無いのか判らない）
    #[test]
    fn 縮退が無い環境の全文は全機能利用可と書く() {
        let mac = PlatformFacts::for_platform(Platform::MacOs);
        for lang in [Lang::Ja, Lang::En] {
            let full = mac.full_section_in(lang);
            assert!(!full.trim().is_empty());
            assert_eq!(full, mac.notes_section_in(lang), "縮退が無ければ短縮しない");
        }
    }

    /// **受け入れ条件 1**: 同じ正本から両プラットフォームを描き分けられ、
    /// 差分はプレースホルダ部分だけであること
    #[test]
    fn 同じ正本から両プラットフォームを描き分けられる() {
        let template = "# 共通の前置き\n\n{{platform_notes}}\n\n# 共通の後書き\n";
        for lang in [Lang::Ja, Lang::En] {
            let mac = render_in(
                template,
                &PlatformFacts::for_platform(Platform::MacOs),
                lang,
            );
            let win = render_in(
                template,
                &PlatformFacts::for_platform(Platform::Windows),
                lang,
            );
            assert_ne!(mac, win, "プラットフォームで内容が変わらない");
            for out in [&mac, &win] {
                assert!(out.starts_with("# 共通の前置き"));
                assert!(out.trim_end().ends_with("# 共通の後書き"));
                assert!(!out.contains(PLACEHOLDER), "プレースホルダが残っている");
            }
            // 差分はプレースホルダ部分だけ = 正本の前後がそのまま残っている
            let (prefix, suffix) = template.split_once(PLACEHOLDER).unwrap();
            for out in [&mac, &win] {
                assert!(out.starts_with(prefix), "正本の前半が変わっている");
                assert!(out.ends_with(suffix), "正本の後半が変わっている");
            }
            assert!(win.contains("Windows") && mac.contains("macOS"));
        }
    }

    #[test]
    fn プレースホルダが無いテンプレートは素通しする() {
        let t = "カスタム prompt（プレースホルダなし）";
        assert_eq!(render(t, &PlatformFacts::current()), t);
    }

    /// 注記も日英そろっていること（#435）
    #[test]
    fn 注記は表示言語に追従する() {
        let facts = PlatformFacts::for_platform(Platform::Windows);
        let en = facts.notes_section_in(Lang::En);
        let ja_text = facts.notes_section_in(Lang::Ja);
        assert!(en.contains("This environment is Windows"));
        assert!(ja_text.contains("実行環境は Windows"));
        assert!(
            !en.lines()
                .next()
                .unwrap()
                .chars()
                .any(|c| matches!(c as u32, 0x3040..=0x30FF | 0x4E00..=0x9FFF)),
            "英語の注記に日本語が残っている: {en}"
        );
    }

    /// #322「最も簡単なコマンドを提案する」: 案内するコマンドは最簡形
    #[test]
    fn 案内コマンドは最簡形() {
        for lang in [Lang::Ja, Lang::En] {
            let s = PlatformFacts::for_platform(Platform::Windows).notes_section_in(lang);
            assert!(s.contains("tako platform --status pending"));
            // 既定値で済む引数を足さない（--platform は省略時に実行中の環境になる）
            assert!(!s.contains("--platform windows"));
        }
    }
}
