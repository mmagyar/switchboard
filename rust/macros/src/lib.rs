#![forbid(unsafe_code)]

use proc_macro::TokenStream;
use proc_macro_crate::{FoundCrate, crate_name};
use proc_macro2::{Span, TokenStream as TokenStream2};
use quote::quote;
use std::collections::HashSet;
use syn::{
    Expr, Ident, LitStr, Result, Token, Type, bracketed,
    parse::{Parse, ParseStream},
    parse_macro_input,
};

/// Defines a typed route with inferred path parameters. See the crate README for syntax.
#[proc_macro]
pub fn route(input: TokenStream) -> TokenStream {
    let route = parse_macro_input!(input as Route);
    let result = dependency_path().map(|root| route.expand(&root));
    result.unwrap_or_else(syn::Error::into_compile_error).into()
}

struct Route {
    method: Ident,
    path: LitStr,
    permission: Expr,
    body: Type,
    output: Type,
    prefixes: Vec<LitStr>,
    params: Vec<Param>,
}

struct Param {
    name: String,
    ident: Ident,
    numeric: bool,
    optional: bool,
}

impl Parse for Route {
    fn parse(input: ParseStream<'_>) -> Result<Self> {
        let method: Ident = input.parse()?;
        let has_body = match method.to_string().as_str() {
            "post" | "put" | "patch" => true,
            "get" | "delete" | "head" | "options" => false,
            _ => {
                return Err(syn::Error::new(
                    method.span(),
                    "expected get, post, put, patch, delete, head, or options",
                ));
            }
        };
        input.parse::<Token![,]>()?;
        let path: LitStr = input.parse().map_err(|_| {
            input.error("route! requires a string literal path for inference; use RouteDef or an explicit TypedRoute parameter struct for dynamic paths")
        })?;
        let params = parse_params(&path)?;
        input.parse::<Token![,]>()?;
        let permission = input.parse()?;
        input.parse::<Token![,]>()?;
        let first: Type = input.parse()?;
        let (body, output) = if has_body {
            if !input.peek(Token![=>]) {
                return Err(syn::Error::new(
                    input.span(),
                    "body methods require BodyType => OutputType",
                ));
            }
            input.parse::<Token![=>]>()?;
            (first, input.parse()?)
        } else {
            if input.peek(Token![=>]) {
                return Err(input.error("this method has no body; provide only OutputType"));
            }
            (syn::parse_quote!(()), first)
        };
        let mut prefixes = Vec::new();
        if !input.is_empty() {
            input.parse::<Token![,]>()?;
            if !input.is_empty() {
                let key: Ident = input.parse()?;
                if key != "prefixes" {
                    return Err(syn::Error::new(
                        key.span(),
                        "expected prefixes = [\"hu\", \"de\"]",
                    ));
                }
                input.parse::<Token![=]>()?;
                let content;
                bracketed!(content in input);
                prefixes = content
                    .parse_terminated(|input| input.parse::<LitStr>(), Token![,])?
                    .into_iter()
                    .collect();
                if !input.is_empty() {
                    input.parse::<Token![,]>()?;
                }
            }
        }
        if !input.is_empty() {
            return Err(input.error("unexpected tokens after route definition"));
        }
        for prefix in &prefixes {
            if prefix.value().starts_with('/') {
                return Err(syn::Error::new(
                    prefix.span(),
                    "prefix must not start with a slash",
                ));
            }
            if prefix.value().contains(':') {
                return Err(syn::Error::new(
                    prefix.span(),
                    "prefix must not contain path params",
                ));
            }
        }
        Ok(Self {
            method,
            path,
            permission,
            body,
            output,
            prefixes,
            params,
        })
    }
}

fn parse_params(path: &LitStr) -> Result<Vec<Param>> {
    let path_value = path.value();
    let segments = path_value
        .strip_suffix('/')
        .unwrap_or(&path_value)
        .split('/');
    let mut optional_seen = false;
    let mut names = HashSet::new();
    let mut params = Vec::new();
    for segment in segments {
        let optional = segment.ends_with('?');
        if optional_seen && !optional {
            return Err(syn::Error::new(
                path.span(),
                "cannot have a mandatory parameter or static segment after an optional segment",
            ));
        }
        optional_seen |= optional;
        let Some(name) = segment.strip_prefix(':') else {
            continue;
        };
        let name = name.strip_suffix('?').unwrap_or(name);
        let invalid_name = || {
            syn::Error::new(
                path.span(),
                format!(
                    "path parameter {name:?} cannot be represented as an inferred Rust field; use raw RouteDef or an explicit TypedRoute parameter struct with serde field renames",
                ),
            )
        };
        if name.is_empty()
            || name == "_"
            || name.starts_with("r#")
            || matches!(name, "self" | "Self" | "super" | "crate")
        {
            return Err(invalid_name());
        }
        // syn accepts `gen` as an identifier, but Rust 2024 reserves it.
        let ident = if name == "gen" {
            syn::parse_str::<Ident>("r#gen")
        } else {
            syn::parse_str::<Ident>(name).or_else(|_| syn::parse_str::<Ident>(&format!("r#{name}")))
        }
        .map_err(|_| invalid_name())?;
        if !names.insert(name.to_owned()) {
            return Err(syn::Error::new(
                path.span(),
                format!(
                    "duplicate path parameter {name:?}; use distinct names, raw RouteDef, or an explicit TypedRoute parameter struct",
                ),
            ));
        }
        params.push(Param {
            name: name.to_owned(),
            ident,
            numeric: name.ends_with("Id") || name.ends_with("_id"),
            optional,
        });
    }
    Ok(params)
}

fn dependency_path() -> Result<TokenStream2> {
    match crate_name("switchboard") {
        Ok(FoundCrate::Itself) => Ok(quote!(crate)),
        Ok(FoundCrate::Name(name)) => {
            let ident = syn::parse_str::<Ident>(&format!("r#{name}")).map_err(|_| {
                syn::Error::new(Span::call_site(), "invalid switchboard dependency alias")
            })?;
            Ok(quote!(::#ident))
        }
        Err(error) => Err(syn::Error::new(
            Span::call_site(),
            format!("route! requires the switchboard dependency: {error}"),
        )),
    }
}

impl Route {
    fn expand(&self, root: &TokenStream2) -> TokenStream2 {
        let Self {
            method,
            path,
            permission,
            body,
            output,
            prefixes,
            params,
        } = self;
        let method = match method.to_string().as_str() {
            "get" => quote!(Get),
            "post" => quote!(Post),
            "put" => quote!(Put),
            "patch" => quote!(Patch),
            "delete" => quote!(Delete),
            "head" => quote!(Head),
            "options" => quote!(Options),
            _ => unreachable!("method validated during parsing"),
        };
        let params_ident = Ident::new("__SwitchboardParams", Span::mixed_site());
        let serde_path = LitStr::new(
            &quote!(#root::__private::serde).to_string(),
            Span::call_site(),
        );
        let schemars_path = LitStr::new(
            &quote!(#root::__private::schemars).to_string(),
            Span::call_site(),
        );
        let fields = params.iter().map(
            |Param {
                 name,
                 ident,
                 numeric,
                 optional,
             }| {
                let ty = if *numeric {
                    quote!(::core::primitive::f64)
                } else {
                    quote!(::std::string::String)
                };
                let ty = if *optional {
                    quote!(::core::option::Option<#ty>)
                } else {
                    ty
                };
                quote! {
                    #[serde(rename = #name)]
                    pub #ident: #ty
                }
            },
        );
        quote! {{
            #[derive(
                #root::__private::serde::Serialize,
                #root::__private::serde::Deserialize,
                #root::__private::schemars::JsonSchema
            )]
            #[serde(crate = #serde_path)]
            #[schemars(crate = #schemars_path)]
            #[allow(non_snake_case)]
            struct #params_ident { #(#fields,)* }
            #root::TypedRoute::<#params_ident, #body, #output, _>::new(
                #root::Method::#method,
                #path,
                #permission,
                ::std::vec![#(::std::string::String::from(#prefixes)),*],
            )
        }}
    }
}

#[cfg(test)]
mod tests;
