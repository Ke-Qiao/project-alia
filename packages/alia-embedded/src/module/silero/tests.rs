use super::*;

fn start(endpoint: &mut Endpoint) {
    for _ in 0..32 {
        if endpoint.accept(0.9) == VoiceActivityState::Speech {
            return;
        }
    }
    panic!("strong speech must start within the configured minimum");
}

#[test]
fn weak_continuous_speech_preserves_hysteresis_without_a_duration_limit() {
    let mut endpoint = Endpoint::new(&VadConfig::default());
    start(&mut endpoint);
    for _ in 0..16_000 * 3_600 / WINDOW {
        assert_eq!(endpoint.accept(0.4), VoiceActivityState::Speech);
        assert_eq!(endpoint.onset, Some(endpoint.min_speech_samples));
        assert_eq!(endpoint.silence, None);
    }
    for _ in 0..22 {
        assert_eq!(endpoint.accept(0.0), VoiceActivityState::Speech);
    }
    assert_eq!(endpoint.accept(0.0), VoiceActivityState::Finished);
    assert_eq!(endpoint.accept(0.0), VoiceActivityState::Silence);
    assert_eq!(endpoint.onset, None);
    assert_eq!(endpoint.silence, None);
}

#[test]
fn short_sounds_do_not_start_and_a_short_pause_does_not_finish() {
    let mut endpoint = Endpoint::new(&VadConfig::default());
    for _ in 0..8 {
        assert_eq!(endpoint.accept(0.9), VoiceActivityState::Silence);
    }
    assert_eq!(endpoint.accept(0.0), VoiceActivityState::Silence);
    start(&mut endpoint);
    for _ in 0..10 {
        assert_eq!(endpoint.accept(0.0), VoiceActivityState::Speech);
    }
    assert_eq!(endpoint.accept(0.9), VoiceActivityState::Speech);
    assert_eq!(endpoint.silence, None);
    endpoint.reset();
    assert_eq!(endpoint.accept(0.4), VoiceActivityState::Silence);
    start(&mut endpoint);
}

#[test]
fn low_threshold_and_equality_cannot_turn_active_speech_into_silence() {
    for threshold in [0.005, 0.01, 0.5] {
        let mut endpoint = Endpoint::new(&VadConfig {
            threshold,
            ..VadConfig::default()
        });
        start(&mut endpoint);
        assert_eq!(endpoint.accept(threshold), VoiceActivityState::Speech);
        assert_eq!(
            endpoint.accept(threshold + 0.002),
            VoiceActivityState::Speech
        );
    }
}

#[test]
fn model_signature_rejects_wrong_names_shapes_and_element_types() {
    use ort::value::{Shape, SymbolicDimensions, ValueType};
    let outlet = |name: &str, shape: &[i64], ty| {
        Outlet::new(
            name,
            ValueType::Tensor {
                ty,
                shape: Shape::new(shape.iter().copied()),
                dimension_symbols: SymbolicDimensions::empty(shape.len()),
            },
        )
    };
    let expected = [("x", &[1, 512][..])];
    assert!(
        validate_signature(
            &[outlet("x", &[1, 512], TensorElementType::Float32)],
            &expected
        )
        .is_ok()
    );
    for bad in [
        outlet("input", &[1, 512], TensorElementType::Float32),
        outlet("x", &[1, 256], TensorElementType::Float32),
        outlet("x", &[-1, 512], TensorElementType::Float32),
        outlet("x", &[1, 512], TensorElementType::Int16),
    ] {
        assert!(validate_signature(&[bad], &expected).is_err());
    }
}

#[test]
#[ignore = "requires ALIA_TEST_VAD_MODEL and ALIA_TEST_VAD_WAV absolute paths; local public fixtures, no devices"]
fn real_silero_streaming_matches_native_and_resets_bounded_state() {
    let model_path = std::path::PathBuf::from(
        std::env::var_os("ALIA_TEST_VAD_MODEL").expect("set ALIA_TEST_VAD_MODEL"),
    );
    let wav_path = std::path::PathBuf::from(
        std::env::var_os("ALIA_TEST_VAD_WAV").expect("set ALIA_TEST_VAD_WAV"),
    );
    assert!(model_path.is_absolute() && wav_path.is_absolute());
    let wave = sherpa_onnx::Wave::read(wav_path.to_str().unwrap()).unwrap();
    assert_eq!(wave.sample_rate(), SAMPLE_RATE as i32);
    let pcm = wave
        .samples()
        .iter()
        .map(|sample| (sample * 32_768.0).round().clamp(-32_768.0, 32_767.0) as i16)
        .collect::<Vec<_>>();
    let config = VadConfig {
        model_path,
        ..VadConfig::default()
    };
    let vad = SileroVad::new(&config).unwrap();
    let chunked = SileroVad::new(&config).unwrap();
    // The comparison oracle keeps its original thresholds; only the removed
    // native PCM-retention strategy used the maximum-duration threshold change.
    let native = sherpa_onnx::VoiceActivityDetector::create(
        &sherpa_onnx::VadModelConfig {
            silero_vad: sherpa_onnx::SileroVadModelConfig {
                model: Some(config.model_path.display().to_string()),
                threshold: config.threshold,
                min_silence_duration: config.min_silence_duration_ms as f32 / 1_000.0,
                min_speech_duration: config.min_speech_duration_ms as f32 / 1_000.0,
                window_size: WINDOW as i32,
                max_speech_duration: 3_600.0,
            },
            sample_rate: SAMPLE_RATE as i32,
            num_threads: 1,
            provider: Some("cpu".into()),
            ..Default::default()
        },
        30.0,
    )
    .unwrap();
    let mut longest = 0;
    let mut active = 0;
    let mut ends = 0;
    let mut first_speech = None;
    let mut speech_after_end = None;
    let began = std::time::Instant::now();
    for (index, window) in pcm.chunks_exact(WINDOW).enumerate() {
        let samples = window
            .iter()
            .map(|sample| f32::from(*sample) / 32_768.0)
            .collect::<Vec<_>>();
        native.accept_waveform(&samples);
        let expected = if native.detected() {
            VoiceActivityState::Speech
        } else if !native.is_empty() {
            VoiceActivityState::Finished
        } else {
            VoiceActivityState::Silence
        };
        native.clear();
        assert_eq!(
            vad.accept(window, SAMPLE_RATE).unwrap(),
            expected,
            "window {index}"
        );
        chunked.accept(&window[..17], SAMPLE_RATE).unwrap();
        chunked.accept(&window[17..117], SAMPLE_RATE).unwrap();
        assert_eq!(
            chunked.accept(&window[117..], SAMPLE_RATE).unwrap(),
            expected,
            "chunked window {index}"
        );
        if expected == VoiceActivityState::Speech {
            first_speech.get_or_insert(index);
            if ends > 0 {
                speech_after_end.get_or_insert(index);
            }
            active += WINDOW;
            longest = longest.max(active);
        } else {
            active = 0;
        }
        if expected == VoiceActivityState::Finished {
            ends += 1;
        }
        let state = vad.state.lock().unwrap();
        assert_eq!(state.pending, 0);
        assert_eq!(state.window.len(), WINDOW);
        assert_eq!(state.h.len() + state.c.len(), 256);
    }
    assert!(ends > 0);
    assert!(
        longest >= 20 * SAMPLE_RATE as usize,
        "fixture must include continuous speech crossing the former 15 s limit"
    );
    let first_speech = first_speech.expect("fixture contains speech");
    vad.accept(&pcm[..13], SAMPLE_RATE).unwrap();
    vad.reset().unwrap();
    {
        let state = vad.state.lock().unwrap();
        assert_eq!(state.pending, 0);
        assert!(
            state
                .h
                .iter()
                .chain(&state.c)
                .chain(&state.window)
                .all(|x| *x == 0.0)
        );
        assert_eq!(state.latest, VoiceActivityState::Silence);
    }
    let restarted = pcm
        .chunks_exact(WINDOW)
        .position(|window| vad.accept(window, SAMPLE_RATE).unwrap() == VoiceActivityState::Speech)
        .unwrap();
    assert_eq!(restarted, first_speech);
    vad.reset().unwrap();
    assert_eq!(
        vad.accept(
            &pcm[..(speech_after_end.unwrap() + 1) * WINDOW],
            SAMPLE_RATE
        )
        .unwrap(),
        VoiceActivityState::Speech,
        "a prior endpoint inside a large accept must not hide its final active window"
    );
    eprintln!(
        "silero_streaming_native_match windows={} max_active_samples={longest} endpoints={ends} reset_speech_window={first_speech} elapsed_ms={}",
        pcm.len() / WINDOW,
        began.elapsed().as_millis()
    );
}
