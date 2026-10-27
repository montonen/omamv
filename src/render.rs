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
    if crate::folder::is_html(path) {
        return html_page(text);
    }
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
    let s = crate::settings::get();
    let web = if s.web_pictures { " https:" } else { "" };
    format!(
        r#"<!doctype html><html><head><meta charset="utf-8">
<meta http-equiv="Content-Security-Policy" content="default-src 'none'; img-src omamv: data:{web}; style-src 'unsafe-inline' omamv:; script-src omamv:; font-src omamv:">
<title>{title}</title>
<style>{vars} .page{{max-width:{width}}}</style>
<link rel="stylesheet" href="omamv://assets/page.css">
</head><body><main class="page">{body}</main><script src="omamv://assets/app.js"></script></body></html>"#,
        title = esc(title),
        vars = theme::page_vars(palette),
        width = s.width.css(),
    )
}

/// The content policy of an HTML page from the folder (ADR-0016): its own
/// styles, fonts and pictures; no script of its own, no frame, no form, no
/// network but web pictures when allowed. omamv's page helper is added by
/// the window through WebKit, which this policy does not stop.
pub fn html_policy() -> String {
    let web = if crate::settings::get().web_pictures { " https:" } else { "" };
    format!(
        "default-src 'none'; script-src 'none'; img-src omamv: data:{web}; style-src 'unsafe-inline' omamv:; \
         font-src omamv: data:; media-src omamv:; frame-src 'none'; form-action 'none'; base-uri 'none'"
    )
}

/// An HTML page as its author made it, with the policy in front of anything
/// the author wrote: a `<meta>` policy only counts from where it stands. The
/// text has been read as UTF-8 already, so that is what the page says it is.
pub fn html_page(text: &str) -> String {
    let head = format!(
        r#"<meta charset="utf-8"><meta http-equiv="Content-Security-Policy" content="{}"><style>html{{background:#fff;color:#000;color-scheme:light}}</style>"#,
        html_policy()
    );
    // After a leading doctype, so the page keeps its standards mode.
    let lower = text.get(..text.len().min(512)).unwrap_or("").to_ascii_lowercase();
    let at = if lower.trim_start().starts_with("<!doctype") {
        lower.find('>').map(|i| i + 1).unwrap_or(0)
    } else {
        0
    };
    format!("{}{head}{}", &text[..at], &text[at..])
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
        | Options::ENABLE_GFM;
    let mut trusted = Trusted::new(src);
    let mut out: Vec<Event> = Vec::new();
    let mut code: Option<(String, String)> = None;
    let mut heading: Option<(HeadingLevel, Vec<Event>)> = None;
    let mut quotes: Vec<Option<BlockQuoteKind>> = Vec::new();
    let mut ids = std::collections::HashSet::new();
    let mut diagrams: Vec<(usize, String)> = Vec::new();

    for ev in Parser::new_ext(src, opts) {
        if let Some((_, body)) = code.as_mut() {
            match ev {
                Event::Text(t) => body.push_str(&t),
                Event::End(TagEnd::CodeBlock) => {
                    let (lang, body) = code.take().unwrap_or_default();
                    if matches!(lang.as_str(), "plantuml" | "puml" | "uml") {
                        let token = trusted.token(String::new());
                        diagrams.push((trusted.pieces.len() - 1, body));
                        out.push(Event::Html(format!("{token}\n").into()));
                    } else {
                        let html = code_block(&lang, &body);
                        out.push(Event::Html(format!("{}\n", trusted.token(html)).into()));
                    }
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

    if !diagrams.is_empty() {
        let sources: Vec<String> = diagrams.iter().map(|(_, b)| b.clone()).collect();
        for ((at, _), html) in diagrams.iter().zip(plantuml_all(&sources, palette)) {
            trusted.pieces[*at] = html;
        }
    }
    let mut html = String::with_capacity(src.len() * 2);
    pulldown_cmark::html::push_html(&mut html, out.into_iter());
    trusted.put_back(clean(&html, &trusted.nonce))
}

/// Raw HTML in a document keeps its structure and loses anything that runs.
/// `nonce` is the page's own: an attribute that carries one of omamv's
/// tokens is dropped, so a piece is only ever put back as content.
fn clean(html: &str, nonce: &str) -> String {
    let _ = nonce;
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
                .add_allowed_classes("div", ["footnote-definition"])
                .add_allowed_classes("sup", ["footnote-reference", "footnote-definition-label"])
                .attribute_filter(|_, _, value| (!value.contains("OMAMV")).then(|| value.into()))
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

/// GitHub's heading ids: lower case, spaces to hyphens, punctuation and
/// symbols dropped; letters, digits and marks of any script kept.
pub fn slug(text: &str) -> String {
    let punctuation = |c: char| {
        c.is_ascii_punctuation() && c != '-' && c != '_'
            || matches!(c as u32, 0x2000..=0x206F | 0x3000..=0x303F | 0xFF01..=0xFF0F | 0x2190..=0x2BFF)
    };
    text.trim()
        .to_lowercase()
        .chars()
        .filter_map(|c| match c {
            ' ' => Some('-'),
            c if c.is_whitespace() || c.is_control() || punctuation(c) => None,
            c => Some(c),
        })
        .collect()
}

/// `foo`, then `foo-1`, `foo-2`: never an id already taken, by a heading
/// called `foo-1` either.
fn unique_id(base: &str, taken: &mut std::collections::HashSet<String>) -> String {
    let mut id = base.to_owned();
    let mut n = 0;
    while taken.contains(&id) {
        n += 1;
        id = format!("{base}-{n}");
    }
    taken.insert(id.clone());
    id
}

fn code_block(lang: &str, body: &str) -> String {
    match lang {
        "mermaid" => format!(r#"<pre class="mermaid">{}</pre>"#, esc(body)),
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

/// PlantUML through the local `plantuml` command, coloured by the palette.
/// Every diagram on a page goes to one run (its Java takes a quarter of a
/// second to start), and what was drawn is kept for as long as omamv runs.
/// PlantUML runs in its SANDBOX profile, from an empty folder, with a time
/// limit: a document must not get it to read other files (`!include`,
/// `%load_json`) or the network. The picture goes in as an image, so nothing
/// in it can run.
fn plantuml_all(sources: &[String], palette: &Palette) -> Vec<String> {
    static CACHE: OnceLock<Mutex<HashMap<u64, String>>> = OnceLock::new();
    let cache = CACHE.get_or_init(Default::default);
    let key = |src: &str| {
        let mut h = std::collections::hash_map::DefaultHasher::new();
        src.hash(&mut h);
        palette.hash(&mut h);
        h.finish()
    };
    let mut out: Vec<Option<String>> = sources
        .iter()
        .map(|src| cache.lock().ok().and_then(|c| c.get(&key(src)).cloned()))
        .collect();
    let missing: Vec<usize> = (0..sources.len()).filter(|i| out[*i].is_none()).collect();
    if !missing.is_empty() {
        let wanted: Vec<&str> = missing.iter().map(|i| sources[*i].as_str()).collect();
        match run_plantuml(&wanted, palette) {
            Ok(drawn) => {
                for (i, result) in missing.iter().zip(drawn) {
                    let html = match result {
                        Ok(svg) => {
                            use base64::Engine;
                            format!(
                                r#"<figure class="diagram plantuml"><img alt="PlantUML diagram" src="data:image/svg+xml;base64,{}"></figure>"#,
                                base64::engine::general_purpose::STANDARD.encode(svg)
                            )
                        }
                        Err(why) => failed(&why, &sources[*i]),
                    };
                    if let Ok(mut c) = cache.lock() {
                        c.insert(key(&sources[*i]), html.clone());
                    }
                    out[*i] = Some(html);
                }
            }
            // Not installed, or timed out: not kept, so it can work next time.
            Err(why) => {
                for i in missing {
                    out[i] = Some(failed(&why, &sources[i]));
                }
            }
        }
    }
    out.into_iter().map(Option::unwrap_or_default).collect()
}

fn failed(why: &str, src: &str) -> String {
    format!(r#"<pre class="diagram-failed" data-error="{}"><code>{}</code></pre>"#, esc(why), esc(src))
}

const PLANTUML_LIMIT: std::time::Duration = std::time::Duration::from_secs(10);
const SPLIT: &str = "OMAMV-PLANTUML-SPLIT";

/// Draw `sources` in one PlantUML run: one result per diagram, or an error
/// for the whole run.
fn run_plantuml(sources: &[&str], p: &Palette) -> Result<Vec<Result<Vec<u8>, String>>, String> {
    use std::io::{Read, Write};
    use std::process::{Command, Stdio};
    let style = format!(
        "<style>\nroot {{\n BackgroundColor transparent\n FontColor {fg}\n FontName \"Adwaita Sans\"\n LineColor {line}\n}}\n\
element {{\n BackgroundColor {raised}\n LineColor {muted}\n}}\narrow {{\n LineColor {line}\n FontColor {fg}\n}}\n\
note {{\n BackgroundColor {raised}\n LineColor {muted}\n}}\nlifeLine {{\n LineColor {muted}\n}}\n</style>\n",
        fg = p.foreground,
        line = p.fg_over_bg(0.7),
        raised = p.lighter_background,
        muted = p.muted,
    );
    let mut input = String::new();
    for src in sources {
        let body = src.trim();
        let body = body.strip_prefix("@startuml").unwrap_or(body);
        let body = body.trim_end().strip_suffix("@enduml").unwrap_or(body);
        input.push_str(&format!("@startuml\n{style}{body}\n@enduml\n"));
    }
    let mut child = Command::new("plantuml")
        .args(["-tsvg", "-pipe", "-charset", "UTF-8", "-nometadata", "-pipedelimitor", SPLIT])
        .env("PLANTUML_SECURITY_PROFILE", "SANDBOX")
        .env_remove("JAVA_TOOL_OPTIONS")
        .current_dir(std::env::temp_dir())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|_| "PlantUML is not installed (the plantuml package draws these)".to_string())?;
    let mut stdin = child.stdin.take().ok_or("PlantUML did not start")?;
    let mut stdout = child.stdout.take().ok_or("PlantUML did not start")?;
    let writer = std::thread::spawn(move || {
        let _ = stdin.write_all(input.as_bytes());
    });
    let reader = std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = stdout.read_to_end(&mut buf);
        buf
    });
    let started = std::time::Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if started.elapsed() > PLANTUML_LIMIT => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("PlantUML took more than {} seconds", PLANTUML_LIMIT.as_secs()));
            }
            Ok(None) => std::thread::sleep(std::time::Duration::from_millis(15)),
            Err(e) => return Err(e.to_string()),
        }
    }
    let _ = writer.join();
    let out = reader.join().unwrap_or_default();
    let text = String::from_utf8_lossy(&out);
    let parts: Vec<&str> = text.split(SPLIT).map(str::trim).filter(|s| !s.is_empty()).collect();
    if parts.len() != sources.len() {
        return Err("PlantUML could not draw these diagrams".into());
    }
    Ok(parts
        .into_iter()
        .map(|svg| {
            if svg.contains("Syntax Error") || !svg.starts_with("<svg") {
                Err("PlantUML could not draw this diagram".to_string())
            } else {
                Ok(svg.as_bytes().to_vec())
            }
        })
        .collect())
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
    fn heading_ids_never_repeat() {
        let html = md("# foo\n# foo-1\n# foo\n# A -- B\n# Café ünïcode");
        assert!(html.contains(r#"id="foo""#));
        assert_eq!(html.matches(r#"id="foo-1""#).count(), 1, "{html}");
        assert!(html.contains(r#"id="foo-2""#), "{html}");
        assert!(html.contains(r#"id="a----b""#), "{html}");
        assert!(html.contains(r#"id="café-ünïcode""#), "{html}");
    }

    #[test]
    fn a_token_inside_an_attribute_is_dropped() {
        let html = md("<div title=\"\n\n```mermaid\nx\n```\n\n<div x=\">");
        assert!(!html.contains("title=\"\n<pre"), "{html}");
    }

    #[test]
    fn footnotes_keep_their_classes() {
        let html = md("Text[^n].\n\n[^n]: The note.");
        assert!(html.contains("footnote-definition"), "{html}");
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
    fn an_html_page_gets_the_policy_first() {
        let page = html_page("<!DOCTYPE html><html><head><script>x()</script></head><body>Hi</body></html>");
        let policy = page.find("Content-Security-Policy").unwrap();
        assert!(page.starts_with("<!DOCTYPE html><meta charset=\"utf-8\">"), "{page}");
        assert!(policy < page.find("<script>").unwrap());
        assert!(page.contains("script-src 'none'"));
        let bare = html_page("<p>no doctype</p>");
        assert!(bare.starts_with("<meta charset=\"utf-8\"><meta http-equiv"));
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
