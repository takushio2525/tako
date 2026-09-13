// 権限が足りないときの導線（#1452）。
//
// 「権限が足りません」で行き止まりにしない: この端末に何が足りないかを出し、
// **その場で PC へ権限の更新をリクエスト**し、承認されたら**画面を読み込み直さずに**
// 続きができるところまでを 1 部品に閉じてある。
//
// ## 新しい API を 1 本も呼ばない
//
// リクエストは #283 からある `POST /api/pair`（`client.pair()`）そのもの。登録済みの
// 端末が今と違う role を要求すると daemon が `kind=upgrade` の承認待ちを作る、という
// 仕組みが元々あって、**スマホ側にそれを押す場所が無かった**だけ。経路は
// `tako_control::remote_role::ROLE_ROUTES` が正本で、番犬がここの呼び口を照合する。
//
// ## 待ち方
//
// 承認待ちの**間だけ** `/api/me` を 2 秒で見に行き、承認 / 拒否 / 打ち切りのどれかで
// 必ず止める（押していない画面がポーリングを続けない）。role が上がったら
// `onGranted()` で親の `me` を取り直し、失敗していた読み込みをやり直す。
//
// ## この画面から権限は上がらない
//
// 押せるのは「要求を出す」までで、許可するのは PC の人だけ（CLI / MCP からも
// 上げられない = `.agent/requirements.md` FR-6.21）。だから文面も「リクエスト」で
// 統一し、「権限を上げる」とは書かない。
import { useState, useEffect, useRef } from 'preact/hooks';
import { createClient } from '../api';
import { getDeviceName } from '../store';

const POLL_MS = 2000;
// 承認されないまま放置された画面がポーリングを続けないための打ち切り（10 分）
const POLL_TIMEOUT_MS = 10 * 60 * 1000;

const ROLE_ORDER = ['observe', 'interact', 'manage', 'admin'];

const ROLE_DESC = {
  observe: '画面を見るだけ',
  interact: '+ テキスト入力・ファイルの参照と編集',
  manage: '+ ペインの管理・新しいタブの起動',
  admin: '+ 端末の管理',
};

/** 強さの比較（daemon の DeviceRole と同じ並び） */
export function roleAtLeast(current, need) {
  return ROLE_ORDER.indexOf(current) >= ROLE_ORDER.indexOf(need);
}

/** `need` 以上で選べる候補（今の role 以下は出さない = 押しても何も起きない選択肢を並べない） */
export function upgradeChoices(current, need) {
  const from = Math.max(ROLE_ORDER.indexOf(current) + 1, ROLE_ORDER.indexOf(need));
  return ROLE_ORDER.slice(Math.max(from, 0));
}

/**
 * #1452 の対照（A/B）。`?tako_1452_legacy=1` を付けて開くと、権限不足の表示は
 * #1452 以前とまったく同じ「足りません + 次の一手なし」に戻る。
 *
 * PWA はブラウザで動くので env（`TAKO_1452_LEGACY=1`）が届かない。
 * **逃げ道はこの 1 つだけ**にしてあり、e2e の legacy 腕もこれを使う
 */
export function legacy1452() {
  try {
    return new URLSearchParams(window.location.search).get('tako_1452_legacy') === '1';
  } catch {
    return false;
  }
}

/**
 * 権限不足の画面。
 *
 * @param {object}   me         `/api/me` の応答（role / pending / requested_role を見る）
 * @param {string}   need       この画面に要る role
 * @param {string}   what       何をするのに要るか（1 語。文面に差し込む）
 * @param {Function} onGranted  承認されたときに呼ぶ（親が me を取り直して再読込する）
 */
export function PermissionRequest({ me, need = 'interact', what = 'ファイルの参照', onGranted }) {
  const legacy = legacy1452();
  const current = (me && me.role) || 'observe';
  const choices = upgradeChoices(current, need);
  const [role, setRole] = useState(choices[0] || need);
  const [reason, setReason] = useState('');
  // form → sending → pending → denied / timeout
  const [phase, setPhase] = useState(me && me.pending ? 'pending' : 'form');
  const [error, setError] = useState(null);
  const timers = useRef({ poll: null, stop: null });

  // 承認待ちの間だけ /api/me を見に行く。承認・拒否・打ち切りで必ず止める
  useEffect(() => {
    if (phase !== 'pending') return undefined;
    const stop = () => {
      clearInterval(timers.current.poll);
      clearTimeout(timers.current.stop);
      timers.current = { poll: null, stop: null };
    };
    timers.current.poll = setInterval(async () => {
      try {
        const next = await createClient().me();
        if (next.registered && roleAtLeast(next.role, need)) {
          stop();
          if (onGranted) onGranted(next);
          return;
        }
        if (next.denied) {
          stop();
          setPhase('denied');
        }
      } catch {
        // 一時的な通信断では止めない（次の周期で拾う）
      }
    }, POLL_MS);
    timers.current.stop = setTimeout(() => {
      stop();
      setPhase('timeout');
    }, POLL_TIMEOUT_MS);
    return stop;
  }, [phase, need]);

  async function send() {
    setError(null);
    setPhase('sending');
    try {
      const result = await createClient().pair(getDeviceName() || '', role, reason);
      if (result.status === 'already_registered') {
        // 既にその権限があった（別の端末から先に上げられた等）
        if (onGranted) onGranted(null);
        return;
      }
      setPhase('pending');
    } catch (e) {
      setError(e.message);
      setPhase('form');
    }
  }

  // A/B: #1452 以前（「足りません」で行き止まり）を同じビルドのまま再現する
  if (legacy) {
    return (
      <div class="empty-state" data-testid="permission-legacy">
        <h2>権限が足りません</h2>
        <p>
          ファイルの参照には interact 以上の権限が要ります。
          Mac の tako で、この端末の権限を上げてください。
        </p>
      </div>
    );
  }

  if (phase === 'pending' || phase === 'sending') {
    const requested = (me && me.requested_role) || role;
    return (
      <div class="empty-state" data-testid="permission-pending">
        <div class="connect-icon"><div class="spinner" /></div>
        <h2>PC で承認待ち</h2>
        <p>
          {me && me.host ? `${me.host} の` : 'PC の'}画面に、この端末の権限を
          {` ${requested} `}へ変更してよいか確認が出ています。
          許可されるとこの画面はそのまま使えるようになります。
        </p>
        <button class="btn" onClick={() => setPhase('form')}>やり直す</button>
      </div>
    );
  }

  if (phase === 'denied' || phase === 'timeout') {
    return (
      <div class="empty-state" data-testid="permission-denied">
        <h2>{phase === 'denied' ? '許可されませんでした' : '応答がありません'}</h2>
        <p>
          {phase === 'denied'
            ? 'PC 側でこのリクエストが拒否されました。'
            : 'PC 側でまだ操作されていません。tako の画面で「設定 → リモート」からでも変更できます。'}
        </p>
        <button class="btn btn-primary" onClick={() => setPhase('form')}>
          もう一度リクエストする
        </button>
      </div>
    );
  }

  return (
    <div class="empty-state" data-testid="permission-request">
      <h2>権限が足りません</h2>
      <p>
        {what}には {need} 以上の権限が要ります（この端末は今 {current} です）。
        PC へ権限の更新をリクエストできます。許可できるのは PC の前にいる人だけです。
      </p>

      <label class="permission-label">要求する権限</label>
      <div class="permission-roles">
        {choices.map(r => (
          <button
            key={r}
            type="button"
            class={`permission-role${role === r ? ' is-selected' : ''}`}
            aria-pressed={role === r}
            data-testid={`permission-role-${r}`}
            onClick={() => setRole(r)}
          >
            <span class="permission-role-name">{r}</span>
            <span class="permission-role-desc">{ROLE_DESC[r]}</span>
          </button>
        ))}
      </div>

      <label class="permission-label" for="permission-reason">理由（任意）</label>
      <input
        id="permission-reason"
        class="permission-reason"
        type="text"
        value={reason}
        maxLength={200}
        placeholder="例: 出先でログを見たい"
        onInput={e => setReason(e.target.value)}
      />

      {error && <p class="error-text">{error}</p>}
      <button class="btn btn-primary" data-testid="permission-send" onClick={send}>
        権限の更新をリクエスト
      </button>
    </div>
  );
}
