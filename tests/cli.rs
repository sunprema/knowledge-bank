//! CLI surface tests (PRD §6): folder bootstrap, empty-corpus behavior,
//! exit codes. Nothing here touches the network.

use assert_cmd::Command;
use predicates::prelude::*;

fn kb(root: &std::path::Path) -> Command {
    let mut cmd = Command::cargo_bin("kb").unwrap();
    cmd.arg("--root").arg(root);
    cmd
}

#[test]
fn init_creates_folder_layout() {
    let dir = tempfile::tempdir().unwrap();
    kb(dir.path())
        .arg("init")
        .assert()
        .success()
        .stdout(predicate::str::contains("initialized KB"));
    assert!(dir.path().join(".arxiv-kb/config.toml").exists());

    let config = std::fs::read_to_string(dir.path().join(".arxiv-kb/config.toml")).unwrap();
    assert!(config.contains("text-embedding-3-small"));
    assert!(config.contains("bit_width = 4"));
}

#[test]
fn list_empty_corpus() {
    let dir = tempfile::tempdir().unwrap();
    kb(dir.path())
        .arg("list")
        .assert()
        .success()
        .stdout(predicate::str::contains("no papers"));
}

#[test]
fn stats_and_verify_on_empty_corpus() {
    let dir = tempfile::tempdir().unwrap();
    kb(dir.path()).arg("stats").assert().success();
    kb(dir.path())
        .arg("verify")
        .assert()
        .success()
        .stdout(predicate::str::contains("status: ok"));
}

#[test]
fn status_reports_no_watcher() {
    let dir = tempfile::tempdir().unwrap();
    kb(dir.path())
        .arg("status")
        .assert()
        .success()
        .stdout(predicate::str::contains("watcher:     not running"));
}

#[test]
fn show_unknown_paper_exits_2() {
    let dir = tempfile::tempdir().unwrap();
    kb(dir.path())
        .args(["show", "2504.19874"])
        .assert()
        .code(2)
        .stderr(predicate::str::contains("not in the KB"));
}

#[test]
fn add_garbage_id_exits_1() {
    let dir = tempfile::tempdir().unwrap();
    kb(dir.path())
        .args(["add", "not-an-arxiv-id"])
        .assert()
        .code(1);
}

#[test]
fn v02_commands_say_so() {
    let dir = tempfile::tempdir().unwrap();
    kb(dir.path())
        .args(["serve"])
        .assert()
        .code(1)
        .stderr(predicate::str::contains("planned for v0.2"));
    kb(dir.path())
        .args(["similar", "2504.19874"])
        .assert()
        .code(1)
        .stderr(predicate::str::contains("planned for v0.2"));
}

#[test]
fn search_json_on_empty_corpus_is_empty_response() {
    let dir = tempfile::tempdir().unwrap();
    // No OPENAI_API_KEY needed: an empty index short-circuits before
    // embedding the query.
    kb(dir.path())
        .env_remove("OPENAI_API_KEY")
        .args(["--format", "json", "search", "anything"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"papers\": []"));
}
