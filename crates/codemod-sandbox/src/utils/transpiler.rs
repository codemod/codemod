use std::sync::Arc;
use swc_core::common::{errors::Handler, SourceMap};
use swc_ts_fast_strip::{Mode, Options};
struct NullEmitter;
impl swc_core::common::errors::Emitter for NullEmitter {
    fn emit(&mut self, _db: &mut swc_core::common::errors::DiagnosticBuilder<'_>) {}
}

#[allow(dead_code)]
pub fn transpile(source: String, filename: String) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let cm: Arc<SourceMap> = Default::default();

    let handler = Handler::with_emitter(false, false, Box::new(NullEmitter));

    let code = swc_ts_fast_strip::operate(
        &cm,
        &handler,
        source,
        Options {
            filename: Some(filename),
            mode: Mode::StripOnly,
            source_map: false,
            ..Default::default()
        },
    )?
    .code
    .into_bytes();

    Ok(code)
}

// TODO(mohebifar): I tried replacing swc with oxc and it works, but it doesn't support type stripping which leads to wrong positions for tracing the source code.

/*
use std::io::Error;
use std::path::Path;

use oxc::allocator::Allocator;
use oxc::codegen::{Codegen, CodegenOptions};
use oxc::parser::Parser;
use oxc::semantic::SemanticBuilder;
use oxc::span::SourceType;
use oxc::transformer::{TransformOptions, Transformer, TypeScriptOptions};

#[allow(dead_code)]
pub fn transpile(source: String, filename: String) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let allocator = Allocator::default();

    let source_type = SourceType::ts();

    let parser = Parser::new(&allocator, &source, source_type);
    let ret = parser.parse();

    let mut program = ret.program;

    let scoping = SemanticBuilder::new()
        .build(&program)
        .semantic
        .into_scoping();

    let transform_options = TransformOptions {
        typescript: TypeScriptOptions::default(),
        ..TransformOptions::default()
    };
    let transformer = Transformer::new(&allocator, Path::new(&filename), &transform_options);
    let transformer_return = transformer.build_with_scoping(scoping, &mut program);

    if !transformer_return.errors.is_empty() {
        return Err(Box::new(Error::other("Transformer errors")));
    }

    let output = Codegen::new()
        .with_options(CodegenOptions {
            minify: false,
            ..CodegenOptions::default()
        })
        .build(&program);

    Ok(output.code.into_bytes())
}
*/
