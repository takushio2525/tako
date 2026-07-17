import { useState, useEffect } from 'preact/hooks';
import { PairingPage } from './pages/pairing';
import { PanesPage } from './pages/panes';
import { TerminalPage } from './pages/terminal';
import { createClient } from './api';
import { cleanupLegacyStore } from './store';

function parseRoute() {
  const hash = window.location.hash.slice(1) || '/';
  const [path, qs] = hash.split('?');
  return {
    path,
    params: new URLSearchParams(qs || ''),
    segments: path.split('/').filter(Boolean),
  };
}

// PWA 自身のバージョン（vite の define でビルド時に Cargo workspace version を埋め込む）。
// daemon と同一バイナリから配信されるため通常は必ず一致する。不一致 = service worker が
// 古いシェルをキャッシュしている状態なので、キャッシュを破棄してリロードする（#283）
const PWA_VERSION = typeof __TAKO_VERSION__ !== 'undefined' ? __TAKO_VERSION__ : 'dev';

async function clearCachesAndReload() {
  try {
    const keys = await caches.keys();
    await Promise.all(keys.map(k => caches.delete(k)));
    const regs = await navigator.serviceWorker?.getRegistrations?.() || [];
    await Promise.all(regs.map(r => r.update()));
  } catch {
    // キャッシュ削除に失敗しても reload で network-first に賭ける
  }
  window.location.reload();
}

export function App() {
  const [route, setRoute] = useState(parseRoute);
  // me: null = 確認中, それ以外 = /api/me の応答
  const [me, setMe] = useState(null);
  const [meError, setMeError] = useState(null);
  const [versionMismatch, setVersionMismatch] = useState(null);

  useEffect(() => {
    const onChange = () => setRoute(parseRoute());
    window.addEventListener('hashchange', onChange);
    return () => window.removeEventListener('hashchange', onChange);
  }, []);

  // 起動時: 旧ストアを掃除し、この端末の登録状態を確認する
  useEffect(() => {
    cleanupLegacyStore();
    refreshMe();
  }, []);

  async function refreshMe() {
    try {
      const result = await createClient().me();
      setMe(result);
      setMeError(null);
      if (
        PWA_VERSION !== 'dev' &&
        result.version &&
        result.version !== PWA_VERSION
      ) {
        setVersionMismatch(result.version);
      }
    } catch (e) {
      setMe(null);
      setMeError(e.message);
    }
  }

  // 接続不能（daemon 停止・tailnet 外・identity 拒否）
  if (meError) {
    return (
      <div class="connect-page">
        <div class="connect-card">
          <div class="connect-icon"><span class="status-badge-circle danger">!</span></div>
          <h1>接続エラー</h1>
          <p class="error-text">{meError}</p>
          <p style="color: var(--text-muted); font-size: 13px; margin-top: 8px;">
            Mac で <code>tako remote start</code> が実行中か、
            この端末が Mac と同じ Tailscale アカウントでログインしているか確認してください。
          </p>
          <div class="connect-actions">
            <button class="btn btn-primary" onClick={() => { setMeError(null); refreshMe(); }}>
              再試行
            </button>
          </div>
        </div>
      </div>
    );
  }

  // 確認中
  if (!me) {
    return (
      <div class="connect-page">
        <div class="connect-card">
          <div class="connect-icon"><div class="spinner" /></div>
          <h1>接続中...</h1>
        </div>
      </div>
    );
  }

  // 未登録 / 承認待ち / 拒否 → ペアリング画面
  if (!me.registered) {
    return <PairingPage me={me} onRegistered={refreshMe} />;
  }

  // バージョン不一致: 古い PWA シェルがキャッシュされている → 更新バナー
  const banner = versionMismatch && (
    <div class="version-banner" style="background: #92400e; color: #fff; padding: 8px 14px; font-size: 13px; display: flex; align-items: center; justify-content: space-between; gap: 10px;">
      <span>アプリの表示が古い可能性があります（Mac: v{versionMismatch} / 画面: v{PWA_VERSION}）</span>
      <button
        class="btn"
        style="flex: none; font-size: 12px; padding: 4px 10px;"
        onClick={clearCachesAndReload}
      >
        再読み込み
      </button>
    </div>
  );

  const { segments } = route;
  let page;
  if (segments[0] === 'panes' && segments[1]) {
    page = <TerminalPage paneId={segments[1]} me={me} />;
  } else {
    page = <PanesPage me={me} />;
  }
  return (
    <>
      {banner}
      {page}
    </>
  );
}
