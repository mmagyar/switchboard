#![forbid(unsafe_code)]

use sb::{Authorization, HandlerDefiner, route};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let route = route!(get, "/items/:itemId/:type/:slug?", "public", String)?;
    let define = HandlerDefiner::new(
        |_| async { Ok(()) },
        |(), _: &str, _| async { Ok(Authorization::Ok) },
    );
    let _bound = define.bind_typed(route, |params, _body, (), prefix| async move {
        Ok(format!(
            "{}/{}/{}/{}",
            params.itemId,
            params.r#type,
            params.slug.unwrap_or_default(),
            prefix.unwrap_or_default()
        ))
    });
    Ok(())
}
