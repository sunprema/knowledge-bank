//! PDF download, outline (TOC) extraction, and fallback text extraction
//! (PRD §4 steps 4-5). Pure-Rust via `lopdf`/`pdf-extract` — keeps the
//! single-static-binary promise (no libpdfium dylib).
//!
//! Note on lopdf: we walk the outline tree ourselves instead of using
//! `Document::get_toc()`, because `get_toc` keys entries by title (an
//! `IndexMap`) — duplicate section titles collapse — and it discards the
//! named-destination *name*, which we need for `#nameddest=` deep links.

use crate::{KbError, TocEntry};
use std::collections::{HashMap, HashSet};
use std::path::Path;

use lopdf::{Dictionary, Document, Object, ObjectId};

/// All PDF URL construction lives here.
pub(crate) fn pdf_url(arxiv_id: &str) -> String {
    format!("https://arxiv.org/pdf/{arxiv_id}")
}

/// Download `https://arxiv.org/pdf/{id}` to `dest` (atomic: temp file then
/// rename). Non-200 ⇒ `Network`; a payload that isn't `%PDF` ⇒ `Extraction`.
pub async fn download_pdf(
    client: &reqwest::Client,
    arxiv_id: &str,
    dest: &Path,
) -> Result<(), KbError> {
    let url = pdf_url(arxiv_id);
    let resp = client.get(&url).send().await?;
    let status = resp.status();
    if !status.is_success() {
        return Err(KbError::Network(format!(
            "PDF download for {arxiv_id} returned HTTP {status}"
        )));
    }
    let bytes = resp.bytes().await.map_err(KbError::from)?;
    if !bytes.starts_with(b"%PDF") {
        return Err(KbError::Extraction(format!(
            "arXiv returned a non-PDF payload for {arxiv_id}"
        )));
    }

    let dir = dest.parent().filter(|p| !p.as_os_str().is_empty());
    let mut tmp = match dir {
        Some(d) => tempfile::NamedTempFile::new_in(d),
        None => tempfile::NamedTempFile::new(),
    }
    .map_err(|e| KbError::Index(format!("create temp file for paper.pdf: {e}")))?;
    std::io::Write::write_all(&mut tmp, &bytes)
        .map_err(|e| KbError::Index(format!("write paper.pdf: {e}")))?;
    tmp.persist(dest)
        .map_err(|e| KbError::Index(format!("rename into {}: {e}", dest.display())))?;
    Ok(())
}

/// Extract the PDF outline as a flat list (depth-first order), with
/// 1-indexed page numbers and named destinations when present.
/// A PDF with no outline returns an empty Vec (PRD §14: page numbers only).
/// A malformed PDF ⇒ `Extraction` error.
pub fn extract_toc(pdf_path: &Path) -> Result<Vec<TocEntry>, KbError> {
    let doc = Document::load(pdf_path).map_err(|e| {
        KbError::Extraction(format!("cannot parse {}: {e}", pdf_path.display()))
    })?;

    // ObjectId → 1-indexed page number.
    let page_numbers: HashMap<ObjectId, u32> = doc
        .get_pages()
        .into_iter()
        .map(|(num, id)| (id, num))
        .collect();
    let named = collect_named_destinations(&doc);

    let catalog = doc
        .catalog()
        .map_err(|e| KbError::Extraction(format!("{}: no catalog: {e}", pdf_path.display())))?;
    let Ok(outlines_obj) = catalog.get(b"Outlines") else {
        return Ok(Vec::new());
    };
    let Some(outlines) = resolve_dict(&doc, outlines_obj) else {
        return Ok(Vec::new());
    };
    let Ok(first) = outlines.get(b"First") else {
        return Ok(Vec::new());
    };

    let mut out = Vec::new();
    let mut visited = HashSet::new();
    walk_outline(&doc, first.clone(), &page_numbers, &named, &mut out, &mut visited, 0);
    Ok(out)
}

/// Walk one sibling chain (an outline node and its `Next` successors),
/// recursing into `First` children depth-first: a node's entry precedes
/// its children's. Malformed individual nodes are skipped, not fatal.
fn walk_outline(
    doc: &Document,
    start: Object,
    page_numbers: &HashMap<ObjectId, u32>,
    named: &HashMap<Vec<u8>, Vec<Object>>,
    out: &mut Vec<TocEntry>,
    visited: &mut HashSet<ObjectId>,
    depth: usize,
) {
    if depth > 64 {
        return; // pathological nesting
    }
    let mut current = start;
    loop {
        if let Ok(id) = current.as_reference()
            && !visited.insert(id)
        {
            return; // cycle guard
        }
        let Some(node) = resolve_dict(doc, &current) else {
            return;
        };
        if let Some(entry) = toc_entry_for_node(doc, &node, page_numbers, named) {
            out.push(entry);
        }
        if let Ok(first) = node.get(b"First") {
            walk_outline(doc, first.clone(), page_numbers, named, out, visited, depth + 1);
        }
        match node.get(b"Next") {
            Ok(next) => current = next.clone(),
            Err(_) => return,
        }
    }
}

fn toc_entry_for_node(
    doc: &Document,
    node: &Dictionary,
    page_numbers: &HashMap<ObjectId, u32>,
    named: &HashMap<Vec<u8>, Vec<Object>>,
) -> Option<TocEntry> {
    let title_obj = resolve_object(doc, node.get(b"Title").ok()?)?;
    let title = lopdf::decode_text_string(&title_obj)
        .ok()
        .or_else(|| {
            title_obj
                .as_str()
                .ok()
                .map(|b| String::from_utf8_lossy(b).into_owned())
        })?
        .trim()
        .to_string();
    if title.is_empty() {
        return None;
    }

    // Destination: /Dest directly, or a /A GoTo action's /D.
    let dest_obj = match node.get(b"Dest") {
        Ok(d) => d.clone(),
        Err(_) => {
            let action = resolve_dict(doc, node.get(b"A").ok()?)?;
            if let Ok(s) = action.get(b"S").and_then(Object::as_name)
                && s != b"GoTo"
            {
                return None; // URI/launch/etc. actions have no page
            }
            action.get(b"D").ok()?.clone()
        }
    };
    let (page, named_dest) = resolve_destination(doc, &dest_obj, page_numbers, named, 0)?;
    Some(TocEntry { title, page, named_dest })
}

/// A destination is an explicit array `[page /XYZ …]`, a name (string or
/// name object) pointing into the named-destination table, or a dictionary
/// wrapping one under `/D`.
fn resolve_destination(
    doc: &Document,
    dest: &Object,
    page_numbers: &HashMap<ObjectId, u32>,
    named: &HashMap<Vec<u8>, Vec<Object>>,
    depth: usize,
) -> Option<(u32, Option<String>)> {
    if depth > 8 {
        return None;
    }
    match dest {
        Object::Reference(id) => resolve_destination(
            doc,
            doc.get_object(*id).ok()?,
            page_numbers,
            named,
            depth + 1,
        ),
        Object::Array(arr) => {
            page_from_dest_array(doc, arr, page_numbers).map(|p| (p, None))
        }
        Object::String(name, _) | Object::Name(name) => {
            let arr = named.get(name)?;
            let page = page_from_dest_array(doc, arr, page_numbers)?;
            Some((page, Some(String::from_utf8_lossy(name).into_owned())))
        }
        Object::Dictionary(d) => resolve_destination(
            doc,
            d.get(b"D").ok()?,
            page_numbers,
            named,
            depth + 1,
        ),
        _ => None,
    }
}

/// First element of a destination array: a page reference, or (in some
/// producers) a 0-based page index integer.
fn page_from_dest_array(
    doc: &Document,
    arr: &[Object],
    page_numbers: &HashMap<ObjectId, u32>,
) -> Option<u32> {
    match arr.first()? {
        Object::Reference(id) => page_numbers
            .get(id)
            .copied()
            // Some PDFs reference an intermediate object; one more hop.
            .or_else(|| {
                let inner = doc.get_object(*id).ok()?.as_reference().ok()?;
                page_numbers.get(&inner).copied()
            }),
        Object::Integer(i) => u32::try_from(*i).ok().map(|i| i + 1),
        _ => None,
    }
}

/// Gather name → destination-array from both the legacy catalog `/Dests`
/// dictionary and the `/Names` → `/Dests` name tree.
fn collect_named_destinations(doc: &Document) -> HashMap<Vec<u8>, Vec<Object>> {
    let mut map = HashMap::new();
    let Ok(catalog) = doc.catalog() else {
        return map;
    };
    if let Ok(dests_obj) = catalog.get(b"Dests")
        && let Some(dests) = resolve_dict(doc, dests_obj)
    {
        for (name, value) in dests.iter() {
            if let Some(arr) = dest_value_to_array(doc, value, 0) {
                map.insert(name.clone(), arr);
            }
        }
    }
    if let Ok(names_obj) = catalog.get(b"Names")
        && let Some(names) = resolve_dict(doc, names_obj)
        && let Ok(tree_obj) = names.get(b"Dests")
        && let Some(tree) = resolve_dict(doc, tree_obj)
    {
        walk_name_tree(doc, &tree, &mut map, 0);
    }
    map
}

fn walk_name_tree(
    doc: &Document,
    node: &Dictionary,
    map: &mut HashMap<Vec<u8>, Vec<Object>>,
    depth: usize,
) {
    if depth > 32 {
        return;
    }
    if let Ok(kids) = node.get(b"Kids")
        && let Some(kids) = resolve_array(doc, kids)
    {
        for kid in &kids {
            if let Some(kid) = resolve_dict(doc, kid) {
                walk_name_tree(doc, &kid, map, depth + 1);
            }
        }
    }
    if let Ok(names) = node.get(b"Names")
        && let Some(names) = resolve_array(doc, names)
    {
        for pair in names.chunks_exact(2) {
            let Ok(key) = pair[0].as_str() else {
                continue;
            };
            if let Some(arr) = dest_value_to_array(doc, &pair[1], 0) {
                map.insert(key.to_vec(), arr);
            }
        }
    }
}

/// A named destination's value: a dest array, a reference to one, or a
/// dictionary wrapping one under `/D`.
fn dest_value_to_array(doc: &Document, value: &Object, depth: usize) -> Option<Vec<Object>> {
    if depth > 8 {
        return None;
    }
    match value {
        Object::Reference(id) => {
            dest_value_to_array(doc, doc.get_object(*id).ok()?, depth + 1)
        }
        Object::Array(arr) => Some(arr.clone()),
        Object::Dictionary(d) => dest_value_to_array(doc, d.get(b"D").ok()?, depth + 1),
        _ => None,
    }
}

fn resolve_object(doc: &Document, obj: &Object) -> Option<Object> {
    match obj {
        Object::Reference(id) => doc.get_object(*id).ok().cloned(),
        other => Some(other.clone()),
    }
}

fn resolve_dict(doc: &Document, obj: &Object) -> Option<Dictionary> {
    match obj {
        Object::Reference(id) => doc.get_dictionary(*id).ok().cloned(),
        Object::Dictionary(d) => Some(d.clone()),
        _ => None,
    }
}

fn resolve_array(doc: &Document, obj: &Object) -> Option<Vec<Object>> {
    match obj {
        Object::Reference(id) => doc
            .get_object(*id)
            .ok()
            .and_then(|o| o.as_array().ok())
            .cloned(),
        Object::Array(a) => Some(a.clone()),
        _ => None,
    }
}

/// Fallback text extraction, one String per page (PRD §4 step 4). The
/// caller assembles `sections.md` with `## Page N` headings. Used only
/// when LaTeX is unavailable or pandoc fails — graceful degradation.
pub fn extract_text_per_page(pdf_path: &Path) -> Result<Vec<String>, KbError> {
    pdf_extract::extract_text_by_pages(pdf_path).map_err(|e| {
        KbError::Extraction(format!(
            "text extraction from {} failed: {e}",
            pdf_path.display()
        ))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use lopdf::content::{Content, Operation};
    use lopdf::{Stream, dictionary};

    fn dest_array(page: ObjectId) -> Object {
        Object::Array(vec![
            Object::Reference(page),
            Object::Name(b"XYZ".to_vec()),
            Object::Null,
            Object::Null,
            Object::Null,
        ])
    }

    /// Three pages with text, plus (optionally) an outline:
    ///   Introduction              -> page 1 (explicit /Dest array)
    ///   Method                    -> page 2 (named destination "sec.method")
    ///     3.2 Lloyd-Max Quantization -> page 3 (/A GoTo action)
    fn build_pdf(path: &Path, with_outline: bool) {
        let mut doc = Document::with_version("1.5");
        let pages_id = doc.new_object_id();
        let font_id = doc.add_object(dictionary! {
            "Type" => "Font",
            "Subtype" => "Type1",
            "BaseFont" => "Helvetica",
        });
        let resources_id = doc.add_object(dictionary! {
            "Font" => dictionary! { "F1" => font_id },
        });

        let mut page_ids: Vec<ObjectId> = Vec::new();
        for i in 1..=3 {
            let content = Content {
                operations: vec![
                    Operation::new("BT", vec![]),
                    Operation::new("Tf", vec!["F1".into(), 24.into()]),
                    Operation::new("Td", vec![72.into(), 700.into()]),
                    Operation::new(
                        "Tj",
                        vec![Object::string_literal(format!("Hello from page {i}"))],
                    ),
                    Operation::new("ET", vec![]),
                ],
            };
            let content_id =
                doc.add_object(Stream::new(dictionary! {}, content.encode().unwrap()));
            let page_id = doc.add_object(dictionary! {
                "Type" => "Page",
                "Parent" => pages_id,
                "Contents" => content_id,
                "MediaBox" => vec![0.into(), 0.into(), 612.into(), 792.into()],
                "Resources" => resources_id,
            });
            page_ids.push(page_id);
        }
        doc.objects.insert(
            pages_id,
            Object::Dictionary(dictionary! {
                "Type" => "Pages",
                "Kids" => page_ids.iter().map(|id| Object::Reference(*id)).collect::<Vec<_>>(),
                "Count" => 3,
            }),
        );

        let mut catalog = dictionary! {
            "Type" => "Catalog",
            "Pages" => pages_id,
        };

        if with_outline {
            let outlines_id = doc.new_object_id();
            let item1_id = doc.new_object_id();
            let item2_id = doc.new_object_id();
            let child_id = doc.new_object_id();
            doc.objects.insert(
                item1_id,
                Object::Dictionary(dictionary! {
                    "Title" => Object::string_literal("Introduction"),
                    "Parent" => outlines_id,
                    "Next" => item2_id,
                    "Dest" => dest_array(page_ids[0]),
                }),
            );
            doc.objects.insert(
                item2_id,
                Object::Dictionary(dictionary! {
                    "Title" => Object::string_literal("Method"),
                    "Parent" => outlines_id,
                    "Prev" => item1_id,
                    "First" => child_id,
                    "Last" => child_id,
                    "Count" => 1,
                    "Dest" => Object::string_literal("sec.method"),
                }),
            );
            doc.objects.insert(
                child_id,
                Object::Dictionary(dictionary! {
                    "Title" => Object::string_literal("3.2 Lloyd-Max Quantization"),
                    "Parent" => item2_id,
                    "A" => dictionary! {
                        "S" => Object::Name(b"GoTo".to_vec()),
                        "D" => dest_array(page_ids[2]),
                    },
                }),
            );
            doc.objects.insert(
                outlines_id,
                Object::Dictionary(dictionary! {
                    "Type" => "Outlines",
                    "First" => item1_id,
                    "Last" => item2_id,
                    "Count" => 3,
                }),
            );

            // Named-destination name tree: "sec.method" -> page 2.
            let dests_id = doc.add_object(dictionary! {
                "Names" => vec![
                    Object::string_literal("sec.method"),
                    dest_array(page_ids[1]),
                ],
            });
            let names_id = doc.add_object(dictionary! { "Dests" => dests_id });
            catalog.set("Outlines", outlines_id);
            catalog.set("Names", names_id);
        }

        let catalog_id = doc.add_object(Object::Dictionary(catalog));
        doc.trailer.set("Root", catalog_id);
        doc.save(path).unwrap();
    }

    #[test]
    fn extracts_outline_depth_first_with_pages_and_named_dests() {
        let tmp = tempfile::tempdir().unwrap();
        let pdf = tmp.path().join("paper.pdf");
        build_pdf(&pdf, true);

        let toc = extract_toc(&pdf).unwrap();
        assert_eq!(
            toc,
            vec![
                TocEntry {
                    title: "Introduction".into(),
                    page: 1,
                    named_dest: None,
                },
                TocEntry {
                    title: "Method".into(),
                    page: 2,
                    named_dest: Some("sec.method".into()),
                },
                TocEntry {
                    title: "3.2 Lloyd-Max Quantization".into(),
                    page: 3,
                    named_dest: None,
                },
            ]
        );
    }

    #[test]
    fn pdf_without_outline_yields_empty_toc() {
        let tmp = tempfile::tempdir().unwrap();
        let pdf = tmp.path().join("plain.pdf");
        build_pdf(&pdf, false);
        assert_eq!(extract_toc(&pdf).unwrap(), Vec::new());
    }

    #[test]
    fn malformed_pdf_is_extraction_error() {
        let tmp = tempfile::tempdir().unwrap();
        let bogus = tmp.path().join("bogus.pdf");
        std::fs::write(&bogus, b"this is not a pdf at all").unwrap();
        let err = extract_toc(&bogus).unwrap_err();
        assert!(matches!(err, KbError::Extraction(_)), "got {err:?}");
    }

    #[test]
    fn utf16be_outline_titles_decode() {
        // lopdf writes plain literals above; here, force a UTF-16BE title
        // (the common case in real arXiv PDFs produced by hyperref).
        let tmp = tempfile::tempdir().unwrap();
        let pdf = tmp.path().join("utf16.pdf");
        {
            let mut doc = Document::with_version("1.5");
            let pages_id = doc.new_object_id();
            let page_id = doc.add_object(dictionary! {
                "Type" => "Page",
                "Parent" => pages_id,
                "MediaBox" => vec![0.into(), 0.into(), 612.into(), 792.into()],
            });
            doc.objects.insert(
                pages_id,
                Object::Dictionary(dictionary! {
                    "Type" => "Pages",
                    "Kids" => vec![Object::Reference(page_id)],
                    "Count" => 1,
                }),
            );
            let outlines_id = doc.new_object_id();
            let item_id = doc.new_object_id();
            let mut title_bytes = vec![0xfe, 0xff];
            for unit in "Méthode".encode_utf16() {
                title_bytes.extend_from_slice(&unit.to_be_bytes());
            }
            doc.objects.insert(
                item_id,
                Object::Dictionary(dictionary! {
                    "Title" => Object::String(title_bytes, lopdf::StringFormat::Hexadecimal),
                    "Parent" => outlines_id,
                    "Dest" => dest_array(page_id),
                }),
            );
            doc.objects.insert(
                outlines_id,
                Object::Dictionary(dictionary! {
                    "Type" => "Outlines",
                    "First" => item_id,
                    "Last" => item_id,
                    "Count" => 1,
                }),
            );
            let catalog_id = doc.add_object(dictionary! {
                "Type" => "Catalog",
                "Pages" => pages_id,
                "Outlines" => outlines_id,
            });
            doc.trailer.set("Root", catalog_id);
            doc.save(&pdf).unwrap();
        }
        let toc = extract_toc(&pdf).unwrap();
        assert_eq!(toc.len(), 1);
        assert_eq!(toc[0].title, "Méthode");
        assert_eq!(toc[0].page, 1);
    }

    #[test]
    fn extracts_text_per_page() {
        let tmp = tempfile::tempdir().unwrap();
        let pdf = tmp.path().join("paper.pdf");
        build_pdf(&pdf, false);

        let pages = extract_text_per_page(&pdf).unwrap();
        assert_eq!(pages.len(), 3);
        assert!(pages[0].contains("Hello from page 1"), "got {:?}", pages[0]);
        assert!(pages[1].contains("Hello from page 2"), "got {:?}", pages[1]);
        assert!(pages[2].contains("Hello from page 3"), "got {:?}", pages[2]);
    }

    #[test]
    fn text_extraction_on_garbage_is_extraction_error() {
        let tmp = tempfile::tempdir().unwrap();
        let bogus = tmp.path().join("bogus.pdf");
        std::fs::write(&bogus, b"nope").unwrap();
        let err = extract_text_per_page(&bogus).unwrap_err();
        assert!(matches!(err, KbError::Extraction(_)), "got {err:?}");
    }

    #[test]
    fn pdf_url_shape() {
        assert_eq!(pdf_url("2504.19874"), "https://arxiv.org/pdf/2504.19874");
    }
}
