//! Fixture exercising derive-macro-generated impls.
//!
//! The derive expands to `impl Tag for Tagged {}`, which only exists in the HIR
//! when the proc-macro server is enabled. Both `Tag` and `Tagged` are local, so
//! the generated impl is an internal `Impl` edge `Tagged -> Tag`.

use procderive_macro::Tag;

/// A local trait implemented through a derive macro.
pub trait Tag {}

/// The derive generates `impl Tag for Tagged {}`.
#[derive(Tag)]
pub struct Tagged;
