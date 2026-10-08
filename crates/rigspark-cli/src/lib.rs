mod terminal_events;
use rigspark_core::sizing::{SizingRequest, evaluate};
use serde::Serialize;
use std::io::{Read, Write};

pub mod accessible;
pub mod accessible_catalog;
pub mod accessible_installed;
pub mod accessible_read_only;
pub mod accessible_recommend;
pub mod accessible_text;
pub mod cancellation;
pub mod dialog_smoke;
pub mod distribution;
pub mod gui_launcher;
pub mod maintenance_file;
pub mod performance;
pub mod retirement;
pub mod terminal;
pub mod tui_chat;
pub mod tui_generate;
pub mod tui_lifecycle;
pub mod tui_mode;
pub mod tui_models;
mod tui_theme;
pub mod tui_view;

pub const MAX_INPUT_BYTES: usize = 8 * 1024 * 1024;
const MAX_BATCH: usize = 4096;

#[derive(Debug, Serialize)]
pub struct RunnerError {
    pub code: &'static str,
    pub message: String,
}

impl RunnerError {
    fn new(code: &'static str, message: impl ToString) -> Self {
        Self {
            code,
            message: message.to_string(),
        }
    }
}

pub fn run(input: impl Read, mut output: impl Write) -> Result<(), RunnerError> {
    let mut bytes = Vec::new();
    input
        .take((MAX_INPUT_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|error| RunnerError::new("IO_ERROR", error))?;
    if bytes.len() > MAX_INPUT_BYTES {
        return Err(RunnerError::new(
            "VALIDATION_ERROR",
            "input exceeds 8 MiB limit",
        ));
    }
    let values: Vec<serde_json::Value> = serde_json::from_slice(&bytes)
        .map_err(|error| RunnerError::new("VALIDATION_ERROR", error))?;
    if values.len() > MAX_BATCH {
        return Err(RunnerError::new(
            "VALIDATION_ERROR",
            "batch exceeds 4096 requests",
        ));
    }
    let mut results = Vec::with_capacity(values.len());
    for value in values {
        let request: SizingRequest = serde_json::from_value(value)
            .map_err(|error| RunnerError::new("VALIDATION_ERROR", error))?;
        results
            .push(evaluate(&request).map_err(|error| RunnerError::new("VALIDATION_ERROR", error))?);
    }
    let encoded = serde_json::to_vec(&results)
        .map_err(|error| RunnerError::new("SERIALIZATION_ERROR", error))?;
    output
        .write_all(&encoded)
        .and_then(|()| output.write_all(b"\n"))
        .map_err(|error| RunnerError::new("IO_ERROR", error))
}
