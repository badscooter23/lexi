# API Definitions

Lexi uses a config-driven API abstraction that separates **how to call an LLM API** from **which provider you're calling**. Each API style is defined in a standalone YAML file — no code changes are needed to support a new API format.

## Overview

Four API definitions ship with Lexi:

| Name | Description | Used by |
|---|---|---|
| `chat-completions` | OpenAI-compatible Chat Completions (`/chat/completions`) | NVIDIA |
| `messages` | Anthropic Messages API (`/messages`) | Anthropic |
| `responses` | OpenAI Responses API (`/responses`) | OpenAI |
| `image-generations` | Image generation (used by `generate`) | — |

The shipped definitions (source: `lexi-core/apis/`) are embedded in the `lexi` binary and always load from there — they are not copied to disk, so upgrading Lexi upgrades them. Custom definitions you create live in `~/.lexi-cli/apis/` (or `$LEXI_HOME/apis/`). Shipped definitions can't be edited in place; copy one first with `api create <new> --from <shipped>`.

The full format is specified in [spec/api-definition-v0.md](spec/api-definition-v0.md); this guide covers the essentials.

Providers reference an API definition by name in `providers-config.yaml`:

```yaml
providers:
  openai:
    url: https://api.openai.com/v1
    api_key: ${OPENAI_API_KEY}
    api: responses

  anthropic:
    url: https://api.anthropic.com/v1
    api_key: ${ANTHROPIC_API_KEY}
    api: messages
```

## Anatomy of an API Definition

Here's an abridged `chat-completions.yaml` as an example (see `api show chat-completions` for the full definition):

```yaml
name: chat-completions
description: "OpenAI-compatible Chat Completions API (/v1/chat/completions)"

auth:
  header: Authorization
  value_prefix: "Bearer "

request:
  method: POST
  path: /chat/completions
  body_schema:
    type: object
    required: [model, messages]
    properties:
      model:
        type: string
        x-bind: { from: model }
      messages:
        type: array
        x-render:
          template:
            - role: "{{role}}"
              content: "{{content}}"
      temperature:
        type: number
        minimum: 0
        maximum: 2
        description: "Sampling temperature."
        x-cli: { flag: --temperature, alias: -t }
      max_tokens:
        type: integer
        minimum: 1
        x-cli: { flag: --max-tokens, alias: -m }
      stream:
        type: boolean
        default: false
        x-cli: { flag: --stream }
        x-include: if_truthy
      tools:
        type: array
        items: { type: object }
        x-include: if_non_empty

tool_presets:
  json_object:
    response_format:
      type: json_object

content_mapping:
  text_only_as_string: true
  parts:
    text:
      template: { type: text, text: "{{text}}" }

response:
  text_extract:
    text_path: "choices[0].message.content"
  metadata:
    model: model
    input_tokens: usage.prompt_tokens
    output_tokens: usage.completion_tokens
    total_tokens: usage.total_tokens
    stop_reason: choices[0].finish_reason
  stream:
    delta_path: choices[0].delta.content
    done_marker: "[DONE]"
```

### `auth` Block

Defines how to authenticate API requests.

| Field | Description | Example |
|---|---|---|
| `header` | HTTP header name for the API key | `Authorization`, `x-api-key` |
| `value_prefix` | String prepended to the API key value | `"Bearer "` (note trailing space) |
| `extra_headers` | Additional headers to include | `anthropic-version: "2023-06-01"` |

### `request` Block

Defines how to build the HTTP request.

**`method`** — HTTP method (default: `POST`).

**`path`** — URL path appended to the provider's base URL (e.g., `/chat/completions`).

**`body_schema`** — A JSON Schema (`type: object`) describing the request body. Lexi composes the body by walking its `properties` in order; standard keywords (`type`, `minimum`/`maximum`, `enum`, `required`, …) validate the result, and `x-*` extensions say where each value comes from:

| Extension | Meaning |
|---|---|
| `x-bind: { from: <var> }` | Value comes from a binding variable, e.g. `model` |
| `x-render: { template: ... }` | Value is a template with `{{var}}` placeholders (e.g. `{{role}}`, `{{content}}`, `{{prompt}}`) |
| `x-cli: { flag, alias, repeat, ... }` | Exposes the property as a `respond` flag |
| `x-include: <mode>` | When to emit the field: `always`, `if_set`, `if_truthy`, `if_non_empty`, `if_any_child_set` |

Templates are rendered recursively through strings, maps, and lists. A string that is exactly `"{{var}}"` keeps the binding's type (so a list or number survives intact).

When you pass `--temperature 0.5` to `respond`, Lexi finds the property whose `x-cli.flag` is `--temperature` and sets it in the body. Run `api show <name>` to see a definition's flags (the `cli=` entries). Anything without a flag can still be set with `respond --set path=value` (or removed with `--unset path`).

**`tool_presets`** (top level) — Named body fragments selected with `respond --tool <preset>`. Presets are deep-merged into the body; lists are concatenated.

**`content_mapping`** (top level) — How the prompt's message content (text, images) is lowered into the API's wire shape; the result is available to templates as `{{content}}`.

### `response` Block

Defines how to extract data from the API response.

**`text_extract`** — How to pull the response text. Either a simple path:

```yaml
text_extract: { text_path: "choices[0].message.content" }
```

or a walk over a list of items (as `responses` does):

```yaml
text_extract:
  items_path: output
  item_filter: { type: message }
  content_path: content
  content_filter: { type: output_text }
  text_field: text
```

| API | Text extraction |
|---|---|
| Chat Completions | `text_path: choices[0].message.content` |
| Messages | `items_path: content`, filtered to `type: text` |
| Responses | `items_path: output`, filtered to `message` / `output_text` |

**`metadata`** — Paths to extract additional response metadata:

```yaml
metadata:
  model: "model"                        # Model that was used
  input_tokens: "usage.prompt_tokens"   # Input token count
  output_tokens: "usage.completion_tokens"
  total_tokens: "usage.total_tokens"
  stop_reason: "choices[0].finish_reason"
```

Optional blocks: `stream` (SSE delta/done handling for `--stream`), `activity` (the tool-use summary shown with `--verbose`), and `annotations` (citations, shown with `--cite`). A definition can also declare `files` (Files API uploads, used by `upload`), `operation` (background jobs), and `artifacts` (binary output, used by `generate`). See the [spec](spec/api-definition-v0.md).

## Provider Overrides

A provider can include inline `auth`, `request`, or `response` blocks that are **deep-merged** on top of the referenced API definition. This lets you customize specific fields without duplicating the entire definition.

For example, to use chat-completions but with a different request path:

```yaml
providers:
  my-provider:
    url: https://api.example.com
    api_key: ${MY_API_KEY}
    api: chat-completions
    request:
      path: /v2/completions    # Overrides /chat/completions from the API def
```

The merge is recursive — nested dicts are merged, other values are replaced.

## Managing API Definitions

### List Definitions

```bash
api list                    # Shipped and custom definitions
api list --raw              # Every definition as JSON
```

### View a Definition

```bash
api show chat-completions          # Formatted view, including CLI flags
api show messages --raw            # The definition as JSON
```

### Create a New Definition

Create from scratch (starts with a chat-completions-style scaffold):

```bash
api create my-api --path /v1/generate --text-path "result.text"
```

Copy an existing definition (shipped or custom):

```bash
api create my-api --from messages
```

Or write the YAML yourself and save it as a custom definition (validation errors are printed):

```bash
api save my-api ./my-api.yaml
```

### Edit a Definition

`api edit` works on custom definitions only; shipped ones are read-only (copy one with `api create <new> --from <shipped>` first).

```bash
# Change the request path
api edit my-api --path /v2/generate

# Change the response text extraction path (switches to simple text_path extraction)
api edit my-api --text-path "output.text"

# Add a metadata extraction path (KEY:PATH, repeatable)
api edit my-api --set-meta total_tokens:usage.total_tokens
```

For other changes, edit `~/.lexi-cli/apis/my-api.yaml` directly, or edit a copy and `api save` it.

### Validate a Definition

Check that a definition has all required fields and valid structure (exits 1 if invalid):

```bash
api validate my-api
```

Validation checks:
- `name` field is present and non-empty
- `request` block exists with a non-empty `path` and a `body_schema` mapping (`type: object`)
- `x-include` modes are valid
- `response` block defines `text_extract` (exactly one of `text_path` / `items_path`) or `artifacts`
- Optional blocks (`tool_presets`, `metadata`, `stream`, `activity`, `annotations`, `operation`, `files`) are well-formed
- Legacy keys (`request.body`, `request.parameters`, `request.tools`, `response.text_path`, …) are rejected

### Preview a Definition

Show validation results, CLI flags, tool presets, and a sample composed request body (always JSON):

```bash
api preview my-api
api preview --file ./my-api.yaml --prompt "Hello" --model my-model
```

### Delete a Definition

```bash
api rm my-api               # Custom definitions only
```

## Tutorial: Adding a New Provider

Let's say you want to add a provider that uses the standard OpenAI Chat Completions format (many providers do).

### Step 1: Add the Provider

```bash
providers add my-llm --url https://api.my-llm.com/v1 --default_model my-model-v1 --api chat-completions
```

This writes the entry to `~/.lexi-cli/providers/providers-config.yaml`:

```yaml
providers:
  my-llm:
    url: https://api.my-llm.com/v1
    hosted_models: /models
    api: chat-completions        # Uses the shipped chat-completions definition
    default_model: my-model-v1
```

(`--api` defaults to `chat-completions`.)

### Step 2: Set Your API Key

```bash
providers api-key set my-llm#main --value your-key-here
```

Keys have names (`my-llm#main`); saving under a new one adds the key. It's saved in your OS key store (else an owner-only file, `providers/my-llm/keys/main`), never in `providers-config.yaml`. To read the key from an environment variable instead, give the key that source: `providers api-key source add my-llm#main --env MY_LLM_API_KEY`. A key's `api_key` is a list tried in order; see [Getting Started](getting-started.md#where-a-key-comes-from).

### Step 3: Test It

```bash
providers status my-llm
prompts set test --prompt "Hello, world!"
respond my-llm test
```

### Adding a Provider with a Custom API

If the provider uses a non-standard API format, create a new API definition first:

```bash
# Start from the closest existing definition
api create my-custom-api --from chat-completions

# Adjust the request path and response extraction
api edit my-custom-api --path /api/generate --text-path "response.content"

# Validate it
api validate my-custom-api
```

Then reference it when adding the provider:

```bash
providers add my-provider --url https://api.example.com --default_model my-model --api my-custom-api
```

## Config Resolution Pipeline

When Lexi sends a request, it resolves the effective configuration in this order:

1. **Load API definition** — If the provider has `api: <name>`, load that definition: the embedded copy for a shipped name, else `~/.lexi-cli/apis/<name>.yaml`
2. **Deep-merge provider overrides** — The provider's entry (including any inline `auth`/`request`/`response` blocks) is merged on top
3. **Inline definition** — With no `api` key, a provider that has its own `request` block is used as-is; otherwise it is an error

This means the API definition provides the base configuration and the provider can customize it. The old `respond_kind`/`respond_path` format is no longer supported.
