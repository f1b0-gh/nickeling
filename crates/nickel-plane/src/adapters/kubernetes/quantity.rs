//! Bounded semantic comparison for explicitly supported native resource quantities.
//!
//! Parsing follows apimachinery v0.34.0 resource/quantity.go and suffix.go:
//! decimal and binary suffixes, rounding away from zero to nano precision,
//! and the BinarySI magnitude cap. Unsupported sizes remain unequal instead of
//! approximating. This is a comparison helper, not a replacement API validator.

pub(super) fn equivalent(kind: &str, path: &str, desired: &str, actual: &str) -> bool {
    if !resource_field(kind, path) {
        return false;
    }
    matches!((parse(desired), parse(actual)), (Some(a), Some(b)) if a == b)
}

fn resource_field(kind: &str, path: &str) -> bool {
    if kind == "PersistentVolumeClaim" {
        return path.strip_prefix("/spec/").and_then(resource_name) == Some("storage");
    }
    if !matches!(kind, "Deployment" | "StatefulSet") {
        return false;
    }
    if kind == "StatefulSet" {
        if let Some(path) = indexed(path, "/spec/volumeClaimTemplates/") {
            return path.strip_prefix("spec/").and_then(resource_name) == Some("storage");
        }
    }
    let Some(path) = path.strip_prefix("/spec/template/spec/") else {
        return false;
    };
    if let Some(path) = indexed(path, "volumes/") {
        return path == "emptyDir/sizeLimit"
            || path
                .strip_prefix("ephemeral/volumeClaimTemplate/spec/")
                .and_then(resource_name)
                == Some("storage");
    }
    for collection in ["containers/", "initContainers/", "ephemeralContainers/"] {
        if let Some(path) = indexed(path, collection) {
            return matches!(resource_name(path), Some("cpu" | "memory"))
                || (collection != "ephemeralContainers/"
                    && resource_name(path) == Some("ephemeral-storage"));
        }
    }
    matches!(resource_name(path), Some("cpu" | "memory"))
}

fn indexed<'a>(path: &'a str, prefix: &str) -> Option<&'a str> {
    let (index, rest) = path.strip_prefix(prefix)?.split_once('/')?;
    (!index.is_empty() && index.bytes().all(|byte| byte.is_ascii_digit())).then_some(rest)
}

fn resource_name(path: &str) -> Option<&str> {
    let (bound, resource) = path.strip_prefix("resources/")?.split_once('/')?;
    matches!(bound, "requests" | "limits").then_some(resource)
}

/// Return sign and integer nano-units without floating point or heap allocation.
fn parse(text: &str) -> Option<(bool, u128)> {
    if text.is_empty() || text.len() > 128 {
        return None;
    }
    let negative = text.starts_with('-');
    let text = text.strip_prefix(['-', '+']).unwrap_or(text);
    let mut mantissa = 0u128;
    let mut fractional = 0i32;
    let mut decimal = false;
    let mut digits = 0usize;
    let mut end = 0usize;
    for byte in text.bytes() {
        if byte.is_ascii_digit() {
            mantissa = mantissa
                .checked_mul(10)?
                .checked_add(u128::from(byte - b'0'))?;
            digits += 1;
            fractional += i32::from(decimal);
        } else if byte == b'.' && !decimal {
            decimal = true;
        } else {
            break;
        }
        end += 1;
    }
    if digits == 0 {
        return None;
    }
    let (decimal_exponent, binary_exponent) = match &text[end..] {
        "" => (0, 0),
        "n" => (-9, 0),
        "u" => (-6, 0),
        "m" => (-3, 0),
        "k" => (3, 0),
        "M" => (6, 0),
        "G" => (9, 0),
        "T" => (12, 0),
        "P" => (15, 0),
        "E" => (18, 0),
        "Ki" => (0, 10),
        "Mi" => (0, 20),
        "Gi" => (0, 30),
        "Ti" => (0, 40),
        "Pi" => (0, 50),
        "Ei" => (0, 60),
        suffix => {
            let exponent: i32 = suffix.strip_prefix(['e', 'E'])?.parse().ok()?;
            if !(-128..=128).contains(&exponent) {
                return None;
            }
            (exponent, 0)
        }
    };
    if mantissa == 0 {
        return Some((false, 0));
    }
    mantissa = mantissa.checked_mul(1u128 << binary_exponent)?;
    let scale = decimal_exponent - fractional + 9;
    let mut nanos = if scale >= 0 {
        mantissa.checked_mul(10u128.checked_pow(scale as u32)?)?
    } else if let Some(divisor) = 10u128.checked_pow(scale.unsigned_abs()) {
        mantissa / divisor + u128::from(!mantissa.is_multiple_of(divisor))
    } else {
        // A nonzero numerator smaller than one nano still rounds away from zero.
        1
    };
    if binary_exponent != 0 {
        nanos = nanos.min(i64::MAX as u128 * 1_000_000_000);
    }
    Some((negative, nanos))
}

#[cfg(test)]
mod tests {
    use super::equivalent;

    const CPU: &str = "/spec/template/spec/containers/0/resources/limits/cpu";
    const MEMORY: &str = "/spec/template/spec/containers/0/resources/requests/memory";

    #[test]
    fn canonical_resource_spellings_do_not_create_drift() {
        for (desired, actual) in [
            ("1000m", "1"),
            ("1.5", "1500m"),
            (".5", "500m"),
            ("+001.000", "1"),
            ("1.", "1000m"),
            ("1e3", "1k"),
            ("1E+3", "1000"),
            ("1000e-3", "1"),
            ("1u", "1000n"),
            ("0.000001", "1u"),
            ("-0", "0n"),
            ("-1.5", "-1500m"),
        ] {
            assert!(
                equivalent("Deployment", CPU, desired, actual),
                "{desired} != {actual}"
            );
            assert!(
                equivalent("Deployment", CPU, actual, desired),
                "{actual} != {desired}"
            );
        }
        for (desired, actual) in [
            ("1024Mi", "1Gi"),
            ("1.5Gi", "1536Mi"),
            ("1Mi", "1048576"),
            (".5Ki", "512"),
            ("1.1Ki", "1126400m"),
            ("1G", "1000M"),
            ("1E", "1000000000000000000"),
            ("1Ei", "1152921504606846976"),
        ] {
            assert!(
                equivalent("StatefulSet", MEMORY, desired, actual),
                "{desired} != {actual}"
            );
        }
    }

    #[test]
    fn quantity_rounding_matches_nano_precision_and_binary_limits() {
        for (desired, actual) in [
            ("0.1n", "1n"),
            ("1.1n", "2n"),
            ("-0.1n", "-1n"),
            ("1e-100", "1n"),
            ("-1e-100", "-1n"),
            ("0.0000000001Ki", "103n"),
            ("8Ei", "9223372036854775807"),
            ("100Ei", "9223372036854775807"),
            ("-8Ei", "-9223372036854775807"),
        ] {
            assert!(
                equivalent("Deployment", MEMORY, desired, actual),
                "{desired} != {actual}"
            );
        }
    }

    #[test]
    fn exact_comparison_preserves_real_resource_changes() {
        for (desired, actual) in [
            ("1000m", "999m"),
            ("1", "1m"),
            ("1Gi", "1G"),
            ("1Mi", "1M"),
            ("9007199254740992", "9007199254740993"),
            ("0", "1n"),
            ("1n", "-1n"),
            ("9223372036854775808", "9223372036854775807"),
            ("1.000000001", "1"),
        ] {
            assert!(
                !equivalent("Deployment", MEMORY, desired, actual),
                "{desired} == {actual}"
            );
        }
    }

    #[test]
    fn comparison_is_scoped_to_native_cpu_and_memory_fields() {
        for kind in ["Deployment", "StatefulSet"] {
            for path in [
                CPU,
                MEMORY,
                "/spec/template/spec/initContainers/12/resources/requests/cpu",
                "/spec/template/spec/ephemeralContainers/1/resources/limits/memory",
                "/spec/template/spec/resources/limits/cpu",
                "/spec/template/spec/resources/requests/memory",
            ] {
                assert!(equivalent(kind, path, "1000m", "1"), "{kind}: {path}");
            }
        }
        for (kind, path) in [
            ("ConfigMap", "/data/cpu"),
            ("ConfigMap", CPU),
            ("Deployment", "/metadata/annotations/cpu"),
            ("Deployment", "/spec/template/spec/containers/0/env/0/value"),
            (
                "Deployment",
                "/spec/template/spec/containers/0/resources/limits/example.com/cpu",
            ),
            (
                "Deployment",
                "/spec/template/spec/containers/0/resources/limits/storage",
            ),
            (
                "Deployment",
                "/spec/template/spec/containers/0/resources/limits/cpu/extra",
            ),
            (
                "Deployment",
                "/spec/template/spec/containers//resources/limits/cpu",
            ),
            (
                "Deployment",
                "/spec/template/spec/containers/one/resources/limits/cpu",
            ),
            (
                "Deployment",
                "/spec/template/spec/other/0/resources/limits/cpu",
            ),
            ("Service", "/spec/ports/0/targetPort"),
            ("Pod", "/spec/containers/0/resources/limits/cpu"),
        ] {
            assert!(!equivalent(kind, path, "1000m", "1"), "{kind}: {path}");
        }
    }

    #[test]
    fn malformed_or_unbounded_quantities_never_gain_relaxed_equality() {
        for malformed in [
            "",
            "+",
            "-",
            ".",
            "1..0",
            " 1",
            "1 ",
            "1\n",
            "1K",
            "1ki",
            "1MI",
            "NaN",
            "Infinity",
            "1_000",
            "1e",
            "1e+",
            "1e1.5",
            "1e99999999999999999",
            "1e129",
            "1e-129",
            "1e100",
            "340282366920938463463374607431768211456",
            "1µ",
            "١",
            "1\0",
        ] {
            assert!(
                !equivalent("Deployment", CPU, malformed, malformed),
                "accepted {malformed:?}"
            );
            assert!(
                !equivalent("Deployment", CPU, malformed, "1"),
                "normalized {malformed:?}"
            );
        }
        let over_budget = "0".repeat(129);
        assert!(!equivalent("Deployment", CPU, &over_budget, "0"));
    }
    #[test]
    fn storage_equivalence_is_scoped_to_contract_backed_paths() {
        let mut cases = Vec::new();
        for bound in ["requests", "limits"] {
            cases.push((
                "PersistentVolumeClaim",
                format!("/spec/resources/{bound}/storage"),
            ));
            for index in ["0", "12"] {
                cases.push((
                    "StatefulSet",
                    format!("/spec/volumeClaimTemplates/{index}/spec/resources/{bound}/storage"),
                ));
                for kind in ["Deployment", "StatefulSet"] {
                    for container in ["containers", "initContainers"] {
                        cases.push((kind, format!("/spec/template/spec/{container}/{index}/resources/{bound}/ephemeral-storage")));
                    }
                    cases.push((kind, format!("/spec/template/spec/volumes/{index}/ephemeral/volumeClaimTemplate/spec/resources/{bound}/storage")));
                }
            }
        }
        for kind in ["Deployment", "StatefulSet"] {
            for index in ["0", "12"] {
                cases.push((
                    kind,
                    format!("/spec/template/spec/volumes/{index}/emptyDir/sizeLimit"),
                ));
            }
        }
        for (kind, path) in cases {
            for (desired, actual) in [("1024Mi", "1Gi"), ("1000M", "1G"), ("1073741824", "1Gi")] {
                assert!(
                    equivalent(kind, &path, desired, actual),
                    "{kind} {path}: {desired} != {actual}"
                );
                assert!(
                    equivalent(kind, &path, actual, desired),
                    "{kind} {path}: asymmetric equality"
                );
            }
            for (desired, actual) in [
                ("1Gi", "1G"),
                ("1Gi", "2Gi"),
                ("1073741824", "1073741825"),
                ("1Gi", "1 Gi"),
            ] {
                assert!(
                    !equivalent(kind, &path, desired, actual),
                    "{kind} {path}: lost real drift"
                );
            }
        }
    }

    #[test]
    fn storage_lookalikes_and_unsupported_paths_remain_exact() {
        for (kind, path) in [
            ("ConfigMap", "/data/storage"),
            ("ConfigMap", "/spec/resources/requests/storage"),
            ("PersistentVolume", "/spec/resources/requests/storage"),
            ("PersistentVolumeClaim", "/spec/resources/requests/cpu"),
            ("PersistentVolumeClaim", "/spec/resources/requests/memory"),
            ("PersistentVolumeClaim", "/spec/resources/requests/example.com/storage"),
            ("PersistentVolumeClaim", "/spec/resources/requests/storage/extra"),
            ("Deployment", "/spec/volumeClaimTemplates/0/spec/resources/requests/storage"),
            ("Deployment", "/spec/template/spec/resources/requests/ephemeral-storage"),
            ("Deployment", "/spec/template/spec/ephemeralContainers/0/resources/requests/ephemeral-storage"),
            ("Deployment", "/spec/template/spec/containers/0/resources/requests/storage"),
            ("Deployment", "/spec/template/spec/containers/0/resources/requests/example.com/ephemeral-storage"),
            ("Deployment", "/spec/template/spec/containers/-1/resources/requests/ephemeral-storage"),
            ("Deployment", "/spec/template/spec/containers//resources/requests/ephemeral-storage"),
            ("Deployment", "/spec/template/spec/containers/one/resources/requests/ephemeral-storage"),
            ("Deployment", "/spec/template/spec/volumes/one/emptyDir/sizeLimit"),
            ("Deployment", "/spec/template/spec/volumes/0/emptyDir/sizeLimit/extra"),
            ("Deployment", "/spec/template/spec/volumes/0/sizeLimit"),
            ("Deployment", "/spec/template/spec/volumes/0/ephemeral/volumeClaimTemplates/0/spec/resources/requests/storage"),
            ("Deployment", "/spec/template/spec/volumes/0/ephemeral/volumeClaimTemplate/resources/requests/storage"),
            ("StatefulSet", "/spec/volumeClaimTemplates//spec/resources/requests/storage"),
            ("StatefulSet", "/spec/volumeClaimTemplates/-1/spec/resources/requests/storage"),
            ("StatefulSet", "/spec/volumeClaimTemplates/one/spec/resources/requests/storage"),
            ("StatefulSet", "/spec/volumeClaimTemplates/0/spec/resources/requests/storage/extra"),
            ("StatefulSet", "/spec/volumeClaimTemplates/0/spec/resources/requests/cpu"),
            ("Pod", "/spec/volumes/0/emptyDir/sizeLimit"),
            ("Deployment", "/metadata/annotations/storage"),
            ("Deployment", "/spec/template/spec/containers/0/env/0/value"),
        ] {
            assert!(!equivalent(kind, path, "1024Mi", "1Gi"), "{kind}: {path}");
        }
    }
}
