//! Turbo直贴实验：按住说话时，把 stable（定稿区）增量直接贴进目标文档，不弹 HUD。
//!
//! 只贴 stable，不碰 provisional：草稿下一拍可能变，贴进文档就得回删，比 HUD 重画贵得多。
//! 容忍度为零：stable 自己 revision / 目标窗口变了 / 粘贴没落盘，
//! 一律停手退回 HUD 老路，松开时再对账，绝不悄悄丢字。

use crate::config::OutputConfig;
use crate::output;
use tracing::{info, warn};

/// 单句直贴超过这么多字就停手：replace 选区越长越不可靠。
const MAX_DIRECT_CHARS: usize = 600;

/// committed=已经落进文档的字，new_stable=边车刚确认的定稿区。
pub enum InsertPlan {
    Noop,
    Append(String),
    /// 定稿区自己变了（缩了/改了）：调用方必须停手，不许猜。
    Revise,
}

pub fn plan_insert(committed: &str, new_stable: &str) -> InsertPlan {
    if new_stable == committed {
        return InsertPlan::Noop;
    }
    if new_stable.starts_with(committed) {
        let tail: String = new_stable.chars().skip(committed.chars().count()).collect();
        if tail.is_empty() {
            InsertPlan::Noop
        } else {
            InsertPlan::Append(tail)
        }
    } else {
        InsertPlan::Revise
    }
}

pub enum TickOutcome {
    Advanced,
    Unchanged,
    Aborted {
        reason: &'static str,
        /// 停手后 HUD 兜底显示的全文（stable+provisional，调用方传进来）。
        fallback: String,
    },
}

pub enum ReconcileOutcome {
    /// 句中一个字都没贴上：调用方走老路整段粘贴。
    NothingInserted,
    Reconciled {
        pasted: String,
    },
    /// 对账失败：文档里留着 stable 片段，fallback 是定稿全文（HUD 显示，不再粘贴，免得复读）。
    Failed {
        fallback: String,
        reason: String,
    },
}

pub struct TurboLiveResult {
    pub released: bool,
    pub text: String,
    pub root: std::path::PathBuf,
    pub elapsed_ms: u128,
    pub direct: Option<DirectSession>,
}

/// 一句话的直贴会话：目标窗口在按下那一刻锁定，句中只增不改。
pub struct DirectSession {
    target: output::OutputTarget,
    output_config: OutputConfig,
    utterance_id: String,
    committed: String,
    live: bool,
}

impl DirectSession {
    /// 按下时调：目标不是可替换文本框（终端/密码框/读不到的）直接回 None，走 HUD 老路。
    pub fn begin(
        allowlist: &[String],
        output_config: &OutputConfig,
        utterance_id: &str,
    ) -> Option<Self> {
        let target = output::capture_output_target(allowlist);
        if target.fingerprint.hwnd == 0 {
            info!(
                utterance_id,
                "turbo直贴实验跳过：抓不到目标窗口，走HUD老路"
            );
            return None;
        }
        match target.context.source {
            "standard_text_control" | "uia_text_pattern" | "uia_text_pattern2" => {}
            other => {
                info!(
                    utterance_id,
                    target_process = %target.summary.process_name,
                    context_source = other,
                    "turbo直贴实验跳过：目标不是可替换文本框，走HUD老路"
                );
                return None;
            }
        }
        info!(
            utterance_id,
            target_process = %target.summary.process_name,
            target_class = %target.summary.class_name,
            context_source = target.context.source,
            "turbo直贴实验开始：句中只贴stable增量"
        );
        Some(Self {
            target,
            output_config: output_config.clone(),
            utterance_id: utterance_id.to_string(),
            committed: String::new(),
            live: true,
        })
    }

    /// 每一拍的新 stable（调用方已做 prepare_asr_text，与落盘口径一致）。
    pub fn offer_stable(&mut self, stable_prepared: &str, full_hypothesis: &str) -> TickOutcome {
        if !self.live {
            return TickOutcome::Unchanged;
        }
        match plan_insert(&self.committed, stable_prepared) {
            InsertPlan::Noop => TickOutcome::Unchanged,
            InsertPlan::Revise => self.abort("stable_revised", full_hypothesis),
            InsertPlan::Append(tail) => {
                if self.committed.chars().count() + tail.chars().count() > MAX_DIRECT_CHARS {
                    return self.abort("span_too_long", full_hypothesis);
                }
                match output::paste_text_to_target_with_trace(
                    &tail,
                    &self.target,
                    &self.output_config,
                    &self.utterance_id,
                    output::TargetPunctuationPolicy::Preserve,
                    output::TargetMatchPolicy::BestEffort,
                ) {
                    // 注意：粘贴函数没贴上也返回 Ok，看 text_actions 里有没有 copy_only_* 才是真凭据。
                    Ok(outcome) if !outcome.text_actions.contains("copy_only") => {
                        self.committed.push_str(&outcome.text);
                        info!(
                            utterance_id = %self.utterance_id,
                            committed_chars = self.committed.chars().count(),
                            "turbo直贴实验：增量已落盘"
                        );
                        TickOutcome::Advanced
                    }
                    Ok(outcome) => self.abort(
                        reason_from_actions(&outcome.text_actions),
                        full_hypothesis,
                    ),
                    Err(error) => {
                        warn!(
                            utterance_id = %self.utterance_id,
                            error = %error,
                            "turbo直贴实验：粘贴报错，停手"
                        );
                        self.abort("paste_error", full_hypothesis)
                    }
                }
            }
        }
    }

    fn abort(&mut self, reason: &'static str, full_hypothesis: &str) -> TickOutcome {
        self.live = false;
        warn!(
            utterance_id = %self.utterance_id,
            reason,
            committed_chars = self.committed.chars().count(),
            "turbo直贴实验：停手，退回HUD，松开时对账"
        );
        TickOutcome::Aborted {
            reason,
            fallback: full_hypothesis.to_string(),
        }
    }

    pub fn committed(&self) -> &str {
        &self.committed
    }

    pub fn target_summary(&self) -> &output::TargetSummary {
        &self.target.summary
    }

    pub fn target_context_source(&self) -> &'static str {
        self.target.context.source
    }

    pub fn target_right_context(&self) -> output::TargetRightContext {
        self.target.context.right
    }

    /// 松开对账：把已落盘的 stable 片段整体换成定稿全文。
    pub fn reconcile(
        &self,
        final_text: &str,
        output_config: &OutputConfig,
        utterance_id: &str,
    ) -> ReconcileOutcome {
        if self.committed.is_empty() {
            return ReconcileOutcome::NothingInserted;
        }
        let want = if final_text.trim().is_empty() {
            // 定稿空了就保留已贴的：总比把用户眼前的字吞了强。
            self.committed.clone()
        } else {
            final_text.to_string()
        };
        let outcome = output::replace_recent_paste_with_trace(
            &self.committed,
            &want,
            &self.target.fingerprint,
            output_config,
            utterance_id,
        );
        if outcome.applied {
            info!(
                utterance_id,
                pasted_chars = want.chars().count(),
                "turbo直贴实验：松开对账成功"
            );
            ReconcileOutcome::Reconciled { pasted: want }
        } else {
            warn!(
                utterance_id,
                reason = %outcome.reason,
                "turbo直贴实验：松开对账失败，文档里留着stable片段，HUD兜底显示定稿"
            );
            ReconcileOutcome::Failed {
                fallback: want,
                reason: outcome.reason,
            }
        }
    }
}

fn reason_from_actions(actions: &str) -> &'static str {
    if actions.contains("copy_only_foreground_changed") {
        "foreground_changed"
    } else if actions.contains("copy_only_modifier_still_down") {
        "modifier_down"
    } else if actions.contains("copy_only_preflight_blocked") {
        "preflight_blocked"
    } else if actions.contains("copy_only") {
        "copy_only_fallback"
    } else {
        "paste_unverified"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plan(committed: &str, new_stable: &str) -> String {
        match plan_insert(committed, new_stable) {
            InsertPlan::Noop => "noop".to_string(),
            InsertPlan::Append(tail) => format!("append:{tail}"),
            InsertPlan::Revise => "revise".to_string(),
        }
    }

    #[test]
    fn empty_committed_takes_whole_stable() {
        assert_eq!(plan("", "你好"), "append:你好");
    }

    #[test]
    fn identical_is_noop() {
        assert_eq!(plan("你好", "你好"), "noop");
    }

    #[test]
    fn growth_appends_tail_only() {
        assert_eq!(plan("你好", "你好世界"), "append:世界");
        assert_eq!(plan("I'm ", "I'm alone"), "append:alone");
    }

    #[test]
    fn shrink_or_rewrite_is_revise() {
        assert_eq!(plan("你好世界", "你好"), "revise");
        assert_eq!(plan("你好", "您好"), "revise");
    }

    #[test]
    fn emoji_boundary_stays_valid() {
        assert_eq!(plan("a🤣", "a🤣b"), "append:b");
        assert_eq!(plan("a🤣", "a😭"), "revise");
    }
}
