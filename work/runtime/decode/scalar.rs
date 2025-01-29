//! Decoding logic for scalar protobuf fields
//! (anything besides messages, enums, and oneofs).

use std::io::Read;
use std::result::Result;

use prost::bytes::Buf;
use prost::encoding::WireType;
use tonic::codec::DecodeBuf;
use wasmtime::component::Val;

use crate::{
    read_length_check_overflow, read_varint, CompoundMerger, DecodeError, MergeFn, Merger,
    BUFFER_OVERFLOW, BUFFER_UNDERFLOW, INVALID_BOOL, INVALID_PERMISSIVE_STRING, INVALID_UTF8,
    INVALID_VARINT, OVERFLOW_32BIT, REPEATED_NON_LIST, WIRETYPE_NON_32BIT, WIRETYPE_NON_64BIT,
    WIRETYPE_NON_LENGTH_DELIMITED, WIRETYPE_NON_VARINT,
};
use metadata_proto::work::runtime::container::field::ScalarCoding;

impl Merger {
    pub(crate) fn scalar(coding: ScalarCoding) -> (Self, Val) {
        // Called in control plane, so O(n) exhaustive match is OK.
        let (merge, default): (MergeFn, Val) = match coding {
            ScalarCoding::BytesImplicit => (bytes_implicit_merge, Val::List(Vec::new())),
            ScalarCoding::BytesExplicit => (bytes_explicit_merge, Val::Option(None)),
            ScalarCoding::BytesExpanded => (bytes_repeated_merge, Val::List(Vec::new())),
            ScalarCoding::StringUtf8Implicit => {
                (string_utf8_implicit_merge, Val::String("".into()))
            }
            ScalarCoding::StringUtf8Explicit => (string_utf8_explicit_merge, Val::Option(None)),
            ScalarCoding::StringUtf8Expanded => (string_utf8_repeated_merge, Val::List(Vec::new())),
            ScalarCoding::StringPermissiveImplicit => {
                (string_permissive_implicit_merge, Val::String("".into()))
            }
            ScalarCoding::StringPermissiveExplicit => {
                (string_permissive_explicit_merge, Val::Option(None))
            }
            ScalarCoding::StringPermissiveExpanded => {
                (string_permissive_repeated_merge, Val::List(Vec::new()))
            }
            ScalarCoding::BoolImplicit => (bool_implicit_merge, Val::Bool(false)),
            ScalarCoding::BoolPacked => (bool_repeated_merge, Val::List(Vec::new())),
            ScalarCoding::BoolExplicit => (bool_explicit_merge, Val::Option(None)),
            ScalarCoding::BoolExpanded => (bool_repeated_merge, Val::List(Vec::new())),
            ScalarCoding::Int32Implicit => (int32_implicit_merge, Val::S32(0)),
            ScalarCoding::Int32Packed => (int32_repeated_merge, Val::List(Vec::new())),
            ScalarCoding::Int32Explicit => (int32_explicit_merge, Val::Option(None)),
            ScalarCoding::Int32Expanded => (int32_repeated_merge, Val::List(Vec::new())),
            ScalarCoding::Sint32Implicit => (sint32_implicit_merge, Val::S32(0)),
            ScalarCoding::Sint32Packed => (sint32_repeated_merge, Val::List(Vec::new())),
            ScalarCoding::Sint32Explicit => (sint32_explicit_merge, Val::Option(None)),
            ScalarCoding::Sint32Expanded => (sint32_repeated_merge, Val::List(Vec::new())),
            ScalarCoding::Sfixed32Implicit => (sfixed32_implicit_merge, Val::S32(0)),
            ScalarCoding::Sfixed32Packed => (sfixed32_repeated_merge, Val::List(Vec::new())),
            ScalarCoding::Sfixed32Explicit => (sfixed32_explicit_merge, Val::Option(None)),
            ScalarCoding::Sfixed32Expanded => (sfixed32_repeated_merge, Val::List(Vec::new())),
            ScalarCoding::Uint32Implicit => (uint32_implicit_merge, Val::U32(0)),
            ScalarCoding::Uint32Packed => (uint32_repeated_merge, Val::List(Vec::new())),
            ScalarCoding::Uint32Explicit => (uint32_explicit_merge, Val::Option(None)),
            ScalarCoding::Uint32Expanded => (uint32_repeated_merge, Val::List(Vec::new())),
            ScalarCoding::Fixed32Implicit => (fixed32_implicit_merge, Val::U32(0)),
            ScalarCoding::Fixed32Packed => (fixed32_repeated_merge, Val::List(Vec::new())),
            ScalarCoding::Fixed32Explicit => (fixed32_explicit_merge, Val::Option(None)),
            ScalarCoding::Fixed32Expanded => (fixed32_repeated_merge, Val::List(Vec::new())),
            ScalarCoding::Int64Implicit => (int64_implicit_merge, Val::S64(0)),
            ScalarCoding::Int64Packed => (int64_repeated_merge, Val::List(Vec::new())),
            ScalarCoding::Int64Explicit => (int64_explicit_merge, Val::Option(None)),
            ScalarCoding::Int64Expanded => (int64_repeated_merge, Val::List(Vec::new())),
            ScalarCoding::Sint64Implicit => (sint64_implicit_merge, Val::S64(0)),
            ScalarCoding::Sint64Packed => (sint64_repeated_merge, Val::List(Vec::new())),
            ScalarCoding::Sint64Explicit => (sint64_explicit_merge, Val::Option(None)),
            ScalarCoding::Sint64Expanded => (sint64_repeated_merge, Val::List(Vec::new())),
            ScalarCoding::Sfixed64Implicit => (sfixed64_implicit_merge, Val::S64(0)),
            ScalarCoding::Sfixed64Packed => (sfixed64_repeated_merge, Val::List(Vec::new())),
            ScalarCoding::Sfixed64Explicit => (sfixed64_explicit_merge, Val::Option(None)),
            ScalarCoding::Sfixed64Expanded => (sfixed64_repeated_merge, Val::List(Vec::new())),
            ScalarCoding::Uint64Implicit => (uint64_implicit_merge, Val::U64(0)),
            ScalarCoding::Uint64Packed => (uint64_repeated_merge, Val::List(Vec::new())),
            ScalarCoding::Uint64Explicit => (uint64_explicit_merge, Val::Option(None)),
            ScalarCoding::Uint64Expanded => (uint64_repeated_merge, Val::List(Vec::new())),
            ScalarCoding::Fixed64Implicit => (fixed64_implicit_merge, Val::U64(0)),
            ScalarCoding::Fixed64Packed => (fixed64_repeated_merge, Val::List(Vec::new())),
            ScalarCoding::Fixed64Explicit => (fixed64_explicit_merge, Val::Option(None)),
            ScalarCoding::Fixed64Expanded => (fixed64_repeated_merge, Val::List(Vec::new())),
            ScalarCoding::FloatImplicit => (float_implicit_merge, Val::Float32(0.0)),
            ScalarCoding::FloatPacked => (float_repeated_merge, Val::List(Vec::new())),
            ScalarCoding::FloatExplicit => (float_explicit_merge, Val::Option(None)),
            ScalarCoding::FloatExpanded => (float_repeated_merge, Val::List(Vec::new())),
            ScalarCoding::DoubleImplicit => (double_implicit_merge, Val::Float64(0.0)),
            ScalarCoding::DoublePacked => (double_repeated_merge, Val::List(Vec::new())),
            ScalarCoding::DoubleExplicit => (double_explicit_merge, Val::Option(None)),
            ScalarCoding::DoubleExpanded => (double_repeated_merge, Val::List(Vec::new())),
        };
        (
            Self {
                merge,
                // `defaults` and `compound` are ignored for scalars.
                defaults: Vec::new(),
                compound: CompoundMerger { scalar: () },
            },
            // Return the default value to the caller
            // (which is always a message merger being instantiated).
            default,
        )
    }
}

/// Define two [functions](MergeFn), `$explicit_name` and `$implicit_name`,
/// which check that the wire type is `$wire_type`
/// then merge the result of `$decode_inner` into the destination.
macro_rules! singular_merge_fns {
    ($explicit_name:ident, $implicit_name:ident, $wire_type:expr, $wire_type_error:expr, $decode_inner:ident,) => {
        fn $explicit_name(
            _merger: &Merger,
            wire_type: WireType,
            limit: &mut u32,
            src: &mut DecodeBuf<'_>,
            dst: &mut Val,
        ) -> Result<(), DecodeError> {
            if wire_type == $wire_type {
                *dst = Val::Option(Some(Box::new(($decode_inner)(limit, src)?)));
                Ok(())
            } else {
                Err(DecodeError::new($wire_type_error))
            }
        }

        fn $implicit_name(
            _merger: &Merger,
            wire_type: WireType,
            limit: &mut u32,
            src: &mut DecodeBuf<'_>,
            dst: &mut Val,
        ) -> Result<(), DecodeError> {
            if wire_type == $wire_type {
                *dst = ($decode_inner)(limit, src)?;
                Ok(())
            } else {
                Err(DecodeError::new($wire_type_error))
            }
        }
    };
}

/// Merge function boilerplate for "stringy" types: strings and bytes.
/// These are distinct in being unpackable; they can only be expanded for repetition.
macro_rules! stringy_mergers {
    ($explicit_name:ident, $implicit_name:ident, $repeated_name:ident, $decode_inner:ident,) => {
        singular_merge_fns!(
            $explicit_name,
            $implicit_name,
            WireType::LengthDelimited,
            WIRETYPE_NON_LENGTH_DELIMITED,
            $decode_inner,
        );

        fn $repeated_name(
            _merger: &Merger,
            wire_type: WireType,
            limit: &mut u32,
            src: &mut DecodeBuf<'_>,
            dst: &mut Val,
        ) -> Result<(), DecodeError> {
            // Strings and bytes cannot be packed. They can only be repeated expanded.
            if let Val::List(items) = dst {
                if wire_type == WireType::LengthDelimited {
                    items.push(($decode_inner)(limit, src).map_err(|e| e.with_index(items.len()))?);
                    Ok(())
                } else {
                    Err(DecodeError::new(WIRETYPE_NON_LENGTH_DELIMITED))
                }
            } else {
                Err(DecodeError::new(REPEATED_NON_LIST))
            }
        }
    };
}

#[inline(always)]
fn bytes_decode_inner(limit: &mut u32, src: &mut DecodeBuf<'_>) -> Result<Val, DecodeError> {
    let mut length = read_length_check_overflow(limit, src)?;
    let mut bytes = Vec::with_capacity(length as usize);
    while length > 0 {
        bytes.push(Val::U8(src.get_u8()));
        length -= 1;
    }
    Ok(Val::List(bytes))
}

stringy_mergers!(
    bytes_explicit_merge,
    bytes_implicit_merge,
    bytes_repeated_merge,
    bytes_decode_inner,
);

#[inline(always)]
fn string_utf8_decode_inner(limit: &mut u32, src: &mut DecodeBuf<'_>) -> Result<Val, DecodeError> {
    let length = read_length_check_overflow(limit, src)? as usize;
    let mut string = String::with_capacity(length);
    src.take(length)
        .reader()
        .read_to_string(&mut string)
        .map_err(|_| DecodeError::new(INVALID_UTF8))?;
    Ok(Val::String(string))
}

stringy_mergers!(
    string_utf8_explicit_merge,
    string_utf8_implicit_merge,
    string_utf8_repeated_merge,
    string_utf8_decode_inner,
);

#[inline(always)]
fn string_permissive_decode_inner(
    limit: &mut u32,
    src: &mut DecodeBuf<'_>,
) -> Result<Val, DecodeError> {
    let length = read_length_check_overflow(limit, src)? as usize;
    let mut string = String::with_capacity(length);
    src.take(length)
        .reader()
        .read_to_end(unsafe { string.as_mut_vec() })
        .map_err(|_| DecodeError::new(INVALID_PERMISSIVE_STRING))?;
    Ok(Val::String(string))
}

stringy_mergers!(
    string_permissive_explicit_merge,
    string_permissive_implicit_merge,
    string_permissive_repeated_merge,
    string_permissive_decode_inner,
);

/// Merge function boilerplate for all the "non-stringy" scalars:
/// Everything besides strings and bytes.
/// These can be both packed and expanded for repetition.
/// The decoder must always handle both, including intermixed.
macro_rules! numeric_mergers {
    ($explicit_name:ident, $implicit_name:ident, $repeated_name:ident, $wire_type:expr, $wire_type_error:expr, $decode_inner:ident,) => {
        singular_merge_fns!(
            $explicit_name,
            $implicit_name,
            $wire_type,
            $wire_type_error,
            $decode_inner,
        );

        fn $repeated_name(
            _merger: &Merger,
            wire_type: WireType,
            limit: &mut u32,
            src: &mut DecodeBuf<'_>,
            dst: &mut Val,
        ) -> Result<(), DecodeError> {
            // Protocol buffer parsers must be able to parse repeated fields
            // that were compiled as packed as if they were not packed, and vice versa.
            // This permits adding `[packed=true]` to existing fields
            // in a forward- and backward-compatible way.
            // https://protobuf.dev/programming-guides/encoding/#packed
            if let Val::List(items) = dst {
                if wire_type == WireType::LengthDelimited {
                    let mut length = read_length_check_overflow(limit, src)?;
                    while length > 0 {
                        items.push(
                            ($decode_inner)(&mut length, src)
                                .map_err(|e| e.with_index(items.len()))?,
                        );
                    }
                    Ok(())
                } else if wire_type == $wire_type {
                    items.push(($decode_inner)(limit, src).map_err(|e| e.with_index(items.len()))?);
                    Ok(())
                } else {
                    Err(DecodeError::new($wire_type_error))
                }
            } else {
                Err(DecodeError::new(REPEATED_NON_LIST))
            }
        }
    };
}

#[inline(always)]
fn bool_decode_inner(limit: &mut u32, src: &mut DecodeBuf<'_>) -> Result<Val, DecodeError> {
    if *limit >= 1 {
        let byte = src.get_u8();
        *limit -= 1;
        if byte <= 1 {
            Ok(Val::Bool(byte != 0))
        } else {
            Err(DecodeError::new(INVALID_BOOL))
        }
    } else {
        Err(DecodeError::new(BUFFER_OVERFLOW))
    }
}
numeric_mergers!(
    bool_explicit_merge,
    bool_implicit_merge,
    bool_repeated_merge,
    WireType::Varint,
    WIRETYPE_NON_VARINT,
    bool_decode_inner,
);

#[inline(always)]
fn int32_decode_inner(limit: &mut u32, src: &mut DecodeBuf<'_>) -> Result<Val, DecodeError> {
    let varint = read_varint(limit, src, INVALID_VARINT)?;
    let value = i32::try_from(varint).map_err(|_| DecodeError::new(OVERFLOW_32BIT))?;
    Ok(Val::S32(value))
}
numeric_mergers!(
    int32_explicit_merge,
    int32_implicit_merge,
    int32_repeated_merge,
    WireType::Varint,
    WIRETYPE_NON_VARINT,
    int32_decode_inner,
);

#[inline(always)]
fn sint32_decode_inner(limit: &mut u32, src: &mut DecodeBuf<'_>) -> Result<Val, DecodeError> {
    let varint = read_varint(limit, src, INVALID_VARINT)?;
    let value = u32::try_from(varint).map_err(|_| DecodeError::new(OVERFLOW_32BIT))?;
    Ok(Val::S32(((value >> 1) as i32) ^ (-((value & 1) as i32))))
}
numeric_mergers!(
    sint32_explicit_merge,
    sint32_implicit_merge,
    sint32_repeated_merge,
    WireType::Varint,
    WIRETYPE_NON_VARINT,
    sint32_decode_inner,
);

#[inline(always)]
fn sfixed32_decode_inner(limit: &mut u32, src: &mut DecodeBuf<'_>) -> Result<Val, DecodeError> {
    if *limit >= 4 {
        *limit -= 4;
        Ok(Val::S32(src.get_i32_le()))
    } else {
        Err(DecodeError::new(BUFFER_UNDERFLOW))
    }
}
numeric_mergers!(
    sfixed32_explicit_merge,
    sfixed32_implicit_merge,
    sfixed32_repeated_merge,
    WireType::ThirtyTwoBit,
    WIRETYPE_NON_32BIT,
    sfixed32_decode_inner,
);

#[inline(always)]
fn uint32_decode_inner(limit: &mut u32, src: &mut DecodeBuf<'_>) -> Result<Val, DecodeError> {
    let varint = read_varint(limit, src, INVALID_VARINT)?;
    let value = u32::try_from(varint).map_err(|_| DecodeError::new(OVERFLOW_32BIT))?;
    Ok(Val::U32(value))
}
numeric_mergers!(
    uint32_explicit_merge,
    uint32_implicit_merge,
    uint32_repeated_merge,
    WireType::Varint,
    WIRETYPE_NON_VARINT,
    uint32_decode_inner,
);

#[inline(always)]
fn fixed32_decode_inner(limit: &mut u32, src: &mut DecodeBuf<'_>) -> Result<Val, DecodeError> {
    if *limit >= 4 {
        *limit -= 4;
        Ok(Val::U32(src.get_u32_le()))
    } else {
        Err(DecodeError::new(BUFFER_UNDERFLOW))
    }
}
numeric_mergers!(
    fixed32_explicit_merge,
    fixed32_implicit_merge,
    fixed32_repeated_merge,
    WireType::ThirtyTwoBit,
    WIRETYPE_NON_32BIT,
    fixed32_decode_inner,
);

#[inline(always)]
fn int64_decode_inner(limit: &mut u32, src: &mut DecodeBuf<'_>) -> Result<Val, DecodeError> {
    let varint = read_varint(limit, src, INVALID_VARINT)?;
    Ok(Val::S64(varint as i64))
}
numeric_mergers!(
    int64_explicit_merge,
    int64_implicit_merge,
    int64_repeated_merge,
    WireType::Varint,
    WIRETYPE_NON_VARINT,
    int64_decode_inner,
);

#[inline(always)]
fn sint64_decode_inner(limit: &mut u32, src: &mut DecodeBuf<'_>) -> Result<Val, DecodeError> {
    let varint = read_varint(limit, src, INVALID_VARINT)?;
    let value = varint as i64;
    Ok(Val::S64(((value >> 1) as i64) ^ (-((value & 1) as i64))))
}
numeric_mergers!(
    sint64_explicit_merge,
    sint64_implicit_merge,
    sint64_repeated_merge,
    WireType::Varint,
    WIRETYPE_NON_VARINT,
    sint64_decode_inner,
);

#[inline(always)]
fn sfixed64_decode_inner(limit: &mut u32, src: &mut DecodeBuf<'_>) -> Result<Val, DecodeError> {
    if *limit >= 8 {
        *limit -= 8;
        Ok(Val::S64(src.get_i64_le()))
    } else {
        Err(DecodeError::new(BUFFER_UNDERFLOW))
    }
}
numeric_mergers!(
    sfixed64_explicit_merge,
    sfixed64_implicit_merge,
    sfixed64_repeated_merge,
    WireType::SixtyFourBit,
    WIRETYPE_NON_64BIT,
    sfixed64_decode_inner,
);

#[inline(always)]
fn uint64_decode_inner(limit: &mut u32, src: &mut DecodeBuf<'_>) -> Result<Val, DecodeError> {
    let value = read_varint(limit, src, INVALID_VARINT)?;
    Ok(Val::U64(value))
}
numeric_mergers!(
    uint64_explicit_merge,
    uint64_implicit_merge,
    uint64_repeated_merge,
    WireType::Varint,
    WIRETYPE_NON_VARINT,
    uint64_decode_inner,
);

#[inline(always)]
fn fixed64_decode_inner(limit: &mut u32, src: &mut DecodeBuf<'_>) -> Result<Val, DecodeError> {
    if *limit >= 8 {
        *limit -= 8;
        Ok(Val::U64(src.get_u64_le()))
    } else {
        Err(DecodeError::new(BUFFER_UNDERFLOW))
    }
}
numeric_mergers!(
    fixed64_explicit_merge,
    fixed64_implicit_merge,
    fixed64_repeated_merge,
    WireType::SixtyFourBit,
    WIRETYPE_NON_64BIT,
    fixed64_decode_inner,
);

#[inline(always)]
fn float_decode_inner(limit: &mut u32, src: &mut DecodeBuf<'_>) -> Result<Val, DecodeError> {
    if *limit >= 4 {
        *limit -= 4;
        Ok(Val::Float32(src.get_f32_le()))
    } else {
        Err(DecodeError::new(BUFFER_UNDERFLOW))
    }
}
numeric_mergers!(
    float_explicit_merge,
    float_implicit_merge,
    float_repeated_merge,
    WireType::ThirtyTwoBit,
    WIRETYPE_NON_32BIT,
    float_decode_inner,
);

#[inline(always)]
fn double_decode_inner(limit: &mut u32, src: &mut DecodeBuf<'_>) -> Result<Val, DecodeError> {
    if *limit >= 8 {
        *limit -= 8;
        Ok(Val::Float64(src.get_f64_le()))
    } else {
        Err(DecodeError::new(BUFFER_UNDERFLOW))
    }
}
numeric_mergers!(
    double_explicit_merge,
    double_implicit_merge,
    double_repeated_merge,
    WireType::SixtyFourBit,
    WIRETYPE_NON_64BIT,
    double_decode_inner,
);
