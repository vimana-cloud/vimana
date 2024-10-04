//! Logic to decode protobuf messages directly into Wasm component values.
#![feature(core_intrinsics)]

use std::collections::HashMap;
use std::intrinsics::{likely, unlikely};
use std::io::{Error as IoError, ErrorKind as IoErrorKind};
use std::result::Result as StdResult;

use prost::bytes::Buf;
use prost::encoding::{decode_varint, WireType};
use tonic::codec::{DecodeBuf, Decoder as TonicDecoder};
use wasmtime::component::Val;

use coding_proto::coding::field::{Coding, CompoundCoding};
use coding_proto::coding::{field, Field};
use common::{invalid_data, ActioDecoder};
use error::{Error, Result};
use scalar::ScalarDecoder;

/// Map from field tag (wire type and field number)
/// to component field names and field decoders.
type MessageDecoderFields = HashMap<u64, (String, ActioDecoder)>;

/// Decoder for a protobuf message.
struct MessageDecoder {
    fields: MessageDecoderFields,
}

/// Generic helper function to wrap a concrete [`ActioDecoder`] as a boxed trait object.
fn box_decoder<T>(decoder: T) -> ActioDecoder
where
    T: TonicDecoder<Item = Val, Error = IoError> + 'static,
{
    Box::new(decoder)
}

impl MessageDecoder {
    /// Construct a new [`MessageDecoder`] from the given [`Field`] representing a message type.
    /// Only the [subfields](Field::subfields) are significant.
    fn new(message: &Field) -> Result<Self> {
        let mut fields: MessageDecoderFields = HashMap::new();

        for subfield in &message.subfields {
            let subfield_decoder: Result<ActioDecoder> = match subfield.coding {
                Some(coding) => match coding {
                    Coding::ScalarCoding(atomic_coding) => {
                        ScalarDecoder::new(atomic_coding).map(box_decoder)
                    }
                    Coding::CompoundCoding(compound_coding) => {
                        match CompoundCoding::try_from(compound_coding) {
                            Ok(compound_coding) => match compound_coding {
                                CompoundCoding::EnumImplicit => todo!(),
                                CompoundCoding::EnumPacked => todo!(),
                                CompoundCoding::EnumExplicit => todo!(),
                                CompoundCoding::EnumExpanded => todo!(),
                                CompoundCoding::Message => {
                                    MessageDecoder::new(subfield).map(box_decoder)
                                }
                                CompoundCoding::MessagePacked => todo!(),
                                CompoundCoding::MessageExpanded => todo!(),
                                CompoundCoding::Oneof => todo!(),
                            },
                            Err(enum_error) => {
                                return Err(Error::wrap("Unexpected enum value", enum_error))
                            }
                        }
                    }
                },
                None => {
                    return Err(Error::leaf(format!(
                        "Message field has no coding: {}",
                        subfield.name
                    )))
                }
            };

            match subfield_decoder {
                Ok(field_decoder) => {
                    fields.insert(subfield.tag, (subfield.name.clone(), field_decoder));
                }
                Err(field_error) => {
                    return Err(Error::wrap(
                        format!("Cannot create decoder for {}", subfield.name),
                        field_error,
                    ))
                }
            }
        }

        Ok(Self { fields })
    }
}

impl TonicDecoder for MessageDecoder {
    /// Decode messages directly to Wasm component values.
    type Item = Val;
    type Error = IoError;

    /// Decode a message from a buffer containing exactly the bytes of a full message.
    fn decode(&mut self, src: &mut DecodeBuf<'_>) -> StdResult<Option<Self::Item>, Self::Error> {
        let mut fields: Vec<(String, Val)> = Vec::new();

        while src.has_remaining() {
            // Decode the tag, which consists of a wire type designator and the field number.
            let tag = decode_varint(src)?;
            if unlikely(tag > u64::from(u32::MAX)) {
                return Err(invalid_data(format!("Invalid tag: {tag}")));
            }

            match self.fields.get_mut(&tag) {
                Some((field_name, field_decoder)) => match field_decoder.decode(src)? {
                    Some(val) => {
                        fields.push((field_name.clone(), val));
                    }
                    None => todo!(),
                },
                None => {
                    // Found an unexpected field tag.
                    // Use wire type information to skip it.
                    let wire_type = WireType::try_from(tag & 0x07)?;
                    //let field_number = tag as u32 >> 3;
                    //if unlikely(field_number < MIN_TAG) {
                    //    return Err(DecodeError::new(format!(
                    //        "Invalid field number: {field_number}"
                    //    )));
                    //}
                    todo!();
                }
            }
        }

        Ok(Some(Val::Record(fields)))
    }
}
