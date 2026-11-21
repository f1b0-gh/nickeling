use nickel_util::sources::{self, Sources};
use std::{error::Error, fs, os::unix::fs::symlink};

#[test]
fn source_credentials_directory_keeps_modules_and_filters_secret_children(
) -> Result<(), Box<dyn Error>> {
    let root = tempfile::tempdir()?;
    let destination = tempfile::tempdir()?;
    fs::create_dir_all(root.path().join("src/credentials/.aws"))?;
    fs::create_dir_all(root.path().join("src/credentials/.ssh"))?;
    fs::write(
        root.path().join("src/credentials/mod.rs"),
        b"pub struct Credential;\n",
    )?;
    for name in [
        "auth.json",
        "credentials",
        "token.key",
        "certificate.pem",
        ".aws/config",
        ".ssh/config",
    ] {
        fs::write(
            root.path().join("src/credentials").join(name),
            b"synthetic secret",
        )?;
    }
    for include in ["src", "src/credentials", "src/credentials/mod.rs"] {
        let spec = Sources {
            include: vec![include.into()],
            exclude: vec![],
        };
        let selected = sources::snapshot(root.path(), &spec, None)?;
        assert_eq!(
            selected.keys().map(String::as_str).collect::<Vec<_>>(),
            ["src/credentials/mod.rs"]
        );
    }
    let spec = Sources {
        include: vec!["src".into()],
        exclude: vec![],
    };
    sources::snapshot(root.path(), &spec, Some(destination.path()))?;
    assert_eq!(
        fs::read(destination.path().join("src/credentials/mod.rs"))?,
        b"pub struct Credential;\n"
    );
    assert_eq!(
        fs::read_dir(destination.path().join("src/credentials"))?.count(),
        1
    );
    assert!(!sources::excluded("src/credentials/mod.rs", &spec));
    assert!(sources::excluded("src/credentials", &spec));
    let excluded = Sources {
        include: vec!["src".into()],
        exclude: vec!["src/credentials".into()],
    };
    assert!(sources::snapshot(root.path(), &excluded, None)?.is_empty());
    Ok(())
}

#[test]
fn cargo_credentials_file_is_omitted_and_cannot_be_explicitly_selected(
) -> Result<(), Box<dyn Error>> {
    let root = tempfile::tempdir()?;
    fs::create_dir(root.path().join(".cargo"))?;
    fs::write(root.path().join(".cargo/credentials"), b"synthetic token")?;
    fs::write(root.path().join(".cargo/config.toml"), b"[build]\n")?;
    let spec = Sources {
        include: vec![".cargo".into()],
        exclude: vec![],
    };
    let selected = sources::snapshot(root.path(), &spec, None)?;
    assert_eq!(
        selected.keys().map(String::as_str).collect::<Vec<_>>(),
        [".cargo/config.toml"]
    );
    let explicit = Sources {
        include: vec![".cargo/credentials".into()],
        exclude: vec![],
    };
    assert!(sources::snapshot(root.path(), &explicit, None).is_err());
    assert!(sources::excluded(".cargo/credentials", &spec));
    Ok(())
}

#[test]
fn credentials_symlink_is_never_followed() -> Result<(), Box<dyn Error>> {
    let root = tempfile::tempdir()?;
    let outside = tempfile::tempdir()?;
    fs::create_dir(root.path().join("src"))?;
    fs::write(outside.path().join("mod.rs"), b"outside")?;
    symlink(outside.path(), root.path().join("src/credentials"))?;
    let spec = Sources {
        include: vec!["src".into()],
        exclude: vec![],
    };
    assert!(sources::snapshot(root.path(), &spec, None)?.is_empty());
    for include in ["src/credentials", "src/credentials/mod.rs"] {
        let explicit = Sources {
            include: vec![include.into()],
            exclude: vec![],
        };
        assert!(sources::snapshot(root.path(), &explicit, None).is_err());
    }
    Ok(())
}
