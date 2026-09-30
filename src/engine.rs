use crate::agent::{run_agent, AgentOptions, AgentResult, ConfirmFn, Events};
use crate::config::{Config, Env};
use crate::fingerprint::{fingerprint, Fingerprint};
use crate::funnel::intake::{inspect_repo, intake, render_plan, Intake, IntakeArgs, Subtask};
use crate::llm::client::{LlmClient, LlmStats};
use crate::llm::metrics::{delta, fetch_metrics, MetricsDelta};
use crate::orchestrator::governor::{Governor, MetricsFn};
use crate::orchestrator::master::{owners_of, run_workers, MasterOptions, WorkerResult};
use crate::agent::prompt::{context_block, ContextParts};
use crate::session::checkpoint::{Changes, Checkpoints};
use crate::session::log::SessionLog;
use crate::skills::design::{design_note, detect_design, is_ui_task, DesignSignal};
use crate::skills::gate::{gate, render_skills, render_user};
use crate::skills::store::{NewSkill, Scope};
use crate::skills::miner::{improve_used, mine, promote_eligible, MineInput, MineOutcome};
use crate::skills::police::{evaluate, POLICE_DEFAULTS};
use crate::skills::reuse::{accept_reuse, find_reuse_offers, ReuseOffer};
use crate::skills::store::{Skill, SkillStore};
use crate::skills::usercfg::load_user_config;
use crate::types::{Message, Mode};
use crate::util::proc::{run_simple, ShellWrap};
use crate::util::sandbox::sandbox_for;
use crate::verify::rounds::{feedback_from, finalize, run_round, ReviewerStats, RoundReport, VerifyCtx, VerifyReport, VerifyState, Verdict};
use async_trait::async_trait;
use futures_util::future::join_all;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Instant;
use tokio_util::sync::CancellationToken;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NoticeKind {
    Phase,
    Skill,
    Verify,
    Warn,
    Info,
}

impl NoticeKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            NoticeKind::Phase => "phase",
            NoticeKind::Skill => "skill",
            NoticeKind::Verify => "verify",
            NoticeKind::Warn => "warn",
            NoticeKind::Info => "info",
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct PlanDecision {
    pub ok: bool,
    pub feedback: Option<String>,
}

/// User interaction surface: terminal, web UI, or headless (nothing is ever asked).
#[async_trait]
pub trait Io: Send + Sync {
    fn notice(&self, kind: NoticeKind, msg: &str);
    fn progress(&self, _delta: &str) {}
    fn reasoning(&self, _delta: &str) {}
    fn tool_start(&self, _name: &str, _args: &Value) {}
    fn tool_end(&self, _name: &str, _ok: bool, _output: &str, _ms: u64) {}
    async fn ask_questions(&self, questions: Vec<String>) -> Vec<String>;
    async fn approve_plan(&self, plan: &str, trivial: bool) -> PlanDecision;
    async fn confirm(&self, tool: &str, args: &Value) -> bool;
    /// (from_project, chosen skill ids) per offer
    async fn offer_reuse(&self, offers: &[ReuseOffer]) -> Vec<(String, Vec<String>)>;
}

/// Non-interactive IO for eval/CI/auto mode.
pub struct HeadlessIo {
    pub log: Option<Arc<dyn Fn(&str) + Send + Sync>>,
}

#[async_trait]
impl Io for HeadlessIo {
    fn notice(&self, kind: NoticeKind, msg: &str) {
        if let Some(l) = &self.log {
            l(&format!("[{}] {msg}", kind.as_str()));
        }
    }
    async fn ask_questions(&self, q: Vec<String>) -> Vec<String> {
        q.iter().map(|_| "(use your best judgement)".to_string()).collect()
    }
    async fn approve_plan(&self, _: &str, _: bool) -> PlanDecision {
        PlanDecision { ok: true, feedback: None }
    }
    async fn confirm(&self, _: &str, _: &Value) -> bool {
        true
    }
    async fn offer_reuse(&self, _: &[ReuseOffer]) -> Vec<(String, Vec<String>)> {
        vec![]
    }
}

#[derive(Clone)]
pub struct TaskOptions {
    /// autonomous: no questions or plan approval, more verification rounds
    pub auto: bool,
    pub approval: Mode,
    pub commit: bool,
    pub plan_only: bool,
    /// keep changes even when verification fails (default: roll back to the checkpoint)
    pub keep_on_fail: bool,
    pub sandbox: bool,
    pub no_mine: bool,
    pub cancel: Option<CancellationToken>,
    /// continue an interrupted session: the stored plan and base checkpoint are reused
    pub resume: Option<ResumeState>,
}

/// State of an interrupted run, loaded from the session log.
#[derive(Clone)]
pub struct ResumeState {
    pub session_id: String,
    pub plan: Intake,
    pub base: String,
    pub history: Option<Vec<Message>>,
}

impl Default for TaskOptions {
    fn default() -> Self {
        TaskOptions { auto: false, approval: Mode::AutoEdit, commit: false, plan_only: false, keep_on_fail: false, sandbox: false, no_mine: false, cancel: None, resume: None }
    }
}

#[derive(Clone, Debug, Default)]
pub struct Timings {
    pub gate_ms: f64,
    pub intake_ms: u64,
    pub work_ms: u64,
    pub verify_ms: u64,
    pub total_ms: u64,
}

#[derive(Clone, Debug)]
pub struct AgentSummary {
    pub steps: usize,
    pub tool_calls: usize,
    pub failed_tools: usize,
    pub stopped: String,
}

#[derive(Clone, Debug)]
pub struct TaskResult {
    /// pass | fail | unverified | planned | aborted | error
    pub verdict: String,
    pub final_text: String,
    pub reason: String,
    pub changed: Vec<String>,
    pub rounds: usize,
    pub rolled_back: bool,
    pub rejected_patch: Option<String>,
    pub plan: Option<Intake>,
    pub skills_used: Vec<String>,
    pub gate: Option<(f64, u32, String)>,
    pub timings: Timings,
    pub llm: LlmStats,
    pub metrics: MetricsDelta,
    pub verify: Option<VerifyReport>,
    pub workers: Vec<WorkerResult>,
    pub agent: Option<AgentSummary>,
    pub reviewer: Option<ReviewerStats>,
    /// unified diff of what the task changed (kept even when rolled back), clipped
    pub diff: String,
}

impl TaskResult {
    fn new(verdict: &str, reason: &str) -> Self {
        TaskResult {
            verdict: verdict.into(),
            final_text: String::new(),
            reason: reason.into(),
            changed: vec![],
            rounds: 0,
            rolled_back: false,
            rejected_patch: None,
            plan: None,
            skills_used: vec![],
            gate: None,
            timings: Timings::default(),
            llm: LlmStats::default(),
            metrics: MetricsDelta::default(),
            verify: None,
            workers: vec![],
            agent: None,
            reviewer: None,
            diff: String::new(),
        }
    }

    pub fn to_json(&self) -> Value {
        let plan = self.plan.as_ref().map(|p| {
            json!({"trivial": p.trivial, "questions": p.questions, "enriched": p.enriched, "acceptance": p.acceptance,
                   "plan": p.plan.iter().map(|s| json!({"step": s.step, "files": s.files})).collect::<Vec<_>>(),
                   "assumptions": p.assumptions, "subtasks": p.subtasks.iter().map(|s| json!({"id": s.id, "goal": s.goal, "files": s.files, "deps": s.deps})).collect::<Vec<_>>()})
        });
        let verify = self.verify.as_ref().map(|v| {
            json!({"verdict": v.verdict.as_str(), "reason": v.reason, "rounds": v.rounds.iter().map(|r| json!({
                "round": r.round, "verdict": r.verdict.as_str(), "droppedFindings": r.dropped_findings, "reviewerRan": r.reviewer_ran,
                "checks": r.checks.iter().map(|c| json!({"name": c.name, "status": c.status.as_str(), "detail": c.detail, "ms": c.ms})).collect::<Vec<_>>(),
                "findings": r.findings.iter().map(|f| json!({"file": f.file, "line": f.line, "severity": f.severity, "claim": f.claim})).collect::<Vec<_>>()
            })).collect::<Vec<_>>()})
        });
        json!({
            "verdict": self.verdict, "final": self.final_text, "reason": self.reason, "changed": self.changed, "rounds": self.rounds,
            "rolledBack": self.rolled_back, "rejectedPatch": self.rejected_patch, "plan": plan, "skillsUsed": self.skills_used,
            "gate": self.gate.as_ref().map(|g| json!({"ms": g.0, "llmCalls": g.1, "decision": g.2})),
            "timings": {"gateMs": self.timings.gate_ms, "intakeMs": self.timings.intake_ms, "workMs": self.timings.work_ms, "verifyMs": self.timings.verify_ms, "totalMs": self.timings.total_ms},
            "llm": {"requests": self.llm.requests, "promptTokens": self.llm.prompt_tokens, "completionTokens": self.llm.completion_tokens, "cachedTokens": self.llm.cached_tokens, "toolCalls": self.llm.tool_calls, "repaired": self.llm.repaired, "malformed": self.llm.malformed, "thinkLeaks": self.llm.think_leaks, "retries": self.llm.retries},
            "metrics": {"acceptanceRate": self.metrics.acceptance_rate, "prefixHitRate": self.metrics.prefix_hit_rate, "meanAcceptedPerDraft": self.metrics.mean_accepted_per_draft, "perPositionAcceptance": self.metrics.per_position_acceptance},
            "verify": verify,
            "workers": self.workers.iter().map(|w| json!({"id": w.id, "summary": w.summary, "touched": w.touched, "stopped": w.stopped.as_str(), "steps": w.steps, "tokens": w.tokens, "blocked": w.blocked.len()})).collect::<Vec<_>>(),
            "agent": self.agent.as_ref().map(|a| json!({"steps": a.steps, "toolCalls": a.tool_calls, "failedTools": a.failed_tools, "stopped": a.stopped})),
            "reviewer": self.reviewer.as_ref().map(|r| json!({"raised": r.raised, "valid": r.valid, "dropped": r.dropped, "blockers": r.blockers})),
            "diff": self.diff,
        })
    }
}

/// Loads an interrupted session (the newest, or `id`) for `TaskOptions::resume`: returns (task text, state).
pub fn load_resume(env: &Env, cwd: &Path, id: Option<&str>) -> Result<(String, ResumeState), String> {
    let fp = fingerprint(cwd);
    let s = crate::session::log::load(env, &fp.project_id, id).ok_or("no session to resume")?;
    if let Some(v) = &s.verdict {
        return Err(format!("session {} already finished: {v} ({})", s.id, s.reason));
    }
    let (Some(plan), Some(base)) = (s.plan.clone(), s.checkpoint.clone().filter(|c| !c.is_empty())) else {
        return Err(format!("session {} was interrupted before planning finished; run the task again: {}", s.id, s.task));
    };
    Ok((s.task.clone(), ResumeState { session_id: s.id.clone(), plan, base, history: s.history.clone() }))
}

/// Scope for the diff-scope check: planned files, their directories, and test files.
pub fn allowed_from_plan(plan: &[crate::funnel::intake::PlanStep], subtasks: &[Subtask]) -> Option<Vec<String>> {
    let files: Vec<String> = plan.iter().flat_map(|p| p.files.clone()).chain(subtasks.iter().flat_map(|s| s.files.clone())).map(|f| f.trim_start_matches("./").to_string()).filter(|f| !f.is_empty()).collect();
    if files.is_empty() {
        return None;
    }
    let mut globs: Vec<String> = ["**/*.test.*", "**/*.spec.*", "test/**", "tests/**", "__tests__/**"].iter().map(|s| s.to_string()).collect();
    for f in files {
        if let Some((dir, _)) = f.rsplit_once('/') {
            if !f.contains('*') {
                globs.push(format!("{dir}/**"));
            }
        }
        globs.push(f);
    }
    globs.dedup();
    Some(globs)
}

pub struct Engine {
    pub cfg: Config,
    pub env: Env,
    pub io: Arc<dyn Io>,
    pub cwd: PathBuf,
    pub llm: LlmClient,
    pub store: SkillStore,
    pub hooks: Arc<crate::hooks::Hooks>,
    mcp: tokio::sync::OnceCell<Vec<crate::tools::ToolRef>>,
    started: std::sync::atomic::AtomicBool,
    session: Mutex<Option<SessionLog>>,
    pending: Mutex<Vec<tokio::task::JoinHandle<()>>>,
}

impl Engine {
    pub fn new(cfg: Config, env: Env, io: Arc<dyn Io>, cwd: impl Into<PathBuf>, store: SkillStore) -> Self {
        let llm = LlmClient::new(cfg.clone(), env.clone());
        let cwd: PathBuf = cwd.into();
        let hooks = Arc::new(crate::hooks::Hooks::load(&cwd, &env));
        Engine { cfg, env, io, cwd, llm, store, hooks, mcp: tokio::sync::OnceCell::new(), started: std::sync::atomic::AtomicBool::new(false), session: Mutex::new(None), pending: Mutex::new(vec![]) }
    }

    /// Wait for post-delivery background work (skill learning) so short-lived CLI runs do not drop it.
    pub async fn drain(&self, timeout_ms: u64) {
        let hs: Vec<_> = self.pending.lock().unwrap().drain(..).collect();
        let _ = tokio::time::timeout(std::time::Duration::from_millis(timeout_ms), join_all(hs)).await;
    }

    fn agent_options(&self, o: &TaskOptions, context: &str, wrap: &Option<ShellWrap>) -> AgentOptions {
        let mut ao = AgentOptions::new(self.llm.clone(), self.cwd.clone(), o.approval);
        ao.max_steps = self.cfg.max_steps;
        ao.cancel = o.cancel.clone();
        ao.context = Some(context.to_string());
        ao.context_window = self.cfg.context_window;
        ao.llm_compaction = self.cfg.llm_compaction;
        ao.wrap_shell = wrap.clone();
        if let Some(t) = self.mcp.get() {
            ao.tools.extend(t.iter().cloned());
        }
        ao.hooks = if self.hooks.is_empty() { None } else { Some(self.hooks.clone()) };
        let io = self.io.clone();
        let (io1, io2, io3, io4, io5) = (io.clone(), io.clone(), io.clone(), io.clone(), io.clone());
        ao.events = Events {
            text: Some(Arc::new(move |d| io1.progress(d))),
            reasoning: Some(Arc::new(move |d| io2.reasoning(d))),
            tool_start: Some(Arc::new(move |c| io3.tool_start(&c.name, &c.args))),
            tool_end: Some(Arc::new(move |c, r, ms| io4.tool_end(&c.name, r.ok, &r.output, ms))),
            notice: Some(Arc::new(move |m| io5.notice(NoticeKind::Info, m))),
            on_messages: self.session.lock().unwrap().clone().map(|sl| Arc::new(move |m: &[Message]| sl.save_history(m)) as Arc<dyn Fn(&[Message]) + Send + Sync>),
        };
        let cio = io.clone();
        let confirm: ConfirmFn = Arc::new(move |tool, args| {
            let cio = cio.clone();
            Box::pin(async move { cio.confirm(&tool, &args).await })
        });
        ao.confirm = Some(confirm);
        ao
    }

    pub async fn run_task(&self, task: &str, o: TaskOptions) -> TaskResult {
        let t0 = Instant::now();
        let mut timings = Timings::default();
        self.llm.reset_stats();
        let m0 = fetch_metrics(&self.cfg, &self.env).await;
        let io = self.io.clone();

        macro_rules! finish {
            ($r:expr) => {{
                let mut r: TaskResult = $r;
                timings.total_ms = t0.elapsed().as_millis() as u64;
                r.timings = timings.clone();
                r.llm = self.llm.stats();
                r.metrics = delta(&m0, &fetch_metrics(&self.cfg, &self.env).await);
                if matches!(r.verdict.as_str(), "pass" | "unverified" | "fail") {
                    crate::session::signals::write_last_task(&self.cwd, &r.verdict, r.rejected_patch.as_deref(), &r.changed);
                }
                if self.cfg.telemetry {
                    crate::session::signals::record_task(&self.env, json!({"ts": std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0), "verdict": r.verdict, "rounds": r.rounds, "totalMs": r.timings.total_ms, "promptTokens": r.llm.prompt_tokens, "completionTokens": r.llm.completion_tokens, "repaired": r.llm.repaired, "acceptanceRate": r.metrics.acceptance_rate}));
                }
                if let Some(sl) = self.session.lock().unwrap().clone() {
                    sl.append(json!({"t": "result", "verdict": r.verdict, "reason": r.reason, "changed": r.changed}));
                }
                self.hooks.fire(crate::hooks::HookEvent::Stop, json!({"verdict": r.verdict, "reason": r.reason, "changed": r.changed})).await;
                return r;
            }};
        }

        if self.hooks.skipped_untrusted && !self.started.load(std::sync::atomic::Ordering::Relaxed) {
            io.notice(NoticeKind::Warn, "project hooks (.fh/hooks.json) were skipped: this workspace is not trusted (run `fh trust`)");
        }
        if !self.started.swap(true, std::sync::atomic::Ordering::Relaxed) {
            for n in self.hooks.fire(crate::hooks::HookEvent::SessionStart, json!({})).await.notes {
                io.notice(NoticeKind::Info, &n);
            }
        }
        if self.mcp.get().is_none() {
            let (tools, notes) = crate::mcp::load_tools(&self.cwd, &self.env).await;
            for n in notes {
                io.notice(NoticeKind::Info, &n);
            }
            let _ = self.mcp.set(tools);
        }
        let up = self.hooks.fire(crate::hooks::HookEvent::UserPromptSubmit, json!({"prompt": task})).await;
        if let Some(why) = up.blocked {
            finish!(TaskResult::new("aborted", &format!("blocked by hook: {why}")));
        }
        let fp: Fingerprint = fingerprint(&self.cwd);
        let known = self.store.is_known_project(&fp.project_id);
        let slog = match &o.resume {
            Some(r) => SessionLog::open(&self.env, &fp.project_id, &r.session_id),
            None => SessionLog::create(&self.env, &fp.project_id),
        };
        slog.append(json!({"t": if o.resume.is_some() { "resume" } else { "start" }, "task": task, "auto": o.auto}));
        *self.session.lock().unwrap() = Some(slog.clone());
        let label = self.cwd.file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_else(|| "project".into());
        self.store.register_project(&fp, &label);

        // reuse offer: guided mode only, asked once, names and summaries only
        if !o.auto && !known {
            let offers = find_reuse_offers(&self.store, &fp, 0.6);
            if !offers.is_empty() {
                for (from, ids) in io.offer_reuse(&offers).await {
                    if let Some(offer) = offers.iter().find(|x| x.from_project == from) {
                        if !ids.is_empty() {
                            let n = accept_reuse(&self.store, &fp, offer, &ids);
                            io.notice(NoticeKind::Skill, &format!("reused {n} skill(s) from {}", offer.from_label));
                        }
                    }
                }
            }
        }

        // skill gate: deterministic, no model call
        let user = load_user_config(&self.cwd);
        let mut g = gate(&self.store, &fp, task, &user);
        timings.gate_ms = g.ms;
        if !self.cfg.embedding_model.is_empty() {
            let te = Instant::now();
            for n in crate::skills::embed::rerank(&self.store, &self.cfg, &self.env, &fp, task, &mut g).await {
                io.notice(NoticeKind::Skill, &format!("added skill \"{n}\" by embedding similarity"));
            }
            timings.gate_ms += te.elapsed().as_secs_f64() * 1000.0;
        }
        for s in &g.selected {
            io.notice(NoticeKind::Skill, &format!("using {} skill \"{}\"", s.scope.as_str(), s.name));
        }

        // UI style order: the project's own design system or patterns first, the HIG-based default otherwise
        if is_ui_task(task) {
            let hig = "hig-ui-baseline";
            match detect_design(&self.cwd) {
                DesignSignal::Strong(name) => {
                    g.selected.retain(|s| s.name != hig);
                    let exists = self.store.user_skills(Some(&fp.project_id)).iter().any(|s| s.name == "Design system" && s.project_id.as_deref() == Some(fp.project_id.as_str()));
                    if !exists {
                        if let Ok(id) = self.store.add(NewSkill { name: "Design system".into(), scope: Scope::Project, stack: None, versions: None, project_id: Some(fp.project_id.clone()), source: "auto".into(), origin: None, summary: format!("UI follows {name}"), keywords: "ui design system components theme style".into(), body: design_note(&name) }, "detected design system") {
                            if let Some(sk) = self.store.get(&id) {
                                g.selected.push(sk);
                            }
                        }
                    }
                    io.notice(NoticeKind::Skill, &format!("UI style: following the project's design system ({name})"));
                }
                DesignSignal::Weak => {
                    let use_default = if o.auto {
                        false
                    } else {
                        let a = io.ask_questions(vec!["This project has UI code but no explicit design system. Follow its existing patterns (Enter), or use the HIG-based default? (type 'default' for HIG)".to_string()]).await;
                        a.first().map(|x| { let l = x.to_lowercase(); l.contains("default") || l.contains("hig") || l.trim() == "2" }).unwrap_or(false)
                    };
                    if use_default {
                        if let Some(h) = self.store.builtins.iter().find(|s| s.name == hig) {
                            if !g.selected.iter().any(|s| s.name == hig) {
                                g.selected.push(h.clone());
                            }
                        }
                        io.notice(NoticeKind::Skill, "UI style: using the HIG-based default");
                    } else {
                        g.selected.retain(|s| s.name != hig);
                        io.notice(NoticeKind::Skill, "UI style: following the project's existing UI patterns");
                    }
                }
                DesignSignal::None => {
                    if let Some(h) = self.store.builtins.iter().find(|s| s.name == hig) {
                        if !g.selected.iter().any(|s| s.name == hig) {
                            g.selected.push(h.clone());
                        }
                    }
                    io.notice(NoticeKind::Skill, "UI style: new project, using the HIG-based default");
                }
            }
        }

        // intake funnel
        io.notice(NoticeKind::Phase, if o.auto { "planning (autonomous)" } else { "inspecting repository and planning" });
        let ti = Instant::now();
        let repo = inspect_repo(&self.cwd, &fp).await;
        macro_rules! ia {
            ($t:expr, $auto:expr, $ans:expr) => {
                intake_args(&self.llm, $t, &repo, $auto, $ans, g.ambiguous.clone(), self.cfg.max_concurrency, o.cancel.clone())
            };
        }
        let resumed = o.resume.clone();
        let mut plan: Intake = if let Some(r) = &resumed {
            io.notice(NoticeKind::Phase, "resuming the interrupted task with its stored plan");
            r.plan.clone()
        } else {
            match intake(ia!(task, o.auto, vec![])).await {
                Ok(p) => p,
                Err(e) => {
                    if e.aborted || o.cancel.as_ref().map(|c| c.is_cancelled()).unwrap_or(false) {
                        finish!(TaskResult::new("aborted", "cancelled"));
                    }
                    finish!(TaskResult::new("error", &format!("planning failed: {e}")));
                }
            }
        };
        if resumed.is_none() && !o.auto && !plan.questions.is_empty() {
            let qs = plan.questions.clone();
            let answers = io.ask_questions(qs.clone()).await;
            let pairs: Vec<(String, String)> = qs.iter().enumerate().map(|(i, q)| (q.clone(), answers.get(i).cloned().unwrap_or_default())).collect();
            match intake(ia!(task, false, pairs)).await {
                Ok(p) => plan = p,
                Err(e) => finish!(TaskResult::new("error", &format!("planning failed: {e}"))),
            }
        }
        if resumed.is_none() && !o.auto {
            let ap = io.approve_plan(&render_plan(&plan), plan.trivial).await;
            if !ap.ok {
                match ap.feedback {
                    Some(fb) => match intake(ia!(&format!("{task}\n\nUser feedback on the plan: {fb}"), true, vec![])).await {
                        Ok(p) => plan = p,
                        Err(e) => finish!(TaskResult::new("error", &format!("planning failed: {e}"))),
                    },
                    None => {
                        let mut r = TaskResult::new("aborted", "plan rejected");
                        r.plan = Some(plan);
                        r.skills_used = g.selected.iter().map(|s| s.name.clone()).collect();
                        finish!(r);
                    }
                }
            }
        }
        timings.intake_ms = ti.elapsed().as_millis() as u64;
        if !g.ambiguous.is_empty() {
            if let Some(choice) = &plan.skill_choice {
                let drop: Vec<String> = g.ambiguous.iter().filter(|n| *n != choice).cloned().collect();
                g.selected.retain(|s| !drop.contains(&s.name));
            }
        }
        let skills_used: Vec<String> = g.selected.iter().map(|s| s.name.clone()).collect();
        if o.plan_only {
            let mut r = TaskResult::new("planned", "plan only");
            r.plan = Some(plan);
            r.skills_used = skills_used;
            finish!(r);
        }

        // work
        let cp = Checkpoints::new(self.cwd.clone());
        slog.append(json!({"t": "plan", "plan": plan}));
        let base_cp = match &resumed {
            Some(r) => {
                if !cp.exists(&r.base).await {
                    finish!(TaskResult::new("error", "the stored checkpoint no longer exists; start a new task"));
                }
                r.base.clone()
            }
            None => cp.create("before task").await.unwrap_or_default(),
        };
        slog.append(json!({"t": "checkpoint", "id": base_cp}));
        let (wrap, backend) = if o.sandbox { sandbox_for(&self.cwd, true) } else { (None, "none") };
        if o.sandbox {
            io.notice(NoticeKind::Info, &format!("shell sandbox: {backend}"));
        }
        let cwd_s = self.cwd.to_string_lossy().to_string();
        let skills_txt = render_skills(&g);
        let user_txt = render_user(&user, &g);
        let plan_txt = render_plan(&plan);
        let context = context_block(&ContextParts { cwd: &cwd_s, fingerprint: Some(&fp.summary), skills: Some(&skills_txt), user_rules: Some(&user_txt), plan: Some(&plan_txt) });
        let acc = if plan.acceptance.is_empty() { "- the task is complete and existing tests still pass".to_string() } else { plan.acceptance.iter().map(|a| format!("- {a}")).collect::<Vec<_>>().join("\n") };
        let task_text = format!("{}\n\nAcceptance criteria:\n{}", plan.enriched, acc);
        let tw = Instant::now();
        let mvfn: MetricsFn = {
            let (cfg, env) = (self.cfg.clone(), self.env.clone());
            Arc::new(move || {
                let (cfg, env) = (cfg.clone(), env.clone());
                Box::pin(async move { fetch_metrics(&cfg, &env).await })
            })
        };
        let gov = Governor::new(self.cfg.max_concurrency, Some(mvfn), 2);
        let multi = plan.subtasks.len() >= 2 && self.cfg.max_concurrency > 1;
        io.notice(NoticeKind::Phase, &if multi { format!("working ({} workers, file-ownership partitioned)", plan.subtasks.len()) } else { "working".to_string() });

        let mut history: Option<Vec<Message>> = None;
        let mut agent: Option<AgentResult> = None;
        let mut agent_sum: Option<AgentSummary> = None;
        let mut workers: Vec<WorkerResult> = vec![];
        if multi {
            let io2 = io.clone();
            let mo = MasterOptions {
                llm: self.llm.clone(),
                cwd: self.cwd.clone(),
                mode: o.approval,
                context: context.clone(),
                governor: gov.clone(),
                cancel: o.cancel.clone(),
                context_window: self.cfg.context_window,
                max_steps: 25,
                notice: Some(Arc::new(move |m| io2.notice(NoticeKind::Info, m))),
                wrap_shell: wrap.clone(),
                confirm: self.agent_options(&o, &context, &wrap).confirm,
                extra_tools: self.mcp.get().cloned().unwrap_or_default(),
                task_budget: self.cfg.max_task_tokens,
                llm_compaction: self.cfg.llm_compaction,
            };
            workers = run_workers(&plan.enriched, &plan.subtasks, &mo).await;
        } else {
            let mut ao = self.agent_options(&o, &context, &wrap);
            let mut first_msg = task_text.clone();
            if let Some(r) = &resumed {
                if let Some(h) = &r.history {
                    ao.history = Some(h.clone());
                    first_msg = "The previous run was interrupted. The working tree holds the work done so far. Inspect it, then continue and finish the task.".to_string();
                } else {
                    first_msg = format!("{task_text}\n\nA previous attempt was interrupted; the working tree may already hold part of the work. Inspect it (git status, git diff) and continue.");
                }
            }
            let r = run_agent(&first_msg, ao).await;
            history = Some(r.messages.clone());
            if r.stopped == crate::agent::Stopped::Error {
                let mut res = TaskResult::new("error", r.error.as_deref().unwrap_or("agent error"));
                res.plan = Some(plan);
                res.skills_used = skills_used;
                res.agent = Some(AgentSummary { steps: r.steps, tool_calls: r.tool_calls, failed_tools: r.failed_tools, stopped: r.stopped.as_str().into() });
                finish!(res);
            }
            agent_sum = Some(AgentSummary { steps: r.steps, tool_calls: r.tool_calls, failed_tools: r.failed_tools, stopped: r.stopped.as_str().into() });
            agent = Some(r);
        }
        timings.work_ms = tw.elapsed().as_millis() as u64;
        if o.cancel.as_ref().map(|c| c.is_cancelled()).unwrap_or(false) {
            if !base_cp.is_empty() && !o.keep_on_fail {
                cp.restore(&base_cp).await;
            }
            let mut r = TaskResult::new("aborted", "cancelled");
            r.rolled_back = !base_cp.is_empty();
            r.plan = Some(plan);
            r.skills_used = skills_used;
            finish!(r);
        }

        // verification rounds: deterministic checks decide
        let max_rounds = if o.auto { self.cfg.verify_rounds_auto } else { self.cfg.verify_rounds_normal };
        let tv = Instant::now();
        let subtasks = plan.subtasks.clone();
        let allowed = allowed_from_plan(&plan.plan, &subtasks);
        let ctx = VerifyCtx { cwd: &self.cwd, cp: &cp, base: &base_cp, fp: &fp, llm: Some(&self.llm), acceptance: &plan.acceptance, allowed_globs: allowed, max_rounds, cancel: o.cancel.clone() };
        let mut vstate = VerifyState::default();
        let mut budget_hit = false;
        for round in 1..=max_rounds {
            if o.cancel.as_ref().map(|c| c.is_cancelled()).unwrap_or(false) {
                break;
            }
            let r: RoundReport = run_round(&ctx, &mut vstate, round).await;
            let failed: Vec<String> = r.checks.iter().filter(|c| c.status == crate::verify::checks::Status::Fail).map(|c| c.name.clone()).collect();
            io.notice(NoticeKind::Verify, &format!("round {round}: {}{}", r.verdict.as_str(), if failed.is_empty() { String::new() } else { format!(" (failed: {})", failed.join(", ")) }));
            if r.verdict != Verdict::Fail || round == max_rounds {
                break;
            }
            let used_tokens = self.llm.stats().prompt_tokens + self.llm.stats().completion_tokens;
            if self.cfg.max_task_tokens > 0 && used_tokens >= self.cfg.max_task_tokens {
                io.notice(NoticeKind::Warn, &format!("token budget reached ({used_tokens} of {}); stopping verification rounds", self.cfg.max_task_tokens));
                budget_hit = true;
                break;
            }
            let feedback = feedback_from(&r);
            io.notice(NoticeKind::Phase, "fixing verification failures");
            if multi {
                let files: Vec<String> = regex::Regex::new(r"([\w./-]+\.[A-Za-z0-9]+):\d+").unwrap().captures_iter(&feedback).map(|m| m[1].to_string()).collect();
                let (owned, unowned) = owners_of(&subtasks, &files);
                let mut jobs = Vec::new();
                for (id, fs) in owned {
                    let s = subtasks.iter().find(|x| x.id == id).unwrap().clone();
                    let mut ao = self.agent_options(&o, &context, &wrap);
                    ao.owned_globs = Some(s.files.clone());
                    ao.max_steps = 15;
                    let gov = gov.clone();
                    let msg = format!("{feedback}\n\nFix only problems in: {}", fs.join(", "));
                    jobs.push(tokio::spawn(async move { gov.run(run_agent(&msg, ao)).await.steps }));
                }
                if !unowned.is_empty() {
                    // failures in files nobody owns: one extra worker per directory, limited to those directories
                    let mut groups: std::collections::BTreeMap<String, Vec<String>> = std::collections::BTreeMap::new();
                    for f in &unowned {
                        groups.entry(f.rsplit_once('/').map(|(d, _)| d.to_string()).unwrap_or_default()).or_default().push(f.clone());
                    }
                    for (dir, fs) in groups {
                        let mut ao = self.agent_options(&o, &context, &wrap);
                        ao.owned_globs = if dir.is_empty() { None } else { Some(vec![format!("{dir}/**")]) };
                        ao.max_steps = 15;
                        let gov = gov.clone();
                        let msg = format!("{feedback}\n\nFix only problems in: {}", fs.join(", "));
                        jobs.push(tokio::spawn(async move { gov.run(run_agent(&msg, ao)).await.steps }));
                    }
                }
                if jobs.is_empty() {
                    // nothing attributable to a file: a single unrestricted fixer
                    let mut ao = self.agent_options(&o, &context, &wrap);
                    ao.max_steps = 15;
                    let gov = gov.clone();
                    let msg = feedback.clone();
                    jobs.push(tokio::spawn(async move { gov.run(run_agent(&msg, ao)).await.steps }));
                }
                join_all(jobs).await;
            } else {
                let mut ao = self.agent_options(&o, &context, &wrap);
                ao.history = history.clone();
                ao.max_steps = 20;
                let r2 = run_agent(&feedback, ao).await;
                history = Some(r2.messages.clone());
                if let Some(s) = agent_sum.as_mut() {
                    s.steps += r2.steps;
                    s.tool_calls += r2.tool_calls;
                    s.failed_tools += r2.failed_tools;
                }
                if let Some(a) = agent.as_mut() {
                    a.final_text = if r2.final_text.is_empty() { a.final_text.clone() } else { r2.final_text.clone() };
                }
            }
        }
        let mut report: VerifyReport = finalize(vstate);
        if budget_hit && report.verdict == Verdict::Fail {
            report.reason = format!("token budget exhausted after {} round(s); {}", report.rounds.len(), report.reason);
        }
        timings.verify_ms = tv.elapsed().as_millis() as u64;
        let ch: Changes = if base_cp.is_empty() { Changes::default() } else { cp.changed_since(&base_cp).await };
        let changed_all = ch.all();

        // the verdict gates the outcome: fail -> roll back, keep the patch for inspection
        let task_diff: String = if !changed_all.is_empty() && !base_cp.is_empty() { cp.diff_since(&base_cp).await.chars().take(200_000).collect() } else { String::new() };
        let (mut rolled_back, mut rejected_patch) = (false, None);
        if report.verdict == Verdict::Fail && !base_cp.is_empty() && !o.keep_on_fail && !changed_all.is_empty() {
            cp.restore(&base_cp).await;
            rolled_back = true;
            let dir = self.cwd.join(".fh").join("rejected");
            let _ = std::fs::create_dir_all(&dir);
            let name = format!("{}.patch", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0));
            if std::fs::write(dir.join(&name), &task_diff).is_ok() {
                rejected_patch = Some(format!(".fh/rejected/{name}"));
            }
        } else if matches!(report.verdict, Verdict::Pass | Verdict::Unverified) && o.commit && !changed_all.is_empty() {
            let msg: String = plan.enriched.lines().next().unwrap_or("").chars().take(60).collect();
            run_simple(&format!("git add -A && git commit -q -m {:?}", format!("fh: {msg}")), &self.cwd, 30_000).await;
        }

        // the final answer is produced only after the verdict
        let worker_summary = workers.iter().map(|w| format!("{}: {}", w.id, w.summary)).collect::<Vec<_>>().join("\n");
        let agent_final = agent.as_ref().map(|a| a.final_text.clone()).filter(|s| !s.is_empty()).unwrap_or(worker_summary);
        let banner = match report.verdict {
            Verdict::Pass => "Verified".to_string(),
            Verdict::Unverified => "UNVERIFIED (no build/test command available)".to_string(),
            Verdict::Fail => format!("NOT delivered: verification failed{}", if rolled_back { "; changes rolled back" } else { "" }),
        };
        let final_text = format!("{banner}\n\n{agent_final}").trim().to_string();

        // post-delivery: outcome tracking, quality police, skill learning (off the critical path)
        let used: Vec<Skill> = g.selected.clone();
        let task_id = format!("{}-{}", fp.project_id, std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0));
        self.store.record_task(&task_id, &fp.project_id, &used, report.verdict.as_str(), report.rounds.len());
        for s in &used {
            self.store.log("used", &s.name, &format!("task {}", report.verdict.as_str()), Some(&fp.project_id));
        }
        self.store.record_verify_stats(&task_id, (report.reviewer.raised, report.reviewer.valid, report.reviewer.dropped, report.reviewer.blockers), report.rounds.len(), report.verdict.as_str(), self.llm.stats().prompt_tokens + self.llm.stats().completion_tokens, t0.elapsed().as_millis() as u64);
        if !o.no_mine {
            let (store, llm, io2, fp2) = (self.store.clone(), self.llm.clone(), io.clone(), fp.clone());
            let (task_s, changed2, verdict) = (task.to_string(), changed_all.clone(), report.verdict.as_str().to_string());
            let diff2 = if report.verdict == Verdict::Pass { task_diff.chars().take(6000).collect::<String>() } else { String::new() };
            let rounds_used = report.rounds.len();
            let used2 = used.clone();
            let cwd2 = self.cwd.clone();
            let h = tokio::spawn(async move {
                evaluate(&store, POLICE_DEFAULTS);
                promote_eligible(&store, POLICE_DEFAULTS.promote_n);
                if verdict == "pass" && rounds_used > 1 {
                    // IMPROVE: a used skill did not prevent rework
                    for sk in used2.iter().filter(|s| s.source != "builtin") {
                        if let Some(n) = improve_used(&store, &llm, sk, MineInput { task: &task_s, diff: &diff2, changed: &changed2, fp: &fp2, verdict: &verdict }, None).await {
                            io2.notice(NoticeKind::Skill, &format!("improved project skill: {n}"));
                            break;
                        }
                    }
                }
                if verdict == "pass" {
                    // thin, frequently used project skills are enriched from the repository (read-only, evidence required)
                    if let Some(sk) = crate::skills::research::find_thin(&store, &fp2.project_id).into_iter().next() {
                        if let crate::skills::research::Outcome::Enriched(n) = crate::skills::research::research_skill(&store, &llm, &cwd2, &sk, None).await {
                            io2.notice(NoticeKind::Skill, &format!("enriched project skill from repository docs: {n}"));
                        }
                    }
                    if let MineOutcome::Created(n) = mine(&store, &llm, MineInput { task: &task_s, diff: &diff2, changed: &changed2, fp: &fp2, verdict: &verdict }, None).await {
                        io2.notice(NoticeKind::Skill, &format!("learned a new project skill: {n}"));
                    }
                }
            });
            self.pending.lock().unwrap().push(h);
        }

        let mut res = TaskResult::new(report.verdict.as_str(), &report.reason);
        res.final_text = final_text;
        res.changed = changed_all;
        res.rounds = report.rounds.len();
        res.rolled_back = rolled_back;
        res.rejected_patch = rejected_patch;
        res.plan = Some(plan);
        res.skills_used = skills_used;
        res.gate = Some((g.ms, g.llm_calls, g.decision.to_string()));
        res.reviewer = Some(report.reviewer.clone());
        res.verify = Some(report);
        res.workers = workers;
        res.agent = agent_sum;
        res.diff = task_diff;
        finish!(res);
    }
}

#[allow(clippy::too_many_arguments)]
fn intake_args<'a>(llm: &'a LlmClient, task: &'a str, repo: &'a str, auto: bool, answers: Vec<(String, String)>, ambiguous: Vec<String>, max_subtasks: usize, cancel: Option<CancellationToken>) -> IntakeArgs<'a> {
    IntakeArgs { llm, task, repo, auto, answers, ambiguous, max_subtasks, cancel }
}

/// Path of the working tree used by callers that need to show relative names.
pub fn workspace_label(p: &Path) -> String {
    p.file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_default()
}
