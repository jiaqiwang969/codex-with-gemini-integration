# Hunyuan AI3D Integration for Codex

腾讯云混元 AI3D 集成，通过 MCP (Model Context Protocol) 为 Codex 提供 3D 模型生成能力。

## ✨ 功能特性

- **文生3D**: 通过文本描述生成3D模型
- **图生3D**: 从2D图片生成3D模型  
- **Sketch模式**: 文字+图片组合输入（Pro版本）
- **自动化流程**: 提交任务 → 轮询状态 → 下载文件，一站式完成
- **多版本支持**: Professional、Rapid、Standard 三个API版本

## 🚀 快速开始

### 1. 设置API密钥

在环境变量中配置腾讯云密钥：

```bash
export TENCENTCLOUD_SECRET_ID="your-secret-id"
export TENCENTCLOUD_SECRET_KEY="your-secret-key"
```

### 2. 编译运行

```bash
cd codex-rs
cargo build --release -p codex-tui
./target/release/codex-tui
```

### 3. 使用示例

在 Codex 中直接输入：

```
生成一个可爱的机器人3D模型
```

或粘贴图片后：

```
[图片] 基于这个图片生成3D模型
```

## 📊 API版本对比

| 特性 | Professional | Rapid | Standard |
|------|-------------|-------|----------|
| 并发数 | 3 | 1 | - |
| 文本长度 | 1024字符 | 200字符 | - |
| PBR材质 | ✅ | ✅ | ⚠️ |
| 面数控制 | ✅ (40K-1.5M) | ❌ | ⚠️ |
| 生成模式 | 4种 | ❌ | ⚠️ |
| 输出格式 | 自动 | 6种可选 | ⚠️ |
| 多视角输入 | ✅ | ❌ | ⚠️ |

## 📁 输出文件组织

生成的文件保存在 `/tmp/hunyuan-3d/` 目录，使用智能命名：

```
/tmp/hunyuan-3d/{时间戳}_{JobID前8位}_{描述}/
├── model.obj      # 3D模型
├── material.mtl   # 材质文件
└── texture.png    # 纹理贴图
```

## 🛠️ 技术架构

- **语言**: Rust
- **协议**: MCP (Model Context Protocol)
- **认证**: TC3-HMAC-SHA256
- **集成**: 自动检测环境变量并注入配置

## 📝 参数说明

### Professional API 专属
- `GenerateType`: Normal/LowPoly/Geometry/Sketch
- `FaceCount`: 40000-1500000
- `MultiViewImages`: 多视角图片输入
- `PolygonType`: triangle/quadrilateral

### Rapid API 专属
- `ResultFormat`: OBJ/GLB/STL/USDZ/FBX/MP4

## 🔗 相关链接

- [腾讯云API密钥管理](https://console.cloud.tencent.com/cam/capi)
- [混元AI3D产品文档](https://cloud.tencent.com/document/product/1729)

## 📄 许可证

本项目遵循 Codex 主项目的许可证。
