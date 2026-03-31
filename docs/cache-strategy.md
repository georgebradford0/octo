# Prompt Caching Strategy

## Overview

Claudulhu uses Anthropic's [prompt caching](https://docs.anthropic.com/en/docs/build-with-claude/prompt-caching) to reduce input token costs during agentic sessions. Cached tokens are billed at ~10% of the normal input rate (cache read) or ~125% (cache write, amortised on first use), so effective placement of cache breakpoints is the primary lever for controlling per-turn cost.

Anthropic allows a maximum of **4 cache breakpoints per request**. We consume all 4.

---

## Breakpoint placement

### 1. System prompt (`cache_control: ephemeral`)
The system prompt is static for the lifetime of a session. It is always marked with a cache breakpoint so it is written once and read on every subsequent turn.

### 2. Tool definitions (`cache_control: ephemeral`)
The tool list (last tool definition block) is also static. It receives a breakpoint immediately after the system prompt, keeping the entire preamble cached.

### 3 & 4. Message history (2 × `cache_control: ephemeral`)
The remaining 2 breakpoints are distributed across the compacted message history. The positions are calculated in `core/src/lib.rs` just before the API request is serialised:

```
breakpoint A  →  messages[n / 3]
breakpoint B  →  messages[(2 * n) / 3]
breakpoint C  →  messages[n - 2]   ← most-recent stable turn
```

For small histories (`n < 4`) only breakpoint C is placed.

#### Why three positions?
With a single breakpoint at `n-2`, turns deep into a long session still pay full price for all earlier messages because the cache only covers the prefix up to that one point. By spreading across thirds:

- Turn 20 (~40 messages): breakpoints at ~13, ~26, ~38 → the vast majority of history is cached.
- Turn 50 (~98 messages): breakpoints at ~33, ~65, ~96 → same coverage ratio.

---

## Cache TTL caveat

Anthropic's ephemeral cache entries expire after **5 minutes** of inactivity. In a session with slow turns (e.g. long tool calls or human think-time), the oldest breakpoint may expire and be re-written as a cache miss on the next turn. This is charged at the normal write rate — not a correctness problem, but it means the oldest breakpoint has diminishing returns in very slow sessions.

For sessions where turns are spaced >5 min apart, consider biasing breakpoints toward the recent end of history (e.g. `n/2`, `3n/4`, `n-2`). This has not been implemented yet.

---

## Message compaction interaction

Before breakpoints are applied, `compact_history` stubs out the bodies of tool-result messages older than the last 6 tool results. This reduces raw token volume independently of caching. The two mechanisms are complementary:

- Compaction reduces the number of tokens sent at all.
- Caching reduces the cost of the tokens that are sent repeatedly.

See [`history-compaction.md`](./history-compaction.md) for details on compaction.

---

## Relevant code

| File | What it does |
|---|---|
| `core/src/lib.rs` | Applies cache breakpoints to system prompt, tools, and message history before serialising the API request |
| `core/src/lib.rs` (`compact_history`) | Stubs old tool results to reduce token volume |
