use futures::StreamExt;

use async_openai::{
    Client,
    config::OpenAIConfig,
    types::{
        ChatCompletionRequestSystemMessageArgs, ChatCompletionRequestUserMessageArgs,
        CreateChatCompletionRequestArgs,
    },
};

const DEFAULT_MODEL: &str = "google/gemini-3.5-flash";
// agent streams come in chunks so using an enum to stream that
pub enum AgentEvent {
    TextChunk(String),
    ToolChunk(String),
    Error(String),
}
pub type AgentEventStream<'a> =
    std::pin::Pin<Box<dyn futures::Stream<Item = AgentEvent> + Send + 'a>>;
pub struct Agent {
    pub client: Client<OpenAIConfig>,
}
impl Agent {
    pub fn new() -> Self {
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
        return Agent { client };
    }

    pub fn chat<'a>(&'a self, prompt: &'a str) -> AgentEventStream<'a> {
        let model = std::env::var("MODEL_NAME").unwrap_or(DEFAULT_MODEL.to_string());
        let stream = async_stream::stream! {

            // TODO: add messages memory and roles

            // here using manual match instead of ? or unwrap due to stream macro
            let system_msg = match ChatCompletionRequestSystemMessageArgs::default()
                .content("You are the assistant, help")
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
                    // since we are catching the error ourselves and sending via stream we can't use ? in any of the build() now
                    // and manually need to match the build output
                    let request = match CreateChatCompletionRequestArgs::default()
                        .model(model)
                        .max_tokens(8162 as u32)
                        .messages(messages)
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
                                for choice in chunk.choices{
                                    if let Some(content) = choice.delta.content{
                                        yield AgentEvent::TextChunk(content);
                                    };
                                }
                            },
                            Err(err) => {
                                yield AgentEvent::Error(err.to_string())
                            }
                        }
                    };
        };
        Box::pin(stream)
    }
}
