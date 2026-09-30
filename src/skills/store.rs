use super::frontmatter::{parse_frontmatter, version_matches};
use super::validate::validate_skill_body;
use crate::config::{data_dir, Env};
use crate::fingerprint::Fingerprint;
use anyhow::Result;
use rusqlite::{params, Connection};
use sha2::{Digest, Sha256};
use std::path::Path;
use std::sync::{Arc, Mutex, MutexGuard};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scope {
    Project,
    Stack,
    Global,
}

impl Scope {
    pub fn as_str(&self) -> &'static str {
        match self {
            Scope::Project => "project",
            Scope::Stack => "stack",
            Scope::Global => "global",
        }
    }
    pub fn parse(s: &str) -> Scope {
        match s {
            "project" => Scope::Project,
            "stack" => Scope::Stack,
            _ => Scope::Global,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Skill {
    pub id: String,
    pub name: String,
    pub scope: Scope,
    /// stack id (react) or token (react@18) for stack scope
    pub stack: Option<String>,
    pub versions: Option<String>,
    pub project_id: Option<String>,
    /// builtin | auto | reused
    pub source: String,
    pub origin: Option<String>,
    pub summary: String,
    pub keywords: String,
    pub body: String,
    pub version: i64,
    pub hash: String,
    /// active | quarantined
    pub state: String,
}

#[derive(Clone, Debug)]
pub struct NewSkill {
    pub name: String,
    pub scope: Scope,
    pub stack: Option<String>,
    pub versions: Option<String>,
    pub project_id: Option<String>,
    pub source: String,
    pub origin: Option<String>,
    pub summary: String,
    pub keywords: String,
    pub body: String,
}

#[derive(Clone, Debug)]
pub struct Activity {
    pub ts: i64,
    pub kind: String,
    pub skill: String,
    pub reason: String,
}

fn h(s: &str) -> String {
    let mut d = Sha256::new();
    d.update(s.as_bytes());
    format!("{:x}", d.finalize()).chars().take(16).collect()
}

fn now_ms() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

const BUILTIN_FILES: [&str; 18] = [
    include_str!("builtin/global-backend.md"),
    include_str!("builtin/global-frontend.md"),
    include_str!("builtin/global-sql.md"),
    include_str!("builtin/global-security.md"),
    include_str!("builtin/global-devops.md"),
    include_str!("builtin/global-testing.md"),
    include_str!("builtin/global-architecture.md"),
    include_str!("builtin/global-performance.md"),
    include_str!("builtin/global-hig-ui.md"),
    include_str!("builtin/stack-dotnet-framework-48.md"),
    include_str!("builtin/stack-dotnet-8-10.md"),
    include_str!("builtin/stack-react-18-19.md"),
    include_str!("builtin/stack-angular-17plus.md"),
    include_str!("builtin/stack-angularjs-1x.md"),
    include_str!("builtin/stack-vue-3.md"),
    include_str!("builtin/stack-vue-2-legacy.md"),
    include_str!("builtin/stack-spring-boot.md"),
    include_str!("builtin/stack-django.md"),
];

/// The immutable layer shipped inside the binary.
pub fn load_builtins() -> Vec<Skill> {
    BUILTIN_FILES
        .iter()
        .filter_map(|raw| {
            let (meta, body) = parse_frontmatter(raw);
            let name = meta.get("name")?.clone();
            Some(Skill {
                id: format!("builtin:{name}"),
                name,
                scope: Scope::parse(meta.get("scope").map(|s| s.as_str()).unwrap_or("global")),
                stack: meta.get("stack").cloned(),
                versions: meta.get("versions").cloned(),
                project_id: None,
                source: "builtin".into(),
                origin: None,
                summary: meta.get("summary").cloned().unwrap_or_default(),
                keywords: meta.get("keywords").cloned().unwrap_or_default(),
                hash: h(&body),
                body,
                version: 1,
                state: "active".into(),
            })
        })
        .collect()
}

pub fn skill_applies(s: &Skill, fp: &Fingerprint) -> bool {
    if s.state != "active" {
        return false;
    }
    match s.scope {
        Scope::Global => true,
        Scope::Project => s.project_id.as_deref() == Some(fp.project_id.as_str()),
        Scope::Stack => {
            let st = s.stack.clone().unwrap_or_default();
            let (id, ver) = match st.split_once('@') {
                Some((i, v)) => (i.to_string(), Some(v.to_string())),
                None => (st.clone(), None),
            };
            fp.stacks.iter().any(|x| x.id == id && match &ver { Some(v) => x.version.as_deref() == Some(v.as_str()), None => version_matches(s.versions.as_deref(), x.version.as_deref()) })
        }
    }
}

pub fn simple_diff(a: &str, b: &str) -> String {
    let (la, lb): (Vec<&str>, Vec<&str>) = (a.split('\n').collect(), b.split('\n').collect());
    let mut out: Vec<String> = la.iter().filter(|l| !lb.contains(l)).map(|l| format!("- {l}")).collect();
    out.extend(lb.iter().filter(|l| !la.contains(l)).map(|l| format!("+ {l}")));
    out.join("\n")
}

/// Per-user skill store (SQLite in the OS user data dir) plus the immutable built-in layer. Cheap to clone.
#[derive(Clone)]
pub struct SkillStore {
    conn: Arc<Mutex<Connection>>,
    pub builtins: Arc<Vec<Skill>>,
}

impl SkillStore {
    pub fn open(env: &Env) -> Result<Self> {
        let dir = data_dir(env);
        std::fs::create_dir_all(&dir)?;
        Self::open_path(&dir.join("skills.db"))
    }

    pub fn open_path(p: &Path) -> Result<Self> {
        Self::init(Connection::open(p)?)
    }

    pub fn in_memory() -> Self {
        Self::init(Connection::open_in_memory().unwrap()).unwrap()
    }

    fn init(conn: Connection) -> Result<Self> {
        // several fh processes may share one store (parallel tasks); wait for the lock instead of failing
        conn.busy_timeout(std::time::Duration::from_secs(30))?;
        conn.execute_batch(
            "PRAGMA journal_mode=WAL;
             CREATE TABLE IF NOT EXISTS skills(id TEXT PRIMARY KEY, name TEXT, scope TEXT, stack TEXT, versions TEXT, project_id TEXT, source TEXT, origin TEXT, summary TEXT, keywords TEXT, body TEXT, version INTEGER, hash TEXT, state TEXT, created INTEGER, updated INTEGER);
             CREATE TABLE IF NOT EXISTS skill_versions(skill_id TEXT, version INTEGER, hash TEXT, body TEXT, diff TEXT, reason TEXT, ts INTEGER);
             CREATE TABLE IF NOT EXISTS projects(id TEXT PRIMARY KEY, label TEXT, stack_key TEXT, tokens TEXT, first_seen INTEGER, last_seen INTEGER);
             CREATE TABLE IF NOT EXISTS tasks(id TEXT PRIMARY KEY, project_id TEXT, verdict TEXT, rounds INTEGER, ts INTEGER);
             CREATE TABLE IF NOT EXISTS task_skills(task_id TEXT, skill_id TEXT, skill_version INTEGER);
             CREATE TABLE IF NOT EXISTS activity(ts INTEGER, kind TEXT, skill TEXT, reason TEXT, project_id TEXT);
             CREATE TABLE IF NOT EXISTS verify_stats(task_id TEXT PRIMARY KEY, raised INTEGER, valid INTEGER, dropped INTEGER, blockers INTEGER, rounds INTEGER, verdict TEXT, tokens INTEGER, ms INTEGER, ts INTEGER);
             CREATE TABLE IF NOT EXISTS suppressed(skill_id TEXT PRIMARY KEY, reason TEXT, ts INTEGER);",
        )?;
        Ok(SkillStore { conn: Arc::new(Mutex::new(conn)), builtins: Arc::new(load_builtins()) })
    }

    /// Direct access for the quality police and the miner (never hold across an await).
    pub fn conn(&self) -> MutexGuard<'_, Connection> {
        self.conn.lock().unwrap()
    }

    // ---- activity log (shown as notices; never as skill content)
    pub fn log(&self, kind: &str, skill: &str, reason: &str, project_id: Option<&str>) {
        let _ = self.conn().execute("INSERT INTO activity VALUES (?,?,?,?,?)", params![now_ms(), kind, skill, reason, project_id]);
    }

    pub fn activity(&self, limit: usize) -> Vec<Activity> {
        let c = self.conn();
        let mut st = c.prepare("SELECT ts,kind,skill,reason FROM activity ORDER BY ts DESC, rowid DESC LIMIT ?").unwrap();
        st.query_map([limit as i64], |r| Ok(Activity { ts: r.get(0)?, kind: r.get(1)?, skill: r.get(2)?, reason: r.get(3)? })).unwrap().flatten().collect()
    }

    // ---- projects
    pub fn register_project(&self, fp: &Fingerprint, label: &str) {
        let now = now_ms();
        let _ = self.conn().execute(
            "INSERT INTO projects VALUES (?,?,?,?,?,?) ON CONFLICT(id) DO UPDATE SET stack_key=excluded.stack_key,tokens=excluded.tokens,last_seen=excluded.last_seen,label=excluded.label",
            params![fp.project_id, label, fp.stack_key, serde_json::to_string(&fp.tokens).unwrap_or_default(), now, now],
        );
    }

    pub fn is_known_project(&self, id: &str) -> bool {
        self.conn().query_row("SELECT 1 FROM projects WHERE id=?", [id], |_| Ok(())).is_ok()
    }

    pub fn other_projects(&self, exclude: &str) -> Vec<(String, String, Vec<String>)> {
        let c = self.conn();
        let mut st = c.prepare("SELECT id,label,tokens FROM projects WHERE id<>?").unwrap();
        st.query_map([exclude], |r| {
            let toks: String = r.get(2)?;
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, serde_json::from_str::<Vec<String>>(&toks).unwrap_or_default()))
        })
        .unwrap()
        .flatten()
        .collect()
    }

    pub fn project_tokens(&self, id: &str) -> Option<Vec<String>> {
        let t: String = self.conn().query_row("SELECT tokens FROM projects WHERE id=?", [id], |r| r.get(0)).ok()?;
        serde_json::from_str(&t).ok()
    }

    // ---- skills
    fn row(r: &rusqlite::Row) -> rusqlite::Result<Skill> {
        Ok(Skill {
            id: r.get("id")?,
            name: r.get("name")?,
            scope: Scope::parse(&r.get::<_, String>("scope")?),
            stack: r.get("stack")?,
            versions: r.get("versions")?,
            project_id: r.get("project_id")?,
            source: r.get("source")?,
            origin: r.get("origin")?,
            summary: r.get("summary")?,
            keywords: r.get("keywords")?,
            body: r.get("body")?,
            version: r.get("version")?,
            hash: r.get("hash")?,
            state: r.get("state")?,
        })
    }

    pub fn get(&self, id: &str) -> Option<Skill> {
        if id.starts_with("builtin:") {
            return self.builtins.iter().find(|b| b.id == id).cloned();
        }
        self.conn().query_row("SELECT * FROM skills WHERE id=?", [id], Self::row).ok()
    }

    /// All user-store skills; when `project_id` is given, project-scoped skills of other projects are excluded.
    pub fn user_skills(&self, project_id: Option<&str>) -> Vec<Skill> {
        let all: Vec<Skill> = {
            let c = self.conn();
            let mut st = c.prepare("SELECT * FROM skills").unwrap();
            st.query_map([], Self::row).unwrap().flatten().collect()
        };
        match project_id {
            Some(p) => all.into_iter().filter(|s| s.scope != Scope::Project || s.project_id.as_deref() == Some(p)).collect(),
            None => all,
        }
    }

    pub fn all_for_project(&self, fp: &Fingerprint) -> Vec<Skill> {
        let sup: Vec<String> = {
            let c = self.conn();
            let mut st = c.prepare("SELECT skill_id FROM suppressed").unwrap();
            st.query_map([], |r| r.get(0)).unwrap().flatten().collect()
        };
        self.builtins.iter().cloned().chain(self.user_skills(Some(&fp.project_id))).filter(|s| !sup.contains(&s.id) && skill_applies(s, fp)).collect()
    }

    pub fn add(&self, s: NewSkill, reason: &str) -> Result<String, String> {
        validate_skill_body(&s.body, &s.summary, &s.name)?;
        if self.user_skills(None).iter().any(|x| x.name.to_lowercase() == s.name.to_lowercase() && x.project_id == s.project_id && x.scope == s.scope) {
            return Err("duplicate name".into());
        }
        let id = format!("u:{}", &format!("{:016x}", rand::random::<u64>())[..8]);
        let (now, hash) = (now_ms(), h(&s.body));
        {
            let c = self.conn();
            c.execute(
                "INSERT INTO skills VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)",
                params![id, s.name, s.scope.as_str(), s.stack, s.versions, s.project_id, s.source, s.origin, s.summary, s.keywords, s.body, 1, hash, "active", now, now],
            )
            .map_err(|e| e.to_string())?;
            c.execute("INSERT INTO skill_versions VALUES (?,?,?,?,?,?,?)", params![id, 1, hash, s.body, "", reason, now]).map_err(|e| e.to_string())?;
        }
        self.log(if s.source == "reused" { "reused" } else { "created" }, &s.name, reason, s.project_id.as_deref());
        Ok(id)
    }

    pub fn improve(&self, id: &str, body: &str, reason: &str) -> Result<(), String> {
        let cur = self.get(id).filter(|s| s.source != "builtin").ok_or("not found or immutable")?;
        validate_skill_body(body, &cur.summary, &cur.name)?;
        if h(body) == cur.hash {
            return Err("no change".into());
        }
        let (ver, now) = (cur.version + 1, now_ms());
        {
            let c = self.conn();
            c.execute("UPDATE skills SET body=?,hash=?,version=?,updated=? WHERE id=?", params![body, h(body), ver, now, id]).map_err(|e| e.to_string())?;
            c.execute("INSERT INTO skill_versions VALUES (?,?,?,?,?,?,?)", params![id, ver, h(body), body, simple_diff(&cur.body, body), reason, now]).map_err(|e| e.to_string())?;
        }
        self.log("improved", &cur.name, reason, cur.project_id.as_deref());
        Ok(())
    }

    /// (version, hash, diff, reason)
    pub fn versions(&self, id: &str) -> Vec<(i64, String, String, String)> {
        let c = self.conn();
        let mut st = c.prepare("SELECT version,hash,diff,reason FROM skill_versions WHERE skill_id=? ORDER BY version").unwrap();
        st.query_map([id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))).unwrap().flatten().collect()
    }

    pub fn quarantine(&self, id: &str, reason: &str) {
        let Some(s) = self.get(id) else { return };
        if s.source == "builtin" {
            let _ = self.conn().execute("INSERT OR REPLACE INTO suppressed VALUES (?,?,?)", params![id, reason, now_ms()]);
        } else {
            let _ = self.conn().execute("UPDATE skills SET state='quarantined' WHERE id=?", [id]);
        }
        self.log("quarantined", &s.name, reason, s.project_id.as_deref());
    }

    pub fn rollback(&self, id: &str, reason: &str) -> bool {
        let Some(s) = self.get(id) else { return false };
        if s.source == "builtin" || s.version < 2 {
            return false;
        }
        let prev: Option<String> = self.conn().query_row("SELECT body FROM skill_versions WHERE skill_id=? AND version=?", params![id, s.version - 1], |r| r.get(0)).ok();
        let Some(prev) = prev else { return false };
        let (ver, now) = (s.version + 1, now_ms());
        {
            let c = self.conn();
            let _ = c.execute("UPDATE skills SET body=?,hash=?,version=?,updated=? WHERE id=?", params![prev, h(&prev), ver, now, id]);
            let _ = c.execute("INSERT INTO skill_versions VALUES (?,?,?,?,?,?,?)", params![id, ver, h(&prev), prev, simple_diff(&s.body, &prev), format!("rollback: {reason}"), now]);
        }
        self.log("rolled_back", &s.name, reason, s.project_id.as_deref());
        true
    }

    pub fn promote(&self, id: &str, scope: Scope, stack: Option<&str>, reason: &str) {
        let Some(s) = self.get(id).filter(|s| s.source != "builtin") else { return };
        let _ = self.conn().execute("UPDATE skills SET scope=?,stack=?,project_id=NULL WHERE id=?", params![scope.as_str(), stack, id]);
        self.log("promoted", &s.name, &format!("{} -> {}: {}", s.scope.as_str(), scope.as_str(), reason), s.project_id.as_deref());
    }

    // ---- outcomes
    pub fn record_task(&self, task_id: &str, project_id: &str, skills: &[Skill], verdict: &str, rounds: usize) {
        let c = self.conn();
        let _ = c.execute("INSERT OR REPLACE INTO tasks VALUES (?,?,?,?,?)", params![task_id, project_id, verdict, rounds as i64, now_ms()]);
        for s in skills {
            let _ = c.execute("INSERT INTO task_skills VALUES (?,?,?)", params![task_id, s.id, s.version]);
        }
    }
}

/// Aggregate runtime statistics shown by `fh stats` (the verifier's own quality signals).
#[derive(Clone, Debug, Default)]
pub struct RunStats {
    pub tasks: i64,
    pub pass: i64,
    pub fail: i64,
    pub unverified: i64,
    pub avg_rounds: f64,
    pub reviewer_raised: i64,
    pub reviewer_dropped: i64,
    pub reviewer_blockers: i64,
    pub avg_tokens: f64,
}

impl SkillStore {
    pub fn record_verify_stats(&self, task_id: &str, s: (usize, usize, usize, usize), rounds: usize, verdict: &str, tokens: u64, ms: u64) {
        let _ = self.conn().execute(
            "INSERT OR REPLACE INTO verify_stats VALUES (?,?,?,?,?,?,?,?,?,?)",
            params![task_id, s.0 as i64, s.1 as i64, s.2 as i64, s.3 as i64, rounds as i64, verdict, tokens as i64, ms as i64, now_ms()],
        );
    }

    pub fn run_stats(&self) -> RunStats {
        let c = self.conn();
        let mut st = RunStats::default();
        let _ = c.query_row(
            "SELECT COUNT(*), COALESCE(SUM(verdict='pass'),0), COALESCE(SUM(verdict='fail'),0), COALESCE(SUM(verdict='unverified'),0), COALESCE(AVG(rounds),0), COALESCE(SUM(raised),0), COALESCE(SUM(dropped),0), COALESCE(SUM(blockers),0), COALESCE(AVG(tokens),0) FROM verify_stats",
            [],
            |r| {
                st = RunStats { tasks: r.get(0)?, pass: r.get(1)?, fail: r.get(2)?, unverified: r.get(3)?, avg_rounds: r.get(4)?, reviewer_raised: r.get(5)?, reviewer_dropped: r.get(6)?, reviewer_blockers: r.get(7)?, avg_tokens: r.get(8)? };
                Ok(())
            },
        );
        st
    }

    /// Version history of skills matching `name` (case-insensitive substring): (skill, version, hash, reason, diff).
    pub fn history(&self, name: &str) -> Vec<(String, i64, String, String, String)> {
        let like = format!("%{}%", name.to_lowercase());
        let c = self.conn();
        let mut st = c.prepare("SELECT s.name, v.version, v.hash, v.reason, v.diff FROM skill_versions v JOIN skills s ON s.id=v.skill_id WHERE lower(s.name) LIKE ? ORDER BY s.name, v.version").unwrap();
        st.query_map([like], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?))).unwrap().flatten().collect()
    }
}
