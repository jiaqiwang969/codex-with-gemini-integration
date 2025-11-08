use anyhow::Result;
use serde_json::json;
use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;

fn main() -> Result<()> {
    println!("🧪 Hunyuan MCP Server Test Suite");
    println!("=================================\n");

    // 设置环境变量
    unsafe {
        std::env::set_var("RUST_LOG", "info");
    }
    
    let secret_id = std::env::var("TENCENTCLOUD_SECRET_ID")
        .expect("Please set TENCENTCLOUD_SECRET_ID");
    let secret_key = std::env::var("TENCENTCLOUD_SECRET_KEY")
        .expect("Please set TENCENTCLOUD_SECRET_KEY");
    
    println!("✅ Credentials loaded: {}...", &secret_id[..10]);
    
    // 启动 MCP 服务器
    println!("\n📡 Starting MCP Server...");
    let mut server = Command::new("./target/release/hunyuan-mcp-server")
        .env("TENCENTCLOUD_SECRET_ID", &secret_id)
        .env("TENCENTCLOUD_SECRET_KEY", &secret_key)
        .env("RUST_LOG", "debug")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    
    let mut stdin = server.stdin.take().expect("Failed to get stdin");
    let stdout = server.stdout.take().expect("Failed to get stdout");
    let stderr = server.stderr.take().expect("Failed to get stderr");
    
    // 启动输出读取线程
    let running = Arc::new(AtomicBool::new(true));
    let running_clone = running.clone();
    
    let stdout_thread = thread::spawn(move || {
        let reader = BufReader::new(stdout);
        for line in reader.lines() {
            if let Ok(line) = line {
                println!("📤 Server: {}", line);
                
                // 解析 JSON 响应
                if let Ok(json) = serde_json::from_str::<serde_json::Value>(&line) {
                    if let Some(result) = json.get("result") {
                        println!("✅ Result: {}", serde_json::to_string_pretty(result).unwrap());
                    }
                    if let Some(error) = json.get("error") {
                        println!("❌ Error: {}", serde_json::to_string_pretty(error).unwrap());
                    }
                }
            }
        }
    });
    
    let stderr_thread = thread::spawn(move || {
        let reader = BufReader::new(stderr);
        for line in reader.lines() {
            if let Ok(line) = line {
                if line.contains("ERROR") || line.contains("WARN") {
                    eprintln!("⚠️  {}", line);
                } else if line.contains("INFO") {
                    println!("ℹ️  {}", line);
                } else if line.contains("DEBUG") {
                    println!("🔍 {}", line);
                }
            }
        }
    });
    
    // 等待服务器启动
    thread::sleep(Duration::from_secs(1));
    
    // 测试序列
    run_test_sequence(&mut stdin)?;
    
    // 等待一段时间以接收响应
    thread::sleep(Duration::from_secs(10));
    
    // 关闭服务器
    running.store(false, Ordering::Relaxed);
    drop(stdin); // 关闭 stdin 以触发服务器退出
    let _ = server.wait();
    
    println!("\n✅ Test completed!");
    
    Ok(())
}

fn run_test_sequence(stdin: &mut impl Write) -> Result<()> {
    println!("\n🚀 Starting test sequence...\n");
    
    // 1. 初始化
    println!("1️⃣  Sending initialize request...");
    let init_request = json!({
        "jsonrpc": "2.0",
        "method": "initialize",
        "params": {
            "protocolVersion": "0.1.0",
            "capabilities": {
                "tools": {
                    "call": true
                }
            },
            "clientInfo": {
                "name": "test-client",
                "version": "1.0.0"
            }
        },
        "id": 1
    });
    send_request(stdin, &init_request)?;
    thread::sleep(Duration::from_millis(500));
    
    // 2. 列出工具
    println!("\n2️⃣  Listing available tools...");
    let list_tools = json!({
        "jsonrpc": "2.0",
        "method": "tools/list",
        "params": {},
        "id": 2
    });
    send_request(stdin, &list_tools)?;
    thread::sleep(Duration::from_millis(500));
    
    // 3. 测试错误处理 - 缺少必需参数
    println!("\n3️⃣  Testing error handling (missing params)...");
    let error_test = json!({
        "jsonrpc": "2.0",
        "method": "tools/call",
        "params": {
            "name": "hunyuan_generate_3d",
            "arguments": {}
        },
        "id": 3
    });
    send_request(stdin, &error_test)?;
    thread::sleep(Duration::from_millis(500));
    
    // 4. 测试简单的文生3D（不等待完成）
    println!("\n4️⃣  Testing text-to-3D (no wait)...");
    let text_to_3d_nowait = json!({
        "jsonrpc": "2.0",
        "method": "tools/call",
        "params": {
            "name": "hunyuan_generate_3d",
            "arguments": {
                "prompt": "一个简单的立方体",
                "api_version": "pro",
                "wait_for_completion": false,
                "output_dir": "/tmp/hunyuan-test"
            }
        },
        "id": 4
    });
    send_request(stdin, &text_to_3d_nowait)?;
    thread::sleep(Duration::from_secs(2));
    
    // 5. 测试不同的 API 版本
    println!("\n5️⃣  Testing different API versions...");
    
    // Pro API
    let pro_test = json!({
        "jsonrpc": "2.0",
        "method": "tools/call",
        "params": {
            "name": "hunyuan_generate_3d",
            "arguments": {
                "prompt": "一个金属球体",
                "api_version": "pro",
                "enable_pbr": true,
                "face_count": 80000,
                "generate_type": "Normal",
                "wait_for_completion": false
            }
        },
        "id": 5
    });
    send_request(stdin, &pro_test)?;
    thread::sleep(Duration::from_secs(1));
    
    // Rapid API
    let rapid_test = json!({
        "jsonrpc": "2.0",
        "method": "tools/call",
        "params": {
            "name": "hunyuan_generate_3d",
            "arguments": {
                "prompt": "一个木制椅子",
                "api_version": "rapid",
                "output_format": "obj",
                "wait_for_completion": false
            }
        },
        "id": 6
    });
    send_request(stdin, &rapid_test)?;
    thread::sleep(Duration::from_secs(1));
    
    // Standard API
    let standard_test = json!({
        "jsonrpc": "2.0",
        "method": "tools/call",
        "params": {
            "name": "hunyuan_generate_3d",
            "arguments": {
                "prompt": "一个玻璃杯",
                "api_version": "standard",
                "wait_for_completion": false
            }
        },
        "id": 7
    });
    send_request(stdin, &standard_test)?;
    thread::sleep(Duration::from_secs(1));
    
    // 6. 测试查询任务状态（需要一个有效的 job_id）
    println!("\n6️⃣  Testing query task (will need valid job_id)...");
    let query_test = json!({
        "jsonrpc": "2.0",
        "method": "tools/call",
        "params": {
            "name": "hunyuan_query_task",
            "arguments": {
                "job_id": "test-job-id-12345",
                "api_version": "pro"
            }
        },
        "id": 8
    });
    send_request(stdin, &query_test)?;
    
    Ok(())
}

fn send_request(stdin: &mut impl Write, request: &serde_json::Value) -> Result<()> {
    let request_str = request.to_string();
    println!("📨 Sending: {}", request_str);
    writeln!(stdin, "{}", request_str)?;
    stdin.flush()?;
    Ok(())
}
