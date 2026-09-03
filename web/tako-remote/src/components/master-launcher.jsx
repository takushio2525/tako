// スマホから「新しいタブ + master 起動」（#1078 / エピック #1059 柱 1-D）
//
// 1 操作で ①タブを作り ②そのタブで master を起動し ③公式の Remote Control へ
// 繋がるまで待って ④Claude アプリへ送り出す。
//
// ## 通る経路（すべて daemon 側に既存の判定を通す）
//
//   GET  /api/master/profiles      … 選択肢（`tako orchestrator profiles list` と同じ 1 実装）
//   POST /api/tabs                 … `TabNew` を dispatch（Manage role）
//   POST /api/tabs/:id/master      … 組み立ては `orchestrator::master_launch`（CLI と同一）
//   GET  /api/v2/panes             … `remote_link.state` が connected になるまで待つ
//
// ## ここで守っている線
//
// - **繋がるまで待つ。待てなければ理由を出す**。URL は daemon が持っているときだけ出す
//   （#1077 と同じ = PWA は URL を組み立てない）
// - opt-in していないプロファイルは**起動した時点で**理由が返る（`remote_control.state`）。
//   待ち続けても繋がらないので、待たずに理由へ切り替える
// - role が足りない端末では**押す前に**理由を出す（サーバーは 403 を返すが、
//   押してから断られるより先に分かる方がよい）
import { useState, useEffect, useRef } from 'preact/hooks';
import { createClient } from '../api';
import { ClaudeOpenLink, RemoteLinkReason } from './remote-link';

// 繋がるまでの待ち。claude の起動 + bridge 接続で実測十数秒かかるので余裕を持たせる
const POLL_MS = 2000;
const MAX_WAIT_MS = 90000;

/// master を立てられる role か（daemon の `required_role` = Manage と対）
export function canLaunchMaster(me) {
  return me?.role === 'manage' || me?.role === 'admin';
}

function ProfileRow({ profile, onPick, disabled }) {
  const rc = profile.remote_control;
  // opt-in していても環境が不適格ならフラグは付かない（#1068）。
  // 「繋がる見込み」は remote_control_effective が正
  const effective = profile.remote_control_effective;
  const badge = !rc
    ? { cls: 'off', text: 'Remote Control OFF' }
    : effective === false
      ? { cls: 'blocked', text: 'Remote Control 不可' }
      : { cls: 'on', text: 'Remote Control ON' };
  const hints = [profile.model || 'CLI 既定', profile.effort].filter(Boolean).join(' · ');
  return (
    <button
      class="launch-profile"
      disabled={disabled}
      onClick={() => onPick(profile)}
    >
      <span class="launch-profile-main">
        <span class="launch-profile-name">{profile.name}</span>
        <span class={`launch-rc-badge rc-${badge.cls}`}>{badge.text}</span>
      </span>
      <span class="launch-profile-sub">
        {hints}
        {profile.cwd ? ` · ${profile.cwd}` : ''}
        {profile.error ? ` · 設定が壊れています` : ''}
      </span>
      {Array.isArray(profile.projects) && profile.projects.length > 0 && (
        <span class="launch-profile-projects">
          {profile.projects.map(p => <span key={p} class="card-chip">{p}</span>)}
        </span>
      )}
    </button>
  );
}

/// 起動後の待ち画面。`remote_link` が connected になるまで `/api/v2/panes` を見る
function WaitPanel({ result, onClose, onOpenPane }) {
  const [link, setLink] = useState(null);
  const [gaveUp, setGaveUp] = useState(false);
  const timerRef = useRef(null);
  const startedRef = useRef(Date.now());

  // opt-in していない / 環境が不適格なら待たない（繋がらないことが起動時点で確定している）
  const willConnect = result.remote_control?.state === 'enabled';

  useEffect(() => {
    if (!willConnect) return undefined;
    const client = createClient();
    const tick = async () => {
      try {
        const { panes = [] } = await client.panes();
        const pane = panes.find(p => String(p.id) === String(result.pane));
        const rl = pane?.remote_link;
        if (rl) setLink(rl);
        if (rl && rl.state === 'connected' && rl.url) {
          clearInterval(timerRef.current);
          return;
        }
      } catch {
        // 一時的な失敗は次の周期で拾う（諦めるのは上限に達したときだけ）
      }
      if (Date.now() - startedRef.current > MAX_WAIT_MS) {
        clearInterval(timerRef.current);
        setGaveUp(true);
      }
    };
    tick();
    timerRef.current = setInterval(tick, POLL_MS);
    return () => clearInterval(timerRef.current);
  }, [willConnect, result.pane]);

  const connected = !!(link && link.state === 'connected' && link.url);
  // 理由の出どころ: ペインの `remote_link`（あれば新しい）→ 起動応答の `remote_control`
  const reason = [link, result.remote_control].find(r => r && (r.reason || r.next_step));

  return (
    <div class="launch-result">
      <div class="launch-result-head">
        <span class="launch-result-title">
          {result.tab_title} を起動しました
        </span>
        <span class="launch-result-sub">
          タブ {result.tab} · ペイン {result.pane} · {result.profile}
          {result.model ? ` · ${result.model}` : ''}
        </span>
      </div>

      {connected ? (
        <>
          <p class="launch-note">Claude アプリ / claude.ai からこの会話へ指示できます。</p>
          <ClaudeOpenLink link={link} block />
        </>
      ) : willConnect && !gaveUp ? (
        <div class="launch-waiting">
          <div class="spinner" />
          <span>Claude 公式に繋がるのを待っています…</span>
        </div>
      ) : (
        // 待たない / 待っても繋がらなかった場合は理由を出す。
        // 文言は daemon が返したもの（#1077 と同じ 1 実装）
        <>
          {gaveUp && (
            <p class="launch-note">
              時間内に Claude 公式へ繋がりませんでした。ペインを開いて起動状況を確認してください。
            </p>
          )}
          {/* 理由を持っていない応答（Remote Control を知らない古い daemon）では
              黙って空にせず、ペインを開けば状況が見えることだけ伝える */}
          {reason ? (
            <RemoteLinkReason link={reason} />
          ) : (
            !gaveUp && (
              <p class="launch-note">
                この会話は Claude 公式へは繋ぎません。ペインを開くと起動状況が見えます。
              </p>
            )
          )}
        </>
      )}

      <div class="launch-result-actions">
        <button class="btn" onClick={onOpenPane}>ペインを開く</button>
        <button class="btn" onClick={onClose}>閉じる</button>
      </div>
    </div>
  );
}

/// master ランチャー本体（一覧画面から開くボトムシート）
export function MasterLauncher({ me, onClose, onLaunched }) {
  const [profiles, setProfiles] = useState(null);
  const [error, setError] = useState(null);
  const [busy, setBusy] = useState(false);
  const [result, setResult] = useState(null);

  useEffect(() => {
    createClient()
      .masterProfiles()
      .then(r => setProfiles(r.profiles || []))
      .catch(e => setError(e.message));
  }, []);

  async function pick(profile) {
    setBusy(true);
    setError(null);
    const client = createClient();
    try {
      // ①タブを作る。プロファイルが起動フォルダを持っていればそこで開く
      //   （スマホからは `cd` できないので、ここで合わせないと会話の作業場所がずれる）
      const tab = await client.createTab(profile.cwd || undefined);
      // ②そのタブで master を起動する（組み立ては daemon 側 = CLI と同一）
      const launched = await client.launchMaster(tab.tab, profile.name);
      setResult({ ...launched, tab: launched.tab ?? tab.tab, pane: launched.pane ?? tab.pane });
      onLaunched?.();
    } catch (e) {
      setError(e.status === 403
        ? `この端末の権限では master を起動できません（Manage 以上が必要です）: ${e.message}`
        : e.message);
    } finally {
      setBusy(false);
    }
  }

  const allowed = canLaunchMaster(me);

  return (
    <div class="sheet-backdrop" onClick={onClose}>
      <div class="sheet" onClick={e => e.stopPropagation()}>
        <div class="sheet-head">
          <span class="sheet-title">master を起動</span>
          <button class="sheet-close" onClick={onClose} aria-label="閉じる">×</button>
        </div>

        {result ? (
          <WaitPanel
            result={result}
            onClose={onClose}
            onOpenPane={() => { window.location.hash = `#/panes/${result.pane}`; }}
          />
        ) : !allowed ? (
          <p class="launch-note">
            この端末の権限（{me?.role || '不明'}）では新しいタブとプロセスを作れません。
            Mac 側で Manage 以上に昇格させてください。
          </p>
        ) : error ? (
          <p class="error-text">{error}</p>
        ) : profiles === null ? (
          <div class="launch-waiting"><div class="spinner" /><span>プロファイルを読み込み中…</span></div>
        ) : profiles.length === 0 ? (
          <p class="launch-note">master プロファイルがありません。Mac 側で `tako setup` を実行してください。</p>
        ) : (
          <>
            <p class="launch-note">
              新しいタブを作って master を起動します。Remote Control が ON のプロファイルなら、
              起動後に Claude アプリから指示できます。
            </p>
            <div class="launch-profile-list">
              {profiles.map(p => (
                <ProfileRow key={p.name} profile={p} onPick={pick} disabled={busy} />
              ))}
            </div>
          </>
        )}
      </div>
    </div>
  );
}
