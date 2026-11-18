# Guida di Juggl
Versione guida: 0.1.99

Juggl riunisce in una finestra le chat con gli agenti di programmazione Claude Code e Codex: mostra chi ti aspetta, chi lavora e cosa è finito, e ti fa rispondere dal Mac o dall'iPhone. Windows, Linux e iPhone sono in beta, Android in sviluppo. Su Windows e Linux Ctrl fa le veci di ⌘.

## Windows e Linux
- Juggl funziona anche su Windows e Linux (in beta). Questa guida dice "Mac": su Windows l'app scrive "PC", su Linux "computer" (per esempio la vista "PC" o "Computer" al posto di "Mac", "Al PC" al posto di "Al Mac"). Le funzioni sono le stesse salvo dove è scritto "solo Mac".
- Tasti: Ctrl al posto di ⌘, Alt al posto di ⌥, Maiusc al posto di ⇧, Invio al posto di ↩. Le scorciatoie scritte nell'app sono già convertite.
- Solo Mac: lo Scatto (F16). L'iPhone per ora si collega solo al Mac.

## Primo avvio
- Si entra con l'email dell'account Juggl, attivato dopo la richiesta di accesso alla beta.
- Il benvenuto chiede il tuo nome, collega Juggl a Claude Code, spiega avvisi e microfono e importa i progetti.
- Il collegamento è un piccolo hook nelle impostazioni di Claude Code. Juggl fa prima una copia del file, i tuoi hook restano, e lo togli quando vuoi da Impostazioni, Disinstalla collegamento.
- Per rifare l'importazione: Impostazioni, Aiuto, Importa progetti.

## La barra laterale
- In cima quattro numeri: ti aspettano, bloccate, lavorano, da leggere. Un clic (o ⌥1...⌥4) mostra solo quelle chat; Esc toglie il filtro.
- "Ti aspettano": chat che hanno finito o ti chiedono una scelta o un permesso, dalla più vecchia, con una riga su cosa devi fare.
- "Bloccate": chat che si sono inceppate, per esempio per il limite d'uso o perché non danno segni da 15 minuti.
- Poi le chat fissate e i progetti. Le chat ferme sono raccolte in una riga ("3 ferme"): un clic le mostra.
- Segni di stato: puntino arancio = ti aspetta, anello che gira = lavora, punto esclamativo = bloccata, cerchio vuoto = da leggere, icona sbiadita = ferma.
- ⌘D fissa la chat scelta. Le fissate stanno in cima e la modalità Eco non le tocca.
- Tasto destro su una riga: rinomina, fissa, archivia, apri la cartella, elimina, icona e colore del progetto, "Unisci a..." (diventa sotto-chat di un'altra; "Togli da sotto" la separa).
- In fondo c'è lo stato del Mac. Con poca memoria, "Libera N GB" mette a riposo le chat ferme, con Annulla per 20 secondi.
- Un sotto-agente diventa una chat sua con "Apri come chat".

## Le chat
- Nuova chat: ⌘N. Il progetto è facoltativo: senza cartella è una "chat veloce". Se Codex è installato, nel Modello compare "ChatGPT · Codex".
- Invio manda, Maiusc+Invio va a capo. Puoi scrivere anche mentre la chat lavora: lo legge al passo dopo. La freccia su modifica l'ultimo messaggio in coda.
- ⇧⌘M apre modello e ragionamento (anche /modello e /ragionamento). ⌘. ferma la risposta. ⌘L porta nella casella.
- Il riquadro "Adesso" mostra cosa sta facendo la chat. ⌘I apre i dettagli tecnici.
- Sotto una risposta che ha cambiato file compare "3 file +42 -7": un clic apre le differenze, con Copia diff e Annulla (con conferma).
- Cercare: ⌘F in questa chat (⌘G e ⇧⌘G per il risultato dopo o prima), ⇧⌘F nel progetto, ⌘K in tutti i messaggi o per saltare a una sessione.
- Cambiare chat: ⌘1...9, frecce su e giù, o ⌥⌘ con le frecce anche mentre scrivi. Rinomina: doppio clic sul nome.
- ⇧⌘A archivia (e riprende un'archiviata). ⌘⌫ elimina, con conferma, senza recupero.
- ⌘E iberna la chat e libera memoria; di nuovo ⌘E la riprende intera. Le chat ferme da ore vanno in pausa da sole.
- ⇧⌘S sposta in un'altra chat quello che hai scritto. ⇧⌘U toglie una chat da "Ti aspettano". ⇧⌘C copia l'ultima risposta.
- ⌘+ e ⌘- cambiano la grandezza del testo; /larghezza la larghezza della colonna.

## Colonne affiancate
- ⌘\ attiva le colonne: fino a quattro chat affiancate. ⇧⌘\ aggiunge una colonna, ⌘ clic su una chat la apre in una colonna nuova.
- ⌥⌘ con frecce destra e sinistra cambia colonna; ⌃1...⌃4 va alla colonna.

## Scorciatoie
⌘/ (o /scorciatoie) apre la lista completa. Le più usate: ⌘N nuova chat, ⌘K cerca, ⌘J chiedi a Juggl, ⌘R richieste, ⌘0 In corso, ⌘\ colonne, ⌘D fissa, ⌘E iberna, ⇧⌘M modello, ⇧⌘P pilota automatico, ⇧⌘N nastro dei media. Esc chiude anteprime e finestre.

## Menu dei comandi "/"
Scrivi "/" nella casella: compare l'elenco. Scrivendo filtri, le frecce scelgono, Invio usa, Tab completa, Esc chiude.
- Comandi di Juggl: /nuova, /modello, /ragionamento, /consumi, /account, /esci, /stato, /rinomina, /sposta, /riprendi, /eco, /ferma, /archivia, /cerca, /richieste, /permessi, /memoria, /larghezza, /impostazioni, /scorciatoie, /aiuto.
- Nomi alternativi: /clear e /new per /nuova, /login per /account, /resume per /riprendi.
- Compaiono anche le tue skill e i comandi di progetto e plugin. Un comando sconosciuto non parte: il testo resta nella casella.
- /memoria apre il file di memoria di Claude (CLAUDE.md). /riprendi ritrova una chat di prima.

## Chiedi a Juggl
- ⌘J porta nel campo "Chiedi a Juggl"; una seconda volta entra nella chat di Juggl, quella che coordina le altre: chiedi chi ha bisogno di te o cosa mandare a quale sessione.
- Nei gruppi di chat c'è una "regia": una riga riassume chi ti aspetta e "Scrivi alla regia" passa il testo alla chat giusta. Si accende o spegne dal tasto destro sul gruppo.

## Richieste (⌘R)
- Ogni cosa che chiedi in una chat diventa una richiesta con il suo stato: tre cose in un messaggio sono tre richieste. Se una chat ne salta una, Juggl la ricorda.
- Raggruppate per progetto: in cima ciò che aspetta te, sotto "In lavoro" e "Fatte". Un ok si dà con Ok, No e Rispondi senza aprire la chat.
- Le doppie sono unite ("chiesta N volte"). Niente si cancella: quelle chiuse o unite da Juggl sono in "Chiuse o unite da Juggl", con Riapri e Separa.
- In Impostazioni, Richieste c'è la pulizia automatica, con annullamento dell'ultima pulizia.

## In corso (⌘0)
- "Ti aspettano": la domanda per esteso con Ok, No, Rispondi, Apri la chat.
- "Al lavoro adesso": cosa fa la chat, passi, da quanto lavora, sotto-agenti; col puntatore sopra compare Ferma.
- "Da leggere" con il tasto Letto, e "Fermi" per progetto.

## Schede nelle risposte
Gli agenti possono rispondere con schede: avanzamento, scelta con pulsanti, numeri, cose da fare per te. Toccando un'opzione la risposta torna alla chat. I tasti 1...9 scelgono l'opzione di una domanda aperta.

## Permessi e pilota automatico
- Gli account nuovi partono su "Chiedi sempre": prima di un comando o di una modifica ai file arriva una domanda con Consenti, Consenti sempre in questa chat o Nega, sul Mac e sull'iPhone.
- In Impostazioni, Permessi ci sono tre livelli: Chiedi sempre, Modifiche ai file sì e comandi su richiesta, Mai chiedere. "Mai chiedere" chiede una conferma e le chat partite così mostrano "Senza permessi". Si possono fare eccezioni per progetto o cartella, e dare un livello a una sola chat da Modello, "Permessi di questa chat".
- Pilota automatico (⇧⌘P per chat): quando una chat chiede solo se continuare un piano già approvato, risponde Juggl dopo un conto alla rovescia che puoi annullare. Consente i permessi solo con l'interruttore "Il pilota può consentire i permessi" (spento di base).

## Impostazioni a parole
- Le Impostazioni (⌘, o /impostazioni) si aprono su una conversazione: scrivi o detta cosa cambiare, per esempio "spegni gli avvisi di notte", "usa Sonnet per le chat nuove", "accendi Eco", "cerca aggiornamenti", "collega l'iPhone".
- Dopo ogni cambio c'è Annulla. Per le cose delicate (account, togliere il collegamento con Claude Code) chiede prima Conferma. I permessi non si cambiano a parole: si apre la loro sezione.
- Sotto ci sono le sezioni classiche. Avvisi di notte: silenzio su Mac e iPhone nell'orario scelto.

## Consumi ed Eco
- In basso nella barra c'è il limite settimanale di Claude con l'orario di azzeramento. Un clic, o /consumi, apre la vista Consumi.
- Consumi mostra quanto ha usato ogni chat negli ultimi 30 minuti, nell'ultima ora, oggi e in settimana, con una stima di quando finirebbe il limite. Se una chat consuma troppo arriva un avviso e puoi fermarla o metterla in pausa. Le cifre per chat sono stime. Puoi chiedere la pausa automatica sopra una soglia scelta da te.
- Modalità Eco (/eco o dalla vista Consumi): le chat non fissate passano a Sonnet con ragionamento medio, le nuove partono su Sonnet, risposte più brevi. Le fissate e Juggl restano come sono; spegnendo Eco tutto torna come prima. Juggl può accenderla da solo quando il limite supera il 50% e al ritmo attuale finirebbe prima dell'azzeramento.
- Se il dato del limite non arriva, compare "non disponibile".

## Compattare una chat (/compact)
- Una chat lunga rimanda tutta la conversazione a ogni messaggio: consuma di più ed è più lenta. /compact la riassume e libera spazio, e la chat continua da lì. Serve quando il lavoro continua ma la chat è diventata grande.
- Tasto Compatta: compare tra i consigli dei consumi (clic sul limite in basso nella barra) quando una chat ha il contesto oltre il 70%.
- Compattazione automatica delle chat ferme: Impostazioni, Compattazione, "Compatta le chat ferme", spenta di base. Una chat grande e ferma da un po' (45 minuti di base) si riassume da sola e quando ci torni riparte leggera. Mai se lavora, ti aspetta o ha lavori in background. Nella chat compare "Compattata mentre eri via" con il tasto "Spegni per questa chat".
- Con il contesto pieno è Claude Code a compattare da solo, e la chat lo segnala.

## Scatto in Juggl (solo Mac)
- F16, da qualunque app, fa come ⌘⇧4: trascini un'area, con Spazio scegli una finestra, Esc annulla.
- L'immagine va negli allegati della chat aperta (Alt+F16 nella chat di Juggl).
- Serve il permesso macOS Registrazione schermo; prima del primo scatto un foglio spiega a cosa serve. Juggl non cattura mai senza che tu prema il tasto.
- In Impostazioni, Scatto in Juggl cambi la combinazione, lo spegni o vedi lo stato del permesso. Le immagini degli scatti si cancellano dopo un giorno.

## Allegati e nastro dei media
- Allegare: tasto + vicino alla casella, trascinare file sulla chat o incollare un'immagine con ⌘V. Le immagini grandi vengono ridotte.
- Il nastro dei media (⇧⌘N) è la fascia con immagini e file della conversazione: "Progetto" segue la chat aperta, "Tutti" mostra tutte le chat. Un clic apre l'anteprima, ⌘ clic porta al punto della chat.

## Dettatura
- Il microfono nella casella (o ⌥Spazio) detta un messaggio: il testo compare mentre parli. ⌥Spazio o Invio per finire, Esc per annullare; tenendo premuto ⌥Spazio parli finché lo tieni.
- Il riconoscimento vocale si scarica una volta sola. L'audio si trascrive sul Mac e poi si cancella. macOS chiede il permesso del microfono la prima volta.
- Su Windows e Linux dipende dalla versione: se il tasto non c'è, su Windows 11 si può usare Win+H.

## Spazio, memoria e pulizia del Mac
- Si apre con un clic sulla riga in fondo alla barra laterale (quella che mostra lo stato del Mac). La vista "Mac" ha in cima la parte Spazio, poi memoria e app aperte.
- Spazio: Juggl guarda solo nomi e dimensioni dei file nella tua cartella, mai il contenuto. "Scansiona il disco" misura tutto; macOS può chiedere il permesso per Scrivania, Documenti e Download: se dici di no quelle cartelle vengono saltate.
- Propone solo cose che si rifanno da sole (build, cache, librerie dei progetti) e le sposta nel Cestino, sempre dopo averti chiesto. I file grandi li mostra soltanto: decidi tu. Lo spazio torna quando svuoti il Cestino (c'è il tasto "Svuota il Cestino", da confermare).
- Dopo ogni pulizia compare l'esito con "Annulla", che rimette a posto quello che è stato spostato.
- Una pulizia lunga va in sottofondo: vedi una barra con i GB fatti sul totale e il tempo stimato. Puoi chiudere la vista: Juggl ti avvisa quando finisce, e riaprendo la vista vedi l'avanzamento a che punto è.

## Account e aggiornamenti
- Impostazioni, Account mostra email, stato (attivo, in attesa, sospeso, rifiutato) e scadenza della licenza.
- /account (o /login) cambia account Claude: si apre la pagina di accesso, incolli il codice, Juggl conferma con l'email e le chat si riavviano sul nuovo account. /esci esce, /stato mostra account, modello e cartella della chat.
- Juggl si aggiorna da solo e le chat continuano a funzionare. In Impostazioni, Aggiornamenti c'è "Cerca aggiornamenti"; quando una versione è pronta compare "Riavvia ora".

## iPhone
- L'app per iPhone (beta su TestFlight, iOS 18 o successivo) mostra le stesse chat del Mac, con i pulsanti per permessi e scelte, dettatura e notifiche.
- Collegare: sul Mac Impostazioni, iPhone, accendi il collegamento e riavvia Juggl. Sull'iPhone tocca "Collega il Mac" e inquadra il QR. Se la camera non lo legge: "Copia codice" sul Mac e "Incolla" sull'iPhone. Il codice contiene la chiave del Mac: non mostrarlo ad altri.
- Sulla stessa Wi-Fi non serve altro. Fuori casa serve Tailscale (gratuito per uso personale) acceso su Mac e iPhone con lo stesso account.
- Il collegamento con Windows e Linux non è ancora pronto: l'iPhone si collega solo a un Mac.

## Privacy
Juggl gira sul tuo computer e legge i file delle sessioni che Claude Code e Codex scrivono già sul disco. Non c'è un server di Juggl in mezzo alle tue chat e Juggl non chiede mai password o chiavi.

## Aiuto e segnalazioni
- In cima alla barra laterale c'è la riga fissa "Aiuto e segnalazioni": apre una chat con un assistente che risponde alle domande su Juggl usando questa guida. Se la risposta non c'è, dice che non lo sa e propone di segnalarlo. L'assistente non vede le tue altre chat né i tuoi file.
- Per un problema o una funzione che manca propone una segnalazione. Prima di partire vedi un'anteprima di tutto: testo (modificabile), versione, sistema, diagnostica tecnica ripulita (si può togliere) e schermate. Parte solo con il tasto "Invia a Juggl".
- Non parte mai il contenuto delle tue chat, né file, chiavi o numeri.
- Schermate: con lo Scatto mentre la chat di aiuto è aperta, incollandole, trascinandole o con la graffetta. Al massimo 2, PNG o JPEG, fino a 2 MB l'una.
- Stati: ricevuta, presa in carico, risolta nella versione indicata, non riproducibile. I cambi arrivano come messaggio nella chat di aiuto e con una notifica.
- Al massimo 5 segnalazioni nelle ultime 24 ore.
- Conservazione: testo 90 giorni, schermate 30, poi cancellati.
- Il cestino della chat la svuota; le segnalazioni mandate restano.
- Puoi anche scrivere dalla pagina Supporto di juggl.dev, con risposta entro due giorni lavorativi.

## Problemi comuni
1. Lo Scatto non funziona: attiva per Juggl il permesso macOS "Registrazione schermo" (il foglio di Juggl ha "Apri Impostazioni di Sistema"), poi riapri Juggl. Controlla che lo Scatto non sia spento in Impostazioni.
2. Una chat non si carica: Juggl riprova da solo. Con "è una chat grande" aspetta; se compare un riquadro con il motivo, tocca Riprova.
3. Chat "Bloccata" o limite d'uso raggiunto: il limite si azzera all'orario scritto in basso nella barra. Intanto guarda Consumi, accendi Eco o cambia account con /account. Una chat senza segni da 15 minuti si ferma con ⌘.
4. La dettatura non parte o è lenta: controlla il permesso Microfono. La prima volta Juggl scarica il riconoscimento vocale.
5. L'iPhone non si collega: Juggl deve essere aperto sul Mac, che non deve essere in stop. Fuori casa serve Tailscale su entrambi. Se hai cambiato Mac, inquadra il nuovo QR.
6. Non vedo le mie sessioni: in Impostazioni, Collegamento con Claude Code deve dire "Collegato" (non "Collegato in parte"). Poi Importa progetti da Impostazioni, Aiuto.
7. Un comando "/..." non parte: se non esiste resta nella casella e Juggl propone i più simili. Alcuni comandi di Claude Code qui si chiamano diversamente: per /login usa /account.
8. Non riesco a mandare una segnalazione: hai già fatto 5 invii nelle ultime 24 ore, o una schermata supera 2 MB. Riprova domani o togli la schermata.
9. Account "in attesa" o "sospeso": Juggl è in beta privata e si entra a gruppi. Guarda lo stato in Impostazioni, Account e scrivi al supporto di juggl.dev.
