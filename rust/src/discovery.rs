use crate::core::{
    ChatStatus, ChatStatusSource, DiscoveryDisposition, DiscoveryRow, DiscoverySource, Route,
    ThreadSource, now_utc,
};
use crate::errors::CamError;
use rusqlite::{Connection, OpenFlags};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DiscoverySummary {
    pub ok: bool,
    pub rows_discovered: usize,
    pub approved: usize,
    pub candidate: usize,
    pub quarantined: usize,
    pub rejected: usize,
    pub promoted: usize,
    pub skipped_existing_thread: usize,
    pub skipped_name_collision: usize,
    pub skipped_not_approved: usize,
    pub skipped_promotion_not_requested: usize,
    pub skipped_invalid_after_reclassify: usize,
    pub promotion_decisions: Vec<PromotionDecision>,
    pub scanner: String,
    pub warning: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub desktop_archive_merge: Option<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PromotionDecision {
    pub row_index: usize,
    pub title: String,
    pub thread_id: Option<String>,
    pub cwd_present: bool,
    pub source: String,
    pub route: String,
    pub thread_source: String,
    pub classified_disposition: String,
    pub generated_name: Option<String>,
    pub existing_agent: Option<String>,
    pub decision: PromotionDecisionKind,
    pub reason: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PromotionDecisionKind {
    Promoted,
    SkippedExistingThread,
    SkippedNameCollision,
    SkippedNotApproved,
    SkippedPromotionNotRequested,
    SkippedInvalidAfterReclassify,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DesktopThreadArchiveEvidence {
    pub thread_id: String,
    pub chat_status: ChatStatus,
    pub chat_status_source: ChatStatusSource,
    pub title: String,
    pub updated_at: String,
}

impl DiscoverySummary {
    pub fn from_rows(
        rows: &[DiscoveryRow],
        promotion_decisions: Vec<PromotionDecision>,
        scanner: impl Into<String>,
        warning: Option<String>,
    ) -> Self {
        let promoted = count_decisions(&promotion_decisions, PromotionDecisionKind::Promoted);
        Self {
            ok: true,
            rows_discovered: rows.len(),
            approved: count(rows, DiscoveryDisposition::Approved),
            candidate: count(rows, DiscoveryDisposition::Candidate),
            quarantined: count(rows, DiscoveryDisposition::Quarantined),
            rejected: count(rows, DiscoveryDisposition::Rejected),
            promoted,
            skipped_existing_thread: count_decisions(
                &promotion_decisions,
                PromotionDecisionKind::SkippedExistingThread,
            ),
            skipped_name_collision: count_decisions(
                &promotion_decisions,
                PromotionDecisionKind::SkippedNameCollision,
            ),
            skipped_not_approved: count_decisions(
                &promotion_decisions,
                PromotionDecisionKind::SkippedNotApproved,
            ),
            skipped_promotion_not_requested: count_decisions(
                &promotion_decisions,
                PromotionDecisionKind::SkippedPromotionNotRequested,
            ),
            skipped_invalid_after_reclassify: count_decisions(
                &promotion_decisions,
                PromotionDecisionKind::SkippedInvalidAfterReclassify,
            ),
            promotion_decisions,
            scanner: scanner.into(),
            warning,
            desktop_archive_merge: None,
        }
    }
}

pub fn classify_row(mut row: DiscoveryRow) -> DiscoveryRow {
    if row
        .thread_id
        .as_deref()
        .unwrap_or_default()
        .trim()
        .is_empty()
    {
        row.disposition = DiscoveryDisposition::Rejected;
        row.reason = "missing thread/session id".to_string();
        return row;
    }
    if let Some(thread_id) = row.thread_id.as_deref()
        && thread_id.trim() != thread_id
    {
        row.disposition = DiscoveryDisposition::Rejected;
        row.reason = "non-canonical thread/session id".to_string();
        return row;
    }

    if looks_machine_spawned(&row.title) {
        row.disposition = DiscoveryDisposition::Quarantined;
        row.reason = "title looks machine-spawned or CAM-message generated".to_string();
        return row;
    }

    if matches!(row.route, Route::Local) && row.cwd.as_deref().unwrap_or_default().trim().is_empty()
    {
        row.disposition = DiscoveryDisposition::Rejected;
        row.reason = "missing local workspace path".to_string();
        return row;
    }
    if matches!(row.route, Route::Local)
        && let Some(cwd) = row.cwd.as_deref()
        && cwd.trim() != cwd
    {
        row.disposition = DiscoveryDisposition::Rejected;
        row.reason = "non-canonical local workspace path".to_string();
        return row;
    }
    if matches!(row.route, Route::Local)
        && !row.workspace_in_project
        && !has_thread_database_archive_evidence(&row)
    {
        row.disposition = DiscoveryDisposition::Rejected;
        row.reason = match row.source {
            DiscoverySource::Rollout => "workspace is outside known project roots".to_string(),
            _ => "workspace is not proven in-project by rollout metadata".to_string(),
        };
        return row;
    }

    if matches!(row.route, Route::Peer { .. }) || row.source == DiscoverySource::RemoteInventory {
        row.disposition = DiscoveryDisposition::Candidate;
        row.reason = "remote inventory row requires peer sync/import step".to_string();
        return row;
    }

    row.disposition = match row.thread_source {
        ThreadSource::Codex => DiscoveryDisposition::Approved,
        ThreadSource::Mailbox | ThreadSource::GuiOnly | ThreadSource::RemoteMirror => {
            DiscoveryDisposition::Candidate
        }
        ThreadSource::AgySession => DiscoveryDisposition::Candidate,
    };
    row.reason = match row.disposition {
        DiscoveryDisposition::Approved if has_thread_database_archive_evidence(&row) => {
            "trusted Codex thread database archive metadata".to_string()
        }
        DiscoveryDisposition::Approved => "trusted local session with id and workspace".to_string(),
        DiscoveryDisposition::Candidate => {
            if row.thread_source == ThreadSource::AgySession {
                "AGY discovery is not trusted until an AGY-specific scanner is implemented"
                    .to_string()
            } else {
                "not a directly promotable local conversational session".to_string()
            }
        }
        DiscoveryDisposition::Quarantined | DiscoveryDisposition::Rejected => row.reason,
    };
    row
}

fn has_thread_database_archive_evidence(row: &DiscoveryRow) -> bool {
    row.source == DiscoverySource::ThreadDatabase
        && row.chat_status_source == ChatStatusSource::ThreadDatabase
        && row.chat_status != ChatStatus::Unknown
}

pub fn classify_rows(rows: Vec<DiscoveryRow>) -> Vec<DiscoveryRow> {
    rows.into_iter().map(classify_row).collect()
}

pub fn discover_local_codex(
    codex_home: &Path,
) -> Result<(Vec<DiscoveryRow>, Option<String>), CamError> {
    let session_index = codex_home.join("session_index.jsonl");
    let codex_state = codex_home.join("codex_state.json");
    let global_state = codex_home.join(".codex-global-state.json");
    let thread_db = codex_home.join("state_5.sqlite");
    let sessions_root = codex_home.join("sessions");
    if !session_index.is_file()
        && !codex_state.is_file()
        && !global_state.is_file()
        && !thread_db.is_file()
    {
        return Ok((
            Vec::new(),
            Some(format!(
                "Codex discovery sources not found at {}, {}, {}, or {}; no local discovery rows were produced",
                session_index.display(),
                codex_state.display(),
                thread_db.display(),
                global_state.display()
            )),
        ));
    }

    let mut rows = Vec::new();
    let mut warnings = Vec::new();
    if thread_db.is_file() {
        match scan_codex_thread_db(&thread_db) {
            Ok(thread_rows) => rows.extend(thread_rows),
            Err(error) => warnings.push(format!(
                "primary Codex thread database source {} was ignored after loud parse failure: {error}",
                thread_db.display()
            )),
        }
    }
    if session_index.is_file() {
        rows.extend(scan_codex_session_index(&session_index)?);
    }
    if codex_state.is_file() {
        match scan_codex_state(&codex_state) {
            Ok(state_rows) => rows.extend(state_rows),
            Err(error) => warnings.push(format!(
                "optional Codex state source {} was ignored after loud parse failure: {error}",
                codex_state.display()
            )),
        }
    }
    let workspace_hints = if global_state.is_file() {
        match load_codex_workspace_hints(&global_state) {
            Ok(hints) => hints,
            Err(error) => {
                warnings.push(format!(
                    "optional Codex global state source {} was ignored after loud parse failure: {error}",
                    global_state.display()
                ));
                BTreeMap::new()
            }
        }
    } else {
        BTreeMap::new()
    };
    let workspace_roots = if global_state.is_file() {
        match load_codex_workspace_roots(&global_state) {
            Ok(roots) => roots,
            Err(error) => {
                warnings.push(format!(
                    "optional Codex global state roots source {} was ignored after loud parse failure: {error}",
                    global_state.display()
                ));
                Vec::new()
            }
        }
    } else {
        Vec::new()
    };
    if global_state.is_file() && workspace_roots.is_empty() {
        warnings.push(format!(
            "Codex global state {} exposed no active or saved workspace roots; in-project workspace approval will reject local rows until roots are visible",
            global_state.display()
        ));
    }
    let session_meta_cwds = if sessions_root.is_dir() {
        match scan_codex_session_meta_cwds(&sessions_root) {
            Ok(cwds) => cwds,
            Err(error) => {
                warnings.push(format!(
                    "optional Codex sessions source {} was ignored after loud parse failure: {error}",
                    sessions_root.display()
                ));
                BTreeMap::new()
            }
        }
    } else {
        BTreeMap::new()
    };

    rows = enrich_codex_rows(rows, &workspace_hints, &workspace_roots, &session_meta_cwds);

    let (rows, duplicate_warnings) = deduplicate_discovery_rows(rows);
    warnings.extend(duplicate_warnings);

    Ok((
        classify_rows(rows),
        if warnings.is_empty() {
            None
        } else {
            Some(warnings.join("; "))
        },
    ))
}

pub fn default_codex_home() -> Option<PathBuf> {
    std::env::var_os("CODEX_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("USERPROFILE").map(|home| PathBuf::from(home).join(".codex")))
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".codex")))
}

pub fn scan_codex_session_index(path: &Path) -> Result<Vec<DiscoveryRow>, CamError> {
    let file = fs::File::open(path)?;
    let reader = BufReader::new(file);
    let mut rows = Vec::new();

    for (index, line) in reader.lines().enumerate() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let value: Value = serde_json::from_str(&line).map_err(|error| {
            CamError::InvalidState(format!(
                "invalid Codex session index JSONL in {} at line {}: {error}",
                path.display(),
                index + 1
            ))
        })?;
        rows.push(session_index_value_to_row(&value));
    }

    Ok(rows)
}

pub fn scan_codex_state(path: &Path) -> Result<Vec<DiscoveryRow>, CamError> {
    let contents = fs::read_to_string(path)?;
    let value: Value = serde_json::from_str(&contents).map_err(|error| {
        CamError::InvalidState(format!(
            "invalid Codex state JSON in {}: {error}",
            path.display()
        ))
    })?;

    let values = codex_state_session_values(&value).ok_or_else(|| {
        CamError::InvalidState(format!(
            "Codex state JSON in {} does not contain a sessions/threads collection",
            path.display()
        ))
    })?;

    Ok(values
        .into_iter()
        .filter(|value| value.is_object())
        .map(|value| codex_state_value_to_row(value))
        .collect())
}

fn session_index_value_to_row(value: &Value) -> DiscoveryRow {
    let thread_id = first_identity_string(value, &["thread_id", "threadId", "id"]);
    let title = first_string(value, &["thread_name", "threadName", "title", "name"])
        .unwrap_or_else(|| "untitled Codex session".to_string());
    let cwd = first_identity_string(
        value,
        &["cwd", "workspace", "workspace_path", "workspacePath"],
    );
    let updated_at = first_string(value, &["updated_at", "updatedAt"]).unwrap_or_else(now_utc);

    DiscoveryRow {
        thread_id,
        title,
        cwd,
        workspace_in_project: false,
        source: DiscoverySource::SessionIndex,
        route: Route::Local,
        peer_name: None,
        thread_source: ThreadSource::Codex,
        chat_status: ChatStatus::Unknown,
        chat_status_source: ChatStatusSource::Unknown,
        updated_at,
        disposition: DiscoveryDisposition::Candidate,
        reason: "unclassified session index row".to_string(),
    }
}

fn codex_state_value_to_row(value: &Value) -> DiscoveryRow {
    let thread_id = first_identity_string(
        value,
        &["thread_id", "threadId", "session_id", "sessionId", "id"],
    );
    let title = first_string(value, &["thread_name", "threadName", "title", "name"])
        .unwrap_or_else(|| "untitled Codex state session".to_string());
    let cwd = first_identity_string(
        value,
        &[
            "cwd",
            "workspace",
            "workspace_path",
            "workspacePath",
            "working_directory",
            "workingDirectory",
        ],
    );
    let updated_at = first_string(value, &["updated_at", "updatedAt", "last_updated_at"])
        .unwrap_or_else(now_utc);

    DiscoveryRow {
        thread_id,
        title,
        cwd,
        workspace_in_project: false,
        source: DiscoverySource::CodexState,
        route: Route::Local,
        peer_name: None,
        thread_source: ThreadSource::Codex,
        chat_status: ChatStatus::Unknown,
        chat_status_source: ChatStatusSource::Unknown,
        updated_at,
        disposition: DiscoveryDisposition::Candidate,
        reason: "unclassified Codex state row".to_string(),
    }
}

pub fn scan_codex_thread_db(path: &Path) -> Result<Vec<DiscoveryRow>, CamError> {
    let connection = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|error| {
        CamError::InvalidState(format!(
            "failed to open Codex thread database {}: {error}",
            path.display()
        ))
    })?;
    let mut statement = connection
        .prepare(
            "SELECT id, title, cwd, updated_at, updated_at_ms, archived, rollout_path
             FROM threads",
        )
        .map_err(|error| {
            CamError::InvalidState(format!(
                "failed to prepare Codex thread database query in {}: {error}",
                path.display()
            ))
        })?;
    let rows = statement
        .query_map([], |row| {
            let thread_id: Option<String> = row.get(0)?;
            let title: String = row.get(1)?;
            let cwd: Option<String> = row.get(2)?;
            let updated_at_seconds: i64 = row.get(3)?;
            let updated_at_ms: Option<i64> = row.get(4)?;
            let archived: i64 = row.get(5)?;
            let rollout_path: Option<String> = row.get(6)?;
            Ok(DiscoveryRow {
                thread_id,
                title,
                cwd,
                workspace_in_project: false,
                source: match rollout_path.as_deref() {
                    Some(path)
                        if path.contains("archived_sessions") || path.contains("sessions") =>
                    {
                        DiscoverySource::Rollout
                    }
                    _ => DiscoverySource::ThreadDatabase,
                },
                route: Route::Local,
                peer_name: None,
                thread_source: ThreadSource::Codex,
                chat_status: if archived == 0 {
                    ChatStatus::Active
                } else {
                    ChatStatus::Archived
                },
                chat_status_source: ChatStatusSource::ThreadDatabase,
                updated_at: format_thread_updated_at(updated_at_ms, updated_at_seconds),
                disposition: DiscoveryDisposition::Candidate,
                reason: "unclassified Codex thread database row".to_string(),
            })
        })
        .map_err(|error| {
            CamError::InvalidState(format!(
                "failed to read Codex thread database {}: {error}",
                path.display()
            ))
        })?;
    let mut discovered = Vec::new();
    for row in rows {
        discovered.push(row.map_err(|error| {
            CamError::InvalidState(format!(
                "failed to decode Codex thread database row in {}: {error}",
                path.display()
            ))
        })?);
    }
    Ok(discovered)
}

pub fn scan_desktop_thread_archive_evidence(
    path: &Path,
) -> Result<BTreeMap<String, DesktopThreadArchiveEvidence>, CamError> {
    let connection = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|error| {
        CamError::InvalidState(format!(
            "failed to open Codex Desktop thread database {}: {error}",
            path.display()
        ))
    })?;
    let mut statement = connection
        .prepare("SELECT id, title, updated_at, updated_at_ms, archived FROM threads")
        .map_err(|error| {
            CamError::InvalidState(format!(
                "failed to prepare Codex Desktop archive query in {}: {error}",
                path.display()
            ))
        })?;
    let rows = statement
        .query_map([], |row| {
            let thread_id: String = row.get(0)?;
            let title: String = row.get(1)?;
            let updated_at_seconds: i64 = row.get(2)?;
            let updated_at_ms: Option<i64> = row.get(3)?;
            let archived: i64 = row.get(4)?;
            Ok(DesktopThreadArchiveEvidence {
                thread_id,
                chat_status: if archived == 0 {
                    ChatStatus::Active
                } else {
                    ChatStatus::Archived
                },
                chat_status_source: ChatStatusSource::DesktopThreadDatabase,
                title,
                updated_at: format_thread_updated_at(updated_at_ms, updated_at_seconds),
            })
        })
        .map_err(|error| {
            CamError::InvalidState(format!(
                "failed to read Codex Desktop archive evidence {}: {error}",
                path.display()
            ))
        })?;
    let mut evidence = BTreeMap::new();
    for row in rows {
        let row = row.map_err(|error| {
            CamError::InvalidState(format!(
                "failed to decode Codex Desktop archive evidence row in {}: {error}",
                path.display()
            ))
        })?;
        evidence.insert(row.thread_id.clone(), row);
    }
    Ok(evidence)
}

fn codex_state_session_values(value: &Value) -> Option<Vec<&Value>> {
    if let Some(values) = value.as_array() {
        return Some(values.iter().collect());
    }

    for key in ["sessions", "threads", "items", "conversations"] {
        if let Some(values) = value.get(key).and_then(Value::as_array) {
            return Some(values.iter().collect());
        }
        if let Some(map) = value.get(key).and_then(Value::as_object) {
            return Some(map.values().collect());
        }
    }

    value.as_object().and_then(|map| {
        if map.values().all(Value::is_object) {
            Some(map.values().collect())
        } else {
            None
        }
    })
}

fn enrich_codex_rows(
    rows: Vec<DiscoveryRow>,
    workspace_hints: &BTreeMap<String, String>,
    workspace_roots: &[String],
    session_meta_cwds: &BTreeMap<String, String>,
) -> Vec<DiscoveryRow> {
    rows.into_iter()
        .map(|mut row| {
            let mut rollout_cwd = None;
            if let Some(thread_id) = row.thread_id.as_deref() {
                rollout_cwd = session_meta_cwds.get(thread_id).cloned();
                if row.cwd.as_deref().unwrap_or_default().trim().is_empty() {
                    row.cwd = rollout_cwd
                        .clone()
                        .or_else(|| workspace_hints.get(thread_id).cloned());
                }
            }
            if rollout_cwd.is_some() {
                row.source = DiscoverySource::Rollout;
            }
            row.workspace_in_project = row
                .cwd
                .as_deref()
                .is_some_and(|cwd| is_in_project_workspace(cwd, workspace_roots))
                && matches!(
                    row.source,
                    DiscoverySource::Rollout | DiscoverySource::ThreadDatabase
                );
            row
        })
        .collect()
}

fn deduplicate_discovery_rows(rows: Vec<DiscoveryRow>) -> (Vec<DiscoveryRow>, Vec<String>) {
    let mut keyed = BTreeMap::new();
    let mut unkeyed = Vec::new();
    let mut warnings = Vec::new();

    for row in rows {
        let Some(thread_id) = row.thread_id.clone() else {
            unkeyed.push(row);
            continue;
        };
        keyed
            .entry(thread_id)
            .and_modify(|existing| {
                if discovery_rows_conflict(existing, &row) {
                    warnings.push(format!(
                        "duplicate thread id `{}` had conflicting source fields; source precedence kept {:?} and ignored {:?}",
                        existing.thread_id.as_deref().unwrap_or_default(),
                        preferred_discovery_source(existing, &row),
                        ignored_discovery_source(existing, &row)
                    ));
                }
                *existing = merge_discovery_rows(existing, &row);
            })
            .or_insert(row);
    }

    let mut merged = keyed.into_values().collect::<Vec<_>>();
    merged.extend(unkeyed);
    (merged, warnings)
}

fn should_replace_discovery_row(existing: &DiscoveryRow, candidate: &DiscoveryRow) -> bool {
    match (&existing.source, &candidate.source) {
        (DiscoverySource::ThreadDatabase, DiscoverySource::ThreadDatabase) => {
            candidate.updated_at > existing.updated_at
        }
        (_, DiscoverySource::ThreadDatabase) => true,
        (DiscoverySource::ThreadDatabase, _) => false,
        (DiscoverySource::SessionIndex, DiscoverySource::CodexState) => true,
        (DiscoverySource::CodexState, DiscoverySource::SessionIndex) => false,
        _ => candidate.updated_at > existing.updated_at,
    }
}

fn merge_discovery_rows(existing: &DiscoveryRow, candidate: &DiscoveryRow) -> DiscoveryRow {
    let mut merged = if should_replace_discovery_row(existing, candidate) {
        candidate.clone()
    } else {
        existing.clone()
    };

    if merged.chat_status == ChatStatus::Unknown {
        merged.chat_status = preferred_chat_status(existing, candidate);
    }
    merged.chat_status_source =
        preferred_chat_status_source(existing, candidate, &merged.chat_status);
    if merged.cwd.as_deref().unwrap_or_default().trim().is_empty() {
        merged.cwd = candidate.cwd.clone().or_else(|| existing.cwd.clone());
    }
    if !merged.workspace_in_project {
        merged.workspace_in_project =
            existing.workspace_in_project || candidate.workspace_in_project;
    }
    if merged.source == DiscoverySource::SessionIndex {
        if existing.source == DiscoverySource::Rollout
            || candidate.source == DiscoverySource::Rollout
        {
            merged.source = DiscoverySource::Rollout;
        }
        if existing.source == DiscoverySource::ThreadDatabase
            || candidate.source == DiscoverySource::ThreadDatabase
        {
            merged.source = DiscoverySource::ThreadDatabase;
        }
    }

    merged
}

fn discovery_rows_conflict(existing: &DiscoveryRow, candidate: &DiscoveryRow) -> bool {
    existing.title != candidate.title
        || existing.cwd != candidate.cwd
        || existing.thread_source != candidate.thread_source
        || existing.source != candidate.source
}

fn preferred_discovery_source<'a>(
    existing: &'a DiscoveryRow,
    candidate: &'a DiscoveryRow,
) -> &'a DiscoverySource {
    if should_replace_discovery_row(existing, candidate) {
        &candidate.source
    } else {
        &existing.source
    }
}

fn ignored_discovery_source<'a>(
    existing: &'a DiscoveryRow,
    candidate: &'a DiscoveryRow,
) -> &'a DiscoverySource {
    if should_replace_discovery_row(existing, candidate) {
        &existing.source
    } else {
        &candidate.source
    }
}

fn preferred_chat_status(existing: &DiscoveryRow, candidate: &DiscoveryRow) -> ChatStatus {
    match (&existing.chat_status, &candidate.chat_status) {
        (ChatStatus::Archived, _) | (_, ChatStatus::Archived) => ChatStatus::Archived,
        (ChatStatus::Active, _) => ChatStatus::Active,
        (_, ChatStatus::Active) => ChatStatus::Active,
        _ => ChatStatus::Unknown,
    }
}

fn preferred_chat_status_source(
    existing: &DiscoveryRow,
    candidate: &DiscoveryRow,
    resolved_status: &ChatStatus,
) -> ChatStatusSource {
    for row in [existing, candidate] {
        if &row.chat_status == resolved_status
            && row.chat_status_source == ChatStatusSource::ThreadDatabase
        {
            return ChatStatusSource::ThreadDatabase;
        }
    }
    for row in [existing, candidate] {
        if &row.chat_status == resolved_status
            && row.chat_status_source == ChatStatusSource::RemoteInventory
        {
            return ChatStatusSource::RemoteInventory;
        }
    }
    ChatStatusSource::Unknown
}

fn first_string(value: &Value, keys: &[&str]) -> Option<String> {
    keys.iter()
        .filter_map(|key| value.get(*key))
        .filter_map(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .next()
}

fn format_thread_updated_at(updated_at_ms: Option<i64>, updated_at_seconds: i64) -> String {
    if let Some(updated_at_ms) = updated_at_ms
        && let Ok(datetime) =
            time::OffsetDateTime::from_unix_timestamp_nanos(i128::from(updated_at_ms) * 1_000_000)
    {
        return datetime
            .format(&time::format_description::well_known::Rfc3339)
            .unwrap_or_else(|_| updated_at_ms.to_string());
    }
    if let Ok(datetime) = time::OffsetDateTime::from_unix_timestamp(updated_at_seconds) {
        return datetime
            .format(&time::format_description::well_known::Rfc3339)
            .unwrap_or_else(|_| updated_at_seconds.to_string());
    }
    updated_at_ms
        .map(|value| value.to_string())
        .unwrap_or_else(|| updated_at_seconds.to_string())
}

fn first_identity_string(value: &Value, keys: &[&str]) -> Option<String> {
    keys.iter()
        .filter_map(|key| value.get(*key))
        .filter_map(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .map(ToOwned::to_owned)
        .next()
}

fn count(rows: &[DiscoveryRow], disposition: DiscoveryDisposition) -> usize {
    rows.iter()
        .filter(|row| row.disposition == disposition)
        .count()
}

fn count_decisions(decisions: &[PromotionDecision], kind: PromotionDecisionKind) -> usize {
    decisions
        .iter()
        .filter(|decision| decision.decision == kind)
        .count()
}

fn looks_machine_spawned(title: &str) -> bool {
    let title = title.to_ascii_lowercase();
    title.contains("subagent")
        || title.contains("sub-agent")
        || title.contains("cam message")
        || title.contains("cam-message")
}

fn load_codex_workspace_hints(path: &Path) -> Result<BTreeMap<String, String>, CamError> {
    let contents = fs::read_to_string(path)?;
    let value: Value = serde_json::from_str(&contents).map_err(|error| {
        CamError::InvalidState(format!(
            "invalid Codex global state JSON in {}: {error}",
            path.display()
        ))
    })?;

    let hints = value
        .get("thread-workspace-root-hints")
        .and_then(Value::as_object)
        .map(|map| {
            map.iter()
                .filter_map(|(thread_id, cwd)| {
                    cwd.as_str()
                        .map(str::trim)
                        .filter(|cwd| !cwd.is_empty())
                        .map(|cwd| (thread_id.clone(), cwd.to_string()))
                })
                .collect::<BTreeMap<_, _>>()
        })
        .unwrap_or_default();

    Ok(hints)
}

fn load_codex_workspace_roots(path: &Path) -> Result<Vec<String>, CamError> {
    let contents = fs::read_to_string(path)?;
    let value: Value = serde_json::from_str(&contents).map_err(|error| {
        CamError::InvalidState(format!(
            "invalid Codex global state JSON in {}: {error}",
            path.display()
        ))
    })?;

    let mut roots = Vec::new();
    extend_string_array_field(&mut roots, &value, "active-workspace-roots");
    extend_string_array_field(&mut roots, &value, "electron-saved-workspace-roots");
    roots.sort();
    roots.dedup();
    Ok(roots)
}

fn extend_string_array_field(target: &mut Vec<String>, value: &Value, key: &str) {
    let Some(values) = value.get(key).and_then(Value::as_array) else {
        return;
    };
    target.extend(values.iter().filter_map(|entry| {
        entry
            .as_str()
            .map(str::trim)
            .filter(|entry| !entry.is_empty())
            .map(ToOwned::to_owned)
    }));
}

fn scan_codex_session_meta_cwds(path: &Path) -> Result<BTreeMap<String, String>, CamError> {
    let mut rows = BTreeMap::new();
    scan_codex_session_meta_cwds_inner(path, &mut rows)?;
    Ok(rows)
}

fn scan_codex_session_meta_cwds_inner(
    path: &Path,
    rows: &mut BTreeMap<String, String>,
) -> Result<(), CamError> {
    for entry in fs::read_dir(path)? {
        let entry = entry?;
        let entry_path = entry.path();
        if entry.file_type()?.is_dir() {
            scan_codex_session_meta_cwds_inner(&entry_path, rows)?;
            continue;
        }
        if entry_path.extension().and_then(|value| value.to_str()) != Some("jsonl") {
            continue;
        }
        if let Some((thread_id, cwd)) = scan_codex_session_file_for_meta(&entry_path)? {
            rows.entry(thread_id).or_insert(cwd);
        }
    }
    Ok(())
}

fn scan_codex_session_file_for_meta(path: &Path) -> Result<Option<(String, String)>, CamError> {
    let file = fs::File::open(path)?;
    let reader = BufReader::new(file);

    for (index, line) in reader.lines().enumerate() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let value: Value = serde_json::from_str(&line).map_err(|error| {
            CamError::InvalidState(format!(
                "invalid Codex session JSONL in {} at line {}: {error}",
                path.display(),
                index + 1
            ))
        })?;
        if value.get("type").and_then(Value::as_str) != Some("session_meta") {
            continue;
        }

        let payload = match value.get("payload") {
            Some(payload) => payload,
            None => return Ok(None),
        };
        let thread_id = first_identity_string(payload, &["id", "thread_id", "threadId"]);
        let cwd = first_identity_string(
            payload,
            &[
                "cwd",
                "workspace",
                "workspace_path",
                "workspacePath",
                "working_directory",
                "workingDirectory",
            ],
        );
        return Ok(match (thread_id, cwd) {
            (Some(thread_id), Some(cwd)) => Some((thread_id, cwd)),
            _ => None,
        });
    }

    Ok(None)
}

fn is_in_project_workspace(cwd: &str, workspace_roots: &[String]) -> bool {
    let normalized_cwd = normalize_path_for_match(cwd);
    workspace_roots.iter().any(|root| {
        let normalized_root = normalize_path_for_match(root);
        normalized_cwd == normalized_root
            || normalized_cwd
                .strip_prefix(&normalized_root)
                .is_some_and(|suffix| suffix.starts_with('/'))
    })
}

fn normalize_path_for_match(path: &str) -> String {
    let trimmed = path.trim().trim_end_matches(['\\', '/']);
    let slash_normalized = trimmed.replace('\\', "/");
    if looks_like_windows_path(&slash_normalized) {
        slash_normalized.to_ascii_lowercase()
    } else {
        slash_normalized
    }
}

fn looks_like_windows_path(path: &str) -> bool {
    path.contains('\\')
        || path
            .as_bytes()
            .get(1)
            .is_some_and(|separator| *separator == b':')
}
