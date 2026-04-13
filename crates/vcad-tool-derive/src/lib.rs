use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use quote::quote;
use syn::{parse_macro_input, DeriveInput, Expr, Fields, Lit, Meta};

#[proc_macro_derive(ToolSchema, attributes(tool))]
pub fn derive_tool_schema(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    match impl_tool_schema(&input) {
        Ok(tokens) => tokens.into(),
        Err(err) => err.to_compile_error().into(),
    }
}

struct ToolAttrs {
    hidden: bool,
    category: Option<String>,
    ai_hint: Option<String>,
}

fn parse_tool_attrs(attrs: &[syn::Attribute]) -> syn::Result<ToolAttrs> {
    let mut hidden = false;
    let mut category = None;
    let mut ai_hint = None;

    for attr in attrs {
        if !attr.path().is_ident("tool") {
            continue;
        }
        attr.parse_nested_meta(|meta| {
            if meta.path.is_ident("hidden") {
                hidden = true;
                Ok(())
            } else if meta.path.is_ident("category") {
                let value = meta.value()?;
                let lit: Lit = value.parse()?;
                if let Lit::Str(s) = lit {
                    category = Some(s.value());
                }
                Ok(())
            } else if meta.path.is_ident("ai_hint") {
                let value = meta.value()?;
                let lit: Lit = value.parse()?;
                if let Lit::Str(s) = lit {
                    ai_hint = Some(s.value());
                }
                Ok(())
            } else {
                Err(meta.error("unknown tool attribute"))
            }
        })?;
    }

    Ok(ToolAttrs {
        hidden,
        category,
        ai_hint,
    })
}

fn extract_doc_comment(attrs: &[syn::Attribute]) -> String {
    let mut lines = Vec::new();
    for attr in attrs {
        if attr.path().is_ident("doc") {
            if let Meta::NameValue(nv) = &attr.meta {
                if let Expr::Lit(expr_lit) = &nv.value {
                    if let Lit::Str(s) = &expr_lit.lit {
                        lines.push(s.value().trim().to_string());
                    }
                }
            }
        }
    }
    lines.join(" ").trim().to_string()
}

fn to_snake_case(s: &str) -> String {
    let mut result = String::new();
    let chars: Vec<char> = s.chars().collect();
    for (i, &ch) in chars.iter().enumerate() {
        if i > 0 {
            let prev = chars[i - 1];
            // Insert underscore at boundaries: lower→UPPER, letter→digit, digit→letter
            let boundary = (ch.is_uppercase() && (prev.is_lowercase() || prev.is_numeric()))
                || (ch.is_numeric() && prev.is_alphabetic());
            if boundary {
                result.push('_');
            }
        }
        result.push(ch.to_lowercase().next().unwrap());
    }
    result
}

fn impl_tool_schema(input: &DeriveInput) -> syn::Result<TokenStream2> {
    let name = &input.ident;

    let data = match &input.data {
        syn::Data::Enum(data) => data,
        _ => {
            return Err(syn::Error::new_spanned(
                input,
                "ToolSchema only supports enums",
            ))
        }
    };

    let enum_attrs = parse_tool_attrs(&input.attrs)?;
    let default_category = enum_attrs
        .category
        .unwrap_or_else(|| "uncategorized".to_string());

    let mut variant_entries = Vec::new();

    for variant in &data.variants {
        let attrs = parse_tool_attrs(&variant.attrs)?;
        if attrs.hidden {
            continue;
        }

        let variant_name = to_snake_case(&variant.ident.to_string());
        let description = extract_doc_comment(&variant.attrs);
        let category = attrs.category.unwrap_or_else(|| default_category.clone());
        let ai_hint_expr = match &attrs.ai_hint {
            Some(h) => quote! { Some(#h.to_string()) },
            None => quote! { None },
        };

        let schema_expr = build_fields_schema(&variant.fields)?;

        variant_entries.push(quote! {
            crate::ToolSchemaEntry {
                name: #variant_name.to_string(),
                description: #description.to_string(),
                category: #category.to_string(),
                ai_hint: #ai_hint_expr,
                input_schema: #schema_expr,
            }
        });
    }

    Ok(quote! {
        impl #name {
            /// Returns tool schema entries for all non-hidden variants.
            pub fn tool_schemas() -> Vec<crate::ToolSchemaEntry> {
                vec![#(#variant_entries),*]
            }
        }
    })
}

fn build_fields_schema(fields: &Fields) -> syn::Result<TokenStream2> {
    match fields {
        Fields::Named(named) => {
            let mut prop_entries = Vec::new();
            let mut required_entries = Vec::new();

            for field in &named.named {
                let field_name = field.ident.as_ref().unwrap().to_string();
                let field_desc = extract_doc_comment(&field.attrs);
                let (schema_expr, is_optional) = type_to_schema(&field.ty)?;

                prop_entries.push(quote! {
                    {
                        let mut prop = #schema_expr;
                        if !#field_desc.is_empty() {
                            prop.as_object_mut().unwrap()
                                .insert("description".to_string(),
                                        serde_json::Value::String(#field_desc.to_string()));
                        }
                        (#field_name.to_string(), prop)
                    }
                });

                if !is_optional {
                    required_entries.push(quote! { #field_name.to_string() });
                }
            }

            Ok(quote! {
                {
                    let props: Vec<(String, serde_json::Value)> = vec![#(#prop_entries),*];
                    let required: Vec<String> = vec![#(#required_entries),*];
                    let mut schema = serde_json::json!({
                        "type": "object",
                        "properties": {},
                    });
                    let obj = schema["properties"].as_object_mut().unwrap();
                    for (k, v) in props {
                        obj.insert(k, v);
                    }
                    if !required.is_empty() {
                        schema.as_object_mut().unwrap()
                            .insert("required".to_string(),
                                    serde_json::Value::Array(
                                        required.into_iter()
                                            .map(serde_json::Value::String)
                                            .collect()));
                    }
                    schema
                }
            })
        }
        Fields::Unit => Ok(quote! { serde_json::json!({ "type": "object", "properties": {} }) }),
        Fields::Unnamed(_) => Err(syn::Error::new_spanned(
            fields,
            "ToolSchema does not support tuple variants",
        )),
    }
}

fn type_to_schema(ty: &syn::Type) -> syn::Result<(TokenStream2, bool)> {
    let ty_str = quote!(#ty).to_string().replace(' ', "");

    // Option<T>
    if ty_str.starts_with("Option<") {
        let inner = extract_generic_inner(ty, "Option")?;
        let (inner_schema, _) = type_to_schema(inner)?;
        return Ok((inner_schema, true));
    }

    // Vec<T> — but not Vec2 or Vec3 which are structs
    if ty_str.starts_with("Vec<") && !ty_str.starts_with("Vec2") && !ty_str.starts_with("Vec3") {
        let inner = extract_generic_inner(ty, "Vec")?;
        let (inner_schema, _) = type_to_schema(inner)?;
        return Ok((
            quote! {
                serde_json::json!({ "type": "array", "items": #inner_schema })
            },
            false,
        ));
    }

    // Box<T>
    if ty_str.starts_with("Box<") {
        let inner = extract_generic_inner(ty, "Box")?;
        return type_to_schema(inner);
    }

    match ty_str.as_str() {
        "f64" => Ok((quote! { serde_json::json!({ "type": "number" }) }, false)),
        "u32" | "u64" | "i32" | "i64" | "usize" => {
            Ok((quote! { serde_json::json!({ "type": "integer" }) }, false))
        }
        "bool" => Ok((quote! { serde_json::json!({ "type": "boolean" }) }, false)),
        "String" => Ok((quote! { serde_json::json!({ "type": "string" }) }, false)),
        "NodeId" => Ok((
            quote! { serde_json::json!({ "type": "string", "description": "Node ID reference" }) },
            false,
        )),
        "Vec3" => Ok((
            quote! {
                serde_json::json!({
                    "type": "object",
                    "properties": {
                        "x": { "type": "number" },
                        "y": { "type": "number" },
                        "z": { "type": "number" }
                    },
                    "required": ["x", "y", "z"]
                })
            },
            false,
        )),
        "Vec2" => Ok((
            quote! {
                serde_json::json!({
                    "type": "object",
                    "properties": {
                        "x": { "type": "number" },
                        "y": { "type": "number" }
                    },
                    "required": ["x", "y"]
                })
            },
            false,
        )),
        _ => {
            // Unknown types — generic object fallback
            Ok((quote! { serde_json::json!({ "type": "object" }) }, false))
        }
    }
}

fn extract_generic_inner<'a>(ty: &'a syn::Type, wrapper: &str) -> syn::Result<&'a syn::Type> {
    if let syn::Type::Path(type_path) = ty {
        if let Some(segment) = type_path.path.segments.last() {
            if segment.ident == wrapper {
                if let syn::PathArguments::AngleBracketed(args) = &segment.arguments {
                    if let Some(syn::GenericArgument::Type(inner)) = args.args.first() {
                        return Ok(inner);
                    }
                }
            }
        }
    }
    Err(syn::Error::new_spanned(
        ty,
        format!("expected {}<T>", wrapper),
    ))
}
