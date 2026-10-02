"""Harbor installed agent that runs Frankenstein Harness (fh) inside the task container.

    harbor run -d terminal-bench@4.0 \
        --agent fh_harbor.agent:FrankensteinHarness -m openai/frankenstein-v2 \
        --ae FH_ENDPOINT=http://host.docker.internal:8001/v1 --allow-agent-host host.docker.internal \
        --ak binary=/path/to/fh-x86_64-unknown-linux-musl

Settings (agent kwarg ``--ak name=value``, else the host environment variable in brackets):
    binary            local path of a static Linux fh build              [FH_BINARY]
    endpoint          OpenAI-compatible base URL reachable from the task  [FH_ENDPOINT]
    model             served model name (default: the -m model after "/") [FH_MODEL]
    api_key_env       host variable that holds the API key                [FH_API_KEY_ENV, default FH_API_KEY]
    context_window    tokens, should equal vLLM --max-model-len           [FH_CONTEXT_WINDOW]
    max_concurrency   1 = single agent; >1 lets fh split work into workers [FH_MAX_CONCURRENCY]
    commit            "true" to git-commit the result (DeepSWE-style graders diff HEAD)
    state_dir         host directory with fh's learned skills, shared across trials (empty = no learning carry-over)
"""

from __future__ import annotations

import shlex
import tempfile
from pathlib import Path
from typing import override

from pydantic import Field

from harbor.agents.capabilities import AgentCapabilities
from harbor.agents.installed.base import BaseInstalledAgent, with_prompt_template
from harbor.agents.options import InstalledAgentOptions
from harbor.environments.base import BaseEnvironment
from harbor.models.agent.context import AgentContext

from fh_harbor import common


class FhOptions(InstalledAgentOptions):
    binary: str | None = Field(default=None, description="Local path of a static Linux fh binary (FH_BINARY).")
    endpoint: str | None = Field(default=None, description="OpenAI-compatible base URL reachable from the container (FH_ENDPOINT).")
    model: str | None = Field(default=None, description="Served model name (FH_MODEL).")
    api_key_env: str | None = Field(default=None, description="Host variable holding the API key (default FH_API_KEY).")
    context_window: int | None = Field(default=None, description="Context window in tokens (FH_CONTEXT_WINDOW).")
    max_concurrency: int | None = Field(default=None, description="fh worker limit; 1 = single agent (FH_MAX_CONCURRENCY).")
    commit: bool = Field(default=False, description="Commit the verified result with git.")
    state_dir: str | None = Field(default=None, description="Host directory for learned skills shared across trials.")
    memory: str | None = Field(default=None, description='"off": still create and improve skills but never use learned ones (FH_MEMORY).')
    consolidate: bool = Field(default=False, description="Max-parallel mode: scouts/workers, then one agent consolidates (needs max_concurrency > 1).")
    mode: str = Field(default="run", description='"run" = the full fh harness; "direct" = the model alone, no harness (baseline); "delegate" = 27B plans, a worker model writes once, 27B verifies with the full fh loop; "delegate-pure" = the same without any harness (the 27B verifier is one no-tools request).')
    worker_endpoint: str | None = Field(default=None, description="delegate: the worker model's OpenAI-compatible URL (FH_WORKER_ENDPOINT).")
    worker_model: str | None = Field(default=None, description="delegate: the worker model's served name (FH_WORKER_MODEL).")
    worker_context_window: int | None = Field(default=None, description="delegate: the worker's context window (FH_WORKER_CONTEXT_WINDOW).")


class FrankensteinHarness(BaseInstalledAgent):
    """Frankenstein Harness, a verify-before-output coding agent for Qwen on vLLM."""

    capabilities = AgentCapabilities()
    options_model = FhOptions

    @staticmethod
    @override
    def name() -> str:
        return "frankenstein-harness"

    @override
    def get_version_command(self) -> str | None:
        return f"{common.FH_BIN} --version"

    def _opt(self, name: str):
        return getattr(self.options, name, None) if self.options is not None else None

    def _endpoint(self) -> str:
        ep = common.setting(self._opt("endpoint"), "FH_ENDPOINT")
        if not ep:
            raise ValueError("fh needs an endpoint reachable from the container: --ae FH_ENDPOINT=... or --ak endpoint=...")
        return ep

    def _model(self) -> str:
        m = common.setting(self._opt("model"), "FH_MODEL")
        if m:
            return m
        if self.model_name:
            return self.model_name.split("/", 1)[-1]
        raise ValueError("fh needs a model name: -m openai/<served-name> or FH_MODEL")

    def _env(self) -> dict[str, str]:
        import os

        env = {
            "FH_ENDPOINT": self._endpoint(),
            "FH_MODEL": self._model(),
            "FH_HOME": common.FH_STATE,
            "FH_CONFIG_HOME": common.FH_STATE + "/config",
            "NO_COLOR": "1",
        }
        key_var = common.setting(self._opt("api_key_env"), "FH_API_KEY_ENV", "FH_API_KEY") or "FH_API_KEY"
        key = self._extra_env.get(key_var) or os.environ.get(key_var)
        if key:
            env["FH_API_KEY"] = key
        cw = common.setting(self._opt("context_window"), "FH_CONTEXT_WINDOW")
        if cw:
            env["FH_CONTEXT_WINDOW"] = str(cw)
        mem = common.setting(self._opt("memory"), "FH_MEMORY")
        if mem:
            env["FH_MEMORY"] = mem
        if self._opt("consolidate"):
            env["FH_CONSOLIDATE"] = "1"
        if self._opt("mode") in ("delegate", "delegate-pure"):
            for name, opt in (("FH_WORKER_ENDPOINT", "worker_endpoint"), ("FH_WORKER_MODEL", "worker_model"), ("FH_WORKER_CONTEXT_WINDOW", "worker_context_window")):
                v = common.setting(self._opt(opt), name)
                if v:
                    env[name] = str(v)
            if "FH_WORKER_ENDPOINT" not in env or "FH_WORKER_MODEL" not in env:
                raise ValueError("mode=delegate needs worker_endpoint and worker_model (FH_WORKER_ENDPOINT, FH_WORKER_MODEL)")
        return env

    @override
    async def install(self, environment: BaseEnvironment) -> None:
        binary = common.setting(self._opt("binary"), "FH_BINARY")
        if not binary or not Path(binary).is_file():
            raise ValueError("set --ak binary=<path> or FH_BINARY to a static Linux fh build (scripts/bench/build-fh-linux.sh)")
        await self.ensure_system_dependencies(environment, ("git",))
        await environment.upload_file(binary, common.FH_BIN)
        user = environment.default_user
        owner = f"chown -R {shlex.quote(str(user))} {common.FH_STATE} && " if user is not None else ""
        await self.exec_as_root(
            environment,
            command=f"chmod 755 {common.FH_BIN} && mkdir -p {common.FH_STATE}/config && {owner}{common.FH_BIN} --version",
        )
        state_dir = common.setting(self._opt("state_dir"), "FH_STATE_DIR")
        if state_dir:
            with tempfile.TemporaryDirectory(prefix="fh-state-") as tmp:
                snap = Path(tmp) / "skills.db"
                if common.snapshot(Path(state_dir), snap):
                    await environment.upload_file(snap, f"{common.FH_STATE}/skills.db")
                    if user is not None:
                        await self.exec_as_root(environment, command=f"chown {shlex.quote(str(user))} {common.FH_STATE}/skills.db")

    @override
    @with_prompt_template
    async def run(self, instruction: str, environment: BaseEnvironment, context: AgentContext) -> None:
        env = self._env()
        max_conc = common.setting(self._opt("max_concurrency"), "FH_MAX_CONCURRENCY")
        commit = bool(self._opt("commit"))
        logs = str(self.environment_logs_dir) if getattr(self, "environment_logs_dir", None) else "/logs/agent"
        try:
            await self.exec_as_agent(
                environment,
                command=common.run_command(instruction, logs_dir=logs, max_concurrency=int(max_conc) if max_conc else None, commit=commit, direct=self._opt("mode") == "direct", delegate=self._opt("mode") in ("delegate", "delegate-pure"), pure=self._opt("mode") == "delegate-pure"),
                env=env,
            )
        finally:
            state_dir = common.setting(self._opt("state_dir"), "FH_STATE_DIR")
            if state_dir and self._opt("mode") != "direct":
                with tempfile.TemporaryDirectory(prefix="fh-state-") as tmp:
                    out = Path(tmp) / "skills.db"
                    try:
                        await environment.download_file(f"{common.FH_STATE}/skills.db", out)
                        common.merge(Path(state_dir), out)
                    except Exception as e:  # learning is best-effort; never fail the trial on it
                        self.logger.warning(f"fh skill sync skipped: {e}")

    @override
    def populate_context_post_run(self, context: AgentContext) -> None:
        r = common.parse_result(self.logs_dir / common.RESULT_FILE)
        if not r:
            return
        llm = r.get("llm") or {}
        prompt = int(llm.get("promptTokens") or 0)
        cached = int(llm.get("cachedTokens") or 0)
        context.n_input_tokens = prompt
        context.n_cache_tokens = cached
        context.n_output_tokens = int(llm.get("completionTokens") or 0)
        context.cost_usd = 0.0
        meta = dict(context.metadata or {})
        meta["fh"] = {
            "verdict": r.get("verdict"),
            "rounds": r.get("rounds"),
            "rolledBack": r.get("rolledBack"),
            "workers": len(r.get("workers") or []),
            "requests": llm.get("requests"),
            "toolCalls": llm.get("toolCalls"),
            "skillsUsed": r.get("skillsUsed"),
            "skillsWithheld": r.get("skillsWithheld"),
            "consolidation": r.get("consolidation"),
            "direct": r.get("direct"),
            "timings": r.get("timings"),
        }
        context.metadata = meta
