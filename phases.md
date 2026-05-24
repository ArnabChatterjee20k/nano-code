# nano-code — build plan

A six-phase incremental build of a Rust port of [nanocode.py](https://github.com/1rgs/nanocode/blob/master/nanocode.py). Each phase compiles and runs.

This is a Rust learning project; the focus is on building reflexes for ownership/borrowing, async, error handling, and idiomatic iterator usage — not on shipping. Pacing: outline → write → review.

## Stack

| Concern | Choice |
|---|---|
| Edition | 2024 |
| HTTP / LLM SDK | `async-openai` (OpenAI-compatible) pointed at **Vercel AI Gateway** |
| Async runtime | `tokio` (full features) |
| Globbing | `glob` |
| Regex | `regex` |
| Tests | `tempfile` (dev-dep) |
| Error type | `Result<T, Box<dyn std::error::Error>>` (may graduate to `anyhow` or a custom enum if it gets painful) |

## Env vars

```
AI_GATEWAY_API_KEY=…
AI_GATEWAY_BASE_URL=https://ai-gateway.vercel.sh/v1   # confirm in Vercel dashboard
MODEL=anthropic/claude-sonnet-4-5                      # provider-prefixed slug
```

Add `.env` to `.gitignore` before putting a real key anywhere.

---

# Phase 1 — REPL skeleton ✅

**Goal:** a loop that prints a colored prompt, reads a line, dispatches on slash commands. No API, no tools.

## Build

1. `Ansi` enum implementing `Display` — gives you `print!("{}…{}", Ansi::Blue, Ansi::Reset)`.
2. `log!` and `log_separator!` macros, wrapped in `{{ }}` for hygiene.
3. `fn main() -> Result<(), Box<dyn std::error::Error>>` so `?` works.
4. Inside `loop { … }`:
   - Print separator + prompt (`❯ ` in bold blue).
   - **Flush stdout** (`use std::io::Write; stdout().flush()?`).
   - Read a line into a `String` declared *inside* the loop (so it doesn't accumulate across iterations).
   - Bind the byte count: `Ok(0)` means EOF (Ctrl+D) → break.
   - `match input.trim()` on the slash commands:
     - `""` → `continue`
     - `"/q" | "exit"` → break
     - `"/c"` → print "cleared" (no history yet, just wire it now)
     - `_` → echo for now; later this will be the API call.

## Concepts

- `String` (owned, heap) vs `&str` (borrowed slice). `read_line` writes into `&mut String`; literals are `&'static str`.
- `read_line` appends, not replaces — buffer must reset each iteration.
- stdout is line-buffered; `print!` without `\n` needs an explicit `.flush()`.
- `.trim()` returns `&str` borrowed from the source — match against it directly.
- `?` requires the function to return a compatible `Result`. `Box<dyn Error>` is the lazy-but-fine top-level error type.

## Landmines

- Macro hygiene: a transcriber `() => { let x = …; }` leaks `x` to the call site. Wrap in `{{ … }}`.
- Forgetting to flush → prompt only appears after user types Enter.
- Ignoring the `read_line` return value → Ctrl+D spins forever.

## Done when

- `cargo run` shows a colored prompt.
- `/q`, `exit`, Ctrl+D all exit cleanly.
- `/c` prints a "cleared" notice.
- Empty Enter just redraws the prompt without errors.

---

# Phase 2 — Local tools ✅

**Goal:** six pure Rust functions matching the Python tool implementations, plus unit tests.

## Module layout

- `src/tools.rs` with `pub fn` for each tool.
- `mod tools;` in `main.rs`.
- `pub type ToolResult = Result<String, Box<dyn std::error::Error>>;` at the top of `tools.rs`.

## Tools

| Name | Signature | Notes |
|---|---|---|
| `read` | `read(path: &str, offset: usize, limit: Option<usize>) -> ToolResult` | Line-numbered output, format `{:4}\| {line}`. Use `writeln!(output, …)` (bring `std::fmt::Write` into scope as `_`) instead of `push_str(format!(…))`. |
| `write` | `write(path: &str, content: &str) -> ToolResult` | `fs::write(path, content)?; Ok("ok".into())`. |
| `edit` | `edit(path: &str, old: &str, new: &str, all: bool) -> ToolResult` | Use `match_indices(old).next()` twice to detect "at least one" + "more than one" without scanning the whole string. Error out if `old` not found, or multiple matches without `all=true`. |
| `glob_files` | `glob_files(pattern: &str, base: &str) -> ToolResult` | `glob::glob(...)`; sort by `modified()` descending; tolerate metadata failures with `unwrap_or(SystemTime::UNIX_EPOCH)`. |
| `grep` | `grep(pattern: &str, base: &str, limit: Option<usize>) -> ToolResult` | Compile regex once. Walk with `glob("**/*")`. Use `let Ok(x) = … else { continue };` for the skip cases. Labeled `'outer: for` + `break 'outer` for short-circuiting at the cap. |
| `bash` | `bash(cmd: &str) -> ToolResult` | `Command::new("sh").arg("-c").arg(cmd).output()?`. Combine `stdout` and `stderr` with `.extend()`. `.trim()` the final string. (Streaming + timeout deferred to Phase 6.) |

## Concepts

- `std::fs::read_to_string` and `std::fs::write` for whole-file I/O.
- `BufReader::lines()` yields `Result<String, _>` (owned, allocates) — different from `&str::lines()` which yields borrowed slices.
- `match_indices` for non-scanning multi-match detection.
- `glob` crate returns `Result<PathBuf, GlobError>`; `filter_map(Result::ok)` to drop unreadable entries.
- `PathBuf::join` for path composition; `to_string_lossy()` to print.
- `Command::output()` returns `std::process::Output { stdout: Vec<u8>, stderr: Vec<u8>, status }`. Use `from_utf8_lossy` because subprocess output isn't UTF-8-guaranteed.
- Let-else (`let Ok(x) = e else { continue };`) flattens nested `if let` chains.
- Labeled loops + `break 'name` to short-circuit nested iteration.

## Tests

`#[cfg(test)] mod tests { … }` at the bottom of `tools.rs`. Use `tempfile::TempDir` (dev-dep) for per-test isolation — `cargo test` runs in parallel by default. Cover happy paths + error returns + edge cases (empty matches, multi-matches, offset numbering, mtime ordering, regex compile errors, empty subprocess output).

## Landmines

- `Ok(content.to_string())` when `content` is already a `String` is a free clone.
- `let _ = expr?;` is incoherent — pick one.
- `return Ok(x)` at the tail is non-idiomatic; drop both keyword and trailing `;`.
- `read` with `offset > 0` must still number from the absolute line: `offset + line_idx + 1`.
- `cargo test` parallelism: don't share filesystem paths across tests.
- mtime granularity on some filesystems is 1s. Sleep ≥ 20ms (or 1.1s on cautious systems) between writes when testing sort order.

## Done when

- All six tools compile.
- `cargo test` passes with no warnings on production code.
- The tool functions are unused (warnings) — those go away in Phase 4.

---

# Phase 3 — HTTP + JSON

**Goal:** one round-trip to Vercel AI Gateway from inside the REPL. No tools, no history. Type a message, get a reply.

## Build

1. **Cargo.toml** — add `tokio = { version = "1", features = ["full"] }` and `async-openai = "0.27"`.
2. **Make main async.** `#[tokio::main] async fn main() -> Result<(), Box<dyn std::error::Error>>`. Existing sync I/O (`read_line`, `print!`) stays unchanged.
3. **Build the client** before the loop:
   ```
   use async_openai::{Client, config::OpenAIConfig};

   let config = OpenAIConfig::new()
       .with_api_base(env::var("AI_GATEWAY_BASE_URL")?)
       .with_api_key(env::var("AI_GATEWAY_API_KEY")?);
   let client = Client::with_config(config);
   let model = env::var("MODEL").unwrap_or_else(|_| "anthropic/claude-sonnet-4-5".into());
   ```
4. **Smoke test first.** Before touching the loop, hardcode a single API call ("Say hi"), print, exit. Verifies config/auth/JSON parsing without REPL plumbing in the way. Delete after it works.
5. **Wire into the loop.** Replace the catch-all `_ =>` arm. Build a one-message conversation each time (no history yet — that's Phase 5):
   ```
   let user_msg = ChatCompletionRequestUserMessageArgs::default()
       .content(input.to_string())
       .build()?
       .into();

   let req = CreateChatCompletionRequestArgs::default()
       .model(&model)
       .messages(vec![user_msg])
       .build()?;

   match client.chat().create(req).await {
       Ok(resp) => {
           if let Some(text) = resp.choices[0].message.content.clone() {
               log!(Ansi::Cyan, "⏺ {}", text);
           }
       }
       Err(e) => log!(Ansi::Red, "error: {e}"),
   }
   ```

## Concepts

- `#[tokio::main]` is sugar for `Runtime::new()?.block_on(async { … })`.
- `async`/`.await` is cooperative concurrency — `.await` is where the runtime can suspend this task.
- Builder pattern: `…Args::default().field(…).field(…).build()? -> Result<T, OpenAIError>`. The `?` propagates field-validation errors.
- Enum-of-message-variants: `ChatCompletionRequestMessage::{System, User, Assistant, Tool, …}` — each variant wraps a struct of the fields valid for that role. `.into()` from the role-specific builder result lifts into the enum.
- `OpenAIError: std::error::Error`, so `?` into `Box<dyn Error>` works without boilerplate.

## Landmines

- **Wrong base URL** → the SDK silently hits OpenAI proper. Always go through `OpenAIConfig::with_api_base`, never `Client::new()`.
- **Model slug** — Vercel uses provider-prefixed slugs (`anthropic/…`, `openai/…`). Plain `claude-…` 404s.
- **Sync tools from async code.** Your `tools.rs` functions block — calling them from an async context (Phase 5+) will stall the runtime. Use `tokio::task::spawn_blocking` for them.
- **Don't crash the REPL on API errors.** Match the `Result` and `eprintln!`; let the user retry.
- First call has cold-start latency at the gateway (1–2s). Subsequent calls are fast.

## Done when

- `cargo run` → type "hi" → get a cyan assistant reply.
- Typing during an API call works (sort of — `read_line` is sync, so we won't actually interleave, but the program is usable).
- Network/API errors print red and keep the loop alive.

---

# Phase 4 — Tool schema + dispatch

**Goal:** the assistant can request a tool, and we route it to the right Rust function. Still one round-trip per user input (the agentic loop is Phase 5).

## Build

1. **Tool schemas.** OpenAI tool format is JSON Schema wrapped in `{ type: "function", function: { name, description, parameters } }`. Two ways:
   - **Hand-written `serde_json::json!`** macros — least magic, easiest to debug.
   - **`schemars` crate** — derives JSON Schema from Rust structs. More moving parts; worth it if/when we add many tools.

   For 6 tools, hand-written is fine. Build a `pub fn tool_schemas() -> Vec<ChatCompletionTool>` in a new `src/api.rs` (or keep in `main.rs` for now).

2. **Dispatch function.**
   ```
   pub fn run_tool(name: &str, args_json: &str) -> String {
       let args: serde_json::Value = match serde_json::from_str(args_json) {
           Ok(v) => v,
           Err(e) => return format!("error: invalid JSON: {e}"),
       };
       let result = match name {
           "read"  => tools::read(args["path"].as_str().unwrap_or(""), …),
           "write" => tools::write(…),
           // …
           other  => Err(format!("unknown tool: {other}").into()),
       };
       match result {
           Ok(s)  => s,
           Err(e) => format!("error: {e}"),
       }
   }
   ```
   Mirrors the Python `run_tool` which catches all exceptions and returns `"error: …"`.

3. **Attach tools to the request.** Set `.tools(tool_schemas())` on the request builder.

4. **Handle a tool call (single, no loop yet).** After the API response, check `resp.choices[0].message.tool_calls`. If present, dispatch each, **but for Phase 4 just print the result and stop.** No follow-up call. Phase 5 closes the loop.

5. **`serde_json` is already pulled in transitively** by `async-openai`. If not, add it explicitly.

## Concepts

- JSON Schema as Rust data: `serde_json::json!({…})` macro builds a `Value` literal that looks like the JSON.
- Argument parsing: `Value::as_str()`, `as_u64()`, `as_bool()` return `Option<…>` — pattern-match or `unwrap_or` to defaults. Typed structs via `#[derive(Deserialize)]` are nicer once you have many tools.
- `ChatCompletionTool` is the OpenAI-shaped wrapper around `function.{name, description, parameters}`.
- The dispatcher returns a `String` (the model-visible result), folding both successful tool output and tool errors into a single string the way Python's `run_tool` does.

## Landmines

- The Python schema uses `"number?"` syntax for optional — that's a Python convention; OpenAI wants standard JSON Schema with `required: [...]` listing only the required fields.
- Args missing required fields → don't panic; return `"error: missing field 'path'"` so the model can self-correct on the next turn.
- `serde_json::Value` indexing (`args["path"]`) returns `Value::Null` for missing keys — `.as_str()` then returns `None`. Fine, just be aware.
- Don't `.unwrap()` user-provided JSON anywhere.

## Done when

- Asking the assistant "what's in src/main.rs?" causes it to call `read`, and you see the tool call + result printed.
- Tool errors (missing file, bad regex, etc.) come back as strings, not panics.

---

# Phase 5 — Agentic loop

**Goal:** the full conversation. Model can call multiple tools, see the results, call more tools, and finally reply. Plus message history persistence within a session.

## Build

1. **Move `messages` outside the input loop.** `let mut messages: Vec<ChatCompletionRequestMessage> = Vec::new();` declared before `loop { }`. Pushed across iterations.

2. **`/c` clears it.** `messages.clear();` in the `/c` arm.

3. **Inner agentic loop.** After appending the user message:
   ```
   loop {
       let req = …messages(messages.clone())…;
       let resp = client.chat().create(req).await?;
       let assistant_msg = resp.choices[0].message.clone();

       // Print any text content.
       if let Some(text) = &assistant_msg.content {
           log!(Ansi::Cyan, "⏺ {}", text);
       }

       // Push the assistant message (with its tool_calls) onto history.
       messages.push(assistant_msg.clone().into());

       // If no tool calls, we're done with this user turn.
       let Some(tool_calls) = assistant_msg.tool_calls else { break };
       if tool_calls.is_empty() { break; }

       // Dispatch each tool call → push tool result messages.
       for call in tool_calls {
           log!(Ansi::Green, "⏺ {}", call.function.name);
           let result = run_tool(&call.function.name, &call.function.arguments);
           log!(Ansi::Dim, "  ⎿ {}", preview(&result));

           let tool_msg = ChatCompletionRequestToolMessageArgs::default()
               .content(result)
               .tool_call_id(call.id)
               .build()?
               .into();
           messages.push(tool_msg);
       }
       // Loop back: re-call API with tool results appended.
   }
   ```

4. **`spawn_blocking` for sync tools from async code.** Each call to `tools::read/edit/bash/grep/glob_files` blocks. Wrap dispatch:
   ```
   let result = tokio::task::spawn_blocking(move || run_tool(&name, &args)).await?;
   ```
   Without this, a slow `bash` call freezes the runtime.

5. **Conversion helpers.** `async-openai` separates `ChatCompletionResponseMessage` (what the API returns) from `ChatCompletionRequestMessage` (what you send back). You'll need to convert — there's typically an `Into` impl or you build a `ChatCompletionRequestAssistantMessage` from the response fields. Read the docs / examples here; this is the most fiddly mechanical bit of Phase 5.

## Concepts

- **Why messages.clone() each request:** the builder consumes `messages`. Cheaper option: build the request once with `&[ChatCompletionRequestMessage]` if the builder supports it; otherwise clone is fine — the strings are small.
- **`tool_call_id` linkage:** every assistant `tool_calls[i].id` must match exactly one subsequent `tool` role message's `tool_call_id`. If you drop one, the API errors on the next turn. Always emit one tool result per tool call.
- **Termination:** the inner loop exits when the assistant's message has no `tool_calls`. That's how an "agent" decides it's done.
- **`spawn_blocking`** moves the closure onto a dedicated blocking-task threadpool. It returns a `JoinHandle<T>` you `.await`.

## Landmines

- **Dropped tool results.** If an exception in your dispatch means no tool message gets pushed, the next API call fails. Always emit, even on internal errors.
- **History grows unboundedly.** No truncation for now. We can add a Phase-6 polish item to cap at N messages.
- **Cloning the whole `messages` per turn** is fine for small histories but quadratic in total token budget. Don't preemptively optimize; revisit if it hurts.
- **`response.choices[0]`** assumes single-choice responses. We never set `n > 1`, so it's safe.

## Done when

- Ask "list files matching *.rs and read the first one" — assistant calls `glob_files`, then `read`, then summarizes. Without you touching anything in between.
- `/c` resets context — follow-up "what did I just ask?" gets "I don't know."
- A `bash` tool call running `sleep 2` doesn't block other parts of the runtime.

---

# Phase 6 — Polish

**Goal:** the niceties that turn a working agent into a usable one.

## Items, by priority

### a. Streamed bash output

Currently `bash` blocks until the subprocess exits. For long-running commands the user sees nothing. Switch from `Command::output()` to `tokio::process::Command::spawn()` so it integrates with async. Then:

- Take `stdout` and `stderr` as async readers.
- Wrap each in `tokio::io::BufReader` and call `.lines()`.
- `tokio::select!` between the two streams, printing each line dimmed as it arrives.
- Accumulate the lines so the model still gets the full output.

Also add a 30s timeout via `tokio::time::timeout(Duration::from_secs(30), …)` matching Python.

### b. Bold markdown in assistant replies

Python uses `re.sub(r"\*\*(.+?)\*\*", BOLD + r"\1" + RESET, text)`. Same with `regex` crate:

```
let re = Regex::new(r"\*\*(.+?)\*\*").unwrap();
let rendered = re.replace_all(text, format!("{}$1{}", Ansi::Bold, Ansi::Reset));
```

### c. Real terminal width for separators

Currently hardcoded to 80. Use the `terminal_size` crate (`terminal_size::terminal_size()` returns `Option<(Width, Height)>`). Clamp to 80 to match Python's `min(cols, 80)`.

### d. `/index` command

The Python README mentions `/index` but the Python code doesn't actually implement it. Options:
- **Stub:** print "not implemented" — honest, low cost.
- **Naive:** walk the cwd with `glob_files("**/*")`, build a tree, prepend to the system prompt. Cheap, sometimes useful.
- **Real:** chunk + embed + store + retrieve. Requires an embeddings provider and a vector store (or in-memory cosine on a `Vec<f32>`). Significant scope.

Default: stub. Promote to naive if you actually want it.

### e. Output preview for tool results

Match Python's "first 60 chars + ` … +N lines`" preview style. Pure formatting:

```rust
fn preview(s: &str) -> String {
    let mut lines = s.lines();
    let first = lines.next().unwrap_or("").chars().take(60).collect::<String>();
    let rest = lines.count();
    if rest > 0 {
        format!("{first} … +{rest} lines")
    } else if first.len() < s.len() {
        format!("{first}…")
    } else {
        first
    }
}
```

### f. Graceful Ctrl+C inside an API call

Right now SIGINT kills the whole process. To recover (cancel the in-flight request, return to the prompt) you'd need `tokio::signal::ctrl_c()` racing the API call via `tokio::select!`. Nice-to-have; skip unless it's bothering you.

### g. `.env` loading

Add `dotenvy = "0.15"` and `dotenvy::dotenv().ok();` at the top of `main`. Lets you keep keys in a local `.env` instead of exporting in your shell.

## Done when

- `bash "ping -c 5 1.1.1.1"` prints lines as they arrive, not all at once.
- `**bold**` in assistant text renders bold.
- Separators fill the terminal.
- `/index` does *something* — even if just printing "not implemented".

---

# Cross-cutting Rust concepts (encountered along the way)

| Phase introduces | Concept |
|---|---|
| 1 | `String`/`&str`, `read_line` semantics, `?` + `Box<dyn Error>`, macros + hygiene |
| 2 | Module system, `Result` chaining, iterator combinators, `match_indices`, `glob`, `regex`, `Command`, let-else, labeled loops, `#[cfg(test)]` |
| 3 | `async`/`.await`, `tokio` runtime, builder pattern with `.build()? ` validation, enum-of-variants for messages, env vars, custom base URL config |
| 4 | `serde_json::Value`, JSON Schema-as-Rust-data, dispatch tables, error-to-string folding |
| 5 | `spawn_blocking`, `Vec` ownership across loop iterations, response→request type conversions, agentic termination conditions |
| 6 | `tokio::process`, `tokio::io::AsyncBufReadExt`, `tokio::select!`, `tokio::time::timeout`, `regex::Captures` substitution, signal handling |

# Out of scope (probably)

- Multiple parallel agents.
- Permissioning / sandboxing for `bash` and `write`/`edit` — the LLM can `rm -rf` your repo. The Python version doesn't sandbox either; consider running this in a worktree.
- Cost/token tracking.
- Conversation persistence to disk.
- Anything with TUI widgets (`ratatui` etc.) — keep the UI plain stdout.
