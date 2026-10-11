from pathlib import Path
import subprocess
root=Path(__file__).resolve().parents[2]
commands=[['python3', 'scripts/check-server-boundaries.py', '--self-test'], ['python3', 'scripts/check-server-boundaries.py'], ['git', 'diff', '--check'], ['rustfmt', '--edition', '2024', '--check', '--config', 'skip_children=true', 'packages/alia-server/src/bootstrap/builder.rs', 'packages/alia-server/src/bootstrap/builder_llm_tests.rs', 'packages/alia-server/src/bootstrap/registry.rs', 'packages/alia-server/src/bootstrap/registry_tests.rs', 'packages/alia-server/src/modules/interaction/routing/service.rs', 'packages/alia-server/src/providers/llm/gemini.rs', 'packages/alia-server/tests/server_runtime/health.rs'], ['python3', 'run/validation-20261011-health/test-counter.py']]
for command in commands:
 print("RUN",command,flush=True)
 subprocess.run(command,cwd=root,check=True)
