FROM rust:1.82-bookworm AS builder

WORKDIR /app

RUN apt-get update && apt-get install -y \
    pkg-config \
    libssl-dev \
    curl \
    && rm -rf /var/lib/apt/lists/*

RUN curl -L -o pdfium.tar.gz \
    https://github.com/bblanchon/pdfium-binaries/releases/latest/download/pdfium-linux-x64.tgz \
    && tar -xzf pdfium.tar.gz -C /usr/local/lib \
    && rm pdfium.tar.gz

COPY Cargo.toml Cargo.lock ./
RUN mkdir src && echo "fn main() {}" > src/main.rs \
    && cargo build --release || true \
    && rm -rf src

COPY src ./src
RUN touch src/main.rs && cargo build --release

FROM debian:bookworm-slim AS runtime

RUN apt-get update && apt-get install -y \
    libssl3 \
    ca-certificates \
    && rm -rf /var/lib/apt/lists/*

COPY --from=builder /usr/local/lib/libpdfium.so /usr/local/lib/libpdfium.so
RUN ldconfig /usr/local/lib

RUN useradd -m -s /bin/bash appuser

WORKDIR /app

COPY --from=builder /app/target/release/papra-vlm-ocr-adapter /app/papra-vlm-ocr-adapter

COPY --from=builder /usr/local/lib/libpdfium.so /app/libpdfium.so

RUN chown -R appuser:appuser /app
USER appuser

EXPOSE 1222

CMD ["/app/papra-vlm-ocr-adapter"]
