use std::mem::{drop, transmute};
use std::sync::Arc;

use bytes::BytesMut;
use tonic::codec::{EncodeBuf, Encoder};
use wasmtime::component::Val;

use encode::ResponseEncoder;
use metadata_proto::vimana::runtime::field::{Coding, CompoundCoding, ScalarCoding};
use metadata_proto::vimana::runtime::{Field, ProtoMessage};
use names::Name;

const EMPTY: [u8; 0] = [];

const COMPONENT_NAME: &str = "1234567890abcdef1234567890abcdef:some-server-id@1.2.3";

/// Every encoding success test (where encoding is not expected to fail)
/// follows the same pattern.
///
/// This macro defines a test function named `$test_name`
/// using a little IDL to express message types, the response value,
/// and the expected encoding.
///
/// Looks like:
///     test_success!(
///         test_function_name:
///         <message definition>, ...
///         <message definition>;
///         [<response index>] = <Wasm component value>;
///         expect = <byte slice>
///     );
///
/// See [`test_messages_deep_nested_lengths`] for an example.
macro_rules! test_success {
    ($test_name:ident: $($message_definition:expr),+; [$response_index:literal] = $value:expr; expect = $expected:expr) => {
        #[test]
        fn $test_name() {
            let mut encoder = ResponseEncoder::new(
                &vec![$($message_definition,)*],
                $response_index,
                Arc::new(Name::parse(COMPONENT_NAME).component().unwrap()),
            ).unwrap();
            let mut buffer = BytesMut::new();
            let mut encode_buffer = unsafe { transmute::<EncodeBufClone<'_>, EncodeBuf<'_>>(EncodeBufClone { buf: &mut buffer }) };

            encoder.encode($value, &mut encode_buffer).unwrap();

            assert_eq!(buffer.as_ref(), $expected);

            // Make sure the decoder's drop method does not panic.
            drop(encoder);
        }
    };
}

/// Idiomatically express [`ProtoMessage`] objects.
macro_rules! message {
    ($($field_name:literal: $field:tt)+) => {
        ProtoMessage {
            fields: vec![$(field!($field_name $field),)*],
        }
    };
}

/// Idiomatically express [`Field`] objects of each type.
macro_rules! field {
    ($name:literal (scalar[$number:literal] ($coding:expr))) => {
        Field {
            name: String::from($name),
            number: $number,
            message: 0,                // ignored
            variants: Vec::default(),  // ignored
            coding: Some(Coding::ScalarCoding($coding as i32)),
        }
    };
    ($name:literal (message[$number:literal] $index:literal)) => {
        Field {
            name: String::from($name),
            number: $number,
            message: $index,
            variants: Vec::default(),  // ignored
            coding: Some(Coding::CompoundCoding(CompoundCoding::Message as i32)),
        }
    };
    ($name:literal (oneof $($variant_name:literal: $variant:tt)+)) => {
        Field {
            name: String::from($name),
            number: 0,   // ignored
            message: 0,  // ignored
            variants: vec![$(field!($variant_name $variant),)*],
            coding: Some(Coding::CompoundCoding(CompoundCoding::Oneof as i32)),
        }
    };
    ($name:literal (enumeration[$number:literal] ($coding:expr) $($variant_name:literal: $variant_number:literal)+)) => {
        Field {
            name: String::from($name),
            number: $number,
            message: 0,  // ignored
            variants: vec![$(
                Field {
                    name: String::from($variant_name),
                    number: $variant_number,
                    message: 0,                // ignored
                    coding: None,              // ignored
                    variants: Vec::default(),  // ignored
                },
            )*],
            coding: Some(Coding::CompoundCoding(($coding) as i32)),
        }
    };
}

// The following macros generate component value constants idiomatically:

/// For messages nested in oneofs or lists (not wrapped in optional).
macro_rules! bare_record {
    // Records cannot be empty as per Wasm spec.
    ($($name:literal $value:expr);+) => {
        Val::Record(vec![$((String::from($name), $value)),*])
    };
}

/// For nested message subfields (wrapped in optional).
macro_rules! record {
    // Records cannot be empty as per Wasm spec.
    ($($name:literal $value:expr);+) => {
        Val::Option(Some(Box::new(bare_record!($($name $value);+))))
    };
}

/// For oneof variants.
macro_rules! oneof_variant {
    ($name:literal $value:expr) => {
        Val::Option(Some(Box::new(Val::Variant(
            String::from($name),
            Some(Box::new($value)),
        ))))
    };
}

/// This has to be an exact clone of [`EncodeBuf`],
/// which has a private constructor that prevents instantiation here.
/// We get around that by unsafely transmuting a structurally-equivalent clone.
/// This is technically undefined behavior, but it works well enough for this test.
///
/// https://github.com/hyperium/tonic/blob/v0.12.3/tonic/src/codec/buffer.rs#L13
#[derive(Debug)]
#[allow(dead_code)]
struct EncodeBufClone<'a> {
    buf: &'a mut BytesMut,
}

// This test verifies that the length pre-computation algorithm works
// for deeply-nested fields of various kinds.
test_success!(
    test_messages_deep_nested_lengths:
    message!(
        "a": (scalar[1] (ScalarCoding::Sint32Implicit))
    ),
    message!(
        "aa": (message[30] 2)
        "bb": (scalar[3] (ScalarCoding::Int64Packed))
    ),
    message!(
        "strings": (scalar[1] (ScalarCoding::StringUtf8Expanded))
        "variants": (oneof
            "another": (message[5] 4)
            "unused": (scalar[6] (ScalarCoding::BoolExplicit))
        )
        "youre-either": (enumeration[128] (CompoundCoding::EnumPacked)
            "in": 1
            "out": 0
            "above": 10_000
        )
    ),
    message!(
        "x": (message[1] 0)
        "y": (message[2] 1)
    ),
    message!(
        "aaa": (scalar[1] (ScalarCoding::FloatPacked))
    );
    [3] = bare_record!(
        "x" record!(
            "a" Val::S32(-5)
        );
        "y" record!(
            "aa" record!(
                "strings" Val::List(vec![Val::String("test".into()), Val::String("ing".into())]);
                "variants" oneof_variant!(
                    "another" bare_record!(
                        "aaa" Val::List(vec![Val::Float32(0.0), Val::Float32(-1.0)])
                    )
                );
                "youre-either" Val::List(vec![
                    Val::Enum("in".into()),
                    Val::Enum("above".into()),
                    Val::Enum("out".into()),
                ])
            );
            "bb" Val::List(vec![Val::S64(127), Val::S64(128), Val::S64(2097152), Val::S64(0)])
        )
    );
    expect = &[
        10,                        // 'x' tag: (1 << 3) + 2
        2,                         // length of submessage
          8,                       //   'a' tag: (1 << 3) + 0
          9,                       //   -5 [zig-zag-encoded]
        18,                        // 'y' tag: (2 << 3) + 2
        43,                        // length of submessage
          242, 1,                  //   'aa' tag: (30 << 2) + 2
          30,                      //   length of submessage
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
            130, 8,                //     'youre-either' tag: (128 << 3) + 2
            4,                     //     length of packed repeated enum
              1,                   //       "in" (1)
              144, 78,             //       "above" (10,000)
              0,                   //       "out" (0)
          26,                      //   'bb' tag: (3 << 2) + 2
          8,                       //   byte length of packed varint
            127,                   //     127
            128, 1,                //     128
            128, 128, 128, 1,      //     2097152
            0,                     //     0
    ]
);

test_success!(
    test_bytes_implicit:
    message!(
        "bytes-implicit": (scalar[12] (ScalarCoding::BytesImplicit))
    );
    [0] = bare_record!(
        "bytes-implicit" Val::List(vec![
            Val::U8(1),
            Val::U8(2),
            Val::U8(3),
            Val::U8(4),
            Val::U8(5),
        ])
    );
    expect = &[
        98,             // tag: (12 << 3) + 2
        5,              // length of bytes
        1, 2, 3, 4, 5,  // bytes
    ]
);

test_success!(
    test_bytes_implicit_empty:
    message!(
        "bytes-implicit-empty": (scalar[12] (ScalarCoding::BytesImplicit))
    );
    [0] = bare_record!(
        "bytes-implicit-empty" Val::List(vec![
            // Empty implicit bytes should not encode at all.
        ])
    );
    expect = &EMPTY
);

test_success!(
    test_bytes_explicit:
    message!(
        "bytes-explicit": (scalar[1] (ScalarCoding::BytesExplicit))
    );
    [0] = bare_record!(
        "bytes-explicit" Val::Option(Some(Box::new(Val::List(vec![
            // Empty explicit bytes should encode with length 0.
        ]))))
    );
    expect = &[
        10,             // tag: (1 << 3) + 2
        0,              // length of bytes
    ]
);

test_success!(
    test_bytes_repeated:
    message!(
        "bytes-repeated": (scalar[1] (ScalarCoding::BytesExpanded))
    );
    [0] = bare_record!(
        "bytes-repeated" Val::List(vec![
            Val::List(vec![
                Val::U8(255),
                Val::U8(127),
            ]),
            Val::List(Vec::new()),
        ])
    );
    expect = &[
        10,             // tag: (1 << 3) + 2
        2,              // length of bytes
          255, 127,     //   bytes
        10,             // tag: (1 << 3) + 2
        0,              // length of bytes
    ]
);

test_success!(
    test_string_implicit:
    message!(
        "string-implicit": (scalar[12] (ScalarCoding::StringUtf8Implicit))
    );
    [0] = bare_record!(
        "string-implicit" Val::String("hello".into())
    );
    expect = &[
        98,                         // tag: (12 << 3) + 2
        5,                          // length of "hello"
          104, 101, 108, 108, 111,  //   bytes
    ]
);

test_success!(
    test_string_implicit_empty:
    message!(
        "string-implicit-empty": (scalar[12] (ScalarCoding::StringUtf8Implicit))
    );
    [0] = bare_record!(
        "string-implicit-empty" Val::String("".into())
    );
    expect = &EMPTY
);

test_success!(
    test_string_explicit:
    message!(
        "string-explicit": (scalar[1] (ScalarCoding::StringPermissiveExplicit))
    );
    [0] = bare_record!(
        "string-explicit" Val::Option(Some(Box::new(
            Val::String("".into())
        )))
    );
    expect = &[
        10,             // tag: (1 << 3) + 2
        0,              // length of bytes
    ]
);

test_success!(
    test_string_repeated:
    message!(
        "string-repeated": (scalar[1] (ScalarCoding::StringPermissiveExpanded))
    );
    [0] = bare_record!(
        "string-repeated" Val::List(vec![
            Val::String("fersher".into()),
            Val::String("".into()),
        ])
    );
    expect = &[
        10,                                   // tag: (1 << 3) + 2
        7,                                    // length of "fersher"
          102, 101, 114, 115, 104, 101, 114,  //   "fersher"
        10,                                   // tag: (1 << 3) + 2
        0,                                    // length of bytes
    ]
);
