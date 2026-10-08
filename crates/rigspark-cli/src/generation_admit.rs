use clap::Parser;
use rigspark_cli::maintenance_file::File;
use rigspark_core::{generation::GenerationCatalog, reports::strip_control};
use rigspark_runtime::{
    admission::{AdmissionTransport, NativeAdmissionTransport, RecordedAdmissionTransport},
    generation_admission::{GenerationAdmissionState, admit_generation},
};
use std::{error::Error, io, path::PathBuf, process::ExitCode};
use tokio_util::sync::CancellationToken;

#[derive(Parser)]
#[command(about = "Admit new Comfy-Org image and video releases as fit-only generation entries")]
struct Args {
    #[arg(long, default_value = "crates/rigspark-core/data/generation.json")]
    catalog_path: PathBuf,
    #[arg(
        long,
        default_value = "docs/references/generation-admission-state.json"
    )]
    state_path: PathBuf,
    /// Replay recorded upstream responses instead of the network.
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

async fn run(args: Args) -> Result<(), Box<dyn Error>> {
    let catalog_file = File::open(&args.catalog_path)?;
    let state_file = File::open(&args.state_path)?;
    let original = catalog_file.read()?;
    let catalog = GenerationCatalog::parse(utf8(&original)?)?;
    let state_raw = state_file.read_optional()?;
    let state: GenerationAdmissionState = match &state_raw {
        Some(raw) => rigspark_core::catalog::parse_document(utf8(raw)?)?,
        None => GenerationAdmissionState::default(),
    };
    let transport: Box<dyn AdmissionTransport> = match &args.fixture {
        Some(path) => Box::new(RecordedAdmissionTransport::parse(utf8(
            &File::open(path)?.read()?,
        )?)?),
        None => Box::new(NativeAdmissionTransport::new()?),
    };
    let now = args
        .now
        .map(Ok)
        .unwrap_or_else(rigspark_runtime::native_chat::timestamp)?;
    let cancel = CancellationToken::new();
    let outcome = tokio::select! {
        biased;
        signal = tokio::signal::ctrl_c() => { signal?; cancel.cancel(); return Err(io::Error::new(io::ErrorKind::Interrupted, "interrupted").into()); }
        outcome = admit_generation(&catalog, transport.as_ref(), &now, state, &cancel) => outcome?,
    };
    let admitted = outcome
        .catalog
        .as_ref()
        .ok_or_else(|| io::Error::other("admission produced no catalog"))?;
    if catalog_file.read()? != original {
        return Err(io::Error::other("generation catalog changed during admission").into());
    }
    if !args.dry_run {
        if serde_json::to_value(admitted)? != serde_json::to_value(&catalog)? {
            catalog_file.write_json(admitted, true)?;
        }
        state_file.write_json(&outcome.state, state_raw.is_some())?;
    }
    println!("{}", serde_json::to_string_pretty(&outcome)?);
    eprintln!(
        "generation-admit{}: added={} removed={} rejected={} failures={} complete={}",
        if args.dry_run { " dry-run" } else { "" },
        outcome.added.len(),
        outcome.removed.len(),
        outcome.rejected.len(),
        outcome.failures.len(),
        outcome.complete
    );
    Ok(())
}

#[tokio::main]
async fn main() -> ExitCode {
    match run(Args::parse()).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!(
                "generation-admit failed: {}",
                strip_control(&error.to_string())
            );
            ExitCode::FAILURE
        }
    }
}
