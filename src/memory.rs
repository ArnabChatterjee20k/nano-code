use async_openai::{
    Client,
    config::OpenAIConfig,
    types::{
        ChatCompletionMessageToolCall, ChatCompletionRequestAssistantMessageArgs,
        ChatCompletionRequestMessage, ChatCompletionRequestSystemMessageArgs,
        ChatCompletionRequestToolMessageArgs, ChatCompletionRequestUserMessageArgs,
        ChatCompletionToolType, CreateChatCompletionRequestArgs, FunctionCall,
    },
};

pub struct ToolCallRecord {
    pub id: String,
    pub name: String,
    pub arguments: String, // raw JSON string
}

// Tool is for tool_id and its output
// Assistant is for the tool_calls it made and the content it generated basically as a summarizer like  Message::Assistant { content: Some("Let me check..."), tool_calls: [a, b, c] }
// where each a,b,c will have a separate Message::Tool

struct MemoryConfig {
    model: String,
    max_tokens: usize,                // MEMORY_CONTEXT_TOKEN_LIMIT
    summarization_threshold: f32,     // SUMMARIZATION_THRESHOLD
    recent_fraction: f32,             // RAW_RECENT_MESSAGES
    summary_max_tokens: usize,        // SUMMARY_MAX_TOKENS
    buffer_tokens: usize,             // hardcoded default ok
}

fn env_or<T: std::str::FromStr>(key: &str, default: T) -> T {
    std::env::var(key)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

/// Pick a summary-output budget scaled to the context window: small windows can
fn default_summary_max_tokens(max_tokens: usize) -> usize {
    if max_tokens < 10_000 {
        (max_tokens / 8).max(512) // ~12.5%
    } else if max_tokens < 50_000 {
        (max_tokens / 10).max(1024) // ~10%
    } else {
        (max_tokens / 20).max(2048) // ~5%
    }
}

impl MemoryConfig {
    fn from_env() -> Self {
        let max_tokens = env_or("MEMORY_CONTEXT_TOKEN_LIMIT", 128_000usize);
        MemoryConfig {
            // Summarizer model used by `summarize()`; falls back to the main default.
            model: std::env::var("SUMMARIZER_MODEL_NAME")
                .unwrap_or_else(|_| "google/gemini-3.5-flash".to_string()),
            max_tokens,
            summarization_threshold: env_or("SUMMARIZATION_THRESHOLD", 0.8f32),
            recent_fraction: env_or("RAW_RECENT_MESSAGES", 0.4f32),
            // Override with SUMMARY_MAX_TOKENS if set; otherwise scale to the window.
            summary_max_tokens: env_or("SUMMARY_MAX_TOKENS", default_summary_max_tokens(max_tokens)),
            // min(max(230, max_tokens/100), 1000): at least 230, at most 1000.
            buffer_tokens: (max_tokens / 100).max(230).min(1000),
        }
    }
}

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
    context: Option<String>,
    config: MemoryConfig
}

impl Memory {
    pub fn new(system_prompt: &str) -> Self {
        let system_msg = ChatCompletionRequestSystemMessageArgs::default()
            .content(system_prompt)
            .build()
            .expect("Error during init system message");
        Memory {
            messages: vec![system_msg.into()],
            context: None,
            config: MemoryConfig::from_env(),
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

    /// Builds the request message list: pinned system prompt, then the rolling
    /// summary (if any) as a system message, then the verbatim history. Returns an
    /// owned Vec because the summary is synthesized here, not stored in `messages`.
    pub fn messages(&self) -> Vec<ChatCompletionRequestMessage> {
        let mut out: Vec<ChatCompletionRequestMessage> =
            Vec::with_capacity(self.messages.len() + 1);
        out.push(self.messages[0].clone()); // system prompt is always index 0
        if let Some(ctx) = &self.context {
            if let Ok(msg) = ChatCompletionRequestSystemMessageArgs::default()
                .content(format!("Summary of earlier conversation:\n{ctx}"))
                .build()
            {
                out.push(msg.into());
            }
        }
        out.extend(self.messages[1..].iter().cloned());
        out
    }

    pub async fn compact(&mut self, client: &Client<OpenAIConfig>, model: &str) {
        let current_tokens = Self::get_tokens(model, &self.messages);

        // Reserve space for new messages, function calls, and response generation.
        // NOTE: cast to f32 *before* multiplying — `0.8 as usize` is 0, which would
        // make the threshold 0 and summarize on every call.
        let token_threshold =
            (self.config.max_tokens as f32 * self.config.summarization_threshold) as usize;

        if current_tokens < token_threshold {
            return;
        }

        // Newest messages we keep verbatim, up to this token budget; everything
        // older gets folded into the summary.
        let recent_token_budget =
            (self.config.max_tokens as f32 * self.config.recent_fraction) as usize;
        let mut recent_message_tokens = 0;
        // recent = messages[split_index..]; starts at len() (nothing kept yet).
        let mut split_index = self.messages.len();

        // walk newest -> oldest, keeping messages until the budget would overflow
        for (index, message) in self.messages.iter().enumerate().rev() {
            let message_token_count = Self::get_tokens(model, std::slice::from_ref(message));
            if recent_message_tokens + message_token_count > recent_token_budget {
                break;
            }
            recent_message_tokens += message_token_count;
            split_index = index;
        }

        // Pin the system prompt (index 0) and never start the recent slice inside an
        // assistant->tool group, or the next request 400s on a dangling tool message.
        let split_index = Self::snap_split_index(&self.messages, split_index).max(1);
        if split_index <= 1 {
            return; // only the system prompt is "old" — nothing to summarize
        }

        // How much OLD history we may feed the summarizer in one pass, so the
        // summarize *request* itself fits: window - summary output - safety buffer.
        // saturating_sub clamps to 0 instead of underflow-wrapping to a huge usize.
        let max_message_tokens = self
            .config
            .max_tokens
            .saturating_sub(self.config.summary_max_tokens + self.config.buffer_tokens);

        // Candidates: everything between the pinned system prompt and the recent slice.
        let summarization_messages = &self.messages[1..split_index];
        let mut summarization_token = 0;
        let mut messages_for_summarization: Vec<String> = Vec::new();
        for message in summarization_messages.iter(){
            let mut message_token = Self::get_tokens(model, std::slice::from_ref(message));
            // Default to the full JSON; the truncate branch overrides it when too big.
            let mut actual_message = serde_json::to_string(message).unwrap_or_default();
            if message_token > max_message_tokens {
                // reuse `actual_message` as the source so we don't serialize twice
                let (truncated, count) =
                    Self::truncate_to_tokens(model, &actual_message, max_message_tokens / 2);
                actual_message = truncated; // assign the OUTER binding (no `let`)
                message_token = count;
            }

            if summarization_token + message_token < max_message_tokens {
                summarization_token += message_token;
                messages_for_summarization.push(actual_message); // move it in, no clone
            } else {
                break;
            }
        }
        if messages_for_summarization.is_empty(){
            return;
        }
        let input = messages_for_summarization.join("\n");
        let summary = Self::summarize(
            client,
            &self.config.model,         // summarizer model, not the main `model` we tokenize with
            &input,                     // already a String; `&` gives the &str the fn wants
            self.context.as_deref(),    // Option<String> -> Option<&str>, no move out of &mut self
            self.config.summary_max_tokens,
        )
        .await;                         // summarize is async + returns Result

        match summary {
            // Model signalled there was nothing worth summarizing — don't store "NONE"
            // as the context, and don't drop the history we have no summary for.
            Ok(s) if s.trim() == "NONE" => return,
            Ok(s) => self.context = Some(s),
            Err(e) => {
                // Don't drop history if summarization failed — leave messages intact.
                eprintln!("summarization failed, keeping history: {e}");
                return;
            }
        }

        // Rebuild: pinned system prompt + verbatim recent tail. The summary itself
        // lives in self.context and is injected by messages() at request time.
        let mut rebuilt = Vec::with_capacity(1 + self.messages.len() - split_index);
        rebuilt.push(self.messages[0].clone());
        rebuilt.extend(self.messages[split_index..].iter().cloned());
        self.messages = rebuilt;
    }

    /// Move `split` (start of the verbatim recent slice) back so it never begins inside
    /// an assistant->tool group: a `Tool` message must stay with the assistant whose
    /// `tool_calls` produced it (just above it). Never drops below 1 (the system prompt).
    fn snap_split_index(messages: &[ChatCompletionRequestMessage], mut split: usize) -> usize {
        while split > 1
            && split < messages.len()
            && matches!(messages[split], ChatCompletionRequestMessage::Tool(_))
        {
            split -= 1;
        }
        split
    }

    fn get_tokens(model: &str, messages: &[ChatCompletionRequestMessage]) -> usize {
        let json = serde_json::to_string(messages).unwrap_or_default();
        Self::get_tokens_from_string(model, &json)
    }

    fn get_tokens_from_string(model: &str, message:&str) -> usize{
        let enc = tiktoken::encoding_for_model(model)
            .unwrap_or_else(|| tiktoken::get_encoding("cl100k_base").unwrap());
        enc.count(&message)
    }

    /// Cut `text` down to at most `max_tokens` tokens by encoding, slicing the
    /// token vec, then decoding back. Boundary-safe — unlike byte/char slicing,
    /// it can't panic on a multi-byte char or split a token.
    ///
    /// Returns `(text, token_count)`: since we already encoded, we hand back the
    /// count too so the caller needn't re-tokenize. When truncated, the count is
    /// the number of ids we kept (`max_tokens`); re-encoding the decoded text
    /// could differ by ~1 at the boundary, which is fine for a budget estimate.
    fn truncate_to_tokens(model: &str, text: &str, max_tokens: usize) -> (String, usize) {
        let enc = tiktoken::encoding_for_model(model)
            .unwrap_or_else(|| tiktoken::get_encoding("cl100k_base").unwrap());
        let tokens = enc.encode(text);
        if tokens.len() <= max_tokens {
            return (text.to_string(), tokens.len());
        }
        // decode only the first `max_tokens` ids back into a String
        let truncated = enc.decode_to_string(&tokens[..max_tokens]).unwrap_or_default();
        (truncated, max_tokens)
    }

    async fn summarize(
        client: &Client<OpenAIConfig>,
        model: &str,
        input: &str,
        previous_context: Option<&str>,
        summary_max_tokens: usize,
    ) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
        // Unit tests can't reach the network — short-circuit with a deterministic summary
        // so `compact` can be exercised end-to-end. `cfg!(test)` is the crate-wide test flag.
        if cfg!(test) {
            return Ok("[test summary]".to_string());
        }

        // Self-contained summarization prompt: instructions + example + the data to fold in.
        let prompt = format!(r#"You are a precise summarization assistant. Your task is to progressively
                        summarize conversation history while maintaining critical context and accuracy.

                        INSTRUCTIONS:
                        1. Build upon the previous summary by incorporating new information chronologically
                        2. Preserve key details: names, technical terms, code references, and important decisions
                        3. Maintain the temporal sequence of events and discussions
                        4. For technical discussions, keep specific terms, versions, and implementation details
                        5. For code-related content, preserve function names, file paths, and important parameters
                        6. If the new content is irrelevant or doesn't add value, return "NONE"
                        7. Keep the summary concise but complete - aim for 2-3 sentences unless more detail is crucial
                        8. Use neutral, factual language

                        EXAMPLE
                        Current summary:
                        The user inquires about retirement investment options, specifically comparing
                        traditional IRAs and Roth IRAs. The assistant explains the key differences in
                        tax treatment, with traditional IRAs offering immediate tax deductions and Roth
                        IRAs providing tax-free withdrawals in retirement.

                        New lines of conversation:
                        Human: What factors should I consider when deciding between the two?
                        Assistant: Several key factors influence this decision: 1) Your current tax
                        bracket vs. expected retirement tax bracket, 2) Time horizon until retirement,
                        3) Current income and eligibility for Roth IRA contributions, and 4) Desire for
                        flexibility in retirement withdrawals. For example, if you expect to be in a
                        higher tax bracket during retirement, a Roth IRA might be more advantageous
                        since qualified withdrawals are tax-free. Additionally, Roth IRAs don't have
                        required minimum distributions (RMDs) during your lifetime, offering more
                        flexibility in estate planning.

                        New summary:
                        The discussion covers retirement investment options, comparing traditional and
                        Roth IRAs' tax implications, with traditional IRAs offering immediate deductions
                        and Roth IRAs providing tax-free withdrawals. The conversation expands to cover
                        decision factors including current vs. future tax brackets, retirement timeline,
                        income eligibility, and withdrawal flexibility, with specific emphasis on Roth
                        IRA advantages for those expecting higher retirement tax brackets and the
                        benefit of no required minimum distributions. END OF EXAMPLE

                        Current summary:
                        {}

                        New lines of conversation:
                        {}

                        New summary:"#, previous_context.unwrap_or(""), input);

        // The template above already embeds the prior summary and the new messages,
        // so send it as one message — no separate system/user message duplicating the data.
        let prompt_msg = ChatCompletionRequestUserMessageArgs::default()
            .content(prompt)
            .build()?;

        let request = CreateChatCompletionRequestArgs::default()
            .model(model)
            .max_tokens(summary_max_tokens as u32)
            .messages(vec![prompt_msg.into()])
            .build()?;

        let response = client.chat().create(request).await?;

        // `content` is Option<String> because an assistant turn can be tool-calls-only;
        // here we always expect text, so a missing body is an error.
        let summary = response
            .choices
            .into_iter()
            .next()
            .and_then(|choice| choice.message.content)
            .ok_or("summarizer returned no content")?;

        Ok(summary)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // tiny constructors so the snap test can build a realistic message sequence
    fn sys() -> ChatCompletionRequestMessage {
        ChatCompletionRequestSystemMessageArgs::default().content("s").build().unwrap().into()
    }
    fn usr() -> ChatCompletionRequestMessage {
        ChatCompletionRequestUserMessageArgs::default().content("u").build().unwrap().into()
    }
    fn assistant() -> ChatCompletionRequestMessage {
        ChatCompletionRequestAssistantMessageArgs::default().content("a").build().unwrap().into()
    }
    fn tool() -> ChatCompletionRequestMessage {
        ChatCompletionRequestToolMessageArgs::default()
            .tool_call_id("c1")
            .content("t")
            .build()
            .unwrap()
            .into()
    }

    #[test]
    fn summary_budget_scales_by_window() {
        assert_eq!(default_summary_max_tokens(8_000), 1_000); // 8000/8
        assert_eq!(default_summary_max_tokens(1_000), 512); // 125 -> floored to 512
        assert_eq!(default_summary_max_tokens(40_000), 4_000); // 40000/10
        assert_eq!(default_summary_max_tokens(60_000), 3_000); // 60000/20
        assert_eq!(default_summary_max_tokens(128_000), 6_400); // 128000/20
    }

    #[test]
    fn truncate_leaves_short_text_and_reports_count() {
        let (out, n) = Memory::truncate_to_tokens("any-model", "hello world", 1_000);
        assert_eq!(out, "hello world"); // under budget -> unchanged
        assert!(n > 0 && n < 1_000);
    }

    #[test]
    fn truncate_caps_long_text_to_budget() {
        let text = "lorem ipsum ".repeat(300); // well over 10 tokens
        let (out, n) = Memory::truncate_to_tokens("any-model", &text, 10);
        assert_eq!(n, 10); // reports the ids kept
        assert!(out.len() < text.len()); // actually shorter
    }

    #[test]
    fn snap_moves_recent_start_off_tool_messages() {
        // system, user, assistant(tool_calls), tool, tool, user
        let msgs = vec![sys(), usr(), assistant(), tool(), tool(), usr()];

        // pointing at the 2nd tool walks back past both tools to the assistant (index 2)
        assert_eq!(Memory::snap_split_index(&msgs, 4), 2);
        // pointing at the 1st tool also lands on the assistant
        assert_eq!(Memory::snap_split_index(&msgs, 3), 2);
        // a non-tool boundary is already safe
        assert_eq!(Memory::snap_split_index(&msgs, 5), 5);
        // never crosses below the pinned system prompt
        assert_eq!(Memory::snap_split_index(&msgs, 1), 1);
        // len() (nothing recent) is in-bounds-safe and left alone
        assert_eq!(Memory::snap_split_index(&msgs, msgs.len()), msgs.len());
    }

    // ---- compact() tests ----------------------------------------------------
    // summarize() is stubbed under cfg!(test), so these exercise the real planning
    // and rebuild logic without any network call. The dummy client is never used
    // (summarize short-circuits before touching it).

    fn test_config(max_tokens: usize) -> MemoryConfig {
        MemoryConfig {
            model: "x".into(),
            max_tokens,
            summarization_threshold: 0.5,
            recent_fraction: 0.3,
            summary_max_tokens: 20,
            buffer_tokens: 10,
        }
    }

    fn dummy_client() -> Client<OpenAIConfig> {
        Client::with_config(OpenAIConfig::default())
    }

    #[tokio::test]
    async fn compact_is_noop_under_threshold() {
        // huge window => current tokens never reach the threshold
        let mut mem = Memory {
            messages: vec![sys(), usr()],
            context: None,
            config: test_config(1_000_000),
        };
        mem.compact(&dummy_client(), "any-model").await;
        assert_eq!(mem.messages.len(), 2); // untouched
        assert!(mem.context.is_none()); // never summarized
    }

    #[tokio::test]
    async fn compact_summarizes_and_pins_system_prompt() {
        // many messages + tiny window => forced over threshold
        let mut messages = vec![sys()];
        messages.extend(std::iter::repeat_with(usr).take(20));
        let mut mem = Memory {
            messages,
            context: None,
            config: test_config(100), // threshold = 50 tokens, easily exceeded
        };

        mem.compact(&dummy_client(), "any-model").await;

        // summary was produced (the cfg!(test) stub) and stored as rolling context
        assert_eq!(mem.context.as_deref(), Some("[test summary]"));
        // system prompt is still pinned at index 0
        assert!(matches!(mem.messages[0], ChatCompletionRequestMessage::System(_)));
        // old middle history was folded away, so the list shrank
        assert!(mem.messages.len() < 21);
    }
}
