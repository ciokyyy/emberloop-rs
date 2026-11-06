const MISC = { glyph: '·', className: 'text-tool-misc' };

const APPEARANCE: Record<string, { glyph: string; className: string }> = {
  bash: { glyph: '$', className: 'text-tool-bash' },
  background: { glyph: '▶', className: 'text-tool-bash' },
  read: { glyph: '≡', className: 'text-tool-read' },
  edit: { glyph: '✎', className: 'text-tool-edit' },
  grep: { glyph: '⌕', className: 'text-tool-grep' }
};

export function toolAppearance(name = ''): { glyph: string; className: string } {
  return APPEARANCE[name] ?? MISC;
}
