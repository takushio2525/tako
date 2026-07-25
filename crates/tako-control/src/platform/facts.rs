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

    /// `{{platform_notes}}` に差し込む本文。
    ///
    /// 縮退が無ければ「全機能が使える」と 1 行で書く。あれば理由を列挙し、
    /// 最新の状態を自分で引く手段（#322 の最簡形コマンド）を案内する
    /// 実行中の表示言語での本文
    pub fn notes_section(&self) -> String {
        self.notes_section_in(i18n::lang())
    }

    /// 言語を明示しての本文。**言語グローバルに触らずテストできる**ようにするため、
    /// 実体はこちらの純粋関数に置く
    pub fn notes_section_in(&self, lang: Lang) -> String {
        let mut s = String::new();
        match lang {
            Lang::Ja => {
                s.push_str(&format!(
                    "実行環境は {} です（既定シェル: {}、設定の置き場: `{}`）。\n",
                    self.os_label, self.shell_label, self.data_dir_hint
                ));
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
                s.push_str(&format!(
                    "This environment is {} (default shell: {}, config location: `{}`).\n",
                    self.os_label, self.shell_label, self.data_dir_hint
                ));
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
    render(template, &PlatformFacts::current())
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
    /// prompt 側の記述を直さなくてよいことの担保
    #[test]
    fn 縮退が増えると注記も自動で増える() {
        let mut facts = PlatformFacts::for_platform(Platform::Windows);
        let before = facts.notes_section_in(Lang::Ja);
        let before_lines = before.lines().filter(|l| l.starts_with("- ")).count();
        assert_eq!(before_lines, facts.degraded.len());

        facts
            .degraded
            .push(Note::new("追加の縮退理由", "extra degraded reason"));
        let after = facts.notes_section_in(Lang::Ja);
        let after_lines = after.lines().filter(|l| l.starts_with("- ")).count();
        assert_eq!(after_lines, before_lines + 1, "注記が自動で増えていない");
        assert!(after.contains("追加の縮退理由"));
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
