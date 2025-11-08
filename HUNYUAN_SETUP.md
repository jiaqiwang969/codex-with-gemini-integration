# Hunyuan AI3D 配置指南

## 🔑 配置密钥

### 方法1：环境变量（推荐）

在您的 shell 配置文件（如 `~/.zshrc` 或 `~/.bashrc`）中添加：

```bash
export TENCENTCLOUD_SECRET_ID="您的SecretId"
export TENCENTCLOUD_SECRET_KEY="您的SecretKey"
```

然后重新加载配置：
```bash
source ~/.zshrc  # 或 source ~/.bashrc
```

### 方法2：临时设置

在运行 Codex 前临时设置：

```bash
export TENCENTCLOUD_SECRET_ID="您的SecretId"
export TENCENTCLOUD_SECRET_KEY="您的SecretKey"
./target/release/codex-tui
```

### 方法3：使用脚本

修改 `run_codex_with_hunyuan.sh` 中的密钥：

```bash
# 编辑脚本
vim run_codex_with_hunyuan.sh

# 将 YOUR_SECRET_ID_HERE 和 YOUR_SECRET_KEY_HERE 替换为实际密钥
```

## 🔍 获取密钥

1. 登录[腾讯云控制台](https://console.cloud.tencent.com/)
2. 访问[API密钥管理](https://console.cloud.tencent.com/cam/capi)
3. 创建或查看您的 SecretId 和 SecretKey

## ⚠️ 安全注意事项

- **不要**将真实密钥提交到 Git
- **不要**在公开代码中硬编码密钥
- **建议**使用环境变量管理密钥
- **定期**轮换密钥以保证安全

## ✅ 验证配置

运行以下命令验证密钥是否正确配置：

```bash
# 检查环境变量
echo $TENCENTCLOUD_SECRET_ID
echo $TENCENTCLOUD_SECRET_KEY

# 运行 Codex
./target/release/codex-tui

# 在 Codex 中输入
/mcp

# 应该看到 hunyuan-3d 服务器已启动
```
