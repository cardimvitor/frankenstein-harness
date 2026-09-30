use super::store::{NewSkill, Scope, SkillStore};
use crate::fingerprint::{similarity, Fingerprint};

#[derive(Clone, Debug)]
pub struct OfferedSkill {
    pub id: String,
    pub name: String,
    pub summary: String,
}

/// Names and one-line summaries only, never bodies.
#[derive(Clone, Debug)]
pub struct ReuseOffer {
    pub from_project: String,
    pub from_label: String,
    pub similarity: f64,
    pub skills: Vec<OfferedSkill>,
}

/// On a new/unfamiliar repo: find project-scoped skills from other projects with a similar fingerprint.
pub fn find_reuse_offers(store: &SkillStore, fp: &Fingerprint, min_sim: f64) -> Vec<ReuseOffer> {
    if store.user_skills(Some(&fp.project_id)).iter().any(|s| s.scope == Scope::Project && s.project_id.as_deref() == Some(fp.project_id.as_str())) {
        return vec![];
    }
    let all = store.user_skills(None);
    let mut offers: Vec<ReuseOffer> = Vec::new();
    for (id, label, tokens) in store.other_projects(&fp.project_id) {
        let sim = similarity(&tokens, &fp.tokens);
        if sim < min_sim {
            continue;
        }
        let skills: Vec<OfferedSkill> = all.iter().filter(|s| s.scope == Scope::Project && s.project_id.as_deref() == Some(id.as_str()) && s.state == "active" && s.source != "builtin").map(|s| OfferedSkill { id: s.id.clone(), name: s.name.clone(), summary: s.summary.clone() }).collect();
        if !skills.is_empty() {
            offers.push(ReuseOffer { from_project: id, from_label: label, similarity: sim, skills });
        }
    }
    offers.sort_by(|a, b| b.similarity.partial_cmp(&a.similarity).unwrap_or(std::cmp::Ordering::Equal));
    offers
}

/// Copy (not link) chosen skills into the new project, tagged with their origin.
pub fn accept_reuse(store: &SkillStore, fp: &Fingerprint, offer: &ReuseOffer, ids: &[String]) -> usize {
    let mut n = 0;
    for id in ids {
        if !offer.skills.iter().any(|x| &x.id == id) {
            continue;
        }
        let Some(s) = store.get(id) else { continue };
        let r = store.add(
            NewSkill { name: s.name, scope: Scope::Project, stack: None, versions: None, project_id: Some(fp.project_id.clone()), source: "reused".into(), origin: Some(offer.from_label.clone()), summary: s.summary, keywords: s.keywords, body: s.body },
            &format!("copied from {}", offer.from_label),
        );
        if r.is_ok() {
            n += 1;
        }
    }
    n
}
