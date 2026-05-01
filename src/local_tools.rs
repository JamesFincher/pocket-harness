use std::fs;
use std::fs::OpenOptions;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use wait_timeout::ChildExt;

use crate::config::{AppConfig, expand_path};
use crate::yaml_edit::set_value;

const MAX_LIST_ENTRIES: usize = 200;
const MAX_FIND_RESULTS: usize = 200;
const MAX_GREP_RESULTS: usize = 200;
const MAX_READ_BYTES: usize = 24 * 1024;
const MAX_OUTPUT_BYTES: usize = 32 * 1024;
const MAX_WALK_DEPTH: usize = 8;
const COMMAND_TIMEOUT: Duration = Duration::from_secs(60);
const TERMINAL_TAIL_BYTES: usize = 12 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalToolCall {
    pub name: String,
    pub args: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ToolKind {
    Pwd,
    Cd,
    Ls,
    Find,
    Grep,
    Cat,
    Sh,
    Bg,
    TermList,
    TermTail,
    TermKill,
    Sudo,
    SudoBg,
}

impl ToolKind {
    fn from_name(name: &str) -> Option<Self> {
        match name {
            "pwd" => Some(Self::Pwd),
            "cd" => Some(Self::Cd),
            "ls" => Some(Self::Ls),
            "find" => Some(Self::Find),
            "grep" => Some(Self::Grep),
            "cat" => Some(Self::Cat),
            "sh" => Some(Self::Sh),
            "bg" => Some(Self::Bg),
            "term_list" => Some(Self::TermList),
            "term_tail" => Some(Self::TermTail),
            "term_kill" => Some(Self::TermKill),
            "sudo" => Some(Self::Sudo),
            "sudo_bg" => Some(Self::SudoBg),
            _ => None,
        }
    }
}

pub struct LocalToolState {
    terminals: Vec<TerminalSession>,
    next_terminal_id: u64,
}

struct TerminalSession {
    id: String,
    command: String,
    cwd: PathBuf,
    log_path: PathBuf,
    child: Child,
}

impl Default for LocalToolState {
    fn default() -> Self {
        Self {
            terminals: Vec::new(),
            next_terminal_id: 1,
        }
    }
}

impl LocalToolState {
    pub fn run_tool(
        &mut self,
        config_path: &Path,
        config: &AppConfig,
        thread: &str,
        call: &LocalToolCall,
    ) -> Result<String> {
        let Some(kind) = ToolKind::from_name(&call.name) else {
            bail!("unknown local tool `{}`", call.name);
        };

        match kind {
            ToolKind::Pwd
            | ToolKind::Cd
            | ToolKind::Ls
            | ToolKind::Find
            | ToolKind::Grep
            | ToolKind::Cat
            | ToolKind::Sh => run_tool(config_path, config, thread, call),
            ToolKind::Bg => {
                if !config.features.terminal.enabled {
                    bail!("terminal feature is disabled");
                }
                let command = call.args.join(" ");
                self.start_terminal(config_path, config, thread, &command, None)
            }
            ToolKind::TermList => self.list_terminals(),
            ToolKind::TermTail => {
                let id = call.args.first().map(String::as_str).unwrap_or_default();
                self.tail_terminal(id)
            }
            ToolKind::TermKill => {
                let id = call.args.first().map(String::as_str).unwrap_or_default();
                self.kill_terminal(id)
            }
            ToolKind::Sudo => {
                if !config.features.terminal.enabled {
                    bail!("terminal feature is disabled");
                }
                let (password, command) = parse_password_command(&call.args)?;
                sudo(config, thread, &password, &command)
            }
            ToolKind::SudoBg => {
                if !config.features.terminal.enabled {
                    bail!("terminal feature is disabled");
                }
                let (password, command) = parse_password_command(&call.args)?;
                self.start_terminal(config_path, config, thread, &command, Some(&password))
            }
        }
    }

    fn start_terminal(
        &mut self,
        config_path: &Path,
        config: &AppConfig,
        thread: &str,
        command: &str,
        sudo_password: Option<&str>,
    ) -> Result<String> {
        if command.trim().is_empty() {
            bail!("usage: /bg <command>");
        }
        if self.terminals.len() >= config.features.terminal.max_sessions {
            bail!(
                "terminal session limit reached ({})",
                config.features.terminal.max_sessions
            );
        }
        let id = format!("t{}", self.next_terminal_id);
        self.next_terminal_id += 1;
        let cwd = current_cwd(config, thread);
        let log_dir = terminal_log_dir(config_path, config);
        fs::create_dir_all(&log_dir).with_context(|| format!("create {}", log_dir.display()))?;
        let log_path = log_dir.join(format!("{id}.log"));
        let stdout = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log_path)
            .with_context(|| format!("open {}", log_path.display()))?;
        let stderr = stdout.try_clone().context("clone terminal log handle")?;

        let child = if sudo_password.is_some() {
            let mut child = Command::new("sudo")
                .arg("-S")
                .arg("-p")
                .arg("")
                .arg("/bin/sh")
                .arg("-lc")
                .arg(command)
                .current_dir(&cwd)
                .stdin(Stdio::piped())
                .stdout(Stdio::from(stdout))
                .stderr(Stdio::from(stderr))
                .spawn()
                .with_context(|| format!("start sudo terminal in {}", cwd.display()))?;
            if let Some(stdin) = child.stdin.as_mut() {
                use std::io::Write;
                writeln!(stdin, "{}", sudo_password.unwrap()).context("send sudo password")?;
            }
            child
        } else {
            Command::new("/bin/sh")
                .arg("-lc")
                .arg(command)
                .current_dir(&cwd)
                .stdin(Stdio::null())
                .stdout(Stdio::from(stdout))
                .stderr(Stdio::from(stderr))
                .spawn()
                .with_context(|| format!("start terminal in {}", cwd.display()))?
        };

        let pid = child.id();
        self.terminals.push(TerminalSession {
            id: id.clone(),
            command: command.to_string(),
            cwd: cwd.clone(),
            log_path: log_path.clone(),
            child,
        });

        Ok(format!(
            "started terminal {id} pid={pid}\ncwd={}\nlog={}",
            cwd.display(),
            log_path.display()
        ))
    }

    fn list_terminals(&mut self) -> Result<String> {
        if self.terminals.is_empty() {
            return Ok("No terminal sessions.".to_string());
        }
        let mut lines = Vec::new();
        for session in &mut self.terminals {
            let status = match session.child.try_wait()? {
                Some(status) => format!("exited {status}"),
                None => "running".to_string(),
            };
            lines.push(format!(
                "{} {} cwd={} command={}",
                session.id,
                status,
                session.cwd.display(),
                session.command
            ));
        }
        Ok(lines.join("\n"))
    }

    fn tail_terminal(&mut self, id: &str) -> Result<String> {
        let session = self
            .terminals
            .iter_mut()
            .find(|session| session.id == id)
            .ok_or_else(|| anyhow!("unknown terminal session `{id}`"))?;
        let status = match session.child.try_wait()? {
            Some(status) => format!("exited {status}"),
            None => "running".to_string(),
        };
        let text = tail_file(&session.log_path, TERMINAL_TAIL_BYTES)?;
        Ok(format!(
            "{} {}\n{}\n{}",
            session.id,
            status,
            session.log_path.display(),
            if text.trim().is_empty() {
                "(no output yet)".to_string()
            } else {
                text
            }
        ))
    }

    fn kill_terminal(&mut self, id: &str) -> Result<String> {
        let index = self
            .terminals
            .iter()
            .position(|session| session.id == id)
            .ok_or_else(|| anyhow!("unknown terminal session `{id}`"))?;
        let mut session = self.terminals.remove(index);
        match session.child.try_wait()? {
            Some(status) => Ok(format!("terminal {id} already exited with {status}")),
            None => {
                session.child.kill()?;
                let status = session.child.wait()?;
                Ok(format!("killed terminal {id}; status {status}"))
            }
        }
    }
}

pub fn current_cwd(config: &AppConfig, thread: &str) -> PathBuf {
    let thread = config.thread_or_default(thread);
    if thread.cwd.trim().is_empty() {
        PathBuf::from(".")
    } else {
        expand_path(&thread.cwd)
    }
}

pub fn run_tool(
    config_path: &Path,
    config: &AppConfig,
    thread: &str,
    call: &LocalToolCall,
) -> Result<String> {
    let Some(kind) = ToolKind::from_name(&call.name) else {
        bail!("unknown local tool `{}`", call.name);
    };

    match kind {
        ToolKind::Pwd => Ok(current_cwd(config, thread).display().to_string()),
        ToolKind::Cd => {
            let path = call.args.first().map(String::as_str).unwrap_or_default();
            cd(config_path, config, thread, path)
        }
        ToolKind::Ls => {
            let path = call.args.first().map(String::as_str);
            ls(config, thread, path)
        }
        ToolKind::Find => {
            let pattern = call.args.first().map(String::as_str).unwrap_or_default();
            let path = call.args.get(1).map(String::as_str);
            find(config, thread, pattern, path)
        }
        ToolKind::Grep => {
            let pattern = call.args.first().map(String::as_str).unwrap_or_default();
            let path = call.args.get(1).map(String::as_str);
            grep(config, thread, pattern, path)
        }
        ToolKind::Cat => {
            let path = call.args.first().map(String::as_str).unwrap_or_default();
            cat(config, thread, path)
        }
        ToolKind::Sh => {
            if !config.features.terminal.enabled {
                bail!("terminal feature is disabled");
            }
            let command = call.args.join(" ");
            sh(config, thread, &command)
        }
        ToolKind::Bg
        | ToolKind::TermList
        | ToolKind::TermTail
        | ToolKind::TermKill
        | ToolKind::Sudo
        | ToolKind::SudoBg => bail!("persistent terminal tools require gateway runtime state"),
    }
}

pub fn try_parse_natural(text: &str) -> Option<LocalToolCall> {
    let trimmed = text.trim();
    let lower = trimmed.to_ascii_lowercase();

    if matches!(
        lower.as_str(),
        "pwd" | "where am i" | "what directory am i in"
    ) {
        return Some(call("pwd", []));
    }

    for prefix in [
        "cd ",
        "go to ",
        "go into ",
        "change directory to ",
        "change directories to ",
        "open folder ",
    ] {
        if let Some(value) = trimmed.strip_prefix_case(prefix) {
            return Some(call("cd", [clean_path(value)]));
        }
    }

    for prefix in ["ls", "list files", "show files"] {
        if lower == prefix {
            return Some(call("ls", []));
        }
        if let Some(value) = trimmed.strip_prefix_case(&(prefix.to_string() + " ")) {
            return Some(call("ls", [clean_path(value)]));
        }
    }

    for prefix in ["find ", "find file ", "find files ", "search for file "] {
        if let Some(value) = trimmed.strip_prefix_case(prefix) {
            return Some(call("find", [clean_query(value)]));
        }
    }

    for prefix in ["grep ", "search text ", "search for text "] {
        if let Some(value) = trimmed.strip_prefix_case(prefix) {
            return Some(call("grep", [clean_query(value)]));
        }
    }

    for prefix in ["cat ", "show file ", "read file ", "open file "] {
        if let Some(value) = trimmed.strip_prefix_case(prefix) {
            return Some(call("cat", [clean_path(value)]));
        }
    }

    for prefix in ["run in background ", "start background ", "start terminal "] {
        if let Some(value) = trimmed.strip_prefix_case(prefix) {
            return Some(call("bg", [value.trim().to_string()]));
        }
    }

    for prefix in [
        "run ",
        "run command ",
        "execute ",
        "execute command ",
        "terminal ",
    ] {
        if let Some(value) = trimmed.strip_prefix_case(prefix) {
            return Some(call("sh", [value.trim().to_string()]));
        }
    }

    None
}

pub fn is_terminal_request(text: &str) -> bool {
    matches!(try_parse_natural(text), Some(call) if call.name == "sh" || call.name == "bg")
        || text.to_ascii_lowercase().contains(" in terminal")
}

fn cd(config_path: &Path, config: &AppConfig, thread: &str, raw: &str) -> Result<String> {
    if raw.trim().is_empty() {
        bail!("usage: /cd <path>");
    }
    let path = resolve_path(config, thread, raw)?;
    if !path.is_dir() {
        bail!("not a directory: {}", path.display());
    }
    set_value(
        config_path,
        &format!("threads.{thread}.cwd"),
        &path.to_string_lossy(),
    )?;
    Ok(format!("cwd set to {}", path.display()))
}

fn ls(config: &AppConfig, thread: &str, raw: Option<&str>) -> Result<String> {
    let path = resolve_optional_path(config, thread, raw)?;
    if !path.is_dir() {
        bail!("not a directory: {}", path.display());
    }
    let mut entries = fs::read_dir(&path)
        .with_context(|| format!("read directory {}", path.display()))?
        .collect::<Result<Vec<_>, _>>()
        .with_context(|| format!("read directory {}", path.display()))?;
    entries.sort_by_key(|entry| entry.file_name());

    let mut lines = vec![format!("{}:", path.display())];
    let total = entries.len();
    for entry in entries.into_iter().take(MAX_LIST_ENTRIES) {
        let file_type = entry.file_type()?;
        let suffix = if file_type.is_dir() { "/" } else { "" };
        lines.push(format!("{}{}", entry.file_name().to_string_lossy(), suffix));
    }
    if total > MAX_LIST_ENTRIES {
        lines.push(format!("... {} more", total - MAX_LIST_ENTRIES));
    }
    Ok(lines.join("\n"))
}

fn find(config: &AppConfig, thread: &str, pattern: &str, raw_path: Option<&str>) -> Result<String> {
    let pattern = pattern.trim();
    if pattern.is_empty() {
        bail!("usage: /find <pattern> [path]");
    }
    let root = resolve_optional_path(config, thread, raw_path)?;
    let needle = pattern.to_ascii_lowercase();
    let mut results = Vec::new();
    walk(&root, 0, &mut |path| {
        if results.len() >= MAX_FIND_RESULTS {
            return;
        }
        if path
            .file_name()
            .map(|name| {
                name.to_string_lossy()
                    .to_ascii_lowercase()
                    .contains(&needle)
            })
            .unwrap_or(false)
        {
            results.push(path.to_path_buf());
        }
    })?;
    Ok(format_paths(results, "No matching files."))
}

fn grep(config: &AppConfig, thread: &str, pattern: &str, raw_path: Option<&str>) -> Result<String> {
    let pattern = pattern.trim();
    if pattern.is_empty() {
        bail!("usage: /grep <pattern> [path]");
    }
    let path = resolve_optional_path(config, thread, raw_path)?;
    if command_exists("rg") {
        let output = Command::new("rg")
            .arg("--line-number")
            .arg("--color")
            .arg("never")
            .arg("--")
            .arg(pattern)
            .arg(&path)
            .output()
            .context("run rg")?;
        if output.status.success() || output.status.code() == Some(1) {
            let text = String::from_utf8_lossy(&output.stdout);
            return Ok(cap_lines(text.trim(), MAX_GREP_RESULTS, "No matches."));
        }
        bail!("{}", String::from_utf8_lossy(&output.stderr).trim());
    }

    let mut matches = Vec::new();
    walk(&path, 0, &mut |candidate| {
        if matches.len() >= MAX_GREP_RESULTS || !candidate.is_file() {
            return;
        }
        if let Ok(text) = fs::read_to_string(candidate) {
            for (idx, line) in text.lines().enumerate() {
                if line.contains(pattern) {
                    matches.push(format!("{}:{}:{}", candidate.display(), idx + 1, line));
                    if matches.len() >= MAX_GREP_RESULTS {
                        break;
                    }
                }
            }
        }
    })?;
    if matches.is_empty() {
        Ok("No matches.".to_string())
    } else {
        Ok(matches.join("\n"))
    }
}

fn cat(config: &AppConfig, thread: &str, raw: &str) -> Result<String> {
    if raw.trim().is_empty() {
        bail!("usage: /cat <path>");
    }
    let path = resolve_path(config, thread, raw)?;
    if !path.is_file() {
        bail!("not a file: {}", path.display());
    }
    let bytes = fs::read(&path).with_context(|| format!("read {}", path.display()))?;
    if bytes.contains(&0) {
        bail!("refusing to print binary file: {}", path.display());
    }
    let truncated = bytes.len() > MAX_READ_BYTES;
    let text = String::from_utf8_lossy(&bytes[..bytes.len().min(MAX_READ_BYTES)]);
    let mut out = format!("{}:\n{}", path.display(), text);
    if truncated {
        out.push_str(&format!("\n... truncated at {} bytes", MAX_READ_BYTES));
    }
    Ok(out)
}

fn sh(config: &AppConfig, thread: &str, command: &str) -> Result<String> {
    if command.trim().is_empty() {
        bail!("usage: /sh <command>");
    }
    let cwd = current_cwd(config, thread);
    let output = run_with_timeout(command, &cwd)?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let mut text = String::new();
    if !stdout.trim().is_empty() {
        text.push_str(stdout.trim_end());
    }
    if !stderr.trim().is_empty() {
        if !text.is_empty() {
            text.push('\n');
        }
        text.push_str(stderr.trim_end());
    }
    if text.is_empty() {
        text = format!("command exited with status {}", output.status);
    }
    if !output.status.success() {
        text = format!("command failed with status {}\n{}", output.status, text);
    }
    Ok(cap_bytes(&text, MAX_OUTPUT_BYTES))
}

fn sudo(config: &AppConfig, thread: &str, password: &str, command: &str) -> Result<String> {
    if command.trim().is_empty() {
        bail!("usage: /sudo <password> -- <command>");
    }
    let cwd = current_cwd(config, thread);
    let mut child = Command::new("sudo")
        .arg("-S")
        .arg("-p")
        .arg("")
        .arg("/bin/sh")
        .arg("-lc")
        .arg(command)
        .current_dir(&cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| format!("run sudo command in {}", cwd.display()))?;
    if let Some(stdin) = child.stdin.as_mut() {
        use std::io::Write;
        writeln!(stdin, "{password}").context("send sudo password")?;
    }
    match child.wait_timeout(COMMAND_TIMEOUT)? {
        Some(_status) => {
            let output = child.wait_with_output().context("read sudo output")?;
            let stdout = String::from_utf8_lossy(&output.stdout);
            let stderr = String::from_utf8_lossy(&output.stderr);
            let combined = [stdout.trim_end(), stderr.trim_end()]
                .into_iter()
                .filter(|part| !part.is_empty())
                .collect::<Vec<_>>()
                .join("\n");
            if output.status.success() {
                Ok(cap_bytes(&combined, MAX_OUTPUT_BYTES))
            } else {
                Ok(cap_bytes(
                    &format!(
                        "sudo command failed with status {}\n{}",
                        output.status, combined
                    ),
                    MAX_OUTPUT_BYTES,
                ))
            }
        }
        None => {
            let _ = child.kill();
            let _ = child.wait();
            bail!(
                "sudo command timed out after {}s",
                COMMAND_TIMEOUT.as_secs()
            )
        }
    }
}

fn run_with_timeout(command: &str, cwd: &Path) -> Result<std::process::Output> {
    let mut child = Command::new("/bin/sh")
        .arg("-lc")
        .arg(command)
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| format!("run command in {}", cwd.display()))?;

    match child.wait_timeout(COMMAND_TIMEOUT)? {
        Some(_status) => child.wait_with_output().context("read command output"),
        None => {
            let _ = child.kill();
            let _ = child.wait();
            bail!("command timed out after {}s", COMMAND_TIMEOUT.as_secs())
        }
    }
}

fn resolve_optional_path(config: &AppConfig, thread: &str, raw: Option<&str>) -> Result<PathBuf> {
    match raw.map(str::trim).filter(|value| !value.is_empty()) {
        Some(raw) => resolve_path(config, thread, raw),
        None => Ok(current_cwd(config, thread)),
    }
}

fn resolve_path(config: &AppConfig, thread: &str, raw: &str) -> Result<PathBuf> {
    let raw = clean_path(raw);
    let path = PathBuf::from(&raw);
    let resolved = if raw.starts_with("~/") || raw == "~" {
        expand_path(&raw)
    } else if path.is_absolute() {
        path
    } else {
        current_cwd(config, thread).join(path)
    };
    normalize_path(&resolved)
}

fn terminal_log_dir(config_path: &Path, config: &AppConfig) -> PathBuf {
    config.data_dir(config_path).join("logs").join("terminals")
}

fn parse_password_command(args: &[String]) -> Result<(String, String)> {
    if args.len() == 1 && args[0].contains(" -- ") {
        let (password, command) = args[0].split_once(" -- ").expect("contains checked");
        if password.trim().is_empty() || command.trim().is_empty() {
            bail!("usage: /sudo <password> -- <command>");
        }
        return Ok((password.trim().to_string(), command.trim().to_string()));
    }
    if args.len() >= 3 && args[1] == "--" {
        let command = args[2..].join(" ");
        return Ok((args[0].trim().to_string(), command));
    }
    bail!("usage: /sudo <password> -- <command>")
}

fn tail_file(path: &Path, max_bytes: usize) -> Result<String> {
    let bytes = fs::read(path).with_context(|| format!("read {}", path.display()))?;
    let start = bytes.len().saturating_sub(max_bytes);
    Ok(String::from_utf8_lossy(&bytes[start..]).to_string())
}

fn normalize_path(path: &Path) -> Result<PathBuf> {
    if path.exists() {
        return path
            .canonicalize()
            .with_context(|| format!("resolve {}", path.display()));
    }
    let parent = path
        .parent()
        .ok_or_else(|| anyhow!("invalid path: {}", path.display()))?;
    let parent = parent
        .canonicalize()
        .with_context(|| format!("resolve {}", parent.display()))?;
    let name = path
        .file_name()
        .ok_or_else(|| anyhow!("invalid path: {}", path.display()))?;
    Ok(parent.join(name))
}

fn walk<F>(root: &Path, depth: usize, f: &mut F) -> Result<()>
where
    F: FnMut(&Path),
{
    if depth > MAX_WALK_DEPTH {
        return Ok(());
    }
    f(root);
    if !root.is_dir() {
        return Ok(());
    }
    for entry in fs::read_dir(root).with_context(|| format!("read directory {}", root.display()))? {
        let entry = entry?;
        let path = entry.path();
        if is_ignored_dir(&path) {
            continue;
        }
        walk(&path, depth + 1, f)?;
    }
    Ok(())
}

fn is_ignored_dir(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .map(|name| {
            matches!(
                name,
                ".git" | "target" | "node_modules" | ".next" | ".cache" | "__pycache__"
            )
        })
        .unwrap_or(false)
}

fn format_paths(paths: Vec<PathBuf>, empty: &str) -> String {
    if paths.is_empty() {
        empty.to_string()
    } else {
        paths
            .into_iter()
            .map(|path| path.display().to_string())
            .collect::<Vec<_>>()
            .join("\n")
    }
}

fn cap_lines(text: &str, max_lines: usize, empty: &str) -> String {
    if text.is_empty() {
        return empty.to_string();
    }
    let lines = text.lines().take(max_lines).collect::<Vec<_>>();
    let mut out = lines.join("\n");
    if text.lines().count() > max_lines {
        out.push_str(&format!("\n... truncated at {max_lines} lines"));
    }
    out
}

fn cap_bytes(text: &str, max_bytes: usize) -> String {
    if text.len() <= max_bytes {
        text.to_string()
    } else {
        let clipped = text
            .char_indices()
            .take_while(|(idx, ch)| idx + ch.len_utf8() <= max_bytes)
            .map(|(_, ch)| ch)
            .collect::<String>();
        format!("{}...\ntruncated at {} bytes", clipped, max_bytes)
    }
}

fn command_exists(command: &str) -> bool {
    std::env::var_os("PATH")
        .map(|paths| std::env::split_paths(&paths).any(|path| path.join(command).exists()))
        .unwrap_or(false)
}

fn call<const N: usize>(name: &str, args: [String; N]) -> LocalToolCall {
    LocalToolCall {
        name: name.to_string(),
        args: args.into_iter().collect(),
    }
}

fn clean_query(value: &str) -> String {
    clean_path(value)
}

fn clean_path(value: &str) -> String {
    value
        .trim()
        .trim_matches('"')
        .trim_matches('\'')
        .trim_end_matches('.')
        .to_string()
}

trait StripPrefixCase {
    fn strip_prefix_case<'a>(&'a self, prefix: &str) -> Option<&'a str>;
}

impl StripPrefixCase for str {
    fn strip_prefix_case<'a>(&'a self, prefix: &str) -> Option<&'a str> {
        if self.len() < prefix.len() {
            return None;
        }
        let (head, tail) = self.split_at(prefix.len());
        head.eq_ignore_ascii_case(prefix).then_some(tail)
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use crate::config::AppConfig;

    use super::{LocalToolCall, LocalToolState, current_cwd, run_tool, try_parse_natural};

    #[test]
    fn parses_natural_directory_and_terminal_requests() {
        assert_eq!(
            try_parse_natural("go to ~/code").unwrap(),
            LocalToolCall {
                name: "cd".to_string(),
                args: vec!["~/code".to_string()],
            }
        );
        assert_eq!(try_parse_natural("run cargo test").unwrap().name, "sh");
        assert_eq!(
            try_parse_natural("run in background sleep 60")
                .unwrap()
                .name,
            "bg"
        );
    }

    #[test]
    fn cd_updates_thread_cwd_and_ls_reads_it() {
        let temp = tempfile::tempdir().unwrap();
        let config_path = temp.path().join("config.yaml");
        let project = temp.path().join("project");
        fs::create_dir(&project).unwrap();
        fs::write(project.join("Cargo.toml"), "[package]\n").unwrap();
        fs::write(
            &config_path,
            serde_yaml::to_string(&AppConfig::default()).unwrap(),
        )
        .unwrap();

        let config = AppConfig::default();
        let reply = run_tool(
            &config_path,
            &config,
            "main",
            &LocalToolCall {
                name: "cd".to_string(),
                args: vec![project.to_string_lossy().to_string()],
            },
        )
        .unwrap();
        assert!(reply.contains("cwd set to"));

        let config: AppConfig =
            serde_yaml::from_str(&fs::read_to_string(&config_path).unwrap()).unwrap();
        assert_eq!(
            current_cwd(&config, "main"),
            project.canonicalize().unwrap()
        );
        let listing = run_tool(
            &config_path,
            &config,
            "main",
            &LocalToolCall {
                name: "ls".to_string(),
                args: vec![],
            },
        )
        .unwrap();
        assert!(listing.contains("Cargo.toml"));
    }

    #[test]
    fn background_terminal_persists_until_killed() {
        let temp = tempfile::tempdir().unwrap();
        let config_path = temp.path().join("config.yaml");
        let mut config = AppConfig::default();
        config.gateway.data_dir = temp.path().join("state").to_string_lossy().to_string();
        fs::write(&config_path, serde_yaml::to_string(&config).unwrap()).unwrap();

        let mut state = LocalToolState::default();
        let started = state
            .run_tool(
                &config_path,
                &config,
                "main",
                &LocalToolCall {
                    name: "bg".to_string(),
                    args: vec!["printf ready; sleep 30".to_string()],
                },
            )
            .unwrap();
        assert!(started.contains("terminal t1"));

        let listed = state
            .run_tool(
                &config_path,
                &config,
                "main",
                &LocalToolCall {
                    name: "term_list".to_string(),
                    args: vec![],
                },
            )
            .unwrap();
        assert!(listed.contains("t1"));

        let killed = state
            .run_tool(
                &config_path,
                &config,
                "main",
                &LocalToolCall {
                    name: "term_kill".to_string(),
                    args: vec!["t1".to_string()],
                },
            )
            .unwrap();
        assert!(killed.contains("killed terminal t1"));
    }
}
