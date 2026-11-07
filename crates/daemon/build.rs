// A release name is baked in when the builder names one: `EMBERLOOP_BUILD_VERSION` lets a local
// build carry the version it will be installed as, and the crate version is the default.
fn main() {
    println!("cargo:rerun-if-env-changed=EMBERLOOP_BUILD_VERSION");
    let version = std::env::var("EMBERLOOP_BUILD_VERSION")
        .or_else(|_| std::env::var("CARGO_PKG_VERSION"))
        .unwrap_or_else(|_| "0.0.0".to_string());
    println!("cargo:rustc-env=EMBERLOOP_BUILD_VERSION={version}");
}
