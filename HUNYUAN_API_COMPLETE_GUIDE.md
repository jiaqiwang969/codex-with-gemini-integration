# 腾讯云混元 AI3D API 完整指南

## 🎯 三个API版本对比

根据[腾讯云API Explorer](https://console.cloud.tencent.com/api/explorer?Product=ai3d&Version=2025-05-13)，混元AI3D提供了三个不同的API版本：

### 1. **Professional API (专业版)**
- **提交接口**: `SubmitHunyuanTo3DProJob`
- **查询接口**: `QueryHunyuanTo3DProJob`
- **并发数**: 3个并发
- **特点**: 功能最全面，参数控制精细

### 2. **Rapid API (极速版)**
- **提交接口**: `SubmitHunyuanTo3DRapidJob`
- **查询接口**: `QueryHunyuanTo3DRapidJob`
- **并发数**: 1个并发
- **特点**: 生成速度快，支持格式选择

### 3. **Standard API (通用版)**
- **提交接口**: `SubmitHunyuanTo3DJob`
- **查询接口**: `QueryHunyuanTo3DJob`
- **并发数**: 待确认
- **特点**: 平衡版本，介于Pro和Rapid之间

## 📊 详细参数支持对比

| 参数名称 | Professional | Rapid | Standard | 说明 |
|---------|--------------|-------|----------|------|
| **基础参数** |
| `Prompt` | ✅ 1024字符 | ✅ 200字符 | ✅ | 文本描述 |
| `ImageBase64` | ✅ | ✅ | ✅ | Base64图片 |
| `ImageUrl` | ✅ | ✅ | ✅ | 图片URL |
| **高级参数** |
| `MultiViewImages` | ✅ | ❌ | ⚠️ | 多视角图片 |
| `EnablePBR` | ✅ | ✅ | ⚠️ | PBR材质 |
| `FaceCount` | ✅ 40K-1.5M | ❌ | ⚠️ | 面数控制 |
| `GenerateType` | ✅ 4种模式 | ❌ | ⚠️ | 生成模式 |
| `PolygonType` | ✅ | ❌ | ⚠️ | 多边形类型 |
| `ResultFormat` | ❌ | ✅ 6种格式 | ⚠️ | 输出格式 |

注：⚠️ 表示Standard API的参数支持需要进一步确认

## 🔧 参数详细说明

### GenerateType (生成模式) - Pro专属
- `Normal`: 标准带纹理的几何模型
- `LowPoly`: 智能减面后的低多边形模型
- `Geometry`: 不带纹理的白模
- `Sketch`: 草图模式，支持文字+图片输入

### ResultFormat (输出格式) - Rapid专属
- `OBJ`: 最通用的3D格式（默认）
- `GLB`: Web友好的二进制glTF格式
- `STL`: 3D打印标准格式
- `USDZ`: Apple生态系统格式
- `FBX`: 游戏引擎常用格式
- `MP4`: 3D模型旋转视频

### PolygonType (多边形类型) - Pro专属
- `triangle`: 三角形面（默认）
- `quadrilateral`: 四边形与三角形混合

## 💡 使用建议

### 选择 Professional API 当您需要：
- 🎨 精细控制生成参数
- 🔧 特定面数要求（游戏/AR/VR）
- 📐 多视角输入生成更精确模型
- 🎭 使用Sketch模式（文字+草图）
- ⚡ 同时处理多个任务（3并发）

### 选择 Rapid API 当您需要：
- 🚀 快速生成结果（30-60秒）
- 📦 特定输出格式（STL/FBX等）
- 💰 成本敏感的批量生成
- 🎯 简单的文生3D或图生3D

### 选择 Standard API 当您需要：
- ⚖️ 平衡速度和质量
- 🔄 兼容性最好的通用方案
- 📊 介于Pro和Rapid之间的功能

## 🛠️ API调用示例

### Professional API
```json
{
  "Prompt": "一个精致的机器人，金属质感，科幻风格",
  "EnablePBR": true,
  "FaceCount": 180000,
  "GenerateType": "Normal",
  "MultiViewImages": [
    {
      "ViewType": "left",
      "ViewImageUrl": "https://example.com/left.jpg"
    }
  ]
}
```

### Rapid API
```json
{
  "Prompt": "一个简单的家具",
  "ResultFormat": "OBJ",
  "EnablePBR": false
}
```

### Standard API
```json
{
  "Prompt": "一个卡通角色",
  "EnablePBR": true
}
```

## 📝 注意事项

1. **字符限制差异**
   - Professional: 最多1024个UTF-8字符
   - Rapid: 最多200个UTF-8字符
   - Standard: 待确认

2. **图片文本互斥规则**
   - 普通模式：`Prompt`和`ImageBase64/ImageUrl`不能同时存在
   - Sketch模式（仅Pro）：可以同时使用文字和图片

3. **参数依赖关系**
   - `PolygonType`仅在`GenerateType=LowPoly`时有效
   - `EnablePBR`在`GenerateType=Geometry`时无效

4. **并发限制**
   - Professional: 3个并发任务
   - Rapid: 1个并发任务
   - Standard: 待确认

## 🔍 错误排查

常见错误及解决方案：

| 错误代码 | 错误信息 | 解决方案 |
|---------|---------|---------|
| `UnknownParameter` | 参数不被识别 | 检查API版本是否支持该参数 |
| `InvalidParameterValue` | 参数值无效 | 检查参数格式和取值范围 |
| `MissingParameter` | 缺少必需参数 | 确保提供Prompt或Image之一 |
| `ResourceInsufficient` | 并发数超限 | 等待其他任务完成 |

## 📚 参考链接

- [Professional API Explorer](https://console.cloud.tencent.com/api/explorer?Product=ai3d&Version=2025-05-13&Action=SubmitHunyuanTo3DProJob)
- [Rapid API Explorer](https://console.cloud.tencent.com/api/explorer?Product=ai3d&Version=2025-05-13&Action=SubmitHunyuanTo3DRapidJob)
- [Standard API Explorer](https://console.cloud.tencent.com/api/explorer?Product=ai3d&Version=2025-05-13&Action=SubmitHunyuanTo3DJob)

## 更新记录

- **2024-11**: 根据腾讯云API Explorer确认三个API版本
  - Professional API: 不支持OutputFormat、NegativePrompt、Seed
  - Rapid API: 支持ResultFormat和EnablePBR
  - Standard API: 新发现的通用版本，参数支持待进一步确认
