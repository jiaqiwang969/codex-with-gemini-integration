# Codex CLI + Gemini API Integration

<div align="center">

[English](#english) | [中文](#中文)

[![License](https://img.shields.io/badge/license-Apache%202.0-blue.svg)](LICENSE)
[![Rust](https://img.shields.io/badge/rust-1.70+-orange.svg)](https://www.rust-lang.org/)
[![Gemini](https://img.shields.io/badge/Gemini%203-supported-green.svg)](https://ai.google.dev/gemini-api)

**An enhanced Codex CLI with native Gemini 3 API support and advanced customization features**

</div>

---

<a name="english"></a>
## English

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

```shell
npm i -g @jiaqiwang969/codex
codex
```

Or build from source:

```shell
git clone https://github.com/jiaqiwang969/codex.git
cd codex/codex-rs
cargo build --release
```

### Configuration

#### Using Gemini API

1. Set your Gemini API key:
```shell
export GEMINI_API_KEY="your-api-key"
```

2. Configure in `~/.codex/config.toml`:
```toml
model = "gemini-3-pro-preview-codex"
# Or for Flash variant:
# model = "gemini-3-flash-preview-gemini"
```

3. Select reasoning level via `/model` command in TUI

#### Supported Models

| Model | Description | Reasoning Levels |
|-------|-------------|------------------|
| `gemini-3-pro-preview-codex` | Best quality reasoning | low, medium, high |
| `gemini-3-flash-preview-gemini` | Fast and efficient | minimal, low, medium, high |
| `gemini-3-pro-image-preview` | Image generation & analysis | medium |
| `gpt-5.2-codex` | OpenAI flagship model | low, medium, high, xhigh |

### Usage Examples

```shell
# Start interactive TUI
codex

# Non-interactive execution
codex exec "Analyze this codebase and suggest improvements"

# With specific model
codex -m gemini-3-pro-preview-codex

# Image workflow with Gemini
codex
> /ref-image screenshot.png -- Analyze this UI and suggest improvements
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

### Contributing

Contributions are welcome! Please feel free to submit issues and pull requests.

### Acknowledgments

- [OpenAI Codex](https://github.com/openai/codex) - The original Codex CLI
- [Google Gemini API](https://ai.google.dev/) - Gemini 3 models

---

<a name="中文"></a>
## 中文

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

```shell
npm i -g @jiaqiwang969/codex
codex
```

或从源码构建：

```shell
git clone https://github.com/jiaqiwang969/codex.git
cd codex/codex-rs
cargo build --release
```

### 配置

#### 使用 Gemini API

1. 设置 Gemini API 密钥：
```shell
export GEMINI_API_KEY="your-api-key"
```

2. 在 `~/.codex/config.toml` 中配置：
```toml
model = "gemini-3-pro-preview-codex"
# 或使用 Flash 变体：
# model = "gemini-3-flash-preview-gemini"
```

3. 在 TUI 中通过 `/model` 命令选择推理级别

#### 支持的模型

| 模型 | 描述 | 推理级别 |
|------|------|----------|
| `gemini-3-pro-preview-codex` | 最佳推理质量 | low, medium, high |
| `gemini-3-flash-preview-gemini` | 快速高效 | minimal, low, medium, high |
| `gemini-3-pro-image-preview` | 图像生成与分析 | medium |
| `gpt-5.2-codex` | OpenAI 旗舰模型 | low, medium, high, xhigh |

### 使用示例

```shell
# 启动交互式 TUI
codex

# 非交互式执行
codex exec "分析这个代码库并提出改进建议"

# 指定模型
codex -m gemini-3-pro-preview-codex

# Gemini 图像工作流
codex
> /ref-image screenshot.png -- 分析这个 UI 并提出改进建议
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

### 贡献

欢迎贡献！请随时提交 Issue 和 Pull Request。

### 致谢

- [OpenAI Codex](https://github.com/openai/codex) - 原始 Codex CLI
- [Google Gemini API](https://ai.google.dev/) - Gemini 3 模型

---

## License

Apache 2.0 - See [LICENSE](LICENSE) for details.
