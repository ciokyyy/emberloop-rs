// fallow-ignore-file unused-file
// Push only: no `fetch` handler, so nothing is cached and a new shell is never hidden behind an
// old service worker. The page owns in-app notices; this owns the alert while the app is closed.
self.addEventListener('push', (event) => {
  if (!event.data) return;
  const { title, body, url } = event.data.json();
  event.waitUntil(
    self.registration.showNotification(title, {
      body,
      icon: '/icon-192.png',
      badge: '/icon-192.png',
      data: { url },
      tag: url
    })
  );
});

self.addEventListener('notificationclick', (event) => {
  event.notification.close();
  const url = new URL(event.notification.data?.url ?? '/', self.location.origin).href;
  event.waitUntil(
    (async () => {
      const open = await self.clients.matchAll({ type: 'window', includeUncontrolled: true });
      const focused = open.find((client) => client.url.split('#')[0] === url.split('#')[0]);
      if (focused) return focused.focus();
      return self.clients.openWindow(url);
    })()
  );
});
