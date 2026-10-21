//! The open pull requests of a project, to pick one to review (`^␣ R`; prototype:
//! `docs/prototypes/pr-review.html`, spec REQ-587 to REQ-589). The daemon reads them with `gh`
//! (`list_prs`) and makes the review (`review_pr`); this is the list and what is typed in it.

use contract::PrInfo;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::{Grid, Rect, Style, clip, look, shell};

/// What the list holds: it is being read, it is read, or `gh` could not.
pub enum Load {
    Loading,
    Ready(Vec<PrInfo>),
    Failed(String),
}

/// What a key asks of the client.
#[derive(Debug, PartialEq, Eq)]
pub enum Act {
    Nothing,
    Close,
    /// Read the list again: of every pull request, or only those that ask for the person's review.
    Reload {
        requested: bool,
    },
    /// Make the review of this pull request.
    Review(u64),
    /// Stop the review that is being made (REQ-604).
    Cancel,
    /// Open this address in the browser.
    Browse(String),
}

pub struct Prs {
    /// The workspace the list was asked from: the project it is of.
    pub workspace: String,
    query: String,
    sel: usize,
    mine: bool,
    load: Load,
    /// The pull request whose review is being made, and what that is doing.
    making: Option<(u64, String)>,
    /// `esc` was pressed while it was made: the daemon was asked to stop it.
    cancelling: bool,
}

/// Rows of the list that are drawn at once.
const SHOWN: usize = 8;

impl Prs {
    pub fn new(workspace: String) -> Prs {
        Prs {
            workspace,
            query: String::new(),
            sel: 0,
            mine: false,
            load: Load::Loading,
            making: None,
            cancelling: false,
        }
    }

    pub fn set_list(&mut self, list: Result<Vec<PrInfo>, String>) {
        self.sel = 0;
        self.load = match list {
            Ok(prs) => Load::Ready(prs),
            Err(why) => Load::Failed(why),
        };
    }

    pub fn loading(&mut self) {
        self.load = Load::Loading;
    }

    /// The review of `pr` is being made: the list waits and says so.
    pub fn making(&mut self, pr: u64, what: &str) {
        self.making = Some((pr, what.to_owned()));
    }

    /// The review could not be made: the list is the person's again.
    pub fn stop_making(&mut self) {
        self.making = None;
        self.cancelling = false;
    }

    /// The pull requests that the filter lets through, in the order they were listed.
    pub fn rows(&self) -> Vec<&PrInfo> {
        let Load::Ready(prs) = &self.load else {
            return Vec::new();
        };
        let q = self.query.to_lowercase();
        prs.iter()
            .filter(|p| {
                q.is_empty()
                    || format!("{} {} {} {}", p.number, p.title, p.author, p.branch)
                        .to_lowercase()
                        .contains(&q)
            })
            .collect()
    }

    pub fn press(&mut self, ev: &KeyEvent) -> Act {
        if self.making.is_some() {
            // `esc` asks the daemon to stop it, once; the reply gives the list back.
            if ev.code == KeyCode::Esc && !self.cancelling {
                self.cancelling = true;
                return Act::Cancel;
            }
            return Act::Nothing;
        }
        let rows = self.rows();
        let count = rows.len();
        let here = rows.get(self.sel).map(|p| (p.number, p.url.clone()));
        let plain = !ev
            .modifiers
            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT);
        match ev.code {
            KeyCode::Esc => return Act::Close,
            KeyCode::Down => self.sel = (self.sel + 1).min(count.saturating_sub(1)),
            KeyCode::Up => self.sel = self.sel.saturating_sub(1),
            KeyCode::Enter => {
                if let Some((n, _)) = here {
                    return Act::Review(n);
                }
            }
            KeyCode::Backspace => {
                self.query.pop();
                self.sel = 0;
            }
            KeyCode::Char('m') if plain && self.query.is_empty() => {
                self.mine = !self.mine;
                return Act::Reload {
                    requested: self.mine,
                };
            }
            KeyCode::Char('o') if plain && self.query.is_empty() => {
                if let Some((_, url)) = here.filter(|(_, u)| !u.is_empty()) {
                    return Act::Browse(url);
                }
            }
            KeyCode::Char(c) if plain => {
                self.query.push(c);
                self.sel = 0;
            }
            _ => {}
        }
        Act::Nothing
    }

    /// Draw the list over `area`, near its top and centred.
    pub fn draw(&self, g: &mut Grid, (x, y, w, h): Rect, now_ms: u64) {
        let bw = 104.min(w.saturating_sub(2)).max(30);
        let bh = h.saturating_sub(2).clamp(12, 24);
        let (bx, by) = (x + w.saturating_sub(bw) / 2, y + 1);
        g.clear_box((bx, by, bw, bh));
        let title = if self.mine {
            "revisar un PR · solo los que piden tu revisión"
        } else {
            "revisar un PR"
        };
        g.boxed(
            (bx, by, bw, bh),
            title,
            look::blue(),
            Style::fg(look::fg()).bold(),
            None,
        );
        let ground = look::panel();
        let dim = Style::fg(look::dim()).on(ground);
        let inner = bw.saturating_sub(6);
        g.put(
            bx + 3,
            by + 1,
            "▸",
            Style::fg(look::blue()).bold().on(ground),
        );
        if self.query.is_empty() {
            g.put(
                bx + 5,
                by + 1,
                &clip("filtra por número, título, autor o rama", inner),
                dim,
            );
        } else {
            g.put(
                bx + 5,
                by + 1,
                &clip(&self.query, inner),
                Style::fg(look::fg()).on(ground),
            );
        }
        let footer = if self.making.is_some() {
            if self.cancelling {
                "cancelando…"
            } else {
                "esc cancela"
            }
        } else {
            "↵ revisar · m mis revisiones · o abrir en el navegador · esc"
        };
        g.put(bx + 3, by + bh - 2, &clip(footer, inner), dim);
        let top = by + 3;
        match &self.load {
            Load::Loading => g.put(
                bx + 5,
                top,
                &format!("{} cargando los PR abiertos…", shell::spin(now_ms)),
                Style::fg(look::yellow()).on(ground),
            ),
            Load::Failed(why) => g.put(
                bx + 5,
                top,
                &clip(why, inner),
                Style::fg(look::red()).on(ground),
            ),
            Load::Ready(_) => self.draw_rows(g, (bx, top, bw, bh - 5), ground),
        }
        if let Some((pr, what)) = &self.making {
            g.put(
                bx + 3,
                by + bh - 2,
                &clip(&format!("{} #{pr} · {what}", shell::spin(now_ms)), inner),
                Style::fg(look::yellow()).on(ground),
            );
        }
    }

    fn draw_rows(&self, g: &mut Grid, (bx, top, bw, room): Rect, ground: super::Rgb) {
        let rows = self.rows();
        let dim = Style::fg(look::dim()).on(ground);
        if rows.is_empty() {
            let text = if matches!(&self.load, Load::Ready(p) if p.is_empty()) {
                "no hay PR abiertos"
            } else {
                "ningún PR coincide"
            };
            g.put(bx + 5, top, text, dim);
            return;
        }
        let from = self.sel.saturating_sub(SHOWN - 1);
        for (i, p) in rows.iter().enumerate().skip(from).take(SHOWN) {
            let y = top + i - from;
            let on = i == self.sel;
            let bg = if on { look::sel() } else { ground };
            g.put(
                bx + 1,
                y,
                &" ".repeat(bw.saturating_sub(2)),
                Style::default().on(bg),
            );
            let (state, tone) = state_of(p);
            let state_w = state.chars().count();
            let title_w = bw.saturating_sub(40 + state_w).max(10);
            g.put(
                bx + 3,
                y,
                &format!("#{:<5}", p.number),
                Style::fg(look::blue()).bold().on(bg),
            );
            let name = Style::fg(look::fg()).on(bg);
            g.put(
                bx + 10,
                y,
                &clip(&p.title, title_w),
                if on { name.bold() } else { name },
            );
            let at = bx + 10 + title_w + 1;
            g.put(at, y, &clip(&p.author, 11), Style::fg(look::dim()).on(bg));
            g.put(
                at + 12,
                y,
                &format!("+{}", p.additions),
                Style::fg(look::green()).on(bg),
            );
            g.put(
                at + 18,
                y,
                &format!("−{}", p.deletions),
                Style::fg(look::red()).on(bg),
            );
            g.put(at + 24, y, &state, Style::fg(tone).on(bg));
        }
        // under the list, what the pull request says and the files it changes
        let Some(p) = rows.get(self.sel) else {
            return;
        };
        let y0 = top + SHOWN.min(rows.len()) + 1;
        let width = bw.saturating_sub(6);
        let end = top + room;
        if y0 >= end {
            return;
        }
        g.put(
            bx + 3,
            y0,
            &clip(&format!("{} → {} · {}", p.branch, p.base, p.updated), width),
            Style::fg(look::cyan()).on(ground),
        );
        let mut y = y0 + 1;
        for line in p.body.lines().filter(|l| !l.trim().is_empty()).take(2) {
            if y < end {
                g.put(
                    bx + 3,
                    y,
                    &clip(line.trim(), width),
                    Style::fg(look::fg()).on(ground),
                );
                y += 1;
            }
        }
        if y < end && !p.files.is_empty() {
            g.put(bx + 3, y, &format!("archivos ({})", p.files.len()), dim);
            y += 1;
        }
        for f in p.files.iter().take(end.saturating_sub(y)) {
            g.put(
                bx + 5,
                y,
                &clip(&f.path, 34),
                Style::fg(look::fg()).on(ground),
            );
            g.put(
                bx + 41,
                y,
                &format!("+{} −{}", f.additions, f.deletions),
                dim,
            );
            y += 1;
        }
    }
}

/// How a pull request stands in review, and the colour that says it.
fn state_of(p: &PrInfo) -> (String, super::Rgb) {
    if p.draft {
        return ("◌ borrador".into(), look::dim());
    }
    match p.decision.as_deref() {
        Some("APPROVED") => ("✓ aprobado".into(), look::green()),
        Some("CHANGES_REQUESTED") => ("✗ cambios pedidos".into(), look::red()),
        Some("REVIEW_REQUIRED") => ("● sin revisar".into(), look::yellow()),
        _ => (String::new(), look::dim()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pr(n: u64, title: &str, author: &str) -> PrInfo {
        PrInfo {
            number: n,
            title: title.into(),
            author: author.into(),
            branch: format!("{author}/x"),
            base: "main".into(),
            draft: false,
            updated: "2026-10-19T10:00:00Z".into(),
            decision: None,
            additions: 3,
            deletions: 1,
            body: "Adds a thing.\n\nMore.".into(),
            url: format!("https://x/pull/{n}"),
            files: Vec::new(),
        }
    }

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn ready() -> Prs {
        let mut p = Prs::new("w".into());
        p.set_list(Ok(vec![
            pr(142, "Add rate limiting", "maria"),
            pr(139, "Fix cursor", "luis"),
        ]));
        p
    }

    #[test]
    fn the_list_is_filtered_by_what_is_typed_and_moves_with_the_arrows() {
        let mut p = ready();
        assert_eq!(p.rows().len(), 2);
        p.press(&key(KeyCode::Down));
        assert_eq!(p.press(&key(KeyCode::Enter)), Act::Review(139));
        for c in "mar".chars() {
            p.press(&key(KeyCode::Char(c)));
        }
        assert_eq!(
            p.rows().iter().map(|r| r.number).collect::<Vec<_>>(),
            [142],
            "by author"
        );
        p.press(&key(KeyCode::Backspace));
        p.press(&key(KeyCode::Backspace));
        p.press(&key(KeyCode::Backspace));
        for c in "139".chars() {
            p.press(&key(KeyCode::Char(c)));
        }
        assert_eq!(p.rows().len(), 1, "by number");
        assert_eq!(p.press(&key(KeyCode::Esc)), Act::Close);
    }

    #[test]
    fn m_and_o_are_keys_only_while_nothing_is_typed() {
        let mut p = ready();
        assert_eq!(
            p.press(&key(KeyCode::Char('o'))),
            Act::Browse("https://x/pull/142".into())
        );
        assert_eq!(
            p.press(&key(KeyCode::Char('m'))),
            Act::Reload { requested: true }
        );
        assert_eq!(
            p.press(&key(KeyCode::Char('m'))),
            Act::Reload { requested: false }
        );
        p.press(&key(KeyCode::Char('r')));
        assert_eq!(
            p.press(&key(KeyCode::Char('m'))),
            Act::Nothing,
            "typed in the filter now"
        );
        assert_eq!(p.rows().len(), 0, "no pull request has «rm»");
    }

    #[test]
    fn nothing_is_asked_while_the_list_loads_and_a_review_is_made_only_once() {
        let mut p = Prs::new("w".into());
        assert_eq!(p.press(&key(KeyCode::Enter)), Act::Nothing, "no rows yet");
        assert_eq!(
            p.press(&key(KeyCode::Esc)),
            Act::Close,
            "esc closes while it loads"
        );
        let mut p = ready();
        p.making(142, "trayendo la rama");
        assert_eq!(p.press(&key(KeyCode::Enter)), Act::Nothing);
        assert_eq!(
            p.press(&key(KeyCode::Esc)),
            Act::Cancel,
            "esc stops the review that is being made"
        );
        assert_eq!(
            p.press(&key(KeyCode::Esc)),
            Act::Nothing,
            "once: the reply gives the list back"
        );
        p.stop_making();
        assert_eq!(p.press(&key(KeyCode::Esc)), Act::Close);
    }
}
