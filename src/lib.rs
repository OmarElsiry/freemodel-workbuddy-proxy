pub mod acp;
pub mod config;
pub mod error;
pub mod models;
pub mod openai;
pub mod routing;
pub mod server;
pub mod session_store;
pub mod sidecar;
pub mod sse;
#[path = "tui/mod.rs"]
pub mod tui;

pub const BUILD_ID: &str = concat!(
    env!("CARGO_PKG_NAME"),
    "-",
    env!("CARGO_PKG_VERSION"),
    "-",
    env!("FREEMODEL_BUILD_ID")
);
