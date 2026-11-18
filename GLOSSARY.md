# GLOSSARY: names that stay Italian on purpose

These names are part of the app's protocol with the iPhone app, the UI and the data already saved on users' disks, so they keep their original (Italian) names even though the code around them is now in English.

## Tauri commands (invoked by name from the UI)

The function name is the command name and its parameter names are the keys the UI passes: neither changes. Listed in the order of `tauri::generate_handler!` in main.rs; `module::` is the Rust module (English) that holds the command.

| Command | Meaning |
|---|---|
| tavolo | the desk: session list and dock items for the main window |
| snap::scatto_stato | screenshot shortcut: current state |
| snap::scatto_imposta | screenshot shortcut: set the key |
| snap::scatto_permesso_chiedi | ask macOS for the screen recording permission |
| snap::scatto_ora | take a screenshot now (from a button) |
| snap::scatto_apri_impostazioni | open System Settings on the screen recording page |
| summaries::riassunto_azioni | plain-language summary of a group of tool actions |
| media_recenti | recent media of all chats (media strip) |
| ponte_info | bridge info (port, key, network) for pairing the iPhone |
| ok_voci | open oks and "left open" items of all chats |
| ok_azione | act on an ok item (ok, no, reply, remove) |
| ok_manda | answer an ok line from the in-chat block |
| richieste_ai | user requests understood by the small model |
| richiesta_ai_azione | act on a request (seen, wrong, reopen, remind, ask, close) |
| richieste_ai_rivaluta | re-evaluate all open requests |
| elimina_chat | delete a chat and every trace of it |
| non_serve | "not needed": chat leaves "Waiting for you" |
| nota_spostato | send the chat an internal note that it was moved |
| regola | set model and effort of a Claude chat |
| regola_locale | set model, temperature, context of a local-model chat |
| consumi | Claude usage: limits and per-chat context |
| consumi_analisi | weekly quota analysis and forecast |
| notifiche | notification log |
| notifiche_lette | mark notifications as read |
| cerca_ovunque | quick search across chats |
| cerca_tutto | full-text search in all conversations |
| cerca_in_sessione | search inside one conversation |
| indice_stato | search index status |
| sessione_chiusa | read a closed conversation |
| riprendi_chiusa | resume a closed conversation in the app |
| archivia | archive a chat |
| riprendi_archiviata | resume an archived chat |
| togli_archivio | take a chat out of the archive |
| imposta_archivio_auto | days after which hibernated chats auto-archive |
| stato_mac | Mac status (memory, disk, heavy chats) |
| pulisci_mac | free memory on the Mac |
| esci_app | ask another app to quit |
| mac_scansione | disk scan for the Mac view |
| mac_ultima | last disk scan result |
| mac_libera | free one disk item |
| mac_libera_auto | free disk space up to a target |
| mac_ram | memory details |
| mac_storico | free space history |
| mac_ripristina | restore an item put in the Trash |
| modelli_locali | models available on the local engine |
| cancella_chat_locali | delete all local-model chats |
| schermo_agente | current screen of the PC agent |
| imposta_pausa_auto | automatic pause when the quota is high |
| eco_imposta | Eco mode on/off and automatic switch |
| nuova_locale | new chat with a local model |
| rispondi_scelta | answer a terminal/permission question by option or text |
| pilota_annulla | cancel the autopilot's pending reply |
| pilota_imposta | autopilot on/off for a chat |
| pilota_ora | send the autopilot's reply now |
| guardando | chat the user is looking at |
| guardando_tutte | chats open side by side |
| invia_ora_dalla_coda | send a queued message now |
| sessione | full chat data for the Mac app |
| modifiche_pezzi | diff chunks of an answer's live edits |
| segna_avvio | log how long the first screen took to show |
| sottoagente | sub-agent transcript |
| ferma_sottoagente | stop a working sub-agent |
| rinomina | rename a chat |
| fissa_sessione | pin a chat to the top |
| apri | bring a chat to front (its terminal tab or the app) |
| invia | send a user message to a chat |
| togli_dalla_coda | remove a queued message |
| interrompi | stop the current turn |
| apri_link | open a link in the browser |
| apri_file | open a file (or show it in Finder) |
| progetti | projects list |
| iberna | hibernate a chat |
| chiudi | close a chat |
| riprendi | resume a hibernated chat |
| nuova | new chat in a folder |
| copia_file | copy a file to the clipboard |
| copia_dati | copy a data URL (image) to the clipboard |
| info_file | file info (size, type) |
| copia_testo | copy text to the clipboard |
| esc_cattura | capture Esc while the viewer is open |
| apri_visore | open the separate media viewer |
| visore_dati | media for the viewer |
| visore_tutti | recent media of all chats for the viewer |
| vai_al_media | from the viewer, go to the media's chat |
| apri_in_mostra | send an image to the shared "mostra" viewer |
| chiudi_visore | close the viewer |
| tieni_allegati | copy attachments the system may delete |
| ultima_notifica | last notification tied to a chat |
| esiste | does a path exist |
| leggi_documento | document content for the viewer |
| anteprima_documento | Quick Look preview of office documents |
| tipo_documento | document type for a clicked path |
| testo_documento | full text of a document ("Copy all") |
| salva_allegato | save an attachment sent as data |
| imposta_logo | set the logo of a chat's project from image data |
| profile::profilo | user profile and features |
| profile::profilo_imposta | change one profile setting |
| profile::controlli | welcome checks (Claude Code, Python) |
| profile::hook_installa | install Juggl's Claude Code hooks |
| profile::hook_togli | remove Juggl's hooks |
| profile::installa_strumenti_apple | install Apple command line tools |
| benvenuto_apri | open a welcome step |
| benvenuto_fine | end of the welcome |
| importa_chiudi | close the standalone import window |
| import::importa_cerca | import: find chats and projects |
| import::importa_ricava | import: derive projects from folders |
| import::importa_aggiungi_radice | import: add a folder to search |
| import::importa_crea | import: create the chosen sessions |
| prova_notifica | test notification |
| updates::aggiornamento_cerca | check for an update |
| updates::aggiornamento_stato | update status |
| updates::aggiornamento_installa | install the downloaded update |
| license::licenza_stato | license status |
| license::licenza_rinnova | renew the license |
| license::account_codice | ask for the sign-in code by email |
| license::account_entra | sign in with email and code |
| license::account_esci | sign out of the Juggl account |
| microphone::mic_avvia | start dictation |
| microphone::mic_riprova | retry a failed transcription |
| microphone::mic_ferma | stop and transcribe |
| microphone::mic_annulla | discard the recording |
| microphone::mic_stato | dictation status |
| microphone::mic_scarica_modello | download the speech model |
| microphone::mic_impostazioni | open microphone settings |
| microphone::mic_imposta_qualita | set dictation quality |
| coordinator::regia_imposta | group coordinator on/off |
| permissions::permessi_stato | permission levels and exceptions |
| permissions::permessi_imposta | set the general permission level |
| permissions::permessi_eccezione | permission exception for a project or folder |
| permissions::permessi_chat | permission exception for a chat |
| permissions::permessi_rispondi | answer a permission request |
| permissions::permessi_avviso_visto | risk notice seen |
| permissions::permessi_pilota | allow the autopilot to grant permissions |
| permissions::permessi_pilota_annulla | cancel the autopilot's automatic consent |
| open_as_chat::sottoagente_in_chat | turn a sub-agent into its own chat |
| engines::motori_stato | Codex status |
| engines::motori_entra_codex | sign in to Codex with ChatGPT |
| live_edits::modifiche_annulla | undo an answer's file edits |
| pc_local_engine::locale_stato | local engine on the PC: status |
| pc_local_engine::locale_ferma | local engine on the PC: stop the task |
| fallback::continua_su_codex | continue a chat on Codex when Claude's limit is reached |
| claude_login::accesso_avvia | Claude login: start |
| claude_login::accesso_codice | Claude login: send the code |
| claude_login::accesso_annulla | Claude login: cancel |
| claude_login::accesso_esci | Claude logout |
| claude_login::accesso_stato | Claude login status |
| claude_login::accesso_dettagli | Claude account details |
| commands::comandi_elenco | "/" menu entries for a chat |
| commands::comando_esegui | run a "/" command |
| commands::modelli_claude | Claude models of the account |
| logos::progetto_colore | set a project color |
| logos::progetto_logo_sito | project icon from a website |
| logos::progetto_logo_iniziali | use initials instead of a logo |
| logos::loghi_online_imposta | online logo search on/off |
| logos::progetto_cerca_online | include/exclude a project from the online logo search |
| requests::richieste_pulizia_imposta | automatic request cleanup on/off |
| segna_letta | mark a chat as read |
| unisci_chat | merge a sub-chat into a main chat |
| separa_chat | split a sub-chat out again |
| mac_spazio | "Space" view actions |
| aiutanti_stato | helpers (small-model calls): settings and usage |
| aiutanti_imposta | change helper settings |
| consumi_dettaglio | full usage breakdown |
| compattazione_stato | idle compaction: status |
| compattazione_imposta | idle compaction: settings |
| compattazione_chat | idle compaction switch for a chat |
| compattazione_ora | compact a chat now |
| requests::richieste_pulizia_estesa_imposta | extended request cleanup on/off |
| requests::richieste_annulla_pulizia | undo the last request cleanup |
| requests::richieste_pulizia_stato | request cleanup status |
| requests::richieste_campione_pulizia | dry run of the request cleanup |
| settings_chat::regolazioni_stato | settings chat: status |
| settings_chat::regolazioni_registra | settings chat: log a change |
| settings_chat::regolazioni_capisci | settings chat: interpret a phrase |
| support::aiuto_storia | help chat history |
| support::aiuto_chiedi | ask the help assistant |
| support::aiuto_anteprima | prepare a report preview |
| support::aiuto_invia | send a report |
| support::aiuto_stati | status of sent reports |
| support::aiuto_svuota | clear the help chat |
| support::aiuto_salva_immagine | save a screenshot for the help chat |

## Bridge API paths (local HTTP server used by the iPhone app and scripts)

Served by bridge.rs (port 47800 by default). Path, method and body/query keys stay as they are.

| Path | Method | Meaning |
|---|---|---|
| /api/tavolo | GET | the desk: session list for the iPhone |
| /api/eventi | GET | server-sent events stream (changes, partial text, notices) |
| /api/sessione | GET | one chat's data |
| /api/invia | POST | send a message to a chat |
| /api/interrompi | POST | stop the current turn |
| /api/nuova | POST | new chat in a folder |
| /api/chiudi | POST | close a chat |
| /api/iberna | POST | hibernate a chat |
| /api/riprendi | POST | resume a chat |
| /api/archivia | POST | archive a chat |
| /api/riprendi-archiviata | POST | resume an archived chat |
| /api/togli-archivio | POST | take a chat out of the archive |
| /api/elimina | POST | delete a chat |
| /api/letta | POST | mark a chat as read |
| /api/non-serve | POST | "not needed" |
| /api/nota-spostato | POST | send the chat the "moved" note |
| /api/chiusa | GET | read a closed conversation |
| /api/progetti | GET | projects |
| /api/coda/ora | POST | send a queued message now |
| /api/coda/togli | POST | remove a queued message |
| /api/scelta | POST | answer a question by option or text |
| /api/permessi | GET | pending permission requests |
| /api/permesso | POST | answer a permission request |
| /api/pilota/imposta, /api/pilota/annulla, /api/pilota/ora | POST | autopilot on/off, cancel, send now |
| /api/regola | POST | model and effort of a chat |
| /api/regola-locale | POST | settings of a local-model chat |
| /api/sottoagente | GET | sub-agent transcript |
| /api/sottoagente/ferma | POST | stop a sub-agent |
| /api/sottoagente/apri | POST | open a sub-agent as a chat |
| /api/modifiche/annulla | POST | undo an answer's file edits |
| /api/comandi | GET | "/" menu entries |
| /api/comando | POST | run a "/" command |
| /api/modelli | GET | Claude models |
| /api/accesso/stato, /api/accesso/avvia, /api/accesso/codice, /api/accesso/annulla, /api/accesso/esci | GET/POST | Claude login flow |
| /api/live/token, /api/live/avvio | POST | Live Activity tokens |
| /api/iphone | POST | iPhone app state (foreground/background) |
| /api/iphone/token | POST | APNs device token |
| /api/android/token | POST | Android push token |
| /api/iphone/push-prova | POST | test push |
| /api/messaggio | POST | message to the user from scripts and sessions |
| /api/messaggio/stato | GET | is the user at the Mac or on the phone |
| /api/notifiche | GET | notification log |
| /api/notifiche/lette | POST | mark notifications read |
| /api/ok | GET | open oks |
| /api/ok/azione, /api/ok/manda | POST | act on / answer an ok |
| /api/richieste-ai | GET | requests understood by the small model |
| /api/richieste-ai/azione, /api/richieste-ai/lette, /api/richieste-ai/rivaluta | POST | act on, mark read, re-evaluate requests |
| /api/riassunto-azioni | POST | plain-language summary of tool actions |
| /api/voce-breve | POST | short spoken summary of a chat |
| /api/cerca | GET | quick search |
| /api/cerca-tutto | GET | full-text search |
| /api/cerca-sessione | GET | search in one chat |
| /api/consumi, /api/consumi/analisi, /api/consumi/dettaglio | GET | usage, weekly analysis, breakdown |
| /api/aiutanti | GET/POST | helpers settings and usage |
| /api/aiuto, /api/aiuto/chiedi, /api/aiuto/anteprima, /api/aiuto/invia, /api/aiuto/stati, /api/aiuto/svuota, /api/aiuto/immagine | GET/POST | help chat and reports |
| /api/mac | GET | Mac status |
| /api/mac/pulisci, /api/mac/esci, /api/mac/libera, /api/mac/libera-auto, /api/mac/ripristina | POST | free memory, quit an app, free disk, restore |
| /api/mac/scansione, /api/mac/ultima, /api/mac/ram, /api/mac/storico | GET | disk scan, last scan, memory, history |
| /api/mac/spazio | GET/POST | "Space" view |
| /api/locale/modelli | GET | local models |
| /api/locale/nuova | POST | new local-model chat |
| /api/agente/schermo | GET | PC agent screen |
| /api/hub-chiave | GET | key for the team hub (personal) |
| /api/media-recenti | GET | recent media |
| /api/visore, /api/visore/chiudi | POST | open/close the viewer from the phone |
| /api/allegato | POST | upload an attachment |
| /api/vocale | POST | upload a voice note |
| /api/vocale/esito | GET | voice note transcription outcome |
| /api/file | GET | serve a file shown in a chat |
| /api/documento, /api/documento/file | GET | document content / raw document |
| /api/migra | POST | migrate terminal sessions to the keeper |
| /api/esci-da-tutto | POST | quit everything |
| /api/licenza | GET/POST | license (allowed even without a valid license) |

Remote paths called by the app (not served by it): `/api/account/codice`, `/api/account/entra`, `/api/licenza`, `/api/tavolo`, `/api/supporto/segnalazione`, `/api/supporto/mie`, `/api/supporto/admin/nuove`, `/api/supporto/admin/schermata` on the juggl.dev account service; `/api/session/...` on the PC agent.

## Event names (Rust -> UI and bridge stream)

`cambiato` (something changed: reload), `parziale` (partial answer text), `transcript`, `messaggio`, `allarme`, `consumo-alto`, `account-cambiato`, `accesso-finito`, `comando-non-disponibile`, `aggiornamento`, `licenza`, `naviga`, `esc`, `main`, `media-nuovo`, `vai-media`, `visore-dati`, `visore-nascosto`, `mic-fase`, `mic-livello`, `mic-modello`, `mic-pieno`, `scatto`, `scatto-permesso`, `importa_avanzamento`.

## JSON keys (saved files and bridge payloads)

The most used keys; all others also stay as they are.

| Key | Meaning |
|---|---|
| ok, msg | outcome of a call and its message |
| id, sessione | chat (session) id |
| stato | state (of a chat: "lavora", "aspetta", "ibernata"...; of a request) |
| vista, vista_turno | how the chat appears in the list (working, waiting, to read) |
| nome, nome_manuale | chat name, name set by hand |
| testo, titolo | text, title |
| tipo | kind |
| t, ts, da, a | times: timestamp, from, to |
| voci, elenco | items, list |
| progetto, progetti | project, projects |
| cartella, cwd, cwd_origine | folder, working folder, starting folder |
| percorso, path, file | file path |
| modello, effort, motore | model, reasoning level, engine ("claude" or "codex") |
| motivo, esito, errore | reason, outcome, error |
| creata, aggiornata | created, updated |
| ospite, pid | hosted in the app, process id |
| pilota | autopilot state |
| fai_tu, serve_te_cosa | "your turn" flags and what the user needs to do |
| aspettano | chats waiting for the user |
| anche_in | request also present in other chats |
| permessi, permessi_attesa, permesso | permission settings, pending requests, a request |
| righe, pezzi, piu, meno | diff lines, chunks, added, removed |
| eventi, messaggi | chat events, messages |
| chiesto, domanda, risposta, azione | what was asked, question, answer, action |
| regia, gruppo | coordinator flag, group |
| agg | update marker in Live Activity state |
| attiva, accesa, acceso | on/off switches |
| logo, colore | project logo and color |
| impostazioni keys (pausa_auto, regia_gruppi, eco, permessi_progetti, permessi_cartelle, ...) | user settings |

## Data files and folders on disk

Data folder: `~/.juggl` for new users, `~/tavolo` for existing ones (`JUGGL_BASE` overrides). Inside it:

| Name | Meaning |
|---|---|
| stato/ | one JSON state file per chat (`<id>.json`) |
| stato/richieste/ | registry of oks and open items |
| impostazioni.json | user settings |
| progetti.json | projects |
| ponte.json | bridge port and key |
| juggl-id | this installation's id |
| notifiche.json, notifiche-lette.json | notification log, read marks |
| richieste-ai.json | requests understood by the small model |
| ok.json | oks registry |
| coda/ | queued messages per chat |
| allegati/ | attachments |
| contesti/, contesto | prepared contexts of ready sessions |
| chat-locali/ | local-model chats |
| chat-veloci/ | fast-open cache of chats |
| indice.db | full-text search index (SQLite FTS5) |
| loghi/ | project icons |
| motori/ | Codex mirror transcripts |
| modifiche/ | live edits (snapshots, shadow folders, switch file `spento`) |
| regia/ | coordinator registry and notices |
| sock/ | keeper sockets |
| custode/ | installed keeper program (`juggl-custode`) |
| bin/ | installed scripts: `juggl`, `juggl-hook` (or `tavolo`), `comune.py`, `cartelle.py`, `registro`, `mac`, `wa` |
| generale-juggl/ | instructions for the Juggl assistant chat |
| supporto/ | help chat and reports |
| spazio/ | "Space" view data |
| comandi/ | "/" menu catalogs |
| account.json, .token-dispositivo | Juggl account cache and device token |
| aiutanti.json, consumi-aiutanti.jsonl | helpers settings and log |
| consumi-storico.json, limiti-soglie.json, limiti-avvisi.json | usage history and limit notices |
| compattazione.jsonl | compaction log |
| regolazioni-storia.jsonl | settings chat change log |
| riassunti-azioni.json | cached action summaries |
| pagine.json | shared pages registry |
| presenza.json | user presence (Mac or phone) |
| live-token.json, live-avvio.json, iphone-push.json, android-push.json | push and Live Activity tokens |
| mac-scansione.json | last disk scan |
| migrazioni.jsonl | keeper migrations |
| istanza.lock | single-instance lock |
| juggl.log, ospite.log | app and hosted-session logs |

Outside the data folder: `~/.config/juggl/personale.json` (personal settings), `~/.config/juggl/apns/`, `~/.config/juggl/agente-pc.txt`, `~/.config/juggl/locale-pc.txt`; caches in `~/Library/Caches/juggl-documenti` and `~/Library/Caches/juggl-miniature`.

## Binaries, features, markers

- Binary names: `tavolo` (the app, src/main.rs) and `juggl-custode` (the session keeper, src/programs/juggl-keeper.rs).
- Cargo features: `personale` (personal features: messaging, PC agent, PC local engine; off in the public tree) and `aggiornamenti` (automatic updates from juggl.dev).
- Markers: comment lines with the word `personale` followed by `inizio` (start) or `fine` (end), in Rust/JS, HTML and CSS comment syntax, delimit personal code that the public-tree script removes.
- Environment variables keep their names (`JUGGL_BASE`, `JUGGL_PORTA`, `TAVOLO_INTERNO`, `JUGGL_SENZA_LICENZA`, ...).
