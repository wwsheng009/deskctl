//! Deskctl core: session isolation, tool invocation, profiles and diagnostics.

pub mod apps;
pub mod capture_policy;
pub mod config;
pub mod doctor;
pub mod docx;
mod engine;
pub mod models;
pub mod platform;
mod resource_gate;
pub mod session;
pub mod tools;
pub mod vision;

pub use config::Config;
pub use engine::{CancelFlag, Engine, SessionInfo};
pub use session::Session;
