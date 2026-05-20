pub(super) fn set_volume_request_from_json(
    value: &serde_json::Value,
) -> anyhow::Result<alia_proto::settings::SetVolumeRequest> {
    let capture = value
        .get("capture_volume")
        .or_else(|| value.get("mic_volume"))
        .and_then(serde_json::Value::as_f64)
        .map(|value| value as f32);
    let playback = value
        .get("playback_volume")
        .or_else(|| value.get("speaker_volume"))
        .and_then(serde_json::Value::as_f64)
        .map(|value| value as f32);

    if capture.is_none() && playback.is_none() {
        anyhow::bail!("set_volume requires 'capture_volume' or 'playback_volume'");
    }

    Ok(alia_proto::settings::SetVolumeRequest {
        capture_volume: capture.unwrap_or_default(),
        set_capture_volume: capture.is_some(),
        playback_volume: playback.unwrap_or_default(),
        set_playback_volume: playback.is_some(),
    })
}

pub(super) fn volume_settings_to_json(
    volume: alia_proto::settings::VolumeSettings,
) -> serde_json::Value {
    serde_json::json!({
        "capture_volume": volume.capture_volume,
        "playback_volume": volume.playback_volume,
        "min_volume": volume.min_volume,
        "max_volume": volume.max_volume,
    })
}

pub(super) fn required_action_str<'a>(
    value: &'a serde_json::Value,
    key: &str,
) -> anyhow::Result<&'a str> {
    value
        .get(key)
        .and_then(serde_json::Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| anyhow::anyhow!("embedded action requires string '{key}'"))
}
