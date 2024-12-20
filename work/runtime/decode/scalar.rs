//! Decoding logic for scalar protobuf fields
//! (anything besides messages, enums, and oneofs).
#![feature(core_intrinsics)]

use std::intrinsics::{likely, unlikely};
use std::result::Result as StdResult;

use prost::bytes::Buf;
use tonic::codec::DecodeBuf;
use tonic::Status;
use wasmtime::component::Val;

use common::{
    buffer_overflow, decode_varint, derive_decoders, invalid_utf8, read_length_check_overflow,
    Decoder,
};
use error::{Error, Result};
use grpc_container_proto::work::runtime::grpc_metadata::method::field::{
    Coding, CompoundCoding, ScalarCoding,
};

type ScalarDecoderFn = fn(&mut DecodeBuf<'_>) -> StdResult<Val, Status>;

pub struct ScalarDecoder {
    decode_fn: ScalarDecoderFn,
}

impl ScalarDecoder {
    /// Construct a new [`MessageDecoder`] for the given variant of [`ScalarCoding`]
    /// (given as its integer representation).
    pub fn new(coding_index: i32) -> Result<Self> {
        if let Err(enum_error) = ScalarCoding::try_from(coding_index) {
            return Err(Error::wrap("Unexpected enum value", enum_error));
        }

        Ok(Self {
            decode_fn: DECODE_FNS[coding_index as usize],
        })
    }
}

impl Decoder for ScalarDecoder {
    /// Decode a message from a buffer containing exactly the bytes of a full message.
    fn decode(&self, src: &mut DecodeBuf<'_>) -> StdResult<Option<Val>, Status> {
        (self.decode_fn)(src).map(Some)
    }
}

const DECODE_FNS: [ScalarDecoderFn; 64] = [
    decode_bytes_implicit,
    decode_bytes_packed,
    decode_bytes_explicit,
    decode_expanded,
    decode_string_utf8_implicit,
    decode_string_utf8_packed,
    decode_string_utf8_explicit,
    decode_expanded,
    decode_string_permissive_implicit,
    decode_string_permissive_packed,
    decode_string_permissive_explicit,
    decode_expanded,
    decode_bool_implicit,
    decode_bool_packed,
    decode_bool_explicit,
    decode_expanded,
    decode_int32_implicit,
    decode_int32_packed,
    decode_int32_explicit,
    decode_expanded,
    decode_sint32_implicit,
    decode_sint32_packed,
    decode_sint32_explicit,
    decode_expanded,
    decode_sfixed32_implicit,
    decode_sfixed32_packed,
    decode_sfixed32_explicit,
    decode_expanded,
    decode_uint32_implicit,
    decode_uint32_packed,
    decode_uint32_explicit,
    decode_expanded,
    decode_fixed32_implicit,
    decode_fixed32_packed,
    decode_fixed32_explicit,
    decode_expanded,
    decode_int64_implicit,
    decode_int64_packed,
    decode_int64_explicit,
    decode_expanded,
    decode_sint64_implicit,
    decode_sint64_packed,
    decode_sint64_explicit,
    decode_expanded,
    decode_sfixed64_implicit,
    decode_sfixed64_packed,
    decode_sfixed64_explicit,
    decode_expanded,
    decode_uint64_implicit,
    decode_uint64_packed,
    decode_uint64_explicit,
    decode_expanded,
    decode_fixed64_implicit,
    decode_fixed64_packed,
    decode_fixed64_explicit,
    decode_expanded,
    decode_float_implicit,
    decode_float_packed,
    decode_float_explicit,
    decode_expanded,
    decode_double_implicit,
    decode_double_packed,
    decode_double_explicit,
    decode_expanded,
];

fn decode_expanded(_src: &mut DecodeBuf<'_>) -> StdResult<Val, Status> {
    // This needs to be handled in an outer layer.
    todo!()
}

#[inline(always)]
fn decode_bytes_implicit(src: &mut DecodeBuf<'_>) -> StdResult<Val, Status> {
    let mut len = read_length_check_overflow(src)?;

    let mut bytes = Vec::new();
    while len > 0 {
        bytes.push(Val::U8(src.get_u8()));
        len -= 1;
    }

    Ok(Val::List(bytes))
}

derive_decoders!(
    decode_bytes_implicit,
    decode_bytes_packed,
    decode_bytes_explicit
);

#[inline(always)]
fn decode_string_utf8_implicit(src: &mut DecodeBuf<'_>) -> StdResult<Val, Status> {
    let len = read_length_check_overflow(src)?;

    // TODO: Investigate how much copying this does.
    let bytes = src.copy_to_bytes(len as usize);
    match String::from_utf8(bytes.to_vec()) {
        Ok(string) => Ok(Val::String(string)),
        Err(_utf8_error) => Err(invalid_utf8()),
    }
}

derive_decoders!(
    decode_string_utf8_implicit,
    decode_string_utf8_packed,
    decode_string_utf8_explicit
);

#[inline(always)]
fn decode_string_permissive_implicit(src: &mut DecodeBuf<'_>) -> StdResult<Val, Status> {
    let len = read_length_check_overflow(src)?;

    // TODO: Investigate how much copying this does.
    let bytes = src.copy_to_bytes(len as usize);
    Ok(Val::String(unsafe {
        String::from_utf8_unchecked(bytes.to_vec())
    }))
}

derive_decoders!(
    decode_string_permissive_implicit,
    decode_string_permissive_packed,
    decode_string_permissive_explicit
);

#[inline(always)]
fn decode_bool_implicit(src: &mut DecodeBuf<'_>) -> StdResult<Val, Status> {
    Ok(Val::Bool(decode_varint(src)? != 0))
}

derive_decoders!(
    decode_bool_implicit,
    decode_bool_packed,
    decode_bool_explicit
);

#[inline(always)]
fn decode_int32_implicit(src: &mut DecodeBuf<'_>) -> StdResult<Val, Status> {
    Ok(Val::S32(decode_varint(src)? as i32))
}

derive_decoders!(
    decode_int32_implicit,
    decode_int32_packed,
    decode_int32_explicit
);

#[inline(always)]
fn decode_sint32_implicit(src: &mut DecodeBuf<'_>) -> StdResult<Val, Status> {
    let raw_value = decode_varint(src)? as u32;
    Ok(Val::S32(
        ((raw_value >> 1) as i32) ^ (-((raw_value & 1) as i32)),
    ))
}

derive_decoders!(
    decode_sint32_implicit,
    decode_sint32_packed,
    decode_sint32_explicit
);

#[inline(always)]
fn decode_sfixed32_implicit(src: &mut DecodeBuf<'_>) -> StdResult<Val, Status> {
    if unlikely(src.remaining() < 4) {
        Err(buffer_overflow())
    } else {
        Ok(Val::S32(src.get_i32_le()))
    }
}

derive_decoders!(
    decode_sfixed32_implicit,
    decode_sfixed32_packed,
    decode_sfixed32_explicit
);

#[inline(always)]
fn decode_uint32_implicit(src: &mut DecodeBuf<'_>) -> StdResult<Val, Status> {
    Ok(Val::U32(decode_varint(src)? as u32))
}

derive_decoders!(
    decode_uint32_implicit,
    decode_uint32_packed,
    decode_uint32_explicit
);

#[inline(always)]
fn decode_fixed32_implicit(src: &mut DecodeBuf<'_>) -> StdResult<Val, Status> {
    if unlikely(src.remaining() < 4) {
        Err(buffer_overflow())
    } else {
        Ok(Val::U32(src.get_u32_le()))
    }
}

derive_decoders!(
    decode_fixed32_implicit,
    decode_fixed32_packed,
    decode_fixed32_explicit
);

#[inline(always)]
fn decode_int64_implicit(src: &mut DecodeBuf<'_>) -> StdResult<Val, Status> {
    Ok(Val::S64(decode_varint(src)? as i64))
}

derive_decoders!(
    decode_int64_implicit,
    decode_int64_packed,
    decode_int64_explicit
);

#[inline(always)]
fn decode_sint64_implicit(src: &mut DecodeBuf<'_>) -> StdResult<Val, Status> {
    let raw_value = decode_varint(src)?;
    Ok(Val::S64(
        ((raw_value >> 1) as i64) ^ (-((raw_value & 1) as i64)),
    ))
}

derive_decoders!(
    decode_sint64_implicit,
    decode_sint64_packed,
    decode_sint64_explicit
);

#[inline(always)]
fn decode_sfixed64_implicit(src: &mut DecodeBuf<'_>) -> StdResult<Val, Status> {
    if unlikely(src.remaining() < 8) {
        Err(buffer_overflow())
    } else {
        Ok(Val::S64(src.get_i64_le()))
    }
}

derive_decoders!(
    decode_sfixed64_implicit,
    decode_sfixed64_packed,
    decode_sfixed64_explicit
);

#[inline(always)]
fn decode_uint64_implicit(src: &mut DecodeBuf<'_>) -> StdResult<Val, Status> {
    Ok(Val::U64(decode_varint(src)?))
}

derive_decoders!(
    decode_uint64_implicit,
    decode_uint64_packed,
    decode_uint64_explicit
);

#[inline(always)]
fn decode_fixed64_implicit(src: &mut DecodeBuf<'_>) -> StdResult<Val, Status> {
    if unlikely(src.remaining() < 8) {
        Err(buffer_overflow())
    } else {
        Ok(Val::U64(src.get_u64_le()))
    }
}

derive_decoders!(
    decode_fixed64_implicit,
    decode_fixed64_packed,
    decode_fixed64_explicit
);

#[inline(always)]
fn decode_float_implicit(src: &mut DecodeBuf<'_>) -> StdResult<Val, Status> {
    if unlikely(src.remaining() < 4) {
        Err(buffer_overflow())
    } else {
        Ok(Val::Float32(src.get_f32_le()))
    }
}

derive_decoders!(
    decode_float_implicit,
    decode_float_packed,
    decode_float_explicit
);

#[inline(always)]
fn decode_double_implicit(src: &mut DecodeBuf<'_>) -> StdResult<Val, Status> {
    if unlikely(src.remaining() < 8) {
        Err(buffer_overflow())
    } else {
        Ok(Val::Float64(src.get_f64_le()))
    }
}

derive_decoders!(
    decode_double_implicit,
    decode_double_packed,
    decode_double_explicit
);
