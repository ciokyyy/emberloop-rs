#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]
//! Model proxy: routing by Session token, usage metering, Ponytail and policy
//! injection, stored splices, and the native billion-context backend.

pub mod compressor;
pub mod ctx;
pub mod meter;
pub mod nudge;
pub mod server;
pub mod splice;
pub mod token;
pub mod wire;

pub use compressor::{
    CompressCall, CompressPolicy, Compressor, CompressorStatus, DecompressCall, NativeCompressor,
    SearchHit,
};
pub use server::{router, ProxyConfig, ProxyState};
pub use splice::{Splice, SpliceReport, SpliceStore};
pub use token::{TokenError, TokenSigner};
