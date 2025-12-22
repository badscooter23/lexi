#!/usr/bin/env python3
"""
List models by calling each provider's REST API using config values.

Reads `providers/providers-config.yaml` (or a custom path) to obtain `url`
and `api_key` for each provider, expands environment variables, and then
queries the provider's models endpoint to display the available model IDs.
Optionally, `--details` queries each model's detail endpoint (when supported)
to show any extra fields returned (e.g., owned_by, context length).
"""

from __future__ import annotations

import argparse
import logging
import os
from pathlib import Path
from typing import Any, Dict, Iterable, List

try:
    import requests
except Exception as exc:  # pragma: no cover - requests not installed
        requests = None  # type: ignore
        REQUESTS_IMPORT_ERROR = exc
else:
    REQUESTS_IMPORT_ERROR = None

DETAIL_KEYS = ("description", "max_tokens", "context_length", "context_window", "owned_by")

try:
    import yaml  # type: ignore
except Exception:  # pragma: no cover - yaml not available everywhere
    yaml = None


def setup_logging(verbose: bool) -> None:
    level = logging.DEBUG if verbose else logging.INFO
    logging.basicConfig(level=level, format="%(asctime)s - %(levelname)s - %(message)s")


def load_yaml_file(path: Path) -> Dict[str, Any]:
    if not path.exists():
        logging.error("Config file not found: %s", path)
        return {}
    try:
        if yaml:
            data = yaml.safe_load(path.read_text()) or {}
        else:  # pragma: no cover - fallback if yaml unavailable
            import json

            data = json.loads(path.read_text())
        return data if isinstance(data, dict) else {}
    except Exception as exc:
        logging.error("Failed to read %s: %s", path, exc)
        return {}


def expand_env_vars(value: Any) -> Any:
    """Recursively expand ${VAR} placeholders using the current environment."""
    if isinstance(value, dict):
        return {k: expand_env_vars(v) for k, v in value.items()}
    if isinstance(value, list):
        return [expand_env_vars(item) for item in value]
    if isinstance(value, str):
        return os.path.expandvars(value)
    return value


def load_providers_config(path: Path) -> Dict[str, Any]:
    raw = load_yaml_file(path)
    return expand_env_vars(raw)


def iter_providers(selected: str, providers: Dict[str, Any]) -> Iterable[str]:
    if selected == "all":
        return providers.keys()
    if selected not in providers:
        logging.error("Provider '%s' not found in configuration", selected)
        return []
    return [selected]


def normalized_base_url(url: str) -> str:
    """Normalize config URL to the base (e.g., trim /messages)."""
    if not url:
        return ""
    clean = url.rstrip("/")
    if "/v1/" in clean:
        prefix, _ = clean.split("/v1/", 1)
        return f"{prefix}/v1"
    if clean.endswith("/v1"):
        return clean
    return clean


def fetch_openai_models(base_url: str, api_key: str, *, details: bool) -> List[Dict[str, Any]]:
    endpoint = f"{normalized_base_url(base_url) or 'https://api.openai.com/v1'}/models"
    headers = {"Authorization": f"Bearer {api_key}", "Content-Type": "application/json"}
    try:
        resp = requests.get(endpoint, headers=headers, timeout=10)
        resp.raise_for_status()
        data = resp.json()
        ids = [item["id"] for item in data.get("data", []) if isinstance(item, dict) and "id" in item]
    except Exception as exc:  # pragma: no cover - network dependent
        logging.error("Failed to fetch OpenAI models: %s", exc)
        ids = []

    if not details:
        return [{"id": mid} for mid in ids]

    detailed: List[Dict[str, Any]] = []
    for mid in ids:
        detail_endpoint = f"{normalized_base_url(base_url) or 'https://api.openai.com/v1'}/models/{mid}"
        try:
            resp = requests.get(detail_endpoint, headers=headers, timeout=10)
            resp.raise_for_status()
            detail_json = resp.json()
        except Exception as exc:  # pragma: no cover - network dependent
            logging.error("Failed to fetch OpenAI model details for %s: %s", mid, exc)
            detail_json = {}
        detailed.append({"id": mid, "detail": detail_json})
    return detailed


def fetch_anthropic_models(base_url: str, api_key: str, *, details: bool) -> List[Dict[str, Any]]:
    endpoint = f"{normalized_base_url(base_url) or 'https://api.anthropic.com/v1'}/models"
    headers = {
        "x-api-key": api_key,
        "anthropic-version": "2023-06-01",
        "Content-Type": "application/json",
    }
    try:
        resp = requests.get(endpoint, headers=headers, timeout=10)
        resp.raise_for_status()
        data = resp.json()
        ids = [item["id"] for item in data.get("data", []) if isinstance(item, dict) and "id" in item]
    except Exception as exc:  # pragma: no cover - network dependent
        logging.error("Failed to fetch Anthropic models (falling back to known list): %s", exc)
        ids = [
            "claude-3-5-sonnet-20241022",
            "claude-3-5-haiku-20241022",
            "claude-3-opus-20240229",
            "claude-3-sonnet-20240229",
            "claude-3-haiku-20240307",
        ]

    if not details:
        return [{"id": mid} for mid in ids]

    detailed: List[Dict[str, Any]] = []
    for mid in ids:
        detail_endpoint = f"{normalized_base_url(base_url) or 'https://api.anthropic.com/v1'}/models/{mid}"
        try:
            resp = requests.get(detail_endpoint, headers=headers, timeout=10)
            resp.raise_for_status()
            detail_json = resp.json()
        except Exception as exc:  # pragma: no cover - network dependent
            logging.error("Failed to fetch Anthropic model details for %s: %s", mid, exc)
            detail_json = {}
        detailed.append({"id": mid, "detail": detail_json})
    return detailed

def fetch_nvidia_models(base_url: str, api_key: str, *, details: bool) -> List[Dict[str, Any]]:
    endpoint = f"{normalized_base_url(base_url) or 'https://integrate.api.nvidia.com/v1'}/models"
    headers = {"Authorization": f"Bearer {api_key}", "Content-Type": "application/json"}
    try:
        resp = requests.get(endpoint, headers=headers, timeout=10)
        resp.raise_for_status()
        data = resp.json()
        ids = [item["id"] for item in data.get("data", []) if isinstance(item, dict) and "id" in item]
    except Exception as exc:  # pragma: no cover - network dependent
        logging.error("Failed to fetch NVIDIA models: %s", exc)
        ids = []

    if not details:
        return [{"id": mid} for mid in ids]

    detailed: List[Dict[str, Any]] = []
    for mid in ids:
        detail_endpoint = f"{normalized_base_url(base_url) or 'https://integrate.api.nvidia.com/v1'}/models/{mid}"
        try:
            resp = requests.get(detail_endpoint, headers=headers, timeout=10)
            resp.raise_for_status()
            detail_json = resp.json()
        except Exception as exc:  # pragma: no cover - network dependent
            logging.error("Failed to fetch NVIDIA model details for %s: %s", mid, exc)
            detail_json = {}
        detailed.append({"id": mid, "detail": detail_json})
    return detailed


def fetch_models(provider: str, cfg: Dict[str, Any], *, details: bool) -> List[Dict[str, Any]]:
    api_key = cfg.get("api_key")
    base_url = cfg.get("url", "")
    if not api_key:
        logging.error("Provider '%s' missing api_key in config.", provider)
        return []
    if requests is None:
        logging.error("The 'requests' library is required (import error: %s)", REQUESTS_IMPORT_ERROR)
        return []

    if provider == "openai":
        return fetch_openai_models(base_url, api_key, details=details)
    if provider == "anthropic":
        return fetch_anthropic_models(base_url, api_key, details=details)
    if provider == "nvidia":
        return fetch_nvidia_models(base_url, api_key, details=details)

    logging.error("Unsupported provider: %s", provider)
    return []


def extract_display_fields(detail: Dict[str, Any]) -> Dict[str, Any]:
    return {k: detail[k] for k in DETAIL_KEYS if k in detail}


def print_models(provider: str, provider_cfg: Dict[str, Any], models: List[Dict[str, Any]], *, show_details: bool) -> None:
    default_model = provider_cfg.get("default_model")
    print(f"\n=== {provider.upper()} Models ===")
    if default_model:
        print(f"Default model: {default_model}")

    if not models:
        print("  No models returned.")
        return

    print("\nAvailable models:")
    for entry in sorted(models, key=lambda m: m.get("id", "")):
        model_id = entry.get("id", "")
        marker = " (default)" if model_id == default_model else ""
        print(f"  {model_id}{marker}")
        if show_details:
            detail = extract_display_fields(entry.get("detail", {}) if isinstance(entry.get("detail", {}), dict) else {})
            if detail:
                for key, val in detail.items():
                    print(f"    {key}: {val}")
            else:
                print("    (no additional details returned)")
    print(f"\nTotal: {len(models)} models")


def main() -> None:
    parser = argparse.ArgumentParser(description="List models by querying provider APIs using local config.")
    parser.add_argument(
        "--provider",
        required=True,
        help="Provider to list (e.g., openai, anthropic, nvidia, or all).",
    )
    parser.add_argument(
        "--providers-config",
        default="providers/providers-config.yaml",
        help="Path to providers-config.yaml",
    )
    parser.add_argument(
        "--details",
        action="store_true",
        help="Fetch and show per-model details from the provider (if the endpoint supports it).",
    )
    parser.add_argument("--verbose", "-v", action="store_true", help="Enable verbose logging.")

    args = parser.parse_args()
    setup_logging(args.verbose)

    config_path = Path(args.providers_config).expanduser().resolve()
    providers_config = load_providers_config(config_path)
    providers = providers_config.get("providers", {}) if isinstance(providers_config.get("providers", {}), dict) else {}
    if not providers:
        logging.error("No providers found in %s", config_path)
        return

    for provider in iter_providers(args.provider.lower(), providers):
        provider_cfg = providers.get(provider, {})
        models = fetch_models(provider, provider_cfg, details=args.details)
        print_models(provider, provider_cfg, models, show_details=args.details)


if __name__ == "__main__":
    main()
