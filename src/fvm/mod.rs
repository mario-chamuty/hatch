pub mod detector;
pub mod installer;
pub mod manager;

pub use detector::FvmDetector;
pub use installer::FvmInstaller;
pub use manager::{FvmManager, ProjectFlutterInfo};