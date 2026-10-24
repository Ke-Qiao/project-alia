use std::time::Duration;

use serde_json::{Value, json};

use super::super::protocol::{self, Frame};
use super::transaction::{Phase, Transaction};

fn response(id: u16, value: Value) -> Frame {
    Frame {
        kind: 5,
        id,
        payload: serde_json::to_vec(&value).unwrap(),
    }
}

fn ready() -> Transaction {
    let mut transaction = Transaction::new(100);
    transaction
        .receive(
            &response(
                100,
                json!({"code":0,"content":protocol::MANUAL_CAPTURE_FIRMWARE}),
            ),
            Duration::ZERO,
        )
        .unwrap();
    transaction
}

fn handshake() -> Frame {
    Frame {
        kind: 1,
        id: 9,
        payload: vec![0xa5, 0, 0, 0],
    }
}

#[test]
fn handshake_requeries_same_version_once_without_extending_deadline() {
    let mut transaction = Transaction::new(100);
    for now in [Duration::from_millis(1900), Duration::from_millis(1999)] {
        let ack = transaction.receive(&handshake(), now).unwrap().unwrap();
        assert_eq!(ack, handshake().handshake_ack().unwrap());
        let query = transaction.requery(now).unwrap();
        if now.as_millis() == 1900 {
            assert_eq!(query, Some(protocol::version_query(100)));
        } else {
            assert!(query.is_none());
        }
    }
    assert!(transaction.requery(Duration::from_secs(2)).is_err());
    assert!(
        transaction
            .receive(
                &response(
                    100,
                    json!({"code":0,"content":protocol::MANUAL_CAPTURE_FIRMWARE})
                ),
                Duration::from_secs(2)
            )
            .is_err()
    );
    assert_eq!(transaction.phase, Phase::Version);
    assert!(transaction.manual(Duration::from_secs(2)).is_err());
}

#[test]
fn exact_historical_firmware_is_required_before_manual_command() {
    let mut transaction = Transaction::new(100);
    let observed = "version: UART_REAL, circle6:1.9.6.f8ef651@master engine:circle6.1004; build date: Sep  3 2026 22:40:02";
    transaction
        .receive(
            &response(100, json!({"code":0,"content":observed})),
            Duration::ZERO,
        )
        .unwrap();
    assert_eq!(transaction.phase, Phase::Ready);
    for changed in [
        observed.replace("Sep  3", "Sep 3"),
        "version: different".into(),
    ] {
        let mut transaction = Transaction::new(100);
        assert!(
            transaction
                .receive(
                    &response(100, json!({"code":0,"content":changed})),
                    Duration::ZERO
                )
                .is_err()
        );
        assert!(transaction.manual(Duration::ZERO).is_err());
    }
}

#[test]
fn manual_is_one_shot_even_if_write_fails_or_ack_times_out() {
    let mut transaction = ready();
    let bytes = transaction.manual(Duration::from_secs(5)).unwrap();
    let command = protocol::Decoder::default().push(&bytes).pop().unwrap();
    assert_eq!((command.kind, command.id), (5, 101));
    assert_eq!(
        serde_json::from_slice::<Value>(&command.payload).unwrap(),
        json!({"type":"manual_wakeup","content":{"beam":0}})
    );
    // No serial completion is required to consume this attempt.
    assert!(transaction.manual(Duration::from_secs(5)).is_err());
    assert!(transaction.check_deadline(Duration::from_secs(7)).is_err());
    assert!(
        transaction
            .receive(&response(101, json!({"code":0})), Duration::from_secs(7))
            .is_err()
    );
    assert!(transaction.manual(Duration::from_secs(7)).is_err());
}

#[test]
fn ack_requires_matching_kind_id_numeric_zero_and_an_attempt() {
    let mut transaction = ready();
    transaction
        .receive(&response(101, json!({"code":0})), Duration::ZERO)
        .unwrap();
    assert_eq!(transaction.phase, Phase::Ready);
    transaction.manual(Duration::ZERO).unwrap();
    for frame in [
        response(100, json!({"code":0})),
        Frame {
            kind: 4,
            ..response(101, json!({"code":0}))
        },
    ] {
        transaction.receive(&frame, Duration::ZERO).unwrap();
        assert_eq!(transaction.phase, Phase::Ack);
    }
    transaction
        .receive(&response(101, json!({"code":0})), Duration::ZERO)
        .unwrap();
    assert_eq!(transaction.phase, Phase::Accepted);
    assert!(transaction.manual(Duration::ZERO).is_err());
    for value in [
        json!({"code":"0"}),
        json!({"code":1}),
        json!({"code":0.0}),
        json!({}),
        json!({"code":null}),
    ] {
        let mut transaction = ready();
        transaction.manual(Duration::ZERO).unwrap();
        assert!(
            transaction
                .receive(&response(101, value), Duration::ZERO)
                .is_err()
        );
        assert!(transaction.manual(Duration::ZERO).is_err());
    }
    let mut transaction = ready();
    transaction.manual(Duration::ZERO).unwrap();
    assert!(
        transaction
            .receive(
                &Frame {
                    kind: 5,
                    id: 101,
                    payload: b"invalid json".to_vec()
                },
                Duration::ZERO
            )
            .is_err()
    );
}

#[test]
fn board_restart_after_verified_version_never_retriggers_manual() {
    for manual in [false, true] {
        let mut transaction = ready();
        if manual {
            transaction.manual(Duration::ZERO).unwrap();
        }
        assert!(transaction.receive(&handshake(), Duration::ZERO).is_err());
        if manual {
            assert!(transaction.manual(Duration::ZERO).is_err());
        }
    }
}

#[test]
fn command_ack_is_never_a_natural_wakeup_event() {
    let frame = response(101, json!({"code":0}));
    assert_eq!(frame.command_ack(101), Some(true));
    assert!(frame.wakeup().is_none());
}

#[test]
fn pre_manual_board_events_reject_claimed_cold_boot_but_never_synthesize_wake() {
    let event = Frame {
        kind: 4,
        id: 5,
        payload: b"any board event".to_vec(),
    };
    for mut transaction in [Transaction::new(100), ready()] {
        assert!(transaction.receive(&event, Duration::ZERO).is_err());
    }
    let mut transaction = ready();
    transaction.manual(Duration::ZERO).unwrap();
    assert!(
        transaction
            .receive(&event, Duration::ZERO)
            .unwrap()
            .is_none()
    );
    assert_eq!(transaction.phase, Phase::Ack);
}
