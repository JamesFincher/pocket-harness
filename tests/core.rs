use std::fs;
use std::os::unix::fs::PermissionsExt;

use pocket_harness::config::{AppConfig, ConnectorConfig, ConnectorKind};
use pocket_harness::config_store::{ConfigSource, ConfigStore};
use pocket_harness::connector::ConnectorManager;

#[test]
fn default_config_validates_and_builtin_echo_runs() {
    let config = AppConfig::default();
    config.validate().unwrap();

    let manager = ConnectorManager::new(&config);
    let response = manager.run("main", "hello").unwrap();

    assert!(response.ok);
    assert!(response.message.contains("hello"));
}

#[test]
fn config_store_promotes_and_loads_last_known_good() {
    let temp = tempfile::tempdir().unwrap();
    let config_path = temp.path().join("pocket-harness.yaml");
    let store = ConfigStore::new(&config_path);

    store.init_default(false).unwrap();
    let active = store.load_with_recovery().unwrap();
    assert_eq!(active.source, ConfigSource::Primary);

    fs::write(&config_path, "not: [valid").unwrap();
    let recovered = store.load_with_recovery().unwrap();
    assert_eq!(recovered.source, ConfigSource::LastKnownGood);
}

#[test]
fn config_store_uses_fallback_snapshot_when_configured_state_dir_fails() {
    let temp = tempfile::tempdir().unwrap();
    let config_path = temp.path().join("pocket-harness.yaml");
    let broken_state_dir = temp.path().join("state-file");
    fs::write(&broken_state_dir, "not a directory").unwrap();

    let mut config = AppConfig::default();
    config.gateway.data_dir = broken_state_dir.to_string_lossy().to_string();
    fs::write(&config_path, serde_yaml::to_string(&config).unwrap()).unwrap();

    let store = ConfigStore::new(&config_path);
    let active = store.load_with_recovery().unwrap();
    assert_eq!(active.source, ConfigSource::Primary);

    fs::write(&config_path, "not: [valid").unwrap();
    let recovered = store.load_with_recovery().unwrap();
    assert_eq!(recovered.source, ConfigSource::LastKnownGood);
}

#[test]
fn json_connector_runs_over_stdin_stdout() {
    let temp = tempfile::tempdir().unwrap();
    let script = temp.path().join("connector.sh");

    fs::write(
        &script,
        r#"#!/bin/sh
read request
case "$request" in
  *'"kind":"health"'*) printf '%s\n' '{"ok":true,"message":"healthy","capabilities":["connector.health","connector.run","connector.cancel","threads.cwd","attachments.images"]}' ;;
  *) printf '%s\n' '{"ok":true,"message":"ran json connector","capabilities":["connector.health","connector.run","connector.cancel","threads.cwd","attachments.images"]}' ;;
esac
"#,
    )
    .unwrap();

    let mut permissions = fs::metadata(&script).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&script, permissions).unwrap();

    let mut config = AppConfig::default();
    config.connectors.default = "json_echo".to_string();
    config.connectors.definitions.insert(
        "json_echo".to_string(),
        ConnectorConfig {
            kind: ConnectorKind::Json,
            display_name: "JSON Echo".to_string(),
            command: vec![script.to_string_lossy().to_string()],
            cwd: temp.path().to_string_lossy().to_string(),
            timeout_seconds: 5,
            env: Default::default(),
            settings: Default::default(),
        },
    );

    config.validate().unwrap();

    let manager = ConnectorManager::new(&config);
    manager.check_all().unwrap();
    let response = manager.run("main", "hello").unwrap();

    assert!(response.ok);
    assert_eq!(response.message, "ran json connector");
}
