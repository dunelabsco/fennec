pub mod traits;
pub mod shell;
pub mod files;
pub mod web;

pub use traits::{Tool, ToolResult, ToolSpec};
pub use web::{WebFetchTool, WebSearchTool};
