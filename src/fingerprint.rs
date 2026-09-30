use crate::tools::fs::list_all;
use regex::Regex;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::path::Path;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StackTag {
    pub id: String,
    pub version: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VerifyKind {
    Build,
    Lint,
    Test,
    Types,
}

#[derive(Clone, Debug)]
pub struct VerifyCmd {
    pub name: String,
    pub cmd: String,
    pub kind: VerifyKind,
}

#[derive(Clone, Debug)]
pub struct Fingerprint {
    pub stacks: Vec<StackTag>,
    /// stable key of stack ids and major versions
    pub stack_key: String,
    /// sorted tokens used for similarity
    pub tokens: Vec<String>,
    pub project_id: String,
    pub verify: Vec<VerifyCmd>,
    pub summary: String,
}

fn tag(id: &str, v: Option<&str>) -> StackTag {
    StackTag { id: id.into(), version: v.map(|s| s.to_string()) }
}

fn major(v: &str) -> Option<String> {
    let t = v.trim_start_matches(|c: char| !c.is_ascii_digit());
    let m = t.split('.').next().unwrap_or("");
    if m.is_empty() { None } else { Some(m.to_string()) }
}

fn read(p: &Path) -> String {
    std::fs::read_to_string(p).unwrap_or_default()
}

fn read_json(p: &Path) -> Option<Value> {
    serde_json::from_str(&std::fs::read_to_string(p).ok()?).ok()
}

pub fn detect_dotnet(files: &[String], cwd: &Path) -> Vec<StackTag> {
    let proj: Vec<&String> = files.iter().filter(|f| f.ends_with(".csproj") || f.ends_with(".fsproj") || f.ends_with(".vbproj")).collect();
    if proj.is_empty() {
        return vec![];
    }
    let tf = Regex::new(r"<TargetFrameworks?>([^<]+)<").unwrap();
    let old = Regex::new(r"<TargetFrameworkVersion>v?([\d.]+)<").unwrap();
    let mut fw: Vec<String> = Vec::new();
    for p in proj.iter().take(50) {
        let x = read(&cwd.join(p));
        for m in tf.captures_iter(&x) {
            for t in m[1].split(';') {
                fw.push(t.trim().to_string());
            }
        }
        if let Some(m) = old.captures(&x) {
            fw.push(format!("net{}", m[1].replace('.', "")));
        }
    }
    let modern = Regex::new(r"^net(\d+)\.\d+").unwrap();
    let framework = Regex::new(r"^net(4\d+)$").unwrap();
    let core = Regex::new(r"^netcoreapp(\d)").unwrap();
    let mut tags = Vec::new();
    for t in fw {
        if let Some(m) = modern.captures(&t) {
            tags.push(tag("dotnet", Some(&m[1])));
        } else if let Some(m) = framework.captures(&t) {
            tags.push(tag("dotnet-framework", Some(&m[1])));
        } else if let Some(m) = core.captures(&t) {
            tags.push(tag("dotnet", Some(&m[1])));
        } else if t.starts_with("netstandard") {
            tags.push(tag("dotnet-standard", Some(t.trim_start_matches("netstandard"))));
        }
    }
    if tags.is_empty() {
        tags.push(tag("dotnet", None));
    }
    tags
}

/// Spring Boot (Maven or Gradle) and the JVM language; the version is the Boot major from the parent POM or plugin.
pub fn detect_jvm(files: &[String], cwd: &Path) -> Vec<StackTag> {
    let builds: Vec<&String> = files.iter().filter(|f| matches!(f.rsplit('/').next(), Some("pom.xml") | Some("build.gradle") | Some("build.gradle.kts"))).collect();
    if builds.is_empty() {
        return vec![];
    }
    let text: String = builds.iter().take(20).map(|f| read(&cwd.join(f))).collect::<Vec<_>>().join("\n");
    let mut tags = vec![tag("java", None)];
    if text.contains("spring-boot") || text.contains("org.springframework.boot") {
        let parent = Regex::new(r"(?s)<parent>.*?spring-boot-starter-parent.*?<version>\s*(\d+)\.").unwrap();
        let plugin = Regex::new(r#"org\.springframework\.boot['"]?\)?\s*(?:version)?\s*['"](\d+)\."#).unwrap();
        let bom = Regex::new(r"spring-boot-dependencies[:\s\S]{0,80}?(\d+)\.\d+\.\d+").unwrap();
        let v = parent.captures(&text).or_else(|| plugin.captures(&text)).or_else(|| bom.captures(&text)).map(|m| m[1].to_string());
        tags.push(StackTag { id: "spring-boot".into(), version: v });
    }
    tags
}

/// Django: manage.py or a Django dependency; the version is major.minor from the first pinned requirement.
pub fn detect_django(files: &[String], cwd: &Path) -> Vec<StackTag> {
    let reqs: Vec<&String> = files.iter().filter(|f| { let n = f.rsplit('/').next().unwrap_or(""); n.starts_with("requirements") && n.ends_with(".txt") || matches!(n, "pyproject.toml" | "setup.py" | "setup.cfg" | "Pipfile") }).collect();
    let text: String = reqs.iter().take(20).map(|f| read(&cwd.join(f))).collect::<Vec<_>>().join("\n");
    let dep = Regex::new(r#"(?im)(?:^|[\s"'\[,])django(?:\[[^\]]*\])?\s*(?:(?:==|>=|~=|<=|=|>|<)\s*["']?(\d+)\.(\d+)[\w.*]*)?(?:[\s"',\]]|$)"#).unwrap();
    let has_manage = files.iter().any(|f| f == "manage.py");
    let m = dep.captures(&text);
    if !has_manage && m.is_none() {
        return vec![];
    }
    let version = m.and_then(|c| Some(format!("{}.{}", c.get(1)?.as_str(), c.get(2)?.as_str())));
    vec![StackTag { id: "django".into(), version }]
}

pub fn detect_node(pkg: &Option<Value>, files: &[String]) -> Vec<StackTag> {
    let Some(pkg) = pkg else { return vec![] };
    let dep = |n: &str| -> Option<String> { ["dependencies", "devDependencies"].iter().find_map(|k| pkg.get(k).and_then(|d| d.get(n)).and_then(|v| v.as_str()).map(|s| s.to_string())) };
    let mut tags = vec![tag("node", None)];
    if let Some(v) = dep("react") {
        tags.push(StackTag { id: "react".into(), version: major(&v) });
    }
    if let Some(v) = dep("@angular/core") {
        tags.push(StackTag { id: "angular".into(), version: major(&v) });
    }
    if let Some(v) = dep("angular") {
        if major(&v).as_deref() == Some("1") {
            tags.push(tag("angularjs", Some("1")));
        }
    }
    if let Some(v) = dep("vue") {
        // a non-numeric spec (latest, catalog:, workspace:) means the current major
        tags.push(StackTag { id: "vue".into(), version: major(&v).or_else(|| Some("3".into())) });
    }
    if dep("typescript").is_some() || files.iter().any(|f| f == "tsconfig.json") {
        tags.push(tag("typescript", None));
    }
    tags
}

pub fn detect_verify(cwd: &Path, files: &[String], pkg: &Option<Value>) -> Vec<VerifyCmd> {
    if let Some(Value::Array(a)) = read_json(&cwd.join(".fh").join("verify.json")) {
        let parsed: Vec<VerifyCmd> = a
            .iter()
            .filter_map(|v| {
                let kind = match v.get("kind")?.as_str()? {
                    "build" => VerifyKind::Build,
                    "lint" => VerifyKind::Lint,
                    "test" => VerifyKind::Test,
                    "types" => VerifyKind::Types,
                    _ => return None,
                };
                Some(VerifyCmd { name: v.get("name")?.as_str()?.to_string(), cmd: v.get("cmd")?.as_str()?.to_string(), kind })
            })
            .collect();
        return parsed;
    }
    let mut out = Vec::new();
    if let Some(scripts) = pkg.as_ref().and_then(|p| p.get("scripts")).and_then(|s| s.as_object()) {
        let pm = if cwd.join("pnpm-lock.yaml").exists() { "pnpm" } else if cwd.join("yarn.lock").exists() { "yarn" } else { "npm" };
        let run = |s: &str| if pm == "npm" { format!("npm run {s}") } else { format!("{pm} {s}") };
        for s in ["typecheck", "tsc", "type-check"] {
            if scripts.contains_key(s) {
                out.push(VerifyCmd { name: s.into(), cmd: run(s), kind: VerifyKind::Types });
                break;
            }
        }
        if scripts.contains_key("build") {
            out.push(VerifyCmd { name: "build".into(), cmd: run("build"), kind: VerifyKind::Build });
        }
        if scripts.contains_key("lint") {
            out.push(VerifyCmd { name: "lint".into(), cmd: run("lint"), kind: VerifyKind::Lint });
        }
        if let Some(t) = scripts.get("test").and_then(|t| t.as_str()) {
            if !t.contains("no test specified") {
                out.push(VerifyCmd { name: "test".into(), cmd: if pm == "npm" { "npm test --silent".into() } else { format!("{pm} test") }, kind: VerifyKind::Test });
            }
        }
    }
    let sln = files.iter().find(|f| f.ends_with(".sln")).or_else(|| files.iter().find(|f| f.ends_with(".csproj")));
    if let Some(sln) = sln {
        out.push(VerifyCmd { name: "dotnet build".into(), cmd: format!("dotnet build {sln:?} --nologo -v q"), kind: VerifyKind::Build });
        let tests = Regex::new(r"Tests?\.csproj$|\.Tests?/").unwrap();
        if files.iter().any(|f| tests.is_match(f)) {
            out.push(VerifyCmd { name: "dotnet test".into(), cmd: format!("dotnet test {sln:?} --nologo -v q"), kind: VerifyKind::Test });
        }
    }
    let root_has = |n: &str| files.iter().any(|f| f == n);
    if root_has("pom.xml") {
        let mvn = if cwd.join("mvnw").exists() { "./mvnw" } else { "mvn" };
        out.push(VerifyCmd { name: "maven compile".into(), cmd: format!("{mvn} -q -B -DskipTests compile"), kind: VerifyKind::Build });
        out.push(VerifyCmd { name: "maven test".into(), cmd: format!("{mvn} -q -B test"), kind: VerifyKind::Test });
    } else if root_has("build.gradle") || root_has("build.gradle.kts") {
        let g = if cwd.join("gradlew").exists() { "./gradlew" } else { "gradle" };
        out.push(VerifyCmd { name: "gradle classes".into(), cmd: format!("{g} -q classes"), kind: VerifyKind::Build });
        out.push(VerifyCmd { name: "gradle test".into(), cmd: format!("{g} -q test"), kind: VerifyKind::Test });
    }
    let pytest_marker = files.iter().any(|f| matches!(f.as_str(), "pyproject.toml" | "pytest.ini" | "setup.py" | "tox.ini" | "conftest.py"));
    let py_tests = Regex::new(r"(^|/)(test_.*|.*_test)\.py$").unwrap();
    let django = root_has("manage.py") && !pytest_marker;
    if django {
        out.push(VerifyCmd { name: "django check".into(), cmd: "python3 manage.py check".into(), kind: VerifyKind::Build });
        out.push(VerifyCmd { name: "django test".into(), cmd: "python3 manage.py test".into(), kind: VerifyKind::Test });
    } else if files.iter().any(|f| py_tests.is_match(f)) {
        if pytest_marker {
            out.push(VerifyCmd { name: "pytest".into(), cmd: "python3 -m pytest -q".into(), kind: VerifyKind::Test });
        } else {
            out.push(VerifyCmd { name: "unittest".into(), cmd: "python3 -m unittest discover -q".into(), kind: VerifyKind::Test });
        }
    }
    if files.iter().any(|f| f == "go.mod") {
        out.push(VerifyCmd { name: "go build".into(), cmd: "go build ./...".into(), kind: VerifyKind::Build });
        out.push(VerifyCmd { name: "go test".into(), cmd: "go test ./...".into(), kind: VerifyKind::Test });
    }
    if files.iter().any(|f| f == "Cargo.toml") {
        out.push(VerifyCmd { name: "cargo check".into(), cmd: "cargo check --quiet".into(), kind: VerifyKind::Build });
        out.push(VerifyCmd { name: "cargo test".into(), cmd: "cargo test --quiet".into(), kind: VerifyKind::Test });
    }
    out
}

pub fn fingerprint(cwd: &Path) -> Fingerprint {
    let files = list_all(cwd, 4000);
    let pkg = read_json(&cwd.join("package.json"));
    let mut stacks: Vec<StackTag> = detect_dotnet(&files, cwd);
    stacks.extend(detect_node(&pkg, &files));
    stacks.extend(detect_jvm(&files, cwd));
    stacks.extend(detect_django(&files, cwd));
    if files.iter().any(|f| f.ends_with(".py")) {
        stacks.push(tag("python", None));
    }
    if files.iter().any(|f| f == "go.mod") {
        stacks.push(tag("go", None));
    }
    if files.iter().any(|f| f == "Cargo.toml") {
        stacks.push(tag("rust", None));
    }
    let mut seen = HashSet::new();
    let uniq: Vec<StackTag> = stacks.into_iter().filter(|s| seen.insert(format!("{}@{}", s.id, s.version.clone().unwrap_or_default()))).collect();
    let mut tokens: Vec<String> = uniq.iter().flat_map(|s| match &s.version { Some(v) => vec![s.id.clone(), format!("{}@{}", s.id, v)], None => vec![s.id.clone()] }).collect();
    tokens.sort();
    tokens.dedup();
    let key: Vec<String> = tokens.iter().filter(|t| t.contains('@') || !uniq.iter().any(|s| &s.id == *t && s.version.is_some())).cloned().collect();
    let stack_key = if key.is_empty() { "unknown".to_string() } else { key.join("+") };
    let remote = Regex::new(r"url = (.+)").unwrap().captures(&read(&cwd.join(".git").join("config"))).map(|m| m[1].trim().to_string()).unwrap_or_default();
    let id_src = if remote.is_empty() { cwd.to_string_lossy().to_string() } else { remote };
    let mut h = Sha256::new();
    h.update(id_src.as_bytes());
    let project_id: String = format!("{:x}", h.finalize()).chars().take(16).collect();
    let mut dirs: Vec<String> = std::fs::read_dir(cwd)
        .map(|rd| rd.flatten().filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false)).map(|e| e.file_name().to_string_lossy().to_string()).filter(|n| !n.starts_with('.') && n != "node_modules" && n != "target").collect())
        .unwrap_or_default();
    dirs.sort();
    dirs.truncate(12);
    let stacks_txt: Vec<String> = uniq.iter().map(|s| match &s.version { Some(v) => format!("{} {}", s.id, v), None => s.id.clone() }).collect();
    let summary = format!("{}{}", if stacks_txt.is_empty() { "unknown stack".to_string() } else { stacks_txt.join(", ") }, if dirs.is_empty() { String::new() } else { format!("; dirs: {}", dirs.join(", ")) });
    let verify = detect_verify(cwd, &files, &pkg);
    Fingerprint { stacks: uniq, stack_key, tokens, project_id, verify, summary }
}

/// Jaccard similarity of stack tokens.
pub fn similarity(a: &[String], b: &[String]) -> f64 {
    let sa: HashSet<&String> = a.iter().collect();
    let sb: HashSet<&String> = b.iter().collect();
    if sa.is_empty() || sb.is_empty() {
        return 0.0;
    }
    let i = sa.intersection(&sb).count();
    i as f64 / (sa.len() + sb.len() - i) as f64
}
