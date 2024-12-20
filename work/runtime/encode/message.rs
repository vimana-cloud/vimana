//! Logic to encode protobuf messages directly from Wasm component values.
#![feature(core_intrinsics)]

use std::collections::HashMap;
use std::intrinsics::{likely, unlikely};
use std::io::ErrorKind as IoErrorKind;
use std::result::Result as StdResult;

use tonic::codec::{EncodeBuf, Encoder as TonicEncoder};
use tonic::Status;
use wasmtime::component::Val;

use common::Encoder;
use error::{Error, Result};
use grpc_container_proto::work::runtime::grpc_metadata::method::Field;

/// An encoder in Vimana is anything that encodes directly from a Wasm component value.
type DynamicEncoder = Box<dyn Encoder + Sync + Send>;

/// Map from component field names
/// to field tags (wire type and field number) and field encoders.
type MessageEncoderFields = HashMap<String, (u64, DynamicEncoder)>;

/// Encoder for a protobuf message.
pub struct MessageEncoder {
    fields: MessageEncoderFields,
}

impl MessageEncoder {
    /// Construct a new [`MessageDecoder`] from the given [`Field`] representing a message type.
    /// Only the [subfields](Field::subfields) are significant.
    pub fn new(message: &Field) -> Result<Self> {
        todo!()
    }
}

impl Encoder for MessageEncoder {
    fn encode(&self, item: Val, dst: &mut EncodeBuf<'_>) -> StdResult<(), Status> {
        Ok(())
    }
}

impl TonicEncoder for MessageEncoder {
    type Item = Val;
    type Error = Status;

    /// Encode a message to a writable buffer.
    fn encode(&mut self, item: Self::Item, dst: &mut EncodeBuf<'_>) -> StdResult<(), Self::Error> {
        Encoder::encode(self, item, dst)
    }
}
