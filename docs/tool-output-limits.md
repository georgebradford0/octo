# Tool output truncation limits

Every tool result returned by `execute_tool` is truncated to a per-tool character
limit before being stored in the conversation history and sent back to the model.
Truncation is applied once at the call site in `run_agentic_loop` via
`truncate_tool_output(result, tool_output_limit(name))`.

## Limits by tool

| Tool | Limit (chars) | Rationale |
|---|---|---|
| `bash` | 20,000 | Shell commands can produce large but meaningful output: test runs, diffs, `git log`. |
| `read_file` | 20,000 | Safety ceiling for callers that omit `offset`/`limit`. Correctly-used calls are always smaller. |
| `web_fetch` | 20,000 | HTML-stripped page bodies contain lots of useful prose. `strip_html` already reduces them substantially before this limit applies. |
| `task_output` | 8,000 | Subprocess/agent output can be substantial (test results, build logs). |
| `grep` | 6,000 | More than ~6 k of match lines is noise the model won't meaningfully act on. |
| `web_search` | 4,000 | 10 results × ~400 chars (title + URL + description) saturates well under this ceiling. |
| `task_list` | 3,000 | Task lists are short one-line records; anything beyond implies an unusually large number of tasks. |
| `glob` | 3,000 | File-path lists; beyond ~3 k the model is unlikely to process all entries usefully. |
| `task_get` | 2,000 | Single-task pretty-printed JSON is typically under 500 chars. |
| *(default)* | 2,000 | `edit_file`, `write_file`, `task_create`, `task_update`, `task_stop`, `ask_user`, `create_pull_request` all return fixed short strings. The ceiling costs nothing in practice. |

## Implementation

```
core/src/lib.rs
  tool_output_limit(tool: &str) -> usize   — returns the char limit for a named tool
  truncate_tool_output(s: String, limit: usize) -> String  — applies the limit
  run_agentic_loop (call site)  — applies truncate_tool_output(result, tool_output_limit(name))
```

Truncation previously lived inside individual tool handlers (`bash`, `grep`) and as
a bespoke inline block inside `web_fetch`. Those were removed in favour of the single
call-site application so all tools — including `glob`, `read_file`, `web_search`, and
the task tools, which had no truncation before — are covered uniformly.
