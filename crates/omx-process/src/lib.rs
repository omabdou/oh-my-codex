pub mod platform_command;
pub mod process_bridge;

pub use platform_command::{
    PlatformCommandSpec, SpawnErrorKind, WindowsCommandKind, build_platform_command_spec,
    classify_spawn_error, resolve_command_path_for_platform,
};
pub use process_bridge::{
    CommandSpec, Platform, PlatformResolution, ProcessBridge, ProcessResult, StdioMode,
};
