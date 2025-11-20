//! Decode incoming requests into Wasm component record values.

#![feature(push_mut)]
#![feature(cold_path)]

mod compound;
mod scalar;

use std::collections::HashMap;
use std::fmt::{Display, Formatter, Result as FmtResult, Write};
use std::mem::{ManuallyDrop, transmute};
use std::ptr::fn_addr_eq;
use std::result::Result as StdResult;
use std::sync::Arc;

use anyhow::Result;
use metadata_proto::vimana::runtime::ProtoMessage;
use prost::bytes::Buf;
use prost::encoding::{WireType, decode_varint};
use tonic::Status;
use tonic::codec::{DecodeBuf, Decoder as TonicDecoder};
use wasmtime::component::Val;

use compound::{
    compile_request_decoder, count_request_decoder_messages, enum_explicit_merge,
    enum_implicit_merge, enum_repeated_merge, message_inner_merge, oneof_variant_merge,
};
use names::ComponentName;

/// Decodes a top-level request message.
///
/// Reference-counted because Tonic's [codec](tonic::codec::Codec)
/// demands an owned decoder for each request, which is achieved by cloning.
#[derive(Clone)]
pub struct RequestDecoder(Arc<RequestDecoderInner>);

// Safe because the raw pointers in the decoder always point
// into the Vec within RequestDecoderInner,
// which is protected by an Arc and never modified after initialization.
unsafe impl Send for RequestDecoder {}
unsafe impl Sync for RequestDecoder {}

/// See [`RequestDecoder`].
struct RequestDecoderInner {
    /// Array of mergers for all messages that may appear within the request.
    /// The first element decodes the overall request message itself
    /// (the length is always at least 1).
    /// Any additional mergers would decode nested messages within the request.
    ///
    /// Using a vector allows us to represent self-referential message types,
    /// which use pointers to mergers within this vector to refer to one another (or themselves).
    /// That makes this a self-referential data structure.
    messages: Vec<MessageMerger>,

    /// Component name used for error logging only,
    /// shared among all encoders / decoders of a single component.
    component: Arc<ComponentName>,
}

pub(crate) struct MessageMerger {
    /// Map from field numbers to field indices and mergers.
    /// The field index is distinct from the Protobuf field number;
    /// it is the 0-based index within the [value](Val)'s `Record` field list
    /// in which to merge the value.
    pub(crate) fields: HashMap<u32, (u32, Merger)>,

    /// Default values for each field, if not encoded.
    pub(crate) defaults: Vec<(String, Val)>,
}

/// Decodes a component [value](Val) for any specific Protobuf field,
/// merging it into an existing value.
struct Merger {
    /// Decode and merge a value.
    merge: MergeFn,

    /// Information for decoding compound types (messages, oneofs, enumerations).
    /// Ignored for scalar types.
    compound: CompoundMerger,
}

/// Information for a [`Merger`] for compound types (messages, oneofs, enumerations).
///
/// Implemented as a union to save space while keeping a known size.
/// Each specific decoding function will know how to deal with this appropriately,
/// but we also have to manually drop the appropriate one in [`Merger::drop`].
union CompoundMerger {
    /// Merger for nested messages.
    /// This is a raw pointer into the [`messages`](RequestDecoderInner::messages) field
    /// of the surrounding [request decoder](RequestDecoderInner).
    message: *const MessageMerger,

    /// Map from enum variant numbers to variant names (for enumerations only).
    enum_variants: ManuallyDrop<HashMap<u32, String>>,

    /// Inner value merge function and variant name for a single oneof variant.
    oneof_variant: ManuallyDrop<(String, Box<Merger>)>,

    /// Set this placeholder value for scalars.
    scalar: (),
}

/// Decode a [value](Val) from the [buffer](Buf), reading only up to `limit` bytes.
/// Merge it into `dst`.
/// `limit` is decremented by the number of bytes read.
/// The wire type is also given so it can be checked by the merge function.
///
/// Each implementation should be specific to a certain Protobuf type.
type MergeFn = fn(
    merger: &Merger,
    wire_type: WireType,
    limit: &mut u32,
    src: &mut DecodeBuf<'_>,
    dst: &mut Val,
) -> StdResult<(), DecodeError>;

/// An error encountered during response decoding.
struct DecodeError {
    /// Basic error message.
    message: &'static str,

    /// Traceback of mutual recursion during decoding (most recent first).
    traceback: Vec<DecodeLevel>,
}

/// Represents a level of mutual recursion among compound subtypes
/// in an error traceback.
enum DecodeLevel {
    /// Message field number (*no* wire type).
    Field(u32),
    /// Repeated field index.
    Index(usize),
}

impl RequestDecoder {
    pub fn new(
        messages: &Vec<ProtoMessage>,
        request_index: u32,
        component: Arc<ComponentName>,
    ) -> Result<Self> {
        let request_index = request_index as usize;
        let mut index_map: HashMap<usize, usize> = HashMap::new();
        count_request_decoder_messages(messages, request_index, &mut index_map)?;

        // Allocate space for the relevant message mergers up front so it's never resized
        // and pointers into the vector can be stable.
        let mut message_mergers: Vec<MessageMerger> = Vec::with_capacity(index_map.len());
        compile_request_decoder(messages, request_index, &mut message_mergers, &index_map)?;

        // Sanity checks.
        debug_assert!(message_mergers.len() == index_map.len());
        debug_assert!(message_mergers.len() == message_mergers.capacity());

        Ok(Self(Arc::new(RequestDecoderInner {
            messages: message_mergers,
            component,
        })))
    }
}

impl TonicDecoder for RequestDecoder {
    type Item = Val;
    type Error = Status;

    /// Decode a message from a readable buffer.
    fn decode(&mut self, src: &mut DecodeBuf<'_>) -> StdResult<Option<Self::Item>, Self::Error> {
        let mut length = u32::try_from(src.remaining())
            .map_err(|_| Status::invalid_argument("Request is too big"))?;
        let message_merger = &self.0.messages[0];
        let mut value = Val::Record(message_merger.defaults.clone());
        message_inner_merge(
            &message_merger.fields,
            WireType::LengthDelimited,
            &mut length,
            src,
            &mut value,
        )
        .map_err(|error| {
            // A decoding error indicates that the client sent a malformed request.
            // Report this as an INVALID_ARGUMENT status to the caller and *do not* log it,
            // because this is considered a normal client error and could occur very frequently.
            Status::invalid_argument(error.to_string())
        })?;
        Ok(Some(value))
    }
}

/// [`Merger`] uses a union internally which must be dropped manually.
impl Drop for Merger {
    fn drop(&mut self) {
        // Mergers are dropped when a container shuts down (infrequently)
        // so we can exhaustively check against the known compound encoding functions
        // to figure out which type-specific data to drop.
        if fn_addr_eq(self.merge, enum_explicit_merge as MergeFn)
            || fn_addr_eq(self.merge, enum_implicit_merge as MergeFn)
            || fn_addr_eq(self.merge, enum_repeated_merge as MergeFn)
        {
            unsafe { ManuallyDrop::drop(&mut self.compound.enum_variants) }
        } else if fn_addr_eq(self.merge, oneof_variant_merge as MergeFn) {
            unsafe { ManuallyDrop::drop(&mut self.compound.oneof_variant) }
        }
    }
}

impl DecodeError {
    #[cold]
    pub(crate) fn new(message: &'static str) -> Self {
        Self {
            message,
            traceback: Vec::new(),
        }
    }

    #[cold]
    pub(crate) fn with_field(mut self, number: u32) -> Self {
        self.traceback.push(DecodeLevel::Field(number));
        self
    }

    #[cold]
    pub(crate) fn with_index(mut self, i: usize) -> Self {
        self.traceback.push(DecodeLevel::Index(i));
        self
    }
}

#[inline(always)]
fn read_varint(
    limit: &mut u32,
    src: &mut DecodeBuf<'_>,
    error: &'static str,
) -> StdResult<u64, DecodeError> {
    let remaining_before = src.remaining();
    let varint = decode_varint(src).map_err(
        // Overflowed 64 bits or incomplete at end of buffer.
        |_| DecodeError::new(error),
    )?;
    let bytes_read = (remaining_before - src.remaining()) as u32;
    if bytes_read > *limit {
        return Err(DecodeError::new(BUFFER_OVERFLOW));
    }
    *limit -= bytes_read;
    Ok(varint)
}

/// Decode a tag from `src`, returning the field number and wire type.
/// Decrement `limit` by the number of bytes read.
#[inline(always)]
fn decode_tag(limit: &mut u32, src: &mut DecodeBuf<'_>) -> StdResult<(u32, WireType), DecodeError> {
    let tag = read_varint(limit, src, INVALID_TAG_VARINT)?;
    let field_number = u32::try_from(tag >> 3).map_err(|_| {
        // Indicates the field number exceeded 32 bits.
        DecodeError::new(INVALID_FIELD_NUMBER)
    })?;
    // Field number 0 is reserved and invalid.
    if field_number == 0 {
        return Err(DecodeError::new(INVALID_FIELD_NUMBER));
    }
    let wire_type = (tag as u8) & 0b111;
    // There are 6 possible wire types. Check that it is valid before unsafely transmuting.
    if wire_type >= 6 {
        return Err(DecodeError::new(INVALID_WIRE_TYPE).with_field(field_number));
    }
    Ok((field_number, unsafe { transmute(wire_type) }))
}

/// Read a varint from the source buffer,
/// check that there are at least as many bytes left in the buffer,
/// then return that varint.
#[inline(always)]
fn read_length_check_overflow(
    limit: &mut u32,
    src: &mut DecodeBuf<'_>,
) -> StdResult<u32, DecodeError> {
    let length = read_varint(limit, src, INVALID_LENGTH_VARINT)?;
    let length = u32::try_from(length).map_err(|_| DecodeError::new(INVALID_LENGTH_VARINT))?;
    if length > *limit {
        return Err(DecodeError::new(BUFFER_OVERFLOW));
    }
    *limit -= length;
    Ok(length)
}

/// Use wire type information to skip an unknown field.
#[cold]
#[inline(always)]
fn skip(
    field_number: u32,
    wire_type: WireType,
    limit: &mut u32,
    src: &mut DecodeBuf<'_>,
) -> StdResult<(), DecodeError> {
    match wire_type {
        WireType::Varint => {
            // To skip a varint, just decode and forget it.
            read_varint(limit, src, INVALID_VARINT)?;
        }
        WireType::SixtyFourBit => {
            if 8 > *limit {
                return Err(DecodeError::new(BUFFER_OVERFLOW));
            }
            *limit -= 8;
            src.advance(8)
        }
        WireType::LengthDelimited => {
            let length = read_length_check_overflow(limit, src)?;
            src.advance(length as usize);
        }
        WireType::ThirtyTwoBit => {
            if 4 > *limit {
                return Err(DecodeError::new(BUFFER_OVERFLOW));
            }
            *limit -= 4;
            src.advance(4)
        }
        WireType::StartGroup => {
            return skip_group(field_number, limit, src);
        }
        WireType::EndGroup => {
            // An EndGroup tag without a preceding StartGroup is an error.
            return Err(DecodeError::new(UNMATCHED_END_GROUP));
        }
    }
    Ok(())
}

/// Skip until finding the [`EndGroup`](WireType::EndGroup) tag
/// that matches a previously-skipped [`StartGroup`](WireType::StartGroup) tag.
#[cold]
fn skip_group(
    field_number: u32,
    limit: &mut u32,
    src: &mut DecodeBuf<'_>,
) -> StdResult<(), DecodeError> {
    loop {
        if *limit == 0 {
            return Err(DecodeError::new(UNMATCHED_START_GROUP));
        }
        let (inner_field_number, inner_wire_type) = decode_tag(limit, src)?;
        if inner_wire_type == WireType::EndGroup {
            if inner_field_number == field_number {
                break;
            } else {
                return Err(DecodeError::new(UNMATCHED_END_GROUP));
            }
        }
        skip(inner_field_number, inner_wire_type, limit, src)?;
    }
    Ok(())
}

/// Return whether the given `ScalarCoding` uses explicit presence tracking.
#[inline(always)]
fn explicit_scalar(scalar_coding: i32) -> bool {
    // Explicit scalar coding numbers all happen to equal `4n+2` for some `n`.
    scalar_coding % 4 == 2
}

/// When returning an error status to a client,
/// a decoding error should be displayed like this:
///     Malformed request (.0.123[0][4].5.5): <message>
///
/// Numbers following dots indicate field numbers.
/// Those between square brackets indicate repeated field indices.
impl Display for DecodeError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> FmtResult {
        formatter.write_str("Malformed request (")?;
        format_decode_error_trace(self, formatter)?;
        formatter.write_str("): ")?;
        formatter.write_str(&self.message)
    }
}

#[inline(always)]
fn format_decode_error_trace(error: &DecodeError, formatter: &mut Formatter<'_>) -> FmtResult {
    for level in error.traceback.iter().rev() {
        match level {
            DecodeLevel::Field(number) => {
                formatter.write_char('.')?;
                Display::fmt(number, formatter)?;
            }
            DecodeLevel::Index(index) => {
                formatter.write_char('[')?;
                Display::fmt(index, formatter)?;
                formatter.write_char(']')?;
            }
        }
    }
    Ok(())
}

const BUFFER_UNDERFLOW: &str = "Buffer underflow";
const BUFFER_OVERFLOW: &str = "Buffer overflow";
const INVALID_TAG_VARINT: &str = "Invalid varint for tag";
const INVALID_LENGTH_VARINT: &str = "Invalid varint for length";
const INVALID_VARINT: &str = "Invalid varint";
const INVALID_FIELD_NUMBER: &str = "Invalid field number";
const INVALID_WIRE_TYPE: &str = "Invalid wire type";
const WIRETYPE_NON_VARINT: &str = "Wire type should be varint";
const WIRETYPE_NON_LENGTH_DELIMITED: &str = "Wire type should be length-delimited";
const WIRETYPE_NON_32BIT: &str = "Wire type should be 32-bit";
const WIRETYPE_NON_64BIT: &str = "Wire type should be 64-bit";
const INVALID_UTF8: &str = "Invalid UTF-8";
const INVALID_PERMISSIVE_STRING: &str = "Invalid permissive string";

const ENUM_NO_DEFAULT: &str = "Enum has no default value";
const NON_EXPLICIT_ONEOF_VARIANT: &str = "Oneof variant is not explicitly presence-tracked";
const MESSAGE_NON_RECORD: &str = "Message is not a record";
const FIELD_INDEX_OUT_OF_BOUNDS: &str = "Field index out of bounds";
const REPEATED_NON_LIST: &str = "Repeated value is not a list";
const UNMATCHED_END_GROUP: &str = "Unmatched end group tag";
const UNMATCHED_START_GROUP: &str = "Unmatched start group tag";
