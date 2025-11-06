use std::collections::{HashMap, HashSet};
use std::default::Default;

use anyhow::{anyhow, bail, Result};
use heck::ToKebabCase;
use prost_types::compiler::code_generator_response::File;
use prost_types::{DescriptorProto, ServiceDescriptorProto};
use semver::Version;
use wit_encoder::{
    Field, Ident, Include, Interface, InterfaceItem, NestedPackage, Package, PackageName, Params,
    Record, StandaloneFunc, Type, TypeDef, TypeDefKind, World, WorldItem,
};

use crate::{VIMANA_API_VERSION, WASI_API_VERSION};

/// Name of the generated WIT file in the output directory.
const FILENAME: &str = "server.wit";
/// The Protobuf package is converted into the WIT package *namespace*,
/// but a complete WIT package also has a *name*,
/// and in this case, its always `server`.
const PACKAGE_NAME: &str = "server";
/// The single generated world always has the static name `guest`.
/// Individual services are converted to interfaces in the `services` subpackage.
const WORLD_NAME: &str = "guest";
/// Message types are compiled into an interface of the main package called `types`.
const TYPES_INTERFACE_NAME: &str = "types";

const REQUEST_PARAMETER_NAME: &str = "request";

#[derive(Default)]
pub(crate) struct WitFile {
    services: Vec<Interface>,
    top_level_types: HashSet<(Ident, Ident)>,
    /// Mapping from **Protobuf**-style fully-qualified type names
    /// to **WIT**-style type definitions.
    all_types: HashMap<String, TypeDef>,
}

impl WitFile {
    pub(crate) fn compile_service(
        &mut self,
        service_name: &str,
        service_descriptor: &ServiceDescriptorProto,
    ) -> Result<()> {
        let mut service = Interface::new(service_name.to_kebab_case());

        for method_descriptor in &service_descriptor.method {
            let method_name = method_descriptor
                .name
                .as_ref()
                .ok_or_else(|| anyhow!("Method in service '{service_name}' lacks a name"))?;

            let request_type = method_descriptor.input_type.as_ref().ok_or_else(|| {
                anyhow!("Method '{method_name}' in service '{service_name}' lacks a request type")
            })?;
            let response_type = method_descriptor.output_type.as_ref().ok_or_else(|| {
                anyhow!("Method '{method_name}' in service '{service_name}' lacks a response type")
            })?;

            match method_descriptor.options.as_ref() {
                Some(options) => {
                    for option in &options.uninterpreted_option {
                        // TODO: Interpret method-level options like JSON transcoding.
                    }
                }
                None => (),
            }

            let client_streaming = method_descriptor.client_streaming.unwrap_or(false);
            let server_streaming = method_descriptor.server_streaming.unwrap_or(false);
            if client_streaming || server_streaming {
                //if service.http_routes.len() > 0 {
                //    return Err(error(
                //        "Streaming method { do not support JSON transcoding",
                //    ));
                //}
            }

            let request_type = convert_type_name(request_type);
            let response_type = convert_type_name(response_type);

            let mut function = StandaloneFunc::new(method_name.to_kebab_case(), false);
            function.set_params((REQUEST_PARAMETER_NAME, Type::Named(request_type.1.clone())));
            function.set_result(Some(Type::Named(response_type.1.clone())));
            service.function(function);

            self.top_level_types.insert(request_type);
            self.top_level_types.insert(response_type);
        }

        self.services.push(service);
        Ok(())
    }

    pub(crate) fn compile_message(
        &mut self,
        message_name: &str,
        message_descriptor: &DescriptorProto,
    ) -> Result<()> {
        // TODO: Also insert depended-upon types recursively.
        let (type_interface, type_name) = convert_type_name(message_name);

        self.all_types.insert(
            String::from(message_name),
            TypeDef::new(
                type_name,
                TypeDefKind::Record(convert_type_definition(message_descriptor)),
            ),
        );

        Ok(())
    }

    pub(crate) fn generate(
        self,
        package_name: &str,
        package_version: &Option<Version>,
    ) -> Result<File> {
        let mut world = World::new(WORLD_NAME);

        world.include(Include::new(format!("wasi:cli/imports@{WASI_API_VERSION}")));
        world.include(Include::new(format!(
            "vimana:grpc/imports@{VIMANA_API_VERSION}"
        )));

        for (namespace, name) in self.top_level_types {
            world.use_type(namespace, name, None)
        }

        for service in self.services {
            world.item(WorldItem::InlineInterfaceExport(service));
        }

        let mut types_interface = Interface::new(TYPES_INTERFACE_NAME);
        for (_, type_def) in self.all_types {
            types_interface.item(InterfaceItem::TypeDef(type_def));
        }

        let mut package = Package::new(convert_package_name(package_name, package_version));
        package.world(world);
        package.interface(types_interface);

        Ok(File {
            name: Some(String::from(FILENAME)),
            insertion_point: None,
            content: Some(format!("{package}")),
            // TODO: Add generated code info to help with debugging.
            generated_code_info: None,
        })
    }
}

fn convert_package_name(proto_name: &str, version: &Option<Version>) -> PackageName {
    // TODO: Use proper nested namespaces once that's supported.
    // https://github.com/bytecodealliance/wit-bindgen/issues/1224
    PackageName::new(proto_name.replace('.', ":"), PACKAGE_NAME, version.clone())
}

fn convert_type_name(proto_name: &str) -> (Ident, Ident) {
    let mut parts: Vec<&str> = proto_name.split('.').collect();
    // `unwrap` is safe here because `split` always yields at least 1 element.
    let name = parts.pop().unwrap().to_kebab_case();
    parts.push(PACKAGE_NAME);
    let interface = format!("{}/{TYPES_INTERFACE_NAME}", parts[1..].join(":"));
    (Ident::from(interface), Ident::from(name))
}

fn convert_type_definition(proto_type: &DescriptorProto) -> Record {
    let fields: Vec<Field> = Vec::new();
    Record::new(fields)
}

// fn convert_type(r#type: Type) -> FieldType {
//     match r#type {
//         Type::Double => FieldType::Float64,
//         Type::Float => FieldType::Float32,
//         Type::Int64 => FieldType::S64,
//         Type::Uint64 => FieldType::U64,
//         Type::Int32 => FieldType::S32,
//         Type::Fixed64 => FieldType::S64,
//         Type::Fixed32 => FieldType::S32,
//         Type::Bool => FieldType::Bool,
//         Type::String => FieldType::String,
//         Type::Group => FieldType::Record,   // TODO
//         Type::Message => FieldType::Record, // TODO
//         Type::Bytes => FieldType::List,     // TODO
//         Type::Uint32 => FieldType::U32,
//         Type::Enum => FieldType::Enum, // TODO
//         Type::Sfixed32 => FieldType::S32,
//         Type::Sfixed64 => FieldType::S64,
//         Type::Sint32 => FieldType::S32,
//         Type::Sint64 => FieldType::S64,
//     }
// }
