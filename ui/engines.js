// engines (Nov 12): chats with Codex (ChatGPT account) besides Claude. Everything is here; index.html only has the <script>.
// - New chat: the "Modello" row shows "ChatGPT · Codex" if Codex is installed; if sign-in is missing
//   it starts anyway but first offers "Entra con ChatGPT" (opens the browser, no terminal).
// - Codex chats have the "Codex" badge next to the name (chat header).
(function () {
  let engineState = null;
  async function loadState(force) {
    if (engineState && !force) return engineState;
    try { engineState = await invoke("motori_stato"); } catch (e) { engineState = { codex: { installato: false } }; }
    return engineState;
  }
  window.motoriEntraCodex = async function () {
    const r = await invoke("motori_entra_codex");
    if (r.ok) toast("Si e' aperto il browser: entra con il tuo account ChatGPT", 4000); else toast("Codex: " + r.msg, 4000);
    engineState = null;
  };
  // chip in the new chat window
  const origOpenNew = window.apriNuova;
  window.apriNuova = async function () {
    await origOpenNew.apply(this, arguments);
    const st = await loadState();
    const c = st?.codex; const box = $("#nuova-mod"); if (!box || !c?.installato) return;
    const hint = c.accesso ? "" : ' title="Serve l\'accesso con l\'account ChatGPT"';
    box.insertAdjacentHTML("beforeend", `<span class="sep-chip">ChatGPT</span><button class="chip" data-mclaude="codex"${hint}>Codex${c.accesso ? "" : " ·  entra"}</button>`);
  };
  const origStartNew = window.avviaNuova;
  window.avviaNuova = async function () {
    if ($("#nuova-mod .chip.on[data-mclaude]")?.dataset.mclaude !== "codex") return origStartNew.apply(this, arguments);
    const st = await loadState();
    if (!st?.codex?.accesso) { await window.motoriEntraCodex(); return; }
    const r = await invoke("nuova", { cartella: $("#nuova-cartella").value, testo: $("#nuova-testo").value, modello: "", effort: "", motore: "codex" });
    if (r.ok) { chiudiNuova(); await caricaElenco(); scegli(r.id); } else toast("Non avviata: " + r.msg, 4000);
  };
  // engine badge in the chat header (like the permissions label)
  // emergency (Nov 12): "careful" fallback chat and, on Claude chats stopped by the limit, the "Continua su Codex" button
  window.motoriTesta = (d) => {
    if (d && (d.motore === "codex" || d.agente === "codex")) return `<span class="etichetta-motore${d.prudente ? " prudente" : ""}" title="${d.prudente ? "Ripiego mentre Claude è fermo: niente azioni verso l'esterno senza il tuo ok" : "Questa chat usa Codex con l'account ChatGPT"}">Codex${d.prudente ? " · prudente" : ""}</span>`;
    if (d && d.limite_claude) return `<button class="tasto-ripiego" data-continua-codex="${esc(d.sessione)}" title="Claude ha finito la settimana: continua in una chat Codex nella stessa cartella, con un riassunto">Continua su Codex</button>`;
    return "";
  };
  document.addEventListener("click", async (e) => {
    const b = e.target.closest("[data-continua-codex]"); if (!b || b.disabled) return;
    b.disabled = true; b.textContent = "Apro Codex…";
    const r = await invoke("continua_su_codex", { id: b.dataset.continuaCodex });
    if (r.ok) { await caricaElenco(); scegli(r.id); toast("Chat Codex aperta in modalità prudente", 3000); }
    else { b.disabled = false; b.textContent = "Continua su Codex"; toast("Codex: " + r.msg, 4000); }
  });
  const css = document.createElement("style");
  css.textContent = `.etichetta-motore.prudente{border-color:var(--ink3);color:var(--ink)}.tasto-ripiego{margin-left:8px;font:500 12px var(--sans);padding:3px 10px;border-radius:9px;background:var(--sel);color:var(--selink);border:0;cursor:pointer;white-space:nowrap}.tasto-ripiego:disabled{opacity:.6}.etichetta-motore{font-size:11px;padding:1px 7px;border-radius:9px;border:1px solid var(--linea,#ddd);color:var(--grigio,#666);margin-left:6px;white-space:nowrap}`;
  document.head.appendChild(css);
})();
