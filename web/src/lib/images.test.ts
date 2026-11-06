import { describe, expect, it } from 'vitest';
import { MAX_IMAGE_BYTES, MAX_IMAGES, pickImages, type FileLike } from './images';

const file = (name: string, type: string, size = 1024): FileLike => ({ name, type, size });

describe('image attachments', () => {
  it('accepts png, jpeg, webp and gif', () => {
    const result = pickImages([], [
      file('a.png', 'image/png'),
      file('b.jpg', 'image/jpeg'),
      file('c.webp', 'image/webp'),
      file('d.gif', 'image/gif')
    ]);
    expect(result.accepted).toHaveLength(4);
    expect(result.rejected).toEqual([]);
  });

  it('rejects non-images and files over 10 MB', () => {
    const result = pickImages([], [
      file('notes.txt', 'text/plain'),
      file('huge.png', 'image/png', MAX_IMAGE_BYTES + 1)
    ]);
    expect(result.accepted).toEqual([]);
    expect(result.rejected.map((r) => r.reason)).toEqual(['type', 'size']);
  });

  it('stops at five attachments', () => {
    const existing = [1, 2, 3, 4].map((n) => ({ file: file(`${n}.png`, 'image/png') }));
    const result = pickImages(existing, [file('5.png', 'image/png'), file('6.png', 'image/png')]);
    expect(result.accepted).toHaveLength(1);
    expect(result.rejected).toEqual([{ name: '6.png', reason: 'limit' }]);
    expect(MAX_IMAGES).toBe(5);
  });
});
