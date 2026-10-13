// Only local, explicit corrections exempt a historical erroneous claim.
// A nearby word such as "误差" or "不是" cannot suppress unrelated assertions.
const rules = [
  { pattern: /0\.60[0-9]*\s*dB/g, reason: "obsolete adjacent-cascade peak (+0.607 dB)" },
  { pattern: /(?:中心系数|center coefficient)/g, reason: "extrastereo c is a clipping switch" },
  { pattern: /30 ms 连续过渡/g, reason: "30 ms is per-tick interpolation, not a sample-continuous envelope" },
  { pattern: /`spatial:m`[^。；\n]{0,65}不受支持/g, reason: "spatial runtime updates were measured working" },
  { pattern: /3\.32\s*octave/g, reason: "obsolete cut width from an invalid bracket" },
];

function explicitlyCorrected(line, match) {
  // Restrict both sides to the same punctuation-delimited clause and the claim.
  const prefix = line.slice(0, match.index).split(/[。；;]/).at(-1);
  const suffix = line.slice(match.index + match[0].length).split(/[。；;]/)[0];
  if (/(?:不是|而非|并非)\s*\*{0,2}$/.test(prefix)) return true;
  if (/^.{0,18}(?:是误读|已撤回|已删除|已作废|应删除|实为)/.test(suffix)) return true;
  return /(?:历史|旧|原文|此前|曾写)[^。；;]{0,65}$/.test(prefix)
    && /(?:已撤回|已删除|已更正|错误范围|扫描范围错误|误读|不适用|应删除)/.test(suffix);
}

export function forbiddenClaims(document) {
  const failures = [];
  for (const [index, line] of document.split("\n").entries()) {
    for (const rule of rules) {
      for (const match of line.matchAll(rule.pattern)) {
        if (!explicitlyCorrected(line, match)) failures.push(`line ${index + 1}: ${rule.reason}: ${line.trim().slice(0, 180)}`);
      }
    }
  }
  return failures;
}
