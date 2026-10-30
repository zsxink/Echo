## 1. 修复

- [x] 1.1 `apps/desktop/src-tauri/src/sync_runtime.rs` 的 `retry_restore()`:`Busy` 检查后调用 `self.cancel_restore_retry()`(推进 generation、唤醒并 join 旧 worker、清尝试计账),随后按原序设置 `state=Disabled`、`last_error=None`、`event_sequence += 1`;函数注释说明「显式重试=用户转换,须与 stop/pause 同级交接」
- [x] 1.2 验证:`cargo test -p echo-app --bin Echo sync_runtime::tests::session_intent` 全过;目标测试循环 ≥50 次无失败;`cargo fmt --all -- --check` 与 `cargo clippy -p echo-app --all-features` 干净

## 2. 收尾

- [x] 2.1 循环运行后跑一次 `cargo test --workspace --all-features`(与 CI 的 Rust job 同口径)
- [x] 2.2 归档 change、同步 delta spec、提 PR 关联 issue #96
