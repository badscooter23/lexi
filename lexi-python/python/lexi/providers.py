"""Providers in ``providers/providers-config.yaml`` and their API keys (``lexi providers``)."""

from __future__ import annotations

from typing import Any, Dict, Iterable, List, Optional

from ._admin import call


def list() -> List[Dict[str, Any]]:
    """Every provider: ``{name, url, api, default_model, hosted_models,
    hosted_model_fields, key_source}``; ``key_source`` is ``"file"``, ``"env"``,
    ``"config"`` or ``None`` (the key itself is never included)."""
    return call("providers.list")


def status(name: str) -> Dict[str, Any]:
    """Check the provider's API key against its models endpoint:
    ``{provider, status, source, from}``, ``status`` one of ``"ready"``,
    ``"no api-key"``, ``"invalid api-key"``; ``source`` is the kind of place
    the key came from (``env``/``file``/``config``) and ``from`` that place
    (``$OPENAI_API_KEY``, a path)."""
    return call("providers.status", name=name)


def add(
    name: str,
    url: str,
    default_model: str,
    *,
    api: str = "chat-completions",
    api_key: Optional[str] = None,
    hosted_models: str = "/models",
) -> Dict[str, Any]:
    """Add a provider speaking API definition ``api`` at ``url``. ``api_key``
    is saved in the macOS Keychain, else the provider's owner-only key file;
    never the config."""
    return call(
        "providers.add",
        name=name,
        url=url,
        default_model=default_model,
        api=api,
        api_key=api_key,
        hosted_models=hosted_models,
    )


def rename(old: str, new: str) -> Dict[str, Any]:
    """Rename a provider (its models, aliases and key file move with it)."""
    return call("providers.rename", old=old, new=new)


def remove(name: str) -> Dict[str, Any]:
    """Remove a provider and its per-provider files."""
    return call("providers.remove", name=name)


def set_key(name: str, key: str, *, api: Optional[str] = None) -> Dict[str, Any]:
    """Save an API key as the provider's saved key (``api``: a key for that
    API only) — in the macOS Keychain, else an owner-only file,
    ``providers/<name>/api-key`` (``api-key-<api>``); ``key_store`` in the
    config picks — looked in after the sources ``api_key`` lists, which are
    never written. Returns ``{provider, location, store, added, used}``:
    ``location`` is where it's kept (``"the macOS Keychain"`` or the file),
    ``store`` ``"keychain"`` or ``"file"``, ``added`` whether providers-config.yaml changed
    (an API got its own ``api_key``, or a key written in it was taken out),
    ``used`` where the key is now read from (another source when an earlier
    one has a key)."""
    return call("providers.set_key", name=name, key=key, api=api)


def show_key(name: str, *, reveal: bool = False, api: Optional[str] = None) -> Dict[str, Any]:
    """Where the provider's (or ``api``'s) key comes from: ``{provider, api,
    own, source, from, key, error, sources, save_to, store, env}``.
    ``sources`` is where it's looked for, in order (the saved key among them),
    each ``{kind, from, state, note?}``; ``source``/``from``/``key`` describe
    the key that's used (masked unless ``reveal``; None when there's none).
    Commands run only with ``reveal``. ``save_to`` is where :func:`set_key`
    saves the key; ``store`` is ``{kind, available, locked}`` (``kind``
    ``"keychain"`` or ``"file"``)."""
    return call("providers.show_key", name=name, reveal=reveal, api=api)


def remove_key(name: str, *, api: Optional[str] = None) -> Dict[str, Any]:
    """Delete the saved key, from the keychain and any key file (the sources
    ``api_key`` lists, and the config, are left as they are):
    ``{"removed": bool}``."""
    return call("providers.remove_key", name=name, api=api)


def _source(
    env: Optional[str],
    file: Optional[str],
    command: Optional[Any],
    saved: bool,
) -> Any:
    given = [x for x in (env, file, command) if x is not None] + ([True] if saved else [])
    if len(given) != 1:
        raise ValueError("give exactly one of env, file, command or saved")
    if env is not None:
        return {"env": env}
    if file is not None:
        return {"file": file}
    if command is not None:
        if isinstance(command, str):
            import shlex

            command = shlex.split(command)
        return {"command": [str(c) for c in command]}
    return "saved"


def key_sources(name: str, *, api: Optional[str] = None) -> Dict[str, Any]:
    """Where the provider's (or ``api``'s own) key is looked for, in order:
    ``{provider, api, own, sources: [{kind, from, value, implicit?}]}``.
    ``value`` is the ``api_key`` entry as written; the saved key is
    ``implicit`` when the list doesn't place it (it's then last)."""
    return call("providers.key_sources", name=name, api=api)


def add_key_source(
    name: str,
    *,
    env: Optional[str] = None,
    file: Optional[str] = None,
    command: Optional[Any] = None,
    saved: bool = False,
    first: bool = False,
    api: Optional[str] = None,
) -> Dict[str, Any]:
    """Add a place to look for the key (exactly one of ``env``, ``file``,
    ``command`` — a list of program and arguments, or one string split like a
    shell would — or ``saved``, the saved key's place): at the end of
    ``api_key``, or first with ``first``. Lexi never writes to it. Returns the
    sources afterwards (:func:`key_sources`)."""
    return call(
        "providers.add_key_source",
        name=name,
        api=api,
        first=first,
        source=_source(env, file, command, saved),
    )


def remove_key_source(
    name: str,
    *,
    env: Optional[str] = None,
    file: Optional[str] = None,
    command: Optional[Any] = None,
    saved: bool = False,
    api: Optional[str] = None,
) -> Dict[str, Any]:
    """Stop looking in a place for the key (``saved`` takes out only its
    place: the saved key is looked in last again): ``{provider, removed}``."""
    return call(
        "providers.remove_key_source",
        name=name,
        api=api,
        source=_source(env, file, command, saved),
    )


def upgrade(name: str, *, template: Optional[str] = None, apply: bool = False) -> Dict[str, Any]:
    """Convert a provider to its shipped template (``lexi providers upgrade``),
    keeping only the values you changed. Without ``apply``, reports what would
    change: ``{provider, template, changed, before, after, dropped, kept}``;
    with it, also backs up and rewrites the config (``backup``)."""
    return call("providers.upgrade", name=name, template=template, apply=apply)


def apis(name: str) -> List[Dict[str, Any]]:
    """The APIs a provider offers: ``[{api, for?, default_model?, url?,
    enabled, source}]`` (``source``: ``template``, ``user`` or ``config``)."""
    return call("providers.apis", name=name)


def add_api(
    name: str,
    api: str,
    *,
    for_tags: Iterable[str] = (),
    default_model: Optional[str] = None,
    url: Optional[str] = None,
) -> Dict[str, Any]:
    """Add (or update) an API on a provider: models tagged with all of
    ``for_tags`` use it (none = the catch-all)."""
    tags = [*for_tags]
    return call("providers.add_api", name=name, api=api, default_model=default_model, url=url,
                **({"for": tags} if tags else {}))


def remove_api(name: str, api: str) -> Dict[str, Any]:
    """Remove an API from a provider (a template's API is turned off)."""
    return call("providers.remove_api", name=name, api=api)
