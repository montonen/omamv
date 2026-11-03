//! Search all files (ADR-0025): the text a reader sees in each file, line by
//! line, and plain-text matching over it. A query with a capital letter
//! matches case; one without ignores it. No GTK here: the window runs it off
//! the main thread.

use std::ops::Range;
use std::path::{Path, PathBuf};

use pulldown_cmark::{CodeBlockKind, Event, LinkType, Parser, Tag, TagEnd};

/// Files larger than this are left out: not a folder of notes.
const MAX_BYTES: u64 = 4 << 20;
/// A line longer than this is shown cut around its first match.
const SHOWN_CHARS: usize = 150;

/// One line of what a file shows: its line number in the source and the text,
/// with runs of white space made one space.
#[derive(Debug, Clone, PartialEq)]
pub struct Line {
    pub no: u32,
    pub text: String,
    /// `text` with each character lowercased, same number of characters.
    folded: String,
}

pub struct Doc {
    /// From the root.
    pub path: PathBuf,
    pub lines: Vec<Line>,
}

#[derive(Debug, Clone)]
pub struct Hit {
    /// Index into the corpus.
    pub doc: usize,
    pub line: u32,
    /// What the panel shows: the line, cut around the match when long.
    pub shown: String,
    /// The matches in `shown`, in characters.
    pub marks: Vec<Range<usize>>,
    /// The whole line, for finding it in the page.
    pub text: String,
    /// The match opened, counted as the page's find counts (ignoring case):
    /// within the line, and within the file.
    pub nth_in_line: usize,
    pub nth_in_file: usize,
}

#[derive(Debug, Default)]
pub struct Found {
    /// The first hits, in the corpus's order.
    pub hits: Vec<Hit>,
    /// Every line that matched, and the files they are in.
    pub lines: usize,
    pub files: usize,
}

/// The text of every file, in the order given (the window gives newest first).
pub fn corpus(root: &Path, files: &[PathBuf]) -> Vec<Doc> {
    files
        .iter()
        .filter_map(|f| {
            let full = root.join(f);
            if std::fs::metadata(&full).ok()?.len() > MAX_BYTES {
                return None;
            }
            let bytes = std::fs::read(&full).ok()?;
            let text = String::from_utf8_lossy(&bytes);
            Some(Doc { path: f.clone(), lines: lines_of(f, &text) })
        })
        .collect()
}

/// The lines a reader sees in a file, by its kind.
pub fn lines_of(path: &Path, src: &str) -> Vec<Line> {
    let raw = if crate::folder::is_html(path) {
        html_lines(src)
    } else if crate::folder::is_markdown(path) {
        markdown_lines(src)
    } else {
        src.lines().enumerate().map(|(i, l)| (i as u32 + 1, l.to_owned())).collect()
    };
    raw.into_iter()
        .filter_map(|(no, t)| {
            let text = squash(&t);
            (!text.is_empty()).then(|| Line { no, folded: fold(&text), text })
        })
        .collect()
}

/// Runs of white space as one space, none at the ends: as the page's find
/// sees text.
pub fn squash(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for word in s.split_whitespace() {
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(word);
    }
    out
}

/// Each character lowercased, keeping one character for one: the page's
/// find does the same, so both count the same matches.
pub fn fold(s: &str) -> String {
    s.chars().map(fold_char).collect()
}

fn fold_char(c: char) -> char {
    let mut l = c.to_lowercase();
    match (l.next(), l.next()) {
        (Some(x), None) => x,
        _ => c,
    }
}

/// Markdown as the page shows it: text, inline code and code blocks, front
/// matter as `key: value`; not its syntax, link targets, HTML, or diagrams.
fn markdown_lines(src: &str) -> Vec<(u32, String)> {
    let starts: Vec<usize> = std::iter::once(0).chain(src.match_indices('\n').map(|(i, _)| i + 1)).collect();
    let line_of = |at: usize| starts.partition_point(|&s| s <= at) as u32; // 1-based
    let mut lines: std::collections::BTreeMap<u32, String> = Default::default();
    let mut push = |no: u32, s: &str| lines.entry(no).or_default().push_str(s);

    let mut skip_block = false;
    let mut in_code = false;
    let mut in_meta = false;
    let mut wiki_bare = false;
    let mut last_line = 1;
    for (ev, range) in Parser::new_ext(src, crate::render::OPTS).into_offset_iter() {
        match ev {
            Event::Start(Tag::CodeBlock(kind)) => {
                let lang = match &kind {
                    CodeBlockKind::Fenced(info) => info.split_whitespace().next().unwrap_or("").to_owned(),
                    CodeBlockKind::Indented => String::new(),
                };
                skip_block = matches!(lang.as_str(), "mermaid" | "plantuml" | "puml" | "uml");
                in_code = true;
            }
            Event::End(TagEnd::CodeBlock) => {
                in_code = false;
                skip_block = false;
            }
            Event::Start(Tag::MetadataBlock(_)) => in_meta = true,
            Event::End(TagEnd::MetadataBlock(_)) => in_meta = false,
            Event::Start(Tag::Link { link_type: LinkType::WikiLink { has_pothole: false }, .. }) => wiki_bare = true,
            Event::End(TagEnd::Link) => wiki_bare = false,
            Event::End(TagEnd::TableCell) => push(last_line, " "),
            Event::Text(t) if in_meta => {
                let first = line_of(range.start);
                for (i, l) in t.lines().enumerate() {
                    let l = l.trim();
                    if l.is_empty() || l.starts_with('#') {
                        continue;
                    }
                    let shown = match l.split_once(':') {
                        Some((k, v)) if !l.starts_with('-') => {
                            format!("{}: {}", k.trim(), unquote(v.trim()))
                        }
                        _ => unquote(l.trim_start_matches('-').trim()),
                    };
                    push(first + i as u32, &shown);
                }
            }
            Event::Text(t) if in_code => {
                if skip_block {
                    continue;
                }
                let first = line_of(range.start);
                for (i, part) in t.split('\n').enumerate() {
                    if !part.is_empty() {
                        push(first + i as u32, part);
                    }
                }
            }
            Event::Text(t) | Event::Code(t) => {
                last_line = line_of(range.start);
                if wiki_bare {
                    push(last_line, &crate::wiki::label(&t));
                } else {
                    push(last_line, &t);
                }
            }
            _ => {}
        }
    }
    lines.into_iter().collect()
}

fn unquote(s: &str) -> String {
    for q in ['"', '\''] {
        if s.len() >= 2 && s.starts_with(q) && s.ends_with(q) {
            return s[1..s.len() - 1].to_owned();
        }
    }
    s.to_owned()
}

/// An HTML report's visible text, by source line: tags out, scripts and
/// styles out, the common entities read.
fn html_lines(src: &str) -> Vec<(u32, String)> {
    let mut out: Vec<(u32, String)> = Vec::new();
    let mut line = 1u32;
    let mut cur = String::new();
    let mut chars = src.char_indices().peekable();
    let lower = src.to_ascii_lowercase();
    let mut flush = |line: u32, cur: &mut String| {
        if !cur.trim().is_empty() {
            out.push((line, entities(cur)));
        }
        cur.clear();
    };
    while let Some((i, c)) = chars.next() {
        match c {
            '\n' => {
                flush(line, &mut cur);
                line += 1;
            }
            '<' => {
                // The tag's name: a script or style is skipped to its end.
                let name: String = lower[i + 1..].chars().take_while(|c| c.is_ascii_alphanumeric()).collect();
                let end_at = if name == "script" || name == "style" { lower[i..].find(&format!("</{name}")).map(|e| i + e) } else { Some(i) };
                let Some(from) = end_at else { break };
                let Some(close) = src[from..].find('>').map(|e| from + e) else {
                    break;
                };
                cur.push(' ');
                while let Some(&(j, c)) = chars.peek() {
                    if j > close {
                        break;
                    }
                    if c == '\n' {
                        flush(line, &mut cur);
                        line += 1;
                    }
                    chars.next();
                }
            }
            c => cur.push(c),
        }
    }
    flush(line, &mut cur);
    out
}

fn entities(s: &str) -> String {
    s.replace("&nbsp;", " ").replace("&lt;", "<").replace("&gt;", ">").replace("&quot;", "\"").replace("&#39;", "'").replace("&amp;", "&")
}

/// Where `needle` starts in `hay`, in characters, not overlapping.
fn starts(hay: &str, needle: &str) -> Vec<usize> {
    let mut out = Vec::new();
    let mut from = 0;
    let mut chars_before = 0;
    while let Some(b) = hay[from..].find(needle) {
        chars_before += hay[from..from + b].chars().count();
        out.push(chars_before);
        let step = b + needle.len();
        chars_before += needle.chars().count();
        from += step;
    }
    out
}

/// Every line that has `query` in it, newest file first, the first `max`
/// kept. Empty or blank queries find nothing.
pub fn search(corpus: &[Doc], query: &str, max: usize) -> Found {
    let q = squash(query);
    let mut found = Found::default();
    if q.is_empty() {
        return found;
    }
    let case = q.chars().any(char::is_uppercase);
    let fq = fold(&q);
    let len = q.chars().count();
    for (d, doc) in corpus.iter().enumerate() {
        let mut before = 0; // matches in this file's earlier lines, ignoring case
        let mut in_file = false;
        for line in &doc.lines {
            let loose = starts(&line.folded, &fq);
            if loose.is_empty() {
                continue;
            }
            let at = if case { starts(&line.text, &q) } else { loose.clone() };
            if let Some(&first) = at.first() {
                found.lines += 1;
                in_file = true;
                if found.hits.len() < max {
                    let nth = loose.iter().take_while(|&&s| s < first).count();
                    let (shown, marks) = cut(&line.text, &at, len);
                    found.hits.push(Hit { doc: d, line: line.no, shown, marks, text: line.text.clone(), nth_in_line: nth, nth_in_file: before + nth });
                }
            }
            before += loose.len();
        }
        if in_file {
            found.files += 1;
        }
    }
    found
}

/// The line as shown, cut to fit around the first match, and the matches in it.
fn cut(text: &str, at: &[usize], len: usize) -> (String, Vec<Range<usize>>) {
    let chars: Vec<char> = text.chars().collect();
    if chars.len() <= SHOWN_CHARS {
        return (text.to_owned(), at.iter().map(|&s| s..s + len).collect());
    }
    let from = at[0].saturating_sub(40);
    let to = (from + SHOWN_CHARS).min(chars.len());
    let lead = usize::from(from > 0);
    let mut shown: String = if from > 0 { "…".into() } else { String::new() };
    shown.extend(&chars[from..to]);
    if to < chars.len() {
        shown.push('…');
    }
    let marks = at.iter().filter(|&&s| s >= from && s + len <= to).map(|&s| s - from + lead..s - from + lead + len).collect();
    (shown, marks)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn md(src: &str) -> Vec<(u32, String)> {
        lines_of(Path::new("a.md"), src).into_iter().map(|l| (l.no, l.text)).collect()
    }

    fn doc(name: &str, src: &str) -> Doc {
        Doc { path: PathBuf::from(name), lines: lines_of(Path::new(name), src) }
    }

    #[test]
    fn markdown_shows_its_words_not_its_syntax() {
        let got = md("# The *title*\n\nSee [the docs](https://x.invalid/secret) and `code`.\nNext   line.\n");
        assert_eq!(got, [(1, "The title".into()), (3, "See the docs and code.".into()), (4, "Next line.".into())]);
    }

    #[test]
    fn code_counts_but_diagrams_do_not() {
        let got = md("```rust\nfn main() {}\n\nlet x = 1;\n```\n\n```mermaid\ngraph TD; A-->B\n```\n");
        assert_eq!(got, [(2, "fn main() {}".into()), (4, "let x = 1;".into())]);
    }

    #[test]
    fn front_matter_reads_as_key_and_value() {
        let got = md("---\ntitle: \"Plan\"\ntags:\n  - one\n---\n\nBody\n");
        assert_eq!(got, [(2, "title: Plan".into()), (3, "tags:".into()), (4, "one".into()), (7, "Body".into())]);
    }

    #[test]
    fn table_cells_keep_apart() {
        let got = md("| a | b |\n|---|---|\n| one | two |\n");
        assert_eq!(got.last(), Some(&(3, "one two".into())));
    }

    #[test]
    fn html_reports_show_text_without_scripts() {
        let got = lines_of(Path::new("r.html"), "<html><style>p{}</style>\n<p>Hello &amp; <b>bye</b></p>\n<script>let secret = 1;\n</script><p>end</p>");
        let got: Vec<_> = got.into_iter().map(|l| (l.no, l.text)).collect();
        assert_eq!(got, [(2, "Hello & bye".into()), (4, "end".into())]);
    }

    #[test]
    fn lower_case_ignores_case_and_a_capital_matches_it() {
        let c = [doc("a.md", "Folder here\n\nfolder there\n")];
        assert_eq!(search(&c, "folder", 10).lines, 2);
        let f = search(&c, "Folder", 10);
        assert_eq!((f.lines, f.hits[0].line), (1, 1));
    }

    #[test]
    fn hits_count_as_the_page_counts() {
        // "Fold" with a capital: the second line's match is the page's third,
        // and the first in that line after one that differs in case.
        let c = [doc("a.md", "fold one\n\nfold and Fold\n")];
        let f = search(&c, "Fold", 10);
        assert_eq!(f.hits.len(), 1);
        let h = &f.hits[0];
        assert_eq!((h.line, h.nth_in_line, h.nth_in_file), (3, 1, 2));
        assert_eq!(h.marks, vec![9..13]);
    }

    #[test]
    fn totals_go_past_the_shown() {
        let c = [doc("a.txt", "x\nx\nx\n"), doc("b.txt", "y\nx\n")];
        let f = search(&c, "x", 2);
        assert_eq!((f.hits.len(), f.lines, f.files), (2, 4, 2));
        assert!(search(&c, "  ", 2).hits.is_empty());
    }

    #[test]
    fn a_long_line_is_cut_around_its_match() {
        let long = format!("{}needle{}", "a".repeat(300), "b".repeat(300));
        let c = [doc("a.txt", &long)];
        let h = &search(&c, "needle", 1).hits[0];
        assert!(h.shown.starts_with('…') && h.shown.ends_with('…'), "{}", h.shown);
        let marked: String = h.shown.chars().skip(h.marks[0].start).take(6).collect();
        assert_eq!(marked, "needle");
    }

    #[test]
    fn non_ascii_keeps_its_place() {
        let c = [doc("a.txt", "Åsa och älgen; ÄLGEN igen")];
        let f = search(&c, "älgen", 1);
        let h = &f.hits[0];
        assert_eq!(h.marks, [8..13, 15..20]);
    }
}
