//! GPUI Studio desktop process entry point.

use std::path::PathBuf;

use anyhow::{Context as _, Result};
use clap::Parser;
use directories::BaseDirs;
use gpui_studio::{StudioConfig, ThemeLocation, WorkspaceSettings, run};

#[derive(Debug, Parser)]
#[command(
    name = "gpui-studio",
    about = "Offline-first native visual builder for HTML-backed GPUI projects"
)]
struct Arguments {
    /// Project root containing ui/app.html, app.css, and app.bindings.ron.
    #[arg(long, value_name = "PATH")]
    project: Option<PathBuf>,
    /// Disable the local owner-restricted MCP endpoint.
    #[arg(long)]
    no_mcp: bool,
}

fn main() -> Result<()> {
    let arguments = Arguments::parse();
    let studio_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let workspace = WorkspaceSettings::load(&studio_root).context("load Studio workspace")?;
    let project_root = arguments
        .project
        .unwrap_or_else(|| workspace.project_root(&studio_root));
    let mut theme_locations = Vec::new();
    if let Some(base) = BaseDirs::new() {
        theme_locations.push(ThemeLocation::User(
            base.home_dir().join(".gpui-studio").join("themes"),
        ));
    }
    theme_locations.push(ThemeLocation::Project(
        project_root.join(".gpui-studio").join("themes"),
    ));
    run(StudioConfig {
        studio_root,
        project_root,
        mcp_enabled: !arguments.no_mcp,
        workspace,
        theme_locations,
    });
    Ok(())
}
