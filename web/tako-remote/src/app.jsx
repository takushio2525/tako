import { useState, useEffect } from 'preact/hooks';
import { ConnectPage } from './pages/connect';
import { MachinesPage } from './pages/machines';
import { PanesPage } from './pages/panes';
import { TerminalPage } from './pages/terminal';

function parseRoute() {
  const hash = window.location.hash.slice(1) || '/';
  const [path, qs] = hash.split('?');
  return {
    path,
    params: new URLSearchParams(qs || ''),
    segments: path.split('/').filter(Boolean),
  };
}

export function App() {
  const [route, setRoute] = useState(parseRoute);

  useEffect(() => {
    const onChange = () => setRoute(parseRoute());
    window.addEventListener('hashchange', onChange);
    return () => window.removeEventListener('hashchange', onChange);
  }, []);

  // QR からの直接アクセス対応: pathname や search params に接続情報があればハッシュルートへ転送
  useEffect(() => {
    const sp = new URLSearchParams(window.location.search);
    const host = sp.get('host');
    const token = sp.get('token');
    if (host && token) {
      const machine = sp.get('machine') || `m-${Date.now()}`;
      const name = sp.get('name') || '';
      window.location.replace(
        `${window.location.pathname}#/connect?host=${encodeURIComponent(host)}&token=${encodeURIComponent(token)}&machine=${encodeURIComponent(machine)}&name=${encodeURIComponent(name)}`
      );
    }
  }, []);

  const { segments } = route;

  if (segments[0] === 'connect') {
    return <ConnectPage params={route.params} />;
  }
  if (segments[0] === 'panes' && segments[1]) {
    return <TerminalPage paneId={parseInt(segments[1], 10)} />;
  }
  if (segments[0] === 'panes') {
    return <PanesPage />;
  }
  return <MachinesPage />;
}
