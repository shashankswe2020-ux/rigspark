//! Presentation and selection for `rigspark add`, kept pure so it can be tested
//! without network or hardware.

use rigspark_core::{catalog::CatalogModel, reports::strip_control, sizing::Hardware};
use rigspark_runtime::hf_add::{best_fit, fits};

#[derive(Debug)]
pub enum Selection<'a> {
    Chosen(&'a CatalogModel),
    /// The named quantization does not fit; adding it needs `--bypass`.
    TooLarge(&'a CatalogModel),
    NoneFits,
    Unknown(String),
}

/// The entry to add: the named quantization, else the largest that fits.
pub fn select<'a>(
    entries: &'a [CatalogModel],
    quant: Option<&str>,
    hardware: &Hardware,
    bypass: bool,
) -> Selection<'a> {
    match quant {
        Some(quant) => match entries
            .iter()
            .find(|model| model.quantizations[0].name.eq_ignore_ascii_case(quant))
        {
            None => Selection::Unknown(quant.to_ascii_uppercase()),
            Some(model) if bypass || fits(model, hardware) => Selection::Chosen(model),
            Some(model) => Selection::TooLarge(model),
        },
        None => best_fit(entries, hardware).map_or(Selection::NoneFits, Selection::Chosen),
    }
}

impl PartialEq for Selection<'_> {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Chosen(left), Self::Chosen(right))
            | (Self::TooLarge(left), Self::TooLarge(right)) => left.id == right.id,
            (Self::NoneFits, Self::NoneFits) => true,
            (Self::Unknown(left), Self::Unknown(right)) => left == right,
            _ => false,
        }
    }
}

/// One row per quantization: name, download size and whether it fits this machine.
pub fn choice_table(entries: &[CatalogModel], hardware: &Hardware, chosen: Option<&str>) -> String {
    let mut text = String::from("  Quant       Download   Fits this machine\n");
    for model in entries {
        let quant = &model.quantizations[0];
        let marker = if chosen == Some(model.id.as_str()) {
            "▸"
        } else {
            " "
        };
        text.push_str(&format!(
            "{marker} {:<11} {:>8.1} GB  {}\n",
            strip_control(&quant.name),
            quant.disk_bytes / 1e9,
            if fits(model, hardware) { "yes" } else { "no" }
        ));
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;
    use rigspark_core::sizing::{CpuArch, Platform};

    fn entries() -> Vec<CatalogModel> {
        let base = rigspark_core::catalog::Catalog::parse(rigspark_core::MODELS_JSON)
            .unwrap()
            .models
            .into_iter()
            .find(|model| model.id == "qwen2.5:0.5b")
            .unwrap();
        [("Q4_K_M", 4e8), ("Q8_0", 6e8)]
            .into_iter()
            .map(|(name, bytes)| {
                let mut model = base.clone();
                model.id = format!("hf.co/a/b:{}", name.to_ascii_lowercase());
                model.quantizations[0].name = name.into();
                model.quantizations[0].disk_bytes = bytes;
                model.quantizations[0].min_ram_bytes = bytes;
                model.quantizations[0].min_vram_bytes = bytes;
                model
            })
            .collect()
    }

    fn machine(gib: f64) -> Hardware {
        Hardware {
            arch: CpuArch::Arm64,
            platform: Platform::Darwin,
            total_ram_bytes: gib * 1073741824.0,
            free_ram_bytes: gib * 1073741824.0,
            free_disk_bytes: 500e9,
            gpu: vec![],
            unified_memory: Some(true),
        }
    }

    #[test]
    fn explicit_quants_are_honoured_or_refused_and_the_default_is_the_best_fit() {
        let all = entries();
        let roomy = machine(64.0);
        assert_eq!(
            select(&all, None, &roomy, false),
            Selection::Chosen(&all[1])
        );
        assert_eq!(
            select(&all, Some("q4_k_m"), &roomy, false),
            Selection::Chosen(&all[0])
        );
        assert_eq!(
            select(&all, Some("Q3_K_S"), &roomy, false),
            Selection::Unknown("Q3_K_S".into())
        );
        let tiny = machine(0.1);
        assert_eq!(select(&all, None, &tiny, false), Selection::NoneFits);
        assert_eq!(
            select(&all, Some("Q8_0"), &tiny, false),
            Selection::TooLarge(&all[1])
        );
        assert_eq!(
            select(&all, Some("Q8_0"), &tiny, true),
            Selection::Chosen(&all[1])
        );
    }

    #[test]
    fn the_table_marks_the_choice_and_fit() {
        let all = entries();
        let table = choice_table(&all, &machine(64.0), Some("hf.co/a/b:q8_0"));
        assert!(table.contains("▸ Q8_0"), "{table}");
        assert!(table.contains("  Q4_K_M"), "{table}");
        assert_eq!(table.matches("yes").count(), 2, "{table}");
    }
}
