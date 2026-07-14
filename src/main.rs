use axum::extract::State;
use axum::{
    Json, Router,
    extract::Multipart,
    http::StatusCode,
    routing::{get, post},
};
use base64::{Engine as _, engine::general_purpose};
use image::{GenericImageView, ImageFormat};
use pdfium_render::prelude::*;
use serde::Serialize;
use serde_json::{Value, json};
use std::io::Cursor;

fn load_config() -> (String, String, u32, String, String, String, String) {
    let max_dimension = std::env::var("MAX_DIMENSION")
        .unwrap_or_else(|_| "3360".to_string())
        .parse::<u32>()
        .expect("MAX_DIMENSION must be a valid u32");

    let host = std::env::var("HOST").unwrap_or_else(|_| "0.0.0.0".to_string());

    let port = std::env::var("PORT").unwrap_or_else(|_| "1222".to_string());

    let vlm_url = std::env::var("VLM_URL").expect("VLM_URL must be set in .env");

    let vlm_model = std::env::var("VLM_MODEL").expect("VLM_MODEL must be set in .env");

    let vlm_api_key = std::env::var("VLM_API_KEY").unwrap_or_else(|_| "".to_string());

    let prompt = std::env::var("PROMPT").unwrap_or_else(|_| "".to_string());

    (
        host,
        port,
        max_dimension,
        vlm_url,
        vlm_model,
        vlm_api_key,
        prompt,
    )
}

async fn extract_text_from_image(
    client: &reqwest::Client,
    image_bytes: &[u8],
    vlm_url: &str,
    vlm_model: &str,
    vlm_api_key: &str,
    prompt: &str,
) -> Result<String, StatusCode> {
    let b64 = general_purpose::STANDARD.encode(image_bytes);
    let data_url = format!("data:image/png;base64,{}", b64);

    eprintln!("[extract] Sending to VLM: model={vlm_model}, payload={} bytes (b64)", b64.len());

    let body = json!({
        "model": vlm_model,
        "messages": [{
            "role": "user",
            "content": [
                { "type": "text", "text": prompt },
                { "type": "image_url", "image_url": { "url": data_url } }
            ]
        }],
        "max_tokens": 4096
    });

    let resp = client
        .post(vlm_url)
        .bearer_auth(vlm_api_key)
        .json(&body)
        .send()
        .await
        .map_err(|e| {
            eprintln!("[extract] VLM request failed: {e}");
            StatusCode::BAD_GATEWAY
        })?;

    let status = resp.status();
    eprintln!("[extract] VLM responded with {status}");

    if !status.is_success() {
        let body = resp.text().await.unwrap_or_default();
        eprintln!("[extract] VLM error body: {body}");
        return Err(StatusCode::BAD_GATEWAY);
    }

    let json: Value = resp.json().await.map_err(|e| {
        eprintln!("[extract] VLM JSON parse failed: {e}");
        StatusCode::BAD_GATEWAY
    })?;

    let text = json
        .pointer("/choices/0/message/content")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    eprintln!("[extract] VLM returned {} chars of text", text.len());
    Ok(text)
}

fn resize_if_needed(image_bytes: &[u8], max_dimension: u32) -> Result<Vec<u8>, StatusCode> {
    let img =
        image::load_from_memory(image_bytes).map_err(|e| {
            eprintln!("[resize] Failed to load image: {e}");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    let (width, height) = img.dimensions();
    let longest = width.max(height);

    let final_img = if longest > max_dimension {
        eprintln!("[resize] {width}x{height} → resizing (max_dim={max_dimension})");
        img.resize(max_dimension, max_dimension, image::imageops::FilterType::Lanczos3)
    } else {
        eprintln!("[resize] {width}x{height} — no resize needed");
        img
    };

    let format = image::guess_format(image_bytes).unwrap_or(ImageFormat::Png);

    let mut buffer = Vec::new();
    final_img
        .write_to(&mut Cursor::new(&mut buffer), format)
        .map_err(|e| {
            eprintln!("[resize] Failed to encode image: {e}");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    Ok(buffer)
}

#[derive(Serialize)]
struct ExtractResponse {
    text: Option<String>,
    error: Option<String>,
}
async fn extract_route(
    State(config): State<(u32, String, String, String, String)>,
    mut multipart: Multipart,
) -> Result<Json<ExtractResponse>, StatusCode> {
    let (max_dimension, vlm_url, vlm_model, vlm_api_key, prompt) = config;

    while let Ok(Some(field)) = multipart.next_field().await {
        if field.name() == Some("file") {
            let filename = field.file_name().unwrap_or("").to_string();
            let content_type = field.content_type().unwrap_or("").to_string();
            let data = field.bytes().await.map_err(|e| {
                eprintln!("[extract] Failed to read multipart field: {e}");
                StatusCode::BAD_REQUEST
            })?;

            eprintln!("[extract] Received: filename={filename}, type={content_type}, {} bytes", data.len());

            let is_pdf = content_type == "application/pdf" || filename.ends_with(".pdf");
            let is_image = content_type.starts_with("image/")
                || [".png", ".jpg", ".jpeg", ".gif", ".webp", ".bmp", ".tiff"]
                    .iter()
                    .any(|ext| filename.to_lowercase().ends_with(ext));

            if !is_pdf && !is_image {
                eprintln!("[extract] Unsupported file type: {content_type}");
                return Err(StatusCode::UNSUPPORTED_MEDIA_TYPE);
            }

            let mut image_buffers: Vec<Vec<u8>> = Vec::new();

            if is_pdf {
                eprintln!("[extract] Processing PDF");
                let pdfium = Pdfium::new(
                    Pdfium::bind_to_library(Pdfium::pdfium_platform_library_name_at_path("./"))
                        .or_else(|_| Pdfium::bind_to_system_library())
                        .map_err(|e| {
                            eprintln!("[extract] Failed to bind pdfium: {e}");
                            StatusCode::INTERNAL_SERVER_ERROR
                        })?,
                );

                let document = pdfium
                    .load_pdf_from_byte_vec(data.to_vec(), None)
                    .map_err(|e| {
                        eprintln!("[extract] Failed to load PDF: {e}");
                        StatusCode::INTERNAL_SERVER_ERROR
                    })?;

                let page_count = document.pages().len();
                eprintln!("[extract] PDF has {page_count} page(s)");

                let dpi = 200.0f32;
                let scale = dpi / 72.0;

                for page in document.pages().iter() {
                    let page_width = page.width().value;
                    let page_height = page.height().value;
                    eprintln!("[extract] Rendering page ({}x{}pt → {}x{}px)",
                        page_width, page_height,
                        (page_width * scale) as i32, (page_height * scale) as i32);

                    let render_config = PdfRenderConfig::new()
                        .set_target_width((page_width * scale) as i32)
                        .set_maximum_height((page_height * scale) as i32);

                    let bitmap = page
                        .render_with_config(&render_config)
                        .map_err(|e| {
                            eprintln!("[extract] PDF render failed: {e}");
                            StatusCode::INTERNAL_SERVER_ERROR
                        })?;

                    let img = bitmap
                        .as_image()
                        .map_err(|e| {
                            eprintln!("[extract] Bitmap→image failed: {e}");
                            StatusCode::INTERNAL_SERVER_ERROR
                        })?
                        .into_rgb8();

                    let mut buffer = Vec::new();
                    img.write_to(&mut Cursor::new(&mut buffer), ImageFormat::Png)
                        .map_err(|e| {
                            eprintln!("[extract] PNG encode failed: {e}");
                            StatusCode::INTERNAL_SERVER_ERROR
                        })?;

                    image_buffers.push(buffer);
                }
            } else {
                image_buffers.push(data.to_vec());
            }

            eprintln!("[extract] {} image(s) to process", image_buffers.len());

            let resized_buffers: Vec<Vec<u8>> = image_buffers
                .iter()
                .map(|buf| resize_if_needed(buf, max_dimension))
                .collect::<Result<_, _>>()?;

            let client = reqwest::Client::new();
            let mut extracted_texts: Vec<String> = Vec::new();

            for (i, buf) in resized_buffers.iter().enumerate() {
                eprintln!("[extract] Processing image {}/{}, {} bytes", i + 1, resized_buffers.len(), buf.len());
                let text = extract_text_from_image(
                    &client, buf, &vlm_url, &vlm_model, &vlm_api_key, &prompt,
                ).await?;
                extracted_texts.push(text);
            }

            let combined_text = extracted_texts.join("\n\n");
            eprintln!("[extract] Done, total extracted text: {} chars", combined_text.len());

            return Ok(Json(ExtractResponse {
                text: Some(combined_text),
                error: None,
            }));
        }
    }

    eprintln!("[extract] No 'file' field found in multipart");
    Err(StatusCode::BAD_REQUEST)
}

#[derive(Serialize)]
struct HealthResponse {
    status: String,
    version: String,
}

async fn health() -> Json<HealthResponse> {
    Json(HealthResponse {
        status: "ok".to_string(),
        version: env!("CARGO_PKG_VERSION").to_string(),
    })
}

#[tokio::main]
async fn main() {
    dotenvy::dotenv().ok();

    let config = load_config();

    let (host, port, max_dimension, vlm_url, vlm_model, vlm_api_key, prompt) = config;

    let host_port = format!("{}:{}", host, port);

    let app = Router::new()
        .route("/health", get(health))
        .route("/extract", post(extract_route))
        .with_state((max_dimension, vlm_url, vlm_model, vlm_api_key, prompt));

    let listener = tokio::net::TcpListener::bind(&host_port).await.unwrap();
    println!("Listening on http://{}", host_port);
    axum::serve(listener, app).await.unwrap();
}
