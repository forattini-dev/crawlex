//! Re-export of the neutral `WaitStrategy` type so the render module can
//! keep importing from `render::wait` while `config` depends on the
//! feature-free copy in the crate root.

pub use crate::wait_strategy::WaitStrategy;
