//! Decoding logic for compound protobuf fields (messages, enums, and oneofs).

use core::hint::cold_path;
use std::collections::HashMap;
use std::mem::ManuallyDrop;
use std::result::Result as StdResult;

use anyhow::{Context, Result, anyhow, bail};
use prost::bytes::Buf;
use prost::encoding::{WireType, decode_varint};
use tonic::codec::DecodeBuf;
use wasmtime::component::Val;

use crate::{
    BUFFER_OVERFLOW, CompoundMerger, DecodeError, ENUM_NO_DEFAULT, FIELD_INDEX_OUT_OF_BOUNDS,
    INVALID_VARINT, MESSAGE_NON_RECORD, MergeFn, Merger, MessageMerger, NON_EXPLICIT_ONEOF_VARIANT,
    REPEATED_NON_LIST, WIRETYPE_NON_LENGTH_DELIMITED, WIRETYPE_NON_VARINT, decode_tag,
    explicit_scalar, read_length_check_overflow, skip,
};
use metadata_proto::vimana::runtime::field::{Coding, CompoundCoding, ScalarCoding};
use metadata_proto::vimana::runtime::{Field, ProtoMessage};

impl Merger {
    fn message(message_merger: *const MessageMerger) -> Self {
        Self {
            merge: message_outer_merge,
            compound: CompoundMerger {
                message: message_merger,
            },
        }
    }

    fn message_repeated(message_merger: *const MessageMerger) -> Self {
        Self {
            merge: message_repeated_merge,
            compound: CompoundMerger {
                message: message_merger,
            },
        }
    }

    pub(crate) fn enum_implicit(enumeration: &Field) -> Result<(Self, Val)> {
        let merger = compile_enum_variants(enumeration, enum_implicit_merge);

        // The enum must have a default zero value.
        if let Some(default) = unsafe { &merger.compound.enum_variants }.get(&0) {
            let default = default.clone();
            Ok((merger, Val::Enum(default)))
        } else {
            cold_path();
            Err(anyhow!("Implicit enum must have a default value"))
        }
    }

    pub(crate) fn enum_packed(enumeration: &Field) -> Self {
        compile_enum_variants(enumeration, enum_repeated_merge)
    }

    pub(crate) fn enum_explicit(enumeration: &Field) -> Self {
        compile_enum_variants(enumeration, enum_explicit_merge)
    }

    pub(crate) fn enum_expanded(enumeration: &Field) -> Self {
        compile_enum_variants(enumeration, enum_repeated_merge)
    }
}

/// Initialization logic for enumerations.
fn compile_enum_variants(enumeration: &Field, merge: MergeFn) -> Merger {
    let mut variants = HashMap::with_capacity(enumeration.variants.len());
    for variant in &enumeration.variants {
        variants.insert(variant.number, variant.name.clone());
    }
    Merger {
        merge,
        compound: CompoundMerger {
            enum_variants: ManuallyDrop::new(variants),
        },
    }
}

pub(crate) fn count_request_decoder_messages(
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
                    count_request_decoder_messages(source, field.message as usize, index_map)?;
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

pub(crate) fn compile_request_decoder(
    source: &Vec<ProtoMessage>,
    source_index: usize,
    destination: &mut Vec<MessageMerger>,
    index_map: &HashMap<usize, usize>,
) -> Result<*const MessageMerger> {
    let destination_index = index_map[&source_index];
    if destination_index < destination.len() {
        Ok(&destination[destination_index])
    } else {
        let message = &source[source_index];
        debug_assert!(destination_index == destination.len());
        let message_merger: *mut MessageMerger = destination
            .push_within_capacity(MessageMerger {
                fields: HashMap::with_capacity(message.fields.len()),
                defaults: Vec::with_capacity(message.fields.len()),
            })
            .map_err(|_| {
                // This should never happen (indicates logic error).
                anyhow!("Pre-compilation message counting failed")
            })?;

        for (index, source_field) in message.fields.iter().enumerate() {
            let (field_merger, field_default) = match source_field
                .coding
                .ok_or_else(|| anyhow!("Field #{} missing required coding", source_field.number))?
            {
                Coding::ScalarCoding(scalar_coding) => {
                    Merger::scalar(ScalarCoding::try_from(scalar_coding).with_context(|| {
                        format!(
                            "Invalid ScalarCoding for field #{}: {:?}",
                            source_field.number, scalar_coding,
                        )
                    })?)
                }
                Coding::CompoundCoding(compound_coding) => {
                    match CompoundCoding::try_from(compound_coding).with_context(|| {
                        format!(
                            "Invalid CompoundCoding for field #{}: {:?}",
                            source_field.number, compound_coding,
                        )
                    })? {
                        CompoundCoding::EnumImplicit => Merger::enum_implicit(source_field)
                            .with_context(|| {
                                format!("Invalid enum for field #{}", source_field.number)
                            })?,
                        CompoundCoding::EnumPacked => {
                            (Merger::enum_packed(source_field), Val::List(Vec::new()))
                        }
                        CompoundCoding::EnumExplicit => {
                            (Merger::enum_explicit(source_field), Val::Option(None))
                        }
                        CompoundCoding::EnumExpanded => {
                            (Merger::enum_expanded(source_field), Val::List(Vec::new()))
                        }
                        CompoundCoding::Message => (
                            Merger::message(
                                compile_request_decoder(
                                    source,
                                    source_field.message as usize,
                                    destination,
                                    index_map,
                                )
                                .with_context(|| {
                                    format!("Invalid message for field #{}", source_field.number)
                                })?,
                            ),
                            Val::Option(None),
                        ),
                        CompoundCoding::MessageExpanded => (
                            Merger::message_repeated(
                                compile_request_decoder(
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
                            Val::List(Vec::new()),
                        ),
                        CompoundCoding::Oneof => {
                            // Oneofs get "flattened" into the containing message:
                            // each variant field number is mapped
                            // to the same field index of the outer message.
                            let oneof_variants =
                                compile_oneof(source, source_field, destination, index_map)
                                    .context("Invalid oneof")?;
                            for (variant_number, variant_merger) in oneof_variants {
                                unsafe { &mut (*message_merger) }
                                    .fields
                                    .insert(variant_number, (index as u32, variant_merger));
                            }
                            unsafe { &mut (*message_merger) }
                                .defaults
                                .push((source_field.name.clone(), Val::Option(None)));
                            continue;
                        }
                    }
                }
            };

            unsafe { &mut (*message_merger) }
                .fields
                .insert(source_field.number, (index as u32, field_merger));
            unsafe { &mut (*message_merger) }
                .defaults
                .push((source_field.name.clone(), field_default));
        }

        Ok(message_merger)
    }
}

fn compile_oneof(
    source: &Vec<ProtoMessage>,
    field: &Field,
    destination: &mut Vec<MessageMerger>,
    index_map: &HashMap<usize, usize>,
) -> Result<HashMap<u32, Merger>> {
    let mut variant_mergers: HashMap<u32, Merger> = HashMap::with_capacity(field.variants.len());

    for variant in &field.variants {
        let variant_merger = match variant
            .coding
            .ok_or_else(|| anyhow!("Field #{} missing required coding", variant.number))?
        {
            Coding::ScalarCoding(scalar_coding) => {
                // Enforce explicit-only coding for oneof variants.
                if !explicit_scalar(scalar_coding) {
                    bail!(
                        "Variant #{} must use explicit scalar coding: {:?}",
                        variant.number,
                        scalar_coding,
                    );
                }

                let (merger, _default) =
                    Merger::scalar(ScalarCoding::try_from(scalar_coding).with_context(|| {
                        format!(
                            "Invalid ScalarCoding for field #{}: {:?}",
                            variant.number, scalar_coding,
                        )
                    })?);

                Merger {
                    merge: oneof_variant_merge,
                    compound: CompoundMerger {
                        oneof_variant: ManuallyDrop::new((variant.name.clone(), Box::new(merger))),
                    },
                }
            }
            Coding::CompoundCoding(compound_coding) => {
                match CompoundCoding::try_from(compound_coding).with_context(|| {
                    format!(
                        "Invalid CompoundCoding for field #{}: {:?}",
                        variant.number, compound_coding,
                    )
                })? {
                    CompoundCoding::EnumExplicit => {
                        let merger = Merger::enum_explicit(variant);
                        Merger {
                            merge: oneof_variant_merge,
                            compound: CompoundMerger {
                                oneof_variant: ManuallyDrop::new((
                                    variant.name.clone(),
                                    Box::new(merger),
                                )),
                            },
                        }
                    }
                    CompoundCoding::Message => {
                        let merger = Merger::message(
                            compile_request_decoder(
                                source,
                                variant.message as usize,
                                destination,
                                index_map,
                            )
                            .with_context(|| {
                                format!("Invalid message for field #{}", variant.number)
                            })?,
                        );
                        Merger {
                            merge: oneof_variant_merge,
                            compound: CompoundMerger {
                                oneof_variant: ManuallyDrop::new((
                                    variant.name.clone(),
                                    Box::new(merger),
                                )),
                            },
                        }
                    }
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

        variant_mergers.insert(variant.number, variant_merger);
    }

    Ok(variant_mergers)
}

#[inline]
pub(crate) fn message_inner_merge(
    field_map: &HashMap<u32, (u32, Merger)>,
    _wire_type: WireType,
    limit: &mut u32,
    src: &mut DecodeBuf<'_>,
    dst: &mut Val,
) -> StdResult<(), DecodeError> {
    // Inner message contents always decode to a complete record.
    // `message_outer_merge` would produce an optional record instead.
    if let Val::Record(fields) = dst {
        // Keep merging in fields until there are none left.
        while *limit > 0 {
            let (field_number, wire_type) = decode_tag(limit, src)?;

            // See if we know how to deal with this field number.
            if let Some((index, subfield_merger)) = field_map.get(&field_number) {
                // Get a mutable pointer to the relevant subvalue within this record.
                if let Some(subdst) = fields.get_mut(*index as usize) {
                    // Call the field's merge function into that subvalue.
                    (subfield_merger.merge)(subfield_merger, wire_type, limit, src, &mut subdst.1)
                        .map_err(|e| e.with_field(field_number))?;
                } else {
                    // The index calculated during compilation is out of bounds.
                    // This should be impossible.
                    cold_path();
                    return Err(
                        DecodeError::new(FIELD_INDEX_OUT_OF_BOUNDS).with_field(field_number)
                    );
                }
            } else {
                // Unknown field number. Use wire type information to skip it.
                cold_path();
                skip(field_number, wire_type, limit, src)
                    .map_err(|e| e.with_field(field_number))?;
            }
        }
        Ok(())
    } else {
        // API violation - this method should always be called for a `Record`.
        cold_path();
        Err(DecodeError::new(MESSAGE_NON_RECORD))
    }
}

pub(crate) fn message_outer_merge(
    merger: &Merger,
    wire_type: WireType,
    limit: &mut u32,
    src: &mut DecodeBuf<'_>,
    dst: &mut Val,
) -> StdResult<(), DecodeError> {
    if wire_type == WireType::LengthDelimited {
        let mut length = read_length_check_overflow(limit, src)?;

        let message_merger = unsafe { &(*merger.compound.message) };

        // If there's already a value, merge into it. Otherwise create a new one.
        let value = if let Val::Option(Some(existing)) = dst {
            existing.as_mut()
        } else {
            *dst = Val::Option(Some(Box::new(Val::Record(message_merger.defaults.clone()))));
            if let Val::Option(Some(v)) = dst {
                v.as_mut()
            } else {
                unreachable!()
            }
        };

        message_inner_merge(&message_merger.fields, wire_type, &mut length, src, value)?;

        Ok(())
    } else {
        Err(DecodeError::new(WIRETYPE_NON_LENGTH_DELIMITED))
    }
}

/// Decode a repeated message.
/// These are always expanded, never packed.
pub(crate) fn message_repeated_merge(
    merger: &Merger,
    wire_type: WireType,
    limit: &mut u32,
    src: &mut DecodeBuf<'_>,
    dst: &mut Val,
) -> StdResult<(), DecodeError> {
    if let Val::List(items) = dst {
        if wire_type == WireType::LengthDelimited {
            let mut length =
                read_length_check_overflow(limit, src).map_err(|e| e.with_index(items.len()))?;

            let message_merger = unsafe { &(*merger.compound.message) };
            let mut value = Val::Record(message_merger.defaults.clone());
            message_inner_merge(
                &message_merger.fields,
                wire_type,
                &mut length,
                src,
                &mut value,
            )
            .map_err(|e| e.with_index(items.len()))?;

            items.push(value);
            Ok(())
        } else {
            Err(DecodeError::new(WIRETYPE_NON_LENGTH_DELIMITED))
        }
    } else {
        Err(DecodeError::new(REPEATED_NON_LIST))
    }
}

/// Decode a oneof variant.
/// These are never repeated, and always explicitly presence-tracked.
pub(crate) fn oneof_variant_merge(
    merger: &Merger,
    wire_type: WireType,
    limit: &mut u32,
    src: &mut DecodeBuf<'_>,
    dst: &mut Val,
) -> StdResult<(), DecodeError> {
    let variant = unsafe { &merger.compound.oneof_variant };
    let variant_name = variant.0.clone();
    let variant_merger = variant.1.as_ref();

    // If the same variant has already been set, use it as the starting value for merging.
    // This enables proper merge semantics for message fields within oneofs.
    let mut value = if let Val::Option(Some(existing_variant)) = dst
        && let Val::Variant(existing_name, existing_value) = existing_variant.as_ref()
        && existing_name == &variant_name
    {
        cold_path();
        Val::Option(existing_value.clone())
    } else {
        Val::Option(None)
    };

    // Call the inner merge function, then wrap the result as a named variant.
    (variant_merger.merge)(variant_merger, wire_type, limit, src, &mut value)?;

    if let Val::Option(value) = value {
        *dst = Val::Option(Some(Box::new(Val::Variant(variant_name, value))));
        Ok(())
    } else {
        // This should have been verified in `compile_oneof_variant`.
        cold_path();
        Err(DecodeError::new(NON_EXPLICIT_ONEOF_VARIANT))
    }
}

#[inline(always)]
fn enum_inner(
    merger: &Merger,
    limit: &mut u32,
    src: &mut DecodeBuf<'_>,
) -> StdResult<Val, DecodeError> {
    let remaining_before = src.remaining();
    let varint = decode_varint(src).map_err(|_| DecodeError::new(INVALID_VARINT))?;
    let bytes_read = (remaining_before - src.remaining()) as u32;
    if bytes_read > *limit {
        return Err(DecodeError::new(BUFFER_OVERFLOW));
    }
    *limit -= bytes_read;

    let value = varint as u32;
    let enum_variants = unsafe { &merger.compound.enum_variants };
    if let Some(name) = enum_variants.get(&value).or_else(|| enum_variants.get(&0)) {
        Ok(Val::Enum(name.clone()))
    } else {
        // According to Protobuf spec,
        // all enums must have at least a default zero value.
        Err(DecodeError::new(ENUM_NO_DEFAULT))
    }
}

pub(crate) fn enum_explicit_merge(
    merger: &Merger,
    wire_type: WireType,
    limit: &mut u32,
    src: &mut DecodeBuf<'_>,
    dst: &mut Val,
) -> StdResult<(), DecodeError> {
    if wire_type == WireType::Varint {
        *dst = Val::Option(Some(Box::new(enum_inner(merger, limit, src)?)));
        Ok(())
    } else {
        Err(DecodeError::new(WIRETYPE_NON_VARINT))
    }
}

pub(crate) fn enum_implicit_merge(
    merger: &Merger,
    wire_type: WireType,
    limit: &mut u32,
    src: &mut DecodeBuf<'_>,
    dst: &mut Val,
) -> StdResult<(), DecodeError> {
    if wire_type == WireType::Varint {
        *dst = enum_inner(merger, limit, src)?;
        Ok(())
    } else {
        Err(DecodeError::new(WIRETYPE_NON_VARINT))
    }
}

pub(crate) fn enum_repeated_merge(
    merger: &Merger,
    wire_type: WireType,
    limit: &mut u32,
    src: &mut DecodeBuf<'_>,
    dst: &mut Val,
) -> StdResult<(), DecodeError> {
    if let Val::List(items) = dst {
        if wire_type == WireType::LengthDelimited {
            let mut length = read_length_check_overflow(limit, src)?;
            while length > 0 {
                items.push(
                    enum_inner(merger, &mut length, src).map_err(|e| e.with_index(items.len()))?,
                );
            }
            Ok(())
        } else if wire_type == WireType::Varint {
            items.push(enum_inner(merger, limit, src).map_err(|e| e.with_index(items.len()))?);
            Ok(())
        } else {
            Err(DecodeError::new(WIRETYPE_NON_VARINT))
        }
    } else {
        Err(DecodeError::new(REPEATED_NON_LIST))
    }
}
