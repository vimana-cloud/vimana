use std::collections::{HashMap, HashSet};
use std::default::Default;

use anyhow::{Result, bail};
use heck::ToKebabCase;
use prost_types::compiler::code_generator_response::File;
use prost_types::field_descriptor_proto::{Label, Type as ProtoType};
use prost_types::{DescriptorProto, EnumDescriptorProto, ServiceDescriptorProto};
use wit_encoder::{
    Enum, Field, Ident, Interface, NestedPackage, Package, PackageName, Params, Record,
    StandaloneFunc, Type, TypeDef, TypeDefKind, Variant, VariantCase, World, WorldItem,
};

use crate::{
    DescriptorMap, PluginParameters, ProtoPackage, ProtoSyntax, QualifiedTypeName,
    TypeNameQualifier, into_sorted_map_entries, into_sorted_set_values, sorted_set_values,
};

/// Subdirectory within the output directory in which to generate the WIT package.
/// This package will have the standard WIT package directory layout,
/// with a single source wit file,
/// and a `deps/` directory containing the Vimana-specific dependencies,
/// which are hardcoded into the compiler binary.
const DIRECTORY: &str = "wit";
/// Static contents of Vimana's gRPC dependency WIT file.
const DEPENDENCY_GRPC: &str = include_str!("wit/grpc/grpc.wit");

/// WIT has separate concepts of package namespaces and package names.
/// Protobuf only has one such analogous concept; the package.
/// We're choosing to map the Protobuf package concept to the WIT package namespace,
/// but we still need a package name in WIT.
/// Always use the static package name 'proto' for every package generated from Protobuf.
const PACKAGE_NAME: &str = "proto";
/// The single generated world always has the static name `server`.
/// Individual services are converted to inline interfaces in exported by that world.
const WORLD_NAME: &str = "server";
/// Message and enum types are compiled into an interface called `types`.
const TYPES_INTERFACE_NAME: &str = "types";
/// One-ofs are compiled into an interface called `oneofs`.
const ONEOFS_INTERFACE_NAME: &str = "oneofs";

/// The fully-qualified (and versioned) interface name for Vimana gRPC types.
/// This must exactly match the values in `grpc.wit`.
const VIMANA_GRPC_TYPES_TARGET: &str = "vimana:grpc/types@0.0.0";
/// The name of the context type within the interface identified by [`VIMANA_GRPC_TYPES_TARGET`].
const VIMANA_GRPC_TYPES_CONTEXT_ITEM: &str = "context";
/// The name of the context parameter of each method handler function.
const CONTEXT_PARAMETER_NAME: &str = "context";
/// The name of the request parameter of each method handler function.
const REQUEST_PARAMETER_NAME: &str = "request";

/// An incrementally-built model of a Vimana server WIT file,
/// generated from Protobuf service and type definitions.
pub(crate) struct WitFile<'a> {
    /// The world that defines the server component.
    server_world: ServerWorld,
    /// Interfaces under which message and enum types are defined.
    types_interfaces: HashMap<TypeNameQualifier<'a>, TypesInterface<'a>>,
    /// Set to keep track of all types compiled so far,
    /// so we don't compile the same type twice.
    types_compiled: HashSet<QualifiedTypeName<'a>>,
    /// A map wherein descriptors for all messages and enums can be looked up
    /// by fully-qualified name.
    descriptors: &'a DescriptorMap<'a>,
    /// The set of command-line options passed to the plugin via `--vimana_opt`.
    parameters: &'a PluginParameters,
}

/// The compiler always generates a single 'server' world for the component to implement.
#[derive(Default)]
struct ServerWorld {
    /// The set of interfaces exported by this world.
    /// Each interface corresponds to a Protobuf service.
    services: Vec<Interface>,
}

/// A collection of message and enum type definitions within the same "qualifier namespace".
///
/// All message types are organized by "name qualifiers",
/// which consist of the Protobuf package and, optionally, any outer nesting message names
/// (e.g. `.package.OuterMessage` for type `.package.OuterMessage.InnerMessage`).
/// Each unique qualifier maps to a distinct WIT interface
/// holding the record and enumeration type definitions within that "qualifier namespace".
struct TypesInterface<'a> {
    /// The set records and enumerations defined within the qualifier namespace.
    types_defined: Vec<TypeDef>,

    /// The set of all messages and enums that are referenced by types in this interface,
    /// but which belong to a different interface.
    /// These types must be imported into the interface with a `use` statement.
    types_used: HashSet<QualifiedTypeName<'a>>,

    /// The set of all one-ofs that are referenced by messages in this interface.
    /// Unlike messages and enums, one-ofs are "single-use";
    /// a one-of type will always be referenced in exactly one single place.
    /// One-of type name qualifiers are always nested inside an outer message.
    oneofs_used: Vec<QualifiedTypeName<'a>>,

    /// An optional [one-ofs interfaces](OneofsInterface)
    /// that exist in the same package as this types interface.
    ///
    /// This would occur when a nested message sits alongside a one-of.
    /// For instance, in this example,
    /// the `choice` one-of would be in the same package as the `Inner` message:
    ///
    ///     message Outer {
    ///         oneof choice {
    ///             first = 1;
    ///             second = 2;
    ///         }
    ///         message Inner {}
    ///     }
    ///
    oneofs_interface: Option<OneofsInterface<'a>>,
}

/// Similar to a [`TypesInterface`] but for one-of fields,
/// which are not considered standalone types in Protobuf,
/// but must be modeled as such in WIT (as [`Variant`] types).
///
/// There will be exactly one one-ofs interface
/// for each message type that includes at least one `oneof` field.
struct OneofsInterface<'a> {
    /// The set of one-of [variants](Variant) defined within the qualifier namespace.
    oneofs_defined: Vec<TypeDef>,
    /// The set of all messages and enums that are referenced by types in this interface.
    /// Such types must necessarily belong to another interface.
    types_used: HashSet<QualifiedTypeName<'a>>,
}

impl<'a> WitFile<'a> {
    pub(crate) fn new(
        descriptors: &'a DescriptorMap<'a>,
        parameters: &'a PluginParameters,
    ) -> Self {
        Self {
            server_world: ServerWorld::default(),
            types_interfaces: HashMap::new(),
            types_compiled: HashSet::new(),
            descriptors,
            parameters,
        }
    }

    pub(crate) fn compile_service(
        &mut self,
        service_descriptor: &'a ServiceDescriptorProto,
        package_qualifier: &TypeNameQualifier<'a>,
    ) -> Result<()> {
        let mut service = Interface::new(service_descriptor.name().to_kebab_case());
        let mut types_used: HashSet<QualifiedTypeName<'a>> = HashSet::new();

        for method_descriptor in &service_descriptor.method {
            if let Some(options) = method_descriptor.options.as_ref() {
                for _option in &options.uninterpreted_option {
                    // TODO: Interpret method-level options like JSON transcoding.
                }
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
                QualifiedTypeName::from_path(method_descriptor.input_type(), package_qualifier);
            let response_type =
                QualifiedTypeName::from_path(method_descriptor.output_type(), package_qualifier);

            let mut function = StandaloneFunc::new(method_descriptor.name().to_kebab_case(), false);
            function.set_params(
                [
                    (
                        Ident::from(REQUEST_PARAMETER_NAME),
                        Type::Named(Ident::from(request_type.name.to_kebab_case())),
                    ),
                    (
                        Ident::from(CONTEXT_PARAMETER_NAME),
                        Type::Named(Ident::from(VIMANA_GRPC_TYPES_CONTEXT_ITEM)),
                    ),
                ]
                .into_iter()
                .collect::<Params>(),
            );
            function.set_result(Some(Type::Named(Ident::from(
                response_type.name.to_kebab_case(),
            ))));
            service.function(function);

            types_used.insert(request_type);
            types_used.insert(response_type);
        }

        service.use_type(
            VIMANA_GRPC_TYPES_TARGET,
            VIMANA_GRPC_TYPES_CONTEXT_ITEM,
            None,
        );

        for used_type in into_sorted_set_values(types_used) {
            service.use_type(
                used_type.use_type_target(&used_type.qualifier == package_qualifier),
                used_type.use_item(),
                None,
            );
        }

        self.server_world.services.push(service);
        Ok(())
    }

    pub(crate) fn compile_message(
        &mut self,
        message_descriptor: &'a DescriptorProto,
        qualifier: &TypeNameQualifier<'a>,
        syntax: ProtoSyntax,
    ) -> Result<()> {
        let type_name = qualifier.r#type(message_descriptor.name());
        if !self.types_compiled.contains(&type_name) {
            self.types_compiled.insert(type_name.clone());

            if let Some((types_interface, oneofs_interface)) =
                message_definition(message_descriptor, &type_name, syntax, self.parameters)?
            {
                for type_used in sorted_set_values(&types_interface.types_used) {
                    // Check if it's a message type first
                    if let Some((depended_descriptor, depended_syntax)) =
                        self.descriptors.get_message(type_used)
                    {
                        // Recursively compile message dependencies
                        self.compile_message(
                            depended_descriptor,
                            &type_used.qualifier,
                            depended_syntax,
                        )?;
                    } else if let Some(enum_descriptor) = self.descriptors.get_enum(type_used) {
                        self.compile_enum(enum_descriptor, &type_used.qualifier);
                    } else {
                        bail!("Type not found: {type_used}");
                    }
                }

                self.upsert_oneofs_interface(type_name.subqualifier(), oneofs_interface);
                self.upsert_types_interface(type_name.qualifier, types_interface);
            }
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

            let type_definition = enum_type_definition(enum_descriptor, type_name.name);
            self.upsert_types_interface(
                type_name.qualifier,
                TypesInterface::define_enum(type_definition),
            );
        }
    }

    fn upsert_types_interface(
        &mut self,
        qualifier: TypeNameQualifier<'a>,
        types_interface: TypesInterface<'a>,
    ) {
        match self.types_interfaces.get_mut(&qualifier) {
            Some(existing_types_interface) => {
                existing_types_interface.merge(types_interface);
            }
            None => {
                self.types_interfaces.insert(qualifier, types_interface);
            }
        }
    }

    fn upsert_oneofs_interface(
        &mut self,
        qualifier: TypeNameQualifier<'a>,
        oneofs_interface: OneofsInterface<'a>,
    ) {
        if !oneofs_interface.oneofs_defined.is_empty() {
            match self.types_interfaces.get_mut(&qualifier) {
                Some(existing_types_interface) => {
                    debug_assert!(existing_types_interface.oneofs_interface.is_none());
                    existing_types_interface.oneofs_interface = Some(oneofs_interface);
                }
                None => {
                    self.types_interfaces
                        .insert(qualifier, TypesInterface::for_oneofs(oneofs_interface));
                }
            }
        }
    }

    pub(crate) fn generate(mut self, server_package: &ProtoPackage<'a>) -> Result<Vec<File>> {
        let mut wit_contents = String::new();

        let server_package_qualifier = server_package.top_level_qualifier();
        let mut server_package = Package::new(server_package.wit_package_name());
        server_package.world(self.server_world.into_world());
        if let Some(server_package_types_interface) =
            self.types_interfaces.remove(&server_package_qualifier)
        {
            server_package.interface(
                server_package_types_interface.into_interface(&server_package_qualifier),
            );
        }
        wit_contents.push_str(server_package.to_string().as_str());

        for (name_qualifier, types_interface) in into_sorted_map_entries(self.types_interfaces) {
            wit_contents.push('\n');
            wit_contents.push_str(
                types_interface
                    .into_nested_package(name_qualifier)
                    .to_string()
                    .as_str(),
            );
        }

        Ok(vec![
            File {
                name: Some(format!("{}/server.wit", DIRECTORY)),
                insertion_point: None,
                content: Some(wit_contents),
                // TODO: Add generated code info to help with debugging.
                generated_code_info: None,
            },
            File {
                name: Some(format!("{}/deps/vimana:grpc/interface.wit", DIRECTORY)),
                insertion_point: None,
                content: Some(String::from(DEPENDENCY_GRPC)),
                generated_code_info: None,
            },
        ])
    }
}

/// Return a single-definition [types interface](TypesInterface) for a message type,
/// which can be merged with other types interfaces in the same qualifying context.
///
/// Also return a dedicated [one-ofs interface](OneofsInterface) for this message type.
fn message_definition<'a>(
    descriptor: &'a DescriptorProto,
    message_type_name: &QualifiedTypeName<'a>,
    syntax: ProtoSyntax,
    parameters: &PluginParameters,
) -> Result<Option<(TypesInterface<'a>, OneofsInterface<'a>)>> {
    // The set of fields (excluding one-ofs) that define this message type.
    let mut fields: Vec<Field> = Vec::with_capacity(descriptor.field.len());
    // Mapping from the short names (Protobuf syntax) of one-of fields contained within this message
    // to lists of variant cases defining the one-of field.
    // These must be handled separately from the rest of the fields
    // because their definition cannot be completed until processing all fields.
    let mut oneof_fields: Vec<(&'a str, Vec<VariantCase>)> = descriptor
        .oneof_decl
        .iter()
        .map(|oneof_decl| (oneof_decl.name(), Vec::new()))
        .collect();
    // The set of external message and enum types referenced by this message
    // (may contain duplicates that will be de-duped later).
    let mut types_used: Vec<QualifiedTypeName> = Vec::new();
    // The set of fully-qualified type names for one-of fields within this message.
    // Has the same length as `oneofs_defined` (because one-of types are "single-use").
    let mut oneofs_used: Vec<QualifiedTypeName> = Vec::new();
    // The set of definitions for one-of fields within this message.
    // Has the same length as `oneofs_used` (because one-of types are "single-use").
    let mut oneofs_defined: Vec<TypeDef> = Vec::new();
    // The set of message and enum types referenced by any of the one-ofs in this message
    // (may contain duplicates that will be de-duped later).
    let mut oneof_types_used: Vec<QualifiedTypeName> = Vec::new();

    for field_descriptor in &descriptor.field {
        let mut type_used: Option<QualifiedTypeName> = None;

        let mut wit_type = match field_descriptor.r#type() {
            ProtoType::Double => Type::F64,
            ProtoType::Float => Type::F32,
            ProtoType::Int64 => Type::S64,
            ProtoType::Uint64 => Type::U64,
            ProtoType::Int32 => Type::S32,
            ProtoType::Fixed64 => Type::U64,
            ProtoType::Fixed32 => Type::U32,
            ProtoType::Bool => Type::Bool,
            ProtoType::String => Type::String,
            ProtoType::Message | ProtoType::Enum => {
                let field_type_name = QualifiedTypeName::from_path(
                    field_descriptor.type_name(),
                    &message_type_name.qualifier,
                );
                let wit_short_name = field_type_name.name.to_kebab_case();
                type_used = Some(field_type_name);
                Type::named(wit_short_name)
            }
            ProtoType::Bytes => Type::list(Type::U8),
            ProtoType::Uint32 => Type::U32,
            ProtoType::Sfixed32 => Type::S32,
            ProtoType::Sfixed64 => Type::S64,
            ProtoType::Sint32 => Type::S32,
            ProtoType::Sint64 => Type::S64,
            ProtoType::Group => {
                if parameters.ignore_groups {
                    return Ok(None);
                }
                bail!("Protobuf groups are not supported; use nested messages instead")
            }
        };
        wit_type = match field_descriptor.label() {
            Label::Optional => {
                if syntax == ProtoSyntax::Proto2 || field_descriptor.proto3_optional() {
                    Type::option(wit_type)
                } else {
                    wit_type
                }
            }
            Label::Required => {
                // YAGNI (this is proto2-only syntax that's highly discouraged).
                if parameters.ignore_required {
                    return Ok(None);
                }
                bail!("Required fields are not supported");
            }
            Label::Repeated => Type::list(wit_type),
        };

        // Proto3 optionals use a hack for presence tracking called "synthetic one-ofs"
        // Those are not real one-ofs.
        if !field_descriptor.proto3_optional()
            && let Some(oneof_index) = field_descriptor.oneof_index
        {
            if let Some((_, cases)) = oneof_fields.get_mut(oneof_index as usize) {
                cases.push(VariantCase::value(
                    field_descriptor.name().to_kebab_case(),
                    wit_type,
                ));
                if let Some(type_used) = type_used {
                    oneof_types_used.push(type_used);
                }
            } else {
                bail!("Invalid oneof index {}", oneof_index);
            }
        } else {
            fields.push(Field::new(
                field_descriptor.name().to_kebab_case(),
                wit_type,
            ));
            if let Some(type_used) = type_used {
                types_used.push(type_used);
            }
        }
    }

    let oneof_qualifier = message_type_name.subqualifier();
    for (oneof_name_proto, cases) in oneof_fields {
        // An empty cases vector indicates that the field is a proto3 optional (synthetic one-of)
        // which does not count as a true one-of.
        // Protobuf guarantees that all synthetic one-ofs are preceeded by all true one-ofs,
        // so we can break early once we encounter the first synthetic.
        if cases.is_empty() {
            break;
        }

        let oneof_name_wit = oneof_name_proto.to_kebab_case();
        oneofs_defined.push(TypeDef::new(
            oneof_name_wit.clone(),
            TypeDefKind::Variant(Variant::from(cases)),
        ));
        oneofs_used.push(oneof_qualifier.r#type(oneof_name_proto));
        fields.push(Field::new(
            oneof_name_wit.clone(),
            Type::named(oneof_name_wit),
        ));
    }

    Ok(Some((
        TypesInterface::define_message(
            TypeDef::new(
                message_type_name.name.to_kebab_case(),
                TypeDefKind::Record(Record::new(fields)),
            ),
            types_used,
            oneofs_used,
        ),
        OneofsInterface::define(oneofs_defined, oneof_types_used),
    )))
}

fn enum_type_definition<'a>(enum_descriptor: &'a EnumDescriptorProto, name: &'a str) -> TypeDef {
    let mut wit_enum = Enum::empty();
    for variant in &enum_descriptor.value {
        wit_enum.case(variant.name().to_kebab_case());
    }
    TypeDef::new(name.to_kebab_case(), TypeDefKind::Enum(wit_enum))
}

impl ServerWorld {
    fn into_world(self) -> World {
        let mut world = World::new(WORLD_NAME);
        for service in self.services {
            world.item(WorldItem::InlineInterfaceExport(service));
        }
        world
    }
}

impl<'a> TypesInterface<'a> {
    fn define_message(
        type_definition: TypeDef,
        types_used: Vec<QualifiedTypeName<'a>>,
        oneofs_used: Vec<QualifiedTypeName<'a>>,
    ) -> Self {
        Self {
            types_defined: vec![type_definition],
            types_used: types_used.into_iter().collect(),
            oneofs_used,
            oneofs_interface: None,
        }
    }

    fn define_enum(type_definition: TypeDef) -> Self {
        Self {
            types_defined: vec![type_definition],
            types_used: HashSet::new(),
            oneofs_used: Vec::new(),
            oneofs_interface: None,
        }
    }

    fn for_oneofs(oneofs_interface: OneofsInterface<'a>) -> Self {
        Self {
            types_defined: Vec::new(),
            types_used: HashSet::new(),
            oneofs_used: Vec::new(),
            oneofs_interface: Some(oneofs_interface),
        }
    }

    fn merge(&mut self, other: TypesInterface<'a>) {
        self.types_defined.extend(other.types_defined);
        self.types_used.extend(other.types_used);
        self.oneofs_used.extend(other.oneofs_used);
        if other.oneofs_interface.is_some() {
            debug_assert!(self.oneofs_interface.is_none());
            self.oneofs_interface = other.oneofs_interface;
        }
    }

    fn into_interface(self, qualifier: &TypeNameQualifier<'a>) -> Interface {
        let mut interface = Interface::new(TYPES_INTERFACE_NAME);
        for used_type in into_sorted_set_values(self.types_used) {
            // Only add use statements for types from different qualifiers.
            if &used_type.qualifier != qualifier {
                interface.use_type(used_type.use_type_target(false), used_type.use_item(), None);
            }
        }
        for used_oneof in self.oneofs_used {
            interface.use_type(used_oneof.use_oneof_target(), used_oneof.use_item(), None);
        }
        for defined_type in self.types_defined {
            interface.type_def(defined_type);
        }
        interface
    }

    fn into_nested_package(mut self, qualifier: TypeNameQualifier<'a>) -> NestedPackage {
        let mut package = NestedPackage::new(qualifier.namespaced_package_name());
        let oneofs_interface = self.oneofs_interface.take();
        if !self.types_defined.is_empty() {
            package.interface(self.into_interface(&qualifier));
        }
        if let Some(oneofs_interface) = oneofs_interface {
            package.interface(oneofs_interface.into_interface());
        }
        package
    }
}

impl<'a> OneofsInterface<'a> {
    fn define(oneofs_defined: Vec<TypeDef>, types_used: Vec<QualifiedTypeName<'a>>) -> Self {
        Self {
            oneofs_defined,
            types_used: types_used.into_iter().collect(),
        }
    }

    fn into_interface(self) -> Interface {
        let mut interface = Interface::new(ONEOFS_INTERFACE_NAME);
        for used_type in into_sorted_set_values(self.types_used) {
            // Types are always defined in separate interfaces from one-ofs,
            // so they can never use a relative import (`use` statement).
            interface.use_type(used_type.use_type_target(false), used_type.use_item(), None);
        }
        for defined_oneof in self.oneofs_defined {
            interface.type_def(defined_oneof);
        }
        interface
    }
}

impl<'a> QualifiedTypeName<'a> {
    fn use_type_target(&self, relative: bool) -> Ident {
        self.use_target(TYPES_INTERFACE_NAME, relative)
    }

    fn use_oneof_target(&self) -> Ident {
        // Oneof imports only occur in types interfaces,
        // so they can never be relative.
        self.use_target(ONEOFS_INTERFACE_NAME, false)
    }

    fn use_target(&self, interface_name: &str, relative: bool) -> Ident {
        if relative {
            Ident::from(String::from(interface_name))
        } else {
            Ident::from(format!(
                "{}:{}/{interface_name}",
                self.qualifier.package_namespace(),
                self.qualifier.package_name()
            ))
        }
    }

    fn use_item(&self) -> Ident {
        Ident::from(self.name.to_kebab_case())
    }
}

impl<'a> TypeNameQualifier<'a> {
    fn namespaced_package_name(&self) -> PackageName {
        PackageName::new(self.package_namespace(), self.package_name(), None)
    }

    fn package_namespace(&self) -> String {
        self.package
            .path
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

impl<'a> ProtoPackage<'a> {
    fn wit_package_name(&self) -> PackageName {
        PackageName::new(
            self.path
                .iter()
                .map(|part| part.to_kebab_case())
                .collect::<Vec<_>>()
                .join(":"),
            Ident::from(PACKAGE_NAME),
            None,
        )
    }
}
