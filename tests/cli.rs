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
fn add_with_both_id_and_pdf_exits_1() {
    let dir = tempfile::tempdir().unwrap();
    kb(dir.path())
        .args(["add", "2504.19874", "--pdf", "paper.pdf"])
        .assert()
        .code(1)
        .stderr(predicate::str::contains("not both"));
}

#[test]
fn add_with_no_input_exits_1() {
    let dir = tempfile::tempdir().unwrap();
    kb(dir.path())
        .arg("add")
        .assert()
        .code(1)
        .stderr(predicate::str::contains("usage: kb add"));
}

#[test]
fn add_pdf_missing_file_exits_2() {
    let dir = tempfile::tempdir().unwrap();
    let missing = dir.path().join("nope.pdf");
    kb(dir.path())
        .args(["add", "--pdf", missing.to_str().unwrap()])
        .assert()
        .code(2)
        .stderr(predicate::str::contains("cannot read"));
    // A bad source must not leave a half-created paper folder behind.
    assert!(!dir.path().join("nope").exists());
}

#[test]
fn add_pdf_non_pdf_payload_exits_4() {
    let dir = tempfile::tempdir().unwrap();
    let fake = dir.path().join("Notes On Things.pdf");
    std::fs::write(&fake, "hello, not a pdf").unwrap();
    kb(dir.path())
        .args(["add", "--pdf", fake.to_str().unwrap()])
        .assert()
        .code(4)
        .stderr(predicate::str::contains("is not a PDF"));
    assert!(!dir.path().join("notes-on-things").exists());
}

#[test]
fn update_local_pdf_paper_exits_1_with_hint() {
    let dir = tempfile::tempdir().unwrap();
    // Fake an already-ingested local-PDF paper: folder + metadata.json.
    let paper = dir.path().join("my-paper");
    std::fs::create_dir_all(&paper).unwrap();
    std::fs::write(
        paper.join("metadata.json"),
        r#"{
            "arxiv_id": "my-paper",
            "title": "My Paper",
            "authors": [],
            "abstract": "",
            "categories": [],
            "published_at": "",
            "updated_at": "",
            "ingested_at": "2026-06-12T00:00:00Z",
            "source_format": "pdf",
            "tags": [],
            "schema_version": 1
        }"#,
    )
    .unwrap();
    kb(dir.path())
        .args(["update", "my-paper"])
        .assert()
        .code(1)
        .stderr(predicate::str::contains("kb add --pdf"));
    // Slug ids work for read commands too.
    kb(dir.path())
        .args(["show", "my-paper"])
        .assert()
        .success()
        .stdout(predicate::str::contains("My Paper"));
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
