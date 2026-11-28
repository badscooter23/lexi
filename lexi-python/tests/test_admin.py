"""The admin namespaces — lexi.providers / models / prompts / config / apis —
over a temp ``$LEXI_HOME`` seeded with the shipped defaults."""

import sys

import pytest

import lexi
from conftest import Mock, ok


def test_providers_lifecycle(home):
    names = [p["name"] for p in lexi.providers.list()]
    assert {"openai", "anthropic", "nvidia"} <= set(names)
    row = next(p for p in lexi.providers.list() if p["name"] == "openai")
    assert set(row) == {
        "name", "template", "url", "api", "apis", "default_model", "hosted_models",
        "hosted_model_fields", "key_source",
    }
    assert row["template"] == "openai" and row["apis"] == ["responses", "image-generations", "embeddings"]

    # A key needs a name.
    with pytest.raises(lexi.LexiError, match="key_name"):
        lexi.providers.add("nameless", "https://x.example/v1", "m1", api_key="sk-x")
    assert lexi.providers.add(
        "mine", "https://x.example/v1", "m1", api_key="sk-abcdefghij", key_name="main"
    ) == {"added": "mine"}
    mine = next(p for p in lexi.providers.list() if p["name"] == "mine")
    assert mine["api"] == "chat-completions" and mine["key_source"] == "saved"
    assert lexi.models.default("mine")["default_model"] == "m1"
    assert (home / "providers" / "mine" / "keys" / "main").read_text().strip() == "sk-abcdefghij"

    assert lexi.providers.show_key("mine#main")["key"] == "sk-a*****ghij"
    assert lexi.providers.show_key("mine", key_name="main", reveal=True)["key"] == "sk-abcdefghij"
    # Named none: every key.
    every = lexi.providers.show_key("mine")
    assert [k["label"] for k in every["keys"]] == ["#main"]
    assert every["default_key"] == "main"
    assert lexi.providers.rename("mine", "yours") == {"renamed": "mine", "to": "yours"}
    with pytest.raises(lexi.LexiError, match="name the key"):
        lexi.providers.remove_key("yours")
    assert lexi.providers.remove_key("yours", key_name="main") == {"removed": True}
    # The saved key followed the rename.
    assert lexi.providers.show_key("yours#main") == {
        "provider": "yours", "key_name": "main", "label": "#main",
        "source": None, "from": None, "key": None, "error": None,
        "sources": [{
            "kind": "saved", "from": "providers/yours/keys/main",
            "state": "missing", "note": "none saved",
        }],
        "save_to": "providers/yours/keys/main",
        "store": {"kind": "file", "available": True, "locked": False},
        "env": None,
    }
    with pytest.raises(lexi.LexiError, match="name the key"):
        lexi.providers.set_key("yours", "k2")
    saved = lexi.providers.set_key("yours#main", "k2")
    assert saved == {
        "provider": "yours", "location": "providers/yours/keys/main", "store": "file",
        "added": False, "used": "providers/yours/keys/main", "key_name": "main",
    }
    assert lexi.providers.show_key("yours#main", reveal=True)["key"] == "k2"
    assert lexi.providers.remove("yours") == {"removed": "yours"}
    assert "yours" not in [p["name"] for p in lexi.providers.list()]

    with pytest.raises(lexi.LexiError, match="unknown provider"):
        lexi.providers.show_key("nope")
    with pytest.raises(lexi.LexiError):
        lexi.providers.add("bad", "https://x", "m", api="no-such-api")


def test_provider_status_and_hosted_models(home):
    m = Mock()
    try:
        m.routes[("GET", "/models")] = ok({"data": [{"id": "m-a"}, {"id": "m-b"}]})
        lexi.providers.add("mock", m.url, "m-a", api_key="k", key_name="main")
        assert lexi.providers.status("mock#main") == {
            "provider": "mock", "key": "mock#main", "status": "ready", "source": "saved",
            "from": "providers/mock/keys/main", "verified": True, "reason": None,
        }
        every = lexi.providers.status("mock")
        assert [(k["key"], k["status"]) for k in every["keys"]] == [("mock#main", "ready")]
        assert every["uses"] == {"key": "main"} and every["ready"] is True
        assert [x["id"] for x in lexi.models.hosted("mock")] == ["m-a", "m-b"]
        lexi.providers.remove_key("mock#main")
        assert lexi.providers.status("mock#main")["status"] == "no api-key"
        lexi.providers.delete_key("mock", "main")
        assert lexi.providers.status("mock")["reason"] == "no key"
    finally:
        m.server.shutdown()


def test_models_and_aliases(home):
    lexi.providers.add("p", "https://x.example", "base")
    assert lexi.models.add("p", "big", name="Big", max_tokens=8000, tier="pro") == {
        "provider": "p", "model": "big", "existed": False,
    }
    assert lexi.models.set("p", "big", description="the big one")["existed"] is True
    lexi.models.add("p", "small", default=True)
    models = {m["id"]: m for m in lexi.models.list("p")}
    assert models["big"]["name"] == "Big"
    assert models["big"]["description"] == "the big one"
    assert models["big"]["metadata"]["tier"] == "pro"
    assert models["small"]["default"] is True
    assert lexi.models.default("p")["default_model"] == "small"
    assert lexi.models.set_default("p", "big") == {"provider": "p", "kind": "chat", "model": "big"}
    assert lexi.models.default("p")["default_model"] == "big"
    assert lexi.models.default("p", kind="chat") == {
        "provider": "p", "kind": "chat", "model": "big", "source": "user",
    }

    lexi.models.set_alias("p", "z", "small")
    lexi.models.set_alias("p", "a", "big")
    assert lexi.models.aliases("p") == [{"alias": "a", "model": "big"}, {"alias": "z", "model": "small"}]
    assert lexi.models.remove_alias("p", "a") == {"provider": "p", "removed": "a"}
    assert lexi.models.remove("p", "small") == {"provider": "p", "removed": "small"}
    assert [m["id"] for m in lexi.models.list("p")] == ["big"]

    with pytest.raises(lexi.LexiError, match="unknown provider"):
        lexi.models.list("nope")


def test_prompts(home):
    saved = lexi.prompts.save("review", "Review this", temperature=0.1)
    assert saved == {
        "name": "review", "active": False, "prompt": "Review this",
        "max_tokens": 256, "temperature": 0.1, "role": "user",
    }
    # Updating keeps the fields not given.
    assert lexi.prompts.save("review", max_tokens=99)["temperature"] == 0.1
    assert lexi.prompts.get("review")["max_tokens"] == 99

    assert lexi.prompts.activate("review") == {"active": "review"}
    first = lexi.prompts.list()[0]
    assert first["name"] == lexi.prompts.ACTIVE and first["active"] is True
    assert first["prompt"] == "Review this"

    assert lexi.prompts.remove("review") == {"removed": "review"}
    with pytest.raises(lexi.LexiError, match='prompt "review" not found'):
        lexi.prompts.get("review")
    with pytest.raises(lexi.LexiError, match="prompt text"):
        lexi.prompts.save("empty")


def test_config(home):
    settings = lexi.config.list()
    assert settings["cli_name"] == "lexi"  # a default
    assert lexi.config.set("edit_mode", "emacs") == {"key": "edit_mode", "value": "emacs"}
    lexi.config.set("count", 3)
    assert lexi.config.get("edit_mode") == "emacs"
    assert lexi.config.get("count") == 3
    assert lexi.config.remove("count") == {"removed": "count"}
    with pytest.raises(lexi.LexiError, match="not found"):
        lexi.config.get("count")


def test_api_definitions(home, tmp_path):
    defs = {d["name"]: d for d in lexi.apis.list()}
    assert defs["messages"]["builtin"] is True
    assert lexi.apis.show("messages")["request"]["path"] == "/messages"
    assert lexi.apis.validate("messages") == {"name": "messages", "valid": True, "errors": []}

    assert lexi.apis.create("mine", template="messages", path="/v2/messages") == {"created": "mine"}
    assert lexi.apis.show("mine")["request"]["path"] == "/v2/messages"
    with pytest.raises(lexi.LexiError, match="already exists"):
        lexi.apis.create("mine")
    lexi.apis.edit("mine", text_path="content[0].text", metadata={"stop": "stop_reason"})
    edited = lexi.apis.show("mine")
    assert edited["response"]["text_extract"]["text_path"] == "content[0].text"
    assert edited["response"]["metadata"]["stop"] == "stop_reason"

    preview = lexi.apis.preview("mine", prompt="hi", model="mx")
    assert preview["valid"] is True and preview["body"]["model"] == "mx"

    bad = tmp_path / "bad.yaml"
    bad.write_text("name: bad\nrequest: {}\n")
    saved = lexi.apis.save("bad", bad)
    assert saved["saved"] == "bad" and saved["valid"] is False and saved["errors"]
    with pytest.raises(lexi.LexiError, match="shipped"):
        lexi.apis.save("messages", "name: messages\n")

    assert lexi.apis.remove("mine") == {"removed": "mine"}
    assert "mine" not in [d["name"] for d in lexi.apis.list()]


def test_model_tags_and_resolution(home):
    # The shipped OpenAI template routes image models to the Images API.
    img = lexi.models.tags("openai", "gpt-image-2.5-flare")
    assert img["api"] == "image-generations" and img["kind"] == "image-generation"
    assert {"tag": "kind:image-generation", "source": "rule:gpt-image-*"} in img["tags"]
    assert lexi.models.resolve("openai", output="image")["api"] == "image-generations"
    assert lexi.models.resolve("openai")["api"] == "responses"

    # A default per kind: an image model becomes the image default only.
    chat = lexi.models.resolve("openai")["model"]
    assert lexi.models.set_default("openai", "gpt-image-9")["kind"] == "image-generation"
    assert lexi.models.resolve("openai", output="image")["model"] == "gpt-image-9"
    assert lexi.models.resolve("openai")["model"] == chat
    with pytest.raises(lexi.LexiError, match="not chat"):
        lexi.models.set_default("openai", "gpt-image-9", kind="chat")
    cleared = lexi.models.clear_default("openai", "image-generation")
    assert cleared["cleared"] is True and cleared["source"] == "binding"
    assert lexi.models.resolve("openai", output="image")["model"] == cleared["model"]

    # NVIDIA classifiers have no API: classification says why; resolve raises.
    safety = lexi.models.tags("nvidia", "nvidia/nemotron-3.5-content-safety")
    assert safety["api"] is None and "classifier" in safety["error"]
    with pytest.raises(lexi.LexiError, match="kind:classifier"):
        lexi.models.resolve("nvidia", "nvidia/nemotron-3.5-content-safety")

    # User tags win and persist per provider.
    out = lexi.models.tag("nvidia", "meta/*", add=["feature:tools", "label:big"], remove=["in:text"])
    assert out["tags"] == ["+feature:tools", "+label:big", "-in:text"]
    tags = {t["tag"]: t["source"] for t in lexi.models.tags("nvidia", "meta/llama-3.1-70b-instruct")["tags"]}
    assert tags["label:big"] == "user" and "in:text" not in tags
    assert (home / "providers" / "nvidia" / "model-tags.yaml").exists()
    assert lexi.models.tag("nvidia", "meta/*", clear=True)["cleared"] is True

    with pytest.raises(lexi.LexiError, match="unknown tag"):
        lexi.models.tag("nvidia", "x", add=["out:smell"])

    vocab = lexi.tags()
    assert "image-generation" in vocab["namespaces"]["kind"]
    assert vocab["kinds"]["embedding"] == {"in": ["text"], "out": ["vector"]}

    listed = {m["id"]: m for m in lexi.models.list("anthropic")}
    assert all(m["api"] == "messages" and "kind:chat" in m["tags"] for m in listed.values())


def test_hosted_catalog_tags_and_cache(home):
    m = Mock()
    try:
        m.routes[("GET", "/models")] = ok({"data": [
            {"id": "claude-a", "capabilities": {"image_input": {"supported": True}}, "max_tokens": 1000},
            {"id": "claude-b", "capabilities": {"image_input": {"supported": False}}},
        ]})
        (home / "providers").mkdir(parents=True, exist_ok=True)
        lexi.providers.list()  # seed
        (home / "providers" / "providers-config.yaml").write_text(
            f"providers:\n  anthropic:\n    template: anthropic\n    url: {m.url}\n    api_key: k\n"
        )
        # The template pages with ?limit=1000 (the mock routes on the path).
        all_ = lexi.models.hosted("anthropic")
        assert [x["id"] for x in all_] == ["claude-a", "claude-b"]
        assert all_[0]["api"] == "messages" and "in:image" in all_[0]["tags"]
        assert "in:image" not in all_[1]["tags"]
        only = lexi.models.hosted("anthropic", tags=["in:image"], cached=True)
        assert [x["id"] for x in only] == ["claude-a"]
        assert lexi.models.tags("anthropic", "claude-a")["limits"] == {"max_output_tokens": 1000}
        assert lexi.models.resolve("anthropic", "claude-b")["api"] == "messages"  # catalog-only model
    finally:
        m.server.shutdown()


def test_provider_upgrade_and_apis(home):
    import pathlib
    lexi.providers.list()  # seed
    legacy = (pathlib.Path(__file__).parents[2] / "lexi-core" / "providers" / "legacy-providers-config.yaml").read_text()
    cfg = home / "providers" / "providers-config.yaml"
    cfg.write_text(legacy.replace("default_model: gpt-4o", "default_model: gpt-5"))
    plan = lexi.providers.upgrade("openai")
    assert plan["changed"] and "backup" not in plan
    assert plan["after"] == {"template": "openai", "apis": [{"api": "responses", "default_model": "gpt-5"}]}
    done = lexi.providers.upgrade("openai", apply=True)
    assert pathlib.Path(done["backup"]).read_text() == legacy.replace("default_model: gpt-4o", "default_model: gpt-5")
    assert lexi.providers.upgrade("openai")["changed"] is False

    names = [b["api"] for b in lexi.providers.apis("openai")]
    assert names == ["responses", "image-generations", "embeddings"]
    lexi.providers.remove_api("openai", "embeddings")
    assert [b["enabled"] for b in lexi.providers.apis("openai")] == [True, True, False]
    assert lexi.providers.add_api("openai", "embeddings")["existed"] is True
    with pytest.raises(lexi.LexiError, match="only API"):
        lexi.providers.remove_api("anthropic", "messages")


def test_a_key_from_before_names_is_written_with_a_bare_hash(home, monkeypatch):
    lexi.providers.add("mine", "https://x.example/v1", "m1")
    monkeypatch.setenv("LEXI_TEST_MINE_KEY", "from-env")
    cfg = home / "providers" / "providers-config.yaml"
    cfg.write_text(cfg.read_text().replace(
        "  mine:\n", "  mine:\n    api_key: [{env: LEXI_TEST_MINE_KEY}, {file: providers/mine/api-key}]\n", 1))
    info = lexi.providers.show_key("mine#", reveal=True)
    assert (info["source"], info["from"], info["key"]) == ("env", "$LEXI_TEST_MINE_KEY", "from-env")
    assert info["label"] == "#"

    # Saved, but the variable comes first in its list and is set.
    saved = lexi.providers.set_key("mine#", "from-file")
    assert (saved["location"], saved["used"]) == ("providers/mine/api-key", "$LEXI_TEST_MINE_KEY")
    monkeypatch.delenv("LEXI_TEST_MINE_KEY")
    assert lexi.providers.show_key("mine", key_name="", reveal=True)["key"] == "from-file"

    # Renamed, it's a named key like the rest.
    lexi.providers.rename_key("mine", "", "personal")
    assert lexi.providers.show_key("mine#personal", reveal=True)["key"] == "from-file"
    with pytest.raises(lexi.LexiError, match="no unnamed key"):
        lexi.providers.show_key("mine#")


def test_key_sources_can_be_listed_added_and_removed(home, tmp_path):
    every = lexi.providers.key_sources("openai")
    assert every["keys"] == [] and every["variable"]["name"] == "OPENAI_API_KEY"
    lexi.providers.add_key("openai", "work")
    listed = lexi.providers.key_sources("openai#work")
    assert [(r["kind"], r["from"]) for r in listed["sources"]] == [
        ("saved", "providers/openai/keys/work"),
    ]
    assert listed["sources"][0]["implicit"] is True

    key = tmp_path / "openai.key"
    key.write_text("from-my-file\n")
    with pytest.raises(lexi.LexiError, match="name the key"):
        lexi.providers.add_key_source("openai", file=str(key))
    after = lexi.providers.add_key_source("openai#work", file=str(key), first=True)
    assert after["sources"][0] == {"kind": "file", "from": str(key), "value": {"file": str(key)}}
    assert lexi.providers.show_key("openai#work", reveal=True)["key"] == "from-my-file"

    cmd = [sys.executable, "-c", "print('from-cmd')"]
    after = lexi.providers.add_key_source("openai", command=cmd, first=True, key_name="work")
    assert after["sources"][0]["value"] == {"command": cmd}
    # Listing doesn't run it; revealing does.
    assert lexi.providers.show_key("openai#work")["sources"][0]["state"] == "not run"
    assert lexi.providers.show_key("openai#work", reveal=True)["key"] == "from-cmd"

    assert lexi.providers.remove_key_source("openai#work", command=cmd) == {
        "provider": "openai", "removed": True,
    }
    assert lexi.providers.remove_key_source("openai#work", file=str(key))["removed"] is True
    assert lexi.providers.remove_key_source("openai#work", file=str(key))["removed"] is False
    with pytest.raises(ValueError, match="exactly one"):
        lexi.providers.add_key_source("openai#work", env="A", file="b")
    with pytest.raises(lexi.LexiError):
        lexi.providers.add_key_source("openai#work", command=[])


def test_hosted_schema_shows_how_the_listing_is_fetched(home):
    s = lexi.models.hosted_schema("anthropic")
    assert s["url"] == "https://api.anthropic.com/v1/models"
    assert (s["source"], s["template"]) == ("template", "anthropic")
    assert s["catalog"]["name"] == {"path": "display_name"}
    with pytest.raises(lexi.LexiError):
        lexi.models.hosted_schema("nope")


def test_keys_pick_the_account_or_project(home, monkeypatch):
    monkeypatch.delenv("OPENAI_API_KEY", raising=False)
    lexi.providers.list()  # seed
    assert lexi.providers.add_key("openai", "work") == {
        "provider": "openai", "key_name": "work", "added": True,
    }
    monkeypatch.setenv("LEXI_PY_WORK_KEY", "sk-work")
    lexi.providers.add_key_source("openai", env="LEXI_PY_WORK_KEY", key_name="work")
    saved = lexi.providers.set_key("openai", "sk-personal", key_name="personal")
    assert (saved["key_name"], saved["location"]) == ("personal", "providers/openai/keys/personal")
    assert lexi.providers.show_key("openai#personal", reveal=True)["key"] == "sk-personal"
    assert lexi.providers.show_key("openai", key_name="work", reveal=True)["key"] == "sk-work"
    keys = lexi.providers.keys("openai")
    # A fresh install has no unnamed key; the variable isn't one.
    assert [k["label"] for k in keys["keys"]] == ["#work", "#personal"]
    assert (keys["default_key"], keys["reason"]) == (None, "no default key")

    # A request picks a key with key= or provider#key; an unknown one is an error.
    assert lexi.prepare("openai", "hi", key="work")["provider"] == "openai"
    assert lexi.prepare("openai#personal", "hi")["provider"] == "openai"
    with pytest.raises(lexi.LexiError, match="several keys and no default"):
        lexi.prepare("openai", "hi")
    with pytest.raises(lexi.LexiError, match='no key named "nope"'):
        lexi.prepare("openai", "hi", key="nope")
    with pytest.raises(lexi.LexiError, match="name different keys"):
        lexi.prepare("openai#work", "hi", key="personal")
    with pytest.raises(lexi.LexiError, match="isn't a key name"):
        lexi.prepare("openai#default", "hi")

    lexi.providers.set_default_key("openai", "work")
    assert lexi.prepare("openai", "hi")["provider"] == "openai"  # now uses work
    assert lexi.providers.keys("openai")["default_key"] == "work"
    # The provider's variable, when set, wins over the default.
    monkeypatch.setenv("OPENAI_API_KEY", "sk-env")
    assert lexi.providers.keys("openai")["uses"] == {"variable": "OPENAI_API_KEY"}
    monkeypatch.delenv("OPENAI_API_KEY")
    lexi.providers.rename_key("openai", "work", "acme")
    assert lexi.providers.keys("openai")["default_key"] == "acme"
    assert lexi.providers.delete_key("openai", "acme") == {"provider": "openai", "removed": "acme"}
    # One key left: it's the default.
    assert lexi.providers.keys("openai")["default_key"] == "personal"
