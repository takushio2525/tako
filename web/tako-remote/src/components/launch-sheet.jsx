// スマホの「+」から新しいタブを立てる（#1449）。
//
// 3 種類（master / ターミナル / SSH ターミナル）を 1 枚のシートで選ばせる。
//
// ## 通る経路（**操作は 1 つも新設していない**）
//
//   master        … POST /api/tabs → POST /api/tabs/:id/master（#1078）
//   ターミナル     … POST /api/tabs だけ（`TabNew` はシェル 1 枚を付けたタブを返す）
//   SSH ターミナル … GET /api/ssh-hosts → POST /api/ssh { target: "tab" }（#1080 / #1006）
//
// どれも PC 側の既存 dispatch（`TabNew` / `OpenRemote`）へ 1:1 で載っているので、
// 同じことが CLI / MCP からもできる（設計原則 5）。**PWA 側に操作ロジックを作らない**:
// 開き先の語彙（`tab`）もホスト一覧も、PC 側の 1 実装が返した値をそのまま使う。
//
// ## ここで守っている線
//
// - 3 経路とも **Manage role**（正は daemon の `required_role`）。足りない端末には
//   選択肢を出さず理由だけ出す（押してから 403 で断られるのが一番悪い）
// - 立ったら**そのペインへ移る**。ただし master だけは移らない:
//   「Claude 公式へ繋がるまで待つ」画面（#1078）がシートの中にあるため
// - 失敗（403 / 400 / 503）は**その場に理由を出す**。黙って閉じない
import { useMemo, useState } from 'preact/hooks';
import { createClient } from '../api';
import { MasterPanel, canLaunch } from './master-launcher';
import { SshHostList } from './ssh';

/// 「+」で選べる種別。**ラベルと説明はここだけ**（メニューと見出しが同じ表を引く）
export const LAUNCH_KINDS = [
  {
    id: 'master',
    label: 'master を起動',
    desc: 'オーケストレーターの会話を新しいタブで始める',
  },
  {
    id: 'terminal',
    label: 'ターミナル',
    desc: '新しいタブにシェルを 1 枚開く',
  },
  {
    id: 'ssh',
    label: 'SSH ターミナル',
    desc: '~/.ssh/config のホストへ新しいタブで接続する',
  },
];

/**
 * #1449 の対照（A/B）。`?tako_1449_legacy=1` を付けて開くと、「+」は
 * #1449 以前とまったく同じ「master 1 択」に戻る。
 *
 * PWA はブラウザで動くので env（`TAKO_1449_LEGACY=1`）が届かない。
 * **逃げ道はこの 1 つだけ**にしてあり、e2e の legacy 腕もこれを使う
 */
export function legacy1449() {
  try {
    return new URLSearchParams(window.location.search).get('tako_1449_legacy') === '1';
  } catch {
    return false;
  }
}

/// 起動できない role の理由（3 種で共通。文言は #1078 のものを保つ）
function RoleNote({ me }) {
  return (
    <p class="launch-note">
      この端末の権限（{me?.role || '不明'}）では新しいタブとプロセスを作れません。
      Mac 側で Manage 以上に昇格させてください。
    </p>
  );
}

/// 種別を選ぶメニュー（既定の入口）
function KindMenu({ onPick, busy, error }) {
  return (
    <>
      <p class="launch-note">新しいタブを作って、何を立てるか選びます。</p>
      {error && <p class="error-text" data-testid="launch-error">{error}</p>}
      <div class="launch-kind-list">
        {LAUNCH_KINDS.map(k => (
          <button
            key={k.id}
            class="launch-kind"
            data-testid={`launch-kind-${k.id}`}
            disabled={!!busy}
            onClick={() => onPick(k.id)}
          >
            <span class="launch-kind-label">{k.label}</span>
            <span class="launch-kind-desc">{k.desc}</span>
            {busy === k.id && <span class="ssh-bar-spinner launch-kind-spinner" />}
          </button>
        ))}
      </div>
    </>
  );
}

/**
 * 「+」のシート本体。
 *
 * `kind` が null ならメニュー、それ以外はその種別の画面。
 * ターミナルは選んだ時点で走るので（1 操作で立つ）、専用の画面を持たない
 */
export function LaunchSheet({ me, onClose, onLaunched }) {
  const legacy = legacy1449();
  // **毎レンダーで作り直さない**: `SshHostList` は client を依存配列に持つので、
  // 新しい object を渡すと一覧を取り直し続ける（無限ループ）
  const client = useMemo(() => createClient(), []);
  // legacy 腕は #1449 以前の挙動（開いたらいきなり master の選択肢）
  const [kind, setKind] = useState(legacy ? 'master' : null);
  const [busy, setBusy] = useState(null);
  const [error, setError] = useState(null);
  const allowed = canLaunch(me);

  /// 失敗の理由を人が読める形へ。403 は「押せる端末ではない」ことを明示する
  function reasonOf(e) {
    return e.status === 403
      ? `この端末の権限では新しいタブを作れません（Manage 以上が必要です）: ${e.message}`
      : e.message;
  }

  /// 立ったペインへ移る（PWA は 1 ペイン = 1 画面なので、これがタブの切り替えにあたる）
  function goToPane(paneId) {
    onLaunched?.();
    window.location.hash = `#/panes/${paneId}`;
    onClose();
  }

  /// ターミナル: `POST /api/tabs` だけ。`TabNew` はシェル 1 枚付きのタブを返す
  async function openTerminal() {
    if (busy) return; // 連打で 2 枚立てない
    setBusy('terminal');
    setError(null);
    try {
      const tab = await client.createTab();
      goToPane(tab.pane);
    } catch (e) {
      setError(reasonOf(e));
      setBusy(null);
    }
  }

  /// SSH: 開き先は常に新しいタブ（「+」= 新規起動なので既定の split へは倒さない）
  async function openSsh(hostEntry) {
    if (busy) return;
    setBusy(hostEntry.name);
    setError(null);
    try {
      const opened = await client.sshOpen(hostEntry.name, { target: 'tab' });
      goToPane(opened.pane);
    } catch (e) {
      // 接続の失敗ではなく「開けなかった」失敗。開けたあとの失敗は
      // ペイン側の `ssh_connect` に出続ける（#919 / #1040 の契約）
      setError(reasonOf(e));
      setBusy(null);
    }
  }

  function pickKind(id) {
    if (id === 'terminal') return openTerminal();
    setError(null);
    setKind(id);
    return undefined;
  }

  const heading = kind
    ? LAUNCH_KINDS.find(k => k.id === kind)?.label
    : '新しく起動';
  // legacy 腕は戻り先が無い（メニューが存在しないため）
  const canGoBack = !!kind && !legacy;

  return (
    <div class="sheet-backdrop" onClick={onClose}>
      <div class="sheet" onClick={e => e.stopPropagation()}>
        <div class="sheet-head">
          {canGoBack && (
            <button
              class="sheet-close"
              data-testid="launch-back"
              onClick={() => { setKind(null); setError(null); setBusy(null); }}
              aria-label="戻る"
            >‹</button>
          )}
          <span class="sheet-title">{heading}</span>
          <button class="sheet-close" onClick={onClose} aria-label="閉じる">×</button>
        </div>

        {!allowed ? (
          <RoleNote me={me} />
        ) : kind === 'master' ? (
          <MasterPanel onClose={onClose} onLaunched={onLaunched} />
        ) : kind === 'ssh' ? (
          <>
            <p class="launch-note">
              選んだホストへ新しいタブで接続します。接続に失敗してもペインは残り、
              理由がその場に出ます。
            </p>
            <SshHostList
              client={client}
              onPick={openSsh}
              busyHost={busy}
              error={error}
            />
          </>
        ) : (
          <KindMenu onPick={pickKind} busy={busy} error={error} />
        )}
      </div>
    </div>
  );
}
