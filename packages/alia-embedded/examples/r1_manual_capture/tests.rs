use std::time::Duration;

use alia_core::audio_clock::*;
use alia_embedded::module::context::AudioFrame;
use serde_json::json;
use uuid::Uuid;

use super::probe::{Phase, Probe};
use super::{FIRMWARE, parse, protocol};

fn response(id: u16, body: serde_json::Value) -> protocol::Frame {
    protocol::Frame {
        kind: 5,
        id,
        payload: serde_json::to_vec(&body).unwrap(),
    }
}

fn baseline() -> Probe {
    let mut probe = Probe::new(0, Duration::from_secs(300), 100);
    probe
        .receive(
            &response(100, json!({"code":0,"content":FIRMWARE})),
            Duration::ZERO,
        )
        .unwrap();
    probe
}

fn audio(start: u64, samples: Vec<i16>) -> AudioFrame {
    let id = Uuid::from_u128(1);
    let format = AudioFormat {
        sample_rate_hz: 16000,
        channels: 1,
    };
    AudioFrame {
        sample_rate: 16000,
        timestamp_us: start * 1_000_000 / 16000,
        timing: Some(CaptureTiming {
            native: AudioTiming {
                stream: AudioStreamId {
                    boot_id: id,
                    stream_id: id,
                    direction: AudioDirection::Capture,
                },
                format,
                start_frame: start,
                frames: samples.len() as u64,
                continuity: 1,
                clock: AudioClockReading {
                    clock_id: id,
                    callback_ns: None,
                    sample_ns: None,
                    queued_frames: None,
                    quality: AudioTimeQuality::Unknown,
                },
                observed_boot_ns: start * 1_000_000_000 / 16000,
            },
            converted_format: format,
            converted_start_frame: start,
            converted_frames: samples.len() as u64,
            resampled: false,
        }),
        pcm: samples,
    }
}

fn waiting_ack() -> Probe {
    let mut probe = baseline();
    probe
        .audio(&audio(0, vec![0; 1600]), Duration::from_secs(4))
        .unwrap();
    assert!(probe.tick(Duration::from_secs(5)).unwrap().is_some());
    probe
}

#[test]
fn historical_firmware_reply_preserves_the_space_padded_build_day() {
    // Independent fixture from the accepted Pi logs: the build day is space padded.
    let payload = br#"{"code":0,"content":"version: UART_REAL, circle6:1.9.6.f8ef651@master engine:circle6.1004; build date: Sep  3 2026 22:40:02"}"#;
    let frame = protocol::Decoder::default()
        .push(&protocol::encode(5, 100, payload))
        .pop()
        .unwrap();
    let mut probe = Probe::new(0, Duration::from_secs(300), 100);
    probe.receive(&frame, Duration::ZERO).unwrap();
    assert_eq!(probe.phase, Phase::Baseline);
    assert!(probe.version.as_deref().unwrap().contains("Sep  3"));
    assert_eq!(probe.manual_attempts, 0);
}

#[test]
fn cold_start_handshake_requeries_version_once_then_manual_wakeup_stays_single() {
    let mut probe = Probe::new(0, Duration::from_secs(300), 15155);
    probe.phase_started = Duration::from_millis(52);
    let handshake = protocol::Frame {
        kind: 1,
        id: 135,
        payload: vec![0xa5, 0, 0, 0],
    };
    let initial_query = protocol::version_query(probe.version_id);
    assert!(probe.tick(Duration::from_millis(153)).unwrap().is_none());
    let ack = probe
        .receive(&handshake, Duration::from_millis(205))
        .unwrap()
        .unwrap();
    let ack = protocol::Decoder::default().push(&ack).pop().unwrap();
    assert_eq!(
        (ack.kind, ack.id, ack.payload),
        (255, 135, handshake.payload.clone())
    );
    // The live loop writes this ACK before calling tick and writing the one requery.
    assert_eq!(
        probe.tick(Duration::from_millis(205)).unwrap(),
        Some(initial_query)
    );
    for ms in [206, 210] {
        assert!(
            probe
                .receive(&handshake, Duration::from_millis(ms))
                .unwrap()
                .is_some()
        );
        assert!(probe.tick(Duration::from_millis(ms)).unwrap().is_none());
    }
    probe
        .receive(
            &response(probe.manual_id, json!({"code":0,"content":FIRMWARE})),
            Duration::from_millis(215),
        )
        .unwrap();
    assert_eq!(probe.phase, Phase::Version);
    probe
        .receive(
            &response(probe.version_id, json!({"code":0,"content":FIRMWARE})),
            Duration::from_millis(220),
        )
        .unwrap();
    assert_eq!(probe.phase, Phase::Baseline);
    assert_eq!(probe.report()["version_requery_attempted"], true);
    probe
        .audio(&audio(0, vec![0; 1600]), Duration::from_secs(4))
        .unwrap();
    let manual = probe.tick(Duration::from_millis(5220)).unwrap().unwrap();
    let manual = protocol::Decoder::default().push(&manual).pop().unwrap();
    assert_eq!(manual.id, probe.manual_id);
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&manual.payload).unwrap()["type"],
        "manual_wakeup"
    );
    probe
        .receive(
            &response(probe.manual_id, json!({"code":0})),
            Duration::from_millis(5230),
        )
        .unwrap();
    for (ms, expected) in [
        (20230, Phase::SpeechBefore),
        (50229, Phase::SpeechBefore),
        (50230, Phase::Silence),
        (350229, Phase::Silence),
        (350230, Phase::SpeechAfter),
        (365230, Phase::SpeechAfter),
        (395229, Phase::SpeechAfter),
        (395230, Phase::Done),
        (400000, Phase::Done),
    ] {
        assert!(probe.tick(Duration::from_millis(ms)).unwrap().is_none());
        assert_eq!(probe.phase, expected);
    }
    assert_eq!(probe.phase, Phase::Done);
    assert_eq!(probe.manual_attempts, 1);
}

#[test]
fn repeated_cold_start_handshakes_do_not_extend_version_deadline() {
    let mut probe = Probe::new(0, Duration::from_secs(300), 100);
    let handshake = protocol::Frame {
        kind: 1,
        id: 135,
        payload: vec![0xa5, 0, 0, 0],
    };
    probe
        .receive(&handshake, Duration::from_millis(1950))
        .unwrap();
    assert!(probe.tick(Duration::from_millis(1950)).unwrap().is_some());
    probe
        .receive(&handshake, Duration::from_millis(1999))
        .unwrap();
    assert!(probe.tick(Duration::from_millis(1999)).unwrap().is_none());
    let error = probe.tick(Duration::from_secs(2)).unwrap_err();
    assert!(error.to_string().contains("manual_wakeup was not sent"));
    assert!(
        probe
            .receive(
                &response(100, json!({"code":0,"content":FIRMWARE})),
                Duration::from_secs(2)
            )
            .unwrap_err()
            .to_string()
            .contains("manual_wakeup was not sent")
    );
    assert_eq!(probe.phase_started, Duration::ZERO);
    assert_eq!(probe.manual_attempts, 0);
}

#[test]
fn one_manual_command_survives_five_minutes_of_silent_pcm_then_audio_recovers() {
    let mut probe = baseline();
    let mut commands = Vec::new();
    let mut cursor = 0;
    for step in 1..=4000 {
        let now = Duration::from_millis(step * 100);
        let speaking = matches!(probe.phase, Phase::SpeechBefore | Phase::SpeechAfter);
        let pcm = (0..1600)
            .map(|i| {
                if speaking {
                    if i % 2 == 0 { 2000 } else { -2000 }
                } else {
                    0
                }
            })
            .collect();
        probe.audio(&audio(cursor, pcm), now).unwrap();
        cursor += 1600;
        probe.check_audio_progress(now).unwrap();
        if let Some(bytes) = probe.tick(now).unwrap() {
            commands.extend(protocol::Decoder::default().push(&bytes));
            probe
                .receive(
                    &response(probe.manual_id, json!({"code":0,"content":"success"})),
                    now,
                )
                .unwrap();
        }
        if probe.phase == Phase::Done {
            break;
        }
    }
    assert_eq!(commands.len(), 1);
    assert_eq!(commands[0].id, probe.manual_id);
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&commands[0].payload).unwrap(),
        json!({"type":"manual_wakeup","content":{"beam":0}})
    );
    assert_eq!(probe.phase, Phase::Done);
    let report = probe.report();
    assert_eq!(report["phases"][1]["levels"]["samples"], 5 * 16000);
    assert_eq!(report["phases"][3]["levels"]["samples"], 45 * 16000);
    assert_eq!(report["phases"][4]["levels"]["samples"], 300 * 16000);
    assert_eq!(report["phases"][4]["levels"]["nonzero_samples"], 0);
    assert_eq!(report["phases"][5]["levels"]["samples"], 45 * 16000);
    assert_eq!(report["phases"][5]["levels"]["ac_rms"], 2000.0);
    assert_eq!(report["manual_ack"], true);
    assert_eq!(report["audio_validity"], "not_established_by_statistics");
}

#[test]
fn only_timely_matching_numeric_success_ack_is_accepted_and_timeout_never_retries() {
    let now = Duration::from_millis(5100);
    let mut probe = waiting_ack();
    for frame in [
        response(100, json!({"code":0})),
        response(999, json!({"code":0})),
        protocol::Frame {
            kind: 4,
            ..response(101, json!({"code":0}))
        },
    ] {
        assert!(probe.receive(&frame, now).unwrap().is_none());
        assert!(!probe.manual_ack);
    }
    for code in [json!(1), json!("0"), json!(null)] {
        let mut probe = waiting_ack();
        assert!(
            probe
                .receive(&response(101, json!({"code":code})), now)
                .is_err()
        );
        assert!(!probe.manual_ack);
    }
    assert!(probe.tick(Duration::from_secs(7)).is_err());
    assert!(probe.tick(Duration::from_secs(100)).is_err());
    assert_eq!(probe.manual_attempts, 1);
    assert!(
        probe
            .receive(&response(101, json!({"code":0})), Duration::from_secs(100))
            .is_err()
    );
    assert!(!probe.manual_ack);
    let mut probe = baseline();
    probe
        .receive(&response(101, json!({"code":0})), Duration::from_secs(1))
        .unwrap();
    assert!(!probe.manual_ack); // An ACK arriving before the attempt cannot acknowledge it.
}

#[test]
fn changed_firmware_and_pre_manual_events_block_manual_command() {
    let mut probe = Probe::new(0, Duration::from_secs(300), 100);
    assert!(
        probe
            .receive(
                &response(100, json!({"code":0,"content":"version: other"})),
                Duration::ZERO
            )
            .is_err()
    );
    assert_eq!(probe.manual_attempts, 0);
    let mut probe = baseline();
    let event = protocol::Frame {
        kind: 4,
        ..response(
            7,
            json!({"type":"aiui_event",
        "content":{"eventType":4,"info":{"ivw":{"keyword":"wake","angle":45}}}}),
        )
    };
    probe
        .audio(&audio(0, vec![0; 1600]), Duration::from_secs(1))
        .unwrap();
    probe.receive(&event, Duration::from_secs(2)).unwrap();
    assert!(probe.tick(Duration::from_secs(5)).is_err());
    assert_eq!(probe.manual_attempts, 0);
    let mut probe = waiting_ack();
    probe.receive(&event, Duration::from_millis(5100)).unwrap();
    assert_eq!(
        probe.report()["manual_only_isolation"],
        "requires_event_review"
    );
    assert_eq!(probe.wake_events, 1);
}

#[test]
fn board_handshake_after_version_invalidates_the_experiment_without_retriggering() {
    let handshake = protocol::Frame {
        kind: 1,
        id: 77,
        payload: vec![0xa5, 0, 0, 0],
    };
    let mut probe = Probe::new(0, Duration::from_secs(300), 100);
    assert!(probe.receive(&handshake, Duration::ZERO).unwrap().is_some());
    let mut probe = baseline();
    assert!(probe.receive(&handshake, Duration::from_secs(1)).is_err());
    assert_eq!(probe.manual_attempts, 0);
    let mut probe = waiting_ack();
    assert!(
        probe
            .receive(&handshake, Duration::from_millis(5100))
            .is_err()
    );
    assert_eq!(probe.manual_attempts, 1);
    assert!(!probe.manual_ack);
}

#[test]
fn lost_reopened_untimed_and_resampled_audio_cannot_look_continuous() {
    for damage in 0..6 {
        let mut probe = baseline();
        probe
            .audio(&audio(0, vec![0; 1600]), Duration::ZERO)
            .unwrap();
        let mut next = audio(1600, vec![1; 1600]);
        let timing = next.timing.as_mut().unwrap();
        match damage {
            0 => timing.native.continuity += 1,
            1 => timing.native.start_frame += 1,
            2 => timing.native.stream.stream_id = Uuid::from_u128(2),
            3 => timing.converted_start_frame += 1,
            4 => timing.resampled = true,
            _ => next.timing = None,
        }
        assert!(probe.audio(&next, Duration::from_millis(100)).is_err());
        assert_ne!(probe.phase, Phase::Done);
    }
    assert!(
        baseline()
            .audio(&audio(1, vec![0; 10]), Duration::ZERO)
            .is_err()
    );
    assert!(
        baseline()
            .check_audio_progress(Duration::from_secs(2))
            .is_err()
    );
}

#[test]
fn auxiliary_reference_loss_is_reported_without_hiding_received_pcm_gaps() {
    let mut probe = baseline();
    probe
        .audio(&audio(0, vec![0; 320]), Duration::ZERO)
        .unwrap();
    assert_eq!(probe.report()["native_reference_complete"], true);
    let loss = probe.reference_loss(1, 320).unwrap();
    assert_eq!(loss["type"], "native_reference_loss");
    assert_eq!(loss["lost_blocks"], 1);
    assert_eq!(loss["lost_frames"], 320);
    assert!(probe.reference_loss(1, 320).is_none());
    assert!(probe.reference_loss(0, 0).is_none());
    probe
        .audio(&audio(320, vec![1; 320]), Duration::from_millis(20))
        .unwrap();
    let report = probe.report();
    assert_eq!(report["total_samples"], 640);
    assert_eq!(report["native_reference_complete"], false);
    assert_eq!(report["native_reference_lost_blocks"], 1);
    assert_eq!(report["native_reference_lost_frames"], 320);
    // Missing a received block still fails, even after a known auxiliary loss.
    assert!(
        probe
            .audio(&audio(960, vec![1; 320]), Duration::from_millis(40))
            .is_err()
    );
    // A lost lifecycle marker is also incomplete evidence, even with zero lost frames.
    let loss = probe.reference_loss(2, 320).unwrap();
    assert_eq!(loss["lost_blocks"], 2);
    assert_eq!(loss["lost_frames"], 320);
}

#[test]
fn constant_dc_has_zero_ac_energy_and_clipping_is_visible() {
    let mut probe = baseline();
    probe
        .audio(&audio(0, vec![i16::MIN; 1600]), Duration::ZERO)
        .unwrap();
    let stats = probe.window.report();
    assert_eq!(stats["ac_rms"], 0.0);
    assert_eq!(stats["rms"], 32768.0);
    assert_eq!(stats["clipped_samples"], 1600);
}

fn args() -> Vec<String> {
    [
        "run",
        "/dev/serial/by-id/usb-test",
        "hw:CARD=L6Microphone,DEV=0",
        "0",
        "300",
        "/tmp/not-created-by-parsing",
        "cold boot declared by operator",
    ]
    .map(String::from)
    .to_vec()
}

#[test]
fn help_and_invalid_arguments_need_no_device_or_output_access() {
    assert!(parse(&[]).unwrap().is_none());
    assert!(parse(&["--help".into()]).unwrap().is_none());
    assert!(!parse(&args()).unwrap().unwrap().save_pcm);
    for (index, value) in [
        (0, "test"),
        (1, "/dev/ttyUSB0"),
        (1, "/dev/serial/by-id/../ttyUSB0"),
        (2, "default"),
        (2, "hw:CARD=1,DEV=0"),
        (3, "6"),
        (4, "29"),
        (4, "3601"),
        (6, ""),
    ] {
        let mut invalid = args();
        invalid[index] = value.into();
        assert!(parse(&invalid).is_err(), "accepted {invalid:?}");
    }
    let mut explicit = args();
    explicit.push("--save-pcm".into());
    assert!(parse(&explicit).unwrap().unwrap().save_pcm);
}

#[cfg(target_os = "linux")]
#[test]
fn pty_fragmented_handshake_and_version_use_shared_owner_and_cancelled_write_stops() {
    use std::io::{Read, Write};
    use tokio_util::sync::CancellationToken;
    let (mut board, mut host, _path) = super::serial::pty();
    let cancel = CancellationToken::new();
    let mut probe = Probe::new(0, Duration::from_secs(300), 100);
    host.write(&protocol::version_query(100), &cancel).unwrap();
    let mut bytes = [0u8; 1024];
    let n = board.read(&mut bytes).unwrap();
    assert_eq!(protocol::Decoder::default().push(&bytes[..n])[0].id, 100);
    let mut decoder = protocol::Decoder::default();
    let handshake = protocol::encode(1, 77, &[0xa5, 0, 0, 0]);
    for chunk in handshake.chunks(3) {
        board.write_all(chunk).unwrap();
        let n = host.read(&mut bytes).unwrap();
        for frame in decoder.push(&bytes[..n]) {
            host.write(
                &probe.receive(&frame, Duration::ZERO).unwrap().unwrap(),
                &cancel,
            )
            .unwrap();
        }
    }
    let n = board.read(&mut bytes).unwrap();
    let ack = decoder.push(&bytes[..n]).pop().unwrap();
    assert_eq!((ack.kind, ack.id), (0xff, 77));
    let version = protocol::encode(
        5,
        100,
        &serde_json::to_vec(&json!({"code":0,"content":FIRMWARE})).unwrap(),
    );
    for chunk in version.chunks(11) {
        board.write_all(chunk).unwrap();
        let n = host.read(&mut bytes).unwrap();
        for frame in decoder.push(&bytes[..n]) {
            probe.receive(&frame, Duration::ZERO).unwrap();
        }
    }
    assert_eq!(probe.phase, Phase::Baseline);
    cancel.cancel();
    assert!(host.write(&protocol::version_query(101), &cancel).is_err());
    assert_eq!(
        board.read(&mut bytes).unwrap_err().kind(),
        std::io::ErrorKind::WouldBlock
    );
}

#[cfg(target_os = "linux")]
#[test]
fn cancelled_and_existing_output_runs_stop_before_opening_devices() {
    let dir = tempfile::tempdir().unwrap();
    let mut options = parse(&args()).unwrap().unwrap();
    options.output = dir.path().join("cancelled");
    let cancel = tokio_util::sync::CancellationToken::new();
    cancel.cancel();
    assert!(super::linux::run(options, cancel).is_err());
    assert!(!dir.path().join("cancelled").exists());
    let mut options = parse(&args()).unwrap().unwrap();
    options.output = dir.path().to_owned();
    assert!(super::linux::run(options, tokio_util::sync::CancellationToken::new()).is_err());
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
}
