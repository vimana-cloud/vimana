//! The compilation step involves consolidating TODO

use std::collections::HashMap;

use anyhow::{anyhow, bail, Context, Result};
use heck::ToKebabCase;
use prost::Message;
use prost_types::compiler::code_generator_response::File;
use prost_types::field_descriptor_proto::{Label, Type};
use prost_types::{
    EnumValueDescriptorProto, FieldDescriptorProto, MethodDescriptorProto, ServiceDescriptorProto,
};

use crate::{DescriptorMap, ProtoPackage, ProtoSyntax, QualifiedTypeName};
use metadata_proto::vimana::runtime::field::Coding;
use metadata_proto::vimana::runtime::{
    Field, GrpcArity, GrpcMethod, GrpcService, Metadata, ProtoMessage,
};

/// Name of the generated metadata file in the output directory.
const FILENAME: &str = "metadata.binpb";

const CODING_IMPLICIT_OFFSET: i32 = 0;
const CODING_PACKED_OFFSET: i32 = 1;
const CODING_EXPLICIT_OFFSET: i32 = 2;
const CODING_EXPANDED_OFFSET: i32 = 3;

const SCALAR_BYTES_OFFSET: i32 = 0;
const SCALAR_STRING_UTF8_OFFSET: i32 = 4;
const SCALAR_STRING_PERMISSIVE_OFFSET: i32 = 8;
const SCALAR_BOOL_OFFSET: i32 = 12;
const SCALAR_INT32_OFFSET: i32 = 16;
const SCALAR_SINT32_OFFSET: i32 = 20;
const SCALAR_SFIXED32_OFFSET: i32 = 24;
const SCALAR_UINT32_OFFSET: i32 = 28;
const SCALAR_FIXED32_OFFSET: i32 = 32;
const SCALAR_INT64_OFFSET: i32 = 36;
const SCALAR_SINT64_OFFSET: i32 = 40;
const SCALAR_SFIXED64_OFFSET: i32 = 44;
const SCALAR_UINT64_OFFSET: i32 = 48;
const SCALAR_FIXED64_OFFSET: i32 = 52;
const SCALAR_FLOAT_OFFSET: i32 = 56;
const SCALAR_DOUBLE_OFFSET: i32 = 60;

const COMPOUND_ENUM_OFFSET: i32 = 0;
const COMPOUND_MESSAGE_OFFSET: i32 = 4;
const COMPOUND_ONEOF_OFFSET: i32 = 8;

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
        main_package: &ProtoPackage<'a>,
        service_descriptor: &'a ServiceDescriptorProto,
    ) -> Result<()> {
        let mut methods: HashMap<String, GrpcMethod> = HashMap::new();
        for method_descriptor in &service_descriptor.method {
            methods.insert(
                String::from(method_descriptor.name()),
                self.compile_method(main_package, method_descriptor)?,
            );
        }

        self.metadata.services.insert(
            qualified_service_name(main_package, service_descriptor.name()),
            GrpcService { methods },
        );

        Ok(())
    }

    fn compile_method(
        &mut self,
        main_package: &ProtoPackage<'a>,
        method_descriptor: &'a MethodDescriptorProto,
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
            request: self.compile_message(main_package, method_descriptor.input_type())?,
            response: self.compile_message(main_package, method_descriptor.output_type())?,
        })
    }

    fn compile_message(
        &mut self,
        main_package: &ProtoPackage<'a>,
        message_name: &'a str,
    ) -> Result<MessageIndex> {
        let message_name = QualifiedTypeName::from_path(message_name, main_package);

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
            for field_descriptor in &message_descriptor.field {
                fields.push(self.compile_field(main_package, field_descriptor, syntax)?);
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
        main_package: &ProtoPackage<'a>,
        field_descriptor: &'a FieldDescriptorProto,
        syntax: ProtoSyntax,
    ) -> Result<Field> {
        let mut field = Field::default();
        field.number = field_descriptor.number() as u32;
        field.name = field_descriptor.name().to_kebab_case();

        let scalar_type_offset = match field_descriptor.r#type() {
            Type::Double => SCALAR_DOUBLE_OFFSET,
            Type::Float => SCALAR_FLOAT_OFFSET,
            Type::Int64 => SCALAR_INT64_OFFSET,
            Type::Uint64 => SCALAR_UINT64_OFFSET,
            Type::Int32 => SCALAR_INT32_OFFSET,
            Type::Fixed64 => SCALAR_FIXED64_OFFSET,
            Type::Fixed32 => SCALAR_FIXED32_OFFSET,
            Type::Bool => SCALAR_BOOL_OFFSET,
            Type::String => {
                if syntax == ProtoSyntax::Proto2 {
                    SCALAR_STRING_PERMISSIVE_OFFSET
                } else {
                    SCALAR_STRING_UTF8_OFFSET
                }
            }
            Type::Message => {
                let coding_offset = field_coding_offset(field_descriptor, syntax)?;
                if coding_offset != CODING_EXPLICIT_OFFSET
                    && coding_offset != CODING_EXPANDED_OFFSET
                {
                    bail!("Message fields must have either explicit or expanded coding");
                }
                field.coding = Some(Coding::CompoundCoding(
                    COMPOUND_MESSAGE_OFFSET + coding_offset,
                ));

                field.message = self.compile_message(main_package, field_descriptor.type_name())?;

                return Ok(field);
            }
            Type::Enum => {
                let coding_offset = field_coding_offset(field_descriptor, syntax)?;
                field.coding = Some(Coding::CompoundCoding(COMPOUND_ENUM_OFFSET + coding_offset));

                let enum_name =
                    QualifiedTypeName::from_path(field_descriptor.type_name(), main_package);
                field.variants = self.compile_enum_variants(enum_name)?;

                return Ok(field);
            }
            Type::Bytes => SCALAR_BYTES_OFFSET,
            Type::Uint32 => SCALAR_UINT32_OFFSET,
            Type::Sfixed32 => SCALAR_SFIXED32_OFFSET,
            Type::Sfixed64 => SCALAR_SFIXED64_OFFSET,
            Type::Sint32 => SCALAR_SINT32_OFFSET,
            Type::Sint64 => SCALAR_SINT64_OFFSET,
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

fn qualified_service_name<'a>(package: &ProtoPackage<'a>, name: &'a str) -> String {
    let mut path = package.path.clone();
    path.push(name);
    path.join(".")
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
                CODING_IMPLICIT_OFFSET
            } else {
                CODING_EXPLICIT_OFFSET
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
                CODING_PACKED_OFFSET
            } else {
                CODING_EXPANDED_OFFSET
            }
        }
    })
}
