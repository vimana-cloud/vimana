//! Decoding logic for compound protobuf fields (messages, enums, and oneofs).

use std::collections::HashMap;
use std::mem::ManuallyDrop;

use prost::encoding::{decode_varint, encoded_len_varint, WireType};
use tonic::codec::DecodeBuf;
use tonic::Status;
use wasmtime::component::Val;

use crate::{
    decode_tag, explicit_scalar, read_length_check_overflow, skip, CompoundMerger, DecodeError,
    MergeFn, Merger, BUFFER_OVERFLOW, ENUM_NO_DEFAULT, FIELD_INDEX_OUT_OF_BOUNDS, INVALID_VARINT,
    MESSAGE_NON_RECORD, NON_EXPLICIT_ONEOF_VARIANT, OVERFLOW_32BIT, REPEATED_NON_LIST,
    WIRETYPE_NON_LENGTH_DELIMITED, WIRETYPE_NON_VARINT,
};
use error::log_error_status;
use metadata_proto::work::runtime::field::{Coding, CompoundCoding, ScalarCoding};
use metadata_proto::work::runtime::Field;
use names::ComponentName;

impl Merger {
    /// Construct a new [`Merger`] from the given [`Field`] representing a message type.
    /// Only the [subfields](Field::subfields) are significant.
    ///
    /// Since this method is expected to be invoked by the control plane,
    /// it returns an error [`Status`] consistent with the Work node's [`error`] handling.
    pub(crate) fn message_inner(
        message: &Field,
        component: &ComponentName,
    ) -> Result<Self, Status> {
        compile_message(message, message_inner_merge, component)
    }
}

/// Common initialization logic for messages and oneofs.
/// Oneofs just have the extra restriction that subfield encoders must be explicit.
fn compile_message(
    field: &Field,
    merge: MergeFn,
    component: &ComponentName,
) -> Result<Merger, Status> {
    let mut subfields: HashMap<u32, (u32, Merger)> = HashMap::with_capacity(field.subfields.len());
    let mut defaults: Vec<(String, Val)> = Vec::with_capacity(field.subfields.len());

    for (index, subfield) in field.subfields.iter().enumerate() {
        let (subfield_merger, subfield_default) = match subfield.coding.ok_or(
            // API violated - coding required.
            Status::internal("decoder-subfield-no-coding"),
        )? {
            Coding::ScalarCoding(scalar_coding) => {
                Merger::scalar(ScalarCoding::try_from(scalar_coding).map_err(
                    // Unrecognized enum value: Protobuf inconsistency?
                    log_error_status!("bad-scalar-coding", component),
                )?)
            }
            Coding::CompoundCoding(compound_coding) => {
                match CompoundCoding::try_from(compound_coding).map_err(
                    // Protobuf inconsistency? Enum number unknown.
                    log_error_status!("bad-compound-coding", component),
                )? {
                    CompoundCoding::EnumImplicit => {
                        let merger = compile_enum_variants(subfield, enum_implicit_merge);

                        // The enum must have a default zero value.
                        if let Some(default) = unsafe { &merger.compound.enum_variants }.get(&0) {
                            let default = default.clone();
                            (merger, Val::Enum(default))
                        } else {
                            return Err(Status::failed_precondition(
                                "Enum must have a default value",
                            ));
                        }
                    }
                    CompoundCoding::EnumPacked => (
                        compile_enum_variants(subfield, enum_repeated_merge),
                        Val::List(Vec::new()),
                    ),
                    CompoundCoding::EnumExplicit => (
                        compile_enum_variants(subfield, enum_explicit_merge),
                        Val::Option(None),
                    ),
                    CompoundCoding::EnumExpanded => (
                        compile_enum_variants(subfield, enum_repeated_merge),
                        Val::List(Vec::new()),
                    ),
                    CompoundCoding::Message => (
                        compile_message(subfield, message_outer_merge, component)?,
                        Val::Option(None),
                    ),
                    CompoundCoding::MessageExpanded => (
                        compile_message(subfield, message_repeated_merge, component)?,
                        Val::List(Vec::new()),
                    ),
                    CompoundCoding::Oneof => {
                        // Oneofs get "flattened" into the containing message:
                        // each variant field number is mapped
                        // to the same subfield of the outer message.
                        for variant in subfield.subfields.iter() {
                            subfields.insert(
                                variant.number,
                                (index as u32, compile_oneof_variant(variant, component)?),
                            );
                        }
                        // Oneofs always have an absent (explicit presence-tracked) default.
                        defaults.push((subfield.name.clone(), Val::Option(None)));
                        continue;
                    }
                }
            }
        };

        subfields.insert(subfield.number, (index as u32, subfield_merger));
        defaults.push((subfield.name.clone(), subfield_default));
    }

    Ok(Merger {
        merge,
        defaults,
        compound: CompoundMerger {
            subfields: ManuallyDrop::new(subfields),
        },
    })
}

fn compile_oneof_variant(variant: &Field, component: &ComponentName) -> Result<Merger, Status> {
    let merger = match variant.coding.ok_or(
        // API violated - coding required.
        Status::internal("decoder-oneof-variant-no-coding"),
    )? {
        Coding::ScalarCoding(scalar_coding) => {
            // Enforce explicit-only coding.
            if explicit_scalar(scalar_coding) {
                // We know the default will be an empty optional
                // because we enforce explicit-only coding.
                let (merger, _default) =
                    Merger::scalar(ScalarCoding::try_from(scalar_coding).map_err(
                        // Unrecognized enum value: Protobuf inconsistency?
                        log_error_status!("bad-scalar-coding", component),
                    )?);
                merger
            } else {
                return Err(Status::internal("decoder-non-explicit-oneof-variant"));
            }
        }
        Coding::CompoundCoding(compound_coding) => {
            match CompoundCoding::try_from(compound_coding).map_err(
                // Protobuf inconsistency? Enum number unknown.
                log_error_status!("bad-compound-coding", component),
            )? {
                CompoundCoding::EnumExplicit => compile_enum_variants(variant, enum_explicit_merge),
                CompoundCoding::Message => {
                    compile_message(variant, message_outer_merge, component)?
                }
                _coding => {
                    return Err(Status::internal("decoder-non-explicit-oneof-variant"));
                }
            }
        }
    };

    Ok(Merger {
        merge: oneof_variant_merge,
        defaults: Vec::new(),
        compound: CompoundMerger {
            oneof_variant: ManuallyDrop::new((variant.name.clone(), Box::new(merger))),
        },
    })
}

/// Initialization logic for enumerations.
fn compile_enum_variants(enumeration: &Field, merge: MergeFn) -> Merger {
    let mut variants = HashMap::with_capacity(enumeration.subfields.len());
    for subfield in &enumeration.subfields {
        variants.insert(subfield.number, subfield.name.clone());
    }
    Merger {
        merge,
        defaults: Vec::new(),
        compound: CompoundMerger {
            enum_variants: ManuallyDrop::new(variants),
        },
    }
}

pub(crate) fn message_inner_merge(
    merger: &Merger,
    _wire_type: WireType,
    limit: &mut u32,
    src: &mut DecodeBuf<'_>,
    dst: &mut Val,
) -> Result<(), DecodeError> {
    // Inner message contents always decode to a complete record.
    // `message_outer_merge` would produce an optional record instead.
    if let Val::Record(fields) = dst {
        // Keep merging in fields until there are none left.
        while *limit > 0 {
            let (field_number, wire_type) = decode_tag(limit, src)?;

            // See if we know how to deal with this field number.
            if let Some((index, subfield_merger)) =
                unsafe { &merger.compound.subfields }.get(&field_number)
            {
                // Get a mutable pointer to the relevant subvalue within this record.
                if let Some(subdst) = fields.get_mut(*index as usize) {
                    // Call the field's merge function into that subvalue.
                    (subfield_merger.merge)(&subfield_merger, wire_type, limit, src, &mut subdst.1)
                        .map_err(|e| e.with_field(field_number))?;
                } else {
                    // The index calculated in `compile_message` is out of bounds.
                    // This should be impossible.
                    return Err(
                        DecodeError::new(FIELD_INDEX_OUT_OF_BOUNDS).with_field(field_number)
                    );
                }
            } else {
                // Unknown field number. Use wire type information to skip it.
                skip(wire_type, limit, src).map_err(|e| e.with_field(field_number))?;
            }
        }
        Ok(())
    } else {
        // API violation - this method should always be called for a `Record`.
        Err(DecodeError::new(MESSAGE_NON_RECORD))
    }
}

pub(crate) fn message_outer_merge(
    merger: &Merger,
    wire_type: WireType,
    limit: &mut u32,
    src: &mut DecodeBuf<'_>,
    dst: &mut Val,
) -> Result<(), DecodeError> {
    if wire_type == WireType::LengthDelimited {
        let mut length = read_length_check_overflow(limit, src)?;

        let mut value = Val::Record(merger.defaults.clone());
        message_inner_merge(merger, wire_type, &mut length, src, &mut value)?;

        *dst = Val::Option(Some(Box::new(value)));
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
) -> Result<(), DecodeError> {
    if let Val::List(items) = dst {
        if wire_type == WireType::LengthDelimited {
            let mut length =
                read_length_check_overflow(limit, src).map_err(|e| e.with_index(items.len()))?;

            let mut value = Val::Record(merger.defaults.clone());
            message_inner_merge(merger, wire_type, &mut length, src, &mut value)
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
) -> Result<(), DecodeError> {
    let variant = unsafe { &merger.compound.oneof_variant };
    let variant_name = variant.0.clone();
    let variant_merger = variant.1.as_ref();
    let mut value = Val::Option(None);

    // Call the inner merge function, then wrap the result as a named variant.
    (variant_merger.merge)(variant_merger, wire_type, limit, src, &mut value)?;

    if let Val::Option(value) = value {
        *dst = Val::Option(Some(Box::new(Val::Variant(variant_name, value))));
        Ok(())
    } else {
        // This should have been verified in `compile_oneof_variant`.
        Err(DecodeError::new(NON_EXPLICIT_ONEOF_VARIANT))
    }
}

#[inline(always)]
fn enum_inner(
    merger: &Merger,
    limit: &mut u32,
    src: &mut DecodeBuf<'_>,
) -> Result<Val, DecodeError> {
    let varint = decode_varint(src).map_err(|_| DecodeError::new(INVALID_VARINT))?;
    let bytes_read = encoded_len_varint(varint) as u32;
    if bytes_read > *limit {
        return Err(DecodeError::new(BUFFER_OVERFLOW));
    }
    *limit -= bytes_read;

    let value = u32::try_from(varint).map_err(|_| DecodeError::new(OVERFLOW_32BIT))?;
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
) -> Result<(), DecodeError> {
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
) -> Result<(), DecodeError> {
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
) -> Result<(), DecodeError> {
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
            items.push(enum_inner(&merger, limit, src).map_err(|e| e.with_index(items.len()))?);
            Ok(())
        } else {
            Err(DecodeError::new(WIRETYPE_NON_VARINT))
        }
    } else {
        Err(DecodeError::new(REPEATED_NON_LIST))
    }
}
