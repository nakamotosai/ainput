# ainput WORKFLOW

## 默认工作流

1. 读取 [AGENTS.md](AGENTS.md)
2. 读取 [SPEC.md](SPEC.md)
3. 读取 [PLAN.md](PLAN.md)
4. 读取 [TASKLIST.md](TASKLIST.md)
5. 根据当前轮次开始实施
6. 实施后更新：
   - `TASKLIST.md`
   - `OPLOG.md`
   - 如有路线变化则更新 `DECISIONS.md`

## 每轮执行要求

- 开始前：
  - 说明本轮目标
  - 说明准备勾掉哪些任务
- 实施中：
  - 不做与当前轮次无关的扩张
- 结束后：
  - 勾选已完成任务
  - 记录验证
  - 回写操作日志

## 文档更新规则

- 改技术路线：更新 `DECISIONS.md`
- 改目标和边界：更新 `SPEC.md`
- 改实施顺序：更新 `PLAN.md`
- 改执行状态：更新 `TASKLIST.md`
- 改每日推进记录：更新 `OPLOG.md`

## 推荐提交节奏

- 一轮一个可验证增量
- 先 `cargo check`
- 再最小本地验证
- 再回写日志

## 禁止事项

- 不要在未更新 `TASKLIST.md` 的情况下声称完成一轮
- 不要引入 Python 到最终常驻链路，除非 `DECISIONS.md` 明确变更
- 不要为了“以后可能用到”提前塞复杂抽象
