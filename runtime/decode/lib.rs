//! Decode incoming requests into Wasm component record values.

mod compound;
mod scalar;

use std::collections::HashMap;
use std::fmt::{Display, Formatter, Result as FmtResult, Write};
use std::mem::{transmute, ManuallyDrop};
use std::ptr::fn_addr_eq;
use std::result::Result as StdResult;
use std::sync::Arc;

use anyhow::{Context, Result};
use metadata_proto::work::runtime::Field;
use prost::bytes::Buf;
use prost::encoding::{decode_varint, encoded_len_varint, WireType};
use tonic::codec::{DecodeBuf, Decoder as TonicDecoder};
use tonic::Status;
use wasmtime::component::Val;

use compound::{
    enum_explicit_merge, enum_implicit_merge, enum_repeated_merge, message_inner_merge,
    message_outer_merge, message_repeated_merge, oneof_variant_merge,
};
use names::ComponentName;

/// Decodes a top-level request message.
///
/// Reference-counted because Tonic's [codec](tonic::codec::Codec)
/// demands an owned decoder for each request, which is achieved by cloning.
#[derive(Clone)]
pub struct RequestDecoder(Arc<RequestDecoderInner>);

/// See [`RequestDecoder`].
struct RequestDecoderInner {
    /// Decodes and merges protobuf contents into a default value.
    inner: Merger,

    /// Component name used for error logging only, shared to save memory.
    component: Arc<ComponentName>,
}

/// Decodes a component [value](Val) for any specific Protobuf field,
/// merging it into an existing value.
struct Merger {
    /// Decode and merge a value.
    merge: MergeFn,

    /// For records only: default values for each field, if not encoded.
    defaults: Vec<(String, Val)>,

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
    /// Map from subfield numbers to field inidices and decoders for messages.
    /// The field index is distinct from the Protobuf field number;
    /// it is the 0-based index within the [value](Val)'s `Record` field list
    /// in which to merge the value.
    subfields: ManuallyDrop<HashMap<u32, (u32, Merger)>>,

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
    pub fn new(request: &Field, component: Arc<ComponentName>) -> Result<Self> {
        Ok(Self(Arc::new(RequestDecoderInner {
            inner: Merger::message_inner(request, component.as_ref())
                .context("Invalid request decoder")?,
            component: component,
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
        let mut value = Val::Record(self.0.inner.defaults.clone());
        (self.0.inner.merge)(
            &self.0.inner,
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
        if fn_addr_eq(self.merge, message_inner_merge as MergeFn)
            || fn_addr_eq(self.merge, message_outer_merge as MergeFn)
            || fn_addr_eq(self.merge, message_repeated_merge as MergeFn)
        {
            unsafe { ManuallyDrop::drop(&mut self.compound.subfields) }
        } else if fn_addr_eq(self.merge, enum_explicit_merge as MergeFn)
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
    let varint = decode_varint(src).map_err(
        // Overflowed 64 bits or incomplete at end of buffer.
        |_| DecodeError::new(error),
    )?;
    let bytes_read = encoded_len_varint(varint) as u32;
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
#[inline(always)]
fn skip(
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
        // StartGroup and EndGroup are deprecated. Always skip.
        // They have no payload, so once we've read the tag, we've already skipped it.
        WireType::StartGroup | WireType::EndGroup => (),
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
const OVERFLOW_32BIT: &str = "Overflowed 32 bits";
const INVALID_UTF8: &str = "Invalid UTF-8";
const INVALID_PERMISSIVE_STRING: &str = "Invalid permissive string";
const INVALID_BOOL: &str = "Invalid boolean value";

const ENUM_NO_DEFAULT: &str = "Enum has no default value";
const NON_EXPLICIT_ONEOF_VARIANT: &str = "Oneof variant is not explicitly presence-tracked";
const MESSAGE_NON_RECORD: &str = "Message is not a record";
const FIELD_INDEX_OUT_OF_BOUNDS: &str = "Field index out of bounds";
const REPEATED_NON_LIST: &str = "Repeated value is not a list";
