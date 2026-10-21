use wasm_bindgen::{JsCast, JsValue, closure::Closure};
use web_sys::{Event, EventTarget};

/// Owns a DOM listener and its Rust callback. Keep the handle alive while listening.
#[must_use = "dropping the handle removes the event listener"]
pub struct EventListener {
    target: EventTarget,
    event: String,
    callback: Option<Closure<dyn FnMut(Event)>>,
}

impl EventListener {
    pub fn new(
        target: &EventTarget,
        event: impl Into<String>,
        callback: impl FnMut(Event) + 'static,
    ) -> Result<Self, JsValue> {
        let event = event.into();
        let callback = Closure::wrap(Box::new(callback) as Box<dyn FnMut(Event)>);
        target.add_event_listener_with_callback(&event, callback.as_ref().unchecked_ref())?;
        Ok(Self {
            target: target.clone(),
            event,
            callback: Some(callback),
        })
    }

    /// Idempotent. A failed removal keeps the callback alive so removal can be retried.
    pub fn close(&mut self) -> Result<(), JsValue> {
        if let Some(callback) = &self.callback {
            self.target.remove_event_listener_with_callback(
                &self.event,
                callback.as_ref().unchecked_ref(),
            )?;
            self.callback = None;
        }
        Ok(())
    }
}

impl Drop for EventListener {
    fn drop(&mut self) {
        if let Err(error) = self.close() {
            // Drop cannot return an error. Standard EventTarget removal does not throw.
            web_sys::console::error_1(&error);
        }
    }
}
