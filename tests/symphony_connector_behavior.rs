use std::collections::BTreeMap;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use pocket_harness::config::{AppConfig, ConnectorConfig, ConnectorKind};
use pocket_harness::connector::ConnectorManager;
use serde_yaml::Value;

fn symphony_script() -> String {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("connectors/symphony/symphony_connector.py")
        .to_string_lossy()
        .to_string()
}

fn fake_symphony_tree(root: &Path) -> (PathBuf, PathBuf) {
    let elixir_dir = root.join("symphony/elixir");
    fs::create_dir_all(&elixir_dir).unwrap();
    fs::write(
        elixir_dir.join("mix.exs"),
        "defmodule Fake.MixProject do\nend\n",
    )
    .unwrap();
    let workflow = elixir_dir.join("WORKFLOW.md");
    fs::write(&workflow, "---\ntracker:\n  kind: memory\n---\n").unwrap();
    (elixir_dir, workflow)
}

fn executable_script(path: &Path, contents: &str) {
    fs::write(path, contents).unwrap();

    let mut permissions = fs::metadata(path).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(path, permissions).unwrap();
}

fn symphony_connector(elixir_dir: &Path, workflow: &Path) -> ConnectorConfig {
    let mut settings = BTreeMap::new();
    settings.insert(
        "elixir_dir".to_string(),
        Value::String(elixir_dir.to_string_lossy().to_string()),
    );
    settings.insert(
        "workflow".to_string(),
        Value::String(workflow.to_string_lossy().to_string()),
    );

    ConnectorConfig {
        kind: ConnectorKind::Json,
        display_name: "Symphony".to_string(),
        command: vec!["python3".to_string(), symphony_script()],
        cwd: env!("CARGO_MANIFEST_DIR").to_string(),
        timeout_seconds: 30,
        env: BTreeMap::new(),
        settings,
    }
}

fn config_with_symphony(connector: ConnectorConfig) -> AppConfig {
    let mut config = AppConfig::default();
    config.connectors.default = "symphony".to_string();
    config
        .connectors
        .definitions
        .insert("symphony".to_string(), connector);
    config
}

#[test]
fn symphony_connector_health_reports_required_capabilities() {
    let temp = tempfile::tempdir().unwrap();
    let (elixir_dir, workflow) = fake_symphony_tree(temp.path());
    let config = config_with_symphony(symphony_connector(&elixir_dir, &workflow));
    config.validate().unwrap();

    let manager = ConnectorManager::new(&config);
    manager.check_all().unwrap();

    let connector = config.connectors.definitions.get("symphony").unwrap();
    let health = manager.health("symphony", connector).unwrap();

    assert!(health.ok);
    assert_eq!(
        health.message,
        "Symphony connector healthy; compiled Symphony bin/symphony was not found"
    );
    assert!(
        health
            .capabilities
            .contains(&"connector.health".to_string())
    );
    assert!(health.capabilities.contains(&"connector.run".to_string()));
    assert!(
        health
            .capabilities
            .contains(&"connector.cancel".to_string())
    );
    assert!(health.capabilities.contains(&"threads.cwd".to_string()));
    assert!(
        health
            .capabilities
            .contains(&"attachments.images".to_string())
    );
}

#[test]
fn symphony_connector_dry_run_is_safe_without_worker_command() {
    let temp = tempfile::tempdir().unwrap();
    let (elixir_dir, workflow) = fake_symphony_tree(temp.path());
    let config = config_with_symphony(symphony_connector(&elixir_dir, &workflow));

    let response = ConnectorManager::new(&config)
        .run("main", "summarize Symphony state")
        .unwrap();

    assert!(response.ok);
    assert!(
        response
            .message
            .contains("Configure settings.run_command to execute a Symphony mobile worker")
    );
    assert_eq!(
        response
            .metadata
            .get("mode")
            .and_then(|value| value.as_str()),
        Some("dry_run")
    );
}

#[test]
fn symphony_connector_rejects_missing_workflow_during_health_check() {
    let temp = tempfile::tempdir().unwrap();
    let (elixir_dir, workflow) = fake_symphony_tree(temp.path());
    fs::remove_file(&workflow).unwrap();
    let config = config_with_symphony(symphony_connector(&elixir_dir, &workflow));

    let error = ConnectorManager::new(&config)
        .check_all()
        .unwrap_err()
        .to_string();

    assert!(error.contains("reported unhealthy"));
    assert!(error.contains("missing file for workflow"));
}

#[test]
fn symphony_connector_can_delegate_run_to_configured_command() {
    let temp = tempfile::tempdir().unwrap();
    let (elixir_dir, workflow) = fake_symphony_tree(temp.path());
    let worker = temp.path().join("worker.sh");
    executable_script(
        &worker,
        r#"#!/bin/sh
read request
printf 'delegated thread=%s prompt=%s workflow=%s\n' "$POCKET_HARNESS_THREAD_ID" "$POCKET_HARNESS_PROMPT" "$SYMPHONY_WORKFLOW"
"#,
    );

    let mut connector = symphony_connector(&elixir_dir, &workflow);
    connector.settings.insert(
        "run_command".to_string(),
        Value::Sequence(vec![Value::String(worker.to_string_lossy().to_string())]),
    );

    let config = config_with_symphony(connector);
    let response = ConnectorManager::new(&config)
        .run("main", "mobile prompt")
        .unwrap();

    assert!(response.ok);
    assert!(response.message.contains("delegated thread=main"));
    assert!(response.message.contains("prompt=mobile prompt"));
    assert!(
        response
            .message
            .contains(&workflow.to_string_lossy().to_string())
    );
}
