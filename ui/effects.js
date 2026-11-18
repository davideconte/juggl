// Subtle effects for the sidebar and dictation (effects Nov 16), inspired by "Thinking orbs" and "Border beam" from libraries.dev
// (MIT, see NOTICE). No code copied: CSS and a few lines of JS, no React.
// 1) "Thinking": three neutral dots on the rows of working chats (span.el-pensa set by sidebar.js), 1.4 s cycle.
//    The ring on the icon stays still: only one motion per row.
// 2) Breathing: slow orange border (3.2 s, at most 35% opacity) ONLY on chats waiting for the user (brand
//    rule: orange = waiting for you). Never on other rows.
// 3) Voice: during dictation a neutral halo around the recording dot follows the microphone level (event
//    "mic-livello", 20 per second), rising fast and falling slowly.
// Rules: still with "Reduce motion"; paused for rows out of view (IntersectionObserver) and for the whole app when the window
// is not active or hidden; at most MAX_ANIMATE rows animated together (the others stay still, same look).
// Same rule as the uploads chat: .fuori-vista, .fuori-vista * {animation-play-state: paused}.
(() => {
  if (!window.__TAURI__) return;
  const MAX_ANIMATE = 10;
  const css = `
.el-pensa{display:inline-flex;align-items:center;gap:2px;margin-right:5px;align-self:center;flex:none}
.el-pensa i{width:3.5px;height:3.5px;border-radius:50%;background:var(--ink3);opacity:.35;will-change:opacity,transform;animation:ef-pensa 1.4s ease-in-out infinite}
.el-pensa i:nth-child(2){animation-delay:.2s}.el-pensa i:nth-child(3){animation-delay:.4s}
@keyframes ef-pensa{0%,70%,100%{opacity:.25;transform:none}35%{opacity:.9;transform:translateY(-1.5px)}}
.riga.sel .el-pensa i{background:var(--selink2)}
.el-pensa.dolce i{opacity:.2;animation-duration:2.6s}
.el-pensa.dolce i:nth-child(2){animation-delay:.4s}.el-pensa.dolce i:nth-child(3){animation-delay:.8s}
.elenco .st-lav::after{animation:none;border-color:color-mix(in srgb,var(--ink3) 45%,transparent)}
.riga.sel .st-lav::after{border-color:var(--selink2)}
.elenco .riga.r2.el-asp:not(.sel)::before{content:"";position:absolute;inset:0;border-radius:inherit;pointer-events:none;box-shadow:inset 0 0 0 1.5px rgba(255,91,31,.35);opacity:.25;will-change:opacity;animation:ef-respira 3.2s ease-in-out infinite}
@keyframes ef-respira{0%,100%{opacity:.25}50%{opacity:1}}
.ef-calmo .el-pensa i,.elenco .riga.r2.el-asp.ef-calmo::before{animation:none}
.elenco .riga.r2.el-asp.ef-calmo:not(.sel)::before{opacity:.6}
.fuori-vista,.fuori-vista *,.fuori-vista::before,html.ef-fermo .elenco *,html.ef-fermo .elenco .riga::before{animation-play-state:paused !important}
.micbar .mic-punto{transition:box-shadow 90ms linear;box-shadow:0 0 0 calc(var(--ef-liv,0) * 7px) color-mix(in srgb,currentColor 22%,transparent)}
.micbar{transition:box-shadow 140ms linear;box-shadow:0 0 calc(var(--ef-liv,0) * 16px) color-mix(in srgb,var(--ink) 14%,transparent)}
@media (prefers-reduced-motion: reduce){
  .el-pensa i{animation:none;opacity:.5}
  .elenco .riga.r2.el-asp:not(.sel)::before{animation:none;opacity:.6}
  .micbar,.micbar .mic-punto{transition:none;box-shadow:none}
}`;
  const st = document.createElement("style"); st.id = "effetti-css"; st.textContent = css; document.head.appendChild(st);
  const reduceMotion = () => matchMedia("(prefers-reduced-motion: reduce)").matches;

  // ---------- animated rows: only the visible ones, at most MAX_ANIMATE ----------
  const visibleRows = new Set();
  const io = "IntersectionObserver" in window ? new IntersectionObserver((entries) => {
    for (const v of entries) { v.target.classList.toggle("fuori-vista", !v.isIntersecting); if (v.isIntersecting) visibleRows.add(v.target); else visibleRows.delete(v.target); }
    limit();
  }, { root: document.getElementById("elenco"), rootMargin: "40px" }) : null;
  const observed = new Set();
  function animate() { return [...document.querySelectorAll("#elenco .riga.r2.el-asp, #elenco .riga.r2 .el-pensa")].map((x) => x.closest(".riga")); }
  function limit() {
    let n = 0;
    for (const r of animate()) {
      const vis = !io || visibleRows.has(r);
      const ok = vis && n < MAX_ANIMATE;
      if (ok) n++;
      r.classList.toggle("ef-calmo", !ok);
    }
  }
  function aggancia() {
    const nowMs = new Set(animate());
    for (const r of [...observed]) if (!nowMs.has(r) || !r.isConnected) { io?.unobserve(r); observed.delete(r); visibleRows.delete(r); }
    for (const r of nowMs) if (!observed.has(r)) { io?.observe(r); observed.add(r); }
    limit();
  }
  const el = document.getElementById("elenco");
  if (el) new MutationObserver(() => { clearTimeout(aggancia._t); aggancia._t = setTimeout(aggancia, 0); }).observe(el, { childList: true, subtree: true });

  // ---------- window not active or hidden: everything stopped (no work when idle) ----------
  const isStill = () => document.documentElement.classList.toggle("ef-fermo", document.hidden || !document.hasFocus());
  addEventListener("focus", isStill); addEventListener("blur", isStill); document.addEventListener("visibilitychange", isStill);
  isStill();

  // ---------- voice: halo following the microphone level ----------
  // timer at 25 per second only during dictation (the level arrives at 20 per second); stopped as soon as the halo is off
  let lvl = 0, goal = 0, raf = 0, last = 0;
  function step() {
    raf = 0;
    // rises fast, falls slowly
    lvl += (goal - lvl) * (goal > lvl ? 0.55 : 0.2);
    if (Date.now() - last > 400) goal = 0;
    const v = Math.min(1, lvl * 1.6);
    document.querySelectorAll(".micbar").forEach((b) => b.style.setProperty("--ef-liv", v.toFixed(3)));
    if (lvl > 0.004 || goal > 0) raf = setTimeout(step, 40);
    else document.querySelectorAll(".micbar").forEach((b) => b.style.removeProperty("--ef-liv"));
  }
  window.__TAURI__.event.listen("mic-livello", (e) => {
    if (reduceMotion()) return;
    goal = Math.max(0, Math.min(1, +(e.payload?.v) || 0)); last = Date.now();
    if (!raf) raf = setTimeout(step, 0);
  });
  window.effetti = { aggancia, MAX_ANIMATE };
})();
