const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;

const $ = (s, el = document) => el.querySelector(s);
const esc = (s) => String(s ?? "").replace(/[&<>"']/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[c]);
const icon = (name, cls = "ico") => `<svg class="${cls}"><use href="#i-${name}"/></svg>`;

const PAGE = 60;
const APP_NAME = "Rusting Medal";
const pref = (k, d) => localStorage.getItem(`rm.${k}`) ?? d;
const setPref = (k, v) => localStorage.setItem(`rm.${k}`, v);
const S = {
  info: null,
  profile: null,         // { userId, username } when viewing someone else's profile
  clips: [],
  byId: new Map(),
  selected: new Set(),
  anchor: null,          // last clicked index for shift-select
  query: "",
  game: "",
  sort: pref("sort", "newest"),  // newest | oldest | longest | shortest
  view: pref("view", "all"),     // all | new
  offset: 0,
  loading: false,
  done: false,
  error: null,
  dls: new Map(),        // id -> {title, thumb, state, done, total, path, error}
  batch: { done: 0, failed: 0 }, // counts for the current run of downloads
};

/* ---------------- formatting ---------------- */
const fmtDur = (s) => {
  if (!s) return "";
  s = Math.round(s);
  return `${Math.floor(s / 60)}:${String(s % 60).padStart(2, "0")}`;
};
const fmtDate = (ms) => {
  if (!ms) return "";
  const d = new Date(ms), now = new Date();
  const opts = d.getFullYear() === now.getFullYear() ? { month: "short", day: "numeric" } : { month: "short", day: "numeric", year: "numeric" };
  return d.toLocaleDateString(undefined, opts);
};
const fmtMB = (b) => (b / 1048576).toFixed(b > 104857600 ? 0 : 1) + " MB";
const folderName = (p) => p.split(/[\\/]/).filter(Boolean).pop() || p;

/* ---------------- toasts ---------------- */
function toast(msg, kind = "") {
  const el = document.createElement("div");
  el.className = `toast ${kind}`;
  el.innerHTML = (kind === "err" ? icon("alert") : icon("check")) + `<span>${esc(msg)}</span>`;
  $("#toasts").append(el);
  setTimeout(() => { el.classList.add("leaving"); setTimeout(() => el.remove(), 220); }, kind === "err" ? 5000 : 2600);
}

/* ---------------- header ---------------- */
function renderHeader() {
  const i = S.info;
  $("#folder-name").textContent = folderName(i.outDir);
  $("#folder-btn").title = `Saving to ${i.outDir}\nClick to change`;
  $("#watch-toggle").checked = i.clipboardWatch;
  const acc = $("#account");
  if (!i.loggedIn) {
    acc.innerHTML = `<button class="btn btn-quiet" id="hdr-login">Log in</button>`;
    $("#hdr-login").onclick = () => startLogin($("#hdr-login"));
  } else {
    const name = i.name || `User ${i.userId}`;
    const av = i.avatar
      ? `<img class="avatar" src="${esc(i.avatar)}" alt="">`
      : `<span class="avatar">${esc(name[0].toUpperCase())}</span>`;
    acc.innerHTML = `<div class="account">${av}<span class="account-name">${esc(name)}</span>
      <button class="icon-btn" id="hdr-logout" title="Log out">${icon("logout")}</button></div>`;
    $("#hdr-logout").onclick = async () => { await invoke("logout"); };
  }
}

/* ---------------- main views ---------------- */
function renderLoggedOut() {
  const sample = (title, game, dur, selected, saved) => `
    <div class="card${selected ? " selected" : ""}">
      <div class="thumb">
        <span class="dur num">${dur}</span>
        <span class="tick">${icon("check")}</span>
      </div>
      <div class="meta">
        <div class="title">${title}</div>
        <div class="sub">${saved ? `<span class="saved">${icon("check")}Saved</span><span class="dot"></span>` : ""}<span>${game}</span></div>
      </div>
    </div>`;
  $("#main").innerHTML = `
    <section class="hero">
      <div class="product-preview" aria-hidden="true">
        ${sample("1v4 clutch on Ascent", "Valorant", "0:24", true, false)}
        ${sample("Double redirect OT goal", "Rocket League", "0:16", true, false)}
        ${sample("clean flick from mid", "CS2", "0:19", false, true)}
      </div>
      <h1>Pick clips from your Medal library and download them at once</h1>
      <p>Log in once to browse your clips, tick the ones you want for your montage, and save clean MP4s in parallel.</p>
      <div class="actions">
        <button class="btn btn-accent btn-lg" id="hero-login">Log in with Medal</button>
        <button class="btn-link" id="hero-token">Log in with a session token instead</button>
      </div>
      <p class="fine">Or paste or drop any clip link, .txt file, or profile URL above without logging in.</p>
    </section>`;
  $("#hero-login").onclick = () => startLogin($("#hero-login"));
  $("#hero-token").onclick = openTokenDialog;
}

function renderLibraryShell() {
  const heading = S.profile ? `@${esc(S.profile.username)}` : "Your clips";
  const backBtn = S.profile
    ? `<button class="btn btn-quiet btn-sm" id="back-mine">${S.info?.loggedIn ? "Back to my clips" : "Close"}</button>`
    : "";
  $("#main").innerHTML = `
    <div class="toolbar">
      <h1>${heading}</h1>
      ${backBtn}
      <span class="count num" id="clip-count"></span>
      <div class="seg" role="group">
        <button data-view="all" aria-pressed="${S.view === "all"}">All</button>
        <button data-view="new" aria-pressed="${S.view === "new"}">Not downloaded</button>
      </div>
      <select class="select" id="game-filter" hidden aria-label="Filter by game"></select>
      <select class="select" id="sort-select" aria-label="Sort clips">
        <option value="newest"${S.sort === "newest" ? " selected" : ""}>Newest</option>
        <option value="oldest"${S.sort === "oldest" ? " selected" : ""}>Oldest</option>
        <option value="longest"${S.sort === "longest" ? " selected" : ""}>Longest</option>
        <option value="shortest"${S.sort === "shortest" ? " selected" : ""}>Shortest</option>
      </select>
      <label class="search">${icon("search")}<input id="search" placeholder="Search titles or games" value="${esc(S.query)}"></label>
      <button class="btn btn-quiet" id="select-all">Select all</button>
      <button class="icon-btn" id="reload" title="Refresh">${icon("refresh")}</button>
    </div>
    <div class="grid" id="grid"></div>
    <div class="more" id="more"></div>`;

  if ($("#back-mine")) {
    $("#back-mine").onclick = () => {
      S.profile = null; S.game = ""; S.selected.clear();
      if (S.info?.loggedIn) { renderLibraryShell(); loadLibrary(true); }
      else renderLoggedOut();
    };
  }
  let searchTimer = null;
  $("#search").oninput = (e) => {
    clearTimeout(searchTimer);
    const v = e.target.value.trim().toLowerCase();
    searchTimer = setTimeout(() => { S.query = v; renderGrid(); }, 120);
  };
  $("#game-filter").onchange = (e) => { S.game = e.target.value; renderGrid(); };
  $("#sort-select").onchange = (e) => { S.sort = e.target.value; setPref("sort", S.sort); renderGrid(); };
  document.querySelectorAll(".seg button").forEach((b) => (b.onclick = () => {
    S.view = b.dataset.view;
    setPref("view", S.view);
    document.querySelectorAll(".seg button").forEach((x) => x.setAttribute("aria-pressed", x === b));
    renderGrid();
  }));
  $("#select-all").onclick = toggleSelectAll;
  $("#reload").onclick = () => S.profile ? openProfile(S.profile.username) : loadLibrary(true);
  $("#grid").addEventListener("click", onCardClick);
  $("#grid").addEventListener("dblclick", onCardDbl);
  $("#grid").addEventListener("contextmenu", onCardContext);
  $("#grid").addEventListener("mouseover", onHover);
  $("#grid").addEventListener("mouseout", onUnhover);
  $("#main").onscroll = maybeLoadMore;
}

function updateGameFilter() {
  const sel = $("#game-filter");
  if (!sel) return;
  const counts = new Map();
  for (const c of S.clips) {
    if (c.game) counts.set(c.game, (counts.get(c.game) || 0) + 1);
  }
  if (counts.size <= 1) { sel.hidden = true; return; }
  const games = [...counts.entries()].sort((a, b) => b[1] - a[1]);
  sel.innerHTML = `<option value="">All games</option>` + games.map(([g, n]) =>
    `<option value="${esc(g)}"${S.game === g ? " selected" : ""}>${esc(g)} (${n})</option>`
  ).join("");
  sel.hidden = false;
}

function visibleClips() {
  const list = S.clips.filter((c) => {
    if (S.view === "new" && c.saved) return false;
    if (S.game && c.game !== S.game) return false;
    if (!S.query) return true;
    return c.title.toLowerCase().includes(S.query) || (c.game || "").toLowerCase().includes(S.query);
  });
  if (S.sort === "oldest") list.sort((a, b) => a.created - b.created);
  else if (S.sort === "longest") list.sort((a, b) => b.duration - a.duration);
  else if (S.sort === "shortest") list.sort((a, b) => a.duration - b.duration);
  return list;
}

function cardHTML(c) {
  const dl = S.dls.get(c.id);
  const downloading = dl && (dl.state === "queued" || dl.state === "downloading");
  const pct = dl && dl.total ? (dl.done / dl.total) * 100 : 0;
  const cls = ["card", S.selected.has(c.id) && "selected", downloading && "downloading", dl?.state === "queued" && "waiting"].filter(Boolean).join(" ");
  const sub = [
    c.saved ? `<span class="saved">${icon("check")}Saved</span>` : "",
    c.game ? `<span>${esc(c.game)}</span>` : "",
    c.created ? `<span class="num">${fmtDate(c.created)}</span>` : "",
  ].filter(Boolean).join(`<span class="dot"></span>`);
  return `<div class="${cls}" data-id="${esc(c.id)}">
    <div class="thumb">
      ${c.thumb ? `<img loading="lazy" src="${esc(c.thumb)}" alt="">` : ""}
      ${c.duration ? `<span class="dur num">${fmtDur(c.duration)}</span>` : ""}
      <span class="tick">${icon("check")}</span>
      <div class="progress"><i style="width:${pct}%"></i></div>
      <div class="done-flash"><span>${icon("check")}</span></div>
    </div>
    <div class="meta">
      <div class="title" title="${esc(c.title)}">${esc(c.title)}</div>
      <div class="sub">${sub || "&nbsp;"}</div>
    </div>
  </div>`;
}

function skeletons(n) {
  return Array.from({ length: n }, () => `<div class="card skel"><div class="thumb"></div><div class="line"></div><div class="line short"></div></div>`).join("");
}

function renderGrid() {
  const grid = $("#grid");
  if (!grid) return;
  updateGameFilter();
  const list = visibleClips();
  const total = S.clips.length;
  $("#clip-count").textContent = total ? `${total}${S.done ? "" : "+"} clips` : "";
  $("#select-all").textContent = list.length && list.every((c) => S.selected.has(c.id)) ? "Deselect all" : "Select all";

  const more = $("#more");
  if (!S.loading && !total && !S.error) {
    grid.className = "state-msg";
    grid.innerHTML = `${icon("folder")}<h2>No clips on this account yet</h2><p>Clips you upload to Medal will show up here.</p>`;
    more.innerHTML = "";
    renderSelbar();
    return;
  }

  grid.className = S.selected.size > 0 ? "grid selecting" : "grid";
  grid.innerHTML = list.map(cardHTML).join("") + (S.loading ? skeletons(S.clips.length ? 4 : 12) : "");

  if (S.error) {
    more.innerHTML = `<span>${esc(S.error)}</span>`;
  } else if (!S.loading && list.length === 0) {
    more.innerHTML = `<span>No clips match “${esc(S.query)}”</span>`;
  } else if (S.done && total > PAGE) {
    more.innerHTML = `<span class="num">That's all ${total} clips</span>`;
  } else {
    more.innerHTML = "";
  }
  renderSelbar();
}

function patchCard(id) {
  const el = document.querySelector(`.card[data-id="${CSS.escape(id)}"]`);
  const c = S.byId.get(id);
  if (!el || !c) return;
  const wasPlaying = el.querySelector("video");
  const tmp = document.createElement("div");
  tmp.innerHTML = cardHTML(c);
  const fresh = tmp.firstElementChild;
  if (wasPlaying) fresh.querySelector(".thumb").append(wasPlaying);
  el.replaceWith(fresh);
  return fresh;
}

/* ---------------- loading ---------------- */
async function loadLibrary(reset = false) {
  if (reset) { S.clips = []; S.byId.clear(); S.offset = 0; S.done = false; S.error = null; }
  if (S.loading || S.done) return;
  S.loading = true;
  renderGrid();
  try {
    const page = await invoke("list_clips", {
      userId: S.profile?.userId || null,
      offset: S.offset,
      limit: PAGE,
    });
    for (const c of page) {
      if (S.byId.has(c.id)) continue;
      S.byId.set(c.id, c);
      S.clips.push(c);
    }
    S.offset += page.length;
    if (page.length < PAGE) S.done = true;
  } catch (e) {
    const msg = String(e);
    if (msg.includes("SESSION_EXPIRED")) {
      S.loading = false;
      $("#main").innerHTML = `<div class="state-msg">${icon("alert")}<h2>Your Medal session expired</h2>
        <p>Log in again to see your clips.</p><button class="btn btn-accent" id="relog">Log in again</button></div>`;
      $("#relog").onclick = () => startLogin($("#relog"));
      return;
    }
    S.error = `Couldn't load clips: ${msg}`;
  }
  S.loading = false;
  renderGrid();
  maybeLoadMore();
}

async function openProfile(username) {
  S.selected.clear(); S.game = ""; S.clips = []; S.byId.clear(); S.offset = 0; S.done = false; S.error = null;
  S.profile = { userId: null, username };
  renderLibraryShell();
  S.loading = true;
  renderGrid();
  try {
    const prof = await invoke("lookup_profile", { username });
    S.profile = { userId: prof.userId, username: prof.username };
    if (S.info?.loggedIn) {
      S.loading = false;
      loadLibrary(true);
      return;
    }
    for (const c of prof.initialClips) {
      S.byId.set(c.id, c);
      S.clips.push(c);
    }
    S.done = true;
  } catch (e) {
    S.error = String(e);
  }
  S.loading = false;
  renderGrid();
}

function maybeLoadMore() {
  const m = $("#main");
  if (!S.loading && !S.done && !S.error && m.scrollTop + m.clientHeight > m.scrollHeight - 600) loadLibrary();
}

/* ---------------- selection ---------------- */
function onCardClick(e) {
  const card = e.target.closest(".card[data-id]");
  if (!card) return;
  const list = visibleClips();
  const idx = list.findIndex((c) => c.id === card.dataset.id);
  if (e.shiftKey && S.anchor != null) {
    const [a, b] = [Math.min(S.anchor, idx), Math.max(S.anchor, idx)];
    for (let i = a; i <= b; i++) S.selected.add(list[i].id);
  } else {
    const id = card.dataset.id;
    S.selected.has(id) ? S.selected.delete(id) : S.selected.add(id);
    S.anchor = idx;
  }
  list.forEach((c) => {
    const el = document.querySelector(`.card[data-id="${CSS.escape(c.id)}"]`);
    el?.classList.toggle("selected", S.selected.has(c.id));
  });
  $("#grid").classList.toggle("selecting", S.selected.size > 0);
  renderSelbar();
}

function onCardDbl(e) {
  const card = e.target.closest(".card[data-id]");
  if (!card) return;
  const c = S.byId.get(card.dataset.id);
  S.selected.delete(c.id);
  card.classList.remove("selected");
  renderSelbar();
  startDownloads([c]);
}

function onCardContext(e) {
  const card = e.target.closest(".card[data-id]");
  if (!card) return;
  e.preventDefault();
  const id = card.dataset.id;
  const c = S.byId.get(id);
  const d = S.dls.get(id);
  if (d?.state === "done" && d.path) return invoke("reveal", { path: d.path });
  if (c?.saved) return invoke("reveal_clip", { id }).catch((err) => toast(String(err), "err"));
  toast("Not downloaded yet. Double-click to grab it");
}

function toggleSelectAll() {
  const list = visibleClips();
  const all = list.length && list.every((c) => S.selected.has(c.id));
  list.forEach((c) => (all ? S.selected.delete(c.id) : S.selected.add(c.id)));
  renderGrid();
}

function renderSelbar() {
  const bar = $("#selbar");
  const n = S.selected.size;
  bar.hidden = n === 0;
  if (!n) return;
  const secs = [...S.selected].reduce((t, id) => t + (S.byId.get(id)?.duration || 0), 0);
  $("#sel-text").innerHTML = `<b>${n}</b> selected${secs ? ` · ${fmtDur(secs)} of footage` : ""}`;
  $("#sel-btn-text").textContent = `Download ${n} clip${n === 1 ? "" : "s"}`;
}

/* ---------------- hover preview ---------------- */
let hoverTimer = null;
function onHover(e) {
  const card = e.target.closest(".card[data-id]");
  if (!card || card.contains(e.relatedTarget)) return;
  const c = S.byId.get(card.dataset.id);
  const src = c?.previewUrl || c?.url;
  if (!src) return;
  clearTimeout(hoverTimer);
  hoverTimer = setTimeout(() => {
    if (card.querySelector("video")) return;
    const v = document.createElement("video");
    v.muted = true; v.loop = true; v.playsInline = true; v.preload = "auto";
    v.src = src;
    v.oncanplay = () => { v.play().catch(() => {}); v.classList.add("playing"); };
    card.querySelector(".thumb").insertBefore(v, card.querySelector(".dur"));
  }, 450);
}
function onUnhover(e) {
  const card = e.target.closest(".card[data-id]");
  if (!card || card.contains(e.relatedTarget)) return;
  clearTimeout(hoverTimer);
  const v = card.querySelector("video");
  if (v) { v.pause(); v.removeAttribute("src"); v.load(); v.remove(); }
}

/* ---------------- downloads ---------------- */
async function startDownloads(clips) {
  if (!clips.length) return;
  const n = await invoke("download", { clips });
  if (n === 0) toast("Those are already downloading");
}

function upsertDl(id, patch) {
  const prev = S.dls.get(id) || { title: id, thumb: null, state: "queued", done: 0, total: 0 };
  const next = { ...prev, ...patch };
  S.dls.set(id, next);
  return next;
}

function updateQueueBadge(pulse = false) {
  const items = [...S.dls.values()];
  const activeItems = items.filter((d) => d.state === "queued" || d.state === "downloading");
  const active = activeItems.length;
  const failed = items.filter((d) => d.state === "error").length;
  const count = active || items.length;
  const badge = $("#queue-count");
  badge.hidden = count === 0;
  badge.textContent = count || "";
  const btn = $("#queue-btn");
  btn.classList.toggle("has-active", active > 0);
  btn.title = active ? `${active} downloading` : "Downloads";
  $("#retry-failed").hidden = failed === 0;
  $("#retry-failed").textContent = failed > 1 ? `Retry ${failed} failed` : "Retry failed";
  updateTitle(activeItems);
  if (pulse) {
    btn.classList.remove("pulse");
    void btn.offsetWidth;
    btn.classList.add("pulse");
  }
}

// Overall progress in the window title, so it's visible from the taskbar.
function updateTitle(activeItems) {
  if (!activeItems.length) { document.title = APP_NAME; return; }
  const total = activeItems.reduce((t, d) => t + (d.total || 0), 0);
  const done = activeItems.reduce((t, d) => t + (d.done || 0), 0);
  const pct = total ? ` · ${Math.floor((done / total) * 100)}%` : "";
  document.title = `${APP_NAME} — ${activeItems.length} downloading${pct}`;
}

// Called whenever a download settles; toasts once the whole run is finished.
function maybeFinishBatch() {
  const active = [...S.dls.values()].some((d) => d.state === "queued" || d.state === "downloading");
  if (active) return;
  const { done, failed } = S.batch;
  S.batch = { done: 0, failed: 0 };
  if (done + failed < 2) return;
  if (failed) toast(`Downloaded ${done} clip${done === 1 ? "" : "s"}, ${failed} failed`, "err");
  else toast(`Downloaded ${done} clips`);
}

function retryDownloads(ids) {
  const clips = ids.map((id) => {
    const c = S.byId.get(id);
    if (c) return c;
    const d = S.dls.get(id) || {};
    return { id, title: d.title || "Finding clip…", url: null, previewUrl: null, thumb: d.thumb || null, duration: 0, created: 0, game: null, saved: false };
  });
  startDownloads(clips);
}

function renderDrawer() {
  const list = $("#dl-list");
  const items = [...S.dls.entries()].reverse();
  updateQueueBadge();
  if (!items.length) {
    list.innerHTML = `<li class="dl-empty">Nothing downloaded yet.<br>Select clips or paste a link to start.</li>`;
    return;
  }
  list.innerHTML = items.map(([id, d]) => dlHTML(id, d)).join("");
}

function dlHTML(id, d) {
  const pct = d.total ? (d.done / d.total) * 100 : 0;
  let sub, state = "";
  switch (d.state) {
    case "queued": sub = "Waiting…"; break;
    case "downloading": sub = d.total ? `${Math.round(pct)}% · ${fmtMB(d.done)} of ${fmtMB(d.total)}` : fmtMB(d.done); break;
    case "done": sub = d.skipped ? "Already in your folder" : `${fmtMB(d.bytes)} · Show in folder`; state = icon("check"); break;
    case "error": sub = `${d.error} · Click to retry`; state = icon("refresh"); break;
  }
  const cls = ["dl", d.state, d.state === "queued" && "waiting"].filter(Boolean).join(" ");
  const bar = d.state === "queued" || d.state === "downloading" ? `<div class="dl-bar"><i style="width:${pct}%"></i></div>` : "";
  return `<li class="${cls}" data-id="${esc(id)}">
    <div class="dl-thumb" ${d.thumb ? `style="background-image:url('${esc(d.thumb)}')"` : ""}></div>
    <div class="dl-body"><div class="dl-title" title="${esc(d.title)}">${esc(d.title)}</div>
      <div class="dl-sub num ${d.state === "error" ? "err" : ""}">${esc(sub)}</div>${bar}</div>
    <div class="dl-state">${state}</div>
  </li>`;
}

function patchDl(id) {
  const el = document.querySelector(`.dl[data-id="${CSS.escape(id)}"]`);
  const d = S.dls.get(id);
  if (!el || !d) return renderDrawer();
  if (d.state === "downloading" && el.classList.contains("downloading")) {
    const pct = d.total ? (d.done / d.total) * 100 : 0;
    el.querySelector(".dl-bar i").style.width = `${pct}%`;
    el.querySelector(".dl-sub").textContent = d.total ? `${Math.round(pct)}% · ${fmtMB(d.done)} of ${fmtMB(d.total)}` : fmtMB(d.done);
    return;
  }
  el.outerHTML = dlHTML(id, d);
}

function openDrawer() { $("#drawer").hidden = false; }

/* ---------------- login ---------------- */
async function startLogin(btn) {
  const prev = btn?.textContent;
  if (btn) { btn.disabled = true; btn.textContent = "Opening Medal…"; }
  try {
    await invoke("open_login");
  } catch (e) {
    toast(String(e), "err");
  } finally {
    if (btn) { btn.disabled = false; btn.textContent = prev; }
  }
}
function openTokenDialog() {
  $("#token-error").hidden = true;
  $("#token-input").value = "";
  $("#token-dialog").showModal();
}

/* ---------------- boot ---------------- */
async function refreshInfo() {
  S.info = await invoke("app_info");
  renderHeader();
}

async function boot() {
  await refreshInfo();
  renderDrawer();
  if (S.info.loggedIn) { renderLibraryShell(); loadLibrary(true); }
  else renderLoggedOut();
}

/* events from Rust */
listen("auth-changed", async () => {
  S.selected.clear();
  await boot();
  if (S.info.loggedIn) toast("Logged in. Loading your clips");
});
listen("auth-error", (e) => toast(e.payload, "err"));
listen("clipboard-grabbed", (e) => toast(`Grabbed ${e.payload} link${e.payload === 1 ? "" : "s"} from your clipboard`));

listen("dl-queued", ({ payload: p }) => {
  const known = S.byId.get(p.id);
  const fresh = !S.dls.has(p.id) || S.dls.get(p.id).state === "done" || S.dls.get(p.id).state === "error";
  upsertDl(p.id, { title: known?.title || p.title, thumb: known?.thumb || p.thumb, ...(fresh ? { state: "queued", done: 0, total: 0, error: null } : {}) });
  if (fresh) { S.dls.delete(p.id); S.dls.set(p.id, upsertDl(p.id, {})); renderDrawer(); updateQueueBadge(true); openDrawer(); }
  else patchDl(p.id);
  patchCard(p.id);
});
listen("dl-progress", ({ payload: p }) => {
  const d = S.dls.get(p.id);
  const first = d?.state !== "downloading";
  upsertDl(p.id, { state: "downloading", done: p.done, total: p.total });
  updateTitle([...S.dls.values()].filter((d) => d.state === "queued" || d.state === "downloading"));
  if (first) { patchDl(p.id); patchCard(p.id); return; }
  patchDl(p.id);
  const bar = document.querySelector(`.card[data-id="${CSS.escape(p.id)}"] .progress i`);
  if (bar) bar.style.width = `${p.total ? (p.done / p.total) * 100 : 0}%`;
});
listen("dl-done", ({ payload: p }) => {
  upsertDl(p.id, { state: "done", path: p.path, bytes: p.bytes, skipped: p.skipped });
  S.batch.done++;
  const c = S.byId.get(p.id);
  if (c) c.saved = true;
  patchDl(p.id);
  updateQueueBadge();
  maybeFinishBatch();
  const el = patchCard(p.id);
  if (el && !p.skipped) { el.classList.add("just-done"); setTimeout(() => el.classList.remove("just-done"), 1500); }
});
listen("dl-error", ({ payload: p }) => {
  upsertDl(p.id, { state: "error", error: p.error });
  S.batch.failed++;
  patchDl(p.id);
  updateQueueBadge();
  maybeFinishBatch();
  patchCard(p.id);
});

/* UI wiring */
async function submitText(text) {
  const res = await invoke("add_links", { text });
  if (res.queued > 0) return true;
  if (res.profile) { openProfile(res.profile); return true; }
  return false;
}

const updateLinkCta = () => {
  const active = $("#link-input").value.trim().length > 0;
  const btn = $("#link-submit");
  btn.classList.toggle("btn-accent", active);
  btn.classList.toggle("btn-quiet", !active);
};
$("#link-input").addEventListener("input", updateLinkCta);
$("#link-form").onsubmit = async (e) => {
  e.preventDefault();
  const input = $("#link-input");
  const text = input.value.trim();
  if (!text) return input.focus();
  if (!(await submitText(text))) toast("No new Medal links found in that", "err");
  input.value = "";
  updateLinkCta();
};
$("#link-input").addEventListener("paste", () => setTimeout(() => {
  // pasting a link is intent enough: start right away
  if (/medal\.tv\//i.test($("#link-input").value)) $("#link-form").requestSubmit();
}, 0));

// Paste or drop links / .txt files anywhere in the window
document.addEventListener("paste", async (e) => {
  if (e.target.matches("input, textarea")) return;
  const text = e.clipboardData?.getData("text") || "";
  if (!/medal\.tv\//i.test(text)) return;
  if (await submitText(text)) toast("Started from clipboard");
});

let dragDepth = 0;
document.addEventListener("dragenter", (e) => {
  e.preventDefault();
  if (++dragDepth === 1) document.body.classList.add("drop-active");
});
document.addEventListener("dragleave", () => {
  if (--dragDepth <= 0) { dragDepth = 0; document.body.classList.remove("drop-active"); }
});
document.addEventListener("dragover", (e) => e.preventDefault());
document.addEventListener("drop", async (e) => {
  e.preventDefault();
  dragDepth = 0;
  document.body.classList.remove("drop-active");
  let text = e.dataTransfer?.getData("text/uri-list") || e.dataTransfer?.getData("text/plain") || "";
  for (const f of e.dataTransfer?.files || []) {
    if (f.size < 2 * 1024 * 1024) text += "\n" + (await f.text());
  }
  if (!text.trim()) return;
  if (!(await submitText(text))) toast("No Medal links found in what you dropped", "err");
});

$("#folder-btn").onclick = async () => {
  const dir = await invoke("pick_dir");
  if (!dir) return;
  await refreshInfo();
  toast(`Saving clips to ${folderName(dir)}`);
  if (S.info.loggedIn) loadLibrary(true);
};
$("#queue-btn").onclick = () => { $("#drawer").hidden = !$("#drawer").hidden; };
$("#close-drawer").onclick = () => { $("#drawer").hidden = true; };
$("#open-folder").onclick = () => invoke("open_dir");
$("#clear-done").onclick = () => {
  for (const [id, d] of S.dls) if (d.state === "done" || d.state === "error") S.dls.delete(id);
  renderDrawer();
};
$("#dl-list").onclick = (e) => {
  const li = e.target.closest(".dl.done, .dl.error");
  if (!li) return;
  if (li.classList.contains("error")) return retryDownloads([li.dataset.id]);
  invoke("reveal", { path: S.dls.get(li.dataset.id).path });
};
$("#retry-failed").onclick = () => {
  retryDownloads([...S.dls].filter(([, d]) => d.state === "error").map(([id]) => id));
};
$("#watch-toggle").onchange = async (e) => {
  await invoke("set_clipboard_watch", { on: e.target.checked });
  S.info.clipboardWatch = e.target.checked;
  if (e.target.checked) toast("Copied Medal links will download automatically");
};

$("#sel-clear").onclick = () => { S.selected.clear(); renderGrid(); };
$("#sel-download").onclick = () => {
  const clips = [...S.selected].map((id) => S.byId.get(id)).filter(Boolean);
  S.selected.clear();
  renderGrid();
  startDownloads(clips);
};

$("#token-form").onsubmit = async (e) => {
  if (e.submitter?.value !== "ok") return;
  e.preventDefault();
  const btn = $("#token-submit");
  btn.disabled = true; btn.textContent = "Checking…";
  try {
    await invoke("login_with_token", { token: $("#token-input").value });
    $("#token-dialog").close();
  } catch (err) {
    $("#token-error").textContent = String(err);
    $("#token-error").hidden = false;
  } finally {
    btn.disabled = false; btn.textContent = "Log in";
  }
};

document.addEventListener("keydown", (e) => {
  const search = $("#search");
  if ((e.key === "/" && !e.target.matches("input, textarea")) || (e.ctrlKey && e.key.toLowerCase() === "f")) {
    if (search) { e.preventDefault(); search.focus(); search.select(); }
    return;
  }
  if (e.target === search && e.key === "Escape") {
    search.value = ""; S.query = ""; search.blur(); renderGrid();
    return;
  }
  if (e.target.matches("input")) return;
  if (e.key === "Escape" && S.selected.size) { S.selected.clear(); renderGrid(); }
  if (e.key.toLowerCase() === "a" && e.ctrlKey && $("#grid")) { e.preventDefault(); toggleSelectAll(); }
  if (e.key === "Enter" && S.selected.size) $("#sel-download").click();
});

boot();
