use llrt_modules::module_builder::ModuleBuilder;
use llrt_modules::{
    abort, assert, buffer, child_process, console, crypto, events, exceptions, fetch, fs, os, path,
    perf_hooks, process, stream_web, string_decoder, timers, tty, url, util, zlib,
};
use std::collections::HashSet;

use crate::types::LlrtSupportedModules;

macro_rules! define_safe_modules {
    (
        $(
            $variant:ident => {
                snake: $snake:literal,
                $( global: $global:expr, )?
                $( module: $module:expr, )?
            }
        ),* $(,)?
    ) => {
        pub const DEFAULT_MODULES: &[LlrtSupportedModules] = &[
            $( LlrtSupportedModules::$variant, )*
        ];

        fn init_safe_modules(default_modules: &HashSet<LlrtSupportedModules>, mut module_builder: ModuleBuilder) -> ModuleBuilder {
            $(
                if default_modules.contains(&LlrtSupportedModules::$variant) {
                    $(
                        module_builder = module_builder.with_global($global);
                    )?
                    $(
                        module_builder = module_builder.with_module($module);
                    )?
                }
            )*
            module_builder
        }
    };
}

macro_rules! define_unsafe_modules {
    (
        $(
            $variant:ident => {
                snake: $snake:literal,
                method: $method:ident,
                init: |$builder:ident| $init_expr:expr
            }
        ),* $(,)?
    ) => {
        pub const UNSAFE_MODULES: &[LlrtSupportedModules] = &[
            $( LlrtSupportedModules::$variant, )*
        ];

        impl LlrtModuleBuilder {
            $(
                pub fn $method(&mut self) -> &mut Self {
                    let $builder = std::mem::take(&mut self.builder);
                    self.builder = $init_expr;
                    self
                }
            )*
        }
    };
}

// Define safe (default) modules
define_safe_modules! {
    Abort => { snake: "abort", global: abort::init, },
    Assert => { snake: "assert", module: assert::AssertModule, },
    Buffer => { snake: "buffer", global: buffer::init, module: buffer::BufferModule, },
    Console => { snake: "console", global: console::init, module: console::ConsoleModule, },
    Crypto => { snake: "crypto", global: crypto::init, module: crypto::CryptoModule, },
    Events => { snake: "events", global: events::init, module: events::EventsModule, },
    Exceptions => { snake: "exceptions", global: exceptions::init, },
    Os => { snake: "os", module: os::OsModule, },
    Path => { snake: "path", module: path::PathModule, },
    PerfHooks => { snake: "perf_hooks", global: perf_hooks::init, module: perf_hooks::PerfHooksModule, },
    Process => { snake: "process", global: process::init, module: process::ProcessModule, },
    StreamWeb => { snake: "stream_web", global: stream_web::init, module: stream_web::StreamWebModule, },
    StringDecoder => { snake: "string_decoder", module: string_decoder::StringDecoderModule, },
    Timers => { snake: "timers", global: timers::init, module: timers::TimersModule, },
    Tty => { snake: "tty", module: tty::TtyModule, },
    Url => { snake: "url", global: url::init, module: url::UrlModule, },
    Util => { snake: "util", global: util::init, module: util::UtilModule, },
    Zlib => { snake: "zlib", module: zlib::ZlibModule, },
}

define_unsafe_modules! {
    Fetch => {
        snake: "fetch",
        method: enable_fetch,
        init: |builder| builder.with_global(fetch::init)
    },
    Fs => {
        snake: "fs",
        method: enable_fs,
        init: |builder| builder
            .with_module(fs::FsPromisesModule)
            .with_module(fs::FsModule)
    },
    ChildProcess => {
        snake: "child_process",
        method: enable_child_process,
        init: |builder| builder.with_module(child_process::ChildProcessModule)
    },
}

fn default_modules_set() -> HashSet<LlrtSupportedModules> {
    DEFAULT_MODULES.iter().copied().collect()
}

pub struct LlrtModuleBuilder {
    pub builder: ModuleBuilder,
}

impl LlrtModuleBuilder {
    pub fn build() -> Self {
        let default_modules = default_modules_set();
        let module_builder = ModuleBuilder::new();
        let module_builder = init_safe_modules(&default_modules, module_builder);

        Self {
            builder: module_builder,
        }
    }
}
