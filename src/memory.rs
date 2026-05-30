use async_openai::types::{
    ChatCompletionMessageToolCall, ChatCompletionRequestAssistantMessageArgs,
    ChatCompletionRequestMessage, ChatCompletionRequestSystemMessageArgs,
    ChatCompletionRequestToolMessageArgs, ChatCompletionRequestUserMessageArgs,
    ChatCompletionToolType, FunctionCall,
};

pub struct ToolCallRecord {
    pub id: String,
    pub name: String,
    pub arguments: String, // raw JSON string
}

// Tool is for tool_id and its output
// Assistant is for the tool_calls it made and the content it generated basically as a summarizer like  Message::Assistant { content: Some("Let me check..."), tool_calls: [a, b, c] }
// where each a,b,c will have a separate Message::Tool
pub enum Message {
    System(String),
    User(String),
    Tool(String, String),
    Assistant {
        content: Option<String>,
        tool_calls: Vec<ToolCallRecord>,
    },
}

pub struct Memory {
    messages: Vec<ChatCompletionRequestMessage>,
}

impl Memory {
    pub fn new(system_prompt: &str) -> Self {
        let system_msg = ChatCompletionRequestSystemMessageArgs::default()
            .content(system_prompt)
            .build()
            .expect("Error during init system message");
        Memory {
            messages: vec![system_msg.into()],
        }
    }

    pub fn update(
        &mut self,
        message: Message,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let msg = match message {
            Message::System(content) => ChatCompletionRequestSystemMessageArgs::default()
                .content(content)
                .build()?
                .into(),
            Message::User(content) => ChatCompletionRequestUserMessageArgs::default()
                .content(content)
                .build()?
                .into(),
            Message::Tool(id, content) => ChatCompletionRequestToolMessageArgs::default()
                .tool_call_id(id)
                .content(content)
                .build()?
                .into(),
            Message::Assistant {
                content,
                tool_calls,
            } => {
                let calls: Vec<ChatCompletionMessageToolCall> = tool_calls
                    .into_iter()
                    .map(|t| ChatCompletionMessageToolCall {
                        id: t.id,
                        r#type: ChatCompletionToolType::Function,
                        function: FunctionCall {
                            name: t.name,
                            arguments: t.arguments,
                        },
                    })
                    .collect();
                let mut builder = ChatCompletionRequestAssistantMessageArgs::default();
                if let Some(c) = content {
                    builder.content(c);
                }
                if !calls.is_empty() {
                    builder.tool_calls(calls);
                }
                builder.build()?.into()
            }
        };
        self.messages.push(msg);
        Ok(())
    }

    pub fn messages(&self) -> &Vec<ChatCompletionRequestMessage> {
        &self.messages
    }
}
