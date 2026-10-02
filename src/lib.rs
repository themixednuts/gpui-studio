//! GPUI Studio: an offline-first native design canvas.
//!
//! Designs are pages of artboards whose contents are real HTML elements with
//! inline CSS, stored as standalone `.html` files. The editor shell is built on
//! GPUI Kit, and every edit is also available to MCP agents through gpui-mcp.

#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

pub mod agent;
pub mod assets;
pub mod comments;
pub mod editor;
pub mod export;
pub mod geometry;
pub mod history;
pub mod model;
pub mod presets;
pub mod project;
pub mod shapes;
pub mod ui;
pub mod workspace;
