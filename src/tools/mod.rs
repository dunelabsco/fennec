pub mod traits;
pub mod shell;
pub mod files;
pub mod web;
pub mod collective_tools;
pub mod delegate_tool;
pub mod skills_tool;
pub mod mcp_tools;

pub use traits::{Tool, ToolResult, ToolSpec};
pub use web::{WebFetchTool, WebSearchTool};
pub use collective_tools::{CollectiveSearchTool, CollectiveReportTool};
pub use delegate_tool::DelegateTool;
pub use skills_tool::SkillsTool;
pub use mcp_tools::{McpToolBridge, register_mcp_tools};
