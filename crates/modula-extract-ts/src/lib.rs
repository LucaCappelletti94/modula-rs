//! TypeScript and JavaScript extraction of the modula-rs IR, built on `oxc`.
//!
//! It discovers the source files under a project, builds the directory and file
//! module tree, and records each file's top-level named declarations as items
//! (exported ones public, the rest module-local). Module-level import edges come
//! from `oxc_resolver` (a relative import becomes an edge between the two file
//! modules), and within-file reference edges come from `oxc_semantic` (an
//! identifier resolving to another top-level item in the same file). Still to
//! come: a type-container level for classes and interfaces, finer visibility,
//! and cross-file symbol-level edges.

#![forbid(unsafe_code)]

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result};
use modula_extract_api::{CrateGraphBuilder, ExtractRequest, Frontend, TypeScript};
use modula_ir::{ItemKind, ModuleId, RefKind, Visibility};
use oxc_allocator::Allocator;
use oxc_ast::ast::{BindingIdentifier, Declaration, IdentifierReference, Statement};
use oxc_ast_visit::Visit;
use oxc_parser::Parser;
use oxc_resolver::{ResolveOptions, Resolver};
use oxc_semantic::{Scoping, SemanticBuilder, SymbolId};
use oxc_span::SourceType;
use walkdir::WalkDir;

/// The `oxc` version this extractor is built against, recorded as IR provenance.
const OXC_VERSION: &str = "0.137";

/// Extensions treated as TypeScript or JavaScript source.
const SOURCE_EXTS: &[&str] = &["ts", "tsx", "mts", "cts", "js", "jsx", "mjs", "cjs"];

/// Directories never descended into during discovery.
const SKIP_DIRS: &[&str] = &["node_modules", ".git", "dist", "build", "out"];

/// The `oxc`-backed TypeScript and JavaScript extractor.
#[derive(Clone, Copy, Debug, Default)]
pub struct TsExtractor;

impl Frontend for TsExtractor {
    type Lang = TypeScript;

    fn detect(&self, root: &Path) -> bool {
        root.join("tsconfig.json").is_file() || root.join("package.json").is_file()
    }

    fn tool_version(&self) -> String {
        format!("oxc {OXC_VERSION}")
    }

    fn populate(&self, req: &ExtractRequest, builder: &mut CrateGraphBuilder) -> Result<()> {
        let root = &req.root;
        let crate_name = root
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("app")
            .to_owned();
        let (_crate_id, root_mod) = builder.add_crate(&crate_name, true, &crate_name);

        // Maps a relative directory to its module, so each directory is created
        // once. The crate root stands in for the empty relative directory.
        let mut dir_modules: HashMap<PathBuf, ModuleId> = HashMap::new();
        dir_modules.insert(PathBuf::new(), root_mod);

        // Maps a canonicalized source file to its module-stub key, so a resolved
        // import can be turned into an edge to that file's module.
        let mut key_of_file: HashMap<PathBuf, String> = HashMap::new();
        // (importing directory, importing module key, relative specifiers), held
        // until every file has a key so an import can resolve to any file.
        let mut pending: Vec<(PathBuf, String, Vec<String>)> = Vec::new();

        for file in discover(root) {
            let rel = file.strip_prefix(root).unwrap_or(&file);
            let parent_dir = rel.parent().unwrap_or_else(|| Path::new(""));
            let parent_mod = ensure_dir_modules(builder, &crate_name, parent_dir, &mut dir_modules);
            let stem = rel.file_stem().and_then(|s| s.to_str()).unwrap_or("module");
            let file_key = format!("{crate_name}::{}", rel.to_string_lossy());
            let file_mod = builder.add_module(parent_mod, &file_key, stem, Visibility::Public);
            let specifiers = collect_file(builder, file_mod, &file_key, &file)?;
            if let Ok(canonical) = file.canonicalize() {
                key_of_file.insert(canonical, file_key.clone());
            }
            let dir = file.parent().unwrap_or(root).to_path_buf();
            pending.push((dir, file_key, specifiers));
        }

        // Module-level import edges: a relative import becomes an edge from the
        // importing file's module to the resolved file's module. Bare (package)
        // specifiers were filtered out, and anything that resolves outside the
        // project or fails to resolve is dropped.
        let resolver = ts_resolver();
        for (dir, from_key, specifiers) in &pending {
            for specifier in specifiers {
                let Ok(resolution) = resolver.resolve(dir, specifier) else {
                    continue;
                };
                let Ok(canonical) = resolution.into_path_buf().canonicalize() else {
                    continue;
                };
                if let Some(to_key) = key_of_file.get(&canonical) {
                    builder.add_edge(from_key, to_key, RefKind::Import);
                }
            }
        }
        Ok(())
    }
}

/// A resolver tuned for TypeScript and JavaScript: it tries `.ts` first and lets
/// a `.js` specifier resolve to a `.ts` source (the TS authoring convention).
fn ts_resolver() -> Resolver {
    Resolver::new(ResolveOptions {
        extensions: SOURCE_EXTS.iter().map(|e| format!(".{e}")).collect(),
        extension_alias: vec![(
            ".js".to_owned(),
            vec![".ts".to_owned(), ".tsx".to_owned(), ".js".to_owned()],
        )],
        ..ResolveOptions::default()
    })
}

/// Discovers source files under `root`, pruning vendored and build directories.
fn discover(root: &Path) -> Vec<PathBuf> {
    WalkDir::new(root)
        .into_iter()
        .filter_entry(|e| {
            !(e.file_type().is_dir()
                && e.file_name()
                    .to_str()
                    .is_some_and(|n| SKIP_DIRS.contains(&n)))
        })
        .filter_map(Result::ok)
        .map(walkdir::DirEntry::into_path)
        .filter(|p| {
            p.is_file()
                && p.extension()
                    .and_then(|x| x.to_str())
                    .is_some_and(|x| SOURCE_EXTS.contains(&x))
        })
        .collect()
}

/// Ensures a module exists for `dir` (relative to the crate root) and every
/// ancestor, returning the id of `dir`'s module.
fn ensure_dir_modules(
    builder: &mut CrateGraphBuilder,
    crate_name: &str,
    dir: &Path,
    cache: &mut HashMap<PathBuf, ModuleId>,
) -> ModuleId {
    if let Some(&id) = cache.get(dir) {
        return id;
    }
    let parent = dir.parent().unwrap_or_else(|| Path::new(""));
    let parent_mod = ensure_dir_modules(builder, crate_name, parent, cache);
    let name = dir.file_name().and_then(|n| n.to_str()).unwrap_or("module");
    let key = format!("{crate_name}::{}/", dir.to_string_lossy());
    let id = builder.add_module(parent_mod, &key, name, Visibility::Public);
    cache.insert(dir.to_path_buf(), id);
    id
}

/// Parses one file, records its top-level named declarations as items with the
/// within-file reference edges between them, and returns the relative import
/// specifiers it found (for later cross-file edge resolution).
fn collect_file(
    builder: &mut CrateGraphBuilder,
    file_mod: ModuleId,
    file_key: &str,
    path: &Path,
) -> Result<Vec<String>> {
    let source =
        std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    let source_type = SourceType::from_path(path).unwrap_or_default();
    let allocator = Allocator::default();
    let parsed = Parser::new(&allocator, &source, source_type).parse();
    // Resolves identifier references to their symbols within the file, setting
    // the reference ids on the AST nodes in place via interior mutability.
    let semantic = SemanticBuilder::new().build(&parsed.program).semantic;
    let scoping = semantic.scoping();

    // First pass: an item per top-level named declaration, the import
    // specifiers, and a map from each item's symbol to its key.
    let mut specifiers = Vec::new();
    let mut targets: HashMap<SymbolId, String> = HashMap::new();
    for statement in &parsed.program.body {
        match statement {
            Statement::ImportDeclaration(import) => {
                push_relative(&mut specifiers, import.source.value.as_str());
            }
            Statement::ExportAllDeclaration(export) => {
                push_relative(&mut specifiers, export.source.value.as_str());
            }
            Statement::ExportNamedDeclaration(export) => {
                if let Some(declaration) = &export.declaration {
                    add_item_decl(
                        builder,
                        &mut targets,
                        file_mod,
                        file_key,
                        declaration,
                        Visibility::Public,
                    );
                } else if let Some(source) = &export.source {
                    push_relative(&mut specifiers, source.value.as_str());
                }
            }
            other => {
                if let Some(declaration) = other.as_declaration() {
                    add_item_decl(
                        builder,
                        &mut targets,
                        file_mod,
                        file_key,
                        declaration,
                        Visibility::Private,
                    );
                }
            }
        }
    }

    // Second pass: within-file reference edges. Walk each item's subtree and
    // emit an edge to every other top-level item its references resolve to.
    for statement in &parsed.program.body {
        let Some(declaration) = statement_declaration(statement) else {
            continue;
        };
        let Some(binding) = declaration_binding(declaration) else {
            continue;
        };
        let from_key = format!("{file_key}::{}", binding.name.as_str());
        let mut collector = RefCollector {
            scoping,
            targets: &targets,
            out: Vec::new(),
        };
        collector.visit_declaration(declaration);
        for to_key in collector.out {
            if to_key != from_key {
                builder.add_edge(&from_key, &to_key, RefKind::Body);
            }
        }
    }
    Ok(specifiers)
}

/// The declaration a statement introduces, whether bare or `export`ed.
fn statement_declaration<'b, 'a>(statement: &'b Statement<'a>) -> Option<&'b Declaration<'a>> {
    match statement {
        Statement::ExportNamedDeclaration(export) => export.declaration.as_ref(),
        other => other.as_declaration(),
    }
}

/// Records a relative module specifier (a within-project import). Bare package
/// specifiers (for example `react`) are ignored.
fn push_relative(specifiers: &mut Vec<String>, specifier: &str) {
    if specifier.starts_with('.') {
        specifiers.push(specifier.to_owned());
    }
}

/// Adds an item for a named declaration and maps its symbol to the item key.
fn add_item_decl(
    builder: &mut CrateGraphBuilder,
    targets: &mut HashMap<SymbolId, String>,
    file_mod: ModuleId,
    file_key: &str,
    declaration: &Declaration,
    visibility: Visibility,
) {
    let Some(kind) = declaration_kind(declaration) else {
        return;
    };
    let Some(binding) = declaration_binding(declaration) else {
        return;
    };
    let name = binding.name.as_str();
    let key = format!("{file_key}::{name}");
    builder.add_item(file_mod, &key, name, kind, visibility);
    if let Some(symbol_id) = binding.symbol_id.get() {
        targets.insert(symbol_id, key);
    }
}

/// The binding identifier a named declaration introduces, if any.
fn declaration_binding<'b, 'a>(
    declaration: &'b Declaration<'a>,
) -> Option<&'b BindingIdentifier<'a>> {
    match declaration {
        Declaration::FunctionDeclaration(function) => function.id.as_ref(),
        Declaration::ClassDeclaration(class) => class.id.as_ref(),
        Declaration::TSInterfaceDeclaration(interface) => Some(&interface.id),
        Declaration::TSTypeAliasDeclaration(alias) => Some(&alias.id),
        Declaration::TSEnumDeclaration(enumeration) => Some(&enumeration.id),
        _ => None,
    }
}

/// Visitor that records, for one declaration's subtree, the keys of the
/// top-level items its identifier references resolve to.
struct RefCollector<'s> {
    scoping: &'s Scoping,
    targets: &'s HashMap<SymbolId, String>,
    out: Vec<String>,
}

impl<'a> Visit<'a> for RefCollector<'_> {
    fn visit_identifier_reference(&mut self, reference: &IdentifierReference<'a>) {
        if let Some(reference_id) = reference.reference_id.get()
            && let Some(symbol_id) = self.scoping.get_reference(reference_id).symbol_id()
            && let Some(key) = self.targets.get(&symbol_id)
        {
            self.out.push(key.clone());
        }
    }
}

/// Maps a TypeScript or JavaScript declaration to an IR item kind. Returns
/// `None` for forms the skeleton does not yet record (variables, namespaces).
fn declaration_kind(declaration: &Declaration) -> Option<ItemKind> {
    Some(match declaration {
        Declaration::FunctionDeclaration(_) => ItemKind::Function,
        Declaration::ClassDeclaration(_) => ItemKind::Struct,
        Declaration::TSInterfaceDeclaration(_) => ItemKind::Trait,
        Declaration::TSTypeAliasDeclaration(_) => ItemKind::TypeAlias,
        Declaration::TSEnumDeclaration(_) => ItemKind::Enum,
        _ => return None,
    })
}
