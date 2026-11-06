//! Embedded default prompt texts. Every model-facing text the daemon owns has
//! one id and one default file; an override in `<config root>/prompts/<id>.md`
//! wins without a rebuild.

pub const PONYTAIL: &str = include_str!("../prompts/ponytail.md");
pub const POLICY: &str = include_str!("../prompts/policy.md");
pub const COMPRESS: &str = include_str!("../prompts/compress.md");
pub const DECOMPRESS: &str = include_str!("../prompts/decompress.md");
pub const SEARCH_CONTEXT: &str = include_str!("../prompts/search_context.md");
pub const COMPRESS_PROMPT: &str = include_str!("../prompts/compress.prompt.md");
pub const TOOL_READ: &str = include_str!("../prompts/tool.read.md");
pub const TOOL_EDIT: &str = include_str!("../prompts/tool.edit.md");
pub const TOOL_BASH: &str = include_str!("../prompts/tool.bash.md");
pub const TOOL_BACKGROUND: &str = include_str!("../prompts/tool.background.md");
pub const TOOL_GREP: &str = include_str!("../prompts/tool.grep.md");
