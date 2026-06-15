use crate::core::{Agent, Config, DaemonState, DiscoveryRow, Message, Peer, default_node_name};
use crate::errors::CamError;
use serde::Serialize;
use serde::de::DeserializeOwned;
use std::fs::{self, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppState {
    pub config: Config,
    pub daemon: DaemonState,
    pub agents: Vec<Agent>,
    pub peers: Vec<Peer>,
    pub discovery_rows: Vec<DiscoveryRow>,
    pub mailbox: Vec<Message>,
}

impl AppState {
    pub fn new() -> Self {
        let config = Config::new(default_node_name());
        let daemon = DaemonState::new(config.node_name.clone(), config.headless);
        Self {
            config,
            daemon,
            agents: Vec::new(),
            peers: Vec::new(),
            discovery_rows: Vec::new(),
            mailbox: Vec::new(),
        }
    }

    pub fn ensure_builtin_agents(&mut self) {
        if !self.agents.iter().any(|agent| agent.name == "operator") {
            self.agents.push(Agent::builtin_operator());
        }
    }

    pub fn find_agent(&self, name: &str) -> Option<&Agent> {
        self.agents.iter().find(|agent| agent.name == name)
    }

    pub fn find_agent_mut(&mut self, name: &str) -> Option<&mut Agent> {
        self.agents.iter_mut().find(|agent| agent.name == name)
    }

    pub fn add_agent(&mut self, agent: Agent) -> Result<(), CamError> {
        if self.find_agent(&agent.name).is_some() {
            return Err(CamError::InvalidState(format!(
                "agent `{}` already exists",
                agent.name
            )));
        }
        self.agents.push(agent);
        Ok(())
    }

    pub fn find_peer(&self, name: &str) -> Option<&Peer> {
        self.peers.iter().find(|peer| peer.name == name)
    }

    pub fn upsert_peer(&mut self, mut peer: Peer) -> bool {
        if let Some(existing) = self
            .peers
            .iter_mut()
            .find(|existing| existing.name == peer.name)
        {
            peer.created_at = existing.created_at.clone();
            *existing = peer;
            true
        } else {
            self.peers.push(peer);
            false
        }
    }

    pub fn push_mailbox_message(&mut self, message: Message) {
        self.mailbox.push(message);
    }
}

impl Default for AppState {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone)]
pub struct StateStore {
    home: PathBuf,
}

impl StateStore {
    pub fn new(home: impl Into<PathBuf>) -> Self {
        Self { home: home.into() }
    }

    pub fn home(&self) -> &Path {
        &self.home
    }

    pub fn init(&self) -> Result<AppState, CamError> {
        self.ensure_layout()?;
        let mut state = if self.any_state_file_exists() {
            self.load_existing()?
        } else {
            AppState::new()
        };
        self.ensure_local_api_token()?;
        state.ensure_builtin_agents();
        self.save_all(&state)?;
        self.touch_file(&self.events_path())?;
        self.touch_file(&self.daemon_log_path())?;
        Ok(state)
    }

    pub fn load_existing(&self) -> Result<AppState, CamError> {
        self.require_initialized_state_files()?;

        Ok(AppState {
            config: read_json(&self.config_path())?,
            daemon: read_json(&self.daemon_path())?,
            agents: read_json(&self.agents_path())?,
            peers: read_json(&self.peers_path())?,
            discovery_rows: read_json(&self.discovery_path())?,
            mailbox: read_jsonl(&self.mailbox_path())?,
        })
    }

    pub fn load_daemon(&self) -> Result<DaemonState, CamError> {
        self.require_initialized_state_files()?;
        read_json(&self.daemon_path())
    }

    pub fn save_daemon(&self, daemon: &DaemonState) -> Result<(), CamError> {
        self.ensure_layout()?;
        let path = self.daemon_path();
        preflight_replace_target(&path)?;
        let staged = stage_json_atomic(&path, daemon)?;
        commit_staged_writes(&[staged])
    }

    pub fn save_all(&self, state: &AppState) -> Result<(), CamError> {
        self.ensure_layout()?;
        let paths = [
            self.config_path(),
            self.daemon_path(),
            self.agents_path(),
            self.peers_path(),
            self.discovery_path(),
            self.mailbox_path(),
        ];
        preflight_replace_targets(&paths)?;

        let mut staged = Vec::new();
        let stage_result = (|| -> Result<(), CamError> {
            staged.push(stage_json_atomic(&paths[0], &state.config)?);
            staged.push(stage_json_atomic(&paths[1], &state.daemon)?);
            staged.push(stage_json_atomic(&paths[2], &state.agents)?);
            staged.push(stage_json_atomic(&paths[3], &state.peers)?);
            staged.push(stage_json_atomic(&paths[4], &state.discovery_rows)?);
            staged.push(stage_jsonl_atomic(&paths[5], &state.mailbox)?);
            Ok(())
        })();
        if let Err(error) = stage_result {
            cleanup_staged_writes(&staged);
            return Err(error);
        }

        commit_staged_writes(&staged)
    }

    pub fn doctor_checks(&self) -> Vec<DoctorCheck> {
        vec![
            DoctorCheck::path("home", &self.home, self.home.is_dir()),
            DoctorCheck::path(
                "config.json",
                self.config_path(),
                self.config_path().is_file(),
            ),
            DoctorCheck::path(
                "agents.json",
                self.agents_path(),
                self.agents_path().is_file(),
            ),
            DoctorCheck::path(
                "daemon.json",
                self.daemon_path(),
                self.daemon_path().is_file(),
            ),
            DoctorCheck::path("peers.json", self.peers_path(), self.peers_path().is_file()),
            DoctorCheck::path(
                "mailbox.jsonl",
                self.mailbox_path(),
                self.mailbox_path().is_file(),
            ),
            DoctorCheck::path(
                "discovery.json",
                self.discovery_path(),
                self.discovery_path().is_file(),
            ),
            DoctorCheck::path(
                "events.jsonl",
                self.events_path(),
                self.events_path().is_file(),
            ),
            DoctorCheck::path(
                "logs/daemon.log",
                self.daemon_log_path(),
                self.daemon_log_path().is_file(),
            ),
            DoctorCheck::path(
                "secrets/local-api-token",
                self.local_api_token_path(),
                self.local_api_token_path().is_file(),
            ),
            DoctorCheck::path(
                "secrets/daemon-identity",
                self.daemon_identity_dir(),
                self.daemon_identity_dir().is_dir(),
            ),
        ]
    }

    pub fn validate_local_api_token(&self) -> Result<(), CamError> {
        let path = self.local_api_token_path();
        let token = fs::read_to_string(&path)?;
        validate_local_api_token(&token).map_err(|error| {
            CamError::InvalidState(format!(
                "invalid local API token at {}: {error}",
                path.display()
            ))
        })
    }

    pub fn read_local_api_token(&self) -> Result<String, CamError> {
        let path = self.local_api_token_path();
        let token = fs::read_to_string(&path)?;
        validate_local_api_token(&token).map_err(|error| {
            CamError::InvalidState(format!(
                "invalid local API token at {}: {error}",
                path.display()
            ))
        })?;
        Ok(token.strip_suffix('\n').unwrap_or(&token).to_string())
    }

    pub fn create_daemon_identity_nonce(
        &self,
        instance_id: &str,
    ) -> Result<(String, String), CamError> {
        validate_daemon_instance_id(instance_id)?;
        let relative_ref = format!("secrets/daemon-identity/{instance_id}.nonce");
        let path = self.home.join(&relative_ref);
        if path.exists() {
            return Err(CamError::InvalidState(format!(
                "daemon identity nonce already exists for instance `{instance_id}`"
            )));
        }

        let nonce = format!("qcam-daemon-{}-{}\n", Uuid::new_v4(), Uuid::new_v4());
        validate_daemon_control_nonce(&nonce).map_err(CamError::InvalidState)?;
        write_atomic(&path, nonce.as_bytes())?;
        Ok((relative_ref, nonce.trim_end_matches('\n').to_string()))
    }

    pub fn read_daemon_identity_nonce(&self, relative_ref: &str) -> Result<String, CamError> {
        validate_daemon_identity_nonce_ref(relative_ref)?;
        let path = self.home.join(relative_ref);
        let nonce = fs::read_to_string(&path)?;
        validate_daemon_control_nonce(&nonce).map_err(|error| {
            CamError::InvalidState(format!(
                "invalid daemon identity nonce at {}: {error}",
                path.display()
            ))
        })?;
        Ok(nonce.strip_suffix('\n').unwrap_or(&nonce).to_string())
    }

    pub fn events_path(&self) -> PathBuf {
        self.home.join("events.jsonl")
    }

    pub fn daemon_log_path(&self) -> PathBuf {
        self.home.join("logs").join("daemon.log")
    }

    fn ensure_layout(&self) -> Result<(), CamError> {
        fs::create_dir_all(&self.home)?;
        fs::create_dir_all(self.home.join("logs"))?;
        fs::create_dir_all(self.home.join("secrets"))?;
        fs::create_dir_all(self.daemon_identity_dir())?;
        Ok(())
    }

    fn ensure_local_api_token(&self) -> Result<(), CamError> {
        let path = self.local_api_token_path();
        if path.exists() {
            self.validate_local_api_token()?;
            return Ok(());
        }

        let token = format!("qcam-local-{}-{}\n", Uuid::new_v4(), Uuid::new_v4());
        write_atomic(&path, token.as_bytes())
    }

    fn touch_file(&self, path: &Path) -> Result<(), CamError> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        OpenOptions::new().create(true).append(true).open(path)?;
        Ok(())
    }

    fn config_path(&self) -> PathBuf {
        self.home.join("config.json")
    }

    fn agents_path(&self) -> PathBuf {
        self.home.join("agents.json")
    }

    fn daemon_path(&self) -> PathBuf {
        self.home.join("daemon.json")
    }

    fn peers_path(&self) -> PathBuf {
        self.home.join("peers.json")
    }

    fn discovery_path(&self) -> PathBuf {
        self.home.join("discovery.json")
    }

    fn mailbox_path(&self) -> PathBuf {
        self.home.join("mailbox.jsonl")
    }

    fn local_api_token_path(&self) -> PathBuf {
        self.home.join("secrets").join("local-api-token")
    }

    fn daemon_identity_dir(&self) -> PathBuf {
        self.home.join("secrets").join("daemon-identity")
    }

    fn required_state_files(&self) -> [(&'static str, PathBuf); 6] {
        [
            ("config.json", self.config_path()),
            ("daemon.json", self.daemon_path()),
            ("agents.json", self.agents_path()),
            ("peers.json", self.peers_path()),
            ("discovery.json", self.discovery_path()),
            ("mailbox.jsonl", self.mailbox_path()),
        ]
    }

    fn any_state_file_exists(&self) -> bool {
        self.required_state_files()
            .iter()
            .any(|(_, path)| path.exists())
    }

    fn require_initialized_state_files(&self) -> Result<(), CamError> {
        for (name, path) in self.required_state_files() {
            if !path.is_file() {
                return Err(CamError::InvalidState(format!(
                    "missing {name} in {}; run `qexow-cam init` first",
                    self.home.display()
                )));
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DoctorCheck {
    pub name: String,
    pub path: PathBuf,
    pub ok: bool,
}

impl DoctorCheck {
    fn path(name: impl Into<String>, path: impl Into<PathBuf>, ok: bool) -> Self {
        Self {
            name: name.into(),
            path: path.into(),
            ok,
        }
    }
}

fn read_json<T: DeserializeOwned>(path: &Path) -> Result<T, CamError> {
    let contents = fs::read_to_string(path)?;
    Ok(serde_json::from_str(&contents)?)
}

fn read_jsonl<T: DeserializeOwned>(path: &Path) -> Result<Vec<T>, CamError> {
    let file = fs::File::open(path)?;
    let reader = BufReader::new(file);
    let mut values = Vec::new();

    for (index, line) in reader.lines().enumerate() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        values.push(serde_json::from_str(&line).map_err(|error| {
            CamError::InvalidState(format!(
                "invalid JSONL in {} at line {}: {error}",
                path.display(),
                index + 1
            ))
        })?);
    }

    Ok(values)
}

#[derive(Debug)]
struct StagedAtomicWrite {
    temp_path: PathBuf,
    final_path: PathBuf,
}

fn write_atomic(path: &Path, contents: &[u8]) -> Result<(), CamError> {
    let staged = stage_atomic(path, contents)?;
    commit_staged_writes(&[staged])
}

fn stage_json_atomic<T: Serialize>(path: &Path, value: &T) -> Result<StagedAtomicWrite, CamError> {
    let contents = serde_json::to_string_pretty(value)?;
    stage_atomic(path, contents.as_bytes())
}

fn stage_jsonl_atomic<T: Serialize>(
    path: &Path,
    values: &[T],
) -> Result<StagedAtomicWrite, CamError> {
    let mut contents = Vec::new();
    for value in values {
        serde_json::to_writer(&mut contents, value)?;
        contents.push(b'\n');
    }
    stage_atomic(path, &contents)
}

fn stage_atomic(path: &Path, contents: &[u8]) -> Result<StagedAtomicWrite, CamError> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    preflight_replace_target(path)?;

    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| CamError::InvalidState(format!("invalid state path {}", path.display())))?;
    let temp_path = path.with_file_name(format!(
        ".{file_name}.{}.{}.tmp",
        std::process::id(),
        Uuid::new_v4()
    ));

    let write_result = (|| -> Result<(), CamError> {
        let mut file = fs::File::create(&temp_path)?;
        file.write_all(contents)?;
        file.sync_all()?;
        Ok(())
    })();
    if let Err(error) = write_result {
        let _ = fs::remove_file(&temp_path);
        return Err(error);
    }

    Ok(StagedAtomicWrite {
        temp_path,
        final_path: path.to_path_buf(),
    })
}

fn commit_staged_writes(staged: &[StagedAtomicWrite]) -> Result<(), CamError> {
    for (index, write) in staged.iter().enumerate() {
        if let Err(error) = replace_file_atomic(&write.temp_path, &write.final_path) {
            cleanup_staged_writes(&staged[index..]);
            return Err(CamError::InvalidState(format!(
                "failed to replace state file {} from temp file {}: {error}",
                write.final_path.display(),
                write.temp_path.display()
            )));
        }
        if let Err(error) = sync_parent_dir(&write.final_path) {
            cleanup_staged_writes(&staged[index + 1..]);
            return Err(error);
        }
    }
    Ok(())
}

fn cleanup_staged_writes(staged: &[StagedAtomicWrite]) {
    for write in staged {
        let _ = fs::remove_file(&write.temp_path);
    }
}

fn preflight_replace_targets(paths: &[PathBuf]) -> Result<(), CamError> {
    for path in paths {
        preflight_replace_target(path)?;
    }
    Ok(())
}

fn preflight_replace_target(path: &Path) -> Result<(), CamError> {
    if path.exists() && !path.is_file() {
        return Err(CamError::InvalidState(format!(
            "cannot replace state file {} because it is not a file",
            path.display()
        )));
    }
    Ok(())
}

#[cfg(unix)]
fn replace_file_atomic(temp_path: &Path, path: &Path) -> Result<(), CamError> {
    Ok(fs::rename(temp_path, path)?)
}

#[cfg(windows)]
fn replace_file_atomic(temp_path: &Path, path: &Path) -> Result<(), CamError> {
    use std::os::windows::ffi::OsStrExt;

    const MOVEFILE_REPLACE_EXISTING: u32 = 0x1;
    const MOVEFILE_WRITE_THROUGH: u32 = 0x8;

    unsafe extern "system" {
        fn MoveFileExW(
            existing_file_name: *const u16,
            new_file_name: *const u16,
            flags: u32,
        ) -> i32;
    }

    fn wide(path: &Path) -> Vec<u16> {
        path.as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect()
    }

    let from = wide(temp_path);
    let to = wide(path);
    let ok = unsafe {
        MoveFileExW(
            from.as_ptr(),
            to.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    };
    if ok == 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    Ok(())
}

#[cfg(not(any(unix, windows)))]
fn replace_file_atomic(temp_path: &Path, path: &Path) -> Result<(), CamError> {
    Ok(fs::rename(temp_path, path)?)
}

#[cfg(unix)]
fn sync_parent_dir(path: &Path) -> Result<(), CamError> {
    if let Some(parent) = path.parent() {
        fs::File::open(parent)?.sync_all()?;
    }
    Ok(())
}

#[cfg(not(unix))]
fn sync_parent_dir(_path: &Path) -> Result<(), CamError> {
    Ok(())
}

fn validate_local_api_token(token: &str) -> Result<(), String> {
    let token = token.strip_suffix('\n').unwrap_or(token);
    if token.trim().is_empty() {
        return Err("token cannot be empty".to_string());
    }
    if token.trim() != token {
        return Err("token must not have leading or trailing whitespace".to_string());
    }
    if token.chars().any(char::is_whitespace) {
        return Err("token cannot contain whitespace".to_string());
    }
    if !token.starts_with("qcam-local-") {
        return Err("token must use qcam-local prefix".to_string());
    }
    if token.len() < 40 {
        return Err("token is too short".to_string());
    }
    Ok(())
}

fn validate_daemon_instance_id(instance_id: &str) -> Result<(), CamError> {
    Uuid::parse_str(instance_id)
        .map_err(|_| CamError::InvalidState("daemon instance_id must be a UUID".to_string()))?;
    Ok(())
}

pub fn validate_daemon_identity_nonce_ref(relative_ref: &str) -> Result<(), CamError> {
    let Some(instance_id) = relative_ref
        .strip_prefix("secrets/daemon-identity/")
        .and_then(|value| value.strip_suffix(".nonce"))
    else {
        return Err(CamError::InvalidState(
            "daemon identity nonce ref must be relative under secrets/daemon-identity".to_string(),
        ));
    };
    if relative_ref.contains('\\') || relative_ref.contains("..") {
        return Err(CamError::InvalidState(
            "daemon identity nonce ref must be canonical and relative".to_string(),
        ));
    }
    validate_daemon_instance_id(instance_id)
}

fn validate_daemon_control_nonce(nonce: &str) -> Result<(), String> {
    let nonce = nonce.strip_suffix('\n').unwrap_or(nonce);
    if nonce.trim().is_empty() {
        return Err("nonce cannot be empty".to_string());
    }
    if nonce.trim() != nonce {
        return Err("nonce must not have leading or trailing whitespace".to_string());
    }
    if nonce.chars().any(char::is_whitespace) {
        return Err("nonce cannot contain whitespace".to_string());
    }
    if !nonce.starts_with("qcam-daemon-") {
        return Err("nonce must use qcam-daemon prefix".to_string());
    }
    if nonce.len() < 40 {
        return Err("nonce is too short".to_string());
    }
    Ok(())
}
