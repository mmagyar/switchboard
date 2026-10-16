use crate::{ItemParams, item_route, refinement_route};
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
};
use switchboard_browser::{
    BrowserClient, CallSettings, ClientOptions, EventListener, HotReloadConnection, RouteCallError,
};
use wasm_bindgen::prelude::*;
use web_sys::Element;

struct Connections {
    click: EventListener,
    hot: HotReloadConnection,
}

#[wasm_bindgen]
pub struct Page {
    controls: Vec<EventListener>,
    connections: Rc<RefCell<Option<Connections>>>,
}

impl Drop for Page {
    fn drop(&mut self) {
        self.controls.clear();
        self.connections.borrow_mut().take();
    }
}

fn element(id: &str) -> Result<Element, JsValue> {
    web_sys::window()
        .and_then(|window| window.document())
        .and_then(|document| document.get_element_by_id(id))
        .ok_or_else(|| JsValue::from_str(&format!("Missing #{id}")))
}

fn js_error(error: impl std::fmt::Display) -> JsValue {
    JsValue::from_str(&error.to_string())
}

#[wasm_bindgen]
pub async fn start() -> Result<Page, JsValue> {
    let client = BrowserClient::new("", ClientOptions::default());
    let params = ItemParams {
        item_id: 42,
        query: "a b&c".into(),
    };
    let settings = CallSettings::default();
    let route = item_route().map_err(js_error)?;
    let item = client
        .call_route(&route, &params, None, &settings)
        .await
        .map_err(js_error)?
        .ok_or_else(|| JsValue::from_str("Missing item"))?;
    element("fetch")?.set_text_content(Some(&format!("{}; query: {}", item.title, item.query)));

    let bad = refinement_route().map_err(js_error)?;
    match client.call_route(&bad, &params, None, &settings).await {
        Err(RouteCallError::Validation(error)) => {
            element("validation")?.set_text_content(Some(&format!("Schema rejected: {error}")))
        }
        result => {
            return Err(js_error(format!(
                "Expected schema rejection, got {}",
                result.is_ok()
            )));
        }
    }
    let value = client
        .call_route_unchecked(&bad, &params, None, &settings)
        .await
        .map_err(js_error)?
        .ok_or_else(|| JsValue::from_str("Missing unchecked value"))?;
    element("unchecked")?.set_text_content(Some(&format!("Untrusted JSON: {value}")));

    let counter = element("count")?;
    let count = Cell::new(0);
    let click = EventListener::new(element("increment")?.as_ref(), "click", move |_| {
        count.set(count.get() + 1);
        counter.set_text_content(Some(&count.get().to_string()));
    })?;
    let logs = element("logs")?;
    let window = web_sys::window().ok_or_else(|| JsValue::from_str("Missing window"))?;
    let scheme = if window.location().protocol()? == "https:" {
        "wss"
    } else {
        "ws"
    };
    let hot = HotReloadConnection::connect(
        &format!("{scheme}://{}/hot", window.location().host()?),
        move |text| {
            logs.set_text_content(Some(&text));
        },
    )?;
    let socket = hot.socket().clone();
    let closed = element("socket")?;
    let close = EventListener::new(socket.as_ref(), "close", move |_| {
        closed.set_text_content(Some("Closed"))
    })?;
    let connections = Rc::new(RefCell::new(Some(Connections { click, hot })));
    let opened = element("socket")?;
    let weak = Rc::downgrade(&connections);
    let open = EventListener::new(socket.as_ref(), "open", move |_| {
        opened.set_text_content(Some("Open"));
        let Some(connections) = weak.upgrade() else {
            return;
        };
        let connections = connections.borrow();
        let Some(connections) = connections.as_ref() else {
            return;
        };
        if let Err(error) = connections.hot.request_logs() {
            web_sys::console::error_1(&error);
        }
    })?;
    let owned = connections.clone();
    let disposed = element("disposal")?;
    let dispose = EventListener::new(element("dispose")?.as_ref(), "click", move |_| {
        let result = owned.borrow_mut().take().map(|mut connections| {
            connections
                .click
                .close()
                .and_then(|()| connections.hot.close())
        });
        if let Some(Err(error)) = result {
            web_sys::console::error_1(&error);
            disposed.set_text_content(Some("Disposal failed"));
            return;
        }
        disposed.set_text_content(Some("Disposed"));
    })?;
    element("status")?.set_text_content(Some("Ready"));
    Ok(Page {
        controls: vec![open, close, dispose],
        connections,
    })
}
