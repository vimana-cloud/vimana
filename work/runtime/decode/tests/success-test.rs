use std::mem::{drop, transmute};
use std::sync::Arc;

use bytes::BytesMut;
use tonic::codec::Decoder;
use wasmtime::component::Val;

use decode::RequestDecoder;
use metadata_proto::work::runtime::container::field::{Coding, CompoundCoding, ScalarCoding};
use metadata_proto::work::runtime::container::Field;
use names::Name;

const EMPTY: [u8; 0] = [];

const COMPONENT_NAME: &str = "1234567890abcdef1234567890abcdef:package.Service@1.2.3";

/// Every decoding success test (where decoding is not expected to fail)
/// follows the same pattern.
///
/// This macro defines a test function named `$name`
/// using a little IDL to express field types, encoded buffers, and expected decoded values.
///
/// Looks like:
///     test_success!(
///         test_function_name,
///         fields = (
///             "field-name" <field type expression>
///             "another-field" <field type expression>
///             ...
///         ),
///         buffer = <byte slice>,
///         expect = (
///             "field-name" <value expression>
///             "another-field" <value expression>
///             ...
///         ),
///     )
///
/// See [`test_messages_deep_nested_lengths`] for an example.
macro_rules! test_success {
    (
        $name:ident,
        fields = ($($field_name:literal $field:tt)*),
        buffer = $buffer:expr,
        expect = ($($value_name:literal $value:expr;)*),
    ) => {
        #[test]
        fn $name() {
            let mut decoder = RequestDecoder::new(
                &Field {
                    number: 0,       // Ignored.
                    name: "".into(), // Ignored.
                    coding: None,    // Ignored.
                    subfields: vec![$(field!($field_name $field),)*],
                },
                Arc::new(Name::parse(COMPONENT_NAME).component().unwrap()),
            ).unwrap();
            let mut buffer = BytesMut::from(&$buffer[..]);
            let length = buffer.len();
            let mut decode_buffer = unsafe { transmute(DecodeBufClone { buf: &mut buffer, len: length }) };

            let result = decoder.decode(&mut decode_buffer).unwrap();

            let expected = Some(bare_record!($($value_name $value);+));
            assert_eq!(result, expected);

            // Make sure the decoder's drop method does not panic.
            drop(decoder);
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

/// This has to be an exact clone of [`tonic::codec::DecodeBuf`],
/// which has a private constructor that prevents instantiation here.
/// We get around that by unsafely transmuting a structurally-equivalent clone.
/// This is technically undefined behavior, but it works well enough for this test.
///
/// https://github.com/hyperium/tonic/blob/v0.12.3/tonic/src/codec/buffer.rs#L13
#[derive(Debug)]
struct DecodeBufClone<'a> {
    buf: &'a mut BytesMut,
    len: usize,
}

// This test verifies that the length pre-computation algorithm works
// for deeply-nested fields of various kinds.
test_success!(
    test_messages_deep_nested_lengths,
    fields = (
        "x" (message 1
            "a" (scalar 1 ScalarCoding::Sint32Implicit)
        )
        "y" (message 2
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
        )
    ),
    buffer = &[
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
    ],
    expect = (
        "x" record!(
            "a" Val::S32(-5)
        );
        "y" record!(
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
    ),
);

test_success!(
    test_bytes_implicit,
    fields = (
        "bytes-implicit" (scalar 12 ScalarCoding::BytesImplicit)
    ),
    buffer = &[
        98,             // tag: (12 << 3) + 2
        5,              // length of bytes
        1, 2, 3, 4, 5,  // bytes
    ],
    expect = (
        "bytes-implicit" Val::List(vec![
            Val::U8(1),
            Val::U8(2),
            Val::U8(3),
            Val::U8(4),
            Val::U8(5),
        ]);
    ),
);

test_success!(
    test_bytes_implicit_empty,
    fields = (
        "bytes-implicit-empty" (scalar 12 ScalarCoding::BytesImplicit)
    ),
    buffer = &EMPTY,
    expect = (
        "bytes-implicit-empty" Val::List(vec![
            // Empty implicit bytes should not encode at all.
        ]);
    ),
);

test_success!(
    test_bytes_explicit,
    fields = (
        "bytes-explicit" (scalar 1 ScalarCoding::BytesExplicit)
    ),
    buffer = &[
        10,             // tag: (1 << 3) + 2
        0,              // length of bytes
    ],
    expect = (
        "bytes-explicit" Val::Option(Some(Box::new(Val::List(vec![
            // Empty explicit bytes should encode with length 0.
        ]))));
    ),
);

test_success!(
    test_bytes_repeated,
    fields = (
        "bytes-repeated" (scalar 1 ScalarCoding::BytesExpanded)
    ),
    buffer = &[
        10,             // tag: (1 << 3) + 2
        2,              // length of bytes
          255, 127,     //   bytes
        10,             // tag: (1 << 3) + 2
        0,              // length of bytes
    ],
    expect = (
        "bytes-repeated" Val::List(vec![
            Val::List(vec![
                Val::U8(255),
                Val::U8(127),
            ]),
            Val::List(Vec::new()),
        ]);
    ),
);

test_success!(
    test_string_implicit,
    fields = (
        "string-implicit" (scalar 12 ScalarCoding::StringUtf8Implicit)
    ),
    buffer = &[
        98,                         // tag: (12 << 3) + 2
        5,                          // length of "hello"
          104, 101, 108, 108, 111,  //   bytes
    ],
    expect = (
        "string-implicit" Val::String("hello".into());
    ),
);

test_success!(
    test_string_implicit_empty,
    fields = (
        "string-implicit-empty" (scalar 12 ScalarCoding::StringUtf8Implicit)
    ),
    buffer = &EMPTY,
    expect = (
        "string-implicit-empty" Val::String("".into());
    ),
);

test_success!(
    test_string_explicit,
    fields = (
        "string-explicit" (scalar 1 ScalarCoding::StringPermissiveExplicit)
    ),
    buffer = &[
        10,             // tag: (1 << 3) + 2
        0,              // length of bytes
    ],
    expect = (
        "string-explicit" Val::Option(Some(Box::new(
            Val::String("".into())
        )));
    ),
);

test_success!(
    test_string_repeated,
    fields = (
        "string-repeated" (scalar 1 ScalarCoding::StringPermissiveExpanded)
    ),
    buffer = &[
        10,                                   // tag: (1 << 3) + 2
        7,                                    // length of "fersher"
          102, 101, 114, 115, 104, 101, 114,  //   "fersher"
        10,                                   // tag: (1 << 3) + 2
        0,                                    // length of bytes
    ],
    expect = (
        "string-repeated" Val::List(vec![
            Val::String("fersher".into()),
            Val::String("".into()),
        ]);
    ),
);
