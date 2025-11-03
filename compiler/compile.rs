//! The compilation step involves consolidating TODO

use std::collections::HashMap;

use anyhow::{anyhow, bail, Error, Result};
use prost_types::compiler::{CodeGeneratorRequest, CodeGeneratorResponse};
use prost_types::field_descriptor_proto::{Label, Type};
use prost_types::{DescriptorProto, FieldDescriptorProto, FileDescriptorProto};

use metadata_proto::work::runtime::field::{Coding, CompoundCoding, ScalarCoding};
use metadata_proto::work::runtime::Field;

const PACKED_OFFSET: i32 = 1;
const EXPLICIT_OFFSET: i32 = 2;
const EXPANDED_OFFSET: i32 = 3;

#[derive(Copy, Clone)]
enum ProtoSyntax {
    Proto2,
    Proto3,
    Editions,
}

pub(crate) fn compile(request: CodeGeneratorRequest) -> Result<Vec<()>> {
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
            let subfields =
                compile_message(&message_name, message_type, syntax, &compiled_messages)?;
            compiled_messages.insert(
                message_name,
                Field {
                    number: 0,               // Ignored.
                    name: String::default(), // Ignored.
                    subfields,
                    // Coding ignored for top-level messages.
                    coding: None,
                },
            );
        }

        file_descriptors.insert(file_name, proto_file);
    }

    // One implementation config is generated per service.
    //for file_to_generate in &request.file_to_generate {
    //    let file_descriptor = file_descriptors.get(file_to_generate).unwrap();
    //    let package_name = file_descriptor.package.as_ref().unwrap();

    //    for service_descriptor in &file_descriptor.service {
    //        let service_name = service_descriptor
    //            .name
    //            .clone()
    //            .ok_or(Error::leaf("Service has empty name"))?;
    //        let mut streaming: bool = false;

    //        for method_descriptor in &service_descriptor.method {
    //            let request_type = method_descriptor.input_type.as_ref().unwrap();
    //            let response_type = method_descriptor.output_type.as_ref().unwrap();

    //            let client_streaming = method_descriptor.client_streaming.unwrap_or(false);
    //            let server_streaming = method_descriptor.server_streaming.unwrap_or(false);
    //            if client_streaming || server_streaming {
    //                streaming = true;
    //            }

    //            match method_descriptor.options.as_ref() {
    //                Some(options) => {
    //                    for option in &options.uninterpreted_option {
    //                        // TODO
    //                    }
    //                }
    //                None => (),
    //            }

    //            if streaming && service.http_routes.len() > 0 {
    //                return Err(error(
    //                    "Service cannot include both streaming RPCs and JSON transcoding",
    //                ));
    //            }

    //            service.type_imports.push(String::from(response_type));
    //            insert_types_recursively(&mut descriptors, request_type, package)?;
    //            insert_types_recursively(&mut descriptors, response_type, package)?;
    //        }
    //        package.services.push(service);
    //    }
    //}

    Ok(Vec::new())
}

fn compile_message(
    message_name: &String,
    descriptor: &DescriptorProto,
    syntax: ProtoSyntax,
    compiled_messages: &HashMap<String, Field>,
) -> Result<Vec<Field>> {
    // A request / response message is represented by a single `Field`
    // with only the subfields populated.
    let mut subfields: Vec<Field> = Vec::new();

    // One-ofs do not correspond 1-to-1 with Protobuf fields,
    // so they have to be compiled separately.
    let mut oneofs: Vec<Field> =
        Result::from_iter(descriptor.oneof_decl.iter().map(|oneof_descriptor| {
            Ok::<Field, Error>(Field {
                number: 0, // ignored
                name: oneof_descriptor
                    .name
                    .clone()
                    .ok_or_else(|| anyhow!("Oneof in '{message_name}' lacks a name"))?,
                subfields: Vec::new(), // Populated later.
                coding: Some(Coding::CompoundCoding(CompoundCoding::Oneof as i32)),
            })
        }))?;

    for proto_field in descriptor.field.iter() {
        let field_name = proto_field
            .name
            .as_ref()
            .ok_or_else(|| anyhow!("Field in '{message_name}' lacks a name"))?;
        let number = proto_field
            .number
            .ok_or_else(|| anyhow!("Field '{field_name}' in '{message_name}' lacks a number"))?
            .try_into()
            .map_err(|_| {
                anyhow!("Field '{field_name}' in '{message_name}' has a negative field number")
            })?;

        if let Some(oneof_index) = proto_field.oneof_index {
            // One-of members are not considered "direct" subfields.
            let oneof_index: usize = oneof_index.try_into().map_err(|_| {
                anyhow!("Field '{field_name}' in '{message_name}' has an invalid one-of index")
            })?;
            let oneof: &mut Field = oneofs.get_mut(oneof_index).ok_or_else(|| {
                anyhow!("Field '{field_name}' in '{message_name}' has an unknown one-of index")
            })?;
            oneof.subfields.push(compile_field(
                number,
                field_name,
                proto_field,
                message_name,
                syntax,
                compiled_messages,
            )?);
        } else {
            subfields.push(compile_field(
                number,
                field_name,
                proto_field,
                message_name,
                syntax,
                compiled_messages,
            )?);
        }
    }

    subfields.append(&mut oneofs);
    Ok(subfields)
}

fn compile_field(
    number: u32,
    field_name: &String,
    field: &FieldDescriptorProto,
    message_name: &String,
    syntax: ProtoSyntax,
    compiled_messages: &HashMap<String, Field>,
) -> Result<Field> {
    let field_type = Type::try_from(
        field
            .r#type
            .ok_or_else(|| anyhow!("Field '{field_name}' in '{message_name}' lacks a type"))?,
    )?;
    // Simplify coding conversion
    // by taking advantage of the recurring pattern with the coding enums:
    //   TODO
    let (compound, mut coding) = match field_type {
        Type::Double => (false, ScalarCoding::DoubleImplicit as i32),
        Type::Float => (false, ScalarCoding::FloatImplicit as i32),
        Type::Int64 => (false, ScalarCoding::Int64Implicit as i32),
        Type::Uint64 => (false, ScalarCoding::Uint64Implicit as i32),
        Type::Int32 => (false, ScalarCoding::Int32Implicit as i32),
        Type::Fixed64 => (false, ScalarCoding::Fixed64Implicit as i32),
        Type::Fixed32 => (false, ScalarCoding::Fixed32Implicit as i32),
        Type::Bool => (false, ScalarCoding::BoolImplicit as i32),
        Type::String => (false, ScalarCoding::StringUtf8Implicit as i32),
        Type::Group => {
            bail!("Field '{field_name}' in '{message_name}' is a group (which is unsupported)")
        }
        Type::Message => (true, CompoundCoding::Message as i32),
        Type::Bytes => (false, ScalarCoding::BytesImplicit as i32),
        Type::Uint32 => (false, ScalarCoding::Uint32Implicit as i32),
        Type::Enum => (true, CompoundCoding::EnumImplicit as i32),
        Type::Sfixed32 => (false, ScalarCoding::Sfixed32Implicit as i32),
        Type::Sfixed64 => (false, ScalarCoding::Sfixed64Implicit as i32),
        Type::Sint32 => (false, ScalarCoding::Sint32Implicit as i32),
        Type::Sint64 => (false, ScalarCoding::Sint64Implicit as i32),
    };
    match syntax {
        ProtoSyntax::Proto2 => {
            if let Some(label) = field.label {
                match Label::try_from(label)? {
                    Label::Optional => {
                        // Messages are unaffected by `optional`.
                        // (they always use explicit presence tracking).
                        if coding != CompoundCoding::Message as i32 {
                            // Any other field marked `optional`
                            // would use explicit, rather than implicit, tracking.
                            coding += EXPLICIT_OFFSET;
                        }
                    }
                    Label::Required => bail!(
                        "Field '{field_name}' in '{message_name}' is required (which not yet unsupported)"
                    ),
                    Label::Repeated => {
                        // All repeated fields in proto2 are always expanded.
                        coding += EXPANDED_OFFSET;
                    }
                }
            }
        }
        ProtoSyntax::Proto3 => {
            if let Some(label) = field.label {
                match Label::try_from(label)? {
                    Label::Optional => {
                        // Messages are unaffected by `optional`.
                        // (they always use explicit presence tracking).
                        if coding != CompoundCoding::Message as i32 {
                            // Any other field marked `optional`
                            // would use explicit, rather than implicit, tracking.
                            coding += EXPLICIT_OFFSET;
                        }
                    }
                    Label::Required => bail!(
                        "Field '{field_name}' in '{message_name}' is required (invalid in proto3)"
                    ),
                    Label::Repeated => {
                        if coding == CompoundCoding::Message as i32 {
                            // Repeated messages are always expanded.
                            coding += EXPANDED_OFFSET;
                        } else {
                            // Repeated non-message fields in proto3 are always packed.
                            coding += PACKED_OFFSET;
                        }
                    }
                }
            }
        }
        ProtoSyntax::Editions => todo!(),
    };

    Ok(Field {
        number,
        name: proto_name_to_wit_name(field_name),
        subfields: match field_type {
            // All scalar types have no subfields.
            Type::Double
            | Type::Float
            | Type::Int64
            | Type::Uint64
            | Type::Int32
            | Type::Fixed64
            | Type::Fixed32
            | Type::Bool
            | Type::String
            | Type::Bytes
            | Type::Uint32
            | Type::Sfixed32
            | Type::Sfixed64
            | Type::Sint32
            | Type::Sint64 => Vec::default(),
            // TODO: Support recursive nesting.
            Type::Message | Type::Enum => {
                let type_name = field.type_name.as_ref().ok_or_else(|| {
                    anyhow!("Field '{field_name}' in '{message_name}' lacks a type name")
                })?;
                compiled_messages
                    .get(type_name)
                    .ok_or_else(|| {
                        // This should be impossible
                        // because `protoc` guarantees that messages are described in TODO order.
                        anyhow!("Field '{field_name}' in '{message_name}' references unknown type '{type_name}'")
                    })?
                    .subfields
                    .clone()
            }
            Type::Group => {
                bail!("Field '{field_name}' in '{message_name}' is a group (which is unsupported)")
            }
        },
        coding: Some(if compound {
            Coding::CompoundCoding(coding)
        } else {
            Coding::ScalarCoding(coding)
        }),
    })
}

/// Convert a protobuf short name to a WIT short name.
/// This mainly involves converting `snake_case` to `kebab-case`.
fn proto_name_to_wit_name(proto_name: &String) -> String {
    // TODO: Think about edge cases.
    proto_name.replace("_", "-")
}
