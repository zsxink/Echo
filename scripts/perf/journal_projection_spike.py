#!/usr/bin/env python3
"""Isolated design spike; never opens Echo's real database or library.

Measure durable operation-file publication and asynchronous SQLite projection.
This is a protocol experiment, not a production adapter or Tauri benchmark.
"""
import argparse
from concurrent.futures import ThreadPoolExecutor
import fcntl
import hashlib
import json
import os
import queue
import sqlite3
import subprocess
import sys
import tempfile
import threading
import time
import unicodedata
import uuid
from pathlib import Path


def encode(value):
    return json.dumps(value, sort_keys=True, separators=(",", ":"), ensure_ascii=False).encode("utf-8")


def sync(fd):
    if sys.platform == "darwin" and hasattr(fcntl, "F_FULLFSYNC"):
        fcntl.fcntl(fd, fcntl.F_FULLFSYNC)
    else:
        os.fsync(fd)


def sync_directory(path):
    fd = os.open(path, os.O_RDONLY)
    try:
        sync(fd)
    finally:
        os.close(fd)


def write_all(fd, data):
    view = memoryview(data)
    while view:
        size = os.write(fd, view)
        if size <= 0:
            raise OSError("short write without progress")
        view = view[size:]


def envelope(event):
    return encode({"event": event, "sha256": hashlib.sha256(encode(event)).hexdigest()})


def crash(point, current):
    if point == current:
        os._exit(73)


def filename(event):
    return "%020d-%s.json" % (event["seq"], event["id"])


def publish(folder, event, point=None):
    """Exclusive publication, one file sync and one directory sync per event."""
    data = envelope(event)
    temporary = folder / (".tmp-" + str(uuid.uuid4()))
    target = folder / filename(event)
    fd = os.open(temporary, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    try:
        if point == "partial_temp":
            write_all(fd, data[:len(data) // 2])
            crash(point, "partial_temp")
        write_all(fd, data)
        crash(point, "temp_written")
        started = time.perf_counter_ns()
        sync(fd)
        file_ms = (time.perf_counter_ns() - started) / 1e6
        crash(point, "file_synced")
    finally:
        os.close(fd)
    try:
        os.link(temporary, target)
        crash(point, "linked")
        os.unlink(temporary)
        started = time.perf_counter_ns()
        sync_directory(folder)
        directory_ms = (time.perf_counter_ns() - started) / 1e6
        crash(point, "directory_synced")
        return file_ms, directory_ms
    finally:
        if temporary.exists():
            temporary.unlink()


def read_events(folder, refence=False):
    """Verify every visible final record and prove its publication on restart."""
    events = []
    ids = set()
    paths = sorted(folder.glob("*.json"))
    for path in paths:
        if path.is_symlink():
            raise ValueError("symlinked operation")
        raw = json.loads(path.read_bytes())
        event = raw["event"]
        if raw["sha256"] != hashlib.sha256(encode(event)).hexdigest():
            raise ValueError("operation checksum mismatch")
        if filename(event) != path.name or event["seq"] != len(events) + 1 or event["id"] in ids:
            raise ValueError("operation sequence or identity mismatch")
        if refence:
            fd = os.open(path, os.O_RDONLY)
            try:
                sync(fd)
            finally:
                os.close(fd)
        events.append(event)
        ids.add(event["id"])
    if refence and paths:
        sync_directory(folder)
    return events


def database(path, songs):
    connection = sqlite3.connect(path, timeout=5)
    connection.execute("PRAGMA journal_mode=WAL")
    connection.execute("PRAGMA synchronous=FULL")
    connection.executescript("""
        CREATE TABLE IF NOT EXISTS songs(id TEXT PRIMARY KEY, favorite INTEGER NOT NULL);
        CREATE TABLE IF NOT EXISTS playlists(id TEXT PRIMARY KEY, name TEXT NOT NULL, name_key TEXT UNIQUE NOT NULL);
        CREATE TABLE IF NOT EXISTS applied(id TEXT PRIMARY KEY, seq INTEGER UNIQUE NOT NULL, digest TEXT NOT NULL);
        CREATE TABLE IF NOT EXISTS watermark(singleton INTEGER PRIMARY KEY CHECK(singleton=1), seq INTEGER NOT NULL);
        INSERT OR IGNORE INTO watermark VALUES(1, 0);
    """)
    connection.executemany("INSERT OR IGNORE INTO songs(id,favorite) VALUES(?,0)", ((f"song-{i}",) for i in range(songs)))
    connection.commit()
    return connection


def apply(connection, events, point=None):
    if not events:
        return
    connection.execute("BEGIN IMMEDIATE")
    try:
        current = connection.execute("SELECT seq FROM watermark WHERE singleton=1").fetchone()[0]
        for event in events:
            digest = hashlib.sha256(encode(event)).hexdigest()
            if event["seq"] <= current:
                existing = connection.execute("SELECT seq,digest FROM applied WHERE id=?", (event["id"],)).fetchone()
                if existing != (event["seq"], digest):
                    raise ValueError("projection identity conflict")
                continue
            if event["seq"] != current + 1:
                raise ValueError("projection gap")
            if event["kind"] == "favorite":
                cursor = connection.execute("UPDATE songs SET favorite=? WHERE id=?", (int(event["favorite"]), event["song"]))
                if cursor.rowcount != 1:
                    raise ValueError("missing song")
            elif event["kind"] == "playlist":
                connection.execute("INSERT INTO playlists(id,name,name_key) VALUES(?,?,?)", (event["playlist"], event["name"], event["name_key"]))
            else:
                raise ValueError("unknown operation")
            connection.execute("INSERT INTO applied VALUES(?,?,?)", (event["id"], event["seq"], digest))
            current = event["seq"]
        crash(point, "sqlite_before_commit")
        connection.execute("UPDATE watermark SET seq=? WHERE singleton=1", (current,))
        connection.commit()
        crash(point, "sqlite_committed")
    except BaseException:
        connection.rollback()
        raise


class Journal:
    def __init__(self, root, songs, start_worker=True):
        self.root = Path(root)
        self.folder = self.root / "operations"
        self.folder.mkdir(exist_ok=True)
        self.lock = open(self.root / "writer.lock", "a+b")
        fcntl.flock(self.lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
        self.events = read_events(self.folder, refence=True)
        self.by_id = {event["id"]: event for event in self.events}
        self.song_values = {f"song-{i}": False for i in range(songs)}
        self.names = {}
        for event in self.events:
            self.remember(event)
        self.locked = threading.Lock()
        self.pending = queue.Queue()
        self.lags = []
        self.errors = []
        self.songs = songs
        self.worker_ready = threading.Event()
        self.connection = database(self.root / "index.sqlite", songs)
        apply(self.connection, self.events)
        self.connection.close()
        self.worker = None
        if start_worker:
            self.worker = threading.Thread(target=self.project, daemon=True)
            self.worker.start()
            if not self.worker_ready.wait(timeout=10) or self.errors:
                raise RuntimeError("projection worker failed to initialize: " + repr(self.errors))

    def remember(self, event):
        if event["kind"] == "favorite":
            self.song_values[event["song"]] = event["favorite"]
        elif event["kind"] == "playlist":
            self.names[event["name_key"]] = event["playlist"]

    def append(self, intent):
        start = time.perf_counter_ns()
        with self.locked:
            existing = self.by_id.get(intent["id"])
            if existing is not None:
                if {k: v for k, v in existing.items() if k != "seq"} != intent:
                    raise ValueError("operation ID reused with a different payload")
                return existing, None
            if intent["kind"] == "favorite" and intent["song"] not in self.song_values:
                raise ValueError("missing song")
            if intent["kind"] == "playlist" and intent["name_key"] in self.names:
                raise ValueError("duplicate playlist name")
            event = dict(intent, seq=len(self.events) + 1)
            file_ms, directory_ms = publish(self.folder, event)
            self.events.append(event)
            self.by_id[event["id"]] = event
            self.remember(event)
            confirmed = time.perf_counter_ns()
            if self.worker is not None:
                self.pending.put((event, confirmed))
            returned = time.perf_counter_ns()
            return event, {"save_ms": (returned - start) / 1e6, "file_sync_ms": file_ms, "dir_sync_ms": directory_ms}

    def project(self):
        try:
            connection = database(self.root / "index.sqlite", self.songs)
            self.worker_ready.set()
            while True:
                first = self.pending.get()
                if first is None:
                    self.pending.task_done()
                    break
                batch = [first]
                while len(batch) < 64:
                    try:
                        item = self.pending.get_nowait()
                    except queue.Empty:
                        break
                    if item is None:
                        self.pending.task_done()
                        self.pending.put(None)
                        break
                    batch.append(item)
                try:
                    apply(connection, [item[0] for item in batch])
                    now = time.perf_counter_ns()
                    self.lags.extend((now - item[1]) / 1e6 for item in batch)
                finally:
                    for _ in batch:
                        self.pending.task_done()
            connection.close()
        except BaseException as error:
            self.errors.append(repr(error))
            self.worker_ready.set()

    def close(self):
        if self.worker:
            self.pending.put(None)
            self.worker.join(timeout=120)
            if self.worker.is_alive() or self.errors:
                raise RuntimeError("projection worker failed or timed out: " + repr(self.errors))
        fcntl.flock(self.lock, fcntl.LOCK_UN)
        self.lock.close()


def percentile(values, fraction):
    values = sorted(values)
    return round(values[max(0, int((len(values) * fraction + 0.999999)) - 1)], 3)


def stats(values):
    return {"count": len(values), "p50_ms": percentile(values, .5), "p95_ms": percentile(values, .95), "p99_ms": percentile(values, .99), "max_ms": round(max(values), 3)}


def child(root, point, event):
    folder = Path(root) / "operations"
    if point == "lock_check":
        try:
            journal = Journal(root, 1, start_worker=False)
        except BlockingIOError:
            return
        journal.close()
        raise AssertionError("second writer unexpectedly acquired the lock")
    if point in ("sqlite_before_commit", "sqlite_committed"):
        connection = database(Path(root) / "index.sqlite", 1)
        apply(connection, [event], point)
    else:
        publish(folder, event, point)
    raise AssertionError("crash injection did not fire")


def invoke_child(root, point, event, expected=73):
    result = subprocess.run([sys.executable, __file__, "--child", str(root), point, json.dumps(event)], capture_output=True, text=True)
    if result.returncode != expected:
        raise AssertionError(f"child {point} returned {result.returncode}: {result.stderr}")


def crash_matrix():
    cases = []
    for point in ("partial_temp", "temp_written", "file_synced", "linked", "directory_synced", "sqlite_before_commit", "sqlite_committed"):
        with tempfile.TemporaryDirectory(prefix="echo-journal-spike-crash-") as name:
            root = Path(name)
            journal = Journal(root, 1, start_worker=False)
            first, _ = journal.append({"id": str(uuid.uuid4()), "kind": "favorite", "song": "song-0", "favorite": True})
            journal.close()
            db = database(root / "index.sqlite", 1)
            apply(db, [first])
            db.close()
            intent = {"id": str(uuid.uuid4()), "kind": "favorite", "song": "song-0", "favorite": False}
            event = dict(intent, seq=2)
            if point.startswith("sqlite_"):
                publish(root / "operations", event)
            invoke_child(root, point, event)
            journal = Journal(root, 1, start_worker=False)
            visible = len(journal.events) == 2
            if point in ("partial_temp", "temp_written", "file_synced") and visible:
                raise AssertionError("unpublished operation became committed")
            if point in ("linked", "directory_synced", "sqlite_before_commit", "sqlite_committed") and not visible:
                raise AssertionError("visible committed operation disappeared in process-crash test")
            saved, _ = journal.append(intent)
            assert saved["seq"] == 2
            duplicate, _ = journal.append(intent)
            assert duplicate == saved and len(journal.events) == 2
            try:
                journal.append(dict(intent, favorite=True))
                raise AssertionError("conflicting retry accepted")
            except ValueError:
                pass
            journal.close()
            rebuilt = Journal(root, 1, start_worker=False)
            db = sqlite3.connect(root / "index.sqlite")
            assert db.execute("SELECT seq FROM watermark").fetchone()[0] == 2
            assert db.execute("SELECT favorite FROM songs WHERE id='song-0'").fetchone()[0] == 0
            assert db.execute("SELECT COUNT(*) FROM applied").fetchone()[0] == 2
            db.close()
            rebuilt.close()
            cases.append(point)
    return cases


def behavioral_checks():
    with tempfile.TemporaryDirectory(prefix="echo-journal-spike-check-") as name:
        root = Path(name)
        journal = Journal(root, 1)
        invoke_child(root, "lock_check", {}, expected=0)
        for value in (True, False, True):
            journal.append({"id": str(uuid.uuid4()), "kind": "favorite", "song": "song-0", "favorite": value})
            assert journal.song_values["song-0"] == value
        key = unicodedata.normalize("NFKC", "测试").casefold()
        journal.append({"id": str(uuid.uuid4()), "kind": "playlist", "playlist": str(uuid.uuid4()), "name": "测试", "name_key": key})
        try:
            journal.append({"id": str(uuid.uuid4()), "kind": "playlist", "playlist": str(uuid.uuid4()), "name": "测试", "name_key": key})
            raise AssertionError("duplicate playlist accepted")
        except ValueError:
            pass
        journal.close()
        # Remove the disposable projection and rebuild it from saved events.
        for path in root.glob("index.sqlite*"):
            path.unlink()
        journal = Journal(root, 1, start_worker=False)
        db = sqlite3.connect(root / "index.sqlite")
        assert db.execute("SELECT favorite FROM songs WHERE id='song-0'").fetchone()[0] == 1
        assert db.execute("SELECT COUNT(*) FROM playlists").fetchone()[0] == 1
        assert db.execute("SELECT seq FROM watermark").fetchone()[0] == 4
        db.close()
        journal.close()
        final = sorted((root / "operations").glob("*.json"))[0]
        raw = json.loads(final.read_bytes())
        raw["event"]["favorite"] = not raw["event"]["favorite"]
        final.write_bytes(encode(raw))
        try:
            read_events(root / "operations")
            raise AssertionError("tampered committed record accepted")
        except ValueError:
            pass
    return ["cross-process writer exclusion", "immediate in-memory reads", "duplicate playlist", "SQLite rebuild", "committed-record tamper"]


def projection_lock_check():
    with tempfile.TemporaryDirectory(prefix="echo-journal-spike-lock-") as name:
        root = Path(name)
        journal = Journal(root, 1)
        blocked = sqlite3.connect(root / "index.sqlite", timeout=1)
        blocked.execute("BEGIN IMMEDIATE")
        event, sample = journal.append({"id": str(uuid.uuid4()), "kind": "favorite", "song": "song-0", "favorite": True})
        assert journal.song_values["song-0"] is True
        assert blocked.execute("SELECT favorite FROM songs WHERE id='song-0'").fetchone()[0] == 0
        time.sleep(.1)
        blocked.rollback()
        blocked.close()
        journal.close()
        db = sqlite3.connect(root / "index.sqlite")
        assert db.execute("SELECT seq FROM watermark").fetchone()[0] == event["seq"]
        assert db.execute("SELECT favorite FROM songs WHERE id='song-0'").fetchone()[0] == 1
        db.close()
        assert journal.lags and journal.lags[0] >= 90
        return {"save_ms_while_sqlite_locked": sample["save_ms"], "sqlite_projection_lag_ms": round(journal.lags[0], 3)}


def concurrent_writers_check():
    with tempfile.TemporaryDirectory(prefix="echo-journal-spike-concurrent-") as name:
        root = Path(name)
        journal = Journal(root, 1)

        def favorite(index):
            return journal.append({"id": str(uuid.uuid4()), "kind": "favorite", "song": "song-0", "favorite": bool(index % 2)})[0]

        def playlist(_index):
            try:
                journal.append({"id": str(uuid.uuid4()), "kind": "playlist", "playlist": str(uuid.uuid4()), "name": "并发歌单", "name_key": "并发歌单"})
                return True
            except ValueError:
                return False

        with ThreadPoolExecutor(max_workers=8) as pool:
            events = list(pool.map(favorite, range(100)))
            creates = list(pool.map(playlist, range(8)))
        assert sorted(event["seq"] for event in events) == list(range(1, 101))
        assert sum(creates) == 1
        expected = max(events, key=lambda event: event["seq"])["favorite"]
        assert journal.song_values["song-0"] == expected
        journal.close()
        db = sqlite3.connect(root / "index.sqlite")
        assert db.execute("SELECT favorite FROM songs WHERE id='song-0'").fetchone()[0] == int(expected)
        assert db.execute("SELECT COUNT(*) FROM playlists").fetchone()[0] == 1
        assert db.execute("SELECT seq FROM watermark").fetchone()[0] == 101
        db.close()
    return {"parallel_writers": 8, "favorite_operations": 100, "duplicate_name_attempts": 8, "playlists_created": 1}


def benchmark(songs, samples):
    with tempfile.TemporaryDirectory(prefix="echo-journal-spike-bench-") as name:
        journal = Journal(name, songs)
        measures = {"favorite": [], "playlist": []}
        for index in range(samples * 2):
            if index % 2 == 0:
                song = f"song-{(index * 37) % songs}"
                intent = {"id": str(uuid.uuid4()), "kind": "favorite", "song": song, "favorite": not journal.song_values[song]}
            else:
                name = f"playlist-{index}"
                intent = {"id": str(uuid.uuid4()), "kind": "playlist", "playlist": str(uuid.uuid4()), "name": name, "name_key": name.casefold()}
            event, sample = journal.append(intent)
            measures[event["kind"]].append(sample)
        journal.close()
        connection = sqlite3.connect(journal.root / "index.sqlite")
        assert connection.execute("SELECT seq FROM watermark").fetchone()[0] == samples * 2
        assert connection.execute("SELECT COUNT(*) FROM applied").fetchone()[0] == samples * 2
        connection.close()
        result = {}
        for kind, rows in measures.items():
            result[kind] = {key: stats([row[key] for row in rows]) for key in ("save_ms", "file_sync_ms", "dir_sync_ms")}
        result["projection_lag"] = stats(journal.lags)
        result["projection_confirmed"] = len(journal.lags)
        return result


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--samples", type=int, default=200)
    parser.add_argument("--songs", type=int, default=1000)
    parser.add_argument("--report")
    parser.add_argument("--child", nargs=3)
    args = parser.parse_args()
    if args.child:
        child(args.child[0], args.child[1], json.loads(args.child[2]))
        return
    if args.samples < 20 or args.songs < 1:
        parser.error("samples must be at least 20 and songs must be positive")
    report = {"scope": "Python prototype; operation-file durability and asynchronous SQLite projection; excludes React, Tauri, actual Echo records/sync and power-loss testing", "platform": sys.platform, "songs": args.songs, "samples_per_operation": args.samples, "sync": "F_FULLFSYNC" if sys.platform == "darwin" and hasattr(fcntl, "F_FULLFSYNC") else "fsync", "crash_injection": crash_matrix(), "behavioral_checks": behavioral_checks(), "sqlite_lock_check": projection_lock_check(), "concurrent_writers_check": concurrent_writers_check(), "measurements": benchmark(args.songs, args.samples)}
    output = json.dumps(report, ensure_ascii=False, indent=2)
    if args.report:
        Path(args.report).write_text(output + "\n")
    print(output)


if __name__ == "__main__":
    main()
