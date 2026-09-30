use crate::engine::{Io, NoticeKind, PlanDecision, TaskResult};
use crate::skills::reuse::ReuseOffer;
use async_trait::async_trait;
use serde_json::Value;
use tokio::sync::{mpsc::UnboundedSender, oneshot};

/// Questions the engine asks the user; each carries the channel the answer goes back on.
pub enum Ask {
    Questions { questions: Vec<String>, reply: oneshot::Sender<Vec<String>> },
    Plan { plan: String, trivial: bool, reply: oneshot::Sender<PlanDecision> },
    Confirm { tool: String, args: String, reply: oneshot::Sender<bool> },
    Reuse { offers: Vec<ReuseOffer>, reply: oneshot::Sender<Vec<(String, Vec<String>)>> },
}

pub enum UiEvent {
    Key(ratatui::crossterm::event::KeyEvent),
    Resize,
    Tick,
    Notice(NoticeKind, String),
    Progress(String),
    Reasoning(String),
    ToolStart(String, String),
    ToolEnd(String, bool, String, u64),
    Ask(Ask),
    Result(Box<TaskResult>),
}

/// Engine-facing Io that forwards everything to the render loop.
pub struct TuiIo {
    pub tx: UnboundedSender<UiEvent>,
}

#[async_trait]
impl Io for TuiIo {
    fn notice(&self, kind: NoticeKind, msg: &str) {
        let _ = self.tx.send(UiEvent::Notice(kind, msg.to_string()));
    }
    fn progress(&self, d: &str) {
        let _ = self.tx.send(UiEvent::Progress(d.to_string()));
    }
    fn reasoning(&self, d: &str) {
        let _ = self.tx.send(UiEvent::Reasoning(d.to_string()));
    }
    fn tool_start(&self, name: &str, args: &Value) {
        let _ = self.tx.send(UiEvent::ToolStart(name.to_string(), args.to_string()));
    }
    fn tool_end(&self, name: &str, ok: bool, output: &str, ms: u64) {
        let _ = self.tx.send(UiEvent::ToolEnd(name.to_string(), ok, output.chars().take(4000).collect(), ms));
    }
    async fn ask_questions(&self, questions: Vec<String>) -> Vec<String> {
        let (reply, rx) = oneshot::channel();
        let n = questions.len();
        let _ = self.tx.send(UiEvent::Ask(Ask::Questions { questions, reply }));
        rx.await.unwrap_or_else(|_| vec!["(use your best judgement)".to_string(); n])
    }
    async fn approve_plan(&self, plan: &str, trivial: bool) -> PlanDecision {
        let (reply, rx) = oneshot::channel();
        let _ = self.tx.send(UiEvent::Ask(Ask::Plan { plan: plan.to_string(), trivial, reply }));
        rx.await.unwrap_or_default()
    }
    async fn confirm(&self, tool: &str, args: &Value) -> bool {
        let (reply, rx) = oneshot::channel();
        let _ = self.tx.send(UiEvent::Ask(Ask::Confirm { tool: tool.to_string(), args: args.to_string(), reply }));
        rx.await.unwrap_or(false)
    }
    async fn offer_reuse(&self, offers: &[ReuseOffer]) -> Vec<(String, Vec<String>)> {
        let (reply, rx) = oneshot::channel();
        let _ = self.tx.send(UiEvent::Ask(Ask::Reuse { offers: offers.to_vec(), reply }));
        rx.await.unwrap_or_default()
    }
}
