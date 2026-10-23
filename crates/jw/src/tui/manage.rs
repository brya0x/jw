//! Closing a pane, closing a workspace and removing a worktree in the terminal client
//! (prototype: `docs/prototypes/tui-manage.html`). Each is a question the person answers
//! before anything is sent. This holds the question and what each key does; `View` sends
//! what comes out and the daemon decides: it never removes a worktree by force, so a
//! worktree with changes is refused there as well as here.

use contract::themes;
use contract::{Diff, FileStatus};
use crossterm::event::KeyCode;

use super::settings::{Mode, Settings};
use super::{Grid, Rect, Rgb, Style, clip, erase, look};

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
    /// Commits of its branch that no remote has: they stay in the branch when the worktree goes.
    pub unpushed: u64,
}

impl Target {
    /// What is typed to remove the worktree with what it holds: its branch, else its name.
    pub fn confirm(&self) -> &str {
        self.branch.as_deref().unwrap_or(&self.name)
    }

    /// The line that says what stays of the commits no remote has.
    fn unpushed_note(&self) -> Option<String> {
        let branch = self.confirm();
        match self.unpushed {
            0 => None,
            1 => Some(format!("1 commit sin subir se queda en la rama {branch}.")),
            n => Some(format!(
                "{n} commits sin subir se quedan en la rama {branch}."
            )),
        }
    }
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
    /// The worktree has changes that would be lost: it is removed only after the branch's name
    /// is typed, and then by force.
    Dirty {
        target: Target,
        files: Vec<String>,
        typed: String,
    },
    /// The worktree has none: it can.
    Clean(Target),
    /// `r`: the name of a workspace or a panel being typed.
    Rename {
        target: Named,
        title: String,
        text: String,
    },
    /// `s`: the branch is being pushed; `esc` asks the daemon to stop it (`cancelling` once it did)
    /// and every other key waits (REQ-604).
    Syncing {
        workspace: String,
        branch: String,
        cancelling: bool,
    },
    /// The daemon refused, or could not be asked.
    Failed { title: String, text: String },
    /// `,`: the mode and the themes of the kind in force; `cursor` is the one the keys are on
    /// and `shown` how many there are.
    Settings { cursor: usize, shown: usize },
    /// `G`, `r` on a group, and `n` in `MoveTo`: the name of a group being typed (REQ-694).
    GroupName {
        target: Naming,
        title: String,
        text: String,
    },
    /// `x` on a group: its workspaces stay and belong to none (REQ-687).
    DeleteGroup {
        group: String,
        name: String,
        members: usize,
    },
    /// `g` on a workspace: the list of groups to put its repository in. `cursor` counts the
    /// groups, then `sin grupo`, then `grupo nuevo…`; `current` is the group it is in now.
    MoveTo {
        workspace: String,
        name: String,
        scope: String,
        groups: Vec<(String, String)>,
        current: Option<String>,
        cursor: usize,
    },
    /// `A`: the archived conversations of their own, the most recent first (REQ-808).
    Archived {
        chats: Vec<(String, String)>,
        cursor: usize,
    },
    /// `d` in `Archived`: its folder goes with it; Claude's own transcript stays (REQ-808).
    DeleteChat { workspace: String, name: String },
    /// `P` on a conversation: the workspaces to take it to as a new conversation (REQ-809).
    ChatTo {
        chat: String,
        name: String,
        targets: Vec<(String, String)>,
        cursor: usize,
    },
}

/// The most characters the name of a group has (REQ-686): the box takes no more.
pub const MAX_GROUP_NAME: usize = 40;

/// What the name of a group typed in `GroupName` is for.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Naming {
    /// A new group, empty.
    New,
    /// A new group that this workspace goes into at once.
    NewWith(String),
    /// A group's new name; the id of the group.
    Rename(String),
}

/// Where `MoveTo` puts a workspace's repository.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Dest {
    Group(String),
    /// Out of every group.
    None,
    /// In a group to be made: its name is asked next.
    New,
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
        force: bool,
    },
    /// Read the workspace's `uncommitted` changes.
    Check(String),
    /// Give a workspace or a panel this name (empty: the automatic one).
    Rename {
        target: Named,
        name: String,
    },
    /// Stop what the daemon runs for this workspace: the push (REQ-604).
    Cancel(String),
    /// Use the theme at this place in the list of the kind in force.
    Theme(usize),
    /// The next mode, or the one before it.
    StepMode(bool),
    /// Name a group: make it, make it with a workspace in it, or rename it.
    GroupName {
        target: Naming,
        name: String,
    },
    DeleteGroup(String),
    /// Put the repository of this workspace there.
    MoveTo {
        workspace: String,
        to: Dest,
    },
    ResumeChat(String),
    DeleteChat(String),
    PromoteChat {
        chat: String,
        to: String,
    },
}

impl Manage {
    /// Option-backspace: the word before the cursor in the name being typed (a rename, the
    /// branch that confirms a removal); any other question is left as it is.
    pub fn erase_word(mut self) -> Manage {
        match &mut self {
            Manage::Rename { text, .. } => erase::pop_word(text),
            Manage::Dirty { typed, .. } => erase::pop_word(typed),
            _ => {}
        }
        self
    }

    /// The question after `code`, and what to send. `None` closes it.
    pub fn press(self, code: KeyCode) -> (Option<Manage>, Option<Act>) {
        let esc = code == KeyCode::Esc;
        let key = |c: char| code == KeyCode::Char(c);
        match self {
            Manage::Rename {
                target,
                title,
                mut text,
            } => {
                match code {
                    KeyCode::Esc => return (None, None),
                    KeyCode::Enter => return (None, Some(Act::Rename { target, name: text })),
                    KeyCode::Backspace => {
                        text.pop();
                    }
                    KeyCode::Char(c) => text.push(c),
                    _ => {}
                }
                (
                    Some(Manage::Rename {
                        target,
                        title,
                        text,
                    }),
                    None,
                )
            }
            Manage::GroupName {
                target,
                title,
                mut text,
            } => {
                match code {
                    KeyCode::Esc => return (None, None),
                    // A name has to be something: the box waits for it.
                    KeyCode::Enter if !text.trim().is_empty() => {
                        return (None, Some(Act::GroupName { target, name: text }));
                    }
                    KeyCode::Backspace => {
                        text.pop();
                    }
                    KeyCode::Char(c) if text.chars().count() < MAX_GROUP_NAME => text.push(c),
                    _ => {}
                }
                (
                    Some(Manage::GroupName {
                        target,
                        title,
                        text,
                    }),
                    None,
                )
            }
            Manage::DeleteGroup { group, .. } if key('y') => (None, Some(Act::DeleteGroup(group))),
            Manage::DeleteGroup { .. } if esc || key('n') => (None, None),
            Manage::MoveTo {
                workspace,
                name,
                scope,
                groups,
                current,
                cursor,
            } => {
                let last = groups.len() + 1;
                let pick = |at: usize| -> Option<Dest> {
                    match at {
                        a if a < groups.len() => Some(Dest::Group(groups[a].0.clone())),
                        a if a == groups.len() => Some(Dest::None),
                        _ => Some(Dest::New),
                    }
                };
                let to = match code {
                    KeyCode::Esc => return (None, None),
                    KeyCode::Enter => pick(cursor),
                    KeyCode::Char('0') => Some(Dest::None),
                    KeyCode::Char('n') => Some(Dest::New),
                    KeyCode::Char(c @ '1'..='9') if (c as usize - '1' as usize) < groups.len() => {
                        pick(c as usize - '1' as usize)
                    }
                    _ => None,
                };
                if let Some(to) = to {
                    // Where it already is, nothing is sent.
                    let same = match &to {
                        Dest::Group(g) => current.as_deref() == Some(g.as_str()),
                        Dest::None => current.is_none(),
                        Dest::New => false,
                    };
                    return (None, (!same).then_some(Act::MoveTo { workspace, to }));
                }
                let cursor = match code {
                    KeyCode::Char('j') | KeyCode::Down => (cursor + 1).min(last),
                    KeyCode::Char('k') | KeyCode::Up => cursor.saturating_sub(1),
                    _ => cursor,
                };
                (
                    Some(Manage::MoveTo {
                        workspace,
                        name,
                        scope,
                        groups,
                        current,
                        cursor,
                    }),
                    None,
                )
            }
            Manage::DeleteChat { workspace, .. } if key('y') => {
                (None, Some(Act::DeleteChat(workspace)))
            }
            // `esc` goes back to the list it came from only when it is asked again (`A`).
            Manage::DeleteChat { .. } if esc || key('n') => (None, None),
            Manage::Archived { chats, cursor } => {
                let at = |cursor| {
                    Some(Manage::Archived {
                        chats: chats.clone(),
                        cursor,
                    })
                };
                match code {
                    KeyCode::Esc | KeyCode::Char('q') => (None, None),
                    KeyCode::Enter => match chats.get(cursor) {
                        Some((id, _)) => (None, Some(Act::ResumeChat(id.clone()))),
                        None => (at(cursor), None),
                    },
                    KeyCode::Char('d') => match chats.get(cursor) {
                        Some((id, name)) => (
                            Some(Manage::DeleteChat {
                                workspace: id.clone(),
                                name: name.clone(),
                            }),
                            None,
                        ),
                        None => (at(cursor), None),
                    },
                    KeyCode::Char('j') | KeyCode::Down => {
                        (at((cursor + 1).min(chats.len().saturating_sub(1))), None)
                    }
                    KeyCode::Char('k') | KeyCode::Up => (at(cursor.saturating_sub(1)), None),
                    _ => (at(cursor), None),
                }
            }
            Manage::ChatTo {
                chat,
                name,
                targets,
                cursor,
            } => {
                let stay = |cursor| {
                    Some(Manage::ChatTo {
                        chat: chat.clone(),
                        name: name.clone(),
                        targets: targets.clone(),
                        cursor,
                    })
                };
                match code {
                    KeyCode::Esc | KeyCode::Char('q') => (None, None),
                    KeyCode::Enter => match targets.get(cursor) {
                        Some((to, _)) => (
                            None,
                            Some(Act::PromoteChat {
                                chat: chat.clone(),
                                to: to.clone(),
                            }),
                        ),
                        None => (stay(cursor), None),
                    },
                    KeyCode::Char(c @ '1'..='9') if (c as usize - '1' as usize) < targets.len() => {
                        let to = targets[c as usize - '1' as usize].0.clone();
                        (None, Some(Act::PromoteChat { chat, to }))
                    }
                    KeyCode::Char('j') | KeyCode::Down => (
                        stay((cursor + 1).min(targets.len().saturating_sub(1))),
                        None,
                    ),
                    KeyCode::Char('k') | KeyCode::Up => (stay(cursor.saturating_sub(1)), None),
                    _ => (stay(cursor), None),
                }
            }
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
                let act = remove(&t, false, false);
                (None, Some(act))
            }
            Manage::Workspace(t) if key('d') && t.worktree => {
                let ws = t.workspace.clone();
                (Some(Manage::Checking(t)), Some(Act::Check(ws)))
            }
            Manage::Workspace(_) if esc => (None, None),
            Manage::Checking(t) | Manage::Clean(t) if esc => (Some(Manage::Workspace(t)), None),
            Manage::Dirty { target, .. } if esc => (Some(Manage::Workspace(target)), None),
            Manage::Dirty {
                target,
                files,
                mut typed,
            } => {
                match code {
                    KeyCode::Enter if typed == target.confirm() => {
                        return (None, Some(remove(&target, true, true)));
                    }
                    KeyCode::Backspace => {
                        typed.pop();
                    }
                    KeyCode::Char(c) => typed.push(c),
                    _ => {}
                }
                (
                    Some(Manage::Dirty {
                        target,
                        files,
                        typed,
                    }),
                    None,
                )
            }
            Manage::Clean(t) if key('y') => {
                let act = remove(&t, true, false);
                (None, Some(act))
            }
            Manage::Failed { .. } => (None, None),
            Manage::Syncing {
                workspace,
                branch,
                cancelling: false,
            } if esc => (
                Some(Manage::Syncing {
                    workspace: workspace.clone(),
                    branch,
                    cancelling: true,
                }),
                Some(Act::Cancel(workspace)),
            ),
            other => (Some(other), None),
        }
    }

    /// The question is still about the worktree's changes: their node is open.
    pub fn is_check(&self) -> bool {
        matches!(
            self,
            Manage::Checking(_) | Manage::Dirty { .. } | Manage::Clean(_)
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
            Manage::Dirty { target, typed, .. } if *typed == target.confirm() => {
                "↵ quitar igual · esc volver"
            }
            Manage::Dirty { .. } => "↵ espera a que el nombre coincida · esc volver",
            Manage::Clean(_) => "y quitar · esc cancelar",
            Manage::Failed { .. } => "cualquier tecla para seguir",
            Manage::Syncing {
                cancelling: true, ..
            } => "cancelando…",
            Manage::Syncing { .. } => "subiendo… esc cancela",
            Manage::Settings { .. } => "j/k tema · m modo · ↵ elegir · esc cerrar",
            Manage::Rename { .. } => "↵ aceptar · esc cancelar (vacío: el nombre automático)",
            Manage::GroupName { .. } => "↵ aceptar · esc cancelar",
            Manage::DeleteGroup { .. } => "y borrar el grupo · esc cancelar",
            Manage::MoveTo { .. } => "j/k · ↵ mover · 1…9 un grupo · 0 sin grupo · n nuevo · esc",
            Manage::Archived { .. } => "j/k · ↵ reanudar · d borrar · esc cerrar",
            Manage::DeleteChat { .. } => "y borrar la conversación · esc cancelar",
            Manage::ChatTo { .. } => "j/k · ↵ llevarla · 1…9 un workspace · esc cancelar",
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
        Manage::Dirty {
            target: t,
            files: lines,
            typed: String::new(),
        }
    }

    /// Title, border colour and the lines of the box.
    fn text(
        &self,
        settings: &Settings,
        os_dark: bool,
        now_ms: u64,
    ) -> (String, Rgb, Vec<(String, Look)>) {
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
            Manage::Dirty {
                target,
                files,
                typed,
            } => {
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
                lines.extend(target.unpushed_note().map(|n| dim(&n)));
                let hint_name = if typed.is_empty() {
                    format!("  {}", target.confirm())
                } else {
                    String::new()
                };
                lines.extend([
                    blank(),
                    dim("Quitarlo borra la carpeta y eso se pierde."),
                    dim("Para quitarlo igual escribe el nombre de la rama:"),
                    blank(),
                    (format!("❯ {typed}▏{hint_name}"), Look::Accent),
                    blank(),
                    dim(self.hint()),
                ]);
                (
                    format!("Quitar {}: se perdería trabajo", target.name),
                    look::red(),
                    lines,
                )
            }
            Manage::Clean(t) => {
                let branch = t
                    .branch
                    .as_deref()
                    .map_or("La rama".to_owned(), |b| format!("La rama {b}"));
                let mut lines = vec![
                    plain(&format!("Se borra la carpeta {}.", t.path)),
                    dim(&format!("No hay cambios sin commitear. {branch} se queda.")),
                ];
                lines.extend(t.unpushed_note().map(|n| dim(&n)));
                lines.extend([blank(), dim(self.hint())]);
                (
                    format!("Quitar el worktree {}?", t.name),
                    look::red(),
                    lines,
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
            Manage::GroupName { title, text, .. } => (
                title.clone(),
                look::blue(),
                vec![
                    dim("nombre (hasta 40 letras)"),
                    (format!("❯ {text}▏"), Look::Accent),
                    blank(),
                    dim(self.hint()),
                ],
            ),
            Manage::DeleteGroup { name, members, .. } => (
                format!("Borrar el grupo {name}?"),
                look::yellow(),
                vec![
                    match members {
                        0 => plain("Está vacío."),
                        1 => plain("Su workspace pasa a «sin grupo»."),
                        n => plain(&format!("Sus {n} workspaces pasan a «sin grupo».")),
                    },
                    dim("Ninguno se cierra: solo se deshace el grupo."),
                    blank(),
                    dim(self.hint()),
                ],
            ),
            Manage::MoveTo {
                name,
                scope,
                groups,
                current,
                cursor,
                ..
            } => {
                let row = |at: usize, key: String, text: &str, here: bool| {
                    let marker = if at == *cursor { '❯' } else { ' ' };
                    let look = if at == *cursor {
                        Look::Accent
                    } else {
                        Look::Plain
                    };
                    (
                        format!("{marker} {key} {text}{}", if here { "  ✓" } else { "" }),
                        look,
                    )
                };
                let mut lines = vec![dim(scope), blank()];
                for (i, (id, group)) in groups.iter().enumerate() {
                    let key = if i < 9 {
                        (i + 1).to_string()
                    } else {
                        " ".to_owned()
                    };
                    lines.push(row(i, key, group, current.as_deref() == Some(id.as_str())));
                }
                lines.push(row(
                    groups.len(),
                    "0".into(),
                    "sin grupo",
                    current.is_none(),
                ));
                lines.push(row(groups.len() + 1, "n".into(), "grupo nuevo…", false));
                lines.extend([blank(), dim(self.hint())]);
                (format!("mover {name}"), look::blue(), lines)
            }
            Manage::Archived { chats, cursor } => {
                let mut lines: Vec<(String, Look)> = chats
                    .iter()
                    .enumerate()
                    .map(|(i, (_, name))| {
                        let (marker, look) = if i == *cursor {
                            ('❯', Look::Accent)
                        } else {
                            (' ', Look::Plain)
                        };
                        (format!("{marker} ✦ {name}"), look)
                    })
                    .collect();
                lines.extend([blank(), dim(self.hint())]);
                ("conversaciones archivadas".to_owned(), look::blue(), lines)
            }
            Manage::DeleteChat { name, .. } => (
                format!("Borrar la conversación {name}?"),
                look::yellow(),
                vec![
                    plain("Se borra su carpeta."),
                    dim("El transcript de Claude queda en ~/.claude/projects/."),
                    blank(),
                    dim(self.hint()),
                ],
            ),
            Manage::ChatTo {
                name,
                targets,
                cursor,
                ..
            } => {
                let mut lines: Vec<(String, Look)> = vec![
                    dim("Una conversación nueva, con un resumen de esta."),
                    dim("Esta se queda donde está."),
                    blank(),
                ];
                for (i, (_, target)) in targets.iter().enumerate() {
                    let (marker, look) = if i == *cursor {
                        ('❯', Look::Accent)
                    } else {
                        (' ', Look::Plain)
                    };
                    let key = if i < 9 {
                        (i + 1).to_string()
                    } else {
                        " ".to_owned()
                    };
                    lines.push((format!("{marker} {key} {target}"), look));
                }
                lines.extend([blank(), dim(self.hint())]);
                (format!("llevar {name} a un proyecto"), look::blue(), lines)
            }
            Manage::Syncing {
                branch, cancelling, ..
            } => (
                "subir el worktree".to_owned(),
                look::blue(),
                vec![
                    dim(&format!("git push origin {branch}")),
                    blank(),
                    (
                        format!(
                            "{} {} {branch}…",
                            super::shell::spin(now_ms),
                            if *cancelling {
                                "cancelando"
                            } else {
                                "subiendo"
                            }
                        ),
                        Look::Warn,
                    ),
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
    pub fn draw(
        &self,
        g: &mut Grid,
        (x, y, w, _): Rect,
        settings: &Settings,
        os_dark: bool,
        now_ms: u64,
    ) {
        let (title, border, lines) = self.text(settings, os_dark, now_ms);
        let widest = lines
            .iter()
            .map(|(l, _)| l.chars().count())
            .chain([title.chars().count() + 4])
            .max()
            .unwrap_or(0);
        let bw = (widest + 6).min(w.saturating_sub(2)).max(20);
        let bh = lines.len() + 4;
        let (bx, by) = (x + w.saturating_sub(bw) / 2, y + 2);
        g.clear_box((bx, by, bw, bh));
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

fn remove(t: &Target, remove_worktree: bool, force: bool) -> Act {
    Act::RemoveWorkspace {
        workspace: t.workspace.clone(),
        remove_worktree,
        force,
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
            unpushed: 0,
        }
    }

    fn removal(worktree: bool) -> Act {
        Act::RemoveWorkspace {
            workspace: "w".into(),
            remove_worktree: worktree,
            force: false,
        }
    }

    fn dirty(typed: &str) -> Manage {
        Manage::Dirty {
            target: target(true),
            files: vec!["M src/a.rs".into()],
            typed: typed.into(),
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
    fn a_worktree_with_changes_is_removed_only_when_its_branch_is_typed_and_then_by_force() {
        // `y` and `d` are letters of the name here, not answers.
        let (next, act) = dirty("").press(KeyCode::Char('y'));
        assert_eq!((next, act), (Some(dirty("y")), None));
        // `↵` waits for the name to match, and a name that is not the branch's never does.
        assert_eq!(
            dirty("feat").press(KeyCode::Enter),
            (Some(dirty("feat")), None)
        );
        assert_eq!(
            dirty("feat/login ").press(KeyCode::Enter),
            (Some(dirty("feat/login ")), None)
        );
        assert_eq!(
            dirty("feat/login").press(KeyCode::Enter),
            (
                None,
                Some(Act::RemoveWorkspace {
                    workspace: "w".into(),
                    remove_worktree: true,
                    force: true,
                })
            )
        );
        assert_eq!(
            dirty("fea").press(KeyCode::Backspace),
            (Some(dirty("fe")), None)
        );
        assert_eq!(
            dirty("feat").press(KeyCode::Esc),
            (Some(Manage::Workspace(target(true))), None),
            "esc goes back to the question and nothing is typed any more"
        );
    }

    #[test]
    fn what_stays_of_the_commits_no_remote_has_is_said_with_its_count() {
        let said = |n| {
            Target {
                unpushed: n,
                ..target(true)
            }
            .unpushed_note()
        };
        assert_eq!(said(0), None);
        assert_eq!(
            said(1).as_deref(),
            Some("1 commit sin subir se queda en la rama feat/login.")
        );
        assert_eq!(
            said(4).as_deref(),
            Some("4 commits sin subir se quedan en la rama feat/login.")
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
        assert_eq!(
            Manage::Clean(target(true)).press(KeyCode::Char('y')),
            (None, Some(removal(true))),
            "a clean worktree is not removed by force"
        );
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

    fn type_into(q: Manage, text: &str) -> (Option<Manage>, Option<Act>) {
        let mut state = (Some(q), None);
        for c in text.chars() {
            state = state.0.unwrap().press(KeyCode::Char(c));
        }
        state
    }

    fn naming(text: &str) -> Manage {
        Manage::GroupName {
            target: Naming::New,
            title: "grupo nuevo".into(),
            text: text.into(),
        }
    }

    #[test]
    fn a_group_name_takes_forty_letters_waits_while_empty_and_is_sent_on_enter() {
        let (q, act) = type_into(naming(""), &"é".repeat(45));
        assert!(act.is_none());
        let Some(Manage::GroupName { text, .. }) = &q else {
            panic!()
        };
        assert_eq!(text.chars().count(), MAX_GROUP_NAME);
        // empty or blank, enter waits
        let (q, act) = naming("  ").press(KeyCode::Enter);
        assert!(q.is_some() && act.is_none());
        let (q, act) = naming("qa").press(KeyCode::Enter);
        assert!(q.is_none());
        assert_eq!(
            act,
            Some(Act::GroupName {
                target: Naming::New,
                name: "qa".into()
            })
        );
        assert_eq!(naming("qa").press(KeyCode::Esc), (None, None));
        let (q, _) = naming("qa").press(KeyCode::Backspace);
        assert!(matches!(q, Some(Manage::GroupName { text, .. }) if text == "q"));
    }

    #[test]
    fn deleting_a_group_is_asked_and_only_y_sends() {
        let q = || Manage::DeleteGroup {
            group: "g".into(),
            name: "salesassist".into(),
            members: 2,
        };
        assert_eq!(
            q().press(KeyCode::Char('y')),
            (None, Some(Act::DeleteGroup("g".into())))
        );
        assert_eq!(q().press(KeyCode::Esc), (None, None));
        assert_eq!(q().press(KeyCode::Char('n')), (None, None));
        let (kept, act) = q().press(KeyCode::Char('x'));
        assert!(kept.is_some() && act.is_none());
    }

    fn move_to(current: Option<&str>, cursor: usize) -> Manage {
        Manage::MoveTo {
            workspace: "w".into(),
            name: "app".into(),
            scope: "solo este workspace".into(),
            groups: vec![("g1".into(), "uno".into()), ("g2".into(), "dos".into())],
            current: current.map(Into::into),
            cursor,
        }
    }

    fn to(dest: Dest) -> Option<Act> {
        Some(Act::MoveTo {
            workspace: "w".into(),
            to: dest,
        })
    }

    #[test]
    fn the_list_of_groups_answers_by_number_zero_n_or_the_row_and_never_sends_where_it_already_is()
    {
        let key = |q: Manage, c: char| q.press(KeyCode::Char(c));
        assert_eq!(
            key(move_to(None, 0), '2'),
            (None, to(Dest::Group("g2".into())))
        );
        assert_eq!(key(move_to(Some("g1"), 0), '0'), (None, to(Dest::None)));
        assert_eq!(key(move_to(None, 0), 'n'), (None, to(Dest::New)));
        // a number past the groups does nothing, and the box stays
        let (q, act) = key(move_to(None, 0), '3');
        assert!(q.is_some() && act.is_none());
        // where it is already, nothing is sent and the box closes
        assert_eq!(key(move_to(Some("g1"), 0), '1'), (None, None));
        assert_eq!(key(move_to(None, 0), '0'), (None, None));
        // the cursor moves over the groups, `sin grupo` and `grupo nuevo…` and enter picks its row
        let (q, _) = key(move_to(None, 0), 'j');
        let (q, _) = q.unwrap().press(KeyCode::Down);
        let (q, _) = q.unwrap().press(KeyCode::Down);
        assert!(
            matches!(&q, Some(Manage::MoveTo { cursor: 3, .. })),
            "it stops at the last row"
        );
        assert_eq!(q.unwrap().press(KeyCode::Enter), (None, to(Dest::New)));
        assert_eq!(
            move_to(Some("g1"), 1).press(KeyCode::Enter),
            (None, to(Dest::Group("g2".into())))
        );
        assert_eq!(move_to(None, 0).press(KeyCode::Esc), (None, None));
    }

    #[test]
    fn option_backspace_takes_a_word_of_the_name_typed_and_changes_no_other_question() {
        assert_eq!(dirty("feat login").erase_word(), dirty("feat "));
        let rename = Manage::Rename {
            target: Named::Pane("p".into()),
            title: "t".into(),
            text: "uno dos".into(),
        };
        let Manage::Rename { text, .. } = rename.erase_word() else {
            panic!()
        };
        assert_eq!(text, "uno ");
        let q = Manage::Workspace(target(true));
        assert_eq!(q.clone().erase_word(), q);
    }
}
