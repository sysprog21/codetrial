//! The `+obs-trace` file of the course's Verilator harness, read into the
//! table the page shows: every probe field, cycle by cycle, over a window.
//! The layout is the course's `docs/observation.md`, "Trace file".

use serde::Serialize;
use serde_json::Value;

const MAGIC: &[u8] = b"OBSEND01";

/// The window of a trace as the page shows it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Table {
    pub first: u64,
    pub fields: Vec<String>,
    /// One row per cycle from `first`, one value per field.
    pub rows: Vec<Vec<u32>>,
    /// The trailer's `end`: cycles, instret and how the run ended.
    pub end: Value,
}

struct Field {
    name: String,
    offset: usize,
    bytes: usize,
}

/// Read a whole trace. A file without its trailer was cut off, and so does a
/// file whose record count is not the trailer's: both are refused.
pub fn read(data: &[u8]) -> Result<Table, String> {
    let newline = data
        .iter()
        .position(|&b| b == b'\n')
        .ok_or("the trace has no header")?;
    let header: Value =
        serde_json::from_slice(&data[..newline]).map_err(|e| format!("the header: {e}"))?;
    let trailer = trailer(data)?;
    let record_bytes = header["record_bytes"]
        .as_u64()
        .and_then(|n| usize::try_from(n).ok())
        .filter(|&n| n > 0)
        .ok_or("the header has no record size")?;
    let fields = fields(&header, record_bytes)?;
    let body_end = data.len() - MAGIC.len() - 8 - trailer.1;
    let body = data
        .get(newline + 1..body_end)
        .ok_or("the trace is shorter than its trailer")?;
    let records = trailer.0["records"]
        .as_u64()
        .ok_or("the trailer has no record count")?;
    if body.len() % record_bytes != 0
        || u64::try_from(body.len() / record_bytes).ok() != Some(records)
    {
        return Err("the record count is not the trailer's".to_owned());
    }
    let rows = body
        .chunks_exact(record_bytes)
        .map(|record| fields.iter().map(|f| value(record, f)).collect())
        .collect();
    Ok(Table {
        first: header["first"]
            .as_u64()
            .ok_or("the header has no first cycle")?,
        fields: fields.into_iter().map(|f| f.name).collect(),
        rows,
        end: trailer.0["end"].clone(),
    })
}

/// The trailer object and its length in bytes.
fn trailer(data: &[u8]) -> Result<(Value, usize), String> {
    let cut = || "the trace was cut off: it has no trailer".to_owned();
    // The trace ends with the trailer, its length and the end marker.
    let (before, length) = data
        .strip_suffix(MAGIC)
        .and_then(<[u8]>::split_last_chunk::<8>)
        .ok_or_else(cut)?;
    let length = usize::try_from(u64::from_le_bytes(*length)).map_err(|_| cut())?;
    let start = before.len().checked_sub(length).ok_or_else(cut)?;
    let object = serde_json::from_slice(&before[start..]).map_err(|_| cut())?;
    Ok((object, length))
}

fn fields(header: &Value, record_bytes: usize) -> Result<Vec<Field>, String> {
    let list = header["fields"]
        .as_array()
        .ok_or("the header has no fields")?;
    let mut out = Vec::with_capacity(list.len());
    for field in list {
        let name = field["name"].as_str().ok_or("a field has no name")?;
        let offset = field["offset"]
            .as_u64()
            .and_then(|n| usize::try_from(n).ok());
        let bytes = field["bytes"]
            .as_u64()
            .and_then(|n| usize::try_from(n).ok());
        let (Some(offset), Some(bytes @ (1 | 2 | 4))) = (offset, bytes) else {
            return Err(format!("field {name}: no offset, or not 1, 2 or 4 bytes"));
        };
        if offset
            .checked_add(bytes)
            .is_none_or(|end| end > record_bytes)
        {
            return Err(format!("field {name} lies outside the record"));
        }
        out.push(Field {
            name: name.to_owned(),
            offset,
            bytes,
        });
    }
    Ok(out)
}

/// A field's little-endian value, of its 1, 2 or 4 bytes.
fn value(record: &[u8], field: &Field) -> u32 {
    let mut bytes = [0_u8; 4];
    bytes[..field.bytes].copy_from_slice(&record[field.offset..field.offset + field.bytes]);
    u32::from_le_bytes(bytes)
}

/// The retire records of a version-2 retire log, with the cycle of each:
/// a `pc=` line is a record in a cycle of its own, `idle <n>` is that many
/// cycles without one, and `mark` and `end` lines take no cycle.
fn retire_records(log: &str) -> Vec<(u64, &str)> {
    let mut cycle = 0_u64;
    let mut out = Vec::new();
    for line in log.lines() {
        if line.starts_with("pc=") {
            out.push((cycle, line));
            cycle += 1;
        } else if let Some(idle) = line.strip_prefix("idle ") {
            cycle = cycle.saturating_add(idle.trim().parse::<u64>().unwrap_or(0));
        }
    }
    out
}

/// Whether a retire log is whole: its last line is the harness's `end`
/// record for a run that stopped by itself, at an exit, a halt, an assertion
/// or `$finish`, and not one cut off by a signal, the cycle limit or the
/// display. Only a whole log can be compared, since a cut-off one would show
/// its end as a divergence.
pub(super) fn ended_by_itself(log: &str) -> bool {
    let Some(how) = log
        .lines()
        .rev()
        .find(|line| !line.trim().is_empty())
        .and_then(|line| line.strip_prefix("end "))
        .and_then(|line| line.split_whitespace().last())
    else {
        return false;
    };
    ["exit=", "halt="].iter().any(|kind| how.starts_with(kind))
        || matches!(how, "stop=assert" | "stop=finish")
}

/// The retire records of a log within `count` cycles from `first`.
pub fn retire_window(log: &str, first: u64, count: u64) -> Vec<(u64, String)> {
    retire_records(log)
        .into_iter()
        .filter(|(cycle, _)| (first..first.saturating_add(count)).contains(cycle))
        .map(|(cycle, line)| (cycle, line.to_owned()))
        .collect()
}

/// A log's retire records as (cycle, digest of the record): what a
/// divergence check compares, kept in place of the log itself.
pub(super) fn retire_digest(log: &str) -> Vec<(u64, u64)> {
    use std::hash::{BuildHasher, BuildHasherDefault, DefaultHasher};
    let hasher = BuildHasherDefault::<DefaultHasher>::default();
    retire_records(log)
        .into_iter()
        .map(|(cycle, line)| (cycle, hasher.hash_one(line)))
        .collect()
}

/// The cycle, in the bug model's records, of the first retire record in
/// which the two runs differ: the first instruction whose effect is not the
/// clean model's. `None` when every record agrees.
pub(super) fn first_divergence_in(clean: &[(u64, u64)], bug: &[(u64, u64)]) -> Option<u64> {
    for index in 0..clean.len().max(bug.len()) {
        match (clean.get(index), bug.get(index)) {
            (Some(a), Some(b)) if a.1 == b.1 => {}
            (_, Some(b)) => return Some(b.0),
            (Some(a), None) => return Some(a.0),
            (None, None) => break,
        }
    }
    None
}

/// `first_divergence_in` of two whole logs.
pub fn first_divergence(clean: &str, bug: &str) -> Option<u64> {
    first_divergence_in(&retire_digest(clean), &retire_digest(bug))
}
