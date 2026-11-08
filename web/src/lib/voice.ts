// Groq's upload cap; the daemon enforces the same limit (MAX_AUDIO_BYTES).
export const MAX_AUDIO_BYTES = 25 * 1024 * 1024;

const RECORDING_TYPES = ['audio/webm;codecs=opus', 'audio/webm', 'audio/mp4'];

/** First recording type the browser can make; `undefined` lets MediaRecorder choose. */
export function pickAudioType(supports: (type: string) => boolean): string | undefined {
  return RECORDING_TYPES.find(supports);
}

/** Put `spoken` where the selection was, with a space if the text before it has none. */
export function insertAt(text: string, start: number, end: number, spoken: string): string {
  const before = text.slice(0, start);
  const lead = before && !/\s$/.test(before) ? ' ' : '';
  return before + lead + spoken + text.slice(end);
}
