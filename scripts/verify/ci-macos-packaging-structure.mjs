// A narrow contract shared by the governance check and focused injections.
const fullIf = "github.event_name != 'pull_request' || steps.macos-package-paths.outcome != 'success' || steps.macos-package-paths.outputs.skip != 'true'";
const skipIf = "github.event_name == 'pull_request' && steps.macos-package-paths.outcome == 'success' && steps.macos-package-paths.outputs.skip == 'true'";
const strictEmbedded = "node scripts/verify/checks/check-embedded-frontend.mjs --strict --bin target/aarch64-apple-darwin/release/bundle/macos/Echo.app/Contents/MacOS/Echo";
const exactRelease = "node scripts/verify/checks/verify-macos-release-dmg.mjs target/release/bundle/dmg apps/desktop/dist/index.html target/release/bundle/macos/echo-libmpv-report.md";

export function assertMacosPackaging(ci, release) {
  const platform = ci.jobs?.["macos-platform-gate"];
  const steps = platform?.steps ?? [];
  const named = (name) => steps.find((s) => s.name === name);
  const verify = (ok, message) => { if (!ok) throw new Error(message); };

  verify(platform?.name === "macOS platform Gate" && platform.if === undefined &&
    platform["continue-on-error"] === undefined, "macOS platform Gate must retain a stable unconditional job");
  const checkout = steps[0];
  verify(checkout?.uses === "actions/checkout@v4" && checkout.with?.["fetch-depth"] === 0 &&
    checkout.if === undefined, "macOS package classifier requires unconditional full-history checkout");
  const node = steps[1];
  verify(node?.uses === "actions/setup-node@v4" && node.with?.["node-version-file"] === ".nvmrc" &&
    node.if === undefined, "macOS package classifier must have verified pinned Node available");
  const classifier = named("Classify macOS packaging paths");
  verify(classifier?.id === "macos-package-paths" && classifier.if === "github.event_name == 'pull_request'" &&
    classifier["continue-on-error"] === true && classifier.run === "node scripts/verify/ci-macos-package-paths.mjs" &&
    classifier.env?.ECHO_PR_BASE_SHA === "${{ github.event.pull_request.base.sha }}" &&
    classifier.env?.ECHO_PR_HEAD_SHA === "${{ github.event.pull_request.head.sha }}",
    "macOS packaging requires the complete fail-open PR diff classifier");
  const note = named("Report macOS package skip");
  verify(note?.if === skipIf && /^echo "SKIP macOS packaging \(not built or verified\);/.test(note.run),
    "macOS packaging skip must be explicitly reported as not run");

  const order = [
    named("Install Rust toolchain (pinned)"),
    named("Cache Rust build artifacts"),
    named("Install pnpm"),
    named("Install dependencies"),
    // The duplicate frontend prep may only be removed with a real clean-dist proof.
    ...steps.filter((s) => s.name === "Build frontend for Tauri context"),
    named("Build and verify macOS platform Gate"),
    named("Verify embedded frontend freshness"),
    named("Upload libmpv Gate report"),
  ];
  verify(order.every((s) => s?.if === fullIf && s["continue-on-error"] === undefined),
    "macOS full package steps must run unless the successful PR classifier says to skip");
  const positions = [checkout, node, classifier, note, ...order].map((s) => steps.indexOf(s));
  verify(positions.every((position, idx) => idx === 0 || position > positions[idx - 1]),
    "macOS packaging check and artifact steps must be ordered after checkout and classification");
  const [build, embedded, report] = order.slice(-3);
  verify(build.run === "pnpm verify:task -- 1.9 1.10 9.7", "macOS packaging must run tasks 1.9, 1.10 and 9.7");
  verify(embedded.run === strictEmbedded, "macOS packaging must run strict embedded freshness on the built bundle");
  verify(report.uses === "actions/upload-artifact@v4" && report.with?.name === "macos-libmpv-gate-report" &&
    report.with?.path === "target/aarch64-apple-darwin/release/bundle/macos/echo-libmpv-report.md" &&
    report.with?.["if-no-files-found"] === "error", "macOS packaging must publish the real gate report");

  const releaseSteps = release.jobs?.build?.steps ?? [];
  const releaseGate = releaseSteps.find((s) => s.name === "Gate release bundles (macOS exact DMG)");
  const releaseBuild = releaseSteps.findIndex((s) => s.name === "Build release bundles");
  const upload = releaseSteps.findIndex((s) => s.name === "Upload platform artifacts");
  verify(releaseGate?.if === "runner.os == 'macOS'" && releaseGate.run === exactRelease &&
    releaseGate["continue-on-error"] === undefined && releaseBuild >= 0 &&
    releaseSteps.indexOf(releaseGate) > releaseBuild && upload > releaseSteps.indexOf(releaseGate),
    "macOS release must validate the exact DMG after build and before upload");
  const macRelease = release.jobs?.build?.strategy?.matrix?.include?.find((r) => r.platform === "macos");
  verify(macRelease?.bundle_dir === "target/release/bundle" && macRelease.artifacts === "dmg/*.dmg" &&
    releaseSteps[upload]?.with?.path === "${{ matrix.bundle_dir }}",
    "macOS release upload must carry that same verified DMG path");
}
