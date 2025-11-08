# Hunyuan AI3D MCP Server

腾讯云混元 AI3D MCP 服务器 - 为 Codex 提供 3D 模型生成能力。

## 功能特性

- **文生3D**: 通过文本描述生成3D模型
- **图生3D**: 从2D图片生成3D模型  
- **Sketch模式**: 文字+图片组合输入（Pro版本）
- **自动化流程**: 提交任务 → 轮询状态 → 下载文件

## 技术架构

- **语言**: Rust
- **协议**: MCP (Model Context Protocol)
- **认证**: TC3-HMAC-SHA256
- **API版本**: Professional / Rapid / Standard

## 项目结构

```
src/
├── lib.rs               # MCP服务器主逻辑
├── models.rs            # 数据模型
├── message_processor.rs # 消息处理
├── image_utils.rs       # 图片工具
├── tencent_cloud/       # 腾讯云API
│   ├── auth.rs         # TC3认证
│   └── client.rs       # API客户端
└── tools/              # MCP工具
    ├── generate.rs     # 生成工具
    ├── query.rs        # 查询工具
    └── download.rs     # 下载工具
```

## 许可证

本项目遵循 Codex 主项目的许可证。