//! Compact binary container for [`CrateGraph`].
//!
//! A naive serialization of [`CrateGraph`] is large on disk and slow to parse:
//! most of its bytes are item and module canonical paths that repeat their
//! module prefix on every entry, and its edge list (the bulk of large crates)
//! spends dozens of ASCII bytes per edge that a binary varint encodes in a
//! handful. This module is the compact format that shrinks the stored and parsed
//! bytes dramatically while keeping the logical shape of [`CrateGraph`]
//! unchanged, so every consumer keeps working.
//!
//! Two levers, both measured before adoption. First, a [`CompactGraph`] mirror
//! drops the fields that are derivable from the module tree: a module's
//! `canonical_path` and `depth` (rebuilt from `parent` plus `name`), and an
//! item's `canonical_path`, replaced by a small suffix enum relative to its
//! owning module. Second, the mirror is encoded with [`postcard`] (varint, no
//! field names). The bytes are framed by a tiny header carrying the format
//! version, codec, and compression.
//!
//! Compression is split to respect the workspace firewall. Decoding uses the
//! pure-Rust [`ruzstd`] decoder, which builds for `wasm32-unknown-unknown`, so
//! [`read_container`] works in the web app. Compression on write uses the
//! C-backed `zstd` crate, which only the native crates depend on. Callers
//! therefore compress the payload themselves and hand it to [`wrap_container`];
//! [`read_container`] decompresses transparently.

use std::borrow::Cow;
use std::collections::HashMap;
use std::io::Read;

use serde::{Deserialize, Serialize};

use crate::{
    Crate, CrateGraph, CrateId, Edge, Item, ItemKind, Module, ModuleId, ModuleKind, Visibility,
};

/// Magic bytes that mark a binary IR container.
pub const CONTAINER_MAGIC: [u8; 4] = *b"MIRz";

/// The container framing version, distinct from
/// [`SCHEMA_VERSION`](crate::SCHEMA_VERSION): this versions the on-disk envelope
/// (header plus codec plus compression), not the logical shape of the graph.
pub const FORMAT_VERSION: u8 = 2;

/// The number of header bytes before the payload: magic (4) plus three `u8`
/// fields (format version, codec, compression).
const HEADER_LEN: usize = 7;

/// How the payload after the header is serialized.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Codec {
    /// [`postcard`] encoding of the [`CompactGraph`] mirror.
    PostcardCompact = 0,
}

/// How the serialized payload is compressed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Compression {
    /// Stored uncompressed.
    None = 0,
    /// Compressed with zstd (single frame).
    Zstd = 1,
}

/// An error reading or writing an IR container.
#[derive(Debug, thiserror::Error)]
pub enum ContainerError {
    /// The payload could not be encoded or decoded as postcard.
    #[error("postcard codec error: {0}")]
    Postcard(#[from] postcard::Error),
    /// The bytes had the container magic but were shorter than the header.
    #[error("container is truncated (shorter than the {HEADER_LEN}-byte header)")]
    Truncated,
    /// The container framing version is not understood by this build.
    #[error("unsupported container format version {0} (this build writes {FORMAT_VERSION})")]
    UnsupportedFormat(u8),
    /// The codec id in the header is not understood by this build.
    #[error("unsupported codec id {0}")]
    UnsupportedCodec(u8),
    /// The compression id in the header is not understood by this build.
    #[error("unsupported compression id {0}")]
    UnsupportedCompression(u8),
    /// zstd decompression of the payload failed.
    #[error("zstd decompression failed: {0}")]
    Decompress(String),
    /// The bytes did not start with the container magic.
    #[error("not an IR container (missing the MIRz header)")]
    NotContainer,
}

impl TryFrom<u8> for Codec {
    type Error = ContainerError;
    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            0 => Ok(Codec::PostcardCompact),
            other => Err(ContainerError::UnsupportedCodec(other)),
        }
    }
}

impl TryFrom<u8> for Compression {
    type Error = ContainerError;
    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            0 => Ok(Compression::None),
            1 => Ok(Compression::Zstd),
            other => Err(ContainerError::UnsupportedCompression(other)),
        }
    }
}

/// Encodes a graph into the compact postcard payload, without header or
/// compression. Native callers compress the result and pass it to
/// [`wrap_container`]; tests and uncompressed writers can wrap it directly.
pub fn encode_compact(graph: &CrateGraph) -> Result<Vec<u8>, ContainerError> {
    Ok(postcard::to_allocvec(&CompactGraph::from_graph(graph))?)
}

/// Decodes a compact postcard payload (no header) back into a graph,
/// reconstructing the derived module and item paths and module depths.
pub fn decode_compact(payload: &[u8]) -> Result<CrateGraph, ContainerError> {
    let compact: CompactGraph = postcard::from_bytes(payload)?;
    Ok(compact.into_graph())
}

/// Prepends the container header to an already-encoded (and, for
/// [`Compression::Zstd`], already-compressed) payload.
#[must_use]
pub fn wrap_container(payload: &[u8], codec: Codec, compression: Compression) -> Vec<u8> {
    let mut out = Vec::with_capacity(HEADER_LEN + payload.len());
    out.extend_from_slice(&CONTAINER_MAGIC);
    out.push(FORMAT_VERSION);
    out.push(codec as u8);
    out.push(compression as u8);
    out.extend_from_slice(payload);
    out
}

/// Reads a graph from a binary container.
///
/// The bytes must start with [`CONTAINER_MAGIC`]; the header is parsed and the
/// payload is optionally zstd-decompressed (via the pure-Rust decoder) and then
/// decoded by its codec. Legacy verbose JSON is no longer accepted here (it was
/// migrated away with `modula-corpus convert`).
pub fn read_container(bytes: &[u8]) -> Result<CrateGraph, ContainerError> {
    if bytes.len() < CONTAINER_MAGIC.len() || bytes[..CONTAINER_MAGIC.len()] != CONTAINER_MAGIC {
        return Err(ContainerError::NotContainer);
    }
    if bytes.len() < HEADER_LEN {
        return Err(ContainerError::Truncated);
    }
    let format = bytes[4];
    if format != FORMAT_VERSION {
        return Err(ContainerError::UnsupportedFormat(format));
    }
    let codec = Codec::try_from(bytes[5])?;
    let compression = Compression::try_from(bytes[6])?;
    let payload = &bytes[HEADER_LEN..];
    let raw = match compression {
        Compression::None => Cow::Borrowed(payload),
        Compression::Zstd => Cow::Owned(zstd_decode(payload)?),
    };
    match codec {
        Codec::PostcardCompact => decode_compact(&raw),
    }
}

/// Decompresses a single zstd frame with the pure-Rust decoder.
fn zstd_decode(payload: &[u8]) -> Result<Vec<u8>, ContainerError> {
    let mut decoder = ruzstd::decoding::StreamingDecoder::new(payload)
        .map_err(|e| ContainerError::Decompress(e.to_string()))?;
    let mut out = Vec::new();
    decoder
        .read_to_end(&mut out)
        .map_err(|e| ContainerError::Decompress(e.to_string()))?;
    Ok(out)
}

// ---------- the compact mirror ----------

/// A [`CrateGraph`] with the tree-derivable fields removed, for compact
/// serialization. Crates and edges are already minimal and are kept as-is.
#[derive(Serialize, Deserialize)]
struct CompactGraph {
    schema_version: u32,
    ra_version: String,
    root_crate: CrateId,
    crates: Vec<Crate>,
    modules: Vec<CompactModule>,
    items: Vec<CompactItem>,
    edges: Vec<Edge>,
}

/// A [`Module`] without `canonical_path` and `depth`, both rebuilt from the tree.
#[derive(Serialize, Deserialize)]
struct CompactModule {
    id: ModuleId,
    crate_id: CrateId,
    parent: Option<ModuleId>,
    name: String,
    visibility: Visibility,
    kind: ModuleKind,
}

/// An item's canonical path expressed relative to its owning module.
#[derive(Serialize, Deserialize)]
enum CompactName {
    /// `module_path::suffix` for a normal item under a module.
    Leaf(String),
    /// The item path equals its owning module's path (type-container and module
    /// stub items).
    SameAsModule,
    /// A path that is not a `::`-suffix of the module path (builtin types, the
    /// synthetic `{anon#..}` form). Stored verbatim.
    Full(String),
}

/// An [`Item`] with its `canonical_path` replaced by a [`CompactName`].
#[derive(Serialize, Deserialize)]
struct CompactItem {
    id: crate::ItemId,
    name: CompactName,
    kind: ItemKind,
    visibility: Visibility,
    owning_module: ModuleId,
    crate_id: CrateId,
    has_canonical_path: bool,
    reachable_pub_api: bool,
    visibility_fixed_by_trait: bool,
}

impl CompactGraph {
    fn from_graph(graph: &CrateGraph) -> Self {
        let module_path: HashMap<ModuleId, &str> = graph
            .modules
            .iter()
            .map(|m| (m.id, m.canonical_path.as_str()))
            .collect();
        CompactGraph {
            schema_version: graph.schema_version,
            ra_version: graph.ra_version.clone(),
            root_crate: graph.root_crate,
            crates: graph.crates.clone(),
            modules: graph
                .modules
                .iter()
                .map(|m| CompactModule {
                    id: m.id,
                    crate_id: m.crate_id,
                    parent: m.parent,
                    name: m.name.clone(),
                    visibility: m.visibility.clone(),
                    kind: m.kind,
                })
                .collect(),
            items: graph
                .items
                .iter()
                .map(|it| {
                    let mp = module_path.get(&it.owning_module).copied().unwrap_or("");
                    let cp = it.canonical_path.as_str();
                    let name = if cp == mp {
                        CompactName::SameAsModule
                    } else if let Some(suffix) =
                        cp.strip_prefix(mp).and_then(|s| s.strip_prefix("::"))
                    {
                        CompactName::Leaf(suffix.to_owned())
                    } else {
                        CompactName::Full(cp.to_owned())
                    };
                    CompactItem {
                        id: it.id,
                        name,
                        kind: it.kind,
                        visibility: it.visibility.clone(),
                        owning_module: it.owning_module,
                        crate_id: it.crate_id,
                        has_canonical_path: it.has_canonical_path,
                        reachable_pub_api: it.reachable_pub_api,
                        visibility_fixed_by_trait: it.visibility_fixed_by_trait,
                    }
                })
                .collect(),
            edges: graph.edges.clone(),
        }
    }

    fn into_graph(self) -> CrateGraph {
        let (modules, items) = {
            let by_id: HashMap<ModuleId, &CompactModule> =
                self.modules.iter().map(|m| (m.id, m)).collect();
            let crate_name: HashMap<CrateId, &str> = self
                .crates
                .iter()
                .map(|c| (c.id, c.name.as_str()))
                .collect();
            let mut path_memo: HashMap<ModuleId, String> = HashMap::new();
            let mut depth_memo: HashMap<ModuleId, u32> = HashMap::new();
            let modules: Vec<Module> = self
                .modules
                .iter()
                .map(|m| Module {
                    id: m.id,
                    crate_id: m.crate_id,
                    parent: m.parent,
                    name: m.name.clone(),
                    canonical_path: module_path(m.id, &by_id, &crate_name, &mut path_memo),
                    depth: module_depth(m.id, &by_id, &mut depth_memo),
                    visibility: m.visibility.clone(),
                    kind: m.kind,
                })
                .collect();
            let items: Vec<Item> = self
                .items
                .iter()
                .map(|it| {
                    let mp = module_path(it.owning_module, &by_id, &crate_name, &mut path_memo);
                    let canonical_path = match &it.name {
                        CompactName::SameAsModule => mp,
                        CompactName::Leaf(suffix) => format!("{mp}::{suffix}"),
                        CompactName::Full(path) => path.clone(),
                    };
                    Item {
                        id: it.id,
                        canonical_path,
                        kind: it.kind,
                        visibility: it.visibility.clone(),
                        owning_module: it.owning_module,
                        crate_id: it.crate_id,
                        has_canonical_path: it.has_canonical_path,
                        reachable_pub_api: it.reachable_pub_api,
                        visibility_fixed_by_trait: it.visibility_fixed_by_trait,
                    }
                })
                .collect();
            (modules, items)
        };
        CrateGraph {
            schema_version: self.schema_version,
            ra_version: self.ra_version,
            root_crate: self.root_crate,
            crates: self.crates,
            modules,
            items,
            edges: self.edges,
        }
    }
}

/// Rebuilds a module's canonical path: the crate name for a root (empty-named)
/// module, otherwise the parent's path joined to this module's name. This assumes
/// every non-root module has a non-empty name (the extractor guarantees it, since
/// the path it builds and the name it stores come from the same source); a
/// nameless non-root module would reconstruct a stray `::` separator.
fn module_path(
    id: ModuleId,
    by_id: &HashMap<ModuleId, &CompactModule>,
    crate_name: &HashMap<CrateId, &str>,
    memo: &mut HashMap<ModuleId, String>,
) -> String {
    if let Some(path) = memo.get(&id) {
        return path.clone();
    }
    let m = by_id[&id];
    let path = match m.parent {
        None => crate_name
            .get(&m.crate_id)
            .copied()
            .unwrap_or("")
            .to_owned(),
        Some(parent) => format!(
            "{}::{}",
            module_path(parent, by_id, crate_name, memo),
            m.name
        ),
    };
    memo.insert(id, path.clone());
    path
}

/// Rebuilds a module's depth: 0 for a root, otherwise the parent's depth plus 1.
fn module_depth(
    id: ModuleId,
    by_id: &HashMap<ModuleId, &CompactModule>,
    memo: &mut HashMap<ModuleId, u32>,
) -> u32 {
    if let Some(depth) = memo.get(&id) {
        return *depth;
    }
    let depth = match by_id[&id].parent {
        None => 0,
        Some(parent) => module_depth(parent, by_id, memo) + 1,
    };
    memo.insert(id, depth);
    depth
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{CrateId, ItemId, ModuleId, RefKind, SCHEMA_VERSION};

    /// A small graph exercising every reconstruction case: a multi-depth tree, a
    /// type container (`SameAsModule`), a normal `Leaf`, a synthetic `{anon#0}`
    /// leaf, a `Full` builtin path, a `Visibility::Module(path)`, and edges.
    fn graph() -> CrateGraph {
        let modules = vec![
            Module {
                id: ModuleId(0),
                crate_id: CrateId(0),
                parent: None,
                name: String::new(),
                canonical_path: "k".to_owned(),
                depth: 0,
                visibility: Visibility::Public,
                kind: ModuleKind::Mod,
            },
            Module {
                id: ModuleId(1),
                crate_id: CrateId(0),
                parent: Some(ModuleId(0)),
                name: "a".to_owned(),
                canonical_path: "k::a".to_owned(),
                depth: 1,
                visibility: Visibility::Public,
                kind: ModuleKind::Mod,
            },
            // Type container under k::a: its path equals the type's path.
            Module {
                id: ModuleId(2),
                crate_id: CrateId(0),
                parent: Some(ModuleId(1)),
                name: "S".to_owned(),
                canonical_path: "k::a::S".to_owned(),
                depth: 2,
                visibility: Visibility::Module("k::a".to_owned()),
                kind: ModuleKind::Type,
            },
        ];
        let items = vec![
            // Type-container stub: path equals the module path -> SameAsModule.
            Item {
                id: ItemId(0),
                canonical_path: "k::a::S".to_owned(),
                kind: ItemKind::Struct,
                visibility: Visibility::Public,
                owning_module: ModuleId(2),
                crate_id: CrateId(0),
                has_canonical_path: true,
                reachable_pub_api: true,
                visibility_fixed_by_trait: false,
            },
            // Normal method under the type container -> Leaf.
            Item {
                id: ItemId(1),
                canonical_path: "k::a::S::run".to_owned(),
                kind: ItemKind::AssocFn,
                visibility: Visibility::Module("k::a".to_owned()),
                owning_module: ModuleId(2),
                crate_id: CrateId(0),
                has_canonical_path: true,
                reachable_pub_api: false,
                visibility_fixed_by_trait: false,
            },
            // Synthetic path under k::a -> Leaf("{anon#0}").
            Item {
                id: ItemId(2),
                canonical_path: "k::a::{anon#0}".to_owned(),
                kind: ItemKind::Const,
                visibility: Visibility::Private,
                owning_module: ModuleId(1),
                crate_id: CrateId(0),
                has_canonical_path: false,
                reachable_pub_api: false,
                visibility_fixed_by_trait: false,
            },
            // Builtin type owned by the root, path not a suffix of "k" -> Full.
            Item {
                id: ItemId(3),
                canonical_path: "u32".to_owned(),
                kind: ItemKind::BuiltinType,
                visibility: Visibility::Public,
                owning_module: ModuleId(0),
                crate_id: CrateId(0),
                has_canonical_path: true,
                reachable_pub_api: false,
                visibility_fixed_by_trait: false,
            },
        ];
        CrateGraph {
            schema_version: SCHEMA_VERSION,
            ra_version: "test".to_owned(),
            root_crate: CrateId(0),
            crates: vec![Crate {
                id: CrateId(0),
                name: "k".to_owned(),
                is_local: true,
                root_module: ModuleId(0),
            }],
            modules,
            items,
            edges: vec![
                Edge {
                    from: ItemId(1),
                    to: ItemId(0),
                    kind: RefKind::Body,
                    weight: 3,
                },
                Edge {
                    from: ItemId(1),
                    to: ItemId(3),
                    kind: RefKind::Signature,
                    weight: 1,
                },
            ],
        }
    }

    #[test]
    fn compact_roundtrip_is_lossless() {
        let g = graph();
        let back = decode_compact(&encode_compact(&g).unwrap()).unwrap();
        assert_eq!(g, back);
    }

    #[test]
    fn container_roundtrip_uncompressed() {
        let g = graph();
        let payload = encode_compact(&g).unwrap();
        let bytes = wrap_container(&payload, Codec::PostcardCompact, Compression::None);
        assert_eq!(&bytes[..4], &CONTAINER_MAGIC);
        assert_eq!(read_container(&bytes).unwrap(), g);
    }

    #[test]
    fn container_roundtrip_zstd() {
        let g = graph();
        let payload = encode_compact(&g).unwrap();
        let compressed = zstd::encode_all(payload.as_slice(), 19).unwrap();
        let bytes = wrap_container(&compressed, Codec::PostcardCompact, Compression::Zstd);
        assert_eq!(read_container(&bytes).unwrap(), g);
    }

    #[test]
    fn non_container_bytes_are_rejected() {
        // Verbose JSON (the old format) no longer reads through the container.
        let json = serde_json::to_vec(&graph()).unwrap();
        assert_ne!(json[..4], CONTAINER_MAGIC);
        assert!(matches!(
            read_container(&json),
            Err(ContainerError::NotContainer)
        ));
    }

    #[test]
    fn truncated_container_errors() {
        let bytes = [b'M', b'I', b'R', b'z', FORMAT_VERSION];
        assert!(matches!(
            read_container(&bytes),
            Err(ContainerError::Truncated)
        ));
    }
}
