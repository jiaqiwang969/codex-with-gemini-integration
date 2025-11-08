//! Hunyuan AI3D MCP Server
//!
//! This MCP server provides tools for generating 3D models using Tencent Cloud's
//! Hunyuan AI3D API. It supports both text-to-3D and image-to-3D generation with
//! all professional parameters.

#![deny(clippy::print_stdout, clippy::print_stderr)]

use std::io::ErrorKind;
use std::io::Result as IoResult;
use std::path::PathBuf;

use codex_common::CliConfigOverrides;
use mcp_types::JSONRPCMessage;
use tokio::io::AsyncBufReadExt;
use tokio::io::AsyncWriteExt;
use tokio::io::BufReader;
use tokio::io::{self};
use tokio::sync::mpsc;
use tracing::debug;
use tracing::error;
use tracing::info;
use tracing_subscriber::EnvFilter;

pub mod image_utils;
pub mod message_processor;
pub mod models;
pub mod tencent_cloud;
pub mod tools;

use crate::message_processor::MessageProcessor;
use crate::message_processor::OutgoingMessage;
use crate::message_processor::OutgoingMessageSender;

/// Size of the bounded channels used to communicate between tasks
const CHANNEL_CAPACITY: usize = 128;

pub async fn run_main(
    _codex_linux_sandbox_exe: Option<PathBuf>,
    _cli_config_overrides: CliConfigOverrides,
) -> IoResult<()> {
    // Install a simple subscriber so `tracing` output is visible
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(EnvFilter::from_default_env())
        .init();

    // Set up channels
    let (incoming_tx, mut incoming_rx) = mpsc::channel::<JSONRPCMessage>(CHANNEL_CAPACITY);
    let (outgoing_tx, mut outgoing_rx) = mpsc::unbounded_channel::<OutgoingMessage>();

    // Task: read from stdin, push to `incoming_tx`
    let stdin_reader_handle = tokio::spawn({
        async move {
            let stdin = io::stdin();
            let reader = BufReader::new(stdin);
            let mut lines = reader.lines();

            while let Some(line) = lines.next_line().await.unwrap_or_default() {
                match serde_json::from_str::<JSONRPCMessage>(&line) {
                    Ok(msg) => {
                        if incoming_tx.send(msg).await.is_err() {
                            // Receiver gone – nothing left to do
                            break;
                        }
                    }
                    Err(e) => error!("Failed to deserialize JSONRPCMessage: {e}"),
                }
            }

            debug!("stdin reader finished (EOF)");
        }
    });

    // Task: process incoming messages
    let processor_handle = tokio::spawn({
        let outgoing_message_sender = OutgoingMessageSender::new(outgoing_tx);
        let mut processor = MessageProcessor::new(outgoing_message_sender);
        async move {
            while let Some(msg) = incoming_rx.recv().await {
                match msg {
                    JSONRPCMessage::Request(r) => processor.process_request(r).await,
                    JSONRPCMessage::Response(r) => processor.process_response(r).await,
                    JSONRPCMessage::Notification(n) => processor.process_notification(n).await,
                    JSONRPCMessage::Error(e) => processor.process_error(e),
                }
            }

            info!("processor task exited (channel closed)");
        }
    });

    // Task: write outgoing messages to stdout
    let stdout_writer_handle = tokio::spawn(async move {
        let mut stdout = io::stdout();
        while let Some(outgoing_message) = outgoing_rx.recv().await {
            let msg: JSONRPCMessage = outgoing_message.into();
            match serde_json::to_string(&msg) {
                Ok(json) => {
                    if let Err(e) = stdout.write_all(json.as_bytes()).await {
                        error!("Failed to write to stdout: {e}");
                        break;
                    }
                    if let Err(e) = stdout.write_all(b"\n").await {
                        error!("Failed to write newline to stdout: {e}");
                        break;
                    }
                }
                Err(e) => error!("Failed to serialize JSONRPCMessage: {e}"),
            }
        }

        info!("stdout writer exited (channel closed)");
    });

    // Wait for all tasks to finish
    let _ = tokio::join!(stdin_reader_handle, processor_handle, stdout_writer_handle);

    Ok(())
}
