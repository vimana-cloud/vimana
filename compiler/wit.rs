use std::collections::{HashMap, HashSet};
use std::default::Default;

use anyhow::{anyhow, bail, Result};
use heck::ToKebabCase;
use prost_types::compiler::code_generator_response::File;
use prost_types::field_descriptor_proto::{Label, Type as ProtoType};
use prost_types::{DescriptorProto, FieldDescriptorProto, ServiceDescriptorProto};
use semver::Version;
use wit_encoder::{
    Field, Ident, Include, Interface, InterfaceItem, NestedPackage, Package, PackageName, Params,
    Record, StandaloneFunc, Type as WitType, TypeDef as WitTypeDef, TypeDefKind as WitTypeDefKind,
    World, WorldItem,
};

use crate::compile::ProtoSyntax;
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

/// An incrementally-built model of a WIT file
/// generated from Protobuf service and type definitions.
#[derive(Default)]
pub(crate) struct WitFile {
    /// The set of services in the main package.
    services: Vec<Interface>,
    /// The set of all types that appear as either requests or responses
    /// in any of the methods of the services in the main package.
    top_level_types: HashSet<FullyQualifiedTypeName>,
    /// Mapping from **Protobuf**-style fully-qualified type names
    /// to **WIT**-style type definitions.
    all_types: HashMap<String, FullyQualifiedTypeDefinition>,
}

/// A WIT-style fully-qualified type name.
#[derive(Eq, PartialEq, Hash)]
struct FullyQualifiedTypeName {
    /// Fully qualified interface name (e.g. `some:package:namespace:name/interface-name`).
    /// If empty, the main package's type interface is assumed
    interface: Option<Ident>,
    /// Simple name of the type within the interface (e.g. `some-type`).
    name: Ident,
}

/// A WIT type definition that includes a fully-qualified interface name.
struct FullyQualifiedTypeDefinition {
    /// Fully qualified interface name (e.g. `some:package:namespace:name/interface-name`).
    interface: Option<Ident>,
    /// Type definition (which includes the type's simple name, e.g. `some-type`).
    type_def: WitTypeDef,
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
            function.set_params((
                REQUEST_PARAMETER_NAME,
                WitType::Named(request_type.name.clone()),
            ));
            function.set_result(Some(WitType::Named(response_type.name.clone())));
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
        syntax: ProtoSyntax,
        all_messages: &HashMap<String, &DescriptorProto>,
    ) -> Result<()> {
        // TODO: Also insert depended-upon types recursively.
        eprintln!("Compiling {}", message_name);
        let type_name = convert_type_name(message_name);

        self.all_types.insert(
            String::from(message_name),
            FullyQualifiedTypeDefinition {
                interface: type_name.interface,
                type_def: WitTypeDef::new(
                    type_name.name,
                    WitTypeDefKind::Record(convert_type_definition(message_descriptor, syntax)?),
                ),
            },
        );

        Ok(())
    }

    pub(crate) fn generate(
        self,
        package_name: &str,
        package_version: &Option<Version>,
    ) -> Result<File> {
        let package_name = convert_package_name(package_name, package_version);
        let default_types_interface = Ident::from(format!("{package_name}/{TYPES_INTERFACE_NAME}"));

        let mut world = World::new(WORLD_NAME);
        world.include(Include::new(format!("wasi:cli/imports@{WASI_API_VERSION}")));
        world.include(Include::new(format!(
            "vimana:grpc/imports@{VIMANA_API_VERSION}"
        )));
        for type_name in self.top_level_types {
            world.use_type(
                type_name
                    .interface
                    .unwrap_or_else(|| default_types_interface.clone()),
                type_name.name,
                None,
            )
        }
        for service in self.services {
            world.item(WorldItem::InlineInterfaceExport(service));
        }

        let mut interfaces: HashMap<Ident, Interface> = HashMap::new();
        for (_, fq_type_def) in self.all_types {
            let interface_name = match &fq_type_def.interface {
                Some(interface_name) => interface_name,
                None => &default_types_interface,
            };
            let interface = match interfaces.get_mut(interface_name) {
                Some(interface) => interface,
                None => {
                    interfaces.insert(
                        interface_name.clone(),
                        Interface::new(interface_name.clone()),
                    );
                    // Unwrap is safe here because the value was just inserted.
                    interfaces.get_mut(interface_name).unwrap()
                }
            };
            interface.item(InterfaceItem::TypeDef(fq_type_def.type_def));
        }

        let mut package = Package::new(package_name);
        package.world(world);
        for (_, interface) in interfaces {
            package.interface(interface);
        }

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

fn convert_type_name(proto_name: &str) -> FullyQualifiedTypeName {
    let mut parts: Vec<&str> = proto_name.split('.').collect();
    // `unwrap` is safe here because `split` always yields at least 1 element.
    let name = Ident::from(parts.pop().unwrap().to_kebab_case());
    // Furthermore, message names always start with a period,
    // (so an *unqualified* name with no package would look like `.MessageName`).
    // These can be identified as having only a single remaining (empty) part
    // after popping the name.
    let interface = if parts.len() <= 1 {
        // The main package's type interface should be used by default.
        None
    } else {
        Some(Ident::from(format!(
            "{}:{PACKAGE_NAME}/{TYPES_INTERFACE_NAME}",
            // Ignore the first (empty) part due to the leading period.
            parts[1..].join(":")
        )))
    };
    FullyQualifiedTypeName { interface, name }
}

fn convert_type_definition(proto_type: &DescriptorProto, syntax: ProtoSyntax) -> Result<Record> {
    let mut wit_fields: Vec<Field> = Vec::with_capacity(proto_type.field.len());
    for proto_field in &proto_type.field {
        let mut wit_type = match proto_field.r#type() {
            ProtoType::Double => WitType::F64,
            ProtoType::Float => WitType::F32,
            ProtoType::Int64 => WitType::S64,
            ProtoType::Uint64 => WitType::U64,
            ProtoType::Int32 => WitType::S32,
            ProtoType::Fixed64 => WitType::U64,
            ProtoType::Fixed32 => WitType::U32,
            ProtoType::Bool => WitType::Bool,
            ProtoType::String => WitType::String,
            ProtoType::Group => {
                bail!("Protobuf groups are not supported; use nested messages instead")
            }
            ProtoType::Message => todo!(),
            ProtoType::Bytes => WitType::list(WitType::U8),
            ProtoType::Uint32 => WitType::U32,
            ProtoType::Enum => todo!(),
            ProtoType::Sfixed32 => WitType::S32,
            ProtoType::Sfixed64 => WitType::S64,
            ProtoType::Sint32 => WitType::S32,
            ProtoType::Sint64 => WitType::S64,
        };
        wit_type = match proto_field.label() {
            Label::Optional => {
                if syntax == ProtoSyntax::Proto2 || proto_field.proto3_optional() {
                    WitType::option(wit_type)
                } else {
                    wit_type
                }
            }
            Label::Required => {
                // YAGNI (this is proto2-only syntax that's highly discouraged).
                bail!("Required fields are not supported");
            }
            Label::Repeated => WitType::list(wit_type),
        };
        wit_fields.push(Field::new(proto_field.name().to_kebab_case(), wit_type));
    }
    Ok(Record::new(wit_fields))
}
