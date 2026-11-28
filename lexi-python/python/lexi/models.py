"""A provider's configured models, default model and aliases (``lexi models``)."""

from __future__ import annotations

from typing import Any, Dict, Iterable, List, Optional

from ._admin import call


def list(provider: str) -> List[Dict[str, Any]]:
    """Configured models: ``[{id, name, description, default, aliases, metadata,
    kind, api, tags}]`` — ``api`` is the API that serves the model, ``tags`` its
    effective tags."""
    return call("models.list", provider=provider)


def hosted(
    provider: str,
    *,
    tags: Iterable[str] = (),
    cached: bool = False,
    key: Optional[str] = None,
) -> List[Dict[str, Any]]:
    """The models the provider's API lists, each with Lexi's ``kind``, ``api``
    and ``tags`` added. A network call that also refreshes the local catalog
    cache (which classification reads); ``cached=True`` reads the cache
    instead. ``tags=["out:image"]`` keeps models having all of them. ``key``
    (or ``"openai#work"``) lists what that named key's account sees."""
    return call("models.hosted", provider=provider, tags=[*tags], cached=cached, key=key)


def hosted_schema(provider: str) -> Dict[str, Any]:
    """How :func:`hosted` gets the provider's listing, without asking it
    (``lexi models hosted PROVIDER --schema``): ``{provider, url, catalog,
    source, template}`` — ``url`` the models endpoint (``GET``), ``catalog``
    the catalog settings that apply (``path``, ``paginate``, ``items_path``,
    ``id_path``, ``fields``, ``tags``, ``limits``, …; ``{}`` when there are
    none), ``source`` where they come from: ``"config"``
    (providers-config.yaml), ``"template"`` (the shipped template named by
    ``template``) or ``"default"``."""
    return call("models.hosted_schema", provider=provider)


def default(provider: str, kind: Optional[str] = None) -> Dict[str, Any]:
    """The provider's default model for each kind: ``{provider, default_model,
    defaults: [{kind, model, source}]}`` (``default_model`` is the chat
    default; ``source`` is ``"user"`` for yours, ``"binding"`` or
    ``"provider"`` for the provider's). With ``kind``
    (``"image-generation"``, …), just that one: ``{provider, kind, model,
    source}``."""
    return call("models.default", provider=provider, kind=kind)


def set_default(provider: str, model: Optional[str], kind: Optional[str] = None) -> Dict[str, Any]:
    """Make ``model`` your default for its kind (from its tags):
    ``{provider, kind, model}``. With ``kind``, the model must be of that kind.
    ``None`` clears your default for ``kind`` (chat when not given), as
    :func:`clear_default`."""
    return call("models.set_default", provider=provider, model=model, kind=kind)


def clear_default(provider: str, kind: str) -> Dict[str, Any]:
    """Clear your default ``kind`` model, so the provider's applies again:
    ``{provider, kind, cleared, model, source}`` (the default now in effect)."""
    return call("models.clear_default", provider=provider, kind=kind)


def add(
    provider: str,
    model: str,
    *,
    name: Optional[str] = None,
    description: Optional[str] = None,
    max_tokens: Optional[int] = None,
    context_window: Optional[int] = None,
    default: bool = False,
    **metadata: Any,
) -> Dict[str, Any]:
    """Add a model, or update the fields given on an existing one:
    ``{provider, model, existed}``. Extra keyword arguments are stored as
    metadata too; ``default=True`` makes it the provider's default."""
    fields = dict(metadata)
    for k, v in (("name", name), ("description", description), ("max_tokens", max_tokens),
                 ("context_window", context_window)):
        if v is not None:
            fields[k] = v
    return call("models.add", provider=provider, model=model, fields=fields, default=default)


set = add  # ``lexi models set`` — the same upsert


def remove(provider: str, model: str) -> Dict[str, Any]:
    """Remove a model (the default falls to the first remaining model)."""
    return call("models.remove", provider=provider, model=model)


def aliases(provider: str) -> List[Dict[str, Any]]:
    """``[{alias, model}]``, sorted by alias."""
    return call("models.aliases", provider=provider)


def set_alias(provider: str, alias: str, model: str) -> Dict[str, Any]:
    """Point ``alias`` at ``model``."""
    return call("models.set_alias", provider=provider, alias=alias, model=model)


def remove_alias(provider: str, alias: str) -> Dict[str, Any]:
    """Remove an alias."""
    return call("models.remove_alias", provider=provider, alias=alias)


def tag(
    provider: str,
    model: str,
    add: Iterable[str] = (),
    remove: Iterable[str] = (),
    *,
    clear: bool = False,
) -> Dict[str, Any]:
    """Add or remove your tags on a model id or glob (``"meta/*"``):
    ``tag("nvidia", "some/model", add=["kind:image-generation"], remove=["in:image"])``.
    ``clear=True`` removes all of your tags on it. Tags must be in the
    vocabulary (:func:`lexi.tags`); ``label:<name>`` is free-form."""
    if clear:
        return call("models.tag", provider=provider, model=model, clear=True)
    ops = ["+" + t for t in add] + ["-" + t for t in remove]
    return call("models.tag", provider=provider, model=model, ops=ops)


def tags(provider: str, model: str) -> Dict[str, Any]:
    """A model's classification: ``{provider, model, kind, api, binding,
    tags: [{tag, source}], error?}`` — ``error`` when no API serves it."""
    return call("models.tags", provider=provider, model=model)


def resolve(
    provider: str, model: Optional[str] = None, *, output: Optional[str] = None
) -> Dict[str, Any]:
    """Which model and API a request would use: ``{provider, model, api, kind,
    tags}``. With no ``model``, the default for ``output`` (``"text"`` when
    omitted; ``"image"``, ``"vector"``, …). With a model and an ``output`` it
    can't produce, raises :class:`lexi.LexiError`; with a model alone, its API
    decides."""
    return call("models.resolve", provider=provider, model=model, output=output)
