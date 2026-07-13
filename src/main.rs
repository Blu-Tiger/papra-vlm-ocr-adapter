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

    let body = json!({
        "model": vlm_model,
        "messages": [
            {
                "role": "user",
                "content": [
                    { "type": "text", "text": prompt },
                    { "type": "image_url", "image_url": { "url": data_url } }
                ]
            }
        ],
        "max_tokens": 4096
    });

    let resp = client
        .post(vlm_url)
        .bearer_auth(vlm_api_key)
        .json(&body)
        .send()
        .await
        .map_err(|_| StatusCode::BAD_GATEWAY)?;

    let json: Value = resp.json().await.map_err(|_| StatusCode::BAD_GATEWAY)?;

    let text = json
        .pointer("/choices/0/message/content")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    Ok(text)
}

fn resize_if_needed(image_bytes: &[u8], max_dimension: u32) -> Result<Vec<u8>, StatusCode> {
    let img =
        image::load_from_memory(image_bytes).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let (width, height) = img.dimensions();
    let longest = width.max(height);

    let final_img = if longest > max_dimension {
        img.resize(
            max_dimension,
            max_dimension,
            image::imageops::FilterType::Lanczos3,
        )
    } else {
        img
    };

    let format = image::guess_format(image_bytes).unwrap_or(ImageFormat::Png);

    let mut buffer = Vec::new();
    final_img
        .write_to(&mut Cursor::new(&mut buffer), format)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

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
            let data = field.bytes().await.map_err(|_| StatusCode::BAD_REQUEST)?;

            let is_pdf = content_type == "application/pdf" || filename.ends_with(".pdf");
            let is_image = content_type.starts_with("image/")
                || [".png", ".jpg", ".jpeg", ".gif", ".webp", ".bmp", ".tiff"]
                    .iter()
                    .any(|ext| filename.to_lowercase().ends_with(ext));

            if !is_pdf && !is_image {
                return Err(StatusCode::UNSUPPORTED_MEDIA_TYPE);
            }

            let mut image_buffers: Vec<Vec<u8>> = Vec::new();

            if is_pdf {
                let pdfium = Pdfium::new(
                    Pdfium::bind_to_library(Pdfium::pdfium_platform_library_name_at_path("./"))
                        .or_else(|_| Pdfium::bind_to_system_library())
                        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?,
                );

                let document = pdfium
                    .load_pdf_from_byte_vec(data.to_vec(), None)
                    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

                let dpi = 200.0f32;
                let scale = dpi / 72.0;

                for page in document.pages().iter() {
                    let page_width = page.width().value;
                    let page_height = page.height().value;

                    let render_config = PdfRenderConfig::new()
                        .set_target_width((page_width * scale) as i32)
                        .set_maximum_height((page_height * scale) as i32);

                    let bitmap = page
                        .render_with_config(&render_config)
                        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

                    let img = bitmap
                        .as_image()
                        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
                        .into_rgb8();

                    let mut buffer = Vec::new();
                    img.write_to(&mut Cursor::new(&mut buffer), ImageFormat::Png)
                        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

                    image_buffers.push(buffer);
                }
            } else {
                image_buffers.push(data.to_vec());
            }

            let resized_buffers: Vec<Vec<u8>> = image_buffers
                .iter()
                .map(|buf| resize_if_needed(buf, max_dimension))
                .collect::<Result<_, _>>()?;

            let client = reqwest::Client::new();
            let mut extracted_texts: Vec<String> = Vec::new();

            for buf in &resized_buffers {
                let text = extract_text_from_image(
                    &client,
                    buf,
                    &vlm_url,
                    &vlm_model,
                    &vlm_api_key,
                    &prompt,
                )
                .await?;
                extracted_texts.push(text);
            }

            let combined_text = extracted_texts.join("\n\n");

            return Ok(Json(ExtractResponse {
                text: Some(combined_text),
                error: None,
            }));
        }
    }

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
