# Papra VLM Adapter

![Rust](https://img.shields.io/badge/Rust-grey?logo=rust&logoColor=white)
![License](https://img.shields.io/badge/License-MIT-blue)

Lightweight Rust adapter that bridges Papra's custom-http document extraction to any OpenAI-compatible VLM.
Built with Axum, Tokio, pdfium-render, and the image crate.

---

## What It Does

Papra is a self-hosted document archiving platform that supports external content extraction via a `custom-http` strategy. This adapter sits between Papra and your VLM, handling the translation:

```
┌─────────┐   POST file   ┌──────────┐   /v1/chat/completions   ┌──────────┐
│  Papra  │ ───────────>  │  Adapter │  ─────────────────────>  │   VLM    │
│         │  (form-data)  │          │  (base64 image + prompt) │          │
│         │ <───────────  │          │  <─────────────────────  │          │
└─────────┘  JSON {text}  └──────────┘   extracted text         └──────────┘
```

The adapter:

1. **Receives** a file (PDF or image) from Papra via multipart form-data
2. **Splits** PDFs into individual page images
3. **Resizes** images exceeding the VLM's optimal resolution threshold (default: 3360px)
4. **Base64-encodes** each image and sends it to the OpenAI-compatible `/v1/chat/completions` endpoint
5. **Concatenates** all page results and returns them as `{"text": "..."}`

---

## Why?

Papra's built-in extraction uses Tesseract OCR, which works but struggles with complex layouts, tables, and multilingual documents. Vision-language models achieve state-of-the-art OCR quality while being small enough to run on edge hardware.

This adapter makes it trivial to plug any OpenAI-compatible VLM into Papra without modifying Papra itself.

---

## Quick Start


---

## Configuration

All settings via environment variables:

| Variable | Required | Default | Description |
|---|---|---|---|
| `HOST` | No | `0.0.0.0` | Host |
| `PORT` | No | `1222` | Service port |
| `MAX_DIMENSION` | No | `3360` | Max pixel dimension before proportional downscaling |
| `VLM_URL` | Yes | — | OpenAI-compatible chat completions endpoint URL |
| `VLM_MODEL` | Yes | — | Model name to use for text extraction |
| `VLM_API_KEY` | No | `""` | API key for the VLM service |
| `PROMPT` | No | `""` | Text prompt sent alongside each image |

> **Note:** Command-line environment variables override `.env` file values.

---

## Papra Configuration

Set these environment variables in your Papra container/service:

```env
CONTENT_EXTRACTION_STRATEGY=custom-http,internal
CONTENT_EXTRACTION_CUSTOM_HTTP_URL=http://adapter:1222/extract
CONTENT_EXTRACTION_CUSTOM_HTTP_UPLOAD_FORMAT=form-data
CONTENT_EXTRACTION_CUSTOM_HTTP_RESPONSE_FORMAT=json
CONTENT_EXTRACTION_CUSTOM_HTTP_JSON_RESPONSE_TEXT_PATH=text
CONTENT_EXTRACTION_CUSTOM_HTTP_REQUEST_TIMEOUT_MS=300000
CONTENT_EXTRACTION_CUSTOM_HTTP_MIME_TYPES_ALLOW_LIST=image/*,application/pdf
```

The `custom-http,internal` strategy means Papra tries the VLM adapter first, then falls back to built-in Tesseract OCR if the adapter fails or times out.

### Docker Compose

```yaml
services:
  papra:
    image: ghcr.io/papra-hq/papra:latest
    ports:
      - "1221:1221"
    environment:
      CONTENT_EXTRACTION_STRATEGY: "custom-http,internal"
      CONTENT_EXTRACTION_CUSTOM_HTTP_URL: "http://adapter:1222/extract"
      CONTENT_EXTRACTION_CUSTOM_HTTP_UPLOAD_FORMAT: "form-data"
      CONTENT_EXTRACTION_CUSTOM_HTTP_RESPONSE_FORMAT: "json"
      CONTENT_EXTRACTION_CUSTOM_HTTP_JSON_RESPONSE_TEXT_PATH: "text"
      CONTENT_EXTRACTION_CUSTOM_HTTP_REQUEST_TIMEOUT_MS: "300000"
      CONTENT_EXTRACTION_CUSTOM_HTTP_MIME_TYPES_ALLOW_LIST: "image/*,application/pdf"
      DATABASE_URL: "file:./db/db.sqlite"
      AUTH_SECRET: "${AUTH_SECRET}"
    volumes:
      - ./app-data:/app/app-data
    depends_on:
      - adapter

  adapter:
     image: ghcr.io/blu-tiger/papra-vlm-ocr-adapter:latest
    environment:
      VLM_URL: "http://vlm-server-host:8080/v1/chat/completions"
      VLM_MODEL: "model-name"
    restart: unless-stopped

volumes:
  papra-data:
```

---

## API Endpoints

| Method | Path | Description |
|--------|------|-------------|
| `GET` | `/health` | Health check — returns `200 ok` |
| `POST` | `/extract` | Main endpoint — receives multipart form-data from Papra |

### Response Format

```json
{
  "text": "All extracted text from all pages concatenated..."
}
```
---

## License

MIT — see [LICENSE](LICENSE) file for details.

---

## Acknowledgements

- [Papra](https://github.com/papra-hq/papra) — The minimalistic document archiving platform
- [Axum](https://github.com/tokio-rs/axum) — Ergonomic and modular web framework for Rust
