//! [`CrateGraphBuilder`]: the one place that knows how to assemble a valid
//! [`modula_ir::CrateGraph`].
//!
//! Every language extractor produces the same IR, and that IR carries invariants
//! that the metric layer relies on: ids are dense indices into their vectors,
//! each real `mod` has a module-stub item so imports of a module are well typed,
//! nominal types live in synthetic [`ModuleKind::Type`] containers, module and
//! item `canonical_path`s plus module `depth` are derived from the tree, and
//! edges are deduplicated into summed weights with self-edges dropped. Hand
//! building this per language is error prone, so the builder owns it once.
//!
//! Structural nodes (crates, modules, items) are added top down and return their
//! dense id immediately. Edges are recorded by stable symbol key and resolved at
//! [`finish`](CrateGraphBuilder::finish), so forward and cross-file references
//! work and references to unknown external symbols are simply dropped. The path
//! and depth derivation matches the compact-container invariants, so any graph
//! the builder produces round-trips losslessly through the binary IR.

use std::collections::{BTreeMap, HashMap};

use modula_ir::{
    Crate, CrateGraph, CrateId, Edge, Item, ItemId, ItemKind, Module, ModuleId, ModuleKind,
    RefKind, SCHEMA_VERSION, Visibility,
};

/// Assembles a [`CrateGraph`] while enforcing the IR invariants.
pub struct CrateGraphBuilder {
    provenance: String,
    crates: Vec<Crate>,
    modules: Vec<Module>,
    items: Vec<Item>,
    /// Maps a frontend's stable symbol key to the item it created, for edge
    /// resolution. Module-stub items and type items are registered here too.
    item_of_key: HashMap<String, ItemId>,
    /// Keys that were registered more than once. A frontend must give every node
    /// a distinct key, otherwise edges would resolve to the wrong item, so any
    /// collision fails the build at `finish` rather than corrupting the graph.
    duplicate_keys: Vec<String>,
    /// Edges by key, resolved against `item_of_key` at `finish`.
    pending_edges: Vec<(String, String, RefKind)>,
    root_crate: Option<CrateId>,
}

impl CrateGraphBuilder {
    /// Creates an empty builder. `provenance` is recorded as the IR's tool
    /// version (the `ra_version` field) for cache invalidation and provenance.
    #[must_use]
    pub fn new(provenance: impl Into<String>) -> Self {
        Self {
            provenance: provenance.into(),
            crates: Vec::new(),
            modules: Vec::new(),
            items: Vec::new(),
            item_of_key: HashMap::new(),
            duplicate_keys: Vec::new(),
            pending_edges: Vec::new(),
            root_crate: None,
        }
    }

    /// Adds a crate and its empty-named root module (whose canonical path is the
    /// crate name). The first crate added becomes the root crate unless
    /// overridden with [`set_root_crate`](Self::set_root_crate). `root_key` is
    /// the stable key for the root module's stub item.
    pub fn add_crate(&mut self, name: &str, is_local: bool, root_key: &str) -> (CrateId, ModuleId) {
        let crate_id = CrateId(self.crates.len() as u32);
        let root_id = ModuleId(self.modules.len() as u32);
        self.crates.push(Crate {
            id: crate_id,
            name: name.to_owned(),
            is_local,
            root_module: root_id,
        });
        if self.root_crate.is_none() {
            self.root_crate = Some(crate_id);
        }
        self.modules.push(Module {
            id: root_id,
            crate_id,
            parent: None,
            name: String::new(),
            canonical_path: name.to_owned(),
            depth: 0,
            visibility: Visibility::Public,
            kind: ModuleKind::Mod,
        });
        self.push_module_stub(root_id, root_key);
        (crate_id, root_id)
    }

    /// Adds a real `mod` under `parent`, plus its module-stub item keyed by
    /// `key` so imports that resolve to this module are well typed.
    pub fn add_module(
        &mut self,
        parent: ModuleId,
        key: &str,
        name: &str,
        visibility: Visibility,
    ) -> ModuleId {
        let id = ModuleId(self.modules.len() as u32);
        let (parent_path, parent_depth, crate_id) = self.module_facts(parent);
        self.modules.push(Module {
            id,
            crate_id,
            parent: Some(parent),
            name: name.to_owned(),
            canonical_path: format!("{parent_path}::{name}"),
            depth: parent_depth + 1,
            visibility,
            kind: ModuleKind::Mod,
        });
        self.push_module_stub(id, key);
        id
    }

    /// Adds a nominal type (struct, enum, class, interface, trait) as a
    /// [`ModuleKind::Type`] container under `parent`, together with the type item
    /// itself, whose canonical path equals the container path. Members are added
    /// under the returned module id with [`add_item`](Self::add_item).
    pub fn add_type(
        &mut self,
        parent: ModuleId,
        key: &str,
        name: &str,
        kind: ItemKind,
        visibility: Visibility,
    ) -> (ModuleId, ItemId) {
        let module_id = ModuleId(self.modules.len() as u32);
        let (parent_path, parent_depth, crate_id) = self.module_facts(parent);
        let canonical_path = format!("{parent_path}::{name}");
        self.modules.push(Module {
            id: module_id,
            crate_id,
            parent: Some(parent),
            name: name.to_owned(),
            canonical_path: canonical_path.clone(),
            depth: parent_depth + 1,
            visibility: visibility.clone(),
            kind: ModuleKind::Type,
        });
        let item_id = ItemId(self.items.len() as u32);
        self.items.push(Item {
            id: item_id,
            canonical_path,
            kind,
            visibility,
            owning_module: module_id,
            crate_id,
            has_canonical_path: true,
            reachable_pub_api: false,
        });
        self.register_item_key(key, item_id);
        (module_id, item_id)
    }

    /// Adds an item under `owning`, keyed by `key`. Its canonical path is the
    /// owning module's path joined with `name`.
    pub fn add_item(
        &mut self,
        owning: ModuleId,
        key: &str,
        name: &str,
        kind: ItemKind,
        visibility: Visibility,
    ) -> ItemId {
        let id = ItemId(self.items.len() as u32);
        let (owning_path, _, crate_id) = self.module_facts(owning);
        self.items.push(Item {
            id,
            canonical_path: format!("{owning_path}::{name}"),
            kind,
            visibility,
            owning_module: owning,
            crate_id,
            has_canonical_path: true,
            reachable_pub_api: false,
        });
        self.register_item_key(key, id);
        id
    }

    /// Records a dependency edge between two symbol keys. Resolved at `finish`:
    /// edges whose endpoints are both known and distinct are kept and their
    /// weight accumulated, edges to unknown (external) keys are dropped.
    pub fn add_edge(&mut self, from_key: &str, to_key: &str, kind: RefKind) {
        self.pending_edges
            .push((from_key.to_owned(), to_key.to_owned(), kind));
    }

    /// Overrides which crate is the analysis root (defaults to the first added).
    pub fn set_root_crate(&mut self, crate_id: CrateId) {
        self.root_crate = Some(crate_id);
    }

    /// Finalizes the graph: resolves and deduplicates edges, then computes the
    /// public-API reachability flags. Fails if no crate was added.
    pub fn finish(self) -> anyhow::Result<CrateGraph> {
        let root_crate = self
            .root_crate
            .ok_or_else(|| anyhow::anyhow!("cannot finish a graph with no crate"))?;
        anyhow::ensure!(
            self.duplicate_keys.is_empty(),
            "frontend produced duplicate symbol keys: {:?}",
            self.duplicate_keys
        );

        let mut weights: BTreeMap<(ItemId, ItemId, RefKind), u32> = BTreeMap::new();
        for (from_key, to_key, kind) in &self.pending_edges {
            let (Some(&from), Some(&to)) =
                (self.item_of_key.get(from_key), self.item_of_key.get(to_key))
            else {
                continue;
            };
            if from != to {
                *weights.entry((from, to, *kind)).or_insert(0) += 1;
            }
        }
        let edges = weights
            .into_iter()
            .map(|((from, to, kind), weight)| Edge {
                from,
                to,
                kind,
                weight,
            })
            .collect();

        let mut graph = CrateGraph {
            schema_version: SCHEMA_VERSION,
            ra_version: self.provenance,
            root_crate,
            crates: self.crates,
            modules: self.modules,
            items: self.items,
            edges,
        };
        graph.compute_public_api();
        Ok(graph)
    }

    /// The (canonical path, depth, crate id) of an existing module.
    fn module_facts(&self, module: ModuleId) -> (String, u32, CrateId) {
        let m = &self.modules[module.index()];
        (m.canonical_path.clone(), m.depth, m.crate_id)
    }

    /// Registers an item key for edge resolution, recording a collision if the
    /// key was already used (a frontend bug, surfaced at `finish`).
    fn register_item_key(&mut self, key: &str, id: ItemId) {
        if self.item_of_key.insert(key.to_owned(), id).is_some() {
            self.duplicate_keys.push(key.to_owned());
        }
    }

    /// Pushes the module-stub item for a real `mod`, keyed for edge resolution.
    fn push_module_stub(&mut self, module_id: ModuleId, key: &str) {
        let m = &self.modules[module_id.index()];
        let id = ItemId(self.items.len() as u32);
        let item = Item {
            id,
            canonical_path: m.canonical_path.clone(),
            kind: ItemKind::Module,
            visibility: m.visibility.clone(),
            owning_module: module_id,
            crate_id: m.crate_id,
            has_canonical_path: true,
            reachable_pub_api: false,
        };
        self.items.push(item);
        self.register_item_key(key, id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use modula_ir::{decode_compact, encode_compact};

    /// Builds a small but representative graph: a crate root, a `mod a`, a type
    /// `S` with a method, a free function, and edges including one to an unknown
    /// external symbol (which must be dropped).
    fn sample() -> CrateGraph {
        let mut b = CrateGraphBuilder::new("test 1.0");
        let (_c, root) = b.add_crate("k", true, "k");
        let a = b.add_module(root, "k::a", "a", Visibility::Public);
        let (_s_mod, _s_item) = b.add_type(a, "k::a::S", "S", ItemKind::Struct, Visibility::Public);
        let _run = b.add_item(
            _s_mod,
            "k::a::S::run",
            "run",
            ItemKind::AssocFn,
            Visibility::Public,
        );
        let _f = b.add_item(a, "k::a::f", "f", ItemKind::Function, Visibility::Private);
        // f calls S::run (kept), and references an external symbol (dropped).
        b.add_edge("k::a::f", "k::a::S::run", RefKind::Body);
        b.add_edge("k::a::f", "k::a::S::run", RefKind::Body); // weight accumulates
        b.add_edge("k::a::f", "std::vec::Vec", RefKind::Signature); // unknown, dropped
        b.add_edge("k::a::S::run", "k::a::S::run", RefKind::Body); // self-edge, dropped
        b.finish().expect("finish")
    }

    #[test]
    fn ids_are_dense() {
        let g = sample();
        for (i, m) in g.modules.iter().enumerate() {
            assert_eq!(m.id, ModuleId(i as u32));
        }
        for (i, it) in g.items.iter().enumerate() {
            assert_eq!(it.id, ItemId(i as u32));
        }
        for (i, c) in g.crates.iter().enumerate() {
            assert_eq!(c.id, CrateId(i as u32));
        }
    }

    #[test]
    fn every_real_mod_has_a_module_stub() {
        let g = sample();
        for m in &g.modules {
            if m.kind == ModuleKind::Mod {
                let has_stub = g
                    .items
                    .iter()
                    .any(|it| it.kind == ItemKind::Module && it.owning_module == m.id);
                assert!(has_stub, "module {} lacks a stub item", m.canonical_path);
            }
        }
    }

    #[test]
    fn type_item_path_equals_container_path() {
        let g = sample();
        let container = g
            .modules
            .iter()
            .find(|m| m.kind == ModuleKind::Type)
            .expect("type container");
        let type_item = g
            .items
            .iter()
            .find(|it| it.owning_module == container.id && it.kind == ItemKind::Struct)
            .expect("type item");
        assert_eq!(type_item.canonical_path, container.canonical_path);
    }

    #[test]
    fn edges_resolve_dedup_and_drop_unknown() {
        let g = sample();
        // Only the f -> S::run Body edge survives, with weight 2.
        assert_eq!(g.edges.len(), 1);
        let e = &g.edges[0];
        assert_eq!(e.kind, RefKind::Body);
        assert_eq!(e.weight, 2);
        assert_eq!(g.item(e.from).canonical_path, "k::a::f");
        assert_eq!(g.item(e.to).canonical_path, "k::a::S::run");
    }

    #[test]
    fn public_api_is_computed() {
        let g = sample();
        let run = g
            .items
            .iter()
            .find(|it| it.canonical_path == "k::a::S::run")
            .unwrap();
        assert!(run.reachable_pub_api, "pub item in pub chain is public API");
        let f = g
            .items
            .iter()
            .find(|it| it.canonical_path == "k::a::f")
            .unwrap();
        assert!(!f.reachable_pub_api, "private item is not public API");
    }

    #[test]
    fn round_trips_losslessly_through_the_compact_container() {
        let g = sample();
        let back = decode_compact(&encode_compact(&g).unwrap()).unwrap();
        assert_eq!(g, back);
    }

    #[test]
    fn duplicate_keys_fail_the_build() {
        let mut b = CrateGraphBuilder::new("test");
        let (_c, root) = b.add_crate("k", true, "k");
        b.add_item(root, "dup", "a", ItemKind::Function, Visibility::Public);
        b.add_item(root, "dup", "b", ItemKind::Function, Visibility::Public);
        assert!(b.finish().is_err(), "a reused key must fail the build");
    }

    #[test]
    fn finishing_without_a_crate_errors() {
        assert!(CrateGraphBuilder::new("test").finish().is_err());
    }
}
