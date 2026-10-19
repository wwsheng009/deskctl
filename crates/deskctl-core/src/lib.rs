//! Deskctl core: session isolation, tool invocation, profiles and diagnostics.

pub mod apps;
pub mod audit;
pub mod capture_policy;
pub mod checkpoint;
pub mod config;
pub mod doctor;
pub mod docx;
mod engine;
pub mod humanize;
pub mod kv;
pub mod models;
pub mod platform;
pub mod pptx;
pub mod recorder;
mod resource_gate;
pub mod script;
pub mod session;
pub mod text_util;
pub mod tools;
pub mod untrusted;
pub mod vision;
pub mod xlsx;

pub use config::Config;
pub use engine::{CancelFlag, Engine, SessionInfo};
pub use session::Session;
