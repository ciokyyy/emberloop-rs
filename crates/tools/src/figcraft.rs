//! FigCraft interface design system integration.
//!
//! Provides the baked-in FigCraft MCP definition and a starter project snapshot.

use std::io::Write;
use std::path::{Path, PathBuf};

use serde_json::{json, Value};

/// The baked-in MCP server name.
pub const SERVER: &str = "figcraft";

/// The standard design system file name for each project.
pub const DESIGN_FILE: &str = "design.figcraft";

/// Human-readable summary for `tools.servers()` and MCP registration.
pub const SUMMARY: &str = "FigCraft interface design tool: create, inspect, modify Figma-grade vector frames, pages, components and design tokens in design.figcraft";

/// Minimal valid zip package containing an initial empty FigCraft document.
pub const STARTER_BYTES: &[u8] = &[
    80, 75, 3, 4, 20, 0, 0, 0, 8, 0, 0, 0, 33, 0, 28, 75, 255, 12, 22, 1, 0, 0, 231, 1, 0, 0, 13,
    0, 0, 0, 100, 111, 99, 117, 109, 101, 110, 116, 46, 106, 115, 111, 110, 116, 144, 203, 106,
    195, 48, 16, 69, 127, 165, 220, 181, 8, 86, 27, 146, 214, 127, 208, 77, 232, 166, 171, 16, 130,
    34, 201, 138, 168, 173, 49, 202, 56, 15, 140, 255, 189, 140, 146, 54, 132, 210, 141, 208, 153,
    225, 204, 149, 102, 68, 67, 185, 51, 140, 26, 77, 12, 54, 155, 134, 161, 112, 244, 249, 16, 41,
    161, 214, 10, 177, 51, 193, 191, 39, 231, 207, 168, 215, 27, 133, 100, 58, 143, 26, 159, 137,
    35, 183, 222, 65, 161, 55, 193, 31, 80, 175, 181, 116, 201, 201, 125, 132, 150, 35, 186, 50,
    226, 166, 124, 152, 224, 159, 52, 20, 248, 210, 11, 139, 7, 133, 157, 177, 95, 33, 211, 144,
    156, 40, 25, 117, 53, 123, 91, 84, 203, 215, 249, 139, 66, 120, 160, 221, 3, 25, 212, 122, 86,
    77, 10, 77, 75, 39, 201, 223, 40, 244, 153, 152, 174, 227, 71, 56, 127, 140, 86, 130, 86, 148,
    254, 9, 210, 122, 185, 152, 47, 171, 197, 45, 233, 142, 37, 234, 142, 191, 89, 153, 216, 112,
    89, 13, 122, 202, 156, 77, 100, 76, 10, 97, 136, 229, 223, 242, 134, 83, 116, 188, 23, 189, 82,
    216, 251, 24, 246, 92, 96, 154, 20, 146, 63, 243, 86, 150, 242, 172, 112, 224, 75, 251, 163,
    28, 77, 142, 102, 87, 112, 132, 165, 182, 245, 86, 66, 254, 118, 215, 155, 73, 193, 14, 57,
    251, 196, 219, 178, 63, 217, 175, 35, 59, 116, 82, 177, 212, 82, 190, 89, 150, 58, 169, 93,
    165, 111, 0, 0, 0, 255, 255, 3, 0, 80, 75, 1, 2, 20, 3, 20, 0, 0, 0, 8, 0, 0, 0, 33, 0, 28, 75,
    255, 12, 22, 1, 0, 0, 231, 1, 0, 0, 13, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 164, 129, 0, 0, 0, 0,
    100, 111, 99, 117, 109, 101, 110, 116, 46, 106, 115, 111, 110, 80, 75, 5, 6, 0, 0, 0, 0, 1, 0,
    1, 0, 59, 0, 0, 0, 65, 1, 0, 0, 0, 0,
];

/// Returns the path to `design.figcraft` in the given project root.
pub fn design_path(project_root: &Path) -> PathBuf {
    project_root.join(DESIGN_FILE)
}

/// Ensures `design.figcraft` exists in `project_root`. Creates it if missing.
pub fn ensure_design_file(project_root: &Path) -> std::io::Result<PathBuf> {
    let path = design_path(project_root);
    std::fs::create_dir_all(project_root)?;
    match std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
    {
        Ok(mut file) => {
            file.write_all(STARTER_BYTES)?;
            file.sync_all()?;
        }
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(error),
    }
    Ok(path)
}

/// Returns the default MCP config entry for figcraft.
pub fn default_entry() -> Value {
    json!({
        "command": "figcraft-cli",
        "args": ["mcp"],
        "enabled": true,
        "summary": SUMMARY
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_ensure_and_read_design_file() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        assert!(!design_path(root).exists());

        let path = ensure_design_file(root).unwrap();
        assert!(path.exists());
        assert_eq!(path, design_path(root));

        assert_eq!(std::fs::read(&path).unwrap(), STARTER_BYTES);
        std::fs::write(&path, b"existing design").unwrap();
        ensure_design_file(root).unwrap();
        assert_eq!(std::fs::read(path).unwrap(), b"existing design");
    }

    #[test]
    fn test_default_entry() {
        let entry = default_entry();
        assert_eq!(entry["command"], "figcraft-cli");
        assert_eq!(entry["args"][0], "mcp");
        assert_eq!(entry["enabled"], true);
    }
}
