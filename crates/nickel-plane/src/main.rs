use clap::{Args, Parser, Subcommand};
use nickel_plane::{
    composition::{compile, CompileRequest, Compiled},
    reconcile::{self, Options},
    store::{self, Store},
    targets::Targets,
    Error, Result,
};
use nickel_util::{files, json};
use serde::de::DeserializeOwned;
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, OnceLock,
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};

mod scheduling;

#[derive(Parser)]
#[command(
    version,
    about = "Compile and reconcile Nickel compositions with explicit targets and durable ownership"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}
#[derive(Args)]
struct Input {
    #[arg(long)]
    config: PathBuf,
    #[arg(long)]
    resources: PathBuf,
    #[arg(long)]
    targets: PathBuf,
    #[arg(long)]
    tf_resources: Option<PathBuf>,
    #[arg(long)]
    k8s_resources: Option<PathBuf>,
    /// Frozen provider bundle mapping, repeat as NAME=PATH.
    #[arg(long,value_parser=schema)]
    schema: Vec<(String, PathBuf)>,
    /// Require an existing input receipt to match before execution.
    #[arg(long)]
    locked: Option<PathBuf>,
}
fn schema(s: &str) -> std::result::Result<(String, PathBuf), String> {
    let (name, path) = s.split_once('=').ok_or("Expected NAME=PATH")?;
    nickel_plane::model::validate_id(name).map_err(|e| e.to_string())?;
    if path.is_empty() {
        return Err("Schema path required".into());
    }
    Ok((name.into(), path.into()))
}
#[derive(Args)]
struct Runtime {
    #[command(flatten)]
    input: Input,
    #[arg(long)]
    store: PathBuf,
    #[arg(long)]
    retry_failed: bool,
}
#[derive(Subcommand)]
enum Command {
    /// Validate source and domain contracts without contacting targets.
    Check(Input),
    /// Render deterministic unit templates; bindings resolve during reconciliation.
    Render(Input),
    /// Emit a digest receipt, containing no configuration values.
    Lock(Input),
    /// Observe targets and prepare changes without applying them.
    Plan(Runtime),
    /// Execute one previously prepared operation; refuse stale plans.
    Apply {
        #[command(flatten)]
        runtime: Runtime,
        #[arg(long)]
        plan: String,
    },
    /// Read public conditions and outputs without taking the controller lease.
    Status {
        #[arg(long)]
        store: PathBuf,
        #[arg(long)]
        instance: String,
    },
    /// Run one reconciliation cycle under the instance's management policy.
    Reconcile {
        #[command(flatten)]
        runtime: Runtime,
        #[arg(long, required = true)]
        once: bool,
    },
    /// Run one controller with bounded retries, watches and periodic resynchronization.
    Serve {
        #[command(flatten)]
        runtime: Runtime,
        #[arg(long, default_value_t = 30)]
        interval_seconds: u64,
    },
    /// Retain resources by default; --delete also requires the instance deletion policy.
    Remove {
        #[arg(long)]
        store: PathBuf,
        #[arg(long)]
        instance: String,
        #[arg(long)]
        targets: PathBuf,
        #[arg(long)]
        delete: bool,
    },
}
fn read<T: DeserializeOwned>(path: &Path) -> Result<T> {
    let bytes = files::read_limited(path, 16 * 1024 * 1024)?;
    Ok(serde_json::from_value(json::parse(&bytes)?)?)
}
fn receipt(c: &Compiled) -> Value {
    json!({"version":c.version,"instance":c.instance.id,"revision":c.revision,"inputs":c.inputs,"units":c.units.iter().map(|(name,u)|(name,json!({"artifactSha256":files::hash(&serde_json::to_vec(&u.artifact).unwrap_or_default()),"receipt":u.receipt}))).collect::<BTreeMap<_,_>>()})
}
impl Input {
    fn load(&self) -> Result<(CompileRequest, Targets, Compiled)> {
        let targets: Targets = read(&self.targets)?;
        targets.validate()?;
        let mut schemas = BTreeMap::new();
        for (name, path) in &self.schema {
            if schemas.insert(name.clone(), path.clone()).is_some() {
                return Err(Error::Invalid("Duplicate schema name".into()));
            }
        }
        let request = CompileRequest {
            config: self.config.clone(),
            resources: self.resources.clone(),
            tf_resources: self.tf_resources.clone(),
            k8s_resources: self.k8s_resources.clone(),
            schemas,
            targets: targets.targets.keys().cloned().collect(),
        };
        let compiled = compile(&request)?;
        for unit in compiled.instance.units.values() {
            if targets.targets[&unit.target].kind() != unit.backend {
                return Err(Error::Invalid("Target backend differs from unit".into()));
            }
        }
        if let Some(lock) = &self.locked {
            let previous: Value = read(lock)?;
            if previous != receipt(&compiled) {
                return Err(Error::Conflict(
                    "Input receipt changed; review and generate a new lock".into(),
                ));
            }
        }
        Ok((request, targets, compiled))
    }
}
fn print(value: &Value) -> Result<()> {
    use std::io::Write;
    let mut out = std::io::stdout().lock();
    serde_json::to_writer_pretty(&mut out, value)?;
    writeln!(out)?;
    Ok(())
}
static CANCEL: OnceLock<Arc<AtomicBool>> = OnceLock::new();
extern "C" fn cancel(_: libc::c_int) {
    if let Some(flag) = CANCEL.get() {
        flag.store(true, Ordering::SeqCst)
    }
}
fn cancellation() -> Arc<AtomicBool> {
    let flag = CANCEL
        .get_or_init(|| Arc::new(AtomicBool::new(false)))
        .clone();
    // SAFETY: installs a function with C signal ABI. The handler only accesses a
    // preinitialized OnceLock and performs a lock-free atomic store.
    unsafe {
        libc::signal(libc::SIGINT, cancel as *const () as libc::sighandler_t);
        libc::signal(libc::SIGTERM, cancel as *const () as libc::sighandler_t);
    }
    flag
}
fn now() -> Result<u64> {
    Ok(SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| Error::Invalid("Clock precedes epoch".into()))?
        .as_secs())
}
fn has_failure(value: &Value) -> bool {
    value["units"]
        .as_object()
        .is_some_and(|m| m.values().any(|v| v["conditions"]["Failed"] == true))
}
fn run_runtime(
    runtime: Runtime,
    prepare: bool,
    approved: Option<String>,
    interval: Option<u64>,
) -> Result<()> {
    if interval.is_some_and(|n| !(1..=3600).contains(&n)) {
        return Err(Error::Invalid("Interval must be 1..3600 seconds".into()));
    }
    let flag = cancellation();
    let mut store = Store::open(&runtime.store)?;
    let (mut validator, mut targets, mut compiled) = runtime.input.load()?;
    if let Some(operation) = &approved {
        store.ensure_revision(&compiled.instance.id, &compiled.revision)?;
        if !store.units(&compiled.instance.id)?.values().any(|s| {
            s.prepared
                .as_ref()
                .is_some_and(|p| &p.operation == operation && p.revision == compiled.revision)
        }) {
            return Err(Error::Conflict(
                "Unknown or superseded prepared operation".into(),
            ));
        }
    }
    let mut first = true;
    loop {
        if flag.load(Ordering::SeqCst) {
            return Ok(());
        }
        if !first {
            match runtime.input.load() {
                Ok((v, t, c)) => {
                    validator = v;
                    targets = t;
                    compiled = c;
                }
                Err(error) => {
                    eprintln!("Compilation failed; desired revision preserved: {error}");
                    std::thread::sleep(Duration::from_secs(1));
                    continue;
                }
            }
        }
        let mut registry = targets.registry()?;
        store.submit(&compiled)?;
        let options = Options {
            now: now()?,
            execute: !prepare,
            prepare,
            approved_operation: approved.clone(),
            retry_failed: runtime.retry_failed && first,
            continuous_retry: interval.is_some(),
            cancelled: flag.clone(),
        };
        match reconcile::reconcile(&mut store, &compiled, &validator, &mut registry, &options) {
            Err(Error::Cancelled) => return Ok(()),
            other => other?,
        }
        let status = store::status(&runtime.store, &compiled.instance.id)?;
        print(&status)?;
        let Some(interval) = interval else {
            return if has_failure(&status) {
                Err(Error::Backend(
                    "One or more units failed; inspect status".into(),
                ))
            } else {
                Ok(())
            };
        };
        first = false;
        let start = std::time::Instant::now();
        scheduling::wait_for_resync(
            Duration::from_secs(interval),
            || flag.load(Ordering::SeqCst),
            || reconcile::poll_events(&store, &compiled, &mut registry),
            || start.elapsed(),
            std::thread::sleep,
        )?;
    }
}
fn run(cli: Cli) -> Result<()> {
    match cli.command {
        Command::Check(input) => {
            let (_, _, compiled) = input.load()?;
            print(
                &json!({"valid":true,"instance":compiled.instance.id,"revision":compiled.revision}),
            )
        }
        Command::Lock(input) => {
            let (_, _, compiled) = input.load()?;
            print(&receipt(&compiled))
        }
        Command::Render(input) => {
            let (_, _, compiled) = input.load()?;
            print(
                &json!({"instance":compiled.instance.id,"revision":compiled.revision,"units":compiled.units.iter().map(|(id,u)|(id,json!({"backend":compiled.instance.units[id].backend,"artifact":u.artifact,"bindings":compiled.instance.units[id].bindings}))).collect::<BTreeMap<_,_>>()}),
            )
        }
        Command::Plan(runtime) => run_runtime(runtime, true, None, None),
        Command::Apply { runtime, plan } => run_runtime(runtime, false, Some(plan), None),
        Command::Reconcile { runtime, .. } => run_runtime(runtime, false, None, None),
        Command::Serve {
            runtime,
            interval_seconds,
        } => run_runtime(runtime, false, None, Some(interval_seconds)),
        Command::Status { store, instance } => print(&store::status(&store, &instance)?),
        Command::Remove {
            store: path,
            instance,
            targets,
            delete,
        } => {
            let mut store = Store::open(&path)?;
            let compiled = store.load(&instance)?;
            let targets: Targets = read(&targets)?;
            let mut registry = targets.registry()?;
            store.mark_removed(&instance, delete)?;
            // Removal observes retained artifacts; no new configuration is evaluated.
            struct NoCompile;
            impl nickel_plane::composition::Validator for NoCompile {
                fn validate(
                    &self,
                    _: &nickel_plane::model::Unit,
                    _: &Value,
                    _: &nickel_plane::composition::CompiledUnit,
                ) -> Result<nickel_plane::composition::CompiledUnit> {
                    Err(Error::Unsupported("Compilation during removal".into()))
                }
            }
            reconcile::reconcile(
                &mut store,
                &compiled,
                &NoCompile,
                &mut registry,
                &Options {
                    now: now()?,
                    cancelled: cancellation(),
                    ..Default::default()
                },
            )?;
            let status = store::status(&path, &instance)?;
            print(&status)?;
            if has_failure(&status) {
                Err(Error::Backend("Removal failed; inspect status".into()))
            } else {
                Ok(())
            }
        }
    }
}
fn main() {
    if let Err(error) = run(Cli::parse()) {
        eprintln!("{error}");
        std::process::exit(1)
    }
}
