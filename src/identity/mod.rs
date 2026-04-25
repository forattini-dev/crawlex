//! IdentityBundle — coherent "who is the browser pretending to be?" record.
//!
//! Replaces the 3-variant `Profile` enum with a versioned, persisted bundle
//! that every observation surface (HTTP headers, JS shim, Chromium launch
//! flags) draws from. A single bundle instance is bound to a
//! [`SessionIdentity`]; every request within a session uses the same
//! bundle, because rotating identity mid-session is itself a tell.
//!
//! The bundle is *derived from* the Chromium version actually bundled, not
//! the other way around — so if we ship Chromium 149 we never claim 131.

pub mod bundle;
pub mod profiles;
pub mod session_registry;
pub mod validator;
pub mod warmup;

pub use bundle::{IdentityBundle, SessionIdentity};
pub use profiles::{
    catalog as persona_catalog, pick as persona_pick, PersonaGpu, PersonaOs, PersonaProfile,
};
pub use session_registry::{
    spawn_cleanup_task, EvictionReason, SessionArchive, SessionDropTarget, SessionEntry,
    SessionRegistry, SessionSnapshot, StorageArchive, DEFAULT_SESSION_TTL_SECS,
};
pub use validator::{IdentityValidator, ValidationError};
