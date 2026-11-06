// Regenerates golden/*.json from the reference kernel (acp-kernel, MIT).
// Usage: ./regen.sh (clones and builds the commit in BASELINE, then runs this with KERNEL_DIR set)
// Not run by scripts/check. A reviewer runs it after the Versions panel reports that
// billion-context moved, then reads the diff of golden/.
import { writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const kernelDir = process.env.KERNEL_DIR;
if (!kernelDir) throw new Error("KERNEL_DIR is not set; run ./regen.sh");
const kernel = await import(join(kernelDir, "dist/index.js"));
const { createCore, createInitialState, defaultConfig, renderNudgeText, defaultPrompts } = kernel;
const { readFileSync } = await import("node:fs");
const kernelVersion = JSON.parse(readFileSync(join(kernelDir, "package.json"), "utf8")).version;

// Fixed tokens-per-character so the Rust side can reproduce counts exactly.
const tokensOf = (messages) =>
  messages.reduce((sum, m) => sum + Math.ceil((m.text ?? "").length / 4), 0);

// Appends one (assistant tool call, tool result) pair per step; returns the new messages.
function appendStep(messages, i, toolName, bytesPerStep) {
  messages.push({
    id: `a${i}`,
    role: "assistant",
    contentType: "tool-call",
    toolName,
    toolCallId: `c${i}`,
    text: `${toolName} file ${i}`,
  });
  messages.push({
    id: `r${i}`,
    role: "tool",
    contentType: "tool-result",
    toolName,
    toolCallId: `c${i}`,
    text: "x".repeat(bytesPerStep),
  });
}

function safeRender(nudge) {
  try {
    const rendered = renderNudgeText(nudge, defaultPrompts);
    return typeof rendered === "string" ? rendered : (rendered?.text ?? JSON.stringify(rendered));
  } catch (error) {
    return `render error: ${error.message}`;
  }
}

// One scenario: a growing conversation, optional compress calls at given steps, and per-step frames.
// compressAt: step -> [startRef, endRef] applied before that step's processTurn (the host's compress tool).
// withStatus: record core.status() per step.
function run(name, windowTokens, { steps, bytesPerStep, toolName = "read", overrides = {}, compressAt = {}, withStatus = false }) {
  const core = createCore();
  const config = defaultConfig(windowTokens, overrides);
  let state = createInitialState();
  const messages = [{ id: "u0", role: "user", contentType: "text", text: "start the task" }];
  const frames = [];
  const compresses = [];
  for (let i = 1; i <= steps; i++) {
    if (compressAt[i]) {
      const [startRef, endRef] = compressAt[i];
      const applied = core.applyCompression({
        ranges: [{ startRef, endRef, summary: `Summary of ${startRef}..${endRef}: the earlier file reads were reviewed and their key facts recorded.` }],
        messages,
        state,
        config,
      });
      state = applied.state;
      compresses.push({ step: i, startRef, endRef, blocksCreated: applied.result.blocksCreated, errors: applied.result.errors, warnings: applied.result.warnings });
    }
    appendStep(messages, i, toolName, bytesPerStep);
    // A host reports the size of what it really sends: the history with folded blocks pruned.
    // A first pass over the raw history gives that pruned view, the second uses its size.
    const pruned = core.processTurn({ messages, state, config, tokenCount: tokensOf(messages) });
    const tokenCount = tokensOf(pruned.messages);
    const result = core.processTurn({ messages, state, config, tokenCount });
    state = result.state;
    const nudge = result.nudge;
    const frame = {
      step: i,
      tokenCount,
      shouldInject: !!nudge?.shouldInject,
      reason: nudge?.reason ?? null,
      tier: nudge?.tier ?? null,
      overLimit: nudge?.breakdown?.overLimit ?? null,
      emergencyOverride: nudge?.breakdown?.emergencyOverride ?? null,
      growth: nudge?.breakdown?.growth ?? null,
      text: nudge?.shouldInject ? safeRender(nudge) : null,
    };
    if (withStatus) frame.status = core.status(state, tokenCount, config);
    frames.push(frame);
  }
  return {
    name,
    kernelVersion,
    windowTokens,
    steps,
    bytesPerStep,
    toolName,
    overrides,
    compresses,
    frames,
  };
}

// Tier triggers lowered from the reference default (1000/2000) so tier-2/3 fire inside a short run.
const lowTiers = { tiers: { enabled: true, tier2Trigger: 2, tier3Trigger: 4 } };

const scenarios = [
  run("window-200k-steady", 200_000, { steps: 60, bytesPerStep: 6_000 }),
  run("window-1m-steady", 1_000_000, { steps: 80, bytesPerStep: 20_000 }),
  run("window-64k-steady", 64_000, { steps: 40, bytesPerStep: 3_000 }),
  // 64000 x 0.75 = 48000 and 64000 x 0.95 = 60800: steps x 750 tokens crosses both.
  run("window-64k-pressure", 64_000, { steps: 90, bytesPerStep: 3_000, withStatus: true }),
  // Same growth, but read is protected: no compressible mass, so no nudge may fire.
  run("window-64k-protected", 64_000, { steps: 90, bytesPerStep: 3_000, overrides: { protectedTools: ["read"] }, withStatus: true }),
  // Compress refs m00002..m00021 at step 70, then keep growing: the baseline resets.
  run("window-64k-compress-reset", 64_000, { steps: 90, bytesPerStep: 3_000, compressAt: { 70: ["m00002", "m00021"] }, withStatus: true }),
  // Lowered tier triggers and repeated compresses. Tier-2/3 did NOT fire here (see ticket 20 report); pins tier-1 blocks only.
  run("window-64k-blocks", 64_000, { steps: 90, bytesPerStep: 3_000, overrides: lowTiers, compressAt: { 30: ["m00002", "m00011"], 50: ["m00002", "m00021"], 70: ["m00002", "m00031"] }, withStatus: true }),
];
for (const scenario of scenarios) {
  writeFileSync(join(here, "golden", `${scenario.name}.json`), JSON.stringify(scenario, null, 2) + "\n");
}
console.log(`wrote ${scenarios.length} golden files from acp-kernel ${kernelVersion}`);
