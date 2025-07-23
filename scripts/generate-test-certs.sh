#!/bin/bash

# Generate test certificates for HTTPS development
# Note: These are for testing only, not for production use

echo "Generating test certificates for HTTPS development..."

mkdir -p certs

openssl genrsa -out certs/server.key 2048

openssl req -new -key certs/server.key -out certs/server.csr -subj "/C=US/ST=Test/L=Test/O=Test/OU=Test/CN=localhost"

openssl x509 -req -days 365 -in certs/server.csr -signkey certs/server.key -out certs/server.crt

cp certs/server.crt certs/server.pem
cp certs/server.key certs/server-key.pem

echo "Test certificates generated in ./certs/ directory:"
echo "  - Certificate: certs/server.pem"
echo "  - Private Key: certs/server-key.pem"
echo ""
echo "WARNING: These are self-signed certificates for development only!"
echo "   Browsers will show security warnings."
echo ""
echo "To test HTTPS, update your config.yaml with:"
echo "  tls:"
echo "    cert_path: \"./certs/server.pem\""
echo "    key_path: \"./certs/server-key.pem\""

rm certs/server.csr
