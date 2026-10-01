//! web3d-M5: the one edit protocol. A model answers a coding request
//! with either
//!
//! - **SEARCH/REPLACE blocks**, for changes to an existing file (output
//!   proportional to the change, not the file):
//!
//!   ```text
//!   <<<<<<< SEARCH
//!   lines copied verbatim from the file
//!   =======
//!   replacement lines
//!   >>>>>>> REPLACE
//!   ```
//!
//! - or **a whole file** in a fenced code block.
//!
//! Ported from Studio's `llm.rs`, with one change: a SEARCH text that
//! matches more than one place is an error, where Studio silently
//! edited the first match.

/// One SEARCH/REPLACE block. An empty `search` appends `replace` to
/// the end of the file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SearchReplace {
    pub search: String,
    pub replace: String,
}

/// What a reply asks for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Edit {
    Blocks(Vec<SearchReplace>),
    WholeFile(String),
    /// Neither blocks nor a fenced file.
    Nothing,
}

/// Read a reply. Blocks win over a fenced file when both appear (a
/// model quoting the file before editing it). `langs` are the fence
/// tags accepted for a whole file (`["twe"]`, `["python", "py"]`); an
/// untagged fence is accepted too.
pub fn parse_reply(text: &str, langs: &[&str]) -> Edit {
    let blocks = parse_blocks(text);
    if !blocks.is_empty() {
        return Edit::Blocks(blocks);
    }
    match fenced_file(text, langs) {
        Some(file) => Edit::WholeFile(file),
        None => Edit::Nothing,
    }
}

/// Apply `edit` to `source` (the current file; empty for a new one).
pub fn apply(source: &str, edit: &Edit) -> Result<String, EditError> {
    match edit {
        Edit::Blocks(blocks) => apply_blocks(source, blocks),
        Edit::WholeFile(file) => Ok(file.clone()),
        Edit::Nothing => Err(EditError::NoEdit),
    }
}

/// Why an edit couldn't be applied. The message is written for the
/// model: the loop sends it back so the next round can fix the block.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EditError {
    NoEdit,
    /// Block `index` (0-based) matched nothing.
    NoMatch { index: usize },
    /// Block `index` matched `count` places.
    Ambiguous { index: usize, count: usize },
}

impl std::fmt::Display for EditError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            EditError::NoEdit => write!(
                f,
                "the reply contained neither SEARCH/REPLACE blocks nor a fenced code block"
            ),
            EditError::NoMatch { index } => write!(
                f,
                "edit block {} did not match the file: its SEARCH text must be copied verbatim from the current file",
                index + 1
            ),
            EditError::Ambiguous { index, count } => write!(
                f,
                "edit block {}'s SEARCH text appears {count} times in the file: include enough surrounding lines to match exactly one place",
                index + 1
            ),
        }
    }
}

impl std::error::Error for EditError {}

/// The first fenced code block tagged with one of `langs`, or else the
/// first untagged one. Its contents, without the fences.
pub fn fenced_file(text: &str, langs: &[&str]) -> Option<String> {
    let mut untagged = None;
    let mut lines = text.lines();
    while let Some(line) = lines.next() {
        let Some(tag) = line.trim_start().strip_prefix("```") else {
            continue;
        };
        let tag = tag.trim();
        let mut body = Vec::new();
        let mut closed = false;
        for inner in lines.by_ref() {
            if inner.trim_start().starts_with("```") {
                closed = true;
                break;
            }
            body.push(inner);
        }
        if !closed {
            break;
        }
        let body = body.join("\n");
        if langs.iter().any(|l| l.eq_ignore_ascii_case(tag)) {
            return Some(body);
        }
        if tag.is_empty() && untagged.is_none() {
            untagged = Some(body);
        }
    }
    untagged
}

/// Every SEARCH/REPLACE block in `text`. Markers are matched leniently
/// (a run of seven `<`, `=` or `>`), so small drift still parses; a
/// block missing its closing marker is dropped.
pub fn parse_blocks(text: &str) -> Vec<SearchReplace> {
    #[derive(PartialEq)]
    enum At {
        Outside,
        Search,
        Replace,
    }
    let mut blocks = Vec::new();
    let mut at = At::Outside;
    let mut search: Vec<&str> = Vec::new();
    let mut replace: Vec<&str> = Vec::new();
    for line in text.lines() {
        let t = line.trim_start();
        if t.starts_with("<<<<<<<") {
            at = At::Search;
            search.clear();
            replace.clear();
        } else if t.starts_with("=======") && at == At::Search {
            at = At::Replace;
        } else if t.starts_with(">>>>>>>") && at == At::Replace {
            blocks.push(SearchReplace {
                search: search.join("\n"),
                replace: replace.join("\n"),
            });
            at = At::Outside;
        } else {
            match at {
                At::Search => search.push(line),
                At::Replace => replace.push(line),
                At::Outside => {}
            }
        }
    }
    blocks
}

/// Apply blocks in order, each to the result of the ones before. A
/// SEARCH is matched as whole lines, ignoring trailing whitespace (a
/// model's stray `\r` or spaces), and must match exactly once.
pub fn apply_blocks(source: &str, blocks: &[SearchReplace]) -> Result<String, EditError> {
    let mut lines: Vec<String> = source.split('\n').map(str::to_string).collect();
    for (index, block) in blocks.iter().enumerate() {
        let replace: Vec<String> = if block.replace.is_empty() {
            Vec::new()
        } else {
            block.replace.split('\n').map(str::to_string).collect()
        };
        if block.search.is_empty() {
            // Appending to an empty file replaces its one empty line.
            if lines.len() == 1 && lines[0].is_empty() {
                lines.clear();
            }
            lines.extend(replace);
            continue;
        }
        let needle: Vec<&str> = block.search.split('\n').collect();
        let same = |a: &str, b: &str| a.trim_end() == b.trim_end();
        let starts: Vec<usize> = if needle.len() > lines.len() {
            Vec::new()
        } else {
            (0..=lines.len() - needle.len())
                .filter(|&i| needle.iter().enumerate().all(|(k, n)| same(&lines[i + k], n)))
                .collect()
        };
        match starts.as_slice() {
            [] => return Err(EditError::NoMatch { index }),
            [start] => {
                lines.splice(*start..*start + needle.len(), replace);
            }
            many => return Err(EditError::Ambiguous { index, count: many.len() }),
        }
    }
    Ok(lines.join("\n"))
}

/// How to ask for edits, for a system prompt. Kept next to the parser
/// so the instructions and what's accepted can't drift apart.
pub fn protocol_instructions(lang: &str) -> String {
    format!(
        "To change an existing file, reply with SEARCH/REPLACE blocks:\n\n\
         <<<<<<< SEARCH\n\
         lines copied exactly from the current file\n\
         =======\n\
         the lines that replace them\n\
         >>>>>>> REPLACE\n\n\
         Each SEARCH must match exactly one place in the file; include enough surrounding lines to make it unique. \
         An empty SEARCH appends to the end of the file. \
         To write a new file, or to rewrite most of one, reply with the whole file in a single ```{lang} fenced block instead."
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const REPLY: &str = "Here is the fix.\n\
        <<<<<<< SEARCH\n\
        let speed = 1\n\
        =======\n\
        let speed = 2\n\
        >>>>>>> REPLACE\n\
        Done.";

    #[test]
    fn blocks_parse_and_apply() {
        let edit = parse_reply(REPLY, &["twe"]);
        let Edit::Blocks(blocks) = &edit else { panic!("{edit:?}") };
        assert_eq!(blocks.len(), 1);
        let out = apply("let a = 0\nlet speed = 1\nprint(a)", &edit).unwrap();
        assert_eq!(out, "let a = 0\nlet speed = 2\nprint(a)");
    }

    #[test]
    fn trailing_whitespace_and_crlf_still_match() {
        let blocks = parse_blocks("<<<<<<< SEARCH\nlet speed = 1  \n=======\nlet speed = 3\n>>>>>>> REPLACE");
        let out = apply_blocks("let speed = 1\r\nprint(speed)", &blocks).unwrap();
        assert_eq!(out, "let speed = 3\nprint(speed)");
    }

    #[test]
    fn a_search_must_match_exactly_once() {
        let blocks = parse_blocks(REPLY);
        assert_eq!(
            apply_blocks("print(1)", &blocks),
            Err(EditError::NoMatch { index: 0 })
        );
        assert_eq!(
            apply_blocks("let speed = 1\nlet speed = 1", &blocks),
            Err(EditError::Ambiguous { index: 0, count: 2 })
        );
    }

    #[test]
    fn empty_search_appends_and_empty_replace_deletes() {
        let blocks = parse_blocks(
            "<<<<<<< SEARCH\n=======\nprint(2)\n>>>>>>> REPLACE\n\
             <<<<<<< SEARCH\nprint(1)\n=======\n>>>>>>> REPLACE",
        );
        assert_eq!(apply_blocks("print(1)", &blocks).unwrap(), "print(2)");
        let new = parse_blocks("<<<<<<< SEARCH\n=======\nlet x = 1\n>>>>>>> REPLACE");
        assert_eq!(apply_blocks("", &new).unwrap(), "let x = 1");
    }

    #[test]
    fn blocks_apply_in_order() {
        let blocks = parse_blocks(
            "<<<<<<< SEARCH\na\n=======\nb\n>>>>>>> REPLACE\n\
             <<<<<<< SEARCH\nb\n=======\nc\n>>>>>>> REPLACE",
        );
        assert_eq!(apply_blocks("a", &blocks).unwrap(), "c");
    }

    #[test]
    fn whole_files_by_tag_then_untagged() {
        let tagged = "```python\nx = 1\n```\n```twe\nlet x = 1\n```";
        assert_eq!(fenced_file(tagged, &["twe"]).as_deref(), Some("let x = 1"));
        assert_eq!(fenced_file(tagged, &["python", "py"]).as_deref(), Some("x = 1"));
        assert_eq!(fenced_file("```\nlet y = 2\n```", &["twe"]).as_deref(), Some("let y = 2"));
        assert_eq!(fenced_file("```twe\nunclosed", &["twe"]), None);
        assert_eq!(parse_reply("no code here", &["twe"]), Edit::Nothing);
        assert_eq!(
            parse_reply("```twe\nlet x = 1\n```", &["twe"]),
            Edit::WholeFile("let x = 1".into())
        );
    }

    #[test]
    fn a_block_without_its_closing_marker_is_dropped() {
        assert!(parse_blocks("<<<<<<< SEARCH\na\n=======\nb\n").is_empty());
    }
}
