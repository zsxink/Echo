import { useEffect, useState } from "react";

import type {
  DiscoveredHost,
  SyncInvitation,
  SyncPairingReceipt,
  SyncPairingRequest,
  SyncStatus,
} from "../../ipc/ipc-types.generated";
import { lanSyncAdapter, type LanSyncRole } from "./lanSyncAdapter";

function describeStatus(status: SyncStatus | null, loading: boolean): string {
  if (loading) return "正在读取同步服务状态…";
  if (!status) return "无法读取同步服务状态";
  if (status.lastError) return `同步服务错误：${status.lastError}`;
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
  const [hosts, setHosts] = useState<readonly DiscoveredHost[]>([]);
  const [invitation, setInvitation] = useState<SyncInvitation | null>(null);
  const [pairingRequests, setPairingRequests] = useState<readonly SyncPairingRequest[]>([]);
  const [pastedInvitation, setPastedInvitation] = useState("");
  const [pairingInvitationJson, setPairingInvitationJson] = useState<string | null>(null);
  const [deviceName, setDeviceName] = useState("");
  const [pairingReceipt, setPairingReceipt] = useState<SyncPairingReceipt | null>(null);
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

  async function refreshPairingRequests() {
    await runPairing(async () => setPairingRequests(await lanSyncAdapter.listPairingRequests()));
  }

  async function createInvitation() {
    setCopied(false);
    await runPairing(async () => setInvitation(await lanSyncAdapter.createInvitation()));
  }

  async function copyInvitation() {
    if (!invitation) return;
    try {
      await navigator.clipboard.writeText(invitation.invitation);
      setCopied(true);
    } catch (cause) {
      setError(`复制邀请失败：${errorMessage(cause)}`);
    }
  }

  async function submitInvitation() {
    const token = pastedInvitation.trim();
    const name = deviceName.trim();
    if (!token || !name) return;
    await runPairing(async () => {
      const receipt = await lanSyncAdapter.requestPairing(token, name);
      setPairingReceipt(receipt);
      setPairingInvitationJson(token);
      setPastedInvitation("");
    });
  }

  async function claimPairing() {
    if (!pairingReceipt || !pairingInvitationJson) return;
    await runPairing(async () => {
      const next = await lanSyncAdapter.claimPairing(
        pairingReceipt.requestId,
        pairingReceipt.receipt,
        pairingInvitationJson,
      );
      setStatus(next);
      setPairingReceipt(null);
      setPairingInvitationJson(null);
    });
  }

  async function decidePairing(requestId: string, approve: boolean) {
    await runPairing(async () => {
      if (approve) await lanSyncAdapter.approvePairing(requestId);
      else await lanSyncAdapter.rejectPairing(requestId);
      setPairingRequests(await lanSyncAdapter.listPairingRequests());
    });
  }

  function selectRole(nextRole: LanSyncRole) {
    setRole(nextRole);
    // Pairing bearer material lives only for the deliberate flow currently
    // shown. Leaving the flow clears its local copy.
    setInvitation(null);
    setPairingReceipt(null);
    setPastedInvitation("");
    setPairingInvitationJson(null);
    setCopied(false);
  }

  const active = status?.enabled === true;
  const paused = status?.state === "paused";

  useEffect(() => {
    if (!active) {
      setInvitation(null);
      setPairingReceipt(null);
      setPastedInvitation("");
      setPairingInvitationJson(null);
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
                邀请是短时凭据。仅在此处生成后展示，复制给要加入的设备。
              </span>
            </div>
            <button
              type="button"
              className="btn btn-primary"
              onClick={() => void createInvitation()}
              disabled={busy}
            >
              生成主机邀请
            </button>
          </div>
          {invitation ? (
            <div className="setting-row" aria-label="主机邀请">
              <div className="storage-value">
                <strong>主机邀请</strong>
                <textarea
                  aria-label="主机邀请内容"
                  readOnly
                  value={invitation.invitation}
                  rows={3}
                />
                <span className="setting-help">
                  {invitation.hostName} · 有效至{" "}
                  {new Date(invitation.expiresAtUnixMs).toLocaleString()}
                </span>
              </div>
              <button type="button" className="btn" onClick={() => void copyInvitation()}>
                {copied ? "已复制" : "复制邀请"}
              </button>
              <button
                type="button"
                className="btn"
                onClick={() => {
                  setInvitation(null);
                  setCopied(false);
                }}
              >
                隐藏邀请
              </button>
            </div>
          ) : null}
          <div className="setting-row">
            <div className="storage-value">
              <strong>待审批设备</strong>
              <span className="setting-help">只有批准后，设备才能领取配对凭据并连接此主机。</span>
            </div>
            <button
              type="button"
              className="btn"
              onClick={() => void refreshPairingRequests()}
              disabled={busy}
            >
              刷新请求
            </button>
          </div>
          <ul aria-label="待审批设备">
            {pairingRequests.length === 0 ? (
              <li>没有待审批请求</li>
            ) : (
              pairingRequests.map((request) => (
                <li key={request.requestId}>
                  <span>
                    {request.deviceName} · {request.deviceId}
                  </span>
                  <button
                    type="button"
                    className="btn"
                    disabled={busy}
                    onClick={() => void decidePairing(request.requestId, true)}
                  >
                    批准
                  </button>
                  <button
                    type="button"
                    className="btn"
                    disabled={busy}
                    onClick={() => void decidePairing(request.requestId, false)}
                  >
                    拒绝
                  </button>
                </li>
              ))
            )}
          </ul>
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
          <span className="setting-help">发现设备不会自动配对或传输资料库数据。</span>
        </div>
        <ul aria-label="已发现的主机" data-testid="lan-sync-hosts">
          {hosts.length === 0 ? (
            <li>尚未发现主机</li>
          ) : (
            hosts.map((host) => (
              <li key={`${host.name}-${host.port}`}>
                {host.name} · {host.addresses.join(", ")}:{host.port}
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
                主机需要本机局域网 IP。加入设备需要原生端已配置可信配对。
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
                <span className="setting-help">例如 192.168.1.20</span>
              </label>
              <input
                id="lan-sync-address"
                value={address}
                onChange={(event) => setAddress(event.currentTarget.value)}
                placeholder="192.168.1.20"
              />
            </div>
          ) : null}
          {role === "host" ? (
            <div className="setting-row">
              <span className="storage-value">
                <strong>启动后可创建邀请</strong>
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
                <label className="storage-value" htmlFor="lan-sync-invitation">
                  <strong>粘贴主机邀请</strong>
                  <span className="setting-help">
                    邀请只在本次配对步骤中使用，不会保存在偏好设置。
                  </span>
                </label>
                <textarea
                  id="lan-sync-invitation"
                  value={pastedInvitation}
                  onChange={(event) => setPastedInvitation(event.currentTarget.value)}
                  rows={3}
                />
              </div>
              <div className="setting-row">
                <label className="storage-value" htmlFor="lan-sync-device-name">
                  <strong>此设备名称</strong>
                </label>
                <input
                  id="lan-sync-device-name"
                  value={deviceName}
                  onChange={(event) => setDeviceName(event.currentTarget.value)}
                  placeholder="例如：客厅电脑"
                />
              </div>
              <div className="setting-row">
                <span className="storage-value">
                  <strong>发送配对请求</strong>
                </span>
                <button
                  type="button"
                  className="btn btn-primary"
                  disabled={busy || !pastedInvitation.trim() || !deviceName.trim()}
                  onClick={() => void submitInvitation()}
                >
                  {busy ? "正在发送…" : "请求加入"}
                </button>
              </div>
              {pairingReceipt ? (
                <div className="setting-row" aria-label="等待主机批准">
                  <div className="storage-value">
                    <strong>等待主机批准</strong>
                    <span className="setting-help">
                      请求已提交。此设备暂存一次性领取凭据；批准后在此处领取连接。
                    </span>
                    <span className="setting-help">请求编号：{pairingReceipt.requestId}</span>
                  </div>
                  <button
                    type="button"
                    className="btn btn-primary"
                    disabled={busy}
                    onClick={() => void claimPairing()}
                  >
                    {busy ? "正在检查…" : "检查并领取连接"}
                  </button>
                </div>
              ) : null}
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
  if (cause instanceof Error) {
    const key = cause.message;
    const messages: Record<string, string> = {
      "sync.unconfigured": "同步服务尚未完成安全配置，未启动连接或传输数据。",
      "sync.invalidAddress": "请输入有效的局域网 IP 地址。",
      "sync.libraryUnavailable": "请先选择可用的音乐目录。",
      "sync.busy": "同步服务正在处理其他操作。",
      "sync.bindFailed": "无法监听该局域网地址。",
      "sync.discoveryFailed": "局域网主机发现失败。",
      "sync.workerFailed": "同步服务遇到错误。",
      "sync.pairingPending": "主机尚未批准请求；领取凭据仍保留在本机当前配对步骤中。",
      "sync.invitationExpired": "邀请已过期，请让主机重新生成邀请。",
      "sync.pairingRejected": "主机拒绝了此设备的配对请求。",
      "sync.receiptExpired": "领取凭据已过期，请重新提交主机邀请。",
    };
    return messages[key] ?? `同步操作失败：${key}`;
  }
  return "同步操作失败，未确认任何数据已传输。";
}
