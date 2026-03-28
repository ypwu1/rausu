# Rausu (ラウス)

> [English Version](./README.md)

一个用 Rust 编写的高性能 LLM API 网关——作为 [LiteLLM Proxy](https://github.com/BerriAI/litellm) 的替代方案，在性能、内存占用和部署便捷性上全面超越（单一二进制文件）。

## 特性

- **OpenAI 兼容 API** — 适配任何 OpenAI SDK 客户端
- **多 Provider 支持** — Phase 1 支持 OpenAI 和 Anthropic，后续版本将增加更多 Provider
- **流式传输** — 完整的 SSE 流式支持
- **单一二进制** — 零运行时依赖
- **YAML 配置** — 支持环境变量插值
- **结构化日志** — 带请求追踪的 JSON 日志

## 快速开始

### 方式一：从源码构建

```bash
cargo build --release
./target/release/rausu --config config.yaml
```

### 方式二：Docker

```bash
docker build -t rausu .
docker run -p 4000:4000 -v $(pwd)/config.yaml:/app/config.yaml rausu
```

## 配置

复制示例配置并修改：

```bash
cp config.example.yaml config.yaml
# 编辑 config.yaml，填入你的 API Key
```

```yaml
server:
  host: 0.0.0.0
  port: 4000

logging:
  level: info
  format: json   # json | pretty

models:
  - name: gpt-4o
    providers:
      - provider: openai
        model: gpt-4o
        api_key: "${OPENAI_API_KEY}"

  - name: claude-sonnet
    providers:
      - provider: anthropic
        model: claude-sonnet-4-20250514
        api_key: "${ANTHROPIC_API_KEY}"
```

环境变量覆盖使用 `RAUSU__` 前缀，以 `__` 为分隔符：

```bash
RAUSU__SERVER__PORT=8080 rausu
```

## 使用方法

将你的 OpenAI SDK 指向 `http://localhost:4000`：

```python
from openai import OpenAI

client = OpenAI(
    api_key="not-used",
    base_url="http://localhost:4000/v1",
)

# 路由到 OpenAI
response = client.chat.completions.create(
    model="gpt-4o",
    messages=[{"role": "user", "content": "你好！"}],
)

# 路由到 Anthropic（相同的 API！）
response = client.chat.completions.create(
    model="claude-sonnet",
    messages=[{"role": "user", "content": "你好！"}],
)
```

## API 端点

| 端点 | 方法 | 描述 |
|------|------|------|
| `/health` | GET | 健康检查 |
| `/v1/models` | GET | 列出已配置的模型 |
| `/v1/chat/completions` | POST | 聊天补全（流式 & 非流式） |

## 构建

要求：Rust 1.70+

```bash
cargo build --release
cargo test
cargo clippy
```

## 开源协议

MIT — 详见 [LICENSE](./LICENSE)
