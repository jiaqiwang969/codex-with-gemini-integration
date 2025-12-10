## Veo / 视频模型集成计划（草案）

本计划只描述未来接入 Veo 系列视频模型所需的代码改动点；当前不执行，实现阶段再逐步细化。

### 1. 模型与配置层

- 在 `common/src/model_presets.rs` 中为 Veo 模型增加若干 `ModelPreset`（例如 `veo-3.1-fast-generate-preview`、`veo-3-fast-frames`），用于 `/model` 选择。
- 在 `core/src/model_family.rs` 中：
  - 为 `veo-*` 衍生一个 `ModelFamily { family: "veo", ... }`。
  - 为 family 为 `veo` 的模型设置合理的默认：不启用 reasoning summaries、不启用工具调用等。
- 在 `config.toml` 中约定 Veo 使用的 provider（例如 `model_providers.veo`），指向 `https://api.vectorengine.ai`，并配置鉴权头。

### 2. 复用现有图片引用管线

以下逻辑保持不变，仅在 Veo 通路中作为输入使用：

- `/ref-image` 在 TUI 中的解析与状态维护（`tui/src/chatwidget.rs` 中的 `RefImageManager` 与 `Op::SetReferenceImages` / `Op::ClearReferenceImages`）。
- core 中的引用图状态：
  - `core/src/codex.rs::set_reference_images`：将 `Vec<PathBuf>` 转为内部 `ContentItem::InputImage { image_url }`，再提取 `image_url: String`。
  - `core/src/state/session.rs`：`active_reference_images: Vec<String>` 及其 `set_reference_images`/`clear_reference_images`/`reference_images`。
  - `core/src/client_common.rs`：`Prompt { reference_images: Vec<String>, ... }`。
- 对本地图片的 data URL 生成逻辑（`protocol/src/models.rs::From<Vec<UserInput>> for ResponseInputItem`），无需对 Veo 特化。

### 2.1 技术细节：图片 URL 与 data URL 的处理

- `Prompt.reference_images` 中的 `String` 可能是：
  - 本地文件转出的 data URL（`data:image/png;base64,...`）；
  - 用户直接提供的远程 URL（例如 `https://filesystem.site/cdn/.../actor.png`）。
- 对于 Veo：
  - 不再尝试本地解析 data URL；直接将 `Prompt.reference_images` 原样放入 JSON `images` 数组，由后端服务负责解析/下载。
  - 需要确认 VectorEngine 后端是否接受 data URL 与 HTTP URL 混用；若只接受 HTTP URL，则：
    - 在 Codex 侧增加一个“图片上传接口”（可选），先将 data URL 上传到统一文件服务，返回 `https://filesystem.site/...`，再写入 `images`。
    - 或在配置中明确：Veo 通路仅支持远程 URL，`/ref-image` 在 Veo 模式下需要依赖前端/后端上传，将本地图片转换为 URL。

### 3. ModelClient 与 Veo 调用通路

- 在 `core/src/client.rs` 的 `ModelClient::stream` 中，为 Veo 模型增加分支：
  - 当 `provider.wire_api == WireApi::Gemini` 且 `config.model_family.family == "veo"` 时，调用新的 `stream_veo_video(&self, prompt: &Prompt)`，而非 `stream_gemini`。
- 新增 `stream_veo_video`（非流式、单次调用），负责：
  - 从 `prompt.input` 抽取本轮用户文本 `prompt_text`（可重用已有的内容拼接辅助函数，或添加专用 helper）。
  - 直接使用 `prompt.reference_images: Vec<String>` 作为请求体中的 `images` 字段。
  - 构造并发送 `POST /v1/video/create` 请求（基于 `ModelProviderInfo` 的 `base_url` 和 HTTP 头）：
    - `model`: 使用 `self.config.model` 或映射为 VectorEngine 的模型名（例如 `"veo3.1-fast"`）。
    - `prompt`: `prompt_text`。
    - `images`: `prompt.reference_images`。
    - 其它字段：`aspect_ratio`、`enhance_prompt`、`enable_upsample` 可先使用简化默认或配置项。
  - 解析响应，提取生成的视频文件引用（URL 或文件 ID），并：
    - 下载到 `~/.codex/videos/<conversation_id>/<index>.mp4`。
    - 构造一条 `ResponseItem::Message`，其中 `content` 至少包含一条 `OutputText`，提示保存位置（例如 `"Generated video saved: videos/... · run /open-video to open it"`）。
    - 将该消息封装为标准 `ResponseStream`：`Created` → `OutputItemDone` → `Completed`。

### 3.1 技术细节：请求构造与错误处理

- `prompt_text` 构造：
  - 需要一个 helper，将本轮用户消息中的文本内容拼成一段字符串。
  - 不能直接串联整个历史，而是：
    - 只取最后一条 user 消息（与 `/ref-image ... -- prompt` 对齐）；
    - 或取一个可配置的“最近若干轮摘要”（如后续需要）。
- `model` 字段映射：
  - Codex 内部 `model` 字段（例如 `veo-3.1-fast-generate-preview`）与 VectorEngine API 里的 `model` 值（如 `"veo3.1-fast"`）可能不同。
  - 需要在配置或代码中维护一张映射表（可以挂在 `ModelPreset` 上，增加一个 `api_model: &'static str` 字段，或在 `model_family` 内部映射）。
- HTTP 客户端与重试：
  - 可以复用 `default_client::build_reqwest_client()` 创建 `reqwest::Client`。
  - 通过 `ModelProviderInfo::apply_http_headers` 注入 `Authorization: Bearer ...` 和其他必要头。
  - 对 `429/5xx` 考虑轻量级重试策略（次数和回退时间可配置，避免与现有 SSE 重试逻辑混淆）。
- 响应结构假设：
  - 需要根据 VectorEngine 的实际响应 JSON 确认字段（例如 `{"task_id": "...", "video_url": "..."}` 或 `{"result": {"videos":[...]}}`）。
  - 如果是异步任务（返回 operation id），则：
    - 需要在 `stream_veo_video` 内部实现一个简单的轮询：`GET /v1/video/result?id=...` 直到完成；
    - 要注意轮询间隔与最长等待时间，避免阻塞过久。
  - 错误响应：
    - 将后端的错误信息串进一条 `ResponseItem::Message`（例如 `"Veo video generation failed: ..."`），同时在日志中保留详细错误。

### 3.2 技术细节：视频下载与文件命名

- 下载位置：
  - 复用图片保存逻辑的约定：`~/.codex/images/<conversation_id>/<index>.*`。
  - 视频建议保存到：`~/.codex/videos/<conversation_id>/<index>.mp4`。
- 命名策略：
  - 复用 `tui/src/chatwidget.rs` 中图片的 `next_generated_image_index` 思路，在 core 或 TUI 中维护一个 `next_generated_video_index` 计数器。
  - 使用固定宽度编号（例如 `000000.mp4`）方便在 UI 中排序与查找。
- 下载失败处理：
  - 若 HTTP 下载失败或写入磁盘失败：
    - 在日志中记录具体错误与 URL；
    - 在用户可见层返回一条提示消息（例如 `"Failed to download generated video from <url>: <error>"`），而不是静默失败。
- 视频 MIME 类型：
  - 可根据响应中的 `Content-Type` 决定扩展名（如 `video/mp4` → `.mp4`），若缺失则默认 `.mp4`。

### 4. TUI 层体验（可选增强）

首版可以只依赖文本提示，无需额外类型：

- 复用现有 `/ref-image` 用法：
  - 单图：`/ref-image hero.png -- prompt` → `images: [hero]`（image → video）。
  - 双图：`/ref-image start.png end.png -- prompt` → `images: [start, end]`，在 prompt 中约定「第一张为起始、第二张为结束」（image + last_frame → video）。
  - 多图：`/ref-image a.png b.png c.png -- prompt` → `images: [a, b, c]`（multi-image references → video）。
  - 在 TUI 中复用已有的“保存生成图片”的 UX 模式，为视频新增：
  - 一个 `save_generated_video` 辅助函数（与 `save_generated_image` 类似），负责写入 `~/.codex/videos/...`。
  - 一个 `/open-video` slash 命令（与 `open_last_generated_image` 结构相同），用于在系统播放器中打开最近生成的视频。

### 4.1 技术细节：TUI 与 core 的职责划分

- 保存逻辑放哪一层：
  - 方案 A：在 core 中完成下载与落盘，TUI 只展示一条文本消息和路径。
  - 方案 B：core 返回视频 URL（或 data URL），TUI 收到 `ContentItem::InputImage`/文本后再自行下载。
  - 推荐方案 A：与现有“生成图片保存”逻辑更一致，避免在多个前端重复实现下载逻辑。
- 最近生成视频的路径缓存：
  - 类似 `last_generated_image_path: Option<PathBuf>`，增加 `last_generated_video_path: Option<PathBuf>`。
  - `/open-video` 仅依赖这个状态，不需要重新扫描目录。
- 键盘与 UI 提示：
  - 在 TUI 状态栏或历史中增加简短提示，例如 `"Veo job submitted..."` / `"Veo video saved to ..."`。
  - 避免在 Veo 调用过程中误把会话状态显示为“就绪”，可复用现有的 `is_task_running` 标志。

### 5. 文档与使用说明

- 在 `docs/` 下新增一份 Veo 使用说明草案（例如 `docs/veo_video_user_guide.tex`），结构参考 `docs/gemini_3_pro_image_user_guide.tex`：
  - 说明 `/model veo-*` 与 `/ref-image` 组合的四种常见模式：
    - 纯文本（无 `/ref-image`）：text → video。
    - 单图 + prompt：image → video。
    - 两图 + prompt：image + last_frame → video（通过 `images[0]` / `images[1]` 顺序约定）。
    - 多图 + prompt：multi-image references → video。
- 在根 `README.md` 或相关文档中，增加一小节链接到上述用户指南，标明 Veo 目前为预览/付费特性。

以上为初始实现计划，后续在真正动手时可按优先级拆成更细的任务（例如先支持 text → video 和单图 → video，再迭代起始/结束帧、多图参考等）。
