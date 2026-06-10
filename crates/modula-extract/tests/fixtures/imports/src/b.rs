//! Module `b` imports from `a` but never otherwise references it, so the only
//! `b -> a` edges are import edges.
#![allow(unused_imports)]

use crate::a;
use crate::a::Thing;
use crate::a::*;

/// Unrelated item that does not reference `a`.
pub fn unrelated() -> u32 {
    0
}
