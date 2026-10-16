#![forbid(unsafe_code)]

mod client;
#[cfg(any(target_arch = "wasm32", test))]
pub use client::BrowserClient;
pub use client::{
    CallSettings, ClientError, ClientErrorKind, ClientOptions, Credentials,
    INVALID_RESPONSE_STATUS, NETWORK_ERROR_STATUS,
};

#[cfg(target_arch = "wasm32")]
mod browser_fetch;
#[cfg(target_arch = "wasm32")]
mod event_listener;
#[cfg(target_arch = "wasm32")]
pub use event_listener::EventListener;
#[cfg(any(target_arch = "wasm32", test))]
mod hot_reload;
#[cfg(target_arch = "wasm32")]
pub use hot_reload::HotReloadConnection;
