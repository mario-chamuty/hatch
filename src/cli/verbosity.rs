use once_cell::sync::OnceCell;

static VERBOSITY_LEVEL: OnceCell<u8> = OnceCell::new();

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum VerbosityLevel {
    Quiet = 0,
    Normal = 1,
    Verbose = 2,      // -v
    VeryVerbose = 3,  // -vv
    Debug = 4,        // -vvv
    UltraVerbose = 5, // -vvvv (extraction/storage details)
}

impl From<u8> for VerbosityLevel {
    fn from(value: u8) -> Self {
        match value {
            0 => VerbosityLevel::Normal,
            1 => VerbosityLevel::Verbose,
            2 => VerbosityLevel::VeryVerbose,
            3 => VerbosityLevel::Debug,
            4 => VerbosityLevel::UltraVerbose,
            _ => VerbosityLevel::UltraVerbose,  // 5+ also ultra-verbose
        }
    }
}

pub fn set_verbosity(level: u8) {
    let _ = VERBOSITY_LEVEL.set(level);
}

pub fn get_verbosity() -> VerbosityLevel {
    VERBOSITY_LEVEL.get().copied().unwrap_or(0).into()
}

pub fn is_verbose() -> bool {
    get_verbosity() >= VerbosityLevel::Verbose
}

pub fn is_very_verbose() -> bool {
    get_verbosity() >= VerbosityLevel::VeryVerbose
}

pub fn is_debug() -> bool {
    get_verbosity() >= VerbosityLevel::Debug
}

pub fn is_ultra_verbose() -> bool {
    get_verbosity() >= VerbosityLevel::UltraVerbose
}

pub fn should_show_download_details() -> bool {
    get_verbosity() >= VerbosityLevel::Debug
}

pub fn should_show_timings() -> bool {
    get_verbosity() >= VerbosityLevel::Verbose
}

pub fn should_show_progress_bars() -> bool {
    get_verbosity() < VerbosityLevel::Debug
}