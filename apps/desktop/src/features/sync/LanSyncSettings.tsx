import { useEffect, useState } from "react";

import type { DiscoveredHost, SyncPairingCode, SyncStatus } from "../../ipc/ipc-types.generated";
import { lanSyncAdapter, type LanSyncRole } from "./lanSyncAdapter";

function describeStatus(status: SyncStatus | null, loading: boolean): string {
  if (loading) return "正在读取同步服务状态…";
  if (!status) return "无法读取同步服务状态";
  if (status.lastError) return `同步服务错误：${syncErrorText(status.lastError)}`;
  switch (status.state) {
    case "disabled":
      return "已关闭";
    case "hosting":
      return "正在作为主机运行";
    case "connected":
      return "已连接";
    case "syncing":
      return "正在同步";
    case "paused":
      return "已暂停";
    case "failed":
      return "同步服务失败";
  }
}

export function LanSyncSettings() {
  const [status, setStatus] = useState<SyncStatus | null>(null);
  const [loading, setLoading] = useState(true);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [role, setRole] = useState<LanSyncRole>("peer");
  const [address, setAddress] = useState("");
  const [localAddresses, setLocalAddresses] = useState<readonly string[] | null>(null);
  const [hosts, setHosts] = useState<readonly DiscoveredHost[]>([]);
  const [pairingCodeState, setPairingCodeState] = useState<SyncPairingCode | null>(null);
  const [pairingCode, setPairingCode] = useState("");
  const [peerAddress, setPeerAddress] = useState("");
  const [copied, setCopied] = useState(false);

  useEffect(() => {
    let current = true;
    let latestSequence = -1;
    let unlisten: (() => void) | undefined;
    void lanSyncAdapter
      .subscribeStatus((next) => {
        if (!current || next.eventSequence < latestSequence) return;
        latestSequence = next.eventSequence;
        setStatus(next);
        setLoading(false);
      })
      .then((stop) => {
        if (current) unlisten = stop;
        else stop();
      })
      .catch((cause: unknown) => {
        if (current) setError(errorMessage(cause));
      });
    void lanSyncAdapter
      .status()
      .then((next) => {
        if (!current || next.eventSequence < latestSequence) return;
        latestSequence = next.eventSequence;
        setStatus(next);
        setLoading(false);
      })
      .catch((cause: unknown) => {
        if (current) {
          setError(errorMessage(cause));
          setLoading(false);
        }
      });
    return () => {
      current = false;
      unlisten?.();
    };
  }, []);

  useEffect(() => {
    let current = true;
    void lanSyncAdapter
      .localLanAddresses()
      .then((found) => {
        if (!current) return;
        const addresses = Array.isArray(found) ? found : [];
        setLocalAddresses(addresses);
        setAddress((selected) => selected.trim() || addresses[0] || "");
      })
      .catch(() => {
        if (!current) return;
        setLocalAddresses([]);
      });
    return () => {
      current = false;
    };
  }, []);

  async function run(action: () => Promise<SyncStatus>) {
    if (busy) return;
    setBusy(true);
    setError(null);
    try {
      setStatus(await action());
    } catch (cause) {
      setError(errorMessage(cause));
      // Read back native state because a failed command may still report a
      // terminal state transition through sync://status.
      void lanSyncAdapter
        .status()
        .then(setStatus)
        .catch(() => undefined);
    } finally {
      setBusy(false);
    }
  }

  async function discover() {
    setError(null);
    try {
      setHosts(await lanSyncAdapter.discoverHosts());
    } catch (cause) {
      setError(errorMessage(cause));
    }
  }

  async function runPairing(action: () => Promise<unknown>, afterSuccess?: () => void) {
    if (busy) return;
    setBusy(true);
    setError(null);
    try {
      await action();
      afterSuccess?.();
    } catch (cause) {
      setError(errorMessage(cause));
    } finally {
      setBusy(false);
    }
  }

  async function createPairingCode() {
    setCopied(false);
    await runPairing(async () => setPairingCodeState(await lanSyncAdapter.createPairingCode()));
  }

  async function copyPairingCode() {
    if (!pairingCodeState) return;
    try {
      await navigator.clipboard.writeText(pairingCodeState.code);
      setCopied(true);
    } catch (cause) {
      setError(`复制验证码失败：${errorMessage(cause)}`);
    }
  }

  async function submitPairingCode() {
    const code = pairingCode.trim();
    const host = peerAddress.trim();
    if (!code || !host) return;
    await runPairing(async () => {
      setStatus(await lanSyncAdapter.requestPairing(code, host));
      setPairingCode("");
      setPeerAddress("");
    });
  }

  function selectRole(nextRole: LanSyncRole) {
    setRole(nextRole);
    // Keep the temporary code only while this pairing flow is visible.
    setPairingCodeState(null);
    setPairingCode("");
    setPeerAddress("");
    setCopied(false);
  }

  const active = status?.enabled === true;
  const paused = status?.state === "paused";

  useEffect(() => {
    if (!active) {
      setPairingCodeState(null);
      setPairingCode("");
      setCopied(false);
    }
  }, [active]);

  return (
    <section className="settings-section" aria-labelledby="lan-sync-heading">
      <header className="settings-section-head">
        <h2 id="lan-sync-heading">局域网资料库同步</h2>
        <p>同步服务默认关闭。主机、发现和同步操作通过桌面端原生服务执行。</p>
      </header>
      <div className="setting-row">
        <div className="storage-value">
          <strong>同步状态</strong>
          <span className="setting-help" data-testid="lan-sync-state" aria-live="polite">
            {describeStatus(status, loading)}
          </span>
          {status?.address ? (
            <span className="setting-help">连接地址：{status.address}</span>
          ) : null}
          {status?.libraryId ? (
            <span className="setting-help">资料库：{status.libraryId}</span>
          ) : null}
        </div>
        <div className="setting-choices">
          <button type="button" className="btn" onClick={() => void discover()} disabled={busy}>
            搜索局域网主机
          </button>
          {active ? (
            <button
              type="button"
              className="btn"
              disabled={busy}
              onClick={() => void run(lanSyncAdapter.hostStop)}
            >
              {busy ? "正在处理…" : "停止同步"}
            </button>
          ) : null}
        </div>
      </div>
      {active && status?.role === "host" ? (
        <>
          <div className="setting-row">
            <div className="storage-value">
              <strong>配对新设备</strong>
              <span className="setting-help">
                生成一个 5 分钟有效的六位验证码，在加入设备上输入主机 IP 和验证码。
              </span>
            </div>
            <button
              type="button"
              className="btn btn-primary"
              onClick={() => void createPairingCode()}
              disabled={busy}
            >
              生成验证码
            </button>
          </div>
          {pairingCodeState ? (
            <div className="setting-row" aria-label="配对验证码">
              <div className="storage-value">
                <strong>本机配对验证码</strong>
                <code aria-label="主机配对验证码" className="setting-code">
                  {pairingCodeState.code}
                </code>
                <span className="setting-help">
                  {pairingCodeState.hostName} · 有效至{" "}
                  {new Date(pairingCodeState.expiresAtUnixMs).toLocaleString()}
                </span>
              </div>
              <button type="button" className="btn" onClick={() => void copyPairingCode()}>
                {copied ? "已复制" : "复制验证码"}
              </button>
              <button
                type="button"
                className="btn"
                onClick={() => {
                  setPairingCodeState(null);
                  setCopied(false);
                }}
              >
                隐藏验证码
              </button>
            </div>
          ) : null}
        </>
      ) : null}
      {active && status?.role === "peer" ? (
        <div className="setting-row">
          <div className="storage-value">
            <strong>从当前主机解绑</strong>
          </div>
          <button
            type="button"
            className="btn"
            disabled={busy}
            onClick={() => void run(lanSyncAdapter.hostStop)}
          >
            断开
          </button>
        </div>
      ) : null}
      <div className="setting-row">
        <div className="storage-value">
          <strong>已发现的主机</strong>
          <span className="setting-help">搜索失败时，可直接输入主机 IP。</span>
        </div>
        <ul aria-label="已发现的主机" data-testid="lan-sync-hosts">
          {hosts.length === 0 ? (
            <li>尚未发现主机</li>
          ) : (
            hosts.map((host) => (
              <li key={`${host.name}-${host.port}`}>
                {host.name} · {host.addresses.join(", ")}:{host.port}{" "}
                {host.addresses[0] ? (
                  <button
                    type="button"
                    className="btn"
                    disabled={busy}
                    aria-label={`使用主机 ${host.name} 的地址`}
                    onClick={() => {
                      const hostAddress = [...host.addresses].sort(
                        (left, right) => Number(left.includes(":")) - Number(right.includes(":")),
                      )[0];
                      setPeerAddress(hostAddress);
                      setRole("peer");
                    }}
                  >
                    使用此主机地址
                  </button>
                ) : null}
              </li>
            ))
          )}
        </ul>
      </div>
      {!active ? (
        <>
          <div className="setting-row">
            <div className="storage-value">
              <strong>启动同步服务</strong>
              <span className="setting-help">
                主机监听所选局域网 IP 的 50826 端口，其他设备通过 IP 和验证码加入。
              </span>
            </div>
            <div className="setting-choices" role="radiogroup" aria-label="同步角色">
              {(["host", "peer"] as const).map((nextRole) => (
                <label className="setting-choice" key={nextRole}>
                  <input
                    type="radio"
                    name="lan-sync-role"
                    checked={role === nextRole}
                    onChange={() => selectRole(nextRole)}
                  />
                  {nextRole === "host" ? "主机" : "加入设备"}
                </label>
              ))}
            </div>
          </div>
          {role === "host" ? (
            <div className="setting-row">
              <label className="storage-value" htmlFor="lan-sync-address">
                <strong>本机局域网 IP</strong>
                <span className="setting-help">只需输入局域网 IP；服务使用端口 50826。</span>
                <span className="setting-help" aria-live="polite">
                  {localAddresses === null
                    ? "正在识别本机局域网地址…"
                    : localAddresses.length > 0
                      ? `已自动识别 ${localAddresses.length} 个地址；可从输入建议中切换。`
                      : "未识别到局域网地址，请手动填写当前网络的 IP。"}
                </span>
              </label>
              <input
                id="lan-sync-address"
                list="lan-sync-local-addresses"
                value={address}
                onChange={(event) => setAddress(event.currentTarget.value)}
                placeholder="192.168.1.20"
              />
              <datalist id="lan-sync-local-addresses">
                {localAddresses?.map((candidate) => (
                  <option key={candidate} value={candidate} />
                ))}
              </datalist>
            </div>
          ) : null}
          {role === "host" ? (
            <div className="setting-row">
              <span className="storage-value">
                <strong>启动后可生成配对验证码</strong>
              </span>
              <button
                type="button"
                className="btn btn-primary"
                disabled={busy || !address.trim()}
                onClick={() => void run(() => lanSyncAdapter.hostStart(address.trim()))}
                data-testid="lan-sync-start"
              >
                {busy ? "正在启动…" : "启动主机"}
              </button>
            </div>
          ) : (
            <>
              <div className="setting-row">
                <label className="storage-value" htmlFor="lan-sync-peer-address">
                  <strong>主机 IP</strong>
                  <span className="setting-help">
                    输入主机显示的局域网 IP。连接成功后会交换并固定 TLS 身份材料。
                  </span>
                </label>
                <input
                  id="lan-sync-peer-address"
                  value={peerAddress}
                  onChange={(event) => setPeerAddress(event.currentTarget.value)}
                  placeholder="192.168.1.20"
                />
              </div>
              <div className="setting-row">
                <label className="storage-value" htmlFor="lan-sync-pairing-code">
                  <strong>六位验证码</strong>
                  <span className="setting-help">
                    设备名称会自动读取系统名称，验证码通过后直接连接主机。
                  </span>
                </label>
                <input
                  id="lan-sync-pairing-code"
                  value={pairingCode}
                  onChange={(event) =>
                    setPairingCode(event.currentTarget.value.replace(/\D/g, "").slice(0, 6))
                  }
                  inputMode="numeric"
                  maxLength={6}
                  autoComplete="one-time-code"
                  placeholder="123456"
                />
              </div>
              <div className="setting-row">
                <span className="storage-value">
                  <strong>加入主机</strong>
                </span>
                <button
                  type="button"
                  className="btn btn-primary"
                  disabled={busy || pairingCode.trim().length !== 6 || !peerAddress.trim()}
                  onClick={() => void submitPairingCode()}
                >
                  {busy ? "正在配对…" : "加入"}
                </button>
              </div>
            </>
          )}
        </>
      ) : (
        <div className="setting-row">
          <div className="storage-value">
            <strong>同步控制</strong>
          </div>
          <div className="setting-choices">
            <button
              type="button"
              className="btn"
              disabled={busy}
              onClick={() => void run(paused ? lanSyncAdapter.resume : lanSyncAdapter.pause)}
            >
              {paused ? "继续同步" : "暂停同步"}
            </button>
            <button
              type="button"
              className="btn btn-primary"
              disabled={busy || paused}
              onClick={() => void run(lanSyncAdapter.syncNow)}
              data-testid="lan-sync-now"
            >
              {busy ? "正在同步…" : "立即同步"}
            </button>
          </div>
        </div>
      )}
      {error ? (
        <p className="settings-note" role="alert">
          {error}
        </p>
      ) : null}
    </section>
  );
}

function errorMessage(cause: unknown): string {
  const key =
    cause instanceof Error
      ? cause.message
      : typeof cause === "string"
        ? cause
        : typeof cause === "object" && cause !== null
          ? "messageKey" in cause && typeof cause.messageKey === "string"
            ? cause.messageKey
            : "message_key" in cause && typeof cause.message_key === "string"
              ? cause.message_key
              : undefined
          : undefined;
  return key ? `同步操作失败：${syncErrorText(key)}` : "同步操作失败，未确认任何数据已传输。";
}

function syncErrorText(key: string): string {
  const messages: Record<string, string> = {
    "sync.unconfigured": "同步服务尚未完成安全配置。",
    "sync.invalidAddress": "请输入有效的局域网 IP 地址。",
    "sync.libraryUnavailable": "请先选择可用且可写的音乐目录，再启动同步。",
    "sync.busy": "同步服务正在处理其他操作。",
    "sync.bindFailed": "无法监听该局域网地址，请从自动识别的地址中选择当前网络地址。",
    "sync.discoveryFailed": "局域网主机发现失败。",
    "sync.pairingCodeInvalid":
      "无法加入主机，请检查 IP 和六位验证码是否正确、是否过期，以及主机是否正在运行。",
    "sync.workerFailed": "同步服务遇到错误。",
    "sync.pairingPending": "配对正在处理中，请稍后重试。",
  };
  return messages[key] ?? key;
}
