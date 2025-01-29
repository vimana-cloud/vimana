//! Logic to encode protobuf messages directly from Wasm component values.

use std::collections::HashMap;
use std::mem::{forget, ManuallyDrop};

use prost::encoding::{encode_varint, encoded_len_varint, WireType};
use tonic::codec::EncodeBuf;
use tonic::Status;
use wasmtime::component::Val;

use crate::{
    explicit_scalar, tag, CompoundEncoder, EncodeError, Encoder, ENUM_NON_ENUM,
    ENUM_VARIANT_UNRECOGNIZED, LENGTH_INCONSISTENCY, MESSAGE_NON_OPTIONAL, MESSAGE_NON_RECORD,
    NO_ENCODER_FOR_FIELD, ONEOF_NON_OPTIONAL, ONEOF_NON_VARIANT, ONEOF_VARIANT_NO_PAYLOAD,
    ONEOF_VARIANT_UNRECOGNIZED, REPEATED_NON_LIST,
};
use error::log_error_status;
use metadata_proto::work::runtime::container::field::{Coding, CompoundCoding, ScalarCoding};
use metadata_proto::work::runtime::container::Field;
use names::ComponentName;

impl Encoder {
    /// Construct a new [`Encoder`] from the given [`Field`] representing a message type.
    /// Only the [subfields](Field::subfields) are significant.
    ///
    /// Since this method is expected to be invoked by the control plane,
    /// it returns an error [`Status`] consistent with the Work node's [`error`] handling.
    pub(crate) fn message_inner(
        message: &Field,
        component: &ComponentName,
    ) -> Result<Self, Status> {
        Ok(Self {
            encode: message_inner_encode,
            length: message_inner_length,
            tag: tag(message.number, WireType::LengthDelimited), // Ignored.
            compound: CompoundEncoder {
                subfields: compile_compound(message, false, component)?,
            },
        })
    }

    fn message_outer(message: &Field, component: &ComponentName) -> Result<Self, Status> {
        Ok(Self {
            encode: message_outer_encode,
            length: message_outer_length,
            tag: tag(message.number, WireType::LengthDelimited),
            compound: CompoundEncoder {
                subfields: compile_compound(message, false, component)?,
            },
        })
    }

    fn message_repeated(message: &Field, component: &ComponentName) -> Result<Self, Status> {
        Ok(Self {
            encode: message_repeated_encode,
            length: message_repeated_length,
            tag: tag(message.number, WireType::LengthDelimited),
            compound: CompoundEncoder {
                subfields: compile_compound(message, false, component)?,
            },
        })
    }

    pub(crate) fn oneof(oneof: &Field, component: &ComponentName) -> Result<Self, Status> {
        Ok(Self {
            encode: oneof_encode,
            length: oneof_length,
            tag: 0, // Ignored. Each variant has a tag.
            compound: CompoundEncoder {
                subfields: compile_compound(oneof, true, component)?,
            },
        })
    }

    pub(crate) fn enum_implicit(enumeration: &Field) -> Self {
        Self {
            encode: enum_implicit_encode,
            length: enum_implicit_length,
            tag: tag(enumeration.number, WireType::Varint), // Ignored.
            compound: CompoundEncoder {
                variants: compile_enum_variants(enumeration),
            },
        }
    }

    pub(crate) fn enum_packed(enumeration: &Field) -> Self {
        Self {
            encode: enum_packed_encode,
            length: enum_packed_length,
            tag: tag(enumeration.number, WireType::LengthDelimited),
            compound: CompoundEncoder {
                variants: compile_enum_variants(enumeration),
            },
        }
    }

    pub(crate) fn enum_explicit(enumeration: &Field) -> Self {
        Self {
            encode: enum_explicit_encode,
            length: enum_explicit_length,
            tag: tag(enumeration.number, WireType::Varint),
            compound: CompoundEncoder {
                variants: compile_enum_variants(enumeration),
            },
        }
    }

    pub(crate) fn enum_expanded(enumeration: &Field) -> Self {
        Self {
            encode: enum_expanded_encode,
            length: enum_expanded_length,
            tag: tag(enumeration.number, WireType::Varint),
            compound: CompoundEncoder {
                variants: compile_enum_variants(enumeration),
            },
        }
    }
}

/// Common initialization logic for messages and oneofs.
/// Oneofs just have the extra restriction that subfield encoders must be explicit.
fn compile_compound(
    field: &Field,
    is_oneof: bool,
    component: &ComponentName,
) -> Result<ManuallyDrop<HashMap<String, Encoder>>, Status> {
    let mut subfields: HashMap<String, Encoder> = HashMap::with_capacity(field.subfields.len());

    for subfield in &field.subfields {
        let subfield_encoder = match subfield.coding.ok_or(
            // API violated - coding required.
            Status::internal("encoder-subfield-no-coding"),
        )? {
            Coding::ScalarCoding(scalar_coding) => {
                // Oneof subfields must use explicit coding.
                // The Protobuf compiler should have made sure of that.
                if is_oneof && !explicit_scalar(scalar_coding) {
                    return Err(log_error_status!("oneof-subfield-non-explicit", component)(
                        scalar_coding,
                    ));
                }

                Encoder::scalar(
                    ScalarCoding::try_from(scalar_coding).map_err(
                        // Unrecognized enum value: Protobuf inconsistency?
                        log_error_status!("bad-scalar-coding", component),
                    )?,
                    subfield.number,
                )
            }
            Coding::CompoundCoding(compound_coding) => {
                // There are only two compound types allowed in a oneof.
                if is_oneof
                    && compound_coding != (CompoundCoding::Message as i32)
                    && compound_coding != (CompoundCoding::EnumExplicit as i32)
                {
                    return Err(log_error_status!("oneof-subfield-non-explicit", component)(
                        compound_coding,
                    ));
                }

                match CompoundCoding::try_from(compound_coding).map_err(
                    // Protobuf inconsistency? Enum number unknown.
                    log_error_status!("bad-compound-coding", component),
                )? {
                    CompoundCoding::EnumImplicit => Encoder::enum_implicit(subfield),
                    CompoundCoding::EnumPacked => Encoder::enum_packed(subfield),
                    CompoundCoding::EnumExplicit => Encoder::enum_explicit(subfield),
                    CompoundCoding::EnumExpanded => Encoder::enum_expanded(subfield),
                    CompoundCoding::Message => Encoder::message_outer(subfield, component)?,
                    CompoundCoding::MessageExpanded => {
                        Encoder::message_repeated(subfield, component)?
                    }
                    CompoundCoding::Oneof => Encoder::oneof(subfield, component)?,
                }
            }
        };

        subfields.insert(subfield.name.clone(), subfield_encoder);
    }

    Ok(ManuallyDrop::new(subfields))
}

/// Initialization logic for enumerations.
fn compile_enum_variants(enumeration: &Field) -> ManuallyDrop<HashMap<String, u32>> {
    let mut variants = HashMap::with_capacity(enumeration.subfields.len());
    for subfield in &enumeration.subfields {
        variants.insert(subfield.name.clone(), subfield.number);
    }
    ManuallyDrop::new(variants)
}

#[inline(always)]
pub(crate) fn message_inner_encode(
    encoder: &Encoder,
    value: &Val,
    lengths: &mut Vec<u32>,
    buf: &mut EncodeBuf<'_>,
) -> Result<(), EncodeError> {
    if let Val::Record(fields) = value {
        for (name, value) in fields.iter() {
            // Look up the encoder for the subfield by name.
            if let Some(encoder) = unsafe { &encoder.compound.subfields }.get(name) {
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
#[inline(always)]
fn message_inner_length(
    encoder: &Encoder,
    value: &Val,
    lengths: &mut Vec<u32>,
) -> Result<u32, EncodeError> {
    if let Val::Record(fields) = value {
        let mut total = 0;
        // Iterate over the subfields in reverse,
        // so sublengths are pushed in the opposite order of
        // how they are later popped during encoding.
        for (name, value) in fields.iter().rev() {
            if let Some(encoder) = unsafe { &encoder.compound.subfields }.get(name) {
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
) -> Result<(), EncodeError> {
    if let Val::Option(option) = value {
        // Message are always explicitly presence-tracked.
        if let Some(value) = option {
            if let Some(length) = lengths.pop() {
                encode_varint(encoder.tag, buf);
                encode_varint(length as u64, buf);
                message_inner_encode(encoder, value, lengths, buf)
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
) -> Result<u32, EncodeError> {
    // Message are always explicitly presence-tracked.
    if let Val::Option(option) = value {
        Ok(if let Some(value) = option {
            let length = message_inner_length(encoder, value, lengths)?;
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
) -> Result<(), EncodeError> {
    if let Val::List(items) = value {
        for (index, value) in items.iter().enumerate() {
            if let Some(length) = lengths.pop() {
                encode_varint(encoder.tag, buf);
                encode_varint(length as u64, buf);
                message_inner_encode(encoder, value, lengths, buf)
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
) -> Result<u32, EncodeError> {
    if let Val::List(items) = value {
        let mut total = 0;
        for (index, value) in items.iter().enumerate() {
            let sublength =
                message_inner_length(encoder, value, lengths).map_err(|e| e.with_index(index))?;
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
) -> Result<(), EncodeError> {
    if let Val::Option(option) = value {
        if let Some(value) = option {
            if let Val::Variant(name, payload) = value.as_ref() {
                if let Some(subfield_encoder) = unsafe { &encoder.compound.subfields }.get(name) {
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
) -> Result<u32, EncodeError> {
    if let Val::Option(option) = value {
        // Always explicitly presence-tracked.
        if let Some(value) = option {
            if let Val::Variant(name, payload) = value.as_ref() {
                // Look up the variant type by name.
                if let Some(subfield_encoder) = unsafe { &encoder.compound.subfields }.get(name) {
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
) -> Result<(), EncodeError> {
    if let Val::Enum(name) = value {
        if let Some(number) = unsafe { &encoder.compound.variants }.get(name) {
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
) -> Result<u32, EncodeError> {
    if let Val::Enum(name) = value {
        // Look up the enum variant number by name.
        if let Some(number) = unsafe { &encoder.compound.variants }.get(name) {
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
) -> Result<(), EncodeError> {
    if let Val::Enum(name) = value {
        if let Some(number) = unsafe { &encoder.compound.variants }.get(name) {
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
) -> Result<u32, EncodeError> {
    if let Val::Enum(name) = value {
        if let Some(number) = unsafe { &encoder.compound.variants }.get(name) {
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
) -> Result<(), EncodeError> {
    if let Val::List(items) = value {
        if items.len() > 0 {
            if let Some(length) = lengths.pop() {
                encode_varint(encoder.tag, buf);
                encode_varint(length as u64, buf);
                for (index, value) in items.iter().enumerate() {
                    if let Val::Enum(name) = value {
                        if let Some(number) = unsafe { &encoder.compound.variants }.get(name) {
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
) -> Result<u32, EncodeError> {
    if let Val::List(items) = value {
        Ok(if items.len() > 0 {
            let mut total = 0;
            for (index, value) in items.iter().enumerate() {
                if let Val::Enum(name) = value {
                    if let Some(number) = unsafe { &encoder.compound.variants }.get(name) {
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
) -> Result<(), EncodeError> {
    if let Val::List(items) = value {
        for (index, value) in items.iter().enumerate() {
            if let Val::Enum(name) = value {
                if let Some(number) = unsafe { &encoder.compound.variants }.get(name) {
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
) -> Result<u32, EncodeError> {
    if let Val::List(items) = value {
        let tag_length = encoded_len_varint(encoder.tag) as u32;
        let mut total = 0;
        for (index, value) in items.iter().enumerate() {
            if let Val::Enum(name) = value {
                if let Some(number) = unsafe { &encoder.compound.variants }.get(name) {
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
