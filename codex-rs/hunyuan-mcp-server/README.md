# Hunyuan AI3D MCP Server

腾讯云混元 AI3D MCP 服务器 - 通过 MCP 协议为 Codex 提供 3D 模型生成能力。

## 功能特性

- 🎨 **文生3D**: 通过文本描述生成3D模型
- 🖼️ **图生3D**: 从2D图片生成3D模型  
- 🎯 **Sketch模式**: 支持文字+图片组合输入，生成更精确的3D模型
- 🚀 **一站式流程**: 自动提交任务、轮询状态、下载文件，一个命令完成所有操作
- 📦 **完整参数支持**: 支持专业版所有高级参数
- 🔄 **自动文件处理**: 自动下载、解压ZIP文件，整理输出目录
- 🖥️ **无缝集成**: 通过 MCP 协议与 Codex 完美集成

## 安装配置

### 1. 设置腾讯云密钥

```bash
export TENCENTCLOUD_SECRET_ID='你的密钥ID'
export TENCENTCLOUD_SECRET_KEY='你的密钥'
```

### 2. 配置 Codex

在 `~/.codex/config.toml` 中添加：

```toml
[mcp_servers.hunyuan_ai3d]
command = "hunyuan-mcp-server"
env = { 
  TENCENTCLOUD_SECRET_ID = "${TENCENTCLOUD_SECRET_ID}",
  TENCENTCLOUD_SECRET_KEY = "${TENCENTCLOUD_SECRET_KEY}"
}
startup_timeout_sec = 30
tool_timeout_sec = 600  # 3D 生成可能需要较长时间
```

## 使用方式

### 工作流 1：纯文本生成3D

```
用户：生成一个可爱的卡通猫咪的3D模型
Agent：我将为您生成一个可爱的卡通猫咪3D模型。
[调用 hunyuan_generate_3d，参数 prompt="一个可爱的卡通猫咪"]
```

### 工作流 2：用户粘贴图片生成3D

```
用户：[粘贴图片到对话框] 基于这个图片生成3D模型
Agent：我看到了您提供的图片，我将基于它生成3D模型。
[自动提取图片并调用 hunyuan_generate_3d]
```

### 工作流 3：文本+图片组合（自动 Sketch 模式）

```
用户：[粘贴图片] 生成一个科幻风格的机器人，参考这个设计
Agent：我将基于您的图片和描述生成3D模型。
[自动使用 Sketch 模式，同时处理文本和图片]
```

## 工具说明

### hunyuan_generate_3d

生成3D模型的主要工具。

**参数：**
- `prompt` (可选): 文本描述
- `image_url` (可选): 图像URL或data URL
- `image_base64` (可选): Base64编码的图像
- `output_format` (可选): 输出格式 (glb/fbx/obj/usdz)，默认 glb
- `api_version` (可选): API版本 (pro/rapid)，默认 pro
- `generate_type` (可选): 生成模式 (Normal/LowPoly/Geometry/Sketch)
- `enable_pbr` (可选): 是否启用PBR材质
- `face_count` (可选): 模型面数 (40000-1500000)
- `polygon_type` (可选): 多边形类型 (仅LowPoly模式)
- `wait_for_completion` (可选): 是否等待完成，默认 true
- `output_dir` (可选): 输出目录，默认 "outputs/hunyuan"

### hunyuan_query_task

查询任务状态。

**参数：**
- `job_id` (必需): 任务ID
- `api_version` (可选): API版本，默认 pro

### hunyuan_download_results

下载生成的3D模型文件。

**参数：**
- `job_id` (必需): 任务ID
- `api_version` (可选): API版本，默认 pro
- `output_dir` (可选): 输出目录，默认 "outputs/hunyuan"

## API 参数详解

### 专业版支持的高级参数

| 参数 | 类型 | 说明 | 可选值 |
|------|------|------|--------|
| GenerateType | string | 生成模式 | Normal/LowPoly/Geometry/Sketch |
| EnablePBR | boolean | PBR材质 | true/false |
| FaceCount | integer | 面数限制 | 40000-1500000 |
| PolygonType | string | 多边形类型 | triangle/quadrilateral（仅LowPoly） |
| Seed | integer | 随机种子 | >=0 |

### 特殊模式说明

- **Sketch模式**: 允许同时输入文本和图片，适合基于草图或线稿生成模型
- **LowPoly模式**: 生成低多边形风格的模型，可选择三角形或四边形
- **Geometry模式**: 生成不带纹理的几何模型（白模）

## 图像处理

服务器自动处理多种图像输入格式：
- Data URLs (来自 view_image 工具或用户粘贴)
- 本地文件路径 (自动转换为 base64)
- 远程 URLs (自动下载并转换)
- 直接的 base64 字符串

图像验证：
- 尺寸要求：128-5000px
- 大小限制：< 6MB (base64编码后)
- 格式支持：jpg, png, jpeg, webp

## 开发

### 构建

```bash
cd codex-rs
cargo build -p hunyuan-mcp-server
```

### 测试

```bash
cargo test -p hunyuan-mcp-server
```

### 格式化

```bash
just fmt
```

## 故障排除

### 凭据错误
确保设置了环境变量：
```bash
echo $TENCENTCLOUD_SECRET_ID
echo $TENCENTCLOUD_SECRET_KEY
```

### 网络问题
如果在沙箱环境中运行，确保允许网络访问：
```bash
codex --sandbox workspace-write
```

### 任务超时
专业版生成可能需要几分钟，可以：
- 增加 `tool_timeout_sec` 配置
- 使用 `wait_for_completion: false` 异步提交
- 使用 `hunyuan_query_task` 手动查询状态

## 许可证

MIT License

