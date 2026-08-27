use hmcl_plugin_sdk::Value;
use hmcl_runtime_protocol::{MAX_FRAME_BYTES, Message, MessageBody, read_frame, write_frame};
use std::io::Cursor;

#[test]
fn round_trips_hello_with_a_big_endian_frame_length() {
    let message = Message::new(7, MessageBody::Hello).expect("valid hello");
    let mut bytes = Vec::new();

    write_frame(&mut bytes, &message).expect("encode hello frame");

    let expected = [
        &[0x00, 0x00, 0x00, 0x6b][..],
        &[0x92, 0x07, 0xdd, 0x00, 0x00, 0x00, 0x04],
        &[0x92, 0xdb, 0x00, 0x00, 0x00, 0x0f],
        b"protocolVersion",
        &[
            0x92, 0x02, 0xd3, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01,
        ],
        &[0x92, 0xdb, 0x00, 0x00, 0x00, 0x09],
        b"requestId",
        &[
            0x92, 0x02, 0xd3, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x07,
        ],
        &[0x92, 0xdb, 0x00, 0x00, 0x00, 0x04],
        b"kind",
        &[0x92, 0x04, 0xdb, 0x00, 0x00, 0x00, 0x05],
        b"hello",
        &[0x92, 0xdb, 0x00, 0x00, 0x00, 0x07],
        b"payload",
        &[0x92, 0x07, 0xdd, 0x00, 0x00, 0x00, 0x00],
    ]
    .concat();
    assert_eq!(bytes, expected);
    let declared = u32::from_be_bytes(bytes[0..4].try_into().expect("four-byte header"));
    assert_eq!(
        usize::try_from(declared).expect("usize length"),
        bytes.len() - 4
    );
    assert_eq!(
        read_frame(&mut bytes.as_slice()).expect("decode hello frame"),
        Some(message)
    );
}

#[test]
fn rejects_unknown_envelope_fields_and_oversized_lengths() {
    let malformed = Value::Map(vec![
        ("protocolVersion".into(), Value::Integer(1)),
        ("requestId".into(), Value::Integer(1)),
        ("kind".into(), Value::String("hello".into())),
        ("payload".into(), Value::Map(vec![])),
        ("unexpected".into(), Value::Bool(true)),
    ])
    .to_wire()
    .expect("encode malformed fixture");
    assert!(Message::from_wire(&malformed).is_err());

    let oversized = (MAX_FRAME_BYTES + 1).to_be_bytes();
    assert!(read_frame(&mut oversized.as_slice()).is_err());
}

#[test]
fn round_trips_every_command_response_and_callback_body() {
    let messages = vec![
        Message::new(1, MessageBody::Hello),
        Message::new(
            3,
            MessageBody::Load {
                package_root: "C:/plugins/example".into(),
                entrypoint: "payload/plugin.dll".into(),
                plugin_id: 11,
                session: 13,
            },
        ),
        Message::new(5, MessageBody::Enable),
        Message::new(
            7,
            MessageBody::Invoke {
                operation: "hook.before-game-launch".into(),
                input: vec![1, 2, 3],
                callback_id: 0,
            },
        ),
        Message::new(9, MessageBody::Disable),
        Message::new(11, MessageBody::Shutdown),
        Message::new(13, MessageBody::Ok),
        Message::new(15, MessageBody::Result { output: vec![4, 5] }),
        Message::new(
            17,
            MessageBody::Error {
                code: "plugin-status".into(),
                message: "Plugin callback failed".into(),
            },
        ),
        Message::new(
            2,
            MessageBody::BridgeInvoke {
                operation: "core.launcher-version".into(),
                input: vec![6],
            },
        ),
        Message::new(
            4,
            MessageBody::RetainHandle {
                object_id: 19,
                generation: 23,
            },
        ),
        Message::new(
            6,
            MessageBody::ReleaseHandle {
                object_id: 29,
                generation: 31,
            },
        ),
        Message::new(8, MessageBody::CallbackResult { output: vec![7, 8] }),
        Message::new(
            10,
            MessageBody::CallbackError {
                code: "permission-denied".into(),
            },
        ),
    ];

    for message in messages {
        let message = message.expect("valid protocol message");
        assert_eq!(
            Message::from_wire(&message.to_wire().expect("encode message"))
                .expect("decode message"),
            message
        );
    }
}

#[test]
fn rejects_direction_id_collisions_and_invalid_body_values() {
    assert!(Message::new(2, MessageBody::Hello).is_err());
    assert!(
        Message::new(
            1,
            MessageBody::BridgeInvoke {
                operation: "core.launcher-version".into(),
                input: vec![],
            }
        )
        .is_err()
    );
    assert!(
        Message::new(
            1,
            MessageBody::Load {
                package_root: "C:/plugins/example".into(),
                entrypoint: "payload/plugin.dll".into(),
                plugin_id: 1,
                session: 0,
            }
        )
        .is_err()
    );
    assert!(
        Message::new(
            1,
            MessageBody::Load {
                package_root: String::new(),
                entrypoint: "payload/plugin.dll".into(),
                plugin_id: 1,
                session: 1,
            }
        )
        .is_err()
    );
    assert!(
        Message::new(
            1,
            MessageBody::Invoke {
                operation: "bridge".into(),
                input: vec![],
                callback_id: i64::MAX as u64 + 1,
            }
        )
        .is_err()
    );
    assert!(
        Message::new(
            1,
            MessageBody::Invoke {
                operation: " ".into(),
                input: vec![],
                callback_id: 0,
            }
        )
        .is_err()
    );
    assert!(
        Message::new(
            1,
            MessageBody::Error {
                code: "plugin-status".into(),
                message: "x".repeat(4097),
            }
        )
        .is_err()
    );
    assert!(
        Message::new(
            1,
            MessageBody::Error {
                code: "Not_Canonical".into(),
                message: "failure".into(),
            }
        )
        .is_err()
    );
    assert!(
        Message::new(
            2,
            MessageBody::RetainHandle {
                object_id: 0,
                generation: 1,
            }
        )
        .is_err()
    );
}

#[test]
fn rejects_wrong_versions_kinds_payload_fields_and_truncation() {
    let wrong_version = raw_message(2, 1, "hello", Value::Map(vec![]));
    assert!(Message::from_wire(&wrong_version).is_err());

    let wrong_kind = raw_message(1, 1, "unknown", Value::Map(vec![]));
    assert!(Message::from_wire(&wrong_kind).is_err());

    let unexpected_payload = raw_message(
        1,
        1,
        "enable",
        Value::Map(vec![("unexpected".into(), Value::Bool(true))]),
    );
    assert!(Message::from_wire(&unexpected_payload).is_err());

    assert_eq!(read_frame(&mut [].as_slice()).expect("clean EOF"), None);
    assert!(read_frame(&mut [0, 0].as_slice()).is_err());
    assert!(read_frame(&mut [0, 0, 0, 0].as_slice()).is_err());

    let hello = Message::new(1, MessageBody::Hello).expect("valid hello");
    let mut framed = Vec::new();
    write_frame(&mut framed, &hello).expect("encode hello");
    framed.pop();
    assert!(read_frame(&mut Cursor::new(framed)).is_err());

    let mut invalid_utf8 = hello.to_wire().expect("encode hello wire");
    let hello_offset = invalid_utf8
        .windows(b"hello".len())
        .position(|window| window == b"hello")
        .expect("hello kind bytes");
    invalid_utf8[hello_offset] = 0xff;
    assert!(Message::from_wire(&invalid_utf8).is_err());

    let oversized_body = Message::new(
        3,
        MessageBody::Result {
            output: vec![0; MAX_FRAME_BYTES as usize],
        },
    )
    .expect("valid logical result");
    assert!(write_frame(&mut Vec::new(), &oversized_body).is_err());
}

fn raw_message(version: i64, request_id: i64, kind: &str, payload: Value) -> Vec<u8> {
    Value::Map(vec![
        ("protocolVersion".into(), Value::Integer(version)),
        ("requestId".into(), Value::Integer(request_id)),
        ("kind".into(), Value::String(kind.into())),
        ("payload".into(), payload),
    ])
    .to_wire()
    .expect("encode raw fixture")
}
