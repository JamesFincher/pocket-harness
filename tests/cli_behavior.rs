use std::path::Path;
use std::process::{Command, Output};

fn pocket_harness(config_path: &Path, home: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_pocket-harness"))
        .env("HOME", home)
        .arg("--config")
        .arg(config_path)
        .args(args)
        .output()
        .expect("run pocket-harness binary")
}

fn pocket_harness_with_env_file(
    config_path: &Path,
    home: &Path,
    env_file: &Path,
    args: &[&str],
) -> Output {
    Command::new(env!("CARGO_BIN_EXE_pocket-harness"))
        .env("HOME", home)
        .arg("--config")
        .arg(config_path)
        .arg("--env-file")
        .arg(env_file)
        .args(args)
        .output()
        .expect("run pocket-harness binary")
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).to_string()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).to_string()
}

#[test]
fn cli_init_check_and_run_work_against_temp_home() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    let config_path = temp.path().join("pocket-harness.yaml");

    let init = pocket_harness(&config_path, &home, &["init"]);
    assert!(init.status.success(), "stderr={}", stderr(&init));
    assert!(config_path.exists());

    let check = pocket_harness(&config_path, &home, &["check", "--health"]);
    assert!(check.status.success(), "stderr={}", stderr(&check));
    assert!(stdout(&check).contains("config ok"));

    let run = pocket_harness(&config_path, &home, &["run", "--thread", "main", "hello"]);
    assert!(run.status.success(), "stderr={}", stderr(&run));
    assert!(stdout(&run).contains("prompt=hello"));

    assert!(temp.path().join("providers.yaml").exists());
}

#[test]
fn cli_set_updates_yaml_and_preserves_valid_config() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    let config_path = temp.path().join("pocket-harness.yaml");

    assert!(
        pocket_harness(&config_path, &home, &["init"])
            .status
            .success()
    );

    let set = pocket_harness(
        &config_path,
        &home,
        &["set", "threads.main.watch.enabled", "true"],
    );
    assert!(set.status.success(), "stderr={}", stderr(&set));

    let text = std::fs::read_to_string(&config_path).unwrap();
    assert!(text.contains("watch:"));
    assert!(text.contains("enabled: true"));

    let check = pocket_harness(&config_path, &home, &["check", "--health"]);
    assert!(check.status.success(), "stderr={}", stderr(&check));
}

#[test]
fn cli_init_refuses_to_overwrite_without_force() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    let config_path = temp.path().join("pocket-harness.yaml");

    assert!(
        pocket_harness(&config_path, &home, &["init"])
            .status
            .success()
    );
    let second = pocket_harness(&config_path, &home, &["init"]);

    assert!(!second.status.success());
    assert!(stderr(&second).contains("config already exists"));
}

#[test]
fn cli_lists_providers_and_models_from_catalog() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    let config_path = temp.path().join("pocket-harness.yaml");

    assert!(
        pocket_harness(&config_path, &home, &["init"])
            .status
            .success()
    );

    let providers = pocket_harness(&config_path, &home, &["providers"]);
    assert!(providers.status.success(), "stderr={}", stderr(&providers));
    assert!(stdout(&providers).contains("openai - OpenAI"));

    let models = pocket_harness(&config_path, &home, &["models", "openai"]);
    assert!(models.status.success(), "stderr={}", stderr(&models));
    assert!(stdout(&models).contains("gpt-5.5"));
}

#[test]
fn cli_loads_env_file_before_validating_config() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    let config_path = temp.path().join("pocket-harness.yaml");
    let env_file = temp.path().join("env");

    assert!(
        pocket_harness(&config_path, &home, &["init"])
            .status
            .success()
    );

    std::fs::write(
        &env_file,
        "TELEGRAM_BOT_TOKEN=telegram-test\nOPENAI_API_KEY=openai-test\n",
    )
    .unwrap();

    assert!(
        pocket_harness_with_env_file(
            &config_path,
            &home,
            &env_file,
            &["set", "mobile.telegram.enabled", "true"],
        )
        .status
        .success()
    );
    assert!(
        pocket_harness_with_env_file(
            &config_path,
            &home,
            &env_file,
            &["set", "llm_router.model", "gpt-5.5"],
        )
        .status
        .success()
    );
    assert!(
        pocket_harness_with_env_file(
            &config_path,
            &home,
            &env_file,
            &["set", "llm_router.enabled", "true"],
        )
        .status
        .success()
    );

    let check = pocket_harness_with_env_file(&config_path, &home, &env_file, &["check"]);
    assert!(check.status.success(), "stderr={}", stderr(&check));
}

#[test]
fn cli_reset_config_removes_config_catalog_and_env_file() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    let config_path = temp.path().join("pocket-harness.yaml");
    let env_file = temp.path().join("env");

    assert!(
        pocket_harness(&config_path, &home, &["init"])
            .status
            .success()
    );
    std::fs::write(&env_file, "TELEGRAM_BOT_TOKEN=test\n").unwrap();

    let reset = pocket_harness_with_env_file(
        &config_path,
        &home,
        &env_file,
        &["reset", "config", "--yes"],
    );
    assert!(reset.status.success(), "stderr={}", stderr(&reset));

    assert!(!config_path.exists());
    assert!(!temp.path().join("providers.yaml").exists());
    assert!(!env_file.exists());
}
