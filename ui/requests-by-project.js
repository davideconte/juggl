// Requests by project (list Nov 15). The user's request of Nov 12: "group requests by project, in a smart way".
// - One section per project, with the project icon and color (EL.coloreDi). First the projects with more things waiting
//   for the user, then the most recent.
// - Inside each project, on top what waits for the user: approvals to give, new answers, "serve te" requests.
//   Below, collapsed in one row and expandable: "In lavoro N" (with how many are stuck) and "Fatte N" (only last 3 days; those
//   closed by Juggl say so in the answer). A project with nothing for the user stays a single expandable row.
// - Duplicates: the extractor merges them within 48 hours; requests.rs superate() marks "doppione_di" on near-identical titles of the
//   same project (nothing is deleted). Here a single card: the most urgent entry, "chiesta N volte" and the chats of all of them.
//   The old rule stays: a "serve te" from a chat that already has an open approval is not repeated.
// - Old ones: superate() closes by itself (chiusa_da_juggl) an in-progress request whose chats have been gone for 7 days.
//   Closures and duplicates decided by Juggl sit at the bottom, in "Chiuse o unite da Juggl", with the reason and Riapri / Separa.
//   Done ones older than 3 days are not shown here (the log keeps them 14 days; those closed by Juggl forever).
// - Settings > Requests: "Pulizia automatica delle richieste" and "Cerca i loghi dei progetti online".
// - "Restano" (things in progress or waiting for others) sits in its project, expandable as before.
// Reuses the cards of index.html (schedaRq, schedaOk, gruppoRestano) and their clicks. Hook in index.html (marked
// "elenco (15/11)"): first line of disegnaRichieste and <script src="requests-by-project.js">. Project chips stay: they filter.
(() => {
  if (!window.__TAURI__) return;
  const RQP = (window.RQP = { aperti: new Set(JSON.parse((() => { try { return localStorage.getItem("rqp-aperti") || "[]"; } catch { return "[]"; } })())) });
  const DONE_DAYS = 3;
  const save = () => { try { localStorage.setItem("rqp-aperti", JSON.stringify([...RQP.aperti])); } catch {} };
  const css = `
.rqp-prog{margin:0 0 18px;border-radius:14px;box-shadow:inset 0 0 0 1px var(--line);padding:10px 12px 4px;border-left:3px solid var(--c,var(--line))}
.rqp-testa{display:flex;align-items:center;gap:9px;width:100%;text-align:left;padding:2px 0 8px;cursor:pointer}
.rqp-testa .av,.rqp-testa .el-icona{flex:none}
.rqp-nome{font-weight:650;font-size:14px;color:var(--ink)}
.rqp-conta{margin-left:auto;font-size:12px;color:var(--ink2);white-space:nowrap}
.rqp-conta b{color:var(--el-ar,#c2410c);font-weight:650}
.rqp-testa svg{flex:none;color:var(--ink3);transition:transform .2s}
.rqp-prog.chiuso .rqp-testa svg{transform:rotate(-90deg)}
.rqp-prog.chiuso{padding-bottom:2px}
.rqp-riga{display:flex;align-items:center;gap:8px;width:100%;text-align:left;padding:7px 2px;border-top:1px solid var(--line);font-size:12.5px;color:var(--ink2);cursor:pointer}
.rqp-riga b{color:var(--ink);font-weight:600}
.rqp-riga .ferme{color:var(--err);font-weight:600}
.rqp-riga svg{margin-left:auto;color:var(--ink3);transition:transform .2s}
.rqp-riga.aperta svg{transform:rotate(180deg)}
.rqp-sotto{padding:4px 0 6px}
.rqp-prog .rq-gruppo{margin:0 0 10px}
.rqp-jt{border-top:0;margin-top:6px}
.rqp-perte{margin:0 0 16px}
.rqp-pt{display:flex;align-items:center;gap:10px;padding:8px 2px;border-top:1px solid var(--line)}
.rqp-pt .tx{flex:1;min-width:0}
.rqp-pt .tx b{display:block;font-size:13px;font-weight:600;color:var(--ink);white-space:nowrap;overflow:hidden;text-overflow:ellipsis}
.rqp-pt .tx small{display:block;font-size:11.5px;color:var(--ink2);margin-top:2px;white-space:nowrap;overflow:hidden;text-overflow:ellipsis}
.rqp-pt .tx small b{display:inline;font-size:inherit;color:var(--ink)}
.rqp-pt.ok .tx small b,.rqp-pt.decisione .tx small b,.rqp-pt.fai_tu .tx small b{color:var(--el-ar,#c2410c)}
.rqp-pt .az,.rqp-riep .az{display:flex;gap:6px;flex:none}
.rqp-pt .az button,.rqp-riep .az button,.rqp-ann button{font-size:12px;font-weight:600;padding:4px 10px;border-radius:8px;box-shadow:inset 0 0 0 1px var(--line);cursor:pointer;color:var(--ink)}
.rqp-pt .az button.nero,.rqp-riep .az button.nero{background:var(--ink);color:var(--bg);box-shadow:none}
.rqp-volte{font-size:11px;font-weight:600;color:var(--ink3);margin-left:6px}
.rqp-tenue{font-size:12px;color:var(--ink3)}
.rqp-blocco{border-top:1px solid var(--line)}
.rqp-sezt{margin-top:18px}
.rqp-chiudi-vecchie{font-size:12px;font-weight:600;color:var(--ink2);text-decoration:underline;text-underline-offset:2px;cursor:pointer;margin:4px 2px}
.rqp-riep{margin:8px 0;padding:10px 12px;border-radius:10px;background:var(--fill);font-size:12.5px;color:var(--ink)}
.rqp-riep ul{margin:6px 0 6px 18px;padding:0}.rqp-riep small{display:block;color:var(--ink2);margin-bottom:8px}
.rqp-ann{display:flex;align-items:center;justify-content:space-between;gap:10px}
@media (prefers-color-scheme: dark){.rqp-pt.ok .tx small b,.rqp-pt.decisione .tx small b,.rqp-pt.fai_tu .tx small b{color:var(--el-ar,#ff7a45)}}
.rqp-j{display:flex;align-items:center;gap:10px;padding:8px 2px;border-top:1px solid var(--line)}
.rqp-j .tx{flex:1;min-width:0}.rqp-j b{display:block;font-size:13px;font-weight:600;color:var(--ink)}.rqp-j small{display:block;font-size:12px;color:var(--ink2);margin-top:2px}
@media (prefers-color-scheme: dark){.rqp-conta b{color:var(--el-ar,#ff7a45)}}
@media (prefers-reduced-motion: reduce){.rqp-testa svg,.rqp-riga svg{transition:none}}
`;
  const st = document.createElement("style"); st.id = "rqp-css"; st.textContent = css; document.head.appendChild(st);
  const FR = '<svg width="12" height="12" viewBox="0 0 12 12" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round"><path d="m3 4.5 3 3 3-3"/></svg>';
  const plur = (n, a, b) => `${n} ${n === 1 ? a : b}`;
  const iconHtml = (p) => window.EL?.coloreDi
    ? (() => { const c = EL.coloreDi(p); const logo = p?.logo; const inside = logo ? `<img src="${esc(T.core.convertFileSrc(logo))}" alt="">` : esc(String(p?.nome || "?").split(/\s+/).filter(Boolean).slice(0, 2).map((w) => [...w][0]).join("").toUpperCase() || "?");
      return `<span class="el-icona st-gr" style="--c:${c};--l:26px"><span class="el-av${logo ? " logo" : ""}">${inside}</span></span>`; })()
    : avatar({ nome: p?.nome || "", progetto: p }, "mini");

  /** What a request is for the user. Uses the server "tipo" field (decisione... | lavoro_in_corso | appesa |
   *  vecchia) when present; otherwise the same rules here. Values: nuova, decisione, fai_tu, appesa, vecchia, lavoro, "" (done). */
  function kindOf(v, nowMs) {
    if (nuovaRq(v)) return "nuova";
    if (v.stato === "fatta") return "";
    const t = v.tipo;
    if (t) return String(t).startsWith("decisione") ? "decisione" : t === "lavoro_in_corso" ? (v.fai_tu ? "fai_tu" : "lavoro") : t;
    if (v.fai_tu) return "fai_tu";
    const promoted = v.promossa_dal_sistema || /ferma da piu|non risponde al sollecito/i.test(v.motivo || "");
    if (v.stato === "serve_te" && !promoted) return !v.aspetta_ok && nowMs - (v.creata || 0) > 7 * 86400 ? "vecchia" : "decisione";
    if (nowMs - (v.creata || 0) > 3 * 86400 && v.vista !== "lavora") return "vecchia";
    if (v.appesa || v.ferma || promoted) return "appesa";
    return "lavoro";
  }
  RQP.tipoDi = kindOf;
  RQP.disegna = () => {
    const el = $("#rq-pagina"); if (!el || (!datiRq && !datiOk)) return;
    const nowMs = Date.now() / 1000;
    const all0 = datiRq?.voci || [];
    // duplicates (doppione_di): a single card, the most urgent entry, with count and chats of all. The entries stay intact.
    const URG = { serve_te: 3, non_presa: 2, in_lavoro: 1 };
    const aperta = (v) => v.stato !== "fatta";
    const byId = new Map(all0.map((v) => [v.id, v]));
    const groups = new Map();
    for (const v of all0) { const p = v.doppione_di && byId.get(v.doppione_di); if (p && aperta(v) && aperta(p)) { if (!groups.has(p.id)) groups.set(p.id, [p]); groups.get(p.id).push(v); } }
    const hiddenIds = new Set();
    const merged = new Map();
    for (const [, l] of groups) {
      const vis = [...l].sort((a, b) => (URG[b.stato] || 0) - (URG[a.stato] || 0) || (a.creata || 0) - (b.creata || 0))[0];
      const also = new Map();
      for (const x of l) { if (x.sessione !== vis.sessione) also.set(x.sessione, { sessione: x.sessione, nome: x.nome, viva: x.viva }); for (const y of x.anche_in || []) if (y.sessione !== vis.sessione) also.set(y.sessione, y); }
      merged.set(vis.id, { ...vis, volte: l.reduce((s, x) => s + (x.volte || 1), 0), anche_in: [...also.values()], aggiornata: Math.max(...l.map((x) => x.aggiornata || 0)) });
      for (const x of l) if (x.id !== vis.id) hiddenIds.add(x.id);
    }
    const allItems = all0.filter((v) => !hiddenIds.has(v.id) && !v.chiusa_da_juggl).map((v) => merged.get(v.id) || v);
    const flagged = new Set([...groups.values()].flatMap((l) => l.filter((x) => x.doppione_di).map((x) => x.id)));
    const byJuggl = all0.filter((v) => v.chiusa_da_juggl || flagged.has(v.id));
    const allOks = (datiOk?.voci || []).filter((v) => v.stato !== "fatta");
    const key = (v) => v.progetto?.id || "altro";
    const inFilter = (v) => filtroRq === "tutti" || key(v) === filtroRq;
    // ---- what is needed from the user: one place (tipoDi), with the server "tipo" field when present ----
    const openOks = allOks.filter((v) => v.tipo === "ok" && (okAperto(v) || v.stato === "in_conferma"));
    const withOk = new Set(openOks.map((v) => v.sessione));
    const covered = (v) => !!v.aspetta_ok || [v.sessione, ...(v.anche_in || []).map((x) => x.sessione)].some((x) => withOk.has(x));
    const kinds = allItems.map((v) => [v, kindOf(v, nowMs)]);
    const ofKind = (t) => kinds.filter(([v, x]) => x === t && inFilter(v)).map(([v]) => v);
    const forYou = [
      ...openOks.filter(inFilter).sort((a, b) => (a.stato === "in_conferma") - (b.stato === "in_conferma") || (a.creata || 0) - (b.creata || 0)).map((v) => ({ v, k: "ok" })),
      ...ofKind("decisione").filter((v) => !covered(v)).sort((a, b) => (a.creata || 0) - (b.creata || 0)).map((v) => ({ v, k: "decisione" })),
      ...ofKind("fai_tu").sort((a, b) => (a.creata || 0) - (b.creata || 0)).map((v) => ({ v, k: "fai_tu" })),
      ...ofKind("nuova").sort((a, b) => (b.fatta_il || 0) - (a.fatta_il || 0)).map((v) => ({ v, k: "nuova" })),
    ];
    const stalled = ofKind("appesa"), oldOnes = ofKind("vecchia"), running = ofKind("lavoro");
    const doneOnes = allItems.filter((v) => v.stato === "fatta" && !nuovaRq(v) && inFilter(v) && nowMs - (v.fatta_il || v.aggiornata || 0) < DONE_DAYS * 86400);
    const remaining = allOks.filter((v) => v.tipo !== "ok" && inFilter(v));
    // ---- one row per entry: what is needed, who the answer goes to, clear buttons ----
    const WHAT = { ok: "decidi tu", decisione: "ti chiede una risposta", fai_tu: "tocca a te", nuova: "risposta nuova", appesa: "ferma", vecchia: "vecchia", lavoro: "in corso" };
    const entryRow = ({ v, k }) => {
      const proj = v.progetto?.nome || "Altre", chat = v.nome || "la chat";
      const times = v.volte > 1 ? `<span class="rqp-volte" title="Chiesta ${v.volte} volte">×${v.volte}</span>` : "";
      const meta = `${esc(proj)} · <b>${WHAT[k]}</b>${k === "nuova" ? ` · da «${esc(chat)}»` : ` · risposta a «${esc(chat)}»`} · ${daQuanto(k === "nuova" ? v.fatta_il || v.aggiornata : v.creata || v.aggiornata)}`;
      let az;
      if (k === "ok") {
        const b = (a, t, c = "") => `<button class="${c}" data-oka="${a}" data-id="${esc(v.id)}">${t}</button>`;
        az = v.stato === "in_conferma" ? `<span class="rqp-tenue">aspetto conferma</span>` : (v.fai_tu ? b("ok", "Fatto", "nero" + provOk(v, "ok")) : b("ok", "Ok", "nero" + provOk(v, "ok")) + b("no", "No", provOk(v, "no"))) + b("scrivi", "Rispondi…");
      } else if (k === "decisione") az = `<button data-rqa="apri" data-id="${esc(v.id)}" class="nero">Rispondi…</button><button data-rqa="chiudi" data-id="${esc(v.id)}">Chiudi</button>`;
      else if (k === "fai_tu") az = `<button data-rqa="fatto" data-id="${esc(v.id)}" class="nero">Fatto</button><button data-rqa="chiudi" data-id="${esc(v.id)}">Chiudi</button>`;
      else if (k === "nuova") az = `<button data-rq-vai="${esc(v.id)}" class="nero">Leggi</button><button data-rqp-az="letta" data-id="${esc(v.id)}">Segna letta</button>`;
      else az = `<button data-rqa="apri" data-id="${esc(v.id)}">Apri</button><button data-rqa="chiudi" data-id="${esc(v.id)}">Chiudi</button>`;
      const write = k === "ok" && okScrivi === v.id ? `<div class="ok-scrivi"><textarea rows="1" data-ok-testo="${esc(v.id)}" placeholder="Rispondi a ${esc(chat)}…"></textarea><button class="tasto bordo" data-oka="rispondi" data-id="${esc(v.id)}">Manda</button></div>` : "";
      return `<div class="rqp-pt ${k}" data-rqp-id="${esc(v.id)}">${iconHtml(v.progetto || { id: "altro", nome: proj })}<div class="tx"><b title="${esc(v.testo || v.titolo)}">${esc(v.titolo)}${times}</b><small>${meta}${v.motivo_appesa && k === "appesa" ? " · " + esc(v.motivo_appesa) : ""}</small></div><div class="az">${az}</div></div>${k === "ok" && v.stato !== "in_conferma" ? barraOk(v) : ""}${write}`;
    };
    const MAX = 7, allOfProject = RQP.aperti.has("pt-tutte");
    const blk = (id, titleText, lista, rend, isOpen) => lista.length ? `<button class="rqp-riga rqp-blocco${isOpen ? " aperta" : ""}" data-rqp-sez="${id}" aria-expanded="${isOpen}">${titleText}${FR}</button>${isOpen ? `<div class="rqp-sotto">${rend(lista)}</div>` : ""}` : "";
    const forYouH = forYou.length
      ? `<section class="rqp-perte"><h3 class="rqg-testa">Per te <span>${forYou.length}</span><em>solo quello che aspetta te</em></h3>${(allOfProject ? forYou : forYou.slice(0, MAX)).map(entryRow).join("")}${forYou.length > MAX ? `<button class="rq-altre" data-rqp-sez="pt-tutte">${allOfProject ? "Mostra meno" : `Mostra altre ${forYou.length - MAX}`}</button>` : ""}</section>`
      : `<div class="rq-vuoto"><h4>Niente che aspetti te</h4><p>Nessuna decisione, nessun lavoro tuo e nessuna risposta nuova.</p></div>`;
    const stuckH = blk("ferme", `<span><b>Ferme</b> ${stalled.length} <span class="tenue">· senza domanda per te</span></span>`, stalled, (l) => l.map((v) => entryRow({ v, k: "appesa" })).join(""), RQP.aperti.has("ferme"));
    // in progress, per project, collapsed in one row per project
    const byProject = new Map();
    for (const v of running) { const k = key(v); if (!byProject.has(k)) byProject.set(k, { p: v.progetto || { id: "altro", nome: "Altre" }, l: [] }); byProject.get(k).l.push(v); }
    const runningH = running.length ? `<h3 class="rqg-testa rqp-sezt">In corso <span>${running.length}</span><em>lavori che le chat stanno facendo</em></h3>` + [...byProject.entries()].map(([k, x]) => blk("lav:" + k, `${iconHtml(x.p)}<span><b>${esc(x.p.nome || "Altre")}</b> ${x.l.length}</span>`, x.l, (l) => l.map((v) => entryRow({ v, k: "lavoro" })).join(""), RQP.aperti.has("lav:" + k))).join("") : "";
    const doneH = blk("fatte", `<span><b>Fatte</b> ${doneOnes.length} <span class="tenue">· ultimi ${DONE_DAYS} giorni</span></span>`, doneOnes, (l) => l.map((v) => schedaRq(v)).join(""), RQP.aperti.has("fatte"));
    const remainingH = remaining.length ? gruppoRestano(remaining) : "";
    // old ones: in "Mostra altre", with "Chiudi le vecchie" (only those without a question for you) and a summary before acting
    const closable = oldOnes.filter((v) => v.stato !== "serve_te" && !v.fai_tu && !covered(v));
    const recapH = RQP.riepilogo ? (() => {
      const pp = new Map(); for (const v of closable) { const k = v.progetto?.nome || "Altre"; pp.set(k, (pp.get(k) || 0) + 1); }
      return `<div class="rqp-riep"><b>Chiudo ${plur(closable.length, "richiesta vecchia", "richieste vecchie")}</b> (oltre 3 giorni, senza domanda per te):<ul>${[...pp.entries()].sort((a, b) => b[1] - a[1]).map(([k, n]) => `<li>${esc(k)}: ${n}</li>`).join("")}</ul><small>Le domande per te e i tuoi lavori non si toccano. Per 20 secondi c'e' Annulla.</small><div class="az"><button class="nero" data-rqp-vecchie="chiudi">Chiudi ${closable.length}</button><button data-rqp-vecchie="no">Annulla</button></div></div>`;
    })() : "";
    const oldH = oldOnes.length ? `<button class="rqp-riga rqp-blocco${RQP.aperti.has("vecchie") ? " aperta" : ""}" data-rqp-sez="vecchie"><span><b>Mostra altre</b> ${oldOnes.length} <span class="tenue">· vecchie, oltre 3 giorni</span></span>${FR}</button>${closable.length ? `<button class="rqp-chiudi-vecchie" data-rqp-vecchie="riepilogo">Chiudi le vecchie…</button>` : ""}${recapH}${RQP.aperti.has("vecchie") ? `<div class="rqp-sotto">${oldOnes.map((v) => entryRow({ v, k: "vecchia" })).join("")}</div>` : ""}` : "";
    const undoH = RQP.chiuse?.length ? `<div class="rqp-riep rqp-ann"><span>${plur(RQP.chiuse.length, "richiesta vecchia chiusa", "richieste vecchie chiuse")}</span><button data-rqp-vecchie="annulla">Annulla</button></div>` : "";
    // project chips: how many things for you in each
    const pc = new Map();
    for (const v of [...allOks, ...allItems]) { const k = key(v); if (!pc.has(k)) pc.set(k, { k, p: v.progetto || { id: "altro", nome: "Altre" }, n: 0, u: 0 }); const x = pc.get(k); x.u = Math.max(x.u, v.aggiornata || v.creata || 0); }
    for (const { v } of forYou) { const x = pc.get(key(v)); if (x) x.n++; }
    const projList = [...pc.values()].sort((a, b) => b.n - a.n || b.u - a.u);
    if (filtroRq !== "tutti" && !pc.has(filtroRq)) filtroRq = "tutti";
    const analysis = (datiRq?.in_analisi || []).length, analysisOk = (datiOk?.in_analisi || []).length;
    const n = (x, uno, multiple, cl = "") => x ? `<span class="${cl}"><b>${x}</b> ${x === 1 ? uno : multiple}</span>` : "";
    const recap = [n(forYou.length, "per te", "per te"), n(stalled.length, "ferma", "ferme", "tenue"), n(running.length, "in corso", "in corso", "tenue"), n(oldOnes.length, "vecchia", "vecchie", "tenue")].filter(Boolean).join("") || `<span class="tenue">Nessuna richiesta aperta</span>`;
    const chip = (k, p, num) => `<button class="rq-chip${k === "tutti" ? " solo" : ""}${filtroRq === k ? " on" : ""}" data-rq-prog="${esc(k)}">${k === "tutti" ? "" : avatar({ nome: p?.nome || "", progetto: p }, "mini")}${esc(k === "tutti" ? "Tutti" : p?.nome || "Altre")}${num ? `<span class="n">${num}</span>` : ""}</button>`;
    const empty = !allItems.length && !allOks.length ? `<div class="vuoto">${analysis ? "Sto leggendo le tue conversazioni per ricavare le richieste…" : "Ancora nessuna richiesta. Quando scrivi a una sessione, le cose che chiedi compaiono qui."}</div>` : "";
    const jugglRow = (v) => { const p = v.doppione_di ? byId.get(v.doppione_di) : null; const c = v.chiusa_da_juggl;
      return `<div class="rqp-j"><div class="tx"><b>${esc(v.titolo)}</b><small>${esc(v.progetto?.nome || v.nome || "")} · ${c ? `chiusa da Juggl ${daQuanto(c.il)}: ${esc(c.motivo || "")}` : `unita a «${esc(p?.titolo || "")}»: stesso progetto, titolo quasi uguale`}</small></div><button class="tasto" data-rqp-annulla="${esc(v.id)}">${c ? "Riapri" : "Separa"}</button></div>`; };
    const jugglSec = byJuggl.length ? `<button class="rqp-riga rqp-jt${RQP.aperti.has("juggl") ? " aperta" : ""}" data-rqp-sez="juggl"><span><b>Chiuse o unite da Juggl</b> ${byJuggl.length} <span class="tenue">· si annullano con un clic</span></span>${FR}</button>${RQP.aperti.has("juggl") ? `<div class="rqp-sotto">${byJuggl.map(jugglRow).join("")}</div>` : ""}` : "";
    const h = `<div class="rq-sommario">${recap}${analysis ? `<span class="rq-analisi"><i></i>Leggo i messaggi nuovi</span>` : analysisOk ? `<span class="rq-analisi"><i></i>Leggo le ultime risposte</span>` : ""}</div>
      ${projList.length > 1 ? `<div class="rq-progetti">${chip("tutti", null, 0)}${projList.map((g) => chip(g.k, g.p, g.n)).join("")}</div>` : ""}
      <div class="rq-lista ${entraRq ? "entra" : ""}">${undoH}${empty || forYouH}${stuckH}${runningH}${remainingH}${doneH}${oldH}${filtroRq === "tutti" ? jugglSec : ""}</div>`;
    entraRq = false; apreRq = "";
    if (el._h !== h) {
      // an answer you are typing stays (text and cursor) even if the list refreshes underneath
      const ta = el.querySelector("[data-ok-testo]"), draft = ta ? { id: ta.dataset.okTesto, v: ta.value, f: document.activeElement === ta } : null;
      const sc = el.closest(".scroll"), top = sc?.scrollTop; el.innerHTML = h; el._h = h; if (sc) sc.scrollTop = top;
      const nt = draft && el.querySelector(`[data-ok-testo="${CSS.escape(draft.id)}"]`);
      if (nt) { nt.value = draft.v; if (draft.f) { nt.focus(); nt.setSelectionRange(nt.value.length, nt.value.length); } }
    }
  };
  // "Chiudi le vecchie": summary, close one at a time, then Undo for 20 s (reopens the same ones)
  async function closeOld() {
    const ad = Date.now() / 1000;
    const okA = new Set((datiOk?.voci || []).filter((v) => v.tipo === "ok" && okAperto(v)).map((v) => v.sessione));
    const ids = (datiRq?.voci || []).filter((v) => kindOf(v, ad) === "vecchia" && v.stato !== "serve_te" && !v.fai_tu && !v.aspetta_ok && !okA.has(v.sessione) && (filtroRq === "tutti" || (v.progetto?.id || "altro") === filtroRq)).map((v) => v.id);
    RQP.riepilogo = false; RQP.chiuse = [];
    for (const id of ids) { const r = await invoke("richiesta_ai_azione", { id, azione: "chiudi" }).catch(() => null); if (r?.ok !== false) RQP.chiuse.push(id); }
    toast(RQP.chiuse.length === 1 ? "1 richiesta chiusa" : `${RQP.chiuse.length} richieste chiuse`, 2200);
    clearTimeout(RQP._t); RQP._t = setTimeout(() => { RQP.chiuse = []; RQP.disegna(); }, 20000);
    caricaRichieste();
  }
  async function undoCloseOld() {
    const ids = RQP.chiuse || []; RQP.chiuse = []; clearTimeout(RQP._t);
    for (const id of ids) await invoke("richiesta_ai_azione", { id, azione: "riapri" }).catch(() => {});
    toast(ids.length === 1 ? "1 richiesta riaperta" : `${ids.length} richieste riaperte`, 2200);
    caricaRichieste();
  }
  document.addEventListener("click", async (e) => {
    const ve = e.target.closest?.("[data-rqp-vecchie]");
    if (ve) {
      const a = ve.dataset.rqpVecchie;
      if (a === "riepilogo") { RQP.riepilogo = true; return RQP.disegna(); }
      if (a === "no") { RQP.riepilogo = false; return RQP.disegna(); }
      if (a === "chiudi") { ve.disabled = true; return closeOld(); }
      if (a === "annulla") return undoCloseOld();
    }
    const le = e.target.closest?.("[data-rqp-az=\"letta\"]");
    if (le) { le.disabled = true; await invoke("richiesta_ai_azione", { id: le.dataset.id, azione: "letta" }).catch(() => {}); return caricaRichieste(); }
    const an = e.target.closest?.("[data-rqp-annulla]");
    if (an) {
      an.disabled = true;
      const r = await invoke("richiesta_ai_azione", { id: an.dataset.rqpAnnulla, azione: "riapri_juggl" }).catch((x) => ({ ok: false, msg: String(x) }));
      toast(r?.ok ? "Fatto: Juggl non la tocca più" : "Non riuscito: " + (r?.msg || ""), 2200);
      return caricaRichieste();
    }
    const t = e.target.closest?.("[data-rqp-prog]");
    if (t) {
      const k = t.dataset.rqpProg, te = +t.dataset.rqpTe;
      // with things for the user the project is open by default (remembers who closes it); without, closed by default (remembers who opens it)
      const id = te ? "chiuso:" + k : k;
      if (RQP.aperti.has(id)) RQP.aperti.delete(id); else RQP.aperti.add(id);
      save(); return RQP.disegna();
    }
    const r = e.target.closest?.("[data-rqp-sez]");
    if (r) { const id = r.dataset.rqpSez; if (RQP.aperti.has(id)) RQP.aperti.delete(id); else RQP.aperti.add(id); save(); return RQP.disegna(); }
  });

  // ---------- Settings > Requests (like the coordinator section in coordinator.js) ----------
  function settingsHtml() {
    const settings = dati?.imp || {}, cleanup = settings.pulizia_richieste !== false, lg = !!settings.loghi_online;
    const sw = (k, on, t, sub) => `<div class="imp-riga"><div class="tx"><b>${t}</b><small>${sub}</small></div><button class="interr ${on ? "on" : ""}" data-rqp-imp="${k}" role="switch" aria-checked="${on}" title="${on ? "Spegni" : "Accendi"}"></button></div>`;
    return `<h4>Richieste e progetti</h4>${sw("pulizia", cleanup, "Pulizia automatica delle richieste", cleanup ? "Accesa: segna i doppioni dello stesso progetto e chiude le richieste di chat sparite da una settimana. Tutto si annulla da Richieste." : "Spenta: Juggl non segna doppioni e non chiude niente da solo")}
      ${sw("loghi", lg, "Cerca i loghi dei progetti online", lg ? "Accesa: per i progetti senza logo Juggl apre il loro sito, una volta al giorno, solo immagini" : "Spenta: solo i file del progetto, altrimenti le iniziali")}`;
  }
  function renderSettings() {
    const f = document.getElementById("foglio");
    if (!f || !f.classList.contains("imp")) return;
    let sec = document.getElementById("imp-rqp");
    const h = settingsHtml();
    if (!sec) { const footer = f.querySelector(".piedef"); if (!footer) return; sec = document.createElement("div"); sec.className = "imp-sez"; sec.id = "imp-rqp"; footer.before(sec); }
    if (sec._h !== h) { sec.innerHTML = h; sec._h = h; }
  }
  const sheet = document.getElementById("foglio");
  if (sheet) new MutationObserver(() => { if (!document.getElementById("imp-rqp")) renderSettings(); }).observe(sheet, { childList: true, attributes: true, attributeFilter: ["class"] });
  document.addEventListener("click", async (e) => {
    const b = e.target.closest?.("[data-rqp-imp]"); if (!b) return;
    e.preventDefault(); e.stopPropagation();
    const acceso = !b.classList.contains("on"), k = b.dataset.rqpImp;
    const r = await invoke(k === "pulizia" ? "richieste_pulizia_imposta" : "loghi_online_imposta", { acceso }).catch((x) => ({ ok: false, msg: String(x) }));
    if (!r?.ok) return toast("Non riuscito: " + (r?.msg || ""));
    dati.imp = { ...(dati.imp || {}), [k === "pulizia" ? "pulizia_richieste" : "loghi_online"]: acceso };
    renderSettings();
  }, true);
})();
