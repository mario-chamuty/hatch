pub const ICON_PNG: &[u8] = include_bytes!("../icon.png");

pub const HATCH_ASCII_LOGO: &str = r#"
    __  __      __       __            ,'"`.
   / / / /___ _/ /______/ /_          /     \
  / /_/ / __ `/ __/ ___/ __ \        / </> /
 / __  / /_/ / /_/ /__/ / / /       /  ˜˜ /
/_/ /_/\__,_/\__/\___/_/ /_/        `.,_,'

"#;

pub const HATCH_BANNER: &str = r#"
╭─────────────────────────────────────────╮
│  Hatch - Flutter Dependency Manager     │
│  Next-gen package management & builds   │
╰─────────────────────────────────────────╯
"#;

pub fn get_version_string() -> String {
    format!("Hatch v{}", env!("CARGO_PKG_VERSION"))
}