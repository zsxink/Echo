# Tasks — 记录歌曲入库时间并随资料库恢复

实现 `record-imported-at` change。领域事实:`Song::added_at` 与 SQLite `songs.added_at` 已存在,缺口在于①可携带 `SongRecord` 未携带该字段,②恢复/接续投影重建歌曲时 `added_at` 归零。

命令均在仓库根 `/Users/xian/Project/music/Echo` 执行。

## 1. 领域模型:可携带歌曲资料携带入库时刻

- [x] 1.1 在 `crates/echo-core/src/domain/library.rs` 的 `SongRecord` 新增字段 `pub added_at: u64`(墙钟毫秒,与 `Song::added_at` 同义),并加 `#[serde(default)]` 使历史记录缺字段时反序列化为 `0`;执行 `cargo test -p echo-core domain::library` 编译通过
- [x] 1.2 用一次全仓 grep 枚举所有 `SongRecord {` / `PortableRecord::Song(SongRecord {` 构造点(生产与测试),确认哪些是生产调用、哪些是测试 fixture;执行 `grep -rn "SongRecord {" crates/` 得到清单

## 2. 写入:对象资料生成时携带入库时刻

- [x] 2.1 在 `crates/echo-core/src/application/portable_materialize.rs::song_record()` 填充 `added_at: song.added_at()`,与 `content_hash` 同一来源(`&Song`);补充/更新该函数测试断言构造的记录携带 `added_at` 等于入参实体;执行 `cargo test -p echo-core application::portable_materialize` 且新断言通过
- [x] 2.2 核对生产写入路径均经 `song_record()`(扫描/导入/recover),并对 import/execution 的歌曲记录断言其 `added_at.is_some()`(≈非缺省);执行 `cargo test -p echo-core application::import` 通过

## 3. 读取与构造点补字段

- [x] 3.1 更新 `SongRecord` 全仓结构化构造点补 `added_at: <值>`(生产 `portable_materialize` 已覆盖;余下为测试 fixture:`portable.rs`、`control_plane.rs`、`restore.rs`、`root_switch.rs`、`scan_fixture.rs`、`continuation/tests.rs`、`library.rs` 自身测试),fixture 用显式非零值以暴露误用;执行 `cargo build -p echo-core` 无编译错误,`cargo test -p echo-core` 全绿
- [x] 3.2 添加一个「老格式记录(不含 `added_at`)反序列化为 `0`」的回归测试(可放 `backward_compatible` 测试,生成 JSON 时字符串去掉该字段);执行 `cargo test -p echo-core` 通过

## 4. 恢复/接续:重置本地歌曲时还原本库时刻

- [x] 4.1 在 `crates/echo-core/src/application/continuation.rs` 重建歌曲处,把 `Song::new(id, root, path, Revision::INITIAL)` 改为 `Song::with_added_at(id, root, path, Revision::INITIAL, song_record.added_at)`(记录缺省为 0 = 旧行为兜底);执行 `cargo build -p echo-core` 通过
- [x] 4.2 更新/新增续接投影测试:断言从携带非零 `added_at` 的 `SongRecord` 恢复后,本地 `song.added_at()` 等于记录的库时刻,「最近添加」(按 added_at desc)顺序与源记录一致;执行 `cargo test -p echo-core application::continuation` 通过
- [x] 4.3 补充恢复(restore)路径断言:新设备从对象资料恢复时,不可用歌曲也保留对象资料中的入库时刻;执行 `cargo test -p echo-core application::restore` 通过

## 5. 兼容性与整体验证

- [x] 5.1 确认旧 reader 读新字段不炸(领域类型无 `deny_unknown_fields`,序列化 camel/snake 行为不变):在 `continuation`/`portable` 测试里对含 `added_at` 的 JSON 反序列化成功;执行 `cargo test -p echo-core` 通过
- [x] 5.2 全仓 grep 确认无遗漏的 `SongRecord` 构造点与 `song_from_parsed`/`with_added_at` 使用点保持与设计一致;执行 `grep -rn "added_at" crates/echo-core/src/` 复查
- [x] 5.3 运行完整 Rust 工作区 lint + 测试(设计约束:Cargo fmt、clippy、全部 echo-core 测试);执行 `cargo fmt --check && cargo clippy -p echo-core --all-targets` 与 `cargo test -p echo-core` 全绿