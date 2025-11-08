#!/bin/bash

# 运行集成了 Hunyuan AI3D 的 Codex

set -e

echo "🚀 启动集成了 Hunyuan AI3D 的 Codex"
echo "===================================="
echo ""

# 设置腾讯云密钥（请替换为您自己的密钥）
export TENCENTCLOUD_SECRET_ID="YOUR_SECRET_ID_HERE"
export TENCENTCLOUD_SECRET_KEY="YOUR_SECRET_KEY_HERE"

# 构建 codex-tui (会自动构建 hunyuan-mcp-server)
echo "📦 构建中..."
cd codex-rs
cargo build --release -p codex-tui

echo ""
echo "✅ 构建完成！"
echo ""
echo "启动 Codex..."
echo "提示：您现在可以直接使用以下命令："
echo "  - '生成一个可爱的机器人3D模型'"
echo "  - '[粘贴图片] 基于这个图片生成3D模型'"
echo ""

# 运行 Codex
./target/release/codex-tui
