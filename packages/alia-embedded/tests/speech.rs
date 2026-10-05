use alia_embedded::module::direct_response_playback::read_pcm16_wav_blocking;
use alia_embedded::module::speech::transport::derive_voice_endpoint;

#[test]
fn reads_pcm16_wav() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("test.wav");
    std::fs::write(&path, test_wav(&[1, -2], 16_000, 1)).unwrap();

    let wav = read_pcm16_wav_blocking(&path).unwrap();
    assert_eq!(wav.sample_rate, 16_000);
    assert_eq!(wav.source_channels, 1);
    assert_eq!(wav.pcm, vec![1, -2]);
}

#[test]
fn cue_wav_downmixes_stereo_by_mean_without_clipping() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("stereo.wav");
    std::fs::write(&path, test_wav(&[1000, -1000, 30_000, 10_000], 16_000, 2)).unwrap();
    let wav = read_pcm16_wav_blocking(&path).unwrap();
    assert_eq!(wav.source_channels, 2);
    assert_eq!(wav.pcm, vec![0, 20_000]);
}

#[test]
fn cue_wav_rejects_invalid_format_and_unbounded_duration() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("invalid.wav");
    for (pcm, rate, channels) in [
        (vec![1], 16_000, 0),
        (vec![1], 0, 1),
        (vec![1, 2, 3], 16_000, 2),
        (vec![0; 16_000 * 5 + 1], 16_000, 1),
    ] {
        std::fs::write(&path, test_wav(&pcm, rate, channels)).unwrap();
        assert!(read_pcm16_wav_blocking(&path).is_err());
    }
}

#[test]
fn cue_wav_rejects_oversized_file_before_decoding() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("oversized.wav");
    let file = std::fs::File::create(&path).unwrap();
    file.set_len(4 * 1024 * 1024 + 1).unwrap();
    assert!(read_pcm16_wav_blocking(&path).is_err());
}

#[test]
fn derives_voice_endpoint_from_bound_server_endpoint() {
    let endpoint = derive_voice_endpoint("http://192.168.1.10:3000").unwrap();
    assert_eq!(endpoint, "http://192.168.1.10:3001");
}

fn test_wav(pcm: &[i16], sample_rate: u32, channels: u16) -> Vec<u8> {
    let pcm = pcm
        .iter()
        .flat_map(|sample| sample.to_le_bytes())
        .collect::<Vec<_>>();
    let mut wav = Vec::new();
    let data_len = pcm.len() as u32;
    let byte_rate = sample_rate * u32::from(channels) * 2;
    let block_align = channels * 2;
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&(36 + data_len).to_le_bytes());
    wav.extend_from_slice(b"WAVE");
    wav.extend_from_slice(b"fmt ");
    wav.extend_from_slice(&16u32.to_le_bytes());
    wav.extend_from_slice(&1u16.to_le_bytes());
    wav.extend_from_slice(&channels.to_le_bytes());
    wav.extend_from_slice(&sample_rate.to_le_bytes());
    wav.extend_from_slice(&byte_rate.to_le_bytes());
    wav.extend_from_slice(&block_align.to_le_bytes());
    wav.extend_from_slice(&16u16.to_le_bytes());
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&data_len.to_le_bytes());
    wav.extend_from_slice(&pcm);
    wav
}
