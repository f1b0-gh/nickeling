use nickel_util::process::Stream;
use std::collections::BTreeSet;

#[derive(Clone, Copy, Debug)]
pub(super) enum Phase {
    Version,
    Init,
    Show,
    Plan,
    Apply,
}
impl Phase {
    pub(super) fn name(self) -> &'static str {
        match self {
            Self::Version => "version",
            Self::Init => "init",
            Self::Show => "show",
            Self::Plan => "plan",
            Self::Apply => "apply",
        }
    }
    pub(super) fn ui(self) -> bool {
        matches!(self, Self::Init | Self::Plan | Self::Apply)
    }
}
#[derive(Debug)]
pub(super) enum Cause {
    Spawn,
    Io,
    MissingPipe,
    Exit(i32),
    Signal(i32),
    Timeout(u64),
    OutputLimit(Stream, usize),
    InvalidJson,
}
impl std::fmt::Display for Cause {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Spawn => f.write_str("could not start process"),
            Self::Io => f.write_str("process I/O failure"),
            Self::MissingPipe => f.write_str("missing process pipe"),
            Self::Exit(code) => write!(f, "exit {code}"),
            Self::Signal(signal) => write!(f, "signal {signal}"),
            Self::Timeout(seconds) => write!(f, "timeout after {seconds}s; process group stopped"),
            Self::OutputLimit(stream, bytes) => write!(
                f,
                "{} exceeds {bytes}-byte capture limit; process group stopped",
                if *stream == Stream::Stdout {
                    "stdout"
                } else {
                    "stderr"
                }
            ),
            Self::InvalidJson => f.write_str("invalid JSON response"),
        }
    }
}
pub(super) enum Retention {
    Disabled,
    Saved(String),
    Unavailable(RetentionReason),
}
impl std::fmt::Debug for Retention {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Disabled => f.write_str("Disabled"),
            Self::Saved(_) => f.write_str("Saved(<private reference>)"),
            Self::Unavailable(reason) => f.debug_tuple("Unavailable").field(reason).finish(),
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum RetentionReason {
    Quota,
    UnsafePath,
    Busy,
    Io,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Category {
    InvalidInput,
    MissingInput,
    InvalidDefault,
    Ephemeral,
}
impl Category {
    fn text(self) -> &'static str {
        match self {
            Self::InvalidInput => {
                "invalid-input: Check input variable types and protected variable settings."
            }
            Self::MissingInput => {
                "missing-input: Check required input variables and protected variable settings."
            }
            Self::InvalidDefault => {
                "invalid-configuration: Check the generated configuration and variable defaults."
            }
            Self::Ephemeral => {
                "invalid-configuration: Check the variable's ephemeral-value declaration."
            }
        }
    }
}
#[derive(Debug, Default)]
pub(super) struct Categories {
    values: BTreeSet<Category>,
    pub(super) incomplete: bool,
    unavailable: bool,
}
pub(super) fn summarize_ui(stdout: &[u8], engine: &str) -> Categories {
    let mut result = Categories::default();
    let mut version = false;
    let mut offset = 0usize;
    let mut diagnostics = 0usize;
    for index in 0..256 {
        if offset == stdout.len() {
            break;
        }
        let end = stdout
            .len()
            .min(4 * 1024 * 1024)
            .min(offset.saturating_add(256 * 1024 + 1));
        if offset >= end {
            result.incomplete = true;
            break;
        }
        let window = &stdout[offset..end];
        let Some(length) = window.iter().position(|b| *b == b'\n') else {
            result.incomplete = true;
            break;
        };
        let line = &window[..length];
        offset += length + 1;
        let Ok(value) = nickel_util::json::parse_with_limits(line, 256 * 1024, 64) else {
            result.incomplete = true;
            break;
        };
        if !version {
            if index != 0
                || value["type"] != "version"
                || value["ui"] != "1.2"
                || value["tofu"] != engine
            {
                result.unavailable = true;
                break;
            }
            version = true;
            continue;
        }
        if value["type"] != "diagnostic" {
            continue;
        }
        diagnostics += 1;
        if diagnostics > 64 {
            result.incomplete = true;
            break;
        }
        if value["diagnostic"]["severity"] != "error" {
            continue;
        }
        let category = match value["diagnostic"]["summary"].as_str() {
            Some("Invalid value for input variable") => Some(Category::InvalidInput),
            Some("Required variable not set") => Some(Category::MissingInput),
            Some("Invalid default value for module argument") => Some(Category::InvalidDefault),
            Some("Variable does not allow ephemeral value") => Some(Category::Ephemeral),
            _ => None,
        };
        if let Some(category) = category {
            if result.values.len() >= 8 {
                result.incomplete = true;
                break;
            }
            result.values.insert(category);
        }
    }
    if offset < stdout.len() {
        result.incomplete = true;
    }
    if !version {
        result.unavailable = true;
    }
    result
}
pub(super) fn artifact_id(value: &str) -> bool {
    value
        .strip_prefix("failure-")
        .and_then(|s| s.strip_suffix(".log"))
        .is_some_and(|s| s.len() == 12 && s.bytes().all(|b| b.is_ascii_alphanumeric()))
}
pub(super) fn format_failure(
    phase: Phase,
    cause: &Cause,
    categories: &Categories,
    artifact: &Retention,
) -> String {
    let mut text = format!("OpenTofu {} failed ({cause}).", phase.name());
    for category in &categories.values {
        text.push(' ');
        text.push_str(category.text());
    }
    if categories.values.is_empty() {
        text.push_str(" Provider diagnostics withheld.");
    }
    if categories.unavailable {
        text.push_str(" diagnostics-unavailable.");
    }
    if categories.incomplete {
        text.push_str(" diagnostics-incomplete.");
    }
    match artifact {
        Retention::Saved(id) if artifact_id(id) => {
            text.push_str(" Private diagnostics: ");
            text.push_str(id);
            text.push_str(" (potentially sensitive).");
        }
        Retention::Disabled => {}
        Retention::Unavailable(RetentionReason::Quota) => {
            text.push_str(" Private diagnostics unavailable (quota full).")
        }
        _ => text.push_str(" Private diagnostics unavailable."),
    }
    text.truncate(text.len().min(2048));
    text
}
#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use serde_json::json;
    fn stream(summary: &str) -> Vec<u8> {
        format!("{}\n{}\n",json!({"type":"version","ui":"1.2","tofu":"1.13.2"}),json!({"type":"diagnostic","diagnostic":{"severity":"error","summary":summary,"detail":"secret-detail","snippet":{"code":"secret-code"},"range":{"filename":"secret-path"}},"@message":"secret-message"})).into_bytes()
    }
    #[test]
    fn categories_are_exact_and_never_echo_provider_fields() {
        for (summary, category) in [
            ("Invalid value for input variable", "invalid-input"),
            ("Required variable not set", "missing-input"),
            (
                "Invalid default value for module argument",
                "invalid-configuration",
            ),
            (
                "Variable does not allow ephemeral value",
                "invalid-configuration",
            ),
        ] {
            let categories = summarize_ui(&stream(summary), "1.13.2");
            let text = format_failure(
                Phase::Plan,
                &Cause::Exit(1),
                &categories,
                &Retention::Disabled,
            );
            assert!(text.contains(category), "{text}");
            assert!(!text.contains("secret"));
            assert!(!format!("{categories:?}").contains("secret"));
        }
        let text = format_failure(
            Phase::Plan,
            &Cause::Exit(1),
            &summarize_ui(
                &stream("Invalid value for input variable secret-suffix"),
                "1.13.2",
            ),
            &Retention::Disabled,
        );
        assert!(!text.contains("invalid-input"));
        assert!(!text.contains("secret"));
    }
    #[test]
    fn malformed_versions_and_parser_budgets_are_safe() {
        for bytes in [
            b"secret-not-json".to_vec(),
            vec![255],
            stream("Required variable not set")
                .into_iter()
                .map(|b| if b == b'2' { b'9' } else { b })
                .collect(),
            vec![b'x'; 256 * 1024 + 1],
            b"{\"type\":\"version\",\"type\":\"secret\"}".to_vec(),
        ] {
            let text = format_failure(
                Phase::Init,
                &Cause::Exit(1),
                &summarize_ui(&bytes, "1.13.2"),
                &Retention::Disabled,
            );
            assert!(text.contains("diagnostics-"));
            assert!(!text.contains("secret"));
            assert!(text.len() <= 2048);
        }
        let mut data = stream("Required variable not set");
        for _ in 0..65 {
            data.extend_from_slice(b"{\"type\":\"diagnostic\",\"diagnostic\":{\"severity\":\"error\",\"summary\":\"secret\"}}\n");
        }
        assert!(format_failure(
            Phase::Plan,
            &Cause::Exit(1),
            &summarize_ui(&data, "1.13.2"),
            &Retention::Disabled
        )
        .contains("diagnostics-incomplete"));
        let mut data = stream("Required variable not set");
        data.extend_from_slice(&vec![b'\n'; 257]);
        assert!(summarize_ui(&data, "1.13.2").incomplete);
    }
    #[test]
    fn incomplete_final_record_and_invalid_artifact_debug_are_withheld() {
        let mut bytes = stream("Required variable not set");
        bytes.pop();
        let categories = summarize_ui(&bytes, "1.13.2");
        assert!(categories.incomplete);
        assert!(!format_failure(
            Phase::Plan,
            &Cause::Exit(1),
            &categories,
            &Retention::Disabled
        )
        .contains("missing-input"));
        assert!(
            !format!("{:?}", Retention::Saved("arbitrary-private-id".into()))
                .contains("arbitrary-private-id")
        );
    }
    #[test]
    fn target_opt_in_defaults_false_and_does_not_change_identity() -> crate::Result<()> {
        use crate::{adapters::opentofu::OpenTofu, backend::Backend, targets::OpenTofuTarget};
        let value = json!({"binary":std::env::current_exe()?,"stack":"test"});
        let a: OpenTofuTarget = serde_json::from_value(value.clone())?;
        assert!(!a.private_diagnostics);
        let mut b = a.clone();
        b.private_diagnostics = true;
        let a = OpenTofu::new(a)?;
        let b = OpenTofu::new(b)?;
        assert_eq!(a.fingerprint()?, b.fingerprint()?);
        assert_eq!(a.state_binding()?, b.state_binding()?);
        for bad in [
            json!({"privateDiagnostics":"true"}),
            json!({"unknown":true}),
        ] {
            let mut v = value.clone();
            v.as_object_mut()
                .unwrap()
                .extend(bad.as_object().unwrap().clone());
            assert!(serde_json::from_value::<OpenTofuTarget>(v).is_err());
        }
        Ok(())
    }
}
