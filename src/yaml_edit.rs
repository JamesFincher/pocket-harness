use std::fs;
use std::path::Path;

use anyhow::{Context, Result, anyhow};
use serde_yaml::{Mapping, Number, Value};

use crate::config_store::{atomic_write, parse_and_validate};

pub fn set_value(config_path: &Path, dotted_path: &str, raw_value: &str) -> Result<()> {
    let text = fs::read_to_string(config_path)
        .with_context(|| format!("read config {}", config_path.display()))?;
    let mut value: Value = serde_yaml::from_str(&text).context("parse yaml before edit")?;
    let path = parse_path(dotted_path)?;
    set_nested_value(&mut value, &path, parse_scalar(raw_value))?;

    let new_text = serde_yaml::to_string(&value).context("serialize edited yaml")?;
    parse_and_validate(&new_text).context("edited config did not validate")?;
    atomic_write(config_path, &new_text)?;
    Ok(())
}

fn parse_path(dotted_path: &str) -> Result<Vec<String>> {
    let path = dotted_path
        .split('.')
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .map(str::to_string)
        .collect::<Vec<_>>();

    if path.is_empty() {
        Err(anyhow!("config path cannot be empty"))
    } else {
        Ok(path)
    }
}

fn set_nested_value(value: &mut Value, path: &[String], new_value: Value) -> Result<()> {
    if path.is_empty() {
        *value = new_value;
        return Ok(());
    }

    let mut cursor = value;
    for key in &path[..path.len() - 1] {
        if !matches!(cursor, Value::Mapping(_)) {
            *cursor = Value::Mapping(Mapping::new());
        }

        let mapping = cursor
            .as_mapping_mut()
            .ok_or_else(|| anyhow!("expected yaml mapping"))?;
        cursor = mapping
            .entry(Value::String(key.clone()))
            .or_insert_with(|| Value::Mapping(Mapping::new()));
    }

    let last = path.last().expect("path checked non-empty");
    if !matches!(cursor, Value::Mapping(_)) {
        *cursor = Value::Mapping(Mapping::new());
    }

    let mapping = cursor
        .as_mapping_mut()
        .ok_or_else(|| anyhow!("expected yaml mapping"))?;
    mapping.insert(Value::String(last.clone()), new_value);
    Ok(())
}

fn parse_scalar(raw: &str) -> Value {
    let trimmed = raw.trim();

    match trimmed {
        "true" => return Value::Bool(true),
        "false" => return Value::Bool(false),
        "null" | "~" => return Value::Null,
        _ => {}
    }

    if let Ok(value) = trimmed.parse::<i64>() {
        return Value::Number(Number::from(value));
    }

    if let Ok(value) = trimmed.parse::<f64>() {
        if let Ok(parsed) = serde_yaml::from_str::<Value>(trimmed) {
            return parsed;
        }
        return Value::String(value.to_string());
    }

    let unquoted = trimmed
        .strip_prefix('"')
        .and_then(|value| value.strip_suffix('"'))
        .or_else(|| {
            trimmed
                .strip_prefix('\'')
                .and_then(|value| value.strip_suffix('\''))
        })
        .unwrap_or(trimmed);

    Value::String(unquoted.to_string())
}

#[cfg(test)]
mod tests {
    use super::parse_scalar;
    use serde_yaml::Value;

    #[test]
    fn parses_scalar_types() {
        assert_eq!(parse_scalar("true"), Value::Bool(true));
        assert_eq!(parse_scalar("42"), serde_yaml::to_value(42).unwrap());
        assert_eq!(parse_scalar("hello"), Value::String("hello".to_string()));
    }
}
