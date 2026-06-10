//! Items: the nodes of the dependency graph.

use serde::{Deserialize, Serialize};

use crate::{CrateId, ItemId, ModuleId, Visibility};

/// The kind of a Rust item.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ItemKind {
    /// A module (`mod`). Modules are also items so that `use` edges that resolve
    /// to a module are well typed.
    Module,
    /// A free function.
    Function,
    /// A `struct`.
    Struct,
    /// An `enum`.
    Enum,
    /// A `union`.
    Union,
    /// An enum variant.
    EnumVariant,
    /// A `const`.
    Const,
    /// A `static`.
    Static,
    /// A `trait`.
    Trait,
    /// A type alias.
    TypeAlias,
    /// A macro (declarative or procedural).
    Macro,
    /// A builtin type (`u32`, `str`, ...). Kept only when referenced.
    BuiltinType,
    /// An `impl` block, modeled as a first-class node so the coupling between a
    /// type and a trait it implements is recorded as edges.
    Impl,
    /// An associated function or method.
    AssocFn,
    /// An associated const.
    AssocConst,
    /// An associated type.
    AssocType,
}

/// An item: a node in the dependency graph.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Item {
    /// Dense id of this item.
    pub id: ItemId,
    /// The canonical path (for example `my_crate::parser::Lexer::next`). This is
    /// the stable, cross-session key. Synthesized when rust-analyzer reports no
    /// path (see `has_canonical_path`).
    pub canonical_path: String,
    /// The kind of item.
    pub kind: ItemKind,
    /// The declared visibility.
    pub visibility: Visibility,
    /// The module this item is attributed to. For associated items and impl
    /// blocks this is the module containing the impl/trait block.
    pub owning_module: ModuleId,
    /// The crate this item belongs to.
    pub crate_id: CrateId,
    /// `false` when `canonical_path` was synthesized because rust-analyzer
    /// reported no path (closures, anonymous consts, some macro internals).
    pub has_canonical_path: bool,
    /// `true` when the item is reachable through an unbroken `pub` chain from
    /// the crate root, i.e. part of the intended public API. The over-exposure
    /// metric treats such items as intentionally exposed.
    pub reachable_pub_api: bool,
}
