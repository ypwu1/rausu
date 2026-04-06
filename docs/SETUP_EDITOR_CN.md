# 交互式配置编辑器 (`rausu setup`)

`rausu setup` 是一个以模型为中心的交互式配置编辑器，在终端中运行。它可以从零创建新配置，也可以加载并编辑现有配置。

## 快速开始

```bash
# 在默认位置创建或编辑配置
rausu setup

# 编辑指定的配置文件
rausu setup --path /path/to/config.yaml
```

## 功能特性

- **加载现有配置** — 如果配置文件已存在，会自动加载到编辑器中
- **以模型为中心** — 先创建虚拟模型，然后为其添加一个或多个供应商部署
- **多供应商故障转移** — 每个模型可以有多个供应商，顺序决定故障转移优先级
- **完整的增删改查** — 添加、编辑、删除和重新排序模型及其供应商部署
- **保存前验证** — 共享验证引擎在写入前检查错误
- **所有配置部分** — 模型、认证、服务器、TLS 和日志均可编辑

## 顶层菜单

启动 `rausu setup` 后，您将看到：

```
Configuration section:
> Models        （模型）
  Auth          （认证）
  Server        （服务器）
  TLS
  Logging       （日志）
  Validate      （验证）
  Save and Exit （保存并退出）
  Exit without Saving （不保存退出）
```

## 模型工作流

### 创建模型

1. 从顶层菜单选择 **Models**
2. 选择 **+ Add model**
3. 输入虚拟模型名称（如 `gpt-4o`）
4. 可选添加别名（如 `gpt-4, gpt4o`）
5. 选择一个或多个供应商（顺序 = 故障转移优先级）
6. 为每个供应商配置其所需字段

### 编辑模型

选择一个现有模型可以：
- **编辑名称** — 更改虚拟模型名称
- **编辑别名** — 修改或清除别名列表
- **查看/编辑供应商部署** — 选择任意供应商进行编辑或删除
- **添加供应商部署** — 为故障转移附加另一个供应商
- **重新排序供应商** — 上移或下移供应商优先级
- **删除模型** — 完全移除该模型

## 支持的供应商

| 供应商 | 必填字段 |
|---|---|
| GitHub Copilot | 上游模型名称 |
| ChatGPT Subscription | 上游模型名称、令牌来源 |
| Claude Subscription | 上游模型名称、令牌来源 |
| OpenAI API | 上游模型名称、API 密钥、可选 base URL |
| Anthropic API | 上游模型名称、API 密钥 |
| Vertex AI | 上游模型名称、GCP 项目 ID、区域 |

自定义 OpenAI 兼容供应商（DeepSeek、Ollama 等）使用 OpenAI 供应商类型，配合自定义 `base_url`。

## 验证

从顶层菜单选择 **Validate** 运行共享验证引擎。它会报告：

- **错误**（阻止启动）：未知供应商类型、缺少必填字段、空模型名称、重复名称/别名、无效令牌来源
- **警告**（仅提示）：缺少 API 密钥、缺少凭据文件、未配置模型

选择 **Save and Exit** 时也会自动运行相同的验证。

## 示例：带故障转移的多供应商模型

```
Models > + Add model
  Virtual model name: claude-sonnet
  Aliases: sonnet
  Providers: Anthropic API, GitHub Copilot

  Configuring Anthropic API for 'claude-sonnet':
    Upstream model: claude-sonnet-4-6
    API key: ****

  Configuring GitHub Copilot for 'claude-sonnet':
    Upstream model: claude-sonnet-4-6
```

这会创建一个模型 `claude-sonnet`（别名 `sonnet`），优先尝试 Anthropic，如果失败则回退到 GitHub Copilot。
