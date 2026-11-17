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

## 2026-11-16（#2942: `tako setup bootstrap status` / `status-all`・MCP `tako_setup_bootstrap`・`tako setup` の検出と導入の段の認証確認を環境診断と同じ上限つき（`auth_state_bounded`）にし、打ち切りを `auth_unconfirmed` と「認証: 不明（N 秒で応答なし）」で知らせる。門番 `agent_probe::output_within` は `probe::output_with_timeout_grouped` で孫まで止める）
- A/B `TAKO_2942_LEGACY=1`・番犬 `issue2942_bootstrap_auth_probe_watchdog`。実経路 `scripts/test-bootstrap-auth-timeout-2942.sh` で修正前 30 秒無言 → 修正後 上限の秒数で理由つき・孫 0 件（PASS 75）・カタログ +0 B。起票 #2951 / #2956。PR #2955

## 2026-11-16（#2943: docs の「ビルド方法」の前提条件に Node.js（npm）（LTS・22 / 24 系で確認済み）と、clone 直後の最初の cargo build が build.rs で PWA を自動ビルドする注記を足し、`.agent/windows-setup.md` の「3. Git と Node.js」を同じ言い方へ。英語版のページは無い（docs は ja のみ））
- docs のビルド + CI と同じ検査一式が緑・描画を読み戻して確認。PR #2958

## 2026-11-16（#2929 / #2931 / #2935 / #2940: Windows 実機のバッチ C（ssh の `win`・origin/main `a1b91bb` をインストーラーで入れた本体・隔離インスタンス）で元の症状が出なくなったのを確かめて閉じた。#2936 / #2937 は閉じてある Issue へ実機の確認のコメントだけ）
- 修正後と `TAKO_<N>_LEGACY=1` の A/B（#2929 サイドバーの赤字・#2931 MCP の登録先・#2935 再 attach したペインの `listen_ports`・#2940 control.json の socket）。副作用: 砂場の setup が本物のレジストリの Path へ書いた → 写しから戻して #2959 を起票・検証プロセスでない GUI が `$PROFILE` の移行を当てた → 写しから戻した。PR なし

## 2026-11-17（#2959: Windows の検証プロセス（`TAKO_ISOLATED` ほか）の `tako setup` / `bootstrap path` / `undo-path` / `setup undo` が本物のユーザー環境変数 Path を読み書きしないように。門は境界 B23 の `user_path::skip_for_verification`（`is_verification_process` の 1 実装）・飛ばしたら理由と置き場を日英で言い setup は PATH を残り作業に残す・`write_typed` も門で断る）
- A/B `TAKO_2959_LEGACY=1`・番犬 `issue2959_user_path_verification_watchdog`。Windows CI の setup の一巡に段 7（検証プロセスで Path の生の値が不変・旧アームは砂場の `.local\bin` を足す）・本番の腕は #2959 前の JSON とバイト一致・カタログ +0 B・実機の確認待ちで Refs。PR #2962

## 2026-11-17（#2956: MCP `tako_setup_bootstrap` / `tako_setup_models` を `prepare_offload` → `OffloadJob::SetupBootstrap` / `SetupModels` の background で答え、UI スレッドで外部 CLI を待たない（同期の腕と 1 実装・書き込みは錠で 1 本ずつ）。番犬 `issue2956_setup_dispatch_offload_watchdog` が dispatch の同期の腕から #1984 の葉へ届く形を名指す）
- A/B `TAKO_2956_LEGACY=1`。実経路で修正前は `tako list` が 3.1 秒 / 6 秒以上待たされ perf.log に `dispatch:SetupBootstrap 3054ms` → 修正後 0.1 秒・専有なし（PASS 21）・カタログ +0 B。残り 7 本は #2961。PR #2963

## 2026-11-17（#2939: Windows のスマホのショートカットの既定のデスクトップ・ダウンロードを境界 B39 `platform::known_folder`（SHGetKnownFolderPath）で引く・#2432 の写しも寄せた・検証プロセスはホームが OS のプロファイルと同じときだけ）
- A/B `TAKO_2939_LEGACY=1`・番犬 `issue2939_known_folder_watchdog`。Windows CI で 4 つの既知フォルダが .NET / シェルの答えと一致・macOS は前後で同じ・カタログ +0 B・実機の確認待ちで Refs。PR #2964

## 2026-11-17（#2951: モデル一覧（`codex debug models` / `agy models`）と stale binary の版（`claude --version`）を上限つきの門番 `agent_probe::output_with_timeout`（`probe_timeout` = 既定 15 秒・孫まで止める）へ寄せ、打ち切りは `failure.kind = timed_out` / 版が取れない扱い。全系統は並べて聞く）
- A/B `TAKO_2951_LEGACY=1`・番犬 `issue2951_agent_probe_unbounded_watchdog`。実経路で修正前 30 秒無言 → 修正後 上限の秒数で理由つき・孫 0 件（PASS 42）・カタログ +12 B。PR #2965

## 2026-11-17（#2961 の 1: MCP `tako_setup_mcp` / `tako_setup` / `tako_config_share` を `prepare_offload` → `OffloadJob::SetupMcp` / `SetupRun` / `ConfigShare` の background で答え、UI スレッドで外部プロセスを待たない（同期の腕と 1 実装・setup の書き込みは錠 `SETUP_WRITE_SERIAL` を bootstrap と共有）。番犬の `KNOWN` から 3 つを外し寄せ先の表 `TARGETS` へ）
- A/B `TAKO_2961_LEGACY=1`。実経路で修正前は `dispatch:SetupMcp 6084ms` / `SetupRun 4007ms` / `ConfigShare 6053ms` → 修正後 `tako list` 0.1 秒・専有なし（PASS 36）・カタログ +0 B。PR #2968

## 2026-11-17（#2961 の 2: MCP `tako_remote_setup` / `tako_remote_scrollback` を `OffloadJob::RemoteSetup` / `RemoteScrollback` の background で答え（採取は器のセッション名だけ UI で解く）、`tako_open_remote` は ssh の実体を #1984 の先取り（`ProbeNeeds::ssh_bin`）で background に済ませる。番犬にメソッド呼び出しの葉 `.capture_scrollback(` を足し、辿れなかった採取を見えるようにした）
- 実経路で修正前は `dispatch:RemoteSetup 10009ms` / `RemoteScrollback 5010ms` / `OpenRemote 5022ms` → 修正後 `tako list` 0.1 秒・専有なし（PASS 33）・カタログ +0 B。PR #2970（閉じて #2974 に含めて merge）

## 2026-11-17（#2948: Windows の Web ビューのページが宛先のあいだも tako のショートカットが届くように（WebView2 の AcceleratorKeyPressed を controller から直に付け、tako の割り当てに当たる打鍵だけ Handled → 前景タスクが宛先を戻して dispatch_keystroke）。判定は macOS の #326 と同じ 1 実装 `keys::webview_key_route`（表は 1 つ・macOS の答えは旧表と全キーコードで一致））
- A/B `TAKO_2948_LEGACY=1`・番犬 `issue2948_webview_shortcut_watchdog`（注入 8 通りで名指し）・新しい crate なし・カタログ +0 B・実機の確認待ちで Refs。PR #2971

## 2026-11-17（#2961 の 3: MCP `tako_remote_folder` を action ごとに段へ分けた（open / ls / open-file / push / list = background の `remote_folder_run`・着地でツリー / プレビュー・close は着地で外して ControlMaster の後始末を background = `Settled`・ssh-pane は先取り）。番犬の `KNOWN` から外し `STAGED_TARGETS` と #1984 の `UI_PATHS` で縛る）
- 実経路で修正前は `dispatch:RemoteFolder` 約 6060ms（ls / open-file / list / close）→ 修正後 `tako list` 0.1 秒・専有なし（PASS 35）・カタログ +0 B。PR #2972（閉じて #2974 に含めて merge）

## 2026-11-17（#2961 の 4 + #2966: MCP `tako_sleep_guard` の蓋閉じの準備・片付け・電池の設定を `OffloadJob::SleepGuardOs`（設定の書き込みは着地）へ、tako-app の `apply_stale_binary_scan` の版の読みを背景の走査へ。状態の照会は今の claude の版を覚えから使う（#2966）。番犬の `KNOWN` は空・tako-app の着地は `TAKO_APP_UI_LANDINGS` が本文で見る）
- 実経路で修正前は `dispatch:SleepGuard` 約 6060ms・着地 `periodic_land:stale_binary` 3014ms・#2966 の照会 6.1 秒 → 修正後 専有なし・照会 3.1 秒（sleep PASS 20・stale PASS 11）・カタログ +0 B。#2970 / #2972 も含めて squash merge。PR #2974
