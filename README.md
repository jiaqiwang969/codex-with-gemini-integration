<p align="center">
  <img src="./.github/codex-cli-splash.png" alt="Codex CLI splash" width="80%" />
</p>

<h1 align="center">Codex CLI + Gemini API Integration</h1>

<p align="center">
  <a href="#english">English</a> | <a href="#中文">中文</a>
</p>

<p align="center">
  <a href="LICENSE"><img src="https://img.shields.io/badge/license-Apache%202.0-blue.svg" alt="License"></a>
  <a href="https://www.rust-lang.org/"><img src="https://img.shields.io/badge/rust-1.70+-orange.svg" alt="Rust"></a>
  <a href="https://ai.google.dev/gemini-api"><img src="https://img.shields.io/badge/Gemini%203-supported-green.svg" alt="Gemini"></a>
</p>

<p align="center">
  <strong>An enhanced Codex CLI with native Gemini 3 API support</strong>
</p>

---

<a name="english"></a>

## English

<details open>
<summary><strong>Click to expand English documentation</strong></summary>

### Overview

This project is a fork of [OpenAI Codex CLI](https://github.com/openai/codex) with **native Google Gemini 3 API integration**. It combines the powerful agentic coding capabilities of Codex with Google's latest Gemini 3 models (Pro, Flash, and Image variants).

### Key Features

#### Gemini 3 Integration
- **Full Gemini 3 Support**: Native support for `gemini-3-pro-preview`, `gemini-3-flash-preview`, and `gemini-3-pro-image-preview` models
- **Configurable Reasoning Levels**:
  - Gemini 3 Flash: `minimal`, `low`, `medium`, `high`
  - Gemini 3 Pro: `low`, `medium`, `high`
- **Parallel Function Calls**: Efficient multi-tool execution for faster task completion
- **Thought Signature Handling**: Proper management of Gemini's `thoughtSignature` for multi-turn conversations
- **Image Analysis**: Support for multimodal inputs with automatic media resolution selection

#### Enhanced Codex Features
- **Dual Model Support**: Seamlessly switch between OpenAI GPT models and Google Gemini models
- **MCP Protocol**: Full Model Context Protocol support for extensible tool integration
- **Sandbox Modes**: Flexible security policies (`read-only`, `workspace-write`, `danger-full-access`)
- **Reference Images**: `/ref-image` command for image-based workflows with Gemini image models

### Installation

Build from source:

```shell
git clone https://github.com/jiaqiwang969/codex.git
cd codex/codex-rs
cargo build --release
```

### Configuration

Configuration files are stored in `~/.codex/`:
- `config.toml` - Main configuration file
- `auth.json` - API keys storage

#### API Keys Setup (`~/.codex/auth.json`)

Create or edit `~/.codex/auth.json`:

```json
{
  "OPENAI_API_KEY": "sk-your-openai-api-key",
  "GEMINI_API_KEY": "AIzaSy-your-gemini-api-key"
}
```

Alternatively, use environment variables:
```shell
export OPENAI_API_KEY="sk-your-openai-api-key"
export GEMINI_API_KEY="AIzaSy-your-gemini-api-key"
```

#### Main Configuration (`~/.codex/config.toml`)

```toml
# Default model - provider is auto-selected based on model
model = "gemini-3-flash-preview-gemini"
model_provider = "openai-proxy"
model_reasoning_effort = "high"

# Disable response storage for privacy
disable_response_storage = true

# Feature flags
[features]
tui2 = false

# Model Providers Configuration
[model_providers.openai-proxy]
name = "Codex OpenAI Proxy"
base_url = "https://api.openai.com/v1"
wire_api = "responses"
requires_openai_auth = true

[model_providers.gemini]
name = "Codex Gemini Direct"
base_url = "https://generativelanguage.googleapis.com/v1"
wire_api = "gemini"
requires_openai_auth = false
auth_json_key = "GEMINI_API_KEY"

# Optional: Profiles for different use cases
[profiles.safe]
model = "gpt-5.2-codex"
model_provider = "openai-proxy"
approval_policy = "on-failure"
sandbox_mode = "workspace-write"
model_reasoning_effort = "medium"
```

#### Provider Auto-Selection

The provider is **automatically selected based on your configured model**. Simply run:

```shell
codex
```

The system will automatically use the appropriate provider and API key based on your model configuration.

#### Supported Models

| Model | Description | Reasoning Levels |
|-------|-------------|------------------|
| `gemini-3-pro-preview-codex` | Best quality reasoning | low, medium, high |
| `gemini-3-flash-preview-gemini` | Fast and efficient | minimal, low, medium, high |
| `gemini-3-pro-image-preview` | Image generation & analysis | medium |
| `gpt-5.2-codex` | OpenAI flagship model | low, medium, high, xhigh |

### Usage Examples

```shell
# Start interactive TUI (uses model from config.toml)
codex

# Non-interactive execution
codex exec "Analyze this codebase and suggest improvements"

# Override model for this session
codex -m gemini-3-pro-preview-codex

# Image workflow with Gemini
codex
> /ref-image screenshot.png -- Analyze this UI and suggest improvements

# Select reasoning level in TUI
> /model
# Then choose your preferred reasoning level
```

### Architecture

```
codex-rs/
├── core/           # Business logic, Gemini API client, model families
├── tui/            # Fullscreen terminal UI (Ratatui)
├── exec/           # Headless CLI for automation
├── cli/            # CLI multitool entry point
└── protocol/       # Wire protocols and types
```

### Documentation

- [Getting started](./docs/getting-started.md)
- [Configuration](./docs/config.md)
- [Sandbox & approvals](./docs/sandbox.md)
- [Authentication](./docs/authentication.md)
- [FAQ](./docs/faq.md)

### Acknowledgments

- [OpenAI Codex](https://github.com/openai/codex) - The original Codex CLI
- [Google Gemini API](https://ai.google.dev/) - Gemini 3 models

</details>

---

<a name="中文"></a>

## 中文

<details open>
<summary><strong>点击展开中文文档</strong></summary>

### 概述

本项目是 [OpenAI Codex CLI](https://github.com/openai/codex) 的增强分支，**原生集成了 Google Gemini 3 API**。它将 Codex 强大的智能编程能力与 Google 最新的 Gemini 3 模型（Pro、Flash 和 Image 变体）相结合。

### 核心特性

#### Gemini 3 集成
- **完整的 Gemini 3 支持**：原生支持 `gemini-3-pro-preview`、`gemini-3-flash-preview` 和 `gemini-3-pro-image-preview` 模型
- **可配置的推理级别**：
  - Gemini 3 Flash：`minimal`（最小）、`low`（低）、`medium`（中）、`high`（高）
  - Gemini 3 Pro：`low`（低）、`medium`（中）、`high`（高）
- **并行函数调用**：高效的多工具执行，加快任务完成速度
- **思维签名处理**：正确管理 Gemini 的 `thoughtSignature`，支持多轮对话
- **图像分析**：支持多模态输入，自动选择媒体分辨率

#### 增强的 Codex 功能
- **双模型支持**：在 OpenAI GPT 模型和 Google Gemini 模型之间无缝切换
- **MCP 协议**：完整的模型上下文协议支持，可扩展工具集成
- **沙箱模式**：灵活的安全策略（`read-only` 只读、`workspace-write` 工作区写入、`danger-full-access` 完全访问）
- **参考图像**：使用 `/ref-image` 命令配合 Gemini 图像模型进行图像工作流

### 安装

从源码构建：

```shell
git clone https://github.com/jiaqiwang969/codex.git
cd codex/codex-rs
cargo build --release
```

### 配置

配置文件存储在 `~/.codex/` 目录下：
- `config.toml` - 主配置文件
- `auth.json` - API 密钥存储

#### API 密钥设置 (`~/.codex/auth.json`)

创建或编辑 `~/.codex/auth.json`：

```json
{
  "OPENAI_API_KEY": "sk-your-openai-api-key",
  "GEMINI_API_KEY": "AIzaSy-your-gemini-api-key"
}
```

或者使用环境变量：
```shell
export OPENAI_API_KEY="sk-your-openai-api-key"
export GEMINI_API_KEY="AIzaSy-your-gemini-api-key"
```

#### 主配置文件 (`~/.codex/config.toml`)

```toml
# 默认模型 - 提供者根据模型自动选择
model = "gemini-3-flash-preview-gemini"
model_provider = "openai-proxy"
model_reasoning_effort = "high"

# 禁用响应存储以保护隐私
disable_response_storage = true

# 功能开关
[features]
tui2 = false

# 模型提供者配置
[model_providers.openai-proxy]
name = "Codex OpenAI Proxy"
base_url = "https://api.openai.com/v1"
wire_api = "responses"
requires_openai_auth = true

[model_providers.gemini]
name = "Codex Gemini Direct"
base_url = "https://generativelanguage.googleapis.com/v1"
wire_api = "gemini"
requires_openai_auth = false
auth_json_key = "GEMINI_API_KEY"

# 可选：不同使用场景的配置文件
[profiles.safe]
model = "gpt-5.2-codex"
model_provider = "openai-proxy"
approval_policy = "on-failure"
sandbox_mode = "workspace-write"
model_reasoning_effort = "medium"
```

#### 提供者自动选择

提供者**根据配置的模型自动选择**。只需运行：

```shell
codex
```

系统会根据模型配置自动使用对应的提供者和 API 密钥。

#### 支持的模型

| 模型 | 描述 | 推理级别 |
|------|------|----------|
| `gemini-3-pro-preview-codex` | 最佳推理质量 | low, medium, high |
| `gemini-3-flash-preview-gemini` | 快速高效 | minimal, low, medium, high |
| `gemini-3-pro-image-preview` | 图像生成与分析 | medium |
| `gpt-5.2-codex` | OpenAI 旗舰模型 | low, medium, high, xhigh |

### 使用示例

```shell
# 启动交互式 TUI（使用 config.toml 中的模型）
codex

# 非交互式执行
codex exec "分析这个代码库并提出改进建议"

# 临时覆盖模型
codex -m gemini-3-pro-preview-codex

# Gemini 图像工作流
codex
> /ref-image screenshot.png -- 分析这个 UI 并提出改进建议

# 在 TUI 中选择推理级别
> /model
# 然后选择你偏好的推理级别
```

### 项目架构

```
codex-rs/
├── core/           # 业务逻辑、Gemini API 客户端、模型家族
├── tui/            # 全屏终端 UI（Ratatui）
├── exec/           # 无头 CLI，用于自动化
├── cli/            # CLI 多功能工具入口
└── protocol/       # 通信协议和类型定义
```

### 文档

- [快速入门](./docs/getting-started.md)
- [配置说明](./docs/config.md)
- [沙箱与审批](./docs/sandbox.md)
- [认证方式](./docs/authentication.md)
- [常见问题](./docs/faq.md)

### 致谢

- [OpenAI Codex](https://github.com/openai/codex) - 原始 Codex CLI
- [Google Gemini API](https://ai.google.dev/) - Gemini 3 模型

</details>

---

## License

Apache 2.0 - See [LICENSE](LICENSE) for details.
