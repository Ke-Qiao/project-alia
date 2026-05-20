use base64::Engine;

pub(super) fn direct_response_rule_from_json(
    value: &serde_json::Value,
) -> anyhow::Result<alia_proto::settings::DirectResponseRule> {
    let keywords = value
        .get("keywords")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| anyhow::anyhow!("direct response requires 'keywords' array"))?
        .iter()
        .map(|keyword| {
            keyword
                .as_str()
                .map(str::to_string)
                .ok_or_else(|| anyhow::anyhow!("direct response keywords must be strings"))
        })
        .collect::<anyhow::Result<Vec<_>>>()?;

    let audio_path = value
        .get("audio_path")
        .or_else(|| value.get("audio"))
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default()
        .to_string();
    let audio_variants = direct_response_audio_variants_from_json(value)?;

    if audio_path.trim().is_empty() && audio_variants.is_empty() {
        anyhow::bail!("direct response requires 'audio_path' or 'audio_variants'");
    }

    Ok(alia_proto::settings::DirectResponseRule {
        id: value
            .get("id")
            .and_then(serde_json::Value::as_i64)
            .unwrap_or(0),
        slug: value
            .get("slug")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_string(),
        keywords,
        audio_path,
        action: value
            .get("action")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_string(),
        enabled: value
            .get("enabled")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(true),
        audio_variants,
    })
}

pub(super) fn direct_response_rule_to_json(
    rule: alia_proto::settings::DirectResponseRule,
) -> serde_json::Value {
    let audio_variants = rule
        .audio_variants
        .into_iter()
        .map(|variant| {
            serde_json::json!({
                "audio_path": variant.audio_path,
                "weight": variant.weight,
            })
        })
        .collect::<Vec<_>>();

    serde_json::json!({
        "id": rule.id,
        "slug": rule.slug,
        "keywords": rule.keywords,
        "audio_path": rule.audio_path,
        "audio_variants": audio_variants,
        "action": if rule.action.is_empty() { serde_json::Value::Null } else { serde_json::Value::String(rule.action) },
        "enabled": rule.enabled,
    })
}

fn direct_response_audio_variants_from_json(
    value: &serde_json::Value,
) -> anyhow::Result<Vec<alia_proto::settings::DirectResponseAudioVariant>> {
    let Some(variants) = value
        .get("audio_variants")
        .or_else(|| value.get("audioVariants"))
        .and_then(serde_json::Value::as_array)
    else {
        return Ok(Vec::new());
    };

    variants
        .iter()
        .map(|variant| {
            let audio_path = variant
                .get("audio_path")
                .or_else(|| variant.get("audio"))
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .to_string();
            let audio_data = variant
                .get("audio_base64")
                .or_else(|| variant.get("audioDataBase64"))
                .and_then(serde_json::Value::as_str)
                .map(|payload| base64::engine::general_purpose::STANDARD.decode(payload))
                .transpose()?
                .unwrap_or_default();
            let weight = variant
                .get("weight")
                .and_then(serde_json::Value::as_f64)
                .unwrap_or(1.0) as f32;

            Ok(alia_proto::settings::DirectResponseAudioVariant {
                audio_path,
                weight,
                audio_data,
            })
        })
        .collect()
}
