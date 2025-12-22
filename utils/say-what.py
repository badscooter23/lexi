#!/usr/bin/env python3
"""
Minimal utility to send a prompt to a provider using LiteLLM.

Usage:
    python say-what.py --prompt <name-or-path> --provider <provider> [--model MODEL]

Assumptions:
- Prompts follow the lexi-cli JSON format (keys: prompt, max_tokens, temperature, role)
  and live under the local `prompts/` directory (e.g., prompts/examples/hello-world.json).
- Provider API keys live in `providers/<provider>/api-key`.
- Provider metadata (url/default_model) is read from `providers/providers-config.yaml`.
"""

from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path
from typing import Any, Dict

try:
    import yaml  # type: ignore
except Exception:
    yaml = None

try:
    import litellm
except Exception as exc:
    print(f"LiteLLM is required (pip install litellm). Import error: {exc}", file=sys.stderr)
    sys.exit(1)


REPO_ROOT = Path(__file__).resolve().parent
PROVIDERS_CONFIG = REPO_ROOT / "providers" / "providers-config.yaml"
PROMPTS_DIR = REPO_ROOT / "prompts"
APP_PROMPTS_DIR = Path.home() / ".lexi-cli" / "prompts"


def load_yaml(path: Path) -> Dict[str, Any]:
    if not path.exists():
        return {}
    if not yaml:
        raise RuntimeError("pyyaml is required to read provider config.")
    try:
        data = yaml.safe_load(path.read_text()) or {}
        return data if isinstance(data, dict) else {}
    except Exception as exc:
        raise RuntimeError(f"Failed to read {path}: {exc}") from exc


def load_providers_config() -> Dict[str, Any]:
    config = load_yaml(PROVIDERS_CONFIG)
    return config.get("providers", {}) if isinstance(config.get("providers", {}), dict) else {}


def resolve_prompt_path(prompt_arg: str) -> Path:
    candidate = Path(prompt_arg)
    if candidate.exists():
        return candidate
    direct_app = APP_PROMPTS_DIR / f"{prompt_arg}.json"
    if direct_app.exists():
        return direct_app
    direct_repo = PROMPTS_DIR / f"{prompt_arg}.json"
    if direct_repo.exists():
        return direct_repo
    example = PROMPTS_DIR / "examples" / f"{prompt_arg}.json"
    if example.exists():
        return example
    raise FileNotFoundError(f"Prompt not found: {prompt_arg}")


def load_prompt(path: Path) -> Dict[str, Any]:
    try:
        data = json.loads(path.read_text())
        if not isinstance(data, dict):
            raise ValueError("Prompt file must contain a JSON object.")
        return data
    except Exception as exc:
        raise RuntimeError(f"Failed to read prompt {path}: {exc}") from exc


def load_api_key(provider: str) -> str:
    key_path = REPO_ROOT / "providers" / provider / "api-key"
    if not key_path.exists():
        raise FileNotFoundError(f"API key file not found for provider {provider}: {key_path}")
    key = key_path.read_text().strip()
    if not key:
        raise RuntimeError(f"API key file is empty: {key_path}")
    return key


def build_messages(prompt_data: Dict[str, Any]) -> list[Dict[str, str]]:
    role = prompt_data.get("role", "user")
    content = prompt_data.get("prompt") or ""
    if not content:
        raise RuntimeError("Prompt content is empty.")
    return [{"role": role, "content": content}]


def send_request(provider: str, model: str, api_key: str, base_url: str, prompt_data: Dict[str, Any]) -> Any:
    messages = build_messages(prompt_data)
    temperature = prompt_data.get("temperature")
    max_tokens = prompt_data.get("max_tokens")
    kwargs = {
        "model": model,
        "messages": messages,
        "api_key": api_key,
    }
    if base_url:
        kwargs["api_base"] = base_url.rstrip("/")
    if temperature is not None:
        kwargs["temperature"] = temperature
    if max_tokens is not None:
        kwargs["max_tokens"] = max_tokens
    return litellm.completion(**kwargs)


def main() -> None:
    parser = argparse.ArgumentParser(description="Send a prompt to a provider using LiteLLM.")
    parser.add_argument("--prompt", required=True, help="Prompt name (or path) in lexi-cli JSON format.")
    parser.add_argument("--provider", required=True, help="Provider name (matches providers-config.yaml).")
    parser.add_argument("--model", help="Override model name (defaults to provider default_model).")
    args = parser.parse_args()

    providers_cfg = load_providers_config()
    provider_cfg = providers_cfg.get(args.provider)
    if not provider_cfg:
        raise SystemExit(f"Provider {args.provider!r} not found in providers-config.yaml")

    model = args.model or provider_cfg.get("default_model")
    if not model:
        raise SystemExit("Model not specified and no default_model found for provider.")
    base_url = provider_cfg.get("url", "")
    api_key = load_api_key(args.provider)

    prompt_path = resolve_prompt_path(args.prompt)
    prompt_data = load_prompt(prompt_path)

    response = send_request(args.provider, model, api_key, base_url, prompt_data)
    try:
        choice = response["choices"][0]["message"]["content"]  # type: ignore[index]
    except Exception:
        print(response)
        return
    print(choice)


if __name__ == "__main__":
    main()
