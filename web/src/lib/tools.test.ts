import { describe, expect, it } from 'vitest';
import { toolAppearance } from './tools';

describe('tool appearance', () => {
  it('gives each core tool its hue and glyph', () => {
    expect(toolAppearance('bash')).toMatchObject({ glyph: '$', className: 'text-tool-bash' });
    expect(toolAppearance('read')).toMatchObject({ glyph: '≡', className: 'text-tool-read' });
    expect(toolAppearance('edit')).toMatchObject({ glyph: '✎', className: 'text-tool-edit' });
    expect(toolAppearance('grep')).toMatchObject({ glyph: '⌕', className: 'text-tool-grep' });
  });

  it('treats background as bash and unknown tools as misc', () => {
    expect(toolAppearance('background').className).toBe('text-tool-bash');
    expect(toolAppearance('find').className).toBe('text-tool-misc');
  });
});
