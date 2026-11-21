//! This consumer imports the technical foundation directly, without liberwin.
use nickel_util::{error::Error, evaluation::EvaluationRequest, files, json, process, sources};
use serde_json::json;
use std::{
    error::Error as StdError, fs, os::unix::fs::symlink, path::Path, process::Command,
    time::Duration,
};

#[test]
fn an_independent_consumer_selects_public_sources() -> Result<(), Box<dyn StdError>> {
    let root = tempfile::tempdir()?;
    fs::create_dir(root.path().join("src"))?;
    fs::write(root.path().join("src/main.rs"), b"fn main() {}\n")?;
    fs::write(root.path().join("src/.env"), b"secret")?;
    fs::write(root.path().join("src/ignored.txt"), b"excluded")?;
    let spec: sources::Sources = serde_json::from_value(json!({
        "include": ["src"], "exclude": ["src/ignored.txt"]
    }))?;
    let selected = sources::snapshot(root.path(), &spec, None)?;
    assert_eq!(
        selected.keys().map(String::as_str).collect::<Vec<_>>(),
        ["src/main.rs"]
    );
    assert_eq!(
        serde_json::to_value(&spec)?,
        json!({"include":["src"],"exclude":["src/ignored.txt"]})
    );
    assert!(serde_json::from_value::<sources::Sources>(
        json!({"include":[],"exclude":[],"provider":"private"})
    )
    .is_err());
    assert!(sources::relative_path("../outside").is_err());
    let invalid = sources::Sources {
        include: vec!["../outside".into()],
        exclude: vec![],
    };
    assert!(sources::snapshot(root.path(), &invalid, None).is_err());
    // An independent caller can describe evaluation without any domain schema.
    let request = EvaluationRequest {
        entry: Path::new("input.ncl"),
        contract: None,
        nix: Path::new("recipes"),
    };
    assert!(request.contract.is_none());
    Ok(())
}

#[test]
fn publication_requires_explicit_replacement_and_rejects_links() -> Result<(), Box<dyn StdError>> {
    let root = tempfile::tempdir()?;
    let target = root.path().join("receipt.json");
    files::publish_json(&target, &json!({"version":1}), false)?;
    let original = fs::read(&target)?;
    assert!(files::publish_json(&target, &json!({"version":2}), false).is_err());
    assert_eq!(fs::read(&target)?, original);
    files::publish_json(&target, &json!({"version":2}), true)?;
    assert_eq!(json::parse(&fs::read(&target)?)?, json!({"version":2}));
    let link = root.path().join("link.json");
    symlink(&target, &link)?;
    for replace in [false, true] {
        assert!(files::publish_json(&link, &json!({"version":3}), replace).is_err());
    }
    assert_eq!(json::parse(&fs::read(&target)?)?, json!({"version":2}));
    let directory_link = root.path().join("linked-parent");
    symlink(root.path(), &directory_link)?;
    assert!(files::publish_json(&directory_link.join("escaped.json"), &json!({}), false).is_err());
    assert!(!root.path().join("escaped.json").exists());
    Ok(())
}

#[test]
fn serialization_failure_leaves_the_previous_publication_intact() -> Result<(), Box<dyn StdError>> {
    struct Refuse;
    impl serde::Serialize for Refuse {
        fn serialize<S: serde::Serializer>(&self, _: S) -> Result<S::Ok, S::Error> {
            Err(serde::ser::Error::custom("refused by serializer"))
        }
    }
    let root = tempfile::tempdir()?;
    let target = root.path().join("receipt.json");
    fs::write(&target, b"original")?;
    assert!(files::publish_json(&target, &Refuse, true).is_err());
    assert_eq!(fs::read(&target)?, b"original");
    assert_eq!(fs::read_dir(root.path())?.count(), 1);
    Ok(())
}

#[test]
fn independent_process_errors_keep_typed_causes_and_exit_codes() -> Result<(), Box<dyn StdError>> {
    let error = process::capture(
        Command::new("/nonexistent/core-consumer-program"),
        Duration::from_secs(1),
        1024,
    )
    .err()
    .ok_or("expected spawn error")?;
    assert!(matches!(error, Error::ProcessSpawn { .. }));
    assert!(error
        .source()
        .and_then(|source| source.downcast_ref::<std::io::Error>())
        .is_some());
    assert_eq!(error.context("consumer operation").exit_code(), 1);
    let mut command = Command::new("/bin/sh");
    command.args(["-c", "exit 7"]);
    let error = process::capture(command, Duration::from_secs(1), 1024)
        .err()
        .ok_or("expected process failure")?;
    assert!(matches!(error, Error::ProcessExit { status, .. } if status.code() == Some(7)));
    assert_eq!(
        Error::Nickel {
            source: Box::new(error)
        }
        .exit_code(),
        2
    );
    Ok(())
}
