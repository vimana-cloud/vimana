use std::mem::transmute;
use std::sync::Arc;

use bytes::BytesMut;
use tonic::codec::Encoder;
use wasmtime::component::Val;

use encode::ResponseEncoder;
use metadata_proto::work::runtime::container::field::{Coding, CompoundCoding, ScalarCoding};
use metadata_proto::work::runtime::container::Field;
use names::Name;

const EMPTY: [u8; 0] = [];

const COMPONENT_NAME: &str = "1234567890abcdef1234567890abcdef:package.Service@1.2.3";

/// Every encoding test where encoding is not expected to fail follows exactly the same pattern.
/// This macro defines a single test function named `$name`
/// using a little IDL to define field types and values.
/// See usages for examples.
macro_rules! test_success {
    ($name:ident, $($field_name:literal: $field:tt $value:expr;)* expect = $expected:expr) => {
        #[test]
        fn $name() {
            let mut encoder = ResponseEncoder::new(
                &Field {
                    number: 0,       // Ignored.
                    name: "".into(), // Ignored.
                    coding: None,    // Ignored.
                    subfields: vec![$(field!($field_name $field),)*],
                },
                Arc::new(Name::parse(COMPONENT_NAME).component().unwrap()),
            ).unwrap();
            let value = bare_record!($($field_name $value);+);
            let mut buffer = BytesMut::new();
            let mut encode_buffer = unsafe { transmute(EncodeBufClone { buf: &mut buffer }) };

            encoder.encode(value, &mut encode_buffer).unwrap();

            assert_eq!(buffer.as_ref(), $expected)
        }
    };
}

macro_rules! field {
    ($name:literal (scalar $number:literal $coding:expr)) => {
        Field {
            name: String::from($name),
            number: $number,
            coding: Some(Coding::ScalarCoding($coding as i32)),
            subfields: Vec::new(),
        }
    };
    ($name:literal (message $number:literal $($subfield_name:literal $subfield:tt)+)) => {
        Field {
            name: String::from($name),
            number: $number,
            coding: Some(Coding::CompoundCoding(CompoundCoding::Message as i32)),
            subfields: vec![$(field!($subfield_name $subfield),)*],
        }
    };
    ($name:literal (oneof $($subfield_name:literal $subfield:tt)+)) => {
        Field {
            name: String::from($name),
            number: 0, // Ignored.
            coding: Some(Coding::CompoundCoding(CompoundCoding::Oneof as i32)),
            subfields: vec![$(field!($subfield_name $subfield),)*],
        }
    };
}

// The following macros generate component value constants idiomatically:

/// For messages nested in oneofs or lists.
macro_rules! bare_record {
    // Records cannot be empty as per the Wasm spec.
    ($($name:literal $value:expr);+) => {
        Val::Record(vec![$((String::from($name), $value)),*])
    };
}

/// For nested message subfields.
macro_rules! record {
    // Records cannot be empty as per the Wasm spec.
    ($($name:literal $value:expr);+) => {
        Val::Option(Some(Box::new(bare_record!($($name $value);+))))
    };
}

/// For oneof variants.
macro_rules! variant {
    ($name:literal $value:expr) => {
        Val::Option(Some(Box::new(Val::Variant(
            String::from($name),
            Some(Box::new($value)),
        ))))
    };
}

/// This has to be an exact clone of [`tonic::codec::EncodeBuf`],
/// which has a private constructor that prevents instantiation here.
/// We get around that by unsafely transmuting a structurally-equivalent clone.
/// This is technically undefined behavior, but it works well enough for this test.
///
/// https://github.com/hyperium/tonic/blob/v0.12.3/tonic/src/codec/buffer.rs#L13
#[derive(Debug)]
struct EncodeBufClone<'a> {
    #[allow(dead_code)]
    buf: &'a mut BytesMut,
}

// This test verifies that the length pre-computation algorithm works
// for deeply-nested fields of various kinds.
test_success!(
    test_messages_deep_nested_lengths,
    "x": (message 1
        "a" (scalar 1 ScalarCoding::Sint32Implicit)
    ) record!(
        "a" Val::S32(-5)
    );
    "y": (message 2
        "aa" (message 30
            "strings" (scalar 1 ScalarCoding::StringUtf8Expanded)
            "variants" (oneof
                "another" (message 5
                    "aaa" (scalar 1 ScalarCoding::FloatPacked)
                )
                "unused" (scalar 6 ScalarCoding::BoolExplicit)
            )
        )
        "bb" (scalar 3 ScalarCoding::Int64Packed)
    ) record!(
        "aa" record!(
            "strings" Val::List(vec![Val::String("test".into()), Val::String("ing".into())]);
            "variants" variant!(
                "another" bare_record!(
                    "aaa" Val::List(vec![Val::Float32(0.0), Val::Float32(-1.0)])
                )
            )
        );
        "bb" Val::List(vec![Val::S64(127), Val::S64(128), Val::S64(2097152), Val::S64(0)])
    );
    expect = &[
        10,                        // 'x' tag: (1 << 3) + 2
        2,                         // length of submessage
          8,                       //   'a' tag: (1 << 3) + 0
          9,                       //   -5 [zig-zag-encoded]
        18,                        // 'y' tag: (2 << 3) + 2
        36,                        // length of submessage
          242, 1,                  //   'aa' tag: (30 << 2) + 2
          23,                      //   length of submessage
            10,                    //     'strings' tag: (1 << 3) + 2
            4,                     //     length of "test"
              116, 101, 115, 116,  //       "test"
            10,                    //     'strings' tag: (1 << 3) + 2
            3,                     //     length of "ing"
              105, 110, 103,       //       "ing"
            42,                    //     'another' tag: (5 << 3) + 2
            10,                    //     length of submessage
              10,                  //       'aaa' tag: (1 << 3) + 2
              8,                   //       length of packed float32
                0, 0, 0, 0,        //         0.0
                0, 0, 128, 191,    //         -1.0
          26,                      //   'bb' tag: (3 << 2) + 2
          8,                       //   byte length of packed varint
            127,                   //     127
            128, 1,                //     128
            128, 128, 128, 1,      //     2097152
            0,                     //     0
    ]
);

test_success!(
    test_bytes_implicit,
    "bytes-implicit": (scalar 12 ScalarCoding::BytesImplicit)
        Val::List(vec![
            Val::U8(1),
            Val::U8(2),
            Val::U8(3),
            Val::U8(4),
            Val::U8(5),
        ]);
    expect = &[
        98,             // tag: (12 << 3) + 2
        5,              // length of bytes
        1, 2, 3, 4, 5,  // bytes
    ]
);

test_success!(
    test_bytes_implicit_empty,
    "bytes-implicit-empty": (scalar 12 ScalarCoding::BytesImplicit)
        Val::List(vec![
            // Empty implicit bytes should not encode at all.
        ]);
    expect = &EMPTY
);

test_success!(
    test_bytes_explicit,
    "bytes-explicit": (scalar 1 ScalarCoding::BytesExplicit)
        Val::Option(Some(Box::new(Val::List(vec![
            // Empty explicit bytes should encode with length 0.
        ]))));
    expect = &[
        10,             // tag: (1 << 3) + 2
        0,              // length of bytes
    ]
);

test_success!(
    test_bytes_repeated,
    "bytes-repeated": (scalar 1 ScalarCoding::BytesExpanded)
        Val::List(vec![
            Val::List(vec![
                Val::U8(255),
                Val::U8(127)]),
            Val::List(Vec::new())]);
    expect = &[
        10,             // tag: (1 << 3) + 2
        2,              // length of bytes
          255, 127,     //   bytes
        10,             // tag: (1 << 3) + 2
        0,              // length of bytes
    ]
);
