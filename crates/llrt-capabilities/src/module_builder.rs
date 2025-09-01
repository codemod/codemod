use llrt_modules::module_builder::ModuleBuilder;
use llrt_modules::{
    abort, assert, buffer, child_process, console, crypto, events, exceptions, fetch, fs, os, path,
    perf_hooks, process, stream_web, string_decoder, timers, tty, url, util, zlib,
};

pub const UNSAFE_MODULES: &[&str] = &["fetch", "child_process", "fs"];
pub const DEFAULT_MODULES: &[&str] = &[
    "abort",
    "assert",
    "buffer",
    "console",
    "crypto",
    "events",
    "exceptions",
    "os",
    "path",
    "perf_hooks",
    "process",
    "stream_web",
    "string_decoder",
    "timers",
    "tty",
    "url",
    "util",
    "zlib",
];

pub struct LlrtModuleBuilder {
    pub builder: ModuleBuilder,
}

impl LlrtModuleBuilder {
    pub fn build() -> Self {
        let mut module_builder = ModuleBuilder::new();

        if DEFAULT_MODULES.contains(&"abort") {
            module_builder = module_builder.with_global(abort::init);
        }

        if DEFAULT_MODULES.contains(&"assert") {
            module_builder = module_builder.with_module(assert::AssertModule);
        }

        if DEFAULT_MODULES.contains(&"buffer") {
            module_builder = module_builder.with_global(buffer::init);
            module_builder = module_builder.with_module(buffer::BufferModule);
        }

        if DEFAULT_MODULES.contains(&"console") {
            module_builder = module_builder.with_global(console::init);
            module_builder = module_builder.with_module(console::ConsoleModule);
        }

        if DEFAULT_MODULES.contains(&"crypto") {
            module_builder = module_builder.with_global(crypto::init);
            module_builder = module_builder.with_module(crypto::CryptoModule);
        }

        if DEFAULT_MODULES.contains(&"events") {
            module_builder = module_builder.with_global(events::init);
            module_builder = module_builder.with_module(events::EventsModule);
        }

        if DEFAULT_MODULES.contains(&"exceptions") {
            module_builder = module_builder.with_global(exceptions::init);
        }

        if DEFAULT_MODULES.contains(&"os") {
            module_builder = module_builder.with_module(os::OsModule);
        }

        if DEFAULT_MODULES.contains(&"path") {
            module_builder = module_builder.with_module(path::PathModule);
        }

        if DEFAULT_MODULES.contains(&"perf_hooks") {
            module_builder = module_builder.with_global(perf_hooks::init);
            module_builder = module_builder.with_module(perf_hooks::PerfHooksModule);
        }

        if DEFAULT_MODULES.contains(&"process") {
            module_builder = module_builder.with_global(process::init);
            module_builder = module_builder.with_module(process::ProcessModule);
        }

        if DEFAULT_MODULES.contains(&"stream_web") {
            module_builder = module_builder.with_global(stream_web::init);
            module_builder = module_builder.with_module(stream_web::StreamWebModule);
        }

        if DEFAULT_MODULES.contains(&"string_decoder") {
            module_builder = module_builder.with_module(string_decoder::StringDecoderModule);
        }

        if DEFAULT_MODULES.contains(&"timers") {
            module_builder = module_builder.with_global(timers::init);
            module_builder = module_builder.with_module(timers::TimersModule);
        }

        if DEFAULT_MODULES.contains(&"tty") {
            module_builder = module_builder.with_module(tty::TtyModule);
        }

        if DEFAULT_MODULES.contains(&"url") {
            module_builder = module_builder.with_global(url::init);
            module_builder = module_builder.with_module(url::UrlModule);
        }

        if DEFAULT_MODULES.contains(&"util") {
            module_builder = module_builder.with_global(util::init);
            module_builder = module_builder.with_module(util::UtilModule);
        }

        if DEFAULT_MODULES.contains(&"zlib") {
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
