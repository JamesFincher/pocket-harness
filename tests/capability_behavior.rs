use std::collections::BTreeMap;

use pocket_harness::config::{AppConfig, ConnectorConfig, ConnectorKind, ThreadConfig};
use pocket_harness::connector::ConnectorManager;

fn json_connector() -> ConnectorConfig {
    ConnectorConfig {
        kind: ConnectorKind::Json,
        display_name: "JSON".to_string(),
        command: vec!["/bin/echo".to_string()],
        cwd: ".".to_string(),
        timeout_seconds: 10,
        env: BTreeMap::new(),
        settings: BTreeMap::new(),
    }
}

fn config_with_json_default() -> AppConfig {
    let mut config = AppConfig::default();
    config.connectors.default = "json".to_string();
    config
        .connectors
        .definitions
        .insert("json".to_string(), json_connector());
    config
}

#[test]
fn selected_default_connector_requires_capabilities_for_enabled_features() {
    let config = config_with_json_default();
    let manager = ConnectorManager::new(&config);

    assert_eq!(
        manager.required_capabilities("json"),
        vec![
            "attachments.images",
            "connector.cancel",
            "connector.health",
            "connector.run",
            "threads.cwd",
        ]
    );

    let error = manager
        .validate_capabilities(
            "json",
            &["connector.health".to_string(), "connector.run".to_string()],
        )
        .unwrap_err()
        .to_string();

    assert!(error.contains("attachments.images"));
    assert!(error.contains("connector.cancel"));
    assert!(error.contains("threads.cwd"));
}

#[test]
fn disabling_connector_dependent_features_reduces_required_capabilities() {
    let mut config = config_with_json_default();
    config.features.images.enabled = false;
    config.features.cancel.enabled = false;
    config.features.threads.enabled = false;

    let manager = ConnectorManager::new(&config);

    assert_eq!(
        manager.required_capabilities("json"),
        vec!["connector.health", "connector.run"]
    );
    manager
        .validate_capabilities(
            "json",
            &["connector.health".to_string(), "connector.run".to_string()],
        )
        .unwrap();
}

#[test]
fn thread_watch_requires_stream_capability_for_that_connector() {
    let mut config = config_with_json_default();
    config
        .threads
        .get_mut("main")
        .expect("main thread")
        .watch
        .enabled = true;

    let manager = ConnectorManager::new(&config);
    let required = manager.required_capabilities("json");

    assert!(required.contains(&"connector.stream".to_string()));

    let error = manager
        .validate_capabilities(
            "json",
            &[
                "attachments.images".to_string(),
                "connector.cancel".to_string(),
                "connector.health".to_string(),
                "connector.run".to_string(),
                "threads.cwd".to_string(),
            ],
        )
        .unwrap_err()
        .to_string();

    assert!(error.contains("connector.stream"));
}

#[test]
fn unselected_connector_only_requires_health_and_run() {
    let mut config = AppConfig::default();
    config
        .connectors
        .definitions
        .insert("unused".to_string(), json_connector());

    let manager = ConnectorManager::new(&config);

    assert_eq!(
        manager.required_capabilities("unused"),
        vec!["connector.health", "connector.run"]
    );
}

#[test]
fn connector_selected_by_secondary_thread_gets_feature_requirements() {
    let mut config = AppConfig::default();
    config
        .connectors
        .definitions
        .insert("json".to_string(), json_connector());

    let mobile = ThreadConfig {
        connector: Some("json".to_string()),
        ..Default::default()
    };
    config.threads.insert("mobile".to_string(), mobile);

    let manager = ConnectorManager::new(&config);
    let required = manager.required_capabilities("json");

    assert!(required.contains(&"connector.health".to_string()));
    assert!(required.contains(&"connector.run".to_string()));
    assert!(required.contains(&"connector.cancel".to_string()));
    assert!(required.contains(&"threads.cwd".to_string()));
    assert!(required.contains(&"attachments.images".to_string()));
}
