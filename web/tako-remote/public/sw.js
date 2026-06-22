const CACHE = 'tako-remote-v2';
const SHELL = ['/', '/index.html', '/manifest.json', '/icons/icon.svg'];

self.addEventListener('install', e => {
  e.waitUntil(caches.open(CACHE).then(c => c.addAll(SHELL)));
  self.skipWaiting();
});

self.addEventListener('activate', e => {
  e.waitUntil(
    caches.keys().then(keys =>
      Promise.all(keys.filter(k => k !== CACHE).map(k => caches.delete(k)))
    )
  );
  self.clients.claim();
});

self.addEventListener('fetch', e => {
  const url = new URL(e.request.url);

  // API はキャッシュしない
  if (url.pathname.startsWith('/api/')) return;

  // Vite ビルドの hashed アセット（/assets/xxx-hash.js 等）は Cache First
  if (url.pathname.startsWith('/assets/')) {
    e.respondWith(
      caches.match(e.request).then(hit => {
        if (hit) return hit;
        return fetch(e.request).then(resp => {
          if (resp.ok) {
            const clone = resp.clone();
            caches.open(CACHE).then(c => c.put(e.request, clone));
          }
          return resp;
        });
      })
    );
    return;
  }

  // それ以外はネットワーク優先、失敗したらキャッシュ
  e.respondWith(
    fetch(e.request)
      .then(resp => {
        if (resp.ok) {
          const clone = resp.clone();
          caches.open(CACHE).then(c => c.put(e.request, clone));
        }
        return resp;
      })
      .catch(() => caches.match(e.request).then(hit => hit || caches.match('/')))
  );
});
