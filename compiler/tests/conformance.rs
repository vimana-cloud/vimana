//! Run Protobuf conformance tests.
//!
//! Parts of this file (like the I/O logic) were copied from the draft [Protobuf Rust conformance test].
//! Copyright 2023 Google LLC. All rights reserved.
//! Use of that source code is governed by a BSD-style license
//! that can be found at https://developers.google.com/open-source/licenses/bsd.
//!
//! [Protobuf Rust conformance test]: https://github.com/protocolbuffers/protobuf/blob/v33.1/conformance/conformance_rust.rs

use std::collections::HashMap;
use std::fs::read;
use std::io::{ErrorKind, Read, Stdin, Stdout, Write, stdin, stdout};
use std::mem::transmute;
use std::result::Result as StdResult;
use std::sync::Arc;

use bytes::BytesMut;
use conformance_proto::conformance::conformance_request::Payload;
use conformance_proto::conformance::conformance_response::Result as ConformanceResult;
use conformance_proto::conformance::{ConformanceRequest, ConformanceResponse, WireFormat};
use prost::Message;
use prost::bytes::Bytes;
use tonic::Status;
use tonic::codec::{Decoder, Encoder};

use decode::RequestDecoder;
use encode::ResponseEncoder;
use metadata_proto::vimana::runtime::Metadata;
use names::Name;

const TEST_ALL_TYPES_PROTO2_MESSAGE_NAME: &str = "protobuf_test_messages.proto2.TestAllTypesProto2";
const TEST_ALL_TYPES_PROTO2_METADATA_PATH: &str =
    "compiler/tests/test-messages-proto2-vimana/metadata.binpb";

const TEST_ALL_TYPES_PROTO3_MESSAGE_NAME: &str = "protobuf_test_messages.proto3.TestAllTypesProto3";
const TEST_ALL_TYPES_PROTO3_METADATA_PATH: &str =
    "compiler/tests/test-messages-proto3-vimana/metadata.binpb";

const DUMMY_COMPONENT_NAME: &str = "0123456789abcdef0123456789abcdef:the-server@1.2.3";

struct Harness {
    stdin: Stdin,
    stdout: Stdout,
    codecs: HashMap<String, Codec>,
}

struct Codec {
    decoder: RequestDecoder,
    encoder: ResponseEncoder,
}

struct DecodeBufStandIn<'a> {
    buf: &'a mut BytesMut,
    len: usize,
}

pub struct EncodeBufStandIn<'a> {
    buf: &'a mut BytesMut,
}

fn main() {
    let mut harness = Harness::new();
    let mut total_runs = 0;

    while let Some(request) = harness.read_request() {
        let response = harness.do_test(request);
        harness.write_response(response);
        total_runs += 1;
    }

    eprintln!("conformance-vimana: Received EOF from test runner after {total_runs} tests");
}

impl Harness {
    fn new() -> Self {
        let mut codecs = HashMap::new();
        codecs.insert(
            String::from(TEST_ALL_TYPES_PROTO2_MESSAGE_NAME),
            Codec::load(
                TEST_ALL_TYPES_PROTO2_METADATA_PATH,
                "protobuf_test_messages.proto2.Dummy",
            ),
        );
        codecs.insert(
            String::from(TEST_ALL_TYPES_PROTO3_MESSAGE_NAME),
            Codec::load(
                TEST_ALL_TYPES_PROTO3_METADATA_PATH,
                "protobuf_test_messages.proto3.Dummy",
            ),
        );
        Self {
            stdin: stdin(),
            stdout: stdout(),
            codecs,
        }
    }

    fn do_test(&mut self, request: ConformanceRequest) -> ConformanceResponse {
        if request.requested_output_format() != WireFormat::Protobuf {
            return result_skipped("Only wire format output is supported");
        }

        let bytes = match request.payload.unwrap() {
            Payload::ProtobufPayload(payload) => payload,
            _ => {
                return result_skipped("Only wire format input is supported");
            }
        };

        self.codecs
            .get_mut(&request.message_type)
            .expect(format!("Unexpected message type: {:?}", request.message_type).as_str())
            .round_trip(BytesMut::from(bytes))
            .map_or_else(result_parse_error, result_protobuf_encoded)
    }

    /// Returns Some of a conformance request read from stdin.
    /// Returns None if we have hit an EOF that suggests the test suite is complete.
    /// Panics in any other case (e.g. an EOF in a place that would imply a
    /// programmer error in the conformance test suite).
    fn read_request(&mut self) -> Option<ConformanceRequest> {
        let msg_len = self.read_little_endian_i32()?;
        let mut serialized = vec![0_u8; msg_len as usize];
        self.stdin.read_exact(&mut serialized).unwrap();
        Some(ConformanceRequest::decode(serialized.as_slice()).unwrap())
    }

    /// Serialize (length-prefixed) a conformance response to stdout.
    fn write_response(&mut self, response: ConformanceResponse) {
        let bytes = response.encode_to_vec();
        let length = bytes.len() as u32;
        self.stdout.write_all(&length.to_le_bytes()).unwrap();
        self.stdout.write(bytes.as_slice()).unwrap();
        self.stdout.flush().unwrap();
    }

    /// Returns Some(i32) if a binary read can succeed from stdin.
    /// Returns None if we have reached an EOF.
    /// Panics for any other error reading.
    fn read_little_endian_i32(&mut self) -> Option<i32> {
        let mut buffer = [0_u8; 4];
        if let Err(e) = self.stdin.read_exact(&mut buffer) {
            match e.kind() {
                ErrorKind::UnexpectedEof => None,
                _ => panic!("Failed to read i32 from stdin"),
            }
        } else {
            Some(i32::from_le_bytes(buffer))
        }
    }
}

impl Codec {
    fn load(path: &str, dummy_service: &str) -> Self {
        let metadata = Metadata::decode(read(path).unwrap().as_slice()).unwrap();
        let index = metadata.services[dummy_service].methods["Dummy"].request;
        let name = Arc::new(Name::parse(DUMMY_COMPONENT_NAME).component().unwrap());

        let decoder = RequestDecoder::new(&metadata.messages, index, name.clone()).unwrap();
        let encoder = ResponseEncoder::new(&metadata.messages, index, name).unwrap();
        Self { decoder, encoder }
    }

    fn round_trip(&mut self, mut input: BytesMut) -> StdResult<Vec<u8>, Status> {
        let input_length = input.len();
        let buffer = DecodeBufStandIn {
            buf: &mut input,
            len: input_length,
        };
        let mut buffer = unsafe { transmute(buffer) };

        let value = self.decoder.decode(&mut buffer)?.unwrap();

        let mut output = BytesMut::new();
        let buffer = EncodeBufStandIn { buf: &mut output };
        let mut buffer = unsafe { transmute(buffer) };

        self.encoder.encode(value, &mut buffer).unwrap();

        Ok(output.to_vec())
    }
}

fn result_protobuf_encoded<T: Into<Bytes>>(serialized: T) -> ConformanceResponse {
    ConformanceResponse {
        result: Some(ConformanceResult::ProtobufPayload(serialized.into())),
    }
}

fn result_skipped<T: Into<String>>(message: T) -> ConformanceResponse {
    ConformanceResponse {
        result: Some(ConformanceResult::Skipped(message.into())),
    }
}

fn result_parse_error(error: Status) -> ConformanceResponse {
    ConformanceResponse {
        result: Some(ConformanceResult::ParseError(error.to_string())),
    }
}
