use clap::Parser;
use rigspark_cli::maintenance_file::File;
use rigspark_core::{
    catalog::Catalog, catalog_request::ModelRequest, generation::GenerationCatalog,
};
use rigspark_runtime::{
    admission::{AdmissionTransport, NativeAdmissionTransport, RecordedAdmissionTransport},
    catalog_request::{RequestDisposition, evaluate_request},
};
use serde_json::Value;
use std::{error::Error, io, path::PathBuf, process::ExitCode};
use tokio_util::sync::CancellationToken;

#[derive(Parser)]
#[command(
    about = "Route a GitHub model request to generation review or source-backed GGUF admission"
)]
struct Args {
    #[arg(long, default_value = "crates/rigspark-core/data/models.json")]
    catalog_path: PathBuf,
    #[arg(long, default_value = "docs/references/catalog-quality-evidence.json")]
    evidence_path: PathBuf,
    #[arg(long, default_value = "crates/rigspark-core/data/generation.json")]
    generation_path: PathBuf,
    #[arg(long)]
    event_path: PathBuf,
    #[arg(long)]
    fixture: Option<PathBuf>,
    #[arg(long)]
    now: Option<String>,
    #[arg(long)]
    dry_run: bool,
}

fn utf8(bytes: &[u8]) -> io::Result<&str> {
    std::str::from_utf8(bytes).map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
}

fn merge_observation(mut evidence: Value, observation: Value) -> io::Result<Value> {
    let id = observation["id"]
        .as_str()
        .ok_or_else(|| io::Error::other("request observation id missing"))?;
    let observations = evidence
        .get_mut("observations")
        .and_then(Value::as_array_mut)
        .ok_or_else(|| io::Error::other("evidence observations missing"))?;
    observations.retain(|item| item["id"].as_str() != Some(id));
    observations.push(observation);
    observations.sort_by(|left, right| left["id"].as_str().cmp(&right["id"].as_str()));
    Ok(evidence)
}

async fn run(args: Args) -> Result<Value, Box<dyn Error>> {
    let catalog_file = File::open(&args.catalog_path)?;
    let evidence_file = File::open(&args.evidence_path)?;
    let event_file = File::open(&args.event_path)?;
    let original_catalog = catalog_file.read()?;
    let original_evidence = evidence_file.read()?;
    let request = ModelRequest::from_event(utf8(&event_file.read()?)?)?;
    let catalog = Catalog::parse(utf8(&original_catalog)?)?;
    let generation = GenerationCatalog::parse(utf8(&File::open(&args.generation_path)?.read()?)?)?;
    let evidence: Value = serde_json::from_slice(&original_evidence)?;
    let transport: Box<dyn AdmissionTransport> = match args.fixture {
        Some(path) => Box::new(RecordedAdmissionTransport::parse(utf8(
            &File::open(&path)?.read()?,
        )?)?),
        None => Box::new(NativeAdmissionTransport::new()?),
    };
    let now = args
        .now
        .map(Ok)
        .unwrap_or_else(rigspark_runtime::native_chat::timestamp)?;
    let outcome = evaluate_request(
        &catalog,
        &generation,
        &request.repository,
        transport.as_ref(),
        &now,
        &CancellationToken::new(),
    )
    .await?;
    let mut report = serde_json::to_value(&outcome)?;
    report["issueNumber"] = Value::from(request.issue_number);
    report["issueUrl"] = Value::from(request.issue_url);

    if outcome.disposition == RequestDisposition::Eligible && !args.dry_run {
        let updated = outcome
            .catalog
            .ok_or_else(|| io::Error::other("eligible request produced no catalog"))?;
        let observation = outcome
            .observation
            .ok_or_else(|| io::Error::other("eligible request produced no evidence"))?;
        let merged_evidence = merge_observation(evidence, observation)?;
        if catalog_file.read()? != original_catalog || evidence_file.read()? != original_evidence {
            return Err(io::Error::other("catalog or evidence changed during request").into());
        }
        catalog_file.write_json(&updated, true)?;
        evidence_file.write_json(&merged_evidence, true)?;
    }
    Ok(report)
}

#[tokio::main]
async fn main() -> ExitCode {
    match run(Args::parse()).await {
        Ok(report) => match serde_json::to_string_pretty(&report) {
            Ok(report) => {
                println!("{report}");
                ExitCode::SUCCESS
            }
            Err(_) => {
                eprintln!("catalog-request: failed to serialize validated outcome");
                ExitCode::FAILURE
            }
        },
        Err(_) => {
            eprintln!(
                "catalog-request: failed; check the event, catalog, evidence, and upstream fixture"
            );
            ExitCode::FAILURE
        }
    }
}
