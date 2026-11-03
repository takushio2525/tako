// PC の負荷と順番待ちの画面（#1999。エピック #1992 の S7。設計は `.agent/plans/2026-10-resource-manager.md` の §7）。
//
// `#/` に機の状態のカード（`MachineCard`）、`#/resources` に実行中と待ち（誰 / 何 / 進み / 見込み / 優先度）。
//
// ## 何も新設していない
//
// - 数値の正本は PC の `tako queue` / MCP `tako_queue` の status（`queue::answer`）。daemon は
//   `Request::Queue` を素通しして、機（`machine`）と順番待ち（`queue`）の節を**手を加えずに**返す
//   （経路は `tako_control::remote_resources::RESOURCE_ROUTES` が正本）。だから受付番号・並び・段・
//   見込みは PC の resources ビュー（#1995）・CLI と同じ答えになる。ここは描くだけ
// - 待つ理由と開始の見込みの文は調停役が組んだ日本語の 1 文（`text` / `start_text`。CLI と同じ 1 実装）。
//   終わりの見込みだけは PC の resources ビューと同じく `eta` の欄から組む（経過を二重に書かない）
// - 優先度の変更と取り消しは CLI `tako queue priority` / `cancel` と同じ `Request::Queue`。
//   **manage 以上**の端末にだけ操作を出し、足りない端末には #1452 の `PermissionRequest` を出す
//   （daemon も同じ role で断る = 画面で隠すだけにしない）
// - ポーリングは tasks.jsx の `usePolling`（画面を離れたら止まる・裏では撃たない）を周期だけ変えて使う
//
// 語彙（段の名前 top / high / normal / low / default・「この処理」「ペイン N の既定」・取り消しの確かめ）は
// PC の resources ビューと同じ。絵文字は使わない（印は CSS の矩形と既存の SVG）。
import { useState, useCallback } from 'preact/hooks';
import { createClient } from '../api';
import { PermissionRequest, roleAtLeast } from '../components/permission-request';
import { usePolling, BackIcon } from './tasks';

/** `#/resources` を見ているあいだの読み直しの周期（ペイン一覧と同じ 3 秒） */
export const RESOURCES_POLL_MS = 3000;
/** `#/` のカードの読み直しの周期（タスクのバッジと同じ 5 秒） */
export const MACHINE_CARD_POLL_MS = 5000;
/** 優先度の変更と取り消しに要る role。daemon の `RESOURCE_ROUTES` と揃える */
export const MANAGE_ROLE = 'manage';
/** 段の選択肢（CLI `tako queue priority` の値と同じ並び。default = 既定へ戻す） */
export const PRIORITY_CHOICES = ['top', 'high', 'normal', 'low', 'default'];

// --- 数の言い方（PC の `ui_text::resources` と同じ区切り）---

/** MiB → 「24.1 GB」/「512 MB」 */
export function size(mb) {
  if (typeof mb !== 'number') return '-';
  return mb >= 1024 ? `${(mb / 1024).toFixed(1)} GB` : `${mb} MB`;
}

/** 経過・待ちの時間（時間 / 分 / 秒） */
export function secs(s) {
  if (typeof s !== 'number') return '-';
  if (s >= 3600) return `${Math.floor(s / 3600)} 時間 ${Math.floor((s % 3600) / 60)} 分`;
  if (s >= 60) return `${Math.floor(s / 60)} 分 ${s % 60} 秒`;
  return `${s} 秒`;
}

/** 見込みのおおよその長さ（1 分未満は秒、以上は分へ丸める。PC の `ui_text::resources::approx` と同じ） */
export function approx(s) {
  return s < 60 ? `${s} 秒` : `${Math.floor((s + 30) / 60)} 分`;
}

/**
 * 終わりの見込み（status の `eta` の欄から。PC の resources ビューと同じ 4 通り）。
 * 経過は同じ行の数値に出すので、ここでは繰り返さない（調停役の `eta.text` は「経過 N（見込みなし）」と
 * 経過を含むので、そのまま出すと二重になる = #1995 の 1 回目の画像と同じ）
 */
export function etaText(eta, progress) {
  if (!eta) return '';
  const rem = eta.remaining_s;
  if (eta.state === 'remaining' && typeof rem === 'number') {
    if (eta.from === 'progress' && progress && progress.total > 0) {
      return `あと約 ${approx(rem)}（進み ${progress.done}/${progress.total} からの見込み）`;
    }
    if (typeof eta.previous_s === 'number') return `あと約 ${approx(rem)}（前回 ${approx(eta.previous_s)}）`;
    return `あと約 ${approx(rem)}`;
  }
  if (eta.state === 'overrun') {
    const prev = typeof eta.previous_s === 'number' ? approx(eta.previous_s) : '-';
    return `前回より長くかかっている（前回 ${prev}）`;
  }
  return '見込みなし';
}

/** 段の出どころ（処理に付けた段 / ペインの既定 / 役割の既定。待ちで上がったら元の段も） */
export function priorityFrom(job) {
  const from = { job: '処理に付けた段', pane: 'ペインの既定', role: '役割の既定' }[job.priority_from];
  if (!from) return '';
  return job.aged ? `${from} ${job.base_priority || '?'} から待ちで上がった` : from;
}

/** 誰の処理か（ペイン · タブ · ペイン名 · 役割）。ペインの外（launchd・別の端末）は「ペインの外」 */
export function owner(job) {
  if (typeof job.pane !== 'number') return 'ペインの外';
  const parts = [`ペイン ${job.pane}`];
  for (const part of [job.tab, job.title, job.role]) {
    if (part && !parts.includes(part)) parts.push(part);
  }
  return parts.join(' · ');
}

function pressureTone(p) {
  if (p === 'critical') return 'critical';
  if (p === 'warn') return 'warn';
  return 'normal';
}

/** 実行中と待ちの件数（`#/` のカードの 1 行。どちらも 0 なら出さない = PC のステータスバーの区画と同じ） */
export function queueCounts(data) {
  const q = data && data.queue;
  const running = Array.isArray(q && q.running) ? q.running.length : 0;
  const waiting = Array.isArray(q && q.waiting) ? q.waiting.length : 0;
  return { running, waiting };
}

// --- 読み ---

/**
 * `/api/resources` を周期で読む。`null` = まだ読んでいない。
 * 読めない（古い daemon = 404 など）ときは `error` に理由を置き、`data` は最後に読めた答えのまま
 */
export function useResources(intervalMs = RESOURCES_POLL_MS) {
  const [data, setData] = useState(null);
  const [error, setError] = useState(null);
  const reload = useCallback(() => {
    return createClient()
      .resources()
      .then(r => { setData(r); setError(null); })
      .catch(e => setError(e));
  }, []);
  usePolling(reload, [], intervalMs);
  return { data, error, reload };
}

// --- 機のカード（`#/` と `#/resources` の頭）---

function MemoryBar({ used, total, pressure }) {
  const pct = total > 0 ? Math.min(100, Math.max(0, Math.round((used / total) * 100))) : 0;
  return (
    <div class={`res-bar tone-${pressureTone(pressure)}`} data-testid="res-memory-bar" aria-hidden="true">
      <span class="res-bar-fill" style={`width: ${pct}%;`} />
    </div>
  );
}

/**
 * 機の状態。`compact`（`#/`）は メモリ・メモリ圧・CPU・件数の 2 行、そうでなければ内訳まで。
 * `onOpen` があればカード全体を押すと `#/resources` へ
 */
export function MachineCard({ data, compact = false, onOpen }) {
  const m = data && data.machine;
  const mem = m && m.memory_mb;
  const { running, waiting } = queueCounts(data);
  const counts = running + waiting > 0 ? `実行 ${running} · 待ち ${waiting}` : '';
  const pct = typeof (m && m.cpu_percent) === 'number' ? `${Math.round(m.cpu_percent)}%` : '-';
  const load = Array.isArray(m && m.load) && m.load.length === 3
    ? ` · load ${m.load.map(l => Number(l).toFixed(1)).join(' / ')}`
    : '';
  const cls = `machine-card${onOpen ? ' clickable' : ''}${compact ? ' compact' : ''}`;
  return (
    <div class={cls} data-testid={compact ? 'machine-card' : 'machine-card-full'} onClick={onOpen}>
      <div class="machine-card-head">
        {/* `#/resources` は見出しが「PC の負荷」なので、カードは PC の resources ビューの節名（機） */}
        <span class="machine-card-title">{compact ? 'PC の負荷' : '機'}</span>
        {counts && <span class="machine-card-counts" data-testid="machine-card-counts">{counts}</span>}
        {onOpen && <span class="footer-arrow" aria-hidden="true">›</span>}
      </div>
      {mem && typeof mem.total === 'number' && mem.total > 0 ? (
        <>
          <div class="machine-card-row">
            <span class="machine-card-label">メモリ</span>
            <MemoryBar used={mem.used || 0} total={mem.total} pressure={m.pressure} />
            <span class="machine-card-value" data-testid="machine-memory">
              {size(mem.used)} / {size(mem.total)}
            </span>
          </div>
          <div class="machine-card-meta">
            <span class={`res-pressure tone-${pressureTone(m.pressure)}`} data-testid="machine-pressure">
              メモリ圧 {m.pressure || '-'}
            </span>
            <span>CPU {pct}（{m.cpus || 0} コア）{compact ? '' : load}</span>
          </div>
          {!compact && (
            <div class="machine-card-meta" data-testid="machine-detail">
              <span>
                圧縮 {size(mem.compressed || 0)} · 空き {size(mem.available || 0)}
                {m.swap_mb && typeof m.swap_mb.total === 'number'
                  ? ` · swap ${size(m.swap_mb.used || 0)} / ${size(m.swap_mb.total)}`
                  : ''}
              </span>
            </div>
          )}
        </>
      ) : (
        <div class="machine-card-meta">
          <span>この OS では機の状態を採れない（未対応）</span>
        </div>
      )}
      {data && data.gui === false && (
        <div class="machine-card-note" data-testid="machine-offline">{data.note || '順番待ちは無効（tako が起動していない）'}</div>
      )}
    </div>
  );
}

// --- 実行中 / 待ちの 1 件 ---

function PriorityChip({ level }) {
  return <span class={`prio-chip prio-${level || 'none'}`} data-testid="prio-chip">{level || '-'}</span>;
}

function ChoiceRow({ label, selected, onPick, busy, testid }) {
  return (
    <div class="prio-choice-row" data-testid={testid}>
      <span class="prio-choice-label">{label}</span>
      <div class="prio-choices">
        {PRIORITY_CHOICES.map(level => (
          <button
            key={level}
            class={`prio-choice${selected === level ? ' selected' : ''}`}
            disabled={busy}
            aria-pressed={selected === level}
            onClick={() => onPick(level)}
          >
            {level}
          </button>
        ))}
      </div>
    </div>
  );
}

function JobRow({ job, running, canManage, open, onToggle, onDone }) {
  const [busy, setBusy] = useState(false);
  const [confirm, setConfirm] = useState(false);
  const progress = job.progress && job.progress.total > 0 ? job.progress : null;
  const detail = [];
  if (running) {
    if (typeof job.cpu_percent === 'number') detail.push(`CPU ${Math.round(job.cpu_percent)}%`);
    if (typeof job.memory_mb === 'number') detail.push(size(job.memory_mb));
    if (typeof job.ran_s === 'number') detail.push(`経過 ${secs(job.ran_s)}`);
  } else {
    if (typeof job.waited_s === 'number') detail.push(`待ち ${secs(job.waited_s)}`);
    if (job.start_text) detail.push(job.start_text);
  }
  const from = priorityFrom(job);

  async function act(fn, ok) {
    setBusy(true);
    try {
      const r = await fn(createClient());
      onDone({ text: ok(r), error: false });
    } catch (e) {
      // 押す前に終わった・取り消された処理（daemon の 404 `gone`）は理由を言って読み直す
      const gone = e.status === 404 && e.body && e.body.kind === 'gone';
      onDone({
        text: gone ? `「${job.label}」はもう順番待ちに居ない` : `できませんでした: ${e.message}`,
        error: true,
      });
    } finally {
      setBusy(false);
      setConfirm(false);
    }
  }

  const pickJob = level => act(
    c => c.setJobPriority(job.id, level),
    r => (typeof r.position === 'number'
      ? `優先度を ${level} にした（待ち ${r.position} 番目）`
      : `優先度を ${level} にした`),
  );
  const pickPane = level => act(
    c => c.setPanePriority(job.pane, level),
    () => `ペイン ${job.pane} の優先度の既定を ${level} にした`,
  );
  const cancel = () => act(
    c => c.cancelJob(job.id),
    () => `「${job.label}」を取り消した`,
  );

  return (
    <div
      class={`job-row${running ? ' running' : ' waiting'}${job.stalled ? ' stalled' : ''}`}
      data-testid={running ? 'running-row' : 'waiting-row'}
      data-job-id={job.id}
    >
      <div class="job-row-head">
        {!running && typeof job.position === 'number' && (
          <span class="job-position" data-testid="job-position">{job.position} 番目</span>
        )}
        <span class="job-label" data-testid="job-label">{job.label}</span>
        <PriorityChip level={job.priority} />
      </div>
      <div class="job-owner" data-testid="job-owner">
        {owner(job)}
        {typeof job.pane === 'number' && (
          <a class="job-open" href={`#/panes/${job.pane}`}>開く ›</a>
        )}
      </div>
      {running && progress && (
        <div class="job-progress" data-testid="job-progress">
          <div class="res-bar tone-normal" aria-hidden="true">
            <span class="res-bar-fill" style={`width: ${Math.round((progress.done / progress.total) * 100)}%;`} />
          </div>
          <span class="job-progress-text">{progress.done}/{progress.total}</span>
        </div>
      )}
      {running && job.eta && (
        <div class="job-eta" data-testid="job-eta">{etaText(job.eta, progress)}</div>
      )}
      {detail.length > 0 && <div class="job-detail">{detail.join(' · ')}</div>}
      {!running && job.text && <div class="job-reason" data-testid="job-reason">{job.text}</div>}
      {(job.stalled || job.cancel) && (
        <div class="job-flags">
          {job.stalled && <span class="job-flag">停滞（枠を返した）</span>}
          {job.cancel && <span class="job-flag">取り消し中</span>}
        </div>
      )}
      <div class="job-row-foot">
        <span class="job-from">{from}</span>
        {canManage && (
          <span class="job-actions">
            <button
              class={`btn btn-small${open ? ' active' : ''}`}
              data-testid="prio-toggle"
              aria-expanded={open}
              onClick={onToggle}
            >
              優先度
            </button>
            <button
              class="btn btn-small btn-danger-outline"
              data-testid="cancel-toggle"
              disabled={busy || job.cancel}
              onClick={() => setConfirm(true)}
            >
              取り消し
            </button>
          </span>
        )}
      </div>
      {canManage && open && (
        <div class="prio-panel" data-testid="prio-panel">
          <ChoiceRow
            label="この処理"
            selected={job.job_priority || 'default'}
            onPick={pickJob}
            busy={busy}
            testid="prio-job"
          />
          {typeof job.pane === 'number' && (
            <ChoiceRow
              label={`ペイン ${job.pane} の既定`}
              selected={job.pane_priority || 'default'}
              onPick={pickPane}
              busy={busy}
              testid="prio-pane"
            />
          )}
          {running && (
            <div class="prio-note">走っている処理は止めない（優先度は待ちの並びにだけ効く）</div>
          )}
        </div>
      )}
      {canManage && confirm && (
        <div class="cancel-confirm" data-testid="cancel-confirm">
          <span>
            {running ? `「${job.label}」を止めて取り消しますか？` : `「${job.label}」を待ちから外しますか？`}
          </span>
          <span class="cancel-confirm-actions">
            <button class="btn btn-small btn-danger" data-testid="cancel-yes" disabled={busy} onClick={cancel}>
              取り消す
            </button>
            <button class="btn btn-small" data-testid="cancel-no" disabled={busy} onClick={() => setConfirm(false)}>
              やめる
            </button>
          </span>
        </div>
      )}
    </div>
  );
}

// --- 画面本体 ---

export function ResourcesPage({ me, onMeRefresh }) {
  const { data, error, reload } = useResources(RESOURCES_POLL_MS);
  const [openId, setOpenId] = useState(null);
  const [notice, setNotice] = useState(null);
  const canManage = roleAtLeast((me && me.role) || 'observe', MANAGE_ROLE);
  const q = data && data.queue;
  const running = (q && Array.isArray(q.running)) ? q.running : [];
  const waiting = (q && Array.isArray(q.waiting)) ? q.waiting : [];
  const max = q && q.limits && typeof q.limits.max_jobs === 'number' ? q.limits.max_jobs : null;

  // 開いている段の行が答えから消えたら畳む（受付番号は wrapper の pid = 使い回されうる）
  const openRow = openId !== null && [...running, ...waiting].some(j => j.id === openId) ? openId : null;

  function onDone(n) {
    setNotice(n);
    setOpenId(null);
    reload();
  }

  return (
    <div class="page">
      <div class="panes-header">
        <div class="panes-header-row res-header-row">
          <button class="btn task-back" onClick={() => { window.location.hash = '#/panes'; }}>
            <BackIcon />
            ペイン
          </button>
          <span class="res-header-title">PC の負荷</span>
          <button class="refresh-btn" aria-label="更新" onClick={() => reload()}>
            <svg width="16" height="16" viewBox="0 0 16 16" fill="none" aria-hidden="true">
              <path d="M13.5 8a5.5 5.5 0 1 1-1.61-3.89" stroke="currentColor" stroke-width="1.4" stroke-linecap="round" />
              <path d="M13.6 2.2v3.1h-3.1z" fill="currentColor" />
            </svg>
          </button>
        </div>
      </div>
      <div class="res-scroll" data-testid="resources-page">
        {data === null && !error && <div class="res-empty">読んでいます…</div>}
        {error && data === null && (
          <div class="res-empty error-text" data-testid="resources-error">読めませんでした: {error.message}</div>
        )}
        {data && (
          <>
            <MachineCard data={data} />
            {notice && (
              <div class={`res-notice${notice.error ? ' error' : ''}`} data-testid="res-notice">{notice.text}</div>
            )}
            {q && q.arbiter === false && (
              <div class="res-empty">この tako は順番待ちの調停役ではない（隔離 / セルフテスト / セカンダリ）</div>
            )}
            {q && q.arbiter !== false && (
              <>
                <section class="res-section">
                  <h2 data-testid="running-title">
                    {max !== null ? `実行中 ${running.length} / 上限 ${max}` : `実行中 ${running.length}`}
                  </h2>
                  {running.length === 0 && <div class="res-empty">走っている処理はない</div>}
                  {running.map(job => (
                    <JobRow
                      key={job.id}
                      job={job}
                      running
                      canManage={canManage}
                      open={openRow === job.id}
                      onToggle={() => setOpenId(openRow === job.id ? null : job.id)}
                      onDone={onDone}
                    />
                  ))}
                </section>
                <section class="res-section">
                  <h2 data-testid="waiting-title">待ち {waiting.length}</h2>
                  {q.blocked && q.blocked.text && (
                    <div class="job-reason" data-testid="blocked-reason">{q.blocked.text}</div>
                  )}
                  {waiting.length === 0 && <div class="res-empty">待っている処理はない</div>}
                  {waiting.map(job => (
                    <JobRow
                      key={job.id}
                      job={job}
                      running={false}
                      canManage={canManage}
                      open={openRow === job.id}
                      onToggle={() => setOpenId(openRow === job.id ? null : job.id)}
                      onDone={onDone}
                    />
                  ))}
                </section>
                {typeof q.unmanaged === 'number' && q.unmanaged > 0 && (
                  <div class="res-empty">順番待ちを通っていない重い処理 {q.unmanaged} 件</div>
                )}
                {!canManage && running.length + waiting.length > 0 && (
                  <section class="res-section" data-testid="resources-permission">
                    <PermissionRequest
                      me={me}
                      need={MANAGE_ROLE}
                      what="優先度の変更と取り消し"
                      onGranted={onMeRefresh}
                    />
                  </section>
                )}
              </>
            )}
          </>
        )}
      </div>
    </div>
  );
}
