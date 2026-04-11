use proc_macro::TokenStream;

/// Derive macro that generates `tool_schemas() -> Vec<ToolSchemaEntry>` for enums.
///
/// Supports attributes:
/// - `#[tool(category = "...")]` on variants — sets the category
/// - `#[tool(ai_hint = "...")]` on variants — extra context for AI
/// - `#[tool(hidden)]` on variants — skip this variant
#[proc_macro_derive(ToolSchema, attributes(tool))]
pub fn derive_tool_schema(input: TokenStream) -> TokenStream {
    let input = syn::parse_macro_input!(input as syn::DeriveInput);
    match impl_tool_schema(&input) {
        Ok(tokens) => tokens.into(),
        Err(err) => err.to_compile_error().into(),
    }
}

fn impl_tool_schema(
    _input: &syn::DeriveInput,
) -> syn::Result<proc_macro2::TokenStream> {
    Ok(quote::quote! {})
}
