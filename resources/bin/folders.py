#!/usr/bin/env python3
# juggl-gestito: Juggl installs and updates this file. If you edit it by hand, remove this line.
"""The "who touches what" registry of Juggl's group coordinator (Nov 12).

  cartelle.py hook                 Claude Code PreToolUse hook (Edit, Write, MultiEdit, NotebookEdit)
  juggl cartelle <folder...>       the chat declares the folders it works on (in its state, stato/<id>.json in the Juggl folder)
  juggl cartelle --libera          removes the declaration
  juggl cartelle --elenco          who holds what in the group
  (--sessione <id> to act for another chat; by default the chat that runs the command)

Rules. Only chats in a group with the coordinator on count (regia/registro.json in the Juggl folder, written by
Juggl): all others pass right away. A chat "holds" a folder when it declared it and is working (state
lavora or bloccata, process alive). Whoever comes later with an edit in that folder is stopped, with a clear
message, and the coordinator is notified (regia/avvisi.jsonl, read by Juggl). Without declarations the same applies to the
single file: the first one that edits it holds it for 30 minutes while working.
The hook must be very fast: standard library only, no heavy imports, and if something goes wrong it lets the edit through.
"""
import json, os, sys, time

BASE = os.environ.get('JUGGL_BASE') or os.path.dirname(os.path.dirname(os.path.realpath(__file__)))   # rename (Nov 14): the script's folder
STATE_DIR = os.environ.get('TAVOLO_STATO_PROVA') or os.path.join(BASE, 'stato')
COORD_DIR = os.path.join(BASE, 'regia')
REGISTRY = os.path.join(COORD_DIR, 'registro.json')
NOTICES = os.path.join(COORD_DIR, 'avvisi.jsonl')
TOUCHES_DIR = os.path.join(COORD_DIR, 'tocchi')
BLOCK_FILE = os.path.join(COORD_DIR, 'blocco-acceso')   # as long as it does not exist: warning only, never a block
TOUCH_WINDOW = 30 * 60
TOOLS = ('Edit', 'Write', 'MultiEdit', 'NotebookEdit')
WORKING_STATES = ('lavora', 'bloccata')


def read_json(p, d=None):
    try:
        with open(p, encoding='utf-8') as f:
            return json.load(f)
    except Exception:
        return d


def alive(pid):
    try:
        pid = int(pid or 0)
        if pid <= 0:
            return False
        os.kill(pid, 0)
        return True
    except PermissionError:
        return True
    except Exception:
        return False


def inside(p, c):
    """p is inside folder c (or is c)."""
    c = c.rstrip('/') or '/'
    return p == c or p.startswith(c + '/')


def norm(p, cwd=''):
    p = os.path.expanduser(p)
    if not os.path.isabs(p):
        p = os.path.join(cwd or os.getcwd(), p)
    return os.path.realpath(p)


def state_of(sid):
    return read_json(os.path.join(STATE_DIR, sid + '.json'), {}) or {}


def is_working(st):
    return st.get('stato') in WORKING_STATES and alive(st.get('pid'))


def folder_since(st, p):
    """Since when the chat holds p (its oldest declared folder containing it), None if it does not hold it."""
    tt = st.get('cartelle_t') or {}
    t = [float(tt.get(c) or 0) for c in st.get('cartelle') or [] if inside(p, c)]
    return min(t) if t else None


def touches(sid, now):
    """Files edited by the chat in the last 30 minutes: path -> first time within the window."""
    out = {}
    try:
        with open(os.path.join(TOUCHES_DIR, sid + '.tsv'), 'rb') as f:
            f.seek(0, 2); n = f.tell(); f.seek(max(0, n - 65536))
            lines = f.read().decode('utf-8', 'ignore').splitlines()
    except OSError:
        return out
    for r in lines:
        t, _, p = r.partition('\t')
        try:
            t = float(t)
        except ValueError:
            continue
        if p and now - t <= TOUCH_WINDOW:
            out.setdefault(p, t)
    return out


def record_touch(sid, p, now):
    try:
        os.makedirs(TOUCHES_DIR, exist_ok=True)
        f = os.path.join(TOUCHES_DIR, sid + '.tsv')
        with open(f, 'a', encoding='utf-8') as fh:
            fh.write(f'{now:.0f}\t{p}\n')
        if os.path.getsize(f) > 262144:          # rare: keep only the tail
            with open(f, 'rb') as fh:
                fh.seek(-65536, 2); tail = fh.read()
            with open(f, 'wb') as fh:
                fh.write(tail[tail.find(b'\n') + 1:])
    except Exception:
        pass


def notify(line):
    """For the coordinator: one line in regia/avvisi.jsonl (Juggl reads it on the next round, within 3 seconds)."""
    try:
        os.makedirs(COORD_DIR, exist_ok=True)
        with open(NOTICES, 'a', encoding='utf-8') as fh:
            fh.write(json.dumps(line, ensure_ascii=False) + '\n')
    except Exception:
        pass


def user_name():
    n = ((read_json(os.path.join(BASE, 'impostazioni.json'), {}) or {}).get('nome') or '').strip()
    return n or "chi usa Juggl"


def who_holds(me, p, reg, now):
    """Another chat of the same group, working, that holds p. (session, folder or '', state) or None."""
    sessions = reg.get('sessioni') or {}
    g = (sessions.get(me) or {}).get('g')
    if not g:
        return None
    own = state_of(me)
    my_t = folder_since(own, p)
    my_touches = None
    for sid, x in sessions.items():
        if sid == me or x.get('g') != g:
            continue
        st = state_of(sid)
        if not is_working(st):
            continue
        their_t = folder_since(st, p)
        if their_t is not None:
            if my_t is not None and my_t <= their_t:
                continue                     # I declared it first
            c = min((c for c in st.get('cartelle') or [] if inside(p, c)), key=len)
            return sid, c, st
        if my_t is None:
            their_touch_t = touches(sid, now).get(p)
            if their_touch_t is not None:
                if my_touches is None:
                    my_touches = touches(me, now)
                my_touch_t = my_touches.get(p)
                if my_touch_t is None or my_touch_t > their_touch_t:
                    return sid, '', st
    return None


def hook():
    try:
        d = json.loads(sys.stdin.read() or '{}')
    except Exception:
        return
    sid = d.get('session_id') or ''
    if not sid or d.get('tool_name') not in TOOLS:
        return
    reg = read_json(REGISTRY) or {}
    sessions = reg.get('sessioni') or {}
    if sid not in sessions:
        return                               # outside a group with a coordinator: pass right away
    ti = d.get('tool_input') or {}
    p = ti.get('file_path') or ti.get('notebook_path') or ''
    if not p:
        return
    p = norm(p, d.get('cwd') or '')
    now = time.time()
    c = who_holds(sid, p, reg, now)
    if not c:
        if p not in touches(sid, now):
            record_touch(sid, p, now)
        return
    theirs, folder, _ = c
    their_name = (sessions.get(theirs) or {}).get('nome') or 'un\'altra chat'
    what = f'la cartella {folder}' if folder else 'questo file'
    u = user_name()
    reason = (f"Fermata da Juggl: {what} la sta usando la chat «{their_name}» dello stesso gruppo, che sta lavorando adesso. "
              f"Non modificare {p} e non aggirare il blocco (niente sed, cp, script o Bash sullo stesso file). "
              f"Fai altro che non tocchi {what}, oppure chiudi il turno dicendo a {u} che aspetti che «{their_name}» finisca. "
              "La regia del gruppo e' gia' stata avvisata.")
    notify({'t': now, 'tipo': 'modifica', 'gruppo': sessions[sid].get('g'), 'sessione': sid, 'nome': sessions[sid].get('nome') or '',
            'tenuta_da': theirs, 'tenuta_nome': their_name, 'cartella': folder, 'file': p})
    if not os.path.exists(BLOCK_FILE):
        # trial period (Nov 12, coordinator): warning only, the edit goes through. The block is turned on by the file regia/blocco-acceso
        note = (f"Attenzione da Juggl: {what} la sta usando anche la chat «{their_name}» dello stesso gruppo, che sta lavorando adesso. "
                f"Se puoi, non modificare {p} finche' non ha finito; la regia del gruppo e' stata avvisata.")
        print(json.dumps({'hookSpecificOutput': {'hookEventName': 'PreToolUse', 'additionalContext': note}}, ensure_ascii=False))
        return
    print(json.dumps({'hookSpecificOutput': {'hookEventName': 'PreToolUse', 'permissionDecision': 'deny',
                                             'permissionDecisionReason': reason}}, ensure_ascii=False))


# ---------- juggl cartelle ----------

def option(a, name):
    if name in a:
        i = a.index(name)
        if i + 1 < len(a):
            v = a[i + 1]; del a[i:i + 2]; return v
        del a[i]
    return None


def group_of(sid, reg):
    g = ((reg.get('sessioni') or {}).get(sid) or {}).get('g')
    if g:
        return g
    for k, x in (reg.get('gruppi') or {}).items():      # the coordinator itself: its group
        if x.get('regia') == sid:
            return k
    return None


def cli(a):
    a = list(a)
    sid = option(a, '--sessione') or os.environ.get('CLAUDE_CODE_SESSION_ID') or ''
    reg = read_json(REGISTRY) or {}
    if '--elenco' in a:
        g = group_of(sid, reg) if sid else None
        groups = [g] if g else list((reg.get('gruppi') or {}).keys())
        if not groups:
            print('nessun gruppo con la regia accesa: nessun controllo sulle cartelle'); return 0
        for gg in groups:
            print(f"Gruppo {(reg.get('gruppi') or {}).get(gg, {}).get('nome') or gg}:")
            for s_, x in (reg.get('sessioni') or {}).items():
                if x.get('g') != gg:
                    continue
                st = state_of(s_)
                folders = st.get('cartelle') or []
                print(f"  - {x.get('nome')} [{'al lavoro' if is_working(st) else st.get('stato') or '?'}]: {', '.join(folders) if folders else 'nessuna cartella dichiarata'}")
        return 0
    if not sid:
        print('juggl cartelle va lanciato da dentro una chat (oppure con --sessione <id>)', file=sys.stderr); return 2
    state_path = os.path.join(STATE_DIR, sid + '.json')
    if not os.path.exists(state_path):
        print(f'chat {sid[:8]} sconosciuta a Juggl: niente da registrare', file=sys.stderr); return 2
    sys.path.insert(0, os.path.dirname(os.path.realpath(__file__)))
    import comune
    if '--libera' in a:
        comune.aggiorna(state_path, lambda v: ({**v, 'cartelle': [], 'cartelle_t': {}} if isinstance(v, dict) and v else comune.SALTA), {}, attesa=5)
        print('cartelle liberate'); return 0
    folders = [norm(x) for x in a if not x.startswith('--')]
    if not folders:
        print('uso: juggl cartelle <cartella...> | --libera | --elenco', file=sys.stderr); return 2
    now = time.time()
    g = group_of(sid, reg)
    if g:
        sessions = reg.get('sessioni') or {}
        own = state_of(sid)
        for s_, x in sessions.items():
            if s_ == sid or x.get('g') != g:
                continue
            st = state_of(s_)
            if not is_working(st):
                continue
            theirs = [c for c in st.get('cartelle') or [] for m in folders if inside(m, c) or inside(c, m)]
            # already own before theirs: stays own
            theirs = [c for c in theirs if not any(inside(c, m) or inside(m, c) for m in own.get('cartelle') or []
                                               if float((own.get('cartelle_t') or {}).get(m) or 0) <= float((st.get('cartelle_t') or {}).get(c) or 0))]
            touched = [p for p in touches(s_, now) if any(inside(p, m) for m in folders)] if not theirs else []
            if theirs or touched:
                what = theirs[0] if theirs else os.path.dirname(touched[0])
                their_name = x.get('nome') or 'un\'altra chat'
                notify({'t': now, 'tipo': 'dichiarazione', 'gruppo': g, 'sessione': sid, 'nome': (sessions.get(sid) or {}).get('nome') or '',
                        'tenuta_da': s_, 'tenuta_nome': their_name, 'cartella': what, 'file': touched[0] if touched else ''})
                print(f"Fermata: {what} la sta usando la chat «{their_name}» dello stesso gruppo, che sta lavorando. "
                      f"Non lavorare li' finche' non ha finito: fai altro, oppure chiudi il turno dicendo a {user_name()} che aspetti. "
                      "La regia del gruppo e' stata avvisata.")
                return 3
    def put(v):
        if not isinstance(v, dict) or not v:
            return comune.SALTA
        before = v.get('cartelle_t') or {}
        v['cartelle'] = folders
        v['cartelle_t'] = {c: before.get(c) or now for c in folders}
        return v
    comune.aggiorna(state_path, put, {}, attesa=5)
    print('cartelle registrate: ' + ', '.join(folders) + ('' if g else '  (nel gruppo non c\'e\' una regia accesa: nessun controllo)'))
    return 0


if __name__ == '__main__':
    a = sys.argv[1:]
    if a[:1] == ['hook']:
        try:
            hook()
        except Exception:
            pass
        sys.exit(0)
    sys.exit(cli(a))
