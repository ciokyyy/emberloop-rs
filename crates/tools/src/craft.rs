//! The ArtCraft apps that ship both an MCP server and a web bundle.
//!
//! Kept in step with `scripts/verify-craft.tsv`; `filmcraft` stays out (D8) and
//! `anicraft` has no web bundle, so neither is listed here.

use serde_json::{json, Value};

/// One Craft app: its `mcp.json` key, display label, CLI, server summary and web page.
pub struct Craft {
    pub name: &'static str,
    pub label: &'static str,
    pub cli: &'static str,
    pub summary: &'static str,
    pub url: &'static str,
}

/// Every verified Craft app except `figcraft`, which the daemon configures itself.
pub const APPS: &[Craft] = &[
    Craft {
        name: "cadcraft",
        label: "CADCraft",
        cli: "cadcraft-cli",
        summary: "CADCraft computer-aided design and drafting app: models and drawings.",
        url: "https://getartcraft.com/apps/cadcraft",
    },
    Craft {
        name: "craftanything",
        label: "Craft Anything",
        cli: "craft-anything-cli",
        summary: "Craft Anything: understand how any software works, and craft your own.",
        url: "https://getartcraft.com/apps/craft-anything",
    },
    Craft {
        name: "deckcraft",
        label: "DeckCraft",
        cli: "deckcraft-cli",
        summary: "DeckCraft presentation app: slides and slide shows.",
        url: "https://getartcraft.com/apps/deckcraft",
    },
    Craft {
        name: "designcraft",
        label: "DesignCraft",
        cli: "designcraft-cli",
        summary: "DesignCraft page and layout design editor.",
        url: "https://getartcraft.com/apps/designcraft",
    },
    Craft {
        name: "effectcraft",
        label: "EffectCraft",
        cli: "effectcraft-cli",
        summary: "EffectCraft motion graphics and compositing tool.",
        url: "https://getartcraft.com/apps/effectcraft",
    },
    Craft {
        name: "gridcraft",
        label: "GridCraft",
        cli: "gridcraft-cli",
        summary: "GridCraft spreadsheet app: sheets, formulas and cells.",
        url: "https://getartcraft.com/apps/gridcraft",
    },
    Craft {
        name: "lightcraft",
        label: "LightCraft",
        cli: "lightcraft-cli",
        summary: "LightCraft non-destructive photo editor.",
        url: "https://getartcraft.com/apps/lightcraft",
    },
    Craft {
        name: "materialcraft",
        label: "MaterialCraft",
        cli: "materialcraft-cli",
        summary: "MaterialCraft node-based material designer.",
        url: "https://getartcraft.com/apps/materialcraft",
    },
    Craft {
        name: "pdfcraft",
        label: "PdfCraft",
        cli: "pdfcraft-cli",
        summary: "PdfCraft PDF reader and editor: pages, forms and export.",
        url: "https://getartcraft.com/apps/pdfcraft",
    },
    Craft {
        name: "photocraft",
        label: "PhotoCraft",
        cli: "photocraft-cli",
        summary: "PhotoCraft photo editor: open, adjust and export images.",
        url: "https://getartcraft.com/apps/photocraft",
    },
    Craft {
        name: "soundcraft",
        label: "SoundCraft",
        cli: "soundcraft-cli",
        summary: "SoundCraft digital audio workstation: tracks, clips and mixing.",
        url: "https://getartcraft.com/apps/soundcraft",
    },
    Craft {
        name: "texturecraft",
        label: "TextureCraft",
        cli: "texturecraft-cli",
        summary: "TextureCraft 3D texture painting workflow.",
        url: "https://getartcraft.com/apps/texturecraft",
    },
    Craft {
        name: "vectorcraft",
        label: "VectorCraft",
        cli: "vectorcraft-cli",
        summary: "VectorCraft vector art tool: paths, shapes and SVG.",
        url: "https://getartcraft.com/apps/vectorcraft",
    },
    Craft {
        name: "wordcraft",
        label: "WordCraft",
        cli: "wordcraft-cli",
        summary: "WordCraft word processor: documents and .docx export.",
        url: "https://getartcraft.com/apps/wordcraft",
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
            assert!(
                app.url.starts_with("https://getartcraft.com/apps/"),
                "{} has no ArtCraft page",
                app.name
            );
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
