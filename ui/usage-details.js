// Full usage (0.1.76): "where and when the tokens were spent", inside the Usage view.
// Real data from Claude transcripts (command consumi_dettaglio, see quota::dettaglio): by thread type, by model,
// by hour and by weekday of the current week, by chat and by project, with warnings. No estimates.
// "Token share" = the share of WEIGHTED tokens (input 1, output 5, cache read 0.1, cache write 1.25; by model
// Haiku 0.33, Sonnet 1, Opus and Fable 1.67) over the week total. It is not the official percentage: Anthropic only gives the total.
// Hook in index.html: <div id="consd"> inside disegnaConsumi, a CONSD.monta() call at the end of the function and the <script>.
(() => {
  if (!window.__TAURI__) return;
  const KINDS = [["chat", "Chat principali"], ["sotto_agenti", "Sotto-agenti"], ["aiutanti", "Aiutanti di Juggl"], ["regia", "Regia"]];
  const MODELS = [["opus", "Opus"], ["sonnet", "Sonnet"], ["haiku", "Haiku"], ["fable", "Fable"]];
  const esc = (x) => String(x ?? "").replace(/[&<>"]/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" }[c]));
  const num = (x, d = 1) => (Math.round(x * 10 ** d) / 10 ** d).toLocaleString("it-IT");
  const tok = (n) => (n >= 1e9 ? num(n / 1e9) + " G" : n >= 1e6 ? num(n / 1e6) + " M" : n >= 1e3 ? num(n / 1e3, 0) + " k" : String(Math.round(n)));
  const css = `
.consd .cd-testa{display:flex;align-items:baseline;gap:8px;margin:18px 0 8px}
.consd .cd-testa small{color:var(--ink3);font-size:11.5px}
.consd .cd-nota{margin:0 0 10px;font-size:11.5px;line-height:1.45;color:var(--ink3)}
.cd-leg{display:flex;flex-wrap:wrap;gap:12px;margin:6px 0 10px;font-size:11.5px;color:var(--ink2)}
.cd-leg i{display:inline-block;width:9px;height:9px;border-radius:2px;margin-right:5px;vertical-align:-1px}
.cd-t0{background:var(--ink)}.cd-t1{background:var(--ink2)}.cd-t2{background:var(--ink3)}.cd-t3{background:var(--line)}
.cd-giorni{display:grid;grid-template-columns:repeat(7,1fr);gap:8px;align-items:end;height:110px}
.cd-giorni>div{display:flex;flex-direction:column;justify-content:flex-end;align-items:center;gap:3px;height:100%;min-width:0}
.cd-pila{display:flex;flex-direction:column-reverse;width:100%;max-width:34px;border-radius:5px 5px 2px 2px;overflow:hidden}
.cd-pila i{display:block;width:100%;min-height:0}
.cd-giorni small,.cd-giorni b{font-size:10.5px;color:var(--ink3);font-variant-numeric:tabular-nums;white-space:nowrap}
.cd-giorni b{color:var(--ink2);font-weight:500}.cd-giorni .oggi small{color:var(--ink);font-weight:600}
.cd-ore{display:flex;align-items:flex-end;gap:1px;height:56px;margin-top:12px;overflow-x:auto}
.cd-ore i{flex:1 0 3px;min-width:3px;background:var(--ink3);opacity:.6;border-radius:1px 1px 0 0;min-height:1px}
.cd-ore i.punta{background:var(--ink);opacity:1}
.cd-scala{display:flex;justify-content:space-between;font-size:10.5px;color:var(--ink3);margin-top:3px}
.cd-riga{display:grid;grid-template-columns:1fr auto;gap:1px 12px;align-items:center;padding:7px 0;border-top:1px solid var(--line)}
.cd-riga:first-of-type{border-top:0}
.cd-riga .n{min-width:0;font-size:12.5px;white-space:nowrap;overflow:hidden;text-overflow:ellipsis}
.cd-riga .v{font-size:12.5px;font-variant-numeric:tabular-nums;color:var(--ink2);text-align:right}
.cd-riga small{grid-column:1;font-size:11px;color:var(--ink3);white-space:nowrap;overflow:hidden;text-overflow:ellipsis}
.cd-riga .b{grid-column:1/-1;height:3px;border-radius:2px;background:var(--fill2);overflow:hidden;margin-top:3px}
.cd-riga .b i{display:block;height:100%;background:var(--ink2);border-radius:2px}
.cd-avviso{padding:8px 10px;margin:0 0 6px;border-radius:9px;background:var(--fill);font-size:12.5px;line-height:1.4;display:flex;gap:10px;align-items:center}
.cd-avviso span{flex:1}
`;
  if (!document.getElementById("cd-css")) { const st = document.createElement("style"); st.id = "cd-css"; st.textContent = css; document.head.appendChild(st); }
  const CD = (window.CONSD = { dati: null, t: 0, carica: false });
  async function readData() {
    if (CD.carica || Date.now() - CD.t < 60000) return;
    CD.carica = true;
    try { CD.dati = await invoke("consumi_dettaglio"); CD.t = Date.now(); } catch (e) { console.error(e); CD.dati = { ok: false, msg: String(e) }; }
    CD.carica = false;
  }
  const dayName = (s) => new Date(s * 1000).toLocaleDateString("it-IT", { weekday: "short" });
  function html(D) {
    if (!D) return `<div class="cd-testa"><h4>Dove e quando</h4></div><p class="cd-nota">Leggo i transcript della settimana…</p>`;
    if (!D.ok) return `<div class="cd-testa"><h4>Dove e quando</h4></div><p class="cd-nota">${esc(D.msg || "Dati non disponibili adesso")}</p>`;
    const weekData = D.limite?.pct, tz = D.fuso || 0;
    const sum = D.totale?.costo || 0.0001;
    // days of the current week: from the start of the window to today
    const g0 = Math.floor((D.da + tz) / 86400), g1 = Math.floor((D.a + tz) / 86400);
    const gg = [];
    for (let g = g0; g <= g1; g++) gg.push({ g, t: g * 86400 - tz, v: [0, 0, 0, 0] });
    for (const o of D.ore || []) { const g = Math.floor((o.t + tz) / 86400), x = gg.find((y) => y.g === g); if (x) { x.v[0] += o.chat; x.v[1] += o.sotto_agenti; x.v[2] += o.aiutanti; x.v[3] += o.regia; } }
    const maxDay = Math.max(...gg.map((x) => x.v.reduce((a, b) => a + b, 0)), 0.0001);
    const todayIdx = g1;
    const legend = `<div class="cd-leg">${KINDS.map(([, n], i) => `<span><i class="cd-t${i}"></i>${n}</span>`).join("")}</div>`;
    const dayList = `<div class="cd-giorni">${gg.map((x) => {
      const s = x.v.reduce((a, b) => a + b, 0);
      return `<div class="${x.g === todayIdx ? "oggi" : ""}" title="${esc(new Date(x.t * 1000).toLocaleDateString("it-IT", { weekday: "long", day: "numeric", month: "long" }))}: ${num((s / sum) * 100)}% dei token pesati della settimana"><b>${num((s / sum) * 100, 0)}%</b>
        <div class="cd-pila" style="height:${Math.max(2, (s / maxDay) * 78)}px">${x.v.map((c, i) => `<i class="cd-t${i}" style="height:${s ? (c / s) * 100 : 0}%"></i>`).join("")}</div><small>${esc(dayName(x.t))}</small></div>`;
    }).join("")}</div>`;
    const hours = D.ore || [], maxHour = Math.max(...hours.map((o) => o.costo), 0.0001);
    const hourly = hours.length ? `<div class="cd-ore" title="Un trattino per ogni ora della settimana: più alto = più token pesati">${hours.map((o) => `<i class="${o.costo >= maxHour * 0.7 ? "punta" : ""}" style="height:${Math.max(2, (o.costo / maxHour) * 100)}%" title="${esc(new Date(o.t * 1000).toLocaleString("it-IT", { weekday: "short", hour: "2-digit", minute: "2-digit" }))}: ${num((o.costo / sum) * 100)}% della settimana"></i>`).join("")}</div>
      <div class="cd-scala"><span>${esc(new Date(hours[0].t * 1000).toLocaleDateString("it-IT", { weekday: "short", day: "numeric" }))}</span><span>ora per ora</span><span>adesso</span></div>` : "";
    const row = (n, sub, q, t, w) => `<div class="cd-riga"><span class="n">${esc(n)}</span><span class="v">${num(q)}%${t ? " · " + tok(t) : ""}</span>${sub ? `<small>${esc(sub)}</small>` : ""}<div class="b"><i style="width:${Math.max(1.5, Math.min(100, w))}%"></i></div></div>`;
    const kinds = KINDS.map(([k, n]) => D.per_tipo?.[k]).some((x) => x?.costo) ? KINDS.map(([k, n]) => { const x = D.per_tipo[k]; return x.costo ? row(n, `${x.risposte} risposte`, x.quota_token, x.token, x.quota_token) : ""; }).join("") : "";
    const models = MODELS.map(([k, n]) => { const x = D.per_modello?.[k]; return x?.costo ? row(n, `${x.risposte} risposte · uscita ${tok(x.uscita)} · cache ${tok(x.cache_letta + x.cache_scritta)}`, x.quota_token, x.token, x.quota_token) : ""; }).join("");
    const maxC = Math.max(...(D.chat || []).map((c) => c.quota_token), 0.01);
    const chat = (D.chat || []).slice(0, 12).map((c) => {
      const parts = [c.progetto, c.per_tipo.sotto_agenti >= 1 ? `sotto-agenti ${num(c.per_tipo.sotto_agenti, 0)}%` : "", c.per_tipo.aiutanti >= 1 ? `aiutanti ${num(c.per_tipo.aiutanti, 0)}%` : "", c.transcript_mb >= 50 ? `conversazione ${num(c.transcript_mb, 0)} MB` : "", !c.juggl ? "fuori da Juggl" : ""].filter(Boolean).join(" · ");
      return `<div class="cd-riga"><span class="n">${c.juggl ? `<a href="#" data-cons-apri="${esc(c.sessione)}" style="color:inherit">${esc(c.nome || "Chat " + c.sessione.slice(0, 8))}</a>` : esc(c.nome || "Chat " + c.sessione.slice(0, 8))}</span><span class="v">${num(c.quota_token)}% · ${tok(c.token)}</span>
        <small>${esc(parts)} · oggi ${num(c.oggi_quota_token, 0)}% di oggi</small><div class="b"><i style="width:${Math.max(1.5, (c.quota_token / maxC) * 100)}%"></i></div></div>`;
    }).join("");
    const maxP = Math.max(...(D.progetti || []).map((p) => p.quota_token), 0.01);
    const projects = (D.progetti || []).slice(0, 10).map((p) => row(p.progetto || "Senza progetto", `${p.risposte} risposte`, p.quota_token, p.token, (p.quota_token / maxP) * 100)).join("");
    const warns = (D.avvisi || []).slice(0, 6).map((a) => `<div class="cd-avviso"><span>${esc(a.testo)}</span>${a.sessione ? `<button class="tasto" data-cons-apri="${esc(a.sessione)}">Apri</button>` : ""}</div>`).join("");
    const section = (t, sub, body) => (body ? `<div class="cd-testa"><h4>${t}</h4><small>${sub || ""}</small></div>${body}` : "");
    return `${section("Cose da sapere", "", warns)}
      <div class="cd-testa"><h4>Quando si è consumato</h4><small>${weekData != null ? `settimana al ${Math.round(weekData)}% (la barra) · ` : ""}${tok(D.totale?.token || 0)} token questa settimana</small></div>
      <p class="cd-nota">Ogni colonna è la parte dei token pesati della settimana usata in quel giorno. Quota dei token, non percentuale ufficiale: Anthropic dà solo il totale della settimana.</p>${legend}${dayList}${hourly}
      ${section("Per tipo", "chi ha usato i token", kinds)}${section("Per modello", "", models)}${section("Le chat più care", "questa settimana", chat)}${section("Per progetto", "", projects)}
      <p class="cd-nota" style="margin-top:14px">${esc(D.nota || "")} ${esc(D.regia_non_distinta || "")}</p>`;
  }
  CD.html = html;   // for tests
  CD.monta = async function () {
    const el = document.getElementById("consd"); if (!el) return;
    el.classList.add("consd");
    const h = html(CD.dati); if (el._h !== h) { el.innerHTML = h; el._h = h; }
    await readData();
    const h2 = html(CD.dati); if (el.isConnected && el._h !== h2) { el.innerHTML = h2; el._h = h2; }
  };
})();
