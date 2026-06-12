//! Web-page ingestion (`kb add --url`): fetch a page, extract the main
//! article with a Mozilla-Readability port (`dom_smoothie`), convert the
//! cleaned HTML to markdown (`htmd`) so the section chunker can split and
//! classify it just like a paper's `sections.md`.
//!
//! There is no PDF and no arXiv round-trip; the page URL is the document's
//! canonical identity (stored in metadata), and the on-disk id is a slug
//! derived from it — URLs are not filesystem-safe and would not round-trip
//! as folder names.

use crate::{content_hash, KbError};
use url::Url;

/// Parsed-and-validated page URL. Rejects anything that isn't an
/// `http`/`https` URL with a host (a `Usage` error, exit 1).
pub fn parse_url(input: &str) -> Result<Url, KbError> {
    let url = Url::parse(input.trim())
        .map_err(|e| KbError::Usage(format!("not a valid URL: {input} ({e})")))?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err(KbError::Usage(format!(
            "only http(s) URLs are supported, got {}://",
            url.scheme()
        )));
    }
    if url.host_str().is_none() {
        return Err(KbError::Usage(format!("URL has no host: {input}")));
    }
    Ok(url)
}

/// Filesystem-safe, collision-free document id for a page: a readable
/// host+path slug plus a short hash of the full URL for uniqueness.
///
/// `https://simonwillison.net/2025/llm-pricing/` →
/// `simonwillison-net-2025-llm-pricing-a3f9c2`.
///
/// The hash guarantees two distinct URLs never share an id even when their
/// host+path slugify the same (query strings, fragments, casing). The
/// embedded hyphens and trailing hex keep it out of the arXiv id namespace.
pub fn slug_from_url(url: &Url) -> String {
    let host = url.host_str().unwrap_or("");
    let path = url.path();
    let mut base = slugify(&format!("{host}{path}"));
    // Keep folder names sane; trim at a hyphen boundary, never mid-word.
    const MAX_BASE: usize = 60;
    if base.len() > MAX_BASE {
        base.truncate(MAX_BASE);
        while base.ends_with('-') {
            base.pop();
        }
    }
    // 6 hex chars of the canonical URL (includes query + fragment).
    let hash = &content_hash(url.as_str())[..6];
    if base.is_empty() {
        format!("page-{hash}")
    } else {
        format!("{base}-{hash}")
    }
}

/// Lowercase ASCII alphanumerics; every other run collapses to one hyphen.
/// (Same rule as `pipeline::slug_from_filename`, but infallible — callers
/// here always append a hash, so an empty result is acceptable.)
fn slugify(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
        } else if !out.is_empty() && !out.ends_with('-') {
            out.push('-');
        }
    }
    while out.ends_with('-') {
        out.pop();
    }
    out
}

/// GET the page and return its decoded HTML body. Non-2xx ⇒ `Network`; a
/// response that is clearly not HTML (by `Content-Type`) ⇒ `Extraction`.
pub async fn fetch_html(client: &reqwest::Client, url: &Url) -> Result<String, KbError> {
    let resp = client.get(url.clone()).send().await?;
    let status = resp.status();
    if !status.is_success() {
        return Err(KbError::Network(format!(
            "fetching {url} returned HTTP {status}"
        )));
    }
    if let Some(ct) = resp
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
    {
        let ct = ct.to_ascii_lowercase();
        if !(ct.contains("html") || ct.contains("xml")) {
            return Err(KbError::Extraction(format!(
                "{url} is not an HTML page (Content-Type: {ct})"
            )));
        }
    }
    // reqwest's `charset` feature decodes per the response charset.
    resp.text().await.map_err(KbError::from)
}

/// Run readability extraction on `html` and convert the result to markdown.
/// Returns `(title, markdown)`. `base_url` lets the extractor resolve
/// relative links/metadata. An empty extraction ⇒ `Extraction` error.
pub fn extract_article(html: &str, base_url: &Url) -> Result<(String, String), KbError> {
    use dom_smoothie::Readability;

    let mut readability = Readability::new(html, Some(base_url.as_str()), None)
        .map_err(|e| KbError::Extraction(format!("readability parse failed for {base_url}: {e}")))?;
    let article = readability
        .parse()
        .map_err(|e| KbError::Extraction(format!("readability extraction failed for {base_url}: {e}")))?;

    let markdown = htmd::convert(&article.content)
        .map(|md| md.trim().to_string())
        .unwrap_or_default();
    // Fall back to readability's plain text if the HTML→markdown step
    // produced nothing usable (rare; malformed extracted fragment).
    let body = if markdown.is_empty() {
        article.text_content.trim().to_string()
    } else {
        markdown
    };
    if body.is_empty() {
        return Err(KbError::Extraction(format!(
            "no readable content extracted from {base_url}"
        )));
    }

    let title = normalize_whitespace(&article.title);
    let title = if title.is_empty() {
        base_url.as_str().to_string()
    } else {
        title
    };
    Ok((title, body))
}

fn normalize_whitespace(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_http_and_https() {
        assert!(parse_url("https://example.com/a").is_ok());
        assert!(parse_url("http://example.com").is_ok());
    }

    #[test]
    fn rejects_non_http_schemes_and_garbage() {
        for bad in ["", "ftp://example.com", "file:///etc/passwd", "not a url", "/abs/2504.19874"] {
            assert!(matches!(parse_url(bad), Err(KbError::Usage(_))), "{bad:?}");
        }
    }

    #[test]
    fn slug_is_readable_with_hash_suffix() {
        let url = parse_url("https://simonwillison.net/2025/llm-pricing/").unwrap();
        let slug = slug_from_url(&url);
        assert!(slug.starts_with("simonwillison-net-2025-llm-pricing-"), "{slug}");
        // host+path + '-' + 6 hex chars
        let hash = slug.rsplit('-').next().unwrap();
        assert_eq!(hash.len(), 6);
        assert!(hash.bytes().all(|b| b.is_ascii_hexdigit()));
    }

    #[test]
    fn slug_is_deterministic_and_url_sensitive() {
        let a = slug_from_url(&parse_url("https://example.com/x?p=1").unwrap());
        let b = slug_from_url(&parse_url("https://example.com/x?p=1").unwrap());
        let c = slug_from_url(&parse_url("https://example.com/x?p=2").unwrap());
        assert_eq!(a, b);
        assert_ne!(a, c, "different query strings must not collide");
    }

    #[test]
    fn slug_stays_out_of_arxiv_namespace() {
        let url = parse_url("https://arxiv.org/abs/2504.19874").unwrap();
        let slug = slug_from_url(&url);
        assert!(crate::ingest::arxiv::parse_arxiv_id(&slug).is_err(), "{slug}");
    }

    #[test]
    fn long_paths_are_truncated_at_hyphen_boundary() {
        let url = parse_url(
            "https://example.com/this/is/a/very/long/path/with/many/segments/that/exceeds/the/limit",
        )
        .unwrap();
        let slug = slug_from_url(&url);
        // base (≤60) + '-' + 6 hex
        assert!(slug.len() <= 60 + 1 + 6, "{} chars: {slug}", slug.len());
        assert!(!slug.contains("--"));
    }

    #[test]
    fn extracts_article_to_markdown() {
        let html = r#"<!DOCTYPE html><html><head><title>Hello World</title></head>
            <body><nav>menu junk</nav>
            <article><h1>Big Heading</h1>
            <p>This is the first substantial paragraph of the article body, long
            enough that readability treats it as the main content of the page.</p>
            <h2>Method</h2>
            <p>Another substantial paragraph describing the method in enough detail
            that the readability heuristics keep it in the extracted article.</p>
            </article></body></html>"#;
        let url = parse_url("https://example.com/post").unwrap();
        let (title, md) = extract_article(html, &url).unwrap();
        assert_eq!(title, "Hello World");
        assert!(md.contains("Method"), "markdown should keep headings: {md}");
        assert!(md.contains("first substantial paragraph"), "{md}");
    }
}
