#!/usr/bin/env node
// macOS 默认音频播放器设置工具
//
// 把 Echo（bundle id com.zsxink.echo）设为 mp3/flac/m4a/ogg/opus/wav/ape 这些
// 系统音频格式的默认打开应用。
//
// 为什么需要它：macOS 把"某类文件的默认打开方式"记录在 LaunchServices
// 里，GUI（Finder 简介 → 打开方式 → 始终使用）在某些情况下写入后并不会
// 立即被系统解析接受（有传播延迟，且第三方 app 常被旧 App 覆盖）。本脚本
// 通过 LSSetDefaultRoleHandlerForContentType 逐 UTI 直写，可绕过 GUI 的
// 静默失败，并回读验证是否真的生效。
//
// 用法:
//   node scripts/macos-default-player.mjs set     # 把六种音频格式设为 Echo
//   node scripts/macos-default-player.mjs verify   # 回读当前默认并报告
//   node scripts/macos-default-player.mjs all      # set 后 verify（默认）
//
// 依赖: macOS 自带 swift（调用 CoreServices）; 无第三方包。
// 注意: LaunchServices 有传播延迟，刚 set 完可能读回旧值，稍等数秒即更新。

import { spawnSync } from "node:child_process";
import { writeFileSync, existsSync } from "node:fs";

const BUNDLE_ID = "com.zsxink.echo"; // Echo 的 CFBundleIdentifier（tauri.conf.json 的 identifier）
const APP_PATH = "/Applications/Echo.app";
const FORMATS = [
  { ext: "mp3", uti: "public.mp3" },
  { ext: "flac", uti: "org.xiph.flac" },
  { ext: "m4a", uti: "com.apple.m4a-audio" },
  { ext: "ogg", uti: "org.xiph.ogg-audio" },
  { ext: "opus", uti: "org.xiph.ogg-audio" },
  { ext: "wav", uti: "com.microsoft.waveform-audio" },
  { ext: "ape", uti: "org.monkeysaudio.ape" },
];

if (process.platform !== "darwin") {
  console.error("macOS-only: LaunchServices 是 macOS 机制。");
  process.exit(1);
}
if (!existsSync(APP_PATH)) {
  console.error(`找不到 ${APP_PATH}——请先把 Echo.app 放入 /Applications。`);
  process.exit(1);
}

// 把一段 Swift 源码写到临时文件并编译执行，参数透传。
function runSwift(swiftSource, args) {
  const tmp = "/tmp/echo-macos-default-player.swift";
  writeFileSync(tmp, swiftSource);
  return spawnSync("swift", [tmp, ...args], { encoding: "utf8" });
}

const SET_SWIFT = `
import CoreServices
import Foundation
let args = CommandLine.arguments
guard args.count >= 3 else { exit(2) }
let uti = args[1], bundleID = args[2]
let r = LSSetDefaultRoleHandlerForContentType(uti as CFString, .all, bundleID as CFString)
print(r == 0 ? "OK" : "ERR \\(r)")
`;

const QUERY_SWIFT = `
import CoreServices
import Foundation
let args = CommandLine.arguments
guard args.count > 1 else { exit(2) }
let url = URL(fileURLWithPath: args[1])
var outErr: Unmanaged<CFError>?
if let app = LSCopyDefaultApplicationURLForURL(url as CFURL, .all, &outErr)?.takeRetainedValue() {
    print((app as NSURL).path ?? "?")
} else {
    print("ERROR")
    exit(1)
}
`;

function setDefault(uti) {
  const r = runSwift(SET_SWIFT, [uti, BUNDLE_ID]);
  return r.status === 0 && r.stdout.includes("OK");
}

// LSCopyDefaultApplicationURLForURL 需要真文件，用临时文件按扩展名询问。
function queryDefault(ext) {
  const probe = `/tmp/echo-default-probe.${ext}`;
  writeFileSync(probe, "x", { flag: "w" });
  const r = runSwift(QUERY_SWIFT, [probe]);
  return r.status === 0 ? r.stdout.trim() : null;
}

function setAll() {
  console.log(`把以下格式的默认打开应用设为 Echo (${BUNDLE_ID}):`);
  let ok = true;
  for (const { ext, uti } of FORMATS) {
    const success = setDefault(uti);
    console.log(`  ${success ? "✓" : "✗"} ${ext} → ${uti}`);
    ok = ok && success;
  }
  return ok;
}

function verifyAll() {
  console.log("回读当前系统默认打开应用:");
  let ok = true;
  for (const { ext } of FORMATS) {
    const current = queryDefault(ext);
    const isEcho = current === APP_PATH;
    console.log(`  ${isEcho ? "✓" : "✗"} ${ext} → ${current ?? "(查询失败)"}`);
    ok = ok && isEcho;
  }
  return ok;
}

const [, , cmd = "all"] = process.argv;
let ok;
switch (cmd) {
  case "set":
    ok = setAll();
    break;
  case "verify":
    ok = verifyAll();
    break;
  case "all":
  default:
    ok = setAll() && verifyAll();
    break;
}
console.log(ok ? "全部完成。" : "有失败项，请查看上方输出。");
process.exit(ok ? 0 : 1);
