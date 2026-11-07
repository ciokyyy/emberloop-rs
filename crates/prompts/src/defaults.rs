//! Embedded default prompt texts. Every model-facing text the daemon owns has
//! one id and one default file; an override in `<config root>/prompts/<id>.md`
//! wins without a rebuild.

pub const PONYTAIL: &str = include_str!("../prompts/ponytail.md");
pub const CAVEMAN: &str = include_str!("../prompts/caveman.md");
pub const POLICY: &str = include_str!("../prompts/policy.md");
pub const SKILL_CODEBASE_MEMORY: &str = include_str!("../prompts/skill.codebase-memory.md");
pub const COMPRESS: &str = include_str!("../prompts/compress.md");
pub const DECOMPRESS: &str = include_str!("../prompts/decompress.md");
pub const SEARCH_CONTEXT: &str = include_str!("../prompts/search_context.md");
pub const ACP_STATUS_DESCRIPTION: &str = include_str!("../prompts/acp_status.md");
pub const COMPRESS_PROMPT: &str = include_str!("../prompts/compress.prompt.md");
pub const TOOL_READ: &str = include_str!("../prompts/tool.read.md");
pub const TOOL_EDIT: &str = include_str!("../prompts/tool.edit.md");
pub const TOOL_BASH: &str = include_str!("../prompts/tool.bash.md");
pub const TOOL_BACKGROUND: &str = include_str!("../prompts/tool.background.md");
pub const TOOL_GREP: &str = include_str!("../prompts/tool.grep.md");
pub const TOOL_WRITE: &str = include_str!("../prompts/tool.write.md");
pub const TOOL_FIND: &str = include_str!("../prompts/tool.find.md");
pub const TOOL_WEBSEARCH: &str = include_str!("../prompts/tool.websearch.md");
pub const NUDGE_FIRST: &str = include_str!("../prompts/nudge.first.md");
pub const NUDGE_PRESSURE: &str = include_str!("../prompts/nudge.pressure.md");
/// The emergency header, then the pressure body it shares.
pub const NUDGE_EMERGENCY: &str = concat!(
    include_str!("../prompts/nudge.emergency.md"),
    "\n\n",
    include_str!("../prompts/nudge.pressure.md"),
);
pub const NUDGE_TIER2: &str = include_str!("../prompts/nudge.tier2.md");
pub const NUDGE_TIER3: &str = include_str!("../prompts/nudge.tier3.md");
pub const TOOL_EXECUTE: &str = include_str!("../prompts/tool.execute.md");
pub const TOOL_TODO: &str = include_str!("../prompts/tool.todo.md");
pub const TOOL_GIT: &str = include_str!("../prompts/tool.git.md");
pub const TOOL_CHECK: &str = include_str!("../prompts/tool.check.md");
pub const TOOL_CTX_SEARCH: &str = include_str!("../prompts/tool.ctx_search.md");
