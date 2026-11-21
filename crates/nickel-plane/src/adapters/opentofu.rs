//! OpenTofu saved-plan execution. State and provider operations are always explicit.
use crate::{backend::*, model::BackendKind, require, targets::OpenTofuTarget, Error, Result};
use nickel_util::{files, process};
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    fs::OpenOptions,
    io::Write,
    os::unix::fs::{MetadataExt, OpenOptionsExt},
    path::Path,
    process::Command,
    time::Duration,
};
const OWNER: &str = "nickel_plane_ownership";
pub struct OpenTofu {
    target: OpenTofuTarget,
}
impl OpenTofu {
    pub fn new(target: OpenTofuTarget) -> Result<Self> {
        require(
            target.binary.is_absolute(),
            "OpenTofu binary must be an explicit absolute path",
        )?;
        files::open_regular(&target.binary)?;
        crate::model::validate_id(&target.stack)?;
        require(
            (1..=1800).contains(&target.timeout_seconds),
            "OpenTofu timeout must be 1..1800 seconds",
        )?;
        if let Some(backend) = &target.backend_config {
            require(
                matches!(backend.kind.as_str(), "s3" | "http") && backend.configuration.is_object(),
                "Execution backend supports s3/http; local state uses the controller workspace",
            )?;
            let fields = backend
                .configuration
                .as_object()
                .ok_or_else(|| Error::Invalid("Backend configuration must be an object".into()))?;
            for key in fields.keys() {
                require(
                    !["password", "username", "access_key", "secret_key", "token"]
                        .contains(&key.as_str()),
                    "Supply backend credentials through the protected credential file",
                )?;
            }
            if backend.kind == "http" {
                require(
                    fields.get("address").is_some_and(Value::is_string)
                        && fields.get("lock_address").is_some_and(Value::is_string)
                        && fields.get("unlock_address").is_some_and(Value::is_string),
                    "HTTP state address, lock_address and unlock_address are required",
                )?;
            } else {
                require(
                    fields.get("bucket").is_some_and(Value::is_string)
                        && fields.get("key").is_some_and(Value::is_string)
                        && fields.get("use_lockfile") == Some(&Value::Bool(true)),
                    "S3 state bucket/key and use_lockfile=true are required",
                )?;
            }
        }
        Ok(Self { target })
    }
    fn identity(&self, context: &Context) -> Value {
        json!({"store":context.owner,"instance":context.instance,"unit":context.unit_id,"stack":self.target.stack})
    }
    fn config(&self, context: &Context) -> Result<Value> {
        let mut config = context.artifact.clone();
        require(
            config.is_object(),
            "OpenTofu configuration must be an object",
        )?;
        require(
            config.get("module").is_none() && config["terraform"].get("backend").is_none(),
            "Generated configuration cannot own modules/backends",
        )?;
        require(
            config["resource"]["terraform_data"].get(OWNER).is_none(),
            "Reserved OpenTofu ownership resource",
        )?;
        let builtin = config["terraform"]["required_providers"]
            .as_object()
            .and_then(|m| {
                m.iter()
                    .find(|(_, p)| p["source"] == "terraform.io/builtin/terraform")
                    .map(|(name, _)| name.clone())
            });
        let alias = if let Some(alias) = builtin {
            alias
        } else {
            require(
                config["terraform"]["required_providers"]
                    .get("nickel-plane-builtin")
                    .is_none(),
                "Reserved built-in provider name",
            )?;
            config["terraform"]["required_providers"]["nickel-plane-builtin"] =
                json!({"source":"terraform.io/builtin/terraform"});
            "nickel-plane-builtin".into()
        };
        if let Some(types) = config["resource"].as_object_mut() {
            for instances in types.values_mut() {
                let instances = instances
                    .as_object_mut()
                    .ok_or_else(|| Error::Invalid("Invalid OpenTofu resource instances".into()))?;
                for body in instances.values_mut() {
                    if body.get("depends_on").is_none() {
                        body["depends_on"] = json!([]);
                    }
                    let dependencies = body["depends_on"]
                        .as_array_mut()
                        .ok_or_else(|| Error::Invalid("depends_on must be a static list".into()))?;
                    dependencies.push(json!(format!("terraform_data.{OWNER}")));
                }
            }
        }
        config["resource"]["terraform_data"][OWNER] =
            json!({"provider":alias,"input":self.identity(context)});
        Ok(config)
    }
    fn command(&self, context: &Context, args: &[&str]) -> Result<Command> {
        let mut command = Command::new(&self.target.binary);
        command
            .current_dir(&context.work)
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .env("HOME", context.work.join(".plane-home"))
            .env("TF_CLI_CONFIG_FILE", context.work.join("plane.tfrc"))
            .env("TF_IN_AUTOMATION", "1")
            .env("CHECKPOINT_DISABLE", "1")
            .env("TF_INPUT", "0");
        if let Some(path) = &self.target.credentials {
            let file = files::open_regular(path)?;
            require(
                file.metadata()?.mode() & 0o077 == 0,
                "Credential file must be private",
            )?;
            let credentials: BTreeMap<String, String> = serde_json::from_value(
                nickel_util::json::parse(&files::read_file(file, 1024 * 1024)?)
                    .map_err(|_| Error::Invalid("Invalid credential JSON".into()))?,
            )
            .map_err(|_| {
                Error::Invalid("Credentials must map environment names to strings".into())
            })?;
            for (name, value) in credentials {
                let valid = name
                    .bytes()
                    .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'_');
                let variable = name.starts_with("TF_VAR_")
                    && name.len() > 7
                    && name[7..]
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b == b'_');
                require(
                    (valid || variable)
                        && !name.is_empty()
                        && !["LD_", "DYLD_", "NIX_"]
                            .iter()
                            .any(|prefix| name.starts_with(prefix))
                        && !(name.starts_with("TF_")
                            && !variable
                            && !matches!(name.as_str(), "TF_HTTP_USERNAME" | "TF_HTTP_PASSWORD"))
                        && !name.starts_with("TOFU_")
                        && ![
                            "PATH",
                            "HOME",
                            "ENV",
                            "BASH_ENV",
                            "SHELL",
                            "RUST_LOG",
                            "SSLKEYLOGFILE",
                            "CHECKPOINT_DISABLE",
                        ]
                        .contains(&name.as_str()),
                    "Credential environment contains a reserved process setting",
                )?;
                command.env(name, value);
            }
        }
        command.args(args);
        Ok(command)
    }
    fn run(&self, context: &Context, args: &[&str]) -> Result<Vec<u8>> {
        process::capture(
            self.command(context, args)?,
            Duration::from_secs(self.target.timeout_seconds),
            16 * 1024 * 1024,
        )
        .map_err(|_| {
            Error::Backend(format!(
                "OpenTofu {}",
                args.first().copied().unwrap_or("operation")
            ))
        })
    }
    fn json(&self, context: &Context, args: &[&str]) -> Result<Value> {
        nickel_util::json::parse(&self.run(context, args)?)
            .map_err(|_| Error::Backend("OpenTofu JSON response".into()))
    }
    fn prepare_workspace(&self, context: &Context) -> Result<String> {
        let dir = files::open_directory(&context.work)?;
        require(
            dir.metadata()?.mode() & 0o077 == 0,
            "OpenTofu workspace must be private",
        )?;
        for item in std::fs::read_dir(&context.work)? {
            let item = item?;
            let name = item.file_name();
            let name = name.to_string_lossy();
            require(
                !item.file_type()?.is_symlink(),
                "Symlinks are forbidden at the execution workspace root",
            )?;
            if name.ends_with(".tf")
                || name.ends_with(".tf.json")
                || name.ends_with(".tofu")
                || name.ends_with(".tofu.json")
            {
                require(
                    name == "main.tf.json" || name == "plane.backend.tf.json",
                    "Unexpected configuration file in controller workspace",
                )?;
            }
        }
        std::fs::create_dir_all(context.work.join(".plane-home"))?;
        write_private(
            &context.work.join("plane.tfrc"),
            b"disable_checkpoint = true\n",
        )?;
        let version = self.json(context, &["version", "-json"])?;
        require(
            version["terraform_version"].as_str() == context.engine_version.as_deref(),
            "OpenTofu binary version differs from locked engine selection",
        )?;
        let config = self.config(context)?;
        let config_bytes = serde_json::to_vec(&config)?;
        write_private(&context.work.join("main.tf.json"), &config_bytes)?;
        if let Some(backend) = &self.target.backend_config {
            let mut block = serde_json::Map::new();
            block.insert(backend.kind.clone(), backend.configuration.clone());
            write_private(
                &context.work.join("plane.backend.tf.json"),
                &serde_json::to_vec(&json!({"terraform":{"backend":block}}))?,
            )?;
        } else {
            require(
                !context.work.join("plane.backend.tf.json").exists(),
                "Workspace backend changed; explicit migration required",
            )?;
        }
        match &context.provider_lock {
            Some(bytes) => {
                let path = context.work.join(".terraform.lock.hcl");
                if path.exists() {
                    require(
                        files::read_limited(&path, 16 * 1024 * 1024)? == *bytes,
                        "Native provider lock differs from locked configuration",
                    )?;
                } else {
                    write_private(&path, bytes)?;
                }
            }
            None => require(
                !context.work.join(".terraform.lock.hcl").exists(),
                "Unexpected native provider lock",
            )?,
        }
        self.run(
            context,
            &["init", "-input=false", "-no-color", "-lockfile=readonly"],
        )?;
        if let Some(bytes) = &context.provider_lock {
            require(
                files::read_limited(&context.work.join(".terraform.lock.hcl"), 16 * 1024 * 1024)?
                    == *bytes,
                "OpenTofu changed the provider lock",
            )?;
        }
        Ok(files::hash(&config_bytes))
    }
    fn state(&self, context: &Context) -> Result<Value> {
        let state = self.json(context, &["show", "-json"])?;
        let root = &state["values"]["root_module"];
        require(
            root["child_modules"].as_array().is_none_or(Vec::is_empty),
            "Existing module state cannot be adopted",
        )?;
        let resources = root["resources"]
            .as_array()
            .map(Vec::as_slice)
            .unwrap_or(&[]);
        if !resources.is_empty()
            || state["values"]["outputs"]
                .as_object()
                .is_some_and(|o| !o.is_empty())
        {
            let owned = resources
                .iter()
                .find(|r| r["address"] == format!("terraform_data.{OWNER}"));
            if !owned.is_some_and(|r| r["values"]["input"] == self.identity(context)) {
                return Err(Error::Conflict(
                    "OpenTofu state belongs to another owner or has no ownership marker".into(),
                ));
            }
        }
        Ok(state)
    }
    fn plan(&self, context: &Context, file: &str, destroy: bool) -> Result<(Value, String)> {
        let path = context.work.join(file);
        if path.exists() {
            files::open_regular(&path)?;
            std::fs::remove_file(&path)?;
        }
        let out = format!("-out={file}");
        let mut args = vec![
            "plan",
            "-input=false",
            "-no-color",
            "-lock-timeout=0s",
            &out,
        ];
        if destroy {
            args.push("-destroy");
        }
        self.run(context, &args)?;
        let plan = self.json(context, &["show", "-json", file])?;
        let hash = files::hash(&files::read_limited(&path, 64 * 1024 * 1024)?);
        Ok((plan, hash))
    }
    fn preconditions(&self, context: &Context, state: &Value, overlay: &str) -> Result<Value> {
        Ok(
            json!({"state":files::hash(&serde_json::to_vec(state)?),"overlay":overlay,"providerLock":context.provider_lock.as_ref().map(|b|files::hash(b))}),
        )
    }
    fn prepare(
        &self,
        context: &Context,
        observed: &Observation,
        operation: &str,
        destroy: bool,
    ) -> Result<PreparedChange> {
        require(
            !operation.is_empty()
                && operation.len() <= 64
                && operation
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_'),
            "Invalid operation ID",
        )?;
        let overlay = self.prepare_workspace(context)?;
        let state = self.state(context)?;
        require(
            self.preconditions(context, &state, &overlay)? == observed.preconditions,
            "OpenTofu state changed before planning",
        )?;
        let file = format!("operation-{operation}.tfplan");
        let (plan, hash) = self.plan(context, &file, destroy)?;
        Ok(PreparedChange {
            operation: operation.into(),
            revision: context.revision.clone(),
            artifact_sha256: context.artifact_sha256.clone(),
            target_sha256: self.fingerprint()?,
            summary: summary(&plan),
            payload: json!({"file":file,"sha256":hash,"binary":self.target.binary,"destroy":destroy}),
            preconditions: observed.preconditions.clone(),
        })
    }
}
fn write_private(path: &Path, bytes: &[u8]) -> Result<()> {
    // All ancestors are checked before opening the final regular file without following links.
    files::open_directory(
        path.parent()
            .ok_or_else(|| Error::Invalid("File parent required".into()))?,
    )?;
    let mut file = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK)
        .open(path)?;
    require(
        file.metadata()?.is_file() && file.metadata()?.nlink() == 1,
        "Execution input must be a private regular file",
    )?;
    file.set_len(0)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    Ok(())
}
fn summary(plan: &Value) -> ChangeSummary {
    let mut result = ChangeSummary::default();
    if let Some(resources) = plan["resource_changes"].as_array() {
        for resource in resources {
            let actions = resource["change"]["actions"]
                .as_array()
                .cloned()
                .unwrap_or_default();
            let create = actions.contains(&json!("create"));
            let delete = actions.contains(&json!("delete"));
            result.creates += usize::from(create);
            result.deletes += usize::from(delete);
            result.replacements += usize::from(create && delete);
            result.updates +=
                usize::from(actions.contains(&json!("update")) || actions.contains(&json!("read")));
        }
    }
    if let Some(outputs) = plan["output_changes"].as_object() {
        for output in outputs.values() {
            if output["actions"] != json!(["no-op"]) {
                result.updates += 1;
            }
        }
    }
    result
}
impl Backend for OpenTofu {
    fn kind(&self) -> BackendKind {
        BackendKind::OpenTofu
    }
    fn fingerprint(&self) -> Result<String> {
        Ok(files::hash(&serde_json::to_vec(
            &json!({"stack":self.target.stack,"backend":self.target.backend_config}),
        )?))
    }
    fn state_binding(&self) -> Result<Option<String>> {
        let key = if let Some(b) = &self.target.backend_config {
            if b.kind == "http" {
                json!({"backend":"http","address":b.configuration["address"]})
            } else {
                json!({"backend":"s3","bucket":b.configuration["bucket"],"key":b.configuration["key"],"endpoint":b.configuration["endpoints"]["s3"]})
            }
        } else {
            json!({"backend":"local","stack":self.target.stack})
        };
        Ok(Some(format!(
            "opentofu:{}",
            files::hash(&serde_json::to_vec(&key)?)
        )))
    }
    fn observe(&mut self, context: &Context) -> Result<Observation> {
        let overlay = self.prepare_workspace(context)?;
        let state = self.state(context)?;
        let (plan, _) = self.plan(context, "observation.tfplan", false)?;
        let exists = state["values"]["root_module"]["resources"]
            .as_array()
            .is_some_and(|a| !a.is_empty());
        let mut root = json!({"outputs":{}});
        let mut sensitive = Vec::new();
        if let Some(outputs) = state["values"]["outputs"].as_object() {
            for (name, output) in outputs {
                root["outputs"][name] = output["value"].clone();
                if output["sensitive"] == true {
                    sensitive.push(format!(
                        "/outputs/{}",
                        name.replace('~', "~0").replace('/', "~1")
                    ));
                }
            }
        }
        Ok(Observation {
            exists,
            drifted: summary(&plan).changed(),
            readiness: Readiness::Unsupported,
            outputs: select_outputs(
                &root,
                &context.unit.outputs,
                &sensitive,
                &format!("opentofu://{}/{}", context.instance, context.unit_id),
            )?,
            inventory: json!({"owner":self.identity(context),"state":files::hash(&serde_json::to_vec(&state)?)}),
            preconditions: self.preconditions(context, &state, &overlay)?,
        })
    }
    fn prepare_change(
        &mut self,
        context: &Context,
        observed: &Observation,
        operation: &str,
    ) -> Result<PreparedChange> {
        self.prepare(context, observed, operation, false)
    }
    fn execute_change(
        &mut self,
        context: &Context,
        change: &PreparedChange,
    ) -> Result<Observation> {
        require(
            change.revision == context.revision
                && change.artifact_sha256 == context.artifact_sha256
                && change.target_sha256 == self.fingerprint()?,
            "Stale OpenTofu saved plan",
        )?;
        require(
            change.payload["binary"] == serde_json::to_value(&self.target.binary)?,
            "OpenTofu binary selection changed since plan",
        )?;
        let overlay = self.prepare_workspace(context)?;
        let state = self.state(context)?;
        require(
            self.preconditions(context, &state, &overlay)? == change.preconditions,
            "OpenTofu saved plan state preconditions changed",
        )?;
        let file = change.payload["file"]
            .as_str()
            .ok_or_else(|| Error::Invalid("Saved plan filename missing".into()))?;
        require(
            file == format!("operation-{}.tfplan", change.operation) && !file.contains('/'),
            "Invalid saved plan path",
        )?;
        let actual = files::hash(&files::read_limited(
            &context.work.join(file),
            64 * 1024 * 1024,
        )?);
        require(
            change.payload["sha256"] == actual,
            "Saved OpenTofu plan was modified",
        )?;
        self.run(
            context,
            &[
                "apply",
                "-input=false",
                "-no-color",
                "-lock-timeout=0s",
                file,
            ],
        )?;
        if change.payload["destroy"] == true {
            let state = self.state(context)?;
            return Ok(Observation {
                exists: state["values"]["root_module"]["resources"]
                    .as_array()
                    .is_some_and(|r| !r.is_empty()),
                inventory: Value::Null,
                ..Default::default()
            });
        }
        self.observe(context)
    }
    fn remove(&mut self, context: &Context, observed: &Observation) -> Result<Observation> {
        let plan = self.prepare(context, observed, "remove", true)?;
        self.execute_change(context, &plan)
    }
}
