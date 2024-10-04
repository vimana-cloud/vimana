use bytes::buf::BufMut;
use prost::Message;

use messages_proto::messages::ScalarTypes;

#[test]
fn test_bytes() {
    let mut msg = ScalarTypes::default();
    msg.bytes_implicit = vec![1u8, 0u8, 255u8, 195u8, 40u8];
    msg.bytes_packed = vec![msg.bytes_implicit.clone(), Vec::new(), vec![0u8]];
    msg.bytes_explicit = Some(Vec::new());
    msg.bytes_expanded = msg.bytes_packed.clone();
    let mut buf = Vec::new();
    msg.encode(&mut buf);

    // TODO: Meaningful testing...
}
