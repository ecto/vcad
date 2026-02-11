//! Proc-macro implementation for wasmosis.
//!
//! Provides the `#[module("name")]` attribute macro for explicit module assignment.
//! Most functions don't need this - wasmosis infers modules from feature gates
//! and dependencies automatically.

use proc_macro::TokenStream;
use quote::quote;
use syn::{parse::Parse, parse::ParseStream, parse_macro_input, ItemFn, LitStr};

/// The custom section name used by wasmosis.
const SECTION_NAME: &str = "wasmosis_module";

/// Optional module name argument.
struct ModuleArgs {
    module_name: Option<String>,
}

impl Parse for ModuleArgs {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        if input.is_empty() {
            Ok(ModuleArgs { module_name: None })
        } else {
            let name: LitStr = input.parse()?;
            Ok(ModuleArgs {
                module_name: Some(name.value()),
            })
        }
    }
}

/// Explicitly assign a function to a WASM module.
///
/// Most functions don't need this - wasmosis infers modules automatically from:
/// - Feature gates: `#[cfg(feature = "physics")]` → `physics` module
/// - Dependencies: `vcad_kernel_physics::` in body → `physics` module
///
/// Use this when you need to override automatic inference.
///
/// # Example
///
/// ```rust,ignore
/// use wasmosis::module;
///
/// #[module("advanced")]
/// #[wasm_bindgen]
/// pub fn experimental_feature() -> Result<(), JsError> {
///     // Force this into "advanced" module
/// }
/// ```
///
/// # How It Works
///
/// Embeds JSON metadata in a custom WASM section:
/// ```json
/// {"module": "advanced", "function": "experimental_feature"}
/// ```
///
/// The wasmosis CLI reads these sections to split the binary.
#[proc_macro_attribute]
pub fn module(attr: TokenStream, item: TokenStream) -> TokenStream {
    let args = parse_macro_input!(attr as ModuleArgs);
    let input_fn = parse_macro_input!(item as ItemFn);

    let fn_name = &input_fn.sig.ident;
    let fn_name_str = fn_name.to_string();

    let metadata = match &args.module_name {
        Some(name) => format!(r#"{{"module":"{}","function":"{}"}}"#, name, fn_name_str),
        None => format!(r#"{{"module":null,"function":"{}"}}"#, fn_name_str),
    };
    let metadata_len = metadata.len();
    let metadata_bytes = metadata.as_bytes();

    let static_name = syn::Ident::new(
        &format!("__WASMOSIS_META_{}", fn_name_str.to_uppercase()),
        fn_name.span(),
    );

    let expanded = quote! {
        #[doc(hidden)]
        #[used]
        #[cfg_attr(target_arch = "wasm32", link_section = #SECTION_NAME)]
        static #static_name: [u8; #metadata_len] = [#(#metadata_bytes),*];

        #input_fn
    };

    TokenStream::from(expanded)
}

#[cfg(test)]
mod tests {
    // Proc-macro tests via trybuild or separate test crate
}
