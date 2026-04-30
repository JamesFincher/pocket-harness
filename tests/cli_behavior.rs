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
