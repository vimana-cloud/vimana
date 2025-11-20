//! The compilation step involves consolidating TODO

use std::collections::HashMap;

use anyhow::{Context, Result, anyhow, bail};
use heck::ToKebabCase;
use prost::Message;
use prost_types::compiler::code_generator_response::File;
use prost_types::field_descriptor_proto::{Label, Type};
use prost_types::{
    EnumValueDescriptorProto, FieldDescriptorProto, MethodDescriptorProto, OneofDescriptorProto,
    ServiceDescriptorProto,
};

use crate::{DescriptorMap, ProtoPackage, ProtoSyntax, QualifiedTypeName, TypeNameQualifier};
use metadata_proto::vimana::runtime::field::Coding;
use metadata_proto::vimana::runtime::{
    Field, GrpcArity, GrpcMethod, GrpcService, Metadata, ProtoMessage,
};

/// Name of the generated metadata file in the output directory.
const FILENAME: &str = "metadata.binpb";

/// Coding values occur in a regular cycle of 4 categories:
/// implicit, packed, explicit, expanded.
const CODING_CATEGORIES: i32 = 4;

// The four categories of coding values.
const CODING_CATEGORY_IMPLICIT: i32 = 0;
const CODING_CATEGORY_PACKED: i32 = 1;
const CODING_CATEGORY_EXPLICIT: i32 = 2;
const CODING_CATEGORY_EXPANDED: i32 = 3;

// Coding constants for scalar values.
const CODING_SCALAR_BYTES: i32 = 0;
const CODING_SCALAR_STRING_UTF8: i32 = 4;
const CODING_SCALAR_STRING_PERMISSIVE: i32 = 8;
const CODING_SCALAR_BOOL: i32 = 12;
const CODING_SCALAR_INT32: i32 = 16;
const CODING_SCALAR_SINT32: i32 = 20;
const CODING_SCALAR_SFIXED32: i32 = 24;
const CODING_SCALAR_UINT32: i32 = 28;
const CODING_SCALAR_FIXED32: i32 = 32;
const CODING_SCALAR_INT64: i32 = 36;
const CODING_SCALAR_SINT64: i32 = 40;
const CODING_SCALAR_SFIXED64: i32 = 44;
const CODING_SCALAR_UINT64: i32 = 48;
const CODING_SCALAR_FIXED64: i32 = 52;
const CODING_SCALAR_FLOAT: i32 = 56;
const CODING_SCALAR_DOUBLE: i32 = 60;

// Coding constants for compound values.
const CODING_COMPOUND_ENUM: i32 = 0;
const CODING_COMPOUND_MESSAGE: i32 = 4;
const CODING_COMPOUND_ONEOF: i32 = 8;

/// The type used to index into the message definition array.
type MessageIndex = u32;

/// An incrementally-built model of a Vimana component metadata file,
/// generated from Protobuf service and type definitions.
pub(crate) struct MetadataFile<'a> {
    /// Incrementally-built result of compilation.
    metadata: Metadata,
    /// Mapping from fully-qualified message names
    /// to indices within the `messages` vector in [`metadata`](Self::metadata).
    message_indices: HashMap<QualifiedTypeName<'a>, MessageIndex>,
    /// Cache for the results of [`compile_enum_variants`](Self::compile_enum_variants).
    enum_variants: HashMap<QualifiedTypeName<'a>, Vec<Field>>,
    /// A map wherein descriptors for all messages and enums can be looked up
    /// by fully-qualified name.
    descriptors: &'a DescriptorMap<'a>,
}

impl<'a> MetadataFile<'a> {
    pub(crate) fn new(descriptors: &'a DescriptorMap<'a>) -> Self {
        Self {
            metadata: Metadata::default(),
            message_indices: HashMap::new(),
            enum_variants: HashMap::new(),
            descriptors,
        }
    }

    pub(crate) fn compile_service(
        &mut self,
        service_descriptor: &'a ServiceDescriptorProto,
        server_qualifier: &TypeNameQualifier<'a>,
    ) -> Result<()> {
        let mut methods: HashMap<String, GrpcMethod> = HashMap::new();
        for method_descriptor in &service_descriptor.method {
            methods.insert(
                String::from(method_descriptor.name()),
                self.compile_method(method_descriptor, server_qualifier)?,
            );
        }

        self.metadata.services.insert(
            qualified_service_name(service_descriptor.name(), &server_qualifier.package),
            GrpcService { methods },
        );

        Ok(())
    }

    fn compile_method(
        &mut self,
        method_descriptor: &'a MethodDescriptorProto,
        server_qualifier: &TypeNameQualifier<'a>,
    ) -> Result<GrpcMethod> {
        Ok(GrpcMethod {
            function: method_descriptor.name().to_kebab_case(),
            arity: if method_descriptor.client_streaming() {
                if method_descriptor.server_streaming() {
                    GrpcArity::BidiStreaming as i32
                } else {
                    GrpcArity::ClientStreaming as i32
                }
            } else if method_descriptor.server_streaming() {
                GrpcArity::ServerStreaming as i32
            } else {
                GrpcArity::Unary as i32
            },
            request: self.compile_message(method_descriptor.input_type(), server_qualifier)?,
            response: self.compile_message(method_descriptor.output_type(), server_qualifier)?,
        })
    }

    fn compile_message(
        &mut self,
        message_name: &'a str,
        server_qualifier: &TypeNameQualifier<'a>,
    ) -> Result<MessageIndex> {
        let message_name = QualifiedTypeName::from_path(message_name, server_qualifier);

        if let Some(index) = self.message_indices.get(&message_name) {
            // If we already compiled this message, return the existing index.
            Ok(*index)
        } else {
            // Otherwise, reserving an index for the new message before recursing
            // to avoid infinite recursion for circular messages.
            let index = self.allocate_message(&message_name)?;

            let (message_descriptor, syntax) = self
                .descriptors
                .get_message(&message_name)
                .ok_or_else(|| anyhow!("Type not found: {message_name}"))?;

            let mut fields: Vec<Field> = Vec::new();
            let mut oneof_fields: Vec<Field> = message_descriptor
                .oneof_decl
                .iter()
                .map(oneof_field)
                .collect();
            for field_descriptor in &message_descriptor.field {
                let mut field = self.compile_field(field_descriptor, server_qualifier, syntax)?;

                if let Some(oneof_index) = field_descriptor.oneof_index
                    && !field_descriptor.proto3_optional()
                {
                    if let Some(oneof_field) = oneof_fields.get_mut(oneof_index as usize) {
                        force_explicit_coding(&mut field);
                        oneof_field.variants.push(field);
                    } else {
                        bail!("Invalid oneof index {}", oneof_index);
                    }
                } else {
                    fields.push(field);
                }
            }
            for oneof_field in oneof_fields {
                // An empty variants vector indicates that the field is a proto3 optional
                // (synthetic one-of) which does not count as a true one-of.
                // Protobuf guarantees that all synthetic one-ofs
                // are preceeded by all true one-ofs,
                // so we can break early once we encounter the first synthetic.
                if oneof_field.variants.is_empty() {
                    break;
                }
                fields.push(oneof_field);
            }

            self.metadata
                .messages
                .get_mut(index as usize)
                .unwrap()
                .fields = fields;
            Ok(index)
        }
    }

    fn compile_field(
        &mut self,
        field_descriptor: &'a FieldDescriptorProto,
        server_qualifier: &TypeNameQualifier<'a>,
        syntax: ProtoSyntax,
    ) -> Result<Field> {
        let mut field = Field::default();
        field.number = field_descriptor.number() as u32;
        field.name = field_descriptor.name().to_kebab_case();

        let scalar_type_offset = match field_descriptor.r#type() {
            Type::Double => CODING_SCALAR_DOUBLE,
            Type::Float => CODING_SCALAR_FLOAT,
            Type::Int64 => CODING_SCALAR_INT64,
            Type::Uint64 => CODING_SCALAR_UINT64,
            Type::Int32 => CODING_SCALAR_INT32,
            Type::Fixed64 => CODING_SCALAR_FIXED64,
            Type::Fixed32 => CODING_SCALAR_FIXED32,
            Type::Bool => CODING_SCALAR_BOOL,
            Type::String => {
                if syntax == ProtoSyntax::Proto2 {
                    CODING_SCALAR_STRING_PERMISSIVE
                } else {
                    CODING_SCALAR_STRING_UTF8
                }
            }
            Type::Message => {
                let coding_offset = field_coding_offset(field_descriptor, syntax)?;
                if coding_offset != CODING_CATEGORY_EXPLICIT
                    && coding_offset != CODING_CATEGORY_EXPANDED
                {
                    bail!("Message fields must have either explicit or expanded coding");
                }
                field.coding = Some(Coding::CompoundCoding(
                    CODING_COMPOUND_MESSAGE + coding_offset,
                ));

                field.message =
                    self.compile_message(field_descriptor.type_name(), server_qualifier)?;

                return Ok(field);
            }
            Type::Enum => {
                let coding_offset = field_coding_offset(field_descriptor, syntax)?;
                field.coding = Some(Coding::CompoundCoding(CODING_COMPOUND_ENUM + coding_offset));

                let enum_name =
                    QualifiedTypeName::from_path(field_descriptor.type_name(), server_qualifier);
                field.variants = self.compile_enum_variants(enum_name)?;

                return Ok(field);
            }
            Type::Bytes => CODING_SCALAR_BYTES,
            Type::Uint32 => CODING_SCALAR_UINT32,
            Type::Sfixed32 => CODING_SCALAR_SFIXED32,
            Type::Sfixed64 => CODING_SCALAR_SFIXED64,
            Type::Sint32 => CODING_SCALAR_SINT32,
            Type::Sint64 => CODING_SCALAR_SINT64,
            Type::Group => {
                bail!("Protobuf groups are not supported; use nested messages instead")
            }
        };
        let coding_offset = field_coding_offset(field_descriptor, syntax)?;
        field.coding = Some(Coding::ScalarCoding(scalar_type_offset + coding_offset));
        Ok(field)
    }

    fn compile_enum_variants(&mut self, name: QualifiedTypeName<'a>) -> Result<Vec<Field>> {
        Ok(if let Some(variants) = self.enum_variants.get(&name) {
            variants.clone()
        } else if let Some(enum_descriptor) = self.descriptors.get_enum(&name) {
            let variants: Vec<Field> = enum_descriptor
                .value
                .iter()
                .map(enum_variant_field)
                .collect();
            self.enum_variants.insert(name, variants.clone());
            variants
        } else {
            bail!("Enum not found: {name}");
        })
    }

    /// This method should only be invoked
    /// if `name` is *not* already tracked in [`message_indices`](Self::message_indices).
    fn allocate_message(&mut self, name: &QualifiedTypeName<'a>) -> Result<MessageIndex> {
        let next_index: MessageIndex = self
            .metadata
            .messages
            .len()
            .try_into()
            .with_context(|| format!("Too many messages (maximum {})", MessageIndex::MAX))?;
        self.metadata.messages.push(ProtoMessage::default());
        self.message_indices.insert(name.clone(), next_index);
        Ok(next_index)
    }

    pub(crate) fn generate(self) -> Result<File> {
        let buffer = self.metadata.encode_to_vec();
        Ok(File {
            name: Some(String::from(FILENAME)),
            insertion_point: None,
            content: Some(unsafe { String::from_utf8_unchecked(buffer) }),
            // TODO: Add generated code info to help with debugging.
            generated_code_info: None,
        })
    }
}

fn qualified_service_name(name: &str, package: &ProtoPackage) -> String {
    let mut path = package.path.clone();
    path.push(name);
    path.join(".")
}

fn oneof_field(oneof: &OneofDescriptorProto) -> Field {
    let mut field = Field::default();
    field.name = oneof.name().to_kebab_case();
    field.coding = Some(Coding::CompoundCoding(CODING_COMPOUND_ONEOF));
    field
}

fn force_explicit_coding(field: &mut Field) {
    field.coding = Some(match field.coding.unwrap() {
        Coding::ScalarCoding(scalar_coding) => {
            Coding::ScalarCoding(force_explicit_coding_offset(scalar_coding))
        }
        Coding::CompoundCoding(compound_coding) => {
            Coding::CompoundCoding(force_explicit_coding_offset(compound_coding))
        }
    });
}

fn force_explicit_coding_offset(offset: i32) -> i32 {
    return (offset / CODING_CATEGORIES) * CODING_CATEGORIES + CODING_CATEGORY_EXPLICIT;
}

fn enum_variant_field(variant: &EnumValueDescriptorProto) -> Field {
    let mut field = Field::default();
    field.name = variant.name().to_kebab_case();
    field.number = variant.number() as u32;
    field
}

fn field_coding_offset<'a>(
    field_descriptor: &'a FieldDescriptorProto,
    syntax: ProtoSyntax,
) -> Result<i32> {
    Ok(match field_descriptor.label() {
        Label::Optional => {
            if syntax == ProtoSyntax::Proto3
                && !field_descriptor.proto3_optional()
                && field_descriptor.r#type() != Type::Message
            {
                CODING_CATEGORY_IMPLICIT
            } else {
                CODING_CATEGORY_EXPLICIT
            }
        }
        Label::Required => {
            // YAGNI (this is proto2-only syntax that's highly discouraged).
            bail!("Required fields are not supported");
        }
        Label::Repeated => {
            let field_type = field_descriptor.r#type();
            // Repeated fields are "expanded" by default in proto2.
            // In addition, Length-delimited fields (bytes, string, message) can never be packed.
            let packed_by_default = syntax != ProtoSyntax::Proto2
                && field_type != Type::Bytes
                && field_type != Type::String
                && field_type != Type::Message;
            let explicitly_packed = field_descriptor
                .options
                .as_ref()
                .and_then(|options| options.packed);
            if explicitly_packed.unwrap_or(packed_by_default) {
                CODING_CATEGORY_PACKED
            } else {
                CODING_CATEGORY_EXPANDED
            }
        }
    })
}
