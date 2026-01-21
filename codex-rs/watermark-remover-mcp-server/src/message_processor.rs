//! MCP message processor for watermark remover

use mcp_types::CallToolRequestParams;
use mcp_types::CallToolResult;
use mcp_types::ContentBlock;
use mcp_types::Implementation;
use mcp_types::InitializeRequestParams;
use mcp_types::InitializeResult;
use mcp_types::JSONRPCError;
use mcp_types::JSONRPCErrorError;
use mcp_types::JSONRPCMessage;
use mcp_types::JSONRPCNotification;
use mcp_types::JSONRPCRequest;
use mcp_types::JSONRPCResponse;
use mcp_types::ListToolsResult;
use mcp_types::ServerCapabilities;
use mcp_types::ServerCapabilitiesTools;
use mcp_types::TextContent;
use tokio::sync::mpsc;
use tracing::debug;
use tracing::error;
use tracing::info;

use crate::tools::get_tool_definitions;
use crate::tools::handle_tool_call;

pub enum OutgoingMessage {
    Response(JSONRPCResponse),
    Error(JSONRPCError),
}

impl From<OutgoingMessage> for JSONRPCMessage {
    fn from(msg: OutgoingMessage) -> Self {
        match msg {
            OutgoingMessage::Response(r) => JSONRPCMessage::Response(r),
            OutgoingMessage::Error(e) => JSONRPCMessage::Error(e),
        }
    }
}

pub struct OutgoingMessageSender {
    tx: mpsc::UnboundedSender<OutgoingMessage>,
}

impl OutgoingMessageSender {
    pub fn new(tx: mpsc::UnboundedSender<OutgoingMessage>) -> Self {
        Self { tx }
    }

    pub fn send_response(&self, id: serde_json::Value, result: serde_json::Value) {
        let request_id = if let Some(s) = id.as_str() {
            mcp_types::RequestId::String(s.to_string())
        } else if let Some(i) = id.as_i64() {
            mcp_types::RequestId::Integer(i)
        } else {
            mcp_types::RequestId::String("unknown".to_string())
        };

        let response = JSONRPCResponse {
            jsonrpc: mcp_types::JSONRPC_VERSION.to_string(),
            id: request_id,
            result,
        };
        let _ = self.tx.send(OutgoingMessage::Response(response));
    }

    pub fn send_error(&self, id: serde_json::Value, code: i64, message: String) {
        let request_id = if let Some(s) = id.as_str() {
            mcp_types::RequestId::String(s.to_string())
        } else if let Some(i) = id.as_i64() {
            mcp_types::RequestId::Integer(i)
        } else {
            mcp_types::RequestId::String("unknown".to_string())
        };

        let error = JSONRPCError {
            jsonrpc: mcp_types::JSONRPC_VERSION.to_string(),
            id: request_id,
            error: JSONRPCErrorError {
                code,
                message,
                data: None,
            },
        };
        let _ = self.tx.send(OutgoingMessage::Error(error));
    }
}

pub struct MessageProcessor {
    sender: OutgoingMessageSender,
    initialized: bool,
}

impl MessageProcessor {
    pub fn new(sender: OutgoingMessageSender) -> Self {
        Self {
            sender,
            initialized: false,
        }
    }

    pub async fn process_request(&mut self, request: JSONRPCRequest) {
        debug!("Processing request: {}", request.method);

        let id = serde_json::to_value(request.id.clone()).unwrap_or(serde_json::Value::Null);
        let params = serde_json::to_value(request.params).unwrap_or(serde_json::Value::Null);

        match request.method.as_str() {
            "initialize" => {
                self.handle_initialize(id, params).await;
            }
            "tools/list" => {
                self.handle_list_tools(id, params).await;
            }
            "tools/call" => {
                self.handle_tool_call(id, params).await;
            }
            _ => {
                self.sender.send_error(
                    serde_json::to_value(request.id).unwrap_or(serde_json::Value::Null),
                    -32601,
                    format!("Method not found: {}", request.method),
                );
            }
        }
    }

    pub async fn process_response(&mut self, response: JSONRPCResponse) {
        debug!("Received response: {:?}", response.id);
    }

    pub async fn process_notification(&mut self, notification: JSONRPCNotification) {
        debug!("Received notification: {}", notification.method);
    }

    pub fn process_error(&mut self, error: JSONRPCError) {
        error!(
            "Received error: {} - {}",
            error.error.code, error.error.message
        );
    }

    async fn handle_initialize(&mut self, id: serde_json::Value, params: serde_json::Value) {
        let _request: InitializeRequestParams = match serde_json::from_value(params) {
            Ok(r) => r,
            Err(e) => {
                self.sender
                    .send_error(id, -32602, format!("Invalid params: {e}"));
                return;
            }
        };

        let result = InitializeResult {
            protocol_version: mcp_types::MCP_SCHEMA_VERSION.to_string(),
            capabilities: ServerCapabilities {
                tools: Some(ServerCapabilitiesTools {
                    list_changed: None,
                }),
                prompts: None,
                resources: None,
                logging: None,
                completions: None,
                experimental: None,
            },
            server_info: Implementation {
                name: "watermark-remover-mcp-server".to_string(),
                title: None,
                version: "0.1.0".to_string(),
                user_agent: None,
            },
            instructions: Some("Watermark Remover MCP Server - Remove watermarks from PDF files and images using OpenCV.".to_string()),
        };

        self.initialized = true;
        match serde_json::to_value(result) {
            Ok(val) => self.sender.send_response(id, val),
            Err(e) => self
                .sender
                .send_error(id, -32000, format!("Serialization error: {e}")),
        }
        info!("Initialized Watermark Remover MCP server");
    }

    async fn handle_list_tools(&mut self, id: serde_json::Value, _params: serde_json::Value) {
        if !self.initialized {
            self.sender
                .send_error(id, -32002, "Server not initialized".to_string());
            return;
        }

        let tools = get_tool_definitions();
        let result = ListToolsResult {
            tools,
            next_cursor: None,
        };

        match serde_json::to_value(result) {
            Ok(val) => self.sender.send_response(id, val),
            Err(e) => self
                .sender
                .send_error(id, -32000, format!("Serialization error: {e}")),
        }
    }

    async fn handle_tool_call(&mut self, id: serde_json::Value, params: serde_json::Value) {
        if !self.initialized {
            self.sender
                .send_error(id, -32002, "Server not initialized".to_string());
            return;
        }

        let request: CallToolRequestParams = match serde_json::from_value(params) {
            Ok(r) => r,
            Err(e) => {
                self.sender
                    .send_error(id, -32602, format!("Invalid params: {e}"));
                return;
            }
        };

        match handle_tool_call(request).await {
            Ok(result) => match serde_json::to_value(result) {
                Ok(val) => self.sender.send_response(id, val),
                Err(e) => self
                    .sender
                    .send_error(id, -32000, format!("Serialization error: {e}")),
            },
            Err(e) => {
                let result = CallToolResult {
                    content: vec![ContentBlock::TextContent(TextContent {
                        r#type: "text".to_string(),
                        text: format!("Error: {e}"),
                        annotations: None,
                    })],
                    is_error: Some(true),
                    structured_content: None,
                };
                match serde_json::to_value(result) {
                    Ok(val) => self.sender.send_response(id, val),
                    Err(e) => {
                        self.sender
                            .send_error(id, -32000, format!("Serialization error: {e}"))
                    }
                }
            }
        }
    }
}
