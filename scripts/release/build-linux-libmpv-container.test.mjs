#!/usr/bin/env node
// Task 2.2 self-test: the Ubuntu 22.04 container bootstrap must not rely on
// anything the pinned base image cannot provide.
//
// Why this exists. The meson bootstrap is the one step in the container that
// is not an apt package, so it is the one step with no distro to fall back on.
// It reached that shape by guessing: the PEP 668 refusal was handled with
// `pip install --break-system-packages`, but that switch only exists in pip
// >= 23.0.1 and 22.04 ships pip 22.0.2. CI proved it the hard way —
// `no such option: --break-system-packages`, exit 2, ~35s into the step, with
// the container never reaching a single compile.
//
// The build cannot be exercised without docker, but the *shape* of the
// bootstrap is a static property of this file, so it can be asserted. That is
// the whole point: this class of bug (a flag the base image's pip predates) is
// detectable without a container, and was not detected for two CI rounds.
//
// These tests read the script as text. They do not build libmpv, do not invoke
// docker, and do not touch the network.

import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = resolve(fileURLToPath(new URL(".", import.meta.url)), "..", "..");
const SCRIPT = resolve(ROOT, "scripts", "release", "build-linux-libmpv-container.sh");
const source = readFileSync(SCRIPT, "utf8");

let failures = 0;
function assert(cond, label, detail) {
  if (cond) {
    process.stdout.write(`ok   ${label}\n`);
  } else {
    process.stderr.write(`FAIL ${label}\n`);
    if (detail) process.stderr.write(`     ${detail}\n`);
    failures += 1;
  }
}

// Matches a comment line (`# ...`) and yields its text, so assertions about
// *stated* reasons cannot be satisfied by a line that is merely present in
// some unrelated example.
function commentLines(pattern) {
  return source
    .split("\n")
    .filter((l) => l.trimStart().startsWith("#"))
    .join("\n")
    .match(pattern);
}

const commentText = source
  .split("\n")
  .filter((l) => l.trimStart().startsWith("#"))
  .join("\n");

// The apt list is the one place a build dependency may legitimately be
// introduced. A pip bootstrap that drifts outside it is the risk this guards.
// It is written with `\` line continuations, so it must be reassembled by
// shell's own rule — keep appending while the line ends in a backslash. A
// single-line match would read only `apt-get install -y --no-install-recommends \`
// and miss every package, which is how the python3-venv assertion below first
// failed against a script that did list it.
function joinedCommand(lines, startPredicate) {
  const out = [];
  for (const line of lines) {
    if (!out.length && !startPredicate(line)) continue;
    out.push(line);
    if (!line.trimEnd().endsWith("\\")) break;
  }
  return out.join("\n");
}
const aptInstall = joinedCommand(source.split("\n"), (l) => l.includes("apt-get install"));
const pipCommands = source
  .split("\n")
  .filter((l) => l.trimStart().startsWith("pip") || l.trimStart().startsWith("pip3"))
  .join("\n");

// 1. The exact regression: the flag 22.04's pip cannot parse. Asserted against
//    live shell lines, so it fires on a re-introduction even if someone rewrites
//    the surrounding comment.
assert(
  !/(^|\s)pip3?\s+install[^\n]*--break-system-packages/.test(pipCommands) &&
    !/(^|\s)pip3?\s+install[^\n]*--break-system-packages/.test(source),
  "pip bootstrap 不用 --break-system-packages（22.04 的 pip 22.0.2 解析不了这个开关）",
  `发现带该开关的 pip 安装行：\n${pipCommands}`,
);

// 2. The install must land in a venv, not the system Python. PEP 668 is the
//    reason the original was reaching for the bypass switch at all.
assert(
  /python3 -m venv/.test(pipCommands) || /python3 -m venv/.test(source),
  "meson 装进 python3 -m venv 建出的环境",
  "看不到 python3 -m venv —— 系统 Python 是 externally-managed 的，\n" +
    "而唯一的绕过开关在这张镜像上不可用（见断言 1）。",
);

// 3. A venv's bin dir must be on PATH, or `meson` on the next line resolves to
//    the apt 0.61.2 binary the whole change exists to replace — a failure that
//    would surface as a confusing libplacebo version error much later.
assert(
  /export PATH="\/opt\/echo-build-venv\/bin:\\\$PATH"/.test(source),
  "venv 的 bin 目录被加进 PATH（否则 meson 仍是 apt 的 0.61.2）",
  "找不到把 venv 加进 PATH 的 export 行。",
);

// 4. `command -v meson` early-checks the resolved version, but it only proves a
//    meson exists. `--version` is what actually catches a stale one, and the
//    whole reason for this bootstrap is the >= 1.3.0 floor.
assert(
  /meson --version/.test(source),
  "bootstrap 早期跑 meson --version（真正的版本下限要在这里就可见）",
  "找不到 meson --version —— libplacebo 的 meson.build 要到 clone 完四个仓库后才会报版本不够。",
);

// 5. python3-venv is what makes `python3 -m venv` work on a --no-install-recommends
//    install; without it the venv module is simply absent on 22.04.
assert(
  /python3-venv/.test(aptInstall),
  "apt 依赖列表含 python3-venv",
  `apt-get install 行里没有 python3-venv：\n${aptInstall}`,
);

// 6. Keep the stated reason honest. The comment block is the only place a
//    future maintainer learns *why* venv instead of the bypass switch, so a
//    comment that cites a different reason while the code does the right thing
//    is a real (if quieter) defect: it teaches the wrong lesson.
assert(
  /23\.0\.1/.test(commentText) && /22\.0\.2/.test(commentText),
  "注释记录了 pip 版本边界（>=23.0.1 才有开关，22.04 是 22.0.2）",
  "venv 方案的理由没有写明 pip 版本边界；后来者会再次去加那个开关。",
);

// 7. The glibc pin must survive the venv change: meson is pure Python, and the
//    script's whole reason for existing is not raising the host glibc. A venv
//    (or any future bootstrap step) that pulled in a newer libc or linked
//    extension would silently defeat the container.
assert(
  !/\b(libc6|libc-bin|libc-dev)\b/.test(aptInstall),
  "apt 依赖列表没有把 libc 拉高（容器的意义就是钉住 glibc 2.35）",
  `apt-get install 行里出现了 libc 相关包：\n${aptInstall}`,
);

// 8. The container image is the load-bearing constant — it is what guarantees
//    glibc 2.35. Pin it so a base-image bump cannot pass unnoticed.
assert(
  /ubuntu:22\.04/.test(source),
  "构建镜像仍是 ubuntu:22.04",
  "找不到 ubuntu:22.04 —— 换基础镜像会同时改动 glibc 门限和 pip 版本，\n" +
    "必须重跑本文件全部断言。",
);

// 9. Guard against the test itself rotting into a no-op: if the file were
//    emptied or the section renamed, every assertion above would still pass.
assert(commentLines(/meson/).length > 0, "测试读到的不是空脚本", "脚本内容异常。");

if (failures > 0) {
  process.stderr.write(`\nbuild-linux-libmpv-container.test.mjs: ${failures} 个断言失败\n`);
  process.exit(1);
}
process.stdout.write("\nbuild-linux-libmpv-container.test.mjs: 全部通过\n");
