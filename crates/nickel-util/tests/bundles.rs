use nickel_util::{
    error::Error,
    evaluation::{
        evaluate, evaluate_bundled, BundleEvaluationRequest, BundledEvaluation, EvaluationRequest,
        LibraryBundle, LibraryContract,
    },
};
use serde_json::json;
use std::{
    error::Error as StdError,
    fs,
    os::unix::fs::symlink,
    path::{Path, PathBuf},
};

type TestResult = Result<(), Box<dyn StdError>>;

struct Fixture {
    _temporary: tempfile::TempDir,
    entry: PathBuf,
    library: PathBuf,
    nix: PathBuf,
}

impl Fixture {
    fn new() -> Result<Self, Box<dyn StdError>> {
        let temporary = tempfile::tempdir()?;
        let project = temporary.path().join("project");
        let library = temporary.path().join("library");
        let nix = temporary.path().join("nix");
        for directory in [&project, &library, &nix] {
            fs::create_dir(directory)?;
        }
        let source = std::env::var_os("NICKEL_BUNDLE_TEST_NIX")
            .map(PathBuf::from)
            .unwrap_or_else(|| Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/nix"));
        // Only the two explicitly public evaluator recipes enter the Nix store.
        // The selected flake's nickel package must not depend on other files.
        for name in ["flake.nix", "flake.lock"] {
            fs::copy(source.join(name), nix.join(name))?;
        }
        let entry = project.join("main.ncl");
        fs::write(&entry, "import \"fixture/application.ncl\"")?;
        fs::write(
            library.join("application.ncl"),
            "{ answer = import \"value.ncl\" }",
        )?;
        fs::write(library.join("value.ncl"), "42")?;
        Ok(Self {
            _temporary: temporary,
            entry,
            library,
            nix,
        })
    }

    fn evaluate(&self, contract: Option<&Path>) -> nickel_util::error::Result<BundledEvaluation> {
        evaluate_bundled(&BundleEvaluationRequest {
            entry: &self.entry,
            nix: &self.nix,
            libraries: &[LibraryBundle {
                name: "fixture",
                root: &self.library,
            }],
            contract: contract.map(|entry| LibraryContract {
                library: "fixture",
                entry,
            }),
        })
    }
}

#[test]
fn named_imports_resolve_siblings_and_hash_only_staged_modules() -> TestResult {
    let fixture = Fixture::new()?;
    fs::write(fixture.library.join("private.txt"), "not a Nickel module")?;
    let first = fixture.evaluate(None)?;
    assert_eq!(first.evaluation.value, json!({"answer":42}));
    assert_eq!(first.evaluation.modules_sha256.len(), 1);
    assert_eq!(
        first.libraries_sha256["fixture"]
            .keys()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        ["application.ncl", "value.ncl"]
    );
    assert_eq!(
        first.libraries_sha256["fixture"]["value.ncl"],
        "73475cb40a568e8da8a045ced110137e159f890ac4da883b6b17dc651b3a8049"
    );
    fs::write(fixture.library.join("value.ncl"), "43")?;
    let second = fixture.evaluate(None)?;
    assert_eq!(second.evaluation.value, json!({"answer":43}));
    assert_ne!(first.libraries_sha256, second.libraries_sha256);
    fs::write(fixture.library.join("unused.ncl"), "null")?;
    assert_ne!(
        second.libraries_sha256,
        fixture.evaluate(None)?.libraries_sha256
    );
    Ok(())
}

#[test]
fn bundle_contract_resolves_siblings_and_rejects_invalid_values() -> TestResult {
    let fixture = Fixture::new()?;
    fs::create_dir(fixture.library.join("contracts"))?;
    fs::write(
        fixture.library.join("contracts/main.ncl"),
        "{ answer | (import \"number.ncl\") }",
    )?;
    fs::write(fixture.library.join("contracts/number.ncl"), "Number")?;
    let result = fixture.evaluate(Some(Path::new("contracts/main.ncl")))?;
    assert_eq!(result.evaluation.value, json!({"answer":42}));
    assert_eq!(
        result.evaluation.contract_sha256.as_ref(),
        result.libraries_sha256["fixture"].get("contracts/main.ncl")
    );
    fs::write(fixture.library.join("value.ncl"), "\"invalid\"")?;
    assert!(matches!(
        fixture.evaluate(Some(Path::new("contracts/main.ncl"))),
        Err(Error::Nickel { .. })
    ));
    Ok(())
}

#[test]
fn project_files_cannot_replace_a_named_library() -> TestResult {
    let fixture = Fixture::new()?;
    let project = fixture.entry.parent().ok_or("missing project")?;
    fs::create_dir(project.join("fixture"))?;
    fs::write(project.join("fixture/application.ncl"), "{ answer = 999 }")?;
    assert!(matches!(
        fixture.evaluate(None),
        Err(Error::InvalidInput { .. })
    ));
    Ok(())
}

#[test]
fn confined_imports_cannot_read_host_files_nonmodules_or_network() -> TestResult {
    let fixture = Fixture::new()?;
    assert_eq!(
        fixture.evaluate(None)?.evaluation.value,
        json!({"answer":42})
    );
    let host = fixture
        .library
        .parent()
        .ok_or("missing parent")?
        .join("secret.ncl");
    fs::write(&host, "{ secret = true }")?;
    fs::write(fixture.library.join("private.json"), "{\"secret\":true}")?;
    for source in [
        format!("import {}", serde_json::to_string(&host.to_string_lossy())?),
        "import \"../../secret.ncl\"".into(),
        "import \"/etc/passwd\"".into(),
        "import \"/proc/self/environ\"".into(),
        "import \"fixture/private.json\"".into(),
        "import \"http://127.0.0.1:1/private.ncl\"".into(),
    ] {
        fs::write(&fixture.entry, &source)?;
        assert!(
            matches!(fixture.evaluate(None), Err(Error::Nickel { .. })),
            "unexpected successful import: {source}"
        );
    }
    Ok(())
}

#[test]
fn library_names_must_be_unique_safe_components() -> TestResult {
    let fixture = Fixture::new()?;
    for name in [
        "",
        ".",
        "..",
        "/absolute",
        "../escape",
        "nested/name",
        "space name",
    ] {
        let result = evaluate_bundled(&BundleEvaluationRequest {
            entry: &fixture.entry,
            nix: &fixture.nix,
            libraries: &[LibraryBundle {
                name,
                root: &fixture.library,
            }],
            contract: None,
        });
        assert!(
            matches!(result, Err(Error::InvalidInput { .. })),
            "accepted {name:?}"
        );
    }
    let result = evaluate_bundled(&BundleEvaluationRequest {
        entry: &fixture.entry,
        nix: &fixture.nix,
        libraries: &[
            LibraryBundle {
                name: "same",
                root: &fixture.library,
            },
            LibraryBundle {
                name: "same",
                root: &fixture.library,
            },
        ],
        contract: None,
    });
    assert!(matches!(result, Err(Error::InvalidInput { .. })));
    Ok(())
}

#[test]
fn bundle_contract_must_be_a_staged_relative_module() -> TestResult {
    let fixture = Fixture::new()?;
    for entry in [
        "../application.ncl",
        "/application.ncl",
        "./application.ncl",
        "contracts/../application.ncl",
        "missing.ncl",
        "private.txt",
    ] {
        assert!(
            fixture.evaluate(Some(Path::new(entry))).is_err(),
            "accepted {entry}"
        );
    }
    assert!(evaluate_bundled(&BundleEvaluationRequest {
        entry: &fixture.entry,
        nix: &fixture.nix,
        libraries: &[LibraryBundle {
            name: "fixture",
            root: &fixture.library
        }],
        contract: Some(LibraryContract {
            library: "absent",
            entry: Path::new("application.ncl")
        }),
    })
    .is_err());
    Ok(())
}

#[test]
fn library_symlinks_are_refused_for_files_directories_and_ancestors() -> TestResult {
    let fixture = Fixture::new()?;
    let parent = fixture.library.parent().ok_or("missing parent")?;
    for name in ["linked.ncl", "linked.txt", "directory"] {
        let link = fixture.library.join(name);
        symlink(
            if name == "directory" {
                parent
            } else {
                &fixture.entry
            },
            &link,
        )?;
        assert!(fixture.evaluate(None).is_err(), "accepted link {name}");
        fs::remove_file(link)?;
    }
    let link = parent.join("linked-parent");
    symlink(parent, &link)?;
    for root in [
        link.join("library"),
        fixture.library.join("../library"),
        fixture.library.join("./"),
    ] {
        assert!(
            evaluate_bundled(&BundleEvaluationRequest {
                entry: &fixture.entry,
                nix: &fixture.nix,
                libraries: &[LibraryBundle {
                    name: "fixture",
                    root: &root
                }],
                contract: None,
            })
            .is_err(),
            "accepted root {}",
            root.display()
        );
    }
    Ok(())
}

#[test]
fn project_entry_and_recipe_ancestors_may_not_be_symlinks() -> TestResult {
    let fixture = Fixture::new()?;
    let parent = fixture.library.parent().ok_or("missing parent")?;
    let link = parent.join("linked-parent");
    symlink(parent, &link)?;
    for (entry, nix) in [
        (link.join("project/main.ncl"), fixture.nix.clone()),
        (fixture.entry.clone(), link.join("nix")),
    ] {
        assert!(evaluate_bundled(&BundleEvaluationRequest {
            entry: &entry,
            nix: &nix,
            libraries: &[],
            contract: None,
        })
        .is_err());
    }
    Ok(())
}

#[test]
fn full_schema_sized_bundles_fit_without_relaxing_project_limits() -> TestResult {
    let fixture = Fixture::new()?;
    for index in 0..1310 {
        fs::write(fixture.library.join(format!("schema-{index}.ncl")), "null")?;
    }
    assert_eq!(
        fixture.evaluate(None)?.libraries_sha256["fixture"].len(),
        1312
    );
    let project = fixture.entry.parent().ok_or("missing project")?;
    for index in 0..100 {
        fs::write(project.join(format!("module-{index}.ncl")), "null")?;
    }
    assert!(fixture.evaluate(None).is_err());
    assert!(evaluate(&EvaluationRequest {
        entry: &fixture.entry,
        contract: None,
        nix: &fixture.nix
    })
    .is_err());
    Ok(())
}

#[test]
fn bundle_file_and_aggregate_module_limits_are_enforced() -> TestResult {
    let fixture = Fixture::new()?;
    fs::write(
        fixture.library.join("large.ncl"),
        vec![b' '; 1024 * 1024 + 1],
    )?;
    assert!(fixture.evaluate(None).is_err());
    fs::remove_file(fixture.library.join("large.ncl"))?;
    for index in 0..1023 {
        fs::write(fixture.library.join(format!("schema-{index}.ncl")), "null")?;
    }
    assert!(evaluate_bundled(&BundleEvaluationRequest {
        entry: &fixture.entry,
        nix: &fixture.nix,
        libraries: &[
            LibraryBundle {
                name: "first",
                root: &fixture.library
            },
            LibraryBundle {
                name: "second",
                root: &fixture.library
            }
        ],
        contract: None,
    })
    .is_err());
    Ok(())
}

#[test]
fn bundle_count_entry_count_total_bytes_and_depth_are_bounded() -> TestResult {
    let fixture = Fixture::new()?;
    let names = (0..9)
        .map(|index| format!("lib{index}"))
        .collect::<Vec<_>>();
    let bundles = names
        .iter()
        .map(|name| LibraryBundle {
            name,
            root: &fixture.library,
        })
        .collect::<Vec<_>>();
    assert!(evaluate_bundled(&BundleEvaluationRequest {
        entry: &fixture.entry,
        nix: &fixture.nix,
        libraries: &bundles,
        contract: None,
    })
    .is_err());
    let mut directory = fixture.library.clone();
    for _ in 0..33 {
        directory = directory.join("deep");
    }
    fs::create_dir_all(&directory)?;
    fs::write(directory.join("value.ncl"), "null")?;
    assert!(fixture.evaluate(None).is_err());
    fs::remove_dir_all(fixture.library.join("deep"))?;
    for index in 0..4097 {
        fs::write(fixture.library.join(format!("ignored-{index}.txt")), "")?;
    }
    assert!(fixture.evaluate(None).is_err());
    for index in 0..4097 {
        fs::remove_file(fixture.library.join(format!("ignored-{index}.txt")))?;
    }
    let module = vec![b' '; 1024 * 1024];
    for index in 0..65 {
        fs::write(fixture.library.join(format!("large-{index}.ncl")), &module)?;
    }
    assert!(fixture.evaluate(None).is_err());
    Ok(())
}

#[test]
fn legacy_evaluation_receipt_keeps_its_serialized_shape() -> TestResult {
    let fixture = Fixture::new()?;
    fs::write(&fixture.entry, "{ value = 7 }")?;
    let old = evaluate(&EvaluationRequest {
        entry: &fixture.entry,
        contract: None,
        nix: &fixture.nix,
    })?;
    let bundled = evaluate_bundled(&BundleEvaluationRequest {
        entry: &fixture.entry,
        nix: &fixture.nix,
        libraries: &[],
        contract: None,
    })?;
    assert_eq!(
        serde_json::to_value(&old)?,
        serde_json::to_value(&bundled.evaluation)?
    );
    assert_eq!(
        serde_json::to_value(&old)?
            .as_object()
            .ok_or("receipt object")?
            .len(),
        5
    );
    assert!(bundled.libraries_sha256.is_empty());
    Ok(())
}
