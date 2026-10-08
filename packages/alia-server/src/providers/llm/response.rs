//! Parse structured model replies without relaxing each domain response schema.

use serde::de::DeserializeOwned;

pub(crate) fn parse_llm_json_object<T>(raw: &str, label: &str) -> anyhow::Result<T>
where
    T: DeserializeOwned,
{
    let candidate = strip_code_fences(raw);
    if let Ok(parsed) = serde_json::from_str::<T>(candidate) {
        return Ok(parsed);
    }

    let mut last_error = None;
    for json in json_object_slices(candidate) {
        match serde_json::from_str::<T>(json) {
            Ok(parsed) => return Ok(parsed),
            Err(error) => last_error = Some(error),
        }
    }

    if let Some(error) = last_error {
        anyhow::bail!("failed to parse {label} JSON: {error}");
    }
    anyhow::bail!("{label} response did not contain a JSON object")
}

fn strip_code_fences(s: &str) -> &str {
    let trimmed = s.trim();
    if let Some(rest) = trimmed.strip_prefix("```") {
        let rest = if let Some(nl) = rest.find('\n') {
            &rest[nl + 1..]
        } else {
            rest
        };
        rest.strip_suffix("```").unwrap_or(rest).trim()
    } else {
        trimmed
    }
}

fn json_object_slices(s: &str) -> Vec<&str> {
    let mut objects = Vec::new();
    let mut start = None;
    let mut depth = 0_i32;
    let mut in_string = false;
    let mut escaped = false;

    for (index, ch) in s.char_indices() {
        if in_string {
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                in_string = false;
            }
            continue;
        }

        match ch {
            '"' if depth > 0 => in_string = true,
            '{' => {
                if depth == 0 {
                    start = Some(index);
                }
                depth += 1;
            }
            '}' if depth > 0 => {
                depth -= 1;
                if depth == 0
                    && let Some(start_index) = start.take()
                {
                    objects.push(&s[start_index..index + ch.len_utf8()]);
                }
            }
            _ => {}
        }
    }

    objects
}

pub(crate) fn join_model_labels(labels: &[String]) -> String {
    let mut output = Vec::new();
    for label in labels.iter().flat_map(|label| label.split('+')) {
        let label = label.trim();
        if !label.is_empty()
            && label != "none"
            && !output.iter().any(|existing: &&str| *existing == label)
        {
            output.push(label);
        }
    }
    if output.is_empty() {
        "none".to_string()
    } else {
        output.join("+")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn composed_model_labels_retain_each_actual_provider_once() {
        assert_eq!(
            join_model_labels(&["memory+reviewer".into(), "reviewer".into(), "none".into(),]),
            "memory+reviewer"
        );
        assert_eq!(join_model_labels(&["none".into(), "".into()]), "none");
    }
    #[test]
    fn json_object_slices_handles_braces_inside_strings() {
        let objects = json_object_slices(r#"prefix {"value":"literal { brace }"} suffix"#);

        assert_eq!(objects, vec![r#"{"value":"literal { brace }"}"#]);
    }
}
