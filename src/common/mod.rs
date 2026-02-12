//! Common utilities and shared types used across the project.
//!
//! This module collects small helpers and shared data shapes that are used by
//! multiple crates/modules in the codebase. The primary items here are:
//!
//! - `get_env` — helper to read configuration from environment variables with a
//!   fallback default. It first checks a `.env` file via `dotenvy`, then the
//!   process environment variables, and finally returns the provided default.
//! - A thread-local `REQUEST_ID` used to attach a per-request `Uuid` to code
//!   running on the same OS thread. Accessors `set_request_id` and
//!   `get_request_id` are provided.
//! - `UserEvent` — a small struct representing analytic/user event data used
//!   when emitting metrics or audit events.
//!
//! # Environment lookup behaviour (get_env)
//!
//! `get_env(key, default)` resolves configuration in the following order:
//! 1. `.env` file via `dotenvy::var(key)` (if present in the project root or
//!    configured dotenv location).
//! 2. Process environment variables via `std::env::var(key)`.
//! 3. The provided `default` string returned when neither source has the key.
//!
//! The function logs an `info!` message when the default is used.

use std::cell::RefCell;
use crate::common;
use log::{debug, error, info, trace, warn, Level, LevelFilter, Log, Metadata, Record};
use nanoid::nanoid;
use std::env;
use once_cell::sync::Lazy;
use tokio::sync::Mutex;
use uuid::Uuid;

/// Read a particular key from the environment with a fallback default.
///
/// This helper first attempts to read the key from a `.env` file (using
/// `dotenvy::var`) which is convenient for local development. If that fails it
/// will fall back to the process environment (`std::env::var`). If both lookups
/// fail the provided `default` string is returned.
///
/// # Arguments
/// * `key` - The environment variable name to read.
/// * `default` - The default value to return when the key is not set.
///
/// # Returns
/// A `String` containing the resolved value or the provided `default`.
///
/// # Example
/// ```no_run
/// let port = crate::common::get_env("PORT", "8080");
/// println!("listening on {}", port);
/// ```
pub fn get_env(key: &str, default: &str) -> String {
    // Check .env file
    match dotenvy::var(key) {
        Ok(v) => return v,
        Err(_) => {
            // Check linux env
            match env::var(key) {
                Ok(v) => return v,
                Err(_) => {
                    // def
                    info!("{key} not provided, defaulting to {default}");
                    default.to_string()
                }
            }
        }
    }
}

thread_local! {
    /// Thread-local storage for an optional request id.
    ///
    /// This stores an `Option<Uuid>` keyed to the current OS thread. It is
    /// useful for code that needs to attach a request identifier to logs or
    /// to propagate correlation ids across synchronous call stacks. Note that
    /// this is thread-local only — if your code switches threads (e.g. across
    /// async await points) the stored value will not automatically follow.
    static REQUEST_ID: RefCell<Option<Uuid>> = RefCell::new(None);
}

/// Set the current thread's request id.
///
/// This stores `request_id` into the thread-local `REQUEST_ID`. Typical use
/// is at the start of handling an incoming request so subsequent code can
/// read the id for logging or tracing.
///
/// # Example
/// ```no_run
/// use uuid::Uuid;
/// let id = Uuid::new_v4();
/// crate::common::set_request_id(id);
/// ```
pub fn set_request_id(request_id: Uuid) {
    REQUEST_ID.with(|id| *id.borrow_mut() = Some(request_id));
}

/// Get the current thread's request id, if one has been set.
///
/// Returns `Some(Uuid)` when a request id was previously set via
/// `set_request_id` on the same thread, otherwise `None`.
///
/// # Example
/// ```no_run
/// if let Some(id) = crate::common::get_request_id() {
///     println!("current request id: {}", id);
/// }
/// ```
pub fn get_request_id() -> Option<Uuid> {
    REQUEST_ID.with(|id| *id.borrow())
}


#[derive(Debug, Clone)]
/// A compact representation of a user/analytic event used across the
/// application when emitting metrics or forwarding events to other services.
///
/// The struct is intentionally small and serializable-friendly; fields reflect
/// the commonly required attributes for tracing and analytics pipelines.
pub struct UserEvent {
    /// Unique identifier for the event instance (UUIDv4 recommended).
    pub event_id : Uuid,
    /// Identifier for the user associated with the event (application-level id).
    pub user_id : String,
    /// Human or programmatic event type (for example: `"join"`, `"leave"`).
    pub event_type : String,
    /// Event timestamp as milliseconds since Unix epoch (i64).
    pub event_time : i64,
    /// Optional instance id where the event originated (for multi-instance
    /// deployments). Stored as a string for flexibility.
    pub instance_id: String,
    /// Identifier for the meeting or session this event belongs to (if any).
    pub meeting_id : String,
}
