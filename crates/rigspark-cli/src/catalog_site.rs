use clap::Parser;
use rigspark_cli::maintenance_file::File;
use rigspark_core::{
    catalog::Catalog,
    generation::GenerationCatalog,
    reports::strip_control,
    site_latest::{latest_script, stamp_catalog_counts},
};
use std::{error::Error, path::PathBuf, process::ExitCode};

#[derive(Parser)]
#[command(
    about = "Write site/data/latest.js (models released or auto-added in the last month) and stamp catalog counts into site/index.html"
)]
struct Args {
    #[arg(long, default_value = "crates/rigspark-core/data/models.json")]
    catalog_path: PathBuf,
    #[arg(long, default_value = "crates/rigspark-core/data/generation.json")]
    generation_path: PathBuf,
    #[arg(long, default_value = "site/data/latest.js")]
    output: PathBuf,
    #[arg(long, default_value = "site/index.html")]
    site: PathBuf,
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
    let site = File::open(&args.site)?;
    let page = String::from_utf8(site.read()?)?;
    let stamped = stamp_catalog_counts(&page, &text)?;
    let mut stale = false;
    for (path, file, expected) in [
        (&args.output, File::open(&args.output)?, script),
        (&args.site, site, stamped),
    ] {
        let current = file.read_optional()?;
        if current.as_deref() == Some(expected.as_bytes()) {
            continue;
        }
        if args.check {
            eprintln!(
                "catalog-site: {} is stale; run `cargo catalog-site`",
                path.display()
            );
            stale = true;
            continue;
        }
        file.write_bytes(expected.as_bytes(), current.is_some())?;
        eprintln!("catalog-site: wrote {}", path.display());
    }
    Ok(u8::from(stale))
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
