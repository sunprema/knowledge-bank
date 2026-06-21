//! arXiv API client and ID parsing (PRD §4 input + step 1).

use crate::{KbError, PaperMetadata, SCHEMA_VERSION, SourceFormat, now_rfc3339};
use std::time::Duration;

/// All arXiv API URL construction lives here (single place; the HTTP path
/// of [`fetch_metadata`] is intentionally thin so parsing stays testable).
pub(crate) fn metadata_query_url(arxiv_id: &str) -> String {
    format!("https://export.arxiv.org/api/query?id_list={arxiv_id}")
}

/// Build a search-listing URL for the arXiv query API, newest first. The
/// `search_query` is an arXiv query expression — e.g. `cat:cs.LG`,
/// `all:retrieval augmented`, `au:Vaswani`, or a boolean combination. Used by
/// ArXiv Watch to list recent submissions matching a standing interest.
pub(crate) fn search_query_url(search_query: &str, max_results: usize) -> String {
    let mut url = url::Url::parse("https://export.arxiv.org/api/query")
        .expect("static arXiv API base URL is valid");
    url.query_pairs_mut()
        .append_pair("search_query", search_query)
        .append_pair("sortBy", "submittedDate")
        .append_pair("sortOrder", "descending")
        .append_pair("max_results", &max_results.to_string());
    url.into()
}

/// Parse any accepted form into `(canonical_id, version)`:
///
/// - `2504.19874`
/// - `2504.19874v2`
/// - `arxiv:2504.19874`
/// - `https://arxiv.org/abs/2504.19874`
/// - `https://arxiv.org/pdf/2504.19874`
/// - `https://arxiv.org/pdf/2504.19874v2`
///
/// Canonical id is `YYMM.NNNNN` (4-5 digits after the dot, modern scheme
/// only for v0.1). The version suffix, if present, is returned separately —
/// v0.1 always fetches the latest version but records what it got.
/// Unrecognized input is a `Usage` error (exit 1).
pub fn parse_arxiv_id(input: &str) -> Result<(String, Option<String>), KbError> {
    let usage = || KbError::Usage(format!("unrecognized arXiv id or URL: {input}"));

    let mut s = input.trim();
    // Drop query string / fragment, then trailing slashes.
    s = s.split(['?', '#']).next().unwrap_or(s);
    s = s.trim_end_matches('/');

    for scheme in ["https://", "http://"] {
        if let Some(rest) = s.strip_prefix(scheme) {
            s = rest;
            break;
        }
    }
    if let Some(rest) = s.strip_prefix("www.") {
        s = rest;
    }

    if let Some(rest) = s.strip_prefix("arxiv.org/") {
        s = rest
            .strip_prefix("abs/")
            .or_else(|| rest.strip_prefix("pdf/"))
            .ok_or_else(usage)?;
        // Tolerate an explicit `.pdf` extension on pdf URLs.
        s = s.strip_suffix(".pdf").unwrap_or(s);
    } else if s.len() >= 6 && s[..6].eq_ignore_ascii_case("arxiv:") {
        s = &s[6..];
    }

    split_id_version(s).ok_or_else(usage)
}

/// `"2504.19874v2"` → `("2504.19874", Some("v2"))`. None if the string is
/// not a modern-scheme arXiv id.
fn split_id_version(s: &str) -> Option<(String, Option<String>)> {
    let bytes = s.as_bytes();
    if bytes.len() < 9 || !bytes[..4].iter().all(u8::is_ascii_digit) || bytes[4] != b'.' {
        return None;
    }
    let after_dot = &s[5..];
    let num_len = after_dot
        .bytes()
        .take_while(u8::is_ascii_digit)
        .count();
    if !(4..=5).contains(&num_len) {
        return None;
    }
    let id = format!("{}.{}", &s[..4], &after_dot[..num_len]);
    let tail = &after_dot[num_len..];
    if tail.is_empty() {
        return Some((id, None));
    }
    let digits = tail.strip_prefix('v')?;
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    Some((id, Some(tail.to_string())))
}

/// Fetch metadata from `https://export.arxiv.org/api/query?id_list={id}`
/// and shape it via [`parse_atom_metadata`].
///
/// Failure policy (PRD §14): non-existent id ⇒ `NotFound` (exit 2);
/// rate-limited ⇒ wait 3s and retry, `Network` (exit 3) after 3 failures.
pub async fn fetch_metadata(
    client: &reqwest::Client,
    arxiv_id: &str,
) -> Result<PaperMetadata, KbError> {
    const MAX_ATTEMPTS: u32 = 3;
    const RETRY_DELAY: Duration = Duration::from_secs(3);

    let url = metadata_query_url(arxiv_id);
    let mut last_err = KbError::Network("arXiv API: request not attempted".to_string());
    for attempt in 1..=MAX_ATTEMPTS {
        if attempt > 1 {
            tracing::warn!(
                "arXiv API attempt {} failed ({last_err}); retrying in {}s",
                attempt - 1,
                RETRY_DELAY.as_secs()
            );
            tokio::time::sleep(RETRY_DELAY).await;
        }
        match client.get(&url).send().await {
            Ok(resp) => {
                let status = resp.status();
                if status.is_success() {
                    let body = resp.text().await.map_err(KbError::from)?;
                    return parse_atom_metadata(&body, arxiv_id);
                }
                let err = KbError::Network(format!("arXiv API returned HTTP {status}"));
                let retryable = status == reqwest::StatusCode::TOO_MANY_REQUESTS
                    || status.is_server_error();
                if !retryable {
                    return Err(err);
                }
                last_err = err;
            }
            Err(e) => last_err = e.into(),
        }
    }
    Err(last_err)
}

/// Fetch a search listing from the arXiv query API and shape every entry via
/// [`parse_atom_feed`]. Newest-first; at most `max_results` entries. Powers
/// ArXiv Watch — a standing `search_query` (category, author, or free text)
/// polled for recent submissions. Shares [`fetch_metadata`]'s retry policy.
///
/// Unlike [`fetch_metadata`] an empty feed is **not** an error here: a watch
/// that currently matches nothing simply yields `Ok(vec![])`.
pub async fn fetch_search(
    client: &reqwest::Client,
    search_query: &str,
    max_results: usize,
) -> Result<Vec<PaperMetadata>, KbError> {
    const MAX_ATTEMPTS: u32 = 3;
    const RETRY_DELAY: Duration = Duration::from_secs(3);

    let url = search_query_url(search_query, max_results);
    let mut last_err = KbError::Network("arXiv API: request not attempted".to_string());
    for attempt in 1..=MAX_ATTEMPTS {
        if attempt > 1 {
            tracing::warn!(
                "arXiv search attempt {} failed ({last_err}); retrying in {}s",
                attempt - 1,
                RETRY_DELAY.as_secs()
            );
            tokio::time::sleep(RETRY_DELAY).await;
        }
        match client.get(&url).send().await {
            Ok(resp) => {
                let status = resp.status();
                if status.is_success() {
                    let body = resp.text().await.map_err(KbError::from)?;
                    return parse_atom_feed(&body);
                }
                let err = KbError::Network(format!("arXiv API returned HTTP {status}"));
                let retryable = status == reqwest::StatusCode::TOO_MANY_REQUESTS
                    || status.is_server_error();
                if !retryable {
                    return Err(err);
                }
                last_err = err;
            }
            Err(e) => last_err = e.into(),
        }
    }
    Err(last_err)
}

/// Parse the Atom XML payload into PaperMetadata. Separated from the HTTP
/// call so it is unit-testable against fixture XML.
///
/// Fills: title (whitespace-normalized), authors, abstract (trimmed),
/// categories (all `<category term=…>`), published_at, updated_at, and
/// `version` extracted from the entry's `<id>` URL (e.g. `…/abs/2504.19874v2`).
/// Sets `ingested_at` = now, `source_format` = Latex (corrected later by the
/// pipeline if the e-print has no LaTeX), `schema_version` = SCHEMA_VERSION.
/// An Atom feed with zero entries ⇒ `NotFound`.
pub fn parse_atom_metadata(xml: &str, arxiv_id: &str) -> Result<PaperMetadata, KbError> {
    let doc = roxmltree::Document::parse(xml)
        .map_err(|e| KbError::Network(format!("arXiv API returned malformed XML: {e}")))?;

    let entry = doc
        .root_element()
        .children()
        .find(|n| n.is_element() && n.tag_name().name() == "entry")
        .ok_or_else(|| KbError::NotFound(format!("paper {arxiv_id} not found on arxiv")))?;

    parse_entry(&entry, arxiv_id)
}

/// Parse every `<entry>` in a search-listing feed, deriving each paper's
/// canonical arXiv id from its own `<id>` URL. Entries whose id can't be
/// parsed (e.g. old-scheme ids, out of scope for v0.1) are skipped rather than
/// failing the whole batch. A feed with zero usable entries ⇒ `Ok(vec![])`.
pub fn parse_atom_feed(xml: &str) -> Result<Vec<PaperMetadata>, KbError> {
    let doc = roxmltree::Document::parse(xml)
        .map_err(|e| KbError::Network(format!("arXiv API returned malformed XML: {e}")))?;

    let mut out = Vec::new();
    for entry in doc
        .root_element()
        .children()
        .filter(|n| n.is_element() && n.tag_name().name() == "entry")
    {
        // Derive `YYMM.NNNNN` (sans version) from <id>; skip unparseable ids.
        let Some((id, _)) = entry_id_version(&entry) else {
            continue;
        };
        if let Ok(meta) = parse_entry(&entry, &id) {
            out.push(meta);
        }
    }
    Ok(out)
}

/// Shape a single Atom `<entry>` node into [`PaperMetadata`], given the
/// canonical arXiv id (the caller knows it: from `id_list` for a single fetch,
/// or from the entry's own `<id>` for a search listing).
fn parse_entry(entry: &roxmltree::Node, arxiv_id: &str) -> Result<PaperMetadata, KbError> {
    let child_text = |name: &str| entry_child_text(entry, name);

    let title = normalize_whitespace(&child_text("title").unwrap_or_default());
    if title.is_empty() {
        return Err(KbError::Network(format!(
            "arXiv API entry for {arxiv_id} has no title"
        )));
    }

    let authors: Vec<String> = entry
        .children()
        .filter(|n| n.is_element() && n.tag_name().name() == "author")
        .filter_map(|a| {
            a.children()
                .find(|n| n.is_element() && n.tag_name().name() == "name")
                .map(|n| normalize_whitespace(&element_text(&n)))
        })
        .filter(|n| !n.is_empty())
        .collect();

    let categories: Vec<String> = entry
        .children()
        .filter(|n| n.is_element() && n.tag_name().name() == "category")
        .filter_map(|c| c.attribute("term").map(str::to_string))
        .collect();

    // <id>http://arxiv.org/abs/2504.19874v2</id> → version "v2"
    let version = entry_id_version(entry).and_then(|(_, v)| v);

    Ok(PaperMetadata {
        arxiv_id: arxiv_id.to_string(),
        kind: crate::DocKind::Paper,
        project: None,
        links: Vec::new(),
        version,
        title,
        authors,
        abstract_text: child_text("summary").unwrap_or_default().trim().to_string(),
        categories,
        published_at: child_text("published").unwrap_or_default().trim().to_string(),
        updated_at: child_text("updated").unwrap_or_default().trim().to_string(),
        ingested_at: now_rfc3339(),
        source_format: SourceFormat::Latex,
        source_url: None,
        main_tex: None,
        tags: Vec::new(),
        schema_version: SCHEMA_VERSION,
    })
}

/// Direct-child element text by tag name (first match).
fn entry_child_text(entry: &roxmltree::Node, name: &str) -> Option<String> {
    entry
        .children()
        .find(|n| n.is_element() && n.tag_name().name() == name)
        .map(|n| element_text(&n))
}

/// `(canonical_id, version)` parsed from an entry's `<id>` URL, e.g.
/// `http://arxiv.org/abs/2504.19874v2` → `("2504.19874", Some("v2"))`.
fn entry_id_version(entry: &roxmltree::Node) -> Option<(String, Option<String>)> {
    let id_url = entry_child_text(entry, "id")?;
    let tail = id_url.trim();
    let tail = tail.rsplit("/abs/").next().unwrap_or(tail);
    split_id_version(tail)
}

/// All descendant text of an element, concatenated (titles occasionally
/// contain nested markup).
fn element_text(node: &roxmltree::Node) -> String {
    node.descendants()
        .filter(|n| n.is_text())
        .filter_map(|n| n.text())
        .collect()
}

fn normalize_whitespace(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    // -------- parse_arxiv_id --------

    #[test]
    fn parses_bare_id() {
        assert_eq!(
            parse_arxiv_id("2504.19874").unwrap(),
            ("2504.19874".to_string(), None)
        );
    }

    #[test]
    fn parses_bare_id_with_version() {
        assert_eq!(
            parse_arxiv_id("2504.19874v2").unwrap(),
            ("2504.19874".to_string(), Some("v2".to_string()))
        );
    }

    #[test]
    fn parses_arxiv_prefix() {
        assert_eq!(
            parse_arxiv_id("arxiv:2504.19874").unwrap(),
            ("2504.19874".to_string(), None)
        );
        // Case-insensitive prefix.
        assert_eq!(
            parse_arxiv_id("arXiv:2504.19874v3").unwrap(),
            ("2504.19874".to_string(), Some("v3".to_string()))
        );
    }

    #[test]
    fn parses_abs_url() {
        assert_eq!(
            parse_arxiv_id("https://arxiv.org/abs/2504.19874").unwrap(),
            ("2504.19874".to_string(), None)
        );
    }

    #[test]
    fn parses_pdf_url() {
        assert_eq!(
            parse_arxiv_id("https://arxiv.org/pdf/2504.19874").unwrap(),
            ("2504.19874".to_string(), None)
        );
    }

    #[test]
    fn parses_pdf_url_with_version() {
        assert_eq!(
            parse_arxiv_id("https://arxiv.org/pdf/2504.19874v2").unwrap(),
            ("2504.19874".to_string(), Some("v2".to_string()))
        );
    }

    #[test]
    fn parses_url_variants() {
        // http scheme, www host, trailing slash, query string, .pdf suffix
        assert_eq!(
            parse_arxiv_id("http://www.arxiv.org/abs/2504.19874/").unwrap().0,
            "2504.19874"
        );
        assert_eq!(
            parse_arxiv_id("https://arxiv.org/pdf/2504.19874v1.pdf").unwrap(),
            ("2504.19874".to_string(), Some("v1".to_string()))
        );
        assert_eq!(
            parse_arxiv_id("https://arxiv.org/abs/2504.19874?context=cs.IR")
                .unwrap()
                .0,
            "2504.19874"
        );
    }

    #[test]
    fn parses_four_digit_number_part() {
        assert_eq!(
            parse_arxiv_id("2405.1249").unwrap(),
            ("2405.1249".to_string(), None)
        );
    }

    #[test]
    fn rejects_bad_input() {
        for bad in [
            "",
            "abc",
            "250.19874",          // 3-digit prefix
            "2504.198",           // too few digits after dot
            "2504.198745v",       // 6 digits... also dangling v
            "2504.19874v",        // dangling version
            "2504.19874vX",       // non-numeric version
            "2504-19874",         // wrong separator
            "https://arxiv.org/html/2504.19874", // unsupported path
            "https://example.com/abs/2504.19874",
            "math.GT/0309136",    // old-scheme id (out of scope for v0.1)
        ] {
            let err = parse_arxiv_id(bad).unwrap_err();
            assert!(matches!(err, KbError::Usage(_)), "{bad:?} gave {err:?}");
        }
    }

    #[test]
    fn rejects_six_digit_suffix() {
        assert!(matches!(
            parse_arxiv_id("2504.198746").unwrap_err(),
            KbError::Usage(_)
        ));
    }

    // -------- parse_atom_metadata --------

    const FIXTURE: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<feed xmlns="http://www.w3.org/2005/Atom">
  <link href="http://arxiv.org/api/query?search_query%3D%26id_list%3D2504.19874" rel="self" type="application/atom+xml"/>
  <title type="html">ArXiv Query: search_query=&amp;id_list=2504.19874</title>
  <id>http://arxiv.org/api/abc123</id>
  <updated>2026-06-01T00:00:00-04:00</updated>
  <opensearch:totalResults xmlns:opensearch="http://a9.com/-/spec/opensearch/1.1/">1</opensearch:totalResults>
  <entry>
    <id>http://arxiv.org/abs/2504.19874v2</id>
    <updated>2024-09-15T17:12:45Z</updated>
    <published>2024-04-28T14:02:21Z</published>
    <title>TurboQuant: Online Vector Quantization with
  Near-optimal Distortion Rate</title>
    <summary>  We present TurboQuant, an online vector quantization
algorithm achieving near-optimal distortion rate.
</summary>
    <author>
      <name>Amir Zandieh</name>
    </author>
    <author>
      <name>Majid Daliri</name>
    </author>
    <arxiv:comment xmlns:arxiv="http://arxiv.org/schemas/atom">29 pages</arxiv:comment>
    <link href="http://arxiv.org/abs/2504.19874v2" rel="alternate" type="text/html"/>
    <link title="pdf" href="http://arxiv.org/pdf/2504.19874v2" rel="related" type="application/pdf"/>
    <arxiv:primary_category xmlns:arxiv="http://arxiv.org/schemas/atom" term="cs.IR" scheme="http://arxiv.org/schemas/atom"/>
    <category term="cs.IR" scheme="http://arxiv.org/schemas/atom"/>
    <category term="cs.LG" scheme="http://arxiv.org/schemas/atom"/>
  </entry>
</feed>"#;

    #[test]
    fn parses_atom_entry() {
        let meta = parse_atom_metadata(FIXTURE, "2504.19874").unwrap();
        assert_eq!(meta.arxiv_id, "2504.19874");
        assert_eq!(meta.version.as_deref(), Some("v2"));
        // Whitespace-normalized title (the fixture wraps across lines).
        assert_eq!(
            meta.title,
            "TurboQuant: Online Vector Quantization with Near-optimal Distortion Rate"
        );
        assert_eq!(meta.authors, vec!["Amir Zandieh", "Majid Daliri"]);
        assert!(meta.abstract_text.starts_with("We present TurboQuant"));
        assert!(meta.abstract_text.ends_with("distortion rate."));
        assert_eq!(meta.categories, vec!["cs.IR", "cs.LG"]);
        assert_eq!(meta.published_at, "2024-04-28T14:02:21Z");
        assert_eq!(meta.updated_at, "2024-09-15T17:12:45Z");
        assert_eq!(meta.source_format, SourceFormat::Latex);
        assert_eq!(meta.schema_version, SCHEMA_VERSION);
        assert!(meta.main_tex.is_none());
        assert!(meta.tags.is_empty());
        assert!(!meta.ingested_at.is_empty());
    }

    #[test]
    fn zero_entry_feed_is_not_found() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<feed xmlns="http://www.w3.org/2005/Atom">
  <title type="html">ArXiv Query: search_query=&amp;id_list=9999.99999</title>
  <id>http://arxiv.org/api/empty</id>
  <updated>2026-06-01T00:00:00-04:00</updated>
  <opensearch:totalResults xmlns:opensearch="http://a9.com/-/spec/opensearch/1.1/">0</opensearch:totalResults>
</feed>"#;
        let err = parse_atom_metadata(xml, "9999.99999").unwrap_err();
        assert!(matches!(err, KbError::NotFound(_)), "got {err:?}");
        assert!(err.to_string().contains("not found on arxiv"));
    }

    #[test]
    fn malformed_xml_is_network_error() {
        let err = parse_atom_metadata("this is not xml <", "2504.19874").unwrap_err();
        assert!(matches!(err, KbError::Network(_)), "got {err:?}");
    }

    #[test]
    fn metadata_url_is_export_api() {
        assert_eq!(
            metadata_query_url("2504.19874"),
            "https://export.arxiv.org/api/query?id_list=2504.19874"
        );
    }

    // -------- search listing --------

    #[test]
    fn search_url_encodes_query_and_sorts_newest_first() {
        let url = search_query_url("cat:cs.LG", 25);
        assert!(url.starts_with("https://export.arxiv.org/api/query?"));
        // colon is percent-encoded by the url crate; arXiv accepts either form.
        assert!(url.contains("search_query=cat%3Acs.LG"), "got: {url}");
        assert!(url.contains("sortBy=submittedDate"), "got: {url}");
        assert!(url.contains("sortOrder=descending"), "got: {url}");
        assert!(url.contains("max_results=25"), "got: {url}");
    }

    #[test]
    fn search_url_encodes_spaces_in_free_text_query() {
        let url = search_query_url("all:retrieval augmented", 10);
        assert!(url.contains("search_query=all%3Aretrieval+augmented"), "got: {url}");
    }

    const FEED_FIXTURE: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<feed xmlns="http://www.w3.org/2005/Atom">
  <title type="html">ArXiv Query</title>
  <id>http://arxiv.org/api/list</id>
  <opensearch:totalResults xmlns:opensearch="http://a9.com/-/spec/opensearch/1.1/">2</opensearch:totalResults>
  <entry>
    <id>http://arxiv.org/abs/2506.11111v1</id>
    <updated>2025-06-10T00:00:00Z</updated>
    <published>2025-06-10T00:00:00Z</published>
    <title>First Recent Paper</title>
    <summary>An abstract for the first paper.</summary>
    <author><name>Ada Lovelace</name></author>
    <category term="cs.LG" scheme="http://arxiv.org/schemas/atom"/>
  </entry>
  <entry>
    <id>http://arxiv.org/abs/2506.22222v2</id>
    <updated>2025-06-09T00:00:00Z</updated>
    <published>2025-06-08T00:00:00Z</published>
    <title>Second Recent Paper</title>
    <summary>An abstract for the second paper.</summary>
    <author><name>Alan Turing</name></author>
    <category term="cs.IR" scheme="http://arxiv.org/schemas/atom"/>
  </entry>
</feed>"#;

    #[test]
    fn parses_feed_with_multiple_entries() {
        let papers = parse_atom_feed(FEED_FIXTURE).unwrap();
        assert_eq!(papers.len(), 2);
        assert_eq!(papers[0].arxiv_id, "2506.11111");
        assert_eq!(papers[0].version.as_deref(), Some("v1"));
        assert_eq!(papers[0].title, "First Recent Paper");
        assert_eq!(papers[0].authors, vec!["Ada Lovelace"]);
        assert_eq!(papers[0].categories, vec!["cs.LG"]);
        assert_eq!(papers[1].arxiv_id, "2506.22222");
        assert_eq!(papers[1].version.as_deref(), Some("v2"));
    }

    #[test]
    fn empty_feed_is_ok_not_an_error() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<feed xmlns="http://www.w3.org/2005/Atom"><id>x</id></feed>"#;
        assert!(parse_atom_feed(xml).unwrap().is_empty());
    }

    #[test]
    fn feed_skips_entries_with_unparseable_ids() {
        // One modern-scheme id, one old-scheme id (out of scope) → only 1 kept.
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<feed xmlns="http://www.w3.org/2005/Atom">
  <entry>
    <id>http://arxiv.org/abs/math.GT/0309136</id>
    <title>Old Scheme</title>
    <summary>x</summary>
  </entry>
  <entry>
    <id>http://arxiv.org/abs/2506.33333v1</id>
    <title>Modern Scheme</title>
    <summary>y</summary>
  </entry>
</feed>"#;
        let papers = parse_atom_feed(xml).unwrap();
        assert_eq!(papers.len(), 1);
        assert_eq!(papers[0].arxiv_id, "2506.33333");
    }

    #[test]
    fn single_entry_parser_still_works_after_refactor() {
        let meta = parse_atom_metadata(FIXTURE, "2504.19874").unwrap();
        assert_eq!(meta.arxiv_id, "2504.19874");
        assert_eq!(meta.title, "TurboQuant: Online Vector Quantization with Near-optimal Distortion Rate");
    }
}
