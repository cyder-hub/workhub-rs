use std::{
    panic::{PanicHookInfo, take_hook},
    sync::Arc,
};

use crate::{
    observability::{
        rotation::RUN_LOG_FILE,
        schema::{ErrorDiagnosticEnvelope, LogKind, PayloadPolicy},
        sinks::{emit_global_event, global_context},
    },
    operations::error::OperationErrorCategory,
};

type PanicHook = dyn Fn(&PanicHookInfo<'_>) + Sync + Send + 'static;

pub(crate) struct PanicHookGuard {
    previous: Option<Arc<PanicHook>>,
}

impl PanicHookGuard {
    pub(crate) fn install() -> Self {
        let previous = take_hook();
        let previous: Arc<PanicHook> = Arc::from(previous);
        let hook_previous = previous.clone();
        std::panic::set_hook(Box::new(move |info| {
            emit_panic_event(info);
            hook_previous(info);
        }));

        Self {
            previous: Some(previous),
        }
    }
}

impl Drop for PanicHookGuard {
    fn drop(&mut self) {
        if std::thread::panicking() {
            return;
        }
        if let Some(previous) = self.previous.take() {
            std::panic::set_hook(Box::new(move |info| previous(info)));
        }
    }
}

fn emit_panic_event(info: &PanicHookInfo<'_>) {
    let Some(context) = global_context() else {
        return;
    };
    let location = info
        .location()
        .map(|location| format!("{}:{}", location.file(), location.line()))
        .unwrap_or_else(|| "unknown".to_string());
    let payload = info
        .payload()
        .downcast_ref::<&str>()
        .map(|value| (*value).to_string())
        .or_else(|| info.payload().downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "panic payload is not a string".to_string());
    let envelope = ErrorDiagnosticEnvelope {
        timestamp_utc: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
        kind: LogKind::Panic,
        event: "panic.captured".to_string(),
        message: "panic captured".to_string(),
        target: "workhub::panic".to_string(),
        mode: context.mode.clone(),
        version: context.version.clone(),
        run_id: context.run_id.clone(),
        pid: context.pid,
        correlation: Default::default(),
        provider: None,
        operation: None,
        tool_name: None,
        command_path: None,
        duration_ms: None,
        exit_code: Some(101),
        error_category: OperationErrorCategory::Business,
        error_kind: "panic".to_string(),
        error_message: payload,
        phase: "panic_hook".to_string(),
        cause_summary: format!("panic at {location}"),
        cause_chain: vec![format!("panic location {location}")],
        impact: "process is unwinding after panic".to_string(),
        remediation_action: "report_panic_with_support_bundle".to_string(),
        remediation_evidence: "include workhub logs bundle --since 24h".to_string(),
        related_log_file: RUN_LOG_FILE.to_string(),
        related_line_hint: format!("run_id={}", context.run_id),
        support_bundle_hint: "workhub logs bundle --since 24h".to_string(),
        payload_policy: PayloadPolicy::Metadata,
        fields: Default::default(),
    };
    emit_global_event(&envelope.to_log_event());
}
