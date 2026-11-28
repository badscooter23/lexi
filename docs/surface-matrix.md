# Lexi Surface Matrix

Every user-facing operation lives once, in `lexi-host`, and is exposed on each
surface (see [cli-consolidation-plan.md](cli-consolidation-plan.md) §3). This
file is the checklist — and it is **checked by CI**
(`lexi-cli-rs/tests/surface_matrix.rs`):

- every top-level `pub fn` in `lexi-host/src/*.rs` must appear below, either in an
  **Operations** row or under **Internal**;
- every name listed must exist in `lexi-host`;
- every **CLI** cell must be one or more real `lexi` command paths (comma-separated,
  in backticks), or `planned (Cn)` for a consolidation slice not yet landed;
- every `lexi` command must appear in an Operations row or under **CLI-only**;
- every **Python API** cell must name one or more real functions of the `lexi`
  wheel (`lexi.respond`, `lexi.models.list`, … — defined in
  `lexi-python/python/lexi/`), and every public function there must appear in a cell.

Cells: `—` = not exposed on that surface. **GUI** names the desktop tab (tracked,
not enforced). The wheel's engine functions (`lexi.compose_request_body`, …) wrap
`lexi-core`, not `lexi-host`, so they aren't listed here.

## Operations

| Operation | lexi-host | CLI | Python API | GUI |
|---|---|---|---|---|
| Send a saved / inline prompt with full options (model aliases, schema flags, presets, `--set`, params) | `prepare_chat`, `run_chat`, `compose_chat_body`, `resolve_prompt` | `respond` | `lexi.respond`, `lexi.prepare` | — |
| Send a prompt (buffered) | `respond_text`, `respond_with_effective` | `respond` | `lexi.respond` | Chat |
| Send a prompt (streaming) | `respond_stream_text`, `respond_stream_with_effective` | `respond` | `lexi.stream` | Chat |
| Send a prompt (background job) | `respond_background_text`, `respond_background_with_effective` | `respond` | `lexi.respond` | — |
| Detached background jobs: submit, check, wait | `submit_chat`, `operation_status`, `wait_operation` | `respond`, `jobs status`, `jobs wait` | `lexi.submit`, `lexi.jobs.status`, `lexi.jobs.wait` | — |
| Attach images to a prompt | `resolve_image`, `resolve_images` | `respond` | `lexi.respond` | — |
| Generate images / save a response's artifacts to disk | `generate_images_text`, `generate_images_with_effective`, `save_artifacts` | `respond`, `generate` | `lexi.respond`, `lexi.generate` | — |
| Embed texts as vectors | `embed` | `embed` | `lexi.embed` | — |
| A time limit for one request, over request_timeout (CLI --timeout SECS, Python timeout=) | `with_request_timeout` | `respond`, `generate`, `embed` | `lexi.respond`, `lexi.stream`, `lexi.generate`, `lexi.embed` | — |
| Upload a file | `upload_file`, `upload_file_with_effective` | `upload` | `lexi.upload` | — |
| List providers | `provider_names`, `list_providers`, `load_providers`, `api_key_source`, `provider_summaries` | `providers list` | `lexi.providers.list` | Providers |
| Add / rename / remove a provider | `add_provider`, `rename_provider`, `remove_provider`, `create_provider` | `providers add`, `providers rename`, `providers rm` | `lexi.providers.add`, `lexi.providers.rename`, `lexi.providers.remove` | — |
| Upgrade a provider to its shipped template | `upgrade_provider` | `providers upgrade` | `lexi.providers.upgrade` | — |
| List / add / remove a provider's APIs (bindings) | `provider_apis`, `add_provider_api`, `remove_provider_api` | `providers api list`, `providers api add`, `providers api rm` | `lexi.providers.apis`, `lexi.providers.add_api`, `lexi.providers.remove_api` | — |
| Store / show / remove an API key | `set_api_key`, `remove_api_key`, `api_key_info` | `providers api-key set`, `providers api-key show`, `providers api-key remove` | `lexi.providers.set_key`, `lexi.providers.show_key`, `lexi.providers.remove_key` | Providers (API Key Management, Setup Wizard) |
| List / add / remove where a key is looked for | `key_source_list`, `add_key_source`, `remove_key_source` | `providers api-key source list`, `providers api-key source add`, `providers api-key source rm` | `lexi.providers.key_sources`, `lexi.providers.add_key_source`, `lexi.providers.remove_key_source` | — |
| Check an API key | `check_api_key` | `providers status` | `lexi.providers.status` | Providers |
| List configured models | `load_models`, `model_summaries` | `models list` | `lexi.models.list` | Models |
| List hosted models (the catalog; classified, filterable by tag, cached) | `fetch_hosted_models`, `fetch_hosted_models_with`, `hosted_summaries`, `cached_catalog` | `models hosted` | `lexi.models.hosted` | Models |
| Show how hosted models are fetched (endpoint, catalog settings and where they come from) | `hosted_schema` | `models hosted` | `lexi.models.hosted_schema` | — |
| Get / set / clear the default model for each kind | `defaults`, `default_models`, `set_default`, `clear_default` | `models default` | `lexi.models.default`, `lexi.models.set_default`, `lexi.models.clear_default` | Models (☆/★), Chat and Images (the default pickers) |
| List model aliases | `model_aliases` | `models alias list` | `lexi.models.aliases` | Models |
| Set / remove model aliases | `set_model_alias`, `delete_model_alias` | `models alias set`, `models alias rm` | `lexi.models.set_alias`, `lexi.models.remove_alias` | Models |
| Add / update / remove a model | `upsert_model`, `delete_model` | `models add`, `models set`, `models rm` | `lexi.models.add`, `lexi.models.set`, `lexi.models.remove` | Models |
| Tag models (add / remove / clear your tags) | `tag_model`, `clear_model_tags`, `user_model_tags` | `models tag` | `lexi.models.tag` | — |
| Show a model's tags, kind and API | `classify` | `models tags` | `lexi.models.tags` | — |
| Resolve which model and API a request uses | `resolve_model` | `respond`, `generate` | `lexi.models.resolve` | — |
| Show the tag vocabulary | `tag_vocabulary` | `tags` | `lexi.tags` | — |
| Read config | `load_config`, `settings` | `config list` | `lexi.config.list`, `lexi.config.get` | Config |
| Write config | `set_config`, `delete_config` | `config set`, `config rm` | `lexi.config.set`, `lexi.config.remove` | Config |
| List prompts | `load_prompts`, `prompt_summaries` | `prompts list` | `lexi.prompts.list`, `lexi.prompts.get` | Prompts |
| Save / remove / activate prompts | `save_prompt`, `delete_prompt`, `set_active_prompt`, `update_prompt` | `prompts set`, `prompts rm`, `prompts activate` | `lexi.prompts.save`, `lexi.prompts.remove`, `lexi.prompts.activate` | Prompts |
| List / show API definitions | `list_api_defs`, `load_api_raw`, `embedded_api_names`, `api_summaries` | `api list`, `api show` | `lexi.apis.list`, `lexi.apis.show` | API definitions |
| Validate an API definition | `validate_api_def` | `api validate` | `lexi.apis.validate` | API definitions |
| Create / edit / remove an API definition | `create_api_def`, `edit_api_def`, `delete_api_def` | `api create`, `api edit`, `api rm` | `lexi.apis.create`, `lexi.apis.edit`, `lexi.apis.remove` | API definitions |
| Save an API definition's raw YAML | `save_api_raw` | `api save` | `lexi.apis.save` | API definitions |
| Preview an API definition | `preview_api_def` | `api preview` | `lexi.apis.preview` | API definitions |

## Internal

Building blocks used by the operations above, or run implicitly by every surface
(home seeding fills in missing defaults at CLI/GUI startup); not surfaced on their own.

`ensure_home`, `ensure_home_in`, `default_aliases`, `lexi_home`, `template_names`, `provider_template`, `binding_config`, `provider_config`, `prompt_defaults`, `prompt_value`, `load_api_def`, `deep_merge`, `effective_config`, `resolve_api_key`, `build_headers`

## CLI-only

Commands with no `lexi-host` operation behind them (CLI mechanics, not Lexi
functionality). Command aliases and history live in `~/.lexi-cli/` and are
shared with the desktop CLI console.

`version`, `help`, `exit`, `history`, `sh`, `alias list`, `alias add`, `alias rm`, `alias reset`
