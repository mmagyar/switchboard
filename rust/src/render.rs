use crate::{Response, response};

pub use maud::{Markup, PreEscaped, html};

/// Maud escapes interpolated text. PreEscaped is an explicit trust boundary.
pub fn html_response(markup: Markup) -> Response {
    response(200, "text/html; charset=utf-8", markup.into_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use http_body_util::BodyExt;

    #[tokio::test]
    async fn markup_is_rendered_without_hydration_and_escapes_untrusted_text() {
        let title = "<script>alert(1)</script>";
        let response = html_response(html! { main { h1 { (title) } } });
        assert_eq!(
            response.headers()["content-type"],
            "text/html; charset=utf-8"
        );
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(
            &bytes[..],
            b"<main><h1>&lt;script&gt;alert(1)&lt;/script&gt;</h1></main>"
        );
    }
}
