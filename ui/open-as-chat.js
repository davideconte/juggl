// Open as chat (Nov 12, src-tauri/src/open_as_chat.rs): button on subagents, on the row in the list (shows up on
// hover or with the row selected) and in the subagent view, next to Stop.
// If the subagent is working it first asks "Fermo il sotto-agente e lo sposto in una chat?". Then the new chat opens right away.
// Already moved: the row keeps "in chat", and clicking opens that chat.
// Hooks in index.html (marked "apri come chat (12/11)"): the subagent row calls window.spostaRiga,
// apriSotto calls window.spostaVista, and the <script src="open-as-chat.js"> tag at the bottom.
(() => {
  const TT = window.__TAURI__;
  if (!TT) return;
  const inv = (c, a) => TT.core.invoke(c, a);
  const E = (s) => String(s ?? "").replace(/[&<>"']/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" }[c]));
  const notice = (t, ms) => (typeof toast === "function" ? toast(t, ms) : console.log(t));
  const ICON = '<svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round"><path d="M9.5 2.5h4v4M13.5 2.5 8 8M11.5 9.5v4h-9v-9h4"/></svg>';
  const CHAT_ICON = '<svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round"><path d="M3 3.5h10v7H7l-3 2.5v-2.5H3z"/></svg>';

  const css = `
.riga-sa{grid-template-columns:18px 20px 1fr auto auto}
.riga-sa .sa-sposta{display:none;align-items:center;gap:4px;height:19px;padding:0 7px;border-radius:6px;font-size:10.5px;font-weight:500;color:var(--ink2);background:var(--bg);border:1px solid var(--line);white-space:nowrap;cursor:pointer}
.riga-sa .sa-sposta svg{width:11px;height:11px;flex:none}
.riga-sa .sa-sposta:hover{color:var(--ink);border-color:var(--ink3)}
.riga-sa:hover .sa-sposta,.riga-sa.sel .sa-sposta,.riga-sa .sa-sposta.fatto{display:inline-flex}
.riga-sa:hover .st,.riga-sa.sel .st,.riga-sa:has(.sa-sposta.fatto) .st{display:none}
.riga-sa .sa-sposta.fatto{background:transparent;border-color:transparent;color:var(--ink3);padding:0 2px}
.riga-sa .sa-sposta.fatto:hover{color:var(--ink)}
.riga-sa .sa-sposta.va{opacity:.55;pointer-events:none}
#sposta-sa{margin-left:auto;display:inline-flex;align-items:center;gap:5px}
#sposta-sa svg{width:12px;height:12px}
#sposta-sa:not([hidden]) + #ferma-sa{margin-left:0}
`;
  const st = document.createElement("style"); st.id = "sposta-css"; st.textContent = css; document.head.appendChild(st);

  const D = () => { try { return dati || {}; } catch { return {}; } };
  /** Chat already created from this subagent (parent state: sotto_spostati). */
  const alreadyIn = (sess, aid) => {
    const s = (D().sessioni || []).find((x) => x.sessione === sess);
    return s?.sotto_spostati?.[aid]?.chat || "";
  };

  /** HTML snippet inside the subagent row (index.html, list). */
  window.spostaRiga = (s, x) => {
    const chat = s?.sotto_spostati?.[x.id]?.chat;
    const data_ = `data-sposta-sa="${E(x.id)}" data-sess="${E(s.sessione)}" data-nome="${E(x.descrizione || "")}" data-stato="${E(x.stato)}"`;
    return chat
      ? `<span class="sa-sposta fatto" role="button" ${data_} title="Questo sotto-agente continua in una chat: aprila">${CHAT_ICON}in chat</span>`
      : `<span class="sa-sposta" role="button" ${data_} title="Apri come chat: il lavoro continua in una chat vera, nella stessa cartella">${ICON}Apri come chat</span>`;
  };

  /** Button in the subagent view bar (index.html, apriSotto). */
  window.spostaVista = (info, aid, sess) => {
    const ferma = document.getElementById("ferma-sa");
    if (!ferma) return;
    let b = document.getElementById("sposta-sa");
    if (!b) {
      b = document.createElement("button");
      b.className = "tasto"; b.id = "sposta-sa";
      ferma.parentNode.insertBefore(b, ferma);
    }
    const chat = alreadyIn(sess, aid);
    b.hidden = !info?.id;
    b.innerHTML = chat ? `${CHAT_ICON}Apri la sua chat` : `${ICON}Apri come chat`;
    b.title = chat ? "Questo sotto-agente continua in una chat: aprila" : "Il lavoro continua in una chat vera, nella stessa cartella della chat madre";
    b.dataset.spostaSa = aid; b.dataset.sess = sess; b.dataset.nome = info?.descrizione || ""; b.dataset.stato = info?.stato || "";
  };

  let va = false;
  async function move(el) {
    if (va) return;
    const { spostaSa: aid, sess, nome, stato } = el.dataset;
    const alreadyMoved = alreadyIn(sess, aid);
    if (alreadyMoved && (D().sessioni || []).some((x) => x.sessione === alreadyMoved)) { if (typeof scegli === "function") scegli(alreadyMoved); return; }
    let ferma = false;
    const ask = () => (typeof conferma === "function"
      ? conferma("Fermo il sotto-agente e lo sposto in una chat?", `"${nome || "Il sotto-agente"}" si ferma subito, come con Ferma. Poi nasce una chat nella stessa cartella che riparte da quello che ha fatto, e la chat madre lo sa.`, "Ferma e sposta")
      : Promise.resolve(window.confirm("Fermo il sotto-agente e lo sposto in una chat?")));
    if (stato === "lavora") { if (!(await ask())) return; ferma = true; }
    va = true; el.classList.add("va");
    try {
      notice(ferma ? "Fermo il sotto-agente e preparo la chat…" : "Preparo la chat…", 20000);
      let r = await inv("sottoagente_in_chat", { id: sess, agente: aid, ferma }).catch((e) => ({ ok: false, msg: String(e) }));
      // it started working again meanwhile: confirmation needed
      if (!r?.ok && r?.conferma) {
        if (!(await ask())) { notice("Non spostato", 1600); return; }
        notice("Fermo il sotto-agente e preparo la chat…", 20000);
        r = await inv("sottoagente_in_chat", { id: sess, agente: aid, ferma: true }).catch((e) => ({ ok: false, msg: String(e) }));
      }
      if (!r?.ok) { notice("Non spostato: " + (r?.msg || "errore"), 4000); return; }
      notice(r.gia ? "Era già in una chat: la apro" : r.fermato ? "Fermato e spostato in una chat nuova" : "Spostato in una chat nuova", 2400);
      if (typeof caricaElenco === "function") await caricaElenco();
      if (typeof scegli === "function") scegli(r.id);
    } finally { va = false; el.classList.remove("va"); }
  }
  // before the click that opens the subagent (the whole row is a button)
  document.addEventListener("click", (e) => {
    const b = e.target.closest?.("[data-sposta-sa]");
    if (!b) return;
    e.preventDefault(); e.stopPropagation();
    move(b);
  }, true);
})();
