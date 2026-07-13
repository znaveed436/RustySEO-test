//! Automated accessibility audit for static HTML.
//!
//! This module intentionally reports only issues that can be detected from the
//! supplied HTML. It does not claim full WCAG conformance because colour
//! contrast, keyboard interaction, focus management, screen-reader behaviour,
//! and JavaScript state generally require rendered/manual testing.

use regex::Regex;
use scraper::{ElementRef, Html, Selector};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

const MAX_HTML_BYTES: usize = 10 * 1024 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct A11yAuditInput {
    pub page_url: String,
    pub html: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum A11ySeverity {
    Critical,
    High,
    Medium,
    Low,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct A11yIssue {
    pub code: String,
    pub category: String,
    pub severity: A11ySeverity,
    pub message: String,
    pub element: String,
    pub evidence: String,
    pub recommendation: String,
    pub wcag_references: Vec<String>,
    pub requires_manual_review: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct A11ySummary {
    pub total_issues: usize,
    pub critical: usize,
    pub high: usize,
    pub medium: usize,
    pub low: usize,
    pub manual_review_items: usize,
    pub automated_score: u8,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct A11yAuditReport {
    pub page_url: String,
    pub document_title: Option<String>,
    pub document_language: Option<String>,
    pub summary: A11ySummary,
    pub issues: Vec<A11yIssue>,
    pub checks_performed: Vec<String>,
    pub limitations: Vec<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
struct A11yCsvRow<'a> {
    page_url: &'a str,
    severity: &'a str,
    code: &'a str,
    category: &'a str,
    message: &'a str,
    element: &'a str,
    evidence: &'a str,
    recommendation: &'a str,
    wcag_references: String,
    requires_manual_review: bool,
}

#[tauri::command]
pub async fn run_a11y_audit_command(
    page_url: String,
    html: String,
) -> Result<A11yAuditReport, String> {
    if page_url.trim().is_empty() {
        return Err("pageUrl cannot be empty".to_string());
    }
    if html.trim().is_empty() {
        return Err("html cannot be empty".to_string());
    }
    if html.len() > MAX_HTML_BYTES {
        return Err(format!(
            "HTML exceeds the {} MB audit limit",
            MAX_HTML_BYTES / 1024 / 1024
        ));
    }

    tokio::task::spawn_blocking(move || audit_html(A11yAuditInput { page_url, html }))
        .await
        .map_err(|error| format!("Accessibility audit task failed: {error}"))?
}

#[tauri::command]
pub fn export_a11y_audit_csv_command(audit_result: A11yAuditReport) -> Result<String, String> {
    let mut writer = csv::WriterBuilder::new()
        .has_headers(true)
        .from_writer(Vec::<u8>::new());

    for issue in &audit_result.issues {
        writer
            .serialize(A11yCsvRow {
                page_url: &audit_result.page_url,
                severity: severity_label(issue.severity),
                code: &issue.code,
                category: &issue.category,
                message: &issue.message,
                element: &issue.element,
                evidence: &issue.evidence,
                recommendation: &issue.recommendation,
                wcag_references: issue.wcag_references.join("; "),
                requires_manual_review: issue.requires_manual_review,
            })
            .map_err(|error| format!("Failed to serialise accessibility CSV: {error}"))?;
    }

    writer
        .flush()
        .map_err(|error| format!("Failed to finish accessibility CSV: {error}"))?;
    let bytes = writer
        .into_inner()
        .map_err(|error| format!("Failed to build accessibility CSV: {}", error.into_error()))?;

    String::from_utf8(bytes).map_err(|error| format!("CSV was not valid UTF-8: {error}"))
}

fn audit_html(input: A11yAuditInput) -> Result<A11yAuditReport, String> {
    let document = Html::parse_document(&input.html);
    let mut issues = Vec::new();

    let document_title = first_text(&document, "title")?;
    let document_language = first_attribute(&document, "html", "lang")?;

    audit_document_metadata(
        &document,
        document_title.as_deref(),
        document_language.as_deref(),
        &mut issues,
    )?;
    audit_images(&document, &mut issues)?;
    audit_headings(&document, &mut issues)?;
    audit_links(&document, &mut issues)?;
    audit_buttons(&document, &mut issues)?;
    audit_forms(&document, &mut issues)?;
    audit_iframes(&document, &mut issues)?;
    audit_tables(&document, &mut issues)?;
    audit_landmarks(&document, &mut issues)?;
    audit_duplicate_ids(&document, &mut issues)?;

    issues.sort_by(|left, right| {
        severity_rank(left.severity)
            .cmp(&severity_rank(right.severity))
            .then_with(|| left.code.cmp(&right.code))
            .then_with(|| left.element.cmp(&right.element))
    });

    let summary = summarise(&issues);

    Ok(A11yAuditReport {
        page_url: input.page_url,
        document_title,
        document_language,
        summary,
        issues,
        checks_performed: vec![
            "Document title and language".to_string(),
            "Image alternative text".to_string(),
            "Heading structure".to_string(),
            "Link and button accessible names".to_string(),
            "Form labels".to_string(),
            "Iframe titles".to_string(),
            "Table headers and captions".to_string(),
            "Main landmarks".to_string(),
            "Duplicate element IDs".to_string(),
        ],
        limitations: vec![
            "The score is an automated prioritisation score, not a WCAG conformance result."
                .to_string(),
            "Rendered colour contrast, keyboard navigation, focus order, focus visibility, motion, audio descriptions and screen-reader behaviour require additional testing."
                .to_string(),
            "JavaScript-generated markup is only checked when it is included in the supplied HTML."
                .to_string(),
        ],
    })
}

fn audit_document_metadata(
    document: &Html,
    title: Option<&str>,
    language: Option<&str>,
    issues: &mut Vec<A11yIssue>,
) -> Result<(), String> {
    match title.map(str::trim).filter(|value| !value.is_empty()) {
        None => push_issue(
            issues,
            "document-title-missing",
            "Document",
            A11ySeverity::High,
            "The document does not have a non-empty title.",
            "head > title",
            "No usable <title> element was found.",
            "Add a concise, unique title that identifies the page purpose.",
            &["2.4.2"],
            false,
        ),
        Some(value) if value.chars().count() > 120 => push_issue(
            issues,
            "document-title-long",
            "Document",
            A11ySeverity::Low,
            "The document title is unusually long and may be difficult to understand quickly.",
            "head > title",
            &truncate(value, 180),
            "Shorten the title while keeping the page purpose clear and unique.",
            &["2.4.2"],
            true,
        ),
        _ => {}
    }

    let language_regex = Regex::new(r"^[A-Za-z]{2,3}(?:-[A-Za-z0-9]{2,8})*$")
        .map_err(|error| format!("Failed to create language validator: {error}"))?;
    match language.map(str::trim).filter(|value| !value.is_empty()) {
        None => push_issue(
            issues,
            "html-lang-missing",
            "Document",
            A11ySeverity::High,
            "The root HTML element has no language declaration.",
            "html",
            "Missing lang attribute.",
            "Add the page's primary BCP 47 language code, for example lang=\"en\" or lang=\"en-GB\".",
            &["3.1.1"],
            false,
        ),
        Some(value) if !language_regex.is_match(value) => push_issue(
            issues,
            "html-lang-invalid",
            "Document",
            A11ySeverity::Medium,
            "The root language declaration does not look like a valid BCP 47 language tag.",
            "html",
            value,
            "Use a valid language code such as en, en-GB, fr or de-DE.",
            &["3.1.1"],
            false,
        ),
        _ => {}
    }

    let meta_refresh_selector = selector("meta[http-equiv]")?;
    for element in document.select(&meta_refresh_selector) {
        let refresh = element
            .value()
            .attr("http-equiv")
            .map(|value| value.eq_ignore_ascii_case("refresh"))
            .unwrap_or(false);
        if refresh {
            push_issue(
                issues,
                "meta-refresh",
                "Timing",
                A11ySeverity::High,
                "The page uses a meta refresh, which can unexpectedly move or reload content.",
                &element_locator(&element),
                &element_snippet(&element),
                "Remove automatic meta refresh behaviour. Use a server-side redirect when redirection is required.",
                &["2.2.1", "3.2.5"],
                false,
            );
        }
    }

    Ok(())
}

fn audit_images(document: &Html, issues: &mut Vec<A11yIssue>) -> Result<(), String> {
    let image_selector = selector("img")?;

    for image in document.select(&image_selector) {
        let locator = element_locator(&image);
        let src = image.value().attr("src").unwrap_or("");
        let alt = image.value().attr("alt");
        let is_linked = has_ancestor_tag(&image, "a");
        let role = image.value().attr("role").unwrap_or("");
        let aria_hidden = image
            .value()
            .attr("aria-hidden")
            .map(|value| value.eq_ignore_ascii_case("true"))
            .unwrap_or(false);

        match alt {
            None if !aria_hidden && !role.eq_ignore_ascii_case("presentation") => push_issue(
                issues,
                "image-alt-missing",
                "Images",
                A11ySeverity::High,
                "An image is missing an alt attribute.",
                &locator,
                src,
                "Add meaningful alternative text, or alt=\"\" when the image is purely decorative.",
                &["1.1.1"],
                false,
            ),
            Some(value) if value.trim().is_empty() && is_linked => push_issue(
                issues,
                "linked-image-empty-alt",
                "Images",
                A11ySeverity::High,
                "A linked image has empty alternative text and may leave the link without an accessible name.",
                &locator,
                src,
                "Give the image or its parent link an accessible name that explains the link destination.",
                &["1.1.1", "2.4.4", "4.1.2"],
                false,
            ),
            Some(value) if looks_like_filename(value) => push_issue(
                issues,
                "image-alt-filename",
                "Images",
                A11ySeverity::Medium,
                "Image alternative text appears to be a filename rather than a useful description.",
                &locator,
                value,
                "Replace the filename with concise text that communicates the image's purpose in context.",
                &["1.1.1"],
                false,
            ),
            Some(value) if value.chars().count() > 200 => push_issue(
                issues,
                "image-alt-long",
                "Images",
                A11ySeverity::Low,
                "Image alternative text is unusually long.",
                &locator,
                &truncate(value, 220),
                "Keep alt text concise. Put extended descriptions in nearby visible content when needed.",
                &["1.1.1"],
                true,
            ),
            _ => {}
        }
    }

    Ok(())
}

fn audit_headings(document: &Html, issues: &mut Vec<A11yIssue>) -> Result<(), String> {
    let heading_selector = selector("h1, h2, h3, h4, h5, h6")?;
    let headings: Vec<ElementRef<'_>> = document.select(&heading_selector).collect();

    if headings.is_empty() {
        push_issue(
            issues,
            "headings-missing",
            "Structure",
            A11ySeverity::Medium,
            "The page has no semantic headings.",
            "body",
            "No h1-h6 elements found.",
            "Use descriptive headings to organise page content into a meaningful hierarchy.",
            &["1.3.1", "2.4.6"],
            true,
        );
        return Ok(());
    }

    let h1_count = headings
        .iter()
        .filter(|heading| heading.value().name() == "h1")
        .count();
    if h1_count == 0 {
        push_issue(
            issues,
            "h1-missing",
            "Structure",
            A11ySeverity::Medium,
            "The page has headings but no level-one heading.",
            "body",
            "No <h1> element found.",
            "Add a clear level-one heading that identifies the main page topic.",
            &["1.3.1", "2.4.6"],
            true,
        );
    }

    let mut previous_level: Option<u8> = None;
    for heading in headings {
        let level = heading_level(&heading).unwrap_or(0);
        let text = normalised_text(&heading);

        if text.is_empty() {
            push_issue(
                issues,
                "heading-empty",
                "Structure",
                A11ySeverity::High,
                "A heading has no accessible text.",
                &element_locator(&heading),
                &element_snippet(&heading),
                "Add descriptive heading text or remove the empty heading element.",
                &["1.3.1", "2.4.6"],
                false,
            );
        }

        if let Some(previous) = previous_level {
            if level > previous.saturating_add(1) {
                push_issue(
                    issues,
                    "heading-level-skip",
                    "Structure",
                    A11ySeverity::Medium,
                    "The heading hierarchy skips one or more levels.",
                    &element_locator(&heading),
                    &format!("Previous level: h{previous}; current level: h{level}; text: {text}"),
                    "Use heading levels to represent nested structure rather than visual size.",
                    &["1.3.1", "2.4.6"],
                    true,
                );
            }
        }
        previous_level = Some(level);
    }

    Ok(())
}

fn audit_links(document: &Html, issues: &mut Vec<A11yIssue>) -> Result<(), String> {
    let link_selector = selector("a[href]")?;
    let generic_texts: HashSet<&'static str> = [
        "click here",
        "here",
        "read more",
        "more",
        "learn more",
        "link",
        "continue",
    ]
    .into_iter()
    .collect();

    for link in document.select(&link_selector) {
        let name = accessible_name(&link);
        let href = link.value().attr("href").unwrap_or("");
        let locator = element_locator(&link);

        if name.is_empty() {
            push_issue(
                issues,
                "link-name-missing",
                "Links",
                A11ySeverity::High,
                "A link has no accessible name.",
                &locator,
                href,
                "Add descriptive visible text or an appropriate aria-label.",
                &["2.4.4", "4.1.2"],
                false,
            );
        } else if generic_texts.contains(name.to_lowercase().as_str()) {
            push_issue(
                issues,
                "link-text-generic",
                "Links",
                A11ySeverity::Medium,
                "Link text is generic and may not explain its destination out of context.",
                &locator,
                &format!("Text: {name}; href: {href}"),
                "Use link text that describes the destination or action.",
                &["2.4.4"],
                true,
            );
        }
    }

    Ok(())
}

fn audit_buttons(document: &Html, issues: &mut Vec<A11yIssue>) -> Result<(), String> {
    let button_selector = selector("button, [role='button']")?;

    for button in document.select(&button_selector) {
        if accessible_name(&button).is_empty() {
            push_issue(
                issues,
                "button-name-missing",
                "Controls",
                A11ySeverity::High,
                "A button or button-role element has no accessible name.",
                &element_locator(&button),
                &element_snippet(&button),
                "Add descriptive visible text, aria-label, or aria-labelledby.",
                &["4.1.2"],
                false,
            );
        }
    }

    Ok(())
}

fn audit_forms(document: &Html, issues: &mut Vec<A11yIssue>) -> Result<(), String> {
    let label_selector = selector("label[for]")?;
    let labelled_ids: HashSet<String> = document
        .select(&label_selector)
        .filter_map(|label| label.value().attr("for"))
        .map(str::to_string)
        .collect();

    let control_selector = selector("input, select, textarea")?;
    for control in document.select(&control_selector) {
        let control_type = control.value().attr("type").unwrap_or("text");
        if matches!(
            control_type.to_ascii_lowercase().as_str(),
            "hidden" | "submit" | "reset" | "button" | "image"
        ) {
            continue;
        }

        let id = control.value().attr("id");
        let has_label_for = id
            .map(|value| labelled_ids.contains(value))
            .unwrap_or(false);
        let is_wrapped_by_label = has_ancestor_tag(&control, "label");
        let has_aria_name = non_empty_attr(&control, "aria-label")
            || non_empty_attr(&control, "aria-labelledby")
            || non_empty_attr(&control, "title");

        if !has_label_for && !is_wrapped_by_label && !has_aria_name {
            push_issue(
                issues,
                "form-label-missing",
                "Forms",
                A11ySeverity::High,
                "A form control does not have a detectable accessible label.",
                &element_locator(&control),
                &element_snippet(&control),
                "Associate a visible <label> using for/id, wrap the control in a label, or provide an appropriate accessible name.",
                &["1.3.1", "3.3.2", "4.1.2"],
                false,
            );
        }
    }

    Ok(())
}

fn audit_iframes(document: &Html, issues: &mut Vec<A11yIssue>) -> Result<(), String> {
    let iframe_selector = selector("iframe")?;
    for iframe in document.select(&iframe_selector) {
        if !non_empty_attr(&iframe, "title") && !non_empty_attr(&iframe, "aria-label") {
            push_issue(
                issues,
                "iframe-title-missing",
                "Embedded content",
                A11ySeverity::High,
                "An iframe does not have a descriptive title.",
                &element_locator(&iframe),
                iframe.value().attr("src").unwrap_or(""),
                "Add a concise title describing the iframe content or purpose.",
                &["2.4.1", "4.1.2"],
                false,
            );
        }
    }
    Ok(())
}

fn audit_tables(document: &Html, issues: &mut Vec<A11yIssue>) -> Result<(), String> {
    let table_selector = selector("table")?;
    let th_selector = selector("th")?;
    let caption_selector = selector("caption")?;

    for table in document.select(&table_selector) {
        let has_headers = table.select(&th_selector).next().is_some();
        let has_caption = table.select(&caption_selector).next().is_some();

        if !has_headers {
            push_issue(
                issues,
                "table-headers-missing",
                "Tables",
                A11ySeverity::High,
                "A data table has no header cells.",
                &element_locator(&table),
                &element_snippet(&table),
                "Use <th> cells and scope attributes to identify row and column headers. Use CSS rather than table markup for layout.",
                &["1.3.1"],
                true,
            );
        }
        if !has_caption {
            push_issue(
                issues,
                "table-caption-missing",
                "Tables",
                A11ySeverity::Low,
                "A table has no caption.",
                &element_locator(&table),
                &element_snippet(&table),
                "Add a concise <caption> when users need context to understand the table.",
                &["1.3.1"],
                true,
            );
        }
    }

    Ok(())
}

fn audit_landmarks(document: &Html, issues: &mut Vec<A11yIssue>) -> Result<(), String> {
    let main_selector = selector("main, [role='main']")?;
    let main_count = document.select(&main_selector).count();

    if main_count == 0 {
        push_issue(
            issues,
            "main-landmark-missing",
            "Landmarks",
            A11ySeverity::Medium,
            "The page has no detectable main landmark.",
            "body",
            "No <main> or role=\"main\" found.",
            "Wrap the primary page content in one <main> landmark.",
            &["1.3.1", "2.4.1"],
            true,
        );
    } else if main_count > 1 {
        push_issue(
            issues,
            "multiple-main-landmarks",
            "Landmarks",
            A11ySeverity::Medium,
            "The page has more than one main landmark.",
            "body",
            &format!("Detected {main_count} main landmarks."),
            "Use one non-hidden main landmark for the page's primary content.",
            &["1.3.1", "2.4.1"],
            true,
        );
    }

    Ok(())
}

fn audit_duplicate_ids(document: &Html, issues: &mut Vec<A11yIssue>) -> Result<(), String> {
    let id_selector = selector("[id]")?;
    let mut occurrences: HashMap<String, Vec<String>> = HashMap::new();

    for element in document.select(&id_selector) {
        if let Some(id) = element.value().attr("id").map(str::trim) {
            if !id.is_empty() {
                occurrences
                    .entry(id.to_string())
                    .or_default()
                    .push(element_locator(&element));
            }
        }
    }

    for (id, elements) in occurrences {
        if elements.len() > 1 {
            push_issue(
                issues,
                "duplicate-id",
                "Parsing",
                A11ySeverity::High,
                "Multiple elements use the same ID.",
                &elements.join(", "),
                &format!("Duplicate id=\"{id}\" found {} times.", elements.len()),
                "Make every non-empty ID unique so labels and ARIA references resolve predictably.",
                &["1.3.1", "4.1.2"],
                false,
            );
        }
    }

    Ok(())
}

fn first_text(document: &Html, css: &str) -> Result<Option<String>, String> {
    let selector = selector(css)?;
    Ok(document
        .select(&selector)
        .next()
        .map(|element| normalised_text(&element))
        .filter(|value| !value.is_empty()))
}

fn first_attribute(document: &Html, css: &str, attribute: &str) -> Result<Option<String>, String> {
    let selector = selector(css)?;
    Ok(document
        .select(&selector)
        .next()
        .and_then(|element| element.value().attr(attribute))
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string))
}

fn selector(css: &str) -> Result<Selector, String> {
    Selector::parse(css).map_err(|error| format!("Invalid internal selector '{css}': {error:?}"))
}

fn normalised_text(element: &ElementRef<'_>) -> String {
    element
        .text()
        .flat_map(str::split_whitespace)
        .collect::<Vec<_>>()
        .join(" ")
        .trim()
        .to_string()
}

fn accessible_name(element: &ElementRef<'_>) -> String {
    for attribute in ["aria-label", "title"] {
        if let Some(value) = element.value().attr(attribute).map(str::trim) {
            if !value.is_empty() {
                return value.to_string();
            }
        }
    }

    if let Some(value) = element.value().attr("aria-labelledby").map(str::trim) {
        if !value.is_empty() {
            return format!("aria-labelledby:{value}");
        }
    }

    let text = normalised_text(element);
    if !text.is_empty() {
        return text;
    }

    let image_selector = match Selector::parse("img[alt]") {
        Ok(selector) => selector,
        Err(_) => return String::new(),
    };
    element
        .select(&image_selector)
        .filter_map(|image| image.value().attr("alt"))
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

fn non_empty_attr(element: &ElementRef<'_>, attribute: &str) -> bool {
    element
        .value()
        .attr(attribute)
        .map(str::trim)
        .map(|value| !value.is_empty())
        .unwrap_or(false)
}

fn heading_level(element: &ElementRef<'_>) -> Option<u8> {
    element
        .value()
        .name()
        .strip_prefix('h')
        .and_then(|level| level.parse::<u8>().ok())
        .filter(|level| (1..=6).contains(level))
}

fn has_ancestor_tag(element: &ElementRef<'_>, tag_name: &str) -> bool {
    element.ancestors().skip(1).any(|node| {
        ElementRef::wrap(node)
            .map(|ancestor| ancestor.value().name().eq_ignore_ascii_case(tag_name))
            .unwrap_or(false)
    })
}

fn element_locator(element: &ElementRef<'_>) -> String {
    let tag = element.value().name();
    if let Some(id) = element.value().attr("id").map(str::trim) {
        if !id.is_empty() {
            return format!("{tag}#{id}");
        }
    }

    let classes = element
        .value()
        .attr("class")
        .unwrap_or("")
        .split_whitespace()
        .take(2)
        .collect::<Vec<_>>();
    if classes.is_empty() {
        tag.to_string()
    } else {
        format!("{tag}.{}", classes.join("."))
    }
}

fn element_snippet(element: &ElementRef<'_>) -> String {
    truncate(&element.html(), 300)
}

fn looks_like_filename(value: &str) -> bool {
    let trimmed = value.trim().to_ascii_lowercase();
    [".jpg", ".jpeg", ".png", ".gif", ".webp", ".svg", ".avif"]
        .iter()
        .any(|extension| trimmed.ends_with(extension))
        || (trimmed.contains('_') && !trimmed.contains(' '))
}

fn truncate(value: &str, max_chars: usize) -> String {
    let mut chars = value.chars();
    let truncated: String = chars.by_ref().take(max_chars).collect();
    if chars.next().is_some() {
        format!("{truncated}…")
    } else {
        truncated
    }
}

#[allow(clippy::too_many_arguments)]
fn push_issue(
    issues: &mut Vec<A11yIssue>,
    code: &str,
    category: &str,
    severity: A11ySeverity,
    message: &str,
    element: &str,
    evidence: &str,
    recommendation: &str,
    wcag_references: &[&str],
    requires_manual_review: bool,
) {
    issues.push(A11yIssue {
        code: code.to_string(),
        category: category.to_string(),
        severity,
        message: message.to_string(),
        element: element.to_string(),
        evidence: truncate(evidence, 500),
        recommendation: recommendation.to_string(),
        wcag_references: wcag_references.iter().map(|value| value.to_string()).collect(),
        requires_manual_review,
    });
}

fn summarise(issues: &[A11yIssue]) -> A11ySummary {
    let mut summary = A11ySummary::default();
    let mut penalty = 0_u32;

    for issue in issues {
        summary.total_issues += 1;
        if issue.requires_manual_review {
            summary.manual_review_items += 1;
        }
        match issue.severity {
            A11ySeverity::Critical => {
                summary.critical += 1;
                penalty = penalty.saturating_add(25);
            }
            A11ySeverity::High => {
                summary.high += 1;
                penalty = penalty.saturating_add(12);
            }
            A11ySeverity::Medium => {
                summary.medium += 1;
                penalty = penalty.saturating_add(6);
            }
            A11ySeverity::Low => {
                summary.low += 1;
                penalty = penalty.saturating_add(2);
            }
        }
    }

    summary.automated_score = 100_u8.saturating_sub(penalty.min(100) as u8);
    summary
}

fn severity_rank(severity: A11ySeverity) -> u8 {
    match severity {
        A11ySeverity::Critical => 0,
        A11ySeverity::High => 1,
        A11ySeverity::Medium => 2,
        A11ySeverity::Low => 3,
    }
}

fn severity_label(severity: A11ySeverity) -> &'static str {
    match severity {
        A11ySeverity::Critical => "critical",
        A11ySeverity::High => "high",
        A11ySeverity::Medium => "medium",
        A11ySeverity::Low => "low",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_core_accessibility_issues() {
        let report = audit_html(A11yAuditInput {
            page_url: "https://example.com".to_string(),
            html: r#"
                <html>
                  <head><title></title></head>
                  <body>
                    <h1>Page</h1><h3>Skipped heading</h3>
                    <a href="/empty"></a>
                    <img src="hero.jpg">
                    <input id="email">
                    <div id="duplicate"></div><div id="duplicate"></div>
                  </body>
                </html>
            "#
            .to_string(),
        })
        .expect("audit should succeed");

        let codes: HashSet<&str> = report.issues.iter().map(|issue| issue.code.as_str()).collect();
        assert!(codes.contains("document-title-missing"));
        assert!(codes.contains("html-lang-missing"));
        assert!(codes.contains("heading-level-skip"));
        assert!(codes.contains("link-name-missing"));
        assert!(codes.contains("image-alt-missing"));
        assert!(codes.contains("form-label-missing"));
        assert!(codes.contains("duplicate-id"));
    }

    #[test]
    fn accepts_a_reasonable_document() {
        let report = audit_html(A11yAuditInput {
            page_url: "https://example.com".to_string(),
            html: r#"
                <html lang="en">
                  <head><title>Example page</title></head>
                  <body>
                    <main>
                      <h1>Example page</h1>
                      <img src="team.jpg" alt="The customer support team">
                      <a href="/contact">Contact our team</a>
                      <label for="email">Email</label><input id="email" type="email">
                    </main>
                  </body>
                </html>
            "#
            .to_string(),
        })
        .expect("audit should succeed");

        assert_eq!(report.summary.high, 0);
    }
}
