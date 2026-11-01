# 原生场景人工举证模板

> **本文件不被任何门禁解析。** 门禁只读
> `artifacts/native-attestations/<ID>.log`（`scripts/verify/checks/check-native-attestation.mjs`）。
> 人工步骤与契约说明见 `docs/native-attestation-playbook.md` §5/§6。

用法：把下面的骨架复制为 `artifacts/native-attestations/<ID>.log`，其中 `<ID>` 是场景 ID
（例如 `DAS-R12-S01`），在目标平台上按 playbook §5 的步骤执行后逐项填写。

```
os: (例如 macOS 15.2 / Windows 11 23H2 / Ubuntu 22.04)
versions: (Echo 版本 + 相关依赖版本)
desktop-env: (桌面环境、显示器、缩放)
operator: (执行人)
result: pass
evidence-path: artifacts/native-attestations/<ID>.log
date: (YYYY-MM-DD)
```

约束：

- 7 个字段缺一不可，字段清单的唯一实现是 `check-native-attestation.mjs` 的
  `REQUIRED_FIELDS`。
- `evidence-path` 必须指向该场景自己的日志（`artifacts/native-attestations/<ID>.log`）。
- `result` 必须以 `pass` 或 `ok` 开头。
- 命令之外没有第二种人工/自动分类来源：一个场景是否需要人工举证，只由它的命令是否
  调用 `check-native-attestation.mjs` 决定。
