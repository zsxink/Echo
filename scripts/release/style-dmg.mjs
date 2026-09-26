#!/usr/bin/env node

// Tauri 打出的 DMG 需要两步收尾：
//   1) Finder 外观：背景图 + 窗口尺寸。只能借 Finder AppleScript（窗口状态住在 Finder 里）。
//   2) 布局归一化：图标坐标 + 排列方式。直接改 .DS_Store 字节 —— 确定性，且不需要任何自动化权限。
//
// ⚠️ 血泪（真机上发生过一次）：icon view 的 `arrangement` 一旦不是"无"（如"按名称"），
// Finder 会**完全忽略** .DS_Store 里存的 Iloc 坐标，把图标按名字重排到左上角 —— 两个图标
// 压在 ECHO 标题上、箭头指向空处。而 `set current view to icon view` 会把 icon view options
// 重置回系统默认（macOS 上默认就是"按名称"），所以 AppleScript 里"先设 arrangement 再切视图"
// 等于白设。create-dmg 自带的 template.applescript 是"先切视图、后设 arrangement"，别调换。
// 第 2 步存在的意义：让"位置对不对"不再取决于 Finder 的心情。
//
// 用法：
//   node scripts/release/style-dmg.mjs                     # 完整收尾（release 流程用）
//   node scripts/release/style-dmg.mjs --layout-only [dmg] # 只做第 2 步（不动 Finder，可修复已有产物）
//   node scripts/release/style-dmg.mjs --verify [dmg]      # 只读体检（不改文件，红了就是布局有问题）
//
// 环境变量：
//   ECHO_DMG_SKIP_LAYOUT_CHECK=1  把布局断言降级为警告（不推荐：错的 DMG 会静默发出去）
//   ECHO_DMG_SKIP_FINDER=1        跳过 Finder 外观改写（无 GUI / 未授权自动化时；布局归一化照做）

import { execFileSync } from "node:child_process";
import {
  existsSync,
  mkdtempSync,
  readFileSync,
  readdirSync,
  renameSync,
  rmSync,
  unlinkSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = join(dirname(fileURLToPath(import.meta.url)), "../..");
const DMG_DIR = join(ROOT, "target/release/bundle/dmg");
const TAURI_CONF = join(ROOT, "apps/desktop/src-tauri/tauri.conf.json");
const BACKGROUND_SOURCE = join(ROOT, "apps/desktop/src-tauri/dmg/background.png");
const STORE = ".DS_Store";
const SKIP_LAYOUT_CHECK = process.env.ECHO_DMG_SKIP_LAYOUT_CHECK === "1";
const FINDER_ICON_SIZE = 96;
const FINDER_TEXT_SIZE = 13;

function run(command, args, options = {}) {
  return execFileSync(command, args, { encoding: "utf8", stdio: ["ignore", "pipe", "pipe"], ...options });
}

function findDmg() {
  const candidates = readdirSync(DMG_DIR)
    .filter((name) => name.endsWith(".dmg") && !name.endsWith(".part.dmg"))
    .map((name) => join(DMG_DIR, name));
  if (candidates.length !== 1) {
    throw new Error(`expected one release DMG in ${DMG_DIR}, found ${candidates.length}`);
  }
  return candidates[0];
}

function mountedVolume(output) {
  const line = output
    .split("\n")
    .map((item) => item.trim())
    .find((item) => item.startsWith("/dev/") && item.includes("/Volumes/"));
  if (!line) throw new Error(`could not find mounted DMG in hdiutil output:\n${output}`);
  return line.slice(line.indexOf("/Volumes/"));
}

/** tauri.conf.json 是 DMG 布局的唯一真源，脚本不再抄一份坐标。 */
function layoutConfig() {
  const conf = JSON.parse(readFileSync(TAURI_CONF, "utf8"));
  const dmg = conf?.bundle?.macOS?.dmg ?? {};
  const { windowSize, appPosition, applicationFolderPosition } = dmg;
  if (!windowSize?.width || !windowSize?.height || !appPosition || !applicationFolderPosition) {
    throw new Error(
      `${TAURI_CONF} 缺少 bundle.macOS.dmg 的 windowSize / appPosition / applicationFolderPosition`,
    );
  }
  return {
    windowSize,
    appItem: `${conf.productName}.app`,
    folderItem: "Applications",
    appPosition,
    applicationFolderPosition,
    iconSize: FINDER_ICON_SIZE,
    textSize: FINDER_TEXT_SIZE,
  };
}

// ---------------------------------------------------------------- Finder 外观

function appleScript(volumePath, layout) {
  const volumeName = volumePath.slice("/Volumes/".length);
  const background = `${volumePath}/.background/background.png`;
  const bounds = `{100, 100, ${100 + layout.windowSize.width}, ${100 + layout.windowSize.height}}`;
  return `on run
  tell application "Finder"
    tell disk "${volumeName}"
      open
      -- 顺序不能改：先切到图标视图、让它建好 icon view options，再设 arrangement / 背景。
      -- 反过来 set current view 会把 arrangement 重置成系统默认（"按名称"）→ 图标坐标被忽略。
      tell container window
        set current view to icon view
        set toolbar visible to false
        set statusbar visible to false
        set the bounds to ${bounds}
      end tell
      delay 1
      tell icon view options of container window
        set arrangement to not arranged
        set icon size to ${layout.iconSize}
        set text size to ${layout.textSize}
        set background picture to POSIX file "${background}"
      end tell
      tell container window
        set position of item "${layout.appItem}" to {${layout.appPosition.x}, ${layout.appPosition.y}}
        set position of item "${layout.folderItem}" to {${layout.applicationFolderPosition.x}, ${layout.applicationFolderPosition.y}}
      end tell
      delay 1
      close
      open
      delay 2
      -- 重开窗口后再确认一次，并把窗口尺寸钉死
      tell container window
        set current view to icon view
        set toolbar visible to false
        set statusbar visible to false
        set the bounds to ${bounds}
      end tell
      tell icon view options of container window
        set arrangement to not arranged
        set icon size to ${layout.iconSize}
        set text size to ${layout.textSize}
      end tell
      delay 2
    end tell
    delay 2
  end tell
end run
`;
}

// ------------------------------------------------------------ .DS_Store 解析
//
// .DS_Store 是「记录串 + 偏移表」的私有格式：文件名用 UTF-16BE 存，前面 4 字节是字符数。
// 这里只认两条记录：item 的 Iloc（16 字节 = x, y, -1, -1）与容器的 icvp / bwsp（二进制 plist）。

function decodeUtf16Be(buf) {
  let out = "";
  for (let i = 0; i + 1 < buf.length; i += 2) out += String.fromCharCode(buf.readUInt16BE(i));
  return out;
}

/** 记录里的文件名紧贴在 code 之前：`<4B 字符数><UTF-16BE 名>`。 */
function recordNameAt(data, codeAt) {
  for (let chars = 1; chars <= 64; chars += 1) {
    const lengthAt = codeAt - 2 * chars - 4;
    if (lengthAt < 0) break;
    if (data.readUInt32BE(lengthAt) !== chars) continue;
    const name = decodeUtf16Be(data.subarray(lengthAt + 4, codeAt));
    if (name.length > 0 && /^[\x20-\x7e]+$/.test(name)) return name;
  }
  return null;
}

function readBlobRecord(data, code) {
  const codeAt = data.indexOf(code, 0, "latin1");
  if (codeAt < 0) return null;
  if (data.toString("latin1", codeAt + 4, codeAt + 8) !== "blob") return null;
  const payloadAt = codeAt + 12;
  const length = data.readUInt32BE(codeAt + 8);
  return { codeAt, payloadAt, payload: data.subarray(payloadAt, payloadAt + length) };
}

/** 名字 -> { x, y }，取自 16 字节的 Iloc blob。 */
function readIconLocations(data) {
  const found = new Map();
  let at = 0;
  for (;;) {
    at = data.indexOf("Iloc", at, "latin1");
    if (at < 0) return found;
    if (data.toString("latin1", at + 4, at + 8) === "blob" && data.readUInt32BE(at + 8) === 16) {
      const name = recordNameAt(data, at);
      if (name) found.set(name, { x: data.readUInt32BE(at + 12), y: data.readUInt32BE(at + 16), at });
    }
    at += 4;
  }
}

/** 二进制 plist（bplist00）只读解析；顺带给出顶层字典每个键的取值对象偏移，便于原地改写。 */
function parseBinaryPlist(buf) {
  const trailer = buf.subarray(buf.length - 32);
  const offsetIntSize = trailer[6];
  const objectRefSize = trailer[7];
  const numObjects = Number(trailer.readBigUInt64BE(8));
  const topObject = Number(trailer.readBigUInt64BE(16));
  const tableOffset = Number(trailer.readBigUInt64BE(24));
  // bplist 的整数可以是 1/2/4/8/16 字节（0x14 就是 16 字节），统一用 BigInt 读
  const integerAt = (i, size) => {
    let value = 0n;
    for (let k = 0; k < size; k += 1) value = (value << 8n) | BigInt(buf[i + k]);
    return value <= BigInt(Number.MAX_SAFE_INTEGER) ? Number(value) : value;
  };
  const offsets = Array.from({ length: numObjects }, (_, i) => integerAt(tableOffset + i * offsetIntSize, offsetIntSize));

  function lengthAt(i, info) {
    if (info < 0x0f) return { length: info, dataAt: i + 1 };
    const size = 1 << (buf[i + 1] & 0x0f);
    return { length: integerAt(i + 2, size), dataAt: i + 2 + size };
  }

  function readObject(index) {
    const i = offsets[index];
    const marker = buf[i];
    const type = marker >> 4;
    const info = marker & 0x0f;
    if (type === 0x0) return info === 0x08 ? false : info === 0x09 ? true : null;
    if (type === 0x1) return integerAt(i + 1, 1 << info);
    if (type === 0x2) return info === 0x02 ? buf.readFloatBE(i + 1) : buf.readDoubleBE(i + 1);
    if (type === 0x4) {
      const { length, dataAt } = lengthAt(i, info);
      return buf.subarray(dataAt, dataAt + length);
    }
    if (type === 0x5 || type === 0x6) {
      const { length, dataAt } = lengthAt(i, info);
      return type === 0x5
        ? buf.toString("latin1", dataAt, dataAt + length)
        : decodeUtf16Be(buf.subarray(dataAt, dataAt + length * 2));
    }
    if (type === 0xa) {
      const { length, dataAt } = lengthAt(i, info);
      return Array.from({ length }, (_, k) => readObject(integerAt(dataAt + k * objectRefSize, objectRefSize)));
    }
    if (type === 0xd) {
      const { length, dataAt } = lengthAt(i, info);
      const dict = {};
      for (let k = 0; k < length; k += 1) {
        const keyRef = integerAt(dataAt + k * objectRefSize, objectRefSize);
        const valueRef = integerAt(dataAt + (length + k) * objectRefSize, objectRefSize);
        const key = readObject(keyRef);
        if (index === topObject) valueOffsets.set(key, offsets[valueRef]);
        dict[key] = readObject(valueRef);
      }
      return dict;
    }
    throw new Error(`unsupported bplist marker 0x${marker.toString(16)}`);
  }

  const valueOffsets = new Map();
  const value = readObject(topObject);
  return { value, valueOffsets };
}

function readSettingsRecord(data, code) {
  const record = readBlobRecord(data, code);
  if (!record) return null;
  return { record, ...parseBinaryPlist(record.payload) };
}

// ------------------------------------------------------------------ 布局归一化

const arrangedHint =
  '"按名称/种类/日期…"排列时 Finder 会完全忽略图标坐标 —— 必须是 "none"（无）';

/** 把 arrangeBy 原地改写成 none（改长度会破坏 bplist，故要求等长）。 */
function patchArrangeBy(data, store) {
  const at = store.valueOffsets.get("arrangeBy");
  const current = store.value.arrangeBy;
  if (at === undefined) return { arranged: current ?? null, patched: false, skipped: "icvp 里没有 arrangeBy 键" };
  const marker = data[store.record.payloadAt + at];
  if (marker >> 4 !== 0x5 || (marker & 0x0f) !== current.length) {
    throw new Error(`icvp 里 arrangeBy 的编码不是 ASCII 短串（marker 0x${marker.toString(16)}），无法原地改写`);
  }
  if (current === "none") return { arranged: current, patched: false };
  if (current.length !== "none".length) {
    throw new Error(`arrangeBy=${JSON.stringify(current)} 与 "none" 长度不同，无法原地改写`);
  }
  writeBuffer(data, store.record.payloadAt + at + 1, Buffer.from("none", "latin1"));
  return { arranged: current, patched: true };
}

function describeArrange({ arranged, patched, skipped }) {
  if (patched) return `arrangeBy ${JSON.stringify(arranged)} -> "none"`;
  if (skipped) return `arrangeBy ${JSON.stringify(arranged)}（跳过：${skipped}）`;
  return `arrangeBy ${JSON.stringify(arranged)}（已是 none）`;
}

function writeBuffer(target, at, bytes) {
  for (let i = 0; i < bytes.length; i += 1) target[at + i] = bytes[i];
}

function readStoreOrThrow(mountPoint) {
  const file = join(mountPoint, STORE);
  if (!existsSync(file)) throw new Error(`${mountPoint} 里没有 ${STORE}（这个 DMG 没被 Finder 排过版？）`);
  return Buffer.from(readFileSync(file));
}

/** 归一化：排列方式必须是"无"，图标坐标必须等于 tauri.conf.json。 */
function normalizeStore(mountPoint, layout) {
  const data = readStoreOrThrow(mountPoint);
  const before = Buffer.from(data);
  const iconView = readSettingsRecord(data, "icvp");
  if (!iconView) throw new Error(`${STORE} 里找不到 icvp 记录（无法确认图标视图设置）`);

  const arrange = patchArrangeBy(data, iconView);

  const wanted = new Map([
    [layout.appItem, layout.appPosition],
    [layout.folderItem, layout.applicationFolderPosition],
  ]);
  const locations = readIconLocations(data);
  const moved = [];
  for (const [name, want] of wanted) {
    const got = locations.get(name);
    if (!got) continue;
    if (got.x !== want.x || got.y !== want.y) {
      patchIconLocation(data, got.at, want.x, want.y);
      moved.push(`${name} ${got.x},${got.y} -> ${want.x},${want.y}`);
    }
  }

  if (!before.equals(data)) writeFileSync(join(mountPoint, STORE), data);
  return { arrange, moved, missing: [...wanted.keys()].filter((name) => !locations.has(name)) };
}

function patchIconLocation(data, at, x, y) {
  data.writeUInt32BE(x, at + 12);
  data.writeUInt32BE(y, at + 16);
}

/** 读回最终产物：这是发出去的那个文件，不是中间态。 */
function auditStore(mountPoint, layout) {
  const data = readStoreOrThrow(mountPoint);
  const problems = [];
  const warnings = [];

  const iconView = readSettingsRecord(data, "icvp");
  if (!iconView) problems.push(`缺少 icvp 记录`);
  else {
    const arrangeBy = iconView.value.arrangeBy;
    if (arrangeBy === undefined) warnings.push(`icvp 未写 arrangeBy，无法确认排列方式（无法断言 Finder 是否采纳图标坐标）`);
    else if (arrangeBy !== "none") problems.push(`arrangeBy=${JSON.stringify(arrangeBy)}；${arrangedHint}`);
    if (iconView.value.backgroundType !== 2) {
      problems.push(`backgroundType=${JSON.stringify(iconView.value.backgroundType)}；应为图片背景 2`);
    }
    if (!iconView.value.backgroundImageAlias) problems.push(`缺少 backgroundImageAlias`);
    if (iconView.value.iconSize !== layout.iconSize) {
      problems.push(`iconSize=${JSON.stringify(iconView.value.iconSize)} ≠ ${layout.iconSize}`);
    }
    if (iconView.value.textSize !== layout.textSize) {
      problems.push(`textSize=${JSON.stringify(iconView.value.textSize)} ≠ ${layout.textSize}`);
    }
  }

  const background = join(mountPoint, ".background", "background.png");
  if (!existsSync(background)) problems.push(`缺少 .background/background.png`);
  else if (!readFileSync(background).equals(readFileSync(BACKGROUND_SOURCE))) {
    problems.push(`.background/background.png 与源背景图不一致`);
  }

  const locations = readIconLocations(data);
  for (const [name, want] of [
    [layout.appItem, layout.appPosition],
    [layout.folderItem, layout.applicationFolderPosition],
  ]) {
    const got = locations.get(name);
    if (!got) problems.push(`缺少 ${name} 的图标坐标`);
    else if (got.x !== want.x || got.y !== want.y) problems.push(`${name} 坐标 ${got.x},${got.y} ≠ ${want.x},${want.y}`);
  }

  const windowSettings = readSettingsRecord(data, "bwsp");
  const bounds = windowSettings?.value?.WindowBounds;
  if (typeof bounds === "string" && !bounds.includes(`${layout.windowSize.width}, ${layout.windowSize.height}`)) {
    warnings.push(`窗口尺寸 ${bounds} 与 tauri.conf.json 的 ${layout.windowSize.width}x${layout.windowSize.height} 不一致`);
  }
  return { problems, warnings };
}

function reportAudit(audit) {
  for (const warning of audit.warnings) console.warn(`DMG 布局提示：${warning}`);
  if (audit.problems.length === 0) return;
  const message = `DMG 布局校验失败：\n  - ${audit.problems.join("\n  - ")}`;
  if (!SKIP_LAYOUT_CHECK) throw new Error(message);
  console.warn(`${message}\n（ECHO_DMG_SKIP_LAYOUT_CHECK=1，已降级为警告）`);
}

// -------------------------------------------------------------------- 挂载辅助

function withImage(image, { readOnly = false, mountPoint } = {}, fn) {
  const mount = mountPoint ?? mkdtempSync(join(tmpdir(), "echo-dmg-mnt-"));
  const borrowed = Boolean(mountPoint);
  try {
    run("hdiutil", ["attach", readOnly ? "-readonly" : "-readwrite", "-nobrowse", "-mountpoint", mount, image]);
  } catch (error) {
    const detail = String(error.stderr ?? error.message).trim();
    throw new Error(`挂载 ${image} 失败：${detail}\n（提示：这个镜像可能还挂载着，先把它推出再重试）`);
  }
  try {
    return fn(mount);
  } finally {
    try {
      run("hdiutil", ["detach", mount], { stdio: "ignore" });
    } catch {
      // 保留原始错误；卷可以事后手动推出
    }
    if (!borrowed) rmSync(mount, { recursive: true, force: true });
  }
}

function toFormat(source, format, target) {
  if (existsSync(target)) unlinkSync(target);
  // Pipe (not stdio:"ignore") so a failure carries hdiutil's own reason. The
  // UDZO step is load-bearing — it is what produces the shipped image — and it
  // failed on a macOS runner with the message swallowed, leaving nothing but
  // "Command failed" in the log and no way to tell a busy image from a full
  // disk from a bad format. The detach/cleanup calls stay on stdio:"ignore":
  // they are best-effort and their output is not worth failing over.
  try {
    run("hdiutil", ["convert", source, "-format", format, "-o", target]);
  } catch (error) {
    const detail = String(error.stderr ?? error.message).trim();
    throw new Error(
      `hdiutil convert ${source} -> ${format} 失败：${detail}\n` +
      `（提示：源镜像若仍挂载着会转换失败，先 hdiutil info 确认没有卷指向 ${source}）`,
    );
  }
}

// ------------------------------------------------------------------------- 主流程

function main() {
  if (process.platform !== "darwin") return;
  const layout = layoutConfig();
  const dmg = findDmg();
  const workDir = mkdtempSync(join(tmpdir(), "echo-dmg-style-"));
  const rwDmg = join(workDir, "rw.dmg");
  const styledDmg = join(workDir, "styled.dmg");
  const script = join(workDir, "style.applescript");
  let volumePath;

  try {
    toFormat(dmg, "UDRW", rwDmg);

    // 1) Finder 外观：背景图 + 窗口尺寸（只有 Finder 能改这些）
    if (process.env.ECHO_DMG_SKIP_FINDER === "1") {
      console.log("ECHO_DMG_SKIP_FINDER=1：跳过 Finder 外观改写（背景图/窗口尺寸沿用 Tauri 产物）");
    } else {
      volumePath = mountedVolume(run("hdiutil", ["attach", "-readwrite", "-nobrowse", rwDmg]));
      writeFileSync(script, appleScript(volumePath, layout));
      run("osascript", [script]);
      try {
        run("hdiutil", ["detach", volumePath], { stdio: "ignore" });
      } catch {
        // Finder 的 close/reopen 可能把卷短暂解锁或已推出；detach 只是清理性动作，
        // 第 2 步会用独立 mountpoint 重新挂载 rwDmg，这里失败不影响产物。
      }
      volumePath = undefined;
    }

    // 2) 布局归一化：Finder 关掉之后再改 .DS_Store，确保它是最后一个写入者
    const report = withImage(rwDmg, {}, (mount) => normalizeStore(mount, layout));
    console.log(
      `DMG 布局：${describeArrange(report.arrange)}` +
        (report.moved.length > 0 ? `; 图标 ${report.moved.join("; ")}` : "; 图标坐标未变"),
    );
    if (report.missing.length > 0) console.warn(`DMG 布局提示：.DS_Store 里没有这些条目的坐标：${report.missing.join(", ")}`);

    // 3) 压回只读镜像，并对着最终文件读回断言
    toFormat(rwDmg, "UDZO", styledDmg);
    reportAudit(withImage(styledDmg, { readOnly: true }, (mount) => auditStore(mount, layout)));

    const backup = `${dmg}.unstyled`;
    if (existsSync(backup)) unlinkSync(backup);
    renameSync(dmg, backup);
    renameSync(styledDmg, dmg);
    unlinkSync(backup);
    console.log(`styled DMG: ${dmg}`);
  } finally {
    if (volumePath) {
      try {
        run("hdiutil", ["detach", volumePath], { stdio: "ignore" });
      } catch {
        // 保留原始错误；临时卷可以手动推出
      }
    }
    rmSync(workDir, { recursive: true, force: true });
  }
}

/** 只做布局归一化：不碰 Finder，因此不需要自动化权限，可在任意机器上修复已有产物。 */
function layoutOnly(target) {
  if (process.platform !== "darwin") return;
  const layout = layoutConfig();
  const dmg = target ? resolve(process.cwd(), target) : findDmg();
  if (!existsSync(dmg)) throw new Error(`找不到 DMG：${dmg}`);
  const workDir = mkdtempSync(join(tmpdir(), "echo-dmg-layout-"));
  const rwDmg = join(workDir, "rw.dmg");
  const fixedDmg = join(workDir, "fixed.dmg");
  try {
    toFormat(dmg, "UDRW", rwDmg);
    const report = withImage(rwDmg, {}, (mount) => normalizeStore(mount, layout));
    toFormat(rwDmg, "UDZO", fixedDmg);
    reportAudit(withImage(fixedDmg, { readOnly: true }, (mount) => auditStore(mount, layout)));
    renameSync(fixedDmg, dmg);
    console.log(
      `DMG 布局已归一化：${dmg}\n  ${describeArrange(report.arrange)}` +
        (report.moved.length > 0 ? `\n  图标 ${report.moved.join("\n  图标 ")}` : "\n  图标坐标未变"),
    );
  } finally {
    rmSync(workDir, { recursive: true, force: true });
  }
}

/** 只读校验：不做任何修改，用来给任意产物做体检（也是这条断言"会红"的证明入口）。 */
function verifyOnly(target) {
  if (process.platform !== "darwin") return;
  const layout = layoutConfig();
  const dmg = resolve(process.cwd(), target ?? findDmg());
  if (!existsSync(dmg)) throw new Error(`找不到 DMG：${dmg}`);
  const audit = withImage(dmg, { readOnly: true }, (mount) => auditStore(mount, layout));
  reportAudit(audit);
  if (audit.problems.length === 0) console.log(`DMG 布局校验通过：${dmg}`);
}

const [subcommand, target] = process.argv.slice(2);
if (subcommand === "--layout-only") layoutOnly(target);
else if (subcommand === "--verify") verifyOnly(target);
else main();
