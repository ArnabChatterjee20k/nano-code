use core::fmt;
use std::collections::HashMap;

use futures::StreamExt;

use async_openai::{
    Client,
    config::OpenAIConfig,
    types::{
        ChatCompletionTool, ChatCompletionToolChoiceOption, ChatCompletionToolType,
        CreateChatCompletionRequest, CreateChatCompletionRequestArgs, FinishReason, FunctionObject,
    },
};
use serde_json::Value;

use crate::memory::{Memory, Message};
use crate::tools::ToolResult;

const DEFAULT_MODEL: &str = "google/gemini-3.5-flash";
// agent streams come in chunks so using an enum to stream that
pub enum AgentEvent {
    TextChunk(String),
    ToolChunk(String, serde_json::Value),
    Error(String),
}
pub type AgentEventStream<'a> =
    std::pin::Pin<Box<dyn futures::Stream<Item = AgentEvent> + Send + 'a>>;
pub struct Agent {
    pub client: Client<OpenAIConfig>,
    tools: HashMap<String, Tool>,
}

pub struct Tool {
    pub name: String,
    pub description: String,
    pub callback: fn(serde_json::Value) -> Result<String, Box<dyn std::error::Error + Send + Sync>>,
    pub parameters: Option<serde_json::Value>,
}

#[derive(Debug)]
pub enum ToolError {
    ToolNotFoundError(String),
}

impl fmt::Display for ToolError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ToolError::ToolNotFoundError(name) => {
                write!(f, "Tool '{}' not available", name)
            }
        }
    }
}

impl Agent {
    pub fn new(tools: Vec<Tool>) -> Self {
        // if want to directly get the string value then use std::env::var("API_KEY").unwrap() -> will panic in case of error
        // if want to throw custom error then std::env::var("API_KEY").expect("MESSAGE")
        let env_api_key = std::env::var("API_KEY").ok();
        let env_api_base = std::env::var("API_BASE_URL").unwrap();
        let config = if let Some(api_key) = env_api_key {
            OpenAIConfig::new()
                .with_api_key(api_key)
                .with_api_base(env_api_base.to_string())
        } else {
            OpenAIConfig::default()
        };
        let client = Client::with_config(config);
        let tools_schema =
            HashMap::from_iter(tools.into_iter().map(|tool| (tool.name.clone(), tool)));

        return Agent {
            client,
            tools: tools_schema,
        };
    }

    pub fn call_tool(&self, name: &str, args: Value) -> ToolResult {
        let tool = self
            .tools
            .get(name)
            .ok_or_else(|| ToolError::ToolNotFoundError(name.to_string()).to_string())?;

        (tool.callback)(args)
    }

    pub fn chat<'a>(&'a self, prompt: &'a str, memory: &'a mut Memory) -> AgentEventStream<'a> {
        let model = std::env::var("MODEL_NAME").unwrap_or(DEFAULT_MODEL.to_string());
        let stream = async_stream::stream! {
            #[derive(Default)]
            struct PendingToolCall {
                id: Option<String>,
                name: Option<String>,
                arguments: String,
            }

            if let Err(e) = memory.update(Message::User(prompt.into())) {
                yield AgentEvent::Error(format!("Failed to update memory: {}", e));
                return;
            }

            // build tool definitions from the registered tools so the model can call them
            let tools_defs: Vec<ChatCompletionTool> = self
                .tools
                .values()
                .map(|t| ChatCompletionTool {
                    r#type: ChatCompletionToolType::Function,
                    function: FunctionObject {
                        name: t.name.clone(),
                        description: Some(t.description.clone()),
                        parameters: t.parameters.clone(),
                        strict: None,
                    },
                })
                .collect();
            // since we are catching the error ourselves and sending via stream we can't use ? in any of the build() now
            // and manually need to match the build output
            let request: CreateChatCompletionRequest = match CreateChatCompletionRequestArgs::default()
                .model(model)
                .max_tokens(8162 as u32)
                .messages(memory.messages().clone())
                .tools(tools_defs)
                .tool_choice(ChatCompletionToolChoiceOption::Auto)
                .build()
                    {
                        Ok(msg) => msg.into(),
                        Err(e) => {
                            yield AgentEvent::Error(format!("Failed to build user message: {}", e));
                            return;
                        }
                    };
            let mut response = match self.client.chat().create_stream(request).await {
                Ok(s) => s,
                Err(e) => {
                    yield AgentEvent::Error(e.to_string());
                    return;
                }
            };

            // Tool call streaming(based on the index we can determine the tool in case of multi tool streaming)
            // Chunk 1: { "delta": { "tool_calls": [{ "index": 0, "id": "call_1" }] } }
            // Chunk 2: { "delta": { "tool_calls": [{ "index": 0, "function": { "name": "read_file" } }] } }
            // Chunk 3: { "delta": { "tool_calls": [{ "index": 0, "function": { "arguments": "{\"path\":\"src/" } }] } }
            // Chunk 4: { "delta": { "tool_calls": [{ "index": 0, "function": { "arguments": "main.rs\"}" } }] } }
            // Final:   { "finish_reason": "tool_calls" }
            let mut pending_tool_calls: HashMap<u32, PendingToolCall> = HashMap::new();
            while let Some(chunk_result) = response.next().await {
                match chunk_result {
                    Ok(chunk) => {
                        for choice in chunk.choices {
                            if let Some(content) = choice.delta.content {
                                yield AgentEvent::TextChunk(content);
                            }

                            if let Some(tool_calls) = choice.delta.tool_calls {
                                for tool_call in tool_calls {
                                    // get or create the value for tool_call.index
                                    let entry = pending_tool_calls
                                        .entry(tool_call.index)
                                        .or_default();

                                    if let Some(id) = tool_call.id {
                                        entry.id = Some(id);
                                    }

                                    if let Some(function) = tool_call.function {
                                        if let Some(name) = function.name {
                                            entry.name = Some(name);
                                        }

                                        if let Some(arguments) = function.arguments {
                                            entry.arguments.push_str(&arguments);
                                        }
                                    }
                                }
                            }

                            if matches!(choice.finish_reason, Some(FinishReason::ToolCalls)) {
                                let mut pending_calls: Vec<(u32, PendingToolCall)> = pending_tool_calls
                                    .drain()
                                    .collect();
                                pending_calls.sort_by_key(|(index, _)| *index);

                                // TODO: we can concurrently run this for the read tool calls but we need to add a category like READ, WRITE to each tool
                                for (_, pending_call) in pending_calls {
                                    let name = pending_call.name.unwrap_or_else(|| "unknown".to_string());
                                    let arguments_value = serde_json::from_str(&pending_call.arguments)
                                    .unwrap_or_else(|_| Value::String(pending_call.arguments));

                                    yield AgentEvent::ToolChunk(name.clone(),arguments_value.clone());
                                    match self.call_tool(&name, arguments_value) {
                                        Ok(output) => {
                                            yield AgentEvent::TextChunk(output);
                                        }
                                        Err(e) => {
                                            yield AgentEvent::Error(format!("Tool '{}' error: {}", name, e));
                                        }
                                    }
                                }
                            }
                        }
                    }
                    Err(err) => {
                        yield AgentEvent::Error(err.to_string())
                    }
                }
            };
        };
        Box::pin(stream)
    }
}
