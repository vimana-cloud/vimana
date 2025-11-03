//! The compilation step involves consolidating TODO

use std::cell::OnceCell;
use std::collections::HashMap;
use std::fmt::{Debug, Display, Formatter, Result as FmtResult, Write};
use std::ops::{Deref, DerefMut};

use anyhow::{anyhow, bail, Error, Result};
use heck::ToKebabCase;
use prost_types::compiler::code_generator_response::File;
use prost_types::compiler::{CodeGeneratorRequest, CodeGeneratorResponse};
use prost_types::field_descriptor_proto::{Label, Type};
use prost_types::{DescriptorProto, FieldDescriptorProto, FileDescriptorProto};
use semver::Version;
use wit_encoder::{
    Include, Interface, Package, PackageName, Render, World, WorldItem, WorldNamedInterface,
};

use metadata_proto::work::runtime::field::{Coding, CompoundCoding, ScalarCoding};
use metadata_proto::work::runtime::Field;

/// Name of the generated WIT file in the output directory.
const WIT_FILENAME: &str = "server.wit";
/// The single generated world always has the static name 'server'.
/// Individual services are converted to interfaces.
const SERVER_WORLD_NAME: &str = "server";
/// Version of the Vimana API to import.
const VIMANA_API_VERSION: &str = "0.0.0";
/// Version of the WASI API to import.
const WASI_API_VERSION: &str = "0.2.0";

#[derive(Copy, Clone)]
enum ProtoSyntax {
    Proto2,
    Proto3,
    Editions,
}

// TODO: Upstream this.
//   https://github.com/bytecodealliance/wasm-tools/issues/2270
struct NestedPackage(Package);

// Temporary crutch to render a nested package
// based on the non-nested rendering of the package.
// TODO: Delete this once `NestedPackage` is upstreamed.
impl Display for NestedPackage {
    fn fmt(&self, f: &mut Formatter<'_>) -> FmtResult {
        let normal_package_string = self.0.to_string();
        let mut lines = normal_package_string.split('\n');

        write!(f, "package {} {{\n", self.0.name())?;
        for line in lines.skip(1) {
            if line.len() > 0 {
                write!(f, "  {}\n", line)?;
            } else {
                f.write_char('\n')?;
            }
        }
        f.write_str("}\n")
    }
}

impl NestedPackage {
    /// Create a new instance of `NestedPackage`.
    pub fn new(name: PackageName) -> Self {
        Self(Package::new(name))
    }
}

impl Deref for NestedPackage {
    type Target = Package;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl DerefMut for NestedPackage {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

pub(crate) fn compile(request: CodeGeneratorRequest) -> Result<File> {
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
    let mut package_name: OnceCell<String> = OnceCell::new();
    // Version of the main package.
    let mut package_version: OnceCell<Version> = OnceCell::new();

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
        let package_name = package_name.get_or_init(|| file_package_name.clone());
        if file_package_name != package_name {
            bail!("Conflicting packages: '{package_name:?}' and '{file_package_name:?}'")
        }

        //for service_descriptor in &file_descriptor.service {
        //    let service_name = service_descriptor
        //        .name
        //        .clone()
        //        .ok_or(Error::leaf("Service has empty name"))?;
        //    let mut streaming: bool = false;

        //    for method_descriptor in &service_descriptor.method {
        //        let request_type = method_descriptor.input_type.as_ref().unwrap();
        //        let response_type = method_descriptor.output_type.as_ref().unwrap();

        //        let client_streaming = method_descriptor.client_streaming.unwrap_or(false);
        //        let server_streaming = method_descriptor.server_streaming.unwrap_or(false);
        //        if client_streaming || server_streaming {
        //            streaming = true;
        //        }

        //        match method_descriptor.options.as_ref() {
        //            Some(options) => {
        //                for option in &options.uninterpreted_option {
        //                    // TODO
        //                }
        //            }
        //            None => (),
        //        }

        //        if streaming && service.http_routes.len() > 0 {
        //            return Err(error(
        //                "Service cannot include both streaming RPCs and JSON transcoding",
        //            ));
        //        }

        //        service.type_imports.push(String::from(response_type));
        //        insert_types_recursively(&mut descriptors, request_type, package)?;
        //        insert_types_recursively(&mut descriptors, response_type, package)?;
        //    }
        //    package.services.push(service);
        //}
    }

    let package_name = convert_package_name(
        package_name
            .get()
            .ok_or_else(|| anyhow!("No package specified"))?,
        package_version.take(),
    )?;
    let service_package_name = "TODO";
    let mut package = Package::new(package_name.clone());
    let mut world = World::new(SERVER_WORLD_NAME);
    world.include(Include::new(format!("wasi:cli/imports@{WASI_API_VERSION}")));
    world.include(Include::new(format!(
        "vimana:grpc/imports@{VIMANA_API_VERSION}"
    )));
    package.world(world);

    let service_package = NestedPackage::new(package_name);
    let types_package = "TODO";

    Ok(File {
        name: Some(String::from(WIT_FILENAME)),
        insertion_point: None,
        content: Some(format!("{package}\n{service_package}\n{types_package}")),
        // TODO: Add generated code info to help with debugging.
        generated_code_info: None,
    })
}

fn convert_package_name(proto_name: &str, version: Option<Version>) -> Result<PackageName> {
    let mut parts: Vec<&str> = proto_name.split('.').collect();
    if parts.len() < 2 {
        bail!("Package name '{proto_name}' must contain at least one namespace");
    }
    let name = String::from(parts.pop().unwrap());
    Ok(PackageName::new(parts.join(":"), name, version))
}

fn sub_namespace(base: PackageName) {}

/// Convert a protobuf short name to a WIT short name.
/// This mainly involves converting `snake_case` to `kebab-case`.
fn proto_name_to_wit_name(proto_name: &String) -> String {
    // TODO: Think about edge cases.
    proto_name.replace("_", "-")
}
