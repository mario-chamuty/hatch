use anyhow::Result;
use colored::Colorize;
use crate::branding;

pub async fn execute() -> Result<()> {
    println!("{}", branding::HATCH_ASCII_LOGO.cyan().bold());

    println!("Version: {}", env!("CARGO_PKG_VERSION").green());
    println!("Build: {}", "Release".yellow());
    println!("Platform: {}", std::env::consts::OS.blue());
    println!("Architecture: {}", std::env::consts::ARCH.blue());

    println!("\n{}", "The unified toolchain Flutter has been missing.".italic());
    println!("{}", "Smarter dependencies, automatic builds, team-wide version sync.".italic());

    println!("\n{}", "Created with ❤️ for the Flutter community".red());

    Ok(())
}