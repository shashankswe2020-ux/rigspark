use clap::Parser;
use rigspark_core::{
    catalog::{Catalog, EntryProvenance},
    reports::strip_control,
};
use rigspark_runtime::{
    admission::{
        AdmissionOptions, AdmissionState, AdmissionTransport, NativeAdmissionTransport,
        RecordedAdmissionTransport, SCOPE_NAME, admit,
    },
    secure_fs::Directory,
};
use serde_json::Value;
use std::{
    collections::BTreeSet,
    error::Error,
    io,
    path::{Path, PathBuf},
    process::ExitCode,
};
use tokio_util::sync::CancellationToken;

const MAX_FILE_BYTES: u64 = 16 * 1024 * 1024;

#[derive(Parser)]
#[command(
    about = "Admit the latest fully sourced Ollama library models into the catalog with cited evidence"
)]
struct Args {
    #[arg(long, default_value = "crates/rigspark-core/data/models.json")]
    catalog_path: PathBuf,
    #[arg(long, default_value = "docs/references/catalog-quality-evidence.json")]
    evidence_path: PathBuf,
    #[arg(long, default_value = "docs/references/catalog-admission-state.json")]
    state_path: PathBuf,
    /// Replay recorded upstream responses instead of the network.
    #[arg(long)]
    fixture: Option<PathBuf>,
    #[arg(long)]
    now: Option<String>,
    /// New variants sourced per run, newest first; the rest are deferred.
    #[arg(long, default_value_t = 150, value_parser = clap::value_parser!(u16).range(1..=2000))]
    max_new: u16,
    #[arg(long)]
    dry_run: bool,
}

struct File {
    directory: Directory,
    name: PathBuf,
}
impl File {
    fn open(path: &Path) -> io::Result<Self> {
        let path = std::path::absolute(path)?;
        let parent = path
            .parent()
            .ok_or_else(|| io::Error::other("file parent required"))?
            .canonicalize()?;
        let name = PathBuf::from(
            path.file_name()
                .ok_or_else(|| io::Error::other("filename required"))?,
        );
        Ok(Self {
            directory: Directory::open(&parent)?,
            name,
        })
    }
    fn read(&self) -> io::Result<Vec<u8>> {
        self.directory.read(&self.name, MAX_FILE_BYTES, false)
    }
    fn read_optional(&self) -> io::Result<Option<Vec<u8>>> {
        match self.read() {
            Ok(bytes) => Ok(Some(bytes)),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error),
        }
    }
    /// Replaces the file in place, or creates it on the first run.
    fn write_json(&self, value: &impl serde::Serialize, exists: bool) -> io::Result<()> {
        let encoded = format!(
            "{}\n",
            serde_json::to_string_pretty(value).map_err(io::Error::other)?
        );
        self.directory
            .write(&self.name, encoded.as_bytes(), !exists, exists)
    }
}

fn utf8(bytes: &[u8]) -> io::Result<&str> {
    std::str::from_utf8(bytes).map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
}

/// Keeps hand-written observations for surviving curated entries, replaces auto ones and the
/// admission scope, and drops observations for entries no longer in the catalog.
fn merge_evidence(
    mut evidence: Value,
    catalog: &Catalog,
    observations: Vec<Value>,
    scope: Value,
) -> io::Result<Value> {
    let curated: BTreeSet<&str> = catalog
        .models
        .iter()
        .filter(|model| model.provenance == EntryProvenance::Curated)
        .map(|model| model.id.as_str())
        .collect();
    let object = evidence
        .as_object_mut()
        .ok_or_else(|| io::Error::other("evidence must be an object"))?;
    let kept: Vec<Value> = object
        .get("observations")
        .and_then(Value::as_array)
        .ok_or_else(|| io::Error::other("evidence observations missing"))?
        .iter()
        .filter(|item| item["id"].as_str().is_some_and(|id| curated.contains(id)))
        .cloned()
        .chain(observations)
        .collect();
    object.insert("observations".into(), Value::Array(kept));
    let mut scopes: Vec<Value> = object
        .get("scopes")
        .and_then(Value::as_array)
        .ok_or_else(|| io::Error::other("evidence scopes missing"))?
        .iter()
        .filter(|item| item["name"] != SCOPE_NAME)
        .cloned()
        .collect();
    scopes.push(scope);
    object.insert("scopes".into(), Value::Array(scopes));
    Ok(evidence)
}

async fn run(args: Args) -> Result<(), Box<dyn Error>> {
    let catalog_file = File::open(&args.catalog_path)?;
    let evidence_file = File::open(&args.evidence_path)?;
    let state_file = File::open(&args.state_path)?;
    let original = catalog_file.read()?;
    let catalog = Catalog::parse(utf8(&original)?)?;
    let evidence_raw = evidence_file.read()?;
    let evidence: Value = serde_json::from_slice(&evidence_raw)?;
    let state_raw = state_file.read_optional()?;
    let state: AdmissionState = match &state_raw {
        Some(raw) => rigspark_core::catalog::parse_document(utf8(raw)?)?,
        None => AdmissionState::default(),
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
    let options = AdmissionOptions {
        now,
        max_new_variants: usize::from(args.max_new),
        state,
    };
    let cancel = CancellationToken::new();
    let outcome = tokio::select! {
        biased;
        signal = tokio::signal::ctrl_c() => { signal?; cancel.cancel(); return Err(io::Error::new(io::ErrorKind::Interrupted, "interrupted").into()); }
        outcome = admit(&catalog, transport.as_ref(), &options, &cancel) => outcome?,
    };
    let admitted = outcome
        .catalog
        .clone()
        .ok_or_else(|| io::Error::other("admission produced no catalog"))?;
    if catalog_file.read()? != original || evidence_file.read()? != evidence_raw {
        return Err(io::Error::other("catalog or evidence changed during admission").into());
    }
    if !args.dry_run {
        if serde_json::to_value(&admitted)? != serde_json::to_value(&catalog)? {
            catalog_file.write_json(&admitted, true)?;
        }
        let merged = merge_evidence(
            evidence,
            &admitted,
            outcome.observations.clone(),
            outcome.scope.clone(),
        )?;
        evidence_file.write_json(&merged, true)?;
        state_file.write_json(&outcome.state, state_raw.is_some())?;
    }
    println!("{}", serde_json::to_string_pretty(&outcome)?);
    eprintln!(
        "catalog-admit{}: added={} updated={} reverified={} removed={} rejected={} deferred={} failures={} complete={}",
        if args.dry_run { " dry-run" } else { "" },
        outcome.added.len(),
        outcome.updated.len(),
        outcome.reverified.len(),
        outcome.removed.len(),
        outcome.rejected.len(),
        outcome.deferred.len(),
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
                "catalog-admit failed: {}",
                strip_control(&error.to_string())
            );
            ExitCode::FAILURE
        }
    }
}
