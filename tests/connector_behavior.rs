use std::collections::BTreeMap;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use pocket_harness::config::{AppConfig, ConnectorConfig, ConnectorKind};
use pocket_harness::connector::ConnectorManager;

fn executable_script(path: &Path, contents: &str) {
    fs::write(path, contents).unwrap();

    let mut permissions = fs::metadata(path).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(path, permissions).unwrap();
}

fn json_connector_config(
    command: Vec<String>,
    cwd: &Path,
    timeout_seconds: u64,
) -> ConnectorConfig {
    ConnectorConfig {
        kind: ConnectorKind::Json,
        display_name: "Test JSON".to_string(),
        command,
        cwd: cwd.to_string_lossy().to_string(),
        timeout_seconds,
        env: BTreeMap::new(),
        settings: BTreeMap::new(),
    }
}

fn config_with_default_json(name: &str, connector: ConnectorConfig) -> AppConfig {
    let mut config = AppConfig::default();
    config.connectors.default = name.to_string();
    config
        .connectors
        .definitions
        .insert(name.to_string(), connector);
    config
}

#[test]
fn builtin_echo_reports_health_and_capabilities() {
    let config = AppConfig::default();
    let manager = ConnectorManager::new(&config);
    let connector = config.connectors.definitions.get("echo").unwrap();

    let health = manager.health("echo", connector).unwrap();
    assert!(health.ok);
    assert_eq!(health.message, "builtin echo connector healthy");
    assert!(
        health
            .capabilities
            .contains(&"connector.health".to_string())
    );
    assert!(health.capabilities.contains(&"connector.run".to_string()));
    assert!(
        health
            .capabilities
            .contains(&"connector.status".to_string())
    );
    assert!(
        health
            .capabilities
            .contains(&"connector.capabilities".to_string())
    );
    assert!(health.capabilities.contains(&"threads.cwd".to_string()));
    assert!(
        health
            .capabilities
            .contains(&"attachments.images".to_string())
    );

    let capabilities = manager.capabilities("echo", connector).unwrap();
    assert!(capabilities.ok);
    assert_eq!(capabilities.message, "builtin echo connector capabilities");
    assert_eq!(capabilities.capabilities, health.capabilities);
}

#[test]
fn json_connector_parses_last_json_stdout_line_after_log_noise() {
    let temp = tempfile::tempdir().unwrap();
    let script = temp.path().join("noisy_connector.sh");
    executable_script(
        &script,
        r#"#!/bin/sh
read request
printf '%s\n' 'starting connector'
printf '%s\n' "$request" | sed 's/^/request: /'
printf '%s\n' '{"ok":true,"message":"parsed despite log noise","capabilities":["connector.health","connector.run"],"metadata":{"source":"last-line"}}'
"#,
    );

    let config = config_with_default_json(
        "noisy",
        json_connector_config(vec![script.to_string_lossy().to_string()], temp.path(), 5),
    );
    config.validate().unwrap();

    let response = ConnectorManager::new(&config).run("main", "hello").unwrap();

    assert!(response.ok);
    assert_eq!(response.message, "parsed despite log noise");
    assert_eq!(
        response
            .metadata
            .get("source")
            .and_then(|value| value.as_str()),
        Some("last-line")
    );
}

#[test]
fn json_connector_non_zero_exit_is_reported_as_failure() {
    let temp = tempfile::tempdir().unwrap();
    let script = temp.path().join("failing_connector.sh");
    executable_script(
        &script,
        r#"#!/bin/sh
read request
printf '%s\n' "boom from connector" >&2
exit 7
"#,
    );

    let config = config_with_default_json(
        "failing",
        json_connector_config(vec![script.to_string_lossy().to_string()], temp.path(), 5),
    );
    config.validate().unwrap();

    let err = ConnectorManager::new(&config)
        .run("main", "hello")
        .unwrap_err()
        .to_string();

    assert!(err.contains("connector exited with status"));
    assert!(err.contains("boom from connector"));
}

#[test]
fn connector_health_ok_false_fails_check_all() {
    let temp = tempfile::tempdir().unwrap();
    let script = temp.path().join("unhealthy_connector.sh");
    executable_script(
        &script,
        r#"#!/bin/sh
read request
printf '%s\n' '{"ok":false,"message":"health probe failed","capabilities":["connector.health","connector.run","connector.cancel","threads.cwd","attachments.images"]}'
"#,
    );

    let config = config_with_default_json(
        "unhealthy",
        json_connector_config(vec![script.to_string_lossy().to_string()], temp.path(), 5),
    );
    config.validate().unwrap();

    let error = ConnectorManager::new(&config)
        .check_all()
        .unwrap_err()
        .to_string();

    assert!(error.contains("reported unhealthy"));
    assert!(error.contains("health probe failed"));
}

#[test]
fn json_connector_timeout_is_reported_as_failure() {
    let temp = tempfile::tempdir().unwrap();
    let script = temp.path().join("slow_connector.sh");
    executable_script(
        &script,
        r#"#!/bin/sh
read request
sleep 2
printf '%s\n' '{"ok":true,"message":"too late"}'
"#,
    );

    let config = config_with_default_json(
        "slow",
        json_connector_config(vec![script.to_string_lossy().to_string()], temp.path(), 1),
    );
    config.validate().unwrap();

    let err = ConnectorManager::new(&config)
        .run("main", "hello")
        .unwrap_err()
        .to_string();

    assert_eq!(err, "connector timed out after 1s");
}

#[test]
fn connector_env_values_are_expanded_before_process_spawn() {
    let temp = tempfile::tempdir().unwrap();
    let script = temp.path().join("env_connector.sh");
    executable_script(
        &script,
        r#"#!/bin/sh
read request
case "$HARNESS_EXPANDED_ENV" in
  expanded:"$HOME":suffix)
    printf '%s\n' '{"ok":true,"message":"env expanded"}'
    ;;
  *)
    printf '%s\n' "HARNESS_EXPANDED_ENV=$HARNESS_EXPANDED_ENV" >&2
    exit 9
    ;;
esac
"#,
    );

    let mut connector =
        json_connector_config(vec![script.to_string_lossy().to_string()], temp.path(), 5);
    connector.env.insert(
        "HARNESS_EXPANDED_ENV".to_string(),
        "expanded:${HOME}:suffix".to_string(),
    );

    let config = config_with_default_json("env", connector);
    config.validate().unwrap();

    let response = ConnectorManager::new(&config).run("main", "hello").unwrap();

    assert!(response.ok);
    assert_eq!(response.message, "env expanded");
}

#[test]
fn unknown_thread_uses_default_connector_and_main_thread_settings() {
    let temp = tempfile::tempdir().unwrap();
    let mut config = AppConfig::default();
    config.threads.get_mut("main").unwrap().cwd = temp.path().to_string_lossy().to_string();
    config.validate().unwrap();

    let response = ConnectorManager::new(&config)
        .run("unknown-mobile-thread", "hello")
        .unwrap();

    assert!(response.ok);
    assert!(response.message.contains("thread=unknown-mobile-thread"));
    assert!(
        response
            .message
            .contains(&format!("cwd={}", temp.path().to_string_lossy()))
    );
    assert!(response.message.contains("prompt=hello"));
}
