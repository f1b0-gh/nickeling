//! Offline conversion of the provider-schema JSON format into closed Nickel contracts.
use nickel_util::error::{Error, Result};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::{BTreeMap, BTreeSet};

pub const MAX_INPUT_BYTES: usize = 16 * 1024 * 1024;
pub const MAX_DEPTH: usize = 64;
// The confined evaluator counts the generated and provider-independent libraries
// together. Reserve the three shipped helper modules before accepting a bundle.
pub const MAX_FILES: usize = 2048 - 3;
pub const MAX_FILE_BYTES: usize = 1024 * 1024;
pub const MAX_GENERATED_BYTES: usize = 64 * 1024 * 1024
    - include_bytes!("../nickel/contracts.ncl").len()
    - include_bytes!("../nickel/expressions.ncl").len()
    - include_bytes!("../nickel/composition.ncl").len();
const CORE: &str = "let Core = import \"/libraries/nickel-tf/contracts.ncl\" in\n";
const BUILTIN: &str = "terraform.io/builtin/terraform";

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Engine {
    pub name: String,
    pub version: String,
}
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ProviderPin {
    pub source: String,
    pub version: Option<String>,
}
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Selection {
    pub engine: Engine,
    pub providers: BTreeMap<String, ProviderPin>,
}

fn invalid(message: impl Into<String>) -> Error {
    Error::InvalidInput {
        message: message.into(),
    }
}

fn identifier(name: &str) -> bool {
    let mut chars = name.chars();
    matches!(chars.next(), Some(c) if c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

fn check_identifier(name: &str, context: &str) -> Result<()> {
    if !identifier(name) || name.len() > 128 {
        return Err(invalid(format!(
            "{context}: unsupported identifier {name:?}"
        )));
    }
    Ok(())
}

fn exact_version(version: &str) -> bool {
    let (base, build) = version
        .split_once('+')
        .map_or((version, None), |(a, b)| (a, Some(b)));
    let (base, prerelease) = base
        .split_once('-')
        .map_or((base, None), |(a, b)| (a, Some(b)));
    let components: Vec<_> = base.split('.').collect();
    components.len() == 3
        && components.iter().all(|s| {
            !s.is_empty()
                && s.bytes().all(|b| b.is_ascii_digit())
                && (s.len() == 1 || !s.starts_with('0'))
        })
        && [build, prerelease].into_iter().flatten().all(|s| {
            !s.is_empty()
                && s.split('.').all(|p| {
                    !p.is_empty() && p.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
                })
        })
        && prerelease.is_none_or(|s| {
            s.split('.').all(|p| {
                !p.bytes().all(|b| b.is_ascii_digit()) || p.len() == 1 || !p.starts_with('0')
            })
        })
}

fn provider_part(part: &str) -> bool {
    !part.is_empty()
        && !part.starts_with('-')
        && !part.ends_with('-')
        && !part.contains("--")
        && part
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

fn source_valid(source: &str) -> bool {
    let parts: Vec<_> = source.split('/').collect();
    if parts.len() != 3 {
        return false;
    }
    let host = parts[0];
    host.contains('.')
        && host.split('.').all(|p| {
            !p.is_empty()
                && !p.starts_with('-')
                && !p.ends_with('-')
                && p.bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        })
        && parts[1..].iter().all(|p| provider_part(p))
}

/// Validate explicit provenance independently of a provider schema export.
pub fn validate_selection(selection: &Selection) -> Result<()> {
    if selection.engine.name != "opentofu" || !exact_version(&selection.engine.version) {
        return Err(invalid(
            "engine selection requires opentofu with an exact semantic version",
        ));
    }
    let mut sources = BTreeSet::new();
    for (name, pin) in &selection.providers {
        if !provider_part(name)
            || !name.starts_with(|c: char| c.is_ascii_lowercase())
            || name.len() > 128
        {
            return Err(invalid(format!("unsupported provider local name {name:?}; use normalized ASCII letters, digits and single dashes, starting with a letter for provider references")));
        }
        if !source_valid(&pin.source) || !sources.insert(&pin.source) {
            return Err(invalid(format!(
                "provider {name}: source must be canonical, fully-qualified and unambiguous"
            )));
        }
        if pin.source == BUILTIN {
            if pin.version.is_some() {
                return Err(invalid("built-in terraform provider version must be null; its version is the selected engine version"));
            }
        } else if !pin.version.as_deref().is_some_and(exact_version) {
            return Err(invalid(format!(
                "provider {name}: an exact provider release version is required"
            )));
        }
    }
    Ok(())
}

fn object<'a>(value: &'a Value, path: &str) -> Result<&'a Map<String, Value>> {
    value
        .as_object()
        .ok_or_else(|| invalid(format!("{path}: expected an object")))
}

fn optional_map<'a>(
    map: &'a Map<String, Value>,
    key: &str,
    path: &str,
) -> Result<Option<&'a Map<String, Value>>> {
    map.get(key)
        .map(|v| object(v, &format!("{path}.{key}")))
        .transpose()
}

fn string<'a>(map: &'a Map<String, Value>, key: &str, path: &str) -> Result<&'a str> {
    map.get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| invalid(format!("{path}.{key}: expected a string")))
}

fn flag(map: &Map<String, Value>, key: &str, path: &str) -> Result<bool> {
    match map.get(key) {
        None => Ok(false),
        Some(Value::Bool(value)) => Ok(*value),
        Some(_) => Err(invalid(format!("{path}.{key}: expected a boolean"))),
    }
}

fn count(map: &Map<String, Value>, key: &str, path: &str) -> Result<u64> {
    match map.get(key) {
        None => Ok(0),
        Some(value) => value
            .as_u64()
            .ok_or_else(|| invalid(format!("{path}.{key}: expected a nonnegative integer"))),
    }
}

fn quoted(text: &str) -> Result<String> {
    let mut result = String::from("\"");
    for character in text.chars() {
        match character {
            '"' => result.push_str("\\\""),
            '\\' => result.push_str("\\\\"),
            '\n' => result.push_str("\\n"),
            '\r' => result.push_str("\\r"),
            '\t' => result.push_str("\\t"),
            value if value.is_control() => result.push_str(&format!("\\u{{{:x}}}", value as u32)),
            value => result.push(value),
        }
    }
    result.push('"');
    Ok(result.replace("%{", "\\%{"))
}

fn field(name: &str, contract: &str, optional: bool) -> Result<String> {
    Ok(format!(
        "{} | {}{},\n",
        quoted(name)?,
        contract,
        if optional { " | optional" } else { "" }
    ))
}

fn record(fields: impl IntoIterator<Item = String>) -> String {
    format!("{{\n{}}}", fields.into_iter().collect::<String>())
}

fn wrap_value(contract: &str, nullable: bool) -> String {
    format!(
        "Core.Value (Core.{} ({contract}))",
        if nullable { "Nullable" } else { "NonNull" }
    )
}

fn type_contract(value: &Value, path: &str, nullable: bool) -> Result<String> {
    let base = match value {
        Value::String(name) => match name.as_str() {
            "string" => "String".into(),
            "number" => "Number".into(),
            "bool" => "Bool".into(),
            "dynamic" => "Dyn".into(),
            _ => return Err(invalid(format!("{path}: unsupported type {name:?}"))),
        },
        Value::Array(parts) if parts.len() == 2 => match parts[0].as_str() {
            Some("list" | "set") => format!("Array ({})", type_contract(&parts[1], path, true)?),
            Some("map") => format!("{{ _ | {} }}", type_contract(&parts[1], path, true)?),
            Some("tuple") => {
                let types = parts[1]
                    .as_array()
                    .ok_or_else(|| invalid(format!("{path}: tuple requires an array of types")))?;
                let mut contracts = Vec::new();
                for (index, item) in types.iter().enumerate() {
                    contracts.push(type_contract(item, &format!("{path}[{index}]"), true)?);
                }
                format!("Core.Tuple [{}]", contracts.join(", "))
            }
            Some("object") => {
                let mut fields = Vec::new();
                for (name, ty) in object(&parts[1], path)? {
                    fields.push(field(
                        name,
                        &type_contract(ty, &format!("{path}.{name}"), true)?,
                        false,
                    )?);
                }
                record(fields)
            }
            _ => return Err(invalid(format!("{path}: unsupported collection type"))),
        },
        _ => return Err(invalid(format!("{path}: unsupported type shape"))),
    };
    Ok(wrap_value(&base, nullable))
}

fn attribute_fields(attributes: &Map<String, Value>, path: &str) -> Result<Vec<String>> {
    let mut fields = Vec::new();
    for (name, value) in attributes {
        check_identifier(name, path)?;
        let path = format!("{path}.{name}");
        let attribute = object(value, &path)?;
        let required = flag(attribute, "required", &path)?;
        let optional = flag(attribute, "optional", &path)?;
        let computed = flag(attribute, "computed", &path)?;
        if (required && (optional || computed)) || !(required || optional || computed) {
            return Err(invalid(format!(
                "{path}: invalid required/optional/computed flags"
            )));
        }
        let contract = if let Some(nested) = attribute.get("nested_type") {
            let nested = object(nested, &path)?;
            let attributes = nested
                .get("attributes")
                .ok_or_else(|| invalid(format!("{path}.nested_type: missing attributes")))?;
            let item = record(attribute_fields(object(attributes, &path)?, &path)?);
            let mode = string(nested, "nesting_mode", &path)?;
            let base = match mode {
                "single" => item,
                "list" | "set" => format!("Array (Core.Value (Core.Nullable ({item})))"),
                "map" => format!("{{ _ | Core.Value (Core.Nullable ({item})) }}"),
                _ => {
                    return Err(invalid(format!(
                        "{path}: unsupported attribute nesting mode {mode:?}"
                    )))
                }
            };
            wrap_value(&base, !required)
        } else {
            type_contract(
                attribute
                    .get("type")
                    .ok_or_else(|| invalid(format!("{path}: missing type or nested_type")))?,
                &path,
                !required,
            )?
        };
        // Omitting computed-only fields from a closed record forbids configuration.
        if required || optional {
            fields.push(field(name, &contract, !required)?);
        }
    }
    Ok(fields)
}

fn block_contract(block: &Value, path: &str) -> Result<String> {
    let block = object(block, path)?;
    let attributes = optional_map(block, "attributes", path)?;
    let mut fields = match attributes {
        Some(attrs) => attribute_fields(attrs, path)?,
        None => Vec::new(),
    };
    if let Some(blocks) = optional_map(block, "block_types", path)? {
        for (name, value) in blocks {
            check_identifier(name, path)?;
            let path = format!("{path}.{name}");
            if attributes.is_some_and(|attrs| attrs.contains_key(name)) {
                return Err(invalid(format!(
                    "{path}: attribute and block names collide"
                )));
            }
            let nested = object(value, &path)?;
            let mode = string(nested, "nesting_mode", &path)?;
            let min = count(nested, "min_items", &path)?;
            let max = count(nested, "max_items", &path)?;
            if max > 0 && max < min {
                return Err(invalid(format!(
                    "{path}: max_items is smaller than min_items"
                )));
            }
            let child = nested
                .get("block")
                .ok_or_else(|| invalid(format!("{path}: missing nested block")))?;
            let contract = block_contract(child, &path)?;
            let contract = match mode {
                "single" | "group" if min <= 1 && max <= 1 => contract,
                "list" | "set" => format!("Core.Bounds {min} {max} (Array ({contract}))"),
                "map" if min == 0 && max == 0 => format!("Core.BlockMap ({contract})"),
                _ => {
                    return Err(invalid(format!(
                        "{path}: unsupported block nesting mode or bounds {mode:?}"
                    )))
                }
            };
            fields.push(field(name, &contract, min == 0)?);
        }
    }
    Ok(record(fields))
}

fn schema_contract(value: &Value, path: &str, provider: bool) -> Result<String> {
    let schema = object(value, path)?;
    if schema.get("version").and_then(Value::as_u64).is_none() {
        return Err(invalid(format!(
            "{path}.version: missing nonnegative schema version"
        )));
    }
    match schema.get("block") {
        Some(block) => {
            let fields = object(block, path)?;
            let reserved: &[&str] = if provider {
                &["alias", "version"]
            } else {
                &[
                    "count",
                    "for_each",
                    "depends_on",
                    "provider",
                    "lifecycle",
                    "connection",
                    "provisioner",
                    "dynamic",
                ]
            };
            for kind in ["attributes", "block_types"] {
                if let Some(fields) = optional_map(fields, kind, path)? {
                    if let Some(name) = reserved.iter().find(|name| fields.contains_key(**name)) {
                        return Err(invalid(format!("{path}.{name}: schema field collides with a reserved core meta-argument")));
                    }
                }
            }
            block_contract(block, path)
        }
        None if provider => Ok("{}".into()),
        None => Err(invalid(format!("{path}: missing block"))),
    }
}

fn check_depth(value: &Value, depth: usize) -> Result<()> {
    if depth > MAX_DEPTH {
        return Err(invalid("provider schema exceeds maximum depth 64"));
    }
    match value {
        Value::Array(values) => {
            for value in values {
                check_depth(value, depth + 1)?;
            }
        }
        Value::Object(values) => {
            for value in values.values() {
                check_depth(value, depth + 1)?;
            }
        }
        _ => {}
    }
    Ok(())
}

#[derive(Default)]
struct Modules {
    files: BTreeMap<String, String>,
    bytes: usize,
}
impl Modules {
    fn insert(&mut self, name: String, body: String) -> Result<()> {
        let body = format!("{CORE}{body}\n");
        if body.len() > MAX_FILE_BYTES
            || self.files.len() >= MAX_FILES
            || self.bytes + body.len() > MAX_GENERATED_BYTES
        {
            return Err(invalid(
                "generated contracts exceed file count or byte limits",
            ));
        }
        self.bytes += body.len();
        self.files.insert(name, body);
        Ok(())
    }
}

/// Convert a frozen schema export using explicit provider-release provenance.
/// Unknown metadata fields are ignored for forward-compatible format minors.
pub fn generate(schema: &Value, selection: &Selection) -> Result<BTreeMap<String, String>> {
    validate_selection(selection)?;
    check_depth(schema, 0)?;
    if serde_json::to_vec(schema)?.len() > MAX_INPUT_BYTES {
        return Err(invalid("provider schema exceeds 16 MiB input limit"));
    }
    let root = object(schema, "schema")?;
    let version = string(root, "format_version", "schema")?;
    let version_parts: Vec<_> = version.split('.').collect();
    if version_parts.len() != 2
        || version_parts[0] != "1"
        || version_parts[1].is_empty()
        || !version_parts[1].bytes().all(|b| b.is_ascii_digit())
    {
        return Err(invalid(format!(
            "unsupported provider schema format version {version:?}"
        )));
    }
    let providers = root
        .get("provider_schemas")
        .ok_or_else(|| invalid("schema.provider_schemas is required"))?;
    let providers = object(providers, "schema.provider_schemas")?;
    let selected: BTreeSet<_> = selection
        .providers
        .values()
        .map(|pin| pin.source.as_str())
        .collect();
    if selected != providers.keys().map(String::as_str).collect() {
        return Err(invalid(
            "provider schema sources must exactly match selected provider sources",
        ));
    }
    let mut modules = Modules::default();
    let mut provider_fields = Vec::new();
    let mut resource_fields = BTreeMap::new();
    let mut data_fields = BTreeMap::new();
    for (local, pin) in &selection.providers {
        let provider = object(&providers[&pin.source], &pin.source)?;
        if let Some(schema) = provider.get("provider") {
            let file = format!("providers/{local}.ncl");
            modules.insert(file.clone(), schema_contract(schema, &pin.source, true)?)?;
            provider_fields.push(field(
                local,
                &format!("Core.ProviderConfigs (import {})", quoted(&file)?),
                true,
            )?);
        }
        for (key, prefix, wrapper, fields) in [
            (
                "resource_schemas",
                "resources",
                "Resource",
                &mut resource_fields,
            ),
            ("data_source_schemas", "data", "Data", &mut data_fields),
        ] {
            if let Some(schemas) = optional_map(provider, key, &pin.source)? {
                for (name, schema) in schemas {
                    check_identifier(name, key)?;
                    let file = format!("{prefix}/{local}/{name}.ncl");
                    modules.insert(
                        file.clone(),
                        format!(
                            "Core.{wrapper}For {} {} ({})",
                            quoted(local)?,
                            quoted(name.split('_').next().unwrap_or(name))?,
                            schema_contract(
                                schema,
                                &format!("{}.{key}.{name}", pin.source),
                                false
                            )?
                        ),
                    )?;
                    let contract = format!("{{ _ | (import {}) }}", quoted(&file)?);
                    if fields
                        .insert(name.clone(), field(name, &contract, true)?)
                        .is_some()
                    {
                        return Err(invalid(format!(
                            "ambiguous {key} name {name:?} from multiple providers"
                        )));
                    }
                }
            }
        }
    }
    let root = record([
        field("provider", &record(provider_fields), true)?,
        field("resource", &record(resource_fields.into_values()), true)?,
        field("data", &record(data_fields.into_values()), true)?,
        field("variable", "{ _ | Core.Variable }", true)?,
        field("locals", "{ _ | Core.Value Dyn }", true)?,
        field("output", "{ _ | Core.Output }", true)?,
        field("terraform", "Core.Terraform", true)?,
    ]);
    modules.insert("schema.ncl".into(), root)?;
    Ok(modules.files)
}
