//! Common definitions for decoders.
#![feature(core_intrinsics)]

use std::intrinsics::unlikely;
use std::result::Result as StdResult;

use prost::bytes::Buf;
use prost::encoding::{decode_varint as tonic_decode_varint, WireType};
use tonic::codec::DecodeBuf;
use tonic::{Code, Status};
use wasmtime::component::Val;

/// Similar to Tonic's [`Decoder`](tonic::codec::Decoder)
/// except the item and error types are static and `decode` takes an immutable reference.
/// [Context](https://users.rust-lang.org/t/immutable-reference-to-mutable-reference/122770).
pub trait Decoder {
    fn decode(&self, src: &mut DecodeBuf<'_>) -> StdResult<Option<Val>, Status>;
}

#[inline(always)]
pub fn invalid_varint() -> Status {
    invalid_argument("Invalid varint")
}

#[inline(always)]
pub fn invalid_utf8() -> Status {
    invalid_argument("Invalid UTF-8")
}

#[inline(always)]
pub fn buffer_overflow() -> Status {
    invalid_argument("Buffer overflow")
}

#[inline(always)]
pub fn invalid_tag(tag: u64) -> Status {
    invalid_argument(format!("Invalid tag: {tag}"))
}

#[inline(always)]
fn invalid_argument<S: Into<String>>(msg: S) -> Status {
    Status::new(Code::InvalidArgument, msg)
}

#[inline(always)]
pub fn decode_varint(src: &mut DecodeBuf<'_>) -> StdResult<u64, Status> {
    tonic_decode_varint(src).map_err(|_| invalid_varint())
}

#[inline(always)]
pub fn wiretype_from_tag(tag: u64) -> StdResult<WireType, Status> {
    WireType::try_from(tag & 0x07).map_err(|_| invalid_argument("Invalid wire type"))
}

/// Read a varint from the source buffer,
/// check that there are at least as many bytes left in the buffer,
/// then return that varint.
#[inline(always)]
pub fn read_length_check_overflow(src: &mut DecodeBuf<'_>) -> StdResult<u64, Status> {
    let len = decode_varint(src)?;
    if unlikely(len > src.remaining() as u64) {
        Err(buffer_overflow())
    } else {
        Ok(len)
    }
}

/// Define two functions:
///
/// The first, called `$packed_name`, decodes a packed repeated field
/// by iteratively invoking the function called `$item_decoder` to decode individual items.
///
/// The other, called `$explicit_name`, decodes a field with explicit presence tracking
/// by invoking the function called `$item_decoder` to decode an item,
/// then wrap it as a component option (always present).
/// Explicit fields that are never decoded would instead use the empty option by default.
#[macro_export]
macro_rules! derive_decoders {
    ($item_decoder:ident, $packed_name:ident, $explicit_name:ident) => {
        pub fn $packed_name(src: &mut DecodeBuf<'_>) -> StdResult<Val, Status> {
            let mut count = decode_varint(src)?;

            let mut list = Vec::new();
            while count > 0 {
                list.push($item_decoder(src)?);
                count -= 1;
            }

            Ok(Val::List(list))
        }

        pub fn $explicit_name(src: &mut DecodeBuf<'_>) -> StdResult<Val, Status> {
            match $item_decoder(src) {
                Ok(val) => Ok(Val::Option(Some(Box::new(val)))),
                err => err,
            }
        }
    };
}
