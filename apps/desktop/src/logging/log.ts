// Echo renderer-side logging conventions (task 1.8).
//
// The renderer never logs IPC payload contents, full absolute paths, lyrics or
// tag text. See docs/LOGGING.md for the documented policy. In later tasks this
// forwards structured events to the Rust side via Tauri; today it is a thin,
// console-based convention with the same privacy contract as the Rust core:
//
// - `log.info` / `log.warn` / `log.error` accept a message, a domain `operation`,
//   and optional structured fields.
// - Paths that look absolute are redacted to a short stable hash + file name
//   before they can reach the output.
// - Payload-like values (arg buffers, base64, raw DTOs) are never logged; a
//   caller who must correlate them passes a precomputed `hash` field instead.

// Stable FNV-1a 32-bit, shared with the frontend's redaction helper so the
// hash tag is deterministic for a given path (documented as not
// cryptographically secure — log redaction only).
export function fnv1a32(input: string): string {
  let hash = 0x811c9dc5;
  for (let i = 0; i < input.length; i++) {
    hash ^= input.charCodeAt(i);
    hash = Math.imul(hash, 0x01000193);
  }
  // Force unsigned 32-bit, then hex.
  return (hash >>> 0).toString(16).padStart(8, "0");
}

/** Absolute-path spans terminate at these characters (path-interior spaces are
 *  kept so `/Users/…/Night Drive/song.mp3` is redacted as one hash tag). */
const PATH_TERMINATORS = new Set(["]", ")", "(", ","]);

/** A path start (`/`, `\`, `file://`, or a drive letter) only denotes an
 *  absolute path when it sits at a token boundary — the start of the string or
 *  after whitespace / a quote / `(` / `[` / `,`. A `/` preceded by a path-segment
 *  character (e.g. the one inside `Albums/Night`) is part of a relative path and
 *  must NOT be treated as an absolute-path start. */
function isPathStartAt(message: string, i: number): boolean {
  if (i > 0 && !/[ \t"'(,[]/.test(message[i - 1])) {
    return false;
  }
  const c = message[i];
  if (c === "/" || c === "\\") return true;
  if (message.startsWith("file://", i)) return true;
  return (
    i + 2 < message.length &&
    /[A-Za-z]/.test(c) &&
    message[i + 1] === ":" &&
    (message[i + 2] === "/" || message[i + 2] === "\\") // Windows drive absolute, e.g. `C:\…`
  );
}

function absolutePathStartAt(message: string, from: number): number {
  for (let i = from; i < message.length; i++) {
    if (isPathStartAt(message, i)) return i;
  }
  return -1;
}

/** Redact one absolute-path span to `<file-name (short-hash)>`. */
function redactSpan(span: string): string {
  const clean = span.split(/[\\/]/).filter(Boolean);
  const name = clean.length ? clean[clean.length - 1] : span;
  const display = name.length > 120 ? "<redacted>" : name;
  return `${display} (${fnv1a32(span)})`;
}

/** Privacy guard: replace every absolute path found in `message` (including
 *  paths that contain spaces) with `<file-name (short-hash)>`. Relative paths,
 *  hashes and ordinary text are preserved. Lyric/tag/payload text is NOT
 *  auto-detected here — per docs/LOGGING.md the caller must pre-hash such
 *  content and never inline it into the message or a field. */
export function redactMessage(message: string): string {
  let out = "";
  let i = 0;
  while (i < message.length) {
    const start = absolutePathStartAt(message, i);
    if (start === -1) {
      out += message[i];
      i += 1;
      continue;
    }
    out += message.slice(i, start);
    let j = start;
    while (j < message.length && !PATH_TERMINATORS.has(message[j])) j += 1;
    const span = message.slice(start, j).replace(/\s+$/, "");
    out += redactSpan(span);
    i = j;
  }
  return out;
}

/** Quote a single value into a JSON-ish string for a structured line. */
function quote(value: unknown): string {
  return JSON.stringify(String(value));
}

interface LogFields {
  [key: string]: unknown;
}

/**
 * Structured, privacy-safe log line. Any string field that looks like an
 * absolute path is redacted; presence of a `hash` field is the caller's
 * responsibility and the only place payload-like correlation data may appear.
 */
function line(level: string, message: string, operation: string, fields: LogFields): string {
  const safe = redactMessage(message);
  const parts: string[] = [`level=${level}`, `operation=${quote(operation)}`, `msg=${quote(safe)}`];
  for (const [key, value] of Object.entries(fields)) {
    const rendered = typeof value === "string" ? redactMessage(value) : String(value);
    parts.push(`${key}=${quote(rendered)}`);
  }
  return parts.join(" ");
}

export function info(message: string, operation: string, fields: LogFields = {}): void {
  console.log(line("INFO", message, operation, fields));
}

export function warn(message: string, operation: string, fields: LogFields = {}): void {
  console.warn(line("WARN", message, operation, fields));
}

export function error(message: string, operation: string, fields: LogFields = {}): void {
  console.error(line("ERROR", message, operation, fields));
}

/** Install the current line builder (used by the privacy test to capture). */
export function buildLine(
  level: string,
  message: string,
  operation: string,
  fields: LogFields = {},
): string {
  return line(level, message, operation, fields);
}
