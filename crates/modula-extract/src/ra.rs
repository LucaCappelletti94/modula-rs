//! The rust-analyzer backed [`Extractor`] implementation.
//!
//! Walks every local crate's modules, items, and impl blocks, then derives
//! dependency edges from:
//! - item signatures (function params/returns, struct/enum/union fields, type
//!   aliases) as `Signature` edges,
//! - impl blocks (the implemented trait) as `Impl` edges, and
//! - function bodies (path references and method calls) as `Body` edges.
//!
//! References that resolve outside the local crates are dropped (their targets
//! are not in the item set), which keeps the graph internal to the workspace.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::panic::{AssertUnwindSafe, catch_unwind};

use ra_ap_hir::{
    self as hir, AssocItem, Crate, GenericDef, HasVisibility as _, Impl, Module, ModuleDef,
    ModuleSource, PathResolution, ScopeDef, Semantics,
};
use ra_ap_ide::{Edition, RootDatabase};
use ra_ap_load_cargo::{LoadCargoConfig, ProcMacroServerChoice, load_workspace};
use ra_ap_paths::{AbsPathBuf, Utf8PathBuf};
use ra_ap_project_model::{
    CargoConfig, ProjectManifest, ProjectWorkspace, ProjectWorkspaceKind, TargetKind,
};
use ra_ap_syntax::ast::{HasModuleItem as _, HasName as _, HasVisibility as _};
use ra_ap_syntax::{AstNode as _, ast};
use ra_ap_vfs::Vfs;

use modula_ir::{
    Crate as IrCrate, CrateGraph, CrateId, Edge, Item, ItemId, ItemKind, Module as IrModule,
    ModuleId, RefKind, SCHEMA_VERSION, Visibility,
};

use crate::{ExtractOptions, Extractor};

/// rust-analyzer version this extractor is pinned against, recorded in the IR
/// for provenance and cache invalidation.
const RA_VERSION: &str = "0.0.336";

/// The rust-analyzer backed extractor.
#[derive(Clone, Copy, Debug, Default)]
pub struct RaExtractor;

impl Extractor for RaExtractor {
    fn extract(&self, opts: &ExtractOptions) -> anyhow::Result<CrateGraph> {
        let cargo_config = CargoConfig::default();
        let load_config = LoadCargoConfig {
            load_out_dirs_from_check: false,
            with_proc_macro_server: ProcMacroServerChoice::None,
            prefill_caches: false,
            num_worker_threads: 1,
            proc_macro_processes: 1,
        };
        let progress = |_: String| {};

        // Resolve the manifest, load the project workspace (kept so we can read
        // its cargo target metadata), and pick the lib/bin target roots to keep.
        let manifest_abs = absolute_manifest(&opts.manifest_path)?;
        let manifest = ProjectManifest::discover_single(manifest_abs.as_path())?;
        let workspace = ProjectWorkspace::load(manifest, &cargo_config, &progress)?;
        let selection = select_targets(&workspace, &manifest_abs, opts)?;

        let (db, vfs, _proc_macro) =
            load_workspace(workspace, &cargo_config.extra_env, &load_config)?;

        // rust-analyzer's trait solver requires the database to be attached to
        // the current thread for the duration of any HIR query.
        hir::attach_db(&db, || {
            // Keep only the crates whose root file is a selected lib/bin target,
            // ordered deterministically with the root lib crate first.
            let mut kept: Vec<Crate> = Crate::all(&db)
                .into_iter()
                .filter(|krate| {
                    crate_root_path(&db, &vfs, *krate)
                        .is_some_and(|path| selection.roots.contains(&path))
                })
                .collect();
            kept.sort_by_key(|krate| crate_root_path(&db, &vfs, *krate).unwrap_or_default());
            if let Some(lib) = &selection.lib_root {
                if let Some(pos) = kept.iter().position(|krate| {
                    crate_root_path(&db, &vfs, *krate).as_deref() == Some(lib.as_str())
                }) {
                    let lib_crate = kept.remove(pos);
                    kept.insert(0, lib_crate);
                }
            }

            if kept.is_empty() {
                anyhow::bail!("no analyzable lib/bin crates were selected");
            }

            let mut builder = Builder::new(&db);
            for krate in &kept {
                builder.add_crate(*krate);
            }
            for krate in &kept {
                builder.add_impls(*krate);
            }
            builder.add_signature_edges();
            builder.add_trait_bound_edges();
            builder.add_import_edges();
            builder.collect_reexports();
            builder.walk_bodies();

            Ok(builder.finish())
        })
    }
}

/// Accumulates the IR while walking the rust-analyzer HIR.
struct Builder<'db> {
    db: &'db RootDatabase,
    crates: Vec<IrCrate>,
    modules: Vec<IrModule>,
    items: Vec<Item>,
    /// Maps a hir crate to its dense [`CrateId`].
    crate_ids: HashMap<Crate, CrateId>,
    /// Maps a hir module to its dense [`ModuleId`].
    module_ids: HashMap<Module, ModuleId>,
    /// Maps a hir item to its dense [`ItemId`] (the node-identity map).
    item_ids: HashMap<ModuleDef, ItemId>,
    /// Every item's def paired with its id, for signature walking.
    defs: Vec<(ModuleDef, ItemId)>,
    /// Functions paired with their item id, for the body-walk pass.
    functions: Vec<(hir::Function, ItemId)>,
    /// `pub use` re-exports: `(re-exporting module, target item)`, including
    /// nested and glob re-exports.
    reexports: Vec<(ModuleId, ItemId)>,
    /// Edge multiplicities, keyed by `(from, to, kind)`.
    edges: BTreeMap<(ItemId, ItemId, RefKind), u32>,
}

impl<'db> Builder<'db> {
    fn new(db: &'db RootDatabase) -> Self {
        Self {
            db,
            crates: Vec::new(),
            modules: Vec::new(),
            items: Vec::new(),
            crate_ids: HashMap::new(),
            module_ids: HashMap::new(),
            item_ids: HashMap::new(),
            defs: Vec::new(),
            functions: Vec::new(),
            reexports: Vec::new(),
            edges: BTreeMap::new(),
        }
    }

    fn add_crate(&mut self, krate: Crate) {
        let crate_id = CrateId(self.crates.len() as u32);
        self.crate_ids.insert(krate, crate_id);
        let edition = krate.edition(self.db);
        let name = crate_name(self.db, krate);
        let root = krate.root_module(self.db);

        let root_module_id = ModuleId(self.modules.len() as u32);
        self.crates.push(IrCrate {
            id: crate_id,
            name,
            is_local: true,
            root_module: root_module_id,
        });

        self.add_module(root, crate_id, None, edition);
    }

    fn add_module(
        &mut self,
        module: Module,
        crate_id: CrateId,
        parent: Option<ModuleId>,
        edition: Edition,
    ) {
        let id = ModuleId(self.modules.len() as u32);
        self.module_ids.insert(module, id);

        let name = if module.is_crate_root(self.db) {
            String::new()
        } else {
            module
                .name(self.db)
                .map(|n| n.display(self.db, edition).to_string())
                .unwrap_or_default()
        };
        let canonical_path = module_path(self.db, module, edition);
        let depth = parent.map_or(0, |_| {
            (module.path_to_root(self.db).len() as u32).saturating_sub(1)
        });
        let visibility = map_visibility(self.db, module.visibility(self.db), module, edition);

        self.modules.push(IrModule {
            id,
            crate_id,
            parent,
            name,
            canonical_path: canonical_path.clone(),
            depth,
            visibility: visibility.clone(),
        });

        // The module is also a first-class item so that `use` imports have a
        // source node and `use module` targets resolve. It owns itself, so its
        // import edges aggregate as this module depending on the target.
        let module_item_id = ItemId(self.items.len() as u32);
        self.item_ids
            .insert(ModuleDef::Module(module), module_item_id);
        self.items.push(Item {
            id: module_item_id,
            canonical_path,
            kind: ItemKind::Module,
            visibility,
            owning_module: id,
            crate_id,
            has_canonical_path: true,
            reachable_pub_api: false,
        });

        for def in module.declarations(self.db) {
            if matches!(def, ModuleDef::Module(_)) {
                continue;
            }
            self.add_item(def, crate_id, id, module, edition);
        }

        for child in module.children(self.db) {
            self.add_module(child, crate_id, Some(id), edition);
        }
    }

    /// Walks every impl block in `krate`, adding its associated items and an
    /// `Impl` edge from the implementing type to the implemented trait.
    fn add_impls(&mut self, krate: Crate) {
        let crate_id = self.crate_ids[&krate];
        let edition = krate.edition(self.db);
        for imp in Impl::all_in_crate(self.db, krate) {
            let module = imp.module(self.db);
            let Some(&owning) = self.module_ids.get(&module) else {
                continue;
            };

            for assoc in imp.items(self.db) {
                let def = match assoc {
                    AssocItem::Function(f) => ModuleDef::Function(f),
                    AssocItem::Const(c) => ModuleDef::Const(c),
                    AssocItem::TypeAlias(t) => ModuleDef::TypeAlias(t),
                };
                self.add_item(def, crate_id, owning, module, edition);
            }

            // Couple the implementing type to the implemented trait.
            if let (Some(self_adt), Some(trait_)) =
                (imp.self_ty(self.db).as_adt(), imp.trait_(self.db))
            {
                let self_def = ModuleDef::from(self_adt);
                let trait_def = ModuleDef::Trait(trait_);
                if let (Some(&from), Some(&to)) =
                    (self.item_ids.get(&self_def), self.item_ids.get(&trait_def))
                {
                    self.add_edge(from, to, RefKind::Impl);
                }
            }
        }
    }

    fn add_item(
        &mut self,
        def: ModuleDef,
        crate_id: CrateId,
        owning_module: ModuleId,
        owning_hir_module: Module,
        edition: Edition,
    ) {
        if self.item_ids.contains_key(&def) {
            return;
        }
        let Some(kind) = item_kind(def) else {
            return;
        };
        let id = ItemId(self.items.len() as u32);
        self.item_ids.insert(def, id);
        self.defs.push((def, id));

        let crate_name = self.crates[crate_id.index()].name.clone();
        let (canonical_path, has_canonical_path) = match def.canonical_path(self.db, edition) {
            Some(relative) => (format!("{crate_name}::{relative}"), true),
            None => (
                synthetic_path(self.db, def, owning_module, id, edition),
                false,
            ),
        };
        let visibility =
            map_visibility(self.db, def.visibility(self.db), owning_hir_module, edition);

        self.items.push(Item {
            id,
            canonical_path,
            kind,
            visibility,
            owning_module,
            crate_id,
            has_canonical_path,
            reachable_pub_api: false,
        });

        if let ModuleDef::Function(function) = def {
            self.functions.push((function, id));
        }
    }

    /// Adds `Signature` edges from each item to the local types and traits named
    /// in its signature.
    fn add_signature_edges(&mut self) {
        for (def, from) in self.defs.clone() {
            match def {
                ModuleDef::Function(function) => {
                    for param in function.params_without_self(self.db) {
                        self.push_type_edges(from, param.ty(), RefKind::Signature);
                    }
                    let return_type = function.ret_type(self.db);
                    self.push_type_edges(from, &return_type, RefKind::Signature);
                }
                ModuleDef::Adt(hir::Adt::Struct(s)) => {
                    for field in s.fields(self.db) {
                        let ty = field.ty(self.db);
                        self.push_type_edges(from, &ty, RefKind::Signature);
                    }
                }
                ModuleDef::Adt(hir::Adt::Union(u)) => {
                    for field in u.fields(self.db) {
                        let ty = field.ty(self.db);
                        self.push_type_edges(from, &ty, RefKind::Signature);
                    }
                }
                ModuleDef::Adt(hir::Adt::Enum(e)) => {
                    for variant in e.variants(self.db) {
                        for field in variant.fields(self.db) {
                            let ty = field.ty(self.db);
                            self.push_type_edges(from, &ty, RefKind::Signature);
                        }
                    }
                }
                ModuleDef::TypeAlias(t) => {
                    let ty = t.ty(self.db);
                    self.push_type_edges(from, &ty, RefKind::Signature);
                }
                _ => {}
            }
        }
    }

    /// Adds `Import` edges from each module item to the local items it brings
    /// into scope via `use` (including globs).
    fn add_import_edges(&mut self) {
        let modules: Vec<Module> = self.module_ids.keys().copied().collect();
        for module in modules {
            let from = self.item_ids[&ModuleDef::Module(module)];
            for (_name, scope_def) in module.scope(self.db, None) {
                let ScopeDef::ModuleDef(def) = scope_def else {
                    continue;
                };
                // Items declared in (or as a submodule of) this module are not
                // imports.
                if def.module(self.db) == Some(module) {
                    continue;
                }
                if let Some(&to) = self.item_ids.get(&def) {
                    self.add_edge(from, to, RefKind::Import);
                }
            }
        }
    }

    /// Records `pub use` re-exports as `(re-exporting module, target item)`,
    /// handling simple (`pub use a::B`), nested (`pub use a::{B, C}`), and glob
    /// (`pub use a::*`) re-exports.
    fn collect_reexports(&mut self) {
        let modules: Vec<(Module, ModuleId)> =
            self.module_ids.iter().map(|(m, id)| (*m, *id)).collect();
        for (module, module_id) in modules {
            let edition = module.krate(self.db).edition(self.db);
            let scope = self.scope_map(module, edition);
            for use_item in module_use_items(self.db, module) {
                // Only plain `pub use` (not pub(crate)/pub(super)) re-exports
                // contribute to the public API.
                let is_pub = use_item
                    .visibility()
                    .is_some_and(|v| v.visibility_inner().is_none());
                if !is_pub {
                    continue;
                }
                if let Some(tree) = use_item.use_tree() {
                    self.collect_use_tree(module, module_id, &scope, edition, &tree, &[]);
                }
            }
        }
    }

    /// Recursively walks a use tree, recording each re-exported local item.
    /// `prefix` is the path accumulated from enclosing nested trees.
    fn collect_use_tree(
        &mut self,
        module: Module,
        module_id: ModuleId,
        scope: &HashMap<String, ModuleDef>,
        edition: Edition,
        tree: &ast::UseTree,
        prefix: &[String],
    ) {
        let mut full = prefix.to_vec();
        full.extend(tree.path().map(path_segments).unwrap_or_default());

        if let Some(list) = tree.use_tree_list() {
            for subtree in list.use_trees() {
                self.collect_use_tree(module, module_id, scope, edition, &subtree, &full);
            }
        } else if tree.star_token().is_some() {
            // `pub use prefix::*`: re-export the public items of `prefix`.
            if let Some(target) = self.resolve_module_path(module, scope, edition, &full) {
                for def in target.declarations(self.db) {
                    if matches!(def.visibility(self.db), hir::Visibility::Public) {
                        if let Some(&to) = self.item_ids.get(&def) {
                            self.reexports.push((module_id, to));
                        }
                    }
                }
            }
        } else {
            // `pub use prefix::Item [as Alias]`: the name brought into scope is
            // the alias or the path's last segment.
            let name = tree
                .rename()
                .and_then(|r| r.name())
                .map(|n| n.text().to_string())
                .or_else(|| full.last().cloned());
            if let Some(name) = name {
                if let Some(def) = scope.get(&name) {
                    if let Some(&to) = self.item_ids.get(def) {
                        self.reexports.push((module_id, to));
                    }
                }
            }
        }
    }

    /// Builds a `name -> ModuleDef` map of a module's scope.
    fn scope_map(&self, module: Module, edition: Edition) -> HashMap<String, ModuleDef> {
        module
            .scope(self.db, None)
            .into_iter()
            .filter_map(|(name, scope_def)| match scope_def {
                ScopeDef::ModuleDef(def) => Some((name.display(self.db, edition).to_string(), def)),
                _ => None,
            })
            .collect()
    }

    /// Resolves a path (the segments of a glob's prefix) to a module, following
    /// `crate`/`self`/`super` and descending through child modules.
    fn resolve_module_path(
        &self,
        module: Module,
        scope: &HashMap<String, ModuleDef>,
        edition: Edition,
        segments: &[String],
    ) -> Option<Module> {
        let (first, rest) = segments.split_first()?;
        let mut current = match first.as_str() {
            "crate" => module.krate(self.db).root_module(self.db),
            "self" => module,
            "super" => module.parent(self.db)?,
            name => match scope.get(name)? {
                ModuleDef::Module(m) => *m,
                _ => return None,
            },
        };
        for segment in rest {
            if segment == "super" {
                current = current.parent(self.db)?;
                continue;
            }
            current = current.children(self.db).find(|child| {
                child
                    .name(self.db)
                    .is_some_and(|n| n.display(self.db, edition).to_string() == *segment)
            })?;
        }
        Some(current)
    }

    /// Adds `TraitBound` edges from each item to the local traits named in its
    /// generic bounds, where-clauses, and (for traits) supertraits.
    fn add_trait_bound_edges(&mut self) {
        for (def, from) in self.defs.clone() {
            let generic = match def {
                ModuleDef::Function(f) => Some(GenericDef::Function(f)),
                ModuleDef::Adt(adt) => Some(GenericDef::Adt(adt)),
                ModuleDef::Trait(t) => Some(GenericDef::Trait(t)),
                ModuleDef::TypeAlias(t) => Some(GenericDef::TypeAlias(t)),
                _ => None,
            };

            let mut traits: Vec<hir::Trait> = Vec::new();
            if let Some(generic) = generic {
                for param in generic.type_or_const_params(self.db) {
                    if let Some(type_param) = param.as_type_param(self.db) {
                        traits.extend(type_param.trait_bounds(self.db));
                    }
                }
            }
            if let ModuleDef::Trait(trait_) = def {
                for supertrait in trait_.all_supertraits(self.db) {
                    if supertrait != trait_ {
                        traits.push(supertrait);
                    }
                }
            }

            for trait_ in traits {
                if let Some(&to) = self.item_ids.get(&ModuleDef::Trait(trait_)) {
                    self.add_edge(from, to, RefKind::TraitBound);
                }
            }
        }
    }

    /// Walks a type and adds an edge to each local ADT or trait it names.
    fn push_type_edges(&mut self, from: ItemId, ty: &hir::Type<'db>, kind: RefKind) {
        let mut targets: Vec<ModuleDef> = Vec::new();
        ty.strip_references().walk(self.db, |t| {
            if let Some(adt) = t.as_adt() {
                targets.push(ModuleDef::from(adt));
            } else if let Some(trait_) = t.as_dyn_trait() {
                targets.push(ModuleDef::Trait(trait_));
            }
        });
        for target in targets {
            if let Some(&to) = self.item_ids.get(&target) {
                self.add_edge(from, to, kind);
            }
        }
    }

    /// Resolves references inside each function body to item-level `Body` edges.
    /// Each body is processed in isolation so one malformed body cannot sink the
    /// whole extraction.
    fn walk_bodies(&mut self) {
        let sema = Semantics::new(self.db);
        let db = self.db;
        let functions = self.functions.clone();
        for (function, from) in functions {
            let item_ids = &self.item_ids;
            let resolved = catch_unwind(AssertUnwindSafe(|| {
                body_targets(&sema, db, item_ids, function)
            }));
            if let Ok(targets) = resolved {
                for to in targets {
                    self.add_edge(from, to, RefKind::Body);
                }
            }
        }
    }

    /// Records one edge occurrence, skipping self-edges.
    fn add_edge(&mut self, from: ItemId, to: ItemId, kind: RefKind) {
        if from != to {
            *self.edges.entry((from, to, kind)).or_insert(0) += 1;
        }
    }

    fn finish(self) -> CrateGraph {
        let edges = self
            .edges
            .into_iter()
            .map(|((from, to, kind), weight)| Edge {
                from,
                to,
                kind,
                weight,
            })
            .collect();

        let reexports = self.reexports;
        let mut graph = CrateGraph {
            schema_version: SCHEMA_VERSION,
            ra_version: RA_VERSION.to_owned(),
            root_crate: CrateId(0),
            crates: self.crates,
            modules: self.modules,
            items: self.items,
            edges,
        };
        // A `pub use` only exposes its target if the re-exporting module is
        // itself publicly reachable.
        let public_reexports: HashSet<ItemId> = reexports
            .into_iter()
            .filter(|(module, _)| graph.module_public_reachable(*module))
            .map(|(_, item)| item)
            .collect();
        graph.compute_public_api_with_reexports(&public_reexports);
        graph
    }
}

/// The segments of a path as plain strings (`crate`/`self`/`super` keywords are
/// kept as those words).
fn path_segments(path: ast::Path) -> Vec<String> {
    let mut segments = Vec::new();
    let mut current = Some(path);
    while let Some(node) = current {
        if let Some(text) = node.segment().and_then(|seg| segment_text(&seg)) {
            segments.push(text);
        }
        current = node.qualifier();
    }
    segments.reverse();
    segments
}

/// The text of a single path segment.
fn segment_text(segment: &ast::PathSegment) -> Option<String> {
    if segment.crate_token().is_some() {
        return Some("crate".to_owned());
    }
    if segment.self_token().is_some() {
        return Some("self".to_owned());
    }
    if segment.super_token().is_some() {
        return Some("super".to_owned());
    }
    segment.name_ref().map(|n| n.text().to_string())
}

/// The `ast::Use` items declared directly in a module.
fn module_use_items(db: &RootDatabase, module: Module) -> Vec<ast::Use> {
    let items: Vec<ast::Item> = match module.definition_source(db).value {
        ModuleSource::SourceFile(file) => file.items().collect(),
        ModuleSource::Module(inline) => inline
            .item_list()
            .map(|list| list.items().collect())
            .unwrap_or_default(),
        ModuleSource::BlockExpr(_) => Vec::new(),
    };
    items
        .into_iter()
        .filter_map(|item| match item {
            ast::Item::Use(use_item) => Some(use_item),
            _ => None,
        })
        .collect()
}

/// Resolves the local items referenced inside a function body: path references,
/// method calls, and field accesses.
fn body_targets(
    sema: &Semantics<'_, RootDatabase>,
    db: &RootDatabase,
    item_ids: &HashMap<ModuleDef, ItemId>,
    function: hir::Function,
) -> Vec<ItemId> {
    let mut targets = Vec::new();
    let Some(source) = sema.source(function) else {
        return targets;
    };
    let Some(body) = source.value.body() else {
        return targets;
    };
    for node in body.syntax().descendants() {
        if let Some(path) = ast::Path::cast(node.clone()) {
            if let Some(PathResolution::Def(def)) = sema.resolve_path(&path) {
                if let Some(&to) = item_ids.get(&def) {
                    targets.push(to);
                }
            }
        } else if let Some(call) = ast::MethodCallExpr::cast(node.clone()) {
            if let Some(function) = sema.resolve_method_call(&call) {
                if let Some(&to) = item_ids.get(&ModuleDef::Function(function)) {
                    targets.push(to);
                }
            }
        } else if let Some(field_expr) = ast::FieldExpr::cast(node) {
            // A field access `x.field` couples to the ADT that owns the field.
            if let Some(field) = sema.resolve_field(&field_expr).and_then(|f| f.left()) {
                let owner = ModuleDef::from(field.parent_def(db));
                if let Some(&to) = item_ids.get(&owner) {
                    targets.push(to);
                }
            }
        }
    }
    targets
}

/// The lib/bin target root paths to analyze, plus the root lib target's path.
struct Selection {
    roots: HashSet<String>,
    lib_root: Option<String>,
}

/// Resolves a possibly-relative manifest path to an absolute UTF-8 path.
fn absolute_manifest(path: &std::path::Path) -> anyhow::Result<AbsPathBuf> {
    let joined = std::env::current_dir()?.join(path);
    let canonical = joined.canonicalize().unwrap_or(joined);
    let utf8 = Utf8PathBuf::from_path_buf(canonical)
        .map_err(|p| anyhow::anyhow!("path is not valid UTF-8: {}", p.display()))?;
    Ok(AbsPathBuf::assert(utf8))
}

/// Selects which cargo lib/bin targets to analyze, excluding test, bench, and
/// example targets.
fn select_targets(
    workspace: &ProjectWorkspace,
    manifest_abs: &AbsPathBuf,
    opts: &ExtractOptions,
) -> anyhow::Result<Selection> {
    let cargo = match &workspace.kind {
        ProjectWorkspaceKind::Cargo { cargo, .. } => cargo,
        _ => anyhow::bail!("modula only supports cargo workspaces"),
    };

    let members: Vec<_> = cargo.packages().filter(|p| cargo[*p].is_member).collect();
    let selected: Vec<_> = if opts.workspace {
        members.clone()
    } else if let Some(name) = &opts.package {
        members
            .iter()
            .copied()
            .filter(|p| cargo[*p].name == *name)
            .collect()
    } else {
        members
            .iter()
            .copied()
            .filter(|p| cargo[*p].manifest.as_str() == manifest_abs.as_str())
            .collect()
    };

    if selected.is_empty() {
        if let Some(name) = &opts.package {
            anyhow::bail!("package `{name}` not found in workspace");
        }
        anyhow::bail!("no package found at the given path; pass --package <NAME> or --workspace");
    }

    let mut roots = HashSet::new();
    let mut lib_root = None;
    for package in &selected {
        for target in &cargo[*package].targets {
            let target = &cargo[*target];
            match target.kind {
                TargetKind::Lib { .. } => {
                    let root = target.root.as_str().to_owned();
                    lib_root.get_or_insert_with(|| root.clone());
                    roots.insert(root);
                }
                TargetKind::Bin => {
                    roots.insert(target.root.as_str().to_owned());
                }
                _ => {}
            }
        }
    }
    Ok(Selection { roots, lib_root })
}

/// The absolute path of a crate's root source file, as a string.
fn crate_root_path(db: &RootDatabase, vfs: &Vfs, krate: Crate) -> Option<String> {
    let file = krate.root_file(db);
    vfs.file_path(file).as_path().map(|p| p.as_str().to_owned())
}

fn item_kind(def: ModuleDef) -> Option<ItemKind> {
    Some(match def {
        ModuleDef::Module(_) => return None,
        ModuleDef::Function(_) => ItemKind::Function,
        ModuleDef::Adt(hir::Adt::Struct(_)) => ItemKind::Struct,
        ModuleDef::Adt(hir::Adt::Enum(_)) => ItemKind::Enum,
        ModuleDef::Adt(hir::Adt::Union(_)) => ItemKind::Union,
        ModuleDef::EnumVariant(_) => ItemKind::EnumVariant,
        ModuleDef::Const(_) => ItemKind::Const,
        ModuleDef::Static(_) => ItemKind::Static,
        ModuleDef::Trait(_) => ItemKind::Trait,
        ModuleDef::TypeAlias(_) => ItemKind::TypeAlias,
        ModuleDef::Macro(_) => ItemKind::Macro,
        ModuleDef::BuiltinType(_) => ItemKind::BuiltinType,
    })
}

fn crate_name(db: &RootDatabase, krate: Crate) -> String {
    krate
        .display_name(db)
        .map(|name| name.to_string().replace('-', "_"))
        .unwrap_or_default()
}

/// Builds a module's canonical path (for example `my_crate::parser`).
fn module_path(db: &RootDatabase, module: Module, edition: Edition) -> String {
    let mut chain = module.path_to_root(db);
    chain.reverse();
    let mut parts = Vec::with_capacity(chain.len());
    for m in chain {
        if m.is_crate_root(db) {
            parts.push(crate_name(db, m.krate(db)));
        } else if let Some(name) = m.name(db) {
            parts.push(name.display(db, edition).to_string());
        }
    }
    parts.join("::")
}

/// A stable synthesized path for an item rust-analyzer reports no path for.
fn synthetic_path(
    db: &RootDatabase,
    def: ModuleDef,
    owning_module: ModuleId,
    id: ItemId,
    edition: Edition,
) -> String {
    let name = def
        .name(db)
        .map(|n| n.display(db, edition).to_string())
        .unwrap_or_else(|| "_".to_owned());
    format!("{{anon#{}#{}#{name}}}", owning_module.index(), id.index())
}

/// Maps a hir visibility to the IR visibility, relative to the item's owning
/// module so `pub(self)` and `pub(super)` are distinguished.
fn map_visibility(
    db: &RootDatabase,
    visibility: hir::Visibility,
    owning: Module,
    edition: Edition,
) -> Visibility {
    match visibility {
        hir::Visibility::Public => Visibility::Public,
        hir::Visibility::PubCrate(_) => Visibility::Crate,
        hir::Visibility::Module(module_id, _) => {
            let vis_module = Module::from(module_id);
            if vis_module.is_crate_root(db) {
                Visibility::Crate
            } else if vis_module == owning {
                Visibility::Private
            } else if Some(vis_module) == owning.parent(db) {
                Visibility::Super
            } else {
                Visibility::Module(module_path(db, vis_module, edition))
            }
        }
    }
}
