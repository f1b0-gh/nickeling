#![cfg(feature = "cli")]
use std::{error::Error, fs, path::Path, process::Command};
fn cli(args: &[&str]) -> Result<std::process::Output, Box<dyn Error>> {
    Ok(Command::new(env!("CARGO_BIN_EXE_nickel-tf"))
        .args(args)
        .output()?)
}
fn text(path: &Path) -> Result<&str, Box<dyn Error>> {
    path.to_str()
        .ok_or_else(|| "non UTF-8 temporary path".into())
}
#[test]
fn commands_import_check_lock_render_and_refuse_overwrite() -> Result<(), Box<dyn Error>> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let tmp = tempfile::tempdir()?;
    let bundle = tmp.path().join("bundle");
    fs::create_dir(tmp.path().join("project"))?;
    let config = tmp.path().join("project/config.ncl");
    let output = tmp.path().join("main.tf.json");
    let lock = tmp.path().join("config.lock.json");
    fs::write(&config, "{resource.terraform_data.empty={}}")?;
    let imported = cli(&[
        "import-schema",
        "--schema",
        text(&root.join("examples/local/schema.json"))?,
        "--selection",
        text(&root.join("examples/local/selection.json"))?,
        "--output",
        text(&bundle)?,
    ])?;
    assert!(
        imported.status.success(),
        "{}",
        String::from_utf8_lossy(&imported.stderr)
    );
    let common = [
        "--config",
        text(&config)?,
        "--resources",
        text(root)?,
        "--bundle",
        text(&bundle)?,
    ];
    let mut args = vec!["check"];
    args.extend(common);
    let checked = cli(&args)?;
    assert!(
        checked.status.success(),
        "{}",
        String::from_utf8_lossy(&checked.stderr)
    );
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&checked.stdout)?["validation"],
        "offline"
    );
    let mut args = vec!["lock"];
    args.extend(common);
    args.extend(["--output", text(&lock)?]);
    assert!(cli(&args)?.status.success());
    let mut args = vec!["render"];
    args.extend(common);
    args.extend(["--lock", text(&lock)?, "--output", text(&output)?]);
    assert!(cli(&args)?.status.success());
    let bytes = fs::read(&output)?;
    assert!(!cli(&args)?.status.success());
    assert_eq!(fs::read(&output)?, bytes);
    args.push("--replace");
    assert!(cli(&args)?.status.success());
    fs::write(&config, "{resource.terraform_data.empty.input=1}")?;
    assert!(!cli(&args)?.status.success());
    assert_eq!(fs::read(&output)?, bytes);
    Ok(())
}
#[test]
fn replace_requires_an_explicit_output() -> Result<(), Box<dyn Error>> {
    let result = cli(&[
        "render",
        "--config",
        "x",
        "--resources",
        "x",
        "--bundle",
        "x",
        "--replace",
    ])?;
    assert_eq!(result.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&result.stderr).contains("--output"));
    Ok(())
}
