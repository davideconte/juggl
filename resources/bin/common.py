# juggl-gestito: Juggl installs and updates this file. If you edit it by hand, remove this line.
"""comune: safe writes of Juggl's shared files (Nov 8, shared-files protocol of the plan).

Same convention as the Rust core, never change it on one side only:
  - for a file X the lock is X.lock, taken with an exclusive flock (fcntl.flock, never lockf/fcntl);
  - the temp file is X.tmp.<pid>.<n> in the same folder, then fsync and os.replace;
  - read-modify-write always under the lock (aggiorna);
  - if X exists but cannot be read, retry and never rewrite it from scratch (Illeggibile).

  leggi(p, default)            JSON of p, or default if missing or unreadable (after a few attempts)
  scrivi(p, dati)              atomic write under the lock
  aggiorna(p, funzione, default={})   reads, calls funzione(dati), writes. funzione may change dati in place
                               or return a new value; if it returns SALTA nothing is written. Returns the final data.
  scrivi_testo(p, testo)       like scrivi, but with ready-made text (jsonl, custom formats)
  aggiungi_riga(p, oggetto)    one JSON line at the end of a .jsonl, under the same lock as whoever rewrites it
  lucchetto(p)                 the lock of p, to use with `with` (for callers that must hold it around several operations)
  scrivi_atomico(p, testo)     only temp file + fsync + rename, for callers that already hold the lock of p
  leggi_rigoroso(p, default)   like leggi, but if the file exists and cannot be read it raises Illeggibile (for callers that rewrite it)

Standard library only, quick to import (the hook of every session uses it too). Compatible with Python 3.9.
"""
import copy
import errno
import fcntl
import itertools
import json
import os
import time

SALTA = object()
_counter = itertools.count()


class Illeggibile(Exception):
    """The file exists but is not valid JSON: better not to write than to lose data."""


class lucchetto:
    """Exclusive flock on <p>.lock. attesa: max seconds (None = no limit), then TimeoutError."""

    def __init__(self, p, attesa=30.0):
        self.p, self.attesa, self.fd = p, attesa, None

    def __enter__(self):
        self.fd = os.open(self.p + '.lock', os.O_RDWR | os.O_CREAT, 0o644)
        if self.attesa is None:
            fcntl.flock(self.fd, fcntl.LOCK_EX)
            return self
        deadline = time.monotonic() + self.attesa
        delay = 0.002
        while True:
            try:
                fcntl.flock(self.fd, fcntl.LOCK_EX | fcntl.LOCK_NB)
                return self
            except OSError as e:
                if e.errno not in (errno.EWOULDBLOCK, errno.EAGAIN, errno.EACCES):
                    os.close(self.fd); self.fd = None
                    raise
            if time.monotonic() > deadline:
                os.close(self.fd); self.fd = None
                raise TimeoutError('lucchetto occupato: ' + self.p + '.lock')
            time.sleep(delay)
            delay = min(delay * 2, 0.05)

    def __exit__(self, *a):
        if self.fd is not None:
            try:
                fcntl.flock(self.fd, fcntl.LOCK_UN)
            finally:
                os.close(self.fd); self.fd = None


def _json_text(dati, indent):
    return json.dumps(dati, ensure_ascii=False, indent=indent)


def _scrivi_atomico(p, testo):
    """Unique temp file in the same folder, fsync, rename. Keeps the permissions of the file it replaces."""
    folder = os.path.dirname(p) or '.'
    os.makedirs(folder, exist_ok=True)
    tmp = '%s.tmp.%d.%d' % (p, os.getpid(), next(_counter))
    dati = testo.encode('utf-8') if isinstance(testo, str) else testo
    fd = os.open(tmp, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o644)
    try:
        try:
            os.fchmod(fd, os.stat(p).st_mode & 0o7777)
        except OSError:
            pass
        view = memoryview(dati)
        while view:
            n = os.write(fd, view)
            view = view[n:]
        os.fsync(fd)
    except BaseException:
        os.close(fd)
        try: os.unlink(tmp)
        except OSError: pass
        raise
    os.close(fd)
    try:
        os.replace(tmp, p)
    except BaseException:
        try: os.unlink(tmp)
        except OSError: pass
        raise


def _leggi_rigoroso(p, default, attempts=4):
    """Missing -> copy of default. Exists but unreadable -> retry, then Illeggibile."""
    for i in range(attempts):
        try:
            with open(p, 'rb') as f:
                raw = f.read()
        except FileNotFoundError:
            return copy.deepcopy(default)
        try:
            return json.loads(raw.decode('utf-8'))
        except (ValueError, UnicodeDecodeError):
            if i + 1 < attempts:
                time.sleep(0.01 * (i + 1))
    raise Illeggibile(p)


def leggi(p, default=None):
    try:
        return _leggi_rigoroso(p, default)
    except (Illeggibile, OSError):
        return copy.deepcopy(default)


def scrivi(p, dati, indent=1, attesa=30.0):
    with lucchetto(p, attesa):
        _scrivi_atomico(p, _json_text(dati, indent))


def scrivi_testo(p, testo, attesa=30.0):
    with lucchetto(p, attesa):
        _scrivi_atomico(p, testo)


def aggiorna(p, funzione, default=None, indent=1, attesa=30.0):
    if default is None:
        default = {}
    with lucchetto(p, attesa):
        dati = _leggi_rigoroso(p, default)
        r = funzione(dati)
        if r is SALTA:
            return dati
        if r is not None:
            dati = r
        _scrivi_atomico(p, _json_text(dati, indent))
        return dati


def aggiungi_riga(p, oggetto, attesa=30.0):
    line = (oggetto if isinstance(oggetto, str) else json.dumps(oggetto, ensure_ascii=False)) + '\n'
    with lucchetto(p, attesa):
        os.makedirs(os.path.dirname(p) or '.', exist_ok=True)
        fd = os.open(p, os.O_WRONLY | os.O_CREAT | os.O_APPEND, 0o644)
        try:
            os.write(fd, line.encode('utf-8'))
        finally:
            os.close(fd)


scrivi_atomico = _scrivi_atomico
leggi_rigoroso = _leggi_rigoroso
