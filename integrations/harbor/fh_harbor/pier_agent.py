"""Pier installed agent (Pier is Datacurve's Harbor fork used for DeepSWE v1.1).

DeepSWE tasks run with no internet: Pier bakes the agent into the image at build time (network on) and, at
run time, lets the agent reach only the hosts in ``network_allowlist`` through its egress proxy.

    pier run -p deep-swe/tasks --agent-import-path fh_harbor.pier_agent:FrankensteinHarness \
        -m openai/frankenstein-v2 \
        --ae FH_ENDPOINT=http://<llm-host>:8001/v1 --ae FH_BINARY_URL=http://<host>:8090/fh --ak commit=true

Settings come from ``--ak name=value`` or the agent/host environment:
    binary_url (FH_BINARY_URL)   where the image build downloads a static Linux fh (required: builds cannot see host files)
    binary_sha256 (FH_BINARY_SHA256)  optional checksum of that file
    endpoint (FH_ENDPOINT), model (FH_MODEL), context_window (FH_CONTEXT_WINDOW),
    max_concurrency (FH_MAX_CONCURRENCY), commit ("true": DeepSWE grades `git diff base..HEAD`, so commit the result),
    state_dir (FH_STATE_DIR): host directory with learned skills shared across trials.
"""

from __future__ import annotations

import shlex
import tempfile
from pathlib import Path

from pier.agents.installed.base import BaseInstalledAgent, with_prompt_template
from pier.agents.network import allowlist_from_urls
from pier.environments.base import BaseEnvironment
from pier.models.agent.context import AgentContext
from pier.models.agent.install import AgentInstallSpec, InstallStep
from pier.models.agent.network import NetworkAllowlist

from fh_harbor import common


def _truthy(v) -> bool:
    return str(v).strip().lower() in ("1", "true", "yes", "on")


class FrankensteinHarness(BaseInstalledAgent):
    SUPPORTS_ATIF = False

    def __init__(
        self,
        *args,
        binary_url: str | None = None,
        binary_sha256: str | None = None,
        endpoint: str | None = None,
        model: str | None = None,
        context_window: int | str | None = None,
        max_concurrency: int | str | None = None,
        commit: bool | str = False,
        state_dir: str | None = None,
        **kwargs,
    ):
        super().__init__(*args, **kwargs)
        self._binary_url = binary_url
        self._binary_sha256 = binary_sha256
        self._endpoint = endpoint
        self._model = model
        self._context_window = context_window
        self._max_concurrency = max_concurrency
        self._commit = _truthy(commit)
        self._state_dir = state_dir

    @staticmethod
    def name() -> str:
        return "frankenstein-harness"

    def get_version_command(self) -> str | None:
        return f"{common.FH_BIN} --version"

    def _get(self, value, env_name: str, default: str | None = None) -> str | None:
        if value not in (None, ""):
            return str(value)
        return self._get_env(env_name) or default

    def _endpoint_url(self) -> str:
        ep = self._get(self._endpoint, "FH_ENDPOINT")
        if not ep:
            raise ValueError("fh needs FH_ENDPOINT (an OpenAI-compatible base URL reachable from the task)")
        return ep

    def install_spec(self) -> AgentInstallSpec:
        url = self._get(self._binary_url, "FH_BINARY_URL")
        if not url:
            raise ValueError("Pier builds the agent into the image: set FH_BINARY_URL to a downloadable static Linux fh")
        sha = self._get(self._binary_sha256, "FH_BINARY_SHA256")
        check = f" && echo {shlex.quote(sha + '  ' + common.FH_BIN)} | sha256sum -c -" if sha else ""
        fetch = (
            f"(command -v curl >/dev/null && curl -fsSL {shlex.quote(url)} -o {common.FH_BIN}) || "
            f"(command -v wget >/dev/null && wget -qO {common.FH_BIN} {shlex.quote(url)}) || "
            f"python3 -c 'import sys,urllib.request; urllib.request.urlretrieve(sys.argv[1], sys.argv[2])' {shlex.quote(url)} {common.FH_BIN}"
        )
        return AgentInstallSpec(
            agent_name=self.name(),
            version=self._version,
            steps=[
                InstallStep(
                    user="root",
                    run=(
                        f"set -e; mkdir -p /installed-agent {common.FH_STATE}/config; ({fetch}){check}; "
                        f"chmod 755 {common.FH_BIN}; chmod -R 777 {common.FH_STATE}; "
                        "command -v git >/dev/null || (apt-get update && apt-get install -y --no-install-recommends git) || true; "
                        f"{common.FH_BIN} --version"
                    ),
                )
            ],
            verification_command=self.get_version_command(),
        )

    def network_allowlist(self) -> NetworkAllowlist:
        return allowlist_from_urls([self._endpoint_url()])

    @with_prompt_template
    async def run(self, instruction: str, environment: BaseEnvironment, context: AgentContext) -> None:
        if self._state_dir or self._get_env("FH_STATE_DIR"):
            state = Path(self._get(self._state_dir, "FH_STATE_DIR"))
            with tempfile.TemporaryDirectory(prefix="fh-state-") as tmp:
                snap = Path(tmp) / "skills.db"
                if common.snapshot(state, snap):
                    await environment.upload_file(snap, f"{common.FH_STATE}/skills.db")
                    await self.exec_as_root(environment, command=f"chmod 666 {common.FH_STATE}/skills.db")
        env = {
            "FH_ENDPOINT": self._endpoint_url(),
            "FH_MODEL": self._get(self._model, "FH_MODEL") or (self.model_name or "").split("/", 1)[-1],
            "FH_HOME": common.FH_STATE,
            "FH_CONFIG_HOME": common.FH_STATE + "/config",
            "NO_COLOR": "1",
        }
        key = self._get_env(self._get_env("FH_API_KEY_ENV") or "FH_API_KEY")
        if key:
            env["FH_API_KEY"] = key
        cw = self._get(self._context_window, "FH_CONTEXT_WINDOW")
        if cw:
            env["FH_CONTEXT_WINDOW"] = str(cw)
        mc = self._get(self._max_concurrency, "FH_MAX_CONCURRENCY")
        try:
            await self.exec_as_agent(
                environment,
                command=common.run_command(instruction, logs_dir="/logs/agent", max_concurrency=int(mc) if mc else None, commit=self._commit),
                env=env,
            )
        finally:
            sd = self._get(self._state_dir, "FH_STATE_DIR")
            if sd:
                with tempfile.TemporaryDirectory(prefix="fh-state-") as tmp:
                    out = Path(tmp) / "skills.db"
                    try:
                        await environment.download_file(f"{common.FH_STATE}/skills.db", out)
                        common.merge(Path(sd), out)
                    except Exception as e:
                        self.logger.warning(f"fh skill sync skipped: {e}")

    def populate_context_post_run(self, context: AgentContext) -> None:
        r = common.parse_result(self.logs_dir / common.RESULT_FILE)
        if not r:
            return
        llm = r.get("llm") or {}
        context.n_input_tokens = int(llm.get("promptTokens") or 0)
        context.n_cache_tokens = int(llm.get("cachedTokens") or 0)
        context.n_output_tokens = int(llm.get("completionTokens") or 0)
        context.cost_usd = 0.0
