<p align="center">
  <img src="assets/icon.jpg" width="160" alt="Rausu Icon" />
</p>

<h1 align="center">Rausu</h1>
<p align="center"><em>ラウス</em></p>

<p align="center">
  <a href="./README.md">English</a> &bull;
  <a href="./README_CN.md">中文</a>
</p>

<p align="center">
  <a href="#クイックスタート">クイックスタート</a> &bull;
  <a href="#機能">機能</a> &bull;
  <a href="#設定">設定</a> &bull;
  <a href="#アーキテクチャ">アーキテクチャ</a> &bull;
  <a href="./README.md">English</a> &bull;
  <a href="./README_CN.md">中文</a>
</p>

<p align="center">
  <img src="https://img.shields.io/badge/language-Rust-orange?style=flat-square" alt="Rust" />
  <img src="https://img.shields.io/badge/license-MIT-blue?style=flat-square" alt="MIT" />
  <img src="https://img.shields.io/badge/version-0.1.0--dev-green?style=flat-square" alt="v0.1.0-dev" />
  <img src="https://img.shields.io/badge/clippy-0%20warnings-brightgreen?style=flat-square" alt="Clippy" />
</p>

Rust 製の高性能 LLM API ゲートウェイ。単一バイナリ、ランタイム依存ゼロ、複数の LLM プロバイダーにわたるプロトコル対応ルーティング。

## 機能

- **OpenAI 互換 API** — 任意の OpenAI SDK クライアントからそのまま利用可能
- **マルチプロバイダー** — OpenAI、Anthropic（API キー）、Claude サブスクリプション（OAuth）、GitHub Copilot、ChatGPT サブスクリプション（OAuth）、OpenRouter、および任意の OpenAI 互換プロバイダー（DeepSeek、Qwen、Ollama、GLM、Moonshot など）に対応
- **プロトコルブリッジ** — OpenAI Responses API と Anthropic Messages API の双方向変換；Codex CLI から Claude モデルや任意の OpenAI 互換プロバイダーを利用可能、Claude Code から GPT モデルや任意の OpenAI 互換プロバイダーを利用可能
- **真の SSE ストリーミング** — プロトコルブリッジ経由を含む全パスでゼロバッファ・イベント単位のストリーミングを実現（最初のトークンレイテンシはパススルーと同等）
- **単一バイナリ** — ランタイム依存ゼロ
- **YAML 設定** — 環境変数インターポレーション対応
- **API キー認証** — リモート公開プロキシを保護するオプションの静的キー認証
- **構造化ログ** — リクエストトレーシング付き JSON ログ

## クイックスタート

### オプション 1: ソースからビルド

```bash
cargo build --release

# テンプレート設定を生成（~/.config/rausu/config.yaml に書き込まれます）
./target/release/rausu init
# 編集後に起動:
./target/release/rausu
```

設定ファイルのパスを明示的に指定することもできます:

```bash
./target/release/rausu --config config.yaml
```

### オプション 2: Docker（GHCR）

```bash
docker pull ghcr.io/ypwu1/rausu:latest
docker run -p 4000:4000 -v $(pwd)/config.yaml:/app/config.yaml ghcr.io/ypwu1/rausu:latest
```

マルチアーキテクチャイメージ（linux/amd64、linux/arm64）は各バージョンタグ時に `ghcr.io/ypwu1/rausu` へ公開されます。利用可能なタグ: `latest`、`vX.Y.Z`、`vX.Y`。

### オプション 3: Docker（ソースからビルド）

```bash
docker build -t rausu .
docker run -p 4000:4000 -v $(pwd)/config.yaml:/app/config.yaml rausu
```

## 設定

### 自動検出

`--config` なしで `rausu` を実行すると、以下の順で設定ファイルを検索します:

1. `RAUSU_CONFIG` 環境変数
2. `./config.yaml`
3. `./rausu-config.yaml`
4. `${XDG_CONFIG_HOME:-~/.config}/rausu/config.yaml`
5. `${XDG_CONFIG_HOME:-~/.config}/rausu/rausu-config.yaml`
6. `~/.rausu/config.yaml`
7. `~/rausu-config.yaml`

見つからない場合はコメント付きテンプレートが `${XDG_CONFIG_HOME:-~/.config}/rausu/config.yaml` に書き出され、編集を促すメッセージとともにプロセスが終了します。

### `rausu init`

```bash
rausu init                    # ~/.config/rausu/config.yaml にテンプレートを書き込む
rausu init --path ./my.yaml   # カスタムパスへ書き込む
rausu init --force            # 既存ファイルを上書き
```

### `rausu setup`

インタラクティブな設定エディター — YAML を手書きせずに設定を作成・編集できます:

```bash
rausu setup                    # デフォルト位置で作成または編集
rausu setup --path ./my.yaml   # 特定のファイルを対象にする
```

エディターはモデル中心の設計です。まず仮想モデルを作成し、フェイルオーバー順序付きのプロバイダーデプロイメントをアタッチします。モデルとプロバイダーの追加・編集・削除・並び替えに対応しており、既存の設定は自動的に読み込まれます。

保存前の検証ではエラー（未知のプロバイダー、必須フィールドの欠如、重複）と警告（認証情報の欠如、到達不能なエンドポイント）をチェックします。詳細は [docs/SETUP_EDITOR.md](docs/SETUP_EDITOR.md) を参照してください。

### `rausu check`

設定の検証とプロバイダーの接続テストを実行します:

```bash
rausu check                    # 自動検出された設定を使用
rausu check --config my.yaml   # 特定の設定ファイルを使用
```

出力例:

```
📋 Config: ~/.config/rausu/config.yaml
   Server: 127.0.0.1:4000
   Auth: static (2 keys)

📦 Models (3):
   ✓ gpt-5.4 → chatgpt-subscription
   ✓ claude-opus-4.6 → github-copilot
   ✓ deepseek-chat → openai (https://api.deepseek.com/v1)

🔌 Connectivity:
   ✓ chatgpt-subscription: token available (codex auth)
   ✓ github-copilot: hosts.json found (~/.config/github-copilot/hosts.json)
   ✓ openai (https://api.deepseek.com/v1): reachable (HTTP 200)
   ✗ openai (http://localhost:11434/v1): connection refused

✅ 3/4 providers OK
```

チェックは4ステップで実行されます: 設定の読み込み、モデル検証（必須フィールド、有効なプロバイダータイプ）、プロバイダー接続性（HTTP 到達性または認証情報ファイルの存在確認）、認証検証。

> **起動時検証**: 同じ検証ロジックが `rausu` のサーバーモード起動時に自動実行されます。ハードエラー（未知のプロバイダー、必須フィールドの欠如、名前の重複）は起動をブロックします。警告（認証情報の欠如、到達不能なエンドポイント）はログに記録されますが、サーバーの起動は続行されます。

### 手動セットアップ

`config.example.yaml` をコピーしてカスタマイズします:

```bash
cp config.example.yaml config.yaml
# config.yaml を編集して API キーを設定
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

  # Claude Pro/Max サブスクリプション — API キー不要
  - name: claude-sonnet-sub
    providers:
      - provider: claude-subscription
        model: claude-sonnet-4-20250514
        # token_source: auto   # auto（デフォルト）| env | credentials_file
        # credentials_path: /custom/path/.credentials.json  # オプション

  # ChatGPT Plus/Pro/Max サブスクリプション — API キー不要
  - name: gpt-5
    providers:
      - provider: chatgpt-subscription
        model: gpt-5.4
        # token_source: auto   # auto（デフォルト）| env | credentials_file
        # credentials_path: ~/.config/rausu/chatgpt-auth.json  # オプション
```

### `claude-subscription` プロバイダー

有料 API キーの代わりに OAuth を介して Claude Pro/Max サブスクリプションを使用します。

**トークンソース（優先順位順）:**

1. **`env`** — `CLAUDE_OAUTH_TOKEN=<access_token>` を設定（静的、自動更新なし）
2. **`credentials_file`** — Claude CLI が書き込む `~/.claude/.credentials.json` を読み込み、自動トークン更新をサポート
3. **`auto`**（デフォルト）— `env` を先に試し、次に `credentials_file` を試す

```yaml
models:
  - name: claude-sonnet-sub
    providers:
      - provider: claude-subscription
        model: claude-sonnet-4-20250514
        token_source: credentials_file          # オプション、デフォルト: auto
        # credentials_path: ~/.claude/.credentials.json  # オプションの上書きパス
```

### `chatgpt-subscription` プロバイダー

有料 API キーの代わりに OAuth を介して ChatGPT Plus/Pro/Max サブスクリプションを使用します。リクエストは内部で Chat Completions 形式から ChatGPT Responses API へブリッジされます。

**トークンソース（優先順位順）:**

1. **`env`** — `CHATGPT_ACCESS_TOKEN=<access_token>` を設定（オプションで `CHATGPT_REFRESH_TOKEN` と `CHATGPT_ACCOUNT_ID` も設定可能）
2. **`credentials_file`** — `~/.config/rausu/chatgpt-auth.json` を読み込み、自動トークン更新をサポート
3. **`auto`**（デフォルト）— `env` を先に試し、次に `credentials_file` を試す

```yaml
models:
  - name: gpt-5
    providers:
      - provider: chatgpt-subscription
        model: gpt-5.4
        token_source: env              # オプション、デフォルト: auto

  - name: gpt-5-pro
    providers:
      - provider: chatgpt-subscription
        model: gpt-5.4-pro
        token_source: credentials_file
        credentials_path: /custom/path/chatgpt-auth.json  # オプションの上書きパス
```

**認証情報ファイルの形式**（`~/.config/rausu/chatgpt-auth.json`）:

```json
{
  "access_token": "eyJ...",
  "refresh_token": "...",
  "expires_at": 1900000000000,
  "account_id": "acc_..."
}
```

**対応モデル:** `gpt-5.4`、`gpt-5.4-pro`、`gpt-5.3-codex`、`gpt-5.3-codex-spark`、`gpt-5.3-instant`、`gpt-5.3-chat-latest`

> **注意:** 全プロバイダー（`openai`、`anthropic`、`claude-subscription`、`chatgpt-subscription`、`openrouter` など）は完全に独立しており、同一の設定ファイルに共存させて異なる仮想モデル名として提供できます。

### `openrouter` プロバイダー

[OpenRouter](https://openrouter.ai) 経由でリクエストをルーティングし、単一の API キーで 100 以上のモデル（OpenAI、Anthropic、Google、Meta など）にアクセスできます。チャット補完、ストリーミング、Responses API ブリッジに対応しています。完全な設定、モデル ID、ケイパビリティ対応ルーティングの詳細については [docs/OPENROUTER_PROVIDER.md](docs/OPENROUTER_PROVIDER.md) を参照してください。

### 認証

Rausu はリモート公開プロキシを保護するためのオプションの API キー認証をサポートしています。2 つのモードがあります:

- **`disabled`**（デフォルト）— 認証なし。全リクエストをそのまま転送します。
- **`static`** — 受信リクエストは設定済みキーのいずれかに一致する有効な `Authorization: Bearer <key>` ヘッダーを持つ必要があります。

```yaml
auth:
  mode: static
  keys:
    - name: "my-laptop"
      key: "rausu-sk-abc123"
    - name: "remote-client"
      key: "${RAUSU_API_KEY}"    # 環境変数インターポレーションをサポート
```

キー値は `${ENV_VAR}` インターポレーションをサポートします。推奨キープレフィックスの規約は `rausu-sk-` です。

`/health` エンドポイントは常に認証が免除されます。

`auth` セクションが完全に省略された場合、認証はデフォルトで `disabled` になります。

環境変数による上書きは `RAUSU__` プレフィックスと `__` をセパレーターとして使用します:

```bash
RAUSU__SERVER__PORT=8080 rausu
```

## 使い方

OpenAI SDK の接続先を `http://localhost:4000` に向けます:

```python
from openai import OpenAI

client = OpenAI(
    api_key="not-used",
    base_url="http://localhost:4000/v1",
)

# OpenAI へルーティング
response = client.chat.completions.create(
    model="gpt-4o",
    messages=[{"role": "user", "content": "こんにちは！"}],
)

# Anthropic へルーティング（同じ API！）
response = client.chat.completions.create(
    model="claude-sonnet",
    messages=[{"role": "user", "content": "こんにちは！"}],
)
```

## クライアント × モデルマトリックス

全てのクライアントとモデルの組み合わせが、パススルーまたはプロトコルブリッジを介してサポートされています:

| クライアント | プロトコル | ターゲット | パス |
|-------------|-----------|-----------|------|
| Claude Code | `/v1/messages` | Claude（Copilot） | パススルー |
| Claude Code | `/v1/messages` | Claude（Anthropic） | パススルー |
| Claude Code | `/v1/messages` | GPT（ChatGPT サブスクリプション） | Messages→Responses ブリッジ |
| Claude Code | `/v1/messages` | 任意の OpenAI 互換プロバイダー | Messages→Responses→ChatCompletions |
| Codex CLI | `/v1/responses` | GPT（ChatGPT サブスクリプション） | パススルー |
| Codex CLI | `/v1/responses` | GPT（Copilot） | パススルー |
| Codex CLI | `/v1/responses` | Claude（Copilot） | Responses→Messages ブリッジ |
| Codex CLI | `/v1/responses` | 任意の OpenAI 互換プロバイダー | Responses→ChatCompletions ブリッジ |

プロトコル変換の詳細は [docs/PROTOCOL_BRIDGE_PLAN.md](docs/PROTOCOL_BRIDGE_PLAN.md) を参照してください。

## API エンドポイント

| エンドポイント | メソッド | 説明 |
|--------------|---------|------|
| `/health` | GET | ヘルスチェック |
| `/v1/models` | GET | 設定済みモデルの一覧 |
| `/v1/chat/completions` | POST | チャット補完 — ルーティング + フォーマット変換 |
| `/v1/responses` | POST | OpenAI Responses API — パススルーまたは Responses→Messages ブリッジ |
| `/v1/responses/compact` | POST | OpenAI Responses API コンパクト版 — 透過的パススルー |
| `/v1/messages` | POST | Anthropic Messages API — パススルーまたは Messages→Responses ブリッジ |

> **注意:** 全ての `/v1/...` ルートはプレフィックスなしでも利用可能です（例: `/responses`、`/chat/completions`、`/models`、`/messages`）。これにより、`{base_url}/responses` を使用する Codex CLI のようなクライアントが追加設定なしで動作します。

## ローカルプロキシとしての使用

Rausu は Codex CLI と Claude Code 向けのシングルユーザーローカルプロキシとして動作できます。ローカルクライアントはプレースホルダーの API キーを渡し、Rausu が実際の上流認証情報を自動的に注入します。

設定例、ダミーキーの動作、Codex CLI と Claude Code の接続手順を含む完全なガイドは [docs/LOCAL_PROXY_USAGE.md](docs/LOCAL_PROXY_USAGE.md) を参照してください。

## アーキテクチャ

完全なアーキテクチャ決定記録（ローカルファースト、ゲートウェイ互換設計）は [docs/ARCHITECTURE_DIRECTION.md](docs/ARCHITECTURE_DIRECTION.md) を参照してください。

## ビルド

必要条件: Rust 1.70+

```bash
cargo build --release
cargo test
cargo clippy
```

## ライセンス

MIT — [LICENSE](./LICENSE) を参照
