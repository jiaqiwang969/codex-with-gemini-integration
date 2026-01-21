# Codex Ralph Loop 实现方案 - 基于 Claude Code 的设计

## 核心洞察

通过学习 Claude Code 的 ralph-wiggum 插件，我们发现关键机制是：

**Stop Hook 拦截 + 提示重注入 = 自引用循环**

## Codex 的实现策略

由于 Codex 的架构与 Claude Code 不同，我们需要在 **事件处理层** 实现类似的拦截机制。

### 架构对比

| Claude Code | Codex | 实现方式 |
|-------------|-------|---------|
| Stop Hook | TaskComplete Event | 在 TaskComplete 时拦截 |
| 环境变量 | TurnSummary.ralph_loop_state | 会话内状态存储 |
| 提示重注入 | conversation.submit() | 重新提交相同 prompt |
| 文件持久化 | 文件系统 + Git | 相同机制 |

---

## 完整实现方案

### 1. 核心拦截逻辑（类似 Stop Hook）

在 `app-server/src/bespoke_event_handling.rs` 中实现：

```rust
EventMsg::TaskComplete(_ev) => {
    // ============ Ralph Loop 拦截点（类似 Stop Hook）============

    // 1. 检查是否有活跃的 Ralph Loop
    let ralph_state = {
        let store = turn_summary_store.lock().await;
        store.get(&conversation_id)
            .and_then(|s| s.ralph_loop_state.clone())
    };

    if let Some(mut state) = ralph_state {
        if state.enabled {
            // 2. 获取最后的 agent 输出（从 conversation 历史中）
            let last_output = get_last_agent_output(&conversation).await;

            // 3. 检查完成条件（类似 check_completion）
            let should_continue = state.should_continue(&last_output);

            if should_continue {
                // 4. 未完成 - 继续循环（类似 Stop Hook 返回 1）

                // 更新迭代计数
                let had_errors = {
                    let store = turn_summary_store.lock().await;
                    store.get(&conversation_id)
                        .and_then(|s| s.last_error.as_ref())
                        .is_some()
                };

                state.next_iteration(
                    truncate_string(&last_output, 200),
                    had_errors,
                );

                // 保存更新后的状态
                {
                    let mut store = turn_summary_store.lock().await;
                    if let Some(summary) = store.get_mut(&conversation_id) {
                        summary.ralph_loop_state = Some(state.clone());
                    }
                }

                // 发送状态通知
                send_ralph_status_notification(
                    &outgoing,
                    conversation_id,
                    state.iteration,
                    state.max_iterations,
                    api_version,
                ).await;

                // ============ 关键：重新注入提示 ============
                // 构建增强的提示（包含上下文信息）
                let enhanced_prompt = format!(
                    "

---
## Ralph Loop Context
Iteration: {}/{}
Previous work visible in files and git history.
Review your changes and continue improving.
",
                    state.original_prompt,
                    state.iteration,
                    state.max_iterations
                );

                // 重新提交到 conversation（类似 claude-code-inject）
                let op = codex_protocol::protocol::Op::UserMessage {
                    text: enhanced_prompt,
                    attachments: vec![],
                };

                if let Err(e) = conversation.submit(op).await {
                    tracing::error!("Failed to resubmit Ralph Loop prompt: {}", e);
                }

                // 不调用 handle_turn_complete，直接返回
                // 这样就"拦截"了正常的完成流程
                return;
            } else {
                // 5. 已完成 - 允许正常退出（类似 Stop Hook 返回 0）

                // 确定完成原因
                let reason = if last_output.contains(&state.completion_promise) {
                    codex_protocol::protocol::RalphCompletionReason::PromiseDetected
                } else {
                    codex_protocol::protocol::RalphCompletionReason::MaxIterations
                };

                // 发送完成通知
                send_ralph_complete_notification(
                    &outgoing,
                    conversation_id,
                    state.iteration,
                    reason,
                    &state.started_at,
                    api_version,
                ).await;

                // 清除 Ralph Loop 状态
                {
                    let mut store = turn_summary_store.lock().await;
                    if let Some(summary) = store.get_mut(&conversation_id) {
                        summary.ralph_loop_state = None;
                    }
                }
            }
        }
    }

    // 正常的 TaskComplete 处理
    handle_turn_complete(
        conversation_id,
        event_turn_id,
        &outgoing,
        &turn_summary_store,
    ).await;
}
```

### 2. 辅助函数实现

```rust
// 获取最后的 agent 输出
async fn get_last_agent_output(conversation: &Arc<CodexConversation>) -> String {
    // 从 conversation 的历史中获取最后一条 agent 消息
    // 这需要访问 conversation 的内部状态

    // 临时实现：返回空字符串
    // TODO: 实现从 rollout 或 conversation 历史中读取
    String::new()
}

// 发送 Ralph Loop 状态通知
async fn send_ralph_status_notification(
    outgoing: &Arc<OutgoingMessageSender>,
    conversation_id: ConversationId,
    iteration: u32,
    max_iterations: u32,
    api_version: ApiVersion,
) {
    let message = format!(
        "🔁 Ralph Loop - Iteration {}/{}",
        iteration,
        max_iterations
    );

    tracing::info!("{}", message);

    // TODO: 发送实际的通知到客户端
    // 可以使用 AgentMessage 或自定义通知类型
}

// 发送 Ralph Loop 完成通知
async fn send_ralph_complete_notification(
    outgoing: &Arc<OutgoingMessageSender>,
    conversation_id: ConversationId,
    total_iterations: u32,
    reason: codex_protocol::protocol::RalphCompletionReason,
    started_at: &str,
    api_version: ApiVersion,
) {
    let duration = calculate_duration(started_at);

    let message = format!(
        "🎉 Ralph Loop Completed!\n✅ Reason: {:?}\n📊 Total iterations: {}\n⏱️  Duration: {:.2}s",
        reason,
        total_iterations,
        duration
    );

    tracing::info!("{}", message);

    // TODO: 发送实际的通知到客户端
}

fn calculate_duration(started_at: &str) -> f64 {
    if let Ok(start) = chrono::DateTime::parse_from_rfc3339(started_at) {
        let now = chrono::Utc::now();
        let duration = now.signed_duration_since(start);
        duration.num_milliseconds() as f64 / 1000.0
    } else {
        0.0
    }
}

fn truncate_string(s: &str, max_len: usize) -> String {
    if s.len() <= max_len {
        s.to_string()
    } else {
        format!("{}...", &s[..max_len])
    }
}
```

---

## 关键设计决策

### 1. 为什么在 TaskComplete 拦截？

| Claude Code | Codex |
|-------------|-------|
| Stop Hook 在会话退出时触发 | TaskComplete 在 AI 完成工作时触发 |
| 拦截退出 = 阻止会话结束 | 拦截 TaskComplete = 不调用 handle_turn_complete |
| 重新注入提示 = 继续会话 | 重新 submit = 开始新 turn |

**效果相同：AI 看到自己的工作，继续迭代**

### 2. 状态存储位置

```rust
// TurnSummary 中的 ralph_loop_state
pub(crate) struct TurnSummary {
    pub(crate) file_change_started: HashSet<String>,
    pub(crate) last_error: Option<TurnError>,
    pub(crate) ralph_loop_state: Option<RalphLoopState>, // ← 这里
}
```

**优势：**
- 与 conversation 生命周期绑定
- 自动清理（conversation 结束时）
- 线程安全（通过 Mutex）

### 3. 提示重注入机制

```rust
// 类似 Claude Code 的 claude-code-inject
conversation.submit(Op::UserMessage {
    text: enhanced_prompt,
    attachments: vec![],
}).await
```

**关键点：**
- 使用相同的 `conversation` 对象
- 保持会话连续性
- AI 能看到之前的所有工作

---

## 完整的执行流程

```
用户: /ralph-loop "Build API. Output COMPLETE when done." -n 30

1. [ralph_loop_handler::handle_slash_command]
   ├─ 解析命令参数
   ├─ 创建 RalphLoopState
   └─ 存储到 TurnSummary

2. [首次提交]
   ├─ 提交原始 prompt
   └─ AI 开始工作

3. [AI 工作完成]
   └─ 触发 EventMsg::TaskComplete

4. [bespoke_event_handling::apply_bespoke_event_handling]
   ├─ 检查 ralph_loop_state
   ├─ 调用 should_continue()
   │   ├─ 检查 iteration < max_iterations
   │   └─ 检查 output 是否包含 completion_promise
   │
   ├─ 如果 should_continue == true:
   │   ├─ next_iteration()
   │   ├─ 发送状态通知
   │   ├─ 重新 submit(enhanced_prompt)
   │   └─ return (不调用 handle_turn_complete)
   │
   └─ 如果 should_continue == false:
       ├─ 发送完成通知
       ├─ 清除 ralph_loop_state
       └─ 调用 handle_turn_complete()

5. [循环继续]
   └─ 回到步骤 2（使用增强的 prompt）

6. [最终完成]
   └─ 正常退出
```

---

## 与 Claude Code 的对比

| 特性 | Claude Code | Codex | 状态 |
|------|-------------|-------|------|
| 拦截机制 | Stop Hook | TaskComplete Event | ✅ 等效 |
| 状态存储 | 环境变量 | TurnSummary | ✅ 更好 |
| 提示重注入 | Shell 脚本 | Rust 异步 | ✅ 更可靠 |
| 完成检测 | grep 文本 | 字符串匹配 | ✅ 相同 |
| 上下文保持 | 文件 + Git | 文件 + Git + 会话 | ✅ 更强 |
| 迭代限制 | 环境变量 | 结构体字段 | ✅ 更安全 |

---

## 实现优先级

### Phase 1: 核心循环（必需）
1. ✅ 已完成：Protocol 定义
2. ✅ 已完成：Slash 命令解析
3. ✅ 已完成：状态管理
4. 🔧 待完成：TaskComplete 拦截逻辑
5. 🔧 待完成：提示重注入

### Phase 2: 增强功能（重要）
6. 🔧 待完成：获取 agent 输出
7. 🔧 待完成：发送通知到客户端
8. 🔧 待完成：用户输入处理

### Phase 3: 用户体验（可选）
9. ⏸️ 待实现：TUI 状态显示
10. ⏸️ 待实现：进度条
11. ⏸️ 待实现：详细日志

---

## 下一步行动

### 立即执行

1. **完成 TaskComplete 拦截逻辑**
   - 编辑 `app-server/src/bespoke_event_handling.rs`
   - 实现上述的拦截代码

2. **实现 get_last_agent_output**
   - 从 conversation 或 rollout 中读取最后的输出
   - 这是完成检测的关键

3. **测试基本循环**
   - 编译：`cargo build`
   - 运行：`cargo run --bin codex`
   - 测试：`/ralph-loop "test" -n 3`

### 技术挑战

1. **访问 conversation 历史**
   - 需要找到 Codex 中读取消息历史的 API
   - 可能需要查看 `CodexConversation` 的实现

2. **提示重注入**
   - 确保 `conversation.submit()` 正确工作
   - 验证新 turn 能看到之前的文件修改

3. **通知发送**
   - 找到正确的通知发送方式
   - 确保客户端能接收并显示

---

## 成功标准

当看到以下输出时，说明实现成功：

```
> /ralph-loop "Build API. Output COMPLETE when done." -n 5

🔄 Ralph Loop activated!

━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
🔁 Ralph Loop - Iteration 1/5
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

[AI 工作...]

━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
🔁 Ralph Loop - Iteration 2/5
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

[AI 继续工作，能看到之前的文件修改...]

COMPLETE

🎉 Ralph Loop Completed!
✅ Reason: PromiseDetected
📊 Total iterations: 2
⏱️  Duration: 00:05:23
```

---

## 总结

通过学习 Claude Code 的 ralph-wiggum 实现，我们现在有了清晰的实现路径：

1. **核心机制**：在 TaskComplete 事件中拦截，检查完成条件
2. **关键操作**：如果未完成，重新 submit 相同的 prompt
3. **状态管理**：使用 TurnSummary 存储循环状态
4. **上下文保持**：利用 Codex 的会话机制，AI 自然能看到之前的工作

这个设计完全符合 Ralph Loop 的核心理念：**自引用反馈循环**。
