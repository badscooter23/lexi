#!/usr/bin/env python3
"""Project CLI entrypoint with interactive shell and persistent history/config."""

from __future__ import annotations

import argparse
import json
import shlex
import subprocess
import sys
import shutil
import os
import urllib.error
import urllib.request
from pathlib import Path
from typing import Iterable
import importlib.resources as importlib_resources

try:
    from rich.console import Console
    from rich.table import Table
except Exception:  # pragma: no cover - optional dependency
    Console = None
    Table = None

try:
    import readline
except ImportError:  # pragma: no cover - readline not available on all platforms
    readline = None
try:
    import yaml  # type: ignore
except Exception:  # pragma: no cover - allow fallback
    yaml = None

RED = "\033[31m"
RESET = "\033[0m"

CLI_ROOT = Path(__file__).resolve().parents[2]  # .../lexi-cli
REPO_ROOT = Path(__file__).resolve().parents[3]  # .../lexi
APP_DIR = Path.home() / ".lexi-cli"
LEGACY_APP_DIR = Path.home() / ".exp-cli"
CONFIG_FILE = APP_DIR / "config.json"
HISTORY_FILE = APP_DIR / "history"
PROMPTS_DIR = APP_DIR / "prompts"
PROVIDERS_ROOT = APP_DIR / "providers"
PROVIDERS_CONFIG = PROVIDERS_ROOT / "providers-config.yaml"
ALIASES_FILE = APP_DIR / "aliases.json"
DEFAULT_PROVIDERS_ROOT = REPO_ROOT / "providers"
if not DEFAULT_PROVIDERS_ROOT.exists():
    try:
        DEFAULT_PROVIDERS_ROOT = Path(importlib_resources.files("lexi_cli") / "providers")
    except Exception:
        DEFAULT_PROVIDERS_ROOT = REPO_ROOT / "providers"
DEFAULT_PROVIDERS_CONFIG = DEFAULT_PROVIDERS_ROOT / "providers-config.yaml"
try:
    DEFAULT_ALIASES_FILE = Path(importlib_resources.files("lexi_cli") / "default-aliases.json")
except Exception:
    DEFAULT_ALIASES_FILE = CLI_ROOT / "default-aliases.json"
LEGACY_CONFIG_FILE = CLI_ROOT / ".config.json"
LEGACY_HISTORY_FILE = CLI_ROOT / ".cli_history"

GENERAL_COMMANDS = {
    "help": "Show available commands and usage.",
    "sh": "Execute the rest of the line as a shell command.",
    "exit": "Exit the CLI.",
    "history": "Show or clear history. history [-n COUNT] [--reset|--clear]",
    "config": "View or update settings. config list [--raw|key] | config set <key> <value> | config rm <key>",
    "alias": "Manage command aliases. alias list | alias add <name> <expansion> | alias rm <name>",
    "version": "Show CLI version.",
}
AI_COMMANDS: dict[str, str] = {}

LEXI_COMMANDS = {
    "prompts": "Manage saved prompts or emit prompt JSON.",
    "providers": "Manage providers (list/add/rename/rm).",
    "models": "Manage models for a provider (list/add/set/rm).",
}

COMMAND_SUBCOMMANDS = {
    "config": {
        "list": {
            "summary": "Show config values (optionally a single key or raw JSON).",
            "usage": "config list [--raw|key]",
        },
        "set": {
            "summary": "Update a config entry.",
            "usage": "config set <key> <value>",
        },
        "rm": {
            "summary": "Remove a config entry.",
            "usage": "config rm <key>",
        },
    },
    "alias": {
        "list": {"summary": "List aliases.", "usage": "alias list"},
        "add": {"summary": "Create/update an alias.", "usage": "alias add <name> <expansion>"},
        "rm": {"summary": "Remove an alias.", "usage": "alias rm <name>"},
    },
    "prompts": {
        "list": {
            "summary": "List prompts (optionally a single prompt).",
            "usage": "prompt list [name] [[-r|--raw] [-d|--detailed]]",
        },
        "set": {
            "summary": "Save or update a prompt definition.",
            "usage": "prompt set [name] [--prompt TEXT] [[-m|--max_tokens] N] [[-t|--temperature] T] [[-r|--role] ROLE]",
        },
        "rm": {
            "summary": "Remove a saved prompt.",
            "usage": "prompt rm <name>",
        },
    },
    "providers": {
        "list": {"summary": "List configured providers.", "usage": "provider list"},
        "add": {
            "summary": "Add a provider.",
            "usage": "provider add <name> --url URL --api_key KEY --default_model MODEL",
        },
        "rename": {
            "summary": "Rename a provider.",
            "usage": "provider rename <old> <new>",
        },
        "rm": {"summary": "Remove a provider.", "usage": "provider rm <name>"},
    },
    "models": {
        "list": {
            "summary": "List models for a provider.",
            "usage": "models list <provider> [--raw|-r] [--table|-t] [--details|-d]",
        },
        "hosted": {
            "summary": "List models hosted by a provider via its API.",
            "usage": "models hosted <provider> [--raw|-r] [--table|-t]",
        },
        "add": {
            "summary": "Add a model to a provider.",
            "usage": "model add <provider> <model> --name NAME [--description TEXT] [--max_tokens N] [--context_window N] [--default]",
        },
        "set": {
            "summary": "Update model metadata.",
            "usage": "model set <provider> <model> [--name NAME] [--description TEXT] [--max_tokens N] [--context_window N] [--default]",
        },
        "rm": {"summary": "Remove a model.", "usage": "model rm <provider> <model>"},
    },
}

SHORTCUTS = {
    "?": "Shortcut for 'help'.",
    "!": "Shortcut for 'sh' in interactive mode.",
    "#<N>": "Re-run a command by its history number (#<N>, negatives are relative).",
    "↑ / ↓": "Navigate command history (previous/next).",
}
EXIT_COMMANDS = {"exit", "quit", "e", "q"}
CONFIG_DEFAULT = {
    "cli_name": "lexi",
    "version": "0.1.0",
    "prompt": "cmd",
    "prompt_delimiter": ">",
    "edit_mode": "vi",
}
CONFIG: dict[str, str] = CONFIG_DEFAULT.copy()
HISTORY: list[str] = []
PROMPT_DEFAULT = {
    "prompt": "",
    "max_tokens": 256,
    "temperature": 0.7,
    "role": "user",
}
PROMPTS: dict[str, dict] = {}
PROMPT_DEFAULTS_LOADED = False
PROMPT_RESOLVED_DEFAULT = PROMPT_DEFAULT.copy()
PROMPT_ACTIVE_NAME = "$$"
ALIASES: dict[str, str] = {}
console = Console() if Console else None


def load_yaml_file(path: Path) -> dict:
    if not path.exists():
        return {}
    try:
        if yaml:
            data = yaml.safe_load(path.read_text()) or {}
            return data if isinstance(data, dict) else {}
        # Fallback: basic JSON parse attempt
        return json.loads(path.read_text())
    except Exception as exc:
        print_error(f"Warning: could not read {path.name} ({exc}).")
        return {}


def save_yaml_file(path: Path, data: dict) -> bool:
    try:
        path.parent.mkdir(parents=True, exist_ok=True)
        if yaml:
            path.write_text(yaml.safe_dump(data, sort_keys=False))
        else:
            path.write_text(json.dumps(data, indent=2))
        return True
    except Exception as exc:
        print_error(f"Warning: could not write {path.name} ({exc}).")
        return False


def expand_env_vars(value: object) -> object:
    """Recursively expand ${VAR} strings using the current environment."""
    if isinstance(value, dict):
        return {k: expand_env_vars(v) for k, v in value.items()}
    if isinstance(value, list):
        return [expand_env_vars(v) for v in value]
    if isinstance(value, str):
        return os.path.expandvars(value)
    return value


def load_aliases() -> None:
    global ALIASES
    if not ALIASES_FILE.exists():
        ALIASES = {}
        return
    try:
        data = json.loads(ALIASES_FILE.read_text())
        if isinstance(data, dict) and all(isinstance(v, list) for v in data.values()):
            # grouped format: command -> [aliases]
            expanded: dict[str, str] = {}
            for cmd, aliases in data.items():
                for alias in aliases:
                    if isinstance(alias, str):
                        expanded[alias] = cmd
            ALIASES = expanded
        elif isinstance(data, dict):
            ALIASES = {k: str(v) for k, v in data.items()}
        else:
            ALIASES = {}
    except Exception as exc:
        print_error(f"Warning: could not read aliases ({exc}); starting empty.")
        ALIASES = {}


def save_aliases() -> None:
    try:
        ALIASES_FILE.parent.mkdir(parents=True, exist_ok=True)
        grouped: dict[str, list[str]] = {}
        for alias, cmd in ALIASES.items():
            grouped.setdefault(cmd, []).append(alias)
        grouped = {k: sorted(v) for k, v in grouped.items()}
        ALIASES_FILE.write_text(json.dumps(grouped, indent=2))
    except Exception as exc:
        print_error(f"Warning: could not save aliases ({exc}).")


def read_alias_file(path: Path) -> dict:
    try:
        data = json.loads(path.read_text())
        return data if isinstance(data, dict) else {}
    except Exception:
        return {}


def print_error(message: str) -> None:
    print(f"{RED}{message}{RESET}")


def parse_default_prompt_settings() -> dict:
    """Read defaults from config/config.yaml if available."""
    defaults_path = REPO_ROOT / "config" / "config.yaml"
    if not defaults_path.exists():
        defaults_path = REPO_ROOT / "config" / "config.yaml.example"
    if not defaults_path.exists():
        return {}

    try:
        import yaml  # type: ignore
    except Exception:
        # Fallback to a minimal parser for the defaults section.
        try:
            text = defaults_path.read_text()
        except OSError:
            return {}
        defaults: dict[str, str | float | int] = {}
        in_defaults = False
        for raw_line in text.splitlines():
            line = raw_line.rstrip("\n")
            stripped = line.strip()
            if not stripped or stripped.startswith("#"):
                continue
            if stripped.startswith("defaults:"):
                in_defaults = True
                continue
            if not in_defaults:
                continue
            if not line.startswith("  "):
                break  # left defaults block
            if ":" not in stripped:
                continue
            key, value = stripped.split(":", 1)
            value = value.strip().strip('"').strip("'")
            if not value:
                continue
            try:
                if "." in value:
                    parsed: float | int | str = float(value)
                else:
                    parsed = int(value)
            except ValueError:
                parsed = value
            defaults[key.strip()] = parsed
        return defaults

    try:
        data = yaml.safe_load(defaults_path.read_text()) or {}
        defaults = data.get("defaults", {}) if isinstance(data, dict) else {}
        return defaults if isinstance(defaults, dict) else {}
    except Exception:
        return {}


def ensure_data_dir() -> None:
    """Create the app data dir and migrate legacy files if present."""
    try:
        APP_DIR.mkdir(parents=True, exist_ok=True)
    except OSError as exc:
        print_error(f"Warning: could not prepare data dir ({exc}).")
        return

    def migrate(src: Path, dest: Path) -> None:
        if dest.exists() or not src.exists():
            return
        try:
            dest.write_text(src.read_text())
        except OSError as exc:
            print_error(f"Warning: could not migrate {src.name} ({exc}).")

    migrate(LEGACY_CONFIG_FILE, CONFIG_FILE)
    migrate(LEGACY_HISTORY_FILE, HISTORY_FILE)
    migrate(LEGACY_APP_DIR / "config.json", CONFIG_FILE)
    migrate(LEGACY_APP_DIR / "history", HISTORY_FILE)
    # migrate legacy prompts.json into per-file prompts
    legacy_prompts_file = LEGACY_APP_DIR / "prompts.json"
    if legacy_prompts_file.exists() and not PROMPTS_DIR.exists():
        try:
            PROMPTS_DIR.mkdir(parents=True, exist_ok=True)
            legacy_data = json.loads(legacy_prompts_file.read_text())
            if isinstance(legacy_data, dict):
                for name, payload in legacy_data.items():
                    if isinstance(payload, dict) and name:
                        (PROMPTS_DIR / f"{name}.json").write_text(json.dumps(payload, indent=2) + "\n")
        except Exception as exc:
            print_error(f"Warning: could not migrate legacy prompts ({exc}).")

    seed_default_providers()


def run_command(command: str) -> int:
    """Run a shell command and stream its output."""
    if not command.strip():
        return 0
    try:
        completed = subprocess.run(command, shell=True)
        return completed.returncode
    except KeyboardInterrupt:
        return 130


def load_history() -> None:
    if not HISTORY_FILE.exists():
        return
    try:
        for line in HISTORY_FILE.read_text().splitlines():
            trimmed = line.strip()
            if trimmed:
                HISTORY.append(trimmed)
    except OSError as exc:
        print_error(f"Warning: could not read history ({exc}).")


def save_history() -> None:
    try:
        HISTORY_FILE.write_text("\n".join(HISTORY) + ("\n" if HISTORY else ""))
    except OSError as exc:
        print_error(f"Warning: could not persist history ({exc}).")


def load_prompts() -> None:
    PROMPTS.clear()
    if not PROMPTS_DIR.exists():
        return
    try:
        for entry in PROMPTS_DIR.glob("*.json"):
            try:
                data = json.loads(entry.read_text())
                if isinstance(data, dict):
                    PROMPTS[entry.stem] = data
            except (OSError, json.JSONDecodeError):
                continue
    except OSError as exc:
        print_error(f"Warning: could not read prompts ({exc}).")


def save_prompts() -> None:
    try:
        PROMPTS_DIR.mkdir(parents=True, exist_ok=True)
    except OSError as exc:
        print_error(f"Warning: could not prepare prompts directory ({exc}).")
        return

    # Persist each prompt to its own file
    existing = set()
    for name, payload in PROMPTS.items():
        if not isinstance(payload, dict):
            continue
        path = PROMPTS_DIR / f"{name}.json"
        existing.add(path.name)
        try:
            path.write_text(json.dumps(payload, indent=2) + "\n")
        except OSError as exc:
            print_error(f"Warning: could not save prompt {name!r} ({exc}).")

    # Clean up stale files
    try:
        for entry in PROMPTS_DIR.glob("*.json"):
            if entry.name not in existing:
                entry.unlink(missing_ok=True)
    except OSError:
        pass


def seed_default_providers() -> None:
    """Copy default providers and models into the app dir if missing."""
    if not DEFAULT_PROVIDERS_CONFIG.exists():
        return
    try:
        PROVIDERS_ROOT.mkdir(parents=True, exist_ok=True)
    except OSError as exc:
        print_error(f"Warning: could not prepare providers dir ({exc}).")
        return

    try:
        default_text = DEFAULT_PROVIDERS_CONFIG.read_text()
    except OSError:
        default_text = ""

    if default_text:
        try:
            needs_update = True
            if PROVIDERS_CONFIG.exists():
                try:
                    if PROVIDERS_CONFIG.read_text() == default_text:
                        needs_update = False
                    else:
                        backup = PROVIDERS_CONFIG.with_suffix(PROVIDERS_CONFIG.suffix + ".bak")
                        shutil.copyfile(PROVIDERS_CONFIG, backup)
                except OSError:
                    pass
            if needs_update:
                shutil.copyfile(DEFAULT_PROVIDERS_CONFIG, PROVIDERS_CONFIG)
        except OSError as exc:
            print_error(f"Warning: could not seed providers config ({exc}).")

    # Seed default aliases if missing or empty
    if DEFAULT_ALIASES_FILE and DEFAULT_ALIASES_FILE.exists():
        should_seed = False
        if not ALIASES_FILE.exists():
            should_seed = True
        else:
            existing_aliases = read_alias_file(ALIASES_FILE)
            if not existing_aliases:
                should_seed = True
        if should_seed:
            try:
                ALIASES_FILE.parent.mkdir(parents=True, exist_ok=True)
                shutil.copyfile(DEFAULT_ALIASES_FILE, ALIASES_FILE)
            except OSError as exc:
                print_error(f"Warning: could not seed aliases ({exc}).")

    for entry in DEFAULT_PROVIDERS_ROOT.iterdir() if DEFAULT_PROVIDERS_ROOT.exists() else []:
        if not entry.is_dir():
            continue
        src_models = entry / "models.yaml"
        if not src_models.exists():
            continue
        dest_dir = PROVIDERS_ROOT / entry.name
        dest_models = dest_dir / "models.yaml"
        if dest_models.exists():
            continue
        try:
            dest_dir.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(src_models, dest_models)
        except OSError as exc:
            print_error(f"Warning: could not seed models for {entry.name} ({exc}).")


def load_providers_config() -> dict:
    data = load_yaml_file(PROVIDERS_CONFIG)
    data = expand_env_vars(data)
    return data if isinstance(data, dict) else {}


def save_providers_config(data: dict) -> bool:
    return save_yaml_file(PROVIDERS_CONFIG, data)


def provider_models_path(name: str) -> Path:
    return PROVIDERS_ROOT / name / "models.yaml"


def load_models(provider: str) -> dict:
    return load_yaml_file(provider_models_path(provider))


def save_models(provider: str, data: dict) -> bool:
    return save_yaml_file(provider_models_path(provider), data)


def load_config() -> None:
    global CONFIG
    if CONFIG_FILE.exists():
        try:
            data = json.loads(CONFIG_FILE.read_text())
            if not isinstance(data, dict):
                raise ValueError("Config file must contain a JSON object.")
            CONFIG = {**CONFIG_DEFAULT, **{k: str(v) for k, v in data.items()}}
            return
        except (OSError, ValueError, json.JSONDecodeError) as exc:
            print_error(f"Warning: could not read config ({exc}); using defaults.")

    CONFIG = CONFIG_DEFAULT.copy()
    save_config()


def save_config() -> None:
    try:
        CONFIG_FILE.write_text(json.dumps(CONFIG, indent=2) + "\n")
    except OSError as exc:
        print_error(f"Warning: could not persist config ({exc}).")


def enable_line_editing() -> None:
    """Configure readline for vi/emacs editing and prime history, if available."""
    if readline is None:
        return

    requested_mode = CONFIG.get("edit_mode", CONFIG_DEFAULT["edit_mode"]).lower()
    if requested_mode not in {"vi", "emacs"}:
        requested_mode = CONFIG_DEFAULT["edit_mode"]

    bindings: Iterable[str] = [
        "set editing-mode vi" if requested_mode == "vi" else "set editing-mode emacs",
        '"\\e[A": previous-history',
        '"\\e[B": next-history',
    ]

    for binding in bindings:
        try:
            readline.parse_and_bind(binding)
        except Exception:
            continue

    try:
        readline.clear_history()
        for entry in HISTORY:
            readline.add_history(entry)
    except Exception:
        return


def prompt_defaults() -> dict:
    """Return prompt defaults, optionally overridden by config/config.yaml defaults."""
    global PROMPT_DEFAULTS_LOADED, PROMPT_RESOLVED_DEFAULT
    if PROMPT_DEFAULTS_LOADED:
        return PROMPT_RESOLVED_DEFAULT.copy()
    base = PROMPT_DEFAULT.copy()
    overrides = parse_default_prompt_settings()
    if overrides:
        for key in ("max_tokens", "temperature", "role"):
            if key in overrides:
                base[key] = overrides[key]
    PROMPT_DEFAULTS_LOADED = True
    PROMPT_RESOLVED_DEFAULT = base.copy()
    return base.copy()


def print_config_values(keys: list[str] | None = None) -> None:
    items = [(k, CONFIG[k]) for k in keys if k in CONFIG] if keys else list(CONFIG.items())
    if not items:
        print("No config values set.")
        return

    for key, value in items:
        print(f'{key}="{value}"')


def print_providers(data: dict) -> None:
    providers = data.get("providers") if isinstance(data, dict) else None
    if not providers:
        print("No providers configured.")
        return
    for name, cfg in providers.items():
        url = cfg.get("url", "")
        default_model = cfg.get("default_model", "")
        print(f"{name}: url={url} default_model={default_model}")


def print_models(provider: str, models_data: dict, *, table: bool = False, details: bool = False) -> None:
    default_model = models_data.get("default_model")
    models_data_dict = models_data if isinstance(models_data, dict) else {}
    models = models_data_dict.get("models", {}) if isinstance(models_data_dict.get("models", {}), dict) else {}
    aliases = models_data_dict.get("aliases", {}) if isinstance(models_data_dict.get("aliases", {}), dict) else {}
    alias_lookup: dict[str, list[str]] = {}
    for alias, target in aliases.items():
        alias_lookup.setdefault(target, []).append(alias)

    if table and (Console is None or Table is None or console is None):
        print_error("Rich is not available; falling back to plain output.")
        table = False

    if not models:
        print(f"No models configured for {provider}.")
        return

    if table and console:
        table_obj = Table(title=f"Models for {provider}")
        table_obj.add_column("Model Name")
        table_obj.add_column("Alias")
        table_obj.add_column("Description")
        for key, meta in models.items():
            meta_dict = meta if isinstance(meta, dict) else {}
            name = meta_dict.get("name") or key
            model_label = f"{name} (default)" if key == default_model else name
            alias_text = ", ".join(sorted(alias_lookup.get(key, [])))
            desc_lines = []
            desc = meta_dict.get("description") or ""
            if desc:
                desc_lines.append(desc)
            if details:
                for detail_key, detail_val in sorted(meta_dict.items()):
                    if detail_key in {"name", "description"}:
                        continue
                    desc_lines.append(f"{detail_key}: {detail_val}")
            description = "\n".join(desc_lines)
            table_obj.add_row(model_label, alias_text, description)
        console.print(table_obj)
        return

    print(f"Models for {provider}:")
    for key, meta in models.items():
        meta_dict = meta if isinstance(meta, dict) else {}
        name = meta_dict.get("name", "")
        desc = meta_dict.get("description", "")
        extra = []
        if key == default_model:
            extra.append("default")
        aliases_for_model = alias_lookup.get(key, [])
        if aliases_for_model:
            extra.append(f"alias: {', '.join(sorted(aliases_for_model))}")
        info = " ".join(extra)
        suffix = f" ({info})" if info else ""
        print(f"  {key}: {name} - {desc}{suffix}")
        if details:
            for detail_key, detail_val in sorted(meta_dict.items()):
                if detail_key in {"name", "description"}:
                    continue
                print(f"    {detail_key}: {detail_val}")
    if aliases:
        print("Aliases:")
        for alias, target in aliases.items():
            print(f"  {alias} -> {target}")


def fetch_hosted_models(provider: str, provider_cfg: dict) -> dict | list | None:
    base_url = (provider_cfg.get("url") or "").rstrip("/")
    # Normalize to base /v1 if someone stored /v1/messages, etc.
    if "/v1/" in base_url:
        base_url = base_url.split("/v1/", 1)[0] + "/v1"
    elif base_url.endswith("/v1/messages"):
        base_url = base_url[: -len("/messages")]
    hosted_path = provider_cfg.get("hosted_models", "/models")
    api_key = provider_cfg.get("api_key") or ""
    if not base_url:
        print_error(f"Provider {provider!r} is missing a url in providers-config.yaml")
        return None
    if not api_key:
        print_error(f"Provider {provider!r} is missing an api_key in providers-config.yaml")
        return None
    if not hosted_path:
        print_error(f"Provider {provider!r} is missing hosted_models path in providers-config.yaml")
        return None
    endpoint = f"{base_url}{hosted_path}" if hosted_path.startswith("/") else f"{base_url}/{hosted_path}"

    headers = {"Content-Type": "application/json", "Accept": "application/json"}
    if provider.lower() == "anthropic":
        headers["x-api-key"] = api_key
        headers.setdefault("anthropic-version", "2023-06-01")
    else:
        headers["Authorization"] = f"Bearer {api_key}"

    try:
        req = urllib.request.Request(endpoint, headers=headers)  # type: ignore[arg-type]
        with urllib.request.urlopen(req, timeout=15) as resp:  # type: ignore[call-arg]
            body = resp.read()
    except urllib.error.HTTPError as exc:
        detail = ""
        try:
            detail_body = exc.read().decode("utf-8")
            if detail_body:
                detail = f": {detail_body}"
        except Exception:
            pass
        print_error(f"HTTP error fetching hosted models from {provider}: {exc}{detail}")
        return None
    except urllib.error.URLError as exc:
        print_error(f"Network error fetching hosted models from {provider}: {exc}")
        return None
    except Exception as exc:
        print_error(f"Unexpected error fetching hosted models from {provider}: {exc}")
        return None

    try:
        return json.loads(body.decode("utf-8"))
    except Exception:
        print_error("Could not parse hosted models response as JSON.")
        return None


def normalize_hosted_items(data: dict | list | None) -> list:
    if isinstance(data, dict):
        if isinstance(data.get("data"), list):
            return data["data"]
        if isinstance(data.get("models"), list):
            return data["models"]
    if isinstance(data, list):
        return data
    return []


def print_hosted_models(provider: str, provider_cfg: dict, data: dict | list | None, *, raw: bool, table: bool) -> None:
    if data is None:
        print_error("No data returned.")
        return
    if raw:
        print(json.dumps(data, indent=2))
        return

    items = normalize_hosted_items(data)
    if not items:
        print(f"No hosted models returned for {provider}.")
        return

    fields = provider_cfg.get("hosted_model_fields")
    if not isinstance(fields, list) or not fields:
        if isinstance(items[0], dict):
            fields = list(items[0].keys())
        else:
            fields = ["id"]

    if table and console:
        table_obj = Table(title=f"Hosted models for {provider}")
        for field in fields:
            table_obj.add_column(field)
        for item in items:
            if isinstance(item, dict):
                row = [str(item.get(field, "")) for field in fields]
            else:
                row = [str(item) if idx == 0 else "" for idx, _ in enumerate(fields)]
            table_obj.add_row(*row)
        console.print(table_obj)
        return

    print(f"Hosted models for {provider}:")
    for item in items:
        if isinstance(item, dict):
            values = [f"{field}={item.get(field, '')}" for field in fields]
            print(f"  {', '.join(values)}")
        else:
            print(f"  {item}")


def record_history(entry: str) -> None:
    trimmed = entry.strip()
    if trimmed:
        HISTORY.append(trimmed)
        if readline is not None:
            try:
                readline.add_history(trimmed)
            except Exception:
                pass
        save_history()


def print_command_with_subcommands(name: str, desc: str) -> None:
    print(f"  {name:<12} - {desc}")
    subcommands = COMMAND_SUBCOMMANDS.get(name, {})
    for sub_name, meta in subcommands.items():
        summary = meta.get("summary", "")
        print(f"    {sub_name:<10} {summary}")


def parse_prompt_options(tokens: list[str], base: dict, *, require_prompt: bool) -> tuple[dict | None, str | None]:
    prompt_parts: list[str] = []
    updated = base.copy()
    i = 0
    while i < len(tokens):
        tok = tokens[i]
        if tok in {"--max_tokens", "-m"}:
            if i + 1 >= len(tokens):
                return None, "max_tokens requires an integer value"
            try:
                updated["max_tokens"] = int(tokens[i + 1])
            except ValueError:
                return None, "max_tokens must be an integer"
            i += 2
            continue
        if tok in {"--temperature", "-t"}:
            if i + 1 >= len(tokens):
                return None, "temperature requires a numeric value"
            try:
                updated["temperature"] = float(tokens[i + 1])
            except ValueError:
                return None, "temperature must be numeric"
            i += 2
            continue
        if tok in {"--role", "-r"}:
            if i + 1 >= len(tokens):
                return None, "role requires a value"
            updated["role"] = tokens[i + 1]
            i += 2
            continue
        if tok == "--prompt":
            if i + 1 >= len(tokens):
                return None, "prompt requires a value"
            prompt_parts.append(tokens[i + 1])
            i += 2
            continue
        prompt_parts.append(tok)
        i += 1

    prompt_text = " ".join(prompt_parts).strip()
    if prompt_text:
        updated["prompt"] = prompt_text
    if require_prompt and not updated.get("prompt"):
        return None, "prompt text is required"
    return updated, None


def run_history_entry(index: int) -> tuple[int, bool]:
    if index == 0:
        print_error("History index 0 is invalid; use positive or negative numbers.")
        return 1, False

    resolved_index = index if index >= 0 else len(HISTORY) + index + 1

    if resolved_index <= 0 or resolved_index > len(HISTORY):
        print_error(f"History index {index} is out of range.")
        return 1, False

    target = HISTORY[resolved_index - 1].strip()
    if target.startswith("#"):
        print_error("Cannot re-run a history reference (#...) from history.")
        return 1, False

    return execute_cli_line(target, record=True)


def print_history(count: int | None) -> None:
    if not HISTORY:
        print("History is empty.")
        return

    if count is None or count == 0:
        items = HISTORY
        start_idx = 1
    elif count > 0:
        items = HISTORY[-count:]
        start_idx = len(HISTORY) - len(items) + 1
    else:
        items = HISTORY[: abs(count)]
        start_idx = 1

    for idx, cmd in enumerate(items, start=start_idx):
        print(f"{idx}: {cmd}")


def handle_history(rest: str | None) -> int:
    reset = False
    count: int | None = None
    if rest:
        tokens = shlex.split(rest)
        i = 0
        while i < len(tokens):
            tok = tokens[i]
            if tok in {"--reset", "--clear"}:
                reset = True
                i += 1
                continue
            if tok in {"-n", "--number"}:
                if i + 1 >= len(tokens):
                    print_error("history: -n requires an integer count.")
                    return 1
                try:
                    count = int(tokens[i + 1])
                except ValueError:
                    print_error("history: -n requires an integer count.")
                    return 1
                i += 2
                continue
            print_error(f"history: unknown argument {tok!r}. Use -n COUNT, --reset, or --clear.")
            return 1

    if reset:
        HISTORY.clear()
        save_history()
        if readline is not None:
            try:
                readline.clear_history()
            except Exception:
                pass
        print("History cleared.")
        if count is None:
            return 0

    print_history(count)
    return 0


def handle_config(rest: str | None) -> int:
    if not rest:
        print_error("Usage: config list [--raw|key] | config set <key> <value> | config rm <key>")
        return 1

    tokens = shlex.split(rest)
    if not tokens:
        print_error("Usage: config list [--raw|key] | config set <key> <value> | config rm <key>")
        return 1

    subcommand = tokens[0].lower()
    if subcommand == "list":
        if len(tokens) == 1:
            print_config_values()
            return 0

        # Support list <key> [--value|-v] and list --value|-v <key>
        if len(tokens) in {2, 3}:
            raw_requested = False
            value_only = False
            key = None
            for tok in tokens[1:]:
                if tok == "--raw":
                    raw_requested = True
                elif tok in {"--value", "-v"}:
                    value_only = True
                else:
                    key = tok

            if raw_requested and not key:
                try:
                    raw = CONFIG_FILE.read_text()
                except OSError as exc:
                    print_error(f"Could not read config file: {exc}")
                    return 1
                print(raw, end="" if raw.endswith("\n") else "\n")
                return 0

            if key:
                if key in CONFIG:
                    if value_only:
                        print(CONFIG[key])
                    else:
                        print_config_values([key])
                    return 0
                print_error(f"Config key {key!r} not found.")
                return 1

            if raw_requested:
                try:
                    raw = CONFIG_FILE.read_text()
                except OSError as exc:
                    print_error(f"Could not read config file: {exc}")
                    return 1
                print(raw, end="" if raw.endswith("\n") else "\n")
                return 0

        print_error("Usage: config list [--raw|key]")
        return 1

    if subcommand == "set":
        if len(tokens) < 3:
            print_error("Usage: config set <key> <value>")
            return 1
        key, value = tokens[1], " ".join(tokens[2:])
        CONFIG[key] = value
        save_config()
        print(f"Set {key} = {value}")
        return 0

    if subcommand == "rm":
        if len(tokens) != 2:
            print_error("Usage: config rm <key>")
            return 1
        key = tokens[1]
        if key not in CONFIG:
            print_error(f"Config key {key!r} not found.")
            return 1
        del CONFIG[key]
        save_config()
        print(f"Removed {key}")
        return 0

    print_error("Usage: config list [--raw|key] | config set <key> <value> | config rm <key>")
    return 1


def handle_prompt(rest: str | None, tokens_override: list[str] | None = None) -> int:
    usage = (
        "prompts <text> [[-m|--max_tokens] N] [[-t|--temperature] T] [[-r|--role] ROLE]\n"
        "prompts list [name] [[-r|--raw] [-d|--detailed] [-t|--table]]\n"
        "prompts set [name] [--prompt TEXT] [[-m|--max_tokens] N] [[-t|--temperature] T] [[-r|--role] ROLE]\n"
        "prompts rm <name>"
    )
    if not rest:
        print_error(f"Usage:\n{usage}")
        return 1

    if tokens_override is not None:
        tokens = tokens_override
    else:
        tokens = shlex.split(rest)
    if not tokens:
        print_error(f"Usage:\n{usage}")
        return 1

    sub = tokens[0].lower()
    if sub in {"list", "ls"}:
        name: str | None = None
        raw = False
        detailed = False
        use_table = False
        i = 1
        while i < len(tokens):
            tok = tokens[i]
            if tok in {"-r", "--raw"}:
                raw = True
                i += 1
                continue
            if tok in {"-d", "--detailed"}:
                detailed = True
                i += 1
                continue
            if tok in {"-t", "--table"}:
                use_table = True
                i += 1
                continue
            if tok.startswith("-"):
                print_error(f"Unknown flag {tok!r} for prompt list.")
                return 1
            if name is None:
                name = tok
                i += 1
                continue
            print_error("prompt list accepts at most one name.")
            return 1

        def render_prompt(pname: str, payload: dict) -> None:
            prompt_text = payload.get("prompt", "")
            if raw:
                print(json.dumps({pname: payload}, indent=2))
                return
            if use_table and console:
                return  # handled in table rendering path
            if detailed:
                max_tokens = payload.get("max_tokens", "")
                temp = payload.get("temperature", "")
                role = payload.get("role", "")
                prefix = "* " if pname == PROMPT_ACTIVE_NAME else ""
                print(f"{prefix}{pname}: {prompt_text}")
                print(f"    max_tokens={max_tokens} temperature={temp} role={role}")
                return
            prefix = "* " if pname == PROMPT_ACTIVE_NAME else ""
            print(f"{prefix}{pname}: {prompt_text}")

        if name:
            payload = PROMPTS.get(name)
            if not payload:
                print_error(f"Prompt {name!r} not found.")
                return 1
            if use_table and console:
                table_obj = Table(title=f"Prompt {name}")
                table_obj.add_column("Name")
                table_obj.add_column("Prompt")
                table_obj.add_column("max_tokens")
                table_obj.add_column("temperature")
                table_obj.add_column("role")
                table_obj.add_row(
                    name,
                    payload.get("prompt", ""),
                    str(payload.get("max_tokens", "")),
                    str(payload.get("temperature", "")),
                    payload.get("role", ""),
                )
                console.print(table_obj)
            else:
                render_prompt(name, payload)
            return 0

        if not PROMPTS:
            print("No prompts saved.")
            return 0

        if use_table and console:
            table_obj = Table(title="Prompts")
            table_obj.add_column("Name")
            table_obj.add_column("Prompt")
            table_obj.add_column("max_tokens")
            table_obj.add_column("temperature")
            table_obj.add_column("role")
            # Active first, then rest
            ordered = []
            if PROMPT_ACTIVE_NAME in PROMPTS:
                ordered.append((PROMPT_ACTIVE_NAME, PROMPTS[PROMPT_ACTIVE_NAME]))
            for pname, payload in PROMPTS.items():
                if pname == PROMPT_ACTIVE_NAME:
                    continue
                ordered.append((pname, payload))
            for pname, payload in ordered:
                table_obj.add_row(
                    pname,
                    payload.get("prompt", ""),
                    str(payload.get("max_tokens", "")),
                    str(payload.get("temperature", "")),
                    payload.get("role", ""),
                )
            console.print(table_obj)
            return 0

        # List active first if present, then others.
        if PROMPT_ACTIVE_NAME in PROMPTS:
            render_prompt(PROMPT_ACTIVE_NAME, PROMPTS[PROMPT_ACTIVE_NAME])

        for pname, payload in PROMPTS.items():
            if pname == PROMPT_ACTIVE_NAME:
                continue
            render_prompt(pname, payload)
        return 0

    if sub in {"set", "save"}:
        name: str | None = None
        remaining_tokens = tokens[1:]

        if remaining_tokens and not remaining_tokens[0].startswith("-"):
            if len(remaining_tokens) >= 2:
                name = remaining_tokens[0]
                remaining_tokens = remaining_tokens[1:]
            else:
                # Single positional with no flags -> treat as prompt text for active prompt
                remaining_tokens = [remaining_tokens[0]]

        name = name or PROMPT_ACTIVE_NAME
        base = prompt_defaults()
        if name in PROMPTS and isinstance(PROMPTS[name], dict):
            base = {**base, **PROMPTS[name]}
        payload, err = parse_prompt_options(remaining_tokens, base, require_prompt=False)
        if err:
            print_error(err)
            return 1
        if not payload or not payload.get("prompt"):
            print_error("prompt set requires prompt text (use --prompt or supply text).")
            return 1
        PROMPTS[name] = payload
        save_prompts()
        print(f"Saved prompt '{name}'.")
        return 0

    if sub in {"rm", "del", "delete"}:
        if len(tokens) != 2:
            print_error("prompt rm <name>")
            return 1
        name = tokens[1]
        if name not in PROMPTS:
            print_error(f"Prompt {name!r} not found.")
            return 1
        del PROMPTS[name]
        save_prompts()
        print(f"Removed prompt '{name}'.")
        return 0

    # Ad-hoc prompt: build payload from defaults and user-supplied text/options.
    payload, err = parse_prompt_options(tokens, prompt_defaults(), require_prompt=True)
    if err:
        print_error(err)
        return 1
    print(json.dumps(payload, indent=2))
    return 0


def handle_provider(rest: str | None) -> int:
    usage = (
        "providers list [--raw|-r]\n"
        "providers add <name> --url URL --api_key KEY --default_model MODEL\n"
        "providers rename <old> <new>\n"
        "providers rm <name>"
    )
    tokens = shlex.split(rest or "")
    if not tokens:
        print_error(f"Usage:\n{usage}")
        return 1
    sub = tokens[0].lower()
    cfg = load_providers_config()
    providers = cfg.setdefault("providers", {})

    # If file missing providers but repo has defaults, hydrate from repo providers-config.yaml
    if not providers and PROVIDERS_CONFIG.exists():
        defaults = load_yaml_file(PROVIDERS_CONFIG)
        if defaults.get("providers"):
            cfg = defaults
            providers = cfg.setdefault("providers", {})

    if sub in {"list", "ls"}:
        args = tokens[1:]
        if args and args[0] in {"-r", "--raw"}:
            try:
                raw = PROVIDERS_CONFIG.read_text()
                print(raw, end="" if raw.endswith("\n") else "\n")
            except OSError as exc:
                print_error(f"Could not read providers config: {exc}")
                return 1
            return 0
        print_providers(cfg)
        return 0

    if sub == "add":
        if len(tokens) < 8:
            print_error("provider add <name> --url URL --api_key KEY --default_model MODEL")
            return 1
        name = tokens[1]
        args = tokens[2:]
        params = {"url": None, "api_key": None, "default_model": None}
        i = 0
        while i < len(args):
            key = args[i]
            if key in {"--url", "--api_key", "--default_model"}:
                if i + 1 >= len(args):
                    print_error(f"{key} requires a value")
                    return 1
                params[key[2:]] = args[i + 1]
                i += 2
                continue
            print_error(f"Unknown option {key!r}")
            return 1
        if name in providers:
            print_error(f"Provider {name!r} already exists.")
            return 1
        if not all(params.values()):
            print_error("Missing required provider fields (--url --api_key --default_model).")
            return 1
        providers[name] = params
        if not save_providers_config(cfg):
            return 1
        # initialize models file
        save_models(name, {"default_model": params["default_model"], "models": {}, "aliases": {}})
        print(f"Added provider {name}.")
        return 0

    if sub == "rename":
        if len(tokens) != 3:
            print_error("provider rename <old> <new>")
            return 1
        old, new = tokens[1], tokens[2]
        if old not in providers:
            print_error(f"Provider {old!r} not found.")
            return 1
        if new in providers:
            print_error(f"Provider {new!r} already exists.")
            return 1
        providers[new] = providers.pop(old)
        save_providers_config(cfg)
        old_dir = PROVIDERS_ROOT / old
        new_dir = PROVIDERS_ROOT / new
        try:
            if old_dir.exists():
                old_dir.rename(new_dir)
        except OSError as exc:
            print_error(f"Warning: could not rename provider directory ({exc}).")
        print(f"Renamed provider {old} -> {new}.")
        return 0

    if sub in {"rm", "del", "delete"}:
        if len(tokens) != 2:
            print_error("provider rm <name>")
            return 1
        name = tokens[1]
        if name not in providers:
            print_error(f"Provider {name!r} not found.")
            return 1
        del providers[name]
        save_providers_config(cfg)
        target_dir = PROVIDERS_ROOT / name
        try:
            if target_dir.exists():
                for path in target_dir.glob("*"):
                    path.unlink(missing_ok=True)
                target_dir.rmdir()
        except OSError:
            pass
        print(f"Removed provider {name}.")
        return 0

    print_error(f"Usage:\n{usage}")
    return 1


def handle_models(rest: str | None) -> int:
    usage = (
        "models list <provider> [--raw|-r] [--table|-t] [--details|-d]\n"
        "models hosted <provider> [--raw|-r] [--table|-t]\n"
        "models add <provider> <model> --name NAME [--description TEXT] [--max_tokens N] [--context_window N] [--default]\n"
        "models set <provider> <model> [--name NAME] [--description TEXT] [--max_tokens N] [--context_window N] [--default]\n"
        "models rm <provider> <model>"
    )
    tokens = shlex.split(rest or "")
    if not tokens:
        print_error(f"Usage:\n{usage}")
        return 1
    sub = tokens[0].lower()
    if sub == "list":
        if len(tokens) < 2:
            print_error("models list <provider> [--raw|-r] [--table|-t] [--details|-d]")
            return 1
        provider = tokens[1]
        raw = False
        use_table = False
        show_details = False
        for token in tokens[2:]:
            if token in {"-r", "--raw"}:
                raw = True
            elif token in {"-t", "--table"}:
                use_table = True
            elif token in {"-d", "--details"}:
                show_details = True
            else:
                print_error(f"Unknown option for models list: {token!r}")
                return 1
        data = load_models(provider)
        if raw:
            try:
                raw_text = provider_models_path(provider).read_text()
                print(raw_text, end="" if raw_text.endswith("\n") else "\n")
            except OSError as exc:
                print_error(f"Could not read models file: {exc}")
                return 1
        else:
            print_models(provider, data, table=use_table, details=show_details)
        return 0
    if sub == "hosted":
        if len(tokens) < 2:
            print_error("models hosted <provider> [--raw|-r] [--table|-t]")
            return 1
        provider = tokens[1]
        raw = False
        use_table = False
        for token in tokens[2:]:
            if token in {"-r", "--raw"}:
                raw = True
            elif token in {"-t", "--table"}:
                use_table = True
            else:
                print_error(f"Unknown option for model hosted: {token!r}")
                return 1
        provider_cfg_all = load_providers_config()
        providers = provider_cfg_all.get("providers", {}) if isinstance(provider_cfg_all.get("providers", {}), dict) else {}
        provider_cfg = providers.get(provider)
        if not provider_cfg:
            print_error(f"Provider {provider!r} not found in providers-config.yaml")
            return 1
        data = fetch_hosted_models(provider, provider_cfg)
        if data is None:
            return 1
        print_hosted_models(provider, provider_cfg, data, raw=raw, table=use_table)
        return 0

    if sub in {"add", "set"}:
        if len(tokens) < 3:
            print_error(f"Usage:\n{usage}")
            return 1
        provider, model = tokens[1], tokens[2]
        args = tokens[3:]
        meta: dict[str, object] = {}
        set_default = False
        i = 0
        while i < len(args):
            key = args[i]
            if key == "--name":
                if i + 1 >= len(args):
                    print_error("--name requires a value")
                    return 1
                meta["name"] = args[i + 1]
                i += 2
                continue
            if key == "--description":
                if i + 1 >= len(args):
                    print_error("--description requires a value")
                    return 1
                meta["description"] = args[i + 1]
                i += 2
                continue
            if key == "--max_tokens":
                if i + 1 >= len(args):
                    print_error("--max_tokens requires a value")
                    return 1
                try:
                    meta["max_tokens"] = int(args[i + 1])
                except ValueError:
                    print_error("--max_tokens must be an integer")
                    return 1
                i += 2
                continue
            if key == "--context_window":
                if i + 1 >= len(args):
                    print_error("--context_window requires a value")
                    return 1
                try:
                    meta["context_window"] = int(args[i + 1])
                except ValueError:
                    print_error("--context_window must be an integer")
                    return 1
                i += 2
                continue
            if key == "--default":
                set_default = True
                i += 1
                continue
            print_error(f"Unknown option {key!r}")
            return 1

        data = load_models(provider)
        models = data.setdefault("models", {})
        if not isinstance(models, dict):
            models = {}
            data["models"] = models
        existing = models.get(model) if isinstance(models.get(model), dict) else {}
        merged = {**existing, **meta}
        if not merged.get("name"):
            merged["name"] = model
        models[model] = merged
        if set_default:
            data["default_model"] = model
        if not save_models(provider, data):
            return 1
        action = "Updated" if sub == "set" and existing else "Added"
        print(f"{action} model {model} for {provider}.")
        return 0

    if sub in {"rm", "del", "delete"}:
        if len(tokens) != 3:
            print_error("model rm <provider> <model>")
            return 1
        provider, model = tokens[1], tokens[2]
        data = load_models(provider)
        models = data.get("models", {})
        if model not in models:
            print_error(f"Model {model!r} not found for provider {provider!r}.")
            return 1
        del models[model]
        if data.get("default_model") == model:
            data["default_model"] = next(iter(models.keys()), None)
        save_models(provider, data)
        print(f"Removed model {model} from {provider}.")
        return 0

    print_error(f"Usage:\n{usage}")
    return 1


def handle_alias(rest: str | None) -> int:
    usage = "alias list\nalias add <name> <expansion>\nalias rm <name>"
    tokens = shlex.split(rest or "")
    if not tokens:
        tokens = ["list"]
    sub = tokens[0].lower()
    if sub in {"list", "ls"}:
        if not ALIASES:
            print("No aliases defined.")
            return 0
        grouped: dict[str, list[str]] = {}
        for alias, cmd in ALIASES.items():
            grouped.setdefault(cmd, []).append(alias)
        for cmd in sorted(grouped):
            aliases = ", ".join(sorted(grouped[cmd]))
            print(f"{cmd:<12} - {aliases}")
        return 0
    if sub in {"add", "set"}:
        if len(tokens) < 3:
            print_error("alias add <name> <expansion>")
            return 1
        name = tokens[1]
        expansion = " ".join(tokens[2:]).strip()
        if not expansion:
            print_error("Expansion text is required.")
            return 1
        ALIASES[name] = expansion
        save_aliases()
        print(f"Alias saved: {name} -> {expansion}")
        return 0
    if sub in {"rm", "del", "delete"}:
        if len(tokens) != 2:
            print_error("alias rm <name>")
            return 1
        name = tokens[1]
        if name not in ALIASES:
            print_error(f"Alias {name!r} not found.")
            return 1
        del ALIASES[name]
        save_aliases()
        print(f"Removed alias {name}.")
        return 0
    print_error(f"Usage:\n{usage}")
    return 1


def execute_cli_line(line: str, *, record: bool = True, alias_depth: int = 0) -> tuple[int, bool]:
    """Process a single CLI line. Returns (exit_code, should_exit)."""
    stripped = line.strip()
    if not stripped:
        return 0, False

    if stripped.startswith("#"):
        ref = stripped[1:].strip()
        if ref == "-":
            index = -1
        elif not ref:
            print_error("Usage: #<number> or # <number>")
            return 1, False
        else:
            try:
                index = int(ref)
            except ValueError:
                print_error("history reference must be an integer (e.g., #3).")
                return 1, False
        return run_history_entry(index)

    if stripped.startswith("!"):
        rest = stripped[1:].strip()
        if record:
            record_history(stripped)
        return handle_sh(rest), False

    parts = shlex.split(stripped)
    if not parts:
        return 0, False

    command, *args = parts
    rest = " ".join(args) if args else None

    if command in ALIASES:
        if record:
            record_history(stripped)
        if alias_depth > 5:
            print_error("Alias expansion too deep; possible recursion.")
            return 1, False
        expanded = ALIASES[command]
        if rest:
            expanded = f"{expanded} {rest}"
        return execute_cli_line(expanded, record=False, alias_depth=alias_depth + 1)

    if command.lower() in EXIT_COMMANDS and not rest:
        if record:
            record_history(command)
        return 0, True

    if record:
        record_history(stripped)

    code = dispatch(command, rest, args)
    return code, False


def print_help(target: str | None = None) -> None:
    if target:
        tokens = shlex.split(target)
        lookup = tokens[0].lower() if tokens else ""
        sub_lookup = tokens[1].lower() if len(tokens) > 1 else None
        if lookup in {"general", "commands"}:
            print("General Commands:")
            for name, desc in GENERAL_COMMANDS.items():
                print_command_with_subcommands(name, desc)
            return
        if lookup in {"lexi", "lexi-commands", "ai", "ai-commands"}:
            print("Lexi Commands:")
            for name, desc in LEXI_COMMANDS.items():
                print_command_with_subcommands(name, desc)
            return
        if lookup in {"shortcuts", "shortcut"}:
            print("Shortcuts:")
            for name, desc in SHORTCUTS.items():
                print(f"  {name:<12} - {desc}")
            return

        if lookup == "!":
            print("! - Shortcut for 'sh' in interactive mode.")
            return
        if lookup == "?":
            print("? - Shortcut for 'help'.")
            return
        desc = GENERAL_COMMANDS.get(lookup) or LEXI_COMMANDS.get(lookup)
        if desc:
            if sub_lookup and lookup in COMMAND_SUBCOMMANDS:
                subcommands = COMMAND_SUBCOMMANDS[lookup]
                sub_meta = subcommands.get(sub_lookup)
                if sub_meta:
                    usage = sub_meta.get("usage")
                    summary = sub_meta.get("summary", "")
                    print(f"{lookup} {sub_lookup} - {summary}")
                    if usage:
                        print(f"  usage: {usage}")
                    return
            print_command_with_subcommands(lookup, desc)
            return

        shortcut_key = target if target in SHORTCUTS else target.upper() if target in {"↑", "↓"} else None
        if shortcut_key and shortcut_key in SHORTCUTS:
            print(f"{shortcut_key} - {SHORTCUTS[shortcut_key]}")
            return

        if target in {"#", "#<N>"}:
            print(f"#<N> - {SHORTCUTS['#<N>']}")
            return

        print_error(f"No help available for {target!r}.")
        return

    print("Lexi Commands:")
    for name, desc in LEXI_COMMANDS.items():
        print(f"  {name:<12} - {desc}")

    print("\nGeneral Commands:")
    for name, desc in GENERAL_COMMANDS.items():
        print(f"  {name:<12} - {desc}")

    print("\nShortcuts:")
    for name, desc in SHORTCUTS.items():
        print(f"  {name:<12} - {desc}")


def handle_sh(command: str | None) -> int:
    if not command:
        print_error("Usage: sh <command>")
        return 1
    return run_command(command)


def print_version() -> int:
    name = CONFIG.get("cli_name", CONFIG_DEFAULT["cli_name"])
    version = CONFIG.get("version", CONFIG_DEFAULT["version"])
    print(f"{name}:{version}")
    return 0


def dispatch(command: str, rest: str | None, args: list[str] | None = None) -> int:
    cmd = command.lower()
    if cmd == "help":
        print_help(rest)
        return 0
    if cmd == "?":
        print_help(rest)
        return 0
    if cmd in EXIT_COMMANDS:
        return 0
    if cmd == "sh":
        return handle_sh(rest)
    if cmd == "history":
        return handle_history(rest)
    if cmd == "config":
        return handle_config(rest)
    if cmd == "prompts":
        return handle_prompt(rest, tokens_override=args)
    if cmd == "providers":
        return handle_provider(rest)
    if cmd == "models":
        return handle_models(rest)
    if cmd == "alias":
        return handle_alias(rest)
    if cmd == "version":
        return print_version()

    print_error(f"Unknown command: {command!r}. Type 'help' to list commands.")
    return 1


def interactive_loop() -> int:
    """Read commands until EOF or an explicit exit command."""
    try:
        while True:
            try:
                prompt = CONFIG.get("prompt", CONFIG_DEFAULT["prompt"])
                delim = CONFIG.get("prompt_delimiter", CONFIG_DEFAULT["prompt_delimiter"])
                line = input(f"{prompt}{delim} ").strip()
            except EOFError:
                print()
                return 0

            code, should_exit = execute_cli_line(line)
            if should_exit:
                return 0

            if code:
                print_error(f"(command exited with {code})")
    except KeyboardInterrupt:
        print()
        return 130

    return 0


def parse_args(argv: list[str]) -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description="Run a single CLI command or enter an interactive shell loop."
    )
    parser.add_argument(
        "command",
        nargs=argparse.REMAINDER,
        help="CLI command to run once; leave empty to enter interactive mode.",
    )
    return parser.parse_args(argv)


def main(argv: list[str] | None = None) -> int:
    argv = argv if argv is not None else sys.argv[1:]
    ensure_data_dir()
    load_config()
    load_history()
    load_prompts()
    load_aliases()
    enable_line_editing()

    args = parse_args(argv)
    if args.command:
        command, *rest_tokens = args.command
        rest = " ".join(rest_tokens) if rest_tokens else None
        code = dispatch(command, rest, rest_tokens)
        return code
    return interactive_loop()


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
