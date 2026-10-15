# Assistant

The viewer can ask a model about the capture that is open. Press **Ask** on the transport bar (on a phone it is the same pill, next to Rec). The panel also opens from More → **Ask the assistant**.

The model runs in `orbit-service`. The page never sees an API key. A capture opened as a file, with no service, has no model: the panel says so.

## What you can ask

The briefing is the capture the service has, not a paste of raw samples:

- Sampling hotspots (self and inclusive) and a few hottest call stacks
- Scope timing totals and thread-state totals, when the capture has them
- The current time-range selection, or the whole capture when nothing is selected
- A short disassembly of the hottest function, when symbols for that process are loaded

Two buttons on the report header, and the same two on a rectangle selection, send a prepared question:

- **Explain this selection** — the report window, or the rectangle’s time span plus a short copy of its text
- **Suggest optimizations** — the same window, and the model may propose hooking a hot function

You can also type a question: “why is frame 120 slow?”, “what should I optimize?”.

## What the assistant can do

An answer cites facts as `[[e1]]`, `[[e2]]`, and so on. Each citation is a button. The same facts sit under the answer as chips. A click jumps to that evidence:

| Action | What the viewer does |
|---|---|
| Select a time range | Replaces the timeline selection and zooms to it, with a little padding |
| Jump to a function | Opens the flat report filtered to that name, or the Live tab when there is no function id |
| Open disassembly | Opens the code view for that function. Needs a selected process and a function id |
| Suggest a hook | Shows a card. **Hook it** adds the function to the hook list for the **next Record**. **Not now** drops the proposal. Nothing is hooked in the capture you are looking at |

The hook card is the confirmation. The model cannot arm a probe by itself.

## Setup

Set these in the environment of `orbit-service`, then restart it. The status line in the panel names the provider and the model. When none of this is set, the panel stays closed to questions and prints the same hint.

| Variable | Meaning |
|---|---|
| `ORBIT_AI_PROVIDER` | `openai`, `anthropic`, `ollama`, `openai-compatible` (also `compatible` or `local`), or `mock` |
| `ORBIT_AI_API_KEY` | Required for OpenAI and Anthropic. Optional for an OpenAI-compatible server. Unused by Ollama and `mock` |
| `ORBIT_AI_BASE_URL` | Overrides the default endpoint |
| `ORBIT_AI_MODEL` | Overrides the default model |

Defaults when the provider is set and the base URL is not:

```
openai              https://api.openai.com/v1          gpt-4o-mini
anthropic           https://api.anthropic.com          claude-sonnet-4-5
ollama              http://127.0.0.1:11434/v1          llama3.2
openai-compatible   (you must set ORBIT_AI_BASE_URL)
mock                no network                         orbit-mock
```

`mock` answers on the machine from the briefing. It is for a demo and for tests. It says so in the answer. It is not selected just because the other variables are unset.

OpenAI:

```
ORBIT_AI_PROVIDER=openai
ORBIT_AI_API_KEY=sk-...
orbit-service
```

Anthropic:

```
ORBIT_AI_PROVIDER=anthropic
ORBIT_AI_API_KEY=sk-ant-...
orbit-service
```

A local OpenAI-compatible server (Ollama’s `/v1` endpoint is the usual one):

```
ORBIT_AI_PROVIDER=ollama
ORBIT_AI_MODEL=llama3.2
orbit-service
```

Any other OpenAI-compatible base URL:

```
ORBIT_AI_PROVIDER=openai-compatible
ORBIT_AI_BASE_URL=http://127.0.0.1:8080/v1
ORBIT_AI_MODEL=local-model
orbit-service
```

If `ORBIT_AI_PROVIDER` is unset, a base URL that contains `11434` or `ollama` is treated as Ollama, a base URL that contains `anthropic` as Anthropic, and any other base URL as OpenAI-compatible. A key on its own, with no base URL, is OpenAI.

## HTTP

`GET /api/assistant/status` reports whether a model is configured. `POST /api/assistant/chat` runs one turn. The JSON body is the conversation (a short history plus the new question), the selected ranges, an optional scope id, an intent, a note, and a pid. The reply is the message, evidence, and actions. Neither route returns an API key.

The capture summary and the tool dispatch live in the `orbit-assistant` crate (`rust/crates/orbit-assistant`). `orbit-service` fills the facts from the sample store, the ring, and a short disassembly, then calls the model. An MCP server can link that crate, or call `POST /api/assistant/chat`, instead of reimplementing the briefing. Scope totals and thread-state totals are not on `/api/sampling/*`; a client that only has the sampling routes can still use `hotspots_from_report_json`, `stacks_from_bottom_up_json`, and `disassembly_from_json` from the same crate.

## Privacy

The prompt is the compact briefing: function names, module names, sample counts and percents, scope aggregates, thread-state aggregates, and a short instruction listing (address and text, not source and not raw bytes). It does not include source files, the raw sample stream, or the API key. The key stays in the service process. The briefing is capped (about 1800 tokens, roughly four characters per token); a long tail is counted as omitted rather than sent.

The service sends that briefing to the provider you configured. A local Ollama or `mock` provider does not leave the machine. OpenAI and Anthropic do.

## Limits

- The model sees a summary, not every sample. A claim about a function that is not in the briefing is the model’s, and it will not have an evidence chip.
- Disassembly needs symbols loaded for the selected process, and only the hottest function is included up front. Opening disassembly from an answer loads the code view, which can then show source.
- Hooking a function waits for your confirmation and for the next Record. The current capture does not gain that hook.
- One question at a time. A second send while the first is in flight is ignored.
- `mock` is a scripted demo. It is not a substitute for a configured model when you want a real answer.
- macOS builds have no sampling and no disassembly. The assistant still answers from scopes and thread states when those events are in the capture.
