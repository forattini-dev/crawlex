//! v2 scraping framework — top-level entry points.
//!
//! Slice 16 introduces the minimal surface needed to route a scrape
//! through a named session:
//!
//! * [`Request`] — the request descriptor exposed to recipes. Carries
//!   an optional `session_id` so callers can pin successive fetches to
//!   an isolated engine state (cookie jar today; identity bundle and
//!   page in later slices).
//! * [`SessionManager`] — owns a registry mapping session ids to a
//!   [`BackendKind`] and an isolated per-session [`CookieJar`]. Unknown
//!   ids fall back to the default backend with a `warn!` log.
//!
//! The manager deliberately does *not* attempt to drive the real HTTP /
//! render pools yet — those land in subsequent slices. The contract
//! locked down here is: "same backend kind, different session id =>
//! cookies do not leak."

pub mod replay;
pub mod request;
pub mod session;
pub mod spider;

pub use replay::{DirReplay, RecordedResponse, ReddbReplay, Replay, ReplayingFetcher};
pub use request::Request;
pub use session::{BackendKind, CookieJar, SessionEntry, SessionManager};
pub use spider::{
    Checkpoint, CheckpointRequest, FetchError, Fetcher, ParseYield, Response, RunOutcome, Spider,
    SpiderConfig, SpiderRunner,
};
