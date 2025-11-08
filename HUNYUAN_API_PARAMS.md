# Hunyuan AI3D API 参数支持对照表

## 📊 Professional API vs Rapid API 参数支持

| 参数名称 | Professional API | Rapid API | 说明 |
|---------|-----------------|-----------|------|
| **Prompt** | ✅ 支持(1024字符) | ✅ 支持(200字符) | 文本描述，注意字符限制差异 |
| **ImageBase64** | ✅ 支持 | ✅ 支持 | Base64编码的图片 |
| **ImageUrl** | ✅ 支持 | ✅ 支持 | 图片URL |
| **MultiViewImages** | ✅ 支持 | ❌ 不支持 | 多视角图片(left/right/back) |
| **EnablePBR** | ✅ 支持 | ✅ 支持 | PBR材质生成 |
| **FaceCount** | ✅ 支持 | ❌ 不支持 | 面数控制(40K-1.5M) |
| **GenerateType** | ✅ 支持 | ❌ 不支持 | 生成模式(Normal/LowPoly/Geometry/Sketch) |
| **PolygonType** | ✅ 支持 | ❌ 不支持 | 多边形类型(仅LowPoly模式) |
| **ResultFormat** | ❌ 不支持 | ✅ 支持 | 输出格式(OBJ/GLB/STL/USDZ/FBX/MP4) |
| **NegativePrompt** | ❌ 不支持 | ❌ 不支持 | 负面提示词(两个API都不支持) |
| **Seed** | ❌ 不支持 | ❌ 不支持 | 随机种子(两个API都不支持) |

## ⚠️ 重要提示

### Professional API 限制
根据官方API文档，Professional API **不支持**以下参数：
- ❌ `OutputFormat` - API会自动选择最优格式
- ❌ `NegativePrompt` - 不支持负面提示词
- ❌ `Seed` - 不支持随机种子控制

### 参数使用规则

1. **图片和文本互斥**（除Sketch模式外）
   - 普通模式：`Prompt` 和 `ImageBase64/ImageUrl` 不能同时存在
   - Sketch模式：可以同时使用文本和图片

2. **PolygonType 仅在 LowPoly 模式有效**
   - 必须设置 `GenerateType: "LowPoly"` 才能使用 `PolygonType`

3. **EnablePBR 在 Geometry 模式无效**
   - 当 `GenerateType: "Geometry"`（白模）时，PBR参数不生效

## 🎯 推荐使用方式

### Professional API（默认）
适合需要精细控制的场景：
```json
{
  "prompt": "一个可爱的机器人",
  "enable_pbr": true,
  "face_count": 180000,
  "generate_type": "Normal"
}
```

### Rapid API
适合快速生成的场景：
```json
{
  "prompt": "一个简单的家具",
  "api_version": "rapid"
}
```

### Sketch模式（仅Pro支持）
文字+图片组合输入：
```json
{
  "prompt": "金属质感，科幻风格",
  "image_url": "草图URL",
  "generate_type": "Sketch"
}
```

## 🔍 错误排查

如果遇到 `UnknownParameter` 错误：
1. 检查是否使用了该API版本不支持的参数
2. Professional API 不要使用 `NegativePrompt`、`Seed`、`OutputFormat`
3. Rapid API 不要使用高级参数如 `EnablePBR`、`FaceCount` 等

## 📝 更新记录

- 2024-11: 根据实际API测试结果更新
  - Professional API 移除 `OutputFormat` 支持
  - Professional API 移除 `NegativePrompt` 和 `Seed` 支持
  - 确认 Rapid API 自动输出 OBJ 格式
