//! The ArtCraft apps that ship both an MCP server and a web bundle.
//!
//! Kept in step with `scripts/verify-craft.tsv`; `filmcraft` stays out (D8) and
//! `anicraft` has no web bundle, so neither is listed here.

use serde_json::{json, Value};

/// One Craft app: its `mcp.json` key, display label, CLI and server summary.
pub struct Craft {
    pub name: &'static str,
    pub label: &'static str,
    pub cli: &'static str,
    pub summary: &'static str,
}

/// Every verified Craft app except `figcraft`, which the daemon configures itself.
pub const APPS: &[Craft] = &[
    Craft {
        name: "cadcraft",
        label: "CADCraft",
        cli: "cadcraft-cli",
        summary: "CADCraft computer-aided design and drafting app: models and drawings.",
    },
    Craft {
        name: "craftanything",
        label: "Craft Anything",
        cli: "craft-anything-cli",
        summary: "Craft Anything: understand how any software works, and craft your own.",
    },
    Craft {
        name: "deckcraft",
        label: "DeckCraft",
        cli: "deckcraft-cli",
        summary: "DeckCraft presentation app: slides and slide shows.",
    },
    Craft {
        name: "designcraft",
        label: "DesignCraft",
        cli: "designcraft-cli",
        summary: "DesignCraft page and layout design editor.",
    },
    Craft {
        name: "effectcraft",
        label: "EffectCraft",
        cli: "effectcraft-cli",
        summary: "EffectCraft motion graphics and compositing tool.",
    },
    Craft {
        name: "gridcraft",
        label: "GridCraft",
        cli: "gridcraft-cli",
        summary: "GridCraft spreadsheet app: sheets, formulas and cells.",
    },
    Craft {
        name: "lightcraft",
        label: "LightCraft",
        cli: "lightcraft-cli",
        summary: "LightCraft non-destructive photo editor.",
    },
    Craft {
        name: "materialcraft",
        label: "MaterialCraft",
        cli: "materialcraft-cli",
        summary: "MaterialCraft node-based material designer.",
    },
    Craft {
        name: "pdfcraft",
        label: "PdfCraft",
        cli: "pdfcraft-cli",
        summary: "PdfCraft PDF reader and editor: pages, forms and export.",
    },
    Craft {
        name: "photocraft",
        label: "PhotoCraft",
        cli: "photocraft-cli",
        summary: "PhotoCraft photo editor: open, adjust and export images.",
    },
    Craft {
        name: "soundcraft",
        label: "SoundCraft",
        cli: "soundcraft-cli",
        summary: "SoundCraft digital audio workstation: tracks, clips and mixing.",
    },
    Craft {
        name: "texturecraft",
        label: "TextureCraft",
        cli: "texturecraft-cli",
        summary: "TextureCraft 3D texture painting workflow.",
    },
    Craft {
        name: "vectorcraft",
        label: "VectorCraft",
        cli: "vectorcraft-cli",
        summary: "VectorCraft vector art tool: paths, shapes and SVG.",
    },
    Craft {
        name: "wordcraft",
        label: "WordCraft",
        cli: "wordcraft-cli",
        summary: "WordCraft word processor: documents and .docx export.",
    },
];

/// The `mcp.json` entry for an app: gated, so a Project must list it to see it.
pub fn default_entry(app: &Craft) -> Value {
    json!({
        "command": app.cli,
        "args": ["mcp"],
        "enabled": true,
        "gated": true,
        "summary": app.summary
    })
}

/// Whether the app's CLI is on `PATH`.
pub fn installed(cli: &str) -> bool {
    std::env::var_os("PATH")
        .is_some_and(|path| std::env::split_paths(&path).any(|dir| dir.join(cli).is_file()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_app_is_unique_and_gated() {
        let mut names: Vec<&str> = APPS.iter().map(|app| app.name).collect();
        names.sort_unstable();
        let before = names.len();
        names.dedup();
        assert_eq!(before, names.len());
        for app in APPS {
            let entry = default_entry(app);
            assert_eq!(entry["command"], app.cli);
            assert_eq!(entry["args"][0], "mcp");
            assert_eq!(entry["gated"], true);
            assert_eq!(entry["enabled"], true);
            assert_eq!(entry["summary"], app.summary);
        }
    }

    #[test]
    fn the_catalogue_matches_the_verify_craft_table() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../scripts/verify-craft.tsv"
        );
        let text = std::fs::read_to_string(path).unwrap();
        let mut rows: Vec<(&str, &str)> = text
            .lines()
            .filter(|line| !line.trim().is_empty() && !line.starts_with('#'))
            .map(|line| {
                let mut cells = line.split('\t');
                (
                    cells.next().unwrap_or_default(),
                    cells.next().unwrap_or_default(),
                )
            })
            .filter(|(name, _)| !matches!(*name, "figcraft" | "filmcraft" | "anicraft"))
            .collect();
        rows.sort_unstable();
        let mut apps: Vec<(&str, &str)> = APPS.iter().map(|app| (app.name, app.cli)).collect();
        apps.sort_unstable();
        assert_eq!(apps, rows);
    }
}
