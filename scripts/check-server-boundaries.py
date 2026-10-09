#!/usr/bin/env python3
"""Check the server's migrated ownership boundaries using only Python's stdlib.

Run from any directory: python3 scripts/check-server-boundaries.py
Check the checker itself: python3 scripts/check-server-boundaries.py --self-test
This checks the completed migration, not every dependency in the modular monolith.
"""

import argparse
from pathlib import Path
import re
import tempfile


REMOVED_PATHS = (
    "src/module",
    "src/providers/memory",
    "src/providers/conversation",
    "src/providers/cache",
    "src/providers/router",
    "src/providers/persona",
    "src/providers/skills",
    "src/providers/insights",
    "src/providers/sensor",
    "src/channels/pipeline.rs",
    "src/channels/pipeline",
    "src/memory_policy.rs",
)
REQUIRED_PATHS = (
    "src/bootstrap/builder.rs",
    "src/bootstrap/factory.rs",
    "src/bootstrap/registry.rs",
    "src/modules/platform/background.rs",
    "src/modules/platform/lifecycle.rs",
    "src/modules/memory/service.rs",
    "src/modules/memory/types.rs",
    "src/modules/interaction/services.rs",
    "src/modules/interaction/conversation/service.rs",
    "src/modules/interaction/cache/store.rs",
    "src/modules/interaction/cache/types.rs",
    "src/modules/interaction/routing/service.rs",
    "src/modules/interaction/persona/service.rs",
    "src/modules/interaction/skills/registry.rs",
    "src/modules/platform/insights/service.rs",
    "src/modules/platform/sensors/registry.rs",
)
RETIRED_SYMBOLS = (
    "MemoryBackend", "NoopMemory", "AliaMemory", "PipelinePolicies", "SharedPipelinePolicies",
    "spawn_pipeline", "ConversationBackend", "DatabaseConversationBackend", "CacheBackend",
    "DbCache", "RouterStrategy", "MultiLayerRouter", "PassthroughRouter", "DirectRouter",
    "PersonaProvider", "TemplatePersona", "SkillProvider", "BuiltinSkills", "InsightStrategy",
    "ScheduledInsights", "NoopInsights", "ModuleContext",
)
FIXED_FACTORIES = "memory|conversation|cache|router|persona|skills|insights|sensor|rule_engine|scheduler|tools_with_sensors"
OLD_SYMBOLS = re.compile(
    r"\b(?:" + "|".join(RETIRED_SYMBOLS) + r")\b"
    r"|\b(?:crate|alia_server)::module\b"
    r"|\bproviders\s*::\s*(?:memory|conversation|cache|router|persona|skills|insights|sensor)\b"
    r"|\bchannels\s*::\s*pipeline\b"
    r"|\bmemory_policy\s*::"
    r"|\bfactory\s*::\s*create_(?:" + FIXED_FACTORIES + r")\b"
)
MEMORY_DECLARATION = re.compile(
    r"\bpub\s+(?:struct|enum|trait|type)\s+(?:Memory\w*|RetrievalOpts|GraphFragment|GraphSnippet|SuppressedMemory)\b"
)
CORE_CONTRACT = re.compile(
    r"\b(?:ConversationService|ConversationStore|ConversationEventEntry|CacheService|CacheStore|CacheVersion|CachedResponse|CacheEntry"
    r"|CacheStatus|CacheReview|CacheWork|CacheCas|SemanticCacheCandidate|DEFAULT_CACHE_SCOPE"
    r"|CACHE_METADATA_SCHEMA|RouterService|RouterInput|RouteDecision|ToolCandidateInfo|ToolSelectionInput"
    r"|PersonaService|Persona|RenderedPersonaPrompt|DEFAULT_ASSISTANT_NAME|sanitize_assistant_name"
    r"|SkillRegistry|SkillMatch|SkillResolution|SkillInfo|PluginSkillDefinition|InsightService|InsightTask|InsightStatus|InsightPhase"
    r"|SensorDefinition|SensorReading|SensorQuery|SensorSummary|SensorReadMode|SensorStalePolicy"
    r"|SensorFreshnessPolicy|SensorReadResult|SensorReadProvider|SensorRegistry|SensorRegistryConfig)\b"
)
FIXED_FACTORY_DECLARATION = re.compile(r"\bfn\s+create_(?:" + FIXED_FACTORIES + r")\b")
ASSEMBLY_DEPENDENCY = re.compile(
    r"\b(?:ModuleRegistry|AppState)\b|\bcrate\s*::\s*(?:bootstrap|adapters)\b"
)
CAPTURE_DEPENDENCY = re.compile(r"\b(?:channels|PendingTextPersistence|InteractionServices)\b")
DOMAIN_TYPE_DEPENDENCY = re.compile(
    r"\b(?:sea_orm|tonic)\b|\b(?:providers|adapters|entity)\s*::"
)
# These implementations now bind explicit dependencies before lifecycle initialization.
NARROW_INITIALIZATION_FILES = (
    "src/modules/platform/lifecycle.rs",
    "src/modules/platform/insights/service.rs",
    "src/modules/memory/service.rs",
    "src/modules/interaction/conversation/service.rs",
    "src/modules/interaction/cache/store.rs",
    "src/modules/interaction/routing/service.rs",
    "src/modules/interaction/persona/service.rs",
    "src/modules/interaction/skills/registry.rs",
    "src/providers/llm/openai.rs",
    "src/providers/llm/anthropic.rs",
    "src/providers/llm/gemini.rs",
    "src/providers/embedding/openai.rs",
    "src/providers/stt/whisper.rs",
    "src/providers/tts/gpt_sovits.rs",
    "src/rules/engine.rs",
    "src/scheduler/cron.rs",
    "src/tools/builtin.rs",
    "src/channels/cli.rs",
    "src/channels/embedded/channel.rs",
    "src/plugins/home_assistant.rs",
)
SERVER_CONFIG_DEPENDENCY = re.compile(r"\bServerConfig\b")
# init is the server lifecycle hook, including test implementations; an argument
# would reintroduce the initialization locator even if its type were renamed.
INIT_SIGNATURE = re.compile(r"\bfn\s+init\s*(?:<[^(){};]*>)?\s*\(([^)]*)\)")
BOUND_INIT_RECEIVER = re.compile(r"\s*&\s*mut\s+self\s*,?\s*")


def check(server: Path) -> list[str]:
    errors = []
    for relative in REMOVED_PATHS:
        if (server / relative).exists():
            errors.append(f"{relative}: retired ownership path exists")
    for relative in REQUIRED_PATHS:
        if not (server / relative).is_file():
            errors.append(f"{relative}: canonical owner is missing")

    for directory in ("src", "tests", "examples"):
        for path in sorted((server / directory).rglob("*.rs")):
            relative = path.relative_to(server).as_posix()
            text = path.read_text()
            match = OLD_SYMBOLS.search(text)
            if match:
                line = text.count("\n", 0, match.start()) + 1
                errors.append(f"{relative}:{line}: retired symbol/path {match.group()}")
            for match in INIT_SIGNATURE.finditer(text):
                if not BOUND_INIT_RECEIVER.fullmatch(match.group(1)):
                    line = text.count("\n", 0, match.start()) + 1
                    errors.append(f"{relative}:{line}: lifecycle init must use pre-bound dependencies")
            if relative in NARROW_INITIALIZATION_FILES:
                match = SERVER_CONFIG_DEPENDENCY.search(text)
                if match:
                    line = text.count("\n", 0, match.start()) + 1
                    errors.append(f"{relative}:{line}: initialization depends on global ServerConfig")
            if relative == "src/bootstrap/factory.rs":
                match = FIXED_FACTORY_DECLARATION.search(text)
                if match:
                    line = text.count("\n", 0, match.start()) + 1
                    errors.append(f"{relative}:{line}: fixed core factory {match.group()}")
            if relative.startswith("src/providers/"):
                declarations = re.findall(
                    r"\bpub\s+(?:struct|enum|trait|type|const|fn)\s+(\w+)", text
                )
                exports = re.findall(r"\bpub\s+use\s+[^;]+;", text)
                if any(CORE_CONTRACT.fullmatch(name) for name in declarations) or any(
                    CORE_CONTRACT.search(export) or re.search(
                        r"\bmodules\s*::\s*(?:interaction\s*::\s*(?:conversation|cache|routing|persona|skills)"
                        r"|platform\s*::\s*(?:insights|sensors))\b", export
                    )
                    for export in exports
                ):
                    errors.append(f"{relative}: fixed core contract must be owned by its domain")
            if relative.startswith(("src/modules/memory/", "src/modules/interaction/")):
                match = ASSEMBLY_DEPENDENCY.search(text)
                if match:
                    line = text.count("\n", 0, match.start()) + 1
                    errors.append(f"{relative}:{line}: domain depends on assembly {match.group()}")
            if relative == "src/modules/memory/capture.rs":
                match = CAPTURE_DEPENDENCY.search(text)
                if match:
                    line = text.count("\n", 0, match.start()) + 1
                    errors.append(f"{relative}:{line}: Memory capture depends on its caller {match.group()}")
            if relative == "src/modules/memory/types.rs":
                match = DOMAIN_TYPE_DEPENDENCY.search(text)
                if match:
                    line = text.count("\n", 0, match.start()) + 1
                    errors.append(f"{relative}:{line}: domain value depends on infrastructure {match.group()}")

    adapter = server / "src/providers/adapter.rs"
    if adapter.exists():
        text = adapter.read_text()
        if MEMORY_DECLARATION.search(text) or re.search(
            r"\bpub\s+use\s+crate\s*::\s*modules\s*::\s*memory\b", text
        ):
            errors.append("src/providers/adapter.rs: Memory contract must be owned by modules/memory")
    return errors


def self_test() -> None:
    with tempfile.TemporaryDirectory(prefix="alia-boundaries-") as temporary:
        server = Path(temporary)
        for relative in REQUIRED_PATHS:
            path = server / relative
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text("")
        assert not check(server)
        for relative in REMOVED_PATHS:
            retired = server / relative
            retired.parent.mkdir(parents=True, exist_ok=True)
            if retired.suffix:
                retired.write_text("")
            else:
                retired.mkdir()
            assert any("retired ownership path" in error for error in check(server)), relative
            if retired.is_dir():
                retired.rmdir()
            else:
                retired.unlink()
        fixture = server / "src/modules/interaction/services.rs"
        for invalid, expected in (
            ("use alia_server::module::ModuleLifecycle;", "retired symbol/path"),
            ("pub memory: Arc<dyn MemoryBackend>,", "retired symbol/path"),
            ("use crate::memory_policy::merge_memory_patch;", "retired symbol/path"),
            ("use crate::bootstrap::ModuleRegistry;", "domain depends on assembly"),
            ("use crate::adapters::easy_turn::EasyTurnDetector;", "domain depends on assembly"),
        ):
            fixture.write_text(invalid)
            assert any(expected in error for error in check(server)), invalid
        for symbol in RETIRED_SYMBOLS:
            fixture.write_text(f"type Retired = {symbol};")
            assert any("retired symbol/path" in error for error in check(server)), symbol
        for invalid in (
            "pub struct ModuleContext {}",
            "use crate::modules::platform::lifecycle::ModuleContext as Dependencies;",
            "pub use crate::modules::platform::lifecycle::{ModuleContext};",
        ):
            fixture.write_text(invalid)
            assert any("retired symbol/path" in error for error in check(server)), invalid
        for owner in ("conversation", "cache", "router", "persona", "skills", "insights", "sensor"):
            fixture.write_text(f"use crate::providers::{owner}::Service;")
            assert any("retired symbol/path" in error for error in check(server)), owner
        for owner in FIXED_FACTORIES.split("|"):
            fixture.write_text(f"let core = factory::create_{owner}();")
            assert any("retired symbol/path" in error for error in check(server)), owner
        fixture.write_text("fn use_ports(_: &dyn ConversationStore, _: &dyn CacheStore, _: &dyn SensorReadProvider, _: &dyn LlmAdapter) {}")
        assert not check(server)
        fixture.write_text("")
        factory = server / "src/bootstrap/factory.rs"
        for owner in FIXED_FACTORIES.split("|"):
            factory.write_text(f"pub fn create_{owner}() {{}}")
            assert any("fixed core factory" in error for error in check(server)), owner
        factory.write_text("pub fn create_llm() -> Box<dyn LlmAdapter> {}")
        api = server / "src/api/personas.rs"
        api.parent.mkdir(parents=True)
        api.write_text("async fn create_persona() {}")
        assert not check(server)
        adapter = server / "src/providers/adapter.rs"
        adapter.parent.mkdir(parents=True, exist_ok=True)
        for invalid in (
            "pub struct MemoryNode;",
            "pub use crate::modules::memory::{MemoryNode};",
        ):
            adapter.write_text(invalid)
            assert any("Memory contract" in error for error in check(server)), invalid
        for invalid in (
            "pub trait ConversationStore {}",
            "pub type CacheService = Service;",
            "pub struct CacheVersion;",
            "pub enum RouteDecision {}",
            "pub struct Persona;",
            "pub struct SkillInfo;",
            "pub enum InsightPhase {}",
            "pub trait SensorReadProvider {}",
            "pub const DEFAULT_CACHE_SCOPE: &str = \"default\";",
            "pub fn sanitize_assistant_name() {}",
            "pub use crate::modules::interaction::cache::*;",
            "pub use crate::modules::platform::sensors::{SensorDefinition};",
        ):
            adapter.write_text(invalid)
            assert any("fixed core contract" in error for error in check(server)), invalid
        adapter.write_text("pub trait LlmAdapter {} pub use crate::modules::platform::lifecycle::ModuleLifecycle;")
        assert not check(server)
        adapter.write_text("")
        capture = server / "src/modules/memory/capture.rs"
        for invalid in (
            "use crate::channels::ChannelRegistry;",
            "fn capture(input: PendingTextPersistence) {}",
            "fn capture(services: &InteractionServices) {}",
        ):
            capture.write_text(invalid)
            assert any("Memory capture depends on its caller" in error for error in check(server)), invalid
        capture.write_text("")
        domain_types = server / "src/modules/memory/types.rs"
        for invalid in (
            "use sea_orm::DatabaseConnection;",
            "use tonic::Request;",
            "use crate::providers::llm::OpenAiLlm;",
            "pub user: crate::entity::user::Model,",
        ):
            domain_types.write_text(invalid)
            assert any("domain value depends on infrastructure" in error for error in check(server)), invalid
        domain_types.write_text("")
        assert not check(server)
        lifecycle = server / "src/modules/platform/lifecycle.rs"
        for invalid in (
            "pub trait ModuleLifecycle { async fn init(&mut self, deps: &Dependencies); }",
            "pub trait ModuleLifecycle { async fn init(&mut self, config: &RenamedConfig); }",
            "pub trait ModuleLifecycle { async fn init<T>(&mut self, deps: T); }",
        ):
            lifecycle.write_text(invalid)
            assert any("lifecycle init must use pre-bound" in error for error in check(server)), invalid
        lifecycle.write_text("pub trait ModuleLifecycle { async fn init(&mut self); }")
        fixture.write_text("async fn init(\n    &mut self,\n    deps: &RenamedInitializationBundle,\n) {}")
        assert any("lifecycle init must use pre-bound" in error for error in check(server))
        fixture.write_text("async fn init(&mut self,) {}")
        assert not check(server)
        fixture.write_text("")
        for relative in NARROW_INITIALIZATION_FILES:
            module = server / relative
            module.parent.mkdir(parents=True, exist_ok=True)
            for invalid in (
                "use crate::config::ServerConfig as Dependencies;",
                "pub fn new(config: &crate::config::ServerConfig) -> Self {}",
                "struct Service { config: Arc<ServerConfig> }",
            ):
                module.write_text(invalid)
                assert any("initialization depends on global ServerConfig" in error for error in check(server)), (relative, invalid)
            module.write_text("pub fn new(config: &NarrowProviderSection) -> Self {} async fn init(&mut self) {}")
            assert not check(server), relative
            module.write_text("")
        for relative in (
            "src/bootstrap/factory.rs",
            "src/modules/memory/context/policy.rs",
            "src/modules/memory/context/appraisal_policy.rs",
            "src/providers/adapter.rs",
            "tests/config_fixture.rs",
        ):
            allowed = server / relative
            allowed.parent.mkdir(parents=True, exist_ok=True)
            allowed.write_text("fn from_server_config(config: &crate::config::ServerConfig) {}")
        assert not check(server)
        fixture.unlink()
        assert any("canonical owner is missing" in error for error in check(server))
    print("Server boundary checker self-test passed")


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--self-test", action="store_true")
    args = parser.parse_args()
    if args.self_test:
        self_test()
        return 0
    server = Path(__file__).resolve().parents[1] / "packages/alia-server"
    errors = check(server)
    if errors:
        print("\n".join(errors))
        return 1
    print("Server ownership boundaries passed")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
