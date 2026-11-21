use clap::{Args, Parser, Subcommand};
use nickel_tf::Request;
use nickel_util::{error::Result, files};
use serde_json::{json, Value};
use std::{io::Write, path::PathBuf};

#[derive(Parser)]
#[command(
    version,
    about = "Compile Nickel configuration to OpenTofu JSON using frozen provider schemas"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}
#[derive(Args)]
struct Input {
    #[arg(long)]
    config: PathBuf,
    /// Explicitly trusted nickel-tf resource directory (flake and Nickel library).
    #[arg(long)]
    resources: PathBuf,
    /// Frozen provider schema bundle produced by import-schema.
    #[arg(long)]
    bundle: PathBuf,
}
impl From<Input> for Request {
    fn from(input: Input) -> Self {
        Self {
            config: input.config,
            resources: input.resources,
            bundle: input.bundle,
        }
    }
}
#[derive(Subcommand)]
enum Command {
    /// Convert a schema export offline. Existing bundle directories are never replaced.
    ImportSchema {
        #[arg(long)]
        schema: PathBuf,
        #[arg(long)]
        selection: PathBuf,
        #[arg(long)]
        provider_lock: Option<PathBuf>,
        #[arg(long)]
        output: PathBuf,
    },
    /// Validate offline; OpenTofu retains expression, provider and state validation.
    Check {
        #[command(flatten)]
        input: Input,
    },
    /// Emit .tf.json configuration. No provider or infrastructure engine is executed.
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
        Command::ImportSchema {
            schema,
            selection,
            provider_lock,
            output,
        } => {
            nickel_tf::bundle::import_schema(
                &schema,
                &selection,
                provider_lock.as_deref(),
                &output,
            )?;
            Ok(None)
        }
        Command::Check { input } => {
            let checked = nickel_tf::check(&input.into())?;
            Ok(Some(
                json!({"valid":true,"validation":"offline","engine":checked.lock.selection.engine}),
            ))
        }
        Command::Render {
            input,
            lock,
            output,
            replace,
        } => {
            let manifest = nickel_tf::render(&input.into(), lock.as_deref())?;
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
            files::publish_json(&output, &nickel_tf::lock(&input.into())?, replace)?;
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
