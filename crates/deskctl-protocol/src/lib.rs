//! Protocol types shared by the Deskctl daemon, CLI, and MCP adapter.

pub mod error;
pub mod result;
pub mod schema;
pub mod tool;

pub use error::*;
pub use result::*;
pub use schema::*;
pub use tool::*;
