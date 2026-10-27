#!/usr/bin/env node
// Real React handlers + loopback transport + native AppServices + durable files
// and SQLite. The transport is HTTP, not Tauri/WKWebView; report that boundary.
import { spawn } from "node:child_process";
import { createServer } from "node:http";
import { mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { dirname, extname, join, resolve, sep } from "node:path";
import { fileURLToPath } from "node:url";
import { createServer as createPortServer } from "node:net";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "../..");
const dist = join(root, "apps/desktop/dist");
const samples = Number(process.env.ECHO_PERF_SAMPLES ?? 200);
const budget = Number(process.env.ECHO_PERF_MAX_P95_MS ?? 50);
const release = process.env.ECHO_PERF_PROFILE === "release";
if (!Number.isSafeInteger(samples) || samples < 20 || !Number.isFinite(budget) || budget <= 0) {
  throw new Error("ECHO_PERF_SAMPLES must be >=20 and ECHO_PERF_MAX_P95_MS must be positive");
}
const pause = (ms) => new Promise((done) => setTimeout(done, ms));
const processes = [];
function child(command, args, options = {}) {
  const process = spawn(command, args, { cwd: root, stdio: ["ignore", "pipe", "pipe"], ...options });
  processes.push(process);
  return process;
}
async function run(command, args, options) {
  const process = child(command, args, options);
  process.stdout.pipe(globalThis.process.stdout);
  process.stderr.pipe(globalThis.process.stderr);
  await new Promise((done, reject) => {
    process.once("error", reject);
    process.once("exit", (code) => (code === 0 ? done() : reject(new Error(`${command} exited ${code}`))));
  });
}
async function availablePort() {
  const server = createPortServer();
  await new Promise((done) => server.listen(0, "127.0.0.1", done));
  const port = server.address().port;
  await new Promise((done) => server.close(done));
  return port;
}
class CDP {
  constructor(url) {
    this.socket = new WebSocket(url);
    this.pending = new Map();
    this.sequence = 0;
    this.socket.addEventListener("message", ({ data }) => {
      const reply = JSON.parse(data);
      const pending = this.pending.get(reply.id);
      if (!pending) return;
      this.pending.delete(reply.id);
      reply.error ? pending.reject(new Error(JSON.stringify(reply.error))) : pending.resolve(reply.result);
    });
  }
  async connect() {
    await new Promise((done, reject) => {
      this.socket.addEventListener("open", done, { once: true });
      this.socket.addEventListener("error", reject, { once: true });
    });
  }
  async call(method, params = {}) {
    const id = ++this.sequence;
    const result = new Promise((resolve, reject) => this.pending.set(id, { resolve, reject }));
    this.socket.send(JSON.stringify({ id, method, params }));
    return result;
  }
  async evaluate(expression) {
    const reply = await this.call("Runtime.evaluate", { expression, awaitPromise: true, returnByValue: true });
    if (reply.exceptionDetails) throw new Error(reply.exceptionDetails.exception?.description ?? "Page evaluation failed");
    return reply.result?.value;
  }
}
function percentile(values, fraction) {
  return values[Math.max(0, Math.ceil(values.length * fraction) - 1)];
}
function summarize(values) {
  if (values.length !== samples || values.some((value) => !Number.isFinite(value) || value < 0)) {
    throw new Error(`Expected ${samples} valid completion samples; got ${values.length}`);
  }
  const sorted = [...values].sort((a, b) => a - b);
  return { count: sorted.length, p50: percentile(sorted, 0.5), p95: percentile(sorted, 0.95), p99: percentile(sorted, 0.99), max: sorted.at(-1) };
}

let server;
let cdp;
let backendUrl;
const temporary = await mkdtemp(join(tmpdir(), "echo-edit-perf-browser-"));
try {
  if (process.env.ECHO_PERF_SKIP_BUILD !== "1") {
    const app = join(root, "apps/desktop");
    await run(process.execPath, [join(app, "node_modules/typescript/bin/tsc"), "-b"], { cwd: app });
    await run(process.execPath, [join(app, "node_modules/vite/bin/vite.js"), "build"], { cwd: app });
  }
  await readFile(join(dist, "e2e/perf.html"));
  const cargoArgs = ["test", "-p", "echo-desktop", "--test", "local_edit_performance", ...(release ? ["--release"] : []), "--", "--ignored", "--nocapture", "--test-threads=1"];
  const backend = child("cargo", cargoArgs, { env: { ...process.env, ECHO_PERF_SERVE: "1" } });
  backend.stderr.pipe(process.stderr);
  const fixture = await new Promise((done, reject) => {
    let output = "";
    const timeout = setTimeout(() => reject(new Error("Native fixture did not become ready within 10 minutes")), 600_000);
    backend.once("error", reject);
    backend.once("exit", (code) => reject(new Error(`Native fixture exited before ready: ${code}`)));
    backend.stdout.on("data", (data) => {
      output += data.toString();
      const lines = output.split("\n");
      output = lines.pop();
      for (const line of lines) {
        const position = line.indexOf("ECHO_PERF_READY ");
        if (position >= 0) {
          clearTimeout(timeout);
          done(JSON.parse(line.slice(position + "ECHO_PERF_READY ".length)));
        } else if (line.trim()) console.log(line);
      }
    });
  });
  backendUrl = fixture.url;
  server = createServer(async (request, response) => {
    const pathname = decodeURIComponent(new URL(request.url, "http://localhost").pathname);
    const path = resolve(dist, `.${pathname}`);
    if (!path.startsWith(`${dist}${sep}`)) {
      response.writeHead(403).end();
      return;
    }
    try {
      const body = await readFile(path);
      response.setHeader("content-type", { ".html": "text/html", ".js": "text/javascript", ".css": "text/css" }[extname(path)] ?? "application/octet-stream");
      response.end(body);
    } catch {
      response.writeHead(404).end();
    }
  });
  await new Promise((done) => server.listen(0, "127.0.0.1", done));
  const pageUrl = `http://127.0.0.1:${server.address().port}/e2e/perf.html?backend=${encodeURIComponent(backendUrl)}`;
  const debugPort = await availablePort();
  const chrome = process.env.CHROME_PATH ?? "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome";
  child(chrome, ["--headless=new", "--disable-gpu", "--no-first-run", "--no-default-browser-check", "--disable-background-timer-throttling", "--disable-renderer-backgrounding", ...(process.env.CHROME_NO_SANDBOX === "1" ? ["--no-sandbox"] : []), `--remote-debugging-port=${debugPort}`, `--user-data-dir=${join(temporary, "chrome")}`, pageUrl]);
  let target;
  for (let attempt = 0; attempt < 100; attempt++) {
    await pause(100);
    try {
      const tabs = await (await fetch(`http://127.0.0.1:${debugPort}/json/list`)).json();
      target = tabs.find((tab) => tab.type === "page");
      if (target) break;
    } catch { /* Chrome is still starting. */ }
  }
  if (!target) throw new Error("Headless Chrome did not start; set CHROME_PATH to a Chromium binary");
  cdp = new CDP(target.webSocketDebuggerUrl);
  await cdp.connect();
  await cdp.call("Runtime.enable");
  let ready = false;
  for (let attempt = 0; attempt < 100; attempt++) {
    ready = await cdp.evaluate("Boolean(window.__echoPerf__?.fixture)");
    if (ready) break;
    await pause(100);
  }
  if (!ready) throw new Error(`Performance page did not become ready: ${await cdp.evaluate("document.body.innerText")}`);
  console.log(`Measuring ${samples} operations per command, ${fixture.songCount} songs, profile=${release ? "release" : "dev"}`);
  const favoriteResult = await cdp.evaluate(`window.__echoPerf__.runFavorites(${samples})`);
  const playlistResult = await cdp.evaluate(`window.__echoPerf__.runPlaylists(${samples})`);
  const report = {
    measuredAt: new Date().toISOString(),
    scope: "real React DOM handler through loopback HTTP to native AppServices, durable directory and SQLite, back to React completion frame; excludes Tauri/WKWebView transport",
    profile: release ? "release" : "dev",
    fixture,
    budgetMs: budget,
    favorite: summarize(favoriteResult.map((sample) => sample.durationMs)),
    createPlaylist: summarize(playlistResult.map((sample) => sample.durationMs)),
    raw: { favorite: favoriteResult, createPlaylist: playlistResult },
  };
  const reportPath = process.env.ECHO_PERF_REPORT ?? join(temporary, "report.json");
  await writeFile(reportPath, `${JSON.stringify(report, null, 2)}\n`);
  console.log(JSON.stringify({ favorite: report.favorite, createPlaylist: report.createPlaylist, reportPath }, null, 2));
  if (report.favorite.p95 >= budget || report.createPlaylist.p95 >= budget) {
    process.exitCode = 1;
    console.error(`P95 must be strictly below ${budget}ms`);
  }
} finally {
  cdp?.socket.close();
  if (backendUrl) await fetch(`${backendUrl}/shutdown`).catch(() => undefined);
  for (const process of processes) if (process.exitCode === null) process.kill("SIGTERM");
  if (server) await new Promise((done) => server.close(done));
  await rm(join(temporary, "chrome"), { recursive: true, force: true, maxRetries: 10, retryDelay: 100 });
}
