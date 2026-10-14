//! Closing a pane, closing a workspace and removing a worktree in the terminal client
//! (prototype: `docs/prototypes/tui-manage.html`). Each is a question the person answers
//! before anything is sent. This holds the question and what each key does; `View` sends
//! what comes out and the daemon decides: it never removes a worktree by force, so a
//! worktree with changes is refused there as well as here.

use contract::{Diff, FileStatus};
use crossterm::event::KeyCode;

use super::{DIM, FG, Grid, PANEL, RED, Rect, Rgb, Style, YELLOW, clip};

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
    /// The daemon refused, or could not be asked.
    Failed { title: String, text: String },
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
}

impl Manage {
    /// The question after `code`, and what to send. `None` closes it.
    pub fn press(self, code: KeyCode) -> (Option<Manage>, Option<Act>) {
        let esc = code == KeyCode::Esc;
        let key = |c: char| code == KeyCode::Char(c);
        match self {
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
    fn text(&self) -> (String, Rgb, Vec<(String, Look)>) {
        let plain = |s: &str| (s.to_owned(), Look::Plain);
        let dim = |s: &str| (s.to_owned(), Look::Dim);
        let blank = || (String::new(), Look::Plain);
        match self {
            Manage::Pane { title, what, .. } => (
                format!("Cerrar {title}?"),
                YELLOW,
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
                    YELLOW,
                    vec![plain(&first), dim(stays), blank(), dim(self.hint())],
                )
            }
            Manage::Checking(t) => (
                format!("Quitar el worktree {}?", t.name),
                RED,
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
                (format!("No se puede quitar {}", t.name), RED, lines)
            }
            Manage::Clean(t) => {
                let branch = t
                    .branch
                    .as_deref()
                    .map_or("La rama".to_owned(), |b| format!("La rama {b}"));
                (
                    format!("Quitar el worktree {}?", t.name),
                    RED,
                    vec![
                        plain(&format!("Se borra la carpeta {}.", t.path)),
                        dim(&format!("No hay cambios sin commitear. {branch} se queda.")),
                        blank(),
                        dim(self.hint()),
                    ],
                )
            }
            Manage::Failed { title, text } => {
                let mut lines: Vec<(String, Look)> = wrap(text, 70)
                    .into_iter()
                    .map(|l| (l, Look::Plain))
                    .collect();
                lines.extend([blank(), dim(self.hint())]);
                (title.clone(), RED, lines)
            }
        }
    }

    /// Draw the question over `area`, near its top and centred.
    pub fn draw(&self, g: &mut Grid, (x, y, w, _): Rect) {
        let (title, border, lines) = self.text();
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
                    g.cells[(by + r) * g.cols + bx + c] = (' ', Style::default().on(PANEL));
                }
            }
        }
        g.boxed((bx, by, bw, bh), &title, border, Style::fg(FG).bold(), None);
        for (i, (text, look)) in lines.iter().enumerate() {
            let style = match look {
                Look::Plain => Style::fg(FG),
                Look::Dim => Style::fg(DIM),
                Look::Warn => Style::fg(YELLOW),
            };
            g.put(
                bx + 3,
                by + 2 + i,
                &clip(text, bw.saturating_sub(6)),
                style.on(PANEL),
            );
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
}
