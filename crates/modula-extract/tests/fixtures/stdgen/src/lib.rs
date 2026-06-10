//! Fixture exercising std-generic wrappers around local types.
//!
//! `Vec`, `Option`, `Box`, and `Result` are defined in the standard library, so
//! they only resolve when the extractor enables the sysroot. Once they resolve,
//! the type walk descends into the local type argument and emits a `Signature`
//! edge to it. Without the sysroot the whole wrapper type is unknown and the
//! local argument is invisible, so no edge is produced.

/// A local type wrapped by std generics.
pub struct Local;

/// A second local type, used as the error arm of a `Result`.
pub struct Failure;

/// A struct field `Vec<Local>` (signature edge `Holder -> Local`).
pub struct Holder {
    pub items: Vec<Local>,
}

/// A return type `Option<Local>` (signature edge `first -> Local`).
pub fn first() -> Option<Local> {
    None
}

/// A boxed return `Box<Local>` (signature edge `boxed -> Local`).
pub fn boxed() -> Box<Local> {
    Box::new(Local)
}

/// A `Result<Local, Failure>` return (signature edges to both local arms).
pub fn fallible() -> Result<Local, Failure> {
    Ok(Local)
}
