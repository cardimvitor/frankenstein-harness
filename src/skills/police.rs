use super::store::SkillStore;
use rusqlite::params;

#[derive(Clone, Copy, Debug)]
pub struct PoliceConfig {
    pub min_uses: usize,
    pub max_worse: f64,
    pub promote_n: usize,
}

pub const POLICE_DEFAULTS: PoliceConfig = PoliceConfig { min_uses: 5, max_worse: 0.25, promote_n: 3 };

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PoliceAction {
    None,
    Quarantine,
    Rollback,
}

#[derive(Clone, Debug)]
pub struct SkillHealth {
    pub skill_id: String,
    pub name: String,
    pub used: usize,
    pub fail_used: usize,
    pub base: usize,
    pub fail_base: usize,
    pub action: PoliceAction,
}

fn rate(f: usize, n: usize) -> f64 {
    if n == 0 { 0.0 } else { f as f64 / n as f64 }
}

/// Internal A/B: compare the failure rate of tasks that used a skill with tasks in the same projects that did not.
/// The user has no debug commands, so the harness polices skill quality itself. Worse than baseline by `max_worse`
/// with enough samples: roll back to the previous version if the current version is the culprit, else quarantine.
pub fn evaluate(store: &SkillStore, cfg: PoliceConfig) -> Vec<SkillHealth> {
    let mut out = Vec::new();
    let skills: Vec<_> = store.builtins.iter().cloned().chain(store.user_skills(None)).collect();
    for s in skills {
        if s.state != "active" {
            continue;
        }
        let used: Vec<(String, i64)> = {
            let c = store.conn();
            let mut st = c.prepare("SELECT t.verdict, ts.skill_version FROM task_skills ts JOIN tasks t ON t.id=ts.task_id WHERE ts.skill_id=?").unwrap();
            st.query_map([&s.id], |r| Ok((r.get(0)?, r.get(1)?))).unwrap().flatten().collect()
        };
        if used.len() < cfg.min_uses {
            continue;
        }
        let projects: Vec<String> = {
            let c = store.conn();
            let mut st = c.prepare("SELECT DISTINCT t.project_id FROM task_skills ts JOIN tasks t ON t.id=ts.task_id WHERE ts.skill_id=?").unwrap();
            st.query_map([&s.id], |r| r.get(0)).unwrap().flatten().collect()
        };
        let base: Vec<String> = {
            let c = store.conn();
            let ph = projects.iter().map(|_| "?").collect::<Vec<_>>().join(",");
            let sql = format!("SELECT verdict FROM tasks WHERE project_id IN ({ph}) AND id NOT IN (SELECT task_id FROM task_skills WHERE skill_id=?)");
            let mut st = c.prepare(&sql).unwrap();
            let mut ps: Vec<&dyn rusqlite::ToSql> = projects.iter().map(|p| p as &dyn rusqlite::ToSql).collect();
            ps.push(&s.id);
            st.query_map(ps.as_slice(), |r| r.get(0)).unwrap().flatten().collect()
        };
        let fail_used = used.iter().filter(|u| u.0 == "fail").count();
        let fail_base = base.iter().filter(|b| *b == "fail").count();
        let mut action = PoliceAction::None;
        if base.len() >= cfg.min_uses && rate(fail_used, used.len()) - rate(fail_base, base.len()) >= cfg.max_worse {
            let cur: Vec<_> = used.iter().filter(|u| u.1 == s.version).collect();
            let prev: Vec<_> = used.iter().filter(|u| u.1 == s.version - 1).collect();
            let cur_fail = rate(cur.iter().filter(|u| u.0 == "fail").count(), cur.len());
            let prev_fail = rate(prev.iter().filter(|u| u.0 == "fail").count(), prev.len());
            if s.version > 1 && prev.len() >= 3 && cur.len() >= 3 && cur_fail > prev_fail {
                store.rollback(&s.id, &format!("fail rate {}% vs {}% before", (cur_fail * 100.0) as i64, (prev_fail * 100.0) as i64));
                action = PoliceAction::Rollback;
            } else {
                store.quarantine(&s.id, &format!("tasks using it fail {}% vs {}% without", (rate(fail_used, used.len()) * 100.0) as i64, (rate(fail_base, base.len()) * 100.0) as i64));
                action = PoliceAction::Quarantine;
            }
        }
        out.push(SkillHealth { skill_id: s.id.clone(), name: s.name.clone(), used: used.len(), fail_used, base: base.len(), fail_base, action });
    }
    let _ = params![];
    out
}
