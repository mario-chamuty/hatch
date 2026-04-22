//! Legacy install entry point. All paths now funnel through
//! `install_smart::execute`; this shim is kept to preserve the public CLI
//! dispatch shape.

use anyhow::Result;

pub async fn execute(profile: Option<String>) -> Result<()> {
    super::install_smart::execute(profile).await
}

pub async fn execute_with_options(
    profile: Option<String>,
    _relax_constraints: bool,
    _force: bool,
) -> Result<()> {
    super::install_smart::execute(profile).await
}
