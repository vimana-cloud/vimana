//! Decode outgoing responses into Wasm component record values.

#![feature(box_as_ptr)]
#![feature(vec_push_within_capacity)]

mod compound;
mod scalar;

use std::collections::HashMap;
use std::fmt::{Debug, Display, Formatter, Result as FmtResult, Write};
use std::mem::ManuallyDrop;
use std::ptr::fn_addr_eq;
use std::result::Result as StdResult;
use std::sync::Arc;

use anyhow::Result;
use metadata_proto::vimana::runtime::ProtoMessage;
use prost::encoding::WireType;
use tonic::Status;
use tonic::codec::{EncodeBuf, Encoder as TonicEncoder};
use wasmtime::component::Val;

use compound::{
    compile_response_encoder, count_response_encoder_messages, message_inner_encode,
    message_inner_length,
};
use logging::log_error;
use names::ComponentName;

/// Encodes a top-level response message (*without* tag or length).
///
/// Reference-counted because Tonic's [codec](tonic::codec::Codec)
/// demands an owned encoder for each request, which is achieved by cloning.
#[derive(Clone)]
pub struct ResponseEncoder(Arc<ResponseEncoderInner>);

/// See [`ResponseEncoder`].
struct ResponseEncoderInner {
    /// Array of encoders for all messages that may appear within the response.
    /// The first element encodes the overall response message itself
    /// (the length is always at least 1).
    /// Any additional encoders would encode nested messages within the response.
    ///
    /// Using a vector allows us to represent self-referential message types,
    /// which use pointers to encoders within this vector to refer to one another (or themselves).
    /// That makes this a self-referential data structure.
    messages: Vec<MessageEncoder>,

    /// Component name used for error logging only,
    /// shared among all encoders / decoders of a single component.
    component: Arc<ComponentName>,
}

struct MessageEncoder {
    /// Map from subfield names to field encoders.
    fields: HashMap<String, Encoder>,
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

    /// Information for encoding compound types (messages, oneofs, enumerations).
    /// Ignored for scalar types.
    compound: CompoundEncoder,
}

/// Information for an [`Encoder`] for compound types (messages, oneofs, enumerations).
///
/// Implemented as a union to save space while keeping a known size.
/// Each specific encoding function will know how to deal with this appropriately,
/// but we also have to manually drop the appropriate one in [`Encoder::drop`].
union CompoundEncoder {
    /// Encoder for nested messages.
    /// This is a raw pointer into the [`messages`](ResponseEncoderInner::messages) field
    /// of the surrounding [response encoder](ResponseEncoderInner).
    message: *const MessageEncoder,

    /// Enumeration variants.
    enum_variants: ManuallyDrop<HashMap<String, u32>>,

    /// Map from subfield names to encoders for oneofs.
    oneof_variants: ManuallyDrop<HashMap<String, Encoder>>,

    /// Set this placeholder value for scalars.
    scalar: (),
}

/// Encode the [value](Val) to the [buffer](EncodeBuf)
/// given the pre-computed lengths of its constituent parts.
/// Each implementation should be specific to a certain Protobuf type.
type EncodeFn = fn(
    encoder: &Encoder,
    value: &Val,
    lengths: &mut Vec<u32>,
    buf: &mut EncodeBuf<'_>,
) -> StdResult<(), EncodeError>;

/// Pre-compute the vector of sub-lengths for the given value, including subfields, recursively,
/// for [encoding](EncodeFn) to consume.
/// Returns the total length of the serialized value, including the leading tag and length.
type LengthFn =
    fn(encoder: &Encoder, value: &Val, lengths: &mut Vec<u32>) -> StdResult<u32, EncodeError>;

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
    pub fn new(
        messages: &Vec<ProtoMessage>,
        response_index: u32,
        component: Arc<ComponentName>,
    ) -> Result<Self> {
        let response_index = response_index as usize;
        let mut index_map: HashMap<usize, usize> = HashMap::new();
        count_response_encoder_messages(messages, response_index, &mut index_map)?;

        // Allocate space for the relevant message encoders up front so it's never resized
        // and pointers into the vector can be stable.
        let mut message_encoders: Vec<MessageEncoder> = Vec::with_capacity(index_map.len());
        compile_response_encoder(messages, response_index, &mut message_encoders, &index_map)?;

        // Sanity checks.
        debug_assert!(message_encoders.len() == index_map.len());
        debug_assert!(message_encoders.len() == message_encoders.capacity());

        Ok(Self(Arc::new(ResponseEncoderInner {
            messages: message_encoders,
            component,
        })))
    }
}

impl TonicEncoder for ResponseEncoder {
    type Item = Val;
    type Error = Status;

    /// Encode a message to a writable buffer.
    fn encode(&mut self, item: Self::Item, dst: &mut EncodeBuf<'_>) -> Result<(), Self::Error> {
        let field_encoders = &self.0.messages[0].fields;
        // TODO: Pre-allocate some space for lengths?
        let mut lengths = Vec::new();
        let result = message_inner_length(field_encoders, &item, &mut lengths)
            .and_then(|_length| message_inner_encode(field_encoders, &item, &mut lengths, dst))
            .map_err(|error| {
                // An encoding error indicates that the Wasm component returned an invalid value.
                // Report this as an INTERNAL status to the caller and log it,
                // because the implementation should have been checked for type correctness.
                // The debug format includes details.
                log_error!(component: self.0.component.as_ref(), "{:?}", error);
                // The display format does *not* include details.
                Status::internal(error.to_string())
            });
        // In tests, make sure we used all the pre-computed lengths as expected.
        debug_assert!(lengths.is_empty());
        result
    }
}

/// Safe because the raw pointers in the encoder always point
/// into the attached [vector](ResponseEncoderInner::messages),
/// which is never modified or re-allocated after initialization.
unsafe impl Send for ResponseEncoderInner {}

/// Safe because the raw pointers in the encoder always point
/// into the attached [vector](ResponseEncoderInner::messages),
/// which is never modified or re-allocated after initialization.
unsafe impl Sync for ResponseEncoderInner {}

/// [`Encoder`] uses a union internally,
/// which requires the hash maps to be dropped manually.
impl Drop for Encoder {
    fn drop(&mut self) {
        // Encoders are dropped when a container shuts down (infrequently)
        // so we can exhaustively check against the known compound encoding functions
        // to figure out which hash map needs to get dropped.
        if fn_addr_eq(self.encode, compound::enum_explicit_encode as EncodeFn)
            || fn_addr_eq(self.encode, compound::enum_implicit_encode as EncodeFn)
            || fn_addr_eq(self.encode, compound::enum_packed_encode as EncodeFn)
            || fn_addr_eq(self.encode, compound::enum_expanded_encode as EncodeFn)
        {
            unsafe {
                ManuallyDrop::drop(&mut self.compound.enum_variants);
            }
        } else if fn_addr_eq(self.encode, compound::oneof_encode as EncodeFn) {
            unsafe {
                ManuallyDrop::drop(&mut self.compound.oneof_variants);
            }
        }
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

/// Return whether the given `ScalarCoding` uses explicit presence tracking.
#[inline(always)]
fn explicit_scalar(scalar_coding: i32) -> bool {
    // Explicit scalar coding numbers all happen to equal `4n+2` for some `n`.
    scalar_coding % 4 == 2
}

/// When returning an error status to a client,
/// an encoding error should be displayed like this:
///     Response serialization error
///
/// This represents an unexpected situation,
/// since the Wasm component should have been verified to return the correct type.
/// More details can be found by searching the logs for `EncodeError`.
impl Display for EncodeError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> FmtResult {
        formatter.write_str("Response serialization error")
    }
}

/// When logging, an encoding error should be displayed like this:
///     EncodeError(.path.to[0][4].the.field): <message>
impl Debug for EncodeError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> FmtResult {
        formatter.write_str("EncodeError(")?;
        format_encode_error_trace(self, formatter)?;
        formatter.write_str("): ")?;
        formatter.write_str(self.message)
    }
}

#[inline(always)]
fn format_encode_error_trace(error: &EncodeError, formatter: &mut Formatter<'_>) -> FmtResult {
    for level in error.traceback.iter().rev() {
        match level {
            EncodeLevel::Field(name) => {
                formatter.write_char('.')?;
                formatter.write_str(name)?;
            }
            EncodeLevel::Index(index) => {
                formatter.write_char('[')?;
                Display::fmt(index, formatter)?;
                formatter.write_char(']')?;
            }
        }
    }
    Ok(())
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
