# Codex + Hunyuan AI3D 集成使用指南

## 🎉 恭喜！Hunyuan AI3D 已内置到 Codex 中

现在您只需要：

## 1. 设置环境变量

```bash
export TENCENTCLOUD_SECRET_ID="您的密钥ID"
export TENCENTCLOUD_SECRET_KEY="您的密钥"
```

## 2. 构建 Codex

```bash
cd codex-rs
cargo build --release -p codex-tui -p hunyuan-mcp-server
```

## 3. 运行 Codex

```bash
./target/release/codex-tui
```

就这么简单！Hunyuan AI3D MCP 服务器会自动启动并集成到 Codex 中。

## 使用方法

在 Codex 对话框中，您可以直接输入：

### 文生3D
```
生成一个可爱的机器人3D模型
```

### 图生3D
```
[粘贴或拖拽图片]
基于这个图片生成3D模型
```

### Sketch模式（文字+图片）
```
[粘贴草图]
基于这个草图生成一个科幻风格的机器人，要有发光的眼睛和金属质感
```

## 工作原理

1. **自动检测**: Codex 启动时会自动检测环境变量中的腾讯云密钥
2. **自动配置**: 如果密钥存在，自动配置并启动 hunyuan-mcp-server
3. **无缝集成**: MCP 服务器作为内置服务运行，无需额外配置
4. **一站式流程**: 自动提交任务、轮询状态、下载文件

## 输出位置

生成的3D模型文件会保存在：
```
outputs/hunyuan/
├── {job_id}_preview.png    # 预览图
├── model.obj               # 3D模型
├── material.mtl            # 材质文件
└── texture.png             # 纹理贴图
```

## 技术细节

- **二进制文件位置**: 
  - `target/release/codex-tui` - 主程序
  - `target/release/hunyuan-mcp-server` - MCP服务器

- **自动启动机制**:
  - 在 `core/src/config/mod.rs` 中实现
  - 检测环境变量并自动添加 MCP 服务器配置
  - 与 codex-tui 同目录的 hunyuan-mcp-server 会被自动发现

## 故障排除

### 如果 MCP 服务器没有启动：

1. 检查环境变量是否设置正确：
```bash
echo $TENCENTCLOUD_SECRET_ID
echo $TENCENTCLOUD_SECRET_KEY
```

2. 查看可用的 MCP 工具：
在 Codex 中输入：
```
/mcp
```

3. 如果看到 "hunyuan-3d" 服务器和相关工具，说明集成成功。

## 总结

通过这个集成，您现在拥有了一个强大的 AI 3D 生成工具，完全内置在 Codex 中：

- ✅ 无需配置文件
- ✅ 自动启动和管理
- ✅ 一站式生成流程
- ✅ 支持多种输入模式
- ✅ 专业级3D模型输出

享受您的3D创作之旅！🚀
