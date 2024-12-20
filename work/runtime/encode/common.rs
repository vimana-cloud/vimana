//! Common definitions for encoders.
#![feature(core_intrinsics)]

use std::intrinsics::unlikely;
use std::result::Result as StdResult;

use prost::bytes::Buf;
use prost::encoding::{decode_varint as tonic_decode_varint, WireType};
use tonic::codec::EncodeBuf;
use tonic::{Code, Status};
use wasmtime::component::Val;

/// Similar to Tonic's [`Encoder`](tonic::codec::Encoder)
/// except the item and error types are static and `encode` takes an immutable reference.
/// [Context](https://users.rust-lang.org/t/immutable-reference-to-mutable-reference/122770).
pub trait Encoder {
    fn encode(&self, item: Val, src: &mut EncodeBuf<'_>) -> StdResult<(), Status>;
}
