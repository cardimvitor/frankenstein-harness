use super::app::{mode_name, wrap_text, App, Item, Modal};
use crate::engine::NoticeKind;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};
use ratatui::Frame;

const SPINNER: [&str; 8] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧"];

fn plain() -> bool {
    std::env::var_os("NO_COLOR").is_some()
}
fn fg(c: Color) -> Style {
    if plain() { Style::default() } else { Style::default().fg(c) }
}
fn dim() -> Style {
    if plain() { Style::default() } else { Style::default().add_modifier(Modifier::DIM) }
}
fn bold() -> Style {
    Style::default().add_modifier(Modifier::BOLD)
}

fn verdict_color(v: &str) -> Color {
    match v {
        "pass" | "planned" => Color::Green,
        "unverified" => Color::Yellow,
        _ => Color::Red,
    }
}

fn notice_glyph(k: NoticeKind) -> (&'static str, Style) {
    match k {
        NoticeKind::Phase => ("▸", fg(Color::Cyan)),
        NoticeKind::Skill => ("✦", fg(Color::Yellow)),
        NoticeKind::Verify => ("✓", fg(Color::Green)),
        NoticeKind::Warn => ("!", fg(Color::Red)),
        NoticeKind::Info => ("·", dim()),
    }
}

/// Convert the conversation into display lines for a given width.
pub fn chat_lines(app: &App, width: usize) -> Vec<Line<'static>> {
    let w = width.saturating_sub(2).max(10);
    let mut out: Vec<Line<'static>> = Vec::new();
    for it in &app.items {
        match it {
            Item::User(t) => {
                for (i, l) in wrap_text(t, w - 2).into_iter().enumerate() {
                    out.push(Line::from(vec![Span::styled(if i == 0 { "› " } else { "  " }, fg(Color::Cyan).add_modifier(Modifier::BOLD)), Span::styled(l, bold())]));
                }
                out.push(Line::from(""));
            }
            Item::Notice(k, m) => {
                let (g, s) = notice_glyph(*k);
                for (i, l) in wrap_text(m, w - 2).into_iter().enumerate() {
                    out.push(Line::from(vec![Span::styled(if i == 0 { format!("{g} ") } else { "  ".to_string() }, s), Span::styled(l, if *k == NoticeKind::Info { dim() } else { Style::default() })]));
                }
            }
            Item::Progress(t) => {
                for l in wrap_text(t.trim_end(), w - 2) {
                    out.push(Line::from(Span::styled(format!("  {l}"), dim())));
                }
            }
            Item::Reasoning(t) => {
                if app.show_reasoning {
                    out.push(Line::from(Span::styled("  thinking:", dim().add_modifier(Modifier::ITALIC))));
                    for l in wrap_text(t.trim_end(), w - 4) {
                        out.push(Line::from(Span::styled(format!("    {l}"), dim().add_modifier(Modifier::ITALIC))));
                    }
                } else {
                    out.push(Line::from(Span::styled(format!("  thinking… ({} chars, Ctrl+T to show)", t.chars().count()), dim())));
                }
            }
            Item::Tool { name, args, ok, ms, output } => {
                let (status, style) = match ok {
                    None => ("…".to_string(), dim()),
                    Some(true) => (format!("ok {ms}ms"), fg(Color::Green)),
                    Some(false) => (format!("fail {ms}ms"), fg(Color::Red)),
                };
                let a: String = args.chars().take(w.saturating_sub(name.len() + status.len() + 8)).collect();
                out.push(Line::from(vec![Span::styled(format!("  {name} "), bold()), Span::styled(a, dim()), Span::raw(" "), Span::styled(status, style)]));
                if *ok == Some(false) {
                    if let Some(first) = output.lines().next() {
                        out.push(Line::from(Span::styled(format!("    {}", first.chars().take(w.saturating_sub(4)).collect::<String>()), fg(Color::Red))));
                    }
                }
            }
            Item::Final { verdict, text, meta } => {
                let c = verdict_color(verdict);
                out.push(Line::from(Span::styled("─".repeat(w.min(60)), fg(c))));
                for l in wrap_text(text, w) {
                    out.push(Line::from(Span::styled(l, if plain() { Style::default() } else { Style::default().fg(c) })));
                }
                out.push(Line::from(Span::styled(meta.clone(), dim())));
                out.push(Line::from(""));
            }
        }
    }
    out
}

fn centered(area: Rect, w_pct: u16, h_pct: u16) -> Rect {
    let v = Layout::default().direction(Direction::Vertical).constraints([Constraint::Percentage((100 - h_pct) / 2), Constraint::Percentage(h_pct), Constraint::Percentage((100 - h_pct) / 2)]).split(area);
    Layout::default().direction(Direction::Horizontal).constraints([Constraint::Percentage((100 - w_pct) / 2), Constraint::Percentage(w_pct), Constraint::Percentage((100 - w_pct) / 2)]).split(v[1])[1]
}

fn side_block(title: &str, lines: Vec<Line<'static>>, empty: &str) -> (Vec<Line<'static>>, String) {
    if lines.is_empty() { (vec![Line::from(Span::styled(empty.to_string(), dim()))], title.to_string()) } else { (lines, title.to_string()) }
}

pub fn draw(f: &mut Frame, app: &mut App) {
    let area = f.area();
    let input_lines = app.input.split('\n').count().clamp(1, 5) as u16;
    let rows = Layout::default().direction(Direction::Vertical).constraints([Constraint::Length(1), Constraint::Min(3), Constraint::Length(input_lines + 2), Constraint::Length(1)]).split(area);

    // header
    let status = if app.busy { format!("{} working", SPINNER[app.spinner % SPINNER.len()]) } else { "idle".to_string() };
    let header = Line::from(vec![
        Span::styled(" Frankenstein ", bold().add_modifier(Modifier::REVERSED)),
        Span::raw(format!(" {} · {} · {} · {}", app.model, if app.auto { "Autonomous" } else { "Guided" }, mode_name(app.approval), status)),
    ]);
    f.render_widget(Paragraph::new(header), rows[0]);

    // body
    let wide = rows[1].width >= 100;
    let cols = if wide { Layout::default().direction(Direction::Horizontal).constraints([Constraint::Percentage(70), Constraint::Percentage(30)]).split(rows[1]) } else { Layout::default().direction(Direction::Horizontal).constraints([Constraint::Percentage(100)]).split(rows[1]) };
    let chat = cols[0];
    let all = chat_lines(app, chat.width as usize);
    let h = chat.height as usize;
    let max_scroll = all.len().saturating_sub(h);
    app.scroll = app.scroll.min(max_scroll);
    let start = all.len().saturating_sub(h + app.scroll);
    let visible: Vec<Line> = all.into_iter().skip(start).take(h).collect();
    f.render_widget(Paragraph::new(visible), chat);

    if wide {
        let side = Layout::default().direction(Direction::Vertical).constraints([Constraint::Percentage(40), Constraint::Percentage(30), Constraint::Percentage(30)]).split(cols[1]);
        let vlines: Vec<Line> = app.verify.iter().rev().take(side[0].height.saturating_sub(2) as usize).rev().map(|(t, s)| Line::from(Span::styled(t.clone(), if s == "fail" { fg(Color::Red) } else if s == "skipped" { dim() } else { fg(Color::Green) }))).collect();
        let flines: Vec<Line> = app.files.iter().take(side[1].height.saturating_sub(2) as usize).map(|t| Line::from(Span::raw(t.clone()))).collect();
        let slines: Vec<Line> = app.skills.iter().rev().take(side[2].height.saturating_sub(2) as usize).rev().map(|t| Line::from(Span::styled(t.clone(), fg(Color::Yellow)))).collect();
        for (i, (lines, title)) in [side_block(" Verification ", vlines, "no checks yet"), side_block(" Changed files ", flines, "none"), side_block(" Skills ", slines, "none")].into_iter().enumerate() {
            f.render_widget(Paragraph::new(lines).block(Block::default().borders(Borders::ALL).title(title)), side[i]);
        }
    }

    // composer
    let title = if app.busy { " Task — Esc stops the running task ".to_string() } else { " Task — Enter sends, Alt+Enter new line ".to_string() };
    let input_view: Vec<Line> = app.input.split('\n').map(|l| Line::from(l.to_string())).collect();
    f.render_widget(Paragraph::new(input_view).block(Block::default().borders(Borders::ALL).title(title)), rows[2]);
    if !app.has_modal() {
        let before: String = app.input.chars().take(app.cursor).collect();
        let line_idx = before.matches('\n').count() as u16;
        let col = before.rsplit('\n').next().map(|s| s.chars().count()).unwrap_or(0) as u16;
        f.set_cursor_position((rows[2].x + 1 + col.min(rows[2].width.saturating_sub(3)), rows[2].y + 1 + line_idx.min(input_lines - 1)));
    }

    // footer
    f.render_widget(Paragraph::new(Line::from(Span::styled(" Tab approvals · Ctrl+A autonomous · Ctrl+T thinking · PgUp/PgDn scroll · Esc stop · F1 help · Ctrl+D quit", dim()))), rows[3]);

    // modal
    if app.has_modal() {
        let r = centered(area, 80, 70);
        f.render_widget(Clear, r);
        let (title, lines) = modal_lines(app, r.width as usize);
        f.render_widget(Paragraph::new(lines).block(Block::default().borders(Borders::ALL).title(title).border_style(fg(Color::Cyan))), r);
    }
}

fn modal_lines(app: &App, width: usize) -> (String, Vec<Line<'static>>) {
    let w = width.saturating_sub(4).max(10);
    let mut l: Vec<Line<'static>> = Vec::new();
    let mut add = |s: &str, st: Style| {
        for x in wrap_text(s, w) {
            l.push(Line::from(Span::styled(x, st)));
        }
    };
    let title = match &app.modal {
        Modal::Plan { plan, trivial, .. } => {
            add(plan, Style::default());
            add("", Style::default());
            add(if *trivial { "Enter = go   n = cancel" } else { "Enter = approve   n = cancel   e = give feedback" }, bold());
            if *trivial { " Plan (small change) " } else { " Plan " }.to_string()
        }
        Modal::Feedback { input, .. } => {
            add("Tell me what to change in the plan, then press Enter (Esc = cancel):", Style::default());
            add(&format!("> {input}▏"), bold());
            " Plan feedback ".to_string()
        }
        Modal::Questions { questions, idx, input, .. } => {
            add(&format!("Question {} of {}", idx + 1, questions.len()), dim());
            add(&questions[(*idx).min(questions.len() - 1)], bold());
            add(&format!("> {input}▏"), Style::default());
            add("Enter = next (empty = decide for me)   Esc = skip all", dim());
            " A few questions ".to_string()
        }
        Modal::Confirm { tool, args, .. } => {
            add(&format!("{tool} wants to run:"), Style::default());
            add(args, dim());
            add("", Style::default());
            add("y = allow once   n = deny", bold());
            " Approval needed ".to_string()
        }
        Modal::Reuse { offers, rows, cursor, chosen, .. } => {
            add("Skills from your other projects match this stack. Reuse them here?", Style::default());
            let mut last = usize::MAX;
            for (n, (i, j)) in rows.iter().enumerate() {
                if *i != last {
                    add(&format!("\"{}\" ({} skills)", offers[*i].from_label, offers[*i].skills.len()), bold());
                    last = *i;
                }
                let s = &offers[*i].skills[*j];
                add(&format!("{} [{}] {} — {}", if n == *cursor { ">" } else { " " }, if chosen[n] { "x" } else { " " }, s.name, s.summary), if n == *cursor { fg(Color::Cyan) } else { Style::default() });
            }
            add("↑/↓ move   Space toggle   a = all   Enter = confirm   n = none", bold());
            " Reuse skills? ".to_string()
        }
        Modal::Help => {
            for s in ["Enter          send the task", "Alt+Enter      new line (Ctrl+J also works)", "Tab            cycle approvals: plan → ask → auto-edit → full auto", "Ctrl+A         toggle Autonomous (no questions, up to 5 verification rounds)", "Ctrl+T         show/hide the model's thinking", "PgUp / PgDn    scroll the conversation", "Esc            stop the running task", "Ctrl+C         stop the task; press again when idle to quit", "Ctrl+D         quit (when the input is empty)", "/sessions      list this repository's task sessions", "/search <words> search past tasks", "/resume [id]   continue an interrupted task with its stored plan", "/exit          quit"] {
                add(s, Style::default());
            }
            add("Press any key to close.", dim());
            " Keys ".to_string()
        }
        Modal::None => String::new(),
    };
    (title, l)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::NoticeKind;
    use crate::tui::io::UiEvent;
    use crate::types::Mode;
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    fn screen(app: &mut App, w: u16, h: u16) -> String {
        let mut t = Terminal::new(TestBackend::new(w, h)).unwrap();
        t.draw(|f| draw(f, app)).unwrap();
        let buf = t.backend().buffer().clone();
        (0..buf.area.height).map(|y| (0..buf.area.width).map(|x| buf[(x, y)].symbol().to_string()).collect::<String>()).collect::<Vec<_>>().join("\n")
    }

    #[test]
    fn renders_header_conversation_and_side_panels() {
        let mut app = App::new("qwen-mock", "http://x/v1", "/w", Mode::AutoEdit, false);
        app.items.push(Item::User("fix the bug".into()));
        app.apply(UiEvent::Notice(NoticeKind::Skill, "using global skill \"backend-senior\"".into()));
        app.apply(UiEvent::Notice(NoticeKind::Verify, "round 1: pass".into()));
        app.items.push(Item::Final { verdict: "pass".into(), text: "Verified\n\nFixed it.".into(), meta: "1.0s".into() });
        app.files = vec!["mathx.py".into()];
        let s = screen(&mut app, 120, 30);
        for want in ["Frankenstein", "qwen-mock", "Guided", "auto-edit", "fix the bug", "backend-senior", "Verified", "Fixed it.", "Verification", "Changed files", "mathx.py", "Skills"] {
            assert!(s.contains(want), "missing {want:?} in:\n{s}");
        }
    }

    #[test]
    fn narrow_terminal_drops_the_side_panel_and_modal_is_drawn_on_top() {
        let mut app = App::new("m", "e", "/w", Mode::Plan, true);
        app.items.push(Item::User("x".into()));
        let s = screen(&mut app, 70, 24);
        assert!(!s.contains("Changed files") && s.contains("Autonomous") && s.contains("plan (read-only)"));
        let (tx, _rx) = tokio::sync::oneshot::channel();
        app.modal = Modal::Plan { plan: "1. change the loop  [mathx.py]".into(), trivial: false, reply: Some(tx) };
        let s = screen(&mut app, 100, 30);
        assert!(s.contains("Plan") && s.contains("change the loop") && s.contains("Enter = approve"), "{s}");
    }

    #[test]
    fn long_output_scrolls_and_follows_the_tail() {
        let mut app = App::new("m", "e", "/w", Mode::AutoEdit, false);
        for i in 0..80 {
            app.items.push(Item::Notice(NoticeKind::Info, format!("line number {i}")));
        }
        let s = screen(&mut app, 80, 20);
        assert!(s.contains("line number 79") && !s.contains("line number 5\n"), "{s}");
        app.scroll = 30;
        let s = screen(&mut app, 80, 20);
        assert!(!s.contains("line number 79"));
    }
}
