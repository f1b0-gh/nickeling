use crate::{require, Error, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "lowercase")]
pub enum BackendKind {
    Kubernetes,
    OpenTofu,
}
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Management {
    #[default]
    Observe,
    Manage,
}
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Deletion {
    #[default]
    Retain,
    Delete,
}
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Gate {
    Applied,
    #[default]
    Ready,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields, default)]
pub struct Policy {
    pub mode: Management,
    pub deletion: Deletion,
    pub allow_delete: bool,
    pub allow_replace: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CompositionRef {
    pub name: String,
    pub version: String,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Dependency {
    pub unit: String,
    #[serde(default)]
    pub condition: Gate,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Binding {
    pub from_unit: String,
    pub output: String,
    pub to: String,
    #[serde(default)]
    pub condition: Gate,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OutputSpec {
    pub pointer: String,
    #[serde(default)]
    pub sensitive: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Unit {
    pub backend: BackendKind,
    pub target: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schema: Option<String>,
    pub configuration: Value,
    #[serde(default)]
    pub depends_on: Vec<Dependency>,
    #[serde(default)]
    pub bindings: Vec<Binding>,
    #[serde(default)]
    pub outputs: BTreeMap<String, OutputSpec>,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Instance {
    pub api_version: String,
    pub id: String,
    pub composition: CompositionRef,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    #[serde(default)]
    pub parameters: Value,
    #[serde(default)]
    pub policy: Policy,
    pub units: BTreeMap<String, Unit>,
}
pub fn validate_id(value: &str) -> Result<()> {
    require(
        !value.is_empty()
            && value.len() <= 63
            && value.as_bytes()[0].is_ascii_lowercase()
            && value
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'_'),
        "IDs must be lower-case ASCII names starting with a letter, at most 63 characters",
    )
}
pub fn pointer_parts(pointer: &str) -> Result<Vec<String>> {
    require(
        pointer.len() <= 4096 && (pointer.is_empty() || pointer.starts_with('/')),
        "Invalid JSON pointer",
    )?;
    if pointer.is_empty() {
        return Ok(vec![]);
    }
    pointer[1..]
        .split('/')
        .map(|part| {
            let mut chars = part.chars();
            let mut result = String::new();
            while let Some(c) = chars.next() {
                if c == '~' {
                    result.push(match chars.next() {
                        Some('0') => '~',
                        Some('1') => '/',
                        _ => return Err(Error::Invalid("Invalid JSON pointer escape".into())),
                    })
                } else {
                    result.push(c)
                }
            }
            Ok(result)
        })
        .collect()
}
impl Instance {
    pub fn validate(&self, targets: &BTreeSet<String>) -> Result<Vec<String>> {
        require(
            self.api_version == "nickel-plane/v1",
            "Unsupported instance API version",
        )?;
        validate_id(&self.id)?;
        validate_id(&self.composition.name)?;
        require(
            !self.composition.version.is_empty() && self.composition.version.len() <= 128,
            "Composition version required",
        )?;
        require(
            self.display_name
                .as_ref()
                .is_none_or(|name| name.len() <= 256),
            "Display name too long",
        )?;
        require(
            !self.units.is_empty() && self.units.len() <= 64,
            "Composition must contain 1..64 named units",
        )?;
        require(
            self.policy.deletion != Deletion::Delete
                || (self.policy.mode == Management::Manage && self.policy.allow_delete),
            "Deletion requires managed mode and explicit allowDelete",
        )?;
        let mut dependencies = BTreeMap::<String, BTreeSet<String>>::new();
        for (id, unit) in &self.units {
            validate_id(id)?;
            validate_id(&unit.target)?;
            require(
                targets.contains(&unit.target),
                format!("Unknown target {}", unit.target),
            )?;
            require(
                unit.configuration.is_object(),
                "Unit configuration must be an object",
            )?;
            match (unit.backend, &unit.schema) {
                (BackendKind::OpenTofu, Some(schema)) => validate_id(schema)?,
                (BackendKind::Kubernetes, None) => (),
                _ => {
                    return Err(Error::Invalid(
                        "Only OpenTofu units require a schema bundle name".into(),
                    ))
                }
            }
            require(
                unit.bindings.len() <= 64
                    && unit.depends_on.len() <= 64
                    && unit.outputs.len() <= 64,
                "Too many unit dependencies or outputs",
            )?;
            let mut edges = BTreeSet::new();
            for dependency in &unit.depends_on {
                require(
                    edges.insert(dependency.unit.clone()),
                    "Duplicate dependency",
                )?;
            }
            let mut paths = Vec::<Vec<String>>::new();
            for binding in &unit.bindings {
                let parts = pointer_parts(&binding.to)?;
                require(
                    !parts.is_empty()
                        && parts.len() <= 64
                        && unit.configuration.pointer(&binding.to).is_some(),
                    format!("Binding target must exist: {}", binding.to),
                )?;
                require(
                    !paths
                        .iter()
                        .any(|path| path.starts_with(&parts) || parts.starts_with(path)),
                    "Overlapping binding targets",
                )?;
                paths.push(parts);
                let source = self
                    .units
                    .get(&binding.from_unit)
                    .ok_or_else(|| Error::Invalid("Binding source unit does not exist".into()))?;
                require(
                    source.outputs.contains_key(&binding.output),
                    format!("Undeclared output {}.{}", binding.from_unit, binding.output),
                )?;
                edges.insert(binding.from_unit.clone());
            }
            for dependency in &edges {
                require(
                    dependency != id && self.units.contains_key(dependency),
                    "Unknown or self dependency",
                )?;
            }
            for (name, output) in &unit.outputs {
                validate_id(name)?;
                require(
                    pointer_parts(&output.pointer)?.len() <= 64,
                    "Output pointer too deep",
                )?;
            }
            dependencies.insert(id.clone(), edges);
        }
        let mut result = Vec::new();
        let mut done = BTreeSet::new();
        while !dependencies.is_empty() {
            let next = dependencies
                .iter()
                .find(|(_, deps)| deps.is_subset(&done))
                .map(|(id, _)| id.clone())
                .ok_or_else(|| Error::Invalid("Dependency cycle in composition".into()))?;
            dependencies.remove(&next);
            done.insert(next.clone());
            result.push(next);
        }
        Ok(result)
    }
}
