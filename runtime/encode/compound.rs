//! Logic to encode protobuf messages directly from Wasm component values.

use std::collections::HashMap;
use std::mem::{forget, ManuallyDrop};
use std::result::Result as StdResult;

use anyhow::{anyhow, bail, Context, Result};
use prost::encoding::{encode_varint, encoded_len_varint, WireType};
use tonic::codec::EncodeBuf;
use wasmtime::component::Val;

use crate::{
    explicit_scalar, tag, CompoundEncoder, EncodeError, Encoder, MessageEncoder, ENUM_NON_ENUM,
    ENUM_VARIANT_UNRECOGNIZED, LENGTH_INCONSISTENCY, MESSAGE_NON_OPTIONAL, MESSAGE_NON_RECORD,
    NO_ENCODER_FOR_FIELD, ONEOF_NON_OPTIONAL, ONEOF_NON_VARIANT, ONEOF_VARIANT_NO_PAYLOAD,
    ONEOF_VARIANT_UNRECOGNIZED, REPEATED_NON_LIST,
};
use metadata_proto::vimana::runtime::field::{Coding, CompoundCoding, ScalarCoding};
use metadata_proto::vimana::runtime::{Field, ProtoMessage};

impl Encoder {
    fn message(field_number: u32, message_encoder: *const MessageEncoder) -> Self {
        Self {
            encode: message_outer_encode,
            length: message_outer_length,
            tag: tag(field_number, WireType::LengthDelimited),
            compound: CompoundEncoder {
                message: message_encoder,
            },
        }
    }

    fn message_repeated(field_number: u32, message_encoder: *const MessageEncoder) -> Self {
        Self {
            encode: message_repeated_encode,
            length: message_repeated_length,
            tag: tag(field_number, WireType::LengthDelimited),
            compound: CompoundEncoder {
                message: message_encoder,
            },
        }
    }

    pub(crate) fn oneof(variants: HashMap<String, Encoder>) -> Self {
        Self {
            encode: oneof_encode,
            length: oneof_length,
            tag: 0, // Ignored. Each variant has a tag.
            compound: CompoundEncoder {
                oneof_variants: ManuallyDrop::new(variants),
            },
        }
    }

    pub(crate) fn enum_implicit(enumeration: &Field) -> Self {
        Self {
            encode: enum_implicit_encode,
            length: enum_implicit_length,
            tag: tag(enumeration.number, WireType::Varint), // Ignored.
            compound: CompoundEncoder {
                enum_variants: ManuallyDrop::new(compile_enum_variants(enumeration)),
            },
        }
    }

    pub(crate) fn enum_packed(enumeration: &Field) -> Self {
        Self {
            encode: enum_packed_encode,
            length: enum_packed_length,
            tag: tag(enumeration.number, WireType::LengthDelimited),
            compound: CompoundEncoder {
                enum_variants: ManuallyDrop::new(compile_enum_variants(enumeration)),
            },
        }
    }

    pub(crate) fn enum_explicit(enumeration: &Field) -> Self {
        Self {
            encode: enum_explicit_encode,
            length: enum_explicit_length,
            tag: tag(enumeration.number, WireType::Varint),
            compound: CompoundEncoder {
                enum_variants: ManuallyDrop::new(compile_enum_variants(enumeration)),
            },
        }
    }

    pub(crate) fn enum_expanded(enumeration: &Field) -> Self {
        Self {
            encode: enum_expanded_encode,
            length: enum_expanded_length,
            tag: tag(enumeration.number, WireType::Varint),
            compound: CompoundEncoder {
                enum_variants: ManuallyDrop::new(compile_enum_variants(enumeration)),
            },
        }
    }
}

pub(crate) fn count_response_encoder_messages(
    source: &Vec<ProtoMessage>,
    source_index: usize,
    index_map: &mut HashMap<usize, usize>,
) -> Result<()> {
    if !index_map.contains_key(&source_index) {
        index_map.insert(source_index, index_map.len());
        count_compound_messages(source, &source[source_index].fields, index_map)
    } else {
        Ok(())
    }
}

fn count_compound_messages(
    source: &Vec<ProtoMessage>,
    fields: &Vec<Field>,
    index_map: &mut HashMap<usize, usize>,
) -> Result<()> {
    for field in fields {
        if let Coding::CompoundCoding(compound_coding) = field
            .coding
            .ok_or_else(|| anyhow!("Field #{} missing required coding", field.number))?
        {
            match CompoundCoding::try_from(compound_coding).with_context(|| {
                format!(
                    "Invalid CompoundCoding for field #{}: {:?}",
                    field.number, compound_coding,
                )
            })? {
                CompoundCoding::Message | CompoundCoding::MessageExpanded => {
                    count_response_encoder_messages(source, field.message as usize, index_map)?;
                }
                CompoundCoding::Oneof => {
                    count_compound_messages(source, &field.variants, index_map)?;
                }
                _ => {}
            }
        }
    }
    Ok(())
}

pub(crate) fn compile_response_encoder(
    source: &Vec<ProtoMessage>,
    source_index: usize,
    destination: &mut Vec<MessageEncoder>,
    index_map: &HashMap<usize, usize>,
) -> Result<*const MessageEncoder> {
    let destination_index = index_map[&source_index];
    if destination_index < destination.len() {
        Ok(&destination[destination_index])
    } else {
        let message = &source[source_index];
        debug_assert!(destination_index == destination.len());
        let message_encoder: *mut MessageEncoder = destination
            .push_mut_within_capacity(MessageEncoder {
                fields: HashMap::with_capacity(message.fields.len()),
            })
            .map_err(|_| {
                // This should never happen (indicates logic error).
                anyhow!("Pre-compilation message counting failed")
            })?;

        for source_field in &message.fields {
            let field_encoder = match source_field
                .coding
                .ok_or_else(|| anyhow!("Field #{} missing required coding", source_field.number))?
            {
                Coding::ScalarCoding(scalar_coding) => Encoder::scalar(
                    ScalarCoding::try_from(scalar_coding).with_context(|| {
                        format!(
                            "Invalid ScalarCoding for field #{}: {:?}",
                            source_field.number, scalar_coding,
                        )
                    })?,
                    source_field.number,
                ),
                Coding::CompoundCoding(compound_coding) => {
                    match CompoundCoding::try_from(compound_coding).with_context(|| {
                        format!(
                            "Invalid CompoundCoding for field #{}: {:?}",
                            source_field.number, compound_coding,
                        )
                    })? {
                        CompoundCoding::EnumImplicit => Encoder::enum_implicit(source_field),
                        CompoundCoding::EnumPacked => Encoder::enum_packed(source_field),
                        CompoundCoding::EnumExplicit => Encoder::enum_explicit(source_field),
                        CompoundCoding::EnumExpanded => Encoder::enum_expanded(source_field),
                        CompoundCoding::Message => Encoder::message(
                            source_field.number,
                            compile_response_encoder(
                                source,
                                source_field.message as usize,
                                destination,
                                index_map,
                            )
                            .with_context(|| {
                                format!("Invalid message for field #{}", source_field.number)
                            })?,
                        ),
                        CompoundCoding::MessageExpanded => Encoder::message_repeated(
                            source_field.number,
                            compile_response_encoder(
                                source,
                                source_field.message as usize,
                                destination,
                                index_map,
                            )
                            .with_context(|| {
                                format!(
                                    "Invalid repeated message for field #{}",
                                    source_field.number
                                )
                            })?,
                        ),
                        CompoundCoding::Oneof => Encoder::oneof(
                            compile_oneof(source, source_field, destination, index_map)
                                .context("Invalid oneof")?,
                        ),
                    }
                }
            };
            unsafe { &mut (*message_encoder) }
                .fields
                .insert(source_field.name.clone(), field_encoder);
        }

        Ok(message_encoder)
    }
}

/// Common initialization logic for messages and oneofs.
/// Oneofs just have the extra restriction that subfield encoders must be explicit.
fn compile_oneof(
    source: &Vec<ProtoMessage>,
    field: &Field,
    destination: &mut Vec<MessageEncoder>,
    index_map: &HashMap<usize, usize>,
) -> Result<HashMap<String, Encoder>> {
    let mut variant_encoders: HashMap<String, Encoder> =
        HashMap::with_capacity(field.variants.len());

    for variant in &field.variants {
        let subfield_encoder = match variant
            .coding
            .ok_or_else(|| anyhow!("Field #{} missing required coding", variant.number))?
        {
            Coding::ScalarCoding(scalar_coding) => {
                // Oneof subfields must use explicit coding.
                // The Protobuf compiler should have made sure of that.
                if !explicit_scalar(scalar_coding) {
                    return Err(anyhow!(
                        "Variant #{} must use explicit scalar coding: {:?}",
                        variant.number,
                        scalar_coding,
                    ));
                }

                Encoder::scalar(
                    ScalarCoding::try_from(scalar_coding).with_context(|| {
                        format!(
                            "Invalid ScalarCoding for field #{}: {:?}",
                            variant.number, scalar_coding,
                        )
                    })?,
                    variant.number,
                )
            }
            Coding::CompoundCoding(compound_coding) => {
                match CompoundCoding::try_from(compound_coding).with_context(|| {
                    format!(
                        "Invalid CompoundCoding for field #{}: {:?}",
                        variant.number, compound_coding,
                    )
                })? {
                    CompoundCoding::EnumExplicit => Encoder::enum_explicit(variant),
                    CompoundCoding::Message => Encoder::message(
                        variant.number,
                        compile_response_encoder(
                            source,
                            variant.message as usize,
                            destination,
                            index_map,
                        )
                        .with_context(|| {
                            format!("Invalid message for field #{}", variant.number)
                        })?,
                    ),
                    _ => {
                        // One-ofs can only contain explicit subfields.
                        bail!(
                            "Variant #{} must use explicit compound coding: {:?}",
                            variant.number,
                            compound_coding,
                        )
                    }
                }
            }
        };

        variant_encoders.insert(variant.name.clone(), subfield_encoder);
    }

    Ok(variant_encoders)
}

/// Initialization logic for enumerations.
fn compile_enum_variants(enumeration: &Field) -> HashMap<String, u32> {
    let mut variants = HashMap::with_capacity(enumeration.variants.len());
    for variant in &enumeration.variants {
        variants.insert(variant.name.clone(), variant.number);
    }
    variants
}

#[inline]
pub(crate) fn message_inner_encode(
    field_encoders: &HashMap<String, Encoder>,
    value: &Val,
    lengths: &mut Vec<u32>,
    buf: &mut EncodeBuf<'_>,
) -> StdResult<(), EncodeError> {
    if let Val::Record(fields) = value {
        for (name, value) in fields.iter() {
            // Look up the encoder for the subfield by name.
            if let Some(encoder) = field_encoders.get(name) {
                (encoder.encode)(&encoder, value, lengths, buf)
                    .map_err(|e| e.with_field(name.clone()))?;
            } else {
                // Mismatch between the component implementation and its container metadata.
                return Err(EncodeError::new(NO_ENCODER_FOR_FIELD).with_field(name.clone()));
            }
        }
        Ok(())
    } else {
        // Messages must correspond to records.
        Err(EncodeError::new(MESSAGE_NON_RECORD))
    }
}

/// Pre-compute the total length of the contents of a message,
/// pushing any length-delimited subfields onto the `lengths` queue,
/// but do *not* push the message's own content length onto the queue.
///
/// Used directly by [`ResponseEncoder`].
/// See [`message_inner_length`] for message subfields
/// where the the length of the message content is also pushed.
#[inline]
pub(crate) fn message_inner_length(
    field_encoders: &HashMap<String, Encoder>,
    value: &Val,
    lengths: &mut Vec<u32>,
) -> StdResult<u32, EncodeError> {
    if let Val::Record(fields) = value {
        let mut total = 0;
        // Iterate over the subfields in reverse,
        // so sublengths are pushed in the opposite order of
        // how they are later popped during encoding.
        for (name, value) in fields.iter().rev() {
            if let Some(encoder) = field_encoders.get(name) {
                let sublength = (encoder.length)(&encoder, value, lengths)
                    .map_err(|e| e.with_field(name.clone()))?;
                total = u32::saturating_add(total, sublength);
            } else {
                // Unexpected mismatch between the component and its compiled metadata.
                return Err(EncodeError::new(NO_ENCODER_FOR_FIELD).with_field(name.clone()));
            }
        }
        Ok(total)
    } else {
        // Messages must correspond to records.
        Err(EncodeError::new(MESSAGE_NON_RECORD))
    }
}

pub(crate) fn message_outer_encode(
    encoder: &Encoder,
    value: &Val,
    lengths: &mut Vec<u32>,
    buf: &mut EncodeBuf<'_>,
) -> StdResult<(), EncodeError> {
    if let Val::Option(option) = value {
        // Message are always explicitly presence-tracked.
        if let Some(value) = option {
            if let Some(length) = lengths.pop() {
                encode_varint(encoder.tag, buf);
                encode_varint(length as u64, buf);
                message_inner_encode(
                    &unsafe { &(*encoder.compound.message) }.fields,
                    value,
                    lengths,
                    buf,
                )
            } else {
                Err(EncodeError::new(LENGTH_INCONSISTENCY))
            }
        } else {
            // Absent messages are ignored.
            Ok(())
        }
    } else {
        // Embedded messages are always optional,
        // with explicit presence tracking.
        Err(EncodeError::new(MESSAGE_NON_OPTIONAL))
    }
}

fn message_outer_length(
    encoder: &Encoder,
    value: &Val,
    lengths: &mut Vec<u32>,
) -> StdResult<u32, EncodeError> {
    // Message are always explicitly presence-tracked.
    if let Val::Option(option) = value {
        Ok(if let Some(value) = option {
            let length = message_inner_length(
                &unsafe { &(*encoder.compound.message) }.fields,
                value,
                lengths,
            )?;
            lengths.push(length);
            u32::saturating_add(
                length,
                (encoded_len_varint(encoder.tag) + encoded_len_varint(length as u64)) as u32,
            )
        } else {
            0 // Absent messages are ignored.
        })
    } else {
        // Embedded messages are always optional,
        // with explicit presence tracking.
        Err(EncodeError::new(MESSAGE_NON_OPTIONAL))
    }
}

/// Encode a repeated message.
/// These are always expanded, never packed.
pub(crate) fn message_repeated_encode(
    encoder: &Encoder,
    value: &Val,
    lengths: &mut Vec<u32>,
    buf: &mut EncodeBuf<'_>,
) -> StdResult<(), EncodeError> {
    if let Val::List(items) = value {
        for (index, value) in items.iter().enumerate() {
            if let Some(length) = lengths.pop() {
                encode_varint(encoder.tag, buf);
                encode_varint(length as u64, buf);
                message_inner_encode(
                    &unsafe { &(*encoder.compound.message) }.fields,
                    value,
                    lengths,
                    buf,
                )
                .map_err(|e| e.with_index(index))?;
            } else {
                return Err(EncodeError::new(LENGTH_INCONSISTENCY).with_index(index));
            }
        }
        Ok(())
    } else {
        Err(EncodeError::new(REPEATED_NON_LIST))
    }
}

/// Pre-calculate lengths for [`message_repeated_encode`].
/// Never pushes to the queue because repeated messages are always expanded,
/// although subfields of messages may push to the queue.
fn message_repeated_length(
    encoder: &Encoder,
    value: &Val,
    lengths: &mut Vec<u32>,
) -> StdResult<u32, EncodeError> {
    if let Val::List(items) = value {
        let mut total = 0;
        for (index, value) in items.iter().enumerate() {
            let sublength = message_inner_length(
                &unsafe { &(*encoder.compound.message) }.fields,
                value,
                lengths,
            )
            .map_err(|e| e.with_index(index))?;
            total = u32::saturating_add(
                total,
                u32::saturating_add(
                    sublength,
                    (encoded_len_varint(encoder.tag) + encoded_len_varint(sublength as u64)) as u32,
                ),
            );
        }
        Ok(total)
    } else {
        Err(EncodeError::new(REPEATED_NON_LIST))
    }
}

/// Encode a oneof.
/// These are never repeated, and always explicitly presence-tracked.
pub(crate) fn oneof_encode(
    encoder: &Encoder,
    value: &Val,
    lengths: &mut Vec<u32>,
    buf: &mut EncodeBuf<'_>,
) -> StdResult<(), EncodeError> {
    if let Val::Option(option) = value {
        if let Some(value) = option {
            if let Val::Variant(name, payload) = value.as_ref() {
                if let Some(subfield_encoder) =
                    unsafe { &encoder.compound.oneof_variants }.get(name)
                {
                    if let Some(value) = payload {
                        // The inner function must use explicit presence tracking,
                        // which expects an optional. Wrap the value in one.
                        // Unsafe voodoo takes ownership of `value` (`&Box<Val>`)
                        // so we can re-use the heap pointer in our wrapper optional.
                        let wrapped_value = Val::Option(Some(unsafe {
                            Box::from_raw(Box::as_ptr(value) as *mut Val)
                        }));
                        let result = (subfield_encoder.encode)(
                            &subfield_encoder,
                            &wrapped_value,
                            lengths,
                            buf,
                        )
                        .map_err(|e| e.with_field(name.clone()));
                        // Forget the wrapped value so it doesn't double-drop the box.
                        forget(wrapped_value);
                        result
                    } else {
                        // Wasm variants allow you to omit the payload
                        // but Protobuf oneof cases always have a payload.
                        Err(EncodeError::new(ONEOF_VARIANT_NO_PAYLOAD).with_field(name.clone()))
                    }
                } else {
                    Err(EncodeError::new(ONEOF_VARIANT_UNRECOGNIZED).with_field(name.clone()))
                }
            } else {
                Err(EncodeError::new(ONEOF_NON_VARIANT))
            }
        } else {
            // Do nothing if the oneof is unset.
            Ok(())
        }
    } else {
        Err(EncodeError::new(ONEOF_NON_OPTIONAL))
    }
}

fn oneof_length(
    encoder: &Encoder,
    value: &Val,
    lengths: &mut Vec<u32>,
) -> StdResult<u32, EncodeError> {
    if let Val::Option(option) = value {
        // Always explicitly presence-tracked.
        if let Some(value) = option {
            if let Val::Variant(name, payload) = value.as_ref() {
                // Look up the variant type by name.
                if let Some(subfield_encoder) =
                    unsafe { &encoder.compound.oneof_variants }.get(name)
                {
                    if let Some(value) = payload {
                        // The inner function must use explicit presence tracking.
                        // Wrap the value as an optional so it always encodes.
                        // Unsafe voodoo "takes ownership" of `value` (`&Box<Val>`)
                        // so we can re-use the heap pointer in our optional.
                        let wrapped_value = Val::Option(Some(unsafe {
                            Box::from_raw(Box::as_ptr(value) as *mut Val)
                        }));
                        let result =
                            (subfield_encoder.length)(&subfield_encoder, &wrapped_value, lengths)
                                .map_err(|e| e.with_field(name.clone()));
                        // Forget the wrapped value so it doesn't double-drop the box.
                        forget(wrapped_value);
                        result
                    } else {
                        // Wasm enum variants allow you to omit the payload
                        // but Protobuf oneof cases have to have a payload.
                        Err(EncodeError::new(ONEOF_VARIANT_NO_PAYLOAD).with_field(name.clone()))
                    }
                } else {
                    Err(EncodeError::new(ONEOF_VARIANT_UNRECOGNIZED).with_field(name.clone()))
                }
            } else {
                Err(EncodeError::new(ONEOF_NON_VARIANT))
            }
        } else {
            // Do nothing if absent.
            Ok(0)
        }
    } else {
        Err(EncodeError::new(ONEOF_NON_OPTIONAL))
    }
}

pub(crate) fn enum_explicit_encode(
    encoder: &Encoder,
    value: &Val,
    _lengths: &mut Vec<u32>,
    buf: &mut EncodeBuf<'_>,
) -> StdResult<(), EncodeError> {
    if let Val::Enum(name) = value {
        if let Some(number) = unsafe { &encoder.compound.enum_variants }.get(name) {
            encode_varint(encoder.tag, buf);
            encode_varint(*number as u64, buf);
            Ok(())
        } else {
            // Got an unexpected enum variant name.
            Err(EncodeError::new(ENUM_VARIANT_UNRECOGNIZED))
        }
    } else {
        // Enumerations must correspond to WIT enums.
        Err(EncodeError::new(ENUM_NON_ENUM))
    }
}

fn enum_explicit_length(
    encoder: &Encoder,
    value: &Val,
    _lengths: &mut Vec<u32>,
) -> StdResult<u32, EncodeError> {
    if let Val::Enum(name) = value {
        // Look up the enum variant number by name.
        if let Some(number) = unsafe { &encoder.compound.enum_variants }.get(name) {
            Ok((encoded_len_varint(encoder.tag) + encoded_len_varint(*number as u64)) as u32)
        } else {
            // Got an unexpected enum variant name.
            Err(EncodeError::new(ENUM_VARIANT_UNRECOGNIZED))
        }
    } else {
        // Enumerations must correspond to WIT enums.
        Err(EncodeError::new(ENUM_NON_ENUM))
    }
}

pub(crate) fn enum_implicit_encode(
    encoder: &Encoder,
    value: &Val,
    _lengths: &mut Vec<u32>,
    buf: &mut EncodeBuf<'_>,
) -> StdResult<(), EncodeError> {
    if let Val::Enum(name) = value {
        if let Some(number) = unsafe { &encoder.compound.enum_variants }.get(name) {
            if *number != 0 {
                encode_varint(encoder.tag, buf);
                encode_varint(*number as u64, buf);
            }
            Ok(())
        } else {
            // Got an unexpected enum variant name.
            Err(EncodeError::new(ENUM_VARIANT_UNRECOGNIZED))
        }
    } else {
        // Enumerations must correspond to WIT enums.
        Err(EncodeError::new(ENUM_NON_ENUM))
    }
}

fn enum_implicit_length(
    encoder: &Encoder,
    value: &Val,
    _lengths: &mut Vec<u32>,
) -> StdResult<u32, EncodeError> {
    if let Val::Enum(name) = value {
        if let Some(number) = unsafe { &encoder.compound.enum_variants }.get(name) {
            Ok(if *number != 0 {
                (encoded_len_varint(encoder.tag) + encoded_len_varint(*number as u64)) as u32
            } else {
                0
            })
        } else {
            // Got an unexpected enum variant name.
            Err(EncodeError::new(ENUM_VARIANT_UNRECOGNIZED))
        }
    } else {
        // Enumerations must correspond to WIT enums.
        Err(EncodeError::new(ENUM_NON_ENUM))
    }
}

pub(crate) fn enum_packed_encode(
    encoder: &Encoder,
    value: &Val,
    lengths: &mut Vec<u32>,
    buf: &mut EncodeBuf<'_>,
) -> StdResult<(), EncodeError> {
    if let Val::List(items) = value {
        if items.len() > 0 {
            if let Some(length) = lengths.pop() {
                encode_varint(encoder.tag, buf);
                encode_varint(length as u64, buf);
                for (index, value) in items.iter().enumerate() {
                    if let Val::Enum(name) = value {
                        if let Some(number) = unsafe { &encoder.compound.enum_variants }.get(name) {
                            encode_varint(*number as u64, buf);
                        } else {
                            return Err(
                                EncodeError::new(ENUM_VARIANT_UNRECOGNIZED).with_index(index)
                            );
                        }
                    } else {
                        return Err(EncodeError::new(ENUM_NON_ENUM).with_index(index));
                    }
                }
            } else {
                return Err(EncodeError::new(LENGTH_INCONSISTENCY));
            }
        }
        Ok(())
    } else {
        Err(EncodeError::new(REPEATED_NON_LIST))
    }
}

fn enum_packed_length(
    encoder: &Encoder,
    value: &Val,
    lengths: &mut Vec<u32>,
) -> StdResult<u32, EncodeError> {
    if let Val::List(items) = value {
        Ok(if items.len() > 0 {
            let mut total = 0;
            for (index, value) in items.iter().enumerate() {
                if let Val::Enum(name) = value {
                    if let Some(number) = unsafe { &encoder.compound.enum_variants }.get(name) {
                        total += encoded_len_varint(*number as u64) as u32;
                    } else {
                        return Err(EncodeError::new(ENUM_VARIANT_UNRECOGNIZED).with_index(index));
                    }
                } else {
                    return Err(EncodeError::new(ENUM_NON_ENUM).with_index(index));
                }
            }
            // Push the length of the contents only.
            lengths.push(total);
            // Return the length of the contents plus tag and length itself.
            total + (encoded_len_varint(encoder.tag) + encoded_len_varint(total as u64)) as u32
        } else {
            0
        })
    } else {
        Err(EncodeError::new(REPEATED_NON_LIST))
    }
}

pub(crate) fn enum_expanded_encode(
    encoder: &Encoder,
    value: &Val,
    _lengths: &mut Vec<u32>,
    buf: &mut EncodeBuf<'_>,
) -> StdResult<(), EncodeError> {
    if let Val::List(items) = value {
        for (index, value) in items.iter().enumerate() {
            if let Val::Enum(name) = value {
                if let Some(number) = unsafe { &encoder.compound.enum_variants }.get(name) {
                    encode_varint(encoder.tag, buf);
                    encode_varint(*number as u64, buf);
                } else {
                    return Err(EncodeError::new(ENUM_VARIANT_UNRECOGNIZED).with_index(index));
                }
            } else {
                return Err(EncodeError::new(ENUM_NON_ENUM).with_index(index));
            }
        }
        Ok(())
    } else {
        Err(EncodeError::new(REPEATED_NON_LIST))
    }
}

fn enum_expanded_length(
    encoder: &Encoder,
    value: &Val,
    _lengths: &mut Vec<u32>,
) -> StdResult<u32, EncodeError> {
    if let Val::List(items) = value {
        let tag_length = encoded_len_varint(encoder.tag) as u32;
        let mut total = 0;
        for (index, value) in items.iter().enumerate() {
            if let Val::Enum(name) = value {
                if let Some(number) = unsafe { &encoder.compound.enum_variants }.get(name) {
                    total = u32::saturating_add(
                        total,
                        tag_length + encoded_len_varint(*number as u64) as u32,
                    );
                } else {
                    return Err(EncodeError::new(ENUM_VARIANT_UNRECOGNIZED).with_index(index));
                }
            } else {
                return Err(EncodeError::new(ENUM_NON_ENUM).with_index(index));
            }
        }
        Ok(total)
    } else {
        Err(EncodeError::new(REPEATED_NON_LIST))
    }
}
