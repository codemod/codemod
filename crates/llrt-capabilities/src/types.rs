use serde::{Deserialize, Serialize};
use ts_rs::TS;

macro_rules! define_module_enum {
    ( $( $variant:ident => $snake:literal ),* $(,)? ) => {
        #[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
        #[serde(rename_all = "snake_case")]
        pub enum LlrtSupportedModules {
            $( $variant, )*
        }

        impl std::str::FromStr for LlrtSupportedModules {
            type Err = String;

            fn from_str(s: &str) -> Result<Self, Self::Err> {
                match s.to_lowercase().as_str() {
                    $( $snake => Ok(Self::$variant), )*
                    _ => Err(format!("Unknown module: {}", s)),
                }
            }
        }
    };
}

// Define the enum and FromStr implementation
define_module_enum! {
    Abort => "abort",
    Assert => "assert",
    Buffer => "buffer",
    Console => "console",
    Crypto => "crypto",
    Events => "events",
    Exceptions => "exceptions",
    Fetch => "fetch",
    Fs => "fs",
    Os => "os",
    Path => "path",
    PerfHooks => "perf_hooks",
    Process => "process",
    StreamWeb => "stream_web",
    StringDecoder => "string_decoder",
    Timers => "timers",
    Tty => "tty",
    Url => "url",
    Util => "util",
    Zlib => "zlib",
    ChildProcess => "child_process",
}
