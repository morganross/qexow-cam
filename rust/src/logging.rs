use crate::errors::CamError;
use crate::state::StateStore;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::VecDeque;
use std::fs::OpenOptions;
use std::fs::{self, File};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use time::{Duration, OffsetDateTime, format_description::well_known::Rfc3339};

#[derive(Debug, Clone)]
pub struct StructuredLogger {
    events_path: PathBuf,
    daemon_log_path: PathBuf,
}

impl StructuredLogger {
    pub fn new(store: &StateStore) -> Self {
        Self {
            events_path: store.events_path(),
            daemon_log_path: store.daemon_log_path(),
        }
    }

    pub fn event(
        &self,
        event_type: impl Into<String>,
        message: impl Into<String>,
        fields: Value,
    ) -> Result<(), CamError> {
        let event_type = event_type.into();
        let message = message.into();
        let _guard = logger_write_lock().lock().map_err(|_| {
            CamError::InvalidState("structured logger write lock was poisoned".to_string())
        })?;
        let _file_lock = acquire_log_pair_lock(&self.events_path, &self.daemon_log_path)?;
        let event = StoredLogEvent {
            at: next_event_timestamp(&self.events_path)?,
            event_type,
            message,
            fields,
        };
        append_json_line(&self.events_path, &event)?;
        append_json_line(&self.daemon_log_path, &event)?;
        Ok(())
    }

    pub fn command_started(&self, command: &str) -> Result<(), CamError> {
        self.event(
            "command.started",
            format!("command `{command}` started"),
            json!({ "command": command }),
        )
    }

    pub fn command_finished(&self, command: &str) -> Result<(), CamError> {
        self.event(
            "command.finished",
            format!("command `{command}` finished"),
            json!({ "command": command }),
        )
    }

    pub fn command_failed(&self, command: &str, error: &CamError) -> Result<(), CamError> {
        self.event(
            "command.failed",
            format!("command `{command}` failed"),
            json!({
                "command": command,
                "ok": false,
                "error_kind": error.kind(),
                "error": error.to_string(),
            }),
        )
    }
}

fn logger_write_lock() -> &'static Mutex<()> {
    static LOGGER_WRITE_LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOGGER_WRITE_LOCK.get_or_init(|| Mutex::new(()))
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct StoredLogEvent {
    pub at: String,
    pub event_type: String,
    pub message: String,
    pub fields: Value,
}

pub fn read_events(
    store: &StateStore,
    limit: Option<usize>,
) -> Result<Vec<StoredLogEvent>, CamError> {
    read_event_file_with_mirror_repair(&store.events_path(), &store.daemon_log_path(), limit)
}

pub fn read_daemon_log_events(
    store: &StateStore,
    limit: Option<usize>,
) -> Result<Vec<StoredLogEvent>, CamError> {
    read_event_file_with_mirror_repair(&store.daemon_log_path(), &store.events_path(), limit)
}

fn read_event_file_with_mirror_repair(
    primary_path: &Path,
    mirror_path: &Path,
    limit: Option<usize>,
) -> Result<Vec<StoredLogEvent>, CamError> {
    let _file_lock = acquire_log_pair_lock(primary_path, mirror_path)?;
    match read_event_file(primary_path, limit) {
        Ok(events) => Ok(events),
        Err(primary_error) => {
            repair_event_file_from_mirror(primary_path, mirror_path, &primary_error)?;
            read_event_file(primary_path, limit)
        }
    }
}

fn read_event_file(path: &Path, limit: Option<usize>) -> Result<Vec<StoredLogEvent>, CamError> {
    if limit == Some(0) {
        return Ok(Vec::new());
    }
    let file = std::fs::File::open(path)?;
    let reader = BufReader::new(file);
    let mut events = Vec::new();
    let mut limited_events = limit.map(VecDeque::with_capacity);

    for (index, line) in reader.lines().enumerate() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let event = serde_json::from_str(&line).map_err(|error| {
            CamError::InvalidState(format!(
                "invalid JSONL in {} at line {}: {error}",
                path.display(),
                index + 1
            ))
        })?;
        if let (Some(limited_events), Some(limit)) = (limited_events.as_mut(), limit) {
            if limited_events.len() == limit {
                limited_events.pop_front();
            }
            limited_events.push_back(event);
        } else {
            events.push(event);
        }
    }

    Ok(match limited_events {
        Some(limited_events) => limited_events.into_iter().collect(),
        None => events,
    })
}

fn next_event_timestamp(path: &Path) -> Result<String, CamError> {
    let now = OffsetDateTime::now_utc();
    let timestamp = match read_last_event_timestamp(path).ok().flatten() {
        Some(previous) if now <= previous => previous + Duration::nanoseconds(1),
        _ => now,
    };
    Ok(timestamp
        .format(&Rfc3339)
        .unwrap_or_else(|_| "1970-01-01T00:00:00Z".to_string()))
}

fn read_last_event_timestamp(path: &Path) -> Result<Option<OffsetDateTime>, CamError> {
    if !path.is_file() {
        return Ok(None);
    }

    let events = read_event_file(path, None)?;
    let Some(event) = events.last() else {
        return Ok(None);
    };
    let at = OffsetDateTime::parse(&event.at, &Rfc3339).map_err(|error| {
        CamError::InvalidState(format!(
            "event `{}` has invalid timestamp `{}`: {error}",
            event.event_type, event.at
        ))
    })?;
    Ok(Some(at))
}

fn repair_event_file_from_mirror(
    target_path: &Path,
    mirror_path: &Path,
    primary_error: &CamError,
) -> Result<(), CamError> {
    let mirror_events = read_event_file(mirror_path, None).map_err(|mirror_error| {
        CamError::InvalidState(format!(
            "failed to repair {} from mirror {} after error `{}` because mirror read also failed: {}",
            target_path.display(),
            mirror_path.display(),
            primary_error,
            mirror_error
        ))
    })?;
    backup_corrupt_event_file(target_path)?;
    write_event_file_atomic(target_path, &mirror_events)
}

fn backup_corrupt_event_file(path: &Path) -> Result<(), CamError> {
    if !path.is_file() {
        return Ok(());
    }
    let timestamp = OffsetDateTime::now_utc()
        .format(&Rfc3339)
        .unwrap_or_else(|_| "1970-01-01T00:00:00Z".to_string())
        .replace(':', "-");
    let file_name = path
        .file_name()
        .and_then(|value| value.to_str())
        .ok_or_else(|| CamError::InvalidState(format!("invalid log path {}", path.display())))?;
    let backup_path = path.with_file_name(format!("{file_name}.corrupt-{timestamp}"));
    fs::rename(path, backup_path)?;
    Ok(())
}

fn write_event_file_atomic(path: &Path, events: &[StoredLogEvent]) -> Result<(), CamError> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let file_name = path
        .file_name()
        .and_then(|value| value.to_str())
        .ok_or_else(|| CamError::InvalidState(format!("invalid log path {}", path.display())))?;
    let temp_path = path.with_file_name(format!("{file_name}.tmp"));
    {
        let mut temp_file = File::create(&temp_path)?;
        for event in events {
            serde_json::to_writer(&mut temp_file, event)?;
            temp_file.write_all(b"\n")?;
        }
        temp_file.flush()?;
        temp_file.sync_all()?;
    }
    fs::rename(temp_path, path)?;
    Ok(())
}

fn acquire_log_pair_lock(primary_path: &Path, mirror_path: &Path) -> Result<File, CamError> {
    let anchor_path =
        if primary_path.file_name().and_then(|value| value.to_str()) == Some("events.jsonl") {
            primary_path
        } else if mirror_path.file_name().and_then(|value| value.to_str()) == Some("events.jsonl") {
            mirror_path
        } else {
            primary_path
        };
    if let Some(parent) = anchor_path.parent() {
        fs::create_dir_all(parent)?;
    }
    let file_name = anchor_path
        .file_name()
        .and_then(|value| value.to_str())
        .ok_or_else(|| {
            CamError::InvalidState(format!("invalid log path {}", anchor_path.display()))
        })?;
    let lock_path = anchor_path.with_file_name(format!("{file_name}.lock"));
    let file = OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .truncate(false)
        .open(lock_path)?;
    file.lock()?;
    Ok(file)
}

fn append_json_line<T: Serialize>(path: &PathBuf, value: &T) -> Result<(), CamError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let mut file = OpenOptions::new().create(true).append(true).open(path)?;
    serde_json::to_writer(&mut file, value)?;
    file.write_all(b"\n")?;
    file.flush()?;
    file.sync_all()?;
    Ok(())
}
