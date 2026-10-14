//! Closing a pane, closing a workspace and removing a worktree in the terminal client
//! (prototype: `docs/prototypes/tui-manage.html`). Each is a question the person answers
//! before anything is sent. This holds the question and what each key does; `View` sends
//! what comes out and the daemon decides: it never removes a worktree by force, so a
//! worktree with changes is refused there as well as here.

use contract::themes;
use contract::{Diff, FileStatus};
use crossterm::event::KeyCode;

use super::settings::{Mode, Settings};
use super::{Grid, Rect, Rgb, Style, clip, look};

/// The workspace a question is about, with what the question says of it.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Target {
    pub workspace: String,
    pub name: String,
    pub path: String,
    pub branch: Option<String>,
    pub worktree: bool,
    pub panes: usize,
    /// What runs in its panes (a program, or a command still going).
    pub running: Vec<String>,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Manage {
    /// `x` on a pane where something runs: closing it stops that.
    Pane {
        pane: String,
        title: String,
        what: String,
    },
    /// `X`.
    Workspace(Target),
    /// `d` in `Workspace`: the changes of the worktree are being read.
    Checking(Target),
    /// The worktree has changes: it cannot be removed.
    Dirty(Target, Vec<String>),
    /// The worktree has none: it can.
    Clean(Target),
    /// `r`: the name of a workspace or a panel being typed.
    Rename {
        target: Named,
        title: String,
        text: String,
    },
    /// The daemon refused, or could not be asked.
    Failed { title: String, text: String },
    /// `,`: the mode and the themes of the kind in force; `cursor` is the one the keys are on
    /// and `shown` how many there are.
    Settings { cursor: usize, shown: usize },
}

/// What a name is for.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Named {
    Workspace(String),
    Pane(String),
}

/// What a key asks the client to send.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Act {
    ClosePane(String),
    RemoveWorkspace {
        workspace: String,
        remove_worktree: bool,
    },
    /// Read the workspace's `uncommitted` changes.
    Check(String),
    /// Give a workspace or a panel this name (empty: the automatic one).
    Rename {
        target: Named,
        name: String,
    },
    /// Use the theme at this place in the list of the kind in force.
    Theme(usize),
    /// The next mode, or the one before it.
    StepMode(bool),
}

impl Manage {
    /// The question after `code`, and what to send. `None` closes it.
    pub fn press(self, code: KeyCode) -> (Option<Manage>, Option<Act>) {
        let esc = code == KeyCode::Esc;
        let key = |c: char| code == KeyCode::Char(c);
        match self {
            Manage::Rename {
                target,
                title,
                mut text,
            } => match code {
                KeyCode::Esc => (None, None),
                KeyCode::Enter => (None, Some(Act::Rename { target, name: text })),
                KeyCode::Backspace => {
                    text.pop();
                    (
                        Some(Manage::Rename {
                            target,
                            title,
                            text,
                        }),
                        None,
                    )
                }
                KeyCode::Char(c) => {
                    text.push(c);
                    (
                        Some(Manage::Rename {
                            target,
                            title,
                            text,
                        }),
                        None,
                    )
                }
                _ => (
                    Some(Manage::Rename {
                        target,
                        title,
                        text,
                    }),
                    None,
                ),
            },
            Manage::Settings { cursor, shown } => {
                let here = |cursor| Some(Manage::Settings { cursor, shown });
                match code {
                    KeyCode::Esc | KeyCode::Char(',' | 'q') => (None, None),
                    KeyCode::Char('j') | KeyCode::Down => {
                        (here((cursor + 1).min(shown.saturating_sub(1))), None)
                    }
                    KeyCode::Char('k') | KeyCode::Up => (here(cursor.saturating_sub(1)), None),
                    KeyCode::Enter => (here(cursor), Some(Act::Theme(cursor))),
                    KeyCode::Char('m' | 'l') | KeyCode::Right => {
                        (here(cursor), Some(Act::StepMode(true)))
                    }
                    KeyCode::Char('h') | KeyCode::Left => {
                        (here(cursor), Some(Act::StepMode(false)))
                    }
                    _ => (here(cursor), None),
                }
            }
            // Anything but `y` leaves the pane alone.
            Manage::Pane { pane, .. } if key('y') => (None, Some(Act::ClosePane(pane))),
            Manage::Pane { .. } => (None, None),
            Manage::Workspace(t) if key('y') => {
                let act = remove(&t, false);
                (None, Some(act))
            }
            Manage::Workspace(t) if key('d') && t.worktree => {
                let ws = t.workspace.clone();
                (Some(Manage::Checking(t)), Some(Act::Check(ws)))
            }
            Manage::Workspace(_) if esc => (None, None),
            Manage::Checking(t) | Manage::Clean(t) if esc => (Some(Manage::Workspace(t)), None),
            Manage::Dirty(t, _) if esc => (Some(Manage::Workspace(t)), None),
            Manage::Dirty(t, _) if key('c') => {
                let act = remove(&t, false);
                (None, Some(act))
            }
            Manage::Clean(t) if key('y') => {
                let act = remove(&t, true);
                (None, Some(act))
            }
            Manage::Failed { .. } => (None, None),
            other => (Some(other), None),
        }
    }

    /// The question is still about the worktree's changes: their node is open.
    pub fn is_check(&self) -> bool {
        matches!(
            self,
            Manage::Checking(_) | Manage::Dirty(..) | Manage::Clean(_)
        )
    }

    /// The mode chip's hint for this question.
    pub fn hint(&self) -> &'static str {
        match self {
            Manage::Pane { .. } => "y cerrar · esc cancelar",
            Manage::Workspace(t) if t.worktree => {
                "y cerrar el workspace · d quitar el worktree · esc cancelar"
            }
            Manage::Workspace(_) => "y cerrar · esc cancelar",
            Manage::Checking(_) => "esc cancelar",
            Manage::Dirty(..) => "c cerrar solo el workspace · esc volver",
            Manage::Clean(_) => "y quitar · esc cancelar",
            Manage::Failed { .. } => "cualquier tecla para seguir",
            Manage::Settings { .. } => "j/k tema · m modo · ↵ elegir · esc cerrar",
            Manage::Rename { .. } => "↵ aceptar · esc cancelar (vacío: el nombre automático)",
        }
    }

    /// A question that has read the worktree's changes: with files it cannot go on.
    pub fn with_changes(t: Target, diff: &Diff) -> Manage {
        if diff.files.is_empty() && diff.omitted.unwrap_or(0) == 0 {
            return Manage::Clean(t);
        }
        let mut lines: Vec<String> = diff
            .files
            .iter()
            .map(|f| format!("{} {}", letter(f.status), f.path))
            .collect();
        if let Some(n) = diff.omitted.filter(|n| *n > 0) {
            lines.push(format!("…y {n} más"));
        }
        Manage::Dirty(t, lines)
    }

    /// Title, border colour and the lines of the box.
    fn text(&self, settings: &Settings, os_dark: bool) -> (String, Rgb, Vec<(String, Look)>) {
        let plain = |s: &str| (s.to_owned(), Look::Plain);
        let dim = |s: &str| (s.to_owned(), Look::Dim);
        let blank = || (String::new(), Look::Plain);
        match self {
            Manage::Pane { title, what, .. } => (
                format!("Cerrar {title}?"),
                look::yellow(),
                vec![
                    plain(&format!("{what} sigue corriendo en este panel.")),
                    dim("Cerrarlo lo detiene."),
                    blank(),
                    dim(self.hint()),
                ],
            ),
            Manage::Workspace(t) => {
                let ends = match t.panes {
                    0 => "No tiene paneles abiertos.".to_owned(),
                    1 => "Se termina su panel".to_owned(),
                    n => format!("Se terminan sus {n} paneles"),
                };
                let first = match (t.panes, t.running.is_empty()) {
                    (0, _) => ends,
                    (_, true) => format!("{ends}."),
                    (_, false) => format!("{ends} ({} sigue corriendo).", t.running.join(", ")),
                };
                let stays = if t.worktree {
                    "El worktree y su carpeta se quedan en el disco."
                } else {
                    "La carpeta se queda en el disco."
                };
                (
                    format!("Cerrar {}?", t.name),
                    look::yellow(),
                    vec![plain(&first), dim(stays), blank(), dim(self.hint())],
                )
            }
            Manage::Checking(t) => (
                format!("Quitar el worktree {}?", t.name),
                look::red(),
                vec![
                    plain("Mirando si tiene cambios sin commitear…"),
                    blank(),
                    dim(self.hint()),
                ],
            ),
            Manage::Dirty(t, files) => {
                let shown = files.len().min(MAX_FILES);
                let mut lines = vec![plain(&format!(
                    "Tiene {} sin commitear:",
                    if files.len() == 1 {
                        "un archivo".to_owned()
                    } else {
                        format!("{} archivos", files.len())
                    }
                ))];
                lines.extend(
                    files[..shown]
                        .iter()
                        .map(|f| (format!("  {f}"), Look::Warn)),
                );
                if files.len() > shown {
                    lines.push(dim(&format!("  …y {} más", files.len() - shown)));
                }
                lines.extend([
                    blank(),
                    dim("Commitéalos o descártalos en un panel: no se borra nada a la fuerza."),
                    dim("La rama y sus commits se quedan."),
                    blank(),
                    dim(self.hint()),
                ]);
                (format!("No se puede quitar {}", t.name), look::red(), lines)
            }
            Manage::Clean(t) => {
                let branch = t
                    .branch
                    .as_deref()
                    .map_or("La rama".to_owned(), |b| format!("La rama {b}"));
                (
                    format!("Quitar el worktree {}?", t.name),
                    look::red(),
                    vec![
                        plain(&format!("Se borra la carpeta {}.", t.path)),
                        dim(&format!("No hay cambios sin commitear. {branch} se queda.")),
                        blank(),
                        dim(self.hint()),
                    ],
                )
            }
            Manage::Settings { cursor, .. } => {
                let dark = settings.is_dark(os_dark);
                let modes = Mode::ALL
                    .iter()
                    .map(|m| {
                        if *m == settings.mode {
                            format!("[{}]", m.label())
                        } else {
                            format!(" {} ", m.label())
                        }
                    })
                    .collect::<Vec<_>>()
                    .join("  ");
                let kind = if dark { "oscuros" } else { "claros" };
                let heading = match settings.mode {
                    Mode::System => format!(
                        "Temas {kind} (la máquina está en {})",
                        if dark { "oscuro" } else { "claro" }
                    ),
                    _ => format!("Temas {kind}"),
                };
                let mut lines = vec![(format!("Modo   {modes}"), Look::Plain), dim(&heading)];
                let chosen_id = &settings.theme(os_dark).id;
                lines.extend(themes::of_kind(dark).enumerate().map(|(i, t)| {
                    let chosen = if t.id == *chosen_id { '●' } else { '○' };
                    let at = if i == *cursor { '❯' } else { ' ' };
                    let look = if i == *cursor {
                        Look::Accent
                    } else {
                        Look::Plain
                    };
                    (format!("{at} {chosen} {}", t.name), look)
                }));
                lines.extend([
                    blank(),
                    dim("Cada modo recuerda su tema."),
                    dim("Se aplica al momento y se guarda en settings.json."),
                ]);
                ("ajustes · tema".to_owned(), look::blue(), lines)
            }
            Manage::Rename { title, text, .. } => (
                title.clone(),
                look::blue(),
                vec![
                    dim("nombre"),
                    (format!("❯ {text}▏"), Look::Accent),
                    blank(),
                    dim(self.hint()),
                ],
            ),
            Manage::Failed { title, text } => {
                let mut lines: Vec<(String, Look)> = wrap(text, 70)
                    .into_iter()
                    .map(|l| (l, Look::Plain))
                    .collect();
                lines.extend([blank(), dim(self.hint())]);
                (title.clone(), look::red(), lines)
            }
        }
    }

    /// Draw the question over `area`, near its top and centred.
    pub fn draw(&self, g: &mut Grid, (x, y, w, _): Rect, settings: &Settings, os_dark: bool) {
        let (title, border, lines) = self.text(settings, os_dark);
        let widest = lines
            .iter()
            .map(|(l, _)| l.chars().count())
            .chain([title.chars().count() + 4])
            .max()
            .unwrap_or(0);
        let bw = (widest + 6).min(w.saturating_sub(2)).max(20);
        let bh = lines.len() + 4;
        let (bx, by) = (x + w.saturating_sub(bw) / 2, y + 2);
        for r in 0..bh {
            for c in 0..bw {
                if by + r < g.rows && bx + c < g.cols {
                    g.cells[(by + r) * g.cols + bx + c] = (' ', Style::default().on(look::panel()));
                }
            }
        }
        g.boxed(
            (bx, by, bw, bh),
            &title,
            border,
            Style::fg(look::fg()).bold(),
            None,
        );
        for (i, (text, tone)) in lines.iter().enumerate() {
            let style = match tone {
                Look::Plain => Style::fg(look::fg()),
                Look::Dim => Style::fg(look::dim()),
                Look::Warn => Style::fg(look::yellow()),
                Look::Accent => Style::fg(look::blue()).bold(),
            };
            g.put(
                bx + 3,
                by + 2 + i,
                &clip(text, bw.saturating_sub(6)),
                style.on(look::panel()),
            );
        }
        if let Manage::Settings { cursor, .. } = self {
            // The colours of the theme the keys are on, to see it before choosing it.
            if let Some(t) = themes::of_kind(settings.is_dark(os_dark)).nth(*cursor) {
                let p = t.palette;
                for (k, c) in [p.blue, p.green, p.yellow, p.red, p.magenta, p.cyan]
                    .into_iter()
                    .enumerate()
                {
                    let at = (bx + bw).saturating_sub(15) + 2 * k;
                    g.put(at, by + 4 + cursor, "██", Style::fg(c).on(look::panel()));
                }
            }
        }
    }
}

/// How many changed files a refusal lists before it says how many more.
const MAX_FILES: usize = 8;

#[derive(Clone, Copy)]
enum Look {
    Plain,
    Dim,
    Warn,
    Accent,
}

fn remove(t: &Target, remove_worktree: bool) -> Act {
    Act::RemoveWorkspace {
        workspace: t.workspace.clone(),
        remove_worktree,
    }
}

/// The letter `git status` would put before a file.
fn letter(status: FileStatus) -> char {
    match status {
        FileStatus::Added => 'A',
        FileStatus::Modified => 'M',
        FileStatus::Deleted => 'D',
        FileStatus::Renamed => 'R',
        FileStatus::Untracked => '?',
    }
}

/// `text` broken at spaces into lines of at most `width` characters.
fn wrap(text: &str, width: usize) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    for word in text.split_whitespace() {
        match lines.last_mut() {
            Some(l) if l.chars().count() + 1 + word.chars().count() <= width => {
                l.push(' ');
                l.push_str(word);
            }
            _ => lines.push(word.to_owned()),
        }
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    fn target(worktree: bool) -> Target {
        Target {
            workspace: "w".into(),
            name: "app: feat/login".into(),
            path: "/code/app-login".into(),
            branch: worktree.then(|| "feat/login".to_owned()),
            worktree,
            panes: 2,
            running: vec!["claude".into()],
        }
    }

    fn removal(worktree: bool) -> Act {
        Act::RemoveWorkspace {
            workspace: "w".into(),
            remove_worktree: worktree,
        }
    }

    #[test]
    fn a_running_pane_closes_only_on_y() {
        let q = Manage::Pane {
            pane: "p".into(),
            title: "claude".into(),
            what: "claude".into(),
        };
        assert_eq!(
            q.clone().press(KeyCode::Char('y')),
            (None, Some(Act::ClosePane("p".into())))
        );
        for other in [KeyCode::Esc, KeyCode::Char('n'), KeyCode::Enter] {
            assert_eq!(q.clone().press(other), (None, None), "{other:?}");
        }
    }

    #[test]
    fn closing_a_workspace_never_removes_its_worktree() {
        let q = Manage::Workspace(target(true));
        assert_eq!(q.press(KeyCode::Char('y')), (None, Some(removal(false))));
    }

    #[test]
    fn d_checks_the_changes_only_of_a_worktree() {
        let q = Manage::Workspace(target(true));
        let (next, act) = q.press(KeyCode::Char('d'));
        assert_eq!(act, Some(Act::Check("w".into())));
        assert_eq!(next, Some(Manage::Checking(target(true))));
        let q = Manage::Workspace(target(false));
        assert_eq!(
            q.clone().press(KeyCode::Char('d')),
            (Some(q), None),
            "a folder has no worktree to remove"
        );
    }

    #[test]
    fn a_worktree_with_changes_offers_only_to_close_the_workspace() {
        let q = Manage::Dirty(target(true), vec!["M src/a.rs".into()]);
        assert_eq!(q.clone().press(KeyCode::Char('y')), (Some(q.clone()), None));
        assert_eq!(q.clone().press(KeyCode::Char('d')), (Some(q.clone()), None));
        assert_eq!(
            q.clone().press(KeyCode::Char('c')),
            (None, Some(removal(false)))
        );
        assert_eq!(
            q.press(KeyCode::Esc),
            (Some(Manage::Workspace(target(true))), None)
        );
    }

    #[test]
    fn a_clean_worktree_is_removed_on_y() {
        let q = Manage::Clean(target(true));
        assert_eq!(
            q.clone().press(KeyCode::Char('y')),
            (None, Some(removal(true)))
        );
        assert_eq!(q.clone().press(KeyCode::Char('c')), (Some(q), None));
    }

    #[test]
    fn esc_while_reading_goes_back_and_a_refusal_goes_on_any_key() {
        assert_eq!(
            Manage::Checking(target(true)).press(KeyCode::Esc),
            (Some(Manage::Workspace(target(true))), None)
        );
        let f = Manage::Failed {
            title: "t".into(),
            text: "x".into(),
        };
        assert_eq!(f.press(KeyCode::Char('q')), (None, None));
    }

    #[test]
    fn wrapping_breaks_at_spaces() {
        assert_eq!(wrap("aa bb cc", 5), ["aa bb", "cc"]);
        assert_eq!(wrap("", 5), Vec::<String>::new());
    }
    #[test]
    fn the_settings_move_between_the_themes_shown_and_ask_for_what_is_chosen() {
        let at = |cursor| Manage::Settings { cursor, shown: 5 };
        assert_eq!(at(0).press(KeyCode::Char('k')), (Some(at(0)), None));
        assert_eq!(at(4).press(KeyCode::Char('j')), (Some(at(4)), None));
        assert_eq!(at(1).press(KeyCode::Char('j')), (Some(at(2)), None));
        assert_eq!(
            at(1).press(KeyCode::Enter),
            (Some(at(1)), Some(Act::Theme(1)))
        );
        assert_eq!(
            at(1).press(KeyCode::Char('m')),
            (Some(at(1)), Some(Act::StepMode(true)))
        );
        assert_eq!(
            at(1).press(KeyCode::Left),
            (Some(at(1)), Some(Act::StepMode(false)))
        );
        assert_eq!(at(1).press(KeyCode::Esc), (None, None));
        assert_eq!(at(1).press(KeyCode::Char('z')), (Some(at(1)), None));
    }
}
