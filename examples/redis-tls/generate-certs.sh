#!/bin/bash
# Generate self-signed TLS certificates for Redis
# These are for local development/testing only — do NOT use in production.

set -e

CERT_DIR="$(cd "$(dirname "$0")" && pwd)/certs"
mkdir -p "$CERT_DIR"

echo "Generating CA key and certificate..."
openssl genrsa -out "$CERT_DIR/ca.key" 4096
openssl req -x509 -new -nodes -sha256 \
  -key "$CERT_DIR/ca.key" \
  -days 365 \
  -subj "/CN=Redis-Test-CA" \
  -out "$CERT_DIR/ca.crt"

echo "Generating Redis server key and certificate..."
openssl genrsa -out "$CERT_DIR/redis.key" 2048
openssl req -new -sha256 \
  -key "$CERT_DIR/redis.key" \
  -subj "/CN=redis" \
  -out "$CERT_DIR/redis.csr"

openssl x509 -req -sha256 \
  -in "$CERT_DIR/redis.csr" \
  -CA "$CERT_DIR/ca.crt" \
  -CAkey "$CERT_DIR/ca.key" \
  -CAcreateserial \
  -days 365 \
  -out "$CERT_DIR/redis.crt"

rm -f "$CERT_DIR/redis.csr" "$CERT_DIR/ca.srl"

chmod 644 "$CERT_DIR"/*.crt "$CERT_DIR"/*.key

echo "Certificates generated in $CERT_DIR/"
echo "  ca.crt     - CA certificate"
echo "  redis.key  - Redis server private key"
echo "  redis.crt  - Redis server certificate"
