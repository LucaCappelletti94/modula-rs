//! Fixture exercising const/static signature and initializer edges.

/// A local type referenced inside the static's type.
pub struct Registry;

/// A local generic wrapper, so the static's type nests a local type argument.
pub struct Wrapper<T>(pub T);

/// A base constant.
pub const BASE: u32 = 10;

/// Its initializer references `BASE` (a body edge `DERIVED -> BASE`).
pub const DERIVED: u32 = BASE + 1;

/// Its type `Wrapper<Registry>` references the local `Registry` (a signature
/// edge to `Registry`, found by walking into the generic argument).
pub static REGISTRY: Wrapper<Registry> = Wrapper(Registry);
