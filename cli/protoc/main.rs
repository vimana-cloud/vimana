mod compile;
mod wit;

use std::io::{stdin, stdout, Read, Write};

use anyhow::{Error, Result};
use prost::Message;
use prost_types::compiler::code_generator_response::{Feature, File};
use prost_types::compiler::{CodeGeneratorRequest, CodeGeneratorResponse};
use prost_types::{DescriptorProto, FileDescriptorProto};

use compile::compile;

/// Name of the generated WIT file in the output directory.
const WIT_FILENAME: &str = "server.wit";

/// Bitwise union of supported features.
/// https://github.com/protocolbuffers/protobuf/blob/v31.1/src/google/protobuf/compiler/code_generator.h#L96
const SUPPORTED_FEATURES: u64 = Feature::Proto3Optional as u64;

fn emit(_wit: Vec<()>) -> Result<CodeGeneratorResponse> {
    Ok(CodeGeneratorResponse {
        file: vec![File {
            name: Some(String::from(WIT_FILENAME)),
            insertion_point: None,
            content: Some(String::from("How witty\n")),
            // TODO: Add generated code info to help with debugging.
            generated_code_info: None,
        }],
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
    let response: CodeGeneratorResponse =
        compile(request)
            .and_then(emit)
            .unwrap_or_else(|error: Error| CodeGeneratorResponse {
                file: Vec::new(),
                error: Some(error.to_string()),
                supported_features: Some(SUPPORTED_FEATURES),
            });

    // Write the response to stdout.
    return Ok(stdout().write_all(response.encode_to_vec().as_slice())?);
}
