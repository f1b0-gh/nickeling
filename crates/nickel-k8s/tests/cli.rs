#![cfg(feature = "cli")]
use std::{
    error::Error,
    fs,
    os::unix::fs::symlink,
    path::Path,
    process::{Command, Output},
};
fn run(config: &Path, args: &[&std::ffi::OsStr]) -> Result<Output, Box<dyn Error>> {
    Ok(Command::new(env!("CARGO_BIN_EXE_nickel-k8s"))
        .arg("render")
        .arg("--config")
        .arg(config)
        .args(["--resources", env!("CARGO_MANIFEST_DIR")])
        .args(args)
        .output()?)
}
#[test]
fn cli_publishes_a_kubernetes_list_and_refuses_overwrite_or_symlinks() -> Result<(), Box<dyn Error>>
{
    let tmp = tempfile::tempdir()?;
    let config = Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/configmap.ncl");
    let output = tmp.path().join("manifest.json");
    let args = [std::ffi::OsStr::new("--output"), output.as_os_str()];
    let first = run(&config, &args)?;
    assert!(
        first.status.success(),
        "{}",
        String::from_utf8_lossy(&first.stderr)
    );
    assert!(first.stdout.is_empty());
    let bytes = fs::read(&output)?;
    let value: serde_json::Value = serde_json::from_slice(&bytes)?;
    assert_eq!(value["kind"], "List");
    assert_eq!(value["items"][0]["kind"], "ConfigMap");
    assert!(!run(&config, &args)?.status.success());
    assert_eq!(fs::read(&output)?, bytes);
    let link = tmp.path().join("link.json");
    symlink(&output, &link)?;
    assert!(!run(
        &config,
        &["--output".as_ref(), link.as_os_str(), "--replace".as_ref()]
    )?
    .status
    .success());
    assert_eq!(fs::read(&output)?, bytes);
    let invalid = Command::new(env!("CARGO_BIN_EXE_nickel-k8s"))
        .args(["render", "--replace"])
        .output()?;
    assert!(!invalid.status.success());
    Ok(())
}
