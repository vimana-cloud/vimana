#![feature(box_as_ptr)]

mod compound;
mod scalar;

use std::collections::HashMap;
use std::fmt::{Debug, Display, Write};
use std::sync::Arc;

use metadata_proto::work::runtime::container::Field;
use prost::encoding::WireType;
use tonic::codec::{EncodeBuf, Encoder as TonicEncoder};
use tonic::Status;
use wasmtime::component::Val;

use error::log_error_status;
use names::ComponentName;

/// Encodes a top-level response message (*without* tag or length).
pub struct ResponseEncoder {
    /// Encodes the protobuf contents.
    inner: Encoder,

    /// Component name used for error messages only, shared to save memory.
    component: Arc<ComponentName>,
}

/// An instance of an encoder is essentially hard-wired
/// to encode component [values](Val) of a specific type
/// for any specific numbered Protobuf field.
struct Encoder {
    /// Encodes a value
    encode: EncodeFn,

    /// Return the length of the would-be-encoded value.
    length: LengthFn,

    /// Tag (field number and wire type).
    tag: u64,

    /// Map from component subfield names to encoders.
    /// Ignored for scalar values.
    subfields: HashMap<String, Encoder>,

    // TODO: Unionize these with subfields.
    /// Enumeration variants.
    variants: HashMap<String, u32>,
}

/// Encode the [value](Val) to the [buffer](EncodeBuf)
/// given the pre-computed [lengths](LengthQueue) of its constituent parts.
/// Each implementation should be specific to a certain Protobuf type.
type EncodeFn = fn(
    encoder: &Encoder,
    value: &Val,
    lengths: &mut Vec<u32>,
    buf: &mut EncodeBuf<'_>,
) -> Result<(), EncodeError>;

/// Pre-compute the queue of sub-lengths for the given value, and subfields recursively,
/// for [encoding](EncodeFn) to consume.
/// Returns the total length of the serialized value, including the leading tag and length.
type LengthFn =
    fn(encoder: &Encoder, value: &Val, lengths: &mut Vec<u32>) -> Result<u32, EncodeError>;

/// An error encountered during response encoding.
struct EncodeError {
    /// Basic error message.
    message: &'static str,

    /// Traceback of mutual recursion during encoding (most recent first).
    traceback: Vec<EncodeLevel>,
}

/// Represents a level of mutual recursion among compound subtypes
/// in an error traceback.
enum EncodeLevel {
    /// Message field.
    Field(String),
    /// Repeated field index.
    Index(usize),
}

impl ResponseEncoder {
    pub fn new(response: &Field, component: Arc<ComponentName>) -> Result<Self, Status> {
        Ok(Self {
            inner: Encoder::message_untagged(response, component.as_ref())?,
            component: component,
        })
    }
}

impl TonicEncoder for ResponseEncoder {
    type Item = Val;
    type Error = Status;

    /// Encode a message to a writable buffer.
    fn encode(&mut self, item: Self::Item, dst: &mut EncodeBuf<'_>) -> Result<(), Self::Error> {
        // TODO: Pre-allocate some space for lengths?
        let mut lengths = Vec::new();
        let result = (self.inner.length)(&self.inner, &item, &mut lengths)
            .and_then(|_length| (self.inner.encode)(&self.inner, &item, &mut lengths, dst))
            .map_err(log_error_status!("encode-error", &self.component));
        // In tests, make sure we used all the pre-computed lengths as expected.
        debug_assert!(lengths.is_empty());
        result
    }
}

impl EncodeError {
    #[cold]
    pub(crate) fn new(message: &'static str) -> Self {
        Self {
            message,
            traceback: Vec::new(),
        }
    }

    #[cold]
    pub(crate) fn with_field(mut self, name: String) -> Self {
        self.traceback.push(EncodeLevel::Field(name));
        self
    }

    #[cold]
    pub(crate) fn with_index(mut self, i: usize) -> Self {
        self.traceback.push(EncodeLevel::Index(i));
        self
    }
}

/// Given a Protobuf field number and wire type,
/// return the Protobuf field tag.
#[inline(always)]
fn tag(number: u32, wire_type: WireType) -> u64 {
    ((number as u64) << 3) | (wire_type as u64)
}

/// An encoding error should be displayed like this:
///   EncodeError(.path.to[0][4].the.field): <message>
impl Debug for EncodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("EncodeError(")?;
        for level in self.traceback.iter().rev() {
            match level {
                EncodeLevel::Field(name) => {
                    f.write_char('.')?;
                    f.write_str(name)?;
                }
                EncodeLevel::Index(index) => {
                    f.write_char('[')?;
                    Display::fmt(index, f)?;
                    f.write_char(']')?;
                }
            }
        }
        f.write_str("): ")?;
        f.write_str(&self.message)
    }
}

// All type mismatch error messages.
// These indicate that a component implementation
// is not compatible with its container metadata.
const NO_ENCODER_FOR_FIELD: &str = "Unexpected field name";
const MESSAGE_NON_OPTIONAL: &str = "Submessage is not optional";
const MESSAGE_NON_RECORD: &str = "Message is not a record";
const REPEATED_NON_LIST: &str = "Repeated field is not a list";
const EXPLICIT_NON_OPTION: &str = "Explicit field is not an option";
const BYTES_NON_LIST: &str = "Bytes field is not a list";
const BYTE_NON_BYTE: &str = "Byte item is not a byte";
const STRING_NON_STRING: &str = "String field is not a string";
const IMPOSSIBLE_PACKED_BYTES: &str = "Packed repeated bytes";
const IMPOSSIBLE_PACKED_STRING: &str = "Packed repeated string";
const BOOL_NON_BOOL: &str = "Boolean field is not boolean";
const INT32_NON_INT32: &str = "Int32 field is not S32";
const SINT32_NON_SINT32: &str = "Sint32 field is not S32";
const SFIXED32_NON_SFIXED32: &str = "Sfixed32 field is not S32";
const UINT32_NON_UINT32: &str = "Uint32 field is not U32";
const FIXED32_NON_FIXED32: &str = "Fixed32 field is not U32";
const INT64_NON_INT64: &str = "Int64 field is not S64";
const SINT64_NON_SINT64: &str = "Sint64 field is not S64";
const SFIXED64_NON_SFIXED64: &str = "Sfixed64 field is not S64";
const UINT64_NON_UINT64: &str = "Uint64 field is not U64";
const FIXED64_NON_FIXED64: &str = "Fixed64 field is not U64";
const FLOAT_NON_FLOAT: &str = "Float field is not Float32";
const DOUBLE_NON_DOUBLE: &str = "Double field is not Float64";
const ENUM_NON_ENUM: &str = "Enum field is not an enumeration";
const ENUM_VARIANT_UNRECOGNIZED: &str = "Unrecognized enum variant";
const ONEOF_NON_OPTIONAL: &str = "Oneof field is not optional";
const ONEOF_NON_VARIANT: &str = "Oneof field is not a variant";
const ONEOF_VARIANT_UNRECOGNIZED: &str = "Unrecognized oneof variant";
const ONEOF_VARIANT_NO_PAYLOAD: &str = "Oneof variant lacks a payload";

// This would indicate a fundamental issue with the algorithm
// that pre-computes the lengths of length-delimited fields for the encoder.
const LENGTH_INCONSISTENCY: &str = "Length pre-computation algorithm error";
