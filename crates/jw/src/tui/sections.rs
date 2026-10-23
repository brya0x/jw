//! The `/` list and the `/skills` list in sections (spec REQ-795 to REQ-803; prototype
//! `docs/prototypes/command-categories.html`, approved 2026-10-23). A pure part: which section a
//! command is in, the lines of a window that holds at most `shown` of them (a title is a line),
//! and the jump from one section to the next. What draws them is `conversation.rs`.

use contract::conversation::{CommandSource, ConvCommand};

use super::{Rgb, look};

/// The plugin the engine's skills of claude.ai are listed under (REQ-794).
const CLAUDE_AI: &str = "claude.ai";

/// A section's title and colour (REQ-800).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Head {
    pub title: String,
    pub colour: Rgb,
}

/// Where a command is listed (REQ-795): its place in the order of the sections, and the section.
pub fn slot(c: &ConvCommand) -> (u8, Head) {
    let (rank, title, colour) = match c.source {
        CommandSource::Jwn => (0, identity::get().name.clone(), look::blue()),
        CommandSource::Skill => (1, "Personales".to_owned(), look::yellow()),
        CommandSource::Project => (2, "Proyecto".to_owned(), look::green()),
        CommandSource::Plugin => match plugin(c) {
            g if g == CLAUDE_AI => (4, g, look::red()),
            g => (3, g, look::cyan()),
        },
        CommandSource::Claude => (5, "Claude".to_owned(), look::magenta()),
    };
    (rank, Head { title, colour })
}

/// A plugin's name: its `group`, or what a name before the first `:` says for a daemon that gave none.
fn plugin(c: &ConvCommand) -> String {
    c.group
        .clone()
        .or_else(|| {
            let (plugin, _) = c.name.trim_start_matches('/').split_once(':')?;
            Some(plugin.to_owned())
        })
        .unwrap_or_else(|| "plugin".to_owned())
}

/// The section of each command of a list, in the list's order.
pub fn heads_of(menu: &[&ConvCommand]) -> Vec<Head> {
    menu.iter().map(|c| slot(c).1).collect()
}

/// What the list sorts by: the section (plugins by name, case aside), then the names that begin
/// with what was typed, then the name.
pub fn order(c: &ConvCommand, typed: &str) -> (u8, String, bool, String) {
    let (rank, head) = slot(c);
    let name = c.name.trim_start_matches('/').to_lowercase();
    (
        rank,
        head.title.to_lowercase(),
        !name.starts_with(typed),
        c.name.clone(),
    )
}

/// A line of the window.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Entry {
    /// A section's title with how many it holds; `sticky` when its own title has scrolled out of
    /// the window and this one stands for it (REQ-798).
    Head {
        head: Head,
        count: usize,
        sticky: bool,
    },
    /// The item at this place of the list.
    Item(usize),
}

/// The lines to draw for a list whose item `i` is in section `heads[i]` (the items of a section
/// are together), with the one at `at` lit: at most `shown` lines, titles included, and a title
/// on top always (REQ-798). `first` is the line the window began at last time, which it keeps
/// while the lit item is in it (under the top line); it returns the one it began at now. A
/// stateless list passes 0, which keeps the lit item at the bottom.
pub fn lines(heads: &[Head], at: usize, shown: usize, first: usize) -> (Vec<Entry>, usize) {
    let mut all = Vec::new();
    let mut sections = Vec::new();
    for (i, head) in heads.iter().enumerate() {
        if i == 0 || *head != heads[i - 1] {
            sections.push((head.clone(), 0));
            all.push(Entry::Head {
                head: head.clone(),
                count: 0,
                sticky: false,
            });
        }
        sections.last_mut().expect("a section was just opened").1 += 1;
        all.push(Entry::Item(i));
    }
    let mut n = 0;
    for line in &mut all {
        if let Entry::Head { count, .. } = line {
            *count = sections[n].1;
            n += 1;
        }
    }
    let lit = all.iter().position(|l| *l == Entry::Item(at)).unwrap_or(0);
    let top = if all.len() <= shown {
        0
    } else {
        first.clamp((lit + 1).saturating_sub(shown), lit.saturating_sub(1))
    };
    let mut out: Vec<Entry> = all.into_iter().skip(top).take(shown).collect();
    if let Some(&Entry::Item(first)) = out.first() {
        let count = heads.iter().filter(|h| **h == heads[first]).count();
        out[0] = Entry::Head {
            head: heads[first].clone(),
            count,
            sticky: true,
        };
    }
    (out, top)
}

/// `RePág` and `AvPág` (REQ-799): the first item of the next section, or of this one, or of the
/// one before when already at the first of this one.
pub fn jump(heads: &[Head], at: usize, down: bool) -> usize {
    let starts: Vec<usize> = (0..heads.len())
        .filter(|&i| i == 0 || heads[i] != heads[i - 1])
        .collect();
    if down {
        return starts
            .iter()
            .copied()
            .find(|&i| i > at)
            .unwrap_or(heads.len().saturating_sub(1));
    }
    let before = starts.iter().rev().copied();
    match before.clone().find(|&i| i <= at) {
        Some(here) if here < at => here,
        _ => before.clone().find(|&i| i < at).unwrap_or(0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use contract::conversation::CommandKind;

    fn cmd(name: &str, source: CommandSource, group: Option<&str>) -> ConvCommand {
        ConvCommand {
            name: name.into(),
            source,
            kind: CommandKind::Prompt,
            description: None,
            group: group.map(str::to_owned),
        }
    }

    fn heads(spec: &[(&str, usize)]) -> Vec<Head> {
        spec.iter()
            .flat_map(|(t, n)| {
                std::iter::repeat_n(
                    Head {
                        title: (*t).to_owned(),
                        colour: look::dim(),
                    },
                    *n,
                )
            })
            .collect()
    }

    #[test]
    fn the_sections_go_jwn_personales_proyecto_plugins_claude_ai_claude() {
        let sorted = |mut v: Vec<ConvCommand>| {
            v.sort_by_key(|c| order(c, ""));
            v.iter().map(|c| slot(c).1.title).collect::<Vec<_>>()
        };
        let v = vec![
            cmd("/init", CommandSource::Claude, None),
            cmd("/docs", CommandSource::Plugin, Some("claude.ai")),
            cmd("/vercel:deploy", CommandSource::Plugin, Some("vercel")),
            cmd(
                "/ponytail:ponytail",
                CommandSource::Plugin,
                Some("Ponytail"),
            ),
            cmd("/proj", CommandSource::Project, None),
            cmd("/commit", CommandSource::Skill, None),
            cmd("/exit", CommandSource::Jwn, None),
            cmd("/other:x", CommandSource::Plugin, None),
        ];
        assert_eq!(
            sorted(v),
            [
                identity::get().name.as_str(),
                "Personales",
                "Proyecto",
                "other",
                "Ponytail",
                "vercel",
                "claude.ai",
                "Claude"
            ],
            "a plugin of a daemon that sent no group is the text before its `:`"
        );
    }

    #[test]
    fn inside_a_section_what_begins_with_the_letters_goes_first() {
        let mut v = [
            cmd("/rename", CommandSource::Jwn, None),
            cmd("/resume", CommandSource::Jwn, None),
            cmd("/status", CommandSource::Jwn, None),
            cmd("/cost", CommandSource::Jwn, None),
        ];
        v.sort_by_key(|c| order(c, "s"));
        let names: Vec<_> = v.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, ["/status", "/cost", "/rename", "/resume"]);
    }

    fn lit(lines: &[Entry], at: usize) -> bool {
        lines.contains(&Entry::Item(at))
    }

    #[test]
    fn a_window_has_at_most_shown_lines_with_a_title_on_top_and_the_lit_one_in_it() {
        let h = heads(&[("a", 5), ("b", 1), ("c", 12)]);
        for at in 0..h.len() {
            let w = lines(&h, at, 8, 0).0;
            assert!(w.len() <= 8, "{at}: {w:?}");
            assert!(lit(&w, at), "{at}: {w:?}");
            assert!(matches!(w[0], Entry::Head { .. }), "{at}: a title on top");
        }
        // short: the whole list, no scroll
        let short = lines(&heads(&[("a", 2), ("b", 1)]), 2, 8, 0).0;
        assert_eq!(short.len(), 5);
        assert!(
            matches!(&short[3], Entry::Head { head, count: 1, sticky: false } if head.title == "b")
        );
        // longer than the window: the title of the section that scrolled is on top, marked
        let w = lines(&h, 15, 8, 0).0;
        assert!(
            matches!(&w[0], Entry::Head { head, count: 12, sticky: true } if head.title == "c"),
            "{w:?}"
        );
        assert!(lines(&[], 0, 8, 0).0.is_empty());
    }

    #[test]
    fn the_window_keeps_its_top_while_the_lit_one_stays_inside() {
        let h = heads(&[("a", 5), ("b", 1), ("c", 12)]);
        let (_, top) = lines(&h, 15, 8, 0);
        // going up three items, the window does not move
        let (w, kept) = lines(&h, 12, 8, top);
        assert_eq!(kept, top);
        assert!(lit(&w, 12) && w.len() == 8);
        // going up past its top it follows, with the lit one just under the title
        let (w, kept) = lines(&h, 6, 8, top);
        assert!(kept < top && lit(&w, 6), "{w:?}");
        assert!(matches!(w[0], Entry::Head { .. }));
        // a stale top over a list that fits shows it from the start
        assert_eq!(lines(&heads(&[("a", 2)]), 1, 8, 9).1, 0);
    }

    #[test]
    fn the_arrows_cross_a_boundary_because_the_list_is_one() {
        let h = heads(&[("a", 2), ("b", 2)]);
        let w = lines(&h, 1, 8, 0).0;
        let w2 = lines(&h, 2, 8, 0).0;
        assert!(
            lit(&w, 1) && lit(&w2, 2),
            "item 2 is the first of b, right after a's last"
        );
    }

    #[test]
    fn page_keys_go_to_the_start_of_a_section() {
        let h = heads(&[("a", 3), ("b", 2), ("c", 2)]);
        assert_eq!(jump(&h, 0, true), 3);
        assert_eq!(jump(&h, 3, true), 5);
        assert_eq!(jump(&h, 6, true), 6, "at the end it stays");
        assert_eq!(jump(&h, 4, false), 3, "the start of this one");
        assert_eq!(jump(&h, 3, false), 0, "the one before when already there");
        assert_eq!(jump(&h, 0, false), 0);
        assert_eq!(jump(&[], 0, true), 0);
    }
}
