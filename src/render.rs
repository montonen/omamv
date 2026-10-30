//! A file to the page omamv shows (ADR-0006, ADR-0007, ADR-0010).
//!
//! Markdown goes through pulldown-cmark with GitHub's extensions. Whatever
//! HTML the file brings is cleaned by ammonia, so a script in a document never
//! runs. omamv's own pieces (highlighted code, diagrams, alert headings, task
//! boxes) are put in after the cleaning, through tokens the document cannot
//! guess.

use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::os::unix::fs::DirBuilderExt;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use pulldown_cmark::{BlockQuoteKind, CodeBlockKind, CowStr, Event, HeadingLevel, LinkType, Options, Parser, Tag, TagEnd};

use crate::wiki::{self, Links};

use crate::theme::{self, Palette};

/// The page for a file's text.
pub fn page(path: &Path, text: &str, palette: &Palette, links: Option<&Links>) -> String {
    if crate::folder::is_html(path) {
        return html_page(text);
    }
    let body = if crate::folder::is_markdown(path) {
        markdown_in(text, palette, links)
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
    let lower = text[..text.floor_char_boundary(512)].to_ascii_lowercase();
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

/// GitHub's extensions, and front matter as its own block.
const OPTS: Options = Options::ENABLE_TABLES
    .union(Options::ENABLE_STRIKETHROUGH)
    .union(Options::ENABLE_TASKLISTS)
    .union(Options::ENABLE_FOOTNOTES)
    .union(Options::ENABLE_GFM)
    .union(Options::ENABLE_YAML_STYLE_METADATA_BLOCKS)
    .union(Options::ENABLE_WIKILINKS);

/// Wikilinks made into ordinary links and pictures (ADR-0018). One that
/// points at nothing in the folder is a dimmed span, not a link.
fn wikilinks<'a>(events: impl Iterator<Item = Event<'a>>, links: Option<&Links>, trusted: &mut Trusted) -> Vec<Event<'a>> {
    /// Inside a wikilink: how it closes, and the label to show in place of
    /// the target written (`None` once shown, or when `|text` is given).
    struct Wiki {
        close: Option<TagEnd>,
        relabel: bool,
        label: Option<String>,
    }
    let missing = |trusted: &mut Trusted, file: &str| {
        Event::InlineHtml(trusted.token(format!(r#"<span class="om-wiki-missing" title="{} is not in this folder">"#, esc(file))).into())
    };
    let link = |href: String| Event::Start(Tag::Link { link_type: LinkType::Inline, dest_url: CowStr::from(href), title: "".into(), id: "".into() });
    let mut out = Vec::new();
    let mut inside: Option<Wiki> = None;
    for ev in events {
        match ev {
            Event::Start(Tag::Link { link_type: LinkType::WikiLink { has_pothole }, dest_url, .. }) => {
                let (file, heading) = wiki::split(&dest_url);
                let found = if file.is_empty() { Some(None) } else { links.and_then(|l| wiki::resolve(file, l)).map(Some) };
                let close = match found {
                    Some(f) => {
                        out.push(link(wiki::href(f.as_deref(), heading)));
                        Some(TagEnd::Link)
                    }
                    None => {
                        out.push(missing(trusted, file));
                        None
                    }
                };
                inside = Some(Wiki { close, relabel: !has_pothole, label: Some(wiki::label(&dest_url)) });
            }
            Event::Start(Tag::Image { link_type: LinkType::WikiLink { .. }, dest_url, .. }) => {
                let target = dest_url.split('|').next().unwrap_or("").to_owned();
                let (file, _) = wiki::split(&target);
                let path = Path::new(file);
                // `![[Note]]` puts a note inside another in Obsidian; here it is a link to it.
                let note = crate::folder::is_shown(path) || path.extension().is_none();
                let found = links.and_then(|l| if note { wiki::resolve(file, l) } else { wiki::picture(file, l) });
                let close = match found {
                    Some(f) if note => {
                        out.push(link(wiki::href(Some(&f), None)));
                        Some(TagEnd::Link)
                    }
                    Some(f) => {
                        let href = CowStr::from(wiki::href(Some(&f), None));
                        out.push(Event::Start(Tag::Image { link_type: LinkType::Inline, dest_url: href, title: "".into(), id: "".into() }));
                        Some(TagEnd::Image)
                    }
                    None => {
                        out.push(missing(trusted, file));
                        None
                    }
                };
                let label = if note { wiki::label(&target) } else { file.to_owned() };
                inside = Some(Wiki { close, relabel: true, label: Some(label) });
            }
            Event::Text(t) => match inside.as_mut() {
                Some(w) if w.relabel => {
                    if let Some(l) = w.label.take() {
                        out.push(Event::Text(l.into()));
                    }
                }
                _ => out.push(Event::Text(t)),
            },
            Event::End(end @ (TagEnd::Link | TagEnd::Image)) => match inside.take() {
                Some(Wiki { close: Some(c), .. }) => out.push(Event::End(c)),
                Some(Wiki { close: None, .. }) => out.push(Event::InlineHtml(trusted.token("</span>".into()).into())),
                None => out.push(Event::End(end)),
            },
            e => out.push(e),
        }
    }
    out
}

/// A heading's text as its id is made from it.
fn heading_text(events: &[Event]) -> String {
    events
        .iter()
        .filter_map(|e| match e {
            Event::Text(t) | Event::Code(t) => Some(t.to_string()),
            _ => None,
        })
        .collect()
}

/// The headings of a Markdown file: (level 1–6, text, id), with the ids the
/// page gives them. For the outline (ADR-0017).
pub fn headings(src: &str) -> Vec<(u8, String, String)> {
    let mut out = Vec::new();
    let mut ids = std::collections::HashSet::new();
    let mut inner: Option<(u8, Vec<Event>)> = None;
    for ev in Parser::new_ext(src, OPTS) {
        match ev {
            Event::Start(Tag::Heading { level, .. }) => inner = Some((level as u8, Vec::new())),
            Event::End(TagEnd::Heading(_)) => {
                if let Some((level, events)) = inner.take() {
                    let text = heading_text(&events);
                    let id = unique_id(&slug(&text), &mut ids);
                    out.push((level, text.trim().to_owned(), id));
                }
            }
            e => {
                if let Some((_, events)) = inner.as_mut() {
                    events.push(e);
                }
            }
        }
    }
    out
}

/// Front matter (`---` YAML at the top, as agents and static sites write
/// it) as a quiet key/value block. Top-level `key: value` lines only; list
/// items and indented lines join the key above. Not a YAML parser.
fn front_matter(yaml: &str) -> String {
    let mut rows: Vec<(String, String)> = Vec::new();
    for line in yaml.lines() {
        if line.trim().is_empty() || line.trim_start().starts_with('#') {
            continue;
        }
        let top = !line.starts_with([' ', '\t', '-']);
        if top && let Some((k, v)) = line.split_once(':') {
            rows.push((k.trim().to_owned(), unquote(v.trim())));
            continue;
        }
        let part = unquote(line.trim().trim_start_matches('-').trim());
        match rows.last_mut() {
            Some((_, v)) if v.is_empty() => *v = part,
            Some((_, v)) => {
                v.push_str(", ");
                v.push_str(&part);
            }
            None => rows.push((String::new(), part)),
        }
    }
    if rows.is_empty() {
        return String::new();
    }
    let mut html = String::from(r#"<dl class="om-meta">"#);
    for (k, v) in rows {
        html.push_str(&format!("<dt>{}</dt><dd>{}</dd>", esc(&k), esc(&v)));
    }
    html.push_str("</dl>");
    html
}

fn unquote(s: &str) -> String {
    let s = s.trim();
    for q in ['"', '\''] {
        if s.len() >= 2 && s.starts_with(q) && s.ends_with(q) {
            return s[1..s.len() - 1].to_owned();
        }
    }
    s.to_owned()
}

#[cfg(test)]
pub fn markdown(src: &str, palette: &Palette) -> String {
    markdown_in(src, palette, None)
}

/// `links` resolves wikilinks to other files; without it only `[[#Heading]]` works.
pub fn markdown_in(src: &str, palette: &Palette, links: Option<&Links>) -> String {
    markdown_with(src, palette, links, None)
}

/// Lines of a file a gallery card draws (ADR-0024): its top, and no more.
const PREVIEW_LINES: usize = 150;

/// The top of a file for a gallery card (ADR-0024): drawn as its page is,
/// but diagrams wait for the page itself (no PlantUML runs for a card), and
/// pictures are found from the folder's root, where the gallery stands.
pub fn preview(path: &Path, text: &str, palette: &Palette, links: &Links) -> String {
    let top: String = text.lines().take(PREVIEW_LINES).collect::<Vec<_>>().join("\n");
    if crate::folder::is_markdown(path) {
        markdown_with(&top, palette, Some(links), Some(links.rel.parent().unwrap_or(Path::new(""))))
    } else {
        format!(r#"<pre class="plain">{}</pre>"#, esc(&top))
    }
}

/// `preview`: the folder (from the root) of a file drawn for a gallery card.
fn markdown_with(src: &str, palette: &Palette, links: Option<&Links>, preview: Option<&Path>) -> String {
    let opts = OPTS;
    let mut trusted = Trusted::new(src);
    let events = wikilinks(Parser::new_ext(src, opts), links, &mut trusted);
    let mut out: Vec<Event> = Vec::new();
    let mut code: Option<(String, String)> = None;
    let mut heading: Option<(HeadingLevel, Vec<Event>)> = None;
    let mut quotes: Vec<Option<BlockQuoteKind>> = Vec::new();
    let mut ids = std::collections::HashSet::new();
    let mut diagrams: Vec<(usize, String)> = Vec::new();
    let mut meta: Option<String> = None;

    for ev in events {
        if let Some(body) = meta.as_mut() {
            match ev {
                Event::Text(t) => body.push_str(&t),
                Event::End(TagEnd::MetadataBlock(_)) => {
                    let html = front_matter(&meta.take().unwrap_or_default());
                    out.push(Event::Html(format!("{}\n", trusted.token(html)).into()));
                }
                _ => {}
            }
            continue;
        }
        if let Some((_, body)) = code.as_mut() {
            match ev {
                Event::Text(t) => body.push_str(&t),
                Event::End(TagEnd::CodeBlock) => {
                    let (lang, body) = code.take().unwrap_or_default();
                    if preview.is_some() && matches!(lang.as_str(), "plantuml" | "puml" | "uml" | "mermaid") {
                        let html = r#"<figure class="diagram om-later">diagram</figure>"#.to_owned();
                        out.push(Event::Html(format!("{}\n", trusted.token(html)).into()));
                    } else if matches!(lang.as_str(), "plantuml" | "puml" | "uml") {
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
                let text = heading_text(inner);
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
            Event::Start(Tag::MetadataBlock(_)) => meta = Some(String::new()),
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
            Event::Start(Tag::Image { link_type, dest_url, title, id }) if preview.is_some() && local(&dest_url) => {
                let dir = wiki::href(preview, None);
                let dest_url = CowStr::from(format!("{dir}/{dest_url}"));
                out.push(Event::Start(Tag::Image { link_type, dest_url, title, id }));
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
    trusted.put_back(clean(&html))
}

/// A picture's address relative to its file, not absolute, a scheme or data.
fn local(dest: &str) -> bool {
    !dest.is_empty() && !dest.starts_with('/') && !dest.starts_with('#') && !dest.contains(':')
}

/// Raw HTML in a document keeps its structure and loses anything that runs.
/// An attribute that carries anything like one of omamv's tokens is dropped,
/// so a piece is only ever put back as content.
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

/// A folder of omamv's own with nothing in it, for PlantUML to run from: a
/// relative `!include` finds nothing there. Made once, readable by no one else.
fn empty_dir() -> PathBuf {
    static DIR: OnceLock<PathBuf> = OnceLock::new();
    DIR.get_or_init(|| {
        let dir = std::env::temp_dir().join(format!("omamv-plantuml-{}", std::process::id()));
        let _ = std::fs::DirBuilder::new().mode(0o700).create(&dir);
        dir
    })
    .clone()
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
        .current_dir(empty_dir())
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
    use std::path::PathBuf;

    fn md(s: &str) -> String {
        markdown(s, &crate::theme::fallback(true))
    }

    #[test]
    fn wikilinks_become_links_and_dead_ones_say_so() {
        let links = Links {
            root: PathBuf::from("/nowhere"),
            rel: PathBuf::from("Project.md"),
            files: std::sync::Arc::new(vec![PathBuf::from("Ideas.md"), PathBuf::from("Research/2026-10-28-01-a b.md")]),
        };
        let html = markdown_in(
            "# Contents\n\n[[#Weather flavour]] [[Ideas#Top-down lake view]] [[Research/2026-10-28-01-a b|the note]] [[Gone]] [[Ideas]]\n\n## Weather flavour\n",
            &crate::theme::fallback(true),
            Some(&links),
        );
        assert!(html.contains(r##"<a href="#weather-flavour" rel="noreferrer">Weather flavour</a>"##), "{html}");
        assert!(html.contains(r##"<a href="/Ideas.md#top-down-lake-view" rel="noreferrer">Ideas › Top-down lake view</a>"##), "{html}");
        assert!(html.contains(r#"<a href="/Research/2026-10-28-01-a%20b.md" rel="noreferrer">the note</a>"#), "{html}");
        assert!(html.contains(r#"<span class="om-wiki-missing" title="Gone is not in this folder">Gone</span>"#), "{html}");
        assert!(html.contains(r#"<a href="/Ideas.md" rel="noreferrer">Ideas</a>"#), "{html}");
        assert!(!html.contains("[["), "{html}");
        // The heading a [[#link]] names has the id it points at.
        assert!(html.contains(r#"id="weather-flavour""#), "{html}");
    }

    #[test]
    fn a_wikilink_cannot_smuggle_markup() {
        let html = md("[[\"><script>x</script>]] [[a|<b onclick=x>t</b>]]");
        assert!(!html.contains("<script") && !html.contains("onclick"), "{html}");
    }

    #[test]
    fn front_matter_is_a_quiet_block_not_a_heading() {
        let html = md("---\ntitle: \"Scan report\"\nrepo: omamv\ntags:\n  - nightly\n  - agents\nnote: <script>x</script>\n---\n\n# Findings\n");
        assert!(html.contains(r#"<dl class="om-meta">"#), "{html}");
        assert!(html.contains("<dt>title</dt><dd>Scan report</dd>"), "{html}");
        assert!(html.contains("<dt>tags</dt><dd>nightly, agents</dd>"), "{html}");
        assert!(html.contains("&lt;script&gt;"), "{html}");
        assert!(!html.contains("<hr"), "{html}");
        assert_eq!(html.matches("<h").count() - html.matches("<hr").count(), 1, "{html}");
    }

    #[test]
    fn outline_ids_match_the_page() {
        let src = "---\na: b\n---\n# Intro\n## Same\n## Same\ntext\n### `code` *it*\n";
        let h = headings(src);
        let ids: Vec<&str> = h.iter().map(|x| x.2.as_str()).collect();
        assert_eq!(ids, ["intro", "same", "same-1", "code-it"]);
        assert_eq!(h[3].0, 3);
        let page = md(src);
        for id in ids {
            assert!(page.contains(&format!("id=\"{id}\"")), "{id} in {page}");
        }
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
        let page = page(Path::new("notes.txt"), "a <b>\n  b", &crate::theme::fallback(true), None);
        assert!(page.contains(r#"<pre class="plain">a &lt;b&gt;"#));
    }

    #[test]
    fn table_alignment_survives_the_cleaning() {
        let html = md("| a | b |\n|:-:|--:|\n| 1 | 2 |");
        assert!(html.contains("text-align"), "{html}");
    }
}
