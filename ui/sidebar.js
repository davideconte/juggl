// Clear list (Nov 14): "Prima tu" sidebar, chosen by the user on Nov 14 (proposals: juggl.dev/p/juggl-sidebar-clarity-...).
// In a 5-second glance: who is waiting for you, who is working and whether it is stuck, what finished and is to read, what is stopped, whether the Mac is struggling.
// - Strip at the top, always visible: 4 numbers (waiting for you, stuck, working, to read); click or ⌥1-4 = only those, Esc clears.
// - First "Ti aspettano" (oldest first) and "Bloccate", with the project name; then Pinned; then Projects with
//   the header that sticks at the top and only chats that work or are to read; stopped ones in one row ("3 ferme · a, b, c",
//   click = shows them); projects with only stopped chats in "Solo ferme"; at the bottom "A riposo" and "Pronte" in one row.
// - Rows without repeated logo, name on two lines, time with its meaning ("da 9 min", "12 min", "55 min fa"),
//   memory only in the hover tooltip (and "pesante" above the threshold), no estimated percentage.
// - "Bloccata" = a working chat that got stuck: usage limit reached, or no sign for 15 min with the process
//   stopped (only while working: never when waiting for the user). If one day the server sends the "inceppata" field, that wins.
//   Note: in the rest of the code view "bloccata" means "asks you something" and here it goes into "Ti aspettano".
// - Helpers: the working ones are written in the row; finished ones become a single row, which disappears when you open the
//   chat (the server removes them anyway after 30 min). On the open chat all are shown, as before.
// - Icons (Nov 15, the user: "le icone lasciamole"): every row has the project icon (logo or initials in the project color)
//   and the state on top, light: orange dot = waiting for you (orange only here), spinning ring = working, "!" = stuck,
//   empty circle = to read, faded icon = stopped, more faded = resting. Color: progetto.colore or automatic from the
//   name (coloreDi: same rule on the iPhone). Right click: "Icona e colore del progetto…". Automatic icons: logos.rs.
// - Sub-chats (merge Nov 15): field "sotto_di" = main chat (commands unisci_chat / separa_chat). In groups the
//   sub-chats sit under the main one, indented and on one row; at the top (waiting for you, stuck, to read) they stay at the
//   top with "Principale ›". Right click: "Unisci a…" and "Togli da sotto". Discreet notice under the strip when a chat is
//   born that looks like an open one (same folder that is not home, or same project and name with the same words: the
//   trailing numbers do not count), with Merge / Ignore.
// - Navigate (Nov 16): ⌘[ / ⌘] back and forward in the history of opened chats, ⌃Tab / ⌃⇧Tab next and
//   previous chat in list order. On the Mac the keys are in the View menu (main.rs, "naviga" event), so they work
//   inside the input box too; on Windows and Linux the page handles them. EL.vicine() tells which chats to prepare in advance.
// - Coordinator: the "Scrivi alla regia" box opens from the icon in the group header (or from right click).
// - Footer: "Mac tranquillo / sotto sforzo / al limite" and "Libera N GB": puts to rest ONLY stopped chats, not pinned, without
//   helpers, without active processes, without an open turn and that are not a coordinator, ONE AT A TIME, then
//   "N chat a riposo, si riaprono con un clic" with Undo (resumes the same ones, one at a time).
// - Fixed order: as before, while the pointer is over the list rows and sections do not move (visibili() + sections
//   frozen here); the strip numbers are always the real ones and a row says how many chats will move.
// Hooks in index.html (marked "elenco (14/11)"): <script src="sidebar.js"> before the main script, the first line
// of disegnaElenco, the ↑↓ order. In coordinator.js: summary and box follow EL. Without this file the list is the old one.
(() => {
  if (!window.__TAURI__) return;
  const EL = (window.EL = { filtro: "", scrivi: new Set(), fermeAperte: new Set(), visti: new Map(), sezFerma: new Map(), gruppoDi: new Map(), riposo: null });
  const NO_SIGN_MIN = 15;
  const OLD_DAYS = 30;   // first impression (Nov 16): chats stopped longer than this end up among the old ones
  const MAC_STRAIN_PCT = 25;     // free memory under 25%: "sotto sforzo"; under avviso_mac_libera_pct (12): "al limite"
  const REST_MAX = 4, REST_MIN_MB = 150, REST_NOTICE_MS = 20000;

  const css = `
#elenco{--el-top:62px;--el-ar:#c2410c}
@media (prefers-color-scheme: dark){#elenco,.piede{--el-ar:#ff7a45}}
.el-striscia{position:sticky;top:-4px;z-index:6;background:var(--side);margin:-4px -8px 0;padding:6px 8px 6px;display:grid;grid-template-columns:1.3fr 1fr 1fr 1fr;gap:4px}
.el-k{border-radius:10px;padding:6px 5px 5px 7px;display:flex;flex-direction:column;gap:1px;box-shadow:inset 0 0 0 1px var(--line);min-width:0;text-align:left;cursor:pointer}
.el-k:hover{background:var(--fill)}
.el-k .n{display:flex;align-items:center;gap:5px;font-size:17px;font-weight:650;font-variant-numeric:tabular-nums;letter-spacing:-.01em;line-height:1.1;color:var(--ink)}
.el-k .l{font-size:10.5px;line-height:1.15;color:var(--ink2)}
.el-k.asp.pieno .n b{color:var(--el-ar)}
.el-k.zero{opacity:.55}
.el-k.sk{cursor:default}
#presenza.el-avvio{visibility:hidden}
/* avvio (16/11): scheletri finche' le chat, la memoria e i limiti non arrivano la prima volta. Stesso riflesso lento
   dei caricamenti: si sposta con transform, 2,4 s con pausa nel ciclo, al massimo 8 giri; fermo con Riduci movimento. */
.el-sk{position:relative;overflow:hidden;display:block;height:9px;border-radius:5px;background:var(--fill)}
.el-sk::after{content:"";position:absolute;inset:0;background:linear-gradient(100deg,transparent 25%,rgba(127,127,127,.11) 50%,transparent 75%);transform:translateX(-100%);animation:elRiflesso 2.4s ease-in-out .4s 8}
@keyframes elRiflesso{0%{transform:translateX(-100%)}45%,100%{transform:translateX(100%)}}
/* r1629: the skeleton is a transparent digit, so it has exactly the height of the real number (no 5 px jump) */
.el-k b.el-sk-n{height:auto;min-width:16px;border-radius:4px;color:transparent}
.el-sk-righe{display:flex;flex-direction:column;gap:2px;padding:2px 0}
.el-sk-riga{display:flex;align-items:center;gap:10px;padding:9px 10px;border-radius:10px}
.el-sk-riga>span{flex:1;min-width:0;display:flex;flex-direction:column;gap:6px}
.el-sk-riga .el-sk-ic{width:26px;height:26px;border-radius:50%;flex:none;background:var(--fill2)}
.el-sk-riga .el-sk-t{height:10px}
.el-sk-mac .seg.el-sk{width:8px;height:8px;border-radius:2px;background:var(--fill)}
.el-sk-mac .st{display:flex;flex-direction:column;gap:5px;flex:1}
.piede-quota.el-sk-q{cursor:default}
.el-sk-q .el-sk-anello{width:22px;height:22px;border-radius:50%;flex:none;background:var(--fill2)}
.el-sk-q .q-t{display:flex;flex-direction:column;gap:5px}
@media (prefers-reduced-motion: reduce){.el-sk::after{animation:none;display:none}}
.el-k.blo.pieno{background:var(--ink);box-shadow:none}
.el-k.blo.pieno .n,.el-k.blo.pieno .l{color:var(--bg)}
.el-k.blo.pieno .el-ic i{background:var(--bg);color:var(--ink)}
.el-k[aria-pressed=true]{box-shadow:inset 0 0 0 2px var(--ink)}
.el-k.blo.pieno[aria-pressed=true]{box-shadow:0 0 0 2px var(--side),0 0 0 3.5px var(--ink)}
.el-info{grid-column:1/-1;display:flex;justify-content:space-between;align-items:center;gap:8px;font-size:11.5px;color:var(--ink2);padding:3px 3px 0}
.el-info button{font-weight:600;color:var(--ink);text-decoration:underline;text-underline-offset:2px;cursor:pointer}
.el-cambi{font-size:11px;line-height:1.35;color:var(--ink2);padding:5px 8px;border-radius:8px;text-align:left;cursor:pointer}
/* jank r1629: notices under the strip float over the list (no in-flow row), so nothing below moves when they appear */
.el-sopra{position:absolute;left:8px;right:8px;top:calc(100% - 2px);display:flex;flex-direction:column;gap:4px;pointer-events:none}
.el-sopra>*{pointer-events:auto;background:linear-gradient(var(--fill),var(--fill)),var(--side);box-shadow:0 0 0 1px var(--line),0 6px 16px rgba(0,0,0,.14)}
.el-cambi b{color:var(--ink);font-weight:600}
.el-ic{width:14px;display:grid;place-items:center;flex:none;height:18px}
.el-ic i{display:block;font-style:normal}
.el-k .el-ic{width:12px;height:auto}
.el-asp>.el-ic i,.el-k.asp .el-ic i{width:9px;height:9px;border-radius:50%;background:#ff5b1f}
.el-k.asp .el-ic i{width:8px;height:8px}
.el-let>.el-ic i,.el-k.let .el-ic i{width:8px;height:8px;border-radius:50%;box-shadow:inset 0 0 0 1.5px var(--ink2)}
.el-fer>.el-ic i,.el-riposo>.el-ic i,.el-pronta>.el-ic i,.el-ferme>.el-ic i{width:8px;height:1.5px;border-radius:1px;background:var(--ink3)}
.el-lav>.el-ic i,.el-k.lav .el-ic i{width:10px;height:10px;border-radius:50%;border:1.5px solid var(--fill2);border-top-color:var(--ink);transform:rotate(45deg);animation:el-giro 1.1s linear infinite}
.el-blo>.el-ic i,.el-k.blo .el-ic i{width:12px;height:12px;border-radius:3.5px;background:var(--ink);color:var(--bg);font:800 9px/12px var(--sans);text-align:center}
@keyframes el-giro{to{transform:rotate(405deg)}}
@media (prefers-reduced-motion: reduce){.el-lav>.el-ic i,.el-k.lav .el-ic i{animation:none}}
.el-sez{padding:12px 8px 4px}
.el-sez.asp{color:var(--el-ar)}
.el-sez span{font-weight:500}
.elenco .riga.r2{grid-template-columns:24px minmax(0,1fr);gap:0 9px;align-items:start;padding:6px 8px;border-radius:9px;margin:0}
.el-icona{position:relative;width:var(--l);height:var(--l);flex:none;margin-top:1px}
.el-av{width:100%;height:100%;border-radius:50%;display:grid;place-items:center;background:var(--c);color:#fff;font:650 calc(var(--l) * .4)/1 var(--sans);letter-spacing:.01em;overflow:hidden;transition:opacity .2s,filter .2s}
.el-av.logo{background:#fff;box-shadow:inset 0 0 0 1px var(--line)}
.el-av img{width:100%;height:100%;object-fit:contain;pointer-events:none}
.el-badge{position:absolute;display:none;font-style:normal}
.st-asp .el-badge{display:block;right:-2px;top:-2px;width:9px;height:9px;border-radius:50%;background:#ff5b1f;box-shadow:0 0 0 2px var(--side)}
.st-let .el-badge{display:block;right:-2px;top:-2px;width:9px;height:9px;border-radius:50%;background:var(--side);box-shadow:inset 0 0 0 1.5px var(--ink2),0 0 0 2px var(--side)}
.st-blo .el-badge{display:block;right:-4px;bottom:-3px;width:13px;height:13px;border-radius:4px;background:var(--ink);color:var(--bg);font:800 9px/13px var(--sans);text-align:center;box-shadow:0 0 0 2px var(--side)}
.st-lav::after{content:"";position:absolute;inset:-3px;border-radius:50%;border:1.5px solid var(--fill2);border-top-color:var(--ink);animation:el-giro2 1.1s linear infinite}
@keyframes el-giro2{to{transform:rotate(360deg)}}
.st-fer .el-av{opacity:.6;filter:grayscale(.5)}
.st-riposo .el-av,.st-pronta .el-av{opacity:.42;filter:grayscale(1)}
.riga.sel .st-asp .el-badge,.riga.sel .st-let .el-badge,.riga.sel .st-blo .el-badge{box-shadow:0 0 0 2px var(--sel)}
.riga.sel .st-let .el-badge{background:var(--sel);box-shadow:inset 0 0 0 1.5px var(--selink),0 0 0 2px var(--sel)}
.riga.sel .st-blo .el-badge{background:var(--selink);color:var(--sel)}
.riga.sel .st-lav::after{border-color:rgba(127,127,127,.35);border-top-color:var(--selink)}
.riga:hover .st-asp .el-badge,.riga:hover .st-let .el-badge,.riga:hover .st-blo .el-badge{box-shadow:0 0 0 2px color-mix(in srgb,var(--side) 100%,var(--ink) 5%)}
@media (prefers-reduced-motion: reduce){.st-lav::after{animation:none;border-top-color:var(--ink);border-right-color:var(--ink)}}
.elenco .riga-gruppo.g2{box-shadow:inset 3px 0 0 var(--c)}
.el-prog{position:fixed;z-index:60;width:244px;padding:10px;border-radius:12px;background:var(--bg);color:var(--ink);box-shadow:0 10px 30px rgba(0,0,0,.25),0 0 0 1px var(--line);font-size:12.5px}
.el-prog h5{margin:0 0 8px;font-size:12.5px;font-weight:650;display:flex;align-items:center;gap:8px}
.el-prog .sw{display:grid;grid-template-columns:repeat(8,1fr);gap:5px;margin-bottom:6px}
.el-prog .sw button{width:22px;height:22px;border-radius:50%;cursor:pointer;box-shadow:inset 0 0 0 1px rgba(255,255,255,.15)}
.el-prog .sw button[aria-pressed=true]{box-shadow:0 0 0 2px var(--bg),0 0 0 3.5px var(--ink)}
.el-prog .auto{font-size:11.5px;color:var(--ink2);text-decoration:underline;text-underline-offset:2px;cursor:pointer;margin-bottom:8px;display:inline-block}
.el-prog .auto[aria-pressed=true]{color:var(--ink);font-weight:600;text-decoration:none}
.el-prog hr{border:0;border-top:1px solid var(--line);margin:6px 0}
.el-prog .az{display:block;width:100%;text-align:left;padding:6px 6px;border-radius:7px;cursor:pointer;color:var(--ink)}
.el-prog .az:hover{background:var(--fill)}
.el-prog .az small{color:var(--ink3);margin-left:4px}
.el-prog form{display:flex;gap:6px;padding:4px 2px}
.el-prog input{flex:1;min-width:0;height:28px;border-radius:7px;border:1px solid var(--line);background:var(--fill);color:inherit;font:inherit;font-size:12px;padding:0 8px;outline:none;-webkit-user-select:text;user-select:text}
.el-onl{display:flex;gap:8px;align-items:flex-start;padding:4px 4px;cursor:pointer;font-size:12px}
.el-onl input{margin-top:2px}
.el-escl{padding-left:22px;color:var(--ink2)}
.el-onl small{display:block;color:var(--ink3);font-size:11px;line-height:1.3;margin-top:2px}
.el-prog form button{padding:0 10px;border-radius:7px;background:var(--ink);color:var(--bg);font-weight:600;cursor:pointer}
.elenco .riga.r2 .r1{align-items:flex-start;gap:6px}
.elenco .riga.r2 .nome{white-space:normal;overflow-wrap:anywhere;display:-webkit-box;-webkit-line-clamp:2;-webkit-box-orient:vertical;overflow:hidden;font-size:13px;line-height:1.3;text-overflow:clip}
.elenco .riga.r2 .ora{font-size:11px;margin-top:2px}
.elenco .riga.r2 .anteprima{font-size:12px;line-height:1.35;min-height:0;margin-top:1px}
.elenco .riga.r2.el-dentro{padding-left:12px}
.el-pj,.el-chi{color:var(--ink);font-weight:600}
.riga.sel .el-pj,.riga.sel .el-chi{color:var(--selink)}
.riga.r2.sel.el-lav>.el-ic i{border-color:rgba(127,127,127,.35);border-top-color:var(--selink)}
.riga.r2.sel.el-blo>.el-ic i{background:var(--selink);color:var(--sel)}
.riga.r2.sel.el-let>.el-ic i{box-shadow:inset 0 0 0 1.5px var(--selink)}
.el-pill{display:inline-block;font-size:10.5px;font-weight:600;padding:0 6px;border-radius:999px;background:var(--fill2);color:var(--ink);line-height:16px;margin-left:5px;vertical-align:1px;white-space:nowrap}
.riga.sel .el-pill{background:var(--selink);color:var(--sel)}
/* role labels (user report part 2): same look as the "regia" label of coordinator.js, so they do not look like counters */
.el-pill.el-ruolo{background:none;margin-left:6px;font-size:10px;font-weight:600;letter-spacing:.03em;text-transform:uppercase;color:var(--ink3);border:1px solid var(--line);border-radius:5px;padding:0 4px;line-height:14px}
.riga.sel .el-pill.el-ruolo{background:none;border-color:var(--selink2);color:var(--selink2)}
.riga-sa .t .el-ruolo{margin:0 5px 0 0}
.el-pesante{color:var(--err);font-weight:600;font-size:11px;margin-left:5px;white-space:nowrap}
.elenco .riga-gruppo.g2{display:flex;align-items:center;gap:8px;position:sticky;top:var(--el-top);z-index:4;background:var(--side);padding:7px 8px 5px;margin:4px 0 0;border-radius:8px}
.elenco .riga-gruppo.g2:hover{background:linear-gradient(var(--fill),var(--fill)),var(--side)}
.elenco .riga-gruppo.g2.sel{background:var(--sel);color:var(--selink)}
.elenco .riga-gruppo.g2.sel .el-gs{color:var(--selink2)}
.el-gn{font-weight:650;font-size:12.5px;min-width:0;overflow-wrap:anywhere}
.el-gs{margin-left:auto;font-size:11px;color:var(--ink2);white-space:nowrap;font-variant-numeric:tabular-nums}
.el-o{color:var(--el-ar);font-weight:650}
.el-regia{width:22px;height:22px;border-radius:7px;display:grid;place-items:center;color:var(--ink2);flex:none;cursor:pointer}
.el-regia:hover,.el-regia.on{background:var(--fill2);color:var(--ink)}
.el-regia svg{width:13px;height:13px}
.elenco .gr-scrivi{padding:0 8px 0 12px}
.el-ferme{display:flex;gap:7px;align-items:start;width:100%;padding:5px 8px 6px;border-radius:9px;font-size:12px;color:var(--ink2);line-height:1.35;text-align:left;cursor:pointer}
.el-ferme:hover{background:var(--fill)}
.el-ferme .el-ic{height:16px}
.el-ferme>.el-icona{margin-top:-1px}
.el-ferme b{color:var(--ink);font-weight:550}
.el-ferme b.el-p{font-weight:650}
.el-ferme.el-dentro{padding-left:12px}
.el-ferme.aperto b::after{content:" · nascondi";font-weight:400;color:var(--ink3)}
.el-aiut{display:flex;align-items:center;gap:6px;width:100%;font-size:11.5px;color:var(--ink2);padding:1px 8px 5px 29px;text-align:left;cursor:pointer}
.el-aiut svg{width:12px;height:12px;flex:none;color:var(--ink3)}
.el-aiut u{text-decoration:none;color:var(--ink);font-weight:550}
.el-aiut:hover u{text-decoration:underline}
.elenco .riga.r2.el-una{grid-template-columns:20px minmax(0,1fr);padding:4px 8px;align-items:center}
.elenco .riga.r2.el-una .nome{display:block;white-space:nowrap;overflow:hidden;text-overflow:ellipsis;-webkit-line-clamp:unset;font-size:12.5px}
.elenco .riga.r2.el-una .r1{align-items:center}
.elenco .riga.r2.el-una .ora{margin-top:0}
.el-msg{font-weight:400;color:var(--ink2)}
.riga.sel .el-msg{color:var(--selink2)}
.el-una .el-icona{margin-top:0}
.el-sez.let{align-items:center}
.el-tutte-lette{margin-left:auto;font-size:11px;font-weight:500;color:var(--ink2);text-decoration:underline;text-underline-offset:2px;cursor:pointer;letter-spacing:0;text-transform:none}
.el-tutte-lette:hover{color:var(--ink)}
.el-altre b{font-weight:600}
.elenco .riga.r2.el-figlia{grid-template-columns:18px minmax(0,1fr);padding:3px 8px 3px 30px}
.elenco .riga.r2.el-figlia .nome{font-size:12px;font-weight:550}
.elenco .riga.r2.el-figlia::before{content:"";position:absolute;left:19px;top:-3px;height:14px;width:7px;border-left:1.5px solid var(--fill2);border-bottom:1.5px solid var(--fill2);border-bottom-left-radius:5px}
.el-nfig{font-weight:500;color:var(--ink2)}
.el-simile{display:flex;align-items:center;gap:6px;font-size:11.5px;line-height:1.3;color:var(--ink2);padding:5px 7px;border-radius:8px;background:var(--fill)}
.el-simile span{flex:1;min-width:0}.el-simile b{color:var(--ink);font-weight:600}
.el-simile button{flex:none;font-size:11px;font-weight:600;padding:2px 8px;border-radius:7px;cursor:pointer;color:var(--ink);box-shadow:inset 0 0 0 1px var(--line)}
.el-simile button[data-el-sim-unisci]{background:var(--ink);color:var(--bg);box-shadow:none}
.el-unisci{width:280px}
.el-u-cerca{width:100%;height:28px;border-radius:7px;border:1px solid var(--line);background:var(--fill);color:inherit;font:inherit;font-size:12px;padding:0 8px;outline:none;margin-bottom:6px;-webkit-user-select:text;user-select:text}
.el-u-lista{max-height:320px;overflow:auto}
.el-unisci .az{display:flex;align-items:center;gap:8px}
.el-unisci .az small{display:block;color:var(--ink3);font-size:11px}
.el-fumetto{background:var(--fill2) !important;color:var(--ink2)}
.el-fumetto svg{width:56%;height:56%}
.el-vecchie{margin-top:10px;color:var(--ink3)}
.el-pz{display:flex;align-items:center;gap:8px;padding:4px 8px 4px 12px}
.el-pz-n{flex:1;min-width:0;font-size:12.5px;font-weight:550;overflow-wrap:anywhere}
.el-pz-n small{display:block;font-weight:400;font-size:11px;color:var(--ink3)}
.el-pz-b{flex:none;font-size:11px;font-weight:600;padding:2px 8px;border-radius:7px;box-shadow:inset 0 0 0 1px var(--line);color:var(--ink);cursor:pointer}
.el-pz-b:hover{background:var(--fill)}
.fm-did{height:auto;align-items:flex-start}
.fm-did[data-el] .fm-logo{display:none}
.el-fm-tx{flex:1;min-width:0;display:flex;flex-direction:column;line-height:14px}
.el-fm-tx .fm-nm{white-space:nowrap;overflow:hidden;text-overflow:ellipsis;font-weight:550;color:var(--ink)}
.el-fm-sotto{display:flex;min-width:0;font-size:10px;color:var(--ink3);white-space:nowrap}
.el-fm-p{overflow:hidden;text-overflow:ellipsis;min-width:0;white-space:pre}
.el-fm-sotto .fm-ora{flex:none}
.el-fm-segue .fm-nm{visibility:hidden}
.el-fm-segue .el-fm-p{display:none}
.el-vuota{font-size:12px;color:var(--ink3);padding:8px}
.el-mac{display:flex;align-items:center;gap:8px}
.el-mac .st{font-size:12px;font-weight:650;color:var(--ink);line-height:1.25;min-width:0;flex:1}
.el-mac .st small{display:block;font-weight:400;font-size:11px;color:var(--ink2);white-space:nowrap;overflow:hidden;text-overflow:ellipsis}
.el-sk-barra{height:4px;margin-top:5px;border-radius:2px}
.el-mac .seg{width:8px;height:8px;border-radius:2px;background:var(--ink3);flex:none}
.el-mac.sforzo .seg{background:var(--ink)}
.el-mac.limite .seg{background:var(--err)}.el-mac.limite .st{color:var(--err)}
.el-mac .disco{color:var(--err);font-weight:600}
.el-libera{margin-left:auto;font-size:11.5px;font-weight:600;padding:4px 9px;border-radius:8px;background:var(--ink);color:var(--bg);white-space:nowrap;cursor:pointer;flex:none}
.el-libera:disabled{opacity:.5;cursor:default}
.el-riposo-msg{display:flex;align-items:center;gap:8px;margin-top:6px;padding:5px 8px;border-radius:8px;background:var(--fill2);font-size:11.5px;color:var(--ink);line-height:1.3}
.el-riposo-msg span{flex:1;min-width:0}
.el-riposo-msg button{font-weight:600;text-decoration:underline;text-underline-offset:2px;cursor:pointer;flex:none}
`;
  const st = document.createElement("style"); st.id = "elenco-css"; st.textContent = css; document.head.appendChild(st);

  // sober palette, same as logos.rs TAVOLOZZA: no gold, no orange (orange means "waiting for you")
  const PALETTE = ["#475569", "#3b5b7a", "#2f6b66", "#55663a", "#6a4c7a", "#8a4545", "#3e4a8c", "#5e5148"];
  /** Project color: the chosen one, otherwise from the id name (h = h*31 + code, unsigned 32 bit, then % 8). */
  function colorOf(p, fallbackColor) {
    if (p?.colore && PALETTE.includes(p.colore)) return p.colore;
    const k = String(p?.id || p?.nome || fallbackColor || "?");
    let h = 0; for (const c of k) h = (h * 31 + c.codePointAt(0)) >>> 0;
    return PALETTE[h % PALETTE.length];
  }
  EL.coloreDi = colorOf; EL.TAV = PALETTE;
  function initialsOf(nome) {
    const w = String(nome || "?").replace(/[·|:\-]/g, " ").split(/\s+/).filter(Boolean);
    const t = w.length >= 2 ? [...w[0]][0] + [...w[1]][0] : [...(w[0] || "?")].slice(0, 2).join("");
    return t.toUpperCase();
  }
  /** Round project icon (logo or initials in its color) with the state on top. */
  const BUBBLE = '<svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linejoin="round"><path d="M3 3.5h10v7H7.5L4.5 13v-2.5H3z"/></svg>';
  function iconHtml(s, sz, sizeSide = 24) {
    const p = s.progetto, c = colorOf(p, s.nome);
    // chat without a project: grey speech bubble, so it does not look like a project
    if (sz !== "gr" && (!p?.id || p.id === "locale") && !String(s.sessione || "").startsWith("wa:"))
      return `<span class="el-icona st-${sz}" style="--l:${sizeSide}px"><span class="el-av el-fumetto">${BUBBLE}</span><i class="el-badge">${sz === "blo" ? "!" : ""}</i></span>`;
    const logo = p?.logo;
    const dentro = logo ? `<img src="${esc(T.core.convertFileSrc(logo))}" alt="" draggable="false">` : esc(initialsOf(p?.nome || s.nome));
    return `<span class="el-icona st-${sz}" style="--c:${c};--l:${sizeSide}px"><span class="el-av${logo ? " logo" : ""}">${dentro}</span><i class="el-badge">${sz === "blo" ? "!" : ""}</i></span>`;
  }
  // for other views (e.g. "In corso"): same icon, same size. icona(s, stato, lato): s = { nome, progetto: { id, nome, logo, colore } }
  EL.icona = iconHtml;
  /** Projects by recent use (for the New chat window and other views): all known ones, even without chats.
   *  Each entry: the project + ultimo (seconds, last activity of its chats, 0 if none), chat (how many),
   *  automatico (found by Juggl: auto field), importato (importato_il present). Order: most recent first, then by name. */
  EL.progettiPerUso = () => {
    const everyone = new Map(), lastTs = new Map(), n = new Map();
    for (const p of EL.progetti || []) if (p?.id && p.id !== "locale") everyone.set(p.id, p);
    for (const s of dati.sessioni || []) {
      const p = s.progetto; if (!p?.id || p.id === "locale") continue;
      if (!everyone.has(p.id)) everyone.set(p.id, p);
      lastTs.set(p.id, Math.max(lastTs.get(p.id) || 0, s.agg || 0, s.da || 0)); n.set(p.id, (n.get(p.id) || 0) + 1);
    }
    return [...everyone.values()].map((p) => ({ ...p, ultimo: lastTs.get(p.id) || 0, chat: n.get(p.id) || 0, automatico: !!p.auto, importato: !!p.importato_il }))
      .sort((a, b) => b.ultimo - a.ultimo || String(a.nome || "").localeCompare(String(b.nome || ""), "it"));
  };
  /** Projects matching the search box (name, keywords, folder name), most recently used first. */
  function projectMatches(q) {
    q = q.trim().toLowerCase(); if (!q) return [];
    const base = (c) => String(c || "").split("/").filter(Boolean).pop() || "";
    return EL.progettiPerUso().filter((p) => [p.nome, ...(Array.isArray(p.parole) ? p.parole : []), base(p.cartella)].some((x) => String(x || "").toLowerCase().includes(q)));
  }
  /** Search results for projects, on top of the list: logo, name, how many chats, Open (most recent chat) and New chat. */
  const PROJ_MAX = 6;
  function projectResults(voci, list) {
    if (!list.length) return;
    voci.push(sectionHtml("pz-cerca", TEXT.projects, list.length));
    for (const p of list.slice(0, PROJ_MAX)) {
      const last = (dati.sessioni || []).filter((x) => x.progetto?.id === p.id).sort((a, b) => Math.max(b.agg || 0, b.da || 0) - Math.max(a.agg || 0, a.da || 0))[0];
      const sub = p.chat ? `${TEXT.chats(p.chat)} · ${quanto(p.ultimo)}` : TEXT.noChat;
      voci.push({ k: "pzc:" + p.id, h: `<div class="el-pz el-pz-cerca">${iconHtml({ nome: p.nome, progetto: p }, "gr", 20)}<span class="el-pz-n">${esc(p.nome)}<small>${esc(sub)}</small></span>${last ? `<button class="el-pz-b" data-el-apri="${esc(last.sessione)}" title="${esc(TEXT.openTip(p.nome))}">${TEXT.open}</button>` : ""}<button class="el-pz-b" data-el-nuova-in="${esc(p.id)}" title="${esc(TEXT.newChatTip)}">${TEXT.newChat}</button></div>` });
    }
    if (list.length > PROJ_MAX) voci.push({ k: "pzc:altri", h: `<div class="el-vuota">${esc(TEXT.more(list.length - PROJ_MAX))}</div>` });
  }
  const nowSec = () => Date.now() / 1000;
  /** Worked time today for a project (user report part 1): union of the turns of its chats since local midnight
   *  (session field turni_oggi = [[start_s, end_s or null if running], ...]). null when no session has the field yet
   *  (older app core): then nothing is shown. */
  EL.tempoOggi = (pid) => {
    const ss = (dati.sessioni || []).filter((x) => x.progetto?.id === pid);
    if (!ss.some((x) => Array.isArray(x.turni_oggi))) return null;
    const midnight = new Date().setHours(0, 0, 0, 0) / 1000, now = nowSec();
    const iv = [];
    for (const x of ss) for (const t of x.turni_oggi || []) {
      const a = Math.max(midnight, +t?.[0] || 0), b = Math.min(now, t?.[1] == null ? now : +t[1]);
      if (b > a) iv.push([a, b]);
    }
    iv.sort((x, y) => x[0] - y[0]);
    let tot = 0, ca = -1, cb = -1;
    for (const [a, b] of iv) { if (a > cb) { if (cb > ca) tot += cb - ca; ca = a; cb = b; } else cb = Math.max(cb, b); }
    if (cb > ca) tot += cb - ca;
    return Math.round(tot);
  };
  /** "12 min", "1 h 40", "3 h"; "" under a minute. */
  const durText = (sec) => { const m = Math.floor((sec || 0) / 60); if (m < 1) return ""; if (m < 60) return m + " min"; const h = Math.floor(m / 60), r = m % 60; return r ? `${h} h ${String(r).padStart(2, "0")}` : `${h} h`; };
  const todayHtml = (pid, lead) => { const t = durText(EL.tempoOggi(pid)); return t ? `${lead ? " · " : ""}<span class="el-oggi" title="${esc(TEXT.todayTip)}">${esc(TEXT.today(t))}</span>` : ""; };
  const special = (s) => !!s.locale || /^(wa|chiusa|locale):/.test(s.sessione || "");
  const limitHit = () => { const l = dati.limiti || {}; return [l.five_hour, l.seven_day].find((w) => (w?.used_percentage ?? 0) >= 100) || null; };
  const hhmm = (t) => new Date(t * 1000).toLocaleTimeString("it-IT", { hour: "2-digit", minute: "2-digit" });

  /** Reason why a working chat got stuck ("" = all fine). Only while the main turn is working. */
  function stuckReason(s) {
    if (s.inceppata) return typeof s.inceppata === "string" ? s.inceppata : s.inceppata.motivo || "Si è fermata";
    // usage limit marked by the server on the chat (fallback.rs, at the end of the turn): stuck even if the turn ended
    if (s.limite_claude && s.vista !== "aspetta" && s.vista !== "bloccata" && s.vista !== "ibernata") return "Limite d'uso raggiunto";
    if (s.vista !== "lavora" || s.lavora_sotto) return "";
    const a = s.ora_fa?.azione;
    const lastMsg = Math.max(s.agg || 0, s.da || 0, a?.t ? Date.parse(a.t) / 1000 || 0 : 0);
    const isStopped = nowSec() - lastMsg;
    const w = limitHit();
    if (w && isStopped > 60) return "Limite d'uso raggiunto" + (w.resets_at ? " · riparte alle " + hhmm(w.resets_at) : "");
    if (isStopped > NO_SIGN_MIN * 60 && (s.cpu ?? 0) < 5) return `Nessun segno da ${Math.round(isStopped / 60)} min`;
    return "";
  }
  EL.inceppata = stuckReason;
  /** Background jobs of the chat (server field in_background: [{descrizione, da}]), oldest first; null if none. */
  const bgOf = (s) => (Array.isArray(s.in_background) && s.in_background.length ? [...s.in_background].sort((a, b) => (+a.da || 0) - (+b.da || 0)) : null);
  /** Only waiting for one of its background jobs: it is not stopped and not waiting for the user (unless it asks something). */
  // if the turn is running (view lavora) the row says what it is doing now, as always
  const inBackground = (s) => !!bgOf(s) && s.vista !== "lavora" && s.vista !== "bloccata" && s.vista !== "ibernata" && !(s.vista === "aspetta" && s.serve_te_cosa);
  EL.inBg = inBackground;
  const liveSec = (s) => inBackground(s) ? "lav" : s.vista === "aspetta" || s.vista === "bloccata" ? "asp" : stuckReason(s) ? "blo" : s.vista === "lavora" ? "lav"
    : s.vista === "fatta" ? "let" : s.vista === "ibernata" ? (s.pronta ? "pronta" : "riposo") : "fer";
  const countable = () => dati.sessioni.filter((x) => x.sessione !== dati.juggl && !diProva(x));

  // ---------- row pieces ----------
  /** Where a chat lives: the project, or "in <cartella>", or "Chat generale" if it is in home. */
  let external = (s) => "";
  const prefix = (s) => s.progetto?.nome || external(s) || (!s.cwd || String(s.cwd).replace(/\/$/, "") === String(dati.home || "").replace(/\/$/, "") ? "Chat generale" : "in " + coda(s.cwd));
  function textStr(s, sz, withProject, parent) {
    let t;
    if (sz === "blo") { const c = piano(vero(s.chiesto)); t = `<span class="el-chi">${esc(stuckReason(s))}</span>` + (c ? " · " + esc(c) : ""); }
    else if ((s.pilota_tra && s.pilota_tra > nowSec()) || (riprese.has(s.sessione) && !riprese.get(s.sessione).err)) t = anteprima(s);
    else if (inBackground(s)) { const b = bgOf(s)[0]; t = `<span class="el-chi">Aspetta un lavoro in background</span>${b.descrizione ? " · " + esc(piano(b.descrizione)) : ""}${bgOf(s).length > 1 ? ` (e altri ${bgOf(s).length - 1})` : ""} · da ${esc(timeText(fromSecs(b.da), "dur"))}`; }
    else if (s.vista === "bloccata") t = `<span class="el-chi">Ti chiede:</span> ${esc(s.domanda || "una scelta")}`;
    else if (s.vista === "aspetta") t = s.serve_te_cosa ? `<span class="el-chi">Da fare:</span> ${esc(s.serve_te_cosa)}` : `<span class="el-chi">Ti aspetta:</span> ${esc(piano(s.risposta) || "")}`;
    else if (s.vista === "lavora") {
      const n = s.lavora_sotto || s.sotto_attivi || 0, a = s.ora_fa?.azione;
      const fa = s.lavora_sotto ? piano(s.risposta) : a ? `${NOMI_AZ[a.nome] || String(a.nome || "").replace(/^mcp__[^_]+__/, "")} ${a.testo || ""}` : piano(vero(s.chiesto));
      t = (n ? `<span class="el-chi">${n === 1 ? "1 aiutante" : n + " aiutanti"} al lavoro</span>${fa ? " · " : ""}` : "") + esc(fa || (n ? "" : "Ragiona"));
    }
    else if (s.vista === "fatta") t = esc(readableMsg(s.risposta));
    else t = anteprima(s);
    if (parent) return `<span class="el-pj">${esc(parent)} ›</span> ` + t;
    return (withProject ? `<span class="el-pj">${esc(prefix(s))}</span> · ` : "") + t;
  }
  /** Last readable message for the row: no code blocks, commands, paths or bare links; the first sentence; if nothing
   *  is left, "Ha finito il turno". (coordinator Nov 15: never technical English or the command) */
  function readableMsg(t) {
    const lineList = String(t || "").replace(/```[\s\S]*?(```|$)/g, " ").split("\n").map((r) => r.trim())
      .filter((r) => r && !/^(\$|>|#!|cd |git |npm |cargo |~\/|\/Users\/|https?:\/\/\S+$|[-*] `|\|)/.test(r) && !/^[\w./-]+\.(rs|js|ts|json|html|md|py|sh)\b/.test(r));
    const txt = piano(lineList.join(" ")).replace(/`[^`]*`/g, "").replace(/\s+/g, " ").trim();
    if (!txt || txt.length < 4) return "Ha finito il turno";
    const m = txt.match(/^(.{12,180}?[.!?])(\s|$)/);
    return (m ? m[1] : txt.slice(0, 160)).trim();
  }
  EL.leggibile = readableMsg;
  /** "da" of background jobs: seconds or milliseconds. */
  const fromSecs = (x) => { const n = +x || 0; return n > 1e12 ? n / 1000 : n; };
  const timeMode = (sz) => (sz === "asp" ? "da" : sz === "lav" || sz === "blo" ? "dur" : "fa");
  function timeText(da, m) {
    if (!da) return "";
    const q = quanto(da);
    if (m === "da") return q === "ora" ? "adesso" : "da " + q;
    if (m === "dur") return q;
    return /min$|h$/.test(q) ? q + " fa" : q;
  }
  const pills = (s) => (s.rq_nuove ? `<span class="el-pill" title="${s.rq_nuove === 1 ? "1 risposta nuova" : s.rq_nuove + " risposte nuove"}">${s.rq_nuove} ${s.rq_nuove === 1 ? "nuova" : "nuove"}</span>` : "")
    + (s.rq_te ? `<span class="el-pill" title="${s.rq_te === 1 ? "1 richiesta aspetta te" : s.rq_te + " richieste aspettano te"}">${s.rq_te} per te</span>` : "");
  function tipText(s, sz) {
    const r = [s.nome];
    const reason = sz === "blo" ? stuckReason(s) : "";
    if (reason) r.push(reason);
    if (s.memoria_mb) r.push("Memoria " + gb(s.memoria_mb));
    const workText = (s.sotto_recenti || []).filter((x) => x.stato === "lavora").length, doneText = (s.sotto_recenti || []).length - workText;
    if (workText || doneText) r.push([workText ? workText + " al lavoro" : "", doneText ? doneText + " finiti" : ""].filter(Boolean).join(", ").replace(/^/, "Aiutanti: "));
    if (s.progetto?.nome) r.push("Progetto " + s.progetto.nome);
    return r.join("\n");
  }
  function rowHtml(s, sz, o = {}) {
    const heavy = (s.memoria_mb || 0) > (dati.imp?.avviso_sessione_gb || 2.5) * 1024 ? `<span class="el-pesante">pesante</span>` : "";
    const m = timeMode(sz);
    const cls = `riga r2 ${s.vista} el-${sz} ${selRiga(s.sessione)}${fissata(s) ? " fissa" : ""}${FOTO_NUOVE.has(s.sessione) ? " foto-nuova" : ""}${o.dentro ? " el-dentro" : ""}`;
    // coordinator: the "regia" label is drawn by coordinator.js (.riga-regia); here only the explanation in the tooltip
    const shownName = o.corto || (o.pj && s.progetto?.nome) ? nomeCorto(s) : s.nome, isCoord = coordIds().has(s.sessione);
    return { k: "r:" + s.sessione, h: `<button class="${cls}" data-id="${esc(s.sessione)}" title="${esc(tipText(s, sz) + (isCoord ? "\n" + TEXT.coordTip : ""))}">
      ${iconHtml(s, sz)}<span class="corpo"><span class="r1"><span class="nome">${esc(shownName)}${pills(s)}${o.figlie ? `<span class="el-pill el-nfig" title="${esc(TEXT.childTip)}">${o.figlie} sotto-chat</span>` : ""}${heavy}</span>${badgeCol(s.sessione)}${sz === "lav" ? `<span class="el-pensa${inBackground(s) ? " dolce" : ""}" aria-hidden="true" title="${inBackground(s) ? "Aspetta un lavoro in background" : "Sta lavorando"}"><i></i><i></i><i></i></span>` : ""}<span class="ora el-t" data-da="${inBackground(s) ? fromSecs(bgOf(s)[0].da) : s.da || 0}" data-m="${m}">${timeText(inBackground(s) ? fromSecs(bgOf(s)[0].da) : s.da, m)}</span></span>
      <span class="anteprima">${textStr(s, sz, o.pj, o.madre)}</span></span>${s.vista === "aspetta" ? `<span class="non-serve" data-nonserve="${esc(s.sessione)}" title="Non devo fare niente: sposta in Da leggere">Non serve</span>` : ""}</button>` };
  }
  /** Sub-chat under its main one: one row, indented (icon with state, name · what it does, time). */
  function childRow(s, sz) {
    const msg = inBackground(s) ? "Aspetta un lavoro in background" + (bgOf(s)[0].descrizione ? " · " + piano(bgOf(s)[0].descrizione) : "") : sz === "lav" ? (s.ora_fa?.azione ? `${NOMI_AZ[s.ora_fa.azione.nome] || ""} ${s.ora_fa.azione.testo || ""}`.trim() : piano(vero(s.chiesto)) || "Ragiona") : readableMsg(s.risposta);
    const m = timeMode(sz);
    return { k: "r:" + s.sessione, h: `<button class="riga r2 el-una el-figlia ${s.vista} el-${sz} ${selRiga(s.sessione)}" data-id="${esc(s.sessione)}" title="${esc(s.nome + "\n" + (coordIds().has(s.sessione) ? TEXT.coordTip : TEXT.childTip) + "\n" + msg)}">
      ${iconHtml(s, sz, 18)}<span class="corpo"><span class="r1"><span class="nome">${esc(nomeCorto(s))}<span class="el-msg"> · ${esc(msg)}</span></span>${badgeCol(s.sessione)}${sz === "lav" ? `<span class="el-pensa${inBackground(s) ? " dolce" : ""}" aria-hidden="true" title="${inBackground(s) ? "Aspetta un lavoro in background" : "Sta lavorando"}"><i></i><i></i><i></i></span>` : ""}<span class="ora el-t" data-da="${s.da || 0}" data-m="${m}">${timeText(s.da, m)}</span></span></span></button>` };
  }
  /** Compact "Da leggere" row: icon, name · last message on one line, time; "Segna come letta" on hover. */
  function readRow(s) {
    const msg = readableMsg(s.risposta);
    return { k: "r:" + s.sessione, h: `<button class="riga r2 el-una ${s.vista} el-let ${selRiga(s.sessione)}${FOTO_NUOVE.has(s.sessione) ? " foto-nuova" : ""}" data-id="${esc(s.sessione)}" title="${esc(s.nome + "\n" + msg + (s.rq_nuove ? `\n${s.rq_nuove} ${s.rq_nuove === 1 ? "risposta nuova" : "risposte nuove"}` : "") + (s.progetto?.nome ? "\nProgetto " + s.progetto.nome : ""))}">
      ${iconHtml(s, "let", 20)}<span class="corpo"><span class="r1"><span class="nome">${esc(s.progetto?.nome ? nomeCorto(s) : s.nome)}<span class="el-msg"> · ${esc(msg)}</span></span>${badgeCol(s.sessione)}<span class="ora el-t" data-da="${s.da || 0}" data-m="fa">${timeText(s.da, "fa")}</span></span></span><span class="non-serve" data-el-letta="${esc(s.sessione)}" title="Segna come letta: va tra le ferme">Segna come letta</span></button>` };
  }
  const PERSON_ICON = '<svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.5"><circle cx="8" cy="5.5" r="2.5"/><path d="M3.5 13.5c.6-2.4 2.4-3.6 4.5-3.6s3.9 1.2 4.5 3.6" stroke-linecap="round"/></svg>';
  /** Helpers under a row: on the open chat all of them (as before), elsewhere only "N aiutanti finiti" until you open it. */
  function helpersHtml(s, voci) {
    const l = s.sotto_recenti || [];
    if (!l.length) return;
    const selected = s.sessione === scelta;
    if (selected) {
      EL.visti.set(s.sessione, nowSec());
      for (const x of l) voci.push({ k: "sa:" + s.sessione + ":" + x.id, h: `<button class="riga-sa ${x.stato} ${saAperto === x.id && s.sessione === (saSess || scelta) ? "sel" : ""}" data-sess="${esc(s.sessione)}" data-sa="${esc(x.id)}"><span class="ramo"></span><span class="ic">${ICONA_SA}</span><span class="t" title="${esc(TEXT.helperTip)}">${roleChip("helper")}${esc(x.descrizione || x.tipo || "Aiutante")}</span><span class="st">${x.stato === "lavora" ? `<span class="el-t" data-da="${x.inizio || 0}" data-m="dur">${timeText(x.inizio, "dur")}</span>` : x.stato === "finito" ? "finito" : "interrotto"}</span>${window.spostaRiga ? window.spostaRiga(s, x) : ""}</button>` });
      return;
    }
    const seen = EL.visti.get(s.sessione) || 0;
    const doneText = l.filter((x) => x.stato !== "lavora" && (x.fine || 0) > seen);
    if (doneText.length) voci.push({ k: "saf:" + s.sessione, h: `<button class="el-aiut" data-el-apri="${esc(s.sessione)}" title="${esc(TEXT.helperTip + "\n\n" + doneText.map((x) => x.descrizione || x.tipo || "Aiutante").join("\n"))}">${PERSON_ICON}<span><u>${doneText.length === 1 ? "1 aiutante finito" : doneText.length + " aiutanti finiti"}</u> · leggi i risultati</span></button>` });
  }
  const COORD_ICON = '<svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linejoin="round"><path d="M2.5 3.5h11v7h-6l-3 2.5v-2.5h-2z"/></svg>';
  const sectionHtml = (id, t, n) => ({ k: "sez:" + id, h: `<div class="sezione el-sez ${id}">${t}${n != null ? `<span>${n}</span>` : ""}</div>` });
  const plural = (n, a, b) => `${n} ${n === 1 ? a : b}`;

  // ---------- startup: skeletons until the data arrives the first time (startup Nov 16) ----------
  // in-avvio is removed by the first successful "tavolo" (juggl-pronto event). Before that, dati is only {sessioni: []}:
  // no fake zeros, no "Nessuna sessione aperta", no "Memoria 0% di ? GB".
  // r1629: rows and counters depend only on the chats having arrived (first "tavolo"), not on the memory data:
  // the footer has its own check (dati.mac == null), so counters are real on the first draw with data.
  const starting = () => document.documentElement.classList.contains("in-avvio") || (dati.ora == null && dati.mac == null);
  // limits arrive from the server a few seconds after startup: until then skeleton, then "non disponibile"
  const LIMITS_WAIT_MS = 15000;
  const limitsPending = () => dati.limiti === undefined || (dati.limiti == null && performance.now() < LIMITS_WAIT_MS);
  const W_SK = [[58, 86], [44, 72], [66, 90], [50, 78], [38, 68], [62, 82]];
  const skeletonRows = () => `<div class="el-sk-righe" role="status" aria-label="Carico le chat…">${W_SK.map(([a, b]) => `<div class="el-sk-riga" aria-hidden="true"><i class="el-sk el-sk-ic"></i><span><i class="el-sk el-sk-t" style="width:${a}%"></i><i class="el-sk" style="width:${b}%"></i></span></div>`).join("")}</div>`;
  const macSkeleton = () => `<div class="el-mac el-sk-mac" role="status" aria-label="Leggo la memoria del ${NOME_PC}…"><span class="seg el-sk"></span><span class="st" aria-hidden="true"><i class="el-sk" style="width:96px;height:10px"></i><i class="el-sk" style="width:136px;height:8px"></i></span></div><div class="el-sk el-sk-barra" aria-hidden="true"></div>`;
  const quotaSkeleton = () => `<div class="piede-quota el-sk-q" role="status" aria-label="Leggo i limiti di questa settimana…"><i class="el-sk el-sk-anello"></i><span class="q-t" aria-hidden="true"><i class="el-sk" style="width:128px;height:9px"></i><i class="el-sk" style="width:84px;height:8px"></i></span></div>`;
  addEventListener("juggl-pronto", () => disegnaElenco());
  setTimeout(() => { if (dati.limiti == null) disegnaElenco(); }, LIMITS_WAIT_MS + 50);

  // ---------- strip ----------
  // Visible texts added for the user report (Nov 18): one place, easy to move to the translation catalog
  const TEXT = {
    projects: "Progetti",
    noChat: "nessuna chat",
    chats: (n) => (n === 1 ? "1 chat" : n + " chat"),
    open: "Apri",
    openTip: (nome) => "Apre la chat più recente di " + nome,
    newChat: "Nuova chat",
    newChatTip: "Apre una chat nuova in questo progetto",
    more: (n) => "e altri " + n + " progetti: scrivi di più per restringere",
    noResult: "Nessun risultato",
    // roles in the list (user report part 2): short label + tooltip
    coordTip: "Regia: coordina le chat di questo progetto. Smista il lavoro, unisce i risultati e segue le altre chat; di solito non lavora sul codice.",
    child: "sotto-chat",
    childTip: "Sotto-chat: chat a sé, con la sua conversazione, legate alla chat principale. Si aprono e si risponde come alle altre.",
    helper: "aiutante",
    today: (t) => t + " oggi",
    todayTip: "Tempo in cui almeno una chat di questo progetto ha lavorato oggi (le chat che lavorano insieme contano una volta)",
    helperTip: "Aiutante: un lavoro avviato da questa chat per il turno in corso. Non è una chat: finisce da solo e il risultato torna nella chat.",
  };
  /** Role label of a helper row ("aiutante"). Sub-chats are shown by the branch lines and the "N sotto-chat" pill of
   *  their main chat (a label on each child ate the name); the explanation is in their tooltips.
   *  Same look as the "regia" label drawn by coordinator.js. */
  const roleChip = (kind) => `<span class="el-pill el-ruolo" title="${esc(TEXT[kind + "Tip"])}">${TEXT[kind]}</span>`;
  const LABELS = { asp: ["ti aspetta", "ti aspettano"], blo: ["bloccata", "bloccate"], lav: ["lavora", "lavorano"], let: ["da leggere", "da leggere"] };
  function stripHtml(counter, changes, isSimilar) {
    const sk = starting();
    const k = (s) => { const n = counter[s] || 0;
      if (sk) return `<button class="el-k ${s} sk" disabled aria-label="${LABELS[s][1]}: in caricamento"><span class="n"><span class="el-ic"><i>${s === "blo" ? "!" : ""}</i></span><b class="el-sk el-sk-n">0</b></span><span class="l">${LABELS[s][1]}</span></button>`;
      return `<button class="el-k ${s} ${n ? "pieno" : "zero"}" data-el-filtro="${s}" aria-pressed="${EL.filtro === s}" title="Mostra solo: ${LABELS[s][1]} (⌥${"asp blo lav let".split(" ").indexOf(s) + 1})"><span class="n"><span class="el-ic"><i>${s === "blo" ? "!" : ""}</i></span><b>${n}</b></span><span class="l">${n === 1 ? LABELS[s][0] : LABELS[s][1]}</span></button>`; };
    let h = `<div class="el-striscia" role="toolbar" aria-label="Stato delle chat">${k("asp")}${k("blo")}${k("lav")}${k("let")}`;
    if (EL.filtro) h += `<div class="el-info"><span>Solo: ${LABELS[EL.filtro][1]} · ${counter[EL.filtro] || 0} di ${counter.tot}</span><button data-el-tutte>Mostra tutte</button></div>`;
    let over = "";
    if (changes) over += `<button class="el-cambi" data-el-cambi><b>${changes === 1 ? "1 chat ha cambiato stato." : changes + " chat hanno cambiato stato."}</b> ${changes === 1 ? "Si sposta" : "Si spostano"} quando togli il mouse (clic: subito).</button>`;
    if (isSimilar) over += `<div class="el-simile"><span>Chat nuova «<b>${esc(isSimilar.n.nome)}</b>» simile a «${esc(isSimilar.y.nome)}»</span><button data-el-sim-unisci="${esc(isSimilar.n.sessione)}" data-el-sim-a="${esc(isSimilar.y.sessione)}" title="Mette la chat nuova sotto quella che c'era">Unisci</button><button data-el-sim-ignora="${esc(isSimilar.n.sessione)}">Ignora</button></div>`;
    if (over) h += `<div class="el-sopra">${over}</div>`;
    return { k: "striscia", h: h + `</div>` };
  }

  // ---------- footer: Mac state with what to do ----------
  const coordIds = () => new Set(Object.values(dati.regie || {}).map((r) => r.regia).filter(Boolean));
  /** Can be put to rest without losing anything: stopped, not pinned, no helpers, no active processes, no open turn. */
  function restable(s, coords = coordIds()) {
    return s && s.vista === "ferma" && !s.vista_turno && !fissata(s) && s.sessione !== dati.juggl && !special(s) && !diProva(s) && !coords.has(s.sessione)
      && !s.sotto_attivi && !s.lavora_sotto && !(s.sotto_recenti || []).some((x) => x.stato === "lavora")
      && (s.cpu ?? 0) < 2 && !(invii[s.sessione] || []).length && (s.memoria_mb || 0) >= REST_MIN_MB;
  }
  function macLevel() {
    const m = dati.mac || {}, freeUp = m.libera_pct ?? 100;
    return freeUp < (dati.imp?.avviso_mac_libera_pct || 12) ? "limite" : freeUp < MAC_STRAIN_PCT ? "sforzo" : "ok";
  }
  function candidates() {
    const m = dati.mac || {}, freeUp = m.libera_pct ?? 100;
    if (macLevel() === "ok") return [];
    const needs = ((MAC_STRAIN_PCT - freeUp) / 100) * (m.totale_gb || 16) * 1024;
    const coords = coordIds(), out = [];
    let total = 0;
    for (const s of [...dati.sessioni].filter((x) => restable(x, coords)).sort((a, b) => (b.memoria_mb || 0) - (a.memoria_mb || 0))) {
      if (out.length >= REST_MAX || total >= needs) break;
      out.push(s); total += s.memoria_mb || 0;
    }
    return out;
  }
  const gbText = (mb) => gb(mb).replace(/^(\d+) MB$/, "$1 MB");
  function footerHtml() {
    if (dati.mac == null) return macSkeleton() + (limitsPending() ? quotaSkeleton() : quotaPiede((dati.limiti || {}).seven_day));
    const m = dati.mac || {}, used = 100 - (m.libera_pct ?? 100), level = macLevel();
    const dsk = dati.disco_libero_gb;
    const disk = dsk != null && dsk < 15 ? ` · <span class="disco">disco: ${String(dsk < 10 ? dsk.toFixed(1) : Math.round(dsk)).replace(".", ",")} GB liberi</span>` : "";
    const PCN = NOME_PC[0].toUpperCase() + NOME_PC.slice(1), titleStr = { ok: `${PCN} tranquillo`, sforzo: `${PCN} sotto sforzo`, limite: `${PCN} al limite` }[level];   // platform Nov 17
    const c = EL.riposo ? [] : candidates(), mb = c.reduce((a, x) => a + (x.memoria_mb || 0), 0);
    const btn = c.length ? `<button class="el-libera" data-el-libera="${esc(c.map((x) => x.sessione).join(","))}" title="${esc(`Mette a riposo ${plural(c.length, "chat ferma", "chat ferme")}, una alla volta: ${c.map((x) => x.nome).join(", ")}. Si riaprono con un clic.`)}">Libera ${gbText(mb)}</button>` : "";
    let msg = "";
    const R = EL.riposo;
    if (R?.lavoro) msg = `<div class="el-riposo-msg"><span>${R.annulla ? "Riapro" : "Metto a riposo"} ${esc(R.ora || "")}… (${R.i} di ${R.tot})</span></div>`;
    else if (R) msg = `<div class="el-riposo-msg"><span>${R.ids.length ? `${plural(R.ids.length, "chat a riposo", "chat a riposo")}, si riaprono con un clic` : "Nessuna chat messa a riposo: nel frattempo erano cambiate"}</span>${R.ids.length ? `<button data-el-annulla>Annulla</button>` : ""}</div>`;
    const lm = dati.limiti || {};
    return `<div class="el-mac ${level}"><span class="seg"></span><span class="st">${titleStr}<small title="${esc(`Memoria ${used}% di ${m.totale_gb || "?"} GB${disk.replace(/<[^>]+>/g, "")}`)}">Memoria ${used}% di ${m.totale_gb || "?"} GB${disk}</small></span>${btn}</div><div class="barra-mem"><i style="width:${used}%"></i></div>${msg}${limitsPending() ? quotaSkeleton() : quotaPiede(lm.seven_day)}${dati.eco?.attiva ? `<button class="piede-eco" data-consumi title="Modalità Eco: cosa cambia e come spegnerla">Eco attiva<small>chat non fissate su Sonnet</small></button>` : ""}`;
  }
  async function putToRest(ids) {
    if (EL.riposo?.lavoro) return;
    clearTimeout(EL._tr);
    const R = (EL.riposo = { lavoro: true, ids: [], tot: ids.length, i: 0 });
    for (const id of ids) {
      R.i++;
      const s = dati.sessioni.find((x) => x.sessione === id);
      if (!restable(s)) continue;   // re-checked right before: it may have restarted meanwhile
      R.ora = s.nome; disegnaElenco();
      const r = await invoke("iberna", { id }).catch((e) => ({ ok: false, msg: String(e) }));
      if (r?.ok) R.ids.push(id);
    }
    R.lavoro = false; R.ora = "";
    EL._tr = setTimeout(() => { if (EL.riposo === R) { EL.riposo = null; disegnaElenco(); } }, REST_NOTICE_MS);
    if (typeof caricaElenco === "function") caricaElenco(); else disegnaElenco();
  }
  async function undoRest() {
    const ids = EL.riposo?.ids || [];
    if (!ids.length || EL.riposo?.lavoro) return;
    clearTimeout(EL._tr);
    const R = (EL.riposo = { lavoro: true, annulla: true, ids: [], tot: ids.length, i: 0 });
    let ok = 0;
    for (const id of ids) {
      R.i++; R.ora = dati.sessioni.find((x) => x.sessione === id)?.nome || ""; disegnaElenco();
      const r = await invoke("riprendi", { id }).catch((e) => ({ ok: false, msg: String(e) }));
      if (r?.ok !== false) ok++;
    }
    EL.riposo = null;
    toast(ok === ids.length ? (ok === 1 ? "Chat riaperta" : `${ok} chat riaperte`) : `Riaperte ${ok} di ${ids.length}`, 2400);
    if (typeof caricaElenco === "function") caricaElenco(); else disegnaElenco();
  }

  // ---------- similar chats and "Unisci a…" ----------
  const STOP_WORDS = new Set(["chat", "con", "per", "del", "della", "dei", "the", "and", "una", "uno", "nuova", "nuovo"]);
  /** Words of a name that count: lowercase, no trailing numbers ("prova 2" = "prova 3"), no stop words. */
  function nameWords(n) {
    return [...new Set(String(n || "").toLowerCase().replace(/[#(]?\d+[)]?\s*$/g, "").split(/[^\p{L}\p{N}]+/u).filter((w) => w.length > 2 && !STOP_WORDS.has(w) && !/^\d+$/.test(w)))];
  }
  function similarNames(a, b) {
    const x = nameWords(a), y = nameWords(b);
    if (!x.length || !y.length) return false;
    const c = x.filter((w) => y.includes(w)).length;
    return c / (x.length + y.length - c) >= 0.6;
  }
  EL.nomiSimili = similarNames;
  const readSet = (k) => { try { return new Set(JSON.parse(localStorage.getItem(k) || "[]")); } catch { return new Set(); } };
  const saveSet = (k, st) => { try { localStorage.setItem(k, JSON.stringify([...st].slice(-600))); } catch {} };
  EL.note = readSet("el-note"); EL.ignorati = readSet("el-simili-ignorati"); EL.avvisi = [];
  /** Recently appeared chats that look like an already open one: the first time all chats are "known", no notices. */
  function newSimilar(alive) {
    const firstTime = EL.note.size === 0;
    const home = String(dati.home || "").replace(/\/$/, "");
    for (const s of alive) {
      if (EL.note.has(s.sessione)) continue;
      EL.note.add(s.sessione);
      if (firstTime || s.sotto_di || special(s) || s.vista === "ibernata") continue;
      const cwd = String(s.cwd || "").replace(/\/$/, "");
      const y = alive.find((x) => x.sessione !== s.sessione && x.vista !== "ibernata" && !special(x) && x.sotto_di !== s.sessione
        && ((cwd && cwd !== home && String(x.cwd || "").replace(/\/$/, "") === cwd) || (s.progetto?.id && x.progetto?.id === s.progetto.id && similarNames(s.nome, x.nome))));
      if (y) EL.avvisi.push({ n: s.sessione, y: y.sessione });
    }
    saveSet("el-note", EL.note);
    EL.avvisi = EL.avvisi.filter((a) => !EL.ignorati.has(a.n) && alive.some((x) => x.sessione === a.n && !x.sotto_di) && alive.some((x) => x.sessione === a.y));
    const a = EL.avvisi[0];
    return a ? { n: alive.find((x) => x.sessione === a.n), y: alive.find((x) => x.sessione === a.y) } : null;
  }
  async function mergeInto(figlia, principale) {
    const r = await invoke("unisci_chat", { figlia, principale }).catch((x) => ({ ok: false, msg: String(x) }));
    toast(r?.ok ? "Unita: ora è una sotto-chat" : "Non riuscito: " + (r?.msg || ""), 2200);
    EL.avvisi = EL.avvisi.filter((a) => a.n !== figlia);
    if (typeof caricaElenco === "function") caricaElenco(); else disegnaElenco();
  }
  function closeMerge() { document.querySelector(".el-unisci")?.remove(); }
  /** "Unisci a…" panel: first chats of the same project or folder, then the most recent. */
  function openMerge(id, x, y) {
    closeMerge(); closeProj();
    const s = dati.sessioni.find((v) => v.sessione === id); if (!s) return;
    const points = (v) => (v.progetto?.id && v.progetto.id === s.progetto?.id ? 2 : 0) + (v.cwd && v.cwd === s.cwd ? 1 : 0) + (similarNames(v.nome, s.nome) ? 2 : 0);
    const cand = dati.sessioni.filter((v) => v.sessione !== id && v.sessione !== dati.juggl && !special(v) && v.sotto_di !== id && !diProva(v))
      .sort((a, b) => points(b) - points(a) || (b.da || 0) - (a.da || 0)).slice(0, 14);
    const el = document.createElement("div");
    el.className = "el-prog el-unisci"; el.dataset.id = id;
    el.innerHTML = `<h5>Metti «${esc(nomeCorto(s))}» sotto…</h5><input class="el-u-cerca" placeholder="Cerca una chat" spellcheck="false" autocomplete="off">
      <div class="el-u-lista">${cand.map((v) => `<button class="az" data-el-unisci-a="${esc(v.sessione)}">${iconHtml(v, "gr", 18)}<span>${esc(v.nome)}${v.progetto?.nome ? `<small>${esc(v.progetto.nome)}</small>` : ""}</span></button>`).join("") || `<div class="el-vuota">Nessuna chat</div>`}</div>`;
    document.body.appendChild(el);
    const r = el.getBoundingClientRect();
    el.style.left = Math.max(8, Math.min(x, innerWidth - r.width - 8)) + "px";
    el.style.top = Math.max(8, Math.min(y, innerHeight - r.height - 8)) + "px";
    el.querySelector(".el-u-cerca").focus();
  }
  addEventListener("input", (e) => {
    if (!e.target.matches?.(".el-u-cerca")) return;
    const q = e.target.value.trim().toLowerCase();
    e.target.closest(".el-unisci").querySelectorAll("[data-el-unisci-a]").forEach((b) => { b.hidden = !!q && !b.textContent.toLowerCase().includes(q); });
  });

  // ---------- drawing ----------
  // full project list (even without chats), refreshed at most every minute
  EL.progetti = null; let projectsT = 0;
  function refreshProjects() {
    if (Date.now() - projectsT < 60000) return; projectsT = Date.now();
    invoke("progetti").then((pp) => { const firstTime = JSON.stringify(EL.progetti); EL.progetti = Array.isArray(pp) ? pp : []; if (JSON.stringify(EL.progetti) !== firstTime) disegnaElenco(); }).catch(() => {});
  }
  EL.disegna = () => {
    refreshProjects();
    rememberChoice();
    const p = $("#presenza");
    if (p) { p.classList.toggle("on", !!dati.al_mac); p.classList.toggle("el-avvio", starting()); $("span", p).textContent = dati.al_mac ? "Al " + NOME_PC : "Via"; }
    $("#piede").className = "piede" + (macLevel() === "limite" ? " allarme" : "");
    // jank r1629: update the footer in place (text and widths only), so hover on its buttons survives each poll
    { const pe = $("#piede"), h = footerHtml(); if (pe && pe._h !== h) { if (typeof morfaFigli === "function") morfaFigli(pe, h); else pe.innerHTML = h; pe._h = h; } }
    ricordaSA();
    const allChats = visibili();
    const jug = allChats.find((x) => x.sessione === dati.juggl);
    const ss = allChats.filter((x) => x.sessione !== dati.juggl);
    const voci = [];
    if (jug) {
      const sz = liveSec(jug);
      voci.push({ k: "juggl", h: `<button class="riga riga-juggl ${selRiga(jug.sessione)} ${jug.vista}" data-id="${esc(jug.sessione)}"><span class="punto"></span>${avatar(jug)}
      <span class="corpo"><span class="r1"><span class="nome">Juggl</span>${badgeCol(jug.sessione)}${(jug.rq_nuove || 0) + (jug.rq_te || 0) ? pills(jug) : ""}<span class="ora">${jug.vista === "lavora" ? "" : quanto(jug.da)}</span></span>
      <span class="anteprima">${jug.vista === "lavora" ? scrive + textStr(jug, sz, false) : esc(piano(jug.risposta) || "Assistente generale: smista, unisce e segue tutte le sessioni")}</span></span></button>` });
    }
    const el = $("#elenco");
    // fixed order: with the pointer over it the sections stay those from when it entered
    const isStopped = !!(sopraElenco && ordineFermo);
    if (!isStopped) EL.sezFerma = new Map(ss.map((s) => [s.sessione, liveSec(s)]));
    const sec = (s) => (isStopped && EL.sezFerma.get(s.sessione)) || liveSec(s);
    // real strip numbers (also with fixed order) and chats that will move
    const alive = countable(), counter = { tot: alive.filter((x) => x.vista !== "ibernata").length };
    let changes = 0;
    for (const x of alive) { const z = liveSec(x); counter[z] = (counter[z] || 0) + 1; if (isStopped && EL.sezFerma.has(x.sessione) && EL.sezFerma.get(x.sessione) !== z) changes++; }
    // sub-chats: only count if the main one is in the list (closed or archived: the sub-chat comes back by itself)
    const byId = new Map(ss.map((s) => [s.sessione, s]));
    const parentOf = (s) => (s.sotto_di && s.sotto_di !== s.sessione && byId.has(s.sotto_di) ? byId.get(s.sotto_di) : null);
    const childrenOf = new Map();
    for (const s of ss) { const m = parentOf(s); if (m) { if (!childrenOf.has(m.sessione)) childrenOf.set(m.sessione, []); childrenOf.get(m.sessione).push(s); } }
    const parentName = (s) => { const m = parentOf(s); return m ? nomeCorto(m) : ""; };
    const opts = (s, o) => ({ ...o, madre: parentName(s), figlie: (childrenOf.get(s.sessione) || []).length });
    voci.push(stripHtml(counter, changes, newSimilar(alive)));
    // user report (Nov 18): with a search, matching projects first, even those with no chats
    const pm = cerca.trim() ? projectMatches(cerca) : [];
    projectResults(voci, pm);
    if (!ss.length) {
      if (!cerca && starting()) voci.push({ k: "scheletro", h: skeletonRows() });
      else if (cerca) { if (!pm.length) voci.push({ k: "vuoto", h: `<div class="vuoto">${TEXT.noResult}</div>` }); voci.push(...vociArchivio()); }
      else { voci.push({ k: "vuoto", h: `<div class="vuoto">Nessuna sessione aperta</div>` }); noChatProjects(voci, true); voci.push(...vociArchivio()); }
      scivola(el, () => riconciliaChiavi(el, voci)); el._ordine = ""; rimettiFiltriArch(); return;
    }
    const F = EL.filtro, withSearch = !!cerca.trim();
    const vedi = (z) => !F || F === z;
    const waitingList = ss.filter((s) => sec(s) === "asp").sort((a, b) => (a.da || 0) - (b.da || 0));
    const stuckList = ss.filter((s) => sec(s) === "blo");
    if (waitingList.length && vedi("asp")) { voci.push(sectionHtml("asp", "Ti aspettano", waitingList.length)); for (const s of waitingList) { voci.push(rowHtml(s, "asp", opts(s, { pj: true }))); helpersHtml(s, voci); } }
    if (stuckList.length && vedi("blo")) { voci.push(sectionHtml("blo", "Bloccate", stuckList.length)); for (const s of stuckList) { voci.push(rowHtml(s, "blo", opts(s, { pj: true }))); helpersHtml(s, voci); } }
    // To read (coordinator Nov 15): compact section, one row per chat (name · last readable message), "Segna tutto come letto",
    // "Segna come letta" on hover; with more than 8 the 8 most recent and "Mostra altre N" (all with the "to read" filter)
    const toRead = ss.filter((s) => sec(s) === "let").sort((a, b) => (b.da || 0) - (a.da || 0));
    if (toRead.length && vedi("let")) {
      voci.push({ k: "sez:let", h: `<div class="sezione el-sez let">Da leggere<span>${toRead.length}</span><button class="el-tutte-lette" data-el-tutteletta="${esc(toRead.map((s) => s.sessione).join(","))}" title="Segna come lette tutte le chat da leggere">Segna tutto come letto</button></div>` });
      const MAXL = 8, allToRead = F === "let" || EL.fermeAperte.has("lett:tutte") || toRead.length <= MAXL + 1;
      for (const s of allToRead ? toRead : toRead.slice(0, MAXL)) { voci.push(readRow(s)); helpersHtml(s, voci); }
      if (!allToRead) voci.push({ k: "lett:altre", h: `<button class="el-ferme el-altre" data-el-ferme="lett:tutte"><span class="el-ic"><i></i></span><span><b>Mostra altre ${toRead.length - MAXL}</b></span></button>` });
    }
    EL.gruppoDi = new Map();
    if (F === "asp" || F === "blo" || F === "let") {
      if (!(F === "asp" ? waitingList : F === "blo" ? stuckList : toRead).length) voci.push({ k: "vuota", h: `<div class="el-vuota">${F === "asp" ? "Nessuna chat ti aspetta." : F === "blo" ? "Nessuna chat bloccata." : "Niente da leggere."}</div>` });
    } else {
      const remaining = ss.filter((s) => !["asp", "blo", "let"].includes(sec(s)));
      // pinned: in the order you pinned them (visibili), with the project
      const pinnedList = remaining.filter((s) => fissata(s) && ["lav", "fer"].includes(sec(s)) && vedi(sec(s)));
      if (pinnedList.length) { voci.push(sectionHtml("fis", "Fissate", pinnedList.length)); for (const s of pinnedList) { voci.push(rowHtml(s, sec(s), opts(s, { pj: true }))); helpersHtml(s, voci); } }
      // first impression (Nov 16): real projects, chats without a project and old or empty chats well separated.
      // Old or empty (hidden in a "Mostra" row): stopped or resting with no user message, or stopped for
      // more than GIORNI_VECCHIA days. Never pinned ones, coordinators, working, waiting, stuck or to-read ones. With search
      // or a filter all are shown.
      const coords = coordIds();
      const hideOld = !F && !withSearch;
      const oldOrEmpty = (s) => hideOld && ["fer", "riposo"].includes(sec(s)) && !fissata(s) && !coords.has(s.sessione)
        && (!String(vero(s.chiesto) || "").trim() || nowSec() - (s.da || 0) > OLD_DAYS * 86400);
      const oldOnes = remaining.filter((s) => !fissata(s) && oldOrEmpty(s));
      const restV = remaining.filter((s) => !oldOnes.includes(s));
      const real_p = (m) => !!m.progetto?.id && m.progetto.id !== "locale" && !special(m);
      const byGroup = new Map(), looseChats = [];
      for (const s of restV) {
        if (fissata(s)) continue;
        const z = sec(s); if (z === "riposo" || z === "pronta") continue;
        // a sub-chat goes in the same place as its main one
        const m = parentOf(s) || s;
        if (!real_p(m)) { looseChats.push(s); continue; }
        const key = m.progetto.id;
        if (!byGroup.has(key)) byGroup.set(key, { key, p: m.progetto, list: [] });
        byGroup.get(key).list.push(s);
      }
      const aboveOf = (pid) => ss.filter((x) => x.progetto?.id === pid && ["asp", "blo", "let"].includes(sec(x))).length;
      const vp = [], onlyStopped = [];
      /** Rows of a group (or of loose chats): working and to read, with sub-chats right below. */
      function rowsOf(arr, list, o) {
        const vivi = list.filter((s) => ["lav", "let"].includes(sec(s)) && vedi(sec(s)));
        const liveHere = new Set(vivi.map((s) => s.sessione));
        const underneath = (s) => { const m = parentOf(s); return m && liveHere.has(m.sessione); };
        const childrenHere = (m) => list.filter((s) => parentOf(s)?.sessione === m.sessione && ["lav", "fer"].includes(sec(s)) && vedi(sec(s)));
        const rank = (s) => (coords.has(s.sessione) ? 0 : sec(s) === "lav" ? 1 : 2);
        for (const s of vivi.filter((s) => !underneath(s)).sort((a, b) => rank(a) - rank(b))) {
          arr.push(rowHtml(s, sec(s), opts(s, o))); helpersHtml(s, arr);
          for (const f of childrenHere(s)) arr.push(childRow(f, sec(f)));
        }
        return { vivi, ferL: F ? [] : list.filter((s) => sec(s) === "fer" && !underneath(s)).sort((a, b) => (b.da || 0) - (a.da || 0)) };
      }
      // 1) Projects: always with the header, even with a single chat
      for (const { key, p, list } of byGroup.values()) {
        const hasAlive = list.some((s) => ["lav", "let"].includes(sec(s)) && vedi(sec(s)));
        if (!hasAlive) { const fer = F ? [] : list.filter((s) => sec(s) === "fer"); if (fer.length) onlyStopped.push({ key, p, fer }); continue; }
        for (const s of list) EL.gruppoDi.set(s.sessione, key);
        const aperto = F || withSearch ? true : gruppiScelti.has(key) ? !!gruppiScelti.get(key) : true;
        const dentro = list.some((s) => s.sessione === scelta);
        const n = (z) => list.filter((s) => sec(s) === z).length, su = aboveOf(key);
        const fer = list.filter((s) => sec(s) === "fer");
        const R = (dati.regie || {})[key], coordOk = !!R?.regia && (dati.imp?.regia_gruppi || {})[key] !== false;
        const sum = aperto
          ? (su && !F ? `<span title="${su === 1 ? "1 chat di questo progetto è in cima" : su + " chat di questo progetto sono in cima"}, in Ti aspettano, Bloccate o Da leggere">${su} sopra ↑</span>` : "")
          : [su ? `<span class="el-o">${su} sopra ↑</span>` : "", n("lav") ? plural(n("lav"), "lavora", "lavorano") : "", n("let") ? n("let") + " da leggere" : "", fer.length ? plural(fer.length, "ferma", "ferme") : ""].filter(Boolean).slice(0, 2).join(" · ");
        vp.push({ k: "g:" + key, h: `<button class="riga-gruppo g2${aperto ? " aperto" : ""}${dentro && !aperto ? " sel" : ""}" style="--c:${colorOf(p)}" data-gruppo="${esc(key)}" title="${aperto ? "Chiudi" : "Apri"} ${esc(p?.nome || "questo progetto")} (← →)">${iconHtml({ nome: p?.nome || "", progetto: p }, "gr", 20)}<span class="el-gn">${esc(p?.nome || "Progetto")}</span><span class="el-gs">${sum}${p?.id ? todayHtml(p.id, !!sum) : ""}</span>${coordOk ? `<span class="el-regia${EL.scrivi.has(key) ? " on" : ""}" role="button" tabindex="0" data-el-regia="${esc(key)}" title="Scrivi alla regia di ${esc(p?.nome || "questo gruppo")}: la passa alla chat giusta">${COORD_ICON}</span>` : ""}<span class="gr-freccia">${FRECCIA_G}</span></button>` });
        if (window.regiaVoci) vp.push(...window.regiaVoci({ pid: key, p, aperto, membri: list }));
        if (!aperto) continue;
        const { ferL } = rowsOf(vp, list, { corto: true, dentro: true });
        stoppedChats(vp, key, ferL, null, true);
      }
      if (vp.length) voci.push(sectionHtml("prog", "Progetti"), ...vp);
      // 2) Chats without a project: speech bubble icon, "in <cartella>" or "Chat generale"; stopped ones in one row
      const vsc = [];
      const { ferL: scStopped } = rowsOf(vsc, looseChats, { pj: true });
      if (vsc.length || scStopped.length) {
        voci.push(sectionHtml("sciolte", "Chat senza progetto", looseChats.filter((s) => vedi(sec(s))).length), ...vsc);
        stoppedChats(voci, "sciolte-ferme", scStopped, null, false);
      }
      // 3) Projects with only stopped chats, one row each
      if (onlyStopped.length) {
        voci.push(sectionHtml("solo", "Progetti fermi", onlyStopped.reduce((a, x) => a + x.fer.length, 0)));
        for (const x of onlyStopped) { const nome = x.p?.nome || prefix(x.fer[0]); stoppedChats(voci, "sf:" + x.key, x.fer.sort((a, b) => (b.da || 0) - (a.da || 0)), nome, false, "fer", x.p?.id ? x.p : { id: "sf:" + x.key, nome }); }
      }
      if (!F) {
        const restingList = restV.filter((s) => sec(s) === "riposo"), pr = restV.filter((s) => sec(s) === "pronta");
        if (restingList.length) stoppedChats(voci, "riposo", restingList, "A riposo", false, "riposo");
        if (pr.length) stoppedChats(voci, "pronte", pr, "Progetti importati, mai aperti", false, "pronta");
      }
      // 3b) Projects with no chats at all (also those found by Juggl for a new user): one row, expandable.
      // With a search they are already in the results on top.
      if (!F && !withSearch) noChatProjects(voci, false);
      // 4) Old or empty chats: hidden in one row
      if (oldOnes.length) {
        const ap = EL.fermeAperte.has("vecchie");
        voci.push({ k: "f:vecchie", h: `<button class="el-ferme el-vecchie${ap ? " aperto" : ""}" data-el-ferme="vecchie" aria-expanded="${ap}" title="Chat senza messaggi tuoi o ferme da oltre ${OLD_DAYS} giorni"><span class="el-ic"><i></i></span><span><b>${plural(oldOnes.length, "chat vecchia o vuota", "chat vecchie o vuote")}</b>${ap ? "" : " · mostra"}</span></button>` });
        if (ap) for (const s of oldOnes) voci.push(rowHtml(s, sec(s) === "riposo" ? "riposo" : "fer", opts(s, { pj: true })));
      }
      const vs = vsc;
      if (!vp.length && !vs.length && F && !pinnedList.length) voci.push({ k: "vuota", h: `<div class="el-vuota">Nessuna chat al lavoro.</div>` });
    }
    if (window.localeVoci) voci.push(...window.localeVoci());
    voci.push(...vociArchivio());
    const orderList = voci.map((v) => v.k).join("|");
    if (el._ordine === orderList) riconciliaChiavi(el, voci); else scivola(el, () => { riconciliaChiavi(el, voci); aggiornaTempi(); });
    el._ordine = orderList;
    rimettiFiltriArch();
    aggiornaTempi();
    // fast chats: after every chat change the neighbors (list and history) are prepared in a queue, one at a time
    if (scelta && scelta !== EL.ultimaPrep) { EL.ultimaPrep = scelta; try { window.preparaChat?.(EL.vicine()); } catch {} }
    const sh = el.querySelector(".el-striscia")?.offsetHeight;
    if (sh) el.style.setProperty("--el-top", sh - 4 + "px");
  };
  /** "3 ferme · a, b, c" (or "Progetto · a, b, c"): click = show or hide the rows. With search they are always open. */
  /** Projects with no chat (sidebar zone): one row; open by default when there are no chats at all. */
  function noChatProjects(voci, openByDefault) {
    if (!EL.progetti) return;
    const withChats = new Set(dati.sessioni.map((x) => x.progetto?.id).filter(Boolean));
    const noProject = EL.progetti.filter((p) => p.id && p.id !== "locale" && !withChats.has(p.id));
    if (noProject.length) {
      // open by default: a click closes it (the same key toggles, inverted)
      const ap = openByDefault ? !EL.fermeAperte.has("senza-chat") : EL.fermeAperte.has("senza-chat");
      voci.push({ k: "f:senza-chat", h: `<button class="el-ferme${ap ? " aperto" : ""}" data-el-ferme="senza-chat" aria-expanded="${ap}" title="Progetti che conosco ma senza chat aperte"><span class="el-ic"><i></i></span><span><b class="el-p">Progetti senza chat</b> <b>${noProject.length}</b>${ap ? "" : " · " + esc(noProject.slice(0, 5).map((p) => p.nome).join(", ")) + (noProject.length > 5 ? ` e altri ${noProject.length - 5}` : "")}</span></button>` });
      if (ap) for (const p of noProject) voci.push({ k: "pz:" + p.id, h: `<div class="el-pz">${iconHtml({ nome: p.nome, progetto: p }, "gr", 20)}<span class="el-pz-n">${esc(p.nome)}${p.auto ? `<small>trovato da Juggl</small>` : ""}</span><button class="el-pz-b" data-el-nuova-in="${esc(p.id)}" title="Apri una chat nuova in questo progetto">Nuova chat</button></div>` });
    }
  }
  /** prog: project of the row (only "Progetti fermi"): instead of the dash the logo or initials, dimmed like stopped ones. */
  function stoppedChats(voci, key, list, labelText, dentro, z = "fer", proj = null) {
    if (!list.length) return;
    const ap = EL.fermeAperte.has(key) || !!cerca.trim();
    const MAXN = 5, everyone = list.map((s) => dentro ? nomeCorto(s) : s.nome);
    const nameList = esc(everyone.slice(0, MAXN).join(", ")) + (everyone.length > MAXN ? ` e altre ${everyone.length - MAXN}` : "");   // never a wall of names (test Nov 15: "A riposo 69")
    const headHtml = labelText ? `<b class="el-p">${esc(labelText)}</b>${z !== "fer" ? ` <b>${list.length}</b>` : ""}` : `<b>${plural(list.length, "ferma", "ferme")}</b>`;
    voci.push({ k: "f:" + key, h: `<button class="el-ferme${dentro ? " el-dentro" : ""}${ap ? " aperto" : ""}" data-el-ferme="${esc(key)}" aria-expanded="${ap}" title="${ap ? "Nascondi" : "Mostra"} queste chat${everyone.length > MAXN ? ": " + esc(everyone.join(", ")) : ""}">${proj ? iconHtml({ nome: labelText, progetto: proj }, "fer", 18) : `<span class="el-ic"><i></i></span>`}<span>${headHtml}${ap ? "" : " · " + nameList}</span></button>` });
    if (ap) for (const s of list) voci.push(rowHtml(s, z, { corto: dentro, dentro, pj: !dentro && !labelText }));
  }
  /** On-screen order of chats (for ↑↓): what you see, not the data order. */
  EL.ordine = () => { const l = [...document.querySelectorAll("#elenco .riga[data-id]")].map((e) => e.dataset.id); return l.length ? l : null; };

  // ---------- "Icona e colore del progetto…" (right click on a row or on the group header) ----------
  const projectOf = (pid) => dati.sessioni.find((x) => x.progetto?.id === pid)?.progetto || null;
  function closeProj() { document.querySelector(".el-prog")?.remove(); }
  function openProj(pid, x, y) {
    closeProj();
    const p = projectOf(pid); if (!p) return toast("Progetto non trovato");
    const c = colorOf(p), picked = p.colore && PALETTE.includes(p.colore) ? p.colore : "";
    const el = document.createElement("div");
    el.className = "el-prog"; el.dataset.pid = pid;
    el.innerHTML = `<h5>${iconHtml({ nome: p.nome, progetto: p }, "gr", 22)}${esc(p.nome)}</h5>
      <div class="sw" role="radiogroup" aria-label="Colore del progetto">${PALETTE.map((t) => `<button style="background:${t}" data-el-col="${t}" aria-pressed="${picked === t}" title="${t === c && !picked ? "Colore automatico" : "Usa questo colore"}"></button>`).join("")}</div>
      <button class="auto" data-el-col="" aria-pressed="${!picked}">Colore automatico</button>
      <hr><button class="az" data-el-logo="sito">Icona dal sito…${p.sito ? `<small>${esc(p.sito.replace(/^https?:\/\//, ""))}</small>` : ""}</button>
      <button class="az" data-el-logo="file">Scegli un file…</button>
      <button class="az" data-el-logo="iniziali">Usa le iniziali</button>
      <hr><label class="el-onl"><input type="checkbox" data-el-online ${dati.imp?.loghi_online ? "checked" : ""}><span>Cerca i loghi dei progetti online<small>Juggl apre il sito del progetto (una volta al giorno, solo immagini). Spento di base.</small></span></label>
      ${dati.imp?.loghi_online ? `<label class="el-onl el-escl"><input type="checkbox" data-el-escludi ${p.cerca_online === false ? "checked" : ""}><span>Non cercare online questo progetto</span></label>` : ""}
      ${p.logo_auto ? `<small style="color:var(--ink3);display:block;margin-top:6px">Icona trovata da Juggl (${esc(p.logo_auto)})</small>` : ""}`;
    document.body.appendChild(el);
    const r = el.getBoundingClientRect();
    el.style.left = Math.max(8, Math.min(x, innerWidth - r.width - 8)) + "px";
    el.style.top = Math.max(8, Math.min(y, innerHeight - r.height - 8)) + "px";
  }
  async function projAction(e) {
    const box = e.target.closest(".el-prog"); if (!box) return false;
    const pid = box.dataset.pid;
    const col = e.target.closest("[data-el-col]");
    if (col) {
      const r = await invoke("progetto_colore", { pid, colore: col.dataset.elCol }).catch((x) => ({ ok: false, msg: String(x) }));
      if (!r?.ok) return toast("Non riuscito: " + (r?.msg || "")), true;
      for (const s of dati.sessioni) if (s.progetto?.id === pid) { if (col.dataset.elCol) s.progetto.colore = col.dataset.elCol; else delete s.progetto.colore; }
      redraw(); const { left, top } = box.getBoundingClientRect(); openProj(pid, left, top); return true;
    }
    const ex = e.target.closest("[data-el-escludi]");
    if (ex) {
      const si = projectOf(pid)?.cerca_online === false;   // it was excluded: let it back in
      const r = await invoke("progetto_cerca_online", { pid, si }).catch((x) => ({ ok: false, msg: String(x) }));
      if (r?.ok) for (const s of dati.sessioni) if (s.progetto?.id === pid) { if (si) delete s.progetto.cerca_online; else s.progetto.cerca_online = false; }
      else toast("Non riuscito: " + (r?.msg || ""));
      ex.checked = projectOf(pid)?.cerca_online === false; return true;
    }
    const on = e.target.closest("[data-el-online]");
    if (on) {
      const acceso = !dati.imp?.loghi_online;
      const r = await invoke("loghi_online_imposta", { acceso }).catch((x) => ({ ok: false, msg: String(x) }));
      if (r?.ok) { dati.imp = { ...(dati.imp || {}), loghi_online: acceso }; toast(acceso ? "Cerco i loghi online: al prossimo giro" : "Niente più ricerche online dei loghi", 2200); }
      else toast("Non riuscito: " + (r?.msg || ""));
      const { left, top } = box.getBoundingClientRect(); openProj(pid, left, top); return true;
    }
    const lg = e.target.closest("[data-el-logo]"); if (!lg) return true;
    const kind = lg.dataset.elLogo;
    if (kind === "sito") {
      const p = projectOf(pid);
      lg.outerHTML = `<form data-el-sito><input name="u" placeholder="indirizzo del sito, es. esempio.it" value="${esc((p?.sito || "").replace(/^https?:\/\//, ""))}" spellcheck="false" autocomplete="off"><button type="submit">Prendi</button></form>`;
      box.querySelector("input")?.focus(); return true;
    }
    if (kind === "iniziali") {
      const r = await invoke("progetto_logo_iniziali", { pid }).catch((x) => ({ ok: false, msg: String(x) }));
      toast(r?.ok ? "Iniziali nel colore del progetto" : "Non riuscito: " + (r?.msg || "")); closeProj(); caricaElenco(); return true;
    }
    if (kind === "file") {
      const sid = dati.sessioni.find((x) => x.progetto?.id === pid)?.sessione; closeProj(); if (!sid) return true;
      const i = document.createElement("input"); i.type = "file"; i.accept = "image/*";
      i.onchange = () => { const f = i.files?.[0]; if (!f) return; const rd = new FileReader(); rd.onload = async () => { const r = await invoke("imposta_logo", { id: sid, dati: rd.result }); toast(r.ok ? "Icona aggiornata" : r.msg); caricaElenco(); }; rd.readAsDataURL(f); };
      i.click(); return true;
    }
    return true;
  }
  addEventListener("submit", async (e) => {
    const f = e.target.closest?.("[data-el-sito]"); if (!f) return;
    e.preventDefault();
    const pid = f.closest(".el-prog")?.dataset.pid, u = f.querySelector("input").value.trim(), b = f.querySelector("button");
    if (!pid || !u) return;
    b.disabled = true; b.textContent = "Cerco…";
    const r = await invoke("progetto_logo_sito", { pid, sito: u }).catch((x) => ({ ok: false, msg: String(x) }));
    toast(r?.ok ? "Icona presa dal sito" : "Non trovata: " + (r?.msg || ""), 2400);
    if (r?.ok) { closeProj(); caricaElenco(); } else { b.disabled = false; b.textContent = "Prendi"; }
  }, true);
  // right-click menu entry (rows: after "Cambia logo…"; headers: after the coordinator entries)
  let ctxProj = null;
  addEventListener("contextmenu", (e) => {
    const r = e.target.closest?.(".elenco .riga[data-id], .elenco .riga-gruppo[data-gruppo]");
    ctxProj = null;
    if (!r) return;
    const pid = r.dataset.gruppo || dati.sessioni.find((x) => x.sessione === r.dataset.id)?.progetto?.id;
    if (pid && pid !== "locale") ctxProj = { pid, x: e.clientX, y: e.clientY };
    ctxRow = r.dataset.id && r.dataset.id !== dati.juggl ? { id: r.dataset.id, x: e.clientX, y: e.clientY } : null;
  }, true);
  let ctxRow = null;
  const menuEl = document.getElementById("menu");
  if (menuEl) new MutationObserver(() => {
    // sub-chats: "Unisci a…" and, if it already is one, "Togli da sotto"
    if (ctxRow && menuEl.classList.contains("on") && !menuEl.querySelector("[data-el-unisci]")) {
      const s = dati.sessioni.find((x) => x.sessione === ctxRow.id);
      if (s && !special(s)) {
        const b = `<button data-el-unisci="${esc(s.sessione)}">Unisci a un'altra chat…</button>${s.sotto_di ? `<button data-el-separa="${esc(s.sessione)}">Togli da sotto</button>` : ""}`;
        const afterEl = menuEl.querySelector('[data-azione="logo"]') || menuEl.querySelector('[data-azione="fissa"]');
        if (afterEl) afterEl.insertAdjacentHTML("afterend", b); else menuEl.insertAdjacentHTML("beforeend", (menuEl.children.length ? "<hr>" : "") + b);
      }
    }
    if (!ctxProj || !menuEl.classList.contains("on") || menuEl.querySelector("[data-el-prog]")) return;
    const b = `<button data-el-prog="${esc(ctxProj.pid)}">Icona e colore del progetto…</button>`;
    const logo = menuEl.querySelector('[data-azione="logo"]');
    if (logo) logo.insertAdjacentHTML("afterend", b); else menuEl.insertAdjacentHTML("beforeend", (menuEl.children.length ? "<hr>" : "") + b);
  }).observe(menuEl, { childList: true, attributes: true, attributeFilter: ["class"] });

  // ---------- navigate: history of opened chats and neighbor chats in the list ----------
  EL.storia = []; EL.pos = -1; let navTarget = null;
  const exists = (id) => id === dati.juggl || dati.sessioni.some((x) => x.sessione === id);
  function rememberChoice() {
    const id = typeof scelta !== "undefined" ? scelta : null;
    if (!id || id === EL.storia[EL.pos]) return;
    if (id === navTarget) { navTarget = null; return; }   // arrived with ⌘[ / ⌘]: does not rewrite the history
    EL.storia = EL.storia.slice(0, EL.pos + 1).filter((x) => x !== id);
    EL.storia.push(id); if (EL.storia.length > 60) EL.storia.shift();
    EL.pos = EL.storia.length - 1;
  }
  function go(id) { if (!id) return; elencoAttivo = true; navTarget = id; scegli(id); }
  function navigateTo(where) {
    rememberChoice();
    if (where === "indietro" || where === "avanti") {
      const d = where === "indietro" ? -1 : 1;
      for (let p = EL.pos + d; p >= 0 && p < EL.storia.length; p += d) if (exists(EL.storia[p])) { EL.pos = p; return go(EL.storia[p]); }
      return;
    }
    const ids = EL.ordine() || []; if (!ids.length) return;
    const i = ids.indexOf(scelta);
    go(ids[i < 0 ? 0 : (i + (where === "succ" ? 1 : -1) + ids.length) % ids.length]);
  }
  EL.naviga = navigateTo;
  /** Chats worth keeping ready: previous and next in the list, previous in the history. */
  EL.vicine = () => {
    const ids = EL.ordine() || [], i = ids.indexOf(scelta);
    return [...new Set([i >= 0 ? ids[(i + 1) % ids.length] : null, i >= 0 ? ids[(i - 1 + ids.length) % ids.length] : null, EL.storia[EL.pos - 1]].filter((x) => x && x !== scelta))];
  };
  try { window.__TAURI__.event.listen("naviga", (e) => navigateTo(String(e.payload || ""))); } catch {}
  // Windows and Linux: no native menu, the page handles the keys (also inside the input box)
  addEventListener("keydown", (e) => {
    if (!WIN) return;
    if (e.ctrlKey && !e.altKey && (e.key === "[" || e.key === "]")) { e.preventDefault(); e.stopPropagation(); return navigateTo(e.key === "[" ? "indietro" : "avanti"); }
    if (e.ctrlKey && e.key === "Tab") { e.preventDefault(); e.stopPropagation(); return navigateTo(e.shiftKey ? "prec" : "succ"); }
  }, true);

  // ---------- media strip (Nov 16): readable captions ----------
  // Before: under every thumbnail the full chat name ("Progetto · lavoro") in 96-136 px, so many identical "Pro…".
  // Now: line 1 the chat name without the project in front (full width, no logo), line 2 small time · project; consecutive photos of the
  // same chat: the name only on the first, the others only the time. The tile does not change (box, skeleton, image,
  // width): only the .fm-did caption is adjusted, once per tile (a reused tile stays adjusted).
  function fixCaption(t) {
    const m = t._m, d = t.querySelector(".fm-did");
    if (!m || !d || d.dataset.el) return;
    const nm = d.querySelector(".fm-nm"), timeStr = d.querySelector(".fm-ora");
    if (!nm || !timeStr) return;
    d.dataset.el = "1";
    const proj = m.progetto?.nome || "";
    nm.textContent = (proj ? nomeCorto(m) : m.nome) || m.nome || "";
    const textStr = document.createElement("span"); textStr.className = "el-fm-tx";
    const underneath = document.createElement("span"); underneath.className = "el-fm-sotto";
    nm.replaceWith(textStr); textStr.append(nm, underneath); underneath.append(timeStr);
    // time first (always whole), then the project which gets cut if needed
    if (proj) { const p = document.createElement("span"); p.className = "el-fm-p"; p.textContent = " · " + proj; underneath.append(p); }
  }
  function fixStrip() {
    const tt = [...document.querySelectorAll("#fm-binario .fm-t")];
    let firstTime = null;
    for (const t of tt) {
      fixCaption(t);
      // same chat as the previous tile: no repeated name
      t.classList.toggle("el-fm-segue", !!firstTime && t.dataset.nmSess && t.dataset.nmSess === firstTime.dataset.nmSess);
      firstTime = t;
    }
  }
  EL.sistemaNastro = fixStrip;
  const track = document.getElementById("fm-binario") || document.getElementById("nastro");
  if (track) new MutationObserver(() => { clearTimeout(fixStrip._t); fixStrip._t = setTimeout(fixStrip, 0); }).observe(track, { childList: true, subtree: true });

  // ---------- clicks and keyboard ----------
  const redraw = () => disegnaElenco();
  function applyFilter(z) { EL.filtro = EL.filtro === z ? "" : z; redraw(); $("#elenco")?.scrollTo({ top: 0 }); }
  addEventListener("click", (e) => {
    const t = e.target;
    const mu = t.closest?.("[data-el-unisci]");
    if (mu) { e.preventDefault(); e.stopPropagation(); document.getElementById("menu")?.classList.remove("on"); const c = ctxRow || {}; return openMerge(mu.dataset.elUnisci, c.x || e.clientX, c.y || e.clientY); }
    const ms = t.closest?.("[data-el-separa]");
    if (ms) { e.preventDefault(); e.stopPropagation(); document.getElementById("menu")?.classList.remove("on"); invoke("separa_chat", { figlia: ms.dataset.elSepara }).then(() => { toast("Tolta da sotto", 1800); caricaElenco(); }); return; }
    const ua = t.closest?.("[data-el-unisci-a]");
    if (ua) { e.preventDefault(); e.stopPropagation(); const id = ua.closest(".el-unisci").dataset.id; closeMerge(); return mergeInto(id, ua.dataset.elUnisciA); }
    if (t.closest?.(".el-unisci")) { e.stopPropagation(); return; }
    if (document.querySelector(".el-unisci")) closeMerge();
    const su = t.closest?.("[data-el-sim-unisci]");
    if (su) { e.preventDefault(); e.stopPropagation(); return mergeInto(su.dataset.elSimUnisci, su.dataset.elSimA); }
    const si = t.closest?.("[data-el-sim-ignora]");
    if (si) { e.preventDefault(); e.stopPropagation(); EL.ignorati.add(si.dataset.elSimIgnora); saveSet("el-simili-ignorati", EL.ignorati); EL.avvisi = EL.avvisi.filter((a) => a.n !== si.dataset.elSimIgnora); return redraw(); }
    const mp = t.closest?.("[data-el-prog]");
    if (mp) { e.preventDefault(); e.stopPropagation(); document.getElementById("menu")?.classList.remove("on"); const c = ctxProj || {}; return openProj(mp.dataset.elProg, c.x || e.clientX, c.y || e.clientY); }
    if (t.closest?.(".el-prog")) { e.preventDefault(); e.stopPropagation(); projAction(e); return; }
    if (document.querySelector(".el-prog")) closeProj();
    const rg = t.closest?.("[data-el-regia]");
    if (rg) {
      e.preventDefault(); e.stopPropagation();
      const pid = rg.dataset.elRegia;
      if (EL.scrivi.has(pid)) EL.scrivi.delete(pid); else EL.scrivi.add(pid);
      if (gruppiScelti.get(pid) === false) { gruppiScelti.set(pid, true); try { localStorage.setItem("gruppiscelti", JSON.stringify(Object.fromEntries(gruppiScelti))); } catch {} }
      redraw();
      if (EL.scrivi.has(pid)) setTimeout(() => document.querySelector(`[data-gr-scrivi="${CSS.escape(pid)}"] input`)?.focus(), 40);
      return;
    }
    const f = t.closest?.("[data-el-filtro]"); if (f) { e.preventDefault(); e.stopPropagation(); return applyFilter(f.dataset.elFiltro); }
    if (t.closest?.("[data-el-tutte]")) { e.preventDefault(); e.stopPropagation(); EL.filtro = ""; return redraw(); }
    if (t.closest?.("[data-el-cambi]")) { e.preventDefault(); e.stopPropagation(); return typeof sbloccaElenco === "function" && sbloccaElenco(); }
    const fm = t.closest?.("[data-el-ferme]");
    if (fm) { e.preventDefault(); e.stopPropagation(); const k = fm.dataset.elFerme; if (EL.fermeAperte.has(k)) EL.fermeAperte.delete(k); else EL.fermeAperte.add(k); return redraw(); }
    const ap = t.closest?.("[data-el-apri]"); if (ap) { e.preventDefault(); e.stopPropagation(); elencoAttivo = true; return scegli(ap.dataset.elApri); }
    const sl = t.closest?.("[data-el-letta]");
    if (sl) { e.preventDefault(); e.stopPropagation(); const id = sl.dataset.elLetta; const r = sl.closest(".riga"); if (r) r.style.opacity = ".4"; invoke("segna_letta", { id }).then(() => caricaElenco()).catch(() => toast("Non riuscito")); return; }
    const tl = t.closest?.("[data-el-tutteletta]");
    if (tl) {
      e.preventDefault(); e.stopPropagation();
      const ids = tl.dataset.elTutteletta.split(",").filter(Boolean);
      (async () => { for (const id of ids) await invoke("segna_letta", { id }).catch(() => {}); toast(ids.length === 1 ? "1 chat segnata come letta" : `${ids.length} chat segnate come lette`, 2000); caricaElenco(); })();
      return;
    }
    const nn = t.closest?.("[data-el-nuova-in]");
    // the New chat window opens already on the project (new-chat.js: apriNuova({ progetto }))
    if (nn) { e.preventDefault(); e.stopPropagation(); if (typeof window.apriNuova === "function") window.apriNuova({ progetto: nn.dataset.elNuovaIn }); return; }
    const lb = t.closest?.("[data-el-libera]"); if (lb) { e.preventDefault(); e.stopPropagation(); return putToRest(lb.dataset.elLibera.split(",").filter(Boolean)); }
    if (t.closest?.("[data-el-annulla]")) { e.preventDefault(); e.stopPropagation(); return undoRest(); }
  }, true);
  // the coordinator box: closes by itself after sending
  addEventListener("submit", (e) => { const f = e.target.closest?.("[data-gr-scrivi]"); if (f) setTimeout(() => { if (!f.querySelector("input")?.value) { EL.scrivi.delete(f.dataset.grScrivi); redraw(); } }, 1500); });
  addEventListener("keydown", (e) => {
    const inText = /^(TEXTAREA|INPUT|SELECT)$/.test(e.target.tagName) || e.target.isContentEditable;
    if (e.target.matches?.("[data-el-regia]") && (e.key === "Enter" || e.key === " ")) { e.preventDefault(); e.target.click(); return; }
    if (e.key === "Escape" && e.target.matches?.(".el-u-cerca")) { closeMerge(); return; }
    if (inText || e.metaKey || e.ctrlKey) return;
    if (e.altKey && /^Digit[1-4]$/.test(e.code)) { e.preventDefault(); return applyFilter(["asp", "blo", "lav", "let"][+e.code.slice(5) - 1]); }
    if (e.key === "Escape" && document.querySelector(".el-unisci")) { closeMerge(); return; }
    if (e.key === "Escape" && document.querySelector(".el-prog")) { closeProj(); return; }
    if (e.key === "Escape" && EL.filtro && !document.querySelector(".foglio.on,#menu.on,.quota-pop")) { EL.filtro = ""; return redraw(); }
    if ((e.key === "ArrowLeft" || e.key === "ArrowRight") && !e.altKey && !e.shiftKey && elencoAttivo && scelta && EL.gruppoDi.has(scelta) && !saAperto) {
      const pid = EL.gruppoDi.get(scelta);
      e.preventDefault();
      gruppiScelti.set(pid, e.key === "ArrowRight");
      try { localStorage.setItem("gruppiscelti", JSON.stringify(Object.fromEntries(gruppiScelti))); } catch {}
      redraw();
    }
  });
  // row times ("da 9 min", "12 min", "55 min fa"): every 20 s is enough
  setInterval(() => document.querySelectorAll("#elenco .el-t[data-da]").forEach((t) => { const x = timeText(+t.dataset.da, t.dataset.m); if (t.textContent !== x) t.textContent = x; }), 20000);
})();
