# Ralph Loop 功能实现总结

## ✅ 已完成的工作

### 1. Protocol 层扩展 (protocol/src/)

#### 新增文件：
- `protocol/src/slash_commands.rs` - Slash 命令解析系统
  - `SlashCommand` 枚举：支持 `/ralph-loop`, `/cancel-ralph`, `/help` 等
  - `RalphLoopCommand` 结构：包含 max_iterations, completion_promise, prompt 参数
  - 完整的命令解析逻辑和单元测试

#### 修改文件：
- `protocol/src/protocol.rs`
  - 添加了 `RalphLoopContinueEvent`, `RalphLoopStatusEvent`, `RalphLoopCompleteEvent`
  - 添加了 `RalphCompletionReason` 枚举
  - 添加了 `RalphLoopState` 和 `IterationRecord` 结构
  - 在 `EventMsg` 枚举中添加了 Ralph Loop 相关事件

- `protocol/src/lib.rs`
  - 导出 `slash_commands` 模块

### 2. App-Server 层扩展 (app-server/src/)

#### 新增文件：
- `app-server/src/ralph_loop_handler.rs` - Ralph Loop 核心逻辑
  - `handle_slash_command()` - 处理用户输入的 slash 命令
  - `handle_ralph_loop_activate()` - 激活 Ralph Loop
  - `handle_ralph_loop_cancel()` - 取消 Ralph Loop
  - `should_continue_ralph_loop()` - 检查是否应该继续循环
  - `continue_ralph_loop()` - 继续到下一次迭代
  - `complete_ralph_loop()` - 完成 Ralph Loop

#### 修改文件：
- `app-server/src/codex_message_processor.rs`
  - 在 `TurnSummary` 结构中添加了 `ralph_loop_state: Option<RalphLoopState>`

- `app-server/src/lib.rs`
  - 添加了 `mod ralph_loop_handler;`

---

## 🔧 剩余的集成步骤

### 3. 事件处理集成 (需要完成)

需要修改 `app-server/src/bespoke_event_handling.rs`：

```rust
// 在 EventMsg::TaskComplete 处理中添加 Ralph Loop 检查
EventMsg::TaskComplete(_ev) => {
    // 检查是否需要继续 Ralph Loop
    let should_continue = crate::ralph_loop_handler::should_continue_ralph_loop(
        conversation_id,
        &turn_summary_store,
        "", // TODO: 获取最后的 agent 输出
    ).await;

    if should_continue {
        // 继续 Ralph Loop
        if let Some(prompt) = crate::ralph_loop_handler::continue_ralph_loop(
            conversation_id,
            &turn_summary_store,
            &outgoing,
            "", // TODO: 获取最后的 agent 输出
            false, // TODO: 检查是否有错误
        ).await {
            // 重新提交相同的 prompt
            // TODO: 调用 conversation.submit() 提交 prompt
        }
    } else {
        // 正常完成
        handle_turn_complete(
            conversation_id,
            event_turn_id,
            &outgoing,
            &turn_summary_store,
        ).await;
    }
}

// 添加新的事件处理
EventMsg::RalphLoopContinue(ev) => {
    // 处理 Ralph Loop 继续事件
}

EventMsg::RalphLoopStatus(ev) => {
    // 处理 Ralph Loop 状态更新
}

EventMsg::RalphLoopComplete(ev) => {
    // 处理 Ralph Loop 完成
}
```

### 4. 用户输入处理 (需要完成)

需要在消息处理流程中检测 slash 命令：

```rust
// 在 app-server/src/codex_message_processor.rs 或相关文件中
// 当收到用户消息时，先检查是否是 slash 命令

async fn handle_user_message(&mut self, message: String, conversation_id: ConversationId) {
    // 检查是否是 slash 命令
    if crate::ralph_loop_handler::handle_slash_command(
        &message,
        conversation_id,
        &self.turn_summary_store,
        &self.outgoing,
    ).await.unwrap_or(false) {
        // 是 slash 命令，已处理
        return;
    }

    // 正常的用户消息处理
    // ...
}
```

### 5. TUI 集成 (可选，增强用户体验)

在 `tui/src/chatwidget.rs` 中添加 Ralph Loop 状态显示：

```rust
// 显示 Ralph Loop 状态栏
fn render_ralph_status(&self, area: Rect, buf: &mut Buffer) {
    if let Some(ralph_state) = &self.ralph_state {
        let status_text = format!(
            "🔄 Ralph Loop: {}/{} | Looking for: \"{}\" | /cancel-ralph to stop",
            ralph_state.iteration,
            ralph_state.max_iterations,
            ralph_state.completion_promise
        );
        // 渲染状态栏
    }
}
```

---

## 📝 使用示例

### 基本用法

```bash
# 1. 启动 codex
$ codex

# 2. 正常对话
> 帮我实现一个 REST API，包含 CRUD 操作和测试。完成后输出 COMPLETE

[AI 开始工作，创建了基础结构但测试未完成]

# 3. 激活 Ralph Loop
> /ralph-loop --prompt "帮我实现一个 REST API，包含 CRUD 操作和测试。完成后输出 COMPLETE" --max-iterations 30 --completion-promise "COMPLETE"

🔄 Ralph Loop activated!
   Repeating: "帮我实现一个 REST API，包含 CRUD 操作和测试。完成后输出 COMPLETE"
   Max iterations: 30
   Looking for: "COMPLETE"

━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
🔁 Ralph Loop - Iteration 1/30
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

[AI 继续工作...]

━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
🔁 Ralph Loop - Iteration 2/30
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

[AI 修复问题...]

COMPLETE

🎉 Ralph Loop Completed!
✅ Completion promise detected: "COMPLETE"
📊 Total iterations: 2
⏱️  Duration: 00:05:23

# 4. 继续正常对话
> 现在添加用户认证功能
```

### 取消 Ralph Loop

```bash
> /ralph-loop --prompt "Long running task..."

🔄 Ralph Loop activated!

━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
🔁 Ralph Loop - Iteration 1/50
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

[AI 工作中...]

> /cancel-ralph

🛑 Ralph Loop cancelled by user
📊 Completed 1 iterations before cancellation
```

---

## 🔑 核心优势

### 为什么方案 2 完美避免配置冲突？

1. **单一会话**：从头到尾只有一个 Codex 会话
2. **配置一次性**：启动时加载配置，之后不再改变
3. **权限持久化**：审批决策在会话内有效，不会重复询问
4. **上下文连续**：AI 能看到所有历史对话，每次迭代都能基于之前的工作

### 与外部脚本方案的对比

| 特性 | 外部脚本 | 方案 2 (事件系统) |
|------|---------|------------------|
| 上下文保持 | ❌ 每次丢失 | ✅ 完整保持 |
| 配置冲突 | ❌ 每次继承 | ✅ 一次性设置 |
| 权限询问 | ❌ 可能重复 | ✅ 会话内持久 |
| 用户体验 | ⚠️ 基础 | ✅✅ 最佳 |

---

## 🧪 测试计划

### 单元测试
- ✅ Slash 命令解析测试（已包含在 slash_commands.rs 中）
- TODO: Ralph Loop 状态管理测试
- TODO: 事件处理逻辑测试

### 集成测试
- TODO: 完整的 Ralph Loop 流程测试
- TODO: 取消和恢复测试
- TODO: 错误处理测试

### 手动测试场景
1. 基本循环：简单任务，2-3 次迭代完成
2. 长循环：复杂任务，10+ 次迭代
3. 中途取消：测试 /cancel-ralph 命令
4. 达到最大迭代次数：测试超时处理
5. 错误恢复：测试在有错误时的继续逻辑

---

## 📚 相关文件清单

### 新增文件
- `protocol/src/slash_commands.rs`
- `app-server/src/ralph_loop_handler.rs`

### 修改文件
- `protocol/src/protocol.rs`
- `protocol/src/lib.rs`
- `app-server/src/codex_message_processor.rs`
- `app-server/src/lib.rs`

### 需要修改的文件（剩余工作）
- `app-server/src/bespoke_event_handling.rs` - 添加 Ralph Loop 事件处理
- `app-server/src/message_processor.rs` 或相关文件 - 添加 slash 命令检测
- `tui/src/chatwidget.rs` (可选) - 添加 Ralph Loop 状态显示

---

## 🚀 下一步行动

1. **完成事件处理集成**
   - 修改 `bespoke_event_handling.rs` 中的 `TaskComplete` 处理
   - 添加 Ralph Loop 事件的处理分支

2. **完成用户输入处理**
   - 在消息处理流程中添加 slash 命令检测
   - 确保 slash 命令优先于普通消息处理

3. **测试基本功能**
   - 编译项目：`cargo build`
   - 运行基本测试：`cargo test`
   - 手动测试 Ralph Loop 功能

4. **添加 TUI 支持**（可选）
   - 在 TUI 中显示 Ralph Loop 状态
   - 添加进度指示器

5. **文档和示例**
   - 更新用户文档
   - 添加使用示例

---

## 💡 设计亮点

1. **模块化设计**：Ralph Loop 逻辑独立在 `ralph_loop_handler.rs` 中
2. **最小侵入**：只在必要的地方修改现有代码
3. **类型安全**：充分利用 Rust 的类型系统
4. **异步友好**：所有操作都是异步的，不会阻塞
5. **可扩展**：易于添加新的 slash 命令

---

## 🎯 总结

我们已经完成了 Ralph Loop 功能的核心架构和大部分实现：

- ✅ **Protocol 层**：完整的事件和状态定义
- ✅ **命令解析**：完整的 slash 命令系统
- ✅ **状态管理**：在 TurnSummary 中集成 Ralph Loop 状态
- ✅ **核心逻辑**：激活、取消、继续、完成等功能
- 🔧 **事件集成**：需要在 bespoke_event_handling.rs 中连接
- 🔧 **输入处理**：需要在消息处理流程中添加检测
- 🎨 **TUI 支持**：可选的用户界面增强

剩余的工作主要是将已实现的模块集成到现有的事件处理流程中。
