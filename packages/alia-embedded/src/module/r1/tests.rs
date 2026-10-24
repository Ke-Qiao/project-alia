use super::*;

fn wake(angle: f64) -> protocol::Frame {
    protocol::Frame {
        kind: 4,
        id: 0x1104,
        payload: serde_json::to_vec(&serde_json::json!({
            "type": "aiui_event", "content": {"eventType": 4, "info": {"ivw": {
                "keyword": "小飞小飞", "angle": angle, "physical": 5
            }}}
        }))
        .unwrap(),
    }
}

fn ready() -> State {
    State {
        version: Some("version: test".into()),
        ..State::default()
    }
}

#[test]
fn wake_requires_verified_version_and_expires_even_without_pcm() {
    let now = Instant::now();
    let mut state = State::default();
    state.receive(0, &wake(12.5), now, 123);
    assert!(state.detect(now).is_none());
    state.version = Some("version: test".into());
    state.receive(0, &wake(12.5), now, 123);
    assert!(state.detect(now + EVENT_TTL).is_none());
}

#[test]
fn fixed_firmware_message_id_does_not_drop_distinct_wakes_and_slot_keeps_latest() {
    let now = Instant::now();
    let mut state = ready();
    state.receive(0, &wake(12.5), now, 123);
    state.receive(0, &wake(359.999999), now, 124);
    let event = state.detect(now).unwrap();
    assert_eq!(event.keyword, "小飞小飞");
    assert!(matches!(
        event.source,
        WakeWordSource::R1 {
            message_id: 0x1104,
            angle_deg: Some(359.999999),
            beam: Some(5)
        }
    ));
    assert_eq!(event.timestamp_us, 124);
    assert!(state.detect(now).is_none());
}

#[test]
fn retransmission_deduplication_expires_and_reset_rejects_old_generation() {
    let now = Instant::now();
    let mut state = ready();
    let frame = wake(90.0);
    state.receive(0, &frame, now, 123);
    assert!(state.detect(now).is_some());
    state.receive(0, &frame, now + Duration::from_millis(500), 123);
    assert!(state.detect(now + Duration::from_millis(500)).is_none());
    state.receive(0, &frame, now + EVENT_TTL, 123);
    assert!(state.detect(now + EVENT_TTL).is_some());
    state.reset().unwrap();
    state.receive(0, &frame, now + EVENT_TTL, 123);
    assert!(state.detect(now + EVENT_TTL).is_none());
    state.receive(1, &frame, now + EVENT_TTL, 123);
    assert!(state.detect(now + EVENT_TTL).is_none());
    state.receive(1, &wake(91.0), now + EVENT_TTL, 123);
    assert!(state.pending.is_some());
    state.disconnect(Some("unplugged".into()));
    assert!(state.detect(now + EVENT_TTL).is_none());
    assert!(state.version.is_none());
}

#[test]
fn reset_generation_exhaustion_is_reported() {
    let mut state = ready();
    state.generation = u64::MAX;
    assert!(state.reset().is_err());
}

#[cfg(target_os = "linux")]
mod linux {
    use super::*;
    use std::fs::File;
    use std::io::{ErrorKind, Read, Write};

    const VERSION_ID: u16 = 0x7ffe;

    fn wire(frame: &protocol::Frame) -> Vec<u8> {
        let mut bytes = vec![0xa5, 1, frame.kind];
        bytes.extend_from_slice(&(frame.payload.len() as u16).to_le_bytes());
        bytes.extend_from_slice(&frame.id.to_le_bytes());
        bytes.extend_from_slice(&frame.payload);
        bytes.push(0u8.wrapping_sub(bytes.iter().fold(0u8, |sum, byte| sum.wrapping_add(*byte))));
        bytes
    }

    fn version(id: u16) -> Vec<u8> {
        wire(&protocol::Frame {
            kind: 5,
            id,
            payload: br#"{"code":0,"content":"version: UART_REAL, circle6:1.9.6.test"}"#.to_vec(),
        })
    }

    fn await_condition(mut condition: impl FnMut() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(1);
        while !condition() {
            assert!(Instant::now() < deadline, "condition did not become true");
            thread::sleep(Duration::from_millis(5));
        }
    }

    fn read_frame(master: &mut File) -> protocol::Frame {
        let mut decoder = protocol::Decoder::default();
        let mut result = None;
        await_condition(|| {
            let mut bytes = [0u8; 2048];
            match master.read(&mut bytes) {
                Ok(count) => result = decoder.push(&bytes[..count]).into_iter().next(),
                Err(error) if error.kind() == ErrorKind::WouldBlock => {}
                other => panic!("reading PTY: {other:?}"),
            }
            result.is_some()
        });
        result.unwrap()
    }

    #[test]
    fn pty_requires_version_match_acks_handshake_and_reports_host_wake() {
        let (mut master, serial, _) = serial::pty();
        let state = Arc::new(Mutex::new(State::default()));
        let cancel = CancellationToken::new();
        let worker = {
            let (state, cancel) = (state.clone(), cancel.clone());
            thread::spawn(move || session(serial, &state, &cancel, VERSION_ID))
        };
        let query = read_frame(&mut master);
        assert_eq!(wire(&query), protocol::version_query(VERSION_ID));
        master.write_all(&version(VERSION_ID - 1)).unwrap();
        master.write_all(&wire(&wake(10.0))).unwrap();
        thread::sleep(Duration::from_millis(30));
        assert!(state.lock().unwrap().version.is_none());
        assert!(state.lock().unwrap().pending.is_none());
        let handshake = protocol::Frame {
            kind: 1,
            id: 10,
            payload: vec![0xa5, 0, 0, 0],
        };
        master.write_all(&wire(&handshake)).unwrap();
        assert_eq!(
            wire(&read_frame(&mut master)),
            handshake.handshake_ack().unwrap()
        );
        master.write_all(&version(VERSION_ID)).unwrap();
        await_condition(|| state.lock().unwrap().version.is_some());
        let frame = wire(&wake(359.999999));
        master.write_all(&frame[..6]).unwrap();
        thread::sleep(Duration::from_millis(20));
        master.write_all(&frame[6..]).unwrap();
        await_condition(|| state.lock().unwrap().pending.is_some());
        assert!(state.lock().unwrap().detect(Instant::now()).is_some());
        cancel.cancel();
        worker.join().unwrap().unwrap();
    }

    #[test]
    fn pty_reset_and_absolute_partial_timeout_discard_old_bytes() {
        let (mut master, serial, _) = serial::pty();
        let state = Arc::new(Mutex::new(State::default()));
        let cancel = CancellationToken::new();
        let worker = {
            let (state, cancel) = (state.clone(), cancel.clone());
            thread::spawn(move || session(serial, &state, &cancel, VERSION_ID))
        };
        read_frame(&mut master);
        master.write_all(&version(VERSION_ID)).unwrap();
        await_condition(|| state.lock().unwrap().version.is_some());
        let frame = wire(&wake(10.0));
        master.write_all(&frame[..6]).unwrap();
        thread::sleep(Duration::from_millis(150));
        state.lock().unwrap().reset().unwrap();
        thread::sleep(Duration::from_millis(150));
        master.write_all(&frame[6..]).unwrap();
        thread::sleep(Duration::from_millis(30));
        assert!(state.lock().unwrap().pending.is_none());
        master.write_all(&[0xa5, 1, 4, 0, 4, 0, 0]).unwrap();
        for _ in 0..5 {
            thread::sleep(Duration::from_millis(80));
            master.write_all(b"x").unwrap();
        }
        master.write_all(&frame).unwrap();
        await_condition(|| state.lock().unwrap().pending.is_some());
        drop(master);
        assert!(worker.join().unwrap().is_err());
    }

    #[test]
    fn pty_backpressure_and_cancel_do_not_leave_unbounded_ack_writes() {
        let (_master, mut serial, _) = serial::pty();
        let cancel = CancellationToken::new();
        let started = Instant::now();
        assert!(
            serial
                .write(&vec![0xa5; 128 * 1024], &cancel)
                .unwrap_err()
                .to_string()
                .contains("timed out")
        );
        assert!(started.elapsed() < Duration::from_secs(1));
        cancel.cancel();
        assert!(
            serial
                .write(b"ack", &cancel)
                .unwrap_err()
                .to_string()
                .contains("cancelled")
        );
    }

    #[test]
    fn missing_version_after_reset_fails_with_bounded_deadline() {
        let (mut master, serial, _) = serial::pty();
        let state = Arc::new(Mutex::new(State::default()));
        let cancel = CancellationToken::new();
        let started = Instant::now();
        let worker = {
            let (state, cancel) = (state.clone(), cancel.clone());
            thread::spawn(move || session(serial, &state, &cancel, VERSION_ID))
        };
        read_frame(&mut master);
        for _ in 0..3 {
            thread::sleep(Duration::from_millis(300));
            state.lock().unwrap().reset().unwrap();
            read_frame(&mut master);
        }
        let error = worker.join().unwrap().unwrap_err();
        assert!(error.to_string().contains("version response timed out"));
        assert!(started.elapsed() < Duration::from_secs(3));
        assert!(state.lock().unwrap().version.is_none());
    }

    #[test]
    fn capture_reset_replaces_version_query_on_same_connection_and_rejects_old_reply() {
        let (mut master, serial, _) = serial::pty();
        let state = Arc::new(Mutex::new(State::default()));
        let cancel = CancellationToken::new();
        let worker = {
            let (state, cancel) = (state.clone(), cancel.clone());
            thread::spawn(move || session(serial, &state, &cancel, VERSION_ID))
        };
        assert_eq!(read_frame(&mut master).id, VERSION_ID);
        state.lock().unwrap().reset().unwrap();
        let replacement = read_frame(&mut master);
        assert_eq!(replacement.id, VERSION_ID.wrapping_add(1));
        assert_eq!(wire(&replacement), protocol::version_query(replacement.id));
        master.write_all(&version(VERSION_ID)).unwrap();
        master.write_all(&wire(&wake(10.0))).unwrap();
        thread::sleep(Duration::from_millis(30));
        assert!(state.lock().unwrap().version.is_none());
        assert!(state.lock().unwrap().pending.is_none());
        master.write_all(&version(replacement.id)).unwrap();
        await_condition(|| state.lock().unwrap().version.is_some());
        cancel.cancel();
        worker.join().unwrap().unwrap();
    }

    #[test]
    fn board_handshake_without_tty_disconnect_invalidates_verified_session() {
        let (mut master, serial, _) = serial::pty();
        let state = Arc::new(Mutex::new(State::default()));
        let cancel = CancellationToken::new();
        let worker = {
            let (state, cancel) = (state.clone(), cancel.clone());
            thread::spawn(move || session(serial, &state, &cancel, VERSION_ID))
        };
        read_frame(&mut master);
        master.write_all(&version(VERSION_ID)).unwrap();
        master.write_all(&wire(&wake(10.0))).unwrap();
        await_condition(|| state.lock().unwrap().pending.is_some());
        let handshake = protocol::Frame {
            kind: 1,
            id: 5,
            payload: vec![0xa5, 0, 0, 0],
        };
        master.write_all(&wire(&handshake)).unwrap();
        let error = worker.join().unwrap().unwrap_err();
        assert!(error.to_string().contains("version must be verified again"));
        assert!(state.lock().unwrap().version.is_none());
        assert!(state.lock().unwrap().pending.is_none());
        assert!(state.lock().unwrap().previous.is_none());
        assert_eq!(
            wire(&read_frame(&mut master)),
            handshake.handshake_ack().unwrap()
        );
    }

    #[test]
    fn strict_open_does_not_ignore_missing_modem_line_support() {
        let (_master, _serial, path) = serial::pty();
        let error = serial::Serial::open(&path)
            .err()
            .expect("PTY cannot set modem lines");
        assert!(format!("{error:#}").contains("DTR/RTS"));
        assert!(
            std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .open(path)
                .is_ok()
        );
    }

    #[test]
    fn stop_joins_original_worker_and_prevents_restart() {
        let owner = Arc::new(
            R1WakeWord::new(&R1Config {
                serial_path: "/not-opened".into(),
            })
            .unwrap(),
        );
        let cancel = owner.cancel.clone();
        let exited = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let finished = exited.clone();
        *owner.worker.lock().unwrap() = Some(thread::spawn(move || {
            while !cancel.is_cancelled() {
                thread::sleep(Duration::from_millis(5));
            }
            thread::sleep(Duration::from_millis(20));
            finished.store(true, std::sync::atomic::Ordering::Release);
        }));
        let other = owner.clone();
        let stopper = thread::spawn(move || other.stop_worker());
        owner.stop_worker().unwrap();
        stopper.join().unwrap().unwrap();
        assert!(exited.load(std::sync::atomic::Ordering::Acquire));
        assert!(owner.worker.lock().unwrap().is_none());
        let runtime = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap();
        assert!(runtime.block_on(owner.start()).is_err());
    }
}
