use async_openai::{
    Client,
    config::OpenAIConfig,
    types::{
        ChatCompletionRequestSystemMessageArgs, ChatCompletionRequestUserMessageArgs,
        CreateChatCompletionRequestArgs,
    },
};

pub type AgentResult = Result<String, Box<dyn std::error::Error>>;
const DEFAULT_MODEL: &str = "google/gemini-3.5-flash";
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

    pub async fn chat(&self, prompt: &str) -> AgentResult {
        let model = std::env::var("MODEL_NAME").unwrap_or(DEFAULT_MODEL.to_string());
        // TODO: add messages memory and roles
        let mut messages = vec![];
        messages.push(
            ChatCompletionRequestSystemMessageArgs::default()
                .content("You are the assistant, help")
                .build()?
                .into(),
        );
        messages.push(
            ChatCompletionRequestUserMessageArgs::default()
                .content(prompt)
                .build()?
                .into(),
        );
        let request = CreateChatCompletionRequestArgs::default()
            .model(model)
            .max_tokens(8162 as u32)
            .messages(messages)
            .build()?;
        let response = self.client.chat().create(request).await?;
        Ok(response
            .choices
            .first()
            .unwrap()
            .message
            .clone()
            .content
            .unwrap())
    }
}
