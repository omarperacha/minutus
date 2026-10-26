//! Shared, non-authoritative projection of bounded remote content and its limits.

use serde_json::{Value, json};
use web_retrieval::WebExtraction;

/// Projects untrusted content without dropping source identity or extraction omissions.
/// Completeness describes extraction limits, not semantic truth or merchant fulfilment.
#[must_use]
pub fn project_web_extraction(extraction: &WebExtraction) -> Value {
    let mut value = match extraction {
        WebExtraction::Html(page) => json!({
            "type": "html",
            "title": page.untrusted_title(),
            "sections": page.sections().iter().map(|section| json!({
                "kind": section_kind_name(section.kind()),
                "text": section.untrusted_text()
            })).collect::<Vec<_>>(),
            "links": page.links().iter().map(|link| json!({
                "label": link.untrusted_label(),
                "target": link.target().as_str()
            })).collect::<Vec<_>>(),
            "truncation": truncation_json(page.truncation())
        }),
        WebExtraction::Json(text) => json!({
            "type": "json",
            "text": text.untrusted_text(),
            "truncation": truncation_json(text.truncation())
        }),
        WebExtraction::PlainText(text) => json!({
            "type": "plainText",
            "text": text.untrusted_text(),
            "truncation": truncation_json(text.truncation())
        }),
    };
    let (source, truncation) = match extraction {
        WebExtraction::Html(page) => (page.source(), page.truncation()),
        WebExtraction::Json(text) | WebExtraction::PlainText(text) => {
            (text.source(), text.truncation())
        }
    };
    value["contentComplete"] = json!(!truncation.is_truncated());
    value["provenance"] = json!({
        "canonicalUrl": source.canonical_url().as_str(),
        "contentSha256": source.content_sha256(),
        "retrievedAtUnixMillis": source.retrieved_at_unix_millis().to_string(),
    });
    value
}

fn truncation_json(truncation: web_retrieval::WebExtractionTruncation) -> Value {
    json!({
        "textShortened": truncation.text_shortened(),
        "omittedSections": truncation.omitted_sections(),
        "omittedLinks": truncation.omitted_links()
    })
}

fn section_kind_name(kind: web_retrieval::WebSectionKind) -> &'static str {
    match kind {
        web_retrieval::WebSectionKind::Heading => "heading",
        web_retrieval::WebSectionKind::Paragraph => "paragraph",
        web_retrieval::WebSectionKind::ListItem => "listItem",
        web_retrieval::WebSectionKind::Preformatted => "preformatted",
        web_retrieval::WebSectionKind::TableRow => "tableRow",
        web_retrieval::WebSectionKind::Caption => "caption",
    }
}

#[cfg(test)]
mod tests {
    use reqwest::{
        StatusCode,
        header::{CONTENT_TYPE, HeaderMap, HeaderValue},
    };
    use web_retrieval::{
        CanonicalWebUrl, WebMethod, extract_web_response, test_support::validated_response,
    };

    #[test]
    fn bounded_json_keeps_truncation_type_and_source() -> Result<(), Box<dyn std::error::Error>> {
        for size in [10, 40_000] {
            let body = serde_json::to_vec(&serde_json::json!({"report": "x".repeat(size)}))?;
            let mut headers = HeaderMap::new();
            headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
            let response = validated_response(
                WebMethod::Get,
                CanonicalWebUrl::parse("https://example.com/report")?,
                StatusCode::OK,
                32,
                &headers,
                &body,
            )?;
            let projection = super::project_web_extraction(&extract_web_response(&response)?);
            assert_eq!(projection["type"], "json");
            assert_eq!(projection["contentComplete"], size < 32_768);
            assert_eq!(projection["truncation"]["textShortened"], size > 32_768);
            assert_eq!(
                projection["provenance"]["canonicalUrl"],
                "https://example.com/report"
            );
            assert_eq!(
                projection["provenance"]["contentSha256"],
                response.provenance().content_sha256()
            );
            let text = projection["text"].as_str().ok_or("missing text")?;
            assert_eq!(
                serde_json::from_str::<serde_json::Value>(text).is_ok(),
                size < 32_768
            );
        }
        Ok(())
    }
}
