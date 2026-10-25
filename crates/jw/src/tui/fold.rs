//! The tools of a conversation folded into one row for each run, as Claude Code draws them
//! (spec REQ-558 and REQ-559; prototype `docs/prototypes/conversation-fidelity.html`): «Buscó 1
//! patrón, ejecutó 3 comandos». Pure: it says which items are a run and what the row says, and the
//! drawing and the clicks are `conversation.rs`'s. The phone has the same rule in `Fold.kt`.

use contract::conversation::{Item, ToolState};

/// What a tool does, for the words of the row.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Class {
    Search,
    Read,
    List,
    Shell,
}

/// The order the classes are said in.
const ORDER: [Class; 4] = [Class::Search, Class::Read, Class::List, Class::Shell];

impl Class {
    /// The verb, and the thing it is done to in the singular and in the plural.
    fn words(self) -> (&'static str, &'static str, &'static str) {
        match self {
            Class::Search => ("Buscó", "patrón", "patrones"),
            Class::Read => ("Leyó", "archivo", "archivos"),
            Class::List => ("Listó", "carpeta", "carpetas"),
            Class::Shell => ("Ejecutó", "comando", "comandos"),
        }
    }
}

/// The class of a tool by its name; none for one that is not folded (an edit has its diff, a
/// subagent its card).
pub fn class_of(name: &str) -> Option<Class> {
    match name {
        "Bash" => Some(Class::Shell),
        "Grep" | "Glob" => Some(Class::Search),
        "Read" => Some(Class::Read),
        "LS" => Some(Class::List),
        _ => None,
    }
}

/// The class of an item that folds: a tool of a class that has ended (one that runs, or waits for
/// an answer, is drawn by itself).
fn class_of_item(item: &Item) -> Option<Class> {
    match item {
        // One with no result (`lost`) may still be running, as far as anyone saw: it is drawn by itself (REQ-889).
        Item::Tool {
            name, state, lost, ..
        } if !*lost && !matches!(state, ToolState::Run | ToolState::Ask) => class_of(name),
        _ => None,
    }
}

/// A run of tools that fold together: the indexes of its first and last item, and what it says.
#[derive(Debug, PartialEq, Eq)]
pub struct Run {
    pub first: usize,
    pub last: usize,
    pub title: String,
}

/// The runs of `items`: each stretch of folding tools, even when it is one tool.
pub fn runs(items: &[Item]) -> Vec<Run> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < items.len() {
        if class_of_item(&items[i]).is_none() {
            i += 1;
            continue;
        }
        let first = i;
        while i < items.len() && class_of_item(&items[i]).is_some() {
            i += 1;
        }
        out.push(Run {
            first,
            last: i - 1,
            title: title(&items[first..i]),
        });
    }
    out
}

/// What a run says: its classes in a fixed order, the first with a capital.
pub fn title(run: &[Item]) -> String {
    let mut parts: Vec<String> = Vec::new();
    for class in ORDER {
        let n = run
            .iter()
            .filter(|i| class_of_item(i) == Some(class))
            .count();
        if n == 0 {
            continue;
        }
        let (verb, one, many) = class.words();
        let part = format!("{verb} {n} {}", if n == 1 { one } else { many });
        parts.push(if parts.is_empty() {
            part
        } else {
            let mut chars = part.chars();
            chars
                .next()
                .map(|c| c.to_lowercase().chain(chars).collect())
                .unwrap_or_default()
        });
    }
    parts.join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tool(name: &str, state: ToolState) -> Item {
        Item::Tool {
            id: name.into(),
            name: name.into(),
            arg: String::new(),
            state,
            summary: None,
            output: Vec::new(),
            diff: Vec::new(),
            url: None,
            by: None,
            lost: false,
        }
    }

    fn text() -> Item {
        Item::Text {
            text: "x".into(),
            live: false,
            blocks: None,
        }
    }

    /// REQ-889: a tool with no result is not folded into the commands that ran: its row stays, and it breaks the run.
    #[test]
    fn a_tool_with_no_result_is_not_folded_into_a_run() {
        let mut lost = tool("Bash", ToolState::Failed);
        if let Item::Tool { lost, .. } = &mut lost {
            *lost = true;
        }
        assert!(class_of_item(&lost).is_none());
        assert!(class_of_item(&tool("Bash", ToolState::Failed)).is_some());
    }

    #[test]
    fn a_stretch_of_folding_tools_is_one_run_and_what_is_not_a_tool_of_a_class_breaks_it() {
        let ok = ToolState::Ok;
        let items = [
            text(),
            tool("Bash", ok),
            tool("Grep", ok),
            tool("Bash", ToolState::Failed),
            tool("Edit", ok),
            tool("Read", ok),
            tool("Bash", ToolState::Run),
            tool("Bash", ok),
            text(),
        ];
        assert_eq!(
            runs(&items),
            [
                Run {
                    first: 1,
                    last: 3,
                    title: "Buscó 1 patrón, ejecutó 2 comandos".into()
                },
                Run {
                    first: 5,
                    last: 5,
                    title: "Leyó 1 archivo".into()
                },
                Run {
                    first: 7,
                    last: 7,
                    title: "Ejecutó 1 comando".into()
                },
            ],
            "an edit and a tool that runs are drawn by themselves and end the run"
        );
    }

    #[test]
    fn the_classes_are_said_in_a_fixed_order_with_the_plural_and_only_the_first_has_a_capital() {
        let ok = ToolState::Ok;
        let items = [
            tool("Bash", ok),
            tool("Bash", ok),
            tool("Bash", ok),
            tool("LS", ok),
            tool("Read", ok),
            tool("Read", ok),
            tool("Glob", ok),
            tool("Grep", ok),
        ];
        assert_eq!(
            title(&items),
            "Buscó 2 patrones, leyó 2 archivos, listó 1 carpeta, ejecutó 3 comandos"
        );
        assert!(runs(&[]).is_empty() && runs(&[text()]).is_empty());
        assert_eq!(class_of("Edit"), None);
        assert_eq!(class_of("Agent"), None);
    }
}
