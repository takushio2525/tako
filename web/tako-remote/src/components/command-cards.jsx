// コマンド提案カード（#1724）
//
// AI が `tako_show_command` / `tako show-command` で PC のペインに出したカード（#666）を、
// スマホのペイン画面にも出して、PC に戻らずに実行できるようにする。
//
// ## 実行は PC のボタンと同じ 1 経路
//
// 「実行」は `POST /api/cards/<id>/run`（`client.runCommandCard`）で、daemon は
// `Request::ShowCommand { action: "run" }` を素通しする。PC のカードの「新規ペインで実行」・
// CLI `tako show-command --run`・MCP `tako_show_command` も同じ dispatch を通るので、
// 実行先のペイン（同じタブに割る新しいペイン）も実行記録（実行中 / 終了コード）も揃う。
// **ここに実行の判断は 1 つも無い**。送るのはカード ID と番号だけで、本文は送らない。
//
// ## 誤タップ対策は確認 1 回
//
// 「実行」を押すと**全文**を見せる確認シートが開き、「PC で実行する」を押したときだけ撃つ。
// 送信中はボタンを止める。前回の実行がまだ走っているコマンドは押せない
// （daemon 側でも 409 で断る = 古い一覧を見たまま押しても 2 本走らない）。
//
// ## 権限
//
// 実行は interact 以上（`remote_cards::CARD_ROUTES` の Run と同じ）。observe の端末には
// 「実行」を出さず、#1452 の権限リクエストへつなぐ。**門の正は daemon**（403）で、
// ここは押せないボタンを並べないためにある。
import { useState } from 'preact/hooks';
import { PermissionRequest, roleAtLeast } from './permission-request';
import { usePolling } from '../pages/tasks';

// 実行に要る role（daemon の `CARD_ROUTES` の Run と一致していることを番犬が見る）
const RUN_ROLE = 'interact';

// 実行記録の語彙は PC のカード（`ui_text::command_card::run_*`）と同じにする
// （同じ記録を 2 つの画面が読むので、言い方がずれると「揃っていない」ように見える）
function runStateLabel(run) {
  if (!run) return null;
  if (run.state === 'running') return { text: '実行中', tone: 'wait' };
  if (run.state === 'exited') {
    return {
      text: `実行済み（終了コード ${run.exit_code}）`,
      tone: run.exit_code === 0 ? 'ok' : 'bad',
    };
  }
  if (run.state === 'closed') return { text: '実行済み（ペインは閉じた）', tone: 'muted' };
  // 未知の状態は握り潰さず素のまま出す
  return { text: String(run.state), tone: 'muted' };
}

function PlayIcon() {
  return (
    <svg width="11" height="11" viewBox="0 0 12 12" aria-hidden="true">
      <path d="M3 1.8v8.4L10 6z" fill="currentColor" />
    </svg>
  );
}

/**
 * そのペインのコマンドカード。カードが無ければ何も描かない。
 *
 * @param {object}   client       `createClient()` の戻り値
 * @param {string}   paneId       PC のペイン ID（数値。tmux ターゲットのときは描かない）
 * @param {object}   me           `/api/me` の応答（role を見る）
 * @param {Function} onMeRefresh  権限が変わったときに親の me を取り直す
 */
export function CommandCards({ client, paneId, me, onMeRefresh }) {
  const [cards, setCards] = useState([]);
  const [collapsed, setCollapsed] = useState(false);
  const [confirm, setConfirm] = useState(null);
  const [sending, setSending] = useState(false);
  const [sheetError, setSheetError] = useState(null);
  // 直近の操作の結果（カード ID + 番号ごと。押したブロックの下に出す）
  const [notice, setNotice] = useState(null);
  const [askPermission, setAskPermission] = useState(false);

  const numeric = /^\d+$/.test(String(paneId || ''));
  const role = (me && me.role) || 'observe';
  const canRun = roleAtLeast(role, RUN_ROLE);

  async function refresh() {
    if (!client || !numeric) return;
    try {
      const result = await client.commandCards(paneId);
      setCards(result.cards || []);
    } catch {
      // PC 側にペインが無い / app が居ない = カードも無い。一覧を空にする
      setCards([]);
    }
  }

  // 画面を離れたら止まり、裏へ回っているあいだは撃たない（タスク画面と同じ 1 実装）
  usePolling(refresh, [paneId, client]);

  // カードが消えても、押した結果（「PC 側で閉じられました」）と確認シートは残す。
  // 消すと、閉じられたカードを押した人には「押したら全部消えた」としか見えない
  if (!numeric || (cards.length === 0 && !notice && !confirm)) return null;
  const orphan = notice && !cards.some(c => c.id === notice.card) ? notice : null;

  function openConfirm(card, index) {
    setSheetError(null);
    setNotice(null);
    setConfirm({
      card: card.id,
      index,
      label: card.label,
      command: card.commands[index - 1],
      total: card.commands.length,
    });
  }

  async function run() {
    if (!confirm || sending) return;
    const target = confirm;
    setSending(true);
    setSheetError(null);
    try {
      const result = await client.runCommandCard(target.card, target.index);
      setConfirm(null);
      setNotice({
        card: target.card,
        index: target.index,
        tone: 'ok',
        text: `PC のペイン ${result.pane} で実行を始めました`,
      });
      if (navigator.vibrate) navigator.vibrate(10);
    } catch (e) {
      if (e.status === 403) {
        // その場で降格された（PC 側の操作）。一覧はそのまま、権限の導線へ切り替える
        setConfirm(null);
        setNotice({
          card: target.card,
          index: target.index,
          tone: 'bad',
          text: `この端末の権限では実行できません（${e.message}）`,
        });
        if (onMeRefresh) onMeRefresh();
      } else if (e.status === 404) {
        setConfirm(null);
        setNotice({
          card: target.card,
          index: target.index,
          tone: 'bad',
          text: 'このカードは PC 側で閉じられました',
        });
      } else if (e.status === 409) {
        setConfirm(null);
        setNotice({ card: target.card, index: target.index, tone: 'wait', text: e.message });
      } else {
        // 通信断・app 不在は確認シートに残す（もう一度押せる）
        setSheetError(e.message || '実行できませんでした');
      }
    } finally {
      setSending(false);
      refresh();
    }
  }

  const host = (me && me.host) || 'PC';

  return (
    <section class="cmd-cards" data-testid="command-cards">
      <button
        type="button"
        class="cmd-cards-head"
        aria-expanded={!collapsed}
        onClick={() => setCollapsed(!collapsed)}
      >
        <span class="cmd-cards-title">PC に出ているコマンド</span>
        <span class="cmd-cards-count" data-testid="command-cards-count">{cards.length}</span>
        <span class={`cmd-cards-chevron${collapsed ? ' is-collapsed' : ''}`} aria-hidden="true" />
      </button>

      {!collapsed && (
        <div class="cmd-cards-body">
          {orphan && (
            <div class={`cmd-notice cmd-state-${orphan.tone}`} data-testid="command-notice">
              <span>{orphan.text}</span>
              <button
                type="button"
                class="cmd-notice-close"
                aria-label="閉じる"
                onClick={() => setNotice(null)}
              >×</button>
            </div>
          )}
          {cards.map(card => (
            <div class="cmd-card" data-testid="command-card" data-card-id={card.id} key={card.id}>
              <div class="cmd-card-label">{card.label || '実行するコマンド'}</div>
              {card.commands.map((command, i) => {
                const index = i + 1;
                const run = (card.runs || [])[i] || null;
                const state = runStateLabel(run);
                const running = run && run.state === 'running';
                const here = notice && notice.card === card.id && notice.index === index;
                return (
                  <div class="cmd-block" data-testid="command-block" key={index}>
                    <pre class="cmd-text">{command}</pre>
                    <div class="cmd-row">
                      {card.commands.length > 1 && (
                        <span class="cmd-index">{index}/{card.commands.length} 件目</span>
                      )}
                      {state && (
                        <span class={`cmd-state cmd-state-${state.tone}`} data-testid="command-run-state">
                          {state.text}
                        </span>
                      )}
                      <span class="cmd-spacer" />
                      {canRun && (
                        <button
                          type="button"
                          class="cmd-run-btn"
                          data-testid="command-run"
                          disabled={running}
                          onClick={() => openConfirm(card, index)}
                        >
                          <PlayIcon />
                          <span>{running ? '実行中' : '実行'}</span>
                        </button>
                      )}
                    </div>
                    {here && (
                      <div class={`cmd-notice cmd-state-${notice.tone}`} data-testid="command-notice">
                        {notice.text}
                      </div>
                    )}
                  </div>
                );
              })}
            </div>
          ))}

          {!canRun && (
            askPermission ? (
              <PermissionRequest
                me={me}
                need={RUN_ROLE}
                what="コマンドの実行"
                onGranted={() => {
                  setAskPermission(false);
                  if (onMeRefresh) onMeRefresh();
                }}
              />
            ) : (
              <button
                type="button"
                class="cmd-perm-link"
                data-testid="command-run-permission"
                onClick={() => setAskPermission(true)}
              >
                実行には {RUN_ROLE} 以上の権限が要ります（この端末は今 {role}）。
                権限の更新をリクエストする
              </button>
            )
          )}
        </div>
      )}

      {confirm && (
        <div class="sheet-backdrop" onClick={() => !sending && setConfirm(null)}>
          <div class="sheet" data-testid="command-confirm" onClick={e => e.stopPropagation()}>
            <div class="sheet-head">
              <span class="sheet-title">PC でこのコマンドを実行しますか</span>
              <button
                class="sheet-close"
                aria-label="閉じる"
                disabled={sending}
                onClick={() => setConfirm(null)}
              >×</button>
            </div>
            {confirm.label && <p class="cmd-confirm-label">{confirm.label}</p>}
            {confirm.total > 1 && (
              <p class="cmd-confirm-label">{confirm.index}/{confirm.total} 件目</p>
            )}
            <pre class="cmd-confirm-text" data-testid="command-confirm-text">{confirm.command}</pre>
            <p class="cmd-confirm-note">
              {host} の同じタブに新しいペインを開いて実行します。いま見ているペインには書き込みません。
            </p>
            {sheetError && <p class="error-text" data-testid="command-confirm-error">{sheetError}</p>}
            <div class="cmd-confirm-actions">
              <button class="btn" disabled={sending} onClick={() => setConfirm(null)}>やめる</button>
              <button
                class="btn btn-primary"
                data-testid="command-confirm-run"
                disabled={sending}
                onClick={run}
              >
                {sending ? '送信中…' : 'PC で実行する'}
              </button>
            </div>
          </div>
        </div>
      )}
    </section>
  );
}
