use crate::state::{RuntimeState, ServerState, StateError, loopback};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

fn invalid(message: &str) -> StateError {
    StateError {
        kind: "invalid",
        message: message.into(),
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProcessIdentity {
    pub pid: u32,
    pub process: String,
    pub executable: String,
    pub started: String,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Listener {
    pub identity: ProcessIdentity,
    pub address: String,
    pub port: u16,
}
fn addresses_match(actual: &str, expected: &str) -> bool {
    let actual = actual.trim_matches(['[', ']']).to_ascii_lowercase();
    let expected = expected.trim_matches(['[', ']']).to_ascii_lowercase();
    if expected == "localhost" {
        ["127.0.0.1", "::1", "localhost"].contains(&actual.as_str())
    } else {
        actual == expected
    }
}
pub fn choose_listener(
    listeners: &[Listener],
    port: u16,
    host: &str,
) -> Result<Listener, StateError> {
    let mut matches = listeners
        .iter()
        .filter(|entry| entry.port == port && addresses_match(&entry.address, host));
    let found = matches
        .next()
        .ok_or_else(|| invalid("listener identity unavailable"))?;
    if matches.next().is_some() {
        return Err(invalid("listener identity ambiguous"));
    }
    Ok(found.clone())
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LiveIdentity {
    pub hash: String,
    pub expected: ProcessIdentity,
}
fn hash(value: &impl Serialize) -> Result<String, StateError> {
    let value = serde_json::to_value(value).map_err(|_| invalid("cannot serialize identity"))?;
    let bytes = serde_json::to_vec(&value).map_err(|_| invalid("cannot encode identity"))?;
    if bytes.len() > 1048576 {
        return Err(invalid("identity exceeds limit"));
    }
    Ok(format!("{:x}", Sha256::digest(bytes)))
}
pub fn capture(
    active: &ServerState,
    observed: &Listener,
    approved_executable: bool,
) -> Result<LiveIdentity, StateError> {
    active.validate()?;
    let url = loopback(&active.endpoint)?;
    let host = url.host_str().ok_or_else(|| invalid("missing host"))?;
    let identity = &observed.identity;
    if observed.port != active.port
        || !addresses_match(&observed.address, host)
        || !approved_executable
        || identity.pid == 0
        || identity.executable.is_empty()
        || identity.started.is_empty()
        || identity.process.is_empty()
        || active.pid.is_some_and(|pid| pid != identity.pid)
        || active
            .process_executable
            .as_ref()
            .is_some_and(|value| value != &identity.executable)
        || active
            .process_started_at
            .as_ref()
            .is_some_and(|value| value != &identity.started)
    {
        return Err(invalid(
            "observed listener does not match approved runtime identity",
        ));
    }
    let value = serde_json::json!({"backend":active.backend,"host":host,"port":active.port,"pid":identity.pid,"ownedByUs":active.owned_by_us,"executable":identity.executable,"startedAt":identity.started});
    Ok(LiveIdentity {
        hash: hash(&value)?,
        expected: identity.clone(),
    })
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Operation {
    Down,
    Detach,
    ReplaceServer,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Confirmation {
    operation: Operation,
    canonical_target_ids: Vec<String>,
    backend: Option<String>,
    endpoint: Option<String>,
    owned_by_us: Option<bool>,
    process_identity_hash: Option<String>,
    state_revision_hash: String,
}
impl Confirmation {
    pub fn prepare(
        operation: Operation,
        state: &RuntimeState,
        target: Option<&str>,
        live: Option<&LiveIdentity>,
    ) -> Result<Self, StateError> {
        state.validate()?;
        let active = state.active.as_ref();
        if active.is_some() != live.is_some() {
            return Err(invalid(
                "active confirmation requires live process identity",
            ));
        }
        match operation {
            Operation::Down if active.is_some_and(|entry| !entry.owned_by_us) => {
                return Err(invalid("down requires owned runtime"));
            }
            Operation::Detach if !active.is_some_and(|entry| !entry.owned_by_us) => {
                return Err(invalid("detach requires attached runtime"));
            }
            Operation::ReplaceServer if target.is_none() => {
                return Err(invalid("replacement target required"));
            }
            Operation::Down | Operation::Detach if target.is_some() => {
                return Err(invalid("unexpected target"));
            }
            _ => (),
        }
        let mut targets = active
            .map(|entry| vec![entry.model_id.clone()])
            .unwrap_or_default();
        if let Some(target) = target {
            targets.push(target.into());
        }
        for target in &targets {
            if target.is_empty()
                || target.len() > 8192
                || !target.as_bytes()[0].is_ascii_alphanumeric()
                || target.split('/').any(|part| part == "..")
                || !target
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || b"._:/-".contains(&byte))
            {
                return Err(invalid("unsafe confirmation target"));
            }
        }
        Ok(Self {
            operation,
            canonical_target_ids: targets,
            backend: active.map(|entry| entry.backend.clone()),
            endpoint: active.map(|entry| entry.endpoint.clone()),
            owned_by_us: active.map(|entry| entry.owned_by_us),
            process_identity_hash: live.map(|identity| identity.hash.clone()),
            state_revision_hash: hash(state)?,
        })
    }
    pub fn verify(&self, current: &Self) -> Result<(), StateError> {
        if self == current {
            Ok(())
        } else {
            Err(StateError {
                kind: "drift",
                message: "confirmation target changed; rebuild review".into(),
            })
        }
    }
}

pub fn parse_lsof_listener(raw: &str, port: u16, host: &str) -> Result<u32, StateError> {
    if raw.len() > 65536 {
        return Err(invalid("listener output exceeds limit"));
    }
    let mut pid = None;
    let mut matches = Vec::new();
    for line in raw.lines() {
        if let Some(value) = line.strip_prefix('p') {
            pid = value
                .parse::<u32>()
                .ok()
                .filter(|pid| *pid > 0 && *pid <= i32::MAX as u32);
        }
        if let Some(value) = line.strip_prefix('n') {
            let Some((address, observed_port)) = value.rsplit_once(':') else {
                return Err(invalid("invalid listener address"));
            };
            if observed_port.parse::<u16>().ok() == Some(port) && addresses_match(address, host) {
                matches.push(pid.ok_or_else(|| invalid("missing listener PID"))?);
            }
        }
    }
    if matches.len() != 1 {
        return Err(invalid("listener absent or ambiguous"));
    }
    Ok(matches[0])
}
pub fn parse_start_time(raw: &str) -> Result<String, StateError> {
    if raw.len() > 4096 {
        return Err(invalid("process start output exceeds limit"));
    }
    let rows: Vec<_> = raw.lines().filter(|line| !line.trim().is_empty()).collect();
    if rows.len() != 1 {
        return Err(invalid("process start absent or ambiguous"));
    }
    let parts: Vec<_> = rows[0].split_whitespace().collect();
    if parts.len() != 5 || !["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"].contains(&parts[0]) {
        return Err(invalid("invalid process start"));
    }
    let month = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ]
    .iter()
    .position(|month| *month == parts[1])
    .ok_or_else(|| invalid("invalid start month"))?
        + 1;
    let day = parts[2]
        .parse::<u8>()
        .map_err(|_| invalid("invalid start day"))?;
    let time: Vec<_> = parts[3].split(':').collect();
    if !(1..=31).contains(&day)
        || parts[4].len() != 4
        || !parts[4].bytes().all(|byte| byte.is_ascii_digit())
        || time.len() != 3
        || time.iter().enumerate().any(|(index, part)| {
            part.len() != 2
                || part
                    .parse::<u8>()
                    .map_or(true, |value| value > if index == 0 { 23 } else { 59 })
        })
    {
        return Err(invalid("invalid process start"));
    }
    Ok(format!("{}-{month:02}-{day:02} {}", parts[4], parts[3]))
}
pub fn parse_windows_listener(raw: &str, port: u16, host: &str) -> Result<Listener, StateError> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Row {
        pid: u32,
        process: String,
        executable: String,
        started: String,
        address: String,
        port: u16,
    }
    if raw.len() > 65536 {
        return Err(invalid("listener output exceeds limit"));
    }
    let rows: Vec<Row> =
        serde_json::from_str(raw).map_err(|_| invalid("invalid Windows listener output"))?;
    let listeners: Vec<_> = rows
        .into_iter()
        .map(|row| Listener {
            identity: ProcessIdentity {
                pid: row.pid,
                process: row.process,
                executable: row.executable,
                started: row.started,
            },
            address: row.address,
            port: row.port,
        })
        .collect();
    let listener = choose_listener(&listeners, port, host)?;
    validate_process(&listener.identity)?;
    Ok(listener)
}
fn validate_process(identity: &ProcessIdentity) -> Result<(), StateError> {
    if identity.pid == 0
        || identity.pid > i32::MAX as u32
        || identity.process.is_empty()
        || identity.executable.is_empty()
        || identity.started.is_empty()
    {
        return Err(invalid("incomplete process identity"));
    }
    Ok(())
}
#[async_trait::async_trait]
pub trait ProcessProbe: Send + Sync {
    async fn listener(&self, port: u16, host: &str) -> Result<Listener, StateError>;
    async fn process(&self, pid: u32) -> Result<ProcessIdentity, StateError>;
    /// Return argv including the executable; unavailable introspection must fail closed.
    async fn command_line(&self, _pid: u32) -> Result<Vec<String>, StateError> {
        Err(invalid("process command line unavailable"))
    }
    /// Whether any process holds `pid`; probes that cannot tell must fail closed.
    async fn alive(&self, _pid: u32) -> Result<bool, StateError> {
        Err(invalid("process liveness unavailable"))
    }
}
pub struct NativeProcessProbe;
async fn output(binary: &str, args: &[&str]) -> Result<String, StateError> {
    crate::hardware::command_output(binary, args)
        .await
        .map_err(|_| invalid("process identity probe failed"))
}
#[async_trait::async_trait]
impl ProcessProbe for NativeProcessProbe {
    async fn alive(&self, pid: u32) -> Result<bool, StateError> {
        if pid == 0 || pid > i32::MAX as u32 {
            return Err(invalid("invalid process PID"));
        }
        tokio::task::spawn_blocking(move || {
            let mut system = sysinfo::System::new();
            let pid = sysinfo::Pid::from_u32(pid);
            system.refresh_processes_specifics(
                sysinfo::ProcessesToUpdate::Some(&[pid]),
                true,
                sysinfo::ProcessRefreshKind::nothing(),
            );
            system.process(pid).is_some()
        })
        .await
        .map_err(|_| invalid("process liveness probe failed"))
    }
    async fn command_line(&self, pid: u32) -> Result<Vec<String>, StateError> {
        tokio::task::spawn_blocking(move || {
            let mut system = sysinfo::System::new();
            let pid = sysinfo::Pid::from_u32(pid);
            system.refresh_processes_specifics(
                sysinfo::ProcessesToUpdate::Some(&[pid]),
                true,
                sysinfo::ProcessRefreshKind::nothing().with_cmd(sysinfo::UpdateKind::Always),
            );
            let process = system
                .process(pid)
                .ok_or_else(|| invalid("process command line unavailable"))?;
            let args = process.cmd();
            if args.is_empty()
                || args.len() > 1024
                || args.iter().map(|arg| arg.len()).sum::<usize>() > 65536
            {
                return Err(invalid("invalid process command line"));
            }
            args.iter()
                .map(|arg| {
                    arg.to_str()
                        .map(str::to_owned)
                        .ok_or_else(|| invalid("non-UTF-8 process command line"))
                })
                .collect()
        })
        .await
        .map_err(|_| invalid("process command line probe failed"))?
    }

    async fn listener(&self, port: u16, host: &str) -> Result<Listener, StateError> {
        loopback(&format!(
            "http://{}:{port}",
            if host.contains(':') && !host.starts_with('[') {
                format!("[{host}]")
            } else {
                host.into()
            }
        ))?;
        #[cfg(windows)]
        {
            let script = format!(
                "$ErrorActionPreference='Stop'; $rows=@(Get-NetTCPConnection -State Listen -LocalPort {port} | ForEach-Object {{ $proc=Get-CimInstance Win32_Process -Filter ('ProcessId='+$_.OwningProcess); [pscustomobject]@{{pid=$_.OwningProcess;process=$proc.Name;executable=$proc.ExecutablePath;started=$proc.CreationDate.ToString('yyyy-MM-dd HH:mm:ss');address=$_.LocalAddress;port=$_.LocalPort}} }}); ConvertTo-Json -InputObject $rows -Compress"
            );
            let raw = output(
                "powershell.exe",
                &["-NoProfile", "-NonInteractive", "-Command", &script],
            )
            .await?;
            parse_windows_listener(&raw, port, host)
        }
        #[cfg(not(windows))]
        {
            let binary = if cfg!(target_os = "macos") {
                "/usr/sbin/lsof"
            } else {
                "lsof"
            };
            let raw = output(
                binary,
                &[
                    "-nP",
                    "-a",
                    &format!("-iTCP:{port}"),
                    "-sTCP:LISTEN",
                    "-Fpn",
                ],
            )
            .await?;
            let pid = parse_lsof_listener(&raw, port, host)?;
            let identity = self.process(pid).await?;
            let repeated = output(
                binary,
                &[
                    "-nP",
                    "-a",
                    &format!("-iTCP:{port}"),
                    "-sTCP:LISTEN",
                    "-Fpn",
                ],
            )
            .await?;
            if parse_lsof_listener(&repeated, port, host)? != pid {
                return Err(invalid("listener changed during probe"));
            }
            Ok(Listener {
                identity,
                address: host.into(),
                port,
            })
        }
    }
    async fn process(&self, pid: u32) -> Result<ProcessIdentity, StateError> {
        if pid == 0 || pid > i32::MAX as u32 {
            return Err(invalid("invalid process PID"));
        }
        #[cfg(windows)]
        {
            let script = format!(
                "$ErrorActionPreference='Stop'; $proc=Get-CimInstance Win32_Process -Filter 'ProcessId={pid}'; [pscustomobject]@{{pid=$proc.ProcessId;process=$proc.Name;executable=$proc.ExecutablePath;started=$proc.CreationDate.ToString('yyyy-MM-dd HH:mm:ss')}} | ConvertTo-Json -Compress"
            );
            let raw = output(
                "powershell.exe",
                &["-NoProfile", "-NonInteractive", "-Command", &script],
            )
            .await?;
            let identity: ProcessIdentity =
                serde_json::from_str(&raw).map_err(|_| invalid("invalid process output"))?;
            validate_process(&identity)?;
            if identity.pid != pid {
                return Err(invalid("process PID drift"));
            }
            Ok(identity)
        }
        #[cfg(not(windows))]
        {
            let pid_text = pid.to_string();
            let started =
                parse_start_time(&output("/bin/ps", &["-p", &pid_text, "-o", "lstart="]).await?)?;
            let executable = if cfg!(target_os = "macos") {
                let raw = output(
                    "/usr/sbin/lsof",
                    &["-a", "-p", &pid_text, "-d", "txt", "-Fn"],
                )
                .await?;
                let rows: Vec<_> = raw.lines().collect();
                rows.windows(2)
                    .find_map(|pair| {
                        if pair[0] == "ftxt" {
                            pair[1].strip_prefix('n')
                        } else {
                            None
                        }
                    })
                    .ok_or_else(|| invalid("process executable unavailable"))?
                    .to_owned()
            } else {
                std::fs::read_link(format!("/proc/{pid}/exe"))
                    .map_err(|_| invalid("process executable unavailable"))?
                    .to_string_lossy()
                    .into_owned()
            };
            let executable = std::fs::canonicalize(executable)
                .map_err(|_| invalid("process executable cannot be resolved"))?
                .to_string_lossy()
                .into_owned();
            let process = std::path::Path::new(&executable)
                .file_name()
                .ok_or_else(|| invalid("missing process name"))?
                .to_string_lossy()
                .into_owned();
            if parse_start_time(&output("/bin/ps", &["-p", &pid_text, "-o", "lstart="]).await?)?
                != started
            {
                return Err(invalid("process changed during probe"));
            }
            let identity = ProcessIdentity {
                pid,
                process,
                executable,
                started,
            };
            validate_process(&identity)?;
            Ok(identity)
        }
    }
}
