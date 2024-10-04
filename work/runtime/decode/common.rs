//! Common definitions for decoders.
#![feature(core_intrinsics)]

use std::intrinsics::unlikely;
use std::io::{Error as IoError, ErrorKind as IoErrorKind};
use std::result::Result as StdResult;

use prost::bytes::Buf;
use prost::encoding::decode_varint;
use tonic::codec::{DecodeBuf, Decoder as TonicDecoder};
use wasmtime::component::Val;

/// A decoder in Actio is anything that decodes directly into a Wasm component value.
pub type ActioDecoder = Box<dyn TonicDecoder<Item = Val, Error = IoError>>;

pub fn invalid_data<S: Into<String>>(msg: S) -> IoError {
    IoError::new(IoErrorKind::InvalidData, msg.into())
}

/// Read a varint from the source buffer and check that there are at least as many bytes
/// left in the buffer.
#[inline(always)]
pub fn read_length_check_overflow(src: &mut DecodeBuf<'_>) -> StdResult<u64, IoError> {
    let len = decode_varint(src)?;
    if unlikely(len > src.remaining() as u64) {
        Err(invalid_data("Buffer overflow"))
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
        pub fn $packed_name(src: &mut DecodeBuf<'_>) -> StdResult<Val, IoError> {
            let mut count = decode_varint(src)?;

            let mut list = Vec::new();
            while count > 0 {
                list.push($item_decoder(src)?);
                count -= 1;
            }

            Ok(Val::List(list))
        }

        pub fn $explicit_name(src: &mut DecodeBuf<'_>) -> StdResult<Val, IoError> {
            match $item_decoder(src) {
                Ok(val) => Ok(Val::Option(Some(Box::new(val)))),
                err => err,
            }
        }
    };
}
