//! Encapsulation metrics, the Rust-specific dimension.
//!
//! Two signals:
//! - **Over-exposure**: an item whose declared visibility is wider than its
//!   actual use-set requires (for example a `pub` item used only within its own
//!   module).
//! - **Interface legitimacy (leaks)**: a cross-module reference is a *leak* when
//!   its target is not part of the crate's public API (`reachable_pub_api`),
//!   i.e. it reaches into another module's internals instead of its published
//!   surface. `mean_leak_cost` is the fraction of cross-real-module references
//!   that are leaks, i.e. `1 - MII`, the complement of Sarkar/Kak's Module
//!   Interaction Index (IEEE TSE 33(1), 2007). Owning modules are climbed past
//!   synthetic per-type containers first, so two types in one module are
//!   intra-module, not a cross-module reference.

use std::collections::HashSet;

use modula_ir::{CrateGraph, ItemId, ModuleId, Visibility};
use serde::Serialize;

/// An item exposed more widely than its use-set requires.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct OverExposure {
    /// The over-exposed item.
    pub item: ItemId,
    /// The item's canonical path.
    pub path: String,
    /// The declared visibility.
    pub declared: Visibility,
    /// The narrowest visibility that still satisfies every consumer.
    pub required: Visibility,
    /// Whether the item is part of the intended public API (advisory only).
    pub reachable_pub_api: bool,
}

/// A leak: a cross-module reference whose target is not part of the crate's
/// public API, i.e. it reaches into another module's internals.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Leak {
    /// The referring item.
    pub from: ItemId,
    /// The referenced (internal) item.
    pub to: ItemId,
    /// Canonical path of the referring item.
    pub from_path: String,
    /// Canonical path of the referenced item.
    pub to_path: String,
    /// Declared visibility of the referenced item (how internal it is).
    pub target_visibility: Visibility,
}

/// The encapsulation report for a crate graph.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct EncapsulationReport {
    /// Over-exposed items (declared visibility wider than required), excluding
    /// intended public-API items.
    pub over_exposed: Vec<OverExposure>,
    /// Fraction of items that are over-exposed.
    pub over_exposed_fraction: f64,
    /// Cross-module references that reach a non-API (internal) item.
    pub leaks: Vec<Leak>,
    /// Total number of distinct cross-real-module references (the denominator of
    /// the leak rate).
    pub n_cross_module_refs: usize,
    /// Leak rate: `leaks / n_cross_module_refs`, the fraction of cross-module
    /// references that pierce a module's internals (`0` when there are none).
    /// Equals `1 - MII` (Module Interaction Index).
    pub mean_leak_cost: f64,
}

/// Computes the encapsulation report.
#[must_use]
pub fn encapsulation(ir: &CrateGraph) -> EncapsulationReport {
    let over_exposed = over_exposed(ir);
    let over_exposed_fraction = if ir.items.is_empty() {
        0.0
    } else {
        over_exposed.len() as f64 / ir.items.len() as f64
    };

    let (leaks, n_cross_module_refs) = leaks(ir);
    let mean_leak_cost = if n_cross_module_refs == 0 {
        0.0
    } else {
        leaks.len() as f64 / n_cross_module_refs as f64
    };

    EncapsulationReport {
        over_exposed,
        over_exposed_fraction,
        leaks,
        n_cross_module_refs,
        mean_leak_cost,
    }
}

/// Finds items whose declared visibility exceeds what their consumers require.
fn over_exposed(ir: &CrateGraph) -> Vec<OverExposure> {
    // Consumer sets: for each item, the items that reference it.
    let mut consumers: Vec<Vec<ItemId>> = vec![Vec::new(); ir.items.len()];
    for edge in &ir.edges {
        consumers[edge.to.index()].push(edge.from);
    }

    let mut result = Vec::new();
    for item in &ir.items {
        if item.reachable_pub_api {
            // Intended public API: over-exposure is advisory, so do not flag it.
            continue;
        }
        if item.visibility_fixed_by_trait {
            // A trait impl member or trait associated item: its visibility follows
            // the trait and cannot be narrowed, so flagging it is a false positive.
            continue;
        }
        let required = required_visibility(ir, item.id, &consumers[item.id.index()]);
        if item.visibility.restrictiveness() > required.restrictiveness() {
            result.push(OverExposure {
                item: item.id,
                path: item.canonical_path.clone(),
                declared: item.visibility.clone(),
                required,
                reachable_pub_api: item.reachable_pub_api,
            });
        }
    }
    result
}

/// The narrowest visibility that still lets every consumer reach `item`.
///
/// Visibility is reasoned about at the real-module level (type containers are
/// climbed to their `mod`), because Rust has no per-type visibility scope, so
/// over-exposure must be expressible as actual `pub`/`pub(crate)`/`pub(super)`.
fn required_visibility(ir: &CrateGraph, item: ItemId, consumers: &[ItemId]) -> Visibility {
    let owner = ir.real_module(ir.item(item).owning_module);
    let krate = ir.item(item).crate_id;
    let mut required = Visibility::Private;
    for &consumer in consumers {
        let needed = needed_visibility(
            ir,
            owner,
            krate,
            ir.real_module(ir.item(consumer).owning_module),
            ir.item(consumer).crate_id,
        );
        if needed.restrictiveness() > required.restrictiveness() {
            required = needed;
        }
    }
    required
}

/// The minimum visibility `item` (owned by `owner` in `krate`) must declare to
/// be reachable from a consumer owned by `consumer_owner` in `consumer_crate`.
fn needed_visibility(
    ir: &CrateGraph,
    owner: ModuleId,
    krate: modula_ir::CrateId,
    consumer_owner: ModuleId,
    consumer_crate: modula_ir::CrateId,
) -> Visibility {
    if consumer_owner == owner {
        Visibility::Private
    } else if consumer_crate == krate {
        match ir.module(owner).parent {
            Some(parent) if is_within_subtree(ir, consumer_owner, parent) => Visibility::Super,
            _ => Visibility::Crate,
        }
    } else {
        Visibility::Public
    }
}

/// Whether `module` is `ancestor` or a descendant of it.
fn is_within_subtree(ir: &CrateGraph, module: ModuleId, ancestor: ModuleId) -> bool {
    let mut current = Some(module);
    while let Some(m) = current {
        if m == ancestor {
            return true;
        }
        current = ir.module(m).parent;
    }
    false
}

/// Returns the leaks (cross-module references targeting a non-API item) and the
/// total number of distinct cross-real-module references.
///
/// Owning modules are climbed to their real `mod` (past synthetic per-type
/// containers): Rust has no per-type visibility scope, so a reference between
/// two types in the same module is intra-module, not cross-module. A cross-module
/// reference whose target is `reachable_pub_api` goes through the crate's public
/// surface (legitimate); otherwise it reaches an internal item (a leak), the
/// implicit-dependency notion from Sarkar/Kak's API-based modularization metrics.
fn leaks(ir: &CrateGraph) -> (Vec<Leak>, usize) {
    let mut seen = HashSet::new();
    let mut leaks = Vec::new();
    let mut n_cross_module_refs = 0usize;
    for edge in &ir.edges {
        let src_module = ir.real_module(ir.item(edge.from).owning_module);
        let dst_module = ir.real_module(ir.item(edge.to).owning_module);
        if src_module == dst_module || !seen.insert((edge.from, edge.to)) {
            continue;
        }
        n_cross_module_refs += 1;
        let target = ir.item(edge.to);
        if !target.reachable_pub_api {
            leaks.push(Leak {
                from: edge.from,
                to: edge.to,
                from_path: ir.item(edge.from).canonical_path.clone(),
                to_path: target.canonical_path.clone(),
                target_visibility: target.visibility.clone(),
            });
        }
    }
    (leaks, n_cross_module_refs)
}

#[cfg(test)]
mod tests {
    use modula_ir::{
        Crate, CrateGraph, CrateId, Edge, Item, ItemId, ItemKind, Module, ModuleId, ModuleKind,
        RefKind, SCHEMA_VERSION, Visibility,
    };

    use super::{encapsulation, is_within_subtree};

    /// Tree: 0 root; 1 child of 0; 2 child of 1; 3 child of 0.
    fn tree() -> CrateGraph {
        let parents = [None, Some(0u32), Some(1), Some(0)];
        let modules = (0..4u32)
            .map(|i| Module {
                id: ModuleId(i),
                crate_id: CrateId(0),
                parent: parents[i as usize].map(ModuleId),
                name: format!("m{i}"),
                canonical_path: format!("c::m{i}"),
                depth: u32::from(i != 0),
                visibility: Visibility::Public,
                kind: ModuleKind::Mod,
            })
            .collect();
        CrateGraph {
            schema_version: SCHEMA_VERSION,
            ra_version: String::new(),
            root_crate: CrateId(0),
            crates: vec![Crate {
                id: CrateId(0),
                name: "c".to_owned(),
                is_local: true,
                root_module: ModuleId(0),
            }],
            modules,
            items: Vec::new(),
            edges: Vec::new(),
        }
    }

    #[test]
    fn within_subtree_walks_ancestors() {
        let g = tree();
        assert!(is_within_subtree(&g, ModuleId(2), ModuleId(1)));
        assert!(is_within_subtree(&g, ModuleId(2), ModuleId(0)));
        assert!(is_within_subtree(&g, ModuleId(1), ModuleId(1)));
        // Sibling branch: 3 is not under 1.
        assert!(!is_within_subtree(&g, ModuleId(3), ModuleId(1)));
    }

    /// Builds a crate with `module_kinds[i]`/`parents[i]` modules and items
    /// `(owning_module, kind, reachable_pub_api)`, plus a single Body edge
    /// `item0 -> item1`.
    fn crate_with(
        module_kinds: &[(Option<u32>, u32, ModuleKind)],
        items: &[(u32, ItemKind, bool)],
    ) -> CrateGraph {
        let modules = module_kinds
            .iter()
            .enumerate()
            .map(|(i, &(parent, depth, kind))| Module {
                id: ModuleId(i as u32),
                crate_id: CrateId(0),
                parent: parent.map(ModuleId),
                name: format!("m{i}"),
                canonical_path: format!("c::m{i}"),
                depth,
                visibility: Visibility::Public,
                kind,
            })
            .collect();
        let items = items
            .iter()
            .enumerate()
            .map(|(i, &(owner, kind, reachable_pub_api))| Item {
                id: ItemId(i as u32),
                canonical_path: format!("c::i{i}"),
                kind,
                visibility: Visibility::Public,
                owning_module: ModuleId(owner),
                crate_id: CrateId(0),
                has_canonical_path: true,
                reachable_pub_api,
                visibility_fixed_by_trait: false,
            })
            .collect();
        CrateGraph {
            schema_version: SCHEMA_VERSION,
            ra_version: String::new(),
            root_crate: CrateId(0),
            crates: vec![Crate {
                id: CrateId(0),
                name: "c".to_owned(),
                is_local: true,
                root_module: ModuleId(0),
            }],
            modules,
            items,
            edges: vec![Edge {
                from: ItemId(0),
                to: ItemId(1),
                kind: RefKind::Body,
                weight: 1,
            }],
        }
    }

    #[test]
    fn cross_type_edge_in_one_module_is_not_cross_module() {
        // root mod (0) with two per-type containers Foo (1) and Bar (2); one
        // item in each, with a Foo -> Bar edge. Both types roll up to the same
        // real module, so it is not a cross-module reference at all.
        let g = crate_with(
            &[
                (None, 0, ModuleKind::Mod),
                (Some(0), 1, ModuleKind::Type),
                (Some(0), 1, ModuleKind::Type),
            ],
            &[(1, ItemKind::Struct, true), (2, ItemKind::Struct, true)],
        );
        let report = encapsulation(&g);
        assert_eq!(report.n_cross_module_refs, 0);
        assert!(report.leaks.is_empty());
        assert_eq!(report.mean_leak_cost, 0.0);
    }

    #[test]
    fn cross_module_call_to_public_api_is_not_a_leak() {
        // Two real sibling modules; module a references a PUBLIC-API item in b.
        // That is legitimate interface use, not a leak (MII = 1).
        let g = crate_with(
            &[
                (None, 0, ModuleKind::Mod),
                (Some(0), 1, ModuleKind::Mod),
                (Some(0), 1, ModuleKind::Mod),
            ],
            &[(1, ItemKind::Struct, true), (2, ItemKind::Struct, true)],
        );
        let report = encapsulation(&g);
        assert_eq!(report.n_cross_module_refs, 1);
        assert!(report.leaks.is_empty(), "API target is not a leak");
        assert_eq!(report.mean_leak_cost, 0.0);
    }

    #[test]
    fn trait_dictated_item_is_not_over_exposed() {
        // One module with two public items and a Body edge item0 -> item1. Both
        // are used only within their module (or not at all) and are not public
        // API, so over-exposure flags both.
        let mut g = crate_with(
            &[(None, 0, ModuleKind::Mod)],
            &[
                (0, ItemKind::Function, false),
                (0, ItemKind::AssocFn, false),
            ],
        );
        let flagged: Vec<ItemId> = super::over_exposed(&g).iter().map(|o| o.item).collect();
        assert!(
            flagged.contains(&ItemId(1)),
            "a narrowable item used only locally is over-exposed"
        );

        // Marking item1 as trait-dictated (its visibility follows a trait, so it
        // cannot be narrowed) removes it from the over-exposed set. item0 stays.
        g.items[1].visibility_fixed_by_trait = true;
        let flagged: Vec<ItemId> = super::over_exposed(&g).iter().map(|o| o.item).collect();
        assert!(
            !flagged.contains(&ItemId(1)),
            "a trait-dictated item must not be flagged over-exposed"
        );
        assert!(
            flagged.contains(&ItemId(0)),
            "a normal item is still flagged"
        );
    }

    #[test]
    fn cross_module_reach_into_internals_is_a_leak() {
        // Same shape, but b's item is NOT public API (reachable_pub_api = false):
        // module a reaches into b's internals, the implicit-dependency leak.
        let g = crate_with(
            &[
                (None, 0, ModuleKind::Mod),
                (Some(0), 1, ModuleKind::Mod),
                (Some(0), 1, ModuleKind::Mod),
            ],
            &[(1, ItemKind::Struct, true), (2, ItemKind::Struct, false)],
        );
        let report = encapsulation(&g);
        assert_eq!(report.n_cross_module_refs, 1);
        assert_eq!(report.leaks.len(), 1, "reaching an internal item is a leak");
        assert_eq!(report.leaks[0].to, ItemId(1));
        assert_eq!(report.mean_leak_cost, 1.0);
    }
}
