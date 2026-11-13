//! Bounded document reading in a disposable process, never in an API worker's address space.
use super::dto::{MAX_CELL_BYTES, MAX_CELLS, MAX_INFLATED_BYTES, MAX_SOURCE_BYTES};
use crate::AppError;
use base64::{Engine, engine::general_purpose::STANDARD};
use calamine::{DataRef, Reader, Xlsx};
use serde::{Deserialize, Serialize};
use std::{
    io::{self, Cursor, Read},
    process::Stdio,
    time::Duration,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    sync::Semaphore,
};

const BATCH: usize = 500;
const OUTPUT_LIMIT: usize = 12 * 1024 * 1024;
static PARSER: Semaphore = Semaphore::const_new(1);

#[derive(Serialize, Deserialize, Default)]
pub struct Request {
    pub media_type: String,
    pub data: String,
    #[serde(default)]
    pub sheet_index: usize,
    #[serde(default)]
    pub after: u32,
    pub header_row: Option<u32>,
    #[serde(default)]
    pub pdf_from: u32,
    #[serde(default = "five")]
    pub pdf_count: u32,
}
fn five() -> u32 {
    5
}
#[derive(Serialize, Deserialize, Default)]
pub struct Row {
    pub line: u32,
    pub cells: Vec<String>,
    pub formula_columns: Vec<usize>,
}
#[derive(Serialize, Deserialize, Default)]
pub struct Output {
    pub sheet: Option<String>,
    pub columns: Vec<String>,
    pub rows: Vec<Row>,
    pub after: u32,
    pub next_sheet: usize,
    pub done: bool,
    pub total: Option<usize>,
    pub pdf: Option<String>,
    pub page_from: u32,
    pub page_count: u32,
    pub total_pages: u32,
}

/// No inherited secrets, shell, filenames or document-controlled addresses reach the child.
pub async fn read(request: Request) -> Result<Output, AppError> {
    let _permit = PARSER.acquire().await.map_err(|_| AppError::Overloaded)?;
    let bytes = serde_json::to_vec(&request).map_err(|e| AppError::Internal(e.to_string()))?;
    if bytes.len() > MAX_SOURCE_BYTES * 2 {
        return Err(AppError::PayloadTooLarge);
    }
    let executable = std::env::current_exe().map_err(|e| AppError::Internal(e.to_string()))?;
    // Test binaries do not implement the parser subcommand. Integration tests explicitly supply
    // the real built binary; production always uses current_exe.
    let executable = if cfg!(debug_assertions) {
        std::env::var_os("GENESY_PRODUCT_PARSER_BIN")
            .map(Into::into)
            .unwrap_or(executable)
    } else {
        executable
    };
    let mut child = tokio::process::Command::new(executable)
        .arg("parse-product-source")
        .env_clear()
        .kill_on_drop(true)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| AppError::Internal(format!("document parser: {e}")))?;
    let mut input = child.stdin.take().ok_or(AppError::Overloaded)?;
    let mut output = child.stdout.take().ok_or(AppError::Overloaded)?;
    let process = async {
        let writer = async {
            input.write_all(&bytes).await?;
            input.shutdown().await?;
            // Child reads the complete request before parsing; shutdown alone does not close this pipe.
            drop(input);
            Ok::<_, io::Error>(())
        };
        let reader = async {
            let mut bytes = Vec::new();
            (&mut output)
                .take((OUTPUT_LIMIT + 1) as u64)
                .read_to_end(&mut bytes)
                .await?;
            Ok::<_, io::Error>(bytes)
        };
        let (_, bytes) = tokio::try_join!(writer, reader).map_err(|_| {
            AppError::Unprocessable("Document parser stopped before completing this part.".into())
        })?;
        if bytes.len() > OUTPUT_LIMIT {
            return Err(AppError::PayloadTooLarge);
        }
        let status = child
            .wait()
            .await
            .map_err(|e| AppError::Internal(e.to_string()))?;
        let response: serde_json::Value = serde_json::from_slice(&bytes).map_err(|_| {
            AppError::Unprocessable(
                "Document exceeds parser memory/time limits or cannot be read.".into(),
            )
        })?;
        if !status.success() {
            return Err(AppError::Unprocessable(
                response["error"]
                    .as_str()
                    .unwrap_or("Document exceeds parser memory/time limits or cannot be read.")
                    .to_owned(),
            ));
        }
        serde_json::from_value(response).map_err(|_| {
            AppError::Unprocessable("Document parser returned incomplete data.".into())
        })
    };
    tokio::time::timeout(Duration::from_secs(60), process)
        .await
        .map_err(|_| {
            AppError::Unprocessable("Document parsing exceeded 60 seconds. Split the file.".into())
        })?
}

/// Runs before Tokio, configuration, database connections or credential loading.
pub fn child_main() -> i32 {
    #[cfg(target_os = "linux")]
    if rlimit::Resource::AS
        .set(256 * 1024 * 1024, 256 * 1024 * 1024)
        .is_err()
        || rlimit::Resource::CPU.set(30, 30).is_err()
        || rlimit::Resource::CORE.set(0, 0).is_err()
    {
        return 2;
    }
    let result = (|| {
        let mut input = Vec::new();
        io::stdin()
            .take((MAX_SOURCE_BYTES * 2 + 1) as u64)
            .read_to_end(&mut input)
            .map_err(|_| "Cannot read source.")?;
        if input.len() > MAX_SOURCE_BYTES * 2 {
            return Err("Source exceeds 10 MiB.");
        }
        let request: Request =
            serde_json::from_slice(&input).map_err(|_| "Invalid parser request.")?;
        parse(&request)
    })();
    let (value, code) = match result {
        Ok(output) => (serde_json::to_value(output).unwrap_or_default(), 0),
        Err(message) => (serde_json::json!({"error":message}), 1),
    };
    if serde_json::to_writer(io::stdout().lock(), &value).is_err() {
        return 2;
    }
    code
}

pub(super) fn parse(request: &Request) -> Result<Output, &'static str> {
    let bytes = STANDARD
        .decode(&request.data)
        .map_err(|_| "Invalid source bytes.")?;
    if bytes.len() > MAX_SOURCE_BYTES {
        return Err("Sources must not exceed 10 MiB.");
    }
    match request.media_type.as_str() {
        "text/csv" => csv(&bytes, request),
        "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet" => {
            xlsx(&bytes, request)
        }
        "application/pdf" => pdf(&bytes, request),
        _ => Err("Use XLSX, UTF-8 CSV or PDF."),
    }
}

fn check_cells(cells: &[String]) -> Result<(), &'static str> {
    if cells.len() > 256 {
        return Err("Sources may have at most 256 columns per table.");
    }
    if cells
        .iter()
        .any(|v| v.len() > MAX_CELL_BYTES || v.contains('\0'))
    {
        return Err("A source cell exceeds 16 KiB or contains a NUL character.");
    }
    Ok(())
}
fn letters(index: usize) -> String {
    let mut index = index + 1;
    let mut text = String::new();
    while index > 0 {
        index -= 1;
        text.insert(0, char::from(b'A' + (index % 26) as u8));
        index /= 26;
    }
    text
}
fn collect(
    rows: impl Iterator<Item = Result<Row, &'static str>>,
    request: &Request,
    sheet: Option<String>,
    has_next: bool,
) -> Result<Output, &'static str> {
    let mut result = Output {
        sheet,
        next_sheet: request.sheet_index,
        ..Default::default()
    };
    let mut cells = 0usize;
    for row in rows {
        let row = row?;
        check_cells(&row.cells)?;
        cells = cells
            .checked_add(row.cells.iter().filter(|c| !c.is_empty()).count())
            .ok_or("Too many cells.")?;
        if cells > MAX_CELLS {
            return Err("Source exceeds two million populated cells.");
        }
        if request.header_row == Some(row.line) {
            result.columns = row.cells.clone();
        }
        if row.line <= request.after
            || request.header_row.is_some_and(|h| row.line <= h)
            || row.formula_columns.is_empty() && row.cells.iter().all(|s| s.trim().is_empty())
        {
            continue;
        }
        if result.rows.len() == BATCH {
            result.done = false;
            if result.columns.is_empty() {
                let width = result.rows.iter().map(|r| r.cells.len()).max().unwrap_or(0);
                result.columns = (0..width).map(letters).collect();
            }
            return Ok(result);
        }
        result.after = row.line;
        result.rows.push(row);
    }
    result.next_sheet = request.sheet_index + 1;
    result.done = !has_next;
    if result.columns.is_empty() {
        let width = result.rows.iter().map(|r| r.cells.len()).max().unwrap_or(0);
        result.columns = (0..width).map(letters).collect();
    }
    Ok(result)
}
fn csv(bytes: &[u8], request: &Request) -> Result<Output, &'static str> {
    let text = std::str::from_utf8(bytes)
        .map_err(|_| "Save CSV as UTF-8.")?
        .trim_start_matches('\u{feff}');
    let mut quoted = false;
    let mut counts = [0usize; 3];
    let mut chars = text.bytes().peekable();
    while let Some(c) = chars.next() {
        if c == b'"' {
            if quoted && chars.peek() == Some(&b'"') {
                chars.next();
            } else {
                quoted = !quoted;
            }
        } else if !quoted {
            if c == b'\n' || c == b'\r' {
                break;
            }
            if let Some(index) = b",;\t".iter().position(|v| *v == c) {
                counts[index] += 1;
            }
        }
    }
    let delimiter = b",;\t"[counts
        .iter()
        .enumerate()
        .max_by_key(|(_, n)| *n)
        .map(|(i, _)| i)
        .unwrap_or(0)];
    let mut reader = ::csv::ReaderBuilder::new()
        .has_headers(false)
        .flexible(true)
        .delimiter(delimiter)
        .from_reader(text.as_bytes());
    let rows = reader.records().enumerate().map(|(index, row)| {
        let row = row.map_err(|_| "CSV has an invalid quoted record.")?;
        let line = row
            .position()
            .map(|p| p.line())
            .unwrap_or((index + 1) as u64);
        Ok(Row {
            line: u32::try_from(line).map_err(|_| "CSV line number is too large.")?,
            cells: row.iter().map(str::to_owned).collect(),
            formula_columns: vec![],
        })
    });
    collect(rows, request, None, false)
}

fn preflight(bytes: &[u8]) -> Result<(), &'static str> {
    let mut archive =
        zip::ZipArchive::new(Cursor::new(bytes)).map_err(|_| "Unreadable XLSX archive.")?;
    if archive.len() > 1024 {
        return Err("XLSX has too many archive entries.");
    }
    let mut total = 0u64;
    let mut inflated = 0u64;
    for index in 0..archive.len() {
        let file = archive
            .by_index(index)
            .map_err(|_| "Unreadable XLSX archive entry.")?;
        if file.encrypted() || file.enclosed_name().is_none() {
            return Err("Encrypted or invalid XLSX archive.");
        }
        total = total
            .checked_add(file.size())
            .ok_or("XLSX expansion limit exceeded.")?;
        if total > MAX_INFLATED_BYTES as u64 {
            return Err("XLSX expands beyond 128 MiB.");
        }
        let remaining = (MAX_INFLATED_BYTES as u64).saturating_sub(inflated);
        let read = io::copy(&mut file.take(remaining + 1), &mut io::sink())
            .map_err(|_| "Unreadable XLSX archive entry.")?;
        if read > remaining {
            return Err("XLSX expansion limit exceeded.");
        }
        inflated += read;
    }
    Ok(())
}
fn xlsx(bytes: &[u8], request: &Request) -> Result<Output, &'static str> {
    preflight(bytes)?;
    let mut book: Xlsx<_> =
        Xlsx::new(Cursor::new(bytes)).map_err(|_| "Cannot read XLSX workbook.")?;
    let sheets = book.sheet_names().to_vec();
    let name = sheets
        .get(request.sheet_index)
        .ok_or("Worksheet does not exist.")?
        .clone();
    let mut reader = book
        .worksheet_cells_reader(&name)
        .map_err(|_| "Cannot read worksheet.")?;
    let padding: std::collections::HashMap<usize, usize> = reader
        .styles()
        .iter()
        .enumerate()
        .filter_map(|(id, style)| {
            let code = &style.number_format.as_ref()?.format_code;
            (code.len() > 1 && code.len() <= 256 && code.bytes().all(|c| c == b'0'))
                .then_some((id, code.len()))
        })
        .collect();
    let mut rows = Vec::new();
    let mut current = Row::default();
    let mut cells_seen = 0usize;
    loop {
        let cell = reader
            .next_cell_with_formula()
            .map_err(|_| "Cannot read worksheet cell.")?;
        let Some(cell) = cell else { break };
        if matches!(&cell.value, DataRef::Empty) && cell.formula.is_none() {
            continue;
        }
        let (r, c) = cell.pos;
        let line = r + 1;
        let column = c as usize;
        if column >= 256 {
            return Err("Sources may have at most 256 columns per table.");
        }
        if current.line != 0 && current.line != line {
            let completed = std::mem::take(&mut current);
            if completed.line > request.after || request.header_row == Some(completed.line) {
                rows.push(completed);
            }
            if rows
                .iter()
                .filter(|v| {
                    v.line > request.after
                        && request.header_row.is_none_or(|h| v.line > h)
                        && (!v.formula_columns.is_empty()
                            || v.cells.iter().any(|c| !c.trim().is_empty()))
                })
                .count()
                > BATCH
            {
                break;
            }
        }
        current.line = line;
        current
            .cells
            .resize(current.cells.len().max(column + 1), String::new());
        let value = &cell.value;
        current.cells[column] = match value {
            DataRef::DateTime(value) => value
                .as_datetime()
                .map(|d| d.format("%Y-%m-%dT%H:%M:%S").to_string())
                .ok_or("Unreadable Excel date.")?,
            DataRef::Error(_) => {
                return Err("Worksheet contains a cell error. Correct it before importing.");
            }
            DataRef::Int(value) => value.to_string(),
            DataRef::Float(value) if value.is_finite() => value.to_string(),
            DataRef::Float(_) => return Err("Worksheet contains an invalid numeric value."),
            DataRef::Bool(value) => value.to_string(),
            DataRef::String(value) | DataRef::DateTimeIso(value) | DataRef::DurationIso(value) => {
                value.clone()
            }
            DataRef::SharedString(value) => (*value).to_owned(),
            DataRef::Empty => String::new(),
        };
        if !current.cells[column].is_empty() {
            cells_seen += 1;
        }
        if cells_seen > MAX_CELLS {
            return Err("Source exceeds two million populated cells.");
        }
        if cell.formula.is_some() {
            current.formula_columns.push(column);
        }
    }
    if current.line != 0
        && (current.line > request.after || request.header_row == Some(current.line))
    {
        rows.push(current);
    }
    drop(reader);
    if !padding.is_empty() && !rows.is_empty() {
        // Formula metadata and style IDs use separate streaming APIs; reread only for zero-padding formats.
        let mut styled = book
            .worksheet_cells_reader(&name)
            .map_err(|_| "Cannot read worksheet styles.")?;
        let last = rows.last().map_or(0, |r| r.line);
        while let Some((cell, style)) = styled
            .next_cell_with_style_id()
            .map_err(|_| "Cannot read worksheet styles.")?
        {
            let line = cell.get_position().0 + 1;
            if line > last {
                break;
            }
            let Some(width) = padding.get(&style) else {
                continue;
            };
            let Ok(index) = rows.binary_search_by_key(&line, |r| r.line) else {
                continue;
            };
            if matches!(cell.get_value(), DataRef::Int(_) | DataRef::Float(_)) {
                let column = cell.get_position().1 as usize;
                let value = &mut rows[index].cells[column];
                if !value.bytes().all(|c| c.is_ascii_digit()) {
                    return Err(
                        "A zero-padded numeric cell would require rounding. Store the intended identifier as text.",
                    );
                }
                if value.len() < *width {
                    *value = format!("{value:0>width$}", width = width);
                }
            }
        }
    }
    collect(
        rows.into_iter().map(Ok),
        request,
        Some(name),
        request.sheet_index + 1 < sheets.len(),
    )
}
fn pdf(bytes: &[u8], request: &Request) -> Result<Output, &'static str> {
    if !bytes.starts_with(b"%PDF-") {
        return Err("The file is not a PDF.");
    }
    let mut document = lopdf::Document::load_mem_with_options(
        bytes,
        lopdf::LoadOptions::with_max_decompressed_size(64 * 1024 * 1024),
    )
    .map_err(|_| "Cannot read PDF within decoding limits.")?;
    if document.is_encrypted() || document.was_encrypted() {
        return Err("Remove PDF password/encryption before importing.");
    }
    let pages = document.get_pages();
    let total = pages.len() as u32;
    if total == 0 || total > 600 {
        return Err("PDF must contain 1 through 600 pages.");
    }
    let from = request.pdf_from.max(1);
    if from > total {
        return Err("PDF page cursor is outside the document.");
    }
    let count = request.pdf_count.clamp(1, 5).min(total - from + 1);
    let end = from + count - 1;
    let context_from = from.saturating_sub(1).max(1);
    let context_to = (end + 1).min(total);
    let remove: Vec<u32> = pages
        .keys()
        .copied()
        .filter(|p| *p < context_from || *p > context_to)
        .collect();
    document.delete_pages(&remove);
    document.prune_objects();
    let mut output = Vec::new();
    document
        .save_to(&mut output)
        .map_err(|_| "Cannot prepare PDF page range.")?;
    if output.len() > MAX_SOURCE_BYTES {
        return Err("PDF page range exceeds 10 MiB.");
    }
    Ok(Output {
        pdf: Some(STANDARD.encode(output)),
        page_from: from,
        page_count: count,
        total_pages: total,
        done: end == total,
        ..Default::default()
    })
}

/// Only named, reviewed transforms run; no model-supplied code or implicit locale guessing.
pub fn transform_cell(
    text: &str,
    transform: &serde_json::Value,
) -> Result<serde_json::Value, AppError> {
    use rust_decimal::{Decimal, RoundingStrategy};
    use serde_json::json;
    let invalid = || {
        AppError::field(
            "transform",
            "Choose a supported explicit transform and valid source value.",
        )
    };
    let kind = transform["kind"].as_str().ok_or_else(invalid)?;
    let decimal = || -> Result<Decimal, AppError> {
        let mut text = text.trim().to_owned();
        if let Some(prefix) = transform["prefix"].as_str() {
            text = text
                .strip_prefix(prefix)
                .ok_or_else(invalid)?
                .trim()
                .to_owned();
        }
        if let Some(suffix) = transform["suffix"].as_str() {
            text = text
                .strip_suffix(suffix)
                .ok_or_else(invalid)?
                .trim()
                .to_owned();
        }
        let separator = transform["decimal_separator"].as_str().unwrap_or(".");
        let group = transform["group_separator"].as_str().unwrap_or("");
        if ![".", ","].contains(&separator)
            || !["", ",", ".", " "].contains(&group)
            || separator == group
        {
            return Err(invalid());
        }
        let parts: Vec<_> = text.split(separator).collect();
        if parts.len() > 2
            || parts
                .get(1)
                .is_some_and(|v| v.is_empty() || !v.bytes().all(|b| b.is_ascii_digit()))
        {
            return Err(invalid());
        }
        let whole = parts[0].strip_prefix('-').unwrap_or(parts[0]);
        if !group.is_empty() && whole.contains(group) {
            let chunks: Vec<_> = whole.split(group).collect();
            if chunks[0].is_empty()
                || chunks[0].len() > 3
                || chunks[1..].iter().any(|c| c.len() != 3)
                || chunks
                    .iter()
                    .any(|c| !c.bytes().all(|b| b.is_ascii_digit()))
            {
                return Err(invalid());
            }
        } else if whole.is_empty() || !whole.bytes().all(|b| b.is_ascii_digit()) {
            return Err(invalid());
        }
        if !group.is_empty() {
            text = text.replace(group, "");
        }
        text = text.replace(separator, ".");
        text.parse().map_err(|_| invalid())
    };
    match kind {
        "text" => Ok(json!(text)),
        "decimal" | "number" => Ok(json!(decimal()?.normalize().to_string())),
        "date" => {
            let format = transform["format"].as_str().ok_or_else(invalid)?;
            if ![
                "%Y-%m-%d",
                "%d/%m/%Y",
                "%m/%d/%Y",
                "%d-%m-%Y",
                "%Y-%m-%dT%H:%M:%S",
            ]
            .contains(&format)
            {
                return Err(invalid());
            }
            let date = if format == "%Y-%m-%dT%H:%M:%S" {
                chrono::NaiveDateTime::parse_from_str(text.trim(), format)
                    .map_err(|_| invalid())?
                    .date()
            } else {
                chrono::NaiveDate::parse_from_str(text.trim(), format).map_err(|_| invalid())?
            };
            Ok(json!(date.format("%Y-%m-%d").to_string()))
        }
        "boolean" => {
            let yes = transform["true_values"].as_array().ok_or_else(invalid)?;
            let no = transform["false_values"].as_array().ok_or_else(invalid)?;
            let yes = yes.iter().any(|v| v.as_str() == Some(text.trim()));
            let no = no.iter().any(|v| v.as_str() == Some(text.trim()));
            if yes == no {
                return Err(invalid());
            }
            Ok(json!(yes))
        }
        "map" => transform["values"].get(text).cloned().ok_or_else(invalid),
        "split" => {
            let separator = transform["separator"]
                .as_str()
                .filter(|v| !v.is_empty() && v.len() <= 4)
                .ok_or_else(invalid)?;
            let values: Vec<_> = text.split(separator).map(str::trim).collect();
            if values.len() > 100 {
                return Err(invalid());
            }
            Ok(json!(values))
        }
        "scale" => {
            let factor: Decimal = transform["factor"]
                .as_str()
                .ok_or_else(invalid)?
                .parse()
                .map_err(|_| invalid())?;
            Ok(json!(
                decimal()?
                    .checked_mul(factor)
                    .ok_or_else(invalid)?
                    .normalize()
                    .to_string()
            ))
        }
        "net_price" => {
            let rate: Decimal = transform["tax_rate"]
                .as_str()
                .ok_or_else(invalid)?
                .parse()
                .map_err(|_| invalid())?;
            if rate < Decimal::ZERO || rate > Decimal::from(100) {
                return Err(invalid());
            }
            let scale = transform["scale"]
                .as_u64()
                .filter(|v| *v <= 4)
                .ok_or_else(invalid)? as u32;
            let divisor = Decimal::ONE
                .checked_add(rate / Decimal::from(100))
                .ok_or_else(invalid)?;
            let value = decimal()?
                .checked_div(divisor)
                .ok_or_else(invalid)?
                .round_dp_with_strategy(scale, RoundingStrategy::MidpointAwayFromZero);
            Ok(json!(value.normalize().to_string()))
        }
        _ => Err(invalid()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn csv_request(text: &str) -> Request {
        Request {
            media_type: "text/csv".into(),
            data: STANDARD.encode(text),
            ..Default::default()
        }
    }
    #[test]
    fn quoted_csv_preserves_codes_and_multiline_text() {
        let output = parse(&csv_request("code;name;price\n0012;\"Two\nlines\";12,50\n")).unwrap();
        assert_eq!(output.rows[1].cells, ["0012", "Two\nlines", "12,50"]);
        assert_eq!(output.rows[1].line, 2);
    }
    #[test]
    fn csv_pages_without_dropping_or_repeating_boundary_rows() {
        let text = (1..=1001)
            .map(|n| format!("{n},item{n}\n"))
            .collect::<String>();
        let mut request = csv_request(&text);
        let first = parse(&request).unwrap();
        assert_eq!(first.rows.len(), 500);
        assert!(!first.done);
        request.after = first.after;
        let next = parse(&request).unwrap();
        assert_eq!(next.rows[0].cells[0], "501");
    }
    #[test]
    fn cell_limits_fail_without_truncation() {
        assert!(parse(&csv_request(&"x".repeat(MAX_CELL_BYTES + 1))).is_err());
        assert!(
            parse(&Request {
                media_type: "application/pdf".into(),
                data: STANDARD.encode(b"not a pdf"),
                ..Default::default()
            })
            .is_err()
        );
    }
    #[test]
    fn transforms_use_explicit_locale_and_never_guess() {
        use serde_json::json;
        assert_eq!(transform_cell("€ 1.234,56",&json!({"kind":"decimal","prefix":"€","decimal_separator":",","group_separator":"."})).unwrap(),json!("1234.56"));
        assert!(
            transform_cell("1,23.45", &json!({"kind":"decimal","group_separator":","})).is_err()
        );
        assert_eq!(
            transform_cell(
                "121",
                &json!({"kind":"net_price","tax_rate":"21","scale":2})
            )
            .unwrap(),
            json!("100")
        );
        assert_eq!(
            transform_cell("31/12/2026", &json!({"kind":"date","format":"%d/%m/%Y"})).unwrap(),
            json!("2026-12-31")
        );
        assert!(
            transform_cell(
                "yes",
                &json!({"kind":"boolean","true_values":["yes"],"false_values":["yes"]})
            )
            .is_err()
        );
    }
}
