//! GPUI Studio desktop entry point.

use std::path::PathBuf;

use clap::Parser;
use gpui_studio::ui::{StudioConfig, run};

#[derive(Debug, Parser)]
#[command(
    name = "gpui-studio",
    about = "Offline-first native design canvas for HTML/CSS artboards"
)]
struct Arguments {
    /// Design folder to open; created with a starter design when empty.
    #[arg(long, value_name = "PATH")]
    project: Option<PathBuf>,
    /// Do not start the local MCP bridge for agents.
    #[arg(long)]
    no_mcp: bool,
}

fn main() {
    let arguments = Arguments::parse();
    let project = arguments.project.unwrap_or_else(|| {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("examples")
            .join("welcome")
    });
    run(StudioConfig {
        project,
        mcp: !arguments.no_mcp,
    });
}
