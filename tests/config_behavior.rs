use std::collections::BTreeMap;
use std::path::Path;

use pocket_harness::config::{
    AppConfig, ConfigError, ConnectorConfig, ConnectorKind, ThreadConfig, expand_path,
    expand_string,
};
use pocket_harness::config_store::parse_and_validate;

fn assert_config_error(config: &AppConfig, expected: fn(&ConfigError) -> bool) {
    let error = config.validate().expect_err("config should be invalid");
    assert!(expected(&error), "unexpected config error: {error:?}",);
}

#[test]
fn validates_default_connector_and_connector_definition_edges() {
    let mut missing_default = AppConfig::default();
    missing_default.connectors.default = "missing".to_string();
    assert_config_error(
        &missing_default,
        |error| matches!(error, ConfigError::MissingDefaultConnector(name) if name == "missing"),
    );

    let mut empty_json_command = AppConfig::default();
    empty_json_command.connectors.definitions.insert(
        "json".to_string(),
        ConnectorConfig {
            kind: ConnectorKind::Json,
            display_name: "JSON".to_string(),
            command: Vec::new(),
            cwd: ".".to_string(),
            timeout_seconds: 10,
            env: BTreeMap::new(),
            settings: BTreeMap::new(),
        },
    );
    assert_config_error(
        &empty_json_command,
        |error| matches!(error, ConfigError::EmptyConnectorCommand(name) if name == "json"),
    );

    let mut invalid_timeout = AppConfig::default();
    invalid_timeout
        .connectors
        .definitions
        .get_mut("echo")
        .unwrap()
        .timeout_seconds = 0;
    assert_config_error(
        &invalid_timeout,
        |error| matches!(error, ConfigError::InvalidConnectorTimeout(name) if name == "echo"),
    );
}

#[test]
fn selects_thread_specific_connector_or_default_connector() {
    let mut config = AppConfig::default();
    config.connectors.definitions.insert(
        "worker".to_string(),
        ConnectorConfig {
            kind: ConnectorKind::BuiltinEcho,
            display_name: "Worker".to_string(),
            command: Vec::new(),
            cwd: ".".to_string(),
            timeout_seconds: 5,
            env: BTreeMap::new(),
            settings: BTreeMap::new(),
        },
    );

    let explicit_thread = ThreadConfig {
        connector: Some("worker".to_string()),
        ..Default::default()
    };
    config
        .threads
        .insert("explicit".to_string(), explicit_thread);

    config.validate().unwrap();

    let (default_name, default_connector) = config.connector_for_thread("main").unwrap();
    assert_eq!(default_name, "echo");
    assert_eq!(default_connector.display_name, "Echo");

    let (explicit_name, explicit_connector) = config.connector_for_thread("explicit").unwrap();
    assert_eq!(explicit_name, "worker");
    assert_eq!(explicit_connector.display_name, "Worker");

    let (fallback_name, _) = config.connector_for_thread("unknown-thread").unwrap();
    assert_eq!(fallback_name, "echo");
}

#[test]
fn rejects_unknown_thread_connector() {
    let mut config = AppConfig::default();
    let thread = ThreadConfig {
        connector: Some("missing".to_string()),
        ..Default::default()
    };
    config.threads.insert("mobile".to_string(), thread);

    assert_config_error(&config, |error| {
        matches!(
            error,
            ConfigError::UnknownThreadConnector(thread, connector)
                if thread == "mobile" && connector == "missing"
        )
    });
}

#[test]
fn rejects_thread_watch_and_queue_when_globally_disabled() {
    let mut watch_disabled = AppConfig::default();
    watch_disabled.features.watch.enabled = false;
    watch_disabled
        .threads
        .get_mut("main")
        .unwrap()
        .watch
        .enabled = true;
    assert_config_error(
        &watch_disabled,
        |error| matches!(error, ConfigError::WatchGloballyDisabled(thread) if thread == "main"),
    );

    let mut queue_disabled = AppConfig::default();
    queue_disabled.features.queue.enabled = false;
    assert_config_error(
        &queue_disabled,
        |error| matches!(error, ConfigError::QueueGloballyDisabled(thread) if thread == "main"),
    );
}

#[test]
fn rejects_missing_telegram_and_llm_required_fields() {
    let mut missing_telegram_token = AppConfig::default();
    missing_telegram_token.mobile.telegram.enabled = true;
    missing_telegram_token.mobile.telegram.bot_token = "   ".to_string();
    assert_config_error(&missing_telegram_token, |error| {
        matches!(error, ConfigError::MissingTelegramToken)
    });

    let mut missing_llm_provider = AppConfig::default();
    missing_llm_provider.llm_router.enabled = true;
    missing_llm_provider.llm_router.provider = "".to_string();
    missing_llm_provider.llm_router.model = "gpt-test".to_string();
    assert_config_error(&missing_llm_provider, |error| {
        matches!(error, ConfigError::MissingLlmProvider)
    });

    let mut missing_llm_model = AppConfig::default();
    missing_llm_model.llm_router.enabled = true;
    missing_llm_model.llm_router.model = "   ".to_string();
    assert_config_error(&missing_llm_model, |error| {
        matches!(error, ConfigError::MissingLlmModel)
    });
}

#[test]
fn parses_yaml_with_defaults_and_expands_home_and_empty_env_values() {
    let yaml = r#"
gateway:
  data_dir: ~/pocket-harness-test-state
connectors:
  default: echo
  definitions:
    echo:
      type: builtin_echo
threads:
  main:
    queue:
      enabled: true
"#;

    let config = parse_and_validate(yaml).unwrap();
    assert_eq!(config.gateway.name, "pocket-harness");
    assert!(config.gateway.hot_reload.enabled);
    assert_eq!(config.thread_or_default("missing").cwd, "~");

    let expected_state_dir = expand_path("~/pocket-harness-test-state");
    assert_eq!(
        config.data_dir(Path::new("/tmp/pocket-harness.yaml")),
        expected_state_dir
    );

    assert_eq!(
        expand_string("$POCKET_HARNESS_CONFIG_BEHAVIOR_UNSET_ENV_VALUE/suffix"),
        "/suffix"
    );
    assert_eq!(
        expand_string("${POCKET_HARNESS_CONFIG_BEHAVIOR_UNSET_ENV_VALUE}/suffix"),
        "/suffix"
    );
}

#[test]
fn default_enabled_feature_keys_match_parent_owned_defaults() {
    let config = AppConfig::default();

    assert_eq!(
        config.enabled_feature_keys(),
        vec![
            "attachments.files",
            "attachments.images",
            "connector.capabilities",
            "connector.health",
            "connector.run",
            "connector.status",
            "jobs.cancel",
            "jobs.history",
            "jobs.queue",
            "mac.screenshot",
            "mac.terminal",
            "threads.cwd",
            "threads.named",
            "threads.watch",
        ]
    );
}
