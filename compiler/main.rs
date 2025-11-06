mod compile;
mod metadata;
mod wit;

use std::io::{stdin, stdout, Read, Write};

use anyhow::Result;
use prost::Message;
use prost_types::compiler::code_generator_response::Feature;
use prost_types::compiler::{CodeGeneratorRequest, CodeGeneratorResponse};

use compile::compile;

/// Version of the Vimana API to import.
pub(crate) const VIMANA_API_VERSION: &str = "0.0.0";
/// Version of the WASI API to import.
pub(crate) const WASI_API_VERSION: &str = "0.2.0";
/// Bitwise union of supported features.
/// https://github.com/protocolbuffers/protobuf/blob/v31.1/src/google/protobuf/compiler/code_generator.h#L96
const SUPPORTED_FEATURES: u64 = Feature::Proto3Optional as u64;

fn emit(_wit: Vec<()>) -> Result<CodeGeneratorResponse> {
    Ok(CodeGeneratorResponse {
        file: vec![],
        error: None,
        supported_features: Some(SUPPORTED_FEATURES),
    })
}

fn main() -> Result<()> {
    // Read and parse the entire input from stdin.
    // If an error occurs here, exit with a failure status.
    let mut buf: Vec<u8> = Vec::new();
    stdin().read_to_end(&mut buf)?;
    let request: CodeGeneratorRequest = CodeGeneratorRequest::decode(buf.as_slice())?;

    // Generate a response.
    // If an error occurs after this point,
    // write it as an error on the generated response, but exit normally.
    let mut response = CodeGeneratorResponse {
        file: Vec::new(),
        error: None,
        supported_features: Some(SUPPORTED_FEATURES),
    };
    match compile(request) {
        Ok(files) => response.file.extend(files),
        Err(error) => response.error = Some(error.to_string()),
    }

    // Write the response to stdout.
    return Ok(stdout().write_all(response.encode_to_vec().as_slice())?);
}
