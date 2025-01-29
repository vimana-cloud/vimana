//! Encoding logic for scalar protobuf fields
//! (anything besides messages, enums, and oneofs).

use std::collections::HashMap;

use prost::bytes::BufMut;
use prost::encoding::{encode_varint, encoded_len_varint, WireType};
use tonic::codec::EncodeBuf;
use wasmtime::component::Val;

use crate::{
    tag, EncodeError, EncodeFn, Encoder, LengthFn, BOOL_NON_BOOL, BYTES_NON_LIST, BYTE_NON_BYTE,
    DOUBLE_NON_DOUBLE, EXPLICIT_NON_OPTION, FIXED32_NON_FIXED32, FIXED64_NON_FIXED64,
    FLOAT_NON_FLOAT, IMPOSSIBLE_PACKED_BYTES, IMPOSSIBLE_PACKED_STRING, INT32_NON_INT32,
    INT64_NON_INT64, LENGTH_INCONSISTENCY, REPEATED_NON_LIST, SFIXED32_NON_SFIXED32,
    SFIXED64_NON_SFIXED64, SINT32_NON_SINT32, SINT64_NON_SINT64, STRING_NON_STRING,
    UINT32_NON_UINT32, UINT64_NON_UINT64,
};
use metadata_proto::work::runtime::container::field::ScalarCoding;

impl Encoder {
    pub(crate) fn scalar(coding: ScalarCoding, number: u32) -> Self {
        // Called in data plane, so O(n) exhaustive match is OK.
        let (encode, length, wire_type): (EncodeFn, LengthFn, WireType) = match coding {
            ScalarCoding::BytesImplicit => (
                bytes_implicit_encode,
                bytes_implicit_length,
                WireType::LengthDelimited,
            ),
            ScalarCoding::BytesPacked => (
                bytes_packed_encode,
                bytes_packed_length,
                WireType::LengthDelimited,
            ),
            ScalarCoding::BytesExplicit => (
                bytes_explicit_encode,
                bytes_explicit_length,
                WireType::LengthDelimited,
            ),
            ScalarCoding::BytesExpanded => (
                bytes_expanded_encode,
                bytes_expanded_length,
                WireType::LengthDelimited,
            ),
            ScalarCoding::StringUtf8Implicit | ScalarCoding::StringPermissiveImplicit => (
                string_implicit_encode,
                string_implicit_length,
                WireType::LengthDelimited,
            ),
            ScalarCoding::StringUtf8Packed | ScalarCoding::StringPermissivePacked => (
                string_packed_encode,
                string_packed_length,
                WireType::LengthDelimited,
            ),
            ScalarCoding::StringUtf8Explicit | ScalarCoding::StringPermissiveExplicit => (
                string_explicit_encode,
                string_explicit_length,
                WireType::LengthDelimited,
            ),
            ScalarCoding::StringUtf8Expanded | ScalarCoding::StringPermissiveExpanded => (
                string_expanded_encode,
                string_expanded_length,
                WireType::LengthDelimited,
            ),
            ScalarCoding::BoolImplicit => {
                (bool_implicit_encode, bool_implicit_length, WireType::Varint)
            }
            ScalarCoding::BoolPacked => (
                bool_packed_encode,
                bool_packed_length,
                WireType::LengthDelimited,
            ),
            ScalarCoding::BoolExplicit => {
                (bool_explicit_encode, bool_explicit_length, WireType::Varint)
            }
            ScalarCoding::BoolExpanded => {
                (bool_expanded_encode, bool_expanded_length, WireType::Varint)
            }
            ScalarCoding::Int32Implicit => (
                int32_implicit_encode,
                int32_implicit_length,
                WireType::Varint,
            ),
            ScalarCoding::Int32Packed => (
                int32_packed_encode,
                int32_packed_length,
                WireType::LengthDelimited,
            ),
            ScalarCoding::Int32Explicit => (
                int32_explicit_encode,
                int32_explicit_length,
                WireType::Varint,
            ),
            ScalarCoding::Int32Expanded => (
                int32_expanded_encode,
                int32_expanded_length,
                WireType::Varint,
            ),
            ScalarCoding::Sint32Implicit => (
                sint32_implicit_encode,
                sint32_implicit_length,
                WireType::Varint,
            ),
            ScalarCoding::Sint32Packed => (
                sint32_packed_encode,
                sint32_packed_length,
                WireType::LengthDelimited,
            ),
            ScalarCoding::Sint32Explicit => (
                sint32_explicit_encode,
                sint32_explicit_length,
                WireType::Varint,
            ),
            ScalarCoding::Sint32Expanded => (
                sint32_expanded_encode,
                sint32_expanded_length,
                WireType::Varint,
            ),
            ScalarCoding::Sfixed32Implicit => (
                sfixed32_implicit_encode,
                sfixed32_implicit_length,
                WireType::ThirtyTwoBit,
            ),
            ScalarCoding::Sfixed32Packed => (
                sfixed32_packed_encode,
                sfixed32_packed_length,
                WireType::LengthDelimited,
            ),
            ScalarCoding::Sfixed32Explicit => (
                sfixed32_explicit_encode,
                sfixed32_explicit_length,
                WireType::ThirtyTwoBit,
            ),
            ScalarCoding::Sfixed32Expanded => (
                sfixed32_expanded_encode,
                sfixed32_expanded_length,
                WireType::ThirtyTwoBit,
            ),
            ScalarCoding::Uint32Implicit => (
                uint32_implicit_encode,
                uint32_implicit_length,
                WireType::Varint,
            ),
            ScalarCoding::Uint32Packed => (
                uint32_packed_encode,
                uint32_packed_length,
                WireType::LengthDelimited,
            ),
            ScalarCoding::Uint32Explicit => (
                uint32_explicit_encode,
                uint32_explicit_length,
                WireType::Varint,
            ),
            ScalarCoding::Uint32Expanded => (
                uint32_expanded_encode,
                uint32_expanded_length,
                WireType::Varint,
            ),
            ScalarCoding::Fixed32Implicit => (
                fixed32_implicit_encode,
                fixed32_implicit_length,
                WireType::ThirtyTwoBit,
            ),
            ScalarCoding::Fixed32Packed => (
                fixed32_packed_encode,
                fixed32_packed_length,
                WireType::LengthDelimited,
            ),
            ScalarCoding::Fixed32Explicit => (
                fixed32_explicit_encode,
                fixed32_explicit_length,
                WireType::ThirtyTwoBit,
            ),
            ScalarCoding::Fixed32Expanded => (
                fixed32_expanded_encode,
                fixed32_expanded_length,
                WireType::ThirtyTwoBit,
            ),
            ScalarCoding::Int64Implicit => (
                int64_implicit_encode,
                int64_implicit_length,
                WireType::Varint,
            ),
            ScalarCoding::Int64Packed => (
                int64_packed_encode,
                int64_packed_length,
                WireType::LengthDelimited,
            ),
            ScalarCoding::Int64Explicit => (
                int64_explicit_encode,
                int64_explicit_length,
                WireType::Varint,
            ),
            ScalarCoding::Int64Expanded => (
                int64_expanded_encode,
                int64_expanded_length,
                WireType::Varint,
            ),
            ScalarCoding::Sint64Implicit => (
                sint64_implicit_encode,
                sint64_implicit_length,
                WireType::Varint,
            ),
            ScalarCoding::Sint64Packed => (
                sint64_packed_encode,
                sint64_packed_length,
                WireType::LengthDelimited,
            ),
            ScalarCoding::Sint64Explicit => (
                sint64_explicit_encode,
                sint64_explicit_length,
                WireType::Varint,
            ),
            ScalarCoding::Sint64Expanded => (
                sint64_expanded_encode,
                sint64_expanded_length,
                WireType::Varint,
            ),
            ScalarCoding::Sfixed64Implicit => (
                sfixed64_implicit_encode,
                sfixed64_implicit_length,
                WireType::SixtyFourBit,
            ),
            ScalarCoding::Sfixed64Packed => (
                sfixed64_packed_encode,
                sfixed64_packed_length,
                WireType::LengthDelimited,
            ),
            ScalarCoding::Sfixed64Explicit => (
                sfixed64_explicit_encode,
                sfixed64_explicit_length,
                WireType::SixtyFourBit,
            ),
            ScalarCoding::Sfixed64Expanded => (
                sfixed64_expanded_encode,
                sfixed64_expanded_length,
                WireType::SixtyFourBit,
            ),
            ScalarCoding::Uint64Implicit => (
                uint64_implicit_encode,
                uint64_implicit_length,
                WireType::Varint,
            ),
            ScalarCoding::Uint64Packed => (
                uint64_packed_encode,
                uint64_packed_length,
                WireType::LengthDelimited,
            ),
            ScalarCoding::Uint64Explicit => (
                uint64_explicit_encode,
                uint64_explicit_length,
                WireType::Varint,
            ),
            ScalarCoding::Uint64Expanded => (
                uint64_expanded_encode,
                uint64_expanded_length,
                WireType::Varint,
            ),
            ScalarCoding::Fixed64Implicit => (
                fixed64_implicit_encode,
                fixed64_implicit_length,
                WireType::SixtyFourBit,
            ),
            ScalarCoding::Fixed64Packed => (
                fixed64_packed_encode,
                fixed64_packed_length,
                WireType::LengthDelimited,
            ),
            ScalarCoding::Fixed64Explicit => (
                fixed64_explicit_encode,
                fixed64_explicit_length,
                WireType::SixtyFourBit,
            ),
            ScalarCoding::Fixed64Expanded => (
                fixed64_expanded_encode,
                fixed64_expanded_length,
                WireType::SixtyFourBit,
            ),
            ScalarCoding::FloatImplicit => (
                float_implicit_encode,
                float_implicit_length,
                WireType::ThirtyTwoBit,
            ),
            ScalarCoding::FloatPacked => (
                float_packed_encode,
                float_packed_length,
                WireType::LengthDelimited,
            ),
            ScalarCoding::FloatExplicit => (
                float_explicit_encode,
                float_explicit_length,
                WireType::ThirtyTwoBit,
            ),
            ScalarCoding::FloatExpanded => (
                float_expanded_encode,
                float_expanded_length,
                WireType::ThirtyTwoBit,
            ),
            ScalarCoding::DoubleImplicit => (
                double_implicit_encode,
                double_implicit_length,
                WireType::SixtyFourBit,
            ),
            ScalarCoding::DoublePacked => (
                double_packed_encode,
                double_packed_length,
                WireType::LengthDelimited,
            ),
            ScalarCoding::DoubleExplicit => (
                double_explicit_encode,
                double_explicit_length,
                WireType::SixtyFourBit,
            ),
            ScalarCoding::DoubleExpanded => (
                double_expanded_encode,
                double_expanded_length,
                WireType::SixtyFourBit,
            ),
        };
        Self {
            encode,
            length,
            tag: tag(number, wire_type),
            subfields: HashMap::new(),
            variants: HashMap::new(),
        }
    }
}

/// Handle only the most banal boilerplate involved in declaring a scalar encoder.
macro_rules! encode_fn {
    ($name:ident, $type:path, $type_error:expr, $encode:expr,) => {
        #[allow(dead_code)]
        fn $name(
            encoder: &Encoder,
            value: &Val,
            lengths: &mut Vec<u32>,
            buf: &mut EncodeBuf<'_>,
        ) -> Result<(), EncodeError> {
            if let $type(value) = value {
                ($encode)(encoder.tag, value, lengths, buf)
            } else {
                Err(EncodeError::new($type_error))
            }
        }
    };
}
macro_rules! length_fn {
    ($name:ident, $type:path, $type_error:expr, $length:expr,) => {
        #[allow(dead_code)]
        fn $name(
            encoder: &Encoder,
            value: &Val,
            lengths: &mut Vec<u32>,
        ) -> Result<u32, EncodeError> {
            if let $type(value) = value {
                ($length)(encoder.tag, value, lengths)
            } else {
                Err(EncodeError::new($type_error))
            }
        }
    };
}
macro_rules! scalar_encode_fns {
    ($encode_inner:ident, $default:ident, $type:path, $type_error:expr, $explicit_name:ident, $implicit_name:ident, $packed_name:ident, $expanded_name:ident,) => {
        encode_fn!(
            $explicit_name,
            Val::Option,
            EXPLICIT_NON_OPTION,
            // TODO: Verify lambda is inlined.
            |tag: u64,
             value: &Option<Box<Val>>,
             _lengths: &mut Vec<u32>,
             buf: &mut EncodeBuf<'_>| {
                if let Some(item) = value {
                    if let $type(value) = item.as_ref() {
                        encode_varint(tag, buf);
                        $encode_inner(value, buf)
                    } else {
                        return Err(EncodeError::new($type_error));
                    }
                } else {
                    Ok(())
                }
            },
        );

        encode_fn!(
            $implicit_name,
            $type,
            $type_error,
            // TODO: Verify lambda is inlined.
            |tag: u64, value, _lengths: &mut Vec<u32>, buf: &mut EncodeBuf<'_>| {
                if !$default(value) {
                    encode_varint(tag, buf);
                    $encode_inner(value, buf)
                } else {
                    Ok(())
                }
            },
        );

        encode_fn!(
            $packed_name,
            Val::List,
            REPEATED_NON_LIST,
            // TODO: Verify lambda is inlined.
            |tag: u64, value: &Vec<Val>, lengths: &mut Vec<u32>, buf: &mut EncodeBuf<'_>| {
                if value.len() > 0 {
                    if let Some(length) = lengths.pop() {
                        encode_varint(tag, buf);
                        encode_varint(length as u64, buf);
                        for item in value.iter() {
                            if let $type(value) = item {
                                $encode_inner(value, buf)?;
                            } else {
                                return Err(EncodeError::new($type_error));
                            }
                        }
                    } else {
                        return Err(EncodeError::new(LENGTH_INCONSISTENCY));
                    }
                }
                Ok(())
            },
        );

        encode_fn!(
            $expanded_name,
            Val::List,
            REPEATED_NON_LIST,
            // TODO: Verify lambda is inlined.
            |tag: u64, value: &Vec<Val>, _lengths: &mut Vec<u32>, buf: &mut EncodeBuf<'_>| {
                for item in value.iter() {
                    if let $type(value) = item {
                        encode_varint(tag, buf);
                        $encode_inner(value, buf)?;
                    } else {
                        return Err(EncodeError::new($type_error));
                    }
                }
                Ok(())
            },
        );
    };
}
macro_rules! scalar_fixed_length_fns {
    ($fixed_length:expr, $default:ident, $type:path, $type_error:expr, $explicit_name:ident, $implicit_name:ident, $packed_name:ident, $expanded_name:ident,) => {
        length_fn!(
            $explicit_name,
            Val::Option,
            EXPLICIT_NON_OPTION,
            // TODO: Verify lambda is inlined.
            |tag: u64, value: &Option<Box<Val>>, _lengths: &mut Vec<u32>| {
                Ok(if value.is_some() {
                    u32::saturating_add($fixed_length, encoded_len_varint(tag) as u32)
                } else {
                    0
                })
            },
        );

        length_fn!(
            $implicit_name,
            $type,
            $type_error,
            // TODO: Verify lambda is inlined.
            |tag: u64, value, _lengths: &mut Vec<u32>| {
                Ok(if !$default(value) {
                    u32::saturating_add($fixed_length, encoded_len_varint(tag) as u32)
                } else {
                    0
                })
            },
        );

        length_fn!(
            $packed_name,
            Val::List,
            REPEATED_NON_LIST,
            // Packed fields always push a length.
            |tag: u64, value: &Vec<Val>, lengths: &mut Vec<u32>| {
                Ok(if value.len() > 0 {
                    let total = u32::try_from(value.len() * $fixed_length).unwrap_or(u32::MAX);
                    lengths.push(total as u32);
                    u32::saturating_add(
                        total,
                        (encoded_len_varint(tag) + encoded_len_varint(total as u64)) as u32,
                    )
                } else {
                    0
                })
            },
        );

        length_fn!(
            $expanded_name,
            Val::List,
            REPEATED_NON_LIST,
            // Expanded fields never push a length.
            |tag: u64, value: &Vec<Val>, _lengths: &mut Vec<u32>| {
                Ok(
                    u32::try_from((encoded_len_varint(tag) + $fixed_length) * value.len())
                        .unwrap_or(u32::MAX),
                )
            },
        );
    };
}
macro_rules! scalar_var_length_fns {
    ($length_inner:ident, $default:ident, $type:path, $type_error:expr, $explicit_name:ident, $implicit_name:ident, $packed_name:ident, $expanded_name:ident,) => {
        length_fn!(
            $explicit_name,
            Val::Option,
            EXPLICIT_NON_OPTION,
            // TODO: Verify lambda is inlined.
            |tag: u64, value: &Option<Box<Val>>, _lengths: &mut Vec<u32>| {
                Ok(if let Some(item) = value {
                    if let $type(value) = item.as_ref() {
                        u32::saturating_add($length_inner(value)?, encoded_len_varint(tag) as u32)
                    } else {
                        return Err(EncodeError::new($type_error));
                    }
                } else {
                    0
                })
            },
        );

        length_fn!(
            $implicit_name,
            $type,
            $type_error,
            // TODO: Verify inlining.
            |tag: u64, value, _lengths: &mut Vec<u32>| {
                Ok(if !$default(value) {
                    u32::saturating_add($length_inner(value)?, encoded_len_varint(tag) as u32)
                } else {
                    0
                })
            },
        );

        length_fn!(
            $packed_name,
            Val::List,
            REPEATED_NON_LIST,
            // Packed fields always push a length.
            |tag: u64, value: &Vec<Val>, lengths: &mut Vec<u32>| {
                Ok(if value.len() > 0 {
                    let mut total = 0;
                    for item in value.iter() {
                        if let $type(value) = item {
                            total = u32::saturating_add(total, $length_inner(value)?);
                        } else {
                            return Err(EncodeError::new($type_error));
                        }
                    }
                    lengths.push(total);
                    u32::saturating_add(
                        total,
                        (encoded_len_varint(tag) + encoded_len_varint(total as u64)) as u32,
                    )
                } else {
                    0
                })
            },
        );

        length_fn!(
            $expanded_name,
            Val::List,
            REPEATED_NON_LIST,
            // Expanded fields never push a length.
            |tag: u64, value: &Vec<Val>, _lengths: &mut Vec<u32>| {
                let tag_length = encoded_len_varint(tag) as u32;
                let mut total = 0;
                for item in value.iter() {
                    if let $type(value) = item {
                        total = u32::saturating_add(
                            total,
                            u32::saturating_add($length_inner(value)?, tag_length),
                        );
                    } else {
                        return Err(EncodeError::new($type_error));
                    }
                }
                Ok(total)
            },
        );
    };
}

#[inline(always)]
fn bytes_encode_inner(value: &Vec<Val>, buf: &mut EncodeBuf<'_>) -> Result<(), EncodeError> {
    encode_varint(value.len() as u64, buf);
    for item in value.iter() {
        if let Val::U8(byte) = item {
            buf.put_u8(*byte);
        } else {
            return Err(EncodeError::new(BYTE_NON_BYTE));
        }
    }
    Ok(())
}
#[inline(always)]
fn bytes_length_inner(value: &Vec<Val>) -> Result<u32, EncodeError> {
    Ok(u32::saturating_add(
        encoded_len_varint(value.len() as u64) as u32,
        u32::try_from(value.len()).unwrap_or(u32::MAX),
    ))
}
#[inline(always)]
fn bytes_default(value: &Vec<Val>) -> bool {
    value.is_empty()
}
scalar_encode_fns!(
    bytes_encode_inner,
    bytes_default,
    Val::List,
    BYTES_NON_LIST,
    bytes_explicit_encode,
    bytes_implicit_encode,
    // Overridden because bytes cannot be packed.
    _bytes_packed_encode_unused,
    bytes_expanded_encode,
);
scalar_var_length_fns!(
    bytes_length_inner,
    bytes_default,
    Val::List,
    BYTES_NON_LIST,
    bytes_explicit_length,
    bytes_implicit_length,
    // Overridden because bytes cannot be packed.
    _bytes_packed_length_unused,
    bytes_expanded_length,
);

// Bytes and string values cannot be packed, only expanded.
fn bytes_packed_encode(
    _encoder: &Encoder,
    _value: &Val,
    _lengths: &mut Vec<u32>,
    _buf: &mut EncodeBuf<'_>,
) -> Result<(), EncodeError> {
    Err(EncodeError::new(IMPOSSIBLE_PACKED_BYTES))
}
fn bytes_packed_length(
    _encoder: &Encoder,
    _value: &Val,
    _lengths: &mut Vec<u32>,
) -> Result<u32, EncodeError> {
    Err(EncodeError::new(IMPOSSIBLE_PACKED_BYTES))
}

#[inline(always)]
fn string_encode_inner(value: &String, buf: &mut EncodeBuf<'_>) -> Result<(), EncodeError> {
    encode_varint(value.len() as u64, buf);
    buf.put_slice(value.as_bytes());
    Ok(())
}
#[inline(always)]
fn string_length_inner(value: &String) -> Result<u32, EncodeError> {
    Ok(u32::saturating_add(
        encoded_len_varint(value.len() as u64) as u32,
        u32::try_from(value.len()).unwrap_or(u32::MAX),
    ))
}
#[inline(always)]
fn string_default(value: &String) -> bool {
    value.is_empty()
}
scalar_encode_fns!(
    string_encode_inner,
    string_default,
    Val::String,
    STRING_NON_STRING,
    string_explicit_encode,
    string_implicit_encode,
    // Overridden because strings cannot be packed.
    _string_packed_encode_unused,
    string_expanded_encode,
);
scalar_var_length_fns!(
    string_length_inner,
    string_default,
    Val::String,
    STRING_NON_STRING,
    string_explicit_length,
    string_implicit_length,
    // Overridden because strings cannot be packed.
    _string_packed_length_unused,
    string_expanded_length,
);

// Bytes and string values cannot be packed, only expanded.
fn string_packed_encode(
    _encoder: &Encoder,
    _value: &Val,
    _lengths: &mut Vec<u32>,
    _buf: &mut EncodeBuf<'_>,
) -> Result<(), EncodeError> {
    Err(EncodeError::new(IMPOSSIBLE_PACKED_STRING))
}
fn string_packed_length(
    _encoder: &Encoder,
    _value: &Val,
    _lengths: &mut Vec<u32>,
) -> Result<u32, EncodeError> {
    Err(EncodeError::new(IMPOSSIBLE_PACKED_STRING))
}

#[inline(always)]
fn bool_encode_inner(value: &bool, buf: &mut EncodeBuf<'_>) -> Result<(), EncodeError> {
    buf.put_u8(*value as u8);
    Ok(())
}
#[inline(always)]
fn bool_default(value: &bool) -> bool {
    !*value // False is the default; return true if false.
}
scalar_encode_fns!(
    bool_encode_inner,
    bool_default,
    Val::Bool,
    BOOL_NON_BOOL,
    bool_explicit_encode,
    bool_implicit_encode,
    bool_packed_encode,
    bool_expanded_encode,
);
scalar_fixed_length_fns!(
    1,
    bool_default,
    Val::Bool,
    BOOL_NON_BOOL,
    bool_explicit_length,
    bool_implicit_length,
    bool_packed_length,
    bool_expanded_length,
);

#[inline(always)]
fn int32_encode_inner(value: &i32, buf: &mut EncodeBuf<'_>) -> Result<(), EncodeError> {
    encode_varint(*value as u64, buf);
    Ok(())
}
#[inline(always)]
fn int32_length_inner(value: &i32) -> Result<u32, EncodeError> {
    Ok(encoded_len_varint(*value as u64) as u32)
}
#[inline(always)]
fn int32_default(value: &i32) -> bool {
    *value == 0
}
scalar_encode_fns!(
    int32_encode_inner,
    int32_default,
    Val::S32,
    INT32_NON_INT32,
    int32_explicit_encode,
    int32_implicit_encode,
    int32_packed_encode,
    int32_expanded_encode,
);
scalar_var_length_fns!(
    int32_length_inner,
    int32_default,
    Val::S32,
    INT32_NON_INT32,
    int32_explicit_length,
    int32_implicit_length,
    int32_packed_length,
    int32_expanded_length,
);

#[inline(always)]
fn sint32_encode_inner(value: &i32, buf: &mut EncodeBuf<'_>) -> Result<(), EncodeError> {
    encode_varint(((*value << 1) ^ (*value >> 31)) as u32 as u64, buf);
    Ok(())
}
#[inline(always)]
fn sint32_length_inner(value: &i32) -> Result<u32, EncodeError> {
    Ok(encoded_len_varint(((*value << 1) ^ (*value >> 31)) as u32 as u64) as u32)
}
scalar_encode_fns!(
    sint32_encode_inner,
    int32_default,
    Val::S32,
    SINT32_NON_SINT32,
    sint32_explicit_encode,
    sint32_implicit_encode,
    sint32_packed_encode,
    sint32_expanded_encode,
);
scalar_var_length_fns!(
    sint32_length_inner,
    int32_default,
    Val::S32,
    SINT32_NON_SINT32,
    sint32_explicit_length,
    sint32_implicit_length,
    sint32_packed_length,
    sint32_expanded_length,
);

#[inline(always)]
fn sfixed32_encode_inner(value: &i32, buf: &mut EncodeBuf<'_>) -> Result<(), EncodeError> {
    buf.put_i32_le(*value);
    Ok(())
}
scalar_encode_fns!(
    sfixed32_encode_inner,
    int32_default,
    Val::S32,
    SFIXED32_NON_SFIXED32,
    sfixed32_explicit_encode,
    sfixed32_implicit_encode,
    sfixed32_packed_encode,
    sfixed32_expanded_encode,
);
scalar_fixed_length_fns!(
    4,
    int32_default,
    Val::S32,
    SFIXED32_NON_SFIXED32,
    sfixed32_explicit_length,
    sfixed32_implicit_length,
    sfixed32_packed_length,
    sfixed32_expanded_length,
);

#[inline(always)]
fn uint32_encode_inner(value: &u32, buf: &mut EncodeBuf<'_>) -> Result<(), EncodeError> {
    encode_varint(*value as u64, buf);
    Ok(())
}
#[inline(always)]
fn uint32_length_inner(value: &u32) -> Result<u32, EncodeError> {
    Ok(encoded_len_varint(*value as u64) as u32)
}
#[inline(always)]
fn uint32_default(value: &u32) -> bool {
    *value == 0
}
scalar_encode_fns!(
    uint32_encode_inner,
    uint32_default,
    Val::U32,
    UINT32_NON_UINT32,
    uint32_explicit_encode,
    uint32_implicit_encode,
    uint32_packed_encode,
    uint32_expanded_encode,
);
scalar_var_length_fns!(
    uint32_length_inner,
    uint32_default,
    Val::U32,
    UINT32_NON_UINT32,
    uint32_explicit_length,
    uint32_implicit_length,
    uint32_packed_length,
    uint32_expanded_length,
);

#[inline(always)]
fn fixed32_encode_inner(value: &u32, buf: &mut EncodeBuf<'_>) -> Result<(), EncodeError> {
    buf.put_u32_le(*value);
    Ok(())
}
scalar_encode_fns!(
    fixed32_encode_inner,
    uint32_default,
    Val::U32,
    FIXED32_NON_FIXED32,
    fixed32_explicit_encode,
    fixed32_implicit_encode,
    fixed32_packed_encode,
    fixed32_expanded_encode,
);
scalar_fixed_length_fns!(
    4,
    uint32_default,
    Val::U32,
    FIXED32_NON_FIXED32,
    fixed32_explicit_length,
    fixed32_implicit_length,
    fixed32_packed_length,
    fixed32_expanded_length,
);

#[inline(always)]
fn int64_encode_inner(value: &i64, buf: &mut EncodeBuf<'_>) -> Result<(), EncodeError> {
    encode_varint(*value as u64, buf);
    Ok(())
}
#[inline(always)]
fn int64_length_inner(value: &i64) -> Result<u32, EncodeError> {
    Ok(encoded_len_varint(*value as u64) as u32)
}
#[inline(always)]
fn int64_default(value: &i64) -> bool {
    *value == 0
}
scalar_encode_fns!(
    int64_encode_inner,
    int64_default,
    Val::S64,
    INT64_NON_INT64,
    int64_explicit_encode,
    int64_implicit_encode,
    int64_packed_encode,
    int64_expanded_encode,
);
scalar_var_length_fns!(
    int64_length_inner,
    int64_default,
    Val::S64,
    INT64_NON_INT64,
    int64_explicit_length,
    int64_implicit_length,
    int64_packed_length,
    int64_expanded_length,
);

#[inline(always)]
fn sint64_encode_inner(value: &i64, buf: &mut EncodeBuf<'_>) -> Result<(), EncodeError> {
    encode_varint(((*value << 1) ^ (*value >> 63)) as u64, buf);
    Ok(())
}
#[inline(always)]
fn sint64_length_inner(value: &i64) -> Result<u32, EncodeError> {
    Ok(encoded_len_varint(((*value << 1) ^ (*value >> 63)) as u64) as u32)
}
scalar_encode_fns!(
    sint64_encode_inner,
    int64_default,
    Val::S64,
    SINT64_NON_SINT64,
    sint64_explicit_encode,
    sint64_implicit_encode,
    sint64_packed_encode,
    sint64_expanded_encode,
);
scalar_var_length_fns!(
    sint64_length_inner,
    int64_default,
    Val::S64,
    SINT64_NON_SINT64,
    sint64_explicit_length,
    sint64_implicit_length,
    sint64_packed_length,
    sint64_expanded_length,
);

#[inline(always)]
fn sfixed64_encode_inner(value: &i64, buf: &mut EncodeBuf<'_>) -> Result<(), EncodeError> {
    buf.put_i64_le(*value);
    Ok(())
}
scalar_encode_fns!(
    sfixed64_encode_inner,
    int64_default,
    Val::S64,
    SFIXED64_NON_SFIXED64,
    sfixed64_explicit_encode,
    sfixed64_implicit_encode,
    sfixed64_packed_encode,
    sfixed64_expanded_encode,
);
scalar_fixed_length_fns!(
    8,
    int64_default,
    Val::S64,
    SFIXED64_NON_SFIXED64,
    sfixed64_explicit_length,
    sfixed64_implicit_length,
    sfixed64_packed_length,
    sfixed64_expanded_length,
);

#[inline(always)]
fn uint64_encode_inner(value: &u64, buf: &mut EncodeBuf<'_>) -> Result<(), EncodeError> {
    encode_varint(*value, buf);
    Ok(())
}
#[inline(always)]
fn uint64_length_inner(value: &u64) -> Result<u32, EncodeError> {
    Ok(encoded_len_varint(*value) as u32)
}
#[inline(always)]
fn uint64_default(value: &u64) -> bool {
    *value == 0
}
scalar_encode_fns!(
    uint64_encode_inner,
    uint64_default,
    Val::U64,
    UINT64_NON_UINT64,
    uint64_explicit_encode,
    uint64_implicit_encode,
    uint64_packed_encode,
    uint64_expanded_encode,
);
scalar_var_length_fns!(
    uint64_length_inner,
    uint64_default,
    Val::U64,
    UINT64_NON_UINT64,
    uint64_explicit_length,
    uint64_implicit_length,
    uint64_packed_length,
    uint64_expanded_length,
);

#[inline(always)]
fn fixed64_encode_inner(value: &u64, buf: &mut EncodeBuf<'_>) -> Result<(), EncodeError> {
    buf.put_u64_le(*value);
    Ok(())
}
scalar_encode_fns!(
    fixed64_encode_inner,
    uint64_default,
    Val::U64,
    FIXED64_NON_FIXED64,
    fixed64_explicit_encode,
    fixed64_implicit_encode,
    fixed64_packed_encode,
    fixed64_expanded_encode,
);
scalar_fixed_length_fns!(
    8,
    uint64_default,
    Val::U64,
    FIXED64_NON_FIXED64,
    fixed64_explicit_length,
    fixed64_implicit_length,
    fixed64_packed_length,
    fixed64_expanded_length,
);

#[inline(always)]
fn float_encode_inner(value: &f32, buf: &mut EncodeBuf<'_>) -> Result<(), EncodeError> {
    buf.put_f32_le(*value);
    Ok(())
}
#[inline(always)]
fn float_default(value: &f32) -> bool {
    *value == 0.0
}
scalar_encode_fns!(
    float_encode_inner,
    float_default,
    Val::Float32,
    FLOAT_NON_FLOAT,
    float_explicit_encode,
    float_implicit_encode,
    float_packed_encode,
    float_expanded_encode,
);
scalar_fixed_length_fns!(
    4,
    float_default,
    Val::Float32,
    FLOAT_NON_FLOAT,
    float_explicit_length,
    float_implicit_length,
    float_packed_length,
    float_expanded_length,
);

#[inline(always)]
fn double_encode_inner(value: &f64, buf: &mut EncodeBuf<'_>) -> Result<(), EncodeError> {
    buf.put_f64_le(*value);
    Ok(())
}
#[inline(always)]
fn double_default(value: &f64) -> bool {
    *value == 0.0
}
scalar_encode_fns!(
    double_encode_inner,
    double_default,
    Val::Float64,
    DOUBLE_NON_DOUBLE,
    double_explicit_encode,
    double_implicit_encode,
    double_packed_encode,
    double_expanded_encode,
);
scalar_fixed_length_fns!(
    8,
    double_default,
    Val::Float64,
    DOUBLE_NON_DOUBLE,
    double_explicit_length,
    double_implicit_length,
    double_packed_length,
    double_expanded_length,
);
