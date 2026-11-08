// Logos from @lobehub/icons-static-svg (MIT), inlined at build time as raw SVG strings.
// Providers use the monochrome glyph (fill currentColor, so it follows the text colour).
// Harnesses (claude, agy, pi) keep their brand colour; pi has no colour variant, so it takes PI_TINT.
import anthropic from '@lobehub/icons-static-svg/icons/anthropic.svg?raw';
import claudeColor from '@lobehub/icons-static-svg/icons/claude-color.svg?raw';
import antigravityColor from '@lobehub/icons-static-svg/icons/antigravity-color.svg?raw';
import pi from '@lobehub/icons-static-svg/icons/pi.svg?raw';
import antgroup from '@lobehub/icons-static-svg/icons/antgroup.svg?raw';
import baseten from '@lobehub/icons-static-svg/icons/baseten.svg?raw';
import cerebras from '@lobehub/icons-static-svg/icons/cerebras.svg?raw';
import deepseek from '@lobehub/icons-static-svg/icons/deepseek.svg?raw';
import fireworks from '@lobehub/icons-static-svg/icons/fireworks.svg?raw';
import groq from '@lobehub/icons-static-svg/icons/groq.svg?raw';
import huggingface from '@lobehub/icons-static-svg/icons/huggingface.svg?raw';
import kimi from '@lobehub/icons-static-svg/icons/kimi.svg?raw';
import minimax from '@lobehub/icons-static-svg/icons/minimax.svg?raw';
import mistral from '@lobehub/icons-static-svg/icons/mistral.svg?raw';
import moonshot from '@lobehub/icons-static-svg/icons/moonshot.svg?raw';
import nvidia from '@lobehub/icons-static-svg/icons/nvidia.svg?raw';
import openai from '@lobehub/icons-static-svg/icons/openai.svg?raw';
import openrouter from '@lobehub/icons-static-svg/icons/openrouter.svg?raw';
import opencode from '@lobehub/icons-static-svg/icons/opencode.svg?raw';
import qwen from '@lobehub/icons-static-svg/icons/qwen.svg?raw';
import together from '@lobehub/icons-static-svg/icons/together.svg?raw';
import vercel from '@lobehub/icons-static-svg/icons/vercel.svg?raw';
import xai from '@lobehub/icons-static-svg/icons/xai.svg?raw';
import xiaomi from '@lobehub/icons-static-svg/icons/xiaomimimo.svg?raw';
import zai from '@lobehub/icons-static-svg/icons/zai.svg?raw';

/** Harness brand colour for the monochrome `pi` glyph; matches the sidebar's `--pi` token. */
export const PI_TINT = '#8b7cf6';

const BRANDS: Record<string, string> = {
  // Harnesses (coloured)
  claude: claudeColor,
  agy: antigravityColor,
  pi,
  // Providers (monochrome); every id the daemon lists in /api/providers
  anthropic,
  'ant-ling': antgroup,
  baseten,
  cerebras,
  deepseek,
  fireworks,
  groq,
  huggingface,
  'kimi-coding': kimi,
  minimax,
  'minimax-cn': minimax,
  mistral,
  moonshotai: moonshot,
  'moonshotai-cn': moonshot,
  nvidia,
  openai,
  'openai-codex': openai,
  openrouter,
  'opencode-go': opencode,
  'qwen-token-plan': qwen,
  'qwen-token-plan-cn': qwen,
  'qwen-token-plan-individual': qwen,
  together,
  'vercel-ai-gateway': vercel,
  xai,
  xiaomi,
  'xiaomi-token-plan-ams': xiaomi,
  'xiaomi-token-plan-cn': xiaomi,
  'xiaomi-token-plan-sgp': xiaomi,
  zai,
  'zai-coding-cn': zai
};

const COLOURED = new Set(['claude', 'agy', 'pi']);

/** Raw SVG for a provider or harness id, or undefined when no logo exists. */
export function brandSvg(id: string): string | undefined {
  return BRANDS[id];
}

/** Text colour for a brand: a harness tint for pi, otherwise inherit (monochrome follows text). */
export function brandTint(id: string): string | undefined {
  return id === 'pi' ? PI_TINT : undefined;
}

/** Whether the brand keeps its own colours (harness) instead of following the text colour. */
export function isColoured(id: string): boolean {
  return COLOURED.has(id);
}
