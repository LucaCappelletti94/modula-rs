//! A flat single-`mod` crate (no `mod` declarations): two types with methods
//! that reference each other. With only the `mod` tree there would be one
//! community; the type-level containers give it a real partition.

/// An engine with behavior.
pub struct Engine {
    power: u32,
}

impl Engine {
    /// A constructor.
    pub fn new() -> Self {
        Engine { power: 0 }
    }
    /// Bumps the power (intra-type call target).
    pub fn boost(&mut self) {
        self.power += 1;
    }
}

/// A car that drives its engine (inter-type coupling).
pub struct Car {
    engine: Engine,
}

impl Car {
    /// Constructs a car around a fresh engine.
    pub fn assemble() -> Self {
        Car {
            engine: Engine::new(),
        }
    }
    /// Calls into `Engine::boost` (a cross-type body edge).
    pub fn drive(&mut self) {
        self.engine.boost();
    }
}
