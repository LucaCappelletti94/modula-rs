//! Dependency edges between items.

use serde::{Deserialize, Serialize};

use crate::ItemId;

/// The kind of reference an edge represents.
///
/// Kinds are kept distinct (rather than pre-summed) so the metric layer can
/// apply a weighting policy without re-extracting. `Body` is the capability that
/// distinguishes this tool from signature-only analyzers: it records references
/// that occur inside function bodies.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum RefKind {
    /// A `use` import or re-export path resolution.
    Import,
    /// A type in a signature: parameter, return, field, type-alias, or const.
    Signature,
    /// A generic bound, where-clause, or supertrait.
    TraitBound,
    /// A reference inside a function body.
    Body,
    /// A structural edge induced by an `impl` block (type/trait coupling).
    Impl,
}

/// A directed dependency edge.
///
/// The extractor emits at most one edge per `(from, to, kind)` triple, with
/// `weight` holding the number of syntactic occurrences collapsed into it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Edge {
    /// The referring item.
    pub from: ItemId,
    /// The referenced item.
    pub to: ItemId,
    /// The kind of reference.
    pub kind: RefKind,
    /// Multiplicity: how many syntactic occurrences this edge collapses.
    pub weight: u32,
}
