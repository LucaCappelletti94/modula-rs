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
use std::process::Command;

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
    // The symbols that became items: the only valid edge sources. The fallback
    // attribution below uses this to ignore parameter and local definitions.
    let mut item_keys: HashSet<&str> = HashSet::new();
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
                item_keys.insert(key.as_str());
            }
            _ => {}
        }
    }

    // Edges: attribute each reference to the innermost enclosing definition.
    for doc in &index.documents {
        // Whether this document carries definition `enclosing_range`s at all.
        // Good indexers (scip-typescript, scip-python, scip-go, scip-java) do, so
        // a reference is placed in the definition whose range contains it. Some
        // indexers (scip-dotnet) omit them entirely, so for those documents we
        // fall back to the nearest preceding item definition.
        let has_enclosing = doc
            .occurrences
            .iter()
            .any(|occ| occ.symbol_roles & ROLE_DEFINITION != 0 && !occ.enclosing_range.is_empty());

        let mut def_ranges: Vec<(&str, Range)> = Vec::new();
        // Item definitions sorted by start position, for the no-enclosing fallback.
        let mut item_starts: Vec<(&str, (i32, i32))> = Vec::new();
        for occ in &doc.occurrences {
            if occ.symbol_roles & ROLE_DEFINITION == 0 {
                continue;
            }
            let raw = if occ.enclosing_range.is_empty() {
                &occ.range
            } else {
                &occ.enclosing_range
            };
            if let Some(range) = Range::from_vec(raw) {
                def_ranges.push((&occ.symbol, range));
            }
            if !has_enclosing
                && item_keys.contains(occ.symbol.as_str())
                && let Some(range) = Range::from_vec(&occ.range)
            {
                item_starts.push((&occ.symbol, (range.start_line, range.start_char)));
            }
        }
        item_starts.sort_by_key(|(_, start)| *start);

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
                .map(|(symbol, _)| *symbol)
                .or_else(|| {
                    // No enclosing definition. When the indexer omits enclosing
                    // ranges, attribute the reference to the nearest item defined
                    // before it. A reference preceding every item (for example a
                    // file-level namespace use) still has no owner and is dropped.
                    if has_enclosing {
                        return None;
                    }
                    let point = (reference.start_line, reference.start_char);
                    item_starts
                        .iter()
                        .take_while(|(_, start)| *start <= point)
                        .last()
                        .map(|(symbol, _)| *symbol)
                });
            // A reference with no enclosing definition (for example a Python
            // module-level import, whose module carries no file-spanning range)
            // is intentionally dropped rather than attributed to a synthetic owner.
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

/// A per-language SCIP indexer that modula can run to produce a `.scip` index.
///
/// Indexing happens in the user's environment (a built, dependency-installed
/// project), so this only assembles and runs a command, preferring an indexer
/// already on `PATH` and otherwise an ecosystem runner with a pinned version.
pub trait ScipIndexer {
    /// Whether `root` looks like a project this indexer handles.
    fn detect(&self, root: &Path) -> bool;

    /// The command that indexes `root`, writing the index to `output`. Prefers an
    /// on-`PATH` binary, else an ecosystem runner. `None` when neither is
    /// available, in which case the caller surfaces [`install_hint`](Self::install_hint).
    fn command(&self, root: &Path, output: &Path) -> Option<Command>;

    /// A one-line hint shown when the indexer cannot be assembled.
    fn install_hint(&self) -> &'static str;
}

/// The pinned `scip-typescript` version fetched via `npx` when it is not on `PATH`.
const SCIP_TYPESCRIPT_VERSION: &str = "0.3.16";

/// The TypeScript and JavaScript indexer, `scip-typescript`.
#[derive(Clone, Copy, Debug, Default)]
pub struct TypeScriptIndexer;

impl ScipIndexer for TypeScriptIndexer {
    fn detect(&self, root: &Path) -> bool {
        root.join("tsconfig.json").is_file() || root.join("package.json").is_file()
    }

    fn command(&self, root: &Path, output: &Path) -> Option<Command> {
        fn index_args(command: &mut Command, root: &Path, output: &Path) {
            command
                .arg("index")
                .arg("--cwd")
                .arg(root)
                .arg("--output")
                .arg(output)
                .arg("--no-progress-bar");
        }
        if on_path("scip-typescript") {
            let mut command = Command::new("scip-typescript");
            index_args(&mut command, root, output);
            Some(command)
        } else if on_path("npx") {
            let mut command = Command::new("npx");
            command.arg("-y").arg(format!(
                "@sourcegraph/scip-typescript@{SCIP_TYPESCRIPT_VERSION}"
            ));
            index_args(&mut command, root, output);
            Some(command)
        } else {
            None
        }
    }

    fn install_hint(&self) -> &'static str {
        "install Node so `npx` is available, or `npm i -g @sourcegraph/scip-typescript`"
    }
}

/// The pinned `scip-python` version fetched via `npx` when it is not on `PATH`.
const SCIP_PYTHON_VERSION: &str = "0.6.6";

/// The Python indexer, `scip-python` (built on pyright, distributed on npm).
///
/// Unlike `scip-typescript`, it resolves through the project's installed
/// environment, so for full results the project's dependencies should be
/// installed (in CI or locally) before indexing.
#[derive(Clone, Copy, Debug, Default)]
pub struct PythonIndexer;

impl ScipIndexer for PythonIndexer {
    fn detect(&self, root: &Path) -> bool {
        root.join("pyproject.toml").is_file()
            || root.join("setup.py").is_file()
            || root.join("setup.cfg").is_file()
    }

    fn command(&self, root: &Path, output: &Path) -> Option<Command> {
        let project = root
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("project");
        fn index_args(command: &mut Command, root: &Path, output: &Path, project: &str) {
            command
                .arg("index")
                .arg("--cwd")
                .arg(root)
                .arg("--output")
                .arg(output)
                .arg("--project-name")
                .arg(project)
                .arg("--quiet");
        }
        if on_path("scip-python") {
            let mut command = Command::new("scip-python");
            index_args(&mut command, root, output, project);
            Some(command)
        } else if on_path("npx") {
            let mut command = Command::new("npx");
            command
                .arg("-y")
                .arg(format!("@sourcegraph/scip-python@{SCIP_PYTHON_VERSION}"));
            index_args(&mut command, root, output, project);
            Some(command)
        } else {
            None
        }
    }

    fn install_hint(&self) -> &'static str {
        "install Node so `npx` is available, or `npm i -g @sourcegraph/scip-python`"
    }
}

/// The pinned `scip-go` version fetched via `go run` when it is not on `PATH`.
const SCIP_GO_VERSION: &str = "v0.2.7";

/// The Go indexer, `scip-go`. Distributed as a Go program (not npm), so the
/// fetch-and-run form uses `go run <package>@version`.
#[derive(Clone, Copy, Debug, Default)]
pub struct GoIndexer;

impl ScipIndexer for GoIndexer {
    fn detect(&self, root: &Path) -> bool {
        root.join("go.mod").is_file()
    }

    fn command(&self, root: &Path, output: &Path) -> Option<Command> {
        fn index_args(command: &mut Command, root: &Path, output: &Path) {
            command
                .arg("index")
                .arg("--module-root")
                .arg(root)
                .arg("--output")
                .arg(output)
                .arg("--quiet");
        }
        if on_path("scip-go") {
            let mut command = Command::new("scip-go");
            index_args(&mut command, root, output);
            Some(command)
        } else if on_path("go") {
            let mut command = Command::new("go");
            command
                .arg("run")
                .arg(format!(
                    "github.com/scip-code/scip-go/cmd/scip-go@{SCIP_GO_VERSION}"
                ))
                // scip-go needs a recent Go; let the toolchain upgrade itself.
                .env("GOTOOLCHAIN", "auto");
            index_args(&mut command, root, output);
            Some(command)
        } else {
            None
        }
    }

    fn install_hint(&self) -> &'static str {
        "install Go (for `go run`), or scip-go from github.com/scip-code/scip-go"
    }
}

/// The pinned `scip-java` version fetched via `coursier` when it is not on `PATH`.
const SCIP_JAVA_VERSION: &str = "0.12.3";

/// The JVM indexer, `scip-java` (Java, Kotlin, Scala, and other JVM languages).
///
/// It rides the project's own build tool (Maven, Gradle, or sbt) to compile
/// with the semanticdb plugin, so the project must be buildable in the
/// environment where indexing runs. `scip-java` reads the build from the current
/// directory rather than a `--cwd` flag, so the command runs with `root` as its
/// working directory. The fetch-and-run form uses `coursier` (`cs launch`),
/// which is how `scip-java` is normally distributed.
#[derive(Clone, Copy, Debug, Default)]
pub struct JvmIndexer;

impl ScipIndexer for JvmIndexer {
    fn detect(&self, root: &Path) -> bool {
        root.join("pom.xml").is_file()
            || root.join("build.gradle").is_file()
            || root.join("build.gradle.kts").is_file()
            || root.join("build.sbt").is_file()
    }

    fn command(&self, root: &Path, output: &Path) -> Option<Command> {
        if on_path("scip-java") {
            let mut command = Command::new("scip-java");
            command
                .current_dir(root)
                .arg("index")
                .arg("--output")
                .arg(output);
            Some(command)
        } else if on_path("cs") {
            let mut command = Command::new("cs");
            command
                .current_dir(root)
                .arg("launch")
                .arg(format!(
                    "com.sourcegraph:scip-java_2.13:{SCIP_JAVA_VERSION}"
                ))
                .arg("--")
                .arg("index")
                .arg("--output")
                .arg(output);
            Some(command)
        } else {
            None
        }
    }

    fn install_hint(&self) -> &'static str {
        "install coursier (`cs`) so scip-java can be fetched, see https://get-coursier.io"
    }
}

/// The C# (.NET) indexer, `scip-dotnet`.
///
/// It loads the project through MSBuild (running `dotnet restore` itself), so the
/// .NET SDK must be present. Unlike the other indexers there is no clean
/// fetch-and-run form: `scip-dotnet` is distributed as a stateful global tool
/// (`dotnet tool install --global scip-dotnet`), so when it is not on `PATH` the
/// command is `None` and the caller surfaces the install hint. It auto-detects
/// the `.sln`/`.csproj` in its working directory, so the command runs with `root`
/// as its working directory and only sets `--output`.
#[derive(Clone, Copy, Debug, Default)]
pub struct DotNetIndexer;

/// Whether `root` directly contains a file with one of `extensions` (a leading
/// dot each), used to spot a C# solution or project file.
fn has_file_with_extension(root: &Path, extensions: &[&str]) -> bool {
    let Ok(entries) = std::fs::read_dir(root) else {
        return false;
    };
    entries.filter_map(Result::ok).any(|entry| {
        let path = entry.path();
        path.is_file()
            && path
                .extension()
                .and_then(|ext| ext.to_str())
                .is_some_and(|ext| extensions.iter().any(|want| ext.eq_ignore_ascii_case(want)))
    })
}

impl ScipIndexer for DotNetIndexer {
    fn detect(&self, root: &Path) -> bool {
        has_file_with_extension(root, &["sln", "csproj"])
    }

    fn command(&self, root: &Path, output: &Path) -> Option<Command> {
        if on_path("scip-dotnet") {
            let mut command = Command::new("scip-dotnet");
            command
                .current_dir(root)
                .arg("index")
                .arg("--output")
                .arg(output);
            Some(command)
        } else {
            None
        }
    }

    fn install_hint(&self) -> &'static str {
        "install the .NET SDK, then `dotnet tool install --global scip-dotnet`"
    }
}

/// The C and C++ indexer, `scip-clang`.
///
/// It indexes from a JSON compilation database (`compile_commands.json`), which
/// the project's build system emits (CMake with `-DCMAKE_EXPORT_COMPILE_COMMANDS=ON`,
/// or Bazel/Meson). Like `scip-dotnet` there is no ephemeral runner: `scip-clang`
/// is a prebuilt binary, so when it is not on `PATH` the command is `None`. It
/// must run from the project root, so the command sets `root` as its working
/// directory and points `--compdb-path` at the database (checked in the root and
/// in a `build/` subdirectory). `scip-clang` does not emit definition enclosing
/// ranges, so edges come from the lowering's nearest-definition fallback.
#[derive(Clone, Copy, Debug, Default)]
pub struct CppIndexer;

impl CppIndexer {
    /// The compilation database location, if present, relative to `root`.
    fn compdb(root: &Path) -> Option<&'static str> {
        ["compile_commands.json", "build/compile_commands.json"]
            .into_iter()
            .find(|rel| root.join(rel).is_file())
    }
}

impl ScipIndexer for CppIndexer {
    fn detect(&self, root: &Path) -> bool {
        Self::compdb(root).is_some() || root.join("CMakeLists.txt").is_file()
    }

    fn command(&self, root: &Path, output: &Path) -> Option<Command> {
        if !on_path("scip-clang") {
            return None;
        }
        // Without a compilation database scip-clang cannot run, so the caller
        // surfaces the install hint, which explains how to produce one.
        let compdb = Self::compdb(root)?;
        let mut command = Command::new("scip-clang");
        command
            .current_dir(root)
            .arg(format!("--compdb-path={compdb}"))
            .arg(format!("--index-output-path={}", output.display()));
        Some(command)
    }

    fn install_hint(&self) -> &'static str {
        "install scip-clang from github.com/sourcegraph/scip-clang and emit a compile_commands.json (CMake: -DCMAKE_EXPORT_COMPILE_COMMANDS=ON)"
    }
}

/// The indexer that recognizes the project at `root`, if any.
#[must_use]
pub fn indexer_for(root: &Path) -> Option<Box<dyn ScipIndexer>> {
    let indexers: Vec<Box<dyn ScipIndexer>> = vec![
        Box::new(TypeScriptIndexer),
        Box::new(PythonIndexer),
        Box::new(GoIndexer),
        Box::new(JvmIndexer),
        Box::new(DotNetIndexer),
        Box::new(CppIndexer),
    ];
    indexers.into_iter().find(|indexer| indexer.detect(root))
}

/// Runs an indexer over `root` to a temporary index and lowers it. The index is
/// written into a private temp directory (unpredictable name, removed on every
/// exit path when the guard drops), avoiding shared-`/tmp` hazards and leaks.
pub fn run_indexer(indexer: &dyn ScipIndexer, root: &Path) -> Result<CrateGraph> {
    let dir = tempfile::Builder::new()
        .prefix("modula-scip-")
        .tempdir()
        .context("creating a temp directory for the SCIP index")?;
    let output = dir.path().join("index.scip");
    let mut command = indexer
        .command(root, &output)
        .ok_or_else(|| anyhow::anyhow!("no SCIP indexer available: {}", indexer.install_hint()))?;
    let status = command.status().context("running the SCIP indexer")?;
    anyhow::ensure!(status.success(), "the SCIP indexer exited with {status}");
    // `lower_path` reads the file fully into memory, so `dir` may drop after.
    lower_path(&output)
}

/// Whether an executable named `program` is found on `PATH`. Unix-oriented: it
/// does not consult Windows `PATHEXT` (`.cmd`/`.exe`), so on Windows the caller
/// falls back to the install hint.
fn on_path(program: &str) -> bool {
    std::env::var_os("PATH")
        .is_some_and(|paths| std::env::split_paths(&paths).any(|dir| dir.join(program).is_file()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> std::path::PathBuf {
        [env!("CARGO_MANIFEST_DIR"), "tests", "fixtures", "sample-ts"]
            .iter()
            .collect()
    }

    #[test]
    fn typescript_indexer_detects_a_project() {
        assert!(TypeScriptIndexer.detect(&fixture()));
        assert!(indexer_for(&fixture()).is_some());
        assert!(!TypeScriptIndexer.detect(Path::new("/nonexistent/place")));
    }

    #[test]
    fn typescript_command_targets_the_output() {
        let output = Path::new("/tmp/out.scip");
        let Some(command) = TypeScriptIndexer.command(&fixture(), output) else {
            return; // Neither scip-typescript nor npx on PATH in this environment.
        };
        let program = command.get_program().to_string_lossy().into_owned();
        assert!(
            program == "npx" || program == "scip-typescript",
            "unexpected program {program}"
        );
        let args: Vec<String> = command
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        let index_at = args.iter().position(|a| a == "index");
        assert!(index_at.is_some(), "{args:?}");
        assert!(args.iter().any(|a| a == "/tmp/out.scip"), "{args:?}");
        if program == "npx" {
            let pkg_at = args.iter().position(|a| a.contains("scip-typescript"));
            assert!(
                pkg_at < index_at,
                "npx package spec must precede the index subcommand: {args:?}"
            );
        }
    }

    fn py_fixture() -> std::path::PathBuf {
        [env!("CARGO_MANIFEST_DIR"), "tests", "fixtures", "sample-py"]
            .iter()
            .collect()
    }

    #[test]
    fn python_indexer_detects_a_project() {
        assert!(PythonIndexer.detect(&py_fixture()));
        // A Python project (no package.json or tsconfig.json) routes to Python.
        assert!(indexer_for(&py_fixture()).is_some());
        assert!(!PythonIndexer.detect(Path::new("/nonexistent/place")));
    }

    #[test]
    fn python_command_targets_the_output() {
        let output = Path::new("/tmp/out.scip");
        let Some(command) = PythonIndexer.command(&py_fixture(), output) else {
            return; // Neither scip-python nor npx on PATH in this environment.
        };
        let program = command.get_program().to_string_lossy().into_owned();
        assert!(
            program == "npx" || program == "scip-python",
            "unexpected program {program}"
        );
        let args: Vec<String> = command
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        let index_at = args.iter().position(|a| a == "index");
        assert!(index_at.is_some(), "{args:?}");
        assert!(args.iter().any(|a| a == "/tmp/out.scip"), "{args:?}");
        if program == "npx" {
            let pkg_at = args.iter().position(|a| a.contains("scip-python"));
            assert!(
                pkg_at < index_at,
                "npx package spec must precede the index subcommand: {args:?}"
            );
        }
    }

    fn go_fixture() -> std::path::PathBuf {
        [env!("CARGO_MANIFEST_DIR"), "tests", "fixtures", "sample-go"]
            .iter()
            .collect()
    }

    #[test]
    fn go_indexer_detects_a_project() {
        assert!(GoIndexer.detect(&go_fixture()));
        assert!(indexer_for(&go_fixture()).is_some());
        assert!(!GoIndexer.detect(Path::new("/nonexistent/place")));
    }

    #[test]
    fn go_command_targets_the_output() {
        let output = Path::new("/tmp/out.scip");
        let Some(command) = GoIndexer.command(&go_fixture(), output) else {
            return; // Neither scip-go nor go on PATH in this environment.
        };
        let program = command.get_program().to_string_lossy().into_owned();
        assert!(
            program == "go" || program == "scip-go",
            "unexpected program {program}"
        );
        let args: Vec<String> = command
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        let index_at = args.iter().position(|a| a == "index");
        assert!(index_at.is_some(), "{args:?}");
        assert!(args.iter().any(|a| a == "/tmp/out.scip"), "{args:?}");
        if program == "go" {
            let pkg_at = args.iter().position(|a| a.contains("scip-go"));
            assert!(
                pkg_at < index_at,
                "go run package spec must precede the index subcommand: {args:?}"
            );
        }
    }

    fn java_fixture() -> std::path::PathBuf {
        [
            env!("CARGO_MANIFEST_DIR"),
            "tests",
            "fixtures",
            "sample-java",
        ]
        .iter()
        .collect()
    }

    #[test]
    fn jvm_indexer_detects_a_project() {
        assert!(JvmIndexer.detect(&java_fixture()));
        assert!(indexer_for(&java_fixture()).is_some());
        assert!(!JvmIndexer.detect(Path::new("/nonexistent/place")));
    }

    #[test]
    fn jvm_command_targets_the_output() {
        let output = Path::new("/tmp/out.scip");
        let Some(command) = JvmIndexer.command(&java_fixture(), output) else {
            return; // Neither scip-java nor coursier (`cs`) on PATH in this environment.
        };
        let program = command.get_program().to_string_lossy().into_owned();
        assert!(
            program == "cs" || program == "scip-java",
            "unexpected program {program}"
        );
        // scip-java reads the build from its working directory, not a flag.
        assert_eq!(
            command.get_current_dir(),
            Some(java_fixture().as_path()),
            "scip-java must run in the project root"
        );
        let args: Vec<String> = command
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        let index_at = args.iter().position(|a| a == "index");
        assert!(index_at.is_some(), "{args:?}");
        assert!(args.iter().any(|a| a == "/tmp/out.scip"), "{args:?}");
        if program == "cs" {
            let pkg_at = args.iter().position(|a| a.contains("scip-java"));
            assert!(
                pkg_at < index_at,
                "coursier artifact spec must precede the index subcommand: {args:?}"
            );
        }
    }

    fn csharp_fixture() -> std::path::PathBuf {
        [
            env!("CARGO_MANIFEST_DIR"),
            "tests",
            "fixtures",
            "sample-csharp",
        ]
        .iter()
        .collect()
    }

    #[test]
    fn dotnet_indexer_detects_a_project() {
        assert!(DotNetIndexer.detect(&csharp_fixture()));
        assert!(indexer_for(&csharp_fixture()).is_some());
        assert!(!DotNetIndexer.detect(Path::new("/nonexistent/place")));
        // The other fixtures carry no .sln/.csproj, so C# never claims them.
        assert!(!DotNetIndexer.detect(&go_fixture()));
    }

    #[test]
    fn dotnet_command_targets_the_output() {
        let output = Path::new("/tmp/out.scip");
        let Some(command) = DotNetIndexer.command(&csharp_fixture(), output) else {
            return; // scip-dotnet not on PATH in this environment.
        };
        assert_eq!(
            command.get_program().to_string_lossy(),
            "scip-dotnet",
            "C# has no ephemeral runner, so only the on-PATH binary is used"
        );
        // scip-dotnet auto-detects the project in its working directory.
        assert_eq!(
            command.get_current_dir(),
            Some(csharp_fixture().as_path()),
            "scip-dotnet must run in the project root"
        );
        let args: Vec<String> = command
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        assert!(args.iter().any(|a| a == "index"), "{args:?}");
        assert!(args.iter().any(|a| a == "/tmp/out.scip"), "{args:?}");
    }

    fn cpp_fixture() -> std::path::PathBuf {
        [
            env!("CARGO_MANIFEST_DIR"),
            "tests",
            "fixtures",
            "sample-cpp",
        ]
        .iter()
        .collect()
    }

    #[test]
    fn cpp_indexer_detects_a_project() {
        // The fixture has a CMakeLists.txt (the committed index needs no compdb).
        assert!(CppIndexer.detect(&cpp_fixture()));
        assert!(indexer_for(&cpp_fixture()).is_some());
        assert!(!CppIndexer.detect(Path::new("/nonexistent/place")));
        // A Go project carries no CMakeLists.txt or compilation database.
        assert!(!CppIndexer.detect(&go_fixture()));
    }

    #[test]
    fn cpp_command_needs_a_compilation_database() {
        // The committed fixture has no compile_commands.json, so even if
        // scip-clang were on PATH there is nothing to index and no command.
        assert!(
            CppIndexer
                .command(&cpp_fixture(), Path::new("/tmp/out.scip"))
                .is_none(),
            "without a compilation database the command must be None"
        );
    }
}
