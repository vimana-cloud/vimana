use std::cell::OnceCell;
use std::collections::{HashMap, HashSet};
use std::default::Default;

use anyhow::{bail, Result};
use heck::ToKebabCase;
use prost_types::compiler::code_generator_response::File;
use prost_types::field_descriptor_proto::{Label, Type as ProtoType};
use prost_types::{DescriptorProto, EnumDescriptorProto, ServiceDescriptorProto};
use wit_encoder::{
    Enum, Field, Ident, Include, Interface, NestedPackage, Package, PackageName, Record,
    StandaloneFunc, Type as WitType, TypeDef as WitTypeDef, TypeDefKind as WitTypeDefKind, World,
    WorldItem,
};

use crate::{
    sorted_map_entries, sorted_set_values, DescriptorMap, ProtoSyntax, QualifiedTypeName,
    TypeNameQualifier, VIMANA_API_VERSION, WASI_API_VERSION,
};

/// Name of the generated WIT file in the output directory.
const FILENAME: &str = "server.wit";
/// WIT has separate concepts of package namespaces and package names.
/// Protobuf only has one such analogous concept; the package.
/// We're choosing to map the Protobuf package concept to the WIT package namespace,
/// but we still need a package name in WIT.
/// Always use the static package name 'proto' for every package generated from Protobuf.
const PACKAGE_NAME: &str = "proto";
/// The single generated world always has the static name `server`.
/// Individual services are converted to inline interfaces in exported by that world.
const WORLD_NAME: &str = "server";
/// Message types are compiled into an interface of the main package called `types`.
const TYPES_INTERFACE_NAME: &str = "types";

const REQUEST_PARAMETER_NAME: &str = "request";

/// An incrementally-built model of a Vimana server WIT file,
/// generated from Protobuf service and type definitions.
#[derive(Default)]
pub(crate) struct WitFile<'a> {
    /// The world that defines the server component.
    server_world: ServerWorld<'a>,
    /// Interfaces under which message and enum types are defined.
    types_interfaces: HashMap<TypeNameQualifier<'a>, TypesInterface<'a>>,
    /// Set to keep track of all types compiled so far,
    /// so we don't compile the same type twice.
    types_compiled: HashSet<QualifiedTypeName<'a>>,
}

/// The compiler always generates a single 'server' world for the component to implement.
#[derive(Default)]
struct ServerWorld<'a> {
    /// WIT-style package namespace
    /// (e.g. `some:package-namespace` for types in the Protobuf package `some.package_namespace`).
    /// Does **not** include the package's name,
    /// which is always simply the value of [`PACKAGE_NAME`].
    package: OnceCell<Vec<&'a str>>,
    /// The set of interfaces exported by this world.
    /// Each interface corresponds to a Protobuf service.
    services: Vec<Interface>,
    /// The set of all types that appear as either requests or responses
    /// in any of the methods of the services in the main package.
    /// These types must be imported into the world with a `use` statement.
    types_used: HashSet<QualifiedTypeName<'a>>,
}

/// All message types are organized by "name qualifiers",
/// which consist of the Protobuf package and, optionally, any outer nesting message names
/// (e.g. `.package.OuterMessage` for type `.package.OuterMessage.InnerMessage`).
/// Each unique qualifier maps to a distinct WIT interface
/// holding the record and enumeration type definitions within that "qualifier namespace".
struct TypesInterface<'a> {
    /// The set records and enumerations defined within the "qualifier namespace".
    types_defined: Vec<WitTypeDef>,
    /// The set of all types that are referenced by types in this interface,
    /// but which belong to a different interface.
    /// These types must be imported into the interface with a `use` statement.
    types_used: HashSet<QualifiedTypeName<'a>>,
}

impl<'a> WitFile<'a> {
    /// Set the package namespace for the server world.
    /// If the namespace has already been set (by compiling a different file),
    /// check that it's consistent with what was previously set.
    ///
    /// This function must be called at least once
    /// before calling either [`compile_service`](Self::compile_service)
    /// or [`compile_message`](Self::compile_message).
    pub(crate) fn set_or_check_server_package(&self, package: &'a str) -> Result<Vec<&'a str>> {
        let package: Vec<&'a str> = package.split('.').collect();
        let existing_package = self.server_world.package.get_or_init(|| package.clone());
        if &package != existing_package {
            bail!("Conflicting packages: {existing_package:?} and {package:?}")
        }
        Ok(package)
    }

    fn server_package(&self) -> &Vec<&'a str> {
        self.server_world.package.get().unwrap()
    }

    fn server_package_qualifier(&self) -> TypeNameQualifier<'a> {
        TypeNameQualifier::top_level(self.server_package().clone())
    }

    fn server_package_name(&self) -> PackageName {
        PackageName::new(
            self.server_package()
                .iter()
                .map(|part| part.to_kebab_case())
                .collect::<Vec<_>>()
                .join(":"),
            Ident::from(PACKAGE_NAME),
            None,
        )
    }

    //fn server_package_qualifier(&self) -> NameQualifier {
    //    NameQualifier {
    //        package_namespace: String::from(self.server_package_namespace()),
    //        package_name: Ident::from(PACKAGE_NAME),
    //    }
    //}

    pub(crate) fn compile_service(
        &mut self,
        service_descriptor: &'a ServiceDescriptorProto,
    ) -> Result<()> {
        let mut service = Interface::new(service_descriptor.name().to_kebab_case());

        for method_descriptor in &service_descriptor.method {
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

            let request_type =
                QualifiedTypeName::from_path(method_descriptor.input_type(), self.server_package());
            let response_type = QualifiedTypeName::from_path(
                method_descriptor.output_type(),
                self.server_package(),
            );

            let mut function = StandaloneFunc::new(method_descriptor.name().to_kebab_case(), false);
            function.set_params((
                REQUEST_PARAMETER_NAME,
                WitType::Named(Ident::from(request_type.name.to_kebab_case())),
            ));
            function.set_result(Some(WitType::Named(Ident::from(
                response_type.name.to_kebab_case(),
            ))));
            service.function(function);

            self.server_world.types_used.insert(request_type);
            self.server_world.types_used.insert(response_type);
        }

        self.server_world.services.push(service);
        Ok(())
    }

    pub(crate) fn compile_message(
        &mut self,
        message_descriptor: &'a DescriptorProto,
        qualifier: &TypeNameQualifier<'a>,
        syntax: ProtoSyntax,
        descriptors: &DescriptorMap<'a>,
    ) -> Result<()> {
        let type_name = qualifier.r#type(message_descriptor.name());
        if !self.types_compiled.contains(&type_name) {
            self.types_compiled.insert(type_name.clone());

            let (type_definition, types_used) =
                self.message_type_definition(message_descriptor, type_name.name, syntax)?;

            for type_used in &types_used {
                // Check if it's a message type first
                if let Some((depended_descriptor, depended_syntax)) =
                    descriptors.get_message(type_used)
                {
                    // Recursively compile message dependencies
                    self.compile_message(
                        depended_descriptor,
                        &type_used.qualifier,
                        depended_syntax,
                        descriptors,
                    )?;
                } else if let Some(enum_descriptor) = descriptors.get_enum(type_used) {
                    self.compile_enum(enum_descriptor, &type_used.qualifier);
                } else {
                    bail!("Type not found: {type_used}");
                }
            }

            self.upsert_type_definition(type_name.qualifier, type_definition, types_used);
        }

        Ok(())
    }

    fn compile_enum(
        &mut self,
        enum_descriptor: &'a EnumDescriptorProto,
        qualifier: &TypeNameQualifier<'a>,
    ) {
        let type_name = qualifier.r#type(enum_descriptor.name());
        if !self.types_compiled.contains(&type_name) {
            self.types_compiled.insert(type_name.clone());

            let type_definition = self.enum_type_definition(enum_descriptor, type_name.name);
            self.upsert_type_definition(type_name.qualifier, type_definition, Vec::new());
        }
    }

    fn upsert_type_definition(
        &mut self,
        qualifier: TypeNameQualifier<'a>,
        type_definition: WitTypeDef,
        types_used: Vec<QualifiedTypeName<'a>>,
    ) {
        match self.types_interfaces.get_mut(&qualifier) {
            Some(types_interface) => {
                types_interface.types_defined.push(type_definition);
                types_interface.types_used.extend(types_used);
            }
            None => {
                self.types_interfaces.insert(
                    qualifier,
                    TypesInterface {
                        types_defined: vec![type_definition],
                        types_used: types_used.into_iter().collect(),
                    },
                );
            }
        }
    }

    pub(crate) fn generate(mut self) -> Result<File> {
        let mut wit_contents = String::new();

        let mut server_package = Package::new(self.server_package_name());
        let server_package_qualifier = self.server_package_qualifier();
        server_package.world(self.server_world.into_world());
        if let Some(server_package_types_interface) =
            self.types_interfaces.remove(&server_package_qualifier)
        {
            server_package.interface(server_package_types_interface.into_interface());
        }
        wit_contents.push_str(server_package.to_string().as_str());

        for (name_qualifier, types_interface) in sorted_map_entries(self.types_interfaces) {
            wit_contents.push('\n');
            wit_contents.push_str(
                types_interface
                    .into_nested_package(name_qualifier)
                    .to_string()
                    .as_str(),
            );
        }

        Ok(File {
            name: Some(String::from(FILENAME)),
            insertion_point: None,
            content: Some(wit_contents),
            // TODO: Add generated code info to help with debugging.
            generated_code_info: None,
        })
    }

    fn message_type_definition(
        &self,
        descriptor: &'a DescriptorProto,
        name: &'a str,
        syntax: ProtoSyntax,
    ) -> Result<(WitTypeDef, Vec<QualifiedTypeName<'a>>)> {
        let mut wit_fields: Vec<Field> = Vec::with_capacity(descriptor.field.len());
        let mut types_used: Vec<QualifiedTypeName> = Vec::new();
        for proto_field in &descriptor.field {
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
                ProtoType::Message | ProtoType::Enum => {
                    let type_name = QualifiedTypeName::from_path(
                        proto_field.type_name(),
                        self.server_package(),
                    );
                    let wit_short_name = type_name.name.to_kebab_case();
                    types_used.push(type_name);
                    WitType::named(wit_short_name)
                }
                ProtoType::Bytes => WitType::list(WitType::U8),
                ProtoType::Uint32 => WitType::U32,
                ProtoType::Sfixed32 => WitType::S32,
                ProtoType::Sfixed64 => WitType::S64,
                ProtoType::Sint32 => WitType::S32,
                ProtoType::Sint64 => WitType::S64,
                ProtoType::Group => {
                    bail!("Protobuf groups are not supported; use nested messages instead")
                }
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
        Ok((
            WitTypeDef::new(
                name.to_kebab_case(),
                WitTypeDefKind::Record(Record::new(wit_fields)),
            ),
            types_used,
        ))
    }

    fn enum_type_definition(
        &self,
        enum_descriptor: &'a EnumDescriptorProto,
        name: &'a str,
    ) -> WitTypeDef {
        let mut wit_enum = Enum::empty();
        for variant in &enum_descriptor.value {
            wit_enum.case(variant.name().to_kebab_case());
        }
        WitTypeDef::new(name.to_kebab_case(), WitTypeDefKind::Enum(wit_enum))
    }
}

impl<'a> ServerWorld<'a> {
    fn into_world(self) -> World {
        let mut world = World::new(WORLD_NAME);
        world.include(Include::new(format!("wasi:cli/imports@{WASI_API_VERSION}")));
        world.include(Include::new(format!(
            "vimana:grpc/imports@{VIMANA_API_VERSION}"
        )));
        for used_type in sorted_set_values(self.types_used) {
            world.use_type(used_type.use_type_target(), used_type.use_type_item(), None);
        }
        for service in self.services {
            world.item(WorldItem::InlineInterfaceExport(service));
        }
        world
    }
}

impl<'a> TypesInterface<'a> {
    fn into_interface(self) -> Interface {
        let mut interface = Interface::new(TYPES_INTERFACE_NAME);
        for used_type in sorted_set_values(self.types_used) {
            interface.use_type(used_type.use_type_target(), used_type.use_type_item(), None);
        }
        for defined_type in self.types_defined {
            interface.type_def(defined_type);
        }
        interface
    }

    fn into_nested_package(self, qualifier: TypeNameQualifier<'a>) -> NestedPackage {
        let mut package = NestedPackage::new(qualifier.into_namespaced_package_name());
        package.interface(self.into_interface());
        package
    }
}

impl<'a> QualifiedTypeName<'a> {
    fn use_type_target(&self) -> Ident {
        Ident::from(format!(
            "{}:{}/{TYPES_INTERFACE_NAME}",
            self.qualifier.package_namespace(),
            self.qualifier.package_name()
        ))
    }
    fn use_type_item(&self) -> Ident {
        Ident::from(self.name.to_kebab_case())
    }
}

impl<'a> TypeNameQualifier<'a> {
    fn into_namespaced_package_name(self) -> PackageName {
        PackageName::new(self.package_namespace(), self.package_name(), None)
    }

    fn package_namespace(&self) -> String {
        self.package
            .iter()
            .map(|part| part.to_kebab_case())
            .collect::<Vec<_>>()
            .join(":")
    }

    fn package_name(&self) -> String {
        let mut package_name = String::from(PACKAGE_NAME);
        for outer_message in &self.outer_messages {
            package_name.push('/');
            package_name.push_str(outer_message.to_kebab_case().as_str());
        }
        package_name
    }
}
