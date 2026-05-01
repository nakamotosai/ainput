# Streaming Duplicate Final Merge V5 PLAN

更新时间：2026-04-30

## Phase 1：写入最高优先级 Spec

- 新增 `specs/streaming-duplicate-final-merge-v5/`。
- 在总 `TASKLIST.md` 顶部新增 Round 20。

## Phase 2：修复 final merge

- 修改 `merge_rolled_over_prefix`：
  - 若当前 final candidate 和 rollover prefix 高度相似，且长度不像纯 tail，则视为完整 replacement。
  - 只有当前 candidate 明显只是后续 tail 时，才继续追加 `rolled_over_prefix`。

## Phase 3：测试

- 增加单元测试覆盖：
  - 用户真实失败例：`你最些分辨率有问` + `你这些分辨率有问题。`
  - 正常 tail 例：`你这些分辨率` + `有问题。`
- 运行：
  - `cargo test -p ainput-desktop streaming`
  - 指定 raw replay：`streaming-raw-1777561249467.wav`
  - raw corpus 抽样

## Phase 4：继续加速测试

- 修复重复问题后，再开始模型/参数 latency sweep。
- 模型候选先测中英双语小模型；中文单语模型只能作为参考，不直接作为默认候选。
