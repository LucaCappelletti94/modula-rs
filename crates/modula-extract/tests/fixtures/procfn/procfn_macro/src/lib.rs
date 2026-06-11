//! Function-like and attribute proc macros, with no external dependencies.

use proc_macro::{TokenStream, TokenTree};

/// A function-like proc macro that expands to a call to the local `local_target`.
#[proc_macro]
pub fn call_local(_input: TokenStream) -> TokenStream {
    "local_target()".parse().expect("expansion parses")
}

/// An attribute proc macro that re-emits the annotated function with a call to
/// `local_target()` injected into its body (the original body is discarded). The
/// function name is parsed from the token stream so no `syn`/`quote` is needed.
#[proc_macro_attribute]
pub fn wrap(_attr: TokenStream, item: TokenStream) -> TokenStream {
    let mut prev_was_fn = false;
    let mut name = None;
    for token in item {
        if let TokenTree::Ident(ident) = token {
            let text = ident.to_string();
            if prev_was_fn {
                name = Some(text);
                break;
            }
            if text == "fn" {
                prev_was_fn = true;
            }
        }
    }
    let name = name.expect("wrap requires a function");
    format!("pub fn {name}() -> u32 {{ local_target() }}")
        .parse()
        .expect("expansion parses")
}
