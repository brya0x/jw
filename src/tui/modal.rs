//! Modals: the new-stream form and the confirmations Go asked on the
//! command line (REQ-11, REQ-12).

use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::Rect as TRect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Clear, Paragraph};

use super::draw::{DIM, FG, FOCUS, LINE, PANEL, WAIT};
use crate::actions::{Project, RmPlan};
use crate::connectors::Pr;
use crate::core::registry::Entry;

// There is at most one modal at a time, so its size doesn't matter.
#[allow(clippy::large_enum_variant)]
pub enum Modal {
    New(NewForm),
    /// Closing a stream that still runs something other than a shell.
    Close {
        entry: Entry,
        running: Vec<String>,
    },
    /// `done`: its PR is merged and nothing would be lost; one yes deletes it.
    Done {
        entry: Entry,
        project: Project,
        plan: RmPlan,
        pr: Pr,
    },
    /// Pick one of several: a dev service, a Markdown file.
    Pick {
        title: String,
        items: Vec<String>,
        cursor: usize,
        entry: Entry,
        purpose: PickFor,
    },
    /// Read-only rows: info.
    Info {
        title: String,
        rows: Vec<(String, String)>,
    },
    /// `I`: a repository to add; without a config, one is drafted for it.
    AddProject {
        path: String,
        in_repo: bool,
        error: Option<String>,
    },
    /// `n` in the free space: name, directory, layout.
    NewFree {
        fields: [String; 2],
        field: usize,
        layout: usize,
        error: Option<String>,
    },
    /// Forgetting a free session: its panes go, its directory stays.
    RmFree {
        entry: Entry,
    },
    /// Text for the stream's agent.
    Prompt {
        entry: Entry,
        text: String,
    },
    /// `rm`: when it would lose work, the stream's name must be typed.
    Rm {
        entry: Entry,
        project: Project,
        plan: RmPlan,
        typed: String,
        error: Option<String>,
    },
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum PickFor {
    Dev,
    Read,
}

pub struct NewForm {
    pub project: Project,
    /// Name, branch, from.
    pub fields: [String; 3],
    pub field: usize,
    pub setup: bool,
    pub error: Option<String>,
}

/// What a key did to the modal.
pub enum Outcome {
    Stay,
    Cancel,
    Submit,
}

const LABELS: [&str; 3] = ["Name", "Branch", "From"];

impl NewForm {
    pub fn new(project: Project) -> Self {
        let setup = !project.cfg.setup.is_empty();
        Self {
            project,
            fields: Default::default(),
            field: 0,
            setup,
            error: None,
        }
    }

    /// What the empty branch and from fields stand for.
    fn hint(&self, i: usize) -> String {
        match i {
            1 => self
                .project
                .cfg
                .branch
                .replace("{name}", non_empty(&self.fields[0], "<name>")),
            2 => "origin/<default branch>".into(),
            _ => "lowercase, digits, dashes".into(),
        }
    }
}

fn non_empty<'a>(s: &'a str, or: &'a str) -> &'a str {
    if s.is_empty() { or } else { s }
}

impl Modal {
    pub fn key(&mut self, k: KeyEvent) -> Outcome {
        if k.code == KeyCode::Esc {
            return Outcome::Cancel;
        }
        match self {
            Modal::New(f) => {
                f.error = None;
                match k.code {
                    KeyCode::Enter => return Outcome::Submit,
                    KeyCode::Tab | KeyCode::Down => f.field = (f.field + 1) % 4,
                    KeyCode::BackTab | KeyCode::Up => f.field = (f.field + 3) % 4,
                    KeyCode::Char(' ') if f.field == 3 => f.setup = !f.setup,
                    KeyCode::Backspace if f.field < 3 => {
                        f.fields[f.field].pop();
                    }
                    KeyCode::Char('u') if k.modifiers.contains(KeyModifiers::CONTROL) => {
                        if f.field < 3 {
                            f.fields[f.field].clear();
                        }
                    }
                    KeyCode::Char(c) if f.field < 3 => f.fields[f.field].push(c),
                    _ => {}
                }
                Outcome::Stay
            }
            Modal::Pick { items, cursor, .. } => match k.code {
                KeyCode::Char('j') | KeyCode::Down => {
                    *cursor = (*cursor + 1).min(items.len().saturating_sub(1));
                    Outcome::Stay
                }
                KeyCode::Char('k') | KeyCode::Up => {
                    *cursor = cursor.saturating_sub(1);
                    Outcome::Stay
                }
                KeyCode::Enter => Outcome::Submit,
                _ => Outcome::Stay,
            },
            Modal::Info { .. } => match k.code {
                KeyCode::Char('q') | KeyCode::Enter => Outcome::Cancel,
                _ => Outcome::Stay,
            },
            Modal::AddProject {
                path,
                in_repo,
                error,
            } => {
                *error = None;
                match k.code {
                    KeyCode::Enter if !path.trim().is_empty() => return Outcome::Submit,
                    KeyCode::Tab => *in_repo = !*in_repo,
                    KeyCode::Backspace => {
                        path.pop();
                    }
                    KeyCode::Char('u') if k.modifiers.contains(KeyModifiers::CONTROL) => {
                        path.clear()
                    }
                    KeyCode::Char(c) => path.push(c),
                    _ => {}
                }
                Outcome::Stay
            }
            Modal::NewFree {
                fields,
                field,
                layout,
                error,
            } => {
                *error = None;
                match k.code {
                    KeyCode::Enter => return Outcome::Submit,
                    KeyCode::Tab | KeyCode::Down => *field = (*field + 1) % 3,
                    KeyCode::BackTab | KeyCode::Up => *field = (*field + 2) % 3,
                    KeyCode::Left | KeyCode::Right | KeyCode::Char(' ') if *field == 2 => {
                        let n = crate::free::Layout::ALL.len();
                        *layout = if k.code == KeyCode::Left {
                            (*layout + n - 1) % n
                        } else {
                            (*layout + 1) % n
                        };
                    }
                    KeyCode::Backspace if *field < 2 => {
                        fields[*field].pop();
                    }
                    KeyCode::Char(c) if *field < 2 => fields[*field].push(c),
                    _ => {}
                }
                Outcome::Stay
            }
            Modal::RmFree { .. } => match k.code {
                KeyCode::Char('y') | KeyCode::Enter => Outcome::Submit,
                KeyCode::Char('n') => Outcome::Cancel,
                _ => Outcome::Stay,
            },
            Modal::Done { .. } => match k.code {
                KeyCode::Char('y') | KeyCode::Enter => Outcome::Submit,
                KeyCode::Char('n') => Outcome::Cancel,
                _ => Outcome::Stay,
            },
            Modal::Prompt { text, .. } => {
                match k.code {
                    KeyCode::Enter if !text.trim().is_empty() => return Outcome::Submit,
                    KeyCode::Backspace => {
                        text.pop();
                    }
                    KeyCode::Char('u') if k.modifiers.contains(KeyModifiers::CONTROL) => {
                        text.clear()
                    }
                    KeyCode::Char(c) => text.push(c),
                    _ => {}
                }
                Outcome::Stay
            }
            Modal::Close { .. } => match k.code {
                KeyCode::Char('y') | KeyCode::Enter => Outcome::Submit,
                KeyCode::Char('n') => Outcome::Cancel,
                _ => Outcome::Stay,
            },
            Modal::Rm {
                plan, typed, error, ..
            } => {
                *error = None;
                match k.code {
                    KeyCode::Enter => Outcome::Submit,
                    KeyCode::Char('y') if !plan.loses_work() => Outcome::Submit,
                    KeyCode::Char('n') if !plan.loses_work() => Outcome::Cancel,
                    KeyCode::Backspace => {
                        typed.pop();
                        Outcome::Stay
                    }
                    KeyCode::Char(c) if plan.loses_work() => {
                        typed.push(c);
                        Outcome::Stay
                    }
                    _ => Outcome::Stay,
                }
            }
        }
    }

    pub fn draw(&self, f: &mut Frame) {
        let (title, lines) = match self {
            Modal::New(form) => (
                format!(" New stream · {} ", form.project.name),
                new_lines(form),
            ),
            Modal::Done {
                entry, plan, pr, ..
            } => {
                let mut l = vec![
                    Line::from(Span::styled(
                        format!(" PR #{} merged", pr.number),
                        Style::default().fg(super::draw::DEV),
                    )),
                    Line::from(Span::styled(
                        format!(" {}", pr.url),
                        Style::default().fg(DIM),
                    )),
                    Line::default(),
                    Line::from(" Deletes:"),
                ];
                for d in &plan.deletes {
                    l.push(Line::from(format!("   {}", short_path(d, 56))));
                }
                l.push(Line::default());
                l.push(keys(&[("y", "delete"), ("esc", "keep")]));
                (format!(" Done with {}? ", entry.name), l)
            }
            Modal::Pick {
                title,
                items,
                cursor,
                ..
            } => {
                let mut l = vec![Line::default()];
                for (i, it) in items.iter().enumerate() {
                    let on = i == *cursor;
                    l.push(Line::from(Span::styled(
                        format!(" {} {it}", if on { "▶" } else { " " }),
                        if on {
                            Style::default().fg(FOCUS).add_modifier(Modifier::BOLD)
                        } else {
                            Style::default()
                        },
                    )));
                }
                l.push(Line::default());
                l.push(keys(&[("j/k", "move"), ("↵", "open"), ("esc", "cancel")]));
                (format!(" {title} "), l)
            }
            Modal::Info { title, rows } => {
                let mut l = vec![Line::default()];
                let mut last = "";
                for (label, value) in rows {
                    let shown = if label == last { "" } else { label.as_str() };
                    last = label;
                    l.push(Line::from(vec![
                        Span::styled(format!(" {shown:<9}"), Style::default().fg(DIM)),
                        Span::raw(short_path(value, 52)),
                    ]));
                }
                l.push(Line::default());
                l.push(keys(&[("esc", "close")]));
                (format!(" {title} "), l)
            }
            Modal::AddProject {
                path,
                in_repo,
                error,
            } => {
                let mut l = vec![
                    Line::from(Span::styled(
                        " A git checkout. Without a jw config, one is drafted from it.",
                        Style::default().fg(DIM),
                    )),
                    Line::default(),
                    Line::from(vec![
                        Span::styled(" Repo  ", Style::default().fg(FOCUS)),
                        Span::styled("[ ", Style::default().fg(FOCUS)),
                        // The end of a long path is the part being typed.
                        Span::raw(tail(path, 46)),
                        Span::styled("▏ ]", Style::default().fg(FOCUS)),
                    ]),
                    Line::default(),
                    Line::from(format!(
                        " Draft goes to: {}",
                        if *in_repo {
                            "<repo>/.jw.toml (to commit)"
                        } else {
                            "~/.config/jw/<project>.toml (yours)"
                        }
                    )),
                    Line::default(),
                ];
                if let Some(e) = error {
                    l.push(Line::from(Span::styled(
                        format!(" {e}"),
                        Style::default().fg(WAIT),
                    )));
                    l.push(Line::default());
                }
                l.push(keys(&[("tab", "where"), ("↵", "add"), ("esc", "cancel")]));
                (" Add a project ".to_string(), l)
            }
            Modal::NewFree {
                fields,
                field,
                layout,
                error,
            } => {
                let mut l = vec![Line::from(Span::styled(
                    " No repository, branch or ports: a shell where you say.",
                    Style::default().fg(DIM),
                ))];
                l.push(Line::default());
                for (i, (label, hint)) in [("Name", "lowercase, digits, dashes"), ("Dir", "~")]
                    .iter()
                    .enumerate()
                {
                    let on = *field == i;
                    let c = if on { FOCUS } else { DIM };
                    let value = &fields[i];
                    l.push(Line::from(vec![
                        Span::styled(format!(" {label:<7}"), Style::default().fg(c)),
                        Span::styled("[ ", Style::default().fg(if on { FOCUS } else { LINE })),
                        if value.is_empty() {
                            Span::styled(hint.to_string(), Style::default().fg(DIM))
                        } else {
                            Span::raw(tail(value, 44))
                        },
                        Span::styled(
                            if on { "▏ ]" } else { " ]" },
                            Style::default().fg(if on { FOCUS } else { LINE }),
                        ),
                    ]));
                }
                let on = *field == 2;
                l.push(Line::from(vec![
                    Span::styled(
                        " Layout ",
                        Style::default().fg(if on { FOCUS } else { DIM }),
                    ),
                    Span::styled(
                        format!("‹ {} ›", crate::free::Layout::ALL[*layout].label()),
                        Style::default().fg(if on { FOCUS } else { FG }),
                    ),
                ]));
                l.push(Line::default());
                if let Some(e) = error {
                    l.push(Line::from(Span::styled(
                        format!(" {e}"),
                        Style::default().fg(WAIT),
                    )));
                    l.push(Line::default());
                }
                l.push(keys(&[
                    ("tab", "field"),
                    ("←→", "layout"),
                    ("↵", "create"),
                    ("esc", "cancel"),
                ]));
                (" New free session ".to_string(), l)
            }
            Modal::RmFree { entry } => {
                let l = vec![
                    Line::from(" Closes its panes and forgets it."),
                    Line::from(Span::styled(
                        format!(" {} is untouched.", short_path(&entry.path, 50)),
                        Style::default().fg(DIM),
                    )),
                    Line::default(),
                    keys(&[("y", "remove"), ("esc", "cancel")]),
                ];
                (format!(" Remove free/{}? ", entry.name), l)
            }
            Modal::Prompt { entry, text } => {
                let l = vec![
                    Line::from(Span::styled(
                        " Sent to the agent once its pane goes quiet.",
                        Style::default().fg(DIM),
                    )),
                    Line::default(),
                    Line::from(vec![
                        Span::styled(" › ", Style::default().fg(FOCUS)),
                        Span::raw(text.clone()),
                        Span::styled("▏", Style::default().fg(FOCUS)),
                    ]),
                    Line::default(),
                    keys(&[("↵", "send"), ("esc", "cancel")]),
                ];
                (format!(" Prompt {} ", entry.name), l)
            }
            Modal::Close { entry, running } => {
                let mut l = vec![Line::from("Still running:"), Line::default()];
                l.extend(running.iter().map(|r| {
                    Line::from(Span::styled(format!("  {r}"), Style::default().fg(WAIT)))
                }));
                l.push(Line::default());
                l.push(Line::from(Span::styled(
                    "Closing kills them. The worktree is kept.",
                    Style::default().fg(DIM),
                )));
                l.push(Line::default());
                l.push(keys(&[("y", "close"), ("esc", "cancel")]));
                (format!(" Close {}? ", entry.name), l)
            }
            Modal::Rm {
                entry,
                plan,
                typed,
                error,
                ..
            } => (
                format!(" Remove {}? ", entry.name),
                rm_lines(entry, plan, typed, error),
            ),
        };
        let area = f.area();
        let w = 64.min(area.width.saturating_sub(4));
        let h = (lines.len() as u16 + 2).min(area.height);
        let r = TRect::new(
            area.x + (area.width - w) / 2,
            area.y + area.height.saturating_sub(h) / 3,
            w,
            h,
        );
        f.render_widget(Clear, r);
        let block = Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(FOCUS))
            .title(Span::styled(
                title,
                Style::default().fg(FOCUS).add_modifier(Modifier::BOLD),
            ))
            .style(Style::default().bg(PANEL).fg(FG));
        let inner = block.inner(r);
        f.render_widget(block, r);
        f.render_widget(Paragraph::new(lines), inner);
    }
}

fn new_lines(form: &NewForm) -> Vec<Line<'static>> {
    let mut l = vec![Line::default()];
    for (i, label) in LABELS.iter().enumerate() {
        let on = form.field == i;
        let value = &form.fields[i];
        let mut spans = vec![
            Span::styled(
                format!(" {label:<7}"),
                Style::default().fg(if on { FOCUS } else { DIM }),
            ),
            Span::styled("[ ", Style::default().fg(if on { FOCUS } else { LINE })),
        ];
        if value.is_empty() {
            spans.push(Span::styled(form.hint(i), Style::default().fg(DIM)));
        } else {
            spans.push(Span::raw(value.clone()));
        }
        if on {
            spans.push(Span::styled("▏", Style::default().fg(FOCUS)));
        }
        spans.push(Span::styled(
            " ]",
            Style::default().fg(if on { FOCUS } else { LINE }),
        ));
        l.push(Line::from(spans));
    }
    l.push(Line::default());
    let on = form.field == 3;
    let setup = if form.project.cfg.setup.is_empty() {
        Span::styled(" no setup in the config", Style::default().fg(DIM))
    } else {
        Span::styled(
            format!(" [{}] run setup", if form.setup { "x" } else { " " }),
            Style::default().fg(if on { FOCUS } else { FG }),
        )
    };
    l.push(Line::from(setup));
    l.push(Line::default());
    if let Some(e) = &form.error {
        l.push(Line::from(Span::styled(
            format!(" {e}"),
            Style::default().fg(WAIT),
        )));
        l.push(Line::default());
    }
    l.push(keys(&[
        ("tab", "field"),
        ("␣", "toggle"),
        ("↵", "create"),
        ("esc", "cancel"),
    ]));
    l
}

fn rm_lines(
    entry: &Entry,
    plan: &RmPlan,
    typed: &str,
    error: &Option<String>,
) -> Vec<Line<'static>> {
    let mut l = vec![Line::from(Span::styled(
        format!(" {}  ·  slot {}", entry.branch, entry.slot),
        Style::default().fg(DIM),
    ))];
    l.push(Line::default());
    if !plan.on_disk {
        l.push(Line::from(format!(
            " worktree {} is already gone",
            entry.path
        )));
    }
    let some = |l: &mut Vec<Line<'static>>, items: &[String]| {
        for (i, s) in items.iter().enumerate() {
            if i == 5 {
                l.push(Line::from(format!("     … and {} more", items.len() - 5)));
                break;
            }
            l.push(Line::from(Span::styled(
                format!("     {}", s.trim()),
                Style::default().fg(DIM),
            )));
        }
    };
    if !plan.dirty.is_empty() {
        l.push(Line::from(Span::styled(
            format!(" {} uncommitted file(s) will be lost:", plan.dirty.len()),
            Style::default().fg(WAIT),
        )));
        some(&mut l, &plan.dirty);
    }
    if !plan.unpushed.is_empty() {
        l.push(Line::from(Span::styled(
            format!(
                " {} commit(s) exist only on this machine:",
                plan.unpushed.len()
            ),
            Style::default().fg(WAIT),
        )));
        some(&mut l, &plan.unpushed);
    }
    if !plan.deletes.is_empty() {
        if l.last().is_some_and(|x| x.width() > 0) {
            l.push(Line::default());
        }
        l.push(Line::from(" Deletes:"));
        for d in &plan.deletes {
            l.push(Line::from(format!("   {}", short_path(d, 56))));
        }
    }
    if plan.keep_branch {
        l.push(Line::from(Span::styled(
            format!(" Keeps branch {} (it existed before jw).", entry.branch),
            Style::default().fg(DIM),
        )));
    }
    l.push(Line::from(Span::styled(
        " The remote branch is untouched.",
        Style::default().fg(DIM),
    )));
    l.push(Line::default());
    if plan.loses_work() {
        l.push(Line::from(vec![
            Span::raw(" Type "),
            Span::styled(
                entry.name.clone(),
                Style::default().add_modifier(Modifier::BOLD),
            ),
            Span::raw(" to remove it anyway: "),
            Span::styled(format!("{typed}▏"), Style::default().fg(FOCUS)),
        ]));
        l.push(Line::default());
    }
    if let Some(e) = error {
        l.push(Line::from(Span::styled(
            format!(" {e}"),
            Style::default().fg(WAIT),
        )));
        l.push(Line::default());
    }
    if plan.loses_work() {
        l.push(keys(&[("↵", "remove"), ("esc", "cancel")]));
    } else {
        l.push(keys(&[("y", "remove"), ("esc", "cancel")]));
    }
    l
}

fn keys(pairs: &[(&str, &str)]) -> Line<'static> {
    let mut spans = vec![Span::raw(" ")];
    for (k, what) in pairs {
        spans.push(Span::styled(
            format!(" {k} "),
            Style::default().bg(LINE).fg(FOCUS),
        ));
        spans.push(Span::styled(
            format!(" {what}   "),
            Style::default().fg(DIM),
        ));
    }
    Line::from(spans)
}

/// The last `max` characters of `s`, with `…` when cut.
fn tail(s: &str, max: usize) -> String {
    let n = s.chars().count();
    if n <= max {
        return s.to_string();
    }
    format!("…{}", s.chars().skip(n - max + 1).collect::<String>())
}

/// Fits a line in `max` columns: `~` for the home directory, then the start
/// of the path cut down to `…`, so its end, the part that names it, shows.
fn short_path(s: &str, max: usize) -> String {
    let home = std::env::var("HOME").unwrap_or_default();
    let s = if home.len() > 1 {
        s.replace(&home, "~")
    } else {
        s.to_string()
    };
    if s.chars().count() <= max {
        return s;
    }
    let (head, path) = match s.split_once(' ') {
        Some((h, p)) => (format!("{h} "), p),
        None => (String::new(), s.as_str()),
    };
    let keep = max.saturating_sub(head.chars().count() + 1);
    let skip = path.chars().count().saturating_sub(keep);
    let tail: String = path.chars().skip(skip).collect();
    // Cut at a directory boundary rather than inside a name.
    let tail = tail.find('/').map_or(tail.as_str(), |i| &tail[i..]);
    format!("{head}…{tail}")
}

#[cfg(test)]
mod tests {
    use super::short_path;

    #[test]
    fn short_paths_keep_their_end() {
        assert_eq!(short_path("worktree /a/b", 40), "worktree /a/b");
        let got = short_path("worktree /very/long/path/to/myapp-wt/docs", 30);
        assert_eq!(got, "worktree …/to/myapp-wt/docs");
        assert!(got.chars().count() <= 30);
        assert_eq!(
            short_path("/very/long/path/to/myapp-wt/docs", 20),
            "…/to/myapp-wt/docs"
        );
    }
}
