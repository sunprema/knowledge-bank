//! LaTeX e-print download, main-tex detection, pandoc invocation
//! (PRD §4 steps 2-3).

use crate::KbError;
use std::io::Read;
use std::path::{Component, Path, PathBuf};

/// All e-print URL construction lives here.
pub(crate) fn eprint_url(arxiv_id: &str) -> String {
    format!("https://arxiv.org/e-print/{arxiv_id}")
}

/// What the e-print archive turned out to contain.
#[derive(Debug, PartialEq)]
pub enum EprintContent {
    /// LaTeX sources were extracted into `source/`; `main_tex` is the
    /// detected entry point (relative to the source dir).
    Latex { main_tex: PathBuf },
    /// The e-print was a bare PDF (no `.tex`) — caller takes the PDF
    /// fallback path and sets `source_format: "pdf"`.
    PdfOnly,
}

/// Download `https://arxiv.org/e-print/{id}` and extract into `source_dir`.
///
/// The payload may be: a `.tar.gz` of a source tree, a bare `.gz` of a
/// single `.tex`, or a raw PDF. Detect by content (magic bytes), not by
/// headers. Reject archive members that would escape `source_dir`
/// (path traversal) — skip them with a warning.
pub async fn download_and_extract_source(
    client: &reqwest::Client,
    arxiv_id: &str,
    source_dir: &Path,
) -> Result<EprintContent, KbError> {
    let url = eprint_url(arxiv_id);
    let resp = client.get(&url).send().await?;
    let status = resp.status();
    if !status.is_success() {
        return Err(KbError::Network(format!(
            "e-print download for {arxiv_id} returned HTTP {status}"
        )));
    }
    let data = resp.bytes().await.map_err(KbError::from)?;
    extract_eprint_bytes(&data, source_dir)
}

const GZIP_MAGIC: [u8; 2] = [0x1f, 0x8b];

fn is_pdf(data: &[u8]) -> bool {
    data.starts_with(b"%PDF")
}

/// POSIX/GNU tar puts "ustar" at offset 257.
fn is_tar(data: &[u8]) -> bool {
    data.len() > 262 && &data[257..262] == b"ustar"
}

/// Content-sniff and extract an e-print payload (the testable core of
/// [`download_and_extract_source`]).
pub(crate) fn extract_eprint_bytes(
    data: &[u8],
    source_dir: &Path,
) -> Result<EprintContent, KbError> {
    if is_pdf(data) {
        return Ok(EprintContent::PdfOnly);
    }

    let decompressed: Vec<u8> = if data.starts_with(&GZIP_MAGIC) {
        let mut out = Vec::new();
        flate2::read::GzDecoder::new(data)
            .read_to_end(&mut out)
            .map_err(|e| KbError::Extraction(format!("e-print gunzip failed: {e}")))?;
        out
    } else {
        data.to_vec()
    };

    if is_pdf(&decompressed) {
        return Ok(EprintContent::PdfOnly);
    }

    std::fs::create_dir_all(source_dir)
        .map_err(|e| KbError::Index(format!("create {}: {e}", source_dir.display())))?;

    if is_tar(&decompressed) {
        extract_tar(&decompressed, source_dir)?;
    } else {
        // A bare .gz of a single .tex (or a raw .tex — be tolerant).
        std::fs::write(source_dir.join("main.tex"), &decompressed)
            .map_err(|e| KbError::Index(format!("write main.tex: {e}")))?;
    }

    match find_main_tex(source_dir) {
        Ok(main_tex) => Ok(EprintContent::Latex { main_tex }),
        // Extracted fine but no usable .tex => the PDF fallback path.
        Err(_) => Ok(EprintContent::PdfOnly),
    }
}

/// Unpack a tar stream into `source_dir`, skipping members that would
/// escape it (path traversal) and non-file/non-dir members (symlinks are
/// a traversal vector too).
fn extract_tar(data: &[u8], source_dir: &Path) -> Result<(), KbError> {
    let mut archive = tar::Archive::new(std::io::Cursor::new(data));
    let entries = archive
        .entries()
        .map_err(|e| KbError::Extraction(format!("e-print tar unreadable: {e}")))?;
    for entry in entries {
        let mut entry =
            entry.map_err(|e| KbError::Extraction(format!("e-print tar entry: {e}")))?;
        let rel: PathBuf = match entry.path() {
            Ok(p) => p.into_owned(),
            Err(e) => {
                tracing::warn!("skipping tar member with undecodable path: {e}");
                continue;
            }
        };
        if !rel
            .components()
            .all(|c| matches!(c, Component::Normal(_) | Component::CurDir))
        {
            tracing::warn!(
                "skipping tar member {} (would escape source dir)",
                rel.display()
            );
            continue;
        }
        let entry_type = entry.header().entry_type();
        let dest = source_dir.join(&rel);
        if entry_type.is_dir() {
            std::fs::create_dir_all(&dest)
                .map_err(|e| KbError::Index(format!("create {}: {e}", dest.display())))?;
            continue;
        }
        if !entry_type.is_file() {
            tracing::warn!(
                "skipping non-regular tar member {} ({entry_type:?})",
                rel.display()
            );
            continue;
        }
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| KbError::Index(format!("create {}: {e}", parent.display())))?;
        }
        let mut file = std::fs::File::create(&dest)
            .map_err(|e| KbError::Index(format!("create {}: {e}", dest.display())))?;
        std::io::copy(&mut entry, &mut file)
            .map_err(|e| KbError::Index(format!("write {}: {e}", dest.display())))?;
    }
    Ok(())
}

/// Find the main `.tex` file (PRD §4 step 2 heuristics):
/// 1. candidates = files containing `\documentclass`
/// 2. prefer `main.tex`, `paper.tex`, `manuscript.tex`, then one with
///    `\begin{document}`
/// 3. still ambiguous ⇒ largest candidate
///
/// No candidates ⇒ `Extraction` error. Returns a path relative to
/// `source_dir`.
pub fn find_main_tex(source_dir: &Path) -> Result<PathBuf, KbError> {
    let mut tex_files = Vec::new();
    collect_tex_files(source_dir, &mut tex_files)?;

    struct Candidate {
        rel: PathBuf,
        size: u64,
        has_begin_document: bool,
    }
    let mut candidates = Vec::new();
    for path in tex_files {
        // Lossy read: arXiv sources are frequently Latin-1.
        let Ok(bytes) = std::fs::read(&path) else {
            continue;
        };
        let content = String::from_utf8_lossy(&bytes);
        if !content.contains("\\documentclass") {
            continue;
        }
        let rel = path
            .strip_prefix(source_dir)
            .unwrap_or(&path)
            .to_path_buf();
        candidates.push(Candidate {
            rel,
            size: bytes.len() as u64,
            has_begin_document: content.contains("\\begin{document}"),
        });
    }

    if candidates.is_empty() {
        return Err(KbError::Extraction(format!(
            "no .tex file with \\documentclass in {}",
            source_dir.display()
        )));
    }
    if candidates.len() == 1 {
        return Ok(candidates.remove(0).rel);
    }

    for preferred in ["main.tex", "paper.tex", "manuscript.tex"] {
        if let Some(c) = candidates
            .iter()
            .find(|c| c.rel.file_name().is_some_and(|n| n == preferred))
        {
            return Ok(c.rel.clone());
        }
    }

    let pool: Vec<&Candidate> = {
        let with_begin: Vec<&Candidate> =
            candidates.iter().filter(|c| c.has_begin_document).collect();
        if with_begin.is_empty() {
            candidates.iter().collect()
        } else {
            with_begin
        }
    };
    if pool.len() == 1 {
        return Ok(pool[0].rel.clone());
    }
    Ok(pool
        .iter()
        .max_by_key(|c| c.size)
        .expect("pool is non-empty")
        .rel
        .clone())
}

fn collect_tex_files(dir: &Path, out: &mut Vec<PathBuf>) -> Result<(), KbError> {
    let entries = std::fs::read_dir(dir)
        .map_err(|e| KbError::Extraction(format!("read {}: {e}", dir.display())))?;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_tex_files(&path, out)?;
        } else if path.extension().is_some_and(|e| e.eq_ignore_ascii_case("tex")) {
            out.push(path);
        }
    }
    Ok(())
}

/// Verify pandoc is runnable (`pandoc --version`). Missing ⇒ `Config`
/// error (exit 10) with install instructions (https://pandoc.org/installing.html).
pub fn check_pandoc(pandoc_path: &str) -> Result<(), KbError> {
    let missing = || {
        KbError::Config(format!(
            "pandoc not found (looked for `{pandoc_path}`). arxiv-kb needs pandoc to \
             convert LaTeX sources; install it from https://pandoc.org/installing.html \
             or set [ingest].pandoc_path in config.toml"
        ))
    };
    let output = std::process::Command::new(pandoc_path)
        .arg("--version")
        .output()
        .map_err(|_| missing())?;
    if !output.status.success() {
        return Err(missing());
    }
    Ok(())
}

/// Run `pandoc {main_tex} -o {out_md} --from latex --to gfm --wrap=none`
/// (plus `--bibliography` if a `.bib` exists next to the source).
/// Run with cwd = the main tex's directory so relative `\input`s resolve.
/// Non-zero exit ⇒ `Extraction` error carrying pandoc's stderr (caller
/// decides to fall back to PDF extraction).
pub async fn run_pandoc(
    pandoc_path: &str,
    main_tex: &Path,
    out_md: &Path,
) -> Result<(), KbError> {
    // cwd changes below, so both paths must be absolute.
    let main_abs = std::path::absolute(main_tex)
        .map_err(|e| KbError::Extraction(format!("resolve {}: {e}", main_tex.display())))?;
    let out_abs = std::path::absolute(out_md)
        .map_err(|e| KbError::Extraction(format!("resolve {}: {e}", out_md.display())))?;
    let work_dir = main_abs
        .parent()
        .ok_or_else(|| KbError::Extraction(format!("{} has no parent dir", main_abs.display())))?
        .to_path_buf();

    let mut cmd = tokio::process::Command::new(pandoc_path);
    cmd.arg(&main_abs)
        .arg("-o")
        .arg(&out_abs)
        .args(["--from", "latex", "--to", "gfm", "--wrap=none"])
        .current_dir(&work_dir);
    for bib in bib_files(&work_dir) {
        let mut arg = std::ffi::OsString::from("--bibliography=");
        arg.push(&bib);
        cmd.arg(arg);
    }

    let output = cmd.output().await.map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            KbError::Config(format!(
                "pandoc not found (looked for `{pandoc_path}`); \
                 install it from https://pandoc.org/installing.html"
            ))
        } else {
            KbError::Extraction(format!("failed to run pandoc: {e}"))
        }
    })?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(KbError::Extraction(format!(
            "pandoc exited with {}: {}",
            output.status,
            stderr.trim()
        )));
    }
    Ok(())
}

/// `.bib` files sitting next to the main tex (PRD §4 step 3).
fn bib_files(dir: &Path) -> Vec<PathBuf> {
    let mut bibs: Vec<PathBuf> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_file() && p.extension().is_some_and(|e| e.eq_ignore_ascii_case("bib")))
        .collect();
    bibs.sort();
    bibs
}

#[cfg(test)]
mod tests {
    use super::*;
    use flate2::Compression;
    use flate2::write::GzEncoder;
    use std::io::Write;

    fn gz(data: &[u8]) -> Vec<u8> {
        let mut enc = GzEncoder::new(Vec::new(), Compression::default());
        enc.write_all(data).unwrap();
        enc.finish().unwrap()
    }

    /// Build a tar from (path, contents) pairs. Paths with `..` need a
    /// hand-rolled header because `Header::set_path` normalizes them away.
    fn build_tar(members: &[(&str, &[u8])]) -> Vec<u8> {
        let mut builder = tar::Builder::new(Vec::new());
        for (path, contents) in members {
            let mut header = tar::Header::new_gnu();
            header.set_size(contents.len() as u64);
            header.set_mode(0o644);
            header.set_entry_type(tar::EntryType::Regular);
            if path.contains("..") {
                // Write the raw name bytes, bypassing set_path's sanitizing.
                let gnu = header.as_gnu_mut().unwrap();
                gnu.name[..path.len()].copy_from_slice(path.as_bytes());
                header.set_cksum();
                builder.append(&header, *contents).unwrap();
            } else {
                builder
                    .append_data(&mut header, path, *contents)
                    .unwrap();
            }
        }
        builder.into_inner().unwrap()
    }

    const MAIN_TEX: &[u8] =
        b"\\documentclass{article}\n\\begin{document}\nHello.\n\\end{document}\n";

    // -------- extract_eprint_bytes --------

    #[test]
    fn extracts_targz_source_tree() {
        let tmp = tempfile::tempdir().unwrap();
        let source_dir = tmp.path().join("source");
        let tar = build_tar(&[
            ("paper.tex", MAIN_TEX),
            ("sections/intro.tex", b"\\section{Introduction}\n"),
            ("refs.bib", b"@article{x, title={X}}\n"),
        ]);
        let content = extract_eprint_bytes(&gz(&tar), &source_dir).unwrap();
        assert_eq!(
            content,
            EprintContent::Latex { main_tex: PathBuf::from("paper.tex") }
        );
        assert!(source_dir.join("paper.tex").exists());
        assert!(source_dir.join("sections/intro.tex").exists());
        assert!(source_dir.join("refs.bib").exists());
    }

    #[test]
    fn path_traversal_member_is_skipped() {
        let tmp = tempfile::tempdir().unwrap();
        let source_dir = tmp.path().join("source");
        let tar = build_tar(&[
            ("main.tex", MAIN_TEX),
            ("../evil.tex", b"\\documentclass{article} gotcha"),
        ]);
        let content = extract_eprint_bytes(&gz(&tar), &source_dir).unwrap();
        assert_eq!(
            content,
            EprintContent::Latex { main_tex: PathBuf::from("main.tex") }
        );
        // The traversal member must exist neither outside nor inside.
        assert!(!tmp.path().join("evil.tex").exists());
        assert!(!source_dir.join("evil.tex").exists());
        assert!(!source_dir.join("../evil.tex").exists());
    }

    #[test]
    fn bare_gz_of_single_tex() {
        let tmp = tempfile::tempdir().unwrap();
        let source_dir = tmp.path().join("source");
        let content = extract_eprint_bytes(&gz(MAIN_TEX), &source_dir).unwrap();
        assert_eq!(
            content,
            EprintContent::Latex { main_tex: PathBuf::from("main.tex") }
        );
        assert_eq!(std::fs::read(source_dir.join("main.tex")).unwrap(), MAIN_TEX);
    }

    #[test]
    fn raw_pdf_payload_is_pdf_only() {
        let tmp = tempfile::tempdir().unwrap();
        let source_dir = tmp.path().join("source");
        let content = extract_eprint_bytes(b"%PDF-1.5 fake pdf bytes", &source_dir).unwrap();
        assert_eq!(content, EprintContent::PdfOnly);
        assert!(!source_dir.exists(), "PDF payload must not create source/");
    }

    #[test]
    fn gzipped_pdf_payload_is_pdf_only() {
        let tmp = tempfile::tempdir().unwrap();
        let source_dir = tmp.path().join("source");
        let content =
            extract_eprint_bytes(&gz(b"%PDF-1.5 fake pdf bytes"), &source_dir).unwrap();
        assert_eq!(content, EprintContent::PdfOnly);
    }

    #[test]
    fn targz_without_documentclass_is_pdf_only() {
        let tmp = tempfile::tempdir().unwrap();
        let source_dir = tmp.path().join("source");
        let tar = build_tar(&[("readme.txt", b"no tex here".as_slice())]);
        let content = extract_eprint_bytes(&gz(&tar), &source_dir).unwrap();
        assert_eq!(content, EprintContent::PdfOnly);
    }

    #[test]
    fn corrupt_gzip_is_extraction_error() {
        let tmp = tempfile::tempdir().unwrap();
        let mut data = gz(MAIN_TEX);
        data.truncate(6); // valid magic, truncated stream
        let err = extract_eprint_bytes(&data, &tmp.path().join("source")).unwrap_err();
        assert!(matches!(err, KbError::Extraction(_)), "got {err:?}");
    }

    // -------- find_main_tex --------

    fn write(dir: &Path, name: &str, contents: &str) {
        let path = dir.join(name);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, contents).unwrap();
    }

    #[test]
    fn prefers_conventional_names() {
        let tmp = tempfile::tempdir().unwrap();
        write(tmp.path(), "zz.tex", "\\documentclass{article}\n\\begin{document}\nbig body text here\n\\end{document}");
        write(tmp.path(), "main.tex", "\\documentclass{article}");
        assert_eq!(find_main_tex(tmp.path()).unwrap(), PathBuf::from("main.tex"));
    }

    #[test]
    fn prefers_begin_document_when_no_conventional_name() {
        let tmp = tempfile::tempdir().unwrap();
        write(tmp.path(), "macros.tex", "\\documentclass{article} % template stub");
        write(tmp.path(), "body.tex", "\\documentclass{article}\n\\begin{document}\nx\\end{document}");
        assert_eq!(find_main_tex(tmp.path()).unwrap(), PathBuf::from("body.tex"));
    }

    #[test]
    fn falls_back_to_largest_candidate() {
        let tmp = tempfile::tempdir().unwrap();
        write(tmp.path(), "a.tex", "\\documentclass{article}\n\\begin{document}\nshort\\end{document}");
        write(
            tmp.path(),
            "b.tex",
            &format!(
                "\\documentclass{{article}}\n\\begin{{document}}\n{}\\end{{document}}",
                "long ".repeat(100)
            ),
        );
        assert_eq!(find_main_tex(tmp.path()).unwrap(), PathBuf::from("b.tex"));
    }

    #[test]
    fn finds_main_in_subdirectory() {
        let tmp = tempfile::tempdir().unwrap();
        write(tmp.path(), "nested/paper.tex", "\\documentclass{article}\n\\begin{document}\nx\\end{document}");
        assert_eq!(
            find_main_tex(tmp.path()).unwrap(),
            PathBuf::from("nested/paper.tex")
        );
    }

    #[test]
    fn no_documentclass_is_extraction_error() {
        let tmp = tempfile::tempdir().unwrap();
        write(tmp.path(), "notes.tex", "just some text, no preamble");
        let err = find_main_tex(tmp.path()).unwrap_err();
        assert!(matches!(err, KbError::Extraction(_)), "got {err:?}");
    }

    // -------- pandoc --------

    #[test]
    fn check_pandoc_bogus_path_is_config_error() {
        let err = check_pandoc("/nonexistent/definitely-not-pandoc").unwrap_err();
        assert!(matches!(err, KbError::Config(_)), "got {err:?}");
        assert!(err.to_string().contains("pandoc.org/installing"));
    }

    #[tokio::test]
    async fn run_pandoc_missing_binary_is_config_error() {
        let tmp = tempfile::tempdir().unwrap();
        write(tmp.path(), "main.tex", "\\documentclass{article}\\begin{document}x\\end{document}");
        let err = run_pandoc(
            "/nonexistent/definitely-not-pandoc",
            &tmp.path().join("main.tex"),
            &tmp.path().join("out.md"),
        )
        .await
        .unwrap_err();
        assert!(matches!(err, KbError::Config(_)), "got {err:?}");
    }

    /// Only runs when a real pandoc is installed (not the case in CI/dev
    /// boxes without it — check_pandoc gates it).
    #[tokio::test]
    async fn run_pandoc_real_conversion_if_available() {
        if check_pandoc("pandoc").is_err() {
            eprintln!("pandoc not installed; skipping real-conversion test");
            return;
        }
        let tmp = tempfile::tempdir().unwrap();
        write(
            tmp.path(),
            "main.tex",
            "\\documentclass{article}\n\\begin{document}\n\\section{Introduction}\nHello pandoc.\n\\end{document}\n",
        );
        let out = tmp.path().join("sections.md");
        run_pandoc("pandoc", &tmp.path().join("main.tex"), &out)
            .await
            .unwrap();
        let md = std::fs::read_to_string(&out).unwrap();
        assert!(md.contains("Introduction"));
        assert!(md.contains("Hello pandoc."));
    }

    #[test]
    fn eprint_url_shape() {
        assert_eq!(eprint_url("2504.19874"), "https://arxiv.org/e-print/2504.19874");
    }
}
