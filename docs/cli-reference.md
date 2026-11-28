# CLI Reference

Complete command reference for `lexi`, the Rust CLI.
This file is generated from the command definitions — regenerate with
`LEXI_UPDATE_DOCS=1 cargo test -p lexi-cli-rs --test cli_reference`.

Run `lexi` with no command for the interactive shell. Every command accepts
the global `--json` flag (machine-readable output; see
[spec/cli-machine-v1.md](spec/cli-machine-v1.md)).

---

## `respond`

#### `respond`

Send a prompt to a provider; get back text, images, or both.

respond <provider> [TEXT…|-] [--prompt NAME] [--model M] [--output KIND] [--out DIR] [--role R] [--stream] [--background [--detach]] [--image URL|PATH]... [<schema flags>] [--set path=value]... [--unset path]... [--tool preset[,preset]]... [--params FILE] [--cite] [--raw|-r] [--verbose|-v] [--no-validate] [--timeout SECS]

The words are the prompt (`-` reads stdin); with none, the saved prompt --prompt NAME, else the active one. The model's tags pick the API; --output image picks the provider's image API and default image model. Images are saved to --out (default: here), never overwriting a file.

Schema flags (e.g. --temperature, --size) come from the selected API's definition: see `lexi api show <api>` (the cli= entries).

--timeout SECS sets this request's time limit (0 = none) instead of request_timeout in config.json: the whole wait, or with --stream the longest silence.

```
lexi respond <PROVIDER> [ARGS]...
```

- `<PROVIDER>` — Provider name, as in providers-config.yaml
- `<ARGS>` — Prompt name and options (see above)

## `generate`

#### `generate`

Generate image(s) and save them: shortcut for `respond <provider> TEXT --output image`

```
lexi generate [OPTIONS] <PROVIDER> <PROMPT>
```

- `<PROVIDER>` — Provider name
- `<PROMPT>` — The image prompt
- `--model` — Override the provider's default model
- `--out` — Directory to write images into
- `--timeout` — This request's time limit in seconds (0 = none), instead of request_timeout in config.json

## `embed`

#### `embed`

Embed texts as vectors (one per text) with a provider's embedding model.

embed <provider> TEXT... [--file FILE] [-] [--model M] [<schema flags>]
[--set path=value]... [--unset path]... [--no-validate] [--timeout SECS]

Each argument is one text; --file adds one text per line, `-` reads
lines from stdin. Schema flags (e.g. --dimensions, --input-type) come
from the embedding API's definition. Use --json for the vectors.
--timeout SECS sets this request's time limit (0 = none).

```
lexi embed <PROVIDER> [ARGS]...
```

- `<PROVIDER>` — Provider name, as in providers-config.yaml
- `<ARGS>` — Texts and options (see above)

## `upload`

#### `upload`

Upload a file to a provider's Files API; prints the file id

```
lexi upload <PROVIDER> <FILE>
```

- `<PROVIDER>` — Provider name
- `<FILE>` — Path to the file to upload

## `jobs`

Check or wait for a background job (`respond --background --detach`)

#### `jobs status`

Check a job once; prints its result if it has finished

```
lexi jobs status <PROVIDER> <ID>
```

- `<PROVIDER>`
- `<ID>`

#### `jobs wait`

Wait for a job to finish and print its result

```
lexi jobs wait [OPTIONS] <PROVIDER> <ID>
```

- `<PROVIDER>`
- `<ID>`
- `--interval` — Seconds between polls
- `--timeout` — Give up after this many seconds

## `tags`

#### `tags`

Show the model tag vocabulary (namespaces, kinds and what they imply)

```
lexi tags
```

## `version`

#### `version`

Show the CLI version

```
lexi version
```

## `alias`

Manage command aliases (`aliases.json`)

#### `alias list`

Aliases: `ls`

List aliases (optionally only those for the given commands/aliases)

```
lexi alias list [NAMES]...
```

- `<NAMES>`

#### `alias add`

Aliases: `set`

Create or update an alias: `alias add pl providers list`

```
lexi alias add <NAME> <EXPANSION>...
```

- `<NAME>`
- `<EXPANSION>`

#### `alias rm`

Aliases: `del`, `delete`

Remove an alias

```
lexi alias rm <NAME>
```

- `<NAME>`

#### `alias reset`

Reset aliases to the shipped defaults

```
lexi alias reset
```

## `history`

#### `history`

Show or clear command history

```
lexi history [OPTIONS]
```

- `-n, --number` — Show the last N entries (negative: the first N)
- `--clear` — Clear the history

## `sh`

#### `sh`

Run a shell command (also `!command` in the interactive shell)

```
lexi sh <COMMAND>...
```

- `<COMMAND>`

## `exit`

#### `exit`

Aliases: `quit`

Leave the interactive shell

```
lexi exit
```

## `config`

View or update CLI settings (config.json)

#### `config list`

Aliases: `ls`

Show config values (all, or a single key)

```
lexi config list [OPTIONS] [KEY]
```

- `<KEY>` — Show only this key
- `-r, --raw` — Print the raw config file
- `-v, --value` — With a key: print only its value

#### `config set`

Set a config value (the rest of the line is the value)

```
lexi config set <KEY> <VALUE>...
```

- `<KEY>`
- `<VALUE>`

#### `config rm`

Aliases: `del`, `delete`

Remove a config value

```
lexi config rm <KEY>
```

- `<KEY>`

## `prompts`

Manage saved prompts

#### `prompts list`

Aliases: `ls`

List prompts (or show one). The active prompt is marked `*`

```
lexi prompts list [OPTIONS] [NAME]
```

- `<NAME>` — Show only this prompt
- `-r, --raw` — Print the prompt JSON
- `-d, --detailed` — Include max_tokens / temperature / role
- `-t, --table` — Render as a table

#### `prompts set`

Aliases: `save`

Save or update a prompt: `prompts set [name] [text…] [--prompt TEXT] [-m|--max_tokens N] [-t|--temperature T] [-r|--role ROLE] [--set path=value] [--unset path]`. With one bare word and no name, it is the text of the active prompt. Text `\n` becomes a newline

```
lexi prompts set [ARGS]...
```

- `<ARGS>`

#### `prompts rm`

Aliases: `del`, `delete`

Remove a saved prompt

```
lexi prompts rm <NAME>
```

- `<NAME>`

#### `prompts activate`

Aliases: `use`

Make a saved prompt the active one (copies it into `$$`)

```
lexi prompts activate <NAME>
```

- `<NAME>`

## `providers`

Manage providers

#### `providers list`

Aliases: `ls`

List configured providers

```
lexi providers list [OPTIONS]
```

- `-r, --raw` — Print the raw providers-config.yaml
- `-t, --table` — Render as a table

#### `providers status`

Check a provider's API key (live call to its models endpoint)

```
lexi providers status [OPTIONS] <NAME>
```

- `<NAME>`
- `-r, --raw` — Print {provider, status, source} as JSON

#### `providers add`

Add a provider

```
lexi providers add [OPTIONS] --url <URL> --default_model <DEFAULT_MODEL> <NAME>
```

- `<NAME>`
- `--url` — Base URL, e.g. https://api.example.com/v1
- `--default_model` — The model used when none is given
- `--api_key` — API key (saved in the provider's owner-only key file, never the config)
- `--api` — API definition the provider speaks
- `--hosted_models` — Path of the provider's models endpoint

#### `providers api-key set`

Save an API key as the provider's saved key (an owner-only file, providers/<name>/api-key), looked in after the sources api_key lists. Prompted for, or read from stdin, without --value

```
lexi providers api-key set [OPTIONS] <NAME>
```

- `<NAME>`
- `-v, --value`
- `--api` — A key for this API only (one of the provider's APIs); it gets its own api_key

#### `providers api-key show`

Show where the key is looked for, in order (the saved key among them), and the key in use, masked unless --reveal. Commands are run only with --reveal

```
lexi providers api-key show [OPTIONS] <NAME>
```

- `<NAME>`
- `-r, --reveal`
- `--api` — The key this API uses

#### `providers api-key remove`

Delete the saved key. The sources api_key lists, and the config, are left as they are

```
lexi providers api-key remove [OPTIONS] <NAME>
```

- `<NAME>`
- `--api` — The API's own key

#### `providers api-key source list`

Aliases: `ls`

List where the key is looked for, in order (the saved key is last unless api_key places it)

```
lexi providers api-key source list [OPTIONS] <NAME>
```

- `<NAME>`
- `--api` — The sources of this API's own key

#### `providers api-key source add`

Add a place to look for the key: at the end of api_key, or first with --first (one already listed moves there). Lexi never writes to it

```
lexi providers api-key source add [OPTIONS] <--env <VAR>|--file <PATH>|--command <COMMAND>|--saved> <NAME>
```

- `<NAME>`
- `--env` — An environment variable
- `--file` — A file holding the key (relative to the Lexi folder; ~/ for your home folder)
- `--command` — A program that prints the key, with its arguments, e.g. 'op read op://Private/OpenAI/credential' (run without a shell)
- `--saved` — The saved key's place in the list
- `--first` — Look here before the other sources
- `--api` — For this API only (it gets its own api_key)

#### `providers api-key source rm`

Aliases: `del`, `remove`

Stop looking in a place for the key (--saved takes out only its place in the list: the saved key is looked in last again)

```
lexi providers api-key source rm [OPTIONS] <--env <VAR>|--file <PATH>|--command <COMMAND>|--saved> <NAME>
```

- `<NAME>`
- `--env` — An environment variable
- `--file` — A file holding the key (relative to the Lexi folder; ~/ for your home folder)
- `--command` — A program that prints the key, with its arguments, e.g. 'op read op://Private/OpenAI/credential' (run without a shell)
- `--saved` — The saved key's place in the list
- `--api` — From this API's own api_key

#### `providers api list`

Aliases: `ls`

List a provider's APIs: which models each serves, its default model, and whether it comes from the template or your config

```
lexi providers api list <NAME>
```

- `<NAME>`

#### `providers api add`

Add an API to a provider (or update one), e.g. `providers api add openai embeddings --for kind:embedding --default-model text-embedding-3-small`

```
lexi providers api add [OPTIONS] <NAME> <API>
```

- `<NAME>`
- `<API>` — API definition name (see `api list`)
- `--for` — Tags a model needs to use this API (repeatable); none = the catch-all
- `--default-model` — The model used when a request asks for this API's output without naming one
- `--url` — A base URL for this API, if it differs from the provider's

#### `providers api rm`

Aliases: `del`, `delete`

Remove an API from a provider (a template's API is turned off)

```
lexi providers api rm <NAME> <API>
```

- `<NAME>`
- `<API>`

#### `providers upgrade`

Convert providers to the shipped templates, keeping your changes.

Shows what would change; --yes applies it (backing up providers-config.yaml). Values still at the old shipped defaults are dropped (the template supplies current ones); values you changed stay.

```
lexi providers upgrade [OPTIONS] [NAMES]...
```

- `<NAMES>` — Providers to upgrade (default: all that have a shipped template)
- `--template` — The template to use (default: the provider's name)
- `-y, --yes` — Apply the change (otherwise just show it)

#### `providers rename`

Rename a provider

```
lexi providers rename <OLD> <NEW>
```

- `<OLD>`
- `<NEW>`

#### `providers rm`

Aliases: `del`, `delete`

Remove a provider and its models, aliases and saved key

```
lexi providers rm <NAME>
```

- `<NAME>`

## `models`

Manage a provider's models

#### `models list`

Aliases: `ls`

List a provider's configured models

```
lexi models list [OPTIONS] <PROVIDER>
```

- `<PROVIDER>`
- `-r, --raw` — Print the raw configured-models.yaml
- `-a, --aliases` — With --raw: also print the raw model-aliases.yaml
- `-t, --table` — Render as a table
- `-d, --details` — Include every metadata field

#### `models hosted`

List the models a provider hosts (live call to its models endpoint)

```
lexi models hosted [OPTIONS] <PROVIDER>
```

- `<PROVIDER>`
- `-r, --raw` — Print the models as JSON
- `-t, --table` — Render as a table
- `--tag` — Only models with this tag (repeatable: all must match), e.g. out:image
- `--cached` — Use the last fetched listing instead of calling the provider
- `--schema` — Show how the listing is fetched instead of fetching it: the models endpoint and the catalog settings (paging, fields, tag and limit rules), and whether they come from the provider's template or providers-config.yaml

#### `models default`

Show a provider's default model for each kind, or set one.

A model becomes the default for its own kind — chat, image-generation, embedding, … (`models tags` shows a model's kind). Requests that don't name a model use the default for the kind of output they ask for.

```
lexi models default [OPTIONS] <PROVIDER> [MODEL]
```

- `<PROVIDER>`
- `<MODEL>` — The model to make the default for its kind
- `--kind` — Only this kind (chat, image-generation, embedding, …); with MODEL, the model must be of this kind
- `--clear` — Clear your default for --kind, so the provider's default applies

#### `models alias list`

Aliases: `ls`

List a provider's model aliases

```
lexi models alias list [OPTIONS] <PROVIDER>
```

- `<PROVIDER>`
- `-r, --raw` — Print the raw model-aliases.yaml

#### `models alias set`

Point an alias at a model

```
lexi models alias set <PROVIDER> <ALIAS> <MODEL>
```

- `<PROVIDER>`
- `<ALIAS>`
- `<MODEL>`

#### `models alias rm`

Aliases: `del`, `delete`

Remove an alias

```
lexi models alias rm <PROVIDER> <ALIAS>
```

- `<PROVIDER>`
- `<ALIAS>`

#### `models add`

Add a model (or update it if it exists)

```
lexi models add [OPTIONS] <PROVIDER> <MODEL>
```

- `<PROVIDER>`
- `<MODEL>`
- `--name` — Display name
- `--description`
- `--max_tokens`
- `--context_window`
- `--default` — Make this the provider's default model

#### `models set`

Update a model's metadata (or add it)

```
lexi models set [OPTIONS] <PROVIDER> <MODEL>
```

- `<PROVIDER>`
- `<MODEL>`
- `--name` — Display name
- `--description`
- `--max_tokens`
- `--context_window`
- `--default` — Make this the provider's default model

#### `models rm`

Aliases: `del`, `delete`

Remove a model

```
lexi models rm <PROVIDER> <MODEL>
```

- `<PROVIDER>`
- `<MODEL>`

#### `models tag`

Add or remove tags on a model id or glob: `+out:image`, `-in:image`.

Tags describe what a model takes, produces and supports; they decide which of the provider's APIs serves it. Your tags win over the provider's tag rules. See `lexi tags` for the vocabulary.

```
lexi models tag [OPTIONS] <PROVIDER> <MODEL> [OPS]...
```

- `<PROVIDER>`
- `<MODEL>` — A model id, or a glob such as "meta/*"
- `<OPS>` — Tag operations: `+tag` (or `tag`) adds, `-tag` removes
- `--clear` — Remove all of your tags on this model or glob

#### `models tags`

Show a model's effective tags, where each came from, and its API

```
lexi models tags <PROVIDER> <MODEL>
```

- `<PROVIDER>`
- `<MODEL>`

## `api`

Manage API definitions

#### `api list`

Aliases: `ls`

List API definitions (shipped and custom)

```
lexi api list [OPTIONS]
```

- `-r, --raw` — Print every definition as JSON

#### `api show`

Show an API definition

```
lexi api show [OPTIONS] <NAME>
```

- `<NAME>`
- `-r, --raw` — Print the definition as JSON

#### `api validate`

Validate an API definition (exit 1 if invalid)

```
lexi api validate <NAME>
```

- `<NAME>`

#### `api create`

Create a custom API definition (scaffold, or a copy of --from)

```
lexi api create [OPTIONS] <NAME>
```

- `<NAME>`
- `--from` — Copy this definition (shipped or custom)
- `--path` — Request path
- `--text-path` — Response text path (switches to simple text extraction)

#### `api edit`

Edit a custom API definition

```
lexi api edit [OPTIONS] <NAME>
```

- `<NAME>`
- `--path`
- `--text-path`
- `--set-meta` — Add response metadata: KEY:PATH. Repeatable

#### `api rm`

Aliases: `del`, `delete`

Remove a custom API definition

```
lexi api rm <NAME>
```

- `<NAME>`

#### `api preview`

Preview a definition: validation, CLI flags, tool presets, and a sample composed request body (always JSON)

```
lexi api preview [OPTIONS] [NAME]
```

- `<NAME>` — A definition by name (or use --file)
- `--file` — Preview a YAML file instead of a saved definition
- `--prompt`
- `--model`
- `--role`

#### `api save`

Save a YAML file as a custom definition (prints validation errors)

```
lexi api save <NAME> <FILE>
```

- `<NAME>`
- `<FILE>`

## Shortcuts (interactive shell)

| Shortcut | Description |
|---|---|
| `?` | Shortcut for `help`. |
| `!<command>` | Run a shell command. |
| `#<N>` | Re-run history entry N (`#-1` is the last). |
| `↑ / ↓` | Navigate command history. |
