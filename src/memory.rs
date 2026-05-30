use async_openai::types::{
    ChatCompletionRequestMessage, ChatCompletionRequestSystemMessageArgs,
    ChatCompletionRequestToolMessageArgs, ChatCompletionRequestUserMessageArgs,
};

pub enum Message {
    System(String),
    User(String),
    Tool(String, String),
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
        };
        self.messages.push(msg);
        Ok(())
    }

    pub fn messages(&self) -> &Vec<ChatCompletionRequestMessage> {
        &self.messages
    }
}
