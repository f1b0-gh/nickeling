//! Context-aware OpenTofu JSON rendering.

use crate::schema::{validate_selection, Selection};
use nickel_util::error::{Error, Result};
use serde_json::{Map, Value};

fn invalid(message: impl Into<String>) -> Error {
    Error::InvalidInput {
        message: message.into(),
    }
}

fn object<'a>(value: &'a Value, path: &str) -> Result<&'a Map<String, Value>> {
    value
        .as_object()
        .ok_or_else(|| invalid(format!("{path} must be an object")))
}

fn allowed(map: &Map<String, Value>, fields: &[&str], path: &str) -> Result<()> {
    for key in map.keys() {
        if !fields.contains(&key.as_str()) {
            return Err(invalid(format!("unsupported field {path}.{key}")));
        }
    }
    Ok(())
}

fn depth(value: &Value, level: usize) -> Result<()> {
    if level > 64 {
        return Err(invalid("configuration exceeds maximum depth 64"));
    }
    match value {
        Value::Array(values) => values.iter().try_for_each(|value| depth(value, level + 1)),
        Value::Object(values) => values
            .values()
            .try_for_each(|value| depth(value, level + 1)),
        _ => Ok(()),
    }
}

/// Render values only where OpenTofu expects expressions. Static strings and
/// block labels deliberately bypass template escaping.
fn value(input: &Value, templates: bool, markers: bool, path: &str) -> Result<Value> {
    if markers {
        if let Some(map) = input.as_object() {
            if let Some(marker) = map.get("__nickel_tf") {
                if map.len() != 1 {
                    return Err(invalid(format!(
                        "{path}: reserved marker must be the only field; use tf.literal for data"
                    )));
                }
                let marker = object(marker, path)?;
                return match marker.get("kind").and_then(Value::as_str) {
                    Some("expression") => {
                        allowed(marker, &["kind", "source"], path)?;
                        let source = marker
                            .get("source")
                            .and_then(Value::as_str)
                            .filter(|s| !s.trim().is_empty() && !s.contains('\0'))
                            .ok_or_else(|| {
                                invalid(format!(
                                    "{path}: expression source must be a nonempty string"
                                ))
                            })?;
                        if !templates {
                            return Err(invalid(format!("{path}: deferred expressions are not supported in a static argument")));
                        }
                        Ok(Value::String(format!("${{{source}}}")))
                    }
                    Some("literal") => {
                        allowed(marker, &["kind", "value"], path)?;
                        let literal = marker.get("value").ok_or_else(|| {
                            invalid(format!("{path}: literal marker requires value"))
                        })?;
                        value(literal, templates, false, path)
                    }
                    _ => Err(invalid(format!(
                        "{path}: invalid reserved marker kind; use tf.literal for data"
                    ))),
                };
            }
        }
    }
    match input {
        Value::String(text) if templates => Ok(Value::String(
            text.replace("${", "$${").replace("%{", "%%{"),
        )),
        Value::Array(values) => values
            .iter()
            .enumerate()
            .map(|(i, item)| value(item, templates, markers, &format!("{path}[{i}]")))
            .collect::<Result<Vec<_>>>()
            .map(Value::Array),
        Value::Object(values) => values
            .iter()
            .map(|(key, item)| {
                let rendered_key = if templates {
                    key.replace("${", "$${").replace("%{", "%%{")
                } else {
                    key.clone()
                };
                value(item, templates, markers, &format!("{path}.{key}")).map(|v| (rendered_key, v))
            })
            .collect::<Result<Map<_, _>>>()
            .map(Value::Object),
        _ => Ok(input.clone()),
    }
}

fn static_value(input: &Value, path: &str) -> Result<Value> {
    value(input, false, true, path)
}

fn static_string(input: &Value, path: &str) -> Result<Value> {
    let result = static_value(input, path)?;
    if !result.is_string() {
        return Err(invalid(format!("{path} must be a literal string")));
    }
    Ok(result)
}

fn static_bool(input: &Value, path: &str) -> Result<Value> {
    let result = static_value(input, path)?;
    if !result.is_boolean() {
        return Err(invalid(format!("{path} must be a literal boolean")));
    }
    Ok(result)
}

fn references(input: &Value, path: &str) -> Result<Value> {
    let result = static_value(input, path)?;
    if !result
        .as_array()
        .is_some_and(|items| items.iter().all(Value::is_string))
    {
        return Err(invalid(format!(
            "{path} must be an array of static reference strings"
        )));
    }
    Ok(result)
}

fn lifecycle(input: &Value, path: &str) -> Result<Value> {
    let fields = object(input, path)?;
    allowed(
        fields,
        &[
            "create_before_destroy",
            "prevent_destroy",
            "ignore_changes",
            "replace_triggered_by",
        ],
        path,
    )?;
    fields
        .iter()
        .map(|(key, input)| {
            let at = format!("{path}.{key}");
            let result = match key.as_str() {
                "create_before_destroy" | "prevent_destroy" => static_bool(input, &at),
                "ignore_changes" if input.as_str() == Some("all") => Ok(input.clone()),
                _ => references(input, &at),
            }?;
            Ok((key.clone(), result))
        })
        .collect::<Result<Map<_, _>>>()
        .map(Value::Object)
}

fn resource_body(input: &Value, is_data: bool, path: &str) -> Result<Value> {
    let fields = object(input, path)?;
    if fields.contains_key("count") && fields.contains_key("for_each") {
        return Err(invalid(format!(
            "{path}: count and for_each are mutually exclusive"
        )));
    }
    fields
        .iter()
        .map(|(key, input)| {
            let at = format!("{path}.{key}");
            let result = match key.as_str() {
                "provider" => static_string(input, &at)?,
                "depends_on" => references(input, &at)?,
                "lifecycle" if !is_data => lifecycle(input, &at)?,
                "provisioner" | "connection" | "dynamic" | "lifecycle" => {
                    return Err(invalid(format!("unsupported field {at}")))
                }
                _ => value(input, true, true, &at)?,
            };
            Ok((key.clone(), result))
        })
        .collect::<Result<Map<_, _>>>()
        .map(Value::Object)
}

fn named_blocks(
    input: &Value,
    path: &str,
    body: impl Fn(&Value, &str) -> Result<Value>,
) -> Result<Value> {
    object(input, path)?
        .iter()
        .map(|(name, input)| body(input, &format!("{path}.{name}")).map(|v| (name.clone(), v)))
        .collect::<Result<Map<_, _>>>()
        .map(Value::Object)
}

fn variable(input: &Value, path: &str) -> Result<Value> {
    let fields = object(input, path)?;
    allowed(
        fields,
        &["type", "default", "description", "sensitive", "nullable"],
        path,
    )?;
    fields
        .iter()
        .map(|(key, input)| {
            let at = format!("{path}.{key}");
            let result = match key.as_str() {
                "type" | "description" => static_string(input, &at),
                "sensitive" | "nullable" => static_bool(input, &at),
                _ => static_value(input, &at),
            }?;
            Ok((key.clone(), result))
        })
        .collect::<Result<Map<_, _>>>()
        .map(Value::Object)
}

fn output(input: &Value, path: &str) -> Result<Value> {
    let fields = object(input, path)?;
    allowed(
        fields,
        &["value", "description", "sensitive", "depends_on"],
        path,
    )?;
    if !fields.contains_key("value") {
        return Err(invalid(format!("{path}.value is required")));
    }
    fields
        .iter()
        .map(|(key, input)| {
            let at = format!("{path}.{key}");
            let result = match key.as_str() {
                "description" => static_string(input, &at),
                "sensitive" => static_bool(input, &at),
                "depends_on" => references(input, &at),
                _ => value(input, true, true, &at),
            }?;
            Ok((key.clone(), result))
        })
        .collect::<Result<Map<_, _>>>()
        .map(Value::Object)
}

fn provider(input: &Value, path: &str) -> Result<Value> {
    let render = |input: &Value, path: &str| -> Result<Value> {
        object(input, path)?
            .iter()
            .map(|(key, input)| {
                let at = format!("{path}.{key}");
                let result = match key.as_str() {
                    "alias" => static_string(input, &at)?,
                    "version" => {
                        return Err(invalid(format!(
                            "{at}: configure provider versions in selection only"
                        )))
                    }
                    _ => value(input, true, true, &at)?,
                };
                Ok((key.clone(), result))
            })
            .collect::<Result<Map<_, _>>>()
            .map(Value::Object)
    };
    match input {
        Value::Array(items) => items
            .iter()
            .enumerate()
            .map(|(i, item)| render(item, &format!("{path}[{i}]")))
            .collect::<Result<Vec<_>>>()
            .map(Value::Array),
        _ => render(input, path),
    }
}

fn exact_version(input: &Value, expected: &str, path: &str) -> Result<()> {
    if !input
        .as_str()
        .is_some_and(|s| s == expected || s == format!("= {expected}"))
    {
        return Err(invalid(format!(
            "{path} conflicts with selected exact version {expected}"
        )));
    }
    Ok(())
}

fn terraform(input: Option<&Value>, selection: &Selection) -> Result<Value> {
    let empty = Map::new();
    let fields = match input {
        Some(input) => object(input, "terraform")?,
        None => &empty,
    };
    allowed(
        fields,
        &["required_version", "required_providers"],
        "terraform",
    )?;
    if let Some(version) = fields.get("required_version") {
        exact_version(
            version,
            &selection.engine.version,
            "terraform.required_version",
        )?;
    }
    let pins = match fields.get("required_providers") {
        Some(pins) => object(pins, "terraform.required_providers")?,
        None => &empty,
    };
    for name in pins.keys() {
        if !selection.providers.contains_key(name) {
            return Err(invalid(format!(
                "terraform.required_providers.{name} is not in selection"
            )));
        }
    }
    let mut required = Map::new();
    for (name, pin) in &selection.providers {
        let path = format!("terraform.required_providers.{name}");
        if let Some(user) = pins.get(name) {
            let user = object(user, &path)?;
            allowed(user, &["source", "version"], &path)?;
            if let Some(source) = user.get("source") {
                if source.as_str() != Some(&pin.source) {
                    return Err(invalid(format!("{path}.source conflicts with selection")));
                }
            }
            if let Some(version) = user.get("version") {
                match &pin.version {
                    Some(expected) => exact_version(version, expected, &format!("{path}.version"))?,
                    None => {
                        return Err(invalid(format!(
                            "{path}: built-in provider cannot have a version constraint"
                        )))
                    }
                }
            }
        }
        let mut requirement = Map::new();
        requirement.insert("source".into(), Value::String(pin.source.clone()));
        if let Some(version) = &pin.version {
            requirement.insert("version".into(), Value::String(format!("= {version}")));
        }
        required.insert(name.clone(), Value::Object(requirement));
    }
    Ok(
        serde_json::json!({"required_version": format!("= {}", selection.engine.version), "required_providers": required}),
    )
}

/// Convert validated Nickel data into OpenTofu's context-sensitive JSON syntax.
/// This performs no engine/plugin invocation. Provider attribute contracts are
/// applied by the trusted generated Nickel schema before this renderer runs.
pub fn render_config(config: Value, selection: &Selection) -> Result<Value> {
    validate_selection(selection)?;
    depth(&config, 0)?;
    let fields = object(&config, "configuration")?;
    allowed(
        fields,
        &[
            "resource",
            "data",
            "provider",
            "variable",
            "locals",
            "output",
            "terraform",
        ],
        "configuration",
    )?;
    let mut result = Map::new();
    for (key, input) in fields {
        let rendered = match key.as_str() {
            "resource" | "data" => named_blocks(input, key, |input, path| {
                named_blocks(input, path, |input, path| {
                    resource_body(input, key == "data", path)
                })
            })?,
            "provider" => named_blocks(input, key, provider)?,
            "variable" => named_blocks(input, key, variable)?,
            "output" => named_blocks(input, key, output)?,
            "locals" => named_blocks(input, key, |input, path| value(input, true, true, path))?,
            "terraform" => continue,
            _ => return Err(invalid("unsupported top-level block")),
        };
        result.insert(key.clone(), rendered);
    }
    result.insert(
        "terraform".into(),
        terraform(fields.get("terraform"), selection)?,
    );
    Ok(Value::Object(result))
}
