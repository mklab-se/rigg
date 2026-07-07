//! Version output (used by MCP subprocess mode; `rigg version` prints the banner).

/// Current rigg version string.
pub fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}
