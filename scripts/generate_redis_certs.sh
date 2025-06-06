#!/usr/bin/env bash
set -euxo pipefail
# This script generates a self-signed SSL certificate for Redis.
# It uses OpenSSL to create a certificate and a private key.
# The certificate is valid for 5 years and includes SAN (Subject Alternative Name) for the specified domain.
# It can be used to secure Redis connections over TLS.
# Step 1: enter the certificate name in the form of an argument
# Step 2 enter the domain name as a second argument otherwise it will be use the full hostname of the host to generate the certificate
## E.g: `./generate_ssl_certs.sh mycertname <domainname>`
# Optionally add a second DNS name to the SAN certificate by using a 3rd argument
## E.g: `./generate_ssl_certs.sh mycertname <domainname> appname`

CERT_DIR="$(dirname "$0")/../certs"
mkdir -p "$CERT_DIR"
cd "$CERT_DIR"

# Check if openssl is installed
if ! command -v openssl &> /dev/null; then
    echo "OpenSSL is not installed. Please install it to generate certificates."
    exit 1
fi

DNS2="${3:-}"
if [ -n "$DNS2" ]; then
    DNS2="DNS:${DNS2},"
else
    DNS2=""
fi

# If the caller passed a second argument, use it; otherwise default to "redis".
DNS1="${2:-}"
if [ -z "$DNS1" ]; then
    DNS1="redis"
fi

# If the caller passed a first argument, use it; otherwise default to "redis".
CERT_NAME="${1:-}"
if [ -z "$CERT_NAME" ]; then
    CERT_NAME="redis"
fi

openssl req \
      -x509 \
      -newkey rsa:4096 \
      -sha512 \
      -days 1825 \
      -nodes \
      -keyout "${CERT_NAME}".key \
      -out "${CERT_NAME}".crt \
      -subj /CN="${DNS1}" \
      -extensions san \
      -config <( \
 echo '[req]'; \
 echo 'distinguished_name=req'; \
 echo '[san]'; \
 echo "subjectAltName=DNS:${DNS1},${DNS2}DNS:localhost")

 # Copy self signed cert content to ca file
cp "${CERT_NAME}".crt ca.crt
