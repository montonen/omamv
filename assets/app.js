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
  // A failed render leaves its scratch element at the end of the body.
  document.querySelectorAll("body > [id^='domomamv-mermaid'], body > [id^='omamv-mermaid'], body > [id^='domomamv']").forEach((n) => n.remove());
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

// ---- find ----

let marks = [];
let current = -1;

function clearFind() {
  for (const m of marks) {
    const parent = m.parentNode;
    if (!parent) continue;
    parent.replaceChild(document.createTextNode(m.textContent), m);
    parent.normalize();
  }
  marks = [];
  current = -1;
}

function find(query) {
  clearFind();
  const q = query.toLowerCase();
  if (!q) return JSON.stringify({ count: 0, at: 0 });
  const root = document.querySelector(".page") || document.body;
  const walker = document.createTreeWalker(root, NodeFilter.SHOW_TEXT, {
    acceptNode: (n) => (n.parentElement && n.parentElement.closest("svg, script, style, pre.mermaid") ? NodeFilter.FILTER_REJECT : NodeFilter.FILTER_ACCEPT),
  });
  const nodes = [];
  for (let n = walker.nextNode(); n; n = walker.nextNode()) nodes.push(n);
  for (const node of nodes) {
    const text = node.nodeValue;
    const lower = text.toLowerCase();
    let at = lower.indexOf(q);
    if (at < 0) continue;
    const frag = document.createDocumentFragment();
    let last = 0;
    while (at >= 0 && marks.length < 5000) {
      frag.appendChild(document.createTextNode(text.slice(last, at)));
      const m = document.createElement("mark");
      m.className = "om-find";
      m.textContent = text.slice(at, at + q.length);
      frag.appendChild(m);
      marks.push(m);
      last = at + q.length;
      at = lower.indexOf(q, last);
    }
    frag.appendChild(document.createTextNode(text.slice(last)));
    node.parentNode.replaceChild(frag, node);
  }
  if (marks.length) {
    // Start at the first match on screen or below it.
    const top = 0;
    current = marks.findIndex((m) => m.getBoundingClientRect().top >= top);
    if (current < 0) current = 0;
    show();
  }
  return JSON.stringify({ count: marks.length, at: marks.length ? current + 1 : 0 });
}

function show() {
  marks.forEach((m, i) => m.classList.toggle("cur", i === current));
  marks[current].scrollIntoView({ block: "center", behavior: "instant" });
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

window.omamv = { place, scroll: scrollCmd, find, step, endFind, gone };
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
