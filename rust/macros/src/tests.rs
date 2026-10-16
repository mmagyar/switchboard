use super::*;
use quote::ToTokens;
use syn::{ExprBlock, Item, Stmt};

fn parse(source: &str) -> Route {
    syn::parse_str(source).unwrap_or_else(|error| panic!("{source}: {error}"))
}

fn diagnostic(source: &str) -> String {
    match syn::parse_str::<Route>(source) {
        Ok(_) => panic!("expected rejection: {source}"),
        Err(error) => error.to_string(),
    }
}

#[test]
fn infers_id_conventions_strings_and_optional_fields() {
    let route = parse(r#"get, "/items/:itemId/:user_id/:slug/:pageId?/:label?", "user", String"#);
    let fields: Vec<_> = route
        .params
        .iter()
        .map(|param| (param.name.as_str(), param.numeric, param.optional))
        .collect();
    assert_eq!(
        fields,
        vec![
            ("itemId", true, false),
            ("user_id", true, false),
            ("slug", false, false),
            ("pageId", true, true),
            ("label", false, true),
        ]
    );
    let expanded: ExprBlock = syn::parse2(route.expand(&quote!(::renamed))).unwrap();
    let Some(Stmt::Item(Item::Struct(params))) = expanded.block.stmts.first() else {
        panic!("expected local params struct");
    };
    let actual: Vec<_> = params
        .fields
        .iter()
        .map(|field| {
            (
                field.ident.as_ref().unwrap().to_string(),
                field.ty.to_token_stream().to_string(),
            )
        })
        .collect();
    assert_eq!(
        actual,
        vec![
            ("itemId".into(), ":: core :: primitive :: f64".into()),
            ("user_id".into(), ":: core :: primitive :: f64".into()),
            ("slug".into(), ":: std :: string :: String".into()),
            (
                "pageId".into(),
                ":: core :: option :: Option < :: core :: primitive :: f64 >".into()
            ),
            (
                "label".into(),
                ":: core :: option :: Option < :: std :: string :: String >".into()
            ),
        ]
    );
}

#[test]
fn accepts_all_methods_and_body_contracts() {
    for (name, variant) in [
        ("get", "Get"),
        ("delete", "Delete"),
        ("head", "Head"),
        ("options", "Options"),
        ("post", "Post"),
        ("put", "Put"),
        ("patch", "Patch"),
    ] {
        let body_method = matches!(name, "post" | "put" | "patch");
        let types = if body_method {
            "Vec<String> => Option<String>"
        } else {
            "Option<String>"
        };
        let route = parse(&format!(r#"{name}, "/", permission(), {types},"#));
        assert_eq!(
            route.body.to_token_stream().to_string(),
            if body_method { "Vec < String >" } else { "()" }
        );
        assert_eq!(
            route.output.to_token_stream().to_string(),
            "Option < String >"
        );
        assert!(route.params.is_empty());
        let expanded = route.expand(&quote!(::switchboard)).to_string();
        assert!(
            expanded.contains(&format!("Method :: {variant}")),
            "{expanded}"
        );
    }
}

#[test]
fn keeps_canonical_path_permission_and_prefixes() {
    let route = parse(
        r#"post, "items/:itemId/", permissions::required::<Role>(1), Body => Output, prefixes = ["hu", "de/at", "",],"#,
    );
    assert_eq!(route.path.value(), "items/:itemId/");
    assert_eq!(
        route.permission.to_token_stream().to_string(),
        "permissions :: required :: < Role > (1)"
    );
    assert_eq!(
        route.prefixes.iter().map(LitStr::value).collect::<Vec<_>>(),
        ["hu", "de/at", ""]
    );
    let expanded = route.expand(&quote!(::switchboard)).to_string();
    assert!(expanded.contains("\"items/:itemId/\""));
    assert!(expanded.contains("String :: from (\"hu\")"));
    assert!(
        parse(r#"get, "/", (), (), prefixes = []"#)
            .prefixes
            .is_empty()
    );
}

#[test]
fn uses_raw_keyword_fields_and_preserves_json_names() {
    let route = parse(r#"get, "/:type/:match/:async/:gen/:café", (), ()"#);
    assert_eq!(
        route
            .params
            .iter()
            .map(|param| param.ident.to_string())
            .collect::<Vec<_>>(),
        ["r#type", "r#match", "r#async", "r#gen", "café"]
    );
    let expanded = route.expand(&quote!(::renamed)).to_string();
    assert!(expanded.contains("serde (rename = \"type\")"));
    assert!(expanded.contains("pub r#type : :: std :: string :: String"));
    assert!(expanded.contains("allow (non_snake_case)"));
}

#[test]
fn derives_use_only_resolved_private_reexports() {
    let route = parse(r#"get, "/:itemId", (), String"#);
    for root in [quote!(::renamed_switchboard), quote!(crate)] {
        let expanded = route.expand(&root).to_string();
        let root = root.to_string();
        assert!(expanded.contains(&format!("{root} :: __private :: serde :: Serialize")));
        assert!(expanded.contains(&format!("{root} :: __private :: serde :: Deserialize")));
        assert!(expanded.contains(&format!("{root} :: __private :: schemars :: JsonSchema")));
        assert!(expanded.contains(&format!("serde (crate = \"{root} :: __private :: serde\")")));
        assert!(expanded.contains(&format!(
            "schemars (crate = \"{root} :: __private :: schemars\")"
        )));
        assert!(expanded.contains(&format!(
            "{root} :: TypedRoute :: < __SwitchboardParams , () , String , _ > :: new"
        )));
    }
}

#[test]
fn optional_order_matches_reference_including_trailing_slash() {
    for path in ["/:id?/:label", "/:id?/fixed", "/:id?//", "/static?/fixed"] {
        assert_eq!(
            diagnostic(&format!("get, {path:?}, (), ()")),
            "cannot have a mandatory parameter or static segment after an optional segment"
        );
    }
    for path in [
        "/:id?",
        "/:id?/",
        "/:id?/:name?",
        "/:id?/:name?/",
        "items/:name",
        "",
        "/",
    ] {
        parse(&format!("get, {path:?}, (), ()"));
    }
}

#[test]
fn rejects_unrepresentable_duplicate_and_empty_names_with_escape_hatch() {
    for path in [
        "/:",
        "/:?",
        "/:foo-bar",
        "/:123",
        "/:_",
        "/:self",
        "/:Self",
        "/:super",
        "/:crate",
        "/:r#type",
        "/:a.b",
        "/:x:y",
        "/:a??",
    ] {
        let message = diagnostic(&format!("get, {path:?}, (), ()"));
        assert!(
            message.contains("cannot be represented as an inferred Rust field"),
            "{message}"
        );
        assert!(
            message.contains(
                "raw RouteDef or an explicit TypedRoute parameter struct with serde field renames"
            ),
            "{message}"
        );
    }
    for path in ["/:name/:name", "/:name/:name?"] {
        assert!(
            diagnostic(&format!("get, {path:?}, (), ()"))
                .contains("duplicate path parameter \"name\"")
        );
    }
}

#[test]
fn rejects_malformed_macro_inputs_and_prefixes() {
    for (source, expected) in [
        (
            r#"trace, "/", (), ()"#,
            "expected get, post, put, patch, delete, head, or options",
        ),
        (
            r#"get, PATH, (), ()"#,
            "route! requires a string literal path for inference",
        ),
        (
            r#"get, concat!("/", "items"), (), ()"#,
            "route! requires a string literal path for inference",
        ),
        (
            r#"post, "/", (), Output"#,
            "body methods require BodyType => OutputType",
        ),
        (
            r#"get, "/", (), Body => Output"#,
            "this method has no body; provide only OutputType",
        ),
        (r#"get, "/", (), (), prefix = []"#, "expected prefixes ="),
        (
            r#"get, "/", (), (), prefixes = ["/hu"]"#,
            "prefix must not start with a slash",
        ),
        (
            r#"get, "/", (), (), prefixes = ["hu/:locale"]"#,
            "prefix must not contain path params",
        ),
        (
            r#"get, "/", (), (), prefixes = [], extra"#,
            "unexpected tokens after route definition",
        ),
    ] {
        assert!(
            diagnostic(source).starts_with(expected),
            "{source}: {}",
            diagnostic(source)
        );
    }
}
