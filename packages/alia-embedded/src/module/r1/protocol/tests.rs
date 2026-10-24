use super::*;
use serde_json::json;

fn frame(kind: u8, id: u16, payload: Value) -> Frame {
    Frame {
        kind,
        id,
        payload: serde_json::to_vec(&payload).unwrap(),
    }
}

fn wake(fields: Value) -> Frame {
    frame(
        0x04,
        0x1104,
        json!({"type": "aiui_event", "content": {"eventType": 4, "info": {"ivw": fields}}}),
    )
}

fn hex(text: &str) -> Vec<u8> {
    text.as_bytes()
        .chunks_exact(2)
        .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
        .collect()
}

#[test]
fn version_query_matches_documented_golden_frame() {
    assert_eq!(
        version_query(1),
        hex("a50105130001007b2274797065223a202276657273696f6e227d9f")
    );
}

#[test]
fn only_valid_handshake_gets_same_id_ack_including_repeats() {
    let bytes = encode(0x01, 0xffff, HANDSHAKE_PAYLOAD);
    let mut decoder = Decoder::default();
    for _ in 0..2 {
        let input = decoder.push(&bytes).pop().unwrap();
        let ack = input.handshake_ack().unwrap();
        assert_eq!(sum(&ack), 0);
        let ack = decoder.push(&ack).pop().unwrap();
        assert_eq!((ack.kind, ack.id), (0xff, 0xffff));
        assert_eq!(ack.payload, HANDSHAKE_PAYLOAD);
        assert!(ack.handshake_ack().is_none());
    }
    for kind in [0x04, 0x05, 0x06, 0xff] {
        let input = decoder
            .push(&encode(kind, 4, HANDSHAKE_PAYLOAD))
            .pop()
            .unwrap();
        assert!(input.handshake_ack().is_none());
    }
    let input = decoder
        .push(&encode(0x01, 4, &[0xa5, 0, 0, 1]))
        .pop()
        .unwrap();
    assert!(input.handshake_ack().is_none());
}

#[test]
fn handles_every_split_and_coalesced_frames_without_deduplicating_ids() {
    let first = encode(0x04, 0x1104, b"first");
    let second = encode(0x04, 0x1104, b"second");
    for split in 0..first.len() {
        let mut decoder = Decoder::default();
        assert!(decoder.push(&first[..split]).is_empty());
        let frames = decoder.push(&first[split..]);
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0].payload, b"first");
        assert!(decoder.is_empty());
    }
    let joined = [b"noise".as_slice(), &first, &second].concat();
    let frames = Decoder::default().push(&joined);
    assert_eq!(frames.len(), 2);
    assert_eq!(frames[0].id, frames[1].id);
    assert_ne!(frames[0].payload, frames[1].payload);
}

#[test]
fn corruption_and_oversized_headers_recover_at_next_frame() {
    let good = version_query(29);
    let mut bad_checksum = good.clone();
    *bad_checksum.last_mut().unwrap() ^= 1;
    for corrupt in [
        bad_checksum,
        vec![0xa5, 2, 0x04, 0, 0, 0, 0],
        vec![0xa5, 1, 0x04, 1, 4, 0, 0],
    ] {
        let mut decoder = Decoder::default();
        let frames = decoder.push(&[corrupt, good.clone()].concat());
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0].id, 29);
        assert!(decoder.is_empty());
    }
}

#[test]
fn retained_buffer_is_bounded_and_reset_discards_incomplete_frames() {
    let mut decoder = Decoder::default();
    assert!(decoder.push(&vec![0xa5; 100_000]).is_empty());
    assert!(decoder.buffer.len() < HEADER_BYTES);
    decoder.clear();
    let maximum = encode(0x06, 0, &vec![0x7f; MAX_PAYLOAD]);
    assert!(decoder.push(&maximum[..maximum.len() - 1]).is_empty());
    assert_eq!(decoder.buffer.len(), MAX_PAYLOAD + HEADER_BYTES);
    assert_eq!(decoder.push(&maximum[maximum.len() - 1..]).len(), 1);
    assert!(decoder.is_empty());
    assert!(decoder.push(&[0xa5, 1, 4, 0, 4, 0, 0]).is_empty());
    decoder.clear();
    assert_eq!(decoder.push(&version_query(7))[0].id, 7);
}

#[test]
fn parses_captured_current_firmware_wakeup_without_inventing_confidence() {
    // Existing FYP r1-raw/b9743909cf2b4aa7ae7a9821c8dace08-control.jsonl, no audio.
    let bytes = hex(concat!(
        "a50104190104117b22636f6e74656e74223a7b2261726731223a322c2261726732223a302c226576656e7454797065223a342c22696e666f223a227b5c226976775c223a7b5c2273746172745f6d735c223a31343736343135302c5c22656e645f6d735c223a31343736353230302c5c226265616d5c223a302c5c22706879736963616c5c223a302c5c2273636f72655c223a313938312e302c5c22706f7765725c223a35333234303630302e3030302c5c22616e676c655c223a3335352e302c5c226b6579776f72645c223a5c227869616f332d666569312d7869616f332d666569315c227d7d222c22726573756c74223a227869616f332d666569312d7869616f332d66656931227d2c2274797065223a22616975695f6576656e74227df0"
    ));
    let parsed = Decoder::default().push(&bytes).pop().unwrap();
    assert_eq!(parsed.id, 0x1104);
    assert_eq!(
        parsed.wakeup(),
        Some(Wakeup {
            keyword: "xiao3-fei1-xiao3-fei1".into(),
            angle_deg: Some(355.0),
            beam: Some(0),
        })
    );
}

#[test]
fn deeper_fields_override_even_when_invalid_and_physical_has_priority() {
    let parsed = frame(
        4,
        0,
        json!({
            "type": "aiui_event", "angle": 30, "beam": 1, "keyword": "outer",
            "content": {"angle": 60, "info": {"ivw": {
                "angle": false, "physical": 6, "beam": 0, "keyword": "inner"
            }}}
        }),
    )
    .wakeup()
    .unwrap();
    assert_eq!(parsed.keyword, "inner");
    assert_eq!(parsed.angle_deg, None);
    assert_eq!(parsed.beam, None);
    assert!(
        frame(
            4,
            0,
            json!({"type": "aiui_event", "keyword": "outer", "content": {"info": "broken"}})
        )
        .wakeup()
        .is_none()
    );
    assert!(
        frame(
            4,
            0,
            json!({"type": "aiui_event", "keyword": "outer", "content": {"keyword": null}})
        )
        .wakeup()
        .is_none()
    );
}

#[test]
fn validates_bearing_and_beam_without_guessing_missing_values() {
    let parsed = wake(json!({"keyword": "wake"})).wakeup().unwrap();
    assert_eq!((parsed.angle_deg, parsed.beam), (None, None));
    for invalid in [json!(true), json!("0"), json!(-1), json!(361), Value::Null] {
        assert_eq!(
            wake(json!({"keyword": "wake", "angle": invalid}))
                .wakeup()
                .unwrap()
                .angle_deg,
            None
        );
    }
    for invalid in [
        json!(true),
        json!("0"),
        json!(-1),
        json!(6),
        json!(1.5),
        Value::Null,
    ] {
        assert_eq!(
            wake(json!({"keyword": "wake", "beam": invalid}))
                .wakeup()
                .unwrap()
                .beam,
            None
        );
    }
    let parsed = wake(json!({"keyword": "wake", "angle": 360, "physical": 5.0}))
        .wakeup()
        .unwrap();
    assert_eq!((parsed.angle_deg, parsed.beam), (Some(0.0), Some(5)));
    assert_eq!(
        wake(json!({"keyword": "wake", "angle": 359.999999}))
            .wakeup()
            .unwrap()
            .angle_deg,
        Some(359.999999)
    );
}

#[test]
fn rejects_non_wakeup_events_and_invalid_keywords() {
    for keyword in [
        Value::Null,
        json!(5),
        json!(""),
        json!(" \t"),
        json!("a".repeat(MAX_KEYWORD_BYTES + 1)),
    ] {
        assert!(wake(json!({"keyword": keyword})).wakeup().is_none());
    }
    for body in [
        json!({"keyword": "wake"}),
        json!({"type": "status", "keyword": "wake"}),
        json!({"type": "aiui_event", "content": {"eventType": 3, "keyword": "wake"}}),
        json!({"type": "aiui_event", "content": {"eventType": true, "keyword": "wake"}}),
    ] {
        assert!(frame(4, 0, body).wakeup().is_none());
    }
    let mut event = wake(json!({"keyword": "wake"}));
    event.kind = 5;
    assert!(event.wakeup().is_none());
    event.kind = 4;
    event.payload = vec![0xff];
    assert!(event.wakeup().is_none());
}

#[test]
fn version_requires_matching_reply_success_code_and_version_text() {
    let good = frame(
        5,
        17,
        json!({"code": 0, "content": "version: UART_REAL, circle6:1.9.6"}),
    );
    assert!(good.version_response(17).is_some());
    assert!(good.version_response(18).is_none());
    for body in [
        json!({"code": false, "content": "version: test"}),
        json!({"code": 1, "content": "version: test"}),
        json!({"code": "0", "content": "version: test"}),
        json!({"code": 0, "content": "success"}),
        json!({"code": 0, "content": "version: "}),
        json!({"code": 0}),
    ] {
        assert!(frame(5, 17, body).version_response(17).is_none());
    }
    assert!(
        frame(4, 17, json!({"code": 0, "content": "version: test"}))
            .version_response(17)
            .is_none()
    );
}
