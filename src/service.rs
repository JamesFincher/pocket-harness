use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, anyhow, bail};

use crate::config::{expand_path, home_dir};
use crate::config_store::atomic_write;

pub const DEFAULT_SERVICE_NAME: &str = "pocket-harness";

#[derive(Debug, Clone)]
pub struct ServiceOptions {
    pub config_path: PathBuf,
    pub env_file: PathBuf,
    pub service_name: String,
    pub binary_path: PathBuf,
    pub log_dir: PathBuf,
}

impl ServiceOptions {
    pub fn new(config_path: PathBuf, env_file: PathBuf, service_name: Option<String>) -> Self {
        let data_dir = home_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join(".pocket-harness");
        let binary_path = current_binary_path().unwrap_or_else(|| PathBuf::from("pocket-harness"));
        Self {
            config_path,
            env_file,
            service_name: service_name.unwrap_or_else(|| DEFAULT_SERVICE_NAME.to_string()),
            binary_path,
            log_dir: data_dir.join("logs"),
        }
    }

    pub fn command_args(&self) -> Vec<String> {
        vec![
            "--env-file".to_string(),
            self.env_file.display().to_string(),
            "--config".to_string(),
            self.config_path.display().to_string(),
            "telegram".to_string(),
        ]
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServicePlatform {
    SystemdUser,
    Launchd,
    WindowsTask,
}

pub fn detect_platform() -> Option<ServicePlatform> {
    if cfg!(target_os = "macos") {
        Some(ServicePlatform::Launchd)
    } else if cfg!(target_os = "windows") {
        Some(ServicePlatform::WindowsTask)
    } else if command_exists("systemctl") {
        Some(ServicePlatform::SystemdUser)
    } else {
        None
    }
}

pub fn install(options: &ServiceOptions) -> Result<PathBuf> {
    let platform =
        detect_platform().ok_or_else(|| anyhow!("no supported service manager found"))?;
    fs::create_dir_all(&options.log_dir)
        .with_context(|| format!("create log dir {}", options.log_dir.display()))?;

    match platform {
        ServicePlatform::SystemdUser => install_systemd(options),
        ServicePlatform::Launchd => install_launchd(options),
        ServicePlatform::WindowsTask => install_windows_task(options),
    }
}

pub fn uninstall(options: &ServiceOptions) -> Result<()> {
    match detect_platform() {
        Some(ServicePlatform::SystemdUser) => {
            run_allow_fail(Command::new("systemctl").args([
                "--user",
                "disable",
                "--now",
                &unit_name(options),
            ]));
            let path = systemd_unit_path(options);
            remove_file_if_exists(&path)?;
            run_allow_fail(Command::new("systemctl").args(["--user", "daemon-reload"]));
        }
        Some(ServicePlatform::Launchd) => {
            let path = launchd_plist_path(options);
            let label = launchd_label(options);
            let domain = launchd_domain();
            run_allow_fail(Command::new("launchctl").args([
                "bootout",
                &domain,
                path.to_string_lossy().as_ref(),
            ]));
            run_allow_fail(
                Command::new("launchctl").args(["disable", &format!("{domain}/{label}")]),
            );
            remove_file_if_exists(&path)?;
        }
        Some(ServicePlatform::WindowsTask) => {
            run_allow_fail(Command::new("schtasks").args([
                "/Delete",
                "/TN",
                &options.service_name,
                "/F",
            ]));
            remove_file_if_exists(&windows_launcher_path(options))?;
        }
        None => bail!("no supported service manager found"),
    }
    Ok(())
}

pub fn start(options: &ServiceOptions) -> Result<()> {
    match detect_platform() {
        Some(ServicePlatform::SystemdUser) => {
            run(Command::new("systemctl").args(["--user", "start", &unit_name(options)]))
        }
        Some(ServicePlatform::Launchd) => {
            let label = launchd_label(options);
            let domain = launchd_domain();
            run(Command::new("launchctl").args(["kickstart", "-k", &format!("{domain}/{label}")]))
        }
        Some(ServicePlatform::WindowsTask) => {
            run(Command::new("schtasks").args(["/Run", "/TN", &options.service_name]))
        }
        None => bail!("no supported service manager found"),
    }
}

pub fn stop(options: &ServiceOptions) -> Result<()> {
    match detect_platform() {
        Some(ServicePlatform::SystemdUser) => {
            run(Command::new("systemctl").args(["--user", "stop", &unit_name(options)]))
        }
        Some(ServicePlatform::Launchd) => {
            let label = launchd_label(options);
            let domain = launchd_domain();
            run(Command::new("launchctl").args(["kill", "TERM", &format!("{domain}/{label}")]))
        }
        Some(ServicePlatform::WindowsTask) => {
            run(Command::new("schtasks").args(["/End", "/TN", &options.service_name]))
        }
        None => bail!("no supported service manager found"),
    }
}

pub fn restart(options: &ServiceOptions) -> Result<()> {
    match detect_platform() {
        Some(ServicePlatform::SystemdUser) => {
            run(Command::new("systemctl").args(["--user", "restart", &unit_name(options)]))
        }
        _ => {
            let _ = stop(options);
            start(options)
        }
    }
}

pub fn status(options: &ServiceOptions) -> Result<()> {
    match detect_platform() {
        Some(ServicePlatform::SystemdUser) => run(Command::new("systemctl").args([
            "--user",
            "status",
            "--no-pager",
            &unit_name(options),
        ])),
        Some(ServicePlatform::Launchd) => {
            let label = launchd_label(options);
            let domain = launchd_domain();
            run_redacted(Command::new("launchctl").args(["print", &format!("{domain}/{label}")]))
        }
        Some(ServicePlatform::WindowsTask) => {
            run(Command::new("schtasks").args(["/Query", "/TN", &options.service_name, "/V"]))
        }
        None => bail!("no supported service manager found"),
    }
}

fn install_systemd(options: &ServiceOptions) -> Result<PathBuf> {
    let path = systemd_unit_path(options);
    atomic_write(&path, &render_systemd_unit(options))?;
    run(Command::new("systemctl").args(["--user", "daemon-reload"]))?;
    run(Command::new("systemctl").args(["--user", "enable", "--now", &unit_name(options)]))?;
    Ok(path)
}

fn install_launchd(options: &ServiceOptions) -> Result<PathBuf> {
    let path = launchd_plist_path(options);
    atomic_write(&path, &render_launchd_plist(options))?;
    let domain = launchd_domain();
    let label = launchd_label(options);
    let service_target = format!("{domain}/{label}");
    run_allow_fail(Command::new("launchctl").args(["bootout", &service_target]));
    run_allow_fail(Command::new("launchctl").args([
        "bootout",
        &domain,
        path.to_string_lossy().as_ref(),
    ]));
    run(Command::new("launchctl").args(["bootstrap", &domain, path.to_string_lossy().as_ref()]))?;
    run(Command::new("launchctl").args(["enable", &service_target]))?;
    run(Command::new("launchctl").args(["kickstart", "-k", &service_target]))?;
    Ok(path)
}

fn install_windows_task(options: &ServiceOptions) -> Result<PathBuf> {
    let path = windows_launcher_path(options);
    atomic_write(&path, &render_windows_launcher(options))?;
    run(Command::new("schtasks").args([
        "/Create",
        "/TN",
        &options.service_name,
        "/SC",
        "ONLOGON",
        "/TR",
        path.to_string_lossy().as_ref(),
        "/F",
    ]))?;
    run(Command::new("schtasks").args(["/Run", "/TN", &options.service_name]))?;
    Ok(path)
}

pub fn render_systemd_unit(options: &ServiceOptions) -> String {
    format!(
        "[Unit]\nDescription=Pocket Harness Telegram gateway\nAfter=network-online.target\nWants=network-online.target\n\n[Service]\nType=simple\nEnvironmentFile={env_file}\nExecStart={binary} {args}\nRestart=always\nRestartSec=5\nWorkingDirectory={workdir}\nStandardOutput=append:{stdout}\nStandardError=append:{stderr}\n\n[Install]\nWantedBy=default.target\n",
        env_file = options.env_file.display(),
        binary = options.binary_path.display(),
        args = shell_join(&options.command_args()),
        workdir = options
            .config_path
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .display(),
        stdout = options.log_dir.join("service.log").display(),
        stderr = options.log_dir.join("service.err.log").display(),
    )
}

pub fn render_launchd_plist(options: &ServiceOptions) -> String {
    let mut program_args = vec![options.binary_path.display().to_string()];
    program_args.extend(options.command_args());
    let args = program_args
        .iter()
        .map(|arg| format!("    <string>{}</string>", xml_escape(arg)))
        .collect::<Vec<_>>()
        .join("\n");
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n<plist version=\"1.0\">\n<dict>\n  <key>Label</key>\n  <string>{label}</string>\n  <key>ProgramArguments</key>\n  <array>\n{args}\n  </array>\n  <key>EnvironmentVariables</key>\n  <dict>\n{env_vars}\n  </dict>\n  <key>RunAtLoad</key>\n  <true/>\n  <key>KeepAlive</key>\n  <true/>\n  <key>WorkingDirectory</key>\n  <string>{workdir}</string>\n  <key>StandardOutPath</key>\n  <string>{stdout}</string>\n  <key>StandardErrorPath</key>\n  <string>{stderr}</string>\n</dict>\n</plist>\n",
        label = launchd_label(options),
        args = args,
        env_vars = render_launchd_env_vars(&options.env_file),
        workdir = xml_escape(
            &options
                .config_path
                .parent()
                .unwrap_or_else(|| Path::new("."))
                .display()
                .to_string()
        ),
        stdout = xml_escape(&options.log_dir.join("service.log").display().to_string()),
        stderr = xml_escape(
            &options
                .log_dir
                .join("service.err.log")
                .display()
                .to_string()
        ),
    )
}

pub fn render_windows_launcher(options: &ServiceOptions) -> String {
    format!(
        "@echo off\r\nsetlocal\r\nif exist \"{env}\" for /f \"usebackq tokens=1,* delims==\" %%A in (\"{env}\") do set \"%%A=%%B\"\r\n\"{binary}\" {args}\r\n",
        env = options.env_file.display(),
        binary = options.binary_path.display(),
        args = shell_join(&options.command_args()),
    )
}

fn render_launchd_env_vars(env_file: &Path) -> String {
    [
        "    <key>POCKET_HARNESS_ENV_FILE</key>".to_string(),
        format!(
            "    <string>{}</string>",
            xml_escape(&env_file.display().to_string())
        ),
    ]
    .join("\n")
}

fn systemd_unit_path(options: &ServiceOptions) -> PathBuf {
    home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".config")
        .join("systemd")
        .join("user")
        .join(unit_name(options))
}

fn launchd_plist_path(options: &ServiceOptions) -> PathBuf {
    home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("Library")
        .join("LaunchAgents")
        .join(format!("{}.plist", launchd_label(options)))
}

fn windows_launcher_path(options: &ServiceOptions) -> PathBuf {
    options
        .log_dir
        .join(format!("{}.cmd", options.service_name))
}

fn unit_name(options: &ServiceOptions) -> String {
    format!("{}.service", options.service_name)
}

fn launchd_label(options: &ServiceOptions) -> String {
    format!(
        "com.pocketharness.{}",
        options.service_name.replace('-', ".")
    )
}

fn launchd_domain() -> String {
    let uid = env::var("UID").unwrap_or_else(|_| unsafe { libc_getuid().to_string() });
    format!("gui/{uid}")
}

unsafe fn libc_getuid() -> u32 {
    unsafe extern "C" {
        fn getuid() -> u32;
    }
    unsafe { getuid() }
}

fn current_binary_path() -> Option<PathBuf> {
    env::current_exe().ok().or_else(|| {
        env::var_os("PATH").and_then(|paths| {
            env::split_paths(&paths)
                .map(|path| path.join("pocket-harness"))
                .find(|path| path.exists())
        })
    })
}

fn command_exists(command: &str) -> bool {
    env::var_os("PATH")
        .map(|paths| env::split_paths(&paths).any(|path| path.join(command).exists()))
        .unwrap_or(false)
}

fn run(command: &mut Command) -> Result<()> {
    let output = command
        .output()
        .with_context(|| format!("run {:?}", command))?;
    print!("{}", String::from_utf8_lossy(&output.stdout));
    eprint!("{}", String::from_utf8_lossy(&output.stderr));
    if output.status.success() {
        Ok(())
    } else {
        bail!(
            "command failed with status {}: {:?}",
            output.status,
            command
        )
    }
}

fn run_allow_fail(command: &mut Command) {
    let _ = command.output();
}

fn run_redacted(command: &mut Command) -> Result<()> {
    let output = command
        .output()
        .with_context(|| format!("run {:?}", command))?;
    print!(
        "{}",
        redact_secrets(&String::from_utf8_lossy(&output.stdout))
    );
    eprint!(
        "{}",
        redact_secrets(&String::from_utf8_lossy(&output.stderr))
    );
    if output.status.success() {
        Ok(())
    } else {
        bail!(
            "command failed with status {}: {:?}",
            output.status,
            command
        )
    }
}

fn redact_secrets(text: &str) -> String {
    text.lines()
        .map(|line| {
            if let Some((left, _right)) = line.split_once("=>") {
                let key = left.trim();
                if is_secret_key(key) {
                    return format!("{left}=> [redacted]");
                }
            }
            line.to_string()
        })
        .collect::<Vec<_>>()
        .join("\n")
        + if text.ends_with('\n') { "\n" } else { "" }
}

fn is_secret_key(key: &str) -> bool {
    let upper = key.to_ascii_uppercase();
    upper.contains("TOKEN")
        || upper.contains("API_KEY")
        || upper.contains("SECRET")
        || upper.contains("PASSWORD")
}

fn remove_file_if_exists(path: &Path) -> Result<()> {
    if path.exists() {
        fs::remove_file(path).with_context(|| format!("remove {}", path.display()))?;
    }
    Ok(())
}

fn shell_join(args: &[String]) -> String {
    args.iter()
        .map(|arg| shell_escape(arg))
        .collect::<Vec<_>>()
        .join(" ")
}

fn shell_escape(value: &str) -> String {
    if value
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || "/._-:=+".contains(ch))
    {
        value.to_string()
    } else {
        format!("'{}'", value.replace('\'', "'\\''"))
    }
}

fn xml_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

pub fn expand_service_path(raw: &str) -> PathBuf {
    expand_path(raw)
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::{
        ServiceOptions, redact_secrets, render_launchd_plist, render_systemd_unit,
        render_windows_launcher,
    };

    fn options() -> ServiceOptions {
        ServiceOptions {
            config_path: "/tmp/pocket/config.yaml".into(),
            env_file: "/tmp/pocket/env".into(),
            service_name: "pocket-harness".to_string(),
            binary_path: "/usr/local/bin/pocket-harness".into(),
            log_dir: "/tmp/pocket/logs".into(),
        }
    }

    #[test]
    fn renders_systemd_unit_with_env_and_restart_policy() {
        let unit = render_systemd_unit(&options());
        assert!(unit.contains("EnvironmentFile=/tmp/pocket/env"));
        assert!(unit.contains("Restart=always"));
        assert!(unit.contains("--config /tmp/pocket/config.yaml telegram"));
    }

    #[test]
    fn renders_launchd_plist_with_keepalive() {
        let plist = render_launchd_plist(&options());
        assert!(plist.contains("<key>KeepAlive</key>"));
        assert!(plist.contains("/usr/local/bin/pocket-harness"));
        assert!(plist.contains("/tmp/pocket/config.yaml"));
    }

    #[test]
    fn launchd_plist_points_to_env_file_without_embedding_secrets() {
        let temp = tempfile::tempdir().unwrap();
        let env_file = temp.path().join("env");
        fs::write(
            &env_file,
            "TELEGRAM_BOT_TOKEN=telegram-secret\nGEMINI_API_KEY=gemini-secret\n",
        )
        .unwrap();
        let mut options = options();
        options.env_file = env_file.clone();
        let plist = render_launchd_plist(&options);

        assert!(plist.contains("<key>POCKET_HARNESS_ENV_FILE</key>"));
        assert!(plist.contains(&env_file.display().to_string()));
        assert!(!plist.contains("TELEGRAM_BOT_TOKEN"));
        assert!(!plist.contains("GEMINI_API_KEY"));
        assert!(!plist.contains("telegram-secret"));
        assert!(!plist.contains("gemini-secret"));
    }

    #[test]
    fn renders_windows_launcher_with_env_loader() {
        let script = render_windows_launcher(&options());
        assert!(script.contains("for /f"));
        assert!(script.contains("pocket-harness"));
    }

    #[test]
    fn redacts_secret_values_from_service_status() {
        let status = "environment = {\n\t\tGEMINI_API_KEY => abc123\n\t\tPATH => /bin\n\t\tTELEGRAM_BOT_TOKEN => token\n\t}\n";
        let redacted = redact_secrets(status);

        assert!(redacted.contains("GEMINI_API_KEY => [redacted]"));
        assert!(redacted.contains("TELEGRAM_BOT_TOKEN => [redacted]"));
        assert!(redacted.contains("PATH => /bin"));
        assert!(!redacted.contains("abc123"));
        assert!(!redacted.contains("token\n"));
    }
}
