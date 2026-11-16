# Progress Log

> AI が作業完了時に**末尾へ追記**する時系列ログ。新しいものほど下。
> **1 エントリは 1〜3 行**（何を / どこを / 結果）。詳細は git log・Issue・PR・`.agent/plans/` に委ねる。

このファイルは `AGENTS.md` から `@import` されるので**毎ターン全文が読み込まれる**。
予算（直近 5 作業日 / 20 エントリ / 12 KB）を超えたぶんは `progress-archive.md` へ
1 行で移る。移送は `tako context-budget fix` が行う（冪等・本文は改変しない・
全文は git 履歴に残る）。規約の全文は `AGENTS.md`「起動時ロードの予算」節。

## 追記フォーマット

```markdown

## YYYY-MM-DD（#Issue 一言）
- {何を / どこを / 結果}
- 関連コミット: `{shortsha}` `[種別] 概要`
- 次: {次にやることがあれば 1 行}
```

---

## 2026-11-11（#2906: claude の MCP の書き先を claude CLI と同じ `$CLAUDE_CONFIG_DIR/.claude.json` へ（正本 `tako_core::claude_config` + 口 `agent_mcp::claude_user_config_json` に 7 か所を寄せた・検証プロセスは一時 dir の外の値を見ない・番犬 `issue2906_*`））
- 砂場の A/B: setup-mcp の 2 回目が「追加しました」→「既に設定」、死んだ登録の `setup --check` が楽観 → 警告 + 修復、直書きが `~/.claude.json` → `~/.claude-alt/.claude.json`・カタログ +0 B・範囲外は #2912。PR #2914

## 2026-11-11（#2907: master の system prompt の Worker Model Policy 節・Available Worker Agents・Session Identity と `tako master` の起動の行の worker の値を spawn と同じ 1 実装（`Profile::default_worker_launch` = `plan_worker_launch_typed`）から組む。言えないときは頭の 1 行と出どころの括弧・未登録のアカウントは理由を出して起動は止めない。番犬 `issue2907_prompt_worker_policy_values`・カタログ ±0 B）
- 実 CLI の A/B で Issue の例の起動の行・prompt が high → medium（Available Worker Agents と一致）・上の段なしはバイト一致・注入 3 か所で名指し。PR #2916

## 2026-11-11（#2860: 子プロセスのグループ化・打ち切り・終了シグナル・daemon 化の 16 塊を境界 B5 の `platform::process` へトークン一致で移設（tako-core 6 口・tako-control 4 口））
- 集計器で境界の外 302 → 286、番犬 `platform_parity` の直呼びの件数 + #2633 / #1533 / tmux_named_cleanup の走査先の追従。移設前後のバイナリで打ち切り・終了シグナル・転送・daemon の停止が一致・カタログ +0 B。PR #2917

## 2026-11-11（#2915: worker の `[1m]` 警告（`tako master` の起動・`profiles` の warnings）を spawn と同じ 1 実装 `default_worker_launch` の model で判定する `worker_one_m_model_note` へ寄せた。claude の語彙の系統だけ・spawn が断る状態と solo では言わない。番犬 `issue2915_one_m_warning_actual_model`・カタログ ±0 B）
- 実 CLI の A/B で上の段・アカウントの `[1m]` は黙る → 鳴る、ポリシーの段だけ / codex / 未登録のアカウントは鳴る → 黙る。注入 2 通りで名指し。範囲外の解除の案内は #2918。PR #2919

## 2026-11-11（#2920: issue2826 のバイト一致テストのシェルが印（OSC 133;A）を `stty raw` の前に出しており、ICRNL の窓で CI の macOS が間欠的に落ちた → 印を stty の後へ。番犬 `issue2920_ready_mark_before_stty_watchdog`（同じ行で合図が stty より前を名指す）・規約 conventions.md）
- 注入 `sleep 0.3` で修正前は `"echo hi\n"` で落ち・修正後は `"echo hi\r"` で通る・25 回連続 25/25・同じ型は他に無し・カタログ +0 B。PR #2922

## 2026-11-11（#2861: ファイルの同一性・状態変更時刻・実行ビット・symlink・置き換え保存の 17 塊を新設の境界 B37 `tako_core::platform::fs_meta` へトークン一致で移設（Windows の縮退は `Option` / 定数で型に出し、呼び手は `None` を 0 で埋める = 判定は不変））
- 集計器で境界の外 286 → 269、番犬 `platform_parity` の直呼びの件数。移設前後のバイナリで同一性・実行ビット・symlink・置き換え保存の結果が一致・カタログ +0 B。Windows の腕は #2921。PR #2923

## 2026-11-11（#2902: setup の --answers の launch_agent の説明を今の挙動（--yes と同じ非対話で値によらず対話を開かない・none は互換で受理）へ。番犬 answersのlaunch_agentの説明が対話を開かない挙動と一致する）
- 注入 3 通りで file:line 名指し・砂場の PTY で素の setup は開き --answers は 5 通りとも開かない・カタログ +0 B。PR #2924

## 2026-11-11（#2918: `[1m]` 警告の解除の案内を、モデルを決めた段（master / solo の model・ポリシーの段・`worker_agents.claude.model`・アカウントの default_model）から `OneMClear::command` で組む。プロファイル名入りのそのまま打てる形・solo は `--solo`・アカウントは他の欄を付け直して登録し直す）
- 砂場の A/B で修正前は 4 通りが打っても残る（solo は同名の master の model を外していた）→ 修正後は 5 通りとも消える。番犬 `issue2918_one_m_clear_hint` は注入と修正前のバイナリで名指して落ちる・カタログ ±0 B。PR #2926

## 2026-11-16（#2921: 境界 B37 fs_meta の Windows の腕に同一性（FileIdInfo のボリュームのシリアル + ファイル ID）・状態変更時刻（FileBasicInfo の ChangeTime）・置き換え保存（一時ファイル + ReplaceFileW、置き換えられないときは #2921 前のその場の上書きへ倒す）を実装）
- A/B `TAKO_2921_LEGACY=1`・CI の Windows に名指しの段 2 本・usage の 0 の行は 1 回読み直すだけで数は不変・カタログ +0 B。PR #2927

## 2026-11-16（#2751: 狭いペイン（幅 240px）で言語サーバの「未導入」の帯が高さいっぱいに折り返して本文 0 行 → 帯の文の器に上限（ペインの高さの 0.33 まで・本文が半分を下回らない範囲・`pane_height_of_area` でペイン全体の高さへ戻す）+ 器の中のスクロール・ボタンの行は器の外。A/B `TAKO_2751_LEGACY=1`・番犬 `issue2751_lsp_band_cap_watchdog`・visual-test `lsp-band-2751`）
- tako-vd で旧 0 行 → 新 10 行・221.5px（半分 207.5px）・広いペインの画素の差 0・カタログ +0 B。PR #2932

## 2026-11-16（#2929: Windows で起動のたびにサイドバーへ「リモートを自動で起動できません（#971）」が赤字で出ていた → `autostart_decision` を意図 → OS の順へ。記録が無ければ未対応 OS でも `not-desired` で黙る・記録ありは今どおり知らせる）
- A/B `TAKO_2929_LEGACY=1`・番犬 `issue2929_remote_autostart_quiet_watchdog`（実経路の probe は Windows の CI で本物の未対応 OS を引く）・カタログ +0 B。PR #2933

## 2026-11-16（#2931: Windows の `tako setup` の MCP 登録先が PATH の別の tako.exe になっていた → `resolve_tako_binary` の ① の次に「実行中の本体がインストーラーで入れた先（境界 B20 の記録）に居れば隣」を置いた。判定は #2434 と同じ `install_info::runs_from_install_dir` の 1 実装・開発ビルド / zip / macOS は不変・A/B `TAKO_2931_LEGACY=1`・番犬 `issue2931_installed_sibling_first_watchdog`）
- 注入 2 通り（順序を戻す・記録を渡さない）で名指し・macOS の砂場の setup-mcp は修正前後で同じ登録先・カタログ +0 B・Windows 実機の確認待ちで Refs。PR #2934

## 2026-11-16（#2429 / #2430: Windows の Web ビューを見えるように（gpui の DirectComposition を起動の間だけ切る・副作用は実機で差なし）・ページの後の打鍵をターミナルへ戻す（Win32 の宛先を読んで SetFocus））
- 境界 B11 `platform/webview_host.rs` を新設・項目 71 / 155 の Windows スキップを外した・`tako_web` を Degraded へ。起票 #2948 / #2949。PR #2938

## 2026-11-16（#2930: PWA の依存から使っていない `geist`（Next.js 用のフォントローダー・peer に next）を外した。Node 24 の npm 11.9 以降は next の任意依存が lock に無いことを EUSAGE で断り、build.rs の自動ビルドごと cargo build が止まっていた。フォントは public/fonts/ に同梱済み・dist は全ファイル一致・node_modules 247 → 60 MB）
- npm 10.9.9 / 10.9.2 / 11.6 / 11.9 / 11.21 で npm ci + build が通る・build.rs の A/B（npm 11.9）は exit 101 → 0・e2e 124 passed は前後同じ・CI の macOS ジョブへ npm 11.21.1 の `ci --dry-run`。範囲外は #2943。PR #2944

## 2026-11-16（#2936: GUI の出力マクロを #1758 / #1805 の差し替えへ載せ、main の 2 文目で GUI の規則（書き込みの失敗を種類によらず捨てる）を採る。書けない標準エラー（閉じたパイプ・読み取り専用のハンドル = Windows の ERROR_ACCESS_DENIED）で落ちない・書けた出力は新旧で同じ・A/B `TAKO_2936_LEGACY=1`・番犬 `issue2936_gui_stdio_watchdog`）
- 隔離 GUI の実経路: 旧は close の記録と終了処理の最初の行で panic（器が孤児で残る）→ 新は走り切る。副産物で #2937（`tako web open` の wry の abort）を起票。PR #2945

## 2026-11-16（#2454 / #2462 / #1886 / #937: バッチ B2 の Windows 実機確認）
- #2454・#2462 を実機の A/B で確かめて閉じた。#1886 は claude 2.1.263 が mod の下限未満で GUI の通しが止まり開けたまま。#937 の未実測 5 件を判定し直した（未実測 0）。起票 #2939〜#2942。PR #2946

## 2026-11-16（#2935: Windows の器（psmux）へ再 attach したペインの dev サーバーのポートを検知しなかった → 器のペインはキーの根を器の中のシェル（`pane_facts` の `#{pane_pid}` を端末へ写す・器への問い合わせは増やさない）に）
- 判定は純関数 `procinfo::pid_tree_root`・A/B `TAKO_2935_LEGACY=1`・番犬 `issue2935_container_port_root_watchdog`。PR #2947

## 2026-11-16（#2484: psmux の器の conf に能力で `warm off`・conf の正本を 1 つに・再適用は器ごとの `-t` + 待機サーバーの片付け・クライアントは backend の解決・素のペインのシェルは `--` で包みの pwsh を挟まない。A/B `TAKO_2484_LEGACY=1`）
- 実機（器 2 本）: pwsh 8 → 2 本・private 294 → 78 MB・修正前の器への再 attach で 8 → 4 本。起票 #2954（器のペインの PATH はレジストリ）。PR #2950

## 2026-11-16（#2940: Windows の検証起動の GUI が本番の固定パイプ名を取っていた → 検証プロセスは data dir ごとの名前 `\\.\pipe\tako-<ユーザー>.<16 桁>`）
- 決め方は `ipc_socket::plan_pipe_name` の純関数・A/B `TAKO_2940_LEGACY=1`・番犬 `issue2940_verification_pipe_name_watchdog`・実経路 `issue2940_isolated_pipe_name` を Windows CI で名指し。PR #2952

## 2026-11-16（#2937: `tako web open 'javascript:…'` が正規化で `https://javascript:…` になり wry の NSURL の unwrap で GUI ごと abort → 検査を `tako_control::web_url` の 1 実装（形・scheme の表・境界 B38 = macOS は wry と同じ NSURL を先に呼ぶ）へ寄せ、全入口が wry へ渡す 2 か所の直前で通る）
- dock / アドレスバーは理由をその場に出す。A/B `TAKO_2937_LEGACY=1`・番犬 `issue2937_webview_url_guard_watchdog`。PR #2953
