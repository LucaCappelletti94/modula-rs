//! Lowers a SCIP code index into the modula-rs IR.
//!
//! SCIP (Sourcegraph's protobuf code-index format) is emitted by per-language
//! indexers that ride each language's real compiler, so the references it records
//! are type-resolved. This crate turns one `.scip` index into a
//! [`modula_ir::CrateGraph`] through the shared [`CrateGraphBuilder`], which is
//! how modula supports any language that has a SCIP indexer.
//!
//! The mapping: every SCIP symbol carries descriptors that encode its hierarchy
//! (`Namespace` becomes a module, `Type` a type container, `Method`/`Term`/`Macro`
//! an item), and a definition occurrence carries an `enclosing_range`, so each
//! reference occurrence is attributed to the innermost definition whose range
//! contains it (the edge `from`) with the referenced symbol as the `to`.
//!
//! Two honest caveats versus a bespoke extractor like the Rust one: SCIP has no
//! visibility field, so visibility is approximated from the global-versus-local
//! distinction, and SCIP has no reference-kind taxonomy, so edges collapse to
//! `Import` (when the occurrence has the import role) or `Body`.

#![forbid(unsafe_code)]

use std::collections::{HashMap, HashSet};
use std::path::Path;

use anyhow::{Context as _, Result};
use modula_extract_api::CrateGraphBuilder;
use modula_ir::{CrateGraph, ItemKind, ModuleId, RefKind, Visibility};
use protobuf::Message as _;
use scip::symbol::{is_global_symbol, parse_symbol};
use scip::types::{Index, Symbol, descriptor::Suffix};

/// SCIP `SymbolRole` bit for a definition occurrence.
const ROLE_DEFINITION: i32 = 1;
/// SCIP `SymbolRole` bit for an import occurrence.
const ROLE_IMPORT: i32 = 2;

/// Reads and lowers a `.scip` index file.
pub fn lower_path(path: &Path) -> Result<CrateGraph> {
    let bytes = std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
    let index = Index::parse_from_bytes(&bytes).context("decoding SCIP index")?;
    lower(&index)
}

/// Lowers an in-memory SCIP index into a `CrateGraph`.
pub fn lower(index: &Index) -> Result<CrateGraph> {
    let provenance = index
        .metadata
        .as_ref()
        .and_then(|m| m.tool_info.as_ref())
        .map(|t| format!("{} {}", t.name, t.version))
        .unwrap_or_default();
    let mut builder = CrateGraphBuilder::new(provenance);

    let crate_name = index
        .metadata
        .as_ref()
        .and_then(|m| project_basename(&m.project_root))
        .unwrap_or_else(|| "crate".to_owned());
    let (_crate_id, root) = builder.add_crate(&crate_name, true, &format!("<root:{crate_name}>"));

    // Local definitions: the documents' SymbolInformation, deduped and limited to
    // global (non-local) symbols. Parameters and other non-item descriptors are
    // dropped later by the suffix match, not here.
    let mut seen = HashSet::new();
    let mut defs: Vec<(String, Symbol)> = Vec::new();
    for doc in &index.documents {
        for si in &doc.symbols {
            if !is_global_symbol(&si.symbol) || !seen.insert(si.symbol.clone()) {
                continue;
            }
            if let Ok(symbol) = parse_symbol(&si.symbol)
                && !symbol.descriptors.is_empty()
            {
                defs.push((si.symbol.clone(), symbol));
            }
        }
    }

    // Maps a descriptor signature to its real SCIP symbol, so an emitted module
    // (for example a file namespace) uses its real key while intermediate
    // namespaces that the indexer does not emit get a synthetic key.
    let by_sig: HashMap<Vec<(String, i32)>, String> = defs
        .iter()
        .map(|(key, sym)| (signature(&sym.descriptors), key.clone()))
        .collect();

    let mut module_of_sig: HashMap<Vec<(String, i32)>, ModuleId> = HashMap::new();
    let mut type_modules: HashSet<ModuleId> = HashSet::new();
    for (key, sym) in &defs {
        let descriptors = &sym.descriptors;
        let n = descriptors.len();
        let sig = signature(descriptors);
        let suffix = descriptors[n - 1].suffix.enum_value_or_default();
        match suffix {
            Suffix::Namespace | Suffix::Package | Suffix::Type => {
                ensure_module(
                    &sig,
                    root,
                    &by_sig,
                    &mut builder,
                    &mut module_of_sig,
                    &mut type_modules,
                );
            }
            Suffix::Method | Suffix::Term | Suffix::Macro => {
                let parent = ensure_module(
                    &sig[..n - 1],
                    root,
                    &by_sig,
                    &mut builder,
                    &mut module_of_sig,
                    &mut type_modules,
                );
                let parent_is_type = type_modules.contains(&parent);
                let name = descriptor_name(&descriptors[n - 1].name);
                let kind = match (suffix, parent_is_type) {
                    (Suffix::Method, true) => ItemKind::AssocFn,
                    (Suffix::Method, false) => ItemKind::Function,
                    (Suffix::Term, true) => ItemKind::AssocConst,
                    (Suffix::Term, false) => ItemKind::Const,
                    _ => ItemKind::Macro,
                };
                builder.add_item(parent, key, name, kind, Visibility::Public);
            }
            _ => {}
        }
    }

    // Edges: attribute each reference to the innermost enclosing definition.
    for doc in &index.documents {
        let mut def_ranges: Vec<(&str, Range)> = Vec::new();
        for occ in &doc.occurrences {
            if occ.symbol_roles & ROLE_DEFINITION != 0 {
                let raw = if occ.enclosing_range.is_empty() {
                    &occ.range
                } else {
                    &occ.enclosing_range
                };
                if let Some(range) = Range::from_vec(raw) {
                    def_ranges.push((&occ.symbol, range));
                }
            }
        }
        for occ in &doc.occurrences {
            if occ.symbol_roles & ROLE_DEFINITION != 0 {
                continue;
            }
            let Some(reference) = Range::from_vec(&occ.range) else {
                continue;
            };
            let from = def_ranges
                .iter()
                .filter(|(_, range)| range.contains(&reference))
                .min_by_key(|(_, range)| range.span())
                .map(|(symbol, _)| *symbol);
            let Some(from) = from else {
                continue;
            };
            let kind = if occ.symbol_roles & ROLE_IMPORT != 0 {
                RefKind::Import
            } else {
                RefKind::Body
            };
            builder.add_edge(from, &occ.symbol, kind);
        }
    }

    builder.finish()
}

/// The descriptor signature (name plus suffix value) used to identify a symbol's
/// ancestors without depending on exact symbol-string formatting.
fn signature(descriptors: &[scip::types::Descriptor]) -> Vec<(String, i32)> {
    descriptors
        .iter()
        .map(|d| (d.name.clone(), d.suffix.value()))
        .collect()
}

/// The proto number of `Descriptor.Suffix.Type`.
const SUFFIX_TYPE: i32 = 2;

/// Ensures a module (or type container) exists for the descriptor prefix `sig`
/// and all of its ancestors, returning its id. Intermediate namespaces the
/// indexer did not emit as their own symbol get a synthetic key.
fn ensure_module(
    sig: &[(String, i32)],
    root: ModuleId,
    by_sig: &HashMap<Vec<(String, i32)>, String>,
    builder: &mut CrateGraphBuilder,
    module_of_sig: &mut HashMap<Vec<(String, i32)>, ModuleId>,
    type_modules: &mut HashSet<ModuleId>,
) -> ModuleId {
    if sig.is_empty() {
        return root;
    }
    if let Some(&module) = module_of_sig.get(sig) {
        return module;
    }
    let parent = ensure_module(
        &sig[..sig.len() - 1],
        root,
        by_sig,
        builder,
        module_of_sig,
        type_modules,
    );
    let (name, suffix) = &sig[sig.len() - 1];
    let display = descriptor_name(name);
    let key = by_sig
        .get(sig)
        .cloned()
        .unwrap_or_else(|| synthetic_key(sig));
    let module = if *suffix == SUFFIX_TYPE {
        let (module, _item) =
            builder.add_type(parent, &key, display, ItemKind::Struct, Visibility::Public);
        type_modules.insert(module);
        module
    } else {
        builder.add_module(parent, &key, display, Visibility::Public)
    };
    module_of_sig.insert(sig.to_vec(), module);
    module
}

/// A descriptor name, substituting a placeholder for the empty name.
fn descriptor_name(name: &str) -> &str {
    if name.is_empty() { "_" } else { name }
}

/// A stable key for an intermediate namespace the indexer did not emit. Cannot
/// collide with a real SCIP symbol, which always begins with a scheme name.
fn synthetic_key(sig: &[(String, i32)]) -> String {
    let mut key = String::from("<ns>");
    for (name, suffix) in sig {
        key.push('\u{1}');
        key.push_str(name);
        key.push('\u{2}');
        key.push_str(&suffix.to_string());
    }
    key
}

/// The final path segment of a `file://` project root, used as the crate name.
fn project_basename(project_root: &str) -> Option<String> {
    project_root
        .trim_end_matches('/')
        .rsplit('/')
        .next()
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
}

/// A normalized SCIP source range.
#[derive(Clone, Copy)]
struct Range {
    start_line: i32,
    start_char: i32,
    end_line: i32,
    end_char: i32,
}

impl Range {
    /// SCIP ranges are `[startLine, startChar, endChar]` for a single line or
    /// `[startLine, startChar, endLine, endChar]` otherwise.
    fn from_vec(v: &[i32]) -> Option<Range> {
        match *v {
            [start_line, start_char, end_char] => Some(Range {
                start_line,
                start_char,
                end_line: start_line,
                end_char,
            }),
            [start_line, start_char, end_line, end_char] => Some(Range {
                start_line,
                start_char,
                end_line,
                end_char,
            }),
            _ => None,
        }
    }

    fn contains(&self, other: &Range) -> bool {
        (self.start_line, self.start_char) <= (other.start_line, other.start_char)
            && (other.end_line, other.end_char) <= (self.end_line, self.end_char)
    }

    fn span(&self) -> (i32, i32) {
        (
            self.end_line - self.start_line,
            self.end_char - self.start_char,
        )
    }
}
