use llrt_modules::module_builder::ModuleBuilder;
use llrt_modules::{
    abort, assert, buffer, child_process, console, crypto, events, exceptions, fetch, fs, os, path,
    perf_hooks, process, stream_web, string_decoder, timers, tty, url, util, zlib,
};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

pub const UNSAFE_MODULES: &[LlrtSupportedModules] = &[
    LlrtSupportedModules::Fetch,
    LlrtSupportedModules::ChildProcess,
    LlrtSupportedModules::Fs,
];
pub const DEFAULT_MODULES: &[LlrtSupportedModules] = &[
    LlrtSupportedModules::Abort,
    LlrtSupportedModules::Assert,
    LlrtSupportedModules::Buffer,
    LlrtSupportedModules::Console,
    LlrtSupportedModules::Crypto,
    LlrtSupportedModules::Events,
    LlrtSupportedModules::Exceptions,
    LlrtSupportedModules::Os,
    LlrtSupportedModules::Path,
    LlrtSupportedModules::PerfHooks,
    LlrtSupportedModules::Process,
    LlrtSupportedModules::StreamWeb,
    LlrtSupportedModules::StringDecoder,
    LlrtSupportedModules::Timers,
    LlrtSupportedModules::Tty,
    LlrtSupportedModules::Url,
    LlrtSupportedModules::Util,
    LlrtSupportedModules::Zlib,
];

fn default_modules_set() -> HashSet<LlrtSupportedModules> {
    DEFAULT_MODULES.iter().copied().collect()
}

pub struct LlrtModuleBuilder {
    pub builder: ModuleBuilder,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LlrtSupportedModules {
    Abort,
    Assert,
    Buffer,
    Console,
    Crypto,
    Events,
    Exceptions,
    Fetch,
    Fs,
    Os,
    Path,
    PerfHooks,
    Process,
    StreamWeb,
    StringDecoder,
    Timers,
    Tty,
    Url,
    Util,
    Zlib,
    ChildProcess,
}

impl std::str::FromStr for LlrtSupportedModules {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "abort" => Ok(Self::Abort),
            "assert" => Ok(Self::Assert),
            "buffer" => Ok(Self::Buffer),
            "console" => Ok(Self::Console),
            "crypto" => Ok(Self::Crypto),
            "events" => Ok(Self::Events),
            "exceptions" => Ok(Self::Exceptions),
            "fetch" => Ok(Self::Fetch),
            "fs" => Ok(Self::Fs),
            "os" => Ok(Self::Os),
            "path" => Ok(Self::Path),
            "perf_hooks" => Ok(Self::PerfHooks),
            "process" => Ok(Self::Process),
            "stream_web" => Ok(Self::StreamWeb),
            "string_decoder" => Ok(Self::StringDecoder),
            "timers" => Ok(Self::Timers),
            "tty" => Ok(Self::Tty),
            "url" => Ok(Self::Url),
            "util" => Ok(Self::Util),
            "zlib" => Ok(Self::Zlib),
            "child_process" => Ok(Self::ChildProcess),
            _ => Err(format!("Unknown module: {}", s)),
        }
    }
}

impl LlrtModuleBuilder {
    pub fn build() -> Self {
        let default_modules = default_modules_set();
        let mut module_builder = ModuleBuilder::new();

        if default_modules.contains(&LlrtSupportedModules::Abort) {
            module_builder = module_builder.with_global(abort::init);
        }

        if default_modules.contains(&LlrtSupportedModules::Assert) {
            module_builder = module_builder.with_module(assert::AssertModule);
        }

        if default_modules.contains(&LlrtSupportedModules::Buffer) {
            module_builder = module_builder.with_global(buffer::init);
            module_builder = module_builder.with_module(buffer::BufferModule);
        }

        if default_modules.contains(&LlrtSupportedModules::Console) {
            module_builder = module_builder.with_global(console::init);
            module_builder = module_builder.with_module(console::ConsoleModule);
        }

        if default_modules.contains(&LlrtSupportedModules::Crypto) {
            module_builder = module_builder.with_global(crypto::init);
            module_builder = module_builder.with_module(crypto::CryptoModule);
        }

        if default_modules.contains(&LlrtSupportedModules::Events) {
            module_builder = module_builder.with_global(events::init);
            module_builder = module_builder.with_module(events::EventsModule);
        }

        if default_modules.contains(&LlrtSupportedModules::Exceptions) {
            module_builder = module_builder.with_global(exceptions::init);
        }

        if default_modules.contains(&LlrtSupportedModules::Os) {
            module_builder = module_builder.with_module(os::OsModule);
        }

        if default_modules.contains(&LlrtSupportedModules::Path) {
            module_builder = module_builder.with_module(path::PathModule);
        }

        if default_modules.contains(&LlrtSupportedModules::PerfHooks) {
            module_builder = module_builder.with_global(perf_hooks::init);
            module_builder = module_builder.with_module(perf_hooks::PerfHooksModule);
        }

        if default_modules.contains(&LlrtSupportedModules::Process) {
            module_builder = module_builder.with_global(process::init);
            module_builder = module_builder.with_module(process::ProcessModule);
        }

        if default_modules.contains(&LlrtSupportedModules::StreamWeb) {
            module_builder = module_builder.with_global(stream_web::init);
            module_builder = module_builder.with_module(stream_web::StreamWebModule);
        }

        if default_modules.contains(&LlrtSupportedModules::StringDecoder) {
            module_builder = module_builder.with_module(string_decoder::StringDecoderModule);
        }

        if default_modules.contains(&LlrtSupportedModules::Timers) {
            module_builder = module_builder.with_global(timers::init);
            module_builder = module_builder.with_module(timers::TimersModule);
        }

        if default_modules.contains(&LlrtSupportedModules::Tty) {
            module_builder = module_builder.with_module(tty::TtyModule);
        }

        if default_modules.contains(&LlrtSupportedModules::Url) {
            module_builder = module_builder.with_global(url::init);
            module_builder = module_builder.with_module(url::UrlModule);
        }

        if default_modules.contains(&LlrtSupportedModules::Util) {
            module_builder = module_builder.with_global(util::init);
            module_builder = module_builder.with_module(util::UtilModule);
        }

        if default_modules.contains(&LlrtSupportedModules::Zlib) {
            module_builder = module_builder.with_module(zlib::ZlibModule);
        }

        Self {
            builder: module_builder,
        }
    }

    pub fn enable_fetch(&mut self) -> &mut Self {
        let builder = std::mem::take(&mut self.builder);
        self.builder = builder.with_global(fetch::init);
        self
    }

    pub fn enable_fs(&mut self) -> &mut Self {
        let builder = std::mem::take(&mut self.builder);
        self.builder = builder
            .with_module(fs::FsPromisesModule)
            .with_module(fs::FsModule);
        self
    }

    pub fn enable_child_process(&mut self) -> &mut Self {
        let builder = std::mem::take(&mut self.builder);
        self.builder = builder.with_module(child_process::ChildProcessModule);
        self
    }
}
