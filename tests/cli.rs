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
        .stdout(predicate::str::contains("no documents"));
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
        .stderr(predicate::str::contains("exactly one of"));
}

#[test]
fn add_with_pdf_and_url_exits_1() {
    let dir = tempfile::tempdir().unwrap();
    kb(dir.path())
        .args(["add", "--pdf", "paper.pdf", "--url", "https://example.com"])
        .assert()
        .code(1)
        .stderr(predicate::str::contains("exactly one of"));
}

#[test]
fn add_url_with_invalid_url_exits_1() {
    let dir = tempfile::tempdir().unwrap();
    kb(dir.path())
        .args(["add", "--url", "not-a-url"])
        .assert()
        .code(1)
        .stderr(predicate::str::contains("valid URL"));
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
fn idea_add_writes_canonical_files_even_without_api_key() {
    let dir = tempfile::tempdir().unwrap();
    // Without OPENAI_API_KEY the embed step fails (exit 10), but the
    // canonical files must already be on disk — `kb reindex`/watch can
    // finish the job later (design invariant).
    kb(dir.path())
        .env_remove("OPENAI_API_KEY")
        .args([
            "idea", "add",
            "--project", "kitgig",
            "--title", "x402 anon lane",
            "--body", "Use x402 micropayments for an anonymous per-call lane.",
            "--tags", "payments,x402",
        ])
        .assert()
        .code(10);

    let folder = dir.path().join("x402-anon-lane");
    assert!(folder.join("metadata.json").exists());
    let body = std::fs::read_to_string(folder.join("idea.md")).unwrap();
    assert!(body.contains("anonymous per-call lane"));
    let meta = std::fs::read_to_string(folder.join("metadata.json")).unwrap();
    assert!(meta.contains("\"kind\": \"note\""));
    assert!(meta.contains("\"project\": \"kitgig\""));

    // Visible to list, filterable by kind and project.
    kb(dir.path())
        .args(["list", "--kind", "note", "--project", "kitgig"])
        .assert()
        .success()
        .stdout(predicate::str::contains("x402-anon-lane"))
        .stdout(predicate::str::contains("(idea: kitgig)"));
    kb(dir.path())
        .args(["list", "--kind", "paper"])
        .assert()
        .success()
        .stdout(predicate::str::contains("no documents"));

    // show renders the body and project.
    kb(dir.path())
        .args(["show", "x402-anon-lane"])
        .assert()
        .success()
        .stdout(predicate::str::contains("project:    kitgig"))
        .stdout(predicate::str::contains("anonymous per-call lane"));

    // Re-capture with the same title = upsert: no duplicate folder, body
    // replaced, tags kept (none supplied the second time).
    kb(dir.path())
        .env_remove("OPENAI_API_KEY")
        .args([
            "idea", "add",
            "--project", "kitgig",
            "--title", "x402 anon lane",
            "--body", "Refined: settle anonymously via x402 escrow.",
        ])
        .assert()
        .code(10);
    let body = std::fs::read_to_string(folder.join("idea.md")).unwrap();
    assert!(body.contains("Refined"));
    assert!(!body.contains("per-call lane"), "body replaced, not appended");
    let meta = std::fs::read_to_string(folder.join("metadata.json")).unwrap();
    assert!(meta.contains("\"payments\""), "tags survive an upsert");

    // update on an idea points at re-capture.
    kb(dir.path())
        .args(["update", "x402-anon-lane"])
        .assert()
        .code(1)
        .stderr(predicate::str::contains("kb idea add"));
}

#[test]
fn idea_add_empty_body_exits_1() {
    let dir = tempfile::tempdir().unwrap();
    kb(dir.path())
        .args(["idea", "add", "--project", "p", "--title", "t", "--body", "  "])
        .assert()
        .code(1)
        .stderr(predicate::str::contains("body must not be empty"));
    assert!(!dir.path().join("t").exists());
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
