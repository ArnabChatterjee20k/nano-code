use core::fmt;
use std::collections::HashMap;

use futures::StreamExt;

use async_openai::{
    Client,
    config::OpenAIConfig,
    types::{
        ChatCompletionRequestSystemMessageArgs, ChatCompletionRequestUserMessageArgs,
        CreateChatCompletionRequestArgs, CreateChatCompletionRequest, ChatCompletionTool, ChatCompletionToolType,
        FunctionObject, ChatCompletionToolChoiceOption,
    },
};
use serde_json::Value;

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

    pub fn chat<'a>(&'a self, prompt: &'a str) -> AgentEventStream<'a> {
        let model = std::env::var("MODEL_NAME").unwrap_or(DEFAULT_MODEL.to_string());
        let stream = async_stream::stream! {

            // TODO: add messages memory and roles

            // here using manual match instead of ? or unwrap due to stream macro
            let cwd = std::env::current_dir()
                .map(|p| p.to_string_lossy().into_owned())
                .unwrap_or_else(|_| ".".to_string());
            let system_content = format!(
                "You are the assistant. Current working directory: {}. When useful, call the available tools to perform file and shell operations.",
                cwd
            );
            let system_msg = match ChatCompletionRequestSystemMessageArgs::default()
                .content(system_content)
                .build()
            {
                Ok(msg) => msg.into(),
                Err(e) => {
                    yield AgentEvent::Error(format!("Failed to build system message: {}", e));
                    return;
                }
            };

            let user_msg = match ChatCompletionRequestUserMessageArgs::default()
                .content(prompt)
                .build()
            {
                Ok(msg) => msg.into(),
                Err(e) => {
                    yield AgentEvent::Error(format!("Failed to build user message: {}", e));
                    return;
                }
            };

            let messages = vec![system_msg, user_msg];

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
                .messages(messages)
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
            while let Some(chunk_result) = response.next().await {
                match chunk_result {
                    Ok(chunk) => {
                        for choice in chunk.choices {
                            if let Some(content) = choice.delta.content {
                                yield AgentEvent::TextChunk(content);
                            }

                            if let Some(tool_calls) = choice.delta.tool_calls {
                                for tool_call in tool_calls {
                                    let tool_call_id = tool_call.id.as_deref().unwrap_or("unknown");

                                    if let Some(function) = tool_call.function {
                                        let name = function.name.unwrap_or_else(|| "unknown".to_string());
                                        let arguments = function.arguments.unwrap_or_default();
                                        let arguments_value = serde_json::from_str(&arguments)
                                            .unwrap_or_else(|_| Value::String(arguments.clone()));

                                        // execute the tool and yield its output
                                        match self.call_tool(&name, arguments_value.clone()) {
                                            Ok(output) => {
                                                // first indicate the tool was called
                                                yield AgentEvent::ToolChunk(name.clone(), arguments_value.clone());
                                                // then yield the tool output as text
                                                yield AgentEvent::TextChunk(output);
                                            }
                                            Err(e) => {
                                                yield AgentEvent::Error(format!("Tool '{}' error: {}", name, e));
                                            }
                                        }
                                    } else {
                                        yield AgentEvent::ToolChunk(
                                            tool_call_id.to_string(),
                                            Value::Null,
                                        );
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
