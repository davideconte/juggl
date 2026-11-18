// space (Nov 14, v2 for everyone): the "Spazio" part of the Mac view. Tauri command mac_spazio: personal engine
// (bin/mac in the data folder, where present) or built-in engine (disk_space.rs, everyone else: Trash only, everything with confirmation).
// It hooks into apriMac without touching its code: puts its box at the top of the grid and hides the old disk
// boxes. Memory, Claude, sessions and apps stay as before. Texts in Italian and English (Mac language).
(() => {
  const SP = { d: null, carica: false, occupato: "", aperti: new Set(), mostraTutte: new Set(), chiusi: new Set(JSON.parse(localStorage.getItem("spChiusi") || "[]")),
    menu: null, timer: null, svuotaArmato: 0, impApre: false, filtro: "tutte", scelte: new Set(), esito: null, lavoro: null };
  const IT = () => (SP.d?.lingua || navigator.language || "it").toLowerCase().startsWith("it");
  const tr = (it, en) => (IT() ? it : en);
  const GBs = (b) => { const g = (b || 0) / 1e9; return g < 0.001 ? tr("meno di 1 MB", "under 1 MB") : g < 0.1 ? Math.round(g * 1000) + " MB" : (g < 10 ? g.toFixed(1) : String(Math.round(g))).replace(".", IT() ? "," : ".") + " GB"; };
  const invokeSp = async (args) => { try { return await invoke("mac_spazio", { args }); } catch (e) { return { ok: false, msg: String(e) }; } };
  const builtIn = () => !!SP.d?.riepilogo?.conferma_sempre;
  const SAFE = () => ({ sicuro: [tr("Si rifà da sola", "Comes back"), "sicuro"], conferma: [tr("Da confermare", "Check first"), "conf"], personale: [tr("File tuoi", "Your files"), "pers"], segnala: [tr("Da guardare", "Just look"), "info"], mai: [tr("Protetto", "Protected"), "info"] });
  const AZ = () => ({ cancella: tr("Libera", "Free up"), cestino: tr("Nel Cestino", "Move to Trash"), sposta: tr("Sposta sull'archivio", "Move to archive"), lascia: tr("Lascia", "Keep"), apri: tr("Mostra nel Finder", "Show in Finder"), svuota: tr("Svuota", "Empty") });
  const runnable = (v) => ["cancella", "cestino", "sposta"].includes(v.azione) && v.sicurezza !== "mai" && v.eseguibile !== false && !v.lasciata;
  const dayCount = (g) => (g == null ? "" : g < 1 ? tr("toccata oggi", "changed today") : g < 2 ? tr("ferma da ieri", "untouched since yesterday") : tr(`ferma da ${Math.round(g)} giorni`, `untouched for ${Math.round(g)} days`));
  const millions = (n) => (n >= 1e6 ? String((n / 1e6).toFixed(1)).replace(".", IT() ? "," : ".") + tr(" milioni di file", " million files") : n >= 1000 ? Math.round(n / 1000) + tr(" mila file", "k files") : (n || 0) + tr(" file", " files"));
  const fmtSecs = (t) => (typeof faMac === "function" ? faMac(t) : "");

  // ---------- hook into the Mac view ----------
  const origOpenMac = window.apriMac;
  window.apriMac = async function () { const r = origOpenMac.apply(this, arguments); mount(); return r; };
  // without the personal engine the old scan does not exist: no "Scansione non riuscita" warning
  if (typeof window.scansionaMac === "function") {
    const sv = window.scansionaMac;
    window.scansionaMac = function () { if (typeof dati !== "undefined" && dati?.funzioni?.mac === false) return Promise.resolve(); return sv.apply(this, arguments); };
  }
  function mount() {
    if (typeof dati !== "undefined" && dati?.funzioni && dati.funzioni.mac === false && dati.funzioni.spazio !== true) return;
    const g = document.getElementById("mac-griglia"); if (!g || document.getElementById("mac-s-spazio")) return;
    g.classList.add("spazio-nuovo");
    const slot = document.createElement("div"); slot.className = "mac-slot pieno"; slot.id = "mac-s-spazio";
    g.insertBefore(slot, g.firstChild);
    if (SP.d) render(); else slot.innerHTML = `<section class="mac-card sp-testa"><div class="sp-num"><b>…</b></div><p class="sp-nota">${tr("Leggo lo spazio del Mac…", "Reading your Mac's storage…")}</p></section>`;
    load(false).then(() => { if (SP.d && !builtIn()) load(true); advance(); });   // avanza: resumes a background cleanup already started
    clearInterval(SP.timer);
    SP.timer = setInterval(() => { if (!vistaMac || !document.getElementById("mac-s-spazio")) return clearInterval(SP.timer); if (SP.d?.completa_in_corso || SP.lavoro) advance(); }, 1500);
  }
  async function load(refresh) {
    if (SP.carica && !refresh) return;
    SP.carica = true;
    const r = await invokeSp(refresh ? ["voci", "--aggiorna"] : ["voci"]);
    SP.carica = false;
    if (r && r.riepilogo) { SP.d = r; for (const id of [...SP.scelte]) if (!r.voci?.[id] || !runnable(r.voci[id])) SP.scelte.delete(id); render(); }
    else if (!SP.d && document.getElementById("mac-s-spazio")) document.getElementById("mac-s-spazio").innerHTML = `<section class="mac-card"><h4>${tr("Spazio", "Storage")}</h4><p class="sp-nota">${tr("Non riesco a leggere lo spazio", "Can't read storage")}: ${esc(r?.msg || "")}</p></section>`;
  }
  async function advance() {
    const a = await invokeSp(["avanza"]);
    if (!SP.d) return;
    // long background cleanup (personal engine): GB done out of the total, estimated time; at the end the result
    const l = a?.lavoro;
    if (l && l.vivo && l.stato === "in_corso") { SP.lavoro = l; SP.occupato = tr("pulizia in sfondo", "cleanup in the background"); render(); }
    else if (SP.lavoro) {
      SP.lavoro = null; SP.occupato = "";
      const lib = Math.max(0, (l?.libero_b || 0) - (l?.libero_inizio_b || 0));
      showOutcome(tr(`Pulizia finita: liberati ${GBs(lib)}`, `Cleanup finished: ${GBs(lib)} freed`));
      load(false);
    }
    if (a && a.vivo && a.stato === "in_corso") { SP.d.completa_in_corso = a; renderProgress(); }
    else if (SP.d.completa_in_corso) { SP.d.completa_in_corso = null; showOutcome(a?.stato === "interrotta" ? tr("Scansione fermata: tengo quello che ha misurato", "Scan stopped: keeping what it measured") : tr("Scansione completa finita", "Full scan finished")); load(false); }
  }

  // ---------- drawing ----------
  function render() {
    const el = document.getElementById("mac-s-spazio"); if (!el || !SP.d) return;
    const isEmpty = !SP.d.riepilogo.completa_ts && !SP.d.completa_in_corso && builtIn();
    const h = headHtml() + outcome() + (isEmpty ? welcome() : plan() + whereSection() + grows() + filters() + groups()) + trashBin() + history() + leftItems() + automatico() + protectedItems() + choicesBar();
    if (el._h !== h) { el.innerHTML = h; el._h = h; }
    renderProgress();
  }
  function headHtml() {
    const r = SP.d.riepilogo, lib = r.libero_b || 0, sum = r.totale_b || 1, ob = r.obiettivo_b || 100e9;
    const alarm = r.libero_pct != null && r.libero_pct < (r.soglia_pct || 15);
    const used = Math.max(0, sum - lib), safe = Math.min(r.sicuro_b || 0, used);
    const pUsed = ((used - safe) / sum) * 100, pSafe = (safe / sum) * 100, pOb = Math.max(0, Math.min(100, ((sum - ob) / sum) * 100));
    const comparison = r.completa_ts ? `${tr("Scansione completa", "Full scan")} ${fmtSecs(r.completa_ts)}${r.completa_file ? " · " + millions(r.completa_file) : ""}${r.completa_interrotta ? tr(" · fermata a metà", " · stopped halfway") : ""}` : tr("Nessuna scansione completa ancora", "No full scan yet");
    const busy = SP.occupato;
    const btnHtml = builtIn() ? tr(`Sposta ${GBs(r.sicuro_b)} nel Cestino`, `Move ${GBs(r.sicuro_b)} to Trash`) : tr(`Libera ${GBs(r.sicuro_b)} in sicurezza`, `Free up ${GBs(r.sicuro_b)} safely`);
    return `<section class="mac-card sp-testa ${alarm ? "allarme" : ""}">
      <div class="sp-riga1"><div class="sp-num"><b>${GBs(lib)}</b> ${tr("liberi", "free")} <small>${tr("su", "of")} ${GBs(sum)}${r.libero_pct != null ? " · " + String(r.libero_pct).replace(".", IT() ? "," : ".") + "%" : ""}</small></div>
        <div class="sp-ob">${tr("Obiettivo", "Goal")} <b>${GBs(ob)}</b> · ${r.manca_b > 0 ? tr("mancano ", "") + GBs(r.manca_b) + tr("", " to go") : tr("raggiunto", "reached")}<button class="sp-link" data-sp-imp>${tr("Impostazioni", "Settings")}</button></div></div>
      <div class="sp-barra" title="${tr("Usato · si rifà da solo · libero", "Used · comes back on its own · free")}"><i class="u" style="width:${pUsed}%"></i><i class="s" style="width:${pSafe}%"></i><span class="ob" style="left:${pOb}%"><em>${GBs(ob)}</em></span></div>
      ${SP.impApre ? settings() : ""}
      <div class="sp-azioni">
        <button class="btn-nero" id="sp-sicuro" ${busy || !(r.sicuro_b > 5e7) ? "disabled" : ""}>${busy ? tr("Lavoro…", "Working…") : r.sicuro_b > 5e7 ? btnHtml : tr("Niente che si rifà da solo", "Nothing that comes back on its own")}</button>
        ${SP.d.completa_in_corso ? "" : `<button class="tasto bordo" id="sp-completa" ${busy ? "disabled" : ""}>${r.completa_ts ? tr("Riscansiona il disco", "Scan again") : tr("Scansiona il disco", "Scan the disk")}</button>`}
        <span class="sp-quando">${esc(comparison)}</span>
      </div>
      <div class="sp-avanza" id="sp-avanza"></div>
      ${jobHtml()}
      ${busy && !SP.lavoro ? `<p class="sp-nota sp-occ">${tr("Sto lavorando", "Working")}: ${esc(busy)}…</p>` : ""}
      ${trend()}
      ${(r.cartelle_negate || []).length ? `<p class="sp-nota">${tr("Non lette (permesso negato da macOS)", "Not read (macOS denied access)")}: ${esc(r.cartelle_negate.join(", "))}. ${tr("Si concede in Impostazioni di Sistema > Privacy e sicurezza > File e cartelle.", "You can allow it in System Settings > Privacy & Security > Files and Folders.")}</p>` : ""}
      ${(SP.d.build_in_corso || []).length ? `<p class="sp-nota">${tr("C'è una build in corso: le build si toccano solo quando finisce.", "A build is running: builds are only touched when it's done.")}</p>` : ""}
    </section>`;
  }
  function settings() {
    const r = SP.d.riepilogo, a = SP.d.automatico;
    return `<div class="sp-imp">
      <label>${tr("Obiettivo spazio libero", "Free space goal")} <input type="number" min="10" max="400" step="5" id="sp-imp-ob" value="${Math.round((r.obiettivo_b || 100e9) / 1e9)}"> GB</label>
      <label>${tr("Avviso sotto il", "Warn below")} <input type="number" min="3" max="40" step="1" id="sp-imp-pct" value="${r.soglia_pct || 15}"> %</label>
      ${builtIn() ? `<label><input type="checkbox" id="sp-imp-avviso" ${r.avviso_disco ? "checked" : ""}> ${tr("Avvisami quando il disco scende sotto la soglia", "Notify me when the disk drops below this")}</label>` : ""}
      ${a ? `<label><input type="checkbox" id="sp-imp-auto" ${a.attiva !== false ? "checked" : ""}> ${tr("Pulizia automatica delle build delle chat ferme da un giorno", "Automatic cleanup of chat builds idle for a day")}</label>` : ""}
      <button class="tasto bordo" id="sp-imp-salva">${tr("Salva", "Save")}</button></div>`;
  }
  function jobHtml() {
    const l = SP.lavoro; if (!l) return "";
    const pct = l.totale_b ? Math.min(99, Math.round((l.fatto_b / l.totale_b) * 100)) : 0;
    const etaText = l.eta_s != null ? (l.eta_s > 90 ? tr(`circa ${Math.round(l.eta_s / 60)} min`, `about ${Math.round(l.eta_s / 60)} min`) : tr("meno di 2 min", "under 2 min")) : "";
    return `<div class="sp-avanza on"><div class="mac-barra sottile"><i style="width:${Math.max(2, pct)}%"></i></div>
      <div class="sp-av-r"><small>${tr("Pulizia in sfondo", "Cleanup in the background")}: ${GBs(l.fatto_b)} ${tr("di", "of")} ${GBs(l.totale_b)}${etaText ? " · " + etaText : ""}${l.voce ? " · " + esc(l.voce) : ""}. ${tr("Puoi chiudere questa vista: ti avviso in Juggl quando finisce.", "You can close this view: Juggl will tell you when it's done.")}</small></div></div>`;
  }
  function renderProgress() {
    const box = document.getElementById("sp-avanza"); if (!box) return;
    const a = SP.d?.completa_in_corso;
    if (!a) { box.classList.remove("on"); box.innerHTML = ""; return; }
    const pct = a.pct ?? 0;
    box.classList.add("on");
    const h = `<div class="mac-barra sottile"><i style="width:${Math.max(2, pct)}%"></i></div>
      <div class="sp-av-r"><small>${tr("Scansione", "Scanning")} ${pct}% · ${millions(a.file || 0)}${a.dove ? " · " + esc(a.dove) : ""}. ${tr("Legge solo nomi e dimensioni, salta iCloud, Dropbox e i file non scaricati.", "Reads only names and sizes, skips iCloud, Dropbox and files not downloaded.")}</small>
      <button class="tasto" id="sp-ferma">${tr("Ferma", "Stop")}</button></div>`;
    if (box._h !== h) { box.innerHTML = h; box._h = h; }
  }
  function trend() {
    const l = SP.d.andamento || [];
    if (l.length < 3) return "";
    const W = 600, Hh = 40, t0 = l[0].ts, t1 = l[l.length - 1].ts, vs = l.map((x) => x.libero_b), mx = Math.max(...vs), mn = Math.min(...vs);
    const x = (t) => ((t - t0) / Math.max(1, t1 - t0)) * W, y = (v) => Hh - 4 - ((v - mn) / Math.max(1, mx - mn)) * (Hh - 10);
    const pts = l.map((p) => `${x(p.ts).toFixed(1)},${y(p.libero_b).toFixed(1)}`).join(" ");
    const prevDay = l.find((p) => p.ts >= t1 - 86400) || l[0], d = l[l.length - 1].libero_b - prevDay.libero_b;
    return `<div class="sp-and"><svg viewBox="0 0 ${W} ${Hh}" preserveAspectRatio="none"><polyline points="${pts}" fill="none" stroke="currentColor" stroke-width="1.6" vector-effect="non-scaling-stroke"/></svg>
      <small>${tr("Spazio libero negli ultimi giorni", "Free space over the last days")}: ${GBs(mn)}–${GBs(mx)}${Math.abs(d) > 1e9 ? ` · ${d < 0 ? "−" : "+"}${GBs(Math.abs(d))} ${tr("nelle ultime 24 ore", "in the last 24 hours")}` : ""}</small></div>`;
  }
  function outcome() {
    const e = SP.esito; if (!e || Date.now() - e.t > 60000) return "";
    return `<section class="mac-card sp-esito ${e.errore ? "err" : ""}"><span>${esc(e.testo)}</span>${e.n ? `<button class="tasto bordo" data-sp-annulla="${esc(e.n)}">${tr("Annulla", "Undo")}</button>` : ""}<button class="tasto sp-x" data-sp-esito-chiudi title="${tr("Chiudi", "Close")}">×</button></section>`;
  }
  function showOutcome(testo, n, errore) { SP.esito = { testo, n, errore, t: Date.now() }; render(); setTimeout(render, 60500); }
  function welcome() {
    return `<section class="mac-card sp-vuoto"><h3>${tr("Trova cosa occupa il disco", "Find what's filling your disk")}</h3>
      <p>${tr("Juggl guarda solo nomi e dimensioni dei file nella tua cartella. Propone soltanto cose che si rifanno da sole (build, cache, librerie dei progetti) e le sposta nel Cestino, sempre dopo averti chiesto. I file grandi te li mostra soltanto: decidi tu.", "Juggl only looks at file names and sizes in your folder. It only suggests things that come back on their own (builds, caches, project libraries) and moves them to the Trash, always after asking. Large files are only shown to you: you decide.")}</p>
      <p class="sp-nota">${tr("macOS può chiederti il permesso di leggere Scrivania, Documenti e Download: Juggl guarda solo nomi e dimensioni. Se dici di no, quelle cartelle vengono saltate.", "macOS may ask permission to read Desktop, Documents and Downloads: Juggl only looks at names and sizes. If you say no, those folders are skipped.")}</p>
      <button class="btn-nero" id="sp-completa">${tr("Scansiona il disco", "Scan the disk")}</button></section>`;
  }
  function plan() {
    const r = SP.d.riepilogo, ob = r.obiettivo_b || 0, lib = r.libero_b || 0;
    if (!(ob > lib)) return "";
    const steps = [];
    // built-in engine: things go to the Trash first, the space comes back by emptying it
    if (r.sicuro_b > 5e7) steps.push({ t: builtIn() ? tr("Sposta nel Cestino ciò che si rifà da solo", "Move what comes back on its own to the Trash") : tr("Libera ciò che si rifà da solo", "Free up what comes back on its own"), b: builtIn() ? 0 : r.sicuro_b, id: "sp-sicuro", nota: builtIn() ? tr(`${GBs(r.sicuro_b)}: lo spazio torna quando svuoti il Cestino`, `${GBs(r.sicuro_b)}: space comes back when you empty the Trash`) : "" });
    const trash = (r.cestino_b || 0) + (builtIn() ? r.sicuro_b || 0 : 0);
    if (trash > 5e7) steps.push({ t: tr("Svuota il Cestino", "Empty the Trash"), b: trash, id: "sp-svuota" });
    if (r.scelte_b > 5e7) steps.push({ t: tr("Scegli tra le voci da confermare", "Pick from the items to check"), b: r.scelte_b, id: "sp-filtro-conf" });
    if (!steps.length) return "";
    let cumulative = lib;
    const rows = steps.map((p, i) => {
      cumulative += p.b;
      const ok = cumulative >= ob;
      return `<div class="sp-passo"><span class="n">${i + 1}</span><span class="t">${esc(p.t)}<small>${p.nota || `+${GBs(p.b)} → ${GBs(cumulative)} ${tr("liberi", "free")}${ok ? tr(" · obiettivo raggiunto", " · goal reached") : ""}`}</small></span><button class="tasto bordo" data-sp-passo="${p.id}">${tr("Vai", "Go")}</button></div>`;
    }).join("");
    return `<section class="mac-card sp-piano"><div class="sp-gt"><h4>${tr("Verso", "Towards")} ${GBs(ob)} ${tr("liberi", "free")}</h4><span class="sp-ggb">${tr("mancano ", "")}${GBs(ob - lib)}${tr("", " to go")}</span></div>${rows}
      ${cumulative < ob ? `<p class="sp-nota">${tr("Anche così non si arriva all'obiettivo: guarda i file grandi qui sotto.", "Even so the goal isn't reached: look at the large files below.")}</p>` : ""}</section>`;
  }
  function whereSection() {
    const gs = (SP.d.gruppi || []).filter((g) => g.id !== "cestino" && g.byte > 5e7);
    const r = SP.d.riepilogo; const sum = gs.reduce((s, g) => s + g.byte, 0) + (r.cestino_b > 5e7 ? r.cestino_b : 0);
    if (!sum) return "";
    const parts = gs.map((g, i) => ({ id: g.id, nome: g.nome, b: g.byte, c: "g" + Math.min(i, 5) }));
    if (r.cestino_b > 5e7) parts.push({ id: "cestino", nome: tr("Cestino", "Trash"), b: r.cestino_b, c: "g6" });
    return `<section class="mac-card sp-dove"><h4>${tr("Dove si può recuperare spazio", "Where space can be recovered")}</h4>
      <div class="mac-seg">${parts.map((p) => `<i class="${p.c}" style="width:${(p.b / sum) * 100}%" title="${esc(p.nome)}: ${GBs(p.b)}"></i>`).join("")}</div>
      <div class="mac-legenda">${parts.map((p) => `<button class="sp-leg" data-sp-vai="${esc(p.id)}"><i class="${p.c}"></i>${esc(p.nome)} <b>${GBs(p.b)}</b></button>`).join("")}</div></section>`;
  }
  function ownerChip(c) {
    if (!c) return "";
    if (c.tipo === "chat" && c.sessione) return `<button class="sp-chi" data-sp-chat="${esc(c.sessione)}" title="${tr("Apri la chat", "Open the chat")}">${esc(c.nome)}${c.attiva ? tr(" · al lavoro", " · working") : ""}</button>`;
    return `<span class="sp-chi muto">${esc(c.nome)}</span>`;
  }
  function grows() {
    const l = (SP.d.cresce || []).slice(0, 6); if (!l.length) return "";
    return `<section class="mac-card sp-cresce"><h4>${tr("Cresce in fretta", "Growing fast")}</h4>${l.map((c) => `<div class="sp-cr"><b>+${GBs(c.delta_b)}</b><span class="t">${esc(c.corto.split("/").pop())}<small>${esc(c.corto)} · ${c.ore} ${tr("ore", "h")}${c.nuova ? tr(" · nuova", " · new") : ""}</small></span>${ownerChip(c.chi)}</div>`).join("")}</section>`;
  }
  const FILTERS = () => [["tutte", tr("Tutte", "All")], ["sicure", tr("Si rifanno da sole", "Come back")], ["conf", tr("Da confermare", "Check first")], ["guardare", tr("Da guardare", "Just look")]];
  const passesFilter = (v, f = SP.filtro) => f === "tutte" || (f === "sicure" && v.sicurezza === "sicuro") || (f === "conf" && ["conferma", "personale"].includes(v.sicurezza)) || (f === "guardare" && v.sicurezza === "segnala");
  function filters() {
    const V = Object.values(SP.d.voci || {}).filter((v) => !v.lasciata && v.sicurezza !== "mai" && v.gruppo !== "cestino");
    if (V.length < 4) return "";
    return `<div class="sp-filtri">${FILTERS().map(([k, t]) => { const n = V.filter((v) => passesFilter(v, k)).length; return n || k === "tutte" ? `<button class="sp-chip ${SP.filtro === k ? "on" : ""}" data-sp-filtro="${k}">${esc(t)} <small>${n}</small></button>` : ""; }).join("")}
      ${V.some((v) => v.sicurezza === "sicuro" && runnable(v)) ? `<button class="sp-link" data-sp-scegli-tutte>${tr("Seleziona quelle che si rifanno", "Select the ones that come back")}</button>` : ""}</div>`;
  }
  function actionBtn(v, mode, main) {
    const ssf = SP.d.riepilogo.archivio_ok ?? SP.d.riepilogo.ssf;
    let et = AZ()[mode] || mode;
    if (mode === "cancella" && v.sicurezza !== "sicuro") et = tr("Cancella", "Delete");
    const required = (builtIn() || v.sicurezza !== "sicuro") && !["lascia", "apri"].includes(mode);
    const no = SP.occupato || (mode === "sposta" && !ssf) || (v.via === "libera" && v.eseguibile === false && mode !== "lascia");
    const pf = mode === "apri" ? `data-sp-finder="${esc(v.elementi?.[0]?.percorso || "")}"` : `data-sp-fai="${esc(v.id)}" data-modo="${mode}"`;
    return `<button class="tasto ${main ? "bordo" : ""}" ${pf} ${no ? "disabled" : ""}>${et}${required ? "…" : ""}</button>`;
  }
  function entry(v) {
    const [et, cls] = SAFE()[v.sicurezza] || ["", ""];
    const el = v.elementi || [], ap = SP.aperti.has(v.id), single = el.length === 1 ? el[0] : null;
    const meta = [ownerChip(v.chi), v.eta_giorni != null ? esc(dayCount(v.eta_giorni)) : "", single?.percorso ? `<span class="sp-p">${esc(corto(single.percorso))}</span>` : ""].filter(Boolean).join('<i class="sp-sep">·</i>');
    const altRow = (v.alternative || []).filter((m) => m !== v.azione);
    const primary = v.azione && AZ()[v.azione] ? actionBtn(v, v.azione, true) : "";
    const sel = runnable(v);
    return `<div class="sp-voce ${SP.scelte.has(v.id) ? "scelta" : ""}" id="sp-v-${esc(v.id)}">
      <div class="sp-v1">${sel ? `<label class="sp-ck"><input type="checkbox" data-sp-scegli="${esc(v.id)}" ${SP.scelte.has(v.id) ? "checked" : ""} aria-label="${tr("Seleziona", "Select")}"></label>` : `<span class="sp-ck"></span>`}
        <span class="sp-tit">${esc(v.titolo)}</span><span class="ms-et ${cls}">${et}</span>${v.cresce_b ? `<span class="sp-cr-et">+${GBs(v.cresce_b)}</span>` : ""}
        <span class="sp-gb">${v.byte > 0 ? (v.stima_massima ? tr("fino a ", "up to ") : "") + GBs(v.byte) : ""}</span>${primary}${altRow.length || (single?.percorso && v.azione !== "apri") ? `<button class="tasto sp-piu" data-sp-menu="${esc(v.id)}" title="${tr("Altre azioni", "More")}"><svg viewBox="0 0 16 16" fill="currentColor"><circle cx="3.5" cy="8" r="1.3"/><circle cx="8" cy="8" r="1.3"/><circle cx="12.5" cy="8" r="1.3"/></svg></button>` : ""}</div>
      ${v.cos_e ? `<p class="sp-v2">${esc(v.cos_e)}</p>` : ""}
      ${v.testo_manuale ? `<p class="sp-v2 man">${esc(v.testo_manuale)}</p>` : ""}
      ${meta ? `<div class="sp-v3">${meta}</div>` : ""}
      ${v.motivo ? `<div class="sp-v3 mot">${esc(v.motivo.charAt(0).toUpperCase() + v.motivo.slice(1))}</div>` : ""}
      ${SP.menu === v.id ? `<div class="sp-menu">${altRow.map((m) => actionBtn(v, m, false)).join("")}${single?.percorso && v.azione !== "apri" ? `<button class="tasto" data-sp-finder="${esc(single.percorso)}">${AZ().apri}</button>` : ""}</div>` : ""}
      ${el.length > 1 ? `<button class="ms-apri sp-ind" data-sp-apri="${esc(v.id)}">${ap ? tr("Nascondi", "Hide") : tr("Mostra", "Show")} ${v.elementi_totali > el.length ? v.elementi_totali : el.length} ${tr("elementi", "items")}</button>${ap ? `<div class="ms-el sp-ind">${el.slice(0, 40).map((e) => `<div class="ms-r"><span class="t">${esc((e.percorso || "").split("/").pop() || e.nome || (e.versione ? "iOS " + e.versione : ""))}<small>${esc(corto(e.percorso || ""))}</small></span><span class="mb">${e.byte > 0 ? GBs(e.byte) : ""}</span></div>`).join("")}</div>` : ""}` : ""}
    </div>`;
  }
  function groups() {
    const V = SP.d.voci || {};
    const html = (SP.d.gruppi || []).filter((g) => g.id !== "cestino").map((g) => {
      const vs = g.voci.map((id) => V[id]).filter((v) => v && passesFilter(v));
      if (!vs.length) return "";
      const closed = SP.chiusi.has(g.id), allItems = SP.mostraTutte.has(g.id), max = 5, vis = allItems ? vs : vs.slice(0, max);
      return `<section class="mac-card sp-gruppo" id="sp-g-${esc(g.id)}">
        <button class="sp-gt sp-gt-btn" data-sp-chiudi="${esc(g.id)}" aria-expanded="${!closed}"><svg class="sp-fr ${closed ? "" : "giu"}" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.6"><path d="m6 4 4 4-4 4"/></svg><h4>${esc(g.nome)}</h4><span class="sp-n">${vs.length}</span><span class="sp-ggb">${g.byte > 5e7 ? GBs(g.byte) : ""}</span></button>
        ${closed ? "" : `<p class="sp-gsp">${esc(g.spiegazione)}</p>${vis.map(entry).join("")}
        ${vs.length > max ? `<button class="ms-apri" data-sp-tutte="${esc(g.id)}">${allItems ? tr("Mostra meno", "Show less") : tr(`Mostra altre ${vs.length - max}`, `Show ${vs.length - max} more`)}</button>` : ""}`}
      </section>`;
    }).join("");
    return html || `<section class="mac-card"><p class="sp-nota">${tr("Niente in questo filtro.", "Nothing in this filter.")}</p></section>`;
  }
  function trashBin() {
    const r = SP.d.riepilogo; if (!(r.cestino_b > 1e7)) return "";
    const armed = Date.now() - SP.svuotaArmato < 5000;
    return `<section class="mac-card sp-cest" id="sp-g-cestino"><div class="sp-gt"><h4>${tr("Cestino", "Trash")}</h4><span class="sp-ggb">${GBs(r.cestino_b)}</span></div>
      <p class="sp-gsp">${tr("Quello che è nel Cestino occupa ancora spazio: il disco si libera solo svuotandolo. Finché non lo svuoti si recupera tutto con Annulla.", "What's in the Trash still takes space: the disk is freed only when you empty it. Until then everything can be restored with Undo.")}</p>
      <div class="sp-azioni"><button class="tasto bordo ${armed ? "pericolo" : ""}" id="sp-svuota" ${SP.occupato ? "disabled" : ""}>${armed ? tr(`Tocca ancora per svuotare ${GBs(r.cestino_b)}`, `Tap again to empty ${GBs(r.cestino_b)}`) : tr("Svuota il Cestino", "Empty the Trash")}</button>
      <button class="tasto" id="sp-svuota-7" ${SP.occupato ? "disabled" : ""}>${tr("Solo le cose di oltre 7 giorni…", "Only items older than 7 days…")}</button></div></section>`;
  }
  const MODES = () => ({ cancella: tr("cancellato", "deleted"), cestino: tr("nel Cestino", "to Trash"), sposta: tr("in archivio", "to archive"), lascia: tr("lasciato", "kept"), svuota: tr("Cestino svuotato", "Trash emptied") });
  function history() {
    const l = (SP.d.storico || []).slice(0, 10); if (!l.length) return "";
    const ap = SP.aperti.has("_storico");
    return `<section class="mac-card mac-protetti"><button class="mac-chiuso" data-sp-apri="_storico"><span>${tr("Cosa è stato fatto", "What was done")}</span><small>${l.length} ${tr("azioni, con Annulla dove si può", "actions, with Undo where possible")}</small><svg class="${ap ? "giu" : ""}" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.6"><path d="m6 4 4 4-4 4"/></svg></button>
      ${ap ? l.map((x) => {
        const b = x.liberato_b > 1e7 ? x.liberato_b : x.byte;
        const es = x.esito === "ok" || !x.esito ? "" : x.esito === "parziale" ? tr(" · in parte", " · partly") : tr(" · non fatto", " · not done");
        return `<div class="mac-voce due"><span class="t">${esc(x.titolo || "")}<small>${fmtSecs(x.ts)}${x.automatica ? tr(" · da sola", " · automatic") : ""}${x.modo && MODES()[x.modo] ? " · " + MODES()[x.modo] : ""}${x.chi ? " · " + esc(x.chi) : ""}${es}${x.annullato ? tr(" · annullato", " · undone") : ""}</small></span><span class="mb">${b > 1e7 ? GBs(b) : ""}</span>${x.annullabile ? `<button class="tasto" data-sp-annulla="${esc(x.n)}">${tr("Annulla", "Undo")}</button>` : ""}</div>`;
      }).join("") : ""}</section>`;
  }
  function leftItems() {
    const V = SP.d.voci || {}, l = (SP.d.lasciate || []).map((id) => V[id]).filter(Boolean); if (!l.length) return "";
    const ap = SP.aperti.has("_lasciate");
    return `<section class="mac-card mac-protetti"><button class="mac-chiuso" data-sp-apri="_lasciate"><span>${tr("Lasciate", "Kept")}</span><small>${l.length} · ${GBs(l.reduce((s, v) => s + (v.byte || 0), 0))}</small><svg class="${ap ? "giu" : ""}" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.6"><path d="m6 4 4 4-4 4"/></svg></button>
      ${ap ? `<div class="ms-el">${l.map((v) => `<div class="ms-r"><span class="t">${esc(v.titolo)}<small>${esc(corto(v.elementi?.[0]?.percorso || ""))}</small></span><span class="mb">${GBs(v.byte)}</span><button class="tasto" data-sp-riproponi="${esc(v.id)}">${tr("Riproponi", "Suggest again")}</button></div>`).join("")}</div>` : ""}</section>`;
  }
  function automatico() {
    const a = SP.d.automatico; if (!a) return "";
    const tolto = a.tolto_48h_b || 0;
    return `<section class="mac-card sp-auto"><div class="sp-gt"><h4>${tr("Pulizia automatica", "Automatic cleanup")}</h4><span class="sp-ggb">${a.attiva === false ? tr("spenta", "off") : tr("attiva", "on")}</span></div>
      <p class="sp-gsp">${tolto > 5e7 ? tr(`Nelle ultime 48 ore ha tolto da sola ${GBs(tolto)} di build e cache ferme.`, `In the last 48 hours it removed ${GBs(tolto)} of idle builds and caches.`) : tr("Nelle ultime 48 ore non ha dovuto togliere niente.", "Nothing to remove in the last 48 hours.")} ${tr("Tocca solo build delle chat ferme da un giorno, mai i tuoi file, mai durante una build.", "Only touches chat builds idle for a day, never your files, never during a build.")}</p></section>`;
  }
  function protectedItems() {
    const V = SP.d.voci || {}, neverList = (SP.d.protetti || []).map((id) => V[id]).filter(Boolean), fixed = SP.d.protetti_fissi || [];
    if (!neverList.length && !fixed.length) return "";
    const ap = SP.aperti.has("_protetti");
    return `<section class="mac-card mac-protetti"><button class="mac-chiuso" data-sp-apri="_protetti"><span>${tr("Protetti", "Protected")}</span><small>${neverList.length + fixed.length} ${tr("cose che non si toccano mai", "things never touched")}</small><svg class="${ap ? "giu" : ""}" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.6"><path d="m6 4 4 4-4 4"/></svg></button>
      ${ap ? `<div class="ms-el">${neverList.map((v) => `<div class="ms-r"><span class="t">${esc(v.titolo)}<small>${esc(v.motivo || v.cos_e || "")}</small></span><span class="mb">${v.byte > 0 ? GBs(v.byte) : ""}</span></div>`).join("")}${fixed.map((x) => `<div class="ms-r"><span class="t">${esc(corto(x.percorso))}<small>${esc(x.motivo)}</small></span></div>`).join("")}</div>` : ""}</section>`;
  }
  function choicesBar() {
    const V = SP.d.voci || {}, l = [...SP.scelte].map((id) => V[id]).filter(Boolean);
    if (!l.length) return "";
    const b = l.reduce((s, v) => s + (v.byte || 0), 0);
    const onlyHtml = builtIn() || l.every((v) => v.azione === "cestino");
    return `<div class="sp-barra-scelte"><span><b>${l.length}</b> ${tr(l.length === 1 ? "voce" : "voci", l.length === 1 ? "item" : "items")} · <b>${GBs(b)}</b></span>
      <button class="tasto" data-sp-deseleziona>${tr("Deseleziona", "Clear")}</button>
      <button class="btn-nero" data-sp-fai-scelte ${SP.occupato ? "disabled" : ""}>${onlyHtml ? tr("Sposta nel Cestino…", "Move to Trash…") : tr("Esegui le azioni consigliate…", "Run suggested actions…")}</button></div>`;
  }

  // ---------- actions ----------
  async function busyWork(name, fn) {
    if (SP.occupato) { toast(tr("Aspetta: sto ancora lavorando su ", "Wait: still working on ") + SP.occupato); return null; }
    SP.occupato = name; render();
    let r; try { r = await fn(); } catch (e) { r = { ok: false, msg: String(e) }; }
    SP.occupato = ""; SP.menu = null;
    await load(false);
    if (!builtIn()) { setTimeout(() => load(true), 400); if (typeof scansionaMac === "function") scansionaMac(false); }
    return r;
  }
  function summary(vs, mode) {
    const sum = vs.reduce((s, v) => s + (v.byte || 0), 0);
    const rows = vs.slice(0, 6).map((v) => `• ${v.titolo}, ${GBs(v.byte)}${v.elementi?.[0]?.percorso ? " (" + corto(v.elementi[0].percorso) + ")" : ""}`).join("\n") + (vs.length > 6 ? `\n${tr("e altre", "and")} ${vs.length - 6}${tr("", " more")}` : "");
    const afterEl = {
      cancella: tr("Si cancella per sempre, ma si rifà da sola quando serve.", "It's deleted for good, but comes back on its own when needed."),
      cestino: tr("Va nel Cestino: si recupera con Annulla. Lo spazio torna quando svuoti il Cestino.", "It goes to the Trash: restore it with Undo. Space comes back when you empty the Trash."),
      sposta: tr("La copio sul disco dell'archivio, controllo che sia identica, poi tolgo l'originale e lascio un collegamento. Si rimette con Annulla.", "Copied to the archive disk, verified, then the original is replaced by a link. Undo puts it back."),
      consigliata: tr("Le build e le cache che si rifanno vengono tolte, il resto va nel Cestino (si recupera con Annulla).", "Builds and caches that come back are removed, the rest goes to the Trash (restore with Undo)."),
    }[mode] || "";
    const warnings = vs.filter((v) => v.motivo).slice(0, 3).map((v) => `${v.titolo}: ${v.motivo}`).join("\n");
    return `${tr("Totale", "Total")} ${GBs(sum)}\n${rows}\n\n${afterEl}${warnings ? "\n\n" + tr("Attenzione", "Note") + ":\n" + warnings : ""}`;
  }
  function showResult(r, mode, v) {
    if (!r) return;
    if (!r.ok) return showOutcome(tr("Non fatto", "Not done") + ": " + (r.msg || tr("errore", "error")), null, true);
    const g = r.liberato_b || 0;
    const testo = mode === "lascia" ? tr("La lascio: non te la propongo più", "Kept: won't suggest it again")
      : mode === "cestino" || r.nel_cestino_b ? tr(`Nel Cestino ${GBs(r.nel_cestino_b || v?.byte)}: lo spazio torna svuotandolo`, `${GBs(r.nel_cestino_b || v?.byte)} moved to Trash: empty it to get the space back`)
      : mode === "sposta" ? tr(`Spostato in archivio: ${GBs(v?.byte)} liberi`, `Moved to archive: ${GBs(v?.byte)} freed`)
      : g > 1e7 ? tr(`Liberati ${GBs(g)}`, `${GBs(g)} freed`) : tr("Fatto", "Done");
    showOutcome(testo + (r.msg ? ` · ${r.msg}` : ""), ["cestino", "sposta", "lascia"].includes(mode) || r.nel_cestino_b ? r.n : null);
  }
  async function doIt(id, mode) {
    const v = SP.d?.voci?.[id]; if (!v) return;
    const required = (builtIn() || v.sicurezza !== "sicuro") && mode !== "lascia";
    if (required) {
      const et = { cancella: tr("Cancella", "Delete"), cestino: tr("Sposta nel Cestino", "Move to Trash"), sposta: tr("Sposta sull'archivio", "Move to archive") }[mode] || tr("Conferma", "Confirm");
      if (!(await conferma(`${v.titolo}?`, summary([v], mode), et))) return;
    }
    const args = ["fai", id, "--modo", mode];
    if (required) args.push("--conferma");
    showResult(await busyWork(v.titolo, () => invokeSp(args)), mode, v);
  }
  async function doChoices() {
    const V = SP.d.voci || {}, vs = [...SP.scelte].map((id) => V[id]).filter(Boolean); if (!vs.length) return;
    const mode = builtIn() || vs.every((v) => v.azione === "cestino") ? "cestino" : "consigliata";
    if (!(await conferma(tr(`${vs.length} voci?`, `${vs.length} items?`), summary(vs, mode), mode === "cestino" ? tr("Sposta nel Cestino", "Move to Trash") : tr("Esegui", "Run")))) return;
    const r = await busyWork(tr(`${vs.length} voci`, `${vs.length} items`), () => invokeSp(["fai-molti", "--modo", mode, "--conferma", ...vs.map((v) => v.id)]));
    if (r?.ok) SP.scelte.clear();
    showResult(r, mode === "cestino" ? "cestino" : "cancella");
  }
  async function freeSafe() {
    const r0 = SP.d?.riepilogo, g = r0?.sicuro_b || 0; if (!(g > 5e7)) return;
    const vs = Object.values(SP.d.voci || {}).filter((v) => v.sicurezza === "sicuro" && ["cancella", "cestino"].includes(v.azione) && !v.lasciata);
    if (builtIn() && !(await conferma(tr(`Spostare ${GBs(g)} nel Cestino?`, `Move ${GBs(g)} to Trash?`), summary(vs, "cestino"), tr("Sposta nel Cestino", "Move to Trash")))) return;
    const r = await busyWork(GBs(g), () => invokeSp(builtIn() ? ["libera-sicuro", "--conferma"] : ["libera-sicuro", "--sfondo"]));
    if (!r) return;
    if (r.in_sfondo) { SP.lavoro = r.lavoro || { fatto_b: 0, totale_b: g }; SP.occupato = tr("pulizia in sfondo", "cleanup in the background"); render(); return advance(); }
    const skippedN = (r.saltati || []).length;
    if (!r.ok) return showOutcome(tr("Non fatto", "Not done") + ": " + (r.msg || ""), null, true);
    showOutcome((builtIn() ? tr(`Nel Cestino ${GBs(r.nel_cestino_b)}: svuotalo per riavere lo spazio`, `${GBs(r.nel_cestino_b)} moved to Trash: empty it to get the space back`) : tr(`Liberati ${GBs(Math.max(0, r.liberato_b || 0))}`, `${GBs(Math.max(0, r.liberato_b || 0))} freed`))
      + (skippedN ? tr(` · ${skippedN} voci saltate${r.build_in_corso ? " (build in corso)" : ""}`, ` · ${skippedN} skipped${r.build_in_corso ? " (build running)" : ""}`) : ""), builtIn() ? r.n : null);
  }
  async function emptyTrash(dayCount) {
    const r0 = SP.d.riepilogo;
    if (!dayCount && Date.now() - SP.svuotaArmato > 5000) { SP.svuotaArmato = Date.now(); render(); setTimeout(render, 5100); return; }
    SP.svuotaArmato = 0;
    const ok = await conferma(dayCount ? tr("Svuotare le cose nel Cestino da oltre 7 giorni?", "Empty items in the Trash older than 7 days?") : tr(`Svuotare il Cestino (${GBs(r0.cestino_b)})?`, `Empty the Trash (${GBs(r0.cestino_b)})?`),
      tr("Si cancellano per sempre: dopo non si possono più recuperare con Annulla.", "They're deleted for good: Undo won't work afterwards."), tr("Svuota", "Empty"));
    if (!ok) return render();
    const r = await busyWork(tr("svuoto il Cestino", "emptying the Trash"), () => invokeSp(dayCount ? ["svuota-cestino", "--conferma", "--giorni", "7"] : ["svuota-cestino", "--conferma"]));
    if (r) showOutcome(r.ok ? tr(`Cestino svuotato: ${GBs(Math.max(0, r.liberato_b || 0))} liberi`, `Trash emptied: ${GBs(Math.max(0, r.liberato_b || 0))} freed`) : tr("Non fatto", "Not done") + ": " + (r.msg || ""), null, !r.ok);
  }
  async function undo(n) {
    const x = (SP.d.storico || []).find((s) => s.n === n);
    if (!(await conferma(tr(`Annullare "${x?.titolo || "questa azione"}"?`, `Undo "${x?.titolo || "this action"}"?`), tr("Rimetto tutto al suo posto.", "Everything goes back where it was."), tr("Annulla", "Undo")))) return;
    const r = await busyWork(tr("rimetto a posto", "restoring"), () => invokeSp(["annulla", n]));
    if (r) showOutcome(r.ok ? tr(`Rimesso a posto: ${(r.ripristinati || []).length} elementi`, `Restored: ${(r.ripristinati || []).length} items`) : tr("Non del tutto", "Not entirely") + ": " + (r.msg || ""), null, !r.ok);
  }
  async function saveSettings() {
    const ob = parseInt(document.getElementById("sp-imp-ob")?.value || "100", 10), pct = parseInt(document.getElementById("sp-imp-pct")?.value || "15", 10);
    const args = ["imposta", "obiettivo_disco_gb", String(Math.max(10, Math.min(400, ob || 100))), "avviso_disco_pct", String(Math.max(3, Math.min(40, pct || 15)))];
    const av = document.getElementById("sp-imp-avviso"); if (av) args.push("avviso_disco", av.checked ? "true" : "false");
    const au = document.getElementById("sp-imp-auto"); if (au) args.push("spazio_auto", au.checked ? "true" : "false");
    const r = await invokeSp(args);
    SP.impApre = false;
    toast(r?.ok ? tr("Salvato", "Saved") : tr("Non salvato", "Not saved"));
    load(false);
  }
  function goTo(id) { if (!id) return; SP.chiusi.delete(id); render(); const el = document.getElementById("sp-g-" + id); if (el) el.scrollIntoView({ behavior: "smooth", block: "start" }); }

  document.addEventListener("click", (e) => {
    if (typeof vistaMac === "undefined" || !vistaMac || !e.target.closest?.("#mac-s-spazio")) {
      if (SP.menu && !e.target.closest?.(".sp-menu")) { SP.menu = null; render(); }
      return;
    }
    const t = (s) => e.target.closest(s);
    let b;
    if ((b = t("[data-sp-scegli]"))) { const id = b.dataset.spScegli; b.checked ? SP.scelte.add(id) : SP.scelte.delete(id); return render(); }
    if (t("#sp-sicuro")) return freeSafe();
    if (t("#sp-completa")) { invokeSp(["esplora-avvia"]).then((r) => { if (r?.ok) { SP.d.completa_in_corso = r.avanza || { pct: 0, inizio: Date.now() / 1000, file: 0 }; render(); } else toast(tr("Non parte", "Couldn't start") + ": " + (r?.msg || "")); }); return; }
    if (t("#sp-ferma")) { invokeSp(["esplora-ferma"]).then((r) => toast(r?.msg || "")); return; }
    if (t("#sp-svuota")) return emptyTrash(0);
    if (t("#sp-svuota-7")) return emptyTrash(7);
    if (t("[data-sp-imp]")) { SP.impApre = !SP.impApre; return render(); }
    if (t("#sp-imp-salva")) return saveSettings();
    if (t("[data-sp-esito-chiudi]")) { SP.esito = null; return render(); }
    if (t("[data-sp-fai-scelte]")) return doChoices();
    if (t("[data-sp-deseleziona]")) { SP.scelte.clear(); return render(); }
    if (t("[data-sp-scegli-tutte]")) { for (const v of Object.values(SP.d.voci || {})) if (v.sicurezza === "sicuro" && runnable(v)) SP.scelte.add(v.id); return render(); }
    if ((b = t("[data-sp-filtro]"))) { SP.filtro = b.dataset.spFiltro; return render(); }
    if ((b = t("[data-sp-passo]"))) { const p = b.dataset.spPasso; if (p === "sp-sicuro") return freeSafe(); if (p === "sp-svuota") return goTo("cestino"); SP.filtro = "conf"; render(); return goTo((SP.d.gruppi || []).find((g) => g.voci.some((id) => passesFilter(SP.d.voci[id] || {})))?.id); }
    if ((b = t("[data-sp-vai]"))) return goTo(b.dataset.spVai);
    if ((b = t("[data-sp-chiudi]"))) { const k = b.dataset.spChiudi; SP.chiusi.has(k) ? SP.chiusi.delete(k) : SP.chiusi.add(k); localStorage.setItem("spChiusi", JSON.stringify([...SP.chiusi])); return render(); }
    if ((b = t("[data-sp-fai]"))) return doIt(b.dataset.spFai, b.dataset.modo);
    if ((b = t("[data-sp-menu]"))) { SP.menu = SP.menu === b.dataset.spMenu ? null : b.dataset.spMenu; return render(); }
    if ((b = t("[data-sp-apri]"))) { const k = b.dataset.spApri; SP.aperti.has(k) ? SP.aperti.delete(k) : SP.aperti.add(k); return render(); }
    if ((b = t("[data-sp-tutte]"))) { const k = b.dataset.spTutte; SP.mostraTutte.has(k) ? SP.mostraTutte.delete(k) : SP.mostraTutte.add(k); return render(); }
    if ((b = t("[data-sp-annulla]"))) return undo(b.dataset.spAnnulla);
    if ((b = t("[data-sp-riproponi]"))) { invokeSp(["riproponi", b.dataset.spRiproponi]).then(() => load(false)); return; }
    if ((b = t("[data-sp-finder]"))) { if (b.dataset.spFinder) invoke("apri_file", { path: b.dataset.spFinder, finder: true }).catch(() => {}); return; }
    if ((b = t("[data-sp-chat]"))) { scegli(b.dataset.spChat); return; }
    if (SP.menu && !t(".sp-menu")) { SP.menu = null; render(); }
  });
  document.addEventListener("keydown", (e) => {
    if (typeof vistaMac === "undefined" || !vistaMac || !document.getElementById("mac-s-spazio")) return;
    if (e.key === "Escape" && (SP.menu || SP.scelte.size) && !document.querySelector("#velo.on")) { e.stopPropagation(); SP.menu = null; SP.scelte.clear(); render(); }
  }, true);

  const st = document.createElement("style");
  st.textContent = `
.spazio-nuovo>#mac-testa,.spazio-nuovo>#mac-s-categorie,.spazio-nuovo>#mac-s-sugg,.spazio-nuovo>#mac-s-protetti,.spazio-nuovo>#mac-s-storico{display:none}
#mac-s-spazio{display:contents}
#mac-s-spazio>.mac-card,#mac-s-spazio>.sp-filtri{grid-column:1/-1}
#mac-s-spazio>.sp-auto,#mac-s-spazio>.mac-protetti,#mac-s-spazio>.sp-cest{grid-column:auto}
.sp-testa{display:flex;flex-direction:column;gap:12px}
.sp-riga1{display:flex;align-items:baseline;gap:14px;flex-wrap:wrap}
.sp-num{font-size:15px;color:var(--ink2)}.sp-num b{font-size:30px;font-weight:650;letter-spacing:-.02em;color:var(--ink);font-variant-numeric:tabular-nums}
.sp-num small{font-size:12.5px;color:var(--ink3);margin-left:4px}
.sp-testa.allarme .sp-num b{color:var(--err)}
.sp-ob{margin-left:auto;font-size:13px;color:var(--ink2)}.sp-ob b{color:var(--ink);font-variant-numeric:tabular-nums}
.sp-link{margin-left:8px;font-size:12px;color:var(--ink3);text-decoration:underline;text-underline-offset:2px}.sp-link:hover{color:var(--ink)}
.sp-barra{position:relative;display:flex;height:12px;border-radius:6px;background:var(--fill);overflow:visible;margin-bottom:10px}
.sp-barra i{display:block;height:100%}.sp-barra .u{background:var(--ink);border-radius:6px 0 0 6px}
.sp-barra .s{background:repeating-linear-gradient(135deg,var(--ink2) 0 3px,transparent 3px 6px);opacity:.8}
.sp-testa.allarme .sp-barra .u{background:var(--err)}
.sp-barra .ob{position:absolute;top:-4px;bottom:-4px;width:2px;background:var(--ink);border-radius:1px}
.sp-barra .ob em{position:absolute;top:18px;left:50%;transform:translateX(-50%);font-style:normal;font-size:10.5px;color:var(--ink3);white-space:nowrap}
.sp-azioni{display:flex;align-items:center;gap:10px;flex-wrap:wrap}
.sp-quando{font-size:11.5px;color:var(--ink3)}
.sp-nota{margin:0;font-size:12px;color:var(--ink3)}.sp-occ{color:var(--ink2)}
.sp-avanza{display:none}.sp-avanza.on{display:block}
.sp-av-r{display:flex;align-items:center;gap:10px;margin-top:6px}.sp-av-r small{flex:1;font-size:11.5px;color:var(--ink3)}
.sp-and{color:var(--ink2)}.sp-and svg{display:block;width:100%;height:40px}.sp-and small{font-size:11px;color:var(--ink3)}
.sp-imp{display:flex;flex-wrap:wrap;gap:10px 18px;align-items:center;padding:10px 12px;border:1px solid var(--line);border-radius:10px;font-size:12.5px;color:var(--ink2)}
.sp-imp input[type=number]{width:56px;height:26px;border:1px solid var(--line);border-radius:7px;background:var(--bg);color:var(--ink);font:inherit;text-align:center}
.sp-esito{display:flex;align-items:center;gap:10px;padding:12px 16px;background:var(--ink);color:var(--bg)}.sp-esito span{flex:1;font-size:13.5px}
.sp-esito .tasto{color:var(--bg);border-color:rgba(255,255,255,.3);background:transparent}.sp-esito.err{background:var(--err)}
.sp-x{font-size:18px;line-height:1;padding:0 6px}
.sp-vuoto{padding:28px}.sp-vuoto h3{margin:0 0 8px;font-size:18px}.sp-vuoto p{margin:0 0 16px;font-size:13px;line-height:1.5;color:var(--ink2);max-width:64ch}
.sp-piano .sp-passo{display:flex;align-items:center;gap:12px;padding:9px 0;border-top:1px solid var(--line)}.sp-piano .sp-gt+.sp-passo{border-top:0;margin-top:4px}
.sp-passo .n{width:22px;height:22px;border-radius:50%;display:grid;place-items:center;background:var(--fill2);font-size:11.5px;font-weight:650;flex:none}
.sp-passo .t{flex:1;font-size:13px}.sp-passo .t small{display:block;font-size:11.5px;color:var(--ink3);margin-top:1px}
.sp-leg{display:inline-flex;align-items:center;gap:6px;font-size:12.5px;color:var(--ink2);padding:2px 4px;border-radius:6px}.sp-leg:hover{background:var(--fill)}
.sp-leg i{width:9px;height:9px;border-radius:3px;display:inline-block}.sp-leg b{color:var(--ink);font-weight:600;font-variant-numeric:tabular-nums}
.sp-filtri{display:flex;flex-wrap:wrap;align-items:center;gap:6px;margin:2px 0}
.sp-chip{font-size:12.5px;padding:5px 11px;border-radius:20px;border:1px solid var(--line);background:var(--bg);color:var(--ink2)}.sp-chip small{color:var(--ink3);margin-left:3px}
.sp-chip.on{background:var(--ink);color:var(--bg);border-color:var(--ink)}.sp-chip.on small{color:var(--bg);opacity:.7}
.sp-gt{display:flex;align-items:baseline;gap:10px}.sp-gt h4{margin:0}.sp-ggb{margin-left:auto;font-weight:650;font-variant-numeric:tabular-nums;font-size:13.5px}
.sp-gt-btn{width:100%;text-align:left;align-items:center}.sp-fr{width:13px;height:13px;color:var(--ink3);transition:transform .2s;flex:none}.sp-fr.giu{transform:rotate(90deg)}
.sp-n{font-size:11px;color:var(--ink3);background:var(--fill);border-radius:10px;padding:1px 7px}
.sp-gsp{margin:4px 0 6px;font-size:12px;color:var(--ink3)}
.sp-voce{border-top:1px solid var(--line);padding:10px 0;transition:background .15s}.sp-voce.scelta{background:var(--fill);margin:0 -10px;padding:10px;border-radius:8px}
.sp-v1{display:flex;align-items:center;gap:8px;flex-wrap:wrap}
.sp-ck{display:inline-flex;width:18px;flex:none}.sp-ck input{width:15px;height:15px;margin:0;accent-color:var(--ink)}
.sp-tit{font-size:13.5px;font-weight:550;min-width:0;overflow:hidden;text-overflow:ellipsis;max-width:46ch;white-space:nowrap}
.sp-gb{margin-left:auto;font-weight:650;font-variant-numeric:tabular-nums;white-space:nowrap}
.ms-et.pers{background:transparent;border:1px solid var(--ink3);color:var(--ink2)}.ms-et.info{background:transparent;border:1px dashed var(--line);color:var(--ink3)}
.sp-cr-et{font-size:10.5px;padding:2px 7px;border-radius:20px;background:var(--ink);color:var(--bg);font-variant-numeric:tabular-nums}
.sp-piu{width:28px;padding:0;display:grid;place-items:center}.sp-piu svg{width:14px;height:14px}
.sp-v2{margin:4px 0 0 26px;font-size:12.5px;line-height:1.45;color:var(--ink2)}.sp-v2.man{color:var(--ink3)}
.sp-v3{display:flex;flex-wrap:wrap;align-items:center;gap:4px;margin:5px 0 0 26px;font-size:11.5px;color:var(--ink3)}
.sp-v3.mot{color:var(--ink2)}.sp-sep{font-style:normal;opacity:.6}.sp-ind{margin-left:26px}
.sp-p{max-width:52ch;overflow:hidden;text-overflow:ellipsis;white-space:nowrap}
.sp-chi{font-size:11.5px;padding:1px 8px;border-radius:20px;background:var(--fill2);color:var(--ink)}.sp-chi:hover{background:var(--ink);color:var(--bg)}.sp-chi.muto{background:var(--fill);color:var(--ink2)}
.sp-menu{display:flex;flex-wrap:wrap;gap:6px;margin:8px 0 0 26px;padding:8px;border-radius:9px;background:var(--fill)}
.sp-cr{display:flex;align-items:center;gap:10px;padding:7px 0;border-top:1px solid var(--line)}.sp-cr:first-of-type{border-top:0}
.sp-cr b{min-width:64px;font-variant-numeric:tabular-nums}.sp-cr .t{flex:1;min-width:0;font-size:13px}.sp-cr .t small{display:block;color:var(--ink3);font-size:11px;overflow:hidden;text-overflow:ellipsis;white-space:nowrap}
.tasto.pericolo{border-color:var(--err);color:var(--err)}
.sp-barra-scelte{position:sticky;bottom:12px;z-index:5;grid-column:1/-1;display:flex;align-items:center;gap:10px;padding:10px 12px 10px 16px;border-radius:12px;background:var(--bg);border:1px solid var(--line);box-shadow:0 8px 28px rgba(0,0,0,.16)}
.sp-barra-scelte span{flex:1;font-size:13px}.sp-barra-scelte b{font-variant-numeric:tabular-nums}
@media (max-width:640px){.sp-ob{margin-left:0}.sp-tit{max-width:100%;white-space:normal}.sp-gb{margin-left:0}.sp-v2,.sp-v3,.sp-menu,.sp-ind{margin-left:0}}
`;
  document.head.appendChild(st);
})();
