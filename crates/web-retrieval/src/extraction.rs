use std::{collections::BTreeSet, fmt};

use dom_query::Document;

use crate::{CanonicalWebUrl, ValidatedWebMediaType, WebBrokerResponse, WebResponseKind};

/// Maximum decoded HTML input admitted to the DOM parser.
pub const MAX_HTML_INPUT_BYTES: usize = 4 * 1_024 * 1_024;
/// Maximum markup-open tokens admitted before DOM parsing.
pub const MAX_HTML_MARKUP_TOKENS: usize = 50_000;
/// Maximum DOM nodes admitted after HTML parsing.
pub const MAX_HTML_DOM_NODES: usize = 100_000;
/// Maximum title bytes in one extracted page.
pub const MAX_WEB_TITLE_BYTES: usize = 512;
/// Maximum text bytes in one extracted page section.
pub const MAX_WEB_SECTION_BYTES: usize = 2 * 1_024;
/// Maximum selected sections in one extracted page.
pub const MAX_WEB_SECTIONS: usize = 64;
/// Maximum link-label bytes in one extracted page.
pub const MAX_WEB_LINK_LABEL_BYTES: usize = 256;
/// Maximum selected links in one extracted page.
pub const MAX_WEB_LINKS: usize = 64;
/// Maximum aggregate model-candidate bytes in one extracted page.
pub const MAX_WEB_EXTRACTION_BYTES: usize = 48 * 1_024;
/// Maximum decoded JSON, plain-text or Markdown bytes retained for downstream projection.
pub const MAX_WEB_TEXT_OUTPUT_BYTES: usize = 32 * 1_024;

/// Sanitized static-extraction failure.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WebExtractionError {
    /// Only a completed HTTP response can enter a document extractor.
    ResponseNotCompleted,
    /// The validated media type has no WEB-005 textual extractor.
    UnsupportedMediaType,
    /// Text is not strict UTF-8 under the initial charset policy.
    UnsupportedOrMalformedEncoding,
    /// Decoded text contains disallowed control characters.
    InvalidText,
    /// Hostile HTML crossed a parser-complexity ceiling.
    HtmlComplexityLimit,
    /// JSON syntax or structure was invalid.
    InvalidJson,
}

impl fmt::Display for WebExtractionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let message = match self {
            Self::ResponseNotCompleted => "web response was not a completed document",
            Self::UnsupportedMediaType => "web response has no textual extractor",
            Self::UnsupportedOrMalformedEncoding => "web text encoding is unsupported or malformed",
            Self::InvalidText => "web text contains unsupported controls",
            Self::HtmlComplexityLimit => "web page exceeded its extraction complexity limit",
            Self::InvalidJson => "web JSON was invalid",
        };
        formatter.write_str(message)
    }
}

impl std::error::Error for WebExtractionError {}

/// Host-owned reference tying extracted remote text to validated provenance.
///
/// This type has no serialization or debug projection because its canonical
/// URL may contain sensitive query values.
pub struct WebExtractionSource {
    canonical_url: CanonicalWebUrl,
    content_sha256: String,
    retrieved_at_unix_millis: u64,
}

impl WebExtractionSource {
    fn from_response(response: &WebBrokerResponse) -> Self {
        Self {
            canonical_url: response.provenance().canonical_url().clone(),
            content_sha256: response.provenance().content_sha256().to_owned(),
            retrieved_at_unix_millis: response.provenance().retrieved_at_unix_millis(),
        }
    }

    /// Returns the complete canonical source URL.
    #[must_use]
    pub const fn canonical_url(&self) -> &CanonicalWebUrl {
        &self.canonical_url
    }

    /// Returns the validated response-body digest.
    #[must_use]
    pub fn content_sha256(&self) -> &str {
        &self.content_sha256
    }

    /// Returns the host retrieval timestamp as Unix milliseconds.
    #[must_use]
    pub const fn retrieved_at_unix_millis(&self) -> u64 {
        self.retrieved_at_unix_millis
    }
}

/// Explicit host-derived extraction truncation facts.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WebExtractionTruncation {
    text_shortened: bool,
    omitted_sections: usize,
    omitted_links: usize,
}

impl WebExtractionTruncation {
    /// Returns whether any selected text was shortened at a byte boundary.
    #[must_use]
    pub const fn text_shortened(self) -> bool {
        self.text_shortened
    }

    /// Returns the number of section candidates omitted by host limits.
    #[must_use]
    pub const fn omitted_sections(self) -> usize {
        self.omitted_sections
    }

    /// Returns the number of valid link candidates omitted by host limits.
    #[must_use]
    pub const fn omitted_links(self) -> usize {
        self.omitted_links
    }

    /// Returns whether any extraction limit truncated the result.
    #[must_use]
    pub const fn is_truncated(self) -> bool {
        self.text_shortened || self.omitted_sections > 0 || self.omitted_links > 0
    }
}

/// Host-derived semantic category for one remote text section.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WebSectionKind {
    /// Heading content.
    Heading,
    /// Paragraph or fallback body content.
    Paragraph,
    /// List or definition-list item.
    ListItem,
    /// Preformatted content.
    Preformatted,
    /// One table row.
    TableRow,
    /// Figure caption.
    Caption,
}

/// One bounded untrusted page-text section.
pub struct WebTextSection {
    kind: WebSectionKind,
    text: String,
}

impl WebTextSection {
    /// Returns the host-derived section category.
    #[must_use]
    pub const fn kind(&self) -> WebSectionKind {
        self.kind
    }

    /// Returns normalized untrusted remote text.
    #[must_use]
    pub fn untrusted_text(&self) -> &str {
        &self.text
    }
}

/// One bounded untrusted link resolved only for display.
///
/// This type has no serialization or debug projection because its target may
/// contain sensitive query values supplied by the remote page.
pub struct WebDisplayLink {
    label: String,
    target: CanonicalWebUrl,
}

impl WebDisplayLink {
    /// Returns the normalized untrusted remote label.
    #[must_use]
    pub fn untrusted_label(&self) -> &str {
        &self.label
    }

    /// Returns the canonical display-only target. Reading it performs no fetch.
    #[must_use]
    pub const fn target(&self) -> &CanonicalWebUrl {
        &self.target
    }
}

/// Bounded extraction from a static HTML page.
pub struct WebPageExtraction {
    source: WebExtractionSource,
    title: Option<String>,
    sections: Vec<WebTextSection>,
    links: Vec<WebDisplayLink>,
    truncation: WebExtractionTruncation,
}

impl WebPageExtraction {
    /// Returns the trusted source reference.
    #[must_use]
    pub const fn source(&self) -> &WebExtractionSource {
        &self.source
    }

    /// Returns the bounded untrusted remote title.
    #[must_use]
    pub fn untrusted_title(&self) -> Option<&str> {
        self.title.as_deref()
    }

    /// Returns selected untrusted sections in source order.
    #[must_use]
    pub fn sections(&self) -> &[WebTextSection] {
        &self.sections
    }

    /// Returns selected untrusted links in source order.
    #[must_use]
    pub fn links(&self) -> &[WebDisplayLink] {
        &self.links
    }

    /// Returns explicit truncation facts.
    #[must_use]
    pub const fn truncation(&self) -> WebExtractionTruncation {
        self.truncation
    }
}

/// Bounded extraction from static JSON or plain text.
pub struct WebTextExtraction {
    source: WebExtractionSource,
    text: String,
    truncation: WebExtractionTruncation,
}

impl WebTextExtraction {
    /// Returns the trusted source reference.
    #[must_use]
    pub const fn source(&self) -> &WebExtractionSource {
        &self.source
    }

    /// Returns bounded untrusted remote text in source order.
    #[must_use]
    pub fn untrusted_text(&self) -> &str {
        &self.text
    }

    /// Returns explicit truncation facts.
    #[must_use]
    pub const fn truncation(&self) -> WebExtractionTruncation {
        self.truncation
    }
}

/// Closed WEB-005 static extraction result.
pub enum WebExtraction {
    /// Parsed static HTML page.
    Html(WebPageExtraction),
    /// Validated, bounded static JSON source.
    Json(WebTextExtraction),
    /// Validated, bounded plain text.
    PlainText(WebTextExtraction),
}

/// Extracts one completed WEB-004 response without network activity.
///
/// # Errors
///
/// Returns a sanitized error for a non-success response, unsupported media,
/// malformed UTF-8/control content, excessive HTML complexity or invalid JSON.
pub fn extract_web_response(
    response: &WebBrokerResponse,
) -> Result<WebExtraction, WebExtractionError> {
    if response.kind() != WebResponseKind::Completed {
        return Err(WebExtractionError::ResponseNotCompleted);
    }
    let media_type = response
        .provenance()
        .media_type()
        .ok_or(WebExtractionError::UnsupportedMediaType)?;
    match media_type {
        ValidatedWebMediaType::Html => extract_html(response).map(WebExtraction::Html),
        ValidatedWebMediaType::Json => extract_json(response).map(WebExtraction::Json),
        ValidatedWebMediaType::PlainText | ValidatedWebMediaType::Markdown => {
            extract_plain_text(response).map(WebExtraction::PlainText)
        }
        ValidatedWebMediaType::Jpeg | ValidatedWebMediaType::Png | ValidatedWebMediaType::Webp => {
            Err(WebExtractionError::UnsupportedMediaType)
        }
    }
}

fn extract_html(response: &WebBrokerResponse) -> Result<WebPageExtraction, WebExtractionError> {
    let decoded = decode_utf8(response.untrusted_body())?;
    validate_controls(decoded)?;
    if decoded.len() > MAX_HTML_INPUT_BYTES
        || decoded.bytes().filter(|byte| *byte == b'<').count() > MAX_HTML_MARKUP_TOKENS
    {
        return Err(WebExtractionError::HtmlComplexityLimit);
    }

    let document = Document::from(decoded);
    if document.root().descendants().len() > MAX_HTML_DOM_NODES {
        return Err(WebExtractionError::HtmlComplexityLimit);
    }
    let title = document
        .select_single("title")
        .nodes()
        .first()
        .map(|node| normalize_whitespace(&node.text()))
        .filter(|value| !value.is_empty());

    document
        .select(
            "script,style,template,noscript,meta,link,base,iframe,frame,object,embed,canvas,svg,math,input,textarea,select,option,button,datalist,dialog",
        )
        .remove();
    remove_hidden_nodes(&document);
    document.select("head").remove();

    let mut budget = ExtractionBudget::new(MAX_WEB_EXTRACTION_BYTES);
    let (title, title_shortened) = title.map_or((None, false), |title| {
        let (title, shortened) = bounded_text(&title, MAX_WEB_TITLE_BYTES, budget.remaining());
        budget.consume(title.len());
        (some_if_nonempty(title), shortened)
    });

    let (sections, omitted_sections, section_text_shortened) =
        extract_html_sections(&document, &mut budget);
    let (links, omitted_links, link_text_shortened) =
        extract_html_links(&document, response, &mut budget);

    Ok(WebPageExtraction {
        source: WebExtractionSource::from_response(response),
        title,
        sections,
        links,
        truncation: WebExtractionTruncation {
            text_shortened: title_shortened || section_text_shortened || link_text_shortened,
            omitted_sections,
            omitted_links,
        },
    })
}

fn extract_html_sections(
    document: &Document,
    budget: &mut ExtractionBudget,
) -> (Vec<WebTextSection>, usize, bool) {
    let mut sections = Vec::new();
    let mut omitted = 0usize;
    let mut shortened_any = false;
    for node in document
        .select("h1,h2,h3,h4,h5,h6,p,li,dt,dd,pre,tr,figcaption")
        .nodes()
    {
        if sections.len() >= MAX_WEB_SECTIONS || budget.remaining() == 0 {
            omitted = omitted.saturating_add(1);
            continue;
        }
        let normalized = normalize_whitespace(&node.formatted_text());
        if normalized.is_empty() {
            continue;
        }
        let (text, shortened) =
            bounded_text(&normalized, MAX_WEB_SECTION_BYTES, budget.remaining());
        if text.is_empty() {
            omitted = omitted.saturating_add(1);
            continue;
        }
        budget.consume(text.len());
        shortened_any |= shortened;
        sections.push(WebTextSection {
            kind: section_kind(node.node_name().as_deref()),
            text,
        });
    }
    if sections.is_empty()
        && let Some(body) = document.body()
    {
        let normalized = normalize_whitespace(&body.formatted_text());
        let (text, shortened) =
            bounded_text(&normalized, MAX_WEB_SECTION_BYTES, budget.remaining());
        if !text.is_empty() {
            budget.consume(text.len());
            shortened_any |= shortened;
            sections.push(WebTextSection {
                kind: WebSectionKind::Paragraph,
                text,
            });
        }
    }
    (sections, omitted, shortened_any)
}

fn extract_html_links(
    document: &Document,
    response: &WebBrokerResponse,
    budget: &mut ExtractionBudget,
) -> (Vec<WebDisplayLink>, usize, bool) {
    let mut links = Vec::new();
    let mut omitted = 0usize;
    let mut shortened_any = false;
    let mut seen = BTreeSet::new();
    for node in document.select("a[href]").nodes() {
        let Some(href) = node.attr("href") else {
            continue;
        };
        let label = normalize_whitespace(&node.formatted_text());
        if label.is_empty() {
            continue;
        }
        let Ok(target) = response
            .provenance()
            .canonical_url()
            .resolve_display_target(&href)
        else {
            continue;
        };
        let (label, shortened) = bounded_text(&label, MAX_WEB_LINK_LABEL_BYTES, usize::MAX);
        shortened_any |= shortened;
        if !seen.insert((label.clone(), target.as_str().to_owned())) {
            continue;
        }
        let link_bytes = label.len().saturating_add(target.as_str().len());
        if links.len() >= MAX_WEB_LINKS || link_bytes > budget.remaining() {
            omitted = omitted.saturating_add(1);
            continue;
        }
        budget.consume(link_bytes);
        links.push(WebDisplayLink { label, target });
    }
    (links, omitted, shortened_any)
}

fn extract_json(response: &WebBrokerResponse) -> Result<WebTextExtraction, WebExtractionError> {
    let decoded = decode_utf8(response.untrusted_body())?;
    serde_json::from_str::<serde_json::Value>(decoded)
        .map_err(|_| WebExtractionError::InvalidJson)?;
    Ok(extract_text_response(response, decoded))
}

fn extract_plain_text(
    response: &WebBrokerResponse,
) -> Result<WebTextExtraction, WebExtractionError> {
    let decoded = decode_utf8(response.untrusted_body())?;
    validate_controls(decoded)?;
    Ok(extract_text_response(response, decoded))
}

fn extract_text_response(response: &WebBrokerResponse, decoded: &str) -> WebTextExtraction {
    let (text, shortened) = bounded_text(
        decoded,
        MAX_WEB_TEXT_OUTPUT_BYTES,
        MAX_WEB_TEXT_OUTPUT_BYTES,
    );
    WebTextExtraction {
        source: WebExtractionSource::from_response(response),
        text,
        truncation: WebExtractionTruncation {
            text_shortened: shortened,
            omitted_sections: 0,
            omitted_links: 0,
        },
    }
}

fn decode_utf8(bytes: &[u8]) -> Result<&str, WebExtractionError> {
    let decoded = std::str::from_utf8(bytes)
        .map_err(|_| WebExtractionError::UnsupportedOrMalformedEncoding)?;
    Ok(decoded.strip_prefix('\u{feff}').unwrap_or(decoded))
}

fn validate_controls(text: &str) -> Result<(), WebExtractionError> {
    if text
        .chars()
        .any(|character| character.is_control() && !matches!(character, '\n' | '\r' | '\t'))
    {
        Err(WebExtractionError::InvalidText)
    } else {
        Ok(())
    }
}

fn remove_hidden_nodes(document: &Document) {
    for node in document.root().descendants() {
        let aria_hidden = node
            .attr("aria-hidden")
            .is_some_and(|value| value.eq_ignore_ascii_case("true"));
        let style_hidden = node.attr("style").is_some_and(|value| {
            let compact = value
                .chars()
                .filter(|character| !character.is_ascii_whitespace())
                .collect::<String>()
                .to_ascii_lowercase();
            compact.contains("display:none")
                || compact.contains("visibility:hidden")
                || compact.contains("content-visibility:hidden")
        });
        let class_hidden = node.class().is_some_and(|value| {
            value.split_ascii_whitespace().any(|class| {
                matches!(
                    class.to_ascii_lowercase().as_str(),
                    "hidden" | "visually-hidden" | "sr-only"
                )
            })
        });
        if node.has_attr("hidden") || aria_hidden || style_hidden || class_hidden {
            node.remove_from_parent();
        }
    }
}

fn section_kind(name: Option<&str>) -> WebSectionKind {
    match name {
        Some("h1" | "h2" | "h3" | "h4" | "h5" | "h6") => WebSectionKind::Heading,
        Some("li" | "dt" | "dd") => WebSectionKind::ListItem,
        Some("pre") => WebSectionKind::Preformatted,
        Some("tr") => WebSectionKind::TableRow,
        Some("figcaption") => WebSectionKind::Caption,
        _ => WebSectionKind::Paragraph,
    }
}

fn normalize_whitespace(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn bounded_text(value: &str, per_item_limit: usize, aggregate_limit: usize) -> (String, bool) {
    let limit = per_item_limit.min(aggregate_limit);
    if value.len() <= limit {
        return (value.to_owned(), false);
    }
    let mut boundary = limit;
    while boundary > 0 && !value.is_char_boundary(boundary) {
        boundary -= 1;
    }
    (value[..boundary].to_owned(), true)
}

fn some_if_nonempty(value: String) -> Option<String> {
    if value.is_empty() { None } else { Some(value) }
}

struct ExtractionBudget {
    remaining: usize,
}

impl ExtractionBudget {
    const fn new(maximum: usize) -> Self {
        Self { remaining: maximum }
    }

    const fn remaining(&self) -> usize {
        self.remaining
    }

    fn consume(&mut self, bytes: usize) {
        self.remaining = self.remaining.saturating_sub(bytes);
    }
}

#[cfg(test)]
mod tests {
    use std::fmt::Write as _;

    use sha2::{Digest, Sha256};

    use super::*;

    fn response(
        media_type: ValidatedWebMediaType,
        body: impl Into<Vec<u8>>,
    ) -> Result<WebBrokerResponse, crate::UrlValidationError> {
        Ok(WebBrokerResponse::test_document(
            CanonicalWebUrl::parse("https://public.example.com/root/page?approved=1")?,
            media_type,
            body.into(),
        ))
    }

    #[test]
    fn markdown_is_untrusted_text_with_source_identity_and_truncation()
    -> Result<(), Box<dyn std::error::Error>> {
        let body = format!(
            "# Guide\n<script>not executed</script>\n{}",
            "x".repeat(MAX_WEB_TEXT_OUTPUT_BYTES)
        );
        let response = response(ValidatedWebMediaType::Markdown, body)?;
        assert_eq!(
            response
                .provenance()
                .media_type()
                .map(ValidatedWebMediaType::as_str),
            Some("text/markdown")
        );
        let WebExtraction::PlainText(text) = extract_web_response(&response)? else {
            return Err("expected text".into());
        };
        assert!(text.untrusted_text().starts_with("# Guide"));
        assert!(
            text.untrusted_text()
                .contains("<script>not executed</script>")
        );
        assert!(text.untrusted_text().len() <= MAX_WEB_TEXT_OUTPUT_BYTES);
        assert!(text.truncation().text_shortened());
        assert_eq!(
            text.source().content_sha256(),
            response.provenance().content_sha256()
        );
        assert!(
            extract_web_response(&self::response(ValidatedWebMediaType::Markdown, [0xff])?)
                .is_err()
        );
        Ok(())
    }

    #[test]
    fn html_extraction_removes_active_hidden_control_and_metadata_content()
    -> Result<(), Box<dyn std::error::Error>> {
        let body = br#"<!doctype html><html><head>
            <title>  Useful   page </title>
            <meta name="description" content="unsafe metadata">
            <style>.secret { display:block }</style>
            <script>stealCredentials()</script>
          </head><body>
            <h1>First heading</h1>
            <p>Visible <span hidden>hidden attribute</span>
               <span aria-hidden="true">aria secret</span>
               <span style="DISPLAY: none">style secret</span></p>
            <template>template secret</template><noscript>noscript secret</noscript>
            <button>control secret</button><svg><text>svg secret</text></svg>
            <p>Second paragraph</p>
          </body></html>"#;
        let extraction = extract_web_response(&response(ValidatedWebMediaType::Html, body)?)?;
        let WebExtraction::Html(page) = extraction else {
            return Err("expected HTML extraction".into());
        };

        assert_eq!(page.untrusted_title(), Some("Useful page"));
        assert_eq!(page.sections().len(), 3);
        assert_eq!(page.sections()[0].kind(), WebSectionKind::Heading);
        assert_eq!(page.sections()[0].untrusted_text(), "First heading");
        assert_eq!(page.sections()[1].untrusted_text(), "Visible");
        assert_eq!(page.sections()[2].untrusted_text(), "Second paragraph");
        let rendered = page
            .sections()
            .iter()
            .map(WebTextSection::untrusted_text)
            .collect::<Vec<_>>()
            .join(" ");
        for excluded in [
            "unsafe metadata",
            "stealCredentials",
            "secret",
            "template",
            "noscript",
        ] {
            assert!(!rendered.contains(excluded));
        }
        Ok(())
    }

    #[test]
    fn links_use_the_response_url_not_remote_base_and_are_never_followed()
    -> Result<(), Box<dyn std::error::Error>> {
        let body = br#"<html><head><base href="https://attacker.com/"></head><body>
            <p>Links</p>
            <a href="../next?x=1#fragment"> Next page </a>
            <a href="https://other.example.org/item">External</a>
            <a href="javascript:alert(1)">Active</a>
            <a href="/blank"><span hidden>Hidden label</span></a>
            <a href="../next?x=1#other">Next page</a>
          </body></html>"#;
        let extraction = extract_web_response(&response(ValidatedWebMediaType::Html, body)?)?;
        let WebExtraction::Html(page) = extraction else {
            return Err("expected HTML extraction".into());
        };

        assert_eq!(page.links().len(), 2);
        assert_eq!(page.links()[0].untrusted_label(), "Next page");
        assert_eq!(
            page.links()[0].target().as_str(),
            "https://public.example.com/next?x=1"
        );
        assert_eq!(page.links()[1].untrusted_label(), "External");
        assert_eq!(
            page.links()[1].target().as_str(),
            "https://other.example.org/item"
        );
        Ok(())
    }

    #[test]
    fn section_link_and_multibyte_truncation_is_explicit_and_deterministic()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut html = String::from("<html><body>");
        write!(html, "<p>{}</p>", "é".repeat(MAX_WEB_SECTION_BYTES))?;
        for index in 0..=MAX_WEB_SECTIONS {
            write!(html, "<p>section {index}</p>")?;
        }
        for index in 0..=MAX_WEB_LINKS {
            write!(html, "<a href=\"/link/{index}\">link {index}</a>")?;
        }
        html.push_str("</body></html>");

        let extraction = extract_web_response(&response(ValidatedWebMediaType::Html, html)?)?;
        let WebExtraction::Html(page) = extraction else {
            return Err("expected HTML extraction".into());
        };
        assert_eq!(page.sections().len(), MAX_WEB_SECTIONS);
        assert_eq!(page.links().len(), MAX_WEB_LINKS);
        assert!(
            page.sections()[0]
                .untrusted_text()
                .is_char_boundary(page.sections()[0].untrusted_text().len())
        );
        assert!(page.truncation().text_shortened());
        assert!(page.truncation().omitted_sections() >= 2);
        assert_eq!(page.truncation().omitted_links(), 1);
        assert!(page.truncation().is_truncated());
        Ok(())
    }

    #[test]
    fn malformed_encoding_controls_and_excessive_markup_fail_closed()
    -> Result<(), Box<dyn std::error::Error>> {
        assert_eq!(
            extract_web_response(&response(
                ValidatedWebMediaType::PlainText,
                vec![0xff, 0xfe]
            )?)
            .err(),
            Some(WebExtractionError::UnsupportedOrMalformedEncoding)
        );
        assert_eq!(
            extract_web_response(&response(
                ValidatedWebMediaType::PlainText,
                b"safe\x07unsafe".to_vec()
            )?)
            .err(),
            Some(WebExtractionError::InvalidText)
        );
        let markup = "<".repeat(MAX_HTML_MARKUP_TOKENS + 1);
        assert_eq!(
            extract_web_response(&response(ValidatedWebMediaType::Html, markup)?).err(),
            Some(WebExtractionError::HtmlComplexityLimit)
        );
        Ok(())
    }

    #[test]
    fn json_and_plain_text_use_separate_bounded_extractors()
    -> Result<(), Box<dyn std::error::Error>> {
        let json_body = br#"{"z":1,"a":[2,3]}"#;
        let json_response = response(ValidatedWebMediaType::Json, json_body)?;
        let extraction = extract_web_response(&json_response)?;
        let WebExtraction::Json(json) = extraction else {
            return Err("expected JSON extraction".into());
        };
        assert_eq!(json.untrusted_text(), "{\"z\":1,\"a\":[2,3]}");
        assert_eq!(
            json.source().content_sha256(),
            format!("{:x}", Sha256::digest(json_body)).as_str()
        );
        assert_eq!(
            extract_web_response(&response(
                ValidatedWebMediaType::Json,
                b"{broken".as_slice()
            )?)
            .err(),
            Some(WebExtractionError::InvalidJson)
        );

        let plain_body = "x".repeat(MAX_WEB_TEXT_OUTPUT_BYTES + 1);
        let extraction =
            extract_web_response(&response(ValidatedWebMediaType::PlainText, plain_body)?)?;
        let WebExtraction::PlainText(text) = extraction else {
            return Err("expected plain-text extraction".into());
        };
        assert_eq!(text.untrusted_text().len(), MAX_WEB_TEXT_OUTPUT_BYTES);
        assert!(text.truncation().text_shortened());
        Ok(())
    }

    #[test]
    fn media_bytes_do_not_enter_the_text_extractor() -> Result<(), Box<dyn std::error::Error>> {
        assert_eq!(
            extract_web_response(&response(
                ValidatedWebMediaType::Png,
                b"\x89PNG\r\n\x1a\n".as_slice()
            )?)
            .err(),
            Some(WebExtractionError::UnsupportedMediaType)
        );
        Ok(())
    }
}
