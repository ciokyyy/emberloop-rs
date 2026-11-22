import type { PromptFile, PromptImage } from './api/types';

export const MAX_IMAGES = 5;
export const MAX_IMAGE_BYTES = 10 * 1024 * 1024;
const IMAGE_TYPES = ['image/png', 'image/jpeg', 'image/webp', 'image/gif'];

export interface FileLike {
  name: string;
  type: string;
  size: number;
}

export interface ImageAttachment<T extends FileLike = FileLike> {
  file: T;
  url?: string;
}

export type RejectReason = 'type' | 'size' | 'limit';

export function pickImages<T extends FileLike>(
  existing: ImageAttachment<T>[],
  incoming: Iterable<T>
): { accepted: ImageAttachment<T>[]; rejected: { name: string; reason: RejectReason }[] } {
  const accepted: ImageAttachment<T>[] = [];
  const rejected: { name: string; reason: RejectReason }[] = [];
  let count = existing.length;
  for (const file of incoming) {
    if (!IMAGE_TYPES.includes(file.type)) rejected.push({ name: file.name, reason: 'type' });
    else if (file.size > MAX_IMAGE_BYTES) rejected.push({ name: file.name, reason: 'size' });
    else if (count >= MAX_IMAGES) rejected.push({ name: file.name, reason: 'limit' });
    else {
      accepted.push({ file });
      count += 1;
    }
  }
  return { accepted, rejected };
}

/** Images reach the model; every other file is only saved on the daemon side. */
export function splitAttachments<T extends FileLike>(
  incoming: Iterable<T>
): { images: T[]; files: T[] } {
  const images: T[] = [];
  const files: T[] = [];
  for (const file of incoming) (IMAGE_TYPES.includes(file.type) ? images : files).push(file);
  return { images, files };
}

function toBase64(file: File): Promise<string> {
  return new Promise((resolve, reject) => {
    const reader = new FileReader();
    reader.onerror = () => reject(reader.error ?? new Error('Could not read the file'));
    reader.onload = () => {
      const result = String(reader.result ?? '');
      resolve(result.slice(result.indexOf(',') + 1));
    };
    reader.readAsDataURL(file);
  });
}

export async function toPromptImage(file: File): Promise<PromptImage> {
  return { mimeType: file.type, dataBase64: await toBase64(file), name: file.name };
}

export async function toPromptFile(file: File): Promise<PromptFile> {
  return { name: file.name, dataBase64: await toBase64(file) };
}
