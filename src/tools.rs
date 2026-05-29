use std::{fs, io::BufRead, path::PathBuf, time::SystemTime};

use regex::Regex;
use serde_json::Value;

use crate::agent::Tool;

pub type ToolResult = Result<String, Box<dyn std::error::Error + Send + Sync>>;

pub fn read(path: &str, offset: usize, limit: Option<usize>) -> ToolResult {
    let file = fs::read_to_string(path)?;
    let mut content = String::new();
    for (line_idx, line) in file
        .lines()
        .skip(offset)
        .take(limit.unwrap_or(usize::MAX))
        .enumerate()
    {
        let line_num = offset + line_idx + 1;
        // :4 is adding padding of size 4
        content.push_str(format!("{:4}| {}\n", line_num, line).as_str());
    }
    Ok(content.to_string())
}
pub fn write(path: &str, content: &str) -> ToolResult {
    std::fs::write(path, content)?;
    Ok("ok".to_string())
}
pub fn edit(path: &str, old: &str, new: &str, all: bool) -> ToolResult {
    let file = fs::read_to_string(path)?;
    let mut matched_indices = file.match_indices(old);
    let first_match = matched_indices.next();
    if first_match.is_none() {
        return Err(format!("Error: The target text '{}' was not found.", old).into());
    }

    let multiple_matches = matched_indices.next().is_some();
    if (multiple_matches && !all) {
        return Err(format!(
            "Error: The old text has multiple matches. Use (all=true) to replace all"
        )
        .into());
    }
    let (updated, message) = {
        let replaced = file.replace(old, new);
        (replaced, "ok".to_string())
    };
    let _ = write(path, &updated)?;
    Ok(message)
}
pub fn glob_files(pattern: &str, base: &str) -> ToolResult {
    let full_pattern = PathBuf::from(base).join(pattern);
    let paths = glob::glob(&full_pattern.to_string_lossy())?;
    // for manual here with loops then use the if(ok(path)) and then push to the vector
    let mut paths: Vec<PathBuf> = paths.filter_map(Result::ok).collect();
    paths.sort_by(|a, b| {
        // unwrap_or ensures if error comes then return mentioned default
        // and_then ensures chaining where results depend on success and if error comes then return it
        let a_time = std::fs::metadata(a)
            .and_then(|meta| meta.modified())
            .unwrap_or(SystemTime::UNIX_EPOCH);

        let b_time = std::fs::metadata(b)
            .and_then(|meta| meta.modified())
            .unwrap_or(SystemTime::UNIX_EPOCH);

        b_time.cmp(&a_time)
    });
    let paths_strings: Vec<String> = paths
        .iter()
        .map(|path| path.to_string_lossy().into_owned())
        .collect();

    return Ok(paths_strings.join("\n"));
}
pub fn grep(pattern: &str, base: &str, limit: Option<usize>) -> ToolResult {
    const RECURSIVE_PATTERN: &str = "**/*";
    const CAP: usize = 50;
    let regex = Regex::new(pattern)?;
    let full_pattern = PathBuf::from(base).join(RECURSIVE_PATTERN);
    let paths = glob::glob(&full_pattern.to_string_lossy())?;
    let mut results = Vec::new();
    for path in paths {
        if let Ok(working_path) = path {
            // borrowing working_path with & as we are using this later down the loop
            let file = std::fs::File::open(&working_path);
            if let Ok(file) = file {
                let reader = std::io::BufReader::new(file);
                let mut matched: Vec<String> = reader
                    .lines()
                    .filter_map(Result::ok)
                    .enumerate() // Yields (index, String)
                    .filter(|(_, line)| regex.is_match(line))
                    .map(|(idx, line)| {
                        format!("{}:{}:{}", working_path.to_string_lossy(), idx + 1, line)
                    })
                    .collect();

                results.append(&mut matched);
            };
        };
    }
    let max_results = limit.unwrap_or(CAP) as usize;

    Ok(results
        .iter()
        .take(max_results)
        .cloned()
        .collect::<Vec<String>>()
        .join("\n"))
}
pub fn bash(cmd: &str) -> ToolResult {
    let out = std::process::Command::new("sh")
        .arg("-c")
        .arg(cmd)
        .output()?;
    // TODO: we need to start printing the process stdout and stderror as colored logs
    let mut stdout = out.stdout;
    let mut error = out.stderr;
    stdout.append(&mut error);
    if stdout.is_empty() {
        return Ok("(empty)".to_string());
    };
    Ok(String::from_utf8_lossy(&stdout).into_owned())
}

pub fn tool_read(args: Value) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
    let path = args["path"].as_str().ok_or("missing 'path'")?;
    let offset = args.get("offset").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
    let limit = args
        .get("limit")
        .and_then(|v| v.as_u64())
        .map(|v| v as usize);
    read(path, offset, limit)
}

pub fn tool_write(args: Value) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
    let path = args["path"].as_str().ok_or("missing 'path'")?;
    let content = args["content"].as_str().ok_or("missing 'content'")?;
    write(path, content)
}

pub fn tool_edit(args: Value) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
    let path = args["path"].as_str().ok_or("missing 'path'")?;
    let old = args["old"].as_str().ok_or("missing 'old'")?;
    let new = args["new"].as_str().ok_or("missing 'new'")?;
    let all = args.get("all").and_then(|v| v.as_bool()).unwrap_or(false);
    edit(path, old, new, all)
}

pub fn tool_glob(args: Value) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
    let pattern = args["pattern"].as_str().ok_or("missing 'pattern'")?;
    let base = args.get("base").and_then(|v| v.as_str()).unwrap_or(".");
    glob_files(pattern, base)
}

pub fn tool_grep(args: Value) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
    let pattern = args["pattern"].as_str().ok_or("missing 'pattern'")?;
    let base = args.get("base").and_then(|v| v.as_str()).unwrap_or(".");
    let limit = args
        .get("limit")
        .and_then(|v| v.as_u64())
        .map(|v| v as usize);
    grep(pattern, base, limit)
}

pub fn tool_bash(args: Value) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
    let cmd = args["cmd"].as_str().ok_or("missing 'cmd'")?;
    bash(cmd)
}

pub fn get_tools() -> Vec<Tool> {
    vec![
        Tool {
            name: "read".to_string(),
            description: "Read file with line numbers".to_string(),
            callback: tool_read as fn(Value) -> _,
            parameters: Some(serde_json::json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string" },
                    "offset": { "type": "integer", "minimum": 0 },
                    "limit": { "type": "integer", "minimum": 0 }
                },
                "required": ["path"]
            })),
        },
        Tool {
            name: "write".to_string(),
            description: "Write content to file".to_string(),
            callback: tool_write as fn(Value) -> _,
            parameters: Some(serde_json::json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string" },
                    "content": { "type": "string" }
                },
                "required": ["path", "content"]
            })),
        },
        Tool {
            name: "edit".to_string(),
            description: "Replace old with new in file (old must be unique unless all=true)"
                .to_string(),
            callback: tool_edit as fn(Value) -> _,
            parameters: Some(serde_json::json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string" },
                    "old": { "type": "string" },
                    "new": { "type": "string" },
                    "all": { "type": "boolean" }
                },
                "required": ["path", "old", "new"]
            })),
        },
        Tool {
            name: "glob".to_string(),
            description: "Find files by pattern, sorted by mtime".to_string(),
            callback: tool_glob as fn(Value) -> _,
            parameters: Some(serde_json::json!({
                "type": "object",
                "properties": {
                    "pattern": { "type": "string" },
                    "base": { "type": "string" }
                },
                "required": ["pattern"]
            })),
        },
        Tool {
            name: "grep".to_string(),
            description: "Search files for regex pattern".to_string(),
            callback: tool_grep as fn(Value) -> _,
            parameters: Some(serde_json::json!({
                "type": "object",
                "properties": {
                    "pattern": { "type": "string" },
                    "base": { "type": "string" },
                    "limit": { "type": "integer", "minimum": 1 }
                },
                "required": ["pattern"]
            })),
        },
        Tool {
            name: "bash".to_string(),
            description: "Run shell command".to_string(),
            callback: tool_bash as fn(Value) -> _,
            parameters: Some(serde_json::json!({
                "type": "object",
                "properties": {
                    "cmd": { "type": "string" }
                },
                "required": ["cmd"]
            })),
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    use tempfile::TempDir;

    // ---------- write ----------

    #[test]
    fn write_creates_file_with_content() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("hello.txt");
        let p = path.to_str().unwrap();

        let result = write(p, "hello world").unwrap();
        assert_eq!(result, "ok");
        assert_eq!(fs::read_to_string(p).unwrap(), "hello world");
    }

    #[test]
    fn write_overwrites_existing_file() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("x.txt");
        let p = path.to_str().unwrap();

        write(p, "first").unwrap();
        write(p, "second").unwrap();

        assert_eq!(fs::read_to_string(p).unwrap(), "second");
    }

    // ---------- read ----------

    #[test]
    fn read_returns_numbered_lines() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("file.txt");
        fs::write(&path, "alpha\nbeta\ngamma\n").unwrap();

        let out = read(path.to_str().unwrap(), 0, None).unwrap();
        assert!(out.contains("   1| alpha"), "got:\n{out}");
        assert!(out.contains("   2| beta"), "got:\n{out}");
        assert!(out.contains("   3| gamma"), "got:\n{out}");
    }

    #[test]
    fn read_with_limit_truncates_output() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("file.txt");
        fs::write(&path, "a\nb\nc\nd\n").unwrap();

        let out = read(path.to_str().unwrap(), 0, Some(2)).unwrap();
        assert_eq!(out.lines().count(), 2);
        assert!(out.contains("| a"));
        assert!(out.contains("| b"));
        assert!(!out.contains("| c"));
        assert!(!out.contains("| d"));
    }

    #[test]
    fn read_with_offset_skips_leading_lines() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("file.txt");
        fs::write(&path, "a\nb\nc\nd\n").unwrap();

        let out = read(path.to_str().unwrap(), 2, None).unwrap();
        assert!(!out.contains("| a"));
        assert!(!out.contains("| b"));
        assert!(out.contains("| c"));
        assert!(out.contains("| d"));
    }

    #[test]
    fn read_offset_preserves_absolute_line_numbers() {
        // SURFACES BUG: read() numbers from 1 even with offset>0.
        // Fix: `let line_num = line_idx + offset + 1;` — then this passes.
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("file.txt");
        fs::write(&path, "a\nb\nc\nd\n").unwrap();

        let out = read(path.to_str().unwrap(), 2, None).unwrap();
        assert!(out.contains("   3| c"), "expected '3| c' in:\n{out}");
        assert!(out.contains("   4| d"), "expected '4| d' in:\n{out}");
    }

    #[test]
    fn read_missing_file_returns_err() {
        let result = read("/tmp/nano-code-does-not-exist-xyz-9876", 0, None);
        assert!(result.is_err());
    }

    // ---------- edit ----------

    #[test]
    fn edit_replaces_unique_match() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("e.txt");
        let p = path.to_str().unwrap();
        fs::write(p, "hello world").unwrap();

        let result = edit(p, "world", "rust", false).unwrap();
        assert_eq!(result, "ok");
        assert_eq!(fs::read_to_string(p).unwrap(), "hello rust");
    }

    #[test]
    fn edit_errors_when_old_not_found() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("e.txt");
        let p = path.to_str().unwrap();
        fs::write(p, "hello world").unwrap();

        let result = edit(p, "missing", "x", false);
        assert!(result.is_err());
        assert_eq!(fs::read_to_string(p).unwrap(), "hello world");
    }

    #[test]
    fn edit_errors_on_multiple_matches_without_all_flag() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("e.txt");
        let p = path.to_str().unwrap();
        fs::write(p, "a a a").unwrap();

        let result = edit(p, "a", "b", false);
        assert!(result.is_err());
        assert_eq!(fs::read_to_string(p).unwrap(), "a a a");
    }

    #[test]
    fn edit_replaces_all_when_flag_is_true() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("e.txt");
        let p = path.to_str().unwrap();
        fs::write(p, "a a a").unwrap();

        edit(p, "a", "b", true).unwrap();
        assert_eq!(fs::read_to_string(p).unwrap(), "b b b");
    }

    // ---------- glob_files ----------

    #[test]
    fn glob_finds_matching_files() {
        let dir = TempDir::new().unwrap();
        let base = dir.path().to_str().unwrap();
        fs::write(dir.path().join("a.txt"), "").unwrap();
        fs::write(dir.path().join("b.txt"), "").unwrap();
        fs::write(dir.path().join("c.md"), "").unwrap();

        let out = glob_files("*.txt", base).unwrap();
        assert!(out.contains("a.txt"));
        assert!(out.contains("b.txt"));
        assert!(!out.contains("c.md"));
    }

    #[test]
    fn glob_returns_empty_string_for_no_matches() {
        let dir = TempDir::new().unwrap();
        let base = dir.path().to_str().unwrap();

        let out = glob_files("*.nope", base).unwrap();
        assert_eq!(out, "");
    }

    #[test]
    fn glob_sorts_newest_file_first() {
        let dir = TempDir::new().unwrap();
        let base = dir.path().to_str().unwrap();

        fs::write(dir.path().join("older.txt"), "").unwrap();
        // small sleep so mtimes differ on filesystems with coarse granularity
        std::thread::sleep(Duration::from_millis(20));
        fs::write(dir.path().join("newer.txt"), "").unwrap();

        let out = glob_files("*.txt", base).unwrap();
        let pos_newer = out.find("newer.txt").expect("newer.txt missing");
        let pos_older = out.find("older.txt").expect("older.txt missing");
        assert!(
            pos_newer < pos_older,
            "newer should come first, got:\n{out}"
        );
    }

    // ---------- grep ----------

    #[test]
    fn grep_finds_pattern_with_path_line_format() {
        let dir = TempDir::new().unwrap();
        let base = dir.path().to_str().unwrap();
        fs::write(dir.path().join("a.txt"), "alpha\nbeta needle here\ngamma\n").unwrap();
        fs::write(dir.path().join("b.txt"), "no match\n").unwrap();

        let out = grep("needle", base, None).unwrap();
        assert!(
            out.contains("a.txt:2:beta needle here"),
            "expected path:line:text format, got:\n{out}"
        );
        assert!(!out.contains("b.txt"));
    }

    #[test]
    fn grep_respects_limit() {
        let dir = TempDir::new().unwrap();
        let base = dir.path().to_str().unwrap();
        let mut content = String::new();
        for i in 0..10 {
            content.push_str(&format!("line {i} match\n"));
        }
        fs::write(dir.path().join("many.txt"), content).unwrap();

        let out = grep("match", base, Some(3)).unwrap();
        assert_eq!(out.lines().count(), 3);
    }

    #[test]
    fn grep_returns_empty_for_no_matches() {
        let dir = TempDir::new().unwrap();
        let base = dir.path().to_str().unwrap();
        fs::write(dir.path().join("a.txt"), "nothing here\n").unwrap();

        let out = grep("needle", base, None).unwrap();
        assert_eq!(out, "");
    }

    #[test]
    fn grep_invalid_regex_returns_err() {
        let dir = TempDir::new().unwrap();
        let base = dir.path().to_str().unwrap();

        // unbalanced bracket — never compiles as a regex
        let result = grep("[unterminated", base, None);
        assert!(result.is_err());
    }

    // ---------- bash ----------

    #[test]
    fn bash_runs_command_and_returns_stdout() {
        let out = bash("echo hello").unwrap();
        assert!(out.contains("hello"), "got: {out}");
    }

    #[test]
    fn bash_captures_stderr_too() {
        let out = bash("echo oops 1>&2").unwrap();
        assert!(out.contains("oops"), "got: {out}");
    }

    #[test]
    fn bash_returns_empty_marker_when_no_output() {
        let out = bash("true").unwrap();
        assert_eq!(out, "(empty)");
    }
}
