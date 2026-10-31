//! GUI が起動の最初に自分の env から外す「ペインの中でだけ意味を持つ env」（#2570）。
//!
//! tako のペインの中から GUI を起こし直すと（master が install の後に GUI を起こし直す・worker が検証で
//! 起こす）、GUI のプロセスはそのペインの env を丸ごと受け継ぐ。ペインの識別（`TAKO_PANE_ID`）・制御
//! プレーンへの接続・mod の起床条件・claude のセッションの印・claude の Bash の子の印（`GIT_EDITOR=true`）・
//! シェル統合の注入（世代ごとに 1 段ずつ重なる `XDG_DATA_DIRS`）が GUI に残り、ペインは「GUI の env +
//! 注入」で作るので新しいペインへ、GUI がペインの外で起こす子（自動命名の `claude -p`・`claude mcp add`）
//! へも素通しになっていた（本番の GUI で `XDG_DATA_DIRS` の fish-data が 6 段・新しいペインの
//! `git commit` がエディタを開かずに中止される形）。
//!
//! **外す名前の正本はこの表 1 つ**（[`STRIP`]）。外さないと決めた名前と理由は [`KEPT`]（外しすぎの
//! 見張り = 両方に載る名前はテストが落とす）。表は OS で分けない（Windows も同じ表を引く。区切りは
//! [`crate::claude_mod::LIST_SEP`]）。理由の文は日英（`why`）。
//!
//! 外すのは [`plan`] が決め、GUI は `main` の最初（器の段 = `__tako-container-exec` の後・スレッドを
//! 起こす前）に [`apply_at_startup`] で当て、unix では続けて [`reexec_if_stripped`] で外した env の
//! まま自分を exec し直す（同じ pid）。exec の時点の env の塊（`ps eww`・macOS の `KERN_PROCARGS2`・
//! Linux の `/proc/<pid>/environ`）はプロセスの中で書き換えても変わらず、資源の走査
//! （`resources::env_pane_of`）が GUI 自身と、ペインの根へ辿れない GUI の子（言語サーバ・自動命名）を
//! 起こしたペインのプロセスとして数えるため。Windows は exec が無いのでプロセスの中で外すだけ（ペインと
//! 子へは届かない。資源の走査は Windows では未実装 = #2000）。A/B は [`LEGACY_ENV`]（何も外さず起こし
//! 直さない = #2570 前の形）

use std::ffi::OsString;

/// 何も外さない A/B の入口（#2570 前の形 = GUI は起こした側の env をそのまま持つ）
pub const LEGACY_ENV: &str = "TAKO_2570_LEGACY";
/// 起こし直した後の自分へ「外したもの」を渡す印（[`reexec_if_stripped`] が立て、[`apply_at_startup`] が
/// 読んで消す）。値は起こしたペインの番号と外した名前だけ（値は載せない）
pub const REEXEC_ENV: &str = "TAKO_2570_REEXEC";

/// 理由の文（日英）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Why {
    pub ja: &'static str,
    pub en: &'static str,
}

/// いつ外すか
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum When {
    /// いつでも外す（tako のペインか claude のセッションの中にしか無い名前）
    Always,
    /// claude の Bash の子から起こされたとき（`CLAUDECODE` がある）だけ。`value` があればその値のときだけ
    /// （利用者が自分で決めた値は外さない）
    InClaude { value: Option<&'static str> },
    /// tako のペインの中から起こされたとき（`TAKO_PANE_ID` がある）だけ。tako の外の端末から起こした
    /// GUI では今までどおり（利用者の端末の印として残す）
    FromPane,
    /// 値のうち tako が足した分だけを抜く（利用者の分は残す）
    TakoPart(Part),
}

/// 値のうち tako が足した分の見分け方
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Part {
    /// mod の置き場（`<data dir>/claude-mod/tako`）を一覧から抜く。利用者自身の plugin dir は残す
    PluginDirs,
    /// シェル統合の fish の置き場（`<data dir>/shell-integration/fish-data`）を一覧から抜き、重なりを畳む。
    /// 残りが無い・tako が既定として足した `/usr/local/share:/usr/share` だけなら名前ごと外す
    /// （XDG Base Directory の既定と同じ = 意味は変わらない）
    XdgDataDirs,
    /// bash の統合の注入（`source '<…>/shell-integration/tako.bash'`）か、それが置き換えた `_tako_precmd`
    PromptCommand,
    /// zsh の統合の置き場（`<…>/shell-integration/zsh`）。`TAKO_ORIG_ZDOTDIR` があればその値へ戻す
    Zdotdir,
    /// tako の共有 jobserver の fifo を指す値（[`crate::jobserver::refers_to_tako`]）
    Jobserver,
    /// 検証の GUI がペインへ入れた履歴の置き場（[`crate::paths::VERIFY_HISTFILE_ENV`] と同じ値）
    Histfile,
}

/// 外す名前の組
#[derive(Debug, Clone, Copy)]
pub struct Strip {
    pub names: &'static [&'static str],
    pub when: When,
    pub why: Why,
}

/// 外さないと決めた名前の組（外しすぎの見張り）
#[derive(Debug, Clone, Copy)]
pub struct Kept {
    pub names: &'static [&'static str],
    pub why: Why,
}

/// GUI が起動の最初に外す名前（#2570 の正本）
pub const STRIP: &[Strip] = &[
    Strip {
        names: crate::backend::PANE_SCOPED_ENV,
        when: When::Always,
        why: Why {
            ja: "ペインの識別（ペイン・タブの番号・器の名前・統合の書き先）。GUI は自分のペインへ付け直す",
            en: "Pane identity (pane and tab IDs, container socket, OSC sink). The app sets them again for each of its own panes",
        },
    },
    Strip {
        names: &["TAKO_SOCKET", "TAKO_TOKEN", "TAKO_MCP_URL"],
        when: When::Always,
        why: Why {
            ja: "起こした側の tako の制御プレーンへの接続。GUI は自分の接続をペインへ付け直す",
            en: "Connection to the control plane of the tako that launched it. The app sets its own for each pane",
        },
    },
    Strip {
        names: crate::claude_mod::HELPER_DROP_ENV,
        when: When::Always,
        why: Why {
            ja: "orchestrator の役割・mod の起床条件・呼び出し元の claude のセッションの印（#2559 と同じ表）",
            en: "Orchestrator role, tako mod wake-up condition and the calling claude session's markers (same list as #2559)",
        },
    },
    Strip {
        names: &[crate::claude_mod::PLUGIN_DIRS_ENV],
        when: When::TakoPart(Part::PluginDirs),
        why: Why {
            ja: "tako の mod の置き場だけを抜く（利用者自身の plugin dir は残す）",
            en: "Only the tako mod directory is removed (the user's own plugin directories stay)",
        },
    },
    Strip {
        names: &[crate::jobserver::ENV],
        when: When::TakoPart(Part::Jobserver),
        why: Why {
            ja: "起こした側の tako の共有 jobserver。GUI は自分の札をペインへ配る",
            en: "Shared jobserver of the tako that launched it. The app hands out its own tokens to its panes",
        },
    },
    Strip {
        names: &["TAKO_ORIG_ZDOTDIR"],
        when: When::Always,
        why: Why {
            ja: "zsh の統合の控え（tako だけが使う名前）",
            en: "Saved ZDOTDIR of the zsh integration (a name only tako uses)",
        },
    },
    Strip {
        names: &["ZDOTDIR"],
        when: When::TakoPart(Part::Zdotdir),
        why: Why {
            ja: "zsh の統合の置き場を指すときだけ（控えがあれば元へ戻す）",
            en: "Only when it points at the zsh integration directory (restored from the saved value if any)",
        },
    },
    Strip {
        names: &["PROMPT_COMMAND"],
        when: When::TakoPart(Part::PromptCommand),
        why: Why {
            ja: "bash の統合の注入（zsh・fish のペインでは使われず残る）だけ",
            en: "Only the bash integration hook (left unused in zsh and fish panes)",
        },
    },
    Strip {
        names: &["XDG_DATA_DIRS"],
        when: When::TakoPart(Part::XdgDataDirs),
        why: Why {
            ja: "fish の統合の置き場を抜いて重なりを畳む（起こし直すたびに 1 段ずつ重なっていた）",
            en: "The fish integration directory is removed and duplicates collapsed (it stacked up once per relaunch)",
        },
    },
    Strip {
        names: &[crate::paths::VERIFY_HISTFILE_ENV],
        when: When::Always,
        why: Why {
            ja: "検証の GUI がペインへ入れた履歴の置き場の印（#1253。tako だけが使う名前）",
            en: "Marker of the shell history file a verification app gave its panes (#1253; a name only tako uses)",
        },
    },
    Strip {
        names: &["HISTFILE"],
        when: When::TakoPart(Part::Histfile),
        why: Why {
            ja: "検証の GUI が入れた履歴の置き場と同じ値のときだけ（利用者の HISTFILE は残す）",
            en: "Only when it equals the history file a verification app set (the user's own HISTFILE stays)",
        },
    },
    Strip {
        names: &["GIT_EDITOR"],
        when: When::InClaude {
            value: Some("true"),
        },
        why: Why {
            ja: "claude が Bash の子へ配る値（新しいペインの git commit がエディタを開かずに中止される）",
            en: "Set by claude for its Bash children (git commit in a new pane would abort without opening an editor)",
        },
    },
    Strip {
        names: &["COREPACK_ENABLE_AUTO_PIN"],
        when: When::InClaude { value: Some("0") },
        why: Why {
            ja: "claude が Bash の子へ配る値",
            en: "Set by claude for its Bash children",
        },
    },
    Strip {
        names: &["NoDefaultCurrentDirectoryInExePath", "CLAUDE_EFFORT"],
        when: When::InClaude { value: None },
        why: Why {
            ja: "claude が Bash の子へ配る値（呼び出し元のセッションの設定）",
            en: "Set by claude for its Bash children (settings of the calling session)",
        },
    },
    Strip {
        names: &[
            "TERM_PROGRAM",
            "TERM_PROGRAM_VERSION",
            "TMUX",
            "TMUX_PANE",
            "SHLVL",
            "PWD",
            "OLDPWD",
        ],
        when: When::FromPane,
        why: Why {
            ja: "起こしたペインの端末とシェルの状態（器の tmux の名乗り・シェルの深さ・作業 dir）",
            en: "Terminal and shell state of the launching pane (tmux identity, shell depth, working directories)",
        },
    },
];

/// 外さないと決めた名前（外しすぎの見張り。[`STRIP`] と重なればテストが落ちる）
pub const KEPT: &[Kept] = &[
    Kept {
        names: &[
            "TAKO_ISOLATED",
            "TAKO_DATA_DIR",
            "TAKO_DISCOVERY_DIR",
            "TAKO_TMUX_SOCKET",
            "TAKO_PERSIST",
            "TAKO_SESSIONS_FILE",
            "TAKO_PANE_LOG_DIR",
            "TAKO_REMOTE_STATE_DIR",
            "TAKO_SELF_TEST",
        ],
        why: Why {
            ja: "隔離・セルフテストの指定。起こす側が並べた値と受け継いだ値を見分けられないので残す（ペインの中から隔離 GUI を起こすときは置き場とソケットを明示する）",
            en: "Isolation and self-test settings. Kept because an inherited value cannot be told from one the launcher set (name the data dir and socket explicitly when launching an isolated app from a pane)",
        },
    },
    Kept {
        names: &["CLAUDE_CONFIG_DIR"],
        why: Why {
            ja: "利用者が launchctl やシェルで全体に渡すことがある（Dock 起動でも要る）。ペインの claude のアカウントは tako が起動の行で付ける",
            en: "Users may set it globally with launchctl or their shell (needed for a Dock launch too). tako sets a pane's claude account on its command line",
        },
    },
    Kept {
        names: &["TERM", "COLORTERM"],
        why: Why {
            ja: "ペインは毎回固定値で上書きする（#946）。#946 の診断は親の値を材料にする",
            en: "Panes always overwrite them with fixed values (#946), whose diagnosis reads the parent's value",
        },
    },
    Kept {
        names: &["PATH", "HOME", "SHELL", "LANG", "LC_ALL", "LC_CTYPE"],
        why: Why {
            ja: "利用者の環境（ペインのシェルは rc で作り直す。tako が入れる LC_CTYPE=UTF-8 とも同じ値）",
            en: "The user's environment (pane shells rebuild it from their rc files; LC_CTYPE=UTF-8 matches what tako injects)",
        },
    },
];

/// 当てる変更
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Plan {
    /// 外す名前
    pub remove: Vec<&'static str>,
    /// tako の分を抜いた値で置き換える名前
    pub set: Vec<(&'static str, OsString)>,
}

impl Plan {
    pub fn is_empty(&self) -> bool {
        self.remove.is_empty() && self.set.is_empty()
    }
}

/// 外すもの・置き換えるものを決める（純関数。`get` は env の読み口）
pub fn plan(get: &dyn Fn(&str) -> Option<OsString>, sep: char) -> Plan {
    let from_pane = get("TAKO_PANE_ID").is_some();
    let in_claude = get("CLAUDECODE").is_some();
    let mut out = Plan::default();
    for strip in STRIP {
        for &name in strip.names {
            if out.remove.contains(&name) || out.set.iter().any(|(n, _)| *n == name) {
                continue;
            }
            let Some(value) = get(name) else {
                continue;
            };
            match strip.when {
                When::Always => out.remove.push(name),
                When::FromPane => {
                    if from_pane {
                        out.remove.push(name);
                    }
                }
                When::InClaude { value: want } => {
                    if in_claude && want.is_none_or(|w| value == *w) {
                        out.remove.push(name);
                    }
                }
                When::TakoPart(part) => match tako_part(part, &value, get, sep) {
                    Some(Change::Remove) => out.remove.push(name),
                    Some(Change::Set(v)) => out.set.push((name, v)),
                    None => {}
                },
            }
        }
    }
    out
}

enum Change {
    Remove,
    Set(OsString),
}

fn tako_part(
    part: Part,
    value: &OsString,
    get: &dyn Fn(&str) -> Option<OsString>,
    sep: char,
) -> Option<Change> {
    // UTF-8 でない値は見分けずに触らない（利用者の値を壊さない）
    let text = value.to_str()?;
    match part {
        Part::PluginDirs => {
            let user = crate::claude_mod::without_tako_mod(text, sep)?;
            Some(match user {
                Some(v) => Change::Set(v.into()),
                None => Change::Remove,
            })
        }
        Part::XdgDataDirs => {
            let entries: Vec<&str> = text.split(sep).collect();
            if !entries.iter().any(|e| is_integration_dir(e, "fish-data")) {
                return None;
            }
            let mut kept: Vec<&str> = Vec::new();
            for entry in entries {
                if entry.is_empty()
                    || is_integration_dir(entry, "fish-data")
                    || kept.contains(&entry)
                {
                    continue;
                }
                kept.push(entry);
            }
            Some(if kept.is_empty() || kept == XDG_DEFAULT_TAIL {
                Change::Remove
            } else {
                Change::Set(kept.join(&sep.to_string()).into())
            })
        }
        Part::PromptCommand => {
            let ours = text.trim() == "_tako_precmd"
                || (text.starts_with("source '")
                    && text.trim_end().ends_with("tako.bash'")
                    && text.contains("shell-integration"));
            ours.then_some(Change::Remove)
        }
        Part::Zdotdir => {
            if !is_integration_dir(text, "zsh") {
                return None;
            }
            Some(
                match get("TAKO_ORIG_ZDOTDIR")
                    .filter(|o| o.to_str().is_none_or(|s| !is_integration_dir(s, "zsh")))
                {
                    Some(orig) if !orig.is_empty() => Change::Set(orig),
                    _ => Change::Remove,
                },
            )
        }
        Part::Jobserver => crate::jobserver::refers_to_tako(text).then_some(Change::Remove),
        Part::Histfile => get(crate::paths::VERIFY_HISTFILE_ENV)
            .is_some_and(|marker| &marker == value)
            .then_some(Change::Remove),
    }
}

/// シェル統合が fish の既定の検索先として足す並び（`shell_integration` の unix の注入と同じ）
const XDG_DEFAULT_TAIL: [&str; 2] = ["/usr/local/share", "/usr/share"];

/// `<…>/shell-integration/<leaf>` か（どのインスタンスの data dir でも）
fn is_integration_dir(path: &str, leaf: &str) -> bool {
    let trimmed = path.trim_end_matches(['/', '\\']);
    let mut parts = trimmed.rsplit(['/', '\\']);
    parts.next() == Some(leaf) && parts.next() == Some("shell-integration")
}

/// 起動の最初に当てた結果（persist.log の 1 行の材料。値は持たない）
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Applied {
    /// A/B（[`LEGACY_ENV`]）で何もしなかった
    pub legacy: bool,
    /// 起こしたペインの番号（`TAKO_PANE_ID` があったとき）
    pub from_pane: Option<String>,
    pub plan: Plan,
    /// exec の前の自分が外して起こし直した（[`REEXEC_ENV`] で受け取った）もの
    pub reexeced: Option<Carried>,
}

/// exec の前の自分が外したもの（[`REEXEC_ENV`] の中身。名前だけ）
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Carried {
    pub from_pane: Option<String>,
    pub names: Vec<String>,
}

impl Carried {
    fn encode(from_pane: Option<&str>, names: &[&str]) -> String {
        format!("{}\t{}", from_pane.unwrap_or_default(), names.join(","))
    }

    fn decode(value: &str) -> Self {
        let (pane, names) = value.split_once('\t').unwrap_or(("", value));
        Self {
            from_pane: (!pane.is_empty()).then(|| pane.to_string()),
            names: names
                .split(',')
                .filter(|n| !n.is_empty())
                .map(str::to_string)
                .collect(),
        }
    }
}

impl Applied {
    /// 外した・抜いた名前（並びは表の順）
    fn names(&self) -> Vec<&'static str> {
        self.plan
            .remove
            .iter()
            .copied()
            .chain(self.plan.set.iter().map(|(n, _)| *n))
            .collect()
    }

    /// persist.log へ残す 1 行（何もしていなければ None）。**名前だけ**で値は載せない（トークンを含む）
    pub fn log_line(&self) -> Option<String> {
        let origin = |pane: Option<&str>| match pane {
            Some(pane) => format!("ペイン {pane} の中から起こされた"),
            None => "起こした側の env に tako のペインか claude のセッションの印があった".into(),
        };
        if let Some(carried) = &self.reexeced {
            return Some(format!(
                "起動時の env（#2570）: {}ので、ペインの中でだけ意味を持つ env を外して自分を起こし直した（exec・{} 個）: {}",
                origin(carried.from_pane.as_deref()),
                carried.names.len(),
                carried.names.join(", ")
            ));
        }
        if self.plan.is_empty() {
            return None;
        }
        let origin = origin(self.from_pane.as_deref());
        let names = self.names().join(", ");
        Some(if self.legacy {
            format!(
                "起動時の env（#2570）: {origin}が、A/B（{LEGACY_ENV}）なのでペインの中でだけ意味を持つ env を外していない: {names}"
            )
        } else {
            format!(
                "起動時の env（#2570）: {origin}ので、ペインの中でだけ意味を持つ env を外した（外した {} 個・tako の分を抜いた {} 個。起こし直してはいない）: {names}",
                self.plan.remove.len(),
                self.plan.set.len(),
            )
        })
    }
}

/// 起動の最初に自分の env から外す（GUI の `main` だけが呼ぶ。**スレッドを起こす前**に呼ぶこと =
/// env の書き換えは同じプロセスの別スレッドの読みと競る）
pub fn apply_at_startup() -> Applied {
    // 起こし直した後の自分なら、前の自分が外したものを受け取って印を消す（子へ渡さない）
    let reexeced = std::env::var(REEXEC_ENV).ok().map(|v| Carried::decode(&v));
    if std::env::var_os(REEXEC_ENV).is_some() {
        std::env::remove_var(REEXEC_ENV);
    }
    let get = |name: &str| std::env::var_os(name);
    let plan = plan(&get, crate::claude_mod::LIST_SEP);
    let from_pane = std::env::var("TAKO_PANE_ID").ok();
    let legacy = std::env::var_os(LEGACY_ENV).is_some_and(|v| !v.is_empty());
    if !legacy {
        for name in &plan.remove {
            std::env::remove_var(name);
        }
        for (name, value) in &plan.set {
            std::env::set_var(name, value);
        }
    }
    Applied {
        legacy,
        from_pane,
        plan,
        reexeced,
    }
}

/// 外した env のまま自分を exec し直すか（純関数。A/B・外していない・既に起こし直した後は起こし直さない
/// = 2 度目は無い）
pub fn wants_reexec(applied: &Applied) -> bool {
    cfg!(unix) && !applied.legacy && !applied.plan.is_empty() && applied.reexeced.is_none()
}

/// 外した env のまま自分を exec し直す（unix の GUI の `main` だけが、[`apply_at_startup`] の直後・
/// スレッドを起こす前に呼ぶ）。同じ pid・同じ argv・同じ fd のまま、exec の時点の env の塊を外した後の
/// 形にする（理由はこの module の説明）。**戻るのは起こし直さないときと exec に失敗したときだけ**で、
/// 失敗したら `Some(理由)`（呼び手は外した env のまま続ける）
pub fn reexec_if_stripped(applied: &Applied) -> Option<std::io::Error> {
    if !wants_reexec(applied) {
        return None;
    }
    reexec(applied)
}

#[cfg(unix)]
fn reexec(applied: &Applied) -> Option<std::io::Error> {
    use std::os::unix::process::CommandExt;
    let exe = match std::env::current_exe() {
        Ok(exe) => exe,
        Err(e) => return Some(e),
    };
    let mut args = std::env::args_os();
    let mut command = std::process::Command::new(exe);
    if let Some(argv0) = args.next() {
        command.arg0(argv0);
    }
    command.args(args).env(
        REEXEC_ENV,
        Carried::encode(applied.from_pane.as_deref(), &applied.names()),
    );
    // 成功すれば戻らない
    Some(command.exec())
}

#[cfg(not(unix))]
fn reexec(_applied: &Applied) -> Option<std::io::Error> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn plan_of(pairs: &[(&str, &str)], sep: char) -> Plan {
        let env: HashMap<String, OsString> = pairs
            .iter()
            .map(|(k, v)| ((*k).to_string(), OsString::from(*v)))
            .collect();
        plan(&|k: &str| env.get(k).cloned(), sep)
    }

    fn set_of(plan: &Plan, name: &str) -> Option<String> {
        plan.set
            .iter()
            .find(|(n, _)| *n == name)
            .map(|(_, v)| v.to_string_lossy().into_owned())
    }

    const FISH: &str = "/u/Library/Application Support/tako/shell-integration/fish-data";

    /// 本番の GUI で見た形（master のペインの claude の Bash から起こし直した = 10/31 の `ps eww`）
    fn master_pane() -> Vec<(&'static str, String)> {
        let xdg = format!("{FISH}:{FISH}:{FISH}:{FISH}:{FISH}:{FISH}:/usr/local/share:/usr/share");
        vec![
            ("TAKO_PANE_ID", "3094".into()),
            ("TAKO_TAB_ID", "151".into()),
            (
                "TAKO_SOCKET",
                "/u/Library/Application Support/tako/tako.sock".into(),
            ),
            ("TAKO_TOKEN", "secret".into()),
            ("TAKO_MCP_URL", "http://127.0.0.1:1/mcp".into()),
            ("TAKO_BACKEND_SOCKET", "tako".into()),
            ("TAKO_ORCHESTRATOR_ROLE", "master:proj".into()),
            (
                "TAKO_CLI",
                "/Applications/tako.app/Contents/MacOS/tako".into(),
            ),
            (
                "CLAUDE_CODE_PLUGIN_DIRS",
                "/u/Library/Application Support/tako/claude-mod/tako:/u/my-plugins".into(),
            ),
            ("CLAUDECODE", "1".into()),
            ("CLAUDE_CODE_SESSION_ID", "session-1".into()),
            ("CLAUDE_CODE_CHILD_SESSION", "1".into()),
            ("CLAUDE_CODE_ENTRYPOINT", "cli".into()),
            (
                "CLAUDE_CODE_EXECPATH",
                "/u/.local/share/claude/versions/2.1.301".into(),
            ),
            (
                "CLAUDE_CODE_MESSAGING_SOCKET",
                "/tmp/cc-socks/1.sock".into(),
            ),
            ("CLAUDE_CODE_MESSAGING_TOKEN", "secret".into()),
            ("CLAUDE_CODE_BRIDGE_SESSION_ID", "session_x".into()),
            ("CLAUDE_CODE_SESSION_ATTENDED", "1".into()),
            ("CLAUDE_PID", "4242".into()),
            ("CLAUDE_EFFORT", "xhigh".into()),
            ("GIT_EDITOR", "true".into()),
            ("COREPACK_ENABLE_AUTO_PIN", "0".into()),
            ("NoDefaultCurrentDirectoryInExePath", "1".into()),
            (
                "CARGO_MAKEFLAGS",
                "-j18 --jobserver-auth=fifo:/var/folders/x/T/tako-0123456789abcdef.jobserver.fifo"
                    .into(),
            ),
            (
                "PROMPT_COMMAND",
                "source '/u/Library/Application Support/tako/shell-integration/tako.bash'".into(),
            ),
            ("XDG_DATA_DIRS", xdg),
            ("TERM_PROGRAM", "tmux".into()),
            ("TERM_PROGRAM_VERSION", "3.6b".into()),
            ("SHLVL", "2".into()),
            ("PWD", "/u".into()),
            ("OLDPWD", "/u".into()),
            // 残すもの
            ("TERM", "tmux-256color".into()),
            ("COLORTERM", "truecolor".into()),
            ("CLAUDE_CONFIG_DIR", "/u/.claude-work".into()),
            ("PATH", "/usr/bin:/bin".into()),
            ("HOME", "/u".into()),
            ("LC_CTYPE", "UTF-8".into()),
            ("DISABLE_AUTOUPDATER", "1".into()),
            ("TAKO_AUTO_RENAME", "0".into()),
        ]
    }

    fn pairs<'a>(v: &'a [(&'static str, String)]) -> Vec<(&'static str, &'a str)> {
        v.iter().map(|(k, v)| (*k, v.as_str())).collect()
    }

    #[test]
    fn issue2570_masterのペインから起こし直したguiはペインの印を外し利用者の分は残す() {
        let env = master_pane();
        let plan = plan_of(&pairs(&env), ':');
        for name in [
            "TAKO_PANE_ID",
            "TAKO_TAB_ID",
            "TAKO_SOCKET",
            "TAKO_TOKEN",
            "TAKO_MCP_URL",
            "TAKO_BACKEND_SOCKET",
            "TAKO_ORCHESTRATOR_ROLE",
            "TAKO_CLI",
            "CLAUDECODE",
            "CLAUDE_CODE_SESSION_ID",
            "CLAUDE_CODE_CHILD_SESSION",
            "CLAUDE_CODE_MESSAGING_TOKEN",
            "CLAUDE_PID",
            "CLAUDE_EFFORT",
            "GIT_EDITOR",
            "COREPACK_ENABLE_AUTO_PIN",
            "NoDefaultCurrentDirectoryInExePath",
            "CARGO_MAKEFLAGS",
            "PROMPT_COMMAND",
            "XDG_DATA_DIRS",
            "TERM_PROGRAM",
            "TERM_PROGRAM_VERSION",
            "SHLVL",
            "PWD",
            "OLDPWD",
        ] {
            assert!(
                plan.remove.contains(&name),
                "{name} を外していない: {plan:?}"
            );
        }
        // 6 段の fish-data と tako が足した既定だけ = 名前ごと外す（Dock 起動と同じく無い）
        assert_eq!(set_of(&plan, "XDG_DATA_DIRS"), None);
        // 利用者自身の plugin dir は残す
        assert_eq!(
            set_of(&plan, "CLAUDE_CODE_PLUGIN_DIRS").as_deref(),
            Some("/u/my-plugins")
        );
        for kept in [
            "TERM",
            "COLORTERM",
            "CLAUDE_CONFIG_DIR",
            "PATH",
            "HOME",
            "LC_CTYPE",
            "DISABLE_AUTOUPDATER",
            "TAKO_AUTO_RENAME",
        ] {
            assert!(!plan.remove.contains(&kept), "{kept} を外した（外しすぎ）");
            assert!(
                set_of(&plan, kept).is_none(),
                "{kept} を書き換えた（外しすぎ）"
            );
        }
    }

    #[test]
    fn 条件つきの名前は条件がそろわなければ残す() {
        // claude の外（CLAUDECODE が無い）なら GIT_EDITOR=true も利用者の値として残す
        let plan = plan_of(&[("GIT_EDITOR", "true"), ("CLAUDE_EFFORT", "high")], ':');
        assert!(plan.is_empty(), "{plan:?}");
        // claude の中でも利用者の値（true 以外）は残す
        let plan = plan_of(
            &[
                ("CLAUDECODE", "1"),
                ("GIT_EDITOR", "vim"),
                ("COREPACK_ENABLE_AUTO_PIN", "1"),
            ],
            ':',
        );
        assert_eq!(plan.remove, vec!["CLAUDECODE"]);
        // tako のペインの外（TAKO_PANE_ID が無い）から起こした GUI は端末の印を残す
        let plan = plan_of(
            &[
                ("TERM_PROGRAM", "iTerm.app"),
                ("SHLVL", "2"),
                ("TMUX", "/tmp/t,1,0"),
            ],
            ':',
        );
        assert!(plan.is_empty(), "{plan:?}");
    }

    #[test]
    fn takoの分だけを抜いて利用者の値は変えない() {
        // fish-data を抜き、利用者の重なりも畳む（先に出た順を保つ = 探す順は変わらない）
        let xdg = format!(
            "{FISH}:/opt/homebrew/share:{FISH}:/opt/homebrew/share:/usr/local/share:/usr/share"
        );
        let plan = plan_of(&[("XDG_DATA_DIRS", xdg.as_str())], ':');
        assert_eq!(
            set_of(&plan, "XDG_DATA_DIRS").as_deref(),
            Some("/opt/homebrew/share:/usr/local/share:/usr/share")
        );
        // tako の分が無い値は重なっていても触らない
        let plan = plan_of(&[("XDG_DATA_DIRS", "/a:/a:/usr/share")], ':');
        assert!(plan.is_empty(), "{plan:?}");
        // 利用者の PROMPT_COMMAND・jobserver・ZDOTDIR は残す
        let plan = plan_of(
            &[
                ("PROMPT_COMMAND", "history -a"),
                (
                    "CARGO_MAKEFLAGS",
                    "-j8 --jobserver-auth=fifo:/tmp/make-fifo",
                ),
                ("ZDOTDIR", "/u/.config/zsh"),
            ],
            ':',
        );
        assert!(plan.is_empty(), "{plan:?}");
        // bash の統合が置き換えた後の形も tako のもの
        let plan = plan_of(&[("PROMPT_COMMAND", "_tako_precmd")], ':');
        assert_eq!(plan.remove, vec!["PROMPT_COMMAND"]);
        // zsh の統合の置き場は控えへ戻す（控えも外す）・控えが無ければ外す
        let zsh = "/u/Library/Application Support/tako/shell-integration/zsh";
        let plan = plan_of(
            &[("ZDOTDIR", zsh), ("TAKO_ORIG_ZDOTDIR", "/u/.config/zsh")],
            ':',
        );
        assert_eq!(set_of(&plan, "ZDOTDIR").as_deref(), Some("/u/.config/zsh"));
        assert!(plan.remove.contains(&"TAKO_ORIG_ZDOTDIR"));
        let plan = plan_of(&[("ZDOTDIR", zsh)], ':');
        assert_eq!(plan.remove, vec!["ZDOTDIR"]);
        // mod の置き場だけの一覧は名前ごと外す・利用者だけの一覧は触らない
        let plan = plan_of(&[("CLAUDE_CODE_PLUGIN_DIRS", "/d/claude-mod/tako")], ':');
        assert_eq!(plan.remove, vec!["CLAUDE_CODE_PLUGIN_DIRS"]);
        let plan = plan_of(&[("CLAUDE_CODE_PLUGIN_DIRS", "/u/my-plugins")], ':');
        assert!(plan.is_empty(), "{plan:?}");
        // 検証の GUI が入れた履歴の置き場は印ごと外す・利用者の HISTFILE は残す
        let plan = plan_of(
            &[
                ("HISTFILE", "/t/shell_history"),
                ("TAKO_VERIFY_HISTFILE", "/t/shell_history"),
            ],
            ':',
        );
        assert_eq!(plan.remove.len(), 2, "{plan:?}");
        let plan = plan_of(&[("HISTFILE", "/u/.zsh_history")], ':');
        assert!(plan.is_empty(), "{plan:?}");
    }

    #[test]
    fn windowsの区切りとパスでも見分ける() {
        let fish = r"C:\Users\winuser\AppData\Roaming\tako\shell-integration\fish-data";
        let xdg = format!(r"{fish};C:\share;{fish}");
        let plan = plan_of(
            &[
                ("XDG_DATA_DIRS", xdg.as_str()),
                (
                    "CLAUDE_CODE_PLUGIN_DIRS",
                    r"C:\Users\winuser\AppData\Roaming\tako\claude-mod\tako;D:\plugins",
                ),
            ],
            ';',
        );
        assert_eq!(set_of(&plan, "XDG_DATA_DIRS").as_deref(), Some(r"C:\share"));
        assert_eq!(
            set_of(&plan, "CLAUDE_CODE_PLUGIN_DIRS").as_deref(),
            Some(r"D:\plugins")
        );
    }

    #[test]
    fn 外す表と残す表は重ならずペインへ配る名前をすべて外す() {
        let strip: Vec<&str> = STRIP.iter().flat_map(|s| s.names.iter().copied()).collect();
        for kept in KEPT.iter().flat_map(|k| k.names.iter()) {
            assert!(!strip.contains(kept), "{kept} が外す表と残す表の両方にある");
        }
        // GUI がペインへ配る名前（器へ固定する表と同じ材料）は、ペインの中から起こした GUI から外す
        let injected = crate::backend::PANE_SCOPED_ENV
            .iter()
            .chain(crate::shell_integration::INJECTED_KEYS)
            .chain(crate::claude_mod::INJECTED_KEYS)
            .chain([&crate::jobserver::ENV, &crate::paths::VERIFY_HISTFILE_ENV]);
        for name in injected {
            assert!(strip.contains(name), "ペインへ配る {name} を外さない");
        }
        // #2559 の補助の子の表も含む
        for name in crate::claude_mod::HELPER_DROP_ENV {
            assert!(strip.contains(name), "#2559 の {name} を外さない");
        }
    }

    #[test]
    fn 理由は日英でそろい英語に日本語が混ざらない() {
        let whys = STRIP
            .iter()
            .map(|s| s.why)
            .chain(KEPT.iter().map(|k| k.why));
        for why in whys {
            assert!(
                !why.ja.is_empty() && !why.ja.is_ascii(),
                "日本語の理由: {why:?}"
            );
            assert!(
                !why.en.is_empty() && why.en.is_ascii(),
                "英語の理由: {why:?}"
            );
        }
    }

    #[test]
    fn ログの行は名前だけで値を載せない() {
        let env = master_pane();
        let applied = Applied {
            legacy: false,
            from_pane: Some("3094".into()),
            plan: plan_of(&pairs(&env), ':'),
            reexeced: None,
        };
        let line = applied.log_line().expect("外したなら 1 行");
        assert!(
            line.contains("ペイン 3094") && line.contains("TAKO_TOKEN"),
            "{line}"
        );
        for (_, value) in &env {
            if value.len() > 4 {
                assert!(!line.contains(value.as_str()), "値が載った: {value}");
            }
        }
        assert_eq!(Applied::default().log_line(), None);
        // ペインの外（claude の Bash だけ）から起こされたときも文が崩れない
        let applied = Applied {
            legacy: false,
            from_pane: None,
            plan: plan_of(&[("CLAUDECODE", "1"), ("GIT_EDITOR", "true")], ':'),
            reexeced: None,
        };
        let line = applied.log_line().expect("外したなら 1 行");
        assert!(
            line.contains("印があったので") && line.contains("GIT_EDITOR"),
            "{line}"
        );
    }

    #[test]
    fn 起こし直しは外したときだけ1度で印は名前だけを運ぶ() {
        let env = master_pane();
        let mut applied = Applied {
            legacy: false,
            from_pane: Some("3094".into()),
            plan: plan_of(&pairs(&env), ':'),
            reexeced: None,
        };
        assert_eq!(wants_reexec(&applied), cfg!(unix));
        // 運ぶのは名前だけ（値 = トークン・パスは載せない）。exec の後の自分が同じ名前を読み戻す
        let encoded = Carried::encode(applied.from_pane.as_deref(), &applied.names());
        for (_, value) in &env {
            if value.len() > 4 {
                assert!(!encoded.contains(value.as_str()), "値が載った: {value}");
            }
        }
        let carried = Carried::decode(&encoded);
        assert_eq!(carried.from_pane.as_deref(), Some("3094"));
        assert_eq!(carried.names, applied.names());
        // 起こし直した後は 2 度目をしない・persist.log は前の自分が外したものを書く
        applied.reexeced = Some(carried);
        assert!(!wants_reexec(&applied));
        assert!(reexec_if_stripped(&applied).is_none());
        let line = applied.log_line().expect("起こし直したなら 1 行");
        assert!(
            line.contains("ペイン 3094")
                && line.contains("起こし直した")
                && line.contains("GIT_EDITOR"),
            "{line}"
        );
        // A/B・何も外していないときは起こし直さない
        applied.reexeced = None;
        applied.legacy = true;
        assert!(!wants_reexec(&applied));
        assert!(!wants_reexec(&Applied::default()));
        assert!(reexec_if_stripped(&Applied::default()).is_none());
        assert_eq!(Carried::decode("\t"), Carried::default());
    }

    #[test]
    fn 起動時に当てるとenvから消えab_では残す() {
        if crate::run_alone!(起動時に当てるとenvから消えab_では残す) {
            return;
        }
        let set = || {
            std::env::set_var("TAKO_PANE_ID", "7");
            std::env::set_var("CLAUDECODE", "1");
            std::env::set_var("GIT_EDITOR", "true");
            std::env::set_var(
                "XDG_DATA_DIRS",
                format!("{FISH}:/usr/local/share:/usr/share"),
            );
            std::env::set_var("TAKO_ISOLATED", "1");
        };
        set();
        std::env::remove_var(LEGACY_ENV);
        let applied = apply_at_startup();
        assert!(!applied.legacy);
        assert_eq!(applied.from_pane.as_deref(), Some("7"));
        for name in ["TAKO_PANE_ID", "CLAUDECODE", "GIT_EDITOR", "XDG_DATA_DIRS"] {
            assert!(std::env::var_os(name).is_none(), "{name} が残った");
        }
        assert_eq!(std::env::var("TAKO_ISOLATED").as_deref(), Ok("1"));
        // 起こし直した後の自分: 印を読み戻して消す（子へ渡さない）。外すものはもう無い
        std::env::set_var(REEXEC_ENV, "7\tTAKO_PANE_ID,CLAUDECODE");
        let applied = apply_at_startup();
        assert!(std::env::var_os(REEXEC_ENV).is_none(), "印が残った");
        assert!(applied.plan.is_empty(), "{:?}", applied.plan);
        assert!(!wants_reexec(&applied));
        assert_eq!(
            applied.reexeced.map(|c| c.names),
            Some(vec!["TAKO_PANE_ID".to_string(), "CLAUDECODE".to_string()])
        );
        // A/B: 何も外さない（#2570 前の形）
        set();
        std::env::set_var(LEGACY_ENV, "1");
        let applied = apply_at_startup();
        assert!(applied.legacy);
        assert_eq!(std::env::var("TAKO_PANE_ID").as_deref(), Ok("7"));
        assert_eq!(std::env::var("GIT_EDITOR").as_deref(), Ok("true"));
        assert!(applied.log_line().is_some_and(|l| l.contains(LEGACY_ENV)));
    }
}
