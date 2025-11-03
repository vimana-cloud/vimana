#![feature(iter_intersperse, ascii_char)]

use std::io::{stdin, stdout, Error, ErrorKind, Read, Result, Write};

use prost::Message;
use prost_types::compiler::{CodeGeneratorRequest, CodeGeneratorResponse};

// Generic boilerplate for implementing a protobuf plugin.
pub fn run_protoc_plugin<T>(
    compile: fn(CodeGeneratorRequest) -> Result<T>,
    emit: fn(T) -> Result<CodeGeneratorResponse>,
) -> Result<()> {
    // Read and parse the entire input from stdin.
    let mut buf: Vec<u8> = Vec::new();
    stdin().read_to_end(&mut buf)?;
    let request: CodeGeneratorRequest = CodeGeneratorRequest::decode(buf.as_slice())?;

    // Generate a response. If an error occurs during this step,
    // write it as an error on the generated response, rather than panicking.
    let response: CodeGeneratorResponse =
        compile(request)
            .and_then(emit)
            .unwrap_or_else(|error: Error| CodeGeneratorResponse {
                file: Vec::new(),
                error: Some(error.to_string()),
                supported_features: None,
            });

    // Write the response to stdout.
    return stdout().write_all(response.encode_to_vec().as_slice());
}

pub fn required<T>(option: Option<T>) -> Result<T> {
    option.ok_or(error("Missing required field"))
}

pub fn error(message: &str) -> Error {
    Error::new(ErrorKind::Other, message)
}
