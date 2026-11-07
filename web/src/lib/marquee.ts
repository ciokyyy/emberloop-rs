/** marqueeShift is how far a folded title slides to show its tail, or null when it fits. */
export function marqueeShift(boxWidth: number, textWidth: number): { shift: number; seconds: number } | null {
  const over = Math.ceil(textWidth - boxWidth);
  if (over <= 0) return null;
  return { shift: -over, seconds: Math.min(12, Math.max(3, over / 25)) };
}
