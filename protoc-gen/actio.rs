use std::collections::HashMap;

use prost_types::compiler::{CodeGeneratorRequest, CodeGeneratorResponse};
use prost_types::{DescriptorProto, FileDescriptorProto};

use common::run_protoc_plugin;
use error::{Error, Result};

fn main() -> Result<()> {
    return run_protoc_plugin(compile, emit);
}

fn compile(request: CodeGeneratorRequest) -> Result<Vec<()>> {
    // Mapping from all filenames to file descriptors.
    let mut file_descriptors: HashMap<String, &FileDescriptorProto> = HashMap::new();
    // Mapping from all fully-qualified message type names to message descriptors.
    let mut descriptors: HashMap<String, &DescriptorProto> = HashMap::new();
    for proto_file in &request.proto_file {
        for message_type in &proto_file.message_type {
            descriptors.insert(
                message_type.name.clone().ok_or(Error::leaf("TODO"))?,
                message_type,
            );
        }
        file_descriptors.insert(
            proto_file.name.clone().ok_or(Error::leaf("TODO"))?,
            proto_file,
        );
    }

    // Process each file.
    // One implementation config is generated per service.
    for file_to_generate in &request.file_to_generate {
        let file_descriptor = file_descriptors.get(file_to_generate).unwrap();
        let package_name = file_descriptor.package.as_ref().unwrap();

        for service_descriptor in &file_descriptor.service {
            let service_name = service_descriptor
                .name
                .clone()
                .ok_or(Error::leaf("Service has empty name"))?;
            let mut streaming: bool = false;

            for method_descriptor in &service_descriptor.method {
                let request_type = method_descriptor.input_type.as_ref().unwrap();
                let response_type = method_descriptor.output_type.as_ref().unwrap();

                let client_streaming = method_descriptor.client_streaming.unwrap_or(false);
                let server_streaming = method_descriptor.server_streaming.unwrap_or(false);
                if client_streaming || server_streaming {
                    streaming = true;
                }

                match method_descriptor.options.as_ref() {
                    Some(options) => {
                        for option in &options.uninterpreted_option {
                            // TODO
                        }
                    }
                    None => (),
                }

                if streaming && service.http_routes.len() > 0 {
                    return Err(error(
                        "Service cannot include both streaming RPCs and JSON transcoding.",
                    ));
                }

                service.type_imports.push(String::from(response_type));
                insert_types_recursively(&mut descriptors, request_type, package)?;
                insert_types_recursively(&mut descriptors, response_type, package)?;
            }
            package.services.push(service);
        }
    }

    Ok(Vec::new())
}

fn emit(_wit: Vec<()>) -> Result<CodeGeneratorResponse> {
    Ok(CodeGeneratorResponse::default())
}
