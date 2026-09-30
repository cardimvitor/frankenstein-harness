//! Optional embedding recall for the skill gate. BM25 stays the default (zero extra calls, p50 < 50 ms).
//! When `embeddingModel` is configured, skills whose embedding is close to the task but that BM25 did not
//! select are added (recall only: nothing BM25 selected is removed). Skill vectors are cached in the store.
use super::gate::GateResult;
use super::store::{Skill, SkillStore};
use crate::config::{auth_headers, Config, Env};
use crate::fingerprint::Fingerprint;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::time::Duration;

pub const MIN_COSINE: f32 = 0.55;
const MAX_ADDED: usize = 2;

pub fn cosine(a: &[f32], b: &[f32]) -> f32 {
    let (mut dot, mut na, mut nb) = (0f32, 0f32, 0f32);
    for (x, y) in a.iter().zip(b) {
        dot += x * y;
        na += x * x;
        nb += y * y;
    }
    if na == 0.0 || nb == 0.0 { 0.0 } else { dot / (na.sqrt() * nb.sqrt()) }
}

fn skill_text(s: &Skill) -> String {
    format!("{}. {} {}", s.name, s.summary, s.keywords)
}

fn text_hash(model: &str, t: &str) -> String {
    let mut h = Sha256::new();
    h.update(model.as_bytes());
    h.update(t.as_bytes());
    format!("{:x}", h.finalize())
}

pub async fn embed(cfg: &Config, env: &Env, texts: &[String]) -> Result<Vec<Vec<f32>>, String> {
    let base = if cfg.embedding_endpoint.is_empty() { cfg.endpoint.clone() } else { cfg.embedding_endpoint.trim_end_matches('/').to_string() };
    let mut req = crate::http::client(cfg).post(format!("{base}/embeddings")).timeout(Duration::from_secs(20)).json(&json!({"model": cfg.embedding_model, "input": texts}));
    for (k, v) in auth_headers(cfg, env) {
        req = req.header(k, v);
    }
    let res = req.send().await.map_err(|e| format!("embedding request failed: {e}"))?;
    if !res.status().is_success() {
        return Err(format!("embedding HTTP {}", res.status().as_u16()));
    }
    let j: Value = res.json().await.map_err(|e| e.to_string())?;
    let data = j["data"].as_array().ok_or("embedding response has no data")?;
    let mut out: Vec<(usize, Vec<f32>)> = data.iter().filter_map(|d| Some((d["index"].as_u64().unwrap_or(0) as usize, d["embedding"].as_array()?.iter().filter_map(|x| x.as_f64().map(|f| f as f32)).collect()))).collect();
    out.sort_by_key(|(i, _)| *i);
    if out.len() != texts.len() {
        return Err("embedding count mismatch".into());
    }
    Ok(out.into_iter().map(|(_, v)| v).collect())
}

fn ensure_table(store: &SkillStore) {
    let _ = store.conn().execute("CREATE TABLE IF NOT EXISTS skill_embeddings(skill_id TEXT, text_hash TEXT, vec TEXT, PRIMARY KEY(skill_id, text_hash))", []);
}

fn cached(store: &SkillStore, id: &str, hash: &str) -> Option<Vec<f32>> {
    let c = store.conn();
    let s: String = c.query_row("SELECT vec FROM skill_embeddings WHERE skill_id=?1 AND text_hash=?2", [id, hash], |r| r.get(0)).ok()?;
    serde_json::from_str(&s).ok()
}

/// Adds skills the embedding thinks apply but BM25 missed. Returns the names added.
/// Any failure leaves the gate result untouched.
pub async fn rerank(store: &SkillStore, cfg: &Config, env: &Env, fp: &Fingerprint, task: &str, g: &mut GateResult) -> Vec<String> {
    if cfg.embedding_model.is_empty() {
        return vec![];
    }
    ensure_table(store);
    let pool: Vec<Skill> = store.all_for_project(fp).into_iter().filter(|s| !g.selected.iter().any(|x| x.id == s.id)).collect();
    if pool.is_empty() {
        return vec![];
    }
    let mut vecs: Vec<Option<Vec<f32>>> = Vec::new();
    let mut missing: Vec<(usize, String)> = Vec::new();
    for (i, s) in pool.iter().enumerate() {
        let h = text_hash(&cfg.embedding_model, &skill_text(s));
        let v = cached(store, &s.id, &h);
        if v.is_none() {
            missing.push((i, skill_text(s)));
        }
        vecs.push(v);
    }
    let mut inputs: Vec<String> = vec![task.to_string()];
    inputs.extend(missing.iter().map(|(_, t)| t.clone()));
    let Ok(mut out) = embed(cfg, env, &inputs).await else { return vec![] };
    let task_vec = out.remove(0);
    for ((i, text), v) in missing.iter().zip(out) {
        let h = text_hash(&cfg.embedding_model, text);
        let _ = store.conn().execute("INSERT OR REPLACE INTO skill_embeddings(skill_id,text_hash,vec) VALUES(?1,?2,?3)", [&pool[*i].id, &h, &serde_json::to_string(&v).unwrap_or_default()]);
        vecs[*i] = Some(v);
    }
    let mut scored: Vec<(f32, &Skill)> = pool.iter().zip(vecs.iter()).filter_map(|(s, v)| v.as_ref().map(|v| (cosine(&task_vec, v), s))).filter(|(c, _)| *c >= MIN_COSINE).collect();
    scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
    let mut added = Vec::new();
    for (c, s) in scored.into_iter().take(MAX_ADDED) {
        if g.selected.len() >= 4 {
            break;
        }
        store.log("embed_recall", &s.name, &format!("added by embedding similarity {c:.2}; BM25 had not selected it"), Some(&fp.project_id));
        added.push(s.name.clone());
        g.selected.push(s.clone());
    }
    added
}
