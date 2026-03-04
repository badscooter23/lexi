#!/usr/bin/env python3
"""Project CLI entrypoint with interactive shell and persistent history/config."""

from __future__ import annotations

import argparse
import base64
import getpass
import importlib.metadata as importlib_metadata
import importlib.resources as importlib_resources
import json
import os
import shlex
import shutil
import subprocess
import sys
import urllib.error
import urllib.request
from collections.abc import Iterable
from pathlib import Path

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
try:
    from cryptography.fernet import Fernet
    from cryptography.hazmat.primitives import hashes
    from cryptography.hazmat.primitives.kdf.pbkdf2 import PBKDF2HMAC
except Exception:  # pragma: no cover - optional dependency
    Fernet = None

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
    "config": "View or update settings. config list [--raw|-r|key] | config set <key> <value> | config rm <key>",
    "alias": "Manage command aliases. alias list | alias add <name> <expansion> | alias rm <name> | alias reset",
    "version": "Show CLI version.",
}
AI_COMMANDS: dict[str, str] = {}

LEXI_COMMANDS = {
    "prompts": "Manage saved prompts or emit prompt JSON.",
    "providers": "Manage providers (list/add/rename/rm).",
    "models": "Manage models for a provider (list/add/set/rm).",
    "respond": "Send a prompt to a provider via the Responses API.",
}

COMMAND_SUBCOMMANDS = {
    "config": {
        "list": {
            "summary": "Show config values (optionally a single key or raw JSON).",
            "usage": "config list [--raw|-r|key]",
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
        "reset": {"summary": "Reset aliases to defaults.", "usage": "alias reset"},
    },
    "prompts": {
        "list": {
            "summary": "List prompts (optionally a single prompt).",
            "usage": "prompts list [name] [[-r|--raw] [-d|--detailed]]",
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
        "list": {"summary": "List configured providers.", "usage": "providers list [--raw|-r]"},
        "add": {
            "summary": "Add a provider.",
            "usage": "provider add <name> --url URL --api_key KEY --default_model MODEL",
        },
        "api-key": {
            "summary": "Manage API keys stored in per-provider files.",
            "usage": "providers api-key set <name> [--value KEY] | providers api-key show <name> [--reveal] | providers api-key remove <name>",
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
        "default": {
            "summary": "View or set the default model for a provider.",
            "usage": "models default <provider> [model_name]",
        },
        "alias": {
            "summary": "Manage model aliases for a provider.",
            "usage": "models alias list <provider> [--raw|-r] | models alias set <provider> <alias> <model> | models alias rm <provider> <alias>",
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
    "prompt": "cmd",
    "prompt_delimiter": ">",
    "edit_mode": "vi",
}


def get_cli_version() -> str:
    """Read CLI version from installed package metadata (pyproject.toml)."""
    try:
        return importlib_metadata.version("lexi-cli")
    except importlib_metadata.PackageNotFoundError:
        return "dev"


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


def reset_aliases_to_defaults() -> None:
    try:
        data = json.loads(DEFAULT_ALIASES_FILE.read_text())
        if isinstance(data, dict):
            ALIASES_FILE.parent.mkdir(parents=True, exist_ok=True)
            ALIASES_FILE.write_text(json.dumps(data, indent=2))
            load_aliases()
    except Exception as exc:
        print_error(f"Failed to reset aliases: {exc}")


def derive_key(passphrase: str, salt: bytes) -> bytes:
    if Fernet is None:
        raise RuntimeError("cryptography is required for encryption (pip install 'lexi-cli[crypto]').")
    kdf = PBKDF2HMAC(
        algorithm=hashes.SHA256(),
        length=32,
        salt=salt,
        iterations=390_000,
    )
    return base64.urlsafe_b64encode(kdf.derive(passphrase.encode()))


def encrypt_api_key(value: str, passphrase: str) -> dict:
    salt = os.urandom(16)
    key = derive_key(passphrase, salt)
    fernet = Fernet(key)
    token = fernet.encrypt(value.encode())
    return {"salt": base64.urlsafe_b64encode(salt).decode(), "token": token.decode()}


def decrypt_api_key(encrypted: dict, passphrase: str) -> str:
    salt_b64 = encrypted.get("salt")
    token = encrypted.get("token")
    if not salt_b64 or not token:
        raise ValueError("Encrypted key missing salt or token.")
    salt = base64.urlsafe_b64decode(salt_b64.encode())
    key = derive_key(passphrase, salt)
    fernet = Fernet(key)
    return fernet.decrypt(token.encode()).decode()


def extract_prompt_payload(name: str) -> dict | None:
    target = name or PROMPT_ACTIVE_NAME
    if target == "$$":
        target = PROMPT_ACTIVE_NAME
    payload = PROMPTS.get(target)
    if not payload:
        return None
    return payload


def resolve_path(data: object, path_expr: str) -> object | None:
    """Walk a dot/bracket path expression into nested dicts/lists.

    Examples: ``"content[0].text"``, ``"choices[0].message.content"``.
    Returns ``None`` when any segment fails to resolve.
    """
    import re

    tokens = re.split(r"\.(?![^\[]*\])", path_expr)
    current: object = data
    for token in tokens:
        parts = re.split(r"[\[\]]", token)
        parts = [p for p in parts if p != ""]
        for part in parts:
            if current is None:
                return None
            if part.isdigit():
                idx = int(part)
                if isinstance(current, list) and idx < len(current):
                    current = current[idx]
                else:
                    return None
            elif isinstance(current, dict):
                current = current.get(part)
            else:
                return None
    return current


def build_auth_headers(provider_cfg: dict, api_key: str) -> dict[str, str]:
    """Build HTTP headers from provider auth config with Bearer token defaults."""
    auth = provider_cfg.get("auth", {})
    header_name = auth.get("header", "Authorization")
    prefix = auth.get("value_prefix", "Bearer ")
    headers: dict[str, str] = {"Content-Type": "application/json", "Accept": "application/json"}
    headers[header_name] = f"{prefix}{api_key}"
    for k, v in auth.get("extra_headers", {}).items():
        headers[k] = str(v)
    return headers


def render_body_template(template: object, variables: dict[str, object]) -> object:
    """Recursively substitute ``{{var}}`` placeholders in a nested structure.

    When a string value is exactly ``"{{var}}"``, the raw variable value replaces
    it (preserving type).  Partial placeholders are string-substituted.
    """
    if isinstance(template, str):
        # Exact placeholder — return raw value to preserve type
        stripped = template.strip()
        if stripped.startswith("{{") and stripped.endswith("}}"):
            key = stripped[2:-2].strip()
            if key in variables:
                return variables[key]
        # Partial substitution
        import re

        def _sub(m: re.Match) -> str:  # type: ignore[type-arg]
            key = m.group(1).strip()
            return str(variables.get(key, m.group(0)))

        return re.sub(r"\{\{(.+?)\}\}", _sub, template)
    if isinstance(template, dict):
        return {k: render_body_template(v, variables) for k, v in template.items()}
    if isinstance(template, list):
        return [render_body_template(item, variables) for item in template]
    return template


def build_request_body(
    provider_cfg: dict,
    *,
    model: str,
    role: str,
    prompt: str,
    temperature: float | None,
    max_tokens: int | None,
    tools_payload: list | None,
) -> dict:
    """Build the JSON request body from the provider's request config."""
    req_cfg = provider_cfg.get("request", {})
    body_template = req_cfg.get("body")
    if not body_template:
        # Fallback: OpenAI responses format
        body_template = {
            "model": "{{model}}",
            "input": [{"role": "{{role}}", "content": [{"type": "input_text", "text": "{{prompt}}"}]}],
        }
    variables: dict[str, object] = {"model": model, "role": role, "prompt": prompt}
    body: dict = render_body_template(body_template, variables)  # type: ignore[assignment]
    param_mapping = req_cfg.get("param_mapping", {})
    if temperature is not None:
        body[param_mapping.get("temperature", "temperature")] = temperature
    if max_tokens is not None:
        body[param_mapping.get("max_tokens", "max_tokens")] = max_tokens
    if tools_payload:
        tools_key = req_cfg.get("tools_key", "tools")
        if tools_key:
            body[tools_key] = tools_payload
    return body


_RESPOND_KIND_TEMPLATES: dict[str, dict] = {
    "chat_completions": {
        "request": {
            "path": "/chat/completions",
            "body": {
                "model": "{{model}}",
                "messages": [{"role": "{{role}}", "content": "{{prompt}}"}],
            },
            "param_mapping": {"temperature": "temperature", "max_tokens": "max_tokens"},
            "tools_key": "tools",
        },
        "response": {"text_path": "choices[0].message.content"},
    },
    "messages": {
        "auth": {
            "header": "x-api-key",
            "value_prefix": "",
            "extra_headers": {"anthropic-version": "2023-06-01"},
        },
        "request": {
            "path": "/messages",
            "body": {
                "model": "{{model}}",
                "messages": [{"role": "{{role}}", "content": [{"type": "text", "text": "{{prompt}}"}]}],
            },
            "param_mapping": {"temperature": "temperature", "max_tokens": "max_tokens"},
        },
        "response": {"text_path": "content[0].text"},
    },
    "responses": {
        "request": {
            "path": "/responses",
            "body": {
                "model": "{{model}}",
                "input": [{"role": "{{role}}", "content": [{"type": "input_text", "text": "{{prompt}}"}]}],
            },
            "param_mapping": {"temperature": "temperature", "max_tokens": "max_output_tokens"},
            "tools_key": "tools",
        },
        "response": {"text_path": "output[0].content[0].text"},
    },
}


def migrate_respond_config(provider: str, provider_cfg: dict) -> dict:
    """Runtime migration for old configs lacking ``request``/``response``/``auth`` blocks.

    Returns a new dict with the legacy ``respond_kind`` / ``respond_path`` expanded into
    the new config format.  If ``request`` is already present, returns the config as-is.
    """
    if "request" in provider_cfg:
        return provider_cfg
    respond_kind = provider_cfg.get("respond_kind", "responses")
    # Legacy nvidia hack: default to chat_completions
    if respond_kind == "responses" and provider.lower() == "nvidia":
        respond_kind = "chat_completions"
    template = _RESPOND_KIND_TEMPLATES.get(respond_kind)
    if not template:
        return provider_cfg
    cfg = dict(provider_cfg)
    for key in ("auth", "request", "response"):
        if key in template and key not in cfg:
            cfg[key] = template[key]
    # Honour legacy respond_path override
    respond_path = provider_cfg.get("respond_path")
    if respond_path and "request" in cfg:
        cfg["request"] = dict(cfg["request"])
        cfg["request"]["path"] = respond_path
    return cfg


def respond_with_provider(
    provider: str,
    provider_cfg: dict,
    prompt_payload: dict,
    *,
    model_override: str | None = None,
    temperature_override: float | None = None,
    max_tokens_override: int | None = None,
    web_search: bool = False,
) -> tuple[int, dict | None]:
    provider_cfg = migrate_respond_config(provider, provider_cfg)
    api_key = load_api_key(provider, provider_cfg)
    base_url = (provider_cfg.get("url") or "").rstrip("/")
    model = model_override or provider_cfg.get("default_model")
    if not api_key or not base_url or not model:
        print_error("Provider config missing url/api_key/default_model.")
        return 1, None

    req_cfg = provider_cfg.get("request", {})
    req_path = req_cfg.get("path", "/responses")
    endpoint = f"{base_url}{req_path}"

    prompt_text = prompt_payload.get("prompt", "")
    if not prompt_text:
        print_error("Prompt text is empty.")
        return 1, None
    role = prompt_payload.get("role", "user")
    temperature = float(temperature_override) if temperature_override is not None else prompt_payload.get("temperature")
    max_tokens = int(max_tokens_override) if max_tokens_override is not None else prompt_payload.get("max_tokens")

    tools_payload = [{"type": "web_search"}] if web_search else None
    body = build_request_body(
        provider_cfg,
        model=model,
        role=role,
        prompt=prompt_text,
        temperature=temperature,
        max_tokens=max_tokens,
        tools_payload=tools_payload,
    )
    headers = build_auth_headers(provider_cfg, api_key)

    try:
        req = urllib.request.Request(endpoint, data=json.dumps(body).encode(), headers=headers)  # type: ignore[arg-type]
        with urllib.request.urlopen(req, timeout=30) as resp:  # type: ignore[call-arg]
            raw = resp.read().decode("utf-8")
            try:
                return 0, json.loads(raw)
            except Exception:
                print(raw)
                return 0, None
    except urllib.error.HTTPError as exc:
        detail = ""
        try:
            detail_body = exc.read().decode("utf-8")
            if detail_body:
                detail = f": {detail_body}"
        except Exception:
            pass
        print_error(f"HTTP error ({exc.code}) from provider: {exc.reason}{detail}")
        return exc.code or 1, None
    except urllib.error.URLError as exc:
        print_error(f"Network error: {exc}")
        return 1, None
    except Exception as exc:
        print_error(f"Unexpected error: {exc}")
        return 1, None


def render_response_output(data: dict | None, *, raw: bool, provider_cfg: dict | None = None) -> None:
    if data is None:
        print_error("No response payload.")
        return
    if raw:
        print(json.dumps(data, indent=2))
        return
    # Config-driven extraction via text_path
    if provider_cfg:
        resp_cfg = provider_cfg.get("response", {})
        text_path = resp_cfg.get("text_path")
        if text_path:
            text = resolve_path(data, text_path)
            if isinstance(text, str) and text:
                print(text)
                return
    # Fallback chain for backward compatibility
    # 1. OpenAI responses format: output[0].content[0].text
    output = data.get("output") if isinstance(data, dict) else None
    if isinstance(output, list) and output:
        first = output[0]
        if isinstance(first, dict):
            content = first.get("content")
            if isinstance(content, list) and content:
                item = content[0]
                if isinstance(item, dict):
                    text = item.get("text") or item.get("value") or item.get("message") or ""
                    if text:
                        print(text)
                        return
    # 2. OpenAI chat completions format: choices[0].message.content
    choices = data.get("choices") if isinstance(data, dict) else None
    if isinstance(choices, list) and choices:
        first = choices[0]
        if isinstance(first, dict):
            message = first.get("message", {})
            if isinstance(message, dict):
                content = message.get("content")
                if isinstance(content, str):
                    print(content)
                    return
    # 3. Anthropic messages format: content[0].text
    if isinstance(data, dict):
        content = data.get("content")
        if isinstance(content, list) and content:
            first = content[0]
            if isinstance(first, dict):
                text = first.get("text")
                if isinstance(text, str) and text:
                    print(text)
                    return
    # Last resort: pretty print
    print(json.dumps(data, indent=2))


def read_alias_file(path: Path) -> dict:
    try:
        data = json.loads(path.read_text())
        return data if isinstance(data, dict) else {}
    except Exception:
        return {}


def print_error(message: str) -> None:
    print(f"{RED}{message}{RESET}")


def print_pretty_json_if_possible(text: str) -> bool:
    try:
        data = json.loads(text)
        if Console:
            console.print_json(json.dumps(data))
        else:
            print(json.dumps(data, indent=2))
        return True
    except Exception:
        return False


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
        dest_dir = PROVIDERS_ROOT / entry.name
        dest_dir.mkdir(parents=True, exist_ok=True)

        src_models = entry / "configured-models.yaml"
        if not src_models.exists():
            src_models = entry / "models.yaml"
        dest_models = dest_dir / "configured-models.yaml"
        if src_models.exists() and not dest_models.exists():
            try:
                shutil.copyfile(src_models, dest_models)
            except OSError as exc:
                print_error(f"Warning: could not seed models for {entry.name} ({exc}).")

        src_aliases = entry / "model-aliases.yaml"
        dest_aliases = dest_dir / "model-aliases.yaml"
        if src_aliases.exists() and not dest_aliases.exists():
            try:
                shutil.copyfile(src_aliases, dest_aliases)
            except OSError as exc:
                print_error(f"Warning: could not seed model aliases for {entry.name} ({exc}).")

        src_default = entry / "default-model.yaml"
        dest_default = dest_dir / "default-model.yaml"
        if src_default.exists() and not dest_default.exists():
            try:
                shutil.copyfile(src_default, dest_default)
            except OSError as exc:
                print_error(f"Warning: could not seed default model for {entry.name} ({exc}).")

        src_api_key = entry / "api-key"
        dest_api_key = dest_dir / "api-key"
        if src_api_key.exists() and not dest_api_key.exists():
            try:
                shutil.copyfile(src_api_key, dest_api_key)
                dest_api_key.chmod(0o600)
            except OSError as exc:
                print_error(f"Warning: could not seed api-key for {entry.name} ({exc}).")


def load_providers_config() -> dict:
    data = load_yaml_file(PROVIDERS_CONFIG)
    data = expand_env_vars(data)
    return data if isinstance(data, dict) else {}


def save_providers_config(data: dict) -> bool:
    return save_yaml_file(PROVIDERS_CONFIG, data)


def provider_models_path(name: str) -> Path:
    return PROVIDERS_ROOT / name / "configured-models.yaml"


def provider_default_model_path(name: str) -> Path:
    return PROVIDERS_ROOT / name / "default-model.yaml"


def provider_model_aliases_path(name: str) -> Path:
    return PROVIDERS_ROOT / name / "model-aliases.yaml"


def provider_api_key_path(name: str) -> Path:
    """Return the path to the plain-text api-key file for a provider."""
    return PROVIDERS_ROOT / name / "api-key"


def load_api_key(name: str, provider_cfg: dict) -> str:
    """Load API key with priority: api-key file > env var in config > literal in config."""
    key_file = provider_api_key_path(name)
    if key_file.exists():
        try:
            return key_file.read_text().strip()
        except OSError:
            pass
    return provider_cfg.get("api_key") or ""


def save_api_key(name: str, key: str) -> bool:
    """Write API key to file with chmod 0o600."""
    key_file = provider_api_key_path(name)
    try:
        key_file.parent.mkdir(parents=True, exist_ok=True)
        key_file.write_text(key + "\n")
        key_file.chmod(0o600)
        return True
    except OSError as exc:
        print_error(f"Could not save API key: {exc}")
        return False


def remove_api_key(name: str) -> bool:
    """Delete the api-key file for a provider."""
    key_file = provider_api_key_path(name)
    if not key_file.exists():
        return False
    try:
        key_file.unlink()
        return True
    except OSError as exc:
        print_error(f"Could not remove API key: {exc}")
        return False


def load_models(provider: str) -> dict:
    path = provider_models_path(provider)
    data = load_yaml_file(path)
    if data:
        return data
    legacy = PROVIDERS_ROOT / provider / "models.yaml"
    if legacy.exists():
        return load_yaml_file(legacy)
    return {}


def save_models(provider: str, data: dict) -> bool:
    return save_yaml_file(provider_models_path(provider), data)


def load_default_model(provider: str) -> str | None:
    data = load_yaml_file(provider_default_model_path(provider))
    if isinstance(data, dict):
        return data.get("default_model")
    if isinstance(data, str):
        return data
    return None


def save_default_model(provider: str, model: str | None) -> bool:
    if model is None:
        return save_yaml_file(provider_default_model_path(provider), {})
    return save_yaml_file(provider_default_model_path(provider), {"default_model": model})


def load_model_aliases(provider: str) -> dict:
    data = load_yaml_file(provider_model_aliases_path(provider))
    if not isinstance(data, dict):
        return {}
    if "aliases" in data and isinstance(data["aliases"], dict):
        return data["aliases"]
    return data


def save_model_aliases(provider: str, aliases: dict) -> bool:
    """Save model aliases, preserving the file's existing wrapper format.

    If the raw file uses an ``aliases:`` key, wrap the data; otherwise save flat.
    """
    path = provider_model_aliases_path(provider)
    wrapped = False
    if path.exists():
        try:
            raw = load_yaml_file(path)
            if isinstance(raw, dict) and "aliases" in raw:
                wrapped = True
        except Exception:
            pass
    data = {"aliases": aliases} if wrapped else aliases
    return save_yaml_file(path, data)


def load_config() -> None:
    global CONFIG
    if CONFIG_FILE.exists():
        try:
            data = json.loads(CONFIG_FILE.read_text())
            if not isinstance(data, dict):
                raise ValueError("Config file must contain a JSON object.")
            # Remove legacy 'version' key — version is now read from package metadata
            dirty = "version" in data
            data.pop("version", None)
            CONFIG = {**CONFIG_DEFAULT, **{k: str(v) for k, v in data.items()}}
            if dirty:
                save_config()
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
        hosted_models = cfg.get("hosted_models", "")
        hosted_fields = cfg.get("hosted_model_fields", [])
        api_key = "[set]" if load_api_key(name, cfg) else "[missing]"
        fields = ", ".join(hosted_fields) if isinstance(hosted_fields, list) else hosted_fields
        print(f"{name}: url={url} hosted={hosted_models} fields=[{fields}] api_key={api_key}")


def print_models(
    provider: str, models_data: dict, aliases_data: dict | None = None, *, table: bool = False, details: bool = False
) -> None:
    default_model = load_default_model(provider)
    models_data_dict = models_data if isinstance(models_data, dict) else {}
    models = models_data_dict.get("models", {}) if isinstance(models_data_dict.get("models", {}), dict) else {}
    aliases = aliases_data if isinstance(aliases_data, dict) else {}
    alias_lookup: dict[str, list[str]] = {}
    for alias, target in aliases.items():
        if isinstance(target, str):
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
    api_key = load_api_key(provider, provider_cfg)
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

    migrated_cfg = migrate_respond_config(provider, provider_cfg)
    headers = build_auth_headers(migrated_cfg, api_key)

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
        fields = list(items[0].keys()) if isinstance(items[0], dict) else ["id"]

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
            import contextlib

            with contextlib.suppress(Exception):
                readline.add_history(trimmed)
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
        # Support escaped newlines in shell input (e.g., "\n") and preserve literal newlines.
        prompt_text = prompt_text.replace("\\n", "\n")
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
            import contextlib

            with contextlib.suppress(Exception):
                readline.clear_history()
        print("History cleared.")
        if count is None:
            return 0

    print_history(count)
    return 0


def handle_config(rest: str | None, tokens_override: list[str] | None = None) -> int:
    if not rest and tokens_override is None:
        print_error("Usage: config list [--raw|-r|key] | config set <key> <value> | config rm <key>")
        return 1

    tokens = tokens_override if tokens_override is not None else shlex.split(rest)
    if not tokens:
        print_error("Usage: config list [--raw|-r|key] | config set <key> <value> | config rm <key>")
        return 1

    subcommand = tokens[0].lower()
    if subcommand == "list":
        raw_requested = False
        if "--raw" in tokens[1:] or "-r" in tokens[1:]:
            raw_requested = True
            tokens = [t for t in tokens if t not in {"--raw", "-r"}]

        if raw_requested and len(tokens) == 1:
            try:
                raw = CONFIG_FILE.read_text()
            except OSError as exc:
                print_error(f"Could not read config file: {exc}")
                return 1
            if not print_pretty_json_if_possible(raw):
                print(raw, end="" if raw.endswith("\n") else "\n")
            return 0

        # Support list <key> [--value|-v] and list --value|-v <key>
        if len(tokens) in {1, 2, 3}:
            value_only = False
            key = None
            for tok in tokens[1:]:
                if tok in {"--value", "-v"}:
                    value_only = True
                else:
                    key = tok

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
                if not print_pretty_json_if_possible(raw):
                    print(raw, end="" if raw.endswith("\n") else "\n")
                return 0

            print_config_values()
            return 0

        print_error("Usage: config list [--raw|-r|key]")
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

    print_error("Usage: config list [--raw|-r|key] | config set <key> <value> | config rm <key>")
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

    tokens = tokens_override if tokens_override is not None else shlex.split(rest)
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


def handle_provider(rest: str | None, tokens_override: list[str] | None = None) -> int:
    usage = (
        "providers list [--raw|-r]\n"
        "providers add <name> --url URL --api_key KEY --default_model MODEL\n"
        "providers api-key set <name> [--value KEY]\n"
        "providers api-key show <name> [--reveal]\n"
        "providers api-key remove <name>\n"
        "providers rename <old> <new>\n"
        "providers rm <name>"
    )
    tokens = tokens_override if tokens_override is not None else shlex.split(rest or "")
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
                if not print_pretty_json_if_possible(raw):
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
        # Save api key to file if provided
        if params.get("api_key"):
            save_api_key(name, params["api_key"])
        # initialize models file
        save_models(name, {"default_model": params["default_model"], "models": {}, "aliases": {}})
        print(f"Added provider {name}.")
        return 0

    if sub == "api-key":
        args = tokens[1:]
        api_key_usage = (
            "providers api-key set <name> [--value KEY]\n"
            "providers api-key show <name> [--reveal]\n"
            "providers api-key remove <name>"
        )
        if not args:
            print_error(f"Usage:\n{api_key_usage}")
            return 1

        action = args[0].lower()
        # Legacy compat: `providers api-key <name>` (no action keyword) treated as `set`
        if action not in {"set", "show", "remove"}:
            # Treat as provider name with implicit 'set'
            action = "set"
            args = ["set"] + args

        if action == "set":
            if len(args) < 2:
                print_error("providers api-key set <name> [--value KEY]")
                return 1
            name = args[1]
            if name not in providers:
                print_error(f"Provider {name!r} not found.")
                return 1
            # Parse optional --value
            key_value = None
            i = 2
            while i < len(args):
                if args[i] in {"--value", "-v"}:
                    if i + 1 >= len(args):
                        print_error("--value requires a value")
                        return 1
                    key_value = args[i + 1]
                    i += 2
                else:
                    print_error(f"Unknown option {args[i]!r}")
                    return 1
            if key_value is None:
                if sys.stdin.isatty():
                    key_value = getpass.getpass("Enter API key: ").strip()
                else:
                    key_value = sys.stdin.readline().strip()
            if not key_value:
                print_error("API key cannot be empty.")
                return 1
            if save_api_key(name, key_value):
                print(f"Saved API key for {name}.")
                return 0
            return 1

        if action == "show":
            if len(args) < 2:
                print_error("providers api-key show <name> [--reveal]")
                return 1
            name = args[1]
            if name not in providers:
                print_error(f"Provider {name!r} not found.")
                return 1
            reveal = any(a in {"--reveal", "-r"} for a in args[2:])
            key = load_api_key(name, providers[name])
            if not key:
                print(f"No API key configured for {name}.")
                return 0
            if reveal:
                print(key)
            else:
                if len(key) > 8:
                    print(f"{key[:4]}{'*' * (len(key) - 8)}{key[-4:]}")
                else:
                    print("*" * len(key))
            return 0

        if action == "remove":
            if len(args) < 2:
                print_error("providers api-key remove <name>")
                return 1
            name = args[1]
            if name not in providers:
                print_error(f"Provider {name!r} not found.")
                return 1
            if remove_api_key(name):
                print(f"Removed API key for {name}.")
            else:
                print(f"No API key file found for {name}.")
            return 0

        print_error(f"Usage:\n{api_key_usage}")
        return 1

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


def handle_models(rest: str | None, tokens_override: list[str] | None = None) -> int:
    usage = (
        "models list <provider> [--raw|-r] [--aliases|-a] [--table|-t] [--details|-d]\n"
        "models hosted <provider> [--raw|-r] [--table|-t]\n"
        "models default <provider> [model]\n"
        "models alias list <provider> [--raw|-r]\n"
        "models alias set <provider> <alias> <model>\n"
        "models alias rm <provider> <alias>\n"
        "models add <provider> <model> --name NAME [--description TEXT] [--max_tokens N] [--context_window N] [--default]\n"
        "models set <provider> <model> [--name NAME] [--description TEXT] [--max_tokens N] [--context_window N] [--default]\n"
        "models rm <provider> <model>"
    )
    tokens = tokens_override if tokens_override is not None else shlex.split(rest or "")
    if not tokens:
        print_error(f"Usage:\n{usage}")
        return 1
    sub = tokens[0].lower()
    if sub == "list":
        if len(tokens) < 2:
            print_error("models list <provider> [--raw|-r] [--aliases|-a] [--table|-t] [--details|-d]")
            return 1
        provider = tokens[1]
        raw = False
        use_table = False
        show_details = False
        include_aliases = False
        for token in tokens[2:]:
            if token in {"-r", "--raw"}:
                raw = True
            elif token in {"-a", "--aliases"}:
                include_aliases = True
            elif token in {"-t", "--table"}:
                use_table = True
            elif token in {"-d", "--details"}:
                show_details = True
            else:
                print_error(f"Unknown option for models list: {token!r}")
                return 1
        data = load_models(provider)
        aliases = load_model_aliases(provider)
        if raw:
            try:
                model_path = provider_models_path(provider)
                if not model_path.exists():
                    legacy_path = PROVIDERS_ROOT / provider / "models.yaml"
                    model_path = legacy_path if legacy_path.exists() else model_path
                raw_models = model_path.read_text() if model_path.exists() else ""
                printed = False
                if raw_models:
                    printed = print_pretty_json_if_possible(raw_models)
                if not printed and raw_models:
                    print(raw_models, end="" if raw_models.endswith("\n") else "\n")
                if include_aliases:
                    alias_path = provider_model_aliases_path(provider)
                    raw_aliases = alias_path.read_text() if alias_path.exists() else ""
                    if raw_aliases:
                        print("\n# aliases")
                        if not print_pretty_json_if_possible(raw_aliases):
                            print(raw_aliases, end="" if raw_aliases.endswith("\n") else "\n")
            except OSError as exc:
                print_error(f"Could not read models file: {exc}")
                return 1
        else:
            print_models(provider, data, aliases, table=use_table, details=show_details)
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
        providers = (
            provider_cfg_all.get("providers", {}) if isinstance(provider_cfg_all.get("providers", {}), dict) else {}
        )
        provider_cfg = providers.get(provider)
        if not provider_cfg:
            print_error(f"Provider {provider!r} not found in providers-config.yaml")
            return 1
        data = fetch_hosted_models(provider, provider_cfg)
        if data is None:
            return 1
        print_hosted_models(provider, provider_cfg, data, raw=raw, table=use_table)
        return 0

    if sub == "default":
        if len(tokens) not in {2, 3}:
            print_error("models default <provider> [model]")
            return 1
        provider = tokens[1]
        data = load_models(provider)
        models = data.get("models", {}) if isinstance(data.get("models", {}), dict) else {}
        if len(tokens) == 2:
            default_model = load_default_model(provider)
            if default_model:
                print(f"Default model for {provider}: {default_model}")
            else:
                print(f"No default model set for {provider}.")
            return 0
        new_default = tokens[2]
        if models and new_default not in models:
            print_error(f"Model {new_default!r} not found for provider {provider!r}.")
            return 1
        if not save_default_model(provider, new_default):
            print_error("Failed to save default model.")
            return 1
        print(f"Set default model for {provider} to {new_default}.")
        return 0

    if sub == "alias":
        alias_usage = (
            "models alias list <provider> [--raw|-r]\n"
            "models alias set <provider> <alias> <model>\n"
            "models alias rm <provider> <alias>"
        )
        if len(tokens) < 2:
            print_error(f"Usage:\n{alias_usage}")
            return 1
        alias_sub = tokens[1].lower()
        if alias_sub == "list":
            if len(tokens) < 3:
                print_error("models alias list <provider> [--raw|-r]")
                return 1
            provider = tokens[2]
            raw = any(t in {"-r", "--raw"} for t in tokens[3:])
            if raw:
                alias_path = provider_model_aliases_path(provider)
                if alias_path.exists():
                    content = alias_path.read_text()
                    if not print_pretty_json_if_possible(content):
                        print(content, end="" if content.endswith("\n") else "\n")
                else:
                    print(f"No aliases file for {provider}.")
            else:
                aliases = load_model_aliases(provider)
                if not aliases:
                    print(f"No aliases configured for {provider}.")
                else:
                    for alias_name, model_id in sorted(aliases.items()):
                        print(f"  {alias_name} -> {model_id}")
            return 0
        if alias_sub == "set":
            if len(tokens) != 5:
                print_error("models alias set <provider> <alias> <model>")
                return 1
            provider, alias_name, model_id = tokens[2], tokens[3], tokens[4]
            aliases = load_model_aliases(provider)
            aliases[alias_name] = model_id
            if not save_model_aliases(provider, aliases):
                print_error("Failed to save model aliases.")
                return 1
            print(f"Set alias '{alias_name}' -> '{model_id}' for {provider}.")
            return 0
        if alias_sub in {"rm", "del", "delete"}:
            if len(tokens) != 4:
                print_error("models alias rm <provider> <alias>")
                return 1
            provider, alias_name = tokens[2], tokens[3]
            aliases = load_model_aliases(provider)
            if alias_name not in aliases:
                print_error(f"Alias '{alias_name}' not found for {provider}.")
                return 1
            del aliases[alias_name]
            if not save_model_aliases(provider, aliases):
                print_error("Failed to save model aliases.")
                return 1
            print(f"Removed alias '{alias_name}' from {provider}.")
            return 0
        print_error(f"Usage:\n{alias_usage}")
        return 1

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
        models[model] = merged
        if set_default:
            save_default_model(provider, model)
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
        default_model = load_default_model(provider)
        if default_model == model:
            new_default = next(iter(models.keys()), None)
            save_default_model(provider, new_default)
        save_models(provider, data)
        print(f"Removed model {model} from {provider}.")
        return 0

    print_error(f"Usage:\n{usage}")
    return 1


def handle_alias(rest: str | None, tokens_override: list[str] | None = None) -> int:
    usage = "alias list\nalias add <name> <expansion>\nalias rm <name>\nalias reset"
    tokens = tokens_override if tokens_override is not None else shlex.split(rest or "")
    if not tokens:
        tokens = ["list"]
    sub = tokens[0].lower()
    if sub in {"list", "ls"}:
        if not ALIASES:
            print("No aliases defined.")
            return 0
        names_to_show = set(tokens[1:]) if len(tokens) > 1 else None
        grouped: dict[str, list[str]] = {}
        for alias, cmd in ALIASES.items():
            grouped.setdefault(cmd, []).append(alias)
        for cmd in sorted(grouped):
            aliases_list = sorted(grouped[cmd])
            if names_to_show and cmd not in names_to_show and not any(a in names_to_show for a in aliases_list):
                continue
            aliases = ", ".join(aliases_list)
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
    if sub == "reset":
        reset_aliases_to_defaults()
        print("Aliases reset to defaults.")
        return 0
    print_error(f"Usage:\n{usage}")
    return 1


def command_aliases_for(target_command: str) -> list[str]:
    return sorted([alias for alias, cmd in ALIASES.items() if cmd == target_command])


def handle_respond(rest: str | None, tokens_override: list[str] | None = None) -> int:
    usage = (
        "respond <provider> [prompt_name] [--model MODEL] [--temperature T] [--max_tokens N] [--web-search] [--raw|-r]"
    )
    tokens = tokens_override if tokens_override is not None else shlex.split(rest or "")
    if not tokens:
        print_error(f"Usage:\n{usage}")
        return 1
    provider = tokens[0]
    prompt_name = PROMPT_ACTIVE_NAME
    idx = 1
    if len(tokens) > 1 and not tokens[1].startswith("-"):
        prompt_name = tokens[1]
        idx = 2

    model_override: str | None = None
    temp_override: float | None = None
    max_tokens_override: int | None = None
    raw = False
    web_search = False

    while idx < len(tokens):
        tok = tokens[idx]
        if tok in {"--model"}:
            if idx + 1 >= len(tokens):
                print_error("--model requires a value")
                return 1
            model_override = tokens[idx + 1]
            idx += 2
            continue
        if tok in {"--temperature"}:
            if idx + 1 >= len(tokens):
                print_error("--temperature requires a value")
                return 1
            try:
                temp_override = float(tokens[idx + 1])
            except ValueError:
                print_error("--temperature must be a number")
                return 1
            idx += 2
            continue
        if tok in {"--max_tokens"}:
            if idx + 1 >= len(tokens):
                print_error("--max_tokens requires a value")
                return 1
            try:
                max_tokens_override = int(tokens[idx + 1])
            except ValueError:
                print_error("--max_tokens must be an integer")
                return 1
            idx += 2
            continue
        if tok == "--web-search":
            web_search = True
            idx += 1
            continue
        if tok in {"-r", "--raw"}:
            raw = True
            idx += 1
            continue
        print_error(f"Unknown option {tok!r}")
        return 1

    provider_cfg_all = load_providers_config()
    providers = provider_cfg_all.get("providers", {}) if isinstance(provider_cfg_all.get("providers", {}), dict) else {}
    provider_cfg = providers.get(provider)
    if not provider_cfg:
        print_error(f"Provider {provider!r} not found in providers-config.yaml")
        return 1

    # Resolve model alias and validate model name
    if model_override:
        aliases = load_model_aliases(provider)
        if model_override in aliases:
            model_override = aliases[model_override]
        else:
            models_data = load_models(provider)
            configured = set()
            if isinstance(models_data, dict):
                models_section = models_data.get("models", models_data)
                if isinstance(models_section, dict):
                    configured = set(models_section.keys())
            if configured and model_override not in configured:
                alias_names = sorted(aliases.keys()) if aliases else []
                msg = f"Model {model_override!r} is not a configured model or alias for provider {provider!r}."
                if alias_names:
                    msg += f"\nAvailable aliases: {', '.join(alias_names)}"
                if configured:
                    msg += f"\nConfigured models: {', '.join(sorted(configured))}"
                print_error(msg)
                return 1

    # If no explicit model was requested, look up the default from default-model.yaml
    if not model_override:
        file_default = load_default_model(provider)
        if file_default:
            provider_cfg = {**provider_cfg, "default_model": file_default}

    # Model-level override merge: if the model entry in configured-models.yaml
    # has auth/request/response keys, deep-merge them into the provider config.
    effective_model = model_override or provider_cfg.get("default_model")
    if effective_model:
        models_data = load_models(provider)
        models_section = models_data.get("models", models_data) if isinstance(models_data, dict) else {}
        model_meta = models_section.get(effective_model, {}) if isinstance(models_section, dict) else {}
        if isinstance(model_meta, dict):
            for key in ("auth", "request", "response"):
                if key in model_meta:
                    provider_cfg = dict(provider_cfg)
                    existing = provider_cfg.get(key)
                    if isinstance(existing, dict) and isinstance(model_meta[key], dict):
                        provider_cfg[key] = {**existing, **model_meta[key]}
                    else:
                        provider_cfg[key] = model_meta[key]

    prompt_payload = extract_prompt_payload(prompt_name)
    if prompt_payload is None:
        print_error(f"Prompt {prompt_name!r} not found.")
        return 1

    # Migrate config before calling respond and render so both share the same effective config
    effective_cfg = migrate_respond_config(provider, provider_cfg)

    code, resp_json = respond_with_provider(
        provider,
        effective_cfg,
        prompt_payload,
        model_override=model_override,
        temperature_override=temp_override,
        max_tokens_override=max_tokens_override,
        web_search=web_search,
    )
    if code != 0:
        return code
    render_response_output(resp_json, raw=raw, provider_cfg=effective_cfg)
    return 0


def execute_cli_line(line: str, *, record: bool = True, alias_depth: int = 0) -> tuple[int, bool]:
    """Process a single CLI line. Returns (exit_code, should_exit)."""
    stripped = line.strip()
    if not stripped:
        return 0, False

    try:
        parts = shlex.split(stripped)
    except ValueError as exc:
        print_error(f"Could not parse command ({exc}). For multi-line prompts, escape newlines as \\n.")
        return 1, False

    if not parts:
        return 0, False

    if parts[0].startswith("#"):
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

    if parts[0].startswith("!"):
        rest = stripped[1:].strip()
        if record:
            record_history(stripped)
        return handle_sh(rest), False

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
        # Resolve command aliases
        if lookup in ALIASES:
            lookup = ALIASES[lookup]
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
            aliases_for_cmd = command_aliases_for(lookup)
            if aliases_for_cmd:
                print(f"  aliases: {', '.join(aliases_for_cmd)}")
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
    version = get_cli_version()
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
        return handle_config(rest, tokens_override=args)
    if cmd == "prompts":
        return handle_prompt(rest, tokens_override=args)
    if cmd == "providers":
        return handle_provider(rest, tokens_override=args)
    if cmd == "models":
        return handle_models(rest, tokens_override=args)
    if cmd == "alias":
        return handle_alias(rest, tokens_override=args)
    if cmd == "respond":
        return handle_respond(rest, tokens_override=args)
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

    except KeyboardInterrupt:
        print()
        return 130

    return 0


def parse_args(argv: list[str]) -> argparse.Namespace:
    parser = argparse.ArgumentParser(description="Run a single CLI command or enter an interactive shell loop.")
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
