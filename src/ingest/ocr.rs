//! OCR fallback for image-only PDF pages, via the `kb-ocr` sidecar.
//!
//! `pdf_extract` (see [`super::pdf::extract_text_per_page`]) only recovers a
//! page's *text layer*. Scanned papers and figure-only pages have none, so the
//! fallback `sections.md` would be blank for them. When such pages appear and
//! the macOS `kb-ocr` helper is reachable, we render+recognize just those pages
//! and splice the recovered text back in.
//!
//! This is a *best-effort enhancement*, not a hard dependency: the helper only
//! ships in the macOS app bundle (it's built on Vision + PDFKit), so on Linux —
//! or any build without it — [`locate_helper`] returns `None` and the caller
//! keeps the blank pages. That preserves the cross-platform single-binary
//! promise: no new Rust dependency, no link against Apple frameworks.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde::Deserialize;

use crate::KbError;

/// What the sidecar emits per page (1-indexed) on stdout.
#[derive(Debug, Deserialize)]
struct PageText {
    page: usize,
    text: String,
}

/// Find the `kb-ocr` sidecar, or `None` if OCR isn't available here.
///
/// Resolution order:
/// 1. `KB_OCR_BIN` — explicit override (tests, custom installs).
/// 2. A `kb-ocr` sibling of the running executable. The macOS build places it
///    next to the `kb` engine in both the app bundle's `Resources/` and the
///    dev `target/{release,debug}/` dir, so this covers shipped and dev runs.
pub fn locate_helper() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("KB_OCR_BIN") {
        let p = PathBuf::from(p);
        if is_executable(&p) {
            return Some(p);
        }
    }
    let sibling = std::env::current_exe().ok()?.parent()?.join("kb-ocr");
    is_executable(&sibling).then_some(sibling)
}

fn is_executable(p: &Path) -> bool {
    // Existence is enough as a portability floor; the spawn below surfaces a
    // genuine permission problem as a (non-fatal) `Extraction` error.
    p.is_file()
}

/// OCR the given 1-indexed `pages` of `pdf_path` with `helper`. Returns a map
/// from page number to recovered text (only pages the helper reported). A
/// helper crash, non-zero exit, or unparseable output is an `Extraction`
/// error — callers treat OCR failure as "leave the page blank", not fatal.
pub fn run(helper: &Path, pdf_path: &Path, pages: &[usize]) -> Result<HashMap<usize, String>, KbError> {
    let pages_arg = pages
        .iter()
        .map(|p| p.to_string())
        .collect::<Vec<_>>()
        .join(",");

    let output = Command::new(helper)
        .arg(pdf_path)
        .arg("--pages")
        .arg(&pages_arg)
        .output()
        .map_err(|e| KbError::Extraction(format!("could not run kb-ocr: {e}")))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(KbError::Extraction(format!(
            "kb-ocr exited with {}: {}",
            output.status,
            stderr.trim()
        )));
    }

    let parsed: Vec<PageText> = serde_json::from_slice(&output.stdout)
        .map_err(|e| KbError::Extraction(format!("kb-ocr produced invalid JSON: {e}")))?;

    Ok(parsed
        .into_iter()
        .map(|pt| (pt.page, pt.text))
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::os::unix::fs::PermissionsExt;

    /// Write an executable stub that emits the given stdout (and exit code),
    /// standing in for the real Vision-based sidecar so this test is hermetic.
    fn stub_helper(dir: &Path, stdout: &str, code: i32) -> PathBuf {
        let path = dir.join("kb-ocr-stub");
        let mut f = std::fs::File::create(&path).unwrap();
        writeln!(f, "#!/bin/sh\ncat <<'EOF'\n{stdout}\nEOF\nexit {code}").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    #[test]
    fn parses_helper_json_into_page_map() {
        let tmp = tempfile::tempdir().unwrap();
        let helper = stub_helper(
            tmp.path(),
            r#"[{"page":1,"text":"hello"},{"page":3,"text":"world"}]"#,
            0,
        );
        let map = run(&helper, Path::new("/nonexistent.pdf"), &[1, 3]).unwrap();
        assert_eq!(map.get(&1).map(String::as_str), Some("hello"));
        assert_eq!(map.get(&3).map(String::as_str), Some("world"));
        assert_eq!(map.len(), 2);
    }

    #[test]
    fn nonzero_exit_is_extraction_error() {
        let tmp = tempfile::tempdir().unwrap();
        let helper = stub_helper(tmp.path(), "boom", 1);
        let err = run(&helper, Path::new("/x.pdf"), &[1]).unwrap_err();
        assert!(matches!(err, KbError::Extraction(_)), "got {err:?}");
    }

    #[test]
    fn locate_helper_honors_env_override() {
        let tmp = tempfile::tempdir().unwrap();
        let helper = stub_helper(tmp.path(), "[]", 0);
        // SAFETY: single-threaded test; restored before returning.
        unsafe { std::env::set_var("KB_OCR_BIN", &helper) };
        let found = locate_helper();
        unsafe { std::env::remove_var("KB_OCR_BIN") };
        assert_eq!(found.as_deref(), Some(helper.as_path()));
    }
}
