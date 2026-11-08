// Windows の CI で `tako setup` を通しで回すときの偽の claude（Issue #2767）。
//
// `scripts/windows/test-setup-windows-2767.ps1` が `rustc` でこの 1 ファイルを `claude.exe` に
// して、隔離した USERPROFILE の `.local\bin`（公式のネイティブインストーラの置き場）へ置く。
// ワークスペースのクレートではない（依存を持たない 1 ファイル。`cargo build` は組まない）。
//
// setup が使う**非対話の問い合わせ**（版・認証・MCP 登録の一覧 / 追加 / 削除）にだけ本物と同じ形で
// 答え、それ以外の呼び出し（引数なしの `claude`・プロンプトつきの起動など = 対話起動）は argv ごと
// `<home>\fake-claude-unexpected.log` へ残して 0 で終わる。検査の側はこのファイルが無いことを見る
// （macOS の `scripts/verify-setup-multiagent.sh` の `make_claude` と同じ取り決め）。
// 呼ばれた全部は `<home>\fake-claude-calls.log` へ 1 行ずつ残す（失敗したときの手がかり）。
//
// MCP の登録は本物と同じく `<home>\.claude.json` の最上位の `mcpServers` に書く（tako は登録の
// 健全性をこのファイルから直接読む）。ほかのキーは読んだまま書き戻す。
// 認証の状態は `<home>\fake-claude-state` の 1 行で切り替える（無ければ `pro`。`unauth` で未ログイン）。

use std::io::Write as _;
use std::path::PathBuf;

/// setup が下限（tako mod の版の判定）と比べる版。本物の `claude --version` と同じ形
const VERSION_LINE: &str = "2.1.301 (Claude Code)";

fn home() -> PathBuf {
    std::env::var_os("USERPROFILE")
        .filter(|v| !v.is_empty())
        .or_else(|| std::env::var_os("HOME"))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}

fn append(name: &str, line: &str) {
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(home().join(name))
    {
        let _ = writeln!(file, "{line}");
    }
}

fn state() -> String {
    std::fs::read_to_string(home().join("fake-claude-state"))
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|_| "pro".to_string())
}

// --- 依存を持たない最小の JSON（読んで、1 キーを差し替えて、書き戻すだけ）---

#[derive(Clone, Debug)]
enum Json {
    Null,
    Bool(bool),
    Num(String),
    Str(String),
    Arr(Vec<Json>),
    Obj(Vec<(String, Json)>),
}

impl Json {
    fn get(&self, key: &str) -> Option<&Json> {
        match self {
            Json::Obj(items) => items.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }

    fn as_str(&self) -> Option<&str> {
        match self {
            Json::Str(s) => Some(s),
            _ => None,
        }
    }

    fn entry(&mut self, key: &str) -> &mut Json {
        if !matches!(self, Json::Obj(_)) {
            *self = Json::Obj(Vec::new());
        }
        let Json::Obj(items) = self else {
            unreachable!()
        };
        let at = match items.iter().position(|(k, _)| k == key) {
            Some(at) => at,
            None => {
                items.push((key.to_string(), Json::Obj(Vec::new())));
                items.len() - 1
            }
        };
        &mut items[at].1
    }

    fn remove(&mut self, key: &str) -> bool {
        match self {
            Json::Obj(items) => {
                let before = items.len();
                items.retain(|(k, _)| k != key);
                items.len() != before
            }
            _ => false,
        }
    }

    fn write(&self, out: &mut String) {
        match self {
            Json::Null => out.push_str("null"),
            Json::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
            Json::Num(n) => out.push_str(n),
            Json::Str(s) => write_str(s, out),
            Json::Arr(items) => {
                out.push('[');
                for (i, item) in items.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    item.write(out);
                }
                out.push(']');
            }
            Json::Obj(items) => {
                out.push('{');
                for (i, (k, v)) in items.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    write_str(k, out);
                    out.push(':');
                    v.write(out);
                }
                out.push('}');
            }
        }
    }
}

fn write_str(s: &str, out: &mut String) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
}

struct Parser<'a> {
    s: &'a [u8],
    i: usize,
}

impl Parser<'_> {
    fn ws(&mut self) {
        while self.i < self.s.len() && self.s[self.i].is_ascii_whitespace() {
            self.i += 1;
        }
    }

    fn eat(&mut self, c: u8) -> Option<()> {
        self.ws();
        (self.s.get(self.i) == Some(&c)).then(|| self.i += 1)
    }

    fn value(&mut self) -> Option<Json> {
        self.ws();
        match *self.s.get(self.i)? {
            b'{' => {
                self.i += 1;
                let mut items = Vec::new();
                if self.eat(b'}').is_some() {
                    return Some(Json::Obj(items));
                }
                loop {
                    self.ws();
                    let key = self.string()?;
                    self.eat(b':')?;
                    items.push((key, self.value()?));
                    if self.eat(b',').is_none() {
                        self.eat(b'}')?;
                        return Some(Json::Obj(items));
                    }
                }
            }
            b'[' => {
                self.i += 1;
                let mut items = Vec::new();
                if self.eat(b']').is_some() {
                    return Some(Json::Arr(items));
                }
                loop {
                    items.push(self.value()?);
                    if self.eat(b',').is_none() {
                        self.eat(b']')?;
                        return Some(Json::Arr(items));
                    }
                }
            }
            b'"' => self.string().map(Json::Str),
            b't' => self.word("true", Json::Bool(true)),
            b'f' => self.word("false", Json::Bool(false)),
            b'n' => self.word("null", Json::Null),
            _ => {
                let start = self.i;
                while self.i < self.s.len() && b"+-.0123456789eE".contains(&self.s[self.i]) {
                    self.i += 1;
                }
                (self.i > start)
                    .then(|| Json::Num(String::from_utf8_lossy(&self.s[start..self.i]).into()))
            }
        }
    }

    fn word(&mut self, word: &str, value: Json) -> Option<Json> {
        self.s[self.i..].starts_with(word.as_bytes()).then(|| {
            self.i += word.len();
            value
        })
    }

    fn string(&mut self) -> Option<String> {
        if self.s.get(self.i) != Some(&b'"') {
            return None;
        }
        self.i += 1;
        let mut out = Vec::new();
        loop {
            let c = *self.s.get(self.i)?;
            self.i += 1;
            match c {
                b'"' => return String::from_utf8(out).ok(),
                b'\\' => {
                    let e = *self.s.get(self.i)?;
                    self.i += 1;
                    match e {
                        b'n' => out.push(b'\n'),
                        b'r' => out.push(b'\r'),
                        b't' => out.push(b'\t'),
                        b'b' => out.push(8),
                        b'f' => out.push(12),
                        b'u' => {
                            let hex = std::str::from_utf8(self.s.get(self.i..self.i + 4)?).ok()?;
                            self.i += 4;
                            let ch = char::from_u32(u32::from_str_radix(hex, 16).ok()?)?;
                            out.extend_from_slice(ch.to_string().as_bytes());
                        }
                        other => out.push(other),
                    }
                }
                other => out.push(other),
            }
        }
    }
}

fn claude_json_path() -> PathBuf {
    home().join(".claude.json")
}

/// 読めない・壊れているファイルは空のオブジェクトとして扱う（本物も上書きして直す）
fn load_claude_json() -> Json {
    std::fs::read(claude_json_path())
        .ok()
        .and_then(|bytes| {
            let mut p = Parser { s: &bytes, i: 0 };
            p.value()
        })
        .unwrap_or(Json::Obj(Vec::new()))
}

fn save_claude_json(value: &Json) {
    let mut out = String::new();
    value.write(&mut out);
    out.push('\n');
    if let Err(e) = std::fs::write(claude_json_path(), out) {
        eprintln!("fake claude: ~/.claude.json を書けない: {e}");
        std::process::exit(1);
    }
}

/// `mcp add --scope user --transport stdio <name> -- <command> <args...>`
fn mcp_add(rest: &[&str]) -> i32 {
    let Some(split) = rest.iter().position(|a| *a == "--") else {
        eprintln!("fake claude: mcp add は `--` の後ろにコマンドが要る");
        return 1;
    };
    let (opts, cmd) = (&rest[..split], &rest[split + 1..]);
    let Some(name) = opts
        .iter()
        .enumerate()
        .filter(|(i, a)| !a.starts_with("--") && (*i == 0 || !opts[i - 1].starts_with("--")))
        .map(|(_, a)| *a)
        .last()
    else {
        eprintln!("fake claude: mcp add にサーバー名が無い");
        return 1;
    };
    let Some((command, args)) = cmd.split_first() else {
        eprintln!("fake claude: mcp add にコマンドが無い");
        return 1;
    };
    let mut root = load_claude_json();
    let servers = root.entry("mcpServers");
    if servers.get(name).is_some() {
        eprintln!("MCP server {name} already exists in user config");
        return 1;
    }
    let entry = servers.entry(name);
    *entry = Json::Obj(vec![
        ("type".into(), Json::Str("stdio".into())),
        ("command".into(), Json::Str((*command).into())),
        (
            "args".into(),
            Json::Arr(args.iter().map(|a| Json::Str((*a).into())).collect()),
        ),
        ("env".into(), Json::Obj(Vec::new())),
    ]);
    save_claude_json(&root);
    println!(
        "Added stdio MCP server {name} with command: {} to user config",
        cmd.join(" ")
    );
    0
}

/// `mcp remove --scope user <name>`
fn mcp_remove(rest: &[&str]) -> i32 {
    let Some(name) = rest.last().filter(|a| !a.starts_with("--")) else {
        eprintln!("fake claude: mcp remove にサーバー名が無い");
        return 1;
    };
    let mut root = load_claude_json();
    if !root.entry("mcpServers").remove(name) {
        eprintln!("No MCP server found with name: {name}");
        return 1;
    }
    save_claude_json(&root);
    println!("Removed MCP server {name} from user config");
    0
}

fn mcp_list() {
    let root = load_claude_json();
    let servers = match root.get("mcpServers") {
        Some(Json::Obj(items)) if !items.is_empty() => items.clone(),
        _ => {
            println!("No MCP servers configured. Use `claude mcp add` to add a server.");
            return;
        }
    };
    println!("Checking MCP server health...");
    println!();
    for (name, server) in servers {
        let command = server.get("command").and_then(Json::as_str).unwrap_or("?");
        let args = match server.get("args") {
            Some(Json::Arr(items)) => items
                .iter()
                .filter_map(Json::as_str)
                .collect::<Vec<_>>()
                .join(" "),
            _ => String::new(),
        };
        println!("{name}: {command} {args} - \u{2713} Connected");
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    append(
        "fake-claude-calls.log",
        &format!("claude argv=[{}]", args.join(" ")),
    );
    let argv: Vec<&str> = args.iter().map(String::as_str).collect();
    let code = match argv.as_slice() {
        ["--version"] => {
            println!("{VERSION_LINE}");
            0
        }
        ["auth", "status", rest @ ..] if rest.iter().all(|a| *a == "--json") => {
            match state().as_str() {
                "unauth" => {
                    println!("{{\"loggedIn\":false}}");
                    1
                }
                plan => {
                    println!(
                        "{{\"loggedIn\":true,\"authMethod\":\"claude.ai\",\"subscriptionType\":\"{plan}\"}}"
                    );
                    0
                }
            }
        }
        ["mcp", "list"] => {
            mcp_list();
            0
        }
        ["mcp", "add", rest @ ..] => mcp_add(rest),
        ["mcp", "remove", rest @ ..] => mcp_remove(rest),
        _ => {
            append(
                "fake-claude-unexpected.log",
                &format!("claude argv=[{}]", args.join(" ")),
            );
            0
        }
    };
    std::process::exit(code);
}
