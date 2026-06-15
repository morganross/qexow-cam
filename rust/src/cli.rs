use crate::core::{
    Agent, AgentKind, DeliveryState, Message, Peer, PeerState, PeerTransport, Route,
};
use crate::delivery::{DeliveryAction, DeliveryDecision};
use crate::delivery_transaction::{DirectDeliveryError, execute_direct_delivery};
use crate::errors::CamError;
use crate::event_contracts::{validate_event_log_invariants, validate_event_mirror};
use crate::local_status::{
    render_health_response_with_status, render_status_ui_response, surface_for_state,
};
use crate::logging::{StructuredLogger, read_daemon_log_events, read_events};
use crate::peers::{
    PeerInventoryFetcher, PeerMessageSender, SshInventoryFetcher, SshPeerAgentReadFetcher,
    SshPeerAgentResumeFetcher, SshPeerMessageSender, export_inventory,
};
use crate::providers::codex;
use crate::providers::{
    DefaultProviderRouter, ProviderLifecycleRouter, ProviderRouter, ProviderTranscriptReader,
    boxed_live_codex_stdio_provider_lifecycle_router, boxed_live_codex_stdio_provider_router,
};
use crate::services;
use crate::state::{AppState, StateStore};
use serde::Deserialize;
use std::collections::{BTreeMap, BTreeSet};
use std::io::ErrorKind;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream, ToSocketAddrs};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

const LOOPBACK_HTTP_REQUEST_READ_TIMEOUT: Duration = Duration::from_secs(5);
const LOOPBACK_HTTP_REJECTION_WRITE_TIMEOUT: Duration = Duration::from_millis(250);
const CLI_DAEMON_SEND_CONNECT_TIMEOUT: Duration = Duration::from_millis(500);
// Real provider-backed turns can exceed 30 seconds during remote delivery.
const CLI_DAEMON_SEND_READ_TIMEOUT: Duration = Duration::from_secs(180);
const CLI_DAEMON_SEND_WRITE_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Debug, Deserialize)]
struct DaemonFirstHealthEnvelope {
    daemon: DaemonFirstHealthStatus,
}

#[derive(Debug, Deserialize)]
struct DaemonFirstHealthStatus {
    message: String,
    state: crate::core::DaemonState,
    live_probe_attempted: bool,
    process_supervisor_wired: bool,
    process_exists: Option<bool>,
    instance_id_present: bool,
    identity_nonce_ref_present: bool,
    instance_id_matched: bool,
    node_name_matched: bool,
    version_matched: bool,
    process_identity_verified: bool,
    identity_mismatch: Option<String>,
    live_probe_error: Option<String>,
    running: bool,
    implemented: bool,
}

struct LoopbackWorkerPermit {
    active_workers: Arc<AtomicUsize>,
}

impl Drop for LoopbackWorkerPermit {
    fn drop(&mut self) {
        self.active_workers.fetch_sub(1, Ordering::AcqRel);
    }
}

pub fn execute_from_env() -> Result<String, CamError> {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    execute(&args)
}

pub fn execute(args: &[String]) -> Result<String, CamError> {
    execute_with_provider_router(args, &DefaultProviderRouter)
}

pub fn execute_with_provider_router(
    args: &[String],
    provider_router: &dyn ProviderRouter,
) -> Result<String, CamError> {
    execute_with_provider_router_codex_stdio_factory_peer_inventory_fetcher_and_peer_message_sender(
        args,
        provider_router,
        &boxed_live_codex_stdio_provider_router,
        &boxed_live_codex_stdio_provider_lifecycle_router,
        &SshInventoryFetcher,
        &SshPeerMessageSender,
    )
}

pub fn execute_with_provider_router_and_lifecycle_router(
    args: &[String],
    provider_router: &dyn ProviderRouter,
    provider_lifecycle_router: &dyn ProviderLifecycleRouter,
) -> Result<String, CamError> {
    execute_with_provider_router_lifecycle_router_codex_stdio_factory_peer_inventory_fetcher_and_peer_message_sender(
        args,
        provider_router,
        provider_lifecycle_router,
        &boxed_live_codex_stdio_provider_router,
        &boxed_live_codex_stdio_provider_lifecycle_router,
        &SshInventoryFetcher,
        &SshPeerMessageSender,
    )
}

pub fn execute_with_provider_transcript_reader(
    args: &[String],
    provider_transcript_reader: &dyn ProviderTranscriptReader,
) -> Result<String, CamError> {
    execute_with_provider_router_lifecycle_router_transcript_reader_codex_stdio_factory_peer_inventory_fetcher_and_peer_message_sender(
        args,
        &DefaultProviderRouter,
        &DefaultProviderRouter,
        provider_transcript_reader,
        &boxed_live_codex_stdio_provider_router,
        &boxed_live_codex_stdio_provider_lifecycle_router,
        &SshInventoryFetcher,
        &SshPeerMessageSender,
    )
}

pub fn execute_with_provider_router_and_codex_stdio_factory(
    args: &[String],
    provider_router: &dyn ProviderRouter,
    codex_stdio_factory: &dyn Fn(Option<String>) -> Result<Box<dyn ProviderRouter>, CamError>,
) -> Result<String, CamError> {
    execute_with_provider_router_codex_stdio_factory_peer_inventory_fetcher_and_peer_message_sender(
        args,
        provider_router,
        codex_stdio_factory,
        &boxed_live_codex_stdio_provider_lifecycle_router,
        &SshInventoryFetcher,
        &SshPeerMessageSender,
    )
}

pub fn execute_with_provider_router_codex_stdio_factory_and_peer_inventory_fetcher(
    args: &[String],
    provider_router: &dyn ProviderRouter,
    codex_stdio_factory: &dyn Fn(Option<String>) -> Result<Box<dyn ProviderRouter>, CamError>,
    peer_inventory_fetcher: &dyn PeerInventoryFetcher,
) -> Result<String, CamError> {
    execute_with_provider_router_codex_stdio_factory_peer_inventory_fetcher_and_peer_message_sender(
        args,
        provider_router,
        codex_stdio_factory,
        &boxed_live_codex_stdio_provider_lifecycle_router,
        peer_inventory_fetcher,
        &SshPeerMessageSender,
    )
}

pub fn execute_with_provider_router_codex_stdio_factory_peer_inventory_fetcher_and_peer_message_sender(
    args: &[String],
    provider_router: &dyn ProviderRouter,
    codex_stdio_factory: &dyn Fn(Option<String>) -> Result<Box<dyn ProviderRouter>, CamError>,
    codex_stdio_lifecycle_factory: &dyn Fn(
        Option<String>,
    )
        -> Result<Box<dyn ProviderLifecycleRouter>, CamError>,
    peer_inventory_fetcher: &dyn PeerInventoryFetcher,
    peer_message_sender: &dyn PeerMessageSender,
) -> Result<String, CamError> {
    execute_with_provider_router_lifecycle_router_codex_stdio_factory_peer_inventory_fetcher_and_peer_message_sender(
        args,
        provider_router,
        &DefaultProviderRouter,
        codex_stdio_factory,
        codex_stdio_lifecycle_factory,
        peer_inventory_fetcher,
        peer_message_sender,
    )
}

fn execute_with_provider_router_lifecycle_router_codex_stdio_factory_peer_inventory_fetcher_and_peer_message_sender(
    args: &[String],
    provider_router: &dyn ProviderRouter,
    provider_lifecycle_router: &dyn ProviderLifecycleRouter,
    codex_stdio_factory: &dyn Fn(Option<String>) -> Result<Box<dyn ProviderRouter>, CamError>,
    codex_stdio_lifecycle_factory: &dyn Fn(
        Option<String>,
    )
        -> Result<Box<dyn ProviderLifecycleRouter>, CamError>,
    peer_inventory_fetcher: &dyn PeerInventoryFetcher,
    peer_message_sender: &dyn PeerMessageSender,
) -> Result<String, CamError> {
    execute_with_provider_router_lifecycle_router_transcript_reader_codex_stdio_factory_peer_inventory_fetcher_and_peer_message_sender(
        args,
        provider_router,
        provider_lifecycle_router,
        &DefaultProviderRouter,
        codex_stdio_factory,
        codex_stdio_lifecycle_factory,
        peer_inventory_fetcher,
        peer_message_sender,
    )
}

fn execute_with_provider_router_lifecycle_router_transcript_reader_codex_stdio_factory_peer_inventory_fetcher_and_peer_message_sender(
    args: &[String],
    provider_router: &dyn ProviderRouter,
    provider_lifecycle_router: &dyn ProviderLifecycleRouter,
    provider_transcript_reader: &dyn ProviderTranscriptReader,
    codex_stdio_factory: &dyn Fn(Option<String>) -> Result<Box<dyn ProviderRouter>, CamError>,
    codex_stdio_lifecycle_factory: &dyn Fn(
        Option<String>,
    )
        -> Result<Box<dyn ProviderLifecycleRouter>, CamError>,
    peer_inventory_fetcher: &dyn PeerInventoryFetcher,
    peer_message_sender: &dyn PeerMessageSender,
) -> Result<String, CamError> {
    let parsed = ParsedArgs::parse(args)?;
    match parsed.command.as_slice() {
        [] => Ok(help()),
        [command] if command == "help" || command == "--help" || command == "-h" => Ok(help()),
        [command] if command == "init" => init(parsed.home),
        [command] if command == "doctor" => doctor(parsed.home),
        [scope, command, rest @ ..] if scope == "daemon" && command == "status" => {
            daemon_status(parsed.home, rest)
        }
        [scope, command, rest @ ..] if scope == "daemon" && command == "health" => {
            daemon_health(parsed.home, rest)
        }
        [scope, command, rest @ ..] if scope == "daemon" && command == "status-ui" => {
            daemon_status_ui(parsed.home, rest)
        }
        [scope, command, rest @ ..] if scope == "daemon" && command == "configure" => {
            daemon_configure(parsed.home, rest)
        }
        [scope, command, rest @ ..] if scope == "daemon" && command == "start" => {
            daemon_start(parsed.home, rest)
        }
        [scope, command, rest @ ..] if scope == "daemon" && command == "run" => {
            daemon_run(parsed.home, rest)
        }
        [scope, command, rest @ ..] if scope == "daemon" && command == "stop" => {
            daemon_stop(parsed.home, rest)
        }
        [command, rest @ ..] if command == "logs" => logs(parsed.home, rest),
        [scope, command, rest @ ..] if scope == "agent" && command == "create" => {
            agent_create(parsed.home, rest)
        }
        [scope, command, rest @ ..] if scope == "agent" && command == "list" => {
            agent_list(parsed.home, rest)
        }
        [scope, command, rest @ ..] if scope == "agent" && command == "status" => {
            agent_status(parsed.home, rest)
        }
        [scope, command, rest @ ..] if scope == "agent" && command == "resume" => agent_resume(
            parsed.home,
            rest,
            provider_lifecycle_router,
            codex_stdio_lifecycle_factory,
        ),
        [scope, command, rest @ ..] if scope == "agent" && command == "read" => {
            agent_read(parsed.home, rest, provider_transcript_reader)
        }
        [scope, command, rest @ ..] if scope == "agent" && command == "set-model" => {
            agent_set_model(parsed.home, rest)
        }
        [command, rest @ ..] if command == "send" => send(
            parsed.home,
            rest,
            provider_router,
            codex_stdio_factory,
            peer_message_sender,
        ),
        [command, rest @ ..] if command == "inbox" => inbox(parsed.home, rest),
        [scope, command, rest @ ..] if scope == "discover" && command == "local" => {
            discover_local_with_args(parsed.home, rest)
        }
        [scope, command, rest @ ..] if scope == "peer" && command == "add" => {
            peer_add(parsed.home, rest)
        }
        [scope, command, rest @ ..] if scope == "peer" && command == "list" => {
            peer_list(parsed.home, rest)
        }
        [scope, command, rest @ ..] if scope == "peer" && command == "sync" => {
            peer_sync(parsed.home, rest, peer_inventory_fetcher)
        }
        [scope, provider, command, rest @ ..]
            if scope == "provider" && provider == "codex" && command == "probe" =>
        {
            provider_codex_probe(parsed.home, rest)
        }
        [scope, command, rest @ ..] if scope == "inventory" && command == "export" => {
            inventory_export(parsed.home, rest)
        }
        _ => Err(CamError::InvalidCommand(format!(
            "unknown command `{}`",
            parsed.command.join(" ")
        ))),
    }
}

fn init(home: PathBuf) -> Result<String, CamError> {
    let store = StateStore::new(home.clone());
    let state = store.init()?;
    let logger = StructuredLogger::new(&store);
    logger.command_started("init")?;
    logger.event(
        "state.initialized",
        "initialized memory-first state files",
        serde_json::to_value(services::state_initialized_event_fields(
            store.home().display().to_string(),
            &state,
        ))?,
    )?;
    logger.command_finished("init")?;

    let summary = services::state_initialized_summary(store.home().display().to_string(), &state);
    Ok(format!(
        "initialized qexow-cam state\nhome: {}\nagents: {}",
        summary.home, summary.agents
    ))
}

fn doctor(home: PathBuf) -> Result<String, CamError> {
    let store = StateStore::new(home.clone());
    let checks = store.doctor_checks();
    let files_ok = checks.iter().all(|check| check.ok);
    let mut lines = vec!["qexow-cam doctor".to_string()];
    let provider_diagnostics = run_doctor_provider_diagnostics(None);

    for check in checks {
        let status = if check.ok { "OK" } else { "MISSING" };
        lines.push(format!(
            "{status:<7} {:<16} {}",
            check.name,
            check.path.display()
        ));
    }

    append_doctor_provider_diagnostic_lines(&mut lines, &provider_diagnostics);

    if !files_ok {
        lines.push("result: missing state; run `qexow-cam init`".to_string());
        return Ok(lines.join("\n"));
    }

    let logger = StructuredLogger::new(&store);
    logger.command_started("doctor")?;
    match store.load_existing() {
        Ok(state) => {
            match services::validate_state_invariants(&state).and_then(|_| {
                store.validate_local_api_token()?;
                let events = read_events(&store, None)?;
                let daemon_events = read_daemon_log_events(&store, None)?;
                validate_event_log_invariants(&state, &events)?;
                validate_event_log_invariants(&state, &daemon_events)?;
                validate_event_mirror(&events, &daemon_events)
            }) {
                Ok(()) => {
                    logger.event(
                        "doctor.providers.checked",
                        "doctor checked local provider command availability",
                        serde_json::to_value(&provider_diagnostics)?,
                    )?;
                    lines.push("OK      state            typed state loaded".to_string());
                    lines.push("OK      events           event JSONL parsed".to_string());
                    lines.push("OK      daemon-log       daemon log mirrors events".to_string());
                    let daemon_status = services::daemon_status(&state);
                    lines.push(format!(
                        "INFO    daemon           observed={} desired={} running={} implemented={} live_probe_attempted={} process_supervisor_wired={} process_exists={:?} instance_id_present={} identity_nonce_ref_present={} process_identity_verified={} state_source={}",
                        services::daemon_observed_state_label(&daemon_status.state.observed_state),
                        services::daemon_desired_state_label(&daemon_status.state.desired_state),
                        daemon_status.running,
                        daemon_status.implemented,
                        daemon_status.live_probe_attempted,
                        daemon_status.process_supervisor_wired,
                        daemon_status.process_exists,
                        daemon_status.instance_id_present,
                        daemon_status.identity_nonce_ref_present,
                        daemon_status.process_identity_verified,
                        daemon_status.state_source
                    ));
                    let surface = surface_for_state(&state);
                    lines.push(format!(
                        "INFO    local-status     bind={} port={} loopback_only={} mutation_routes_enabled={} public_network_enabled={}",
                        surface.bind,
                        surface.port,
                        surface.loopback_only,
                        surface.mutation_routes_enabled,
                        surface.public_network_enabled
                    ));
                    lines.push(format!("INFO    agents           {}", state.agents.len()));
                    lines.push(format!("INFO    peers            {}", state.peers.len()));
                    lines.push(format!("INFO    mailbox          {}", state.mailbox.len()));
                    logger.command_finished("doctor")?;
                    lines.push("result: ok".to_string());
                }
                Err(error) => {
                    logger.event(
                        "doctor.failed",
                        "doctor found invalid state invariants or event log",
                        serde_json::to_value(services::doctor_failed_event_fields(&error))?,
                    )?;
                    lines.push(format!("BAD     state            {error}"));
                    lines.push(
                        "result: invalid state; inspect JSON files and repair before continuing"
                            .to_string(),
                    );
                }
            }
        }
        Err(error) => {
            logger.event(
                "doctor.failed",
                "doctor found invalid persisted state",
                serde_json::to_value(services::doctor_failed_event_fields(&error))?,
            )?;
            lines.push(format!("BAD     state            {error}"));
            lines.push(
                "result: invalid state; inspect JSON files and repair before continuing"
                    .to_string(),
            );
        }
    }
    Ok(lines.join("\n"))
}

#[derive(Debug, serde::Serialize)]
struct DoctorProviderDiagnostics {
    codex_program: String,
    codex_cli_probe_attempted: bool,
    codex_cli_available: bool,
    codex_cli_version: Option<String>,
    codex_cli_error: Option<String>,
    codex_auth_probe_attempted: bool,
    codex_auth_available: Option<bool>,
    codex_auth_status: Option<String>,
    codex_auth_error: Option<String>,
    codex_app_server_stdio_probe_attempted: bool,
    codex_app_server_initialized: bool,
    codex_app_server_error: Option<String>,
    agy_delivery_primitive_verified: bool,
}

struct TimedCommandOutput {
    status_success: bool,
    stdout: String,
    stderr: String,
    timed_out: bool,
}

fn run_doctor_provider_diagnostics(codex_program: Option<String>) -> DoctorProviderDiagnostics {
    let codex_program = codex_program.unwrap_or_else(|| "codex".to_string());
    let version_probe = run_timed_command(&codex_program, &["--version"], Duration::from_secs(10));
    let (codex_cli_available, codex_cli_version, codex_cli_error) = match version_probe {
        Ok(output) if output.status_success => (
            true,
            first_nonempty_line(&output.stdout)
                .or_else(|| first_nonempty_line(&output.stderr))
                .or_else(|| Some("version command succeeded without version text".to_string())),
            None,
        ),
        Ok(output) => (
            false,
            None,
            Some(short_command_error(
                "`codex --version`",
                &output,
                "Codex CLI version command failed",
            )),
        ),
        Err(error) => (false, None, Some(error.to_string())),
    };

    let auth_probe =
        run_timed_command(&codex_program, &["auth", "status"], Duration::from_secs(10));
    let (codex_auth_available, codex_auth_status, codex_auth_error) = match auth_probe {
        Ok(output) if output.status_success => (
            Some(true),
            first_nonempty_line(&output.stdout)
                .or_else(|| first_nonempty_line(&output.stderr))
                .or_else(|| Some("auth status command succeeded".to_string())),
            None,
        ),
        Ok(output) => (
            Some(false),
            first_nonempty_line(&output.stdout).or_else(|| first_nonempty_line(&output.stderr)),
            Some(short_command_error(
                "`codex auth status`",
                &output,
                "Codex auth status command failed",
            )),
        ),
        Err(error) => (Some(false), None, Some(error.to_string())),
    };

    let app_server_probe = codex::probe_app_server(Some(codex_program.clone()));

    DoctorProviderDiagnostics {
        codex_program,
        codex_cli_probe_attempted: true,
        codex_cli_available,
        codex_cli_version,
        codex_cli_error,
        codex_auth_probe_attempted: true,
        codex_auth_available,
        codex_auth_status,
        codex_auth_error,
        codex_app_server_stdio_probe_attempted: app_server_probe.initialize_attempted,
        codex_app_server_initialized: app_server_probe.initialized,
        codex_app_server_error: app_server_probe.error,
        agy_delivery_primitive_verified: false,
    }
}

fn append_doctor_provider_diagnostic_lines(
    lines: &mut Vec<String>,
    diagnostics: &DoctorProviderDiagnostics,
) {
    let cli_status = if diagnostics.codex_cli_available {
        "OK"
    } else {
        "BAD"
    };
    lines.push(format!(
        "{cli_status:<7} {:<16} program={} version={} error={}",
        "codex-cli",
        diagnostics.codex_program,
        diagnostics.codex_cli_version.as_deref().unwrap_or("-"),
        diagnostics.codex_cli_error.as_deref().unwrap_or("-")
    ));

    let auth_status = match diagnostics.codex_auth_available {
        Some(true) => "OK",
        Some(false) => "WARN",
        None => "WARN",
    };
    lines.push(format!(
        "{auth_status:<7} {:<16} attempted={} status={} error={}",
        "codex-auth",
        diagnostics.codex_auth_probe_attempted,
        diagnostics.codex_auth_status.as_deref().unwrap_or("-"),
        diagnostics.codex_auth_error.as_deref().unwrap_or("-")
    ));

    let app_server_status = if diagnostics.codex_app_server_initialized {
        "OK"
    } else {
        "BAD"
    };
    lines.push(format!(
        "{app_server_status:<7} {:<16} attempted={} initialized={} error={}",
        "codex-stdio",
        diagnostics.codex_app_server_stdio_probe_attempted,
        diagnostics.codex_app_server_initialized,
        diagnostics.codex_app_server_error.as_deref().unwrap_or("-")
    ));

    lines.push(format!(
        "INFO    {:<16} default=real_codex_app_server_when_available agy_delivery_primitive_verified={}",
        "providers",
        diagnostics.agy_delivery_primitive_verified
    ));
}

fn run_timed_command(
    program: &str,
    args: &[&str],
    timeout: Duration,
) -> Result<TimedCommandOutput, CamError> {
    let mut child = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| {
            CamError::DeliveryFailed(format!(
                "failed to spawn command `{}`: {error}",
                command_display(program, args)
            ))
        })?;
    let deadline = Instant::now() + timeout;
    let mut timed_out = false;

    loop {
        if child
            .try_wait()
            .map_err(|error| {
                CamError::DeliveryFailed(format!(
                    "failed to poll command `{}`: {error}",
                    command_display(program, args)
                ))
            })?
            .is_some()
        {
            break;
        }
        if Instant::now() >= deadline {
            timed_out = true;
            let _ = child.kill();
            break;
        }
        thread::sleep(Duration::from_millis(50));
    }

    let output = child.wait_with_output().map_err(|error| {
        CamError::DeliveryFailed(format!(
            "failed to collect command `{}` output: {error}",
            command_display(program, args)
        ))
    })?;
    Ok(TimedCommandOutput {
        status_success: output.status.success() && !timed_out,
        stdout: String::from_utf8_lossy(&output.stdout).to_string(),
        stderr: String::from_utf8_lossy(&output.stderr).to_string(),
        timed_out,
    })
}

fn command_display(program: &str, args: &[&str]) -> String {
    let mut parts = vec![program.to_string()];
    parts.extend(args.iter().map(|arg| arg.to_string()));
    parts.join(" ")
}

fn first_nonempty_line(value: &str) -> Option<String> {
    value
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map(str::to_string)
}

fn short_command_error(command: &str, output: &TimedCommandOutput, fallback: &str) -> String {
    if output.timed_out {
        return format!("{command} timed out");
    }
    first_nonempty_line(&output.stderr)
        .or_else(|| first_nonempty_line(&output.stdout))
        .unwrap_or_else(|| fallback.to_string())
}

fn daemon_status(home: PathBuf, args: &[String]) -> Result<String, CamError> {
    let options = CommandOptions::parse(args, &[], &[])?;
    if !options.positionals.is_empty() {
        return Err(CamError::InvalidCommand(
            "daemon status does not accept positional arguments".to_string(),
        ));
    }

    let store = StateStore::new(home.clone());
    let state = store.load_existing()?;
    let logger = StructuredLogger::new(&store);
    logger.command_started("daemon status")?;
    let status = match try_daemon_first_status(&store, &state) {
        Ok(Some(status)) => status,
        Ok(None) => services::daemon_status(&state),
        Err(error) => {
            logger.command_failed("daemon status", &error)?;
            return Err(error);
        }
    };
    logger.event(
        "daemon.status.checked",
        if status.process_identity_verified {
            "daemon status checked using live loopback daemon identity"
        } else {
            "daemon status checked without treating persisted state as liveness proof"
        },
        serde_json::to_value(services::daemon_status_checked_event_fields(&status))?,
    )?;
    logger.command_finished("daemon status")?;
    Ok(serde_json::to_string_pretty(&status)?)
}

fn daemon_health(home: PathBuf, args: &[String]) -> Result<String, CamError> {
    let options = CommandOptions::parse(args, &[], &[])?;
    if !options.positionals.is_empty() {
        return Err(CamError::InvalidCommand(
            "daemon health does not accept positional arguments".to_string(),
        ));
    }

    let store = StateStore::new(home);
    let state = store.load_existing()?;
    let logger = StructuredLogger::new(&store);
    logger.command_started("daemon health")?;
    match try_daemon_first_health(&store, &logger, &state) {
        Ok(Some(result)) => {
            logger.command_finished("daemon health")?;
            return Ok(result);
        }
        Ok(None) => {}
        Err(error) => {
            logger.command_failed("daemon health", &error)?;
            return Err(error);
        }
    }
    let status = services::daemon_status(&state);
    let response = render_health_response_with_status(&state, &status);
    let surface = surface_for_state(&state);
    logger.event(
        "daemon.health.rendered",
        "rendered read-only loopback health response without claiming process liveness",
        serde_json::to_value(services::daemon_health_rendered_event_fields(
            &response, &surface, &status,
        ))?,
    )?;
    logger.command_finished("daemon health")?;
    Ok(serde_json::to_string_pretty(&response)?)
}

fn daemon_status_ui(home: PathBuf, args: &[String]) -> Result<String, CamError> {
    let options = CommandOptions::parse(args, &[], &[])?;
    if !options.positionals.is_empty() {
        return Err(CamError::InvalidCommand(
            "daemon status-ui does not accept positional arguments".to_string(),
        ));
    }

    let store = StateStore::new(home);
    let state = store.load_existing()?;
    let logger = StructuredLogger::new(&store);
    logger.command_started("daemon status-ui")?;
    let response = render_status_ui_response(&state);
    let surface = surface_for_state(&state);
    logger.event(
        "daemon.status_ui.rendered",
        "rendered read-only local status UI response",
        serde_json::to_value(services::daemon_status_ui_rendered_event_fields(
            &response,
            &surface,
            &state.daemon,
        ))?,
    )?;
    logger.command_finished("daemon status-ui")?;
    Ok(serde_json::to_string_pretty(&response)?)
}

fn daemon_configure(home: PathBuf, args: &[String]) -> Result<String, CamError> {
    let options = CommandOptions::parse(args, &["bind", "port"], &[])?;
    if !options.positionals.is_empty() {
        return Err(CamError::InvalidCommand(
            "daemon configure does not accept positional arguments".to_string(),
        ));
    }
    if options.values.is_empty() {
        return Err(CamError::InvalidCommand(
            "daemon configure requires --bind and/or --port".to_string(),
        ));
    }

    let store = StateStore::new(home.clone());
    let mut state = store.load_existing()?;
    let logger = StructuredLogger::new(&store);
    logger.command_started("daemon configure")?;
    if state.daemon.observed_state == crate::core::DaemonObservedState::Running {
        let error = CamError::InvalidState(
            "refusing to change daemon endpoint while persisted daemon state is running"
                .to_string(),
        );
        logger.command_failed("daemon configure", &error)?;
        return Err(error);
    }
    if let Some(bind) = options.value("bind") {
        if !crate::local_status::is_loopback_bind(bind) {
            let error = CamError::InvalidCommand(format!(
                "daemon bind `{bind}` is not loopback; daemon configure only accepts loopback binds"
            ));
            logger.command_failed("daemon configure", &error)?;
            return Err(error);
        }
        state.daemon.bind = bind.clone();
    }
    if let Some(port) = options.value("port") {
        state.daemon.port = match parse_daemon_port(port) {
            Ok(port) => port,
            Err(error) => {
                logger.command_failed("daemon configure", &error)?;
                return Err(error);
            }
        };
    }
    state.daemon.updated_at = crate::core::now_utc();
    store.save_daemon(&state.daemon)?;
    logger.event(
        "daemon.endpoint.configured",
        "daemon loopback endpoint configured",
        serde_json::json!({
            "bind": state.daemon.bind,
            "port": state.daemon.port,
            "loopback_only": true,
        }),
    )?;
    logger.command_finished("daemon configure")?;
    Ok(serde_json::to_string_pretty(&state.daemon)?)
}

fn daemon_start(home: PathBuf, args: &[String]) -> Result<String, CamError> {
    let options = CommandOptions::parse(
        args,
        &["port", "identity-challenge"],
        &["headless", "foreground"],
    )?;
    if !options.positionals.is_empty() {
        return Err(CamError::InvalidCommand(
            "daemon start does not accept positional arguments".to_string(),
        ));
    }

    let store = StateStore::new(home.clone());
    let mut state = store.load_existing()?;
    let logger = StructuredLogger::new(&store);
    logger.command_started("daemon start")?;
    let headless = options.has_flag("headless") || state.config.headless;
    if let Some(port) = options.value("port") {
        state.daemon.port = match parse_daemon_port(port) {
            Ok(port) => port,
            Err(error) => {
                logger.command_failed("daemon start", &error)?;
                return Err(error);
            }
        };
    }
    let instance_id = services::new_daemon_instance_id();
    let (identity_nonce_ref, _nonce) = store.create_daemon_identity_nonce(&instance_id)?;
    let identity_challenge = options
        .value("identity-challenge")
        .cloned()
        .unwrap_or_else(|| instance_id.clone());
    let foreground = options.has_flag("foreground");
    let _status = services::request_daemon_start(
        &mut state,
        headless,
        instance_id.clone(),
        identity_nonce_ref.clone(),
    );
    store.save_daemon(&state.daemon)?;

    if foreground {
        start_daemon_foreground(
            store,
            logger,
            state,
            headless,
            instance_id,
            identity_nonce_ref,
            identity_challenge,
            "daemon start",
            "daemon.start.foreground",
            "daemon start entered foreground loopback HTTP runtime",
        )
    } else {
        let child = spawn_background_daemon_process(
            &home,
            state.daemon.port,
            headless,
            &instance_id,
            &identity_nonce_ref,
            &identity_challenge,
        )?;
        wait_for_daemon_ready(&store, &state, child, "daemon start")?;
        let refreshed = store.load_existing()?;
        let live_status = daemon_live_status_after_ready(&store, &refreshed)?;
        logger.event(
            "daemon.start.requested",
            "daemon start launched a background loopback runtime and returned after readiness",
            serde_json::to_value(services::daemon_status_checked_event_fields(&live_status))?,
        )?;
        logger.command_finished("daemon start")?;
        Ok(serde_json::to_string_pretty(&live_status)?)
    }
}

fn daemon_run(home: PathBuf, args: &[String]) -> Result<String, CamError> {
    let options = CommandOptions::parse(
        args,
        &[
            "instance-id",
            "identity-nonce-ref",
            "identity-challenge",
            "port",
        ],
        &["headless"],
    )?;
    if !options.positionals.is_empty() {
        return Err(CamError::InvalidCommand(
            "daemon run does not accept positional arguments".to_string(),
        ));
    }
    let instance_id = required_option(&options, "instance-id")?.clone();
    let identity_nonce_ref = required_option(&options, "identity-nonce-ref")?.clone();
    let identity_challenge = required_option(&options, "identity-challenge")?.clone();

    let store = StateStore::new(home);
    let mut state = store.load_existing()?;
    if let Some(port) = options.value("port") {
        state.daemon.port = parse_daemon_port(port)?;
    }
    if options.has_flag("headless") {
        state.daemon.headless = true;
    }
    let daemon_headless = state.daemon.headless;
    let logger = StructuredLogger::new(&store);
    logger.command_started("daemon run")?;
    start_daemon_foreground(
        store,
        logger,
        state,
        daemon_headless,
        instance_id,
        identity_nonce_ref,
        identity_challenge,
        "daemon run",
        "daemon.run.foreground",
        "daemon run entered foreground loopback HTTP runtime",
    )
}

fn start_daemon_foreground(
    store: StateStore,
    logger: StructuredLogger,
    mut state: AppState,
    headless: bool,
    instance_id: String,
    identity_nonce_ref: String,
    identity_challenge: String,
    command_name: &str,
    start_event_type: &str,
    start_event_message: &str,
) -> Result<String, CamError> {
    let nonce = store.read_daemon_identity_nonce(&identity_nonce_ref)?;
    let output = services::daemon_run_identity_check(
        &state.daemon,
        &instance_id,
        &identity_nonce_ref,
        &identity_challenge,
        Some(nonce.as_str()),
        true,
        None,
    );
    if !output.ok {
        let error = CamError::InvalidState(format!(
            "{command_name} identity check failed before starting the loopback runtime"
        ));
        logger.event(
            "daemon.run.identity_checked",
            "daemon run identity checked without starting supervisor loop or exposing nonce",
            serde_json::to_value(services::daemon_run_identity_checked_event_fields(&output))?,
        )?;
        logger.command_failed(command_name, &error)?;
        return Err(error);
    }

    let listener = bind_daemon_loopback_listener(&state.daemon.bind, state.daemon.port)?;
    state.daemon.mark_running(
        headless,
        std::process::id(),
        instance_id,
        identity_nonce_ref,
    );
    store.save_daemon(&state.daemon)?;
    logger.event(
        start_event_type,
        start_event_message,
        serde_json::to_value(services::daemon_status(&state))?,
    )?;
    serve_daemon_loop(store, logger, state, listener, command_name)
}

fn serve_daemon_loop(
    store: StateStore,
    logger: StructuredLogger,
    state: AppState,
    listener: TcpListener,
    command_name: &str,
) -> Result<String, CamError> {
    let bind = state.daemon.bind.clone();
    let port = state.daemon.port;
    listener.set_nonblocking(true)?;
    logger.event(
        "daemon.http.listening",
        "daemon loopback HTTP listener is serving",
        serde_json::json!({
            "bind": bind,
            "port": port,
            "headless": state.daemon.headless,
            "pid": std::process::id(),
        }),
    )?;

    let shared_state = Arc::new(Mutex::new(state));
    let shared_runtime = Arc::new(Mutex::new(services::DaemonRuntime::new()));
    let active_workers = Arc::new(AtomicUsize::new(0));
    loop {
        let running_instance_id = shared_state
            .lock()
            .map_err(|_| CamError::InvalidState("daemon state lock was poisoned".to_string()))?
            .daemon
            .instance_id
            .clone();
        if foreground_stop_requested(&store, running_instance_id.as_deref())? {
            logger.event(
                "daemon.stop.observed",
                "daemon foreground loop observed persisted stop request",
                serde_json::json!({
                    "pid": std::process::id(),
                    "instance_id": running_instance_id,
                }),
            )?;
            break;
        }

        let stream = match listener.accept() {
            Ok((stream, _address)) => stream,
            Err(error) if error.kind() == ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(250));
                continue;
            }
            Err(error) => {
                let error = error.to_string();
                logger.event(
                    "daemon.http.accept_failed",
                    "daemon failed to accept loopback HTTP connection",
                    serde_json::json!({
                        "error_kind": "io",
                        "error": error,
                    }),
                )?;
                continue;
            }
        };
        let peer_addr = stream
            .peer_addr()
            .map(|address| address.to_string())
            .unwrap_or_else(|_| "<unknown>".to_string());
        let Some(worker_permit) = try_acquire_loopback_worker(&active_workers) else {
            reject_loopback_worker_limit(stream, &logger, &peer_addr, &active_workers);
            continue;
        };
        let request_store = store.clone();
        let request_logger = logger.clone();
        let request_state = Arc::clone(&shared_state);
        let request_runtime = Arc::clone(&shared_runtime);
        let spawn_error_peer_addr = peer_addr.clone();
        if let Err(error) = thread::Builder::new()
            .name("qexow-cam-loopback-worker".to_string())
            .spawn(move || {
                handle_daemon_loopback_connection(
                    request_store,
                    request_logger,
                    request_state,
                    request_runtime,
                    stream,
                    peer_addr,
                    worker_permit,
                );
            })
        {
            logger.event(
                "daemon.http.worker.rejected",
                "daemon rejected loopback request because worker thread spawn failed",
                serde_json::json!({
                    "peer_addr": spawn_error_peer_addr,
                    "ok": false,
                    "status_code": 503,
                    "active_workers": active_workers.load(Ordering::Acquire),
                    "max_workers": services::MAX_LOOPBACK_WORKERS,
                    "error_kind": "worker_spawn_failed",
                    "error": error.to_string(),
                }),
            )?;
        }
    }

    let mut state = shared_state
        .lock()
        .map_err(|_| CamError::InvalidState("daemon state lock was poisoned".to_string()))?;
    state.daemon.stop_requested();
    store.save_daemon(&state.daemon)?;
    logger.command_finished(command_name)?;
    Ok(serde_json::to_string_pretty(&services::daemon_status(
        &state,
    ))?)
}

fn spawn_background_daemon_process(
    home: &PathBuf,
    port: u16,
    headless: bool,
    instance_id: &str,
    identity_nonce_ref: &str,
    identity_challenge: &str,
) -> Result<Child, CamError> {
    let executable = std::env::current_exe().map_err(|error| {
        CamError::InvalidState(format!("failed to resolve current executable: {error}"))
    })?;
    let mut command = Command::new(executable);
    command
        .arg("--home")
        .arg(home)
        .arg("daemon")
        .arg("run")
        .arg("--instance-id")
        .arg(instance_id)
        .arg("--identity-nonce-ref")
        .arg(identity_nonce_ref)
        .arg("--identity-challenge")
        .arg(identity_challenge);
    if headless {
        command.arg("--headless");
    }
    command.arg("--port").arg(port.to_string());
    command.stdin(Stdio::null());
    command.stdout(Stdio::null());
    command.stderr(Stdio::null());
    command.spawn().map_err(|error| {
        CamError::DeliveryFailed(format!(
            "failed to spawn background daemon process: {error}"
        ))
    })
}

fn wait_for_daemon_ready(
    store: &StateStore,
    state: &AppState,
    mut child: Child,
    command_name: &str,
) -> Result<(), CamError> {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if let Some(status) = child.try_wait().map_err(|error| {
            CamError::DeliveryFailed(format!(
                "{command_name} failed while polling background daemon process: {error}"
            ))
        })? {
            return Err(CamError::DeliveryFailed(format!(
                "{command_name} background daemon process exited early with status {status}"
            )));
        }
        match health_via_daemon_loopback(store, state) {
            Ok(response) if (200..300).contains(&response.status_code) => return Ok(()),
            Ok(_) => {}
            Err(_) => {}
        }
        if Instant::now() >= deadline {
            return Err(CamError::DeliveryFailed(format!(
                "{command_name} timed out waiting for background daemon readiness"
            )));
        }
        thread::sleep(Duration::from_millis(100));
    }
}

fn daemon_live_status_after_ready(
    store: &StateStore,
    state: &AppState,
) -> Result<services::DaemonStatusOutput, CamError> {
    match health_via_daemon_loopback(store, state) {
        Ok(response) if (200..300).contains(&response.status_code) => {
            daemon_status_from_live_health_response(&response.body)
        }
        _ => Ok(services::daemon_status(state)),
    }
}

fn handle_daemon_loopback_connection(
    store: StateStore,
    logger: StructuredLogger,
    state: Arc<Mutex<AppState>>,
    runtime: Arc<Mutex<services::DaemonRuntime>>,
    mut stream: TcpStream,
    peer_addr: String,
    _worker_permit: LoopbackWorkerPermit,
) {
    if let Err(error) = stream.set_nonblocking(false) {
        let _ = logger.event(
            "daemon.http.read_failed",
            "daemon failed to configure loopback HTTP request blocking mode",
            serde_json::json!({
                "peer_addr": peer_addr,
                "phase": "configure_blocking_mode",
                "error_kind": "io",
                "error": error.to_string(),
            }),
        );
        return;
    }
    if let Err(error) = stream.set_read_timeout(Some(LOOPBACK_HTTP_REQUEST_READ_TIMEOUT)) {
        let _ = logger.event(
            "daemon.http.read_failed",
            "daemon failed to configure loopback HTTP request read timeout",
            serde_json::json!({
                "peer_addr": peer_addr,
                "phase": "configure_read_timeout",
                "error_kind": "io",
                "error": error.to_string(),
            }),
        );
        return;
    }
    let request = match read_loopback_http_request(&mut stream) {
        Ok(request) => request,
        Err(error) => {
            let _ = logger.event(
                "daemon.http.read_failed",
                "daemon failed to read loopback HTTP request",
                serde_json::json!({
                    "peer_addr": peer_addr,
                    "phase": "read_request",
                    "error_kind": error.kind(),
                    "error": error.to_string(),
                }),
            );
            return;
        }
    };
    match state.lock() {
        Ok(mut state) => {
            state.daemon.heartbeat();
        }
        Err(_) => {
            let _ = logger.event(
                "daemon.http.read_failed",
                "daemon state lock was poisoned after loopback request read",
                serde_json::json!({
                    "peer_addr": peer_addr,
                    "phase": "lock_state",
                    "error_kind": "invalid_state",
                    "error": "daemon state lock was poisoned",
                }),
            );
            return;
        }
    }
    let response = if request.starts_with("GET /health ")
        || request.starts_with("GET /health?")
        || request.starts_with("GET /status-ui ")
        || request.starts_with("GET /status-ui?")
    {
        match state.lock() {
            Ok(state) => {
                crate::local_status::handle_loopback_http_request(&state, &peer_addr, &request)
            }
            Err(_) => crate::local_status::LocalStatusResponse::internal_server_error_text(
                "daemon state lock was poisoned before local status request",
            ),
        }
    } else {
        crate::local_api::handle_local_api_http_request_with_shared_state(
            &store, &logger, &state, &runtime, &peer_addr, &request,
        )
    };
    if let Err(error) = write_loopback_http_response(&mut stream, &response) {
        let _ = logger.event(
            "daemon.http.response_write_failed",
            "daemon failed to write loopback HTTP response",
            serde_json::json!({
                "peer_addr": peer_addr,
                "status_code": response.status_code,
                "content_type": response.content_type,
                "error_kind": "io",
                "error": error.to_string(),
            }),
        );
    }
}

fn try_acquire_loopback_worker(active_workers: &Arc<AtomicUsize>) -> Option<LoopbackWorkerPermit> {
    let mut current = active_workers.load(Ordering::Acquire);
    while current < services::MAX_LOOPBACK_WORKERS {
        match active_workers.compare_exchange_weak(
            current,
            current + 1,
            Ordering::AcqRel,
            Ordering::Acquire,
        ) {
            Ok(_) => {
                return Some(LoopbackWorkerPermit {
                    active_workers: Arc::clone(active_workers),
                });
            }
            Err(actual) => current = actual,
        }
    }
    None
}

fn reject_loopback_worker_limit(
    mut stream: TcpStream,
    logger: &StructuredLogger,
    peer_addr: &str,
    active_workers: &AtomicUsize,
) {
    let active_worker_count = active_workers.load(Ordering::Acquire);
    let response = crate::local_status::LocalStatusResponse::service_unavailable_text(
        "loopback worker limit reached; retry shortly",
    );
    let _ = logger.event(
        "daemon.http.worker.rejected",
        "daemon rejected loopback request because worker limit was reached",
        serde_json::json!({
            "peer_addr": peer_addr,
            "ok": false,
            "status_code": response.status_code,
            "active_workers": active_worker_count,
            "max_workers": services::MAX_LOOPBACK_WORKERS,
            "error_kind": "worker_limit_reached",
            "error": "loopback worker limit reached",
        }),
    );
    let _ = stream.set_write_timeout(Some(LOOPBACK_HTTP_REJECTION_WRITE_TIMEOUT));
    if let Err(error) = write_loopback_http_response(&mut stream, &response) {
        let _ = logger.event(
            "daemon.http.response_write_failed",
            "daemon failed to write loopback worker-limit response",
            serde_json::json!({
                "peer_addr": peer_addr,
                "status_code": response.status_code,
                "content_type": response.content_type,
                "error_kind": "io",
                "error": error.to_string(),
            }),
        );
    }
}

fn write_loopback_http_response(
    stream: &mut TcpStream,
    response: &crate::local_status::LocalStatusResponse,
) -> Result<(), std::io::Error> {
    stream.write_all(response.to_http_response().as_bytes())?;
    stream.flush()
}

fn bind_daemon_loopback_listener(bind: &str, port: u16) -> Result<TcpListener, CamError> {
    if !crate::local_status::is_loopback_bind(bind) {
        return Err(CamError::InvalidState(format!(
            "daemon bind `{bind}` is not loopback; refusing to expose CAM daemon"
        )));
    }
    validate_daemon_port(port)?;
    Ok(TcpListener::bind((bind, port))?)
}

fn foreground_stop_requested(
    store: &StateStore,
    running_instance_id: Option<&str>,
) -> Result<bool, CamError> {
    let daemon = store.load_daemon()?;
    if daemon.desired_state != crate::core::DaemonDesiredState::Stopped {
        return Ok(false);
    }
    if running_instance_id.is_none() {
        return Ok(true);
    }
    Ok(daemon.instance_id.as_deref().is_none()
        || daemon.instance_id.as_deref() == running_instance_id)
}

fn read_loopback_http_request(stream: &mut TcpStream) -> Result<String, CamError> {
    const MAX_REQUEST_BYTES: usize = 1024 * 1024;
    let deadline = Instant::now() + LOOPBACK_HTTP_REQUEST_READ_TIMEOUT;
    let mut buffer = Vec::new();
    let mut chunk = [0_u8; 8192];
    loop {
        let bytes_read = match stream.read(&mut chunk) {
            Ok(bytes_read) => bytes_read,
            Err(error) if error.kind() == ErrorKind::WouldBlock => {
                if Instant::now() >= deadline {
                    return Err(CamError::DeliveryFailed(
                        "timed out waiting for loopback HTTP request bytes".to_string(),
                    ));
                }
                thread::sleep(Duration::from_millis(10));
                continue;
            }
            Err(error) => return Err(CamError::from(error)),
        };
        if bytes_read == 0 {
            break;
        }
        buffer.extend_from_slice(&chunk[..bytes_read]);
        if buffer.len() > MAX_REQUEST_BYTES {
            return Err(CamError::InvalidCommand(
                "local HTTP request exceeded 1 MiB limit".to_string(),
            ));
        }
        if http_request_complete(&buffer)? {
            break;
        }
    }
    Ok(String::from_utf8_lossy(&buffer).to_string())
}

fn http_request_complete(buffer: &[u8]) -> Result<bool, CamError> {
    let Some(header_end) = find_header_end(buffer) else {
        return Ok(false);
    };
    let headers = String::from_utf8_lossy(&buffer[..header_end]).to_ascii_lowercase();
    let content_length = headers
        .lines()
        .find_map(|line| line.strip_prefix("content-length:"))
        .map(str::trim)
        .map(|value| {
            value.parse::<usize>().map_err(|_| {
                CamError::InvalidCommand("invalid local HTTP Content-Length".to_string())
            })
        })
        .transpose()?
        .unwrap_or(0);
    Ok(buffer.len() >= header_end + http_header_separator_len(buffer, header_end) + content_length)
}

fn find_header_end(buffer: &[u8]) -> Option<usize> {
    buffer
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .or_else(|| buffer.windows(2).position(|window| window == b"\n\n"))
}

fn http_header_separator_len(buffer: &[u8], header_end: usize) -> usize {
    if buffer
        .get(header_end..header_end + 4)
        .is_some_and(|bytes| bytes == b"\r\n\r\n")
    {
        4
    } else {
        2
    }
}

fn daemon_stop(home: PathBuf, args: &[String]) -> Result<String, CamError> {
    let options = CommandOptions::parse(args, &[], &[])?;
    if !options.positionals.is_empty() {
        return Err(CamError::InvalidCommand(
            "daemon stop does not accept positional arguments".to_string(),
        ));
    }

    let store = StateStore::new(home);
    let mut state = store.load_existing()?;
    let logger = StructuredLogger::new(&store);
    logger.command_started("daemon stop")?;
    let status = services::request_daemon_stop(&mut state);
    store.save_daemon(&state.daemon)?;
    logger.event(
        "daemon.stop.requested",
        "daemon stop requested; foreground daemon will observe persisted desired state",
        serde_json::to_value(services::daemon_stop_requested_event_fields(&status))?,
    )?;
    logger.command_finished("daemon stop")?;
    Ok(serde_json::to_string_pretty(&status)?)
}

fn logs(home: PathBuf, args: &[String]) -> Result<String, CamError> {
    let options = CommandOptions::parse(args, &["limit"], &["json"])?;
    if !options.positionals.is_empty() {
        return Err(CamError::InvalidCommand(
            "logs does not accept positional arguments".to_string(),
        ));
    }

    let limit = options
        .value("limit")
        .map(|value| services::parse_log_limit(value))
        .transpose()?;
    let store = StateStore::new(home);
    let events = services::read_logs(&store, limit)?;

    if options.has_flag("json") {
        return Ok(serde_json::to_string_pretty(&events)?);
    }

    Ok(format_log_events(&services::log_list_rows(&events)))
}

fn agent_list(home: PathBuf, args: &[String]) -> Result<String, CamError> {
    let options = CommandOptions::parse(args, &[], &["json"])?;
    if !options.positionals.is_empty() {
        return Err(CamError::InvalidCommand(
            "agent list does not accept positional arguments".to_string(),
        ));
    }

    let store = StateStore::new(home);
    let state = store.load_existing()?;
    let logger = StructuredLogger::new(&store);
    logger.command_started("agent list")?;
    logger.command_finished("agent list")?;
    let rows = services::list_agent_rows(&state);

    if options.has_flag("json") {
        return Ok(serde_json::to_string_pretty(&rows)?);
    }

    Ok(format_agent_rows(&rows))
}

fn agent_status(home: PathBuf, args: &[String]) -> Result<String, CamError> {
    let options = CommandOptions::parse(args, &[], &[])?;
    if options.positionals.len() != 1 {
        return Err(CamError::InvalidCommand(
            "agent status requires exactly one agent name".to_string(),
        ));
    }

    let name = &options.positionals[0];
    let store = StateStore::new(home);
    let state = store.load_existing()?;
    let logger = StructuredLogger::new(&store);
    logger.command_started("agent status")?;
    match try_daemon_first_agent_status(&store, &logger, &state, name) {
        Ok(Some(result)) => {
            logger.command_finished("agent status")?;
            return Ok(result);
        }
        Ok(None) => {}
        Err(error) => {
            logger.command_failed("agent status", &error)?;
            return Err(error);
        }
    }
    let agent = services::get_agent(&state, name)?;
    logger.event(
        "agent.inspected",
        format!("agent `{name}` status inspected"),
        serde_json::to_value(services::agent_inspected_event_fields(&agent))?,
    )?;
    logger.command_finished("agent status")?;
    Ok(serde_json::to_string_pretty(&agent)?)
}

fn agent_resume(
    home: PathBuf,
    args: &[String],
    provider_lifecycle_router: &dyn ProviderLifecycleRouter,
    codex_stdio_lifecycle_factory: &dyn Fn(
        Option<String>,
    )
        -> Result<Box<dyn ProviderLifecycleRouter>, CamError>,
) -> Result<String, CamError> {
    let options = CommandOptions::parse(args, &["codex-program"], &["codex-stdio"])?;
    if options.positionals.len() != 1 {
        return Err(CamError::InvalidCommand(
            "agent resume requires exactly one agent name".to_string(),
        ));
    }
    let use_codex_stdio = options.has_flag("codex-stdio");
    let codex_program = options.value("codex-program").cloned();
    if codex_program.is_some() && !use_codex_stdio {
        return Err(CamError::InvalidCommand(
            "--codex-program requires --codex-stdio".to_string(),
        ));
    }

    let name = &options.positionals[0];
    let store = StateStore::new(home);
    let mut state = store.load_existing()?;
    let logger = StructuredLogger::new(&store);
    logger.command_started("agent resume")?;
    if !use_codex_stdio {
        match try_daemon_first_agent_resume(&store, &logger, &state, name) {
            Ok(Some(result)) => {
                logger.command_finished("agent resume")?;
                return Ok(result);
            }
            Ok(None) => {}
            Err(error) => {
                logger.command_failed("agent resume", &error)?;
                return Err(error);
            }
        }
    }
    let live_lifecycle_router = if use_codex_stdio {
        Some(codex_stdio_lifecycle_factory(codex_program.clone())?)
    } else {
        None
    };
    let selected_lifecycle_router: &dyn ProviderLifecycleRouter = if use_codex_stdio {
        live_lifecycle_router.as_deref().ok_or_else(|| {
            CamError::InvalidState(
                "Codex stdio lifecycle provider was requested but no provider lifecycle router was available"
                    .to_string(),
            )
        })?
    } else {
        provider_lifecycle_router
    };
    let application = if use_codex_stdio {
        let agent = state
            .find_agent(name)
            .ok_or_else(|| CamError::NotFound(format!("agent `{name}` does not exist")))?
            .clone();
        if agent.kind == AgentKind::Codex
            && agent.route == Route::Local
            && agent.thread_id.is_none()
        {
            let outcome = codex::start_thread_with_program(&agent, codex_program)?;
            services::apply_codex_thread_start_success(&mut state, name, outcome)?
        } else {
            services::resume_agent_with_peer_fetcher(
                &mut state,
                name,
                selected_lifecycle_router,
                &SshPeerAgentResumeFetcher,
            )?
        }
    } else {
        services::resume_agent_with_peer_fetcher(
            &mut state,
            name,
            selected_lifecycle_router,
            &SshPeerAgentResumeFetcher,
        )?
    };
    if application.persist_state {
        store.save_all(&state)?;
    }
    let result = application.result;
    logger.event(
        result.event_type(),
        format!("agent `{name}` resume checked"),
        serde_json::to_value(services::agent_resume_event_fields(&result))?,
    )?;
    logger.command_finished("agent resume")?;
    Ok(serde_json::to_string_pretty(&result)?)
}

fn agent_read(
    home: PathBuf,
    args: &[String],
    provider_transcript_reader: &dyn ProviderTranscriptReader,
) -> Result<String, CamError> {
    let options = CommandOptions::parse(
        args,
        &["turns", "wait-seconds"],
        &["latest", "include-turns"],
    )?;
    if options.positionals.len() != 1 {
        return Err(CamError::InvalidCommand(
            "agent read requires exactly one agent name".to_string(),
        ));
    }

    let name = &options.positionals[0];
    let store = StateStore::new(home);
    let state = store.load_existing()?;
    let read_options = services::AgentReadOptions {
        latest_only: options.has_flag("latest"),
        include_turns: options.has_flag("include-turns") || options.value("turns").is_some(),
        turn_limit: options
            .value("turns")
            .map(|value| services::parse_agent_read_turn_limit(value))
            .transpose()?,
        wait_seconds: options
            .value("wait-seconds")
            .map(|value| services::parse_agent_read_wait_seconds(value))
            .transpose()?,
    };
    let logger = StructuredLogger::new(&store);
    logger.command_started("agent read")?;
    match try_daemon_first_agent_read(&store, &logger, &state, name, read_options) {
        Ok(Some(result)) => {
            logger.command_finished("agent read")?;
            return Ok(result);
        }
        Ok(None) => {}
        Err(error) => {
            logger.command_failed("agent read", &error)?;
            return Err(error);
        }
    }
    let snapshot = services::read_agent_snapshot_with_peer_fetcher(
        &state,
        name,
        read_options,
        provider_transcript_reader,
        &SshPeerAgentReadFetcher,
    )?;
    logger.event(
        "agent.read",
        format!("agent `{name}` read from local evidence"),
        serde_json::to_value(services::agent_read_event_fields(&snapshot))?,
    )?;
    logger.command_finished("agent read")?;
    Ok(serde_json::to_string_pretty(&snapshot)?)
}

fn agent_create(home: PathBuf, args: &[String]) -> Result<String, CamError> {
    let options = CommandOptions::parse(
        args,
        &[
            "source",
            "cwd",
            "thread-id",
            "model",
            "model-provider",
            "effort",
            "speed",
            "service-tier",
        ],
        &[],
    )?;

    if options.positionals.len() != 1 {
        return Err(CamError::InvalidCommand(
            "agent create requires exactly one agent name".to_string(),
        ));
    }

    let name = options.positionals[0].clone();
    let agent_request = services::build_create_agent_request(services::BuildCreateAgentRequest {
        name: name.clone(),
        source: options.value("source").cloned(),
        cwd: options.value("cwd").cloned(),
        thread_id: options.value("thread-id").cloned(),
        model: options.value("model").cloned(),
        model_provider: options.value("model-provider").cloned(),
        effort: options.value("effort").cloned(),
        speed: options.value("speed").cloned(),
        service_tier: options.value("service-tier").cloned(),
    })?;

    let store = StateStore::new(home);
    let mut state = store.load_existing()?;
    let logger = StructuredLogger::new(&store);
    logger.command_started("agent create")?;
    match try_daemon_first_agent_create(&store, &logger, &state, &agent_request) {
        Ok(Some(agent)) => {
            logger.command_finished("agent create")?;
            let summary = services::agent_created_summary(&agent);
            return Ok(format!(
                "agent created\nname: {}\nkind: {}\nthread: {}",
                summary.name,
                summary.kind,
                summary.thread_id.as_deref().unwrap_or("-")
            ));
        }
        Ok(None) => {}
        Err(error) => {
            logger.command_failed("agent create", &error)?;
            return Err(error);
        }
    }
    let agent = match services::create_agent(&mut state, agent_request) {
        Ok(agent) => agent,
        Err(error) => {
            logger.command_failed("agent create", &error)?;
            return Err(error);
        }
    };
    store.save_all(&state)?;
    logger.event(
        "agent.created",
        format!("agent `{name}` created"),
        serde_json::to_value(services::agent_created_event_fields(&agent))?,
    )?;
    logger.command_finished("agent create")?;

    let summary = services::agent_created_summary(&agent);
    Ok(format!(
        "agent created\nname: {}\nkind: {}\nthread: {}",
        summary.name,
        summary.kind,
        summary.thread_id.as_deref().unwrap_or("-")
    ))
}

fn agent_set_model(home: PathBuf, args: &[String]) -> Result<String, CamError> {
    let options = CommandOptions::parse(
        args,
        &["model", "model-provider", "effort", "speed", "service-tier"],
        &[],
    )?;

    if options.positionals.len() != 1 {
        return Err(CamError::InvalidCommand(
            "agent set-model requires exactly one agent name".to_string(),
        ));
    }

    if options.values.is_empty() {
        return Err(CamError::InvalidCommand(
            "agent set-model requires at least one model option".to_string(),
        ));
    }

    let name = &options.positionals[0];
    services::validate_command_agent_name(name)?;
    let raw_update_request = services::BuildSetModelUpdateRequest {
        model: options.value("model").cloned(),
        model_provider: options.value("model-provider").cloned(),
        effort: options.value("effort").cloned(),
        speed: options.value("speed").cloned(),
        service_tier: options.value("service-tier").cloned(),
    };
    let store = StateStore::new(home);
    let mut state = store.load_existing()?;
    let logger = StructuredLogger::new(&store);
    logger.command_started("agent set-model")?;

    let update = match services::build_set_model_update(raw_update_request.clone()) {
        Ok(update) => update,
        Err(error) => {
            logger.command_failed("agent set-model", &error)?;
            return Err(error);
        }
    };
    match try_daemon_first_agent_set_model(&store, &logger, &state, name, &raw_update_request) {
        Ok(Some(updated)) => {
            logger.command_finished("agent set-model")?;
            let summary = services::agent_model_updated_summary(&updated);
            return Ok(format!(
                "agent model updated\nname: {}\nthread: {}",
                summary.name,
                summary.thread_id.as_deref().unwrap_or("-")
            ));
        }
        Ok(None) => {}
        Err(error) => {
            logger.command_failed("agent set-model", &error)?;
            return Err(error);
        }
    }

    let updated = match services::set_agent_model(&mut state, name, update) {
        Ok(agent) => agent,
        Err(error) => {
            logger.command_failed("agent set-model", &error)?;
            return Err(error);
        }
    };
    store.save_all(&state)?;
    logger.event(
        "agent.model_updated",
        format!("agent `{name}` model settings updated"),
        serde_json::to_value(services::agent_model_updated_event_fields(&updated))?,
    )?;
    logger.command_finished("agent set-model")?;

    let summary = services::agent_model_updated_summary(&updated);
    Ok(format!(
        "agent model updated\nname: {}\nthread: {}",
        summary.name,
        summary.thread_id.as_deref().unwrap_or("-")
    ))
}

fn send(
    home: PathBuf,
    args: &[String],
    provider_router: &dyn ProviderRouter,
    codex_stdio_factory: &dyn Fn(Option<String>) -> Result<Box<dyn ProviderRouter>, CamError>,
    peer_message_sender: &dyn PeerMessageSender,
) -> Result<String, CamError> {
    let options = CommandOptions::parse(
        args,
        &[
            "from",
            "source-node",
            "correlation-id",
            "message-type",
            "codex-program",
            "receipt-nonce",
            "expected-delivery",
            "delivery-action",
        ],
        &["strict", "codex-stdio"],
    )?;
    let source_agent = options.value("from").cloned();
    let source_node = options.value("source-node").cloned();
    let correlation_id = options.value("correlation-id").cloned();
    let message_type = options.value("message-type").cloned();
    let receipt_nonce = options.value("receipt-nonce").cloned();
    let expected_delivery = options.value("expected-delivery").cloned();
    let delivery_action = options.value("delivery-action").cloned();
    let codex_program = options.value("codex-program").cloned();
    let strict = options.has_flag("strict");
    let use_codex_stdio = options.has_flag("codex-stdio");
    let send_request = services::build_send_request(services::BuildSendRequest {
        positionals: options.positionals,
        source_agent,
        source_node,
        correlation_id,
        message_type,
        receipt_nonce,
        expected_delivery,
        delivery_action,
        strict,
        use_codex_stdio,
        codex_program,
    })?;

    let live_provider_router = if send_request.use_codex_stdio {
        Some(codex_stdio_factory(send_request.codex_program.clone())?)
    } else {
        None
    };
    let selected_provider_router: &dyn ProviderRouter = if send_request.use_codex_stdio {
        live_provider_router.as_deref().ok_or_else(|| {
            CamError::InvalidState(
                "Codex stdio provider was requested but no provider router was available"
                    .to_string(),
            )
        })?
    } else {
        provider_router
    };

    let store = StateStore::new(home);
    let mut state = store.load_existing()?;
    let logger = StructuredLogger::new(&store);
    logger.command_started("send")?;
    match try_daemon_first_send(&store, &logger, &state, &send_request) {
        Ok(Some(result)) => {
            logger.command_finished("send")?;
            return Ok(result);
        }
        Ok(None) => {}
        Err(error) => {
            logger.command_failed("send", &error)?;
            return Err(error);
        }
    }
    let command_result = send_started(
        &store,
        &logger,
        &mut state,
        send_request,
        selected_provider_router,
        peer_message_sender,
    );
    finish_started_command(&logger, "send", command_result)
}

fn send_started(
    store: &StateStore,
    logger: &StructuredLogger,
    state: &mut AppState,
    request: services::BuiltSendRequest,
    selected_provider_router: &dyn ProviderRouter,
    peer_message_sender: &dyn PeerMessageSender,
) -> Result<String, CamError> {
    if let Some(result) =
        try_codex_threadless_stdio_send(store, logger, state, &request, peer_message_sender)?
    {
        return Ok(result);
    }
    ensure_codex_thread_for_stdio_send(store, logger, state, &request)?;
    let prepared = services::prepare_send(state, request.prepare);
    services::validate_send_contract_metadata(
        &prepared.decision,
        request.expected_delivery.as_ref(),
        request.delivery_action.as_ref(),
    )?;
    let mut result = services::SendResult::from_message(&prepared.message);
    result.receipt_nonce = request.receipt_nonce;
    let application = services::apply_prepared_send_decision(state, prepared)?;

    match application {
        services::SendApplication::Local(application) => {
            let local_error = services::local_send_decision_error(&application);
            result.apply_message_with_error(
                &application.message,
                application.ok,
                application.result_error.clone(),
                local_error.as_ref(),
            );
            if application.persist_state {
                store.save_all(state)?;
            }
            log_message_event(
                &logger,
                application.event_type,
                &application.message,
                application.target.as_ref(),
                &application.decision,
                &application.decision.reason,
                local_error.as_ref(),
            )?;
        }
        services::SendApplication::Direct(application) => {
            let agent = &application.target;
            let decision = application.decision;
            let mut message = application.message;
            let delivery = execute_direct_delivery(
                state,
                agent,
                &mut message,
                &decision,
                selected_provider_router,
                peer_message_sender,
            );
            match delivery {
                Ok(event_type) => {
                    store.save_all(state)?;
                    result.apply_message(&message, true, None);
                    log_message_event(
                        &logger,
                        event_type,
                        &message,
                        Some(agent),
                        &decision,
                        &decision.reason,
                        None,
                    )?;
                }
                Err(DirectDeliveryError::Terminal(error)) => return Err(error),
                Err(DirectDeliveryError::Unqueueable(error)) => {
                    let application = services::apply_direct_delivery_failure(
                        state,
                        agent.clone(),
                        decision.clone(),
                        message,
                        &error,
                        services::DirectDeliveryFailureKind::Unqueueable,
                    );
                    result.apply_message_with_error(
                        &application.message,
                        application.ok,
                        application.result_error.clone(),
                        Some(&error),
                    );
                    log_message_event(
                        &logger,
                        application.event_type,
                        &application.message,
                        application.target.as_ref(),
                        &application.decision,
                        required_failure_reason(&application.result_error)?,
                        Some(&error),
                    )?;
                }
                Err(DirectDeliveryError::Recoverable(error)) => {
                    let application = services::apply_direct_delivery_failure(
                        state,
                        agent.clone(),
                        decision.clone(),
                        message,
                        &error,
                        services::DirectDeliveryFailureKind::Recoverable,
                    );
                    result.apply_message_with_error(
                        &application.message,
                        application.ok,
                        application.result_error.clone(),
                        Some(&error),
                    );
                    if application.persist_state {
                        store.save_all(state)?;
                    }
                    log_message_event(
                        &logger,
                        application.event_type,
                        &application.message,
                        application.target.as_ref(),
                        &application.decision,
                        required_failure_reason(&application.result_error)?,
                        Some(&error),
                    )?;
                }
            }
        }
    }

    Ok(serde_json::to_string_pretty(&result)?)
}

#[derive(Debug, serde::Serialize)]
struct CliDaemonSendEventFields {
    target_agent: String,
    daemon_running: bool,
    daemon_bind: String,
    daemon_port: u16,
    attempted: bool,
    used: bool,
    fallback_to_direct: bool,
    status_code: Option<u16>,
    error_kind: Option<&'static str>,
    error: Option<String>,
}

#[derive(Debug, serde::Serialize)]
struct CliDaemonSendRequestBody {
    target_agent: String,
    body: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    source_agent: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    source_node: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    correlation_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    message_type: Option<String>,
    strict: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    receipt_nonce: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    expected_delivery: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    delivery_action: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    codex_program: Option<String>,
}

struct CliDaemonSendHttpResponse {
    status_code: u16,
    body: String,
}

struct CliDaemonSendError {
    error: CamError,
    fallback_allowed: bool,
}

#[derive(Debug, serde::Serialize)]
struct CliDaemonAgentCreateEventFields {
    agent: String,
    daemon_running: bool,
    daemon_bind: String,
    daemon_port: u16,
    source: String,
    cwd_present: bool,
    thread_id_present: bool,
    attempted: bool,
    used: bool,
    fallback_to_direct: bool,
    status_code: Option<u16>,
    error_kind: Option<&'static str>,
    error: Option<String>,
}

#[derive(Debug, serde::Serialize)]
struct CliDaemonAgentCreateRequestBody {
    name: String,
    source: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    cwd: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    thread_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    model_provider: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    effort: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    service_tier: Option<String>,
}

struct CliDaemonAgentCreateError {
    error: CamError,
    fallback_allowed: bool,
}

#[derive(Debug, serde::Serialize)]
struct CliDaemonResumeEventFields {
    agent: String,
    daemon_running: bool,
    daemon_bind: String,
    daemon_port: u16,
    attempted: bool,
    used: bool,
    fallback_to_direct: bool,
    status_code: Option<u16>,
    error_kind: Option<&'static str>,
    error: Option<String>,
}

struct CliDaemonResumeError {
    error: CamError,
    fallback_allowed: bool,
}

#[derive(Debug, serde::Serialize)]
struct CliDaemonAgentStatusEventFields {
    agent: String,
    daemon_running: bool,
    daemon_bind: String,
    daemon_port: u16,
    attempted: bool,
    used: bool,
    fallback_to_direct: bool,
    status_code: Option<u16>,
    error_kind: Option<&'static str>,
    error: Option<String>,
}

struct CliDaemonAgentStatusError {
    error: CamError,
    fallback_allowed: bool,
}

#[derive(Debug, serde::Serialize)]
struct CliDaemonHealthEventFields {
    daemon_running: bool,
    daemon_bind: String,
    daemon_port: u16,
    attempted: bool,
    used: bool,
    fallback_to_direct: bool,
    status_code: Option<u16>,
    error_kind: Option<&'static str>,
    error: Option<String>,
}

struct CliDaemonHealthError {
    error: CamError,
    fallback_allowed: bool,
}

#[derive(Debug, serde::Serialize)]
struct CliDaemonPeerSyncEventFields {
    mode: &'static str,
    peer: Option<String>,
    daemon_running: bool,
    daemon_bind: String,
    daemon_port: u16,
    attempted: bool,
    used: bool,
    fallback_to_direct: bool,
    status_code: Option<u16>,
    error_kind: Option<&'static str>,
    error: Option<String>,
}

struct CliDaemonPeerSyncError {
    error: CamError,
    fallback_allowed: bool,
}

#[derive(Debug, serde::Serialize)]
struct CliDaemonPeerAddEventFields {
    peer: String,
    daemon_running: bool,
    daemon_bind: String,
    daemon_port: u16,
    ssh_present: bool,
    key_present: bool,
    remote_root_present: bool,
    attempted: bool,
    used: bool,
    fallback_to_direct: bool,
    status_code: Option<u16>,
    error_kind: Option<&'static str>,
    error: Option<String>,
}

#[derive(Debug, serde::Serialize)]
struct CliDaemonPeerAddRequestBody {
    name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    ssh: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    remote_root: Option<String>,
}

struct CliDaemonPeerAddError {
    error: CamError,
    fallback_allowed: bool,
}

#[derive(Debug, serde::Serialize)]
struct CliDaemonAgentSetModelEventFields {
    agent: String,
    daemon_running: bool,
    daemon_bind: String,
    daemon_port: u16,
    model_present: bool,
    model_provider_present: bool,
    effort_present: bool,
    speed_present: bool,
    service_tier_present: bool,
    attempted: bool,
    used: bool,
    fallback_to_direct: bool,
    status_code: Option<u16>,
    error_kind: Option<&'static str>,
    error: Option<String>,
}

#[derive(Debug, serde::Serialize)]
struct CliDaemonAgentSetModelRequestBody {
    #[serde(skip_serializing_if = "Option::is_none")]
    model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    model_provider: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    effort: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    speed: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    service_tier: Option<String>,
}

struct CliDaemonAgentSetModelError {
    error: CamError,
    fallback_allowed: bool,
}

#[derive(Debug, serde::Serialize)]
struct CliDaemonDiscoverLocalEventFields {
    daemon_running: bool,
    daemon_bind: String,
    daemon_port: u16,
    codex_home_present: bool,
    promote_approved: bool,
    attempted: bool,
    used: bool,
    fallback_to_direct: bool,
    status_code: Option<u16>,
    error_kind: Option<&'static str>,
    error: Option<String>,
}

#[derive(Debug, serde::Serialize)]
struct CliDaemonDiscoverLocalRequestBody {
    #[serde(skip_serializing_if = "Option::is_none")]
    codex_home: Option<String>,
    promote_approved: bool,
}

struct CliDaemonDiscoverLocalError {
    error: CamError,
    fallback_allowed: bool,
}

fn try_daemon_first_health(
    store: &StateStore,
    logger: &StructuredLogger,
    state: &AppState,
) -> Result<Option<String>, CamError> {
    if !crate::local_status::is_loopback_bind(&state.daemon.bind) {
        let error = CamError::InvalidState(format!(
            "daemon bind `{}` is not loopback; refusing daemon-first CLI daemon health",
            state.daemon.bind
        ));
        log_cli_daemon_health_route(
            logger,
            &CliDaemonHealthEventFields {
                daemon_running: state.daemon.observed_state
                    == crate::core::DaemonObservedState::Running,
                daemon_bind: state.daemon.bind.clone(),
                daemon_port: state.daemon.port,
                attempted: false,
                used: false,
                fallback_to_direct: false,
                status_code: None,
                error_kind: Some(error.kind()),
                error: Some(error.to_string()),
            },
        )?;
        return Err(error);
    }

    match health_via_daemon_loopback(store, state) {
        Ok(response) if (200..300).contains(&response.status_code) => {
            serde_json::from_str::<serde_json::Value>(&response.body).map_err(|error| {
                CamError::PeerProtocolViolation(format!(
                    "daemon-first CLI daemon health returned invalid JSON: {error}"
                ))
            })?;
            let rendered = crate::local_status::LocalStatusResponse {
                status_code: response.status_code,
                content_type: "application/json",
                body: response.body,
            };
            log_cli_daemon_health_route(
                logger,
                &CliDaemonHealthEventFields {
                    daemon_running: state.daemon.observed_state
                        == crate::core::DaemonObservedState::Running,
                    daemon_bind: state.daemon.bind.clone(),
                    daemon_port: state.daemon.port,
                    attempted: true,
                    used: true,
                    fallback_to_direct: false,
                    status_code: Some(rendered.status_code),
                    error_kind: None,
                    error: None,
                },
            )?;
            Ok(Some(serde_json::to_string_pretty(&rendered)?))
        }
        Ok(response) => {
            let error = CamError::DeliveryFailed(format!(
                "daemon-first CLI daemon health returned HTTP {}: {}",
                response.status_code,
                response.body.trim()
            ));
            log_cli_daemon_health_route(
                logger,
                &CliDaemonHealthEventFields {
                    daemon_running: state.daemon.observed_state
                        == crate::core::DaemonObservedState::Running,
                    daemon_bind: state.daemon.bind.clone(),
                    daemon_port: state.daemon.port,
                    attempted: true,
                    used: false,
                    fallback_to_direct: false,
                    status_code: Some(response.status_code),
                    error_kind: Some(error.kind()),
                    error: Some(error.to_string()),
                },
            )?;
            Err(error)
        }
        Err(error) => {
            let fallback_allowed = error.fallback_allowed;
            let error = error.error;
            log_cli_daemon_health_route(
                logger,
                &CliDaemonHealthEventFields {
                    daemon_running: state.daemon.observed_state
                        == crate::core::DaemonObservedState::Running,
                    daemon_bind: state.daemon.bind.clone(),
                    daemon_port: state.daemon.port,
                    attempted: true,
                    used: false,
                    fallback_to_direct: fallback_allowed,
                    status_code: None,
                    error_kind: Some(error.kind()),
                    error: Some(error.to_string()),
                },
            )?;
            if fallback_allowed {
                Ok(None)
            } else {
                Err(error)
            }
        }
    }
}

fn health_via_daemon_loopback(
    store: &StateStore,
    state: &AppState,
) -> Result<CliDaemonSendHttpResponse, CliDaemonHealthError> {
    let token = store
        .read_local_api_token()
        .map_err(|error| cli_daemon_health_error(error, true))?;
    let mut stream = TcpStream::connect_timeout(
        &daemon_socket_addr(&state.daemon.bind, state.daemon.port)
            .map_err(|error| cli_daemon_health_error(error, true))?,
        CLI_DAEMON_SEND_CONNECT_TIMEOUT,
    )
    .map_err(|error| {
        cli_daemon_health_error(
            CamError::DeliveryFailed(format!(
                "failed to connect to foreground daemon for CLI daemon health: {error}"
            )),
            true,
        )
    })?;
    stream
        .set_write_timeout(Some(CLI_DAEMON_SEND_WRITE_TIMEOUT))
        .map_err(|error| {
            cli_daemon_health_error(
                CamError::DeliveryFailed(format!(
                    "failed to set daemon health write timeout: {error}"
                )),
                true,
            )
        })?;
    stream
        .set_read_timeout(Some(CLI_DAEMON_SEND_READ_TIMEOUT))
        .map_err(|error| {
            cli_daemon_health_error(
                CamError::DeliveryFailed(format!(
                    "failed to set daemon health read timeout: {error}"
                )),
                true,
            )
        })?;
    let request_text = format!(
        "GET /health HTTP/1.1\r\nHost: {}\r\nAuthorization: Bearer {}\r\nConnection: close\r\n\r\n",
        state.daemon.bind, token,
    );
    stream.write_all(request_text.as_bytes()).map_err(|error| {
        cli_daemon_health_error(
            CamError::DeliveryFailed(format!(
                "failed to write daemon CLI health request: {error}"
            )),
            false,
        )
    })?;
    stream.flush().map_err(|error| {
        cli_daemon_health_error(
            CamError::DeliveryFailed(format!(
                "failed to flush daemon CLI health request: {error}"
            )),
            false,
        )
    })?;
    let mut response = String::new();
    stream.read_to_string(&mut response).map_err(|error| {
        cli_daemon_health_error(
            CamError::DeliveryFailed(format!(
                "failed to read daemon CLI health response: {error}"
            )),
            false,
        )
    })?;
    parse_daemon_http_response(&response).map_err(|error| cli_daemon_health_error(error, false))
}

fn cli_daemon_health_error(error: CamError, fallback_allowed: bool) -> CliDaemonHealthError {
    CliDaemonHealthError {
        error,
        fallback_allowed,
    }
}

fn try_daemon_first_status(
    store: &StateStore,
    state: &AppState,
) -> Result<Option<services::DaemonStatusOutput>, CamError> {
    match health_via_daemon_loopback(store, state) {
        Ok(response) if (200..300).contains(&response.status_code) => Ok(Some(
            daemon_status_from_live_health_response(&response.body)?,
        )),
        Ok(response) => Err(CamError::DeliveryFailed(format!(
            "daemon-first CLI daemon status returned HTTP {}: {}",
            response.status_code,
            response.body.trim()
        ))),
        Err(error) if error.fallback_allowed => Ok(None),
        Err(error) => Err(error.error),
    }
}

fn daemon_status_from_live_health_response(
    body: &str,
) -> Result<services::DaemonStatusOutput, CamError> {
    let envelope = serde_json::from_str::<DaemonFirstHealthEnvelope>(body).map_err(|error| {
        CamError::PeerProtocolViolation(format!(
            "daemon-first CLI daemon status returned invalid JSON: {error}"
        ))
    })?;
    Ok(services::DaemonStatusOutput {
        ok: true,
        implemented: envelope.daemon.implemented,
        running: envelope.daemon.running,
        state_source: services::DAEMON_STATE_SOURCE,
        live_probe_attempted: envelope.daemon.live_probe_attempted,
        process_supervisor_wired: envelope.daemon.process_supervisor_wired,
        process_exists: envelope.daemon.process_exists,
        instance_id_present: envelope.daemon.instance_id_present,
        identity_nonce_ref_present: envelope.daemon.identity_nonce_ref_present,
        instance_id_matched: envelope.daemon.instance_id_matched,
        node_name_matched: envelope.daemon.node_name_matched,
        version_matched: envelope.daemon.version_matched,
        process_identity_verified: envelope.daemon.process_identity_verified,
        identity_mismatch: envelope.daemon.identity_mismatch,
        live_probe_error: envelope.daemon.live_probe_error,
        message: envelope.daemon.message,
        state: envelope.daemon.state,
    })
}

fn try_daemon_first_peer_sync(
    store: &StateStore,
    logger: &StructuredLogger,
    state: &AppState,
    peer_name: Option<&str>,
) -> Result<Option<String>, CamError> {
    if daemon_first_disabled() {
        return Ok(None);
    }
    if !crate::local_status::is_loopback_bind(&state.daemon.bind) {
        let error = CamError::InvalidState(format!(
            "daemon bind `{}` is not loopback; refusing daemon-first CLI peer sync",
            state.daemon.bind
        ));
        log_cli_daemon_peer_sync_route(
            logger,
            &CliDaemonPeerSyncEventFields {
                mode: if peer_name.is_some() { "single" } else { "all" },
                peer: peer_name.map(str::to_string),
                daemon_running: state.daemon.observed_state
                    == crate::core::DaemonObservedState::Running,
                daemon_bind: state.daemon.bind.clone(),
                daemon_port: state.daemon.port,
                attempted: false,
                used: false,
                fallback_to_direct: false,
                status_code: None,
                error_kind: Some(error.kind()),
                error: Some(error.to_string()),
            },
        )?;
        return Err(error);
    }

    match peer_sync_via_daemon_loopback(store, state, peer_name) {
        Ok(response) if (200..300).contains(&response.status_code) => {
            if let Some(name) = peer_name {
                serde_json::from_str::<services::PeerSyncResult>(&response.body).map_err(
                    |error| {
                        CamError::PeerProtocolViolation(format!(
                            "daemon-first CLI peer sync returned invalid PeerSyncResult JSON: {error}"
                        ))
                    },
                )?;
                log_cli_daemon_peer_sync_route(
                    logger,
                    &CliDaemonPeerSyncEventFields {
                        mode: "single",
                        peer: Some(name.to_string()),
                        daemon_running: state.daemon.observed_state
                            == crate::core::DaemonObservedState::Running,
                        daemon_bind: state.daemon.bind.clone(),
                        daemon_port: state.daemon.port,
                        attempted: true,
                        used: true,
                        fallback_to_direct: false,
                        status_code: Some(response.status_code),
                        error_kind: None,
                        error: None,
                    },
                )?;
            } else {
                serde_json::from_str::<services::PeerSyncAllResult>(&response.body).map_err(
                    |error| {
                        CamError::PeerProtocolViolation(format!(
                            "daemon-first CLI peer sync-all returned invalid PeerSyncAllResult JSON: {error}"
                        ))
                    },
                )?;
                log_cli_daemon_peer_sync_route(
                    logger,
                    &CliDaemonPeerSyncEventFields {
                        mode: "all",
                        peer: None,
                        daemon_running: state.daemon.observed_state
                            == crate::core::DaemonObservedState::Running,
                        daemon_bind: state.daemon.bind.clone(),
                        daemon_port: state.daemon.port,
                        attempted: true,
                        used: true,
                        fallback_to_direct: false,
                        status_code: Some(response.status_code),
                        error_kind: None,
                        error: None,
                    },
                )?;
            }
            Ok(Some(response.body))
        }
        Ok(response) => {
            let error = CamError::DeliveryFailed(format!(
                "daemon-first CLI peer sync returned HTTP {}: {}",
                response.status_code,
                response.body.trim()
            ));
            log_cli_daemon_peer_sync_route(
                logger,
                &CliDaemonPeerSyncEventFields {
                    mode: if peer_name.is_some() { "single" } else { "all" },
                    peer: peer_name.map(str::to_string),
                    daemon_running: state.daemon.observed_state
                        == crate::core::DaemonObservedState::Running,
                    daemon_bind: state.daemon.bind.clone(),
                    daemon_port: state.daemon.port,
                    attempted: true,
                    used: false,
                    fallback_to_direct: false,
                    status_code: Some(response.status_code),
                    error_kind: Some(error.kind()),
                    error: Some(error.to_string()),
                },
            )?;
            Err(error)
        }
        Err(error) => {
            let fallback_allowed = error.fallback_allowed;
            let error = error.error;
            log_cli_daemon_peer_sync_route(
                logger,
                &CliDaemonPeerSyncEventFields {
                    mode: if peer_name.is_some() { "single" } else { "all" },
                    peer: peer_name.map(str::to_string),
                    daemon_running: state.daemon.observed_state
                        == crate::core::DaemonObservedState::Running,
                    daemon_bind: state.daemon.bind.clone(),
                    daemon_port: state.daemon.port,
                    attempted: true,
                    used: false,
                    fallback_to_direct: fallback_allowed,
                    status_code: None,
                    error_kind: Some(error.kind()),
                    error: Some(error.to_string()),
                },
            )?;
            if fallback_allowed {
                Ok(None)
            } else {
                Err(error)
            }
        }
    }
}

fn peer_sync_via_daemon_loopback(
    store: &StateStore,
    state: &AppState,
    peer_name: Option<&str>,
) -> Result<CliDaemonSendHttpResponse, CliDaemonPeerSyncError> {
    let token = store
        .read_local_api_token()
        .map_err(|error| cli_daemon_peer_sync_error(error, true))?;
    let path = match peer_name {
        Some(name) => format!("/v1/peers/{name}/sync"),
        None => "/v1/peers:sync".to_string(),
    };
    let mut stream = TcpStream::connect_timeout(
        &daemon_socket_addr(&state.daemon.bind, state.daemon.port)
            .map_err(|error| cli_daemon_peer_sync_error(error, true))?,
        CLI_DAEMON_SEND_CONNECT_TIMEOUT,
    )
    .map_err(|error| {
        cli_daemon_peer_sync_error(
            CamError::DeliveryFailed(format!(
                "failed to connect to foreground daemon for CLI peer sync: {error}"
            )),
            true,
        )
    })?;
    stream
        .set_write_timeout(Some(CLI_DAEMON_SEND_WRITE_TIMEOUT))
        .map_err(|error| {
            cli_daemon_peer_sync_error(
                CamError::DeliveryFailed(format!(
                    "failed to set daemon peer sync write timeout: {error}"
                )),
                true,
            )
        })?;
    stream
        .set_read_timeout(Some(CLI_DAEMON_SEND_READ_TIMEOUT))
        .map_err(|error| {
            cli_daemon_peer_sync_error(
                CamError::DeliveryFailed(format!(
                    "failed to set daemon peer sync read timeout: {error}"
                )),
                true,
            )
        })?;
    let request_text = format!(
        "POST {path} HTTP/1.1\r\nHost: {}\r\nAuthorization: Bearer {}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
        state.daemon.bind, token,
    );
    stream.write_all(request_text.as_bytes()).map_err(|error| {
        cli_daemon_peer_sync_error(
            CamError::DeliveryFailed(format!(
                "failed to write daemon CLI peer sync request: {error}"
            )),
            false,
        )
    })?;
    stream.flush().map_err(|error| {
        cli_daemon_peer_sync_error(
            CamError::DeliveryFailed(format!(
                "failed to flush daemon CLI peer sync request: {error}"
            )),
            false,
        )
    })?;
    let mut response = String::new();
    stream.read_to_string(&mut response).map_err(|error| {
        cli_daemon_peer_sync_error(
            CamError::DeliveryFailed(format!(
                "failed to read daemon CLI peer sync response: {error}"
            )),
            false,
        )
    })?;
    parse_daemon_http_response(&response).map_err(|error| cli_daemon_peer_sync_error(error, false))
}

fn cli_daemon_peer_sync_error(error: CamError, fallback_allowed: bool) -> CliDaemonPeerSyncError {
    CliDaemonPeerSyncError {
        error,
        fallback_allowed,
    }
}

fn try_daemon_first_peer_add(
    store: &StateStore,
    logger: &StructuredLogger,
    state: &AppState,
    request: &services::EnrollPeerRequest,
) -> Result<Option<Peer>, CamError> {
    if daemon_first_disabled() {
        return Ok(None);
    }
    if !crate::local_status::is_loopback_bind(&state.daemon.bind) {
        let error = CamError::InvalidState(format!(
            "daemon bind `{}` is not loopback; refusing daemon-first CLI peer add",
            state.daemon.bind
        ));
        log_cli_daemon_peer_add_route(
            logger,
            &CliDaemonPeerAddEventFields {
                peer: request.name.clone(),
                daemon_running: state.daemon.observed_state
                    == crate::core::DaemonObservedState::Running,
                daemon_bind: state.daemon.bind.clone(),
                daemon_port: state.daemon.port,
                ssh_present: true,
                key_present: request.key_path.is_some(),
                remote_root_present: request.remote_root.is_some(),
                attempted: false,
                used: false,
                fallback_to_direct: false,
                status_code: None,
                error_kind: Some(error.kind()),
                error: Some(error.to_string()),
            },
        )?;
        return Err(error);
    }

    match peer_add_via_daemon_loopback(store, state, request) {
        Ok(response) if (200..300).contains(&response.status_code) => {
            let peer = serde_json::from_str::<Peer>(&response.body).map_err(|error| {
                CamError::PeerProtocolViolation(format!(
                    "daemon-first CLI peer add returned invalid Peer JSON: {error}"
                ))
            })?;
            if peer.name != request.name {
                return Err(CamError::PeerProtocolViolation(format!(
                    "daemon-first CLI peer add returned unexpected peer `{}`",
                    peer.name
                )));
            }
            log_cli_daemon_peer_add_route(
                logger,
                &CliDaemonPeerAddEventFields {
                    peer: request.name.clone(),
                    daemon_running: state.daemon.observed_state
                        == crate::core::DaemonObservedState::Running,
                    daemon_bind: state.daemon.bind.clone(),
                    daemon_port: state.daemon.port,
                    ssh_present: true,
                    key_present: request.key_path.is_some(),
                    remote_root_present: request.remote_root.is_some(),
                    attempted: true,
                    used: true,
                    fallback_to_direct: false,
                    status_code: Some(response.status_code),
                    error_kind: None,
                    error: None,
                },
            )?;
            Ok(Some(peer))
        }
        Ok(response) => {
            let error = CamError::DeliveryFailed(format!(
                "daemon-first CLI peer add returned HTTP {}: {}",
                response.status_code,
                response.body.trim()
            ));
            log_cli_daemon_peer_add_route(
                logger,
                &CliDaemonPeerAddEventFields {
                    peer: request.name.clone(),
                    daemon_running: state.daemon.observed_state
                        == crate::core::DaemonObservedState::Running,
                    daemon_bind: state.daemon.bind.clone(),
                    daemon_port: state.daemon.port,
                    ssh_present: true,
                    key_present: request.key_path.is_some(),
                    remote_root_present: request.remote_root.is_some(),
                    attempted: true,
                    used: false,
                    fallback_to_direct: false,
                    status_code: Some(response.status_code),
                    error_kind: Some(error.kind()),
                    error: Some(error.to_string()),
                },
            )?;
            Err(error)
        }
        Err(error) => {
            let fallback_allowed = error.fallback_allowed;
            let error = error.error;
            log_cli_daemon_peer_add_route(
                logger,
                &CliDaemonPeerAddEventFields {
                    peer: request.name.clone(),
                    daemon_running: state.daemon.observed_state
                        == crate::core::DaemonObservedState::Running,
                    daemon_bind: state.daemon.bind.clone(),
                    daemon_port: state.daemon.port,
                    ssh_present: true,
                    key_present: request.key_path.is_some(),
                    remote_root_present: request.remote_root.is_some(),
                    attempted: true,
                    used: false,
                    fallback_to_direct: fallback_allowed,
                    status_code: None,
                    error_kind: Some(error.kind()),
                    error: Some(error.to_string()),
                },
            )?;
            if fallback_allowed {
                Ok(None)
            } else {
                Err(error)
            }
        }
    }
}

fn peer_add_via_daemon_loopback(
    store: &StateStore,
    state: &AppState,
    request: &services::EnrollPeerRequest,
) -> Result<CliDaemonSendHttpResponse, CliDaemonPeerAddError> {
    let token = store
        .read_local_api_token()
        .map_err(|error| cli_daemon_peer_add_error(error, true))?;
    let body = serde_json::to_string(&CliDaemonPeerAddRequestBody {
        name: request.name.clone(),
        ssh: Some(request.ssh_target.clone()),
        key: request.key_path.clone(),
        remote_root: request.remote_root.clone(),
    })
    .map_err(|error| cli_daemon_peer_add_error(error.into(), true))?;
    let mut stream = TcpStream::connect_timeout(
        &daemon_socket_addr(&state.daemon.bind, state.daemon.port)
            .map_err(|error| cli_daemon_peer_add_error(error, true))?,
        CLI_DAEMON_SEND_CONNECT_TIMEOUT,
    )
    .map_err(|error| {
        cli_daemon_peer_add_error(
            CamError::DeliveryFailed(format!(
                "failed to connect to foreground daemon for CLI peer add: {error}"
            )),
            true,
        )
    })?;
    stream
        .set_write_timeout(Some(CLI_DAEMON_SEND_WRITE_TIMEOUT))
        .map_err(|error| {
            cli_daemon_peer_add_error(
                CamError::DeliveryFailed(format!(
                    "failed to set daemon peer add write timeout: {error}"
                )),
                true,
            )
        })?;
    stream
        .set_read_timeout(Some(CLI_DAEMON_SEND_READ_TIMEOUT))
        .map_err(|error| {
            cli_daemon_peer_add_error(
                CamError::DeliveryFailed(format!(
                    "failed to set daemon peer add read timeout: {error}"
                )),
                true,
            )
        })?;
    let request_text = format!(
        "POST /v1/peers HTTP/1.1\r\nHost: {}\r\nAuthorization: Bearer {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        state.daemon.bind,
        token,
        body.len(),
        body
    );
    stream.write_all(request_text.as_bytes()).map_err(|error| {
        cli_daemon_peer_add_error(
            CamError::DeliveryFailed(format!(
                "failed to write daemon CLI peer add request: {error}"
            )),
            false,
        )
    })?;
    stream.flush().map_err(|error| {
        cli_daemon_peer_add_error(
            CamError::DeliveryFailed(format!(
                "failed to flush daemon CLI peer add request: {error}"
            )),
            false,
        )
    })?;
    let mut response = String::new();
    stream.read_to_string(&mut response).map_err(|error| {
        cli_daemon_peer_add_error(
            CamError::DeliveryFailed(format!(
                "failed to read daemon CLI peer add response: {error}"
            )),
            false,
        )
    })?;
    parse_daemon_http_response(&response).map_err(|error| cli_daemon_peer_add_error(error, false))
}

fn cli_daemon_peer_add_error(error: CamError, fallback_allowed: bool) -> CliDaemonPeerAddError {
    CliDaemonPeerAddError {
        error,
        fallback_allowed,
    }
}

fn try_daemon_first_agent_set_model(
    store: &StateStore,
    logger: &StructuredLogger,
    state: &AppState,
    agent_name: &str,
    request: &services::BuildSetModelUpdateRequest,
) -> Result<Option<Agent>, CamError> {
    if daemon_first_disabled() {
        return Ok(None);
    }
    if !crate::local_status::is_loopback_bind(&state.daemon.bind) {
        let error = CamError::InvalidState(format!(
            "daemon bind `{}` is not loopback; refusing daemon-first CLI agent set-model",
            state.daemon.bind
        ));
        log_cli_daemon_agent_set_model_route(
            logger,
            &CliDaemonAgentSetModelEventFields {
                agent: agent_name.to_string(),
                daemon_running: state.daemon.observed_state
                    == crate::core::DaemonObservedState::Running,
                daemon_bind: state.daemon.bind.clone(),
                daemon_port: state.daemon.port,
                model_present: request.model.is_some(),
                model_provider_present: request.model_provider.is_some(),
                effort_present: request.effort.is_some(),
                speed_present: request.speed.is_some(),
                service_tier_present: request.service_tier.is_some(),
                attempted: false,
                used: false,
                fallback_to_direct: false,
                status_code: None,
                error_kind: Some(error.kind()),
                error: Some(error.to_string()),
            },
        )?;
        return Err(error);
    }

    match agent_set_model_via_daemon_loopback(store, state, agent_name, request) {
        Ok(response) if (200..300).contains(&response.status_code) => {
            let agent = serde_json::from_str::<Agent>(&response.body).map_err(|error| {
                CamError::PeerProtocolViolation(format!(
                    "daemon-first CLI agent set-model returned invalid Agent JSON: {error}"
                ))
            })?;
            if agent.name != agent_name {
                return Err(CamError::PeerProtocolViolation(format!(
                    "daemon-first CLI agent set-model returned unexpected agent `{}`",
                    agent.name
                )));
            }
            log_cli_daemon_agent_set_model_route(
                logger,
                &CliDaemonAgentSetModelEventFields {
                    agent: agent_name.to_string(),
                    daemon_running: state.daemon.observed_state
                        == crate::core::DaemonObservedState::Running,
                    daemon_bind: state.daemon.bind.clone(),
                    daemon_port: state.daemon.port,
                    model_present: request.model.is_some(),
                    model_provider_present: request.model_provider.is_some(),
                    effort_present: request.effort.is_some(),
                    speed_present: request.speed.is_some(),
                    service_tier_present: request.service_tier.is_some(),
                    attempted: true,
                    used: true,
                    fallback_to_direct: false,
                    status_code: Some(response.status_code),
                    error_kind: None,
                    error: None,
                },
            )?;
            Ok(Some(agent))
        }
        Ok(response) => {
            let error = CamError::DeliveryFailed(format!(
                "daemon-first CLI agent set-model returned HTTP {}: {}",
                response.status_code,
                response.body.trim()
            ));
            log_cli_daemon_agent_set_model_route(
                logger,
                &CliDaemonAgentSetModelEventFields {
                    agent: agent_name.to_string(),
                    daemon_running: state.daemon.observed_state
                        == crate::core::DaemonObservedState::Running,
                    daemon_bind: state.daemon.bind.clone(),
                    daemon_port: state.daemon.port,
                    model_present: request.model.is_some(),
                    model_provider_present: request.model_provider.is_some(),
                    effort_present: request.effort.is_some(),
                    speed_present: request.speed.is_some(),
                    service_tier_present: request.service_tier.is_some(),
                    attempted: true,
                    used: false,
                    fallback_to_direct: false,
                    status_code: Some(response.status_code),
                    error_kind: Some(error.kind()),
                    error: Some(error.to_string()),
                },
            )?;
            Err(error)
        }
        Err(error) => {
            let fallback_allowed = error.fallback_allowed;
            let error = error.error;
            log_cli_daemon_agent_set_model_route(
                logger,
                &CliDaemonAgentSetModelEventFields {
                    agent: agent_name.to_string(),
                    daemon_running: state.daemon.observed_state
                        == crate::core::DaemonObservedState::Running,
                    daemon_bind: state.daemon.bind.clone(),
                    daemon_port: state.daemon.port,
                    model_present: request.model.is_some(),
                    model_provider_present: request.model_provider.is_some(),
                    effort_present: request.effort.is_some(),
                    speed_present: request.speed.is_some(),
                    service_tier_present: request.service_tier.is_some(),
                    attempted: true,
                    used: false,
                    fallback_to_direct: fallback_allowed,
                    status_code: None,
                    error_kind: Some(error.kind()),
                    error: Some(error.to_string()),
                },
            )?;
            if fallback_allowed {
                Ok(None)
            } else {
                Err(error)
            }
        }
    }
}

fn agent_set_model_via_daemon_loopback(
    store: &StateStore,
    state: &AppState,
    agent_name: &str,
    request: &services::BuildSetModelUpdateRequest,
) -> Result<CliDaemonSendHttpResponse, CliDaemonAgentSetModelError> {
    let token = store
        .read_local_api_token()
        .map_err(|error| cli_daemon_agent_set_model_error(error, true))?;
    let body = serde_json::to_string(&CliDaemonAgentSetModelRequestBody {
        model: request.model.clone(),
        model_provider: request.model_provider.clone(),
        effort: request.effort.clone(),
        speed: request.speed.clone(),
        service_tier: request.service_tier.clone(),
    })
    .map_err(|error| cli_daemon_agent_set_model_error(error.into(), true))?;
    let mut stream = TcpStream::connect_timeout(
        &daemon_socket_addr(&state.daemon.bind, state.daemon.port)
            .map_err(|error| cli_daemon_agent_set_model_error(error, true))?,
        CLI_DAEMON_SEND_CONNECT_TIMEOUT,
    )
    .map_err(|error| {
        cli_daemon_agent_set_model_error(
            CamError::DeliveryFailed(format!(
                "failed to connect to foreground daemon for CLI agent set-model: {error}"
            )),
            true,
        )
    })?;
    stream
        .set_write_timeout(Some(CLI_DAEMON_SEND_WRITE_TIMEOUT))
        .map_err(|error| {
            cli_daemon_agent_set_model_error(
                CamError::DeliveryFailed(format!(
                    "failed to set daemon agent set-model write timeout: {error}"
                )),
                true,
            )
        })?;
    stream
        .set_read_timeout(Some(CLI_DAEMON_SEND_READ_TIMEOUT))
        .map_err(|error| {
            cli_daemon_agent_set_model_error(
                CamError::DeliveryFailed(format!(
                    "failed to set daemon agent set-model read timeout: {error}"
                )),
                true,
            )
        })?;
    let path = format!("/v1/agents/{agent_name}/model");
    let request_text = format!(
        "POST {path} HTTP/1.1\r\nHost: {}\r\nAuthorization: Bearer {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        state.daemon.bind,
        token,
        body.len(),
        body
    );
    stream.write_all(request_text.as_bytes()).map_err(|error| {
        cli_daemon_agent_set_model_error(
            CamError::DeliveryFailed(format!(
                "failed to write daemon CLI agent set-model request: {error}"
            )),
            false,
        )
    })?;
    stream.flush().map_err(|error| {
        cli_daemon_agent_set_model_error(
            CamError::DeliveryFailed(format!(
                "failed to flush daemon CLI agent set-model request: {error}"
            )),
            false,
        )
    })?;
    let mut response = String::new();
    stream.read_to_string(&mut response).map_err(|error| {
        cli_daemon_agent_set_model_error(
            CamError::DeliveryFailed(format!(
                "failed to read daemon CLI agent set-model response: {error}"
            )),
            false,
        )
    })?;
    parse_daemon_http_response(&response)
        .map_err(|error| cli_daemon_agent_set_model_error(error, false))
}

fn cli_daemon_agent_set_model_error(
    error: CamError,
    fallback_allowed: bool,
) -> CliDaemonAgentSetModelError {
    CliDaemonAgentSetModelError {
        error,
        fallback_allowed,
    }
}

fn try_daemon_first_discover_local(
    store: &StateStore,
    logger: &StructuredLogger,
    state: &AppState,
    codex_home: Option<&str>,
    promote_approved: bool,
) -> Result<Option<crate::discovery::DiscoverySummary>, CamError> {
    if daemon_first_disabled() {
        return Ok(None);
    }
    if !crate::local_status::is_loopback_bind(&state.daemon.bind) {
        let error = CamError::InvalidState(format!(
            "daemon bind `{}` is not loopback; refusing daemon-first CLI discover local",
            state.daemon.bind
        ));
        log_cli_daemon_discover_local_route(
            logger,
            &CliDaemonDiscoverLocalEventFields {
                daemon_running: state.daemon.observed_state
                    == crate::core::DaemonObservedState::Running,
                daemon_bind: state.daemon.bind.clone(),
                daemon_port: state.daemon.port,
                codex_home_present: codex_home.is_some(),
                promote_approved,
                attempted: false,
                used: false,
                fallback_to_direct: false,
                status_code: None,
                error_kind: Some(error.kind()),
                error: Some(error.to_string()),
            },
        )?;
        return Err(error);
    }

    match discover_local_via_daemon_loopback(store, state, codex_home, promote_approved) {
        Ok(response) if (200..300).contains(&response.status_code) => {
            let summary =
                serde_json::from_str::<crate::discovery::DiscoverySummary>(&response.body)
                    .map_err(|error| {
                        CamError::PeerProtocolViolation(format!(
                            "daemon-first CLI discover local returned invalid DiscoverySummary JSON: {error}"
                        ))
                    })?;
            log_cli_daemon_discover_local_route(
                logger,
                &CliDaemonDiscoverLocalEventFields {
                    daemon_running: state.daemon.observed_state
                        == crate::core::DaemonObservedState::Running,
                    daemon_bind: state.daemon.bind.clone(),
                    daemon_port: state.daemon.port,
                    codex_home_present: codex_home.is_some(),
                    promote_approved,
                    attempted: true,
                    used: true,
                    fallback_to_direct: false,
                    status_code: Some(response.status_code),
                    error_kind: None,
                    error: None,
                },
            )?;
            Ok(Some(summary))
        }
        Ok(response) => {
            let error = CamError::DeliveryFailed(format!(
                "daemon-first CLI discover local returned HTTP {}: {}",
                response.status_code,
                response.body.trim()
            ));
            log_cli_daemon_discover_local_route(
                logger,
                &CliDaemonDiscoverLocalEventFields {
                    daemon_running: state.daemon.observed_state
                        == crate::core::DaemonObservedState::Running,
                    daemon_bind: state.daemon.bind.clone(),
                    daemon_port: state.daemon.port,
                    codex_home_present: codex_home.is_some(),
                    promote_approved,
                    attempted: true,
                    used: false,
                    fallback_to_direct: false,
                    status_code: Some(response.status_code),
                    error_kind: Some(error.kind()),
                    error: Some(error.to_string()),
                },
            )?;
            Err(error)
        }
        Err(error) => {
            let fallback_allowed = error.fallback_allowed;
            let error = error.error;
            log_cli_daemon_discover_local_route(
                logger,
                &CliDaemonDiscoverLocalEventFields {
                    daemon_running: state.daemon.observed_state
                        == crate::core::DaemonObservedState::Running,
                    daemon_bind: state.daemon.bind.clone(),
                    daemon_port: state.daemon.port,
                    codex_home_present: codex_home.is_some(),
                    promote_approved,
                    attempted: true,
                    used: false,
                    fallback_to_direct: fallback_allowed,
                    status_code: None,
                    error_kind: Some(error.kind()),
                    error: Some(error.to_string()),
                },
            )?;
            if fallback_allowed {
                Ok(None)
            } else {
                Err(error)
            }
        }
    }
}

fn discover_local_via_daemon_loopback(
    store: &StateStore,
    state: &AppState,
    codex_home: Option<&str>,
    promote_approved: bool,
) -> Result<CliDaemonSendHttpResponse, CliDaemonDiscoverLocalError> {
    let token = store
        .read_local_api_token()
        .map_err(|error| cli_daemon_discover_local_error(error, true))?;
    let body = serde_json::to_string(&CliDaemonDiscoverLocalRequestBody {
        codex_home: codex_home.map(str::to_string),
        promote_approved,
    })
    .map_err(|error| cli_daemon_discover_local_error(error.into(), true))?;
    let mut stream = TcpStream::connect_timeout(
        &daemon_socket_addr(&state.daemon.bind, state.daemon.port)
            .map_err(|error| cli_daemon_discover_local_error(error, true))?,
        CLI_DAEMON_SEND_CONNECT_TIMEOUT,
    )
    .map_err(|error| {
        cli_daemon_discover_local_error(
            CamError::DeliveryFailed(format!(
                "failed to connect to foreground daemon for CLI discover local: {error}"
            )),
            true,
        )
    })?;
    stream
        .set_write_timeout(Some(CLI_DAEMON_SEND_WRITE_TIMEOUT))
        .map_err(|error| {
            cli_daemon_discover_local_error(
                CamError::DeliveryFailed(format!(
                    "failed to set daemon discover local write timeout: {error}"
                )),
                true,
            )
        })?;
    stream
        .set_read_timeout(Some(CLI_DAEMON_SEND_READ_TIMEOUT))
        .map_err(|error| {
            cli_daemon_discover_local_error(
                CamError::DeliveryFailed(format!(
                    "failed to set daemon discover local read timeout: {error}"
                )),
                true,
            )
        })?;
    let request_text = format!(
        "POST /v1/discovery/local:run HTTP/1.1\r\nHost: {}\r\nAuthorization: Bearer {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        state.daemon.bind,
        token,
        body.len(),
        body
    );
    stream.write_all(request_text.as_bytes()).map_err(|error| {
        cli_daemon_discover_local_error(
            CamError::DeliveryFailed(format!(
                "failed to write daemon CLI discover local request: {error}"
            )),
            false,
        )
    })?;
    stream.flush().map_err(|error| {
        cli_daemon_discover_local_error(
            CamError::DeliveryFailed(format!(
                "failed to flush daemon CLI discover local request: {error}"
            )),
            false,
        )
    })?;
    let mut response = String::new();
    stream.read_to_string(&mut response).map_err(|error| {
        cli_daemon_discover_local_error(
            CamError::DeliveryFailed(format!(
                "failed to read daemon CLI discover local response: {error}"
            )),
            false,
        )
    })?;
    parse_daemon_http_response(&response)
        .map_err(|error| cli_daemon_discover_local_error(error, false))
}

fn cli_daemon_discover_local_error(
    error: CamError,
    fallback_allowed: bool,
) -> CliDaemonDiscoverLocalError {
    CliDaemonDiscoverLocalError {
        error,
        fallback_allowed,
    }
}

fn try_daemon_first_agent_create(
    store: &StateStore,
    logger: &StructuredLogger,
    state: &AppState,
    request: &services::CreateAgentRequest,
) -> Result<Option<Agent>, CamError> {
    if daemon_first_disabled() {
        return Ok(None);
    }
    if !crate::local_status::is_loopback_bind(&state.daemon.bind) {
        let error = CamError::InvalidState(format!(
            "daemon bind `{}` is not loopback; refusing daemon-first CLI agent create",
            state.daemon.bind
        ));
        log_cli_daemon_agent_create_route(
            logger,
            &CliDaemonAgentCreateEventFields {
                agent: request.name.clone(),
                daemon_running: state.daemon.observed_state
                    == crate::core::DaemonObservedState::Running,
                daemon_bind: state.daemon.bind.clone(),
                daemon_port: state.daemon.port,
                source: cli_daemon_agent_create_source(request)?.to_string(),
                cwd_present: request.cwd.is_some(),
                thread_id_present: request.thread_id.is_some(),
                attempted: false,
                used: false,
                fallback_to_direct: false,
                status_code: None,
                error_kind: Some(error.kind()),
                error: Some(error.to_string()),
            },
        )?;
        return Err(error);
    }

    match agent_create_via_daemon_loopback(store, state, request) {
        Ok(response) if (200..300).contains(&response.status_code) => {
            let agent = serde_json::from_str::<Agent>(&response.body).map_err(|error| {
                CamError::PeerProtocolViolation(format!(
                    "daemon-first CLI agent create returned invalid Agent JSON: {error}"
                ))
            })?;
            if agent.name != request.name || agent.kind != request.kind {
                return Err(CamError::PeerProtocolViolation(format!(
                    "daemon-first CLI agent create returned unexpected agent `{}` kind `{:?}`",
                    agent.name, agent.kind
                )));
            }
            log_cli_daemon_agent_create_route(
                logger,
                &CliDaemonAgentCreateEventFields {
                    agent: request.name.clone(),
                    daemon_running: state.daemon.observed_state
                        == crate::core::DaemonObservedState::Running,
                    daemon_bind: state.daemon.bind.clone(),
                    daemon_port: state.daemon.port,
                    source: cli_daemon_agent_create_source(request)?.to_string(),
                    cwd_present: request.cwd.is_some(),
                    thread_id_present: request.thread_id.is_some(),
                    attempted: true,
                    used: true,
                    fallback_to_direct: false,
                    status_code: Some(response.status_code),
                    error_kind: None,
                    error: None,
                },
            )?;
            Ok(Some(agent))
        }
        Ok(response) => {
            let error = CamError::DeliveryFailed(format!(
                "daemon-first CLI agent create returned HTTP {}: {}",
                response.status_code,
                response.body.trim()
            ));
            log_cli_daemon_agent_create_route(
                logger,
                &CliDaemonAgentCreateEventFields {
                    agent: request.name.clone(),
                    daemon_running: state.daemon.observed_state
                        == crate::core::DaemonObservedState::Running,
                    daemon_bind: state.daemon.bind.clone(),
                    daemon_port: state.daemon.port,
                    source: cli_daemon_agent_create_source(request)?.to_string(),
                    cwd_present: request.cwd.is_some(),
                    thread_id_present: request.thread_id.is_some(),
                    attempted: true,
                    used: false,
                    fallback_to_direct: false,
                    status_code: Some(response.status_code),
                    error_kind: Some(error.kind()),
                    error: Some(error.to_string()),
                },
            )?;
            Err(error)
        }
        Err(error) => {
            let fallback_allowed = error.fallback_allowed;
            let error = error.error;
            log_cli_daemon_agent_create_route(
                logger,
                &CliDaemonAgentCreateEventFields {
                    agent: request.name.clone(),
                    daemon_running: state.daemon.observed_state
                        == crate::core::DaemonObservedState::Running,
                    daemon_bind: state.daemon.bind.clone(),
                    daemon_port: state.daemon.port,
                    source: cli_daemon_agent_create_source(request)?.to_string(),
                    cwd_present: request.cwd.is_some(),
                    thread_id_present: request.thread_id.is_some(),
                    attempted: true,
                    used: false,
                    fallback_to_direct: fallback_allowed,
                    status_code: None,
                    error_kind: Some(error.kind()),
                    error: Some(error.to_string()),
                },
            )?;
            if fallback_allowed {
                Ok(None)
            } else {
                Err(error)
            }
        }
    }
}

fn agent_create_via_daemon_loopback(
    store: &StateStore,
    state: &AppState,
    request: &services::CreateAgentRequest,
) -> Result<CliDaemonSendHttpResponse, CliDaemonAgentCreateError> {
    let token = store
        .read_local_api_token()
        .map_err(|error| cli_daemon_agent_create_error(error, true))?;
    let body = serde_json::to_string(&CliDaemonAgentCreateRequestBody {
        name: request.name.clone(),
        source: cli_daemon_agent_create_source(request)
            .map_err(|error| cli_daemon_agent_create_error(error, true))?
            .to_string(),
        cwd: request.cwd.clone(),
        thread_id: request.thread_id.clone(),
        model: request.model.clone(),
        model_provider: request.model_provider.clone(),
        effort: request
            .effort
            .as_ref()
            .map(cli_daemon_agent_create_effort)
            .map(str::to_string),
        service_tier: request.service_tier.clone(),
    })
    .map_err(|error| cli_daemon_agent_create_error(error.into(), true))?;
    let mut stream = TcpStream::connect_timeout(
        &daemon_socket_addr(&state.daemon.bind, state.daemon.port)
            .map_err(|error| cli_daemon_agent_create_error(error, true))?,
        CLI_DAEMON_SEND_CONNECT_TIMEOUT,
    )
    .map_err(|error| {
        cli_daemon_agent_create_error(
            CamError::DeliveryFailed(format!(
                "failed to connect to foreground daemon for CLI agent create: {error}"
            )),
            true,
        )
    })?;
    stream
        .set_write_timeout(Some(CLI_DAEMON_SEND_WRITE_TIMEOUT))
        .map_err(|error| {
            cli_daemon_agent_create_error(
                CamError::DeliveryFailed(format!(
                    "failed to set daemon agent create write timeout: {error}"
                )),
                true,
            )
        })?;
    stream
        .set_read_timeout(Some(CLI_DAEMON_SEND_READ_TIMEOUT))
        .map_err(|error| {
            cli_daemon_agent_create_error(
                CamError::DeliveryFailed(format!(
                    "failed to set daemon agent create read timeout: {error}"
                )),
                true,
            )
        })?;
    let request_text = format!(
        "POST /v1/agents HTTP/1.1\r\nHost: {}\r\nAuthorization: Bearer {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        state.daemon.bind,
        token,
        body.len(),
        body
    );
    stream.write_all(request_text.as_bytes()).map_err(|error| {
        cli_daemon_agent_create_error(
            CamError::DeliveryFailed(format!(
                "failed to write daemon CLI agent create request: {error}"
            )),
            false,
        )
    })?;
    stream.flush().map_err(|error| {
        cli_daemon_agent_create_error(
            CamError::DeliveryFailed(format!(
                "failed to flush daemon CLI agent create request: {error}"
            )),
            false,
        )
    })?;
    let mut response = String::new();
    stream.read_to_string(&mut response).map_err(|error| {
        cli_daemon_agent_create_error(
            CamError::DeliveryFailed(format!(
                "failed to read daemon CLI agent create response: {error}"
            )),
            false,
        )
    })?;
    parse_daemon_http_response(&response)
        .map_err(|error| cli_daemon_agent_create_error(error, false))
}

fn cli_daemon_agent_create_source(
    request: &services::CreateAgentRequest,
) -> Result<&'static str, CamError> {
    match request.kind {
        AgentKind::Codex => Ok("codex"),
        AgentKind::AgySession => Ok("agy"),
        AgentKind::VirtualInbox => Ok("mailbox"),
        AgentKind::RemoteMirror => Err(CamError::InvalidCommand(
            "remote mirror agents must come from peer sync, not local create".to_string(),
        )),
    }
}

fn cli_daemon_agent_create_effort(effort: &crate::core::Effort) -> &'static str {
    match effort {
        crate::core::Effort::Minimal => "minimal",
        crate::core::Effort::Low => "low",
        crate::core::Effort::Medium => "medium",
        crate::core::Effort::High => "high",
        crate::core::Effort::Xhigh => "xhigh",
    }
}

fn cli_daemon_agent_create_error(
    error: CamError,
    fallback_allowed: bool,
) -> CliDaemonAgentCreateError {
    CliDaemonAgentCreateError {
        error,
        fallback_allowed,
    }
}

#[derive(Debug, serde::Serialize)]
struct CliDaemonAgentReadEventFields {
    agent: String,
    daemon_running: bool,
    daemon_bind: String,
    daemon_port: u16,
    latest_only: bool,
    include_turns: bool,
    turn_limit: Option<usize>,
    wait_seconds: Option<u64>,
    attempted: bool,
    used: bool,
    fallback_to_direct: bool,
    status_code: Option<u16>,
    error_kind: Option<&'static str>,
    error: Option<String>,
}

struct CliDaemonAgentReadError {
    error: CamError,
    fallback_allowed: bool,
}

fn try_daemon_first_agent_read(
    store: &StateStore,
    logger: &StructuredLogger,
    state: &AppState,
    name: &str,
    options: services::AgentReadOptions,
) -> Result<Option<String>, CamError> {
    if daemon_first_disabled() {
        return Ok(None);
    }
    if !crate::local_status::is_loopback_bind(&state.daemon.bind) {
        let error = CamError::InvalidState(format!(
            "daemon bind `{}` is not loopback; refusing daemon-first CLI agent read",
            state.daemon.bind
        ));
        log_cli_daemon_agent_read_route(
            logger,
            &CliDaemonAgentReadEventFields {
                agent: name.to_string(),
                daemon_running: state.daemon.observed_state
                    == crate::core::DaemonObservedState::Running,
                daemon_bind: state.daemon.bind.clone(),
                daemon_port: state.daemon.port,
                latest_only: options.latest_only,
                include_turns: options.include_turns,
                turn_limit: options.turn_limit,
                wait_seconds: options.wait_seconds,
                attempted: false,
                used: false,
                fallback_to_direct: false,
                status_code: None,
                error_kind: Some(error.kind()),
                error: Some(error.to_string()),
            },
        )?;
        return Err(error);
    }

    match agent_read_via_daemon_loopback(store, state, name, options) {
        Ok(response) if (200..300).contains(&response.status_code) => {
            serde_json::from_str::<services::AgentReadSnapshot>(&response.body).map_err(
                |error| {
                    CamError::PeerProtocolViolation(format!(
                        "daemon-first CLI agent read returned invalid AgentReadSnapshot JSON: {error}"
                    ))
                },
            )?;
            log_cli_daemon_agent_read_route(
                logger,
                &CliDaemonAgentReadEventFields {
                    agent: name.to_string(),
                    daemon_running: state.daemon.observed_state
                        == crate::core::DaemonObservedState::Running,
                    daemon_bind: state.daemon.bind.clone(),
                    daemon_port: state.daemon.port,
                    latest_only: options.latest_only,
                    include_turns: options.include_turns,
                    turn_limit: options.turn_limit,
                    wait_seconds: options.wait_seconds,
                    attempted: true,
                    used: true,
                    fallback_to_direct: false,
                    status_code: Some(response.status_code),
                    error_kind: None,
                    error: None,
                },
            )?;
            Ok(Some(response.body))
        }
        Ok(response) => {
            let error = CamError::DeliveryFailed(format!(
                "daemon-first CLI agent read returned HTTP {}: {}",
                response.status_code,
                response.body.trim()
            ));
            log_cli_daemon_agent_read_route(
                logger,
                &CliDaemonAgentReadEventFields {
                    agent: name.to_string(),
                    daemon_running: state.daemon.observed_state
                        == crate::core::DaemonObservedState::Running,
                    daemon_bind: state.daemon.bind.clone(),
                    daemon_port: state.daemon.port,
                    latest_only: options.latest_only,
                    include_turns: options.include_turns,
                    turn_limit: options.turn_limit,
                    wait_seconds: options.wait_seconds,
                    attempted: true,
                    used: false,
                    fallback_to_direct: false,
                    status_code: Some(response.status_code),
                    error_kind: Some(error.kind()),
                    error: Some(error.to_string()),
                },
            )?;
            Err(error)
        }
        Err(error) => {
            let fallback_allowed = error.fallback_allowed;
            let error = error.error;
            log_cli_daemon_agent_read_route(
                logger,
                &CliDaemonAgentReadEventFields {
                    agent: name.to_string(),
                    daemon_running: state.daemon.observed_state
                        == crate::core::DaemonObservedState::Running,
                    daemon_bind: state.daemon.bind.clone(),
                    daemon_port: state.daemon.port,
                    latest_only: options.latest_only,
                    include_turns: options.include_turns,
                    turn_limit: options.turn_limit,
                    wait_seconds: options.wait_seconds,
                    attempted: true,
                    used: false,
                    fallback_to_direct: fallback_allowed,
                    status_code: None,
                    error_kind: Some(error.kind()),
                    error: Some(error.to_string()),
                },
            )?;
            if fallback_allowed {
                Ok(None)
            } else {
                Err(error)
            }
        }
    }
}

fn agent_read_via_daemon_loopback(
    store: &StateStore,
    state: &AppState,
    name: &str,
    options: services::AgentReadOptions,
) -> Result<CliDaemonSendHttpResponse, CliDaemonAgentReadError> {
    let token = store
        .read_local_api_token()
        .map_err(|error| cli_daemon_agent_read_error(error, true))?;
    let path = agent_read_daemon_path(name, options);
    let mut stream = TcpStream::connect_timeout(
        &daemon_socket_addr(&state.daemon.bind, state.daemon.port)
            .map_err(|error| cli_daemon_agent_read_error(error, true))?,
        CLI_DAEMON_SEND_CONNECT_TIMEOUT,
    )
    .map_err(|error| {
        cli_daemon_agent_read_error(
            CamError::DeliveryFailed(format!(
                "failed to connect to foreground daemon for CLI agent read: {error}"
            )),
            true,
        )
    })?;
    stream
        .set_write_timeout(Some(CLI_DAEMON_SEND_WRITE_TIMEOUT))
        .map_err(|error| {
            cli_daemon_agent_read_error(
                CamError::DeliveryFailed(format!(
                    "failed to set daemon agent read write timeout: {error}"
                )),
                true,
            )
        })?;
    stream
        .set_read_timeout(Some(cli_daemon_agent_read_timeout(options)))
        .map_err(|error| {
            cli_daemon_agent_read_error(
                CamError::DeliveryFailed(format!(
                    "failed to set daemon agent read timeout: {error}"
                )),
                true,
            )
        })?;
    let request_text = format!(
        "GET {path} HTTP/1.1\r\nHost: {}\r\nAuthorization: Bearer {}\r\nConnection: close\r\n\r\n",
        state.daemon.bind, token,
    );
    stream.write_all(request_text.as_bytes()).map_err(|error| {
        cli_daemon_agent_read_error(
            CamError::DeliveryFailed(format!(
                "failed to write daemon CLI agent read request: {error}"
            )),
            false,
        )
    })?;
    stream.flush().map_err(|error| {
        cli_daemon_agent_read_error(
            CamError::DeliveryFailed(format!(
                "failed to flush daemon CLI agent read request: {error}"
            )),
            false,
        )
    })?;
    let mut response = String::new();
    stream.read_to_string(&mut response).map_err(|error| {
        cli_daemon_agent_read_error(
            CamError::DeliveryFailed(format!(
                "failed to read daemon CLI agent read response: {error}"
            )),
            false,
        )
    })?;
    parse_daemon_http_response(&response).map_err(|error| cli_daemon_agent_read_error(error, false))
}

fn agent_read_daemon_path(name: &str, options: services::AgentReadOptions) -> String {
    let mut query = Vec::new();
    if options.latest_only {
        query.push("latest=true".to_string());
    }
    if options.include_turns {
        query.push("include_turns=true".to_string());
    }
    if let Some(limit) = options.turn_limit {
        query.push(format!("turns={limit}"));
    }
    if let Some(wait_seconds) = options.wait_seconds {
        query.push(format!("wait_seconds={wait_seconds}"));
    }
    if query.is_empty() {
        format!("/v1/agents/{name}/thread")
    } else {
        format!("/v1/agents/{name}/thread?{}", query.join("&"))
    }
}

fn cli_daemon_agent_read_timeout(options: services::AgentReadOptions) -> Duration {
    let wait_timeout = options
        .wait_seconds
        .map(|seconds| Duration::from_secs(seconds.saturating_add(5)))
        .unwrap_or(Duration::ZERO);
    CLI_DAEMON_SEND_READ_TIMEOUT.max(wait_timeout)
}

fn cli_daemon_agent_read_error(error: CamError, fallback_allowed: bool) -> CliDaemonAgentReadError {
    CliDaemonAgentReadError {
        error,
        fallback_allowed,
    }
}

fn try_daemon_first_agent_status(
    store: &StateStore,
    logger: &StructuredLogger,
    state: &AppState,
    name: &str,
) -> Result<Option<String>, CamError> {
    if daemon_first_disabled() {
        return Ok(None);
    }
    if !crate::local_status::is_loopback_bind(&state.daemon.bind) {
        let error = CamError::InvalidState(format!(
            "daemon bind `{}` is not loopback; refusing daemon-first CLI agent status",
            state.daemon.bind
        ));
        log_cli_daemon_agent_status_route(
            logger,
            &CliDaemonAgentStatusEventFields {
                agent: name.to_string(),
                daemon_running: state.daemon.observed_state
                    == crate::core::DaemonObservedState::Running,
                daemon_bind: state.daemon.bind.clone(),
                daemon_port: state.daemon.port,
                attempted: false,
                used: false,
                fallback_to_direct: false,
                status_code: None,
                error_kind: Some(error.kind()),
                error: Some(error.to_string()),
            },
        )?;
        return Err(error);
    }

    match agent_status_via_daemon_loopback(store, state, name) {
        Ok(response) if (200..300).contains(&response.status_code) => {
            serde_json::from_str::<Agent>(&response.body).map_err(|error| {
                CamError::PeerProtocolViolation(format!(
                    "daemon-first CLI agent status returned invalid Agent JSON: {error}"
                ))
            })?;
            log_cli_daemon_agent_status_route(
                logger,
                &CliDaemonAgentStatusEventFields {
                    agent: name.to_string(),
                    daemon_running: state.daemon.observed_state
                        == crate::core::DaemonObservedState::Running,
                    daemon_bind: state.daemon.bind.clone(),
                    daemon_port: state.daemon.port,
                    attempted: true,
                    used: true,
                    fallback_to_direct: false,
                    status_code: Some(response.status_code),
                    error_kind: None,
                    error: None,
                },
            )?;
            Ok(Some(response.body))
        }
        Ok(response) => {
            let error = CamError::DeliveryFailed(format!(
                "daemon-first CLI agent status returned HTTP {}: {}",
                response.status_code,
                response.body.trim()
            ));
            log_cli_daemon_agent_status_route(
                logger,
                &CliDaemonAgentStatusEventFields {
                    agent: name.to_string(),
                    daemon_running: state.daemon.observed_state
                        == crate::core::DaemonObservedState::Running,
                    daemon_bind: state.daemon.bind.clone(),
                    daemon_port: state.daemon.port,
                    attempted: true,
                    used: false,
                    fallback_to_direct: false,
                    status_code: Some(response.status_code),
                    error_kind: Some(error.kind()),
                    error: Some(error.to_string()),
                },
            )?;
            Err(error)
        }
        Err(error) => {
            let fallback_allowed = error.fallback_allowed;
            let error = error.error;
            log_cli_daemon_agent_status_route(
                logger,
                &CliDaemonAgentStatusEventFields {
                    agent: name.to_string(),
                    daemon_running: state.daemon.observed_state
                        == crate::core::DaemonObservedState::Running,
                    daemon_bind: state.daemon.bind.clone(),
                    daemon_port: state.daemon.port,
                    attempted: true,
                    used: false,
                    fallback_to_direct: fallback_allowed,
                    status_code: None,
                    error_kind: Some(error.kind()),
                    error: Some(error.to_string()),
                },
            )?;
            if fallback_allowed {
                Ok(None)
            } else {
                Err(error)
            }
        }
    }
}

fn agent_status_via_daemon_loopback(
    store: &StateStore,
    state: &AppState,
    name: &str,
) -> Result<CliDaemonSendHttpResponse, CliDaemonAgentStatusError> {
    let token = store
        .read_local_api_token()
        .map_err(|error| cli_daemon_agent_status_error(error, true))?;
    let path = format!("/v1/agents/{name}");
    let mut stream = TcpStream::connect_timeout(
        &daemon_socket_addr(&state.daemon.bind, state.daemon.port)
            .map_err(|error| cli_daemon_agent_status_error(error, true))?,
        CLI_DAEMON_SEND_CONNECT_TIMEOUT,
    )
    .map_err(|error| {
        cli_daemon_agent_status_error(
            CamError::DeliveryFailed(format!(
                "failed to connect to foreground daemon for CLI agent status: {error}"
            )),
            true,
        )
    })?;
    stream
        .set_write_timeout(Some(CLI_DAEMON_SEND_WRITE_TIMEOUT))
        .map_err(|error| {
            cli_daemon_agent_status_error(
                CamError::DeliveryFailed(format!(
                    "failed to set daemon agent status write timeout: {error}"
                )),
                true,
            )
        })?;
    stream
        .set_read_timeout(Some(CLI_DAEMON_SEND_READ_TIMEOUT))
        .map_err(|error| {
            cli_daemon_agent_status_error(
                CamError::DeliveryFailed(format!(
                    "failed to set daemon agent status read timeout: {error}"
                )),
                true,
            )
        })?;
    let request_text = format!(
        "GET {path} HTTP/1.1\r\nHost: {}\r\nAuthorization: Bearer {}\r\nConnection: close\r\n\r\n",
        state.daemon.bind, token,
    );
    stream.write_all(request_text.as_bytes()).map_err(|error| {
        cli_daemon_agent_status_error(
            CamError::DeliveryFailed(format!(
                "failed to write daemon CLI agent status request: {error}"
            )),
            false,
        )
    })?;
    stream.flush().map_err(|error| {
        cli_daemon_agent_status_error(
            CamError::DeliveryFailed(format!(
                "failed to flush daemon CLI agent status request: {error}"
            )),
            false,
        )
    })?;
    let mut response = String::new();
    stream.read_to_string(&mut response).map_err(|error| {
        cli_daemon_agent_status_error(
            CamError::DeliveryFailed(format!(
                "failed to read daemon CLI agent status response: {error}"
            )),
            false,
        )
    })?;
    parse_daemon_http_response(&response)
        .map_err(|error| cli_daemon_agent_status_error(error, false))
}

fn cli_daemon_agent_status_error(
    error: CamError,
    fallback_allowed: bool,
) -> CliDaemonAgentStatusError {
    CliDaemonAgentStatusError {
        error,
        fallback_allowed,
    }
}

fn try_daemon_first_agent_resume(
    store: &StateStore,
    logger: &StructuredLogger,
    state: &AppState,
    name: &str,
) -> Result<Option<String>, CamError> {
    if daemon_first_disabled() {
        return Ok(None);
    }
    if !crate::local_status::is_loopback_bind(&state.daemon.bind) {
        let error = CamError::InvalidState(format!(
            "daemon bind `{}` is not loopback; refusing daemon-first CLI agent resume",
            state.daemon.bind
        ));
        log_cli_daemon_resume_route(
            logger,
            &CliDaemonResumeEventFields {
                agent: name.to_string(),
                daemon_running: state.daemon.observed_state
                    == crate::core::DaemonObservedState::Running,
                daemon_bind: state.daemon.bind.clone(),
                daemon_port: state.daemon.port,
                attempted: false,
                used: false,
                fallback_to_direct: false,
                status_code: None,
                error_kind: Some(error.kind()),
                error: Some(error.to_string()),
            },
        )?;
        return Err(error);
    }

    match resume_via_daemon_loopback(store, state, name) {
        Ok(response) if (200..300).contains(&response.status_code) => {
            serde_json::from_str::<serde_json::Value>(&response.body).map_err(|error| {
                CamError::PeerProtocolViolation(format!(
                    "daemon-first CLI agent resume returned invalid JSON: {error}"
                ))
            })?;
            log_cli_daemon_resume_route(
                logger,
                &CliDaemonResumeEventFields {
                    agent: name.to_string(),
                    daemon_running: state.daemon.observed_state
                        == crate::core::DaemonObservedState::Running,
                    daemon_bind: state.daemon.bind.clone(),
                    daemon_port: state.daemon.port,
                    attempted: true,
                    used: true,
                    fallback_to_direct: false,
                    status_code: Some(response.status_code),
                    error_kind: None,
                    error: None,
                },
            )?;
            Ok(Some(response.body))
        }
        Ok(response) => {
            let error = CamError::DeliveryFailed(format!(
                "daemon-first CLI agent resume returned HTTP {}: {}",
                response.status_code,
                response.body.trim()
            ));
            log_cli_daemon_resume_route(
                logger,
                &CliDaemonResumeEventFields {
                    agent: name.to_string(),
                    daemon_running: state.daemon.observed_state
                        == crate::core::DaemonObservedState::Running,
                    daemon_bind: state.daemon.bind.clone(),
                    daemon_port: state.daemon.port,
                    attempted: true,
                    used: false,
                    fallback_to_direct: false,
                    status_code: Some(response.status_code),
                    error_kind: Some(error.kind()),
                    error: Some(error.to_string()),
                },
            )?;
            Err(error)
        }
        Err(error) => {
            let fallback_allowed = error.fallback_allowed;
            let error = error.error;
            log_cli_daemon_resume_route(
                logger,
                &CliDaemonResumeEventFields {
                    agent: name.to_string(),
                    daemon_running: state.daemon.observed_state
                        == crate::core::DaemonObservedState::Running,
                    daemon_bind: state.daemon.bind.clone(),
                    daemon_port: state.daemon.port,
                    attempted: true,
                    used: false,
                    fallback_to_direct: fallback_allowed,
                    status_code: None,
                    error_kind: Some(error.kind()),
                    error: Some(error.to_string()),
                },
            )?;
            if fallback_allowed {
                Ok(None)
            } else {
                Err(error)
            }
        }
    }
}

fn resume_via_daemon_loopback(
    store: &StateStore,
    state: &AppState,
    name: &str,
) -> Result<CliDaemonSendHttpResponse, CliDaemonResumeError> {
    let token = store
        .read_local_api_token()
        .map_err(|error| cli_daemon_resume_error(error, true))?;
    let path = format!("/v1/agents/{name}/resume");
    let mut stream = TcpStream::connect_timeout(
        &daemon_socket_addr(&state.daemon.bind, state.daemon.port)
            .map_err(|error| cli_daemon_resume_error(error, true))?,
        CLI_DAEMON_SEND_CONNECT_TIMEOUT,
    )
    .map_err(|error| {
        cli_daemon_resume_error(
            CamError::DeliveryFailed(format!(
                "failed to connect to foreground daemon for CLI agent resume: {error}"
            )),
            true,
        )
    })?;
    stream
        .set_write_timeout(Some(CLI_DAEMON_SEND_WRITE_TIMEOUT))
        .map_err(|error| {
            cli_daemon_resume_error(
                CamError::DeliveryFailed(format!(
                    "failed to set daemon resume write timeout: {error}"
                )),
                true,
            )
        })?;
    stream
        .set_read_timeout(Some(CLI_DAEMON_SEND_READ_TIMEOUT))
        .map_err(|error| {
            cli_daemon_resume_error(
                CamError::DeliveryFailed(format!(
                    "failed to set daemon resume read timeout: {error}"
                )),
                true,
            )
        })?;
    let request_text = format!(
        "POST {path} HTTP/1.1\r\nHost: {}\r\nAuthorization: Bearer {}\r\nContent-Type: application/json\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{{}}",
        state.daemon.bind, token,
    );
    stream.write_all(request_text.as_bytes()).map_err(|error| {
        cli_daemon_resume_error(
            CamError::DeliveryFailed(format!(
                "failed to write daemon CLI agent resume request: {error}"
            )),
            false,
        )
    })?;
    stream.flush().map_err(|error| {
        cli_daemon_resume_error(
            CamError::DeliveryFailed(format!(
                "failed to flush daemon CLI agent resume request: {error}"
            )),
            false,
        )
    })?;
    let mut response = String::new();
    stream.read_to_string(&mut response).map_err(|error| {
        cli_daemon_resume_error(
            CamError::DeliveryFailed(format!(
                "failed to read daemon CLI agent resume response: {error}"
            )),
            false,
        )
    })?;
    parse_daemon_http_response(&response).map_err(|error| cli_daemon_resume_error(error, false))
}

fn cli_daemon_resume_error(error: CamError, fallback_allowed: bool) -> CliDaemonResumeError {
    CliDaemonResumeError {
        error,
        fallback_allowed,
    }
}

fn try_daemon_first_send(
    store: &StateStore,
    logger: &StructuredLogger,
    state: &AppState,
    request: &services::BuiltSendRequest,
) -> Result<Option<String>, CamError> {
    if daemon_first_disabled() {
        return Ok(None);
    }
    if !crate::local_status::is_loopback_bind(&state.daemon.bind) {
        let error = CamError::InvalidState(format!(
            "daemon bind `{}` is not loopback; refusing daemon-first CLI send",
            state.daemon.bind
        ));
        log_cli_daemon_send_route(
            logger,
            request,
            &CliDaemonSendEventFields {
                target_agent: request.prepare.target_agent.clone(),
                daemon_running: state.daemon.observed_state
                    == crate::core::DaemonObservedState::Running,
                daemon_bind: state.daemon.bind.clone(),
                daemon_port: state.daemon.port,
                attempted: false,
                used: false,
                fallback_to_direct: false,
                status_code: None,
                error_kind: Some(error.kind()),
                error: Some(error.to_string()),
            },
        )?;
        return Err(error);
    }

    let http_result = send_via_daemon_loopback(store, state, request);
    match http_result {
        Ok(response) if (200..300).contains(&response.status_code) => {
            serde_json::from_str::<services::SendResult>(&response.body).map_err(|error| {
                CamError::PeerProtocolViolation(format!(
                    "daemon-first CLI send returned invalid SendResult JSON: {error}"
                ))
            })?;
            log_cli_daemon_send_route(
                logger,
                request,
                &CliDaemonSendEventFields {
                    target_agent: request.prepare.target_agent.clone(),
                    daemon_running: state.daemon.observed_state
                        == crate::core::DaemonObservedState::Running,
                    daemon_bind: state.daemon.bind.clone(),
                    daemon_port: state.daemon.port,
                    attempted: true,
                    used: true,
                    fallback_to_direct: false,
                    status_code: Some(response.status_code),
                    error_kind: None,
                    error: None,
                },
            )?;
            Ok(Some(response.body))
        }
        Ok(response) => {
            let error = CamError::DeliveryFailed(format!(
                "daemon-first CLI send returned HTTP {}: {}",
                response.status_code,
                response.body.trim()
            ));
            log_cli_daemon_send_route(
                logger,
                request,
                &CliDaemonSendEventFields {
                    target_agent: request.prepare.target_agent.clone(),
                    daemon_running: state.daemon.observed_state
                        == crate::core::DaemonObservedState::Running,
                    daemon_bind: state.daemon.bind.clone(),
                    daemon_port: state.daemon.port,
                    attempted: true,
                    used: false,
                    fallback_to_direct: false,
                    status_code: Some(response.status_code),
                    error_kind: Some(error.kind()),
                    error: Some(error.to_string()),
                },
            )?;
            Err(error)
        }
        Err(error) => {
            let fallback_allowed = error.fallback_allowed;
            let error = error.error;
            log_cli_daemon_send_route(
                logger,
                request,
                &CliDaemonSendEventFields {
                    target_agent: request.prepare.target_agent.clone(),
                    daemon_running: state.daemon.observed_state
                        == crate::core::DaemonObservedState::Running,
                    daemon_bind: state.daemon.bind.clone(),
                    daemon_port: state.daemon.port,
                    attempted: true,
                    used: false,
                    fallback_to_direct: fallback_allowed,
                    status_code: None,
                    error_kind: Some(error.kind()),
                    error: Some(error.to_string()),
                },
            )?;
            if fallback_allowed {
                Ok(None)
            } else {
                Err(error)
            }
        }
    }
}

fn daemon_first_disabled() -> bool {
    std::env::var("QEXOW_CAM_DISABLE_DAEMON_FIRST")
        .map(|value| matches!(value.as_str(), "1" | "true" | "TRUE" | "yes" | "YES"))
        .unwrap_or(false)
}

fn send_via_daemon_loopback(
    store: &StateStore,
    state: &AppState,
    request: &services::BuiltSendRequest,
) -> Result<CliDaemonSendHttpResponse, CliDaemonSendError> {
    let token = store
        .read_local_api_token()
        .map_err(|error| cli_daemon_send_error(error, true))?;
    let body = serde_json::to_string(&CliDaemonSendRequestBody {
        target_agent: request.prepare.target_agent.clone(),
        body: request.prepare.body.clone(),
        source_agent: request.prepare.source_agent.clone(),
        source_node: request.prepare.source_node.clone(),
        correlation_id: request.prepare.correlation_id.clone(),
        message_type: request.prepare.message_type.clone(),
        strict: request.prepare.strict,
        receipt_nonce: request.receipt_nonce.clone(),
        expected_delivery: request
            .expected_delivery
            .as_ref()
            .map(|delivery| services::delivery_state_label(delivery).to_string()),
        delivery_action: request
            .delivery_action
            .as_ref()
            .map(|action| services::delivery_action_label(action).to_string()),
        codex_program: request.codex_program.clone(),
    })
    .map_err(|error| cli_daemon_send_error(error.into(), true))?;
    let mut stream = TcpStream::connect_timeout(
        &daemon_socket_addr(&state.daemon.bind, state.daemon.port)
            .map_err(|error| cli_daemon_send_error(error, true))?,
        CLI_DAEMON_SEND_CONNECT_TIMEOUT,
    )
    .map_err(|error| {
        cli_daemon_send_error(
            CamError::DeliveryFailed(format!(
                "failed to connect to foreground daemon for CLI send: {error}"
            )),
            true,
        )
    })?;
    stream
        .set_write_timeout(Some(CLI_DAEMON_SEND_WRITE_TIMEOUT))
        .map_err(|error| {
            cli_daemon_send_error(
                CamError::DeliveryFailed(format!(
                    "failed to set daemon send write timeout: {error}"
                )),
                true,
            )
        })?;
    stream
        .set_read_timeout(Some(CLI_DAEMON_SEND_READ_TIMEOUT))
        .map_err(|error| {
            cli_daemon_send_error(
                CamError::DeliveryFailed(format!(
                    "failed to set daemon send read timeout: {error}"
                )),
                true,
            )
        })?;
    let request_text = format!(
        "POST /v1/messages HTTP/1.1\r\nHost: {}\r\nAuthorization: Bearer {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        state.daemon.bind,
        token,
        body.len(),
        body
    );
    stream.write_all(request_text.as_bytes()).map_err(|error| {
        cli_daemon_send_error(
            CamError::DeliveryFailed(format!("failed to write daemon CLI send request: {error}")),
            false,
        )
    })?;
    stream.flush().map_err(|error| {
        cli_daemon_send_error(
            CamError::DeliveryFailed(format!("failed to flush daemon CLI send request: {error}")),
            false,
        )
    })?;
    let mut response = String::new();
    stream.read_to_string(&mut response).map_err(|error| {
        cli_daemon_send_error(
            CamError::DeliveryFailed(format!("failed to read daemon CLI send response: {error}")),
            false,
        )
    })?;
    parse_daemon_http_response(&response).map_err(|error| cli_daemon_send_error(error, false))
}

fn cli_daemon_send_error(error: CamError, fallback_allowed: bool) -> CliDaemonSendError {
    CliDaemonSendError {
        error,
        fallback_allowed,
    }
}

fn parse_daemon_port(value: &str) -> Result<u16, CamError> {
    let port = value.parse::<u16>().map_err(|error| {
        CamError::InvalidCommand(format!(
            "daemon port `{value}` is not a valid TCP port: {error}"
        ))
    })?;
    validate_daemon_port(port)?;
    Ok(port)
}

fn validate_daemon_port(port: u16) -> Result<(), CamError> {
    if port == 0 {
        return Err(CamError::InvalidCommand(
            "daemon port must be between 1 and 65535".to_string(),
        ));
    }
    Ok(())
}

fn daemon_socket_addr(bind: &str, port: u16) -> Result<std::net::SocketAddr, CamError> {
    validate_daemon_port(port)?;
    (bind, port)
        .to_socket_addrs()
        .map_err(|error| {
            CamError::InvalidState(format!(
                "daemon endpoint `{bind}:{port}` cannot be resolved as socket address: {error}"
            ))
        })?
        .find(|address| address.ip().is_loopback())
        .ok_or_else(|| {
            CamError::InvalidState(format!(
                "daemon endpoint `{bind}:{port}` did not resolve to a loopback socket address"
            ))
        })
}

fn parse_daemon_http_response(raw: &str) -> Result<CliDaemonSendHttpResponse, CamError> {
    let (head, body) = raw.split_once("\r\n\r\n").ok_or_else(|| {
        CamError::PeerProtocolViolation("daemon response missing HTTP header boundary".to_string())
    })?;
    let status_line = head.lines().next().ok_or_else(|| {
        CamError::PeerProtocolViolation("daemon response missing HTTP status line".to_string())
    })?;
    let mut parts = status_line.split_whitespace();
    let Some(version) = parts.next() else {
        return Err(CamError::PeerProtocolViolation(
            "daemon response status line missing version".to_string(),
        ));
    };
    if !version.starts_with("HTTP/") {
        return Err(CamError::PeerProtocolViolation(format!(
            "daemon response status line has invalid version `{version}`"
        )));
    }
    let status_code = parts
        .next()
        .ok_or_else(|| {
            CamError::PeerProtocolViolation(
                "daemon response status line missing status code".to_string(),
            )
        })?
        .parse::<u16>()
        .map_err(|error| {
            CamError::PeerProtocolViolation(format!(
                "daemon response status code is invalid: {error}"
            ))
        })?;
    Ok(CliDaemonSendHttpResponse {
        status_code,
        body: body.to_string(),
    })
}

fn log_cli_daemon_send_route(
    logger: &StructuredLogger,
    request: &services::BuiltSendRequest,
    fields: &CliDaemonSendEventFields,
) -> Result<(), CamError> {
    let event_type = if fields.used {
        "send.daemon.used"
    } else if fields.fallback_to_direct {
        "send.daemon.fallback"
    } else {
        "send.daemon.failed"
    };
    logger.event(
        event_type,
        format!(
            "CLI send daemon route decision for `{}`",
            request.prepare.target_agent
        ),
        serde_json::to_value(fields)?,
    )
}

fn log_cli_daemon_agent_create_route(
    logger: &StructuredLogger,
    fields: &CliDaemonAgentCreateEventFields,
) -> Result<(), CamError> {
    let event_type = if fields.used {
        "agent.create.daemon.used"
    } else if fields.fallback_to_direct {
        "agent.create.daemon.fallback"
    } else {
        "agent.create.daemon.failed"
    };
    logger.event(
        event_type,
        format!(
            "CLI agent create daemon route decision for `{}`",
            fields.agent
        ),
        serde_json::to_value(fields)?,
    )
}

fn log_cli_daemon_resume_route(
    logger: &StructuredLogger,
    fields: &CliDaemonResumeEventFields,
) -> Result<(), CamError> {
    let event_type = if fields.used {
        "resume.daemon.used"
    } else if fields.fallback_to_direct {
        "resume.daemon.fallback"
    } else {
        "resume.daemon.failed"
    };
    logger.event(
        event_type,
        format!(
            "CLI agent resume daemon route decision for `{}`",
            fields.agent
        ),
        serde_json::to_value(fields)?,
    )
}

fn log_cli_daemon_agent_status_route(
    logger: &StructuredLogger,
    fields: &CliDaemonAgentStatusEventFields,
) -> Result<(), CamError> {
    let event_type = if fields.used {
        "agent.status.daemon.used"
    } else if fields.fallback_to_direct {
        "agent.status.daemon.fallback"
    } else {
        "agent.status.daemon.failed"
    };
    logger.event(
        event_type,
        format!(
            "CLI agent status daemon route decision for `{}`",
            fields.agent
        ),
        serde_json::to_value(fields)?,
    )
}

fn log_cli_daemon_agent_read_route(
    logger: &StructuredLogger,
    fields: &CliDaemonAgentReadEventFields,
) -> Result<(), CamError> {
    let event_type = if fields.used {
        "agent.read.daemon.used"
    } else if fields.fallback_to_direct {
        "agent.read.daemon.fallback"
    } else {
        "agent.read.daemon.failed"
    };
    logger.event(
        event_type,
        format!(
            "CLI agent read daemon route decision for `{}`",
            fields.agent
        ),
        serde_json::to_value(fields)?,
    )
}

fn log_cli_daemon_health_route(
    logger: &StructuredLogger,
    fields: &CliDaemonHealthEventFields,
) -> Result<(), CamError> {
    let event_type = if fields.used {
        "daemon.health.daemon.used"
    } else if fields.fallback_to_direct {
        "daemon.health.daemon.fallback"
    } else {
        "daemon.health.daemon.failed"
    };
    logger.event(
        event_type,
        format!(
            "CLI daemon health route decision for `{}:{}`",
            fields.daemon_bind, fields.daemon_port
        ),
        serde_json::to_value(fields)?,
    )
}

fn log_cli_daemon_peer_sync_route(
    logger: &StructuredLogger,
    fields: &CliDaemonPeerSyncEventFields,
) -> Result<(), CamError> {
    let event_type = if fields.used {
        "peer.sync.daemon.used"
    } else if fields.fallback_to_direct {
        "peer.sync.daemon.fallback"
    } else {
        "peer.sync.daemon.failed"
    };
    logger.event(
        event_type,
        if let Some(peer) = fields.peer.as_deref() {
            format!("CLI peer sync daemon route decision for `{peer}`")
        } else {
            "CLI peer sync-all daemon route decision".to_string()
        },
        serde_json::to_value(fields)?,
    )
}

fn log_cli_daemon_peer_add_route(
    logger: &StructuredLogger,
    fields: &CliDaemonPeerAddEventFields,
) -> Result<(), CamError> {
    let event_type = if fields.used {
        "peer.add.daemon.used"
    } else if fields.fallback_to_direct {
        "peer.add.daemon.fallback"
    } else {
        "peer.add.daemon.failed"
    };
    logger.event(
        event_type,
        format!("CLI peer add daemon route decision for `{}`", fields.peer),
        serde_json::to_value(fields)?,
    )
}

fn log_cli_daemon_agent_set_model_route(
    logger: &StructuredLogger,
    fields: &CliDaemonAgentSetModelEventFields,
) -> Result<(), CamError> {
    let event_type = if fields.used {
        "agent.set_model.daemon.used"
    } else if fields.fallback_to_direct {
        "agent.set_model.daemon.fallback"
    } else {
        "agent.set_model.daemon.failed"
    };
    logger.event(
        event_type,
        format!(
            "CLI agent set-model daemon route decision for `{}`",
            fields.agent
        ),
        serde_json::to_value(fields)?,
    )
}

fn log_cli_daemon_discover_local_route(
    logger: &StructuredLogger,
    fields: &CliDaemonDiscoverLocalEventFields,
) -> Result<(), CamError> {
    let event_type = if fields.used {
        "discovery.local.daemon.used"
    } else if fields.fallback_to_direct {
        "discovery.local.daemon.fallback"
    } else {
        "discovery.local.daemon.failed"
    };
    logger.event(
        event_type,
        "CLI discover local daemon route decision",
        serde_json::to_value(fields)?,
    )
}

fn try_codex_threadless_stdio_send(
    store: &StateStore,
    logger: &StructuredLogger,
    state: &mut AppState,
    request: &services::BuiltSendRequest,
    _peer_message_sender: &dyn PeerMessageSender,
) -> Result<Option<String>, CamError> {
    if !request.use_codex_stdio {
        return Ok(None);
    }
    let Some(agent) = state.find_agent(&request.prepare.target_agent).cloned() else {
        return Ok(None);
    };
    if agent.kind != AgentKind::Codex || agent.route != Route::Local || agent.thread_id.is_some() {
        return Ok(None);
    }

    let decision = DeliveryDecision {
        state: DeliveryState::Delivered,
        action: DeliveryAction::WakeKnownSession,
        reason: "target is a local Codex agent with cwd; creating a thread before delivery"
            .to_string(),
    };
    services::validate_send_contract_metadata(
        &decision,
        request.expected_delivery.as_ref(),
        request.delivery_action.as_ref(),
    )?;

    let mut message = Message::new(
        request.prepare.target_agent.clone(),
        request.prepare.body.clone(),
    );
    message.source_agent = request.prepare.source_agent.clone();
    message.source_node = Some(
        request
            .prepare
            .source_node
            .clone()
            .unwrap_or_else(|| state.config.node_name.clone()),
    );
    message.correlation_id = request.prepare.correlation_id.clone();
    message.message_type = request.prepare.message_type.clone();
    message.strict = request.prepare.strict;

    let mut result = services::SendResult::from_message(&message);
    result.receipt_nonce = request.receipt_nonce.clone();

    let mut owner = codex::CodexAppServerOwner::start_for_program_and_model(
        request.codex_program.clone(),
        agent.model.clone(),
    )?;
    match codex::start_thread_then_wake_with_owner(&agent, &message, &mut owner) {
        Ok(outcome) => {
            let application = services::apply_codex_threadless_send_success(
                state,
                agent.clone(),
                decision,
                message,
                outcome.send,
            )?;
            store.save_all(state)?;
            result.apply_message(&application.message, true, None);
            log_message_event(
                logger,
                "message.delivered",
                &application.message,
                Some(&application.target),
                &application.decision,
                &application.decision.reason,
                None,
            )?;
        }
        Err(error) => {
            let failure_kind = if matches!(error, CamError::ProviderContractViolation(_)) {
                services::DirectDeliveryFailureKind::Unqueueable
            } else {
                services::DirectDeliveryFailureKind::Recoverable
            };
            let application = services::apply_direct_delivery_failure(
                state,
                agent,
                decision,
                message,
                &error,
                failure_kind,
            );
            result.apply_message_with_error(
                &application.message,
                application.ok,
                application.result_error.clone(),
                Some(&error),
            );
            if application.persist_state {
                store.save_all(state)?;
            }
            log_message_event(
                logger,
                application.event_type,
                &application.message,
                application.target.as_ref(),
                &application.decision,
                required_failure_reason(&application.result_error)?,
                Some(&error),
            )?;
        }
    }

    Ok(Some(serde_json::to_string_pretty(&result)?))
}

fn required_failure_reason(result_error: &Option<String>) -> Result<&str, CamError> {
    result_error.as_deref().ok_or_else(|| {
        CamError::InvalidState(
            "delivery failure application did not include a result error".to_string(),
        )
    })
}

fn ensure_codex_thread_for_stdio_send(
    store: &StateStore,
    logger: &StructuredLogger,
    state: &mut AppState,
    request: &services::BuiltSendRequest,
) -> Result<(), CamError> {
    if !request.use_codex_stdio {
        return Ok(());
    }

    let Some(agent) = state.find_agent(&request.prepare.target_agent).cloned() else {
        return Ok(());
    };
    if agent.kind != AgentKind::Codex || agent.route != Route::Local || agent.thread_id.is_some() {
        return Ok(());
    }

    let outcome = codex::start_thread_with_program(&agent, request.codex_program.clone())?;
    let thread_id = outcome.thread_id.clone();
    let application = services::apply_codex_thread_start_success(state, &agent.name, outcome)?;
    if application.persist_state {
        store.save_all(state)?;
    }
    logger.event(
        "agent.thread.bound",
        format!(
            "agent `{}` Codex thread/start bound before send",
            agent.name
        ),
        serde_json::json!({
            "agent": agent.name,
            "kind": "codex",
            "route": "local",
            "thread_id_present": true,
            "thread_id": thread_id,
            "reason": application.result.message,
        }),
    )?;
    Ok(())
}

fn finish_started_command<T>(
    logger: &StructuredLogger,
    command: &str,
    result: Result<T, CamError>,
) -> Result<T, CamError> {
    match result {
        Ok(value) => {
            logger.command_finished(command)?;
            Ok(value)
        }
        Err(error) => {
            logger.command_failed(command, &error)?;
            Err(error)
        }
    }
}

fn inbox(home: PathBuf, args: &[String]) -> Result<String, CamError> {
    let options = CommandOptions::parse(args, &["wait"], &["json"])?;
    if options.positionals.len() > 1 {
        return Err(CamError::InvalidCommand(
            "inbox accepts at most one agent name".to_string(),
        ));
    }
    if let Some(name) = options.positionals.first() {
        services::validate_command_agent_name(name)?;
    }

    let wait_seconds = options
        .value("wait")
        .map(|value| services::parse_inbox_wait_seconds(value))
        .transpose()?
        .unwrap_or(0);
    let store = StateStore::new(home);
    let logger = StructuredLogger::new(&store);
    logger.command_started("inbox")?;

    let filter = options.positionals.first().cloned();
    let inbox_result = match services::read_mailbox_with_wait(
        &store,
        filter.as_deref(),
        wait_seconds,
        Duration::from_millis(100),
    ) {
        Ok(result) => result,
        Err(error) => {
            logger.command_failed("inbox", &error)?;
            return Err(error);
        }
    };

    logger.event(
        "inbox.read",
        "inbox read",
        serde_json::to_value(services::inbox_read_event_fields(
            &inbox_result,
            filter.as_deref(),
        ))?,
    )?;
    logger.command_finished("inbox")?;

    if options.has_flag("json") {
        return Ok(serde_json::to_string_pretty(&inbox_result.messages)?);
    }

    let mut lines = vec!["MESSAGE_ID\tTARGET\tDELIVERY\tSOURCE\tTYPE\tBODY".to_string()];
    for row in services::inbox_list_rows(&inbox_result.messages) {
        lines.push(format!(
            "{}\t{}\t{}\t{}\t{}\t{}",
            row.message_id, row.target, row.delivery, row.source, row.message_type, row.body
        ));
    }
    Ok(lines.join("\n"))
}

fn discover_local_with_args(home: PathBuf, args: &[String]) -> Result<String, CamError> {
    let options = CommandOptions::parse(args, &["codex-home"], &["promote-approved"])?;
    if !options.positionals.is_empty() {
        return Err(CamError::InvalidCommand(
            "discover local does not accept positional arguments".to_string(),
        ));
    }
    services::validate_command_optional_nonempty_trimmed(
        "codex-home",
        options.value("codex-home").map(String::as_str),
    )?;

    let store = StateStore::new(home);
    let mut state = store.load_existing()?;
    let logger = StructuredLogger::new(&store);
    logger.command_started("discover local")?;
    match try_daemon_first_discover_local(
        &store,
        &logger,
        &state,
        options.value("codex-home").map(String::as_str),
        options.has_flag("promote-approved"),
    ) {
        Ok(Some(summary)) => {
            logger.command_finished("discover local")?;
            return Ok(serde_json::to_string_pretty(&summary)?);
        }
        Ok(None) => {}
        Err(error) => {
            logger.command_failed("discover local", &error)?;
            return Err(error);
        }
    }
    let application = match services::run_local_discovery(
        &mut state,
        options.value("codex-home").map(PathBuf::from),
        options.has_flag("promote-approved"),
    ) {
        Ok(application) => application,
        Err(error) => {
            logger.command_failed("discover local", &error)?;
            return Err(error);
        }
    };
    if application.persist_state {
        store.save_all(&state)?;
    }
    let source_path_present = application.source_path.is_some();
    logger.event(
        "discovery.local.scanned",
        if source_path_present {
            "local discovery scan completed"
        } else {
            "local discovery scan completed with no Codex home"
        },
        serde_json::to_value(services::discovery_local_scanned_event_fields(&application))?,
    )?;
    logger.command_finished("discover local")?;

    Ok(serde_json::to_string_pretty(&application.summary)?)
}

fn peer_add(home: PathBuf, args: &[String]) -> Result<String, CamError> {
    let options = CommandOptions::parse(args, &["ssh", "key", "remote-root"], &[])?;
    let ssh_target = options.value("ssh").cloned();
    let key_path = options.value("key").cloned();
    let remote_root = options.value("remote-root").cloned();
    let request = services::build_enroll_peer_request(services::BuildEnrollPeerRequest {
        peer_names: options.positionals,
        ssh_target,
        key_path,
        remote_root,
    })?;
    let name = request.name.clone();

    let store = StateStore::new(home);
    let mut state = store.load_existing()?;
    let logger = StructuredLogger::new(&store);
    logger.command_started("peer add")?;
    match try_daemon_first_peer_add(&store, &logger, &state, &request) {
        Ok(Some(peer)) => {
            logger.command_finished("peer add")?;
            let note = "no SSH connection was attempted";
            return Ok(format!(
                "peer enrolled\nname: {}\ntransport: {}\nssh: {}\nstate: {}\nnote: {}",
                peer.name,
                cli_peer_transport_label(&peer.transport),
                peer.ssh_target.as_deref().unwrap_or("-"),
                cli_peer_state_label(&peer.state),
                note
            ));
        }
        Ok(None) => {}
        Err(error) => {
            logger.command_failed("peer add", &error)?;
            return Err(error);
        }
    }
    let outcome = services::enroll_peer(&mut state, request)?;
    store.save_all(&state)?;
    let event_type = if outcome.updated_existing {
        "peer.updated"
    } else {
        "peer.added"
    };
    logger.event(
        event_type,
        format!("peer `{name}` enrolled or updated"),
        serde_json::to_value(services::peer_enrollment_event_fields(&outcome))?,
    )?;
    logger.command_finished("peer add")?;

    let summary = services::peer_enrollment_summary(&outcome);
    let note = if summary.network_probe_attempted {
        "SSH connection was attempted"
    } else {
        "no SSH connection was attempted"
    };
    Ok(format!(
        "peer enrolled\nname: {}\ntransport: {}\nssh: {}\nstate: {}\nnote: {}",
        summary.name,
        summary.transport,
        summary.ssh_target.as_deref().unwrap_or("-"),
        summary.state,
        note
    ))
}

fn cli_peer_transport_label(transport: &PeerTransport) -> &'static str {
    match transport {
        PeerTransport::Ssh => "ssh",
        PeerTransport::CodexManaged => "codex_managed",
    }
}

fn cli_peer_state_label(state: &PeerState) -> &'static str {
    match state {
        PeerState::Unknown => "unknown",
        PeerState::Verified => "verified",
        PeerState::Mirrored => "mirrored",
        PeerState::MirroredDegraded => "mirrored_degraded",
        PeerState::SyncFailed => "sync_failed",
    }
}

fn peer_list(home: PathBuf, args: &[String]) -> Result<String, CamError> {
    let options = CommandOptions::parse(args, &[], &["json"])?;
    if !options.positionals.is_empty() {
        return Err(CamError::InvalidCommand(
            "peer list does not accept positional arguments".to_string(),
        ));
    }

    let store = StateStore::new(home);
    let state = store.load_existing()?;
    let logger = StructuredLogger::new(&store);
    logger.command_started("peer list")?;
    logger.command_finished("peer list")?;
    let rows = services::list_peer_rows(&state);

    if options.has_flag("json") {
        return Ok(serde_json::to_string_pretty(&rows)?);
    }

    let mut lines = vec!["NAME\tTRANSPORT\tSTATE\tSSH\tREMOTE_ROOT".to_string()];
    for peer in rows {
        lines.push(format!(
            "{}\t{}\t{}\t{}\t{}",
            peer.name,
            peer.transport,
            peer.state,
            peer.ssh_target.as_deref().unwrap_or("-"),
            peer.remote_root.as_deref().unwrap_or("-")
        ));
    }
    Ok(lines.join("\n"))
}

fn peer_sync(
    home: PathBuf,
    args: &[String],
    peer_inventory_fetcher: &dyn PeerInventoryFetcher,
) -> Result<String, CamError> {
    let options = CommandOptions::parse(args, &[], &[])?;
    if options.positionals.len() > 1 {
        return Err(CamError::InvalidCommand(
            "peer sync accepts at most one peer name".to_string(),
        ));
    }

    let store = StateStore::new(home);
    let mut state = store.load_existing()?;
    let logger = StructuredLogger::new(&store);

    if let Some(name) = options.positionals.first() {
        services::validate_command_peer_name(name)?;
        if state.find_peer(name).is_none() {
            return Err(CamError::NotFound(format!("peer `{name}` does not exist")));
        }
        logger.command_started("peer sync")?;
        match try_daemon_first_peer_sync(&store, &logger, &state, Some(name.as_str())) {
            Ok(Some(result)) => {
                logger.command_finished("peer sync")?;
                return Ok(result);
            }
            Ok(None) => {}
            Err(error) => {
                logger.command_failed("peer sync", &error)?;
                return Err(error);
            }
        }
        let result =
            services::sync_peer_inventory_from_fetcher(&mut state, name, peer_inventory_fetcher)?;
        store.save_all(&state)?;
        log_peer_sync_result(&logger, &result)?;
        logger.command_finished("peer sync")?;
        return Ok(serde_json::to_string_pretty(&result)?);
    }

    logger.command_started("peer sync")?;
    match try_daemon_first_peer_sync(&store, &logger, &state, None) {
        Ok(Some(result)) => {
            logger.command_finished("peer sync")?;
            return Ok(result);
        }
        Ok(None) => {}
        Err(error) => {
            logger.command_failed("peer sync", &error)?;
            return Err(error);
        }
    }
    let aggregate =
        services::sync_all_peer_inventories_from_fetcher(&mut state, peer_inventory_fetcher);
    store.save_all(&state)?;
    for result in &aggregate.results {
        log_peer_sync_result(&logger, result)?;
    }
    logger.event(
        if aggregate.peers_requested == 0 {
            "peer.sync.all.noop"
        } else if aggregate.peers_failed == 0 {
            "peer.sync.all.completed"
        } else {
            "peer.sync.all.failed"
        },
        if aggregate.peers_requested == 0 {
            "peer sync-all had no enrolled peers"
        } else {
            "peer sync-all completed trusted inventory attempts"
        },
        serde_json::to_value(services::peer_sync_all_event_fields(&aggregate))?,
    )?;
    logger.command_finished("peer sync")?;
    Ok(serde_json::to_string_pretty(&aggregate)?)
}

fn log_peer_sync_result(
    logger: &StructuredLogger,
    result: &services::PeerSyncResult,
) -> Result<(), CamError> {
    let event_type = if result.ok {
        "peer.sync.completed"
    } else {
        "peer.sync.failed"
    };
    let message = if result.ok {
        format!(
            "peer `{}` sync completed from trusted inventory export",
            result.peer
        )
    } else if result.remote_command_attempted {
        format!(
            "peer `{}` sync failed loudly after remote execution",
            result.peer
        )
    } else {
        format!(
            "peer `{}` sync failed loudly before remote execution",
            result.peer
        )
    };
    logger.event(
        event_type,
        message,
        serde_json::to_value(services::peer_sync_event_fields(result))?,
    )
}

fn inventory_export(home: PathBuf, args: &[String]) -> Result<String, CamError> {
    let options = CommandOptions::parse(args, &[], &[])?;
    if !options.positionals.is_empty() {
        return Err(CamError::InvalidCommand(
            "inventory export does not accept positional arguments".to_string(),
        ));
    }

    let store = StateStore::new(home);
    let mut state = store.load_existing()?;
    let logger = StructuredLogger::new(&store);
    logger.command_started("inventory export")?;
    let discovery = services::run_local_discovery(&mut state, None, false)?;
    if discovery.persist_state {
        store.save_all(&state)?;
    }
    logger.event(
        "inventory.discovery_refreshed",
        "inventory export refreshed local Codex discovery before building peer inventory",
        serde_json::to_value(services::discovery_local_scanned_event_fields(&discovery))?,
    )?;
    let inventory = services::export_inventory(&state);
    logger.event(
        "inventory.exported",
        "inventory export generated",
        serde_json::to_value(services::inventory_exported_event_fields(&inventory))?,
    )?;
    logger.command_finished("inventory export")?;
    export_inventory(&inventory)
}

fn provider_codex_probe(home: PathBuf, args: &[String]) -> Result<String, CamError> {
    let options = CommandOptions::parse(args, &["codex-program"], &[])?;
    if !options.positionals.is_empty() {
        return Err(CamError::InvalidCommand(
            "provider codex probe does not accept positional arguments".to_string(),
        ));
    }
    let codex_program = options.value("codex-program").cloned();
    let store = StateStore::new(home);
    let logger = StructuredLogger::new(&store);
    logger.command_started("provider codex probe")?;
    let output = codex::probe_app_server(codex_program);
    logger.event(
        "provider.codex.probed",
        "Codex app-server stdio probe completed",
        serde_json::to_value(&output)?,
    )?;
    logger.command_finished("provider codex probe")?;
    Ok(serde_json::to_string_pretty(&output)?)
}

fn format_agent_rows(rows: &[services::AgentListRow]) -> String {
    let mut lines = vec!["NAME\tKIND\tRUNTIME\tCHAT\tCHAT_SOURCE\tROUTE\tTHREAD".to_string()];

    for agent in rows {
        lines.push(format!(
            "{}\t{}\t{}\t{}\t{}\t{}\t{}",
            agent.name,
            agent.kind,
            agent.status,
            agent.chat_status,
            agent.chat_status_source,
            agent.route,
            agent.thread_id.as_deref().unwrap_or("-")
        ));
    }

    lines.join("\n")
}

fn help() -> String {
    [
        "qexow-cam",
        "",
        "commands:",
        "  qexow-cam [--home <path>] init",
        "  qexow-cam [--home <path>] doctor",
        "  qexow-cam [--home <path>] daemon status",
        "  qexow-cam [--home <path>] daemon health",
        "  qexow-cam [--home <path>] daemon status-ui",
        "  qexow-cam [--home <path>] daemon configure [--bind <loopback>] [--port <port>]",
        "  qexow-cam [--home <path>] daemon start [--headless] [--port <port>]",
        "  qexow-cam [--home <path>] daemon run --instance-id <uuid> --identity-nonce-ref <relative-path> --identity-challenge <token>",
        "  qexow-cam [--home <path>] daemon stop",
        "  qexow-cam [--home <path>] logs [--json] [--limit <n>]",
        "  qexow-cam [--home <path>] agent create <name> --cwd <path> [--thread-id <id>] [--source <codex|agy|mailbox>] [--model <id>] [--model-provider <provider>] [--effort <minimal|low|medium|high|xhigh>] [--speed <standard|fast>] [--service-tier <tier>]",
        "  qexow-cam [--home <path>] agent list [--json]",
        "  qexow-cam [--home <path>] agent status <name>",
        "  qexow-cam [--home <path>] agent resume <name> [--codex-stdio] [--codex-program <path>]",
        "  qexow-cam [--home <path>] agent read <name> [--latest] [--include-turns] [--turns <n>] [--wait-seconds <seconds>]",
        "  qexow-cam [--home <path>] agent set-model <name> [--model <id>] [--model-provider <provider>] [--effort <minimal|low|medium|high|xhigh>] [--speed <standard|fast>] [--service-tier <tier>]",
        "  qexow-cam [--home <path>] send <target-agent> <message> [--from <agent>] [--source-node <node>] [--correlation-id <id>] [--message-type <type>] [--strict] [--codex-stdio] [--codex-program <path>]",
        "  qexow-cam [--home <path>] inbox [agent-name] [--json] [--wait <seconds>]",
        "  qexow-cam [--home <path>] discover local [--codex-home <path>] [--promote-approved]",
        "  qexow-cam [--home <path>] peer add <name> --ssh <user@host> [--key <path>] [--remote-root <path>]",
        "  qexow-cam [--home <path>] peer list [--json]",
        "  qexow-cam [--home <path>] peer sync [name]",
        "  qexow-cam [--home <path>] provider codex probe [--codex-program <path>]",
        "  qexow-cam [--home <path>] inventory export",
        "",
        "note: send --codex-stdio and agent resume --codex-stdio are direct one-shot CLI provider paths; daemon-owned paths use the authenticated loopback daemon when reachable.",
        "note: --codex-program requires --codex-stdio.",
        "note: Provider delivery uses real provider primitives when available; non-strict provider failure may queue durably, while --strict fails without queueing.",
    ]
    .join("\n")
}

fn log_message_event(
    logger: &StructuredLogger,
    event_type: &str,
    message: &Message,
    target: Option<&Agent>,
    decision: &DeliveryDecision,
    reason: &str,
    error: Option<&CamError>,
) -> Result<(), CamError> {
    logger.event(
        event_type,
        format!(
            "message `{}` is {}",
            message.message_id,
            services::delivery_state_label(&message.delivery)
        ),
        serde_json::to_value(services::message_event_fields(
            message, target, decision, reason, error,
        ))?,
    )
}

fn format_log_events(rows: &[services::LogListRow]) -> String {
    let mut lines = vec!["AT\tTYPE\tMESSAGE".to_string()];
    for row in rows {
        lines.push(format!("{}\t{}\t{}", row.at, row.event_type, row.message));
    }
    lines.join("\n")
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ParsedArgs {
    home: PathBuf,
    command: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CommandOptions {
    positionals: Vec<String>,
    values: BTreeMap<String, String>,
    flags: BTreeSet<String>,
}

impl CommandOptions {
    fn parse(
        args: &[String],
        value_options: &[&str],
        bool_options: &[&str],
    ) -> Result<Self, CamError> {
        let value_options = value_options.iter().copied().collect::<BTreeSet<_>>();
        let bool_options = bool_options.iter().copied().collect::<BTreeSet<_>>();
        let mut positionals = Vec::new();
        let mut values = BTreeMap::new();
        let mut flags = BTreeSet::new();
        let mut index = 0;

        while index < args.len() {
            let token = &args[index];
            if let Some(option) = token.strip_prefix("--") {
                let (name, inline_value) = split_option(option);

                if value_options.contains(name) {
                    let value = if let Some(value) = inline_value {
                        value.to_string()
                    } else {
                        let Some(value) = args.get(index + 1) else {
                            return Err(CamError::InvalidCommand(format!(
                                "--{name} requires a value"
                            )));
                        };
                        index += 1;
                        value.clone()
                    };
                    if value.is_empty() {
                        return Err(CamError::InvalidCommand(format!(
                            "--{name} requires a non-empty value"
                        )));
                    }
                    values.insert(name.to_string(), value);
                } else if bool_options.contains(name) {
                    if inline_value.is_some() {
                        return Err(CamError::InvalidCommand(format!(
                            "--{name} does not accept a value"
                        )));
                    }
                    flags.insert(name.to_string());
                } else {
                    return Err(CamError::InvalidCommand(format!("unknown option --{name}")));
                }
            } else {
                positionals.push(token.clone());
            }
            index += 1;
        }

        Ok(Self {
            positionals,
            values,
            flags,
        })
    }

    fn value(&self, name: &str) -> Option<&String> {
        self.values.get(name)
    }

    fn has_flag(&self, name: &str) -> bool {
        self.flags.contains(name)
    }
}

fn required_option<'a>(options: &'a CommandOptions, name: &str) -> Result<&'a String, CamError> {
    options
        .value(name)
        .ok_or_else(|| CamError::InvalidCommand(format!("--{name} is required")))
}

fn split_option(option: &str) -> (&str, Option<&str>) {
    if let Some((name, value)) = option.split_once('=') {
        (name, Some(value))
    } else {
        (option, None)
    }
}

impl ParsedArgs {
    fn parse(args: &[String]) -> Result<Self, CamError> {
        let mut home = default_home();
        let mut command = Vec::new();
        let mut index = 0;

        while index < args.len() {
            match args[index].as_str() {
                "--home" => {
                    let Some(value) = args.get(index + 1) else {
                        return Err(CamError::InvalidCommand(
                            "--home requires a path argument".to_string(),
                        ));
                    };
                    home = PathBuf::from(value);
                    index += 2;
                }
                value if value.starts_with("--home=") => {
                    let path = value.trim_start_matches("--home=");
                    if path.is_empty() {
                        return Err(CamError::InvalidCommand(
                            "--home requires a non-empty path argument".to_string(),
                        ));
                    }
                    home = PathBuf::from(path);
                    index += 1;
                }
                _ => {
                    command.extend(args[index..].iter().cloned());
                    break;
                }
            }
        }

        Ok(Self { home, command })
    }
}

fn default_home() -> PathBuf {
    if let Ok(home) = std::env::var("CAM_HOME") {
        return PathBuf::from(home);
    }

    if let Ok(home) = std::env::var("USERPROFILE").or_else(|_| std::env::var("HOME")) {
        return PathBuf::from(home).join(".qexow-cam-rust");
    }

    PathBuf::from(".qexow-cam-rust")
}
