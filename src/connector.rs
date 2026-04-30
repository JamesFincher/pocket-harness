use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::process::{Command, Stdio};
use std::time::Duration;

use anyhow::{Context, Result, anyhow};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;
use wait_timeout::ChildExt;

use crate::config::{
    AppConfig, ConnectorConfig, ConnectorKind, ThreadConfig, expand_path, expand_string,
};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConnectorRequestKind {
    Health,
    Capabilities,
    Status,
    Run,
    Cancel,
    Shutdown,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConnectorRequest {
    pub kind: ConnectorRequestKind,
    pub request_id: String,
    pub thread_id: String,
    pub prompt: String,
    pub cwd: String,
    pub attachments: Vec<Attachment>,
    pub settings: BTreeMap<String, serde_yaml::Value>,
    pub metadata: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Attachment {
    pub kind: String,
    pub path: String,
    pub mime_type: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ConnectorResponse {
    pub ok: bool,
    pub message: String,
    pub capabilities: Vec<String>,
    pub retryable: bool,
    pub metadata: BTreeMap<String, Value>,
}

impl Default for ConnectorResponse {
    fn default() -> Self {
        Self {
            ok: true,
            message: String::new(),
            capabilities: Vec::new(),
            retryable: false,
            metadata: BTreeMap::new(),
        }
    }
}

pub struct ConnectorManager<'a> {
    config: &'a AppConfig,
}

impl<'a> ConnectorManager<'a> {
    pub fn new(config: &'a AppConfig) -> Self {
        Self { config }
    }

    pub fn check_all(&self) -> Result<()> {
        for (name, connector) in &self.config.connectors.definitions {
            self.health(name, connector)
                .with_context(|| format!("connector `{name}` health check failed"))?;
        }
        Ok(())
    }

    pub fn health(&self, name: &str, connector: &ConnectorConfig) -> Result<ConnectorResponse> {
        let request = ConnectorRequest {
            kind: ConnectorRequestKind::Health,
            request_id: Uuid::new_v4().to_string(),
            thread_id: "health".to_string(),
            prompt: String::new(),
            cwd: ".".to_string(),
            attachments: Vec::new(),
            settings: connector.settings.clone(),
            metadata: BTreeMap::from([("connector".to_string(), Value::String(name.to_string()))]),
        };

        self.dispatch(connector, request)
    }

    pub fn capabilities(
        &self,
        name: &str,
        connector: &ConnectorConfig,
    ) -> Result<ConnectorResponse> {
        let request = ConnectorRequest {
            kind: ConnectorRequestKind::Capabilities,
            request_id: Uuid::new_v4().to_string(),
            thread_id: "capabilities".to_string(),
            prompt: String::new(),
            cwd: ".".to_string(),
            attachments: Vec::new(),
            settings: connector.settings.clone(),
            metadata: BTreeMap::from([("connector".to_string(), Value::String(name.to_string()))]),
        };

        self.dispatch(connector, request)
    }

    pub fn run(&self, thread_name: &str, prompt: &str) -> Result<ConnectorResponse> {
        let (connector_name, connector) = self.config.connector_for_thread(thread_name)?;
        let thread = self.config.thread_or_default(thread_name);
        let request = run_request(connector_name, thread_name, connector, &thread, prompt);
        self.dispatch(connector, request)
    }

    pub fn dispatch(
        &self,
        connector: &ConnectorConfig,
        request: ConnectorRequest,
    ) -> Result<ConnectorResponse> {
        match connector.kind {
            ConnectorKind::BuiltinEcho => Ok(echo_response(&request)),
            ConnectorKind::Json => run_json_connector(connector, &request),
        }
    }
}

fn run_request(
    connector_name: &str,
    thread_name: &str,
    connector: &ConnectorConfig,
    thread: &ThreadConfig,
    prompt: &str,
) -> ConnectorRequest {
    let cwd = if thread.cwd.trim().is_empty() {
        ".".to_string()
    } else {
        expand_path(&thread.cwd).to_string_lossy().to_string()
    };

    ConnectorRequest {
        kind: ConnectorRequestKind::Run,
        request_id: Uuid::new_v4().to_string(),
        thread_id: thread_name.to_string(),
        prompt: prompt.to_string(),
        cwd,
        attachments: Vec::new(),
        settings: connector.settings.clone(),
        metadata: BTreeMap::from([
            (
                "connector".to_string(),
                Value::String(connector_name.to_string()),
            ),
            (
                "reply_style".to_string(),
                Value::String(format!("{:?}", thread.reply_style).to_lowercase()),
            ),
        ]),
    }
}

fn echo_response(request: &ConnectorRequest) -> ConnectorResponse {
    match request.kind {
        ConnectorRequestKind::Health => ConnectorResponse {
            ok: true,
            message: "builtin echo connector healthy".to_string(),
            capabilities: default_capabilities(),
            retryable: false,
            metadata: BTreeMap::new(),
        },
        ConnectorRequestKind::Capabilities => ConnectorResponse {
            ok: true,
            message: "builtin echo connector capabilities".to_string(),
            capabilities: default_capabilities(),
            retryable: false,
            metadata: BTreeMap::new(),
        },
        ConnectorRequestKind::Run => ConnectorResponse {
            ok: true,
            message: format!(
                "echo thread={} cwd={} prompt={}",
                request.thread_id, request.cwd, request.prompt
            ),
            capabilities: default_capabilities(),
            retryable: false,
            metadata: BTreeMap::new(),
        },
        _ => ConnectorResponse {
            ok: true,
            message: "builtin echo connector accepted control request".to_string(),
            capabilities: default_capabilities(),
            retryable: false,
            metadata: BTreeMap::new(),
        },
    }
}

fn default_capabilities() -> Vec<String> {
    [
        "connector.health",
        "connector.run",
        "connector.status",
        "connector.capabilities",
        "threads.cwd",
        "attachments.images",
    ]
    .into_iter()
    .map(str::to_string)
    .collect()
}

fn run_json_connector(
    connector: &ConnectorConfig,
    request: &ConnectorRequest,
) -> Result<ConnectorResponse> {
    let program = connector
        .command
        .first()
        .ok_or_else(|| anyhow!("json connector command is empty"))?;

    let args = connector
        .command
        .iter()
        .skip(1)
        .map(|arg| expand_string(arg));
    let cwd = expand_path(&connector.cwd);
    let timeout = Duration::from_secs(connector.timeout_seconds);

    let mut command = Command::new(expand_string(program));
    command
        .args(args)
        .current_dir(cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    for (key, value) in &connector.env {
        command.env(key, expand_string(value));
    }

    let mut child = command.spawn().context("spawn connector command")?;

    let input = serde_json::to_vec(request).context("encode connector request")?;
    {
        let stdin = child
            .stdin
            .as_mut()
            .ok_or_else(|| anyhow!("connector stdin unavailable"))?;
        stdin.write_all(&input)?;
        stdin.write_all(b"\n")?;
    }
    drop(child.stdin.take());

    match child.wait_timeout(timeout)? {
        Some(status) => {
            let mut stdout = String::new();
            let mut stderr = String::new();

            if let Some(mut out) = child.stdout.take() {
                out.read_to_string(&mut stdout)?;
            }
            if let Some(mut err) = child.stderr.take() {
                err.read_to_string(&mut stderr)?;
            }

            if !status.success() {
                return Err(anyhow!(
                    "connector exited with status {}: {}",
                    status,
                    stderr.trim()
                ));
            }

            parse_connector_response(&stdout)
                .with_context(|| format!("parse connector response; stderr={}", stderr.trim()))
        }
        None => {
            let _ = child.kill();
            let _ = child.wait();
            Err(anyhow!("connector timed out after {}s", timeout.as_secs()))
        }
    }
}

fn parse_connector_response(stdout: &str) -> Result<ConnectorResponse> {
    let trimmed = stdout.trim();
    if trimmed.is_empty() {
        return Err(anyhow!("connector returned empty stdout"));
    }

    if let Ok(response) = serde_json::from_str::<ConnectorResponse>(trimmed) {
        return Ok(response);
    }

    let last_json_line = trimmed
        .lines()
        .rev()
        .map(str::trim)
        .find(|line| line.starts_with('{') && line.ends_with('}'))
        .ok_or_else(|| anyhow!("connector stdout did not contain a JSON response"))?;

    serde_json::from_str(last_json_line).context("parse last JSON response line")
}
