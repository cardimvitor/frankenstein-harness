use super::io::{Ask, UiEvent};
use crate::engine::{NoticeKind, PlanDecision, TaskResult};
use crate::types::Mode;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

#[derive(Clone, Debug)]
pub enum Item {
    User(String),
    Notice(NoticeKind, String),
    /// streamed assistant progress text (dim, replaced by the verdict card at the end)
    Progress(String),
    Reasoning(String),
    Tool { name: String, args: String, ok: Option<bool>, ms: u64, output: String },
    Final { verdict: String, text: String, meta: String },
}

pub enum Modal {
    None,
    Plan { plan: String, trivial: bool, reply: Option<tokio::sync::oneshot::Sender<PlanDecision>> },
    Feedback { input: String, reply: Option<tokio::sync::oneshot::Sender<PlanDecision>> },
    Questions { questions: Vec<String>, answers: Vec<String>, idx: usize, input: String, reply: Option<tokio::sync::oneshot::Sender<Vec<String>>> },
    Confirm { tool: String, args: String, reply: Option<tokio::sync::oneshot::Sender<bool>> },
    Reuse { offers: Vec<crate::skills::reuse::ReuseOffer>, rows: Vec<(usize, usize)>, cursor: usize, chosen: Vec<bool>, reply: Option<tokio::sync::oneshot::Sender<Vec<(String, Vec<String>)>>> },
    Help,
}

/// What the terminal loop must do after a key press.
#[derive(Debug, PartialEq, Eq)]
pub enum Action {
    None,
    Submit(String),
    /// cancel the running task
    Cancel,
    Quit,
}

pub struct App {
    pub items: Vec<Item>,
    pub input: String,
    pub cursor: usize,
    pub busy: bool,
    pub auto: bool,
    pub approval: Mode,
    pub model: String,
    pub endpoint: String,
    pub cwd: String,
    /// lines scrolled up from the bottom (0 = follow the newest output)
    pub scroll: usize,
    pub show_reasoning: bool,
    pub verify: Vec<(String, String)>,
    pub files: Vec<String>,
    pub skills: Vec<String>,
    pub modal: Modal,
    pub spinner: usize,
    pub quit_armed: bool,
    pub last_verdict: Option<String>,
}

const MODES: [Mode; 4] = [Mode::Plan, Mode::Ask, Mode::AutoEdit, Mode::Yolo];

pub fn mode_name(m: Mode) -> &'static str {
    match m {
        Mode::Plan => "plan (read-only)",
        Mode::Ask => "ask each time",
        Mode::AutoEdit => "auto-edit",
        Mode::Yolo => "full auto",
    }
}

impl App {
    pub fn new(model: &str, endpoint: &str, cwd: &str, approval: Mode, auto: bool) -> App {
        App {
            items: vec![],
            input: String::new(),
            cursor: 0,
            busy: false,
            auto,
            approval,
            model: model.into(),
            endpoint: endpoint.into(),
            cwd: cwd.into(),
            scroll: 0,
            show_reasoning: false,
            verify: vec![],
            files: vec![],
            skills: vec![],
            modal: Modal::None,
            spinner: 0,
            quit_armed: false,
            last_verdict: None,
        }
    }

    pub fn has_modal(&self) -> bool {
        !matches!(self.modal, Modal::None)
    }

    fn push_text(&mut self, kind: &str, d: &str) {
        match (self.items.last_mut(), kind) {
            (Some(Item::Progress(s)), "progress") | (Some(Item::Reasoning(s)), "reasoning") => s.push_str(d),
            _ => self.items.push(if kind == "progress" { Item::Progress(d.to_string()) } else { Item::Reasoning(d.to_string()) }),
        }
    }

    /// Apply an engine/terminal event that is not a key press.
    pub fn apply(&mut self, ev: UiEvent) {
        match ev {
            UiEvent::Key(_) | UiEvent::Resize => {}
            UiEvent::Tick => self.spinner = self.spinner.wrapping_add(1),
            UiEvent::Notice(k, m) => {
                if k == NoticeKind::Skill {
                    self.skills.push(m.clone());
                    if self.skills.len() > 8 {
                        self.skills.remove(0);
                    }
                }
                if k == NoticeKind::Verify {
                    let ok = m.contains(": pass") || m.contains(": unverified");
                    self.verify.push((m.clone(), if ok { "ok".into() } else { "fail".into() }));
                }
                self.items.push(Item::Notice(k, m));
            }
            UiEvent::Progress(d) => self.push_text("progress", &d),
            UiEvent::Reasoning(d) => self.push_text("reasoning", &d),
            UiEvent::ToolStart(name, args) => self.items.push(Item::Tool { name, args, ok: None, ms: 0, output: String::new() }),
            UiEvent::ToolEnd(name, ok, output, ms) => {
                if let Some(Item::Tool { ok: o, ms: m, output: out, .. }) = self.items.iter_mut().rev().find(|i| matches!(i, Item::Tool { name: n, ok: None, .. } if *n == name)) {
                    *o = Some(ok);
                    *m = ms;
                    *out = output;
                }
            }
            UiEvent::Ask(a) => {
                self.modal = match a {
                    Ask::Plan { plan, trivial, reply } => Modal::Plan { plan, trivial, reply: Some(reply) },
                    Ask::Questions { questions, reply } => Modal::Questions { answers: vec![], idx: 0, input: String::new(), questions, reply: Some(reply) },
                    Ask::Confirm { tool, args, reply } => Modal::Confirm { tool, args, reply: Some(reply) },
                    Ask::Reuse { offers, reply } => {
                        let rows: Vec<(usize, usize)> = offers.iter().enumerate().flat_map(|(i, o)| (0..o.skills.len()).map(move |j| (i, j))).collect();
                        let n = rows.len();
                        Modal::Reuse { offers, rows, cursor: 0, chosen: vec![false; n], reply: Some(reply) }
                    }
                };
            }
            UiEvent::Result(r) => self.finish(*r),
        }
    }

    fn finish(&mut self, r: TaskResult) {
        self.busy = false;
        // the streamed progress text is superseded by the gated final answer
        self.items.retain(|i| !matches!(i, Item::Progress(_)));
        self.verify = r.verify.as_ref().map(|v| v.rounds.iter().flat_map(|rd| rd.checks.iter().map(move |c| (format!("r{} {}: {}", rd.round, c.name, c.status.as_str()), c.status.as_str().to_string()))).collect()).unwrap_or_else(|| self.verify.clone());
        self.files = r.changed.clone();
        self.last_verdict = Some(r.verdict.clone());
        let meta = format!("{:.1}s · {} round(s) · {} tokens out · {} requests", r.timings.total_ms as f64 / 1000.0, r.rounds, r.llm.completion_tokens, r.llm.requests);
        let text = if r.final_text.is_empty() { r.reason.clone() } else { r.final_text.clone() };
        self.items.push(Item::Final { verdict: r.verdict.clone(), text, meta });
        if let Some(p) = &r.rejected_patch {
            self.items.push(Item::Notice(NoticeKind::Warn, format!("rejected patch saved: {p}")));
        }
        self.scroll = 0;
    }

    fn insert(&mut self, c: char) {
        let at = self.input.char_indices().nth(self.cursor).map(|(i, _)| i).unwrap_or(self.input.len());
        self.input.insert(at, c);
        self.cursor += 1;
    }

    fn backspace(&mut self) {
        if self.cursor > 0 {
            let at = self.input.char_indices().nth(self.cursor - 1).map(|(i, _)| i).unwrap();
            self.input.remove(at);
            self.cursor -= 1;
        }
    }

    pub fn cycle_approval(&mut self) {
        let i = MODES.iter().position(|m| *m == self.approval).unwrap_or(2);
        self.approval = MODES[(i + 1) % MODES.len()];
    }

    /// Handle a key press. Modals capture input first.
    pub fn on_key(&mut self, k: KeyEvent) -> Action {
        if k.kind == KeyEventKind::Release {
            return Action::None;
        }
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
        if self.has_modal() {
            return self.modal_key(k);
        }
        if ctrl && k.code == KeyCode::Char('c') {
            if self.busy {
                self.quit_armed = false;
                return Action::Cancel;
            }
            return Action::Quit;
        }
        if !(ctrl && k.code == KeyCode::Char('c')) {
            self.quit_armed = false;
        }
        match k.code {
            KeyCode::Char('d') if ctrl => {
                if self.input.is_empty() { return Action::Quit; }
            }
            KeyCode::Char('a') if ctrl => self.auto = !self.auto,
            KeyCode::Char('t') if ctrl => self.show_reasoning = !self.show_reasoning,
            KeyCode::Char('u') if ctrl => {
                self.input.clear();
                self.cursor = 0;
            }
            KeyCode::Char('j') if ctrl => self.insert('\n'),
            KeyCode::Tab => self.cycle_approval(),
            KeyCode::F(1) => self.modal = Modal::Help,
            KeyCode::Esc => {
                if self.busy {
                    return Action::Cancel;
                }
                self.input.clear();
                self.cursor = 0;
            }
            KeyCode::Enter if k.modifiers.contains(KeyModifiers::ALT) || k.modifiers.contains(KeyModifiers::SHIFT) => self.insert('\n'),
            KeyCode::Enter => {
                let t = self.input.trim().to_string();
                if t.is_empty() {
                    return Action::None;
                }
                if t == "/exit" || t == "/quit" {
                    return Action::Quit;
                }
                if self.busy {
                    self.items.push(Item::Notice(NoticeKind::Warn, "a task is running; press Esc to stop it".into()));
                    return Action::None;
                }
                self.input.clear();
                self.cursor = 0;
                self.items.push(Item::User(t.clone()));
                self.busy = true;
                self.verify.clear();
                self.scroll = 0;
                return Action::Submit(t);
            }
            KeyCode::Backspace => self.backspace(),
            KeyCode::Left => self.cursor = self.cursor.saturating_sub(1),
            KeyCode::Right => self.cursor = (self.cursor + 1).min(self.input.chars().count()),
            KeyCode::Home => self.cursor = 0,
            KeyCode::End => self.cursor = self.input.chars().count(),
            KeyCode::PageUp => self.scroll += 8,
            KeyCode::PageDown => self.scroll = self.scroll.saturating_sub(8),
            KeyCode::Char(c) if !ctrl => self.insert(c),
            _ => {}
        }
        Action::None
    }

    fn modal_key(&mut self, k: KeyEvent) -> Action {
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
        if ctrl && k.code == KeyCode::Char('c') {
            // reject whatever is pending and cancel the task
            self.close_modal_default();
            return Action::Cancel;
        }
        let modal = std::mem::replace(&mut self.modal, Modal::None);
        match modal {
            Modal::Help => {
                // any key closes help
            }
            Modal::Plan { plan, trivial, mut reply } => match k.code {
                KeyCode::Enter => {
                    if let Some(r) = reply.take() { let _ = r.send(PlanDecision { ok: true, feedback: None }); }
                    self.items.push(Item::Notice(NoticeKind::Info, "plan approved".into()));
                }
                KeyCode::Char('n') | KeyCode::Esc => {
                    if let Some(r) = reply.take() { let _ = r.send(PlanDecision { ok: false, feedback: None }); }
                }
                KeyCode::Char('e') => self.modal = Modal::Feedback { input: String::new(), reply },
                _ => self.modal = Modal::Plan { plan, trivial, reply },
            },
            Modal::Feedback { mut input, mut reply } => match k.code {
                KeyCode::Enter => {
                    if let Some(r) = reply.take() { let _ = r.send(PlanDecision { ok: false, feedback: if input.trim().is_empty() { None } else { Some(input) } }); }
                }
                KeyCode::Esc => {
                    if let Some(r) = reply.take() { let _ = r.send(PlanDecision { ok: false, feedback: None }); }
                }
                KeyCode::Backspace => {
                    input.pop();
                    self.modal = Modal::Feedback { input, reply };
                }
                KeyCode::Char(c) if !ctrl => {
                    input.push(c);
                    self.modal = Modal::Feedback { input, reply };
                }
                _ => self.modal = Modal::Feedback { input, reply },
            },
            Modal::Questions { questions, mut answers, idx, mut input, mut reply } => match k.code {
                KeyCode::Enter => {
                    answers.push(if input.trim().is_empty() { "(use your best judgement)".to_string() } else { input.trim().to_string() });
                    if answers.len() >= questions.len() {
                        if let Some(r) = reply.take() { let _ = r.send(answers); }
                    } else {
                        self.modal = Modal::Questions { questions, answers, idx: idx + 1, input: String::new(), reply };
                    }
                }
                KeyCode::Esc => {
                    let n = questions.len();
                    if let Some(r) = reply.take() { let _ = r.send(vec!["(use your best judgement)".to_string(); n]); }
                }
                KeyCode::Backspace => {
                    input.pop();
                    self.modal = Modal::Questions { questions, answers, idx, input, reply };
                }
                KeyCode::Char(c) if !ctrl => {
                    input.push(c);
                    self.modal = Modal::Questions { questions, answers, idx, input, reply };
                }
                _ => self.modal = Modal::Questions { questions, answers, idx, input, reply },
            },
            Modal::Confirm { tool, args, mut reply } => match k.code {
                KeyCode::Char('y') | KeyCode::Char('Y') => {
                    if let Some(r) = reply.take() { let _ = r.send(true); }
                }
                KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => {
                    if let Some(r) = reply.take() { let _ = r.send(false); }
                }
                _ => self.modal = Modal::Confirm { tool, args, reply },
            },
            Modal::Reuse { offers, rows, mut cursor, mut chosen, mut reply } => {
                let mut done: Option<Vec<(String, Vec<String>)>> = None;
                match k.code {
                    KeyCode::Down | KeyCode::Char('j') => cursor = (cursor + 1).min(rows.len().saturating_sub(1)),
                    KeyCode::Up | KeyCode::Char('k') => cursor = cursor.saturating_sub(1),
                    KeyCode::Char(' ') => {
                        if let Some(c) = chosen.get_mut(cursor) { *c = !*c; }
                    }
                    KeyCode::Char('a') => chosen.iter_mut().for_each(|c| *c = true),
                    KeyCode::Enter => {
                        let mut out: Vec<(String, Vec<String>)> = Vec::new();
                        for (n, (i, j)) in rows.iter().enumerate() {
                            if chosen[n] {
                                let o = &offers[*i];
                                match out.iter_mut().find(|(p, _)| *p == o.from_project) {
                                    Some((_, ids)) => ids.push(o.skills[*j].id.clone()),
                                    None => out.push((o.from_project.clone(), vec![o.skills[*j].id.clone()])),
                                }
                            }
                        }
                        done = Some(out);
                    }
                    KeyCode::Char('n') | KeyCode::Esc => done = Some(vec![]),
                    _ => {}
                }
                match done {
                    Some(d) => {
                        if let Some(r) = reply.take() { let _ = r.send(d); }
                    }
                    None => self.modal = Modal::Reuse { offers, rows, cursor, chosen, reply },
                }
            }
            Modal::None => {}
        }
        Action::None
    }

    fn close_modal_default(&mut self) {
        match std::mem::replace(&mut self.modal, Modal::None) {
            Modal::Plan { reply: Some(r), .. } | Modal::Feedback { reply: Some(r), .. } => { let _ = r.send(PlanDecision { ok: false, feedback: None }); }
            Modal::Questions { questions, reply: Some(r), .. } => { let _ = r.send(vec!["(use your best judgement)".to_string(); questions.len()]); }
            Modal::Confirm { reply: Some(r), .. } => { let _ = r.send(false); }
            Modal::Reuse { reply: Some(r), .. } => { let _ = r.send(vec![]); }
            _ => {}
        }
    }
}

/// Wrap text to `width` columns (character-based), preserving explicit newlines.
pub fn wrap_text(s: &str, width: usize) -> Vec<String> {
    let width = width.max(4);
    let mut out = Vec::new();
    for line in s.split('\n') {
        if line.chars().count() <= width {
            out.push(line.to_string());
            continue;
        }
        let mut cur = String::new();
        for word in line.split(' ') {
            let wl = word.chars().count();
            if cur.chars().count() + wl + usize::from(!cur.is_empty()) > width {
                if !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                }
                let mut w = word;
                while w.chars().count() > width {
                    let cut = w.char_indices().nth(width).map(|(i, _)| i).unwrap_or(w.len());
                    out.push(w[..cut].to_string());
                    w = &w[cut..];
                }
                cur = w.to_string();
            } else {
                if !cur.is_empty() {
                    cur.push(' ');
                }
                cur.push_str(word);
            }
        }
        out.push(cur);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wraps_long_lines_and_words() {
        assert_eq!(wrap_text("aaa bbb ccc", 7), vec!["aaa bbb", "ccc"]);
        assert_eq!(wrap_text("abcdefghij", 4), vec!["abcd", "efgh", "ij"]);
        assert_eq!(wrap_text("a\nb", 10), vec!["a", "b"]);
    }
}
