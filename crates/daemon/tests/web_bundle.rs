//! The SPA is baked into the binary by `rust-embed` at compile time, from a `web/build/` that git
//! ignores. A release built without a fresh bundle ships the old UI, so a stale bundle is a test
//! failure rather than something a User finds after an update.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// newest walks `path` and returns the latest modification time it holds.
fn newest(path: &Path) -> Option<SystemTime> {
    let meta = std::fs::metadata(path).ok()?;
    if meta.is_file() {
        return meta.modified().ok();
    }
    std::fs::read_dir(path)
        .ok()?
        .filter_map(|entry| entry.ok())
        .filter_map(|entry| newest(&entry.path()))
        .max()
}

fn web() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../web")
}

#[test]
fn the_embedded_web_bundle_is_newer_than_its_sources() {
    let web = web();
    let newest_source = [
        "src",
        "static",
        "package.json",
        "svelte.config.js",
        "vite.config.ts",
    ]
    .iter()
    .map(|name| web.join(name))
    .filter_map(|path| newest(&path))
    .max();
    let Some(bundle) = newest(&web.join("build")) else {
        panic!("web/build is missing: run `npm --prefix web run build` before building");
    };
    let Some(newest_source) = newest_source else {
        panic!("web sources are missing");
    };
    assert!(
        bundle >= newest_source,
        "web/build ({bundle:?}) is older than the newest web source ({newest_source:?}): run `npm --prefix web run build` before building"
    );
}

/// The install and push files live in `web/static/`, which the adapter copies into the build. A
/// manifest missing from the bundle is an app that cannot be installed, and a missing worker is one
/// that never receives a push.
#[test]
fn the_bundle_ships_the_manifest_and_the_push_worker() {
    for name in [
        "manifest.webmanifest",
        "sw.js",
        "icon-192.png",
        "icon-512.png",
        "icon-maskable-512.png",
        "apple-touch-icon.png",
        "favicon.ico",
    ] {
        let path = web().join("build").join(name);
        assert!(path.is_file(), "web/build/{name} is missing");
    }
}
