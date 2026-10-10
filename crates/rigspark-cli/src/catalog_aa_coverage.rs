use clap::Parser;
use rigspark_cli::maintenance_file::File;
use rigspark_core::{
    catalog::{Catalog, parse_document},
    reports::strip_control,
};
use rigspark_runtime::{
    admission::{AdmissionTransport, NativeAdmissionTransport, RecordedAdmissionTransport},
    artificial_analysis::{PublisherSelection, collect_coverage},
    secure_fs::same_file,
};
use std::{
    error::Error,
    io::{self, Write},
    path::{Path, PathBuf},
    process::ExitCode,
};
use tokio_util::sync::CancellationToken;

#[derive(Parser)]
#[command(about = "Report immutable Artificial Analysis Open Weights catalog coverage")]
struct Args {
    #[arg(long, default_value = "crates/rigspark-core/data/models.json")]
    catalog_path: PathBuf,
    /// Reviewed publisher associations and complete export file selections.
    #[arg(
        long,
        default_value = "docs/references/artificial-analysis-publishers.json"
    )]
    publishers_path: PathBuf,
    /// Replay recorded public responses without network access.
    #[arg(long)]
    fixture: Option<PathBuf>,
    #[arg(long, default_value = "artificial-analysis-coverage.json")]
    out: PathBuf,
    #[arg(long)]
    now: Option<String>,
    /// Emit the report without writes; fail if coverage is incomplete or ambiguous.
    #[arg(long)]
    check: bool,
}
struct Input {
    path: PathBuf,
    file: File,
    bytes: Vec<u8>,
}
fn normalized(path: &Path) -> io::Result<PathBuf> {
    let absolute = std::path::absolute(path)?;
    let parent = absolute
        .parent()
        .ok_or_else(|| io::Error::other("file parent required"))?
        .canonicalize()?;
    Ok(parent.join(
        absolute
            .file_name()
            .ok_or_else(|| io::Error::other("filename required"))?,
    ))
}
fn utf8(bytes: &[u8]) -> io::Result<&str> {
    std::str::from_utf8(bytes).map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
}

async fn run(args: Args) -> Result<bool, Box<dyn Error>> {
    let mut paths = vec![args.catalog_path, args.publishers_path];
    paths.extend(args.fixture);
    let inputs: Vec<Input> = paths
        .into_iter()
        .map(|path| {
            let path = normalized(&path)?;
            let file = File::open(&path)?;
            let bytes = file.read()?;
            Ok(Input { path, file, bytes })
        })
        .collect::<io::Result<_>>()?;
    let catalog = Catalog::parse(utf8(&inputs[0].bytes)?)?;
    let selections: Vec<PublisherSelection> = parse_document(utf8(&inputs[1].bytes)?)?;
    let output = if args.check {
        None
    } else {
        let path = normalized(&args.out)?;
        let file = File::open(&path)?;
        let previous = file.read_optional()?;
        for input in &inputs {
            if path == input.path || (previous.is_some() && same_file(&path, &input.path)?) {
                return Err(io::Error::other("coverage output must not alias an input").into());
            }
        }
        Some((file, previous))
    };
    let transport: Box<dyn AdmissionTransport> = match inputs.get(2) {
        Some(fixture) => Box::new(RecordedAdmissionTransport::parse(utf8(&fixture.bytes)?)?),
        None => Box::new(NativeAdmissionTransport::new()?),
    };
    let now = args
        .now
        .map(Ok)
        .unwrap_or_else(rigspark_runtime::native_chat::timestamp)?;
    let cancel = CancellationToken::new();
    let report = tokio::select! {
        biased;
        signal = tokio::signal::ctrl_c() => {
            signal?;
            cancel.cancel();
            return Err(io::Error::new(io::ErrorKind::Interrupted, "Artificial Analysis coverage cancelled").into());
        }
        report = collect_coverage(&catalog, &selections, transport.as_ref(), &now, &cancel) => report?,
    };
    let encoded = format!("{}\n", serde_json::to_string_pretty(&report)?);
    if encoded.len() > 16 * 1024 * 1024 {
        return Err(io::Error::other("coverage snapshot exceeds 16 MiB").into());
    }
    for input in &inputs {
        if input.file.read()? != input.bytes {
            return Err(io::Error::other("coverage input changed during collection").into());
        }
    }
    if let Some((file, previous)) = output {
        if file.read_optional()? != previous {
            return Err(io::Error::other("coverage output changed during collection").into());
        }
        file.write_bytes(encoded.as_bytes(), previous.is_some())?;
    }
    io::stdout().lock().write_all(encoded.as_bytes())?;
    eprintln!(
        "Artificial Analysis coverage: {} ({} artifacts, {} runnable, {} advisory-only, {} unresolved rows)",
        if report.coverage.complete {
            "complete"
        } else {
            "incomplete"
        },
        report.coverage.unique_artifacts,
        report.coverage.covered_runnable,
        report.coverage.covered_advisory_only,
        report.coverage.unmatched.len(),
    );
    Ok(!args.check || report.coverage.complete)
}
#[tokio::main]
async fn main() -> ExitCode {
    match run(Args::parse()).await {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::FAILURE,
        Err(error) => {
            eprintln!(
                "catalog-aa-coverage failed: {}",
                strip_control(&error.to_string())
            );
            ExitCode::FAILURE
        }
    }
}
