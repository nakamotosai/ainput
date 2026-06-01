# Streaming Final Repair Budget V7 RESULTS

更新时间：2026-05-01

## 结果

已完成本轮三条：

- 长音频 release 后不再整段同步跑 offline final。
- 超预算或无法可靠合并时 fallback 到 streaming final / HUD 文本。
- 长句只对尾部窗口做 final repair，尾部无重叠不合并。

## 关键测速

报告：`tmp\streaming-latency-benchmark\20260501-014239-785`

- `sentence_combo_long`：`offline_final_elapsed_ms = 164ms`
- v6 基线同类长句约 `1038ms`
- 本轮长句 release 后 final repair 耗时明显下降

## 已通过

- `cargo fmt --check`
- `cargo check -p ainput-desktop`
- `cargo test -p ainput-desktop offline_final -- --nocapture`：2/2 pass
- `cargo test -p ainput-desktop streaming -- --nocapture`：31/31 pass
- `cargo test -p ainput-output`：9/9 pass
- `cargo test -p ainput-rewrite`：16/16 pass
- `cargo test -p ainput-shell`：6/6 pass
- `scripts\run-streaming-selftest.ps1`：6/6 pass
- 包内 startup idle：`tmp\startup-idle-acceptance\20260501-015003-544`，pass
- 包内 synthetic live E2E：`tmp\streaming-live-e2e\20260501-015017-595`，3/3 pass
- 包内 wav live E2E：`tmp\streaming-live-e2e\20260501-015159-399`，3/3 pass

## 打包

- 成功包：`dist\ainput-1.0.0-preview.37`
- 成功 zip：`dist\ainput-1.0.0-preview.37.zip`
- `preview.36` 在压缩阶段遇到 Windows 临时 exe 文件占用，已改名为 failed 半成品目录，不作为可用包。
- 当前已启动：`dist\ainput-1.0.0-preview.37\ainput-desktop.exe`

## 未覆盖 / 残留

- raw corpus 使用 `preview.32` 保存的真实录音抽样回放时，短样本失败为 `raw_tail_late`：final 比最后一个 HUD partial 多 2 个内容字。
- 该问题属于“松手前 HUD 尾部追帧/首字首段实时性”链路，不属于本轮三条 release 后 final repair 预算分流；下一轮单独处理。
