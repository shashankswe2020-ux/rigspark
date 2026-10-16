use clap::Parser;
use rigspark_cli::maintenance_file::File;
use rigspark_core::{
    generation::GenerationCatalog,
    reports::strip_control,
    workflow::{DeviceClass, Registry},
};
use rigspark_runtime::{
    admission::{AdmissionTransport, NativeAdmissionTransport, RecordedAdmissionTransport},
    workflow_compile::ObjectInfo,
    workflow_harvest::{HarvestInput, harvest},
};
use serde_json::Value;
use std::{error::Error, io, path::PathBuf, process::ExitCode};
use tokio_util::sync::CancellationToken;

#[derive(Parser)]
#[command(about = "Harvest official ComfyUI templates into pinned, unverified workflow candidates")]
struct Args {
    #[arg(long, default_value = "crates/rigspark-core/data/workflows.json")]
    workflows_path: PathBuf,
    #[arg(long, default_value = "crates/rigspark-core/data/generation.json")]
    catalog_path: PathBuf,
    /// `<cpu|mps|cuda>=<path>`: a ComfyUI `/object_info` captured on that device class.
    #[arg(long = "object-info", required = true)]
    object_info: Vec<String>,
    /// Templates commit to harvest (40 hex); defaults to the current `main`.
    #[arg(long)]
    commit: Option<String>,
    /// Most candidates one run may propose.
    #[arg(long, default_value_t = 10, value_parser = clap::value_parser!(u16).range(1..=50))]
    limit: u16,
    #[arg(long, default_value = "docs/references/workflow-harvest-report.json")]
    report_path: PathBuf,
    /// Replay recorded upstream responses instead of the network.
    #[arg(long)]
    fixture: Option<PathBuf>,
    /// Print the report without writing workflows or the report file.
    #[arg(long)]
    dry_run: bool,
}

fn utf8(bytes: &[u8]) -> io::Result<&str> {
    std::str::from_utf8(bytes).map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
}

fn device(spec: &str) -> io::Result<(DeviceClass, PathBuf)> {
    let (class, path) = spec
        .split_once('=')
        .filter(|(_, path)| !path.is_empty())
        .ok_or_else(|| io::Error::other("--object-info must be <cpu|mps|cuda>=<path>"))?;
    let class: DeviceClass = serde_json::from_value(Value::from(class))
        .map_err(|_| io::Error::other(format!("unknown device class {class}")))?;
    Ok((class, PathBuf::from(path)))
}

async fn run(args: Args) -> Result<(), Box<dyn Error>> {
    let workflows_file = File::open(&args.workflows_path)?;
    let original = workflows_file.read()?;
    let registry = Registry::parse(utf8(&original)?)?;
    let catalog = GenerationCatalog::parse(utf8(&File::open(&args.catalog_path)?.read()?)?)?;
    let mut object_info = Vec::new();
    for spec in &args.object_info {
        let (class, path) = device(spec)?;
        if object_info.iter().any(|(known, _)| *known == class) {
            return Err(io::Error::other(format!("duplicate --object-info for {spec}")).into());
        }
        let info =
            ObjectInfo::parse(utf8(&File::open(&path)?.read()?)?).map_err(io::Error::other)?;
        object_info.push((class, info));
    }
    let transport: Box<dyn AdmissionTransport> = match &args.fixture {
        Some(path) => Box::new(RecordedAdmissionTransport::parse(utf8(
            &File::open(path)?.read()?,
        )?)?),
        None => Box::new(NativeAdmissionTransport::new()?),
    };
    let input = HarvestInput {
        commit: args.commit.as_deref(),
        object_info: &object_info,
        registry: &registry,
        catalog: &catalog,
        limit: usize::from(args.limit),
    };
    let cancel = CancellationToken::new();
    let outcome = tokio::select! {
        biased;
        signal = tokio::signal::ctrl_c() => {
            signal?;
            cancel.cancel();
            return Err(io::Error::new(io::ErrorKind::Interrupted, "interrupted").into());
        }
        outcome = harvest(&input, transport.as_ref(), &cancel) => outcome?,
    };
    if !args.dry_run {
        if workflows_file.read()? != original {
            return Err(io::Error::other("workflows changed during the harvest").into());
        }
        if !outcome.candidates.is_empty() {
            let mut files = registry.files().to_vec();
            for candidate in &outcome.candidates {
                files.retain(|existing| existing.id != candidate.id);
                files.push(candidate.clone());
            }
            workflows_file.write_bytes(Registry::new(files)?.to_json().as_bytes(), true)?;
        }
        let report = File::open(&args.report_path)?;
        let exists = report.read_optional()?.is_some();
        report.write_json(&outcome.report, exists)?;
    }
    println!("{}", serde_json::to_string_pretty(&outcome.report)?);
    eprintln!(
        "workflow-harvest{}: commit={} candidates={} rejected={} skipped={} failures={} complete={}",
        if args.dry_run { " dry-run" } else { "" },
        &outcome.report.commit[..7],
        outcome.candidates.len(),
        outcome.report.rejected.len(),
        outcome.report.skipped.len(),
        outcome.report.failures.len(),
        outcome.report.complete
    );
    Ok(())
}

#[tokio::main]
async fn main() -> ExitCode {
    match run(Args::parse()).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!(
                "workflow-harvest failed: {}",
                strip_control(&error.to_string())
            );
            ExitCode::FAILURE
        }
    }
}
