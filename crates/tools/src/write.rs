use std::fs;

use serde::Deserialize;
use serde_json::Value;

use crate::tag::{content_hash, content_tag, line_count, lines_of};
use crate::{ToolError, ToolResult, Toolbox};

#[derive(Deserialize)]
struct WriteArgs {
    path: String,
    content: String,
}

pub fn run(toolbox: &Toolbox, args: &Value) -> ToolResult<String> {
    let args: WriteArgs = serde_json::from_value(args.clone())?;
    if args.path.trim().is_empty() {
        return Err(ToolError::msg("write requires a non-empty path"));
    }
    let path = toolbox.jail().resolve_writable(&args.path)?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(&path, args.content.as_bytes())?;
    let hash = content_hash(args.content.as_bytes());
    let tag = content_tag(args.content.as_bytes());
    let count = line_count(&args.content);
    let owned_lines: Vec<String> = lines_of(&args.content)
        .into_iter()
        .map(str::to_string)
        .collect();
    toolbox
        .seen()
        .record_with_content(&path, &hash, owned_lines, 1..=count.max(1) as u32, true);
    Ok(format!(
        "[{}#{tag}] written ({count} lines)",
        path.display()
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ToolConfig;
    use serde_json::json;

    #[test]
    fn write_creates_file_and_records_seen_snapshot() {
        let dir = tempfile::tempdir().unwrap();
        let spill = tempfile::tempdir().unwrap();
        let tb = Toolbox::new(ToolConfig::new(dir.path(), spill.path())).unwrap();
        let out = run(
            &tb,
            &json!({
                "path": "sub/hello.txt",
                "content": "one\ntwo\nthree\n"
            }),
        )
        .unwrap();
        assert!(out.contains("written (3 lines)"), "got: {out}");
        let written = fs::read_to_string(dir.path().join("sub/hello.txt")).unwrap();
        assert_eq!(written, "one\ntwo\nthree\n");
    }
}
