//! arXiv API client and ID parsing (PRD §4 input + step 1).

use crate::{KbError, PaperMetadata};

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
    let _ = input;
    todo!("implemented in the ingest slice")
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
    let _ = (client, arxiv_id);
    todo!("implemented in the ingest slice")
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
    let _ = (xml, arxiv_id);
    todo!("implemented in the ingest slice")
}
