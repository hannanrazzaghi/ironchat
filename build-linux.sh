#!/bin/bash
# Build Linux binary using Docker

docker run --rm \
  -v "$PWD":/workspace \
  -w /workspace \
  rust:latest \
  cargo build --release

echo "Linux binaries are in target/release/"
echo "Copy target/release/chatctl to your friend"
