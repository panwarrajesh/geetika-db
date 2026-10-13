//! RajeshDB platform (Phase 5): users, organizations, projects/environments, per-project
//! databases, project API keys, quotas, usage and audit log, on top of the document layer.
//!
//! Isolation model: every project environment is its own directory with its own `DocDb`
//! (`<data>/projects/<project_id>/<env>`). The directory name comes only from a validated
//! 32-hex project id and a fixed environment list, never from user-supplied paths, and every
//! data request is authorized (key scope or org membership) BEFORE that database is opened.

pub mod backup;
pub mod error;
pub mod handlers;
pub mod limiter;
pub mod state;
pub mod types;
pub mod util;

pub use handlers::build_app;
pub use state::{AppState, Config};
