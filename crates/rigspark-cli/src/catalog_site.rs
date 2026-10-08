use clap::Parser;
use rigspark_cli::maintenance_file::File;
use rigspark_core::{
    catalog::Catalog, generation::GenerationCatalog, reports::strip_control,
    site_latest::latest_script,
};
use std::{error::Error, path::PathBuf, process::ExitCode};

#[derive(Parser)]
#[command(about = "Write site/data/latest.js: models released or auto-added in the last month")]
struct Args {
    #[arg(long, default_value = "crates/rigspark-core/data/models.json")]
    catalog_path: PathBuf,
    #[arg(long, default_value = "crates/rigspark-core/data/generation.json")]
    generation_path: PathBuf,
    #[arg(long, default_value = "site/data/latest.js")]
    output: PathBuf,
    /// Exit non-zero instead of writing when the committed file is stale.
    #[arg(long)]
    check: bool,
}

fn run(args: Args) -> Result<u8, Box<dyn Error>> {
    let text = Catalog::parse(std::str::from_utf8(
        &File::open(&args.catalog_path)?.read()?,
    )?)?;
    let generation = GenerationCatalog::parse(std::str::from_utf8(
        &File::open(&args.generation_path)?.read()?,
    )?)?;
    let script = latest_script(&text, &generation)?;
    let output = File::open(&args.output)?;
    let current = output.read_optional()?;
    if current.as_deref() == Some(script.as_bytes()) {
        return Ok(0);
    }
    if args.check {
        eprintln!(
            "catalog-site: {} is stale; run `cargo catalog-site`",
            args.output.display()
        );
        return Ok(1);
    }
    output.write_bytes(script.as_bytes(), current.is_some())?;
    eprintln!("catalog-site: wrote {}", args.output.display());
    Ok(0)
}

fn main() -> ExitCode {
    match run(Args::parse()) {
        Ok(code) => ExitCode::from(code),
        Err(error) => {
            eprintln!("catalog-site failed: {}", strip_control(&error.to_string()));
            ExitCode::FAILURE
        }
    }
}
