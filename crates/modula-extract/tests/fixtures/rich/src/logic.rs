//! Logic module: impls and a free function that calls methods.

use crate::types::{Greet, Outer};

impl Outer {
    /// Inherent helper, called from the trait impl below.
    fn helper(&self) -> u32 {
        7
    }
}

impl Greet for Outer {
    fn greet(&self) -> u32 {
        // Method call resolving to the inherent `helper`: a body edge.
        self.helper()
    }
}

/// Takes `Outer` by reference (signature edge to `Outer`) and calls its trait
/// method (body edge to `greet`).
pub fn run(outer: &Outer) -> u32 {
    outer.greet()
}
