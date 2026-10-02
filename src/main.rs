//! GPUI Studio desktop entry point.

use std::path::PathBuf;

use clap::Parser;
use gpui_studio::ui::{LaunchConfig, run};

#[derive(Debug, Parser)]
#[command(
    name = "gpui-studio",
    about = "Offline-first native design canvas for HTML/CSS artboards"
)]
struct Arguments {
    /// Design folder to open in a tab; created with a starter design when empty.
    /// Without it, Studio restores the previous session.
    #[arg(long, value_name = "PATH")]
    project: Option<PathBuf>,
    /// Do not start the local MCP bridge for agents.
    #[arg(long)]
    no_mcp: bool,
}

fn main() {
    let arguments = Arguments::parse();
    let example = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("examples")
        .join("welcome");
    run(LaunchConfig {
        project: arguments.project,
        example: example.is_dir().then_some(example),
        state_path: None,
        mcp: !arguments.no_mcp,
    });
}
