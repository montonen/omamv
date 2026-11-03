// omamv's page script: draws diagrams, puts the reader back where they were,
// and answers the window (scrolling, find, the removed-file state).
// The window calls `omamv.*`; the page tells the window things through the
// "omamv" message handler.
"use strict";
if (!window.__omamvLoaded) {

const css = getComputedStyle(document.documentElement);
const v = (name) => css.getPropertyValue(name).trim();
const tell = (msg) => {
  try { window.webkit.messageHandlers.omamv.postMessage(JSON.stringify(msg)); } catch (_) { /* not in omamv */ }
};

function loadMermaid() {
  return new Promise((ok, fail) => {
    const s = document.createElement("script");
    s.src = "omamv://assets/mermaid.js";
    s.onload = ok;
    s.onerror = () => fail(new Error("Mermaid did not load"));
    document.head.appendChild(s);
  });
}

async function drawMermaid(blocks) {
  await loadMermaid();
  const ui = '"Adwaita Sans", system-ui, sans-serif';
  mermaid.initialize({
    startOnLoad: false,
    securityLevel: "strict",
    theme: "base",
    fontFamily: ui,
    flowchart: { htmlLabels: true, curve: "basis" },
    themeVariables: {
      darkMode: v("--dark") === "1",
      fontFamily: ui,
      fontSize: "14px",
      background: v("--bg"),
      primaryColor: v("--bg-lighter"),
      primaryTextColor: v("--fg-bright"),
      primaryBorderColor: v("--muted"),
      secondaryColor: v("--bg-lighter"),
      secondaryTextColor: v("--fg"),
      secondaryBorderColor: v("--muted"),
      tertiaryColor: v("--bg"),
      tertiaryTextColor: v("--fg"),
      tertiaryBorderColor: v("--muted"),
      lineColor: v("--fg-70"),
      textColor: v("--fg"),
      mainBkg: v("--bg-lighter"),
      nodeBorder: v("--muted"),
      clusterBkg: v("--bg"),
      clusterBorder: v("--muted"),
      edgeLabelBackground: v("--bg"),
      titleColor: v("--fg-bright"),
      actorBkg: v("--bg-lighter"),
      actorBorder: v("--muted"),
      actorTextColor: v("--fg-bright"),
      actorLineColor: v("--muted"),
      signalColor: v("--fg-70"),
      signalTextColor: v("--fg"),
      labelBoxBkgColor: v("--bg-lighter"),
      labelBoxBorderColor: v("--muted"),
      labelTextColor: v("--fg"),
      loopTextColor: v("--fg"),
      noteBkgColor: v("--bg-lighter"),
      noteBorderColor: v("--muted"),
      noteTextColor: v("--fg"),
      activationBkgColor: v("--bg-lighter"),
      activationBorderColor: v("--muted"),
      sequenceNumberColor: v("--bg"),
      pie1: v("--accent"), pie2: v("--green"), pie3: v("--magenta"), pie4: v("--yellow"),
      pie5: v("--cyan"), pie6: v("--orange"), pie7: v("--red"), pie8: v("--blue"),
      git0: v("--accent"), git1: v("--green"), git2: v("--magenta"), git3: v("--yellow"),
    },
  });
  for (const [i, pre] of blocks.entries()) {
    try {
      const { svg } = await mermaid.render("omamv-mermaid-" + i, pre.textContent);
      const fig = document.createElement("figure");
      fig.className = "diagram mermaid-drawn";
      fig.innerHTML = svg;
      pre.replaceWith(fig);
    } catch (e) {
      pre.classList.remove("mermaid");
      pre.classList.add("diagram-failed");
      pre.dataset.error = String((e && e.message) || e).split("\n")[0];
    }
  }
  // A failed render leaves its scratch element ("d" + the id, and an "i"
  // frame) at the end of the body, with Mermaid's own error drawing in it.
  document.querySelectorAll("body > [id^='domamv-mermaid'], body > [id^='iomamv-mermaid'], body > [id^='omamv-mermaid']").forEach((n) => n.remove());
}

// ---- where the reader is ----

// The heading at or above the top of the window, and how far below it the
// top is: a re-render that moves things above keeps the reader at the same
// words, where a pixel position would not.
function place() {
  const y = window.scrollY;
  let best = null;
  for (const h of document.querySelectorAll("h1[id],h2[id],h3[id],h4[id],h5[id],h6[id]")) {
    const top = h.getBoundingClientRect().top + y;
    if (top <= y + 1) best = { id: h.id, off: y - top };
    else break;
  }
  return JSON.stringify(best ? { id: best.id, off: Math.round(best.off), y: Math.round(y) } : { id: "", off: 0, y: Math.round(y) });
}

function restore(q) {
  const id = q.get("a");
  const off = Number(q.get("o") || 0);
  const y = Number(q.get("y") || 0);
  const h = id ? document.getElementById(id) : null;
  if (h) window.scrollTo({ top: h.getBoundingClientRect().top + window.scrollY + off, behavior: "instant" });
  else if (y > 0) window.scrollTo({ top: y, behavior: "instant" });
  else if (location.hash) {
    const t = document.getElementById(decodeURIComponent(location.hash.slice(1)));
    if (t) t.scrollIntoView({ block: "start", behavior: "instant" });
  }
}

// ---- scrolling by key ----

const line = 48;
function scrollCmd(cmd) {
  const h = window.innerHeight;
  const to = {
    down: () => window.scrollBy({ top: line, behavior: "instant" }),
    up: () => window.scrollBy({ top: -line, behavior: "instant" }),
    "half-down": () => window.scrollBy({ top: h / 2, behavior: "instant" }),
    "half-up": () => window.scrollBy({ top: -h / 2, behavior: "instant" }),
    top: () => window.scrollTo({ top: 0, behavior: "instant" }),
    bottom: () => window.scrollTo({ top: document.documentElement.scrollHeight, behavior: "instant" }),
  }[cmd];
  if (to) to();
  watchScroll();
  return "ok";
}

let scrolled = false;
function watchScroll() {
  const now = window.scrollY > 0;
  if (now !== scrolled) {
    scrolled = now;
    tell({ scrolled: now });
  }
}
window.addEventListener("scroll", watchScroll, { passive: true });

// Where the reader is, once scrolling stops: omamv keeps it for the folder
// (ADR-0019), so a reopened folder starts at the same words.
let settle = 0;
window.addEventListener("scroll", () => {
  clearTimeout(settle);
  settle = setTimeout(() => tell({ place: JSON.parse(place()) }), 500);
}, { passive: true });

// ---- find ----

// One entry per match: the <mark>s it is drawn in (several when it runs
// across bold, a link or highlighted code).
let marks = [];
let current = -1;

// Text nodes grouped by the block they read in: a match may cross the inline
// pieces of one paragraph, line of a table or code block, never two blocks.
const BLOCK = "p, li, td, th, h1, h2, h3, h4, h5, h6, dt, dd, pre, blockquote, figcaption, summary, div, section, article, body";

function clearFind() {
  const parents = new Set();
  for (const group of marks) {
    for (const m of group) {
      const parent = m.parentNode;
      if (!parent) continue;
      parent.replaceChild(document.createTextNode(m.textContent), m);
      parents.add(parent);
    }
  }
  for (const p of parents) p.normalize();
  marks = [];
  current = -1;
}

// Lowercased one character for one, white space runs as one space; `map`
// says where each kept character was. The search in all files folds the same.
function fold(text) {
  let out = "";
  const map = [];
  for (let i = 0; i < text.length; i++) {
    const c = text[i];
    if (/\s/.test(c)) {
      if (out.length && out[out.length - 1] !== " ") {
        out += " ";
        map.push(i);
      }
      continue;
    }
    const l = c.toLowerCase();
    out += l.length === 1 ? l : c;
    map.push(i);
  }
  return { text: out, map };
}

// Every block's text and its matches: [{block, text, hits: [start in text]}].
function search(query) {
  const q = fold(query.trim()).text.trim();
  if (!q) return { q, blocks: [] };
  const root = document.querySelector(".page") || document.body;
  const walker = document.createTreeWalker(root, NodeFilter.SHOW_TEXT, {
    acceptNode: (n) => (n.parentElement && n.parentElement.closest("svg, script, style, pre.mermaid") ? NodeFilter.FILTER_REJECT : NodeFilter.FILTER_ACCEPT),
  });
  const blocks = [];
  let last = null;
  for (let n = walker.nextNode(); n; n = walker.nextNode()) {
    const block = n.parentElement.closest(BLOCK) || root;
    if (!last || last.block !== block) {
      last = { block, nodes: [], raw: "" };
      blocks.push(last);
    }
    last.nodes.push({ node: n, from: last.raw.length });
    last.raw += n.nodeValue;
  }
  const out = [];
  for (const b of blocks) {
    const f = fold(b.raw);
    const hits = [];
    let at = f.text.indexOf(q);
    while (at >= 0) {
      hits.push(at);
      at = f.text.indexOf(q, at + q.length);
    }
    out.push({ ...b, folded: f, hits });
  }
  return { q, blocks: out };
}

function find(query) {
  clearFind();
  const { q, blocks } = search(query);
  for (const b of blocks) {
    if (!b.hits.length) continue;
    // Raw ranges of the matches, cut at the node they fall in, last first so
    // earlier offsets stay true.
    const ranges = b.hits.slice(0, Math.max(0, 5000 - marks.length)).map((s) => [b.folded.map[s], b.folded.map[s + q.length - 1] + 1]);
    const groups = ranges.map(() => []);
    for (let k = b.nodes.length - 1; k >= 0; k--) {
      const { node, from } = b.nodes[k];
      const to = from + node.nodeValue.length;
      const pieces = [];
      ranges.forEach(([s, e], i) => {
        if (s < to && e > from) pieces.push([Math.max(s, from) - from, Math.min(e, to) - from, i]);
      });
      if (!pieces.length) continue;
      const text = node.nodeValue;
      const frag = document.createDocumentFragment();
      let lastAt = 0;
      for (const [s, e, i] of pieces) {
        frag.appendChild(document.createTextNode(text.slice(lastAt, s)));
        const m = document.createElement("mark");
        m.className = "om-find";
        m.textContent = text.slice(s, e);
        frag.appendChild(m);
        groups[i].unshift(m);
        lastAt = e;
      }
      frag.appendChild(document.createTextNode(text.slice(lastAt)));
      node.parentNode.replaceChild(frag, node);
    }
    for (const g of groups) if (g.length) marks.push(g);
  }
  if (marks.length) {
    // Start at the first match on screen or below it.
    current = marks.findIndex((g) => g[0].getBoundingClientRect().top >= 0);
    if (current < 0) current = 0;
    show();
  }
  return JSON.stringify({ count: marks.length, at: marks.length ? current + 1 : 0 });
}

// Find, then go to one match from the search in all files (ADR-0025): the
// `nth` in the block that holds `line`, or else the `nthInFile` of the page.
function findAt(query, line, nth, nthInFile) {
  clearFind();
  const want = fold(line).text;
  const { q, blocks } = search(query);
  let target = -1;
  let seen = 0;
  for (const b of blocks) {
    const at = want ? b.folded.text.indexOf(want) : -1;
    if (target < 0 && at >= 0) {
      const inLine = b.hits.filter((s) => s >= at && s < at + want.length);
      if (nth < inLine.length) target = seen + b.hits.indexOf(inLine[nth]);
    }
    seen += b.hits.length;
  }
  if (!q) return JSON.stringify({ count: 0, at: 0 });
  const r = JSON.parse(find(query));
  if (!r.count) return JSON.stringify(r);
  if (target < 0) target = Math.min(nthInFile, marks.length - 1);
  current = Math.min(target, marks.length - 1);
  show();
  return JSON.stringify({ count: marks.length, at: current + 1 });
}

function show() {
  marks.forEach((g, i) => g.forEach((m) => m.classList.toggle("cur", i === current)));
  marks[current][0].scrollIntoView({ block: "center", behavior: "instant" });
}

function step(dir) {
  if (!marks.length) return JSON.stringify({ count: 0, at: 0 });
  current = (current + dir + marks.length) % marks.length;
  show();
  return JSON.stringify({ count: marks.length, at: current + 1 });
}

// Close the bar but keep the place: the marks go, the scroll stays.
function endFind() {
  clearFind();
  return "ok";
}

function gone(on) {
  document.body.classList.toggle("gone", !!on);
  return "ok";
}

// ---- the gallery (ADR-0024) ----

// Each card's page is drawn at the page's own width and scaled down to the
// card's: the height follows from the width.
function fitGallery() {
  for (const page of document.querySelectorAll(".om-card-page")) {
    const s = page.parentElement.clientWidth / page.offsetWidth;
    page.style.transform = `scale(${s})`;
  }
}
function select(i) {
  document.querySelectorAll(".om-card").forEach((c) => c.classList.toggle("om-sel", Number(c.dataset.i) === i));
}
if (document.querySelector(".om-gallery")) {
  fitGallery();
  window.addEventListener("resize", fitGallery);
}

window.omamv = { place, scroll: scrollCmd, find, findAt, step, endFind, gone, select };
window.__omamvLoaded = true;

(async () => {
  const t0 = performance.now();
  try {
    const blocks = [...document.querySelectorAll("pre.mermaid")];
    if (blocks.length) await drawMermaid(blocks);
  } catch (e) {
    for (const pre of document.querySelectorAll("pre.mermaid")) {
      pre.classList.remove("mermaid");
      pre.classList.add("diagram-failed");
      pre.dataset.error = String((e && e.message) || e);
    }
  }
  restore(new URLSearchParams(location.search));
  watchScroll();
  document.documentElement.dataset.ready = "1";
  tell({ ready: true, ms: Math.round(performance.now() - t0), scrolled: window.scrollY > 0 });
})();
}
