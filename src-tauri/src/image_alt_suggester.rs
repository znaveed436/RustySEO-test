//! Gemini-powered image alternative-text suggestions.
//!
//! Images are downloaded with strict limits, validated, and sent to Gemini as
//! inline image data. API credentials are read from RustySEO's existing Gemini
//! configuration; no API key is embedded in source code.

use base64::{engine::general_purpose::STANDARD as BASE64_STANDARD, Engine as _};
use futures::stream::{self, StreamExt};
use image::GenericImageView;
use reqwest::{redirect::Policy, Client, StatusCode};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::net::IpAddr;
use std::time::Duration;
use url::Url;

const MAX_IMAGES_PER_REQUEST: usize = 50;
const MAX_IMAGE_BYTES: usize = 10 * 1024 * 1024;
const MAX_CONCURRENCY: usize = 3;
const DEFAULT_VISION_MODEL: &str = "gemini-2.5-flash-lite";
const GEMINI_BASE_URL: &str = "https://generativelanguage.googleapis.com/v1beta/models";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ImageAltInput {
    Url(String),
    Detailed(ImageData),
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ImageData {
    #[serde(alias = "imageUrl")]
    pub url: String,
    #[serde(default)]
    pub current_alt: Option<String>,
    #[serde(default)]
    pub page_title: Option<String>,
    #[serde(default)]
    pub surrounding_text: Option<String>,
    #[serde(default)]
    pub link_destination: Option<String>,
    #[serde(default)]
    pub decorative: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AltSuggestionStatus {
    Suggested,
    KeepCurrent,
    Decorative,
    ReviewRequired,
    Error,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AltSuggestion {
    pub image_url: String,
    pub current_alt: Option<String>,
    pub suggested_alt: String,
    pub confidence: f64,
    pub reasoning: String,
    pub visible_text: Option<String>,
    pub is_decorative: bool,
    pub status: AltSuggestionStatus,
    pub mime_type: Option<String>,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub model: Option<String>,
    pub error: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GeminiAltPayload {
    alt_text: String,
    is_decorative: bool,
    confidence: f64,
    reasoning: String,
    #[serde(default)]
    visible_text: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GeminiResponse {
    #[serde(default)]
    candidates: Vec<GeminiCandidate>,
    #[serde(default)]
    prompt_feedback: Option<GeminiPromptFeedback>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GeminiCandidate {
    content: Option<GeminiContent>,
    #[serde(default)]
    finish_reason: Option<String>,
}

#[derive(Debug, Deserialize)]
struct GeminiContent {
    #[serde(default)]
    parts: Vec<GeminiPart>,
}

#[derive(Debug, Deserialize)]
struct GeminiPart {
    #[serde(default)]
    text: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GeminiPromptFeedback {
    #[serde(default)]
    block_reason: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
struct AltCsvRow<'a> {
    image_url: &'a str,
    current_alt: &'a str,
    suggested_alt: &'a str,
    confidence: f64,
    status: &'a str,
    decorative: bool,
    visible_text: &'a str,
    reasoning: &'a str,
    mime_type: &'a str,
    width: Option<u32>,
    height: Option<u32>,
    model: &'a str,
    error: &'a str,
}

#[derive(Debug)]
struct DownloadedImage {
    bytes: Vec<u8>,
    mime_type: String,
    width: Option<u32>,
    height: Option<u32>,
}

#[tauri::command]
pub async fn suggest_image_alt_text_command(
    page_url: String,
    images: Vec<ImageAltInput>,
) -> Result<Vec<AltSuggestion>, String> {
    validate_page_url(&page_url)?;
    if images.is_empty() {
        return Ok(Vec::new());
    }
    if images.len() > MAX_IMAGES_PER_REQUEST {
        return Err(format!(
            "A maximum of {MAX_IMAGES_PER_REQUEST} images can be processed per request"
        ));
    }

    let config = crate::gemini::get_gemini_config()
        .map_err(|error| format!("Gemini is not configured: {error}"))?;
    if config.key.trim().is_empty() {
        return Err("Gemini API key is empty. Add it in RustySEO settings.".to_string());
    }

    let model = normalise_model_name(&config.gemini_model);
    let api_key = config.key;
    let client = build_http_client()?;

    let mut indexed_results = stream::iter(images.into_iter().enumerate().map(|(index, image)| {
        let client = client.clone();
        let page_url = page_url.clone();
        let api_key = api_key.clone();
        let model = model.clone();
        async move {
            let result = process_image(&client, &api_key, &model, &page_url, image).await;
            (index, result)
        }
    }))
    .buffer_unordered(MAX_CONCURRENCY)
    .collect::<Vec<_>>()
    .await;

    indexed_results.sort_by_key(|(index, _)| *index);
    Ok(indexed_results
        .into_iter()
        .map(|(_, suggestion)| suggestion)
        .collect())
}

#[tauri::command]
pub fn export_alt_suggestions_csv_command(
    suggestions: Vec<AltSuggestion>,
) -> Result<String, String> {
    let mut writer = csv::WriterBuilder::new()
        .has_headers(true)
        .from_writer(Vec::<u8>::new());

    for suggestion in &suggestions {
        writer
            .serialize(AltCsvRow {
                image_url: &suggestion.image_url,
                current_alt: suggestion.current_alt.as_deref().unwrap_or(""),
                suggested_alt: &suggestion.suggested_alt,
                confidence: round(suggestion.confidence, 3),
                status: status_label(&suggestion.status),
                decorative: suggestion.is_decorative,
                visible_text: suggestion.visible_text.as_deref().unwrap_or(""),
                reasoning: &suggestion.reasoning,
                mime_type: suggestion.mime_type.as_deref().unwrap_or(""),
                width: suggestion.width,
                height: suggestion.height,
                model: suggestion.model.as_deref().unwrap_or(""),
                error: suggestion.error.as_deref().unwrap_or(""),
            })
            .map_err(|error| format!("Failed to serialise alt-text CSV: {error}"))?;
    }

    writer
        .flush()
        .map_err(|error| format!("Failed to finish alt-text CSV: {error}"))?;
    let bytes = writer
        .into_inner()
        .map_err(|error| format!("Failed to build alt-text CSV: {}", error.into_error()))?;

    String::from_utf8(bytes).map_err(|error| format!("CSV was not valid UTF-8: {error}"))
}

async fn process_image(
    client: &Client,
    api_key: &str,
    model: &str,
    page_url: &str,
    input: ImageAltInput,
) -> AltSuggestion {
    let data = match input {
        ImageAltInput::Url(url) => ImageData {
            url,
            ..ImageData::default()
        },
        ImageAltInput::Detailed(data) => data,
    };

    let image_url = data.url.trim().to_string();
    if image_url.is_empty() {
        return error_suggestion("", data.current_alt, "Image URL cannot be empty");
    }

    if data.decorative == Some(true) {
        return AltSuggestion {
            image_url,
            current_alt: data.current_alt,
            suggested_alt: String::new(),
            confidence: 1.0,
            reasoning: "The caller marked this image as decorative; decorative images should normally use an empty alt attribute."
                .to_string(),
            visible_text: None,
            is_decorative: true,
            status: AltSuggestionStatus::Decorative,
            mime_type: None,
            width: None,
            height: None,
            model: None,
            error: None,
        };
    }

    let downloaded = match download_image(client, &image_url).await {
        Ok(image) => image,
        Err(error) => return error_suggestion(&image_url, data.current_alt, &error),
    };

    let payload = match request_alt_from_gemini(
        client,
        api_key,
        model,
        page_url,
        &data,
        &downloaded,
    )
    .await
    {
        Ok(payload) => payload,
        Err(error) => {
            let mut suggestion = error_suggestion(&image_url, data.current_alt, &error);
            suggestion.mime_type = Some(downloaded.mime_type);
            suggestion.width = downloaded.width;
            suggestion.height = downloaded.height;
            suggestion.model = Some(model.to_string());
            return suggestion;
        }
    };

    let current_alt = data
        .current_alt
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string);
    let mut suggested_alt = clean_alt_text(&payload.alt_text);
    let is_decorative = payload.is_decorative;
    if is_decorative {
        suggested_alt.clear();
    }

    let confidence = if payload.confidence.is_finite() {
        payload.confidence.clamp(0.0, 1.0)
    } else {
        0.0
    };
    let status = if is_decorative {
        AltSuggestionStatus::Decorative
    } else if suggested_alt.is_empty() {
        AltSuggestionStatus::ReviewRequired
    } else if current_alt
        .as_deref()
        .map(|current| normalise_for_comparison(current) == normalise_for_comparison(&suggested_alt))
        .unwrap_or(false)
    {
        AltSuggestionStatus::KeepCurrent
    } else if confidence < 0.65 {
        AltSuggestionStatus::ReviewRequired
    } else {
        AltSuggestionStatus::Suggested
    };

    AltSuggestion {
        image_url,
        current_alt,
        suggested_alt,
        confidence: round(confidence, 3),
        reasoning: truncate(&payload.reasoning, 600),
        visible_text: payload
            .visible_text
            .map(|value| truncate(value.trim(), 300))
            .filter(|value| !value.is_empty()),
        is_decorative,
        status,
        mime_type: Some(downloaded.mime_type),
        width: downloaded.width,
        height: downloaded.height,
        model: Some(model.to_string()),
        error: None,
    }
}

async fn download_image(client: &Client, image_url: &str) -> Result<DownloadedImage, String> {
    let parsed = validate_remote_image_url(image_url)?;
    let response = client
        .get(parsed)
        .send()
        .await
        .map_err(|error| format!("Failed to download image: {error}"))?;

    if !response.status().is_success() {
        return Err(format!(
            "Image request returned HTTP {}",
            response.status().as_u16()
        ));
    }

    if let Some(length) = response.content_length() {
        if length > MAX_IMAGE_BYTES as u64 {
            return Err(format!(
                "Image exceeds the {} MB limit",
                MAX_IMAGE_BYTES / 1024 / 1024
            ));
        }
    }

    let declared_mime = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(';').next())
        .map(str::trim)
        .unwrap_or("")
        .to_ascii_lowercase();

    let bytes = response
        .bytes()
        .await
        .map_err(|error| format!("Failed to read image response: {error}"))?;
    if bytes.is_empty() {
        return Err("Image response was empty".to_string());
    }
    if bytes.len() > MAX_IMAGE_BYTES {
        return Err(format!(
            "Image exceeds the {} MB limit",
            MAX_IMAGE_BYTES / 1024 / 1024
        ));
    }

    let decoded = image::load_from_memory(&bytes)
        .map_err(|error| format!("Unsupported or invalid raster image: {error}"))?;
    let (width, height) = decoded.dimensions();
    if width == 0 || height == 0 {
        return Err("Image has invalid zero dimensions".to_string());
    }

    let detected_mime = image::guess_format(&bytes)
        .ok()
        .and_then(image_format_mime)
        .unwrap_or_else(|| declared_mime.clone());
    if !matches!(
        detected_mime.as_str(),
        "image/jpeg" | "image/png" | "image/webp" | "image/gif"
    ) {
        return Err(format!(
            "Unsupported image type '{}'. Use JPEG, PNG, WebP or GIF.",
            if detected_mime.is_empty() {
                "unknown"
            } else {
                &detected_mime
            }
        ));
    }

    Ok(DownloadedImage {
        bytes: bytes.to_vec(),
        mime_type: detected_mime,
        width: Some(width),
        height: Some(height),
    })
}

async fn request_alt_from_gemini(
    client: &Client,
    api_key: &str,
    model: &str,
    page_url: &str,
    data: &ImageData,
    image: &DownloadedImage,
) -> Result<GeminiAltPayload, String> {
    let prompt = format!(
        "You are generating accessible HTML alternative text for a website image.\n\
         Page URL: {page_url}\n\
         Image URL: {}\n\
         Page title: {}\n\
         Existing alt text: {}\n\
         Nearby visible text: {}\n\
         Link destination when the image is clickable: {}\n\n\
         Inspect the image itself. Return concise, factual alt text that communicates the image's purpose in this page context. Do not keyword-stuff. Do not begin with 'image of' or 'picture of' unless that wording is necessary. Preserve important visible words, but do not transcribe irrelevant decorative text. If the image is decorative and conveys no information or link purpose, set isDecorative=true and altText to an empty string. Confidence must be between 0 and 1. The reasoning should be brief and suitable for a human reviewer.",
        data.url,
        data.page_title.as_deref().unwrap_or("Not supplied"),
        data.current_alt.as_deref().unwrap_or("Not supplied"),
        data.surrounding_text.as_deref().unwrap_or("Not supplied"),
        data.link_destination.as_deref().unwrap_or("Not supplied"),
    );

    let encoded = BASE64_STANDARD.encode(&image.bytes);
    let request_body = json!({
        "contents": [{
            "role": "user",
            "parts": [
                { "text": prompt },
                {
                    "inlineData": {
                        "mimeType": image.mime_type.clone(),
                        "data": encoded
                    }
                }
            ]
        }],
        "generationConfig": {
            "temperature": 0.15,
            "maxOutputTokens": 500,
            "responseMimeType": "application/json",
            "responseSchema": {
                "type": "OBJECT",
                "properties": {
                    "altText": {
                        "type": "STRING",
                        "description": "Concise contextual HTML alt text, or an empty string for a decorative image"
                    },
                    "isDecorative": { "type": "BOOLEAN" },
                    "confidence": {
                        "type": "NUMBER",
                        "minimum": 0,
                        "maximum": 1
                    },
                    "reasoning": { "type": "STRING" },
                    "visibleText": {
                        "type": "STRING",
                        "description": "Important text visibly present in the image, or an empty string"
                    }
                },
                "required": ["altText", "isDecorative", "confidence", "reasoning", "visibleText"]
            }
        }
    });

    let response_text = call_gemini_with_retry(client, api_key, model, request_body).await?;
    let clean_json = strip_markdown_fence(&response_text);
    serde_json::from_str::<GeminiAltPayload>(clean_json)
        .map_err(|error| format!("Gemini returned invalid structured alt-text JSON: {error}"))
}

async fn call_gemini_with_retry(
    client: &Client,
    api_key: &str,
    model: &str,
    request_body: Value,
) -> Result<String, String> {
    let endpoint = format!("{GEMINI_BASE_URL}/{model}:generateContent");
    let mut last_error = String::new();

    for attempt in 0..3_u32 {
        let response = client
            .post(&endpoint)
            .header("x-goog-api-key", api_key)
            .json(&request_body)
            .send()
            .await;

        match response {
            Ok(response) if response.status().is_success() => {
                let parsed = response
                    .json::<GeminiResponse>()
                    .await
                    .map_err(|error| format!("Failed to parse Gemini API response: {error}"))?;
                return extract_gemini_text(parsed);
            }
            Ok(response) => {
                let status = response.status();
                let retryable = is_retryable_status(status);
                let body = response.text().await.unwrap_or_default();
                last_error = format!(
                    "Gemini API returned HTTP {}: {}",
                    status.as_u16(),
                    truncate(&body, 500)
                );
                if !retryable {
                    return Err(last_error);
                }
            }
            Err(error) => {
                last_error = format!("Gemini API request failed: {error}");
            }
        }

        if attempt < 2 {
            tokio::time::sleep(Duration::from_millis(750 * 2_u64.pow(attempt))).await;
        }
    }

    Err(if last_error.is_empty() {
        "Gemini API request failed after retries".to_string()
    } else {
        last_error
    })
}

fn extract_gemini_text(response: GeminiResponse) -> Result<String, String> {
    if response.candidates.is_empty() {
        let block_reason = response
            .prompt_feedback
            .and_then(|feedback| feedback.block_reason)
            .unwrap_or_else(|| "No candidate was returned".to_string());
        return Err(format!("Gemini did not return an alt-text candidate: {block_reason}"));
    }

    let candidate = &response.candidates[0];
    let text = candidate
        .content
        .as_ref()
        .into_iter()
        .flat_map(|content| content.parts.iter())
        .filter_map(|part| part.text.as_deref())
        .collect::<Vec<_>>()
        .join("");

    if text.trim().is_empty() {
        return Err(format!(
            "Gemini returned an empty response{}",
            candidate
                .finish_reason
                .as_deref()
                .map(|reason| format!(" (finish reason: {reason})"))
                .unwrap_or_default()
        ));
    }
    Ok(text)
}

fn build_http_client() -> Result<Client, String> {
    Client::builder()
        .connect_timeout(Duration::from_secs(15))
        .timeout(Duration::from_secs(60))
        .redirect(Policy::limited(5))
        .user_agent("RustySEO/0.3.9 (+https://rustyseo.com)")
        .build()
        .map_err(|error| format!("Failed to initialise HTTP client: {error}"))
}

fn validate_page_url(page_url: &str) -> Result<(), String> {
    let parsed = Url::parse(page_url.trim()).map_err(|error| format!("Invalid pageUrl: {error}"))?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return Err("pageUrl must use http or https".to_string());
    }
    Ok(())
}

fn validate_remote_image_url(image_url: &str) -> Result<Url, String> {
    let parsed = Url::parse(image_url).map_err(|error| format!("Invalid image URL: {error}"))?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return Err("Image URL must use http or https".to_string());
    }
    let host = parsed
        .host_str()
        .ok_or_else(|| "Image URL has no host".to_string())?;
    if host.eq_ignore_ascii_case("localhost") || host.ends_with(".localhost") {
        return Err("Localhost image URLs are blocked".to_string());
    }
    if let Ok(ip) = host.parse::<IpAddr>() {
        let blocked = match ip {
            IpAddr::V4(ip) => {
                ip.is_private()
                    || ip.is_loopback()
                    || ip.is_link_local()
                    || ip.is_broadcast()
                    || ip.is_unspecified()
            }
            IpAddr::V6(ip) => ip.is_loopback() || ip.is_unspecified() || ip.is_unique_local(),
        };
        if blocked {
            return Err("Private or local image IP addresses are blocked".to_string());
        }
    }
    Ok(parsed)
}

fn normalise_model_name(configured_model: &str) -> String {
    let value = configured_model.trim().trim_start_matches("models/");
    if value.is_empty() {
        DEFAULT_VISION_MODEL.to_string()
    } else {
        value.to_string()
    }
}

fn image_format_mime(format: image::ImageFormat) -> Option<String> {
    match format {
        image::ImageFormat::Jpeg => Some("image/jpeg".to_string()),
        image::ImageFormat::Png => Some("image/png".to_string()),
        image::ImageFormat::WebP => Some("image/webp".to_string()),
        image::ImageFormat::Gif => Some("image/gif".to_string()),
        _ => None,
    }
}

fn clean_alt_text(value: &str) -> String {
    let clean = value
        .replace('\r', " ")
        .replace('\n', " ")
        .replace('\t', " ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .trim_matches(|character| matches!(character, '\"' | '\'' | ' '))
        .to_string();
    truncate(&clean, 220)
}

fn normalise_for_comparison(value: &str) -> String {
    value
        .chars()
        .filter(|character| character.is_alphanumeric() || character.is_whitespace())
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_ascii_lowercase()
}

fn strip_markdown_fence(value: &str) -> &str {
    let trimmed = value.trim();
    let without_opening = trimmed
        .strip_prefix("```json")
        .or_else(|| trimmed.strip_prefix("```JSON"))
        .or_else(|| trimmed.strip_prefix("```"))
        .unwrap_or(trimmed);
    without_opening
        .strip_suffix("```")
        .unwrap_or(without_opening)
        .trim()
}

fn is_retryable_status(status: StatusCode) -> bool {
    status == StatusCode::TOO_MANY_REQUESTS
        || status == StatusCode::REQUEST_TIMEOUT
        || status.is_server_error()
}

fn error_suggestion(
    image_url: &str,
    current_alt: Option<String>,
    error: &str,
) -> AltSuggestion {
    AltSuggestion {
        image_url: image_url.to_string(),
        current_alt,
        suggested_alt: String::new(),
        confidence: 0.0,
        reasoning: "No suggestion was generated because the image could not be processed."
            .to_string(),
        visible_text: None,
        is_decorative: false,
        status: AltSuggestionStatus::Error,
        mime_type: None,
        width: None,
        height: None,
        model: None,
        error: Some(truncate(error, 600)),
    }
}

fn status_label(status: &AltSuggestionStatus) -> &'static str {
    match status {
        AltSuggestionStatus::Suggested => "suggested",
        AltSuggestionStatus::KeepCurrent => "keep_current",
        AltSuggestionStatus::Decorative => "decorative",
        AltSuggestionStatus::ReviewRequired => "review_required",
        AltSuggestionStatus::Error => "error",
    }
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

fn round(value: f64, decimal_places: u32) -> f64 {
    let factor = 10_f64.powi(decimal_places as i32);
    (value * factor).round() / factor
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cleans_model_names() {
        assert_eq!(normalise_model_name(""), DEFAULT_VISION_MODEL);
        assert_eq!(
            normalise_model_name("models/gemini-2.5-flash-lite"),
            "gemini-2.5-flash-lite"
        );
    }

    #[test]
    fn cleans_alt_text_without_keyword_mutation() {
        assert_eq!(
            clean_alt_text("  A red front door\nwith a brass handle  "),
            "A red front door with a brass handle"
        );
    }

    #[test]
    fn blocks_local_image_urls() {
        assert!(validate_remote_image_url("http://127.0.0.1/image.png").is_err());
        assert!(validate_remote_image_url("https://example.com/image.png").is_ok());
    }

    #[test]
    fn parses_structured_payload() {
        let value = r#"{
            "altText":"A technician inspecting an outdoor air-conditioning unit",
            "isDecorative":false,
            "confidence":0.91,
            "reasoning":"The image shows the service being performed.",
            "visibleText":""
        }"#;
        let parsed: GeminiAltPayload = serde_json::from_str(value).expect("payload should parse");
        assert_eq!(parsed.alt_text, "A technician inspecting an outdoor air-conditioning unit");
    }
}
