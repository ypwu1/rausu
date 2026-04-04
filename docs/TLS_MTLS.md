# TLS and mTLS Support

Rausu supports optional TLS (Transport Layer Security) and mutual TLS (mTLS) to encrypt traffic between clients and the gateway.

- **TLS (one-way)**: The server presents a certificate; the client verifies the server's identity.
- **mTLS (mutual)**: Both server and client present certificates. The server verifies the client's identity against a trusted CA, providing strong authentication at the transport layer.

TLS is **orthogonal** to the existing API key authentication middleware. You can use TLS alone, API key auth alone, or both together.

## Configuration

Add a `tls` section under `server` in your config YAML:

### TLS-only (one-way)

```yaml
server:
  host: "0.0.0.0"
  port: 4000
  tls:
    cert_file: "/path/to/server.crt"
    key_file: "/path/to/server.key"
```

### mTLS (mutual TLS)

```yaml
server:
  host: "0.0.0.0"
  port: 4000
  tls:
    cert_file: "/path/to/server.crt"
    key_file: "/path/to/server.key"
    client_ca_file: "/path/to/client-ca.crt"
```

When `client_ca_file` is set, clients **must** present a certificate signed by that CA.

### Environment Variable Interpolation

All TLS paths support `${ENV_VAR}` syntax:

```yaml
server:
  tls:
    cert_file: "${RAUSU_TLS_CERT_FILE}"
    key_file: "${RAUSU_TLS_KEY_FILE}"
    client_ca_file: "${RAUSU_TLS_CLIENT_CA_FILE}"
```

## Generating Test Certificates

Use `openssl` to create a local CA, server certificate, and client certificate for development/testing.

### 1. Create a CA

```bash
# Generate CA private key
openssl genrsa -out ca.key 4096

# Generate self-signed CA certificate (valid for 365 days)
openssl req -new -x509 -key ca.key -out ca.crt -days 365 \
  -subj "/CN=Rausu Test CA"
```

### 2. Create a Server Certificate

```bash
# Generate server private key
openssl genrsa -out server.key 2048

# Create a certificate signing request (CSR)
openssl req -new -key server.key -out server.csr \
  -subj "/CN=localhost"

# Sign with CA (add SAN for localhost)
openssl x509 -req -in server.csr -CA ca.crt -CAkey ca.key \
  -CAcreateserial -out server.crt -days 365 \
  -extfile <(printf "subjectAltName=DNS:localhost,IP:127.0.0.1")
```

### 3. Create a Client Certificate (for mTLS)

```bash
# Generate client private key
openssl genrsa -out client.key 2048

# Create CSR
openssl req -new -key client.key -out client.csr \
  -subj "/CN=rausu-client"

# Sign with CA
openssl x509 -req -in client.csr -CA ca.crt -CAkey ca.key \
  -CAcreateserial -out client.crt -days 365
```

### 4. Test with curl

```bash
# TLS-only
curl --cacert ca.crt https://localhost:4000/health

# mTLS
curl --cacert ca.crt --cert client.crt --key client.key \
  https://localhost:4000/health
```

## Validating Configuration

Run `rausu check` to verify your TLS configuration without starting the server:

```bash
rausu check
```

This will:
- Verify that cert/key/CA files exist and are readable.
- Attempt to parse the PEM files.
- Report whether the configuration is TLS-only or mTLS.

## Integration Notes

### Claude Code

Claude Code supports mTLS via environment variables. Point it at your Rausu gateway:

```bash
export OPENAI_BASE_URL=https://localhost:4000/v1
export OPENAI_API_KEY=your-rausu-key
# For mTLS, Claude Code can be configured with client certs via env vars.
```

### Codex CLI

Codex CLI does not support client certificates (mTLS). Use TLS-only with static API key authentication:

```yaml
server:
  tls:
    cert_file: server.crt
    key_file: server.key
auth:
  mode: static
  keys:
    - name: codex
      key: "${RAUSU_API_KEY}"
```

## Technical Details

- TLS is implemented with **rustls** (no OpenSSL dependency).
- Both TLS 1.2 and TLS 1.3 are supported.
- In TLS mode, each connection goes through `tokio-rustls::TlsAcceptor` before being served by hyper/axum.
- Graceful shutdown works in both plain HTTP and TLS modes.
