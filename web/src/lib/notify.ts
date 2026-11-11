// Notifications: the alert while the app is open (a sound from the page) and the one while it is
// closed (a push from the daemon). The daemon only pushes when this page stopped reporting the
// Session, so exactly one of the two fires.
import { createUISFX, type UISFXPlayer } from 'uisfx';

const base = String(import.meta.env.VITE_API_BASE ?? '').replace(/\/$/, '');
/// How often an open page tells the daemon it still shows this Session; the daemon's grace is 30 s.
const HEARTBEAT_MS = 20_000;

let player: UISFXPlayer | null = null;
let heartbeat: ReturnType<typeof setInterval> | null = null;

/** uisfx may only start an AudioContext from a trusted gesture, and a reload starts it suspended. */
export function unlockSound() {
  player ??= createUISFX({ pack: 'minimal', volume: 0.7 });
  void player.unlock().catch(() => {});
}

/** One sound per finished turn or error, from the same cue family that carries its notice. */
export function playAlert(failed: boolean) {
  unlockSound();
  player?.play(failed ? 'error' : 'success');
}

export async function registerWorker() {
  if (!navigator.serviceWorker) return;
  try {
    await navigator.serviceWorker.register('/sw.js');
  } catch {
    // Without a worker the app still runs; it only loses the alert while closed.
  }
}

async function postJson(path: string, body: unknown) {
  const response = await fetch(base + path, {
    method: 'POST',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify(body)
  });
  if (!response.ok) throw new Error(`${response.status} ${await response.text()}`);
}

function keyBytes(encoded: string): Uint8Array<ArrayBuffer> {
  const raw = atob(encoded.replace(/-/g, '+').replace(/_/g, '/'));
  const bytes = new Uint8Array(new ArrayBuffer(raw.length));
  for (let at = 0; at < raw.length; at += 1) bytes[at] = raw.charCodeAt(at);
  return bytes;
}

/** Asks for permission and stores this browser's subscription; must run from a user gesture. */
export async function enablePush(): Promise<'ok' | 'denied' | 'unsupported' | 'failed'> {
  if (!('serviceWorker' in navigator) || !('PushManager' in window)) return 'unsupported';
  void registerWorker();
  const permission = await Notification.requestPermission();
  if (permission !== 'granted') return 'denied';
  try {
    const { key } = (await (await fetch(base + '/api/push/key')).json()) as { key: string };
    const registration = await navigator.serviceWorker.ready;
    const subscription = await registration.pushManager.subscribe({
      userVisibleOnly: true,
      applicationServerKey: keyBytes(key)
    });
    const json = subscription.toJSON() as { endpoint?: string; keys?: Record<string, string> };
    await postJson('/api/push/subscribe', {
      endpoint: json.endpoint,
      p256dh: json.keys?.p256dh,
      auth: json.keys?.auth
    });
    return 'ok';
  } catch {
    return 'failed';
  }
}

/** A browser that already subscribed, so Settings can show the state without asking again. */
export async function pushSubscribed(): Promise<boolean> {
  if (!('serviceWorker' in navigator)) return false;
  const registration = await navigator.serviceWorker.getRegistration('/');
  const subscription = await registration?.pushManager.getSubscription();
  return Boolean(subscription);
}

export async function disablePush() {
  const registration = await navigator.serviceWorker.getRegistration('/');
  const subscription = await registration?.pushManager.getSubscription();
  if (!subscription) return;
  await postJson('/api/push/unsubscribe', { endpoint: subscription.endpoint });
  await subscription.unsubscribe();
}

/** The Settings row's gesture: it subscribes when needed, then asks the daemon for one alert. */
export async function sendTestPush(): Promise<'ok' | 'denied' | 'unsupported' | 'failed'> {
  if (!(await pushSubscribed())) {
    const enabled = await enablePush();
    if (enabled !== 'ok') return enabled;
  }
  try {
    await postJson('/api/push/test', {});
    return 'ok';
  } catch {
    return 'failed';
  }
}

/** Reports the Session this page shows while it is visible; the daemon needs no push for it then. */
export function reportVisible(sessionId: string | null) {
  if (heartbeat) clearInterval(heartbeat);
  heartbeat = null;
  if (!sessionId) return;
  const beat = () => {
    if (document.visibilityState !== 'visible') return;
    void postJson('/api/push/visible', { sessionId }).catch(() => {});
  };
  beat();
  heartbeat = setInterval(beat, HEARTBEAT_MS);
}
