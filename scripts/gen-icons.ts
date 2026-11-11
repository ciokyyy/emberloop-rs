#!/usr/bin/env bun
// Renders the installable-app icons from the brand image. The platform owns the rounding, so every
// icon is full-bleed background with the mark inside the maskable 80% safe circle, and any Android
// mask, iOS squircle or favicon crop keeps the whole mark on colour.
//
//   scripts/gen-icons           write web/static/
//   scripts/gen-icons --check   fail when the committed icons differ from a fresh render
import { deflateSync, inflateSync } from 'node:zlib';
import { readFileSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';

const root = join(import.meta.dir, '..');
const source = join(root, 'docs/logo.png');
const out = join(root, 'web/static');
const background = [15, 17, 23]; // the mark's own background: a mask must crop colour, not holes
const safeCircleDiameter = 0.8; // any mask keeps the central 80% of the width; the mark fits inside

export type Image = { width: number; height: number; data: Uint8Array };

function crc32(bytes: Uint8Array): number {
  let crc = 0xffffffff;
  for (const byte of bytes) {
    crc ^= byte;
    for (let bit = 0; bit < 8; bit += 1) crc = crc & 1 ? (crc >>> 1) ^ 0xedb88320 : crc >>> 1;
  }
  return (crc ^ 0xffffffff) >>> 0;
}

function chunk(kind: string, body: Buffer): Buffer {
  const length = Buffer.alloc(4);
  length.writeUInt32BE(body.length);
  const payload = Buffer.concat([Buffer.from(kind, 'latin1'), body]);
  const crc = Buffer.alloc(4);
  crc.writeUInt32BE(crc32(payload));
  return Buffer.concat([length, payload, crc]);
}

export function encode({ width, height, data }: Image): Buffer {
  const header = Buffer.alloc(13);
  header.writeUInt32BE(width, 0);
  header.writeUInt32BE(height, 4);
  header.set([8, 6, 0, 0, 0], 8); // 8-bit RGBA, deflate, filter 0, no interlace
  const stride = width * 4;
  const raw = Buffer.alloc((stride + 1) * height);
  for (let y = 0; y < height; y += 1) {
    Buffer.from(data.subarray(y * stride, (y + 1) * stride)).copy(raw, y * (stride + 1) + 1);
  }
  return Buffer.concat([
    Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]),
    chunk('IHDR', header),
    chunk('IDAT', deflateSync(raw, { level: 9 })),
    chunk('IEND', Buffer.alloc(0))
  ]);
}

export function decode(png: Buffer, name = 'png'): Image {
  let at = 8;
  let ihdr: Buffer | null = null;
  const parts: Buffer[] = [];
  while (at < png.length) {
    const length = png.readUInt32BE(at);
    const kind = png.toString('latin1', at + 4, at + 8);
    if (kind === 'IHDR') ihdr = png.subarray(at + 8, at + 21);
    else if (kind === 'IDAT') parts.push(png.subarray(at + 8, at + 8 + length));
    else if (kind === 'IEND') break;
    at += 12 + length;
  }
  if (!ihdr) throw new Error(`${name}: no IHDR`);
  const width = ihdr.readUInt32BE(0);
  const height = ihdr.readUInt32BE(4);
  if (ihdr[8] !== 8 || ihdr[9] !== 6 || ihdr[12] !== 0) {
    throw new Error(`${name}: only 8-bit RGBA non-interlaced PNG is supported`);
  }
  const raw = inflateSync(Buffer.concat(parts));
  const stride = width * 4;
  const data = new Uint8Array(stride * height);
  for (let y = 0; y < height; y += 1) {
    const filter = raw[y * (stride + 1)];
    const line = raw.subarray(y * (stride + 1) + 1, (y + 1) * (stride + 1));
    for (let x = 0; x < stride; x += 1) {
      const left = x >= 4 ? data[y * stride + x - 4] : 0;
      const up = y > 0 ? data[(y - 1) * stride + x] : 0;
      const corner = x >= 4 && y > 0 ? data[(y - 1) * stride + x - 4] : 0;
      let value = line[x];
      if (filter === 1) value += left;
      else if (filter === 2) value += up;
      else if (filter === 3) value += (left + up) >> 1;
      else if (filter === 4) {
        const estimate = left + up - corner;
        const [a, b, c] = [
          Math.abs(estimate - left),
          Math.abs(estimate - up),
          Math.abs(estimate - corner)
        ];
        value += a <= b && a <= c ? left : b <= c ? up : corner;
      } else if (filter !== 0) throw new Error(`${name}: unknown filter ${filter}`);
      data[y * stride + x] = value & 0xff;
    }
  }
  return { width, height, data };
}

/** The mark's own area: the brand image is mostly background, so the mark is cropped and fitted. */
export function markBox({ width, height, data }: Image) {
  let [left, top, right, bottom] = [width, height, -1, -1];
  for (let y = 0; y < height; y += 1) {
    for (let x = 0; x < width; x += 1) {
      const at = (y * width + x) * 4;
      const distance = Math.max(
        ...[0, 1, 2].map((c) => Math.abs(data[at + c] - background[c]))
      );
      if (distance <= 24) continue;
      [left, top] = [Math.min(left, x), Math.min(top, y)];
      [right, bottom] = [Math.max(right, x), Math.max(bottom, y)];
    }
  }
  if (right < 0) throw new Error('the brand image holds no mark above the background');
  const side = Math.max(right - left + 1, bottom - top + 1) * 1.08; // a little air around the mark
  return { x: (left + right + 1) / 2 - side / 2, y: (top + bottom + 1) / 2 - side / 2, side };
}

/** Draws the mark, fitted to the maskable safe circle and centred, over the full-bleed background. */
export function render(art: Image, size: number): Image {
  const data = new Uint8Array(size * size * 4);
  const box = markBox(art);
  const step = (size * safeCircleDiameter) / box.side;
  const offset = [(size - box.side * step) / 2, (size - box.side * step) / 2];
  for (let y = 0; y < size; y += 1) {
    for (let x = 0; x < size; x += 1) {
      const mean = [0, 0, 0, 0];
      for (const [dy, dx] of [[0.25, 0.25], [0.25, 0.75], [0.75, 0.25], [0.75, 0.75]]) {
        const sx = Math.min(art.width - 1, Math.max(0, Math.floor(box.x + (x + dx - offset[0]) / step)));
        const sy = Math.min(art.height - 1, Math.max(0, Math.floor(box.y + (y + dy - offset[1]) / step)));
        const at = (sy * art.width + sx) * 4;
        const alpha = art.data[at + 3] / 255;
        for (let c = 0; c < 3; c += 1) mean[c] += art.data[at + c] * alpha;
        mean[3] += alpha;
      }
      const at = (y * size + x) * 4;
      const alpha = mean[3] / 4;
      for (let c = 0; c < 3; c += 1) {
        // 6 bits per channel: the source dithers per pixel, and flattening it keeps the PNG small.
        data[at + c] = Math.round(mean[c] / 4 + background[c] * (1 - alpha)) & 0xfc;
      }
      data[at + 3] = 255;
    }
  }
  return { width: size, height: size, data };
}

/** Everything outside the 80% safe circle must be background, and no pixel may be transparent. */
export function check(image: Image, name: string) {
  const radius = (safeCircleDiameter / 2) * image.width;
  const centre = image.width / 2;
  let worst = 0;
  for (let y = 0; y < image.height; y += 1) {
    for (let x = 0; x < image.width; x += 1) {
      const at = (y * image.width + x) * 4;
      if (image.data[at + 3] !== 255) throw new Error(`${name}: (${x},${y}) is transparent`);
      if (Math.hypot(x + 0.5 - centre, y + 0.5 - centre) <= radius) continue;
      for (let c = 0; c < 3; c += 1) worst = Math.max(worst, Math.abs(image.data[at + c] - background[c]));
    }
  }
  if (worst > 6) throw new Error(`${name}: the mark leaves the maskable safe circle (${worst})`);
}

function ico(png: Buffer, size: number): Buffer {
  const header = Buffer.alloc(22);
  header.writeUInt16LE(1, 2); // icon
  header.writeUInt16LE(1, 4); // one image
  header.set([size === 256 ? 0 : size, size === 256 ? 0 : size, 0, 0, 1, 0, 32, 0], 6);
  header.writeUInt32LE(png.length, 14);
  header.writeUInt32LE(22, 18);
  return Buffer.concat([header, png]);
}

const icons: Record<string, Buffer> = {};
const art = decode(readFileSync(source), source);
for (const [name, size] of Object.entries({
  'icon-192.png': 192,
  'icon-512.png': 512,
  'icon-maskable-512.png': 512,
  'apple-touch-icon.png': 180
})) {
  const image = render(art, size);
  check(image, name);
  icons[name] = encode(image);
}
const favicon = render(art, 32);
check(favicon, 'favicon.ico');
icons['favicon.ico'] = ico(encode(favicon), 32);

const published = process.argv.includes('--check')
  ? Object.entries(icons).flatMap(([name, png]) => {
      const path = join(out, name);
      try {
        return Buffer.compare(readFileSync(path), png) === 0 ? [] : [name];
      } catch {
        return [name];
      }
    })
  : [];
if (process.argv.includes('--check')) {
  if (published.length) throw new Error(`stale icons: ${published.join(', ')}; run scripts/gen-icons`);
  console.log(`icons match docs/logo.png (${Object.keys(icons).length} files)`);
} else {
  for (const [name, png] of Object.entries(icons)) writeFileSync(join(out, name), png);
  console.log(`wrote ${Object.keys(icons).join(', ')} to web/static/`);
}
