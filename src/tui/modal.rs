//! Modals: the confirmations Go asked on the command line (REQ-11, REQ-12).

use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::Rect as TRect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Clear, Paragraph};

use crate::actions::{Project, RmPlan};
use crate::connectors::Pr;
use crate::core::registry::Entry;
use crate::theme::p;

// There is at most one modal at a time, so its size doesn't matter.
#[allow(clippy::large_enum_variant)]
pub enum Modal {
    /// Closing a workspace that still runs something other than a shell,
    /// or by closing its last pane.
    Close {
        entry: Entry,
        running: Vec<String>,
        last: bool,
    },
    /// Closing a pane whose program isn't a shell.
    ClosePane {
        pane: crate::proto::PaneId,
        title: String,
        running: String,
    },
    /// `^␣ r`: a new name for a worktree, with what changes (REQ-40).
    Rename {
        entry: Entry,
        project: Project,
        text: String,
        /// The plan for `text`, or why it can't be.
        plan: Result<crate::actions::Renamed, String>,
    },
    /// `^␣ n`: a name for the pane; empty goes back to its automatic title.
    Name {
        pane: crate::proto::PaneId,
        text: String,
    },
    /// `done`: its PR is merged and nothing would be lost; one yes deletes it.
    Done {
        entry: Entry,
        project: Project,
        plan: RmPlan,
        pr: Pr,
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

/// The longest pane name, so it fits a pane's title.
const NAME_MAX: usize = 24;

/// What a key did to the modal.
pub enum Outcome {
    Stay,
    Cancel,
    Submit,
}

impl Modal {
    pub fn key(&mut self, k: KeyEvent) -> Outcome {
        if k.code == KeyCode::Esc {
            return Outcome::Cancel;
        }
        match self {
            Modal::Done { .. } => match k.code {
                KeyCode::Char('y') | KeyCode::Enter => Outcome::Submit,
                KeyCode::Char('n') => Outcome::Cancel,
                _ => Outcome::Stay,
            },
            Modal::Close { .. } | Modal::ClosePane { .. } => match k.code {
                KeyCode::Char('y') | KeyCode::Enter => Outcome::Submit,
                KeyCode::Char('n') => Outcome::Cancel,
                _ => Outcome::Stay,
            },
            Modal::Rename {
                entry,
                project,
                text,
                plan,
            } => {
                match k.code {
                    KeyCode::Enter if plan.is_ok() && *text != entry.name => {
                        return Outcome::Submit;
                    }
                    KeyCode::Backspace => {
                        text.pop();
                    }
                    KeyCode::Char(c) if text.chars().count() < 40 => text.push(c),
                    _ => return Outcome::Stay,
                }
                *plan = crate::core::registry::default_path()
                    .and_then(|reg| crate::actions::rename_plan(project, entry, text, &reg))
                    .map_err(|e| format!("{e:#}"));
                Outcome::Stay
            }
            Modal::Name { text, .. } => {
                match k.code {
                    KeyCode::Enter => return Outcome::Submit,
                    KeyCode::Backspace => {
                        text.pop();
                    }
                    KeyCode::Char(c) if text.chars().count() < NAME_MAX => text.push(c),
                    _ => {}
                }
                Outcome::Stay
            }
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
            Modal::Done {
                entry, plan, pr, ..
            } => {
                let mut l = vec![
                    Line::from(Span::styled(
                        format!(" PR #{} merged", pr.number),
                        Style::default().fg(p().green),
                    )),
                    Line::from(Span::styled(
                        format!(" {}", pr.url),
                        Style::default().fg(p().dim),
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
            Modal::Close {
                entry,
                running,
                last,
            } => {
                let mut l = Vec::new();
                if *last {
                    l.push(Line::from(
                        " This is its last pane, so the workspace closes too.",
                    ));
                    l.push(Line::default());
                }
                if !running.is_empty() {
                    l.push(Line::from(" Still running:"));
                    l.extend(running.iter().map(|r| {
                        Line::from(Span::styled(
                            format!("   {r}"),
                            Style::default().fg(p().yellow),
                        ))
                    }));
                    l.push(Line::default());
                    l.push(Line::from(Span::styled(
                        " Closing stops them.",
                        Style::default().fg(p().dim),
                    )));
                }
                l.push(Line::from(Span::styled(
                    " The folder stays on disk.",
                    Style::default().fg(p().dim),
                )));
                l.push(Line::default());
                l.push(keys(&[("y", "close"), ("esc", "cancel")]));
                (format!(" Close {}? ", entry.name), l)
            }
            Modal::ClosePane { title, running, .. } => {
                let l = vec![
                    Line::from(vec![
                        Span::raw(" "),
                        Span::styled(running.clone(), Style::default().fg(p().yellow)),
                        Span::raw(" is still running in it. Closing stops it."),
                    ]),
                    Line::default(),
                    keys(&[("y", "close"), ("esc", "cancel")]),
                ];
                (format!(" Close {title}? "), l)
            }
            Modal::Rename {
                entry, text, plan, ..
            } => {
                let row = |label: &str, old: String, new: String| {
                    Line::from(vec![
                        Span::styled(format!(" {label:<8}"), Style::default().fg(p().dim)),
                        Span::styled(old, Style::default().fg(p().dim)),
                        Span::raw(" → "),
                        Span::styled(new, Style::default().fg(p().green)),
                    ])
                };
                let mut l = vec![
                    Line::from(vec![
                        Span::styled(" name    ", Style::default().fg(p().dim)),
                        Span::raw(text.clone()),
                        Span::styled("▏", Style::default().fg(p().blue)),
                    ]),
                    Line::default(),
                ];
                match plan {
                    Ok(r) => {
                        l.push(row("branch", entry.branch.clone(), r.branch.clone()));
                        l.push(row(
                            "folder",
                            short_path(&entry.path, 26),
                            short_path(&r.path.display().to_string(), 26),
                        ));
                        l.push(Line::default());
                        l.push(Line::from(Span::styled(
                            " Its panes start again in the new folder.",
                            Style::default().fg(p().dim),
                        )));
                    }
                    Err(e) => l.push(Line::from(Span::styled(
                        format!(" {e}"),
                        Style::default().fg(p().red),
                    ))),
                }
                l.push(Line::default());
                l.push(keys(&[("↵", "rename"), ("esc", "cancel")]));
                (format!(" Rename {} ", entry.name), l)
            }
            Modal::Name { text, .. } => {
                let hint = if text.is_empty() {
                    " Empty: the title follows what runs in it."
                } else {
                    " Stays until you change it."
                };
                let l = vec![
                    Line::from(vec![
                        Span::styled(" › ", Style::default().fg(p().blue)),
                        Span::raw(text.clone()),
                        Span::styled("▏", Style::default().fg(p().blue)),
                    ]),
                    Line::default(),
                    Line::from(Span::styled(hint, Style::default().fg(p().dim))),
                    Line::default(),
                    keys(&[("↵", "save"), ("esc", "cancel")]),
                ];
                (" Name this pane ".to_string(), l)
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
            .border_style(Style::default().fg(p().blue))
            .title(Span::styled(
                title,
                Style::default().fg(p().blue).add_modifier(Modifier::BOLD),
            ))
            .style(Style::default().bg(p().panel).fg(p().fg));
        let inner = block.inner(r);
        f.render_widget(block, r);
        f.render_widget(Paragraph::new(lines), inner);
    }
}

fn rm_lines(
    entry: &Entry,
    plan: &RmPlan,
    typed: &str,
    error: &Option<String>,
) -> Vec<Line<'static>> {
    let mut l = vec![Line::from(Span::styled(
        format!(" {}  ·  slot {}", entry.branch, entry.slot),
        Style::default().fg(p().dim),
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
                Style::default().fg(p().dim),
            )));
        }
    };
    if !plan.dirty.is_empty() {
        l.push(Line::from(Span::styled(
            format!(" {} uncommitted file(s) will be lost:", plan.dirty.len()),
            Style::default().fg(p().yellow),
        )));
        some(&mut l, &plan.dirty);
    }
    if !plan.unpushed.is_empty() {
        l.push(Line::from(Span::styled(
            format!(
                " {} commit(s) exist only on this machine:",
                plan.unpushed.len()
            ),
            Style::default().fg(p().yellow),
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
            Style::default().fg(p().dim),
        )));
    }
    l.push(Line::from(Span::styled(
        " The remote branch is untouched.",
        Style::default().fg(p().dim),
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
            Span::styled(format!("{typed}▏"), Style::default().fg(p().blue)),
        ]));
        l.push(Line::default());
    }
    if let Some(e) = error {
        l.push(Line::from(Span::styled(
            format!(" {e}"),
            Style::default().fg(p().yellow),
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
            Style::default().bg(p().line).fg(p().blue),
        ));
        spans.push(Span::styled(
            format!(" {what}   "),
            Style::default().fg(p().dim),
        ));
    }
    Line::from(spans)
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
