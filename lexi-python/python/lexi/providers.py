"""Providers in ``providers/providers-config.yaml`` and their API keys (``lexi providers``).

A provider's keys each have a name (``openai#work``; ``openai#`` is a key saved
before keys had names). The functions that change a key need its name
(``key_name=`` or ``"openai#work"``); the ones that only look cover every key
when given none. A request uses the key it names (``key="work"``), else the
provider's variable (``$OPENAI_API_KEY``) when it's set, else the default key.
"""

from __future__ import annotations

from typing import Any, Dict, Iterable, List, Optional

from ._admin import call


def list() -> List[Dict[str, Any]]:
    """Every provider: ``{name, url, api, default_model, hosted_models,
    hosted_model_fields, key_source}``; ``key_source`` is where the key a
    request would use comes from (``"saved"``, ``"env"``, ``"file"``,
    ``"command"``, ``"config"``) or ``None`` (the key itself is never
    included)."""
    return call("providers.list")


def status(name: str, *, key: Optional[str] = None) -> Dict[str, Any]:
    """Check a provider's API keys against its models endpoint (nothing is
    generated or billed). With ``key`` (or ``"openai#work"``), that key:
    ``{provider, key, status, source, from, verified, reason}``, ``status``
    one of ``"ready"``, ``"no api-key"``, ``"invalid api-key"``. Without,
    every key and the provider's variable: ``{provider, keys: [...],
    variable, default_key, uses, ready, reason}`` — ``uses`` is what a
    request that names no key would use (``{"variable": NAME}`` or ``{"key":
    NAME}``)."""
    return call("providers.status", name=name, key=key)


def add(
    name: str,
    url: str,
    default_model: str,
    *,
    api: str = "chat-completions",
    api_key: Optional[str] = None,
    key_name: Optional[str] = None,
    hosted_models: str = "/models",
) -> Dict[str, Any]:
    """Add a provider speaking API definition ``api`` at ``url``. ``api_key``
    is saved as the key ``key_name`` (required with it), in the OS key store,
    else an owner-only key file; never the config."""
    return call(
        "providers.add",
        name=name,
        url=url,
        default_model=default_model,
        api=api,
        api_key=api_key,
        key_name=key_name,
        hosted_models=hosted_models,
    )


def rename(old: str, new: str) -> Dict[str, Any]:
    """Rename a provider (its models, aliases and keys move with it)."""
    return call("providers.rename", old=old, new=new)


def remove(name: str) -> Dict[str, Any]:
    """Remove a provider and its per-provider files."""
    return call("providers.remove", name=name)


def set_key(name: str, key: str, *, key_name: Optional[str] = None) -> Dict[str, Any]:
    """Save ``key`` as the value of the key ``key_name`` (or ``name`` written
    ``"openai#work"``; ``"openai#"`` a key saved before keys had names) —
    adding the key if it's new — in the OS key store (macOS Keychain,
    Windows Credential Manager, the Linux desktop keyring), else an
    owner-only file; ``key_store`` in the config picks. It's looked in after
    the sources the key's ``api_key`` lists, which are never written. The
    key must be named (``ValueError`` otherwise). Returns ``{provider,
    key_name, location, store, added, used}``: ``location`` is where it's
    kept (``"the macOS Keychain"``, ``"Windows Credential Manager"``,
    ``"your desktop keyring"`` or the file), ``store`` ``"keychain"``,
    ``"credential-manager"``, ``"secret-service"`` or ``"file"``, ``added``
    whether providers-config.yaml changed (the key was added, or a key
    written in it was taken out), ``used`` where the key is now read from
    (another source when an earlier one has a key)."""
    return call("providers.set_key", name=name, key=key, key_name=key_name)


def show_key(name: str, *, reveal: bool = False, key_name: Optional[str] = None) -> Dict[str, Any]:
    """Where a key comes from. For one key (``key_name``, or
    ``"openai#work"``): ``{provider, key_name, label, source, from, key,
    error, sources, save_to, store, env}`` — ``sources`` is where it's looked
    for, in order (the saved key among them), each ``{kind, from, state,
    note?}``; ``source``/``from``/``key`` describe the key that's used
    (masked unless ``reveal``; None when there's none). Commands run only
    with ``reveal``. ``save_to`` is where :func:`set_key` saves it; ``store``
    is ``{kind, available, locked}``. Without a key: every key (``keys``),
    as :func:`keys` reports them, each with the above."""
    return call("providers.show_key", name=name, reveal=reveal, key_name=key_name)


def remove_key(name: str, *, key_name: Optional[str] = None) -> Dict[str, Any]:
    """Delete a key's saved value, from the key store and any key file (the
    key, its sources and the config stay; :func:`delete_key` removes the key
    itself). The key must be named. ``{"removed": bool}``."""
    return call("providers.remove_key", name=name, key_name=key_name)


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


def key_sources(name: str, *, key_name: Optional[str] = None) -> Dict[str, Any]:
    """Where a key is looked for, in order: ``{provider, key_name, sources:
    [{kind, from, value, implicit?}]}`` — ``value`` is the ``api_key`` entry
    as written; the saved key is ``implicit`` when the list doesn't place it
    (it's then last). Without a key: ``{provider, variable, keys: [{key_name,
    default, sources}]}``, every key's."""
    return call("providers.key_sources", name=name, key_name=key_name)


def add_key_source(
    name: str,
    *,
    env: Optional[str] = None,
    file: Optional[str] = None,
    command: Optional[Any] = None,
    saved: bool = False,
    first: bool = False,
    key_name: Optional[str] = None,
) -> Dict[str, Any]:
    """Add a place to look for a key (exactly one of ``env``, ``file``,
    ``command`` — a list of program and arguments, or one string split like a
    shell would — or ``saved``, the saved key's place): at the end of its
    ``api_key``, or first with ``first``. Lexi never writes to it. The key
    must be named. Returns its sources afterwards (:func:`key_sources`)."""
    return call(
        "providers.add_key_source",
        name=name,
        key_name=key_name,
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
    key_name: Optional[str] = None,
) -> Dict[str, Any]:
    """Stop looking in a place for a key (``saved`` takes out only its place:
    the saved key is looked in last again). The key must be named.
    ``{provider, removed}``."""
    return call(
        "providers.remove_key_source",
        name=name,
        key_name=key_name,
        source=_source(env, file, command, saved),
    )


def keys(name: str) -> Dict[str, Any]:
    """The provider's keys (``lexi providers keys list``): ``{provider,
    default_key, keys: [{name, label, default, source, from, error?}],
    variable: {name, set}, ready, uses, reason}`` — a key saved before keys
    had names first (``name`` ``""``, ``label`` ``"#"``), then the named
    ones, with where each comes from; ``default`` marks the one requests use
    when they name none (``default_key``, None when there's none);
    ``variable`` is the provider's variable, which a request that names no
    key uses when it's set; ``uses`` what such a request would use, and
    ``reason`` why not (``"no key"``, ``"no default key"``, ``"no key
    found"``). Never the keys themselves."""
    return call("providers.keys", name=name)


def add_key(name: str, key: str) -> Dict[str, Any]:
    """Add the key ``key`` without a value (``lexi providers keys add``): its
    saved key alone until you save it (:func:`set_key` with ``key_name``) or
    give it sources (:func:`add_key_source`). ``{provider, key_name,
    added}``."""
    return call("providers.add_key", name=name, key=key)


def rename_key(name: str, old: str, new: str) -> Dict[str, Any]:
    """Rename a key (``old`` ``""``: a key saved before keys had names, which
    gets a name); its sources and saved key go along, and it stays the
    default if it was."""
    return call("providers.rename_key", name=name, old=old, new=new)


def delete_key(name: str, key: str) -> Dict[str, Any]:
    """Remove the key ``key`` and its saved key (``lexi providers keys rm``;
    ``""`` a key saved before keys had names): ``{provider, removed}``. Any
    key can go, the default and the only one included."""
    return call("providers.delete_key", name=name, key=key)


def set_default_key(name: str, key: str) -> Dict[str, Any]:
    """Make ``key`` the one the provider's requests use when they name none
    (``lexi providers keys default``)."""
    return call("providers.set_default_key", name=name, key=key)


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
