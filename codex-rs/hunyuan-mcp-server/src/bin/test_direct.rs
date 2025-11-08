use anyhow::Result;
use serde_json::json;
use tokio::time::Duration;
use tokio::time::sleep;

#[tokio::main]
async fn main() -> Result<()> {
    // 设置日志
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::DEBUG)
        .init();

    println!("Testing Hunyuan MCP Server directly...");

    // 测试初始化
    let init_request = json!({
        "jsonrpc": "2.0",
        "method": "initialize",
        "params": {
            "protocolVersion": "0.1.0",
            "capabilities": {},
            "clientInfo": {
                "name": "test-client",
                "version": "1.0.0"
            }
        },
        "id": 1
    });

    println!("Sending initialize request: {init_request}");

    // 测试工具调用
    let tool_request = json!({
        "jsonrpc": "2.0",
        "method": "tools/call",
        "params": {
            "name": "hunyuan_generate_3d",
            "arguments": {
                "prompt": "一个简单的立方体",
                "api_version": "pro",
                "wait_for_completion": false
            }
        },
        "id": 2
    });

    println!("Tool request prepared: {tool_request}");

    // 直接测试 API
    test_api_directly().await?;

    Ok(())
}

async fn test_api_directly() -> Result<()> {
    use hunyuan_mcp_server::models::ApiVersion;
    use hunyuan_mcp_server::models::Generate3DRequest;
    use hunyuan_mcp_server::tencent_cloud::client::TencentCloudClient;

    println!("\n=== Testing API directly ===");

    let secret_id = std::env::var("TENCENTCLOUD_SECRET_ID")?;
    let secret_key = std::env::var("TENCENTCLOUD_SECRET_KEY")?;

    println!("Using credentials: {}...", &secret_id[..10]);

    let client = TencentCloudClient::new(secret_id, secret_key)?;

    let mut request = Generate3DRequest::default();
    request.prompt = Some("一个简单的立方体".to_string());

    println!("Submitting job with Professional API...");
    match client.submit_job(request.clone(), ApiVersion::Pro).await {
        Ok(resp) => {
            println!("✅ Success! Job ID: {}", resp.job_id);

            // 查询一次状态
            sleep(Duration::from_secs(2)).await;
            println!("Querying job status...");
            match client.query_job(&resp.job_id, ApiVersion::Pro).await {
                Ok(status) => {
                    println!("Job status: {}", status.status);
                    println!("Full response: {status:?}");
                }
                Err(e) => {
                    println!("❌ Query error: {e}");
                }
            }
        }
        Err(e) => {
            println!("❌ Submit error: {e}");
            println!("Error details: {e:?}");

            // 尝试 Rapid API
            println!("\nTrying Rapid API...");
            match client.submit_job(request.clone(), ApiVersion::Rapid).await {
                Ok(resp) => {
                    println!("✅ Rapid API Success! Job ID: {}", resp.job_id);
                }
                Err(e) => {
                    println!("❌ Rapid API error: {e}");
                }
            }

            // 尝试 Standard API
            println!("\nTrying Standard API...");
            match client.submit_job(request, ApiVersion::Standard).await {
                Ok(resp) => {
                    println!("✅ Standard API Success! Job ID: {}", resp.job_id);
                }
                Err(e) => {
                    println!("❌ Standard API error: {e}");
                }
            }
        }
    }

    Ok(())
}
