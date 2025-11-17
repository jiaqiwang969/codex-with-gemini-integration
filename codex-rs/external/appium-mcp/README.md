# MCP Appium - MCP server for Mobile Development and Automation | iOS, Android, Simulator, Emulator, and Real Devices

MCP Appium is an intelligent MCP (Model Context Protocol) server designed to empower AI assistants with a robust suite of tools for mobile automation. It streamlines mobile app testing by enabling natural language interactions, intelligent locator generation, and automated test creation for both Android and iOS platforms.

## Table of Contents

- [Features](#-features)
- [Prerequisites](#-prerequisites)
- [Installation](#️-installation)
- [Configuration](#️-configuration)
- [Available Tools](#-available-tools)
- [Client Support](#-client-support)
- [Usage Examples](#-usage-examples)
- [Contributing](#-contributing)
- [License](#-license)

## 🚀 Features

- **Cross-Platform Support**: Automate tests for both Android (UiAutomator2) and iOS (XCUITest).
- **Intelligent Locator Generation**: AI-powered element identification using priority-based strategies.
- **Interactive Session Management**: Easily create and manage sessions on local mobile devices.
- **Smart Element Interactions**: Perform actions like clicks, text input, screenshots, and element finding.
- **Automated Test Generation**: Generate Java/TestNG test code from natural language descriptions.
- **Page Object Model Support**: Utilize built-in templates that follow industry best practices.
- **Flexible Configuration**: Customize capabilities and settings for different environments.

## 📋 Prerequisites

Before you begin, ensure you have the following installed:

### System Requirements

- **Node.js** (v22 or higher)
- **npm** or **yarn**
- **Java Development Kit (JDK)** (8 or higher)
- **Android SDK** (for Android testing)
- **Xcode** (for iOS testing on macOS)

> macOS 提示：如果未显式设置 `ANDROID_HOME`，本服务会尝试使用默认路径 `/Users/jqwang/Library/Android/sdk`，并自动将 `<ANDROID_HOME>/platform-tools` 加入 `PATH`；若默认路径不存在，请手动设置 `ANDROID_HOME` 并确保 `platform-tools` 在 `PATH` 中。


## 🧰 Quick Installation Guide

1. **Clone 本仓库**（或下载发布包）：`git clone https://github.com/appium/appium-mcp.git`
2. **安装依赖**：在仓库根目录执行 `npm install`
3. **构建 + 链接**（用于全局 `mcp-appium` 命令）：
   - `npm run build`
   - `npm link`（或 `npm install -g .`）
4. **环境要求**：
   - Node.js ≥ 22、npm ≥ 10
   - JDK ≥ 8、Android SDK（已接受许可）、Xcode（如需 iOS）
   - 可选：`CAPABILITIES_CONFIG` 指向你的 `capabilities.json`
   - 若 ANDROID_HOME 未设且默认路径存在，`mcp-appium` 会自行设置；否则请手动配置
5. **接入 Codex 或其他 MCP 客户端**：
   - 例如：`codex mcp add appium -- mcp-appium`
   - 如果希望通过 SSE 暴露：`mcp-appium --sse --port=7099`

完成后即可在 MCP 客户端中看到 `appium` 工具列表，按需运行 `select_platform → create_session → appium_*` 等工具来执行移动端自动化任务。

### Mobile Testing Setup

#### Android

1.  Install Android Studio and the Android SDK.
2.  Set the `ANDROID_HOME` environment variable.
3.  Add the Android SDK tools to your system's PATH.
4.  Enable USB debugging on your Android device.
5.  Install the Appium UiAutomator2 driver dependencies.

#### iOS (macOS only)

1.  Install Xcode from the App Store.
2.  Install the Xcode Command Line Tools: `xcode-select --install`.
3.  Install iOS simulators through Xcode.
4.  For real device testing, configure your provisioning profiles.

## 🛠️ Installation

### As an MCP Server

To integrate MCP Appium with your MCP client, add the following to your configuration:

```json
{
  "mcpServers": {
    "mcp-appium": {
      "disabled": false,
      "timeout": 100,
      "type": "stdio",
      "command": "npx",
      "args": ["appium-mcp"],
      "env": {
        "ANDROID_HOME": "/path/to/android/sdk",
        "CAPABILITIES_CONFIG": "/path/to/your/capabilities.json"
      }
    }
  }
}
```

## ⚙️ Configuration

### Capabilities

Create a `capabilities.json` file to define your device capabilities:

```json
{
  "android": {
    "appium:app": "/path/to/your/android/app.apk",
    "appium:deviceName": "Android Device",
    "appium:platformVersion": "11.0",
    "appium:automationName": "UiAutomator2",
    "appium:udid": "your-device-udid"
  },
  "ios": {
    "appium:app": "/path/to/your/ios/app.ipa",
    "appium:deviceName": "iPhone 15 Pro",
    "appium:platformVersion": "17.0",
    "appium:automationName": "XCUITest",
    "appium:udid": "your-device-udid"
  }
}
```

Set the `CAPABILITIES_CONFIG` environment variable to point to your configuration file.

## 🎯 Available Tools

### Session Management

- `select_platform`: Choose between "android" or "ios".
- `create_session`: Create a new mobile automation session.
- `appium_activate_app`: Activate a specified app.
- `appium_terminate_app`: Terminate a specified app.

### Element Interaction

- `generate_locators`: Generate intelligent locators for all interactive elements on the current screen.
- `appium_find_element`: Find a specific element using various locator strategies.
- `appium_click`: Click on an element.
- `appium_set_value`: Enter text into an input field.
- `appium_get_text`: Retrieve the text content of an element.
- `appium_screenshot`: Capture a screenshot of the current screen.
- `appium_scroll`: Scroll the screen vertically.
- `appium_scroll_to_element`: Scroll until a specific element is found.

### Test Generation

- `appium_generate_tests`: Generate automated test code from natural language scenarios.

## 🤖 Client Support

MCP Appium is designed to be compatible with any MCP-compliant client.

## 📚 Usage Examples

### Amazon Mobile App Checkout Flow

Here's an example prompt to test the Amazon mobile app checkout process:

```
Open Amazon mobile app, search for "iPhone 15 Pro", select the first search result, add the item to cart, proceed to checkout, sign in with email "test@example.com" and password "testpassword123", select shipping address, choose payment method, review order details, and place the order. Use JAVA + TestNG for test generation.
```

This example demonstrates a complete e-commerce checkout flow that can be automated using MCP Appium's intelligent locator generation and test creation capabilities.

## 🙌 Contributing

Contributions are welcome! Please feel free to submit a pull request or open an issue to discuss any changes.

## 📄 License

This project is licensed under the MIT License. See the [LICENSE](LICENSE) file for details.
