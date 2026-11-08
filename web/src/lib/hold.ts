type Hold = {
  lift: () => void;
  move: (event: PointerEvent) => void;
  drop: (event: PointerEvent, lifted: boolean) => void;
};

/**
 * Press and hold `ms` to lift, then drag and release. Moving before the lift cancels the hold,
 * so a scroll or a select passes through.
 */
export function holdToLift(start: PointerEvent, ms: number, hold: Hold) {
  let lifted = false;
  const timer = setTimeout(() => {
    lifted = true;
    hold.lift();
  }, ms);
  const end = (event: PointerEvent) => {
    clearTimeout(timer);
    window.removeEventListener('pointermove', onMove);
    window.removeEventListener('pointerup', end);
    window.removeEventListener('pointercancel', end);
    hold.drop(event, lifted);
  };
  const onMove = (event: PointerEvent) => {
    if (lifted) hold.move(event);
    else if (Math.hypot(event.clientX - start.clientX, event.clientY - start.clientY) > 6) end(event);
  };
  window.addEventListener('pointermove', onMove);
  window.addEventListener('pointerup', end);
  window.addEventListener('pointercancel', end);
}
