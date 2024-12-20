//! Logic to decode protobuf messages directly into Wasm component values.
#![feature(core_intrinsics)]

use std::collections::HashMap;
use std::intrinsics::{likely, unlikely};
use std::io::ErrorKind as IoErrorKind;
use std::result::Result as StdResult;

use prost::bytes::Buf;
use tonic::codec::{DecodeBuf, Decoder as TonicDecoder};
use tonic::Status;
use wasmtime::component::Val;

use common::{decode_varint, invalid_tag, wiretype_from_tag, Decoder};
use error::{Error, Result};
use grpc_container_proto::work::runtime::grpc_metadata::method::field::{Coding, CompoundCoding};
use grpc_container_proto::work::runtime::grpc_metadata::method::Field;
use scalar::ScalarDecoder;

/// A decoder in Vimana is anything that decodes directly into a Wasm component value.
type DynamicDecoder = Box<dyn Decoder + Sync + Send>;

/// Map from field tags (wire type and field number)
/// to component field names and field decoders.
type MessageDecoderFields = HashMap<u64, (String, DynamicDecoder)>;

/// Decoder for a protobuf message.
pub struct MessageDecoder {
    fields: MessageDecoderFields,
}

/// Generic helper function to wrap a concrete [`DynamicDecoder`] as a boxed trait object.
fn box_decoder<T: Decoder + Sync + Send + 'static>(decoder: T) -> DynamicDecoder {
    Box::new(decoder)
}

impl MessageDecoder {
    /// Construct a new [`MessageDecoder`] from the given [`Field`] representing a message type.
    /// Only the [subfields](Field::subfields) are significant.
    pub fn new(message: &Field) -> Result<Self> {
        let mut fields: MessageDecoderFields = HashMap::new();

        for subfield in &message.subfields {
            let subfield_decoder: Result<DynamicDecoder> = match subfield.coding {
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

impl Decoder for MessageDecoder {
    fn decode(&self, src: &mut DecodeBuf<'_>) -> StdResult<Option<Val>, Status> {
        let mut fields: Vec<(String, Val)> = Vec::new();

        while src.has_remaining() {
            // Decode the tag, which consists of a wire type designator and the field number.
            let tag = decode_varint(src)?;
            if unlikely(tag > u64::from(u32::MAX)) {
                return Err(invalid_tag(tag));
            }

            match self.fields.get(&tag) {
                Some((field_name, field_decoder)) => match field_decoder.decode(src)? {
                    Some(val) => {
                        fields.push((field_name.clone(), val));
                    }
                    None => todo!(),
                },
                None => {
                    // Found an unexpected field tag.
                    // Use wire type information to skip it.
                    let wire_type = wiretype_from_tag(tag)?;
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

impl TonicDecoder for MessageDecoder {
    type Item = Val;
    type Error = Status;

    /// Decode a message from a buffer containing exactly the bytes of a full message.
    fn decode(&mut self, src: &mut DecodeBuf<'_>) -> StdResult<Option<Self::Item>, Self::Error> {
        Decoder::decode(self, src)
    }
}
