use clap::{Args, Parser, Subcommand};
use nickel_k8s::Request;
use nickel_util::{error::Result, files};
use serde_json::{json, Value};
use std::{io::Write, path::PathBuf};

#[derive(Parser)]
#[command(
    version,
    about = "Compose and validate Kubernetes manifests with pinned Nickel contracts"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}
#[derive(Args)]
struct Input {
    #[arg(long)]
    config: PathBuf,
    /// Explicitly trusted nickel-k8s resource directory (flake and Nickel library).
    #[arg(long)]
    resources: PathBuf,
}
impl From<Input> for Request {
    fn from(input: Input) -> Self {
        Self {
            config: input.config,
            resources: input.resources,
        }
    }
}
#[derive(Subcommand)]
enum Command {
    /// Validate offline; does not contact a cluster or prove admission/runtime behavior.
    Check {
        #[command(flatten)]
        input: Input,
    },
    /// Emit a Kubernetes List in JSON, also accepted as YAML by Kubernetes tools.
    Render {
        #[command(flatten)]
        input: Input,
        #[arg(long)]
        lock: Option<PathBuf>,
        #[arg(long)]
        output: Option<PathBuf>,
        #[arg(long, requires = "output")]
        replace: bool,
    },
    /// Record configuration, contracts and evaluator versions without network access.
    Lock {
        #[command(flatten)]
        input: Input,
        #[arg(long)]
        output: PathBuf,
        #[arg(long)]
        replace: bool,
    },
}
fn run(cli: Cli) -> Result<Option<Value>> {
    match cli.command {
        Command::Check { input } => {
            let checked = nickel_k8s::check(&input.into())?;
            Ok(Some(
                json!({"valid":true,"validation":"offline","target":checked.target,
                "resources":checked.manifest["items"].as_array().map_or(0,Vec::len)}),
            ))
        }
        Command::Render {
            input,
            lock,
            output,
            replace,
        } => {
            let manifest = nickel_k8s::render(&input.into(), lock.as_deref())?;
            if let Some(path) = output {
                files::publish_json(&path, &manifest, replace)?;
                Ok(None)
            } else {
                Ok(Some(manifest))
            }
        }
        Command::Lock {
            input,
            output,
            replace,
        } => {
            files::publish_json(&output, &nickel_k8s::lock(&input.into())?, replace)?;
            Ok(None)
        }
    }
}
fn main() {
    let result = run(Cli::parse()).and_then(|value| {
        if let Some(value) = value {
            writeln!(
                std::io::stdout().lock(),
                "{}",
                serde_json::to_string_pretty(&value)?
            )?;
        }
        Ok(())
    });
    if let Err(error) = result {
        let _ = writeln!(std::io::stderr().lock(), "Error: {error}");
        std::process::exit(error.exit_code());
    }
}
