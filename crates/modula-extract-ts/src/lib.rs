//! TypeScript and JavaScript extraction of the modula-rs IR, built on `oxc`.
//!
//! Walking skeleton: discover the source files under a project, build the
//! directory and file module tree, and record each file's top-level named
//! declarations as items (exported ones public, the rest module-local). No
//! dependency edges yet. Those arrive in later steps, imports via `oxc_resolver`
//! and within-file references via `oxc_semantic`.

#![forbid(unsafe_code)]

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result};
use modula_extract_api::{CrateGraphBuilder, ExtractRequest, Frontend, TypeScript};
use modula_ir::{ItemKind, ModuleId, Visibility};
use oxc_allocator::Allocator;
use oxc_ast::ast::{Declaration, Statement};
use oxc_parser::Parser;
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

        for file in discover(root) {
            let rel = file.strip_prefix(root).unwrap_or(&file);
            let parent_dir = rel.parent().unwrap_or_else(|| Path::new(""));
            let parent_mod = ensure_dir_modules(builder, &crate_name, parent_dir, &mut dir_modules);
            let stem = rel.file_stem().and_then(|s| s.to_str()).unwrap_or("module");
            let file_key = format!("{crate_name}::{}", rel.to_string_lossy());
            let file_mod = builder.add_module(parent_mod, &file_key, stem, Visibility::Public);
            add_file_items(builder, file_mod, &file_key, &file)?;
        }
        Ok(())
    }
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

/// Parses one file and records its top-level named declarations as items.
fn add_file_items(
    builder: &mut CrateGraphBuilder,
    file_mod: ModuleId,
    file_key: &str,
    path: &Path,
) -> Result<()> {
    let source =
        std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    let source_type = SourceType::from_path(path).unwrap_or_default();
    let allocator = Allocator::default();
    let parsed = Parser::new(&allocator, &source, source_type).parse();

    for statement in &parsed.program.body {
        match statement {
            Statement::ExportNamedDeclaration(export) => {
                if let Some(declaration) = &export.declaration {
                    add_declaration(builder, file_mod, file_key, declaration, Visibility::Public);
                }
            }
            other => {
                if let Some(declaration) = other.as_declaration() {
                    add_declaration(
                        builder,
                        file_mod,
                        file_key,
                        declaration,
                        Visibility::Private,
                    );
                }
            }
        }
    }
    Ok(())
}

/// Records a single named declaration as an item.
fn add_declaration(
    builder: &mut CrateGraphBuilder,
    file_mod: ModuleId,
    file_key: &str,
    declaration: &Declaration,
    visibility: Visibility,
) {
    let Some(kind) = declaration_kind(declaration) else {
        return;
    };
    let Some(name) = declaration.id().map(|ident| ident.name.as_str().to_owned()) else {
        return;
    };
    let key = format!("{file_key}::{name}");
    builder.add_item(file_mod, &key, &name, kind, visibility);
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
