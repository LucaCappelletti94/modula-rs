//! Fixture exercising field-access body edges.

/// A config struct.
pub struct Config {
    /// A field.
    pub value: u32,
}

/// Builds a `Config`.
pub fn make() -> Config {
    Config { value: 7 }
}

/// Reads `Config::value` through a field access only. `read` has no other
/// reference to `Config`, so the edge `read -> Config` is discoverable solely
/// via the `c.value` field access.
pub fn read() -> u32 {
    let c = make();
    c.value
}
