// new chat (Nov 18): the "Nuova chat" window. You only write what you want to do; Juggl picks the project from the text,
// the open chat and the recently used projects, shows it as a chip you can change (3-4 proposals with logo and folder),
// warns when a chat on the same topic already exists (Enter = continue there, ⌘Enter = new anyway, or new as a sub-chat),
// and keeps model and permissions in "Altre opzioni". Study: share.juggl.dev/juggl-new-chat-study-950eca68ef70fbbb/.
// Compatibility: keeps the ids #nuova-testo, #nuova-cartella (chosen folder) and #nuova-mod (model chips) used by
// ui/engines.js (Codex chip, Codex start) and by the model chip handler in index.html. Load BEFORE engines.js.
// window.apriNuova({ progetto: id }) opens on that project (sidebar "nuova chat in questo progetto"); no argument = ⌘N.
// Hooks in index.html (marked "new chat (Nov 18)"): the <script src="new-chat.js"> tag; the old window code was removed.
(() => {
  const TT = window.__TAURI__;
  if (!TT) return;
  const inv = (c, a) => TT.core.invoke(c, a);
  const q = (s) => document.querySelector(s);
  const E = (s) => String(s ?? "").replace(/[&<>"']/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" }[c]));
  const norm = (s) => String(s || "").toLowerCase().normalize("NFD").replace(/[̀-ͯ]/g, "");
  const ARROW_DOWN = '<svg viewBox="0 0 12 12" fill="none" stroke="currentColor" stroke-width="1.8"><path d="m3 4.5 3 3 3-3"/></svg>';
  const ARROW_RIGHT = '<svg viewBox="0 0 12 12" fill="none" stroke="currentColor" stroke-width="1.8"><path d="m4.5 3 3 3-3 3"/></svg>';
  const FOLDER = '<svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.5"><path d="M2 4.5h4l1.5 1.5H14v6.5H2z"/></svg>';
  const SEARCH = '<svg width="12" height="12" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.8"><circle cx="7" cy="7" r="5"/><path d="m11 11 3.5 3.5"/></svg>';
  // Every visible text of this file, in one object (to be moved to the Languages catalog: ui/locales/*.json).
  const TXT = {
    title: "Nuova chat", closeHint: "Esc per chiudere", placeholder: "Cosa vuoi fare?", in: "In",
    quick: "Chat veloce", quickSub: "senza progetto", otherFolder: "Un'altra cartella…", otherFolderSub: "scrivi il percorso in Altre opzioni",
    search: "Cerca un progetto o una cartella", noMatch: "Nessun progetto con questo nome", chipTitle: "Cambia dove parte (⌘P)",
    similar: "C'è già una chat simile", sub: "Nuova, sotto questa", subTitle: "Apre una chat nuova sotto questa nell'elenco",
    more: "Altre opzioni", enter: "Invio", enterThere: "continua lì", enterGo: "per partire", newAnyway: "Nuova comunque", continueThere: "Continua lì", start: "Avvia",
    model: "Modello", perms: "Permessi", folder: "Cartella", folderPlaceholder: "vuota = chat veloce", defaultModel: "Predefinito",
    neverWarn: "Claude esegue comandi e modifica file senza chiederti nulla, solo in questa chat.",
    notStarted: "Non avviata: ", working: "sta lavorando", waiting: "ti aspetta", stoppedFor: (t) => `ferma da ${t}`,
    whyText: "dal testo", whyOpen: "chat aperta", whyManual: "scelto da te", whyList: "dall'elenco", whyRecent: "di recente",
    lineWords: (w) => `dalle parole ${w}`, lineOpen: "come la chat aperta", lineRecent: (t) => `usato di recente, ${t}`, lineList: "dall'elenco",
    agoNow: "poco fa", agoHours: (n) => `${n} h fa`, agoYesterday: "ieri", agoDays: (n) => `${n} giorni fa`,
    forNow: "poco", forHours: (n) => `${n} h`, forYesterday: "ieri", forDays: (n) => `${n} giorni`,
    pcStudio: "PC studio", pcAgent: "Agente PC",
    perms_: [["", "Come le impostazioni"], ["chiedi", "Chiedi"], ["modifiche", "Modifiche"], ["mai", "Mai chiedere"]],
    permSummary: { chiedi: "chiede prima di modificare", modifiche: "modifica senza chiedere, chiede per i comandi", mai: "non chiede mai" },
  };
  const PERMS = TXT.perms_;
  const PERM_SUMMARY = TXT.permSummary;

  const css = `
.foglio.nc{width:min(640px,94vw);padding:16px 18px 14px}
.nc-head{display:flex;align-items:baseline;gap:10px;margin:0 2px 10px}
.nc-head h3{margin:0;font:600 17px/1.2 var(--serif)}
.nc-head kbd{font:10.5px var(--sans);color:var(--ink3);box-shadow:inset 0 0 0 1px var(--line);border-radius:5px;padding:1px 5px}
.nc-head small{margin-left:auto;color:var(--ink3);font-size:11.5px}
.foglio.nc .nc-text{width:100%;min-height:96px;max-height:40vh;resize:none;border:1px solid var(--line);border-radius:14px;background:var(--bg);color:var(--ink);font:15px/1.5 var(--sans);padding:12px 14px;outline:none;box-shadow:0 1px 3px rgba(0,0,0,.04)}
.foglio.nc .nc-text:focus{border-color:var(--ink3)}
.foglio.nc .nc-text::placeholder{color:var(--ink3)}
.nc-where{display:flex;align-items:center;gap:8px;margin:10px 2px 0;font-size:12.5px;color:var(--ink3);flex-wrap:wrap;position:relative}
.nc-chip{display:inline-flex;align-items:center;gap:7px;border:1px solid var(--line);border-radius:999px;padding:3px 10px 3px 4px;font-size:13px;color:var(--ink);background:var(--bg);max-width:60%}
.nc-chip:hover,.nc-chip.open{background:var(--fill)}
.nc-chip b{font-weight:600;white-space:nowrap;overflow:hidden;text-overflow:ellipsis}
.nc-chip>svg{width:9px;height:9px;color:var(--ink3);flex:none}
.nc-chip .nc-ic{width:22px;height:22px;border-radius:50%;display:grid;place-items:center;background:var(--fill2);color:var(--ink2);flex:none;font-size:11px}
.nc-chip .nc-ic svg{width:12px;height:12px}
.nc-why{font-size:11.5px;color:var(--ink3);white-space:nowrap;overflow:hidden;text-overflow:ellipsis;min-width:0;flex:1}
.nc-why code{font:11px var(--mono);background:none;padding:0;color:var(--ink2)}
.nc-menu{position:absolute;left:22px;top:32px;z-index:5;width:min(380px,80vw);background:var(--bg);border:1px solid var(--line);border-radius:14px;box-shadow:0 14px 40px rgba(0,0,0,.18);padding:5px;animation:ncIn .14s ease-out}
@keyframes ncIn{from{opacity:0;transform:translateY(3px)}to{opacity:1;transform:none}}
.nc-menu .nc-search{display:flex;align-items:center;gap:8px;margin:2px 4px 4px;padding:0 9px;border-radius:9px;background:var(--fill);color:var(--ink3)}
.foglio.nc .nc-menu .nc-search input{flex:1;border:0;background:none;outline:none;font:12.5px var(--sans);color:var(--ink);padding:7px 0;width:auto}
.nc-menu .nc-list{max-height:min(320px,42vh);overflow:auto}
.nc-menu .v{display:flex;align-items:center;gap:10px;padding:7px 9px;border-radius:9px;color:var(--ink);width:100%;text-align:left}
.nc-menu .v.on{background:var(--fill)}
.nc-menu .v div{flex:1;min-width:0}
.nc-menu .v b{display:block;font-weight:500;font-size:13px;white-space:nowrap;overflow:hidden;text-overflow:ellipsis}
.nc-menu .v small{display:block;color:var(--ink3);font-size:11.5px;white-space:nowrap;overflow:hidden;text-overflow:ellipsis}
.nc-menu .v small.path{font:11px var(--mono)}
.nc-menu .v em{font-style:normal;font-size:11px;color:var(--ink3);flex:none}
.nc-menu .v .nc-ic{width:26px;height:26px;border-radius:50%;display:grid;place-items:center;background:var(--fill2);color:var(--ink2);flex:none}
.nc-menu .v .nc-ic svg{width:13px;height:13px}
.nc-menu hr{border:0;border-top:1px solid var(--line);margin:4px 6px}
.nc-menu .nc-none{padding:8px 10px;color:var(--ink3);font-size:12.5px}
.nc-dup{display:flex;align-items:center;gap:11px;margin:12px 0 0;border:1px solid var(--line);border-radius:12px;padding:10px 12px;animation:ncIn .2s ease-out}
.nc-dup .tx{flex:1;min-width:0}
.nc-dup .tx small{display:block;color:var(--ink3);font-size:11.5px}
.nc-dup .tx b{display:block;font-weight:600;font-size:13.5px;white-space:nowrap;overflow:hidden;text-overflow:ellipsis}
.nc-dup .tx span{display:block;color:var(--ink2);font-size:12px;white-space:nowrap;overflow:hidden;text-overflow:ellipsis}
.nc-dup button{font-size:12px;padding:5px 10px;border-radius:8px;border:1px solid var(--line);color:var(--ink2);white-space:nowrap;flex:none}
.nc-dup button:hover{background:var(--fill)}
.nc-opts{margin:12px 2px 0;display:grid;grid-template-columns:84px 1fr;gap:8px 12px;align-items:center;font-size:12.5px;color:var(--ink3)}
.nc-opts .chips{margin:0}
.foglio.nc .nc-opts input{font:12px var(--mono);padding:6px 9px;border-radius:8px}
.nc-opts .nc-warn{grid-column:2;color:var(--err);font-size:12px;margin-top:-2px}
.nc-foot{display:flex;align-items:center;gap:10px;margin-top:14px}
.nc-more{display:inline-flex;align-items:center;gap:6px;color:var(--ink3);font-size:12px;padding:4px 6px;border-radius:7px;min-width:0}
.nc-more:hover{background:var(--fill)}
.nc-more>svg{width:9px;height:9px;flex:none;transition:transform .15s}
.nc-more.open>svg{transform:rotate(90deg)}
.nc-more span{color:var(--ink2);white-space:nowrap;overflow:hidden;text-overflow:ellipsis}
.nc-foot small{margin-left:auto;color:var(--ink3);font-size:11.5px;white-space:nowrap}
.nc-foot kbd{font:11px var(--sans);border:1px solid var(--line);border-radius:5px;padding:0 5px;color:var(--ink2)}
.nc-foot button{white-space:nowrap}
`;
  const st = document.createElement("style"); st.id = "new-chat-css"; st.textContent = css; document.head.appendChild(st);

  // ---------- state ----------
  // where: { kind: "project" | "quick" | "folder", p?, folder?, why } ; manual = the user picked it, the text no longer changes it
  const NC = { where: null, manual: false, ranked: [], menu: false, menuQ: "", menuI: 0, dup: null, skipDup: new Set(), opts: false, perm: "", globalPerm: "chiedi", timer: 0 };
  const sheet = () => q("#foglio.nc");

  // ---------- understanding the text ----------
  const STOP = new Set("che chi con per una uno gli le la lo il dei del della delle degli nel nella nei sul sulla sui come cosa dove quando anche ancora fai fare mi ti si ci vi non piu poi solo tutto tutti questa questo quella quello sono ho hai ha abbiamo avere essere stato stata alla allo agli alle dal dalla dai dalle tra fra mio mia tuo tua suo sua the and for with from this that into".split(" "));
  /** Significant words: lowercase, no accents, at least 3 letters, no stopwords. */
  function words(t) { return norm(t).split(/[^a-z0-9]+/).filter((w) => w.length >= 3 && !STOP.has(w)); }
  const stem = (w) => w.slice(0, 5);
  const base = (c) => String(c || "").split("/").filter(Boolean).pop() || "";
  const ago = (s, short) => {
    if (!s) return "";
    const m = (Date.now() / 1000 - s) / 60;
    if (m < 60) return short ? TXT.forNow : TXT.agoNow;
    if (m < 60 * 24) return (short ? TXT.forHours : TXT.agoHours)(Math.round(m / 60));
    if (m < 60 * 48) return short ? TXT.forYesterday : TXT.agoYesterday;
    return (short ? TXT.forDays : TXT.agoDays)(Math.round(m / 1440));
  };
  const allProjects = () => (window.EL?.progettiPerUso ? EL.progettiPerUso() : (window.EL?.progetti || [])).filter((p) => p?.id && p.id !== "locale");
  const openProject = () => { try { const p = (typeof det !== "undefined" && det) ? det.progetto : null; return p?.id && p.id !== "locale" ? p : null; } catch { return null; } };
  const sessions = () => { try { return (dati.sessioni || []).filter((s) => s.sessione !== dati.juggl && !String(s.sessione || "").startsWith("locale:") && !String(s.sessione || "").startsWith("wa:")); } catch { return []; } };

  /** Projects ranked for this text: words of the project (name, keywords, folder), names of its chats, open chat, recent use. */
  function rank(text) {
    const tw = words(text), ts = new Set(tw.map(stem)), nt = " " + norm(text) + " ";
    const open = openProject(), list = allProjects();
    const chatsOf = new Map();
    for (const s of sessions()) if (s.progetto?.id) (chatsOf.get(s.progetto.id) || chatsOf.set(s.progetto.id, []).get(s.progetto.id)).push(s);
    return list.map((p, i) => {
      const hits = [];
      let score = 0;
      for (const [k, w] of [[p.nome, 3], [p.id, 2], [base(p.cartella), 2], ...(Array.isArray(p.parole) ? p.parole : []).map((x) => [x, 2])]) {
        const nk = norm(k).trim(); if (nk.length < 3) continue;
        if (nt.includes(" " + nk) || (nk.length >= 5 && nt.includes(nk))) {
          score += w;
          // shown words: no technical id, no repeats ("App mobile" already covers "app")
          if (k !== p.id && !hits.some((h) => norm(h).includes(nk) || nk.includes(norm(h)))) hits.push(k);
        }
      }
      // topic: words in common with the names of its chats
      let topic = 0; const tHits = [];
      for (const s of chatsOf.get(p.id) || []) for (const w of words(s.nome)) if (ts.has(stem(w)) && !tHits.includes(w)) { topic += 0.6; tHits.push(w); }
      score += Math.min(topic, 2.4);
      const fromText = score > 0;
      if (open && open.id === p.id) score += 1.5;
      score += 0.8 / (1 + i);   // recent use (list already sorted by last activity)
      const why = fromText ? { kind: "text", words: [...hits, ...tHits.filter((w) => !hits.some((h) => norm(h).includes(w) || w.includes(norm(h))))].slice(0, 3) }
        : open && open.id === p.id ? { kind: "open" } : { kind: "recent", t: p.ultimo };
      return { p, score, why };
    }).sort((a, b) => b.score - a.score);
  }
  const whyLabel = (w) => !w ? "" : w.kind === "text" ? TXT.whyText : w.kind === "open" ? TXT.whyOpen : w.kind === "manual" ? TXT.whyManual : w.kind === "list" ? TXT.whyList : ago(w.t) || TXT.whyRecent;
  function whyLine(w) {
    if (!w) return "";
    if (w.kind === "text") return w.words?.length ? TXT.lineWords(w.words.map((x) => `<code>${E(x)}</code>`).join(", ")) : TXT.whyText;
    if (w.kind === "open") return TXT.lineOpen;
    if (w.kind === "recent") return w.t ? TXT.lineRecent(ago(w.t)) : "";
    if (w.kind === "list") return TXT.lineList;
    return "";
  }

  /** A chat on the same topic in the chosen place: at least 2 important words in common with its name
   *  (or all the words of a short name), excluding chats the user already said no to. */
  function findDup(text) {
    const w = NC.where; if (!w || w.kind === "quick") return null;
    const ts = new Set(words(text).map(stem)); if (ts.size < 2) return null;
    let best = null;
    for (const s of sessions()) {
      if (NC.skipDup.has(s.sessione)) continue;
      const same = w.kind === "project" ? s.progetto?.id === w.p.id : String(s.cwd || "").replace(/\/$/, "") === String(w.folder || "").replace(/\/$/, "");
      if (!same) continue;
      const nw = [...new Set(words(s.nome).map(stem))]; if (!nw.length) continue;
      const common = nw.filter((x) => ts.has(x)).length;
      const ok = common >= 2 || (nw.length <= 2 && common === nw.length && nw.every((x) => x.length >= 5));
      if (!ok) continue;
      const score = common / nw.length + common * 0.5 + (s.da || 0) / 1e12;
      if (!best || score > best.score) best = { s, score };
    }
    return best?.s || null;
  }

  // ---------- drawing ----------
  function icon(p, size) {
    if (window.EL?.icona) return EL.icona({ nome: p.nome, progetto: p }, "gr", size);
    return typeof avatar === "function" ? avatar({ nome: p.nome, progetto: p }, "piccolo") : "";
  }
  function chipHtml() {
    const w = NC.where;
    const inner = !w || w.kind === "quick" ? `<span class="nc-ic">⚡</span><b>${E(TXT.quick)}</b>`
      : w.kind === "folder" ? `<span class="nc-ic">${FOLDER}</span><b>${E(base(w.folder) || w.folder)}</b>`
      : `${icon(w.p, 22)}<b>${E(w.p.nome)}</b>`;
    return `<button class="nc-chip${NC.menu ? " open" : ""}" type="button" data-nc-chip title="${E(TXT.chipTitle)}">${inner}${ARROW_DOWN}</button>`;
  }
  function menuHtml() {
    if (!NC.menu) return "";
    const qn = norm(NC.menuQ).trim();
    const items = qn
      ? allProjects().filter((p) => [p.nome, ...(Array.isArray(p.parole) ? p.parole : []), base(p.cartella)].some((x) => norm(x).includes(qn))).slice(0, 8).map((p) => ({ p, why: { kind: "recent", t: p.ultimo } }))
      : NC.ranked.slice(0, 4);
    const rows = items.map((x, i) => ({ k: "p:" + x.p.id, h: `${icon(x.p, 26)}<div><b>${E(x.p.nome)}</b><small class="path">${E(x.p.cartella || "")}</small></div><em>${E(whyLabel(x.why))}</em>` }));
    rows.push({ sep: true });
    rows.push({ k: "quick", h: `<span class="nc-ic">⚡</span><div><b>${E(TXT.quick)}</b><small>${E(TXT.quickSub)}</small></div>` });
    rows.push({ k: "folder", h: `<span class="nc-ic">${FOLDER}</span><div><b>${E(TXT.otherFolder)}</b><small>${E(TXT.otherFolderSub)}</small></div>` });
    const sel = rows.filter((r) => !r.sep);
    NC.menuI = Math.max(0, Math.min(NC.menuI, sel.length - 1));
    let j = -1;
    return `<div class="nc-menu" role="listbox"><label class="nc-search">${SEARCH}<input id="nc-search" placeholder="${E(TXT.search)}" value="${E(NC.menuQ)}" autocomplete="off" spellcheck="false"></label>
      <div class="nc-list">${qn && !items.length ? `<div class="nc-none">${E(TXT.noMatch)}</div>` : ""}${rows.map((r) => r.sep ? "<hr>" : `<button type="button" class="v${++j === NC.menuI ? " on" : ""}" data-nc-pick="${E(r.k)}" role="option">${r.h}</button>`).join("")}</div></div>`;
  }
  function dupHtml() {
    const s = NC.dup; if (!s) return "";
    const p = s.progetto?.id ? s.progetto : null;
    const stato = s.vista === "lavora" ? TXT.working : s.vista === "aspetta" || s.vista === "bloccata" ? TXT.waiting : s.da ? TXT.stoppedFor(ago(s.da, true)) : "";
    return `<div class="nc-dup">${p ? icon(p, 28) : ""}<div class="tx"><small>${E(TXT.similar)}</small><b>${E(s.nome || "")}</b><span>${E([p?.nome, stato].filter(Boolean).join(" · "))}</span></div>
      <button type="button" data-nc-sub title="${E(TXT.subTitle)}">${E(TXT.sub)}</button></div>`;
  }
  function optsSummary() {
    const m = q("#nuova-mod .chip.on");
    const model = m && (m.dataset.mclaude || m.dataset.mpc) ? m.textContent.trim() : (dati?.imp?.modello_nuove ? dati.imp.modello_nuove[0].toUpperCase() + dati.imp.modello_nuove.slice(1) : "");
    const perm = PERM_SUMMARY[NC.perm || NC.globalPerm] || "";
    return [model, perm].filter(Boolean).join(" · ");
  }
  function draw(focus) {
    const f = sheet(); if (!f) return;
    const box = f.querySelector(".nc-where");
    box.innerHTML = `<span>${E(TXT.in)}</span>${chipHtml()}<span class="nc-why">${NC.manual ? "" : whyLine(NC.where?.why)}</span>${menuHtml()}`;
    f.querySelector(".nc-dupbox").innerHTML = dupHtml();
    const foot = f.querySelector(".nc-foot"), dup = !!NC.dup;
    const sum = optsSummary();
    foot.innerHTML = `<button type="button" class="nc-more${NC.opts ? " open" : ""}" data-nc-more>${ARROW_RIGHT}${E(TXT.more)}${!NC.opts && !dup && sum ? ` <span>· ${E(sum)}</span>` : ""}</button>`
      + (dup ? `<small><kbd>${E(TXT.enter)}</kbd> ${E(TXT.enterThere)}</small><button type="button" class="tasto" data-nc-new title="⌘${E(TXT.enter)}">${E(TXT.newAnyway)}</button><button type="button" class="btn-nero" data-nc-there>${E(TXT.continueThere)}</button>`
        : `<small><kbd>${E(TXT.enter)}</kbd> ${E(TXT.enterGo)}</small><button type="button" class="btn-nero" data-nc-go>${E(TXT.start)}</button>`);
    f.querySelector(".nc-opts").hidden = !NC.opts;
    const warn = f.querySelector(".nc-warn"); if (warn) warn.hidden = NC.perm !== "mai";
    if (focus === "search") q("#nc-search")?.focus();
  }
  /** Chosen folder for the start (also read by engines.js). */
  function syncFolder() {
    const w = NC.where, inp = q("#nuova-cartella"); if (!inp) return;
    if (w?.kind === "project") inp.value = w.p.cartella || "";
    else if (w?.kind === "quick") inp.value = "";
    else if (w?.kind === "folder") inp.value = w.folder || "";
  }
  function think() {
    const text = q("#nuova-testo")?.value || "";
    NC.ranked = rank(text);
    if (!NC.manual) {
      const top = NC.ranked[0];
      NC.where = top ? { kind: "project", p: top.p, why: top.why } : { kind: "quick" };
    }
    syncFolder();
    NC.dup = findDup(text);
    draw();
  }

  // ---------- open ----------
  async function open(opts) {
    opts = opts && typeof opts === "object" ? opts : {};
    Object.assign(NC, { where: null, manual: false, ranked: [], menu: false, menuQ: "", menuI: 0, dup: null, opts: false, perm: "" });
    NC.skipDup = new Set();
    const f = q("#foglio"); f.classList.remove("salto", "imp", "ic"); f.classList.add("nc");
    f.innerHTML = `<div class="nc-head"><h3>${E(TXT.title)}</h3><kbd>⌘N</kbd><small>${E(TXT.closeHint)}</small></div>
      <textarea id="nuova-testo" class="nc-text" placeholder="${E(TXT.placeholder)}" spellcheck="true"></textarea>
      <div class="nc-where"></div><div class="nc-dupbox"></div>
      <div class="nc-opts" hidden>
        <span>${E(TXT.model)}</span><div class="chips" id="nuova-mod">${(typeof MODELLI !== "undefined" ? MODELLI : [["", TXT.defaultModel]]).map(([v, t]) => `<button class="chip ${v === "" ? "on" : ""}" data-mclaude="${E(v)}">${E(v === "" && /^(fable|opus|sonnet|haiku)$/.test(dati?.imp?.modello_nuove || "") ? `${t} (${dati.imp.modello_nuove[0].toUpperCase()}${dati.imp.modello_nuove.slice(1)})` : t)}</button>`).join("")}<span class="chips-pc"></span></div>
        <span>${E(TXT.perms)}</span><div class="chips" id="nc-perm">${PERMS.map(([v, t]) => `<button class="chip ${v === "" ? "on" : ""}" data-nc-perm="${v}">${E(t)}</button>`).join("")}</div>
        <span class="nc-warn" hidden>${E(TXT.neverWarn)}</span>
        <span>${E(TXT.folder)}</span><input id="nuova-cartella" spellcheck="false" placeholder="${E(TXT.folderPlaceholder)}">
      </div>
      <div class="nc-foot"></div>`;
    q("#velo").classList.add("on");
    const ta = q("#nuova-testo"); ta.focus({ preventScroll: true });
    if (opts.testo) ta.value = opts.testo;
    // projects of the sidebar may still be loading: ask once if empty
    if (!allProjects().length) { try { const pp = await inv("progetti"); if (window.EL && !EL.progetti?.length) EL.progetti = pp; } catch {} }
    if (opts.progetto) {
      const p = allProjects().find((x) => x.id === opts.progetto);
      if (p) { NC.where = { kind: "project", p, why: { kind: "list" } }; NC.manual = true; }
    }
    inv("permessi_stato").then((r) => { NC.globalPerm = r?.livello || r?.globale || NC.globalPerm; draw(); }).catch(() => {});
    think();
    // models of the PC arrive later: the window opens at once even if the PC is off
    inv("modelli_locali").then((ml) => {
      const b = q("#nuova-mod .chips-pc"); if (!b || !ml?.ok || !ml.modelli?.length) { b?.remove(); return; }
      b.outerHTML = `<span class="sep-chip">${E(TXT.pcStudio)}</span>` + (ml.agente ? `<button class="chip" data-mpc="agente-pc">${E(TXT.pcAgent)}</button>` : "") + ml.modelli.map((m) => `<button class="chip" data-mpc="${E(m)}">${E(m.split(":")[0])}</button>`).join("");
    }).catch(() => q("#nuova-mod .chips-pc")?.remove());
  }

  // ---------- start ----------
  let starting = false;
  async function start(mode) {
    if (starting || !sheet()) return;
    const text = q("#nuova-testo").value;
    if (!text.trim()) { q("#nuova-testo").focus(); return; }
    if (NC.dup && mode !== "new" && mode !== "sub") return continueThere();
    starting = true;
    try {
      const pc = q("#nuova-mod .chip.on[data-mpc]")?.dataset.mpc;
      const r = pc ? await inv("nuova_locale", { modello: pc, testo: text })
        : await inv("nuova", { cartella: q("#nuova-cartella").value, testo: text, modello: q("#nuova-mod .chip.on[data-mclaude]")?.dataset.mclaude || "", effort: "" });
      if (!r?.ok) { toast(TXT.notStarted + (r?.msg || ""), 4000); return; }
      if (!pc && NC.perm) await inv("permessi_chat", { id: r.id, livello: NC.perm, confermato: NC.perm === "mai" }).catch(() => {});
      if (mode === "sub" && NC.dup) await inv("unisci_chat", { figlia: r.id, principale: NC.dup.sessione }).catch(() => {});
      chiudiNuova(); await caricaElenco(); scegli(r.id);
    } finally { starting = false; }
  }
  function continueThere() {
    const s = NC.dup, text = q("#nuova-testo").value;
    chiudiNuova();
    // the text goes in its box, it is not sent: you decide
    if (typeof apriConBozza === "function") apriConBozza(s.sessione, text); else scegli(s.sessione);
  }
  function pick(k) {
    if (k === "quick") NC.where = { kind: "quick" };
    else if (k === "folder") { NC.opts = true; NC.where = { kind: "folder", folder: q("#nuova-cartella").value || "" }; NC.menu = false; NC.manual = true; draw(); q("#nuova-cartella")?.focus(); return; }
    else { const p = allProjects().find((x) => "p:" + x.id === k); if (!p) return; NC.where = { kind: "project", p, why: { kind: "manual" } }; }
    NC.manual = true; NC.menu = false; NC.menuQ = ""; syncFolder(); NC.dup = findDup(q("#nuova-testo").value);
    draw(); q("#nuova-testo")?.focus();
  }

  // ---------- events ----------
  document.addEventListener("input", (e) => {
    if (!sheet()) return;
    if (e.target.id === "nuova-testo") { clearTimeout(NC.timer); NC.timer = setTimeout(think, 140); }
    else if (e.target.id === "nc-search") { NC.menuQ = e.target.value; NC.menuI = 0; const pos = e.target.selectionStart; draw("search"); const i = q("#nc-search"); i?.setSelectionRange(pos, pos); }
    else if (e.target.id === "nuova-cartella") { const v = e.target.value.trim(); NC.where = v ? { kind: "folder", folder: v } : { kind: "quick" }; NC.manual = true; NC.dup = findDup(q("#nuova-testo").value); draw(); }
  });
  document.addEventListener("click", (e) => {
    const f = sheet(); if (!f || !f.contains(e.target)) return;
    if (e.target.closest("[data-nc-chip]")) { NC.menu = !NC.menu; NC.menuQ = ""; NC.menuI = 0; draw(NC.menu ? "search" : null); return; }
    const pk = e.target.closest("[data-nc-pick]"); if (pk) return pick(pk.dataset.ncPick);
    if (NC.menu && !e.target.closest(".nc-menu")) { NC.menu = false; draw(); }
    if (e.target.closest("[data-nc-more]")) { NC.opts = !NC.opts; draw(); return; }
    const pm = e.target.closest("[data-nc-perm]");
    if (pm) { NC.perm = pm.dataset.ncPerm; f.querySelectorAll("[data-nc-perm]").forEach((x) => x.classList.toggle("on", x === pm)); draw(); return; }
    if (e.target.closest("[data-nc-go]")) return window.avviaNuova();
    if (e.target.closest("[data-nc-there]")) return continueThere();
    if (e.target.closest("[data-nc-new]")) { if (NC.dup) NC.skipDup.add(NC.dup.sessione); return window.avviaNuova("new"); }
    if (e.target.closest("[data-nc-sub]")) return window.avviaNuova("sub");
    if (e.target.closest("#nuova-mod .chip")) setTimeout(draw, 0);   // summary of "Altre opzioni" (the chip itself is toggled by index.html)
  });
  // keys: before the general handler of index.html (Enter there would start without these rules)
  window.addEventListener("keydown", (e) => {
    const f = sheet(); if (!f || e.isComposing) return;
    const stop = () => { e.preventDefault(); e.stopImmediatePropagation(); };
    if (NC.menu) {
      const n = f.querySelectorAll("[data-nc-pick]").length;
      if (e.key === "ArrowDown") { stop(); NC.menuI = (NC.menuI + 1) % n; return draw("search"); }
      if (e.key === "ArrowUp") { stop(); NC.menuI = (NC.menuI - 1 + n) % n; return draw("search"); }
      if (e.key === "Enter") { stop(); const b = f.querySelectorAll("[data-nc-pick]")[NC.menuI]; if (b) pick(b.dataset.ncPick); return; }
      if (e.key === "Escape") { stop(); NC.menu = false; draw(); q("#nuova-testo")?.focus(); return; }
    }
    if (e.metaKey && (e.key === "p" || e.key === "P") && !e.shiftKey) { stop(); NC.menu = true; NC.menuQ = ""; NC.menuI = 0; return draw("search"); }
    if (e.key === "Enter" && !e.shiftKey && (e.target.id === "nuova-testo" || e.target.id === "nuova-cartella")) {
      stop();
      if (e.metaKey) { if (NC.dup) NC.skipDup.add(NC.dup.sessione); return window.avviaNuova("new"); }
      return window.avviaNuova();
    }
  }, true);

  // another sheet reusing #foglio (settings, confirmations) must not keep the new chat look
  const fg = q("#foglio");
  if (fg) new MutationObserver(() => { if (fg.classList.contains("nc") && !fg.querySelector(".nc-text")) fg.classList.remove("nc"); }).observe(fg, { childList: true });

  window.apriNuova = open;
  // always through window.avviaNuova: engines.js wraps it to start Codex chats
  window.avviaNuova = (mode) => start(mode);
  // for tools/test-new-chat.mjs
  window.nuovaChatCapisci = (text, extra) => { const r = rank(text); return { top: r[0] ? { id: r[0].p.id, why: r[0].why } : null, ranked: r.slice(0, 4).map((x) => x.p.id), dup: (() => { const w0 = NC.where; NC.where = r[0] ? { kind: "project", p: r[0].p } : null; const d = findDup(text); NC.where = w0; return d?.sessione || null; })(), ...(extra || {}) }; };
})();
