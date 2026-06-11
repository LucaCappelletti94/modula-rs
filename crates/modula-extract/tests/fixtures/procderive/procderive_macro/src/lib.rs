//! A minimal derive macro with no external dependencies.
//!
//! `#[derive(Tag)]` emits `impl Tag for <Name> {}` for the annotated type. The
//! type name is parsed straight from the token stream (no `syn`/`quote`) so the
//! fixture pulls in nothing beyond `proc_macro` itself.

use proc_macro::{TokenStream, TokenTree};

/// Emits an empty `impl Tag for <Name> {}` for the derived type.
#[proc_macro_derive(Tag)]
pub fn derive_tag(input: TokenStream) -> TokenStream {
    // Find the identifier immediately following `struct`/`enum`/`union`.
    let mut prev_was_keyword = false;
    let mut name = None;
    for token in input {
        if let TokenTree::Ident(ident) = token {
            let text = ident.to_string();
            if prev_was_keyword {
                name = Some(text);
                break;
            }
            if text == "struct" || text == "enum" || text == "union" {
                prev_was_keyword = true;
            }
        }
    }
    let name = name.expect("derive(Tag) requires a named type");
    format!("impl Tag for {name} {{}}")
        .parse()
        .expect("generated impl parses")
}
