# TLS 和 mTLS 支持

Rausu 支持可选的 TLS（传输层安全）和双向 TLS（mTLS），用于加密客户端与网关之间的流量。

- **TLS（单向）**：服务器出示证书；客户端验证服务器身份。
- **mTLS（双向）**：服务器和客户端都出示证书。服务器使用受信任的 CA 验证客户端身份，在传输层提供强认证。

TLS 与现有的 API 密钥认证中间件**正交**。您可以单独使用 TLS、单独使用 API 密钥认证，或两者同时使用。

## 配置

在配置 YAML 的 `server` 下添加 `tls` 部分：

### 仅 TLS（单向）

```yaml
server:
  host: "0.0.0.0"
  port: 4000
  tls:
    cert_file: "/path/to/server.crt"
    key_file: "/path/to/server.key"
```

### mTLS（双向 TLS）

```yaml
server:
  host: "0.0.0.0"
  port: 4000
  tls:
    cert_file: "/path/to/server.crt"
    key_file: "/path/to/server.key"
    client_ca_file: "/path/to/client-ca.crt"
```

设置 `client_ca_file` 后，客户端**必须**提供由该 CA 签署的证书。

### 环境变量插值

所有 TLS 路径支持 `${ENV_VAR}` 语法：

```yaml
server:
  tls:
    cert_file: "${RAUSU_TLS_CERT_FILE}"
    key_file: "${RAUSU_TLS_KEY_FILE}"
    client_ca_file: "${RAUSU_TLS_CLIENT_CA_FILE}"
```

## 生成测试证书

使用 `openssl` 创建本地 CA、服务器证书和客户端证书，用于开发/测试。

### 1. 创建 CA

```bash
# 生成 CA 私钥
openssl genrsa -out ca.key 4096

# 生成自签名 CA 证书（有效期 365 天）
openssl req -new -x509 -key ca.key -out ca.crt -days 365 \
  -subj "/CN=Rausu Test CA"
```

### 2. 创建服务器证书

```bash
# 生成服务器私钥
openssl genrsa -out server.key 2048

# 创建证书签名请求 (CSR)
openssl req -new -key server.key -out server.csr \
  -subj "/CN=localhost"

# 使用 CA 签署（为 localhost 添加 SAN）
openssl x509 -req -in server.csr -CA ca.crt -CAkey ca.key \
  -CAcreateserial -out server.crt -days 365 \
  -extfile <(printf "subjectAltName=DNS:localhost,IP:127.0.0.1")
```

### 3. 创建客户端证书（用于 mTLS）

```bash
# 生成客户端私钥
openssl genrsa -out client.key 2048

# 创建 CSR
openssl req -new -key client.key -out client.csr \
  -subj "/CN=rausu-client"

# 使用 CA 签署
openssl x509 -req -in client.csr -CA ca.crt -CAkey ca.key \
  -CAcreateserial -out client.crt -days 365
```

### 4. 使用 curl 测试

```bash
# 仅 TLS
curl --cacert ca.crt https://localhost:4000/health

# mTLS
curl --cacert ca.crt --cert client.crt --key client.key \
  https://localhost:4000/health
```

## 验证配置

运行 `rausu check` 以在不启动服务器的情况下验证 TLS 配置：

```bash
rausu check
```

这将：
- 验证证书/密钥/CA 文件是否存在且可读。
- 尝试解析 PEM 文件。
- 报告配置是仅 TLS 还是 mTLS。

## 集成说明

### Claude Code

Claude Code 支持通过环境变量配置 mTLS。将其指向您的 Rausu 网关：

```bash
export OPENAI_BASE_URL=https://localhost:4000/v1
export OPENAI_API_KEY=your-rausu-key
# 对于 mTLS，Claude Code 可通过环境变量配置客户端证书。
```

### Codex CLI

Codex CLI 不支持客户端证书（mTLS）。请使用仅 TLS 配合静态 API 密钥认证：

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

## 技术细节

- TLS 使用 **rustls** 实现（无需 OpenSSL 依赖）。
- 支持 TLS 1.2 和 TLS 1.3。
- 在 TLS 模式下，每个连接先经过 `tokio-rustls::TlsAcceptor` 处理，然后由 hyper/axum 提供服务。
- 优雅关闭在纯 HTTP 和 TLS 模式下均可工作。
