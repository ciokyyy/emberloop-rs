import { expect, it } from 'vitest';
import { marqueeShift } from './marquee';

it('does not move a title that fits', () => {
  expect(marqueeShift(200, 200)).toBeNull();
  expect(marqueeShift(200, 40)).toBeNull();
});

it('slides a folded title by its overflow, slower the longer it is', () => {
  expect(marqueeShift(100, 300)).toEqual({ shift: -200, seconds: 8 });
  expect(marqueeShift(100, 3000)).toEqual({ shift: -2900, seconds: 12 });
});
