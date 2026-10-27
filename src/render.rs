//! A file to the page omamv shows (ADR-0006, ADR-0007, ADR-0010).
//!
//! Markdown goes through pulldown-cmark with GitHub's extensions. Whatever
//! HTML the file brings is cleaned by ammonia, so a script in a document never
//! runs. omamv's own pieces (highlighted code, diagrams, alert headings, task
//! boxes) are put in after the cleaning, through tokens the document cannot
//! guess.

use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::path::Path;
use std::sync::{Mutex, OnceLock};

use pulldown_cmark::{BlockQuoteKind, CodeBlockKind, Event, HeadingLevel, Options, Parser, Tag, TagEnd};

use crate::theme::{self, Palette};

/// The page for a file's text.
pub fn page(path: &Path, text: &str, palette: &Palette) -> String {
    let body = if crate::folder::is_markdown(path) {
        markdown(text, palette)
    } else {
        format!(r#"<pre class="plain">{}</pre>"#, esc(text))
    };
    wrap(&crate::folder::file_name(path), &body, palette)
}

/// The page around a body: the content policy lets nothing run but omamv's
/// own script, and loads nothing but omamv's own files, data, and pictures.
pub fn wrap(title: &str, body: &str, palette: &Palette) -> String {
    format!(
        r#"<!doctype html><html><head><meta charset="utf-8">
<meta http-equiv="Content-Security-Policy" content="default-src 'none'; img-src omamv: data: https:; style-src 'unsafe-inline' omamv:; script-src omamv:; font-src omamv:">
<title>{title}</title>
<style>{vars}</style>
<link rel="stylesheet" href="omamv://assets/page.css">
</head><body><main class="page">{body}</main><script src="omamv://assets/app.js"></script></body></html>"#,
        title = esc(title),
        vars = theme::page_vars(palette),
    )
}

pub fn esc(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            _ => out.push(c),
        }
    }
    out
}

/// Pieces omamv writes itself, held back from the cleaning.
struct Trusted {
    nonce: String,
    pieces: Vec<String>,
}

impl Trusted {
    fn new(text: &str) -> Self {
        // Unguessable enough: a document that wanted to forge a token would
        // have to contain its own hash and the moment it was drawn.
        let mut h = std::collections::hash_map::DefaultHasher::new();
        text.hash(&mut h);
        std::time::SystemTime::now().hash(&mut h);
        std::process::id().hash(&mut h);
        Trusted { nonce: format!("{:016x}", h.finish()), pieces: Vec::new() }
    }

    fn token(&mut self, html: String) -> String {
        self.pieces.push(html);
        format!("OMAMV{}X{}Z", self.nonce, self.pieces.len() - 1)
    }

    fn put_back(&self, mut html: String) -> String {
        for (i, piece) in self.pieces.iter().enumerate().rev() {
            html = html.replace(&format!("OMAMV{}X{}Z", self.nonce, i), piece);
        }
        html
    }
}

pub fn markdown(src: &str, palette: &Palette) -> String {
    let opts = Options::ENABLE_TABLES
        | Options::ENABLE_STRIKETHROUGH
        | Options::ENABLE_TASKLISTS
        | Options::ENABLE_FOOTNOTES
        | Options::ENABLE_GFM
        | Options::ENABLE_SMART_PUNCTUATION;
    let mut trusted = Trusted::new(src);
    let mut out: Vec<Event> = Vec::new();
    let mut code: Option<(String, String)> = None;
    let mut heading: Option<(HeadingLevel, Vec<Event>)> = None;
    let mut quotes: Vec<Option<BlockQuoteKind>> = Vec::new();
    let mut ids: HashMap<String, usize> = HashMap::new();

    for ev in Parser::new_ext(src, opts) {
        if let Some((_, body)) = code.as_mut() {
            match ev {
                Event::Text(t) => body.push_str(&t),
                Event::End(TagEnd::CodeBlock) => {
                    let (lang, body) = code.take().unwrap_or_default();
                    let html = code_block(&lang, &body, palette);
                    out.push(Event::Html(format!("{}\n", trusted.token(html)).into()));
                }
                _ => {}
            }
            continue;
        }
        if let Some((level, inner)) = heading.as_mut() {
            if let Event::End(TagEnd::Heading(_)) = ev {
                let level = *level;
                let text: String = inner
                    .iter()
                    .filter_map(|e| match e {
                        Event::Text(t) | Event::Code(t) => Some(t.to_string()),
                        _ => None,
                    })
                    .collect();
                let id = unique_id(&slug(&text), &mut ids);
                let inner = heading.take().map(|(_, e)| e).unwrap_or_default();
                let mut html = String::new();
                pulldown_cmark::html::push_html(&mut html, inner.into_iter());
                out.push(Event::Html(format!("<{level} id=\"{}\">{html}</{level}>\n", esc(&id)).into()));
            } else {
                inner.push(ev);
            }
            continue;
        }
        match ev {
            Event::Start(Tag::CodeBlock(kind)) => {
                let lang = match kind {
                    CodeBlockKind::Fenced(info) => info.split([' ', ',', '{']).next().unwrap_or("").to_lowercase(),
                    CodeBlockKind::Indented => String::new(),
                };
                code = Some((lang, String::new()));
            }
            Event::Start(Tag::Heading { level, .. }) => heading = Some((level, Vec::new())),
            Event::Start(Tag::BlockQuote(kind)) => {
                quotes.push(kind);
                match kind {
                    Some(kind) => {
                        let html = alert_open(kind);
                        out.push(Event::Html(format!("{}\n", trusted.token(html)).into()));
                    }
                    None => out.push(Event::Start(Tag::BlockQuote(None))),
                }
            }
            Event::End(TagEnd::BlockQuote(_)) => match quotes.pop().flatten() {
                Some(_) => out.push(Event::Html(format!("{}\n", trusted.token("</div>".into())).into())),
                None => out.push(Event::End(TagEnd::BlockQuote(None))),
            },
            Event::TaskListMarker(done) => {
                let html = if done {
                    format!(r#"<span class="cb on" aria-label="done">{CHECK}</span>"#)
                } else {
                    r#"<span class="cb" aria-label="open"></span>"#.to_string()
                };
                out.push(Event::InlineHtml(trusted.token(html).into()));
            }
            e => out.push(e),
        }
    }

    let mut html = String::with_capacity(src.len() * 2);
    pulldown_cmark::html::push_html(&mut html, out.into_iter());
    trusted.put_back(clean(&html))
}

/// Raw HTML in a document keeps its structure and loses anything that runs.
fn clean(html: &str) -> String {
    static CLEANER: OnceLock<ammonia::Builder<'static>> = OnceLock::new();
    CLEANER
        .get_or_init(|| {
            let mut b = ammonia::Builder::default();
            b.add_tags(["details", "summary", "kbd", "mark", "sup", "sub", "picture", "source"])
                .add_generic_attributes(["id", "align"])
                .add_tag_attributes("img", ["width", "height", "align"])
                .add_tag_attributes("source", ["srcset", "media"])
                .add_tag_attributes("th", ["style"])
                .add_tag_attributes("td", ["style"])
                .add_tag_attributes("details", ["open"])
                .add_tag_attributes("ol", ["start"])
                .filter_style_properties(["text-align"].into_iter().collect())
                .link_rel(Some("noreferrer"));
            b
        })
        .clean(html)
        .to_string()
}

const CHECK: &str = r#"<svg viewBox="0 0 16 16" width="13" height="13" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M3.5 8.5 6.5 11.5 12.5 5"/></svg>"#;

fn alert_open(kind: BlockQuoteKind) -> String {
    let (class, title, icon) = match kind {
        BlockQuoteKind::Note => ("note", "Note", r#"<circle cx="8" cy="8" r="6"/><path d="M8 7.2v4M8 5v.2"/>"#),
        BlockQuoteKind::Tip => ("tip", "Tip", r#"<path d="M6 12.5h4M6.5 14.5h3M8 1.8a4.2 4.2 0 0 0-2.4 7.6c.5.4.9 1 .9 1.6h3c0-.6.4-1.2.9-1.6A4.2 4.2 0 0 0 8 1.8z"/>"#),
        BlockQuoteKind::Important => ("important", "Important", r#"<path d="M2.5 3.5h11v7h-6l-3 2.5v-2.5h-2z"/><path d="M8 5.2v2.6M8 9.4v.2"/>"#),
        BlockQuoteKind::Warning => ("warning", "Warning", r#"<path d="M8 2 14.5 13.5h-13z"/><path d="M8 6.5v3M8 11.4v.2"/>"#),
        BlockQuoteKind::Caution => ("caution", "Caution", r#"<path d="M5.5 1.8h5l3.7 3.7v5l-3.7 3.7h-5l-3.7-3.7v-5z"/><path d="M8 5v3.6M8 10.6v.2"/>"#),
    };
    format!(
        r#"<div class="alert {class}"><div class="alert-t"><svg viewBox="0 0 16 16" width="16" height="16" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">{icon}</svg>{title}</div>"#
    )
}

/// GitHub's heading ids: lower case, spaces to hyphens, punctuation dropped.
pub fn slug(text: &str) -> String {
    text.trim()
        .to_lowercase()
        .chars()
        .filter_map(|c| match c {
            c if c.is_alphanumeric() || c == '-' || c == '_' => Some(c),
            ' ' => Some('-'),
            _ => None,
        })
        .collect()
}

fn unique_id(base: &str, seen: &mut HashMap<String, usize>) -> String {
    let n = seen.entry(base.to_owned()).or_insert(0);
    let id = if *n == 0 { base.to_owned() } else { format!("{base}-{n}") };
    *n += 1;
    id
}

fn code_block(lang: &str, body: &str, palette: &Palette) -> String {
    match lang {
        "mermaid" => format!(r#"<pre class="mermaid">{}</pre>"#, esc(body)),
        "plantuml" | "puml" | "uml" => plantuml(body, palette),
        _ => highlight(lang, body),
    }
}

fn syntaxes() -> &'static syntect::parsing::SyntaxSet {
    static SET: OnceLock<syntect::parsing::SyntaxSet> = OnceLock::new();
    SET.get_or_init(syntect::parsing::SyntaxSet::load_defaults_newlines)
}

/// Code coloured by its language, in classes the page's palette colours.
pub fn highlight(lang: &str, code: &str) -> String {
    use syntect::html::{ClassStyle, ClassedHTMLGenerator};
    use syntect::util::LinesWithEndings;
    let set = syntaxes();
    let lang = match lang {
        "sh" | "shell" | "zsh" | "console" => "bash",
        "js" | "javascript" | "jsx" | "ts" | "typescript" | "tsx" => "js",
        "yml" => "yaml",
        "py" | "python3" => "python",
        "c++" => "cpp",
        other => other,
    };
    let syntax = (!lang.is_empty()).then(|| set.find_syntax_by_token(lang)).flatten();
    let Some(syntax) = syntax else {
        return format!("<pre><code>{}</code></pre>", esc(code));
    };
    let mut generator = ClassedHTMLGenerator::new_with_class_style(syntax, set, ClassStyle::SpacedPrefixed { prefix: "sy-" });
    for line in LinesWithEndings::from(code) {
        if generator.parse_html_for_line_which_includes_newline(line).is_err() {
            return format!("<pre><code>{}</code></pre>", esc(code));
        }
    }
    format!("<pre><code>{}</code></pre>", generator.finalize())
}

/// PlantUML through the local `plantuml` command, coloured by the palette and
/// kept for as long as omamv runs: its Java takes half a second to start.
/// The picture goes in as an image, so nothing in it can run.
fn plantuml(src: &str, palette: &Palette) -> String {
    static CACHE: OnceLock<Mutex<HashMap<u64, String>>> = OnceLock::new();
    let mut h = std::collections::hash_map::DefaultHasher::new();
    src.hash(&mut h);
    palette.hash(&mut h);
    let key = h.finish();
    let cache = CACHE.get_or_init(Default::default);
    if let Some(hit) = cache.lock().ok().and_then(|c| c.get(&key).cloned()) {
        return hit;
    }
    let html = match run_plantuml(src, palette) {
        Ok(svg) => {
            use base64::Engine;
            format!(
                r#"<figure class="diagram plantuml"><img alt="PlantUML diagram" src="data:image/svg+xml;base64,{}"></figure>"#,
                base64::engine::general_purpose::STANDARD.encode(svg)
            )
        }
        Err(why) => format!(r#"<pre class="diagram-failed" data-error="{}"><code>{}</code></pre>"#, esc(&why), esc(src)),
    };
    if let Ok(mut c) = cache.lock() {
        c.insert(key, html.clone());
    }
    html
}

fn run_plantuml(src: &str, p: &Palette) -> Result<Vec<u8>, String> {
    use std::io::Write;
    use std::process::{Command, Stdio};
    let body = src.trim();
    let body = body.strip_prefix("@startuml").unwrap_or(body);
    let body = body.trim_end().strip_suffix("@enduml").unwrap_or(body);
    let style = format!(
        "<style>\nroot {{\n BackgroundColor transparent\n FontColor {fg}\n FontName \"Adwaita Sans\"\n LineColor {line}\n}}\n\
element {{\n BackgroundColor {raised}\n LineColor {muted}\n}}\narrow {{\n LineColor {line}\n FontColor {fg}\n}}\n\
note {{\n BackgroundColor {raised}\n LineColor {muted}\n}}\nlifeLine {{\n LineColor {muted}\n}}\n</style>\n",
        fg = p.foreground,
        line = p.fg_over_bg(0.7),
        raised = p.lighter_background,
        muted = p.muted,
    );
    let input = format!("@startuml\n{style}{body}\n@enduml\n");
    let mut child = Command::new("plantuml")
        .args(["-tsvg", "-pipe", "-charset", "UTF-8", "-nometadata"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|_| "PlantUML is not installed (the plantuml package draws these)".to_string())?;
    child
        .stdin
        .take()
        .ok_or("PlantUML did not start")?
        .write_all(input.as_bytes())
        .map_err(|e| e.to_string())?;
    let out = child.wait_with_output().map_err(|e| e.to_string())?;
    if !out.status.success() || out.stdout.is_empty() {
        return Err("PlantUML could not draw this diagram".into());
    }
    Ok(out.stdout)
}

impl Hash for Palette {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.foreground.hash(state);
        self.background.hash(state);
        self.lighter_background.hash(state);
        self.muted.hash(state);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn md(s: &str) -> String {
        markdown(s, &crate::theme::fallback(true))
    }

    #[test]
    fn scripts_in_a_document_never_reach_the_page() {
        let html = md("<script>alert(1)</script>\n\n<img src=x onerror=\"alert(2)\">\n\n[x](javascript:alert(3))");
        assert!(!html.contains("<script"));
        assert!(!html.contains("onerror"));
        assert!(!html.contains("javascript:"));
    }

    #[test]
    fn a_forged_token_stays_text() {
        let html = md("OMAMV0000000000000000X0Z and ```rust\nfn x(){}\n```");
        assert!(html.contains("OMAMV0000000000000000X0Z"));
    }

    #[test]
    fn headings_get_github_ids() {
        let html = md("# Open a folder\n## Open a folder\n## Keys & more");
        assert!(html.contains(r#"<h1 id="open-a-folder">"#));
        assert!(html.contains(r#"<h2 id="open-a-folder-1">"#));
        assert!(html.contains(r#"<h2 id="keys--more">"#));
    }

    #[test]
    fn alerts_tasks_and_code_are_omamvs_own() {
        let html = md("> [!WARNING]\n> Careful.\n\n- [x] done\n- [ ] open\n\n```rust\nlet x = 1;\n```\n\n```mermaid\ngraph LR; A-->B\n```");
        assert!(html.contains(r#"<div class="alert warning">"#));
        assert!(html.contains("Careful."));
        assert!(html.contains(r#"class="cb on""#));
        assert!(html.contains("sy-keyword") || html.contains("sy-storage"));
        assert!(html.contains(r#"<pre class="mermaid">graph LR; A--&gt;B"#));
    }

    #[test]
    fn plain_text_is_shown_as_it_is() {
        let page = page(Path::new("notes.txt"), "a <b>\n  b", &crate::theme::fallback(true));
        assert!(page.contains(r#"<pre class="plain">a &lt;b&gt;"#));
    }

    #[test]
    fn table_alignment_survives_the_cleaning() {
        let html = md("| a | b |\n|:-:|--:|\n| 1 | 2 |");
        assert!(html.contains("text-align"), "{html}");
    }
}
