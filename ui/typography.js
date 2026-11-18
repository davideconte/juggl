// typography (Nov 16): text size of the replies. Three sizes (small, normal, large) with ⌘+ and ⌘−;
// the chat column grows with the text (--col in index.html), so lines stay around 74 characters.
// Saved in localStorage "testo". window.tipografia.imposta("grande") is also used by Settings.
// Hooks in index.html (marked "tipografia (16/11)"): variables --t and --col, the <script src="typography.js"> tag.
(() => {
  const SIZES = { piccolo: 14, normale: 15, grande: 16.5 };
  const ORDER = ["piccolo", "normale", "grande"];
  const NAMES = { piccolo: "Testo piccolo", normale: "Testo normale", grande: "Testo grande" };
  const notice = (t, ms) => (typeof toast === "function" ? toast(t, ms) : console.log(t));
  let current = "normale";
  try { const v = localStorage.getItem("testo"); if (SIZES[v]) current = v; } catch {}

  function apply() { document.documentElement.style.setProperty("--t", SIZES[current] + "px"); }
  function imposta(v, quiet) {
    if (!SIZES[v]) return false;
    current = v; apply();
    try { localStorage.setItem("testo", v); } catch {}
    if (!quiet) notice(NAMES[v], 1400);
    return true;
  }
  function step(d) {
    const i = Math.max(0, Math.min(ORDER.length - 1, ORDER.indexOf(current) + d));
    if (ORDER[i] === current) return notice(d > 0 ? "Il testo è già al massimo" : "Il testo è già al minimo", 1400);
    imposta(ORDER[i]);
  }
  apply();

  window.addEventListener("keydown", (e) => {
    if (!(e.metaKey || e.ctrlKey) || e.altKey) return;
    if (document.querySelector("#visore.on")) return;   // the photo viewer has its own zoom
    const bigger = e.key === "+" || e.key === "=" || e.code === "Equal" || e.code === "NumpadAdd";
    const smaller = e.key === "-" || e.key === "_" || e.code === "Minus" || e.code === "NumpadSubtract";
    if (!bigger && !smaller) return;
    e.preventDefault(); e.stopPropagation();
    step(bigger ? 1 : -1);
  }, true);

  // width (Nov 16): message column Narrow / Normal / Wide (data-larghezza on <html>, sizes in index.html).
  // localStorage "larghezza" (only if picked by hand: whoever picked it keeps it); Wide by default.
  // window.tipografia.larghezza("normale"), or with no value = the next one.
  const WIDTHS = ["stretta", "normale", "larga"];
  let widthVal = "larga";
  try { const v = localStorage.getItem("larghezza"); if (WIDTHS.includes(v)) widthVal = v; } catch {}
  const applyWidth = () => { document.documentElement.dataset.larghezza = widthVal; };
  applyWidth();
  function larghezza(v, quiet) {
    if (!v) v = WIDTHS[(WIDTHS.indexOf(widthVal) + 1) % WIDTHS.length];
    if (!WIDTHS.includes(v)) return false;
    widthVal = v; applyWidth();
    try { localStorage.setItem("larghezza", v); } catch {}
    if (!quiet) notice({ stretta: "Colonna stretta", normale: "Colonna normale", larga: "Colonna larga" }[v], 1400);
    return v;
  }

  window.tipografia = { imposta, attuale: () => current, misure: () => ORDER.slice(), larghezza, attualeLarghezza: () => widthVal, larghezze: () => WIDTHS.slice() };
})();
