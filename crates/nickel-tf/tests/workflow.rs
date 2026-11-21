use nickel_tf::{bundle, lock, render, Request};
use nickel_util::{files, sources};
use serde_json::json;
use std::{error::Error, fs, path::PathBuf};
fn resources() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}
const CONFIG: &str = r#"
let tf = import "/libraries/nickel-tf/expressions.ncl" in
{ resource.terraform_data = {
  empty = {},
  fixture = { input = { literal = "$" ++ "{not_a_reference}", empty = {}, list = [] } },
  derived = { input = tf.ref ["terraform_data", "fixture", "output"] },
}, output.result.value = tf.ref ["terraform_data", "derived", "output"] }
"#;
fn fixture() -> Result<(tempfile::TempDir, Request), Box<dyn Error>> {
    let tmp = tempfile::tempdir()?;
    let bundle_path = tmp.path().join("bundle");
    bundle::import_schema(
        &resources().join("examples/local/schema.json"),
        &resources().join("examples/local/selection.json"),
        None,
        &bundle_path,
    )?;
    fs::create_dir(tmp.path().join("project"))?;
    let config = tmp.path().join("project/config.ncl");
    fs::write(&config, CONFIG)?;
    Ok((
        tmp,
        Request {
            config,
            resources: resources(),
            bundle: bundle_path,
        },
    ))
}
#[test]
fn real_evaluator_preserves_empty_blocks_and_deferred_values() -> Result<(), Box<dyn Error>> {
    let (_tmp, request) = fixture()?;
    let value = render(&request, None)?;
    assert_eq!(value["resource"]["terraform_data"]["empty"], json!({}));
    assert_eq!(
        value["resource"]["terraform_data"]["fixture"]["input"],
        json!({"literal":"$${not_a_reference}","empty":{},"list":[]})
    );
    assert_eq!(
        value["resource"]["terraform_data"]["derived"]["input"],
        "${terraform_data.fixture.output}"
    );
    assert_eq!(value["terraform"]["required_version"], "= 1.13.2");
    Ok(())
}
#[test]
fn lock_tracks_project_modules_bundle_and_helper_changes() -> Result<(), Box<dyn Error>> {
    let (tmp, mut request) = fixture()?;
    let module = tmp.path().join("project/settings.ncl");
    fs::write(&module, "\"first\"")?;
    fs::write(
        &request.config,
        "{resource.terraform_data.fixture.input=import \"settings.ncl\"}",
    )?;
    let lock_path = tmp.path().join("lock.json");
    files::publish_json(&lock_path, &lock(&request)?, false)?;
    render(&request, Some(&lock_path))?;
    fs::write(&module, "\"changed\"")?;
    assert!(render(&request, Some(&lock_path)).is_err());
    fs::write(&module, "\"first\"")?;
    let copied = tempfile::tempdir()?;
    sources::snapshot(
        &resources(),
        &sources::Sources {
            include: vec!["flake.nix".into(), "flake.lock".into(), "nickel".into()],
            exclude: vec![],
        },
        Some(copied.path()),
    )?;
    request.resources = copied.path().to_path_buf();
    render(&request, Some(&lock_path))?;
    let helper = copied.path().join("nickel/expressions.ncl");
    fs::write(
        &helper,
        format!("{}\n# revised helper\n", fs::read_to_string(&helper)?),
    )?;
    assert!(render(&request, Some(&lock_path)).is_err());
    request.resources = resources();
    let schema = request.bundle.join("schema.json");
    fs::write(&schema, format!("{}\n", fs::read_to_string(&schema)?))?;
    assert!(render(&request, Some(&lock_path)).is_err());
    Ok(())
}
#[test]
fn generated_contract_rejects_computed_only_and_unknown_resources() -> Result<(), Box<dyn Error>> {
    let (_tmp, request) = fixture()?;
    for text in [
        "{resource.terraform_data.fixture.output=1}",
        "{resource.unknown.fixture={}}",
        "{resource.terraform_data.fixture.typo=1}",
    ] {
        fs::write(&request.config, text)?;
        let error = render(&request, None)
            .err()
            .ok_or("invalid configuration accepted")?;
        assert!(
            matches!(error, nickel_util::error::Error::Nickel { .. }),
            "unexpected error for {text}: {error}"
        );
    }
    Ok(())
}
#[test]
fn rejects_symlinked_config_and_library_shadowing() -> Result<(), Box<dyn Error>> {
    let (tmp, mut request) = fixture()?;
    let link = tmp.path().join("project/link.ncl");
    std::os::unix::fs::symlink(&request.config, &link)?;
    request.config = link;
    assert!(render(&request, None).is_err());
    fs::remove_file(&request.config)?;
    request.config = tmp.path().join("project/config.ncl");
    fs::create_dir(tmp.path().join("project/providers"))?;
    fs::write(tmp.path().join("project/providers/schema.ncl"), "Dyn")?;
    assert!(render(&request, None).is_err());
    Ok(())
}
