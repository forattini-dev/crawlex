//! In-house reCAPTCHA v3 invisible solver.
//!
//! Port of [`reCaptchaV3-Invisible-Solver`](https://github.com/h4ckf0r0day/reCaptchaV3-Invisible-Solver)
//! adapted to our identity stack:
//! * UA / UA-CH / screen / timezone / canvas / WebGL flow from the active
//!   `IdentityBundle` instead of a hardcoded Windows / Chrome 136 — fixes
//!   the cross-check failure mode of the reference.
//! * Server-side replay (no browser). Empirical scoring 0.3-0.9 from the
//!   reference; treat as a fallback when a real browser path isn't
//!   available.
//!
//! Layout:
//! * `proto.rs` — minimal protobuf encoder (~80 LOC, no `prost`).
//! * `utils.rs` — base36, `cb` query param, `co` origin encoding,
//!   `scramble_oz` per-byte cipher.
//! * `oz.rs` — build the `oz` JSON with the right field numbers.
//! * `telemetry.rs` — synthesise the field-74 client blob (mouse,
//!   scroll, perf, domains).
//! * `solver.rs` — pipeline: api.js → anchor → reload, regex out the
//!   tokens, return the `rresp`.
//! * `adapter.rs` — wires the solver into our `CaptchaSolver` trait so
//!   the existing scheduler / policy paths can route reCAPTCHA challenges
//!   to it via `SolverKind::RecaptchaInvisible`.
//!
//! Pure modules (`proto`, `utils`, `oz`, `telemetry`) are always
//! available. The networked `solver` and `adapter` modules need
//! `reqwest`, which is part of the default `cdp-backend` feature.

pub mod oz;
pub mod proto;
pub mod telemetry;
pub mod utils;

#[cfg(feature = "cdp-backend")]
pub mod adapter;
#[cfg(feature = "cdp-backend")]
pub mod solver;

#[cfg(feature = "cdp-backend")]
pub use adapter::RecaptchaInvisibleAdapter;
#[cfg(feature = "cdp-backend")]
pub use solver::{solve, SolveOutcome, SolveRequest, SolverError as RecaptchaSolverError};
