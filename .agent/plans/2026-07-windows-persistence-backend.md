# Windows バックグラウンド永続バックエンド設計（Issue #518 / 抽象境界 B2）

- 作成日: 2026-07-25
- 対象 commit: `67fe297`（origin/main。#467 P0 マージ直後）
- 位置づけ: **設計のみ。コード変更なし**。実装は #519（プレ版 v1b）が本文書に沿って行う
- 前提資料:
  - `.agent/plans/2026-07-windows-port-architecture.md`（抽象境界カタログ・サポートマトリクス・原則）
  - `.agent/plans/2026-07-windows-port-survey.md`（全数調査。§2.1 に案 A/B/C）
- 調査方法: リポジトリ静的読解（`rg` + 該当箇所の実読）。コード実行・ビルドは行っていない
  （本体 worktree が別 worker 使用中のため）

---

## 0. この設計が答える問い

親設計（architecture.md）は B2 を「最大の境界」とだけ定義し、中身を本 Issue へ委譲した。
本文書が確定させるのは次の 5 点（Issue #518 のスコープ 1〜5）。

1. `trait SessionBackend` の境界（何がこの trait に入り、何が入らないか）
2. Windows 永続戦略の決定（案 A/B/C の再評価）
3. orchestrator の縮退モード定義
4. 縮退時の UI 表示方針
5. macOS 復元系（#30 / #113 / #177 / #381）を壊さない回帰テスト計画

**本文書の最重要の主張は §1 にある。** tmux は tako にとって単一の役割ではなく
2 つの独立した役割を担っており、Windows で失うのは主に片方だけである。
この分離を trait に写し取らないと、境界は tmux コマンドの薄いラッパーになり、
呼び出し側が「in-process 経路」と「フォールバック経路」を区別できないまま Windows へ流れる。

---

## 1. 現状分析: tmux は 2 つの別々の役割を担っている

コードを読むまで、tmux 依存は「7 サブシステムに散らばった 350+ 箇所」という粒度でしか
把握されていなかった。実際に読むと、依存は次の 2 種類にきれいに割れる。

### 役割 A: 生存の器（process container）

シェルの PTY を tako プロセスの外に置き、tako が死んでも実行中プロセスと画面内容を保持する。

実体は 1 箇所しかない — `tmux_backend::wrap_options`（`tmux_backend.rs:134`）。
これは `SpawnOptions` を「`tmux new-session -A -D` を起動する `SpawnOptions`」へ書き換える
**純粋な変換関数**であり、PTY を所有するのは相変わらず in-process の
`TerminalSession::spawn`（`main.rs:4110`）である。tmux は PTY の中身にすぎない。

適用点も 2 箇所だけ（`main.rs:4095` の `spawn_session`、`main.rs:13479` の
`reserve_backend_session`）。**器の役割は境界として極めて小さい。**

### 役割 B: アウトオブプロセス到達（detached reach）

tako-app が動いていない、あるいはペインが tako の管理から外れているときに、
CLI / daemon / MCP から画面を読み、キーを送り、履歴を採取する手段。

こちらが「350+ 箇所」の正体で、`dispatch.rs` の tmux 直呼び約 40 箇所・`remote.rs` 155 ヒット・
`agents.rs` / `sessions.rs` / `orchestrator/*` がここに属する。

**そして決定的な事実: 役割 B はほぼ全経路で「フォールバック」であって主経路ではない。**

| 経路 | 主（tmux 非依存） | フォールバック（tmux） |
|---|---|---|
| `Request::Send` | `host.queue_send_flow` / `session.write`（`dispatch.rs:640-658`） | `spawn_tmux_delivery` / `tmux::send_keys`（同 `:665-675`） |
| `Request::Read` | `host.session(pane).visible_lines()`（`dispatch.rs:690-696`） | `tmux::capture_session`（同 `:704`） |
| `worker_status` の画面採取 | `live_tail`（in-process） | `capture_session`（`dispatch.rs:5511-5516`） |
| remote `/api/v2/panes` | `refresh_pane_mapping`（IPC 経由） | `tmux_list_panes_v2`（`remote.rs:3033`） |
| スクロール | 直接ペイン = alacritty `display_offset`（`main.rs:8880`） | backend ペイン = `scroll_mirror`（同 `:8877`） |
| pane_log | 直接ペイン = `session.history_plain_lines`（`main.rs:5183-5195`） | backend ペイン = `pane_log_probe_batch`（`main.rs:1058`） |

`resolve_pane` が成功する限り tmux は一切呼ばれない。tmux が呼ばれるのは
**「tako-app が居ない」または「ペインが消えた」ときだけ**である。

### 1.1 この分析から導かれる 3 つの訂正

Issue #518 と調査レポートの前提のうち、実コードと食い違う点を先に申告する。

**訂正 1: 「縮退モード = 送達確認なし」は正しくない。**
orchestrator spawn のプロンプト送達は `host.queue_prompt_flow`（`dispatch.rs:5055` / `:5261`）
= GUI 側の `PromptFlow` を通る。`PromptFlow` は `session.visible_lines()` /
`session.paste()` / `session.write()` のみを使い（`main.rs:3806-3939`）、**tmux に一切依存しない**。
信頼ダイアログ承諾・入力欄への反映確認・送信検証・Enter 再送はすべて維持される。
tmux 依存の `deliver_via_tmux`（`claude_tui.rs:429`）が呼ばれるのは
`Request::Send` の pane 解決失敗時だけ（`dispatch.rs:617` / `:665`）。
→ **Windows で失われるのは「GUI 不在時の送達」であり「送達確認」ではない。**

**訂正 2: report の代替はすでに存在する。**
`orchestrator report` 第 1 層（`capture_scrollback_joined`、`dispatch.rs:4911`）は tmux 専用だが、
pane_log（FR-5.13、#112）が **ディスク永続の平文ログ**として同じ情報を持つ。
pane_log の直接ペイン経路は alacritty history の増分取り込みで完全に実装済みであり
（`main.rs:5183-5205`）、`tako logs show` はペイン死亡後も読める。
→ Windows の report 第 1 層は「tmux scrollback」→「pane_log」への差し替えで成立する。
alacritty history の直読（`session.history_plain_lines`）は GUI 稼働中しか使えないため、
**ディスクに落ちている pane_log の方が tmux scrollback の正しい対応物**である。

**訂正 3: スクロールは縮退しない。**
`scroll_mirror` は「履歴を tmux が持っている」ことへの対策として生まれた（#159）。
バックエンドが無ければ履歴は in-process の alacritty が持ち、直接ペイン経路
（ピクセル単位・#159 の本命実装）がそのまま効く。
→ Windows のスクロールは `Degraded` ではなく `Supported`。

---

## 2. Windows 永続戦略の再評価（案 A / B / C）

調査レポート §2.1 の推奨は C だった。**再評価しても C を採る**が、根拠は調査時より強くなり、
同時に「C の trait は B をそのまま受け入れられる形でなければならない」という条件が付く。

### 2.1 案 A: WSL2 必須（棄却）

`wsl tmux ...` でパスと socket を橋渡しする案。

棄却理由は調査レポートのとおり（ゼロコンフィグ原則違反）だが、コードを読んで**より強い棄却理由**が
見つかった。役割 A（器）は `wrap_options` が返す `SpawnOptions` を in-process の PTY で起動する
構造であり、WSL2 を挟むと **PowerShell ペインの PTY が WSL の中に入る**。
tako の主用途は「Windows ネイティブのシェルで開発する」ことなので、
永続化のためにシェルが Linux になるのは目的と手段の転倒である。
部分適用（WSL ペインだけ永続）は `backend_sessions` の有無でペインの性質が二分され、
復元・orchestrator・pane_log のすべてに二重経路を生む。**却下。**

### 2.2 案 B: ConPTY ネイティブ + 独自永続層（今は採らない。ただし温存する）

`tako session-host` のような別プロセスが ConPTY を所有し、named pipe で
attach / capture / send を提供する = ミニ tmux。

技術的には成立する（ConPTY ハンドルは作成プロセスが所有するので、
ホストプロセスを分ければ tako 終了後もシェルは生きる）。B が唯一「役割 A + 役割 B の両方」を
Windows で回復できる案でもある。

**今は採らない理由**は工数だけではない。自前永続層は
「tako が死んでも生きているプロセス群」を tako 自身が管理することになり、
macOS 側で #113 / #177 / #381 が示した事故（強奪・ゾンビ・縮退保存）のクラスを
**新規に自作すること**を意味する。これらは tmux という枯れた実装の上でさえ 4 回踏んだ。
最初の Windows リリースで背負う負債としては大きすぎる。

**ただし B は死んでいない。** 本設計の trait は
「B の実装を後から差し込んでも呼び出し側を一切変更しなくてよい」ことを設計の合格条件にする（§3.6）。
段階も切っておく:

- **B-1（器のみ）**: `survives_app_exit = true` / `detached_access = false`。
  セッションホストは PTY を保持するだけで、画面採取・キー送出は提供しない。
  これだけで persist の完全復元が戻る。実装量は B 全体の 1/3 程度
- **B-2（到達つき）**: capture / send / probe を named pipe 越しに提供。
  orchestrator の縮退が全解除される

### 2.3 案 C: 抽象化 + 段階導入（採用）

Windows は当面 `NullBackend`（器なし・到達なし）。#30 で実装・検証済みの
「tmux 不在 = 構造のみ永続化」経路を正式仕様にする。

**再評価で C が強くなった根拠（調査時には未確認だった実コードの事実）:**

1. 劣化経路は仮説ではなく**現に本番で走っている**。`save_layout` のゲートは
   `tmux_persist`（ユーザー設定）だけで、`available()` は見ない（`main.rs:5020`）。
   復元側は `tmux_available` で分岐してメッセージまで分けている
   （`main.rs:1820-1824`「tmux 不在: タブ構成のみ・新シェルで開き直し」）。
   Homebrew 配布先（tmux 無し）はこの経路の本番実績である
2. 構造のみ復元の上に、**意味的な復元がすでに載っている**。
   `claude_resume_command`（`main.rs:2228`）は backend が死んでいても
   claude transcript が残っていれば `claude --resume` を新シェルへ流し込む。
   「画面は消えるが会話は続く」= Windows 初期リリースの体験は「全部消える」ではない
3. 縮退の実幅は §1.1 の訂正 3 件ぶん狭い（送達確認は残る / report は pane_log で代替 /
   スクロールは無傷）

**トレードオフ（正直に）**: C の Windows 初期リリースでは
「tako を閉じると実行中のエージェントが死ぬ」。これは tako の中核価値
（エージェント集約監視）に対して小さくない欠落であり、Windows ユーザーにとっては
「tako を開きっぱなしにする」運用が前提になる。ただし macOS でも persist OFF 設定は
選択可能であり、未知の自前永続層より既知の制約の方が扱いやすい。

### 2.4 判断

| 案 | 判断 | 理由（1 行） |
|---|---|---|
| A: WSL2 | **棄却** | ネイティブシェルのペインが永続対象外になる歪みが構造的に解けない |
| B: 独自永続層 | **保留（後継 impl として温存）** | 唯一の完全解だが、macOS で 4 回踏んだ事故クラスを自作することになる |
| C: 抽象化 + NullBackend | **採用** | 劣化経路は既に本番実績あり。縮退幅は調査時想定より狭い |

**推奨: C を採用し、trait は B-1 / B-2 を無改修で受け入れられる形に切る。**

---

## 3. `trait SessionBackend` の境界設計

### 3.1 素朴な案（Issue の列挙そのまま）を採らない理由

Issue は `create / attach / detach / capture / send_keys / list / kill / resize` を挙げている。
これは tmux サブコマンドの写しであり、2 つの問題がある。

**問題 1: `attach` / `create` は tako の構造に存在しない。**
tako は backend に PTY を作らせない。`wrap_options` で `SpawnOptions` を書き換え、
in-process の `TerminalSession::spawn` が起動する。`new-session -A` が
「新規」と「再 attach」を同一コマンドにしているのも、この構造だから成り立っている。
`create()` / `attach()` を trait に置くと、この構造を trait 側が持てず、
実装が `SpawnOptions` を返すだけの嘘のメソッドになる。
**正しい primitive は `wrap_spawn(SpawnOptions) -> SpawnOptions`。**

**問題 2: `capture` / `send_keys` を同じ trait に置くと、役割 A と B が混ざる。**
呼び出し側は「in-process 経路が使えないときだけ backend を呼ぶ」という規律を持っているが
（§1 の表）、この規律は現在**どこにも型で表現されていない**。同じ trait に置くと、
Windows 実装が全メソッド `Err` を返すだけになり、呼び出し側は
「フォールバックが失敗した」のか「そもそも到達手段が無い」のかを区別できない。

### 3.2 採る形: 能力ベースの 2 trait + 不透明な参照型

```rust
// crates/tako-core/src/backend/mod.rs

/// バックエンドセッションの参照。文字列直渡しを禁止するための newtype。
/// #428（`"session:0.0"` を session 名として渡し、`=session:0.0:` で
/// can't find pane の無音失敗）は、この型があれば構造的に起きない
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct SessionRef(String);

pub struct BackendCapabilities {
    /// tako 終了後もセッション内プロセスが生き残るか（役割 A）
    pub survives_app_exit: bool,
    /// tako-app 不在 / ペイン消失時に画面採取・入力送出ができるか（役割 B）
    pub detached_access: bool,
    /// スクロールバックの権威がどちらにあるか
    pub scrollback: ScrollbackAuthority,   // Backend | InProcess
    /// UI・診断・prompt に出す名前（"tmux" / "none"）
    pub label: &'static str,
    /// 能力が落ちている理由。サポートマトリクスの note と同一文字列を使う
    pub degraded_note: Option<&'static str>,
}

/// 役割 A: 生存の器
pub trait SessionBackend: Send + Sync {
    fn capabilities(&self) -> BackendCapabilities;

    /// ペインに対応するセッション参照を払い出す（器が無ければ None）
    fn reserve(&self, pane: PaneId) -> Option<SessionRef>;

    /// spawn を器の中で起動する形へ書き換える。NullBackend は恒等変換
    fn wrap_spawn(&self, opts: SpawnOptions, session: &SessionRef) -> SpawnOptions;

    fn exists(&self, session: &SessionRef) -> bool;
    fn kill(&self, session: &SessionRef) -> Result<(), BackendError>;
    fn list(&self) -> Vec<SessionInfo>;

    /// 器を握っている他インスタンスのクライアント（#177 復元強奪ガードの材料）
    fn foreign_holders(&self, sessions: &[SessionRef]) -> Vec<Holder>;

    /// 器の残骸（#191 orphan 復帰 / FR-2.16.11 cleanup の材料）
    fn orphans(&self, protected: &HashSet<SessionRef>, min_idle: Option<Duration>)
        -> Vec<SessionInfo>;

    /// ペイン配下プロセスの制御端末（listen ポート検知 FR-2.4.2 の突き合わせ。B5 と接する）
    fn pane_tty(&self, session: &SessionRef) -> Option<String>;

    fn sync_config(&self) {}

    /// 役割 B の入口。持たない実装は None を返す
    fn detached(&self) -> Option<&dyn DetachedAccess> { None }
}

/// 役割 B: アウトオブプロセス到達。**これを持たない = Windows 初期リリース**
pub trait DetachedAccess: Send + Sync {
    fn capture_screen(&self, s: &SessionRef) -> Result<Vec<String>, BackendError>;
    fn capture_history(&self, s: &SessionRef, lines: usize) -> Result<Vec<String>, BackendError>;
    fn history_probe(&self, s: &SessionRef) -> Option<HistoryProbe>;
    fn history_probe_batch(&self) -> Vec<(SessionRef, HistoryProbe)>;
    fn send_text(&self, s: &SessionRef, text: &str) -> Result<(), BackendError>;
    fn send_key(&self, s: &SessionRef, key: &str) -> Result<(), BackendError>;
    fn paste(&self, s: &SessionRef, text: &str) -> Result<(), BackendError>;
    fn send_wheel(&self, s: &SessionRef, delta: i32, col: usize, row: usize, sgr: bool);
    fn resize(&self, s: &SessionRef, cols: u16, rows: u16) -> Result<(), BackendError>;
    fn pane_pids(&self) -> Vec<(SessionRef, u32)>;
    fn has_running_children(&self, s: &SessionRef) -> bool;
}
```

実装は 2 つ。`TmuxBackend`（既存 `tmux_backend.rs` + `tmux.rs` をそのまま移設。
`detached()` は `Some(self)`）と `NullBackend`（`reserve` → `None`、
`wrap_spawn` → 恒等、`list` / `orphans` → 空、`detached()` → `None`）。

選択は `platform/mod.rs` 相当の 1 行（原則 1）。ただし**プラットフォームで固定しない**:
macOS でも tmux 不在なら `NullBackend` になるのが現状の挙動であり、それを維持する（§7 の R8 も参照）。

```rust
pub fn backend() -> &'static dyn SessionBackend {
    static B: OnceLock<Box<dyn SessionBackend>> = OnceLock::new();
    B.get_or_init(|| match backend_choice() {   // env override → tmux available → none
        Choice::Tmux => Box::new(TmuxBackend::new()),
        Choice::None => Box::new(NullBackend),
    }).as_ref()
}
```

### 3.3 呼び出し側の是正: `PaneReach`

`dispatch.rs` の tmux 直呼び約 40 箇所は、すべて同じ形をしている
（in-process 解決 → 失敗したら tmux）。これを 1 つの型に畳む。

```rust
pub enum PaneReach<'a> {
    /// tako-app が当該ペインを持っている（主経路。全機能が使える）
    InProcess(PaneId),
    /// app 不在 / ペイン消失。backend の役割 B 経由で届く
    Detached(SessionRef, &'a dyn DetachedAccess),
    /// 届かない。理由はマトリクスの note か「ペインが存在しない」
    Unreachable(UnreachableReason),
}

impl dyn ControlHost {
    fn reach<'a>(&self, pane: Option<u64>, hint: Option<&SessionRef>) -> PaneReach<'a>;
}
```

`Unreachable` は網羅 match を強制するので、Windows で `Detached` が消えたときに
「フォールバックを書き忘れた経路」がコンパイル時に露出する。
**呼び出し側に `#[cfg]` は 1 行も足さない**（原則 1）。

`UnreachableReason` は `NoBackend { note }` を持ち、note は
サポートマトリクスの `Degraded` note と同一文字列を使う（architecture.md T5「診断一致」）。

### 3.4 モジュール配置

```
crates/tako-core/src/backend/
├── mod.rs        ← trait 定義・SessionRef・capabilities・実装選択（この 1 箇所だけが選ぶ）
├── tmux/         ← 現 tmux_backend.rs + tmux.rs の移設先
└── null.rs       ← NullBackend
```

`scroll_mirror.rs` と `pane_log.rs` は backend の**利用者**であって実装ではないため
移設しない。ただし tmux 直呼びは `DetachedAccess` 経由へ差し替える。

**B2 は `platform/` ではなく `backend/` に置く。** プラットフォームの分岐ではなく
「能力の有無」の分岐であり（macOS でも tmux 不在なら NullBackend）、
`platform/` に入れると `cfg` で選ぶ誤解を生む。

### 3.5 7 サブシステムが境界越しに必要とするもの（洗い出し）

| # | サブシステム | 必要な trait メソッド | 現在の直呼び箇所 |
|---|---|---|---|
| 1 | persist（復元 / 保存 / 強奪ガード / orphan 復帰 / cleanup） | `reserve` `wrap_spawn` `exists` `kill` `list` `foreign_holders` `orphans` `sync_config` `capabilities` | `main.rs:4095,13479,2207,4635,4982,500-526,2297` |
| 2 | orchestrator spawn | `reserve`（`tmux_session` の払い出し）| `main.rs:13478` → `dispatch.rs` spawn |
| 3 | プロンプト送達 | `DetachedAccess::{capture_screen, send_key, paste}`（**フォールバック時のみ**） | `claude_tui.rs:446-535`、`dispatch.rs:617,665,4477` |
| 4 | worker 監視・報告・レジストリ | `DetachedAccess::{capture_screen, capture_history, has_running_children, pane_pids}` `exists` | `dispatch.rs:4842-4913,5511,5636`、`agents.rs:130-241`、`wait.rs:209,319` |
| 5 | スクロールミラー | `DetachedAccess::{capture_history, send_wheel}` `capabilities().scrollback` | `scroll_mirror.rs:67,124,157`、`main.rs:8827-8850` |
| 6 | pane_log | `DetachedAccess::history_probe_batch` `capture_history` | `main.rs:1058`、`tmux.rs:331-401` |
| 7 | remote | `DetachedAccess::{capture_screen, capture_history}` `list` `exists` | `remote.rs:1147,1818,1894,3026,3035` |

横断で使う `SessionRef` の払い出し元は `reserve` の 1 箇所に集約される。
現在は `new_backend_session_name()`（`main.rs:554`）が名前を作り、
`registry.rs` / `sessions.rs` / `layout.rs` が `String` で持ち回っている。
これらのフィールド型を `SessionRef` にすると #428 クラスの取り違えが構造的に消える
（永続ファイルの表現は文字列のままでよい。serde で透過）。

### 3.6 trait の合格条件

**「B-1（器だけの ConPTY セッションホスト）を実装したとき、呼び出し側の変更が 0 行で済むか」**
を trait 設計の受け入れテストとする。B-1 は `survives_app_exit = true` /
`detached_access = false` / `scrollback = InProcess` という capabilities を持つ実装であり、
これは NullBackend と TmuxBackend の**中間**にあたる。
この中間状態が表現できない trait は切り方を間違えている。

同じ理由で、`capabilities()` は bool の集合であって
`enum Backend { Tmux, None }` にはしない。呼び出し側が実装名で分岐した瞬間に
B-1 の追加が全呼び出し側の変更になる。

**M1（2026-07-27 実施）で合格条件を実際に満たした。** 段取り ①〜④ の時点では
`main.rs` の本番 spawn 経路が `tmux_backend::wrap_options` を直呼びしており、
B-1 を登録しても**その器は一度も使われない**状態だった。M1 で役割 A
（生成・破棄・列挙・環境・tty）の呼び出しを `SessionBackend` 経由へ寄せた:

- 器の割り当てと spawn の書き換えは `backend::wrap_spawn_for_pane` の 1 箇所
  （`spawn_session` と `reserve_backend_session` が共有する）。問いは
  「tmux があるか」でも「`survives_app_exit` か」でもなく **`reserve` が器を配るか**
- 保護対象の名前が 1 件でも `SessionRef` にできないときは orphan 判定を**行わない**
  （保護が欠けた集合で回すと守りたいセッションを誤爆する）
- 再発防止は番犬テスト `器のライフサイクルの直呼びが境界の外に残っていない`
  （`tests/platform_parity.rs`）。役割 B と `tako_tmux_*` の機能面は対象外

**M2（session-host 本体）が触るのは `backend/` の中だけでよい。** 具体的には
`Choice` へ実装を 1 つ足し、`SessionBackend` を実装する。呼び出し側は変更不要。
M0 の実測（`poc/conpty-survival/`）が示す制約 — セッションホストは
`DETACHED_PROCESS` で起動する — は `wrap_spawn` が返す `SpawnOptions` ではなく
**ホストの起動側**の要件なので、この境界の形と矛盾しない。

---

## 4. 7 サブシステムの縮退時期待挙動

`NullBackend`（Windows 初期リリース / macOS の tmux 不在）での期待挙動を確定する。

### 4.1 一覧

| # | サブシステム | macOS + tmux | NullBackend での期待挙動 | 分類 |
|---|---|---|---|---|
| 1 | persist | 実行中プロセス + 画面ごと完全復元 | タブ / ペイン構成・cwd・プレビュー・Web ビュー・claude 会話は復元。**実行中プロセスと画面は失われる** | `Degraded` |
| 2 | orchestrator spawn | ペイン生成 + `-e` で env 直接注入 + セッション名登録 | ペイン生成と env 注入は維持（`SpawnOptions.env` は in-process PTY にそのまま渡る）。`tmux_session` は None | `Degraded` |
| 3 | プロンプト送達 | GUI 経路 = PromptFlow / GUI 不在 = `deliver_via_tmux` | **GUI 稼働中は完全に同一**（§1.1 訂正 1）。GUI 不在時の送信は不可 | `Degraded` |
| 4 | worker 監視・報告 | 画面採取 + scrollback + transcript の 3 層 | GUI 稼働中は画面採取 OK。report は pane_log + transcript の 2 層。ペイン消失後の追跡は transcript / pane_log のみ | `Degraded` |
| 5 | スクロール | capture ベースのミラー | alacritty 履歴の直接ペイン経路（#159 本命実装） | **`Supported`** |
| 6 | pane_log | tmux capture の増分 | alacritty history の増分（実装済み・`main.rs:5183`）。**tako 停止中の出力は記録されない** | `Degraded` |
| 7 | remote | tmux ターゲット解決 + app 不在フォールバック | app 稼働中は IPC 経由で同等。**app 不在時はペイン一覧すら出ない** | `Degraded` |

### 4.2 各論と実装時の要点

**1. persist.** 変更は「ゲートの言い換え」だけで挙動は現状維持。
`tmux_backend::available()` → `backend().capabilities().survives_app_exit`。
`save_layout` が `tmux_persist`（ユーザー設定）だけを見て `available()` を見ない構造は
**#30 の根治そのもの**なので絶対に触らない（§7 R3）。
復元メッセージ（`main.rs:1820-1824`）は `capabilities().label` から生成する。
`foreign_holders` / `orphans` は NullBackend で常に空を返すため、
強奪ガード（#177）と orphan 復帰（#191）は「発動しない」= 現状と同じ。

**2. orchestrator spawn.** `reserve()` が `None` を返すと `backend_sessions` に入らない。
spawn 応答の `tmux_session` は `null` になる。これは既に `Option<String>` であり
（`registry.rs:96`、`wait.rs:42`）、`None` 時の分岐も実装済み
（`registry.rs:434`「キーが無い（tmux 無し spawn）場合は番号のみで判定」）。
**レジストリのスキーマ変更は不要**。§5 で PID 列の追加を提案するが、これは強化であって必須ではない。

**3. プロンプト送達.** `dispatch.rs:617,665` のフォールバック分岐は
`PaneReach::Unreachable` へ落ち、`DispatchError` に `note` 付きで返る。
現在は `tmux_session` が None なら元のエラーをそのまま返しているので（`dispatch.rs:676`）、
差分は「理由が構造化される」ことだけ。

**4. worker 監視・報告.** ここが実装量の中心。
- `worker_status`: `live_tail` が主。`recent_output` のフォールバックが消える
- `report`: 第 1 層を `capture_scrollback_joined` → **pane_log 読み出し**へ差し替える。
  `pane_log::latest_for_pane` + `read_tail` が既にあり（`pane_log.rs:427,437`）、
  ペイン死亡後も読める。`source` フィールドに `"pane_log"` を追加する
- `wait.rs:209,319` の「pane 消滅は tako 再起動中かもしれない」判定は
  `tmux_session_alive` に依存。NullBackend では判定不能 → **gone の確定を早める**のではなく、
  `registry` の PID 生存（§5）に置き換える

**5. スクロール.** `mirror_scroll_pane`（`main.rs:8827`）は
`backend_sessions.contains_key || tmux_view_panes.contains_key`。
NullBackend では両方空なので直接ペイン経路。**コード変更不要。**
`tako tmux open`（外部セッションの取り込み）は Windows では別途 `Unsupported`。

**6. pane_log.** 直接ペイン経路が既に完備。差分は
「backend 経路の分岐が dead になる」だけ。**ただし意味が変わる**: tmux があれば
tako 停止中の出力も次回起動時に差分として取り込めるが（`main.rs:2217-2223`）、
NullBackend では tako が動いている間の出力しか残らない。
これは `Degraded` note に明記する。

**7. remote.** `remote.rs` の app 不在フォールバック（`tmux_list_panes_v2` /
`capture_session` / `scrollback`）が全滅する。remote daemon は tako-app とは別プロセスなので、
**Windows では「tako-app が起動していないとリモートは何も見えない」**が正しい仕様になる。
daemon 起動時に capabilities を見て、app 不在時のレスポンスに
`{"error": ..., "note": ...}` を返す（現在は空配列を返す経路がある）。

---

## 5. orchestrator の縮退モード定義

Issue の 3 項目（送達確認なし・PID ベースレジストリ・alacritty history ベース report）を
§1.1 の訂正を織り込んで再定義する。

### 5.1 送達確認: 「なし」ではなく「GUI 前提」

| 条件 | 送達 | 確認 |
|---|---|---|
| tako-app 稼働 + ペイン健在 | `queue_prompt_flow` | **あり**（信頼ダイアログ承諾・入力欄反映・送信検証・Enter 再送のすべて） |
| tako-app 不在 or ペイン消失 | 不可 | — |

実装上の要件は 1 つだけ: **spawn 直後の prompt が確実に GUI 経路を通ること**。
`dispatch.rs:5261` は既に `host.queue_prompt_flow` を呼んでおり、CLI から spawn しても
IPC 経由で GUI が受け取る。CLI スタンドアロン（app 不在）で spawn する経路は
Windows では `Unreachable` で明示エラーにする（黙って送らない、が最悪）。

**縮退の可視化**: `tako orchestrator spawn` の応答に
`"delivery": {"verified_by": "prompt_flow" | "none", "note": ...}` を追加する。
master が「送ったつもりで届いていない」を検出できるようにする（#390 の
`prompt_undelivered` 判定の Windows 版に相当）。

### 5.2 レジストリ: PID を第一級のキーに昇格

現状の追跡キーは 3 段（`registry.rs:405-435`）:
`pane` → `tmux_session` → `session_id`（claude transcript）。
NullBackend では真ん中が消え、pane が消えると transcript しか残らない
（codex / agy は transcript も無い）。

**`WorkerEntry` に `pid: Option<u32>` を追加する。**

- 記録: spawn 時に `SpawnOptions` で起動した PTY の子プロセス PID
  （`TerminalSession` が持つ。in-process なので取得は容易）
- 検証: `platform::procinfo`（境界 B5）の `all_pids` / `name_of` で生存確認。
  **PID 再利用の誤判定を防ぐため `spawned_at` と併用**し、
  プロセス開始時刻が `spawned_at` より前なら別プロセスとみなす
- 位置づけ: `tmux_session` の**代替ではなく並列**。tmux があるときも記録する
  （macOS でも突然死検知の材料が増える = 純粋な強化）

追跡キーの優先順は `pane` → `tmux_session` → `pid` → `session_id`。
`tmux_session` が None のとき自動的に `pid` が繰り上がる。
スキーマは `#[serde(default)]` で後方互換。

**PID で分かること / 分からないこと**を明示する:

| | tmux_session | pid |
|---|---|---|
| プロセス生存 | 分かる | 分かる |
| 画面内容 | 分かる | **分からない** |
| busy / idle | 画面から判定 | 子プロセスの有無のみ（粗い） |
| tako 再起動をまたぐ | 分かる | **分からない**（プロセスが tako と共に死ぬ） |

最後の行が重要で、NullBackend では tako 再起動でワーカーは消える。
よって `pid` の役割は「tako 稼働中の突然死検知」に限定される。
`WORKER_DEAD`（#390）の判定材料はこれで足りる。

### 5.3 report: pane_log を第 1 層に

```
第 1 層: pane_log（ディスク永続。ペイン死亡後も読める）  ← tmux scrollback の置き換え
第 2 層: claude transcript（claude のみ。現状どおり）
```

`source` の値に `"pane_log"` を追加。tmux があるときは従来どおり `"scrollback"` を優先し、
**pane_log 経路は tmux があっても使えるようにする**（macOS でも `--source pane-log` で選べる）。
そうしないと Windows 専用コードになって macOS で腐る（原則 3）。

pane_log には制約がある。`tako logs set --enabled false` で無効化できること、
5MB ローテート・200MB 全体上限があること。
report が空を返すときは「pane_log が無効 / ローテートで消えた」を note で区別する。

### 5.4 watch

`watch` は `worker_status` のポーリングなので、上記が入れば自動的に縮退版になる。
1 点だけ明示が要る: `WORKER_GONE` の確定（`wait.rs:208`「tmux session が生きていれば
pane 消滅は tako 再起動中とみなす」）は NullBackend では取り消せない。
**NullBackend では tako 再起動 = worker 消失が事実**なので、取り消さないのが正しい。
`gone` の理由に `"backend_absent"` を付けて master が誤解しないようにする。

---

## 6. 縮退時の UI 表示方針

原則 2（使えない機能は「無い」のではなく「明示的な縮退」）に従い、
**新しい UI を作らず、既にある 4 つの表示点へ capabilities を載せる**。

| 表示点 | 現状 | Windows / NullBackend での表示 |
|---|---|---|
| 設定画面「セッション永続化」 | `desc_persist_no_tmux()` =「tmux が見つからないため構成のみ復元されます」（`ui_text/settings.rs:178`） | **既存文言を capabilities 由来へ一般化**。「バックグラウンド永続バックエンドが無いため、構成のみ復元されます（実行中のプロセスは tako 終了時に停止します）」 |
| 起動時の復元レポート | `main.rs:1820-1824` で「tmux 不在: タブ構成のみ・新シェルで開き直し」 | `capabilities().label` から生成。persist.log にも同文が残る |
| `tako persist` / `tako_persist` | `"available": tmux_backend::available()`（`dispatch.rs:1300`） | `"backend": {"label", "survives_app_exit", "detached_access", "note"}` を追加。`available` は後方互換で残す |
| 右パネル tmux ビュー | tmux セッション一覧 | NullBackend では一覧の代わりに 1 行の note。タブ名は「tmux」→ 能力名から生成（i18n 経由） |

加えて #515 / #516 の仕組みに乗せる:

- **サポートマトリクス**（#515）: §7.1 の分類を `MATRIX` へ登録。
  `tako platform --platform windows --status degraded` で一覧できる
- **master / solo system prompt**（#516）: `{{platform_notes}}` に
  マトリクスの `Degraded` note が自動で入る。master は
  「この環境では worker は tako 終了で死ぬ」「report は pane_log 由来」を
  プロンプト時点で知る。**縮退を AI に伝えるのはこの経路が正**であり、
  prompt に Windows 専用の文章を書き足さない（T6 = プラットフォーム別複製の禁止）

**note の文言は 1 箇所（マトリクス）で定義し、UI もエラーも prompt もそこから引く**
（architecture.md T5「診断一致」）。UI に文字列リテラルを書かない。
i18n（#435）の対象なので `tr!` を通す — マトリクスの note はキーを持ち、
表示時に日英カタログを引く形にする（マトリクスに日本語文字列を直書きすると英語 UI で破れる）。

---

## 7. サポートマトリクスへの登録（`Degraded` / `Pending` 一覧）

キーは MCP ツール名を正とする（architecture.md §3.2）。B2 が触るキーは以下。

### 7.1 Windows = `Degraded`

| key | note（マトリクスに登録する要旨） |
|---|---|
| `tako_persist` | 永続バックエンドが無いため、タブ・ペイン構成と cwd のみ復元。実行中プロセスと画面内容は tako 終了時に失われる |
| `tako_orchestrator_spawn` | worker は tako 終了時に停止する。spawn 応答の `tmux_session` は常に null |
| `tako_orchestrator_report` | 第 1 層は pane_log（tako 稼働中の出力のみ）。tako 停止中の出力は残らない |
| `tako_orchestrator_worker_status` | ペイン消失後の画面採取は不可。生存判定は PID と transcript のみ |
| `tako_orchestrator_workers` | 同上。`tmux_alive` は常に false |
| `tako_orchestrator_run` / `_run_status` / `_run_result` | 完了待ちは tako 稼働が前提 |
| `tako_logs` | tako 停止中の出力は記録されない |
| `tako_remote_scrollback` | tako-app 稼働中のみ。app 不在ではペイン一覧も取得できない |
| `tako_sessions` | resume は claude transcript のみが根拠（backend 生存による復帰は不可） |

### 7.2 Windows = `Pending`（#519 で実装、または後続）

| key | note | issue |
|---|---|---|
| `tako_tmux_list` / `_kill` / `_cleanup` / `_select_window` / `_resize` | 永続バックエンドが無いため管理対象が存在しない。将来のセッションホスト（案 B）で復活する | #519 |
| `tako_tmux_open` | 外部セッションの取り込みは backend 依存 | #519 |

### 7.3 Windows = `Supported`（縮退しない。誤って落とさないための明示）

`tako_scroll_pane`（§1.1 訂正 3）、`tako_send`（GUI 稼働中）、`tako_read`、
`tako_split` / `tako_close` / `tako_focus` などのレイアウト操作一式。

### 7.4 #515 とのスキーマ整合

`Support` enum・`Feature` struct・`MATRIX`・`support_for` は architecture.md §3.1 の定義を
そのまま前提にしている。#515 の実装がこれと食い違った場合、**本文書側で吸収する**
（分類の内容は変えず、表現だけ #515 のスキーマへ写す）。
本文書が #515 に対して追加で要求するのは 1 点だけ:
**`note` は表示時に i18n を通せる形であること**（§6 末尾）。
`&'static str` 直書きだと英語 UI で日本語が出る。

---

## 8. macOS 復元系を壊さないための回帰テスト計画

B2 は #30 / #113 / #177 / #381 の 4 事故が固めた不変条件の真上を通る。
**「壊さない」を人間の注意力ではなくテストで担保する**（原則 3）。

### 8.1 守るべき不変条件（実コード上の位置）

| # | 不変条件 | 実装位置 |
|---|---|---|
| I1 | 保存は `tmux_persist` 設定だけをゲートにする。backend の有無で保存を止めない（#30 根因 1） | `main.rs:5020` |
| I2 | PTY 死亡（`CloseReason::Exited`）ではセッション kill も layout 削除もしない（#30 根因 2） | `main.rs:463-469,4670` |
| I3 | 復元対象を別インスタンスが握っていたらセカンダリ降格（#177 復元強奪ガード） | `main.rs:500-526` |
| I4 | 縮退した layout の保存前に `.bak.1〜3` 退避（#177）／空 layout の保存拒否 + `layout.json.good`（#381） | `layout.rs:625,647` |
| I5 | 多重起動ガード `PRIMARY_CLAIMED` と `release_primary`（#113 / #381 / #312 再修正） | `main.rs:1649,1697` |
| I6 | 起動時 orphan cleanup の 1 時間猶予（#113） | `main.rs:550,2325` |
| I7 | orphan 自動復帰の `catch_unwind`（#381 silent death 対策） | `main.rs:2301` |

### 8.2 テスト計画

**R0（最重要・前提）: NullBackend を macOS 上で走らせられるようにする。**

現状、Windows の縮退経路を macOS で再現する手段が無い。`TAKO_PERSIST=0` は
保存ごと止めてしまい（`main.rs:5020`）、Windows の「保存する・復元は構造のみ」とは別物になる。

→ `TAKO_BACKEND=none|tmux|auto`（既定 `auto`）を新設し、`backend_choice()` の 1 箇所で解決する。
これで **macOS のセルフテスト・CI・手元検証で Windows 側の全経路が実行できる**。
これが本回帰計画の土台であり、#519 の受け入れ条件に必ず含める。

| ID | 内容 | 落ちる状況 | 実行環境 |
|---|---|---|---|
| R1 | `TmuxBackend::wrap_spawn` の生成 argv が現行 `wrap_options` とバイト等価（スナップショット） | 移設で `-u` / `-A -D` / `-e` / `-c` の順序や有無が変わった | macOS 単体 |
| R2 | I1: `capabilities().survives_app_exit == false` でも `save_layout` がファイルを書く | #30 根因 1 の再発 | `TAKO_BACKEND=none` 単体 |
| R3 | I2: `CloseReason::Exited` で `kill` が呼ばれず layout も残る（両 backend） | Exited と Explicit の取り違え | 単体（backend をモック） |
| R4 | I3: `foreign_holders` が非空 → セカンダリ降格。NullBackend では常に空 → 降格しない | 強奪ガードが trait 化で無効になった | macOS 隔離 e2e（既存 #177 手順） |
| R5 | I4: 空 workspace の保存拒否 + `.good` スナップショット（両 backend） | #381 の再発 | 単体 |
| R6 | I5: 2 プロセス目がセカンダリ、赤ボタン close 後の再取得（両 backend） | #113 / #312 の再発 | macOS e2e |
| R7 | I6/I7: `orphans` の猶予判定と `catch_unwind`。NullBackend では `orphans` が空 | cleanup が実行中 worker を巻き込む | 単体 + e2e |
| R8 | **通しセルフテスト（`TAKO_SELF_TEST=1`）を `TAKO_BACKEND=none` でも完走**させる | 縮退経路のどこかが panic / 無限待ち | macOS セルフテスト |
| R9 | `PaneReach` の網羅 match（`Unreachable` を足すとコンパイルエラー） | フォールバック未記述の経路 | コンパイル時 |
| R10 | マトリクス T1〜T4 に §7 のキーが登録済み | 分類漏れ | 単体（#515 と共有） |
| R11 | `SessionRef` が `"session:0.0"` 形式を拒否する（#428 回帰） | ターゲット形式とセッション名の取り違え | 単体 |
| R12 | report の `source` が backend 有無で `scrollback` / `pane_log` に切り替わり、両方で非空 | pane_log 経路の実装漏れ | 単体 + e2e |

**R8 の位置づけ**: セルフテストは GUI 実画面での機械検証（項目 80 本超）であり、
これを NullBackend で完走させることは「Windows で GUI が起動したら何が動くか」を
Windows 実機なしで先取りすることに等しい。#519 の主要な検証手段になる。

### 8.3 段取り（#519 への引き渡し）

呼び出し側を触る順ではなく、**不変条件から遠い順**に進める。

1. `backend/` 骨格 + `SessionRef` + `TmuxBackend`（既存コードの移設のみ。挙動不変）→ R1 / R11
2. `TAKO_BACKEND` と `NullBackend` → R0 / R2 / R8（この時点で macOS 上に Windows 経路が生える）
3. `DetachedAccess` の切り出しと `PaneReach` 導入（`dispatch.rs` 40 箇所）→ R9
4. persist 系のゲート言い換え（`available()` → `capabilities()`）→ R3〜R7
5. orchestrator 縮退（PID 列 / report 第 1 層 / delivery 表示）→ R12
6. マトリクス登録 + UI note → R10

1〜2 が終わった時点で macOS のテストが増え、以降の変更は既存テストに守られる。
逆順（先に呼び出し側を触る）だと守りが無い状態で不変条件の上を歩くことになる。

---

## 9. 未決事項（master の判断が要る点）

1. **Windows 初期リリースが「tako を閉じるとエージェントが死ぬ」ことを受容するか**（§2.3）。
   受容しないなら案 B-1（器だけのセッションホスト）を #519 のスコープに入れる必要があり、
   工数は +10〜20 worker-日。本設計は「受容する」前提で書いている
2. **`WorkerEntry.pid` を macOS でも記録するか**（§5.2）。
   記録すれば macOS の突然死検知も強化されるが、`workers.yaml` のスキーマ変更を伴う。
   本設計は「両プラットフォームで記録」を推奨（Windows 専用フィールドを作らない = 原則 2）
3. **report の pane_log 経路を macOS でも選べるようにするか**（§5.3）。
   推奨は「する」（Windows 専用コードを作らないため）。ただし `--source` オプションが増えるのは
   「最も簡単なコマンドを提案する」原則（#322）と緊張する。既定は自動選択、明示指定は上級者向け
4. **`tako_tmux_*` 系ツールの Windows での扱い**（§7.2）。
   `Pending`（将来復活する）と `Unsupported`（概念が無い）のどちらか。
   案 B を温存する立場なら `Pending` が正しいが、B を採らないと決めるなら `Unsupported`
5. **`backend/` を `tako-core` のどこに置くか**。本設計は `crates/tako-core/src/backend/` を
   提案しているが、#515 が `platform/` 配下に support.rs を作る流れと並べたときに
   「なぜ platform ではないのか」が読み手に伝わる配置か（§3.4 の理由付けで足りるか）

---

## 10. 参照した実コード（設計の根拠）

推測ではなく実読に基づくことの明示。行番号は commit `67fe297` 時点。

- `crates/tako-core/src/tmux_backend.rs:28-369`（`socket_name` / `available` / `wrap_options` /
  `pane_tty` / `find_orphans` / `cleanup_orphans` / `kill_server`）
- `crates/tako-core/src/tmux.rs:20-447`（`list_sessions` / `capture_session` / `send_keys` /
  `paste_text` / `pane_log_probe_batch` / `capture_history_plain` / `session_alive`）
- `crates/tako-core/src/scroll_mirror.rs:67,124,157`、`crates/tako-core/src/pane_log.rs:154-249,427-447`
- `crates/tako-app/src/main.rs:444-560`（persist ゲート・強奪ガード・セッション名払い出し）、
  `:1774-1843`（復元分岐）、`:2200-2330`（復元 spawn / orphan 復帰 / cleanup）、
  `:3806-3964`（PromptFlow）、`:4090-4110`（spawn の backend ラップ）、
  `:5019-5105`（save_layout）、`:5158-5205`（pane_log 収集）、`:8827-8900`（ミラー経路判定）、
  `:13478-13489`（reserve_backend_session）
- `crates/tako-control/src/dispatch.rs:612-720`（Send / Read の主経路とフォールバック）、
  `:4474-4489`（`spawn_tmux_delivery`）、`:4824-4913`（report 2 層）、
  `:5495-5560`（worker_status）、`:5785-5882`（permission ダイアログ応答）
- `crates/tako-control/src/claude_tui.rs:429-540`（`deliver_via_tmux`）
- `crates/tako-control/src/orchestrator/registry.rs:80-135,403-460`、
  `crates/tako-control/src/orchestrator/wait.rs:40-45,200-330`
- `crates/tako-control/src/remote.rs:593-600,1146-1170,3018-3040`
- `crates/tako-control/src/agents.rs:130-260,324-`
- `crates/tako-control/src/host.rs:74-135`（`ControlHost` の既存 backend 系メソッド）
- `crates/tako-app/src/ui_text/settings.rs:167-190`（既存の縮退表示文言）
- `crates/tako-app/testdata/mcp_tools_snapshot.txt`（116 ツール。マトリクスのキー体系）

---

## 11. M2 実装記録: 器は自作せず psmux を採る（2026-07-27・#519）

§2.2 は案 B-1（器だけのセッションホスト）を「後から差し込む後継 impl」として温存し、
§9-1 で「Windows 初期リリースが『tako を閉じるとエージェントが死ぬ』ことを受容するか」を
master 判断に残していた。**受容しない**方向で判断が出たので、B-1 を実装した。
ただし**自作（winmux）ではなく既存 OSS の psmux**（MIT / Rust 製 / tmux 互換 CLI）を器にする。

適合検証は別途実施済み（レポート実体はリポジトリ外の作業ディレクトリ）。要旨:

- M0（`poc/conpty-survival/`）が洗い出した 7 罠のうち **6 つを psmux は既に越えている**。
  残り 1 つ（Job object 配下では永続化が無効）は自作でも同じ制約
- tako を強制 kill してもサーバー・シェル・scrollback が**欠落ゼロで生存**
- 能力は §2.2 の B-1 そのもの（`survives_app_exit=true` / `detached_access=false` /
  `scrollback=InProcess`）

**§3.6 の合格条件は満たされた**: `Choice` に実装を 1 つ足すだけで器が生え、
呼び出し側の変更は #177 の強奪ガード 1 箇所のみ（下記 (7) の理由による意図的な変更）。

### 11.1 案 1（TmuxBackend の流用）が不可である理由

psmux は tmux 互換を名乗るが、**コマンドごとに互換の深さが違う**。tako は全経路で
`=`（exact-match 接頭辞）付きターゲットを使うが、`kill-session -t =name` は psmux で
**3/3 決定的に失敗する（各 5.1 秒ブロック）**。流用するとペインを閉じるたびに器と
pwsh がリークし、close が 5 秒固まる。

### 11.2 実装が満たしている受け入れ条件（すべてテストつき）

| # | 条件 | 実装 |
|---|---|---|
| 1 | ターゲットに `=` を付けない | `PsmuxBackend::target`。前方一致で別の器を巻き込まないことも実測 |
| 2 | `show-environment` は全変数から `K=` 行を選ぶ | `select_env_value`（純関数 + 実バイナリ往復） |
| 3 | `#{history_bytes}` に依存しない | `detached()` が `None` = probe 経路を**構造的に持たない** |
| 4 | `pane_tty` は `None` | psmux は実在しない `/dev/pty1` を返す。境界の外へ出さない |
| 5 | conf に `set -g warm off` | 常駐 +243MB → +131MB/session。**psmux が知らない行を書くとペインへ警告が出る**ので、受理を実測した行だけを置く |
| 6 | バージョン固定 + 起動時プローブ | `VERIFIED_VERSION` 以外は `behavior_probe`（作る → 見つける → 壊す）で採否を決め、駄目なら Null へ明示縮退 |
| 7 | 多重起動安全性 | 下記 §11.3 |
| 8 | tmux 誤判別ガード | 下記 §11.4 |

### 11.3 多重起動安全性: 器に尋ねられないので tako 側で記録する

psmux は `list-clients -F` を無視してクライアント PID を返さず、`new-session -D` でも
他クライアントを切り離さない。**§8.1 の I3（#177 の復元強奪ガード）が器側から観測できない。**

さらに Windows では `ports::is_live_tako_app` が常に `false` を返すため、
discovery ベースの多重起動ガード（#113）も**構造的に効かない**。
つまり 2 個目を止められる仕組みが 1 つも無い状態だった。

対策として `backend/owner.rs` を新設し、**OS のファイルロックを生存の証明に使う**
（`<data_dir>/backend-owners/<session>.<pid>.owner` を所有インスタンスが
プロセスの生存中ずっと排他ロックする）。PID の生死判定もプロセス名の照合も要らず、
tako が異常終了してもハンドルは OS が閉じるので記録が居座らない。

`Holder` に `kind` を足し、tmux（クライアント PID = 呼び出し側が祖先辿り）と
psmux（所有インスタンス PID = 生存確認済み）を型で区別する。**呼び出し側の変更は
この 1 箇所だけ**で、tmux 側の判定は 1 ミリも変えていない。

実測（隔離・Windows）: discovery だけ隔離した 2 個目を起動すると
「復元スキップ: 復元対象のバックエンドセッション <名前> を別の tako（pid N）が使用中」で
セカンダリへ降格し、1 個目のペインは attach を保ったまま操作できた。

### 11.4 tmux 誤判別ガード（配布前に必須だった穴）

psmux は `psmux.exe` / `pmux.exe` / `tmux.exe` の 3 本を配り、`-V` の 1 行目で
`tmux 3.3.7` を**詐称**する（素性は 2 行目）。従来の `tmux -V` 判定では
psmux を `Choice::Tmux` と誤認し、「器は作れるが kill が効かない」半端に壊れた
永続化になっていた。`backend::Binary` を新設して `-V` の 2 行目で判別する。

あわせて **Windows では本物の tmux（MSYS2 / Cygwin 版）も器に選ばない**。
ネイティブの ConPTY シェルを抱えられず、`-f` の Windows パスも解釈できないため、
器があるように見えて壊れているより構成のみ永続化へ倒す方が安全。

### 11.5 psmux の導入経路（現時点の方針）

**当面はユーザー導入**（winget / scoop）+ 未導入時は Null へ縮退して案内する。
インストーラー同梱（MIT なので同梱自体は可能。7MB）は今回のスコープ外。
同梱すると「ゼロコンフィグで完全復元」になる代わりに、psmux の更新追従と
配布物の肥大を tako が抱える。判断は配布前（テスター展開時）に行う。

### 11.6 M2 で変わらなかったもの

- **macOS の挙動は不変**（分岐は `backend/` の内側。TmuxBackend の argv スナップショットも不変）
- §4 の縮退定義と §6 の UI 表示方針は**そのまま必要**（psmux 未導入環境は NullBackend のまま）
- §5 の orchestrator 縮退（PID レジストリ / report の pane_log 第 1 層 / delivery 表示）も
  **そのまま残る**。psmux は `detached_access=false` なので役割 B は依然として無い
- pane_log は器の中を記録しない（到達手段が無いため 2 秒ごとの probe を撃たない）。
  §4.1 の #6 に対する Windows での確定挙動
