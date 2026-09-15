# 平台 Gate 交接清单（0.1.0 人工/跨平台部分）

本文由 `docs/acceptance/PRD-matrix.md` 引用：PRD A1–A14 的 macOS 可自动执行部分由场景命令直接证明，
下列 `tests/native/` 场景是真实的多平台 OS 交互（关机行为、托盘、单实例、文件打开唤醒、watcher 权限撤销等），
需要在目标平台上按各 manifest 的人工步骤执行并记录操作者与证据路径。

运行方式：
1. 在目标平台构建并启动应用（`dev.sh` 或打包产物）。
2. 打开对应 manifest，按「人工步骤」逐项执行。
3. 记录平台版本、操作者与证据（截图/日志）路径到 manifest 末尾的证据区。

| 场景 ID | 场景 | 平台 | manifest |
|---|---|---|---|
| `DAS-R04-S01` |  |  | `tests/native/DAS-R04-S01.md` |
| `DAS-R04-S02` |  |  | `tests/native/DAS-R04-S02.md` |
| `DAS-R04-S03` |  |  | `tests/native/DAS-R04-S03.md` |
| `DAS-R05-S01` |  |  | `tests/native/DAS-R05-S01.md` |
| `DAS-R05-S02` |  |  | `tests/native/DAS-R05-S02.md` |
| `DAS-R05-S03` |  |  | `tests/native/DAS-R05-S03.md` |
| `DAS-R06-S01` |  |  | `tests/native/DAS-R06-S01.md` |
| `DAS-R06-S02` |  |  | `tests/native/DAS-R06-S02.md` |
| `DAS-R06-S03` |  |  | `tests/native/DAS-R06-S03.md` |
| `DAS-R09-S01` |  |  | `tests/native/DAS-R09-S01.md` |
| `DAS-R09-S02` |  |  | `tests/native/DAS-R09-S02.md` |
| `DP-R01-S01` |  |  | `tests/native/DP-R01-S01.md` |
| `DP-R01-S02` |  |  | `tests/native/DP-R01-S02.md` |
| `DP-R05-S01` |  |  | `tests/native/DP-R05-S01.md` |
| `DP-R05-S02` |  |  | `tests/native/DP-R05-S02.md` |
| `DP-R08-S01` |  |  | `tests/native/DP-R08-S01.md` |
| `DP-R08-S02` |  |  | `tests/native/DP-R08-S02.md` |
| `DP-R09-S01` |  |  | `tests/native/DP-R09-S01.md` |
| `DP-R09-S02` |  |  | `tests/native/DP-R09-S02.md` |
| `DP-R09-S03` |  |  | `tests/native/DP-R09-S03.md` |
| `IL-R09-S02` |  |  | `tests/native/IL-R09-S02.md` |
| `LE-R05-S01` |  |  | `tests/native/LE-R05-S01.md` |
| `LE-R05-S02` |  |  | `tests/native/LE-R05-S02.md` |
| `LE-R05-S03` |  |  | `tests/native/LE-R05-S03.md` |
| `LE-R05-S04` |  |  | `tests/native/LE-R05-S04.md` |
| `LE-R05-S05` |  |  | `tests/native/LE-R05-S05.md` |
| `LE-R05-S06` |  |  | `tests/native/LE-R05-S06.md` |
| `LL-R03-S01` |  |  | `tests/native/LL-R03-S01.md` |
| `LL-R03-S02` |  |  | `tests/native/LL-R03-S02.md` |
| `LL-R03-S03` |  |  | `tests/native/LL-R03-S03.md` |
| `LL-R07-S01` |  |  | `tests/native/LL-R07-S01.md` |
| `LL-R07-S02` |  |  | `tests/native/LL-R07-S02.md` |
| `LL-R07-S03` |  |  | `tests/native/LL-R07-S03.md` |
| `LL-R08-S01` |  |  | `tests/native/LL-R08-S01.md` |
| `LL-R08-S02` |  |  | `tests/native/LL-R08-S02.md` |
| `LL-R08-S03` |  |  | `tests/native/LL-R08-S03.md` |
| `SFI-R06-S01` |  |  | `tests/native/SFI-R06-S01.md` |
| `SFI-R06-S02` |  |  | `tests/native/SFI-R06-S02.md` |
| `SFI-R06-S03` |  |  | `tests/native/SFI-R06-S03.md` |
| `SFI-R06-S04` |  |  | `tests/native/SFI-R06-S04.md` |
| `SFI-R06-S05` |  |  | `tests/native/SFI-R06-S05.md` |
| `SFI-R07-S01` |  |  | `tests/native/SFI-R07-S01.md` |
| `SFI-R07-S02` |  |  | `tests/native/SFI-R07-S02.md` |
| `SFI-R08-S01` |  |  | `tests/native/SFI-R08-S01.md` |
| `SFI-R08-S02` |  |  | `tests/native/SFI-R08-S02.md` |
