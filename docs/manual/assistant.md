# Ask

Ask is a panel in the live viewer. It reads the capture that is open — sampling hotspots, call stacks, instrumented scopes, thread states, the scheduler, the current time-range selection, and disassembly when the service has it — and answers in that context. Claims in the reply are links. Click one and the timeline, the report, or the code view moves to that evidence.

The model runs in `orbit-service`, not in the page. The API key never leaves the machine that runs the service.

## Open it

More → **Ask**, or the **Ask** tab on the report. `?report=ask` opens the same tab.

On a wide window Ask is the right-hand report pane. On a phone (the window narrower than 840 CSS pixels) it is a sheet along the bottom, so the timeline keeps the width. **Close** puts the sheet away.

Two buttons sit on the selection report and on Flat, Flame, Top-down, Bottom-up and Live:

- **Explain this selection** — what is slow in the current window, and which functions account for it.
- **Suggest optimizations** — the highest-value changes, grounded in that same window.

With nothing selected, both use the whole capture. The same buttons are at the top of Ask. The text field takes a question (“why is frame 120 slow?”, “what should I optimize?”). Enter or **Send** submits it.

## What it can do

The reply can move the viewer. Each of these is a tool call, and the sentence that justifies it carries an `orbit://` link you can click again later.

- Select a time range, and the report recomputes for that window.
- Jump to a function on the timeline.
- Open disassembly of a function the service can disassemble.
- Propose hooking a function. Nothing is armed until you press **Hook** on the card. **Not now** dismisses it. A confirmed hook is armed for the next Record, the same list as a hook from the Functions tab.

Thread-state and scheduler numbers are included when the capture has them. A capture with only manual scopes is summarized from those scopes.

## Configure the model

Set the variables on the machine that runs `orbit-service`, then restart it. An empty provider, or `off` / `disabled` / `none`, leaves Ask off. The panel then shows how to turn it on.

OpenAI-compatible endpoint:

```
ORBIT_AI_PROVIDER=openai
ORBIT_AI_API_KEY=sk-...
ORBIT_AI_MODEL=gpt-4o-mini
```

`ORBIT_AI_BASE_URL` overrides the default `https://api.openai.com/v1`. Any other OpenAI-compatible server uses the same provider name and a base URL.

A local server needs no key. Ollama’s default port:

```
ORBIT_AI_PROVIDER=ollama
```

That is `http://127.0.0.1:11434/v1` and the model `llama3.2`. `ORBIT_AI_PROVIDER=local` is the same. A localhost base URL is treated as local even when the provider is `openai`, so a key is optional.

Anthropic:

```
ORBIT_AI_PROVIDER=anthropic
ORBIT_AI_API_KEY=sk-ant-...
ORBIT_AI_MODEL=claude-sonnet-4-5
```

The default base is `https://api.anthropic.com`.

`ORBIT_AI_SUMMARY_TOKENS` is the budget for the capture summary (default 2000, clamped to 200–16000). `ORBIT_AI_TIMEOUT_SECS` is the provider timeout (default 60).

`GET /api/ai/status` reports whether a provider is configured. The page uses that for the disabled state. `POST /api/ai/chat` is the question. Both are served by `orbit-service`.

## What is sent

The model does not receive the raw event ring. It receives a short summary: capture bounds, the current selection, the hottest functions (name, module, self and inclusive share, a link), instrumented scope timings, thread-state shares, scheduler slice and core counts, a few call stacks, a disassembly excerpt when one is available, and the selection-report text, already shortened.

That summary, plus the question and the recent conversation, is sent to the configured provider. A local server receives it on this machine; a remote provider receives it at that provider. The API key stays in the service process.

## Limits

The summary is capped. Past the token budget, disassembly, call stacks, extra scopes, the selection report, thread states and the scheduler are dropped before the hottest names. A function with no id in the symbol list cannot be disassembled or hooked; Ask says so instead of pretending. Hooking is a proposal until you confirm it. Up to four tool rounds run per question. An empty capture gets an empty summary and a reply that says there is nothing loaded.
