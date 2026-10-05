use clap::{Parser, ValueEnum};
use rigspark_core::{
    generation::{GenerationCatalog, GenerationKind, GenerationModel, enrich_generation_catalog},
    reports::strip_control,
};
use rigspark_runtime::secure_fs::Directory;
use std::{
    error::Error,
    io,
    path::{Path, PathBuf},
    process::ExitCode,
};

#[derive(Clone, Copy, ValueEnum)]
enum Kind {
    Image,
    Video,
}
impl From<Kind> for GenerationKind {
    fn from(value: Kind) -> Self {
        match value {
            Kind::Image => Self::Image,
            Kind::Video => Self::Video,
        }
    }
}

#[derive(Parser)]
#[command(about = "Apply reviewed Hugging Face metadata to the image/video catalog")]
struct Args {
    #[arg(value_enum)]
    kind: Kind,
    #[arg(long, default_value = "crates/rigspark-core/data/generation.json")]
    catalog_path: PathBuf,
    #[arg(long)]
    candidates: PathBuf,
    #[arg(long)]
    report: PathBuf,
    #[arg(long)]
    now: String,
    #[arg(long)]
    dry_run: bool,
}

fn read(path: &Path, maximum: usize) -> io::Result<Vec<u8>> {
    let path = std::path::absolute(path)?;
    let parent = path
        .parent()
        .ok_or_else(|| io::Error::other("file parent required"))?
        .canonicalize()?;
    let name = Path::new(
        path.file_name()
            .ok_or_else(|| io::Error::other("filename required"))?,
    );
    Directory::open(&parent)?.read(name, maximum as u64, false)
}

fn write(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let path = std::path::absolute(path)?;
    let exists = path.try_exists()?;
    let parent = path
        .parent()
        .ok_or_else(|| io::Error::other("file parent required"))?;
    std::fs::create_dir_all(parent)?;
    let parent = parent.canonicalize()?;
    let name = Path::new(
        path.file_name()
            .ok_or_else(|| io::Error::other("filename required"))?,
    );
    Directory::open(&parent)?.write(name, bytes, !exists, exists)
}

fn run(args: Args) -> Result<(), Box<dyn Error>> {
    let before = read(&args.catalog_path, 16 * 1024 * 1024)?;
    let catalog = GenerationCatalog::parse(std::str::from_utf8(&before)?)?;
    let candidates: Vec<GenerationModel> =
        serde_json::from_slice(&read(&args.candidates, 16 * 1024 * 1024)?)?;
    let result = enrich_generation_catalog(&catalog, candidates, args.kind.into(), &args.now)?;
    let report = format!("{}\n", serde_json::to_string_pretty(&result)?);
    write(&args.report, report.as_bytes())?;
    if !args.dry_run && !result.updated.is_empty() {
        if read(&args.catalog_path, 16 * 1024 * 1024)? != before {
            return Err(io::Error::other("generation catalog changed during enrichment").into());
        }
        let encoded = format!("{}\n", serde_json::to_string_pretty(&result.catalog)?);
        write(&args.catalog_path, encoded.as_bytes())?;
    }
    eprintln!(
        "generation-enrich: updated={} rejected={}",
        result.updated.len(),
        result.rejected.len()
    );
    Ok(())
}

fn main() -> ExitCode {
    match run(Args::parse()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!(
                "generation-enrich failed: {}",
                strip_control(&error.to_string())
            );
            ExitCode::FAILURE
        }
    }
}
