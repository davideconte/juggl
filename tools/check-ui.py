#!/usr/bin/env python3
"""UI check before every release (Nov 14, after the 0.1.56-0.1.59 regression: a patch removed
from index.html the tags of claude-login.js and commands.js plus two lines, and the "/" menu and the Claude Account panel disappeared).
1) every ui/*.js must have its <script src="..."> in index.html (or welcome.html);
2) the key functions must exist in the files, and the hook-up lines in index.html;
Exits with 1 and says what is missing: pubblica.sh stops."""
import os, re, sys
UI = os.path.join(os.path.dirname(os.path.abspath(__file__)), '..', 'ui')
idx = open(os.path.join(UI, 'index.html'), encoding='utf-8').read()
welcome = open(os.path.join(UI, 'welcome.html'), encoding='utf-8').read() if os.path.exists(os.path.join(UI, 'welcome.html')) else ''
errors = []
for f in sorted(os.listdir(UI)):
    if f.endswith('.js') and f'src="{f}"' not in idx and f'src="{f}"' not in welcome:
        errors.append(f'ui/{f} non e\' caricato da index.html (manca <script src="{f}">)')
def has(file, text, why):
    p = os.path.join(UI, file)
    if not os.path.exists(p) or text not in open(p, encoding='utf-8').read(): errors.append(f'{file}: manca {why}')
has('claude-login.js', 'window.accessoAvvia', 'window.accessoAvvia (cambio account)')
has('claude-login.js', 'window.accessoEsci', 'window.accessoEsci (esci dall\'account)')
has('claude-login.js', 'window.accessoComando', 'window.accessoComando (messaggi dei comandi /)')
has('commands.js', 'window.comandiApri', 'window.comandiApri (menu dei comandi /)')
has('index.html', 'esito === "gestito"', 'la riga in manda() che gestisce i comandi di Juggl')
has('index.html', 'aggiornato_adesso', 'l\'etichetta "aggiornato adesso" dei consumi')
for marker in ('window.__TAURI__', 'function caricaElenco'):
    has('index.html', marker, marker)
if errors:
    print('check-ui: PROBLEMI, pubblicazione fermata:'); [print('  -', e) for e in errors]; sys.exit(1)
print('check-ui: ok')
