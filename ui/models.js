// models (Nov 14): full model choice (src-tauri/src/commands.rs, command modelli_claude).
// The list used to be fixed (Default/Opus/Sonnet/Haiku). Now it comes from Claude Code ("initialize" handshake, models field):
// main ones on top (Default, Fable, Opus, Sonnet, Haiku: aliases always at the latest version), below "Versioni precedenti".
// For each model only the reasoning levels it supports. In the header the active model with name and version.
// Hooks in index.html (marked "modelli (14/11)"): only the <script src="models.js"> tag after commands.js.
(() => {
  const TT = window.__TAURI__;
  if (!TT) return;
  const inv = (c, a) => TT.core.invoke(c, a);
  const E = (s) => String(s ?? "").replace(/[&<>"']/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" }[c]));
  let LIST = [];   // [{v, nome, corto, desc, risolto, sforzi, principale}]

  const css = `
.rg-seg.mod-lista{flex-direction:column;flex-wrap:nowrap;gap:2px;max-height:min(330px,52vh);overflow-y:auto}
.rg-seg.mod-lista button{flex:none;display:flex;flex-direction:column;align-items:flex-start;text-align:left;padding:6px 9px;gap:1px}
.rg-seg.mod-lista button b{font-weight:600;font-size:12.5px;color:var(--ink)}
.rg-seg.mod-lista button small{font-size:11px;color:var(--ink3);line-height:1.3}
.rg-seg.mod-lista button.on small{color:var(--ink2)}
.rg-seg.mod-lista .mod-sez{font-size:10.5px;font-weight:600;letter-spacing:.04em;text-transform:uppercase;color:var(--ink3);padding:8px 9px 2px;cursor:pointer;user-select:none;display:flex;justify-content:space-between}
.rg-seg.mod-lista .mod-altri[hidden]{display:none}
.rg-seg.mod-lista .mod-altri{display:flex;flex-direction:column;gap:2px}
.rg-seg button.mod-no{opacity:.35;pointer-events:none}
.regola-pop{width:360px}
`;
  const st = document.createElement("style"); st.id = "modelli-css"; st.textContent = css; document.head.appendChild(st);

  async function load() {
    try {
      const r = await inv("modelli_claude");
      if (!r?.ok || !r.modelli?.length) return;
      LIST = r.modelli;
      // MODELLI (index.html) stays the short list of the main ones: used by the New chat window and the old hooks
      if (typeof MODELLI !== "undefined" && Array.isArray(MODELLI)) {
        MODELLI.length = 0;
        for (const m of LIST.filter((x) => x.principale)) MODELLI.push([m.v, m.v ? m.corto : "Predefinito"]);
      }
      // the open chat header is redrawn with the real name
      try { if (typeof det !== "undefined" && det && typeof caricaDettaglio === "function") caricaDettaglio(false); } catch {}
    } catch (e) { console.error("modelli", e); }
  }

  // ---------- header: active model with name and version ----------
  const legacy = window.etichettaModello;
  if (typeof legacy === "function") {
    window.etichettaModello = function (d) {
      if (!d || d.locale || !LIST.length) return legacy(d);
      const m = LIST.find((x) => x.v === (d.modello || "")) || null;
      const label = m ? m.corto : d.modello || "Modello";
      const e = (typeof SFORZI !== "undefined" ? SFORZI : []).find((x) => x[0] === (d.effort || ""))?.[1];
      return d.effort ? `${label} · ${e || d.effort}` : label;
    };
  }

  // ---------- model panel: full list instead of the 4 buttons ----------
  function fix(pop) {
    if (!LIST.length || pop.querySelector("#rg-t")) return;   // #rg-t = chat with the PC model: not Claude
    const seg = pop.querySelector('.rg-seg[data-rg="modello"]');
    if (!seg || seg.classList.contains("mod-lista")) return;
    const current = seg.querySelector(".on")?.dataset.v ?? "";
    const btn = (m) => `<button data-v="${E(m.v)}" class="${m.v === current ? "on" : ""}"><b>${E(m.nome)}</b><small>${E(m.desc)}</small></button>`;
    const primary = LIST.filter((m) => m.principale), others = LIST.filter((m) => !m.principale);
    const otherSelected = others.some((m) => m.v === current);
    seg.classList.add("mod-lista");
    seg.innerHTML = primary.map(btn).join("") + (others.length ? `<div class="mod-sez" data-mod-altri><span>Versioni precedenti</span><span>${otherSelected ? "▾" : "▸"}</span></div><div class="mod-altri" ${otherSelected ? "" : "hidden"}>${others.map(btn).join("")}</div>` : "");
    // model the CLI no longer lists (typed by hand): stays selected, at the bottom
    if (current && !LIST.some((m) => m.v === current)) seg.insertAdjacentHTML("beforeend", btn({ v: current, nome: current, desc: "Scelto prima, non più nell'elenco" }));
    if (!seg.querySelector(".on")) seg.querySelector("button")?.classList.add("on");
    efforts(pop);
    // the panel is taller: it must stay inside the window
    const r = pop.getBoundingClientRect();
    if (r.bottom > innerHeight - 12) pop.style.top = Math.max(12, innerHeight - 12 - r.height) + "px";
    if (r.right > innerWidth - 12) pop.style.left = Math.max(12, innerWidth - 12 - r.width) + "px";
  }

  /** Only the reasoning levels the selected model supports (Default always). */
  function efforts(pop) {
    const v = pop.querySelector('.rg-seg[data-rg="modello"] .on')?.dataset.v ?? "";
    const m = LIST.find((x) => x.v === v);
    const ok = new Set(m ? m.sforzi : []);
    const seg = pop.querySelector('.rg-seg[data-rg="effort"]'); if (!seg) return;
    seg.querySelectorAll("button").forEach((b) => {
      const no = !!b.dataset.v && !ok.has(b.dataset.v);
      b.classList.toggle("mod-no", no); b.title = no ? "Questo modello non lo supporta" : "";
      if (no && b.classList.contains("on")) { b.classList.remove("on"); seg.querySelector('button[data-v=""]')?.classList.add("on"); }
    });
  }

  document.addEventListener("click", (e) => {
    const sec = e.target.closest?.("[data-mod-altri]");
    if (sec) {
      const box = sec.nextElementSibling; box.hidden = !box.hidden;
      sec.lastElementChild.textContent = box.hidden ? "▸" : "▾";
      e.stopPropagation(); return;
    }
    const b = e.target.closest?.('.regola-pop .rg-seg[data-rg="modello"] button');
    if (b) {
      // index.html only lights up siblings: with the "Versioni precedenti" section the buttons sit in two containers
      const seg = b.closest(".rg-seg");
      setTimeout(() => { seg.querySelectorAll("button").forEach((x) => x.classList.toggle("on", x === b)); efforts(b.closest(".regola-pop")); }, 0);
    }
  }, true);

  new MutationObserver((ms) => {
    for (const m of ms) for (const n of m.addedNodes) if (n.nodeType === 1 && n.classList?.contains("regola-pop")) fix(n);
  }).observe(document.body, { childList: true });

  load();
  // the list changes rarely (new Claude Code version): re-read every 6 hours
  setInterval(load, 6 * 3600 * 1000);
  window.modelliClaude = () => LIST;
})();
