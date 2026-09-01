//! MCP で公開するツール名・説明・入力スキーマのカタログ。

use serde_json::{json, Value};

/// ペイン ID 引数のスキーマ（省略時は呼び出し元）
fn pane_schema(description: &str) -> Value {
    json!({ "type": "integer", "minimum": 0, "description": description })
}

/// 公開ツールカタログ（FR-2.5 と 1:1。CLI のサブコマンドと同じ操作セット）
pub fn tools() -> Vec<Value> {
    vec![
        json!({
            "name": "tako_list_panes",
            "description": "タブとペインのツリー構造・ジオメトリ（位置・サイズ・分割比率）・\
                状態（タイトル・role・origin・フォーカス・cwd・state・listen_ports・surface）を JSON で返す。\
                shelved_panes（バックグラウンドに退避されたペイン）も含む。\
                state はシェル統合（OSC 133）由来で idle / running / failed（exit_code 付き）\
                / unknown（統合なし）。surface はそのペインが前面表示中か裏で実行中かの分類で\
                foreground（アクティブタブ所属＝画面に出ている）/ background（非アクティブタブ＝裏で実行中）。\
                listen_ports はペイン配下プロセスが listen 中の\
                TCP ポート（dev サーバーの起動検知に使える）。エージェントや dev サーバーの\
                実行状況の把握に使える。\
                ペインを操作する前にまずこれを呼び、現状のレイアウトとペイン ID を把握すること。",
            "inputSchema": { "type": "object", "properties": {}, "additionalProperties": false },
        }),
        json!({
            "name": "tako_split_pane",
            "description": "ペインを分割して新しいターミナルペインを作り、新ペイン ID を返す。\
                command を指定するとシェルの代わりにそのコマンドを実行する\
                （dev サーバーの起動、`git diff` やファイルビューアの表示に使う）。\
                ユーザーに成果物を見せるとき・レビューを求めるときは、このツールで結果を\
                開いて提示すること（見せたいものは口頭で説明せず実際に開く）。\
                対象の指定方法: pane（特定ペインの隣に生やす）または tab（そのタブの\
                フォーカス中ペインの隣に生やす。ユーザーがどのタブを見ていても正確に\
                対象タブ内に分割できる）。どちらも省略すると呼び出し元ペインの隣に生える。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "pane": pane_schema("分割の基準ペイン ID（省略時は呼び出し元ペインの隣に生える。tab と排他）"),
                    "tab": {
                        "type": "integer", "minimum": 0,
                        "description": "分割先タブ ID（そのタブのフォーカス中ペインの隣に生える。pane と排他。\
                            特定タブ内に確実に分割したいときに使う）",
                    },
                    "direction": {
                        "type": "string",
                        "enum": ["right", "down", "left", "up"],
                        "description": "新ペインが生える方向（省略時は right）",
                    },
                    "ratio": {
                        "type": "number",
                        "exclusiveMinimum": 0.0,
                        "exclusiveMaximum": 1.0,
                        "description": "新ペイン側の取り分（省略時は等分）",
                    },
                    "command": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "シェルの代わりに実行するコマンドと引数（例: [\"npm\",\"run\",\"dev\"]）。\
                            終了するとペインも閉じる。省略時は対話シェルが起動する",
                    },
                    "cwd": { "type": "string", "description": "新ペインの作業ディレクトリ" },
                    "focus": {
                        "type": "boolean",
                        "description": "新ペインにフォーカスを移すか（省略時は false = 分割元を維持。\
                            ユーザーの入力中にフォーカスを奪わない）",
                    },
                },
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "tako_send_input",
            "description": "指定ペインの端末へテキストを書き込む（既定で末尾に改行を付けて実行する）。\
                対象の誤指定はそのまま誤実行になるため、必ず tako_list_panes で確認した\
                ペイン ID を渡すこと。tmux_session を指定するとペインが見つからない場合でも \
                tmux session 経由で送信できる。await_prompt を true にすると、claude TUI の\
                プロンプト（❯）が表示されるまで待ってからテキストを送信する。\
                claude 等の全画面 TUI への改行つき送信は送達確認ループで配送される: \
                信頼ダイアログの自動承諾 → bracketed paste 貼り付け → 分離 Enter → \
                入力欄が空になったことの検証 + Enter 単独再送（マルチラインもそのまま送れる。\
                応答は queued: true が即座に返り、実際の送達確認はバックグラウンドで行われる）。\
                text を空にして newline: true にすると Enter 単独送信になる: 入力欄に残った\
                テキストの送信代行に使え、入力欄が空へ戻るまで Enter を自動再送する。\
                #748: **選択肢ダイアログ表示中は送信を拒否してエラーを返す**（入力欄が奪われており、\
                テキストはダイアログのキー操作として食われ、数字なら選択が確定してしまう）。\
                エラー本文に選択肢一覧が入るので、tako_orchestrator_respond で応答すること。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "pane": { "type": "integer", "minimum": 0, "description": "送信先ペイン ID（必須）" },
                    "text": { "type": "string", "description": "送信するテキスト" },
                    "newline": {
                        "type": "boolean",
                        "description": "末尾に改行を付けるか（省略時 true。プロンプトへの部分入力は false）",
                    },
                    "tmux_session": {
                        "type": "string",
                        "description": "tmux session 名（pane ID 解決不能時のフォールバック。tako_orchestrator_spawn の返り値に含まれる）",
                    },
                    "await_prompt": {
                        "type": "boolean",
                        "description": "true にすると claude TUI の ❯ プロンプト表示を待ってから送信する（省略時 false）。\
                            子の Claude Code にメッセージを送るときに使う。送信はバックグラウンドで行われ、\
                            応答は即座に返る（queued: true）",
                    },
                },
                "required": ["pane", "text"],
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "tako_read_pane",
            "description": "指定ペインの画面内容（表示中のテキスト）を返す。\
                別ペインで実行したコマンドの結果確認や、エージェント・dev サーバーの出力監視に使う。\
                tmux_session を指定するとペインが見つからない場合でも tmux session 経由で読める。\
                応答の input_status は Claude Code TUI の入力行（❯）のテキスト属性を示す: \
                style が ghost なら自動提案（ゴーストテキスト）、user なら手動入力、\
                mixed なら混在、none なら入力テキストなし。❯ 行が見つからなければ null。\
                重要: ghost の場合はユーザーの意図した入力ではないため、送信してはならない。\
                queued_messages_pending が true なら、busy 中に人間が打った指示が claude の\
                メッセージキューに未送信で残っている（入力欄自体は空なので Enter を代行しても\
                発火しない）。tako が idle 継続時に自動で送り出すので待つこと。\
                このペインを閉じるとキューごと指示が失われる。\
                choice_dialog が非 null なら**選択肢ダイアログが表示中**で入力欄は存在しない（#748）。\
                このとき input_status は null になる（ダイアログの選択カーソルは入力欄と同じ字面なので、\
                かつては選択肢テキストが style=user の残留入力として報告されていた）。\
                応答は tako_send_input ではなく tako_orchestrator_respond を使う。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "pane": { "type": "integer", "minimum": 0, "description": "対象ペイン ID（必須）" },
                    "lines": { "type": "integer", "minimum": 1, "description": "末尾からの行数制限" },
                    "tmux_session": {
                        "type": "string",
                        "description": "tmux session 名（pane ID 解決不能時のフォールバック。tako_orchestrator_spawn の返り値に含まれる）",
                    },
                },
                "required": ["pane"],
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "tako_scroll_pane",
            "description": "ペインのスクロールバック表示を動かす。\
                to は絶対位置（0 = 最下部、大きいほど過去）、delta は相対行数（正 = 過去方向）。\
                どちらか一方を指定する。応答に現在の offset と history（保持行数）を返す。\
                過去の出力を確認するときは tako_read_pane と組み合わせる。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "pane": { "type": "integer", "minimum": 0, "description": "対象ペイン ID（省略時は呼び出し元）" },
                    "to": { "type": "integer", "minimum": 0, "description": "絶対位置（0 = 最下部）" },
                    "delta": { "type": "integer", "description": "相対行数（正 = 過去方向）" },
                },
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "tako_tmux_list",
            "description": "実行中の全 tmux セッションを一覧する。各セッションの\
                window 一覧・作成日時・attach 状態に加え、attach クライアントが tako の\
                どのタブ・ペインに表示されているか（pane / tab が null なら tako 外の\
                ターミナル由来）を返す。消し忘れて裏で動き続ける tmux の発見に使う。\
                backend = true のセッションは tako 自身のペイン永続化用: kill すると\
                対応ペイン（backend_pane）の中身が消えるため、通常は対象にしないこと。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "socket": { "type": "string", "description": "tmux サーバー名（tmux -L 相当。省略時は既定サーバー）" },
                },
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "tako_tmux_kill",
            "description": "tmux セッション（window 指定時はその window）を kill する。\
                **破壊的操作**: 中で動いているプロセスごと終了する。必ず tako_tmux_list で\
                対象を確認し、ユーザーの同意を得てから実行すること。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "session": { "type": "string", "description": "対象セッション名（必須）" },
                    "window": { "type": "integer", "minimum": 0, "description": "window index（指定時は kill-window、省略時は kill-session）" },
                    "socket": { "type": "string", "description": "tmux サーバー名（tmux -L 相当）" },
                },
                "required": ["session"],
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "tako_tmux_resize",
            "description": "tmux window を指定サイズ（cols × rows）へリサイズする。\
                スマホリモート（Issue #23）のビューポート連動用で、tmux の window-size が \
                manual に切り替わる。PC 側の表示に合わせ直すときは reset=true で解除する。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "session": { "type": "string", "description": "対象セッション名（必須）" },
                    "window": { "type": "integer", "minimum": 0, "default": 0, "description": "window index（省略時は 0）" },
                    "cols": { "type": "integer", "minimum": 1, "description": "幅（桁数）。reset なしなら rows と併せて必須" },
                    "rows": { "type": "integer", "minimum": 1, "description": "高さ（行数）。reset なしなら cols と併せて必須" },
                    "reset": { "type": "boolean", "description": "true で manual サイズを解除しサーバー既定へ戻す" },
                    "socket": { "type": "string", "description": "tmux サーバー名（tmux -L 相当）" },
                },
                "required": ["session"],
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "tako_tmux_open",
            "description": "tmux セッションを現在のタブへ取り込んで表示する。\
                pane を direction（省略時は右）へ分割した新ペインで attach クライアントを\
                起動する。管理外・kill 漏れセッション（tako_tmux_list で発見したもの）の\
                中身をユーザーに見せる・自分で確認するときに使う。\
                新ペインを閉じてもセッション側は終了しない（kill ではない）。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "session": { "type": "string", "description": "対象セッション名（必須。tako_tmux_list の name）" },
                    "socket": { "type": "string", "description": "tmux サーバー名（tmux -L 相当。tako_tmux_list の socket をそのまま渡す）" },
                    "pane": pane_schema("分割の基準ペイン ID（省略時は呼び出し元の隣に生える）"),
                    "direction": {
                        "type": "string",
                        "enum": ["right", "down", "left", "up"],
                        "description": "新ペインが生える方向（省略時は right）",
                    },
                },
                "required": ["session"],
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "tako_tmux_cleanup",
            "description": "取り残された orphan tmux セッションを一括クリーンアップする。\
                tako バックエンドサーバー上の detached・非 grouped・未使用の tako- セッション\
                （前回クラッシュ等で残った裸のバックエンドセッション）だけを kill し、kill した\
                名前を返す。**使用中（attached）・表示中ビュー・ユーザーの実セッションには\
                一切触れない**ため tako_tmux_kill より安全。消し忘れ掃除の定型操作に使う。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "socket": { "type": "string", "description": "tmux サーバー名（tmux -L 相当。省略時は tako バックエンドサーバー）" },
                },
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "tako_tmux_select_window",
            "description": "バックエンドセッション内の tmux window を切り替える。\
                pane のバックエンドセッション内で指定した window index をアクティブにする。\
                tako tmux list でペインの backend セッションの windows を確認してから使う。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "pane": { "type": "integer", "minimum": 0, "description": "対象ペイン ID（省略時は呼び出し元）" },
                    "window": { "type": "integer", "minimum": 0, "description": "切り替え先 window index（必須）" },
                },
                "required": ["window"],
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "tako_video_playback",
            "description": "動画プレビューペインの再生/一時停止/音量/ループを操作する。\
                対象ペインが動画プレビュー（tako open で .mp4/.mov 等を開いた状態）の場合のみ有効。\
                action: status（状態を変えずに現在値だけ取得）/ play / pause / toggle / \
                rate:N（N は 0.1〜4.0 の速度倍率、例: rate:2.0）/ \
                mute / unmute / toggle_mute / loop_on / loop_off / toggle_loop。\
                応答には UI のシークバー・時刻表示と同じ position / duration / state が入る。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "pane": pane_schema("対象ペイン ID（省略時は呼び出し元）"),
                    "action": {
                        "type": "string",
                        "description": "再生操作（status / play / pause / toggle / rate:N / mute / unmute / toggle_mute / loop_on / loop_off / toggle_loop）",
                    },
                },
                "required": ["action"],
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "tako_video_seek",
            "description": "動画プレビューペインのシーク位置を指定する（秒単位の絶対位置）。\
                対象ペインが動画プレビューの場合のみ有効。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "pane": pane_schema("対象ペイン ID（省略時は呼び出し元）"),
                    "seconds": { "type": "number", "minimum": 0, "description": "シーク先の秒数（絶対位置）" },
                },
                "required": ["seconds"],
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "tako_video_volume",
            "description": "動画プレビューペインの音量を設定する（0.0〜1.0）。\
                対象ペインが動画プレビューの場合のみ有効。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "pane": pane_schema("対象ペイン ID（省略時は呼び出し元）"),
                    "volume": { "type": "number", "minimum": 0, "maximum": 1, "description": "音量（0.0〜1.0）" },
                },
                "required": ["volume"],
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "tako_focus_pane",
            "description": "ペインへフォーカスを移す。pane（ID 指定。別タブならタブも切り替わる）か\
                direction（アクティブタブ内の隣接移動）のどちらか一方を指定する。\
                ユーザーに見てほしいペインへ注意を向ける用途にも使う。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "pane": { "type": "integer", "minimum": 0, "description": "フォーカス先ペイン ID" },
                    "direction": {
                        "type": "string",
                        "enum": ["right", "down", "left", "up"],
                        "description": "隣接ペインへの方向移動",
                    },
                },
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "tako_close_pane",
            "description": "ペインを閉じる。pane 省略時は呼び出し元自身（自分のペイン）を閉じる。\
                役目を終えた作業ペインはこのツールで片付けること。\
                タブ最後の 1 ペインならタブごと閉じる（最後のタブの最後のペインは閉じられない）。\
                orchestrator-worker role のペインは busy 時に close が拒否される。\
                強制 close するには force: true を指定する。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "pane": pane_schema("対象ペイン ID（省略時は呼び出し元 = 自己片付け）"),
                    "force": {
                        "type": "boolean",
                        "description": "true にすると busy な worker でも強制的に close する（省略時 false）",
                    },
                },
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "tako_resize_pane",
            "description": "ペインの取り分（サイズ比率）を変える。delta は相対変更（正で拡大）、\
                share は 0–1 の絶対指定で、どちらか一方だけを渡す。\
                ユーザーに見せたいペインを広げる用途にも使う。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "pane": pane_schema("対象ペイン ID（省略時は呼び出し元）"),
                    "axis": {
                        "type": "string",
                        "enum": ["x", "y"],
                        "description": "x = 横幅、y = 縦幅",
                    },
                    "delta": { "type": "number", "description": "取り分の相対変更量（例: 0.1 / -0.1）" },
                    "share": {
                        "type": "number",
                        "exclusiveMinimum": 0.0,
                        "exclusiveMaximum": 1.0,
                        "description": "取り分の絶対指定",
                    },
                },
                "required": ["axis"],
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "tako_equalize_layout",
            "description": "タブ内の全ペインのサイズを均等化する。作業後にレイアウトが乱れたら\
                これで整えること。複数案をペインで並べて見せるときの仕上げにも使う。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "tab": { "type": "integer", "minimum": 0, "description": "対象タブ ID（省略時は呼び出し元ペインのタブ）" },
                },
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "tako_set_title",
            "description": "ペインの表示タイトルと役割ラベル（role。例: worker-1, dev-server）を設定する。\
                ペインを作ったら役割が分かる名前を付け、ユーザーが監視しやすくすること。\
                空文字を渡すとクリアする。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "pane": pane_schema("対象ペイン ID（省略時は呼び出し元）"),
                    "title": { "type": "string", "description": "表示タイトル" },
                    "role": { "type": "string", "description": "役割ラベル" },
                },
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "tako_rename_tab",
            "description": "タブの表示タイトルを変更する。\
                source=\"manual\"（既定）は手動リネームとして自動更新をブロックする。\
                source=\"auto\" はタスク内容に基づく自動命名として、手動リネーム済みタブは上書きしない。\
                手動で付けた名前（tako_set_title / tako_rename_tab）は自動より常に優先される。\
                空文字を渡すと手動指定を解除し、自動リネームが再びタブ名を更新するようになる。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "tab": { "type": "integer", "minimum": 0, "description": "対象タブ ID（省略時は呼び出し元ペインのタブ）" },
                    "title": { "type": "string", "description": "新しいタブタイトル（空文字で手動指定を解除）" },
                    "source": { "type": "string", "enum": ["manual", "auto"], "description": "manual（既定）= 手動リネームとして自動更新をブロック。auto = 作業内容に基づく自動命名（手動リネーム済みタブは上書きしない）" },
                },
                "required": ["title"],
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "tako_pin_tab_title",
            "description": "いまのタブ名をそのまま固定する（以後 自動リネームに上書きされない）。\
                GUI で自動命名の直後にタブへ出る「この名前を固定」の印と同じ操作で、\
                名前を打ち直さずに気に入った名前を残せる。\
                pinned=false で固定を解除すると自動リネームが再開する。\
                pinned 省略時は現在の固定状態の取得のみ。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "tab": { "type": "integer", "minimum": 0, "description": "対象タブ ID（省略時は呼び出し元ペインのタブ）" },
                    "pinned": { "type": "boolean", "description": "true = いまの名前を固定、false = 固定解除（自動リネーム再開）。省略時は状態取得のみ" },
                },
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "tako_create_tab",
            "description": "新しいタブ（= エージェントグループ）を作り、タブ ID と初期ペイン ID を返す。\
                いまのタブと無関係な作業系列を始めるときに使う（1 グループ = 1 タブ）。\
                既定ではアクティブタブは変わらない（ユーザーの入力を奪わない）。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "title": { "type": "string", "description": "タブのタイトル（省略時は連番）" },
                    "focus": { "type": "boolean", "description": "true にすると新タブをアクティブにする（省略時は false = 現在のタブを維持）" },
                    "cwd": { "type": "string", "description": "初期ペインのシェルを起動するフォルダ（省略時は継承）。存在しない・フォルダでないパスはエラー" },
                },
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "tako_select_tab",
            "description": "表示するタブを切り替える。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "tab": { "type": "integer", "minimum": 0, "description": "アクティブにするタブ ID" },
                },
                "required": ["tab"],
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "tako_reorder_tab",
            "description": "タブの並び順を変更する（D&D 並べ替えと同等）。\
                tab を index（0 始まり）の位置へ移動する。範囲外は末尾にクランプされる。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "tab": { "type": "integer", "minimum": 0, "description": "移動するタブ ID" },
                    "index": { "type": "integer", "minimum": 0, "description": "移動先インデックス（0 始まり）" },
                },
                "required": ["tab", "index"],
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "tako_window",
            "description": "複数ウィンドウの操作（ビューポート方式: タブ・ペインの実体は全ウィンドウで\
                共有され、各ウィンドウは表示タブだけを持つ）。action: list = ウィンドウ一覧、\
                new = 新しいウィンドウを開く（tab 指定でそのタブを分離、省略で新規タブ付き）、\
                close = ウィンドウを閉じる（タブは残存ウィンドウへ合流しプロセスは殺さない）、\
                move-tab = タブを別ウィンドウへ移動、focus = ウィンドウをアクティブにして前面化、\
                minimize = 最小化、maximize = 最大化、restore = 最大化を解除して元のサイズへ戻す。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "action": { "type": "string", "enum": ["list", "new", "close", "move-tab", "focus", "minimize", "maximize", "restore"], "description": "省略時は list" },
                    "tab": { "type": "integer", "minimum": 0, "description": "new: 分離するタブ ID（省略で新規タブ）/ move-tab: 移動するタブ ID" },
                    "window": { "type": "integer", "minimum": 0, "description": "close / move-tab / focus の対象ウィンドウ ID。minimize / maximize / restore は省略でアクティブウィンドウ" },
                },
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "tako_menu",
            "description": "アプリメニュー（ファイル / 編集 / 表示 / ウインドウ / ヘルプ）の操作。\
                action: list = メニュー構成と開閉状態を取得（項目のアクション名とショートカットつき）、\
                open = メニューを開く、close = 閉じる、invoke = 項目を実行。\
                open / close は Windows の in-window メニューバーだけで使える（macOS はメニューが\
                OS のメニューバーに載るため tako から開閉できない）。invoke は両 OS で使える。\
                メニューに実在する項目だけが対象。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "action": { "type": "string", "enum": ["list", "open", "close", "invoke"], "description": "省略時は list" },
                    "menu": { "type": "string", "description": "open: メニュー名（完全一致 → 前方一致 → 部分一致で解決。添字も可）" },
                    "path": { "type": "string", "description": "invoke: 「メニュー名/項目名」または項目名のみ（例: ファイル/新規タブ、新規タブ、表示/パネル/git ビュー）" },
                },
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "tako_move_pane_to_tab",
            "description": "ペインを移動する。tab 指定 = 別タブの末尾へ移送（グループ分け）、\
                target 指定 = そのペインの隣（direction 側）へ挿し直す（同タブ内の並べ替え = \
                ペインタイトルバーの D&D と同じ操作。タブまたぎも可）、new_tab = true で新タブとして分離。\
                tab / target / new_tab は排他。既定ではアクティブタブは変わらない（ユーザーの入力を奪わない）。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "tab": { "type": "integer", "minimum": 0, "description": "移送先タブ ID（target / new_tab と排他）" },
                    "target": { "type": "integer", "minimum": 0, "description": "挿入先ペイン ID（このペインの隣に入る）" },
                    "new_tab": { "type": "boolean", "description": "true = 新しいタブとして分離する（tab / target と排他）" },
                    "direction": {
                        "type": "string",
                        "enum": ["right", "down", "left", "up"],
                        "description": "target のどちら側に入るか（省略時は right。target 指定時のみ有効）",
                    },
                    "pane": pane_schema("対象ペイン ID（省略時は呼び出し元）"),
                    "focus": { "type": "boolean", "description": "true にすると移動先タブをアクティブにする（省略時は false = 現在のタブを維持）" },
                },
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "tako_port_detect",
            "description": "listen ポート検知 + 提案チップの ON/OFF を\
                切り替える（enabled 省略時は現在状態の取得のみ）。設定は永続化される。\
                有効時、各ペインの listen 中 TCP ポートは tako_list_panes の listen_ports で読める。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "enabled": { "type": "boolean", "description": "true = 有効化、false = 無効化（省略時は状態取得）" },
                },
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "tako_autosuggest",
            "description": "tako 内の zsh に出す入力予測（履歴ベースのゴーストテキスト）の ON/OFF と、\
                確定キーの案内（ヒント）・Tab 確定を切り替える（3 つとも省略時は現在状態の取得のみ）。\
                いずれも既定 ON。設定は永続化され、稼働中のペインにも次のプロンプトから反映される。\
                予測は右矢印キー、または Tab（ゴースト表示中かつカーソルが行末のときだけ）で確定する。\
                ヒントはゴーストの直後に薄く出るチュートリアルで、既定 10 回で自動的に消える\
                （応答の hint_remaining が残り回数。hint=true で既定回数に戻せる）。\
                同梱している zsh-autosuggestions を tako が起動したシェルにだけ読み込ませる方式なので、\
                tako の外の zsh とユーザーの ~/.zshrc には一切影響しない。\
                ユーザーが自前で zsh-autosuggestions を導入しているペインでは二重注入を避けて何もしない。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "enabled": { "type": "boolean", "description": "true = 予測を出す（既定）、false = 出さない（省略時は状態取得）" },
                    "hint": { "type": "boolean", "description": "確定キーのヒント表示。true = 残り回数を既定へ戻して出す、false = 恒久 OFF" },
                    "tab": { "type": "boolean", "description": "ゴースト表示中の Tab を確定にするか。false にすると Tab は常に従来の補完" },
                },
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "tako_auto_rename",
            "description": "タブ・ペイン名の AI 自動リネームの ON/OFF を切り替える\
                （enabled 省略時は現在状態の取得のみ）。設定は永続化される。\
                手動で付けた名前（tako_set_title / tako_rename_tab）は自動より常に優先される。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "enabled": { "type": "boolean", "description": "true = 有効化、false = 無効化（省略時は状態取得）" },
                },
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "tako_panel",
            "description": "右サイドバー情報パネルの表示・非表示・幅・ビュー切替と、\
                左サイドバーのファイルツリーの表示・非表示を操作する（全省略で現在状態の取得）。\
                view の値は GUI のタブ表示名と同じ。view=fleet はタブごとの全ペイン一覧 + \
                管理外 / kill 漏れ tmux セッションの統合ビュー、view=orch はオーケストレーター俯瞰、\
                view=git は git。ユーザーにセッションやエージェントの状況を見せたいとき表示し、\
                邪魔なら隠す。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "visible": { "type": "boolean", "description": "true = 表示、false = 非表示" },
                    "width": { "type": "number", "exclusiveMinimum": 0, "description": "パネル幅（px）" },
                    "view": { "type": "string", "enum": ["fleet", "orch", "git", "tmux"], "description": "表示するビュー（GUI のタブ名と同じ。fleet = ペイン / セッション俯瞰、orch = オーケストレーター俯瞰、git = git。tmux は fleet の旧称で後方互換のみ）" },
                    "filetree": { "type": "boolean", "description": "左サイドバーのファイルツリーの表示・非表示" },
                    "sidebar_width": { "type": "number", "exclusiveMinimum": 0, "description": "左サイドバーの幅（px。GUI のドラッグと同じ規則で下限 120 / 上限はウィンドウ幅の 50% にクランプされる。応答の sidebar_width が実際に適用された幅、sidebar_width_max がその時点の上限。Issue #307 / #789）" },
                    "show_hidden": { "type": "boolean", "description": "ファイルツリーでドット始まり（.git / .env 等）の項目を表示するか。既定 false = 非表示（Issue #550）" },
                },
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "tako_collapse_tab",
            "description": "サイドバー tmux ビューのタブ枠を折りたたむ / 展開する。\
                折りたたむと、そのタブ配下のバックグラウンド項目（裏で実行中のペイン行 + バックグラウンド）を\
                隠し、前面表示中の行は残す。雑然とした一覧を畳んで注目すべきタブだけ見せたいときに使う。\
                collapsed 省略でトグル、tab 省略で呼び出し元のタブ。現在状態は tako_list_panes の\
                各タブ collapsed でも取得できる。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "tab": { "type": "integer", "description": "対象タブの ID（省略時は呼び出し元ペインのタブ）" },
                    "pane": pane_schema("タブ解決に使う基準ペイン ID（tab 省略時。省略時は呼び出し元）"),
                    "collapsed": { "type": "boolean", "description": "true = 折りたたむ、false = 展開（省略時はトグル）" },
                },
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "tako_pin_preview",
            "description": "サイドバー tmux ビューのバックグラウンドペイン、または閉じたタブグループの\
                実画面サムネイルを、アプリ内のフローティングウィンドウとして常駐させる（ライブ更新し続ける）。\
                裏で動いているペインを画面に出さず見張りたいときに使う。pane = 対象ペイン、\
                group_tab = 閉じたタブグループの由来タブ ID（排他、どちらも省略で呼び出し元ペイン）。\
                pinned=false で解除、省略でトグル。現在のピンは tako_list_panes の pinned で確認できる。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "pane": pane_schema("ピン留めするペイン ID（省略時は呼び出し元）"),
                    "group_tab": { "type": "integer", "description": "閉じたタブグループの由来タブ ID（pane と排他）" },
                    "pinned": { "type": "boolean", "description": "true = ピン留め、false = 解除（省略時はトグル）" },
                },
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "tako_open_file",
            "description": "ファイルをプレビューペインで開いてユーザーに見せる。\
                コードはシンタックスハイライト付き、Markdown は既定でレンダリング表示\
                （mode=code でソース表示へ切替可能 = プレビューの目アイコントグルと同じ操作）。\
                ペインは再利用される: 対象がプレビューペインなら差し替え、同タブに既存の\
                プレビューペインがあればそこへ、無ければ pane を分割して生やす（ターミナルは\
                起動しない）。direction を指定すると再利用せず必ずその方向へ分割して開く\
                （表示位置を制御したいとき）。new_tab を指定すると新しいタブ 1 枚を\
                そのファイル専用にする（Finder の「このアプリケーションで開く」と同じ表示）。\
                「このファイルを見て」「成果物を確認して」の\
                提示に使うこと。相対パスは pane の cwd 基準で解決する。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "pane": pane_schema("基準ペイン ID（省略時は呼び出し元。プレビューの表示先解決に使う）"),
                    "path": { "type": "string", "description": "開くファイルのパス（必須。相対パスは pane の cwd 基準）" },
                    "mode": {
                        "type": "string",
                        "enum": ["code", "markdown"],
                        "description": "表示モード（省略時は拡張子から自動判定。.md / .markdown → markdown）",
                    },
                    "direction": {
                        "type": "string",
                        "enum": ["right", "down", "left", "up"],
                        "description": "指定時は既存プレビューを再利用せず pane をこの方向へ分割して開く",
                    },
                    "focus": { "type": "boolean", "description": "true にするとプレビューペインにフォーカスを移す（省略時は false = 元ペインを維持）" },
                    "new_tab": { "type": "boolean", "description": "true にすると新しいタブを作り、そのタブ 1 枚をこのファイル専用のプレビューにする（タブ名はファイル名。ターミナルは起動しない）。いまのタブを一切動かさず別物として見せたいときに使う。direction とは排他" },
                },
                "required": ["path"],
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "tako_preview_view",
            "description": "PDF・画像プレビューのズーム・ページ・パンを操作する。全操作を省略すると現在状態を返す。\
                zoom は百分率（150 = 150%）、page は 1 始まり。zoom と page を同時指定できるため、\
                『3 ページ目を 150% で見せて』を 1 回で実行できる。zoom_in / zoom_out は 1 段階、\
                reset は幅フィット（100%）+ パン位置リセット。pan_x / pan_y は現在位置へ加える logical px。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "pane": pane_schema("対象 PDF・画像プレビューペイン ID（省略時は呼び出し元）"),
                    "zoom": { "type": "number", "minimum": 25, "maximum": 400, "description": "表示倍率（百分率）" },
                    "zoom_in": { "type": "boolean", "description": "true = 1 段階ズームイン" },
                    "zoom_out": { "type": "boolean", "description": "true = 1 段階ズームアウト" },
                    "reset": { "type": "boolean", "description": "true = 100% + パン位置リセット" },
                    "page": { "type": "integer", "minimum": 1, "description": "PDF の表示ページ（1 始まり）" },
                    "pan_x": { "type": "number", "description": "横パン差分（logical px。正 = 右）" },
                    "pan_y": { "type": "number", "description": "縦パン差分（logical px。正 = 下）" },
                },
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "tako_preview_outline",
            "description": "Markdown 見出しまたは PDF 目次のアウトラインを取得し、項目へジャンプする。\
                item は返却順の 1 始まり。item を省略すると一覧取得だけを行う。Markdown の重複見出しも\
                別項目として保持され、PDF 項目は PDFKit のリンク先ページへ移動する。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "pane": pane_schema("対象 Markdown・PDF プレビューペイン ID（省略時は呼び出し元）"),
                    "item": { "type": "integer", "minimum": 1, "description": "ジャンプするアウトライン項目（表示順の 1 始まり）" },
                },
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "tako_preview_link_list",
            "description": "プレビュー内のリンクを一覧する。Markdown なら [text](url) のリンク\
                （kind=markdown。text / url / openable / line）、PDF なら注釈リンク（kind=pdf。\
                外部 URL・内部ページ参照）。リンクの index は follow-link で使う。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "pane": pane_schema("対象 Markdown・PDF プレビューペイン ID（省略時は呼び出し元）"),
                },
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "tako_preview_follow_link",
            "description": "プレビュー内のリンクをフォローする。URL は OS 既定ブラウザで開き\
                （http / https のみ。それ以外はエラー）、PDF の内部リンクは該当ページへジャンプする。\
                index は link-list の結果で得られる 0 始まりインデックス。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "pane": pane_schema("対象 Markdown・PDF プレビューペイン ID（省略時は呼び出し元）"),
                    "index": { "type": "integer", "minimum": 0, "description": "フォローするリンクのインデックス（0 始まり）" },
                },
                "required": ["index"],
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "tako_preview_copy_code",
            "description": "Markdown プレビューのコードブロック全文（装飾なし・インデントと空行を保持）を\
                クリップボードへ入れる。UI のコピーボタンと同じ経路。index は出現順の 0 始まり（省略時は先頭）。\
                応答にコピーした text も含む。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "pane": pane_schema("対象 Markdown プレビューペイン ID（省略時は呼び出し元）"),
                    "index": { "type": "integer", "minimum": 0, "description": "コードブロックの出現順（0 始まり。省略時は先頭）" },
                },
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "tako_chat_copy",
            "description": "GUI モードのチャットビュー（claude 対話ペイン）の発話をクリップボードへ入れる。\
                UI のコピーボタンと同じ経路。list=true なら発話一覧（添字・role・文字数・コードブロック数）を\
                返すだけでコピーしない。message は表示順の 0 始まりで、省略時は最後の assistant 発話。\
                code を指定するとその発話の中のコードブロック（出現順 0 始まり）だけをコピーする。\
                既定は画面と同じプレーンテキストで、markdown=true のときだけ md ソースをそのまま渡す。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "pane": pane_schema("対象チャット表示ペイン ID（省略時は呼び出し元）"),
                    "list": { "type": "boolean", "description": "true = 発話一覧を返すだけ（コピーしない）" },
                    "message": { "type": "integer", "minimum": 0, "description": "発話の表示順（0 始まり。省略時は最後の assistant 発話）" },
                    "code": { "type": "integer", "minimum": 0, "description": "その発話の中のコードブロック出現順（0 始まり。省略時は本文全体）" },
                    "markdown": { "type": "boolean", "description": "true = md ソースをそのままコピー（既定は画面と同じプレーンテキスト）" },
                },
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "tako_preview_reload",
            "description": "表示中プレビューファイルのライブリロードを設定する。enabled 省略時は現在状態を返す。\
                有効時は外部変更をイベント駆動で検知し、デバウンス後に background で再構築する。\
                編集中の外部変更は表示内容を上書きせず競合として通知する。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "enabled": { "type": "boolean", "description": "true = ライブリロード ON（既定）、false = OFF" },
                },
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "tako_preview_cache",
            "description": "PDF・画像・動画サムネのデコード済み画像キャッシュをバイト予算つき LRU で管理する。\
                max_mb 省略時は現在の上限・使用 bytes・entry 数を返す。変更値は settings.json に永続化する。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "max_mb": {
                        "type": "integer",
                        "minimum": 256,
                        "maximum": 8192,
                        "description": "キャッシュ上限（MiB、既定 512）"
                    },
                },
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "tako_preview_edit",
            "description": "コードプレビューの編集モードを開始・終了する。enabled 省略時は状態取得。\
                PDF・画像・動画・末尾省略された巨大ファイルは編集できない。状態は editing / dirty で返す。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "pane": pane_schema("対象プレビューペイン ID（省略時は呼び出し元）"),
                    "enabled": { "type": "boolean", "description": "true = 編集開始、false = 編集終了（省略時は状態取得）" },
                },
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "tako_preview_apply",
            "description": "コードプレビューの編集バッファ全文を text で置き換える。編集モード未開始なら開始する。\
                ファイルへはまだ書き込まず dirty になるため、続けて tako_preview_save を呼ぶ。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "pane": pane_schema("対象プレビューペイン ID（省略時は呼び出し元）"),
                    "text": { "type": "string", "description": "適用するファイル全文（UTF-8）" },
                },
                "required": ["text"],
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "tako_preview_save",
            "description": "コードプレビューの未保存編集をファイルへ書き戻す。読み込み後に外部変更があれば競合として拒否し、上書きしない。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "pane": pane_schema("対象プレビューペイン ID（省略時は呼び出し元）"),
                },
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "tako_preview_undo",
            "description": "コードプレビュー編集の undo。直前の編集操作を取り消す。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "pane": pane_schema("対象プレビューペイン ID（省略時は呼び出し元）"),
                },
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "tako_preview_redo",
            "description": "コードプレビュー編集の redo。取り消した操作をやり直す。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "pane": pane_schema("対象プレビューペイン ID（省略時は呼び出し元）"),
                },
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "tako_preview_search",
            "description": "コードプレビューのテキスト検索。query でインクリメンタル検索し、direction で移動（next/prev）。\
                編集モードでなくても使える。query 省略時は現在の検索状態を返す。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "pane": pane_schema("対象プレビューペイン ID（省略時は呼び出し元）"),
                    "query": { "type": "string", "description": "検索文字列（大文字小文字区別なし）" },
                    "direction": { "type": "string", "enum": ["next", "prev"], "description": "移動方向（省略時は next）" },
                },
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "tako_preview_replace",
            "description": "コードプレビューのテキスト置換。query に一致する箇所を replacement で置換する。all=true で全置換。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "pane": pane_schema("対象プレビューペイン ID（省略時は呼び出し元）"),
                    "query": { "type": "string", "description": "検索文字列" },
                    "replacement": { "type": "string", "description": "置換文字列" },
                    "all": { "type": "boolean", "description": "true = 全置換、false = 1 件（既定 false）" },
                },
                "required": ["query", "replacement"],
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "tako_preview_autosave",
            "description": "コードプレビュー編集の自動保存設定。enabled 省略時は状態取得のみ。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "pane": pane_schema("対象プレビューペイン ID（省略時は呼び出し元）"),
                    "enabled": { "type": "boolean", "description": "true = 自動保存 ON（既定）、false = 手動保存" },
                },
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "tako_preview_changelog",
            "description": "プレビューペインのチェンジログビュー切替（Issue #338）。\
                enabled=true で git 履歴ベースのファイル変更履歴表示に切り替える。\
                enabled 省略時は状態取得のみ。expand にコミットハッシュを指定すると diff を展開/折りたたみ。\
                git 管理外ファイルでは「履歴なし」を返す。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "pane": pane_schema("対象プレビューペイン ID（省略時は呼び出し元）"),
                    "enabled": { "type": "boolean", "description": "true = チェンジログ表示、false = コードプレビューに戻す" },
                    "max_count": { "type": "integer", "description": "取得するコミット数の上限（省略時は 50）", "minimum": 1 },
                    "expand": { "type": "string", "description": "diff を展開/折りたたみするコミットハッシュ" },
                },
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "tako_file_op",
            "description": "ファイル操作を実行する。op で種別を指定:\n\
                copy_absolute_path = 絶対パスを取得 / copy_relative_path = ペイン cwd 基準の相対パスを取得 /\n\
                reveal = ファイルマネージャ（Finder / エクスプローラー）でファイルの場所を表示 /\n\
                open_terminal = 指定パスのディレクトリへペイン内で cd /\n\
                rename = name でファイル名を変更 / create_file = path 配下に name でファイル作成 /\n\
                create_dir = path 配下に name でフォルダ作成 /\n\
                trash = ゴミ箱（Windows はごみ箱）へ移動。完全削除ではないので復元できる /\n\
                open_default = デフォルトアプリで開く /\n\
                open_with = name で指定したアプリで開く（name 必須）。\n\
                rename / create_file / create_dir / open_with は name パラメータが必須。\
                open_terminal / copy_relative_path は pane パラメータでペインを指定する。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "op": {
                        "type": "string",
                        "enum": ["copy_absolute_path","copy_relative_path","reveal","open_terminal","rename","create_file","create_dir","trash","open_default","open_with"],
                        "description": "操作種別",
                    },
                    "path": { "type": "string", "description": "対象のファイル・フォルダパス（必須）" },
                    "name": { "type": "string", "description": "新しい名前 / アプリ名（rename / create_file / create_dir / open_with で必須）" },
                    "pane": pane_schema("対象ペイン ID（open_terminal の cd 先 / copy_relative_path の基準。省略時は呼び出し元）"),
                },
                "required": ["op", "path"],
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "tako_persist",
            "description": "セッション永続化の ON/OFF を切り替える（enabled 省略時は現在\
                状態の取得のみ）。有効時、タブ / ペイン構成は tmux の有無に関わらず保存・\
                復元される。tmux があれば各ペインは tako 専用 tmux サーバーのセッションと\
                して保持され、実行中プロセスごと復元される。available = false は tmux 不在で\
                構成のみ永続化（復元時は保存 cwd の新シェル）に劣化していることを示す。\
                切替は以後生成されるペインに効く。設定は永続化される。応答の layout_path /\
                layout_exists / last_restore / log_path で保存先と直近の復元結果を診断できる。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "enabled": { "type": "boolean", "description": "true = 有効化、false = 無効化（省略時は状態取得）" },
                },
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "tako_confirm_close",
            "description": "タブ / ペインを閉じる際の確認ダイアログの ON/OFF を\
                切り替える（enabled 省略時は現在状態の取得のみ）。有効時、× クリックと cmd+W で\
                「失われるもの」を要約した確認ダイアログを表示し、⌘クリックでスキップできる。\
                確認が入るのは role 付き（エージェント）ペインと実行中プロセスを持つペインだけで、\
                空のシェルペインは従来どおり即クローズする。CLI / MCP からの close は\
                確認なし（AI フルコントロール）。設定は config.yaml に永続化される。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "enabled": { "type": "boolean", "description": "true = 有効化、false = 無効化（省略時は状態取得）" },
                },
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "tako_limit_resume",
            "description": "利用上限（5h / 週次）後の自動復帰を**ペイン単位**で ON/OFF する\
                （enabled 省略時は現在状態の取得のみ。既定 OFF）。有効にしたペインが\
                上限で止まると、tako がリセット時刻（画面の「reset at …」から解決）+ 数分の\
                安全マージンを過ぎたところで作業を再開させる: 上限対処ダイアログが出ていれば\
                「解除まで待つ」相当の選択肢をラベル一致で確定し（課金・モデル変更を伴う\
                選択肢は構造的に選ばない）、ダイアログが無ければ継続ナッジを送達する。\
                発動するのは上限由来の停止だけで、permission ダイアログ・API エラー・\
                通常の idle・人間の下書きが入力欄にあるときは発動しない。試行は 1 回の\
                上限あたり 3 回までで打ち切る。実行の記録は <data_dir>/supervisor.log の\
                action=limit_autoresume に残る。設定は layout.json に永続化され\
                再起動・復元をまたいで維持される。all=true で全ペインの状態を一覧できる。\
                応答の state は現在の停止状況・復帰予定時刻・試行回数。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "pane": pane_schema("対象ペイン"),
                    "enabled": { "type": "boolean", "description": "true = 有効化、false = 無効化（省略時は状態取得）" },
                    "all": { "type": "boolean", "description": "true = 全ペインの状態を一覧（enabled とは併用しない）" },
                },
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "tako_git_log",
            "description": "git リポジトリのコミット履歴・ブランチ一覧・変更状態を取得する。\
                対象ペインの cwd から git リポジトリを解決する。\
                コミットグラフ描画・ブランチ操作の判断材料として使う。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "pane": pane_schema("対象ペイン"),
                    "max_count": { "type": "integer", "description": "取得するコミット数上限（省略時 200）" },
                },
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "tako_git_diff",
            "description": "git diff を取得する。対象ペインの cwd の\
                リポジトリの diff をファイル・ハンク・行単位で返す。target で種別を指定: \
                \"unstaged\"（ワーキングツリー変更。既定）/ \"staged\"（ステージ済み）/ \
                コミットハッシュ（そのコミットの差分）。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "pane": pane_schema("対象ペイン"),
                    "target": { "type": "string", "description": "diff 種別: unstaged / staged / コミットハッシュ" },
                },
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "tako_git_show",
            "description": "特定コミットの詳細情報を取得する（#495）。フルハッシュ・author・committer・\
                日時・メッセージ全文・親コミット・変更ファイル一覧（パス・変更種別・増減行数）を返す。\
                file を指定するとそのファイルの diff も含まれる。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "pane": pane_schema("対象ペイン"),
                    "hash": { "type": "string", "description": "コミットハッシュ（短縮可）" },
                    "file": { "type": "string", "description": "diff を取得するファイルパス（省略時はファイル一覧のみ）" },
                },
                "required": ["hash"],
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "tako_git_commit",
            "description": "git commit を実行する。対象ペインの cwd のリポジトリでコミットする。\
                コミットメッセージは必須。all=true で tracked ファイルを自動ステージ（git commit -a 相当）。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "pane": pane_schema("対象ペイン"),
                    "message": { "type": "string", "description": "コミットメッセージ" },
                    "all": { "type": "boolean", "description": "tracked ファイルを自動ステージ（-a 相当。省略時 false）" },
                },
                "required": ["message"],
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "tako_git_pull",
            "description": "git pull を実行する。対象ペインの cwd のリポジトリで pull する。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "pane": pane_schema("対象ペイン"),
                },
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "tako_git_push",
            "description": "git push を実行する。対象ペインの cwd のリポジトリで push する。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "pane": pane_schema("対象ペイン"),
                },
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "tako_git_stage",
            "description": "git add でファイルをステージングする。paths が空なら全変更をステージ（git add -A 相当）。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "pane": pane_schema("対象ペイン"),
                    "paths": { "type": "array", "items": { "type": "string" }, "description": "ステージするファイルパス（空で全変更）" },
                },
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "tako_git_unstage",
            "description": "git reset HEAD でファイルをアンステージする。paths が空なら全アンステージ。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "pane": pane_schema("対象ペイン"),
                    "paths": { "type": "array", "items": { "type": "string" }, "description": "アンステージするファイルパス（空で全変更）" },
                },
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "tako_git_checkout",
            "description": "ブランチを切り替える（#496）。confirm=false（既定）で呼ぶと、\
                未コミット変更があるなど破壊的になり得る場合は**実行せず**に何が起きるかを返す\
                （preview.carried_files = 切替後も持ち越される変更 / preview.blocking_files = \
                git が切替を拒否するファル）。内容を確認したうえで confirm=true で実行する。\
                branch に `origin/foo` を指定すると detached HEAD ではなく同名のローカル追跡ブランチを作る。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "pane": pane_schema("対象ペイン"),
                    "branch": { "type": "string", "description": "切替先ブランチ名" },
                    "confirm": { "type": "boolean", "description": "事前提示を承諾して実行する（省略時 false = 提示のみ）" },
                },
                "required": ["branch"],
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "tako_git_branch_create",
            "description": "新規ブランチを作成する（#496）。start_point 省略時は現在の HEAD が基点。\
                checkout=true（既定）で作成後そのまま切り替える。既存名・不正なブランチ名は実行前に拒否する。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "pane": pane_schema("対象ペイン"),
                    "name": { "type": "string", "description": "作成するブランチ名" },
                    "start_point": { "type": "string", "description": "基点のブランチ / コミット（省略時は現在の HEAD）" },
                    "checkout": { "type": "boolean", "description": "作成後に切り替えるか（省略時 true）" },
                },
                "required": ["name"],
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "tako_git_merge",
            "description": "指定ブランチを現在のブランチへマージする（#496）。confirm=false（既定）では\
                **作業ツリーに一切触れず**に予測だけを返す: preview.kind（up-to-date / fast-forward / \
                three-way / unrelated）・incoming_commits・changed_files・predicted_conflicts\
                （git merge-tree による事前計算）。内容を確認して confirm=true で実行する。\
                コンフリクトはエラーではなく conflicted=true として返り、tako_git_resolve_agent へ繋げられる。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "pane": pane_schema("対象ペイン"),
                    "branch": { "type": "string", "description": "マージ元ブランチ名" },
                    "confirm": { "type": "boolean", "description": "事前提示を承諾して実行する（省略時 false = 提示のみ）" },
                    "no_ff": { "type": "boolean", "description": "早送りせずマージコミットを作る（省略時 false）" },
                },
                "required": ["branch"],
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "tako_git_merge_abort",
            "description": "進行中の merge / rebase / cherry-pick / revert を中止して\
                コンフリクト前の状態へ戻す（#496）。進行中の操作が無ければエラーを返す。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "pane": pane_schema("対象ペイン"),
                },
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "tako_git_conflicts",
            "description": "コンフリクト状態を取得する（#496）。進行中の操作\
                （merging / rebasing / cherry-picking / reverting）・未解決ファイル一覧・\
                取り込み先（ours）と取り込み元（theirs）・中止コマンドを返す。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "pane": pane_schema("対象ペイン"),
                },
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "tako_git_resolve_agent",
            "description": "コンフリクト解消エージェントを起動する（#496）。同じタブにペインを立て、\
                エージェント CLI（claude / codex / agy）を起動して解消用プロンプトを自動投入する。\
                プロンプトにはリポジトリパス・未解決ファイル一覧・マージ元/先ブランチと\
                「解消したら報告し、勝手に commit / push しない」制約が含まれる。\
                コンフリクトが発生していないときはエラーを返す。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "pane": pane_schema("対象ペイン（コンフリクトしているリポジトリの cwd を持つペイン）"),
                    "agent": { "type": "string", "enum": ["claude", "codex", "agy"], "description": "エージェント種別（省略時はプロファイル既定）" },
                    "tab": { "type": "integer", "description": "分割先タブ ID（省略時は呼び出し元ペインのタブ）" },
                },
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "tako_background_pane",
            "description": "ペインまたはタブをバックグラウンドへ送る。プロセスは生きたまま\
                画面から外す。邪魔なペインやタブを画面外へ送るのに使う。バックグラウンドのペインは\
                tako_background_list で確認でき、tako_foreground_pane で画面に戻せる。\
                tab 指定時はタブ内全ペインを一括退避する（pane と tab は排他）。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "pane": { "type": "integer", "description": "バックグラウンドへ送るペインの ID（省略時は呼び出し元。tab と排他）" },
                    "tab": { "type": "integer", "description": "バックグラウンドへ送るタブの ID（タブ内全ペインを一括退避。pane と排他）" },
                },
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "tako_foreground_pane",
            "description": "バックグラウンドのペインを画面に復帰させる。target ペインの\
                direction 側を分割して表示する。target 省略時は由来タブへ戻す\
                （由来タブが閉じていればアクティブタブ）。バックグラウンドで動かしていたペインを取り出すのに使う。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "pane": { "type": "integer", "description": "復帰させるペインの ID（background list から取得）" },
                    "target": { "type": "integer", "description": "挿入先ペインの ID（省略時はフォーカス中ペイン）" },
                    "direction": { "type": "string", "enum": ["right","down","left","up"], "description": "分割方向（省略時は right）" },
                },
                "required": ["pane"],
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "tako_background_list",
            "description": "バックグラウンドのペイン一覧を取得する。各ペインの\
                ID / title / role / state / cwd に加え、由来タブ（origin_tab / origin_tab_title）と\
                surface（常に background = 裏で実行中）を返す。バックグラウンドペインはこの由来タブで\
                グループ分けして表示され、tako_foreground_pane で由来タブへ戻せる。",
            "inputSchema": {
                "type": "object",
                "properties": {},
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "tako_background_kill",
            "description": "バックグラウンドのペインを kill する。プロセスとバックエンド\
                セッションも終了する。復帰不要なペインの片付けに使う。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "pane": { "type": "integer", "description": "kill するペインの ID" },
                },
                "required": ["pane"],
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "tako_check_health",
            "description": "tako 環境の健全性を診断する。接続直後に呼んで環境に問題がないか確認すること。\
                チェック項目: tako CLI が PATH に通っているか / CLI とアプリのバージョンが一致するか / \
                外部ツール（tmux 等）の有無 / セッション永続化の状態。\
                問題がある場合は issue 配列に修正方法の提案を含めて返す。",
            "inputSchema": {
                "type": "object",
                "properties": {},
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "tako_setup_mcp",
            "description": "エージェント CLI に tako MCP サーバーの接続設定を\
                自動追加する。初回セットアップ時に呼ぶ。既に設定済みなら何もしない。\
                agent を省略すると claude + この環境に導入済みの codex / agy へまとめて\
                登録する（未導入は理由つきで skip）。agent を明示したときだけ、\
                その CLI が未導入・非対応スコープなら分類済みエラーで止まる。\
                書き込み先は claude = ~/.claude.json、codex = codex mcp add\
                （~/.codex/config.toml。tako と通信する env の転送設定 env_vars も一緒に書く。\
                値ではなく変数名だけなのでトークンは残らない）、\
                agy = agy mcp add（~/.gemini/config/mcp_config.json。env はそのまま継承される）。\
                scope=global（既定）はユーザーグローバル、scope=project は\
                呼び出し元ペインの cwd の .mcp.json に書き込む（claude のみ対応）。\
                旧バージョンが ~/.claude/settings.json に書いた無効な設定は自動で掃除する。\
                応答の agents 配列が各エージェントの結果（skipped / error_kind つき）。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "scope": {
                        "type": "string",
                        "enum": ["global", "project"],
                        "description": "設定の書き込み先スコープ（省略時は global = ユーザーグローバル）",
                    },
                    "agent": {
                        "type": "string",
                        "enum": ["claude", "codex", "agy"],
                        "description": "登録対象のエージェント（省略時は claude + 導入済みの codex / agy すべて）",
                    },
                    "pane": pane_schema("対象ペイン ID（scope=project 時の cwd 解決に使う。省略時は呼び出し元）"),
                },
                "additionalProperties": false,
            },
        }),
        // --- オーケストレーター MCP ツール ---
        json!({
            "name": "tako_orchestrator_projects",
            "description": "オーケストレーターのプロジェクトを管理する。\
                action=list で登録済みプロジェクト一覧、add で新規追加、remove で削除。\
                プロジェクトは projects.yaml に保存され、tako_orchestrator_spawn の\
                対象として使える。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "action": {
                        "type": "string",
                        "enum": ["list", "add", "remove"],
                        "description": "操作種別（省略時は list）",
                    },
                    "key": { "type": "string", "description": "プロジェクトキー（add / remove 時に必須）" },
                    "cwd": { "type": "string", "description": "作業ディレクトリ（add 時に必須。~ は $HOME に展開される）" },
                    "description": { "type": "string", "description": "プロジェクトの説明（add 時に任意）" },
                },
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "tako_orchestrator_profiles",
            "description": "オーケストレーターのプロファイル（tako master / tako solo の起動設定）を管理する。\
                action=list で一覧、show で単一表示、set で作成・更新、create で新規作成、\
                copy で複製（from に複製元）、delete で削除（default は削除不可）。\
                kind=master（既定。tako master が読む profiles/）と kind=solo（tako solo が読む \
                solo-profiles/）を切り替える。スキーマは両者共通。\
                list / show / set は参照整合性の警告（未登録 project / 未登録アカウント / \
                [1m] モデル）を warnings フィールドで返す。\
                プロファイルは profiles/<name>.yaml に保存され、master のエージェント種別・\
                モデル・effort と子 worker のモデル決定に使われる。model が null / 未指定の\
                プロファイルはその CLI の既定モデルで起動する（プラン非依存・推奨）。\
                1M コンテキスト版（[1m] サフィックス）は Max / API プラン限定のため、\
                set で明示指定した場合のみ使われる（Pro プランでは起動不能になる点に注意）。\
                master のエージェント種別は master_agent（claude / codex。agy は master 非対応）で\
                指定し、model / effort はその CLI のネイティブ表記で書く\
                （codex 例: model=gpt-5.6-sol / effort=xhigh）。master_agent が claude 以外のとき\
                master の model / effort は claude worker へ継承されない。\
                worker のエージェント種別（claude / codex / agy）は worker_agent（既定種別）と\
                agent_* 系（worker_agents.<agent> のエージェント別 worker 設定: モデル・effort・\
                許可スキップ・追加引数）で指定する。\
                自動ハンドオフ（#749）は ctx_threshold（50〜60%）と auto_handoff で調整する。\
                応答の resolved_ctx_threshold / ctx_threshold_source / resolved_auto_handoff が\
                実効値（プロファイル → config.yaml → 既定 60 の解決結果）。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "action": {
                        "type": "string",
                        "enum": ["list", "show", "set", "create", "copy", "delete"],
                        "description": "操作種別（省略時は list）",
                    },
                    "name": { "type": "string", "description": "プロファイル名（set / create / copy / delete 時に必須。show 省略時は default）" },
                    "kind": {
                        "type": "string",
                        "enum": ["master", "solo"],
                        "description": "プロファイル種別（master = tako master の profiles/ 既定 / solo = tako solo の solo-profiles/）",
                    },
                    "from": { "type": "string", "description": "複製元プロファイル名（copy 時に必須）" },
                    "projects": {
                        "type": "array", "items": { "type": "string" },
                        "description": "このプロファイルに割り当てるプロジェクトキー（projects.yaml のキー。丸ごと置き換え。空配列でクリア。set 時）",
                    },
                    "clear_projects": { "type": "boolean", "description": "projects の割り当てを解除する（set 時）" },
                    "master_agent": {
                        "type": "string",
                        "enum": ["claude", "codex"],
                        "description": "master のエージェント種別（set 時。tako master / solo がこの CLI で起動する。agy は master 非対応）",
                    },
                    "clear_master_agent": { "type": "boolean", "description": "master_agent の指定を解除して claude 既定に戻す（set 時）" },
                    "model": { "type": "string", "description": "master のモデル（master_agent のネイティブ表記。set 時。省略で現状維持）" },
                    "clear_model": { "type": "boolean", "description": "master のモデル指定を解除して claude 既定に戻す（set 時）" },
                    "worker_model": { "type": "string", "description": "worker_model_policy=fixed 時の子 worker モデル（set 時）" },
                    "clear_worker_model": { "type": "boolean", "description": "子 worker のモデル指定を解除する（set 時）" },
                    "effort": { "type": "string", "description": "master の thinking effort（set 時。省略で現状維持）" },
                    "worker_effort": { "type": "string", "description": "子 worker の thinking effort（set 時）" },
                    "worker_agent": {
                        "type": "string",
                        "enum": ["claude", "codex", "agy"],
                        "description": "worker の既定エージェント種別（set 時。省略時の spawn はこの種別で起動する）",
                    },
                    "clear_worker_agent": { "type": "boolean", "description": "worker_agent の指定を解除して claude 既定に戻す（set 時）" },
                    "agent": {
                        "type": "string",
                        "enum": ["claude", "codex", "agy"],
                        "description": "agent_* 系で編集する対象エージェント名（set 時。agent_* 指定に必須）",
                    },
                    "agent_model": { "type": "string", "description": "対象エージェントの worker 既定モデル（CLI ネイティブ表記。codex: gpt-5.6-terra 等 / agy: 'Gemini 3.5 Flash (High)' 等）" },
                    "clear_agent_model": { "type": "boolean", "description": "対象エージェントのモデル指定を解除する" },
                    "agent_effort": { "type": "string", "description": "対象エージェントの worker 既定 effort（claude: --effort / codex: model_reasoning_effort。agy は無視される）" },
                    "clear_agent_effort": { "type": "boolean", "description": "対象エージェントの effort 指定を解除する" },
                    "agent_skip_permissions": { "type": "boolean", "description": "対象エージェントの許可プロンプトをスキップして起動する（明示 opt-in。agy は既定でコマンド毎に許可が出るため自律 worker 運用ではほぼ必須）" },
                    "agent_args": {
                        "type": "array", "items": { "type": "string" },
                        "description": "対象エージェントの追加 CLI 引数（丸ごと置き換え。空配列でクリア）",
                    },
                    "worker_model_policy": { "type": "string", "enum": ["inherit", "delegate", "fixed"], "description": "worker のモデル選択ポリシー（inherit: master と同じ / delegate: master が都度選ぶ / fixed: worker_model 固定）" },
                    "tab_naming_convention": { "type": "string", "description": "タブ名の命名規則（master プロンプトに注入される自由記述。空文字でクリア。set 時）" },
                    "env_set": {
                        "type": "array", "items": { "type": "string" },
                        "description": "環境変数を設定する（KEY=VALUE 形式の配列。値の ~ は $HOME に展開される。set 時。Issue #500）",
                    },
                    "env_unset": {
                        "type": "array", "items": { "type": "string" },
                        "description": "環境変数を削除する（キー名の配列。set 時。Issue #500）",
                    },
                    "master_account": { "type": "string", "description": "master の既定アカウント名（accounts.yaml のキー。空文字でクリア。set 時。#504）" },
                    "clear_master_account": { "type": "boolean", "description": "master_account を解除する（set 時。#504）" },
                    "worker_account": { "type": "string", "description": "worker の既定アカウント名（空文字でクリア。set 時。#504）" },
                    "clear_worker_account": { "type": "boolean", "description": "worker_account を解除する（set 時。#504）" },
                    "ctx_threshold": { "type": "integer", "minimum": 50, "maximum": 60, "description": "master が引き継ぎを始める ctx 使用率の閾値（%。50〜60。範囲外はエラー。未設定なら config.yaml → 既定 60。set 時。#749）" },
                    "clear_ctx_threshold": { "type": "boolean", "description": "ctx_threshold の指定を解除する（config.yaml → 既定 60 へ戻る。set 時。#749）" },
                    "auto_handoff": { "type": "boolean", "description": "閾値超過時に tako が master へ引き継ぎを促す自動通知（既定 true）。false にすると通知は止まるが tako_orchestrator_self / tako_orchestrator_handoff は従来どおり使える（set 時。#749）" },
                    "clear_auto_handoff": { "type": "boolean", "description": "auto_handoff の指定を解除して既定（有効）へ戻す（set 時。#749）" },
                    "limit_resume": { "type": "boolean", "description": "このプロファイルから spawn した worker ペインで利用上限後の自動復帰（5h / 週次上限のリセット後に tako が再開させる）を既定 ON にする（既定 false。set 時。#822）。spawn 側の limit_resume が指定されていればそちらが勝つ" },
                    "clear_limit_resume": { "type": "boolean", "description": "limit_resume の指定を解除して既定（無効）へ戻す（set 時。#822）" },
                    "bypass_sandbox": { "type": "boolean", "description": "codex（master / worker）を --dangerously-bypass-approvals-and-sandbox で起動することを許可する（既定 false = 許可しない。set 時。#981）。true にすると承認プロンプトと codex のサンドボックスが両方無効になり、書き込み先もネットワークも制限されなくなる。false へ戻すと codex の既定（承認プロンプトが出る）に戻る" },
                },
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "tako_orchestrator_accounts",
            "description": "アカウントレジストリの管理（Issue #504）。名前つきアカウント（accounts.yaml）の CRUD。\
                アカウントは config_dir（CLAUDE_CONFIG_DIR の値）または inherit（未設定のまま = 既定の資格情報）と\
                既定モデル/effort を持ち、\
                spawn の account パラメータやプロファイルの master_account/worker_account で使う。\
                action: list（全アカウント一覧）/ show（name 必須。1 件の詳細）/ \
                add（name + config_dir か inherit のどちらか必須。追加または更新）/ remove（name 必須。削除）。\
                既定の claude アカウント（~/.claude）を登録するときは config_dir ではなく inherit=true を使う: \
                CLAUDE_CONFIG_DIR は設定されているだけで Keychain のエントリ名が変わり、\
                既定パスを明示しても既存ログインが未ログイン扱いになる（Issue #512）",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "action": { "type": "string", "enum": ["list", "show", "add", "remove"], "description": "操作（list / show / add / remove）" },
                    "name": { "type": "string", "description": "アカウント名（show / add / remove 時に必須）" },
                    "config_dir": { "type": "string", "description": "CLAUDE_CONFIG_DIR の値（add 時。~ は $HOME に展開される。inherit と排他）" },
                    "inherit": { "type": "boolean", "description": "true = CLAUDE_CONFIG_DIR を設定しない（既定の資格情報をそのまま使う。spawn 時は明示 unset で direnv 等の値も消す。#512）" },
                    "description": { "type": "string", "description": "アカウントの説明（add 時。任意）" },
                    "default_model": { "type": "string", "description": "このアカウントの既定モデル（add 時。任意。spawn で model 未指定時のフォールバック）" },
                    "default_effort": { "type": "string", "description": "このアカウントの既定 effort（add 時。任意）" },
                },
                "required": ["action"],
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "tako_orchestrator_layout",
            "description": "worker spawn のレイアウト設定を取得・変更する（config.yaml の spawn_layout）。\
                全パラメータ省略で現在値の取得、いずれか指定でその項目を更新して結果を返す。\
                policy=master-reserved（既定）は spawn 元（master）の取り分を維持し、\
                worker を右側の worker 領域内に配置する。legacy は従来の右等分割\
                （worker が増えるほど全ペインが横に圧縮される）。\
                master_ratio は master 側へ残す取り分（0.1〜0.9。既定 0.5 = 画面半分）。\
                algorithm は worker 領域内の配置: grid（1 体=全面 → 2 体=上下 → 3〜4 体=十字四分割）/ \
                spiral（縦横交互に半分ずつの渦巻き分割）。\
                worker close 時は領域内だけがリフローされ、master とユーザーが自分で開いた\
                ペインの矩形は変わらない。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "policy": {
                        "type": "string",
                        "enum": ["master-reserved", "legacy"],
                        "description": "配置ポリシー（省略で現状維持）",
                    },
                    "master_ratio": {
                        "type": "number",
                        "description": "master 側へ残す取り分 0.1〜0.9（省略で現状維持）",
                    },
                    "algorithm": {
                        "type": "string",
                        "enum": ["grid", "spiral"],
                        "description": "worker 領域内の配置アルゴリズム（省略で現状維持）",
                    },
                },
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "tako_orchestrator_spawn",
            "description": "プロジェクトの作業ディレクトリで子 worker を spawn する。\
                worker のエージェント CLI は claude（既定）/ codex / agy から選べる（agent パラメータ）。\
                呼び出し元ペインを右に分割して新ペインを作り、エージェントを起動してプロンプトを送信する。\
                worker の pane_id・tmux_session・spawned_by（spawn 元ペイン ID）・agent・\
                worker_id（レジストリ ID。#390: ペイン消失後の watch / status / report に使える）を返す。\
                tmux_session は pane ID が解決できない場合\
                （BG タブ移動・tako 再起動後）のフォールバックとして tako_read_pane / tako_send_input に渡せる。\
                worker_status / watch は pane_id だけで session を自動解決するため session_id は不要\
                （codex / agy は画面推定で判定される）。\
                起動からプロンプト送信まで 15〜20 秒かかる（これは想定内）。\
                pane または tab のいずれかを必ず指定すること。省略すると呼び出し元タブに出るため、\
                master が別タブにいる場合に意図しないタブに子が生える。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "project": { "type": "string", "description": "プロジェクトキー（projects.yaml に登録済みであること）" },
                    "prompt": { "type": "string", "description": "worker に渡す初期プロンプト" },
                    "label": { "type": "string", "description": "ペインタイトルに付けるラベル（省略時は '<project>-worker'）" },
                    "agent": {
                        "type": "string",
                        "enum": ["claude", "codex", "agy"],
                        "description": "worker のエージェント CLI（省略時はプロファイルの worker_agent → claude）",
                    },
                    "model": { "type": "string", "description": "worker のモデル（agent のネイティブ表記。省略時はプロファイル設定に従う）" },
                    "effort": { "type": "string", "description": "thinking / reasoning effort（claude・codex のみ。agy はモデル名に組込みのため無視。省略時はプロファイル設定に従う）" },
                    "pane": pane_schema("分割元ペイン ID（省略時は呼び出し元。このペインの右に子が生える）。\
                        pane と tab の両方を指定した場合は pane を優先する"),
                    "tab": { "type": "integer", "minimum": 0, "description": "子を出すタブ ID。\
                        指定するとそのタブのフォーカスペインを分割元にする。\
                        複数マスター運用時は tab で出力先タブを明示指定することを推奨" },
                    "task_type": {
                        "type": "string",
                        "enum": ["bugfix-rooted", "bugfix-unrooted", "investigation", "feature-verifiable", "feature-ui", "docs", "review"],
                        "description": "委任台帳の task_type（省略時は investigation）。\
                            spawn 時に自動記録され、ledger stats で task_type x model の成功率・差し戻し率を集計できる",
                    },
                    "account": { "type": "string", "description": "アカウント名（accounts.yaml のキー。この worker だけ該当 config dir / モデルで起動する。#504）" },
                    "limit_resume": {
                        "type": "boolean",
                        "description": "この worker だけ利用上限後の自動復帰（5h / 週次上限のリセット後に tako が再開させる）を明示指定する（#822）。\
                            省略時はプロファイルの limit_resume → 無効。長時間の自律タスクを任せるときに true にする。\
                            適用結果は応答の limit_resume に返る（ペイン単位の切替は tako_limit_resume）",
                    },
                },
                "required": ["project", "prompt"],
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "tako_orchestrator_worker_status",
            "description": "子 worker の状態を確認する。status は busy（作業中）/ idle（入力待ち・完了）/ \
                error（API エラー・usage limit 等の異常で停止。#157）/ \
                gone（ペイン消滅かつ tmux session も消滅）/ unknown（agents 不可）。\
                error 時は応答の error オブジェクトに kind（api_error = 続行指示で復帰可 / \
                usage_limit = 解除時刻まで待つ / limit_dialog = モデル切替等のダイアログに応答）と \
                detail（検知した画面上の行）、recommended_action（resume / wait_reset / respond_dialog）が入る。\
                events 配列に直近の検知イベントが入る（#243）: \
                question = worker が質問中（idle 時のみ。画面末尾に ? 終端行・選択肢・Should I 等のパターン）/ \
                model_switched = 自動モデル切替が発生（from/to つき。limit reached, now using ... の検知）/ \
                context_high = ctx 使用率が 60% 超（percent つき。handoff やセーフティコミットの判断材料）/ \
                queued_messages_pending = 人間が busy 中に打った指示が claude のキューに未送信で残っている（#572。\
                入力欄は空なので Enter 代行では発火しない。tako が自動で送り出すまで待ち、このペインを閉じない）。\
                session_id を省略しても pane→session の自動解決（pid 祖先辿り）で claude agents --json の \
                正確な status を取得する（status_source が agents-auto になる）。自動解決失敗時のみ \
                画面パターン推定にフォールバック（status_source が screen）。\
                codex / agy worker は agents API が無いため常に画面推定で判定される（claude / codex / agy \
                すべての入力欄・busy パターンに対応済み）。\
                tmux_session を渡すとペインが消えても tmux session が生きている限り \
                recent_output を取得でき、gone にならない。\
                退避（shelved）されたペインも追跡可能。recent_output はペインの最近 30 行の出力。\
                resolved_session_id に自動解決された session_id が入る。\
                #390: spawn 済み worker はレジストリに登録済みのため、pane_id 指定でも \
                tmux_session / session_id が自動補完され、ペイン消失後も追跡が切れない。\
                worker（レジストリ ID）指定なら pane_id 省略可。\
                応答の prompt_delivery（delivered / pending / undelivered / unverified）と \
                events の prompt_undelivered イベントで spawn プロンプトの未達を検知できる \
                （undelivered なら tako_send_input でプロンプトを再送する）。\
                #530: 送達フローがプロンプトの到達を確認できなかった場合は、claude が起動して \
                session が観測できていても undelivered になる（起動 ≠ プロンプト到達）。\
                #983: unverified は「猶予を過ぎたが、この系統には送達を裏づける一次シグナルが無い」\
                （agy 等）という意味で、未達とは断定していない。events には prompt_undelivered ではなく \
                prompt_delivery_unverified（recommended_action=verify_then_resend）が載るので、\
                画面を見て届いているか確かめてから再送すること（そのまま再送すると二重指示になる）。\
                #983: エージェント CLI の起動そのものが失敗している（CLI 不在で command not found / \
                未認証）ときは status が error になり、error.kind=launch_failed / \
                error.launch_problem（cli_not_found / not_authenticated / …）/ \
                error.detail に「理由 + 次の一手」が入る（recommended_action=fix_launch）。\
                続行指示や再送では直らないので、detail の手順を先に実施する。\
                events の agent_dead はエージェント CLI プロセスの突然死（SIGSEGV 等）の疑い: \
                応答の resume_command（レジストリの session ID から組み立てた claude --resume）を \
                ペインのシェルへ tako_send_input すれば文脈ごと復旧できる（自動 resume はしない）。\
                #748: 選択肢ダイアログ（permission だけでなく usage limit の対処選択・モデル選択・\
                plan 確認・AskUserQuestion・一覧選択も）が画面にあるときは status が waiting になり、\
                応答の choice_dialog に構造（kind / title / options[number,label,highlighted] / numbered / \
                recommended_action）が入る。events には choice_dialog（dialog_kind つき）が積まれ、\
                このとき question は出さない（ダイアログ待ちは本文への返信では解けない）。\
                kind が trust / bypass のものは tako 自身が承諾するので触らないこと（auto_accepted: true）。\
                応答は tako_orchestrator_respond（choice 省略で下見できる）。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "pane_id": { "type": "integer", "minimum": 0, "description": "worker のペイン ID（worker と排他。どちらか必須）" },
                    "worker": { "type": "string", "description": "worker レジストリの ID（#390。tako_orchestrator_spawn の返り値 worker_id / tako_orchestrator_workers で確認）" },
                    "session_id": { "type": "string", "description": "claude の session ID（あれば精度向上）" },
                    "tmux_session": {
                        "type": "string",
                        "description": "tmux session 名（pane 消滅時のフォールバック追跡。tako_orchestrator_spawn の返り値に含まれる）",
                    },
                },
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "tako_orchestrator_workers",
            "description": "worker レジストリの一覧（#390）。spawn 済み worker をペインの生死と無関係に列挙する。\
                tako 再起動でペインが消えても、レジストリに残る worker は tmux_session / session_id 経由で \
                watch / status / report を継続できる。各エントリに worker_id / pane / tmux_session / \
                session_id / pane_alive（GUI にペインが現存するか）/ tmux_alive（tmux session が生存中か）/ \
                prompt_delivery（delivered = プロンプト到達済み / pending = 確認中 / undelivered = 未達の疑い）/ \
                prompt_delivery_failure（未達の理由コード。#530: choice_dialog = 初回のテーマ選択・\
                ログイン方法選択ダイアログが出て送れなかった / paste_not_reflected / residual_after_retries / \
                flow_timeout）/ resend_command（未達 worker にだけ入る再送コマンド。同じ依頼文を \
                tako_send_input で送り直す）/ resume_command（session ID 検出済み claude worker の復旧コマンド。\
                突然死時に使う）が入る。既定は active のみ。all = true で closed（明示 close 済み）も含める。\
                列挙のついでに、ペインも tmux session も 5 分以上続けて観測できない active エントリを \
                closed（close_reason = gone）へ倒す（#658。resume_command / report は closed でも引けるので \
                突然死からの復旧材料は失われない）。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "all": { "type": "boolean", "description": "closed の worker も含める（既定 false）" },
                },
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "tako_orchestrator_self",
            "description": "master / solo が自分自身の pane・tab・ctx%・session_id を取得する。\
                master は MCP 経由では自分のペイン ID を知る手段がなかったが（#123）、\
                このツールで自己特定できる。ctx_percent はコンテキスト使用率（0〜100）、\
                ctx_threshold は引き継ぎ閾値（プロファイルの ctx_threshold → config.yaml → \
                既定 60 の順で解決。値域 50〜60。出どころは ctx_threshold_source）、\
                ctx_over_threshold は閾値超えフラグ。\
                ctx_over_threshold が true になったら、ユーザーの許可を待たずに \
                handoff_path のファイルを最新化して tako_orchestrator_handoff を呼ぶ（#749）。\
                auto_handoff は tako 側の自動通知が有効かどうか（有効なら閾値超過で \
                「【tako 自動通知】」で始まる指示が届く。届いたら即座に引き継ぎを始める）。\
                handoff_path / handoff_exists は**プロファイル運用メモ**（handoff/<profile>.md。\
                プロジェクトに紐付かない運用知識の置き場）。\
                #915: プロジェクト固有の引き継ぎは project_handoffs の各 path\
                （handoff/projects/<project-key>.md）に書く。そこへ書いたものは、そのプロジェクトを\
                管轄する master の後任にだけ渡る。旧形式は読むついでに自動移行される\
                （handoff_migration。冪等）。profile_source が pane_role なら呼び出し元の\
                TAKO_ORCHESTRATOR_ROLE が失われている（ペインの role ラベルから復元した。#854）。\
                handoff_format はその書式（#792。sectioned = 知識 / 実行状態の 2 節に分かれている、\
                legacy = 節分離前、null = ファイル未作成）、handoff_sections は認識できた節。\
                legacy なら次に更新するとき 2 節へ書き直す。\
                pane を省略すると caller の環境変数（TAKO_PANE_ID / TAKO_ORCHESTRATOR_ROLE）\
                から自動解決する。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "pane": pane_schema("自 pane ID（省略時は caller から自動解決）"),
                },
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "tako_orchestrator_handoff",
            "description": "master の引き継ぎを実行する。**管轄プロジェクトの引き継ぎだけ**を読み、\
                同プロファイルの新 master を spawn して引き継ぎプロンプトを注入する。\
                role / プロファイル / アカウント / モデル / effort / タブは旧 master と同一を引き継ぐ。\
                呼ぶ前に引き継ぎファイルを今の状況で最新化すること（このツールはファイルの\
                内容をそのまま後任へ渡すだけで、中身の鮮度は確認しない）。\
                #915: 置き場はプロジェクト単位（handoff/projects/<project-key>.md）。\
                管轄は projects 引数 → プロファイルの担当プロジェクト + 稼働中 worker の\
                プロジェクト → 稼働中 worker だけ の順で解決し（応答の jurisdiction_source）、\
                どれも決まらなければ**本文を貼らずに一覧とパスだけ**を後任へ渡す\
                （無関係なプロジェクトの長文で後任の文脈を食わない）。\
                プロジェクトに紐付かない運用知識は handoff/<profile>.md（プロファイル運用メモ）\
                に置き、こちらは常に渡る。旧形式（プロファイル単位の混在ファイル）は\
                この呼び出しの中で自動移行される（応答の handoff_migration。冪等・原本は退避）。\
                #749: 旧 master のペインは**後任が引き継ぎを確認したあとに後任自身が閉じる**\
                （初期プロンプトにその手順が入る: 実態突き合わせ → 旧ペインの入力欄に\
                ユーザーの未送達指示が残っていないか確認 → close）。この呼び出しでは閉じないので、\
                後任の起動が失敗しても旧 master は失われない。応答の previous_master_pane_id が\
                退役予定のペイン（null なら後任に close を指示していない）。\
                引き継ぎの材料が 1 つも無ければエラーを返す（master は事前に書く必要がある）。\
                #792: 各ファイルは 2 節に分けて書く。\
                「## 知識（マシン非依存）」= 決定事項・方針・残タスクの意図（pane / tab 番号を書かない）、\
                「## 実行状態（このマシン限定）」= worker とその pane / tab・実行中のもの。\
                pane / tab はこのマシンでしか意味を持たないので、知識に混ぜると別デバイスで\
                誤った指示の元になる。節分離前の旧書式もそのまま読める（応答の handoff_format が\
                sectioned / legacy / mixed）。\
                tab を省略すると呼び出し元と同タブに新 master を spawn する。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "pane": pane_schema("呼び出し元ペイン ID（省略時は caller から自動解決）"),
                    "tab": { "type": "integer", "minimum": 0, "description": "新 master を出すタブ ID（省略時は呼び出し元と同タブ）" },
                    "projects": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "後任へ渡すプロジェクトキー（#915）。指定すると推定より優先される。省略時はプロファイルの担当 + 稼働中 worker から推定",
                    },
                },
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "tako_orchestrator_handoffs",
            "description": "引き継ぎファイルの管理（Issue #915）。プロジェクト単位の引き継ぎ\
                （handoff/projects/<project-key>.md）とプロファイル運用メモ（handoff/<profile>.md）の\
                一覧・読み・書き、および旧形式からの移行。\
                action: list（両方の一覧。行数・書式・肥大警告つき）/ \
                show（project か profile のどちらか一方。内容と書式。未作成なら雛形を返す）/ \
                write（project か profile のどちらか一方 + content。アトミック + 世代バックアップ）/ \
                migrate（旧形式の自動移行。profile 省略で全プロファイル）。\
                移行は通常 setup 実行時と master が引き継ぎを読む経路で**自動**で走るので、\
                migrate を手で呼ぶ必要はない（冪等なので呼んでも壊れない）。\
                プロジェクト固有の引き継ぎをここへ書けば、そのプロジェクトを管轄する master の\
                後任にだけ渡る。運用メモは常に渡るので、プロジェクトに紐付かない知識だけを置く",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "action": { "type": "string", "enum": ["list", "show", "write", "migrate"], "description": "操作（list / show / write / migrate）" },
                    "project": { "type": "string", "description": "プロジェクトキー（projects.yaml のキー。show / write でどちらか一方）" },
                    "profile": { "type": "string", "description": "プロファイル名（運用メモ側。show / write でどちらか一方、migrate では対象の絞り込み）" },
                    "content": { "type": "string", "description": "書き込む内容（write 時に必須）" },
                },
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "tako_orchestrator_run",
            "description": "子 worker を spawn し、即座に run_id を返す（非同期。#121）。\
                MCP 呼び出しが中断されても worker は孤児化せず、run_id で追跡できる。\
                進捗確認は tako_orchestrator_run_status、結果回収は tako_orchestrator_run_result を使う。\
                worker のエージェント CLI は claude（既定）/ codex / agy から選べる（agent パラメータ）。\
                完了判定はバックグラウンドで OrchestratorWorkerStatus と同じロジックを繰り返す。\
                タイムアウト（既定 1800 秒）に達した場合は run_status が status=timeout を返す。\
                worker が API エラー等で停止した場合は status=worker_error + error オブジェクト。\
                sync=true を指定すると旧挙動（完了までブロッキング）に戻る（後方互換）。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "project": { "type": "string", "description": "プロジェクトキー（projects.yaml に登録済み）" },
                    "prompt": { "type": "string", "description": "worker に渡すプロンプト" },
                    "label": { "type": "string", "description": "ペインタイトルのラベル（省略時は '<project>-worker'）" },
                    "agent": {
                        "type": "string",
                        "enum": ["claude", "codex", "agy"],
                        "description": "worker のエージェント CLI（省略時はプロファイルの worker_agent → claude）",
                    },
                    "model": { "type": "string", "description": "worker のモデル（agent のネイティブ表記。省略時はマスターのプロファイル設定に従う）" },
                    "effort": { "type": "string", "description": "thinking / reasoning effort（claude・codex のみ。省略時はマスターのプロファイル設定に従う）" },
                    "pane": pane_schema("分割元ペイン ID（省略時は呼び出し元）"),
                    "tab": { "type": "integer", "minimum": 0, "description": "子を出すタブ ID" },
                    "timeout_seconds": {
                        "type": "integer", "minimum": 10, "default": 1800,
                        "description": "完了待ちタイムアウト秒数（省略時 1800 = 30 分）",
                    },
                    "auto_close": {
                        "type": "boolean", "default": true,
                        "description": "完了後にペインを自動 close するか（省略時 true）",
                    },
                    "output_lines": {
                        "type": "integer", "minimum": 1, "default": 200,
                        "description": "返す出力の末尾行数（省略時 200）",
                    },
                    "sync": {
                        "type": "boolean", "default": false,
                        "description": "true にすると完了までブロッキングする旧挙動（後方互換。既定 false = 非同期）",
                    },
                    "task_type": {
                        "type": "string",
                        "enum": ["bugfix-rooted", "bugfix-unrooted", "investigation", "feature-verifiable", "feature-ui", "docs", "review"],
                        "description": "委任台帳の task_type（省略時は investigation）",
                    },
                    "account": { "type": "string", "description": "アカウント名（accounts.yaml のキー。この worker だけ該当 config dir / モデルで起動する。#504）" },
                },
                "required": ["project", "prompt"],
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "tako_orchestrator_ledger",
            "description": "委任台帳を操作する（Issue #292）。\
                action=list で一覧（project / task_type でフィルタ、limit で件数制限）、\
                stats で task_type x model の集計（成功率・差し戻し率・平均所要時間・未評価数）、\
                record で検収結果の記録（id + outcome + rounds + note）、\
                amend で事後修正（検収 pass だが実使用で問題発覚。id + note）、\
                prune で project 前方一致によるエントリ除去（project 必須。selftest 混入等の掃除用）。\
                spawn / run 時に task_type を指定すると自動記録され、stats で判断材料になる。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "action": {
                        "type": "string",
                        "enum": ["list", "stats", "record", "amend", "prune"],
                        "description": "操作種別",
                    },
                    "id": { "type": "string", "description": "対象エントリ ID（record / amend 時に必須。spawn 応答の ledger_id）" },
                    "outcome": {
                        "type": "string",
                        "enum": ["pass", "rework", "fail"],
                        "description": "検収結果（record 時に必須）",
                    },
                    "rounds": { "type": "integer", "minimum": 1, "description": "差し戻し回数（record 時に任意）" },
                    "note": { "type": "string", "description": "メモ（record / amend 時に任意）" },
                    "project": { "type": "string", "description": "フィルタ用プロジェクト（list 時に任意）" },
                    "task_type": {
                        "type": "string",
                        "enum": ["bugfix-rooted", "bugfix-unrooted", "investigation", "feature-verifiable", "feature-ui", "docs", "review"],
                        "description": "フィルタ用 task_type（list 時に任意）",
                    },
                    "limit": { "type": "integer", "minimum": 1, "description": "返す件数の上限（list 時に任意。既定 50）" },
                },
                "required": ["action"],
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "tako_orchestrator_run_status",
            "description": "非同期 run の進捗を照会する（#121）。run_id を指定すると \
                {run_id, pane_id, status, phase, elapsed_seconds} を返す。\
                phase は 'running'（進行中）または 'finished'（完了済み）。\
                status は busy / idle / error / gone / starting / completed / worker_error / timeout。\
                run_id を省略すると全 run の一覧を返す。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "run_id": { "type": "string", "description": "照会する run_id（省略時は全 run 一覧）" },
                },
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "tako_orchestrator_run_result",
            "description": "完了した非同期 run の結果を回収する（#121）。\
                未完了なら phase='running' を返す（エラーにはならない）。\
                完了済みなら出力取得 + auto_close を行い、レジストリから除去して \
                {run_id, pane_id, status, output, duration_seconds, closed} を返す。\
                run ごとに 1 回だけ呼べる（2 回目は run_id が見つからないエラー）。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "run_id": { "type": "string", "description": "回収する run_id" },
                },
                "required": ["run_id"],
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "tako_orchestrator_respond",
            "description": "worker の**選択肢ダイアログ**に応答する（#319 permission → #748 で全種別）。\
                対象は permission（ツール承認）のほか usage limit の対処選択・モデル選択（/model）・\
                plan モードの実行確認・AskUserQuestion の質問・一覧選択（/mcp）など。\
                watch の WORKER_PERMISSION / WORKER_DIALOG、または worker_status / read_pane の \
                choice_dialog で検知されたダイアログに対して使う。\
                **choice を省略すると送信せず構造だけ返す**（下見。選択肢一覧・現在のハイライト・番号キーの可否）。\
                ダイアログが画面に存在しない場合はエラー（誤爆防止）。\
                番号つきダイアログは番号キーだけで確定し、番号なしダイアログは矢印移動 + ラベル一致検証 + Enter で応答する。\
                応答内容は persist.log に監査記録される。\
                危険なコマンド（rm -rf / 本番 DB 操作等）への承認、および課金・モデル変更を伴う選択肢はユーザーに確認すること。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "pane_id": { "type": "integer", "description": "対象の worker ペイン ID" },
                    "choice": {
                        "type": "string",
                        "description": "選択肢: 番号（画面の番号 or 1 始まりの順番）／ラベルの部分一致（大小無視・複数一致はエラー）／'yes'/'allow'／'no'/'deny'。省略すると送信せず構造だけ返す",
                    },
                },
                "required": ["pane_id"],
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "tako_orchestrator_supervisor",
            "description": "worker 自動復旧 supervisor の操作（#401）。\
                usage_limit / api_error / agent_dead / prompt_undelivered に対する自動リカバリの設定・状態照会・履歴参照。\
                action=status: 現在の設定（mode / auto_resume_dead / max_retries）と監査ログ末尾。\
                action=set_mode: supervisor モードを変更する（auto = 自動復旧 / notify_only = 通知のみ / off = 無効）。\
                action=history: 監査ログの末尾を取得する（復旧アクションの全記録）。\
                WORKER_DEAD の自動 resume は既定 notify-only（auto_resume_dead=false）。\
                opt-in するには set_mode で auto_resume_dead=true を設定する。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "action": {
                        "type": "string",
                        "enum": ["status", "set_mode", "history"],
                        "description": "status: 現在の設定と監査ログ / set_mode: モード変更 / history: 監査ログ"
                    },
                    "mode": {
                        "type": "string",
                        "enum": ["auto", "notify_only", "off"],
                        "description": "set_mode 時のモード"
                    },
                    "auto_resume_dead": {
                        "type": "boolean",
                        "description": "set_mode 時: WORKER_DEAD の自動 resume を有効にする（既定 false）"
                    },
                    "max_retries": {
                        "type": "integer", "minimum": 1, "maximum": 20,
                        "description": "set_mode 時: 同一 worker の最大リトライ回数（既定 3。超過でエスカレーション）"
                    },
                    "lines": {
                        "type": "integer", "minimum": 1, "maximum": 200,
                        "description": "status / history: 監査ログの返却行数（既定 20）"
                    },
                },
                "required": ["action"],
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "tako_orchestrator_report",
            "description": "worker の報告内容を取得する（#364）。\
                第 1 層: tmux scrollback（capture-pane -J で折返し結合。全 agent 共通）。\
                第 2 層: 構造化ソース（claude の transcript JSONL。ペイン幅非依存の全文品質）。\
                transcript が利用可能なら source=transcript で全文テキストを返し、scrollback_text に \
                スクロールバック版も併記する。利用不可（codex / agy 等）なら source=scrollback。\
                tako_read_pane（可視画面のみ）と異なり、スクロールバック履歴を遡るため長い出力も取得できる。\
                報告の読み取りには report を使い、read_pane は配置・生存確認用に限定すること。\
                messages で直近 n 件の assistant テキストを取得できる（古い順で返す。省略時 1 件）。\
                #390: worker（レジストリ ID）指定なら pane_id 省略可。ペイン消失後も \
                レジストリの tmux_session / session_id 経由で報告を取得できる。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "pane_id": { "type": "integer", "minimum": 0, "description": "worker のペイン ID（worker と排他。どちらか必須）" },
                    "worker": { "type": "string", "description": "worker レジストリの ID（#390。ペイン消失後の取得にも使える）" },
                    "lines": {
                        "type": "integer", "minimum": 1, "maximum": 100000,
                        "description": "スクロールバック取得行数（既定 2000）",
                    },
                    "messages": {
                        "type": "integer", "minimum": 1, "maximum": 1000,
                        "description": "transcript から取得する直近 assistant メッセージ件数（既定 1。古い順で返す。総数超過時は全件）",
                    },
                },
                "additionalProperties": false,
            },
        }),
        // --- リモートアクセス MCP ツール ---
        json!({
            "name": "tako_remote_start",
            "description": "リモートアクセス API サーバーを起動する。スマホからブラウザ経由で\
                ペインを操作するための HTTP API サーバーが Unix domain socket で開始される。\
                transport は Tailscale Serve のみ: daemon は UDS（0600）のみで listen し、\
                tailnet 内限定の恒久固定 URL（https://<ホスト名>.<tailnet>.ts.net）で公開される\
                （WireGuard E2E 暗号化・TCP ポートは一切開かない）。\
                Tailscale が未セットアップ（未導入・未ログイン・HTTPS 未有効等）の場合は\
                不足項目を列挙して起動を拒否するので、ユーザーに `tako remote setup` を案内する。\
                接続には機器ペアリングが必要: 初回アクセス時に Mac 画面へ承認ダイアログが表示され、\
                ユーザーが許可した端末だけが role（observe / interact / manage / admin）に応じて\
                操作できる。承認・role 変更は Mac の GUI 限定で AI からは行えない。\
                注意: interact 以上を許可した端末はターミナルへ任意コマンドを送信できる（実質シェルアクセス）。",
            "inputSchema": {
                "type": "object",
                "properties": {},
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "tako_remote_stop",
            "description": "リモートアクセス API サーバーを停止する。\
                既定は SIGTERM で停止を試みる。force=true で SIGKILL を使う。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "force": {
                        "type": "boolean",
                        "description": "true で SIGKILL を使う（既定 false = SIGTERM）",
                    },
                },
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "tako_remote_status",
            "description": "リモートアクセス API サーバーの状態を取得する。\
                起動中なら running=true・socket パス・恒久固定 URL・登録済み端末数を返す。\
                URL に secret は含まれない（接続時の認証は機器ペアリングが行う）。",
            "inputSchema": {
                "type": "object",
                "properties": {},
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "tako_remote_agents",
            "description": "動作中の Claude Code エージェント一覧を取得する\
                （claude agents --json のプロキシ）。各エージェントの session_id / status / \
                ctx_percent / model / name / cwd に加え、tmux バックエンドのどのペインで\
                動いているか（pane）をプロセス祖先の突き合わせで対応付けて返す。\
                スマホリモートのエージェント監視やセッション ID の特定に使う。",
            "inputSchema": {
                "type": "object",
                "properties": {},
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "tako_remote_messages",
            "description": "Claude Code セッションの会話ログ（transcript）の末尾を\
                正規化 JSON で取得する。user / assistant メッセージ・ツール使用サマリ・\
                thinking（折りたたみ用に分離）を返す。session_id は tako_remote_agents で\
                確認できる。エージェントの進捗確認や会話の振り返りに使う。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "session_id": { "type": "string", "description": "対象セッション ID（必須。claude の sessionId）" },
                    "tail": { "type": "integer", "minimum": 1, "default": 30, "description": "取得する末尾件数（省略時は 30）" },
                },
                "required": ["session_id"],
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "tako_remote_scrollback",
            "description": "ペインのスクロールバック履歴をプレーンテキストで取得する。\
                tmux capture-pane で指定行数の履歴を取得し、ANSI なしのテキストとして返す。\
                リモートからの画面履歴確認やログ検索に使う。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "pane_id": { "type": "string", "description": "対象ペイン ID（必須。session:window.pane）" },
                    "lines": { "type": "integer", "minimum": 1, "default": 1000, "description": "取得する履歴行数（省略時は 1000）" },
                },
                "required": ["pane_id"],
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "tako_remote_devices",
            "description": "リモート接続のペアリング済み端末を管理する（#283 機器ペアリング認証）。\
                action=list で登録済み端末（id・名前・role・最終アクセス）と保留中の\
                ペアリング要求を一覧、action=revoke で device_id の登録を失効させる\
                （接続中の端末は即時切断される）。\
                ペアリングの承認・role 変更はこのツールでは行えない: Mac 画面に表示される\
                承認ダイアログでユーザー本人だけが操作できる（セキュリティ境界のため AI には\
                承認 API を提供しない）。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "action": {
                        "type": "string", "enum": ["list", "revoke"],
                        "description": "list = 端末一覧 / revoke = 登録失効",
                    },
                    "device_id": {
                        "type": "string",
                        "description": "revoke の対象デバイス ID（list で確認できる）",
                    },
                },
                "required": ["action"],
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "tako_remote_setup",
            "description": "リモートアクセスの Tailscale セットアップ状態を確認・実行する（#286）。\
                action=check で Tailscale の導入・ログイン・HTTPS・serve の各項目を確認、\
                action=run で Tailscale 系統の決定 + serve 設定 + QR PNG 生成まで実行する\
                （既定のループバック TCP では serve は `tako remote start` 時に張るので \
                serve_config は deferred になる）。\
                Tailscale が未導入・未ログインの場合は手順を案内して停止する。\
                対話的なウィザード（brew install の実行等）は CLI `tako remote setup` で行い、\
                このツールでは非対話実行のみ。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "action": {
                        "type": "string", "enum": ["check", "run"],
                        "description": "check = 状態確認のみ / run = セットアップ実行",
                    },
                    "answers": {
                        "type": "object",
                        "description": "run 時のオプション。yes=true で全質問を自動承認。\
                            tailscale で使う Tailscale 系統を選ぶ（#1038: macOS では \
                            GUI 版アプリと standalone tailscaled が同居し、\
                            別ノードとして二重登録されることがある。省略時は検出結果から決め、\
                            選んだ理由を tailscale_reason に返す）",
                        "properties": {
                            "yes": { "type": "boolean" },
                            "tailscale": {
                                "type": "string",
                                "enum": ["auto", "gui", "standalone"],
                            },
                        },
                    },
                },
                "required": ["action"],
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "tako_web",
            "description": "ネイティブ Web ビューペインの操作（FR-3.8）。macOS の WKWebView を \
                ペインとして表示し、ユーザーはクリック・スクロール・文字入力を直接行える。\
                dev サーバーのプレビュー表示・ドキュメント提示・成果物の URL 提示に使う。\
                ペインから外しても（hide）ページは dock に生きたまま維持され、show で呼び戻せる。\
                action: open = url を新規ペインで開く / list = 一覧（id・URL・タイトル・表示中ペイン）/ \
                show = dock から id をペインへ呼び出す / hide = ペインから外して dock へ退避 / \
                close = 完全破棄 / navigate = to（back・forward・reload・URL）でページ遷移 / \
                eval = js を非同期評価して token を返す / eval_result = token の結果回収 \
                （eval 発行後 200ms 程度おいて呼ぶ。pending: true なら再試行）/ \
                read = URL・タイトル・読み込み状態の取得。\
                ページ内の操作（クリック・入力・スクロール・テキスト取得）は eval の JS で行う \
                （例: document.querySelector('button').click()）。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "action": {
                        "type": "string",
                        "enum": ["open", "list", "show", "hide", "close", "navigate", "eval", "eval_result", "read"],
                        "description": "実行する操作（必須）",
                    },
                    "url": { "type": "string", "description": "open: 開く URL（必須）" },
                    "id": { "type": "integer", "description": "対象 Web ビュー ID（list で確認。show では必須）" },
                    "pane": pane_schema("open / show: 分割の基準ペイン ID（省略時は呼び出し元）。その他: 対象 Web ビューが表示中のペイン ID"),
                    "direction": {
                        "type": "string",
                        "enum": ["right", "down", "left", "up"],
                        "description": "open / show: 分割方向（省略時は右）",
                    },
                    "to": { "type": "string", "description": "navigate: back / forward / reload / URL（必須）" },
                    "js": { "type": "string", "description": "eval: 実行する JavaScript（必須）" },
                    "token": { "type": "integer", "description": "eval_result: eval が返した token（必須）" },
                    "focus": { "type": "boolean", "description": "open / show: true にすると新ペインにフォーカスを移す（省略時は false = 元ペインを維持）" },
                },
                "required": ["action"],
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "tako_update",
            "description": "アプリ内更新の診断・チェック・実行（#36 + #50 + #403 チャンネル制）。\
                action=status で配布系統（homebrew / zip / broken-brew）・現在バージョン・\
                現在チャンネル（stable / test）・PATH 上の重複 CLI を返す。\
                action=check で GitHub Releases から最新版の有無を確認する（更新は行わない）。\
                channel で stable / test を指定可。省略で全チャンネル同時チェック。\
                action=apply で配布系統に応じた更新を実行する。\
                channel で stable（既定）/ test を指定。\
                action=apply-zip で zip 経由で強制更新する。\
                action=repair で broken-brew 状態を修復する。\
                apply 成功後の再起動は UI 側で行う（CLI / MCP からは apply 結果の確認まで）。\
                action=open で GUI のアップデート専用画面を開く（#616。\
                現在 / 最新バージョン・チャンネル・配布物・リリースノート・更新ボタンが載る。\
                開いているかは action=status の window_open で分かる）。\
                action=card で画面上部の更新通知カードの状態を返す。\
                action=card-dismiss でカードを閉じる（そのバージョンは以後通知しない）。\
                action=card-show で抑止を解除して出し直す。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "action": {
                        "type": "string",
                        "enum": [
                            "status", "check", "apply", "apply-zip", "repair",
                            "open", "card", "card-dismiss", "card-show",
                        ],
                        "description": "操作種別（省略時は status）",
                    },
                    "channel": {
                        "type": "string",
                        "enum": ["stable", "test"],
                        "description": "対象チャンネル。check 時は省略で全チャンネル、apply 時は省略で stable",
                    },
                },
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "tako_fda",
            "description": "macOS のフルディスクアクセス (FDA) の状態確認と設定画面の起動（Issue #118）。\
                フォルダアクセス許可ダイアログが頻発する場合、FDA を付与すれば一括で消せる。\
                action=status で FDA の付与状態を返す（granted: true/false）。\
                action=open でシステム設定のフルディスクアクセスパネルを開く。\
                ユーザーが「フォルダの許可が何度も出る」と言った場合は、\
                まず status で確認し、未付与なら open で設定画面を案内すること。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "action": {
                        "type": "string",
                        "enum": ["status", "open"],
                        "description": "操作種別（省略時は status）",
                    },
                },
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "tako_sleep_guard",
            "description": "スリープ防止機能の状態確認・設定変更（Issue #173 + #218 蓋閉じ対応）。\
                macOS のアイドルスリープを IOKit 電源アサーションで防止する。\
                蓋閉じ防止は pmset disablesleep で制御（sudoers 登録が必要）。\
                action=status（既定）: モード・電源条件・アサーション状態・蓋の開閉・thermal 状態を返す。\
                action=set: mode / power_condition / lid_sleep_mode を設定する。\
                action=install-lid-sleep: sudoers.d に pmset NOPASSWD を登録（管理者パスワード必要、初回のみ）。\
                action=remove-lid-sleep: sudoers.d から削除 + disablesleep 解除。\
                action=open-battery-settings: System Settings の Battery を開く（フォールバック）。\
                ユーザーが「PC がスリープして作業が止まった」「蓋を閉じても続けたい」と言った場合は、\
                まず status で確認し、蓋閉じ防止なら install-lid-sleep で登録を案内すること。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "action": {
                        "type": "string",
                        "enum": ["status", "set", "install-lid-sleep", "remove-lid-sleep", "open-battery-settings"],
                        "description": "操作種別（省略時は status）",
                    },
                    "mode": {
                        "type": "string",
                        "enum": ["off", "on", "while-agents-running"],
                        "description": "アイドルスリープ防止モード（set 時のみ有効）",
                    },
                    "power_condition": {
                        "type": "string",
                        "enum": ["ac-only", "always"],
                        "description": "電源条件（set 時のみ有効）",
                    },
                    "lid_sleep_mode": {
                        "type": "string",
                        "enum": ["off", "while-agents-running"],
                        "description": "蓋閉じ防止モード（set 時のみ有効。要 sudoers 登録）",
                    },
                },
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "tako_theme",
            "description": "UI テーマの状態確認・切替・色設定・プリセット・フォント（Issue #217/#459）。\
                action=status（既定）: 現在のテーマ + 利用可能プリセットを返す。\
                action=set: mode でテーマ（dark/light/プリセット名）を切り替える。\
                action=toggle: ダーク/ライトを反転する。\
                action=colors: 58色キーの現在値とソースを一覧する。\
                action=set-color: key の色を value(#RRGGBB) へ変更する。\
                action=reset-color: key の色上書きを削除しビルトインへ戻す。\
                action=reset-colors: 全色上書きを削除する。\
                action=save-preset: 現在の色を name で保存する。\
                action=delete-preset: プリセットを削除する。\
                action=set-font: フォントファミリーやサイズを変更する。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "action": {
                        "type": "string",
                        "enum": ["status", "set", "toggle", "colors", "set-color", "reset-color", "reset-colors", "save-preset", "delete-preset", "set-font"],
                        "description": "操作種別（省略時は status）",
                    },
                    "mode": { "type": "string", "description": "テーマ: dark / light / プリセット名（set 時に必須）" },
                    "target": { "type": "string", "description": "色操作の対象（省略 = 現在の theme）" },
                    "key": { "type": "string", "description": "色キー名（set-color / reset-color 時に必須）" },
                    "value": { "type": "string", "description": "#RRGGBB（set-color 時に必須）" },
                    "name": { "type": "string", "description": "プリセット名（save-preset / delete-preset 時に必須）" },
                    "font_family": { "type": "string", "description": "フォントファミリー（set-font 時）" },
                    "font_size": { "type": "string", "description": "フォントサイズ（set-font 時。8〜32）" },
                },
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "tako_settings",
            "description": "設定画面の操作（Issue #459）。\
                action=open（既定）: GUI 設定画面を開く（個別設定は tako_lang / tako_theme 等を使う）。\
                action=status: 設定画面が開いているかを返す。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "action": {
                        "type": "string",
                        "enum": ["open", "status"],
                        "description": "操作種別（省略時は open）",
                    },
                    "tab": {
                        "type": "string",
                        "enum": ["general", "appearance", "runner", "profiles", "setup", "sleep", "remote", "advanced"],
                        "description": "開くタブ指定（省略時は現在タブ維持）。profiles = master / solo の起動プロファイル編集（#721）",
                    },
                },
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "tako_stale_binary",
            "description": "稼働中 claude セッションのバイナリ鮮度を検知し、新版への張り直しを行う（Issue #498）。\
                claude CLI は symlink 張り替えで更新されるが、長生きセッション（特に master）は起動時の旧バイナリを\
                握り続ける。action=status（既定）で指定ペインの stale 判定（握っている版 / 最新版 / stale か）を返す。\
                action=restart で張り直し（worker は claude --resume で会話復元、master は handoff で引き継ぎ）。\
                busy（実行中）のペインでは restart は拒否される。action=dismiss でバナーを閉じる。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "action": {
                        "type": "string",
                        "enum": ["status", "restart", "dismiss"],
                        "description": "操作種別（省略時は status）",
                    },
                    "pane": {
                        "type": "integer",
                        "description": "対象ペイン ID（省略時はデフォルト解決）",
                    },
                },
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "tako_migrate",
            "description": "設定・データファイルのスキーマ自動マイグレーション（Issue #916）。\
                tako は設定ファイルの形式や置き場を変えたとき**利用者へ手動移行を要求しない**。\
                旧形式は setup 実行時と実行時の差分検出で自動的に直る。このツールは\
                その状態を確認・手動発火するためのもの。\
                action=status（既定）で全永続ファイル（settings.json / layout.json / \
                projects.yaml / profiles / accounts / sessions / workers / ledger 等）の\
                形式の版数と、これから当たる移行を見るだけで返す（何も書き換えない）。\
                action=run で実際に当てる（旧内容は .pre-v<N>.bak へ退避され消えない。\
                冪等なので何度実行しても壊れない）。\
                応答の files[].state は absent / up_to_date / migrated / unreadable / \
                refused / failed で、files[].steps に当てた（当てる）移行の説明が入る。\
                action=status のときは書き換えていないので backup_planned / \
                quarantine_planned というキー名になる（「退避済み」と読み違えないため）。**unreadable は「設定が壊れているので既定値で動いている」\
                という意味**で、退避先（quarantine）に元の内容が残っているので\
                ユーザーへ知らせること。schema でファイル種別を 1 つに絞れる。\
                設定が壊れて GUI が起動しないときは CLI の `tako migrate` が同じ処理を\
                GUI 無しで実行できる。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "action": {
                        "type": "string",
                        "enum": ["status", "run"],
                        "description": "操作種別（省略時は status = 見るだけ）",
                    },
                    "schema": {
                        "type": "string",
                        "description": "対象のファイル種別（省略時は全種別）",
                    },
                },
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "tako_welcome",
            "description": "初回起動のウェルカムバナー（Issue #549）。tako を初めて起動したユーザーへ\
                「tako setup で初期設定 → tako master で AI 司令塔」の導線を画面上部に出す。\
                action=status（既定）で表示状態（visible / dismissed / first_launch）と\
                案内すべきコマンド（setup_command / master_command）を返す。\
                action=show でバナーを再表示（初期設定がまだのユーザーへ導線を出し直したいとき）。\
                action=dismiss で閉じて以後出さない（settings.json に永続化）。\
                ユーザーが「何から始めればいいか」で迷っていたら status で状態を確認し、\
                setup_command をそのまま案内するか tako_setup を実行すること。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "action": {
                        "type": "string",
                        "enum": ["status", "show", "dismiss"],
                        "description": "操作種別（省略時は status）",
                    },
                },
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "tako_show_command",
            "description": "ユーザーに実行してほしいコマンドを、コピー可能なカードとして画面に出す\
                （FR-2.22 / Issue #666）。**ユーザーへコマンドを実行してもらうときは必ずこれを使う**。\
                会話本文にコマンドを書くだけだと、TUI がペイン幅で物理改行を入れるため\
                ユーザーが画面からコピーすると壊れる。このツールに渡した文字列はそのまま保管され、\
                カードは「コピー」（論理文字列を丸ごとクリップボードへ）と「新規ペインで実行」\
                （同じタブに別ペインを開いて実行。対話中のペインは触らない）のボタンを持つ。\
                action=show（既定）でカードを出す。commands は 1 件でも複数でもよく、\
                改行を含む複数行コマンドは 1 要素として渡す（改行はそのまま保たれる）。\
                label には「何のためのコマンドか」を短く書く。\
                action=list で表示中カードと保管されている論理文字列を確認できる。\
                action=copy / action=run はカードのボタンと同じ操作を AI から行う\
                （run は確認なしで実行されるので、ユーザーが明示的に頼んだときだけ使う）。\
                action=dismiss でカードを閉じる（card 省略時はそのペインの全カード）。\
                会話に書いたコマンドの代わりに出すこと。カードを出したら\
                「ペイン下部のカードからコピーか実行ができます」と一言添える。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "action": {
                        "type": "string",
                        "enum": ["show", "list", "copy", "run", "dismiss"],
                        "description": "操作種別（省略時は show）",
                    },
                    "commands": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "提示するコマンド（action=show で必須）。\
                            改行を含む複数行コマンドは 1 要素として渡す",
                    },
                    "label": {
                        "type": "string",
                        "description": "何のためのコマンドかの短い説明（任意。カード見出しに出る）",
                    },
                    "pane": {
                        "type": "integer",
                        "description": "カードを出すペイン ID（省略時は呼び出し元ペイン = 自分の会話ペイン）",
                    },
                    "card": {
                        "type": "integer",
                        "description": "対象カード ID（copy / run / dismiss。省略時は最新カード。\
                            dismiss は省略でそのペインの全カード）",
                    },
                    "index": {
                        "type": "integer",
                        "description": "対象コマンド番号（copy / run。1 始まり。省略時は 1）",
                    },
                    "focus": {
                        "type": "boolean",
                        "description": "run で新しいペインへフォーカスを移すか（既定 false）",
                    },
                },
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "tako_config_share",
            "description": "AI 系設定の git ベース共有（Issue #513）。tako の宣言的設定\
                （profiles / projects / accounts / local-rules / settings）と claude の\
                グローバル指示（CLAUDE.md / snippets / commands / templates）を 1 つの git\
                リポジトリでデバイス間（mac ⇔ Windows）共有する。\
                action=status（既定）で配線状態と push / pull 待ちの差分。\
                action=init で共有リポジトリを新規作成して配線（--remote で origin 登録）。\
                action=link で既存リポジトリ（ローカルパスまたは git URL）へ配線。\
                action=push で実体 → リポジトリへ書き出し + commit（+ push）。\
                action=pull でリポジトリ → 実体へ取り込み（世代バックアップつき）。\
                action=list で共有 / 非共有の分類表。\
                秘匿情報（token / credentials / .claude.json）とマシンローカル状態\
                （layout.json / sessions.yaml / workers.yaml）はホワイトリストで構造的に除外され、\
                未分類のファイルは共有されない。設定内の絶対パスはホーム部分が ~ に置き換わる。\
                ユーザーが「別の PC でも同じ設定を使いたい」と言ったらこれを使うこと。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "action": {
                        "type": "string",
                        "enum": ["status", "init", "link", "unlink", "push", "pull", "list"],
                        "description": "操作種別（省略時は status）",
                    },
                    "target": {
                        "type": "string",
                        "description": "link の対象（ローカルパスまたは git URL）",
                    },
                    "path": {
                        "type": "string",
                        "description": "リポジトリの配置先（init / URL clone 時。省略時は ~/tako-config-sync）",
                    },
                    "remote": {
                        "type": "string",
                        "description": "init 時に origin として登録するリモート URL",
                    },
                    "message": {
                        "type": "string",
                        "description": "push のコミットメッセージ",
                    },
                    "no_push": {
                        "type": "boolean",
                        "description": "true でリモートへ送らずコミットまでで止める",
                    },
                },
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "tako_shell_integration",
            "description": "シェル統合（OSC 7 / 133）の配置状態の確認と配置・解除（Issue #525 / #467）。\
                これが効いていないとペインの cwd 追従（`list` の cwd）とコマンド実行状態\
                （idle / running / failed）が取れない。\
                action: status（既定。状態だけ返す）/ install / uninstall。\
                unix は環境変数の注入だけで完結するので配置操作は要らない（uninstall はエラー）。\
                Windows は PowerShell の $PROFILE へマーカー付きブロックを 1 個置く（冪等）。\
                **応答の installed は「配置できたか」で effective は「実際に効くか」**で、\
                器（永続バックエンド）が OSC を通さないと配置済みでも効かない。\
                効かない理由は blocked_by_backend に入る。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "action": {
                        "type": "string",
                        "enum": ["status", "install", "uninstall"],
                        "description": "操作種別（省略時は status）",
                    },
                },
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "tako_agent_support",
            "description": "agent 能力マトリクスの参照（Issue #982 / #975）。\
                どのエージェント CLI（claude / codex / agy / ローカル LLM）でどの機能が \
                claude 同等に使えるか・縮退しているか・まだ使えないかを返す。\
                tako は claude を基準に実装してきたため、系統によって使えない操作がある。\
                **worker を codex / agy で立てる前と、その worker が期待どおり動かないときに引くこと**。\
                agent: 対象の系統（省略時は全系統ぶんの表）。\
                status: 絞り込み（supported / degraded / pending / unsupported。省略時は全件）。\
                応答の各項目は key（能力名）・summary（説明）・agents（系統ごとの status / note / issue）・\
                evidence（判定の根拠の種別）を持つ。pending は「tako が未実装」か「まだ調べていない」で、\
                unsupported は「上流の CLI にその手段が無い」の意味。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "agent": {
                        "type": "string",
                        "enum": ["claude", "codex", "agy", "local"],
                        "description": "対象の系統（省略時は全系統ぶんの表）",
                    },
                    "status": {
                        "type": "string",
                        "enum": ["supported", "degraded", "pending", "unsupported"],
                        "description": "この状態のものだけに絞る（省略時は全件）",
                    },
                },
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "tako_platform",
            "description": "プラットフォーム対応マトリクスの参照（Issue #515 / #467）。\
                どの機能がこの環境で使えるか・縮退しているか・未実装かを返す。\
                tako は macOS 先行で開発して Windows へ反映していくため、環境によって使えない操作がある。\
                Windows で作業していて操作が失敗したときは、まず status=pending で確認すること。\
                platform: 対象プラットフォーム（macos / windows。省略時は実行中の環境）。\
                status: 絞り込み（supported / degraded / pending / unsupported。省略時は全件）。\
                応答の各項目は key（MCP ツール名）・status・note（縮退の理由）・issue（追跡先）を持つ。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "platform": {
                        "type": "string",
                        "enum": ["macos", "windows"],
                        "description": "対象プラットフォーム（省略時は実行中の環境）",
                    },
                    "status": {
                        "type": "string",
                        "enum": ["supported", "degraded", "pending", "unsupported"],
                        "description": "この状態のものだけに絞る（省略時は全件）",
                    },
                    "known_limitations": {
                        "type": "boolean",
                        "description": "リリースノート用の Known limitations 節（日英併記の markdown）を \
                            known_limitations_markdown に併せて返す（Issue #594）",
                    },
                },
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "tako_ui_mode",
            "description": "UI 表示モード（GUI ライク表示 ⇔ ターミナル表示）の状態確認・切替（Issue #691）。\
                action=status（既定）: 現在のモードと、ターミナル表示へ戻してあるペインを返す。\
                action=set: mode（terminal / gui）へ切り替える。action=toggle: 反転する。\
                gui モードでは、アイドルなシェルのペインが「AI チームに任せる / AI と 1 対 1 で話す / \
                コマンド入力へ」の 3 ボタン（スターター）になる。set / toggle は settings.json へ \
                永続化され、全ウィンドウへ即時反映される。\
                action=release: pane で指定したペインだけをターミナル表示に戻す（揮発。\
                再起動すると gui 表示へ戻る）。action=restore: その解除を取り消す。\
                表示レイヤだけの切替なので PTY・tmux セッション・実行中プロセスには影響しない。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "action": {
                        "type": "string",
                        "enum": ["status", "set", "toggle", "release", "restore"],
                        "description": "操作種別（省略時は status）",
                    },
                    "mode": {
                        "type": "string",
                        "enum": ["terminal", "gui"],
                        "description": "表示モード（set 時に必須）",
                    },
                    "pane": {
                        "type": "integer",
                        "description": "release / restore の対象ペイン ID（省略時は呼び出し元ペイン）",
                    },
                },
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "tako_lang",
            "description": "UI 表示言語（日本語/英語）の状態確認・切替（Issue #435）。\
                action=status（既定）: 言語設定（system / ja / en）と実際の表示言語を返す。\
                action=set: value で指定した言語へ切り替える（system = OS ロケール追従）。\
                変更は settings.json に永続化され、GUI に即時反映される。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "action": {
                        "type": "string",
                        "enum": ["status", "set"],
                        "description": "操作種別（省略時は status）",
                    },
                    "value": {
                        "type": "string",
                        "enum": ["system", "ja", "en"],
                        "description": "言語設定（set 時に必須。system = OS ロケール追従）",
                    },
                },
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "tako_limit_service",
            "description": "ステータスバーの利用制限表示サービスの状態確認・切替・再取得（Issue #321 / #357）。\
                ステータスバーの 5h / 7d リミットメーターにどのサービス（claude / codex / agy）の値を表示するかを制御する。\
                action=status（既定）: 現在の選択サービスと利用可能サービス一覧を返す。\
                action=set: service で指定したサービスへ切り替える。変更は settings.json に永続化され、GUI に即時反映される。\
                action=refresh: 全ペインの TUI フッターを即時再走査し、各サービスの最新メトリクスを返す。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "action": {
                        "type": "string",
                        "enum": ["status", "set", "refresh"],
                        "description": "操作種別（省略時は status）",
                    },
                    "service": {
                        "type": "string",
                        "enum": ["claude", "codex", "agy"],
                        "description": "サービス名（set 時に必須）",
                    },
                },
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "tako_telemetry",
            "description": "エラーレポートの自動送信（テレメトリ）の状態確認・切替（Issue #333）。\
                tako 内で発生した panic / 重大エラーを PII なしで収集エンドポイントへ送信する。\
                action=status（既定）: 現在の ON/OFF・直近の送信件数・ログパスを返す。\
                action=on: テレメトリを有効化する。\
                action=off: テレメトリを無効化する。\
                変更は settings.json に永続化される。送信内容はすべてローカルの telemetry.log に記録される。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "action": {
                        "type": "string",
                        "enum": ["status", "on", "off"],
                        "description": "操作種別（省略時は status）",
                    },
                },
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "tako_setup_bootstrap",
            "description": "エージェント CLI（Claude Code）のゼロスタート導入を確認・実行する（Issue #868）。\
                **claude が入っていない環境で `tako setup` を通すための前段**。\
                action=status（既定・読み取り専用）は「次に何をすべきか」を next_step で返す\
                （install = 未導入 / path = PATH に無い / auth = 未ログイン / ready = 導入済み）。\
                install_plan には「何をどこに入れるか」（公式コマンド・取得元・置き場所・\
                自動更新の有無）が入るので、実行前に必ずユーザーへ提示すること。\
                action=install で公式インストーラ（macOS は curl -fsSL https://claude.ai/install.sh | bash）を\
                実行する。dry_run=true なら実行せず計画だけ返す。\
                action=path でランチャーの置き場所をログインシェルの PATH へ通す（冪等）。\
                action=undo-path で置いた設定を取り除く。\
                認証（auth）はブラウザ操作を伴うため自動化せず、ユーザーに \
                `claude auth login` の実行を案内すること。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "action": {
                        "type": "string",
                        "enum": ["status", "install", "path", "undo-path"],
                        "description": "操作種別（省略時は status）",
                    },
                    "dry_run": {
                        "type": "boolean",
                        "description": "action=install で実行せず計画だけ返す",
                    },
                },
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "tako_setup_models",
            "description": "エージェント CLI が使えるモデルの一覧を返す（Issue #1002）。読み取り専用。\
                **モデルを勧める前に必ずこれを引くこと**（学習時点の知識で存在しないモデル名を\
                提案すると起動が失敗する）。一覧は各 CLI の一覧コマンドから実取得する\
                （codex = `codex debug models` / agy = `agy models`）。claude は該当コマンドが\
                無いので同梱の既知エイリアス（opus / sonnet / fable）+ ローカルキャッシュを返し、\
                failure.kind = no_list_command で「実取得ではない」ことを明示する。\
                各モデルの efforts はその系統・そのモデルが受け付ける effort 語彙\
                （codex はモデルごとに違う）なので、effort はここに載っている値から選ぶ。\
                失敗は failure.kind で分類される（cli_not_found = 未導入・install に導入手順 / \
                not_authenticated = 未ログイン / no_list_command = 一覧コマンドが無い / \
                command_failed / parse_failed）。**選んだ値の反映はこのツールではしない**: \
                tako_orchestrator_profiles の model / effort へ書くこと（apply_command が形を示す）。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "agent": {
                        "type": "string",
                        "enum": ["claude", "codex", "agy", "all"],
                        "description": "対象の系統（省略時 = all で全系統）",
                    },
                },
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "tako_setup",
            "description": "tako setup を非対話で実行する（Issue #262）。ユーザーが日本語で伝えた好みを answers に変換して代行する。省略項目は detected → previous → default の順で自動解決され、標準ケースは質問ゼロで完走する。instructions / profile / projects / orchestrator / sleep_guard は明示指定時だけ既存値を更新する。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "selected_agent": {
                        "type": "string",
                        "enum": ["claude", "codex", "agy"],
                        "description": "setup の既定 agent。省略時は検出・前回値から自動決定",
                    },
                    "provider_plans": {
                        "type": "object",
                        "description": "プロバイダ別プラン。キーは claude / gpt / google",
                        "additionalProperties": {"type": "string"},
                    },
                    "instruction_content": {
                        "type": "string",
                        "description": "選択 agent のグローバル指示ファイルへ書く完全な Markdown。省略時は既存維持",
                    },
                    "profile": {
                        "type": "object",
                        "description": "profiles/default.yaml の完全な設定。省略時は既存維持または推奨生成",
                        "additionalProperties": true,
                    },
                    "projects": {
                        "type": "object",
                        "description": "projects.yaml の全登録。明示時だけ既存一覧を置換",
                        "additionalProperties": {
                            "type": "object",
                            "properties": {
                                "cwd": {"type": "string"},
                                "description": {"type": "string"},
                            },
                            "required": ["cwd"],
                            "additionalProperties": false,
                        },
                    },
                    "orchestrator": {
                        "type": "object",
                        "properties": {
                            "auto_close": {"type": "boolean"},
                            "auto_push": {"type": "boolean"},
                        },
                        "additionalProperties": false,
                    },
                    "sleep_guard": {
                        "type": "object",
                        "properties": {
                            "mode": {"type": "string", "enum": ["off", "on", "while-agents-running"]},
                            "power": {"type": "string", "enum": ["ac-only", "always"]},
                        },
                        "additionalProperties": false,
                    },
                },
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "tako_setup_changes",
            "description": "tako setup のアップデート追従状況を照会する（Issue #94）。\
                前回 `tako setup` 完了時に適用したリビジョン（applied_revision）と\
                バイナリ同梱の setup changelog の現在リビジョンを突き合わせ、\
                未適用の setup 関連変更（セットアップ項目・設定フォーマット・\
                master 用システムプロンプト等の変更）の一覧を返す。読み取り専用。\
                pending の各エントリの kind が auto なら `tako setup` の再実行だけで追従が\
                完了する。guided ならユーザー所有ファイル（CLAUDE.md・profiles 等）に関わる\
                ため、`tako setup --review` で個別確認する。自動追従は `tako setup` を案内すること。",
            "inputSchema": {
                "type": "object",
                "properties": {},
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "tako_agents_sync_rules",
            "description": "エージェント共通ルールの同期（#136）。\
                正本ファイルの内容を各エージェント（claude / codex / agy）のグローバル指示ファイルに\
                マーカーブロックで埋め込む。ブロック外の既存内容は一切変更しない。\
                action=sync（既定）: 同期を実行し結果を返す。書き換え前にバックアップ(.bak)を生成する。\
                action=status: 設定と現在の同期状態を返す（読み取り専用）。\
                正本パスは tako setup で設定済みの値を使うが、source で一時的に上書きできる。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "action": {
                        "type": "string",
                        "enum": ["sync", "status"],
                        "description": "操作種別（省略時は sync）",
                    },
                    "source": {
                        "type": "string",
                        "description": "正本ファイルの絶対パス（省略時は config.yaml の設定値）",
                    },
                    "targets": {
                        "type": "array",
                        "items": { "type": "string", "enum": ["claude", "codex", "agy"] },
                        "description": "同期対象エージェント（省略時は設定値 or 全対象）",
                    },
                },
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "tako_tree_folder",
            "description": "ファイルツリーへのフォルダの追加・削除・一覧（#134）と \
                git ステータスの取得（#1009）。\
                AI が作業対象プロジェクトのフォルダをファイルツリーに明示追加する。\
                追加されたフォルダは cwd 由来のエントリと並んでツリーに表示される。\
                プロジェクトの指示を受けたらそのルートフォルダを追加し、\
                作業対象外になったら削除する。タブ単位スコープ（永続化される）。\
                action=git-status: ツリーに色とバッジで出ている git の状態を\
                そのまま返す（画面と同じ分類）。entries[] の state は \
                modified / added / deleted / renamed / untracked / conflicted / ignored、\
                staged / unstaged は git の XY（`git status --short` と同じ記号）で\
                ステージ済みと未ステージを分けて持つ。propagated=true は\
                ディレクトリ行（配下からの伝播で、changed が配下の変更ファイル数）。\
                「未コミットのファイルは？」「どのフォルダに変更がある？」には \
                git を叩き直さずここから答えられる。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "action": {
                        "type": "string",
                        "enum": ["add", "remove", "list", "git-status"],
                        "description": "add: フォルダを追加, remove: フォルダを削除, \
                            list: 追加済み一覧, git-status: ツリーの git 状態を取得"
                    },
                    "path": {
                        "type": "string",
                        "description": "追加・削除するフォルダの絶対パス（list 時は省略可）。\
                            git-status では対象をこのフォルダ 1 件へ絞る（省略時はタブの\
                            ワークスペースフォルダ全部 = 画面に出ている範囲）"
                    },
                    "tab": {
                        "type": "integer",
                        "description": "対象タブ ID（省略時は呼び出し元ペインのタブ）"
                    },
                    "limit": {
                        "type": "integer",
                        "description": "git-status: 返すエントリ数の上限（既定 500）"
                    },
                },
                "required": ["action"],
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "tako_sessions",
            "description": "セッションカタログの参照と会話の復元（Issue #112）。\
                tako が起動した master / worker / solo / 手動 claude の会話セッションを、\
                ラベル・ロール・プロジェクト・Issue 番号つきで発見できるインデックス。\
                会話本文は claude の transcript（~/.claude/projects/）への参照のみ持つ。\
                action=list: 一覧（role / project で絞り込み、last_seen の新しい順に limit 件）。\
                action=show: id（前方一致可）のメタ情報 + 会話冒頭の抜粋。\
                action=resume: ペイン / タブ / 永続化の器（tmux / psmux）が全滅していても、記録された cwd で\
                新しいペインを分割起動し `claude --resume <session_id>` で会話文脈ごと復元する。\
                「昨日の #159 の子を呼び戻して」のような依頼は list で特定 → resume で復元する。\
                制限: resume は claude セッションのみ（codex / agy は list に載るが復元不可）。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "action": {
                        "type": "string",
                        "enum": ["list", "show", "resume"],
                        "description": "操作種別",
                    },
                    "id": {
                        "type": "string",
                        "description": "session_id（前方一致可。show / resume で必須）",
                    },
                    "role": {
                        "type": "string",
                        "enum": ["master", "worker", "solo", "pane"],
                        "description": "list の種別絞り込み",
                    },
                    "project": {
                        "type": "string",
                        "description": "list のプロジェクト絞り込み",
                    },
                    "limit": {
                        "type": "integer",
                        "description": "list の最大件数（既定 30）",
                    },
                    "pane": {
                        "type": "integer",
                        "description": "resume の分割元ペイン ID（省略時は呼び出し元）",
                    },
                    "tab": {
                        "type": "integer",
                        "description": "resume の分割先タブ ID（そのタブのフォーカスペインの隣）",
                    },
                    "direction": {
                        "type": "string",
                        "enum": ["right", "down", "left", "up"],
                        "description": "resume の分割方向（省略時 right）",
                    },
                },
                "required": ["action"],
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "tako_logs",
            "description": "ペインの平文ターミナルログの参照・設定（Issue #112）。\
                全ペインのスクロールバック確定行を平文でローテーション保存しており、\
                ペイン / タブ / アプリが死んだ後でもビルド・テスト出力を遡れる。\
                TUI（claude 等）の描画は保存されない（「TUI 実行中」マーカーのみ。\
                会話の復元は tako_sessions を使う）。\
                action=list: ログファイル一覧。action=read: 末尾 lines 行（既定 200）を返す。\
                対象は pane（クローズ済み可）か session_id（カタログ経由）。\
                action=status: 有効/無効・上限・保存先。action=set: enabled / max_mb / \
                total_max_mb の変更（永続化）。ログはユーザーローカル保存で、\
                トークン等が写り込み得るため内容を外部へ送らないこと。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "action": {
                        "type": "string",
                        "enum": ["list", "read", "status", "set"],
                        "description": "操作種別",
                    },
                    "pane": {
                        "type": "integer",
                        "description": "read 対象のペイン ID（クローズ済みでも可）",
                    },
                    "session_id": {
                        "type": "string",
                        "description": "read 対象のセッション ID（カタログ経由で端末ログを引く）",
                    },
                    "lines": {
                        "type": "integer",
                        "description": "read の表示行数（既定 200）",
                    },
                    "enabled": {
                        "type": "boolean",
                        "description": "set: ログ保存の ON/OFF",
                    },
                    "max_mb": {
                        "type": "integer",
                        "description": "set: ペインあたりの上限（MB）",
                    },
                    "total_max_mb": {
                        "type": "integer",
                        "description": "set: ログ全体の上限（MB）",
                    },
                },
                "required": ["action"],
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "tako_open_dir",
            "description": "ディレクトリを新タブで開く（#20）。cwd を設定してシェルを起動し、\
                ファイルツリーにフォルダを自動追加する。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "開くディレクトリの絶対パス",
                    },
                    "focus": {
                        "type": "boolean",
                        "description": "新タブにフォーカスを移すか（省略時 true）",
                    },
                },
                "required": ["path"],
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "tako_open_remote",
            "description": "SSH ホストに接続するペインを開く（#20 / #919 / #1006）。~/.ssh/config の Host 名を\
                指定すると、HostName / User / Port 等の設定を尊重して ssh コマンドを実行する。\
                未定義ホストでも ssh <host> として実行できる。\
                接続に失敗した場合はペインを閉じずに理由と次の一手を表示する（#919）。\
                ツリー側（tako_remote_folder）と同じ接続を共有するので、ここで一度ログインすれば\
                パスワード認証しか無い相手でもリモートツリーが追加認証なしで開く。\
                開き先は target で選ぶ（既定 split = いま開いているタブへ新ペイン。#1006）。\
                target=pane は**すでにあるペインをそのまま SSH にする**（ペイン ID は変わらず、\
                接続に失敗してもそのペインのシェルへ戻る）。素のシェルでないペイン\
                （全画面 TUI・実行中・AI エージェント・プレビュー）は理由つきで断る。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "host": {
                        "type": "string",
                        "description": "SSH ホスト名（~/.ssh/config の Host、または直接 hostname）",
                    },
                    "focus": {
                        "type": "boolean",
                        "description": "接続したペインへフォーカスを移すか（省略時 true）",
                    },
                    "remote_dir": {
                        "type": "string",
                        "description": "接続後に cd するリモートのパス（省略時はログイン時の cwd）",
                    },
                    "target": {
                        "type": "string",
                        "enum": ["split", "tab", "pane"],
                        "description": "開き先（#1006。省略時 split = いまのタブへ新ペイン / \
                            tab = 新しいタブ / pane = 既存ペインをそのまま SSH 化）",
                    },
                    "pane": {
                        "type": "integer",
                        "description": "対象ペイン ID（target=pane は SSH 化するペイン / \
                            target=split は分割元。省略時は呼び出し元ペイン）",
                    },
                    "tab": {
                        "type": "integer",
                        "description": "対象タブ ID（target=split のとき。省略時はアクティブタブ）",
                    },
                    "direction": {
                        "type": "string",
                        "enum": ["right", "down", "left", "up"],
                        "description": "分割方向（target=split のとき。省略時 right）",
                    },
                },
                "required": ["host"],
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "tako_remote_folder",
            "description": "リモート（SSH 先）のフォルダをワークスペースとして開く・閉じる・覗く\
                （#919 / #65。Zed / VSCode の Remote SSH 相当）。ファイルツリーに SSH 先の\
                ディレクトリ構造が並び、ファイルはプレビューで開いて**編集・保存できる**\
                （#966。保存は SFTP の一時ファイル + rename でアトミック。開いた時点から\
                リモートが変わっていたら上書きせず conflict を返す。書けないファイルは\
                read_only=true で返る）。\
                認証は ~/.ssh/config・鍵・ControlMaster をそのまま使う（追加設定なし）。\
                action: open = 接続してフォルダをツリーへ開く（path 省略でリモートのホーム。\
                接続に失敗したら開かずに理由を返す。#1041: ツリーの**先頭**（ローカルより前）に\
                並び、同じタブへ SSH 済み + そのフォルダへ cd 済みのターミナルペインも用意する\
                = VSCode Remote 相当。同じホストへ繋がった生きたペインがあれば作らない。\
                terminal=false で開くだけにできる。応答の terminal.connected / reason / pane と\
                origin / placement で結果が読める）/ close = 閉じる（path 省略でそのホストの全部、\
                all=true で全ホスト）/ list = 開いているリモートフォルダの一覧（読み込み状態つき。\
                **ツリーに出ている並び**で返り、各行の origin = explicit / auto と\
                placement = leading / trailing でローカルの前後どちらに出ているかが分かる）/\
                ls = ツリーを開かずにリモートのディレクトリを一覧する（構造の把握に使う）/\
                open-file = リモートのファイルをプレビューで開く（応答の read_only / size /\
                mode / mtime で書けるかが分かる。保存は tako_preview_save）/ ssh-pane = そのフォルダで\
                SSH ペインを開く / pending = リモートへ押し出せていない保存の一覧（切断中の保存は\
                ここに残るので無言で消えない）/ push = 押し出せていない保存の再試行\
                （force=true で競合を承知のうえ上書き）/ auto = ペインの ssh を検知した自動追加の\
                状態を返す（enabled=true/false で切替。#976。ユーザーがペインで `ssh <host>` に\
                入ると、そのホストのホームがツリーへ自動で並ぶ。検知した接続の生死・見送った\
                理由もここで読める）。GUI の「リモートからフォルダを開く」と 1:1。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "action": {
                        "type": "string",
                        "enum": ["open", "close", "list", "ls", "open-file", "ssh-pane", "pending", "push", "auto"],
                        "description": "操作の種類",
                    },
                    "host": {
                        "type": "string",
                        "description": "SSH ホスト名（~/.ssh/config の Host）。list 以外で必須",
                    },
                    "path": {
                        "type": "string",
                        "description": "リモート側の絶対パス（POSIX。Windows の相手は /C:/Users/... の形）",
                    },
                    "tab": {
                        "type": "integer",
                        "description": "対象タブ ID（省略時はアクティブタブ）",
                    },
                    "focus": {
                        "type": "boolean",
                        "description": "open-file / ssh-pane で新しいペイン・タブにフォーカスを移すか",
                    },
                    "all": {
                        "type": "boolean",
                        "description": "close でホスト指定なしに全部閉じる（既定は全タブ横断。tab で 1 タブへ絞れる）",
                    },
                    "force": {
                        "type": "boolean",
                        "description": "push で競合（開いた時点からリモートが変わっている）を承知のうえ上書きする（#966）",
                    },
                    "enabled": {
                        "type": "boolean",
                        "description": "auto で自動追加の有効・無効を切り替える（省略で現在値の照会だけ。#976）",
                    },
                    "terminal": {
                        "type": "boolean",
                        "description": "open でターミナルも同じホストへ繋ぐか（#1041。省略時 true。false にすると開くだけ = あとから action=ssh-pane で繋げる）",
                    },
                },
                "required": ["action"],
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "tako_ssh_hosts",
            "description": "~/.ssh/config の Host 一覧を返す（#20）。ワイルドカード（*）を含む\
                エントリは除外される。各ホストの name / hostname / user / port を返す。",
            "inputSchema": {
                "type": "object",
                "properties": {},
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "tako_recent",
            "description": "最近開いたディレクトリ/リポジトリ/SSH ホストの一覧・クリア（#20）。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "action": {
                        "type": "string",
                        "enum": ["list", "clear"],
                        "description": "操作種別",
                    },
                },
                "required": ["action"],
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "tako_task_checkpoint",
            "description": "タスクチェックポイントの記録・更新（Issue #242）。\
                worker タスクの進行状態（Issue 番号・ブランチ・フェーズ・直近コミット等）を \
                永続化し、クラッシュ・利用上限・API 切断からの resume を可能にする。\
                task_id を省略すると自動採番される。既存の task_id を指定すると上書き更新する。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "task_id": { "type": "string", "description": "タスク ID（省略時は自動採番 task-N）" },
                    "pane": { "type": "integer", "description": "ペイン ID" },
                    "issue": { "type": "integer", "description": "GitHub Issue 番号" },
                    "branch": { "type": "string", "description": "作業ブランチ名" },
                    "phase": { "type": "string", "enum": ["queued", "running", "verifying", "done", "failed", "suspended"], "description": "フェーズ（省略時 running）" },
                    "last_commit": { "type": "string", "description": "直近の git commit SHA" },
                    "agent": { "type": "string", "description": "エージェント種別（claude / codex / agy）" },
                    "model": { "type": "string", "description": "モデル名" },
                    "prompt_head": { "type": "string", "description": "コンテキスト復元用のプロンプト冒頭" },
                    "suspended_reason": { "type": "string", "description": "一時停止の理由（usage_limit / api_error / crash 等）" },
                    "project": { "type": "string", "description": "プロジェクト名（projects.yaml のキー）" },
                    "cwd": { "type": "string", "description": "作業ディレクトリ" },
                },
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "tako_task_list",
            "description": "タスクチェックポイントの一覧（Issue #242）。\
                永続化された全チェックポイントを updated_at の新しい順に返す。\
                phase で絞り込み可能（例: suspended で中断中のタスクだけ表示）。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "phase": { "type": "string", "enum": ["queued", "running", "verifying", "done", "failed", "suspended"], "description": "フェーズで絞り込む（省略時は全件）" },
                },
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "tako_task_resume",
            "description": "チェックポイントから worker を再開する（Issue #242）。\
                指定した task_id のチェックポイントを読み、元の branch / cwd / issue コンテキストを \
                resume プロンプトに含めて新しいペインに worker を spawn する。\
                モデルを変更して再開することも可能（usage_limit 後に別モデルへ切り替え等）。\
                再開後、チェックポイントの phase は running に遷移し、pane_id が新ペインに更新される。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "task_id": { "type": "string", "description": "再開するチェックポイントの task_id" },
                    "model": { "type": "string", "description": "モデルを変更して再開する（省略時はチェックポイントのモデル）" },
                    "pane": { "type": "integer", "description": "分割元ペイン ID（省略時は呼び出し元）" },
                    "tab": { "type": "integer", "description": "分割先タブ ID" },
                },
                "required": ["task_id"],
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "tako_task_gate",
            "description": "受け入れゲートの定義（Issue #244）。\
                タスクに機械検証可能な受け入れ条件（述語）を設定する。\
                Command 述語はシェルコマンドの exit code、PrMerged は PR のマージ状態、\
                Custom は人間判断で判定する。\
                設定後は tako_task_gate_check で述語を実行し、結果を記録する。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "task_id": { "type": "string", "description": "対象のタスク ID（checkpoint の task_id と同じ）" },
                    "criteria": {
                        "type": "array",
                        "description": "受け入れ条件の配列",
                        "items": {
                            "type": "object",
                            "properties": {
                                "id": { "type": "string", "description": "条件 ID（例: tests_green, pr_merged）" },
                                "kind": {
                                    "type": "object",
                                    "description": "条件の種別。type=command: {cmd, expect_exit_0?}、type=pr_merged: {pr_number, repo?}、type=custom: {description}",
                                },
                            },
                            "required": ["id", "kind"],
                        },
                    },
                    "cwd": { "type": "string", "description": "Command 述語の実行ディレクトリ（省略時は worker の cwd）" },
                },
                "required": ["task_id", "criteria"],
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "tako_task_gate_check",
            "description": "受け入れゲートの述語を実行し、結果を記録する（Issue #244）。\
                Command 述語はシェルコマンドを実行し exit code で判定、\
                PrMerged 述語は gh pr view で PR のマージ状態を判定する。\
                Custom 述語はスキップされる（手動で tako_task_gate の record_results で設定）。\
                sync_checkpoint=true のとき、全 Passed で checkpoint.phase が done に遷移する。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "task_id": { "type": "string", "description": "対象のタスク ID" },
                    "sync_checkpoint": { "type": "boolean", "description": "true のとき、全 Passed で checkpoint.phase を done に遷移させる（既定 true）" },
                },
                "required": ["task_id"],
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "tako_task_gate_show",
            "description": "受け入れゲートの状態を表示する（Issue #244）。\
                各 criterion の id / kind / status / evidence / checked_at と、\
                overall（pending / passed / failed）を返す。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "task_id": { "type": "string", "description": "対象のタスク ID" },
                },
                "required": ["task_id"],
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "tako_run_interactive",
            "description": "ユーザー入力が必要なコマンドを可視ペインに委譲する（Issue #305）。\
                sudo パスワード・ブラウザ認証・対話プロンプト等、AI が直接入力できない操作を \
                split -> タイトル設定 -> コマンド投入までアトミックに実行し、pane_id を返す。\
                コマンドは exit code 回収マーカーでラップされるため、完了後に \
                tako_run_interactive_status で exit code を回収できる。\
                使い方: (1) run_interactive でペインを開く (2) ユーザーに入力を案内する \
                (3) status で完了を確認する (4) auto_close に従いペインが自動 close される",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "command": {
                        "type": "string",
                        "description": "実行するコマンド文字列（シェル経由で実行される）",
                    },
                    "input_hint": {
                        "type": "string",
                        "description": "ユーザーへの入力案内（タイトルに表示。省略時はコマンド文字列が使われる）",
                    },
                    "pane": pane_schema("分割の基準ペイン ID（省略時は呼び出し元。tab と排他）"),
                    "tab": {
                        "type": "integer", "minimum": 0,
                        "description": "分割先タブ ID（pane と排他）",
                    },
                    "direction": {
                        "type": "string",
                        "enum": ["right", "down", "left", "up"],
                        "description": "新ペインが生える方向（省略時は right）",
                    },
                    "ratio": {
                        "type": "number",
                        "exclusiveMinimum": 0.0,
                        "exclusiveMaximum": 1.0,
                        "description": "新ペイン側の取り分（省略時は 0.3）",
                    },
                    "auto_close": {
                        "type": "string",
                        "enum": ["success", "always", "never"],
                        "description": "完了後の自動 close 方針。success（既定）= exit 0 で close / always = 常に close / never = 残す",
                    },
                },
                "required": ["command"],
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "tako_run_interactive_status",
            "description": "run-interactive で起動したペインの完了状態を確認する（Issue #305）。\
                ペイン出力から exit code マーカーを探し、見つかれば exit code と auto_close の \
                結果を返す。見つからなければ status: running を返す。\
                完了検知後、auto_close 方針に従いペインを自動 close する（success: exit 0 のみ / \
                always: 常に / never: 残す）。AI は完了まで定期的にポーリングすること。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "pane": {
                        "type": "integer", "minimum": 0,
                        "description": "run-interactive が返した pane ID",
                    },
                },
                "required": ["pane"],
                "additionalProperties": false,
            },
        }),
        // --- Code Runner (FR-3.18, #453) ---
        json!({
            "name": "tako_run",
            "description": "ファイルを実行する（Code Runner: FR-3.18, #453）。\
                ファイル内の tako:run 宣言または拡張子既定コマンドで新ペインを分割して実行する。\
                \n\n## tako:run 宣言の書式\n\
                ファイル先頭 64 行以内に以下の形式でコメント内に記述する:\n\
                - `tako:run: <コマンド>` — 既定の実行コマンド\n\
                - `tako:run[name]: <コマンド>` — 名前付きプロファイル（複数定義可）\n\
                - `tako:cwd: <ディレクトリ>` — 作業ディレクトリ（相対パスはファイル基準）\n\
                - `tako:cwd[name]: <ディレクトリ>` — プロファイル別作業ディレクトリ\n\
                - `tako:shell: <シェル>` — コマンドを解釈するシェル\n\
                \nスキャン範囲: 先頭 64 行 / 16 KiB。各言語のコメント記法に依存しない（接頭辞は任意）。\n\
                \n## 変数展開\n\
                コマンド・cwd 内で以下の変数が使える（自動シングルクオートエスケープ）:\n\
                - `${file}` — ファイルの絶対パス\n\
                - `${fileDir}` — ファイルのあるディレクトリ\n\
                - `${fileBase}` — ファイル名（拡張子付き）\n\
                - `${fileNoExt}` — ファイル名（拡張子なし）\n\
                - `${ext}` — 拡張子（小文字・ドットなし）\n\
                \n## 解決優先順位\n\
                1. command パラメータ（最優先）\n\
                2. ファイル内宣言\n\
                3. 拡張子既定（settings + 組み込み）\n\
                4. エラー\n\
                \n完了確認は tako_run_interactive_status を使う。cwd はファイルのあるディレクトリが既定。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "実行対象ファイルパス（相対パスは呼び出し元ペインの cwd 基準）",
                    },
                    "profile": {
                        "type": "string",
                        "description": "実行プロファイル名（省略時は既定プロファイル）",
                    },
                    "command": {
                        "type": "string",
                        "description": "コマンド上書き（最優先。宣言・拡張子既定より優先される）",
                    },
                    "pane": pane_schema("分割の基準ペイン ID（省略時は呼び出し元）"),
                    "tab": {
                        "type": "integer", "minimum": 0,
                        "description": "分割先タブ ID（pane と排他）",
                    },
                    "direction": {
                        "type": "string",
                        "enum": ["right", "down", "left", "up"],
                        "description": "新ペインが生える方向（既定 down）",
                    },
                    "ratio": {
                        "type": "number",
                        "exclusiveMinimum": 0.0,
                        "exclusiveMaximum": 1.0,
                        "description": "新ペイン側の取り分（既定 0.3）",
                    },
                    "auto_close": {
                        "type": "string",
                        "enum": ["success", "always", "never"],
                        "description": "完了後の自動 close 方針。never（既定）= 残す / success = exit 0 で close / always = 常に close",
                    },
                    "focus": {
                        "type": "boolean",
                        "description": "新ペインにフォーカスを移すか（既定 false）",
                    },
                },
                "required": ["path"],
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "tako_run_resolve",
            "description": "ファイルの実行プロファイル一覧を解決して返す（実行しない。FR-3.18, #453）。\
                ファイル内宣言と拡張子既定から検出されたプロファイル一覧・コマンド・cwd・source を返す。\
                UI のドロップダウンと同じデータ。tako_run 実行前の事前確認に使う。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "対象ファイルパス（相対パスは呼び出し元ペインの cwd 基準）",
                    },
                    "pane": pane_schema("相対パス解決の基準ペイン ID（省略時は呼び出し元）"),
                },
                "required": ["path"],
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "tako_run_defaults",
            "description": "拡張子ごとの実行コマンド既定を一覧/設定/削除する（FR-3.18, #453）。\
                ext を省略すると全一覧。ext のみで単一情報。ext + command で設定。ext + remove で削除。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "ext": {
                        "type": "string",
                        "description": "拡張子（ドットなし・小文字。省略時は全一覧）",
                    },
                    "command": {
                        "type": "string",
                        "description": "設定するコマンドテンプレート（変数展開 ${fileBase} 等が使える）",
                    },
                    "remove": {
                        "type": "boolean",
                        "description": "true で削除（組み込み既定に戻る）",
                    },
                },
                "additionalProperties": false,
            },
        }),
    ]
}
