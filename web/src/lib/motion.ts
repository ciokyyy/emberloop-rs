import { fade, slide as slideHeight, type TransitionConfig } from 'svelte/transition';

// Ported from emberloop-go `motion.js`. The CSS tokens (--dur-*, --ease-smooth)
// own every duration and easing; reduced motion or a missing token yields a zero
// duration, so the end state wins at once without a second code path. Parameters
// are read when a transition runs, so call the helpers: `out:fade={motionParams()}`.
// Not ported: stagger, skeletonSwap, swapParams (nothing here uses them yet).

// flip is Svelte's reorder animation; `animate:flip` compiles to a call of this binding.
export { flip } from 'svelte/animate';

type Step = 'instant' | 'standard' | 'emphasis';
type SlideOptions = { axis?: 'x' | 'y'; amount?: number; step?: Step; delay?: number; skip?: boolean };

const DURATION_TOKENS: Record<Step, string> = {
  instant: '--dur-instant',
  standard: '--dur-standard',
  emphasis: '--dur-emphasis'
};

const bezier = (p1: number, p2: number, t: number) => {
  const u = 1 - t;
  return 3 * u * u * t * p1 + 3 * u * t * t * p2 + t * t * t;
};

// cubicBezier turns a CSS cubic-bezier() token into an easing; bisection on x finds t.
function cubicBezier(value: string): (t: number) => number {
  const numbers = value.match(/-?\d*\.?\d+/g)?.map(Number) ?? [];
  if (numbers.length !== 4) return (t) => t;
  const [x1, y1, x2, y2] = numbers;
  return (t) => {
    if (t <= 0 || t >= 1) return t;
    let low = 0;
    let high = 1;
    for (let i = 0; i < 16; i += 1) {
      const mid = (low + high) / 2;
      if (bezier(x1, x2, mid) < t) low = mid;
      else high = mid;
    }
    return bezier(y1, y2, (low + high) / 2);
  };
}

export function prefersReducedMotion(): boolean {
  if (typeof window === 'undefined' || typeof window.matchMedia !== 'function') return false;
  return Boolean(window.matchMedia('(prefers-reduced-motion: reduce)').matches);
}

const token = (name: string) =>
  typeof document === 'undefined' ? '' : getComputedStyle(document.documentElement).getPropertyValue(name);

export function motionParams(step: Step = 'standard') {
  const ms = Number.parseFloat(token(DURATION_TOKENS[step]));
  const duration = prefersReducedMotion() || !(ms > 0) ? 0 : ms;
  return { duration, easing: cubicBezier(token('--ease-smooth')) };
}

// slide: opacity plus a short offset on one axis; `delay` is the place in a stagger.
export function slide(_node: Element | null, options: SlideOptions = {}): TransitionConfig {
  const { axis = 'y', amount = 8, step = 'emphasis', delay = 0 } = options;
  const { duration, easing } = motionParams(step);
  return {
    duration,
    delay,
    easing,
    css: (t) => {
      const travel = (1 - t) * amount;
      return `opacity: ${t}; transform: ${axis === 'x' ? `translateX(${travel}px)` : `translateY(${travel}px)`};`;
    }
  };
}

// `skip` is for a block that must not animate (a bulk-loaded history).
const travel = (node: Element | null, { skip = false, ...options }: SlideOptions = {}) =>
  skip ? ({ duration: 0 } as TransitionConfig) : slide(node, options);

export const rise = (node: Element | null, options?: SlideOptions) => travel(node, { amount: 8, ...options });
export const slideUp = (node: Element | null, options?: SlideOptions) => travel(node, { amount: 10, ...options });
export const slideDown = (node: Element | null, options?: SlideOptions) => travel(node, { amount: -8, ...options });

export function fadeAt(node: Element, options?: { step?: Step; skip?: boolean }): TransitionConfig {
  if (options?.skip) return { duration: 0 };
  return fade(node, motionParams(options?.step ?? 'standard'));
}

// collapse gives up the row's height as it leaves; Svelte measures, the scale times it.
export function collapse(node: Element, options?: { step?: Step }) {
  return slideHeight(node, motionParams(options?.step ?? 'standard'));
}

// flipAnimation: animate:flip parameters; rows change place at once under reduced motion.
export function flipAnimation() {
  const { duration, easing } = motionParams('standard');
  return duration ? { duration, easing } : { duration: 0 };
}

export const rowEnter: SlideOptions = { amount: 8, step: 'standard' };

// flipReady: a test DOM has no getAnimations and a reordered row must still move.
export function flipReady(node: Element) {
  if (typeof node.getAnimations !== 'function') node.getAnimations = () => [];
}
