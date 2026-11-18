// settings as chat (Nov 16): Settings open on a conversation. Type (or dictate with the Mac dictation)
// "spegni gli avvisi di notte", "usa Sonnet per le chat nuove", "cambia account": Juggl does it and replies with a card
// (Undo), or asks for Confirm on delicate things (account, connections, features that send outside the Mac).
// Below stay all the classic sections, collapsed, with the state in one word.
//  - the classic panel stays the one of index.html and of the modules that add sections to it (account, usage,
//    compaction, requests...): here the conversation is added on top and sections are collapsed, without rewriting them;
//  - understanding: first local rules (instant, free); if they are not enough the small model picks ONE entry and ONE value
//    from the list (regolazioni_capisci in src-tauri/src/settings_chat.rs). Only this file ever applies it;
//  - every change leaves a line in ~/tavolo/regolazioni-storia.jsonl (regolazioni_registra) and can be undone;
//  - "/impostazioni <frase>" from the "/" menu arrives here (window.impostazioniChat).
// Hooks: <script src="settings-chat.js"> in index.html, one line in commands.js, keys in profile.rs.
(() => {
  const TT = window.__TAURI__;
  if (!TT) return;
  const inv = (c, a) => TT.core.invoke(c, a);
  const E = (s) => String(s ?? "").replace(/[&<>"']/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" }[c]));
  const notice = (t, ms) => (typeof toast === "function" ? toast(t, ms) : console.log(t));
  const q = (s) => document.querySelector(s);
  const norm = (s) => String(s || "").toLowerCase().normalize("NFD").replace(/[̀-ͯ]/g, "").replace(/[’']/g, " ").replace(/\s+/g, " ").trim();
  const CHECK = '<svg viewBox="0 0 10 10"><path d="M2 5.2l2 2L8 3" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"/></svg>';
  const ARROW = '<svg viewBox="0 0 12 12" fill="none" stroke="currentColor" stroke-width="1.8"><path d="m4.5 3 3 3-3 3"/></svg>';

  const css = `
.foglio.imp.ic{width:min(600px,94vw);max-height:80vh}
.foglio.imp.ic>h3{display:none}
.ic-chat{margin:2px 0 6px}
.ic-j{color:var(--ink2);font-size:13.5px;line-height:1.5;margin:2px 0 10px}
.ic-sug{display:flex;flex-wrap:wrap;gap:6px;margin:0 0 10px}
.ic-sug button{border:1px solid var(--line);border-radius:999px;padding:4px 11px;font-size:12.5px;color:var(--ink)}
.ic-sug button:hover{background:var(--fill)}
.ic-tu{display:flex;justify-content:flex-end;margin:10px 0 8px}
.ic-tu span{background:var(--bolla);color:var(--bollaink);border-radius:16px;padding:6px 12px;font-size:13.5px;max-width:80%;word-wrap:break-word}
.ic-es{border:1px solid var(--line);border-radius:12px;padding:10px 12px;margin:0 0 6px;display:flex;gap:11px;align-items:flex-start;animation:icEntra .2s ease-out}
@keyframes icEntra{from{opacity:0;transform:translateY(3px)}to{opacity:1;transform:none}}
.ic-es .sp{flex:none;width:18px;height:18px;border-radius:50%;background:var(--ink);color:var(--bg);display:grid;place-items:center;margin-top:1px}
.ic-es .sp svg{width:10px;height:10px}
.ic-es .sp.att{background:none;box-shadow:inset 0 0 0 1.5px var(--ink);color:var(--ink);font:700 11px var(--sans)}
.ic-es .sp.vuoto{background:none;box-shadow:inset 0 0 0 1.5px var(--line);color:var(--ink3)}
.ic-es .sp.pensa::after{content:"";width:6px;height:6px;border-radius:50%;background:var(--ink3);animation:jpulsa 1.4s ease-in-out infinite}
.ic-es .tx{flex:1;min-width:0}
.ic-es b{display:block;font-weight:600;font-size:13.5px;line-height:1.35}
.ic-es small{display:block;color:var(--ink2);font-size:12.5px;line-height:1.45;margin-top:2px}
.ic-es .az{display:flex;gap:6px;flex:none;align-self:center;flex-wrap:wrap;justify-content:flex-end}
.ic-es .az button{font-size:12px;padding:4px 10px;border-radius:8px;border:1px solid var(--line);color:var(--ink2);white-space:nowrap}
.ic-es .az button:hover{background:var(--fill)}
.ic-es .az button.nero{background:var(--ink);color:var(--bg);border-color:var(--ink)}
.ic-es .az em{font-style:normal;font-size:12px;color:var(--ink3);white-space:nowrap}
.ic-es.annullata{opacity:.55}
.ic-opz{display:flex;flex-wrap:wrap;gap:6px;margin-top:8px}
.ic-opz button{font-size:12.5px;padding:4px 11px;border-radius:999px;border:1px solid var(--line);color:var(--ink)}
.ic-opz button:hover{background:var(--fill)}
.ic-opz button.on{background:var(--ink);color:var(--bg);border-color:var(--ink)}
.ic-comp{display:flex;align-items:center;gap:8px;border:1px solid var(--line);border-radius:18px;padding:4px 4px 4px 14px;margin:8px 0 4px;background:var(--bg)}
.ic-comp:focus-within{border-color:var(--ink3)}
.ic-comp input{flex:1;border:0!important;background:none!important;outline:none;font:14px var(--sans)!important;color:var(--ink);padding:6px 0!important;min-width:0}
.ic-comp input::placeholder{color:var(--ink3)}
.ic-comp .invia{width:28px;height:28px;border-radius:50%;background:var(--ink);color:var(--bg);display:grid;place-items:center;flex:none}
.ic-comp .invia:disabled{opacity:.35}
.ic-tutte{font-size:11px;font-weight:600;color:var(--ink3);margin:18px 0 4px;text-transform:uppercase;letter-spacing:.04em;border-top:1px solid var(--line);padding-top:12px}
.foglio.imp.ic .imp-sez{margin-top:0;border-bottom:1px solid var(--line)}
.foglio.imp.ic .imp-sez>h4{display:flex;align-items:center;gap:9px;margin:0;padding:9px 2px;cursor:default;text-transform:none;letter-spacing:0;font-size:13px;font-weight:500;color:var(--ink)}
.foglio.imp.ic .imp-sez>h4:hover{color:var(--ink)}
.foglio.imp.ic .imp-sez>h4 svg.ic-fr{flex:none;display:block;width:10px;height:10px;margin:0;padding:0;color:var(--ink3);transition:transform .15s}
.foglio.imp.ic .imp-sez:not(.ic-chiusa)>h4 .ic-fr{transform:rotate(90deg)}
.foglio.imp.ic .imp-sez>h4 .ic-st{margin-left:auto;color:var(--ink3);font-size:12px;font-weight:400;white-space:nowrap;overflow:hidden;text-overflow:ellipsis;max-width:55%}
.foglio.imp.ic .imp-sez.ic-chiusa>:not(h4){display:none!important}
.foglio.imp.ic .imp-sez:not(.ic-chiusa){padding-bottom:10px}
.foglio.imp.ic .imp-sez.ic-lampo>h4{animation:icLampo 1.2s ease-out}
@keyframes icLampo{0%{background:var(--fill2)}100%{background:none}}
.ic-ore{display:flex;align-items:center;gap:6px;font-size:12.5px;color:var(--ink2)}
.ic-ore input{width:64px!important;height:28px;border-radius:7px;border:1px solid var(--line);background:var(--fill);padding:0 6px!important;font:12.5px var(--sans)!important;color:var(--ink)}
`;
  const st = document.createElement("style"); st.id = "impostazioni-chat-css"; st.textContent = css; document.head.appendChild(st);

  // ---------- state ----------
  const S = { conv: [], aperte: new Set(), bozza: "", fuoco: false, P: null, R: null, n: 0 };
  async function load() {
    const [P, R] = await Promise.all([inv("profilo").catch(() => null), inv("regolazioni_stato").catch(() => null)]);
    if (P) S.P = P; if (R) S.R = R;
  }
  const featureState = (id) => (S.P?.funzioni || []).find((x) => x.id === id);
  const MOD = { fable: "Fable", opus: "Opus", sonnet: "Sonnet", haiku: "Haiku", predefinito: "il modello predefinito" };
  const fmtTime = (t) => String(t || "").replace(/^0(\d)/, "$1").replace(/:00$/, "");
  const record = (id, da, a, frase) => inv("regolazioni_registra", { riga: { id, da, a, frase: String(frase || "").slice(0, 200) } }).catch(() => {});

  // ---------- understanding ----------
  const NEG = /\b(spegni|spengi|disattiva|togli|disabilita|niente|basta|non voglio|stop|silenzia|zittisci|via)\b/;
  const POS = /\b(accendi|attiva|abilita|riaccendi|metti|voglio|usa|rimetti)\b/;
  const isOn = (f, baseVal = true) => (NEG.test(f) ? false : POS.test(f) ? true : baseVal);
  function timeRange(f) {
    const m = f.match(/dall?e?\s*(\d{1,2})(?:[:.](\d{2}))?\s*(?:alle?|fino alle?|a)\s*(\d{1,2})(?:[:.](\d{2}))?/);
    if (!m) return null;
    const h = (a, b) => `${String(+a).padStart(2, "0")}:${b || "00"}`;
    if (+m[1] > 23 || +m[3] > 23) return null;
    return { da: h(m[1], m[2]), a: h(m[3], m[4]) };
  }
  const FEATURES = [
    ["iphone", /iphone|telefono|cellulare/], ["mac", /pulizia del mac/], ["spazio", /spazio|disco/], ["cmux", /cmux|terminale/], ["pc", /modell\w* local|ollama|pc dello studio/]];
  const OUTSIDE_FEATURES = new Set([
    "iphone"]);   // they send or open something outside the Mac: confirmation to turn them on

  // Each entry: id, nome, valori (for the small model), prendi(normalized sentence) -> {v, extra} | null,
  // delicata(v) -> {titolo, dett, si} | null, fai(v, extra, frase) -> {titolo, dett, annulla?} (annulla = function).
  const ENTRIES = [
    { id: "avvisi_notte", nome: "Avvisi di notte (silenzio su Mac e iPhone)", valori: ["acceso", "spento"],
      prendi(f) {
        const nightOn = /(avvis|notific)/.test(f) && /(nott|sera|dorm)/.test(f), quietState = /(silenzio|non disturbare)/.test(f);
        if (!nightOn && !quietState) return null;
        // "spegni gli avvisi di notte" = quiet on; "riaccendi gli avvisi di notte" = quiet off
        const v = quietState ? (isOn(f) ? "acceso" : "spento") : (NEG.test(f) ? "acceso" : POS.test(f) ? "spento" : "acceso");
        return { v, extra: timeRange(f) };
      },
      async fai(v, extra) {
        const prevVal = { ...(S.R?.avvisi_notte || { acceso: false, da: "23:00", a: "08:00" }) };
        const fresh = { acceso: v === "acceso", da: extra?.da || prevVal.da || "23:00", a: extra?.a || prevVal.a || "08:00" };
        const r = await inv("profilo_imposta", { chiave: "avvisi_notte", valore: fresh });
        if (!r?.ok) throw new Error(r?.msg || "non salvato");
        return { titolo: fresh.acceso ? `Avvisi spenti di notte, dalle ${fmtTime(fresh.da)} alle ${fmtTime(fresh.a)}` : "Avvisi di nuovo accesi anche di notte",
          dett: fresh.acceso ? "Sul Mac e sull'iPhone. Quello che arriva resta nell'elenco di Juggl." : "", da: prevVal, a: fresh,
          altro: fresh.acceso ? [["Cambia orario", () => { openSection("Avvisi"); }]] : null,
          annulla: () => inv("profilo_imposta", { chiave: "avvisi_notte", valore: prevVal }) };
      } },
    { id: "modello_nuove", nome: "Modello delle chat nuove", valori: ["fable", "opus", "sonnet", "haiku", "predefinito"],
      prendi(f) {
        const m = f.match(/\b(fable|opus|sonnet|haiku)\b/), def = /(predefinit|come prima|normale|di default)/.test(f);
        const incoming = /(chat|session)\w*\s+nuov|nuov\w*\s+(chat|session)|di (default|base)|predefinit|sempre/.test(f);
        if (!(m || (def && incoming))) return null;
        if (!incoming && !/\b(usa|metti|voglio)\b/.test(f)) return null;
        return { v: m ? m[1] : "predefinito" };
      },
      async fai(v) {
        const prevVal = S.R?.modello_nuove || "";
        const r = await inv("profilo_imposta", { chiave: "modello_nuove", valore: v === "predefinito" ? "" : v });
        if (!r?.ok) throw new Error(r?.msg || "non salvato");
        return { titolo: v === "predefinito" ? "Le chat nuove partono con il modello predefinito" : `Le chat nuove partono con ${MOD[v]}`,
          dett: "Quelle già aperte restano come sono. Per una chat aperta scrivi /modello lì.", da: prevVal, a: v,
          annulla: () => inv("profilo_imposta", { chiave: "modello_nuove", valore: prevVal }) };
      } },
    { id: "eco", nome: "Modalità Eco (chat su Sonnet per risparmiare quota)", valori: ["acceso", "spento"],
      prendi(f) { if (!/\beco\b|risparmi/.test(f)) return null; return { v: isOn(f) ? "acceso" : "spento" }; },
      async fai(v) {
        const prevVal = !!(typeof dati !== "undefined" && dati.eco?.attiva);
        const r = await inv("eco_imposta", { attiva: v === "acceso", auto: null });
        if (r && r.ok === false) throw new Error(r.msg || "non riuscito");
        return { titolo: v === "acceso" ? "Modalità Eco accesa" : "Modalità Eco spenta",
          dett: v === "acceso" ? "Le chat non fissate passano a Sonnet con risposte più brevi." : "", da: prevVal, a: v === "acceso",
          annulla: () => inv("eco_imposta", { attiva: prevVal, auto: null }) };
      } },
    { id: "testo", nome: "Grandezza del testo nelle chat", valori: ["piccolo", "normale", "grande"],
      prendi(f) {
        if (!/(testo|caratter|scritt|font|lettere)/.test(f)) return null;
        const t = window.tipografia; if (!t) return null;
        const order = t.misure(), i = order.indexOf(t.attuale());
        if (/(piu grand|ingrand|aumenta|piu gross)/.test(f)) return { v: order[Math.min(order.length - 1, i + 1)] };
        if (/(piu piccol|rimpicciol|riduc|diminuisc)/.test(f)) return { v: order[Math.max(0, i - 1)] };
        const m = f.match(/\b(piccol|normal|grand)/); return m ? { v: { piccol: "piccolo", normal: "normale", grand: "grande" }[m[1]] } : null;
      },
      async fai(v) {
        const prevVal = window.tipografia.attuale(); window.tipografia.imposta(v, true);
        return { titolo: `Testo ${v}`, dett: "Anche con ⌘+ e ⌘−.", da: prevVal, a: v, annulla: async () => window.tipografia.imposta(prevVal, true) };
      } },
    { id: "larghezza", nome: "Larghezza della colonna dei messaggi", valori: ["stretta", "normale", "larga"],   // width (Nov 16)
      prendi(f) {
        if (!/(larg|strett|colonna|spazio|tutto lo schermo|righe)/.test(f) || !window.tipografia?.larghezza) return null;
        if (/(testo|caratter|scritt|font)/.test(f) && !/(larg|strett|colonna)/.test(f)) return null;
        if (/(piu larg|allarga|tutta la larghezza|tutto lo spazio|tutto lo schermo|\blarga\b|\blargo\b)/.test(f)) return { v: "larga" };
        if (/(piu strett|restring|\bstretta\b|\bstretto\b)/.test(f)) return { v: "stretta" };
        if (/(normal|media|di base)/.test(f)) return { v: "normale" };
        return null;
      },
      async fai(v) {
        const prevVal = window.tipografia.attualeLarghezza(); window.tipografia.larghezza(v, true);
        return { titolo: `Colonna ${v}`, dett: { stretta: "Circa 74 caratteri per riga.", normale: "Circa 100 caratteri per riga; foto, tabelle e codice più larghi.", larga: "Circa 140 caratteri per riga: per monitor grandi." }[v] + " Anche con /larghezza.", da: prevVal, a: v, annulla: async () => window.tipografia.larghezza(prevVal, true) };
      } },
    { id: "nome", nome: "Il tuo nome", valori: [],
      prendi(f, raw) { const m = raw.match(/(?:chiamami|mi chiamo|il mio nome (?:e|è))\s+(.{1,40})$/i); return m ? { v: m[1].trim().replace(/[.!]+$/, "") } : null; },
      async fai(v) {
        const prevVal = S.P?.nome_impostato ? S.P.nome : "";
        const r = await inv("profilo_imposta", { chiave: "nome", valore: v }); if (!r?.ok) throw new Error(r?.msg || "non salvato");
        return { titolo: `Ti chiamo ${v}`, dett: "Juggl lo usa quando parla di te alle chat.", da: prevVal, a: v, annulla: () => inv("profilo_imposta", { chiave: "nome", valore: prevVal }) };
      } },
    ...FEATURES.map(([fid, rx]) => ({ id: "funzioni." + fid, nome: "", valori: ["acceso", "spento"], fid,
      prendi(f) { if (!rx.test(f) || !featureState(fid)?.disponibile) return null; if (/\bcollega\b/.test(f) && fid === "iphone") return { v: "acceso" }; return (NEG.test(f) || POS.test(f)) ? { v: isOn(f) ? "acceso" : "spento" } : null; },
      delicata(v) { return v === "acceso" && OUTSIDE_FEATURES.has(fid) ? { titolo: `Accendere ${featureState(fid)?.titolo || fid}?`, dett: fid === "iphone" ? "Apre il collegamento con l'app iPhone sulla rete di questo Mac." : "Juggl manderà avvisi e messaggi fuori da questo Mac.", si: "Accendi" } : null; },
      async fai(v) {
        const prevVal = !!featureState(fid)?.accesa;
        const r = await inv("profilo_imposta", { chiave: "funzioni." + fid, valore: v === "acceso" }); if (!r?.ok) throw new Error(r?.msg || "non salvato");
        if (typeof dati !== "undefined") dati.funzioni = r.attive;
        const t = featureState(fid)?.titolo || fid;
        return { titolo: `${t}: ${v === "acceso" ? "accesa" : "spenta"}`, dett: fid === "iphone" && v === "acceso" ? "Il codice da inquadrare è nella sezione iPhone qui sotto." : "Alcune funzioni partono al prossimo avvio.", da: prevVal, a: v === "acceso",
          altro: fid === "iphone" && v === "acceso" ? [["Mostra il codice", () => openSection("iPhone")]] : null,
          annulla: () => inv("profilo_imposta", { chiave: "funzioni." + fid, valore: prevVal }) };
      } })),
    { id: "account_claude", nome: "Account Claude (cambia, esci)", valori: ["cambia", "esci"],
      prendi(f) {
        if (/(juggl|licenza)/.test(f)) return null;
        if (/\b(esci|logout|disconnett)/.test(f) && /account|claude/.test(f)) return { v: "esci" };
        if (/(cambia|altro|nuovo|un altro)\s+account|account (di )?claude|\blogin\b|entra con/.test(f)) return { v: "cambia" };
        return null;
      },
      delicata(v) { return v === "cambia" ? { titolo: "Cambiare account Claude?", dett: "Le chat al lavoro finiscono la risposta con l'account di adesso, poi ripartono con il nuovo. Si apre il login di Claude.", si: "Cambia account" }
        : { titolo: "Uscire dall'account Claude?", dett: "Le chat non potranno rispondere finché non rientri.", si: "Esci" }; },
      async fai(v) { close(); if (v === "cambia") window.accessoAvvia?.(); else window.accessoEsci?.(); return { titolo: v === "cambia" ? "Login di Claude aperto" : "Uscita in corso", dett: "" }; } },
    { id: "account_juggl", nome: "Account di Juggl (esci)", valori: ["esci"],
      prendi(f) { return /(esci|logout)/.test(f) && /(juggl|licenza)/.test(f) ? { v: "esci" } : null; },
      delicata() { return { titolo: "Uscire dall'account di Juggl?", dett: "Juggl si blocca finché non rientri con email e codice. Le chat già al lavoro continuano.", si: "Esci" }; },
      async fai() { await inv("account_esci"); close(); return { titolo: "Uscito dall'account di Juggl", dett: "" }; } },
    { id: "collegamento", nome: "Collegamento con Claude Code (hook)", valori: ["collega", "togli"],
      prendi(f) { if (!/(claude code|hook|collegamento con claude)/.test(f)) return null; return { v: /(togli|scollega|disinstalla|rimuovi|stacca)/.test(f) ? "togli" : "collega" }; },
      delicata(v) { return v === "togli" ? { titolo: "Togliere il collegamento con Claude Code?", dett: "Tolgo solo gli hook di Juggl, dopo averne fatto una copia. Le chat nuove del terminale non compariranno più qui.", si: "Togli" } : null; },
      async fai(v) {
        const r = await inv(v === "togli" ? "hook_togli" : "hook_installa"); if (!r?.ok) throw new Error(r?.msg || "non riuscito");
        return { titolo: v === "togli" ? "Collegamento tolto" : "Collegato a Claude Code", dett: "", annulla: () => inv(v === "togli" ? "hook_installa" : "hook_togli") };
      } },
    { id: "permessi", nome: "Permessi delle chat (cosa possono fare senza chiederti)", valori: ["apri"],
      prendi(f) { return /permess/.test(f) ? { v: "apri" } : null; },
      async fai() { const ok = openSection("Permessi"); return { titolo: ok ? "I permessi sono qui sotto" : "Permessi", dett: "Li scegli tu: sono una cosa delicata, non li cambio a parole." }; } },
    { id: "aggiornamenti", nome: "Cerca aggiornamenti di Juggl", valori: ["cerca"],
      prendi(f) { return /(aggiorn|nuova versione|ultima versione)/.test(f) ? { v: "cerca" } : null; },
      async fai() { TT.event.emit("aggiornamenti"); return { titolo: "Cerco aggiornamenti", dett: "Se c'è una versione nuova te lo dico qui sotto, nella sezione Aggiornamenti." }; } },
    { id: "consumi", nome: "Consumi e quota di Claude", valori: ["apri"],
      prendi(f) { return /(consum|quota|limit)/.test(f) ? { v: "apri" } : null; },
      async fai() { close(); if (typeof apriConsumi === "function") apriConsumi(); return { titolo: "Consumi aperti", dett: "" }; } },
    { id: "aiuto", nome: "Giro di Juggl, benvenuto, importa progetti", valori: ["giro", "benvenuto", "importa"],
      prendi(f) { return /\bgiro\b|scorciatoie/.test(f) ? { v: "giro" } : /benvenuto/.test(f) ? { v: "benvenuto" } : /importa/.test(f) ? { v: "importa" } : null; },
      async fai(v) { close(); inv("benvenuto_apri", { passo: v === "benvenuto" ? "" : v }); return { titolo: "Aperto", dett: "" }; } },
  ];
  // entries added by other files (e.g. ui/snap.js): same shape {id, nome, valori, prendi, delicata?, fai}
  const allEntries = () => ENTRIES.concat(window.impostazioniVociExtra || []);
  const voce = (id) => allEntries().find((v) => v.id === id);
  function understandLocally(raw) {
    const f = norm(raw);
    for (const v of allEntries()) { const r = v.prendi(f, raw); if (r) return { voce: v, ...r }; }
    return null;
  }
  // the 3 closest entries (words in common with the name), for "non ho capito"
  function vicine(raw) {
    const pp = new Set(norm(raw).split(" ").filter((p) => p.length > 3));
    return allEntries().filter((v) => v.nome && v.valori.length).map((v) => ({ v, n: norm(v.nome).split(/\W+/).filter((p) => pp.has(p)).length })).sort((a, b) => b.n - a.n).slice(0, 3).map((x) => x.v);
  }

  // ---------- doing ----------
  async function execute(v, valore, extra, frase, item) {
    item.tipo = "pensa"; item.titolo = "Un attimo…"; renderChat();
    try {
      const r = await v.fai(valore, extra, frase);
      Object.assign(item, { tipo: "fatto", titolo: r.titolo, dett: r.dett || "", annulla: r.annulla || null, altro: r.altro || null, id: v.id });
      record(v.id, r.da ?? null, r.a ?? valore, frase);
      notice(r.titolo, 2200);
    } catch (e) {
      Object.assign(item, { tipo: "errore", titolo: "Non ci sono riuscito", dett: String(e?.message || e || "") });
    }
    await load(); renderChat();
  }
  async function manda(frase) {
    frase = String(frase || "").trim(); if (!frase) return;
    S.conv.push({ tipo: "tu", testo: frase }); S.bozza = "";
    const item = { k: ++S.n, tipo: "pensa", titolo: "Un attimo…" }; S.conv.push(item); renderChat(true);
    let c = understandLocally(frase);
    if (!c) {
      // the small model only picks entry and value from the list (never a free name)
      const voci = allEntries().filter((v) => v.valori.length && (!v.fid || featureState(v.fid)?.disponibile)).map((v) => ({ id: v.id, nome: v.nome || featureState(v.fid)?.titolo || v.id, valori: v.valori }));
      const r = await inv("regolazioni_capisci", { frase, voci }).catch(() => null);
      if (r?.ok && voce(r.id)) c = { voce: voce(r.id), v: r.valore, extra: null };
    }
    if (!c) {
      Object.assign(item, { tipo: "nc", titolo: "Non ho trovato un'impostazione per questo", dett: "Forse cercavi:", sug: vicine(frase).map((v) => v.nome || featureState(v.fid)?.titolo).filter(Boolean) });
      return renderChat(true);
    }
    const del = c.voce.delicata?.(c.v);
    if (del) { Object.assign(item, { tipo: "conf", titolo: del.titolo, dett: del.dett, si: del.si, t0: Date.now(), fai: () => execute(c.voce, c.v, c.extra, frase, item) }); return renderChat(true); }
    execute(c.voce, c.v, c.extra, frase, item);
  }

  // ---------- suggestions ----------
  function suggest() {
    const n = S.R?.avvisi_notte?.acceso, eco = typeof dati !== "undefined" && dati.eco?.attiva;
    const everything = [
      ["avvisi_notte", n ? "Riaccendi gli avvisi di notte" : "Spegni gli avvisi di notte"],
      ["modello_nuove", "Modello delle chat nuove", "scelta"],
      ["eco", eco ? "Spegni Eco" : "Accendi Eco"],
      ["testo", window.tipografia?.attuale() === "grande" ? "Testo normale" : "Testo più grande"],
      ["funzioni.iphone", "Collega l'iPhone", null, () => featureState("iphone")?.disponibile && !featureState("iphone")?.accesa],
      ["aggiornamenti", "Cerca aggiornamenti"],
      ["account_claude", "Cambia account Claude"],
    ].filter((x) => !x[3] || x[3]());
    // recently changed ones first
    const recent = (S.R?.storia || []).map((r) => r.id);
    everything.sort((a, b) => { const ia = recent.indexOf(a[0]), ib = recent.indexOf(b[0]); return (ia < 0 ? 99 : ia) - (ib < 0 ? 99 : ib); });
    return everything.slice(0, 6);
  }

  // ---------- drawing ----------
  function cardHtml(x, i) {
    if (x.tipo === "tu") return `<div class="ic-tu"><span>${E(x.testo)}</span></div>`;
    const icon = x.tipo === "fatto" ? `<span class="sp">${CHECK}</span>` : x.tipo === "conf" ? '<span class="sp att">!</span>' : x.tipo === "pensa" ? '<span class="sp vuoto pensa"></span>' : '<span class="sp vuoto">?</span>';
    let az = "";
    if (x.tipo === "fatto") az = (x.altro || []).map(([t], j) => `<button data-ic-altro="${i}:${j}">${E(t)}</button>`).join("") + (x.annullata ? "<em>Annullato</em>" : x.annulla ? `<button data-ic-annulla="${i}">Annulla</button>` : "");
    if (x.tipo === "conf") az = x.esito ? `<em>${E(x.esito)}</em>` : `<button data-ic-no="${i}">Annulla</button><button class="nero" data-ic-si="${i}">${E(x.si || "Conferma")}</button>`;
    const opts = x.tipo === "scelta" ? `<div class="ic-opz">${x.opz.map(([t, f, on], j) => `<button class="${on ? "on" : ""}" data-ic-opz="${i}:${j}">${E(t)}</button>`).join("")}</div>`
      : x.tipo === "nc" && x.sug?.length ? `<div class="ic-opz">${x.sug.map((t) => `<button data-ic-frase="${E(t)}">${E(t)}</button>`).join("")}</div>` : "";
    return `<div class="ic-es${x.annullata ? " annullata" : ""}">${icon}<div class="tx"><b>${E(x.titolo)}</b>${x.dett ? `<small>${E(x.dett)}</small>` : ""}${opts}</div>${az ? `<div class="az">${az}</div>` : ""}</div>`;
  }
  function chatHtml() {
    const empty = !S.conv.length;
    return `${empty ? `<p class="ic-j">Dimmi cosa vuoi cambiare. Le cose delicate te le confermo prima.</p>` : ""}
      ${S.conv.map(cardHtml).join("")}
      ${empty ? `<div class="ic-sug">${suggest().map(([id, t, kind]) => `<button data-ic-sug="${E(id)}" data-ic-tipo="${kind || ""}">${E(t)}</button>`).join("")}</div>` : ""}
      <form class="ic-comp" id="ic-form"><input id="ic-in" placeholder="${empty ? "Scrivi cosa cambiare" : "Altro da cambiare?"}" autocomplete="off" spellcheck="false" value="${E(S.bozza)}"><button class="invia" type="submit" aria-label="Manda"${S.bozza.trim() ? "" : " disabled"}><svg width="13" height="13" viewBox="0 0 14 14" fill="none" stroke="currentColor" stroke-width="2"><path d="M7 12V2M3 6l4-4 4 4"/></svg></button></form>`;
  }
  function renderChat(focusIn) {
    const el = q("#imp-chat"); if (!el) return;
    const inp = q("#ic-in"), had = focusIn || (inp && document.activeElement === inp);
    el.innerHTML = chatHtml();
    if (had) { const i = q("#ic-in"); i?.focus(); }
  }

  // classic sections: collapsed, with the state in one word
  const sectionTitle = (s) => (s.querySelector(":scope > h4")?.dataset.icT || s.querySelector(":scope > h4")?.textContent || "").trim();
  function sectionState(s) {
    const sw = [...s.querySelectorAll(".interr")];
    if (sw.length === 1) return sw[0].classList.contains("on") ? "Acceso" : "Spento";
    if (sw.length > 1) { const on = sw.filter((x) => x.classList.contains("on")).length; return `${on} ${on === 1 ? "accesa" : "accese"} su ${sw.length}`; }
    const seg = s.querySelector(".arch-seg button.on, .ic-opz button.on"); if (seg) return seg.textContent.trim();
    const inp = s.querySelector(".imp-riga input[type=text]"); if (inp) return inp.value || "";
    // a single row: its title is the state (account, version, connected); with more rows nothing
    const rowEls = s.querySelectorAll(".imp-riga");
    return rowEls.length === 1 ? (rowEls[0].querySelector("b")?.textContent || "").trim().slice(0, 40) : "";
  }
  function arrangeSections(f) {
    for (const s of f.querySelectorAll(":scope > .imp-sez")) {
      const h = s.querySelector(":scope > h4"); if (!h) continue;
      if (!h.dataset.icT) { h.dataset.icT = h.textContent.trim(); h.setAttribute("role", "button"); h.tabIndex = 0; }
      const t = h.dataset.icT, statusText = sectionState(s);
      const html = `${ARROW.replace("<svg ", '<svg class="ic-fr" ')}<span>${E(t)}</span><span class="ic-st">${E(statusText)}</span>`;
      if (h._ic !== html) { h.innerHTML = html; h._ic = html; }
      const closed = !S.aperte.has(t);
      if (s.classList.contains("ic-chiusa") !== closed) s.classList.toggle("ic-chiusa", closed);
      if (h.getAttribute("aria-expanded") !== String(!closed)) h.setAttribute("aria-expanded", String(!closed));
    }
  }
  function openSection(nome) {
    const f = q("#foglio.imp"); if (!f) return false;
    const s = [...f.querySelectorAll(":scope > .imp-sez")].find((x) => norm(sectionTitle(x)).startsWith(norm(nome)));
    if (!s) return false;
    S.aperte.add(sectionTitle(s)); arrangeSections(f);
    s.classList.add("ic-lampo"); setTimeout(() => s.classList.remove("ic-lampo"), 1300);
    s.scrollIntoView({ block: "nearest", behavior: "smooth" });
    return true;
  }
  function close() { if (typeof chiudiNuova === "function") chiudiNuova(); }

  // new sections in the classic panel: Chat (text, model of new chats) and Notifications (at night)
  function ownSections(f) {
    if (f.querySelector("#imp-ic-chat")) return;
    const anchor = [...f.querySelectorAll(":scope > .imp-sez")].find((x) => /^tu$/i.test(sectionTitle(x))) || f.querySelector(":scope > .imp-sez");
    if (!anchor) return;
    const n = S.R?.avvisi_notte || { acceso: false, da: "23:00", a: "08:00" }, mn = S.R?.modello_nuove || "", t = window.tipografia?.attuale() || "normale";
    const chip = (a, v, txt, on) => `<button class="${on ? "on" : ""}" data-ic-imposta="${a}" data-v="${v}">${txt}</button>`;
    anchor.insertAdjacentHTML("afterend", `
      <div class="imp-sez" id="imp-ic-chat"><h4>Chat</h4>
        <div class="imp-riga"><div class="tx"><b>Modello delle chat nuove</b><small>Quando apri una chat senza scegliere il modello</small>
          <div class="ic-opz">${[["", "Predefinito"], ["opus", "Opus"], ["sonnet", "Sonnet"], ["haiku", "Haiku"], ["fable", "Fable"]].map(([v, x]) => chip("modello", v, x, mn === v)).join("")}</div></div></div>
        <div class="imp-riga"><div class="tx"><b>Larghezza della colonna</b><small>Stretta circa 74 caratteri per riga, Normale circa 100, Larga circa 140. Anche con /larghezza</small>
          <div class="ic-opz">${["stretta", "normale", "larga"].map((v) => chip("larghezza", v, v[0].toUpperCase() + v.slice(1), (window.tipografia?.attualeLarghezza?.() || "larga") === v)).join("")}</div></div></div>
        <div class="imp-riga"><div class="tx"><b>Grandezza del testo</b><small>Anche con ⌘+ e ⌘−</small>
          <div class="ic-opz">${["piccolo", "normale", "grande"].map((v) => chip("testo", v, v[0].toUpperCase() + v.slice(1), t === v)).join("")}</div></div></div>
      </div>
      <div class="imp-sez" id="imp-ic-avvisi"><h4>Avvisi</h4>
        <div class="imp-riga"><div class="tx"><b>Silenzio di notte</b><small>Niente avvisi sul Mac e sull'iPhone; quello che arriva resta nell'elenco</small>
          <div class="ic-ore" style="margin-top:8px">Dalle <input type="text" id="ic-da" value="${E(n.da)}" maxlength="5"> alle <input type="text" id="ic-a" value="${E(n.a)}" maxlength="5"></div></div><button class="interr ${n.acceso ? "on" : ""}" data-ic-notte role="switch" aria-checked="${n.acceso}"></button></div>
      </div>`);
  }

  function mount() {
    const f = q("#foglio");
    if (!f) return;
    // only real changes: classList.add of a class already present counts as a mutation and would restart the observer
    if (!f.classList.contains("imp")) { if (f.classList.contains("ic")) f.classList.remove("ic"); return; }
    if (!f.classList.contains("ic")) f.classList.add("ic");
    if (!f.querySelector("#imp-chat")) {
      const h3 = f.querySelector(":scope > h3");
      const box = document.createElement("div"); box.id = "imp-chat"; box.className = "ic-chat";
      if (h3) h3.after(box); else f.prepend(box);
      box.innerHTML = chatHtml();
      const lab = document.createElement("div"); lab.className = "ic-tutte"; lab.textContent = "Tutte le impostazioni"; box.after(lab);
      if (S.fuoco) q("#ic-in")?.focus();
    }
    ownSections(f);
    arrangeSections(f);
  }
  let applying = false;
  const sheet = q("#foglio");
  if (sheet) new MutationObserver(() => { if (applying) return; applying = true; try { mount(); } finally { applying = false; } })
    .observe(sheet, { childList: true, subtree: true, attributes: true, attributeFilter: ["class"] });
  TT.event.listen("impostazioni", () => { S.fuoco = true; load().then(() => { const f = q("#foglio.imp"); f?.querySelector("#imp-ic-chat")?.remove(); f?.querySelector("#imp-ic-avvisi")?.remove(); mount(); renderChat(true); }); });

  // ---------- events ----------
  document.addEventListener("input", (e) => {
    if (e.target.id !== "ic-in") return;
    S.bozza = e.target.value; const b = q("#ic-form .invia"); if (b) b.disabled = !S.bozza.trim();
  });
  document.addEventListener("submit", (e) => { if (e.target.id !== "ic-form") return; e.preventDefault(); manda(q("#ic-in")?.value); });
  document.addEventListener("focusin", (e) => { if (e.target.id === "ic-in") S.fuoco = true; });
  document.addEventListener("keydown", (e) => {
    const h = e.target.closest?.(".foglio.imp.ic .imp-sez > h4"); if (!h || (e.key !== "Enter" && e.key !== " ")) return;
    e.preventDefault(); h.click();
  });
  document.addEventListener("change", async (e) => {
    if (e.target.id !== "ic-da" && e.target.id !== "ic-a") return;
    const n = { ...(S.R?.avvisi_notte || { acceso: false }), da: q("#ic-da").value, a: q("#ic-a").value };
    const r = await inv("profilo_imposta", { chiave: "avvisi_notte", valore: n });
    await load();
    notice(r?.ok ? `Silenzio dalle ${fmtTime(S.R.avvisi_notte.da)} alle ${fmtTime(S.R.avvisi_notte.a)}` : "Orario non valido: scrivi per esempio 23:00", 2400);
    if (r?.ok) record("avvisi_notte", null, S.R.avvisi_notte, "orario");
  });
  document.addEventListener("click", async (e) => {
    if (!e.target.closest?.("#foglio.imp.ic")) return;
    const h = e.target.closest(".imp-sez > h4");
    if (h && h.parentElement.parentElement?.id === "foglio") {
      const t = h.dataset.icT; if (!t) return;
      S.aperte.has(t) ? S.aperte.delete(t) : S.aperte.add(t); return arrangeSections(q("#foglio"));
    }
    const sg = e.target.closest("[data-ic-sug]");
    if (sg) {
      if (sg.dataset.icTipo === "scelta") {
        const mn = S.R?.modello_nuove || "";
        S.conv.push({ tipo: "scelta", titolo: "Con quale modello partono le chat nuove?", dett: "Quelle già aperte restano come sono.",
          opz: [["Predefinito", "predefinito", !mn], ["Opus", "opus", mn === "opus"], ["Sonnet", "sonnet", mn === "sonnet"], ["Haiku", "haiku", mn === "haiku"], ["Fable", "fable", mn === "fable"]] });
        return renderChat();
      }
      return manda(sg.textContent);
    }
    const op = e.target.closest("[data-ic-opz]");
    if (op) {
      const [i, j] = op.dataset.icOpz.split(":").map(Number), x = S.conv[i], v = x?.opz?.[j]; if (!v) return;
      const vc = voce(x.voce || "modello_nuove"), item = { k: ++S.n, tipo: "pensa", titolo: "" }; S.conv.splice(i, 1, item);
      const del = vc.delicata?.(v[1]);
      if (del) { Object.assign(item, { tipo: "conf", titolo: del.titolo, dett: del.dett, si: del.si, t0: Date.now(), fai: () => execute(vc, v[1], null, v[0], item) }); return renderChat(); }
      return execute(vc, v[1], null, (x.voce ? vc.nome + ": " : "Chat nuove: ") + v[0], item);
    }
    // "Forse cercavi": tap on an entry = card with its values
    const fr = e.target.closest("[data-ic-frase]");
    if (fr) {
      const v = allEntries().find((x) => (x.nome || featureState(x.fid)?.titolo) === fr.dataset.icFrase); if (!v) return;
      S.conv.push({ tipo: "tu", testo: fr.dataset.icFrase });
      S.conv.push({ k: ++S.n, tipo: "scelta", titolo: v.nome || featureState(v.fid)?.titolo, dett: "Cosa vuoi fare?", voce: v.id, opz: v.valori.map((val) => [val[0].toUpperCase() + val.slice(1), val]) });
      return renderChat();
    }
    const an = e.target.closest("[data-ic-annulla]");
    if (an) {
      const x = S.conv[+an.dataset.icAnnulla]; if (!x?.annulla || x.annullata) return;
      try { await x.annulla(); x.annullata = true; record(x.id, null, "annullato", "annulla"); notice("Annullato", 1800); } catch { notice("Non riuscito", 2000); }
      await load(); return renderChat();
    }
    const al = e.target.closest("[data-ic-altro]"); if (al) { const [i, j] = al.dataset.icAltro.split(":").map(Number); S.conv[i]?.altro?.[j]?.[1](); return; }
    const si = e.target.closest("[data-ic-si]");
    if (si) { const x = S.conv[+si.dataset.icSi]; if (!x || x.esito || Date.now() - x.t0 < 400) return; x.fai(); return; }
    const no = e.target.closest("[data-ic-no]"); if (no) { const x = S.conv[+no.dataset.icNo]; if (x) { x.esito = "Lasciato com'era"; record(null, null, "rifiutato", x.titolo); } return renderChat(); }
    const ni = e.target.closest("[data-ic-notte]");
    if (ni) { const on = !ni.classList.contains("on"); S.conv.push({ tipo: "tu", testo: on ? "Spegni gli avvisi di notte" : "Riaccendi gli avvisi di notte" }); const it = { k: ++S.n, tipo: "pensa", titolo: "" }; S.conv.push(it); await execute(voce("avvisi_notte"), on ? "acceso" : "spento", null, "interruttore", it); q("#imp-ic-avvisi")?.remove(); q("#imp-ic-chat")?.remove(); return mount(); }
    const ip = e.target.closest("[data-ic-imposta]");
    if (ip) {
      const a = ip.dataset.icImposta, v = ip.dataset.v, it = { k: ++S.n, tipo: "pensa", titolo: "" }; S.conv.push(it);
      await execute(voce(a === "testo" ? "testo" : a === "larghezza" ? "larghezza" : "modello_nuove"), a === "testo" || a === "larghezza" ? v : v || "predefinito", null, "pannello", it);
      q("#imp-ic-avvisi")?.remove(); q("#imp-ic-chat")?.remove(); return mount();
    }
  });
  // for tests (tools/test-settings.mjs) and to see what it understood: id and value, no actions
  window.impostazioniCapisci = (f) => { const c = understandLocally(f); return c ? { id: c.voce.id, v: c.v, extra: c.extra || null } : null; };
  // "/impostazioni <frase>": opens Settings and sends the sentence
  window.impostazioniChat = (frase) => {
    TT.event.emit("impostazioni");
    let n = 0; const t = setInterval(() => { if (q("#imp-chat") || ++n > 40) { clearInterval(t); if (q("#imp-chat")) manda(frase); } }, 50);
  };
  load();
})();
