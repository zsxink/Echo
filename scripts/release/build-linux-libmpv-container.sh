#!/usr/bin/env bash
# 在 Ubuntu 22.04 容器里构建 Linux libmpv。
#
# 为什么必须是容器：GLIBC_ 符号版本是**构建宿主机 glibc 头文件**的属性，不是被编译代码的属性。
# 在 Ubuntu 24.04（glibc 2.39）上编出来的 libmpv 会引用 GLIBC_2.38 之类的符号，而 Echo 承诺
# 支持 glibc 2.35+（Ubuntu 22.04 / 24.04）。那样的库在 22.04 上直接 `GLIBC_2.38 not found` 起不来。
# mpv/FFmpeg 的 configure 没有"目标 glibc 版本"这类开关（autotools 没有，meson 也没有），
# 所以唯一能把符号版本压下来的办法就是换一个 glibc 2.35 的构建环境。
# openspec design.md（introduce-windows-linux-libmpv）也是这么定的：构建容器固定 22.04。
#
# 为什么不把整个 job 换成 ubuntu-22.04 runner：Tauri 的 webkit2gtk-4.1-dev 在 22.04 源里没有，
# 换 runner 会让桌面本体的编译先挂掉。只有 libmpv 这一步需要 2.35 宿主，所以只把它关进容器。
#
# 真正的门限校验仍在 build-linux-libmpv.mjs 的 checkGlibc() 里（它对着真实产物跑 readelf）。
# 本脚本只是把"宿主 glibc 偏高"这个根因消掉；门限被抬高时门限检查照样会红。
set -euo pipefail

IMAGE="${ECHO_LIBMPV_BUILD_IMAGE:-ubuntu:22.04}"

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"

if [ $# -lt 1 ]; then
  echo "usage: build-linux-libmpv-container.sh <repo-relative-outdir> [workdir]" >&2
  exit 2
fi
OUT_REL="$1"
WORK_DIR="${2:-${RUNNER_TEMP:-$REPO_ROOT}/echo-libmpv-build}"

# docker 的 bind mount 会自动创建源目录，但默认属主是 root；22.04 的容器以 root
# 跑，留下的产物会污染 runner 上的工作树。预先建好并交给调用方，避免后面 mv
# 的时候才发现权限不对。
mkdir -p "${WORK_DIR}"

if ! command -v docker >/dev/null 2>&1; then
  echo "build-linux-libmpv-container.sh: 需要 docker。" >&2
  echo "直接跑 build-linux-libmpv.mjs 也能编，但产物会带上宿主 glibc 的符号版本，" >&2
  echo "而 checkGlibc() 会按 2.35 门限拒绝它（见 design.md：构建容器固定 Ubuntu 22.04）。" >&2
  exit 1
fi

# Node 版本从 .nvmrc 推导而不是写死：.nvmrc 是 "22" 这样的主版本范围，写死一个
# 具体小版本只会在下次升级 CI Node 时悄悄落后于 setup-node 装的那一个。
# 解析用宿主 node（CI 里 actions/setup-node 已经装好）；宿主没有 node 时不猜，
# 直接失败，而不是默默用一个可能对不上的版本去编。
NODE_MAJOR="$(tr -dc '0-9' < "$REPO_ROOT/.nvmrc")"
if ! command -v node >/dev/null 2>&1; then
  echo "build-linux-libmpv-container.sh: 解析 Node 版本需要宿主 node（当前没有）。" >&2
  echo "装好 node，或用 ECHO_LIBMPV_NODE_VERSION=vX.Y.Z 显式指定。" >&2
  exit 1
fi
NODE_VERSION="${ECHO_LIBMPV_NODE_VERSION:-$(curl -fsSL https://nodejs.org/dist/index.json \
  | node -e 'let s="";process.stdin.on("data",d=>s+=d).on("end",()=>{const major=process.argv[1];const hit=JSON.parse(s).find(r=>r.version.startsWith("v"+major+"."));if(!hit)process.exit(1);console.log(hit.version)})' \
  "$NODE_MAJOR")}"
if [ -z "${NODE_VERSION}" ]; then
  echo "build-linux-libmpv-container.sh: 没能从 .nvmrc（${NODE_MAJOR}）解析出 Node 版本。" >&2
  exit 1
fi

echo "==> 在 ${IMAGE} 中构建 Linux libmpv（Node ${NODE_VERSION}，宿主 glibc 无关）"

# 依赖列表与 release.yml 的 "Install Linux libmpv build dependencies" 保持一致，
# 外加 git / curl / ca-certificates（取 Node 二进制）、xz-utils（解包）、
# python3-pip（装 meson，见下）。
#
# meson 刻意**不从 apt 装**：22.04 只有 0.61.2，而 libplacebo 要求 >= 1.3.0
# （首次容器构建实测：`Meson version is 0.61.2 but project requires >=1.3.0`）。
# meson 是纯 Python、不链接 glibc，所以从 PyPI 装新版既满足版本下限、
# 又不会把构建宿主的 glibc 抬上去——而 glibc 版本正是这里唯一真正要钉死的东西。
# 具体装法（venv 而非系统 Python）见下方 pip 处的说明。
docker run --rm \
  --volume "${REPO_ROOT}:/src" \
  --volume "${WORK_DIR}:/work" \
  --workdir /src \
  "${IMAGE}" \
  bash -euxo pipefail -c "
    export DEBIAN_FRONTEND=noninteractive
    apt-get update
    apt-get install -y --no-install-recommends \
      build-essential git pkg-config python3 python3-setuptools python3-pip python3-venv \
      ninja-build yasm nasm autoconf automake libtool libtool-bin \
      libfreetype-dev libfontconfig1-dev libharfbuzz-dev libfribidi-dev \
      zlib1g-dev libunistring-dev libbz2-dev glslang-tools \
      patchelf curl ca-certificates xz-utils

    # 装新版 meson 覆盖 apt 的 0.61.2，并让脚本早失败：真正报错要等到
    # libplacebo 的 meson.build 才看得见，那时已经 clone 完四个仓库了。
    #
    # 装进 venv 而不是往系统 Python 里装，有两个各自独立的原因：
    #
    #  1. 不能靠 --break-system-packages 绕过 PEP 668。那个开关是 pip 23.0.1
    #     才加的，而 22.04 源里的 python3-pip 是 22.0.2（CI 实跑日志里 apt 装出
    #     的是 python3-pip 22.0.2+dfsg-1ubuntu0.7）——旧 pip 会在**解析参数**阶段
    #     就报 "no such option: --break-system-packages" 退出，根本走不到
    #     PEP 668 那一步。绕过开关在这张镜像上从一开始就不存在。
    #  2. venv 天然不是 externally-managed 的环境，所以无论 22.04 到底带不带
    #     EXTERNALLY-MANAGED 标记，pip 都不会拿 PEP 668 拒绝它。写法不依赖
    #     「这张镜像有没有那个标记」这个我们没在本地核实过的事实。
    #
    # 本段注释刻意不用反引号：整段在下面那个 bash -c "..." 的双引号里，
    # **双引号内的反引号是活的**。写注释时带上反引号，命令替换会在**宿主**
    # 上先跑一遍（run 36247701161 实测：宿主上多出一次
    # "ERROR: You must give at least one requirement to install"，
    # 紧接着 "line 70: syntax error near unexpected token"），
    # 随后真正要执行的 payload 被这段替换污染。引号只表达含义，不做修饰。
    #
    # --system-site-packages：venv 里的 python3 仍能看到系统 site-packages，
    # 免得 meson 构建过程里 shell out 到 python3 时丢掉发行版装的模块。
    python3 -m venv --system-site-packages /opt/echo-build-venv
    /opt/echo-build-venv/bin/pip install --no-cache-dir 'meson>=1.3.0'
    export PATH="/opt/echo-build-venv/bin:\$PATH"
    meson --version
    command -v meson ninja python3 git curl patchelf

    # Node 的官方二进制是自包含的（只要求 glibc >= 2.28），在 2.35 的宿主上直接可用。
    curl -fsSL \"https://nodejs.org/dist/${NODE_VERSION}/node-${NODE_VERSION}-linux-x64.tar.xz\" \
      | tar -xJ -C /usr/local --strip-components=1 --exclude=CHANGELOG.md --exclude=LICENSE --exclude=README.md
    node --version

    node scripts/release/build-linux-libmpv.mjs --work /work --out \"${OUT_REL}\"
  "

echo "==> 容器构建完成：${OUT_REL}"
