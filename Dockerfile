# Образ Oberih: багатоетапна збірка, невеликий фінальний образ, не root.
#
#   docker build -t oberih .
#   docker run --rm oberih version
#   docker run --rm -p 8080:8080 -v "$PWD":/app oberih run app.obh
#
# Образ можна перевірити без жодних власних файлів:
#   docker run --rm oberih run /opt/oberih/examples/29_http_server.obh

# ---- етап збірки ----
FROM rust:1-bookworm AS build
WORKDIR /src
COPY Cargo.toml Cargo.lock* ./
COPY src ./src
RUN cargo build --release

# ---- фінальний образ ----
FROM debian:bookworm-slim
# tini: `docker stop` надсилає SIGTERM процесові з PID 1, а Linux ігнорує SIGTERM для PID 1,
# якщо процес не встановив обробник. tini передає сигнал далі, тож контейнер зупиняється одразу.
RUN apt-get update \
 && apt-get install -y --no-install-recommends tini \
 && rm -rf /var/lib/apt/lists/* \
 && useradd --system --create-home --uid 10001 oberih
COPY --from=build /src/target/release/oberih /usr/local/bin/oberih
COPY examples /opt/oberih/examples
USER oberih
WORKDIR /app
EXPOSE 8080
ENTRYPOINT ["/usr/bin/tini", "--", "oberih"]
CMD ["help"]
