use std::collections::{HashMap, HashSet};

const STOP: &str = "a an the and or of to in on for with is are be this that it as at by from into can you your we our not do does did make add fix use using create update change should would could please so if then when how what all any more than but also need want get set new one two its their them they there here about after before over under out up down just like into onto per via";

pub fn tokenize(text: &str) -> Vec<String> {
    let stop: HashSet<&str> = STOP.split(' ').collect();
    // split camelCase: insert a space between a lowercase letter and an uppercase one
    let mut s = String::with_capacity(text.len() + 8);
    let mut prev_lower = false;
    for c in text.chars() {
        if prev_lower && c.is_uppercase() {
            s.push(' ');
        }
        prev_lower = c.is_lowercase();
        s.push(c);
    }
    s.to_lowercase()
        .split(|c: char| !(c.is_ascii_alphanumeric() || "#@.+".contains(c)))
        .map(|t| t.trim_matches(|c| c == '.' || c == '+').to_string())
        .filter(|t| t.chars().count() > 1 && !stop.contains(t.as_str()))
        .collect()
}

pub struct Doc {
    pub id: String,
    pub text: String,
    pub boost: String,
}

struct Entry {
    id: String,
    tf: HashMap<String, f64>,
    len: f64,
}

/// Small in-memory BM25 (k1=1.4, b=0.75). Corpora are tens to hundreds of skills: well under a millisecond.
pub struct Bm25 {
    docs: Vec<Entry>,
    df: HashMap<String, f64>,
    avg: f64,
}

impl Bm25 {
    pub fn new(docs: Vec<Doc>) -> Self {
        let mut entries = Vec::new();
        let mut df: HashMap<String, f64> = HashMap::new();
        for d in docs {
            let mut toks = tokenize(&d.text);
            let b = tokenize(&d.boost);
            toks.extend(b.iter().cloned());
            toks.extend(b);
            let mut tf: HashMap<String, f64> = HashMap::new();
            for t in &toks {
                *tf.entry(t.clone()).or_insert(0.0) += 1.0;
            }
            for t in tf.keys() {
                *df.entry(t.clone()).or_insert(0.0) += 1.0;
            }
            entries.push(Entry { id: d.id, len: toks.len() as f64, tf });
        }
        let avg = (entries.iter().map(|e| e.len).sum::<f64>() / entries.len().max(1) as f64).max(1.0);
        Bm25 { docs: entries, df, avg }
    }

    pub fn score(&self, query: &[String]) -> HashMap<String, f64> {
        let n = self.docs.len() as f64;
        let q: HashSet<&String> = query.iter().collect();
        let mut out = HashMap::new();
        for d in &self.docs {
            let mut s = 0.0;
            for t in &q {
                let Some(f) = d.tf.get(*t) else { continue };
                let df = self.df.get(*t).copied().unwrap_or(0.0);
                let idf = (1.0 + (n - df + 0.5) / (df + 0.5)).ln();
                s += idf * ((f * 2.4) / (f + 1.4 * (0.25 + 0.75 * (d.len / self.avg))));
            }
            if s > 0.0 {
                out.insert(d.id.clone(), s);
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn ranks_relevant_first_and_splits_camel_case() {
        let b = Bm25::new(vec![Doc { id: "a".into(), text: "sql index query database".into(), boost: String::new() }, Doc { id: "b".into(), text: "css layout button color".into(), boost: String::new() }]);
        let sc = b.score(&tokenize("optimize the slow SQL query"));
        assert!(sc.get("a").copied().unwrap_or(0.0) > sc.get("b").copied().unwrap_or(0.0));
        assert_eq!(tokenize("fix sumRange"), vec!["sum", "range"]);
    }
}
