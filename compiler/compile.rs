//! The compilation step involves consolidating TODO

use std::cell::OnceCell;
use std::collections::HashMap;
use anyhow::{anyhow, bail, Result};
use prost_types::compiler::code_generator_response::File;
use prost_types::compiler::CodeGeneratorRequest;
use prost_types::FileDescriptorProto;
use semver::Version;

use crate::metadata::MetadataFile;
use crate::wit::WitFile;
use metadata_proto::work::runtime::Field;

#[derive(Copy, Clone)]
enum ProtoSyntax {
    Proto2,
    Proto3,
    Editions,
}

pub(crate) fn compile(request: CodeGeneratorRequest) -> Result<Vec<File>> {
    // Mapping from all filenames to file descriptors.
    let mut file_descriptors: HashMap<String, &FileDescriptorProto> = HashMap::new();
    // Mapping from all fully-qualified message type names to resolved message types.
    let mut compiled_messages: HashMap<String, Field> = HashMap::new();

    for proto_file in &request.proto_file {
        let file_name = proto_file
            .name
            .clone()
            .ok_or_else(|| anyhow!("Proto file lacks a name"))?;

        let syntax = match proto_file.syntax.as_ref().map(String::as_str) {
            None | Some("proto2") => ProtoSyntax::Proto2,
            Some("proto3") => ProtoSyntax::Proto3,
            Some("editions") => bail!("Editions syntax is not yet supported"),
            Some(syntax) => bail!("Unknown syntax '{syntax}' in '{file_name}'"),
        };

        for message_type in &proto_file.message_type {
            let message_name = message_type
                .name
                .clone()
                .ok_or_else(|| anyhow!("Message in '{file_name}' lacks a name"))?;
            //let subfields =
            //    compile_message(&message_name, message_type, syntax, &compiled_messages)?;
            //compiled_messages.insert(
            //    message_name,
            //    Field {
            //        number: 0,               // Ignored.
            //        name: String::default(), // Ignored.
            //        subfields,
            //        // Coding ignored for top-level messages.
            //        coding: None,
            //    },
            //);
        }

        file_descriptors.insert(file_name, proto_file);
    }

    // Main package, in Protobuf syntax.
    // All proto files within `file_to_generate` must be part of the same package.
    let package_name: OnceCell<String> = OnceCell::new();
    // Version of the main package.
    let mut package_version: OnceCell<Version> = OnceCell::new();

    let mut wit_file: WitFile = WitFile::default();
    let mut metadata_file: MetadataFile = MetadataFile::default();

    // One implementation config is generated per service.
    for file_to_generate in &request.file_to_generate {
        let file_descriptor = file_descriptors.get(file_to_generate).ok_or_else(|| {
            anyhow!("Malformed request contains unknown file '{file_to_generate}")
        })?;

        // Check that the file's package name is consistent with all the other source files.
        let file_package_name = file_descriptor
            .package
            .as_ref()
            .ok_or_else(|| anyhow!("Proto file '{file_to_generate}' lacks a package"))?;
        let package_name_value = package_name.get_or_init(|| file_package_name.clone());
        if file_package_name != package_name_value {
            bail!("Conflicting packages: '{package_name_value:?}' and '{file_package_name:?}'")
        }

        // Compile all the services in this file.
        for service_descriptor in &file_descriptor.service {
            let service_name = service_descriptor
                .name
                .as_ref()
                .ok_or_else(|| anyhow!("Service in '{file_to_generate}' lacks a name"))?;

            wit_file.compile_service(service_name, service_descriptor)?;
        }

        // Compile all the types in this file.
        for message_descriptor in &file_descriptor.message_type {
            let message_name = message_descriptor
                .name
                .as_ref()
                .ok_or_else(|| anyhow!("Message in '{file_to_generate}' lacks a name"))?;

            wit_file.compile_message(message_name, message_descriptor)?;
        }
    }

    let package_name = package_name
        .get()
        .ok_or_else(|| anyhow!("No package specified"))?;
    let package_version = package_version.take();

    Ok(vec![
        wit_file.generate(package_name, &package_version)?,
        metadata_file.generate(package_name, &package_version)?,
    ])
}
