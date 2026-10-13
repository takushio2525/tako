//! 参照検索・呼び出し階層・型階層（S8 / #1685）の純粋部分
//!
//! 右クリックメニュー・右パネルの `references` ビューと CLI `tako lsp references` 等 /
//! MCP `tako_lsp` が同じ 1 本を通る。ここに置くのは I/O を持たない判断と状態だけ:
//!
//! - [`XrefKind`] — 5 種（参照 / 呼び出し元 / 呼び出し先 / スーパータイプ / サブタイプ）と
//!   LSP の method・能力のキー
//! - [`parse_references`] — `textDocument/references` の応答を並べて上限で切る
//! - [`parse_items`] / [`parse_expand`] — 階層の要素（prepare・展開の応答）
//! - [`Tree`] — 階層ツリーの展開・折りたたみ・子の読み込み。**展開状態の正本はここ**で、
//!   GUI のクリックも CLI / MCP の `expand` / `collapse` も同じ口を通る（#1407 と同じ穴 =
//!   「画面でしか開閉できず AI から読めない」を作らない）
//! - [`XrefStore`] — 問い合わせたペインごとの結果（ペインを閉じたら消す。保持件数の口）
//!
//! 問い合わせ（プロセスと I/O）は `tako_control::lsp::manager`、行を開く着地は
//! `tako_control::dispatch` が定義ジャンプと同じ `goto::plan_landing` で行う。
//! **言語名はここに書かない**（検出表 `servers.rs` だけが持つ）。

use std::collections::HashSet;
use std::path::PathBuf;

use serde_json::Value;

use super::goto::{self, RawLocation};
use crate::PaneId;

/// 1 つの結果に持つ参照の上限（サーバが巨大な配列を返しても保持と描画を有限にする。
/// 切った件数は応答の `truncated` に載る）
pub const MAX_REFERENCES: usize = 2000;
/// 1 つのツリーに持つノードの上限（展開を重ねても有限にする）
pub const MAX_NODES: usize = 2000;
/// 1 回の展開で足す子の上限
pub const MAX_CHILDREN: usize = 500;
/// prepare の答えのうち根にする数の上限（多重定義で複数返るサーバがある）
pub const MAX_ROOTS: usize = 8;
/// 展開できる深さの上限（再帰する呼び出しを開き続けても止まる）
pub const MAX_DEPTH: usize = 32;

/// 5 種（CLI のサブコマンド名 = MCP `tako_lsp` の `action`）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum XrefKind {
    References,
    IncomingCalls,
    OutgoingCalls,
    Supertypes,
    Subtypes,
}

impl XrefKind {
    pub const ALL: [XrefKind; 5] = [
        XrefKind::References,
        XrefKind::IncomingCalls,
        XrefKind::OutgoingCalls,
        XrefKind::Supertypes,
        XrefKind::Subtypes,
    ];

    /// 綴り（CLI / MCP が共有する）。順は [`Self::ALL`] と同じ
    pub const NAMES: [&'static str; 5] = [
        "references",
        "incoming-calls",
        "outgoing-calls",
        "supertypes",
        "subtypes",
    ];

    pub fn slug(self) -> &'static str {
        let index = Self::ALL
            .iter()
            .position(|k| *k == self)
            .unwrap_or_default();
        Self::NAMES[index]
    }

    pub fn parse(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|kind| kind.slug() == name)
    }

    /// 答えがツリー（階層）か。参照は一覧
    pub fn is_tree(self) -> bool {
        self != Self::References
    }

    /// `ServerCapabilities` のキー（無い / `false` なら未対応）
    pub fn provider_key(self) -> &'static str {
        match self {
            Self::References => "referencesProvider",
            Self::IncomingCalls | Self::OutgoingCalls => "callHierarchyProvider",
            Self::Supertypes | Self::Subtypes => "typeHierarchyProvider",
        }
    }

    /// 階層の根を得る method（参照は無い）
    pub fn prepare_method(self) -> Option<&'static str> {
        match self {
            Self::References => None,
            Self::IncomingCalls | Self::OutgoingCalls => Some("textDocument/prepareCallHierarchy"),
            Self::Supertypes | Self::Subtypes => Some("textDocument/prepareTypeHierarchy"),
        }
    }

    /// 本体の method（参照の検索 / 階層の 1 段の展開）
    pub fn method(self) -> &'static str {
        match self {
            Self::References => "textDocument/references",
            Self::IncomingCalls => "callHierarchy/incomingCalls",
            Self::OutgoingCalls => "callHierarchy/outgoingCalls",
            Self::Supertypes => "typeHierarchy/supertypes",
            Self::Subtypes => "typeHierarchy/subtypes",
        }
    }
}

/// サーバがその種類に応じるか（`true` / 登録オプションのオブジェクト = 応じる）
pub fn server_supports(capabilities: &Value, kind: XrefKind) -> bool {
    match capabilities.get(kind.provider_key()) {
        Some(Value::Bool(on)) => *on,
        Some(Value::Object(_)) => true,
        _ => false,
    }
}

/// 参照の応答（`Location[]` / `null`）を読む。返すのは（場所, 重複を畳んだ全件数）。
///
/// 場所は（URI, 行, 桁）の順に並べる（同じファイルの参照がまとまり、サーバの返す順に
/// 依らず同じ答えが同じ一覧になる）。[`MAX_REFERENCES`] を超えたぶんは捨てる（全件数は残す）
pub fn parse_references(result: &Value) -> (Vec<RawLocation>, usize) {
    let Value::Array(items) = result else {
        return (Vec::new(), 0);
    };
    let mut seen: HashSet<(String, usize, usize)> = HashSet::new();
    let mut out: Vec<RawLocation> = items
        .iter()
        .filter_map(goto::raw_location)
        .filter(|l| seen.insert((l.uri.clone(), l.line, l.character)))
        .collect();
    out.sort_by(|a, b| (&a.uri, a.line, a.character).cmp(&(&b.uri, b.line, b.character)));
    let total = out.len();
    out.truncate(MAX_REFERENCES);
    (out, total)
}

/// 階層の要素 1 つ（`CallHierarchyItem` / `TypeHierarchyItem`。座標は LSP のまま）
#[derive(Debug, Clone, PartialEq)]
pub struct HierarchyItem {
    pub name: String,
    /// LSP の `SymbolKind`（1〜26。名前は [`symbol_kind_name`]）
    pub kind: u64,
    pub detail: Option<String>,
    pub uri: String,
    /// 着地する位置（`selectionRange` の始まり。呼び出し元の展開では呼び出しの位置
    /// = `fromRanges` の先頭）
    pub line: usize,
    pub character: usize,
    /// サーバへ送り返す要素そのもの（展開の要求の `item`。`data` を含むので加工しない）
    pub raw: Value,
}

fn item_of(value: &Value) -> Option<HierarchyItem> {
    let range = value.get("selectionRange").or_else(|| value.get("range"))?;
    let (line, character) = goto::lsp_start(range.get("start")?)?;
    Some(HierarchyItem {
        name: value.get("name")?.as_str()?.to_string(),
        kind: value.get("kind").and_then(Value::as_u64).unwrap_or(0),
        detail: value
            .get("detail")
            .and_then(Value::as_str)
            .filter(|d| !d.is_empty())
            .map(str::to_string),
        uri: value.get("uri")?.as_str()?.to_string(),
        line,
        character,
        raw: value.clone(),
    })
}

/// 要素の配列（prepare の答え・型階層の展開の答え）を読む。形の崩れた要素は捨て、
/// [`MAX_CHILDREN`] で切る
pub fn parse_items(result: &Value) -> Vec<HierarchyItem> {
    let Value::Array(items) = result else {
        return Vec::new();
    };
    items
        .iter()
        .filter_map(item_of)
        .take(MAX_CHILDREN)
        .collect()
}

/// 展開の答えを読む。呼び出し元（`{ from, fromRanges }`）は呼び出しの位置へ着地し
/// （`fromRanges` は呼び出し元の文書の座標）、呼び出し先（`{ to, fromRanges }`）は
/// 呼ばれる側の定義へ着地する（`fromRanges` は親の文書の座標なので使わない）
pub fn parse_expand(result: &Value, kind: XrefKind) -> Vec<HierarchyItem> {
    let key = match kind {
        XrefKind::IncomingCalls => "from",
        XrefKind::OutgoingCalls => "to",
        XrefKind::Supertypes | XrefKind::Subtypes => return parse_items(result),
        XrefKind::References => return Vec::new(),
    };
    let Value::Array(calls) = result else {
        return Vec::new();
    };
    calls
        .iter()
        .filter_map(|call| {
            let mut item = item_of(call.get(key)?)?;
            if kind == XrefKind::IncomingCalls {
                if let Some(site) = call
                    .get("fromRanges")
                    .and_then(|r| r.get(0))
                    .and_then(|r| r.get("start"))
                    .and_then(goto::lsp_start)
                {
                    (item.line, item.character) = site;
                }
            }
            Some(item)
        })
        .take(MAX_CHILDREN)
        .collect()
}

/// LSP の `SymbolKind` の名前（応答の `kind`。知らない値は `symbol`）
pub fn symbol_kind_name(kind: u64) -> &'static str {
    const NAMES: [&str; 26] = [
        "file",
        "module",
        "namespace",
        "package",
        "class",
        "method",
        "property",
        "field",
        "constructor",
        "enum",
        "interface",
        "function",
        "variable",
        "constant",
        "string",
        "number",
        "boolean",
        "array",
        "object",
        "key",
        "null",
        "enum-member",
        "struct",
        "event",
        "operator",
        "type-parameter",
    ];
    usize::try_from(kind)
        .ok()
        .and_then(|k| k.checked_sub(1))
        .and_then(|k| NAMES.get(k))
        .copied()
        .unwrap_or("symbol")
}

/// 場所（tako の座標へ直したもの。定義ジャンプの飛び先と同じ形）
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Place {
    pub uri: String,
    pub path: PathBuf,
    /// 0 起点の行
    pub line: usize,
    /// 行内の UTF-8 バイト（`tako edit replace-range` と同じ桁）
    pub column: usize,
    /// 行内の文字数での桁（0 起点。開くときは +1 して `OpenFile` の `column` へ）
    pub char_column: usize,
    /// その行の抜粋（前後の空白を落としたもの）
    pub excerpt: String,
}

/// ツリーへ足す要素（control が座標を直してから渡す）
#[derive(Debug, Clone, PartialEq)]
pub struct NodeSeed {
    pub name: String,
    pub kind: u64,
    pub detail: Option<String>,
    pub place: Place,
    /// サーバへ送り返す要素（[`HierarchyItem::raw`]）
    pub raw: Value,
}

/// ツリーのノード 1 つ
#[derive(Debug, Clone, PartialEq)]
pub struct Node {
    pub name: String,
    pub kind: u64,
    pub detail: Option<String>,
    pub place: Place,
    pub raw: Value,
    /// 親（根は `None`）。ID は 1 始まり
    pub parent: Option<usize>,
    pub depth: usize,
    pub expanded: bool,
    /// 読み込んだ子（`None` = まだ問い合わせていない）
    pub children: Option<Vec<usize>>,
}

/// 見えている行 1 つ（[`Tree::rows`]）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Row {
    /// ノードの ID（1 始まり。展開しても変わらない = CLI / MCP の `item`）
    pub id: usize,
    pub depth: usize,
}

/// 展開・折りたたみの結果
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Expand {
    /// 状態が変わった
    Changed,
    /// もともとその状態だった
    Unchanged,
    /// 子をまだ読んでいない（呼び手がサーバへ問い合わせて [`Tree::set_children`] する）
    NeedsLoad,
}

/// ツリーの操作の失敗
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TreeError {
    /// その ID のノードが無い
    NoNode(usize),
    /// 深さの上限（[`MAX_DEPTH`]）を超える展開
    TooDeep(usize),
}

/// 階層ツリー（呼び出し階層・型階層の答え）。**展開状態の正本**
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Tree {
    nodes: Vec<Node>,
    roots: Vec<usize>,
    /// 上限（[`MAX_CHILDREN`] / [`MAX_NODES`]）で足さなかった子の数
    pub truncated: usize,
}

impl Tree {
    /// 根だけのツリー（どれも閉じていて子は未読み込み）
    pub fn new(roots: Vec<NodeSeed>) -> Self {
        let mut tree = Self::default();
        for seed in roots.into_iter().take(MAX_ROOTS) {
            let id = tree.push(seed, None, 0);
            tree.roots.push(id);
        }
        tree
    }

    fn push(&mut self, seed: NodeSeed, parent: Option<usize>, depth: usize) -> usize {
        self.nodes.push(Node {
            name: seed.name,
            kind: seed.kind,
            detail: seed.detail,
            place: seed.place,
            raw: seed.raw,
            parent,
            depth,
            expanded: false,
            children: None,
        });
        self.nodes.len()
    }

    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    pub fn roots(&self) -> &[usize] {
        &self.roots
    }

    /// ID（1 始まり）のノード
    pub fn node(&self, id: usize) -> Option<&Node> {
        id.checked_sub(1).and_then(|i| self.nodes.get(i))
    }

    fn node_mut(&mut self, id: usize) -> Result<&mut Node, TreeError> {
        id.checked_sub(1)
            .and_then(|i| self.nodes.get_mut(i))
            .ok_or(TreeError::NoNode(id))
    }

    /// 見えている行（根から深さ優先。閉じたノードの子は出ない）
    pub fn rows(&self) -> Vec<Row> {
        let mut out = Vec::new();
        let mut stack: Vec<usize> = self.roots.iter().rev().copied().collect();
        while let Some(id) = stack.pop() {
            let Some(node) = self.node(id) else {
                continue;
            };
            out.push(Row {
                id,
                depth: node.depth,
            });
            if node.expanded {
                if let Some(children) = &node.children {
                    stack.extend(children.iter().rev());
                }
            }
        }
        out
    }

    /// 開いているノードの ID（昇順）
    pub fn expanded(&self) -> Vec<usize> {
        (1..=self.nodes.len())
            .filter(|id| self.node(*id).is_some_and(|n| n.expanded))
            .collect()
    }

    /// 展開 / 折りたたみ。子を読んでいないノードの展開は状態を変えずに
    /// [`Expand::NeedsLoad`]（呼び手が問い合わせて [`Self::set_children`] = 開く）
    pub fn set_expanded(&mut self, id: usize, expanded: bool) -> Result<Expand, TreeError> {
        let node = self.node_mut(id)?;
        if expanded && node.children.is_none() {
            if node.depth + 1 > MAX_DEPTH {
                return Err(TreeError::TooDeep(id));
            }
            return Ok(Expand::NeedsLoad);
        }
        if node.expanded == expanded {
            return Ok(Expand::Unchanged);
        }
        node.expanded = expanded;
        Ok(Expand::Changed)
    }

    /// 読み込んだ子を足して開く。返すのは足した数（上限で足さなかった数は [`Self::truncated`]）。
    /// 既に読み込んでいたら置き換えない（開くだけ）
    pub fn set_children(&mut self, id: usize, seeds: Vec<NodeSeed>) -> Result<usize, TreeError> {
        let depth = self.node(id).ok_or(TreeError::NoNode(id))?.depth + 1;
        if depth > MAX_DEPTH {
            return Err(TreeError::TooDeep(id));
        }
        if self.node(id).is_some_and(|n| n.children.is_some()) {
            self.node_mut(id)?.expanded = true;
            return Ok(0);
        }
        let room = MAX_NODES.saturating_sub(self.nodes.len()).min(MAX_CHILDREN);
        let offered = seeds.len();
        let ids: Vec<usize> = seeds
            .into_iter()
            .take(room)
            .map(|seed| self.push(seed, Some(id), depth))
            .collect();
        self.truncated += offered - ids.len();
        let added = ids.len();
        let node = self.node_mut(id)?;
        node.children = Some(ids);
        node.expanded = true;
        Ok(added)
    }
}

/// 結果の中身
#[derive(Debug, Clone, PartialEq)]
pub enum Body {
    /// 参照の一覧（行の ID = 添字 + 1）
    List(Vec<Place>),
    /// 階層ツリー
    Tree(Tree),
}

impl Body {
    /// 保持している行の数（一覧の行・ツリーのノード）
    pub fn len(&self) -> usize {
        match self {
            Body::List(items) => items.len(),
            Body::Tree(tree) => tree.len(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// 行 / ノードの ID（1 始まり）の着地先
    pub fn place(&self, id: usize) -> Option<&Place> {
        match self {
            Body::List(items) => id.checked_sub(1).and_then(|i| items.get(i)),
            Body::Tree(tree) => tree.node(id).map(|n| &n.place),
        }
    }
}

/// 1 つの結果（問い合わせたペインごとに 1 つ。次の問い合わせで置き換わる）
#[derive(Debug, Clone, PartialEq)]
pub struct XrefResult {
    /// 問い合わせたペイン（閉じたら結果も消える）
    pub pane: PaneId,
    /// 問い合わせた文書（展開の問い合わせはこの文書を受け持つサーバへ送る）
    pub path: PathBuf,
    /// 問い合わせた位置（行 1 始まり・桁 0 始まりの行内 UTF-8 バイト）
    pub line: usize,
    pub column: usize,
    /// その位置の識別子（見出しに出す。識別子でなければ空）
    pub symbol: String,
    pub kind: XrefKind,
    /// 答えたサーバの ID
    pub server: String,
    pub body: Body,
    /// 参照の全件数（[`MAX_REFERENCES`] で切る前）。ツリーはノード数と同じ
    pub total: usize,
    /// 置いた順の番号（[`XrefStore::put`] が振る。背景の展開の答えが同じ結果へ戻るかを見る）
    pub seq: u64,
}

/// ペインごとの結果の置き場（GUI が 1 つ持つ。CLI / MCP も dispatch 経由でこれを読む）
#[derive(Debug, Clone, Default)]
pub struct XrefStore {
    results: Vec<XrefResult>,
    /// 右パネルが出す結果（最後に置いた / 選んだ）
    active: Option<PaneId>,
    seq: u64,
}

impl XrefStore {
    /// 結果を置く（同じペインの前の結果は置き換える）。置いた結果が右パネルに出る
    pub fn put(&mut self, mut result: XrefResult) -> u64 {
        self.seq += 1;
        result.seq = self.seq;
        let pane = result.pane;
        self.results.retain(|r| r.pane != pane);
        self.results.push(result);
        self.active = Some(pane);
        self.seq
    }

    pub fn get(&self, pane: PaneId) -> Option<&XrefResult> {
        self.results.iter().find(|r| r.pane == pane)
    }

    pub fn get_mut(&mut self, pane: PaneId) -> Option<&mut XrefResult> {
        self.results.iter_mut().find(|r| r.pane == pane)
    }

    /// 右パネルが出している結果
    pub fn active(&self) -> Option<&XrefResult> {
        self.active.and_then(|pane| self.get(pane))
    }

    /// 対象の結果のペイン（`pane` を省けば右パネルの結果）
    pub fn resolve(&self, pane: Option<PaneId>) -> Option<PaneId> {
        match pane {
            Some(pane) => self.get(pane).map(|r| r.pane),
            None => self.active().map(|r| r.pane),
        }
    }

    /// 右パネルに出す結果を選ぶ（無いペインなら何もしない）
    pub fn set_active(&mut self, pane: PaneId) -> bool {
        let found = self.get(pane).is_some();
        if found {
            self.active = Some(pane);
        }
        found
    }

    /// そのペインの結果を消す。右パネルの結果を消したら、残りの最後のものが出る
    pub fn remove(&mut self, pane: PaneId) -> bool {
        let before = self.results.len();
        self.results.retain(|r| r.pane != pane);
        if self.active == Some(pane) {
            self.active = self.results.last().map(|r| r.pane);
        }
        before != self.results.len()
    }

    /// `live` が偽のペインの結果を消す（閉じたペインの取りこぼしの保険）。消した数を返す
    pub fn retain_panes(&mut self, live: impl Fn(PaneId) -> bool) -> usize {
        let dead: Vec<PaneId> = self
            .results
            .iter()
            .map(|r| r.pane)
            .filter(|p| !live(*p))
            .collect();
        for pane in &dead {
            self.remove(*pane);
        }
        dead.len()
    }

    /// 保持している結果の数（保持件数の口 = `tako lsp results` の `retained`）
    pub fn len(&self) -> usize {
        self.results.len()
    }

    pub fn is_empty(&self) -> bool {
        self.results.is_empty()
    }

    /// 保持している行の総数（一覧の行 + ツリーのノード）
    pub fn rows(&self) -> usize {
        self.results.iter().map(|r| r.body.len()).sum()
    }

    pub fn iter(&self) -> impl Iterator<Item = &XrefResult> {
        self.results.iter()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn loc(uri: &str, line: u64, character: u64) -> Value {
        json!({
            "uri": uri,
            "range": {
                "start": { "line": line, "character": character },
                "end": { "line": line, "character": character + 3 },
            },
        })
    }

    #[test]
    fn 種類の綴りと_method_は_1_対_1() {
        for kind in XrefKind::ALL {
            assert_eq!(XrefKind::parse(kind.slug()), Some(kind));
            assert!(kind.provider_key().ends_with("Provider"));
            assert_eq!(kind.is_tree(), kind.prepare_method().is_some());
        }
        assert_eq!(
            XrefKind::NAMES.to_vec(),
            XrefKind::ALL.iter().map(|k| k.slug()).collect::<Vec<_>>()
        );
        assert_eq!(XrefKind::parse("incoming_calls"), None);
        assert_eq!(
            XrefKind::IncomingCalls.method(),
            "callHierarchy/incomingCalls"
        );
        assert_eq!(
            XrefKind::Subtypes.prepare_method(),
            Some("textDocument/prepareTypeHierarchy")
        );
    }

    #[test]
    fn 能力は真偽でもオブジェクトでも読む() {
        let caps = json!({
            "referencesProvider": true,
            "callHierarchyProvider": {},
            "typeHierarchyProvider": false,
        });
        assert!(server_supports(&caps, XrefKind::References));
        assert!(server_supports(&caps, XrefKind::IncomingCalls));
        assert!(server_supports(&caps, XrefKind::OutgoingCalls));
        assert!(!server_supports(&caps, XrefKind::Supertypes));
        assert!(!server_supports(&json!({}), XrefKind::References));
    }

    #[test]
    fn 参照は重複を畳んで並べ全件数を残す() {
        let (items, total) = parse_references(&json!([
            loc("file:///w/b.rs", 4, 2),
            loc("file:///w/a.rs", 9, 0),
            loc("file:///w/a.rs", 2, 7),
            loc("file:///w/b.rs", 4, 2),
            { "uri": "file:///broken" },
            7,
        ]));
        assert_eq!(total, 3);
        let flat: Vec<(&str, usize, usize)> = items
            .iter()
            .map(|l| (l.uri.as_str(), l.line, l.character))
            .collect();
        assert_eq!(
            flat,
            vec![
                ("file:///w/a.rs", 2, 7),
                ("file:///w/a.rs", 9, 0),
                ("file:///w/b.rs", 4, 2),
            ]
        );
        assert_eq!(parse_references(&Value::Null), (Vec::new(), 0));
        assert_eq!(parse_references(&json!([])), (Vec::new(), 0));
    }

    #[test]
    fn 参照は上限で切り全件数は切る前() {
        let many: Vec<Value> = (0..(MAX_REFERENCES as u64 + 25))
            .map(|n| loc("file:///w/a.rs", n, 0))
            .collect();
        let (items, total) = parse_references(&Value::Array(many));
        assert_eq!(items.len(), MAX_REFERENCES);
        assert_eq!(total, MAX_REFERENCES + 25);
    }

    fn item(name: &str, uri: &str, line: u64) -> Value {
        json!({
            "name": name,
            "kind": 12,
            "detail": "fn x()",
            "uri": uri,
            "range": { "start": { "line": line, "character": 0 }, "end": { "line": line + 3, "character": 1 } },
            "selectionRange": { "start": { "line": line, "character": 3 }, "end": { "line": line, "character": 8 } },
            "data": { "opaque": 1 },
        })
    }

    #[test]
    fn 階層の要素は選択範囲へ着地し要素そのものを残す() {
        let items = parse_items(&json!([item("helper", "file:///w/a.rs", 4), { "name": "x" }]));
        assert_eq!(items.len(), 1);
        assert_eq!((items[0].line, items[0].character), (4, 3));
        assert_eq!(items[0].raw["data"], json!({ "opaque": 1 }));
        assert_eq!(symbol_kind_name(items[0].kind), "function");
        assert_eq!(items[0].detail.as_deref(), Some("fn x()"));
        assert!(parse_items(&Value::Null).is_empty());
    }

    #[test]
    fn 呼び出し元は呼び出しの位置へ呼び出し先は定義へ着地する() {
        let site = json!([{ "start": { "line": 20, "character": 8 }, "end": { "line": 20, "character": 14 } }]);
        let incoming = parse_expand(
            &json!([{ "from": item("main", "file:///w/m.rs", 10), "fromRanges": site }]),
            XrefKind::IncomingCalls,
        );
        assert_eq!(
            (
                incoming[0].name.as_str(),
                incoming[0].line,
                incoming[0].character
            ),
            ("main", 20, 8)
        );
        let outgoing = parse_expand(
            &json!([{ "to": item("leaf", "file:///w/l.rs", 30), "fromRanges": site }]),
            XrefKind::OutgoingCalls,
        );
        assert_eq!((outgoing[0].line, outgoing[0].character), (30, 3));
        // 型階層は要素の配列そのもの
        let supers = parse_expand(
            &json!([item("Base", "file:///w/b.rs", 1)]),
            XrefKind::Supertypes,
        );
        assert_eq!(supers[0].name, "Base");
        // fromRanges が無い呼び出し元は定義へ
        let bare = parse_expand(
            &json!([{ "from": item("solo", "file:///w/s.rs", 5) }]),
            XrefKind::IncomingCalls,
        );
        assert_eq!((bare[0].line, bare[0].character), (5, 3));
        assert!(parse_expand(&json!([]), XrefKind::References).is_empty());
    }

    #[test]
    fn 種類の名前() {
        assert_eq!(symbol_kind_name(23), "struct");
        assert_eq!(symbol_kind_name(26), "type-parameter");
        assert_eq!(symbol_kind_name(0), "symbol");
        assert_eq!(symbol_kind_name(99), "symbol");
    }

    fn place(path: &str, line: usize) -> Place {
        Place {
            uri: format!("file://{path}"),
            path: PathBuf::from(path),
            line,
            column: 0,
            char_column: 0,
            excerpt: String::new(),
        }
    }

    fn seed(name: &str) -> NodeSeed {
        NodeSeed {
            name: name.into(),
            kind: 12,
            detail: None,
            place: place(&format!("/w/{name}.rs"), 0),
            raw: json!({ "name": name }),
        }
    }

    #[test]
    fn ツリーの展開と折りたたみは行の並びへ写る() {
        let mut tree = Tree::new(vec![seed("root")]);
        assert_eq!(tree.rows(), vec![Row { id: 1, depth: 0 }]);
        // 読んでいない子は開かない（呼び手が問い合わせる）
        assert_eq!(tree.set_expanded(1, true), Ok(Expand::NeedsLoad));
        assert!(tree.expanded().is_empty());
        assert_eq!(tree.set_children(1, vec![seed("a"), seed("b")]), Ok(2));
        assert_eq!(tree.expanded(), vec![1]);
        assert_eq!(
            tree.rows()
                .iter()
                .map(|r| (r.id, r.depth))
                .collect::<Vec<_>>(),
            vec![(1, 0), (2, 1), (3, 1)]
        );
        // 孫を読んで開く → 深さ優先の並び
        assert_eq!(tree.set_children(2, vec![seed("a1")]), Ok(1));
        assert_eq!(
            tree.rows().iter().map(|r| r.id).collect::<Vec<_>>(),
            vec![1, 2, 4, 3]
        );
        // 親を閉じると孫ごと消え、開き直すと孫の展開も戻る（状態は消さない）
        assert_eq!(tree.set_expanded(1, false), Ok(Expand::Changed));
        assert_eq!(tree.rows().len(), 1);
        assert_eq!(tree.set_expanded(1, false), Ok(Expand::Unchanged));
        assert_eq!(tree.set_expanded(1, true), Ok(Expand::Changed));
        assert_eq!(tree.rows().len(), 4);
        assert_eq!(tree.expanded(), vec![1, 2]);
        // 子が 0 件のノードも「読んだ」（もう問い合わせない）
        assert_eq!(tree.set_children(3, Vec::new()), Ok(0));
        assert_eq!(tree.set_expanded(3, false), Ok(Expand::Changed));
        assert_eq!(tree.set_expanded(3, true), Ok(Expand::Changed));
        // 無い ID
        assert_eq!(tree.set_expanded(99, true), Err(TreeError::NoNode(99)));
        assert_eq!(tree.set_expanded(0, true), Err(TreeError::NoNode(0)));
        // 読み込み済みへの set_children は置き換えない（開くだけ）
        assert_eq!(tree.set_children(1, vec![seed("z")]), Ok(0));
        assert_eq!(tree.len(), 4);
    }

    #[test]
    fn ツリーは子と全体の上限で切り深さで止まる() {
        let mut tree = Tree::new(vec![seed("root")]);
        let many: Vec<NodeSeed> = (0..MAX_CHILDREN + 7)
            .map(|i| seed(&format!("c{i}")))
            .collect();
        assert_eq!(tree.set_children(1, many), Ok(MAX_CHILDREN));
        assert_eq!(tree.truncated, 7);
        // 全体の上限
        let mut big = Tree::new(vec![seed("root")]);
        let mut parent = 1;
        while big.len() < MAX_NODES {
            let seeds: Vec<NodeSeed> = (0..MAX_CHILDREN).map(|i| seed(&format!("n{i}"))).collect();
            big.set_children(parent, seeds).unwrap();
            parent += 1;
        }
        assert_eq!(big.len(), MAX_NODES);
        let before = big.truncated;
        assert_eq!(big.set_children(parent, vec![seed("over")]), Ok(0));
        assert_eq!(big.truncated, before + 1);
        // 深さの上限
        let mut deep = Tree::new(vec![seed("d0")]);
        for id in 1..=MAX_DEPTH {
            assert_eq!(deep.set_children(id, vec![seed("d")]), Ok(1), "深さ {id}");
        }
        let last = deep.len();
        assert_eq!(deep.node(last).map(|n| n.depth), Some(MAX_DEPTH));
        assert_eq!(deep.set_expanded(last, true), Err(TreeError::TooDeep(last)));
        assert_eq!(
            deep.set_children(last, vec![seed("x")]),
            Err(TreeError::TooDeep(last))
        );
    }

    fn result(pane: u64, rows: usize) -> XrefResult {
        XrefResult {
            pane: PaneId::from_raw(pane),
            path: PathBuf::from("/w/src/main.rs"),
            line: 1,
            column: 0,
            symbol: "x".into(),
            kind: XrefKind::References,
            server: "s".into(),
            body: Body::List((0..rows).map(|i| place("/w/a.rs", i)).collect()),
            total: rows,
            seq: 0,
        }
    }

    #[test]
    fn 置き場はペインごとに_1_つで閉じたら消える() {
        let mut store = XrefStore::default();
        assert_eq!(store.put(result(3, 2)), 1);
        assert_eq!(store.put(result(5, 4)), 2);
        assert_eq!((store.len(), store.rows()), (2, 6));
        assert_eq!(store.active().map(|r| r.pane.as_u64()), Some(5));
        // 同じペインの次の問い合わせは置き換える
        assert_eq!(store.put(result(3, 1)), 3);
        assert_eq!((store.len(), store.rows()), (2, 5));
        assert_eq!(store.active().map(|r| r.seq), Some(3));
        // 省略は右パネルの結果
        assert_eq!(store.resolve(None), Some(PaneId::from_raw(3)));
        assert_eq!(store.resolve(Some(PaneId::from_raw(9))), None);
        assert!(store.set_active(PaneId::from_raw(5)));
        assert!(!store.set_active(PaneId::from_raw(9)));
        // 右パネルの結果を消すと残りが出る
        assert!(store.remove(PaneId::from_raw(5)));
        assert_eq!(store.active().map(|r| r.pane.as_u64()), Some(3));
        assert!(!store.remove(PaneId::from_raw(5)));
        // 閉じたペインの取りこぼしの保険 → 保持件数 0
        assert_eq!(store.retain_panes(|p| p.as_u64() != 3), 1);
        assert_eq!((store.len(), store.rows()), (0, 0));
        assert!(store.active().is_none());
    }

    #[test]
    fn 行の_id_から着地先を引く() {
        let list = Body::List(vec![place("/w/a.rs", 4), place("/w/b.rs", 7)]);
        assert_eq!(list.place(2).map(|p| p.line), Some(7));
        assert_eq!(list.place(0), None);
        assert_eq!(list.place(3), None);
        let mut tree = Tree::new(vec![seed("root")]);
        tree.set_children(1, vec![seed("a")]).unwrap();
        let body = Body::Tree(tree);
        assert_eq!(
            body.place(2).map(|p| p.path.clone()),
            Some(PathBuf::from("/w/a.rs"))
        );
        assert_eq!(body.len(), 2);
    }
}
