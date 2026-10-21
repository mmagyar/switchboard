#[derive(Debug, Eq, PartialEq)]
enum HotReloadMessage<'a> {
    Reload,
    Text(&'a str),
}

fn classify_message(text: &str) -> HotReloadMessage<'_> {
    if text == "RELOAD" {
        HotReloadMessage::Reload
    } else {
        HotReloadMessage::Text(text)
    }
}

#[cfg(target_arch = "wasm32")]
mod connection {
    use super::{HotReloadMessage, classify_message};
    use crate::EventListener;
    use wasm_bindgen::{JsCast, JsValue};
    use web_sys::{MessageEvent, WebSocket};

    /// A hot-reload/log socket. Drop detaches its callback and closes the connection.
    /// Binary messages are ignored; every non-RELOAD text message is passed unchanged.
    #[must_use = "dropping the handle closes the WebSocket"]
    pub struct HotReloadConnection {
        socket: WebSocket,
        message_listener: EventListener,
    }

    impl HotReloadConnection {
        pub fn connect(
            url: &str,
            mut on_text: impl FnMut(String) + 'static,
        ) -> Result<Self, JsValue> {
            let window = web_sys::window()
                .ok_or_else(|| JsValue::from_str("Hot reload requires a browser Window"))?;
            let socket = WebSocket::new(url)?;
            let listener = EventListener::new(socket.as_ref(), "message", move |event| {
                let Ok(event) = event.dyn_into::<MessageEvent>() else {
                    return;
                };
                let Some(text) = event.data().as_string() else {
                    return;
                };
                match classify_message(&text) {
                    HotReloadMessage::Reload => {
                        if let Err(error) = window.location().reload() {
                            web_sys::console::error_1(&error);
                        }
                    }
                    HotReloadMessage::Text(text) => on_text(text.to_owned()),
                }
            });
            match listener {
                Ok(message_listener) => Ok(Self {
                    socket,
                    message_listener,
                }),
                Err(error) => {
                    if let Err(close_error) = socket.close() {
                        web_sys::console::error_1(&close_error);
                    }
                    Err(error)
                }
            }
        }

        /// Exposes readyState and registration of open/error/close listeners via EventListener.
        pub fn socket(&self) -> &WebSocket {
            &self.socket
        }

        /// Call after the socket opens. The server treats any outbound payload as a log request.
        /// No automatic reconnection or initial log request is performed.
        pub fn request_logs(&self) -> Result<(), JsValue> {
            if self.socket.ready_state() != WebSocket::OPEN {
                return Err(JsValue::from_str(
                    "Cannot request logs: WebSocket is not open",
                ));
            }
            self.socket.send_with_str("")
        }

        pub fn close(&mut self) -> Result<(), JsValue> {
            let removed = self.message_listener.close();
            let closed = self.socket.close();
            removed.and(closed)
        }
    }

    impl Drop for HotReloadConnection {
        fn drop(&mut self) {
            if let Err(error) = self.close() {
                web_sys::console::error_1(&error);
            }
        }
    }
}

#[cfg(target_arch = "wasm32")]
pub use connection::HotReloadConnection;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_exact_reload_command_reloads() {
        assert_eq!(classify_message("RELOAD"), HotReloadMessage::Reload);
        for text in [
            "",
            "reload",
            "RELOAD\n",
            " RELOAD",
            "request log line\nsecond line",
        ] {
            assert_eq!(classify_message(text), HotReloadMessage::Text(text));
        }
    }
}
