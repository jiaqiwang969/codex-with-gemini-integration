# Ralph Loop 功能 - 快速开始指南

## 📊 当前进度

✅ **已完成 (70%)**
- Protocol 层定义
- Slash 命令解析系统
- 状态管理扩展
- 核心逻辑实现

🔧 **待完成 (30%)**
- 事件处理集成
- 用户输入处理
- TUI 支持（可选）

---

## 🚀 完成剩余工作的步骤

### 步骤 1: 测试编译

首先确保当前代码可以编译：

```bash
cd codex-rs
cargo build
```

如果有编译错误，主要检查：
- `protocol/src/protocol.rs` 中的 `chrono` 依赖
- 确保 `Cargo.toml` 中有 `chrono = "0.4"` 依赖

---

### 步骤 2: 集成事件处理（最关键）

编辑 `app-server/src/bespoke_event_handling.rs`：

#### 2.1 添加导入

在文件顶部添加：

```rust
use crate::ralph_loop_handler;
use codex_protocol::protocol::RalphCompletionReason;
```

#### 2.2 修改 TaskComplete 处理

找到 `EventMsg::TaskComplete(_ev) =>` 这一行（约在第 95 行），替换为：

```rust
EventMsg::TaskComplete(_ev) => {
    // 检查是否需要继续 Ralph Loop
    // TODO: 需要获取最后的 agent 输出
    let last_output = String::new(); // 临时占位符

    let should_continue = ralph_loop_handler::should_continue_ralph_loop(
        conversation_id,
        &turn_summary_store,
        &last_output,
    ).await;

    if should_continue {
        // 继续 Ralph Loop
        let had_errors = {
            let store = turn_summary_store.lock().await;
            store.get(&conversation_id)
                .and_then(|s| s.last_error.as_ref())
                .is_some()
        };

        if let Some(prompt) = ralph_loop_handler::continue_ralph_loop(
            conversation_id,
            &turn_summary_store,
            &outgoing,
            &last_output,
            had_errors,
        ).await {
            // 重新提交相同的 prompt
            // TODO: 需要调用 conversation.submit() 来提交 prompt
            // 这需要访问 conversation 对象并调用其 submit 方法
            tracing::info!("Ralph Loop continuing with prompt: {}", prompt);
        }
    } else {
        // 检查是否需要完成 Ralph Loop
        {
            let store = turn_summary_store.lock().await;
            if let Some(summary) = store.get(&conversation_id) {
                if summary.ralph_loop_state.is_some() {
                    // Ralph Loop 已完成
                    drop(store); // 释放锁

                    let reason = if last_output.contains("COMPLETE") {
                        RalphCompletionReason::PromiseDetected
                    } else {
                        RalphCompletionReason::MaxIterations
                    };

                    ralph_loop_handler::complete_ralph_loop(
                        conversation_id,
                        &turn_summary_store,
                        &outgoing,
                        reason,
                    ).await;
                }
            }
        }

        // 正常完成
        handle_turn_complete(
            conversation_id,
            event_turn_id,
            &outgoing,
            &turn_summary_store,
        ).await;
    }
}
```

#### 2.3 添加新事件处理

在 `match msg` 块的末尾（在最后一个事件处理之后），添加：

```rust
EventMsg::RalphLoopContinue(ev) => {
    tracing::info!("Ralph Loop continue: iteration {}/{}", ev.iteration, ev.max_iterations);
}

EventMsg::RalphLoopStatus(ev) => {
    tracing::info!("Ralph Loop status: ", ev.message);
}

EventMsg::RalphLoopComplete(ev) => {
    tracing::info!(
        "Ralph Loop complete: {} iterations, reason: {:?}",
        ev.total_iterations,
        ev.completion_reason
    );
}
```

---

### 步骤 3: 集成用户输入处理

这一步需要找到处理用户消息的地方。通常在 `codex_message_processor.rs` 或 `message_processor.rs` 中。

#### 3.1 查找用户消息处理位置

```bash
cd app-server/src
grep -n "SendUserMessage\|user.*message" *.rs
```

#### 3.2 添加 Slash 命令检测

在处理用户消息的函数中，添加：

```rust
// 在处理用户消息之前，检查是否是 slash 命令
if let Ok(is_slash_command) = crate::ralph_loop_handler::handle_slash_command(
    &user_message,
    conversation_id,
    &self.turn_summary_store,
    &self.outgoing,
).await {
    if is_slash_command {
        // 是 slash 命令，已处理，直接返回
        return Ok(());
    }
}

// 继续正常的消息处理
```

---

### 步骤 4: 测试基本功能

#### 4.1 编译项目

```bash
cargo build
```

#### 4.2 运行测试

```bash
cargo test slash_commands
```

#### 4.3 手动测试（如果编译成功）

```bash
# 启动 codex
cargo run --bin codex

# 在 codex 中测试
> /help
> /ralph-loop --prompt "test" --max-iterations 5
> /cancel-ralph
```

---

## 🔍 调试技巧

### 查看日志

```bash
RUST_LOG=debug cargo run --bin codex
```

### 常见问题

1. **编译错误：找不到 chrono**
   ```toml
   # 在 protocol/Cargo.toml 中添加
   [dependencies]
   chrono = "0.4"
   ```

2. **编译错误：找不到 ralph_loop_handler**
   - 确保 `app-server/src/lib.rs` 中有 `mod ralph_loop_handler;`

3. **运行时错误：slash 命令不工作**
   - 检查是否正确集成了用户输入处理
   - 查看日志确认命令是否被解析

---

## 📝 简化版实现（如果遇到困难）

如果完整集成遇到困难，可以先实现一个简化版本：

### 简化版：只实现命令解析和状态管理

1. 保留已完成的代码
2. 暂时跳过自动循环功能
3. 只实现手动触发：
   - `/ralph-loop` 激活状态
   - 用户手动重复发送消息
   - `/cancel-ralph` 取消状态

这样可以先验证基础架构是否正常工作。

---

## 🎯 最小可行产品 (MVP)

如果时间有限，优先实现这些功能：

1. ✅ Slash 命令解析（已完成）
2. ✅ 状态存储（已完成）
3. 🔧 命令响应（需要集成）
4. 🔧 基本的循环逻辑（需要集成）
5. ⏸️ TUI 支持（可以后续添加）

---

## 📚 参考资料

### 关键文件位置

```
codex-rs/
├── protocol/src/
│   ├── protocol.rs          # 事件定义
│   ├── slash_commands.rs    # 命令解析
│   └── lib.rs              # 模块导出
│
├── app-server/src/
│   ├── ralph_loop_handler.rs      # 核心逻辑
│   ├── bespoke_event_handling.rs  # 事件处理（需要修改）
│   ├── codex_message_processor.rs # 消息处理（需要修改）
│   └── lib.rs                     # 模块导出
│
└── RALPH_LOOP_IMPLEMENTATION.md   # 完整文档
```

### 相关代码示例

查看 `ralph_loop_handler.rs` 中的函数签名和文档注释，了解如何使用各个函数。

---

## 💡 下一步建议

1. **先编译通过**
   - 解决所有编译错误
   - 确保测试通过

2. **逐步集成**
   - 先集成事件处理
   - 再集成用户输入
   - 最后添加 TUI

3. **测试驱动**
   - 每完成一个功能就测试
   - 使用日志调试问题

4. **寻求帮助**
   - 如果遇到困难，可以查看 Codex 现有的类似功能实现
   - 参考其他 slash 命令的实现方式

---

## ✅ 完成检查清单

- [ ] 代码编译通过
- [ ] 单元测试通过
- [ ] `/help` 命令显示 Ralph Loop 帮助
- [ ] `/ralph-loop` 命令可以激活状态
- [ ] `/cancel-ralph` 命令可以取消状态
- [ ] 基本的循环逻辑工作
- [ ] 日志输出正确的状态信息

---

## 🎉 成功标志

当你看到以下输出时，说明基本功能已经工作：

```
🔄 Ralph Loop activated!
   Repeating: "your prompt here"
   Max iterations: 50
   Looking for: "COMPLETE"
```

祝你实现顺利！如果遇到问题，可以参考 `RALPH_LOOP_IMPLEMENTATION.md` 中的详细文档。
