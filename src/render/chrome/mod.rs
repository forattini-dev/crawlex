//! A high-level API for programmatically interacting with the [Chrome DevTools Protocol](https://chromedevtools.github.io/devtools-protocol/).
//!
//! This module uses the [Chrome DevTools protocol] to drive/launch a Chromium or
//! Chrome (potentially headless) browser.

#![allow(clippy::all)]
#![allow(warnings)]
#![allow(macro_expanded_macro_exports_accessed_by_absolute_paths)]
#![warn(missing_debug_implementations, rust_2018_idioms)]

use crate::render::chrome::handler::http::HttpRequest;
use std::sync::Arc;

/// reexport the generated cdp types
pub use crate::render::chrome_protocol as cdp;
pub use crate::render::chrome_wire::{self as types, Binary, Command, Method, MethodType};

pub use crate::render::chrome::browser::{Browser, BrowserConfig};
pub use crate::render::chrome::conn::Connection;
pub use crate::render::chrome::element::Element;
pub use crate::render::chrome::error::Result;
#[cfg(feature = "fetcher")]
pub use crate::render::chrome::fetcher::{BrowserFetcher, BrowserFetcherOptions};
pub use crate::render::chrome::handler::Handler;
pub use crate::render::chrome::page::Page;

pub mod auth;
pub mod browser;
pub mod cmd;
pub mod conn;
pub mod detection;
pub mod element;
pub mod error;
#[cfg(feature = "fetcher")]
pub mod fetcher {
    pub use crate::render::chrome_fetcher::*;
}
pub mod async_process;
pub mod handler;
pub mod js;
pub mod keys;
pub mod layout;
pub mod listeners;
pub mod page;
pub(crate) mod utils;

pub type ArcHttpRequest = Option<Arc<HttpRequest>>;
