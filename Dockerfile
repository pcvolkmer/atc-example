FROM rust:alpine AS build

RUN apk update
RUN apk add cmake make musl-dev g++

WORKDIR /build
COPY Cargo.toml ./
COPY askama.toml ./
COPY src ./src

RUN cargo build --release

# Build image from scratch
FROM scratch
LABEL org.opencontainers.image.source="https://github.com/pcvolkmer/atc-example"
LABEL org.opencontainers.image.licenses="AGPL-3.0-or-later"
LABEL org.opencontainers.image.description="Proof of Concept - Ermitteln von ATC-Codes aus Freitext"

COPY --from=build /build/target/release/atc-example .
USER 65532
EXPOSE 3000
CMD ["./atc-example"]