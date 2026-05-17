use std::collections::BTreeMap;

use serde_json::Value;

pub fn string_setting(updates: &BTreeMap<String, Value>, key: &str) -> Option<String> {
    updates.get(key)?.as_str().map(ToOwned::to_owned)
}

pub fn optional_string_setting(
    updates: &BTreeMap<String, Value>,
    key: &str,
) -> Option<Option<String>> {
    let value = updates.get(key)?;
    if value.is_null() {
        return Some(None);
    }
    value.as_str().map(str::trim).map(|value| {
        if value.is_empty() {
            None
        } else {
            Some(value.to_owned())
        }
    })
}

pub fn string_vec_setting(updates: &BTreeMap<String, Value>, key: &str) -> Option<Vec<String>> {
    let value = updates.get(key)?;
    value.as_array().map(|items| {
        items
            .iter()
            .filter_map(|item| item.as_str())
            .map(str::trim)
            .filter(|item| !item.is_empty())
            .map(ToOwned::to_owned)
            .collect()
    })
}

pub fn bool_setting(updates: &BTreeMap<String, Value>, key: &str) -> Option<bool> {
    updates.get(key)?.as_bool()
}

pub fn u32_setting(updates: &BTreeMap<String, Value>, key: &str) -> Option<u32> {
    updates
        .get(key)?
        .as_u64()
        .and_then(|value| u32::try_from(value).ok())
}

pub fn u64_setting(updates: &BTreeMap<String, Value>, key: &str) -> Option<u64> {
    updates.get(key)?.as_u64()
}

pub fn usize_setting(updates: &BTreeMap<String, Value>, key: &str) -> Option<usize> {
    updates
        .get(key)?
        .as_u64()
        .and_then(|value| usize::try_from(value).ok())
}

pub fn i32_setting(updates: &BTreeMap<String, Value>, key: &str) -> Option<i32> {
    updates
        .get(key)?
        .as_i64()
        .and_then(|value| i32::try_from(value).ok())
}

pub fn f32_setting(updates: &BTreeMap<String, Value>, key: &str) -> Option<f32> {
    updates.get(key)?.as_f64().map(|value| value as f32)
}
