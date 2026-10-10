#[cfg(test)]
mod tests {
    use super::*;

    /// #1945: `tako_limit_resume` の `all` + `enabled` は一括（呼び出し元ペインを解かない =
    /// tako の外の MCP クライアントからも押せる）。`pane` は退避中でも素通しして dispatch が引く
    #[test]
    fn tako_limit_resume_の_all_と_enabled_は一括の要求になる() {
        assert_eq!(
            build_request(
                "tako_limit_resume",
                &json!({"all": true, "enabled": true}),
                None,
                None
            )
            .unwrap(),
            Request::LimitResume {
                pane: None,
                enabled: Some(true),
                all: Some(true),
            }
        );
        assert_eq!(
            build_request("tako_limit_resume", &json!({"pane": 42, "enabled": false}), Some(7), None)
                .unwrap(),
            Request::LimitResume {
                pane: Some(42),
                enabled: Some(false),
                all: None,
            }
        );
        assert!(validate_known_params("tako_limit_resume", &json!({"all": true, "enabled": false})).is_ok());
    }

    /// #1908: `tako_tree_folder` の `selection` はツリーの選択の要求（CLI `tako tree selection`
    /// と同じ）になり、ほかの action はこれまでどおり `TreeFolder`
    #[test]
    fn tako_tree_folder_の_selection_はツリーの選択の要求になる() {
        assert_eq!(
            build_request(
                "tako_tree_folder",
                &json!({"action": "selection", "key": "extend_bottom"}),
                None,
                None
            )
            .unwrap(),
            Request::TreeSelection {
                path: None,
                key: Some("extend_bottom".into()),
                tab: None,
            }
        );
        assert_eq!(
            build_request(
                "tako_tree_folder",
                &json!({"action": "selection", "path": "/w/a", "tab": 3}),
                None,
                None
            )
            .unwrap(),
            Request::TreeSelection {
                path: Some("/w/a".into()),
                key: None,
                tab: Some(3),
            }
        );
        assert!(matches!(
            build_request("tako_tree_folder", &json!({"action": "list"}), None, None).unwrap(),
            Request::TreeFolder { .. }
        ));
        assert!(
            validate_known_params("tako_tree_folder", &json!({"action": "selection", "key": "up"}))
                .is_ok()
        );
    }

    /// #1867: `tako_file_op` の `paths` はまとめた要求（ツリーの複数選択と同じ 1 要求）になり、
    /// 進み具合・取り消しはパスを取らない。`path` も `paths` も無ければ断る
    #[test]
    fn tako_file_op_の_paths_はまとめた要求になり進み具合はパスを取らない() {
        use crate::protocol::FileOpKind;
        assert_eq!(
            build_request(
                "tako_file_op",
                &json!({"op": "move", "paths": ["/w/a", "/w/b"], "dest": "/w/d"}),
                None,
                None
            )
            .unwrap(),
            Request::FileOpMany {
                op: FileOpKind::Move,
                paths: vec!["/w/a".into(), "/w/b".into()],
                dest: Some("/w/d".into()),
            }
        );
        assert!(validate_known_params("tako_file_op", &json!({"op": "trash", "paths": []})).is_ok());
        // #1895: `paths` の copy も 1 要求（CLI `tako file copy a b dst` と同じ要求 = 1 つのジョブ）
        assert_eq!(
            build_request(
                "tako_file_op",
                &json!({"op": "copy", "paths": ["/w/a", "/w/b"], "dest": "/w/d"}),
                None,
                None
            )
            .unwrap(),
            Request::FileOpMany {
                op: FileOpKind::Copy,
                paths: vec!["/w/a".into(), "/w/b".into()],
                dest: Some("/w/d".into()),
            }
        );
        for (op, kind) in [
            ("copy_progress", FileOpKind::CopyProgress),
            ("copy_cancel", FileOpKind::CopyCancel),
        ] {
            assert_eq!(
                build_request("tako_file_op", &json!({ "op": op }), None, None).unwrap(),
                Request::FileOp {
                    op: kind,
                    path: String::new(),
                    name: None,
                    pane: None,
                    dest: None,
                }
            );
        }
        assert!(matches!(
            build_request(
                "tako_file_op",
                &json!({"op": "paste_move", "path": "/w/d"}),
                None,
                None
            )
            .unwrap(),
            Request::FileOp {
                op: FileOpKind::PasteMove,
                ..
            }
        ));
        let err = build_request("tako_file_op", &json!({"op": "trash"}), None, None).unwrap_err();
        assert!(err.contains("path"), "{err}");
        let err = build_request(
            "tako_file_op",
            &json!({"op": "trash", "paths": "/w/a"}),
            None,
            None,
        )
        .unwrap_err();
        assert!(err.contains("配列"), "{err}");
    }

    /// #1679: `tako_lsp` は CLI `tako lsp diagnostics` と同じ要求になる。
    /// pane を省けば全文書（呼び出し元のペインで補わない。呼び出し元はたいてい端末）
    #[test]
    fn tako_lsp_は_cli_と同じ診断の要求になる() {
        assert_eq!(
            build_request("tako_lsp", &json!({}), Some(3), None).unwrap(),
            Request::LspDiagnostics {
                pane: None,
                severity: None
            }
        );
        assert_eq!(
            build_request(
                "tako_lsp",
                &json!({"action": "diagnostics", "pane": 7, "severity": "warning"}),
                Some(3),
                None
            )
            .unwrap(),
            Request::LspDiagnostics {
                pane: Some(7),
                severity: Some("warning".into())
            }
        );
        let err =
            build_request("tako_lsp", &json!({"action": "no-such-action"}), None, None).unwrap_err();
        assert!(err.contains("diagnostics"), "{err}");
    }

    /// #1679 と #1680 の合流: `tako_lsp` は 1 本で、action の先頭が既定の diagnostics、
    /// 続いて定義ジャンプの綴りの正本（`GotoKind::NAMES`）そのもの、整形の 2 つ（#1683）、補完（#1682）、
    /// 右クリックメニュー（#1684）、最後にホバー（#1681）。
    /// 種類を足したのに action の表へ載せ忘れる・振り分けが受けない、のどちらもここで落ちる
    #[test]
    fn tako_lsp_の_action_は診断と定義ジャンプの全種を振り分ける() {
        let actions = crate::dispatch::LSP_FEATURE_ACTIONS;
        assert_eq!(actions[0], "diagnostics");
        let goto = &actions[1..=tako_core::lsp::goto::GotoKind::NAMES.len()];
        assert_eq!(goto, &tako_core::lsp::goto::GotoKind::NAMES[..]);
        // 定義ジャンプの後ろは整形の 2 つと補完と右クリックメニュー（#1684）とホバー（#1681）
        // （残りはこれだけ = 足し忘れはここで落ちる）
        assert_eq!(
            &actions[goto.len() + 1..],
            &[
                crate::dispatch::LSP_FORMAT_ACTION,
                crate::dispatch::LSP_FORMAT_ON_SAVE_ACTION,
                crate::dispatch::LSP_COMPLETION_ACTION,
                crate::dispatch::LSP_MENU_ACTION,
                crate::dispatch::LSP_HOVER_ACTION,
            ]
        );
        for action in goto {
            assert_eq!(
                build_request(
                    "tako_lsp",
                    &json!({"action": action, "line": 3, "column": 4}),
                    Some(9),
                    None
                )
                .unwrap(),
                Request::LspGoto {
                    action: action.to_string(),
                    pane: Some(9),
                    line: 3,
                    column: 4,
                    open: None,
                    choice: None,
                    focus: None,
                },
                "{action}"
            );
        }
        // 定義ジャンプは位置が要る（診断の既定へ黙って倒れない）
        assert!(build_request("tako_lsp", &json!({"action": "definition"}), Some(9), None).is_err());
    }

    /// #1683: 整形は CLI `tako lsp format [--pane N] [--range L1:C1-L2:C2]` と同じ要求になる。
    /// 範囲は 4 つそろえて指定し、一部だけは全体へ倒さずに拒否する
    #[test]
    fn tako_lsp_の整形は_cli_と同じ要求になる() {
        assert_eq!(
            build_request("tako_lsp", &json!({"action": "format"}), Some(4), None).unwrap(),
            Request::LspFormat {
                pane: Some(4),
                range: None
            }
        );
        assert_eq!(
            build_request(
                "tako_lsp",
                &json!({"action": "format", "pane": 7, "line": 2, "column": 0, "end_line": 5, "end_column": 3}),
                Some(4),
                None
            )
            .unwrap(),
            Request::LspFormat {
                pane: Some(7),
                range: Some(crate::protocol::LineColRange {
                    start_line: 2,
                    start_col: 0,
                    end_line: 5,
                    end_col: 3
                })
            }
        );
        let err = build_request(
            "tako_lsp",
            &json!({"action": "format", "line": 2, "column": 0}),
            Some(4),
            None,
        )
        .unwrap_err();
        assert!(err.contains("end_line"), "{err}");
        assert_eq!(
            build_request("tako_lsp", &json!({"action": "format-on-save"}), Some(4), None).unwrap(),
            Request::LspFormatOnSave { enabled: None }
        );
        assert_eq!(
            build_request(
                "tako_lsp",
                &json!({"action": "format-on-save", "enabled": true}),
                None,
                None
            )
            .unwrap(),
            Request::LspFormatOnSave {
                enabled: Some(true)
            }
        );
    }

    /// #1682: `tako_lsp` の action=completion は CLI `tako lsp completion` と同じ要求になる
    /// （pane の省略は呼び出し元 = 定義ジャンプと同じ。位置は必須）
    #[test]
    fn tako_lsp_の補完は_cli_と同じ要求になる() {
        assert_eq!(
            build_request(
                "tako_lsp",
                &json!({"action": "completion", "line": 3, "column": 4, "limit": 20, "choice": 2, "resolve": true}),
                Some(9),
                None
            )
            .unwrap(),
            Request::LspCompletion {
                pane: Some(9),
                line: 3,
                column: 4,
                limit: Some(20),
                choice: Some(2),
                resolve: Some(true),
            }
        );
        assert_eq!(
            build_request(
                "tako_lsp",
                &json!({"action": "completion", "pane": 5, "line": 1, "column": 0}),
                Some(9),
                None
            )
            .unwrap(),
            Request::LspCompletion {
                pane: Some(5),
                line: 1,
                column: 0,
                limit: None,
                choice: None,
                resolve: None,
            }
        );
        assert!(build_request("tako_lsp", &json!({"action": "completion"}), Some(9), None).is_err());
    }

    /// #1684: `tako_lsp` の action=menu は CLI `tako lsp menu` と同じ要求になる
    /// （pane の省略は呼び出し元 = 定義ジャンプと同じ。位置は必須）
    #[test]
    fn tako_lsp_の右クリックメニューは_cli_と同じ要求になる() {
        assert_eq!(
            build_request(
                "tako_lsp",
                &json!({"action": "menu", "line": 3, "column": 4}),
                Some(9),
                None
            )
            .unwrap(),
            Request::LspMenu {
                pane: Some(9),
                line: 3,
                column: 4,
            }
        );
        assert!(build_request("tako_lsp", &json!({"action": "menu", "line": 3}), Some(9), None).is_err());
    }

    /// #1684 の番犬: 右クリックメニューの項目はすべて、応答の `args` をそのまま `tako_lsp` へ渡すと
    /// **GUI のクリックと同じ要求**（`lsp::menu::item_request`）になる。項目を足したのに MCP で
    /// 押せない・GUI と AI で別の操作になる、のどちらもここで落ちる
    #[test]
    fn 右クリックメニューの項目はすべて同じ要求になる() {
        use crate::lsp::menu::{item_args, item_request, ItemTarget};
        let target = ItemTarget {
            pane: 5,
            line: 3,
            column: 4,
            selection: Some(crate::protocol::LineColRange {
                start_line: 2,
                start_col: 0,
                end_line: 6,
                end_col: 1,
            }),
        };
        for item in tako_core::lsp::menu::MenuItem::ALL {
            let args = item_args(item, &target).expect("選択があれば全項目に引数がある");
            assert_eq!(
                args["action"].as_str(),
                Some(crate::lsp::menu::action_of(item)),
                "{item:?}"
            );
            assert_eq!(
                build_request("tako_lsp", &args, Some(99), None).unwrap(),
                item_request(item, &target, None).unwrap(),
                "{item:?}: MCP の args と GUI のクリックの要求が違う"
            );
        }
    }

    /// #1681: `tako_lsp` の action=hover は CLI `tako lsp hover` と同じ要求になる
    /// （pane の省略は呼び出し元 = 定義ジャンプと同じ。位置は必須。show は省略で wire に載らない）
    #[test]
    fn tako_lsp_のホバーは_cli_と同じ要求になる() {
        assert_eq!(
            build_request(
                "tako_lsp",
                &json!({"action": "hover", "line": 3, "column": 4}),
                Some(9),
                None
            )
            .unwrap(),
            Request::LspHover {
                pane: Some(9),
                line: 3,
                column: 4,
                show: None,
                limit: None,
            }
        );
        assert_eq!(
            build_request(
                "tako_lsp",
                &json!({"action": "hover", "pane": 5, "line": 1, "column": 0, "show": true}),
                Some(9),
                None
            )
            .unwrap(),
            Request::LspHover {
                pane: Some(5),
                line: 1,
                column: 0,
                show: Some(true),
                limit: None,
            }
        );
        assert!(build_request("tako_lsp", &json!({"action": "hover"}), Some(9), None).is_err());
        // #1893: limit は本文の字数の上限（0 = 全文 = CLI の --full）。負・文字列は拒否する
        assert_eq!(
            build_request(
                "tako_lsp",
                &json!({"action": "hover", "line": 2, "column": 1, "limit": 0}),
                Some(9),
                None
            )
            .unwrap(),
            Request::LspHover {
                pane: Some(9),
                line: 2,
                column: 1,
                show: None,
                limit: Some(0),
            }
        );
        for bad in [json!(-1), json!("all")] {
            assert!(build_request(
                "tako_lsp",
                &json!({"action": "hover", "line": 2, "column": 1, "limit": bad}),
                Some(9),
                None
            )
            .is_err());
        }
    }

    #[test]
    fn 全公開ツールにrequest変換またはspecial_handlerがある() {
        for tool in tools() {
            let name = tool["name"].as_str().expect("ツール名は文字列");
            if special_tool(name).is_some() {
                continue;
            }
            let result = build_request(name, &json!({}), Some(1), None);
            assert!(
                !matches!(&result, Err(message) if message == &format!("不明なツール: {name}")),
                "{name} はカタログにあるが Request 変換が無い"
            );
        }
    }

    /// #1653: 検索・置換の条件は省略すると wire に載らない（= dispatch が既定で埋める）。
    /// 指定したときだけ CLI（`-i` / `-w`）と同じ値が 1:1 で渡る
    #[test]
    fn issue1653_検索と置換の条件は省略と指定がそのまま渡る() {
        let (_, requests) = run(
            call("tako_preview_search", json!({ "query": "value" })),
            Some(7),
            true,
        );
        assert_eq!(
            requests,
            vec![Request::PreviewSearch {
                pane: Some(7),
                query: Some("value".into()),
                direction: None,
                case_sensitive: None,
                whole_word: None,
                visible: None,
            }]
        );
        let (_, requests) = run(
            call(
                "tako_preview_search",
                json!({ "case_sensitive": false, "whole_word": true }),
            ),
            Some(7),
            true,
        );
        assert_eq!(
            requests,
            vec![Request::PreviewSearch {
                pane: Some(7),
                query: None,
                direction: None,
                case_sensitive: Some(false),
                whole_word: Some(true),
                visible: None,
            }]
        );
        let (_, requests) = run(
            call(
                "tako_preview_replace",
                json!({ "query": "value", "replacement": "item", "all": true }),
            ),
            Some(7),
            true,
        );
        assert_eq!(
            requests,
            vec![Request::PreviewReplace {
                pane: Some(7),
                query: "value".into(),
                replacement: "item".into(),
                all: Some(true),
                case_sensitive: None,
                whole_word: None,
            }]
        );
        let (_, requests) = run(
            call(
                "tako_preview_replace",
                json!({ "query": "v", "replacement": "w", "case_sensitive": false }),
            ),
            Some(7),
            true,
        );
        assert_eq!(
            requests,
            vec![Request::PreviewReplace {
                pane: Some(7),
                query: "v".into(),
                replacement: "w".into(),
                all: None,
                case_sensitive: Some(false),
                whole_word: None,
            }]
        );
        // 省略したときの wire は引数が生える前とバイト一致（skip_serializing_if）
        let wire = serde_json::to_value(&requests[0]).unwrap();
        assert!(wire.get("whole_word").is_none(), "{wire}");
        // 型違いは黙って捨てずに拒否する
        let (response, requests) = run(
            call(
                "tako_preview_search",
                json!({ "query": "v", "case_sensitive": "no" }),
            ),
            Some(7),
            true,
        );
        assert!(requests.is_empty(), "型違いが Request まで届いた");
        assert!(response.unwrap().to_string().contains("case_sensitive"));
    }

    /// #1873: 検索欄の開閉は `visible` が CLI（`--open` / `--close`）と 1:1 で渡る。
    /// 省略したときは wire に載らない（引数が生える前の JSON とバイト一致）
    #[test]
    fn issue1873_検索欄の開閉はvisibleでそのまま渡る() {
        for visible in [true, false] {
            let (_, requests) = run(
                call("tako_preview_search", json!({ "visible": visible })),
                Some(7),
                true,
            );
            assert_eq!(
                requests,
                vec![Request::PreviewSearch {
                    pane: Some(7),
                    query: None,
                    direction: None,
                    case_sensitive: None,
                    whole_word: None,
                    visible: Some(visible),
                }]
            );
        }
        let (_, requests) = run(
            call("tako_preview_search", json!({ "query": "v" })),
            Some(7),
            true,
        );
        let wire = serde_json::to_value(&requests[0]).unwrap();
        assert!(wire.get("visible").is_none(), "{wire}");
        let (response, requests) = run(
            call("tako_preview_search", json!({ "visible": "open" })),
            Some(7),
            true,
        );
        assert!(requests.is_empty(), "型違いが Request まで届いた");
        assert!(response.unwrap().to_string().contains("visible"));
    }

    /// 受けた Request を記録して固定値を返す exec
    fn run(message: Value, caller: Option<u64>, connected: bool) -> (Option<Value>, Vec<Request>) {
        let mut seen = Vec::new();
        let mut exec = |request: Request| -> Result<Value, String> {
            seen.push(request);
            Ok(json!({ "pane": 7 }))
        };
        let mut session = McpSession {
            caller_pane: caller,
            caller_role: None,
            connected,
            exec: &mut exec,
        };
        let response = handle_message(&message, &mut session);
        (response, seen)
    }

    fn call(name: &str, args: Value) -> Value {
        json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/call",
            "params": { "name": name, "arguments": args },
        })
    }

    #[test]
    fn initializeはバージョン交渉とinstructionsを返す() {
        let message = json!({
            "jsonrpc": "2.0", "id": 0, "method": "initialize",
            "params": { "protocolVersion": "2025-03-26" },
        });
        let (response, _) = run(message, None, true);
        let result = &response.unwrap()["result"];
        assert_eq!(result["protocolVersion"], "2025-03-26");
        assert_eq!(result["serverInfo"]["name"], "tako");
        // 行動規範（FR-2.7.5）が埋め込まれている
        let instructions = result["instructions"].as_str().unwrap();
        assert!(instructions.contains("レビューを求めるときは見せろ"));
        assert!(instructions.contains("片付け"));

        // 未知バージョンは最新を名乗る
        let message = json!({
            "jsonrpc": "2.0", "id": 0, "method": "initialize",
            "params": { "protocolVersion": "9999-01-01" },
        });
        let (response, _) = run(message, None, true);
        assert_eq!(
            response.unwrap()["result"]["protocolVersion"],
            PROTOCOL_VERSION
        );
    }

    #[test]
    fn notificationとresponseには応答しない() {
        let (response, _) = run(
            json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }),
            None,
            true,
        );
        assert!(response.is_none());
        let (response, _) = run(
            json!({ "jsonrpc": "2.0", "id": 5, "result": {} }),
            None,
            true,
        );
        assert!(response.is_none());
    }

    #[test]
    fn open_fileはモードを解釈し呼び出し元へフォールバックする() {
        let (response, requests) = run(
            call(
                "tako_open_file",
                json!({ "path": "/tmp/x.md", "mode": "code" }),
            ),
            Some(7),
            true,
        );
        assert!(response.is_some());
        assert_eq!(
            requests,
            vec![Request::OpenFile {
                pane: Some(7),
                path: "/tmp/x.md".into(),
                mode: Some(crate::protocol::PreviewModeWire::Code),
                direction: None,
                focus: None,
                new_tab: false,
                line: None,
                column: None,
            }]
        );
        // mode 省略は拡張子の自動判定に委ねる（None で渡る）。direction も省略可
        let (_, requests) = run(
            call("tako_open_file", json!({ "path": "a.rs" })),
            Some(7),
            true,
        );
        assert_eq!(
            requests,
            vec![Request::OpenFile {
                pane: Some(7),
                path: "a.rs".into(),
                mode: None,
                direction: None,
                focus: None,
                new_tab: false,
                line: None,
                column: None,
            }]
        );
        // direction 指定（FR-3.11 = D&D のドロップ位置の同等操作）
        let (_, requests) = run(
            call(
                "tako_open_file",
                json!({ "path": "a.rs", "direction": "down" }),
            ),
            Some(7),
            true,
        );
        assert_eq!(
            requests,
            vec![Request::OpenFile {
                pane: Some(7),
                path: "a.rs".into(),
                mode: None,
                direction: Some(Direction::Down),
                focus: None,
                new_tab: false,
                line: None,
                column: None,
            }]
        );
        // #1676: line / column は 1 始まりでそのまま dispatch へ渡る（CLI と同じ経路）
        let (_, requests) = run(
            call(
                "tako_open_file",
                json!({ "path": "a.rs", "line": 42, "column": 7 }),
            ),
            Some(7),
            true,
        );
        assert_eq!(
            requests,
            vec![Request::OpenFile {
                pane: Some(7),
                path: "a.rs".into(),
                mode: None,
                direction: None,
                focus: None,
                new_tab: false,
                line: Some(42),
                column: Some(7),
            }]
        );
        // 負値は引数の型で弾く（0 と丸めは dispatch = open_plan の 1 実装が見る）
        let (response, requests) = run(
            call("tako_open_file", json!({ "path": "a.rs", "line": -1 })),
            Some(7),
            true,
        );
        assert!(requests.is_empty());
        assert!(response.unwrap()["error"]["message"]
            .as_str()
            .unwrap()
            .contains("line"));
        // 不正な mode と path 欠落は引数エラー
        let (response, requests) = run(
            call("tako_open_file", json!({ "path": "a.rs", "mode": "html" })),
            Some(7),
            true,
        );
        assert!(requests.is_empty());
        assert!(response.unwrap()["error"]["message"]
            .as_str()
            .unwrap()
            .contains("mode"));
        let (response, _) = run(call("tako_open_file", json!({})), Some(7), true);
        assert!(response.unwrap()["error"]["message"]
            .as_str()
            .unwrap()
            .contains("path"));
    }

    /// #1677: `tako_jump` は action 引数の 1 ツールで `Request::Jump` へ写る（CLI と同じ綴り）。
    /// action 省略は list、基準ペインは呼び出し元へフォールバックし、分からなくても失敗しない
    #[test]
    fn jumpはactionをそのまま渡し既定はlist() {
        for action in crate::dispatch::JUMP_ACTIONS {
            let (_, requests) = run(
                call("tako_jump", json!({ "action": action, "focus": true })),
                Some(7),
                true,
            );
            assert_eq!(
                requests,
                vec![Request::Jump {
                    action: action.to_string(),
                    pane: Some(7),
                    focus: Some(true),
                }]
            );
        }
        let (_, requests) = run(call("tako_jump", json!({})), None, true);
        assert_eq!(
            requests,
            vec![Request::Jump {
                action: "list".into(),
                pane: None,
                focus: None,
            }]
        );
        let (_, requests) = run(
            call("tako_jump", json!({ "action": "back", "pane": 3 })),
            Some(7),
            true,
        );
        assert_eq!(
            requests,
            vec![Request::Jump {
                action: "back".into(),
                pane: Some(3),
                focus: None,
            }]
        );
    }

    /// #1654: `tako_preview_edit` の `command` は編集コマンドへ写る（ツールを増やさない）。
    /// `enabled` と同時に渡したら拒否する（どちらをしたいのか決まらない）
    #[test]
    fn preview_editのcommandは編集コマンドへ写す() {
        let (_, requests) = run(
            call(
                "tako_preview_edit",
                json!({ "command": "indent", "expected_version": 3 }),
            ),
            Some(7),
            true,
        );
        assert_eq!(
            requests,
            vec![Request::PreviewEditCommand {
                pane: Some(7),
                command: "indent".into(),
                expected_version: Some(3),
            }]
        );
        let (response, requests) = run(
            call(
                "tako_preview_edit",
                json!({ "command": "newline", "enabled": true }),
            ),
            Some(7),
            true,
        );
        assert!(requests.is_empty(), "{requests:?}");
        assert!(format!("{response:?}").contains("同時に"), "{response:?}");
    }

    #[test]
    fn preview編集3操作をrequestへ写す() {
        let (_, requests) = run(
            call("tako_preview_edit", json!({ "enabled": true })),
            Some(7),
            true,
        );
        assert_eq!(
            requests,
            vec![Request::PreviewEdit {
                pane: Some(7),
                enabled: Some(true),
            }]
        );
        let (_, requests) = run(
            call(
                "tako_preview_apply",
                json!({ "pane": 9, "text": "日本語\n" }),
            ),
            Some(7),
            true,
        );
        assert_eq!(
            requests,
            vec![Request::PreviewApply {
                pane: Some(9),
                text: "日本語\n".into(),
            }]
        );
        let (_, requests) = run(call("tako_preview_save", json!({})), Some(7), true);
        assert_eq!(
            requests,
            vec![Request::PreviewSave {
                pane: Some(7),
                force: false
            }]
        );
    }

    /// #1659: 外部変更の後の逃げ道はツールを増やさず `tako_preview_save` の action で表す
    #[test]
    fn preview_saveのactionは上書きと読み直しと差分へ写る() {
        let cases = [
            ("save", Request::PreviewSave { pane: Some(7), force: false }),
            ("overwrite", Request::PreviewSave { pane: Some(7), force: true }),
            ("reload", Request::PreviewRevert { pane: Some(7) }),
            ("diff", Request::PreviewDiff { pane: Some(7) }),
        ];
        // 綴り表（カタログの enum）と写像の対応が 1 つも欠けない
        assert_eq!(
            cases.iter().map(|(a, _)| *a).collect::<Vec<_>>(),
            crate::dispatch::PREVIEW_SAVE_ACTIONS
        );
        for (action, expected) in cases {
            let (_, requests) = run(
                call("tako_preview_save", json!({ "action": action })),
                Some(7),
                true,
            );
            assert_eq!(requests, vec![expected], "action={action}");
        }
        let (response, requests) = run(
            call("tako_preview_save", json!({ "action": "force" })),
            Some(7),
            true,
        );
        assert!(requests.is_empty(), "知らない action は送らない");
        assert!(format!("{response:?}").contains("overwrite"), "{response:?}");
    }

    #[test]
    fn preview_viewは倍率ページパンをrequestへ写す() {
        let (_, requests) = run(
            call(
                "tako_preview_view",
                json!({
                    "pane": 7,
                    "zoom": 150.0,
                    "page": 3,
                    "pan_x": 24.0,
                    "pan_y": 48.0
                }),
            ),
            None,
            true,
        );
        assert_eq!(
            requests,
            vec![Request::PreviewView {
                pane: Some(7),
                zoom: Some(150.0),
                zoom_in: false,
                zoom_out: false,
                reset: false,
                page: Some(3),
                pan_x: Some(24.0),
                pan_y: Some(48.0),
            }]
        );
    }

    #[test]
    fn preview_outlineは一覧取得と項目ジャンプをrequestへ写す() {
        let (_, requests) = run(
            call("tako_preview_outline", json!({ "pane": 7 })),
            None,
            true,
        );
        assert_eq!(
            requests,
            vec![Request::PreviewOutline {
                pane: Some(7),
                item: None,
            }]
        );
        let (_, requests) = run(
            call("tako_preview_outline", json!({ "item": 2 })),
            Some(5),
            true,
        );
        assert_eq!(
            requests,
            vec![Request::PreviewOutline {
                pane: Some(5),
                item: Some(2),
            }]
        );
    }

    #[test]
    fn tmux_openはセッション必須でドロップ位置相当を写す() {
        let (_, requests) = run(
            call(
                "tako_tmux_open",
                json!({ "session": "master-tako", "socket": "work", "direction": "down" }),
            ),
            Some(3),
            true,
        );
        assert_eq!(
            requests,
            vec![Request::TmuxOpen {
                socket: Some("work".into()),
                session: "master-tako".into(),
                window: None,
                pane: Some(3),
                direction: Some(Direction::Down),
            }]
        );
        // session 欠落は引数エラー
        let (response, requests) = run(call("tako_tmux_open", json!({})), Some(3), true);
        assert!(requests.is_empty());
        assert!(response.unwrap()["error"]["message"]
            .as_str()
            .unwrap()
            .contains("session"));
    }

    /// #1185: protocol にある `window` が MCP から到達できる
    /// （catalog の inputSchema に無いと `additionalProperties: false` が弾いていた）
    #[test]
    fn tmux_openのwindowがmcpから到達できる() {
        let (response, requests) = run(
            call(
                "tako_tmux_open",
                json!({ "session": "mywork", "socket": "usersock", "window": 2 }),
            ),
            Some(3),
            true,
        );
        assert_eq!(
            requests,
            vec![Request::TmuxOpen {
                socket: Some("usersock".into()),
                session: "mywork".into(),
                window: Some(2),
                pane: Some(3),
                direction: None,
            }]
        );
        assert_eq!(response.unwrap()["result"]["isError"], false);
    }

    #[test]
    fn tako_setup_changesはsetup_changesリクエストに変換される() {
        let (response, requests) = run(call("tako_setup_changes", json!({})), None, true);
        assert_eq!(requests, vec![Request::SetupChanges]);
        assert_eq!(response.unwrap()["result"]["isError"], false);
    }

    #[test]
    fn tako_setup_modelsはsetup_modelsリクエストに変換される() {
        // 省略 = 全系統
        let (response, requests) = run(call("tako_setup_models", json!({})), None, true);
        assert_eq!(requests, vec![Request::SetupModels { agent: None }]);
        assert_eq!(response.unwrap()["result"]["isError"], false);
        // 系統の絞り込みがそのまま渡る（CLI `--agent` と 1:1）
        let (_, requests) = run(
            call("tako_setup_models", json!({"agent": "codex"})),
            None,
            true,
        );
        assert_eq!(
            requests,
            vec![Request::SetupModels {
                agent: Some("codex".into())
            }]
        );
    }

    #[test]
    fn tako_setupは全回答をsetup_runリクエストに変換する() {
        let answers = json!({
            "selected_agent": "codex",
            "provider_plans": {"gpt": "plus"},
            "instruction_content": "# Rules",
            "profile": {"master_agent": "codex", "effort": "high"},
            "projects": {"app": {"cwd": "~/src/app"}},
            "orchestrator": {"auto_close": false, "auto_push": false},
            "sleep_guard": {"mode": "while-agents-running", "power": "ac-only"}
        });
        let (_, requests) = run(call("tako_setup", answers.clone()), None, true);
        assert_eq!(
            requests,
            vec![Request::SetupRun {
                answers: Some(answers)
            }]
        );
    }

    /// **#513 受け入れ条件 4**: MCP → dispatch の写像が CLI と 1:1 であること
    #[test]
    fn tako_config_shareはリクエストに変換される() {
        // 省略 = status（CLI の `tako config` と同じ既定）
        let (response, requests) = run(call("tako_config_share", json!({})), None, true);
        assert_eq!(
            requests,
            vec![Request::ConfigShare {
                action: None,
                target: None,
                path: None,
                remote: None,
                message: None,
                no_push: false,
            }]
        );
        assert_eq!(response.unwrap()["result"]["isError"], false);

        let (_, requests) = run(
            call(
                "tako_config_share",
                json!({
                    "action": "push",
                    "message": "[改善] 設定を更新",
                    "no_push": true,
                }),
            ),
            None,
            true,
        );
        assert_eq!(
            requests,
            vec![Request::ConfigShare {
                action: Some("push".into()),
                target: None,
                path: None,
                remote: None,
                message: Some("[改善] 設定を更新".into()),
                no_push: true,
            }]
        );

        let (_, requests) = run(
            call(
                "tako_config_share",
                json!({ "action": "link", "target": "git@example.com:me/cfg.git", "path": "~/cfg" }),
            ),
            None,
            true,
        );
        assert_eq!(
            requests,
            vec![Request::ConfigShare {
                action: Some("link".into()),
                target: Some("git@example.com:me/cfg.git".into()),
                path: Some("~/cfg".into()),
                remote: None,
                message: None,
                no_push: false,
            }]
        );
    }

    #[test]
    fn tako_orchestrator_layoutはリクエストに変換される() {
        // 全省略 = 取得
        let (response, requests) = run(call("tako_orchestrator_layout", json!({})), None, true);
        assert_eq!(
            requests,
            vec![Request::OrchestratorLayout {
                policy: None,
                master_ratio: None,
                algorithm: None,
                min_worker_cols: None,
                auto_shrink_font: None,
                min_worker_font_scale: None,
            }]
        );
        assert_eq!(response.unwrap()["result"]["isError"], false);

        // 指定あり = 設定
        let (_, requests) = run(
            call(
                "tako_orchestrator_layout",
                json!({ "policy": "legacy", "master_ratio": 0.6, "algorithm": "spiral", "min_worker_cols": 80, "auto_shrink_font": false, "min_worker_font_scale": 0.7 }),
            ),
            None,
            true,
        );
        assert_eq!(
            requests,
            vec![Request::OrchestratorLayout {
                policy: Some("legacy".into()),
                master_ratio: Some(0.6),
                algorithm: Some("spiral".into()),
                // #1132: worker ペインの下限幅も 1:1 で届く
                min_worker_cols: Some(80),
                // #1439: 自動縮小の可否と床も 1:1 で届く
                auto_shrink_font: Some(false),
                min_worker_font_scale: Some(0.7),
            }]
        );
    }

    #[test]
    fn preview_reloadは状態取得と切替をrequestへ写す() {
        let (_, requests) = run(call("tako_preview_reload", json!({})), None, true);
        assert_eq!(requests, vec![Request::PreviewReload { enabled: None }]);

        let (_, requests) = run(
            call("tako_preview_reload", json!({ "enabled": false })),
            None,
            true,
        );
        assert_eq!(
            requests,
            vec![Request::PreviewReload {
                enabled: Some(false)
            }]
        );
    }

    /// #600 / #614: 入力予測本体・確定ヒント・Tab 確定が MCP から 1:1 で操作できる
    #[test]
    fn autosuggestは状態取得と切替をrequestへ写す() {
        let (_, requests) = run(call("tako_autosuggest", json!({})), None, true);
        assert_eq!(
            requests,
            vec![Request::Autosuggest {
                enabled: None,
                hint: None,
                tab: None
            }]
        );

        let (_, requests) = run(
            call("tako_autosuggest", json!({ "enabled": false })),
            None,
            true,
        );
        assert_eq!(
            requests,
            vec![Request::Autosuggest {
                enabled: Some(false),
                hint: None,
                tab: None
            }]
        );

        // #614: ヒント・Tab 確定だけを触る（本体は None のまま = 変更しない）
        let (_, requests) = run(
            call("tako_autosuggest", json!({ "hint": false, "tab": false })),
            None,
            true,
        );
        assert_eq!(
            requests,
            vec![Request::Autosuggest {
                enabled: None,
                hint: Some(false),
                tab: Some(false)
            }]
        );
    }

    #[test]
    fn preview_cacheは状態取得と上限変更をrequestへ写す() {
        let (_, requests) = run(call("tako_preview_cache", json!({})), None, true);
        assert_eq!(requests, vec![Request::PreviewCache { max_mb: None }]);

        let (_, requests) = run(
            call("tako_preview_cache", json!({ "max_mb": 768 })),
            None,
            true,
        );
        assert_eq!(requests, vec![Request::PreviewCache { max_mb: Some(768) }]);
    }

    /// #818: 状態取得と上限変更が Request へ 1:1 で写る
    #[test]
    fn scrollbackは状態取得と上限変更をrequestへ写す() {
        let (_, requests) = run(call("tako_scrollback", json!({})), None, true);
        assert_eq!(requests, vec![Request::Scrollback { lines: None }]);

        let (_, requests) = run(call("tako_scrollback", json!({ "lines": 2000 })), None, true);
        assert_eq!(
            requests,
            vec![Request::Scrollback {
                lines: Some(2_000)
            }]
        );
    }

    #[test]
    fn ツールカタログは操作セットを網羅する() {
        let tools = tools();
        // 件数の固定値。ツール追加時はここと対応マトリクス（#515）の両方を更新する
        // （分類漏れ自体は tests/platform_parity.rs の T1 が検出する）。
        // #549 の tako_welcome と #552 の tako_pin_tab_title が別 PR で同時に
        // 125 → 126 へ更新したため、両方 merge 後の main では 127 とずれていた
        // #513 の tako_config_share を追加して 129
        // #666 の tako_show_command を追加して 130
        // #680 の tako_preview_copy_code を追加して 131
        // #694 の tako_ui_mode を追加して 132
        // #725 の tako_chat_copy を追加して 133
        // #813 の tako_limit_resume を追加して 134
        // #657 の tako_menu（Windows 移植スライス 5）を追加して 135
        // #525 の tako_shell_integration（Windows 移植スライス 7）を追加して 136
        // #868 の tako_setup_bootstrap（ゼロスタート導入）を追加して 137
        // #915 の tako_orchestrator_handoffs（引き継ぎファイルの管理）を追加して 138
        // #916 の tako_migrate（設定の自動マイグレーション）を追加して 139
        // #919 の tako_remote_folder（リモートからフォルダを開く）を追加して 140
        // #1002 の tako_setup_models（モデル一覧の実取得）を追加して 142
        // #1057 の tako_setup_deps（任意依存の検出とその場導入）を追加して 143
        // #1067 の tako_session_restart（会話を引き継いだ再起動）を追加して 144
        // #1154 の tako_orchestrator_guide（master の手順書）を追加して 146
        // #818 の tako_scrollback（スクロールバック保持上限）を追加して 147
        // #1283 の tako_links（画面のリンク検出）を追加して 148
        // #1296 の tako_test_residue（テスト・検証プロセスの残骸の掃除）を追加して 149
        // #1450 の tako_todo（ユーザー向けタスク）を追加して 150
        // #1451 の tako_remote_shortcuts（リモート閲覧のショートカット）を追加して 151
        // #1453 の tako_orchestrator_adopt（master を専用プロファイルへその場で寄せる）を追加して 152
        // #1658 の tako_preview_edit_range / tako_preview_cursor（行・桁の範囲編集とカーソル）を追加して 154
        // #1652 の tako_preview_move / tako_preview_delete（単語・行単位の移動と削除）を追加して 156
        // #1678 の tako_lsp_server（言語サーバの状態と起動・停止）を追加して 157
        // #1677 の tako_jump（ジャンプ履歴の戻る / 進む / 一覧）を追加して 158
        // #1679 の tako_lsp（言語機能。診断の一覧）を追加して 159。#1680 の定義ジャンプは同じツールの action なので増えない
        // #1879 の tako_mod（Claude Code の mod 連携の状態と切替）を追加して 160
        assert_eq!(tools.len(), 160);
        for tool in &tools {
            let name = tool["name"].as_str().unwrap();
            assert!(name.starts_with("tako_"), "{name} は tako_ 接頭辞");
            assert!(!tool["description"].as_str().unwrap().is_empty());
            assert_eq!(tool["inputSchema"]["type"], "object");
        }
        // 行動規範が説明文側にも埋め込まれている（FR-2.7.5）
        let split = tools
            .iter()
            .find(|t| t["name"] == "tako_split_pane")
            .unwrap();
        assert!(split["description"].as_str().unwrap().contains("レビュー"));
        // #666: 「会話に書かずカードで出せ」の行動規範を説明文へ埋め込む（FR-2.22.9）
        let card = tools
            .iter()
            .find(|t| t["name"] == "tako_show_command")
            .unwrap();
        let desc = card["description"].as_str().unwrap();
        assert!(desc.contains("実行してほしいコマンド"), "{desc}");
        assert!(desc.contains("物理改行"), "壊れる理由を書くこと: {desc}");
    }

    /// #666: コマンド提案カードの引数マッピング。commands は取りこぼしを許さない
    #[test]
    fn show_commandの引数が正しく写る() {
        let (_, requests) = run(
            call(
                "tako_show_command",
                json!({ "commands": ["brew install tmux", "tako setup"], "label": "依存を入れる" }),
            ),
            Some(7),
            true,
        );
        assert_eq!(
            requests,
            vec![Request::ShowCommand {
                action: None,
                commands: vec!["brew install tmux".into(), "tako setup".into()],
                label: Some("依存を入れる".into()),
                pane: None,
                card: None,
                index: None,
                focus: None,
            }]
        );
        // 単一文字列も 1 件として受理する（配列を忘れた呼び出しを弾かない）
        let (_, requests) = run(
            call("tako_show_command", json!({ "commands": "tako master" })),
            Some(7),
            true,
        );
        assert!(matches!(
            requests.first(),
            Some(Request::ShowCommand { commands, .. }) if commands == &vec!["tako master".to_string()]
        ));
        // 文字列以外の要素は黙って捨てずエラーにする（提示内容が変わってしまうため）
        let (response, requests) = run(
            call("tako_show_command", json!({ "commands": ["ok", 42] })),
            Some(7),
            true,
        );
        assert!(requests.is_empty(), "不正な引数で dispatch しない");
        let text = response.unwrap().to_string();
        assert!(text.contains("commands"), "{text}");
    }

    #[test]
    fn 未接続ではツールを公開しない() {
        let (response, _) = run(
            json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/list" }),
            None,
            false,
        );
        assert_eq!(response.unwrap()["result"]["tools"], json!([]));
    }

    #[test]
    fn splitは呼び出し元ペインへフォールバックする() {
        let (response, seen) = run(
            call("tako_split_pane", json!({ "direction": "down" })),
            Some(3),
            true,
        );
        assert_eq!(
            seen,
            vec![Request::Split {
                pane: Some(3),
                tab: None,
                direction: Some(Direction::Down),
                ratio: None,
                command: None,
                cwd: None,
                focus: None,
            }]
        );
        let result = &response.unwrap()["result"];
        assert_eq!(result["isError"], false);
        assert!(result["content"][0]["text"].as_str().unwrap().contains("7"));
    }

    #[test]
    fn 呼び出し元不明でpane省略はエラー() {
        let (response, seen) = run(call("tako_close_pane", json!({})), None, true);
        assert!(seen.is_empty());
        let error = &response.unwrap()["error"];
        assert_eq!(error["code"], -32602);
        assert!(error["message"].as_str().unwrap().contains("pane"));
    }

    #[test]
    fn sendとreadはpane必須() {
        let (response, seen) = run(
            call("tako_send_input", json!({ "text": "ls" })),
            Some(3), // 呼び出し元があってもフォールバックしない（誤送信防止）
            true,
        );
        assert!(seen.is_empty());
        assert_eq!(response.unwrap()["error"]["code"], -32602);

        // #1283: リンク検出は pane / text のどちらでも渡せる
        let (_, seen_links) = run(
            call("tako_links", json!({ "pane": 4 })),
            None,
            true,
        );
        assert_eq!(
            seen_links,
            vec![Request::Links {
                pane: Some(4),
                text: None,
                cols: None,
                cwd: None,
            }]
        );
        let (_, seen_links) = run(
            call(
                "tako_links",
                json!({ "text": "見て `/tmp/x.md`。", "cols": 80, "cwd": "/tmp" }),
            ),
            None,
            true,
        );
        assert_eq!(
            seen_links,
            vec![Request::Links {
                pane: None,
                text: Some("見て `/tmp/x.md`。".into()),
                cols: Some(80),
                cwd: Some("/tmp".into()),
            }]
        );

        let (_, seen) = run(
            call("tako_read_pane", json!({ "pane": 4, "lines": 10 })),
            None,
            true,
        );
        assert_eq!(
            seen,
            vec![Request::Read {
                pane: Some(4),
                lines: Some(10),
                tmux_session: None,
            }]
        );
    }

    #[test]
    fn 実行エラーはエラーフラグ付き結果になる() {
        let mut exec = |_: Request| -> Result<Value, String> {
            Err("ペイン 9 が見つからない".into())
        };
        let mut session = McpSession {
            caller_pane: None,
            caller_role: None,
            connected: true,
            exec: &mut exec,
        };
        let response = handle_message(&call("tako_list_panes", json!({})), &mut session).unwrap();
        let result = &response["result"];
        assert_eq!(result["isError"], true);
        assert!(result["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("見つからない"));
    }

    #[test]
    fn 不明なツールと未対応メソッドはエラー() {
        let (response, _) = run(call("tako_explode", json!({})), None, true);
        assert_eq!(response.unwrap()["error"]["code"], -32602);
        let (response, _) = run(
            json!({ "jsonrpc": "2.0", "id": 1, "method": "resources/list" }),
            None,
            true,
        );
        assert_eq!(response.unwrap()["error"]["code"], -32601);
    }

    #[test]
    fn pin_previewはペインまたはグループタブをトグルする() {
        // pane 指定（呼び出し元フォールバック）
        let (_, requests) = run(
            call("tako_pin_preview", json!({ "pinned": true })),
            Some(5),
            true,
        );
        assert_eq!(
            requests,
            vec![Request::Pin {
                pane: Some(5),
                group_tab: None,
                pinned: Some(true),
            }]
        );
        // group_tab 指定時は pane を補完しない（排他）
        let (_, requests) = run(
            call("tako_pin_preview", json!({ "group_tab": 2 })),
            Some(5),
            true,
        );
        assert_eq!(
            requests,
            vec![Request::Pin {
                pane: None,
                group_tab: Some(2),
                pinned: None,
            }]
        );
        // 両方省略 = 呼び出し元ペインでトグル
        let (_, requests) = run(call("tako_pin_preview", json!({})), Some(5), true);
        assert_eq!(
            requests,
            vec![Request::Pin {
                pane: Some(5),
                group_tab: None,
                pinned: None,
            }]
        );
        // pinned に不正な型を渡すとエラー
        let (response, requests) = run(
            call("tako_pin_preview", json!({ "pinned": "yes" })),
            Some(5),
            true,
        );
        assert!(requests.is_empty());
        assert!(response.unwrap()["error"]["message"]
            .as_str()
            .unwrap()
            .contains("pinned"));
    }

    #[test]
    fn video_playbackはaction必須でペインへフォールバックする() {
        let (_, requests) = run(
            call("tako_video_playback", json!({ "action": "toggle" })),
            Some(3),
            true,
        );
        assert_eq!(
            requests,
            vec![Request::VideoPlayback {
                pane: Some(3),
                action: "toggle".into(),
            }]
        );
        // pane 明示指定
        let (_, requests) = run(
            call(
                "tako_video_playback",
                json!({ "pane": 10, "action": "play" }),
            ),
            Some(3),
            true,
        );
        assert_eq!(
            requests,
            vec![Request::VideoPlayback {
                pane: Some(10),
                action: "play".into(),
            }]
        );
        // action 欠落はエラー
        let (response, requests) = run(call("tako_video_playback", json!({})), Some(3), true);
        assert!(requests.is_empty());
        assert!(response.unwrap()["error"]["message"]
            .as_str()
            .unwrap()
            .contains("action"));
        // 呼び出し元なし + pane 省略もエラー
        let (response, requests) = run(
            call("tako_video_playback", json!({ "action": "pause" })),
            None,
            true,
        );
        assert!(requests.is_empty());
        assert!(response.unwrap()["error"]["message"]
            .as_str()
            .unwrap()
            .contains("pane"));
    }

    #[test]
    fn video_seekはseconds必須でペインへフォールバックする() {
        let (_, requests) = run(
            call("tako_video_seek", json!({ "seconds": 42.5 })),
            Some(3),
            true,
        );
        assert_eq!(
            requests,
            vec![Request::VideoSeek {
                pane: Some(3),
                seconds: 42.5,
            }]
        );
        // seconds 欠落はエラー
        let (response, requests) = run(call("tako_video_seek", json!({})), Some(3), true);
        assert!(requests.is_empty());
        assert!(response.unwrap()["error"]["message"]
            .as_str()
            .unwrap()
            .contains("seconds"));
        // seconds に負値（スキーマでは minimum: 0 だが、f64_arg は型のみ検証。
        // ここではパース層が通ることを確認。意味検証は dispatch 側の責務）
        let (_, requests) = run(
            call("tako_video_seek", json!({ "seconds": 0.0 })),
            Some(3),
            true,
        );
        assert_eq!(
            requests,
            vec![Request::VideoSeek {
                pane: Some(3),
                seconds: 0.0,
            }]
        );
        // seconds に文字列を渡すとエラー
        let (response, requests) = run(
            call("tako_video_seek", json!({ "seconds": "ten" })),
            Some(3),
            true,
        );
        assert!(requests.is_empty());
        assert!(response.unwrap()["error"]["message"]
            .as_str()
            .unwrap()
            .contains("seconds"));
    }

    #[test]
    fn video_volumeはvolume必須でペインへフォールバックする() {
        let (_, requests) = run(
            call("tako_video_volume", json!({ "volume": 0.5 })),
            Some(3),
            true,
        );
        assert_eq!(
            requests,
            vec![Request::VideoVolume {
                pane: Some(3),
                volume: 0.5,
            }]
        );
        let (response, requests) = run(call("tako_video_volume", json!({})), Some(3), true);
        assert!(requests.is_empty());
        assert!(response.unwrap()["error"]["message"]
            .as_str()
            .unwrap()
            .contains("volume"));
    }

    #[test]
    fn video_playbackのmute_loop操作がパースできる() {
        for action in &[
            "mute",
            "unmute",
            "toggle_mute",
            "loop_on",
            "loop_off",
            "toggle_loop",
        ] {
            let (_, requests) = run(
                call("tako_video_playback", json!({ "action": action })),
                Some(3),
                true,
            );
            assert_eq!(
                requests,
                vec![Request::VideoPlayback {
                    pane: Some(3),
                    action: action.to_string(),
                }]
            );
        }
    }

    #[test]
    fn webはactionごとにcaller既定を使い分ける() {
        // open: pane 省略 → caller が分割元になる
        let (_, requests) = run(
            call(
                "tako_web",
                json!({ "action": "open", "url": "http://localhost:3000" }),
            ),
            Some(5),
            true,
        );
        assert_eq!(
            requests,
            vec![Request::Web {
                action: "open".into(),
                url: Some("http://localhost:3000".into()),
                id: None,
                pane: Some(5),
                direction: None,
                to: None,
                js: None,
                token: None,
                focus: None,
            }]
        );
        // navigate: pane 省略でも caller を埋めない（対象は表示中 Web ビューの自動解決）
        let (_, requests) = run(
            call("tako_web", json!({ "action": "navigate", "to": "reload" })),
            Some(5),
            true,
        );
        assert_eq!(
            requests,
            vec![Request::Web {
                action: "navigate".into(),
                url: None,
                id: None,
                pane: None,
                direction: None,
                to: Some("reload".into()),
                js: None,
                token: None,
                focus: None,
            }]
        );
        // action 欠落はエラー
        let (response, requests) = run(call("tako_web", json!({})), Some(5), true);
        assert!(requests.is_empty());
        assert!(response.unwrap()["error"]["message"]
            .as_str()
            .unwrap()
            .contains("action"));
        // 不正な direction はエラー
        let (response, requests) = run(
            call(
                "tako_web",
                json!({ "action": "open", "url": "http://localhost:3000", "direction": "diagonal" }),
            ),
            Some(5),
            true,
        );
        assert!(requests.is_empty());
        assert!(response.unwrap()["error"]["message"]
            .as_str()
            .unwrap()
            .contains("direction"));
    }

    /// #1745: stdio ブリッジ（`tako mcp serve`）は受け口のチャネルを持たない。
    /// 非同期 run が exec（= IPC）へ開始要求を 1 件渡すだけなら HTTP と同じ答えになる。
    /// #1745 まではここで JSON-RPC エラー（`sync=true` を指定してください）が返っていた
    #[test]
    fn orchestrator_runの非同期は開始要求をexecへ渡すだけ() {
        let mut seen = Vec::new();
        let response = {
            let mut exec = |request: Request| -> Result<Value, String> {
                seen.push(request);
                Ok(json!({ "run_id": "run-9", "pane_id": 42, "spawned_by": 7, "tmux_session": null }))
            };
            let mut session = McpSession {
                caller_pane: Some(7),
                caller_role: Some("master:t".into()),
                connected: true,
                exec: &mut exec,
            };
            let message = call(
                "tako_orchestrator_run",
                json!({ "project": "demo", "prompt": "やって", "label": "l" }),
            );
            handle_message(&message, &mut session).expect("応答がある")
        };
        assert!(
            response.get("error").is_none(),
            "非同期 run が JSON-RPC エラーになった: {response}"
        );
        assert_eq!(response["result"]["isError"], false);
        let text = response["result"]["content"][0]["text"].as_str().unwrap();
        let body: Value = serde_json::from_str(text).unwrap();
        assert_eq!(body["run_id"], "run-9");

        assert_eq!(
            seen.len(),
            1,
            "exec へ渡すのは開始要求 1 件だけ（spawn も完了待ちも受け口側）: {seen:?}"
        );
        let opts = wait::RunOptions::from_start_request(&seen[0])
            .unwrap_or_else(|| panic!("開始要求ではない: {:?}", seen[0]));
        assert_eq!(opts.project, "demo");
        assert_eq!(opts.label.as_deref(), Some("l"));
        assert_eq!(opts.pane, Some(7), "pane 省略は呼び出し元");
        assert_eq!(opts.caller_role.as_deref(), Some("master:t"));
        assert_eq!(opts.timeout, std::time::Duration::from_secs(1800));
        assert!(opts.auto_close);
        assert_eq!(opts.output_lines, 200);
        assert_eq!(opts.initial_delay, wait::RUN_INITIAL_DELAY);
        assert_eq!(opts.interval, wait::RUN_INTERVAL);
    }

    /// sync=true は従来どおり MCP 側で spawn から合成する（開始要求は使わない）
    #[test]
    fn orchestrator_runのsyncはspawnから合成する() {
        let mut seen = Vec::new();
        let response = {
            // spawn で断れば完了待ち（20 秒の初期待機）へ進まずに返る
            let mut exec = |request: Request| -> Result<Value, String> {
                seen.push(request);
                Err("spawn できない".into())
            };
            let mut session = McpSession {
                caller_pane: Some(7),
                caller_role: None,
                connected: true,
                exec: &mut exec,
            };
            let message = call(
                "tako_orchestrator_run",
                json!({ "project": "demo", "prompt": "やって", "sync": true }),
            );
            handle_message(&message, &mut session).expect("応答がある")
        };
        assert!(
            matches!(seen.as_slice(), [Request::OrchestratorSpawn { .. }]),
            "{seen:?}"
        );
        assert_eq!(response["error"]["message"], "spawn できない");
    }

    #[test]
    fn orchestrator_spawnのpaneとtab優先順位() {
        // pane のみ → pane が使われ tab は None
        let (_, requests) = run(
            call(
                "tako_orchestrator_spawn",
                json!({ "project": "p", "prompt": "hi", "pane": 5 }),
            ),
            Some(99),
            true,
        );
        assert_eq!(requests.len(), 1);
        match &requests[0] {
            Request::OrchestratorSpawn { pane, tab, .. } => {
                assert_eq!(*pane, Some(5));
                assert_eq!(*tab, None);
            }
            other => panic!("unexpected: {other:?}"),
        }

        // tab のみ → tab が使われ pane は None（caller もフォールバックしない）
        let (_, requests) = run(
            call(
                "tako_orchestrator_spawn",
                json!({ "project": "p", "prompt": "hi", "tab": 2 }),
            ),
            Some(99),
            true,
        );
        match &requests[0] {
            Request::OrchestratorSpawn { pane, tab, .. } => {
                assert_eq!(*pane, None);
                assert_eq!(*tab, Some(2));
            }
            other => panic!("unexpected: {other:?}"),
        }

        // pane と tab 両方 → pane 優先、tab は None
        let (_, requests) = run(
            call(
                "tako_orchestrator_spawn",
                json!({ "project": "p", "prompt": "hi", "pane": 5, "tab": 2 }),
            ),
            Some(99),
            true,
        );
        match &requests[0] {
            Request::OrchestratorSpawn { pane, tab, .. } => {
                assert_eq!(*pane, Some(5), "pane が tab より優先される");
                assert_eq!(*tab, None, "pane 指定時は tab を無視する");
            }
            other => panic!("unexpected: {other:?}"),
        }

        // 両方省略、caller あり → caller がフォールバック
        let (_, requests) = run(
            call(
                "tako_orchestrator_spawn",
                json!({ "project": "p", "prompt": "hi" }),
            ),
            Some(42),
            true,
        );
        match &requests[0] {
            Request::OrchestratorSpawn { pane, tab, .. } => {
                assert_eq!(*pane, Some(42), "caller へフォールバック");
                assert_eq!(*tab, None);
            }
            other => panic!("unexpected: {other:?}"),
        }

        // 両方省略、caller なし → エラー
        let (response, requests) = run(
            call(
                "tako_orchestrator_spawn",
                json!({ "project": "p", "prompt": "hi" }),
            ),
            None,
            true,
        );
        assert!(requests.is_empty());
        let error = &response.unwrap()["error"];
        assert!(
            error["message"]
                .as_str()
                .unwrap()
                .contains("pane または tab"),
            "pane も tab も無い場合はエラー"
        );
    }

    // --- HTTP トランスポート（実ポートで往復） ---

    mod http {
        use super::*;
        use crate::ipc::IncomingRequest;
        use futures::channel::mpsc::unbounded;
        use futures::StreamExt;
        use std::io::{Read, Write};
        use tako_core::PaneOrigin;

        const TOKEN: &str = "http-test-token";

        /// サーバー + ダミーディスパッチャ（list に固定値を返す）を立てる
        fn start_server() -> McpServer {
            let (tx, mut rx) = unbounded::<IncomingRequest>();
            let server = McpServer::start(tx, TOKEN.into()).expect("MCP サーバーを起動できる");
            std::thread::spawn(move || {
                while let Some(incoming) = futures::executor::block_on(rx.next()) {
                    assert_eq!(incoming.origin, PaneOrigin::Mcp);
                    let _ = incoming.reply.send(Ok(json!({ "tabs": [] })));
                }
            });
            server
        }

        fn post(
            url: &str,
            auth: Option<&str>,
            extra_headers: &[(&str, &str)],
            body: &str,
        ) -> (u16, String) {
            let rest = url.strip_prefix("http://").expect("テスト URL は http");
            let (hostport, path) = rest.split_once('/').expect("URL にパスがある");
            let mut stream = std::net::TcpStream::connect(hostport).expect("接続できる");
            let mut request = format!(
                "POST /{path} HTTP/1.1\r\nHost: {hostport}\r\nContent-Type: application/json\r\n\
                 Accept: application/json, text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n",
                body.len()
            );
            if let Some(token) = auth {
                request.push_str(&format!("Authorization: Bearer {token}\r\n"));
            }
            for (name, value) in extra_headers {
                request.push_str(&format!("{name}: {value}\r\n"));
            }
            request.push_str("\r\n");
            request.push_str(body);
            stream.write_all(request.as_bytes()).expect("送信できる");
            let mut response = String::new();
            stream.read_to_string(&mut response).expect("受信できる");
            let status = response
                .split_whitespace()
                .nth(1)
                .and_then(|s| s.parse().ok())
                .expect("ステータス行がある");
            let body = response
                .split_once("\r\n\r\n")
                .map(|(_, b)| b.to_string())
                .unwrap_or_default();
            (status, body)
        }

        #[test]
        fn 認証付きでツール呼び出しが往復する() {
            let server = start_server();
            let body = call("tako_list_panes", json!({})).to_string();
            let (status, response) = post(server.url(), Some(TOKEN), &[], &body);
            assert_eq!(status, 200);
            let response: Value = serde_json::from_str(&response).unwrap();
            assert_eq!(response["result"]["isError"], false);
            assert!(response["result"]["content"][0]["text"]
                .as_str()
                .unwrap()
                .contains("tabs"));
        }

        #[test]
        fn 不正トークンと不正オリジンは拒否される() {
            let server = start_server();
            let body = call("tako_list_panes", json!({})).to_string();
            let (status, _) = post(server.url(), Some("bogus"), &[], &body);
            assert_eq!(status, 401);
            let (status, _) = post(server.url(), None, &[], &body);
            assert_eq!(status, 401);
            let (status, _) = post(
                server.url(),
                Some(TOKEN),
                &[("Origin", "http://evil.example")],
                &body,
            );
            assert_eq!(status, 403);
        }

        #[test]
        fn tools_listはhttp経由で全カタログを返す() {
            // 50 ツール（日本語説明文込みで数十 KB）の大きな応答が HTTP 層で
            // 欠けずに返ることを検証する（セルフテスト項目 32 のユニット版）
            let server = start_server();
            let body = r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#;
            let (status, response) = post(server.url(), Some(TOKEN), &[], body);
            assert_eq!(status, 200);
            let response: Value = serde_json::from_str(&response).unwrap();
            assert_eq!(
                response["result"]["tools"].as_array().unwrap().len(),
                tools().len()
            );
        }

        #[test]
        fn notificationは202になる() {
            let server = start_server();
            let body = json!({ "jsonrpc": "2.0", "method": "notifications/initialized" });
            let (status, _) = post(server.url(), Some(TOKEN), &[], &body.to_string());
            assert_eq!(status, 202);
        }

        #[test]
        fn 呼び出し元ペインはヘッダで申告できる() {
            let (tx, mut rx) = unbounded::<IncomingRequest>();
            let server = McpServer::start(tx, TOKEN.into()).unwrap();
            std::thread::spawn(move || {
                while let Some(incoming) = futures::executor::block_on(rx.next()) {
                    // X-Tako-Pane がデフォルト対象として解決されている（FR-2.3.3）
                    assert_eq!(
                        incoming.request,
                        Request::Close {
                            pane: Some(42),
                            force: false,
                            caller_role: None,
                        },
                        "X-Tako-Pane が呼び出し元として使われる"
                    );
                    let _ = incoming.reply.send(Ok(json!({ "closed": 42 })));
                }
            });
            let body = call("tako_close_pane", json!({})).to_string();
            let (status, response) =
                post(server.url(), Some(TOKEN), &[("X-Tako-Pane", "42")], &body);
            assert_eq!(status, 200);
            let response: Value = serde_json::from_str(&response).unwrap();
            assert_eq!(response["result"]["isError"], false);
        }

        #[test]
        fn 遅いdispatch中も並行リクエストがブロックされない() {
            let (tx, mut rx) = unbounded::<IncomingRequest>();
            let server = McpServer::start(tx, TOKEN.into()).unwrap();
            // dispatch ハンドラ: 重い dispatch は別スレッドへ offload（実 app の
            // OffloadJob と同じパターン。UI スレッドは即座に次のリクエストへ進む）
            std::thread::spawn(move || {
                while let Some(incoming) = futures::executor::block_on(rx.next()) {
                    match &incoming.request {
                        Request::Read { .. } => {
                            std::thread::spawn(move || {
                                std::thread::sleep(std::time::Duration::from_millis(500));
                                let _ = incoming.reply.send(Ok(json!({ "slow": true })));
                            });
                        }
                        _ => {
                            let _ = incoming.reply.send(Ok(json!({ "tabs": [] })));
                        }
                    }
                }
            });
            let url = server.url().to_string();
            // 遅い read_pane を先に投げる
            let url_slow = url.clone();
            let slow = std::thread::spawn(move || {
                let body = call("tako_read_pane", json!({"pane": 1})).to_string();
                let start = std::time::Instant::now();
                let (status, _) = post(&url_slow, Some(TOKEN), &[], &body);
                (status, start.elapsed())
            });
            // 少し待ってから高速な list_panes を投げる
            std::thread::sleep(std::time::Duration::from_millis(50));
            let url_fast = url.clone();
            let fast = std::thread::spawn(move || {
                let body = call("tako_list_panes", json!({})).to_string();
                let start = std::time::Instant::now();
                let (status, _) = post(&url_fast, Some(TOKEN), &[], &body);
                (status, start.elapsed())
            });
            let (slow_status, slow_elapsed) = slow.join().unwrap();
            let (fast_status, fast_elapsed) = fast.join().unwrap();
            assert_eq!(slow_status, 200);
            assert_eq!(fast_status, 200);
            // 並行化されていれば fast は slow を待たず 200ms 以内に返る
            // （直列なら slow の 500ms 完了後にしか処理されない）
            assert!(
                fast_elapsed < std::time::Duration::from_millis(200),
                "list_panes が read_pane の完了を待ってしまった（{:?}、並行化されていない）",
                fast_elapsed,
            );
            assert!(slow_elapsed >= std::time::Duration::from_millis(400));
        }
    }

    #[test]
    fn 未知パラメータはエラーになる_spawn() {
        let msg = call(
            "tako_orchestrator_spawn",
            json!({ "project": "p", "prompt": "hi", "agentt": "codex" }),
        );
        let (resp, _) = run(msg, Some(0), true);
        let err = &resp.unwrap()["error"];
        assert_eq!(err["code"], -32602);
        let msg = err["message"].as_str().unwrap();
        assert!(msg.contains("agentt"), "エラーに未知キー名を含む: {msg}");
        assert!(
            msg.contains("tako_orchestrator_spawn"),
            "エラーにツール名を含む: {msg}"
        );
    }

    #[test]
    fn 未知パラメータはエラーになる_list_panes() {
        let msg = call("tako_list_panes", json!({ "foo": "bar" }));
        let (resp, _) = run(msg, Some(0), true);
        let err = &resp.unwrap()["error"];
        assert_eq!(err["code"], -32602);
        let msg = err["message"].as_str().unwrap();
        assert!(msg.contains("foo"), "エラーに未知キー名を含む: {msg}");
    }

    /// #1154: 手順書は MCP の `tools/call` から引ける。プロファイルは
    /// 呼び出し元の role から解決するので、master は自分の閾値が焼かれた本文を受け取る
    #[test]
    fn 手順書はtools_callから引ける() {
        let mut seen = Vec::new();
        let mut exec = |request: Request| -> Result<Value, String> {
            seen.push(request);
            Ok(json!({ "topic": "monitoring" }))
        };
        let mut session = McpSession {
            caller_pane: Some(3),
            caller_role: Some("master:takodev".into()),
            connected: true,
            exec: &mut exec,
        };
        let response = handle_message(
            &call("tako_orchestrator_guide", json!({ "topic": "monitoring" })),
            &mut session,
        );
        assert!(response.as_ref().unwrap().get("error").is_none(), "{response:?}");
        assert!(
            matches!(
                &seen[0],
                Request::OrchestratorGuide { topic, profile, caller_role, .. }
                    if topic.as_deref() == Some("monitoring")
                        && profile.is_none()
                        && caller_role.as_deref() == Some("master:takodev")
            ),
            "role が dispatch へ渡っていない: {:?}",
            seen[0]
        );
    }

    #[test]
    fn 正規パラメータはエラーにならない_spawn() {
        let msg = call(
            "tako_orchestrator_spawn",
            json!({ "project": "p", "prompt": "hi", "agent": "codex", "pane": 0 }),
        );
        let (resp, _) = run(msg, Some(0), true);
        assert!(
            resp.as_ref().unwrap().get("error").is_none(),
            "正規パラメータでエラー: {:?}",
            resp
        );
    }

    /// #1959: `tako_mod` の install / uninstall は `dry_run` つきの要求になり、report は断る
    #[test]
    fn tako_mod_の_install_は_dry_run_つきの要求になる() {
        assert_eq!(
            build_request(
                "tako_mod",
                &json!({"action": "install", "dry_run": true}),
                None,
                None
            )
            .unwrap(),
            Request::Mod {
                action: Some("install".into()),
                report: None,
                pane: None,
                ui: None,
                dry_run: true,
            }
        );
        assert_eq!(
            build_request("tako_mod", &json!({"action": "uninstall"}), None, None).unwrap(),
            Request::Mod {
                action: Some("uninstall".into()),
                report: None,
                pane: None,
                ui: None,
                dry_run: false,
            }
        );
        assert!(build_request("tako_mod", &json!({"action": "report"}), None, None).is_err());
        assert!(
            validate_known_params("tako_mod", &json!({"action": "install", "dry_run": true}))
                .is_ok()
        );
    }
}
