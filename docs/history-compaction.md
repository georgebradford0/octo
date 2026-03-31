# History compaction

## Why it exists

Every turn of the agentic loop appends two messages to the session history:

1. An **assistant message** — reasoning text plus one or more `tool_use` blocks.
2. A **user message** — one `tool_result` block per tool that was called.

Without any pruning, turn N sends the full history of all previous turns as input
tokens. Because the history grows with every turn, the input token cost of a session
scales quadratically with the number of turns. A 20-turn session ends up sending
roughly 20× the average turn size in input tokens on its final call.

`compact_history` in `core/src/lib.rs` reduces this by stubbing out old turns before
each API request.

---

## What gets compacted and what does not

### The `keep_full` window

`compact_history` is called with `keep_full = 6`. This means the **6 most recent
tool-result user messages** and their paired assistant turns are kept at full fidelity.
Everything older is stubbed.

The window exists because the model frequently needs to refer back to recent tool
results (e.g. a file it just read, the output of the last bash command). Stubbing
those would hurt task quality. Turns beyond 6 steps ago are rarely consulted.

### What counts as a "tool-result message"

Only user messages whose **entire content** is `ToolResult` blocks qualify. The
initial user message (the human's prompt) is never touched, regardless of age,
because it contains the task description the model is working towards.

---

## What stubs look like

### Tool-result user messages (old)

The raw content is replaced with a compact outcome + size summary:

| Outcome | Stub |
|---|---|
| Success | `[ok — 3 412 chars, truncated]` |
| Error (starts with `error:` or `HTTP `) | `[error — 87 chars, truncated]` |
| Empty content | `[empty]` |

This tells the model what happened and roughly how much output the tool produced,
without including any of the content. The model can infer from the outcome tag
whether the step succeeded, and from the size whether the result was trivial or
substantial.

**Before this change** the stub was the first 400 raw characters of the content
followed by `…[truncated]`. This gave the model an incomplete fragment with no
signal about success/failure or how much was omitted.

### Paired assistant messages (old)

Each old tool-result message has an assistant turn immediately before it. That turn
is also stubbed:

- **`Text` blocks** — replaced with `[truncated]`.
- **`ToolUse` blocks** — `id` and `name` are preserved; `input` is replaced with `{}`.

The `id` must be kept intact because the API validates that every `tool_use` block
in an assistant message has a matching `tool_use_id` in the following user message.
The `name` is kept so the model can see which tool was called at each step.
The `input` detail is dropped because it is redundant once the result is known.

**Before this change** old assistant messages were passed through untouched at full
size. In a long session this was the dominant source of history bloat — a 20-turn
run with 200–500 token assistant reasoning blocks would send the full text of all 14
old assistant turns on every call, regardless of how many tool-result stubs were in
place.

---

## Token impact

For a session with T total turns and `keep_full = 6`:

- **Tool-result messages:** turns 1 through T−6 go from their full output size (up
  to 20 000 chars / ~5 000 tokens each) down to a single stub line (~10 tokens).
- **Paired assistant messages:** turns 1 through T−6 go from full reasoning text
  (typically 100–500 tokens each) down to one `[truncated]` text block plus
  minimal `ToolUse` stubs.

In a 20-turn session this reduces the compacted history sent to the API by roughly
80–90% compared to sending the full history, and by 50–70% compared to the previous
approach that only stubbed tool-results.

---

## Call site

```
stream_turn()  →  compact_history(messages, 6)  →  messages_json (sent to API)
```

Compaction runs on the in-memory session snapshot before each API call and does not
mutate the stored session. The full history is preserved in `Session::messages` so
that future turns are compacted from the authoritative source, not from a
previously-compacted view.
